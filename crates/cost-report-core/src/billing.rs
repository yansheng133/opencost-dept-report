//! Billing policy: the rate card, the billing gate and reconciliation.
//!
//! None of this is about querying OpenCost. It is the part between a cost
//! number and an invoice:
//!
//! * who set the unit price, and from when?            → rate card
//! * can these numbers be used to charge anyone?        → billing gate
//! * does the allocated total match what was spent?     → reconciliation
//!
//! Seals (freezing a billed day) are in [`crate::seals`].

use serde_json::{json, Value};

use crate::num::{field, round};

/// Gate thresholds. These are policy, not technical constants: how much
/// estimation or reconciliation drift a company accepts is its own call,
/// so the chart can override every one of them.
#[derive(Debug, Clone, PartialEq)]
pub struct Thresholds {
    /// Coverage (%) at or above which a period bills as is.
    pub coverage_ok: f64,
    /// At or above this it bills but must be marked; below it is blocked.
    pub coverage_mark: f64,
    /// Estimated share (%) at or below which numbers count as clean.
    pub estimated_ok: f64,
    /// At or below this they must be marked; above it is blocked.
    pub estimated_mark: f64,
    /// Reconciliation drift (%), allocated total vs assets.
    pub drift_ok: f64,
    pub drift_mark: f64,
    /// Unallocated share (%) above which governance needs attention (never blocks).
    pub unallocated_mark: f64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            coverage_ok: 99.0,
            coverage_mark: 95.0,
            estimated_ok: 1.0,
            estimated_mark: 10.0,
            drift_ok: 0.5,
            drift_mark: 2.0,
            unallocated_mark: 20.0,
        }
    }
}

impl Thresholds {
    pub fn to_json(&self) -> Value {
        json!({
            "coverageOk": self.coverage_ok,
            "coverageMark": self.coverage_mark,
            "estimatedOk": self.estimated_ok,
            "estimatedMark": self.estimated_mark,
            "driftOk": self.drift_ok,
            "driftMark": self.drift_mark,
            "unallocatedMark": self.unallocated_mark,
        })
    }
}

// ── Rate card ────────────────────────────────────────────────────────────

/// The rate card: a list of versions, each effective from a date. A price
/// change mid-period must not rewrite the earlier part, so it is never a
/// single set of numbers.
#[derive(Debug, Clone, PartialEq)]
pub struct RateCard {
    pub currency: Option<String>,
    pub note: Option<String>,
    /// Sorted by `effective` (`YYYY-MM-DD`).
    pub versions: Vec<Value>,
}

impl RateCard {
    /// From the JSON file. `None` when it has no versions: callers must
    /// work without a rate card (prices are then implied from the data).
    pub fn parse(text: &str) -> Option<Self> {
        let card: Value = serde_json::from_str(text).ok()?;
        let mut versions: Vec<Value> = card.get("versions")?.as_array()?.clone();
        versions.retain(Value::is_object);
        if versions.is_empty() {
            return None;
        }
        versions.sort_by(|a, b| effective(a).cmp(effective(b)));
        let s = |k: &str| card.get(k).and_then(Value::as_str).map(str::to_string);
        Some(Self {
            currency: s("currency"),
            note: s("note"),
            versions,
        })
    }

    /// The version in force at a moment: the last one whose effective date
    /// is on or before that day, as contracts count it.
    pub fn rate_at(&self, when_iso: Option<&str>) -> Option<&Value> {
        let day = when_iso.map(|s| &s[..s.len().min(10)]).unwrap_or("");
        let mut chosen = None;
        for v in &self.versions {
            if effective(v) <= day {
                chosen = Some(v);
            } else {
                break;
            }
        }
        chosen
    }
}

fn effective(v: &Value) -> &str {
    v.get("effective").and_then(Value::as_str).unwrap_or("")
}

/// Prices per resource: `cpu` per vCPU hour, `ram` and `storage` per GiB
/// hour (2^30 bytes). `None` when unknown.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Rates {
    pub cpu: Option<f64>,
    pub ram: Option<f64>,
    pub storage: Option<f64>,
}

pub const RESOURCES: [&str; 3] = ["cpu", "ram", "storage"];

