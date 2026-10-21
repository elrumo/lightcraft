//! *Keep Photos in iCloud Drive*: for a library without a server, its photos travel through a folder of the person's
//! iCloud Drive (the host's, named by `icloud.folder`: a native frontend on Apple devices gives its iCloud Drive
//! container), which iCloud Drive carries to their other devices.
//!
//! - **New imports** that copy their originals put them there ([`Session::icloud_import_root`], the import's
//!   destination when none was chosen), in the folders an import makes (`YYYY/YYYY-MM-DD/`).
//! - **Photos already in the library** move there on request (`icloud.moveIn`, a few at a time): those whose original
//!   the library keeps itself (`Originals/`), keeping their place in it. Photos opened from the Photos library stay
//!   (iCloud Photos carries them), and so do files referenced where they are.
//! - **Each such photo's `edit` record** says where its file is in the folder (`file`, relative) beside its edits. A
//!   device that has no photo with that picture holds the record and lists the file as wanted (`icloud.wanted`); the
//!   host brings it down (iCloud Drive may not have it on the device yet) and hands it over (`icloud.adopt`): it is
//!   added to the library where it is, as an import in place, and takes the record's edits. Not an undo step: undoing
//!   it would remove the photo from every device.
//! - **A file iCloud Drive took back** to save space is wanted again too, and `icloud.adopt` tells the renderer it is
//!   there once it has come.
//! - **Recently Deleted and Hidden** travel with the record; a photo removed for good on one device is removed on the
//!   others (its file stays where it is: LightCraft never deletes originals).
//!
//! Each of the two long jobs (photos coming in, originals moving in) has an activity row while there is work left.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use lightcraft_catalog::{Op, Photo, PhotoId, Source};
use serde_json::{Value, json};

use super::{EDIT, ICloud};
use crate::{EngineError, Result, Session};

/// A relative path in the folder is this long at most.
const REL_MAX: usize = 1024;
/// Files handed over at once at most (an import in place reads each one whole).
pub(super) const ADOPT_MAX: usize = 16;
/// Originals moved at once at most.
pub(super) const MOVE_MAX: usize = 64;

/// `p`'s original as a path relative to `folder` (with `/`), when it is in it.
pub(super) fn file_in(p: &Photo, folder: &Path) -> Option<String> {
    let Source::File { path } = &p.source else { return None };
    relative(Path::new(path).strip_prefix(folder).ok()?)
}

