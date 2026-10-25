//! Self-hosted sync driven from a frontend's loop (`docs/sync.md`): the engine says what to send next
//! ([`Session::sync_tasks`]), the host runs it off its own thread ([`native_exec`]: a worker thread
//! per request) and the answers come back through [`Driver::poll`], so a slow or absent server never
//! holds a frame or the engine thread. Shared by the egui app (its frame loop) and the engine host
//! (its tick).

use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use lightcraft_engine::Session;
use lightcraft_engine::activity::{Cancel, TaskGuard};
use lightcraft_engine::cloud_sync::{Done, POLL, Task};
use lightcraft_engine::icloud::folder::FolderDone;
use web_time::Instant;

/// A cloud-folder pass runs at least this often (the provider brings the other devices' files whenever it likes) ...
const FOLDER_POLL: Duration = Duration::from_secs(30);
/// ... and sooner after something changed here or the app came to the front, but never closer than this.
const FOLDER_MIN: Duration = Duration::from_secs(5);
/// A pass that takes longer than this gets a row in the activity stack.
const FOLDER_ROW_AFTER: Duration = Duration::from_secs(1);

/// What one [`Driver::poll`] asks of the frontend.
#[derive(Debug, Default, PartialEq)]
pub struct Polled {
    /// A share link was just made: put it on the clipboard and say so.
    pub link: Option<String>,
    /// Signing in met a server that has a library already: ask what to do with this library's
    /// photos (once per sign-in; nothing syncs meanwhile).
    pub ask_conflict: bool,
    /// Call again within this long (a busy sync polls fast, an idle one every [`POLL`]); `None`
    /// when not signed in.
    pub again: Option<Duration>,
}

/// Sync requests in flight and their answers.
pub struct Driver {
    tx: Sender<Done>,
    rx: Receiver<Done>,
    /// Tasks handed to the executor and not answered yet.
    pub in_flight: usize,
    /// The app was in front at the last poll (coming back to the front pulls at once).
    focused: bool,
    /// The conflict choice was put to the user (once per sign-in: it can be put off, and is offered
    /// again from the cloud button and Settings).
    conflict_asked: bool,
    folder: FolderPoll,
}

impl Default for Driver {
    fn default() -> Self {
        let (tx, rx) = channel();
        Driver { tx, rx, in_flight: 0, focused: false, conflict_asked: false, folder: FolderPoll::default() }
    }
}

/// The passes through the cloud folder the library syncs through (`icloud.useFolder`, `docs/sync.md`): the file work
/// runs on a thread of its own, the engine's half of it here.
#[derive(Default)]
struct FolderPoll {
    /// The pass on its thread: its answer, when it started, and its row in the activity stack (made when it runs long).
    pass: Option<(Receiver<FolderDone>, Instant, Option<TaskGuard>)>,
    /// An answer waiting for a slider drag to end (nothing from the other devices is applied during one).
    held: Option<FolderDone>,
    last_start: Option<Instant>,
    /// What `icloud_rev` was when the last pass started.
    rev: Option<u64>,
    /// The last pass left records to write: another is due at once.
    more: bool,
    focused: bool,
}

impl Driver {
    /// Hand finished tasks to the engine and start the next ones on `exec` (cheap when there is
    /// nothing to do). `focused`: the app is in front. Without `exec` (no network on this
    /// platform) answers are still applied and nothing new starts.
    pub fn poll(&mut self, session: &mut Session, focused: bool, exec: Option<&dyn Fn(Task, Sender<Done>)>) -> Polled {
        let mut out = self.poll_server(session, focused, exec);
        let folder = self.folder.poll(session, focused);
        out.again = match (out.again, folder) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        out
    }

    fn poll_server(&mut self, session: &mut Session, focused: bool, exec: Option<&dyn Fn(Task, Sender<Done>)>) -> Polled {
        let mut out = Polled::default();
        while let Ok(d) = self.rx.try_recv() {
            self.in_flight = self.in_flight.saturating_sub(1);
            session.sync_done(d);
        }
        out.link = session.sync_take_new_link();
        if session.sync_conflict().is_some() {
            out.ask_conflict = !std::mem::replace(&mut self.conflict_asked, true);
        } else {
            self.conflict_asked = false;
        }
        let Some(exec) = exec else { return out };
        let Some(st) = session.sync_state() else { return out };
        if !st.signed_in() || st.config.paused {
            return out;
        }
        if focused && !self.focused {
            session.sync_soon();
        }
        self.focused = focused;
        for t in session.sync_tasks() {
            self.in_flight += 1;
            exec(t, self.tx.clone());
        }
        // (signing out waits for the sending to end: it is as busy as syncing)
        let busy = self.in_flight > 0 || session.sync_state().is_some_and(|st| matches!(st.state(), "syncing" | "signingOut"));
        out.again = Some(if busy { Duration::from_millis(500) } else { POLL });
        out
    }
}

