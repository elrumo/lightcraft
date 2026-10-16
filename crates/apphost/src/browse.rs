//! Showing a folder in Local (`library.browse`) without stalling the frontend's thread, shared by the egui app and
//! the engine host: the view switches at once (the photos the library already knows from the folder show), the
//! folder is listed and its new files probed on a worker thread (a network share can take minutes), they join the
//! view as Local records in batches, and then the sidecars another app changed are read again and the folder is noted
//! as browsed. One activity row says so the whole time (✕ stops it). The engine's `library.browse` does the same in one
//! go (CLI, MCP).

use lightcraft_engine::activity::{Cancel, TaskGuard};
use lightcraft_engine::{LibrarySource, Session};
use serde_json::{Value, json};

use crate::import::{Run, Scan, Tally, commit};

/// The activity row's label.
pub const LABEL: &str = "Reading folder";

#[allow(clippy::large_enum_variant)] // one per browse, never in a collection
enum Stage {
    /// Listing the folder and probing what is new in it.
    Scan(Scan),
    /// Adding the new files as Local records.
    Add { run: Run, guard: TaskGuard },
    /// Answered (its activity row has gone with the stage before).
    Done,
}

/// A browse under way: [`Browse::poll`] it between frames (or requests) until it answers.
pub struct Browse {
    pub path: String,
    pub subfolders: bool,
    stage: Stage,
    undo0: usize,
    listed: Vec<String>,
    forgotten: usize,
    tally: Tally,
}

impl Browse {
    /// Show `path` (with the folders inside it when `subfolders`; default: as the last browse did).
    pub fn start(session: &mut Session, path: &str, subfolders: Option<bool>) -> Result<Browse, String> {
        let dir = std::path::absolute(std::path::Path::new(path)).map_err(|e| e.to_string())?;
        if !dir.is_dir() {
            return Err(format!("{path}: not a folder"));
        }
        let dir_s = dir.to_string_lossy().trim_end_matches(['/', '\\']).to_string();
        let subfolders = subfolders.unwrap_or_else(|| session.browse.as_ref().is_some_and(|b| b.subfolders));
        session.browse = Some(lightcraft_engine::Browse { path: dir_s.clone(), subfolders });
        session.source = LibrarySource::Folder;
        session.keep_active_in_view();
        // the folder itself (the scan lists everything below it), or just its own files
        // ponytail: the files of one folder are listed on this thread; on the worker too if a share is slow to list
        let paths = if subfolders { vec![dir_s.clone()] } else { own_files(&dir) };
        let scan = Scan::start(session, &paths, LABEL);
        Ok(Browse {
            path: dir_s,
            subfolders,
            stage: Stage::Scan(scan),
            undo0: session.undo.len(),
            listed: Vec::new(),
            forgotten: 0,
            tally: Tally::default(),
        })
    }

    /// Whether it shows `path` the same way (asked again: nothing to restart).
    pub fn same(&self, path: &str, subfolders: Option<bool>) -> bool {
        std::path::absolute(path).is_ok_and(|d| d.to_string_lossy().trim_end_matches(['/', '\\']) == self.path)
            && subfolders.is_none_or(|s| s == self.subfolders)
    }

