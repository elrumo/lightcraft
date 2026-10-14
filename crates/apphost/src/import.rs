//! Importing photos in the background, shared by the egui app and the engine host: the scan the
//! review shows ([`Scan`]: files probed, duplicates found, on a worker thread), and the import
//! itself ([`Run`]: files readied on a worker thread in batches, each batch added to the catalog on
//! the frontend's thread with [`commit`], so a slow drive never stalls a frame or the engine
//! thread), with what it added counted up ([`Tally`]).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use lightcraft_engine::Session;
use lightcraft_engine::activity::{Cancel, TaskGuard};
use lightcraft_engine::import::{ImportJob, ImportOptions, ImportWarning, Prepared, ScanInput, ScanOutput, ScanProgress, scan_with};
use serde_json::{Value, json};

/// A scan of files and folders for the import review, on a worker thread (a network share can take
/// minutes to read), with its row in the activity stack (✕ stops it).
pub struct Scan {
    pub progress: Arc<ScanProgress>,
    rx: std::sync::mpsc::Receiver<ScanOutput>,
    /// Its row in the activity stack (its ✕ and `activity.cancel` set `progress.cancel`).
    pub guard: TaskGuard,
}

impl Scan {
    /// Start scanning `paths`; `label` is the activity row's ("Scanning folder", "Reading folder").
    pub fn start(session: &mut Session, paths: &[String], label: &str) -> Scan {
        let (input, paths) = ScanInput::new(session, paths);
        let progress = Arc::new(ScanProgress::default());
        let (tx, rx) = std::sync::mpsc::channel();
        let p = progress.clone();
        let job = move || {
            let _ = tx.send(scan_with(input, &paths, &p));
        };
        #[cfg(not(target_arch = "wasm32"))]
        std::thread::spawn(job);
        #[cfg(target_arch = "wasm32")]
        job();
        let guard = session.activity.start("scan", label, Cancel::Flag(progress.cancel.clone()));
        Scan { progress, rx, guard }
    }

    /// The scan's outcome once it has ended (`None` while it runs; `Some(Err)` when its worker
    /// died). Keeps the activity row current meanwhile: `looking` is its detail while folders are
    /// still being listed (the total isn't known yet), in the frontend's language.
    pub fn poll(&self, looking: &str) -> Option<Result<ScanOutput, String>> {
        match self.rx.try_recv() {
            Ok(o) => Some(Ok(o)),
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                let (total, done) = (self.progress.total.load(Ordering::Relaxed), self.progress.done.load(Ordering::Relaxed));
                self.guard.progress(done as u64, total as u64);
                self.guard.detail(if total == 0 { looking } else { "" });
                None
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(Err("Scan failed".into())),
        }
    }

    /// ✕ was pressed: the worker stops at its next file (don't wait for it).
    pub fn cancelled(&self) -> bool {
        self.guard.is_cancelled() || self.progress.cancel.load(Ordering::Relaxed)
    }
}

/// What an import is told as its worker goes (the egui app's grid holds a tile for each dropped
/// photo until it is in the catalog).
pub trait Watch: Send + Sync {
    /// The files to import are known (a folder has been listed).
    fn listed(&self, _files: &[String]) {}
    /// A batch of `n` files is being readied.
    fn readying(&self, _n: usize) {}
    /// A batch is ready for [`commit`] (wake the loop).
    fn ready(&self) {}
}

/// Files per batch in the browser build (each batch joins the catalog as it is ready, so the
/// progress updates); the native worker uses [`lightcraft_engine::import::batch_size`].
#[cfg(target_arch = "wasm32")]
const WEB_BATCH: usize = 8;

/// Where an import's batches are readied: a worker thread, or (browser build: no threads, and no
/// slow drives) inline, one batch per call.
enum Runner {
    #[cfg(not(target_arch = "wasm32"))]
    Thread(std::sync::mpsc::Receiver<Prepared>),
    #[cfg(target_arch = "wasm32")]
    Inline { job: Box<ImportJob>, queue: Vec<String>, files: Option<std::collections::VecDeque<String>> },
}

/// An import running: its worker readies batches of files, [`Run::next_batches`] hands them over
/// to be added to the catalog ([`commit`]).
pub struct Run {
    runner: Runner,
    pub cancel: Arc<AtomicBool>,
    pub total: Arc<AtomicUsize>,
    pub done: Arc<AtomicUsize>,
    pub opts: ImportOptions,
    pub now: String,
    /// The album the photos go into (a new one named `album_name` is made with the first batch).
    pub album: Option<u64>,
    pub album_name: Option<String>,
}

impl std::fmt::Debug for Run {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Run").field("cancel", &self.cancel.load(Ordering::Relaxed)).finish()
    }
}

