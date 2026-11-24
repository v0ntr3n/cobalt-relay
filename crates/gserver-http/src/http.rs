//! Multi-threaded `std::net::TcpListener` HTTP/1.1 server with `GET`/`POST`,
//! `Cookie`/`Set-Cookie`, `Keep-Alive` persistent connections, `Transfer-Encoding: chunked`,
//! `Content-Length` framing, and graceful shutdown (`#![forbid(unsafe_code)]`, `std` only).

use std::collections::BTreeMap;
use std::io::{self, BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::json::JsonValue;
use crate::url::{parse_cookie_header, parse_query_params, split_request_target};

const MAX_HEADER_LINE_BYTES: usize = 8 * 1024;
const MAX_HEADER_COUNT: usize = 128;
const MAX_BODY_BYTES: usize = 2 * 1024 * 1024; // 2 MiB max payload

/// Supported HTTP methods.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
    Head,
    Options,
    Other,
}

impl HttpMethod {
    pub fn from_token(s: &str) -> Self {
        if s.eq_ignore_ascii_case("GET") {
            HttpMethod::Get
        } else if s.eq_ignore_ascii_case("POST") {
            HttpMethod::Post
        } else if s.eq_ignore_ascii_case("HEAD") {
            HttpMethod::Head
        } else if s.eq_ignore_ascii_case("OPTIONS") {
            HttpMethod::Options
        } else {
            HttpMethod::Other
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            HttpMethod::Get => "GET",
            HttpMethod::Post => "POST",
            HttpMethod::Head => "HEAD",
            HttpMethod::Options => "OPTIONS",
            HttpMethod::Other => "OTHER",
        }
    }
}

/// Parsed incoming HTTP/1.x request.
#[derive(Clone, Debug)]
pub struct HttpRequest {
    pub method: HttpMethod,
    pub raw_method: String,
    pub raw_target: String,
    pub path: String,
    pub raw_query: String,
    pub query: BTreeMap<String, String>,
    pub version: String,
    /// Lowercased header names mapped to header values.
    pub headers: BTreeMap<String, String>,
    /// Parsed cookies from `Cookie:` header(s).
    pub cookies: BTreeMap<String, String>,
    pub body: Vec<u8>,
    pub remote_addr: Option<SocketAddr>,
}

impl HttpRequest {
    /// Case-insensitive header lookup.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(&name.to_ascii_lowercase()).map(String::as_str)
    }

    /// Looks up a parsed cookie by exact cookie name (e.g. `"PHPSESSID"` or `"GSESS_ID"`).
    pub fn cookie(&self, name: &str) -> Option<&str> {
        self.cookies.get(name).map(String::as_str)
    }

    /// Interprets the request body as UTF-8 text.
    pub fn body_utf8(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// Merges URL query parameters with `application/x-www-form-urlencoded` or `application/json`
    /// body fields (query params take precedence if present, with body filling missing keys).
    pub fn merged_params(&self) -> BTreeMap<String, String> {
        let mut params = BTreeMap::new();
        if !self.body.is_empty() {
            let ct = self.header("content-type").unwrap_or("");
            let body_str = self.body_utf8();
            let trimmed = body_str.trim();
            if ct.contains("application/json") || trimmed.starts_with('{') {
                if let Ok(JsonValue::Object(obj)) = crate::json::parse_json(trimmed) {
                    for (k, v) in obj.iter() {
                        if let Some(s) = v.as_scalar_string() {
                            params.insert(k.clone(), s);
                        }
                    }
                }
            } else {
                for (k, v) in parse_query_params(&body_str) {
                    params.insert(k, v);
                }
            }
        }
        for (k, v) in &self.query {
            params.insert(k.clone(), v.clone());
        }
        params
    }

    /// Determines whether this request wants a persistent HTTP connection.
    pub fn wants_keep_alive(&self) -> bool {
        if let Some(conn) = self.header("connection") {
            if conn
                .split(',')
                .any(|tok| tok.trim().eq_ignore_ascii_case("close"))
            {
                return false;
            }
            if conn
                .split(',')
                .any(|tok| tok.trim().eq_ignore_ascii_case("keep-alive"))
            {
                return true;
            }
        }
        self.version.eq_ignore_ascii_case("HTTP/1.1")
    }
}

