//! Library folders: a user's photo folders on the server, read in place.
//!
//! An admin gives a user folders that are already on the server — a NAS share, a backup disk,
//! years of `2019/Holidays/…` — and they stay exactly where they are (`lightcraft-server folder
//! add ann /photos/ann`, or the admin page). The server walks them, adds each photo it finds to
//! the user's library, so every device sees it in its folder ([`Photo::server_path`]) and can put
//! it in albums, builds its smart and mini previews, and serves its original straight from the
//! folder when a device asks for it. Nothing in the folders is ever written, moved or deleted:
//! mount them read-only.
//!
//! - **The library is the reference** for which file is which photo (`server_path`); the index
//!   (`users/<name>/folders.json`) only remembers each file's size, time and content hash, so a
//!   file that didn't change isn't read again, and which photo it became.
//! - **Scans** run at start, every [`Scanner::interval`], and on demand (the admin page,
//!   `lightcraft-server scan`). A new file becomes a photo, with what its XMP sidecar (or the
//!   raw's own XMP) says: rating, label, keywords, title, Lightroom edits. A file that moved keeps
//!   its photo (same content, its old place gone); one changed in place keeps its photo and edits
//!   and gets new previews; a file a device uploaded already becomes that photo's place.
//! - **Nothing is taken away by a scan.** A file that disappears keeps its photo (its original
//!   can't be downloaded until it's back); a folder that is missing, or empty when it had photos
//!   (an unmounted disk), is skipped, never read as everything deleted. A photo removed from the
//!   library on a device isn't added back, unless its file changes.
//! - **Untrusted files**: decoding runs under a panic guard, files larger than [`FILE_MAX`] are
//!   skipped, symbolic links aren't followed, depth is bounded.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant, UNIX_EPOCH};

use lightcraft_engine::catalog::sync::original_path;
use lightcraft_engine::catalog::{MediaKind, Op, Photo, PhotoId, Source};
use lightcraft_engine::sidecar::{self, SidecarData, SidecarNaming};
use serde::{Deserialize, Serialize};

use crate::State;
use crate::accounts::{self, LibraryFolder};
use crate::api::UserLib;

/// The index of a user's library folders, in their folder.
pub const INDEX: &str = "folders.json";
/// Left in a user's folder by `lightcraft-server scan` while the server runs: scan them soon.
pub const REQUEST: &str = "scan.request";
/// The id space of the photos the server adds (no device is given it).
pub const SPACE: u32 = crate::MAX_SPACE;
/// Larger files aren't read as photos.
pub const FILE_MAX: u64 = 2 << 30;
/// Files up to this size are read once for their hash and description; larger ones are hashed
/// as they stream and read again only when they're new.
const READ_WHOLE: u64 = 256 << 20;
/// Folder depth followed at most, and photo files listed per user at most.
const DEPTH_MAX: usize = 64;
const FILES_MAX: usize = 5_000_000;
/// Files read before their changes go to the library.
const BATCH: usize = 100;
const BATCH_TIME: Duration = Duration::from_secs(10);
/// How often the index is saved during a long scan.
const SAVE_EVERY: Duration = Duration::from_secs(30);
/// Folders never descended into: NAS bookkeeping and recycle bins (and every hidden one).
const SKIP_DIRS: [&str; 6] = ["@eaDir", "#recycle", "#snapshot", "$RECYCLE.BIN", "lost+found", "System Volume Information"];

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What the index knows about one photo file.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Entry {
    pub size: u64,
    /// Modification time, nanoseconds since 1970.
    pub mtime: u64,
    /// Content hash (empty when the file couldn't be read).
    pub hash: String,
    /// The photo this file is (or was, until it was removed from the library).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub photo: Option<u64>,
    /// Not found by the last scan (its folder was there).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub missing: bool,
    /// Why it isn't a photo or has no previews (tried again when the file changes).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// A user's library folders as last scanned.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Index {
    /// By path: the library folder's name, then the path inside it (`Photos/2024/a.jpg`).
    pub files: BTreeMap<String, Entry>,
    /// Where each library folder is, by name.
    pub roots: BTreeMap<String, String>,
    #[serde(skip)]
    by_hash: HashMap<String, Vec<String>>,
}

