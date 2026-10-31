//! `cost-report fallback`: allocation from snapshot files, optionally
//! compared with OpenCost's measured allocation.
//!
//! A fallback whose error was never measured is barely better than none:
//! when it is needed you cannot tell whether to trust it. Run `--compare`
//! over a period with good data to know how far declared requests are
//! from what OpenCost measured. Declared requests under-count workloads
//! that use more than they request (OpenCost bills max(request, usage)).

use std::path::PathBuf;
use std::time::Duration;

use cost_report_core::num::{field, GIB};
use cost_report_core::snapshot;
use cost_report_core::time::{fmt_iso, now, parse_iso};
use serde_json::Value;

use crate::upstream::{OpenCost, Upstream};

pub struct Args {
    pub snapshots: PathBuf,
    pub start: i64,
    pub end: i64,
    pub compare: Option<String>,
    pub label: String,
    pub rates: Option<(f64, f64, f64)>,
    pub json: bool,
}

fn usage() -> String {
    "usage: cost-report fallback --snapshots DIR (--start T --end T | --last 30m) [--compare OPENCOST_URL] \
     [--label cost-center] [--cpu-rate X --ram-rate Y [--pv-rate Z]] [--json]"
        .into()
}

pub fn parse(args: &[String]) -> Result<Args, String> {
    let mut it = args.iter();
    let (mut snaps, mut start, mut end, mut last, mut compare) = (None, None, None, None, None);
    let (mut cpu, mut ram, mut pv, mut json_out, mut label) =
        (None, None, None, false, "cost-center".to_string());
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{a} needs a value\n{}", usage()))
        };
        match a.as_str() {
            "--snapshots" => snaps = Some(PathBuf::from(val()?)),
            "--start" => start = Some(parse_iso(&val()?)?),
            "--end" => end = Some(parse_iso(&val()?)?),
            "--last" => last = Some(val()?),
            "--compare" => compare = Some(val()?),
            "--label" => label = val()?,
            "--cpu-rate" => {
                cpu = Some(
                    val()?
                        .parse::<f64>()
                        .map_err(|_| "--cpu-rate must be a number")?,
                )
            }
            "--ram-rate" => {
                ram = Some(
                    val()?
                        .parse::<f64>()
                        .map_err(|_| "--ram-rate must be a number")?,
                )
            }
            "--pv-rate" => {
                pv = Some(
                    val()?
                        .parse::<f64>()
                        .map_err(|_| "--pv-rate must be a number")?,
                )
            }
            "--json" => json_out = true,
            other => return Err(format!("unknown argument {other:?}\n{}", usage())),
        }
    }
    let snapshots = snaps.ok_or_else(usage)?;
    let (start, end) = match (start, end, last) {
        (_, _, Some(l)) => {
            let mins = cost_report_core::coverage::nominal_minutes(&l)
                .ok_or_else(|| format!("cannot read {l:?} (like 30m, 2h, 1d)"))?;
            let e = now().floor() as i64;
            (e - (mins * 60.0) as i64, e)
        }
        (Some(s), Some(e), None) => (s, e),
        _ => return Err(usage()),
    };
    if start >= end {
        return Err("start must be before end".into());
    }
    Ok(Args {
        snapshots,
        start,
        end,
        compare,
        label,
        rates: cpu.zip(ram).map(|(c, r)| (c, r, pv.unwrap_or(0.0))),
        json: json_out,
    })
}

fn measured(url: &str, label: &str, start: i64, end: i64) -> Result<(Value, f64), String> {
    let oc = OpenCost(Upstream::new(url, Duration::from_secs(60)));
    let window = format!("{},{}", fmt_iso(start as f64), fmt_iso(end as f64));
    let set = oc
        .allocation(
            &window,
            &format!("label:{label}"),
            &[("includeIdle", "false")],
        )
        .map_err(|e| format!("OpenCost: {e}"))?;
    // OpenCost often covers less than asked; rates per hour make the two comparable.
    let minutes = set
        .as_object()
        .into_iter()
        .flatten()
        .map(|(_, a)| field(a, "minutes"))
        .fold(0.0, f64::max);
    Ok((set, minutes))
}

