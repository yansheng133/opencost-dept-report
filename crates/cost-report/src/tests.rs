//! End-to-end tests against local stand-ins: OpenCost, Prometheus, the
//! snapshotter and the Kubernetes API server are small HTTP servers here,
//! and the TLS port is exercised with certificates made per test.

use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cost_report_core::scope::Scope;
use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};
use serde_json::{json, Value};

use crate::authz::{Authz, Reviewer, SarReviewer};
use crate::config::Config;
use crate::http::{self, Request};
use crate::identity::{AuthConfig, Caller, GROUP, VERSION};
use crate::server::App;

/// A request the stand-ins saw: method, path with query, body.
type Seen = Arc<Mutex<Vec<(String, String, String)>>>;

/// A local HTTP server answering with `reply(method, path?query, body)`.
fn mock(
    reply: impl Fn(&str, &str, &str) -> (u16, String) + Send + Sync + 'static,
) -> (String, Seen) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let s2 = Arc::clone(&seen);
    std::thread::spawn(move || {
        for sock in l.incoming().flatten() {
            let mut sock = sock;
            sock.set_read_timeout(Some(Duration::from_secs(5))).ok();
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            let head_end = loop {
                let n = sock.read(&mut chunk).unwrap_or(0);
                if n == 0 {
                    break None;
                }
                buf.extend_from_slice(&chunk[..n]);
                if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    break Some(i + 4);
                }
            };
            let Some(head_end) = head_end else { continue };
            let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
            let len: usize = head
                .lines()
                .find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|v| v.trim().parse().unwrap_or(0))
                })
                .unwrap_or(0);
            while buf.len() < head_end + len {
                let n = sock.read(&mut chunk).unwrap_or(0);
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            let body = String::from_utf8_lossy(&buf[head_end..]).to_string();
            let mut first = head.lines().next().unwrap_or("").split(' ');
            let (method, target) = (
                first.next().unwrap_or("").to_string(),
                first.next().unwrap_or("").to_string(),
            );
            s2.lock()
                .unwrap()
                .push((method.clone(), target.clone(), body.clone()));
            let (status, out) = reply(&method, &target, &body);
            let _ = write!(
                sock,
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{out}",
                out.len()
            );
        }
    });
    (format!("http://{addr}"), seen)
}

fn q(target: &str, key: &str) -> Option<String> {
    let query = target.split_once('?')?.1;
    query.split('&').find_map(|p| {
        let (k, v) = p.split_once('=')?;
        (k == key).then(|| http::percent_decode(&v.replace('+', " ")).unwrap_or_default())
    })
}

const START: &str = "2026-09-24T00:00:00Z";
const END: &str = "2026-09-25T00:00:00Z";

fn row(cost: f64) -> Value {
    json!({"start": START, "end": END, "minutes": 1440.0, "cpuCost": cost * 0.8, "ramCost": cost * 0.2,
           "pvCost": 0.0, "totalCost": cost, "cpuCoreHours": cost * 0.8,
           "ramByteHours": cost * 0.8 * 1073741824.0, "pvByteHours": 0.0,
           "cpuCoreRequestAverage": 1.0, "cpuEfficiency": 0.5, "ramByteRequestAverage": 1.0, "ramEfficiency": 0.4})
}

