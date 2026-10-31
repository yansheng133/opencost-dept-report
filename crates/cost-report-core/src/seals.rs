//! Sealed billing days and monthly periods built from them.
//!
//! Prometheus keeps a limited history, so last month's invoice cannot be
//! recomputed later, and recomputing a day that is partly expired gives a
//! different number each time. A seal freezes one finished UTC day: the
//! same day always reads the same, and a SHA-256 over the content shows it
//! was not edited afterwards.
//!
//! A seal is never overwritten. Sealing a day again with different content
//! reports a conflict instead; a silently rewritten invoice is worse than
//! a visible disagreement. Corrections belong in a new record, not in a
//! rewrite (deleting and resealing once lost a morning that had already
//! left retention).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::num::{f, round};
use crate::time::{days_in_month, parse_day};

/// The hash input: compact JSON with keys sorted (serde_json's map is
/// ordered), so the same content always hashes the same.
pub fn content_hash(payload: &Value) -> String {
    let blob = serde_json::to_string(payload).unwrap_or_default();
    let digest = Sha256::digest(blob.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

fn path_of(dir: &Path, day: &str) -> Option<PathBuf> {
    // Only real dates: the day comes from a URL and must not name any other file.
    parse_day(day)?;
    Some(dir.join(format!("{day}.json")))
}

/// Result of [`seal_day`].
#[derive(Debug, Clone, PartialEq)]
pub enum SealResult {
    Sealed {
        sha256: String,
    },
    Unchanged {
        sha256: String,
    },
    Conflict {
        sealed: Option<String>,
        current: String,
        sealed_at: Option<String>,
    },
    Unreadable,
}

impl SealResult {
    pub fn status(&self) -> &'static str {
        match self {
            SealResult::Sealed { .. } => "sealed",
            SealResult::Unchanged { .. } => "unchanged",
            SealResult::Conflict { .. } => "conflict",
            SealResult::Unreadable => "unreadable",
        }
    }
}

/// Seal one day. Never overwrites an existing seal.
pub fn seal_day(
    dir: &Path,
    day: &str,
    payload: &Value,
    sealed_at: &str,
) -> std::io::Result<SealResult> {
    let path = path_of(dir, day)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "not a day"))?;
    fs::create_dir_all(dir)?;
    let mut body = payload.as_object().cloned().unwrap_or_default();
    body.insert("day".into(), json!(day));
    let body = Value::Object(body);
    let digest = content_hash(&body);

    if path.exists() {
        let old = match fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        {
            Some(v) => v,
            None => return Ok(SealResult::Unreadable),
        };
        if old.get("sha256").and_then(Value::as_str) == Some(digest.as_str()) {
            return Ok(SealResult::Unchanged { sha256: digest });
        }
        return Ok(SealResult::Conflict {
            sealed: old
                .get("sha256")
                .and_then(Value::as_str)
                .map(str::to_string),
            current: digest,
            sealed_at: old
                .get("sealedAt")
                .and_then(Value::as_str)
                .map(str::to_string),
        });
    }

    let mut doc = body.as_object().cloned().unwrap_or_default();
    doc.insert("sha256".into(), json!(digest));
    doc.insert("sealedAt".into(), json!(sealed_at));
    let text = serde_json::to_string(&Value::Object(doc)).unwrap_or_default();
    // Write, flush to disk, then rename: a reader never sees half an
    // invoice, and a crash does not leave an empty file that blocks the
    // day from ever being sealed.
    let tmp = dir.join(format!(".{day}.json.tmp"));
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
    }
    fs::rename(&tmp, &path)?;
    if let Ok(d) = fs::File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(SealResult::Sealed { sha256: digest })
}

