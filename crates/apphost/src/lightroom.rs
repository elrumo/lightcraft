//! Background Lightroom catalog inspection and import, for both apps.
//!
//! Reading a catalog, probing its originals and staging its archive can take minutes on a
//! removable or network drive. A [`Runner`] keeps that work off the owner thread, commits prepared
//! data only on the owner thread ([`Runner::poll`]), then finishes the archive index on a worker.
//! egui's `library.inspectLightroom` / `library.importLightroom` and the SwiftUI app's (through the
//! engine host) drive the same runner; each app words the outcome itself.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

use lightcraft_engine::Session;
use lightcraft_engine::activity::{Cancel, TaskGuard};
use lightcraft_engine::lightroom_job::{LightroomProgress, LightroomStep};
use serde_json::{Value, json};

/// Called from a worker when it has something for [`Runner::poll`] (egui: a repaint).
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// What a task does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Inspect,
    Import,
}

pub enum Message {
    Inspected(Result<Value, String>),
    Prepared(Result<Box<lightcraft_engine::lightroom_job::PreparedLightroom>, String>),
    Finalized(Result<(), String>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Reading,
    Committing,
    Finalizing,
}

/// A Lightroom task in flight. Its receiver is polled on the owner thread; the activity stack
/// shows it.
pub struct Task {
    pub kind: Kind,
    pub path: PathBuf,
    cancel: Arc<AtomicBool>,
    progress: LightroomProgress,
    rx: mpsc::Receiver<Message>,
    phase: Phase,
    cancelled: bool,
    guard: TaskGuard,
}

impl Task {
    /// A task fed by `rx`, in the activity stack (`words` translates its row's words).
    pub fn new(
        s: &Session,
        kind: Kind,
        path: PathBuf,
        cancel: Arc<AtomicBool>,
        progress: LightroomProgress,
        rx: mpsc::Receiver<Message>,
        words: &dyn Fn(&'static str) -> String,
    ) -> Self {
        let label = match kind {
            Kind::Inspect => "Reading Lightroom catalog",
            Kind::Import => "Importing Lightroom catalog",
        };
        let guard = s.activity.start("lightroom", label, Cancel::Flag(cancel.clone()));
        let task = Self { kind, path, cancel, progress, rx, phase: Phase::Reading, cancelled: false, guard };
        task.refresh(words);
        task
    }

    /// Move to `phase`; only reading the catalog can stop, adding the photos runs to the end.
    pub fn enter(&mut self, phase: Phase, words: &dyn Fn(&'static str) -> String) {
        self.phase = phase;
        self.guard.set_cancellable(phase == Phase::Reading);
        self.refresh(words);
    }

    /// Bring the activity row up to date: while reading, the step under way and its own count
    /// (reading the photos takes nearly all of an import, so it is counted, not waited out);
    /// then what is being done.
    fn refresh(&self, words: &dyn Fn(&'static str) -> String) {
        let (step, done, total) = self.progress.step();
        let detail = match (self.phase, self.kind, step) {
            (Phase::Reading, Kind::Inspect, _) => String::new(),
            (Phase::Reading, Kind::Import, LightroomStep::Catalog) => words("Reading Lightroom catalog…"),
            (Phase::Reading, Kind::Import, LightroomStep::ReadPhotos) => words("Reading photos…"),
            (Phase::Reading, Kind::Import, LightroomStep::AddPhotos) => words("Adding photos…"),
            (Phase::Committing, ..) => words("Adding Lightroom photos…"),
            (Phase::Finalizing, ..) => words("Saving Lightroom import index…"),
        };
        if self.phase == Phase::Reading {
            self.guard.progress(done as u64, total as u64);
        } else {
            self.guard.progress(0, 0);
        }
        self.guard.detail(&detail);
    }

    fn status(&self) -> Value {
        let kind = match self.kind {
            Kind::Inspect => "inspect",
            Kind::Import => "import",
        };
        let phase = match self.phase {
            Phase::Reading => "reading",
            Phase::Committing => "committing",
            Phase::Finalizing => "finalizing",
        };
        let (step, done, total) = self.progress.step();
        let step = match step {
            LightroomStep::Catalog => "catalog",
            LightroomStep::ReadPhotos => "readPhotos",
            LightroomStep::AddPhotos => "addPhotos",
        };
        json!({
            "running": true,
            "kind": kind,
            "path": self.path,
            "phase": phase,
            "step": step,
            "done": done,
            "total": total,
            "fraction": self.progress.fraction(),
            "cancelled": self.cancelled || self.cancel.load(Ordering::Relaxed),
        })
    }

    fn cancel(&mut self) {
        self.cancelled = true;
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Drop for Task {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// A task that ended: what it was and its result (`{cancelled}`, `{error}`, the inspection's
/// report, or the import's report and `indexWarning`), with the catalog's `path`.
pub struct Finished {
    pub kind: Kind,
    pub value: Value,
}

/// At most one Lightroom task, and the last one's result.
#[derive(Default)]
pub struct Runner {
    pub task: Option<Task>,
    /// The last task's result (also an import's report while its index is finished).
    pub last: Option<Value>,
}

#[cfg(target_arch = "wasm32")]
fn unsupported_wasm() -> Result<Value, String> {
    Err("Lightroom catalog import is unavailable in browser builds; open the catalog in the native LightCraft app".into())
}

/// The catalog `path` names: the `.lrcat` itself, a Lightroom catalog folder (or part of one), or
/// a folder holding one at its top level or one folder down (a catalog's folder beside its photos).
pub fn find_catalog(path: &Path) -> Result<PathBuf, String> {
    if path.is_file() {
        return Ok(path.to_path_buf());
    }
    if let Some(found) = lightcraft_engine::lightroom_folder::lightroom_catalog_at(path).and_then(|p| p.catalog) {
        return Ok(found);
    }
    // ponytail: one level down only, in name order; deeper needs a real search with a limit
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .take(lightcraft_engine::lightroom_folder::SCAN_LIMIT)
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    dirs.iter()
        .find_map(|d| lightcraft_engine::lightroom_folder::find_lrcat(d))
        .ok_or_else(|| format!("{}: no Lightroom Classic catalog (.lrcat) there", path.display()))
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn_inspect(path: PathBuf, cancel: Arc<AtomicBool>, progress: LightroomProgress, tx: mpsc::Sender<Message>, wake: Wake) -> Result<(), String> {
    std::thread::Builder::new()
        .name("lc-lightroom-inspect".into())
        .spawn(move || {
            let result = lightcraft_engine::guard::catch("Lightroom inspect", || {
                lightcraft_engine::lightroom_catalog::read_with_progress(&path, &cancel, &progress.catalog_total, &progress.catalog_done)
            })
            .and_then(|r| r.map(inspect_report));
            let _ = tx.send(Message::Inspected(result));
            wake();
        })
        .map(|_| ())
        .map_err(|e| format!("could not start Lightroom inspect: {e}"))
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn_prepare(
    mut job: lightcraft_engine::lightroom_job::LightroomJob,
    cancel: Arc<AtomicBool>,
    tx: mpsc::Sender<Message>,
    wake: Wake,
) -> Result<(), String> {
    std::thread::Builder::new()
        .name("lc-lightroom-import".into())
        .spawn(move || {
            let result =
                lightcraft_engine::guard::catch("Lightroom import", || job.prepare(&cancel)).and_then(|r| r.map(Box::new).map_err(|e| e.to_string()));
            if let Err(mpsc::SendError(Message::Prepared(Ok(prepared)))) = tx.send(Message::Prepared(result)) {
                // The owner dropped the task (for example while closing): staged copies/moves
                // must be put back even though no receiver remains to accept the result.
                prepared.rollback();
            }
            wake();
        })
        .map(|_| ())
        .map_err(|e| format!("could not start Lightroom import: {e}"))
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn_finalize(
    finalization: lightcraft_engine::lightroom_job::LightroomArchiveFinalization,
    tx: mpsc::Sender<Message>,
    wake: Wake,
) -> Result<(), String> {
    std::thread::Builder::new()
        .name("lc-lightroom-finalize".into())
        .spawn(move || {
            let result = lightcraft_engine::guard::catch("Lightroom archive finalization", || finalization.finish())
                .and_then(|r| r.map_err(|e| e.to_string()));
            let _ = tx.send(Message::Finalized(result));
            wake();
        })
        .map(|_| ())
        .map_err(|e| format!("could not start Lightroom archive finalization: {e}"))
}

#[cfg(target_arch = "wasm32")]
fn spawn_finalize(
    _finalization: lightcraft_engine::lightroom_job::LightroomArchiveFinalization,
    _tx: mpsc::Sender<Message>,
    _wake: Wake,
) -> Result<(), String> {
    unsupported_wasm().map(|_| ())
}

/// An inspection's answer: photos, collections (not Lightroom's own), originals not found, warnings.
#[cfg(not(target_arch = "wasm32"))]
pub fn inspect_report(data: lightcraft_engine::lightroom_catalog::CatalogImport) -> Value {
    json!({
        "photos": data.photos.len(),
        "collections": data.collections.iter().filter(|r| r.get("systemOnly").and_then(Value::as_f64).unwrap_or(0.0) == 0.0).count(),
        "missing": data.photos.iter().filter(|p| !std::path::Path::new(&p.path).is_file()).map(|p| &p.path).collect::<Vec<_>>(),
        "warnings": data.warnings,
    })
}

impl Runner {
    pub fn running(&self) -> bool {
        self.task.is_some()
    }

    /// The task running (`{running, kind, path, phase, step, done, total, fraction, cancelled}`), else the last
    /// one's result, else null.
    pub fn status(&self) -> Value {
        self.task.as_ref().map(Task::status).or_else(|| self.last.clone()).unwrap_or(Value::Null)
    }

    pub fn cancel(&mut self) {
        if let Some(task) = self.task.as_mut() {
            task.cancel();
        }
    }

    /// Read the catalog at `path` (see [`find_catalog`]) on a worker.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn start_inspect(&mut self, s: &Session, path: &Path, wake: Wake, words: &dyn Fn(&'static str) -> String) -> Result<Value, String> {
        if self.task.is_some() {
            return Err("a Lightroom catalog task is already running".into());
        }
        let path = find_catalog(path)?;
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = LightroomProgress::default();
        let (tx, rx) = mpsc::channel();
        spawn_inspect(path.clone(), cancel.clone(), progress.clone(), tx, wake)?;
        self.last = None;
        self.task = Some(Task::new(s, Kind::Inspect, path.clone(), cancel, progress, rx, words));
        Ok(json!({"running": true, "kind": "inspect", "path": path}))
    }

    /// Prepare the import of the catalog at `path` on a worker (committed in [`Runner::poll`]).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn start_import(
        &mut self,
        s: &mut Session,
        path: &Path,
        update_existing: bool,
        wake: Wake,
        words: &dyn Fn(&'static str) -> String,
    ) -> Result<Value, String> {
        if self.task.is_some() {
            return Err("a Lightroom catalog task is already running".into());
        }
        let path = find_catalog(path)?;
        let job = lightcraft_engine::lightroom_job::LightroomJob::new(s, path.clone(), update_existing).map_err(|e| e.to_string())?;
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = job.progress();
        let (tx, rx) = mpsc::channel();
        spawn_prepare(job, cancel.clone(), tx, wake)?;
        self.last = None;
        self.task = Some(Task::new(s, Kind::Import, path.clone(), cancel, progress, rx, words));
        Ok(json!({"running": true, "kind": "import", "path": path}))
    }

    #[cfg(target_arch = "wasm32")]
    pub fn start_inspect(&mut self, _s: &Session, _path: &Path, _wake: Wake, _words: &dyn Fn(&'static str) -> String) -> Result<Value, String> {
        unsupported_wasm()
    }

    #[cfg(target_arch = "wasm32")]
    pub fn start_import(
        &mut self,
        _s: &mut Session,
        _path: &Path,
        _update_existing: bool,
        _wake: Wake,
        _words: &dyn Fn(&'static str) -> String,
    ) -> Result<Value, String> {
        unsupported_wasm()
    }

    /// The task is over: keep its result (with the catalog's `path`) for [`Runner::status`].
    fn finish(&mut self, task: Task, mut value: Value) -> Finished {
        if let Some(o) = value.as_object_mut() {
            o.entry("path").or_insert_with(|| json!(task.path));
        }
        self.last = Some(value.clone());
        Finished { kind: task.kind, value }
    }

    /// On the owner thread: apply what the worker sent — commit a prepared import, start finishing
    /// its archive index — and say when the task has ended.
    pub fn poll(&mut self, s: &mut Session, wake: Wake, words: &dyn Fn(&'static str) -> String) -> Option<Finished> {
        let mut task = self.task.take()?;
        let message = match task.rx.try_recv() {
            Ok(m) => m,
            Err(mpsc::TryRecvError::Empty) => {
                task.refresh(words);
                self.task = Some(task);
                return None;
            }
            Err(mpsc::TryRecvError::Disconnected) => return Some(self.finish(task, json!({"error": "Lightroom worker stopped unexpectedly"}))),
        };
        // The task's cancel flag says whether it was cancelled (the worker only stops for it), not an
        // error's words: a real failure that mentions "cancelled" is still reported as a failure.
        let stopped = task.cancel.load(Ordering::Relaxed);
        match message {
            Message::Inspected(result) => Some(match result {
                Ok(report) if !stopped => self.finish(task, report),
                Ok(_) => self.finish(task, json!({"cancelled": true})),
                Err(_) if stopped => self.finish(task, json!({"cancelled": true})),
                Err(e) => self.finish(task, json!({"error": e})),
            }),
            Message::Prepared(result) => {
                let prepared = match result {
                    Ok(p) => p,
                    Err(_) if stopped => return Some(self.finish(task, json!({"cancelled": true}))),
                    Err(e) => return Some(self.finish(task, json!({"error": e}))),
                };
                if stopped || prepared.is_cancelled() || !prepared.token().matches(s) {
                    prepared.rollback();
                    let value =
                        if stopped { json!({"cancelled": true}) } else { json!({"error": "Lightroom import belongs to a different open library"}) };
                    return Some(self.finish(task, value));
                }
                task.enter(Phase::Committing, words);
                let mut finalization = None;
                let mut report = None;
                let committed = s.execute_fn("library.importLightroom", |s| {
                    let completion = lightcraft_engine::lightroom_job::commit_prepared(s, *prepared)?;
                    report = Some(completion.report.clone());
                    finalization = Some(completion.finalization);
                    Ok(completion.report)
                });
                let report = report.unwrap_or(Value::Null);
                task.enter(Phase::Finalizing, words);
                let (tx, rx) = mpsc::channel();
                let commit_error = committed.as_ref().err().map(ToString::to_string);
                let Some(finalization) = finalization else {
                    let error = commit_error.unwrap_or_else(|| "Lightroom import did not commit".into());
                    return Some(self.finish(task, json!({"error": error})));
                };
                if let Err(e) = spawn_finalize(finalization, tx, wake) {
                    let mut result = json!({"report": report, "indexWarning": e});
                    if let Some(error) = commit_error {
                        result["error"] = json!(error);
                    }
                    return Some(self.finish(task, result));
                }
                task.rx = rx;
                task.cancelled = false;
                let mut pending = json!({"running": true, "kind": "import", "report": report});
                if let Some(error) = commit_error {
                    pending["error"] = json!(error);
                }
                self.last = Some(pending);
                self.task = Some(task);
                None
            }
            Message::Finalized(result) => {
                let pending = self.last.take().unwrap_or(Value::Null);
                let report = pending.get("report").cloned().unwrap_or(Value::Null);
                Some(match result {
                    Ok(()) if pending.get("error").is_none() => self.finish(task, report),
                    Ok(()) => self.finish(task, json!({"report": report, "error": pending["error"].clone()})),
                    Err(e) => {
                        let mut value = json!({"report": report, "indexWarning": e});
                        if let Some(error) = pending.get("error") {
                            value["error"] = error.clone();
                        }
                        self.finish(task, value)
                    }
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightcraft_engine::lightroom_catalog::CatalogImport;

    fn words(w: &'static str) -> String {
        w.to_string()
    }

    fn wake() -> Wake {
        Arc::new(|| {})
    }

    /// A task as `start_import` makes it, fed by a channel the test holds instead of a worker.
    fn fake_import(s: &Session) -> (Task, mpsc::Sender<Message>, Arc<AtomicBool>, LightroomProgress) {
        let (tx, rx) = mpsc::channel();
        let (cancel, progress) = (Arc::new(AtomicBool::new(false)), LightroomProgress::default());
        let task = Task::new(s, Kind::Import, "fixture.lrcat".into(), cancel.clone(), progress.clone(), rx, &words);
        (task, tx, cancel, progress)
    }

    #[test]
    fn lightroom_row_is_not_cancellable_while_committing() {
        let mut s = Session::with_demo();
        let mut runner = Runner::default();
        let (task, tx, cancel, progress) = fake_import(&s);
        runner.task = Some(task);
        progress.catalog_total.store(10, Ordering::Relaxed);
        progress.catalog_done.store(3, Ordering::Relaxed);
        assert!(runner.poll(&mut s, wake(), &words).is_none());
        let rows = s.activity.list();
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].kind, rows[0].label.as_str()), ("lightroom", "Importing Lightroom catalog"));
        assert_eq!((rows[0].done, rows[0].total), (3, 10));
        assert!(rows[0].cancellable, "reading the catalog can stop");
        runner.task.as_mut().unwrap().enter(Phase::Committing, &words);
        let row = s.activity.list().remove(0);
        assert!(!row.cancellable, "once photos are being added it runs to the end");
        assert!(s.activity.cancel(row.id).is_err());
        assert!(!cancel.load(Ordering::Relaxed));
        tx.send(Message::Finalized(Ok(()))).unwrap();
        let finished = runner.poll(&mut s, wake(), &words).expect("it ended");
        assert_eq!(finished.kind, Kind::Import);
        assert!(!runner.running());
        assert!(s.activity.list().is_empty(), "the row goes with the task");
    }

    #[test]
    fn lightroom_cancel_from_the_stack_stops_the_read() {
        let mut s = Session::with_demo();
        let mut runner = Runner::default();
        let (task, tx, cancel, _) = fake_import(&s);
        runner.task = Some(task);
        let id = s.activity.list()[0].id;
        s.activity.cancel(id).unwrap();
        assert!(cancel.load(Ordering::Relaxed), "the worker sees the stack's ✕");
        tx.send(Message::Prepared(Err("cancelled".into()))).unwrap();
        let finished = runner.poll(&mut s, wake(), &words).expect("it ended");
        assert_eq!(finished.value, json!({"cancelled": true, "path": "fixture.lrcat"}));
        assert!(!runner.running());
        assert_eq!(runner.status(), json!({"cancelled": true, "path": "fixture.lrcat"}));
        assert!(s.activity.list().is_empty());
    }

    /// Whether the task was cancelled is the task's flag, not an error's words (#1046): a cancel
    /// reads as one whatever the message says, and a failure that mentions "cancelled" is a failure.
    #[test]
    fn lightroom_cancel_is_the_flag_not_the_words() {
        let mut s = Session::with_demo();
        let mut runner = Runner::default();
        let (task, tx, cancel, _) = fake_import(&s);
        runner.task = Some(task);
        cancel.store(true, Ordering::Relaxed);
        tx.send(Message::Prepared(Err("Lightroom-Import abgebrochen".into()))).unwrap();
        runner.poll(&mut s, wake(), &words).expect("it ended");
        assert_eq!(runner.status(), json!({"cancelled": true, "path": "fixture.lrcat"}));

        for message in
            [Message::Prepared(Err("the drive was ejected: copy cancelled by the system".into())), Message::Inspected(Err("cancelled".into()))]
        {
            let (task, tx, _, _) = fake_import(&s);
            runner.task = Some(task);
            tx.send(message).unwrap();
            runner.poll(&mut s, wake(), &words).expect("it ended");
            let last = runner.status();
            assert!(last.get("cancelled").is_none() && last["error"].as_str().is_some_and(|e| e.contains("cancelled")), "{last}");
        }
    }

    #[test]
    fn inspect_report_keeps_empty_catalog_shape() {
        let report = inspect_report(CatalogImport {
            source: "synthetic.lrcat".into(),
            photos: Vec::new(),
            collections: Vec::new(),
            members: Vec::new(),
            collection_content: Vec::new(),
            stacks: Vec::new(),
            warnings: vec!["warning".into()],
        });
        assert_eq!(report["photos"], 0);
        assert_eq!(report["collections"], 0);
        assert_eq!(report["missing"], json!([]));
        assert_eq!(report["warnings"], json!(["warning"]));
    }

    // A catalog is found from its file, its folder, or the folder holding its folder
    #[test]
    fn a_catalog_is_found_from_a_folder_above_it() {
        let base = std::env::temp_dir().join(format!("lc-find-lrcat-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let folder = base.join("Lightroom");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::create_dir_all(base.join("Photos")).unwrap();
        let lrcat = folder.join("Lightroom Catalog.lrcat");
        std::fs::write(&lrcat, b"").unwrap();
        assert_eq!(find_catalog(&lrcat).unwrap(), lrcat);
        assert_eq!(find_catalog(&folder).unwrap(), lrcat);
        assert_eq!(find_catalog(&base).unwrap(), lrcat);
        assert!(find_catalog(&base.join("Photos")).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }
}