/// OpenCost and Prometheus in one stand-in (different paths).
fn upstream() -> (String, Seen) {
    mock(|_, target, _| {
        let path = target.split('?').next().unwrap_or("");
        let ok = |data: Value| (200, json!({"code": 200, "data": data}).to_string());
        match path {
            "/allocation/compute" => {
                // A day OpenCost fails on (it answers 500 when overloaded).
                if q(target, "window").is_some_and(|w| w.starts_with("2001-01-01")) {
                    return (500, "{}".into());
                }
                let agg = q(target, "aggregate").unwrap_or_default();
                if q(target, "accumulate").as_deref() == Some("false") {
                    return ok(json!([{"rd": row(1.0), "mfg": row(0.5)}, {}, {"rd": row(1.0)}]));
                }
                match agg.as_str() {
                    "label:cost-center" => ok(json!([{"rd": row(30.0), "mfg": row(10.0),
                        "__idle__": {"totalCost": 40.0}, "__unallocated__": {"totalCost": 20.0}}])),
                    "label:cost-center,label:env" => ok(json!([{"rd/prod": {"totalCost": 30.0}, "mfg/dev": {"totalCost": 10.0}}])),
                    "controller,label:cost-center,label:env" => ok(json!([{
                        "deployment:api/rd/prod": {"name": "deployment:api/rd/prod", "properties": {"namespace": "rd-apps"}, "totalCost": 30.0},
                        "deployment:line/mfg/dev": {"name": "deployment:line/mfg/dev", "properties": {"namespace": "mfg-apps"}, "totalCost": 10.0}}])),
                    "namespace" => ok(json!([{"rd-apps": {"totalCost": 30.0}, "mfg-apps": {"totalCost": 10.0}, "kube-system": {"totalCost": 20.0}}])),
                    _ => (400, "{}".into()),
                }
            }
            "/assets" => ok(json!({"node-1": {"type": "Node", "totalCost": 100.0, "minutes": 1440.0}})),
            "/api/v1/targets" => (200, json!({"status": "success", "data": {"activeTargets": [
                {"labels": {"job": "opencost"}, "scrapeInterval": "60s"}]}}).to_string()),
            "/api/v1/query_range" => {
                let query = q(target, "query").unwrap_or_default();
                let start: f64 = q(target, "start").and_then(|s| s.parse().ok()).unwrap_or(0.0);
                let step: f64 = q(target, "step").and_then(|s| s.parse().ok()).unwrap_or(60.0);
                if query.starts_with("up{") {
                    // One hour missing in the middle of the day.
                    let values: Vec<Value> = (0..=1440)
                        .map(|i| json!([start + i as f64 * step, if (600..660).contains(&i) { "0" } else { "1" }]))
                        .collect();
                    return (200, json!({"status": "success", "data": {"result": [{"metric": {"job": "opencost"}, "values": values}]}}).to_string());
                }
                (200, json!({"status": "success", "data": {"result": []}}).to_string())
            }
            "/api/v1/query" => (200, json!({"status": "success", "data": {"result": []}}).to_string()),
            "/allocation" => (200, json!({"snapshotCoveragePct": 100.0,
                "departments": {"rd": {"cpuCoreHours": 1.0, "ramGiBHours": 0.0, "pvGiBHours": 0.0}}}).to_string()),
            _ => (404, "{}".into()),
        }
    })
}

struct Fake;

impl Reviewer for Fake {
    fn allowed(&self, c: &Caller, name: Option<&str>) -> Result<bool, String> {
        Ok(match c.user.as_str() {
            "finance" => true,
            "rd-manager" => name == Some("rd"),
            _ => false,
        })
    }
}

fn app(base: &str, seal_dir: &std::path::Path) -> App {
    let cfg = Config::parse(
        &json!({
            "cluster": "test", "opencostUrl": base, "prometheusUrl": base, "snapshotterUrl": base,
            "departments": [{"id": "mfg", "name": "製造部"}, {"id": "rd", "name": "研發部"}],
            "seals": {"dir": seal_dir.to_string_lossy(), "daysBack": 2},
            "seriesBudget": 100,
        })
        .to_string(),
        "/nonexistent/ratecard.json".into(),
    )
    .unwrap();
    let mut a = App::new(cfg, Authz::new(Box::new(Fake)));
    a.seal_pause = Duration::ZERO;
    a
}

fn get(app: &App, path: &str, user: Option<&str>) -> (u16, Value) {
    let (p, query) = path.split_once('?').unwrap_or((path, ""));
    let req =
        http::parse_head(format!("GET /apis/{GROUP}/{VERSION}/{p}?{query} HTTP/1.1").as_bytes())
            .unwrap();
    let caller = user.map(Caller::named);
    let r = app.handle_api(&req, caller.as_ref());
    (
        r.status,
        serde_json::from_slice(&r.body).unwrap_or(Value::Null),
    )
}

