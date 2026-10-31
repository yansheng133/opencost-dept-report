//! `cost-report server`: the report as a Kubernetes aggregated API.
//!
//! * A background thread refreshes every configured window from OpenCost
//!   every `cacheTtlSeconds`. When OpenCost fails the previous report is
//!   kept and marked stale, so the page never goes blank.
//! * A second thread seals each finished UTC day once an hour (see
//!   `cost_report_core::seals`). A sealing failure never stops the reports.
//! * Requests are cut to the departments the caller may see
//!   ([`crate::authz`]).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cost_report_core::billing::RateCard;
use cost_report_core::coverage::{self, CoverageInput};
use cost_report_core::num::{f, r4};
use cost_report_core::report::{self, Allocations, Extras, GapAnswer};
use cost_report_core::scope::{self, Scope};
use cost_report_core::seals;
use cost_report_core::series::{self, SeriesInput};
use cost_report_core::steps::{self, Budget};
use cost_report_core::storage::{self, Usage};
use cost_report_core::time::{fmt_day, fmt_iso, now, parse_iso};
use serde_json::{json, Value};

use crate::authz::Authz;
use crate::config::{day_window, Config};
use crate::http::{Request, Response};
use crate::identity::{self, Caller, GROUP, VERSION};
use crate::upstream::{OpenCost, Prometheus, Upstream, UpstreamError};

pub const NAME: &str = env!("CARGO_PKG_NAME");
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Commit the binary was built from (`GIT_SHA` at compile time).
pub fn git_sha() -> &'static str {
    option_env!("GIT_SHA")
        .filter(|s| !s.is_empty())
        .unwrap_or("unknown")
}

#[derive(Debug, Clone, Default)]
struct Entry {
    data: Option<Value>,
    fetched: f64,
    stale: bool,
    error: Option<String>,
}

#[derive(Default)]
struct Metrics {
    refresh_ok: AtomicU64,
    refresh_failed: AtomicU64,
    last_success: AtomicU64,
    seals_sealed: AtomicU64,
    seals_conflict: AtomicU64,
    seals_failed: AtomicU64,
    series_rejected: AtomicU64,
    authz_failed: AtomicU64,
}

pub struct App {
    pub cfg: Config,
    oc: OpenCost,
    prom: Option<Prometheus>,
    snapshotter: Option<Upstream>,
    cache: Mutex<HashMap<String, Entry>>,
    series_cache: Mutex<HashMap<(String, String), (f64, Value)>>,
    /// User-chosen series queries run one at a time (see `steps::Budget`).
    series_lock: Mutex<()>,
    budget: Budget,
    ready: AtomicBool,
    authz: Authz,
    metrics: Metrics,
    /// Pause between sealed days. Each day is five allocation queries plus a
    /// series; run back to back at startup they exhausted OpenCost's
    /// connections to Prometheus and the user's own query failed with them.
    pub(crate) seal_pause: Duration,
}

fn up_err(e: UpstreamError) -> String {
    e.to_string()
}

impl App {
    pub fn new(cfg: Config, authz: Authz) -> Self {
        let oc = OpenCost(Upstream::new(
            &cfg.opencost_url,
            Duration::from_secs(cfg.opencost_timeout),
        ));
        let ptimeout = Duration::from_secs(cfg.prometheus_timeout);
        let prom = (!cfg.prometheus_url.is_empty())
            .then(|| Prometheus(Upstream::new(&cfg.prometheus_url, ptimeout)));
        let snapshotter = (!cfg.snapshotter_url.is_empty())
            .then(|| Upstream::new(&cfg.snapshotter_url, ptimeout));
        let budget = Budget::new(cfg.series_budget, 60.0);
        Self {
            cfg,
            oc,
            prom,
            snapshotter,
            cache: Mutex::new(HashMap::new()),
            series_cache: Mutex::new(HashMap::new()),
            series_lock: Mutex::new(()),
            budget,
            ready: AtomicBool::new(false),
            authz,
            metrics: Metrics::default(),
            seal_pause: Duration::from_secs(20),
        }
    }

