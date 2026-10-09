//! Self-hosted sync (`sync.*`, see [`crate::sync`] and `docs/sync.md`): sign in to a LightCraft
//! server, sync now, pause, status; Make Available Offline for albums and photos; keep originals
//! on this device.

use lightcraft_catalog::{AlbumId, PhotoId};
use serde_json::{Value, json};

use super::{CommandSpec, always, bad, bool_or, cmd, str_param};
use crate::{LibrarySource, Result, Session};

/// The library was signed in to a server once (signed in now or not).
fn synced(s: &Session) -> std::result::Result<(), String> {
    match s.sync_state() {
        Some(st) if !st.config.library.is_empty() || st.signed_in() => Ok(()),
        _ => Err("this library isn't synced (Settings → Sync)".into()),
    }
}

fn signed_in(s: &Session) -> std::result::Result<(), String> {
    synced(s)?;
    if s.sync_state().is_some_and(|st| st.signed_in()) { Ok(()) } else { Err("not signed in to the sync server".into()) }
}

fn synced_selection(s: &Session) -> std::result::Result<(), String> {
    synced(s)?;
    super::has_selection(s)
}

fn status(s: &mut Session, _: &Value) -> Result<Value> {
    Ok(s.sync_state().map(|st| st.status()).unwrap_or_else(|| json!({"state": "off", "signedIn": false})))
}

/// What this library takes on the server and on this computer. The server's numbers are the last
/// ones it gave (`refresh`: ask it now and wait, natively); this computer's are measured here.
fn usage(s: &mut Session, p: &Value) -> Result<Value> {
    if bool_or(p, "refresh", false) {
        #[cfg(not(target_arch = "wasm32"))]
        s.sync_fetch_usage();
        // (the browser can't wait here: its requests finish on later frames)
        #[cfg(target_arch = "wasm32")]
        s.sync_refresh_usage();
    } else {
        s.sync_want_usage();
    }
    let (photos, remote) = s.photo_counts();
    let st = s.sync_state();
    Ok(json!({
        "server": st.and_then(|st| st.usage()),
        "serverError": st.and_then(|st| st.usage_error()),
        "serverAgeSecs": st.and_then(|st| st.usage_age()).map(|a| a.as_secs()),
        "local": s.local_dirs().map(|d| crate::usage::measure(&d)),
        "photos": {"total": photos, "onlyPreviewsHere": remote},
    }))
}

fn sign_in(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "sync.signIn";
    let server = str_param(p, "server").ok_or_else(|| bad(C, "missing `server` (https://…)"))?;
    let user = str_param(p, "user").ok_or_else(|| bad(C, "missing `user`"))?;
    let password = str_param(p, "password").ok_or_else(|| bad(C, "missing `password`"))?;
    let device = str_param(p, "device").filter(|d| !d.trim().is_empty()).map(str::to_string).unwrap_or_else(default_device_name);
    s.sync_sign_in(server, user, password, &device)?;
    s.sync_merge_on_join(bool_or(p, "merge", false));
    status(s, p)
}

/// A name for this device on the server: `$LIGHTCRAFT_DEVICE` (the iOS host sets the device's
/// name), else the computer's name.
fn default_device_name() -> String {
    let named = ["LIGHTCRAFT_DEVICE", "HOSTNAME", "COMPUTERNAME"].iter().find_map(|v| std::env::var(v).ok().filter(|h| !h.trim().is_empty()));
    named.or_else(computer_name).unwrap_or_else(|| format!("LightCraft ({})", std::env::consts::OS))
}

/// The computer's name as its system shows it (macOS: "Ann's MacBook Pro"; elsewhere the host
/// name), when it can be asked.
fn computer_name() -> Option<String> {
    #[cfg(not(any(target_arch = "wasm32", target_os = "ios")))]
    {
        let ask = |cmd: &str, args: &[&str]| {
            let out = std::process::Command::new(cmd).args(args).output().ok().filter(|o| o.status.success())?;
            let name: String = String::from_utf8_lossy(&out.stdout).trim().chars().filter(|c| !c.is_control()).take(100).collect();
            (!name.is_empty()).then_some(name)
        };
        if cfg!(target_os = "macos")
            && let Some(n) = ask("scutil", &["--get", "ComputerName"])
        {
            return Some(n);
        }
        ask("hostname", &[])
    }
    #[cfg(any(target_arch = "wasm32", target_os = "ios"))]
    None
}

fn sign_out(s: &mut Session, p: &Value) -> Result<Value> {
    s.sync_sign_out()?;
    status(s, p)
}

