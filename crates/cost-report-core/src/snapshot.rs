//! Declared-request snapshots: the second basis for allocation.
//!
//! When Prometheus is down, cost keeps accruing (nodes run, bills come)
//! but the basis for "whose money is this" is gone. The snapshotter talks
//! only to the API server, never to Prometheus, so the two do not fail
//! together. Every few minutes it stores each running pod's requests and
//! each PVC's size with their department; during a gap those snapshots
//! say what was declared.
//!
//! It is deliberately simple: no database, JSON files in a directory. A
//! backup more complex than what it backs up defeats the purpose.
//!
//! The files are schema 2 (`pods` and `pvcs`); schema 1 files (no `pvcs`)
//! still read, with storage 0 — it was not collected then, and a guessed
//! number would be worse than none.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use crate::num::{f, round, GIB};
use crate::time::{fmt_iso, parse_iso};

pub const SCHEMA: u64 = 2;

/// A Kubernetes CPU quantity in cores.
pub fn cpu_cores(v: Option<&Value>) -> f64 {
    let s = match v {
        Some(Value::String(s)) => s.trim().to_string(),
        Some(Value::Number(n)) => return n.as_f64().unwrap_or(0.0),
        _ => return 0.0,
    };
    for (suffix, div) in [("m", 1e3), ("u", 1e6), ("n", 1e9)] {
        if let Some(n) = s.strip_suffix(suffix) {
            return n.parse::<f64>().map(|x| x / div).unwrap_or(0.0);
        }
    }
    s.parse().unwrap_or(0.0)
}

/// A Kubernetes memory or storage quantity in bytes. `Ki`/`Mi`/`Gi`/`Ti`
/// are powers of 2, `k`/`M`/`G`/`T` powers of 10 (PVCs often say `10G`).
pub fn mem_bytes(v: Option<&Value>) -> f64 {
    let s = match v {
        Some(Value::String(s)) => s.trim().to_string(),
        Some(Value::Number(n)) => return n.as_f64().unwrap_or(0.0),
        _ => return 0.0,
    };
    const UNITS: [(&str, f64); 12] = [
        ("Ki", 1024.0),
        ("Mi", 1_048_576.0),
        ("Gi", 1_073_741_824.0),
        ("Ti", 1_099_511_627_776.0),
        ("Pi", 1_125_899_906_842_624.0),
        ("Ei", 1_152_921_504_606_846_976.0),
        ("k", 1e3),
        ("K", 1e3),
        ("M", 1e6),
        ("G", 1e9),
        ("T", 1e12),
        ("P", 1e15),
    ];
    for (suffix, mult) in UNITS {
        if let Some(n) = s.strip_suffix(suffix) {
            return n.parse::<f64>().map(|x| x * mult).unwrap_or(0.0);
        }
    }
    s.parse().unwrap_or(0.0)
}

fn request(c: &Value, key: &str, conv: fn(Option<&Value>) -> f64) -> f64 {
    conv(c.pointer(&format!("/resources/requests/{key}")))
}

/// A pod's effective requests (cores, bytes) as the scheduler counts them.
///
/// Not just the containers' sum: the scheduler takes the larger of the
/// regular containers' sum and the peak during init, because init
/// containers run one after another. Native sidecars (init containers with
/// `restartPolicy: Always`) run alongside, so they count in the sum, and
/// during init only those declared before the running init container are
/// up.
pub fn effective_requests(spec: &Value) -> (f64, f64) {
    let list = |k: &str| {
        spec.get(k)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let regular = list("containers");
    let inits = list("initContainers");
    let is_sidecar = |c: &Value| c.get("restartPolicy").and_then(Value::as_str) == Some("Always");
    let one = |key: &str, conv: fn(Option<&Value>) -> f64| {
        let running: f64 = regular
            .iter()
            .chain(inits.iter().filter(|c| is_sidecar(c)))
            .map(|c| request(c, key, conv))
            .sum();
        let (mut peak, mut started) = (0.0f64, 0.0f64);
        for c in &inits {
            if is_sidecar(c) {
                started += request(c, key, conv);
            } else {
                peak = peak.max(started + request(c, key, conv));
            }
        }
        running.max(peak)
    };
    (one("cpu", cpu_cores), one("memory", mem_bytes))
}

/// `Kind/name` of a pod's owner; a ReplicaSet is reported as its Deployment.
pub fn owner_of(pod: &Value) -> String {
    let Some(r) = pod.pointer("/metadata/ownerReferences/0") else {
        return String::new();
    };
    let kind = r.get("kind").and_then(Value::as_str).unwrap_or("");
    let name = r.get("name").and_then(Value::as_str).unwrap_or("");
    match (kind, name.rsplit_once('-')) {
        // A ReplicaSet's name ends in the pod-template hash.
        ("ReplicaSet", Some((base, _))) => format!("Deployment/{base}"),
        _ => format!("{kind}/{name}"),
    }
}

/// Bytes of storage a pod mounts. The same PVC mounted twice (different
/// subPaths) is one disk, so claims are de-duplicated first.
pub fn claimed_storage(spec: &Value, pvc_bytes: &BTreeMap<(String, String), f64>, ns: &str) -> f64 {
    let claims: BTreeSet<&str> = spec
        .get("volumes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|v| {
            v.pointer("/persistentVolumeClaim/claimName")
                .and_then(Value::as_str)
        })
        .collect();
    claims
        .iter()
        .map(|c| {
            pvc_bytes
                .get(&(ns.to_string(), c.to_string()))
                .copied()
                .unwrap_or(0.0)
        })
        .sum()
}