pub fn read_seal(dir: &Path, day: &str) -> Option<Value> {
    let path = path_of(dir, day)?;
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Does the stored hash still match the content?
pub fn intact(doc: &Value) -> bool {
    let stored = doc.get("sha256").and_then(Value::as_str);
    let mut body = doc.as_object().cloned().unwrap_or_default();
    body.remove("sha256");
    body.remove("sealedAt");
    stored == Some(content_hash(&Value::Object(body)).as_str())
}

/// Every seal, oldest first, each with its hash re-verified: storing a hash
/// without checking it is no tamper protection at all.
pub fn list_seals(dir: &Path) -> Vec<Value> {
    let mut names: Vec<String> = match fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(Result::ok)
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| n.ends_with(".json") && parse_day(&n[..n.len() - 5]).is_some())
            .collect(),
        Err(_) => return Vec::new(),
    };
    names.sort();
    names
        .iter()
        .map(|name| {
            let day = &name[..name.len() - 5];
            match read_seal(dir, day) {
                None => json!({ "day": day, "status": "unreadable", "intact": false }),
                Some(doc) => json!({
                    "day": doc.get("day").and_then(Value::as_str).unwrap_or(day),
                    "sealedAt": doc.get("sealedAt"),
                    "total": doc.pointer("/totals/total"),
                    "coveragePct": doc.pointer("/coverage/pct"),
                    "estimatedPct": doc.pointer("/basis/estimatedPct"),
                    // The verdict when the day was sealed; it never changes.
                    "gate": doc.get("gate"),
                    "driftPct": doc.pointer("/reconciliation/driftPct"),
                    "intact": intact(&doc),
                }),
            }
        })
        .collect()
}

fn gate_rank(g: &str) -> u8 {
    match g {
        "block" => 3,
        "mark" => 2,
        _ => 1,
    }
}

