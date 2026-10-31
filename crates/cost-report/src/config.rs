//! The server's settings: one JSON file (rendered by the Helm chart) plus
//! a few environment variables for paths and ports.
//!
//! ```json
//! {
//!   "cluster": "prod-east",
//!   "opencostUrl": "http://opencost.opencost.svc:9003",
//!   "prometheusUrl": "http://prometheus-server.prometheus-system.svc:80",
//!   "snapshotterUrl": "http://cost-report-snapshotter.cost-report.svc:80",
//!   "labels": { "department": "cost-center", "environment": "env", "environments": ["prod", "dev"] },
//!   "departments": [{ "id": "rd", "name": "研發部" }],
//!   "systemNamespaces": ["kube-system", "cattle-"],
//!   "windows": ["1h", "24h", "7d"], "defaultWindow": "24h",
//!   "seriesSteps": { "1h": "5m", "24h": "10m", "7d": "1h" },
//!   "cacheTtlSeconds": 60, "seriesTtlSeconds": 300, "seriesBudget": 300,
//!   "coverageJob": "opencost", "ingestLagMinutes": 5,
//!   "timeouts": { "opencostSeconds": 30, "prometheusSeconds": 5 },
//!   "seals": { "enabled": true, "dir": "/seals", "daysBack": 7 },
//!   "thresholds": { "coverageOk": 99, "coverageMark": 95, "...": 0 }
//! }
//! ```
//!
//! Everything has a default, so an empty object is a valid file.

use std::collections::BTreeMap;
use std::path::PathBuf;

use cost_report_core::billing::Thresholds;
use cost_report_core::report::ReportConfig;
use cost_report_core::time::parse_day;
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct Config {
    pub report: ReportConfig,
    pub opencost_url: String,
    /// Empty: coverage, scrape targets and storage use are not measured.
    pub prometheus_url: String,
    /// Empty: gaps are not filled from snapshots.
    pub snapshotter_url: String,
    pub department_label: String,
    pub environment_label: String,
    pub windows: Vec<String>,
    pub default_window: String,
    pub series_steps: BTreeMap<String, String>,
    pub cache_ttl: u64,
    pub series_ttl: u64,
    pub series_budget: f64,
    pub coverage_job: String,
    pub opencost_timeout: u64,
    pub prometheus_timeout: u64,
    pub seals_enabled: bool,
    pub seal_dir: PathBuf,
    pub seal_days_back: u32,
    /// Minutes before now that OpenCost has not written yet; queries stop
    /// that far back so the newest minutes do not read as unmeasured.
    pub ingest_lag_minutes: u64,
    pub ratecard_file: PathBuf,
}

fn s(v: &Value, ptr: &str, default: &str) -> String {
    v.pointer(ptr)
        .and_then(Value::as_str)
        .unwrap_or(default)
        .trim()
        .to_string()
}

fn u(v: &Value, ptr: &str, default: u64) -> u64 {
    v.pointer(ptr).and_then(Value::as_u64).unwrap_or(default)
}

fn list(v: &Value, ptr: &str, default: &[&str]) -> Vec<String> {
    match v.pointer(ptr).and_then(Value::as_array) {
        Some(a) => a
            .iter()
            .filter_map(Value::as_str)
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect(),
        None => default.iter().map(|x| x.to_string()).collect(),
    }
}

/// A Kubernetes label key or value the OpenCost aggregate can carry.
fn valid_label(l: &str) -> bool {
    !l.is_empty()
        && l.len() <= 253
        && l.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./".contains(&b))
}

/// A window OpenCost accepts and [`cost_report_core::coverage::nominal_minutes`] understands.
fn valid_window(w: &str) -> bool {
    cost_report_core::coverage::nominal_minutes(w).is_some() && !w.contains(',')
}

