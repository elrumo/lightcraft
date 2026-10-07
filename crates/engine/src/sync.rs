//! Self-hosted sync: this library shared with the user's other devices through a LightCraft
//! server (`apps/lightcraft-server`, `docs/sync.md`). The rules — one order kept by the server,
//! a keyed outbox, merges, device-local state — are in [`lightcraft_catalog::sync`]; this is the
//! device side:
//!
//! - **What to send next and what came back** ([`Session::sync_tasks`], [`Session::sync_done`])
//!   without doing any I/O: the host runs the [`Task`]s (natively [`run`] on a worker thread, the
//!   browser with `fetch`), so a slow network never blocks the UI. [`Session::sync_now`] runs
//!   them in place (CLI, tests).
//! - **Pixels by content hash.** This device uploads the originals it has, with a smart preview
//!   and a mini preview (≤ 512 px) it builds from each. It downloads the mini previews of the
//!   other photos (grid thumbnails, rendered here with the current edits), the smart preview of
//!   the photo being looked at and of everything made available offline, and originals when
//!   asked to keep them. A downloaded original becomes the photo's file on this device.
//!
//! Files in the library: `sync.json` ([`SyncConfig`]), `sync.outbox` (changes the server hasn't
//! acknowledged; written before the op log) and `sync.uploaded` (content hashes the server has).

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::time::Duration;

use lightcraft_catalog::sync::{Outbox, PATH_PREFIX, Pushed, carry_local, original_path, proto};
use lightcraft_catalog::{AlbumId, Catalog, Op, Photo, PhotoId, Source, Store};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use web_time::Instant;

use crate::{EngineError, Result, Session};

pub const CONFIG: &str = "sync.json";
pub const OUTBOX: &str = "sync.outbox";
pub const UPLOADED: &str = "sync.uploaded";

/// How often the server is asked for other devices' changes.
pub const POLL: Duration = Duration::from_secs(5);
/// Longest wait between retries after a failure (network down, server away).
const BACKOFF_MAX: Duration = Duration::from_secs(60);
/// A missing or failed blob is tried again after this long.
const BLOB_RETRY: Duration = Duration::from_secs(60);
/// Ops per push / pull.
const PUSH_LIMIT: usize = 500;
const PULL_LIMIT: usize = 2000;
/// Blob transfers at a time.
const BLOB_PARALLEL: usize = 4;
/// Edge of the mini preview.
pub const MINI_EDGE: usize = 512;

/// This library's sync settings (`sync.json`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SyncConfig {
    /// The server's URL (`https://photos.example.com`).
    pub server: String,
    pub user: String,
    /// This device's token; empty when signed out.
    pub token: String,
    pub device: u64,
    /// The id space this device allocates new ids in.
    pub space: u32,
    /// The server library this one is a copy of (empty: never synced).
    pub library: String,
    /// The newest server op applied here.
    pub cursor: u64,
    /// Kept on this device even offline: smart previews (and originals with
    /// [`SyncConfig::store_originals`]).
    pub offline_albums: Vec<AlbumId>,
    pub offline_photos: Vec<PhotoId>,
    /// Keep the original of every photo on this device too (not just the ones available offline).
    pub store_originals: bool,
}

/// A photo file kept on the server, by content hash.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Blob {
    Original,
    Smart,
    Mini,
}