fn relative(rel: &Path) -> Option<String> {
    let mut parts = Vec::new();
    for c in rel.components() {
        match c {
            Component::Normal(s) => parts.push(s.to_str()?.to_string()),
            _ => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// A relative path another device sent, as a path in `folder`: nothing that could leave it.
pub(super) fn path_in(folder: &Path, rel: &str) -> Option<PathBuf> {
    if rel.is_empty() || rel.len() > REL_MAX || rel.contains('\\') || rel.chars().any(char::is_control) {
        return None;
    }
    let mut out = folder.to_path_buf();
    for part in rel.split('/') {
        if part.is_empty() || part == "." || part == ".." || (cfg!(windows) && part.contains(':')) {
            return None;
        }
        out.push(part);
    }
    Some(out)
}

/// The library's photos whose original is in `folder`.
pub(super) fn in_folder(s: &Session, folder: &Path) -> Vec<PhotoId> {
    s.catalog.photos().filter(|p| !p.local && p.copy_of.is_none() && file_in(p, folder).is_some()).map(|p| p.id).collect()
}

/// Those of them whose picture is `key`.
pub(super) fn in_folder_with(s: &Session, folder: &Path, key: &str) -> Vec<PhotoId> {
    s.catalog
        .photos()
        .filter(|p| {
            !p.local && p.copy_of.is_none() && file_in(p, folder).is_some() && lightcraft_catalog::sync::content_key(p).as_deref() == Some(key)
        })
        .map(|p| p.id)
        .collect()
}

/// The `file` an edit record's data names.
fn file_of(data: &str) -> Option<String> {
    serde_json::from_str::<Value>(data).ok()?.get("file")?.as_str().map(str::to_string)
}

/// A name for `to` that no file has yet: `IMG_1.JPG`, else `IMG_1-1.JPG`, `IMG_1-2.JPG`…
fn free_name(to: &Path) -> Option<PathBuf> {
    if !to.exists() {
        return Some(to.to_path_buf());
    }
    let stem = to.file_stem()?.to_string_lossy().to_string();
    let ext = to.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    (1..10_000).map(|k| to.with_file_name(format!("{stem}-{k}{ext}"))).find(|p| !p.exists())
}

impl ICloud {
    /// The activity row of photos coming from iCloud Drive: there while `left` are wanted.
    fn bringing_row(&mut self, activity: &crate::activity::Activity, left: usize) {
        if left == 0 {
            self.bringing = None;
            return;
        }
        let (guard, done) = self.bringing.get_or_insert_with(|| {
            let guard = activity.start("icloud", "Bringing photos from iCloud Drive", crate::activity::Cancel::No);
            (guard, 0)
        });
        guard.progress(*done, done.saturating_add(left as u64));
    }

    /// The activity row of originals moving into iCloud Drive: there while `left` are to move.
    fn moving_row(&mut self, activity: &crate::activity::Activity, left: usize) {
        if left == 0 {
            self.moving = None;
            return;
        }
        let (guard, done) = self.moving.get_or_insert_with(|| {
            let guard = activity.start("icloud", "Moving photos to iCloud Drive", crate::activity::Cancel::No);
            (guard, 0)
        });
        guard.progress(*done, done.saturating_add(left as u64));
    }
}

impl Session {
    /// Where an import copies originals when no destination was chosen: the iCloud Drive folder, with *Keep Photos in
    /// iCloud Drive* on (`None`: the library's `Originals/`).
    pub(crate) fn icloud_import_root(&self) -> Option<PathBuf> {
        let ic = self.icloud.as_ref()?;
        if !ic.saved.photos || self.server_linked() {
            return None;
        }
        ic.folder.clone()
    }

    /// The host's iCloud Drive folder for photos on this device (`None`: iCloud Drive isn't available now).
    pub fn icloud_set_folder(&mut self, folder: Option<PathBuf>) -> Result<()> {
        if let Some(f) = &folder
            && (!f.is_absolute() || !f.is_dir())
        {
            return Err(EngineError::Other(format!("{}: not a folder", f.display())));
        }
        let Some(ic) = self.icloud.as_mut() else { return Err(EngineError::Other("iCloud sync is off for this library".into())) };
        if ic.folder != folder {
            ic.folder = folder;
            // (which photos travel depends on it: all of them are looked at again)
            ic.scanned = false;
            ic.rev = ic.rev.wrapping_add(1);
        }
        Ok(())
    }

    /// Turn *Keep Photos in iCloud Drive* on or off for this library. On needs the host's folder and no server (a server
    /// keeps the photos then).
    pub fn icloud_set_photos(&mut self, on: bool) -> Result<()> {
        let linked = self.server_linked();
        let Some(ic) = self.icloud.as_mut() else { return Err(EngineError::Other("iCloud sync is off for this library".into())) };
        if on && linked {
            return Err(EngineError::Other("this library syncs its photos with your server".into()));
        }
        if on && ic.folder.is_none() {
            return Err(EngineError::Other("iCloud Drive isn't available on this device: turn it on in the system's iCloud settings".into()));
        }
        if ic.saved.photos != on {
            ic.saved.photos = on;
            ic.saved_dirty = true;
            ic.rev = ic.rev.wrapping_add(1);
        }
        if !on {
            ic.bringing = None;
            ic.moving = None;
        }
        Ok(())
    }

    /// The files of the iCloud Drive folder this device needs (`limit` at most): the photos other devices keep there
    /// that this library doesn't have, and this library's own that iCloud Drive took back to save space. `{files:
    /// [{file, key?, id?}], total}`; the host brings each down, then hands it over with `icloud.adopt`. The activity row
    /// shows while the host is `waiting` for them (it stops waiting for files that never come: deleted in iCloud Drive).
    pub fn icloud_wanted(&mut self, limit: usize, waiting: bool) -> Value {
        self.with_icloud(|s, ic| {
            let folder = ic.folder.clone().filter(|_| ic.saved.photos && !s.server_linked());
            let Some(folder) = folder else {
                ic.bringing = None;
                return json!({"files": [], "total": 0});
            };
            let wanted = s.icloud_wanted_files(ic, &folder);
            ic.bringing_row(&s.activity, if waiting { wanted.len() } else { 0 });
            let total = wanted.len();
            json!({"files": wanted.into_iter().take(limit).collect::<Vec<_>>(), "total": total})
        })
        .unwrap_or_else(|| json!({"files": [], "total": 0}))
    }

    fn icloud_wanted_files(&self, ic: &ICloud, folder: &Path) -> Vec<Value> {
        let here: HashSet<String> = self.catalog.photos().filter(|p| !p.local).filter_map(|p| lightcraft_catalog::sync::content_key(p)).collect();
        let mut out: Vec<Value> = ic
            .held
            .values()
            .filter(|h| h.kind == EDIT && !here.contains(&h.key))
            .filter_map(|h| {
                let file = file_of(&h.data)?;
                path_in(folder, &file)?;
                Some(json!({"file": file, "key": h.key}))
            })
            .collect();
        for p in self.catalog.photos().filter(|p| !p.local && p.copy_of.is_none()) {
            // (asked without blocking: the app's cache answers, and checks on a worker)
            if let (Some(file), Source::File { path }) = (file_in(p, folder), &p.source)
                && self.media.availability.is_offline(path)
            {
                out.push(json!({"file": file, "id": p.id.0}));
            }
        }
        out
    }

    /// Files of the iCloud Drive folder the host brought down (relative paths, [`ADOPT_MAX`] at most are taken): a
    /// photo of this library whose file came back is shown from it again; another device's is added to the library
    /// where it is, with that device's edits. `{added, back, missing, refused, left}`.
    pub fn icloud_adopt(&mut self, files: &[String]) -> Result<Value> {
        self.with_icloud(|s, ic| s.icloud_adopt_with(ic, files))
            .unwrap_or_else(|| Err(EngineError::Other("iCloud sync is off for this library".into())))
    }

    fn icloud_adopt_with(&mut self, ic: &mut ICloud, files: &[String]) -> Result<Value> {
        let Some(folder) = ic.folder.clone().filter(|_| ic.saved.photos && !self.server_linked()) else {
            return Err(EngineError::Other("Keep Photos in iCloud Drive is off for this library".into()));
        };
        let (mut back, mut missing, mut refused) = (0usize, 0usize, 0usize);
        let mut fresh: Vec<String> = Vec::new();
        for rel in files.iter().take(ADOPT_MAX) {
            let Some(path) = path_in(&folder, rel) else {
                refused += 1;
                continue;
            };
            let path_s = path.to_string_lossy().to_string();
            // a photo of this library whose file iCloud Drive took back, and that has come again
            let own: Vec<PhotoId> =
                self.catalog.photos().filter(|p| matches!(&p.source, Source::File { path } if *path == path_s)).map(|p| p.id).collect();
            if !path.is_file() {
                missing += 1;
                continue;
            }
            if !own.is_empty() {
                self.media.availability.forget(&path_s);
                for id in own {
                    self.media.forget(id);
                    self.media.note_arrived(id);
                }
                back += 1;
                continue;
            }
            if !fresh.contains(&path_s) {
                fresh.push(path_s);
            }
        }
        let mut added = 0;
        if !fresh.is_empty() {
            // added where it is, as an import in place; not an undo step (Undo would remove it from every device), and redo
            // stays as it was
            let (undo0, redo) = (self.undo.len(), std::mem::take(&mut self.redo));
            let opts = crate::import::ImportOptions { mode: crate::import::ImportMode::Add, ..Default::default() };
            let report = crate::import::import_with(self, &fresh, &opts);
            self.undo.truncate(undo0);
            self.redo = redo;
            let report = report?;
            for id in report.imported.iter().map(|id| PhotoId(*id)) {
                // (it takes the record held for its picture: the other device's edits)
                ic.dirty.insert(id);
                self.icloud_refresh_photo(ic, id);
                added += 1;
            }
        }
        let left = self.icloud_wanted_files(ic, &folder).len();
        if let Some((guard, done)) = ic.bringing.as_mut() {
            *done = done.saturating_add((added + back) as u64);
            guard.progress(*done, done.saturating_add(left as u64));
        }
        ic.bringing_row(&self.activity, left);
        ic.rev = ic.rev.wrapping_add(1);
        Ok(json!({"added": added, "back": back, "missing": missing, "refused": refused, "left": left}))
    }

    /// The photos whose original the library keeps itself (in `Originals/`) and that could move into the iCloud Drive
    /// folder: not opened from the Photos library (iCloud Photos has those), not virtual copies. Their ids and sizes.
    fn icloud_movable(&self) -> Vec<(PhotoId, String, u64)> {
        let Some(originals) = self.library.as_ref().filter(|l| l.on_disk).map(|l| l.originals_dir()) else { return Vec::new() };
        self.catalog
            .photos()
            .filter(|p| !p.local && p.copy_of.is_none() && !self.photo_library.has(p.id))
            .filter_map(|p| match &p.source {
                Source::File { path } if Path::new(path).starts_with(&originals) => Some((p.id, path.clone(), p.file_size)),
                _ => None,
            })
            .collect()
    }

    /// How many photos could move into iCloud Drive, and their bytes.
    pub(super) fn icloud_movable_count(&self) -> (usize, u64) {
        let m = self.icloud_movable();
        (m.len(), m.iter().map(|(_, _, b)| *b).sum())
    }

    /// Move up to `limit` ([`MOVE_MAX`] at most) of the originals the library keeps itself into the iCloud Drive folder,
    /// keeping their place (`YYYY/YYYY-MM-DD/name`): each file with its sidecars, then its photo (and virtual copies)
    /// pointed there. Not an undo step. `{moved, failed: [{file, error}], left, bytesLeft}`.
    pub fn icloud_move_in(&mut self, limit: usize) -> Result<Value> {
        let folder = self.icloud_import_root().ok_or_else(|| EngineError::Other("Keep Photos in iCloud Drive is off for this library".into()))?;
        let originals = self.library.as_ref().map(|l| l.originals_dir()).ok_or_else(|| EngineError::Other("no library is open".into()))?;
        // (a file that isn't there can't move: Find Missing Photos is the way to it)
        let mut seen = HashSet::new();
        let paths: Vec<String> = self
            .icloud_movable()
            .into_iter()
            .map(|(_, path, _)| path)
            .filter(|path| seen.insert(path.clone()))
            .filter(|path| Path::new(path).is_file())
            .take(limit.clamp(1, MOVE_MAX))
            .collect();
        // every photo of each file, virtual copies included
        let mut of_path: std::collections::HashMap<&str, Vec<PhotoId>> = paths.iter().map(|p| (p.as_str(), Vec::new())).collect();
        for p in self.catalog.photos() {
            if let Source::File { path } = &p.source
                && let Some(ids) = of_path.get_mut(path.as_str())
            {
                ids.push(p.id);
            }
        }
        let mut ops = Vec::new();
        let (mut moved, mut failed, mut files) = (Vec::new(), Vec::new(), 0usize);
        for from in &paths {
            let Some(rel) = Path::new(from).strip_prefix(&originals).ok().map(Path::to_path_buf) else { continue };
            let Some(to) = free_name(&folder.join(&rel)) else {
                failed.push(json!({"file": from, "error": "no free name in iCloud Drive"}));
                continue;
            };
            if let Some(parent) = to.parent()
                && let Err(e) = std::fs::create_dir_all(parent)
            {
                failed.push(json!({"file": from, "error": e.to_string()}));
                continue;
            }
            let to_s = to.to_string_lossy().to_string();
            let went = match crate::rename::move_file(from, &to_s) {
                Ok(()) => true,
                // (the file moved but a sidecar couldn't follow, nor the file go back: the photo is where its file is)
                Err(e) => {
                    failed.push(json!({"file": from, "error": e.message}));
                    e.moved
                }
            };
            if went {
                files += 1;
                let name = to.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                for id in of_path.get(from.as_str()).into_iter().flatten() {
                    ops.push(Op::Relink { id: *id, file_name: name.clone(), source: Source::File { path: to_s.clone() }, format: None });
                    moved.push(*id);
                }
            }
        }
        if !ops.is_empty() {
            self.apply_system(Op::Batch { ops })?;
            for id in &moved {
                self.media.forget(*id);
            }
        }
        // (counted without looking at the disk; a host stops when a call moves nothing)
        let (left, bytes_left) = self.icloud_movable_count();
        let activity = self.activity.clone();
        if let Some(ic) = self.icloud.as_mut() {
            if let Some((guard, done)) = ic.moving.as_mut() {
                *done = done.saturating_add(files as u64);
                guard.progress(*done, done.saturating_add(left as u64));
            }
            ic.moving_row(&activity, left);
            ic.rev = ic.rev.wrapping_add(1);
        }
        Ok(json!({"moved": files, "failed": failed, "left": left, "bytesLeft": bytes_left}))
    }
}