/// Outgoing HTTP/1.1 response with support for multiple `Set-Cookie` headers.
#[derive(Clone, Debug)]
pub struct HttpResponse {
    pub status_code: u16,
    pub reason: &'static str,
    pub headers: Vec<(String, String)>,
    pub set_cookies: Vec<String>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn new(status_code: u16, reason: &'static str) -> Self {
        Self {
            status_code,
            reason,
            headers: Vec::new(),
            set_cookies: Vec::new(),
            body: Vec::new(),
        }
    }

    pub fn json(status_code: u16, value: &JsonValue) -> Self {
        let reason = status_reason(status_code);
        let payload = value.to_json_string();
        let mut resp = Self::new(status_code, reason);
        resp.set_header("Content-Type", "application/json; charset=utf-8");
        resp.set_header("Cache-Control", "no-store, no-cache, must-revalidate");
        resp.body = payload.into_bytes();
        resp
    }

    pub fn html(status_code: u16, html_body: impl Into<String>) -> Self {
        let mut resp = Self::new(status_code, status_reason(status_code));
        resp.set_header("Content-Type", "text/html; charset=utf-8");
        resp.set_header("Cache-Control", "no-cache");
        resp.body = html_body.into().into_bytes();
        resp
    }

    pub fn text(status_code: u16, text_body: impl Into<String>) -> Self {
        let mut resp = Self::new(status_code, status_reason(status_code));
        resp.set_header("Content-Type", "text/plain; charset=utf-8");
        resp.body = text_body.into().into_bytes();
        resp
    }

    pub fn set_header(&mut self, name: impl Into<String>, value: impl Into<String>) {
        let name = name.into();
        let value = value.into();
        if let Some((_, existing)) = self
            .headers
            .iter_mut()
            .find(|(k, _)| k.eq_ignore_ascii_case(&name))
        {
            *existing = value;
        } else {
            self.headers.push((name, value));
        }
    }

    /// Appends a `Set-Cookie` header with standard attributes (`Path=/; HttpOnly`).
    pub fn add_cookie(&mut self, name: &str, value: &str, max_age_secs: Option<u64>) {
        let mut header = format!("{}={}; Path=/; HttpOnly", name, value);
        if let Some(age) = max_age_secs {
            header.push_str(&format!("; Max-Age={}", age));
        }
        self.set_cookies.push(header);
    }

    /// Serializes the response into wire HTTP/1.1 bytes.
    pub fn write_to<W: Write>(&self, writer: &mut W, keep_alive: bool, head_only: bool) -> io::Result<()> {
        write!(writer, "HTTP/1.1 {} {}\r\n", self.status_code, self.reason)?;
        write!(writer, "Server: gserver-http/0.1.0\r\n")?;
        write!(writer, "Content-Length: {}\r\n", self.body.len())?;
        if keep_alive {
            write!(writer, "Connection: keep-alive\r\n")?;
            write!(writer, "Keep-Alive: timeout=15, max=100\r\n")?;
        } else {
            write!(writer, "Connection: close\r\n")?;
        }
        for (k, v) in &self.headers {
            if k.eq_ignore_ascii_case("content-length") || k.eq_ignore_ascii_case("connection") {
                continue;
            }
            write!(writer, "{}: {}\r\n", k, v)?;
        }
        for cookie in &self.set_cookies {
            write!(writer, "Set-Cookie: {}\r\n", cookie)?;
        }
        write!(writer, "\r\n")?;
        if !head_only && !self.body.is_empty() {
            writer.write_all(&self.body)?;
        }
        writer.flush()
    }
}

pub fn status_reason(code: u16) -> &'static str {
    match code {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        500 => "Internal Server Error",
        _ => "OK",
    }
}

/// Trait implemented by HTTP request handlers (such as [`crate::router::MapiService`]).
pub trait HttpHandler: Send + Sync + 'static {
    fn handle(&self, req: &HttpRequest) -> HttpResponse;
}

impl<F> HttpHandler for F
where
    F: Fn(&HttpRequest) -> HttpResponse + Send + Sync + 'static,
{
    fn handle(&self, req: &HttpRequest) -> HttpResponse {
        (self)(req)
    }
}