impl Blob {
    pub fn name(self) -> &'static str {
        match self {
            Blob::Original => "original",
            Blob::Smart => "smart",
            Blob::Mini => "mini",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Body {
    Empty,
    Json(String),
    /// The contents of a file (a catalog path: the host reads it).
    File(String),
}

/// Something for the host to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Task {
    /// An HTTP request. `save_to`: write the response body there (atomically, only on success).
    Http { id: u64, method: &'static str, url: String, token: String, body: Body, save_to: Option<String> },
    /// Build a photo's smart and mini previews from its original (decodes it: run it off the UI
    /// thread; natively [`build_proxies`]).
    Proxies { id: u64, original: String, smart: String, mini: String },
}

impl Task {
    pub fn id(&self) -> u64 {
        match self {
            Task::Http { id, .. } | Task::Proxies { id, .. } => *id,
        }
    }
}

/// A finished task: the HTTP status (0: no answer, `body` says why) and the response body.
#[derive(Clone, Debug, PartialEq)]
pub struct Done {
    pub id: u64,
    pub status: u16,
    pub body: String,
}

impl Done {
    pub fn failed(id: u64, why: impl Into<String>) -> Done {
        Done { id, status: 0, body: why.into() }
    }
    fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// The request in flight that orders the library (one at a time).
#[derive(Debug)]
enum Control {
    Login,
    Snapshot,
    Pull,
    Push(Pushed),
}

/// A photo file transfer step. Uploads go original → proxies → smart → mini, one step at a time.
#[derive(Clone, Debug, PartialEq)]
enum Job {
    /// Does the server have the original?
    Head { key: String, path: String, id: PhotoId },
    Put { key: String, blob: Blob, path: String, id: PhotoId },
    Proxies { key: String, path: String, id: PhotoId },
    Get { key: String, blob: Blob, dest: String },
}

impl Job {
    fn key(&self) -> &str {
        match self {
            Job::Head { key, .. } | Job::Put { key, .. } | Job::Proxies { key, .. } | Job::Get { key, .. } => key,
        }
    }
}

/// Sync state of an open library (signed in, or signed out after syncing).
pub struct SyncState {
    pub config: SyncConfig,
    pub outbox: Outbox,
    /// Content hashes the server has (all three files).
    uploaded: HashSet<String>,
    /// A sign-in waiting to be sent (the password is never saved).
    login: Option<proto::Login>,
    control: Option<(u64, Control)>,
    jobs: HashMap<u64, Job>,
    /// Upload steps ready to go (the next step of a finished one).
    ready: VecDeque<Job>,
    next_id: u64,
    /// When to pull next (`None`: now).
    pull_at: Option<Instant>,
    /// After a failure: not before then.
    retry_at: Option<Instant>,
    backoff: Duration,
    /// Pulled during a slider drag: applied once it ends.
    held: Vec<(u64, Op)>,
    needs_snapshot: bool,
    /// Blob transfers that failed: not retried before then.
    blob_retry: HashMap<String, Instant>,
    /// Originals asked for (by content hash).
    want_originals: HashSet<String>,
    error: Option<String>,
    /// Catalog revision the blob plan was made for.
    planned: Option<u64>,
    plan: VecDeque<Job>,
    written_config: String,
    written_outbox: String,
}

impl SyncState {
    fn new(config: SyncConfig) -> SyncState {
        SyncState {
            config,
            outbox: Outbox::default(),
            uploaded: HashSet::new(),
            login: None,
            control: None,
            jobs: HashMap::new(),
            ready: VecDeque::new(),
            next_id: 1,
            pull_at: None,
            retry_at: None,
            backoff: Duration::from_secs(1),
            held: Vec::new(),
            needs_snapshot: false,
            blob_retry: HashMap::new(),
            want_originals: HashSet::new(),
            error: None,
            planned: None,
            plan: VecDeque::new(),
            written_config: String::new(),
            written_outbox: String::new(),
        }
    }

    /// Read a library's sync files (`None`: the library never synced). Damaged files read as
    /// empty: the server's copy is the reference, so the next sync repairs this one.
    pub fn load(files: &mut dyn Store) -> Option<SyncState> {
        let config: SyncConfig = serde_json::from_slice(&files.read(CONFIG).ok()??).ok()?;
        let mut st = SyncState::new(config);
        st.written_config = serde_json::to_string_pretty(&st.config).unwrap_or_default();
        match files.read(OUTBOX).ok().flatten().map(|b| serde_json::from_slice::<Outbox>(&b)) {
            Some(Ok(o)) => st.outbox = o,
            Some(Err(e)) => {
                log::error!("sync: {OUTBOX} is damaged ({e}); reloading the library from the server");
                st.needs_snapshot = true;
            }
            None => {}
        }
        st.written_outbox = serde_json::to_string(&st.outbox).unwrap_or_default();
        if let Some(b) = files.read(UPLOADED).ok().flatten() {
            st.uploaded = String::from_utf8_lossy(&b).lines().filter(|l| !l.is_empty()).map(str::to_string).collect();
        }
        // a library that was synced before reloads the server's state once, in case another
        // device changed what this one's pending changes touch
        st.needs_snapshot |= !st.config.library.is_empty() && !st.config.token.is_empty() && !st.outbox.is_empty();
        Some(st)
    }

    pub fn signed_in(&self) -> bool {
        !self.config.token.is_empty() || self.login.is_some()
    }

    fn id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.config.server.trim_end_matches('/'))
    }

    fn http(&mut self, method: &'static str, path: &str, body: Body, save_to: Option<String>) -> Task {
        Task::Http { id: self.id(), method, url: self.url(path), token: self.config.token.clone(), body, save_to }
    }

    fn fail(&mut self, why: String) {
        log::warn!("sync: {why}");
        self.error = Some(why);
        self.retry_at = Some(Instant::now() + self.backoff);
        self.backoff = (self.backoff * 2).min(BACKOFF_MAX);
    }

    fn succeeded(&mut self) {
        self.error = None;
        self.retry_at = None;
        self.backoff = Duration::from_secs(1);
    }

    fn signed_out(&mut self, why: &str) {
        self.config.token.clear();
        self.login = None;
        self.error = Some(why.to_string());
    }

    /// Write the outbox (before the op log, so a change is never logged without being queued).
    pub(crate) fn save_outbox(&mut self, files: &mut dyn Store) -> std::io::Result<()> {
        let s = serde_json::to_string(&self.outbox).unwrap_or_default();
        if s != self.written_outbox {
            files.write_atomic(OUTBOX, s.as_bytes())?;
            self.written_outbox = s;
        }
        Ok(())
    }

    /// Write `sync.json` if it changed (after the op log: the cursor never runs ahead of it).
    pub(crate) fn save_config(&mut self, files: &mut dyn Store) -> std::io::Result<()> {
        let s = serde_json::to_string_pretty(&self.config).unwrap_or_default();
        if s != self.written_config {
            files.write_atomic(CONFIG, s.as_bytes())?;
            self.written_config = s;
        }
        Ok(())
    }

    /// What `sync.status` reports.
    pub fn status(&self) -> Value {
        let uploads = self.plan.iter().chain(self.ready.iter()).chain(self.jobs.values()).filter(|j| !matches!(j, Job::Get { .. })).count();
        let downloads = self.plan.iter().chain(self.jobs.values()).filter(|j| matches!(j, Job::Get { .. })).count();
        let state = if !self.signed_in() {
            "signedOut"
        } else if self.error.is_some() {
            "error"
        } else if self.control.is_some() || !self.jobs.is_empty() || !self.outbox.is_empty() || uploads + downloads > 0 {
            "syncing"
        } else {
            "idle"
        };
        json!({
            "state": state,
            "signedIn": self.signed_in(),
            "server": self.config.server,
            "user": self.config.user,
            "device": self.config.device,
            "cursor": self.config.cursor,
            "pending": self.outbox.len(),
            "uploads": uploads,
            "downloads": downloads,
            "error": self.error,
            "offlineAlbums": self.config.offline_albums,
            "offlinePhotos": self.config.offline_photos,
            "storeOriginals": self.config.store_originals,
        })
    }
}

/// The server's key for a photo's files: its content hash (without the suffix that keeps
/// converted copies apart), when it is a 128-bit hex hash.
pub fn blob_key(p: &Photo) -> Option<String> {
    let h = p.content_hash.as_deref()?.split(':').next()?;
    (h.len() == 32 && h.bytes().all(|b| b.is_ascii_hexdigit())).then(|| h.to_ascii_lowercase())
}

/// The mini preview's file name in the smart previews folder (beside the smart preview's).
pub fn mini_file_name(p: &Photo) -> String {
    crate::smart::file_name(p).replace(".lcsp", ".lcsm")
}

fn decode<T: serde::de::DeserializeOwned>(d: &Done) -> std::result::Result<T, String> {
    serde_json::from_str(&d.body).map_err(|e| format!("unexpected answer from the server ({e})"))
}

/// Why a request failed, for the user.
fn why(d: &Done) -> String {
    match d.status {
        0 => format!("can't reach the server: {}", d.body),
        401 => "the server refused this device's sign-in".into(),
        s => {
            let msg = serde_json::from_str::<Value>(&d.body).ok().and_then(|v| v["error"].as_str().map(str::to_string));
            format!("server error {s}: {}", msg.unwrap_or_else(|| d.body.chars().take(200).collect()))
        }
    }
}

fn is_removal(op: &Op) -> bool {
    matches!(op, Op::RemovePhoto { .. } | Op::RemoveAlbum { .. } | Op::RemoveStack { .. })
}

impl Session {
    /// The library's sync, when it was ever signed in.
    pub fn sync_state(&self) -> Option<&SyncState> {
        self.sync.as_ref()
    }

