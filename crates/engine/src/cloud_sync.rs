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
//!   other photos as the grid shows them (thumbnails, rendered here with the current edits; all
//!   of them with [`SyncConfig::all_previews`]), the smart preview of the photo being looked at and
//!   of everything made available offline, and originals when asked to keep them. A downloaded original becomes the photo's file on this device. A photo
//!   whose file isn't here renders from the best of those it has ([`crate::media::SourceRef::Synced`]).
//!
//! - **Presets** are one document on the server (`/api/presets`, versioned). A device sends its
//!   user presets when they change, and merges the server's by preset id against the copy both
//!   last agreed on ([`merge_presets`]), so a preset deleted on one device stays deleted. What a
//!   device leaves out of the server's document (past the most it holds, or not readable as a preset)
//!   goes back with what it sends, so leaving something out never deletes it on the other devices.
//!
//! Files in the library: `sync.json` ([`SyncConfig`]), `sync.outbox` (changes the server hasn't
//! acknowledged; written before the op log), `sync.uploaded` (content hashes the server has) and
//! `sync.presets` (the user presets as last synced).

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use lightcraft_catalog::join::{self, Counts};
use lightcraft_catalog::sync::{Outbox, PATH_PREFIX, Pushed, carry_local, original_path, proto};
use lightcraft_catalog::{AlbumId, Catalog, Op, Photo, PhotoId, Source, Store};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use web_time::Instant;

use crate::{EngineError, Result, Session};

mod docs;
mod evict;
mod leave;
mod shares;

pub use shares::{SHARE_PASSWORD_MAX, SHARE_PASSWORD_MIN, ShareOptions};

pub const CONFIG: &str = "sync.json";
pub const OUTBOX: &str = "sync.outbox";
pub const UPLOADED: &str = "sync.uploaded";
pub const PRESETS: &str = "sync.presets";
/// The other shared documents as last synced (the base of their merges), by name.
pub const DOCS: &str = "sync.docs";
/// User presets a device holds at most (the server's document is untrusted input): about ten
/// Lightroom collections imported whole (one is ~11 000 presets). The device's own presets always
/// stay; what is left out of the server's is sent back as it came ([`Session::presets_to_send`]).
const PRESETS_MAX: usize = 100_000;
/// A synced preset's id is this long at most.
const PRESET_ID_MAX: usize = 200;

/// Signing in to a server whose library this one isn't a copy of (the server was set up again,
/// or the user's library there was replaced).
const NOT_THIS_LIBRARY: &str = "this library is a copy of another library on that server (was the server set up again?): \
     remove it from this device (Settings › Sync) and sign in again, or sign in from a new library (Settings › General › Open Library…; \
     in a browser, open the server's address with ?reset)";

/// How often the server is asked for other devices' changes.
pub const POLL: Duration = Duration::from_secs(5);
/// Longest wait between retries after a failure (network down, server away).
const BACKOFF_MAX: Duration = Duration::from_secs(60);
/// A missing or failed photo file is tried again after this long.
const BLOB_RETRY: Duration = Duration::from_secs(60);
/// Ops per push / pull.
const PUSH_LIMIT: usize = 500;
const PULL_LIMIT: usize = 2000;
/// Photos per page when reading the server's library (the server also keeps a page to ~8 MiB).
const SNAPSHOT_PAGE: usize = 2000;
/// Photo file transfers at a time.
const BLOB_PARALLEL: usize = 4;
/// Downloaded originals are checked against their budget this often (and right after one arrives).
const EVICT_EVERY: Duration = Duration::from_secs(60);
/// The server's storage numbers are asked for at most this often (it walks every photo file).
const USAGE_EVERY: Duration = Duration::from_secs(20);
/// What the server is doing (scans, previews…) is asked for this often while someone looks.
const ACTIVITY_EVERY: Duration = Duration::from_secs(2);
/// Long edge of the mini preview.
pub const MINI_EDGE: usize = 512;
/// A grid cell's mini preview is wanted while the cell asks for it again within this many sync
/// steps (frames): one scrolled out of the grid's reach isn't downloaded. (A cell that is drawn asks
/// every frame, see [`Session::note_thumb_ask`].)
const ASK_TICKS: u64 = 30;
/// A preview the server said it is building is asked for again after this long (it builds the ones a
/// device asks for before the rest, so it takes seconds, not the minute of [`BLOB_RETRY`]).
pub(crate) const BUILDING_RETRY: Duration = Duration::from_secs(4);
/// What a mini preview is taken to weigh before the server says what its own weigh.
const MINI_BYTES_GUESS: u64 = 60_000;
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
    /// The most the originals downloaded from the server may take on this device, in MB: the ones used longest
    /// ago are deleted beyond it (never one the server doesn't have, one that is open, selected or available
    /// offline; not at all with [`SyncConfig::store_originals`]). `None`: no limit.
    pub originals_budget_mb: Option<u64>,
    /// Don't talk to the server for now (File → Pause Syncing).
    pub paused: bool,
    /// Let the server build the previews of the originals this device uploads, instead of building them here
    /// (a phone saves its battery and memory; a browser tab its main thread, where decoding a large raw takes
    /// seconds). `None`: the platform's default, which is on for iOS and the browser.
    pub server_previews: Option<bool>,
    /// Download the mini previews of every photo, not only of the ones the grid shows (browsing
    /// offline, at the cost of the disk space [`Session::sync_previews_left`] estimates).
    pub all_previews: bool,
    /// Version of the server's presets document this device last synced with.
    pub presets_version: u64,
    /// The same for the other shared documents ([`docs`]), by name.
    pub docs_versions: std::collections::BTreeMap<String, u64>,
    /// The server's library being read into this one page by page, shown as it arrives.
    pub filling: Option<Filling>,
    /// Signing out: what this device still has to send is being sent, and the device keeps its token until it is
    /// (`leave.rs`). Then it signs out on the server and the library is emptied.
    pub leaving: bool,
    /// The wipe's own marker: the library is being emptied after signing out. Written before anything is removed, so
    /// opening the library finishes a wipe that was cut short. Nothing else sets it.
    pub wiping: bool,
}

/// A device joining a server's library with no photos of its own takes the server's: each page
/// of it goes into the library as it arrives ([`SyncConfig::filling`]), so its photos show (and
/// their previews download) while the rest comes, and a read carries on after a restart.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Filling {
    /// The copy of the server's library read: the op it is at.
    pub seq: u64,
    /// Where the next page starts; `None`: from the first page (the copy was gone).
    pub next: Option<String>,
    /// Photos read so far, and in the server's library.
    pub read: usize,
    pub total: usize,
    /// Read again from the first page: changes made here meanwhile are made again at the end.
    pub restarted: bool,
}

impl SyncConfig {
    /// Whether the server builds the previews of what this device uploads ([`SyncConfig::server_previews`]).
    pub fn server_builds_previews(&self) -> bool {
        self.server_previews.unwrap_or(cfg!(any(target_os = "ios", target_arch = "wasm32")))
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
    /// The rest of a file from byte `offset` (an upload the server has the start of), sent with
    /// `Content-Range: bytes <offset>-<last>/<length>`. An offset at or past the end sends the whole file.
    FileFrom(String, u64),
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
    /// A shared document ([`docs`]) by name, and what was sent.
    GetDoc(String),
    PutDoc(String, Vec<Value>),
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
        /// The server already has this many bytes of the file (an upload that broke): send the rest.
        offset: u64,
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
        id: PhotoId,
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

/// A library with photos signed in to a server that has a library already: two libraries that
/// never met, whose ids name different photos. Nothing syncs until the user chooses what to do
/// ([`Session::sync_resolve`]); the server's library waits here, as it was when fetched.
struct Conflict {
    snapshot: proto::Snapshot,
    info: ConflictInfo,
}

/// What the sign-in choice is about: this library and the server's.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictInfo {
    /// The server (`photos.example.com`) and the user signed in as.
    pub server: String,
    pub user: String,
    /// This library, as joining would carry it over.
    pub here: Counts,
    /// The server's library.
    pub there: Counts,
    /// Photos both have (the same file): joining keeps them once.
    pub shared: usize,
}

/// What to do about a [`ConflictInfo`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolution {
    /// Add this library's photos, edits and albums to the server's library (and get the server's).
    Upload,
    /// Replace this library with the server's (a copy of this one is kept).
    UseServer,
    /// Don't sync: sign out and leave this library as it is.
    Cancel,
}

