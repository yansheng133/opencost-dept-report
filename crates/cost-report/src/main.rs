//! `cost-report`: one binary, four commands.
//!
//! ```text
//! cost-report server         the report as a Kubernetes aggregated API
//! cost-report snapshotter    declared-request snapshots + /allocation
//! cost-report volume-usage   local-path PVC use on a node, as metrics
//! cost-report fallback       allocation from snapshot files (CLI)
//! ```
//!
//! Every long-running command stops cleanly on SIGTERM or SIGINT. As PID 1
//! in a container a process without a handler ignores SIGTERM, and
//! Kubernetes would wait out the grace period and kill it.

use std::net::TcpListener;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use signal_hook::consts::{SIGINT, SIGTERM};
use signal_hook::iterator::Signals;

mod authz;
mod config;
mod fallback;
mod http;
mod identity;
mod kube;
mod server;
mod snapshotter;
#[cfg(test)]
mod tests;
mod tls;
mod upstream;

use server::{git_sha, App, APP_VERSION, NAME};

const HELP: &str = "\
cost-report: department cost allocation from OpenCost

Commands:
  server         serve the report as a Kubernetes aggregated API
  snapshotter    store declared requests and answer /allocation
                 [--once] take one snapshot and exit
                 [--kubectl [KUBECONFIG]] read with kubectl instead of in-cluster
  volume-usage   measure local-path PVC directories, serve /metrics
  fallback       allocation from snapshot files (see `cost-report fallback --help`)
  --version

server environment:
  COST_REPORT_CONFIG          settings JSON (default /etc/cost-report/config.json)
  COST_REPORT_RATECARD        rate card JSON (default /etc/cost-report/ratecard.json)
  COST_REPORT_LISTEN_ADDR     probes and metrics (default 0.0.0.0:8080)
  COST_REPORT_TLS_LISTEN_ADDR aggregated API (default 0.0.0.0:8443)
  COST_REPORT_TLS_CERT_FILE   serving certificate (PEM)
  COST_REPORT_TLS_KEY_FILE    its key (PEM)
  COST_REPORT_DEV_SCOPES      local development only: serve the API on the plain
                              port, trusting X-Remote-User, with scopes like
                              \"finance=*;rd-manager=rd,it\" (never in a cluster)

snapshotter environment: OUT_DIR (/data), INTERVAL (300), DEPT_LABEL (cost-center),
  ENV_LABEL (env), RETENTION_DAYS (35), LISTEN_ADDR (0.0.0.0:8080; empty: no API)

volume-usage environment: VOLUME_ROOT (/volumes), SCAN_INTERVAL (600),
  LISTEN_ADDR (0.0.0.0:9102), NODE_NAME
";

fn env_or(k: &str, d: &str) -> String {
    std::env::var(k)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| d.into())
}

fn stop_on_signal() -> Result<Arc<AtomicBool>, String> {
    let stop = Arc::new(AtomicBool::new(false));
    let mut signals = Signals::new([SIGTERM, SIGINT])
        .map_err(|e| format!("cannot install signal handlers: {e}"))?;
    let s = Arc::clone(&stop);
    std::thread::spawn(move || {
        if let Some(sig) = signals.forever().next() {
            eprintln!("{NAME}: received signal {sig}, shutting down");
            s.store(true, Ordering::Relaxed);
        }
    });
    Ok(stop)
}

fn bind(addr: &str) -> Result<TcpListener, String> {
    TcpListener::bind(addr).map_err(|e| format!("cannot listen on {addr}: {e}"))
}

/// Development scopes: `user=*` sees everything, `user=a,b` those departments.
struct DevReviewer(Vec<(String, Option<Vec<String>>)>);

impl authz::Reviewer for DevReviewer {
    fn allowed(&self, c: &identity::Caller, name: Option<&str>) -> Result<bool, String> {
        Ok(
            match self.0.iter().find(|(u, _)| *u == c.user).map(|(_, d)| d) {
                Some(None) => true,
                Some(Some(depts)) => name.is_some_and(|n| depts.iter().any(|d| d == n)),
                None => false,
            },
        )
    }
}

