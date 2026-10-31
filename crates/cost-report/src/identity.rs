//! Who is calling: the user the Kubernetes API server verified.
//!
//! The backend is a Kubernetes aggregated API (`APIService`
//! [`GROUP`]/[`VERSION`]). The Rancher UI calls it through Rancher's
//! `/k8s/clusters/<id>` proxy, so the request reaches this cluster's API
//! server as the signed-in Rancher user. The API server authenticates the
//! user, checks RBAC on the resource, then forwards the request over
//! mutual TLS with the user in request headers (`X-Remote-User`,
//! `X-Remote-Group`, `X-Remote-Extra-*`).
//!
//! The headers are trusted only on a TLS connection whose client
//! certificate chains to the request-header CA and, when the API server
//! lists allowed names, has one of them as its common name. Both come from
//! the `kube-system/extension-apiserver-authentication` ConfigMap. Another
//! pod reaching the Service directly cannot present such a certificate,
//! so cannot claim to be someone.
//!
//! Docs: https://kubernetes.io/docs/tasks/extend-kubernetes/configure-aggregation-layer/

use std::collections::BTreeMap;

use serde_json::{json, Value};

/// API group of the aggregated API.
pub const GROUP: &str = "api.costreport.yansheng133.github.io";
pub const VERSION: &str = "v1alpha1";

/// Longest user, group or extra value accepted from the headers.
const MAX_NAME_BYTES: usize = 256;
/// At most this many groups (and extra values) are kept.
const MAX_GROUPS: usize = 64;

/// What the API server told us about the request-header proxy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthConfig {
    /// PEM of the CA that signs the API server's proxy client certificate.
    pub client_ca_pem: String,
    /// Allowed common names of that certificate; empty means any.
    pub allowed_names: Vec<String>,
    /// Header names carrying the user, lower case, in order of preference.
    pub username_headers: Vec<String>,
    /// Header names carrying groups, lower case.
    pub group_headers: Vec<String>,
    /// Header name prefixes of user extras, lower case.
    pub extra_prefixes: Vec<String>,
    /// Header names carrying the user's UID, lower case.
    pub uid_headers: Vec<String>,
}

fn json_list(data: &Value, key: &str) -> Result<Vec<String>, String> {
    match data.get(key).and_then(Value::as_str) {
        None | Some("") => Ok(Vec::new()),
        Some(s) => serde_json::from_str::<Vec<String>>(s)
            .map_err(|_| format!("{key} is not a JSON list of strings")),
    }
}

fn clean(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty() && s.len() <= MAX_NAME_BYTES && !s.chars().any(char::is_control))
        .then(|| s.to_string())
}

impl AuthConfig {
    /// From the ConfigMap's `data`.
    pub fn from_configmap_data(data: &Value) -> Result<Self, String> {
        let client_ca_pem = data
            .get("requestheader-client-ca-file")
            .and_then(Value::as_str)
            .filter(|s| s.contains("BEGIN CERTIFICATE"))
            .ok_or("requestheader-client-ca-file is missing")?
            .to_string();
        let lower = |v: Vec<String>| -> Vec<String> {
            v.into_iter()
                .map(|h| h.trim().to_ascii_lowercase())
                .filter(|h| !h.is_empty())
                .collect()
        };
        let username_headers = lower(json_list(data, "requestheader-username-headers")?);
        if username_headers.is_empty() {
            return Err("requestheader-username-headers is empty".into());
        }
        Ok(Self {
            client_ca_pem,
            allowed_names: json_list(data, "requestheader-allowed-names")?,
            username_headers,
            group_headers: lower(json_list(data, "requestheader-group-headers")?),
            extra_prefixes: lower(json_list(data, "requestheader-extra-headers-prefix")?),
            uid_headers: lower(json_list(data, "requestheader-uid-headers")?),
        })
    }

    /// Is a proxy client certificate with this common name allowed?
    pub fn name_allowed(&self, common_name: Option<&str>) -> bool {
        self.allowed_names.is_empty()
            || common_name.is_some_and(|cn| self.allowed_names.iter().any(|n| n == cn))
    }

    /// The caller from the request headers (`(lower-case name, value)`).
    /// `None` when no user header is present (for example the API
    /// server's own availability check).
    pub fn caller(&self, headers: &[(String, String)]) -> Option<Caller> {
        let user = self.username_headers.iter().find_map(|name| {
            headers
                .iter()
                .find(|(n, v)| n == name && !v.trim().is_empty())
                .map(|(_, v)| v.trim().to_string())
        })?;
        let user = clean(&user)?;
        let groups = headers
            .iter()
            .filter(|(n, _)| self.group_headers.contains(n))
            .filter_map(|(_, v)| clean(v))
            .take(MAX_GROUPS)
            .collect();
        let mut extra: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut count = 0;
        for (n, v) in headers {
            let Some(key) = self
                .extra_prefixes
                .iter()
                .find_map(|p| n.strip_prefix(p.as_str()))
            else {
                continue;
            };
            // Extra keys are percent-encoded in header names.
            let (Some(key), Some(v)) = (crate::http::percent_decode(key), clean(v)) else {
                continue;
            };
            if key.is_empty() || count >= MAX_GROUPS {
                continue;
            }
            count += 1;
            extra.entry(key.to_ascii_lowercase()).or_default().push(v);
        }
        let uid = self.uid_headers.iter().find_map(|name| {
            headers
                .iter()
                .find(|(n, _)| n == name)
                .and_then(|(_, v)| clean(v))
        });
        Some(Caller {
            user,
            groups,
            extra,
            uid,
        })
    }
}