#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
enum ReadRequestOutcome {
    Request(HttpRequest),
    CleanEof,
    BadRequest(&'static str),
    PayloadTooLarge,
}

fn read_bounded_line<R: BufRead>(reader: &mut R, max_len: usize) -> io::Result<Option<String>> {
    let mut buf = Vec::new();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            if buf.is_empty() {
                return Ok(None);
            }
            return Ok(Some(String::from_utf8_lossy(&buf).into_owned()));
        }
        if let Some(nl_pos) = available.iter().position(|&b| b == b'\n') {
            if buf.len() + nl_pos + 1 > max_len {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "header line too long"));
            }
            buf.extend_from_slice(&available[..=nl_pos]);
            reader.consume(nl_pos + 1);
            if buf.ends_with(b"\n") {
                buf.pop();
            }
            if buf.ends_with(b"\r") {
                buf.pop();
            }
            return Ok(Some(String::from_utf8_lossy(&buf).into_owned()));
        } else {
            if buf.len() + available.len() > max_len {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "header line too long"));
            }
            let len = available.len();
            buf.extend_from_slice(available);
            reader.consume(len);
        }
    }
}

#[allow(clippy::result_large_err)]
fn read_chunked_body<R: BufRead>(reader: &mut R) -> Result<Vec<u8>, ReadRequestOutcome> {
    let mut out = Vec::new();
    loop {
        let line = match read_bounded_line(reader, MAX_HEADER_LINE_BYTES) {
            Ok(Some(l)) => l,
            Ok(None) | Err(_) => return Err(ReadRequestOutcome::BadRequest("truncated chunk size")),
        };
        let size_hex = line.split(';').next().unwrap_or("").trim();
        let chunk_size = usize::from_str_radix(size_hex, 16)
            .map_err(|_| ReadRequestOutcome::BadRequest("invalid chunk size"))?;
        if chunk_size == 0 {
            while let Ok(Some(trailer)) = read_bounded_line(reader, MAX_HEADER_LINE_BYTES) {
                if trailer.is_empty() {
                    break;
                }
            }
            return Ok(out);
        }
        if out.len().saturating_add(chunk_size) > MAX_BODY_BYTES {
            return Err(ReadRequestOutcome::PayloadTooLarge);
        }
        let start = out.len();
        out.resize(start + chunk_size, 0);
        if reader.read_exact(&mut out[start..]).is_err() {
            return Err(ReadRequestOutcome::BadRequest("truncated chunk payload"));
        }
        let mut crlf = [0u8; 2];
        if reader.read_exact(&mut crlf).is_err() || &crlf != b"\r\n" {
            return Err(ReadRequestOutcome::BadRequest("missing CRLF after chunk"));
        }
    }
}

