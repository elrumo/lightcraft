//! Live Photos: the few seconds of motion a phone records around a photo, kept with it.
//!
//! A Live Photo is a still (a HEIC or JPEG) and a short QuickTime clip of the moments around it. The still is the
//! photo: imported, edited, synced and exported as any other. Its clip, the **motion**, is kept in the library
//! folder's `Motion/` and recorded in `live-photos.json` (photo id → clip). A frontend plays it over the photo
//! (`live.info` says where it is) and hands both to a system photo library when it saves the photo as a Live Photo.
//!
//! - **This device's own.** The clip is not synced yet: other devices, and the sync server, have the still alone.
//! - **As recorded.** Develop settings are not applied to the clip (video frames are not developed yet, see
//!   `docs/video.md`).
//! - **No paths.** The app's folder moves with every iOS update (`photo_library.rs`), so the entry keeps the clip's
//!   name in `Motion/` only. It also keeps the still's content hash: an entry whose photo no longer has it (an id can
//!   name another photo after a sign-out) is stale.
//! - **Removed late.** A photo deleted for good keeps its clip until the library next opens: until then Undo can bring
//!   the photo back, with its motion. A photo opened from the photo library and left unchanged takes its clip with it
//!   when it is forgotten.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use lightcraft_catalog::{MediaKind, Photo, PhotoId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::cmd::{CommandSpec, always, bad, cmd, str_param};
use crate::{Result, Session};

/// The folder in the library that holds the clips.
pub const MOTION_DIR: &str = "Motion";
/// The longest clip taken as a photo's motion (a phone's is about 3 s; a long exposure's clip is still a few).
const MAX_SECONDS: f64 = 60.0;
/// The largest clip file taken.
const MAX_BYTES: u64 = 512 << 20;
/// What a clip's file may be.
const EXTENSIONS: [&str; 4] = ["mov", "mp4", "m4v", "qt"];
/// Longest content identifier kept (Apple's are UUIDs, 36 characters).
const MAX_CONTENT_ID: usize = 256;

/// A photo's motion clip.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Motion {
    /// The still's content hash when the clip was kept with it.
    pub(crate) hash: Option<String>,
    /// The clip's file name in `Motion/`.
    file: String,
    /// Its length in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    duration: Option<f64>,
    /// When in the clip the still was taken (seconds), if the frontend knew.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    still_time: Option<f64>,
    /// What pairs the still and the clip in a system photo library (Apple's content identifier, in the still's maker
    /// notes and the clip's metadata), if the frontend knew: saving the two to such a library again needs it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    content_id: Option<String>,
}

/// Photo → motion, saved as `live-photos.json`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LivePhotos {
    pub(crate) photos: BTreeMap<u64, Motion>,
    #[serde(skip)]
    dirty: bool,
}

impl LivePhotos {
    pub const FILE: &'static str = "live-photos.json";

    pub fn from_json(bytes: &[u8]) -> LivePhotos {
        serde_json::from_slice(bytes).unwrap_or_default()
    }

    /// `p` is a Live Photo here: it has a clip, kept with the content it has now.
    pub fn is_live(&self, p: &Photo) -> bool {
        !self.photos.is_empty() && self.photos.get(&p.id.0).is_some_and(|m| m.hash == p.content_hash)
    }

    fn take(&mut self, id: PhotoId) -> Option<Motion> {
        let m = self.photos.remove(&id.0);
        self.dirty |= m.is_some();
        m
    }
}

/// What a motion clip is, read and checked before it is kept.
struct Clip {
    path: PathBuf,
    ext: String,
    duration: Option<f64>,
}

/// Check that `path` is a clip that can be a photo's motion.
fn check_clip(path: &Path, c: &str) -> Result<Clip> {
    let shown = path.display();
    let meta = std::fs::metadata(path).map_err(|e| bad(c, format!("{shown}: {e}")))?;
    if !meta.is_file() {
        return Err(bad(c, format!("{shown}: not a file")));
    }
    if meta.len() > MAX_BYTES {
        return Err(bad(c, format!("{shown}: larger than a Live Photo's motion can be")));
    }
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if !EXTENSIONS.contains(&ext.as_str()) {
        return Err(bad(c, format!("{shown}: not a QuickTime or MP4 clip")));
    }
    #[cfg(not(target_arch = "wasm32"))]
    let duration = {
        let info = lightcraft_video::probe_path(path).map_err(|e| bad(c, format!("{shown}: {e}")))?;
        if !info.duration.is_finite() || info.duration <= 0.0 || info.duration > MAX_SECONDS {
            return Err(bad(c, format!("{shown}: {:.1} s is not the length of a Live Photo's motion", info.duration)));
        }
        Some(info.duration)
    };
    #[cfg(target_arch = "wasm32")]
    let duration = None;
    Ok(Clip { path: path.to_path_buf(), ext, duration })
}

/// Move `from` to `to` (a copy and a removal across volumes).
fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to)?;
    if let Err(e) = std::fs::remove_file(from) {
        log::warn!("live photo: remove {}: {e}", from.display());
    }
    Ok(())
}

