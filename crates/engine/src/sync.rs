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
//!   asked to keep them. A downloaded original becomes the photo's file on this device. A photo
//!   whose file isn't here renders from the best of those it has ([`crate::media::SourceRef::Synced`]).
//!
//! - **Presets** are one document on the server (`/api/presets`, versioned). A device sends its
//!   user presets when they change, and merges the server's by preset id against the copy both
//!   last agreed on ([`merge_presets`]), so a preset deleted on one device stays deleted.
//!
//! Files in the library: `sync.json` ([`SyncConfig`]), `sync.outbox` (changes the server hasn't
//! acknowledged; written before the op log), `sync.uploaded` (content hashes the server has) and
//! `sync.presets` (the user presets as last synced).

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
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
pub const PRESETS: &str = "sync.presets";
/// User presets taken from the server at most (untrusted input).
const PRESETS_MAX: usize = 10_000;

/// Signing in to a server whose library this one isn't a copy of (the server was set up again,
/// or the user's library there was replaced).
const NOT_THIS_LIBRARY: &str = "this library is a copy of another library on that server (was the server set up again?): \
     sign in from a new library (Settings › General › Open Library…; in a browser, open the server's address with ?reset)";

/// How often the server is asked for other devices' changes.
pub const POLL: Duration = Duration::from_secs(5);
/// Longest wait between retries after a failure (network down, server away).
const BACKOFF_MAX: Duration = Duration::from_secs(60);
/// A missing or failed photo file is tried again after this long.
const BLOB_RETRY: Duration = Duration::from_secs(60);
/// Ops per push / pull.
const PUSH_LIMIT: usize = 500;
const PULL_LIMIT: usize = 2000;
/// Photo file transfers at a time.
const BLOB_PARALLEL: usize = 4;
/// The server's storage numbers are asked for at most this often (it walks every photo file).
const USAGE_EVERY: Duration = Duration::from_secs(20);
/// Long edge of the mini preview.
pub const MINI_EDGE: usize = 512;
/// The server's id spaces stay below this, so ids remain exact in JSON numbers read by
/// JavaScript (`space << 32 | n` < 2^53).
pub const MAX_SPACE: u32 = (1 << 21) - 1;

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
    /// Don't talk to the server for now (File → Pause Syncing).
    pub paused: bool,
    /// Let the server build the previews of the originals this device uploads, instead of building them here
    /// (a phone saves its battery and memory). `None`: the platform's default, which is on for iOS.
    pub server_previews: Option<bool>,
    /// Version of the server's presets document this device last synced with.
    pub presets_version: u64,
}

impl SyncConfig {
    /// Whether the server builds the previews of what this device uploads ([`SyncConfig::server_previews`]).
    pub fn server_builds_previews(&self) -> bool {
        self.server_previews.unwrap_or(cfg!(target_os = "ios"))
    }
}

/// Where a host without a previews folder (the browser) keeps synced photo files: storage keys
/// `<prefix><hash>.lcsp` / `.lcsm` for the smart and mini previews, `originals/<hash>` for
/// originals (the web build's own layout). The host fills in what its storage holds and keeps
/// `originals` up to date as photos are added there.
#[derive(Clone, Debug, Default)]
pub struct BrowserStore {
    pub prefix: String,
    /// Preview file names stored (`<hash>.lcsp`, `<hash>.lcsm`).
    pub proxies: HashSet<String>,
    /// Content hashes of the originals stored.
    pub originals: HashSet<String>,
}

impl BrowserStore {
    pub fn original_key(hash: &str) -> String {
        format!("originals/{hash}")
    }
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
    /// An HTTP request with `Authorization: Bearer <token>`. `save_to`: write the response body
    /// there (atomically, only on success) instead of returning it.
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
    pub(crate) fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// Task ids are unique in the process: an answer for a library that was closed meanwhile is
/// never mistaken for one of the next library's.
static NEXT_TASK: AtomicU64 = AtomicU64::new(1);

/// The request in flight that orders the library (one at a time).
#[derive(Debug)]
enum Control {
    /// (kept to try again when the server can't be reached)
    Login(proto::Login),
    Snapshot,
    Pull,
    Push(Pushed),
    GetPresets,
    /// (what was sent)
    PutPresets(Vec<Value>),
}

/// A photo file transfer step. Uploads go original → proxies → smart → mini, one step at a time.
#[derive(Clone, Debug, PartialEq)]
enum Job {
    /// Does the server have the original?
    Head {
        key: String,
        path: String,
        id: PhotoId,
    },
    Put {
        key: String,
        blob: Blob,
        path: String,
        id: PhotoId,
    },
    Proxies {
        key: String,
        path: String,
        id: PhotoId,
    },
    Get {
        key: String,
        blob: Blob,
        dest: String,
    },
}

impl Job {
    /// What a job moves: one transfer at a time per file.
    fn target(&self) -> String {
        match self {
            Job::Head { key, .. } | Job::Put { key, .. } | Job::Proxies { key, .. } => format!("up:{key}"),
            Job::Get { dest, .. } => dest.clone(),
        }
    }
}

/// Sync state of an open library (signed in, or signed out after syncing).
pub struct SyncState {
    pub config: SyncConfig,
    pub outbox: Outbox,
    /// The outbox changed since it was last written.
    outbox_dirty: bool,
    /// Content hashes the server has (all three files).
    uploaded: HashSet<String>,
    /// A sign-in waiting to be sent (the password is never saved).
    login: Option<proto::Login>,
    control: Option<(u64, Control)>,
    jobs: HashMap<u64, Job>,
    /// Requests beside the library's own sync (search by description through the server): what
    /// each one is for, and the ones waiting to be handed to the host.
    pub(crate) aux: HashMap<u64, crate::vision::Aux>,
    pub(crate) aux_ready: VecDeque<Task>,
    /// Upload steps ready to go (the next step of a finished one).
    ready: VecDeque<Job>,
    /// When to pull next (`None`: now).
    pull_at: Option<Instant>,
    /// After a failure: not before then.
    retry_at: Option<Instant>,
    backoff: Duration,
    /// Pulled during a slider drag: applied once it ends.
    held: Vec<(u64, Op)>,
    held_snapshot: Option<proto::Snapshot>,
    needs_snapshot: bool,
    /// Photo files whose transfer failed: not tried again before then.
    blob_retry: HashMap<String, Instant>,
    /// Originals the server refused (not the file their hash says): not sent again this session.
    refused: HashSet<String>,
    /// Originals asked for (by content hash).
    want_originals: HashSet<String>,
    error: Option<String>,
    /// What the transfer plan was made for (catalog revision, active photo, settings changes).
    planned: Option<(u64, Option<PhotoId>, u64)>,
    plan: VecDeque<Job>,
    /// Bumped when what to keep offline changes.
    plan_gen: u64,
    /// File names in the proxies folder (read once, then kept up to date).
    have: Option<HashSet<String>>,
    /// A photo file arrived: renders that failed for want of it are tried again.
    landed: bool,
    written_config: String,
    /// The user presets as last synced (the base of [`merge_presets`]).
    presets_base: Vec<Value>,
    presets_base_dirty: bool,
    /// The presets document's version on the server, as the last pull said.
    presets_remote: u64,
    /// The presets differ from the last synced copy: send them.
    presets_changed: bool,
    /// Which presets.json write was last compared.
    presets_seen: Option<u64>,
    /// What the server says this library takes there, when it said so, and the request in flight.
    usage: Option<proto::Usage>,
    usage_at: Option<Instant>,
    usage_task: Option<u64>,
    /// Why the last answer wasn't one (the numbers above are older than that).
    usage_error: Option<String>,
    /// Someone is looking (Settings ▸ Sync, `sync.usage`): ask again once the last answer is old.
    usage_wanted: bool,
}

impl SyncState {
    fn new(config: SyncConfig) -> SyncState {
        SyncState {
            config,
            outbox: Outbox::default(),
            outbox_dirty: false,
            uploaded: HashSet::new(),
            login: None,
            control: None,
            jobs: HashMap::new(),
            aux: HashMap::new(),
            aux_ready: VecDeque::new(),
            ready: VecDeque::new(),
            pull_at: None,
            retry_at: None,
            backoff: Duration::from_secs(1),
            held: Vec::new(),
            held_snapshot: None,
            needs_snapshot: false,
            blob_retry: HashMap::new(),
            refused: HashSet::new(),
            want_originals: HashSet::new(),
            error: None,
            planned: None,
            plan: VecDeque::new(),
            plan_gen: 0,
            have: None,
            landed: false,
            written_config: String::new(),
            presets_base: Vec::new(),
            presets_base_dirty: false,
            presets_remote: 0,
            presets_changed: false,
            presets_seen: None,
            usage: None,
            usage_at: None,
            usage_task: None,
            usage_error: None,
            usage_wanted: false,
        }
    }

