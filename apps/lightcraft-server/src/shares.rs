//! Read-only album links: a person gives someone a link to an album and that someone sees it in a browser,
//! with no account and nothing to install (`https://photos.example.com/s/<token>`).
//!
//! - A link is a 256-bit random token the owner makes for one of their albums (`POST /api/shares`), optionally
//!   ending after some days and optionally letting the visitor download the originals. The owner lists and
//!   revokes their links (`GET /api/shares`, `DELETE /api/shares/<id>`; also on the admin page). They are kept in
//!   `users/<name>/shares.json`, which only the server reads (the tokens are in it: whoever has one has the link).
//! - A visitor gets a plain page of the album's photos ([`gallery`]) and the pictures ([`picture`]): each photo
//!   **rendered with the edits it has now** at 400 px (the grid) and 1600 px (the view), by the same pipeline an
//!   export uses and without its metadata (no GPS, no camera serial number), or — if the link allows it — the
//!   original file. A rendered picture is kept under `users/<name>/shares/cache/` until the edits change.
//! - Only the album's photos are reachable through its link (a photo id from another album, or a deleted photo,
//!   is `404`), and only static albums can be shared (not folders or smart albums). Unknown, expired and revoked
//!   tokens are all `404`, and guessing is throttled by address.
//!
//! The pages carry their own styles and no script; nothing of the visitor's is stored; the page asks search
//! engines to leave it alone.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::PoisonError;
use std::time::Duration;

use lightcraft_catalog::AlbumId;
use lightcraft_catalog::sync::proto::{self, Share};
use lightcraft_engine::export::{ExportFormat, ExportOptions, MetadataPolicy, Resize, ResizeMode};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::State;
use crate::accounts;
use crate::api::{Resp, bytes, error, header, header_value, json as json_resp, lib};
use crate::http::{Header, Method, Request, Response, StatusCode};

/// Links one user may have at once.
pub const MAX_PER_USER: usize = 200;
/// The longest a link may last, in days.
pub const MAX_DAYS: u32 = 3650;
const THUMB_EDGE: usize = 400;
const VIEW_EDGE: usize = 1600;
/// How long a picture waits for a free place to render before the visitor's browser is told to try again.
const RENDER_WAIT: Duration = Duration::from_secs(45);

/// `users/<name>/shares.json`.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct File {
    shares: Vec<Share>,
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
    by_token: HashMap<String, (String, Share)>,
}

impl Index {
    /// Read every user's links.
    pub fn load(data: &Path) -> Index {
        let mut by_token = HashMap::new();
        for user in accounts::read_users(data).map(|f| f.users.into_keys().collect::<Vec<_>>()).unwrap_or_default() {
            for s in read_file(data, &user).shares {
                by_token.insert(s.token.clone(), (user.clone(), s));
            }
        }
        Index { by_token }
    }

    /// The link with this token, if it exists and hasn't ended.
    pub fn find(&self, token: &str) -> Option<(&str, &Share)> {
        let (user, s) = self.by_token.get(token)?;
        s.expires.is_none_or(|e| accounts::now() < e).then_some((user.as_str(), s))
    }

    /// A user's links, oldest first.
    pub fn list(&self, user: &str) -> Vec<Share> {
        let mut v: Vec<Share> = self.by_token.values().filter(|(u, _)| u == user).map(|(_, s)| s.clone()).collect();
        v.sort_by(|a, b| (a.created, &a.id).cmp(&(b.created, &b.id)));
        v
    }

    pub fn add(&mut self, data: &Path, user: &str, share: Share) -> Result<(), String> {
        let mut f = read_file(data, user);
        if f.shares.len() >= MAX_PER_USER {
            return Err(format!("at most {MAX_PER_USER} links: revoke some first"));
        }
        f.shares.push(share.clone());
        write_file(data, user, &f)?;
        self.by_token.insert(share.token.clone(), (user.to_string(), share));
        Ok(())
    }

