//! How much of a window was actually measured, and where the gaps are.
//!
//! Without this a gap is silent: the report still renders, some money is
//! simply not counted, and nobody notices. OpenCost does not say "I did not
//! measure this part", so the answer comes from Prometheus' `up` series of
//! the OpenCost scrape job.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::num::round;
use crate::time::{fmt_iso, parse_iso};

/// Length in minutes of the window that was **asked for**.
///
/// That is the right denominator. An early version divided by the window
/// OpenCost returned, so a day cut to its last 60 minutes, with no gap
/// inside those 60 minutes, showed "100 % coverage, ok to bill". With the
/// wrong denominator the thinnest data looks the most complete.
pub fn nominal_minutes(window: &str) -> Option<f64> {
    let w = window.trim();
    if let Some((a, b)) = w.split_once(',') {
        let (a, b) = (parse_iso(a.trim()).ok()?, parse_iso(b.trim()).ok()?);
        return Some(((b - a) as f64 / 60.0).max(1.0));
    }
    let (num, unit) = w.split_at(w.len().checked_sub(1)?);
    let mult = match unit {
        "m" => 1.0,
        "h" => 60.0,
        "d" => 1440.0,
        _ => return None,
    };
    num.parse::<f64>().ok().map(|n| n * mult)
}

/// The absolute range to ask OpenCost for, as epoch seconds.
///
/// OpenCost aligns relative windows to the clock hour and includes the part
/// of the hour that has not happened yet: at 04:38, "1h" is 04:00–05:00 and
/// "24h" ends at 05:00. Counted against the window asked for, that future
/// time looked like 22 missing minutes, so "1h" could never pass the
/// coverage check, and a day still running looked mostly unmeasured. So a
/// relative window becomes the last N minutes up to now, and an absolute
/// one that ends in the future is cut at now. Both are minute-aligned.
pub fn resolve_window(window: &str, now: f64) -> Option<(i64, i64)> {
    let now_min = (now / 60.0).floor() as i64 * 60;
    let w = window.trim();
    if let Some((a, b)) = w.split_once(',') {
        let (a, b) = (parse_iso(a.trim()).ok()?, parse_iso(b.trim()).ok()?);
        let end = b.min(now_min);
        return (end > a).then_some((a, end));
    }
    let minutes = nominal_minutes(w)?;
    Some((now_min - (minutes * 60.0) as i64, now_min))
}

/// [`resolve_window`] as an OpenCost window string.
pub fn window_param(start: i64, end: i64) -> String {
    format!("{},{}", fmt_iso(start as f64), fmt_iso(end as f64))
}

/// The scrape interval (seconds) of `job`, from `/api/v1/targets` data.
/// Asked, not guessed: a wrong interval biases coverage up or down with no
/// visible symptom.
pub fn scrape_interval(targets: &Value, job: &str) -> Option<f64> {
    targets
        .get("activeTargets")?
        .as_array()?
        .iter()
        .filter(|t| t.pointer("/labels/job").and_then(Value::as_str) == Some(job))
        .find_map(|t| {
            let s = t.get("scrapeInterval")?.as_str()?;
            if let Some(v) = s.strip_suffix("ms") {
                v.parse::<f64>().ok().map(|x| x / 1000.0)
            } else if let Some(v) = s.strip_suffix('s') {
                v.parse().ok()
            } else if let Some(v) = s.strip_suffix('m') {
                v.parse::<f64>().ok().map(|x| x * 60.0)
            } else {
                None
            }
        })
}

/// Step (seconds) of the `up` range query over `span` seconds: the scrape
/// interval, but no more than 1500 points.
pub fn query_step(span: f64, scrape: Option<f64>) -> f64 {
    scrape.unwrap_or(60.0).max(span / 1500.0)
}

/// Inputs of [`coverage`].
pub struct CoverageInput<'a> {
    pub start: i64,
    pub end: i64,
    pub step: f64,
    /// Whether the step came from Prometheus (false: the 60 s default).
    pub measured_step: bool,
    /// `result` of `up{job=…}` over the window.
    pub series: &'a [Value],
    pub nominal: Option<f64>,
    pub measured_minutes: Option<f64>,
}

