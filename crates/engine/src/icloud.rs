//! iCloud sync for a library that has no server: a frontend on Apple devices carries small records
//! between one person's devices through their own iCloud (CloudKit's private database, which the
//! engine never talks to itself). The engine says what to send and merges what comes back; the host
//! only moves records: [`Session::icloud_changes`] (what changed here), [`Session::icloud_records`]
//! (their contents, to save), [`Session::icloud_fetched`] (what the cloud has) and
//! [`Session::icloud_sent`] (what it took).
//!
//! What travels, one record per item:
//! - `presets`: the user's develop presets, by id;
//! - the settings a sync server keeps too ([`crate::cloud_sync`]'s shared documents: export, metadata,
//!   filter and curve presets, label and keyword sets, LUT profiles, import defaults, the order of the
//!   preset groups, the hidden presets, the privacy settings), one record per item of each;
//! - `server`: the sync server this library is signed in to (its address and the user, never a
//!   password or a token), so the other devices can offer it;
//! - `edit`: a photo opened from the system photo library and changed (its develop settings, rating,
//!   flag, label and metadata: its XMP sidecar), by the content hash of its original. Another device
//!   applies it to the same picture when it has it, and otherwise holds it until that picture is
//!   opened there (`photoLibrary.open`), which then comes in edited.
//!
//! - with *Keep Photos in iCloud Drive* on ([`photos`]), the photos themselves: their originals go to a folder of the
//!   person's iCloud Drive that the host names (`icloud.folder`), which iCloud Drive carries to the other devices, and
//!   each one's `edit` record says where its file is there, so another device adds it to its library when the file
//!   has come (`icloud.wanted`, `icloud.adopt`);
//! - `analysis`: what was found in such a photo (faces, the words in it, the description vector, the sensitive-content
//!   score), so a device that can't run the models or hasn't yet has them too ([`analysis`]).
//!
//! A library signed in to a server syncs its presets, settings and photos through the server: then only
//! `server` records travel here, and what this device knew of the others is forgotten, so signing out
//! of the server later (which removes the account's presets) merges again instead of deleting them.
//!
//! **Merging.** For every record the device keeps the cloud's version as last agreed ([`Known`], by
//! hash). A record changed on one side only takes that side; changed on both, the newer change wins
//! (each device notes when it first saw its own change), and the host's store orders the writes (a
//! save over a version it hasn't seen is refused and comes back as fetched), so the devices end up the
//! same. A record removed on one side goes on the other unless that side changed it. The cloud's
//! version, once taken, may be stored a little differently here (another app version): the device
//! notes its own rendering of it ([`Known::local`]) so it doesn't send it back. `server` and `edit`
//! records that have no place here yet are held ([`Held`]) and count as this device's own, so having no
//! such photo or server is never taken for a deletion.
//!
//! Files in the library: `icloud.json` ([`Saved`]: whether it is on, the known versions, the change
//! times and the host's own state, kept with them so the two never disagree) and `icloud.held`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use lightcraft_catalog::{Op, Photo, PhotoId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::cmd::{CommandSpec, always, bad, cmd};
use crate::{EngineError, Result, Session};

pub mod analysis;
pub mod collections;
pub mod folder;
pub mod photos;

/// The sync state, in the library.
pub const STATE: &str = "icloud.json";
/// The records held for later, in the library.
pub const HELD: &str = "icloud.held";

/// The user's develop presets.
pub const PRESETS: &str = "presets";
/// A sync server a device is signed in to.
pub const SERVER: &str = "server";
/// A photo-library photo's edit.
pub const EDIT: &str = "edit";
/// The History label of an edit taken from another device.
pub const LABEL: &str = "Synced from iCloud";

/// A record name is this long at most (CloudKit's limit).
const NAME_MAX: usize = 255;
/// A record's key is this long at most.
const KEY_MAX: usize = 1024;
/// A record's data is this large at most (a LUT profile carries its `.cube` text, up to 4 MiB).
const DATA_MAX: usize = 8 << 20;
/// Records per call at most.
const BATCH_MAX: usize = 2000;
/// Records held at most.
const HELD_MAX: usize = 100_000;
/// Develop presets taken at most (as a server's are).
const PRESETS_MAX: usize = 100_000;
/// A preset id is this long at most.
const PRESET_ID_MAX: usize = 200;
/// The host's own state is this large at most.
const TRANSPORT_MAX: usize = 16 << 20;

/// The cloud's version of a record, as this device last agreed with it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Known {
    kind: String,
    key: String,
    /// The hash of the cloud's data.
    hash: String,
    /// This device's rendering of that data, when it stores it differently.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    local: Option<String>,
    /// An `analysis` record's parts ([`analysis`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parts: Option<u8>,
}

/// A record the cloud has that has no place on this device yet.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Held {
    kind: String,
    key: String,
    data: String,
    hash: String,
}

/// `icloud.json`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Saved {
    pub on: bool,
    /// *Keep Photos in iCloud Drive*: imports go to the host's iCloud Drive folder, and photos other devices keep there
    /// come into this library ([`photos`]).
    pub photos: bool,
    /// Changes whenever this device forgets what the cloud has: the host then fetches everything again.
    epoch: u64,
    /// This library's name in a shared cloud folder ([`folder`]); kept when sync is turned off and on.
    device: String,
    /// The folder this library syncs through ([`folder`]; `icloud.useFolder`).
    folder: Option<String>,
    /// The uid of each album (`a<id>`) and stack (`s<id>`) of this library that has been sent ([`collections`]).
    uids: BTreeMap<String, String>,
    /// Members of an album the cloud has that this device has no photo for yet, by album uid.
    pending: BTreeMap<String, Vec<String>>,
    /// The library was signed in to a server when last looked at (signing out forgets, so it fetches again).
    linked: bool,
    known: BTreeMap<String, Known>,
    /// When this device first saw its own change to a record (ms since 1970): the newer change wins.
    changed: BTreeMap<String, i64>,
    /// The host's own state (opaque to the engine).
    #[serde(skip_serializing_if = "Option::is_none")]
    transport: Option<String>,
}

/// One of this device's records as it is now.
#[derive(Clone, Debug, PartialEq)]
struct Mine {
    kind: String,
    key: String,
    hash: String,
    /// An `analysis` record's parts (0 for the other kinds).
    parts: u8,
}

/// A record the host fetched (or that a save was refused over).
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Incoming {
    pub name: String,
    pub kind: String,
    pub key: String,
    pub data: String,
    /// When the device that wrote it changed it (ms since 1970).
    #[serde(default)]
    pub modified: i64,
}

/// A record the host saved.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sent {
    pub name: String,
    pub kind: String,
    pub key: String,
    pub hash: String,
}

/// What changed here since the cloud last agreed: records to save, records to delete (by name).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Changes {
    pub save: Vec<String>,
    pub delete: Vec<String>,
}

/// What this device's records of each kind but `edit` were last worked out from (`None`: work them out).
#[derive(Clone, Debug, Default, PartialEq)]
struct Seen {
    /// The develop presets' generation.
    presets: Option<u64>,
    /// prefs.json's writes (the shared documents are kept there).
    prefs: Option<u64>,
    /// The server record's key ("" for none).
    server: Option<String>,
    linked: Option<bool>,
    /// Albums and stacks: worked out (`Some`) until an op that touches them.
    collections: Option<()>,
}

/// Milliseconds since 1970 now.
fn now_ms() -> i64 {
    web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX)).unwrap_or(0)
}

