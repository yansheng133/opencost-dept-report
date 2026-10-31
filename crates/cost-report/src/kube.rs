//! Calls to the Kubernetes API server with the pod's own ServiceAccount.
//!
//! * the server reads `kube-system/extension-apiserver-authentication` at
//!   startup (built-in Role `extension-apiserver-authentication-reader`)
//!   and creates SubjectAccessReviews (ClusterRole `system:auth-delegator`);
//! * the snapshotter lists namespaces, PVCs and running pods (its own
//!   read-only ClusterRole).
//!
//! The token is read again for every call: projected ServiceAccount tokens
//! rotate about hourly, and a token cached at startup starts failing with
//! 401 an hour later — for the snapshotter, exactly when Prometheus is
//! down and it is needed most.

use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::CertificateDer;
use rustls::{ClientConfig, RootCertStore};
use serde_json::Value;

use crate::identity::AuthConfig;

pub const SA_DIR: &str = "/var/run/secrets/kubernetes.io/serviceaccount";
const AUTH_CONFIGMAP: &str =
    "/api/v1/namespaces/kube-system/configmaps/extension-apiserver-authentication";
/// Longest response read; a cluster-wide pod list can be large.
const MAX_RESPONSE_BYTES: u64 = 256 * 1024 * 1024;

pub struct Kube {
    base: String,
    agent: ureq::Agent,
    token_file: PathBuf,
}

#[derive(Debug)]
pub enum KubeError {
    Status(u16, String),
    Other(String),
}

impl std::fmt::Display for KubeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KubeError::Status(code, msg) => write!(f, "HTTP {code}: {msg}"),
            KubeError::Other(m) => f.write_str(m),
        }
    }
}

impl Kube {
    /// From inside the pod.
    pub fn in_cluster() -> Result<Self, String> {
        let host = std::env::var("KUBERNETES_SERVICE_HOST")
            .map_err(|_| "not running in Kubernetes (KUBERNETES_SERVICE_HOST unset)".to_string())?;
        let port = std::env::var("KUBERNETES_SERVICE_PORT").unwrap_or_else(|_| "443".into());
        let host = if host.contains(':') {
            format!("[{host}]")
        } else {
            host
        };
        let ca = std::fs::read(format!("{SA_DIR}/ca.crt"))
            .map_err(|_| "no cluster CA in the pod".to_string())?;
        let mut roots = RootCertStore::empty();
        for c in CertificateDer::pem_slice_iter(&ca) {
            roots
                .add(c.map_err(|_| "cluster CA is not valid PEM".to_string())?)
                .map_err(|_| "cluster CA is not a valid certificate".to_string())?;
        }
        let tls =
            ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .map_err(|e| e.to_string())?
                .with_root_certificates(roots)
                .with_no_client_auth();
        let agent = ureq::AgentBuilder::new()
            .tls_config(Arc::new(tls))
            .timeout(Duration::from_secs(30))
            .build();
        Ok(Self {
            base: format!("https://{host}:{port}"),
            agent,
            token_file: PathBuf::from(format!("{SA_DIR}/token")),
        })
    }

    /// Against `base` (a test server), with the token in `token_file`.
    #[cfg(test)]
    pub fn with_base(base: &str, token_file: PathBuf) -> Self {
        Self {
            base: base.trim_end_matches('/').to_string(),
            agent: ureq::AgentBuilder::new()
                .timeout(Duration::from_secs(10))
                .build(),
            token_file,
        }
    }

    fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value, KubeError> {
        let token = std::fs::read_to_string(&self.token_file)
            .map_err(|_| KubeError::Other("no ServiceAccount token in the pod".into()))?;
        let req = self
            .agent
            .request(method, &format!("{}{path}", self.base))
            .set("Authorization", &format!("Bearer {}", token.trim()))
            .set("Accept", "application/json");
        let result = match body {
            Some(b) => req
                .set("Content-Type", "application/json")
                .send_string(&b.to_string()),
            None => req.call(),
        };
        let read = |resp: ureq::Response| -> Result<Value, KubeError> {
            let mut text = String::new();
            resp.into_reader()
                .take(MAX_RESPONSE_BYTES)
                .read_to_string(&mut text)
                .map_err(|_| KubeError::Other("cannot read the API server's answer".into()))?;
            serde_json::from_str(&text)
                .map_err(|_| KubeError::Other("the API server's answer is not JSON".into()))
        };
        match result {
            Ok(resp) => read(resp),
            Err(ureq::Error::Status(code, resp)) => {
                let msg = read(resp)
                    .ok()
                    .and_then(|v| v["message"].as_str().map(str::to_string))
                    .unwrap_or_default();
                Err(KubeError::Status(code, msg))
            }
            Err(ureq::Error::Transport(t)) => Err(KubeError::Other(format!(
                "cannot reach the Kubernetes API: {t}"
            ))),
        }
    }

    pub fn get(&self, path: &str) -> Result<Value, KubeError> {
        self.call("GET", path, None)
    }

    pub fn post(&self, path: &str, body: &Value) -> Result<Value, KubeError> {
        self.call("POST", path, Some(body))
    }

    /// Every item of a list, following `continue` tokens.
    pub fn list(&self, path: &str) -> Result<Vec<Value>, KubeError> {
        let mut items = Vec::new();
        let mut cont: Option<String> = None;
        loop {
            let sep = if path.contains('?') { '&' } else { '?' };
            let page = match &cont {
                Some(c) => format!("{path}{sep}limit=500&continue={}", urlencode(c)),
                None => format!("{path}{sep}limit=500"),
            };
            let v = self.get(&page)?;
            items.extend(v["items"].as_array().cloned().unwrap_or_default());
            match v
                .pointer("/metadata/continue")
                .and_then(Value::as_str)
                .filter(|c| !c.is_empty())
            {
                Some(c) => cont = Some(c.to_string()),
                None => return Ok(items),
            }
        }
    }

    pub fn load_auth_config(&self) -> Result<AuthConfig, String> {
        let v = self.get(AUTH_CONFIGMAP).map_err(|e| e.to_string())?;
        AuthConfig::from_configmap_data(&v["data"])
    }
}

pub fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}
