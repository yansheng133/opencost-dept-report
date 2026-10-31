//! Which departments a caller may see, asked of the API server.
//!
//! The API server already checked that the caller may read the resource
//! (`reports`, `seals`, ...). Which departments is a second question, and
//! RBAC answers it too: the backend sends SubjectAccessReviews for
//! `get departments/<id>` on the caller's behalf. A rule without
//! `resourceNames` means every department, and also the cluster-wide
//! numbers; that is checked as `get departments` without a name, which
//! only such a rule allows.
//!
//! So who sees what is managed where the rest of Kubernetes access is
//! managed (ClusterRoles, Rancher role templates), not in this backend.

use std::collections::{BTreeSet, HashMap};
use std::sync::Mutex;

use cost_report_core::scope::Scope;
use serde_json::{json, Value};

use crate::identity::{Caller, GROUP};
use crate::kube::Kube;

/// How long an answer is reused. RBAC changes take effect within this.
const TTL_SECS: f64 = 30.0;
/// Callers remembered at most.
const MAX_ENTRIES: usize = 1024;

pub trait Reviewer: Send + Sync {
    /// May `caller` `get departments/<name>` (`None`: without a name)?
    fn allowed(&self, caller: &Caller, name: Option<&str>) -> Result<bool, String>;
}

/// SubjectAccessReviews through the API server.
pub struct SarReviewer(pub Kube);

impl Reviewer for SarReviewer {
    fn allowed(&self, caller: &Caller, name: Option<&str>) -> Result<bool, String> {
        let mut attrs = json!({ "group": GROUP, "resource": "departments", "verb": "get" });
        if let Some(n) = name {
            attrs["name"] = json!(n);
        }
        let mut spec =
            json!({ "user": caller.user, "groups": caller.groups, "resourceAttributes": attrs });
        if !caller.extra.is_empty() {
            spec["extra"] = json!(caller.extra);
        }
        if let Some(uid) = &caller.uid {
            spec["uid"] = json!(uid);
        }
        let body = json!({ "apiVersion": "authorization.k8s.io/v1", "kind": "SubjectAccessReview", "spec": spec });
        let v: Value = self
            .0
            .post("/apis/authorization.k8s.io/v1/subjectaccessreviews", &body)
            .map_err(|e| format!("SubjectAccessReview failed: {e}"))?;
        Ok(v.pointer("/status/allowed")
            .and_then(Value::as_bool)
            .unwrap_or(false))
    }
}

/// (caller, departments asked about) → (when, answer).
type Cache = HashMap<(Caller, Vec<String>), (f64, Scope)>;

pub struct Authz {
    reviewer: Box<dyn Reviewer>,
    cache: Mutex<Cache>,
}

impl Authz {
    pub fn new(reviewer: Box<dyn Reviewer>) -> Self {
        Self {
            reviewer,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// The caller's scope among `known` departments.
    pub fn scope(
        &self,
        caller: &Caller,
        known: &BTreeSet<String>,
        now: f64,
    ) -> Result<Scope, String> {
        let key = (caller.clone(), known.iter().cloned().collect::<Vec<_>>());
        if let Some((at, s)) = self
            .cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
        {
            if now - at < TTL_SECS {
                return Ok(s.clone());
            }
        }
        let scope = if self.reviewer.allowed(caller, None)? {
            Scope::All
        } else {
            let mut allowed = BTreeSet::new();
            for d in known {
                if self.reviewer.allowed(caller, Some(d))? {
                    allowed.insert(d.clone());
                }
            }
            Scope::Departments(allowed)
        };
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if cache.len() >= MAX_ENTRIES {
            cache.retain(|_, (at, _)| now - *at < TTL_SECS);
            if cache.len() >= MAX_ENTRIES {
                cache.clear();
            }
        }
        cache.insert(key, (now, scope.clone()));
        Ok(scope)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct Fake {
        calls: Arc<AtomicUsize>,
    }

    impl Reviewer for Fake {
        fn allowed(&self, c: &Caller, name: Option<&str>) -> Result<bool, String> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(matches!(
                (c.user.as_str(), name),
                ("finance", _) | ("rd-manager", Some("rd"))
            ))
        }
    }

    #[test]
    fn scopes_and_caches() {
        let calls = Arc::new(AtomicUsize::new(0));
        let a = Authz::new(Box::new(Fake {
            calls: Arc::clone(&calls),
        }));
        let known: BTreeSet<String> = ["rd", "mfg"].iter().map(|s| s.to_string()).collect();
        assert_eq!(
            a.scope(&Caller::named("finance"), &known, 0.0).unwrap(),
            Scope::All
        );
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        let s = a.scope(&Caller::named("rd-manager"), &known, 0.0).unwrap();
        assert_eq!(s, Scope::Departments(["rd".to_string()].into()));
        assert_eq!(calls.load(Ordering::Relaxed), 4);
        // Cached within the TTL, asked again after it.
        a.scope(&Caller::named("rd-manager"), &known, 10.0).unwrap();
        assert_eq!(calls.load(Ordering::Relaxed), 4);
        a.scope(&Caller::named("rd-manager"), &known, 31.0).unwrap();
        assert_eq!(calls.load(Ordering::Relaxed), 7);
        let s = a.scope(&Caller::named("nobody"), &known, 0.0).unwrap();
        assert_eq!(s, Scope::Departments(BTreeSet::new()));
    }
}
