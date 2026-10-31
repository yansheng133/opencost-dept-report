//! A small HTTP/1.1 server for GET requests: one request per connection.
//!
//! Every endpoint here only reads, so a request with a body is refused
//! rather than parsed. Connections are closed after the answer; the
//! Kubernetes API server opens new ones as needed.

use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Longest request head accepted.
const MAX_HEAD_BYTES: usize = 16 * 1024;
const MAX_HEADERS: usize = 100;
/// Requests handled at once per listener; more get 503.
const MAX_CONNECTIONS: usize = 64;
/// Bound on reading the head and writing the answer.
pub const IO_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    /// Path without the query string.
    pub path: String,
    /// Query parameters, percent-decoded.
    pub query: Vec<(String, String)>,
    /// (lower-case name, trimmed value).
    pub headers: Vec<(String, String)>,
}

impl Request {
    pub fn param(&self, key: &str) -> Option<&str> {
        self.query
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
    pub headers: Vec<(&'static str, String)>,
    /// For the log line only.
    pub error_code: Option<&'static str>,
}

impl Response {
    pub fn text(status: u16, body: &str) -> Self {
        Self {
            status,
            content_type: "text/plain; charset=utf-8",
            body: body.as_bytes().to_vec(),
            headers: Vec::new(),
            error_code: None,
        }
    }

    pub fn json(status: u16, body: &serde_json::Value) -> Self {
        Self {
            status,
            content_type: "application/json",
            body: body.to_string().into_bytes(),
            headers: Vec::new(),
            error_code: None,
        }
    }

    /// An error that is also a Kubernetes `Status`, so `kubectl get --raw`
    /// prints the message; `error` carries the code the page reads.
    pub fn api_error(
        status: u16,
        code: &'static str,
        message: &str,
        extra: serde_json::Value,
    ) -> Self {
        let mut error = serde_json::json!({ "code": code, "message": message });
        if let (Some(e), Some(x)) = (error.as_object_mut(), extra.as_object()) {
            e.extend(x.clone());
        }
        let mut status_doc = serde_json::json!({
            "kind": "Status",
            "apiVersion": "v1",
            "metadata": {},
            "status": "Failure",
            "message": message,
            "reason": status_reason(status),
            "code": status,
            "error": error,
        });
        if let Some(retry) = extra.get("retryAfterSeconds") {
            status_doc["details"] = serde_json::json!({ "retryAfterSeconds": retry });
        }
        let mut r = Self::json(status, &status_doc);
        r.error_code = Some(code);
        if let Some(retry) = extra
            .get("retryAfterSeconds")
            .and_then(serde_json::Value::as_u64)
        {
            r.headers.push(("Retry-After", retry.to_string()));
        }
        r
    }
}

/// Kubernetes `StatusReason` for an HTTP status.
fn status_reason(status: u16) -> &'static str {
    match status {
        400 => "BadRequest",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "NotFound",
        405 => "MethodNotAllowed",
        429 => "TooManyRequests",
        503 => "ServiceUnavailable",
        504 => "Timeout",
        _ => "InternalError",
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        413 => "Content Too Large",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Unknown",
    }
}

pub fn write_response<W: Write>(
    stream: &mut W,
    resp: &Response,
    head_only: bool,
) -> io::Result<()> {
    let mut out = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n",
        resp.status,
        reason(resp.status),
        resp.content_type,
        resp.body.len(),
    )
    .into_bytes();
    for (k, v) in &resp.headers {
        out.extend_from_slice(format!("{k}: {v}\r\n").as_bytes());
    }
    out.extend_from_slice(b"\r\n");
    if !head_only {
        out.extend_from_slice(&resp.body);
    }
    stream.write_all(&out)?;
    stream.flush()
}

fn io_status(e: &io::Error) -> u16 {
    if matches!(
        e.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    ) {
        408
    } else {
        400
    }
}