fn parse_http_request<R: BufRead>(
    reader: &mut R,
    remote_addr: Option<SocketAddr>,
) -> ReadRequestOutcome {
    let request_line = loop {
        match read_bounded_line(reader, MAX_HEADER_LINE_BYTES) {
            Ok(Some(line)) if line.is_empty() => continue,
            Ok(Some(line)) => break line,
            Ok(None) => return ReadRequestOutcome::CleanEof,
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::ConnectionReset) => {
                return ReadRequestOutcome::CleanEof;
            }
            Err(_) => return ReadRequestOutcome::BadRequest("malformed request line"),
        }
    };

    let mut parts = request_line.split_whitespace();
    let Some(raw_method) = parts.next() else {
        return ReadRequestOutcome::BadRequest("missing HTTP method");
    };
    let Some(raw_target) = parts.next() else {
        return ReadRequestOutcome::BadRequest("missing request target");
    };
    let version = parts.next().unwrap_or("HTTP/1.0").to_string();
    if parts.next().is_some() {
        return ReadRequestOutcome::BadRequest("extra tokens in request line");
    }

    let mut headers = BTreeMap::new();
    let mut cookies = BTreeMap::new();
    let mut header_count = 0usize;

    loop {
        let line = match read_bounded_line(reader, MAX_HEADER_LINE_BYTES) {
            Ok(Some(l)) => l,
            Ok(None) => return ReadRequestOutcome::BadRequest("unterminated HTTP headers"),
            Err(_) => return ReadRequestOutcome::BadRequest("header line too large"),
        };
        if line.is_empty() {
            break;
        }
        header_count += 1;
        if header_count > MAX_HEADER_COUNT {
            return ReadRequestOutcome::BadRequest("too many HTTP headers");
        }
        let Some(colon) = line.find(':') else {
            return ReadRequestOutcome::BadRequest("malformed header line");
        };
        let name = line[..colon].trim().to_ascii_lowercase();
        let val = line[colon + 1..].trim().to_string();
        if name == "cookie" {
            for (ck, cv) in parse_cookie_header(&val) {
                cookies.insert(ck, cv);
            }
        }
        headers
            .entry(name)
            .and_modify(|existing: &mut String| {
                existing.push_str("; ");
                existing.push_str(&val);
            })
            .or_insert(val);
    }

    let body = if headers
        .get("transfer-encoding")
        .map(|v| v.to_ascii_lowercase().contains("chunked"))
        .unwrap_or(false)
    {
        match read_chunked_body(reader) {
            Ok(b) => b,
            Err(outcome) => return outcome,
        }
    } else if let Some(cl_str) = headers.get("content-length") {
        let Ok(cl) = cl_str.trim().parse::<usize>() else {
            return ReadRequestOutcome::BadRequest("invalid Content-Length");
        };
        if cl > MAX_BODY_BYTES {
            return ReadRequestOutcome::PayloadTooLarge;
        }
        let mut buf = vec![0u8; cl];
        if cl > 0 && reader.read_exact(&mut buf).is_err() {
            return ReadRequestOutcome::BadRequest("truncated request body");
        }
        buf
    } else {
        Vec::new()
    };

    let (path, raw_query) = split_request_target(raw_target);
    let query = parse_query_params(&raw_query);

    ReadRequestOutcome::Request(HttpRequest {
        method: HttpMethod::from_token(raw_method),
        raw_method: raw_method.to_string(),
        raw_target: raw_target.to_string(),
        path,
        raw_query,
        query,
        version,
        headers,
        cookies,
        body,
        remote_addr,
    })
}

fn serve_connection<H: HttpHandler>(
    stream: TcpStream,
    remote_addr: Option<SocketAddr>,
    handler: &H,
    shutdown: &AtomicBool,
    keep_alive_timeout: Duration,
    max_requests_per_conn: usize,
) {
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(keep_alive_timeout));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(15)));

    let Ok(writer_stream) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(stream);
    let mut writer = io::BufWriter::new(writer_stream);

    for req_idx in 0..max_requests_per_conn {
        if shutdown.load(Ordering::Acquire) {
            break;
        }
        match parse_http_request(&mut reader, remote_addr) {
            ReadRequestOutcome::CleanEof => break,
            ReadRequestOutcome::BadRequest(msg) => {
                let mut err_obj = crate::json::JsonObject::new();
                err_obj.insert("errornu", "1");
                err_obj.insert("errordesc", msg);
                let resp = HttpResponse::json(400, &JsonValue::Object(err_obj));
                let _ = resp.write_to(&mut writer, false, false);
                break;
            }
            ReadRequestOutcome::PayloadTooLarge => {
                let mut err_obj = crate::json::JsonObject::new();
                err_obj.insert("errornu", "1");
                err_obj.insert("errordesc", "payload too large");
                let resp = HttpResponse::json(413, &JsonValue::Object(err_obj));
                let _ = resp.write_to(&mut writer, false, false);
                break;
            }
            ReadRequestOutcome::Request(req) => {
                let keep_alive = req.wants_keep_alive()
                    && !shutdown.load(Ordering::Acquire)
                    && (req_idx + 1 < max_requests_per_conn);
                let head_only = req.method == HttpMethod::Head;

                let resp = if req.method == HttpMethod::Options {
                    let mut r = HttpResponse::new(204, "No Content");
                    r.set_header("Allow", "GET, POST, HEAD, OPTIONS");
                    r.set_header("Access-Control-Allow-Origin", "*");
                    r.set_header("Access-Control-Allow-Methods", "GET, POST, HEAD, OPTIONS");
                    r.set_header("Access-Control-Allow-Headers", "Content-Type, Cookie");
                    r
                } else {
                    let mut r = handler.handle(&req);
                    r.set_header("Access-Control-Allow-Origin", "*");
                    r
                };

                if resp.write_to(&mut writer, keep_alive, head_only).is_err() {
                    break;
                }
                if !keep_alive {
                    break;
                }
            }
        }
    }
}