impl Drop for Run {
    /// A dropped import (replaced, or the app closing) stops its worker at the next file.
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Run {
    /// Start importing `queue` (files or folders) with `library.import`'s params `p` (without `paths`).
    pub fn begin(session: &mut Session, queue: Vec<String>, p: &Value, watch: Option<Arc<dyn Watch>>) -> Result<Run, String> {
        let mut p = p.clone();
        p["paths"] = json!(queue);
        let req = lightcraft_engine::cmd::library::import_params(session, &p).map_err(|e| e.to_string())?;
        let job = ImportJob::new(session, req.opts).map_err(|e| e.to_string())?;
        Self::start(job, queue, req.album, req.album_name, watch)
    }

    /// Start `job` on `queue`.
    pub fn start(
        job: ImportJob,
        queue: Vec<String>,
        album: Option<u64>,
        album_name: Option<String>,
        watch: Option<Arc<dyn Watch>>,
    ) -> Result<Run, String> {
        let cancel = Arc::new(AtomicBool::new(false));
        let (total, done) = (job.total.clone(), job.done.clone());
        let (opts, now) = (job.opts.clone(), job.now().to_string());
        let runner = Self::spawn(job, queue, cancel.clone(), watch)?;
        Ok(Run { runner, cancel, total, done, opts, now, album, album_name })
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn spawn(mut job: ImportJob, queue: Vec<String>, cancel: Arc<AtomicBool>, watch: Option<Arc<dyn Watch>>) -> Result<Runner, String> {
        let (tx, rx) = std::sync::mpsc::channel();
        let work = move || {
            let files = job.expand(&queue);
            if let Some(w) = &watch {
                w.listed(&files);
            }
            for chunk in files.chunks(lightcraft_engine::import::batch_size()) {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                if let Some(w) = &watch {
                    w.readying(chunk.len());
                }
                let prepared = match lightcraft_engine::guard::catch("import", || job.prepare_files(chunk.to_vec(), &cancel)) {
                    Ok(p) => p,
                    Err(_) => break, // logged; the files readied so far are added
                };
                if let Err(unsent) = tx.send(prepared) {
                    // the import was dropped: nothing will add these, so take back what a Move placed
                    unsent.0.rollback();
                    break;
                }
                if let Some(w) = &watch {
                    w.ready();
                }
            }
            if let Some(w) = &watch {
                w.ready();
            }
        };
        std::thread::Builder::new().name("lc-import".into()).spawn(work).map(|_| Runner::Thread(rx)).map_err(|e| format!("could not start: {e}"))
    }

    #[cfg(target_arch = "wasm32")]
    fn spawn(job: ImportJob, queue: Vec<String>, _cancel: Arc<AtomicBool>, _watch: Option<Arc<dyn Watch>>) -> Result<Runner, String> {
        Ok(Runner::Inline { job: Box::new(job), queue, files: None })
    }

    /// Readied batches (at most a few a call). `None` once the worker is done.
    pub fn next_batches(&mut self) -> Option<Vec<Prepared>> {
        match &mut self.runner {
            #[cfg(not(target_arch = "wasm32"))]
            Runner::Thread(rx) => {
                let mut out = Vec::new();
                while out.len() < 4 {
                    match rx.try_recv() {
                        Ok(p) => out.push(p),
                        Err(std::sync::mpsc::TryRecvError::Empty) => return Some(out),
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => return if out.is_empty() { None } else { Some(out) },
                    }
                }
                Some(out)
            }
            #[cfg(target_arch = "wasm32")]
            Runner::Inline { job, queue, files } => {
                if self.cancel.load(Ordering::Relaxed) {
                    return None;
                }
                if files.is_none() {
                    *files = Some(job.expand(queue).into());
                    return Some(Vec::new());
                }
                let Some(files) = files.as_mut() else { return None };
                if files.is_empty() {
                    return None;
                }
                let n = files.len().min(WEB_BATCH);
                let batch: Vec<String> = files.drain(..n).collect();
                Some(vec![job.prepare_files(batch, &self.cancel)])
            }
        }
    }
}

/// Add one readied batch to the catalog (an undo step, merged into the import's at the end with
/// [`Tally::label`]). A new album (named in the params) is made with the first photos; later
/// batches join it. The answer is `library.import`'s for the batch.
pub fn commit(session: &mut Session, run: &mut Run, prepared: Prepared) -> Result<Value, String> {
    let (opts, now, album, album_name) = (&run.opts, run.now.as_str(), run.album, run.album_name.clone());
    let v = session
        .execute_fn("library.import", |s| {
            let report = lightcraft_engine::import::commit_prepared(s, opts, now, prepared)?;
            lightcraft_engine::cmd::library::import_batch_done(s, report, album, album_name.as_deref())
        })
        .map_err(|e| e.to_string())?;
    if let Some(a) = v["album"].as_u64() {
        run.album = Some(a);
    }
    Ok(v)
}

/// What an import added, counted over its batches.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tally {
    pub imported: usize,
    /// Brought back from Recently Deleted (`onDeleted: restore`).
    pub restored: usize,
    pub duplicates: usize,
    /// The library photos the skipped duplicates match (shown when nothing new was added).
    pub existing: Vec<u64>,
    pub failed: usize,
    /// Move: originals moved, and sources left in place (with a reason, logged).
    pub moved: usize,
    pub kept: usize,
    /// Structured diagnostics from foreign XMP develop settings.
    pub warnings: Vec<ImportWarning>,
    /// The first photo added (selected when the import ends).
    pub first: Option<u64>,
}

impl Tally {
    /// Count a batch's answer ([`commit`]'s).
    pub fn add(&mut self, v: &Value) {
        let len = |k: &str| v[k].as_array().map_or(0, Vec::len);
        self.imported += len("imported");
        self.restored += len("restored");
        self.duplicates += len("duplicates");
        self.existing.extend(v["duplicates"].as_array().into_iter().flatten().filter_map(|d| d["existing"].as_u64()));
        self.failed += len("failed");
        self.moved += len("moved");
        self.kept += len("kept");
        self.warnings.extend(v["warnings"].as_array().into_iter().flatten().filter_map(|w| serde_json::from_value(w.clone()).ok()));
        for k in v["kept"].as_array().into_iter().flatten() {
            log::warn!("import: kept {}: {}", k["path"].as_str().unwrap_or(""), k["reason"].as_str().unwrap_or(""));
        }
        if self.first.is_none() {
            self.first = ["imported", "restored"].iter().find_map(|k| v[*k].as_array().and_then(|a| a.first()).and_then(Value::as_u64));
        }
    }