    /// Take the next step. `None` while it runs; then `library.browse`'s answer (`{path, subfolders, photos, new,
    /// reread, failed, forgotten}`), or an error (`cancelled` when ✕ stopped it).
    pub fn poll(&mut self, session: &mut Session) -> Option<Result<Value, String>> {
        match &mut self.stage {
            Stage::Done => Some(Err("the folder has been read".into())),
            Stage::Scan(scan) => {
                if scan.cancelled() {
                    return Some(Err("cancelled".into()));
                }
                let out = match scan.poll("Looking for photos…")? {
                    Ok(out) => out,
                    Err(e) => return Some(Err(e)),
                };
                session.import_probes = out.probes;
                self.listed = out.candidates.iter().map(|c| c.path.clone()).collect();
                // records of files another app deleted or moved, that nothing was changed on, go (issue #1392)
                self.forgotten = session.forget_gone_from(&self.path, self.subfolders, &self.listed);
                let queue: Vec<String> =
                    out.candidates.iter().filter(|c| c.duplicate.as_deref() != Some("path") && c.error.is_none()).map(|c| c.path.clone()).collect();
                if queue.is_empty() {
                    self.stage = Stage::Done;
                    return Some(self.finish(session));
                }
                let run = match Run::begin(session, queue, &json!({"mode": "add", "local": true, "onDeleted": "skip"}), None) {
                    Ok(run) => run,
                    Err(e) => return Some(Err(e)),
                };
                let guard = session.activity.start("import", LABEL, Cancel::Flag(run.cancel.clone()));
                self.stage = Stage::Add { run, guard };
                None
            }
            Stage::Add { run, guard } => {
                let batches = run.next_batches();
                let finished = batches.is_none();
                for prepared in batches.into_iter().flatten() {
                    let n = prepared.len();
                    match commit(session, run, prepared) {
                        Ok(v) => self.tally.add(&v),
                        Err(e) => {
                            log::warn!("browse: {e}");
                            self.tally.failed += n;
                        }
                    }
                }
                let (done, total) = (run.done.load(std::sync::atomic::Ordering::Relaxed), run.total.load(std::sync::atomic::Ordering::Relaxed));
                guard.progress(done as u64, total as u64);
                if guard.is_cancelled() {
                    run.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                if !finished {
                    return None;
                }
                self.stage = Stage::Done;
                Some(self.finish(session))
            }
        }
    }

    /// The new records as one undo step (as `library.browse` makes them), the changed sidecars, the browse noted.
    fn finish(&mut self, session: &mut Session) -> Result<Value, String> {
        let steps = session.undo.len().saturating_sub(self.undo0);
        if steps > 0 {
            session.merge_undo(steps, &self.tally.label());
        }
        let reread = session.browse_finished(&self.path, &self.listed).map_err(|e| e.to_string())?;
        let photos = session.visible().len();
        Ok(json!({
            "path": self.path,
            "subfolders": self.subfolders,
            "photos": photos,
            "new": self.tally.imported,
            "reread": reread,
            "failed": self.tally.failed,
            "forgotten": self.forgotten,
        }))
    }
}

/// The picture files directly in `dir` (not hidden), sorted.
fn own_files(dir: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|f| {
                    f.is_file() && lightcraft_engine::import::is_supported(f) && !f.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'))
                })
                .map(|f| f.to_string_lossy().to_string())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(p: &std::path::Path, seed: u8) {
        let img = lightcraft_raster::Rgba8::from_fn(16, 12, |x, y| [(x * 9) as u8, (y * 11) as u8, seed, 255]);
        let opts = lightcraft_engine::export::ExportOptions { format: lightcraft_engine::export::ExportFormat::Png, ..Default::default() };
        let b = lightcraft_engine::export::encode_image(&img, &opts).unwrap();
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b).unwrap();
    }

    fn run(session: &mut Session, b: &mut Browse) -> Value {
        for _ in 0..2000 {
            if let Some(r) = b.poll(session) {
                return r.unwrap();
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("the browse never ended");
    }

    #[test]
    fn a_folder_is_read_on_a_worker_and_answers_as_library_browse_does() {
        let dir = std::env::temp_dir().join(format!("lc-apphost-browse-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        png(&dir.join("a.png"), 1);
        png(&dir.join("b.png"), 2);
        png(&dir.join("inner").join("c.png"), 3);
        let mut s = Session::new().with_fs();
        let mut b = Browse::start(&mut s, &dir.to_string_lossy(), Some(false)).unwrap();
        assert_eq!(s.source, LibrarySource::Folder, "the view switches at once");
        assert!(s.activity.list().iter().any(|t| t.label == LABEL), "its row is in the activity stack");
        let r = run(&mut s, &mut b);
        assert_eq!((r["photos"].as_u64(), r["new"].as_u64()), (Some(2), Some(2)), "{r}");
        assert!(s.catalog.photos().all(|p| p.local), "Local records, not library photos");
        assert_eq!(s.undo.len(), 1, "one step");
        assert!(s.activity.list().iter().all(|t| t.label != LABEL), "the row goes when it ends");
        // again, with the folders inside: only the new one is added
        let mut b = Browse::start(&mut s, &dir.to_string_lossy(), Some(true)).unwrap();
        let r = run(&mut s, &mut b);
        assert_eq!((r["photos"].as_u64(), r["new"].as_u64()), (Some(3), Some(1)), "{r}");
        assert!(Browse::start(&mut s, &dir.join("nothing").to_string_lossy(), None).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