fn parse_dev_scopes(s: &str) -> DevReviewer {
    DevReviewer(
        s.split(';')
            .filter_map(|p| p.split_once('='))
            .map(|(u, d)| {
                let d = d.trim();
                (
                    u.trim().to_string(),
                    (d != "*").then(|| d.split(',').map(|x| x.trim().to_string()).collect()),
                )
            })
            .collect(),
    )
}

fn server() -> Result<(), String> {
    let cfg_file = PathBuf::from(env_or("COST_REPORT_CONFIG", "/etc/cost-report/config.json"));
    let ratecard = PathBuf::from(env_or(
        "COST_REPORT_RATECARD",
        "/etc/cost-report/ratecard.json",
    ));
    let text = match std::fs::read_to_string(&cfg_file) {
        Ok(t) => t,
        Err(_) if !cfg_file.exists() => String::new(),
        Err(e) => return Err(format!("cannot read {}: {e}", cfg_file.display())),
    };
    let cfg = config::Config::parse(&text, ratecard)
        .map_err(|e| format!("{}: {e}", cfg_file.display()))?;
    let dev = std::env::var("COST_REPORT_DEV_SCOPES")
        .ok()
        .filter(|v| !v.trim().is_empty());
    let in_cluster = std::env::var("KUBERNETES_SERVICE_HOST").is_ok();
    if dev.is_some() && in_cluster {
        return Err("COST_REPORT_DEV_SCOPES must not be set in a cluster: it trusts unauthenticated headers".into());
    }

    let stop = stop_on_signal()?;
    let plain_addr = env_or("COST_REPORT_LISTEN_ADDR", "0.0.0.0:8080");
    let plain = bind(&plain_addr)?;

    let (app, tls_status) = if let Some(scopes) = &dev {
        let app = Arc::new(App::new(
            cfg,
            authz::Authz::new(Box::new(parse_dev_scopes(scopes))),
        ));
        (
            app,
            "DEVELOPMENT MODE: API on the plain port, X-Remote-User trusted without verification"
                .to_string(),
        )
    } else {
        let kube = kube::Kube::in_cluster()?;
        let auth = kube.load_auth_config().map_err(|e| {
            format!("cannot read kube-system/extension-apiserver-authentication: {e}")
        })?;
        let (cert_file, key_file) = (
            env_or("COST_REPORT_TLS_CERT_FILE", ""),
            env_or("COST_REPORT_TLS_KEY_FILE", ""),
        );
        if cert_file.is_empty() || key_file.is_empty() {
            return Err(
                "COST_REPORT_TLS_CERT_FILE and COST_REPORT_TLS_KEY_FILE are required".into(),
            );
        }
        let (cert, key) = match (std::fs::read(&cert_file), std::fs::read(&key_file)) {
            (Ok(c), Ok(k)) => (c, k),
            _ => {
                return Err(
                    "cannot read COST_REPORT_TLS_CERT_FILE or COST_REPORT_TLS_KEY_FILE".into(),
                )
            }
        };
        let tls_cfg = tls::server_config(&cert, &key, &auth)?;
        let tls_addr = env_or("COST_REPORT_TLS_LISTEN_ADDR", "0.0.0.0:8443");
        let listener = bind(&tls_addr)?;
        let names = if auth.allowed_names.is_empty() {
            "any".to_string()
        } else {
            auth.allowed_names.join(",")
        };
        let reviewer = authz::SarReviewer(kube::Kube::in_cluster()?);
        let app = Arc::new(App::new(cfg, authz::Authz::new(Box::new(reviewer))));
        let (a, s) = (Arc::clone(&app), Arc::clone(&stop));
        let auth = Arc::new(auth);
        std::thread::spawn(move || {
            http::accept_loop(listener, s, move |sock| {
                let peer = sock.peer_addr().ok();
                match tls::accept(&tls_cfg, &auth, sock) {
                    Ok(stream) => {
                        let a = Arc::clone(&a);
                        let auth = Arc::clone(&auth);
                        http::serve_one(stream, &move |req| {
                            a.handle_api(req, auth.caller(&req.headers).as_ref())
                        });
                    }
                    Err(e) => eprintln!("{NAME}: TLS from {peer:?} refused: {e}"),
                }
            })
        });
        (
            app,
            format!(
                "aggregated API {}/{} on {tls_addr} (proxy client names: {names})",
                identity::GROUP,
                identity::VERSION
            ),
        )
    };

    if app.cfg.seals_enabled {
        if let Err(e) = server::check_seal_dir(&app.cfg.seal_dir) {
            eprintln!("{NAME}: warning: seals: {e}");
        }
    }
    let card = app.rate_card();
    eprintln!(
        "{NAME} {APP_VERSION} ({}) probes on {plain_addr}",
        git_sha()
    );
    eprintln!("{NAME}: {tls_status}");
    eprintln!(
        "{NAME}: OpenCost {}, Prometheus {}, snapshotter {}, windows {:?}, refresh every {}s",
        app.cfg.opencost_url,
        if app.cfg.prometheus_url.is_empty() {
            "(not set: coverage not measured)"
        } else {
            &app.cfg.prometheus_url
        },
        if app.cfg.snapshotter_url.is_empty() {
            "(not set: gaps not filled)"
        } else {
            &app.cfg.snapshotter_url
        },
        app.cfg.windows,
        app.cfg.cache_ttl
    );
    eprintln!(
        "{NAME}: rate card {}: {}; seals {}",
        app.cfg.ratecard_file.display(),
        card.map(|c| format!("{} version(s)", c.versions.len()))
            .unwrap_or_else(|| "none (prices implied from the data)".into()),
        if app.cfg.seals_enabled {
            format!("on, {}", app.cfg.seal_dir.display())
        } else {
            "off".into()
        }
    );

    for (f, name) in [
        (
            App::refresher as fn(&Arc<App>, Arc<AtomicBool>),
            "refresher",
        ),
        (App::sealer, "sealer"),
    ] {
        let (a, s) = (Arc::clone(&app), Arc::clone(&stop));
        std::thread::Builder::new()
            .name(name.into())
            .spawn(move || f(&a, s))
            .map_err(|e| e.to_string())?;
    }
    let a = Arc::clone(&app);
    let dev_mode = dev.is_some();
    http::accept_loop(plain, Arc::clone(&stop), move |sock| {
        let a = Arc::clone(&a);
        http::serve_one(sock, &move |req| {
            if dev_mode && req.path.starts_with("/apis/") {
                let user = req
                    .headers
                    .iter()
                    .find(|(n, _)| n == "x-remote-user")
                    .map(|(_, v)| identity::Caller::named(v));
                a.handle_api(req, user.as_ref())
            } else {
                a.handle_basic(req)
            }
        });
    });
    eprintln!("{NAME}: stopped");
    Ok(())
}

