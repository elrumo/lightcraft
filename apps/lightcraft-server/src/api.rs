//! The HTTP API (`/api/…`, JSON; see `lightcraft_catalog::sync::proto` and `docs/sync.md`) and
//! the web build at `/`.
//!
//! Every route but `POST /api/login` wants `Authorization: Bearer <token>`. Bodies are capped
//! ([`JSON_MAX`], [`BLOB_MAX`]); content hashes are parsed before they're used in a path; an
//! uploaded original is hashed while it streams to a temporary file and only kept when the hash is
//! the one its URL names. Nothing here panics on what a client sends.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use lightcraft_catalog::sync::{Pull, PushError, ServerCore, proto};
use lightcraft_catalog::{FsStore, LibraryLock};
use serde_json::{Value, json};
use tiny_http::{Header, Method, Request, Response, StatusCode};

use crate::State;
use crate::accounts::{self, LoginError, Session};

/// Largest JSON body (a push is at most a few hundred ops).
pub const JSON_MAX: u64 = 64 << 20;
/// Largest body of the routes anyone can call (signing in, setting up): they are small.
pub const SMALL_JSON_MAX: u64 = 64 << 10;
/// Largest photo file: originals (raw files, videos) and previews.
pub const BLOB_MAX: u64 = 16 << 30;
const PREVIEW_MAX: u64 = 256 << 20;
/// Ops per pull at most.
const PULL_MAX: usize = 5000;
/// A storage answer is reused this long: it walks every photo file the user has.
const USAGE_TTL: Duration = Duration::from_secs(5);
/// Smart and mini previews start with this.
const PREVIEW_MAGIC: &[u8] = b"LCSP1\n";

pub type Resp = Response<Box<dyn Read + Send>>;

pub(crate) fn header(k: &str, v: &str) -> Option<Header> {
    Header::from_bytes(k.as_bytes(), v.as_bytes()).ok()
}

pub(crate) fn bytes(status: u16, ctype: &str, body: Vec<u8>) -> Resp {
    let len = body.len();
    Response::new(StatusCode(status), header("Content-Type", ctype).into_iter().collect(), Box::new(std::io::Cursor::new(body)), Some(len), None)
}

pub(crate) fn json(status: u16, v: &Value) -> Resp {
    bytes(status, "application/json", v.to_string().into_bytes())
}

/// A JSON document that is already text.
pub(crate) fn raw_json(status: u16, body: String) -> Resp {
    bytes(status, "application/json", body.into_bytes())
}

pub(crate) fn error(status: u16, msg: impl std::fmt::Display) -> Resp {
    json(status, &json!({"error": msg.to_string()}))
}

/// A user's library on this server: the op log, the presets document, and the index of their
/// library folders.
pub struct UserLib {
    pub(crate) core: ServerCore,
    presets: proto::Presets,
    /// The user's folder.
    pub(crate) dir: PathBuf,
    pub(crate) index: crate::folders::Index,
    /// The last `GET /api/usage` answer and when it was made.
    usage: Option<(Instant, proto::Usage)>,
    _lock: LibraryLock,
}

impl UserLib {
    fn open(data: &Path, user: &str) -> Result<UserLib, String> {
        let dir = accounts::user_dir(data, user);
        let lib = dir.join("library");
        std::fs::create_dir_all(&lib).map_err(|e| format!("{}: {e}", lib.display()))?;
        let lock = LibraryLock::acquire(&lib, "lightcraft-server").map_err(|e| e.to_string())?;
        let store = FsStore::open(&lib).map_err(|e| format!("{}: {e}", lib.display()))?;
        let core = ServerCore::open(Box::new(store)).map_err(|e| format!("{user}'s library: {e}"))?;
        let presets = match std::fs::read(dir.join("presets.json")) {
            Ok(b) => serde_json::from_slice(&b).map_err(|e| format!("{user}'s presets.json is damaged: {e}"))?,
            Err(_) => proto::Presets { version: 0, presets: json!([]) },
        };
        let index = crate::folders::Index::load(&dir);
        Ok(UserLib { core, presets, dir, index, usage: None, _lock: lock })
    }