    /// Read a library's sync files (`None`: the library never synced). A damaged outbox reloads
    /// the library from the server (the server's copy is the reference); a damaged `sync.json`
    /// reads as never synced.
    pub fn load(files: &mut dyn Store) -> Option<SyncState> {
        let bytes = match files.read(CONFIG) {
            Ok(b) => b?,
            Err(e) => {
                log::error!("sync: {CONFIG}: {e}");
                return None;
            }
        };
        let config: SyncConfig = match serde_json::from_slice(&bytes) {
            Ok(c) => c,
            Err(e) => {
                log::error!("sync: {CONFIG} is damaged ({e}); sign in again");
                return None;
            }
        };
        let mut st = SyncState::new(config);
        st.config.space = st.config.space.min(MAX_SPACE);
        st.written_config = serde_json::to_string_pretty(&st.config).unwrap_or_default();
        match files.read(OUTBOX).ok().flatten().map(|b| serde_json::from_slice::<Outbox>(&b)) {
            Some(Ok(o)) => st.outbox = o,
            Some(Err(e)) => {
                log::error!("sync: {OUTBOX} is damaged ({e}); reloading the library from the server");
                st.needs_snapshot = true;
            }
            None => {}
        }
        if let Some(b) = files.read(PRESETS).ok().flatten() {
            st.presets_base = serde_json::from_slice(&b).unwrap_or_default();
        }
        st.presets_remote = st.config.presets_version;
        if let Some(b) = files.read(UPLOADED).ok().flatten() {
            st.uploaded = String::from_utf8_lossy(&b).lines().filter(|l| !l.is_empty()).map(str::to_string).collect();
        }
        // pending changes made before the library closed: reload the server's state once and
        // replay them on top (merged), in case other devices changed what they touch
        st.needs_snapshot |= !st.config.library.is_empty() && !st.outbox.is_empty();
        Some(st)
    }

    /// Signed in, or signing in (the sign-in waiting to be sent or on its way).
    pub fn signed_in(&self) -> bool {
        !self.config.token.is_empty() || self.login.is_some() || matches!(self.control, Some((_, Control::Login(_))))
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.config.server.trim_end_matches('/'))
    }

    pub(crate) fn http(&mut self, method: &'static str, path: &str, body: Body, save_to: Option<String>) -> Task {
        let id = NEXT_TASK.fetch_add(1, Ordering::Relaxed);
        Task::Http { id, method, url: self.url(path), token: self.config.token.clone(), body, save_to }
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
        log::warn!("sync: {why}");
        self.config.token.clear();
        self.login = None;
        self.control = None;
        self.jobs.clear();
        self.ready.clear();
        self.plan.clear();
        self.planned = None;
        self.usage_task = None;
        self.error = Some(why.to_string());
    }

    /// What the server says this library takes there (as of [`SyncState::usage_age`] ago).
    pub fn usage(&self) -> Option<&proto::Usage> {
        self.usage.as_ref()
    }

    /// Why the server's last answer about storage wasn't one.
    pub fn usage_error(&self) -> Option<&str> {
        self.usage_error.as_deref()
    }

    /// How long ago the server was asked about storage (answered or not).
    pub fn usage_age(&self) -> Option<Duration> {
        self.usage_at.map(|t| t.elapsed())
    }

    /// The answer to `GET /api/usage`.
    fn usage_done(&mut self, d: &Done) {
        // (a failure waits as long as a success before the next try)
        self.usage_at = Some(Instant::now());
        match d.status {
            401 => self.signed_out("the server signed this device out: sign in again"),
            404 | 405 => self.usage_error = Some("this server is too old to report storage: update it".into()),
            _ if d.ok() => match decode::<proto::Usage>(d) {
                Ok(u) => {
                    self.usage = Some(u);
                    self.usage_error = None;
                }
                Err(e) => self.usage_error = Some(e),
            },
            _ => self.usage_error = Some(why(d)),
        }
    }

    fn outbox_changed(&mut self) {
        self.outbox_dirty = true;
    }

    /// Write the outbox if it changed (before the op log, so a change is never logged without
    /// being queued).
    pub(crate) fn save_outbox(&mut self, files: &mut dyn Store) -> std::io::Result<()> {
        if self.outbox_dirty {
            let s = serde_json::to_vec(&self.outbox).map_err(std::io::Error::other)?;
            files.write_atomic(OUTBOX, &s)?;
            self.outbox_dirty = false;
        }
        Ok(())
    }

    /// Write `sync.presets` if it changed.
    pub(crate) fn save_presets_base(&mut self, files: &mut dyn Store) -> std::io::Result<()> {
        if self.presets_base_dirty {
            let s = serde_json::to_vec(&self.presets_base).map_err(std::io::Error::other)?;
            files.write_atomic(PRESETS, &s)?;
            self.presets_base_dirty = false;
        }
        Ok(())
    }

    /// Write `sync.json` if it changed (after the op log: the cursor never runs ahead of it).
    pub(crate) fn save_config(&mut self, files: &mut dyn Store) -> std::io::Result<()> {
        let s = serde_json::to_string_pretty(&self.config).map_err(std::io::Error::other)?;
        if s != self.written_config {
            files.write_atomic(CONFIG, s.as_bytes())?;
            self.written_config = s;
        }
        Ok(())
    }

    /// Photo file transfers queued or running: (uploads, downloads).
    fn transfers(&self) -> (usize, usize) {
        let all = || self.plan.iter().chain(self.ready.iter()).chain(self.jobs.values());
        let downloads = all().filter(|j| matches!(j, Job::Get { .. })).count();
        (all().count() - downloads, downloads)
    }