/// Coverage of a window. Two numbers, both needed:
///
/// * `scrapePct` — inside the part that has data, was Prometheus scraping? (gaps in the middle)
/// * `windowPct` — of the window asked for, how much has data at all? (cut-off edges)
///
/// `pct`, the product, is what the gate uses.
pub fn coverage(c: &CoverageInput<'_>) -> Option<Value> {
    let span = (c.end - c.start) as f64;
    if span <= 0.0 || c.step <= 0.0 {
        return None;
    }
    let expected = (span / c.step).floor() as usize + 1;
    let mut seen = BTreeSet::new();
    for s in c.series {
        for p in s
            .get("values")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let ts = p.get(0).and_then(Value::as_f64);
            // Any target of the job reporting counts as measured.
            if let (Some(ts), Some("1")) = (ts, p.get(1).and_then(Value::as_str)) {
                let i = ((ts - c.start as f64) / c.step).floor();
                if i >= 0.0 && (i as usize) < expected {
                    seen.insert(i as usize);
                }
            }
        }
    }
    let mut gaps: Vec<(usize, usize)> = Vec::new();
    let mut run: Option<usize> = None;
    for i in 0..expected {
        match (seen.contains(&i), run) {
            (false, None) => run = Some(i),
            (true, Some(s)) => {
                gaps.push((s, i));
                run = None;
            }
            _ => {}
        }
    }
    if let Some(s) = run {
        gaps.push((s, expected));
    }
    let iso = |i: usize| fmt_iso(c.start as f64 + i as f64 * c.step);
    let scrape_pct = round(seen.len() as f64 / expected as f64 * 100.0, 2);
    let window_pct = match (c.nominal, c.measured_minutes) {
        (Some(n), Some(m)) if n > 0.0 => Some(round((m / n * 100.0).min(100.0), 2)),
        _ => None,
    };
    let pct = window_pct.map_or(scrape_pct, |w| round(scrape_pct * w / 100.0, 2));
    let gap_minutes = gaps
        .iter()
        .map(|(a, b)| (b - a) as f64 * c.step)
        .sum::<f64>()
        / 60.0;
    // Largest few only: an invoice needs where and how long, not every point.
    let mut largest = gaps.clone();
    largest.sort_by_key(|(a, b)| std::cmp::Reverse(b - a));
    Some(json!({
        "available": true,
        "scrapePct": scrape_pct,
        "windowPct": window_pct,
        "requestedMinutes": c.nominal.map(|n| round(n, 1)),
        "measuredMinutes": c.measured_minutes.map(|m| round(m, 1)),
        "pct": pct,
        "measuredStep": c.measured_step,
        "stepSeconds": c.step as i64,
        "samples": { "found": seen.len(), "expected": expected },
        "gaps": largest.iter().take(5).map(|(a, b)| json!({
            "start": iso(*a), "end": iso(*b), "minutes": round((b - a) as f64 * c.step / 60.0, 1),
        })).collect::<Vec<_>>(),
        "gapMinutes": round(gap_minutes, 1),
    }))
}

/// Coverage could not be measured (Prometheus unreachable or not configured).
pub fn unavailable(reason: &str) -> Value {
    json!({ "available": false, "reason": reason })
}