/// The iCloud sync of a library (in [`Session`] while it is on).
pub struct ICloud {
    saved: Saved,
    held: BTreeMap<String, Held>,
    saved_dirty: bool,
    held_dirty: bool,
    /// This device's records now, by name.
    ours: BTreeMap<String, Mine>,
    /// Each photo's edit record, and each edit record's photo.
    edits_of: HashMap<PhotoId, String>,
    photo_of: HashMap<String, PhotoId>,
    /// Photos to look at again.
    dirty: HashSet<PhotoId>,
    /// What the presets, settings and server records were last worked out from.
    seen: Seen,
    /// Every photo-library photo was looked at since this was loaded.
    scanned: bool,
    /// Changes whenever what there is to send may have changed.
    rev: u64,
    clock: Box<dyn Fn() -> i64 + Send>,
    /// This device's iCloud Drive folder for photos, as the host last said (`icloud.folder`; not kept: the host says
    /// it at every start).
    folder: Option<PathBuf>,
    /// The activity rows of photos coming from iCloud Drive and of originals moving into it, with how many are done.
    bringing: Option<(crate::activity::TaskGuard, u64)>,
    moving: Option<(crate::activity::TaskGuard, u64)>,
    /// What [`analysis`] last saw of the search indexes (their sizes): a change makes it look again.
    analysis_seen: Option<[usize; 4]>,
    /// How the last pass through the cloud folder went (not kept).
    folder_last: Option<folder::Last>,
    /// This device's `album` and `stack` records' data, by record name ([`collections`]).
    coll: BTreeMap<String, String>,
    /// A pass through the cloud folder was asked for (`icloud.folderNow`); the host's next poll runs it.
    folder_now: bool,
}

impl ICloud {
    fn new(saved: Saved, held: BTreeMap<String, Held>) -> ICloud {
        ICloud {
            saved,
            held,
            saved_dirty: false,
            held_dirty: false,
            ours: BTreeMap::new(),
            edits_of: HashMap::new(),
            photo_of: HashMap::new(),
            dirty: HashSet::new(),
            seen: Seen::default(),
            scanned: false,
            rev: 1,
            clock: Box::new(now_ms),
            folder: None,
            bringing: None,
            moving: None,
            analysis_seen: None,
            folder_last: None,
            coll: BTreeMap::new(),
            folder_now: false,
        }
    }

    /// Read from the library's files (`None`: it was never turned on, or is off).
    pub(crate) fn load(files: &mut dyn lightcraft_catalog::Store) -> Option<ICloud> {
        let saved: Saved = files.read(STATE).ok().flatten().and_then(|b| serde_json::from_slice(&b).ok())?;
        if !saved.on {
            return None;
        }
        let held = files.read(HELD).ok().flatten().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Some(ICloud::new(saved, held))
    }

    /// Write what changed (errors are logged: the next save tries again).
    pub(crate) fn save(&mut self, files: &mut dyn lightcraft_catalog::Store) {
        if self.held_dirty {
            match serde_json::to_vec(&self.held).map_err(|e| e.to_string()).and_then(|b| files.write_atomic(HELD, &b).map_err(|e| e.to_string())) {
                Ok(()) => self.held_dirty = false,
                Err(e) => log::error!("library: {HELD}: {e}"),
            }
        }
        if self.saved_dirty {
            match serde_json::to_vec(&self.saved).map_err(|e| e.to_string()).and_then(|b| files.write_atomic(STATE, &b).map_err(|e| e.to_string())) {
                Ok(()) => self.saved_dirty = false,
                Err(e) => log::error!("library: {STATE}: {e}"),
            }
        }
    }

    /// An op was applied: the photos it touches are looked at again.
    pub(crate) fn note(&mut self, op: &Op) {
        if collections::touches(op, 0) && self.seen.collections.take().is_some() {
            self.rev = self.rev.wrapping_add(1);
        }
        let before = self.dirty.len();
        touched(op, &mut self.dirty, 0);
        if self.dirty.len() != before {
            self.rev = self.rev.wrapping_add(1);
        }
    }

    /// Use `clock` for the change times (tests).
    pub fn set_clock(&mut self, clock: Box<dyn Fn() -> i64 + Send>) {
        self.clock = clock;
    }

    fn bump(&mut self) {
        self.rev = self.rev.wrapping_add(1);
        self.saved_dirty = true;
    }

    /// Forget what the cloud has (the account changed, the cloud's data was deleted, or sync was turned off):
    /// the next sync merges this device's records with the cloud's as they are, removing nothing.
    fn forget(&mut self) {
        self.saved.known.clear();
        self.saved.changed.clear();
        if !self.held.is_empty() {
            self.held.clear();
            self.held_dirty = true;
        }
        self.refetch();
    }

    /// Have the host fetch everything again (what this device knows of the cloud stays).
    fn refetch(&mut self) {
        self.saved.epoch = self.saved.epoch.wrapping_add(1).max(now_ms().unsigned_abs());
        self.saved.transport = None;
        self.seen = Seen::default();
        self.scanned = false;
        self.ours.clear();
        self.edits_of.clear();
        self.photo_of.clear();
        self.bump();
    }

    /// Whether this device's version of `name` differs from the cloud's as last agreed.
    fn differs(&self, name: &str, mine: Option<&Mine>) -> bool {
        match (mine, self.saved.known.get(name)) {
            // (analysis only fills in: sent while this device has a part the cloud lacks)
            (Some(m), None) if m.kind == analysis::ANALYSIS => m.parts != 0,
            (Some(m), Some(k)) if m.kind == analysis::ANALYSIS => m.parts & !k.parts.unwrap_or(0) != 0,
            (None, None) => false,
            (Some(_), None) | (None, Some(_)) => true,
            (Some(m), Some(k)) => m.hash != k.hash && k.local.as_deref() != Some(m.hash.as_str()),
        }
    }
}

/// The photos an op touches in a way an edit record shows (or that adds, removes or replaces them).
fn touched(op: &Op, out: &mut HashSet<PhotoId>, depth: usize) {
    match op {
        Op::SetRating { id, .. }
        | Op::SetFlag { id, .. }
        | Op::SetLabel { id, .. }
        | Op::SetDevelop { id, .. }
        | Op::SetMeta { id, .. }
        | Op::SetCaptured { id, .. }
        | Op::SetDeleted { id, .. }
        | Op::SetVersions { id, .. }
        | Op::SetLocal { id, .. }
        | Op::SetContent { id, .. }
        | Op::RemovePhoto { id } => {
            out.insert(*id);
        }
        Op::AddPhoto { photo } => {
            out.insert(photo.id);
        }
        Op::Batch { ops } if depth < 32 => ops.iter().for_each(|o| touched(o, out, depth + 1)),
        _ => {}
    }
}

/// The record name of `key` of `kind`: the kind and a hash of the key (a name CloudKit takes whatever the key is).
pub fn record_name(kind: &str, key: &str) -> String {
    format!("{kind}.{}", lightcraft_preview::hash_bytes(key.as_bytes()))
}

fn hash_of(data: &str) -> String {
    lightcraft_preview::hash_bytes(data.as_bytes()).to_string()
}

/// A kind this engine syncs: the presets, a shared document, a server or an edit.
fn known_kind(kind: &str) -> bool {
    matches!(kind, PRESETS | SERVER | EDIT | analysis::ANALYSIS | collections::ALBUM | collections::STACK)
        || crate::cloud_sync::shared_doc_names().any(|n| n == kind)
}

/// Kinds a library signed in to a server leaves to it.
fn server_keeps(kind: &str) -> bool {
    kind != SERVER
}

impl Session {
    /// Whether iCloud sync is on for this library.
    pub fn icloud_on(&self) -> bool {
        self.icloud.is_some()
    }