impl Index {
    /// The index in a user's folder (a damaged one reads as empty: it is only a cache, the next
    /// scan reads every file again and finds each one's photo by its place).
    pub fn load(user_dir: &Path) -> Index {
        let path = user_dir.join(INDEX);
        let mut ix: Index = match std::fs::read(&path) {
            Ok(b) => serde_json::from_slice(&b).unwrap_or_else(|e| {
                log::error!("{} is damaged ({e}): the next scan reads every file again", path.display());
                Index::default()
            }),
            Err(_) => Index::default(),
        };
        ix.reindex();
        ix
    }

    pub fn save(&self, user_dir: &Path) -> Result<(), String> {
        let bytes = serde_json::to_vec(self).map_err(|e| e.to_string())?;
        let path = user_dir.join(INDEX);
        lightcraft_engine::catalog::safe_file::write_atomic(&path, &bytes).map_err(|e| format!("{}: {e}", path.display()))
    }

    fn reindex(&mut self) {
        self.by_hash.clear();
        for (k, e) in &self.files {
            if !e.missing && !e.hash.is_empty() {
                self.by_hash.entry(e.hash.clone()).or_default().push(k.clone());
            }
        }
    }

    /// Where file `key` is on this server.
    pub fn path_of(&self, key: &str) -> Option<PathBuf> {
        let (root, rest) = key.split_once('/')?;
        let mut p = PathBuf::from(self.roots.get(root)?);
        for c in rest.split('/') {
            if c.is_empty() || c == "." || c == ".." {
                return None;
            }
            p.push(c);
        }
        Some(p)
    }

    /// The files holding content `hash`, with the size and time they were indexed with.
    pub fn originals(&self, hash: &str) -> Vec<(PathBuf, u64, u64)> {
        let keys = self.by_hash.get(hash).map(Vec::as_slice).unwrap_or(&[]);
        keys.iter().filter_map(|k| Some((self.path_of(k)?, self.files.get(k)?))).map(|(p, e)| (p, e.size, e.mtime)).collect()
    }

    /// Has a photo file with content `hash` (the server needs no upload of it).
    pub fn has(&self, hash: &str) -> bool {
        self.by_hash.contains_key(hash)
    }
}

/// Nanoseconds since 1970 of a file's modification time (0 when unknown).
fn mtime_of(m: &std::fs::Metadata) -> u64 {
    m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
}

/// Is `path` still the file the index saw (same size and time, a file, not a link)?
pub fn unchanged(path: &Path, size: u64, mtime: u64) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file() && m.len() == size && mtime_of(&m) == mtime)
}

/// What a scan did, and is doing (the admin page shows it).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub scanning: bool,
    /// Scans finished since the server started.
    pub scans: u64,
    /// Files read so far / to read in this scan.
    pub done: usize,
    pub todo: usize,
    /// Photo files in the folders.
    pub files: usize,
    /// Of the files read: new photos, moved ones, changed ones, files a device had uploaded,
    /// files that aren't photos LightCraft reads (or couldn't be read).
    pub added: usize,
    pub moved: usize,
    pub changed: usize,
    pub linked: usize,
    pub failed: usize,
    /// Indexed files not found any more.
    pub missing: usize,
    /// Previews waiting to be built, and the ones that couldn't be.
    pub previews: usize,
    pub preview_errors: Vec<String>,
    /// When the last scan ended (unix seconds).
    pub last_scan: Option<u64>,
    /// Folders that couldn't be read, and the first few files that couldn't.
    pub errors: Vec<String>,
}

/// A file found in a library folder.
#[derive(Clone, Debug)]
struct Found {
    key: String,
    path: PathBuf,
    size: u64,
    mtime: u64,
}