    /// What `sync.status` reports.
    pub fn status(&self) -> Value {
        let (uploads, downloads) = self.transfers();
        let state = if !self.signed_in() {
            "signedOut"
        } else if self.config.paused {
            "paused"
        } else if self.error.is_some() {
            "error"
        } else if self.control.is_some() || !self.outbox.is_empty() || uploads + downloads > 0 || self.login.is_some() {
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
            "library": self.config.library,
            "cursor": self.config.cursor,
            "pending": self.outbox.len(),
            "uploads": uploads,
            "downloads": downloads,
            "error": self.error,
            "offlineAlbums": self.config.offline_albums,
            "offlinePhotos": self.config.offline_photos,
            "storeOriginals": self.config.store_originals,
            "serverPreviews": self.config.server_builds_previews(),
            "paused": self.config.paused,
        })
    }

    /// The state for the topbar's cloud icon: `signedOut`, `paused`, `error`, `syncing` or `idle`.
    pub fn state(&self) -> &'static str {
        match self.status()["state"].as_str() {
            Some("paused") => "paused",
            Some("error") => "error",
            Some("syncing") => "syncing",
            Some("idle") => "idle",
            _ => "signedOut",
        }
    }

    /// The last error, if the last request failed.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
}

/// The server's key for a photo's files: its content hash (without the suffix that keeps
/// converted copies apart), when it is a 128-bit hex hash.
pub fn blob_key(p: &Photo) -> Option<String> {
    let h = p.content_hash.as_deref()?.split(':').next()?;
    (h.len() == 32 && h.bytes().all(|b| b.is_ascii_hexdigit())).then(|| h.to_ascii_lowercase())
}

/// The mini preview's file name in the proxies folder (beside the smart preview's).
pub fn mini_file_name(p: &Photo) -> String {
    let mut n = crate::smart::file_name(p);
    n.truncate(n.len().saturating_sub(".lcsp".len()));
    n + ".lcsm"
}

/// Preview file names in a [`BrowserStore`].
pub fn store_proxy_names(key: &str) -> (String, String) {
    (format!("{key}.lcsp"), format!("{key}.lcsm"))
}

/// Where to have a photo rendered on the sync server ([`Session::sync_render_target`]).
#[derive(Clone, Debug, PartialEq)]
pub struct RenderTarget {
    pub url: String,
    pub token: String,
    /// The photo original's content hash, the server's key for it.
    pub hash: String,
}

/// A photo whose file isn't on this device: the library has it by content (`web/<hash>/…`).
pub fn is_remote(p: &Photo) -> bool {
    matches!(&p.source, Source::File { path } if path.starts_with(PATH_PREFIX))
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

/// A server address as typed: `photos.example.com` means `https://photos.example.com`, while an
/// address on this computer or the home network without a scheme — `localhost`, a private or
/// Tailscale IP address, a one-word machine name (`nas`), `*.local` / `*.lan` / `*.home.arpa` /
/// `*.internal` — means plain `http://` (such servers rarely have a certificate; their traffic
/// stays at home or in the tailnet's tunnel). The scheme and host are lower-cased (phone keyboards
/// capitalise the first letter) and trailing `/`s go. `None`: not an `http(s)://` address.
pub fn server_address(typed: &str) -> Option<String> {
    let t = typed.trim();
    if t.contains(char::is_whitespace) {
        return None;
    }
    let (scheme, rest) = match t.split_once("://") {
        Some((scheme, rest)) => (Some(scheme.to_ascii_lowercase()), rest),
        None => (None, t),
    };
    let rest = rest.trim_end_matches('/');
    let (host, path) = match rest.split_once('/') {
        Some((host, path)) => (host, format!("/{path}")),
        None => (rest, String::new()),
    };
    let scheme = scheme.unwrap_or_else(|| if is_home_host(host) { "http".into() } else { "https".into() });
    if !matches!(scheme.as_str(), "http" | "https") || !is_host_port(host) || path.contains(['?', '#']) {
        return None;
    }
    Some(format!("{scheme}://{}{path}", host.to_ascii_lowercase()))
}

/// Is `host[:port]` this computer or on a home / private network (see [`server_address`])?
fn is_home_host(host_port: &str) -> bool {
    let host = match host_port.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(""),
        None => host_port.rsplit_once(':').map_or(host_port, |(h, _)| h),
    }
    .to_ascii_lowercase();
    match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(ip)) => {
            let [a, b, ..] = ip.octets();
            ip.is_loopback() || ip.is_private() || ip.is_link_local() || (a == 100 && (64..128).contains(&b))
        }
        Ok(std::net::IpAddr::V6(ip)) => ip.is_loopback() || ip.segments().first().is_some_and(|s| s & 0xfe00 == 0xfc00 || s & 0xffc0 == 0xfe80),
        Err(_) => {
            host == "localhost"
                || !host.contains('.')
                || [".local", ".lan", ".home.arpa", ".internal", ".localhost"].iter().any(|s| host.ends_with(s))
        }
    }
}