    /// Revoke link `id` of `user`. `false`: they have no such link.
    pub fn remove(&mut self, data: &Path, user: &str, id: &str) -> Result<bool, String> {
        let mut f = read_file(data, user);
        let before = f.shares.len();
        f.shares.retain(|s| s.id != id);
        if f.shares.len() == before {
            return Ok(false);
        }
        write_file(data, user, &f)?;
        self.by_token.retain(|_, (u, s)| !(u == user && s.id == id));
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
    let now = accounts::now();
    let expires = n.expires_days.filter(|d| *d > 0).map(|d| now.saturating_add(u64::from(d.min(MAX_DAYS)) * 86_400));
    let share = Share { id, token, album: n.album, name, created: now, expires, originals: n.originals };
    match index(st).add(&st.data, user, share.clone()) {
        Ok(()) => {
            log::info!("{user}: shared album {} ({})", share.album, share.name);
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

fn esc(s: &str) -> String {
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
fn safe_name(name: &str) -> String {
    let n: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' }).take(100).collect();
    if n.is_empty() || n.starts_with('.') { format!("photo{n}") } else { n }
}

/// What every page and picture of a link says about itself.
fn page_headers(r: &mut Resp) {
    for (k, v) in [
        ("X-Robots-Tag", "noindex, nofollow, noarchive"),
        ("Referrer-Policy", "no-referrer"),
        ("X-Content-Type-Options", "nosniff"),
        (
            "Content-Security-Policy",
            "default-src 'none'; img-src 'self'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
        ),
        ("Cross-Origin-Resource-Policy", "same-origin"),
    ] {
        if let Some(h) = header(k, v) {
            r.add_header(h);
        }
    }
}

const STYLE: &str = "\
:root{color-scheme:light dark;--bg:#fafafa;--fg:#1c1c1e;--mute:#6e6e73;--line:#d8d8dc}\
@media(prefers-color-scheme:dark){:root{--bg:#1c1c1e;--fg:#f2f2f7;--mute:#98989f;--line:#3a3a3c}}\
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--fg);font:16px/1.4 system-ui,-apple-system,Segoe UI,sans-serif}\
header,footer{padding:20px 16px;max-width:1400px;margin:0 auto}h1{margin:0;font-size:1.5rem;font-weight:600}\
p{margin:4px 0 0;color:var(--mute)}main{display:grid;grid-template-columns:repeat(auto-fill,minmax(180px,1fr));gap:6px;padding:0 16px;max-width:1400px;margin:0 auto}\
figure{margin:0;position:relative;background:var(--line);aspect-ratio:1/1;overflow:hidden;border-radius:4px}\
figure a.pic{display:block;width:100%;height:100%}img{width:100%;height:100%;object-fit:cover;display:block}\
figcaption{position:absolute;right:6px;bottom:6px}figcaption a{font-size:.75rem;background:rgba(0,0,0,.6);color:#fff;text-decoration:none;padding:2px 8px;border-radius:99px}\
footer{color:var(--mute);font-size:.8rem}a:focus-visible{outline:2px solid #0a84ff;outline-offset:2px}";

/// `GET /s/<token>`: the album as a page.
fn gallery(st: &State, token: &str, user: &str, share: &Share) -> Resp {
    let l = match lib(st, user) {
        Ok(l) => l,
        Err(e) => return error(500, e),
    };
    let (name, photos) = {
        let l = l.lock().unwrap_or_else(PoisonError::into_inner);
        let c = l.core.catalog();
        let Some(album) = c.album(AlbumId(share.album)).filter(|a| !a.is_smart() && !a.folder) else {
            return error(404, "this link's album is gone");
        };
        let photos: Vec<(u64, String)> =
            album.photos.iter().filter_map(|id| c.photo(*id)).filter(|p| !p.deleted && !p.local).map(|p| (p.id.0, p.file_name.clone())).collect();
        (album.name.clone(), photos)
    };
    let mut html = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <meta name=\"robots\" content=\"noindex,nofollow\"><title>{t}</title><style>{STYLE}</style></head><body><header><h1>{t}</h1><p>{n} photo{s}</p></header><main>",
        t = esc(&name),
        n = photos.len(),
        s = if photos.len() == 1 { "" } else { "s" },
    );
    for (id, file) in &photos {
        let f = esc(file);
        html.push_str(&format!(
            "<figure><a class=\"pic\" href=\"/s/{token}/{id}/view\"><img loading=\"lazy\" alt=\"{f}\" src=\"/s/{token}/{id}/thumb\"></a>"
        ));
        if share.originals {
            html.push_str(&format!("<figcaption><a href=\"/s/{token}/{id}/original\" download>Original</a></figcaption>"));
        }
        html.push_str("</figure>");
    }
    html.push_str("</main><footer>Shared with LightCraft</footer></body></html>");
    let mut r = bytes(200, "text/html; charset=utf-8", html.into_bytes());
    page_headers(&mut r);
    if let Some(h) = header("Cache-Control", "private, no-cache") {
        r.add_header(h);
    }
    r
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
        let Some(p) = c.photo(lightcraft_catalog::PhotoId(photo)).filter(|p| in_album && !p.deleted && !p.local) else {
            return error(404, "no such photo here");
        };
        let Some(hash) = lightcraft_catalog::sync::content_key(p) else { return error(404, "the server has no file for this photo") };
        (hash, p.file_name.clone(), serde_json::to_string(&*p.develop).unwrap_or_default())
    };
    let dir = crate::accounts::user_dir(&st.data, user);
    let Some(original) = crate::api::blob_path(&dir.join("blobs"), "original", &hash)
        .filter(|b| b.is_file())
        .or_else(|| crate::api::folder_original_path(st, &l, user, &hash))
    else {
        return error(404, "the server has no file for this photo");
    };
    match kind {
        Kind::Original => original_file(req, &original, &name),
        Kind::Edge(edge) => {
            // kept until the photo's edits (or its file) change
            let key = lightcraft_preview::Hasher128::new().str(&settings).str(&hash).finish();
            let cache = dir.join("shares").join("cache").join(format!("{:032x}-{edge}.jpg", key.0));
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
            page_headers(&mut r);
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
    let Some(_slot) = st.render.acquire(RENDER_WAIT) else {
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
    page_headers(&mut r);
    r
}

/// `/s/…`: the public side, without a sign-in.
pub fn public(st: &State, req: &Request, method: &Method, path: &str) -> Resp {
    if !matches!(method, Method::Get | Method::Head) {
        return error(405, "GET or HEAD");
    }
    let parts: Vec<&str> = path.trim_start_matches("/s/").split('/').filter(|p| !p.is_empty()).collect();
    let Some(token) = parts.first().copied() else { return error(404, "not found") };
    // guessing is throttled by address (tokens are 256 bits: this is for politeness)
    let ip = crate::throttle::client(req.remote_addr().copied(), header_value(req, "X-Forwarded-For"));
    let keys = vec![format!("share:{ip}")];
    if let Some(r) = crate::api::throttled(st, &keys) {
        return r;
    }
    let found = index(st).find(token).map(|(u, s)| (u.to_string(), s.clone()));
    let Some((user, share)) = found else {
        st.throttle.lock().unwrap_or_else(PoisonError::into_inner).failed(&keys);
        return error(404, "this link doesn't exist, has ended, or was taken back");
    };
    match parts.as_slice() {
        [_] => gallery(st, token, &user, &share),
        [_, photo, kind] => match (photo.parse::<u64>(), kind_of(kind)) {
            (Ok(p), Some(k)) => picture(st, req, &user, &share, p, k),
            _ => error(404, "not found"),
        },
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
    fn file_names_are_safe_in_a_header() {
        assert_eq!(safe_name("IMG 001.CR3"), "IMG_001.CR3");
        assert_eq!(safe_name("..\\x\"y\r\nz"), "photo.._x_y__z");
        assert_eq!(safe_name(""), "photo");
        assert!(safe_name(&"a".repeat(500)).len() <= 100);
    }

    #[test]
    fn ended_and_revoked_links_are_not_found() {
        let dir = std::env::temp_dir().join(format!("lc-shares-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut ix = Index::default();
        let live = Share { id: "a".into(), token: "t1".into(), album: 1, name: "A".into(), created: 1, expires: None, originals: false };
        let old = Share { id: "b".into(), token: "t2".into(), album: 1, name: "A".into(), created: 2, expires: Some(1), originals: false };
        ix.add(&dir, "ann", live).unwrap();
        ix.add(&dir, "ann", old).unwrap();
        assert!(ix.find("t1").is_some() && ix.find("t2").is_none() && ix.find("nope").is_none());
        assert_eq!(ix.list("ann").len(), 2);
        assert_eq!(ix.remove(&dir, "ann", "a"), Ok(true));
        assert_eq!(ix.remove(&dir, "ann", "a"), Ok(false));
        assert_eq!(ix.remove(&dir, "bob", "b"), Ok(false), "only the owner revokes");
        assert!(ix.find("t1").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
