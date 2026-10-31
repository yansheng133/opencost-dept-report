//! `cost-report snapshotter`: store declared requests every few minutes,
//! and answer `GET /allocation?start=…&end=…` for the report's gaps.
//!
//! Files: `<OUT_DIR>/<YYYY-MM-DD>/<HHMMSS>.json`, about 6 KiB each; kept
//! `RETENTION_DAYS` (a billing period plus margin). `<OUT_DIR>/.heartbeat`
//! is touched after every successful snapshot for the liveness probe: the
//! failure worth catching is "running but no longer writing", which is
//! what silently eats the backup.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use cost_report_core::num::GIB;
use cost_report_core::snapshot::{self, Labels};
use cost_report_core::time::{fmt_day, fmt_iso, now, parse_iso};
use serde_json::{json, Value};

use crate::http::{Request, Response};
use crate::kube::Kube;
use crate::server::{sleep_unless, NAME};

pub struct Settings {
    pub out_dir: PathBuf,
    pub interval: u64,
    pub labels: Labels,
    pub retention_days: u32,
}

impl Settings {
    pub fn from_env() -> Result<Self, String> {
        let var = |k: &str, d: &str| {
            std::env::var(k)
                .ok()
                .filter(|v| !v.trim().is_empty())
                .unwrap_or_else(|| d.into())
        };
        let num = |k: &str, d: u64| -> Result<u64, String> {
            var(k, &d.to_string())
                .trim()
                .parse()
                .map_err(|_| format!("{k} must be a number"))
        };
        Ok(Self {
            out_dir: PathBuf::from(var("OUT_DIR", "/data")),
            interval: num("INTERVAL", 300)?.max(30),
            labels: Labels {
                department: var("DEPT_LABEL", "cost-center"),
                environment: var("ENV_LABEL", "env"),
            },
            retention_days: num("RETENTION_DAYS", 35)?.clamp(1, 400) as u32,
        })
    }
}

type Lists = (Vec<Value>, Vec<Value>, Vec<Value>);

/// Where lists come from: the API server, or `kubectl` (to preview what
/// would be collected before deploying; the fields are the same).
pub enum Source {
    Kube(Kube),
    Kubectl(Option<String>),
}

impl Source {
    fn kubectl(&self, args: &[&str]) -> Result<Vec<Value>, String> {
        let Source::Kubectl(cfg) = self else {
            unreachable!()
        };
        let mut cmd = Command::new("kubectl");
        if let Some(c) = cfg {
            cmd.args(["--kubeconfig", c]);
        }
        let out = cmd
            .args(args)
            .args(["-o", "json"])
            .output()
            .map_err(|e| format!("kubectl: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "kubectl {}: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        let v: Value = serde_json::from_slice(&out.stdout)
            .map_err(|_| "kubectl output is not JSON".to_string())?;
        Ok(v["items"].as_array().cloned().unwrap_or_default())
    }

    /// Namespaces, PVCs and running pods.
    fn lists(&self) -> Result<Lists, String> {
        match self {
            Source::Kube(k) => Ok((
                k.list("/api/v1/namespaces").map_err(|e| e.to_string())?,
                k.list("/api/v1/persistentvolumeclaims")
                    .map_err(|e| e.to_string())?,
                k.list("/api/v1/pods?fieldSelector=status.phase%3DRunning")
                    .map_err(|e| e.to_string())?,
            )),
            Source::Kubectl(_) => Ok((
                self.kubectl(&["get", "namespaces"])?,
                self.kubectl(&["get", "persistentvolumeclaims", "--all-namespaces"])?,
                self.kubectl(&[
                    "get",
                    "pods",
                    "--all-namespaces",
                    "--field-selector=status.phase=Running",
                ])?,
            )),
        }
    }
}

/// Take one snapshot and write it. Returns the file and a summary line.
pub fn take(src: &Source, s: &Settings) -> Result<(PathBuf, String), String> {
    let (nss, pvcs, pods) = src.lists()?;
    let (rows, pvc_rows) = snapshot::collect(&nss, &pvcs, &pods, &s.labels);
    let t = now();
    let doc = snapshot::document(t, s.interval, &s.labels, rows.clone(), pvc_rows.clone());
    let dir = s.out_dir.join(fmt_day(t));
    fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let stamp = fmt_iso(t);
    let path = dir.join(format!("{}.json", stamp[11..19].replace(':', "")));
    let tmp = dir.join(format!(".{}.tmp", stamp[11..19].replace(':', "")));
    let write = || -> std::io::Result<()> {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(doc.to_string().as_bytes())?;
        f.sync_all()?;
        fs::rename(&tmp, &path)
    };
    write().map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    let sum = |rows: &[Value], k: &str| {
        rows.iter()
            .map(|r| r[k].as_f64().unwrap_or(0.0))
            .sum::<f64>()
    };
    let with_dept = rows
        .iter()
        .filter(|r| r["dept"].as_str().is_some_and(|d| !d.is_empty()))
        .count();
    let line = format!(
        "snapshot {}: {} pods ({} with a department), {:.3} cores / {:.2} GiB declared, {} PVCs {:.2} GiB",
        path.file_name().and_then(|n| n.to_str()).unwrap_or(""),
        rows.len(),
        with_dept,
        sum(&rows, "cpu"),
        sum(&rows, "mem") / GIB,
        pvc_rows.len(),
        sum(&pvc_rows, "bytes") / GIB,
    );
    Ok((path, line))
}

/// Remove day directories older than the retention.
pub fn prune(s: &Settings) {
    let cutoff = fmt_day(now() - f64::from(s.retention_days) * 86_400.0);
    let Ok(rd) = fs::read_dir(&s.out_dir) else {
        return;
    };
    for e in rd.filter_map(Result::ok) {
        let name = e.file_name().to_string_lossy().to_string();
        if cost_report_core::time::parse_day(&name).is_some() && name < cutoff && e.path().is_dir()
        {
            match fs::remove_dir_all(e.path()) {
                Ok(()) => eprintln!("{NAME} snapshotter: pruned {name}"),
                Err(err) => eprintln!("{NAME} snapshotter: cannot prune {name}: {err}"),
            }
        }
    }
}

/// Snapshots with `ts` in `[start, end)`. Unreadable files are skipped:
/// one bad file must not hide a whole range.
pub fn load(dir: &Path, start: i64, end: i64) -> Vec<Value> {
    let (first, last) = (fmt_day(start as f64), fmt_day(end as f64));
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(dir) else {
        return out;
    };
    let mut days: Vec<String> = rd
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|d| cost_report_core::time::parse_day(d).is_some() && *d >= first && *d <= last)
        .collect();
    days.sort();
    for d in days {
        let Ok(files) = fs::read_dir(dir.join(&d)) else {
            continue;
        };
        for f in files.filter_map(Result::ok) {
            let p = f.path();
            if p.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            if let Some(doc) = fs::read_to_string(&p)
                .ok()
                .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            {
                out.push(doc);
            }
        }
    }
    out
}

/// The read-only query API. It reads files already written and never
/// calls the API server, so a flood of queries only slows itself.
/// Is the snapshot loop still writing? The heartbeat file is touched by
/// the loop, not by this HTTP thread, so a stuck loop fails this check even
/// though the thread answering it is fine. Before the first snapshot the
/// process gets the same grace from its start.
pub fn healthy(dir: &Path, interval: u64, started: f64, at: f64) -> bool {
    let limit = 3.0 * interval as f64;
    let beat = fs::metadata(dir.join(".heartbeat"))
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64());
    match beat {
        Some(b) => at - b < limit,
        None => at - started < limit,
    }
}