    /// (photos, albums) in the library.
    pub(crate) fn counts(&self) -> (usize, usize) {
        let c = self.core.catalog();
        (c.len(), c.albums().count())
    }

    /// Open a user's library (the command line, while the server isn't running).
    pub fn open_for(data: &Path, user: &str) -> Result<UserLib, String> {
        UserLib::open(data, user)
    }
}

pub(crate) fn lib(st: &State, user: &str) -> Result<Arc<Mutex<UserLib>>, String> {
    let mut libs = st.libs.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(l) = libs.get(user) {
        return Ok(l.clone());
    }
    let l = Arc::new(Mutex::new(UserLib::open(&st.data, user)?));
    libs.insert(user.to_string(), l.clone());
    Ok(l)
}

/// Read a body of at most `max` bytes.
pub(crate) fn read_body(req: &mut Request, max: u64) -> Result<Vec<u8>, Resp> {
    if req.body_length().is_some_and(|n| n as u64 > max) {
        return Err(error(413, format!("the request is larger than {max} bytes")));
    }
    let mut buf = Vec::new();
    match req.as_reader().take(max + 1).read_to_end(&mut buf) {
        Ok(n) if n as u64 > max => Err(error(413, format!("the request is larger than {max} bytes"))),
        Ok(_) => Ok(buf),
        Err(e) => Err(error(400, format!("reading the request: {e}"))),
    }
}

pub(crate) fn read_json<T: serde::de::DeserializeOwned>(req: &mut Request) -> Result<T, Resp> {
    read_json_max(req, JSON_MAX)
}

/// [`read_json`] for the routes that need no sign-in.
pub(crate) fn read_small_json<T: serde::de::DeserializeOwned>(req: &mut Request) -> Result<T, Resp> {
    read_json_max(req, SMALL_JSON_MAX)
}

fn read_json_max<T: serde::de::DeserializeOwned>(req: &mut Request, max: u64) -> Result<T, Resp> {
    let b = read_body(req, max)?;
    serde_json::from_slice(&b).map_err(|e| error(400, format!("not the JSON this route takes: {e}")))
}

pub(crate) fn header_value<'a>(req: &'a Request, name: &'static str) -> Option<&'a str> {
    req.headers().iter().find(|h| h.field.equiv(name)).map(|h| h.value.as_str())
}

pub(crate) fn query<'a>(q: &'a str, key: &str) -> Option<&'a str> {
    q.split('&').filter_map(|kv| kv.split_once('=')).find(|(k, _)| *k == key).map(|(_, v)| v)
}

/// Answer one request.
pub fn handle(st: &State, mut req: Request) {
    let url = req.url().to_string();
    let (path, q) = url.split_once('?').unwrap_or((url.as_str(), ""));
    let method = req.method().clone();
    let is_api = path == "/api" || path.starts_with("/api/");
    let mut resp = if let Some(rest) = path.strip_prefix("/api/admin/") {
        crate::admin::api(st, &mut req, &method, rest)
    } else if is_api {
        api(st, &mut req, &method, path, q)
    } else if path == "/admin" || path.starts_with("/admin/") {
        crate::admin::page(&method, path)
    } else {
        web(st, &req, &method, path)
    };
    // what every answer says, unless it says otherwise: don't guess types, don't leak the URL,
    // and (the API) don't keep answers that hold a user's library
    let has = |r: &Resp, k: &'static str| r.headers().iter().any(|h| h.field.equiv(k));
    for (k, v, when) in [("X-Content-Type-Options", "nosniff", true), ("Referrer-Policy", "no-referrer", true), ("Cache-Control", "no-store", is_api)]
    {
        if when
            && !has(&resp, k)
            && let Some(h) = header(k, v)
        {
            resp.add_header(h);
        }
    }
    if let Err(e) = req.respond(resp) {
        log::debug!("{method} {path}: {e}");
    }
}

