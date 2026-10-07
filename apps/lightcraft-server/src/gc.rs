//! `lightcraft-server gc`: delete photo files no library refers to any more (photos removed for
//! good), and uploads that never finished. Files younger than [`KEEP_NEW`] stay: a device
//! uploads a photo's files before the change that adds the photo reaches the server.

use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, SystemTime};

use lightcraft_catalog::sync::ServerCore;
use lightcraft_catalog::{MemStore, Store};

use crate::accounts;

/// New files are never collected.
pub const KEEP_NEW: Duration = Duration::from_secs(24 * 3600);

#[derive(Debug, Default, PartialEq)]
pub struct Report {
    pub kept: usize,
    pub removed: usize,
    pub bytes: u64,
    pub errors: Vec<String>,
}

/// The content hashes a user's library refers to. Reads a copy of the log, so it can run while
/// the server does.
fn referenced(user_dir: &Path) -> Result<HashSet<String>, String> {
    let lib = user_dir.join("library");
    let mut mem = MemStore::new();
    for name in ["catalog.snap", "catalog.log"] {
        if let Ok(b) = std::fs::read(lib.join(name)) {
            mem.write_atomic(name, &b).map_err(|e| e.to_string())?;
        }
    }
    let core = ServerCore::open(Box::new(mem)).map_err(|e| format!("{}: {e}", lib.display()))?;
    Ok(core.catalog().photos().filter_map(|p| p.content_hash.as_deref()?.split(':').next().map(str::to_ascii_lowercase)).collect())
}

fn old(meta: &std::fs::Metadata, now: SystemTime) -> bool {
    meta.modified().ok().and_then(|t| now.duration_since(t).ok()).is_some_and(|age| age > KEEP_NEW)
}

/// Collect every user's unreferenced files (`dry_run`: only count them).
pub fn run(data: &Path, dry_run: bool) -> Result<Report, String> {
    let users = accounts::read_users(data)?;
    let now = SystemTime::now();
    let mut r = Report::default();
    for name in users.users.keys() {
        let dir = accounts::user_dir(data, name);
        let keep = match referenced(&dir) {
            Ok(k) => k,
            Err(e) => {
                // a library that can't be read keeps all its files
                r.errors.push(e);
                continue;
            }
        };
        let blobs = dir.join("blobs");
        let remove = |path: &Path, meta: &std::fs::Metadata, r: &mut Report| {
            r.removed += 1;
            r.bytes += meta.len();
            if !dry_run && let Err(e) = std::fs::remove_file(path) {
                r.errors.push(format!("{}: {e}", path.display()));
            }
        };
        for kind in lightcraft_catalog::sync::proto::BLOB_KINDS {
            let Ok(shards) = std::fs::read_dir(blobs.join(kind)) else { continue };
            for shard in shards.flatten() {
                let Ok(files) = std::fs::read_dir(shard.path()) else { continue };
                for f in files.flatten() {
                    let Ok(meta) = f.metadata() else { continue };
                    let hash = f.file_name().to_string_lossy().to_string();
                    if keep.contains(&hash) || !old(&meta, now) {
                        r.kept += 1;
                    } else {
                        remove(&f.path(), &meta, &mut r);
                    }
                }
            }
        }
        if let Ok(tmp) = std::fs::read_dir(blobs.join("tmp")) {
            for f in tmp.flatten() {
                if let Ok(meta) = f.metadata()
                    && old(&meta, now)
                {
                    remove(&f.path(), &meta, &mut r);
                }
            }
        }
    }
    Ok(r)
}