/// `Ok(None)` when the peer closed without sending anything (TCP probes).
pub fn read_request<R: Read>(stream: &mut R) -> Result<Option<Request>, u16> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    let end = loop {
        let n = match stream.read(&mut chunk) {
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(io_status(&e)),
        };
        if n == 0 {
            return if buf.is_empty() { Ok(None) } else { Err(400) };
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        if buf.len() > MAX_HEAD_BYTES {
            return Err(431);
        }
    };
    parse_head(&buf[..end]).map(Some)
}

pub fn parse_head(head: &[u8]) -> Result<Request, u16> {
    let text = std::str::from_utf8(head).map_err(|_| 400u16)?;
    let mut lines = text.split("\r\n");
    let mut parts = lines.next().unwrap_or("").split(' ');
    let (method, target, version) = match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(m), Some(t), Some(v), None) if !m.is_empty() && t.starts_with('/') => (m, t, v),
        _ => return Err(400),
    };
    if !version.starts_with("HTTP/1.") {
        return Err(400);
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let mut headers = Vec::new();
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            return Err(400);
        };
        if headers.len() >= MAX_HEADERS {
            return Err(431);
        }
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim().to_string();
        // Only GET is served; a body would be a request we do not read.
        let has_body = match name.as_str() {
            "content-length" => value != "0",
            "transfer-encoding" => true,
            _ => false,
        };
        if has_body {
            return Err(413);
        }
        headers.push((name, value));
    }
    Ok(Request {
        method: method.to_string(),
        path: percent_decode(path).ok_or(400u16)?,
        query: parse_query(query).ok_or(400u16)?,
        headers,
    })
}

pub fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' => {
                let hex = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                i += 3;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

fn parse_query(q: &str) -> Option<Vec<(String, String)>> {
    q.split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            Some((
                percent_decode(&k.replace('+', " "))?,
                percent_decode(&v.replace('+', " "))?,
            ))
        })
        .collect()
}

/// A connection this server can answer on: plain TCP or TLS.
pub trait Conn: Read + Write + Send {
    fn tcp(&self) -> &TcpStream;
    /// Say goodbye at the protocol level (TLS close_notify) before closing.
    fn close_notify(&mut self) {}
}

impl Conn for TcpStream {
    fn tcp(&self) -> &TcpStream {
        self
    }
}

/// Close gracefully so the kernel does not reset the connection before
/// the peer read the answer.
fn linger_close(stream: &TcpStream) {
    let _ = stream.shutdown(Shutdown::Write);
    let _ = stream.set_read_timeout(Some(Duration::from_millis(300)));
    let mut sink = [0u8; 1024];
    let mut s = stream;
    let deadline = Instant::now() + Duration::from_secs(1);
    while Instant::now() < deadline {
        match s.read(&mut sink) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
    }
}

/// Answer one request on `conn` with `handle`.
pub fn serve_one<C: Conn>(mut conn: C, handle: &dyn Fn(&Request) -> Response) {
    let started = Instant::now();
    let resp_and_req = match read_request(&mut conn) {
        Ok(None) => return,
        Ok(Some(req)) => {
            let resp = if req.method == "GET" || req.method == "HEAD" {
                handle(&req)
            } else {
                let mut r = Response::text(405, "only GET is served\n");
                r.headers.push(("Allow", "GET, HEAD".into()));
                r
            };
            (resp, Some(req))
        }
        Err(status) => (Response::text(status, reason(status)), None),
    };
    let (resp, req) = resp_and_req;
    let head_only = req.as_ref().is_some_and(|r| r.method == "HEAD");
    if let Some(r) = &req {
        log_line(r, &resp, started);
    }
    let _ = write_response(&mut conn, &resp, head_only);
    conn.close_notify();
    linger_close(conn.tcp());
}

fn log_safe(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .take(200)
        .collect()
}