    /// Run `f` with the iCloud state taken out of the session (put back after, whatever happens).
    fn with_icloud<R>(&mut self, f: impl FnOnce(&mut Session, &mut ICloud) -> R) -> Option<R> {
        let mut ic = self.icloud.take()?;
        let r = f(self, &mut ic);
        self.icloud = Some(ic);
        Some(r)
    }

    /// Turn iCloud sync on or off for this library. Off forgets what the cloud has (the cloud keeps it, and this
    /// device's presets, settings and edits stay as they are); on again merges as on a new device.
    pub fn icloud_enable(&mut self, on: bool) -> Result<()> {
        let Some(lib) = self.library.as_mut().filter(|l| l.on_disk) else {
            return Err(EngineError::Other("iCloud sync needs a library that is saved (open or create one first)".into()));
        };
        match (on, self.icloud.as_mut()) {
            (true, Some(_)) => {}
            (true, None) => {
                // (past the epoch it had when it was turned off: one from the clock alone could come back equal to
                // the one the host had while it was on, turned on again within the millisecond or after the clock
                // went back, and the host would not fetch everything again)
                let last = lib.files_mut().read(STATE).ok().flatten().and_then(|b| serde_json::from_slice::<Saved>(&b).ok());
                let epoch = last.as_ref().map_or(0, |s| s.epoch).wrapping_add(1).max(now_ms().unsigned_abs());
                let device = last.map(|s| s.device).unwrap_or_default();
                let mut ic = ICloud::new(Saved { on: true, epoch, device, ..Saved::default() }, BTreeMap::new());
                ic.saved_dirty = true;
                ic.held_dirty = true;
                ic.save(lib.files_mut());
                self.icloud = Some(ic);
            }
            (false, Some(ic)) => {
                ic.forget();
                ic.saved.on = false;
                ic.save(lib.files_mut());
                self.icloud = None;
            }
            (false, None) => {}
        }
        Ok(())
    }

    /// Forget what the cloud has (the iCloud account changed, or its data was deleted): the next sync merges
    /// this device's records with the cloud's, removing nothing.
    pub fn icloud_reset(&mut self) {
        if let Some(ic) = self.icloud.as_mut() {
            ic.forget();
        }
    }

    /// Changes whenever what there is to send may have changed (the host then asks [`Session::icloud_changes`]);
    /// `None` while sync is off.
    pub fn icloud_rev(&self) -> Option<u64> {
        let ic = self.icloud.as_ref()?;
        let lib = self.library.as_ref().map(|l| (l.presets_gen(), l.prefs_gen())).unwrap_or_default();
        let signed = self.sync.as_ref().map(|st| (st.signed_in(), st.config.library.is_empty()));
        let mut h = std::hash::DefaultHasher::new();
        // (what the search indexes hold, coarsely: a few dozen photos more worked out may be worth sending)
        let found: usize = self.analysis_sizes().iter().sum::<usize>() / 32;
        std::hash::Hash::hash(&(ic.rev, self.presets.generation(), lib, signed, self.photo_library.len(), found), &mut h);
        Some(std::hash::Hasher::finish(&h))
    }

    /// Whether this library syncs through a server (which then keeps its presets, settings and photos).
    fn server_linked(&self) -> bool {
        self.sync.as_ref().is_some_and(|st| st.signed_in() || !st.config.library.is_empty())
    }

    /// The server this device is signed in to, as a record (key, data).
    fn server_record(&self) -> Option<(String, String)> {
        let st = self.sync.as_ref().filter(|st| !st.config.token.is_empty() && !st.config.leaving)?;
        let (server, user) = (st.config.server.trim(), st.config.user.trim());
        if server.is_empty() || user.is_empty() {
            return None;
        }
        Some((format!("{}\n{user}", server.to_lowercase()), json!({"server": server, "user": user}).to_string()))
    }