/// How many scrape targets dropped out per job.
///
/// A partial outage is more dangerous than a total one: if all of them go
/// down you see it; if two nodes' exporters drop, the report still renders
/// and only those nodes' cost goes quietly missing. So compare targets
/// expected with targets reporting, per sample.
pub fn target_health(total: &[Value], healthy: &[Value]) -> Vec<Value> {
    fn series(data: &[Value]) -> std::collections::BTreeMap<String, Vec<f64>> {
        data.iter()
            .map(|s| {
                let job = s
                    .pointer("/metric/job")
                    .and_then(Value::as_str)
                    .unwrap_or("?")
                    .to_string();
                let vals = s
                    .get("values")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .map(|p| {
                        p.get(1)
                            .and_then(Value::as_str)
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(0.0)
                    })
                    .collect();
                (job, vals)
            })
            .collect()
    }
    let (tot, hea) = (series(total), series(healthy));
    tot.iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(job, values)| {
            let expected = values.iter().cloned().fold(0.0, f64::max);
            let h = hea.get(job).cloned().unwrap_or_default();
            let worst = h.iter().cloned().fold(f64::INFINITY, f64::min);
            let worst = if worst.is_finite() { worst } else { 0.0 };
            let degraded = values
                .iter()
                .enumerate()
                .filter(|(i, v)| h.get(*i).copied().unwrap_or(0.0) < **v)
                .count();
            json!({
                "job": job,
                "expected": expected as i64,
                "worstHealthy": worst as i64,
                "degradedPct": round(degraded as f64 / values.len() as f64 * 100.0, 2),
                "status": if worst >= expected && degraded == 0 { "ok" } else { "degraded" },
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nominal() {
        assert_eq!(nominal_minutes("24h"), Some(1440.0));
        assert_eq!(nominal_minutes("7d"), Some(10_080.0));
        assert_eq!(nominal_minutes("30m"), Some(30.0));
        assert_eq!(
            nominal_minutes("2026-09-24T00:00:00Z,2026-09-24T23:59:59Z").map(|m| m.round()),
            Some(1440.0)
        );
        assert_eq!(nominal_minutes("x"), None);
        assert_eq!(nominal_minutes(""), None);
    }

    #[test]
    fn resolves_windows_up_to_now() {
        let now = parse_iso("2026-10-31T04:38:30Z").unwrap() as f64;
        let (a, b) = resolve_window("1h", now).unwrap();
        assert_eq!(
            window_param(a, b),
            "2026-10-31T03:38:00Z,2026-10-31T04:38:00Z"
        );
        let (a, b) = resolve_window("24h", now).unwrap();
        assert_eq!(b - a, 86_400);
        // Today is cut at now; a past day is kept whole.
        let (a, b) = resolve_window("2026-10-31T00:00:00Z,2026-10-31T23:59:59Z", now).unwrap();
        assert_eq!(
            window_param(a, b),
            "2026-10-31T00:00:00Z,2026-10-31T04:38:00Z"
        );
        let (a, b) = resolve_window("2026-10-30T00:00:00Z,2026-10-30T23:59:59Z", now).unwrap();
        assert_eq!(
            window_param(a, b),
            "2026-10-30T00:00:00Z,2026-10-30T23:59:59Z"
        );
        // A day that has not started yet has nothing to ask for.
        assert!(resolve_window("2026-11-01T00:00:00Z,2026-11-01T23:59:59Z", now).is_none());
        assert!(resolve_window("bad", now).is_none());
    }

    #[test]
    fn scrape_interval_from_targets() {
        let t = json!({"activeTargets": [
            {"labels": {"job": "node"}, "scrapeInterval": "15s"},
            {"labels": {"job": "opencost"}, "scrapeInterval": "1m"}]});
        assert_eq!(scrape_interval(&t, "opencost"), Some(60.0));
        assert_eq!(scrape_interval(&t, "node"), Some(15.0));
        assert_eq!(scrape_interval(&t, "none"), None);
    }

    fn up(start: i64, step: i64, ups: &[bool]) -> Vec<Value> {
        let values: Vec<Value> = ups
            .iter()
            .enumerate()
            .map(|(i, u)| json!([(start + i as i64 * step) as f64, if *u { "1" } else { "0" }]))
            .collect();
        vec![json!({"metric": {"job": "opencost"}, "values": values})]
    }

    #[test]
    fn finds_gaps() {
        let start = 1_790_208_000;
        let mut ups = vec![true; 61];
        for u in ups.iter_mut().skip(10).take(5) {
            *u = false;
        }
        let series = up(start, 60, &ups);
        let c = coverage(&CoverageInput {
            start,
            end: start + 3600,
            step: 60.0,
            measured_step: true,
            series: &series,
            nominal: Some(60.0),
            measured_minutes: Some(60.0),
        })
        .unwrap();
        assert_eq!(c["samples"]["expected"], 61);
        assert_eq!(c["samples"]["found"], 56);
        assert_eq!(c["gapMinutes"], 5.0);
        assert_eq!(c["gaps"][0]["start"], fmt_iso((start + 600) as f64));
        assert_eq!(c["windowPct"], 100.0);
    }

    #[test]
    fn denominator_is_the_window_asked_for() {
        // A day with data only in its last hour: no gap inside that hour,
        // but the day is 4 % covered, not 100 %.
        let start = 1_790_208_000;
        let series = up(start, 60, &[true; 61]);
        let c = coverage(&CoverageInput {
            start,
            end: start + 3600,
            step: 60.0,
            measured_step: true,
            series: &series,
            nominal: Some(1440.0),
            measured_minutes: Some(60.0),
        })
        .unwrap();
        assert_eq!(c["scrapePct"], 100.0);
        assert_eq!(c["pct"], 4.17);
        assert!(coverage(&CoverageInput {
            start,
            end: start,
            step: 60.0,
            measured_step: true,
            series: &series,
            nominal: None,
            measured_minutes: None,
        })
        .is_none());
    }

    #[test]
    fn partial_target_loss() {
        let tot =
            vec![json!({"metric": {"job": "node"}, "values": [[0, "3"], [60, "3"], [120, "3"]]})];
        let hea =
            vec![json!({"metric": {"job": "node"}, "values": [[0, "3"], [60, "2"], [120, "3"]]})];
        let r = target_health(&tot, &hea);
        assert_eq!(r[0]["status"], "degraded");
        assert_eq!(r[0]["worstHealthy"], 2);
        assert_eq!(r[0]["degradedPct"], 33.33);
        let r = target_health(&tot, &tot);
        assert_eq!(r[0]["status"], "ok");
    }
}
