//! Photos from the system photo library (Apple Photos on iPhone, iPad and Mac), shown by the app
//! without being imported, as Lightroom's Device tab does.
//!
//! The app browses the photo library itself (PhotoKit) and hands the engine one photo's original
//! only when it is opened (`photoLibrary.open`). The engine files it where an import would
//! (`Originals/YYYY/YYYY-MM-DD/`) as a **Local record**: out of All Photos, albums, counts and
//! sync, but shown and edited like any photo. Opening it is no undo step. A non-default develop
//! edit brings it into the library in that same undo step ([`Session::adopt_changed`]); leaving it
//! reset to its import look demotes it again, so rating, flags and metadata alone never import it.
//! Every user change goes through
//! [`Session::commit`] (LightCraft's own bookkeeping, `apply_system`, never brings a photo in), so
//! a photo opened from the photo library that is still Local was only looked at: it is forgotten
//! when it is left (`photoLibrary.close`) or when the library next opens, its record and the
//! engine's copy of the file both.
//!
//! Which asset is which photo is kept in `photo-library.json` in the library folder: this device's
//! alone (asset ids mean nothing elsewhere), never synced, by photo id and content hash (an id can
//! name another photo after a sign-out). No paths: the app's folder moves with every iOS update,
//! and the photo's own record follows it.
//!
//! With Keep Originals Where They Are ([`Session::keeps_in_place`]) the original stays in the photo library: the
//! engine's copy goes to [`CACHE_DIR`] instead of `Originals/`, the photo carries a Photos
//! [`lightcraft_catalog::Origin`] (never uploaded, [`crate::cloud_sync`]), and copies of photos left are removed once
//! the cache is over [`CACHE_BUDGET`] and the photo's smart preview stands in for it. Opening one whose copy is gone
//! answers `needsFile`, and the file the app fetches again is relinked (the same goes for a photo another device kept
//! in its photo library, found here by its content).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use lightcraft_catalog::{Op, OriginKind, Photo, PhotoId, Source};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::cmd::{CommandSpec, always, bad, cmd, str_param};
use crate::import::{ImportMode, ImportOptions, OnDeleted, Organize, import_with, is_supported};
use crate::sidecar::{SidecarNaming, sidecar_path};
use crate::{Result, Selection, Session};

/// Longest asset id taken (PhotoKit's are ~40 characters).
const MAX_ASSET: usize = 512;

/// Where the engine keeps its copies of originals kept in the photo library (Keep Originals Where They Are), in the
/// library folder: a cache, emptied down to [`CACHE_BUDGET`] as photos are left.
pub const CACHE_DIR: &str = "Photos Cache";

/// How much the copies in [`CACHE_DIR`] may take before the least recently opened go.
// ponytail: one fixed budget; a setting if people want more (or less) kept on the device
const CACHE_BUDGET: u64 = if cfg!(test) { 1 } else { 2 << 30 };

/// One asset the engine knows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    id: u64,
    /// The photo's content hash when it was opened: an entry whose photo no longer has it is stale.
    hash: Option<String>,
    /// The engine filed the file itself (so it goes with a forgotten record); `false` for an asset
    /// that turned out to be a photo the library already had.
    placed: bool,
    /// The asset's id on the person's other devices (PhotoKit's cloud identifier), when the app gave it: an
    /// edit sent through iCloud names it ([`crate::icloud`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cloud: Option<String>,
}

/// Asset id → photo, saved as `photo-library.json`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PhotoLibraryMap {
    assets: BTreeMap<String, Entry>,
    #[serde(skip)]
    dirty: bool,
}

impl PhotoLibraryMap {
    pub const FILE: &'static str = "photo-library.json";

    pub fn from_json(bytes: &[u8]) -> PhotoLibraryMap {
        serde_json::from_slice(bytes).unwrap_or_default()
    }

    fn by_photo(&self, id: PhotoId) -> Option<(&String, &Entry)> {
        self.assets.iter().find(|(_, e)| e.id == id.0)
    }