fn remove_file(path: &Path) {
    if let Err(e) = std::fs::remove_file(path)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        log::warn!("live photo: remove {}: {e}", path.display());
    }
}

/// A content identifier as a frontend sent it: printable, not too long, else none.
fn content_id_param(p: &Value) -> Option<String> {
    str_param(p, "contentId")
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.len() <= MAX_CONTENT_ID && !s.chars().any(char::is_control))
        .map(str::to_string)
}

/// A still time as a frontend sent it: a number of seconds inside the clip, else none.
fn still_time_param(p: &Value) -> Option<f64> {
    p.get("stillTime").and_then(Value::as_f64).filter(|t| t.is_finite() && (0.0..=MAX_SECONDS).contains(t))
}

/// How a clip is kept with its photo: moved (the app's staged copy) or copied (a file of the person's own).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keep {
    Move,
    Copy,
}

impl Session {
    /// The library's `Motion/` folder (`None` for a library that is not saved on disk).
    fn motion_dir(&self) -> Option<PathBuf> {
        self.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.join(MOTION_DIR))
    }

    /// Save `live-photos.json` if it changed (a failure is logged: at worst a clip is missing from the next open, and
    /// its file is tidied away).
    pub(crate) fn save_live_photos(&mut self) {
        if !self.live_photos.dirty {
            return;
        }
        let Some(lib) = self.library.as_mut() else { return };
        match serde_json::to_vec(&self.live_photos) {
            Ok(bytes) => match lib.files_mut().write_atomic(LivePhotos::FILE, &bytes) {
                Ok(()) => self.live_photos.dirty = false,
                Err(e) => log::error!("library: {}: {e}", LivePhotos::FILE),
            },
            Err(e) => log::error!("library: {}: {e}", LivePhotos::FILE),
        }
    }

    /// Keep the clip at `path` as the motion of photo `id` (replacing the one it had). The photo must be a still (a
    /// picture or a raw, not a video) in a library saved on disk. Returns its length in seconds, when read.
    pub fn attach_motion(
        &mut self,
        id: PhotoId,
        path: &Path,
        keep: Keep,
        content_id: Option<String>,
        still_time: Option<f64>,
    ) -> Result<Option<f64>> {
        const C: &str = "live.attach";
        let photo = self.catalog.photo(id).ok_or_else(|| bad(C, format!("no photo {}", id.0)))?;
        if photo.kind == MediaKind::Video {
            return Err(bad(C, format!("photo {} is a video: a Live Photo's motion goes with its still", id.0)));
        }
        if photo.copy_of.is_some() {
            return Err(bad(C, format!("photo {} is a virtual copy: its master keeps the motion", id.0)));
        }
        let hash = photo.content_hash.clone();
        let dir = self.motion_dir().ok_or_else(|| bad(C, "needs a library saved on disk"))?;
        let clip = check_clip(path, C)?;
        std::fs::create_dir_all(&dir).map_err(|e| bad(C, format!("{}: {e}", dir.display())))?;
        let file = format!("{}.{}", id.0, clip.ext);
        let dst = dir.join(&file);
        // (the clip it had goes, unless it is the same file under another name)
        if let Some(old) = self.live_photos.take(id)
            && old.file != file
        {
            remove_file(&dir.join(&old.file));
        }
        let placed = match keep {
            Keep::Move => move_file(&clip.path, &dst),
            Keep::Copy => std::fs::copy(&clip.path, &dst).map(|_| ()),
        };
        placed.map_err(|e| bad(C, format!("{}: {e}", clip.path.display())))?;
        self.live_photos.photos.insert(id.0, Motion { hash, file, duration: clip.duration, still_time, content_id });
        self.live_photos.dirty = true;
        self.save_live_photos();
        Ok(clip.duration)
    }

    /// Photo `id`'s motion goes (its record and its file). Whether it had one.
    pub(crate) fn detach_motion(&mut self, id: PhotoId) -> bool {
        let Some(m) = self.live_photos.take(id) else { return false };
        if let Some(dir) = self.motion_dir() {
            remove_file(&dir.join(&m.file));
        }
        self.save_live_photos();
        true
    }

    /// Keep each import's clip with the photo its still became (`pairs`: photo, clip). A clip that can't be kept is
    /// logged: the photo is imported all the same, as a still. Returns how many photos have their motion.
    pub(crate) fn attach_imported_motions(&mut self, pairs: &[(PhotoId, String)], keep: Keep) -> usize {
        let mut n = 0;
        for (id, clip) in pairs {
            match self.attach_motion(*id, Path::new(clip), keep, None, None) {
                Ok(_) => n += 1,
                Err(e) => log::warn!("import: the motion of photo {}: {e}", id.0),
            }
        }
        n
    }

    /// When a library opens (and after a sign-out emptied it): clips whose photo is gone, or is another photo now, go;
    /// so do files in `Motion/` no entry names (a crash between placing a clip and saving the list).
    pub(crate) fn tidy_live_photos(&mut self) {
        let stale: Vec<u64> = self
            .live_photos
            .photos
            .iter()
            .filter(|(id, m)| self.catalog.photo(PhotoId(**id)).is_none_or(|p| p.content_hash != m.hash))
            .map(|(id, _)| *id)
            .collect();
        let dir = self.motion_dir();
        for id in stale {
            if let (Some(m), Some(dir)) = (self.live_photos.take(PhotoId(id)), dir.as_ref()) {
                remove_file(&dir.join(&m.file));
            }
        }
        self.save_live_photos();
        let Some(dir) = dir else { return };
        let Ok(entries) = std::fs::read_dir(&dir) else { return };
        let kept: std::collections::HashSet<&str> = self.live_photos.photos.values().map(|m| m.file.as_str()).collect();
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if e.file_type().is_ok_and(|t| t.is_file()) && !kept.contains(name.as_str()) {
                remove_file(&e.path());
            }
        }
    }

    /// `id`'s motion as a frontend plays it: `{id, path, duration, stillTime, contentId}`, or `null` when it has none.
    fn motion_json(&self, id: PhotoId) -> Value {
        let (Some(photo), Some(dir)) = (self.catalog.photo(id), self.motion_dir()) else { return Value::Null };
        if !self.live_photos.is_live(photo) {
            return Value::Null;
        }
        let Some(m) = self.live_photos.photos.get(&id.0) else { return Value::Null };
        let path = dir.join(&m.file);
        if !path.is_file() {
            return Value::Null;
        }
        json!({"id": id.0, "path": path.to_string_lossy(), "duration": m.duration, "stillTime": m.still_time, "contentId": m.content_id})
    }
}