    /// Queue an op this device applied for the server (`inverse` as [`Catalog::apply`] returned).
    /// Recorded while signed out too: signing in again sends them.
    pub(crate) fn sync_record(&mut self, op: &Op, inverse: &Op) {
        if let Some(st) = self.sync.as_mut() {
            st.outbox.record(op, inverse, &self.catalog);
        }
    }

    /// Sign this library in to a server (sent by the next [`Session::sync_tasks`]). A library
    /// that has photos can only start syncing with an empty server library (it uploads them);
    /// to get a server's photos, sign in from a new library.
    pub fn sync_sign_in(&mut self, server: &str, user: &str, password: &str, device: &str) -> Result<()> {
        let server = server.trim().trim_end_matches('/');
        if !(server.starts_with("http://") || server.starts_with("https://")) {
            return Err(EngineError::Other(format!("not a server address: `{server}` (http://… or https://…)")));
        }
        if self.library.is_none() {
            return Err(EngineError::Other("sync needs a library that is saved (open or create one first)".into()));
        }
        let st = self.sync.get_or_insert_with(|| SyncState::new(SyncConfig::default()));
        if !st.config.library.is_empty() && (st.config.server != server || st.config.user != user) {
            return Err(EngineError::Other(format!(
                "this library is a copy of {}'s library on {}: sign in there, or use a new library for another server or user",
                st.config.user, st.config.server
            )));
        }
        st.config.server = server.to_string();
        st.config.user = user.to_string();
        st.login = Some(proto::Login { user: user.to_string(), password: password.to_string(), device: device.to_string() });
        st.error = None;
        st.retry_at = None;
        st.pull_at = None;
        Ok(())
    }

