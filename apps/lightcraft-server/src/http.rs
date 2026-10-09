//! The HTTP/1.1 server under the API: what the API needs of an HTTP library (a request's method,
//! URL, headers and body; a response with headers and a body), with the limits the library it
//! replaced (`tiny_http`) didn't have. A connection that stops sending is closed (the head of a
//! request must arrive within [`Limits::head`], a body may pause for [`Limits::body`], an idle
//! connection is closed after [`Limits::idle`]); connections, connections per client address and
//! requests served at once are bounded; the head of a request (line and headers) and its number of headers
//! are capped. `/api/health` and `OPTIONS` never wait for a free place. TLS isn't here: the server
//! sits behind a reverse proxy or on a private network (`docs/sync.md`).
//!
//! One thread per connection, each serving its requests in turn (keep-alive). Request bodies must
//! say their length (`Content-Length`): `Transfer-Encoding: chunked` is answered with `411`.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{IpAddr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// The most a request's head (line and headers) may hold.
const HEAD_MAX: usize = 32 << 10;
const HEADERS_MAX: usize = 64;
/// How often a waiting read looks at the clock and at whether the server is stopping.
const SLICE: Duration = Duration::from_millis(200);
/// What an answer takes at most to be written to a client that reads it.
const WRITE_TIMEOUT: Duration = Duration::from_secs(60);
/// A request body of at most this size must arrive within [`SMALL_BODY_TIME`] altogether (the
/// ones anyone can send are far smaller: no trickling a few bytes a minute).
const SMALL_BODY: u64 = 1 << 20;
const SMALL_BODY_TIME: Duration = Duration::from_secs(30);
/// An unread request body this small is read and dropped to keep the connection; else it closes.
const DRAIN_MAX: u64 = 1 << 20;

/// What is bounded, and how long things may take.
#[derive(Clone, Debug)]
pub struct Limits {
    /// Open connections at most (more are answered `503` and closed).
    pub connections: usize,
    /// Open connections from one address at most.
    pub per_address: usize,
    /// Requests being answered at once at most (more get `503`; `/api/health` and `OPTIONS` don't count).
    pub active: usize,
    /// The head of a request must arrive this soon after its first byte.
    pub head: Duration,
    /// A request body may pause this long.
    pub body: Duration,
    /// A connection with no request for this long is closed.
    pub idle: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            connections: 512,
            per_address: 128,
            active: 64,
            head: Duration::from_secs(10),
            body: Duration::from_secs(60),
            idle: Duration::from_secs(30),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Head,
    Post,
    Put,
    Delete,
    Options,
    Other(String),
}

impl Method {
    fn parse(s: &str) -> Method {
        match s {
            "GET" => Method::Get,
            "HEAD" => Method::Head,
            "POST" => Method::Post,
            "PUT" => Method::Put,
            "DELETE" => Method::Delete,
            "OPTIONS" => Method::Options,
            other => Method::Other(other.to_string()),
        }
    }
}

impl std::fmt::Display for Method {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Method::Get => "GET",
            Method::Head => "HEAD",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Delete => "DELETE",
            Method::Options => "OPTIONS",
            Method::Other(o) => o,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatusCode(pub u16);

#[derive(Clone, Debug)]
pub struct Field(String);

impl Field {
    /// Header names compare without regard to case.
    pub fn equiv(&self, other: &str) -> bool {
        self.0.eq_ignore_ascii_case(other)
    }
}

impl std::fmt::Display for Field {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug)]
pub struct Header {
    pub field: Field,
    pub value: String,
}

/// A header name or value that can't go on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BadHeader;

impl Header {
    /// A header whose name and value can go on the wire: no control characters, a name made of
    /// token characters.
    pub fn from_bytes(field: &[u8], value: &[u8]) -> Result<Header, BadHeader> {
        let name = std::str::from_utf8(field).map_err(|_| BadHeader)?;
        let value = std::str::from_utf8(value).map_err(|_| BadHeader)?;
        let token = |c: char| c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c);
        if name.is_empty() || !name.chars().all(token) || value.chars().any(|c| c.is_control() && c != '\t') {
            return Err(BadHeader);
        }
        Ok(Header { field: Field(name.to_string()), value: value.to_string() })
    }
}

/// An answer: status, headers and a body of `len` bytes (unknown: sent until it ends, and the
/// connection closes after it).
pub struct Response<R> {
    status: StatusCode,
    headers: Vec<Header>,
    body: R,
    len: Option<usize>,
}