/// `/healthz` (the liveness probe, see [`healthy`]) and `/allocation`.
pub fn handle(dir: &Path, interval: u64, started: f64, req: &Request) -> Response {
    match req.path.as_str() {
        "/healthz" if healthy(dir, interval, started, now()) => Response::text(200, "ok\n"),
        "/healthz" => Response::text(503, "no snapshot written for three intervals\n"),
        "/readyz" => Response::text(200, "ok\n"),
        "/allocation" => {
            let parse = |k: &str| req.param(k).and_then(|v| parse_iso(v).ok());
            let (Some(start), Some(end)) = (parse("start"), parse("end")) else {
                return Response::json(
                    400,
                    &json!({ "error": "start and end must be RFC 3339 UTC, like 2026-09-24T05:00:00Z" }),
                );
            };
            if start >= end {
                return Response::json(400, &json!({ "error": "start must be before end" }));
            }
            if end - start > 40 * 86_400 {
                return Response::json(400, &json!({ "error": "at most 40 days per query" }));
            }
            match snapshot::allocation(&load(dir, start, end), start, end) {
                Some(doc) => Response::json(200, &doc),
                // 404 is not an error: "no snapshots for this range". A 200
                // with zeros would be read as "this time cost nothing".
                None => Response::json(
                    404,
                    &json!({ "error": format!(
                    "no snapshots for {} ~ {}", fmt_iso(start as f64), fmt_iso(end as f64)) }),
                ),
            }
        }
        _ => Response::json(404, &json!({ "error": "not found" })),
    }
}

pub fn run(src: Source, s: Settings, stop: Arc<AtomicBool>) {
    eprintln!(
        "{NAME} snapshotter: every {}s, department label {}, writing {}, keeping {} days",
        s.interval,
        s.labels.department,
        s.out_dir.display(),
        s.retention_days
    );
    let mut last_prune = String::new();
    while !stop.load(Ordering::Relaxed) {
        let started = now();
        match take(&src, &s) {
            Ok((_, line)) => {
                eprintln!("{NAME} snapshotter: {line}");
                let _ = fs::write(s.out_dir.join(".heartbeat"), fmt_iso(now()));
                let today = fmt_day(now());
                if today != last_prune {
                    prune(&s);
                    last_prune = today;
                }
            }
            // A failed snapshot must not end the loop.
            Err(e) => eprintln!("{NAME} snapshotter: error: {e}"),
        }
        let spent = (now() - started).max(0.0) as u64;
        sleep_unless(&stop, s.interval.saturating_sub(spent).max(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heartbeat_decides_health() {
        let dir = std::env::temp_dir().join(format!("cost-report-hb-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let t = now();
        // No heartbeat yet: healthy during the grace period only.
        assert!(healthy(&dir, 300, t, t + 100.0));
        assert!(!healthy(&dir, 300, t - 2000.0, t));
        fs::write(dir.join(".heartbeat"), "x").unwrap();
        assert!(healthy(&dir, 300, t - 2000.0, now()));
        assert!(!healthy(&dir, 300, t - 2000.0, now() + 901.0));
        fs::remove_dir_all(&dir).unwrap();
    }
}