    /// Stop syncing (the library and its pending changes stay; signing in again resumes).
    pub fn sync_sign_out(&mut self) -> Result<()> {
        if let Some(st) = self.sync.as_mut() {
            st.config.token.clear();
            st.login = None;
            st.control = None;
            st.jobs.clear();
            st.ready.clear();
        }
        self.persist()
    }

    /// Pull now (instead of at the next poll).
    pub fn sync_soon(&mut self) {
        if let Some(st) = self.sync.as_mut() {
            st.pull_at = None;
            st.retry_at = None;
        }
    }

    /// Keep an original on this device (downloaded by the sync).
    pub fn sync_want_original(&mut self, p: &Photo) {
        if let (Some(st), Some(key)) = (self.sync.as_mut(), blob_key(p)) {
            st.blob_retry.remove(&key);
            st.want_originals.insert(key);
            st.planned = None;
        }
    }

    /// What the host should do next: at most one request that orders the library (sign-in,
    /// reload, pull or push) plus photo file transfers. Cheap when there's nothing to do; call
    /// it once per frame (or after [`Session::sync_done`]).
    pub fn sync_tasks(&mut self) -> Vec<Task> {
        let Some(mut st) = self.sync.take() else { return Vec::new() };
        if !st.held.is_empty() && self.interaction.is_none() {
            let held = std::mem::take(&mut st.held);
            self.apply_pulled(&mut st, held);
        }
        let now = Instant::now();
        let mut tasks = Vec::new();
        let waiting = st.retry_at.is_some_and(|t| now < t);
        if st.control.is_none() && !waiting {
            if let Some(login) = st.login.take() {
                let body = Body::Json(serde_json::to_string(&login).unwrap_or_default());
                let t = st.http("POST", "/api/login", body, None);
                st.control = Some((t.id(), Control::Login));
                tasks.push(t);
            } else if !st.config.token.is_empty() {
                if st.needs_snapshot {
                    let t = st.http("GET", "/api/snapshot", Body::Empty, None);
                    st.control = Some((t.id(), Control::Snapshot));
                    tasks.push(t);
                } else if self.interaction.is_some() {
                    // a slider drag holds a preview value: pull after it
                } else if st.pull_at.is_none_or(|t| now >= t) {
                    let path = format!("/api/ops?since={}&limit={PULL_LIMIT}", st.config.cursor);
                    let t = st.http("GET", &path, Body::Empty, None);
                    st.control = Some((t.id(), Control::Pull));
                    tasks.push(t);
                } else if !st.outbox.is_empty() {
                    let (ops, pushed) = st.outbox.take_push(PUSH_LIMIT);
                    let body = serde_json::to_string(&proto::Push { base: st.config.cursor, ops }).unwrap_or_default();
                    let t = st.http("POST", "/api/ops", Body::Json(body), None);
                    st.control = Some((t.id(), Control::Push(pushed)));
                    tasks.push(t);
                }
            }
        }
        if !st.config.token.is_empty() && !st.config.library.is_empty() && !waiting {
            self.blob_tasks(&mut st, now, &mut tasks);
        }
        self.sync = Some(st);
        tasks
    }

    /// Plan photo file transfers (when the catalog changed) and start some.
    fn blob_tasks(&mut self, st: &mut SyncState, now: Instant, tasks: &mut Vec<Task>) {
        if st.planned != Some(self.catalog.revision) && st.plan.is_empty() {
            st.plan = self.plan_blobs(st, now).into();
            st.planned = Some(self.catalog.revision);
        }
        while st.jobs.len() < BLOB_PARALLEL {
            let (mut job, planned) = match st.ready.pop_front() {
                Some(j) => (j, false),
                None => match st.plan.pop_front() {
                    Some(j) => (j, true),
                    None => break,
                },
            };
            // one transfer at a time per photo file (an upload is several steps)
            if planned && (st.jobs.values().any(|j| j.key() == job.key()) || st.ready.iter().any(|j| j.key() == job.key())) {
                continue;
            }
            if let Job::Proxies { key, id, .. } = &job {
                match self.proxy_paths(*id) {
                    // no folder for previews (the browser): the original is enough
                    None => {
                        let key = key.clone();
                        self.uploaded(st, key);
                        continue;
                    }
                    // built before (Build Smart Previews, or an earlier try): send them
                    Some((smart, mini)) if crate::smart::is_valid(std::path::Path::new(&smart)) && std::path::Path::new(&mini).exists() => {
                        job = Job::Put { key: key.clone(), blob: Blob::Smart, path: smart, id: *id };
                    }
                    Some(_) => {}
                }
            }
            let task = match &job {
                Job::Head { key, .. } => st.http("HEAD", &format!("/api/blobs/original/{key}"), Body::Empty, None),
                Job::Put { key, blob, path, .. } => st.http("PUT", &format!("/api/blobs/{}/{key}", blob.name()), Body::File(path.clone()), None),
                Job::Get { key, blob, dest } => st.http("GET", &format!("/api/blobs/{}/{key}", blob.name()), Body::Empty, Some(dest.clone())),
                Job::Proxies { path, id, .. } => {
                    let Some((smart, mini)) = self.proxy_paths(*id) else { continue };
                    Task::Proxies { id: st.id(), original: path.clone(), smart, mini }
                }
            };
            st.jobs.insert(task.id(), job);
            tasks.push(task);
        }
    }