    /// Whether `id` is a photo opened from the photo library.
    pub(crate) fn has(&self, id: PhotoId) -> bool {
        !self.assets.is_empty() && self.by_photo(id).is_some()
    }

    fn insert(&mut self, asset: &str, e: Entry) {
        self.assets.insert(asset.to_string(), e);
        self.dirty = true;
    }

    fn remove(&mut self, asset: &str) {
        if self.assets.remove(asset).is_some() {
            self.dirty = true;
        }
    }

    /// The photos of the assets the engine knows.
    pub(crate) fn photo_ids(&self) -> impl Iterator<Item = PhotoId> + '_ {
        self.assets.values().map(|e| PhotoId(e.id))
    }

    /// How many assets the engine knows.
    pub(crate) fn len(&self) -> usize {
        self.assets.len()
    }

    /// Photo `id`'s content hash when it was opened, and its asset's cloud identifier.
    pub(crate) fn entry_of(&self, id: PhotoId) -> Option<(Option<&str>, Option<&str>)> {
        self.by_photo(id).map(|(_, e)| (e.hash.as_deref(), e.cloud.as_deref()))
    }
}

/// The entry's photo, when it is still the one the asset was opened as.
fn live<'a>(s: &'a Session, e: &Entry) -> Option<&'a Photo> {
    s.catalog.photo(PhotoId(e.id)).map(|p| &**p).filter(|p| !p.deleted && p.content_hash == e.hash)
}

fn asset_param<'a>(p: &'a Value, c: &str) -> Result<&'a str> {
    let a = str_param(p, "asset").ok_or_else(|| bad(c, "missing `asset`"))?;
    if a.is_empty() || a.len() > MAX_ASSET || a.chars().any(char::is_control) {
        return Err(bad(c, "`asset` must be 1 to 512 printable characters"));
    }
    Ok(a)
}

/// The optional `cloud` parameter: the asset's cloud identifier (the same rules as `asset`).
fn cloud_param(p: &Value) -> Option<String> {
    str_param(p, "cloud").filter(|a| !a.is_empty() && a.len() <= MAX_ASSET && !a.chars().any(char::is_control)).map(str::to_string)
}

fn id_param(p: &Value, c: &str) -> Result<PhotoId> {
    p.get("id").and_then(Value::as_u64).map(PhotoId).ok_or_else(|| bad(c, "missing photo `id`"))
}

/// `path` lies inside `dir` (both resolved: no `..` or link games).
fn inside(path: &Path, dir: &Path) -> bool {
    match (std::fs::canonicalize(path), std::fs::canonicalize(dir)) {
        (Ok(p), Ok(d)) => p.starts_with(d),
        _ => false,
    }
}

/// A photo whose original stays in the photo library ([`OriginKind::Photos`]).
fn kept_in_photos(p: &Photo) -> bool {
    p.origin.as_ref().is_some_and(|o| o.kind == OriginKind::Photos)
}

/// A photo kept in the photo library with no copy of its original on this device (removed from [`CACHE_DIR`], or
/// one another device kept in its photo library).
fn needs_copy(p: &Photo) -> bool {
    kept_in_photos(p)
        && match &p.source {
            Source::File { path } => path.starts_with(lightcraft_catalog::sync::PATH_PREFIX) || !Path::new(path).is_file(),
            Source::Demo { .. } => false,
        }
}

