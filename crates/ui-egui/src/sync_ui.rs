//! Self-hosted sync on the frame loop (`docs/sync.md`): the engine says what to send next
//! ([`lightcraft_engine::Session::sync_tasks`]), the host runs it ([`crate::Services::sync_exec`]:
//! a worker thread on the desktop) and the answers come back here, so a slow or absent server
//! never holds a frame. Pulls every few seconds while signed in, and when the window comes back
//! to the front.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use lightcraft_engine::sync::{Done, POLL, Task};
use lightcraft_engine::usage::LocalUsage;

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
    /// Combine this library with the server's when both have photos.
    pub merge: bool,
}

pub struct SyncDriver {
    tx: Sender<Done>,
    rx: Receiver<Done>,
    /// Tasks handed to the host and not answered yet.
    pub in_flight: usize,
    focused: bool,
    pub form: SyncForm,
    pub storage: Storage,
    /// The choice after signing in to a server that has a library already was put to the user (once
    /// per sign-in: it can be put off, and is offered again from the cloud button and Settings).
    choice_asked: bool,
}

impl Default for SyncDriver {
    fn default() -> Self {
        let (tx, rx) = channel();
        SyncDriver { tx, rx, in_flight: 0, focused: false, form: SyncForm::default(), storage: Storage::default(), choice_asked: false }
    }
}

/// How often this computer's folders are measured again while Settings ▸ Sync is open.
const MEASURE_EVERY: f64 = 30.0;

/// What this computer takes, for Settings ▸ Sync's storage section. Reading every file's size
/// takes a while on a big library, so a worker thread does it and a frame only picks up the answer.
#[derive(Default)]
pub struct Storage {
    /// The last measurement (`None`: not yet, or not a library on a disk).
    pub local: Option<LocalUsage>,
    /// (photos in the library, those with only previews here) when `local` was measured.
    pub counts: (usize, usize),
    #[cfg(not(target_arch = "wasm32"))]
    measuring: Option<Receiver<LocalUsage>>,
    /// `ctx` time of the last measurement.
    at: Option<f64>,
}

/// Keep the storage numbers current while Settings ▸ Sync shows them: the server is asked (the
/// engine says when: at most every 20 s) and this computer measured again every 30 s, or at
/// once with `refresh`. Call it every frame the section is visible.
pub fn storage_poll(app: &mut LightcraftApp, ctx: &egui::Context, refresh: bool) {
    if refresh {
        app.session.sync_refresh_usage();
    } else {
        app.session.sync_want_usage();
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let now = ctx.input(|i| i.time);
        let s = &mut app.sync.storage;
        if let Some(rx) = &s.measuring {
            match rx.try_recv() {
                Ok(u) => {
                    s.local = Some(u);
                    s.at = Some(now);
                    s.measuring = None;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => s.measuring = None,
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(Duration::from_millis(250));
                }
            }
        }
        if s.measuring.is_none()
            && (refresh || s.at.is_none_or(|t| now - t >= MEASURE_EVERY))
            && let Some(dirs) = app.session.local_dirs()
        {
            app.sync.storage.counts = app.session.photo_counts();
            let (tx, rx) = channel();
            let ctx = ctx.clone();
            let spawned = std::thread::Builder::new().name("lc-usage".into()).spawn(move || {
                let _ = tx.send(lightcraft_engine::usage::measure(&dirs));
                ctx.request_repaint();
            });
            match spawned {
                Ok(_) => app.sync.storage.measuring = Some(rx),
                Err(e) => {
                    log::warn!("storage: can't start a thread: {e}");
                    app.sync.storage.at = Some(now);
                }
            }
        }
    }
    #[cfg(target_arch = "wasm32")]
    let _ = ctx;
}

/// Bytes as a person reads them: `412 GB`, `1.9 GB`, `37 MB` (decimal units, as the system's file
/// manager shows them).
pub fn bytes_label(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 999.5 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    match unit {
        0 => format!("{bytes} B"),
        _ if value >= 99.5 => format!("{value:.0} {}", UNITS[unit]),
        _ => format!("{value:.1} {}", UNITS[unit]),
    }
}

/// A count with thousands separators: `22,796`.
pub fn count_label(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Hand finished tasks to the engine and start the next ones (cheap when there's nothing to do).
pub fn poll(app: &mut LightcraftApp, ctx: &egui::Context) {
    while let Ok(d) = app.sync.rx.try_recv() {
        app.sync.in_flight = app.sync.in_flight.saturating_sub(1);
        app.session.sync_done(d);
    }
    // a link just made goes on the clipboard
    if let Some(url) = app.session.sync_take_new_link() {
        ctx.copy_text(url.clone());
        app.toast_for(ctx, format!("Link copied: {url}"), 4.0);
    }
    // two libraries with photos met: ask what to do about it (once; nothing syncs meanwhile)
    if app.session.sync_conflict().is_some() {
        if !std::mem::replace(&mut app.sync.choice_asked, true) {
            app.ui.dialog = Some(crate::state::Dialog::SyncChoice);
        }
    } else {
        app.sync.choice_asked = false;
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
        "conflict" => (format!("{server} has a library already: choose what to do with this library's photos"), true, false),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_counts_read_well() {
        assert_eq!(bytes_label(0), "0 B");
        assert_eq!(bytes_label(999), "999 B");
        assert_eq!(bytes_label(1_000), "1.0 KB");
        assert_eq!(bytes_label(37_400_000), "37.4 MB");
        assert_eq!(bytes_label(412_300_000_000), "412 GB");
        assert_eq!(bytes_label(1_900_000_000), "1.9 GB");
        assert_eq!(bytes_label(999_900_000), "1.0 GB", "no `1000 MB`");
        assert_eq!(bytes_label(4_000_000_000_000), "4.0 TB");
        assert_eq!(bytes_label(u64::MAX), "18446744 TB");
        assert_eq!(count_label(0), "0");
        assert_eq!(count_label(999), "999");
        assert_eq!(count_label(22_796), "22,796");
        assert_eq!(count_label(1_234_567), "1,234,567");
    }
}