/// A verified caller.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Caller {
    /// Kubernetes user name; for Rancher users the Rancher user ID.
    pub user: String,
    pub groups: Vec<String>,
    pub extra: BTreeMap<String, Vec<String>>,
    pub uid: Option<String>,
}

impl Caller {
    pub fn named(user: &str) -> Self {
        Self {
            user: user.into(),
            groups: Vec::new(),
            extra: BTreeMap::new(),
            uid: None,
        }
    }
}

/// Resources of the API, as RBAC rules name them.
pub const RESOURCES: [(&str, &[&str]); 6] = [
    ("config", &["list"]),
    ("reports", &["list"]),
    ("series", &["list"]),
    ("seals", &["get", "list"]),
    ("periods", &["list"]),
    ("departments", &["get", "list"]),
];

/// `GET /apis/<group>/<version>`: discovery, also used by the API server
/// to decide the APIService is available.
pub fn discovery_json() -> Value {
    json!({
        "kind": "APIResourceList",
        "apiVersion": "v1",
        "groupVersion": format!("{GROUP}/{VERSION}"),
        "resources": RESOURCES.iter().map(|(name, verbs)| json!({
            "name": name, "singularName": "", "namespaced": false, "kind": "CostReport", "verbs": verbs,
        })).collect::<Vec<_>>(),
    })
}

/// `GET /apis/<group>`.
pub fn group_json() -> Value {
    let gv = json!({ "groupVersion": format!("{GROUP}/{VERSION}"), "version": VERSION });
    json!({ "kind": "APIGroup", "apiVersion": "v1", "name": GROUP, "versions": [gv], "preferredVersion": gv })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cm() -> Value {
        json!({
            "requestheader-client-ca-file": "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n",
            "requestheader-allowed-names": "[\"system:auth-proxy\"]",
            "requestheader-username-headers": "[\"X-Remote-User\"]",
            "requestheader-group-headers": "[\"X-Remote-Group\"]",
            "requestheader-extra-headers-prefix": "[\"X-Remote-Extra-\"]",
        })
    }

    fn h(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(n, v)| (n.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn reads_the_configmap() {
        let c = AuthConfig::from_configmap_data(&cm()).unwrap();
        assert_eq!(c.allowed_names, ["system:auth-proxy"]);
        assert_eq!(c.username_headers, ["x-remote-user"]);
        assert_eq!(c.extra_prefixes, ["x-remote-extra-"]);
        assert!(c.name_allowed(Some("system:auth-proxy")));
        assert!(!c.name_allowed(Some("someone")));
        assert!(!c.name_allowed(None));
        let mut no_ca = cm();
        no_ca["requestheader-client-ca-file"] = json!("");
        assert!(AuthConfig::from_configmap_data(&no_ca).is_err());
        let mut bad = cm();
        bad["requestheader-allowed-names"] = json!("system:auth-proxy");
        assert!(AuthConfig::from_configmap_data(&bad).is_err());
    }

    #[test]
    fn caller_from_headers() {
        let c = AuthConfig::from_configmap_data(&cm()).unwrap();
        let caller = c
            .caller(&h(&[
                ("x-remote-user", "u-abc12"),
                ("x-remote-group", "system:authenticated"),
                ("x-remote-group", "g2"),
                ("x-remote-extra-principalid", "local://u-abc12"),
                ("x-remote-extra-user%2fname", "sam"),
            ]))
            .unwrap();
        assert_eq!(caller.user, "u-abc12");
        assert_eq!(caller.groups, ["system:authenticated", "g2"]);
        assert_eq!(caller.extra["principalid"], ["local://u-abc12"]);
        assert_eq!(caller.extra["user/name"], ["sam"]);
        assert_eq!(c.caller(&h(&[("x-remote-group", "g")])), None);
        assert_eq!(c.caller(&h(&[("x-remote-user", "  ")])), None);
        assert_eq!(c.caller(&h(&[("x-remote-user", &"a".repeat(300))])), None);
        let d = discovery_json();
        assert_eq!(d["groupVersion"], format!("{GROUP}/{VERSION}"));
        assert_eq!(d["resources"].as_array().unwrap().len(), 6);
    }
}