/// `name[:port]` or `[ipv6][:port]`.
fn is_host_port(a: &str) -> bool {
    let (name, port) = match a.strip_prefix('[') {
        Some(v6) => match v6.split_once(']') {
            Some((ip, "")) => (ip, None),
            Some((ip, rest)) => match rest.strip_prefix(':') {
                Some(port) => (ip, Some(port)),
                None => return false,
            },
            None => return false,
        },
        None => match a.split_once(':') {
            Some((name, port)) => (name, Some(port)),
            None => (a, None),
        },
    };
    let v6 = a.starts_with('[');
    let name_ok = !name.is_empty()
        && name
            .chars()
            .all(|c| if v6 { c.is_ascii_hexdigit() || matches!(c, ':' | '.') } else { c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') });
    name_ok && port.is_none_or(|p| (1..=5).contains(&p.len()) && p.chars().all(|c| c.is_ascii_digit()))
}

impl Session {
    /// The library's sync, when it was ever signed in.
    pub fn sync_state(&self) -> Option<&SyncState> {
        self.sync.as_ref()
    }

    /// Where `p` can be rendered on the sync server: this device is signed in and the photo has a content hash (the
    /// server only has it once its original was uploaded; the server says when it hasn't).
    pub fn sync_render_target(&self, p: &Photo) -> Option<RenderTarget> {
        let st = self.sync.as_ref().filter(|st| !st.config.token.is_empty())?;
        Some(RenderTarget { url: st.url("/api/render"), token: st.config.token.clone(), hash: blob_key(p)? })
    }

    /// Queue an op this device applied for the server (`inverse` as [`Catalog::apply`] returned).
    /// Recorded while signed out too: signing in again sends them.
    pub(crate) fn sync_record(&mut self, op: &Op, inverse: &Op) {
        if let Some(st) = self.sync.as_mut().filter(|st| !st.config.library.is_empty()) {
            st.outbox.record(op, inverse, &self.catalog);
            st.outbox_changed();
        }
    }

    /// Sign this library in to a server (sent by the next [`Session::sync_tasks`]). A library
    /// that has photos can only start syncing with an empty server library (it uploads them);
    /// to get a server's photos, sign in from a new library.
    pub fn sync_sign_in(&mut self, server: &str, user: &str, password: &str, device: &str) -> Result<()> {
        let typed = server;
        let Some(server) = server_address(typed) else {
            return Err(EngineError::Other(format!("not a server address: `{}` (https://… or http://…)", typed.trim())));
        };
        let server = server.as_str();
        if user.trim().is_empty() {
            return Err(EngineError::Other("a user name is needed".into()));
        }
        if self.library.is_none() {
            return Err(EngineError::Other("sync needs a library that is saved (open or create one first)".into()));
        }
        let st = self.sync.get_or_insert_with(|| SyncState::new(SyncConfig::default()));
        if !st.config.library.is_empty() && (!st.config.server.eq_ignore_ascii_case(server) || st.config.user != user) {
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
        self.persist()
    }

    /// Stop syncing (the library and its pending changes stay; signing in again resumes).
    pub fn sync_sign_out(&mut self) -> Result<()> {
        if let Some(st) = self.sync.as_mut() {
            st.signed_out("signed out");
            st.error = None;
        }
        self.persist()
    }

    /// Pull now (instead of at the next poll).
    pub fn sync_soon(&mut self) {
        if let Some(st) = self.sync.as_mut() {
            st.pull_at = None;
            st.retry_at = None;
            st.blob_retry.clear();
        }
    }

    /// Download these photos' originals to this device (kept until made unavailable offline).
    pub fn sync_want_originals(&mut self, ids: &[PhotoId]) -> usize {
        let Some(st) = self.sync.as_mut() else { return 0 };
        let mut n = 0;
        for p in ids.iter().filter_map(|id| self.catalog.photo(*id)).filter(|p| is_remote(p)) {
            if let Some(key) = blob_key(p) {
                st.blob_retry.remove(&key);
                n += usize::from(st.want_originals.insert(key));
            }
        }
        st.plan_gen += 1;
        n
    }

    /// Make an album or photos available offline on this device (or not).
    pub fn sync_offline(&mut self, albums: &[AlbumId], photos: &[PhotoId], on: bool) -> Result<()> {
        let st = self.sync.as_mut().ok_or_else(|| EngineError::Other("this library isn't synced".into()))?;
        fn toggle<T: PartialEq + Copy>(list: &mut Vec<T>, items: &[T], on: bool) {
            for i in items {
                list.retain(|x| x != i);
                if on {
                    list.push(*i);
                }
            }
        }
        toggle(&mut st.config.offline_albums, albums, on);
        toggle(&mut st.config.offline_photos, photos, on);
        st.plan_gen += 1;
        self.persist()
    }

    /// Stop talking to the server for now (or resume).
    pub fn sync_pause(&mut self, on: bool) -> Result<()> {
        let st = self.sync.as_mut().ok_or_else(|| EngineError::Other("this library isn't synced".into()))?;
        st.config.paused = on;
        st.pull_at = None;
        st.retry_at = None;
        self.persist()
    }

    /// Keep the original of every photo on this device (or only the ones asked for).
    pub fn sync_store_originals(&mut self, on: bool) -> Result<()> {
        let st = self.sync.as_mut().ok_or_else(|| EngineError::Other("this library isn't synced".into()))?;
        st.config.store_originals = on;
        st.plan_gen += 1;
        self.persist()
    }

    /// Have the server build the previews of what this device uploads (or build them here again).
    pub fn sync_server_previews(&mut self, on: bool) -> Result<()> {
        let st = self.sync.as_mut().ok_or_else(|| EngineError::Other("this library isn't synced".into()))?;
        st.config.server_previews = Some(on);
        self.persist()
    }

    /// Someone is looking at the storage numbers: have the server asked again (at most every
    /// [`USAGE_EVERY`]) by the next [`Session::sync_tasks`].
    pub fn sync_want_usage(&mut self) {
        if let Some(st) = self.sync.as_mut() {
            st.usage_wanted = true;
        }
    }

    /// Ask the server for the storage numbers again at once, instead of when the last answer is
    /// old.
    pub fn sync_refresh_usage(&mut self) {
        if let Some(st) = self.sync.as_mut() {
            st.usage_wanted = true;
            st.usage_at = None;
        }
    }

    /// Ask the server what it keeps for this library now and wait for the answer (blocking: the
    /// CLI, tests), through `run`. Silent when signed out or paused: the last numbers stay.
    pub fn sync_fetch_usage_with(&mut self, run: &mut dyn FnMut(&Task) -> Done) {
        self.sync_refresh_usage();
        let Some(st) = self.sync.as_mut().filter(|st| !st.config.paused && !st.config.token.is_empty() && st.usage_task.is_none()) else {
            return;
        };
        st.usage_wanted = false;
        let t = st.http("GET", "/api/usage", Body::Empty, None);
        st.usage_task = Some(t.id());
        let d = run(&t);
        self.sync_done(d);
    }

    /// Is photo `id` kept on this device even offline?
    pub fn sync_is_offline(&self, id: PhotoId) -> bool {
        let Some(st) = &self.sync else { return false };
        st.config.offline_photos.contains(&id) || st.config.offline_albums.iter().any(|a| self.catalog.album_photos(*a).contains(&id))
    }

    /// What the host should do next: at most one request that orders the library (sign-in,
    /// reload, pull or push) plus photo file transfers. Cheap when there's nothing to do; call
    /// it once per frame (or after [`Session::sync_done`]).
    pub fn sync_tasks(&mut self) -> Vec<Task> {
        if self.sync.as_ref().is_none_or(|st| st.config.paused) {
            return Vec::new();
        }
        let Some(mut st) = self.sync.take() else { return Vec::new() };
        if self.interaction.is_none() {
            if let Some(snap) = st.held_snapshot.take() {
                self.snapshot_done(&mut st, snap);
            }
            if !st.held.is_empty() {
                let held = std::mem::take(&mut st.held);
                self.apply_pulled(&mut st, held);
            }
        }
        if std::mem::take(&mut st.landed) {
            // renders that failed for want of a file try again once the catalog moves on
            let was_current = st.planned.is_some_and(|p| p.0 == self.catalog.revision);
            self.catalog.revision += 1;
            if was_current && let Some(p) = st.planned.as_mut() {
                p.0 = self.catalog.revision;
            }
        }
        let now = Instant::now();
        let mut tasks = Vec::new();
        // (search questions don't wait for the library's own sync to be idle)
        if !st.config.token.is_empty() {
            tasks.extend(st.aux_ready.drain(..));
        }
        let waiting = st.retry_at.is_some_and(|t| now < t);
        // a slider drag holds a preview value in the catalog: nothing changes it meanwhile
        if st.control.is_none() && !waiting && self.interaction.is_none() {
            if let Some(login) = st.login.take() {
                let body = Body::Json(serde_json::to_string(&login).unwrap_or_default());
                let t = st.http("POST", "/api/login", body, None);
                st.control = Some((t.id(), Control::Login(login)));
                tasks.push(t);
            } else if !st.config.token.is_empty() {
                if st.needs_snapshot {
                    let t = st.http("GET", "/api/snapshot", Body::Empty, None);
                    st.control = Some((t.id(), Control::Snapshot));
                    tasks.push(t);
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
                } else if !st.config.library.is_empty() && st.presets_remote > st.config.presets_version {
                    let t = st.http("GET", "/api/presets", Body::Empty, None);
                    st.control = Some((t.id(), Control::GetPresets));
                    tasks.push(t);
                } else if !st.config.library.is_empty() && self.presets_changed(&mut st) {
                    let presets = self.user_presets();
                    let body = serde_json::to_string(&proto::Presets { version: st.config.presets_version, presets: Value::Array(presets.clone()) })
                        .unwrap_or_default();
                    let t = st.http("PUT", "/api/presets", Body::Json(body), None);
                    st.control = Some((t.id(), Control::PutPresets(presets)));
                    tasks.push(t);
                }
            }
        }
        if st.usage_wanted
            && st.usage_task.is_none()
            && !st.config.token.is_empty()
            && !waiting
            && st.usage_at.is_none_or(|t| t.elapsed() >= USAGE_EVERY)
        {
            st.usage_wanted = false;
            let t = st.http("GET", "/api/usage", Body::Empty, None);
            st.usage_task = Some(t.id());
            tasks.push(t);
        }
        if !st.config.token.is_empty() && !st.config.library.is_empty() && !st.needs_snapshot && !waiting {
            self.blob_tasks(&mut st, now, &mut tasks);
        }
        self.sync = Some(st);
        tasks
    }

    /// Plan photo file transfers (when the library, the active photo or what to keep offline
    /// changed) and start some.
    fn blob_tasks(&mut self, st: &mut SyncState, now: Instant, tasks: &mut Vec<Task>) {
        let dir = self.media.smart_dir.clone();
        if dir.is_none() && self.sync_store.is_none() {
            return;
        }
        // a transfer that failed is tried again once its wait is over
        if st.blob_retry.values().any(|t| now >= *t) {
            st.blob_retry.retain(|_, t| now < *t);
            st.plan_gen += 1;
        }
        let key = (self.catalog.revision, self.active(), st.plan_gen);
        if st.planned != Some(key) {
            if st.have.is_none() {
                st.have = Some(match (&dir, &self.sync_store) {
                    (Some(d), _) => read_names(d),
                    (None, Some(s)) => s.proxies.clone(),
                    (None, None) => HashSet::new(),
                });
            }
            st.plan = self.plan_blobs(st, now).into();
            st.planned = Some(key);
        }
        while st.jobs.len() < BLOB_PARALLEL {
            let Some(mut job) = st.ready.pop_front().or_else(|| st.plan.pop_front()) else { break };
            let target = job.target();
            if st.jobs.values().any(|j| j.target() == target) {
                continue;
            }
            if let Job::Proxies { key, id, .. } = &job {
                let Some((smart, mini)) = self.proxy_paths(*id) else { continue };
                // built before (Build Smart Previews, or an earlier try): send them
                let built = match &self.media.smart_dir {
                    Some(_) => crate::smart::is_valid(Path::new(&smart)) && crate::smart::is_valid(Path::new(&mini)),
                    None => st.have.as_ref().is_some_and(|h| {
                        let (s, m) = store_proxy_names(key);
                        h.contains(&s) && h.contains(&m)
                    }),
                };
                if built {
                    job = Job::Put { key: key.clone(), blob: Blob::Smart, path: smart, id: *id };
                }
            }
            let task = match &job {
                Job::Head { key, .. } => st.http("HEAD", &format!("/api/blobs/original/{key}"), Body::Empty, None),
                Job::Put { key, blob, path, .. } => {
                    // (the server builds the previews of an original it is sent this way; one that doesn't know the
                    // parameter ignores it and the device builds them, see `job_done`)
                    let ask = if *blob == Blob::Original && st.config.server_builds_previews() { "?previews=1" } else { "" };
                    st.http("PUT", &format!("/api/blobs/{}/{key}{ask}", blob.name()), Body::File(path.clone()), None)
                }
                Job::Get { key, blob, dest } => st.http("GET", &format!("/api/blobs/{}/{key}", blob.name()), Body::Empty, Some(dest.clone())),
                Job::Proxies { path, id, .. } => {
                    let Some((smart, mini)) = self.proxy_paths(*id) else { continue };
                    Task::Proxies { id: NEXT_TASK.fetch_add(1, Ordering::Relaxed), original: path.clone(), smart, mini }
                }
            };
            st.jobs.insert(task.id(), job);
            tasks.push(task);
        }
    }

    /// Where a photo's smart and mini previews live on this device: files in the previews folder,
    /// else keys in the host's storage (`None`: neither).
    fn proxy_paths(&self, id: PhotoId) -> Option<(String, String)> {
        let p = self.catalog.photo(id)?;
        if let Some(dir) = &self.media.smart_dir {
            let s = |n: String| dir.join(n).to_string_lossy().to_string();
            return Some((s(crate::smart::file_name(p)), s(mini_file_name(p))));
        }
        let store = self.sync_store.as_ref()?;
        let (smart, mini) = store_proxy_names(&blob_key(p)?);
        Some((format!("{}{smart}", store.prefix), format!("{}{mini}", store.prefix)))
    }

    /// The file names [`Session::proxy_paths`] ends in, and which of them are here.
    fn proxy_names(&self, p: &Photo, key: &str, have: &HashSet<String>) -> (String, String, bool, bool) {
        let (smart, mini) = match &self.sync_store {
            Some(_) if self.media.smart_dir.is_none() => store_proxy_names(key),
            _ => (crate::smart::file_name(p), mini_file_name(p)),
        };
        let (s, m) = (have.contains(&smart), have.contains(&mini));
        (smart, mini, s, m)
    }

    /// Where downloaded originals are kept.
    pub(crate) fn originals_dir(&self) -> Option<PathBuf> {
        self.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.join("sync").join("originals"))
    }

    /// Photo file transfers this device needs: uploads of the originals it has that the server
    /// doesn't, then downloads — the smart previews of the active photo and of what's available
    /// offline, mini previews of every photo whose file isn't here, and wanted originals.
    fn plan_blobs(&mut self, st: &SyncState, now: Instant) -> Vec<Job> {
        let empty = HashSet::new();
        let have = st.have.as_ref().unwrap_or(&empty);
        let retry_ok = |key: &str| st.blob_retry.get(key).is_none_or(|t| now >= *t);
        let mut offline: HashSet<PhotoId> = st.config.offline_photos.iter().copied().collect();
        for a in &st.config.offline_albums {
            offline.extend(self.catalog.album_photos(*a));
        }
        let active = self.active();
        let originals_dir = self.originals_dir();
        let (mut ups, mut smart, mut minis, mut originals) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let mut tiers = HashMap::new();
        let store = self.sync_store.as_ref().filter(|_| self.media.smart_dir.is_none());
        let in_store = |key: &str| store.is_some_and(|s| s.originals.contains(key));
        for p in self.catalog.photos().filter(|p| !p.local && p.copy_of.is_none()) {
            let (Some(key), Source::File { path }) = (blob_key(p), &p.source) else { continue };
            // this device has the file: on its disk, or in the host's storage
            let here = match (path.starts_with(PATH_PREFIX), in_store(&key)) {
                (false, _) => Some(path.clone()),
                (true, true) => Some(BrowserStore::original_key(&key)),
                (true, false) => None,
            };
            if let Some(file) = here {
                if !st.uploaded.contains(&key) && !st.refused.contains(&key) && retry_ok(&key) {
                    ups.push(Job::Head { key, path: file, id: p.id });
                }
                continue;
            }
            let (smart_name, mini_name, has_smart, has_mini) = self.proxy_names(p, &key, have);
            let dest = |name: &str| match (&self.media.smart_dir, store) {
                (Some(dir), _) => dir.join(name).to_string_lossy().to_string(),
                (None, Some(s)) => format!("{}{name}", s.prefix),
                (None, None) => name.to_string(),
            };
            tiers.insert(crate::media::content_key(p), if has_smart { 2 } else { u8::from(has_mini) });
            if !retry_ok(&key) {
                continue;
            }
            let pinned = offline.contains(&p.id);
            if (pinned || active == Some(p.id)) && !has_smart {
                let job = Job::Get { key: key.clone(), blob: Blob::Smart, dest: dest(&smart_name) };
                if active == Some(p.id) { smart.insert(0, job) } else { smart.push(job) }
            }
            if !has_mini && !has_smart {
                minis.push(Job::Get { key: key.clone(), blob: Blob::Mini, dest: dest(&mini_name) });
            }
            if st.config.store_originals || st.want_originals.contains(&key) {
                let dest = match (&originals_dir, store) {
                    (Some(odir), _) => Some(odir.join(&key).join(file_name_of(path, &p.file_name)).to_string_lossy().to_string()),
                    (None, Some(_)) => Some(BrowserStore::original_key(&key)),
                    (None, None) => None,
                };
                if let Some(dest) = dest {
                    originals.push(Job::Get { key, blob: Blob::Original, dest });
                }
            }
        }
        self.media.synced_tiers = tiers;
        ups.into_iter().chain(smart).chain(minis).chain(originals).collect()
    }

    /// A finished task.
    pub fn sync_done(&mut self, done: Done) {
        let Some(mut st) = self.sync.take() else { return };
        if st.control.as_ref().is_some_and(|(id, _)| *id == done.id) {
            if let Some((_, c)) = st.control.take() {
                self.control_done(&mut st, c, &done);
            }
        } else if let Some(aux) = st.aux.remove(&done.id) {
            self.vision_server_done(&mut st, aux, &done);
        } else if st.usage_task == Some(done.id) {
            st.usage_task = None;
            st.usage_done(&done);
        } else if let Some(job) = st.jobs.remove(&done.id) {
            self.job_done(&mut st, job, &done);
        }
        self.sync = Some(st);
        if let Err(e) = self.persist() {
            log::error!("sync: {e}");
        }
    }

    fn control_done(&mut self, st: &mut SyncState, c: Control, d: &Done) {
        if d.status == 401 && !matches!(c, Control::Login(_)) {
            st.signed_out("the server signed this device out: sign in again");
            return;
        }
        match c {
            Control::Login(login) => match (d.ok(), decode::<proto::Device>(d)) {
                (true, Ok(dev)) if dev.space == 0 || dev.space > MAX_SPACE => st.signed_out("the server gave this device an unusable id space"),
                (true, Ok(dev)) => {
                    if !st.config.library.is_empty() && st.config.library != dev.library {
                        st.signed_out(NOT_THIS_LIBRARY);
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
                (false, _) if d.status == 0 || d.status >= 500 => {
                    // try again later: the server may be away
                    st.login = Some(login);
                    st.fail(why(d));
                }
                (false, _) => st.signed_out(&why(d)),
            },
            Control::Snapshot => match (d.ok(), decode::<proto::Snapshot>(d)) {
                (true, Ok(snap)) if self.interaction.is_some() => st.held_snapshot = Some(snap),
                (true, Ok(snap)) => self.snapshot_done(st, snap),
                (true, Err(e)) => st.fail(e),
                (false, _) => st.fail(why(d)),
            },
            Control::Pull => match d.status {
                410 => st.needs_snapshot = true,
                _ if d.ok() => match decode::<proto::Ops>(d) {
                    Ok(ops) => {
                        let more = ops.ops.len() >= PULL_LIMIT;
                        st.presets_remote = ops.presets;
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
                        st.outbox_changed();
                    }
                    st.needs_snapshot = true;
                }
                _ if d.ok() => match decode::<proto::Head>(d) {
                    Ok(h) => {
                        st.outbox.acked(&pushed);
                        st.outbox_changed();
                        st.config.cursor = h.head;
                        st.succeeded();
                    }
                    Err(e) => st.fail(e),
                },
                _ => st.fail(why(d)),
            },
            Control::GetPresets => match (d.ok(), decode::<proto::Presets>(d)) {
                (true, Ok(p)) => {
                    self.take_presets(st, p);
                    st.succeeded();
                }
                (true, Err(e)) => st.fail(e),
                (false, _) => st.fail(why(d)),
            },
            Control::PutPresets(sent) => match d.status {
                // another device changed them first: the answer is theirs, merge and send again
                412 => match decode::<proto::Presets>(d) {
                    Ok(p) => self.take_presets(st, p),
                    Err(e) => st.fail(e),
                },
                _ if d.ok() => match serde_json::from_str::<Value>(&d.body).ok().and_then(|v| v["version"].as_u64()) {
                    Some(v) => {
                        st.config.presets_version = v;
                        st.presets_remote = st.presets_remote.max(v);
                        st.presets_base = sent;
                        st.presets_base_dirty = true;
                        st.presets_seen = None;
                        st.succeeded();
                    }
                    None => st.fail("unexpected answer from the server".into()),
                },
                _ => st.fail(why(d)),
            },
        }
    }

    /// This device's user presets, as synced.
    fn user_presets(&self) -> Vec<Value> {
        self.presets.iter().filter(|p| !p.builtin).filter_map(|p| serde_json::to_value(p).ok()).collect()
    }

    /// Did the user presets change since they were last synced? (Compared only after presets.json
    /// was written, which happens when they change.)
    fn presets_changed(&self, st: &mut SyncState) -> bool {
        let generation = self.library.as_ref().map(|l| l.presets_gen());
        if st.presets_seen != generation {
            st.presets_seen = generation;
            st.presets_changed = self.user_presets() != st.presets_base;
        }
        st.presets_changed
    }

    /// The server's presets document (`theirs`) merged into this device's.
    fn take_presets(&mut self, st: &mut SyncState, theirs: proto::Presets) {
        let remote: Vec<Value> = theirs.presets.as_array().cloned().unwrap_or_default();
        let merged = merge_presets(&st.presets_base, &self.user_presets(), &remote);
        let builtin: HashSet<String> = self.presets.iter().filter(|p| p.builtin).map(|p| p.id.clone()).collect();
        let mut users = Vec::new();
        let mut seen = HashSet::new();
        for v in merged.into_iter().take(PRESETS_MAX) {
            // untrusted: a preset that doesn't read as one, a built-in's id or a repeated id is left out
            let Ok(mut p) = serde_json::from_value::<lightcraft_develop::Preset>(v) else { continue };
            if p.id.trim().is_empty() || p.id.len() > 200 || builtin.contains(&p.id) || !seen.insert(p.id.clone()) {
                continue;
            }
            p.builtin = false;
            users.push(p);
        }
        self.presets.retain(|p| p.builtin);
        self.presets.extend(users);
        st.presets_base = remote;
        st.presets_base_dirty = true;
        st.config.presets_version = theirs.version;
        st.presets_remote = st.presets_remote.max(theirs.version);
        st.presets_seen = None;
    }

    fn snapshot_done(&mut self, st: &mut SyncState, snap: proto::Snapshot) {
        match self.adopt_snapshot(st, snap) {
            Ok(()) => st.succeeded(),
            Err(e) => st.signed_out(&e),
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
            let merged = st.outbox.merge_remote(&op);
            st.outbox_changed();
            if let Some(op) = merged {
                removed |= is_removal(&op);
                match self.catalog.apply(op.clone()) {
                    Ok(_) => {
                        self.pending_log.push(op);
                        applied = true;
                    }
                    // our own photo coming back after a lost acknowledgement, already here
                    Err(_) if matches!(&op, Op::AddPhoto { photo } if self.catalog.photo(photo.id).is_some()) => {}
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
        // joining: this library's ids name other photos than the server's
        let joining = st.config.library != snap.library;
        if joining {
            if !st.config.library.is_empty() {
                return Err(NOT_THIS_LIBRARY.into());
            }
            // the procedural demo photos a new library starts with don't count: the server's
            // library replaces them (an empty one too: they are never uploaded on their own)
            let user_photo = |p: &Photo| !p.local && !matches!(p.source, Source::Demo { .. });
            let has_photos = self.catalog.photos().any(|p| user_photo(p))
                || self
                    .catalog
                    .albums()
                    .any(|a| a.smart.is_some() || a.photos.iter().any(|id| self.catalog.photo(*id).is_some_and(|p| user_photo(p))));
            let server_empty = snap.seq == 0 && snap.catalog.is_empty() && snap.catalog.albums().next().is_none();
            if server_empty && has_photos {
                // the first device: upload this library
                st.config.library = snap.library;
                st.config.cursor = 0;
                st.outbox.seed(&self.catalog);
                st.outbox_changed();
                return Ok(());
            }
            if has_photos {
                return Err(
                    "this library has photos and the server already has a library: sign in from a new library to get the server's photos".into()
                );
            }
            st.config.library = snap.library;
        }
        let mut c: Catalog = snap.catalog;
        lightcraft_catalog::sync::sanitize(&mut c);
        if joining {
            // nothing of the (empty or demo-only) library joining carries over by id
            self.selection = crate::Selection::default();
            self.previous_active = None;
            self.before.clear();
        } else {
            carry_local(&self.catalog, &mut c);
        }
        c.set_id_space(st.config.space);
        let dropped = st.outbox.rebase(&mut c);
        st.outbox_changed();
        if dropped > 0 {
            log::info!("sync: {dropped} change(s) made here no longer apply to the server's library");
        }
        st.config.cursor = snap.seq;
        st.planned = None;
        self.replace_catalog(c);
        Ok(())
    }

    /// Switch to another state of the library wholesale (a snapshot replaces the op log).
    fn replace_catalog(&mut self, mut c: Catalog) {
        if let Err(e) = self.persist() {
            log::error!("sync: {e}");
        }
        c.revision = self.catalog.revision + 1;
        self.catalog = c;
        self.undo.clear();
        self.redo.clear();
        self.interaction = None;
        self.selection.ids.retain(|id| self.catalog.photo(*id).is_some());
        self.selection.active = self.selection.active.filter(|id| self.catalog.photo(*id).is_some());
        self.media.clear_sources();
        let unlogged = self.pending_log.len() as u64;
        if let Some(lib) = self.library.as_mut() {
            match lib.journal_mut().snapshot_with_unlogged(&self.catalog, unlogged) {
                Ok(()) => self.pending_log.clear(),
                Err(e) => log::error!("sync: saving the library: {e}"),
            }
        }
    }

    fn job_done(&mut self, st: &mut SyncState, job: Job, d: &Done) {
        let retry = |st: &mut SyncState, key: &str, why: String| {
            log::warn!("sync: {key}: {why}");
            st.blob_retry.insert(key.to_string(), Instant::now() + BLOB_RETRY);
            st.plan_gen += 1;
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
                Blob::Original => {
                    // the server answers `{"previews": "queued" | "built"}` when it is making them: nothing to build here
                    let by_server = st.config.server_builds_previews()
                        && serde_json::from_str::<Value>(&d.body).ok().is_some_and(|v| matches!(v["previews"].as_str(), Some("queued" | "built")));
                    if by_server {
                        self.uploaded(st, key);
                    } else {
                        st.ready.push_front(Job::Proxies { key, path, id });
                    }
                }
                Blob::Smart => match self.proxy_paths(id) {
                    Some((_, mini)) => st.ready.push_front(Job::Put { key, blob: Blob::Mini, path: mini, id }),
                    None => self.uploaded(st, key),
                },
                Blob::Mini => self.uploaded(st, key),
            },
            Job::Put { key, blob: Blob::Original, .. } if d.status == 422 => {
                log::warn!("sync: the server refused the original {key}: {}", why(d));
                st.refused.insert(key);
            }
            Job::Put { key, .. } => retry(st, &key, why(d)),
            Job::Proxies { key, id, .. } => match (d.ok(), self.proxy_paths(id)) {
                (true, Some((smart, mini))) => {
                    let names: Vec<String> =
                        [&smart, &mini].iter().filter_map(|p| Path::new(p).file_name()).map(|n| n.to_string_lossy().to_string()).collect();
                    if let Some(s) = self.sync_store.as_mut() {
                        s.proxies.extend(names.iter().cloned());
                    }
                    if let Some(have) = st.have.as_mut() {
                        have.extend(names);
                    }
                    st.ready.push_front(Job::Put { key, blob: Blob::Smart, path: smart, id });
                }
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
                st.landed = true;
                if blob == Blob::Original && self.media.smart_dir.is_none() {
                    // in the host's storage now, where the photo's path already points
                    self.uploaded(st, key.clone());
                    if let Some(s) = self.sync_store.as_mut() {
                        s.originals.insert(key.clone());
                    }
                } else if blob == Blob::Original {
                    // the file is here now: the photo points at it (on this device only); the
                    // server has it, so it's never sent back
                    self.uploaded(st, key.clone());
                    for id in ids {
                        let Some(p) = self.catalog.photo(id) else { continue };
                        if !is_remote(p) {
                            continue;
                        }
                        let op = Op::Relink { id, file_name: p.file_name.clone(), source: Source::File { path: dest.clone() }, format: None };
                        if self.catalog.apply(op.clone()).is_ok() {
                            self.pending_log.push(op);
                        }
                    }
                } else if let Some(have) = st.have.as_mut()
                    && let Some(n) = Path::new(&dest).file_name()
                {
                    let n = n.to_string_lossy().to_string();
                    if let Some(s) = self.sync_store.as_mut() {
                        s.proxies.insert(n.clone());
                    }
                    have.insert(n);
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

    /// Run the sync to completion here (blocking: the CLI, tests): sign-in, reload, pull, push
    /// and photo file transfers, until there's nothing left to do or `limit` tasks ran.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn sync_now(&mut self, limit: usize) -> Value {
        self.sync_now_with(limit, &mut run)
    }

    /// [`Session::sync_fetch_usage_with`] over the network.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn sync_fetch_usage(&mut self) {
        self.sync_fetch_usage_with(&mut run);
    }

    /// [`Session::sync_now`] with another transport (tests).
    pub fn sync_now_with(&mut self, limit: usize, run: &mut dyn FnMut(&Task) -> Done) -> Value {
        self.sync_soon();
        let mut n = 0;
        // until nothing is left to do now (the next pull waits for the poll interval, a failure
        // for its retry time)
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

/// Three-way merge of preset lists by preset id: what changed on this device since `base` (the
/// list both sides last agreed on) applied over `theirs`. A preset deleted on one side and
/// untouched on the other is gone; deleted on one and edited on the other, the edit stays; edited
/// on both, the settings merge field by field ([`lightcraft_catalog::sync::merge3`]). Theirs keep
/// their order; presets added here come after.
pub fn merge_presets(base: &[Value], ours: &[Value], theirs: &[Value]) -> Vec<Value> {
    let id = |v: &Value| v.get("id").and_then(Value::as_str).map(str::to_string);
    let index = |list: &[Value]| list.iter().filter_map(|v| Some((id(v)?, v.clone()))).collect::<HashMap<String, Value>>();
    let (b, o, t) = (index(base), index(ours), index(theirs));
    let mut order: Vec<String> = theirs.iter().filter_map(id).collect();
    order.extend(ours.iter().filter_map(id).filter(|i| !t.contains_key(i)));
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for i in order {
        if !seen.insert(i.clone()) {
            continue;
        }
        let kept = match (b.get(&i), o.get(&i), t.get(&i)) {
            (base, Some(o), Some(t)) => Some(lightcraft_catalog::sync::merge3(base.unwrap_or(&Value::Null), o, t)),
            // deleted here: unless changed there meanwhile
            (Some(b), None, Some(t)) => (t != b).then(|| t.clone()),
            // deleted there: unless changed here meanwhile
            (Some(b), Some(o), None) => (o != b).then(|| o.clone()),
            (None, Some(v), None) | (None, None, Some(v)) => Some(v.clone()),
            _ => None,
        };
        out.extend(kept);
    }
    out
}

/// The file names in a folder (none if it can't be read).
fn read_names(dir: &Path) -> HashSet<String> {
    let Ok(rd) = std::fs::read_dir(dir) else { return HashSet::new() };
    rd.flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect()
}

/// The file name an original is kept under (its name in the catalog path, else the photo's).
fn file_name_of(path: &str, fallback: &str) -> String {
    let n = path.rsplit(['/', '\\']).next().filter(|n| !n.is_empty() && *n != "." && *n != "..").unwrap_or(fallback);
    let n = n.replace(['/', '\\', ':'], "_");
    if n.is_empty() || n == "." || n == ".." { "photo".into() } else { n }
}

/// Path of a photo file kept by content (`web/<hash>/<name>`), for hosts.
pub fn content_path(p: &Photo) -> Option<String> {
    Some(original_path(&blob_key(p)?, &p.file_name))
}

/// Have the sync server render a photo (`POST /api/render`): the encoded image. Blocking, and for as long as the
/// render takes (minutes for a big photo on a small server): run it off the UI thread.
#[cfg(not(target_arch = "wasm32"))]
pub fn render_on_server(target: &RenderTarget, request: &proto::Render) -> std::result::Result<Vec<u8>, String> {
    net::render(&target.url, &target.token, &serde_json::to_string(request).map_err(|e| e.to_string())?)
}

/// Build a photo's smart and mini previews from its original (decodes it).
#[cfg(not(target_arch = "wasm32"))]
pub fn build_proxies(original: &str, smart: &str, mini: &str) -> std::result::Result<(), String> {
    let bytes = std::fs::read(original).map_err(|e| format!("{original}: {e}"))?;
    let (s, m) = crate::smart::encode_pair(&bytes, MINI_EDGE)?;
    let write = |path: &str, b: Vec<u8>| lightcraft_catalog::safe_file::write_atomic(Path::new(path), &b).map_err(|e| format!("{path}: {e}"));
    if let Some(dir) = Path::new(smart).parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    write(smart, s)?;
    write(mini, m)
}

/// Run a task here (blocking; native hosts call it on a worker thread). Never panics: a panic
/// in the HTTP or TLS stack is this task's failure.
#[cfg(not(target_arch = "wasm32"))]
pub fn run(task: &Task) -> Done {
    let id = task.id();
    crate::guard::catch("sync", || match task {
        Task::Proxies { id, original, smart, mini } => match build_proxies(original, smart, mini) {
            Ok(()) => Done { id: *id, status: 200, body: String::new() },
            Err(e) => Done::failed(*id, e),
        },
        Task::Http { id, method, url, token, body, save_to } => match net::http(method, url, token, body, save_to.as_deref()) {
            Ok((status, body)) => Done { id: *id, status, body },
            Err(e) => Done::failed(*id, e),
        },
    })
    .unwrap_or_else(|e| Done::failed(id, e))
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
                .root_certs(ureq::tls::RootCerts::WebPki)
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

    /// `POST` JSON and read the answer's bytes, waiting for a render ([`super::render_on_server`]).
    pub(super) fn render(url: &str, token: &str, json: &str) -> Result<Vec<u8>, String> {
        let auth = format!("Bearer {token}");
        let mut resp = agent()
            .post(url)
            .header("Authorization", &auth)
            .header("Content-Type", "application/json")
            .config()
            .timeout_recv_response(Some(Duration::from_secs(900)))
            .build()
            .send(json.as_bytes())
            .map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        let body = resp.body_mut().with_config().limit(1 << 30).read_to_vec().map_err(|e| e.to_string())?;
        if (200..300).contains(&status) {
            return Ok(body);
        }
        let why = serde_json::from_slice::<serde_json::Value>(&body).ok().and_then(|v| v["error"].as_str().map(str::to_string));
        Err(match (status, why) {
            (404, _) if body.is_empty() || why_is_route(&body) => "this server can't render (it is older than this app)".to_string(),
            (_, Some(why)) => why,
            (s, None) => format!("the server answered {s}"),
        })
    }

    /// A `404` for the route itself (an older server), not for a missing original.
    fn why_is_route(body: &[u8]) -> bool {
        String::from_utf8_lossy(body).contains("no route")
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
            let mut reader = resp.body_mut().with_config().limit(u64::MAX).reader();
            lightcraft_catalog::safe_file::write_atomic_with(path, &mut |w| std::io::copy(&mut reader, w).map(|_| ()))
                .map_err(|e| format!("{dest}: {e}"))?;
            return Ok((status, String::new()));
        }
        let mut text = String::new();
        resp.body_mut().with_config().limit(TEXT_MAX).reader().read_to_string(&mut text).map_err(|e| e.to_string())?;
        Ok((status, text))
    }
}