/// The photo a command is about: `id`, else the active one.
fn photo_param(s: &Session, p: &Value, c: &str) -> Result<PhotoId> {
    match p.get("id").filter(|v| !v.is_null()) {
        Some(v) => v.as_u64().map(PhotoId).ok_or_else(|| bad(c, "`id` must be a photo id")),
        None => s.active().ok_or_else(|| bad(c, "no photo selected: pass `id`")),
    }
}

/// A staged file the app handed over is moved; anything else is copied (never removed).
fn keep_for(s: &Session, path: &Path) -> Keep {
    if crate::photo_library::inside_staging(s, path) { Keep::Move } else { Keep::Copy }
}

fn attach(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "live.attach";
    let id = photo_param(s, p, C)?;
    let path = PathBuf::from(str_param(p, "path").ok_or_else(|| bad(C, "missing `path`: the clip"))?);
    let keep = keep_for(s, &path);
    s.attach_motion(id, &path, keep, content_id_param(p), still_time_param(p))?;
    Ok(s.motion_json(id))
}

fn info(s: &mut Session, p: &Value) -> Result<Value> {
    let id = photo_param(s, p, "live.info")?;
    Ok(s.motion_json(id))
}

fn detach(s: &mut Session, p: &Value) -> Result<Value> {
    let id = photo_param(s, p, "live.detach")?;
    Ok(json!({"removed": s.detach_motion(id)}))
}

/// `photoLibrary.open`'s `motion`: the asset's clip, kept with the photo it opened as (when that one has none yet).
/// Never fails the open: a clip that can't be kept is logged and dropped. Whether the photo is a Live Photo now.
pub(crate) fn open_with_motion(s: &mut Session, id: PhotoId, p: &Value) -> bool {
    let live = s.catalog.photo(id).is_some_and(|ph| s.live_photos.is_live(ph));
    let Some(path) = str_param(p, "motion").map(PathBuf::from) else { return live };
    let keep = keep_for(s, &path);
    if live {
        if keep == Keep::Move {
            remove_file(&path);
        }
        return true;
    }
    match s.attach_motion(id, &path, keep, content_id_param(p), still_time_param(p)) {
        Ok(_) => true,
        Err(e) => {
            log::warn!("photo library: the motion of photo {}: {e}", id.0);
            if keep == Keep::Move {
                remove_file(&path);
            }
            false
        }
    }
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        // (not journaled: the clip is this device's, and so is the list of them)
        cmd!(
            query "live.attach",
            "Keep Live Photo Motion",
            [],
            None,
            "{id?, path, contentId?, stillTime?}: the clip at `path` (MOV / MP4, up to 60 s) becomes the photo's Live Photo motion, kept in the library's Motion/ folder (moved from the app's staging folder, else copied); this device's only, not synced; → live.info's answer",
            always,
            attach
        ),
        cmd!(
            query "live.info",
            "Live Photo Motion",
            [],
            None,
            "{id?} → {id, path, duration, stillTime, contentId} of the photo's Live Photo motion (a clip a system player plays as recorded: develop settings are not applied to it), or null",
            always,
            info
        ),
        cmd!(query "live.detach", "Remove Live Photo Motion", [], None, "{id?} → {removed}: the photo's motion clip goes; the photo stays, as a still", always, detach),
    ]
}