/// List the photo files under `root` (named `name`), without following links.
fn walk(name: &str, root: &Path, out: &mut Vec<Found>, errors: &mut Vec<String>, stop: &AtomicBool) {
    let mut stack = vec![(root.to_path_buf(), name.to_string(), 0usize)];
    while let Some((dir, key, depth)) = stack.pop() {
        if stop.load(Ordering::Relaxed) || out.len() >= FILES_MAX {
            return;
        }
        let rd = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) => {
                errors.push(format!("{}: {e}", dir.display()));
                continue;
            }
        };
        for e in rd.flatten() {
            // (a name that isn't UTF-8 can't be shown or found again by its path: skipped)
            let Some(n) = e.file_name().to_str().map(str::to_string) else { continue };
            if n.starts_with('.') || SKIP_DIRS.contains(&n.as_str()) {
                continue;
            }
            let Ok(ft) = e.file_type() else { continue };
            let k = format!("{key}/{n}");
            if ft.is_dir() {
                if depth < DEPTH_MAX {
                    stack.push((e.path(), k, depth + 1));
                }
            } else if ft.is_file()
                && lightcraft_engine::import::is_supported(Path::new(&n))
                && let Ok(m) = e.metadata()
            {
                out.push(Found { key: k, path: e.path(), size: m.len(), mtime: mtime_of(&m) });
            }
        }
    }
}

/// A file read: its hash, and its description when it is new to the library.
struct FileRead {
    file: Found,
    hash: Result<String, String>,
    probed: Option<Result<(Photo, Option<SidecarData>), String>>,
}

/// Stream a file through the content hash.
fn hash_file(path: &Path) -> Result<String, String> {
    let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut h = lightcraft_preview::Hasher128::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        match f.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                h.update(buf.get(..n).unwrap_or(&[]));
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(h.finish().to_string())
}

/// A photo for a new file (its id is given when it goes into the library).
fn describe(file: &Found, hash: &str, bytes: &[u8], now: &str) -> Result<(Photo, Option<SidecarData>), String> {
    let name = file.key.rsplit('/').next().unwrap_or(&file.key).to_string();
    let info = lightcraft_engine::guard::catch("reading a photo", || lightcraft_engine::files::probe_bytes(&name, bytes))??;
    let mut p = Photo::new(PhotoId(0), Source::File { path: original_path(hash, &name) }, &name, &info.format, info.width, info.height, now);
    p.kind = info.kind;
    p.file_size = info.file_size;
    p.captured = info.captured;
    p.meta = info.meta;
    p.as_shot_wb = info.as_shot_wb;
    p.content_hash = Some(hash.to_string());
    p.embedded_lens = info.embedded_lens;
    p.preview_only = info.preview_only;
    p.server_path = Some(file.key.clone());
    p.develop = Arc::new(p.camera_defaults());
    // the XMP sidecar (or a raw's own XMP): ratings, keywords, Lightroom edits
    let raw = p.kind == MediaKind::Raw;
    let packet = sidecar::find_sidecar(&file.path.to_string_lossy(), SidecarNaming::Stem)
        .and_then(|f| std::fs::read_to_string(f).ok())
        .or_else(|| info.xmp.filter(|_| raw));
    let sc = packet.and_then(|x| sidecar::parse_sidecar(&x, raw).map_err(|e| log::warn!("{}: XMP: {e}", file.key)).ok());
    Ok((p, sc))
}

/// What the library knows, to tell which files need describing (refreshed every batch).
#[derive(Default)]
struct Known {
    /// The places of the library's photos by content hash (`None`: not in a library folder).
    by_hash: HashMap<String, Vec<Option<String>>>,
    /// Each library-folder photo's content hash, by its place.
    places: HashMap<String, String>,
}

impl Known {
    fn of(l: &UserLib) -> Known {
        let mut k = Known::default();
        for p in l.core.catalog().photos().filter(|p| p.copy_of.is_none()) {
            if let Some(h) = &p.content_hash {
                k.by_hash.entry(h.clone()).or_default().push(p.server_path.clone());
                if let Some(sp) = &p.server_path {
                    k.places.insert(sp.clone(), h.clone());
                }
            }
        }
        k
    }

    /// Does a file with this content at this place need describing: new content for its photo,
    /// or a new photo (no photo with this content is free to take the place: every one is in a
    /// place that is still there, `found`)?
    fn needs_description(&self, key: &str, hash: &str, found: &HashSet<String>) -> bool {
        match self.places.get(key) {
            Some(h) => h != hash,
            None => !self.by_hash.get(hash).is_some_and(|places| places.iter().any(|p| p.as_ref().is_none_or(|p| !found.contains(p)))),
        }
    }
}