    /// "Now" as far as OpenCost's data goes: its newest minutes are written
    /// a few minutes late (measured 3 to 4), and counting them against the
    /// window made "1h" sit on the 95 % coverage line.
    ///
    /// It also writes in 5-minute blocks: at 05:17 the data ended at 05:10,
    /// so the lagged time is aligned down to a 5-minute boundary as well.
    pub fn data_now(&self) -> f64 {
        let t = now() - (self.cfg.ingest_lag_minutes * 60) as f64;
        (t / 300.0).floor() * 300.0
    }

    /// Read on every report so a ConfigMap update applies without a restart.
    pub fn rate_card(&self) -> Option<RateCard> {
        std::fs::read_to_string(&self.cfg.ratecard_file)
            .ok()
            .and_then(|t| RateCard::parse(&t))
    }

    // ── Gathering ────────────────────────────────────────────────────────

    fn coverage(
        &self,
        start: &str,
        end: &str,
        nominal: Option<f64>,
        measured: f64,
    ) -> Option<Value> {
        let prom = self.prom.as_ref()?;
        let (start, end) = (parse_iso(start).ok()?, parse_iso(end).ok()?);
        if end <= start {
            return None;
        }
        let scrape = prom
            .targets()
            .ok()
            .and_then(|t| coverage::scrape_interval(&t, &self.cfg.coverage_job));
        let step = coverage::query_step((end - start) as f64, scrape);
        let q = format!("up{{job=\"{}\"}}", self.cfg.coverage_job);
        match prom.query_range(&q, start, end, step as i64) {
            Ok(series) => coverage::coverage(&CoverageInput {
                start,
                end,
                step: step.floor().max(1.0),
                measured_step: scrape.is_some(),
                series: &series,
                nominal,
                measured_minutes: Some(measured),
            }),
            Err(e) => Some(coverage::unavailable(&e.to_string())),
        }
    }

    fn targets(&self, start: &str, end: &str) -> Option<Vec<Value>> {
        let prom = self.prom.as_ref()?;
        let (start, end) = (parse_iso(start).ok()?, parse_iso(end).ok()?);
        if end <= start {
            return None;
        }
        let step = (60f64).max((end - start) as f64 / 500.0) as i64;
        let tot = prom
            .query_range("count by (job) (up)", start, end, step)
            .ok()?;
        let hea = prom
            .query_range("sum by (job) (up)", start, end, step)
            .ok()?;
        Some(coverage::target_health(&tot, &hea))
    }

    fn fill(&self, gaps: &[Value], rates: &cost_report_core::billing::Rates) -> Option<Value> {
        let snap = self.snapshotter.as_ref()?;
        if gaps.is_empty() {
            return None;
        }
        let answers: Vec<(String, String, GapAnswer)> = gaps
            .iter()
            .map(|g| {
                let (s, e) = (
                    g["start"].as_str().unwrap_or(""),
                    g["end"].as_str().unwrap_or(""),
                );
                let a = match snap.get_json("/allocation", &[("start", s), ("end", e)]) {
                    Ok(doc) => GapAnswer::Found(doc),
                    Err(UpstreamError::Status(404)) => GapAnswer::NoSnapshot,
                    Err(e) => GapAnswer::Unreachable(e.to_string()),
                };
                (s.to_string(), e.to_string(), a)
            })
            .collect();
        report::declared_fill(&answers, rates)
    }