impl Resolution {
    /// `upload` / `useServer` / `cancel` (the `sync.resolveConflict` parameter).
    pub fn parse(s: &str) -> Option<Resolution> {
        match s.trim().to_ascii_lowercase().as_str() {
            "upload" | "merge" | "join" => Some(Resolution::Upload),
            "useserver" | "replace" | "server" => Some(Resolution::UseServer),
            "cancel" | "signout" | "keep" => Some(Resolution::Cancel),
            _ => None,
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            Resolution::Upload => "upload",
            Resolution::UseServer => "useServer",
            Resolution::Cancel => "cancel",
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
    pub(crate) uploaded: HashSet<String>,
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
    /// The server's library read so far, while it is read in pages (its `next` is where to go on).
    reading: Option<proto::Snapshot>,
    /// Photo files whose transfer failed: not tried again before then.
    blob_retry: HashMap<String, Instant>,
    /// Previews the server said it is building (answer `previews: "queued"`): not asked for again before
    /// then. Not a failure: nothing is shown as one, and no plan is made again when the wait is over.
    building: HashMap<String, Instant>,
    /// Originals the server refused (not the file their hash says): not sent again this session.
    refused: HashSet<String>,
    /// Originals asked for (by content hash).
    want_originals: HashSet<String>,
    error: Option<String>,
    /// What the transfer plan was made for (catalog revision, settings changes). It holds the uploads,
    /// the smart previews of what is kept offline and the originals to download; the active photo's
    /// smart preview and the grid's mini previews are [`SyncState::urgent`], asked for every step.
    planned: Option<(u64, u64)>,
    plan: VecDeque<Job>,
    /// Bumped when what to keep offline changes.
    plan_gen: u64,
    /// With [`SyncConfig::all_previews`]: every mini preview missing here, and what that list was made
    /// for (the number of photos, `minis_gen`). Not made again for each edit, as the plan is: it
    /// takes a while in a big library.
    minis: VecDeque<Job>,
    minis_planned: Option<(usize, u64)>,
    minis_gen: u64,
    /// Mini previews of that list whose download failed: back in it once their wait is over.
    minis_failed: Vec<(Instant, Job)>,
    /// The active photo's smart preview and the mini previews the grid asks for, most recent first,
    /// that haven't started.
    urgent: Vec<Job>,
    /// Mini previews still missing here, as last counted (photos in the library then, count; less
    /// one for each that came since): see [`Session::sync_previews_left`].
    minis_left: Option<(usize, usize)>,
    /// File names in the proxies folder (read once, then kept up to date).
    have: Option<HashSet<String>>,
    /// An original arrived: renders that failed for want of it are tried again.
    landed: bool,
    /// When downloaded originals are next checked against their budget (`None`: now).
    evict_at: Option<Instant>,
    /// The photo whose original was last marked as used ([`Session::touch_active_original`]).
    touched: Option<PhotoId>,
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
    /// The other shared documents ([`docs`]): the copy both sides last agreed on and what's new, by name.
    docs: std::collections::BTreeMap<String, docs::DocState>,
    docs_base_dirty: bool,
    /// Which prefs.json write was last compared.
    docs_seen: Option<u64>,
    /// Signing in to a server that has a library when this one has photos: combine the two
    /// ([`lightcraft_catalog::merge`]) instead of refusing. Only for this sign-in.
    merge: bool,
    /// What the last merge of this library into the server's found and did.
    merged: Option<lightcraft_catalog::merge::MergeReport>,
    /// Album links ([`shares`]).
    shares: shares::ShareState,
    /// What the server says this library takes there, when it said so, and the request in flight.
    usage: Option<proto::Usage>,
    usage_at: Option<Instant>,
    usage_task: Option<u64>,
    /// Why the last answer wasn't one (the numbers above are older than that).
    usage_error: Option<String>,
    /// Someone is looking (Settings ▸ Sync, `sync.usage`): ask again once the last answer is old.
    usage_wanted: bool,
    /// What the server says it is doing for this user, when it said so, and the request in flight.
    activity: Option<proto::Activity>,
    activity_at: Option<Instant>,
    activity_task: Option<u64>,
    activity_error: Option<String>,
    /// Someone is looking (the cloud popover, `sync.activity`): ask again soon.
    activity_wanted: bool,
    /// Photos uploaded and files downloaded since the queue was last empty (progress "12 of 56").
    up_done: usize,
    down_done: usize,
    /// Signed in to a server whose library this one has to be joined to or replaced by: waiting for the user.
    conflict: Option<Conflict>,
    /// Why the last photo file transfer failed (a full disk on the server, a refused file); cleared by the next that works.
    transfer_error: Option<String>,
    /// While signing out ([`SyncConfig::leaving`]): the activity stack's row (cancelling it stays signed in), how many
    /// photos still had to be sent when last counted and when, and the most that was left at once (the row's whole).
    signout: Option<crate::activity::TaskGuard>,
    unsent: Option<(Instant, usize)>,
    signout_peak: usize,
    /// Syncing was paused when the wait began (it is paused again if the wait is cancelled).
    signout_paused: bool,
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
            reading: None,
            blob_retry: HashMap::new(),
            building: HashMap::new(),
            refused: HashSet::new(),
            want_originals: HashSet::new(),
            error: None,
            planned: None,
            plan: VecDeque::new(),
            plan_gen: 0,
            minis: VecDeque::new(),
            minis_planned: None,
            minis_gen: 0,
            minis_failed: Vec::new(),
            urgent: Vec::new(),
            minis_left: None,
            have: None,
            landed: false,
            evict_at: None,
            touched: None,
            written_config: String::new(),
            presets_base: Vec::new(),
            presets_base_dirty: false,
            presets_remote: 0,
            presets_changed: false,
            presets_seen: None,
            docs: Default::default(),
            docs_base_dirty: false,
            docs_seen: None,
            merge: false,
            merged: None,
            shares: Default::default(),
            usage: None,
            usage_at: None,
            usage_task: None,
            usage_error: None,
            usage_wanted: false,
            activity: None,
            activity_at: None,
            activity_task: None,
            activity_error: None,
            activity_wanted: false,
            up_done: 0,
            down_done: 0,
            conflict: None,
            transfer_error: None,
            signout: None,
            unsent: None,
            signout_peak: 0,
            signout_paused: false,
        }
    }

    /// The wait for signing out is over without a sign-out (cancelled): back to how syncing was. What was dropped while
    /// it waited (downloads) is planned again.
    fn stop_leaving(&mut self) {
        self.config.leaving = false;
        self.config.paused = std::mem::take(&mut self.signout_paused);
        self.signout = None;
        self.unsent = None;
        self.planned = None;
        self.minis_planned = None;
    }

    /// (tests) Are the photo file transfers to be planned afresh at the next step?
    #[cfg(test)]
    pub(crate) fn plan_is_stale(&self) -> bool {
        self.planned.is_none()
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
        let mut config: SyncConfig = match serde_json::from_slice(&bytes) {
            Ok(c) => c,
            Err(e) => {
                log::error!("sync: {CONFIG} is damaged ({e}); sign in again");
                return None;
            }
        };
        // nothing in it: a library that was signed out and emptied (`leave.rs` leaves `{}`) reads as one that never synced
        if config == SyncConfig::default() {
            return None;
        }
        // (a wait for what is still to be sent needs the token it sends with)
        config.leaving &= !config.token.is_empty();
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
        st.load_docs_base(files.read(DOCS).ok().flatten().as_deref());
        if let Some(b) = files.read(UPLOADED).ok().flatten() {
            st.uploaded = String::from_utf8_lossy(&b).lines().filter(|l| !l.is_empty()).map(str::to_string).collect();
        }
        // pending changes made before the library closed: reload the server's state once and
        // replay them on top (merged), in case other devices changed what they touch
        st.needs_snapshot |= !st.config.library.is_empty() && !st.outbox.is_empty();
        // signed in but never joined to the server's library (closed while the choice at sign-in waited):
        // fetch it again, which asks again
        st.needs_snapshot |= st.config.library.is_empty() && !st.config.token.is_empty();
        // a library being filled in from the server's: the rest of it
        st.needs_snapshot |= st.config.filling.is_some();
        Some(st)
    }

    /// Signed in, or signing in (the sign-in waiting to be sent or on its way).
    pub fn signed_in(&self) -> bool {
        !self.config.token.is_empty() || self.login.is_some() || matches!(self.control, Some((_, Control::Login(_))))
    }