impl Session {
    /// [`CACHE_DIR`] of a library on disk.
    fn photos_cache_dir(&self) -> Option<PathBuf> {
        self.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.join(CACHE_DIR))
    }

    /// Where the app stages the originals it fetches from the photo library: a file opened from
    /// there is moved into the library; from anywhere else it is copied (never removed).
    pub fn set_photo_library_staging(&mut self, dir: Option<PathBuf>) {
        self.photo_library_staging = dir;
    }

    /// Save `photo-library.json` if it changed (a failure is logged: at worst an asset is fetched
    /// again, or a forgotten record's entry is dropped at the next open).
    pub(crate) fn save_photo_library(&mut self) {
        if !self.photo_library.dirty {
            return;
        }
        let Some(lib) = self.library.as_mut() else { return };
        match serde_json::to_vec(&self.photo_library) {
            Ok(bytes) => match lib.files_mut().write_atomic(PhotoLibraryMap::FILE, &bytes) {
                Ok(()) => self.photo_library.dirty = false,
                Err(e) => log::error!("library: {}: {e}", PhotoLibraryMap::FILE),
            },
            Err(e) => log::error!("library: {}: {e}", PhotoLibraryMap::FILE),
        }
    }

    /// Part of [`Session::commit`]: photo-library Local records that `fwd` changed the way a user
    /// does join the library in the same step. Returns the step's ops (the promotion first, so a
    /// server learns of the photo before its change) and their inverse.
    pub(crate) fn adopt_changed(&mut self, fwd: Op, inv: Op) -> (Op, Op) {
        if self.photo_library.assets.is_empty() {
            return (fwd, inv);
        }
        let mut ids = Vec::new();
        user_changed(&fwd, &mut ids, 0);
        ids.sort();
        ids.dedup();
        ids.retain(|id| self.catalog.photo(*id).is_some_and(|p| p.local && p.is_edited()) && self.photo_library.has(*id));
        if ids.is_empty() {
            return (fwd, inv);
        }
        let promote = Op::Batch { ops: ids.into_iter().map(|id| Op::SetLocal { id, local: false }).collect() };
        match self.catalog.apply(promote.clone()) {
            Ok(pinv) => (Op::Batch { ops: vec![promote, fwd] }, Op::Batch { ops: vec![inv, pinv] }),
            Err(e) => {
                log::warn!("photo library: adding a changed photo to the library: {e}");
                (fwd, inv)
            }
        }
    }

    /// When a library opens: photo-library photos left open and never changed (the app quit with
    /// one open) go, with the engine's copy of their file; entries whose photo is gone are dropped.
    pub(crate) fn tidy_photo_library(&mut self) {
        let assets: Vec<(String, Entry)> = self.photo_library.assets.iter().map(|(a, e)| (a.clone(), e.clone())).collect();
        for (asset, e) in assets {
            match live(self, &e).map(|p| p.local) {
                None => self.photo_library.remove(&asset),
                Some(true) if self.forgettable(PhotoId(e.id)) => self.forget_opened(&asset, &e),
                Some(_) => {}
            }
        }
        // (the view's saved selection was restored before this ran)
        let exists = |id: &PhotoId| self.catalog.photo(*id).is_some();
        let ids = self.selection.ids.iter().copied().filter(exists).collect();
        let active = self.selection.active.filter(exists);
        self.selection = Selection { ids, active };
        self.save_photo_library();
    }

    /// Nothing still needs a Local record that was only looked at: no undo / redo step, drag or
    /// virtual copy names it.
    fn forgettable(&self, id: PhotoId) -> bool {
        let mut held = self.interaction.as_ref().is_some_and(|i| i.photo == id);
        for e in self.undo.iter().chain(self.redo.iter()) {
            e.op.photo_ids(&mut |p| held |= p == id);
        }
        !held && !self.catalog.photos().any(|p| p.copy_of == Some(id))
    }

    /// Remove a looked-at record (journaled, not an undo step) and, when the engine filed its file,
    /// that file, its sidecars (unless another photo shares them) and the day folders left empty
    /// (only inside the library's `Originals/`).
    fn forget_opened(&mut self, asset: &str, e: &Entry) {
        let id = PhotoId(e.id);
        let path = match self.catalog.photo(id).map(|p| &p.source) {
            Some(Source::File { path }) => Some(PathBuf::from(path)),
            _ => None,
        };
        let op = Op::RemovePhoto { id };
        match self.catalog.apply(op.clone()) {
            Ok(_) => self.pending_log.push(op),
            Err(err) => {
                log::warn!("photo library: forget {asset}: {err}");
                return;
            }
        }
        self.media.forget(id);
        self.before.remove(&id);
        if self.previous_active == Some(id) {
            self.previous_active = None;
        }
        self.photo_library.remove(asset);
        // (its Live Photo motion, if the app handed one over)
        self.detach_motion(id);
        let roots = [self.library.as_ref().filter(|l| l.on_disk).map(|l| l.originals_dir()), self.photos_cache_dir()];
        let Some(path) = path.filter(|_| e.placed) else { return };
        let Some(originals) = roots.into_iter().flatten().find(|r| inside(&path, r)) else { return };
        let shared = |sc: &Path| {
            let stem = sc.with_extension("");
            self.catalog.photos().any(|p| matches!(&p.source, Source::File { path } if Path::new(path).with_extension("") == stem))
        };
        let mut gone = vec![path.clone()];
        for naming in [SidecarNaming::Stem, SidecarNaming::Full] {
            let sc = sidecar_path(&path.to_string_lossy(), naming);
            if sc.exists() && (naming == SidecarNaming::Full || !shared(&sc)) {
                gone.push(sc);
            }
        }
        for f in gone {
            if let Err(err) = std::fs::remove_file(&f)
                && err.kind() != std::io::ErrorKind::NotFound
            {
                log::warn!("photo library: remove {}: {err}", f.display());
            }
        }
        // the day and year folders it leaves empty (remove_dir refuses a folder with anything in it)
        let mut dir = path.parent().map(Path::to_path_buf);
        while let Some(d) = dir.filter(|d| d.starts_with(&originals) && *d != originals) {
            if std::fs::remove_dir(&d).is_err() {
                break;
            }
            dir = d.parent().map(Path::to_path_buf);
        }
    }

    /// Build the smart preview of photo `id` kept in the photo library when it has none, so that it can stand in for
    /// the copy of its original once [`Session::trim_photos_cache`] removes that (in the background).
    // ponytail: skipped while another preview build runs; the copy then just stays (over the budget) until a later leave
    fn keep_smart_preview(&mut self, id: PhotoId) {
        let Some(dir) = self.media.smart_dir.clone() else { return };
        if self.catalog.photo(id).is_some_and(|p| crate::smart::is_valid(&crate::smart::file_path(&dir, p))) {
            return;
        }
        if let Err(e) = self.execute("library.smartPreviews", &json!({"ids": [id.0], "background": true})) {
            log::info!("photo library: smart preview of photo {}: {e}", id.0);
        }
    }

    /// Keep the copies in [`CACHE_DIR`] under [`CACHE_BUDGET`]: those of library photos not shown whose smart preview
    /// can stand in for them go, least recently fetched first. Opening one fetches it again ([`needs_copy`]).
    // ponytail: a stat per cached copy on every leave; a running total if big caches make leaving slow
    pub(crate) fn trim_photos_cache(&mut self) {
        let (Some(dir), Some(smart)) = (self.photos_cache_dir(), self.media.smart_dir.clone()) else { return };
        let shown = self.selection.active;
        let mut total = 0u64;
        let mut copies = Vec::new();
        for p in self.catalog.photos().filter(|p| kept_in_photos(p)) {
            let Source::File { path } = &p.source else { continue };
            let path = Path::new(path);
            if !path.starts_with(&dir) {
                continue;
            }
            let Ok(m) = std::fs::metadata(path) else { continue };
            total = total.saturating_add(m.len());
            if !p.local && !p.deleted && Some(p.id) != shown {
                let fetched = m.modified().unwrap_or(std::time::UNIX_EPOCH);
                copies.push((fetched, m.len(), p.id, path.to_path_buf(), crate::smart::file_path(&smart, p)));
            }
        }
        if total <= CACHE_BUDGET {
            return;
        }
        copies.sort_by_key(|c| c.0);
        for (_, len, id, file, stand_in) in copies {
            if total <= CACHE_BUDGET {
                break;
            }
            if !crate::smart::is_valid(&stand_in) {
                continue;
            }
            match std::fs::remove_file(&file) {
                Ok(()) => {
                    total = total.saturating_sub(len);
                    self.media.forget(id);
                }
                Err(e) => log::warn!("photo library: remove {}: {e}", file.display()),
            }
        }
    }

    /// Make `id` the photo shown, keeping what was selected to come back to when it is left.
    fn show_opened(&mut self, id: PhotoId) -> Result<()> {
        self.end_interaction()?;
        // (kept while photo-library photos are being looked at, from the first opened to the last closed: it is also
        // what tells next / Auto Advance that the photo shown isn't the grid's, `cmd::library::step`)
        if self.photo_library_return.is_none() {
            self.photo_library_return = Some(self.selection.clone());
        }
        self.selection = Selection::single(id);
        self.active_mask = None;
        self.active_spot = None;
        Ok(())
    }

    /// Importing for real a file that is open from the photo library and still Local (the picker
    /// chose the same photo): it joins the library as it is, where it already is.
    pub(crate) fn opened_from_photo_library(&self, id: PhotoId) -> bool {
        self.catalog.photo(id).is_some_and(|p| p.local) && self.photo_library.has(id)
    }
}