fn temp(tag: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("cost-report-test-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    p
}

#[test]
fn report_end_to_end_and_scoped() {
    let (base, _) = upstream();
    let dir = temp("e2e");
    let a = app(&base, &dir);
    assert!(a.refresh("24h"));

    let (s, full) = get(&a, "reports?window=24h", Some("finance"));
    assert_eq!(s, 200, "{full}");
    assert_eq!(full["scope"]["all"], true);
    assert_eq!(full["depts"].as_array().unwrap().len(), 2);
    assert_eq!(full["depts"][1]["name"], "研發部");
    // The hour without scrapes was found and filled from the snapshotter.
    assert!(
        full["coverage"]["gapMinutes"].as_f64().unwrap() >= 59.0,
        "{}",
        full["coverage"]
    );
    assert_eq!(full["basis"]["fill"]["byDept"]["rd"], 1.0);
    assert_eq!(full["reconciliation"]["assets"], 100.0);
    assert!(full["series"]["points"].as_array().unwrap().len() >= 2);
    assert_eq!(full["gate"]["checks"][0]["code"], "has_data");
    // The window is reported as asked and as resolved (the last 24 h up to now).
    assert_eq!(full["window"]["query"], "24h");
    let req = &full["window"]["requested"];
    let span = cost_report_core::time::parse_iso(req["end"].as_str().unwrap()).unwrap()
        - cost_report_core::time::parse_iso(req["start"].as_str().unwrap()).unwrap();
    assert_eq!(span, 86_400);
    let (s, e) = get(&a, "reports?day=2999-01-01", Some("finance"));
    assert_eq!((s, e["error"]["code"].as_str()), (400, Some("bad_window")));
    // The stand-in's data reaches back further than 7 days: no data start
    // inside the longest window, so nothing is clamped and dataSince is null.
    assert!(a.refresh("7d"));
    let (_, cfg) = get(&a, "config", Some("finance"));
    assert!(cfg["dataSince"].is_null());

    let (s, rd) = get(&a, "reports", Some("rd-manager"));
    assert_eq!(s, 200);
    assert_eq!(rd["scope"], json!({"all": false, "departments": ["rd"]}));
    assert_eq!(rd["depts"].as_array().unwrap().len(), 1);
    assert!(rd["idle"].is_null() && rd["reconciliation"].is_null() && rd["unallocRows"].is_null());
    assert!(rd["workloads"]
        .as_array()
        .unwrap()
        .iter()
        .all(|w| w["dept"] == "rd"));
    assert!(rd["series"]["points"][0]["depts"].get("mfg").is_none());

    let (s, cfg) = get(&a, "config", Some("rd-manager"));
    assert_eq!(s, 200);
    assert_eq!(cfg["departments"], json!([{"id": "rd", "name": "研發部"}]));
    let (s, cfg) = get(&a, "config", Some("nobody"));
    assert_eq!(s, 200);
    assert_eq!(cfg["departments"], json!([]));
    let (s, e) = get(&a, "reports", Some("nobody"));
    assert_eq!(
        (s, e["error"]["code"].as_str()),
        (403, Some("no_department"))
    );
    let (s, e) = get(&a, "reports", None);
    assert_eq!(
        (s, e["error"]["code"].as_str()),
        (401, Some("unauthenticated"))
    );
    let (s, e) = get(&a, "reports?window=30d", Some("finance"));
    assert_eq!((s, e["error"]["code"].as_str()), (400, Some("bad_window")));
    let (s, e) = get(&a, "reports?day=2026-13-01", Some("finance"));
    assert_eq!((s, e["error"]["code"].as_str()), (400, Some("bad_window")));
    // A day built on demand that OpenCost fails is an upstream error, not "not ready".
    let (s, e) = get(&a, "reports?day=2001-01-01", Some("finance"));
    assert_eq!((s, e["error"]["code"].as_str()), (502, Some("upstream")));
    let (s, e) = get(&a, "nothing", Some("finance"));
    assert_eq!((s, e["kind"].as_str()), (404, Some("Status")));
    let (s, d) = get(&a, "", Some("finance"));
    assert_eq!((s, d["kind"].as_str()), (404, Some("Status")));
    let _ = std::fs::remove_dir_all(dir);
}

/// OpenCost whose data began two hours ago: rows start at max(window start, then).
#[test]
fn queries_start_where_data_begins() {
    let began = ((cost_report_core::time::now() as i64 - 7200) / 300) * 300;
    let (base, seen) = mock(move |_, target, _| {
        let path = target.split('?').next().unwrap_or("");
        if path != "/allocation/compute" {
            return (404, "{}".into());
        }
        let w = q(target, "window").unwrap_or_default();
        let (a, b) = w.split_once(',').unwrap_or(("", ""));
        let (a, b) = (
            cost_report_core::time::parse_iso(a).unwrap_or(0),
            cost_report_core::time::parse_iso(b).unwrap_or(0),
        );
        let s = a.max(began);
        let r = json!({"start": cost_report_core::time::fmt_iso(s as f64), "end": cost_report_core::time::fmt_iso(b as f64),
            "minutes": (b - s) as f64 / 60.0, "totalCost": 1.0, "cpuCoreHours": 1.0});
        (200, json!({"code": 200, "data": [{"rd": r}]}).to_string())
    });
    let dir = temp("clamp");
    let a = app(&base, &dir);
    assert!(a.refresh("7d"));
    let (_, cfg) = get(&a, "config", Some("finance"));
    assert_eq!(
        cfg["dataSince"],
        cost_report_core::time::fmt_iso(began as f64)
    );
    seen.lock().unwrap().clear();
    assert!(a.refresh("24h"));
    // Every allocation query of the 24h report starts where the data begins…
    let starts: Vec<String> = seen
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, t, _)| {
            t.starts_with("/allocation/compute") && q(t, "accumulate").as_deref() == Some("true")
        })
        .filter_map(|(_, t, _)| {
            q(t, "window").and_then(|w| w.split_once(',').map(|(a, _)| a.to_string()))
        })
        .collect();
    assert!(!starts.is_empty());
    assert!(
        starts
            .iter()
            .all(|s| *s == cost_report_core::time::fmt_iso(began as f64)),
        "{starts:?}"
    );
    // …while the header still shows the 24 hours asked for.
    let (_, r) = get(&a, "reports?window=24h", Some("finance"));
    let req = &r["window"]["requested"];
    let span = cost_report_core::time::parse_iso(req["end"].as_str().unwrap()).unwrap()
        - cost_report_core::time::parse_iso(req["start"].as_str().unwrap()).unwrap();
    assert_eq!(span, 86_400);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn series_whitelist_and_budget() {
    let (base, seen) = upstream();
    let dir = temp("series");
    let a = app(&base, &dir);
    assert!(a.refresh("24h"));
    let (s, e) = get(&a, "series?window=24h&step=1m", Some("finance"));
    assert_eq!((s, e["error"]["code"].as_str()), (400, Some("bad_step")));
    assert!(e["error"]["options"]
        .as_array()
        .unwrap()
        .contains(&json!("5m")));
    // The default step comes from the report: no query to OpenCost.
    let before = seen.lock().unwrap().len();
    let (s, _) = get(&a, "series?window=24h&step=10m", Some("finance"));
    assert_eq!(s, 200);
    assert_eq!(seen.lock().unwrap().len(), before);
    // 288 segments: allowed alone (first in the period), then over the budget of 100.
    let (s, _) = get(&a, "series?window=24h&step=5m", Some("finance"));
    assert_eq!(s, 200);
    let (s, e) = get(&a, "series?window=24h&step=30m", Some("rd-manager"));
    assert_eq!(
        (s, e["error"]["code"].as_str()),
        (429, Some("series_budget"))
    );
    assert!(e["details"]["retryAfterSeconds"].as_u64().unwrap() >= 1);
    // Cached: the same query again is served without the budget.
    let (s, _) = get(&a, "series?window=24h&step=5m", Some("rd-manager"));
    assert_eq!(s, 200);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn seals_are_written_once_and_scoped() {
    let (base, _) = upstream();
    let dir = temp("seals");
    let a = app(&base, &dir);
    a.seal_completed_days();
    let (s, list) = get(&a, "seals", Some("finance"));
    assert_eq!(s, 200);
    let seals = list["seals"].as_array().unwrap();
    assert_eq!(seals.len(), 2, "{list}");
    // 100 measured + 1 filled from the snapshotter for the gap hour.
    assert!(
        seals
            .iter()
            .all(|r| r["intact"] == true && r["total"] == 101.0),
        "{list}"
    );
    // Sealing again does not rewrite anything.
    let day = seals[0]["day"].as_str().unwrap().to_string();
    let before = std::fs::read_to_string(dir.join(format!("{day}.json"))).unwrap();
    a.seal_completed_days();
    assert_eq!(
        std::fs::read_to_string(dir.join(format!("{day}.json"))).unwrap(),
        before
    );

    let (_, list) = get(&a, "seals", Some("rd-manager"));
    assert_eq!(list["seals"][0]["total"], 31.0);
    assert!(list["seals"][0]["driftPct"].is_null());
    let (s, doc) = get(&a, &format!("seals/{day}"), Some("rd-manager"));
    assert_eq!(s, 200);
    assert_eq!(doc["depts"].as_array().unwrap().len(), 1);
    assert_eq!(doc["totals"], json!({"total": 31.0}));
    let (s, _) = get(&a, "seals/2001-01-01", Some("finance"));
    assert_eq!(s, 404);
    let (s, _) = get(&a, "seals/..%2Fetc", Some("finance"));
    assert_eq!(s, 404);
    let (s, p) = get(&a, "periods", Some("rd-manager"));
    assert_eq!(s, 200);
    assert!(p["months"][0]["idle"].is_null());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn subject_access_reviews_carry_the_caller() {
    let (base, seen) = mock(|_, _, body| {
        let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
        let allowed = v
            .pointer("/spec/resourceAttributes/name")
            .and_then(Value::as_str)
            == Some("rd");
        (201, json!({"status": {"allowed": allowed}}).to_string())
    });
    let token = std::env::temp_dir().join(format!("cost-report-token-{}", std::process::id()));
    std::fs::write(&token, "test-token\n").unwrap();
    let authz = Authz::new(Box::new(SarReviewer(crate::kube::Kube::with_base(
        &base,
        token.clone(),
    ))));
    let mut caller = Caller::named("u-abc12");
    caller.groups = vec!["system:authenticated".into()];
    caller
        .extra
        .insert("principalid".into(), vec!["local://u-abc12".into()]);
    let known: BTreeSet<String> = ["rd", "mfg"].iter().map(|s| s.to_string()).collect();
    let scope = authz.scope(&caller, &known, 0.0).unwrap();
    assert_eq!(scope, Scope::Departments(["rd".to_string()].into()));
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 3);
    let (method, path, body) = &seen[0];
    assert_eq!(method, "POST");
    assert_eq!(path, "/apis/authorization.k8s.io/v1/subjectaccessreviews");
    let v: Value = serde_json::from_str(body).unwrap();
    assert_eq!(v["spec"]["user"], "u-abc12");
    assert_eq!(v["spec"]["groups"], json!(["system:authenticated"]));
    assert_eq!(
        v["spec"]["extra"]["principalid"],
        json!(["local://u-abc12"])
    );
    assert_eq!(
        v["spec"]["resourceAttributes"],
        json!({"group": GROUP, "resource": "departments", "verb": "get"})
    );
    let _ = std::fs::remove_file(token);
}

// ── TLS ──────────────────────────────────────────────────────────────────

struct Ca {
    cert: rcgen::Certificate,
    issuer: Issuer<'static, KeyPair>,
}

fn ca(name: &str) -> Ca {
    let mut p = CertificateParams::new(Vec::<String>::new()).unwrap();
    p.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    p.distinguished_name.push(DnType::CommonName, name);
    let key = KeyPair::generate().unwrap();
    let cert = p.self_signed(&key).unwrap();
    Ca {
        cert,
        issuer: Issuer::new(p, key),
    }
}

fn leaf(ca: &Ca, cn: &str, purpose: ExtendedKeyUsagePurpose) -> (rcgen::Certificate, KeyPair) {
    let mut p = CertificateParams::new(vec!["localhost".to_string()]).unwrap();
    p.distinguished_name.push(DnType::CommonName, cn);
    p.extended_key_usages = vec![purpose];
    let key = KeyPair::generate().unwrap();
    let cert = p.signed_by(&key, &ca.issuer).unwrap();
    (cert, key)
}

fn client(server_ca: &Ca, cert: Option<&(rcgen::Certificate, KeyPair)>) -> Arc<ClientConfig> {
    let mut roots = RootCertStore::empty();
    roots.add(server_ca.cert.der().clone()).unwrap();
    let b = ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots);
    Arc::new(match cert {
        Some((c, k)) => b
            .with_client_auth_cert(
                vec![CertificateDer::from(c.der().to_vec())],
                PrivateKeyDer::try_from(k.serialize_der()).unwrap(),
            )
            .unwrap(),
        None => b.with_no_client_auth(),
    })
}