fn now(s: &mut Session, p: &Value) -> Result<Value> {
    let wait = bool_or(p, "wait", false);
    #[cfg(not(target_arch = "wasm32"))]
    if wait {
        let limit = p.get("limit").and_then(Value::as_u64).unwrap_or(100_000) as usize;
        return Ok(s.sync_now(limit));
    }
    // (the browser can't wait here: its requests finish on later frames)
    #[cfg(target_arch = "wasm32")]
    let _ = wait;
    s.sync_soon();
    status(s, p)
}

fn pause(s: &mut Session, p: &Value) -> Result<Value> {
    let on = match p.get("on").and_then(Value::as_bool) {
        Some(on) => on,
        None => !s.sync_state().is_some_and(|st| st.config.paused),
    };
    s.sync_pause(on)?;
    status(s, p)
}

fn store_originals(s: &mut Session, p: &Value) -> Result<Value> {
    let on = p.get("on").and_then(Value::as_bool).ok_or_else(|| bad("sync.storeOriginalsLocally", "missing `on` (true or false)"))?;
    s.sync_store_originals(on)?;
    status(s, p)
}

fn originals_budget(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "sync.originalsBudget";
    let mb = match p.get("mb") {
        Some(Value::Null) => None,
        Some(v) => Some(v.as_u64().filter(|n| *n > 0).ok_or_else(|| bad(C, "`mb` is a number of megabytes (1 or more), or null for no limit"))?),
        None => return Err(bad(C, "missing `mb` (megabytes, or null for no limit)")),
    };
    s.sync_originals_budget(mb)?;
    status(s, p)
}

fn server_previews(s: &mut Session, p: &Value) -> Result<Value> {
    let on = p.get("on").and_then(Value::as_bool).ok_or_else(|| bad("sync.serverPreviews", "missing `on` (true or false)"))?;
    s.sync_server_previews(on)?;
    status(s, p)
}

fn album_share(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "album.share";
    let id = match p.get("id").and_then(Value::as_u64) {
        Some(id) => AlbumId(id),
        None => match s.source {
            LibrarySource::Album(a) => a,
            _ => return Err(bad(C, "missing `id` (or show an album first)")),
        },
    };
    let days = p.get("expiresDays").and_then(Value::as_u64).map(|d| d.min(3650) as u32).filter(|d| *d > 0);
    s.sync_share_album(id, days, bool_or(p, "originals", false))?;
    Ok(json!({"started": true, "album": id.0}))
}

fn album_unshare(s: &mut Session, p: &Value) -> Result<Value> {
    let id = str_param(p, "id").ok_or_else(|| bad("album.unshare", "missing `id` (a link's id, see shares.list)"))?.to_string();
    s.sync_unshare(&id)?;
    Ok(json!({"started": true, "link": id}))
}

fn shares_list(s: &mut Session, p: &Value) -> Result<Value> {
    s.sync_refresh_shares(bool_or(p, "refresh", false));
    let (links, error) = s.sync_shares();
    let links: Vec<Value> = links
        .into_iter()
        .map(|(l, url)| json!({"id": l.id, "album": l.album, "name": l.name, "url": url, "created": l.created, "expires": l.expires, "originals": l.originals}))
        .collect();
    Ok(json!({"shares": links, "error": error}))
}

fn download_originals(s: &mut Session, p: &Value) -> Result<Value> {
    let ids = s.targets(p);
    let n = s.sync_want_originals(&ids);
    Ok(json!({"queued": n}))
}

fn album_offline(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "album.makeAvailableOffline";
    let id = match p.get("id").and_then(Value::as_u64) {
        Some(id) => AlbumId(id),
        None => match s.source {
            LibrarySource::Album(a) => a,
            _ => return Err(bad(C, "missing `id` (or show an album first)")),
        },
    };
    if s.catalog.album(id).is_none() {
        return Err(bad(C, format!("no album {}", id.0)));
    }
    let on = match p.get("on").and_then(Value::as_bool) {
        Some(on) => on,
        None => !s.sync_state().is_some_and(|st| st.config.offline_albums.contains(&id)),
    };
    s.sync_offline(&[id], &[], on)?;
    Ok(json!({"album": id.0, "offline": on}))
}

