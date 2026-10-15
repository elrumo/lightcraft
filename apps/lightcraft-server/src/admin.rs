//! The admin web page (`/admin`) and its API (`/api/admin/…`): users (add, reset a password,
//! make admin, remove), their devices (sign out), storage, and the server's settings, read-only.
//!
//! - **First run:** while no user is an admin, the server prints a one-time setup code to its log
//!   at start; `/admin` asks for it to create the first admin (so a server just put on the
//!   internet can't be claimed by whoever finds it first). Wrong codes rotate it after a few tries.
//! - **Sessions:** an admin signs in with their user name and password and gets a session token
//!   of its own (idle sessions end, see [`accounts::ADMIN_IDLE`]); it is not a device and can't
//!   sync, and a device token never opens these routes.
//! - **Settings stay deployment settings:** where the data lives, the address the server listens
//!   on and its domain are shown, not edited: moving a library under a running server, or its
//!   address under its clients, is how libraries get lost (see `docs/sync.md`).
//!
//! The page is ours, embedded in the binary (`admin/`), and served with a strict Content
//! Security Policy: no inline script or style, nothing from other origins, no framing.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::http::{Header, Method, Request, Response, StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::Digest;

use crate::State;
use crate::accounts::{self, LoginError};
use crate::api::{Resp, error, header, header_value, json, read_json, read_small_json};
use crate::switch::Part;

const INDEX: &str = include_str!("../admin/index.html");
const SCRIPT: &str = include_str!("../admin/admin.js");
const STYLE: &str = include_str!("../admin/admin.css");
/// The app icon (64 px), for the page's header and tab.
const ICON: &[u8] = include_bytes!("../../../assets/app-icon/hicolor/64x64/apps/ai.storyteller.lightcraft.png");

/// Wrong setup codes before a new one is made.
const SETUP_TRIES: u32 = 10;

/// How long the bytes of a user's photo files are trusted before they are walked again.
const SIZES_TTL: Duration = Duration::from_secs(60);

/// The bytes of the photo files the server keeps for each user (uploaded originals and previews). Counting
/// them stats every file, which takes seconds on a NAS disk, so the admin page is answered from the last
/// count and a new one runs in the background (the first when the server starts). The photos in a user's
/// library folders aren't walked: the index of those knows their sizes.
#[derive(Default)]
pub struct Sizes {
    known: Arc<Mutex<HashMap<String, (Instant, u64)>>>,
    walking: Arc<AtomicBool>,
}

impl Sizes {
    /// The last count of `name`'s bytes (`None`: not counted yet).
    fn get(&self, name: &str) -> Option<u64> {
        self.known.lock().unwrap_or_else(PoisonError::into_inner).get(name).map(|&(_, bytes)| bytes)
    }

    /// What `name` keeps here now: the last count and what arrived since (counted here and now when it never
    /// was: the first upload after a start or a clean-up waits for it).
    pub(crate) fn used(&self, data: &Path, name: &str) -> u64 {
        if let Some(bytes) = self.get(name) {
            return bytes;
        }
        let bytes = size_of(&accounts::user_dir(data, name).join("blobs"), 3);
        self.known.lock().unwrap_or_else(PoisonError::into_inner).entry(name.to_string()).or_insert((Instant::now(), bytes)).1
    }

    /// `bytes` more arrived for `name` (an upload), until the next count.
    pub(crate) fn add(&self, name: &str, bytes: u64) {
        if let Some(e) = self.known.lock().unwrap_or_else(PoisonError::into_inner).get_mut(name) {
            e.1 = e.1.saturating_add(bytes);
        }
    }

    /// Forget the counts (files were removed): the next ones are made in the background.
    fn clear(&self) {
        self.known.lock().unwrap_or_else(PoisonError::into_inner).clear();
    }

    /// Count the users whose bytes are unknown or old, in the background, one walk at a time.
    pub fn refresh(&self, data: &Path) {
        let Ok(f) = accounts::read_users(data) else { return };
        let stale: Vec<String> = {
            let known = self.known.lock().unwrap_or_else(PoisonError::into_inner);
            f.users.keys().filter(|n| known.get(*n).is_none_or(|(at, _)| at.elapsed() > SIZES_TTL)).cloned().collect()
        };
        if stale.is_empty() || self.walking.swap(true, Ordering::AcqRel) {
            return;
        }
        let (known, walking, data) = (self.known.clone(), self.walking.clone(), data.to_path_buf());
        let spawned = std::thread::Builder::new().name("lc-sizes".into()).spawn(move || {
            for name in stale {
                let bytes = size_of(&accounts::user_dir(&data, &name).join("blobs"), 3);
                known.lock().unwrap_or_else(PoisonError::into_inner).insert(name, (Instant::now(), bytes));
            }
            walking.store(false, Ordering::Release);
        });
        if spawned.is_err() {
            self.walking.store(false, Ordering::Release);
        }
    }
}

