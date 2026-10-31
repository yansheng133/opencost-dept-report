//! Actual disk use of local-path PVCs, measured on the node.
//!
//! OpenCost bills PVCs by **claimed** capacity; to compare with actual use
//! you need the use. The usual source, the kubelet's
//! `kubelet_volume_stats_used_bytes`, covers CSI volumes only. local-path
//! volumes are hostPath directories and the kubelet does not measure them
//! at all — the query does not fail, it is just empty.
//!
//! This walks each PVC directory under the local-path root and adds up the
//! blocks in use, like `du`. Read-only. With Longhorn or another CSI
//! driver the kubelet has the number and this is not needed.

use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

/// local-path's default directory name: `<pv>_<namespace>_<pvc>`.
/// Namespaces and PVC names cannot contain `_`, so splitting on it is
/// safe. A changed `pathPattern` gives other names; those are counted as
/// unparsed.
pub fn parse_dir(name: &str) -> Option<(&str, &str, &str)> {
    let mut parts = name.splitn(3, '_');
    let (pv, ns, pvc) = (parts.next()?, parts.next()?, parts.next()?);
    let uuid = pv.strip_prefix("pvc-")?;
    let uuid_ok = uuid.len() == 36
        && uuid.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_digit() || (b'a'..=b'f').contains(&b)
            }
        });
    let dns = |s: &str, dots: bool| {
        !s.is_empty()
            && s.bytes().all(|b| {
                b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || (dots && b == b'.')
            })
            && s.bytes().next().is_some_and(|b| b.is_ascii_alphanumeric())
            && s.bytes().last().is_some_and(|b| b.is_ascii_alphanumeric())
    };
    (uuid_ok && dns(ns, false) && dns(pvc, true)).then_some((pv, ns, pvc))
}