fn snapshotter_cmd(args: &[String]) -> Result<(), String> {
    let s = snapshotter::Settings::from_env()?;
    let once = args.iter().any(|a| a == "--once");
    let src = match args.iter().position(|a| a == "--kubectl") {
        Some(i) => {
            snapshotter::Source::Kubectl(args.get(i + 1).filter(|v| !v.starts_with('-')).cloned())
        }
        None => snapshotter::Source::Kube(kube::Kube::in_cluster()?),
    };
    if once {
        let (path, line) = snapshotter::take(&src, &s)?;
        eprintln!("{NAME} snapshotter: {line} → {}", path.display());
        return Ok(());
    }
    std::fs::create_dir_all(&s.out_dir)
        .map_err(|e| format!("cannot create {}: {e}", s.out_dir.display()))?;
    let stop = stop_on_signal()?;
    let addr = std::env::var("LISTEN_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into());
    if !addr.trim().is_empty() {
        // The query API is secondary; if it cannot start, snapshots go on.
        match bind(&addr) {
            Ok(l) => {
                let (dir, st, interval, started) = (
                    s.out_dir.clone(),
                    Arc::clone(&stop),
                    s.interval,
                    cost_report_core::time::now(),
                );
                std::thread::spawn(move || {
                    http::accept_loop(l, st, move |sock| {
                        let d = dir.clone();
                        http::serve_one(sock, &move |req| {
                            snapshotter::handle(&d, interval, started, req)
                        });
                    })
                });
                eprintln!("{NAME} snapshotter: query API on {addr} (/allocation, /healthz)");
            }
            Err(e) => eprintln!("{NAME} snapshotter: {e}; snapshots continue"),
        }
    }
    snapshotter::run(src, s, stop);
    Ok(())
}

fn volume_usage_cmd() -> Result<(), String> {
    use cost_report_core::volume;
    let root = PathBuf::from(env_or("VOLUME_ROOT", "/volumes"));
    // Ten minutes: disk use changes slowly, and walking every file competes
    // with the workloads for I/O.
    let interval: u64 = env_or("SCAN_INTERVAL", "600")
        .parse()
        .map_err(|_| "SCAN_INTERVAL must be a number")?;
    let addr = env_or("LISTEN_ADDR", "0.0.0.0:9102");
    let node = std::env::var("NODE_NAME").unwrap_or_default();
    let stop = stop_on_signal()?;
    let listener = bind(&addr)?;
    let last: Arc<Mutex<Option<volume::Scan>>> = Arc::new(Mutex::new(None));
    let (l2, st) = (Arc::clone(&last), Arc::clone(&stop));
    let root2 = root.clone();
    std::thread::spawn(move || {
        while !st.load(Ordering::Relaxed) {
            let s = volume::scan(&root2, cost_report_core::time::now);
            let errors: u64 = s.volumes.iter().map(|v| v.errors).sum();
            eprintln!(
                "{NAME} volume-usage: {} PVCs in {:.1}s{}{}{}",
                s.volumes.len(),
                s.duration,
                if errors > 0 {
                    format!(", {errors} entries unreadable")
                } else {
                    String::new()
                },
                if s.unparsed > 0 {
                    format!(", {} directories not recognised", s.unparsed)
                } else {
                    String::new()
                },
                if s.root_ok { "" } else { ", ROOT UNREADABLE" },
            );
            *l2.lock().unwrap_or_else(|e| e.into_inner()) = Some(s);
            server::sleep_unless(&st, interval.max(30));
        }
    });
    eprintln!(
        "{NAME} volume-usage: measuring {} every {interval}s, :{addr}/metrics",
        root.display()
    );
    http::accept_loop(listener, stop, move |sock| {
        let l = Arc::clone(&last);
        let node = node.clone();
        http::serve_one(sock, &move |req| match req.path.as_str() {
            "/metrics" => {
                let g = l.lock().unwrap_or_else(|e| e.into_inner());
                let mut r = http::Response::text(200, &volume::render(g.as_ref(), &node));
                r.content_type = "text/plain; version=0.0.4; charset=utf-8";
                r
            }
            "/healthz" => http::Response::text(200, "ok\n"),
            "/readyz" if l.lock().map(|g| g.is_some()).unwrap_or(false) => {
                http::Response::text(200, "ok\n")
            }
            "/readyz" => http::Response::text(503, "scanning\n"),
            _ => http::Response::text(404, "not found\n"),
        });
    });
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("--version" | "-V") => {
            println!("{NAME} {APP_VERSION} ({})", git_sha());
            Ok(())
        }
        Some("--help" | "-h") | None => {
            print!("{HELP}");
            Ok(())
        }
        Some("server") => server(),
        Some("snapshotter") => snapshotter_cmd(&args[1..]),
        Some("volume-usage") => volume_usage_cmd(),
        Some("fallback") if args.get(1).is_some_and(|a| a == "--help") => {
            println!("{}", fallback::parse(&[]).err().unwrap_or_default());
            Ok(())
        }
        Some("fallback") => fallback::parse(&args[1..]).and_then(fallback::run),
        Some(other) => Err(format!("unknown command {other:?} (try --help)")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{NAME}: {e}");
            ExitCode::FAILURE
        }
    }
}