fn read(file: Found, known: &Known, found: &HashSet<String>, now: &str) -> FileRead {
    if file.size > FILE_MAX {
        let why = format!("larger than {} GiB", FILE_MAX >> 30);
        return FileRead { file, hash: Err(why), probed: None };
    }
    let whole = if file.size <= READ_WHOLE { Some(std::fs::read(&file.path)) } else { None };
    let (hash, bytes) = match whole {
        Some(Ok(b)) => (Ok(lightcraft_preview::hash_bytes(&b).to_string()), Some(b)),
        Some(Err(e)) => (Err(e.to_string()), None),
        None => (hash_file(&file.path), None),
    };
    let probed = match &hash {
        Ok(h) if known.needs_description(&file.key, h, found) => {
            let bytes = match bytes {
                Some(b) => Ok(b),
                None => std::fs::read(&file.path).map_err(|e| e.to_string()),
            };
            Some(bytes.and_then(|b| describe(&file, h, &b, now)))
        }
        _ => None,
    };
    FileRead { file, hash, probed }
}

/// What a file read means for the library.
enum Plan {
    /// Its photo is in the library already (nothing to change).
    Same(PhotoId),
    /// Its photo, whose content changed.
    Changed(PhotoId),
    /// Removed from the library on a device: stays removed.
    Removed(u64),
    /// A photo already in the library (uploaded by a device, or moved here from `from`): its
    /// place.
    Place {
        id: PhotoId,
        from: Option<String>,
    },
    /// A new photo.
    New,
    /// Not described yet (the library changed meanwhile): the next scan.
    Later,
    Failed(String),
}