    /// This device's develop presets, as (key, data).
    fn preset_records(&self) -> impl Iterator<Item = (String, String)> + '_ {
        self.presets
            .iter()
            .filter(|p| !p.builtin && !p.id.starts_with(crate::presets::SERVER_PREFIX))
            .filter_map(|p| serde_json::to_string(p).ok().map(|v| (p.id.clone(), v)))
    }

    /// This device's items of shared document `name`, as (key, data).
    fn doc_records(&self, name: &str) -> Vec<(String, String)> {
        self.doc_items(name).into_iter().filter_map(|item| Some((crate::cloud_sync::shared_doc_key(name, &item)?, item.to_string()))).collect()
    }

    /// The data of this device's record `m` of a kind but `edit` (not held), as it is now.
    fn setting_data(&self, m: &Mine) -> Option<String> {
        match m.kind.as_str() {
            PRESETS => self.preset_records().find(|(k, _)| *k == m.key).map(|(_, d)| d),
            SERVER => self.server_record().filter(|(k, _)| *k == m.key).map(|(_, d)| d),
            kind => self.doc_records(kind).into_iter().find(|(k, _)| *k == m.key).map(|(_, d)| d),
        }
    }

    /// Photo `p`'s edit record (key and data), when it is one this device sends: a library photo opened from
    /// the photo library, or one whose original is in the iCloud Drive folder `folder`, with a content hash. A photo
    /// in Recently Deleted or Hidden says so.
    fn edit_record(&self, p: &Photo, folder: Option<&Path>) -> Option<(String, String)> {
        if p.local || p.copy_of.is_some() {
            return None;
        }
        let file = folder.and_then(|f| photos::file_in(p, f));
        let cloud = match self.photo_library.entry_of(p.id) {
            Some((hash, cloud)) if hash == p.content_hash.as_deref() => cloud,
            _ if file.is_some() => None,
            _ => return None,
        };
        let key = lightcraft_catalog::sync::content_key(p)?;
        let mut data = json!({
            "xmp": crate::sidecar::sidecar_packet(p, &self.catalog),
            "name": p.file_name,
            "asset": cloud,
            "versions": p.versions,
        });
        if let Some(o) = data.as_object_mut() {
            if let Some(file) = file {
                o.insert("file".into(), json!(file));
            }
            if p.deleted {
                o.insert("deleted".into(), json!(true));
            }
            if p.hidden {
                o.insert("hidden".into(), json!(true));
            }
        }
        Some((key, data.to_string()))
    }

    /// Replace this device's records of `kind` by `records` (key, data).
    fn set_ours(ic: &mut ICloud, kind: &str, records: impl IntoIterator<Item = (String, String)>) {
        ic.ours.retain(|_, m| m.kind != kind);
        for (key, data) in records {
            ic.ours.insert(record_name(kind, &key), Mine { kind: kind.to_string(), hash: hash_of(&data), key, parts: 0 });
        }
    }

    /// Bring this device's records up to date (`ic` is out of the session).
    fn icloud_refresh(&mut self, ic: &mut ICloud) {
        let linked = self.server_linked();
        if ic.saved.linked != linked {
            // signed out of the server: the presets, settings and edits the cloud has are fetched again and merged, as
            // on a new device (what it knew of them was forgotten when the library signed in)
            if !linked {
                ic.refetch();
            }
            ic.saved.linked = linked;
            ic.saved_dirty = true;
        }
        if linked && ic.saved.known.values().any(|k| server_keeps(&k.kind)) {
            // the server keeps them now: what the cloud had of them is forgotten, never deleted
            ic.saved.known.retain(|_, k| !server_keeps(&k.kind));
            ic.saved.changed.retain(|name, _| ic.saved.known.contains_key(name));
            ic.saved_dirty = true;
        }
        if linked && ic.held.values().any(|h| server_keeps(&h.kind)) {
            ic.held.retain(|_, h| !server_keeps(&h.kind));
            ic.held_dirty = true;
        }
        // presets, settings and the server: each kind worked out again when it may have changed
        if ic.seen.linked != Some(linked) {
            ic.seen = Seen { linked: Some(linked), ..Seen::default() };
        }
        let presets = self.presets.generation();
        if ic.seen.presets != Some(presets) {
            let records: Vec<_> = if linked { Vec::new() } else { self.preset_records().collect() };
            Self::set_ours(ic, PRESETS, records);
            ic.seen.presets = Some(presets);
        }
        let prefs = self.library.as_ref().map(|l| l.prefs_gen()).unwrap_or_default();
        if ic.seen.prefs != Some(prefs) {
            for name in crate::cloud_sync::shared_doc_names() {
                let records = if linked { Vec::new() } else { self.doc_records(name) };
                Self::set_ours(ic, name, records);
            }
            ic.seen.prefs = Some(prefs);
        }
        let server = self.server_record();
        let server_key = server.as_ref().map(|(k, _)| k.clone()).unwrap_or_default();
        if ic.seen.server.as_ref() != Some(&server_key) {
            Self::set_ours(ic, SERVER, server);
            ic.seen.server = Some(server_key);
        }
        // albums and stacks (a library signed in to a server has them there)
        if linked {
            ic.ours.retain(|_, m| !collections::is_collection(&m.kind));
            ic.coll.clear();
            ic.seen.collections = None;
        } else if ic.seen.collections.is_none() && self.interaction.is_none() {
            collections::refresh(self, ic);
            ic.seen.collections = Some(());
        }
        // edits: every photo-library photo once, then the ones that changed (never mid-drag: the catalog holds a preview)
        if linked {
            ic.ours.retain(|_, m| m.kind != EDIT);
            ic.edits_of.clear();
            ic.photo_of.clear();
            ic.dirty.clear();
            ic.scanned = false;
        } else if self.interaction.is_none() {
            if !ic.scanned {
                ic.ours.retain(|_, m| m.kind != EDIT);
                ic.edits_of.clear();
                ic.photo_of.clear();
                ic.dirty = self.photo_library.photo_ids().collect();
                if let Some(folder) = ic.folder.clone() {
                    ic.dirty.extend(photos::in_folder(self, &folder));
                }
                ic.scanned = true;
            } else {
                // assets mapped to photos the library already had (no op says so)
                let fresh: Vec<PhotoId> = self.photo_library.photo_ids().filter(|id| !ic.edits_of.contains_key(id)).collect();
                ic.dirty.extend(fresh);
            }
            let dirty: Vec<PhotoId> = ic.dirty.drain().collect();
            for id in dirty {
                self.icloud_refresh_photo(ic, id);
            }
        }
        // what is held counts as this device's own (an edit of a photo it has wins)
        for (name, h) in &ic.held {
            ic.ours.entry(name.clone()).or_insert_with(|| Mine { kind: h.kind.clone(), key: h.key.clone(), hash: h.hash.clone(), parts: 0 });
        }
        // what was found in the photos that travel
        if linked {
            ic.ours.retain(|_, m| m.kind != analysis::ANALYSIS);
        } else {
            analysis::refresh(self, ic);
        }
    }

    /// Work out photo `id`'s edit record again.
    fn icloud_refresh_photo(&mut self, ic: &mut ICloud, id: PhotoId) {
        let folder = ic.folder.clone();
        let mut now = self.catalog.photo(id).and_then(|p| self.edit_record(p, folder.as_deref()));
        let name = now.as_ref().map(|(key, _)| record_name(EDIT, key));
        // a photo that has just come (opened, imported, brought from iCloud Drive) takes the edit another device made to
        // its picture, held until now: the other device's work isn't overwritten by an untouched copy
        if let Some(n) = name.as_ref().filter(|n| !ic.edits_of.contains_key(&id) && !ic.photo_of.contains_key(*n))
            && let Some(h) = ic.held.get(n).filter(|h| h.kind == EDIT).cloned()
        {
            match self.icloud_apply_edit(id, &h.data) {
                Ok(()) => {
                    ic.held.remove(n);
                    ic.held_dirty = true;
                    now = self.catalog.photo(id).and_then(|p| self.edit_record(p, folder.as_deref()));
                    let mine = now.as_ref().map(|(_, d)| hash_of(d));
                    if let Some(k) = ic.saved.known.get_mut(n) {
                        k.local = mine.filter(|m| *m != k.hash);
                        ic.saved_dirty = true;
                    }
                }
                Err(e) => log::warn!("iCloud: photo {}: {e}", id.0),
            }
        }
        if let Some(old) = ic.edits_of.get(&id).cloned().filter(|o| Some(o) != name.as_ref()) {
            ic.edits_of.remove(&id);
            if ic.photo_of.get(&old) == Some(&id) {
                ic.photo_of.remove(&old);
                ic.ours.remove(&old);
                // another photo of the same picture speaks for it now
                let others: Vec<PhotoId> = ic.edits_of.iter().filter(|(_, n)| **n == old).map(|(o, _)| *o).collect();
                for o in others {
                    ic.edits_of.remove(&o);
                    ic.dirty.insert(o);
                }
            }
        }
        let (Some((key, data)), Some(name)) = (now, name) else { return };
        ic.edits_of.insert(id, name.clone());
        // two assets of the same picture: the photo with the lower id speaks for it
        if ic.photo_of.get(&name).is_some_and(|o| *o < id && ic.edits_of.get(o) == Some(&name)) {
            return;
        }
        ic.photo_of.insert(name.clone(), id);
        ic.ours.insert(name, Mine { kind: EDIT.to_string(), key, hash: hash_of(&data), parts: 0 });
    }

    /// What changed here since the cloud last agreed: the records to save and to delete.
    pub fn icloud_changes(&mut self) -> Changes {
        self.with_icloud(|s, ic| {
            s.icloud_refresh(ic);
            let mut out = Changes::default();
            for (name, m) in &ic.ours {
                if ic.differs(name, Some(m)) {
                    out.save.push(name.clone());
                }
            }
            for (name, k) in &ic.saved.known {
                // (an analysis record goes only with its photo's: one this device has nothing of yet stays for the others)
                let photo_travels = k.kind == analysis::ANALYSIS && ic.ours.contains_key(&record_name(EDIT, &k.key));
                if !ic.ours.contains_key(name) && !photo_travels {
                    out.delete.push(name.clone());
                }
            }
            let now = (ic.clock)();
            let mut noted = false;
            for name in out.save.iter().chain(&out.delete) {
                if !ic.saved.changed.contains_key(name) {
                    ic.saved.changed.insert(name.clone(), now);
                    noted = true;
                }
            }
            let before = ic.saved.changed.len();
            ic.saved.changed.retain(|name, _| out.save.contains(name) || out.delete.contains(name));
            if noted || ic.saved.changed.len() != before {
                ic.saved_dirty = true;
            }
            out
        })
        .unwrap_or_default()
    }

    /// The data of records `names` as they are now, to save: `{name, kind, key, data, hash, modified}` each; a
    /// record this device no longer has is `{name, gone: true}`.
    pub fn icloud_records(&mut self, names: &[String]) -> Vec<Value> {
        self.with_icloud(|s, ic| {
            s.icloud_refresh(ic);
            let now = (ic.clock)();
            // (a document's items are read once per call, and so are the indexes the analysis records come from)
            let mut docs: HashMap<String, Vec<(String, String)>> = HashMap::new();
            let analysed: Vec<String> =
                names.iter().take(BATCH_MAX).filter_map(|n| ic.ours.get(n).filter(|m| m.kind == analysis::ANALYSIS)).map(|m| m.key.clone()).collect();
            let mut found_in = if analysed.is_empty() { HashMap::new() } else { s.analysis_data(&analysed) };
            names
                .iter()
                .take(BATCH_MAX)
                .map(|name| {
                    let held = || ic.held.get(name).map(|h| (h.kind.clone(), h.key.clone(), h.data.clone()));
                    let found = match ic.ours.get(name) {
                        Some(m) if m.kind == EDIT => {
                            let photo =
                                ic.photo_of.get(name).and_then(|id| s.catalog.photo(*id)).and_then(|p| s.edit_record(p, ic.folder.as_deref()));
                            photo.map(|(key, data)| (EDIT.to_string(), key, data)).or_else(held)
                        }
                        Some(m) if m.kind == analysis::ANALYSIS => found_in.remove(&m.key).map(|v| (m.kind.clone(), m.key.clone(), v.to_string())),
                        Some(m) if m.kind == PRESETS || m.kind == SERVER => {
                            s.setting_data(m).map(|d| (m.kind.clone(), m.key.clone(), d)).or_else(held)
                        }
                        Some(m) if collections::is_collection(&m.kind) => {
                            ic.coll.get(name).map(|d| (m.kind.clone(), m.key.clone(), d.clone())).or_else(held)
                        }
                        Some(m) => {
                            let items = docs.entry(m.kind.clone()).or_insert_with(|| s.doc_records(&m.kind));
                            items.iter().find(|(k, _)| *k == m.key).map(|(k, d)| (m.kind.clone(), k.clone(), d.clone()))
                        }
                        None => None,
                    };
                    match found {
                        Some((kind, key, data)) => {
                            let hash = hash_of(&data);
                            if let Some(m) = ic.ours.get_mut(name).filter(|m| m.kind != analysis::ANALYSIS) {
                                m.hash.clone_from(&hash);
                            }
                            let modified = ic.saved.changed.get(name).copied().unwrap_or(now);
                            json!({"name": name, "kind": kind, "key": key, "data": data, "hash": hash, "modified": modified})
                        }
                        None => json!({"name": name, "gone": true}),
                    }
                })
                .collect()
        })
        .unwrap_or_default()
    }

    /// The host saved these records and deleted these: the cloud has them as sent.
    pub fn icloud_sent(&mut self, saved: &[Sent], deleted: &[String]) {
        self.with_icloud(|_, ic| {
            for r in saved.iter().take(BATCH_MAX) {
                if r.name.len() > NAME_MAX || r.name != record_name(&r.kind, &r.key) || !known_kind(&r.kind) {
                    continue;
                }
                let parts = ic.ours.get(&r.name).filter(|m| m.kind == analysis::ANALYSIS).map(|m| m.parts);
                ic.saved.known.insert(r.name.clone(), Known { kind: r.kind.clone(), key: r.key.clone(), hash: r.hash.clone(), local: None, parts });
                if parts.is_some() || ic.ours.get(&r.name).is_some_and(|m| m.hash == r.hash) {
                    ic.saved.changed.remove(&r.name);
                }
            }
            for name in deleted.iter().take(BATCH_MAX) {
                ic.saved.known.remove(name);
                if !ic.ours.contains_key(name) {
                    ic.saved.changed.remove(name);
                }
            }
            // (a pass that wrote nothing changes nothing: a host that runs a pass when this changes would never rest)
            if !saved.is_empty() || !deleted.is_empty() {
                ic.bump();
            }
        });
    }

    /// Records the cloud has (fetched, or the cloud's version of one a save was refused over) and records it
    /// deleted: merged into this device's. Answers how many changed something here.
    pub fn icloud_fetched(&mut self, records: &[Incoming], deleted: &[String]) -> usize {
        self.with_icloud(|s, ic| s.icloud_merge(ic, records, deleted)).unwrap_or(0)
    }

    fn icloud_merge(&mut self, ic: &mut ICloud, records: &[Incoming], deleted: &[String]) -> usize {
        self.icloud_refresh(ic);
        let linked = self.server_linked();
        let now = (ic.clock)();
        // (name, kind, key, data): what to take; data `None` removes it here
        let mut take: Vec<(String, String, String, Option<String>)> = Vec::new();
        for r in records.iter().take(BATCH_MAX) {
            if r.name.len() > NAME_MAX
                || r.key.is_empty()
                || r.key.len() > KEY_MAX
                || r.data.len() > DATA_MAX
                || !known_kind(&r.kind)
                || r.name != record_name(&r.kind, &r.key)
                || (linked && server_keeps(&r.kind))
            {
                log::warn!("iCloud: record `{}` left out (not one this version reads)", r.name.chars().take(80).collect::<String>());
                continue;
            }
            let hash = hash_of(&r.data);
            // what was found in a photo only fills in what this device lacks (its own stays)
            if r.kind == analysis::ANALYSIS {
                let parts = serde_json::from_str::<Value>(&r.data).map(|v| crate::vision::analysis_parts_of(&v)).unwrap_or(0);
                let mine = ic.ours.get(&r.name).map_or(0, |m| m.parts);
                ic.saved.known.insert(r.name.clone(), Known { kind: r.kind.clone(), key: r.key.clone(), hash, local: None, parts: Some(parts) });
                ic.saved.changed.remove(&r.name);
                if parts & !mine != 0 {
                    take.push((r.name.clone(), r.kind.clone(), r.key.clone(), Some(r.data.clone())));
                }
                continue;
            }
            let mine = ic.ours.get(&r.name);
            if mine.is_some_and(|m| m.hash == hash) {
                // the same as here
                ic.saved.known.insert(r.name.clone(), Known { kind: r.kind.clone(), key: r.key.clone(), hash, local: None, parts: None });
                ic.saved.changed.remove(&r.name);
                continue;
            }
            if ic.saved.known.get(&r.name).is_some_and(|k| k.hash == hash) {
                // already known: nothing new (this device's change, if any, still goes)
                continue;
            }
            let changed_here = ic.differs(&r.name, mine);
            let mine_at = *ic.saved.changed.entry(r.name.clone()).or_insert(now);
            if !changed_here || r.modified > mine_at {
                take.push((r.name.clone(), r.kind.clone(), r.key.clone(), Some(r.data.clone())));
            }
            ic.saved.known.insert(r.name.clone(), Known { kind: r.kind.clone(), key: r.key.clone(), hash, local: None, parts: None });
            if !changed_here {
                ic.saved.changed.remove(&r.name);
            }
        }
        for name in deleted.iter().take(BATCH_MAX) {
            let Some(k) = ic.saved.known.remove(name) else { continue };
            // (what was found in a photo stays here: it is this device's now)
            if k.kind == analysis::ANALYSIS {
                continue;
            }
            let mine = ic.ours.get(name);
            let unchanged = mine.is_some_and(|m| m.hash == k.hash || k.local.as_deref() == Some(m.hash.as_str()));
            if unchanged {
                take.push((name.clone(), k.kind.clone(), k.key.clone(), None));
                ic.saved.changed.remove(name);
            }
        }
        ic.bump();
        if take.is_empty() {
            return 0;
        }
        let applied = self.icloud_apply(ic, &take);
        // what this device now has of each taken record: its own rendering of the cloud's version
        ic.seen = Seen::default();
        self.icloud_refresh(ic);
        for (name, kind, _, _) in &take {
            if kind == analysis::ANALYSIS {
                continue;
            }
            let mine = ic.ours.get(name).map(|m| m.hash.clone());
            if let Some(k) = ic.saved.known.get_mut(name) {
                k.local = mine.filter(|h| *h != k.hash);
            }
            ic.saved.changed.remove(name);
        }
        ic.saved_dirty = true;
        applied
    }

    /// Take records into this device: presets and settings replace or remove their item, a server or an edit of
    /// a photo not here is held, an edit of a photo here is applied to it.
    fn icloud_apply(&mut self, ic: &mut ICloud, take: &[(String, String, String, Option<String>)]) -> usize {
        let mut applied = 0;
        // presets: one change to the list
        let presets: Vec<_> = take.iter().filter(|t| t.1 == PRESETS).collect();
        if !presets.is_empty() {
            applied += self.icloud_take_presets(&presets);
        }
        // settings documents: one change to each
        for name in crate::cloud_sync::shared_doc_names() {
            let changes: Vec<_> = take.iter().filter(|t| t.1 == name).collect();
            if changes.is_empty() {
                continue;
            }
            let mut items = self.doc_items(name);
            for (_, _, key, data) in &changes {
                // (an item this version can't read leaves this device's as it is)
                let new = match data.as_deref().map(serde_json::from_str::<Value>) {
                    None => None,
                    Some(Ok(v)) if crate::cloud_sync::shared_doc_key(name, &v).as_deref() == Some(key.as_str()) => Some(v),
                    Some(_) => continue,
                };
                items.retain(|v| crate::cloud_sync::shared_doc_key(name, v).as_deref() != Some(key.as_str()));
                items.extend(new);
            }
            match self.set_doc_items(name, &items) {
                Ok(()) => applied += changes.len(),
                Err(e) => log::warn!("iCloud: {name}: {e}"),
            }
        }
        // what was found in photos: into this device's indexes
        for (_, _, key, data) in take.iter().filter(|t| t.1 == analysis::ANALYSIS) {
            if let Some(v) = data.as_deref().and_then(|d| serde_json::from_str::<Value>(d).ok())
                && self.analysis_take(key, &v) != 0
            {
                applied += 1;
                ic.analysis_seen = None;
            }
        }
        // albums and stacks
        let coll: Vec<_> = take.iter().filter(|t| collections::is_collection(&t.1)).collect();
        if !coll.is_empty() {
            applied += collections::apply(self, ic, &coll);
        }
        // servers and edits
        for (name, kind, key, data) in take.iter().filter(|t| t.1 == SERVER || t.1 == EDIT) {
            let Some(data) = data else {
                if ic.held.remove(name).is_some() {
                    ic.held_dirty = true;
                    ic.ours.remove(name);
                    applied += 1;
                }
                // a photo kept in iCloud Drive that another device removed for good goes here too (it is the same file);
                // a Photos-library picture stays (the Photos library still has it)
                if kind == EDIT
                    && let Some(folder) = ic.folder.clone()
                {
                    let gone = photos::in_folder_with(self, &folder, key);
                    if !gone.is_empty() {
                        let op = self.catalog.delete_photos_permanently_ops(&gone);
                        match self.apply_system(op) {
                            Ok(()) => applied += gone.len(),
                            Err(e) => log::warn!("iCloud: removing {key}: {e}"),
                        }
                        ic.dirty.extend(gone);
                    }
                }
                continue;
            };
            let ids = if kind == EDIT { self.traveling_with(key, ic.folder.as_deref()) } else { Vec::new() };
            if !ids.is_empty() {
                for id in ids {
                    match self.icloud_apply_edit(id, data) {
                        Ok(()) => applied += 1,
                        Err(e) => log::warn!("iCloud: photo {}: {e}", id.0),
                    }
                    ic.dirty.insert(id);
                }
                if ic.held.remove(name).is_some() {
                    ic.held_dirty = true;
                }
                continue;
            }
            if ic.held.len() >= HELD_MAX && !ic.held.contains_key(name) {
                log::warn!("iCloud: more than {HELD_MAX} records held: `{name}` left out");
                continue;
            }
            ic.held.insert(name.clone(), Held { kind: kind.clone(), key: key.clone(), data: data.clone(), hash: hash_of(data) });
            ic.held_dirty = true;
            applied += 1;
        }
        applied
    }

    /// Develop presets from the cloud: each replaces the one with its id, or is removed.
    fn icloud_take_presets(&mut self, changes: &[&(String, String, String, Option<String>)]) -> usize {
        let builtin: HashSet<String> = self.presets.iter().filter(|p| p.builtin).map(|p| p.id.clone()).collect();
        let mut users: Vec<lightcraft_develop::Preset> = self.presets.iter().filter(|p| !p.builtin).cloned().collect();
        let mut applied = 0;
        for (_, _, key, data) in changes {
            if key.len() > PRESET_ID_MAX || builtin.contains(key.as_str()) || key.starts_with(crate::presets::SERVER_PREFIX) {
                continue;
            }
            let incoming = data
                .as_deref()
                .and_then(|d| serde_json::from_str::<lightcraft_develop::Preset>(d).ok())
                .filter(|p| p.id == *key && !p.name.trim().is_empty() && p.settings.is_object());
            // (a preset this version can't read leaves this device's as it is)
            if data.is_some() && incoming.is_none() {
                continue;
            }
            match (users.iter().position(|p| p.id == *key), incoming) {
                (Some(i), Some(p)) => {
                    if let Some(slot) = users.get_mut(i) {
                        *slot = lightcraft_develop::Preset { builtin: false, ..p };
                    }
                }
                (None, Some(p)) if users.len() < PRESETS_MAX => users.push(lightcraft_develop::Preset { builtin: false, ..p }),
                (Some(i), None) => {
                    users.remove(i);
                }
                _ => continue,
            }
            applied += 1;
        }
        if applied > 0 {
            self.presets.retain(|p| p.builtin);
            self.presets.extend(users);
        }
        applied
    }

    /// The photos whose picture is `key` and that travel: photo-library photos (still Local ones included) and those
    /// whose original is in the iCloud Drive folder `folder`.
    fn traveling_with(&self, key: &str, folder: Option<&Path>) -> Vec<PhotoId> {
        let same = |p: &Photo| p.copy_of.is_none() && lightcraft_catalog::sync::content_key(p).as_deref() == Some(key);
        let mut ids: Vec<PhotoId> = self.photo_library.photo_ids().filter(|id| self.catalog.photo(*id).is_some_and(|p| same(p))).collect();
        if let Some(folder) = folder {
            for id in photos::in_folder_with(self, folder, key) {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
        ids
    }

    /// Apply an edit record's data to photo `id`: what its XMP says (a History step), bringing a photo only looked
    /// at into the library. Not an undo step.
    fn icloud_apply_edit(&mut self, id: PhotoId, data: &str) -> Result<()> {
        let v: Value = serde_json::from_str(data).map_err(|e| EngineError::Other(format!("not an edit: {e}")))?;
        let xmp = v.get("xmp").and_then(Value::as_str).ok_or_else(|| EngineError::Other("not an edit: no XMP".into()))?;
        let p = self.catalog.photo(id).ok_or(lightcraft_catalog::CatalogError::NoPhoto(id))?;
        let sc = crate::sidecar::parse_sidecar(xmp, crate::crs::Target::for_photo(p)).map_err(EngineError::Other)?.resolve_label(&self.catalog);
        let mut ops = crate::sidecar::sidecar_ops(p, &sc, &(self.clock)(), LABEL);
        // (Recently Deleted and Hidden: what the record says, absent = not)
        let flag = |k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
        if flag("deleted") != p.deleted {
            ops.push(Op::SetDeleted { id, deleted: flag("deleted") });
        }
        if flag("hidden") != p.hidden {
            ops.push(Op::SetHidden { id, hidden: flag("hidden") });
        }
        // (a record from a version that didn't send them leaves this device's as they are)
        if let Some(versions) = v.get("versions").and_then(|x| serde_json::from_value::<Vec<lightcraft_catalog::Version>>(x.clone()).ok())
            && versions != p.versions
        {
            ops.push(Op::SetVersions { id, versions });
        }
        if ops.is_empty() {
            return Ok(());
        }
        if p.local {
            ops.insert(0, Op::SetLocal { id, local: false });
        }
        let promote = p.local;
        self.apply_system(crate::sidecar::with_history(ops))?;
        if promote {
            self.save_photo_library();
        }
        Ok(())
    }

    /// A photo just opened from the photo library: the edit another device made to its picture, if one is held,
    /// is applied (and brings it into the library). Answers whether one was.
    pub(crate) fn icloud_take_held(&mut self, id: PhotoId) -> bool {
        if self.server_linked() {
            return false;
        }
        let Some(key) = self.catalog.photo(id).and_then(|p| lightcraft_catalog::sync::content_key(p)) else { return false };
        let name = record_name(EDIT, &key);
        self.with_icloud(|s, ic| {
            let Some(h) = ic.held.get(&name).cloned() else { return false };
            match s.icloud_apply_edit(id, &h.data) {
                Ok(()) => {
                    ic.held.remove(&name);
                    ic.held_dirty = true;
                    ic.dirty.insert(id);
                    ic.rev = ic.rev.wrapping_add(1);
                    // this device's rendering of the cloud's version: not sent back
                    s.icloud_refresh_photo(ic, id);
                    let mine = ic.ours.get(&name).map(|m| m.hash.clone());
                    if let Some(k) = ic.saved.known.get_mut(&name) {
                        k.local = mine.filter(|m| *m != k.hash);
                        ic.saved_dirty = true;
                    }
                    true
                }
                Err(e) => {
                    log::warn!("iCloud: photo {}: {e}", id.0);
                    false
                }
            }
        })
        .unwrap_or(false)
    }

    /// Changes whenever this device forgot what the cloud has: the host then starts over and fetches everything
    /// (`None` while sync is off).
    pub fn icloud_epoch(&self) -> Option<u64> {
        self.icloud.as_ref().map(|ic| ic.saved.epoch)
    }

    /// Keep the host's own sync state with the library's (the host reads it back from `icloud.status`).
    pub fn icloud_set_transport(&mut self, state: Option<String>) -> Result<()> {
        if state.as_ref().is_some_and(|s| s.len() > TRANSPORT_MAX) {
            return Err(EngineError::Other(format!("the host's iCloud state is over {} MiB", TRANSPORT_MAX >> 20)));
        }
        if let Some(ic) = self.icloud.as_mut()
            && ic.saved.transport != state
        {
            ic.saved.transport = state;
            ic.saved_dirty = true;
        }
        Ok(())
    }

    /// What `icloud.status` reports.
    pub fn icloud_status(&mut self, transport: bool) -> Value {
        let linked = self.server_linked();
        let rev = self.icloud_rev();
        let changes = self.icloud_on().then(|| self.icloud_changes());
        let Some(ic) = self.icloud.as_ref() else { return json!({"on": false}) };
        let servers: Vec<Value> = ic
            .held
            .values()
            .filter(|h| h.kind == SERVER)
            .filter_map(|h| serde_json::from_str::<Value>(&h.data).ok())
            .filter(|v| v["server"].is_string() && v["user"].is_string())
            .map(|v| json!({"server": v["server"], "user": v["user"]}))
            .collect();
        let count = |kind: &dyn Fn(&str) -> bool| ic.ours.values().filter(|m| kind(&m.kind)).count();
        let changes = changes.unwrap_or_default();
        let mut out = json!({
            "on": true,
            "epoch": ic.saved.epoch,
            "rev": rev,
            "serverLinked": linked,
            "pending": changes.save.len() + changes.delete.len(),
            "presets": count(&|k| k == PRESETS),
            "settings": count(&|k| k != PRESETS && k != SERVER && k != EDIT),
            "edits": ic.photo_of.len(),
            "heldEdits": ic.held.values().filter(|h| h.kind == EDIT).count(),
            "servers": servers,
            "known": ic.saved.known.len(),
            "photos": ic.saved.photos,
            "folder": ic.folder.is_some(),
            "analysis": ic.ours.values().filter(|m| m.kind == analysis::ANALYSIS).count(),
        });
        // Keep Photos in iCloud Drive: how many are there, and how many could move there
        if let Some(folder) = ic.folder.clone()
            && let Some(o) = out.as_object_mut()
        {
            o.insert("inFolder".into(), json!(photos::in_folder(self, &folder).len()));
            let (movable, bytes) = self.icloud_movable_count();
            o.insert("movable".into(), json!({"photos": movable, "bytes": bytes}));
        }
        if let Some(f) = Session::folder_status(ic)
            && let Some(o) = out.as_object_mut()
        {
            o.insert("cloudFolder".into(), f);
        }
        if transport && let Some(o) = out.as_object_mut() {
            o.insert("transport".into(), json!(ic.saved.transport));
        }
        out
    }

    /// The photo-library assets (their cross-device ids, as the app gave them) of the edits held for pictures this
    /// device hasn't opened yet.
    pub fn icloud_held_assets(&self) -> Vec<String> {
        let Some(ic) = self.icloud.as_ref() else { return Vec::new() };
        ic.held
            .values()
            .filter(|h| h.kind == EDIT)
            .filter_map(|h| serde_json::from_str::<Value>(&h.data).ok()?.get("asset")?.as_str().map(str::to_string))
            .collect()
    }

    /// The change times come from `clock` (tests: one device's clock ahead of another's).
    pub fn icloud_set_clock(&mut self, clock: Box<dyn Fn() -> i64 + Send>) {
        if let Some(ic) = self.icloud.as_mut() {
            ic.set_clock(clock);
        }
    }
}

// ---------------------------------------------------------------- commands

fn names_param(p: &Value, c: &str) -> Result<Vec<String>> {
    let a = p.get("names").and_then(Value::as_array).ok_or_else(|| bad(c, "missing `names`"))?;
    Ok(a.iter().take(BATCH_MAX).filter_map(Value::as_str).filter(|n| n.len() <= NAME_MAX).map(str::to_string).collect())
}

fn list_param<T: serde::de::DeserializeOwned>(p: &Value, key: &str, c: &str) -> Result<Vec<T>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(a)) => {
            a.iter().take(BATCH_MAX).map(|v| serde_json::from_value(v.clone()).map_err(|e| bad(c, format!("`{key}`: {e}")))).collect()
        }
        Some(_) => Err(bad(c, format!("`{key}` must be a list"))),
    }
}

