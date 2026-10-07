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

use std::path::Path;
use std::sync::PoisonError;

use serde::Deserialize;
use serde_json::{Value, json};
use tiny_http::{Header, Method, Request, Response, StatusCode};

use crate::State;
use crate::accounts::{self, LoginError};
use crate::api::{Resp, error, header_value, json, read_json};

const INDEX: &str = include_str!("../admin/index.html");
const SCRIPT: &str = include_str!("../admin/admin.js");
const STYLE: &str = include_str!("../admin/admin.css");

/// Wrong setup codes before a new one is made.
const SETUP_TRIES: u32 = 10;

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
        ("Cache-Control", "no-store"),
        ("Cross-Origin-Opener-Policy", "same-origin"),
        ("Cross-Origin-Resource-Policy", "same-origin"),
    ]
    .into_iter()
    .filter_map(|(k, v)| Header::from_bytes(k.as_bytes(), v.as_bytes()).ok())
    .collect()
}

/// `/admin`, `/admin/admin.js`, `/admin/admin.css`.
pub fn page(method: &Method, path: &str) -> Resp {
    if !matches!(method, Method::Get | Method::Head) {
        return error(405, "GET or HEAD");
    }
    let (ctype, body) = match path.trim_end_matches('/') {
        "/admin" | "/admin/index.html" => ("text/html; charset=utf-8", INDEX),
        "/admin/admin.js" => ("text/javascript; charset=utf-8", SCRIPT),
        "/admin/admin.css" => ("text/css; charset=utf-8", STYLE),
        _ => return error(404, "not found"),
    };
    let bytes = body.as_bytes().to_vec();
    let len = bytes.len();
    Response::new(StatusCode(200), page_headers(ctype), Box::new(std::io::Cursor::new(bytes)), Some(len), None)
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
        (Method::Delete, ["users", name]) => remove_user(st, name),
        (Method::Get, ["users", name, "devices"]) => devices(st, name),
        (Method::Delete, ["users", name, "devices", id]) => match id.parse::<u64>() {
            Ok(id) => st.accounts.lock().unwrap_or_else(PoisonError::into_inner).revoke(name, id).map(|()| json!({})).map_err(|e| Ok((404, e))),
            Err(_) => Err(Ok((400, format!("not a device id: {id}")))),
        },
        (Method::Post, ["gc"]) => {
            let g = read_json::<Gc>(req).unwrap_or_default();
            match crate::gc::run(&st.data, g.dry_run) {
                Ok(r) => {
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
    let s: Setup = match read_json(req) {
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
    let l: Login = match read_json(req) {
        Ok(l) => l,
        Err(r) => return r,
    };
    let r = st.accounts.lock().unwrap_or_else(PoisonError::into_inner).admin_login(&l.user, &l.password);
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
        .sum()
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

fn users(st: &State) -> AdminResult {
    let f = accounts::read_users(&st.data).map_err(|e| Ok((500, e)))?;
    let mut out = Vec::new();
    for (name, u) in &f.users {
        let devices = st.accounts.lock().unwrap_or_else(PoisonError::into_inner).devices_of(name).len();
        let (photos, albums) = match crate::api::lib(st, name) {
            Ok(l) => l.lock().unwrap_or_else(PoisonError::into_inner).counts(),
            Err(e) => {
                log::error!("{e}");
                (0, 0)
            }
        };
        let bytes = size_of(&accounts::user_dir(&st.data, name).join("blobs"), 3);
        out.push(json!({"name": name, "admin": u.admin, "photos": photos, "albums": albums, "devices": devices, "bytes": bytes}));
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
    st.accounts.lock().unwrap_or_else(PoisonError::into_inner).forget(name);
    // closes the library (and releases its lock)
    st.libs.lock().unwrap_or_else(PoisonError::into_inner).remove(name);
    log::info!("removed user {name} (their files stay in {})", accounts::user_dir(&st.data, name).display());
    Ok(json!({"files": accounts::user_dir(&st.data, name).to_string_lossy()}))
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
