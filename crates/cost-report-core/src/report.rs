//! The department report: OpenCost allocations shaped for the page, plus
//! the checks that decide whether they can be billed.
//!
//! The server does the I/O (OpenCost, Prometheus, the snapshotter) and
//! hands the answers to [`assemble`]; everything here is computed from
//! those answers only, so it can be tested without a cluster.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use crate::billing::{self, GateInput, RateCard, Rates, Thresholds};
use crate::num::{f, field, r4, round, GIB, MIB};

/// What the report needs to know about the organisation.
#[derive(Debug, Clone)]
pub struct ReportConfig {
    /// Display name of the cluster.
    pub cluster: String,
    /// Departments in display order with their names. Departments found
    /// in the data but not listed here come after, by ID, named by ID.
    pub departments: Vec<(String, String)>,
    /// Environment label values with their own column (e.g. prod, dev);
    /// every other value is "other".
    pub environments: Vec<String>,
    /// Namespace names or prefixes of platform components: not offered
    /// as "label this for a department", and not listed as workloads
    /// unless they carry a department label.
    pub system_namespaces: Vec<String>,
    pub thresholds: Thresholds,
}

impl ReportConfig {
    pub fn is_system(&self, ns: &str) -> bool {
        self.system_namespaces
            .iter()
            .any(|p| ns == p || ns.starts_with(p.as_str()))
    }

    pub fn name_of(&self, id: &str) -> String {
        self.departments
            .iter()
            .find(|(d, _)| d == id)
            .map(|(_, n)| n.clone())
            .unwrap_or_else(|| id.to_string())
    }

    fn order_of(&self, id: &str) -> usize {
        self.departments
            .iter()
            .position(|(d, _)| d == id)
            .unwrap_or(usize::MAX)
    }
}

/// The five allocation queries behind one report.
#[derive(Debug, Clone, Default)]
pub struct Allocations {
    /// By department, idle separate.
    pub by_cc: Value,
    /// By department, idle shared out proportionally.
    pub by_cc_shared: Value,
    /// By department and environment.
    pub by_cc_env: Value,
    /// By controller, department and environment.
    pub by_ctrl: Value,
    /// By namespace.
    pub by_ns: Value,
}

fn entries(v: &Value) -> impl Iterator<Item = (&String, &Value)> {
    v.as_object().into_iter().flatten()
}

fn departments_of(v: &Value) -> impl Iterator<Item = (&String, &Value)> {
    entries(v).filter(|(k, _)| !k.starts_with("__"))
}

/// Start, end and minutes of the window OpenCost actually covered, from
/// any department row.
pub fn window_of(by_cc: &Value) -> (Option<String>, Option<String>, f64) {
    match departments_of(by_cc).next() {
        Some((_, v)) => (
            v.get("start").and_then(Value::as_str).map(str::to_string),
            v.get("end").and_then(Value::as_str).map(str::to_string),
            field(v, "minutes"),
        ),
        None => (None, None, 0.0),
    }
}

/// Prices implied by the data: cost ÷ usage, so the price lives in one
/// place (OpenCost) and changing it needs no code change.
pub fn implied_rates(by_cc: &Value) -> Rates {
    Rates {
        cpu: billing::implied_rate(by_cc, "cpuCost", "cpuCoreHours", 1.0),
        ram: billing::implied_rate(by_cc, "ramCost", "ramByteHours", GIB),
        storage: billing::implied_rate(by_cc, "pvCost", "pvByteHours", GIB),
    }
}

/// Prices to use for estimates: the rate card version in force at the
/// window's start, falling back to implied prices (the page says which).
pub fn effective_rates<'a>(
    card: Option<&'a RateCard>,
    w_start: Option<&str>,
    implied: Rates,
) -> (Rates, Option<&'a Value>) {
    let version = card.and_then(|c| c.rate_at(w_start));
    match version {
        Some(v) => (Rates::from_version(v).or(implied), Some(v)),
        None => (implied, None),
    }
}