impl Rates {
    pub fn get(&self, key: &str) -> Option<f64> {
        match key {
            "cpu" => self.cpu,
            "ram" => self.ram,
            "storage" => self.storage,
            _ => None,
        }
    }

    /// From a rate card version.
    pub fn from_version(v: &Value) -> Self {
        let g = |k: &str| v.get(k).and_then(Value::as_f64);
        Self {
            cpu: g("cpu"),
            ram: g("ram"),
            storage: g("storage"),
        }
    }

    /// The rate card where it has a price, else `fallback`.
    pub fn or(self, fallback: Rates) -> Rates {
        Rates {
            cpu: self.cpu.or(fallback.cpu),
            ram: self.ram.or(fallback.ram),
            storage: self.storage.or(fallback.storage),
        }
    }

    pub fn to_json(&self) -> Value {
        json!({ "cpu": self.cpu, "ram": self.ram, "storage": self.storage })
    }
}

/// Below this share of total cost (0.01 = 1 %) a resource's price drift is
/// too small to matter for the invoice.
pub const MATERIAL_SHARE: f64 = 0.01;

/// Compare the rate card's prices with the ones OpenCost actually used.
///
/// A mismatch means someone changed one side only: the invoice will never
/// agree with the system, and nothing reports an error. So it is a hard
/// gate condition.
///
/// But an implied price is unreliable when the money is tiny: over a
/// two-minute window storage may cost 0.0001, and dividing by that gives
/// 5 % "drift" on 0.15 % of the bill. Blocking the whole invoice for that
/// is a false alarm, and a false "cannot bill" hurts trust more than no
/// check. So resources below [`MATERIAL_SHARE`] are reported as `minor`.
pub fn rate_check(
    configured: &Rates,
    implied: &Rates,
    cost_share: &[(&str, Option<f64>)],
    tolerance_pct: f64,
) -> Value {
    let mut out = serde_json::Map::new();
    for key in RESOURCES {
        let (c, i) = (configured.get(key), implied.get(key));
        let (c, i) = match (c, i) {
            (Some(c), Some(i)) => (c, i),
            _ => {
                out.insert(
                    key.into(),
                    json!({ "configured": c, "implied": i, "status": "unknown" }),
                );
                continue;
            }
        };
        let drift = if i != 0.0 {
            (c - i).abs() / i * 100.0
        } else if c == 0.0 {
            0.0
        } else {
            100.0
        };
        let share = cost_share
            .iter()
            .find(|(k, _)| *k == key)
            .and_then(|(_, s)| *s);
        let mut row = json!({
            "configured": round(c, 6),
            "implied": round(i, 6),
            "driftPct": round(drift, 3),
        });
        if let Some(s) = share {
            row["costShare"] = json!(round(s * 100.0, 3));
        }
        row["status"] = if drift <= tolerance_pct {
            json!("ok")
        } else if share.is_some_and(|s| s < MATERIAL_SHARE) {
            row["impactPct"] = json!(round(drift * share.unwrap_or(0.0), 4));
            json!("minor")
        } else {
            json!("mismatch")
        };
        out.insert(key.into(), row);
    }
    Value::Object(out)
}

// ── Reconciliation ───────────────────────────────────────────────────────

/// Does the allocated total match what the assets (nodes, disks) cost?
///
/// It must be compared with the total **including idle**: idle is the
/// part of the nodes nobody requested, and it is part of the asset cost.
/// Without it the allocation looks 26 % short and you hunt a leak that
/// does not exist.
///
/// An asset total of zero or below is not a cost, it is OpenCost not having
/// priced the nodes yet (right after it starts the total can even be
/// negative for a few minutes). Reconciling against it would report drift
/// of hundreds of percent, so it is "unknown" with the reason.
pub fn reconcile(allocated: f64, assets: Option<f64>, t: &Thresholds) -> Value {
    match assets {
        Some(a) if a > 0.0 => {
            let drift = (allocated - a) / a * 100.0;
            let status = if drift.abs() <= t.drift_ok {
                "ok"
            } else if drift.abs() <= t.drift_mark {
                "mark"
            } else {
                "block"
            };
            json!({
                "allocated": round(allocated, 4),
                "assets": round(a, 4),
                "driftPct": round(drift, 3),
                "status": status,
            })
        }
        Some(a) => json!({
            "allocated": round(allocated, 4),
            "assets": round(a, 4),
            "driftPct": null,
            "status": "unknown",
            "reason": "assets_not_positive",
        }),
        None => json!({
            "allocated": round(allocated, 4),
            "assets": null,
            "driftPct": null,
            "status": "unknown",
            "reason": "assets_unavailable",
        }),
    }
}