fn api(st: &State, req: &mut Request, method: &Method, path: &str, q: &str) -> Resp {
    if (method, path) == (&Method::Post, "/api/login") {
        return login(st, req);
    }
    if path == "/api/health" {
        return json(200, &json!({"ok": true, "version": env!("CARGO_PKG_VERSION")}));
    }
    let token = header_value(req, "Authorization").and_then(|v| v.strip_prefix("Bearer ")).unwrap_or("").trim().to_string();
    let Some(who) = st.accounts.lock().unwrap_or_else(PoisonError::into_inner).check(&token) else {
        return error(401, "sign in first");
    };
    let l = match lib(st, &who.user) {
        Ok(l) => l,
        Err(e) => {
            log::error!("{e}");
            return error(500, e);
        }
    };
    let parts: Vec<&str> = path.trim_start_matches("/api/").split('/').collect();
    match (method, parts.as_slice()) {
        (Method::Post, ["logout"]) => match st.accounts.lock().unwrap_or_else(PoisonError::into_inner).logout(&who) {
            Ok(()) => json(200, &json!({})),
            Err(e) => error(500, e),
        },
        (Method::Get, ["me"]) => me(st, &who),
        (Method::Get, ["snapshot"]) => snapshot(st, &l, &who),
        (Method::Get, ["usage"]) => usage(st, &l, &who),
        (Method::Get, ["ops"]) => ops(&l, q),
        (Method::Post, ["ops"]) => push(req, &l, &who),
        (Method::Get, ["presets"]) => {
            let l = l.lock().unwrap_or_else(PoisonError::into_inner);
            json(200, &serde_json::to_value(&l.presets).unwrap_or(Value::Null))
        }
        (Method::Put, ["presets"]) => put_presets(req, &l),
        (m, ["blobs", kind, hash]) => blob(st, req, m, &l, &who.user, kind, hash),
        // search by description (see `vision`)
        (Method::Get, ["search", "status"]) => crate::vision::status(st, &l, &who.user),
        (Method::Get, ["search"]) => crate::vision::search(st, &l, &who.user, q),
        (Method::Get, ["index", "embeddings", "keys"]) => crate::vision::keys(st, &who.user),
        (Method::Post, ["index", "embeddings"]) => crate::vision::upload(st, req, &l, &who.user),
        (Method::Get, ["index", "text", "keys"]) => crate::vision::text_keys(st, &who.user),
        (Method::Post, ["index", "text"]) => crate::vision::text_upload(st, req, &l, &who.user),
        (Method::Get, ["people", "status"]) => crate::vision::people_status(st, &l, &who.user),
        (Method::Get, ["people", "clusters"]) => crate::vision::people_clusters(st, &l, &who.user),
        (Method::Get, ["index", "faces", "keys"]) => crate::vision::face_keys(st, &who.user),
        (Method::Post, ["index", "faces"]) => crate::vision::face_upload(st, req, &l, &who.user),
        (Method::Delete, ["index", "faces"]) => crate::vision::face_delete(st, &who.user),
        (Method::Post, ["render"]) => render(st, req, &l, &who.user),
        _ => error(404, format!("no route {method} {path}")),
    }
}

/// The throttle's keys for a sign-in: the client's address and the user name.
pub(crate) fn throttle_keys(req: &Request, user: &str) -> Vec<String> {
    let ip = crate::throttle::client(req.remote_addr().copied(), header_value(req, "X-Forwarded-For"));
    vec![format!("ip:{ip}"), format!("user:{}", user.chars().take(64).collect::<String>().to_lowercase())]
}

/// `429` while these sign-in keys must wait.
pub(crate) fn throttled(st: &State, keys: &[String]) -> Option<Resp> {
    let wait = st.throttle.lock().unwrap_or_else(PoisonError::into_inner).wait(keys)?;
    let secs = wait.as_secs().max(1);
    let mut r = error(429, format!("too many wrong passwords: try again in {secs} s"));
    if let Some(h) = header("Retry-After", &secs.to_string()) {
        r.add_header(h);
    }
    Some(r)
}

