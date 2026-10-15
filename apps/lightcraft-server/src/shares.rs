//! Album links: a person gives someone a link to an album and that someone sees it in a browser, with no account
//! and nothing to install (`https://photos.example.com/s/<token>`), the way a cloud photo service shares an album.
//!
//! - A link is a 256-bit random token the owner makes for one of their albums (`POST /api/shares`), optionally
//!   ending after some days, letting the visitor download the originals (one by one or the whole album as a ZIP,
//!   [`crate::zip`]), asking for a **password** first, or letting visitors **add photos** ([`crate::guest`]). The
//!   owner lists and revokes their links (`GET /api/shares`, `DELETE /api/shares/<id>`; also on the admin page).
//!   They are kept in `users/<name>/shares.json`, which only the server reads (the tokens are in it, and the
//!   passwords' argon2id hashes).
//! - A visitor gets the album as a page ([`gallery`]): its photos by the day they were taken, a **slideshow**, and
//!   a page per photo ([`photo_page`]) to step through with the arrow keys. The pictures ([`picture`]) are each
//!   photo **rendered with the edits it has now** at 400 px (the grid) and 1600 px (the view), by the same
//!   pipeline an export uses and without its metadata (no GPS, no camera serial number), or — if the link allows
//!   it — the original file. A rendered picture is kept under `users/<name>/shares/cache/` until the edits change.
//! - A link with a password shows a form first; the right password leaves a cookie that opens this link only
//!   (path `/s/<token>`, derived from the token and the password's hash: changing nothing else, a new link or
//!   a revoked one opens nothing). Wrong passwords are throttled like sign-ins.
//! - Only the album's photos are reachable through its link (a photo id from another album, or a deleted photo,
//!   is `404`), and only static albums can be shared (not folders or smart albums). Unknown, expired and revoked
//!   tokens are all `404`, and guessing is throttled by address.
//!
//! The pages carry their own styles; the photo page has a few lines of script for the arrow keys, allowed by
//! its hash and nothing else. Nothing of the visitor's is stored but the unlock cookie; the pages ask search
//! engines to leave them alone, and tell link previews (a chat app's card) the album's name and first picture,
//! unless the link has a password.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::PoisonError;
use std::time::Duration;

use lightcraft_catalog::sync::proto::{self, Share};
use lightcraft_catalog::{AlbumId, PhotoId};
use lightcraft_engine::export::{ExportFormat, ExportOptions, MetadataPolicy, Resize, ResizeMode};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::Digest;

use crate::State;
use crate::accounts;
use crate::api::{Resp, bytes, error, header, header_value, json as json_resp, lib};
use crate::http::{Header, Method, Request, Response, StatusCode};

/// Links one user may have at once.
pub const MAX_PER_USER: usize = 200;
/// The longest a link may last, in days.
pub const MAX_DAYS: u32 = 3650;
/// The shortest and longest password a link may have, in characters.
pub const PASSWORD_MIN: usize = 4;
pub const PASSWORD_MAX: usize = 256;
const THUMB_EDGE: usize = 400;
const VIEW_EDGE: usize = 1600;
/// How long a browser that typed a link's password keeps it open (or until the link ends).
const UNLOCK_SECS: u64 = 30 * 86_400;
/// The cookie that says a browser typed a link's password.
const COOKIE: &str = "lc_share";
/// Seconds each photo shows in a slideshow.
const SLIDE_SECS: u32 = 5;

/// A link as kept in `shares.json`: what its owner's devices see, and its password's hash (the server's eyes only).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Link {
    #[serde(flatten)]
    pub share: Share,
    /// argon2id PHC string of the password ([`Share::password`] says there is one).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_hash: Option<String>,
}

impl From<Share> for Link {
    fn from(share: Share) -> Link {
        Link { share, password_hash: None }
    }
}

/// `users/<name>/shares.json`.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct File {
    shares: Vec<Link>,
}

fn file_of(data: &Path, user: &str) -> PathBuf {
    accounts::user_dir(data, user).join("shares.json")
}

