//! Keeping the originals downloaded from the server under a size budget
//! ([`SyncConfig::originals_budget_mb`](super::SyncConfig::originals_budget_mb)): the ones used
//! longest ago go first, and a photo whose original goes is the server's again (its smart and mini
//! previews stay; Download Originals brings the original back).
//!
//! Only files this folder holds are ever deleted — `<library>/sync/originals/<hash>/…`, which
//! sync made — and only when the server has the photo's original (so a deletion never loses the
//! last copy). A photo that is open, selected or kept offline keeps its original.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use lightcraft_catalog::{Op, PhotoId, Source};

use super::{Job, SyncState, blob_key, content_path};
use crate::Session;

/// What eviction leaves of the budget, so it doesn't run again after the next download.
const LEAVE: u64 = 90;

/// A downloaded original: `<originals>/<hash>/<name>`.
struct Held {
    key: String,
    dir: PathBuf,
    bytes: u64,
    /// When it was last opened or downloaded.
    used: SystemTime,
}

/// What an originals folder holds, one entry per hash (a download that hasn't finished — `.part` —
/// is not an original yet).
fn held(dir: &Path) -> Vec<Held> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    rd.flatten()
        .filter_map(|e| {
            let key = e.file_name().to_string_lossy().to_string();
            // (only what a download made: a folder named by a content hash)
            if key.len() != 32 || !key.bytes().all(|b| b.is_ascii_hexdigit()) || !e.path().is_dir() {
                return None;
            }
            let (mut bytes, mut used) = (0u64, SystemTime::UNIX_EPOCH);
            for f in std::fs::read_dir(e.path()).ok()?.flatten() {
                let m = f.metadata().ok()?;
                if !m.is_file() || f.file_name().to_string_lossy().ends_with(".part") {
                    continue;
                }
                bytes = bytes.saturating_add(m.len());
                used = used.max(m.modified().unwrap_or(SystemTime::UNIX_EPOCH));
            }
            (bytes > 0).then_some(Held { key, dir: e.path(), bytes, used })
        })
        .collect()
}

impl Session {
    /// The photo being worked on has been used now: its original (if downloaded) is the last to go.
    pub(super) fn touch_active_original(&mut self, st: &mut SyncState) {
        let active = self.active();
        if active == st.touched {
            return;
        }
        st.touched = active;
        let (Some(dir), Some(p)) = (self.originals_dir(), active.and_then(|id| self.catalog.photo(id))) else { return };
        if let Source::File { path } = &p.source
            && Path::new(path).starts_with(&dir)
            && let Ok(f) = std::fs::OpenOptions::new().append(true).open(path)
        {
            let _ = f.set_modified(SystemTime::now());
        }
    }

    /// Delete downloaded originals, longest unused first, until they take no more than
    /// [`LEAVE`] percent of the budget. Returns how many went. Nothing is deleted without a
    /// budget, with "keep every original on this device" on, or when the originals aren't on a disk.
    pub(super) fn evict_originals(&mut self, st: &mut SyncState) -> usize {
        let Some(mb) = st.config.originals_budget_mb.filter(|_| !st.config.store_originals) else { return 0 };
        let Some(dir) = self.originals_dir() else { return 0 };
        let budget = mb.saturating_mul(1 << 20);
        let mut all = held(&dir);
        let mut total: u64 = all.iter().map(|h| h.bytes).fold(0, u64::saturating_add);
        if total <= budget {
            return 0;
        }
        let target = budget / 100 * LEAVE;
        // the photos whose original this device needs: kept offline, open, selected
        let mut need: HashSet<PhotoId> = st.config.offline_photos.iter().copied().collect();
        for a in &st.config.offline_albums {
            need.extend(self.catalog.album_photos(*a));
        }
        need.extend(self.selection.ids.iter().copied());
        need.extend(self.active());
        let pinned: HashSet<String> = need.iter().filter_map(|id| self.catalog.photo(*id)).filter_map(|p| blob_key(p)).collect();
        all.sort_by_key(|h| h.used);
        let mut gone = 0;
        for h in all {
            if total <= target {
                break;
            }
            let downloading = st.jobs.values().any(|j| matches!(j, Job::Get { key, .. } if *key == h.key));
            if pinned.contains(&h.key) || !st.uploaded.contains(&h.key) || downloading {
                continue;
            }
            // the photos that point into this folder go back to being the server's
            let back: Vec<(Op, String)> = self
                .catalog
                .photos()
                .filter(|p| blob_key(p).as_deref() == Some(h.key.as_str()))
                .filter_map(|p| match &p.source {
                    Source::File { path } if Path::new(path).starts_with(&h.dir) => Some((
                        Op::Relink { id: p.id, file_name: p.file_name.clone(), source: Source::File { path: content_path(p)? }, format: None },
                        path.clone(),
                    )),
                    _ => None,
                })
                .collect();
            if let Err(e) = std::fs::remove_dir_all(&h.dir) {
                log::warn!("sync: can't remove the original {}: {e}", h.dir.display());
                continue;
            }
            total = total.saturating_sub(h.bytes);
            for (op, old) in back {
                let id = match &op {
                    Op::Relink { id, .. } => *id,
                    _ => continue,
                };
                if self.catalog.apply(op.clone()).is_ok() {
                    self.pending_log.push(op);
                }
                self.media.availability.forget(&old);
                self.media.forget(id);
            }
            st.want_originals.remove(&h.key);
            gone += 1;
        }
        if gone > 0 {
            st.plan_gen += 1;
            st.planned = None;
        }
        gone
    }
}
