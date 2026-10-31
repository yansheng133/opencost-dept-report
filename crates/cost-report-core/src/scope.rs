//! What a caller may see: every department, or some of them.
//!
//! The decision (which departments) is made by the server with
//! SubjectAccessReviews; this module only cuts a response down to it. A
//! limited caller sees their departments' rows and the trust of the
//! numbers (coverage, gate), never what other departments spent or the
//! cluster-wide totals that would let them work it out (idle,
//! unallocated, reconciliation, asset cost, scrape targets).

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::num::{f, r4};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    All,
    Departments(BTreeSet<String>),
}

impl Scope {
    pub fn allows(&self, dept: &str) -> bool {
        match self {
            Scope::All => true,
            Scope::Departments(d) => d.contains(dept),
        }
    }

    pub fn is_all(&self) -> bool {
        matches!(self, Scope::All)
    }

    pub fn to_json(&self, known: &[String]) -> Value {
        match self {
            Scope::All => json!({ "all": true, "departments": known }),
            Scope::Departments(d) => json!({ "all": false, "departments": d }),
        }
    }
}

fn keep_array(v: &mut Value, key: &str, keep: impl Fn(&Value) -> bool) {
    if let Some(arr) = v.get_mut(key).and_then(Value::as_array_mut) {
        arr.retain(keep);
    }
}

fn keep_keys(v: &mut Value, scope: &Scope) {
    if let Some(m) = v.as_object_mut() {
        m.retain(|k, _| scope.allows(k));
    }
}

/// A usage series cut to the scope.
pub fn series(mut s: Value, scope: &Scope) -> Value {
    if scope.is_all() {
        return s;
    }
    for p in s
        .get_mut("points")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        if let Some(d) = p.get_mut("depts").filter(|d| d.is_object()) {
            keep_keys(d, scope);
        }
    }
    s
}

/// A report cut to the scope.
pub fn report(mut r: Value, scope: &Scope) -> Value {
    if scope.is_all() {
        return r;
    }
    let allowed = |v: &Value, key: &str| {
        v.get(key)
            .and_then(Value::as_str)
            .is_some_and(|d| scope.allows(d))
    };
    keep_array(&mut r, "depts", |d| allowed(d, "id"));
    keep_array(&mut r, "ccEnv", |d| allowed(d, "dept"));
    keep_array(&mut r, "workloads", |w| allowed(w, "dept"));
    let total = r4(r["depts"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|d| f(d.get("total")))
        .sum());
    r["total"] = json!(total);
    for key in [
        "idle",
        "unallocated",
        "measuredTotal",
        "reconciliation",
        "targets",
        "unallocRows",
    ] {
        r[key] = Value::Null;
    }
    if let Some(fill) = r.pointer_mut("/basis/fill") {
        *fill = Value::Null;
    }
    // Storage: only PVCs paid (in part) by an allowed department, and only
    // those departments' shares.
    if let Some(st) = r.get_mut("storage").filter(|s| s.is_object()) {
        if let Some(rows) = st.get_mut("rows").and_then(Value::as_array_mut) {
            for row in rows.iter_mut() {
                keep_array(row, "depts", |d| allowed(d, "id"));
                let cost = r4(row["depts"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|d| f(d.get("cost")))
                    .sum());
                row["cost"] = json!(cost);
            }
            rows.retain(|row| row["depts"].as_array().is_some_and(|d| !d.is_empty()));
            // Per-row idle cost is the row's own cost × unused share; recompute it
            // for the share that is left.
            for row in rows.iter_mut() {
                if let Some(p) = row.get("usedPct").and_then(Value::as_f64) {
                    row["idleCost"] = json!(r4(f(row.get("cost")) * (1.0 - p).max(0.0)));
                }
            }
            let measured: Vec<&Value> = rows.iter().filter(|x| !x["idleCost"].is_null()).collect();
            let idle = (!measured.is_empty())
                .then(|| r4(measured.iter().map(|x| f(x.get("idleCost"))).sum()));
            let measured_cost = r4(measured.iter().map(|x| f(x.get("cost"))).sum());
            let (n_measured, n, total) = (
                measured.len(),
                rows.len(),
                r4(rows.iter().map(|x| f(x.get("cost"))).sum()),
            );
            st["idleCost"] = json!(idle);
            st["measuredCost"] = json!(measured_cost);
            st["measured"] = json!(n_measured);
            st["count"] = json!(n);
            st["total"] = json!(total);
        }
    }
    if let Some(s) = r.get_mut("series").filter(|s| s.is_object()) {
        *s = series(s.take(), scope);
    }
    r
}

