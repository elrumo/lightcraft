//! Self-hosted sync on the frame loop (`docs/sync.md`): the engine says what to send next
//! ([`lightcraft_engine::Session::sync_tasks`]), the host runs it ([`crate::Services::sync_exec`]:
//! a worker thread on the desktop) and the answers come back here, so a slow or absent server
//! never holds a frame. Pulls every few seconds while signed in, and when the window comes back
//! to the front.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use lightcraft_engine::sync::{Done, POLL, Task};

use crate::LightcraftApp;

/// Runs one sync task off the UI thread and sends its answer back (then repaints).
pub type SyncExec = Box<dyn Fn(Task, Sender<Done>, egui::Context)>;

/// The native transport: each request on its own worker thread (uploads and downloads stream
/// files), answered through the channel.
#[cfg(not(target_arch = "wasm32"))]
pub fn native_exec() -> SyncExec {
    Box::new(|task, tx, ctx| {
        let id = task.id();
        let done = tx.clone();
        let spawned = std::thread::Builder::new().name("lc-sync".into()).spawn(move || {
            let _ = tx.send(lightcraft_engine::sync::run(&task));
            ctx.request_repaint();
        });
        if let Err(e) = spawned {
            let _ = done.send(Done::failed(id, format!("can't start a sync thread: {e}")));
        }
    })
}

/// The Settings > Sync form (never saved: the password only goes to the server).
#[derive(Clone, Debug, Default)]
pub struct SyncForm {
    pub server: String,
    pub user: String,
    pub password: String,
}

pub struct SyncDriver {
    tx: Sender<Done>,
    rx: Receiver<Done>,
    /// Tasks handed to the host and not answered yet.
    pub in_flight: usize,
    focused: bool,
    pub form: SyncForm,
}

impl Default for SyncDriver {
    fn default() -> Self {
        let (tx, rx) = channel();
        SyncDriver { tx, rx, in_flight: 0, focused: false, form: SyncForm::default() }
    }
}

/// Hand finished tasks to the engine and start the next ones (cheap when there's nothing to do).
pub fn poll(app: &mut LightcraftApp, ctx: &egui::Context) {
    while let Ok(d) = app.sync.rx.try_recv() {
        app.sync.in_flight = app.sync.in_flight.saturating_sub(1);
        app.session.sync_done(d);
    }
    let Some(exec) = app.services.sync_exec.as_ref() else { return };
    let Some(st) = app.session.sync_state() else { return };
    if !st.signed_in() || st.config.paused {
        return;
    }
    let focused = ctx.input(|i| i.focused);
    if focused && !app.sync.focused {
        app.session.sync_soon();
    }
    app.sync.focused = focused;
    for t in app.session.sync_tasks() {
        app.sync.in_flight += 1;
        exec(t, app.sync.tx.clone(), ctx.clone());
    }
    let busy = app.sync.in_flight > 0 || app.session.sync_state().is_some_and(|st| st.state() == "syncing");
    ctx.request_repaint_after(if busy { Duration::from_millis(500) } else { POLL });
}

/// The top bar cloud icon's tooltip, and whether it shows a problem / activity. (English only for
/// now: `tr_format!` messages need every catalog's translation.)
pub fn cloud_status(app: &LightcraftApp) -> (String, bool, bool) {
    let Some(st) = app.session.sync_state() else {
        return (crate::i18n::tr("Local library — no cloud account needed").to_string(), false, false);
    };
    let s = st.status();
    let server = st.config.server.trim_start_matches("https://").trim_start_matches("http://");
    match st.state() {
        "signedOut" => (format!("Signed out of {server}: sign in again in Settings > Sync"), false, false),
        "paused" => (format!("Syncing with {server} is paused"), false, false),
        "error" => (format!("Can't sync with {server}: {}\nLightCraft keeps trying.", st.error().unwrap_or("")), true, false),
        "syncing" => (
            format!("Syncing with {server}: {} change(s) to send, {} upload(s), {} download(s)", s["pending"], s["uploads"], s["downloads"]),
            false,
            true,
        ),
        _ => (format!("Synced with {server}"), false, false),
    }
}