/// (bytes, files, unreadable entries) under `path`.
///
/// Counts allocated blocks (`st_blocks`), not file length: a sparse file
/// can be far longer than the space it takes. Hard links count once;
/// symlinks are not followed; other file systems mounted inside are
/// skipped. Unreadable entries are reported: an undercount is silent, and
/// the reader must know the number may be low.
pub fn disk_usage(path: &Path) -> (u64, u64, u64) {
    let Ok(top) = fs::symlink_metadata(path) else {
        return (0, 0, 1);
    };
    let (mut total, mut files, mut errors) = (top.blocks() * 512, 0u64, 0u64);
    let mut seen = HashSet::new();
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = fs::read_dir(&dir) else {
            errors += 1;
            continue;
        };
        for e in rd {
            let Ok(e) = e else {
                errors += 1;
                continue;
            };
            let Ok(md) = fs::symlink_metadata(e.path()) else {
                errors += 1;
                continue;
            };
            if md.dev() != top.dev() {
                continue;
            }
            if !md.is_dir() && md.nlink() > 1 && !seen.insert((md.dev(), md.ino())) {
                continue;
            }
            total += md.blocks() * 512;
            files += 1;
            if md.is_dir() {
                stack.push(e.path());
            }
        }
    }
    (total, files, errors)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Volume {
    pub pv: String,
    pub ns: String,
    pub pvc: String,
    pub used: u64,
    pub files: u64,
    pub errors: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Scan {
    pub volumes: Vec<Volume>,
    pub unparsed: u64,
    pub root_ok: bool,
    pub duration: f64,
    pub at: f64,
}

pub fn scan(root: &Path, now: impl Fn() -> f64) -> Scan {
    let t0 = now();
    let mut names: Vec<String> = Vec::new();
    let root_ok = match fs::read_dir(root) {
        Ok(rd) => {
            names = rd
                .filter_map(Result::ok)
                .filter_map(|e| e.file_name().into_string().ok())
                .collect();
            true
        }
        Err(_) => false,
    };
    names.sort();
    let mut volumes = Vec::new();
    let mut unparsed = 0;
    for name in &names {
        let full = root.join(name);
        match fs::symlink_metadata(&full) {
            Ok(md) if md.is_dir() => {}
            _ => continue,
        }
        let Some((pv, ns, pvc)) = parse_dir(name) else {
            unparsed += 1;
            continue;
        };
        let (used, files, errors) = disk_usage(&full);
        volumes.push(Volume {
            pv: pv.into(),
            ns: ns.into(),
            pvc: pvc.into(),
            used,
            files,
            errors,
        });
    }
    let end = now();
    Scan {
        volumes,
        unparsed,
        root_ok,
        duration: end - t0,
        at: end,
    }
}

fn esc(v: &str) -> String {
    v.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

/// Prometheus text format. The label is `pvc_namespace`, not `namespace`:
/// the scrape adds the pod's own `namespace`, and with `honor_labels` the
/// two would be confused.
pub fn render(scan: Option<&Scan>, node: &str) -> String {
    let mut out = String::new();
    let nl = if node.is_empty() {
        String::new()
    } else {
        format!("node=\"{}\"", esc(node))
    };
    let nb = if nl.is_empty() {
        String::new()
    } else {
        format!("{{{nl}}}")
    };
    let mut metric = |name: &str, help: &str, rows: Vec<String>| {
        out.push_str(&format!("# HELP {name} {help}\n# TYPE {name} gauge\n"));
        for r in rows {
            out.push_str(&r);
            out.push('\n');
        }
    };
    let Some(s) = scan else {
        metric(
            "cost_report_volume_scan_ok",
            "Whether the last scan succeeded (0 until the first one finishes)",
            vec![format!("cost_report_volume_scan_ok{nb} 0")],
        );
        return out;
    };
    let lab = |v: &Volume| {
        let mut parts = vec![
            format!("pv=\"{}\"", esc(&v.pv)),
            format!("pvc_namespace=\"{}\"", esc(&v.ns)),
            format!("pvc=\"{}\"", esc(&v.pvc)),
        ];
        if !nl.is_empty() {
            parts.push(nl.clone());
        }
        format!("{{{}}}", parts.join(","))
    };
    metric(
        "cost_report_pvc_used_bytes",
        "Disk space a PVC directory occupies (bytes, allocated blocks like du)",
        s.volumes
            .iter()
            .map(|v| format!("cost_report_pvc_used_bytes{} {}", lab(v), v.used))
            .collect(),
    );
    metric(
        "cost_report_pvc_scan_errors",
        "Entries that could not be read while measuring this PVC; above 0 the use may be low",
        s.volumes
            .iter()
            .map(|v| format!("cost_report_pvc_scan_errors{} {}", lab(v), v.errors))
            .collect(),
    );
    metric("cost_report_volume_unparsed_dirs", "Directories under the root not recognised as a PVC (above 0 when local-path's pathPattern was changed)",
        vec![format!("cost_report_volume_unparsed_dirs{nb} {}", s.unparsed)]);
    metric(
        "cost_report_volume_scan_duration_seconds",
        "Duration of the last scan",
        vec![format!(
            "cost_report_volume_scan_duration_seconds{nb} {:.3}",
            s.duration
        )],
    );
    metric(
        "cost_report_volume_scan_timestamp_seconds",
        "When the last scan finished (Unix seconds)",
        vec![format!(
            "cost_report_volume_scan_timestamp_seconds{nb} {:.0}",
            s.at
        )],
    );
    metric(
        "cost_report_volume_scan_ok",
        "Whether the last scan succeeded (0 until the first one finishes)",
        vec![format!(
            "cost_report_volume_scan_ok{nb} {}",
            u8::from(s.root_ok)
        )],
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const PV: &str = "pvc-0b8e4c1e-6a3f-4c4e-9a43-2f6c1f7b9d10";

    #[test]
    fn dir_names() {
        assert_eq!(
            parse_dir(&format!("{PV}_rd-apps_data")),
            Some((PV, "rd-apps", "data"))
        );
        assert_eq!(
            parse_dir(&format!("{PV}_ns_data.v1")),
            Some((PV, "ns", "data.v1"))
        );
        assert_eq!(parse_dir(&format!("{PV}_Bad_data")), None);
        assert_eq!(parse_dir(&format!("{PV}_ns")), None);
        assert_eq!(parse_dir("pvc-short_ns_data"), None);
        assert_eq!(parse_dir("lost+found"), None);
    }

    #[test]
    fn scans_and_renders() {
        let root = std::env::temp_dir().join(format!("cost-report-vol-{}", std::process::id()));
        let dir = root.join(format!("{PV}_rd-apps_data"));
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("a"), vec![1u8; 10_000]).unwrap();
        fs::write(dir.join("sub/b"), vec![1u8; 5_000]).unwrap();
        fs::hard_link(dir.join("a"), dir.join("a-link")).unwrap();
        fs::create_dir_all(root.join("not-a-pvc")).unwrap();
        let s = scan(&root, || 1000.0);
        fs::remove_dir_all(&root).unwrap();
        assert!(s.root_ok);
        assert_eq!(s.unparsed, 1);
        assert_eq!(s.volumes.len(), 1);
        let v = &s.volumes[0];
        // The hard link is counted once: a(10000) + b(5000) rounded up to blocks.
        assert!(
            v.used >= 15_000 && v.used < 15_000 + 64 * 1024,
            "used {}",
            v.used
        );
        assert_eq!(v.files, 3);
        let text = render(Some(&s), "node-1");
        assert!(text.contains(&format!("cost_report_pvc_used_bytes{{pv=\"{PV}\",pvc_namespace=\"rd-apps\",pvc=\"data\",node=\"node-1\"}} {}", v.used)));
        assert!(text.contains("cost_report_volume_scan_ok{node=\"node-1\"} 1"));
        assert!(render(None, "").contains("cost_report_volume_scan_ok 0"));
        let missing = scan(Path::new("/nonexistent-cost-report"), || 0.0);
        assert!(!missing.root_ok);
    }
}