/// Apply a batch of files read to the library, under its lock: the ops in one push, then their
/// index entries. Returns the files whose previews may be missing.
fn commit(lib: &Mutex<UserLib>, batch: &mut Vec<FileRead>, found: &HashSet<String>, s: &mut Status, now: &str) -> Vec<(String, PathBuf)> {
    let mut l = lock(lib);
    let plans: Vec<Plan> = {
        let c = l.core.catalog();
        let mut by_place: HashMap<&str, &Photo> = HashMap::new();
        let mut by_hash: HashMap<&str, Vec<&Photo>> = HashMap::new();
        for p in c.photos().filter(|p| p.copy_of.is_none()) {
            if let Some(sp) = &p.server_path {
                by_place.insert(sp, p);
            }
            if let Some(h) = &p.content_hash {
                by_hash.entry(h).or_default().push(p);
            }
        }
        let mut claimed: HashSet<PhotoId> = HashSet::new();
        batch
            .iter()
            .map(|r| {
                let hash = match &r.hash {
                    Ok(h) => h.as_str(),
                    Err(e) => return Plan::Failed(e.clone()),
                };
                let key = r.file.key.as_str();
                let old = l.index.files.get(key);
                if let Some(p) = by_place.get(key) {
                    return if p.content_hash.as_deref() == Some(hash) { Plan::Same(p.id) } else { Plan::Changed(p.id) };
                }
                if let Some(id) = old.filter(|o| o.hash == hash).and_then(|o| o.photo).filter(|id| c.photo(PhotoId(*id)).is_none()) {
                    return Plan::Removed(id);
                }
                // a device's upload of this content, or this content's photo whose place is gone
                let free = by_hash
                    .get(hash)
                    .and_then(|ps| ps.iter().find(|p| !claimed.contains(&p.id) && p.server_path.as_deref().is_none_or(|sp| !found.contains(sp))));
                if let Some(p) = free {
                    claimed.insert(p.id);
                    return Plan::Place { id: p.id, from: p.server_path.clone() };
                }
                Plan::New
            })
            .collect()
    };
    let mut ops: Vec<(Op, usize)> = Vec::new();
    let mut entries: Vec<Option<Entry>> = Vec::with_capacity(batch.len());
    let mut kinds = Vec::new();
    let mut moved_from: HashMap<usize, String> = HashMap::new();
    let mut removed: HashSet<usize> = HashSet::new();
    for (i, (r, plan)) in batch.iter_mut().zip(plans).enumerate() {
        let mut e =
            Entry { size: r.file.size, mtime: r.file.mtime, hash: r.hash.clone().unwrap_or_default(), photo: None, missing: false, error: None };
        let probed = r.probed.take();
        let plan = match (plan, &probed) {
            (Plan::Changed(_) | Plan::New, None) => Plan::Later,
            (Plan::Changed(_) | Plan::New, Some(Err(why))) => Plan::Failed(why.clone()),
            (p, _) => p,
        };
        match plan {
            Plan::Same(id) => e.photo = Some(id.0),
            Plan::Removed(id) => {
                e.photo = Some(id);
                removed.insert(i);
            }
            Plan::Changed(id) => {
                if let Some(Ok((p, _))) = &probed {
                    let op = Op::SetContent {
                        id,
                        width: p.width,
                        height: p.height,
                        file_size: p.file_size,
                        content_hash: p.content_hash.clone(),
                        preview_only: p.preview_only.clone(),
                    };
                    ops.push((op, i));
                    kinds.push((i, "changed"));
                }
                e.photo = Some(id.0);
            }
            Plan::Place { id, from } => {
                ops.push((Op::SetServerPath { id, path: Some(r.file.key.clone()) }, i));
                kinds.push((i, if from.is_some() { "moved" } else { "linked" }));
                moved_from.extend(from.map(|f| (i, f)));
                e.photo = Some(id.0);
            }
            Plan::New => {
                if let Some(Ok((mut p, sc))) = probed {
                    p.id = l.core.alloc_photo_id(SPACE);
                    if let Some(sc) = sc {
                        let sc = sc.resolve_label(l.core.catalog());
                        sidecar::merge_into(&mut p, &sc, now);
                    }
                    e.photo = Some(p.id.0);
                    ops.push((Op::AddPhoto { photo: Box::new(p) }, i));
                    kinds.push((i, "added"));
                }
            }
            Plan::Later => {
                entries.push(None);
                continue;
            }
            Plan::Failed(why) => {
                s.failed += 1;
                if s.errors.len() < 20 {
                    s.errors.push(format!("{}: {why}", r.file.key));
                }
                e.error = Some(why);
            }
        }
        entries.push(Some(e));
    }
    // one push; an op the library refuses is left out (its file is read again next scan)
    let mut refused: HashSet<usize> = HashSet::new();
    loop {
        let list: Vec<Op> = ops.iter().filter(|(_, i)| !refused.contains(i)).map(|(op, _)| op.clone()).collect();
        if list.is_empty() {
            break;
        }
        let head = l.core.head();
        match l.core.push(head, &list) {
            Ok(_) => break,
            Err(lightcraft_engine::catalog::sync::PushError::Rejected { index, error }) => {
                let Some((_, i)) = ops.iter().filter(|(_, i)| !refused.contains(i)).nth(index) else { break };
                log::warn!("library folders: {}: {error}", batch.get(*i).map_or("?", |r| r.file.key.as_str()));
                refused.insert(*i);
            }
            Err(e) => {
                log::error!("library folders: saving the library: {e:?}");
                return Vec::new();
            }
        }
    }
    for (i, kind) in kinds {
        if refused.contains(&i) {
            continue;
        }
        match kind {
            "added" => s.added += 1,
            "moved" => s.moved += 1,
            "changed" => s.changed += 1,
            _ => s.linked += 1,
        }
    }
    let mut previews = Vec::new();
    for (i, (r, e)) in batch.drain(..).zip(entries).enumerate() {
        let Some(e) = e.filter(|_| !refused.contains(&i)) else { continue };
        if e.photo.is_some() && e.error.is_none() && !e.hash.is_empty() && !removed.contains(&i) {
            previews.push((e.hash.clone(), r.file.path.clone()));
        }
        // a moved file is no longer in its old place
        if let Some(from) = moved_from.get(&i) {
            l.index.files.remove(from);
        }
        l.index.files.insert(r.file.key, e);
    }
    l.index.reindex();
    previews
}