fn on(s: &Session) -> std::result::Result<(), String> {
    if s.icloud_on() { Ok(()) } else { Err("iCloud sync is off for this library".into()) }
}

/// On, and no slider drag in progress (what comes from the cloud waits until it ends, as a server's changes do).
fn on_and_idle(s: &Session) -> std::result::Result<(), String> {
    on(s)?;
    if s.interaction.is_some() { Err("a slider drag is in progress: try again when it ends".into()) } else { Ok(()) }
}

fn status(s: &mut Session, p: &Value) -> Result<Value> {
    Ok(s.icloud_status(p.get("transport").and_then(Value::as_bool).unwrap_or(false)))
}

fn enable(s: &mut Session, p: &Value) -> Result<Value> {
    let want = p.get("on").and_then(Value::as_bool).ok_or_else(|| bad("icloud.enable", "missing `on`"))?;
    s.icloud_enable(want)?;
    Ok(s.icloud_status(false))
}

fn changes(s: &mut Session, _: &Value) -> Result<Value> {
    let c = s.icloud_changes();
    Ok(json!({"save": c.save, "delete": c.delete, "rev": s.icloud_rev(), "epoch": s.icloud_epoch()}))
}

fn records(s: &mut Session, p: &Value) -> Result<Value> {
    let names = names_param(p, "icloud.records")?;
    Ok(json!({"records": s.icloud_records(&names)}))
}