/// The seal list cut to the scope: the total becomes the allowed
/// departments' total for that day, which needs each seal's content.
pub fn seal_row(mut row: Value, doc: Option<&Value>, scope: &Scope) -> Value {
    if scope.is_all() {
        return row;
    }
    let total = doc.map(|d| {
        r4(d["depts"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|x| {
                x.get("id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| scope.allows(id))
            })
            .map(|x| f(x.get("total")))
            .sum())
    });
    row["total"] = json!(total);
    row["driftPct"] = Value::Null;
    row
}

/// One seal cut to the scope.
pub fn seal_doc(mut doc: Value, scope: &Scope) -> Value {
    if scope.is_all() {
        return doc;
    }
    keep_array(&mut doc, "depts", |d| {
        d.get("id")
            .and_then(Value::as_str)
            .is_some_and(|id| scope.allows(id))
    });
    let total = r4(doc["depts"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|d| f(d.get("total")))
        .sum());
    doc["totals"] = json!({ "total": total });
    doc["reconciliation"] = Value::Null;
    doc
}

/// A monthly period cut to the scope.
pub fn period(mut m: Value, scope: &Scope) -> Value {
    if scope.is_all() {
        return m;
    }
    if let Some(d) = m.get_mut("depts") {
        keep_keys(d, scope);
    }
    let total = r4(m["depts"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(_, v)| f(Some(v)))
        .sum());
    m["total"] = json!(total);
    m["idle"] = Value::Null;
    m["unallocated"] = Value::Null;
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::{assemble, Extras};
    use crate::storage;

    fn only(ids: &[&str]) -> Scope {
        Scope::Departments(ids.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn report_is_cut_to_the_departments() {
        let cfg = crate::report::tests::cfg();
        let a = crate::report::tests::alloc();
        let full = assemble(&cfg, &a, "24h", None, &Extras::default());
        assert_eq!(report(full.clone(), &Scope::All), full);

        let r = report(full, &only(&["rd"]));
        assert_eq!(r["depts"].as_array().unwrap().len(), 1);
        assert_eq!(r["total"], 30.0);
        assert!(r["idle"].is_null());
        assert!(r["reconciliation"].is_null());
        assert!(r["unallocRows"].is_null());
        assert!(r["workloads"]
            .as_array()
            .unwrap()
            .iter()
            .all(|w| w["dept"] == "rd"));
        assert_eq!(r["ccEnv"].as_array().unwrap().len(), 1);
        // The gate stays: it says whether the numbers can be trusted.
        assert!(r["gate"]["verdict"].is_string());
    }

    #[test]
    fn storage_keeps_only_allowed_shares() {
        let by_cc = json!({
            "rd": {"pvs": {"k:name=pvc-a": {"cost": 2.0, "byteHours": 1.0}}},
            "mfg": {"pvs": {"k:name=pvc-a": {"cost": 1.0, "byteHours": 1.0}, "k:name=pvc-b": {"cost": 5.0, "byteHours": 1.0}}},
        });
        let mut usage = std::collections::BTreeMap::new();
        usage.insert(
            "pvc-a".to_string(),
            storage::Usage {
                peak: Some(1.0),
                ..Default::default()
            },
        );
        let mut cap = std::collections::BTreeMap::new();
        cap.insert("pvc-a".to_string(), 4.0);
        let st = storage::build(&by_cc, &usage, &cap, &Default::default());
        let r = report(json!({"depts": [], "storage": st}), &only(&["rd"]));
        let rows = r["storage"]["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["cost"], 2.0);
        assert_eq!(rows[0]["idleCost"], 1.5);
        assert_eq!(r["storage"]["total"], 2.0);
    }

    #[test]
    fn seals_and_periods() {
        let doc = json!({"depts": [{"id": "rd", "total": 3.0}, {"id": "mfg", "total": 7.0}],
                         "totals": {"total": 10.0, "idle": 1.0}, "reconciliation": {"driftPct": 0.1}});
        let row = seal_row(
            json!({"total": 10.0, "driftPct": 0.1}),
            Some(&doc),
            &only(&["rd"]),
        );
        assert_eq!(row["total"], 3.0);
        assert!(row["driftPct"].is_null());
        let d = seal_doc(doc, &only(&["rd"]));
        assert_eq!(d["totals"], json!({"total": 3.0}));
        assert_eq!(d["depts"].as_array().unwrap().len(), 1);
        let m = period(
            json!({"depts": {"rd": 3.0, "mfg": 7.0}, "total": 10.0, "idle": 1.0}),
            &only(&["mfg"]),
        );
        assert_eq!(m["total"], 7.0);
        assert!(m["idle"].is_null());
        assert_eq!(m["depts"], json!({"mfg": 7.0}));
    }

    #[test]
    fn series_points() {
        let s = json!({"points": [{"depts": {"rd": {}, "mfg": {}}}, {"depts": null}]});
        let s = series(s, &only(&["rd"]));
        assert_eq!(s["points"][0]["depts"], json!({"rd": {}}));
        assert!(s["points"][1]["depts"].is_null());
    }
}