    /// Capacity, claims and measured use of every PV, as of the window's
    /// end. Without the collector installed `installed` is false: the
    /// page must say "not measured", never show empty as zero use.
    fn storage(&self, by_cc: &Value, start: Option<&str>, end: Option<&str>) -> Value {
        let mut status =
            json!({ "installed": false, "lastScan": null, "error": null, "coveredPvs": 0 });
        let (mut usage, mut cap, mut claims) = (BTreeMap::new(), BTreeMap::new(), BTreeMap::new());
        let window = start
            .zip(end)
            .and_then(|(s, e)| Some((parse_iso(s).ok()?, parse_iso(e).ok()?)));
        match (&self.prom, window) {
            (None, _) => status["error"] = json!("prometheusUrl is not set"),
            (Some(_), None) => {}
            (Some(prom), Some((start, end))) => {
                // A window still running ends a few seconds in the future;
                // Prometheus answers nothing for that.
                let at = (end as f64).min(now());
                let mins = (((at - start as f64) / 60.0) as i64).max(1);
                let res: Result<(), UpstreamError> = (|| {
                    for r in prom.query("kube_persistentvolume_capacity_bytes", Some(at))? {
                        if let Some(pv) = r
                            .pointer("/metric/persistentvolume")
                            .and_then(Value::as_str)
                        {
                            cap.insert(pv.to_string(), f(r.pointer("/value/1")));
                        }
                    }
                    for r in prom.query("kube_persistentvolume_claim_ref", Some(at))? {
                        let m = &r["metric"];
                        if let (Some(pv), Some(ns), Some(n)) = (
                            m["persistentvolume"].as_str(),
                            m["claim_namespace"].as_str(),
                            m["name"].as_str(),
                        ) {
                            claims.insert(pv.to_string(), (ns.to_string(), n.to_string()));
                        }
                    }
                    let by = "pv, pvc_namespace, pvc";
                    for (key, q) in [
                        ("used", format!("max by ({by}) (last_over_time(cost_report_pvc_used_bytes[{mins}m]))")),
                        ("peak", format!("max by ({by}) (max_over_time(cost_report_pvc_used_bytes[{mins}m]))")),
                        ("errors", format!("max by ({by}) (max_over_time(cost_report_pvc_scan_errors[{mins}m]))")),
                    ] {
                        for r in prom.query(&q, Some(at))? {
                            let m = &r["metric"];
                            let Some(pv) = m["pv"].as_str() else { continue };
                            let u: &mut Usage = usage.entry(pv.to_string()).or_insert_with(|| Usage {
                                ns: m["pvc_namespace"].as_str().map(str::to_string),
                                pvc: m["pvc"].as_str().map(str::to_string),
                                ..Default::default()
                            });
                            let v = Some(f(r.pointer("/value/1")));
                            match key {
                                "used" => u.used = v,
                                "peak" => u.peak = v,
                                _ => u.errors = v,
                            }
                        }
                    }
                    // "Installed" is asked about now, not the window's end:
                    // a day before the install has no data, of course.
                    if let Some(last) = prom
                        .query("max(cost_report_volume_scan_timestamp_seconds)", None)?
                        .first()
                    {
                        status["installed"] = json!(true);
                        status["lastScan"] = json!(fmt_iso(f(last.pointer("/value/1"))));
                    }
                    Ok(())
                })();
                if let Err(e) = res {
                    status["error"] = json!(e.to_string());
                }
            }
        }
        status["coveredPvs"] = json!(usage.len());
        let mut out = storage::build(by_cc, &usage, &cap, &claims);
        out["collector"] = status;
        out
    }