/// The photos `op` changes the way a person does (not analysis, faces, files or albums).
fn user_changed(op: &Op, out: &mut Vec<PhotoId>, depth: usize) {
    match op {
        Op::SetDevelop { id, .. } => out.push(*id),
        Op::Batch { ops } if depth < 32 => ops.iter().for_each(|o| user_changed(o, out, depth + 1)),
        _ => {}
    }
}

fn open(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "photoLibrary.open";
    let asset = asset_param(p, C)?;
    let cloud = cloud_param(p);
    // opened before, and still the same photo: no file needed
    if let Some(e) = s.photo_library.assets.get(asset).cloned() {
        match live(s, &e).map(|p| p.local) {
            Some(local) => {
                // kept in the photo library, and its copy here is gone: the app fetches the original again
                if s.catalog.photo(PhotoId(e.id)).is_some_and(|p| needs_copy(p)) {
                    let Some(path) = str_param(p, "path") else { return Ok(json!({"needsFile": true})) };
                    copy_again(s, PhotoId(e.id), Path::new(path))?;
                } else if let Some(path) = str_param(p, "path") {
                    drop_staged(s, Path::new(path));
                }
                if cloud.is_some() && e.cloud != cloud {
                    s.photo_library.insert(asset, Entry { cloud, ..e.clone() });
                    s.save_photo_library();
                }
                s.show_opened(PhotoId(e.id))?;
                let live = crate::live_photo::open_with_motion(s, PhotoId(e.id), p);
                return Ok(json!({"id": e.id, "local": local, "known": true, "live": live}));
            }
            None => s.photo_library.remove(asset),
        }
    }
    let Some(path) = str_param(p, "path") else { return Ok(json!({"needsFile": true})) };
    let src = PathBuf::from(path);
    if !src.is_file() {
        return Err(bad(C, format!("{path}: not a file")));
    }
    if !is_supported(&src) {
        return Err(bad(C, format!("{path}: not a photo LightCraft reads")));
    }
    let probe = s.media.file_probe.clone().ok_or_else(|| bad(C, "this build can't read files"))?;
    let info = probe(path).map_err(|e| bad(C, format!("{path}: {e}")))?;
    // a photo the library already has (imported before through the picker, or on another device)
    let hash = info.content_hash.clone();
    let existing =
        hash.as_deref().and_then(|h| s.catalog.photos().find(|p| !p.local && !p.deleted && p.content_hash.as_deref() == Some(h)).map(|p| p.id));
    if let Some(id) = existing {
        s.photo_library.insert(asset, Entry { id: id.0, hash, placed: false, cloud });
        s.save_photo_library();
        // (a photo kept in a photo library, here or by another device: this copy is the one it is edited from)
        if s.catalog.photo(id).is_some_and(|p| needs_copy(p)) {
            // (the photo still opens without it, from its previews)
            if let Err(e) = copy_again(s, id, &src) {
                log::warn!("photo library: {e}");
            }
        } else {
            drop_staged(s, &src);
        }
        // (an edit another device made to this picture, held until it was opened here, is taken)
        let synced = s.icloud_take_held(id);
        s.show_opened(id)?;
        let live = crate::live_photo::open_with_motion(s, id, p);
        return Ok(json!({"id": id.0, "local": false, "known": true, "synced": synced, "live": live}));
    }
    // filed where an import would put it (or in the cache, when the original stays in the photo library), then
    // catalogued there in place
    let origin = s.new_origin(OriginKind::Photos, cloud.clone());
    let root = match origin {
        Some(_) => s.photos_cache_dir(),
        None => s.library.as_ref().filter(|l| l.on_disk).map(|l| l.originals_dir()),
    }
    .ok_or_else(|| bad(C, "needs a library saved on disk"))?;
    let name = src.file_name().map(|n| n.to_string_lossy().to_string()).ok_or_else(|| bad(C, format!("{path}: no file name")))?;
    let date = info.captured.clone().or_else(|| info.file_modified.clone()).unwrap_or_else(|| (s.clock)());
    let mut dated = Photo::new(PhotoId(0), Source::File { path: path.into() }, &name, &info.format, info.width, info.height, &date);
    dated.captured = info.captured.clone();
    let dir = Organize::ByDay.folders(&dated).into_iter().fold(root, |d, f| d.join(f));
    let placed = crate::import_move::place(&src, &dir, &name).map_err(|e| bad(C, e))?;
    let dst = placed.dst.to_string_lossy().to_string();
    s.import_probes.insert(dst.clone(), info);
    // opening isn't a step to undo (Undo would take away the photo being looked at), and leaves redo as it was
    let (undo0, redo) = (s.undo.len(), std::mem::take(&mut s.redo));
    // ponytail: an import job snapshots the catalog (~ms per 10k photos); a staged-file fast path if big libraries feel it
    let opts = ImportOptions { mode: ImportMode::Add, local: true, on_deleted: OnDeleted::Skip, origin, ..Default::default() };
    let report = import_with(s, std::slice::from_ref(&dst), &opts).map(|r| r.imported.first().copied());
    s.undo.truncate(undo0);
    s.redo = redo;
    let id = match report {
        Ok(Some(id)) => PhotoId(id),
        other => {
            crate::import_move::rollback(&placed);
            s.import_probes.remove(&dst);
            return Err(match other {
                Err(e) => e,
                _ => bad(C, format!("{path}: could not be read")),
            });
        }
    };
    let hash = s.catalog.photo(id).and_then(|p| p.content_hash.clone());
    s.photo_library.insert(asset, Entry { id: id.0, hash, placed: true, cloud });
    s.save_photo_library();
    if inside_staging(s, &src) {
        for sc in &placed.sidecars {
            let _ = std::fs::remove_file(&sc.src);
        }
    }
    drop_staged(s, &src);
    // an edit another device made to this picture brings it into the library edited
    let synced = s.icloud_take_held(id);
    s.show_opened(id)?;
    let local = s.catalog.photo(id).is_some_and(|p| p.local);
    let live = crate::live_photo::open_with_motion(s, id, p);
    Ok(json!({"id": id.0, "local": local, "known": false, "synced": synced, "live": live}))
}