/// Labels used to attribute objects.
#[derive(Debug, Clone)]
pub struct Labels {
    pub department: String,
    pub environment: String,
}

fn label<'a>(meta: &'a Value, key: &str) -> Option<&'a str> {
    meta.pointer("/labels")
        .and_then(|l| l.get(key))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

/// One snapshot from the API server's lists (`items` of namespaces,
/// PVCs and running pods). The object's own label wins over its
/// namespace's, as in OpenCost, so cross-department charging adds up.
/// Returns (pod rows, PVC rows): storage belongs to the PVC, not to the
/// pods mounting it, so it is allocated from the PVC rows only.
pub fn collect(
    namespaces: &[Value],
    pvcs: &[Value],
    pods: &[Value],
    l: &Labels,
) -> (Vec<Value>, Vec<Value>) {
    let ns_meta: BTreeMap<&str, &Value> = namespaces
        .iter()
        .filter_map(|n| Some((n.pointer("/metadata/name")?.as_str()?, n.get("metadata")?)))
        .collect();
    let attr = |meta: &Value, ns: &str, key: &str| -> String {
        label(meta, key)
            .or_else(|| ns_meta.get(ns).and_then(|m| label(m, key)))
            .unwrap_or("")
            .to_string()
    };

    let mut pvc_rows = Vec::new();
    let mut pvc_bytes = BTreeMap::new();
    for c in pvcs {
        let Some(meta) = c.get("metadata") else {
            continue;
        };
        let ns = meta.get("namespace").and_then(Value::as_str).unwrap_or("");
        let name = meta.get("name").and_then(Value::as_str).unwrap_or("");
        let size = mem_bytes(c.pointer("/spec/resources/requests/storage")).floor();
        pvc_bytes.insert((ns.to_string(), name.to_string()), size);
        pvc_rows.push(json!({
            "ns": ns, "name": name,
            "dept": attr(meta, ns, &l.department),
            "env": attr(meta, ns, &l.environment),
            "bytes": size as u64,
        }));
    }

    let mut rows = Vec::new();
    for p in pods {
        let (Some(meta), Some(spec)) = (p.get("metadata"), p.get("spec")) else {
            continue;
        };
        let ns = meta.get("namespace").and_then(Value::as_str).unwrap_or("");
        let (cpu, mem) = effective_requests(spec);
        rows.push(json!({
            "ns": ns,
            "pod": meta.get("name").and_then(Value::as_str).unwrap_or(""),
            "dept": attr(meta, ns, &l.department),
            "env": attr(meta, ns, &l.environment),
            "cpu": round(cpu, 6),
            "mem": mem.floor() as u64,
            "pv": claimed_storage(spec, &pvc_bytes, ns) as u64,
            "node": spec.get("nodeName").and_then(Value::as_str).unwrap_or(""),
            "owner": owner_of(p),
        }));
    }
    (rows, pvc_rows)
}

