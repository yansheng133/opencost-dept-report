//! Storage: what each PVC claims, what it actually uses, and how much is
//! paid for empty space.
//!
//! Three sources:
//!
//! * **cost** — OpenCost bills a PV by claimed capacity × hours mounted ×
//!   price, regardless of use. Each department's allocation has `pvs`,
//!   keyed like `cluster=prod:name=pvc-…`.
//! * **capacity and claim** — kube-state-metrics.
//! * **actual use** — `cost-report volume-usage` on the nodes, or the
//!   kubelet for CSI volumes.
//!
//! Idle cost uses the **peak** use in the window, not the last value: a
//! database that cleaned its scratch space at night looks empty, but it
//! needed the space by day.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::num::{f, round, GIB};

/// One PV's measured use.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Usage {
    pub ns: Option<String>,
    pub pvc: Option<String>,
    pub used: Option<f64>,
    pub peak: Option<f64>,
    pub errors: Option<f64>,
}

/// `cluster=prod:name=pvc-…` → `pvc-…`.
pub fn pv_from_key(key: &str) -> &str {
    key.split(':')
        .find_map(|p| p.strip_prefix("name="))
        .unwrap_or(key)
}

fn special_label(id: &str) -> Option<&'static str> {
    match id {
        "__unallocated__" => Some("unallocated"),
        "__unmounted__" => Some("unmounted"),
        "__idle__" => Some("idle"),
        _ => None,
    }
}

/// Who pays for this PVC. Rows that round to 0 are dropped unless they are
/// the only one: OpenCost also lists the same PV under `__unallocated__`
/// with cost 0 and a few seconds of byte-hours (a pod changing hands),
/// which would make one PVC look split between two owners.
fn depts(costs: &BTreeMap<String, f64>) -> Vec<Value> {
    let mut rows: Vec<(&String, f64)> = costs.iter().map(|(k, v)| (k, *v)).collect();
    rows.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let mk = |(d, c): &(&String, f64)| json!({ "id": d, "cost": round(*c, 4), "special": special_label(d) });
    let shown: Vec<Value> = rows
        .iter()
        .filter(|(_, c)| round(*c, 4) > 0.0)
        .map(mk)
        .collect();
    if shown.is_empty() {
        rows.first().map(mk).into_iter().collect()
    } else {
        shown
    }
}