fn login(st: &State, req: &mut Request) -> Resp {
    let l: proto::Login = match read_small_json(req) {
        Ok(l) => l,
        Err(r) => return r,
    };
    let keys = throttle_keys(req, &l.user);
    if let Some(r) = throttled(st, &keys) {
        return r;
    }
    let r = st.accounts.lock().unwrap_or_else(PoisonError::into_inner).login(&l.user, &l.password, &l.device);
    let mut t = st.throttle.lock().unwrap_or_else(PoisonError::into_inner);
    if matches!(r, Err(LoginError::Refused)) {
        t.failed(&keys)
    } else {
        t.succeeded(&keys)
    }
    drop(t);
    match r {
        Ok((token, d, library)) => {
            if let Err(e) = lib(st, &l.user) {
                log::error!("{e}");
                return error(500, e);
            }
            log::info!("{}: signed in device {} ({}), id space {}", l.user, d.id, d.name, d.space);
            json(200, &json!(proto::Device { token, device: d.id, space: d.space, library }))
        }
        Err(LoginError::Refused) => {
            log::warn!("refused a sign-in as `{}`", l.user.chars().take(64).collect::<String>());
            // slow down password guessing
            std::thread::sleep(std::time::Duration::from_millis(500));
            error(401, "wrong user name or password")
        }
        Err(LoginError::Failed(e)) => {
            log::error!("sign-in: {e}");
            error(500, e)
        }
    }
}

fn me(st: &State, who: &Session) -> Resp {
    let mut a = st.accounts.lock().unwrap_or_else(PoisonError::into_inner);
    let devices: Vec<Value> =
        a.devices_of(&who.user).iter().map(|d| json!({"id": d.id, "name": d.name, "space": d.space, "created": d.created})).collect();
    json(200, &json!({"user": who.user, "device": who.device, "library": a.library_of(&who.user).unwrap_or_default(), "devices": devices}))
}

/// What the user's library takes here: their photo files by kind, the photos in their library
/// folders (read where they are, not stored), and how much room the disk has left.
fn usage(st: &State, l: &Mutex<UserLib>, who: &Session) -> Resp {
    let (dir, mut u) = {
        let l = l.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((at, u)) = &l.usage
            && at.elapsed() < USAGE_TTL
        {
            return json(200, &json!(u));
        }
        let (photos, albums) = l.counts();
        let mut folders = proto::Files::default();
        for e in l.index.files.values().filter(|e| !e.missing) {
            folders.files += 1;
            folders.bytes = folders.bytes.saturating_add(e.size);
        }
        (l.dir.clone(), proto::Usage { photos: photos as u64, albums: albums as u64, folders, ..Default::default() })
    };
    // (the folders are walked without the library locked: pushes and pulls go on meanwhile)
    let blobs = dir.join("blobs");
    u.original = blob_usage(&blobs, "original");
    u.smart = blob_usage(&blobs, "smart");
    u.mini = blob_usage(&blobs, "mini");
    u.disk = lightcraft_engine::usage::disk_space(&st.data);
    u.devices = st.accounts.lock().unwrap_or_else(PoisonError::into_inner).devices_of(&who.user).len() as u64;
    l.lock().unwrap_or_else(PoisonError::into_inner).usage = Some((Instant::now(), u.clone()));
    json(200, &json!(u))
}

/// The files of one kind under `blobs/<kind>/<xx>/<hash>`.
fn blob_usage(blobs: &Path, kind: &str) -> proto::Files {
    let mut out = proto::Files::default();
    let Ok(prefixes) = std::fs::read_dir(blobs.join(kind)) else { return out };
    for prefix in prefixes.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())) {
        let Ok(files) = std::fs::read_dir(prefix.path()) else { continue };
        for f in files.flatten() {
            if let Ok(m) = f.metadata()
                && m.is_file()
            {
                out.files += 1;
                out.bytes = out.bytes.saturating_add(m.len());
            }
        }
    }
    out
}

