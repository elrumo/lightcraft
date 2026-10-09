//! Album links from the device's side (`docs/sync.md` → *Share an album*): ask the server for a link to
//! an album (`POST /api/shares`), list them, revoke one. The server makes the link and serves the page; this
//! keeps the last list, and the address of the link just made until the app has put it on the clipboard.
//!
//! The requests go out like the rest of the sync's ([`Task`]), so they work in the app, in a browser tab and
//! from the command line (`album.share`, then `sync.now wait=true`, then `shares.list`).

use std::collections::{HashMap, VecDeque};

use lightcraft_catalog::AlbumId;
use lightcraft_catalog::sync::proto;
use serde_json::Value;

use super::{Body, Done, SyncState, Task, decode, why};
use crate::{EngineError, Result, Session};

/// What a request in flight is for.
#[derive(Debug)]
pub(super) enum Kind {
    List,
    Create,
    Revoke(String),
}

#[derive(Default)]
pub struct ShareState {
    list: Vec<proto::Share>,
    /// Why the last request failed.
    error: Option<String>,
    /// The address of the link just made, until [`Session::sync_take_new_link`].
    latest: Option<String>,
    /// Requests asked of the host and not answered yet.
    tasks: HashMap<u64, Kind>,
    /// Requests waiting to be handed to the host.
    pub(super) ready: VecDeque<Task>,
    /// The list was asked for (or came): once is enough, the answers to the other requests keep it up to date.
    asked: bool,
}

impl ShareState {
    /// What request `id` was for, if it was one about links.
    pub(super) fn take_kind(&mut self, id: u64) -> Option<Kind> {
        self.tasks.remove(&id)
    }
}

impl SyncState {
    /// The address of a link: the server's, `/s/<token>`.
    fn link(&self, token: &str) -> String {
        format!("{}/s/{token}", self.config.server.trim_end_matches('/'))
    }

    fn queue_share(&mut self, kind: Kind, method: &'static str, path: &str, body: Body) {
        let t = self.http(method, path, body, None);
        self.shares.tasks.insert(t.id(), kind);
        self.shares.ready.push_back(t);
    }
}

fn not_signed_in() -> EngineError {
    EngineError::Other("not signed in to the sync server".into())
}

impl Session {
    /// Ask the server for a link to album `album` (a plain album: not a folder or a smart album). The new link
    /// arrives with a later [`Session::sync_done`]; see [`Session::sync_take_new_link`].
    pub fn sync_share_album(&mut self, album: AlbumId, expires_days: Option<u32>, originals: bool) -> Result<()> {
        match self.catalog.album(album) {
            None => return Err(EngineError::Other(format!("no album {}", album.0))),
            Some(a) if a.folder || a.is_smart() => {
                return Err(EngineError::Other("only an album of photos can be shared: not a folder or a smart album".into()));
            }
            Some(_) => {}
        }
        let st = self.sync.as_mut().filter(|st| st.signed_in()).ok_or_else(not_signed_in)?;
        let body =
            serde_json::to_string(&proto::NewShare { album: album.0, expires_days, originals }).map_err(|e| EngineError::Other(e.to_string()))?;
        st.shares.error = None;
        st.queue_share(Kind::Create, "POST", "/api/shares", Body::Json(body));
        Ok(())
    }

    /// Take link `id` back: it stops working at once.
    pub fn sync_unshare(&mut self, id: &str) -> Result<()> {
        if id.is_empty() || id.len() > 64 || !id.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(EngineError::Other(format!("not a link id: `{id}`")));
        }
        let st = self.sync.as_mut().filter(|st| st.signed_in()).ok_or_else(not_signed_in)?;
        st.shares.error = None;
        st.queue_share(Kind::Revoke(id.to_string()), "DELETE", &format!("/api/shares/{id}"), Body::Empty);
        Ok(())
    }

    /// Ask for the list of links again (once is enough unless `again`: the answers to making and revoking keep it).
    pub fn sync_refresh_shares(&mut self, again: bool) {
        let Some(st) = self.sync.as_mut().filter(|st| st.signed_in()) else { return };
        let in_flight = st.shares.tasks.values().any(|k| matches!(k, Kind::List));
        if in_flight || (st.shares.asked && !again) {
            return;
        }
        st.shares.asked = true;
        st.queue_share(Kind::List, "GET", "/api/shares", Body::Empty);
    }

    /// The links this user has, with their addresses, and why the last request about them failed.
    pub fn sync_shares(&self) -> (Vec<(proto::Share, String)>, Option<&str>) {
        let Some(st) = &self.sync else { return (Vec::new(), None) };
        (st.shares.list.iter().map(|s| (s.clone(), st.link(&s.token))).collect(), st.shares.error.as_deref())
    }

    /// The address of the link made since the last call, once the server has made it.
    pub fn sync_take_new_link(&mut self) -> Option<String> {
        self.sync.as_mut()?.shares.latest.take()
    }

    /// The answer to a request about links.
    pub(super) fn share_done(&mut self, st: &mut SyncState, kind: Kind, d: &Done) {
        if d.status == 401 {
            st.signed_out("the server signed this device out: sign in again");
            return;
        }
        let failed = |st: &mut SyncState, d: &Done| {
            st.shares.error = Some(match d.status {
                404 | 405 if d.body.contains("no route") => "this server is too old to share links: update it".to_string(),
                _ => serde_json::from_str::<Value>(&d.body).ok().and_then(|v| v["error"].as_str().map(str::to_string)).unwrap_or_else(|| why(d)),
            });
        };
        match kind {
            Kind::List if d.ok() => match decode::<ListBody>(d) {
                Ok(b) => st.shares.list = b.shares,
                Err(e) => st.shares.error = Some(e),
            },
            Kind::Create if d.ok() => match decode::<proto::Share>(d) {
                Ok(s) => {
                    st.shares.latest = Some(st.link(&s.token));
                    st.shares.list.push(s);
                }
                Err(e) => st.shares.error = Some(e),
            },
            // (gone already is as good as revoked)
            Kind::Revoke(id) if d.ok() || d.status == 404 => st.shares.list.retain(|s| s.id != id),
            _ => failed(st, d),
        }
    }
}

#[derive(serde::Deserialize)]
struct ListBody {
    #[serde(default)]
    shares: Vec<proto::Share>,
}