fn log_line(req: &Request, resp: &Response, started: Instant) {
    // Probes hit these every few seconds; logging them would bury everything else.
    if matches!(req.path.as_str(), "/healthz" | "/readyz" | "/metrics") && resp.status == 200 {
        return;
    }
    let user = req
        .headers
        .iter()
        .find(|(n, _)| n == "x-remote-user")
        .map(|(_, v)| log_safe(v))
        .unwrap_or_else(|| "-".into());
    let code = resp.error_code.map(|c| format!(" {c}")).unwrap_or_default();
    eprintln!(
        "{} {} {}{} {}ms user={}",
        req.method,
        log_safe(&req.path),
        resp.status,
        code,
        started.elapsed().as_millis(),
        user
    );
}

/// Accept connections until `stop` is set, each handled on its own
/// thread, at most [`MAX_CONNECTIONS`] at once.
pub fn accept_loop<F>(listener: TcpListener, stop: Arc<AtomicBool>, handle: F)
where
    F: Fn(TcpStream) + Send + Sync + 'static,
{
    let handle = Arc::new(handle);
    let active = Arc::new(AtomicUsize::new(0));
    let _ = listener.set_nonblocking(true);
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((sock, _)) => {
                let _ = sock.set_nonblocking(false);
                let _ = sock.set_read_timeout(Some(IO_TIMEOUT));
                let _ = sock.set_write_timeout(Some(IO_TIMEOUT));
                if active.load(Ordering::Relaxed) >= MAX_CONNECTIONS {
                    let mut s = sock;
                    let _ = write_response(&mut s, &Response::text(503, "busy\n"), false);
                    continue;
                }
                active.fetch_add(1, Ordering::Relaxed);
                let (h, a) = (Arc::clone(&handle), Arc::clone(&active));
                std::thread::spawn(move || {
                    h(sock);
                    a.fetch_sub(1, Ordering::Relaxed);
                });
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(20))
            }
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Result<Request, u16> {
        parse_head(s.as_bytes())
    }

    #[test]
    fn heads() {
        let r = parse("GET /apis/x/v1/reports?window=24h&day=2026-09-24 HTTP/1.1\r\nHost: a\r\nX-Remote-User: u-1").unwrap();
        assert_eq!(r.path, "/apis/x/v1/reports");
        assert_eq!(r.param("window"), Some("24h"));
        assert_eq!(r.param("day"), Some("2026-09-24"));
        assert_eq!(r.headers[1], ("x-remote-user".into(), "u-1".into()));
        assert_eq!(
            parse("GET /a?x=%E7%A0%94 HTTP/1.1").unwrap().param("x"),
            Some("研")
        );
        assert_eq!(parse("GET a HTTP/1.1"), Err(400));
        assert_eq!(parse("GET /a HTTP/2"), Err(400));
        assert_eq!(parse("POST /a HTTP/1.1\r\nContent-Length: 3"), Err(413));
        assert_eq!(
            parse("POST /a HTTP/1.1\r\nTransfer-Encoding: chunked"),
            Err(413)
        );
        assert!(parse("GET /a HTTP/1.1\r\nContent-Length: 0").is_ok());
        assert_eq!(parse("GET /a?x=%zz HTTP/1.1"), Err(400));
        assert_eq!(parse("GET /a HTTP/1.1\r\nbroken"), Err(400));
    }

    #[test]
    fn error_is_a_status() {
        let r = Response::api_error(
            429,
            "series_budget",
            "wait",
            serde_json::json!({"retryAfterSeconds": 12}),
        );
        let v: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
        assert_eq!(v["kind"], "Status");
        assert_eq!(v["reason"], "TooManyRequests");
        assert_eq!(v["details"]["retryAfterSeconds"], 12);
        assert_eq!(v["error"]["code"], "series_budget");
        assert_eq!(v["error"]["retryAfterSeconds"], 12);
        assert_eq!(r.headers, vec![("Retry-After", "12".to_string())]);
        let mut out = Vec::new();
        write_response(&mut out, &r, true).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.starts_with("HTTP/1.1 429 Too Many Requests\r\n"));
        assert!(s.contains("Retry-After: 12\r\n"));
        assert!(s.ends_with("\r\n\r\n"));
    }
}