fn snapshot(st: &State, l: &Mutex<UserLib>, who: &Session) -> Resp {
    let library = match st.accounts.lock().unwrap_or_else(PoisonError::into_inner).library_of(&who.user) {
        Ok(lib) => lib,
        Err(e) => return error(500, e),
    };
    let (seq, catalog) = l.lock().unwrap_or_else(PoisonError::into_inner).core.snapshot();
    let lib = Value::String(library).to_string();
    bytes(200, "application/json", format!("{{\"library\":{lib},\"seq\":{seq},\"catalog\":{catalog}}}").into_bytes())
}

fn ops(l: &Mutex<UserLib>, q: &str) -> Resp {
    let since = query(q, "since").and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
    let limit = query(q, "limit").and_then(|v| v.parse::<usize>().ok()).unwrap_or(1000).clamp(1, PULL_MAX);
    let mut l = l.lock().unwrap_or_else(PoisonError::into_inner);
    let presets = l.presets.version;
    match l.core.since(since, limit) {
        Ok(Pull::Ops(ops)) => json(200, &json!(proto::Ops { head: l.core.head(), ops, presets })),
        Ok(Pull::Gone) => error(410, "that is older than the server keeps: reload the library"),
        Err(e) => {
            log::error!("reading the log: {e}");
            error(500, e)
        }
    }
}

fn push(req: &mut Request, l: &Mutex<UserLib>, who: &Session) -> Resp {
    let p: proto::Push = match read_json(req) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let mut l = l.lock().unwrap_or_else(PoisonError::into_inner);
    match l.core.push(p.base, &p.ops) {
        Ok(head) => {
            log::debug!("{} device {}: {} op(s), head {head}", who.user, who.device, p.ops.len());
            json(200, &json!(proto::Head { head }))
        }
        Err(PushError::Behind { head }) => json(409, &json!(proto::Head { head })),
        Err(PushError::Rejected { index, error }) => {
            log::info!("{} device {}: refused op {index}: {error}", who.user, who.device);
            json(422, &json!(proto::Refused { index, error }))
        }
        Err(PushError::Storage(e)) => {
            log::error!("{}: writing the log: {e}", who.user);
            error(500, e)
        }
    }
}

fn put_presets(req: &mut Request, l: &Mutex<UserLib>) -> Resp {
    let p: proto::Presets = match read_json(req) {
        Ok(p) => p,
        Err(r) => return r,
    };
    if !p.presets.is_array() {
        return error(400, "`presets` must be an array");
    }
    let mut l = l.lock().unwrap_or_else(PoisonError::into_inner);
    if p.version != l.presets.version {
        // another device changed them first: get them, merge, put again
        return json(412, &serde_json::to_value(&l.presets).unwrap_or(Value::Null));
    }
    let next = proto::Presets { version: l.presets.version + 1, presets: p.presets };
    let body = serde_json::to_vec(&next).unwrap_or_default();
    if let Err(e) = lightcraft_catalog::safe_file::write_atomic(&l.dir.join("presets.json"), &body) {
        return error(500, format!("presets.json: {e}"));
    }
    l.presets = next;
    json(200, &json!({"version": l.presets.version}))
}

/// Where a photo file is kept: `blobs/<kind>/<first two digits>/<hash>`.
pub(crate) fn blob_path(dir: &Path, kind: &str, hash: &str) -> Option<PathBuf> {
    if !proto::BLOB_KINDS.contains(&kind) {
        return None;
    }
    let h = lightcraft_preview::Hash128::parse(hash)?.to_string();
    Some(dir.join(kind).join(h.get(..2)?).join(h))
}