impl Config {
    pub fn parse(text: &str, ratecard_file: PathBuf) -> Result<Self, String> {
        let v: Value = if text.trim().is_empty() {
            Value::Object(Default::default())
        } else {
            serde_json::from_str(text).map_err(|e| format!("config is not valid JSON: {e}"))?
        };
        let department_label = s(&v, "/labels/department", "cost-center");
        let environment_label = s(&v, "/labels/environment", "env");
        for l in [&department_label, &environment_label] {
            if !valid_label(l) {
                return Err(format!("label key {l:?} is not valid"));
            }
        }
        let mut departments = Vec::new();
        for d in v
            .get("departments")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let id = d.get("id").and_then(Value::as_str).unwrap_or("").trim();
            if !valid_label(id) {
                return Err(format!("department id {id:?} is not a valid label value"));
            }
            let name = d.get("name").and_then(Value::as_str).unwrap_or(id).trim();
            departments.push((
                id.to_string(),
                if name.is_empty() { id } else { name }.to_string(),
            ));
        }
        let windows = list(&v, "/windows", &["1h", "24h", "7d"]);
        if windows.is_empty() || !windows.iter().all(|w| valid_window(w)) {
            return Err(format!("windows {windows:?} must be like 1h, 24h, 7d"));
        }
        let default_window = s(&v, "/defaultWindow", "24h");
        if !windows.contains(&default_window) {
            return Err(format!(
                "defaultWindow {default_window:?} is not in windows"
            ));
        }
        let mut series_steps = BTreeMap::new();
        let steps_v = v
            .get("seriesSteps")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({"1h": "5m", "24h": "10m", "7d": "1h"}));
        for (w, st) in steps_v.as_object().into_iter().flatten() {
            let st = st.as_str().unwrap_or("");
            if cost_report_core::steps::parse_step(st).is_none() {
                return Err(format!("seriesSteps.{w} {st:?} is not a step like 10m"));
            }
            series_steps.insert(w.clone(), st.to_string());
        }
        let d = Thresholds::default();
        let t = |k: &str, def: f64| {
            v.pointer(&format!("/thresholds/{k}"))
                .and_then(Value::as_f64)
                .unwrap_or(def)
        };
        let thresholds = Thresholds {
            coverage_ok: t("coverageOk", d.coverage_ok),
            coverage_mark: t("coverageMark", d.coverage_mark),
            estimated_ok: t("estimatedOk", d.estimated_ok),
            estimated_mark: t("estimatedMark", d.estimated_mark),
            drift_ok: t("driftOk", d.drift_ok),
            drift_mark: t("driftMark", d.drift_mark),
            unallocated_mark: t("unallocatedMark", d.unallocated_mark),
        };
        if thresholds.coverage_mark > thresholds.coverage_ok
            || thresholds.estimated_ok > thresholds.estimated_mark
            || thresholds.drift_ok > thresholds.drift_mark
        {
            return Err("thresholds: each ok/mark pair must be ordered (coverageMark ≤ coverageOk, estimatedOk ≤ estimatedMark, driftOk ≤ driftMark)".into());
        }
        let opencost_url = s(
            &v,
            "/opencostUrl",
            "http://opencost.opencost.svc.cluster.local:9003",
        );
        if !opencost_url.starts_with("http://") && !opencost_url.starts_with("https://") {
            return Err("opencostUrl must start with http:// or https://".into());
        }
        Ok(Self {
            report: ReportConfig {
                cluster: s(&v, "/cluster", ""),
                departments,
                environments: list(&v, "/labels/environments", &["prod", "dev"]),
                system_namespaces: list(
                    &v,
                    "/systemNamespaces",
                    &["kube-system", "kube-public", "kube-node-lease", "cattle-"],
                ),
                thresholds,
            },
            opencost_url,
            prometheus_url: s(&v, "/prometheusUrl", ""),
            snapshotter_url: s(&v, "/snapshotterUrl", ""),
            department_label,
            environment_label,
            windows,
            default_window,
            series_steps,
            cache_ttl: u(&v, "/cacheTtlSeconds", 60).max(10),
            series_ttl: u(&v, "/seriesTtlSeconds", 300),
            series_budget: u(&v, "/seriesBudget", 300) as f64,
            coverage_job: s(&v, "/coverageJob", "opencost"),
            opencost_timeout: u(&v, "/timeouts/opencostSeconds", 30).max(1),
            // Coverage is extra information; a wrong address with the main
            // timeout would slow every refresh by a minute.
            prometheus_timeout: u(&v, "/timeouts/prometheusSeconds", 5).max(1),
            seals_enabled: v
                .pointer("/seals/enabled")
                .and_then(Value::as_bool)
                .unwrap_or(true),
            seal_dir: PathBuf::from(s(&v, "/seals/dir", "/seals")),
            seal_days_back: u(&v, "/seals/daysBack", 7).clamp(1, 60) as u32,
            ingest_lag_minutes: u(&v, "/ingestLagMinutes", 5).min(60),
            ratecard_file,
        })
    }

    /// OpenCost `aggregate` value for the department label.
    pub fn agg_dept(&self) -> String {
        format!("label:{}", self.department_label)
    }

    pub fn agg_dept_env(&self) -> String {
        format!(
            "label:{},label:{}",
            self.department_label, self.environment_label
        )
    }

    /// The OpenCost window for a request: `day=YYYY-MM-DD` or an allowed
    /// `window`. Only whole days or whitelisted windows: nobody can ask for
    /// a query large enough to knock Prometheus over.
    pub fn window_for(&self, window: Option<&str>, day: Option<&str>) -> Result<String, String> {
        if let Some(day) = day.map(str::trim).filter(|d| !d.is_empty()) {
            if parse_day(day).is_none() {
                return Err("day must be YYYY-MM-DD".into());
            }
            return Ok(day_window(day));
        }
        let w = window.unwrap_or(&self.default_window);
        if self.windows.iter().any(|x| x == w) {
            Ok(w.to_string())
        } else {
            Err(format!(
                "window must be one of {:?}, or use day=YYYY-MM-DD",
                self.windows
            ))
        }
    }
}