    /// Where a photo's smart and mini previews live on this device (`None`: no folder for them).
    fn proxy_paths(&self, id: PhotoId) -> Option<(String, String)> {
        let dir = self.media.smart_dir.as_ref()?;
        let p = self.catalog.photo(id)?;
        let s = |n: String| dir.join(n).to_string_lossy().to_string();
        Some((s(crate::smart::file_name(p)), s(mini_file_name(p))))
    }

    /// Where downloaded originals are kept.
    fn originals_dir(&self) -> Option<PathBuf> {
        self.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.join("sync").join("originals"))
    }

    /// Photo file transfers this device needs: uploads of the originals it has that the server
    /// doesn't, then downloads — the smart previews of the active photo and of what's available
    /// offline, mini previews of every photo whose file isn't here, and wanted originals.
    // ponytail: one stat per photo per catalog change; keep an index of the proxies folder once
    // libraries get big enough for that to show.
    fn plan_blobs(&self, st: &SyncState, now: Instant) -> Vec<Job> {
        let exists = |p: &str| std::path::Path::new(p).exists();
        let retry_ok = |key: &str| st.blob_retry.get(key).is_none_or(|t| now >= *t);
        let mut offline: HashSet<PhotoId> = st.config.offline_photos.iter().copied().collect();
        for a in &st.config.offline_albums {
            offline.extend(self.catalog.album_photos(*a));
        }
        let active = self.active();
        let (mut ups, mut smart, mut minis, mut originals) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let mut seen = HashSet::new();
        for p in self.catalog.photos().filter(|p| !p.local && p.copy_of.is_none()) {
            let (Some(key), Source::File { path }) = (blob_key(p), &p.source) else { continue };
            if !retry_ok(&key) || !seen.insert(key.clone()) {
                continue;
            }
            if !path.starts_with(PATH_PREFIX) {
                // this device has the file
                if !st.uploaded.contains(&key) && cfg!(not(target_arch = "wasm32")) {
                    ups.push(Job::Head { key, path: path.clone(), id: p.id });
                }
                continue;
            }
            let Some((smart_path, mini_path)) = self.proxy_paths(p.id) else { continue };
            let pinned = offline.contains(&p.id);
            if (pinned || active == Some(p.id)) && !exists(&smart_path) {
                let job = Job::Get { key: key.clone(), blob: Blob::Smart, dest: smart_path.clone() };
                if active == Some(p.id) { smart.insert(0, job) } else { smart.push(job) }
            }
            if !exists(&mini_path) && !exists(&smart_path) {
                minis.push(Job::Get { key: key.clone(), blob: Blob::Mini, dest: mini_path });
            }
            if (st.config.store_originals || st.want_originals.contains(&key))
                && let Some(dir) = self.originals_dir()
            {
                let dest = dir.join(&key).join(file_name_of(path, &p.file_name));
                originals.push(Job::Get { key, blob: Blob::Original, dest: dest.to_string_lossy().to_string() });
            }
        }
        ups.into_iter().chain(smart).chain(minis).chain(originals).collect()
    }

    /// A finished task.
    pub fn sync_done(&mut self, done: Done) {
        let Some(mut st) = self.sync.take() else { return };
        if st.control.as_ref().is_some_and(|(id, _)| *id == done.id) {
            if let Some((_, c)) = st.control.take() {
                self.control_done(&mut st, c, &done);
            }
        } else if let Some(job) = st.jobs.remove(&done.id) {
            self.job_done(&mut st, job, &done);
        }
        self.sync = Some(st);
        if let Err(e) = self.persist() {
            log::error!("sync: {e}");
        }
    }