/// The document written to disk for one snapshot.
pub fn document(ts: f64, interval_s: u64, l: &Labels, pods: Vec<Value>, pvcs: Vec<Value>) -> Value {
    json!({
        "schema": SCHEMA,
        "ts": fmt_iso(ts),
        "interval_s": interval_s,
        "dept_label": l.department,
        "env_label": l.environment,
        "pods": pods,
        "pvcs": pvcs,
    })
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeptHours {
    pub cpu_h: f64,
    pub ram_gib_h: f64,
    pub pv_gib_h: f64,
    pub workloads: BTreeSet<String>,
}

/// Each snapshot stands for `interval_s` of time; declared amount × that
/// time is declared resource-hours. Storage comes from `pvcs` only (pods
/// sharing a PVC would count it twice).
pub fn allocate(snaps: &[Value], interval_s: f64) -> (BTreeMap<String, DeptHours>, DeptHours) {
    let hours = interval_s / 3600.0;
    let mut by: BTreeMap<String, DeptHours> = BTreeMap::new();
    let mut totals = DeptHours::default();
    let key = |d: Option<&Value>| {
        d.and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or("__unallocated__")
            .to_string()
    };
    for s in snaps {
        for p in s
            .get("pods")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let r = by.entry(key(p.get("dept"))).or_default();
            let (c, m) = (f(p.get("cpu")) * hours, f(p.get("mem")) / GIB * hours);
            r.cpu_h += c;
            r.ram_gib_h += m;
            let owner = p
                .get("owner")
                .and_then(Value::as_str)
                .filter(|o| !o.is_empty());
            let pod = p.get("pod").and_then(Value::as_str).unwrap_or("");
            r.workloads.insert(format!(
                "{}/{}",
                p.get("ns").and_then(Value::as_str).unwrap_or(""),
                owner.unwrap_or(pod)
            ));
            totals.cpu_h += c;
            totals.ram_gib_h += m;
        }
        for c in s
            .get("pvcs")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let r = by.entry(key(c.get("dept"))).or_default();
            let g = f(c.get("bytes")) / GIB * hours;
            r.pv_gib_h += g;
            totals.pv_gib_h += g;
        }
    }
    (by, totals)
}

/// The `/allocation` answer for `[start, end)`, or `None` when no snapshot
/// falls in it — "no data" must stay distinguishable from "zero".
pub fn allocation(snaps: &[Value], start: i64, end: i64) -> Option<Value> {
    let mut inside: Vec<&Value> = snaps
        .iter()
        .filter(|s| {
            s.get("ts")
                .and_then(Value::as_str)
                .and_then(|t| parse_iso(t).ok())
                .is_some_and(|t| t >= start && t < end)
        })
        .collect();
    if inside.is_empty() {
        return None;
    }
    inside.sort_by(|a, b| a["ts"].as_str().cmp(&b["ts"].as_str()));
    let interval_s = inside[0]
        .get("interval_s")
        .and_then(Value::as_f64)
        .unwrap_or(300.0);
    let expected = (((end - start) as f64 / interval_s).round()).max(1.0);
    let owned: Vec<Value> = inside.iter().map(|v| (*v).clone()).collect();
    let (by, _) = allocate(&owned, interval_s);
    // Capped at 100: a sample landing on both edges would read 105 %, and
    // then nobody knows which number to trust.
    let coverage = (inside.len() as f64 / expected * 100.0).min(100.0);
    Some(json!({
        "window": { "start": fmt_iso(start as f64), "end": fmt_iso(end as f64) },
        "basis": "declared",
        "snapshotCoveragePct": round(coverage, 2),
        "samples": { "found": inside.len(), "expected": expected as u64, "intervalSeconds": interval_s },
        "departments": departments_json(&by),
    }))
}