fn photo_offline(s: &mut Session, p: &Value) -> Result<Value> {
    let ids: Vec<PhotoId> = s.targets(p).into_iter().filter(|id| s.catalog.photo(*id).is_some()).collect();
    if ids.is_empty() {
        return Err(bad("photo.makeAvailableOffline", "no photos"));
    }
    let on = match p.get("on").and_then(Value::as_bool) {
        Some(on) => on,
        None => !ids.iter().all(|id| s.sync_state().is_some_and(|st| st.config.offline_photos.contains(id))),
    };
    s.sync_offline(&[], &ids, on)?;
    Ok(json!({"photos": ids.len(), "offline": on}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        // not journaled: the parameters carry a password
        cmd!(query "sync.signIn", "Sign In to Sync", [], None,
            "{server: \"https://…\", user, password, device?, merge?: bool}: share this library with a LightCraft server (docs/sync.md). An empty server library gets this one; a library with photos can join a server that has some only with `merge` (photos matched by content, nothing removed on either side; without it the error says what a merge would do) — or sign in from a new library to just get the server's photos",
            always, sign_in),
        cmd!(
            "sync.signOut",
            "Sign Out of Sync",
            [],
            None,
            "{}: stop syncing; the library and its pending changes stay (signing in again resumes)",
            signed_in,
            sign_out
        ),
        cmd!(
            "sync.now",
            "Sync Now",
            ["File"],
            None,
            "{wait?: bool, limit?}: pull and push now (wait: run until done here; CLI / MCP)",
            signed_in,
            now
        ),
        cmd!("sync.pause", "Pause Syncing", ["File"], None, "{on?: bool}: stop talking to the server for now (toggle without `on`)", synced, pause),
        cmd!(query "sync.status", "Sync Status", [], None, "{}: state (off / signedOut / idle / syncing / error), pending changes, transfers, error", always, status),
        cmd!(query "sync.usage", "Sync Storage", [], None,
            "{refresh?: bool}: what this library takes on the sync server (originals, smart and small previews, library folders, the server's free disk) and on this computer (catalog, thumbnails, previews, downloaded originals, free disk), in bytes; photos with only previews here. The server's numbers are the last it gave (asked for again every 20 s while someone looks) unless `refresh` asks it now",
            synced, usage),
        cmd!(
            "sync.storeOriginalsLocally",
            "Store Originals Locally",
            [],
            None,
            "{on: bool}: keep every photo's original on this device too",
            synced,
            store_originals
        ),
        cmd!(
            "sync.originalsBudget",
            "Limit Downloaded Originals",
            [],
            None,
            "{mb: number | null}: keep the originals downloaded from the server under this many megabytes by deleting the ones used longest ago (never one the server doesn't have, one that is open, selected or available offline; not with Store Originals Locally); null: no limit",
            synced,
            originals_budget
        ),
        cmd!(
            "sync.serverPreviews",
            "Server Builds Previews",
            [],
            None,
            "{on: bool}: have the sync server build the previews of the originals this device uploads (default on iOS and in the browser: the phone spares its battery and memory, the page its main thread); a server that can't builds none and this device does",
            synced,
            server_previews
        ),
        cmd!(
            "album.share",
            "Share Link",
            [],
            None,
            "{id?, expiresDays?: n, originals?: bool}: make a link to an album (default: the album shown) that anyone can open in a browser, no account needed: its pictures as they are edited now, and the originals if `originals`; ends after `expiresDays` or when revoked. The address arrives with the next sync (sync.now wait=true), then shares.list shows it",
            signed_in,
            album_share
        ),
        cmd!(
            "album.unshare",
            "Revoke Link",
            [],
            None,
            "{id: link id from shares.list}: take an album link back; it stops working at once",
            signed_in,
            album_unshare
        ),
        cmd!(query "shares.list", "Shared Links", [], None,
            "{refresh?: bool} → {shares: [{id, album, name, url, created, expires, originals}], error}: this user's album links (asked of the server once, then kept up to date; `refresh` asks again)",
            signed_in, shares_list),
        cmd!(
            "sync.downloadOriginals",
            "Download Originals",
            ["Photo"],
            None,
            "{ids?}: download these photos' originals to this device (default: the selection)",
            synced_selection,
            download_originals
        ),
        cmd!(
            "album.makeAvailableOffline",
            "Make Album Available Offline",
            ["File"],
            None,
            "{id?, on?}: keep the album's smart previews on this device (toggle without `on`; default: the album shown)",
            synced,
            album_offline
        ),
        cmd!(
            "photo.makeAvailableOffline",
            "Make Available Offline",
            ["Photo"],
            None,
            "{ids?, on?}: keep these photos' smart previews on this device (toggle without `on`; default: the selection)",
            synced_selection,
            photo_offline
        ),
    ]
}