    /// The server has accepted this device: requests can carry its token (unlike while [`signed_in`](Self::signed_in)
    /// is only "signing in").
    pub(crate) fn has_token(&self) -> bool {
        !self.config.token.is_empty()
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
        // (a server that signs this device out while it waits to sign out leaves the library as it is: only
        // signing out with `force` ever removes what wasn't sent)
        self.config.leaving = false;
        self.signout = None;
        self.unsent = None;
        self.login = None;
        self.control = None;
        self.jobs.clear();
        self.ready.clear();
        self.plan.clear();
        self.planned = None;
        self.usage_task = None;
        self.activity_task = None;
        self.conflict = None;
        self.reading = None;
        self.transfer_error = None;
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

    /// Write `sync.docs` if it changed.
    pub(crate) fn save_docs_base(&mut self, files: &mut dyn Store) -> std::io::Result<()> {
        if self.docs_base_dirty {
            let s = serde_json::to_vec(&self.docs_base()).map_err(std::io::Error::other)?;
            files.write_atomic(DOCS, &s)?;
            self.docs_base_dirty = false;
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

    /// Photo file transfers queued or running: (uploads, downloads). (Every frame: the mini previews
    /// of a big library are counted, not walked.)
    fn transfers(&self) -> (usize, usize) {
        let small = || self.plan.iter().chain(self.ready.iter()).chain(self.jobs.values()).chain(self.urgent.iter());
        let downloads = small().filter(|j| matches!(j, Job::Get { .. })).count();
        (small().count() - downloads, downloads + self.minis.len() + self.minis_failed.len())
    }

    /// What `sync.status` reports.
    pub fn status(&self) -> Value {
        let (uploads, downloads) = self.transfers();
        json!({
            "state": self.state(),
            "signedIn": self.signed_in(),
            "server": self.config.server,
            "user": self.config.user,
            "device": self.config.device,
            "library": self.config.library,
            "cursor": self.config.cursor,
            "pending": self.outbox.len(),
            "reading": self.reading(),
            "uploads": uploads,
            "downloads": downloads,
            "uploadsDone": self.up_done,
            "downloadsDone": self.down_done,
            "conflict": self.conflict.as_ref().map(|c| &c.info),
            "error": self.error,
            "offlineAlbums": self.config.offline_albums,
            "offlinePhotos": self.config.offline_photos,
            "storeOriginals": self.config.store_originals,
            "originalsBudgetMb": self.config.originals_budget_mb,
            "merged": self.merged,
            "serverPreviews": self.config.server_builds_previews(),
            "allPreviews": self.config.all_previews,
            "paused": self.config.paused,
            // signing out: how many photos still had to be sent when last counted (`null` before the first count),
            // and the originals the server refused (they are not waited for: they can't be sent)
            "leaving": self.config.leaving,
            "unsent": self.config.leaving.then(|| self.unsent.map(|(_, n)| n)).flatten(),
            "refused": self.refused.len(),
        })
    }

    /// The state for the topbar's cloud icon: `signedOut`, `signingOut`, `conflict`, `paused`, `error`, `syncing` or
    /// `idle`.
    pub fn state(&self) -> &'static str {
        if !self.signed_in() {
            "signedOut"
        } else if self.config.leaving {
            "signingOut"
        } else if self.conflict.is_some() {
            "conflict"
        } else if self.config.paused {
            "paused"
        } else if self.error.is_some() {
            "error"
        } else if self.control.is_some() || self.reading().is_some() || !self.outbox.is_empty() || self.transfers() != (0, 0) || self.login.is_some()
        {
            "syncing"
        } else {
            "idle"
        }
    }

    /// While the server's library is read in pages: (photos read, photos it has).
    pub fn reading(&self) -> Option<(usize, usize)> {
        if let Some(f) = &self.config.filling {
            return Some((f.read, f.total));
        }
        let r = self.reading.as_ref()?;
        Some((r.catalog.len(), r.total.unwrap_or(0)))
    }

    /// The last error, if the last request failed.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// The choice waiting after signing in to a server that has a library already (see
    /// [`Session::sync_resolve`]).
    pub fn conflict(&self) -> Option<&ConflictInfo> {
        self.conflict.as_ref().map(|c| &c.info)
    }

    /// What the server says it is doing for this user (scanning folders, building previews…), as of
    /// [`SyncState::activity_age`] ago.
    pub fn activity(&self) -> Option<&proto::Activity> {
        self.activity.as_ref()
    }

    /// Why the server's last answer about what it is doing wasn't one.
    pub fn activity_error(&self) -> Option<&str> {
        self.activity_error.as_deref()
    }

    pub fn activity_age(&self) -> Option<Duration> {
        self.activity_at.map(|t| t.elapsed())
    }

    /// The answer to `GET /api/activity`.
    fn activity_done(&mut self, d: &Done) {
        self.activity_at = Some(Instant::now());
        match d.status {
            401 => self.signed_out("the server signed this device out: sign in again"),
            404 | 405 => {
                self.activity = None;
                self.activity_error = Some("this server is too old to say what it is doing: update it".into());
            }
            _ if d.ok() => match decode::<proto::Activity>(d) {
                Ok(a) => {
                    self.activity = Some(a);
                    self.activity_error = None;
                }
                Err(e) => self.activity_error = Some(e),
            },
            _ => self.activity_error = Some(why(d)),
        }
    }

    /// Photos uploaded and files downloaded since the queue was last empty, and what is still
    /// queued or running: ((done, left), (done, left)) for uploads and downloads.
    pub fn progress(&self) -> ((usize, usize), (usize, usize)) {
        let (uploads, downloads) = self.transfers();
        ((self.up_done, uploads), (self.down_done, downloads))
    }
}

/// The server's key for a photo's files: its content hash (without the suffix that keeps
/// converted copies apart), when it is a 128-bit hex hash.
pub fn blob_key(p: &Photo) -> Option<String> {
    lightcraft_catalog::sync::content_key(p)
}

/// Does the file at `path` hash to `key` (a [`blob_key`])? A file that can't be read doesn't.
fn file_has_hash(path: &str, key: &str) -> bool {
    use std::io::Read;
    let Ok(mut f) = std::fs::File::open(path) else { return false };
    let mut hasher = lightcraft_preview::Hasher128::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        match f.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                hasher.update(buf.get(..n).unwrap_or(&[]));
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return false,
        }
    }
    hasher.finish().to_string() == key
}

/// Is `key` (a [`blob_key`]) the content hash of `p`'s file? (No allocation: scans every photo.)
fn has_key(p: &Photo, key: &str) -> bool {
    p.content_hash.as_deref().and_then(|h| h.split(':').next()).is_some_and(|h| h.eq_ignore_ascii_case(key))
}

/// What this device is sending and receiving ([`Session::sync_transfers`]).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Transfers {
    /// Changes made here and not yet sent.
    pub pending: usize,
    /// Photos queued to upload (a photo is its original and its previews), and how many finished
    /// since nothing was queued.
    pub uploads: usize,
    pub uploads_done: usize,
    /// Files queued to download (previews, originals), and how many finished likewise.
    pub downloads: usize,
    pub downloads_done: usize,
    /// What is moving at this moment.
    pub running: Vec<Transfer>,
    /// Files whose transfer failed and waits to be tried again, and why the last one failed.
    pub failing: usize,
    pub error: Option<String>,
}

/// One file moving ([`Transfers::running`]).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Transfer {
    pub upload: bool,
    /// `original`, `smart preview`, `preview` or `building previews`.
    pub what: &'static str,
    /// The photo's file name (empty when it isn't in the library any more).
    pub name: String,
}

/// A synced photo's preview file names ([`Session::proxy_names`]), and which of them are here.
struct ProxyNames {
    smart: String,
    mini: String,
    has_smart: bool,
    has_mini: bool,
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

/// Does the answer to a preview's download say the server is making that preview (`previews: "queued"`)? A
/// server from before it did answers a plain `404`.
fn server_is_building(d: &Done) -> bool {
    serde_json::from_str::<Value>(&d.body).ok().is_some_and(|v| v["previews"].as_str() == Some("queued"))
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

    /// Sign this library in to a server (sent by the next [`Session::sync_tasks`]). An empty server
    /// library gets this library's photos; a library with no photos gets the server's. When both
    /// have photos the sign-in waits for a choice ([`SyncState::conflict`], [`Session::sync_resolve`]):
    /// add this library's photos to the server's, or use the server's instead.
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
        // (the library is still being emptied, or is waiting to be: another account's files would meet the old one's)
        if self.sync.as_ref().is_some_and(|st| st.config.leaving) || self.signing_out_rows() {
            return Err(EngineError::Other("this library is signing out: wait until it has, or cancel it in Settings › Sync".into()));
        }
        // (the last sign-out could not finish emptying the library: done now, or its photos would go up to this account)
        if self.sync.is_none() && self.wipe_pending() {
            self.reset_synced_library().map_err(|e| {
                EngineError::Other(format!("the library of the account that signed out is not removed from this device yet, and can't be: {e}"))
            })?;
        }
        let st = self.sync.get_or_insert_with(|| SyncState::new(SyncConfig::default()));
        if !st.config.library.is_empty() && (!st.config.server.eq_ignore_ascii_case(server) || st.config.user != user) {
            return Err(EngineError::Other(format!(
                "this library is a copy of {}'s library on {}: sign in there, or remove it from this device (Settings › Sync) to sign in \
                 as someone else, or use a new library for another server or user",
                st.config.user, st.config.server
            )));
        }
        st.config.server = server.to_string();
        st.config.user = user.to_string();
        st.login = Some(proto::Login { user: user.to_string(), password: password.to_string(), device: device.to_string() });
        st.conflict = None;
        st.error = None;
        st.retry_at = None;
        st.pull_at = None;
        self.persist()
    }

    /// Combine this library with the server's when the sign-in finds both have photos (instead of
    /// refusing): matched by what the photos are, nothing removed on either side. Call it after
    /// [`Session::sync_sign_in`]; it holds for that sign-in only.
    pub fn sync_merge_on_join(&mut self, on: bool) {
        if let Some(st) = self.sync.as_mut() {
            st.merge = on;
        }
    }