/// Field names the report reads; do not rename casually.
pub fn departments_json(by: &BTreeMap<String, DeptHours>) -> Value {
    Value::Object(
        by.iter()
            .map(|(d, r)| {
                (
                    d.clone(),
                    json!({
                        "cpuCoreHours": round(r.cpu_h, 4),
                        "ramGiBHours": round(r.ram_gib_h, 4),
                        "pvGiBHours": round(r.pv_gib_h, 4),
                        "workloads": r.workloads.len(),
                    }),
                )
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(s: &str) -> Value {
        json!(s)
    }

    #[test]
    fn quantities() {
        assert_eq!(cpu_cores(Some(&q("100m"))), 0.1);
        assert_eq!(cpu_cores(Some(&q("1500m"))), 1.5);
        assert_eq!(cpu_cores(Some(&q("2"))), 2.0);
        assert_eq!(cpu_cores(None), 0.0);
        assert_eq!(mem_bytes(Some(&q("128Mi"))), 134_217_728.0);
        assert_eq!(mem_bytes(Some(&q("1Gi"))), GIB);
        assert_eq!(mem_bytes(Some(&q("500M"))), 5e8);
        assert_eq!(mem_bytes(Some(&q("10G"))), 1e10);
        assert_eq!(mem_bytes(Some(&q("1T"))), 1e12);
        assert_eq!(mem_bytes(Some(&q("1Ti"))), 1_099_511_627_776.0);
        assert_eq!(mem_bytes(Some(&q("1024"))), 1024.0);
    }

    fn c(cpu: &str, mem: &str) -> Value {
        json!({"resources": {"requests": {"cpu": cpu, "memory": mem}}})
    }

    #[test]
    fn effective_requests_follow_the_scheduler() {
        let base = vec![c("100m", "64Mi"), c("200m", "64Mi")];
        let (cpu, mem) = effective_requests(&json!({"containers": base}));
        assert!((cpu - 0.3).abs() < 1e-9);
        assert_eq!(mem, 128.0 * 1_048_576.0);
        // A big init container sets the peak.
        let (cpu, _) =
            effective_requests(&json!({"containers": base, "initContainers": [c("1", "1Mi")]}));
        assert_eq!(cpu, 1.0);
        // A sidecar runs alongside: in the sum.
        let mut side = c("500m", "0");
        side["restartPolicy"] = json!("Always");
        let (cpu, _) =
            effective_requests(&json!({"containers": base, "initContainers": [side.clone()]}));
        assert!((cpu - 0.8).abs() < 1e-9);
        // A sidecar declared after an init container was not up while it ran.
        let (cpu, _) = effective_requests(
            &json!({"containers": [c("100m", "0")], "initContainers": [c("700m", "0"), side.clone()]}),
        );
        assert!((cpu - 0.7).abs() < 1e-9);
        let (cpu, _) = effective_requests(
            &json!({"containers": [c("100m", "0")], "initContainers": [side, c("700m", "0")]}),
        );
        assert!((cpu - 1.2).abs() < 1e-9);
    }

    #[test]
    fn collect_attributes_and_dedups() {
        let l = Labels {
            department: "cost-center".into(),
            environment: "env".into(),
        };
        let nss = vec![
            json!({"metadata": {"name": "rd-apps", "labels": {"cost-center": "rd", "env": "prod"}}}),
        ];
        let pvcs = vec![
            json!({"metadata": {"namespace": "rd-apps", "name": "data"}, "spec": {"resources": {"requests": {"storage": "2Gi"}}}}),
        ];
        let pods = vec![
            json!({"metadata": {"namespace": "rd-apps", "name": "api-7d9f-abcde", "labels": {"cost-center": "mfg"},
                    "ownerReferences": [{"kind": "ReplicaSet", "name": "api-7d9f"}]},
                   "spec": {"nodeName": "n1", "containers": [c("1", "1Gi")],
                            "volumes": [{"persistentVolumeClaim": {"claimName": "data"}}, {"persistentVolumeClaim": {"claimName": "data"}}]}}),
            json!({"metadata": {"namespace": "other", "name": "x"}, "spec": {"containers": []}}),
        ];
        let (rows, pvc_rows) = collect(&nss, &pvcs, &pods, &l);
        assert_eq!(rows[0]["dept"], "mfg");
        assert_eq!(rows[0]["env"], "prod");
        assert_eq!(rows[0]["owner"], "Deployment/api");
        assert_eq!(rows[0]["pv"], 2 * 1_073_741_824u64);
        assert_eq!(rows[1]["dept"], "");
        assert_eq!(pvc_rows[0]["dept"], "rd");
    }

    fn snap(ts: &str, cpu: f64, schema1: bool) -> Value {
        let mut s = json!({"ts": ts, "interval_s": 300,
            "pods": [{"ns": "a", "pod": "p", "owner": "Deployment/x", "dept": "rd", "cpu": cpu, "mem": GIB}],
            "pvcs": [{"dept": "rd", "bytes": GIB}, {"dept": "", "bytes": GIB}]});
        if schema1 {
            s.as_object_mut().unwrap().remove("pvcs");
        }
        s
    }

    #[test]
    fn allocation_window_edges_and_schema1() {
        let t0 = parse_iso("2026-09-24T05:00:00Z").unwrap();
        let snaps = vec![
            snap("2026-09-24T05:00:00Z", 1.0, false),
            snap("2026-09-24T05:05:00Z", 1.0, true),
            snap("2026-09-24T05:10:00Z", 1.0, false), // end is exclusive
        ];
        let a = allocation(&snaps, t0, t0 + 600).unwrap();
        assert_eq!(a["samples"]["found"], 2);
        assert_eq!(a["snapshotCoveragePct"], 100.0);
        let rd = &a["departments"]["rd"];
        assert!((rd["cpuCoreHours"].as_f64().unwrap() - 2.0 / 12.0).abs() < 1e-4);
        // Only the schema-2 snapshot has storage.
        assert!((rd["pvGiBHours"].as_f64().unwrap() - 1.0 / 12.0).abs() < 1e-4);
        assert_eq!(rd["workloads"], 1);
        assert!(a["departments"]["__unallocated__"].is_object());
        assert!(allocation(&snaps, t0 + 3600, t0 + 7200).is_none());
    }
}