    fn control_done(&mut self, st: &mut SyncState, c: Control, d: &Done) {
        if d.status == 401 && !matches!(c, Control::Login) {
            st.signed_out("the server signed this device out: sign in again");
            return;
        }
        match c {
            Control::Login => match (d.ok(), decode::<proto::Device>(d)) {
                (true, Ok(dev)) => {
                    if !st.config.library.is_empty() && st.config.library != dev.library {
                        st.signed_out("this library is a copy of another library on that server: use a new library");
                        return;
                    }
                    st.config.token = dev.token;
                    st.config.device = dev.device;
                    st.config.space = dev.space;
                    self.catalog.set_id_space(dev.space);
                    st.needs_snapshot = true;
                    st.succeeded();
                }
                (true, Err(e)) => st.signed_out(&e),
                (false, _) if d.status == 401 => st.signed_out("wrong user name or password"),
                (false, _) => st.signed_out(&why(d)),
            },
            Control::Snapshot => match (d.ok(), decode::<proto::Snapshot>(d)) {
                (true, Ok(snap)) => {
                    match self.adopt_snapshot(st, snap) {
                        Ok(()) => st.succeeded(),
                        Err(e) => st.signed_out(&e),
                    }
                }
                (true, Err(e)) => st.fail(e),
                (false, _) => st.fail(why(d)),
            },
            Control::Pull => match d.status {
                410 => st.needs_snapshot = true,
                _ if d.ok() => match decode::<proto::Ops>(d) {
                    Ok(ops) => {
                        let more = ops.ops.len() >= PULL_LIMIT;
                        if self.interaction.is_some() {
                            st.held.extend(ops.ops);
                        } else {
                            self.apply_pulled(st, ops.ops);
                        }
                        st.pull_at = (!more).then(|| Instant::now() + POLL);
                        st.succeeded();
                    }
                    Err(e) => st.fail(e),
                },
                _ => st.fail(why(d)),
            },
            Control::Push(pushed) => match d.status {
                409 => st.pull_at = None,
                422 => {
                    let r = decode::<proto::Refused>(d);
                    log::warn!("sync: the server refused a change ({r:?}); reloading the library");
                    if let Ok(r) = r {
                        st.outbox.rejected(&pushed, r.index);
                    }
                    st.needs_snapshot = true;
                }
                _ if d.ok() => match decode::<proto::Head>(d) {
                    Ok(h) => {
                        st.outbox.acked(&pushed);
                        st.config.cursor = h.head;
                        st.succeeded();
                    }
                    Err(e) => st.fail(e),
                },
                _ => st.fail(why(d)),
            },
        }
    }

    /// Apply other devices' ops (merged with this device's pending changes).
    fn apply_pulled(&mut self, st: &mut SyncState, ops: Vec<(u64, Op)>) {
        let mut removed = false;
        let mut applied = false;
        for (seq, op) in ops {
            if seq <= st.config.cursor {
                continue;
            }
            if let Some(op) = st.outbox.merge_remote(&op) {
                removed |= is_removal(&op);
                match self.catalog.apply(op.clone()) {
                    Ok(_) => {
                        self.pending_log.push(op);
                        applied = true;
                    }
                    Err(e) => {
                        log::warn!("sync: a change from another device doesn't apply here ({e}); reloading the library");
                        st.needs_snapshot = true;
                    }
                }
            }
            st.config.cursor = seq;
        }
        if applied {
            // undo steps restore whole values: never let them silently revert another device's edit
            self.redo.clear();
            if removed {
                self.undo.clear();
            }
        }
    }

    /// The server's library (`snap`) becomes this one: at sign-in, or to repair a copy that
    /// went wrong. This device's own state (Local records, where its files are, History) and its
    /// pending changes are kept on top.
    fn adopt_snapshot(&mut self, st: &mut SyncState, snap: proto::Snapshot) -> std::result::Result<(), String> {
        st.needs_snapshot = false;
        if st.config.library != snap.library {
            if !st.config.library.is_empty() {
                return Err("this library is a copy of another library on that server: use a new library".into());
            }
            let server_empty = snap.seq == 0 && snap.catalog.is_empty() && snap.catalog.albums().next().is_none();
            if server_empty {
                // the first device: upload this library
                st.config.library = snap.library;
                st.config.cursor = 0;
                st.outbox.seed(&self.catalog);
                return Ok(());
            }
            let has_photos = self.catalog.photos().any(|p| !p.local) || self.catalog.albums().next().is_some();
            if has_photos {
                return Err(
                    "this library has photos and the server already has a library: sign in from a new library to get the server's photos".into()
                );
            }
            st.config.library = snap.library;
        }
        let mut c: Catalog = snap.catalog;
        carry_local(&self.catalog, &mut c);
        c.set_id_space(st.config.space);
        let dropped = st.outbox.rebase(&mut c);
        if dropped > 0 {
            log::info!("sync: {dropped} change(s) made here no longer apply to the server's library");
        }
        st.config.cursor = snap.seq;
        self.replace_catalog(c);
        Ok(())
    }