impl FolderPoll {
    /// Take a finished pass, start the next when it is due. Answers when to come back (`None`: no cloud folder).
    fn poll(&mut self, session: &mut Session, focused: bool) -> Option<Duration> {
        let Some(dir) = session.icloud_folder() else {
            *self = FolderPoll::default();
            return None;
        };
        if let Some((rx, started, guard)) = &mut self.pass {
            match rx.try_recv() {
                Ok(done) => {
                    self.pass = None;
                    self.held = Some(done);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    if guard.is_none() && started.elapsed() >= FOLDER_ROW_AFTER {
                        *guard = Some(session.activity.start("cloudFolder", "Syncing through the cloud folder", Cancel::No));
                    }
                    return Some(Duration::from_millis(250));
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    log::warn!("cloud folder: the pass's thread ended without an answer");
                    self.pass = None;
                }
            }
        }
        if self.held.is_some() {
            if session.interaction.is_some() {
                return Some(Duration::from_millis(250));
            }
            if let Some(done) = self.held.take() {
                self.more = session.icloud_folder_done(done).more;
            }
        }
        let now = Instant::now();
        let since = self.last_start.map(|t| now.saturating_duration_since(t));
        let came_back = focused && !self.focused;
        self.focused = focused;
        let rev = session.icloud_rev();
        let asked = session.icloud_take_folder_now();
        let due = asked
            || since.is_none_or(|d| {
                d >= FOLDER_POLL || (d >= FOLDER_MIN && (rev != self.rev || came_back)) || (self.more && d >= Duration::from_millis(500))
            });
        if !due {
            return Some(Duration::from_secs(1));
        }
        self.last_start = Some(now);
        self.rev = rev;
        let task = match session.icloud_folder_task(Some(&dir)) {
            Ok(t) => t,
            Err(e) => {
                log::warn!("cloud folder: {e}");
                return Some(FOLDER_POLL);
            }
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (tx, rx) = channel();
            let spawned = std::thread::Builder::new().name("lc-cloud-folder".into()).spawn(move || {
                let _ = tx.send(lightcraft_engine::icloud::folder::run(task));
            });
            match spawned {
                Ok(_) => self.pass = Some((rx, now, None)),
                Err(e) => log::warn!("cloud folder: can't start a thread: {e}"),
            }
        }
        #[cfg(target_arch = "wasm32")]
        let _ = task;
        Some(Duration::from_millis(250))
    }
}

/// The native transport: each request on its own worker thread (uploads and downloads stream
/// files), answered through the channel; `notify` runs after each answer (wake the loop).
#[cfg(not(target_arch = "wasm32"))]
pub fn native_exec(notify: impl Fn() + Send + Sync + 'static) -> impl Fn(Task, Sender<Done>) {
    let notify = std::sync::Arc::new(notify);
    move |task, tx| {
        let id = task.id();
        let done = tx.clone();
        let notify = notify.clone();
        let spawned = std::thread::Builder::new().name("lc-sync".into()).spawn(move || {
            let _ = tx.send(lightcraft_engine::cloud_sync::run(&task));
            notify();
        });
        if let Err(e) = spawned {
            let _ = done.send(Done::failed(id, format!("can't start a sync thread: {e}")));
        }
    }
}

/// What the frontend's cloud icon shows: the state (`signedOut`, `signingOut`, `conflict`, `paused`, `error`,
/// `syncing`, `idle`; `None` for a local library with no server), the server, how much is left, and where the
/// open photo's original stands.
pub fn summary(session: &Session) -> Option<serde_json::Value> {
    let st = session.sync_state()?;
    let s = st.status();
    Some(serde_json::json!({
        "state": s["state"],
        "server": s["server"],
        "pending": s["pending"],
        "uploads": s["uploads"],
        "downloads": s["downloads"],
        "reading": s["reading"],
        "error": s["error"],
        // the open photo's original (the photo screen's pill): `{state: original|loading|preview, progress?}`
        "original": session.active().and_then(|id| session.original_status(id)),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    // A local library has no server: nothing to ask, nothing runs, no summary
    #[test]
    fn a_local_library_never_syncs() {
        let mut s = Session::with_demo();
        let mut d = Driver::default();
        let ran = std::cell::Cell::new(0);
        let exec = |_: Task, _: Sender<Done>| ran.set(ran.get() + 1);
        let out = d.poll(&mut s, true, Some(&exec));
        assert_eq!(out, Polled::default());
        assert_eq!(ran.get(), 0);
        assert!(summary(&s).is_none());
    }

    fn session_in(dir: &std::path::Path) -> Session {
        let mut s = Session::new().with_fs();
        s.open_library(dir, false).unwrap();
        s
    }

    /// Poll until `done` says so (the passes run on threads of their own).
    fn poll_until(d: &mut Driver, s: &mut Session, mut done: impl FnMut(&mut Session) -> bool) {
        for _ in 0..400 {
            d.poll(s, true, None);
            if done(s) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("the cloud folder pass did not finish");
    }

    // A library with a cloud folder runs passes by itself: its preset reaches the other library, and a long pass shows in the
    // activity stack only while it runs
    #[test]
    fn a_library_with_a_cloud_folder_syncs_by_itself() {
        let root = std::env::temp_dir().join(format!("lc-apphost-cloud-folder-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let shared = root.join("Dropbox");
        std::fs::create_dir_all(&shared).unwrap();
        let (mut a, mut b) = (session_in(&root.join("a")), session_in(&root.join("b")));
        let (mut da, mut db) = (Driver::default(), Driver::default());
        // without a folder: nothing to come back to
        assert_eq!(da.poll(&mut a, true, None).again, None);

        a.presets.push(lightcraft_develop::Preset {
            id: "warm".into(),
            name: "Warm".into(),
            group: "Mine".into(),
            settings: serde_json::json!({"light": {"exposure": 0.3}}),
            favorite: false,
            builtin: false,
        });
        a.icloud_use_folder(Some(&shared)).unwrap();
        b.icloud_use_folder(Some(&shared)).unwrap();
        assert!(da.poll(&mut a, true, None).again.is_some());
        poll_until(&mut da, &mut a, |s| s.icloud_status(false)["cloudFolder"]["last"].is_object());
        poll_until(&mut db, &mut b, |s| s.presets.iter().any(|p| p.id == "warm"));
        assert_eq!(b.icloud_status(false)["cloudFolder"]["last"]["report"]["changed"], 1);
        assert!(b.activity.list().is_empty());

        // stopping turns the sync off and the driver rests
        b.icloud_use_folder(None).unwrap();
        assert_eq!(db.poll(&mut b, true, None).again, None);
    }
}
