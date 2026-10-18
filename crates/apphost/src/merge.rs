//! Photo Merge (HDR, Panorama, HDR Panorama) off the UI thread, for both apps: a preview for the
//! open dialog, replaced when its options change, and the full merge, which keeps going after the
//! dialog closes (a row in the activity stack) and adds its result to the library
//! ([`lightcraft_engine::Session::finish_merge`]). egui's merge dialog and the SwiftUI app
//! (`host.merge.*`) drive the same [`Runner`].

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::sync::{Arc, Mutex, PoisonError};

use lightcraft_engine::Session;
use lightcraft_engine::activity::{Cancel, TaskGuard, Unit};
use lightcraft_engine::catalog::PhotoId;
pub use lightcraft_engine::merge::MergeOptions;
use lightcraft_engine::merge::{MergeJob, MergeOutput};
use lightcraft_raster::Rgba8;
use serde_json::Value;

/// A merge running on a worker thread.
pub struct Task {
    pub job: MergeJob,
    /// The options it was started with.
    pub options: MergeOptions,
    /// How far it is (0..1) and the stage it is at.
    pub progress: Arc<Mutex<(f32, String)>>,
    pub cancel: Arc<AtomicBool>,
    rx: Receiver<Result<MergeOutput, String>>,
    /// The full merge's row in the activity stack (previews have none).
    guard: Option<TaskGuard>,
}

impl Task {
    fn stop(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// What finished since the last [`Runner::poll`].
pub enum Done {
    /// The preview for `options`: its picture (none when the merge makes none) and statistics, or
    /// why it couldn't be made.
    Preview { options: MergeOptions, result: Result<(Option<Rgba8>, Value), String> },
    /// The full merge: what was added (`{id, path, …}`), or why not; `cancelled` when stopped.
    Merged { options: MergeOptions, result: Result<Value, String>, cancelled: bool },
}

/// Merges in flight: at most one preview and one full merge.
#[derive(Default)]
pub struct Runner {
    pub preview: Option<Task>,
    pub merging: Option<Task>,
    /// Options whose preview failed: not tried again until they change.
    pub failed: Option<MergeOptions>,
}

/// The options of the last merge of kind `command` started (kept in the library preferences), or
/// the defaults the first time. Fields missing from an older or hand-edited prefs.json take their
/// defaults; options that don't parse at all fall back to the defaults.
pub fn last_options(session: &Session, command: &str) -> MergeOptions {
    let saved = session.last_merge.get(command).and_then(|v| serde_json::from_value::<MergeOptions>(v.clone()).ok());
    MergeOptions { command: command.into(), ..saved.unwrap_or_default() }
}

/// Start `options`' merge of `ids` (a preview: small frames and a picture, no file).
pub fn spawn(session: &Session, options: &MergeOptions, ids: &[PhotoId], preview: bool) -> Result<Task, String> {
    let (kind, finish) = lightcraft_engine::merge::parse(&options.command, &options.params()).map_err(|e| e.to_string())?;
    let job = session.plan_merge(kind, finish, ids, preview).map_err(|e| e.to_string())?;
    let progress = Arc::new(Mutex::new((0.0, "Starting".to_string())));
    let cancel = Arc::new(AtomicBool::new(false));
    let (tx, rx) = channel();
    let (j, p, c) = (job.clone(), progress.clone(), cancel.clone());
    let work = move || {
        let run = || {
            j.run(&|f, stage| {
                if let Ok(mut g) = p.lock() {
                    *g = (f, stage.to_string());
                }
                !c.load(Ordering::Relaxed)
            })
        };
        // a panicking merge reports an error instead of leaving "Merging…" up forever
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(run)).unwrap_or_else(|_| Err("the merge failed unexpectedly".into()));
        let _ = tx.send(r);
    };
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::Builder::new().name("photo-merge".into()).spawn(work).map_err(|e| e.to_string())?;
    #[cfg(target_arch = "wasm32")]
    work();
    Ok(Task { job, options: options.clone(), progress, cancel, rx, guard: None })
}

impl Runner {
    /// A preview or a merge is running.
    pub fn busy(&self) -> bool {
        self.preview.is_some() || self.merging.is_some()
    }

    /// The preview being drawn, or the options of the one that failed: nothing to start for them.
    pub fn wants_preview(&self, options: &MergeOptions) -> bool {
        self.preview.as_ref().map(|t| &t.options) != Some(options) && self.failed.as_ref() != Some(options)
    }