    /// Switch to another state of the library wholesale (a snapshot replaces the op log).
    fn replace_catalog(&mut self, mut c: Catalog) {
        let _ = self.persist();
        c.revision = self.catalog.revision + 1;
        self.catalog = c;
        self.undo.clear();
        self.redo.clear();
        self.interaction = None;
        self.selection.ids.retain(|id| self.catalog.photo(*id).is_some());
        self.selection.active = self.selection.active.filter(|id| self.catalog.photo(*id).is_some());
        let unlogged = self.pending_log.len() as u64;
        if let Some(lib) = self.library.as_mut() {
            match lib.journal.snapshot_with_unlogged(&self.catalog, unlogged) {
                Ok(()) => self.pending_log.clear(),
                Err(e) => log::error!("sync: saving the library: {e}"),
            }
        }
    }

    fn job_done(&mut self, st: &mut SyncState, job: Job, d: &Done) {
        let retry = |st: &mut SyncState, key: &str, why: String| {
            log::warn!("sync: {key}: {why}");
            st.blob_retry.insert(key.to_string(), Instant::now() + BLOB_RETRY);
        };
        if d.status == 401 {
            st.signed_out("the server signed this device out: sign in again");
            return;
        }
        match job {
            Job::Head { key, path, id } => match d.status {
                200 => st.ready.push_front(Job::Proxies { key, path, id }),
                404 => st.ready.push_front(Job::Put { key, blob: Blob::Original, path, id }),
                _ => retry(st, &key, why(d)),
            },
            Job::Put { key, blob, path, id } if d.ok() => match blob {
                Blob::Original => st.ready.push_front(Job::Proxies { key, path, id }),
                Blob::Smart => match self.proxy_paths(id) {
                    Some((_, mini)) => st.ready.push_front(Job::Put { key, blob: Blob::Mini, path: mini, id }),
                    None => self.uploaded(st, key),
                },
                Blob::Mini => self.uploaded(st, key),
            },
            Job::Put { key, .. } => retry(st, &key, why(d)),
            Job::Proxies { key, id, .. } => match (d.ok(), self.proxy_paths(id)) {
                (true, Some((smart, _))) => st.ready.push_front(Job::Put { key, blob: Blob::Smart, path: smart, id }),
                // no proxies folder (the browser): the original is enough
                (_, None) => self.uploaded(st, key),
                (false, _) => {
                    // an original that doesn't decode here: the server keeps it, others try
                    log::warn!("sync: can't build previews of {key}: {}", d.body);
                    self.uploaded(st, key);
                }
            },
            Job::Get { key, blob, dest } => {
                if !d.ok() {
                    retry(st, &key, why(d));
                    return;
                }
                let ids: Vec<PhotoId> = self.catalog.photos().filter(|p| blob_key(p).as_deref() == Some(&key)).map(|p| p.id).collect();
                self.media.availability.forget(&dest);
                for id in &ids {
                    self.media.forget(*id);
                }
                if blob == Blob::Original {
                    // the file is here now: the photo points at it (on this device only)
                    st.uploaded.insert(key.clone());
                    for id in ids {
                        let Some(p) = self.catalog.photo(id) else { continue };
                        if let Source::File { path } = &p.source {
                            self.media.availability.forget(path);
                        }
                        let op = Op::Relink { id, file_name: p.file_name.clone(), source: Source::File { path: dest.clone() }, format: None };
                        if self.catalog.apply(op.clone()).is_ok() {
                            self.pending_log.push(op);
                        }
                    }
                }
                st.planned = None;
            }
        }
    }

    /// The server has all of a photo's files.
    fn uploaded(&mut self, st: &mut SyncState, key: String) {
        if let Some(lib) = self.library.as_mut()
            && let Err(e) = lib.files_mut().append(UPLOADED, format!("{key}\n").as_bytes())
        {
            log::error!("sync: {UPLOADED}: {e}");
        }
        st.uploaded.insert(key);
    }

    /// Make an album or photos available offline on this device (or not).
    pub fn sync_offline(&mut self, albums: &[AlbumId], photos: &[PhotoId], on: bool) -> Result<()> {
        let st = self.sync.as_mut().ok_or_else(|| EngineError::Other("this library isn't synced".into()))?;
        let toggle = |list: &mut Vec<_>, items: &[_]| {
            for i in items {
                list.retain(|x| x != i);
                if on {
                    list.push(*i);
                }
            }
        };
        toggle(&mut st.config.offline_albums, albums);
        toggle(&mut st.config.offline_photos, photos);
        st.planned = None;
        self.persist()
    }

    /// Run the sync to completion here (blocking: the CLI, tests): sign-in, reload, pull, push
    /// and photo file transfers, until there's nothing left to do or `limit` tasks ran.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn sync_now(&mut self, limit: usize) -> Value {
        self.sync_soon();
        let mut n = 0;
        while n < limit {
            let tasks = self.sync_tasks();
            if tasks.is_empty() {
                break;
            }
            for t in tasks {
                n += 1;
                let d = run(&t);
                self.sync_done(d);
            }
        }
        self.sync.as_ref().map(SyncState::status).unwrap_or(Value::Null)
    }
}