fn tls_get(
    addr: SocketAddr,
    cfg: Arc<ClientConfig>,
    req: &str,
) -> Result<(u16, String), std::io::Error> {
    let sock = TcpStream::connect(addr)?;
    sock.set_read_timeout(Some(Duration::from_secs(10)))?;
    let conn = ClientConnection::new(cfg, ServerName::try_from("localhost").unwrap()).unwrap();
    let mut tls = StreamOwned::new(conn, sock);
    tls.write_all(req.as_bytes())?;
    let mut out = String::new();
    tls.read_to_string(&mut out)?;
    let (head, body) = out
        .split_once("\r\n\r\n")
        .ok_or_else(|| std::io::Error::other("no response"))?;
    Ok((
        head.split(' ').nth(1).unwrap().parse().unwrap(),
        body.to_string(),
    ))
}

#[test]
fn tls_port_trusts_only_the_proxy_certificate() {
    let (proxy_ca, server_ca, other_ca) = (ca("proxy-ca"), ca("server-ca"), ca("other-ca"));
    let (srv_cert, srv_key) = leaf(
        &server_ca,
        "cost-report.cost-report.svc",
        ExtendedKeyUsagePurpose::ServerAuth,
    );
    let auth = Arc::new(AuthConfig {
        client_ca_pem: proxy_ca.cert.pem(),
        allowed_names: vec!["front-proxy-client".into()],
        username_headers: vec!["x-remote-user".into()],
        group_headers: vec!["x-remote-group".into()],
        extra_prefixes: vec!["x-remote-extra-".into()],
        uid_headers: vec![],
    });
    let cfg = crate::tls::server_config(
        srv_cert.pem().as_bytes(),
        srv_key.serialize_pem().as_bytes(),
        &auth,
    )
    .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let users = Arc::new(Mutex::new(Vec::<Option<String>>::new()));
    let served = Arc::new(AtomicUsize::new(0));
    {
        let (stop, users, served, auth) = (
            Arc::clone(&stop),
            Arc::clone(&users),
            Arc::clone(&served),
            Arc::clone(&auth),
        );
        std::thread::spawn(move || {
            http::accept_loop(listener, stop, move |sock| {
                if let Ok(stream) = crate::tls::accept(&cfg, &auth, sock) {
                    let (users, served, auth) =
                        (Arc::clone(&users), Arc::clone(&served), Arc::clone(&auth));
                    http::serve_one(stream, &move |req: &Request| {
                        served.fetch_add(1, Ordering::Relaxed);
                        users
                            .lock()
                            .unwrap()
                            .push(auth.caller(&req.headers).map(|c| c.user));
                        http::Response::text(200, "ok")
                    });
                }
            })
        });
    }
    let req = "GET /apis/x HTTP/1.1\r\nHost: x\r\nX-Remote-User: u-abc12\r\n\r\n";
    let proxy = leaf(
        &proxy_ca,
        "front-proxy-client",
        ExtendedKeyUsagePurpose::ClientAuth,
    );
    assert_eq!(
        tls_get(addr, client(&server_ca, Some(&proxy)), req)
            .unwrap()
            .0,
        200
    );
    // No client certificate, a certificate from another CA, or the right CA
    // with a name not allowed: refused before any request is read.
    assert!(tls_get(addr, client(&server_ca, None), req).is_err());
    let stranger = leaf(
        &other_ca,
        "front-proxy-client",
        ExtendedKeyUsagePurpose::ClientAuth,
    );
    assert!(tls_get(addr, client(&server_ca, Some(&stranger)), req).is_err());
    let wrong_name = leaf(
        &proxy_ca,
        "someone-else",
        ExtendedKeyUsagePurpose::ClientAuth,
    );
    assert!(tls_get(addr, client(&server_ca, Some(&wrong_name)), req).is_err());
    assert_eq!(served.load(Ordering::Relaxed), 1);
    assert_eq!(*users.lock().unwrap(), vec![Some("u-abc12".to_string())]);
    stop.store(true, Ordering::Relaxed);
}