    /// Draw a preview of `options`, dropping the one being drawn.
    pub fn preview(&mut self, session: &Session, options: &MergeOptions, ids: &[PhotoId]) -> Result<(), String> {
        self.cancel_preview();
        match spawn(session, options, ids, true) {
            Ok(t) => {
                self.failed = None;
                self.preview = Some(t);
                Ok(())
            }
            Err(e) => {
                self.failed = Some(options.clone());
                Err(e)
            }
        }
    }

    pub fn cancel_preview(&mut self) {
        if let Some(t) = self.preview.take() {
            t.stop();
        }
    }

    /// Start the full merge (one at a time), in the activity stack.
    pub fn start(&mut self, session: &mut Session, options: &MergeOptions, ids: &[PhotoId]) -> Result<(), String> {
        if self.merging.is_some() {
            return Err("a merge is already running".into());
        }
        if ids.len() < 2 {
            return Err("select at least 2 photos to merge".into());
        }
        self.cancel_preview();
        let mut task = spawn(session, options, ids, false)?;
        let guard = session.activity.start("merge", "Merging", Cancel::Flag(task.cancel.clone()));
        guard.set_unit(Unit::Percent);
        guard.progress(0, 1000);
        task.guard = Some(guard);
        self.merging = Some(task);
        session.last_merge.insert(options.command.clone(), options.params());
        if let Err(e) = session.save_prefs() {
            log::warn!("merge: {e}");
        }
        Ok(())
    }

    /// Merge `ids` with the options last used for `command` (the defaults the first time).
    pub fn start_last(&mut self, session: &mut Session, command: &str, ids: &[PhotoId]) -> Result<(), String> {
        let options = last_options(session, command);
        self.start(session, &options, ids)
    }

    /// Collect what finished (a full merge is added to the library here) and move the full merge's
    /// activity row on; `stage` words its stage ("Aligning"…) for the row.
    pub fn poll(&mut self, session: &mut Session, stage: &dyn Fn(&str) -> String) -> Vec<Done> {
        let mut done = Vec::new();
        if let Some(t) = &self.preview {
            match t.rx.try_recv() {
                Ok(r) => {
                    if let Some(t) = self.preview.take() {
                        match r {
                            Ok(out) => done.push(Done::Preview { options: t.options, result: Ok((out.preview, out.info)) }),
                            // (a preview dropped for a newer one says nothing)
                            Err(e) if e == "cancelled" => {}
                            Err(e) => {
                                self.failed = Some(t.options.clone());
                                done.push(Done::Preview { options: t.options, result: Err(e) });
                            }
                        }
                    }
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    if let Some(t) = self.preview.take() {
                        done.push(Done::Preview { options: t.options, result: Err("the merge stopped".into()) });
                    }
                }
            }
        }
        if let Some(t) = &self.merging {
            match t.rx.try_recv() {
                Ok(r) => {
                    if let Some(t) = self.merging.take() {
                        let cancelled = t.cancel.load(Ordering::Relaxed);
                        let result = r
                            .map_err(lightcraft_engine::EngineError::Other)
                            .and_then(|out| session.finish_merge(&t.job, out))
                            .map_err(|e| e.to_string());
                        done.push(Done::Merged { options: t.options, result, cancelled });
                    }
                }
                Err(TryRecvError::Empty) => {
                    let (f, at) = t.progress.lock().unwrap_or_else(PoisonError::into_inner).clone();
                    if let Some(guard) = &t.guard {
                        let f = if f.is_finite() { f.clamp(0.0, 1.0) } else { 0.0 };
                        guard.progress((f * 1000.0) as u64, 1000);
                        guard.detail(&stage(&at));
                    }
                }
                Err(TryRecvError::Disconnected) => {
                    if let Some(t) = self.merging.take() {
                        done.push(Done::Merged { options: t.options, result: Err("the merge stopped".into()), cancelled: false });
                    }
                }
            }
        }
        done
    }
}

/// What a merge is called in a message: "HDR", "Panorama", "HDR panorama".
pub fn what(command: &str) -> &'static str {
    match command {
        "merge.panorama" => "Panorama",
        "merge.hdrPanorama" => "HDR panorama",
        _ => "HDR",
    }
}
