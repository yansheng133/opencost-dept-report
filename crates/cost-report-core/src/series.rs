//! Usage over time per department: usage ÷ request, segment by segment.
//!
//! Utilisation is not cost. Cost is "what was paid"; utilisation is "is
//! what was requested being used". Segments without data stay in the
//! series with `depts: null` so the line breaks there: joining two points
//! three days apart draws a line that looks measured, and those three days
//! may be a monitoring outage.

use serde_json::{json, Value};

use crate::num::{f, round};
use crate::steps;
use crate::time::{fmt_iso, parse_iso};

/// Inputs of [`build`].
pub struct SeriesInput<'a> {
    /// `data` of an `accumulate=false` allocation query: one map per segment.
    pub segments: &'a [Value],
    pub step: &'a str,
    /// Minutes of the window asked for.
    pub span: Option<f64>,
    pub default_step: Option<&'a str>,
    /// The axis never extends past this (now, or the end of an absolute window).
    pub limit: f64,
}

fn rows(seg: &Value) -> Vec<(&String, &Value)> {
    seg.as_object()
        .into_iter()
        .flatten()
        .filter(|(k, v)| !k.starts_with("__") && v.is_object())
        .collect()
}

pub fn build(input: &SeriesInput<'_>) -> Option<Value> {
    let step_min = steps::parse_step(input.step)?;

    // Empty segments have no start, but every segment has the same length,
    // so times follow from one anchor. The anchor must be a **full**
    // segment: OpenCost cuts the first segment's start to the first data
    // point, and anchoring on that shifted a 7-day chart five hours into
    // the future.
    let mut anchor: Option<(usize, i64)> = None;
    'outer: for full_only in [true, false] {
        for (i, seg) in input.segments.iter().enumerate() {
            let Some((_, row)) = rows(seg).into_iter().next() else {
                continue;
            };
            let Some(start) = row.get("start").and_then(Value::as_str) else {
                continue;
            };
            if full_only && f(row.get("minutes")) < step_min * 0.9 {
                continue;
            }
            if let Ok(ts) = parse_iso(start) {
                anchor = Some((i, ts));
                break 'outer;
            }
        }
    }
    let (base_idx, base_ts) = anchor?;
    let seg_start = |i: usize| base_ts as f64 + (i as f64 - base_idx as f64) * step_min * 60.0;

    let mut points = Vec::new();
    let mut covered = 0;
    for (idx, seg) in input.segments.iter().enumerate() {
        // A segment starting after the limit is an empty cell in the future.
        // Allowing one step of grace was wrong: with 6 h steps the axis
        // reached half a day ahead.
        if seg_start(idx) > input.limit {
            continue;
        }
        let rs = rows(seg);
        let minutes = rs.first().map(|(_, v)| f(v.get("minutes"))).unwrap_or(0.0);
        // Slivers of a few minutes at the edges would become spikes that
        // never happened; treat them as no data.
        // At most an hour: with usage per covered minute (num::cpu_usage) a
        // few hours of a 1-day step are meaningful, and dropping them left a
        // 7-day chart at 1 d with no points at all.
        if rs.is_empty() || minutes < (step_min * 0.25).min(60.0) {
            points.push(json!({ "start": fmt_iso(seg_start(idx)), "depts": null }));
            continue;
        }
        covered += 1;
        let mut depts = serde_json::Map::new();
        for (dept, v) in &rs {
            let req_cpu = f(v.get("cpuCoreRequestAverage"));
            let req_ram = f(v.get("ramByteRequestAverage"));
            // No request, no utilisation: null so the line breaks.
            depts.insert(
                (*dept).clone(),
                json!({
                    "cpu": crate::num::cpu_efficiency(v, Some(step_min)).map(|e| round(e, 4)),
                    "ram": (req_ram > 0.0).then(|| round(f(v.get("ramEfficiency")), 4)),
                    "cpuReq": round(req_cpu, 3),
                    "cpuUse": round(crate::num::cpu_usage(v, Some(step_min)), 3),
                }),
            );
        }
        let first = rs[0].1;
        points.push(json!({
            "start": first.get("start"),
            "end": first.get("end"),
            "minutes": round(minutes, 1),
            // Less than 90 % of the step: the first segment after a gap or at
            // the start of data, or the unfinished last one. Averages over a
            // part-segment read low (a 20-minute first hour showed a third of
            // the usage), so the page draws these dashed.
            "partial": minutes < step_min * 0.9,
            "depts": depts,
        }));
    }
    if covered == 0 {
        return None;
    }
    Some(json!({
        "step": input.step,
        "stepMinutes": step_min,
        "points": points,
        "covered": covered,
        "expected": input.segments.len(),
        "options": steps::options(input.span),
        "defaultStep": input.default_step,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(start: &str, minutes: f64) -> Value {
        json!({
            "rd": {"start": start, "end": "", "minutes": minutes, "cpuCoreRequestAverage": 1.0,
                   "cpuEfficiency": 0.6667, "ramByteRequestAverage": 0.0, "cpuCoreUsageAverage": 0.6667},
            "__idle__": {"start": start, "minutes": minutes},
        })
    }

    #[test]
    fn keeps_gaps_and_stops_at_now() {
        let t0 = parse_iso("2026-09-24T00:00:00Z").unwrap();
        let segs = vec![
            seg("2026-09-24T00:08:00Z", 2.0), // cut first segment: not the anchor
            seg("2026-09-24T00:10:00Z", 10.0),
            json!({}),
            seg("2026-09-24T00:30:00Z", 10.0),
            json!({}), // the future
        ];
        let s = build(&SeriesInput {
            segments: &segs,
            step: "10m",
            span: Some(60.0),
            default_step: Some("10m"),
            limit: (t0 + 35 * 60) as f64,
        })
        .unwrap();
        let pts = s["points"].as_array().unwrap();
        assert_eq!(pts.len(), 4);
        // The 2-minute sliver is below a quarter step: no data, at the anchored time.
        assert_eq!(pts[0]["start"], "2026-09-24T00:00:00Z");
        assert!(pts[0]["depts"].is_null());
        assert_eq!(pts[1]["depts"]["rd"]["cpu"], 0.6667);
        assert_eq!(pts[1]["partial"], false);
        assert!(pts[1]["depts"]["rd"]["ram"].is_null());
        assert!(pts[1]["depts"].get("__idle__").is_none());
        assert_eq!(pts[2]["start"], "2026-09-24T00:20:00Z");
        assert!(pts[2]["depts"].is_null());
        assert_eq!(s["covered"], 2);
        assert_eq!(s["expected"], 5);
    }

    #[test]
    fn short_segment_of_a_long_step_counts() {
        // A 1-day step with 290 minutes of data today: kept (not under the
        // 60-minute cap), partial, and its CPU usage per covered minute.
        let segs = vec![
            json!({"rd": {"start": "2026-10-31T01:20:00Z", "minutes": 290.0,
            "cpuCoreRequestAverage": 0.1, "cpuCoreUsageAverage": 0.0024 * 1440.0 / 1440.0}}),
        ];
        let s = build(&SeriesInput {
            segments: &segs,
            step: "1d",
            span: Some(10_080.0),
            default_step: None,
            limit: 1e12,
        })
        .unwrap();
        let p = &s["points"][0];
        assert_eq!(p["partial"], true);
        assert!(
            (p["depts"]["rd"]["cpu"].as_f64().unwrap() - 0.0024 * 1440.0 / 290.0 / 0.1).abs()
                < 1e-3
        );
    }

    #[test]
    fn nothing_covered() {
        let segs = vec![json!({}), json!({})];
        assert!(build(&SeriesInput {
            segments: &segs,
            step: "1h",
            span: Some(120.0),
            default_step: None,
            limit: 1e12
        })
        .is_none());
        assert!(build(&SeriesInput {
            segments: &segs,
            step: "bad",
            span: None,
            default_step: None,
            limit: 0.0
        })
        .is_none());
    }
}