/// Scan a user's library folders: add, move and update their photos in the user's library.
/// `progress` sees the status as it goes, `preview` each photo file whose previews may be
/// missing (the caller builds them, see [`build_previews`]).
pub fn scan(
    data: &Path,
    user: &str,
    lib: &Mutex<UserLib>,
    stop: &AtomicBool,
    progress: &mut dyn FnMut(&Status),
    preview: &mut dyn FnMut(String, PathBuf),
) -> Result<Status, String> {
    let folders: Vec<LibraryFolder> =
        accounts::read_users(data)?.users.get(user).map(|u| u.folders.clone()).ok_or_else(|| format!("no user `{user}`"))?;
    let now = lightcraft_engine::import::system_clock();
    let mut s = Status { scanning: true, ..Default::default() };
    progress(&s);
    let had: HashSet<String> = {
        let l = lock(lib);
        l.index.files.iter().filter(|(_, e)| !e.missing).filter_map(|(k, _)| k.split_once('/').map(|(r, _)| r.to_string())).collect()
    };
    // list every folder (no lock held)
    let mut found: Vec<Found> = Vec::new();
    let mut available: BTreeSet<String> = BTreeSet::new();
    for f in &folders {
        let dir = Path::new(&f.path);
        if !dir.is_dir() {
            s.errors.push(format!("{} ({}) isn't there (not mounted?): skipped", f.name, f.path));
            continue;
        }
        let before = found.len();
        let mut errs = Vec::new();
        walk(&f.name, dir, &mut found, &mut errs, stop);
        if found.len() == before && had.contains(&f.name) {
            s.errors.push(format!("{} ({}) is empty but had photos (not mounted?): skipped", f.name, f.path));
            continue;
        }
        s.errors.extend(errs.into_iter().take(10));
        available.insert(f.name.clone());
    }
    if found.len() >= FILES_MAX {
        s.errors.push(format!("more than {FILES_MAX} photo files: the rest are left out"));
    }
    let keys: HashSet<String> = found.iter().map(|f| f.key.clone()).collect();
    let (todo, mut known) = {
        let mut l = lock(lib);
        l.index.roots = folders.iter().map(|f| (f.name.clone(), f.path.clone())).collect();
        let todo: Vec<Found> = found
            .iter()
            .filter(|f| l.index.files.get(&f.key).is_none_or(|e| e.size != f.size || e.mtime != f.mtime || (e.photo.is_none() && e.error.is_none())))
            .cloned()
            .collect();
        // found again where they were
        for f in &found {
            if let Some(e) = l.index.files.get_mut(&f.key) {
                e.missing = false;
            }
        }
        l.index.reindex();
        (todo, Known::of(&l))
    };
    s.files = found.len();
    s.todo = todo.len();
    progress(&s);
    let user_dir = lock(lib).dir.clone();
    let mut batch = Vec::new();
    let (mut batch_at, mut saved_at) = (Instant::now(), Instant::now());
    for f in todo {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        batch.push(read(f, &known, &keys, &now));
        s.done += 1;
        if batch.len() >= BATCH || batch_at.elapsed() >= BATCH_TIME {
            for (hash, path) in commit(lib, &mut batch, &keys, &mut s, &now) {
                preview(hash, path);
            }
            let l = lock(lib);
            known = Known::of(&l);
            if saved_at.elapsed() >= SAVE_EVERY {
                if let Err(e) = l.index.save(&user_dir) {
                    log::error!("{e}");
                }
                saved_at = Instant::now();
            }
            drop(l);
            batch_at = Instant::now();
            progress(&s);
        }
    }
    for (hash, path) in commit(lib, &mut batch, &keys, &mut s, &now) {
        preview(hash, path);
    }
    // gone from folders that are there; every photo file whose previews may be missing
    let mut l = lock(lib);
    let roots: HashSet<&str> = folders.iter().map(|f| f.name.as_str()).collect();
    l.index.files.retain(|k, _| roots.contains(k.split_once('/').map_or("", |(r, _)| r)));
    let mut all = Vec::new();
    for (k, e) in l.index.files.iter_mut() {
        let root = k.split_once('/').map_or("", |(r, _)| r);
        if available.contains(root) && !keys.contains(k) {
            e.missing = true;
        }
        if e.missing {
            s.missing += 1;
        } else if e.photo.is_some() && e.error.is_none() && !e.hash.is_empty() {
            all.push((k.clone(), e.hash.clone()));
        }
    }
    l.index.reindex();
    let in_library: HashSet<u64> = l.core.catalog().photos().map(|p| p.id.0).collect();
    let all: Vec<(String, PathBuf)> = all
        .into_iter()
        .filter(|(k, _)| l.index.files.get(k).and_then(|e| e.photo).is_some_and(|id| in_library.contains(&id)))
        .filter_map(|(k, h)| Some((h, l.index.path_of(&k)?)))
        .collect();
    let saved = l.index.save(&user_dir);
    drop(l);
    if let Err(e) = saved {
        s.errors.push(e);
    }
    for (hash, path) in all {
        preview(hash, path);
    }
    s.scanning = false;
    s.last_scan = Some(accounts::now());
    Ok(s)
}