/// Merge the three sources into the table the page shows.
pub fn build(
    by_cc: &Value,
    usage: &BTreeMap<String, Usage>,
    capacity: &BTreeMap<String, f64>,
    claims: &BTreeMap<String, (String, String)>,
) -> Value {
    struct Row {
        cost: f64,
        gib_hours: f64,
        depts: BTreeMap<String, f64>,
    }
    let mut rows: BTreeMap<String, Row> = BTreeMap::new();
    for (dept, alloc) in by_cc.as_object().into_iter().flatten() {
        for (key, pv) in alloc
            .get("pvs")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            let cost = f(pv.get("cost"));
            let byte_hours = f(pv.get("byteHours"));
            if cost <= 0.0 && byte_hours <= 0.0 {
                continue;
            }
            let r = rows.entry(pv_from_key(key).to_string()).or_insert(Row {
                cost: 0.0,
                gib_hours: 0.0,
                depts: BTreeMap::new(),
            });
            r.cost += cost;
            r.gib_hours += byte_hours / GIB;
            *r.depts.entry(dept.clone()).or_insert(0.0) += cost;
        }
    }

    let mut out: Vec<Value> = rows
        .iter()
        .map(|(name, r)| {
            let u = usage.get(name).cloned().unwrap_or_default();
            let (ns, pvc) = match claims.get(name) {
                Some((n, p)) => (Some(n.clone()), Some(p.clone())),
                None => (u.ns.clone(), u.pvc.clone()),
            };
            let cap = capacity.get(name).copied();
            let basis = u.peak.or(u.used);
            let pct = match (basis, cap) {
                (Some(b), Some(c)) if c > 0.0 => Some(b / c),
                _ => None,
            };
            // Used beyond capacity means nothing is idle; local-path does not
            // enforce capacity, so this happens.
            let idle = pct.map(|p| r.cost * (1.0 - p).max(0.0));
            json!({
                "pv": name, "ns": ns, "pvc": pvc,
                "depts": depts(&r.depts),
                "capacityBytes": cap,
                "usedBytes": u.used, "peakBytes": u.peak,
                "usedPct": pct.map(|p| round(p, 4)),
                "over": matches!((cap, basis), (Some(c), Some(b)) if c > 0.0 && b > c),
                "partial": u.errors.is_some_and(|e| e > 0.0),
                "cost": round(r.cost, 4),
                "idleCost": idle.map(|i| round(i, 4)),
                "gibHours": round(r.gib_hours, 2),
            })
        })
        .collect();
    out.sort_by(|a, b| {
        f(b.get("cost"))
            .partial_cmp(&f(a.get("cost")))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a["pv"].as_str().cmp(&b["pv"].as_str()))
    });

    let measured: Vec<&Value> = out.iter().filter(|x| !x["idleCost"].is_null()).collect();
    json!({
        "total": round(out.iter().map(|x| f(x.get("cost"))).sum(), 4),
        // Idle adds up only the measured rows; unmeasured is unknown, not zero.
        "idleCost": if measured.is_empty() { Value::Null } else {
            json!(round(measured.iter().map(|x| f(x.get("idleCost"))).sum(), 4)) },
        "measuredCost": round(measured.iter().map(|x| f(x.get("cost"))).sum(), 4),
        "measured": measured.len(),
        "count": out.len(),
        "rows": out,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn by_cc() -> Value {
        json!({
            "rd": {"pvs": {"cluster=c:name=pvc-a": {"cost": 2.0, "byteHours": GIB * 48.0}}},
            "__unallocated__": {"pvs": {
                "cluster=c:name=pvc-a": {"cost": 0.0, "byteHours": 1.0},
                "cluster=c:name=pvc-b": {"cost": 1.0, "byteHours": GIB * 24.0}}},
        })
    }

    #[test]
    fn merges_sources() {
        let mut usage = BTreeMap::new();
        usage.insert(
            "pvc-a".into(),
            Usage {
                used: Some(0.25 * GIB),
                peak: Some(0.5 * GIB),
                ..Default::default()
            },
        );
        let mut cap = BTreeMap::new();
        cap.insert("pvc-a".to_string(), 2.0 * GIB);
        cap.insert("pvc-b".to_string(), 1.0 * GIB);
        let mut claims = BTreeMap::new();
        claims.insert("pvc-a".to_string(), ("db".to_string(), "data".to_string()));
        let s = build(&by_cc(), &usage, &cap, &claims);
        assert_eq!(s["count"], 2);
        let a = &s["rows"][0];
        assert_eq!(a["pv"], "pvc-a");
        assert_eq!(a["ns"], "db");
        // Peak, not last value: 0.5 of 2 GiB used → 75 % of 2.0 is idle.
        assert_eq!(a["usedPct"], 0.25);
        assert_eq!(a["idleCost"], 1.5);
        // The zero-cost twin under __unallocated__ is not listed as an owner.
        assert_eq!(
            a["depts"],
            json!([{"id": "rd", "cost": 2.0, "special": null}])
        );
        let b = &s["rows"][1];
        assert!(b["idleCost"].is_null());
        assert_eq!(b["depts"][0]["special"], "unallocated");
        assert_eq!(s["idleCost"], 1.5);
        assert_eq!(s["measured"], 1);
    }

    #[test]
    fn over_capacity_and_nothing_measured() {
        let mut usage = BTreeMap::new();
        usage.insert(
            "pvc-a".into(),
            Usage {
                peak: Some(3.0 * GIB),
                errors: Some(2.0),
                ..Default::default()
            },
        );
        let mut cap = BTreeMap::new();
        cap.insert("pvc-a".to_string(), 2.0 * GIB);
        let s = build(&by_cc(), &usage, &cap, &BTreeMap::new());
        let a = &s["rows"][0];
        assert_eq!(a["over"], true);
        assert_eq!(a["partial"], true);
        assert_eq!(a["idleCost"], 0.0);
        let s = build(
            &by_cc(),
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        assert!(s["idleCost"].is_null());
        assert_eq!(pv_from_key("plain"), "plain");
    }
}