fn read_file(data: &Path, user: &str) -> File {
    std::fs::read(file_of(data, user)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn write_file(data: &Path, user: &str, f: &File) -> Result<(), String> {
    accounts::write_private_json(&file_of(data, user), f)
}

/// Every link of every user, by token.
#[derive(Default)]
pub struct Index {
    by_token: HashMap<String, (String, Link)>,
}

impl Index {
    /// Read every user's links.
    pub fn load(data: &Path) -> Index {
        let mut by_token = HashMap::new();
        for user in accounts::read_users(data).map(|f| f.users.into_keys().collect::<Vec<_>>()).unwrap_or_default() {
            for l in read_file(data, &user).shares {
                by_token.insert(l.share.token.clone(), (user.clone(), l));
            }
        }
        Index { by_token }
    }

    /// The link with this token, if it exists and hasn't ended.
    pub fn find(&self, token: &str) -> Option<(&str, &Link)> {
        let (user, l) = self.by_token.get(token)?;
        l.share.expires.is_none_or(|e| accounts::now() < e).then_some((user.as_str(), l))
    }

    /// A user's links, oldest first.
    pub fn list(&self, user: &str) -> Vec<Share> {
        let mut v: Vec<Share> = self.by_token.values().filter(|(u, _)| u == user).map(|(_, l)| l.share.clone()).collect();
        v.sort_by(|a, b| (a.created, &a.id).cmp(&(b.created, &b.id)));
        v
    }

    pub fn add(&mut self, data: &Path, user: &str, link: Link) -> Result<(), String> {
        let mut f = read_file(data, user);
        if f.shares.len() >= MAX_PER_USER {
            return Err(format!("at most {MAX_PER_USER} links: revoke some first"));
        }
        f.shares.push(link.clone());
        write_file(data, user, &f)?;
        self.by_token.insert(link.share.token.clone(), (user.to_string(), link));
        Ok(())
    }

    /// Revoke link `id` of `user`. `false`: they have no such link.
    pub fn remove(&mut self, data: &Path, user: &str, id: &str) -> Result<bool, String> {
        let mut f = read_file(data, user);
        let before = f.shares.len();
        f.shares.retain(|l| l.share.id != id);
        if f.shares.len() == before {
            return Ok(false);
        }
        write_file(data, user, &f)?;
        self.by_token.retain(|_, (u, l)| !(u == user && l.share.id == id));
        Ok(true)
    }

    /// A removed user's links stop working at once (their file stays).
    pub fn drop_user(&mut self, user: &str) {
        self.by_token.retain(|_, (u, _)| u != user);
    }
}

fn index(st: &State) -> std::sync::MutexGuard<'_, Index> {
    st.shares.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---- the owner's side ----

/// `GET /api/shares`.
pub fn list(st: &State, user: &str) -> Resp {
    json_resp(200, &json!({"shares": index(st).list(user)}))
}

/// `POST /api/shares` ([`proto::NewShare`]).
pub fn create(st: &State, user: &str, req: &mut Request) -> Resp {
    let n: proto::NewShare = match crate::api::read_small_json(req) {
        Ok(n) => n,
        Err(r) => return r,
    };
    let password = n.password.as_deref().filter(|p| !p.is_empty());
    if let Some(p) = password {
        let len = p.chars().count();
        if !(PASSWORD_MIN..=PASSWORD_MAX).contains(&len) {
            return error(400, format!("a link's password has {PASSWORD_MIN} to {PASSWORD_MAX} characters"));
        }
    }
    let l = match lib(st, user) {
        Ok(l) => l,
        Err(e) => return error(500, e),
    };
    let name = {
        let l = l.lock().unwrap_or_else(PoisonError::into_inner);
        match l.core.catalog().album(AlbumId(n.album)) {
            Some(a) if !a.is_smart() && !a.folder => a.name.clone(),
            Some(_) => return error(400, "only an album of photos can be shared: not a folder or a smart album"),
            None => return error(404, format!("no album {}", n.album)),
        }
    };
    let (id, token) = match (accounts::random_hex(8), accounts::random_hex(32)) {
        (Ok(i), Ok(t)) => (i, t),
        (Err(e), _) | (_, Err(e)) => return error(500, e),
    };
    let password_hash = match password.map(accounts::hash_password).transpose() {
        Ok(h) => h,
        Err(e) => return error(500, e),
    };
    let now = accounts::now();
    let expires = n.expires_days.filter(|d| *d > 0).map(|d| now.saturating_add(u64::from(d.min(MAX_DAYS)) * 86_400));
    let share =
        Share { id, token, album: n.album, name, created: now, expires, originals: n.originals, password: password_hash.is_some(), upload: n.upload };
    match index(st).add(&st.data, user, Link { share: share.clone(), password_hash }) {
        Ok(()) => {
            let how = [(share.password, "password"), (share.upload, "uploads"), (share.originals, "originals")]
                .iter()
                .filter(|(on, _)| *on)
                .map(|(_, w)| *w)
                .collect::<Vec<_>>()
                .join(", ");
            log::info!("{user}: shared album {} ({}){}", share.album, share.name, if how.is_empty() { String::new() } else { format!(": {how}") });
            json_resp(200, &json!(share))
        }
        Err(e) => error(409, e),
    }
}

/// `DELETE /api/shares/<id>`.
pub fn revoke(st: &State, user: &str, id: &str) -> Resp {
    match index(st).remove(&st.data, user, id) {
        Ok(true) => json_resp(200, &json!({})),
        Ok(false) => error(404, "no such link"),
        Err(e) => error(500, e),
    }
}

// ---- the visitor's side ----

pub(crate) fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// A file name that is safe in a `Content-Disposition` header and on a disk.
pub(crate) fn safe_name(name: &str) -> String {
    let n: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' }).take(100).collect();
    if n.is_empty() || n.starts_with('.') { format!("photo{n}") } else { n }
}

/// The policy of the pages: their own styles, pictures from here, forms that post here.
pub(crate) const PAGE_CSP: &str =
    "default-src 'none'; img-src 'self'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'";

/// What every page and picture of a link says about itself.
pub(crate) fn page_headers(r: &mut Resp, csp: &str) {
    for (k, v) in [
        ("X-Robots-Tag", "noindex, nofollow, noarchive"),
        ("Referrer-Policy", "no-referrer"),
        ("X-Content-Type-Options", "nosniff"),
        ("Content-Security-Policy", csp),
        ("Cross-Origin-Resource-Policy", "same-origin"),
    ] {
        if let Some(h) = header(k, v) {
            r.add_header(h);
        }
    }
}

/// An HTML page of a link.
pub(crate) fn html(status: u16, body: String, csp: &str) -> Resp {
    let mut r = bytes(status, "text/html; charset=utf-8", body.into_bytes());
    page_headers(&mut r, csp);
    if let Some(h) = header("Cache-Control", "private, no-cache") {
        r.add_header(h);
    }
    r
}

pub(crate) const STYLE: &str = "\
:root{color-scheme:light dark;--bg:#fafafa;--fg:#1c1c1e;--mute:#6e6e73;--line:#d8d8dc;--card:#fff;--accent:#0a6cdf;--on-accent:#fff;--bad:#c4281c}\
@media(prefers-color-scheme:dark){:root{--bg:#1c1c1e;--fg:#f2f2f7;--mute:#98989f;--line:#3a3a3c;--card:#2c2c2e;--accent:#3d9bff;--on-accent:#06182e;--bad:#ff6b5e}}\
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--fg);font:16px/1.45 system-ui,-apple-system,Segoe UI,sans-serif}\
.wrap{max-width:1400px;margin:0 auto;padding:0 16px}header.top{padding:28px 0 6px}h1{margin:0;font-size:1.75rem;font-weight:700;overflow-wrap:anywhere}\
.sub{margin:4px 0 0;color:var(--mute)}.actions{display:flex;flex-wrap:wrap;gap:8px;margin-top:14px}\
.btn{display:inline-flex;align-items:center;gap:6px;min-height:36px;padding:6px 16px;border-radius:99px;border:1px solid var(--line);background:var(--card);color:var(--fg);text-decoration:none;font:inherit;font-size:.92rem;cursor:pointer}\
.btn:hover{border-color:var(--mute)}.btn.primary{background:var(--accent);border-color:var(--accent);color:var(--on-accent);font-weight:600}\
.note{margin:14px 0 0;padding:10px 14px;border-radius:10px;background:var(--card);border:1px solid var(--line)}.note.bad{border-color:var(--bad);color:var(--bad)}\
.day h2{font-size:1rem;font-weight:600;margin:24px 0 8px}.grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(150px,1fr));gap:4px}\
figure{margin:0;position:relative;background:var(--line);aspect-ratio:1/1;overflow:hidden;border-radius:4px}\
figure a.pic{display:block;width:100%;height:100%}img{display:block}figure img{width:100%;height:100%;object-fit:cover}\
figcaption{position:absolute;right:6px;bottom:6px}figcaption a{font-size:.75rem;background:rgba(0,0,0,.6);color:#fff;text-decoration:none;padding:2px 8px;border-radius:99px}\
form.add{margin:16px 0 0;padding:14px 16px;border:1px dashed var(--mute);border-radius:12px;display:flex;flex-wrap:wrap;gap:10px 16px;align-items:end}\
form.add b{flex-basis:100%}label{display:flex;flex-direction:column;gap:4px;font-size:.85rem;color:var(--mute)}\
input{font:inherit;color:var(--fg);background:var(--card);border:1px solid var(--line);border-radius:8px;padding:6px 10px;max-width:100%}\
.empty{padding:48px 0;color:var(--mute);text-align:center}footer{padding:32px 16px;color:var(--mute);font-size:.8rem;text-align:center}\
a:focus-visible,.btn:focus-visible,input:focus-visible{outline:2px solid var(--accent);outline-offset:2px}\
main.lock{max-width:360px;margin:12vh auto 0;padding:0 16px;text-align:center}main.lock form{display:flex;flex-direction:column;gap:10px;margin-top:20px;text-align:left}\
body.viewer{background:#000;color:#f2f2f7;min-height:100vh;display:flex;flex-direction:column}\
.bar{display:flex;flex-wrap:wrap;align-items:center;gap:8px 12px;padding:10px 16px}.bar a{color:#f2f2f7}.bar .back{text-decoration:none;font-weight:600;margin-right:auto;overflow-wrap:anywhere}\
.bar .count{color:#98989f;font-variant-numeric:tabular-nums}.bar .btn{background:#2c2c2e;border-color:#3a3a3c;color:#f2f2f7}\
.stage{flex:1;display:flex;align-items:center;justify-content:center;position:relative;min-height:0;padding:0 8px}\
.stage img{max-width:100%;max-height:calc(100vh - 120px);object-fit:contain}\
.stage .nav{position:absolute;top:0;bottom:0;width:22%;display:flex;align-items:center;padding:0 12px;font-size:2.6rem;text-decoration:none;color:rgba(255,255,255,.75)}\
.stage .prev{left:0}.stage .next{right:0;justify-content:flex-end}.stage .nav:hover{color:#fff}.caption{text-align:center;color:#98989f;margin:8px 16px 14px;font-size:.9rem}\
.preload{display:none}";

/// The photo page's script: ← and → step through the album, Esc goes back to it (each a link the page has).
const KEYS_JS: &str = "addEventListener('keydown',function(e){if(e.altKey||e.ctrlKey||e.metaKey||e.shiftKey)return;\
var k={ArrowLeft:'prev',ArrowRight:'next',Escape:'up'}[e.key];var a=k&&document.getElementById(k);if(a){e.preventDefault();location.href=a.href;}});";

/// Base64 (RFC 4648, padded): the hash in a Content Security Policy.
fn base64(b: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(b.len().div_ceil(3) * 4);
    for c in b.chunks(3) {
        let n = (u32::from(c.first().copied().unwrap_or(0)) << 16)
            | (u32::from(c.get(1).copied().unwrap_or(0)) << 8)
            | u32::from(c.get(2).copied().unwrap_or(0));
        for i in 0..4 {
            if i <= c.len() {
                out.push(char::from(A.get(((n >> (18 - 6 * i)) & 63) as usize).copied().unwrap_or(b'=')));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The photo page's policy: [`PAGE_CSP`] and its one script, by hash.
fn viewer_csp() -> &'static str {
    static CSP: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    CSP.get_or_init(|| format!("{PAGE_CSP}; script-src 'sha256-{}'", base64(&sha2::Sha256::digest(KEYS_JS.as_bytes()))))
}

/// The page's address as the visitor typed it (`https://photos.example.com`), for link previews, which want
/// whole addresses: the proxy's `X-Forwarded-Proto` / `X-Forwarded-Host`, else `Host`.
fn site(req: &Request) -> Option<String> {
    let host = header_value(req, "X-Forwarded-Host").or_else(|| header_value(req, "Host"))?.split(',').next()?.trim();
    let ok = !host.is_empty() && host.len() <= 255 && host.bytes().all(|b| b.is_ascii_alphanumeric() || b".-:[]".contains(&b));
    let proto = match header_value(req, "X-Forwarded-Proto").and_then(|p| p.split(',').next()).map(str::trim) {
        Some(p) if p.eq_ignore_ascii_case("https") => "https",
        _ => "http",
    };
    ok.then(|| format!("{proto}://{host}"))
}

/// What the visitor's connection came over (a proxy says so): the unlock cookie is `Secure` on HTTPS.
fn is_https(req: &Request) -> bool {
    header_value(req, "X-Forwarded-Proto").is_some_and(|p| p.split(',').next().is_some_and(|p| p.trim().eq_ignore_ascii_case("https")))
}

/// The unlock cookie's value for a link and its password: whoever has it typed the password (or was given
/// this cookie by someone who did). It changes with the password and with the link.
fn unlock_value(token: &str, hash: &str) -> String {
    let d = sha2::Sha256::digest(format!("lightcraft share unlock\n{token}\n{hash}").as_bytes());
    d.iter().map(|b| format!("{b:02x}")).collect()
}

/// The browser sent the cookie that opens this link.
fn unlocked(req: &Request, token: &str, hash: &str) -> bool {
    let want = unlock_value(token, hash);
    req.headers().iter().filter(|h| h.field.equiv("Cookie")).flat_map(|h| h.value.as_str().split(';')).any(|c| {
        c.trim().split_once('=').is_some_and(|(k, v)| {
            k.trim() == COOKIE && v.trim().len() == want.len() && v.trim().bytes().zip(want.bytes()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
        })
    })
}

/// `application/x-www-form-urlencoded` field `key` (`+` and `%XX` decoded).
pub(crate) fn form_field(body: &str, key: &str) -> Option<String> {
    let raw = body.split('&').filter_map(|kv| kv.split_once('=')).find(|(k, _)| *k == key)?.1;
    let b = raw.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while let Some(&c) = b.get(i) {
        let hex = |j: usize| b.get(j).and_then(|c| char::from(*c).to_digit(16));
        match c {
            b'+' => out.push(b' '),
            b'%' if hex(i + 1).is_some() && hex(i + 2).is_some() => {
                out.push((hex(i + 1).unwrap_or(0) * 16 + hex(i + 2).unwrap_or(0)) as u8);
                i += 2;
            }
            c => out.push(c),
        }
        i += 1;
    }
    Some(String::from_utf8_lossy(&out).into_owned())
}

/// The page that asks for a link's password. It doesn't name the album: that is for those who know it.
fn locked_page(token: &str, wrong: bool) -> Resp {
    let note = if wrong { "<p class=\"note bad\" role=\"alert\">That isn't the password. Try again.</p>" } else { "" };
    html(
        401,
        format!(
            "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
             <meta name=\"robots\" content=\"noindex,nofollow\"><title>A shared album</title><style>{STYLE}</style></head><body>\
             <main class=\"lock\"><h1>A shared album</h1><p class=\"sub\">Type its password to see the photos.</p>{note}\
             <form method=\"post\" action=\"/s/{token}/unlock\"><label for=\"pw\">Password</label>\
             <input id=\"pw\" type=\"password\" name=\"password\" autocomplete=\"current-password\" required autofocus>\
             <button class=\"btn primary\" type=\"submit\">View album</button></form></main>\
             <footer>Shared with LightCraft</footer></body></html>"
        ),
        PAGE_CSP,
    )
}

/// `POST /s/<token>/unlock` (`password=…`): the right password sets the cookie and goes to the album.
fn unlock(st: &State, req: &mut Request, token: &str, link: &Link, keys: &mut Vec<String>) -> Resp {
    let Some(hash) = link.password_hash.as_deref() else { return see_other(&format!("/s/{token}"), None) };
    keys.push(format!("sharepw:{}", link.share.id));
    if let Some(r) = crate::api::throttled(st, keys) {
        return r;
    }
    let body = match crate::api::read_body(req, 4096) {
        Ok(b) => String::from_utf8_lossy(&b).into_owned(),
        Err(r) => return r,
    };
    let given = form_field(&body, "password").unwrap_or_default();
    if !accounts::password_ok(hash, &given) {
        st.throttle.lock().unwrap_or_else(PoisonError::into_inner).failed(keys);
        std::thread::sleep(Duration::from_millis(300));
        return locked_page(token, true);
    }
    st.throttle.lock().unwrap_or_else(PoisonError::into_inner).succeeded(keys);
    let max_age = link.share.expires.map_or(UNLOCK_SECS, |e| e.saturating_sub(accounts::now()).min(UNLOCK_SECS));
    let cookie = format!(
        "{COOKIE}={}; Path=/s/{token}; Max-Age={max_age}; HttpOnly; SameSite=Lax{}",
        unlock_value(token, hash),
        if is_https(req) { "; Secure" } else { "" }
    );
    see_other(&format!("/s/{token}"), Some(&cookie))
}

/// `303 See Other` to `to` (after a form), with a cookie to set.
pub(crate) fn see_other(to: &str, cookie: Option<&str>) -> Resp {
    let mut r = bytes(303, "text/plain; charset=utf-8", Vec::new());
    for h in [header("Location", to), cookie.and_then(|c| header("Set-Cookie", c)), header("Cache-Control", "no-store")].into_iter().flatten() {
        r.add_header(h);
    }
    page_headers(&mut r, PAGE_CSP);
    r
}

/// A photo of a link's album, as its pages show it.
#[derive(Clone, Debug)]
pub(crate) struct Item {
    pub id: u64,
    pub name: String,
    /// When it was taken (else added): `YYYY-MM-DDTHH:MM:SS`.
    pub date: String,
}

/// The album's name and its photos, oldest first (the album's own order among photos of the same time).
pub(crate) fn album_items(st: &State, user: &str, album: u64) -> Result<(String, Vec<Item>), Resp> {
    let l = lib(st, user).map_err(|e| error(500, e))?;
    let l = l.lock().unwrap_or_else(PoisonError::into_inner);
    let c = l.core.catalog();
    let Some(a) = c.album(AlbumId(album)).filter(|a| !a.is_smart() && !a.folder) else {
        return Err(error(404, "this link's album is gone"));
    };
    let mut items: Vec<Item> = a
        .photos
        .iter()
        .filter_map(|id| c.photo(*id))
        .filter(|p| !p.deleted && !p.local)
        .map(|p| Item { id: p.id.0, name: p.file_name.clone(), date: p.date().to_string() })
        .collect();
    items.sort_by(|a, b| a.date.cmp(&b.date));
    Ok((a.name.clone(), items))
}

/// `2026-03-12…` → `Thursday 12 March 2026` (the text itself when it isn't a date).
fn day_label(date: &str) -> String {
    let parts = (
        date.get(..4).and_then(|y| y.parse::<u32>().ok()),
        date.get(5..7).and_then(|m| m.parse::<usize>().ok()),
        date.get(8..10).and_then(|d| d.parse::<u32>().ok()),
    );
    match (parts, lightcraft_catalog::dates::weekday(date)) {
        ((Some(y), Some(m), Some(d)), Some(w)) if (1..=12).contains(&m) => {
            format!("{w} {d} {} {y}", lightcraft_catalog::dates::MONTHS.get(m - 1).copied().unwrap_or(""))
        }
        _ => date.chars().take(10).collect(),
    }
}

/// The days an album spans: `12 March 2026`, `12 – 19 March 2026`, `30 March – 2 April 2026`…
fn span_label(first: &str, last: &str) -> String {
    let short = |date: &str| -> Option<(u32, &'static str, u32)> {
        let m = date.get(5..7)?.parse::<usize>().ok()?;
        Some((date.get(..4)?.parse().ok()?, lightcraft_catalog::dates::MONTHS.get(m.checked_sub(1)?).copied()?, date.get(8..10)?.parse().ok()?))
    };
    match (short(first), short(last)) {
        (Some((y1, m1, d1)), Some((y2, m2, d2))) if (y1, m1, d1) == (y2, m2, d2) => format!("{d1} {m1} {y1}"),
        (Some((y1, m1, d1)), Some((y2, m2, d2))) if (y1, m1) == (y2, m2) => format!("{d1} – {d2} {m2} {y2}"),
        (Some((y1, m1, d1)), Some((y2, m2, d2))) if y1 == y2 => format!("{d1} {m1} – {d2} {m2} {y2}"),
        (Some((y1, m1, d1)), Some((y2, m2, d2))) => format!("{d1} {m1} {y1} – {d2} {m2} {y2}"),
        _ => String::new(),
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The `<head>` of a link's page.
fn head(title: &str, extra: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <meta name=\"robots\" content=\"noindex,nofollow\">{extra}<title>{}</title><style>{STYLE}</style></head>",
        esc(title)
    )
}

/// `GET /s/<token>`: the album as a page, by day. `?added=N&skipped=M` thanks a visitor who added photos.
fn gallery(st: &State, req: &Request, token: &str, user: &str, link: &Link, q: &str) -> Resp {
    let share = &link.share;
    let (name, items) = match album_items(st, user, share.album) {
        Ok(x) => x,
        Err(r) => return r,
    };
    let count = plural(items.len(), "photo", "photos");
    let span = match (items.first(), items.last()) {
        (Some(a), Some(b)) => span_label(&a.date, &b.date),
        _ => String::new(),
    };
    // a chat app's card for the link: its name and first picture (not for a link with a password: those
    // are for the people who know it)
    let mut og = String::new();
    if !share.password {
        og.push_str(&format!(
            "<meta property=\"og:type\" content=\"website\"><meta property=\"og:title\" content=\"{}\"><meta property=\"og:description\" content=\"{}\">",
            esc(&name),
            esc(&format!("{count} shared with LightCraft"))
        ));
        if let (Some(site), Some(first)) = (site(req), items.first()) {
            og.push_str(&format!(
                "<meta property=\"og:image\" content=\"{site}/s/{token}/{}/view\"><meta name=\"twitter:card\" content=\"summary_large_image\">",
                first.id
            ));
        }
    }
    let mut h = head(&name, &og);
    h.push_str(&format!(
        "<body><div class=\"wrap\"><header class=\"top\"><h1>{}</h1><p class=\"sub\">{count}{}{}</p>",
        esc(&name),
        if span.is_empty() { String::new() } else { format!(" · {}", esc(&span)) },
        if share.upload { " · Anyone with the link can add photos" } else { "" }
    ));
    let mut actions = Vec::new();
    if let Some(first) = items.first() {
        actions.push(format!("<a class=\"btn primary\" href=\"/s/{token}/{}?play=1\">&#9654; Slideshow</a>", first.id));
        if share.originals {
            actions.push(format!("<a class=\"btn\" href=\"/s/{token}/download\" download>Download all</a>"));
        }
    }
    if share.upload {
        actions.push("<a class=\"btn\" href=\"#add\">Add photos</a>".to_string());
    }
    if !actions.is_empty() {
        h.push_str(&format!("<nav class=\"actions\">{}</nav>", actions.concat()));
    }
    // (numbers only: nothing the visitor typed is shown back)
    let num = |k: &str| crate::api::query(q, k).and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
    let (added, skipped, already) = (num("added"), num("skipped"), num("already"));
    if added > 0 {
        h.push_str(&format!("<p class=\"note\" role=\"status\">Thank you: {} added to the album.</p>", plural(added, "photo was", "photos were")));
    }
    if already > 0 {
        h.push_str(&format!("<p class=\"note\" role=\"status\">{} in the album already.</p>", plural(already, "photo was", "photos were")));
    }
    if skipped > 0 {
        h.push_str(&format!(
            "<p class=\"note bad\" role=\"status\">{} left out: not photos or videos the server can read.</p>",
            plural(skipped, "file was", "files were")
        ));
    }
    h.push_str("</header><main>");
    if items.is_empty() {
        h.push_str("<p class=\"empty\">No photos in this album yet.</p>");
    }
    let mut day = None;
    for it in &items {
        let d = it.date.get(..10).unwrap_or("");
        if day != Some(d) {
            if day.is_some() {
                h.push_str("</div></section>");
            }
            h.push_str(&format!("<section class=\"day\"><h2>{}</h2><div class=\"grid\">", esc(&day_label(d))));
            day = Some(d);
        }
        let f = esc(&it.name);
        h.push_str(&format!(
            "<figure><a class=\"pic\" href=\"/s/{token}/{id}\"><img loading=\"lazy\" alt=\"{f}\" src=\"/s/{token}/{id}/thumb\"></a>",
            id = it.id
        ));
        if share.originals {
            h.push_str(&format!("<figcaption><a href=\"/s/{token}/{}/original\" download>Original</a></figcaption>", it.id));
        }
        h.push_str("</figure>");
    }
    if day.is_some() {
        h.push_str("</div></section>");
    }
    if share.upload {
        h.push_str(&format!(
            "<form class=\"add\" id=\"add\" method=\"post\" action=\"/s/{token}/upload\" enctype=\"multipart/form-data\">\
             <b>Add your photos to this album</b>\
             <label for=\"photos\">Photos and videos<input id=\"photos\" type=\"file\" name=\"photos\" multiple required \
             accept=\"image/*,video/*,.dng,.cr2,.cr3,.crw,.nef,.nrw,.arw,.srf,.sr2,.raf,.orf,.rw2,.raw,.pef,.srw,.x3f,.3fr,.iiq,.erf,.kdc,.dcr,.mrw,.heic,.heif,.avif,.jxl,.psd\"></label>\
             <label for=\"who\">Your name (optional)<input id=\"who\" name=\"name\" maxlength=\"64\" autocomplete=\"name\"></label>\
             <button class=\"btn primary\" type=\"submit\">Upload</button></form>"
        ));
    }
    h.push_str("</main></div><footer>Shared with LightCraft</footer></body></html>");
    html(200, h, PAGE_CSP)
}

/// `GET /s/<token>/<photo>[?play=1]`: one photo, large, with the ones before and after it (← →), and a
/// slideshow that moves on by itself (`play`, a refresh every [`SLIDE_SECS`]: no script needed).
fn photo_page(st: &State, token: &str, user: &str, link: &Link, photo: u64, q: &str) -> Resp {
    let share = &link.share;
    let (name, items) = match album_items(st, user, share.album) {
        Ok(x) => x,
        Err(r) => return r,
    };
    let Some(i) = items.iter().position(|it| it.id == photo) else { return error(404, "no such photo here") };
    let play = crate::api::query(q, "play") == Some("1");
    let n = items.len();
    let at = |j: usize| items.get(j).map(|it| it.id);
    // (a slideshow goes round; the pages stop at the ends)
    let (prev, next) = if play { (at((i + n - 1) % n), at((i + 1) % n)) } else { (i.checked_sub(1).and_then(at), at(i + 1)) };
    let suffix = if play { "?play=1" } else { "" };
    let (file, day) = items.get(i).map(|it| (it.name.clone(), day_label(&it.date))).unwrap_or_default();
    let refresh = match (play, next) {
        (true, Some(nx)) => format!("<meta http-equiv=\"refresh\" content=\"{SLIDE_SECS};url=/s/{token}/{nx}?play=1\">"),
        _ => String::new(),
    };
    let mut h = head(&format!("{file} – {name}"), &refresh);
    h.push_str("<body class=\"viewer\"><header class=\"bar\">");
    h.push_str(&format!("<a class=\"back\" id=\"up\" href=\"/s/{token}\">&#8249; {}</a><span class=\"count\">{} of {n}</span>", esc(&name), i + 1));
    if play {
        h.push_str(&format!("<a class=\"btn\" href=\"/s/{token}/{photo}\">&#10074;&#10074; Pause</a>"));
    } else {
        h.push_str(&format!("<a class=\"btn\" href=\"/s/{token}/{photo}?play=1\">&#9654; Slideshow</a>"));
    }
    if share.originals {
        h.push_str(&format!("<a class=\"btn\" href=\"/s/{token}/{photo}/original\" download>Original</a>"));
    }
    h.push_str("</header><main class=\"stage\">");
    if let Some(p) = prev {
        h.push_str(&format!(
            "<a class=\"nav prev\" id=\"prev\" rel=\"prev\" href=\"/s/{token}/{p}{suffix}\" aria-label=\"Previous photo\">&#8249;</a>"
        ));
    }
    h.push_str(&format!("<img src=\"/s/{token}/{photo}/view\" alt=\"{}\">", esc(&file)));
    if let Some(nx) = next {
        h.push_str(&format!("<a class=\"nav next\" id=\"next\" rel=\"next\" href=\"/s/{token}/{nx}{suffix}\" aria-label=\"Next photo\">&#8250;</a>"));
    }
    h.push_str(&format!("</main><p class=\"caption\">{}</p>", esc(&day)));
    // the next picture loads while this one shows
    if let Some(nx) = next.filter(|nx| *nx != photo) {
        h.push_str(&format!("<img class=\"preload\" src=\"/s/{token}/{nx}/view\" alt=\"\">"));
    }
    h.push_str(&format!("<script>{KEYS_JS}</script></body></html>"));
    html(200, h, viewer_csp())
}

/// What picture `kind` of a photo is: its long edge, or the original.
enum Kind {
    Edge(usize),
    Original,
}

fn kind_of(s: &str) -> Option<Kind> {
    match s {
        "thumb" => Some(Kind::Edge(THUMB_EDGE)),
        "view" => Some(Kind::Edge(VIEW_EDGE)),
        "original" => Some(Kind::Original),
        _ => None,
    }
}

/// Where the server has the original with this content: its own store, or a library folder.
fn original_of(st: &State, l: &std::sync::Mutex<crate::api::UserLib>, user: &str, hash: &str) -> Option<PathBuf> {
    let dir = accounts::user_dir(&st.data, user);
    crate::api::blob_path(&dir.join("blobs"), "original", hash)
        .filter(|b| b.is_file())
        .or_else(|| crate::api::folder_original_path(st, l, user, hash))
}

/// `GET /s/<token>/<photo>/<thumb|view|original>`.
fn picture(st: &State, req: &Request, user: &str, share: &Share, photo: u64, kind: Kind) -> Resp {
    if matches!(kind, Kind::Original) && !share.originals {
        return error(404, "this link doesn't offer the originals");
    }
    let l = match lib(st, user) {
        Ok(l) => l,
        Err(e) => return error(500, e),
    };
    // what the photo is, from the catalog (only the album's own photos are reachable)
    let (hash, name, settings) = {
        let l = l.lock().unwrap_or_else(PoisonError::into_inner);
        let c = l.core.catalog();
        let in_album = c.album(AlbumId(share.album)).is_some_and(|a| !a.is_smart() && !a.folder && a.photos.iter().any(|p| p.0 == photo));
        let Some(p) = c.photo(PhotoId(photo)).filter(|p| in_album && !p.deleted && !p.local) else {
            return error(404, "no such photo here");
        };
        let Some(hash) = lightcraft_catalog::sync::content_key(p) else { return error(404, "the server has no file for this photo") };
        (hash, p.file_name.clone(), serde_json::to_string(&*p.develop).unwrap_or_default())
    };
    let Some(original) = original_of(st, &l, user, &hash) else {
        return error(404, "the server has no file for this photo");
    };
    match kind {
        Kind::Original => original_file(req, &original, &name),
        Kind::Edge(edge) => {
            // kept until the photo's edits (or its file) change
            let key = lightcraft_preview::Hasher128::new().str(&settings).str(&hash).finish();
            let cache = accounts::user_dir(&st.data, user).join("shares").join("cache").join(format!("{:032x}-{edge}.jpg", key.0));
            let body = match std::fs::read(&cache) {
                Ok(b) => b,
                Err(_) => match render(st, &original, &name, &settings, edge, &hash) {
                    Ok(b) => {
                        if let Some(parent) = cache.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        let _ = lightcraft_catalog::safe_file::write_atomic_nosync(&cache, &b);
                        b
                    }
                    Err(r) => return r,
                },
            };
            let mut r = bytes(200, "image/jpeg", body);
            page_headers(&mut r, PAGE_CSP);
            if let Some(h) = header("Cache-Control", "private, max-age=300") {
                r.add_header(h);
            }
            r
        }
    }
}

/// Render the photo with its edits at `edge` pixels long, JPEG, without its metadata.
fn render(st: &State, original: &Path, name: &str, settings: &str, edge: usize, hash: &str) -> Result<Vec<u8>, Resp> {
    let export = ExportOptions {
        format: ExportFormat::Jpeg,
        quality: 85,
        resize: Some(Resize { mode: ResizeMode::LongEdge, value: edge as f32, height: edge as u32, dont_enlarge: true }),
        metadata: MetadataPolicy::None,
        remove_location: true,
        ..Default::default()
    }
    .to_json();
    let settings: serde_json::Value = serde_json::from_str(settings).unwrap_or(serde_json::Value::Null);
    let request = proto::Render { hash: hash.to_string(), name: name.to_string(), settings, export };
    let Some(_slot) = st.render.acquire(st.render.wait()) else {
        let mut r = error(503, "the server is rendering other pictures: try again in a moment");
        if let Some(h) = header("Retry-After", "30") {
            r.add_header(h);
        }
        return Err(r);
    };
    let scratch = st.data.join("tmp");
    match lightcraft_engine::guard::catch("rendering a shared picture", || crate::render::render_file(original, &scratch, &request)) {
        Ok(Ok(out)) => Ok(out.bytes),
        Ok(Err(e)) | Err(e) => {
            log::warn!("share: {name}: {e}");
            Err(error(422, "this photo can't be shown"))
        }
    }
}

/// The original file, with `Range` like the blob route.
fn original_file(req: &Request, path: &Path, name: &str) -> Resp {
    let Ok(mut f) = std::fs::File::open(path) else { return error(404, "the server has no file for this photo") };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let mut headers: Vec<Header> = [
        header("Content-Type", "application/octet-stream"),
        header("Content-Disposition", &format!("attachment; filename=\"{}\"", safe_name(name))),
        header("Accept-Ranges", "bytes"),
        header("Cache-Control", "private, max-age=300"),
    ]
    .into_iter()
    .flatten()
    .collect();
    let (status, start, n) = match header_value(req, "Range").and_then(|v| crate::api::range(v, len)) {
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
    let body: Box<dyn Read + Send> = Box::new(f.take(n));
    let mut r = Response::new(StatusCode(status), headers, body, usize::try_from(n).ok(), None);
    page_headers(&mut r, PAGE_CSP);
    r
}

/// `GET /s/<token>/download`: every original of the album in one ZIP (only on a link that offers the originals).
/// Photos whose file the server doesn't have (yet) are left out.
fn download(st: &State, user: &str, share: &Share) -> Resp {
    if !share.originals {
        return error(404, "this link doesn't offer the originals");
    }
    let (name, items) = match album_items(st, user, share.album) {
        Ok(x) => x,
        Err(r) => return r,
    };
    let l = match lib(st, user) {
        Ok(l) => l,
        Err(e) => return error(500, e),
    };
    let hashes: Vec<Option<String>> = {
        let l = l.lock().unwrap_or_else(PoisonError::into_inner);
        let c = l.core.catalog();
        items.iter().map(|it| c.photo(PhotoId(it.id)).and_then(|p| lightcraft_catalog::sync::content_key(p))).collect()
    };
    let mut found = Vec::new();
    for (it, hash) in items.iter().zip(hashes) {
        let Some(path) = hash.and_then(|h| original_of(st, &l, user, &h)) else { continue };
        let Ok(m) = std::fs::metadata(&path) else { continue };
        found.push((it, path, m.len()));
    }
    let names = crate::zip::unique_names(found.iter().map(|(it, ..)| it.name.as_str()));
    let entries: Vec<crate::zip::Entry> = found
        .into_iter()
        .zip(names)
        .map(|((it, path, size), name)| crate::zip::Entry { name: zip_name(&name), path, size, time: crate::zip::dos_time(&it.date) })
        .collect();
    let zip = crate::zip::Zip::new(entries);
    let len = usize::try_from(zip.len()).ok();
    let headers: Vec<Header> = [
        header("Content-Type", "application/zip"),
        header("Content-Disposition", &format!("attachment; filename=\"{}.zip\"", safe_name(&name))),
        header("Cache-Control", "private, no-store"),
    ]
    .into_iter()
    .flatten()
    .collect();
    let mut r = Response::new(StatusCode(200), headers, Box::new(zip) as Box<dyn Read + Send>, len, None);
    page_headers(&mut r, PAGE_CSP);
    r
}

/// A photo's file name as a name in a ZIP: no folders, nothing that climbs out of where it is unpacked.
fn zip_name(name: &str) -> String {
    let n: String = name.chars().map(|c| if c.is_control() || matches!(c, '/' | '\\' | ':') { '_' } else { c }).take(200).collect();
    let n = n.trim().trim_start_matches('.').to_string();
    if n.is_empty() { "photo".into() } else { n }
}

/// `/s/…`: the public side, without a sign-in.
pub fn public(st: &State, req: &mut Request, method: &Method, path: &str, q: &str) -> Resp {
    if !matches!(method, Method::Get | Method::Head | Method::Post) {
        return error(405, "GET, HEAD or POST");
    }
    let parts: Vec<String> = path.trim_start_matches("/s/").split('/').filter(|p| !p.is_empty()).map(str::to_string).collect();
    let Some(token) = parts.first().cloned() else { return error(404, "not found") };
    // guessing is throttled by address (tokens are 256 bits: this is for politeness)
    let ip = crate::throttle::client(req.remote_addr().copied(), header_value(req, "X-Forwarded-For"));
    let mut keys = vec![format!("share:{ip}")];
    if let Some(r) = crate::api::throttled(st, &keys) {
        return r;
    }
    let found = index(st).find(&token).map(|(u, l)| (u.to_string(), l.clone()));
    let Some((user, link)) = found else {
        st.throttle.lock().unwrap_or_else(PoisonError::into_inner).failed(&keys);
        return error(404, "this link doesn't exist, has ended, or was taken back");
    };
    let rest: Vec<&str> = parts.iter().skip(1).map(String::as_str).collect();
    let post = matches!(method, Method::Post);
    if let (true, ["unlock"]) = (post, rest.as_slice()) {
        return unlock(st, req, &token, &link, &mut keys);
    }
    if let Some(hash) = link.password_hash.as_deref()
        && !unlocked(req, &token, hash)
    {
        return match rest.as_slice() {
            [] if !post => locked_page(&token, false),
            _ => error(401, "type the album's password first"),
        };
    }
    match (post, rest.as_slice()) {
        (false, []) => gallery(st, req, &token, &user, &link, q),
        (true, ["upload"]) => crate::guest::upload(st, req, &token, &user, &link),
        (false, ["download"]) => download(st, &user, &link.share),
        (false, [photo]) => match photo.parse::<u64>() {
            Ok(p) => photo_page(st, &token, &user, &link, p, q),
            Err(_) => error(404, "not found"),
        },
        (false, [photo, kind]) => match (photo.parse::<u64>(), kind_of(kind)) {
            (Ok(p), Some(k)) => picture(st, req, &user, &link.share, p, k),
            _ => error(404, "not found"),
        },
        (true, [] | [_] | [_, _]) => error(405, "GET or HEAD"),
        _ => error(404, "not found"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_text_is_escaped() {
        assert_eq!(esc("<b>\"Tom\" & 'Jerry'</b>\n"), "&lt;b&gt;&quot;Tom&quot; &amp; &#39;Jerry&#39;&lt;/b&gt;");
    }

    #[test]
    fn file_names_are_safe_in_a_header_and_a_zip() {
        assert_eq!(safe_name("IMG 001.CR3"), "IMG_001.CR3");
        assert_eq!(safe_name("..\\x\"y\r\nz"), "photo.._x_y__z");
        assert_eq!(safe_name(""), "photo");
        assert!(safe_name(&"a".repeat(500)).len() <= 100);
        assert_eq!(zip_name("../../etc/passwd"), "_.._etc_passwd");
        assert_eq!(zip_name("C:\\x.jpg"), "C__x.jpg");
        assert_eq!(zip_name("..."), "photo");
        assert_eq!(zip_name("Été à Paris.jpg"), "Été à Paris.jpg");
    }

    fn share(id: &str, token: &str, expires: Option<u64>) -> Link {
        Link::from(Share {
            id: id.into(),
            token: token.into(),
            album: 1,
            name: "A".into(),
            created: 1,
            expires,
            originals: false,
            password: false,
            upload: false,
        })
    }

    #[test]
    fn ended_and_revoked_links_are_not_found() {
        let dir = std::env::temp_dir().join(format!("lc-shares-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut ix = Index::default();
        ix.add(&dir, "ann", share("a", "t1", None)).unwrap();
        ix.add(&dir, "ann", share("b", "t2", Some(1))).unwrap();
        assert!(ix.find("t1").is_some() && ix.find("t2").is_none() && ix.find("nope").is_none());
        assert_eq!(ix.list("ann").len(), 2);
        assert_eq!(ix.remove(&dir, "ann", "a"), Ok(true));
        assert_eq!(ix.remove(&dir, "ann", "a"), Ok(false));
        assert_eq!(ix.remove(&dir, "bob", "b"), Ok(false), "only the owner revokes");
        assert!(ix.find("t1").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn links_from_before_passwords_still_read_and_hashes_stay_on_the_server() {
        let old = r#"{"shares":[{"id":"a","token":"t","album":3,"name":"Trip","created":5,"expires":null,"originals":true}]}"#;
        let f: File = serde_json::from_str(old).unwrap();
        let l = &f.shares[0];
        assert_eq!((l.share.album, l.share.originals, l.share.password, l.share.upload, l.password_hash.is_none()), (3, true, false, false, true));
        let mut l = l.clone();
        l.password_hash = Some("$argon2id$secret".into());
        l.share.password = true;
        let kept = serde_json::to_string(&File { shares: vec![l.clone()] }).unwrap();
        assert!(kept.contains("password_hash"), "kept in shares.json");
        let sent = serde_json::to_string(&l.share).unwrap();
        assert!(!sent.contains("argon2") && sent.contains("\"password\":true"), "devices never see the hash: {sent}");
    }

    #[test]
    fn unlock_cookies_open_one_link_with_one_password() {
        let v = unlock_value("tok", "hash1");
        assert_eq!(v.len(), 64);
        assert_ne!(v, unlock_value("tok", "hash2"));
        assert_ne!(v, unlock_value("tok2", "hash1"));
        assert_eq!(form_field("a=1&password=p%C3%A4ss+word%21&b=2", "password").as_deref(), Some("päss word!"));
        assert_eq!(form_field("password=%zz%4", "password").as_deref(), Some("%zz%4"));
        assert_eq!(form_field("x=1", "password"), None);
    }

    #[test]
    fn dates_read_as_days() {
        assert_eq!(day_label("2026-03-12T14:03:22"), "Thursday 12 March 2026");
        assert_eq!(day_label("garbage"), "garbage");
        assert_eq!(span_label("2026-03-12T10:00:00", "2026-03-12T18:00:00"), "12 March 2026");
        assert_eq!(span_label("2026-03-12", "2026-03-19"), "12 – 19 March 2026");
        assert_eq!(span_label("2026-03-30", "2026-04-02"), "30 March – 2 April 2026");
        assert_eq!(span_label("2025-12-30", "2026-01-02"), "30 December 2025 – 2 January 2026");
        assert_eq!(span_label("x", "2026-01-02"), "");
    }

    #[test]
    fn the_script_hash_is_base64_of_sha256() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert!(viewer_csp().contains("script-src 'sha256-") && viewer_csp().ends_with("='"));
    }
}