    /// Pull now (instead of at the next poll), and try again the photo files that failed (their
    /// transfers are planned afresh: clearing their waits alone would leave them where they were).
    pub fn sync_soon(&mut self) {
        if let Some(st) = self.sync.as_mut() {
            st.pull_at = None;
            st.retry_at = None;
            if !st.blob_retry.is_empty() {
                st.blob_retry.clear();
                st.plan_gen += 1;
            }
            let failed = std::mem::take(&mut st.minis_failed);
            st.minis.extend(failed.into_iter().map(|(_, job)| job));
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
        if on && st.config.leaving {
            return Err(EngineError::Other("this library is signing out: it can't be paused (cancel the sign out first)".into()));
        }
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

    /// Keep the originals downloaded from the server under `mb` megabytes, deleting the ones used longest ago
    /// (`None`: no limit).
    pub fn sync_originals_budget(&mut self, mb: Option<u64>) -> Result<()> {
        let st = self.sync.as_mut().ok_or_else(|| EngineError::Other("this library isn't synced".into()))?;
        st.config.originals_budget_mb = mb;
        st.evict_at = None;
        st.plan_gen += 1;
        self.persist()
    }

    /// Download the mini previews of these photos (synced ones whose original isn't here and that
    /// have no preview here) whatever the grid shows: for work that needs their pixels, like indexing
    /// them for search on this device. Returns how many are on their way.
    pub fn sync_want_minis(&mut self, ids: &[PhotoId]) -> usize {
        if self.sync.as_ref().is_some_and(|st| st.have.is_none()) {
            let have = self.read_have();
            if let Some(st) = self.sync.as_mut() {
                st.have = Some(have);
            }
        }
        let Some(st) = self.sync.as_ref() else { return 0 };
        let now = Instant::now();
        let jobs: Vec<Job> = ids.iter().filter_map(|id| self.catalog.photo(*id)).filter_map(|p| self.preview_get(st, p, Blob::Mini, now)).collect();
        let n = jobs.len();
        if let Some(st) = self.sync.as_mut() {
            st.minis.extend(jobs);
        }
        n
    }

    /// Download the mini previews of every photo (or only of the ones the grid shows).
    pub fn sync_all_previews(&mut self, on: bool) -> Result<()> {
        let st = self.sync.as_mut().ok_or_else(|| EngineError::Other("this library isn't synced".into()))?;
        st.config.all_previews = on;
        st.minis_gen += 1;
        if !on {
            st.minis.clear();
            st.minis_failed.clear();
        }
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

    /// Someone is looking at what the server is doing (the cloud popover): have it asked again (at
    /// most every [`ACTIVITY_EVERY`]) by the next [`Session::sync_tasks`].
    pub fn sync_want_activity(&mut self) {
        if let Some(st) = self.sync.as_mut() {
            st.activity_wanted = true;
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

    /// Ask the server what it is doing for this user now and wait for the answer (blocking: the CLI,
    /// tests), through `run`. Silent when signed out or paused: the last answer stays.
    pub fn sync_fetch_activity_with(&mut self, run: &mut dyn FnMut(&Task) -> Done) {
        let Some(st) = self.sync.as_mut().filter(|st| !st.config.paused && !st.config.token.is_empty() && st.activity_task.is_none()) else {
            return;
        };
        st.activity_wanted = false;
        let t = st.http("GET", "/api/activity", Body::Empty, None);
        st.activity_task = Some(t.id());
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
        self.media.thumb_tick = self.media.thumb_tick.wrapping_add(1);
        if self.interaction.is_none() {
            if let Some(page) = st.held_snapshot.take() {
                self.page_done(&mut st, page);
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
            tasks.extend(st.shares.ready.drain(..));
        }
        let waiting = st.retry_at.is_some_and(|t| now < t);
        // a slider drag holds a preview value in the catalog: nothing changes it meanwhile; the choice at
        // sign-in comes first (this library isn't the server's yet: nothing may be pulled or pushed)
        if st.control.is_none() && !waiting && self.interaction.is_none() && st.conflict.is_none() {
            if let Some(login) = st.login.take() {
                let body = Body::Json(serde_json::to_string(&login).unwrap_or_default());
                let t = st.http("POST", "/api/login", body, None);
                st.control = Some((t.id(), Control::Login(login)));
                tasks.push(t);
            } else if !st.config.token.is_empty() {
                if st.needs_snapshot {
                    let next = match &st.config.filling {
                        Some(f) => f.next.as_deref(),
                        None => st.reading.as_ref().and_then(|r| r.next.as_deref()),
                    };
                    let path = match next {
                        Some(next) => format!("/api/snapshot?limit={SNAPSHOT_PAGE}&compact=1&cursor={next}"),
                        None => format!("/api/snapshot?limit={SNAPSHOT_PAGE}&compact=1"),
                    };
                    let t = st.http("GET", &path, Body::Empty, None);
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
                    let presets = self.presets_to_send(&st);
                    let body = serde_json::to_string(&proto::Presets { version: st.config.presets_version, presets: Value::Array(presets.clone()) })
                        .unwrap_or_default();
                    let t = st.http("PUT", "/api/presets", Body::Json(body), None);
                    st.control = Some((t.id(), Control::PutPresets(presets)));
                    tasks.push(t);
                } else if !st.config.library.is_empty()
                    && let Some((control, method, path, body)) = self.doc_request(&mut st)
                {
                    let t = st.http(method, &path, body, None);
                    st.control = Some((t.id(), control));
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
        if st.activity_wanted
            && st.activity_task.is_none()
            && !st.config.token.is_empty()
            && !waiting
            && st.activity_at.is_none_or(|t| t.elapsed() >= ACTIVITY_EVERY)
        {
            st.activity_wanted = false;
            let t = st.http("GET", "/api/activity", Body::Empty, None);
            st.activity_task = Some(t.id());
            tasks.push(t);
        }
        if !st.config.token.is_empty() && !st.config.library.is_empty() && (!st.needs_snapshot || st.config.filling.is_some()) && !waiting {
            self.blob_tasks(&mut st, now, &mut tasks);
        }
        // nothing queued or running: the next batch of transfers counts from zero
        if st.transfers() == (0, 0) {
            st.up_done = 0;
            st.down_done = 0;
        }
        // signing out: once everything has been sent, the device signs out on the server and the library is emptied
        if st.config.leaving && self.leave_step(&mut st, now) {
            return self.finish_leaving(st, tasks);
        }
        self.sync = Some(st);
        tasks
    }

    /// Plan photo file transfers (when the library or what to keep changed) and start some: the
    /// active photo's smart preview and the mini previews the grid asks for first.
    fn blob_tasks(&mut self, st: &mut SyncState, now: Instant, tasks: &mut Vec<Task>) {
        self.touch_active_original(st);
        if st.config.originals_budget_mb.is_some() && st.evict_at.is_none_or(|t| now >= t) {
            self.evict_originals(st);
            st.evict_at = Some(now + EVICT_EVERY);
        }
        if self.media.smart_dir.is_none() && self.sync_store.is_none() {
            return;
        }
        if st.have.is_none() {
            st.have = Some(self.read_have());
        }
        // a transfer that failed is tried again once its wait is over
        if st.blob_retry.values().any(|t| now >= *t) {
            st.blob_retry.retain(|_, t| now < *t);
            st.plan_gen += 1;
        }
        // (the grid asks for these again on its own: no plan to make)
        st.building.retain(|_, t| now < *t);
        if st.minis_failed.iter().any(|(t, _)| now >= *t) {
            let (waited, waiting) = std::mem::take(&mut st.minis_failed).into_iter().partition::<Vec<_>, _>(|(t, _)| now >= *t);
            st.minis_failed = waiting;
            st.minis.extend(waited.into_iter().map(|(_, job)| job));
        }
        // (a slider drag changes the catalog every frame, and nothing a plan is made of)
        let key = (self.catalog.revision, st.plan_gen);
        if st.planned != Some(key) && (self.interaction.is_none() || st.planned.is_none()) {
            st.plan = self.plan_blobs(st, now).into();
            st.planned = Some(key);
        }
        let key = (self.catalog.len(), st.minis_gen);
        if st.config.all_previews && st.minis_planned != Some(key) && self.interaction.is_none() {
            st.minis = self.plan_minis(st, now).into();
            st.minis_planned = Some(key);
        }
        let mut urgent = self.urgent_jobs(st, now).into_iter();
        while st.jobs.len() < BLOB_PARALLEL {
            let Some(mut job) = st.ready.pop_front().or_else(|| urgent.next()).or_else(|| st.plan.pop_front()).or_else(|| st.minis.pop_front())
            else {
                break;
            };
            // (signing out sends what is left and downloads nothing: the files would go with the library)
            if st.config.leaving && matches!(job, Job::Get { .. }) {
                continue;
            }
            let target = job.target();
            if st.jobs.values().any(|j| j.target() == target) {
                continue;
            }
            // (a preview of the lists that came meanwhile, asked for by the grid or the loupe)
            if let Job::Get { blob: Blob::Mini | Blob::Smart, dest, .. } = &job
                && let (Some(have), Some(name)) = (&st.have, Path::new(dest).file_name())
                && have.contains(&*name.to_string_lossy())
            {
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
                    job = Job::Put { key: key.clone(), blob: Blob::Smart, path: smart, id: *id, offset: 0 };
                }
            }
            let task = match &job {
                Job::Head { key, .. } => st.http("HEAD", &format!("/api/blobs/original/{key}"), Body::Empty, None),
                Job::Put { key, blob, path, offset, .. } => {
                    // (the server builds the previews of an original it is sent this way; one that doesn't know the
                    // parameter ignores it and the device builds them, see `job_done`)
                    let ask = if *blob == Blob::Original && st.config.server_builds_previews() { "?previews=1" } else { "" };
                    let body = if *offset > 0 { Body::FileFrom(path.clone(), *offset) } else { Body::File(path.clone()) };
                    st.http("PUT", &format!("/api/blobs/{}/{key}{ask}", blob.name()), body, None)
                }
                Job::Get { key, blob, dest, .. } => st.http("GET", &format!("/api/blobs/{}/{key}", blob.name()), Body::Empty, Some(dest.clone())),
                Job::Proxies { path, id, .. } => {
                    let Some((smart, mini)) = self.proxy_paths(*id) else { continue };
                    Task::Proxies { id: NEXT_TASK.fetch_add(1, Ordering::Relaxed), original: path.clone(), smart, mini }
                }
            };
            st.jobs.insert(task.id(), job);
            tasks.push(task);
        }
        // (what waits, for the counts shown; asked for again next step)
        st.urgent = urgent.collect();
    }

    /// The preview files in the previews folder, or in the host's storage.
    fn read_have(&self) -> HashSet<String> {
        match (&self.media.smart_dir, &self.sync_store) {
            (Some(d), _) => read_names(d),
            (None, Some(s)) => s.proxies.clone(),
            (None, None) => HashSet::new(),
        }
    }

    /// How much longer the server's building of `id`'s preview is waited for (tests).
    #[cfg(test)]
    pub(crate) fn sync_building_wait(&self, id: PhotoId) -> Option<Duration> {
        let key = blob_key(self.catalog.photo(id)?)?;
        self.sync.as_ref()?.building.get(&key)?.checked_duration_since(Instant::now())
    }

    /// The previews the server was building are asked for again (tests: instead of waiting seconds).
    #[cfg(test)]
    pub(crate) fn sync_end_building_waits(&mut self) {
        if let Some(st) = self.sync.as_mut() {
            st.building.clear();
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

    /// Where a preview file named `name` ([`Session::proxy_names`]) goes: the previews folder, or the
    /// host's storage.
    fn proxy_dest(&self, name: &str) -> String {
        match (&self.media.smart_dir, &self.sync_store) {
            (Some(dir), _) => dir.join(name).to_string_lossy().to_string(),
            (None, Some(s)) => format!("{}{name}", s.prefix),
            (None, None) => name.to_string(),
        }
    }

    /// The file names [`Session::proxy_paths`] ends in, and which of them are here.
    fn proxy_names(&self, p: &Photo, key: &str, have: &HashSet<String>) -> ProxyNames {
        let (smart, mini) = match &self.sync_store {
            Some(_) if self.media.smart_dir.is_none() => store_proxy_names(key),
            _ => (crate::smart::file_name(p), mini_file_name(p)),
        };
        ProxyNames { has_smart: have.contains(&smart), has_mini: have.contains(&mini), smart, mini }
    }

    /// The server's key for a synced photo whose original isn't on this device (`None`: any other photo).
    fn remote_key(&self, p: &Photo) -> Option<String> {
        if !is_remote(p) {
            return None;
        }
        let key = blob_key(p)?;
        let in_store = self.media.smart_dir.is_none() && self.sync_store.as_ref().is_some_and(|s| s.originals.contains(&key));
        (!in_store).then_some(key)
    }

    /// Which previews a synced photo whose original isn't on this device has here: 0 none, 1 the
    /// mini, 2 the smart preview (`None`: not such a photo). Part of render keys, so a view rendered
    /// from the mini preview renders again once the smart one arrives.
    pub fn synced_tier(&self, p: &Photo) -> Option<u8> {
        let st = self.sync.as_ref()?;
        let key = self.remote_key(p)?;
        let Some(have) = &st.have else { return Some(0) };
        let n = self.proxy_names(p, &key, have);
        Some(if n.has_smart { 2 } else { u8::from(n.has_mini) })
    }

    /// The download of `p`'s preview `blob` (a smart or mini one) when `p` is a synced photo whose
    /// original isn't here, that preview isn't here (for a mini: nor the smart one), no failure of
    /// its files is being waited out and the server isn't building it right now.
    fn preview_get(&self, st: &SyncState, p: &Photo, blob: Blob, now: Instant) -> Option<Job> {
        let key = self.remote_key(p)?;
        if st.blob_retry.get(&key).is_some_and(|t| now < *t) || st.building.get(&key).is_some_and(|t| now < *t) {
            return None;
        }
        let n = self.proxy_names(p, &key, st.have.as_ref()?);
        let name = match blob {
            Blob::Smart if !n.has_smart => n.smart,
            Blob::Mini if !n.has_smart && !n.has_mini => n.mini,
            _ => return None,
        };
        Some(Job::Get { dest: self.proxy_dest(&name), key, blob, id: p.id })
    }

    /// The active photo's smart preview, then the mini previews of the cells the grid asked for in
    /// its last [`ASK_TICKS`] steps — those on screen before those only in its reach, the most recent
    /// ask first (within a step, in the order the cells were drawn): the ones not here and not on their
    /// way. Cells scrolled out of reach are forgotten.
    fn urgent_jobs(&mut self, st: &SyncState, now: Instant) -> Vec<Job> {
        let tick = self.media.thumb_tick;
        self.media.thumb_asks.retain(|_, a| tick.saturating_sub(a.tick) <= ASK_TICKS);
        let mut asked: Vec<(bool, std::cmp::Reverse<u64>, u64, PhotoId)> =
            self.media.thumb_asks.iter().map(|(id, a)| (!a.shown, std::cmp::Reverse(a.tick), a.seq, *id)).collect();
        asked.sort_unstable();
        let running: HashSet<String> = st.jobs.values().map(Job::target).collect();
        let active = self.active().map(|id| (id, Blob::Smart));
        active
            .into_iter()
            .chain(asked.into_iter().map(|(_, _, _, id)| (id, Blob::Mini)))
            .filter_map(|(id, blob)| self.preview_get(st, self.catalog.photo(id)?, blob, now))
            .filter(|job| !running.contains(&job.target()))
            .collect()
    }

    /// Where downloaded originals are kept.
    pub(crate) fn originals_dir(&self) -> Option<PathBuf> {
        self.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.join("sync").join("originals"))
    }

    /// Photo file transfers this device needs besides the previews on screen: uploads of the
    /// originals it has that the server doesn't, the smart previews of what's available offline and
    /// wanted originals. Made again whenever the library changes, so a synced photo that needs
    /// none of these costs no allocation.
    fn plan_blobs(&self, st: &SyncState, now: Instant) -> Vec<Job> {
        let retry_ok = |key: &str| st.blob_retry.get(key).is_none_or(|t| now >= *t);
        let mut offline: HashSet<PhotoId> = st.config.offline_photos.iter().copied().collect();
        for a in &st.config.offline_albums {
            offline.extend(self.catalog.album_photos(*a));
        }
        let originals_dir = self.originals_dir();
        let store = self.sync_store.as_ref().filter(|_| self.media.smart_dir.is_none());
        let originals = st.config.store_originals || !st.want_originals.is_empty();
        let (mut ups, mut smart, mut downloads) = (Vec::new(), Vec::new(), Vec::new());
        for p in self.catalog.photos().filter(|p| !p.local && p.copy_of.is_none()) {
            let Source::File { path } = &p.source else { continue };
            let remote = path.starts_with(PATH_PREFIX);
            let pinned = remote && offline.contains(&p.id);
            if remote && store.is_none() && !pinned && !originals {
                continue;
            }
            let Some(key) = blob_key(p) else { continue };
            // this device has the file: on its disk, or in the host's storage
            let here = match (remote, store.is_some_and(|s| s.originals.contains(&key))) {
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
            if pinned && let Some(job) = self.preview_get(st, p, Blob::Smart, now) {
                smart.push(job);
            }
            if (st.config.store_originals || st.want_originals.contains(&key)) && retry_ok(&key) {
                let dest = match (&originals_dir, store) {
                    (Some(odir), _) => Some(odir.join(&key).join(file_name_of(path, &p.file_name)).to_string_lossy().to_string()),
                    (None, Some(_)) => Some(BrowserStore::original_key(&key)),
                    (None, None) => None,
                };
                if let Some(dest) = dest {
                    downloads.push(Job::Get { key, blob: Blob::Original, dest, id: p.id });
                }
            }
        }
        ups.into_iter().chain(smart).chain(downloads).collect()
    }

    /// With [`SyncConfig::all_previews`]: the mini previews of every synced photo whose original
    /// isn't here and that has no preview here yet (one pass over the library: a while in a big one).
    fn plan_minis(&self, st: &SyncState, now: Instant) -> Vec<Job> {
        self.catalog.photos().filter(|p| p.copy_of.is_none()).filter_map(|p| self.preview_get(st, p, Blob::Mini, now)).collect()
    }

    /// Mini previews this device doesn't have yet (of synced photos whose original isn't here), and
    /// about how many bytes downloading them all would take, at what the server's minis weigh on
    /// average once it said so (`sync.usage`). Counted again when photos come or go (a while in a big
    /// library); `None` while the server's library is still being read.
    pub fn sync_previews_left(&mut self) -> Option<(usize, u64)> {
        let st = self.sync.as_ref().filter(|st| st.reading().is_none())?;
        let n = match st.minis_left {
            Some((photos, n)) if photos == self.catalog.len() => n,
            _ => {
                let none = HashSet::new();
                let have = st.have.as_ref().unwrap_or(&none);
                let missing = |p: &Photo| {
                    self.remote_key(p).is_some_and(|key| {
                        let n = self.proxy_names(p, &key, have);
                        !n.has_smart && !n.has_mini
                    })
                };
                self.catalog.photos().filter(|p| p.copy_of.is_none() && missing(p)).count()
            }
        };
        let each = st.usage.as_ref().map(|u| u.mini).filter(|m| m.files > 0).map_or(MINI_BYTES_GUESS, |m| m.bytes / m.files);
        let photos = self.catalog.len();
        if let Some(st) = self.sync.as_mut() {
            st.minis_left = Some((photos, n));
        }
        Some((n, (n as u64).saturating_mul(each)))
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
        } else if let Some(kind) = st.shares.take_kind(done.id) {
            self.share_done(&mut st, kind, &done);
        } else if st.usage_task == Some(done.id) {
            st.usage_task = None;
            st.usage_done(&done);
        } else if st.activity_task == Some(done.id) {
            st.activity_task = None;
            st.activity_done(&done);
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
            Control::Snapshot => {
                match (d.ok(), proto::Snapshot::from_json(&d.body).map_err(|e| format!("unexpected answer from the server ({e})"))) {
                    (true, Ok(page)) if self.interaction.is_some() => st.held_snapshot = Some(page),
                    (true, Ok(page)) => self.page_done(st, page),
                    (true, Err(e)) => st.fail(e),
                    // the copy being read is gone: from the start
                    (false, _) if d.status == 410 => {
                        st.reading = None;
                        if let Some(f) = st.config.filling.as_mut() {
                            f.next = None;
                        }
                    }
                    (false, _) => st.fail(why(d)),
                }
            }
            Control::Pull => match d.status {
                410 => st.needs_snapshot = true,
                _ if d.ok() => match decode::<proto::Ops>(d) {
                    Ok(ops) => {
                        let more = ops.ops.len() >= PULL_LIMIT;
                        st.presets_remote = ops.presets;
                        st.note_docs(&ops.docs);
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
            Control::GetDoc(name) => self.doc_got(st, &name, d),
            Control::PutDoc(name, sent) => self.doc_put(st, &name, sent, d),
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

    /// The ids of the built-in presets, and of this device's user presets.
    fn preset_ids(&self) -> (HashSet<String>, HashSet<String>) {
        let (builtin, user): (Vec<_>, Vec<_>) = self.presets.iter().partition(|p| p.builtin);
        (builtin.into_iter().map(|p| p.id.clone()).collect(), user.into_iter().map(|p| p.id.clone()).collect())
    }

    /// The presets document this device sends: its user presets, then what it left out of the
    /// server's as last synced ([`presets_left_out`]), as it came. Leaving a preset out (past
    /// [`PRESETS_MAX`], not readable here) then never deletes it for the user's other devices.
    fn presets_to_send(&self, st: &SyncState) -> Vec<Value> {
        let mut out = self.user_presets();
        let (builtin, own) = self.preset_ids();
        out.extend(presets_left_out(&st.presets_base, &builtin, &own, PRESETS_MAX));
        out
    }

    /// Did the user presets change since they were last synced? (Compared only after presets.json
    /// was written, which happens when they change.)
    fn presets_changed(&self, st: &mut SyncState) -> bool {
        let generation = self.library.as_ref().map(|l| l.presets_gen());
        if st.presets_seen != generation {
            st.presets_seen = generation;
            st.presets_changed = self.presets_to_send(st) != st.presets_base;
        }
        st.presets_changed
    }

    /// The server's presets document (`theirs`) merged into this device's.
    fn take_presets(&mut self, st: &mut SyncState, theirs: proto::Presets) {
        let remote: Vec<Value> = theirs.presets.as_array().cloned().unwrap_or_default();
        let merged = merge_presets(&st.presets_base, &self.user_presets(), &remote);
        let (builtin, own) = self.preset_ids();
        let users = hold_presets(&merged, &builtin, &own, PRESETS_MAX);
        self.presets.retain(|p| p.builtin);
        self.presets.extend(users);
        // (the whole document: what wasn't held is sent back from it, `presets_to_send`)
        st.presets_base = remote;
        st.presets_base_dirty = true;
        st.config.presets_version = theirs.version;
        st.presets_remote = st.presets_remote.max(theirs.version);
        st.presets_seen = None;
    }

    /// A page of the server's library (or all of it, from a server that doesn't page): into the
    /// library being filled in, or added to the pages read so far and, after the last, adopted.
    fn page_done(&mut self, st: &mut SyncState, page: proto::Snapshot) {
        if let Some(f) = st.config.filling.clone() {
            if page.library != st.config.library {
                // (the server's library was replaced meanwhile: that is said when it is adopted)
                st.config.filling = None;
            } else if f.next.is_none() {
                return self.start_filling(st, page, true);
            } else if page.seq != f.seq {
                if let Some(f) = st.config.filling.as_mut() {
                    f.next = None;
                }
                return st.fail("the server's library changed while it was read: reading it again".into());
            } else {
                return self.fill(st, page);
            }
        }
        let first = st.reading.is_none();
        let snap = match st.reading.take() {
            None => page,
            Some(mut read) => {
                if !read.add_page(page) {
                    return st.fail("the server's library changed while it was read: reading it again".into());
                }
                read
            }
        };
        // joining with nothing of its own: the server's library replaces this one, so it shows as it comes
        if first && snap.next.is_some() && st.config.library.is_empty() && !snap.library.is_empty() && !self.has_own_photos() {
            return self.start_filling(st, snap, false);
        }
        if snap.next.is_some() {
            // (the next page on the next round)
            st.reading = Some(snap);
            st.succeeded();
        } else {
            self.snapshot_done(st, snap);
        }
    }

    /// The first page of the server's library becomes this one, the rest goes in as it comes
    /// ([`Filling`]). `again`: a read that started over (its copy was gone).
    fn start_filling(&mut self, st: &mut SyncState, page: proto::Snapshot, again: bool) {
        let mut c = page.catalog;
        lightcraft_catalog::sync::sanitize(&mut c);
        c.set_id_space(st.config.space);
        let restarted = again || st.config.filling.as_ref().is_some_and(|f| f.restarted);
        st.config.filling = Some(Filling { seq: page.seq, next: page.next, read: c.len(), total: page.total.unwrap_or(0), restarted });
        st.config.library = page.library;
        st.planned = None;
        self.forget_selection();
        self.replace_catalog(c);
        self.filled_if_done(st);
    }

    /// The next page of a library being filled in: its photos are added (logged like pulled changes).
    fn fill(&mut self, st: &mut SyncState, page: proto::Snapshot) {
        for p in page.catalog.photos() {
            let op = Op::AddPhoto { photo: Box::new(Photo::clone(p)) };
            match self.catalog.apply(op.clone()) {
                Ok(_) => self.pending_log.push(op),
                Err(e) => log::warn!("sync: photo {} of the server's library doesn't go in here ({e})", p.id.0),
            }
        }
        if let Some(f) = st.config.filling.as_mut() {
            f.read = f.read.saturating_add(page.catalog.len());
            f.next = page.next;
        }
        self.filled_if_done(st);
    }

    /// After the last page: this library is at the copy's op, and pulls what changed since.
    fn filled_if_done(&mut self, st: &mut SyncState) {
        let Some(f) = st.config.filling.take_if(|f| f.next.is_none()) else {
            return st.succeeded();
        };
        if f.restarted {
            // what was changed here while it was read, made again on what was read the second time
            let mut c = self.catalog.clone();
            let dropped = st.outbox.rebase(&mut c);
            st.outbox_changed();
            if dropped > 0 {
                log::info!("sync: {dropped} change(s) made here no longer apply to the server's library");
            }
            self.replace_catalog(c);
        }
        st.config.cursor = f.seq;
        st.needs_snapshot = false;
        st.planned = None;
        st.succeeded();
    }

    /// Photos the user put in this library (not the demo photos a new one starts with, nor Local
    /// records), or albums that hold some.
    fn has_own_photos(&self) -> bool {
        let user_photo = |p: &Photo| !p.local && !matches!(p.source, Source::Demo { .. });
        self.catalog.photos().any(|p| user_photo(p))
            || self.catalog.albums().any(|a| a.smart.is_some() || a.photos.iter().any(|id| self.catalog.photo(*id).is_some_and(|p| user_photo(p))))
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
            let has_photos = self.has_own_photos();
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
                if st.merge {
                    return self.join_by_merging(st, snap);
                }
                // two libraries that never met: the user chooses ([`Session::sync_resolve`]); nothing is
                // pulled or pushed meanwhile
                let info = ConflictInfo {
                    server: st.config.server.trim_start_matches("https://").trim_start_matches("http://").trim_end_matches('/').to_string(),
                    user: st.config.user.clone(),
                    here: join::counts(&self.catalog),
                    there: join::counts(&snap.catalog),
                    shared: join::shared_photos(&self.catalog, &snap.catalog),
                };
                st.conflict = Some(Conflict { snapshot: snap, info });
                return Ok(());
            }
            st.config.library = snap.library;
        }
        let mut c: Catalog = snap.catalog;
        lightcraft_catalog::sync::sanitize(&mut c);
        if joining {
            // nothing of the (empty or demo-only) library joining carries over by id
            self.forget_selection();
        } else {
            carry_local(&self.catalog, &mut c);
        }
        c.set_id_space(st.config.space);
        let dropped = st.outbox.rebase(&mut c);
        st.outbox_changed();
        if dropped > 0 {
            log::info!("sync: {dropped} change(s) made here no longer apply to the server's library");
        }
        if !joining {
            // photos added here and not pushed yet came back with the replay in their shared form: they are
            // still files on this disk (a library joined to the server's has a queue of them)
            carry_local(&self.catalog, &mut c);
        }
        st.config.cursor = snap.seq;
        st.planned = None;
        self.replace_catalog(c);
        Ok(())
    }

    /// This library has photos and the server already has a library: without the person's say-so say what a merge
    /// would do; with it, merge. The result is the server's library with this one's changes made on top, so they
    /// reach the server (and every other device) like any edit; what belongs to this device alone — where its
    /// files are, History, Local records — is carried over under the new ids.
    fn join_by_merging(&mut self, st: &mut SyncState, snap: proto::Snapshot) -> std::result::Result<(), String> {
        let merged = lightcraft_catalog::merge::merge(&self.catalog, snap.catalog, st.config.space);
        let r = merged.report.clone();
        st.merge = false;
        let mut c = merged.catalog;
        lightcraft_catalog::merge::carry_local_mapped(&self.catalog, &mut c, &merged.photos);
        lightcraft_catalog::sync::sanitize(&mut c);
        st.config.library = snap.library;
        st.config.cursor = snap.seq;
        st.outbox = merged.outbox;
        st.outbox_changed();
        // what this device keeps offline is named by the new ids
        st.config.offline_photos = st.config.offline_photos.iter().filter_map(|id| merged.photos.get(id).copied()).collect();
        st.config.offline_albums = st.config.offline_albums.iter().filter_map(|id| merged.albums.get(id).copied()).collect();
        st.planned = None;
        st.merged = Some(r);
        // views and selections named the old ids
        self.selection = crate::Selection::default();
        self.previous_active = None;
        self.before.clear();
        self.source = crate::LibrarySource::All;
        self.filter = Default::default();
        self.replace_catalog(c);
        Ok(())
    }

    /// The photos selected here (and what points at them) belong to a library that was replaced.
    fn forget_selection(&mut self) {
        self.selection = crate::Selection::default();
        self.previous_active = None;
        self.before.clear();
    }

    /// The choice waiting after signing in to a server that has a library already, when there is one
    /// ([`Session::sync_resolve`]).
    pub fn sync_conflict(&self) -> Option<&ConflictInfo> {
        self.sync.as_ref()?.conflict()
    }

    /// Settle the choice waiting after signing in to a server that has a library already:
    ///
    /// - [`Resolution::Upload`]: this library's photos, albums, stacks and edits are added to the
    ///   server's library — photos the server has too (the same file) are kept once, see
    ///   [`lightcraft_catalog::join`] — and the server's photos come here. This library's originals
    ///   upload in the background, as after any import.
    /// - [`Resolution::UseServer`]: this library becomes the server's. Photo files on this computer
    ///   are not touched; they are just no longer in the library.
    /// - [`Resolution::Cancel`]: sign out; this library stays as it is.
    ///
    /// Before either of the first two changes anything, a copy of this library's catalog is kept in
    /// the library folder (`catalog-before-sync-<time>.snap`, in the format of `catalog.snap`); if
    /// that can't be written nothing changes and the choice stays open.
    pub fn sync_resolve(&mut self, how: Resolution) -> Result<Value> {
        let Some(mut st) = self.sync.take() else {
            return Err(EngineError::Other("this library isn't synced".into()));
        };
        let result = self.resolve_conflict(&mut st, how);
        self.sync = Some(st);
        let v = result?;
        self.persist()?;
        Ok(v)
    }

    fn resolve_conflict(&mut self, st: &mut SyncState, how: Resolution) -> Result<Value> {
        let Some(conflict) = st.conflict.take() else {
            return Err(EngineError::Other("this library isn't waiting for a choice".into()));
        };
        if how == Resolution::Cancel {
            st.signed_out("signed out");
            st.error = None;
            return Ok(json!({"choice": how.id()}));
        }
        let backup = match self.keep_library_copy("catalog-before-sync") {
            Ok(name) => name,
            Err(e) => {
                st.conflict = Some(conflict);
                return Err(e);
            }
        };
        let Conflict { snapshot, .. } = conflict;
        let mut c = snapshot.catalog;
        lightcraft_catalog::sync::sanitize(&mut c);
        let joined = match how {
            Resolution::Upload => {
                let joined = join::join_libraries(&self.catalog, &mut c, st.config.space, &mut st.outbox);
                st.outbox_changed();
                Some(joined)
            }
            _ => {
                c.set_id_space(st.config.space);
                None
            }
        };
        st.config.library = snapshot.library;
        st.config.cursor = snapshot.seq;
        st.planned = None;
        // the photos here have new ids (or are gone): what pointed at the old ones doesn't
        self.forget_selection();
        self.source = crate::LibrarySource::All;
        self.filter = Default::default();
        self.replace_catalog(c);
        Ok(json!({"choice": how.id(), "backup": backup, "joined": joined}))
    }

    /// A copy of this library's catalog, kept beside it before the server's library joins or replaces
    /// it (`prefix`: `catalog-before-sync`) or before it is removed on signing out (`catalog-before-signout`). Its
    /// file name.
    fn keep_library_copy(&mut self, prefix: &str) -> Result<String> {
        let stamp: String = (self.clock)().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
        let lib = self.library.as_mut().ok_or_else(|| EngineError::Other("sync needs a library that is saved (open or create one first)".into()))?;
        // (an earlier copy is never overwritten: the same second, or a host whose clock stands still)
        let mut name = format!("{prefix}-{stamp}.snap");
        for n in 2..100 {
            if lib.files_mut().read(&name).ok().flatten().is_none() {
                break;
            }
            name = format!("{prefix}-{stamp}-{n}.snap");
        }
        lightcraft_catalog::journal::write_backup(lib.files_mut(), &name, &self.catalog)
            .map_err(|e| EngineError::Other(format!("can't keep a copy of this library first ({e}): nothing was changed")))?;
        Ok(name)
    }

    /// What this device is sending and receiving now, for the cloud popover: the changes waiting to
    /// go, how many photos and files are queued (and done since the queue was last empty), and the
    /// few that are moving at this moment.
    pub fn sync_transfers(&self) -> Transfers {
        let Some(st) = &self.sync else { return Transfers::default() };
        let ((uploads_done, uploads), (downloads_done, downloads)) = st.progress();
        let file_name = |id: PhotoId| self.catalog.photo(id).map(|p| p.file_name.clone());
        let mut running: Vec<Transfer> = st
            .jobs
            .values()
            .map(|job| match job {
                Job::Head { id, .. } | Job::Put { blob: Blob::Original, id, .. } => {
                    Transfer { upload: true, what: "original", name: file_name(*id).unwrap_or_default() }
                }
                Job::Put { blob, id, .. } => Transfer {
                    upload: true,
                    what: if *blob == Blob::Smart { "smart preview" } else { "preview" },
                    name: file_name(*id).unwrap_or_default(),
                },
                Job::Proxies { id, .. } => Transfer { upload: true, what: "building previews", name: file_name(*id).unwrap_or_default() },
                Job::Get { blob, id, .. } => Transfer {
                    upload: false,
                    what: match blob {
                        Blob::Original => "original",
                        Blob::Smart => "smart preview",
                        Blob::Mini => "preview",
                    },
                    name: file_name(*id).unwrap_or_default(),
                },
            })
            .collect();
        // (a stable order: the jobs are kept in a map)
        running.sort_by(|a, b| (!a.upload, &a.name, a.what).cmp(&(!b.upload, &b.name, b.what)));
        Transfers {
            pending: st.outbox.len(),
            uploads,
            uploads_done,
            downloads,
            downloads_done,
            running,
            failing: st.blob_retry.len(),
            error: st.transfer_error.clone(),
        }
    }

    /// Switch to another state of the library wholesale (a snapshot replaces the op log).
    fn replace_catalog(&mut self, c: Catalog) {
        if let Err(e) = self.replace_catalog_saved(c) {
            log::error!("sync: {e}");
        }
    }

    /// [`Session::replace_catalog`], saying when the new state could not be written (the old one is then still what
    /// is on disk).
    fn replace_catalog_saved(&mut self, mut c: Catalog) -> Result<()> {
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
                Err(e) => return Err(EngineError::Other(format!("saving the library: {e}"))),
            }
        }
        Ok(())
    }

    fn job_done(&mut self, st: &mut SyncState, job: Job, d: &Done) {
        // (planned again once the wait is over, see `blob_tasks`: not now, for a server that hasn't built
        // the previews of thousands of photos yet answers each of them at once)
        let retry = |st: &mut SyncState, key: &str, why: String| {
            log::warn!("sync: {key}: {why}");
            st.transfer_error = Some(why);
            st.blob_retry.insert(key.to_string(), Instant::now() + BLOB_RETRY);
        };
        if d.status == 401 {
            st.signed_out("the server signed this device out: sign in again");
            return;
        }
        match job {
            Job::Head { key, path, id } => match d.status {
                200 => st.ready.push_front(Job::Proxies { key, path, id }),
                // (a `HEAD` has no body: the host puts the `Upload-Offset` header there, the bytes of an
                // earlier, broken upload the server kept)
                404 => st.ready.push_front(Job::Put { key, blob: Blob::Original, path, id, offset: d.body.trim().parse().unwrap_or(0) }),
                _ => retry(st, &key, why(d)),
            },
            Job::Put { key, blob, path, id, .. } if d.ok() => match blob {
                Blob::Original => {
                    // the server answers `{"previews": "queued" | "built"}` when it is making them: nothing to build here
                    let by_server = st.config.server_builds_previews()
                        && serde_json::from_str::<Value>(&d.body).ok().is_some_and(|v| matches!(v["previews"].as_str(), Some("queued" | "built")));
                    if by_server {
                        self.upload_finished(st, key);
                    } else {
                        st.ready.push_front(Job::Proxies { key, path, id });
                    }
                }
                Blob::Smart => match self.proxy_paths(id) {
                    Some((_, mini)) => st.ready.push_front(Job::Put { key, blob: Blob::Mini, path: mini, id, offset: 0 }),
                    None => self.upload_finished(st, key),
                },
                Blob::Mini => self.upload_finished(st, key),
            },
            Job::Put { key, blob: Blob::Original, .. } if d.status == 422 => {
                log::warn!("sync: the server refused the original {key}: {}", why(d));
                st.refused.insert(key);
                st.unsent = None;
            }
            // the server has another number of bytes than the device thought (or is taking this file
            // from another request): go on from where it says, unless that is where this tried
            Job::Put { key, blob: Blob::Original, path, id, offset } if d.status == 409 => {
                match serde_json::from_str::<Value>(&d.body).ok().and_then(|v| v["offset"].as_u64()) {
                    Some(at) if at != offset => st.ready.push_front(Job::Put { key, blob: Blob::Original, path, id, offset: at }),
                    _ => retry(st, &key, why(d)),
                }
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
                    st.ready.push_front(Job::Put { key, blob: Blob::Smart, path: smart, id, offset: 0 });
                }
                (_, None) => self.upload_finished(st, key),
                (false, _) => {
                    // an original that doesn't decode here: the server keeps it, others try
                    log::warn!("sync: can't build previews of {key}: {}", d.body);
                    self.upload_finished(st, key);
                }
            },
            Job::Get { key, blob, dest, id } => {
                // a preview the server hasn't built yet and now builds first: not a failure, asked for again soon
                // (the bulk of "download all previews" walks on, and comes back to it in its own time)
                if d.status == 404 && blob != Blob::Original && server_is_building(d) {
                    st.building.insert(key.clone(), Instant::now() + BUILDING_RETRY);
                    if blob == Blob::Mini && st.config.all_previews {
                        st.minis_failed.push((Instant::now() + BLOB_RETRY, Job::Get { key, blob, dest, id }));
                    }
                    return;
                }
                if !d.ok() {
                    retry(st, &key, why(d));
                    if blob == Blob::Mini && st.config.all_previews {
                        st.minis_failed.push((Instant::now() + BLOB_RETRY, Job::Get { key, blob, dest, id }));
                    }
                    return;
                }
                st.down_done += 1;
                st.transfer_error = None;
                self.media.availability.forget(&dest);
                // (a mini preview replaces nothing decoded: the photo had no preview here)
                let ids: Vec<PhotoId> =
                    if blob == Blob::Mini { vec![id] } else { self.catalog.photos().filter(|p| has_key(p, &key)).map(|p| p.id).collect() };
                for id in &ids {
                    self.media.forget(*id);
                    self.media.thumb_asks.remove(id);
                    // (a grid cell that remembers its render failed for want of the file asks again)
                    self.media.note_arrived(*id);
                }
                if blob == Blob::Mini
                    && let Some((_, left)) = st.minis_left.as_mut()
                {
                    *left = left.saturating_sub(1);
                }
                // a preview changes the photo's render keys ([`Session::synced_tier`]): renders that failed for
                // want of it are tried again without the library changing (which would redo every list of it)
                st.landed |= blob == Blob::Original;
                if blob == Blob::Original {
                    // (here now: the plan goes back to skipping the photos that want nothing)
                    st.want_originals.remove(&key);
                }
                if blob == Blob::Original && self.media.smart_dir.is_none() {
                    // in the host's storage now, where the photo's path already points
                    self.uploaded(st, key.clone());
                    if let Some(s) = self.sync_store.as_mut() {
                        s.originals.insert(key.clone());
                    }
                } else if blob == Blob::Original {
                    // a new original may put the device over its budget
                    st.evict_at = None;
                    // (a download that was resumed, or a folder file that changed on the server, may not be the
                    // photo it should be: the hash is the key)
                    if !file_has_hash(&dest, &key) {
                        let _ = std::fs::remove_file(&dest);
                        retry(st, &key, format!("{dest} isn't the original it should be"));
                        return;
                    }
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
            }
        }
    }

    /// This device finished uploading a photo (counted for the progress shown).
    fn upload_finished(&mut self, st: &mut SyncState, key: String) {
        st.up_done += 1;
        st.transfer_error = None;
        self.uploaded(st, key);
    }

    /// The server has all of a photo's files.
    fn uploaded(&mut self, st: &mut SyncState, key: String) {
        if let Some(lib) = self.library.as_mut()
            && let Err(e) = lib.files_mut().append(UPLOADED, format!("{key}\n").as_bytes())
        {
            log::error!("sync: {UPLOADED}: {e}");
        }
        st.uploaded.insert(key);
        // (signing out counts the photos left again at its next step: one fewer)
        st.unsent = None;
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

    /// [`Session::sync_fetch_activity_with`] over the network.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn sync_fetch_activity(&mut self) {
        self.sync_fetch_activity_with(&mut run);
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
    merge_by(base, ours, theirs, &|v| v.get("id").and_then(Value::as_str).map(str::to_string))
}

/// One item of a synced presets document, as this device takes it (the document is untrusted).
enum SyncedPreset {
    /// A user preset this device can hold.
    Usable(lightcraft_develop::Preset),
    /// Not one it can hold: not readable as a preset, an unusable id or a built-in's id.
    Unusable,
    /// An id an earlier item has (the first one counts).
    Repeated,
}

fn classify_preset(v: &Value, builtin: &HashSet<String>, seen: &mut HashSet<String>) -> SyncedPreset {
    let Ok(mut p) = <lightcraft_develop::Preset as Deserialize>::deserialize(v) else { return SyncedPreset::Unusable };
    if p.id.trim().is_empty() || p.id.len() > PRESET_ID_MAX || builtin.contains(&p.id) {
        return SyncedPreset::Unusable;
    }
    if !seen.insert(p.id.clone()) {
        return SyncedPreset::Repeated;
    }
    p.builtin = false;
    SyncedPreset::Usable(p)
}

/// The user presets this device holds of a synced list (`items`, the server's document merged with
/// its own): the usable ones, its `own` (the ids it holds already) always and the others while it
/// holds fewer than `max`.
pub(crate) fn hold_presets(items: &[Value], builtin: &HashSet<String>, own: &HashSet<String>, max: usize) -> Vec<lightcraft_develop::Preset> {
    let mut seen = HashSet::new();
    let mut held = Vec::new();
    let mut count = own.len();
    for v in items {
        let SyncedPreset::Usable(p) = classify_preset(v, builtin, &mut seen) else { continue };
        if !own.contains(&p.id) {
            if count >= max {
                continue;
            }
            count += 1;
        }
        held.push(p);
    }
    held
}

/// The items of the server's presets document as last synced (`base`) that this device left out
/// ([`hold_presets`]: unusable here, or past `max`), as they came: they go back with what it sends,
/// so they stay for the user's other devices. One of its `own` ids is its own copy; a usable one
/// within `max` that it doesn't hold was deleted here, and isn't sent back.
pub(crate) fn presets_left_out(base: &[Value], builtin: &HashSet<String>, own: &HashSet<String>, max: usize) -> Vec<Value> {
    let mut seen = HashSet::new();
    let mut count = own.len();
    let mut out = Vec::new();
    for v in base {
        if v.get("id").and_then(Value::as_str).is_some_and(|id| own.contains(id)) {
            continue;
        }
        match classify_preset(v, builtin, &mut seen) {
            SyncedPreset::Unusable => out.push(v.clone()),
            SyncedPreset::Repeated => {}
            SyncedPreset::Usable(_) if count < max => count += 1,
            SyncedPreset::Usable(_) => out.push(v.clone()),
        }
    }
    out
}

/// [`merge_presets`] for lists whose items are told apart by `id(item)` (a name, an id).
pub(crate) fn merge_by(base: &[Value], ours: &[Value], theirs: &[Value], id: &dyn Fn(&Value) -> Option<String>) -> Vec<Value> {
    // (an id that repeats in a list: its first item counts)
    let index = |list: &[Value]| list.iter().rev().filter_map(|v| Some((id(v)?, v.clone()))).collect::<HashMap<String, Value>>();
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
    // (a TIFF of several GB, mostly layers, is read in part)
    let bytes = crate::files::read_file(Path::new(original))?;
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

/// The shared HTTP client (pure-Rust TLS, Mozilla roots): sync and map tiles use it.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn http_agent() -> &'static ureq::Agent {
    net::agent()
}

#[cfg(not(target_arch = "wasm32"))]
mod net {
    use std::io::Read;
    use std::sync::{Arc, OnceLock};
    use std::time::Duration;

    use super::Body;

    /// Responses read into memory are capped (control answers, not photo files). The largest is a
    /// server from before paging sending the whole library at once (~4 KB a photo: 62,000 photos
    /// already pass 256 MiB); a page of it is ~8 MiB.
    const TEXT_MAX: u64 = 2 << 30;

    pub(super) fn agent() -> &'static ureq::Agent {
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
        // An original that broke on its way down is kept as `<file>.part` and asked for again from
        // there. (Not the previews: the server may have built a new one since.)
        let part = save_to.filter(|_| method == "GET" && url.contains("/api/blobs/original/")).map(|d| format!("{d}.part"));
        let resume = part.as_ref().and_then(|p| std::fs::metadata(p).ok()).map_or(0, |m| m.len());
        let r = match (method, body) {
            ("GET", _) if resume > 0 => agent().get(url).header("Authorization", &auth).header("Range", format!("bytes={resume}-")).call(),
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
                    Body::FileFrom(path, offset) => {
                        use std::io::{Seek, SeekFrom};
                        let mut f = std::fs::File::open(path).map_err(|e| format!("{path}: {e}"))?;
                        let len = f.metadata().map(|m| m.len()).map_err(|e| format!("{path}: {e}"))?;
                        let req = req.header("Content-Type", "application/octet-stream");
                        if *offset == 0 || *offset >= len {
                            req.header("Content-Length", len.to_string()).send(f)
                        } else {
                            f.seek(SeekFrom::Start(*offset)).map_err(|e| format!("{path}: {e}"))?;
                            req.header("Content-Length", (len - offset).to_string())
                                .header("Content-Range", format!("bytes {offset}-{}/{len}", len - 1))
                                .send(f)
                        }
                    }
                }
            }
        };
        let mut resp = r.map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        if method == "HEAD" {
            // (no body to read: the one thing a HEAD can add, how far an unfinished upload got)
            let at = resp.headers().get("upload-offset").and_then(|v| v.to_str().ok()).unwrap_or("");
            return Ok((status, at.to_string()));
        }
        if resume > 0
            && status == 416
            && let Some(part) = &part
        {
            // what is kept doesn't fit what the server has now: start again
            let _ = std::fs::remove_file(part);
            return http(method, url, token, body, save_to);
        }
        if let (Some(dest), true) = (save_to, (200..300).contains(&status)) {
            let path = std::path::Path::new(dest);
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            }
            let from = resp
                .headers()
                .get("content-range")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("bytes "))
                .and_then(|v| v.split('-').next())
                .and_then(|v| v.trim().parse::<u64>().ok());
            let mut reader = resp.body_mut().with_config().limit(u64::MAX).reader();
            let Some(part) = part else {
                lightcraft_catalog::safe_file::write_atomic_with(path, &mut |w| std::io::copy(&mut reader, w).map(|_| ()))
                    .map_err(|e| format!("{dest}: {e}"))?;
                return Ok((status, String::new()));
            };
            // 206 from where it was asked: carry on; the whole file: from the start
            let carry_on = status == 206 && resume > 0 && from == Some(resume);
            let mut f = if carry_on { std::fs::OpenOptions::new().append(true).open(&part) } else { std::fs::File::create(&part) }
                .map_err(|e| format!("{part}: {e}"))?;
            // (an error half-way leaves what arrived in the .part file)
            std::io::copy(&mut reader, &mut f).map_err(|e| format!("{dest}: {e}"))?;
            f.sync_all().map_err(|e| format!("{part}: {e}"))?;
            drop(f);
            std::fs::rename(&part, path).map_err(|e| format!("{dest}: {e}"))?;
            return Ok((status, String::new()));
        }
        let mut text = String::new();
        resp.body_mut().with_config().limit(TEXT_MAX).reader().read_to_string(&mut text).map_err(|e| e.to_string())?;
        Ok((status, text))
    }
}