/// The original of photo `id` kept in the photo library ([`needs_copy`]), fetched again by the app at `src`: checked to
/// be the same picture, filed in [`CACHE_DIR`], and the photo pointed at it (on this device only; no undo step).
fn copy_again(s: &mut Session, id: PhotoId, src: &Path) -> Result<()> {
    const C: &str = "photoLibrary.open";
    let p = s.catalog.photo(id).cloned().ok_or(lightcraft_catalog::CatalogError::NoPhoto(id))?;
    let probe = s.media.file_probe.clone().ok_or_else(|| bad(C, "this build can't read files"))?;
    let read = probe(&src.to_string_lossy()).map_err(|e| bad(C, format!("{}: {e}", src.display())));
    if read.as_ref().map_or(true, |info| info.content_hash.is_none() || info.content_hash != p.content_hash) {
        drop_staged(s, src);
        read?;
        return Err(bad(C, "the photo library's original is no longer this photo's"));
    }
    let root = s.photos_cache_dir().ok_or_else(|| bad(C, "needs a library saved on disk"))?;
    let dir = Organize::ByDay.folders(&p).into_iter().fold(root, |d, f| d.join(f));
    let placed = crate::import_move::place(src, &dir, &p.file_name).map_err(|e| bad(C, e))?;
    let path = placed.dst.to_string_lossy().to_string();
    let op = Op::Relink { id, file_name: p.file_name.clone(), source: Source::File { path }, format: None };
    if let Err(e) = s.catalog.apply(op.clone()) {
        crate::import_move::rollback(&placed);
        return Err(e.into());
    }
    s.pending_log.push(op);
    s.media.forget(id);
    if inside_staging(s, src) {
        for sc in &placed.sidecars {
            let _ = std::fs::remove_file(&sc.src);
        }
    }
    drop_staged(s, src);
    Ok(())
}