// ── Billing gate ─────────────────────────────────────────────────────────

fn grade(value: Option<f64>, ok: f64, mark: f64, higher_is_better: bool) -> &'static str {
    match value {
        None => "unknown",
        Some(v) if higher_is_better => {
            if v >= ok {
                "ok"
            } else if v >= mark {
                "mark"
            } else {
                "block"
            }
        }
        Some(v) => {
            if v <= ok {
                "ok"
            } else if v <= mark {
                "mark"
            } else {
                "block"
            }
        }
    }
}

/// Inputs of [`billing_gate`].
pub struct GateInput<'a> {
    pub has_data: bool,
    pub coverage_pct: Option<f64>,
    pub estimated_pct: f64,
    pub reconciliation: &'a Value,
    /// Output of [`rate_check`]; `None` without a rate card or without a
    /// version in force for the period.
    pub rates: Option<&'a Value>,
    /// A rate card is loaded (so `rates: None` means no version applies).
    pub rate_card: bool,
    pub unallocated_pct: f64,
}

/// Can these numbers be used to charge anyone?
///
/// * `ok`    — bill as is
/// * `mark`  — bill, but the invoice must state how much is estimated
/// * `block` — needs someone to sign off on the estimate
///
/// `block` does not mean "no invoice"; it means someone takes
/// responsibility for it, and that is recorded.
///
/// Checks carry codes; the page words them in the reader's language.
pub fn billing_gate(input: &GateInput<'_>, t: &Thresholds) -> Value {
    let mut checks = Vec::new();

    // First, because with no data every check below looks perfect: 100 %
    // coverage, 0 % estimated, 0 % unallocated, zero drift. A freshly
    // installed cluster without labels is exactly that, and it is the
    // worst moment to say "all good".
    let mut has = json!({
        "code": "has_data", "value": null, "unit": "",
        "status": if input.has_data { "ok" } else { "block" },
    });
    if !input.has_data {
        has["detail"] = json!({ "code": "no_data" });
    }
    checks.push(has);

    // Without data the share-based checks have nothing to measure: 0 %
    // estimated or 0 % unallocated of nothing is not a pass. They report
    // "unknown" so the page never shows "passed" next to an empty period.
    let measured = |status: &'static str| if input.has_data { status } else { "unknown" };
    checks.push(json!({
        "code": "coverage", "value": input.coverage_pct, "unit": "%",
        "status": measured(grade(input.coverage_pct, t.coverage_ok, t.coverage_mark, true)),
    }));
    checks.push(json!({
        "code": "estimated", "value": input.has_data.then_some(input.estimated_pct), "unit": "%",
        "status": measured(grade(Some(input.estimated_pct), t.estimated_ok, t.estimated_mark, false)),
    }));
    let recon = input.reconciliation;
    checks.push(json!({
        "code": "reconciliation", "value": recon.get("driftPct").cloned().unwrap_or(Value::Null), "unit": "%",
        "status": recon.get("status").and_then(Value::as_str).unwrap_or("unknown"),
    }));

    let empty = serde_json::Map::new();
    let rates = input.rates.and_then(Value::as_object).unwrap_or(&empty);
    let bad: Vec<&str> = RESOURCES
        .iter()
        .copied()
        .filter(|k| rates.get(*k).and_then(|r| r.get("status")) == Some(&json!("mismatch")))
        .collect();
    let minor: Vec<Value> = RESOURCES
        .iter()
        .filter_map(|k| {
            let r = rates.get(*k)?;
            (r.get("status") == Some(&json!("minor"))).then(|| {
                json!({
                    "resource": k,
                    "driftPct": r.get("driftPct"),
                    "costShare": r.get("costShare"),
                    "impactPct": r.get("impactPct"),
                })
            })
        })
        .collect();
    let (status, detail) = if !bad.is_empty() {
        (
            "block",
            json!({ "code": "rates_mismatch", "args": { "resources": bad } }),
        )
    } else if !minor.is_empty() {
        (
            "mark",
            json!({ "code": "rates_minor", "args": { "items": minor } }),
        )
    } else if rates.is_empty() && input.rate_card {
        // A rate card exists, but no version is in force for this period
        // (it has no data, or it is before the first effective date).
        ("unknown", json!({ "code": "rates_no_version" }))
    } else if rates.is_empty() {
        ("unknown", json!({ "code": "rates_unknown" }))
    } else {
        ("ok", json!({ "code": "rates_ok" }))
    };
    checks.push(json!({
        "code": "rates", "value": null, "unit": "", "status": status, "detail": detail,
    }));
    checks.push(json!({
        "code": "unallocated", "value": input.has_data.then_some(input.unallocated_pct), "unit": "%",
        "status": measured(if input.unallocated_pct <= t.unallocated_mark { "ok" } else { "mark" }),
    }));

    let rank = |s: &str| match s {
        "block" => 3,
        "mark" | "unknown" => 2,
        _ => 1,
    };
    let worst = checks
        .iter()
        .map(|c| rank(c["status"].as_str().unwrap_or("ok")))
        .max()
        .unwrap_or(1);
    let verdict = match worst {
        3 => "block",
        2 => "mark",
        _ => "ok",
    };
    json!({ "verdict": verdict, "checks": checks, "thresholds": t.to_json() })
}