/// `Range: bytes=a-b` / `bytes=a-` / `bytes=-n` → (start, end inclusive); `None`: not a range
/// this server serves (the whole file is sent).
fn range(v: &str, len: u64) -> Option<Result<(u64, u64), ()>> {
    let spec = v.trim().strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None;
    }
    let (a, b) = spec.split_once('-')?;
    let r = match (a.trim().parse::<u64>().ok(), b.trim().parse::<u64>().ok()) {
        (Some(a), Some(b)) if a <= b && a < len => Ok((a, b.min(len.saturating_sub(1)))),
        (Some(a), None) if b.trim().is_empty() && a < len => Ok((a, len - 1)),
        (None, Some(n)) if a.trim().is_empty() && n > 0 && len > 0 => Ok((len.saturating_sub(n), len - 1)),
        _ => Err(()),
    };
    Some(r)
}

/// An original kept in one of the user's library folders (still the file that was indexed).
fn folder_original(st: &State, l: &Mutex<UserLib>, user: &str, hash: &str) -> Option<std::fs::File> {
    folder_original_path(st, l, user, hash).and_then(|p| std::fs::File::open(p).ok())
}

/// Where a library folder keeps the original with this content, when it is there, unchanged and readable.
fn folder_original_path(st: &State, l: &Mutex<UserLib>, user: &str, hash: &str) -> Option<PathBuf> {
    let found = l.lock().unwrap_or_else(PoisonError::into_inner).index.originals(hash);
    for (path, size, mtime) in &found {
        if crate::folders::unchanged(path, *size, *mtime) && std::fs::File::open(path).is_ok() {
            return Some(path.clone());
        }
    }
    if !found.is_empty() {
        // changed or gone since the last scan: look again
        st.folders.request(user);
    }
    None
}

fn blob(st: &State, req: &mut Request, method: &Method, l: &Mutex<UserLib>, user: &str, kind: &str, hash: &str) -> Resp {
    let dir = l.lock().unwrap_or_else(PoisonError::into_inner).dir.join("blobs");
    let Some(path) = blob_path(&dir, kind, hash) else {
        return error(400, "not a photo file this server keeps (/api/blobs/original|smart|mini/<32 hex digits>)");
    };
    let hash = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    match method {
        Method::Head | Method::Get => {
            let f = match std::fs::File::open(&path) {
                Ok(f) => Some(f),
                Err(_) if kind == "original" => folder_original(st, l, user, &hash),
                Err(_) => None,
            };
            let Some(mut f) = f else { return error(404, "the server doesn't have this file yet") };
            let len = f.metadata().map(|m| m.len()).unwrap_or(0);
            // by content: never changes
            let mut headers: Vec<Header> = [
                header("Content-Type", "application/octet-stream"),
                header("Accept-Ranges", "bytes"),
                header("Cache-Control", "private, max-age=31536000, immutable"),
            ]
            .into_iter()
            .flatten()
            .collect();
            let (status, start, n) = match header_value(req, "Range").and_then(|v| range(v, len)) {
                Some(Ok((a, b))) => {
                    headers.extend(header("Content-Range", &format!("bytes {a}-{b}/{len}")));
                    (206, a, b - a + 1)
                }
                Some(Err(())) => {
                    headers.extend(header("Content-Range", &format!("bytes */{len}")));
                    return Response::new(StatusCode(416), headers, Box::new(std::io::empty()), Some(0), None);
                }
                None => (200, 0, len),
            };
            if start > 0 && f.seek(SeekFrom::Start(start)).is_err() {
                return error(500, "can't read the file");
            }
            let size = usize::try_from(n).ok();
            Response::new(StatusCode(status), headers, Box::new(f.take(n)), size, None)
        }
        Method::Put => {
            // `?previews=1` on an original: the device asks the server to build its previews
            let previews = kind == "original" && req.url().split_once('?').is_some_and(|(_, q)| query(q, "previews") == Some("1"));
            let resp = put_blob(st, req, &dir, kind, &path);
            if previews && resp.status_code().0 == 200 {
                return json(200, &json!({"previews": st.folders.queue(&dir, user, hash, path).word()}));
            }
            resp
        }
        _ => error(405, "HEAD, GET or PUT"),
    }
}