pub(crate) fn inside_staging(s: &Session, path: &Path) -> bool {
    s.photo_library_staging.as_deref().is_some_and(|d| inside(path, d))
}

/// The app's staged copy has done its job (only ever a file in the staging folder).
fn drop_staged(s: &Session, path: &Path) {
    if inside_staging(s, path)
        && let Err(e) = std::fs::remove_file(path)
    {
        log::warn!("photo library: remove {}: {e}", path.display());
    }
}

/// The assets whose photo is in the library: `{asset: id}`.
fn added(s: &mut Session, _: &Value) -> Result<Value> {
    let out: serde_json::Map<String, Value> =
        s.photo_library.assets.iter().filter(|(_, e)| live(s, e).is_some_and(|p| !p.local)).map(|(a, e)| (a.clone(), json!(e.id))).collect();
    Ok(Value::Object(out))
}

/// What the app fetches for photo `id` kept in the photo library with no copy here ([`needs_copy`]): the asset this
/// device opened it as, and its cloud identifier (another device's photo: the same picture in this device's photo
/// library, when it syncs with the same iCloud Photos). `{}` for any other photo.
fn wanted(s: &mut Session, p: &Value) -> Result<Value> {
    let id = id_param(p, "photoLibrary.wanted")?;
    let Some(photo) = s.catalog.photo(id).filter(|p| needs_copy(p)) else { return Ok(json!({})) };
    let asset = s.photo_library.by_photo(id).map(|(a, _)| a.clone());
    let cloud = photo.origin.as_ref().and_then(|o| o.cloud.clone());
    Ok(json!({"asset": asset, "cloud": cloud}))
}