/// The OpenCost window of one UTC day.
pub fn day_window(day: &str) -> String {
    format!("{day}T00:00:00Z,{day}T23:59:59Z")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_validation() {
        let c = Config::parse("", "/x".into()).unwrap();
        assert_eq!(c.windows, ["1h", "24h", "7d"]);
        assert_eq!(c.agg_dept(), "label:cost-center");
        assert_eq!(c.agg_dept_env(), "label:cost-center,label:env");
        assert_eq!(c.series_steps["24h"], "10m");
        assert_eq!(c.ingest_lag_minutes, 5);
        assert_eq!(c.window_for(None, None).unwrap(), "24h");
        assert_eq!(c.window_for(Some("7d"), None).unwrap(), "7d");
        assert!(c.window_for(Some("30d"), None).is_err());
        assert_eq!(
            c.window_for(Some("30d"), Some("2026-09-24")).unwrap(),
            "2026-09-24T00:00:00Z,2026-09-24T23:59:59Z"
        );
        assert!(c.window_for(None, Some("2026-9-24")).is_err());
        assert!(c.window_for(None, Some("2026-02-30")).is_err());

        let c = Config::parse(r#"{"departments":[{"id":"rd","name":"研發部"},{"id":"it"}],"labels":{"department":"team"}}"#, "/x".into()).unwrap();
        assert_eq!(
            c.report.departments,
            [
                ("rd".to_string(), "研發部".to_string()),
                ("it".to_string(), "it".to_string())
            ]
        );
        assert_eq!(c.agg_dept(), "label:team");

        for bad in [
            r#"{"departments":[{"id":"bad id"}]}"#,
            r#"{"windows":["1h,2h"]}"#,
            r#"{"defaultWindow":"2h"}"#,
            r#"{"seriesSteps":{"24h":"10x"}}"#,
            r#"{"thresholds":{"coverageOk":90,"coverageMark":95}}"#,
            r#"{"opencostUrl":"opencost:9003"}"#,
            r#"{"labels":{"department":"a b"}}"#,
            "not json",
        ] {
            assert!(Config::parse(bad, "/x".into()).is_err(), "{bad}");
        }
    }
}