impl<R: Read> Response<R> {
    pub fn new(status: StatusCode, headers: Vec<Header>, body: R, len: Option<usize>, _unused: Option<()>) -> Response<R> {
        Response { status, headers, body, len }
    }
    pub fn status_code(&self) -> StatusCode {
        self.status
    }
    pub fn headers(&self) -> &[Header] {
        &self.headers
    }
    pub fn add_header(&mut self, h: Header) {
        self.headers.push(h);
    }
}

/// What the API answers with.
pub type Resp = Response<Box<dyn Read + Send>>;

/// The buffered connection. Reads wait in slices, so a stopping server and the deadlines are
/// noticed within a fraction of a second.
struct Conn {
    stream: TcpStream,
    buf: Box<[u8]>,
    pos: usize,
    end: usize,
    stop: Arc<AtomicBool>,
}

impl Conn {
    fn buffered(&self) -> &[u8] {
        self.buf.get(self.pos..self.end).unwrap_or_default()
    }

    fn consume(&mut self, n: usize) {
        self.pos = self.pos.saturating_add(n).min(self.end);
    }

    /// The bytes that have arrived and weren't used, waiting for some for at most `idle` and
    /// until `deadline`. Empty: the other end closed the connection.
    fn fill(&mut self, idle: Duration, deadline: Option<Instant>) -> io::Result<&[u8]> {
        if self.pos < self.end {
            return Ok(self.buffered());
        }
        let started = Instant::now();
        loop {
            if self.stop.load(Ordering::Relaxed) {
                return Err(io::Error::new(io::ErrorKind::ConnectionAborted, "the server is stopping"));
            }
            let mut left = idle.saturating_sub(started.elapsed());
            if let Some(d) = deadline {
                left = left.min(d.saturating_duration_since(Instant::now()));
            }
            if left.is_zero() {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "nothing arrived in time"));
            }
            self.stream.set_read_timeout(Some(left.min(SLICE).max(Duration::from_millis(1))))?;
            match self.stream.read(&mut self.buf) {
                Ok(n) => {
                    self.pos = 0;
                    self.end = n;
                    return Ok(self.buffered());
                }
                Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted) => continue,
                Err(e) => return Err(e),
            }
        }
    }
}

/// A request body: exactly its `Content-Length` bytes. A connection that breaks before then is
/// an error (not an early end), so a half-sent upload is never taken for a whole one.
struct Body {
    conn: Conn,
    left: u64,
    idle: Duration,
    deadline: Option<Instant>,
}

impl Read for Body {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.left == 0 || out.is_empty() {
            return Ok(0);
        }
        let want = usize::try_from(self.left).unwrap_or(usize::MAX).min(out.len());
        let avail = self.conn.fill(self.idle, self.deadline)?;
        if avail.is_empty() {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "the connection closed before the request body was complete"));
        }
        let n = avail.len().min(want);
        let (Some(from), Some(to)) = (avail.get(..n), out.get_mut(..n)) else { return Err(io::Error::other("buffer")) };
        to.copy_from_slice(from);
        self.conn.consume(n);
        self.left -= n as u64;
        Ok(n)
    }
}

pub struct Request {
    method: Method,
    url: String,
    headers: Vec<Header>,
    remote: SocketAddr,
    length: u64,
    body: Body,
}

impl Request {
    pub fn method(&self) -> &Method {
        &self.method
    }
    /// The path and query as sent.
    pub fn url(&self) -> &str {
        &self.url
    }
    pub fn headers(&self) -> &[Header] {
        &self.headers
    }
    /// The length of the body (`0` when there is none).
    pub fn body_length(&self) -> Option<usize> {
        usize::try_from(self.length).ok()
    }
    pub fn as_reader(&mut self) -> &mut dyn Read {
        &mut self.body
    }
    pub fn remote_addr(&self) -> Option<&SocketAddr> {
        Some(&self.remote)
    }
}

/// Answers a request.
pub type Handler = dyn Fn(&mut Request) -> Resp + Send + Sync;

struct Shared {
    limits: Limits,
    stop: Arc<AtomicBool>,
    conns: AtomicUsize,
    per_ip: Mutex<HashMap<IpAddr, usize>>,
    active: AtomicUsize,
    handler: Arc<Handler>,
}

/// A place among the open connections, given back on drop.
struct Seat {
    shared: Arc<Shared>,
    ip: IpAddr,
}