fn fetched(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "icloud.fetched";
    let records: Vec<Incoming> = list_param(p, "records", C)?;
    let deleted: Vec<String> = list_param(p, "deleted", C)?;
    let changed = s.icloud_fetched(&records, &deleted);
    Ok(json!({"changed": changed}))
}

fn sent(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "icloud.sent";
    let saved: Vec<Sent> = list_param(p, "saved", C)?;
    let deleted: Vec<String> = list_param(p, "deleted", C)?;
    s.icloud_sent(&saved, &deleted);
    Ok(json!({}))
}

fn transport(s: &mut Session, p: &Value) -> Result<Value> {
    let state = match p.get("state") {
        None | Some(Value::Null) => None,
        Some(Value::String(t)) => Some(t.clone()),
        Some(_) => return Err(bad("icloud.transport", "`state` must be a string or null")),
    };
    s.icloud_set_transport(state)?;
    Ok(json!({}))
}

fn reset(s: &mut Session, _: &Value) -> Result<Value> {
    s.icloud_reset();
    Ok(s.icloud_status(false))
}

fn held_assets(s: &mut Session, _: &Value) -> Result<Value> {
    Ok(json!({"assets": s.icloud_held_assets()}))
}

fn folder(s: &mut Session, p: &Value) -> Result<Value> {
    let folder = match p.get("path") {
        None | Some(Value::Null) => None,
        Some(Value::String(t)) if !t.is_empty() => Some(PathBuf::from(t)),
        Some(_) => return Err(bad("icloud.folder", "`path` must be a folder's absolute path or null")),
    };
    s.icloud_set_folder(folder)?;
    Ok(s.icloud_status(false))
}