/// Handle to a running multi-threaded HTTP/1.1 server. Dropping or calling
/// [`HttpServerHandle::shutdown`] cleanly stops the accept loop and joins worker threads.
pub struct HttpServerHandle {
    local_addr: SocketAddr,
    shutdown: Arc<AtomicBool>,
    active_connections: Arc<AtomicU64>,
    accept_thread: Option<JoinHandle<()>>,
    workers: Arc<Mutex<Vec<JoinHandle<()>>>>,
}

impl HttpServerHandle {
    /// Bound socket address (useful when binding to `127.0.0.1:0` in tests).
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Base URL string `http://<ip>:<port>`.
    pub fn base_url(&self) -> String {
        format!("http://{}", self.local_addr)
    }

    /// Number of currently active client TCP connections.
    pub fn active_connections(&self) -> u64 {
        self.active_connections.load(Ordering::Relaxed)
    }

    /// Signals shutdown, wakes the blocking `TcpListener::accept`, and joins threads.
    pub fn shutdown(&mut self) {
        if !self.shutdown.swap(true, Ordering::SeqCst) {
            let _ = TcpStream::connect_timeout(&self.local_addr, Duration::from_millis(200));
        }
        if let Some(handle) = self.accept_thread.take() {
            let _ = handle.join();
        }
        if let Ok(mut guard) = self.workers.lock() {
            for worker in guard.drain(..) {
                let _ = worker.join();
            }
        }
    }
}

impl Drop for HttpServerHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Multi-threaded HTTP/1.1 server configuration.
#[derive(Clone, Debug)]
pub struct HttpServerConfig {
    pub keep_alive_timeout: Duration,
    pub max_requests_per_connection: usize,
}

impl Default for HttpServerConfig {
    fn default() -> Self {
        Self {
            keep_alive_timeout: Duration::from_secs(5),
            max_requests_per_connection: 100,
        }
    }
}

/// Starts a multi-threaded HTTP/1.1 server bound to `addr` (`"127.0.0.1:0"`, `"0.0.0.0:8080"`, etc.).
pub fn spawn_http_server<A, H>(
    addr: A,
    handler: Arc<H>,
    config: HttpServerConfig,
) -> io::Result<HttpServerHandle>
where
    A: ToSocketAddrs,
    H: HttpHandler,
{
    let listener = TcpListener::bind(addr)?;
    let local_addr = listener.local_addr()?;
    let shutdown = Arc::new(AtomicBool::new(false));
    let active_connections = Arc::new(AtomicU64::new(0));
    let workers: Arc<Mutex<Vec<JoinHandle<()>>>> = Arc::new(Mutex::new(Vec::new()));

    let shutdown_accept = Arc::clone(&shutdown);
    let active_accept = Arc::clone(&active_connections);
    let workers_accept = Arc::clone(&workers);

    let accept_thread = thread::Builder::new()
        .name("gserver-http-accept".to_string())
        .spawn(move || {
            for incoming in listener.incoming() {
                if shutdown_accept.load(Ordering::Acquire) {
                    break;
                }
                let Ok(stream) = incoming else {
                    continue;
                };
                if shutdown_accept.load(Ordering::Acquire) {
                    break;
                }

                let remote = stream.peer_addr().ok();
                let handler_cl = Arc::clone(&handler);
                let shutdown_cl = Arc::clone(&shutdown_accept);
                let active_cl = Arc::clone(&active_accept);
                let ka_timeout = config.keep_alive_timeout;
                let max_reqs = config.max_requests_per_connection;

                active_cl.fetch_add(1, Ordering::Relaxed);
                if let Ok(worker) = thread::Builder::new()
                    .name("gserver-http-worker".to_string())
                    .spawn(move || {
                        serve_connection(
                            stream,
                            remote,
                            handler_cl.as_ref(),
                            &shutdown_cl,
                            ka_timeout,
                            max_reqs,
                        );
                        active_cl.fetch_sub(1, Ordering::Relaxed);
                    })
                {
                    if let Ok(mut list) = workers_accept.lock() {
                        list.retain(|h| !h.is_finished());
                        list.push(worker);
                    }
                } else {
                    active_accept.fetch_sub(1, Ordering::Relaxed);
                }
            }
        })?;

    Ok(HttpServerHandle {
        local_addr,
        shutdown,
        active_connections,
        accept_thread: Some(accept_thread),
        workers,
    })
}