pub fn run(a: Args) -> Result<(), String> {
    let snaps = crate::snapshotter::load(&a.snapshots, a.start, a.end);
    let doc = snapshot::allocation(&snaps, a.start, a.end).ok_or_else(|| {
        format!(
            "no snapshots for {} ~ {}",
            fmt_iso(a.start as f64),
            fmt_iso(a.end as f64)
        )
    })?;
    if a.json {
        println!("{}", serde_json::to_string_pretty(&doc).unwrap_or_default());
        return Ok(());
    }
    let depts = doc["departments"].as_object().cloned().unwrap_or_default();
    let cov = doc["snapshotCoveragePct"].as_f64().unwrap_or(0.0);
    let found = doc["samples"]["found"].as_u64().unwrap_or(0);
    let interval = doc["samples"]["intervalSeconds"].as_f64().unwrap_or(300.0);
    println!(
        "window     {} ~ {} ({:.2} h)",
        fmt_iso(a.start as f64),
        fmt_iso(a.end as f64),
        (a.end - a.start) as f64 / 3600.0
    );
    println!(
        "snapshots  {found}/{} at {interval}s, {cov:.1}% complete{}",
        doc["samples"]["expected"],
        if cov >= 99.0 {
            ""
        } else {
            "  ← the backup itself has gaps; treat as indicative"
        }
    );
    println!("basis      declared requests (no Prometheus needed)\n");

    let total_cpu: f64 = depts.values().map(|d| field(d, "cpuCoreHours")).sum();
    let (m_set, m_min) = match &a.compare {
        Some(url) => measured(url, &a.label, a.start, a.end).map(|(s, m)| (Some(s), m))?,
        None => (None, 0.0),
    };
    let m_cpu_total: f64 = m_set
        .as_ref()
        .and_then(Value::as_object)
        .map(|m| m.values().map(|v| field(v, "cpuCoreHours")).sum())
        .unwrap_or(0.0);
    let mut head = format!(
        "{:<20}{:>14}{:>14}{:>14}{:>9}",
        "department", "core-h", "RAM GiB-h", "PV GiB-h", "share%"
    );
    if m_set.is_some() {
        head += &format!("{:>11}{:>9}", "measured%", "Δpp");
    }
    if a.rates.is_some() {
        head += &format!("{:>12}", "amount");
    }
    println!("{head}\n{}", "-".repeat(head.chars().count()));
    let mut rows: Vec<(&String, &Value)> = depts.iter().collect();
    rows.sort_by(|x, y| {
        field(y.1, "cpuCoreHours")
            .partial_cmp(&field(x.1, "cpuCoreHours"))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut worst: f64 = 0.0;
    for (d, r) in rows {
        let share = if total_cpu > 0.0 {
            field(r, "cpuCoreHours") / total_cpu * 100.0
        } else {
            0.0
        };
        let mut line = format!(
            "{d:<20}{:>14.4}{:>14.4}{:>14.4}{share:>9.2}",
            field(r, "cpuCoreHours"),
            field(r, "ramGiBHours"),
            field(r, "pvGiBHours")
        );
        if let Some(m) = &m_set {
            match m.get(d.as_str()) {
                Some(mv) if m_cpu_total > 0.0 => {
                    let ms = field(mv, "cpuCoreHours") / m_cpu_total * 100.0;
                    worst = worst.max((share - ms).abs());
                    line += &format!("{ms:>11.2}{:>+9.2}", share - ms);
                }
                _ => line += &format!("{:>11}{:>9}", "—", "—"),
            }
        }
        if let Some((c, ram, pv)) = a.rates {
            line += &format!(
                "{:>12.4}",
                field(r, "cpuCoreHours") * c
                    + field(r, "ramGiBHours") * ram
                    + field(r, "pvGiBHours") * pv
            );
        }
        println!("{line}");
    }
    if let Some(m) = &m_set {
        let hours_declared = found as f64 * interval / 3600.0;
        let hours_measured = if m_min > 0.0 {
            m_min / 60.0
        } else {
            hours_declared
        };
        let (d_rate, m_rate) = (total_cpu / hours_declared, m_cpu_total / hours_measured);
        let gap = if m_rate > 0.0 {
            (d_rate - m_rate) / m_rate * 100.0
        } else {
            0.0
        };
        let m_pv: f64 = m
            .as_object()
            .map(|o| o.values().map(|v| field(v, "pvByteHours") / GIB).sum())
            .unwrap_or(0.0);
        let d_pv: f64 = depts.values().map(|d| field(d, "pvGiBHours")).sum();
        println!("\nmeasured   {m_cpu_total:.4} core-h over {m_min:.1} min");
        println!(
            "as rates   declared {d_rate:.4} cores vs measured {m_rate:.4} cores ({gap:+.2}%)"
        );
        if m_pv > 0.0 {
            let (dp, mp) = (d_pv / hours_declared, m_pv / hours_measured);
            println!(
                "storage    declared {dp:.4} GiB vs measured {mp:.4} GiB ({:+.2}%)",
                (dp - mp) / mp * 100.0
            );
        } else {
            println!("storage    OpenCost has no pvByteHours for this window; not compared");
        }
        println!("\nlargest share difference: {worst:.2} percentage points");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn args() {
        let a = parse(&s(&[
            "--snapshots",
            "/d",
            "--start",
            "2026-09-24T05:00:00Z",
            "--end",
            "2026-09-24T05:30:00Z",
            "--json",
        ]))
        .unwrap();
        assert_eq!(a.end - a.start, 1800);
        assert!(a.json);
        assert!(a.rates.is_none());
        let a = parse(&s(&[
            "--snapshots",
            "/d",
            "--last",
            "2h",
            "--cpu-rate",
            "1",
            "--ram-rate",
            "0.25",
        ]))
        .unwrap();
        assert_eq!(a.end - a.start, 7200);
        assert_eq!(a.rates, Some((1.0, 0.25, 0.0)));
        assert!(parse(&s(&["--start", "x"])).is_err());
        assert!(parse(&s(&["--snapshots", "/d"])).is_err());
        assert!(parse(&s(&["--snapshots", "/d", "--bogus"])).is_err());
    }
}
