//! A model download as a background task the session starts, watches and cancels without
//! ever blocking: progress lives in atomics, messages behind a mutex only ever `try_lock`ed
//! by the session (the download thread holds it for a few instructions at a time).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use lightcraft_fetch::{self as fetch, Options, Progress};

use crate::activity::{Activity, Cancel, TaskHandle, Unit};

/// Model downloads that run at once; the others wait for their turn, in the order they were asked for.
pub const AT_ONCE: usize = 2;

/// A set of files, the mirrors to try for them, and the folder they go into.
pub type Part = (&'static [fetch::FileSpec<'static>], Vec<String>, PathBuf);

/// What the session sees of a download.
pub use crate::segment::DownloadStatus as Status;

#[derive(Default)]
struct Text {
    file: String,
    error: Option<String>,
    cancelled: bool,
}

#[derive(Default)]
struct Shared {
    running: AtomicBool,
    cancel: Arc<AtomicBool>,
    finished: AtomicBool,
    done: AtomicU64,
    total: AtomicU64,
    text: Mutex<Text>,
}

/// Clears `running` when the download thread ends, however it ends.
struct Running(Arc<Shared>);

impl Drop for Running {
    fn drop(&mut self) {
        self.0.running.store(false, Ordering::SeqCst);
    }
}

#[derive(Default)]
pub struct Downloader {
    shared: Arc<Shared>,
    /// The last text seen (when the download thread held the lock at that moment).
    last: Mutex<Text>,
    /// The last download's row in the activity stack (only the session's thread locks it).
    task: Mutex<Option<TaskHandle>>,
}