/// Build a photo file's smart and mini previews into the user's photo files (`blobs`), unless
/// they're there.
pub fn build_previews(blobs: &Path, hash: &str, file: &Path) -> Result<bool, String> {
    let (Some(smart), Some(mini)) = (crate::api::blob_path(blobs, "smart", hash), crate::api::blob_path(blobs, "mini", hash)) else {
        return Err(format!("not a content hash: {hash}"));
    };
    if smart.is_file() && mini.is_file() {
        return Ok(false);
    }
    for dir in [&smart, &mini].into_iter().filter_map(|p| p.parent()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let (f, s, m) = (file.to_string_lossy().to_string(), smart.to_string_lossy().to_string(), mini.to_string_lossy().to_string());
    // a huge file (a scan, a PSB) is read whole and decoded: one at a time
    static BIG: Mutex<()> = Mutex::new(());
    let _one = (std::fs::metadata(file).map_or(0, |m| m.len()) > READ_WHOLE).then(|| lock(&BIG));
    lightcraft_engine::guard::catch("building previews", || lightcraft_engine::sync::build_proxies(&f, &s, &m))??;
    Ok(true)
}

/// Previews whose photo files exist (cheap: skips the ones built).
fn previews_missing(blobs: &Path, hash: &str) -> bool {
    match (crate::api::blob_path(blobs, "smart", hash), crate::api::blob_path(blobs, "mini", hash)) {
        (Some(s), Some(m)) => !(s.is_file() && m.is_file()),
        _ => false,
    }
}

/// A preview to build: the user, the content hash, the file.
type Job = (String, String, PathBuf);

/// Scans and preview building in the background, for every user with library folders.
pub struct Scanner {
    /// Scan every user's folders this often (`None`: only at start and on demand).
    pub interval: Option<Duration>,
    wanted: Mutex<BTreeSet<String>>,
    wake: Condvar,
    status: Mutex<BTreeMap<String, Status>>,
    jobs: Mutex<(VecDeque<Job>, HashSet<(String, String)>)>,
    jobs_wake: Condvar,
    /// Previews that couldn't be built (user, hash): not tried again until the server restarts.
    failed: Mutex<HashMap<(String, String), String>>,
    pub(crate) stop: AtomicBool,
}

impl Scanner {
    pub fn new(interval: Option<Duration>) -> Scanner {
        Scanner {
            interval,
            wanted: Mutex::new(BTreeSet::new()),
            wake: Condvar::new(),
            status: Mutex::new(BTreeMap::new()),
            jobs: Mutex::new((VecDeque::new(), HashSet::new())),
            jobs_wake: Condvar::new(),
            failed: Mutex::new(HashMap::new()),
            stop: AtomicBool::new(false),
        }
    }

    /// Scan this user's folders soon.
    pub fn request(&self, user: &str) {
        lock(&self.wanted).insert(user.to_string());
        self.wake.notify_all();
    }

    pub fn status(&self, user: &str) -> Status {
        let mut s = lock(&self.status).get(user).cloned().unwrap_or_default();
        // (waiting or being built)
        s.previews = lock(&self.jobs).1.iter().filter(|(u, _)| u == user).count();
        s.preview_errors = lock(&self.failed).iter().filter(|((u, _), _)| u == user).map(|(_, e)| e.clone()).take(20).collect();
        s
    }

    fn queue(&self, blobs: &Path, user: &str, hash: String, file: PathBuf) {
        if !previews_missing(blobs, &hash) || lock(&self.failed).contains_key(&(user.to_string(), hash.clone())) {
            return;
        }
        let mut j = lock(&self.jobs);
        if j.1.insert((user.to_string(), hash.clone())) {
            j.0.push_back((user.to_string(), hash, file));
            self.jobs_wake.notify_one();
        }
    }

    pub(crate) fn shut_down(&self) {
        self.stop.store(true, Ordering::SeqCst);
        self.wake.notify_all();
        self.jobs_wake.notify_all();
    }
}

/// Users with library folders.
fn users_with_folders(data: &Path) -> Vec<String> {
    accounts::read_users(data).map(|f| f.users.into_iter().filter(|(_, u)| !u.folders.is_empty()).map(|(n, _)| n).collect()).unwrap_or_default()
}

/// The scanner thread: every user with folders at start, then on request and every interval.
pub(crate) fn run_scanner(st: &Arc<State>) {
    let sc = &st.folders;
    let mut periodic_at = Instant::now();
    for u in users_with_folders(&st.data) {
        sc.request(&u);
    }
    while !sc.stop.load(Ordering::SeqCst) {
        // asked from the command line (`lightcraft-server scan` while the server runs)
        for u in users_with_folders(&st.data) {
            let req = accounts::user_dir(&st.data, &u).join(REQUEST);
            if req.exists() {
                let _ = std::fs::remove_file(&req);
                sc.request(&u);
            }
        }
        if sc.interval.is_some_and(|i| periodic_at.elapsed() >= i) {
            periodic_at = Instant::now();
            for u in users_with_folders(&st.data) {
                sc.request(&u);
            }
        }
        let next = lock(&sc.wanted).pop_first();
        let Some(user) = next else {
            let w = lock(&sc.wanted);
            let _ = sc.wake.wait_timeout(w, Duration::from_secs(5));
            continue;
        };
        let lib = match crate::api::lib(st, &user) {
            Ok(l) => l,
            Err(e) => {
                log::error!("library folders of {user}: {e}");
                continue;
            }
        };
        let blobs = accounts::user_dir(&st.data, &user).join("blobs");
        let t = Instant::now();
        let scans = lock(&sc.status).get(&user).map_or(0, |s| s.scans);
        let r = lightcraft_engine::guard::catch("a library folder scan", || {
            scan(
                &st.data,
                &user,
                &lib,
                &sc.stop,
                &mut |s| {
                    lock(&sc.status).insert(user.clone(), Status { scans, ..s.clone() });
                },
                &mut |hash, file| sc.queue(&blobs, &user, hash, file),
            )
        });
        match r.and_then(|r| r) {
            Ok(mut s) => {
                s.scans = scans + 1;
                if s.added + s.moved + s.changed + s.linked + s.failed > 0 || !s.errors.is_empty() {
                    log::info!(
                        "{user}'s library folders: {} file(s) in {:.1} s; {} new, {} moved, {} changed, {} uploaded before, {} not read, {} missing{}",
                        s.files,
                        t.elapsed().as_secs_f64(),
                        s.added,
                        s.moved,
                        s.changed,
                        s.linked,
                        s.failed,
                        s.missing,
                        s.errors.first().map(|e| format!("; {e}")).unwrap_or_default()
                    );
                }
                lock(&sc.status).insert(user, s);
            }
            Err(e) => {
                log::error!("library folders of {user}: {e}");
                let mut all = lock(&sc.status);
                let s = all.entry(user).or_default();
                s.scanning = false;
                s.scans = scans + 1;
                s.errors = vec![e];
            }
        }
    }
}

/// A preview builder thread.
pub(crate) fn run_previews(st: &Arc<State>) {
    let sc = &st.folders;
    loop {
        let job = {
            let mut j = lock(&sc.jobs);
            loop {
                if sc.stop.load(Ordering::SeqCst) {
                    return;
                }
                if let Some(job) = j.0.pop_front() {
                    break job;
                }
                j = sc.jobs_wake.wait_timeout(j, Duration::from_secs(5)).unwrap_or_else(PoisonError::into_inner).0;
            }
        };
        let (user, hash, file) = job;
        let blobs = accounts::user_dir(&st.data, &user).join("blobs");
        let r = build_previews(&blobs, &hash, &file);
        lock(&sc.jobs).1.remove(&(user.clone(), hash.clone()));
        if let Err(e) = r {
            log::warn!("{user}: previews of {}: {e}", file.display());
            lock(&sc.failed).insert((user, hash), format!("{}: {e}", file.display()));
        }
        lightcraft_engine::memory::release();
    }
}
