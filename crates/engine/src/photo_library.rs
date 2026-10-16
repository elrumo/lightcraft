//! Photos from the system photo library (Apple Photos on iPhone, iPad and Mac), shown by the app
//! without being imported, as Lightroom's Device tab does.
//!
//! The app browses the photo library itself (PhotoKit) and hands the engine one photo's original
//! only when it is opened (`photoLibrary.open`). The engine files it where an import would
//! (`Originals/YYYY/YYYY-MM-DD/`) as a **Local record**: out of All Photos, albums, counts and
//! sync, but shown and edited like any photo. Opening it is no undo step. The first change the
//! user makes to it (develop settings, rating, flag, label, metadata, capture time, versions)
//! brings it into the library in that same undo step ([`Session::adopt_changed`]), so it syncs from
//! then on, and undoing the change takes it back out. Every user change goes through
//! [`Session::commit`] (LightCraft's own bookkeeping, `apply_system`, never brings a photo in), so
//! a photo opened from the photo library that is still Local was only looked at: it is forgotten
//! when it is left (`photoLibrary.close`) or when the library next opens, its record and the
//! engine's copy of the file both.
//!
//! Which asset is which photo is kept in `photo-library.json` in the library folder: this device's
//! alone (asset ids mean nothing elsewhere), never synced, by photo id and content hash (an id can
//! name another photo after a sign-out). No paths: the app's folder moves with every iOS update,
//! and the photo's own record follows it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use lightcraft_catalog::{Op, Photo, PhotoId, Source};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::cmd::{CommandSpec, always, bad, cmd, str_param};
use crate::import::{ImportMode, ImportOptions, OnDeleted, Organize, import_with, is_supported};
use crate::sidecar::{SidecarNaming, sidecar_path};
use crate::{Result, Selection, Session};

/// Longest asset id taken (PhotoKit's are ~40 characters).
const MAX_ASSET: usize = 512;

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

impl Session {
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
        ids.retain(|id| self.catalog.photo(*id).is_some_and(|p| p.local) && self.photo_library.has(*id));
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
        let originals = self.library.as_ref().filter(|l| l.on_disk).map(|l| l.originals_dir());
        let (Some(path), Some(originals)) = (path.filter(|_| e.placed), originals) else { return };
        if !inside(&path, &originals) {
            return;
        }
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
        Op::SetRating { id, .. }
        | Op::SetFlag { id, .. }
        | Op::SetLabel { id, .. }
        | Op::SetDevelop { id, .. }
        | Op::SetMeta { id, .. }
        | Op::SetVersions { id, .. }
        | Op::SetHistory { id, .. }
        | Op::PushHistory { id, .. }
        | Op::SetHistoryCursor { id, .. }
        | Op::SetCaptured { id, .. } => out.push(*id),
        Op::Batch { ops } if depth < 32 => ops.iter().for_each(|o| user_changed(o, out, depth + 1)),
        _ => {}
    }
}

fn open(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "photoLibrary.open";
    let asset = asset_param(p, C)?;
    // opened before, and still the same photo: no file needed
    if let Some(e) = s.photo_library.assets.get(asset).cloned() {
        match live(s, &e).map(|p| p.local) {
            Some(local) => {
                if let Some(path) = str_param(p, "path") {
                    drop_staged(s, Path::new(path));
                }
                s.show_opened(PhotoId(e.id))?;
                return Ok(json!({"id": e.id, "local": local, "known": true}));
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
        s.photo_library.insert(asset, Entry { id: id.0, hash, placed: false });
        s.save_photo_library();
        drop_staged(s, &src);
        s.show_opened(id)?;
        return Ok(json!({"id": id.0, "local": false, "known": true}));
    }
    // filed where an import would put it, then catalogued there in place
    let root = s.library.as_ref().filter(|l| l.on_disk).map(|l| l.originals_dir()).ok_or_else(|| bad(C, "needs a library saved on disk"))?;
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
    let opts = ImportOptions { mode: ImportMode::Add, local: true, on_deleted: OnDeleted::Skip, ..Default::default() };
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
    s.photo_library.insert(asset, Entry { id: id.0, hash, placed: true });
    s.save_photo_library();
    if inside_staging(s, &src) {
        for sc in &placed.sidecars {
            let _ = std::fs::remove_file(&sc.src);
        }
    }
    drop_staged(s, &src);
    s.show_opened(id)?;
    Ok(json!({"id": id.0, "local": true, "known": false}))
}

fn inside_staging(s: &Session, path: &Path) -> bool {
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

fn close(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "photoLibrary.close";
    let id = id_param(p, C)?;
    // (a drag in progress is committed first: it may bring the photo in)
    if s.interaction.as_ref().is_some_and(|i| i.photo == id) {
        s.end_interaction()?;
    }
    let entry = s.photo_library.by_photo(id).map(|(a, e)| (a.clone(), e.clone()));
    let promoted = s.catalog.photo(id).is_some_and(|p| !p.local && !p.deleted);
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
            "{asset, path?}: the photo-library asset `asset` (its original at `path`; without one, {needsFile: true} unless it is known): shown and edited without joining the library until it is changed; {id, local, known}",
            always,
            open
        ),
        cmd!(query "photoLibrary.added", "Photo Library Photos in the Library", [], None, "{}: {asset: id} of the assets whose photo is in the library", always, added),
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
