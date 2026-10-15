//! Self-hosted sync driven from a frontend's loop (`docs/sync.md`): the engine says what to send next
//! ([`Session::sync_tasks`]), the host runs it off its own thread ([`native_exec`]: a worker thread
//! per request) and the answers come back through [`Driver::poll`], so a slow or absent server never
//! holds a frame or the engine thread. Shared by the egui app (its frame loop) and the engine host
//! (its tick).

use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use lightcraft_engine::Session;
use lightcraft_engine::cloud_sync::{Done, POLL, Task};

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
}

impl Default for Driver {
    fn default() -> Self {
        let (tx, rx) = channel();
        Driver { tx, rx, in_flight: 0, focused: false, conflict_asked: false }
    }
}

impl Driver {
    /// Hand finished tasks to the engine and start the next ones on `exec` (cheap when there is
    /// nothing to do). `focused`: the app is in front. Without `exec` (no network on this
    /// platform) answers are still applied and nothing new starts.
    pub fn poll(&mut self, session: &mut Session, focused: bool, exec: Option<&dyn Fn(Task, Sender<Done>)>) -> Polled {
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
}
