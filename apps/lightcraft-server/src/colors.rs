//! The colours of each user's photos, so `GET /api/search?q=red` finds the photos that are mostly red.
//!
//! No model and no download, so nothing for an admin to switch on: the worker works out each photo's palette
//! ([`lightcraft_vision::colors`]) from the mini preview the server keeps, a few hundred at a time between the
//! other work, and `GET /api/search` understands colour words in the query (see [`crate::vision::search`]). A device
//! never has to send them: they cost milliseconds. The palettes are derived data in
//! `<data>/users/<name>/search/colors-<engine>.bin`, safe to delete.
//!
//! ```text
//! DELETE /api/index/colors    forget this user's colours; the worker makes them again
//! ```

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use lightcraft_catalog::Photo;
use lightcraft_engine::guard;
use lightcraft_vision::colors::{self, ColorIndex};
use lightcraft_vision::{Error as VisionError, Key};

use crate::State;
use crate::accounts;
use crate::api;

/// Photos looked at in one pass over a user.
const CHUNK: usize = 400;
/// Long edge of the rendering a palette is made from.
const EDGE: usize = lightcraft_engine::vision::THUMB_EDGE;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The users' palettes.
#[derive(Default)]
pub struct Colors {
    indexes: Mutex<HashMap<String, Arc<Mutex<ColorIndex>>>>,
    /// Photos that could not be rendered (not retried until the server restarts).
    failed: Mutex<HashSet<(String, Key)>>,
}

impl Colors {
    /// A user's palettes, opened if needed (another version's, or damaged, are derived data: replaced). Without
    /// `create`, a user who has none yet gets `None` and no file is made.
    pub(crate) fn index(&self, data: &Path, user: &str, create: bool) -> Result<Option<Arc<Mutex<ColorIndex>>>, String> {
        let mut all = lock(&self.indexes);
        if let Some(ix) = all.get(user) {
            return Ok(Some(ix.clone()));
        }
        let dir = accounts::user_dir(data, user).join("search");
        let path = dir.join(format!("colors-{}.bin", colors::ENGINE));
        if !create && !path.is_file() {
            return Ok(None);
        }
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let ix = match ColorIndex::open(&path, colors::ENGINE) {
            Ok(ix) => ix,
            Err(VisionError::Mismatch { .. } | VisionError::Format(_)) => {
                log::info!("{}: the colours are another version's or damaged: starting over", path.display());
                let _ = std::fs::remove_file(&path);
                ColorIndex::open(&path, colors::ENGINE).map_err(|e| format!("{}: {e}", path.display()))?
            }
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        let ix = Arc::new(Mutex::new(ix));
        all.insert(user.to_string(), ix.clone());
        Ok(Some(ix))
    }

    /// How many of `user`'s photos have a palette.
    pub(crate) fn analysed(&self, data: &Path, user: &str) -> usize {
        self.index(data, user, false).ok().flatten().map_or(0, |ix| lock(&ix).len())
    }

    /// Forget `user`'s palettes: the file goes, memory lets go of them and the photos that couldn't be done are tried again.
    pub(crate) fn reset(&self, data: &Path, user: &str) -> Result<bool, String> {
        lock(&self.indexes).remove(user);
        lock(&self.failed).retain(|(u, _)| u != user);
        let path = accounts::user_dir(data, user).join("search").join(format!("colors-{}.bin", colors::ENGINE));
        let existed = path.is_file();
        if existed {
            std::fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        log::info!("{user}: colours forgotten");
        Ok(existed)
    }
}

/// Works out the palettes of up to [`CHUNK`] of a user's photos that have a mini preview and no palette yet. True when
/// there are more.
pub(crate) fn run_user(st: &Arc<State>, user: &str) -> Result<bool, String> {
    let v = &st.vision;
    let c = &v.colors;
    let lib = api::lib(st, user)?;
    let (photos, blobs) = {
        let l = lock(&lib);
        (crate::vision::hashed(&l), l.dir.join("blobs"))
    };
    if photos.is_empty() {
        return Ok(false);
    }
    let index = c.index(&st.data, user, true)?.ok_or("the colours are unavailable")?;
    let failed = lock(&c.failed).clone();
    let mut work: Vec<(Key, Arc<Photo>, PathBuf)> = Vec::new();
    {
        let ix = lock(&index);
        for (key, photo) in photos {
            if ix.contains(&key) || failed.contains(&(user.to_string(), key)) {
                continue;
            }
            // (no mini preview yet: the search worker's turn comes after the previews are made)
            if let Some(path) = photo.content_hash.as_deref().and_then(|h| api::blob_path(&blobs, "mini", h)).filter(|p| p.is_file()) {
                work.push((key, photo, path));
            }
        }
    }
    let more = work.len() > CHUNK;
    work.truncate(CHUNK);
    let started = std::time::Instant::now();
    let mut done = 0;
    for (key, photo, path) in &work {
        if v.stopping() {
            return Ok(false);
        }
        let palette = lightcraft_engine::vision::neutral_from_preview(photo, path, EDGE)
            .and_then(|img| guard::catch("reading colours", || colors::extract(&img)));
        match palette {
            Ok(p) if lock(&index).insert(*key, p).is_ok() => done += 1,
            Ok(_) => {
                lock(&c.failed).insert((user.to_string(), *key));
            }
            Err(e) => {
                log::debug!("{user}: can't read the colours of {}: {e}", path.display());
                lock(&c.failed).insert((user.to_string(), *key));
            }
        }
    }
    if done > 0 {
        log::info!("{user}: colours of {done} of {} photo(s) in {:.1} s", work.len(), started.elapsed().as_secs_f64());
        lightcraft_engine::memory::release();
    }
    Ok(more)
}