/// A gap filled from the snapshotter, or why it could not be.
#[derive(Debug, Clone)]
pub enum GapAnswer {
    /// The snapshotter's `/allocation` document.
    Found(Value),
    /// HTTP 404: no snapshots for this range.
    NoSnapshot,
    /// Could not ask.
    Unreachable(String),
}

/// Cost of gaps from declared requests × the price in force.
///
/// Declared requests are what really existed during the gap (read from the
/// API server), not an average of the neighbours, so the estimate has a
/// stated basis. It is still an estimate and is labelled as one.
pub fn declared_fill(gaps: &[(String, String, GapAnswer)], rates: &Rates) -> Option<Value> {
    if gaps.is_empty() {
        return None;
    }
    let mut by_dept: BTreeMap<String, f64> = BTreeMap::new();
    let mut notes = Vec::new();
    let mut covered: Vec<f64> = Vec::new();
    for (start, end, answer) in gaps {
        match answer {
            GapAnswer::NoSnapshot => {
                notes.push(json!({ "start": start, "end": end, "code": "no_snapshot" }))
            }
            GapAnswer::Unreachable(e) => notes
                .push(json!({ "start": start, "end": end, "code": "unreachable", "detail": e })),
            GapAnswer::Found(doc) => {
                if let Some(c) = doc.get("snapshotCoveragePct").and_then(Value::as_f64) {
                    covered.push(c);
                }
                for (dept, v) in entries(doc.get("departments").unwrap_or(&Value::Null)) {
                    let cost = field(v, "cpuCoreHours") * rates.cpu.unwrap_or(0.0)
                        + field(v, "ramGiBHours") * rates.ram.unwrap_or(0.0)
                        + field(v, "pvGiBHours") * rates.storage.unwrap_or(0.0);
                    let e = by_dept.entry(dept.clone()).or_insert(0.0);
                    *e = r4(*e + cost);
                }
            }
        }
    }
    if by_dept.is_empty() && notes.is_empty() {
        return None;
    }
    Some(json!({
        "total": r4(by_dept.values().sum()),
        "byDept": by_dept,
        "basis": "declared",
        // Filling an incomplete record from another incomplete record hides
        // errors better, so the backup's own completeness is shown.
        "snapshotCoveragePct": covered.iter().cloned().reduce(f64::min).map(|c| round(c, 2)),
        "notes": notes,
    }))
}

/// Asset cost (nodes, disks) of a window, for reconciliation.
#[derive(Debug, Clone, Default)]
pub struct Assets {
    pub total: f64,
    pub by_type: Map<String, Value>,
    /// Minutes the node assets actually cover; OpenCost often covers less
    /// of the window for assets than for allocations.
    pub minutes: Option<f64>,
}

/// Everything besides the allocations, gathered by the server.
#[derive(Debug, Clone, Default)]
pub struct Extras {
    pub coverage: Option<Value>,
    pub fill: Option<Value>,
    /// Asset cost total and by type.
    pub assets: Option<Assets>,
    pub targets: Option<Vec<Value>>,
    pub series: Option<Value>,
    pub storage: Value,
    /// Minutes of the window asked of OpenCost (CPU usage averages are
    /// divided by it, see `num::cpu_usage`).
    pub requested_minutes: Option<f64>,
}