/// The file name an original is kept under (its name in the catalog path, else the photo's).
fn file_name_of(path: &str, fallback: &str) -> String {
    let n = path.rsplit(['/', '\\']).next().filter(|n| !n.is_empty() && *n != "." && *n != "..").unwrap_or(fallback);
    n.replace(['/', '\\'], "_")
}

/// Path of a photo file kept by content (`web/<hash>/<name>`), for hosts.
pub fn content_path(p: &Photo) -> Option<String> {
    Some(original_path(&blob_key(p)?, &p.file_name))
}

/// Build a photo's smart and mini previews from its original (decodes it).
#[cfg(not(target_arch = "wasm32"))]
pub fn build_proxies(original: &str, smart: &str, mini: &str) -> std::result::Result<(), String> {
    use lightcraft_raster::resample::{Filter, fit};
    let bytes = std::fs::read(original).map_err(|e| format!("{original}: {e}"))?;
    let (img, info) = crate::files::load_vec(bytes, crate::media::SourceLevel::Preview.max_edge())?;
    let tone = info.camera_tone.as_ref();
    let write = |path: &str, b: Vec<u8>| lightcraft_catalog::safe_file::write_atomic(std::path::Path::new(path), &b).map_err(|e| format!("{path}: {e}"));
    if let Some(dir) = std::path::Path::new(smart).parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    write(smart, crate::smart::encode(&img, tone)?)?;
    let small = if img.width.max(img.height) > MINI_EDGE { fit(&img, MINI_EDGE, MINI_EDGE, Filter::Mitchell) } else { img };
    write(mini, crate::smart::encode(&small, tone)?)
}

/// Run a task here (blocking; native hosts call it on a worker thread).
#[cfg(not(target_arch = "wasm32"))]
pub fn run(task: &Task) -> Done {
    match task {
        Task::Proxies { id, original, smart, mini } => match build_proxies(original, smart, mini) {
            Ok(()) => Done { id: *id, status: 200, body: String::new() },
            Err(e) => Done::failed(*id, e),
        },
        Task::Http { id, method, url, token, body, save_to } => match net::http(method, url, token, body, save_to.as_deref()) {
            Ok((status, body)) => Done { id: *id, status, body },
            Err(e) => Done::failed(*id, e),
        },
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod net {
    use std::io::Read;
    use std::sync::{Arc, OnceLock};
    use std::time::Duration;

    use super::Body;

    /// Responses read into memory are capped (control answers, not photo files).
    const TEXT_MAX: u64 = 256 << 20;

    fn agent() -> &'static ureq::Agent {
        static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
        AGENT.get_or_init(|| {
            // pure-Rust TLS (no C crypto), with the Mozilla root certificates
            let tls = ureq::tls::TlsConfig::builder()
                .provider(ureq::tls::TlsProvider::Rustls)
                .unversioned_rustls_crypto_provider(Arc::new(rustls_rustcrypto::provider()))
                .build();
            ureq::Agent::config_builder()
                .http_status_as_error(false)
                .timeout_connect(Some(Duration::from_secs(15)))
                .timeout_recv_response(Some(Duration::from_secs(120)))
                .tls_config(tls)
                .build()
                .into()
        })
    }

    pub(super) fn http(method: &str, url: &str, token: &str, body: &Body, save_to: Option<&str>) -> Result<(u16, String), String> {
        let auth = format!("Bearer {token}");
        let r = match (method, body) {
            ("GET", _) => agent().get(url).header("Authorization", &auth).call(),
            ("HEAD", _) => agent().head(url).header("Authorization", &auth).call(),
            ("DELETE", _) => agent().delete(url).header("Authorization", &auth).call(),
            (m, b) => {
                let req = match m {
                    "PUT" => agent().put(url),
                    _ => agent().post(url),
                };
                let req = req.header("Authorization", &auth);
                match b {
                    Body::Empty => req.send_empty(),
                    Body::Json(j) => req.header("Content-Type", "application/json").send(j.as_bytes()),
                    Body::File(path) => {
                        let f = std::fs::File::open(path).map_err(|e| format!("{path}: {e}"))?;
                        let len = f.metadata().map(|m| m.len()).map_err(|e| format!("{path}: {e}"))?;
                        req.header("Content-Type", "application/octet-stream").header("Content-Length", len.to_string()).send(f)
                    }
                }
            }
        };
        let mut resp = r.map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        if let (Some(dest), true) = (save_to, (200..300).contains(&status)) {
            let path = std::path::Path::new(dest);
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            }
            let mut reader = resp.body_mut().as_reader();
            lightcraft_catalog::safe_file::write_atomic_with(path, &mut |w| std::io::copy(&mut reader, w).map(|_| ()))
                .map_err(|e| format!("{dest}: {e}"))?;
            return Ok((status, String::new()));
        }
        let mut text = String::new();
        resp.body_mut().as_reader().take(TEXT_MAX).read_to_string(&mut text).map_err(|e| e.to_string())?;
        Ok((status, text))
    }
}