fn photos_on(s: &mut Session, p: &Value) -> Result<Value> {
    let want = p.get("on").and_then(Value::as_bool).ok_or_else(|| bad("icloud.photos", "missing `on`"))?;
    s.icloud_set_photos(want)?;
    Ok(s.icloud_status(false))
}

fn wanted(s: &mut Session, p: &Value) -> Result<Value> {
    let limit = p.get("limit").and_then(Value::as_u64).map_or(200, |n| usize::try_from(n).unwrap_or(usize::MAX)).clamp(1, BATCH_MAX);
    let waiting = p.get("waiting").and_then(Value::as_bool).unwrap_or(true);
    Ok(s.icloud_wanted(limit, waiting))
}

fn adopt(s: &mut Session, p: &Value) -> Result<Value> {
    let files: Vec<String> = list_param(p, "files", "icloud.adopt")?;
    s.icloud_adopt(&files)
}

fn move_in(s: &mut Session, p: &Value) -> Result<Value> {
    let limit = p.get("limit").and_then(Value::as_u64).map_or(16, |n| usize::try_from(n).unwrap_or(usize::MAX));
    s.icloud_move_in(limit)
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        // (none journaled: they move this device's sync state, which replaying would undo)
        cmd!(query "icloud.status", "iCloud Sync Status", [], None,
            "{transport?: bool}: {on, epoch (changes when this device forgot what the cloud has: fetch everything again), rev, serverLinked (a server keeps the presets, settings and edits: only the server travels through iCloud), pending (records to send), presets, settings, edits (photos sent: from the Photos library or kept in iCloud Drive), heldEdits (edits of pictures not here yet), analysis (photos whose faces, text… are sent), servers ([{server, user}] other devices are signed in to), known, photos (Keep Photos in iCloud Drive), folder (the host gave its iCloud Drive folder), inFolder (photos kept there), movable {photos, bytes} (could move there)} or {on: false}; `transport`: also the host's own state as it gave it",
            always, status),
        cmd!(query "icloud.enable", "Sync with iCloud", [], None,
            "{on: bool}: sync this library's presets, settings, sync server and photo-library edits with the person's other devices through the host's iCloud (docs/sync.md → iCloud). Off forgets what the cloud has (it keeps it, and this device keeps its own); on again merges as a new device does. Answers icloud.status",
            always, enable),
        cmd!(query "icloud.changes", "iCloud Changes", [], None,
            "{}: {save: [name], delete: [name], rev, epoch}: the records that changed here since the cloud last agreed",
            on, changes),
        cmd!(query "icloud.records", "iCloud Records", [], None,
            "{names: [name]}: {records: [{name, kind, key, data, hash, modified} | {name, gone: true}]}: what to save, as it is now",
            on, records),
        cmd!(query "icloud.fetched", "iCloud Fetched", [], None,
            "{records?: [{name, kind, key, data, modified}], deleted?: [name]}: what the cloud has (or its version of a record a save was refused over) and what it deleted, merged here; {changed}. Disabled during a slider drag: hand them over again when it ends",
            on_and_idle, fetched),
        cmd!(query "icloud.sent", "iCloud Sent", [], None,
            "{saved?: [{name, kind, key, hash}], deleted?: [name]}: what the cloud took",
            on, sent),
        cmd!(query "icloud.transport", "iCloud Host State", [], None,
            "{state: string | null}: the host's own sync state, kept with the library's (icloud.status {transport: true} gives it back)",
            on, transport),
        cmd!(query "icloud.reset", "Forget iCloud State", [], None,
            "{}: the iCloud account changed or its data was deleted: forget what the cloud had, so the next sync merges as a new device (nothing is removed)",
            on, reset),
        cmd!(query "icloud.heldAssets", "Photo Library Photos Edited Elsewhere", [], None,
            "{}: {assets: [cloud id]}: the photo-library assets other devices edited that haven't been opened here",
            always, held_assets),
        cmd!(query "icloud.folder", "iCloud Drive Folder", [], None,
            "{path: string | null}: this device's iCloud Drive folder for photos (the host's; null: iCloud Drive isn't available). Answers icloud.status",
            on, folder),
        cmd!(query "icloud.photos", "Keep Photos in iCloud Drive", [], None,
            "{on: bool}: imports put their originals in the iCloud Drive folder, and photos other devices keep there come into this library (icloud.wanted, icloud.adopt). Needs icloud.folder and no sync server. Answers icloud.status",
            on, photos_on),
        cmd!(query "icloud.wanted", "iCloud Drive Files Wanted", [], None,
            "{limit?, waiting?: bool}: {files: [{file, key?, id?}], total}: the files of the iCloud Drive folder this device needs (relative paths): photos other devices keep there that this library doesn't have, and this library's own whose file iCloud Drive took back. Bring each down, then icloud.adopt. An activity row shows while some are wanted; `waiting: false` (the host stopped waiting for them) takes it away",
            on, wanted),
        cmd!(query "icloud.adopt", "Add Photos from iCloud Drive", [], None,
            "{files: [relative path]}: files of the iCloud Drive folder that are on this device now (16 at most a call): another device's photo is added to the library where it is, with that device's edits (not an undo step); one of this library's is shown from its file again. {added, back, missing, refused, left}",
            on, adopt),
        cmd!(query "icloud.moveIn", "Move Photos to iCloud Drive", [], None,
            "{limit?}: move up to `limit` (16; 64 at most) of the originals this library keeps itself into the iCloud Drive folder, in the same folders, each with its sidecars (not an undo step). {moved, failed: [{file, error}], left, bytesLeft}; call again while `left`",
            on, move_in),
    ]
}