impl Seat {
    fn take(shared: &Arc<Shared>, ip: IpAddr) -> Option<Seat> {
        if shared.conns.fetch_add(1, Ordering::SeqCst) >= shared.limits.connections {
            shared.conns.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        let mut per = shared.per_ip.lock().unwrap_or_else(PoisonError::into_inner);
        let n = per.entry(ip).or_insert(0);
        if *n >= shared.limits.per_address {
            drop(per);
            shared.conns.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        *n += 1;
        Some(Seat { shared: shared.clone(), ip })
    }
}

impl Drop for Seat {
    fn drop(&mut self) {
        let mut per = self.shared.per_ip.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(n) = per.get_mut(&self.ip) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                per.remove(&self.ip);
            }
        }
        drop(per);
        self.shared.conns.fetch_sub(1, Ordering::SeqCst);
    }
}

/// One request being answered, counted against [`Limits::active`].
struct Slot<'a>(&'a AtomicUsize);

impl<'a> Slot<'a> {
    fn take(active: &'a AtomicUsize, max: usize) -> Option<Slot<'a>> {
        if active.fetch_add(1, Ordering::SeqCst) >= max {
            active.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        Some(Slot(active))
    }
}

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

pub struct Server {
    addr: SocketAddr,
    shared: Arc<Shared>,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// An address being listened on, before anything is answered (the handler may need to know it).
pub struct Bound {
    listener: TcpListener,
    addr: SocketAddr,
    limits: Limits,
}

impl Bound {
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Answer requests with `handler`, each on a thread of its connection.
    pub fn serve(self, handler: Arc<Handler>) -> Result<Server, String> {
        let Bound { listener, addr, limits } = self;
        let shared = Arc::new(Shared {
            limits,
            stop: Arc::new(AtomicBool::new(false)),
            conns: AtomicUsize::new(0),
            per_ip: Mutex::new(HashMap::new()),
            active: AtomicUsize::new(0),
            handler,
        });
        let s = shared.clone();
        let thread = std::thread::Builder::new().name("lc-accept".into()).spawn(move || accept(&listener, &s)).map_err(|e| e.to_string())?;
        Ok(Server { addr, shared, thread: Some(thread) })
    }
}

impl Server {
    /// Listen on `listen` (`host:port`).
    pub fn bind(listen: &str, limits: Limits) -> Result<Bound, String> {
        let listener = TcpListener::bind(listen).map_err(|e| e.to_string())?;
        let addr = listener.local_addr().map_err(|e| e.to_string())?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        Ok(Bound { listener, addr, limits })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Serve until the process ends.
    pub fn wait(mut self) {
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Server {
    /// Stop accepting, let the requests being answered finish (a few seconds at most), close the rest.
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        let until = Instant::now() + Duration::from_secs(5);
        while self.shared.conns.load(Ordering::SeqCst) > 0 && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

fn accept(listener: &TcpListener, shared: &Arc<Shared>) {
    while !shared.stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, remote)) => {
                let Some(seat) = Seat::take(shared, remote.ip()) else {
                    refuse(stream);
                    continue;
                };
                let s = shared.clone();
                let spawned = std::thread::Builder::new().name("lc-conn".into()).spawn(move || {
                    let _seat = seat;
                    // a panic in the API is that connection's failure
                    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| serve(stream, remote, &s))).is_err() {
                        log::error!("a request failed unexpectedly");
                    }
                });
                if let Err(e) = spawned {
                    log::error!("can't start a connection thread: {e}");
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => {
                log::warn!("accepting a connection: {e}");
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

/// Too many connections: say so and hang up (without waiting for the client).
fn refuse(mut stream: TcpStream) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
    let _ = stream.write_all(b"HTTP/1.1 503 Service Unavailable\r\nConnection: close\r\nContent-Length: 0\r\nRetry-After: 5\r\n\r\n");
    let _ = stream.shutdown(Shutdown::Both);
}

/// Why a request couldn't be read, as the answer to send.
struct Bad(u16, &'static str);

/// A request's head.
struct Head {
    method: Method,
    url: String,
    headers: Vec<Header>,
    keep: bool,
    length: u64,
    expect_continue: bool,
}

fn serve(stream: TcpStream, remote: SocketAddr, shared: &Shared) {
    if stream.set_nonblocking(false).is_err() || stream.set_write_timeout(Some(WRITE_TIMEOUT)).is_err() {
        return;
    }
    let _ = stream.set_nodelay(true);
    let Ok(mut out) = stream.try_clone() else { return };
    let mut conn = Conn { stream, buf: vec![0u8; 16 << 10].into_boxed_slice(), pos: 0, end: 0, stop: shared.stop.clone() };
    loop {
        let head = match read_head(&mut conn, &shared.limits) {
            Ok(Some(h)) => h,
            Ok(None) => return,
            Err(Bad(code, why)) => {
                let _ = respond(&mut out, error_response(code, why), false, false);
                return;
            }
        };
        if head.expect_continue && out.write_all(b"HTTP/1.1 100 Continue\r\n\r\n").is_err() {
            return;
        }
        let small = head.length <= SMALL_BODY;
        let body = Body { conn, left: head.length, idle: shared.limits.body, deadline: small.then(|| Instant::now() + SMALL_BODY_TIME) };
        let (path, _) = head.url.split_once('?').unwrap_or((head.url.as_str(), ""));
        // a health check and a preflight are never turned away for want of a place
        let exempt = path == "/api/health" || head.method == Method::Options;
        let head_only = head.method == Method::Head;
        let keep = head.keep;
        let mut req = Request { method: head.method, url: head.url, headers: head.headers, remote, length: head.length, body };
        let slot = if exempt { None } else { Slot::take(&shared.active, shared.limits.active) };
        let resp = if slot.is_none() && !exempt {
            let mut r = error_response(503, "the server is busy: try again in a moment");
            r.add_header(Header { field: Field("Retry-After".into()), value: "5".into() });
            r
        } else {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (shared.handler)(&mut req))) {
                Ok(r) => r,
                Err(_) => {
                    log::error!("{} {}: the request failed unexpectedly", req.method, req.url);
                    let _ = respond(&mut out, error_response(500, "the server failed on this request"), head_only, false);
                    return;
                }
            }
        };
        drop(slot);
        // an unread body is read away to keep the connection, if it is small
        let Request { body: mut b, .. } = req;
        let mut keep = keep && !shared.stop.load(Ordering::Relaxed);
        if b.left > 0 {
            if b.left <= DRAIN_MAX && b.deadline.is_none() {
                b.deadline = Some(Instant::now() + SMALL_BODY_TIME);
            }
            keep = keep && b.left <= DRAIN_MAX && io::copy(&mut (&mut b).take(DRAIN_MAX), &mut io::sink()).is_ok() && b.left == 0;
        }
        conn = b.conn;
        if respond(&mut out, resp, head_only, keep).is_err() || !keep {
            let _ = out.shutdown(Shutdown::Both);
            return;
        }
    }
}

/// Read the next request's head. `None`: the client closed the connection (or went quiet) between requests.
fn read_head(conn: &mut Conn, limits: &Limits) -> Result<Option<Head>, Bad> {
    let mut acc: Vec<u8> = Vec::new();
    let mut began: Option<Instant> = None;
    let end = loop {
        let wait = if acc.is_empty() { limits.idle } else { limits.head };
        let deadline = began.map(|t| t + limits.head);
        let avail = match conn.fill(wait, deadline) {
            Ok(a) => a,
            // quiet between requests, or gone, is not an error
            Err(_) if acc.is_empty() => return Ok(None),
            Err(e) if e.kind() == io::ErrorKind::TimedOut => return Err(Bad(408, "the request did not arrive in time")),
            Err(_) => return Ok(None),
        };
        if avail.is_empty() {
            return Ok(None);
        }
        began.get_or_insert_with(Instant::now);
        let room = HEAD_MAX.saturating_sub(acc.len());
        let take = avail.len().min(room);
        let before = acc.len();
        acc.extend_from_slice(avail.get(..take).unwrap_or_default());
        // (the end may straddle two reads)
        let from = before.saturating_sub(3);
        if let Some(i) = acc.get(from..).and_then(|t| t.windows(4).position(|w| w == b"\r\n\r\n")) {
            let end = from + i + 4;
            conn.consume(end - before);
            break end;
        }
        conn.consume(take);
        if acc.len() >= HEAD_MAX {
            return Err(Bad(431, "the request's headers are too large"));
        }
    };
    let text = acc.get(..end).unwrap_or_default();
    let mut slots = [httparse::EMPTY_HEADER; HEADERS_MAX];
    let mut parsed = httparse::Request::new(&mut slots);
    match parsed.parse(text) {
        Ok(httparse::Status::Complete(_)) => {}
        Ok(httparse::Status::Partial) => return Err(Bad(400, "not a complete HTTP request")),
        Err(httparse::Error::TooManyHeaders) => return Err(Bad(431, "too many headers")),
        Err(_) => return Err(Bad(400, "not an HTTP request")),
    }
    let (Some(method), Some(url)) = (parsed.method, parsed.path) else { return Err(Bad(400, "not an HTTP request")) };
    let http11 = parsed.version == Some(1);
    let mut headers = Vec::new();
    let (mut length, mut keep, mut expect_continue) = (None::<u64>, http11, false);
    for h in parsed.headers.iter() {
        let Ok(value) = std::str::from_utf8(h.value) else { return Err(Bad(400, "a header isn't text")) };
        let value = value.trim();
        if h.name.eq_ignore_ascii_case("content-length") {
            let Ok(n) = value.parse::<u64>() else { return Err(Bad(400, "bad Content-Length")) };
            if length.is_some_and(|old| old != n) {
                return Err(Bad(400, "two different Content-Length headers"));
            }
            length = Some(n);
        } else if h.name.eq_ignore_ascii_case("transfer-encoding") {
            return Err(Bad(411, "send the body with a Content-Length (chunked bodies aren't taken)"));
        } else if h.name.eq_ignore_ascii_case("connection") {
            let has = |t: &str| value.split(',').any(|v| v.trim().eq_ignore_ascii_case(t));
            if has("close") {
                keep = false;
            } else if has("keep-alive") {
                keep = true;
            }
        } else if h.name.eq_ignore_ascii_case("expect") && value.eq_ignore_ascii_case("100-continue") {
            expect_continue = true;
        }
        headers.push(Header { field: Field(h.name.to_string()), value: value.to_string() });
    }
    Ok(Some(Head { method: Method::parse(method), url: url.to_string(), headers, keep, length: length.unwrap_or(0), expect_continue }))
}

fn error_response(code: u16, why: &str) -> Resp {
    let body = format!("{{\"error\":{}}}", serde_json::Value::from(why));
    let len = body.len();
    let ct = Header { field: Field("Content-Type".into()), value: "application/json".into() };
    Response::new(StatusCode(code), vec![ct], Box::new(io::Cursor::new(body.into_bytes())), Some(len), None)
}

fn reason(code: u16) -> &'static str {
    match code {
        100 => "Continue",
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        206 => "Partial Content",
        301 => "Moved Permanently",
        302 => "Found",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        409 => "Conflict",
        411 => "Length Required",
        412 => "Precondition Failed",
        413 => "Payload Too Large",
        414 => "URI Too Long",
        416 => "Range Not Satisfiable",
        422 => "Unprocessable Content",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Status",
    }
}

/// Write an answer. `head_only`: a `HEAD` request (the headers say how long the body would be).
/// `keep`: whether the connection stays open (an answer of unknown length closes it).
fn respond(out: &mut TcpStream, resp: Resp, head_only: bool, keep: bool) -> io::Result<()> {
    let Response { status, headers, mut body, len } = resp;
    let keep = keep && (len.is_some() || head_only);
    let mut head = format!("HTTP/1.1 {} {}\r\n", status.0, reason(status.0));
    for h in &headers {
        // (what the connection decides, the server says itself)
        if h.field.equiv("content-length") || h.field.equiv("connection") || h.field.equiv("transfer-encoding") {
            continue;
        }
        head.push_str(&format!("{}: {}\r\n", h.field, h.value));
    }
    if let Some(n) = len {
        head.push_str(&format!("Content-Length: {n}\r\n"));
    }
    head.push_str(if keep { "Connection: keep-alive\r\n\r\n" } else { "Connection: close\r\n\r\n" });
    out.write_all(head.as_bytes())?;
    if !head_only && status.0 != 204 && status.0 != 304 {
        match len {
            // exactly the length announced: an answer that comes up short breaks the connection
            Some(n) => {
                let sent = io::copy(&mut (&mut body).take(n as u64), out)?;
                if sent != n as u64 {
                    return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "the answer ended before its length"));
                }
            }
            None => {
                io::copy(&mut body, out)?;
            }
        }
    }
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serve_with(limits: Limits, f: impl Fn(&mut Request) -> Resp + Send + Sync + 'static) -> Server {
        Server::bind("127.0.0.1:0", limits).unwrap().serve(Arc::new(f)).unwrap()
    }

    fn text(code: u16, body: &str) -> Resp {
        let len = body.len();
        Response::new(StatusCode(code), vec![], Box::new(io::Cursor::new(body.as_bytes().to_vec())), Some(len), None)
    }

    fn fast() -> Limits {
        Limits { head: Duration::from_millis(400), body: Duration::from_millis(400), idle: Duration::from_millis(600), ..Limits::default() }
    }

    /// Send raw bytes, read everything until the server closes (or `wait` passes).
    fn raw(addr: SocketAddr, send: &[u8], wait: Duration) -> String {
        let mut c = TcpStream::connect(addr).unwrap();
        c.set_read_timeout(Some(wait)).unwrap();
        // (a server that turns us away may close before we are done writing)
        let _ = c.write_all(send);
        let mut got = Vec::new();
        let _ = c.read_to_end(&mut got);
        String::from_utf8_lossy(&got).to_string()
    }

    #[test]
    fn answers_requests_and_keeps_the_connection() {
        let s = serve_with(fast(), |r| {
            let mut body = Vec::new();
            let _ = r.as_reader().read_to_end(&mut body);
            text(200, &format!("{} {} {}", r.method(), r.url(), String::from_utf8_lossy(&body)))
        });
        let two = b"GET /a?b=1 HTTP/1.1\r\nHost: x\r\n\r\nPUT /c HTTP/1.1\r\nHost: x\r\nContent-Length: 3\r\nConnection: close\r\n\r\nabc";
        let got = raw(s.addr(), two, Duration::from_secs(5));
        assert!(got.contains("GET /a?b=1 ") && got.contains("PUT /c abc"), "{got}");
        assert_eq!(got.matches("HTTP/1.1 200 OK").count(), 2, "{got}");
        assert!(got.contains("Connection: keep-alive") && got.contains("Connection: close"), "{got}");
    }

    #[test]
    fn a_head_request_gets_no_body() {
        let s = serve_with(fast(), |_| text(200, "hello"));
        let got = raw(s.addr(), b"HEAD / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n", Duration::from_secs(5));
        assert!(got.contains("Content-Length: 5") && !got.contains("hello"), "{got}");
    }

    #[test]
    fn hostile_requests_are_refused_not_crashes() {
        let s = serve_with(fast(), |_| text(200, "ok"));
        let a = s.addr();
        let t = Duration::from_secs(5);
        assert!(raw(a, b"\x00\x01garbage\r\n\r\n", t).starts_with("HTTP/1.1 400"));
        assert!(raw(a, b"GET / HTTP/1.1\r\nContent-Length: -1\r\n\r\n", t).starts_with("HTTP/1.1 400"));
        assert!(raw(a, b"POST / HTTP/1.1\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\n", t).starts_with("HTTP/1.1 400"));
        assert!(raw(a, b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n", t).starts_with("HTTP/1.1 411"));
        // (a long URL is fine up to the size of a request's head, the API cuts what it reads)
        let long = format!("GET /{} HTTP/1.1\r\nConnection: close\r\n\r\n", "a".repeat(20_000));
        assert!(raw(a, long.as_bytes(), t).starts_with("HTTP/1.1 200"));
        let longer = format!("GET /{} HTTP/1.1\r\n\r\n", "a".repeat(40_000));
        assert!(raw(a, longer.as_bytes(), t).starts_with("HTTP/1.1 431"));
        let many = format!("GET / HTTP/1.1\r\n{}\r\n", "X-A: b\r\n".repeat(100));
        assert!(raw(a, many.as_bytes(), t).starts_with("HTTP/1.1 431"));
        let big = format!("GET / HTTP/1.1\r\nX-A: {}\r\n\r\n", "b".repeat(40_000));
        assert!(raw(a, big.as_bytes(), t).starts_with("HTTP/1.1 431"));
        // still serving
        assert!(raw(a, b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n", t).contains("ok"));
    }

    #[test]
    fn a_head_that_trickles_in_is_cut_off() {
        let s = serve_with(fast(), |_| text(200, "ok"));
        let mut c = TcpStream::connect(s.addr()).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let t0 = Instant::now();
        // a byte every 150 ms never completes the head: the deadline counts from the first byte
        for b in b"GET / HTTP/1.1\r\nX-Slow: yes\r\n" {
            if c.write_all(&[*b]).is_err() {
                break;
            }
            std::thread::sleep(Duration::from_millis(150));
            if t0.elapsed() > Duration::from_secs(3) {
                break;
            }
        }
        let mut got = String::new();
        let _ = c.read_to_string(&mut got);
        assert!(got.starts_with("HTTP/1.1 408"), "{got:?} after {:?}", t0.elapsed());
        assert!(t0.elapsed() < Duration::from_secs(3), "{:?}", t0.elapsed());
    }

    #[test]
    fn an_idle_connection_is_closed() {
        let s = serve_with(fast(), |_| text(200, "ok"));
        let t0 = Instant::now();
        let got = raw(s.addr(), b"", Duration::from_secs(5));
        assert_eq!(got, "");
        assert!(t0.elapsed() < Duration::from_secs(3), "{:?}", t0.elapsed());
    }

    #[test]
    fn a_body_that_stops_midway_is_an_error_for_the_handler() {
        let seen = Arc::new(Mutex::new(None));
        let s2 = seen.clone();
        let s = serve_with(fast(), move |r| {
            let mut body = Vec::new();
            let e = r.as_reader().read_to_end(&mut body).map_err(|e| e.kind());
            *s2.lock().unwrap() = Some((body.len(), e));
            text(400, "short")
        });
        let _ = raw(s.addr(), b"PUT / HTTP/1.1\r\nContent-Length: 100\r\n\r\n0123456789", Duration::from_millis(1500));
        let got = *seen.lock().unwrap();
        assert_eq!(got, Some((10, Err(io::ErrorKind::TimedOut))), "the body paused: that is a timeout, not an end");
    }

    #[test]
    fn busy_servers_still_answer_health_and_turn_away_the_rest() {
        let gate = Arc::new(AtomicBool::new(false));
        let g = gate.clone();
        let limits = Limits { active: 2, ..fast() };
        let s = serve_with(limits, move |r| {
            if r.url() == "/api/health" {
                return text(200, "healthy");
            }
            while !g.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(10));
            }
            text(200, "done")
        });
        let a = s.addr();
        let held: Vec<_> =
            (0..2).map(|_| std::thread::spawn(move || raw(a, b"GET /slow HTTP/1.1\r\nConnection: close\r\n\r\n", Duration::from_secs(5)))).collect();
        std::thread::sleep(Duration::from_millis(300));
        let t = Duration::from_secs(5);
        assert!(raw(a, b"GET /third HTTP/1.1\r\nConnection: close\r\n\r\n", t).starts_with("HTTP/1.1 503"));
        assert!(raw(a, b"GET /api/health HTTP/1.1\r\nConnection: close\r\n\r\n", t).contains("healthy"));
        gate.store(true, Ordering::SeqCst);
        for h in held {
            assert!(h.join().unwrap().contains("done"));
        }
    }

    #[test]
    fn connections_are_bounded() {
        let limits = Limits { connections: 3, per_address: 2, idle: Duration::from_secs(5), ..fast() };
        let s = serve_with(limits, |_| text(200, "ok"));
        let a = s.addr();
        let first: Vec<_> = (0..2).map(|_| TcpStream::connect(a).unwrap()).collect();
        std::thread::sleep(Duration::from_millis(200));
        // the same address has its two; a third is turned away
        assert!(raw(a, b"GET / HTTP/1.1\r\n\r\n", Duration::from_secs(3)).starts_with("HTTP/1.1 503"));
        drop(first);
        std::thread::sleep(Duration::from_millis(600));
        assert!(raw(a, b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n", Duration::from_secs(3)).contains("ok"));
    }

    #[test]
    fn dropping_the_server_closes_waiting_connections() {
        let s = serve_with(Limits { idle: Duration::from_secs(60), ..fast() }, |_| text(200, "ok"));
        let _c = TcpStream::connect(s.addr()).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        let t0 = Instant::now();
        drop(s);
        assert!(t0.elapsed() < Duration::from_secs(3), "{:?}", t0.elapsed());
    }

    #[test]
    fn headers_must_be_wire_safe() {
        assert!(Header::from_bytes(b"X-A", b"b").is_ok());
        assert!(Header::from_bytes(b"X A", b"b").is_err());
        assert!(Header::from_bytes(b"X-A", b"b\r\nX-B: c").is_err());
        assert!(Header::from_bytes(b"", b"b").is_err());
    }
}