/// The original of photo `id` kept in the photo library, fetched by the app from asset `asset` at `path`: taken as
/// [`copy_again`] does, and the asset remembered as this photo's. Nothing else changes (not the photo shown).
fn attach(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "photoLibrary.attach";
    let id = id_param(p, C)?;
    let asset = asset_param(p, C)?.to_string();
    let path = str_param(p, "path").ok_or_else(|| bad(C, "missing `path`"))?;
    if !s.catalog.photo(id).is_some_and(|p| needs_copy(p)) {
        drop_staged(s, Path::new(path));
        return Ok(json!({"attached": false}));
    }
    copy_again(s, id, Path::new(path))?;
    let hash = s.catalog.photo(id).and_then(|p| p.content_hash.clone());
    s.photo_library.insert(&asset, Entry { id: id.0, hash, placed: false, cloud: cloud_param(p) });
    s.save_photo_library();
    Ok(json!({"attached": true}))
}

fn close(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "photoLibrary.close";
    let id = id_param(p, C)?;
    // (a drag in progress is committed first: it may bring the photo in)
    if s.interaction.as_ref().is_some_and(|i| i.photo == id) {
        s.end_interaction()?;
    }
    let entry = s.photo_library.by_photo(id).map(|(a, e)| (a.clone(), e.clone()));
    let edited = s.catalog.photo(id).is_some_and(|p| p.is_edited());
    if !edited && s.catalog.photo(id).is_some_and(|p| !p.local && !p.deleted) && s.photo_library.by_photo(id).is_some_and(|(_, entry)| entry.placed) {
        let op = Op::SetLocal { id, local: true };
        let inv = s.apply_catalog(op.clone())?;
        s.sync_record(&op, &inv);
        s.pending_log.push(op);
    }
    let promoted = s.catalog.photo(id).is_some_and(|p| !p.local && !p.deleted && p.is_edited());
    if promoted && s.catalog.photo(id).is_some_and(|p| kept_in_photos(p)) {
        s.keep_smart_preview(id);
    }
    let mut forgotten = false;
    if let Some((asset, e)) = entry
        && live(s, &e).is_some_and(|p| p.local)
        && s.forgettable(id)
    {
        s.forget_opened(&asset, &e);
        s.save_photo_library();
        forgotten = true;
    }
    if s.selection.active == Some(id) || forgotten {
        let back = s.photo_library_return.take().unwrap_or_default();
        let exists = |i: &PhotoId| s.catalog.photo(*i).is_some();
        let selection = Selection { ids: back.ids.into_iter().filter(exists).collect(), active: back.active.filter(exists) };
        s.selection = selection;
    }
    s.trim_photos_cache();
    Ok(json!({"promoted": promoted, "forgotten": forgotten}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        // (not journaled: the paths are this device's, and gone once the photo is opened)
        cmd!(
            query "photoLibrary.open",
            "Open from Photo Library",
            [],
            None,
            "{asset, path?, cloud?, motion?, contentId?, stillTime?}: the photo-library asset `asset` (its original at `path`; without one, {needsFile: true} unless it is known; `cloud`: its id on the person's other devices, which an edit synced through iCloud carries): shown and edited without joining the library until it is changed, or until an edit another device made to it is taken (`synced`); `motion`, a Live Photo's clip, is kept with it when it has none yet (live.attach); {id, local, known, synced, live}",
            always,
            open
        ),
        cmd!(query "photoLibrary.added", "Photo Library Photos in the Library", [], None, "{}: {asset: id} of the assets whose photo is in the library", always, added),
        cmd!(
            query "photoLibrary.wanted",
            "Photo Library Original Wanted",
            [],
            None,
            "{id}: for a photo whose original stays in the photo library (Keep Originals Where They Are) and has no copy on this device: {asset?: the asset this device opened it as, cloud?: its cloud identifier, for the same picture in this device's photo library}; {} for any other photo",
            always,
            wanted
        ),
        cmd!(
            query "photoLibrary.attach",
            "Attach Photo Library Original",
            [],
            None,
            "{id, asset, path, cloud?}: the original the app fetched from asset `asset` for photo `id` (see photoLibrary.wanted), checked to be the same picture and kept as the photo's copy on this device; the photo shown doesn't change; {attached}",
            always,
            attach
        ),
        cmd!(
            query "photoLibrary.close",
            "Leave Photo Library Photo",
            [],
            None,
            "{id}: a photo-library photo nobody changed is forgotten (its record and the engine's copy go); {promoted, forgotten}",
            always,
            close
        ),
    ]
}