/// Headers every admin page file gets.
fn page_headers(ctype: &str) -> Vec<Header> {
    [
        ("Content-Type", ctype),
        (
            "Content-Security-Policy",
            "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self' data:; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
        ),
        ("X-Content-Type-Options", "nosniff"),
        ("Referrer-Policy", "no-referrer"),
        ("Cache-Control", "no-cache"),
        ("Cross-Origin-Opener-Policy", "same-origin"),
        ("Cross-Origin-Resource-Policy", "same-origin"),
    ]
    .into_iter()
    .filter_map(|(k, v)| Header::from_bytes(k.as_bytes(), v.as_bytes()).ok())
    .collect()
}

/// A page file as served: the bytes, gzipped, and the tag that says whether the browser's copy is current.
struct Asset {
    ctype: &'static str,
    raw: &'static [u8],
    gz: Vec<u8>,
    etag: String,
}

/// The page's files, by path. Built once: they are in the binary and never change.
fn assets() -> &'static HashMap<&'static str, Asset> {
    static ASSETS: std::sync::OnceLock<HashMap<&'static str, Asset>> = std::sync::OnceLock::new();
    ASSETS.get_or_init(|| {
        let files: [(&str, &str, &'static [u8]); 4] = [
            ("/admin", "text/html; charset=utf-8", INDEX.as_bytes()),
            ("/admin/admin.js", "text/javascript; charset=utf-8", SCRIPT.as_bytes()),
            ("/admin/admin.css", "text/css; charset=utf-8", STYLE.as_bytes()),
            ("/admin/icon.png", "image/png", ICON),
        ];
        files
            .into_iter()
            .map(|(path, ctype, raw)| {
                let mut z = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
                let gz =
                    std::io::Write::write_all(&mut z, raw).ok().and_then(|()| z.finish().ok()).filter(|g| g.len() < raw.len()).unwrap_or_default();
                let etag = format!("\"{}\"", sha2::Sha256::digest(raw).iter().take(8).map(|b| format!("{b:02x}")).collect::<String>());
                (path, Asset { ctype, raw, gz, etag })
            })
            .collect()
    })
}

/// `/admin`, `/admin/admin.js`, `/admin/admin.css`, `/admin/icon.png`: gzipped to browsers that take it, and
/// answered `304` to one that already has them (the browser asks every time, so a new server version shows at once).
pub fn page(req: &Request, method: &Method, path: &str) -> Resp {
    if !matches!(method, Method::Get | Method::Head) {
        return error(405, "GET or HEAD");
    }
    let path = match path.trim_end_matches('/') {
        "/admin/index.html" => "/admin",
        p => p,
    };
    let Some(a) = assets().get(path) else { return error(404, "not found") };
    let mut headers = page_headers(a.ctype);
    headers.extend(header("ETag", &a.etag));
    headers.extend(header("Vary", "Accept-Encoding"));
    if header_value(req, "If-None-Match").is_some_and(|v| v.split(',').any(|t| t.trim() == a.etag)) {
        return Response::new(StatusCode(304), headers, Box::new(std::io::empty()), Some(0), None);
    }
    let gzip = !a.gz.is_empty() && header_value(req, "Accept-Encoding").is_some_and(|v| v.split(',').any(|e| e.trim().starts_with("gzip")));
    let body = if gzip { a.gz.clone() } else { a.raw.to_vec() };
    if gzip {
        headers.extend(header("Content-Encoding", "gzip"));
    }
    let len = body.len();
    Response::new(StatusCode(200), headers, Box::new(std::io::Cursor::new(body)), Some(len), None)
}

/// A new one-time setup code (`ABCD-1234`), when the server has no admin yet.
pub fn new_setup_code() -> Option<String> {
    let hex = accounts::random_hex(4).ok()?.to_ascii_uppercase();
    Some(format!("{}-{}", hex.get(..4)?, hex.get(4..)?))
}

fn token(req: &Request) -> String {
    header_value(req, "Authorization").and_then(|v| v.strip_prefix("Bearer ")).unwrap_or("").trim().to_string()
}

#[derive(Deserialize)]
struct Setup {
    code: String,
    user: String,
    password: String,
}

#[derive(Deserialize)]
struct Login {
    user: String,
    password: String,
}

#[derive(Deserialize)]
struct NewUser {
    name: String,
    password: String,
    #[serde(default)]
    admin: bool,
}

#[derive(Deserialize)]
struct Password {
    password: String,
}

#[derive(Deserialize)]
struct Admin {
    admin: bool,
}

#[derive(Deserialize)]
struct NewFolder {
    path: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    writable: bool,
    #[serde(default)]
    imports: bool,
}

#[derive(Deserialize)]
struct FolderMode {
    #[serde(default)]
    writable: Option<bool>,
    #[serde(default)]
    imports: Option<bool>,
}

#[derive(Deserialize)]
struct Ignore {
    ignore: Vec<String>,
}

#[derive(Deserialize)]
struct Enabled {
    enabled: bool,
}

/// `PUT /api/admin/search/schedule/<search|text|faces>`: pause making that index, and/or limit it to a
/// daily window (minutes of the day in UTC).
#[derive(Deserialize)]
struct Schedule {
    paused: bool,
    from: Option<u32>,
    to: Option<u32>,
}

/// `PUT /api/admin/users/<name>/quota`: `{quota: bytes}`, or `{quota: null}` for no limit.
#[derive(Deserialize)]
struct Quota {
    quota: Option<u64>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Licence {
    acknowledged: bool,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Gc {
    dry_run: bool,
}

/// `/api/admin/…`.
pub fn api(st: &State, req: &mut Request, method: &Method, rest: &str) -> Resp {
    let parts: Vec<&str> = rest.split('/').filter(|p| !p.is_empty()).collect();
    // without a session
    match (method, parts.as_slice()) {
        (Method::Get, ["state"]) => {
            let setup = st.setup.lock().unwrap_or_else(PoisonError::into_inner).is_some() && !accounts::has_admin(&st.data);
            return json(200, &json!({"setup": setup}));
        }
        (Method::Post, ["setup"]) => return setup(st, req),
        (Method::Post, ["login"]) => return login(st, req),
        _ => {}
    }
    let token = token(req);
    let Some(me) = st.accounts.lock().unwrap_or_else(PoisonError::into_inner).check_admin(&token) else {
        return error(401, "sign in as an admin");
    };
    // a user name in the path is one (never `..` or the like)
    if let ["users", name, ..] = parts.as_slice()
        && !accounts::valid_name(name)
    {
        return error(400, format!("not a user name: {name}"));
    }
    let r = match (method, parts.as_slice()) {
        (Method::Post, ["logout"]) => {
            st.accounts.lock().unwrap_or_else(PoisonError::into_inner).admin_logout(&token);
            Ok(json!({}))
        }
        (Method::Get, ["status"]) => Ok(status(st, &me)),
        (Method::Get, ["users"]) => users(st),
        (Method::Post, ["users"]) => read_json::<NewUser>(req).map_err(Err).and_then(|u| add_user(st, u).map_err(Ok)),
        (Method::Put, ["users", name, "password"]) => read_json::<Password>(req)
            .map_err(Err)
            .and_then(|p| accounts::set_user(&st.data, name, &p.password, true).map(|()| json!({})).map_err(|e| Ok((400, e)))),
        (Method::Put, ["users", name, "admin"]) => read_json::<Admin>(req)
            .map_err(Err)
            .and_then(|a| accounts::set_admin(&st.data, name, a.admin).map(|()| json!({})).map_err(|e| Ok((409, e)))),
        (Method::Put, ["users", name, "quota"]) => read_json::<Quota>(req).map_err(Err).and_then(|q| {
            let code = if accounts::read_users(&st.data).is_ok_and(|f| f.users.contains_key(*name)) { 400 } else { 404 };
            accounts::set_quota(&st.data, name, q.quota).map_err(|e| Ok((code, e)))?;
            match q.quota {
                Some(b) => log::info!("admin {me}: {name} may keep {b} bytes on the server"),
                None => log::info!("admin {me}: {name} has no storage limit"),
            }
            Ok(json!({"quota": q.quota}))
        }),
        (Method::Delete, ["users", name]) => remove_user(st, name),
        (Method::Get, ["users", name, "devices"]) => devices(st, name),
        (Method::Delete, ["users", name, "devices", id]) => match id.parse::<u64>() {
            Ok(id) => st.accounts.lock().unwrap_or_else(PoisonError::into_inner).revoke(name, id).map(|()| json!({})).map_err(|e| Ok((404, e))),
            Err(_) => Err(Ok((400, format!("not a device id: {id}")))),
        },
        (Method::Get, ["users", name, "shares"]) => Ok(json!(st.shares.lock().unwrap_or_else(PoisonError::into_inner).list(name))),
        (Method::Delete, ["users", name, "shares", id]) => {
            let r = st.shares.lock().unwrap_or_else(PoisonError::into_inner).remove(&st.data, name, id);
            match r {
                Ok(true) => {
                    log::info!("admin {me}: revoked {name}'s link {id}");
                    Ok(json!({}))
                }
                Ok(false) => Err(Ok((404, "no such link".to_string()))),
                Err(e) => Err(Ok((500, e))),
            }
        }
        (Method::Get, ["users", name, "folders"]) => folders(st, name),
        (Method::Post, ["users", name, "folders"]) => read_json::<NewFolder>(req).map_err(Err).and_then(|f| {
            let added = accounts::add_folder(&st.data, name, &f.path, f.name.as_deref()).map_err(|e| Ok((400, e)))?;
            log::info!("admin {me}: {name} has the library folder {} ({})", added.name, added.path);
            if f.writable || f.imports {
                // (a folder the server can't write in is not added at all)
                if let Err(e) = accounts::set_folder_mode(&st.data, name, &added.name, Some(f.writable || f.imports), Some(f.imports)) {
                    let _ = accounts::remove_folder(&st.data, name, &added.name);
                    return Err(Ok((400, e)));
                }
                log::info!("admin {me}: {name}'s {} is writable{}", added.name, if f.imports { " and takes the uploads" } else { "" });
            }
            st.folders.request(name);
            folders(st, name)
        }),
        (Method::Put, ["users", name, "folders", folder]) => read_json::<FolderMode>(req).map_err(Err).and_then(|m| {
            let folder = percent_decode(folder);
            let f = accounts::set_folder_mode(&st.data, name, &folder, m.writable, m.imports).map_err(|e| Ok((400, e)))?;
            log::info!(
                "admin {me}: {name}'s {} is {}{}",
                f.name,
                if f.writable { "writable" } else { "read-only" },
                if f.imports { " and takes the uploads" } else { "" }
            );
            folders(st, name)
        }),
        (Method::Delete, ["users", name, "folders", folder]) => {
            let folder = percent_decode(folder);
            accounts::remove_folder(&st.data, name, &folder).map_err(|e| Ok((404, e))).and_then(|()| {
                log::info!("admin {me}: {name} no longer has the library folder {folder}");
                folders(st, name)
            })
        }
        (Method::Put, ["users", name, "ignore"]) => read_json::<Ignore>(req).map_err(Err).and_then(|i| {
            let kept = accounts::set_ignore(&st.data, name, &i.ignore).map_err(|e| Ok((400, e)))?;
            log::info!("admin {me}: {name} ignores {kept:?}");
            st.folders.request(name);
            folders(st, name)
        }),
        (Method::Post, ["users", name, "previews", action @ ("pause" | "resume" | "generate")]) => {
            match *action {
                "pause" => st.folders.pause(true),
                "resume" => st.folders.pause(false),
                _ => st.folders.generate_all(name),
            }
            log::info!("admin {me}: {action} previews ({name})");
            folders(st, name)
        }
        (Method::Post, ["users", name, "scan"]) => {
            st.folders.request(name);
            folders(st, name)
        }
        (Method::Get, ["search"]) => Ok(search(st)),
        (Method::Put, ["search"]) => read_json::<Enabled>(req).map_err(Err).and_then(|b| {
            st.vision.set_enabled(b.enabled).map_err(|e| Ok((500, e)))?;
            log::info!("admin {me}: search by description turned {}", if b.enabled { "on" } else { "off" });
            Ok(search(st))
        }),
        (Method::Put, ["search", "text"]) => read_json::<Enabled>(req).map_err(Err).and_then(|b| {
            st.vision.set_text_enabled(b.enabled).map_err(|e| Ok((500, e)))?;
            log::info!("admin {me}: reading the text in photos turned {}", if b.enabled { "on" } else { "off" });
            Ok(search(st))
        }),
        (Method::Put, ["search", "schedule", kind]) => match Part::from_key(kind) {
            None => Err(Ok((404, "no such part".to_string()))),
            Some(part) => read_json::<Schedule>(req).map_err(Err).and_then(|b| {
                st.vision.switch.set_schedule(part, b.paused, b.from.zip(b.to)).map_err(|e| Ok((400, e)))?;
                log::info!(
                    "admin {me}: {kind} index {}{}",
                    if b.paused { "paused" } else { "not paused" },
                    b.from.zip(b.to).map_or(String::new(), |w| format!(", only {}-{} UTC", w.0, w.1))
                );
                st.vision.wake();
                Ok(search(st))
            }),
        },
        (Method::Post, ["search", "download"]) => read_json::<Licence>(req).map_err(Err).and_then(|l| fetch_models(st, &me, Part::Search, l)),
        (Method::Post, ["search", "text", "download"]) => read_json::<Licence>(req).map_err(Err).and_then(|l| fetch_models(st, &me, Part::Text, l)),
        (Method::Post, ["search", "faces", "download"]) => read_json::<Licence>(req).map_err(Err).and_then(|l| fetch_models(st, &me, Part::Faces, l)),
        (Method::Delete, ["search", "download"]) => {
            st.vision.switch.cancel(Part::Search);
            Ok(search(st))
        }
        (Method::Delete, ["search", "text", "download"]) => {
            st.vision.switch.cancel(Part::Text);
            Ok(search(st))
        }
        (Method::Delete, ["search", "faces", "download"]) => {
            st.vision.switch.cancel(Part::Faces);
            Ok(search(st))
        }
        // (faces are personal data: found only for a user an admin turned it on for)
        (Method::Put, ["users", name, "faces"]) => read_json::<Enabled>(req).map_err(Err).and_then(|b| {
            accounts::set_faces(&st.data, name, b.enabled).map_err(|e| Ok((404, e)))?;
            log::info!("admin {me}: finding the people in {name}'s photos turned {}", if b.enabled { "on" } else { "off" });
            st.vision.wake();
            Ok(search(st))
        }),
        (Method::Post, ["gc"]) => {
            let g = read_json::<Gc>(req).unwrap_or_default();
            match crate::gc::run(&st.data, g.dry_run) {
                Ok(r) => {
                    if !g.dry_run {
                        st.sizes.clear();
                    }
                    log::info!("admin {me}: gc{} removed {} file(s)", if g.dry_run { " (dry run)" } else { "" }, r.removed);
                    Ok(json!({"dryRun": g.dry_run, "removed": r.removed, "bytes": r.bytes, "kept": r.kept, "errors": r.errors}))
                }
                Err(e) => Err(Ok((500, e))),
            }
        }
        _ => Err(Ok((404, format!("no admin route {method} /api/admin/{rest}")))),
    };
    match r {
        Ok(v) => json(200, &v),
        Err(Ok((status, msg))) => error(status, msg),
        Err(Err(resp)) => resp,
    }
}

/// An error: a status and message, or a response made already (a body that didn't read).
type AdminResult = Result<Value, std::result::Result<(u16, String), Resp>>;

fn setup(st: &State, req: &mut Request) -> Resp {
    let s: Setup = match read_small_json(req) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let mut code = st.setup.lock().unwrap_or_else(PoisonError::into_inner);
    if accounts::has_admin(&st.data) || code.is_none() {
        return error(409, "this server has an admin already: sign in");
    }
    let given = s.code.trim().to_ascii_uppercase();
    if code.as_deref() != Some(given.as_str()) {
        let tries = st.setup_tries.fetch_add(1, std::sync::atomic::Ordering::SeqCst).saturating_add(1);
        if tries >= SETUP_TRIES {
            *code = new_setup_code();
            st.setup_tries.store(0, std::sync::atomic::Ordering::SeqCst);
            log::warn!("too many wrong setup codes: the new one is {}", code.as_deref().unwrap_or("(none)"));
        }
        drop(code);
        std::thread::sleep(std::time::Duration::from_millis(500));
        return error(403, "wrong setup code (it is in the server's log)");
    }
    if accounts::read_users(&st.data).is_ok_and(|f| f.users.contains_key(&s.user)) {
        return error(409, format!("`{}` exists: make them admin with `lightcraft-server user admin {} on`, or pick another name", s.user, s.user));
    }
    if let Err(e) = accounts::set_user(&st.data, &s.user, &s.password, false).and_then(|()| accounts::set_admin(&st.data, &s.user, true)) {
        return error(400, e);
    }
    *code = None;
    log::info!("admin {} created from the setup page", s.user);
    json(200, &json!({"user": s.user}))
}

fn login(st: &State, req: &mut Request) -> Resp {
    let l: Login = match read_small_json(req) {
        Ok(l) => l,
        Err(r) => return r,
    };
    let keys = crate::api::throttle_keys(req, &l.user);
    if let Some(r) = crate::api::throttled(st, &keys) {
        return r;
    }
    let r = st.accounts.lock().unwrap_or_else(PoisonError::into_inner).admin_login(&l.user, &l.password);
    let mut t = st.throttle.lock().unwrap_or_else(PoisonError::into_inner);
    if matches!(r, Err(LoginError::Refused)) {
        t.failed(&keys)
    } else {
        t.succeeded(&keys)
    }
    drop(t);
    match r {
        Ok(token) => {
            log::info!("admin {} signed in to the admin page", l.user);
            json(200, &json!({"token": token, "user": l.user}))
        }
        Err(LoginError::Refused) => {
            log::warn!("refused an admin sign-in as `{}`", l.user.chars().take(64).collect::<String>());
            std::thread::sleep(std::time::Duration::from_millis(500));
            error(401, "wrong user name or password, or not an admin")
        }
        Err(LoginError::Failed(e)) => error(500, e),
    }
}

/// Bytes on the file system holding `dir`: (total, available), from `df` where there is one.
fn disk(dir: &Path) -> Option<(u64, u64)> {
    let out = std::process::Command::new("df").arg("-Pk").arg(dir).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let cols: Vec<&str> = text.lines().nth(1)?.split_whitespace().collect();
    let k = |i: usize| cols.get(i)?.parse::<u64>().ok().map(|n| n.saturating_mul(1024));
    Some((k(1)?, k(3)?))
}

/// Bytes of the files under `dir` (a user's photo files), at most a few levels deep.
fn size_of(dir: &Path, depth: u32) -> u64 {
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    rd.flatten()
        .map(|e| match e.metadata() {
            Ok(m) if m.is_dir() && depth > 0 => size_of(&e.path(), depth - 1),
            Ok(m) if m.is_file() => m.len(),
            _ => 0,
        })
        .fold(0, u64::saturating_add)
}

fn status(st: &State, me: &str) -> Value {
    let data = std::fs::canonicalize(&st.data).unwrap_or_else(|_| st.data.clone());
    let users = accounts::read_users(&st.data).map(|f| f.users.len()).unwrap_or(0);
    json!({
        "me": me,
        "version": env!("CARGO_PKG_VERSION"),
        "data": data.to_string_lossy(),
        "listen": st.listen,
        "web": st.web.as_ref().map(|w| w.to_string_lossy().to_string()),
        "disk": disk(&st.data).map(|(total, free)| json!({"total": total, "free": free})),
        "users": users,
    })
}

/// What an admin agrees to before a download: its size, and its licences.
fn terms(part: Part) -> (f64, String) {
    use lightcraft_engine::vision::{
        FACE_BYTES, FACE_LICENSES, LICENSE_NAME, LICENSE_URL, MODEL_BYTES, TEXT_BYTES, TEXT_LICENSE_NAME, TEXT_LICENSE_URL,
    };
    match part {
        Part::Search => (MODEL_BYTES as f64, format!("{LICENSE_NAME} ({LICENSE_URL})")),
        Part::Text => (TEXT_BYTES as f64, format!("{TEXT_LICENSE_NAME} ({TEXT_LICENSE_URL})")),
        Part::Faces => {
            (FACE_BYTES as f64, format!("{} ({}) and {} ({})", FACE_LICENSES[0].0, FACE_LICENSES[0].1, FACE_LICENSES[1].0, FACE_LICENSES[1].1))
        }
    }
}

/// Fetch `part`'s models, once the licence was agreed to. For search and text that also turns it
/// on (nobody fetches a model to leave it off); people are turned on per user.
fn fetch_models(st: &State, me: &str, part: Part, l: Licence) -> AdminResult {
    let (bytes, licences) = terms(part);
    if !l.acknowledged {
        let (what, size) = match part {
            Part::Search => ("the search model", format!("{:.1} GB", bytes / 1e9)),
            Part::Text => ("the text-reading models", format!("{:.0} MB", bytes / 1e6)),
            Part::Faces => ("the face models", format!("{:.0} MB", bytes / 1e6)),
        };
        return Err(Ok((400, format!("{what}: a {size} download under the {licences}: send `acknowledged: true` once you agree to it"))));
    }
    match part {
        Part::Search => st.vision.set_enabled(true),
        Part::Text => st.vision.set_text_enabled(true),
        Part::Faces => Ok(()),
    }
    .map_err(|e| Ok((500, e)))?;
    let started = st.vision.start_download(part).map_err(|e| Ok((500, e)))?;
    log::info!("admin {me}: {part:?} models{}", if started { ": downloading" } else { ": already there" });
    Ok(search(st))
}

/// `GET /api/admin/search`: what the server does with the photos (describe, read the text, find the
/// people), each one's models and download, and how far each user's photos are.
fn search(st: &State) -> Value {
    use lightcraft_engine::vision::{
        FACE_BYTES, FACE_LICENSES, LICENSE_NAME, LICENSE_URL, MODEL_BYTES, TEXT_BYTES, TEXT_LICENSE_NAME, TEXT_LICENSE_URL,
    };
    let v = &st.vision;
    let (model, dim) = v.spec();
    // (what the server has indexed only matters while it does something)
    let busy = v.installed() || v.text_installed() || v.faces_installed();
    let users: Vec<Value> = accounts::read_users(&st.data)
        .map(|f| f.users.into_iter().map(|(name, u)| (name, u.faces)).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .map(|(name, faces)| {
            let (a, b, c) = match busy.then(|| crate::api::lib(st, &name).ok()).flatten() {
                Some(lib) => crate::vision::progress(st, &lib, &name),
                None => (None, None, None),
            };
            let total = a.as_ref().or(b.as_ref()).or(c.as_ref()).map(|p| p.total);
            json!({"name": name, "faces": faces, "total": total, "indexed": a.map(|p| p.done), "read": b.map(|p| p.done), "found": c.map(|p| p.done)})
        })
        .collect();
    // paused / window / time left for one part; the left is photos still to do times the lately measured time each took
    let schedule = |part: Part, done: fn(&Value) -> Option<u64>| {
        let left: u64 = users.iter().filter_map(|u| Some(u["total"].as_u64()?.saturating_sub(done(u)?))).sum();
        let w = v.switch.window(part);
        json!({
            "paused": v.switch.paused(part),
            "from": w.map(|w| w.0), "to": w.map(|w| w.1),
            "runsNow": v.switch.may_run(part),
            "left": left,
            "perPhoto": v.switch.per_photo(part),
        })
    };
    json!({
        "schedule": schedule(Part::Search, |u| u["indexed"].as_u64()),
        "enabled": v.switch.enabled(),
        "installed": v.files_installed(),
        "active": v.installed(),
        "loaded": v.loaded(),
        "model": model,
        "dim": dim,
        "dir": v.model_dir().to_string_lossy(),
        "sizeBytes": MODEL_BYTES,
        "license": LICENSE_NAME,
        "licenseUrl": LICENSE_URL,
        "download": v.switch.status(Part::Search),
        "text": {
            "enabled": v.switch.text_enabled(),
            "installed": v.text_files_installed(),
            "active": v.text_installed(),
            "sizeBytes": TEXT_BYTES,
            "license": TEXT_LICENSE_NAME,
            "licenseUrl": TEXT_LICENSE_URL,
            "download": v.switch.status(Part::Text),
            "schedule": schedule(Part::Text, |u| u["read"].as_u64()),
        },
        "faces": {
            "installed": v.faces_installed(),
            "sizeBytes": FACE_BYTES,
            "licenses": FACE_LICENSES.iter().map(|(name, url)| json!({"name": name, "url": url})).collect::<Vec<_>>(),
            "download": v.switch.status(Part::Faces),
            "schedule": schedule(Part::Faces, |u| u["found"].as_u64()),
        },
        "users": users,
    })
}

fn users(st: &State) -> AdminResult {
    let f = accounts::read_users(&st.data).map_err(|e| Ok((500, e)))?;
    st.sizes.refresh(&st.data);
    let mut out = Vec::new();
    for (name, u) in &f.users {
        let devices = st.accounts.lock().unwrap_or_else(PoisonError::into_inner).devices_of(name).len();
        let (photos, albums, in_folders) = match crate::api::lib(st, name) {
            Ok(l) => {
                let l = l.lock().unwrap_or_else(PoisonError::into_inner);
                let (photos, albums) = l.counts();
                (photos, albums, l.index.folder_total().bytes)
            }
            Err(e) => {
                log::error!("{e}");
                (0, 0, 0)
            }
        };
        let folders: Vec<Value> = u.folders.iter().map(|f| json!({"name": f.name, "path": f.path})).collect();
        out.push(json!({
            "name": name, "admin": u.admin, "photos": photos, "albums": albums, "devices": devices,
            // what their photos take on the server in all: the files kept for them (counted in the background:
            // null until they are) and the photos in their library folders, which are only read
            "bytes": st.sizes.get(name).map(|kept| kept.saturating_add(in_folders)), "folderBytes": in_folders,
            // (their storage limit counts the files kept for them only)
            "quota": u.quota,
            "shares": st.shares.lock().unwrap_or_else(PoisonError::into_inner).list(name).len(),
            "folders": folders, "scan": st.folders.status(name),
        }));
    }
    Ok(Value::Array(out))
}

fn add_user(st: &State, u: NewUser) -> std::result::Result<Value, (u16, String)> {
    accounts::set_user(&st.data, &u.name, &u.password, false).map_err(|e| (400, e))?;
    if u.admin {
        accounts::set_admin(&st.data, &u.name, true).map_err(|e| (500, e))?;
    }
    log::info!("added user {}{}", u.name, if u.admin { " (admin)" } else { "" });
    Ok(json!({"name": u.name}))
}

fn remove_user(st: &State, name: &str) -> AdminResult {
    accounts::remove_user(&st.data, name).map_err(|e| Ok((409, e)))?;
    // (their album links stop at once)
    st.shares.lock().unwrap_or_else(PoisonError::into_inner).drop_user(name);
    st.accounts.lock().unwrap_or_else(PoisonError::into_inner).forget(name);
    // closes the library (and releases its lock)
    st.libs.lock().unwrap_or_else(PoisonError::into_inner).remove(name);
    log::info!("removed user {name} (their files stay in {})", accounts::user_dir(&st.data, name).display());
    Ok(json!({"files": accounts::user_dir(&st.data, name).to_string_lossy()}))
}

/// `%20` and the like in a path segment (folder names may have spaces).
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while let Some(&c) = b.get(i) {
        let hex = b.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
        match (c, hex) {
            (b'%', Some(v)) => {
                out.push(v);
                i += 3;
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A user's library folders and what the last scan of them did.
fn folders(st: &State, name: &str) -> AdminResult {
    let f = accounts::read_users(&st.data).map_err(|e| Ok((500, e)))?;
    let u = f.users.get(name).ok_or_else(|| Ok((404, format!("no user `{name}`"))))?;
    let list: Vec<Value> = u
        .folders
        .iter()
        .map(|f| {
            // (a writable folder is tried, so the page shows a mount that went read-only)
            let can_write = f.writable.then(|| accounts::check_writable(std::path::Path::new(&f.path)).map_err(|e| e.to_string()));
            json!({
                "name": f.name, "path": f.path, "there": std::path::Path::new(&f.path).is_dir(),
                "writable": f.writable, "imports": f.imports,
                "writeError": can_write.and_then(Result::err),
            })
        })
        .collect();
    let mut scan = st.folders.status(name);
    (scan.previews_have, scan.previews_photos) = st.folders.coverage(st, name).unzip();
    Ok(json!({"folders": list, "ignore": u.ignore, "scan": scan}))
}

fn devices(st: &State, name: &str) -> AdminResult {
    if !accounts::read_users(&st.data).is_ok_and(|f| f.users.contains_key(name)) {
        return Err(Ok((404, format!("no user `{name}`"))));
    }
    let mut a = st.accounts.lock().unwrap_or_else(PoisonError::into_inner);
    let list: Vec<Value> = a
        .devices_of(name)
        .iter()
        .map(|d| json!({"id": d.id, "name": d.name, "space": d.space, "created": d.created, "lastSeen": a.last_seen(name, d.id)}))
        .collect();
    Ok(Value::Array(list))
}