fn lock(m: &Mutex<Text>) -> std::sync::MutexGuard<'_, Text> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn lock_task(m: &Mutex<Option<TaskHandle>>) -> std::sync::MutexGuard<'_, Option<TaskHandle>> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Downloader {
    pub fn status(&self) -> Status {
        let s = &self.shared;
        if let Ok(t) = s.text.try_lock()
            && let Ok(mut last) = self.last.try_lock()
        {
            last.file.clone_from(&t.file);
            last.error.clone_from(&t.error);
            last.cancelled = t.cancelled;
        }
        // (only the session's thread locks `last`)
        let (file, error, cancelled) = self.last.try_lock().map(|l| (l.file.clone(), l.error.clone(), l.cancelled)).unwrap_or_default();
        let running = s.running.load(Ordering::SeqCst);
        let row = self.task.try_lock().ok().and_then(|t| t.as_ref().map(TaskHandle::info)).filter(|_| running);
        Status {
            running,
            queued: row.as_ref().is_some_and(|r| r.queued),
            rate: row.as_ref().map_or(0, |r| r.rate),
            eta_ms: row.and_then(|r| r.eta_ms),
            done: s.done.load(Ordering::SeqCst),
            total: s.total.load(Ordering::SeqCst),
            file,
            error,
            cancelled,
            finished: s.finished.load(Ordering::SeqCst),
        }
    }

    pub fn running(&self) -> bool {
        self.shared.running.load(Ordering::SeqCst)
    }

    /// Ask a running download to stop (it keeps its partial files to resume later).
    pub fn cancel(&self) -> bool {
        let running = self.running();
        if running {
            self.shared.cancel.store(true, Ordering::SeqCst);
        }
        running
    }

    /// Start downloading `files` from `mirrors` into `dir` on a background thread, shown in the activity stack as
    /// `name` (its ✕ cancels like [`Downloader::cancel`]). False when one is already running.
    #[cfg(any(feature = "sam", feature = "vision"))]
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        &self,
        label: &'static str,
        files: &'static [fetch::FileSpec<'static>],
        mirrors: Vec<String>,
        dir: PathBuf,
        opts: Options,
        activity: &Activity,
        name: &str,
    ) -> Result<bool, String> {
        self.start_parts(label, vec![(files, mirrors, dir)], opts, activity, name)
    }

    /// Start downloading several sets of files, one after the other (each with its own mirrors and
    /// folder), as one download: one progress, one cancel, one row in the activity stack. False when
    /// one is already running. While [`AT_ONCE`] other model downloads run, it waits for its turn
    /// (`queued`), and can be cancelled meanwhile.
    pub fn start_parts(&self, label: &'static str, parts: Vec<Part>, opts: Options, activity: &Activity, name: &str) -> Result<bool, String> {
        let s = self.shared.clone();
        if s.running.swap(true, Ordering::SeqCst) {
            return Ok(false);
        }
        s.cancel.store(false, Ordering::SeqCst);
        s.finished.store(false, Ordering::SeqCst);
        s.done.store(0, Ordering::SeqCst);
        s.total.store(parts.iter().flat_map(|(files, _, _)| files.iter()).filter_map(|f| f.size).sum(), Ordering::SeqCst);
        // nothing else runs while `running` was false
        *lock(&s.text) = Text::default();
        *lock(&self.last) = Text::default();
        let task = activity.start("download", "Downloading model", Cancel::Flag(s.cancel.clone()));
        task.set_unit(Unit::Bytes);
        task.detail(name);
        task.progress(0, s.total.load(Ordering::SeqCst));
        let turn = activity.try_turn("download", AT_ONCE);
        task.set_queued(turn.is_none());
        *lock_task(&self.task) = Some(task.handle());
        let activity = activity.clone();
        let guard = Running(s.clone());
        let spawned = std::thread::Builder::new().name(format!("{label}-download")).spawn(move || {
            let s = guard.0.clone();
            let mut last_file = String::new();
            // bytes of the parts finished so far, which the next part's progress adds to
            let mut before = 0u64;
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _turn = match turn {
                    Some(t) => t,
                    None => activity.wait_turn("download", AT_ONCE, &s.cancel).ok_or(fetch::DownloadError::Cancelled)?,
                };
                task.set_queued(false);
                for (files, mirrors, dir) in &parts {
                    let mut on_progress = |p: &Progress| {
                        s.done.store(before.saturating_add(p.done), Ordering::SeqCst);
                        if parts.len() == 1 {
                            // (sizes the files don't pin are learnt from the server)
                            s.total.store(p.total, Ordering::SeqCst);
                        }
                        task.progress(s.done.load(Ordering::SeqCst), s.total.load(Ordering::SeqCst));
                        if p.file != last_file {
                            last_file.clone_from(&p.file);
                            lock(&s.text).file.clone_from(&p.file);
                        }
                    };
                    fetch::download(files, mirrors, dir, &opts, &s.cancel, &mut on_progress)?;
                    before = before.saturating_add(files.iter().filter_map(|f| f.size).sum());
                }
                Ok::<(), fetch::DownloadError>(())
            }));
            let cancelled = matches!(r, Ok(Err(fetch::DownloadError::Cancelled)));
            let error = match r {
                Ok(Ok(())) => {
                    s.finished.store(true, Ordering::SeqCst);
                    log::info!("{label} model downloaded");
                    None
                }
                Ok(Err(e)) => Some(e.to_string()),
                Err(_) => Some("the download failed unexpectedly".to_string()),
            };
            if let Some(e) = &error {
                log::warn!("{label} download: {e}");
            }
            let mut text = lock(&s.text);
            text.cancelled = cancelled;
            text.error = error;
            drop(text);
            drop(guard);
        });
        match spawned {
            Ok(_) => Ok(true),
            Err(e) => {
                // the closure (and its guard) was dropped: `running` is false again
                Err(format!("could not start the download: {e}"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    static FILES: &[fetch::FileSpec<'static>] = &[fetch::FileSpec { name: "model.bin", size: Some(1000), sha256: None, max: 1000 }];

    /// A mirror that takes every connection and never answers, until the test ends.
    fn silent_mirror() -> String {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/m", l.local_addr().unwrap());
        std::thread::spawn(move || {
            let mut held = Vec::new();
            for c in l.incoming().flatten() {
                held.push(c);
            }
        });
        base
    }

    fn wait_until(what: &str, ok: impl Fn() -> bool) {
        let t = Instant::now();
        while !ok() {
            assert!(t.elapsed() < Duration::from_secs(30), "{what} never happened");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Two model downloads run at once; a third waits for its turn (and says so), can be cancelled
    /// while it waits, and starts by itself once one of the two ends.
    #[test]
    fn a_third_download_waits_its_turn() {
        let base = silent_mirror();
        let root = std::env::temp_dir().join(format!("lc-download-turns-{}", std::process::id()));
        let activity = Activity::default();
        let d: [Downloader; 4] = Default::default();
        let start = |i: usize| {
            let dir = root.join(i.to_string());
            d[i].start_parts("test", vec![(FILES, vec![base.clone()], dir)], Options::default(), &activity, &format!("Model {i}")).unwrap()
        };
        assert!(start(0) && start(1) && start(2));
        assert_eq!(d.iter().take(3).map(|d| d.status().queued).collect::<Vec<_>>(), [false, false, true]);
        assert!(d.iter().take(3).all(Downloader::running));
        let rows = activity.list();
        assert_eq!(
            rows.iter().map(|r| (r.detail.as_str(), r.queued)).collect::<Vec<_>>(),
            [("Model 0", false), ("Model 1", false), ("Model 2", true)]
        );
        assert!(rows[2].cancellable && rows[2].eta_ms.is_none());
        // a fourth waits behind the third
        assert!(start(3) && d[3].status().queued);
        // cancelled while it waits: it ends as cancelled, and its row goes
        assert!(d[2].cancel());
        wait_until("the waiting download's end", || !d[2].running());
        let st = d[2].status();
        assert!(st.cancelled && !st.finished && !st.queued, "{st:?}");
        assert_eq!(activity.list().len(), 3);
        // one of the two ends: the next in line (the fourth) starts
        assert!(d[0].cancel());
        wait_until("the fourth download's turn", || !d[3].status().queued);
        assert!(d[3].running() && !d[1].status().queued);
        for x in &d {
            x.cancel();
        }
        wait_until("every download's end", || activity.list().is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