/// How long a render waits for a free place before the device is told to try again.
const RENDER_WAIT: Duration = Duration::from_secs(45);

/// `POST /api/render` ([`proto::Render`]): the photo with this original, rendered with these edits and options.
fn render(st: &State, req: &mut Request, l: &Mutex<UserLib>, user: &str) -> Resp {
    let r: proto::Render = match read_json(req) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let (dir, data) = {
        let l = l.lock().unwrap_or_else(PoisonError::into_inner);
        (l.dir.join("blobs"), st.data.join("tmp"))
    };
    let Some(blob) = blob_path(&dir, "original", &r.hash) else {
        return error(400, "not a content hash (32 hex digits)");
    };
    let Some(original) = Some(blob).filter(|b| b.is_file()).or_else(|| folder_original_path(st, l, user, &r.hash)) else {
        return error(404, "the server doesn't have this photo's original");
    };
    let Some(_slot) = st.render.acquire(RENDER_WAIT) else {
        let mut resp = error(503, "the server is rendering other photos: try again in a minute");
        header("Retry-After", "60").into_iter().for_each(|h| resp.add_header(h));
        return resp;
    };
    match lightcraft_engine::guard::catch("rendering", || crate::render::render_file(&original, &data, &r)) {
        Ok(Ok(out)) => {
            let mut resp = bytes(200, out.content_type, out.bytes);
            header("X-LightCraft-Size", &format!("{}x{}", out.width, out.height)).into_iter().for_each(|h| resp.add_header(h));
            resp
        }
        Ok(Err(e)) | Err(e) => error(422, e),
    }
}

fn put_blob(st: &State, req: &mut Request, dir: &Path, kind: &str, path: &Path) -> Resp {
    let max = if kind == "original" { BLOB_MAX } else { PREVIEW_MAX };
    if req.body_length().is_some_and(|n| n as u64 > max) {
        return error(413, format!("larger than {max} bytes"));
    }
    let tmp_dir = dir.join("tmp");
    let tmp = match accounts::random_hex(8) {
        Ok(r) => tmp_dir.join(format!("{r}.part")),
        Err(e) => return error(500, e),
    };
    let r = (|| -> Result<(), (u16, String)> {
        let io = |e: std::io::Error| (500, e.to_string());
        std::fs::create_dir_all(&tmp_dir).map_err(io)?;
        let mut f = std::fs::File::create(&tmp).map_err(io)?;
        let mut hasher = lightcraft_preview::Hasher128::new();
        let mut buf = vec![0u8; 1 << 20];
        let (mut total, mut head) = (0u64, Vec::<u8>::new());
        let reader = req.as_reader();
        loop {
            let n = match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err((400, format!("reading the upload: {e}"))),
            };
            let chunk = buf.get(..n).unwrap_or(&[]);
            total += n as u64;
            if total > max {
                return Err((413, format!("larger than {max} bytes")));
            }
            if head.len() < PREVIEW_MAGIC.len() {
                head.extend(chunk.iter().take(PREVIEW_MAGIC.len() - head.len()));
            }
            hasher.update(chunk);
            f.write_all(chunk).map_err(io)?;
        }
        // (the path's file name is the hash, normalized)
        if kind == "original" && path.file_name().is_none_or(|n| *n != *hasher.finish().to_string()) {
            return Err((422, "the file isn't the one its hash names".into()));
        }
        if kind != "original" && head != PREVIEW_MAGIC {
            return Err((422, "not a LightCraft preview".into()));
        }
        f.sync_all().map_err(io)?;
        drop(f);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(io)?;
        }
        std::fs::rename(&tmp, path).map_err(io)?;
        if let Some(parent) = path.parent()
            && let Ok(d) = std::fs::File::open(parent)
        {
            let _ = d.sync_all();
        }
        Ok(())
    })();
    match r {
        Ok(()) => {
            if kind == "mini" || kind == "smart" {
                // a photo that can be indexed for search now (its text is read from the smart preview)
                st.vision.wake();
            }
            json(200, &json!({}))
        }
        Err((status, msg)) => {
            let _ = std::fs::remove_file(&tmp);
            error(status, msg)
        }
    }
}

