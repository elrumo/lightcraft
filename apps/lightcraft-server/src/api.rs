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

use lightcraft_catalog::sync::{Pull, PushError, ServerCore, proto};
use lightcraft_catalog::{FsStore, LibraryLock};
use serde_json::{Value, json};
use tiny_http::{Header, Method, Request, Response, StatusCode};

use crate::State;
use crate::accounts::{self, LoginError, Session};

/// Largest JSON body (a push is at most a few hundred ops).
pub const JSON_MAX: u64 = 64 << 20;
/// Largest photo file: originals (raw files, videos) and previews.
pub const BLOB_MAX: u64 = 16 << 30;
const PREVIEW_MAX: u64 = 256 << 20;
/// Ops per pull at most.
const PULL_MAX: usize = 5000;
/// Smart and mini previews start with this.
const PREVIEW_MAGIC: &[u8] = b"LCSP1\n";

pub type Resp = Response<Box<dyn Read + Send>>;

fn header(k: &str, v: &str) -> Option<Header> {
    Header::from_bytes(k.as_bytes(), v.as_bytes()).ok()
}

fn bytes(status: u16, ctype: &str, body: Vec<u8>) -> Resp {
    let len = body.len();
    Response::new(StatusCode(status), header("Content-Type", ctype).into_iter().collect(), Box::new(std::io::Cursor::new(body)), Some(len), None)
}

fn json(status: u16, v: &Value) -> Resp {
    bytes(status, "application/json", v.to_string().into_bytes())
}

fn error(status: u16, msg: impl std::fmt::Display) -> Resp {
    json(status, &json!({"error": msg.to_string()}))
}

/// A user's library on this server: the op log, and the presets document.
pub struct UserLib {
    core: ServerCore,
    presets: proto::Presets,
    dir: PathBuf,
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
        Ok(UserLib { core, presets, dir, _lock: lock })
    }
}

fn lib(st: &State, user: &str) -> Result<Arc<Mutex<UserLib>>, String> {
    let mut libs = st.libs.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(l) = libs.get(user) {
        return Ok(l.clone());
    }
    let l = Arc::new(Mutex::new(UserLib::open(&st.data, user)?));
    libs.insert(user.to_string(), l.clone());
    Ok(l)
}

/// Read a body of at most `max` bytes.
fn read_body(req: &mut Request, max: u64) -> Result<Vec<u8>, Resp> {
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

fn read_json<T: serde::de::DeserializeOwned>(req: &mut Request) -> Result<T, Resp> {
    let b = read_body(req, JSON_MAX)?;
    serde_json::from_slice(&b).map_err(|e| error(400, format!("not the JSON this route takes: {e}")))
}

fn header_value<'a>(req: &'a Request, name: &'static str) -> Option<&'a str> {
    req.headers().iter().find(|h| h.field.equiv(name)).map(|h| h.value.as_str())
}

fn query<'a>(q: &'a str, key: &str) -> Option<&'a str> {
    q.split('&').filter_map(|kv| kv.split_once('=')).find(|(k, _)| *k == key).map(|(_, v)| v)
}

/// Answer one request.
pub fn handle(st: &State, mut req: Request) {
    let url = req.url().to_string();
    let (path, q) = url.split_once('?').unwrap_or((url.as_str(), ""));
    let method = req.method().clone();
    let resp = if path == "/api" || path.starts_with("/api/") { api(st, &mut req, &method, path, q) } else { web(st, &req, &method, path) };
    if let Err(e) = req.respond(resp) {
        log::debug!("{method} {path}: {e}");
    }
}

fn api(st: &State, req: &mut Request, method: &Method, path: &str, q: &str) -> Resp {
    if (method, path) == (&Method::Post, "/api/login") {
        return login(st, req);
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
        (Method::Get, ["ops"]) => ops(&l, q),
        (Method::Post, ["ops"]) => push(req, &l, &who),
        (Method::Get, ["presets"]) => {
            let l = l.lock().unwrap_or_else(PoisonError::into_inner);
            json(200, &serde_json::to_value(&l.presets).unwrap_or(Value::Null))
        }
        (Method::Put, ["presets"]) => put_presets(req, &l),
        (m, ["blobs", kind, hash]) => {
            let dir = l.lock().unwrap_or_else(PoisonError::into_inner).dir.join("blobs");
            blob(req, m, &dir, kind, hash)
        }
        _ => error(404, format!("no route {method} {path}")),
    }
}

fn login(st: &State, req: &mut Request) -> Resp {
    let l: proto::Login = match read_json(req) {
        Ok(l) => l,
        Err(r) => return r,
    };
    let r = st.accounts.lock().unwrap_or_else(PoisonError::into_inner).login(&l.user, &l.password, &l.device);
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
fn blob_path(dir: &Path, kind: &str, hash: &str) -> Option<PathBuf> {
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

fn blob(req: &mut Request, method: &Method, dir: &Path, kind: &str, hash: &str) -> Resp {
    let Some(path) = blob_path(dir, kind, hash) else {
        return error(400, "not a photo file this server keeps (/api/blobs/original|smart|mini/<32 hex digits>)");
    };
    match method {
        Method::Head | Method::Get => {
            let Ok(mut f) = std::fs::File::open(&path) else { return error(404, "the server doesn't have this file yet") };
            let len = f.metadata().map(|m| m.len()).unwrap_or(0);
            let mut headers: Vec<Header> =
                [header("Content-Type", "application/octet-stream"), header("Accept-Ranges", "bytes")].into_iter().flatten().collect();
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
        Method::Put => put_blob(req, dir, kind, &path),
        _ => error(405, "HEAD, GET or PUT"),
    }
}

fn put_blob(req: &mut Request, dir: &Path, kind: &str, path: &Path) -> Resp {
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
        Ok(()) => json(200, &json!({})),
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