    /// The import's undo step (English: a frontend may say it in its own language).
    pub fn label(&self) -> String {
        let plural = |n: usize| if n == 1 { "" } else { "s" };
        if self.imported == 0 && self.restored > 0 {
            format!("Restore {} Photo{}", self.restored, plural(self.restored))
        } else if self.restored > 0 {
            format!("Add {} Photo{}, restore {}", self.imported, plural(self.imported), self.restored)
        } else {
            format!("Add {} Photo{}", self.imported, plural(self.imported))
        }
    }

    /// As a frontend reads it: `{imported, restored, duplicates, existing, failed, moved, kept, warnings, first}`.
    pub fn to_json(&self) -> Value {
        json!({
            "imported": self.imported, "restored": self.restored, "duplicates": self.duplicates, "existing": self.existing,
            "failed": self.failed, "moved": self.moved, "kept": self.kept, "warnings": self.warnings, "first": self.first,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("lc-apphost-import-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    // A scan lists what a folder holds; the import adds it in batches off the thread, one undo step
    #[test]
    fn a_folder_is_scanned_then_imported_in_the_background() {
        let lib = temp("lib");
        let src = temp("src");
        // three photos to import: the demo's, exported small
        let mut demo = Session::with_demo();
        let ids: Vec<u64> = demo.visible_cloned().iter().take(3).map(|id| id.0).collect();
        let p = json!({"dir": src.to_string_lossy(), "ids": ids, "longEdge": 48, "format": "png"});
        crate::export::export_now(&mut demo, &p, &mut |_, _, _| true).unwrap();
        let mut s = Session::new();
        s.open_library(&lib, false).unwrap();
        let paths = vec![src.to_string_lossy().to_string()];
        let scan = Scan::start(&mut s, &paths, "Scanning folder");
        let out = loop {
            if let Some(o) = scan.poll("Looking for photos…") {
                break o.unwrap();
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert_eq!(out.candidates.len(), 3);
        let files: Vec<String> = out.candidates.iter().map(|c| c.path.clone()).collect();
        let undo0 = s.undo.len();
        let mut run = Run::begin(&mut s, files, &json!({"mode": "add"}), None).unwrap();
        let mut tally = Tally::default();
        loop {
            let batches = run.next_batches();
            let done = batches.is_none();
            for p in batches.into_iter().flatten() {
                tally.add(&commit(&mut s, &mut run, p).unwrap());
            }
            if done {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(tally.imported, 3, "{tally:?}");
        assert!(tally.first.is_some());
        assert_eq!(tally.label(), "Add 3 Photos");
        s.merge_undo(s.undo.len() - undo0, &tally.label());
        assert_eq!(s.undo.len(), undo0 + 1);
        let _ = (std::fs::remove_dir_all(&lib), std::fs::remove_dir_all(&src));
    }
}