fn mime(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "wasm" => "application/wasm",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "png" => "image/png",
        "svg" => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

/// The web build (`cargo xtask web` → `target/web/`), cross-origin isolated like the dev server,
/// with its precompressed copies when the browser takes them.
fn web(st: &State, req: &Request, method: &Method, path: &str) -> Resp {
    if !matches!(method, Method::Get | Method::Head) {
        return error(405, "GET or HEAD");
    }
    let Some(root) = &st.web else {
        return bytes(200, "text/plain; charset=utf-8", b"LightCraft sync server. Sign in from LightCraft (Settings > Sync).\n".to_vec());
    };
    let rel = path.trim_start_matches('/');
    let rel = if rel.is_empty() { "index.html" } else { rel };
    if rel.split('/').any(|c| c.is_empty() || c == "." || c == ".." || c.contains('\\') || c.contains(':')) {
        return error(404, "not found");
    }
    let accept = header_value(req, "Accept-Encoding").unwrap_or("");
    let accepts = |enc: &str| accept.split(',').any(|e| e.split(';').next().is_some_and(|n| n.trim().eq_ignore_ascii_case(enc)));
    let pick = [("br", ".br"), ("gzip", ".gz")].into_iter().find(|(enc, sfx)| accepts(enc) && root.join(format!("{rel}{sfx}")).is_file());
    let file = match pick {
        Some((_, sfx)) => root.join(format!("{rel}{sfx}")),
        None => root.join(rel),
    };
    let Ok(f) = std::fs::File::open(&file) else { return error(404, "not found") };
    let len = f.metadata().ok().and_then(|m| usize::try_from(m.len()).ok());
    let mut headers: Vec<Header> = [
        header("Content-Type", mime(rel)),
        header("Cache-Control", "no-cache"),
        header("Vary", "Accept-Encoding"),
        header("Cross-Origin-Opener-Policy", "same-origin"),
        header("Cross-Origin-Embedder-Policy", "require-corp"),
        header("Cross-Origin-Resource-Policy", "same-origin"),
        // never inside another site's page
        header("X-Frame-Options", "DENY"),
        header("Content-Security-Policy", "frame-ancestors 'none'"),
    ]
    .into_iter()
    .flatten()
    .collect();
    if let Some((enc, _)) = pick {
        headers.extend(header("Content-Encoding", enc));
    }
    Response::new(StatusCode(200), headers, Box::new(f), len, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_and_paths() {
        assert_eq!(range("bytes=0-9", 100), Some(Ok((0, 9))));
        assert_eq!(range("bytes=90-", 100), Some(Ok((90, 99))));
        assert_eq!(range("bytes=-10", 100), Some(Ok((90, 99))));
        assert_eq!(range("bytes=50-500", 100), Some(Ok((50, 99))));
        assert_eq!(range("bytes=100-", 100), Some(Err(())));
        assert_eq!(range("bytes=9-0", 100), Some(Err(())));
        assert_eq!(range("bytes=-0", 0), Some(Err(())));
        assert_eq!(range("bytes=0-1,5-6", 100), None);
        assert_eq!(range("items=0-1", 100), None);
        let d = Path::new("/d");
        assert!(blob_path(d, "original", "../../etc/passwd").is_none());
        assert!(blob_path(d, "secret", "0123456789abcdef0123456789abcdef").is_none());
        assert_eq!(blob_path(d, "mini", "0123456789ABCDEF0123456789abcdef"), Some(PathBuf::from("/d/mini/01/0123456789abcdef0123456789abcdef")));
        assert_eq!(query("since=5&limit=9", "limit"), Some("9"));
    }
}
