//! Self-hosted sync on the frame loop (`docs/sync.md`): the engine says what to send next
//! ([`lightcraft_engine::Session::sync_tasks`]), [`Driver`] hands each request to
//! [`crate::Services::sync_exec`] (a worker thread on the desktop) and the answers come back through
//! [`Driver::poll`], so a slow or absent server never holds a frame. Pulls every few seconds while signed
//! in, and when the window comes back to the front.
//!
//! (Not to be confused with [`crate::sync`], the Synchronize Folder dialog.)

use std::sync::mpsc::{Receiver, Sender, channel};
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

use lightcraft_engine::Session;
use lightcraft_engine::cloud_sync::{Done, POLL, Task};
use lightcraft_engine::usage::LocalUsage;

use crate::LightcraftApp;

/// Runs one sync task off the UI thread and sends its answer back (then repaints).
pub type SyncExec = Box<dyn Fn(Task, Sender<Done>, egui::Context)>;

/// The native transport: each request on its own worker thread (uploads and downloads stream
/// files), answered through the channel; the window is woken after each answer.
#[cfg(not(target_arch = "wasm32"))]
pub fn native_exec() -> SyncExec {
    Box::new(|task, tx, ctx| {
        let id = task.id();
        let failed = tx.clone();
        let spawned = std::thread::Builder::new().name("lc-sync".into()).spawn(move || {
            let _ = tx.send(lightcraft_engine::cloud_sync::run(&task));
            ctx.request_repaint();
        });
        if let Err(e) = spawned {
            let _ = failed.send(Done::failed(id, format!("can't start a sync thread: {e}")));
        }
    })
}

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
    pub again: Option<std::time::Duration>,
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
        out.again = Some(if busy { std::time::Duration::from_millis(500) } else { POLL });
        out
    }
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

/// The question open under Settings ▸ Sync's sign-out buttons: what its answer does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignOutHow {
    /// Send what is left, then sign out and remove the library from this device.
    Wait,
    /// Stop waiting: remove the library now; what hasn't been sent is lost.
    Anyway,
    /// The server signed this device out: remove the library from it.
    Remove,
}

#[derive(Default)]
pub struct SyncDriver {
    /// Requests in flight and their answers.
    pub driver: Driver,
    pub form: SyncForm,
    pub storage: Storage,
    /// The sign-out question being asked in Settings ▸ Sync (cleared when it no longer applies).
    pub sign_out: Option<SignOutHow>,
}

impl SyncDriver {
    /// Another library is on screen (one was opened, or this one was emptied for another account): the sign-in form,
    /// a sign-out question and the storage numbers of the old one are not the new one's.
    pub fn forget_library(&mut self) {
        self.form = SyncForm::default();
        self.sign_out = None;
        self.storage = Storage::default();
    }
}

/// How often this computer's folders are measured again while Settings ▸ Sync is open.
#[cfg(not(target_arch = "wasm32"))]
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
    #[cfg(not(target_arch = "wasm32"))]
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
        let s = &mut app.cloud_sync.storage;
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
            app.cloud_sync.storage.counts = app.session.photo_counts();
            let (tx, rx) = channel();
            let ctx = ctx.clone();
            let spawned = std::thread::Builder::new().name("lc-usage".into()).spawn(move || {
                let _ = tx.send(lightcraft_engine::usage::measure(&dirs));
                ctx.request_repaint();
            });
            match spawned {
                Ok(_) => app.cloud_sync.storage.measuring = Some(rx),
                Err(e) => {
                    log::warn!("storage: can't start a thread: {e}");
                    app.cloud_sync.storage.at = Some(now);
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
        _ if value >= 99.5 => format!("{value:.0} {}", UNITS.get(unit).copied().unwrap_or("TB")),
        _ => format!("{value:.1} {}", UNITS.get(unit).copied().unwrap_or("TB")),
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
    let focused = ctx.input(|i| i.focused);
    let exec = app.services.sync_exec.as_ref().map(|exec| move |t: Task, tx: Sender<Done>| exec(t, tx, ctx.clone()));
    let polled = app.cloud_sync.driver.poll(&mut app.session, focused, exec.as_ref().map(|e| e as &dyn Fn(Task, Sender<Done>)));
    // a link just made goes on the clipboard
    if let Some(url) = polled.link {
        ctx.copy_text(url.clone());
        app.toast_for(ctx, format!("Link copied: {url}"), 4.0);
    }
    // two libraries with photos met: ask what to do about it (once; nothing syncs meanwhile)
    if polled.ask_conflict {
        app.ui.dialog = Some(crate::state::Dialog::SyncChoice);
    }
    if let Some(again) = polled.again {
        ctx.request_repaint_after(again);
    }
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
        "signingOut" => {
            // (what is left to send: the changes, and the photos still to upload)
            let photos = s["unsent"].as_u64().unwrap_or(0).max(s["uploads"].as_u64().unwrap_or(0));
            let mut text = format!("Signing out of {server}: sending what is left ({} change(s), {photos} photo(s))", s["pending"]);
            if let Some(e) = st.error() {
                text.push_str(&format!("\n{e}\nLightCraft keeps trying; Sign Out Anyway doesn't wait."));
            }
            (text, st.error().is_some(), true)
        }
        "paused" => (format!("Syncing with {server} is paused"), false, false),
        "error" => (format!("Can't sync with {server}: {}\nLightCraft keeps trying.", st.error().unwrap_or("")), true, false),
        "syncing" if st.reading().is_some() => {
            let (read, total) = st.reading().unwrap_or_default();
            (format!("Loading the library from {server}: {} of {} photos", count_label(read as u64), count_label(total as u64)), false, true)
        }
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

    /// A local library has no server: nothing to ask, nothing runs.
    #[test]
    fn a_local_library_never_syncs() {
        let mut s = Session::with_demo();
        let mut d = Driver::default();
        let ran = std::cell::Cell::new(0);
        let exec = |_: Task, _: Sender<Done>| ran.set(ran.get() + 1);
        let out = d.poll(&mut s, true, Some(&exec));
        assert_eq!(out, Polled::default());
        assert_eq!(ran.get(), 0);
    }
}