    /// Usage series for `window` at `step` (default when `None`).
    pub fn usage_series(&self, window: &str, step: Option<&str>) -> Option<Value> {
        // Steps offered and configured follow the window as the user named it;
        // the query is the resolved range (see coverage::resolve_window).
        let (rs, re) = coverage::resolve_window(window, self.data_now())?;
        let q = coverage::window_param(rs, re);
        let span = coverage::nominal_minutes(window);
        let default =
            steps::default_step(span, self.cfg.series_steps.get(window).map(String::as_str));
        let step = step.map(str::to_string).or_else(|| default.clone())?;
        let segments = match self.oc.segments(&q, &self.cfg.agg_dept(), &step) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{NAME}: warning: cannot read the usage series: {e}");
                return None;
            }
        };
        let limit = (re as f64).min(now());
        series::build(&SeriesInput {
            segments: &segments,
            step: &step,
            span,
            default_step: default.as_deref(),
            limit,
        })
    }

    /// The full (unscoped) report for an OpenCost window.
    pub fn build_report(&self, window: &str) -> Result<Value, String> {
        let (rs, re) = coverage::resolve_window(window, self.data_now())
            .ok_or_else(|| format!("{window} has not started yet"))?;
        let user_window = window;
        let q = coverage::window_param(rs, re);
        let window = q.as_str();
        let dept = self.cfg.agg_dept();
        let dept_env = self.cfg.agg_dept_env();
        let a = Allocations {
            by_cc: self
                .oc
                .allocation(
                    window,
                    &dept,
                    &[("includeIdle", "true"), ("shareIdle", "false")],
                )
                .map_err(up_err)?,
            by_cc_shared: self
                .oc
                .allocation(
                    window,
                    &dept,
                    &[("includeIdle", "true"), ("shareIdle", "true")],
                )
                .map_err(up_err)?,
            by_cc_env: self.oc.allocation(window, &dept_env, &[]).map_err(up_err)?,
            by_ctrl: self
                .oc
                .allocation(window, &format!("controller,{dept_env}"), &[])
                .map_err(up_err)?,
            by_ns: self
                .oc
                .allocation(window, "namespace", &[])
                .map_err(up_err)?,
        };
        let (w_start, w_end, minutes) = report::window_of(&a.by_cc);
        let card = self.rate_card();
        let implied = report::implied_rates(&a.by_cc);
        let (rates, _) = report::effective_rates(card.as_ref(), w_start.as_deref(), implied);

        let cov = match (&w_start, &w_end) {
            (Some(s), Some(e)) => self.coverage(s, e, coverage::nominal_minutes(window), minutes),
            _ => None,
        };
        let gaps: Vec<Value> = cov
            .as_ref()
            .and_then(|c| c["gaps"].as_array().cloned())
            .unwrap_or_default();
        let assets = match self.oc.assets(window) {
            Ok(a) => Some(a),
            Err(e) => {
                eprintln!("{NAME}: warning: cannot read asset cost: {e}");
                None
            }
        };
        let x = Extras {
            fill: self.fill(&gaps, &rates),
            coverage: cov,
            assets,
            targets: match (&w_start, &w_end) {
                (Some(s), Some(e)) => self.targets(s, e),
                _ => None,
            },
            series: self.usage_series(user_window, None),
            storage: self.storage(&a.by_cc, w_start.as_deref(), w_end.as_deref()),
        };
        let mut r = report::assemble(&self.cfg.report, &a, user_window, card.as_ref(), &x);
        r["window"]["requested"] =
            json!({ "start": fmt_iso(rs as f64), "end": fmt_iso(re as f64) });
        Ok(r)
    }

    pub fn refresh(&self, window: &str) -> bool {
        match self.build_report(window) {
            Ok(data) => {
                self.cache.lock().unwrap_or_else(|e| e.into_inner()).insert(
                    window.to_string(),
                    Entry {
                        data: Some(data),
                        fetched: now(),
                        stale: false,
                        error: None,
                    },
                );
                self.ready.store(true, Ordering::Relaxed);
                self.metrics.refresh_ok.fetch_add(1, Ordering::Relaxed);
                self.metrics
                    .last_success
                    .store(now() as u64, Ordering::Relaxed);
                true
            }
            Err(e) => {
                let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
                let entry = cache.entry(window.to_string()).or_default();
                entry.stale = true;
                entry.error = Some(e.clone());
                self.metrics.refresh_failed.fetch_add(1, Ordering::Relaxed);
                eprintln!("{NAME}: warning: refreshing {window} failed: {e}");
                false
            }
        }
    }

    // ── Background work ──────────────────────────────────────────────────

    pub fn refresher(self: &Arc<Self>, stop: Arc<AtomicBool>) {
        while !stop.load(Ordering::Relaxed) {
            for w in self.cfg.windows.clone() {
                self.refresh(&w);
            }
            sleep_unless(&stop, self.cfg.cache_ttl);
        }
    }

    /// Seal every finished day of the last `daysBack` that is not sealed.
    /// Only finished days: today is half an invoice.
    pub fn seal_completed_days(&self) {
        let dir = self.cfg.seal_dir.clone();
        let t = now();
        let today = fmt_day(t);
        for i in 1..=self.cfg.seal_days_back {
            let day = fmt_day(t - f64::from(i) * 86_400.0);
            if day >= today || seals::read_seal(&dir, &day).is_some() {
                continue;
            }
            let built = self.build_report(&day_window(&day));
            std::thread::sleep(self.seal_pause);
            let data = match built {
                Ok(d) => d,
                Err(e) => {
                    self.metrics.seals_failed.fetch_add(1, Ordering::Relaxed);
                    eprintln!("{NAME}: warning: sealing {day} failed: {e}");
                    continue;
                }
            };
            // A day without data (before collection started, or the cluster
            // was off) is skipped: a zero invoice looks like "nothing spent".
            if data["window"]["start"].is_null() || f(data.get("total")) == 0.0 {
                eprintln!("{NAME}: seal {day}: no data, skipped");
                continue;
            }
            match seals::seal_day(&dir, &day, &report::seal_payload(&data), &fmt_iso(now())) {
                Ok(r) => {
                    match r.status() {
                        "sealed" => self.metrics.seals_sealed.fetch_add(1, Ordering::Relaxed),
                        "conflict" => self.metrics.seals_conflict.fetch_add(1, Ordering::Relaxed),
                        _ => 0,
                    };
                    eprintln!("{NAME}: seal {day}: {}", r.status());
                }
                Err(e) => {
                    self.metrics.seals_failed.fetch_add(1, Ordering::Relaxed);
                    eprintln!("{NAME}: warning: writing the seal of {day} failed: {e}");
                }
            }
        }
    }

    pub fn sealer(self: &Arc<Self>, stop: Arc<AtomicBool>) {
        // Let the first refresh of every window go first: sealing a week of
        // days is the heaviest load this process puts on OpenCost.
        sleep_unless(&stop, 120);
        while !stop.load(Ordering::Relaxed) {
            if self.cfg.seals_enabled {
                self.seal_completed_days();
            }
            sleep_unless(&stop, 3600);
        }
    }

    // ── Requests ─────────────────────────────────────────────────────────

    /// Departments the caller could be allowed: configured ones and any
    /// seen in the reports.
    fn known_departments(&self) -> BTreeSet<String> {
        let mut known: BTreeSet<String> = self
            .cfg
            .report
            .departments
            .iter()
            .map(|(id, _)| id.clone())
            .collect();
        for e in self
            .cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
        {
            for d in e
                .data
                .as_ref()
                .and_then(|d| d["depts"].as_array())
                .into_iter()
                .flatten()
            {
                if let Some(id) = d["id"].as_str() {
                    known.insert(id.to_string());
                }
            }
        }
        known
    }

    /// Earliest data start among the cached reports (the longest window
    /// reaches furthest back); the page uses it to say when the first
    /// sealed day will exist.
    fn data_since(&self) -> Option<String> {
        self.cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .filter_map(|e| {
                e.data.as_ref()?["window"]["start"]
                    .as_str()
                    .map(str::to_string)
            })
            .filter_map(|s| parse_iso(&s).ok())
            .min()
            .map(|t| fmt_iso(t as f64))
    }

    fn departments_json(&self, known: &BTreeSet<String>, scope: &Scope) -> Value {
        let mut ids: Vec<&String> = known.iter().filter(|d| scope.allows(d)).collect();
        let order = |id: &str| {
            self.cfg
                .report
                .departments
                .iter()
                .position(|(d, _)| d == id)
                .unwrap_or(usize::MAX)
        };
        ids.sort_by(|a, b| order(a).cmp(&order(b)).then(a.cmp(b)));
        json!(ids
            .iter()
            .map(|id| json!({ "id": id, "name": self.cfg.report.name_of(id) }))
            .collect::<Vec<_>>())
    }

    /// Answer a request on the API port. `caller` is `None` for requests
    /// that did not come through the API server with a user.
    pub fn handle_api(&self, req: &Request, caller: Option<&Caller>) -> Response {
        let group = format!("/apis/{GROUP}");
        let gv = format!("{group}/{VERSION}");
        if req.path == group {
            return Response::json(200, &identity::group_json());
        }
        if req.path == gv {
            return Response::json(200, &identity::discovery_json());
        }
        let Some(rest) = req.path.strip_prefix(&format!("{gv}/")) else {
            return not_found();
        };
        let Some(caller) = caller else {
            return Response::api_error(
                401,
                "unauthenticated",
                "this API is served through the Kubernetes API server",
                json!({}),
            );
        };
        let known = self.known_departments();
        let scope = match self.authz.scope(caller, &known, now()) {
            Ok(s) => s,
            Err(e) => {
                self.metrics.authz_failed.fetch_add(1, Ordering::Relaxed);
                eprintln!("{NAME}: warning: {e}");
                return Response::api_error(
                    503,
                    "authz_unavailable",
                    "cannot check the caller's departments right now",
                    json!({}),
                );
            }
        };
        let scope_json = scope.to_json(&known.iter().cloned().collect::<Vec<_>>());
        let deny = || {
            Response::api_error(
                403,
                "no_department",
                "you have no permission to see any department's cost",
                json!({}),
            )
        };
        if matches!(&scope, Scope::Departments(d) if d.is_empty()) && rest != "config" {
            return deny();
        }
        let parts: Vec<&str> = rest.split('/').collect();
        match parts.as_slice() {
            ["config"] => {
                let card = self.rate_card();
                Response::json(
                    200,
                    &json!({
                        "version": APP_VERSION,
                        "cluster": self.cfg.report.cluster,
                        "windows": self.cfg.windows,
                        "defaultWindow": self.cfg.default_window,
                        "departments": self.departments_json(&known, &scope),
                        "scope": scope_json,
                        "labels": {
                            "department": self.cfg.department_label,
                            "environment": self.cfg.environment_label,
                            "environments": self.cfg.report.environments,
                        },
                        "features": {
                            "seals": self.cfg.seals_enabled,
                            "coverage": self.prom.is_some(),
                            "volumeUsage": self.prom.is_some(),
                            "snapshotter": self.snapshotter.is_some(),
                        },
                        "currency": card.and_then(|c| c.currency),
                        "dataSince": self.data_since(),
                        "ingestLagMinutes": self.cfg.ingest_lag_minutes,
                    }),
                )
            }
            ["departments"] => Response::json(
                200,
                &json!({ "departments": self.departments_json(&known, &scope), "scope": scope_json }),
            ),
            ["reports"] => self.reports(req, &scope, scope_json),
            ["series"] => self.series(req, &scope),
            ["seals"] => {
                let dir = &self.cfg.seal_dir;
                let rows: Vec<Value> = seals::list_seals(dir)
                    .into_iter()
                    .map(|row| {
                        let doc = (!scope.is_all())
                            .then(|| row["day"].as_str().and_then(|d| seals::read_seal(dir, d)))
                            .flatten();
                        scope::seal_row(row, doc.as_ref(), &scope)
                    })
                    .collect();
                Response::json(
                    200,
                    &json!({ "enabled": self.cfg.seals_enabled, "scope": scope_json, "seals": rows }),
                )
            }
            ["seals", day] => match seals::read_seal(&self.cfg.seal_dir, day) {
                Some(doc) => {
                    let mut d = scope::seal_doc(doc, &scope);
                    d["scope"] = scope_json;
                    Response::json(200, &d)
                }
                None => {
                    Response::api_error(404, "not_found", &format!("no seal for {day}"), json!({}))
                }
            },
            ["periods"] => {
                let months: Vec<Value> =
                    seals::aggregate_periods(&self.cfg.seal_dir, &fmt_day(now()))
                        .into_iter()
                        .map(|m| scope::period(m, &scope))
                        .collect();
                Response::json(200, &json!({ "months": months, "scope": scope_json }))
            }
            _ => not_found(),
        }
    }

    fn reports(&self, req: &Request, scope: &Scope, scope_json: Value) -> Response {
        let window = match self.cfg.window_for(req.param("window"), req.param("day")) {
            Ok(w) => w,
            Err(e) => {
                return Response::api_error(
                    400,
                    "bad_window",
                    &e,
                    json!({ "windows": self.cfg.windows }),
                )
            }
        };
        let resolved = coverage::resolve_window(&window, self.data_now());
        if resolved.is_none() {
            return Response::api_error(
                400,
                "bad_window",
                "this period has not started yet",
                json!({ "windows": self.cfg.windows }),
            );
        }
        let cached = self
            .cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&window)
            .cloned();
        // A day still running is not refreshed in the background (only the
        // configured windows are), so it is rebuilt here once it is older than
        // the cache period; otherwise "today" froze at its first answer.
        let running = !self.cfg.windows.contains(&window)
            && resolved.is_some_and(|(_, end)| end as f64 >= self.data_now() - 60.0);
        let too_old = |e: &Entry| now() - e.fetched > self.cfg.cache_ttl as f64;
        let cached = match cached {
            Some(e) if running && too_old(&e) => {
                self.refresh(&window);
                self.cache
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get(&window)
                    .cloned()
            }
            other => other,
        };
        let entry = match cached {
            Some(e) if e.data.is_some() => e,
            // Absolute days and windows not refreshed yet are built on demand.
            _ => {
                self.refresh(&window);
                let e = self
                    .cache
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get(&window)
                    .cloned()
                    .unwrap_or_default();
                if e.data.is_none() {
                    let msg = e
                        .error
                        .unwrap_or_else(|| "no data from OpenCost yet".into());
                    // A configured window that never loaded is "not ready yet";
                    // a day built on demand that failed is OpenCost failing now.
                    return if self.cfg.windows.contains(&window) {
                        Response::api_error(503, "not_ready", &msg, json!({}))
                    } else {
                        Response::api_error(502, "upstream", &msg, json!({}))
                    };
                }
                e
            }
        };
        // Day reports are not refreshed in the background; drop them from
        // the cache after a while so days picked in the page do not pile up.
        self.prune_cache();
        let data = scope::report(entry.data.unwrap_or(Value::Null), scope);
        let mut out = json!({
            "generatedAt": fmt_iso(entry.fetched),
            "ageSeconds": (now() - entry.fetched).round(),
            "ttlSeconds": self.cfg.cache_ttl,
            // Stale also when nothing refreshed it for a while (a background
            // refresh hung, or a running day failed to rebuild).
            "stale": entry.stale || now() - entry.fetched > (3 * self.cfg.cache_ttl) as f64,
            "staleReason": entry.error,
            "windows": self.cfg.windows,
            "scope": scope_json,
        });
        if let (Some(o), Some(d)) = (out.as_object_mut(), data.as_object()) {
            for (k, v) in d {
                o.insert(k.clone(), v.clone());
            }
        }
        Response::json(200, &out)
    }

    fn prune_cache(&self) {
        let ttl = (self.cfg.series_ttl.max(self.cfg.cache_ttl) * 2) as f64;
        let t = now();
        let windows = &self.cfg.windows;
        self.cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|w, e| windows.contains(w) || t - e.fetched < ttl);
    }

    fn series(&self, req: &Request, scope: &Scope) -> Response {
        let window = match self.cfg.window_for(req.param("window"), req.param("day")) {
            Ok(w) => w,
            Err(e) => return Response::api_error(400, "bad_window", &e, json!({})),
        };
        let span = coverage::nominal_minutes(&window);
        let step = req.param("step").unwrap_or("").trim().to_string();
        if !steps::is_allowed(span, &step) {
            let opts = steps::options(span);
            return Response::api_error(
                400,
                "bad_step",
                &format!("this window allows steps {opts:?}"),
                json!({ "options": opts }),
            );
        }
        let ok = |v: &Value, fetched: f64| {
            let mut v = scope::series(v.clone(), scope);
            v["ageSeconds"] = json!((now() - fetched).round());
            Response::json(200, &v)
        };
        // The default step is in the report already (refreshed in the
        // background): served from there, not counted against the budget.
        let entry = self
            .cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&window)
            .cloned();
        if let Some(e) = &entry {
            if let Some(s) = e
                .data
                .as_ref()
                .map(|d| &d["series"])
                .filter(|s| s["step"] == json!(step))
            {
                return ok(s, e.fetched);
            }
        }
        let key = (window.clone(), step.clone());
        let ttl = self.cfg.series_ttl as f64;
        let fresh = |h: &Option<(f64, Value)>| h.as_ref().is_some_and(|(t, _)| now() - t <= ttl);
        let hit = self
            .series_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
            .cloned();
        if fresh(&hit) {
            let (t, v) = hit.unwrap_or_default();
            return ok(&v, t);
        }
        let _one = self.series_lock.lock().unwrap_or_else(|e| e.into_inner());
        // Someone else may have run it while we queued.
        let hit = self
            .series_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
            .cloned();
        if fresh(&hit) {
            let (t, v) = hit.unwrap_or_default();
            return ok(&v, t);
        }
        let segs = (span.unwrap_or(0.0) / steps::step_minutes(&step).unwrap_or(60.0)).ceil();
        if let Err(wait) = self.budget.try_take(segs, now()) {
            self.metrics.series_rejected.fetch_add(1, Ordering::Relaxed);
            return Response::api_error(
                429,
                "series_budget",
                &format!("OpenCost just ran a large query; retry in {wait} seconds (back-to-back large queries make it lose its connections to Prometheus, and the billing report fails with them)"),
                json!({ "retryAfterSeconds": wait }),
            );
        }
        let Some(data) = self.usage_series(&window, Some(&step)) else {
            return Response::api_error(
                502,
                "upstream",
                "OpenCost returned no segments for this window",
                json!({}),
            );
        };
        let t = now();
        let mut cache = self.series_cache.lock().unwrap_or_else(|e| e.into_inner());
        cache.insert(key, (t, data.clone()));
        // Days can be changed endlessly; the cache must not grow without bound.
        cache.retain(|_, (at, _)| t - *at <= ttl);
        drop(cache);
        ok(&data, t)
    }

    /// Answer on the plain port: probes, version, metrics.
    pub fn handle_basic(&self, req: &Request) -> Response {
        match req.path.as_str() {
            "/healthz" => Response::text(200, "ok\n"),
            "/readyz" if self.ready.load(Ordering::Relaxed) => Response::text(200, "ready\n"),
            "/readyz" => Response::text(503, "waiting for the first answer from OpenCost\n"),
            "/version" => Response::json(200, &json!({ "name": NAME, "version": APP_VERSION, "commit": git_sha() })),
            "/metrics" => {
                let mut r = Response::text(200, &self.metrics_text());
                r.content_type = "text/plain; version=0.0.4; charset=utf-8";
                r
            }
            p if p.starts_with("/apis/") => Response::api_error(
                404,
                "not_found",
                "this port only serves health checks; the API is served through the Kubernetes API server",
                json!({}),
            ),
            _ => Response::text(404, "not found\n"),
        }
    }

    fn metrics_text(&self) -> String {
        let m = &self.metrics;
        let g = |a: &AtomicU64| a.load(Ordering::Relaxed);
        let stale = self
            .cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .filter(|e| e.stale)
            .count();
        let seals = seals::list_seals(&self.cfg.seal_dir);
        let tampered = seals.iter().filter(|s| s["intact"] == json!(false)).count();
        [
            (
                "cost_report_refresh_total{result=\"ok\"}",
                "counter",
                "Report refreshes from OpenCost",
                g(&m.refresh_ok) as f64,
            ),
            (
                "cost_report_refresh_total{result=\"failed\"}",
                "",
                "",
                g(&m.refresh_failed) as f64,
            ),
            (
                "cost_report_last_success_timestamp_seconds",
                "gauge",
                "When a report was last built successfully",
                g(&m.last_success) as f64,
            ),
            (
                "cost_report_stale_windows",
                "gauge",
                "Windows currently served from an older report",
                stale as f64,
            ),
            (
                "cost_report_seals_total{result=\"sealed\"}",
                "counter",
                "Days sealed by this process",
                g(&m.seals_sealed) as f64,
            ),
            (
                "cost_report_seals_total{result=\"conflict\"}",
                "",
                "",
                g(&m.seals_conflict) as f64,
            ),
            (
                "cost_report_seals_total{result=\"failed\"}",
                "",
                "",
                g(&m.seals_failed) as f64,
            ),
            (
                "cost_report_sealed_days",
                "gauge",
                "Sealed days on disk",
                seals.len() as f64,
            ),
            (
                "cost_report_sealed_days_tampered",
                "gauge",
                "Sealed days whose hash no longer matches",
                tampered as f64,
            ),
            (
                "cost_report_series_rejected_total",
                "counter",
                "User-chosen series refused by the query budget",
                g(&m.series_rejected) as f64,
            ),
            (
                "cost_report_authz_failures_total",
                "counter",
                "SubjectAccessReviews that could not be made",
                g(&m.authz_failed) as f64,
            ),
        ]
        .iter()
        .map(|(name, kind, help, v)| {
            let base = name.split('{').next().unwrap_or(name);
            let head = if kind.is_empty() {
                String::new()
            } else {
                format!("# HELP {base} {help}\n# TYPE {base} {kind}\n")
            };
            format!("{head}{name} {}\n", r4(*v))
        })
        .collect()
    }
}

fn not_found() -> Response {
    Response::api_error(404, "not_found", "no such resource", json!({}))
}

pub fn sleep_unless(stop: &AtomicBool, secs: u64) {
    for _ in 0..secs * 10 {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Seals directory exists and is writable (checked at startup so a
/// read-only mount is a clear log line, not a silent hourly failure).
pub fn check_seal_dir(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let probe = dir.join(".write-check");
    std::fs::write(&probe, b"ok").map_err(|e| format!("{} is not writable: {e}", dir.display()))?;
    let _ = std::fs::remove_file(probe);
    Ok(())
}