/// Build the report.
pub fn assemble(
    cfg: &ReportConfig,
    a: &Allocations,
    window: &str,
    card: Option<&RateCard>,
    x: &Extras,
) -> Value {
    let get_total = |v: &Value, k: &str| v.get(k).map(|r| field(r, "totalCost")).unwrap_or(0.0);
    let idle = r4(get_total(&a.by_cc, "__idle__"));
    let unalloc_sep = r4(get_total(&a.by_cc, "__unallocated__"));
    let unalloc_shr = r4(get_total(&a.by_cc_shared, "__unallocated__"));

    let mut depts: Vec<Value> = departments_of(&a.by_cc)
        .map(|(key, v)| {
            json!({
                "id": key,
                "name": cfg.name_of(key),
                "cpu": r4(field(v, "cpuCost")), "ram": r4(field(v, "ramCost")), "pv": r4(field(v, "pvCost")),
                "total": r4(field(v, "totalCost")),
                "shared": r4(get_total(&a.by_cc_shared, key)),
                "cpuCoreHours": r4(field(v, "cpuCoreHours")),
                "ramGiBHours": r4(field(v, "ramByteHours") / GIB),
                "pvGiBHours": r4(field(v, "pvByteHours") / GIB),
            })
        })
        .collect();
    depts.sort_by(|x, y| {
        let (ix, iy) = (
            x["id"].as_str().unwrap_or(""),
            y["id"].as_str().unwrap_or(""),
        );
        cfg.order_of(ix).cmp(&cfg.order_of(iy)).then(ix.cmp(iy))
    });

    // Department × environment.
    let mut env_rows: BTreeMap<String, (Map<String, Value>, f64)> = BTreeMap::new();
    for (key, v) in entries(&a.by_cc_env) {
        let (cc, env) = key.split_once('/').unwrap_or((key, ""));
        if cc.starts_with("__") {
            continue;
        }
        let row = env_rows.entry(cc.to_string()).or_insert_with(|| {
            (
                cfg.environments
                    .iter()
                    .map(|e| (e.clone(), json!(0.0)))
                    .collect(),
                0.0,
            )
        });
        let cost = field(v, "totalCost");
        if cfg.environments.iter().any(|e| e == env) {
            let prev = f(row.0.get(env));
            row.0.insert(env.into(), json!(r4(prev + cost)));
        } else {
            row.1 = r4(row.1 + cost);
        }
    }
    let mut cc_env: Vec<(String, Value)> = env_rows
        .into_iter()
        .map(|(cc, (envs, other))| {
            (
                cc.clone(),
                json!({ "dept": cc, "envs": envs, "other": other }),
            )
        })
        .collect();
    cc_env.sort_by(|(x, _), (y, _)| cfg.order_of(x).cmp(&cfg.order_of(y)).then(x.cmp(y)));

    // Workloads.
    let mut workloads: Vec<Value> = Vec::new();
    for (key, v) in entries(&a.by_ctrl) {
        let name = v.get("name").and_then(Value::as_str).unwrap_or(key);
        let name = name.split('/').next().unwrap_or(name);
        let Some((kind, wname)) = name.split_once(':') else {
            continue;
        };
        if !matches!(kind, "deployment" | "statefulset" | "daemonset") {
            continue;
        }
        let parts: Vec<&str> = key.split('/').collect();
        let ns = v
            .pointer("/properties/namespace")
            .and_then(Value::as_str)
            .unwrap_or("");
        let cc = (parts.len() > 2)
            .then(|| parts[1])
            .filter(|c| !c.starts_with("__"));
        let env = (parts.len() > 2)
            .then(|| parts[2])
            .filter(|e| !e.starts_with("__"));
        // System components stay out unless labelled for a department
        // (cross-department charging needs those).
        if cc.is_none() && cfg.is_system(ns) {
            continue;
        }
        workloads.push(json!({
            "name": wname, "kind": kind, "ns": ns, "dept": cc, "env": env,
            "total": r4(field(v, "totalCost")),
            "cpuReq": round(field(v, "cpuCoreRequestAverage"), 3),
            "cpuUse": round(crate::num::cpu_usage(v, x.requested_minutes), 3),
            "ramReqMi": (field(v, "ramByteRequestAverage") / MIB).round(),
            "ramUseMi": (field(v, "ramByteUsageAverage") / MIB).round(),
            // No request, no efficiency: OpenCost reports 1.0 there, which
            // reads as "perfect" for a workload using capacity it never asked for.
            "cpuEff": crate::num::cpu_efficiency(v, x.requested_minutes).map(|e| round(e, 3)),
            "ramEff": (field(v, "ramByteRequestAverage") > 0.0).then(|| round(field(v, "ramEfficiency"), 3)),
        }));
    }
    workloads.sort_by(|x, y| {
        f(y.get("total"))
            .partial_cmp(&f(x.get("total")))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Namespaces whose cost reached no department. System components are
    // kept apart from unlabelled applications: the first need a sharing
    // rule (platform absorbs or spreads it), only the second need labels.
    // Mixing them produced advice like "label kube-system for manufacturing".
    let dept_ns: std::collections::BTreeSet<&str> = workloads
        .iter()
        .filter(|w| !w["dept"].is_null())
        .filter_map(|w| w["ns"].as_str())
        .collect();
    let mut unalloc_rows: Vec<Value> = entries(&a.by_ns)
        .filter(|(k, _)| !k.starts_with("__") && !dept_ns.contains(k.as_str()))
        .map(|(k, v)| json!({ "ns": k, "total": r4(field(v, "totalCost")), "system": cfg.is_system(k) }))
        .collect();
    unalloc_rows.sort_by(|x, y| {
        f(y.get("total"))
            .partial_cmp(&f(x.get("total")))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let (w_start, w_end, minutes) = window_of(&a.by_cc);
    let measured_total = r4(entries(&a.by_cc).map(|(_, v)| field(v, "totalCost")).sum());
    let implied = implied_rates(&a.by_cc);
    let (rates, version) = effective_rates(card, w_start.as_deref(), implied);

    // Add the gap estimates to each department and record how much is estimated.
    let fill_by = |id: &str| {
        x.fill
            .as_ref()
            .and_then(|fl| fl.pointer(&format!("/byDept/{id}")))
            .map(|v| f(Some(v)))
            .unwrap_or(0.0)
    };
    for d in depts.iter_mut() {
        let id = d["id"].as_str().unwrap_or("").to_string();
        let est = r4(fill_by(&id));
        let measured = f(d.get("total"));
        d["measured"] = json!(measured);
        d["estimated"] = json!(est);
        d["total"] = json!(r4(measured + est));
        d["shared"] = json!(r4(f(d.get("shared")) + est));
        d["basis"] = json!(if est <= 0.0 { "measured" } else { "mixed" });
    }
    let est_unalloc = r4(fill_by("__unallocated__"));
    let estimated_total = r4(x.fill.as_ref().map(|fl| field(fl, "total")).unwrap_or(0.0));
    let total = r4(measured_total + estimated_total);
    let estimated_pct = if total != 0.0 {
        round(estimated_total / total * 100.0, 2)
    } else {
        0.0
    };

    // Compare like with like: when OpenCost priced the nodes over a shorter
    // span than it allocated (it does, right after it starts and at the
    // edges of a window), the asset cost is scaled to the allocation's
    // minutes. Comparing totals then reported +117 % drift that was only
    // 4 minutes of assets against 9 of allocation.
    let asset_minutes = x
        .assets
        .as_ref()
        .and_then(|a| a.minutes)
        .filter(|m| *m > 0.0);
    let scaled = match (x.assets.as_ref(), asset_minutes) {
        (Some(a), Some(am)) if minutes > 0.0 && ((am - minutes) / minutes).abs() > 0.01 => {
            Some(a.total * minutes / am)
        }
        _ => None,
    };
    let recon_base = billing::reconcile(
        total,
        scaled.or(x.assets.as_ref().map(|a| a.total)),
        &cfg.thresholds,
    );
    // The share of each resource in the cost: a resource with tiny cost
    // has an unreliable implied price and must not block the bill.
    let cost_by: Vec<(&str, f64)> = [
        ("cpu", "cpuCost"),
        ("ram", "ramCost"),
        ("storage", "pvCost"),
    ]
    .iter()
    .map(|(k, col)| {
        (
            *k,
            departments_of(&a.by_cc).map(|(_, v)| field(v, col)).sum(),
        )
    })
    .collect();
    let res_sum: f64 = cost_by.iter().map(|(_, v)| v).sum();
    let share: Vec<(&str, Option<f64>)> = cost_by
        .iter()
        .map(|(k, v)| (*k, (res_sum != 0.0).then(|| v / res_sum)))
        .collect();
    let rate_status =
        version.map(|v| billing::rate_check(&Rates::from_version(v), &implied, &share, 0.5));
    let unalloc_pct = if total != 0.0 {
        round((unalloc_sep + est_unalloc) / total * 100.0, 2)
    } else {
        0.0
    };
    let gate = billing::billing_gate(
        &GateInput {
            has_data: total > 0.0,
            coverage_pct: x
                .coverage
                .as_ref()
                .and_then(|c| c.get("pct"))
                .and_then(Value::as_f64),
            estimated_pct,
            reconciliation: &recon_base,
            rates: rate_status.as_ref(),
            rate_card: card.is_some(),
            unallocated_pct: unalloc_pct,
        },
        &cfg.thresholds,
    );
    let mut recon = recon_base.clone();
    recon["byType"] = json!(x
        .assets
        .as_ref()
        .map(|a| a.by_type.clone())
        .unwrap_or_default());
    recon["allocatedMinutes"] = json!(round(minutes, 1));
    recon["assetsMinutes"] = json!(asset_minutes.map(|m| round(m, 1)));
    if let (Some(s), Some(a)) = (scaled, x.assets.as_ref()) {
        recon["assetsMeasured"] = json!(r4(a.total));
        recon["assets"] = json!(r4(s));
        recon["scaled"] = json!(true);
    }

    json!({
        "window": { "query": window, "start": w_start, "end": w_end, "minutes": round(minutes, 1) },
        "cluster": cfg.cluster,
        "coverage": x.coverage,
        "targets": x.targets,
        "series": x.series,
        "rates": rates.to_json(),
        "ratesImplied": implied.to_json(),
        "rateCard": {
            "loaded": card.is_some(),
            "currency": card.and_then(|c| c.currency.clone()),
            "note": card.and_then(|c| c.note.clone()),
            "version": version,
            "versions": card.map(|c| c.versions.clone()).unwrap_or_default(),
            "check": rate_status.unwrap_or_else(|| json!({})),
        },
        "depts": depts,
        "idle": idle,
        "unallocated": { "separate": r4(unalloc_sep + est_unalloc), "shared": unalloc_shr },
        "total": total,
        "measuredTotal": measured_total,
        "basis": { "estimatedTotal": estimated_total, "estimatedPct": estimated_pct, "fill": x.fill },
        "reconciliation": recon,
        "gate": gate,
        "ccEnv": cc_env.into_iter().map(|(_, v)| v).collect::<Vec<_>>(),
        "workloads": workloads,
        "unallocRows": unalloc_rows,
        "storage": x.storage,
    })
}

/// What a seal stores of a report: the invoice-relevant numbers only.
pub fn seal_payload(report: &Value) -> Value {
    json!({
        "window": report["window"],
        "cluster": report["cluster"],
        "totals": {
            "total": report["total"], "measured": report["measuredTotal"],
            "idle": report["idle"], "unallocated": report["unallocated"]["separate"],
        },
        "depts": report["depts"].as_array().into_iter().flatten().map(|d| json!({
            "id": d["id"], "total": d["total"], "measured": d["measured"],
            "estimated": d["estimated"], "basis": d["basis"],
        })).collect::<Vec<_>>(),
        "coverage": { "pct": report.pointer("/coverage/pct"), "gapMinutes": report.pointer("/coverage/gapMinutes") },
        "basis": { "estimatedTotal": report["basis"]["estimatedTotal"], "estimatedPct": report["basis"]["estimatedPct"] },
        "rates": report["rates"],
        "rateVersion": report["rateCard"]["version"],
        "reconciliation": {
            "allocated": report["reconciliation"]["allocated"], "assets": report["reconciliation"]["assets"],
            "driftPct": report["reconciliation"]["driftPct"], "status": report["reconciliation"]["status"],
        },
        "gate": report["gate"]["verdict"],
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn cfg() -> ReportConfig {
        ReportConfig {
            cluster: "test".into(),
            departments: vec![
                ("mfg".into(), "製造部".into()),
                ("rd".into(), "研發部".into()),
            ],
            environments: vec!["prod".into(), "dev".into()],
            system_namespaces: vec!["kube-system".into(), "cattle-".into()],
            thresholds: Thresholds::default(),
        }
    }

    fn row(cost: f64, cpu_h: f64) -> Value {
        json!({"start": "2026-09-24T00:00:00Z", "end": "2026-09-25T00:00:00Z", "minutes": 1440.0,
               "cpuCost": cost * 0.8, "ramCost": cost * 0.2, "pvCost": 0.0, "totalCost": cost,
               "cpuCoreHours": cpu_h, "ramByteHours": cost * 0.8 * GIB, "pvByteHours": 0.0})
    }

    pub fn alloc() -> Allocations {
        Allocations {
            by_cc: json!({"rd": row(30.0, 24.0), "mfg": row(10.0, 8.0), "zz": row(5.0, 4.0),
                          "__idle__": {"totalCost": 40.0}, "__unallocated__": {"totalCost": 15.0}}),
            by_cc_shared: json!({"rd": {"totalCost": 55.0}, "mfg": {"totalCost": 20.0}, "zz": {"totalCost": 10.0},
                                 "__unallocated__": {"totalCost": 15.0}}),
            by_cc_env: json!({"rd/prod": {"totalCost": 20.0}, "rd/dev": {"totalCost": 8.0}, "rd/__unallocated__": {"totalCost": 2.0},
                              "mfg/prod": {"totalCost": 10.0}, "__unallocated__/__unallocated__": {"totalCost": 15.0}}),
            by_ctrl: json!({
                "deployment:api/rd/prod": {"name": "deployment:api/rd/prod", "properties": {"namespace": "rd-apps"}, "totalCost": 20.0},
                "deployment:coredns/__unallocated__/__unallocated__": {"name": "deployment:coredns", "properties": {"namespace": "kube-system"}, "totalCost": 3.0},
                "deployment:tool/__unallocated__/__unallocated__": {"name": "deployment:tool", "properties": {"namespace": "tools"}, "totalCost": 4.0},
                "job:batch/rd/dev": {"name": "job:batch", "properties": {"namespace": "rd-apps"}, "totalCost": 1.0},
            }),
            by_ns: json!({"rd-apps": {"totalCost": 30.0}, "kube-system": {"totalCost": 3.0}, "tools": {"totalCost": 4.0}, "__idle__": {"totalCost": 40.0}}),
        }
    }

    #[test]
    fn shapes_the_report() {
        let r = assemble(&cfg(), &alloc(), "24h", None, &Extras::default());
        let ids: Vec<&str> = r["depts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["mfg", "rd", "zz"]);
        assert_eq!(r["depts"][1]["name"], "研發部");
        assert_eq!(r["depts"][2]["name"], "zz");
        assert_eq!(r["total"], 100.0);
        assert_eq!(r["idle"], 40.0);
        assert_eq!(r["ccEnv"][1]["envs"]["prod"], 20.0);
        assert_eq!(r["ccEnv"][1]["other"], 2.0);
        // The batch job is not a long-running controller; coredns is a system component.
        // tool has no requests: no efficiency figure.
        let tool = r["workloads"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["name"] == "tool")
            .unwrap();
        assert!(tool["cpuEff"].is_null() && tool["ramEff"].is_null());
        let names: Vec<&str> = r["workloads"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["api", "tool"]);
        let un: Vec<(&str, bool)> = r["unallocRows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|u| (u["ns"].as_str().unwrap(), u["system"].as_bool().unwrap()))
            .collect();
        assert_eq!(un, [("tools", false), ("kube-system", true)]);
        // No rate card: implied prices, gate "unknown" on rates, so at best mark.
        assert_eq!(r["rates"]["cpu"], 1.0);
        assert_eq!(r["gate"]["verdict"], "mark");
        assert_eq!(r["window"]["start"], "2026-09-24T00:00:00Z");
    }

    #[test]
    fn empty_cluster_blocks() {
        let r = assemble(
            &cfg(),
            &Allocations::default(),
            "24h",
            None,
            &Extras::default(),
        );
        assert_eq!(r["total"], 0.0);
        assert_eq!(r["gate"]["verdict"], "block");
        // A loaded rate card with no version for an empty period says so.
        let card =
            RateCard::parse(r#"{"versions":[{"effective":"2026-01-01","cpu":1.0}]}"#).unwrap();
        let r = assemble(
            &cfg(),
            &Allocations::default(),
            "2026-10-30T00:00:00Z,2026-10-30T23:59:59Z",
            Some(&card),
            &Extras::default(),
        );
        assert_eq!(r["gate"]["checks"][4]["detail"]["code"], "rates_no_version");
        assert_eq!(r["gate"]["checks"][2]["status"], "unknown");
    }

    #[test]
    fn fill_is_added_and_labelled() {
        let rates = Rates {
            cpu: Some(1.0),
            ram: Some(0.25),
            storage: Some(0.001),
        };
        let gaps = vec![
            (
                "a".to_string(),
                "b".to_string(),
                GapAnswer::Found(json!({"snapshotCoveragePct": 98.0,
                "departments": {"rd": {"cpuCoreHours": 2.0, "ramGiBHours": 4.0, "pvGiBHours": 0.0}}})),
            ),
            ("c".to_string(), "d".to_string(), GapAnswer::NoSnapshot),
        ];
        let fill = declared_fill(&gaps, &rates).unwrap();
        assert_eq!(fill["byDept"]["rd"], 3.0);
        assert_eq!(fill["notes"][0]["code"], "no_snapshot");
        assert_eq!(fill["snapshotCoveragePct"], 98.0);
        let x = Extras {
            fill: Some(fill),
            ..Default::default()
        };
        let r = assemble(&cfg(), &alloc(), "24h", None, &x);
        let rd = r["depts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["id"] == "rd")
            .unwrap();
        assert_eq!(rd["measured"], 30.0);
        assert_eq!(rd["estimated"], 3.0);
        assert_eq!(rd["total"], 33.0);
        assert_eq!(rd["basis"], "mixed");
        assert_eq!(r["total"], 103.0);
        assert_eq!(r["basis"]["estimatedPct"], 2.91);
        assert!(declared_fill(&[], &rates).is_none());
    }

    #[test]
    fn assets_covering_less_time_are_scaled() {
        // Assets cover 720 of the 1440 minutes: 50 there is 100 over the day.
        let x = Extras {
            assets: Some(Assets {
                total: 50.0,
                minutes: Some(720.0),
                ..Default::default()
            }),
            ..Default::default()
        };
        let r = assemble(&cfg(), &alloc(), "24h", None, &x);
        assert_eq!(r["reconciliation"]["assets"], 100.0);
        assert_eq!(r["reconciliation"]["assetsMeasured"], 50.0);
        assert_eq!(r["reconciliation"]["scaled"], true);
        assert_eq!(r["reconciliation"]["status"], "ok");
        // Same span: not scaled.
        let x = Extras {
            assets: Some(Assets {
                total: 100.0,
                minutes: Some(1440.0),
                ..Default::default()
            }),
            ..Default::default()
        };
        let r = assemble(&cfg(), &alloc(), "24h", None, &x);
        assert!(r["reconciliation"].get("scaled").is_none());
    }

    #[test]
    fn rate_card_and_reconciliation_feed_the_gate() {
        let card = RateCard::parse(
            r#"{"versions":[{"effective":"2026-01-01","cpu":1.0,"ram":0.25,"storage":0.001}]}"#,
        )
        .unwrap();
        let x = Extras {
            coverage: Some(json!({"pct": 99.8})),
            assets: Some(Assets {
                total: 100.0,
                minutes: Some(1440.0),
                ..Default::default()
            }),
            ..Default::default()
        };
        let r = assemble(&cfg(), &alloc(), "24h", Some(&card), &x);
        assert_eq!(r["rateCard"]["version"]["effective"], "2026-01-01");
        assert_eq!(r["rateCard"]["check"]["cpu"]["status"], "ok");
        assert_eq!(r["reconciliation"]["status"], "ok");
        // RAM is priced 0.25 on the card, but the data implies 0.25 too.
        assert_eq!(r["rateCard"]["check"]["ram"]["status"], "ok");
        let seal = seal_payload(&r);
        assert_eq!(seal["totals"]["total"], 100.0);
        assert_eq!(seal["depts"].as_array().unwrap().len(), 3);
        assert_eq!(seal["reconciliation"]["status"], "ok");
    }
}