/// Implied price of a resource from the labelled departments: cost ÷ usage.
/// Only departments count: `__idle__` has cost but zero usage and would
/// inflate the price.
pub fn implied_rate(by_cc: &Value, cost_key: &str, usage_key: &str, div: f64) -> Option<f64> {
    let rows: Vec<&Value> = by_cc
        .as_object()
        .map(|m| {
            m.iter()
                .filter(|(k, _)| !k.starts_with("__"))
                .map(|(_, v)| v)
                .collect()
        })
        .unwrap_or_default();
    let cost: f64 = rows.iter().map(|r| field(r, cost_key)).sum();
    let usage: f64 = rows.iter().map(|r| field(r, usage_key)).sum::<f64>() / div;
    (usage != 0.0).then(|| round(cost / usage, 6))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card() -> RateCard {
        RateCard::parse(
            r#"{"currency":"TWD","versions":[
                {"effective":"2026-10-01","cpu":2.0,"ram":0.5,"storage":0.002},
                {"effective":"2026-01-01","cpu":1.0,"ram":0.25,"storage":0.001}]}"#,
        )
        .unwrap()
    }

    #[test]
    fn rate_card_versions() {
        let c = card();
        assert_eq!(c.versions[0]["effective"], "2026-01-01");
        assert_eq!(c.rate_at(Some("2026-09-30T23:59:59Z")).unwrap()["cpu"], 1.0);
        assert_eq!(c.rate_at(Some("2026-10-01T00:00:00Z")).unwrap()["cpu"], 2.0);
        assert!(c.rate_at(Some("2025-12-31")).is_none());
        assert!(c.rate_at(None).is_none());
        assert!(RateCard::parse(r#"{"versions":[]}"#).is_none());
        assert!(RateCard::parse("not json").is_none());
    }

    #[test]
    fn rate_check_marks_immaterial_drift_minor() {
        let conf = Rates {
            cpu: Some(1.0),
            ram: Some(0.25),
            storage: Some(0.001),
        };
        let imp = Rates {
            cpu: Some(1.0),
            ram: Some(0.2501),
            storage: Some(0.00106),
        };
        let r = rate_check(
            &conf,
            &imp,
            &[
                ("cpu", Some(0.8)),
                ("ram", Some(0.1985)),
                ("storage", Some(0.0015)),
            ],
            0.5,
        );
        assert_eq!(r["cpu"]["status"], "ok");
        assert_eq!(r["ram"]["status"], "ok");
        assert_eq!(r["storage"]["status"], "minor");
        let r = rate_check(&conf, &imp, &[("storage", Some(0.5))], 0.5);
        assert_eq!(r["storage"]["status"], "mismatch");
        let r = rate_check(&conf, &Rates::default(), &[], 0.5);
        assert_eq!(r["cpu"]["status"], "unknown");
    }

    #[test]
    fn reconciliation_grades() {
        let t = Thresholds::default();
        assert_eq!(reconcile(100.0, Some(100.3), &t)["status"], "ok");
        assert_eq!(reconcile(100.0, Some(101.5), &t)["status"], "mark");
        assert_eq!(reconcile(74.0, Some(100.0), &t)["status"], "block");
        assert_eq!(reconcile(1.0, None, &t)["status"], "unknown");
        assert_eq!(reconcile(1.0, Some(0.0), &t)["status"], "unknown");
        // OpenCost right after a restart: negative asset cost is not a basis.
        let r = reconcile(0.4353, Some(-0.1282), &t);
        assert_eq!(
            (r["status"].as_str(), r["reason"].as_str()),
            (Some("unknown"), Some("assets_not_positive"))
        );
        assert_eq!(reconcile(1.0, None, &t)["reason"], "assets_unavailable");
    }

    fn gate(
        has_data: bool,
        cov: Option<f64>,
        est: f64,
        rates: Option<&Value>,
        unalloc: f64,
    ) -> Value {
        let t = Thresholds::default();
        let recon = reconcile(100.0, Some(100.0), &t);
        billing_gate(
            &GateInput {
                has_data,
                coverage_pct: cov,
                estimated_pct: est,
                reconciliation: &recon,
                rates,
                rate_card: rates.is_some(),
                unallocated_pct: unalloc,
            },
            &t,
        )
    }

    #[test]
    fn gate_verdicts() {
        let ok = json!({"cpu":{"status":"ok"},"ram":{"status":"ok"},"storage":{"status":"ok"}});
        assert_eq!(gate(true, Some(99.5), 0.0, Some(&ok), 5.0)["verdict"], "ok");
        // No data blocks even though every other check looks perfect.
        let g = gate(false, Some(100.0), 0.0, Some(&ok), 0.0);
        assert_eq!(g["verdict"], "block");
        assert_eq!(g["checks"][0]["code"], "has_data");
        assert_eq!(g["checks"][0]["detail"]["code"], "no_data");
        assert_eq!(
            gate(true, Some(96.0), 0.0, Some(&ok), 5.0)["verdict"],
            "mark"
        );
        assert_eq!(
            gate(true, Some(90.0), 0.0, Some(&ok), 5.0)["verdict"],
            "block"
        );
        assert_eq!(
            gate(true, Some(99.5), 11.0, Some(&ok), 5.0)["verdict"],
            "block"
        );
        // Unallocated never blocks.
        assert_eq!(
            gate(true, Some(99.5), 0.0, Some(&ok), 60.0)["verdict"],
            "mark"
        );
        // No rate card is "unknown", which counts as mark.
        let g = gate(true, Some(99.5), 0.0, None, 5.0);
        assert_eq!(g["verdict"], "mark");
        assert_eq!(g["checks"][4]["detail"]["code"], "rates_unknown");
        let bad = json!({"cpu":{"status":"mismatch"},"ram":{"status":"ok"}});
        let g = gate(true, Some(99.5), 0.0, Some(&bad), 5.0);
        assert_eq!(g["verdict"], "block");
        assert_eq!(
            g["checks"][4]["detail"]["args"]["resources"],
            json!(["cpu"])
        );
        assert_eq!(g["thresholds"]["coverageOk"], 99.0);
        // No data: share checks are unknown, never "passed".
        let g = gate(false, Some(100.0), 0.0, Some(&ok), 0.0);
        for i in [1, 2, 5] {
            assert_eq!(g["checks"][i]["status"], "unknown", "{}", g["checks"][i]);
        }
        assert!(g["checks"][2]["value"].is_null());
        // Missing coverage is unknown, not ok.
        assert_eq!(
            gate(true, None, 0.0, Some(&ok), 5.0)["checks"][1]["status"],
            "unknown"
        );
    }

    #[test]
    fn implied_rates_ignore_idle() {
        let by_cc = json!({
            "rd": {"cpuCost": 2.0, "cpuCoreHours": 2.0},
            "__idle__": {"cpuCost": 10.0, "cpuCoreHours": 0.0},
        });
        assert_eq!(
            implied_rate(&by_cc, "cpuCost", "cpuCoreHours", 1.0),
            Some(1.0)
        );
        assert_eq!(
            implied_rate(&json!({}), "cpuCost", "cpuCoreHours", 1.0),
            None
        );
    }
}