/// Monthly periods from the seals. `today` is `YYYY-MM-DD` (UTC).
///
/// Days without a seal are listed, never glossed over: twenty sealed days
/// added up are not a month's invoice, and on screen the two look alike.
/// Any missing day, any blocked day or any tampered seal blocks the month.
pub fn aggregate_periods(dir: &Path, today: &str) -> Vec<Value> {
    struct Month {
        days: Vec<String>,
        total: f64,
        idle: f64,
        unallocated: f64,
        estimated: f64,
        depts: Map<String, Value>,
        worst: String,
        tampered: Vec<String>,
        min_cov: Option<f64>,
        gap_minutes: f64,
    }
    let mut months: std::collections::BTreeMap<String, Month> = Default::default();
    for row in list_seals(dir) {
        let day = row["day"].as_str().unwrap_or("").to_string();
        if parse_day(&day).is_none() {
            continue;
        }
        let Some(doc) = read_seal(dir, &day) else {
            continue;
        };
        let m = months.entry(day[..7].to_string()).or_insert_with(|| Month {
            days: Vec::new(),
            total: 0.0,
            idle: 0.0,
            unallocated: 0.0,
            estimated: 0.0,
            depts: Map::new(),
            worst: "ok".into(),
            tampered: Vec::new(),
            min_cov: None,
            gap_minutes: 0.0,
        });
        m.total += f(doc.pointer("/totals/total"));
        m.idle += f(doc.pointer("/totals/idle"));
        m.unallocated += f(doc.pointer("/totals/unallocated"));
        m.estimated += f(doc.pointer("/basis/estimatedTotal"));
        for d in doc
            .get("depts")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(id) = d.get("id").and_then(Value::as_str) {
                let prev = m.depts.get(id).and_then(Value::as_f64).unwrap_or(0.0);
                m.depts
                    .insert(id.into(), json!(round(prev + f(d.get("total")), 4)));
            }
        }
        let g = doc.get("gate").and_then(Value::as_str).unwrap_or("ok");
        if gate_rank(g) > gate_rank(&m.worst) {
            m.worst = g.into();
        }
        if let Some(c) = doc.pointer("/coverage/pct").and_then(Value::as_f64) {
            m.min_cov = Some(m.min_cov.map_or(c, |p: f64| p.min(c)));
        }
        m.gap_minutes += f(doc.pointer("/coverage/gapMinutes"));
        if row["intact"] == json!(false) {
            m.tampered.push(day.clone());
        }
        m.days.push(day);
    }

    let this_month = &today[..today.len().min(7)];
    let today_dom: u32 = today.get(8..10).and_then(|s| s.parse().ok()).unwrap_or(31);
    months
        .into_iter()
        .map(|(ym, mut m)| {
            let y: i64 = ym[..4].parse().unwrap_or(1970);
            let mo: u32 = ym[5..7].parse().unwrap_or(1);
            let month_days = days_in_month(y, mo);
            // The current month only counts up to today: days still ahead are not "missing".
            let last = if ym == this_month { month_days.min(today_dom) } else { month_days };
            let expected: Vec<String> = (1..=last).map(|d| format!("{ym}-{d:02}")).collect();
            let missing: Vec<String> = expected.iter().filter(|d| !m.days.contains(d)).cloned().collect();
            let complete = missing.is_empty() && ym != this_month;
            let (verdict, reason, args) = if !m.tampered.is_empty() {
                ("block", "tampered", json!({ "days": m.tampered }))
            } else if !missing.is_empty() {
                ("block", "missing_days", json!({ "missing": missing.len(), "sealed": m.days.len() }))
            } else if !complete {
                (if m.worst == "block" { "block" } else { "mark" }, "in_progress", json!({}))
            } else {
                let r = match m.worst.as_str() {
                    "block" => "day_blocked",
                    "mark" => "day_marked",
                    _ => "all_ok",
                };
                (
                    match m.worst.as_str() {
                        "block" => "block",
                        "mark" => "mark",
                        _ => "ok",
                    },
                    r,
                    json!({}),
                )
            };
            m.days.sort();
            json!({
                "month": ym,
                "days": m.days,
                "sealedDays": m.days.len(),
                "expectedDays": expected.len(),
                "monthDays": month_days,
                "missingDays": missing,
                "complete": complete,
                "total": round(m.total, 4),
                "idle": round(m.idle, 4),
                "unallocated": round(m.unallocated, 4),
                "estimated": round(m.estimated, 4),
                "estimatedPct": if m.total != 0.0 { round(m.estimated / m.total * 100.0, 2) } else { 0.0 },
                "depts": m.depts,
                "worstGate": m.worst,
                "tampered": m.tampered,
                "minCoverage": m.min_cov,
                "gapMinutes": round(m.gap_minutes, 1),
                "verdict": verdict,
                "reason": { "code": reason, "args": args },
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!(
                "cost-report-seals-{tag}-{}-{}",
                std::process::id(),
                crate::time::now()
            ));
            fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const AT: &str = "2026-09-24T01:00:00Z";

    #[test]
    fn never_overwrites() {
        let t = TempDir::new("ow");
        let body = json!({"totals": {"total": 100.0}, "depts": [{"id": "mfg", "total": 60.0}]});
        assert_eq!(
            seal_day(&t.0, "2026-09-23", &body, AT).unwrap().status(),
            "sealed"
        );
        assert_eq!(
            seal_day(&t.0, "2026-09-23", &body, AT).unwrap().status(),
            "unchanged"
        );
        let changed = json!({"totals": {"total": 999.0}, "depts": [{"id": "mfg", "total": 60.0}]});
        assert_eq!(
            seal_day(&t.0, "2026-09-23", &changed, AT).unwrap().status(),
            "conflict"
        );
        assert_eq!(
            read_seal(&t.0, "2026-09-23").unwrap()["totals"]["total"],
            100.0
        );
        assert!(seal_day(&t.0, "../etc/passwd", &body, AT).is_err());
        assert!(read_seal(&t.0, "../2026-09-23").is_none());
    }

    #[test]
    fn list_verifies_hashes() {
        let t = TempDir::new("list");
        seal_day(&t.0, "2026-09-23", &json!({"totals": {"total": 100.0}}), AT).unwrap();
        seal_day(
            &t.0,
            "2026-09-22",
            &json!({"totals": {"total": 5.0}, "gate": "block",
            "reconciliation": {"driftPct": -0.02}}),
            AT,
        )
        .unwrap();
        let listed = list_seals(&t.0);
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0]["day"], "2026-09-22");
        assert_eq!(listed[0]["gate"], "block");
        assert_eq!(listed[0]["driftPct"], -0.02);
        assert!(listed.iter().all(|r| r["intact"] == true));

        // Edit the file by hand: intact must turn false, for that day only.
        let p = t.0.join("2026-09-23.json");
        let mut doc: Value = serde_json::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
        doc["totals"]["total"] = json!(1.0);
        fs::write(&p, doc.to_string()).unwrap();
        let listed = list_seals(&t.0);
        let by = |d: &str| listed.iter().find(|r| r["day"] == d).unwrap().clone();
        assert_eq!(by("2026-09-23")["intact"], false);
        assert_eq!(by("2026-09-22")["intact"], true);
    }

    fn seal(dir: &Path, day: &str, total: f64, gate: &str) {
        seal_day(
            dir,
            day,
            &json!({
                "totals": {"total": total, "idle": total * 0.2, "unallocated": total * 0.3},
                "depts": [{"id": "mfg", "total": total * 0.3}, {"id": "rd", "total": total * 0.2}],
                "coverage": {"pct": 100.0, "gapMinutes": 0.0},
                "basis": {"estimatedTotal": 0.0}, "gate": gate,
            }),
            AT,
        )
        .unwrap();
    }

    fn month(dir: &Path, ym: &str, today: &str) -> Value {
        aggregate_periods(dir, today)
            .into_iter()
            .find(|m| m["month"] == ym)
            .unwrap()
    }

    #[test]
    fn full_month() {
        let t = TempDir::new("full");
        for d in 1..=31 {
            seal(&t.0, &format!("2026-08-{d:02}"), 10.0, "ok");
        }
        let aug = month(&t.0, "2026-08", "2026-09-15");
        assert_eq!(aug["sealedDays"], 31);
        assert_eq!(aug["missingDays"], json!([]));
        assert!((aug["total"].as_f64().unwrap() - 310.0).abs() < 1e-6);
        assert!((aug["depts"]["mfg"].as_f64().unwrap() - 93.0).abs() < 1e-6);
        assert_eq!(aug["verdict"], "ok");
        assert_eq!(aug["reason"]["code"], "all_ok");
    }

    #[test]
    fn missing_blocked_in_progress_tampered() {
        let t = TempDir::new("miss");
        for d in (1..=31).filter(|d| *d != 15) {
            seal(&t.0, &format!("2026-08-{d:02}"), 10.0, "ok");
        }
        let aug = month(&t.0, "2026-08", "2026-09-15");
        assert_eq!(aug["missingDays"], json!(["2026-08-15"]));
        assert_eq!(aug["verdict"], "block");
        assert_eq!(aug["reason"]["code"], "missing_days");

        let t = TempDir::new("blk");
        for d in 1..=31 {
            seal(
                &t.0,
                &format!("2026-08-{d:02}"),
                10.0,
                if d == 7 { "block" } else { "ok" },
            );
        }
        assert_eq!(month(&t.0, "2026-08", "2026-09-15")["verdict"], "block");

        let t = TempDir::new("cur");
        for d in 1..=10 {
            seal(&t.0, &format!("2026-09-{d:02}"), 10.0, "ok");
        }
        let sep = month(&t.0, "2026-09", "2026-09-10");
        assert_eq!(sep["missingDays"], json!([]));
        assert_eq!(sep["complete"], false);
        assert_eq!(sep["verdict"], "mark");

        let p = t.0.join("2026-09-03.json");
        let mut doc: Value = serde_json::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
        doc["totals"]["total"] = json!(999.0);
        fs::write(&p, doc.to_string()).unwrap();
        let sep = month(&t.0, "2026-09", "2026-09-10");
        assert_eq!(sep["verdict"], "block");
        assert_eq!(sep["tampered"], json!(["2026-09-03"]));
    }
}
