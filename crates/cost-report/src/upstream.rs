//! HTTP clients for OpenCost, Prometheus and the snapshotter.

use std::io::Read;
use std::time::Duration;

use serde_json::Value;

/// Longest answer read. An allocation by controller over seven days can
/// be tens of megabytes on a big cluster.
const MAX_RESPONSE_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub enum UpstreamError {
    Status(u16),
    Transport(String),
    Invalid(String),
}

impl std::fmt::Display for UpstreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UpstreamError::Status(c) => write!(f, "HTTP {c}"),
            UpstreamError::Transport(m) | UpstreamError::Invalid(m) => f.write_str(m),
        }
    }
}

pub struct Upstream {
    pub base: String,
    agent: ureq::Agent,
}

impl Upstream {
    pub fn new(base: &str, timeout: Duration) -> Self {
        Self {
            base: base.trim_end_matches('/').to_string(),
            agent: ureq::AgentBuilder::new().timeout(timeout).build(),
        }
    }

    /// GET `path` with `params` (unencoded), parsed as JSON.
    pub fn get_json(&self, path: &str, params: &[(&str, &str)]) -> Result<Value, UpstreamError> {
        let mut req = self
            .agent
            .get(&format!("{}{path}", self.base))
            .set("Accept", "application/json");
        for (k, v) in params {
            req = req.query(k, v);
        }
        let resp = match req.call() {
            Ok(r) => r,
            Err(ureq::Error::Status(code, _)) => return Err(UpstreamError::Status(code)),
            Err(ureq::Error::Transport(t)) => return Err(UpstreamError::Transport(t.to_string())),
        };
        let mut text = String::new();
        resp.into_reader()
            .take(MAX_RESPONSE_BYTES)
            .read_to_string(&mut text)
            .map_err(|e| UpstreamError::Transport(e.to_string()))?;
        serde_json::from_str(&text)
            .map_err(|_| UpstreamError::Invalid(format!("{path}: answer is not JSON")))
    }
}

/// OpenCost's allocation API.
pub struct OpenCost(pub Upstream);

impl OpenCost {
    /// One accumulated allocation set (`data[0]`).
    pub fn allocation(
        &self,
        window: &str,
        aggregate: &str,
        extra: &[(&str, &str)],
    ) -> Result<Value, UpstreamError> {
        let mut p = vec![
            ("window", window),
            ("aggregate", aggregate),
            ("accumulate", "true"),
        ];
        p.extend_from_slice(extra);
        let body = self.0.get_json("/allocation/compute", &p)?;
        if body.get("code").and_then(Value::as_u64) != Some(200) {
            return Err(UpstreamError::Invalid(format!(
                "OpenCost answered code={} message={}",
                body.get("code").unwrap_or(&Value::Null),
                body.get("message").and_then(Value::as_str).unwrap_or("")
            )));
        }
        Ok(body
            .pointer("/data/0")
            .cloned()
            .filter(Value::is_object)
            .unwrap_or_else(|| serde_json::json!({})))
    }

    /// Unaccumulated segments (`data`).
    pub fn segments(
        &self,
        window: &str,
        aggregate: &str,
        step: &str,
    ) -> Result<Vec<Value>, UpstreamError> {
        let p = [
            ("window", window),
            ("aggregate", aggregate),
            ("accumulate", "false"),
            ("step", step),
        ];
        let body = self.0.get_json("/allocation/compute", &p)?;
        body.get("data")
            .and_then(Value::as_array)
            .cloned()
            .ok_or_else(|| UpstreamError::Invalid("OpenCost returned no segments".into()))
    }

    /// Asset cost (nodes, disks) total and by type, and the minutes the
    /// nodes cover, for reconciliation.
    pub fn assets(&self, window: &str) -> Result<cost_report_core::report::Assets, UpstreamError> {
        use cost_report_core::num::{field, r4};
        let body = self.0.get_json(
            "/assets",
            &[
                ("window", window),
                ("aggregate", "type"),
                ("accumulate", "true"),
            ],
        )?;
        let data = body.get("data").cloned().unwrap_or(Value::Null);
        let items = match data {
            Value::Array(a) => a.into_iter().next().unwrap_or(Value::Null),
            other => other,
        };
        let mut by_type = serde_json::Map::new();
        let mut node_minutes: Option<f64> = None;
        for v in items.as_object().into_iter().flatten().map(|(_, v)| v) {
            let t = v
                .get("type")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .unwrap_or("other");
            let prev = by_type.get(t).and_then(Value::as_f64).unwrap_or(0.0);
            by_type.insert(
                t.into(),
                serde_json::json!(r4(prev + field(v, "totalCost"))),
            );
            if t == "Node" {
                node_minutes = Some(node_minutes.unwrap_or(0.0).max(field(v, "minutes")));
            }
        }
        let total = by_type.values().filter_map(Value::as_f64).sum();
        Ok(cost_report_core::report::Assets {
            total: r4(total),
            by_type,
            minutes: node_minutes,
        })
    }
}

/// Prometheus' HTTP API.
pub struct Prometheus(pub Upstream);

impl Prometheus {
    fn data(&self, path: &str, params: &[(&str, &str)]) -> Result<Value, UpstreamError> {
        let body = self.0.get_json(path, params)?;
        if body.get("status").and_then(Value::as_str) != Some("success") {
            return Err(UpstreamError::Invalid(format!(
                "Prometheus answered {}: {}",
                body.get("status").and_then(Value::as_str).unwrap_or("?"),
                body.get("error").and_then(Value::as_str).unwrap_or("")
            )));
        }
        Ok(body.get("data").cloned().unwrap_or(Value::Null))
    }

    pub fn targets(&self) -> Result<Value, UpstreamError> {
        self.data("/api/v1/targets", &[("state", "active")])
    }

    pub fn query_range(
        &self,
        q: &str,
        start: i64,
        end: i64,
        step: i64,
    ) -> Result<Vec<Value>, UpstreamError> {
        let (s, e, st) = (start.to_string(), end.to_string(), step.max(1).to_string());
        let d = self.data(
            "/api/v1/query_range",
            &[("query", q), ("start", &s), ("end", &e), ("step", &st)],
        )?;
        Ok(d.get("result")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    pub fn query(&self, q: &str, at: Option<f64>) -> Result<Vec<Value>, UpstreamError> {
        let t = at.map(|t| format!("{t:.3}"));
        let mut p = vec![("query", q)];
        if let Some(t) = &t {
            p.push(("time", t.as_str()));
        }
        let d = self.data("/api/v1/query", &p)?;
        Ok(d.get("result")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }
}
