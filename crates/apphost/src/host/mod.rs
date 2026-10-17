//! The engine on a thread of its own, driven by messages: what a frontend that isn't written in
//! Rust (the SwiftUI iOS app, through `apps/lightcraft-ios-swift`) talks to.
//!
//! One thread (`lc-engine`) owns the [`Session`] (it is `Send`, not `Sync`), as the egui app's UI
//! thread does: it answers [`Request`]s in order, runs the render [`Scheduler`], saves, and turns
//! what changed into [`Event`]s. A frontend [`Host::submit`]s requests from any thread and reads
//! events with [`Host::poll`] (blocking up to a timeout) on a thread of its own; pictures and JSON
//! answers that cross a C ABI are [`Leases`]. The thread sleeps while nothing happens: requests,
//! finished renders (the job pool's notify) and [`Host::wake`] wake it.
//!
//! Methods (`docs/ios-swift.md` → *Protocol*):
//! - `engine.execute {command, params}` — any engine command (`Session::execute`), and `app.export`
//!   ([`crate::export`]); `engine.commands` — every command, with whether it is enabled.
//! - `host.state` — the [`HostState`] (also sent as a `state` event when it changes);
//!   `host.lifecycle {event}` — `resignActive`, `background`, `foreground`, `memoryWarning`,
//!   `terminate`.
//! - `render.thumb {photo, edge, priority?}`, `render.view {photo?, width, height, draft?, before?,
//!   applyCrop?}` (a `draft` comes smaller, as fast as this host renders: a slider drag),
//!   `render.prefetch {photos, width, height}`, `render.cancel {slots}`, `render.retain {thumbs, max?}`
//!   — pictures arrive as `image` events. A thumbnail asked for once stays asked for, every tick, while
//!   `render.retain` lists its photo and it has no picture (`thumbs_wanted`).
//! - `import.scan {paths}` — the files and folders for the import review (answered when the scan
//!   ends: `{candidates}`, each with `inTrash`); `render.candidate {index, edge?}` — a candidate's
//!   thumbnail (slot `import:<index>`); `import.start {paths | indexes, …library.import params,
//!   albumName?}` — import in the background (answered when it ends: what it added).
//! - `host.admin.signIn | call | signOut | session` — the sync server's admin API, on worker threads
//!   ([`admin`]: the SwiftUI app's Server Administration screens).
//! - `ui.reply {id, result | error}` — the frontend's answer to a `uiRequest` (automation).
//!
//! Between requests the thread drives what the egui app's frame loop drives: renders, saving, the
//! activity stack and the self-hosted sync ([`crate::sync`]; a share link comes as a `copy` event,
//! the choice after signing in to a server that has a library as `ask {dialog: "syncChoice"}`).

mod admin;
mod browsing;
pub mod leases;
pub mod protocol;
mod script;

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use lightcraft_catalog::PhotoId;
use lightcraft_engine::Session;
use lightcraft_engine::cloud_sync::{Done, Task};
use lightcraft_raster::Rgba8;
use serde_json::{Value, json};

pub use leases::{BytesRef, ImageRef, Leases};
pub use protocol::{ErrorBody, Event, HostState, OpenConfig, Request};

use crate::render::{HdrPicture, Present, Scheduler, Slot};
use crate::startup::Profile;

/// Most events one [`Host::poll`] returns (the rest wait for the next).
pub const MAX_EVENTS: usize = 512;
/// How often the thread looks around while renders are in flight.
const BUSY_TICK: Duration = Duration::from_millis(16);
/// …and while nothing is (saving, background tasks, sync).
const IDLE_TICK: Duration = Duration::from_millis(250);
/// The most `activity` events a second.
const ACTIVITY_EVERY: Duration = Duration::from_millis(100);
/// How long an automation script waits for an answer.
const SCRIPT_TIMEOUT: Duration = Duration::from_secs(120);
/// Priority of preparing the neighbouring photos (as the egui loupe does).
const PREFETCH_PRIORITY: u32 = 4;
/// How long a preview (`render.variant`) is asked for again while it hasn't rendered.
const VARIANT_WANTED: Duration = Duration::from_secs(10);
/// Thumbnails kept known to the scheduler beyond what `render.retain` lists.
const MAX_THUMBS: usize = 600;
/// The most thumbnails asked for again every tick (`thumbs_wanted`): `render.retain` trims them to what is on
/// screen, this only bounds a frontend that never sends one (a frontend asks once: one dropped stays blank).
const MAX_WANTED: usize = 20_000;

/// Bytes a pixel of an original takes while it is decoded for a zoomed window: the linear source (12; a
/// window uploads only the part it reads, never the whole source) and the decode's temporaries at their
/// peak (12: the picture turned upright, or the highlight rebuild's pyramid with the samples). Measured
/// with `drag_bench zoom` on a 48 MP ProRAW: 1.39 to 1.48 GB above the fit view at the peak, the window's
/// render included; settled, 0.88 GB at 1:1 and 0.72 GB at 8x.
const ORIGINAL_BYTES_PER_PX: f64 = 24.0;

/// What is kept free beside a decoded original: the zoomed window's render (its GPU stages and
/// buffers: up to ~0.9 GB above the source for a phone's 1:1 window) and the app's pictures.
const ORIGINAL_RESERVE: f64 = 600.0 * 1048576.0;

/// Whether decoding an original of `px` pixels leaves room under the app's memory limit, given what the
/// system says is `left` before it ends the app (`None`: no such limit, a desktop) and whether the
/// original is `decoded` already (a 48 MP ProRAW needs 1.7 GB: an iPhone 17 Pro, which the system ends at
/// about 3.3 GB, has 2.1 to 2.4 GB left with one open).
fn original_fits(left: Option<usize>, decoded: bool, px: f64) -> bool {
    let Some(left) = left else { return true };
    decoded || px.is_finite() && px * ORIGINAL_BYTES_PER_PX + ORIGINAL_RESERVE <= left as f64
}

enum Msg {
    /// A frontend's request: answered with a `reply` event.
    Request(Request),
    /// A request answered on `reply` instead of the event queue (`Host::call`, scripts).
    Call(Request, Sender<Event>),
    /// Look around now (a render finished, `Host::wake`).
    Wake,
    /// Save, stop the renders and end the thread.
    Close,
}

/// The event queue: the engine thread pushes, [`Host::poll`] drains.
#[derive(Default)]
struct Events {
    queue: Mutex<(VecDeque<Value>, bool)>,
    cv: Condvar,
}

impl Events {
    fn push(&self, e: Value) {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner).0.push_back(e);
        self.cv.notify_all();
    }

    fn wake(&self) {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner).1 = true;
        self.cv.notify_all();
    }

    fn poll(&self, timeout: Duration) -> Vec<Value> {
        let deadline = Instant::now() + timeout;
        let mut q = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
        while q.0.is_empty() && !q.1 {
            let Some(left) = deadline.checked_duration_since(Instant::now()).filter(|d| !d.is_zero()) else { break };
            q = self.cv.wait_timeout(q, left).unwrap_or_else(PoisonError::into_inner).0;
        }
        q.1 = false;
        let n = q.0.len().min(MAX_EVENTS);
        q.0.drain(..n).collect()
    }
}

/// The engine, running on its own thread.
pub struct Host {
    tx: Sender<Msg>,
    events: Arc<Events>,
    leases: Arc<Leases>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl Host {
    /// Start the engine: open the library `config` names (or the demo), start its thread, and run
    /// the automation script if there is one. Returns the host and what was opened
    /// (`{library, photos, demo, warnings}`).
    pub fn open(config: OpenConfig) -> Result<(Host, Value), String> {
        Self::open_with(config, Arc::new(Leases::default()))
    }

    /// [`Self::open`], handing pictures out through `leases` (one registry for every engine of a
    /// process, so a token alone names its buffer: the C ABI's `lc_release`).
    pub fn open_with(config: OpenConfig, leases: Arc<Leases>) -> Result<(Host, Value), String> {
        let (tx, rx) = channel();
        let (opened_tx, opened_rx) = channel();
        let events = Arc::new(Events::default());
        let (ev, ls, wake) = (events.clone(), leases.clone(), tx.clone());
        let script_tx = tx.clone();
        let thread = std::thread::Builder::new()
            .name("lc-engine".into())
            .spawn(move || {
                let started = lightcraft_engine::guard::catch("starting the engine", || Engine::start(&config, ev, ls, wake)).and_then(|r| r);
                let mut engine = match started {
                    Ok(engine) => engine,
                    Err(e) => {
                        let _ = opened_tx.send(Err(e));
                        return;
                    }
                };
                let _ = opened_tx.send(Ok(engine.opened.clone()));
                if let Some(script) = config.script.as_deref().filter(|s| !s.is_empty()) {
                    let path = engine.tmp.join(script);
                    script::run(path, script_tx);
                }
                engine.run(rx);
            })
            .map_err(|e| format!("the engine thread could not start: {e}"))?;
        let opened = opened_rx.recv().map_err(|_| "the engine stopped while starting".to_string())??;
        Ok((Host { tx, events, leases, thread: Mutex::new(Some(thread)) }, opened))
    }

    /// Queue a request (JSON `{id, method, params}`); its answer comes as a `reply` event.
    pub fn submit(&self, request: &[u8]) -> Result<(), ErrorBody> {
        let req: Request = serde_json::from_slice(request).map_err(|e| ErrorBody::new("badRequest", format!("not a request: {e}")))?;
        self.tx.send(Msg::Request(req)).map_err(|_| ErrorBody::new("closed", "the engine has stopped"))
    }

    /// Run a request and wait up to `timeout` for its answer (a `reply` event, as JSON).
    pub fn call(&self, request: &[u8], timeout: Duration) -> Value {
        let req: Request = match serde_json::from_slice(request) {
            Ok(r) => r,
            Err(e) => return Event::err(0, ErrorBody::new("badRequest", format!("not a request: {e}"))).to_value(),
        };
        let id = req.id;
        let (reply_tx, reply_rx) = channel();
        if self.tx.send(Msg::Call(req, reply_tx)).is_err() {
            return Event::err(id, ErrorBody::new("closed", "the engine has stopped")).to_value();
        }
        match reply_rx.recv_timeout(timeout) {
            Ok(e) => e.to_value(),
            Err(_) => Event::err(id, ErrorBody::new("timeout", "no answer in time")).to_value(),
        }
    }

    /// The events since the last poll, waiting up to `timeout` for one (or for [`Self::wake`]).
    pub fn poll(&self, timeout: Duration) -> Vec<Value> {
        self.events.poll(timeout)
    }

    /// A waiting [`Self::poll`] returns now.
    pub fn wake(&self) {
        self.events.wake();
    }

    /// The buffers handed out across the ABI.
    pub fn leases(&self) -> &Leases {
        &self.leases
    }

    /// Save, stop the renders and end the engine thread, waiting up to `timeout` for it.
    /// Requests after this are refused (`closed`). Idempotent.
    pub fn close(&self, timeout: Duration) {
        let Some(thread) = self.thread.lock().unwrap_or_else(PoisonError::into_inner).take() else { return };
        let _ = self.tx.send(Msg::Close);
        let t0 = Instant::now();
        while !thread.is_finished() && t0.elapsed() < timeout {
            std::thread::sleep(Duration::from_millis(5));
        }
        if thread.is_finished() {
            if thread.join().is_err() {
                log::error!("the engine thread ended with a panic");
            }
        } else {
            log::warn!("the engine thread is still busy after {timeout:?}; not waiting for it");
        }
        self.events.wake();
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.close(Duration::from_secs(5));
    }
}

/// Collects finished renders' pixels (the scheduler keeps only their facts: a frontend that isn't
/// Rust keeps its own copy, so holding them here too would count every picture twice).
#[derive(Default)]
struct Collect(Vec<(Slot, Rgba8, Option<Arc<HdrPicture>>)>);

impl Present<()> for Collect {
    fn make(&mut self, slot: Slot, image: Rgba8) {
        self.0.push((slot, image, None));
    }
    fn update(&mut self, slot: Slot, image: Rgba8, _: &mut ()) {
        self.0.push((slot, image, None));
    }
    /// (called right after the `make` or `update` of the same render: its picture's HDR rendition, or none)
    fn set_hdr(&mut self, slot: Slot, hdr: Option<Arc<HdrPicture>>, _: &mut ()) {
        if let Some(last) = self.0.iter_mut().rev().find(|(s, ..)| *s == slot) {
            last.2 = hdr;
        }
    }
}

/// What lives on the engine thread.
struct Engine {
    session: Session,
    sched: Scheduler<()>,
    events: Arc<Events>,
    leases: Arc<Leases>,
    tmp: PathBuf,
    demo: bool,
    opened: Value,
    state: Option<HostState>,
    activity: Option<(Vec<lightcraft_engine::activity::TaskInfo>, Instant)>,
    /// `uiRequest`s sent to the frontend, by id, waiting for `ui.reply`.
    ui_pending: HashMap<u64, (u64, Sender<Event>)>,
    next_ui: u64,
    busy: bool,
    /// Self-hosted sync: requests in flight on worker threads ([`crate::sync`]).
    sync: crate::sync::Driver,
    sync_exec: Box<dyn Fn(Task, Sender<Done>)>,
    /// The app is in front (`host.lifecycle`): coming back pulls from the server at once.
    in_front: bool,
    /// When the frontend last asked for something (the user is at work: background scans hold back).
    last_request: Instant,
    /// When face recognition's scan is pumped next ([`crate::faces`]).
    faces_next: Instant,
    /// How far that scan is (`HostState::faces`).
    faces_scan: Option<Value>,
    /// `app.export` running on its worker: answered when it ends.
    export: Option<PendingExport>,
    /// `enhance.superRes` running on its worker: answered when it ends.
    super_res: Option<PendingSuperRes>,
    /// Photo Merge: its previews and full merges (`host.merge.*`), the preview request waiting for
    /// its picture, and a counter for the pictures' keys.
    merge: crate::merge::Runner,
    merge_preview: Option<(u64, Option<Sender<Event>>)>,
    merge_shown: u64,
    /// Export with Previous runs so far (each writes into a staging folder of its own).
    export_runs: u64,
    /// Lightroom catalog inspection or import on its workers, and the request answered when it ends.
    lightroom: crate::lightroom::Runner,
    lightroom_reply: Option<(u64, Option<Sender<Event>>)>,
    /// The server administration session and its requests in flight (`host.admin.*`).
    admin: admin::Admin,
    /// The zoom window last asked for (`render.view {zoom}`): its image event says where it goes.
    region: Option<RegionShown>,
    /// The system ends the app past a memory limit (a phone): a zoomed view keeps one window's stages.
    memory_limited: bool,
    /// `import.scan` running on its worker, and the candidates of the last one (`render.candidate`).
    scan: Option<PendingScan>,
    candidates: Vec<lightcraft_engine::import::ImportCandidate>,
    /// `import.start` running: answered when it ends.
    import: Option<PendingImport>,
    /// `library.browse` reading its folder on a worker (`crate::browse`): answered when it ends.
    browse: Option<browsing::PendingBrowse>,
    /// Wakes the thread (a worker has something for it).
    wake: Sender<Msg>,
    /// Previews asked for (`render.variant`) and not rendered yet: asked again every tick (the
    /// scheduler drops variants nobody has asked for lately), for a while.
    variants: HashMap<u64, (lightcraft_engine::media::RenderJob, Instant)>,
    /// Grid thumbnails asked for (`render.thumb`: size and priority) that have no picture yet, while the
    /// frontend lists them as on screen (`render.retain`): asked again every tick, because the engine
    /// downloads a synced photo's mini preview only while its cell keeps asking, and renders again once
    /// it has landed. (A frontend asks once, when the cell appears.)
    thumbs_wanted: HashMap<PhotoId, (usize, u32)>,
    /// The files handed to an external editor (`host.editIn`), watched for what it saves.
    edits: crate::external_edit::Watch,
}

/// A scan for the import review the frontend asked for.
struct PendingScan {
    id: u64,
    reply: Option<Sender<Event>>,
    scan: crate::import::Scan,
}

/// An import the frontend asked for: its batches are added to the catalog on this thread as its
/// worker readies them.
struct PendingImport {
    id: u64,
    reply: Option<Sender<Event>>,
    run: crate::import::Run,
    tally: crate::import::Tally,
    /// Its row in the activity stack (✕ stops it: no further files are started).
    guard: lightcraft_engine::activity::TaskGuard,
    undo0: usize,
    cancelled: bool,
}

/// Wakes the engine thread when an import's worker has readied a batch.
struct WakeOnReady(Sender<Msg>);

impl crate::import::Watch for WakeOnReady {
    fn ready(&self) {
        let _ = self.0.send(Msg::Wake);
    }
}

/// An export the frontend asked for, running in the background ([`crate::export::Batch`]): the
/// request it answers when it ends.
struct PendingExport {
    id: u64,
    reply: Option<Sender<Event>>,
    batch: crate::export::Batch,
}

/// A zoom window asked for: its render's key, the zoomed frame it is cut from and where.
#[derive(Clone, Copy)]
struct RegionShown {
    key: u64,
    full: (usize, usize),
    window: lightcraft_engine::pipeline::PixelWindow,
}

/// The largest picture the SwiftUI app draws in one piece (a window is at most this wide).
const TEXTURE_SIDE: usize = 8192;

/// Super Resolution under way, answered when it ends.
struct PendingSuperRes {
    id: u64,
    reply: Option<Sender<Event>>,
    running: crate::superres::Running,
}

impl Engine {
    fn start(config: &OpenConfig, events: Arc<Events>, leases: Arc<Leases>, wake: Sender<Msg>) -> Result<Engine, String> {
        let desktop = config.profile == Profile::Desktop;
        let available = config.available_memory.and_then(|m| usize::try_from(m).ok()).or_else(lightcraft_sysmem::available_memory);
        crate::startup::set_memory_budget_for(config.profile, available);
        let tmp = config.tmp.as_deref().map_or_else(std::env::temp_dir, PathBuf::from);
        crate::startup::clean_staging(&tmp);
        let mut warnings = Vec::new();
        let (mut session, demo) = if config.demo {
            (Session::with_demo().with_fs().with_system_clock(), true)
        } else {
            let dir = crate::startup::library_dir_for(config.profile, config.library.clone().map(Into::into), config.home.clone().map(Into::into));
            if desktop {
                // never a demo in the library's place: the user closes the other LightCraft or chooses another library
                (crate::startup::open_session_or_err(dir.as_deref())?, false)
            } else {
                let (session, fallback) = crate::startup::open_session(dir.as_deref());
                let demo = fallback.is_some();
                warnings.extend(fallback);
                (session, demo)
            }
        };
        if desktop {
            crate::startup::configure_desktop(&mut session);
        } else {
            let home = config.home.as_deref().map(std::ffi::OsStr::new);
            crate::startup::configure_enhancer(&mut session.enhancer, home, available);
            // (on a phone, what the app may use decides which Generative Expand models it is offered)
            session.set_app_memory(available);
            session.face_models_dir = crate::startup::face_models_dir(home);
        }
        // the photo-library originals the app fetches are staged with its picks (moved into the library when opened)
        session.set_photo_library_staging(Some(crate::startup::staging(&tmp).0));
        // searches by description and indexing run off the engine thread (as in egui); the tick applies them
        session.vision.background = true;
        let sched = match config.render_threads {
            Some(n) => Scheduler::with_worker_threads(n),
            None => Scheduler::default(),
        };
        let sync_wake = wake.clone();
        let host_wake = wake.clone();
        sched.set_notify(Some(Arc::new(move || {
            let _ = wake.send(Msg::Wake);
        })));
        let sync_exec = Box::new(crate::sync::native_exec(move || {
            let _ = sync_wake.send(Msg::Wake);
        }));
        let library = session.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.display().to_string());
        let edits = crate::external_edit::Watch::load(&session);
        let opened = json!({"library": library, "photos": session.catalog.len(), "demo": demo, "warnings": warnings});
        Ok(Engine {
            session,
            sched,
            events,
            leases,
            tmp,
            demo,
            opened,
            state: None,
            activity: None,
            ui_pending: HashMap::new(),
            next_ui: 0,
            busy: false,
            sync: crate::sync::Driver::default(),
            sync_exec,
            in_front: true,
            last_request: Instant::now(),
            faces_next: Instant::now(),
            faces_scan: None,
            export: None,
            lightroom: Default::default(),
            lightroom_reply: None,
            admin: Default::default(),
            super_res: None,
            merge: crate::merge::Runner::default(),
            merge_preview: None,
            merge_shown: 0,
            export_runs: 0,
            region: None,
            memory_limited: available.is_some(),
            scan: None,
            candidates: Vec::new(),
            import: None,
            browse: None,
            wake: host_wake,
            variants: HashMap::new(),
            thumbs_wanted: HashMap::new(),
            edits,
        })
    }

    fn run(&mut self, rx: Receiver<Msg>) {
        loop {
            let wait = if self.busy { BUSY_TICK } else { IDLE_TICK };
            let first = match rx.recv_timeout(wait) {
                Ok(m) => Some(m),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            };
            let mut close = false;
            for msg in first.into_iter().chain(std::iter::from_fn(|| rx.try_recv().ok())) {
                if self.handle(msg) {
                    close = true;
                    break;
                }
            }
            if close {
                self.shutdown();
                return;
            }
            if let Err(e) = lightcraft_engine::guard::catch("the engine's tick", || self.tick()) {
                self.events.push(Event::Message { text: e, level: "error".into() }.to_value());
            }
        }
        self.shutdown();
    }

    /// Handle one message; true when the engine should stop.
    fn handle(&mut self, msg: Msg) -> bool {
        match msg {
            Msg::Wake => false,
            Msg::Close => true,
            Msg::Request(req) => {
                let id = req.id;
                let reply = self.answer(req, None);
                if let Some(e) = reply {
                    self.events.push(e.to_value());
                } else {
                    log::debug!("request {id} answered later");
                }
                false
            }
            Msg::Call(req, reply_tx) => {
                if let Some(e) = self.answer(req, Some(&reply_tx)) {
                    let _ = reply_tx.send(e);
                }
                false
            }
        }
    }

    /// The answer to `req`, or `None` when it comes later (a `uiRequest` forwarded to the frontend).
    fn answer(&mut self, req: Request, call: Option<&Sender<Event>>) -> Option<Event> {
        let id = req.id;
        if req.method != "host.state" {
            self.last_request = Instant::now();
        }
        if req.method.starts_with("ui.") && req.method != "ui.reply" && req.method != "ui.render" && req.method != "ui.menu.invoke" {
            let Some(reply) = call else {
                return Some(Event::err(id, ErrorBody::new("unknownMethod", format!("`{}` is answered by the UI itself", req.method))));
            };
            self.next_ui += 1;
            self.ui_pending.insert(self.next_ui, (id, reply.clone()));
            self.events.push(Event::UiRequest { id: self.next_ui, method: req.method, params: req.params }.to_value());
            return None;
        }
        // scanning and importing run on workers: answered when they end
        if req.method == "import.scan" || req.method == "import.start" {
            let scan = req.method == "import.scan";
            return lightcraft_engine::guard::catch(&format!("`{}`", req.method), || {
                if scan { self.start_scan(id, call, &req.params) } else { self.start_import(id, call, &req.params) }
            })
            .unwrap_or_else(|e| Some(Event::err(id, ErrorBody::new("panic", e))));
        }
        // Export Now runs on a worker (its row in the activity stack, ✕ stops it): answered when it ends
        if matches!(req.method.as_str(), "engine.execute" | "ui.menu.invoke" | "command")
            && req.params.get("command").or(req.params.get("id")).and_then(Value::as_str) == Some("app.export")
        {
            let params = req.params.get("params").cloned().filter(|v| !v.is_null()).unwrap_or_else(|| json!({}));
            return lightcraft_engine::guard::catch("`app.export`", || self.start_export(id, call, &params))
                .unwrap_or_else(|e| Some(Event::err(id, ErrorBody::new("panic", e))));
        }
        // a Lightroom catalog is read and imported on workers (its row in the activity stack): answered
        // when it ends; `{status: true, cancel?}` answers at once
        if matches!(req.method.as_str(), "engine.execute" | "ui.menu.invoke" | "command")
            && let Some(command @ ("library.inspectLightroom" | "library.importLightroom")) =
                req.params.get("command").or(req.params.get("id")).and_then(Value::as_str)
        {
            let command = command.to_string();
            let params = req.params.get("params").cloned().filter(|v| !v.is_null()).unwrap_or_else(|| json!({}));
            return lightcraft_engine::guard::catch(&format!("`{command}`"), || self.start_lightroom(id, call, &command, &params))
                .unwrap_or_else(|e| Some(Event::err(id, ErrorBody::new("panic", e))));
        }
        // Super Resolution likewise (Enlarging, with its ✕): answered with the new photo when it ends
        if matches!(req.method.as_str(), "engine.execute" | "ui.menu.invoke" | "command")
            && req.params.get("command").or(req.params.get("id")).and_then(Value::as_str) == Some("enhance.superRes")
        {
            let params = req.params.get("params").cloned().filter(|v| !v.is_null()).unwrap_or_else(|| json!({}));
            return lightcraft_engine::guard::catch("`enhance.superRes`", || self.start_super_res(id, call, &params))
                .unwrap_or_else(|e| Some(Event::err(id, ErrorBody::new("panic", e))));
        }
        // Export with Previous: the last export's options, for the photos chosen now; on a worker like Export Now
        if req.method == "host.exportPrevious" {
            let Some(prev) = self.session.last_export.clone() else {
                return Some(Event::err(id, ErrorBody::new("other", "nothing exported yet: use Export…")));
            };
            let mut params = lightcraft_engine::export::ExportOptions::known_keys_only(&prev);
            if let Some(o) = params.as_object_mut() {
                o.remove("ids");
                o.remove("id");
                // no folder of the user's (a phone shares what it writes): a staging one of its own
                if !o.contains_key("dir") {
                    self.export_runs += 1;
                    let dir = crate::startup::staging(&self.tmp).1.join(format!("previous-{}", self.export_runs));
                    if let Err(e) = std::fs::create_dir_all(&dir) {
                        return Some(Event::err(id, ErrorBody::new("other", format!("{}: {e}", dir.display()))));
                    }
                    o.insert("dir".into(), json!(dir.to_string_lossy()));
                    o.insert("conflict".into(), json!("unique"));
                }
            }
            return lightcraft_engine::guard::catch("`host.exportPrevious`", || self.start_export(id, call, &params))
                .unwrap_or_else(|e| Some(Event::err(id, ErrorBody::new("panic", e))));
        }
        // showing a Local folder reads it on a worker (`crate::browse`): answered when it ends
        if matches!(req.method.as_str(), "engine.execute" | "ui.menu.invoke" | "command")
            && req.params.get("command").or(req.params.get("id")).and_then(Value::as_str) == Some("library.browse")
        {
            let params = req.params.get("params").cloned().filter(|v| !v.is_null()).unwrap_or_else(|| json!({}));
            return lightcraft_engine::guard::catch("`library.browse`", || self.start_browse(id, call, &params))
                .unwrap_or_else(|e| Some(Event::err(id, ErrorBody::new("panic", e))));
        }
        // a Local folder's subfolders, listed on a worker (a network share can take long)
        if req.method == "host.local.subfolders" {
            return self.list_subfolders(id, call, &req.params);
        }
        // the server's admin API (`host.admin.*`): on a worker, answered when the server has
        if req.method.starts_with("host.admin.") {
            return lightcraft_engine::guard::catch(&format!("`{}`", req.method), || self.start_admin(id, call, &req.method, &req.params))
                .unwrap_or_else(|e| Some(Event::err(id, ErrorBody::new("panic", e))));
        }
        // a merge preview: answered when its picture is drawn (a newer one answers the older "cancelled")
        if req.method == "host.merge.preview" {
            return lightcraft_engine::guard::catch("`host.merge.preview`", || self.start_merge_preview(id, call, &req.params))
                .unwrap_or_else(|e| Some(Event::err(id, ErrorBody::new("panic", e))));
        }
        let method = req.method.clone();
        let result = lightcraft_engine::guard::catch(&format!("`{method}`"), || self.dispatch(&req.method, &req.params))
            .unwrap_or_else(|e| Err(ErrorBody::new("panic", e)));
        Some(match result {
            Ok(v) => Event::ok(id, v),
            Err(e) => Event::err(id, e),
        })
    }

    fn dispatch(&mut self, method: &str, p: &Value) -> Result<Value, ErrorBody> {
        let bad = |msg: &str| ErrorBody::new("badParams", format!("`{method}`: {msg}"));
        let photo = |p: &Value, s: &Session| p.get("photo").and_then(Value::as_u64).map(PhotoId).or(s.active());
        let size = |v: Option<&Value>| v.and_then(Value::as_u64).map(|n| n.clamp(8, 16384) as usize);
        match method {
            "engine.execute" | "ui.menu.invoke" | "command" => {
                let cmd = p.get("command").or(p.get("id")).and_then(Value::as_str).ok_or_else(|| bad("missing `command`"))?;
                let params = p.get("params").cloned().filter(|v| !v.is_null()).unwrap_or_else(|| json!({}));
                self.session.execute(cmd, &params).map_err(|e| ErrorBody::engine(&e))
            }
            "engine.commands" => {
                let mut v: Vec<Value> = self.session.commands().into_iter().map(|c| serde_json::to_value(c).unwrap_or_default()).collect();
                v.push(crate::export::command_info(&self.session));
                Ok(Value::Array(v))
            }
            "host.state" => Ok(serde_json::to_value(self.snapshot()).unwrap_or_default()),
            // what memory the engine holds: its caches (`library.memory`) and the views' stage caches; and what
            // the system still lets the app use before it ends it (iOS)
            "host.memory" => {
                Ok(json!({"engine": self.session.memory_report(), "views": self.sched.memory(), "left": lightcraft_sysmem::available_memory()}))
            }
            "host.uiCommands" => Ok(Value::Array(
                crate::ui_commands::UI_COMMANDS
                    .iter()
                    .map(|(id, label, shortcut, menu)| json!({"id": id, "label": label, "shortcut": shortcut, "menu": menu}))
                    .collect(),
            )),
            // the keys no command declares itself (secondary keys, the rating and label digits) and the
            // commands that share a key by context (`crate::keys`)
            "host.keyBindings" => Ok(json!({
                "fixed": crate::keys::FIXED_KEYS
                    .iter()
                    .map(|(shortcut, command, params)| json!({"shortcut": shortcut, "command": command, "params": serde_json::from_str::<Value>(params).unwrap_or_else(|_| json!({}))}))
                    .collect::<Vec<_>>(),
                "contextual": crate::keys::CONTEXTUAL.iter().map(|(a, b)| json!([a, b])).collect::<Vec<_>>(),
            })),
            // the sidebar's arrangement, which the app keeps: `{layout?, command?, params?}` → the
            // arrangement after one of the `view.sidebar*` commands, and the sections shown
            // Local's places (`crate::local`) from the roots and hidden places the app keeps: `{roots, hidden,
            // browseRoot?, command?: addRoot | hide | restoreHidden, path?}` → `{roots, hidden, places, owner, browseRoot, current}`
            "host.local" => self.local_places(p).map_err(|e| bad(&e)),
            "host.sidebar" => {
                let mut layout = p.get("layout").cloned().map(crate::sidebar::SidebarLayout::from).unwrap_or_default();
                if let Some(command) = p.get("command").and_then(Value::as_str) {
                    crate::sidebar::command(&mut layout, command, p.get("params").unwrap_or(&Value::Null)).map_err(|e| bad(&e))?;
                }
                let shown: Vec<&str> = layout.shown().map(|s| s.id()).collect();
                Ok(json!({"layout": layout, "shown": shown}))
            }
            // Photo ▸ Edit In (`crate::external_edit`): the copy to hand to the editor, what it saved coming
            // back, the originals themselves, and what a phone's share sheet sends back by name
            "host.editIn" => self.edits.start(&mut self.session, p).map_err(|e| ErrorBody::engine(&e)),
            "host.editIn.check" => self.check_external_edits().map_err(|e| ErrorBody::engine(&e)),
            "host.editIn.originals" => crate::external_edit::originals(&self.session, p).map_err(|e| ErrorBody::new("other", e)),
            "host.editIn.claim" => {
                let paths: Vec<String> =
                    p.get("paths").and_then(Value::as_array).into_iter().flatten().filter_map(|v| v.as_str().map(str::to_string)).collect();
                let (r, text) = self.edits.claim(&mut self.session, &paths).map_err(|e| ErrorBody::engine(&e))?;
                self.note_external_edits(text);
                Ok(r)
            }
            "host.merge.start" | "host.merge.last" => {
                let ids = merge_ids(&self.session, p);
                let r = if method == "host.merge.last" {
                    let command = p.get("command").and_then(Value::as_str).ok_or_else(|| bad("missing `command`"))?;
                    self.merge.start_last(&self.session, command, &ids)
                } else {
                    self.merge.start(&self.session, &merge_options(p), &ids)
                };
                r.map_err(|e| ErrorBody::new("other", e))?;
                self.busy = true;
                Ok(json!({"started": true, "photos": ids.len()}))
            }
            "host.merge.cancel" => {
                self.merge.cancel_preview();
                if let Some((id, reply)) = self.merge_preview.take() {
                    Self::reply(&self.events, reply, Event::err(id, ErrorBody::new("cancelled", "the merge preview was closed")));
                }
                Ok(json!({}))
            }
            // the display can (or no longer can) show the range above SDR white: an HDR edit's `main`
            // and `region` pictures then come with an `hdr` rendition, as soon as they render again
            "render.hdrDisplay" => {
                let on = p.get("on").and_then(Value::as_bool).ok_or_else(|| bad("missing `on`"))?;
                self.sched.set_hdr_display(on);
                self.busy = true;
                Ok(json!({}))
            }
            "host.lifecycle" => {
                let event = p.get("event").and_then(Value::as_str).ok_or_else(|| bad("missing `event`"))?;
                self.lifecycle(event).map_err(|e| bad(&e))?;
                Ok(json!({}))
            }
            "ui.render" => {
                let id = photo(p, &self.session).ok_or_else(|| bad("no photo"))?;
                let edge = size(p.get("size").or(p.get("longEdge"))).unwrap_or(1600);
                let img = self.session.render_now(id, edge, edge).map_err(|e| ErrorBody::new("other", e))?.image;
                Ok(json!({"width": img.width, "height": img.height}))
            }
            "render.thumb" => {
                let id = p.get("photo").and_then(Value::as_u64).map(PhotoId).ok_or_else(|| bad("missing `photo`"))?;
                let edge = size(p.get("edge")).unwrap_or(256);
                let priority = p.get("priority").and_then(Value::as_u64).map_or(10, |n| n.min(99) as u32);
                self.sched.request_thumb(&mut self.session, id, edge, priority);
                if self.thumbs_wanted.len() >= MAX_WANTED {
                    self.thumbs_wanted.clear();
                }
                self.thumbs_wanted.insert(id, (edge, priority));
                self.busy = true;
                Ok(json!({"slot": Slot::Thumb(id).name()}))
            }
            "render.view" => {
                let id = photo(p, &self.session).ok_or_else(|| bad("no photo"))?;
                let (w, h) =
                    (size(p.get("width")).ok_or_else(|| bad("missing `width`"))?, size(p.get("height")).ok_or_else(|| bad("missing `height`"))?);
                let apply_crop = p.get("applyCrop").and_then(Value::as_bool).unwrap_or(true);
                let draft = p.get("draft").and_then(Value::as_bool).unwrap_or(false);
                if let Some(pid) = p.get("preset").and_then(Value::as_str) {
                    // resting on a preset: the photo with it, nothing committed (`hover`, its `key` to tell it by).
                    // `replacing`: in place of the last step (another preset applied, which a click would undo first)
                    let preset = self.session.presets.iter().find(|x| x.id == pid).ok_or_else(|| bad(&format!("no preset `{pid}`")))?;
                    let d = self.session.develop_of(id).ok_or_else(|| bad("no such photo"))?;
                    let base = match p.get("replacing").and_then(Value::as_bool) {
                        Some(true) => self.session.undo.last().and_then(|u| undone_develop(&u.op, id, 0)).unwrap_or(d),
                        _ => d,
                    };
                    let amount = p.get("amount").and_then(Value::as_f64).unwrap_or(100.0).clamp(0.0, 200.0) / 100.0;
                    let job = self.session.preview_job(id, w, h, apply_crop, &preset.apply(&base, amount)).ok_or_else(|| bad("no such photo"))?;
                    // (as a string: a u64 doesn't survive every JSON reader)
                    let key = self.sched.job_for(Slot::Hover, job.clone()).key.to_string();
                    self.sched.request(Slot::Hover, job, 105);
                    self.busy = true;
                    return Ok(json!({"slot": Slot::Hover.name(), "key": key}));
                }
                let key;
                let slot = if p.get("before").and_then(Value::as_bool).unwrap_or(false) {
                    let job =
                        self.sched.job_for(Slot::Before, self.session.render_job(id, w, h, true, apply_crop).ok_or_else(|| bad("no such photo"))?);
                    key = job.key;
                    self.sched.request(Slot::Before, job, 90);
                    Slot::Before
                } else {
                    // a drag renders a draft: smaller the slower this host renders (as egui's loupe)
                    let scale = f64::from(self.sched.draft_scale(draft, w as f64 * h as f64));
                    let (dw, dh) = ((w as f64 * scale).round().max(8.0) as usize, (h as f64 * scale).round().max(8.0) as usize);
                    let job = self.session.loupe_job(id, dw, dh, apply_crop).ok_or_else(|| bad("no such photo"))?;
                    let shown = overlay(p.get("overlay"));
                    // soft proofing: the photo as `space` (or a device profile) holds it, out-of-gamut colours marked
                    let proof = match p.get("proof").filter(|v| !v.is_null()) {
                        Some(v) => Some(serde_json::from_value::<lightcraft_pipeline::Proof>(v.clone()).map_err(|e| bad(&format!("`proof`: {e}")))?),
                        None => None,
                    };
                    let job = self.sched.job_for(Slot::Main, if draft { job.draft() } else { job }.with_overlay(shown).with_proof(proof));
                    key = job.key;
                    self.sched.request(Slot::Main, job, 100);
                    // zoomed in: the window on screen at the zoom scale too (a drag keeps the one there is;
                    // a proof is of the whole frame, as egui's)
                    match p.get("zoom") {
                        Some(zoom) if !draft && proof.is_none() => self.request_region(id, (w, h), apply_crop, zoom, shown),
                        Some(_) => {}
                        None => self.no_region(),
                    }
                    // until the loupe has this photo: its cached render, embedded preview or thumbnail
                    if self.sched.textures.get(&Slot::Main).is_none_or(|t| t.photo != id)
                        && let Some(q) = self.session.quick_view_job(id, w.max(h), apply_crop)
                    {
                        self.sched.request_quick(Slot::Preview, q, 110);
                    }
                    Slot::Main
                };
                self.busy = true;
                // the render the slot's picture will be once it comes (its image event's `key`), so a UI can say
                // it is still working on it: a string, as JSON numbers past 2^53 lose bits in some readers
                Ok(json!({"slot": slot.name(), "key": key.to_string()}))
            }
            "render.candidate" => {
                let i = p.get("index").and_then(Value::as_u64).ok_or_else(|| bad("missing `index`"))?;
                let edge = size(p.get("edge")).unwrap_or(256);
                let c = usize::try_from(i).ok().and_then(|i| self.candidates.get(i)).cloned().ok_or_else(|| bad("no such candidate"))?;
                let slot = Slot::Import(u32::try_from(i).unwrap_or(u32::MAX));
                if let Some(job) = self.session.candidate_thumb_job(&c, edge, i) {
                    self.sched.request_quick(slot, job, 4);
                    self.busy = true;
                }
                Ok(json!({"slot": slot.name()}))
            }
            "render.variant" => {
                let id = photo(p, &self.session).ok_or_else(|| bad("no photo"))?;
                let edge = size(p.get("edge")).unwrap_or(160);
                let d = self.session.develop_of(id).ok_or_else(|| bad("no such photo"))?;
                let settings = if let Some(pid) = p.get("preset").and_then(Value::as_str) {
                    // over the original photo (framed as it is), not its edit: the presets panel's pictures stay put
                    // while presets are applied, and a B&W one applied doesn't turn every other picture grey
                    let preset = self.session.presets.iter().find(|x| x.id == pid).ok_or_else(|| bad(&format!("no preset `{pid}`")))?;
                    let original = self.session.catalog.photo(id).ok_or_else(|| bad("no such photo"))?.import_defaults();
                    let base = lightcraft_engine::develop::DevelopSettings { crop: d.crop, orientation: d.orientation, ..original };
                    preset.apply(&base, p.get("amount").and_then(Value::as_f64).unwrap_or(100.0).clamp(0.0, 200.0) / 100.0)
                } else if let Some(profile) = p.get("profile").and_then(Value::as_str) {
                    d.with_profile(profile)
                } else if let Some(v) = p.get("version").and_then(Value::as_u64) {
                    let photo = self.session.catalog.photo(id).ok_or_else(|| bad("no such photo"))?;
                    let version = usize::try_from(v).ok().and_then(|v| photo.versions.get(v)).ok_or_else(|| bad("no such version"))?;
                    (*version.settings).clone()
                } else if p.get("edited").and_then(Value::as_bool) == Some(true) {
                    // the photo as it is (Compare, Survey, the filmstrip: several photos large at once)
                    (*d).clone()
                } else {
                    return Err(bad("give `preset`, `profile`, `version` or `edited: true`"));
                };
                let job = self.session.variant_job(id, &settings, edge).ok_or_else(|| bad("no such photo"))?;
                let key = job.key;
                if self.sched.variant(job.clone()).is_none() {
                    self.variants.insert(key, (job, Instant::now()));
                    self.busy = true;
                }
                Ok(json!({"slot": Slot::Variant(key).name()}))
            }
            "render.face" => {
                // a face as People shows it: `rect` as people.list and faces.person give it (on the upright photo)
                let id = photo(p, &self.session).ok_or_else(|| bad("no photo"))?;
                let edge = size(p.get("edge")).unwrap_or(160);
                let r = p.get("rect").ok_or_else(|| bad("missing `rect`"))?;
                let num = |k: &str, i: usize| r.get(k).or_else(|| r.get(i)).and_then(Value::as_f64).filter(|v| v.is_finite());
                let (Some(x0), Some(y0), Some(x1), Some(y1)) = (num("x0", 0), num("y0", 1), num("x1", 2), num("y1", 3)) else {
                    return Err(bad("`rect` must be {x0, y0, x1, y1} or [x0, y0, x1, y1]"));
                };
                let view = self.session.face_view(id, lightcraft_geom::Rect { x0, y0, x1, y1 });
                let job = self.session.face_job(id, view, edge).ok_or_else(|| bad("no such photo"))?;
                let key = job.key;
                if self.sched.variant(job.clone()).is_none() {
                    self.variants.insert(key, (job, Instant::now()));
                    self.busy = true;
                }
                Ok(json!({"slot": Slot::Variant(key).name()}))
            }
            "render.prefetch" => {
                let (w, h) = (size(p.get("width")).unwrap_or(1024), size(p.get("height")).unwrap_or(1024));
                let photos = p.get("photos").and_then(Value::as_array).ok_or_else(|| bad("missing `photos`"))?;
                for (n, id) in photos.iter().filter_map(Value::as_u64).take(2).enumerate() {
                    if let Some(job) = self.session.loupe_job(PhotoId(id), w, h, true) {
                        self.sched.prefetch(Slot::Prefetch(n as u8), job, PREFETCH_PRIORITY);
                    }
                }
                self.busy = true;
                Ok(json!({}))
            }
            "render.cancel" => {
                let slots = p.get("slots").and_then(Value::as_array).ok_or_else(|| bad("missing `slots`"))?;
                for s in slots.iter().filter_map(Value::as_str) {
                    let slot = Slot::parse(s).ok_or_else(|| bad(&format!("no slot `{s}`")))?;
                    self.sched.cancel(slot);
                }
                Ok(json!({}))
            }
            "render.retain" => {
                let keep: HashSet<PhotoId> =
                    p.get("thumbs").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_u64).map(PhotoId).collect();
                let max = p.get("max").and_then(Value::as_u64).map_or(MAX_THUMBS, |n| n.clamp(1, 100_000) as usize);
                self.sched.evict_thumbs(&keep, max);
                self.thumbs_wanted.retain(|id, _| keep.contains(id));
                Ok(json!({}))
            }
            "ui.reply" => {
                let ui = p.get("id").and_then(Value::as_u64).ok_or_else(|| bad("missing `id`"))?;
                let (id, reply) = self.ui_pending.remove(&ui).ok_or_else(|| bad("no such request"))?;
                let event = match p.get("error") {
                    Some(e) if !e.is_null() => Event::err(id, ErrorBody::new("ui", e.as_str().map_or_else(|| e.to_string(), str::to_string))),
                    _ => Event::ok(id, p.get("result").cloned().unwrap_or(Value::Null)),
                };
                let _ = reply.send(event);
                Ok(json!({}))
            }
            other => Err(ErrorBody::new("unknownMethod", format!("unknown method `{other}`"))),
        }
    }

    /// Collect finished renders, save, and report what changed.
    fn tick(&mut self) {
        // previews still wanted: asked again (until they render, for a few seconds)
        let now = Instant::now();
        let sched = &mut self.sched;
        self.variants.retain(|_, (job, at)| now.duration_since(*at) < VARIANT_WANTED && sched.variant(job.clone()).is_none());
        if !self.variants.is_empty() {
            self.busy = true;
        }
        // (a synced photo's variant waits for its preview: keep asking the server for it, as a grid cell does every frame)
        for (job, _) in self.variants.values() {
            self.session.note_thumb_ask(job.photo, true);
        }
        let mut collect = Collect::default();
        let polled = self.sched.poll(&mut self.session, &mut collect);
        for (slot, image, hdr) in collect.0 {
            let Some(shown) = self.sched.textures.get(&slot) else { continue };
            let (width, height) = (u32::try_from(image.width).unwrap_or(u32::MAX), u32::try_from(image.height).unwrap_or(u32::MAX));
            let name = slot.name();
            let token = self.leases.put_image(&name, Arc::new(image));
            // (the HDR rendition of the same render, a lease of its own)
            let hdr = hdr.map(|h| json!({"token": self.leases.put_hdr(&name, h.clone()), "width": h.width, "height": h.height, "peak": h.peak}));
            // (with the share of pixels clipped to black and to white: the clipping warnings)
            let histogram = (slot == Slot::Main)
                .then(|| {
                    shown.histogram.as_ref().map(|h| {
                        let mut v = serde_json::to_value(h).unwrap_or_default();
                        let (lo, hi) = h.clipping();
                        v["clipping"] = json!([lo, hi]);
                        v
                    })
                })
                .flatten();
            let quick = shown.quick.map(|q| serde_json::to_value(q).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default());
            // a zoom window: where it goes in the zoomed frame
            let window = (slot == Slot::Region)
                .then(|| self.region.filter(|r| r.key == shown.key))
                .flatten()
                .map(|r| json!({"full": [r.full.0, r.full.1], "x": r.window.x, "y": r.window.y, "w": r.window.w, "h": r.window.h}));
            let event = Event::Image {
                slot: name,
                photo: shown.photo,
                key: shown.key,
                token,
                width,
                height,
                is_final: !shown.draft && shown.quick.is_none(),
                draft: shown.draft,
                quick,
                ms: shown.ms,
                histogram,
                window,
                hdr,
            };
            self.events.push(event.to_value());
        }
        for (slot, photo, key, error) in polled.failed {
            self.events.push(Event::RenderFailed { slot: slot.name(), photo, key, error }.to_value());
        }
        self.busy = polled.wake_after.is_some_and(|d| d <= BUSY_TICK);
        let synced = self.sync.poll(&mut self.session, self.in_front, Some(&*self.sync_exec));
        let (sched, session) = (&mut self.sched, &mut self.session);
        let mut built = false;
        self.thumbs_wanted.retain(|id, (edge, priority)| {
            if sched.thumb(*id).is_some() {
                return false;
            }
            built |= sched.request_thumb(session, *id, *edge, *priority);
            true
        });
        self.busy |= built;
        if let Some(url) = synced.link {
            self.events.push(Event::Copy { text: url.clone() }.to_value());
            self.events.push(Event::Message { text: format!("Link copied: {url}"), level: "info".into() }.to_value());
        }
        if synced.ask_conflict {
            self.events.push(Event::Ask { dialog: "syncChoice".into(), params: json!({}) }.to_value());
        }
        self.pump_faces();
        self.poll_export();
        self.poll_super_res();
        self.poll_expand();
        self.poll_merge();
        self.poll_vision();
        self.poll_lightroom();
        self.poll_admin();
        self.poll_scan();
        self.poll_import();
        self.poll_browse();
        self.session.persist_if_dirty();
        let state = self.snapshot();
        if self.state.as_ref() != Some(&state) {
            self.events.push(Event::State { state: state.clone() }.to_value());
            self.state = Some(state);
        }
        let now = Instant::now();
        if self.activity.as_ref().is_none_or(|(_, at)| now.duration_since(*at) >= ACTIVITY_EVERY) {
            let tasks = self.session.activity.list();
            if self.activity.as_ref().is_none_or(|(last, _)| *last != tasks) {
                self.events.push(Event::Activity { tasks: tasks.clone() }.to_value());
                self.activity = Some((tasks, now));
            }
        }
    }

    /// Start `app.export` on a worker; `None` (answered when it ends) or the error now.
    fn start_export(&mut self, id: u64, call: Option<&Sender<Event>>, params: &Value) -> Option<Event> {
        if self.export.is_some() {
            return Some(Event::err(id, ErrorBody::new("other", "an export is already running")));
        }
        // a photo dragged into another app (`remember: false`) isn't what Export with Previous repeats
        let mut params = params.clone();
        let remember = params.as_object_mut().and_then(|o| o.remove("remember")) != Some(Value::Bool(false));
        let params = &params;
        let started = crate::export::prepare(&mut self.session, params).and_then(|prepared| {
            let write: crate::export::SharedWrite = Arc::new(lightcraft_engine::export::write_file);
            crate::export::Batch::start(&self.session, prepared, write, "Adding to Apple Photos…".into())
        });
        match started {
            Ok(batch) => {
                // for Export with Previous (the phone's export folder is a staging one: not kept)
                if remember {
                    crate::export::remember(&mut self.session, params, None);
                }
                self.export = Some(PendingExport { id, reply: call.cloned(), batch });
                None
            }
            Err(e) => Some(Event::err(id, ErrorBody::new("other", e))),
        }
    }

    /// Inspect or import a Lightroom catalog (`library.inspectLightroom` / `library.importLightroom`
    /// `{path, updateExisting?}`, `path` the `.lrcat` or a folder holding it): `None` (answered when
    /// it ends) or the error now. `{status: true, cancel?}`: what runs or last ran, at once.
    fn start_lightroom(&mut self, id: u64, call: Option<&Sender<Event>>, command: &str, p: &Value) -> Option<Event> {
        let path = p.get("path").and_then(Value::as_str);
        if p.get("status").and_then(Value::as_bool) == Some(true) || path.is_none() {
            if p.get("cancel").and_then(Value::as_bool) == Some(true) {
                self.lightroom.cancel();
            }
            return Some(Event::ok(id, self.lightroom.status()));
        }
        if self.scan.is_some() || self.import.is_some() || self.browse.is_some() {
            return Some(Event::err(id, ErrorBody::new("other", "an import is running")));
        }
        let path = std::path::PathBuf::from(path.unwrap_or_default());
        let wake: crate::lightroom::Wake = Arc::new(|| {});
        let words = |w: &'static str| w.to_string();
        let started = if command == "library.inspectLightroom" {
            self.lightroom.start_inspect(&self.session, &path, wake, &words)
        } else {
            let update = p.get("updateExisting").and_then(Value::as_bool).unwrap_or(false);
            self.lightroom.start_import(&mut self.session, &path, update, wake, &words)
        };
        match started {
            Ok(_) => {
                self.lightroom_reply = Some((id, call.cloned()));
                None
            }
            Err(e) => Some(Event::err(id, ErrorBody::new("other", e))),
        }
    }

    /// A Lightroom task that ended answers its request (an error or a cancel as an error).
    fn poll_lightroom(&mut self) {
        if !self.lightroom.running() {
            return;
        }
        self.busy = true;
        let words = |w: &'static str| w.to_string();
        let Some(done) = self.lightroom.poll(&mut self.session, Arc::new(|| {}), &words) else { return };
        let Some((id, reply)) = self.lightroom_reply.take() else { return };
        let event = match (done.value.get("error").and_then(Value::as_str), done.value.get("cancelled").and_then(Value::as_bool)) {
            (Some(e), _) => Event::err(id, ErrorBody::new("other", e)),
            (None, Some(true)) => Event::err(id, ErrorBody::new("cancelled", "cancelled")),
            _ => Event::ok(id, done.value),
        };
        Self::reply(&self.events, reply, event);
    }

    /// Start scanning `paths` for the import review; `None` (answered when it ends) or the error now.
    fn start_scan(&mut self, id: u64, call: Option<&Sender<Event>>, p: &Value) -> Option<Event> {
        let bad = |m: &str| Some(Event::err(id, ErrorBody::new("badParams", format!("`import.scan`: {m}"))));
        if self.scan.is_some() || self.import.is_some() || self.browse.is_some() {
            return Some(Event::err(id, ErrorBody::new("other", "an import is running")));
        }
        if !self.session.library.as_ref().is_some_and(|l| l.on_disk) {
            return Some(Event::err(id, ErrorBody::new("other", "photos can't be imported: the library isn't saved on this device")));
        }
        let paths: Vec<String> =
            p.get("paths").and_then(Value::as_array).into_iter().flatten().filter_map(|v| v.as_str().map(str::to_string)).collect();
        if paths.is_empty() {
            return bad("missing `paths`");
        }
        self.sched.forget_imports();
        self.candidates.clear();
        let scan = crate::import::Scan::start(&mut self.session, &paths, "Scanning folder");
        self.scan = Some(PendingScan { id, reply: call.cloned(), scan });
        None
    }

    /// A scan that has ended: the candidates (each with `inTrash` when its duplicate is in
    /// Recently Deleted), kept for `render.candidate`.
    fn poll_scan(&mut self) {
        let Some(outcome) =
            self.scan.as_ref().and_then(|p| if p.scan.cancelled() { Some(Err("cancelled".into())) } else { p.scan.poll("Looking for photos…") })
        else {
            return;
        };
        let Some(p) = self.scan.take() else { return };
        let event = match outcome {
            Ok(out) => {
                self.session.import_probes = out.probes;
                let catalog = &self.session.catalog;
                let candidates: Vec<Value> = out
                    .candidates
                    .iter()
                    .map(|c| {
                        let mut v = serde_json::to_value(c).unwrap_or_default();
                        let trashed = c.duplicate.is_some() && c.existing.is_some_and(|e| catalog.photo(PhotoId(e)).is_some_and(|ph| ph.deleted));
                        v["inTrash"] = json!(trashed);
                        v
                    })
                    .collect();
                self.candidates = out.candidates;
                Event::ok(p.id, json!({"candidates": candidates}))
            }
            Err(e) => Event::err(p.id, ErrorBody::new("other", e)),
        };
        Self::reply(&self.events, p.reply, event);
    }

    /// Start importing (`paths`, or `indexes` of the last scan's candidates, with `library.import`'s
    /// params; `albumName` makes a new album first); `None` (answered when it ends) or the error now.
    fn start_import(&mut self, id: u64, call: Option<&Sender<Event>>, p: &Value) -> Option<Event> {
        if self.import.is_some() || self.scan.is_some() || self.browse.is_some() {
            return Some(Event::err(id, ErrorBody::new("other", "an import is running")));
        }
        let mut paths: Vec<String> =
            p.get("paths").and_then(Value::as_array).into_iter().flatten().filter_map(|v| v.as_str().map(str::to_string)).collect();
        for i in p.get("indexes").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_u64) {
            if let Some(c) = usize::try_from(i).ok().and_then(|i| self.candidates.get(i)) {
                paths.push(c.path.clone());
            }
        }
        if paths.is_empty() {
            return Some(Event::err(id, ErrorBody::new("badParams", "`import.start`: no photos chosen")));
        }
        let undo0 = self.session.undo.len();
        let mut params = p.clone();
        if let Some(o) = params.as_object_mut() {
            o.remove("paths");
            o.remove("indexes");
        }
        // a new album, named in the review, is made first
        if let Some(name) = p.get("albumName").and_then(Value::as_str).map(str::trim).filter(|n| !n.is_empty()) {
            match self.session.execute("album.create", &json!({"name": name})) {
                Ok(r) => params["album"] = r["id"].clone(),
                Err(e) => return Some(Event::err(id, ErrorBody::engine(&e))),
            }
            if let Some(o) = params.as_object_mut() {
                o.remove("albumName");
            }
        }
        let watch: Arc<dyn crate::import::Watch> = Arc::new(WakeOnReady(self.wake.clone()));
        match crate::import::Run::begin(&mut self.session, paths, &params, Some(watch)) {
            Ok(run) => {
                let guard = self.session.activity.start("import", "Importing", lightcraft_engine::activity::Cancel::Flag(run.cancel.clone()));
                self.import = Some(PendingImport { id, reply: call.cloned(), run, tally: Default::default(), guard, undo0, cancelled: false });
                None
            }
            Err(e) => Some(Event::err(id, ErrorBody::new("other", e))),
        }
    }

    /// Add the batches the import's worker has readied; when it is done, one undo step, the first
    /// photo selected, and the answer (the tally; `cancelled` when ✕ stopped it).
    fn poll_import(&mut self) {
        let Some(imp) = self.import.as_mut() else { return };
        let batches = imp.run.next_batches();
        let finished = batches.is_none();
        for prepared in batches.into_iter().flatten() {
            let n = prepared.len();
            match crate::import::commit(&mut self.session, &mut imp.run, prepared) {
                Ok(v) => imp.tally.add(&v),
                Err(e) => {
                    log::warn!("import: {e}");
                    imp.tally.failed += n;
                }
            }
        }
        let (done, total) = (imp.run.done.load(std::sync::atomic::Ordering::Relaxed), imp.run.total.load(std::sync::atomic::Ordering::Relaxed));
        imp.guard.progress(done as u64, total as u64);
        imp.cancelled |= imp.guard.is_cancelled();
        if !finished {
            self.busy = true;
            return;
        }
        let Some(imp) = self.import.take() else { return };
        let steps = self.session.undo.len().saturating_sub(imp.undo0);
        self.session.merge_undo(steps, &imp.tally.label());
        if let Some(first) = imp.tally.first {
            let _ = self.session.execute("library.select", &json!({"ids": [first]}));
        }
        self.sched.forget_imports();
        let mut out = imp.tally.to_json();
        out["cancelled"] = json!(imp.cancelled);
        out["album"] = json!(imp.run.album);
        Self::reply(&self.events, imp.reply, Event::ok(imp.id, out));
    }

    /// Answer a request that waited: on its own channel (`Host::call`), else as an event.
    fn reply(events: &Events, reply: Option<Sender<Event>>, event: Event) {
        match reply {
            Some(tx) => {
                let _ = tx.send(event);
            }
            None => events.push(event.to_value()),
        }
    }

    /// An export that has ended: its answer (the files; `cancelled` when ✕ stopped it).
    /// The window of the zoomed loupe that is on screen (`render.view {zoom: {drawn, visible}}`:
    /// the photo's long edge as drawn, in pixels, and the part of it on screen, as fractions),
    /// planned with [`crate::region`] and rendered at the zoom scale to be drawn over `main`.
    fn request_region(&mut self, id: PhotoId, canvas: (usize, usize), apply_crop: bool, zoom: &Value, overlay: lightcraft_pipeline::Overlay) {
        use crate::region;
        let drawn = zoom.get("drawn").and_then(Value::as_f64).filter(|v| v.is_finite());
        let visible: Vec<f64> = zoom
            .get("visible")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_f64).filter(|v| v.is_finite()).collect())
            .unwrap_or_default();
        let (Some(drawn), [x0, y0, x1, y1]) = (drawn, visible.as_slice()) else {
            self.no_region();
            return;
        };
        let Some(photo) = self.session.catalog.photo(id) else { return };
        let d = self.session.develop_of(id).unwrap_or_default();
        let lens = self.session.embedded_lens(id);
        let frame =
            lightcraft_pipeline::geometry::Frame::with_lens(photo.width.max(1) as usize, photo.height.max(1) as usize, &d, apply_crop, lens.as_ref());
        let (nw, nh) = frame.native_size();
        if !(nw.is_finite() && nh.is_finite() && nw >= 1.0 && nh >= 1.0) {
            return;
        }
        // a phone ends the app past its memory limit: when decoding the original for a window past the
        // preview's size would take it there, the windows are cut from the preview's frame instead
        // (magnified past it: soft, but the app stays)
        let native_long = nw.max(nh).round() as usize;
        let decoded = self.session.media.has_source(id, lightcraft_engine::SourceLevel::Full);
        let px = f64::from(photo.width) * f64::from(photo.height);
        let native_long = if original_fits(lightcraft_sysmem::available_memory(), decoded, px) {
            native_long
        } else {
            log::info!("zoom: the original of photo {} doesn't fit in the memory left: windows from the preview", id.0);
            native_long.min(lightcraft_engine::SourceLevel::Preview.max_edge())
        };
        let sizes = region::ViewSizes {
            drawn_long: drawn as f32,
            canvas_long: canvas.0.max(canvas.1) as f32,
            native_long,
            texture_side: TEXTURE_SIDE,
            draft: None,
            windows: region::overlay_supports_window(overlay),
        };
        let Some(edge) = region::plan(0, sizes).window_edge else {
            self.no_region();
            return;
        };
        let aspect = nw / nh;
        let (fw, fh) = if aspect >= 1.0 { (edge as f64, edge as f64 / aspect) } else { (edge as f64 * aspect, edge as f64) };
        let (fw, fh) = (fw.round().max(1.0) as usize, fh.round().max(1.0) as usize);
        let visible = ((x0 * fw as f64) as f32, (y0 * fh as f64) as f32, (x1 * fw as f64) as f32, (y1 * fh as f64) as f32);
        // refused: what the window reads (a spot's source, an Auto Mask stroke) is too far for one render
        let job = region::window_for(fw, fh, visible, region::max_span(TEXTURE_SIDE))
            .and_then(|win| Some((win, self.session.region_job(id, fw, fh, win, apply_crop)?)));
        let Some((window, job)) = job else {
            self.no_region();
            return;
        };
        // the original a window is cut from stays decoded while the view is zoomed (over the cache budget: a
        // 48 MP one is 560 MB), or every pan would decode it again
        if job.level == lightcraft_engine::SourceLevel::Full {
            self.session.media.set_on_screen(Some(id));
        }
        let job = job.with_overlay(overlay);
        // another window: what the last one's stages hold can't serve it (only a pan back), and a phone needs the room
        if self.memory_limited && self.region.as_ref().is_some_and(|r| (r.full, r.window) != ((fw, fh), window)) {
            self.sched.clear_stages(Slot::Region);
        }
        self.region = Some(RegionShown { key: job.key, full: (fw, fh), window });
        self.sched.request(Slot::Region, job, 95);
        self.busy = true;
    }

    /// The loupe shows no window: the original windows were cut from may go (zoomed out: memory back at once).
    fn no_region(&mut self) {
        self.region = None;
        self.session.media.set_on_screen(None);
    }

    /// Start enlarging `{id?}` (the active photo); `None` (answered when it ends) or the error now.
    fn start_super_res(&mut self, id: u64, call: Option<&Sender<Event>>, p: &Value) -> Option<Event> {
        if self.super_res.is_some() {
            return Some(Event::err(id, ErrorBody::new("other", "Super Resolution is already running")));
        }
        let Some(photo) = p.get("id").and_then(Value::as_u64).map(PhotoId).or(self.session.active()) else {
            return Some(Event::err(id, ErrorBody::new("badParams", "`enhance.superRes`: no photo")));
        };
        match crate::superres::Running::start(&mut self.session, photo, None) {
            Ok(running) => {
                self.super_res = Some(PendingSuperRes { id, reply: call.cloned(), running });
                self.busy = true;
                None
            }
            Err(e) => Some(Event::err(id, ErrorBody::new("other", e))),
        }
    }

    /// A Generative Expand started with `crop.generate {wait: false}`: store its fill when it ends
    /// (one undo step) and say how it went, as egui's `expand::poll`.
    fn poll_expand(&mut self) {
        let Some(r) = self.session.poll_expand() else { return };
        let (text, level) = match r {
            Ok(m) => (format!("New area painted with {}", m.label()), "info"),
            Err(e) if e.starts_with("cancelled") => ("Generate cancelled".to_string(), "info"),
            Err(e) => (format!("Generate failed: {e}"), "error"),
        };
        self.events.push(Event::Message { text, level: level.into() }.to_value());
    }

    fn poll_super_res(&mut self) {
        let Some(outcome) = self.super_res.as_ref().and_then(|p| p.running.poll(&mut self.session)) else { return };
        let Some(p) = self.super_res.take() else { return };
        let event = match outcome {
            Ok(v) => Event::ok(p.id, v),
            Err(e) => Event::err(p.id, ErrorBody::new(if e.contains("cancelled") { "cancelled" } else { "other" }, e)),
        };
        Self::reply(&self.events, p.reply, event);
    }

    fn start_merge_preview(&mut self, id: u64, call: Option<&Sender<Event>>, p: &Value) -> Option<Event> {
        let ids = merge_ids(&self.session, p);
        if ids.len() < 2 {
            return Some(Event::err(id, ErrorBody::new("badParams", "select at least 2 photos to merge")));
        }
        // the preview waiting is dropped for this one
        if let Some((old, reply)) = self.merge_preview.take() {
            Self::reply(&self.events, reply, Event::err(old, ErrorBody::new("cancelled", "a newer preview took its place")));
        }
        match self.merge.preview(&self.session, &merge_options(p), &ids) {
            Ok(()) => {
                self.merge_preview = Some((id, call.cloned()));
                self.busy = true;
                None
            }
            Err(e) => Some(Event::err(id, ErrorBody::new("other", e))),
        }
    }

    /// A merge preview's picture (slot `merge`) and its answer; a full merge's message.
    fn poll_merge(&mut self) {
        if !self.merge.busy() {
            return;
        }
        self.busy = true;
        for done in self.merge.poll(&mut self.session, &|stage| stage.to_string()) {
            match done {
                crate::merge::Done::Preview { result, .. } => {
                    let Some((id, reply)) = self.merge_preview.take() else { continue };
                    let event = match result {
                        Ok((img, info)) => {
                            if let Some(img) = img {
                                self.merge_shown += 1;
                                let (width, height) = (u32::try_from(img.width).unwrap_or(u32::MAX), u32::try_from(img.height).unwrap_or(u32::MAX));
                                let token = self.leases.put_image("merge", Arc::new(img));
                                self.events.push(
                                    Event::Image {
                                        slot: "merge".into(),
                                        photo: PhotoId(0),
                                        key: self.merge_shown,
                                        token,
                                        width,
                                        height,
                                        is_final: true,
                                        draft: false,
                                        quick: None,
                                        ms: 0.0,
                                        histogram: None,
                                        window: None,
                                        hdr: None,
                                    }
                                    .to_value(),
                                );
                            }
                            Event::ok(id, json!({"info": info, "slot": "merge"}))
                        }
                        Err(e) => Event::err(id, ErrorBody::new("other", e)),
                    };
                    Self::reply(&self.events, reply, event);
                }
                crate::merge::Done::Merged { options, result, cancelled } => {
                    let what = crate::merge::what(&options.command);
                    let (text, level) = match result {
                        Ok(_) => (format!("{what} merge added"), "info"),
                        Err(_) if cancelled => (format!("{what} merge cancelled"), "info"),
                        Err(e) => (format!("{what} merge failed: {e}"), "error"),
                    };
                    self.events.push(Event::Message { text, level: level.into() }.to_value());
                }
            }
        }
    }

    /// A search by description that finished becomes the view's filter (the state event shows it);
    /// what background vision work has to say is said.
    fn poll_vision(&mut self) {
        let polled = self.session.vision_poll();
        for text in polled.messages {
            self.events.push(Event::Message { text, level: "info".into() }.to_value());
        }
        if self.session.vision.busy() || self.session.vision.searching() {
            self.busy = true;
        }
    }

    fn poll_export(&mut self) {
        let Some(outcome) = self.export.as_ref().and_then(|p| p.batch.poll()) else { return };
        let Some(p) = self.export.take() else { return };
        let event = match outcome {
            Ok((files, photos, cancelled)) => {
                let mut out = crate::export::answer(files, photos);
                out["cancelled"] = json!(cancelled);
                Event::ok(p.id, out)
            }
            Err(e) => Event::err(p.id, ErrorBody::new("other", e)),
        };
        Self::reply(&self.events, p.reply, event);
    }

    /// Face recognition's background scan (Settings ▸ Faces): the next photos for its workers, as
    /// fast as what the user is doing allows.
    fn pump_faces(&mut self) {
        let now = Instant::now();
        if now < self.faces_next {
            return;
        }
        let worked = now.duration_since(self.last_request).as_secs_f64();
        let pace = crate::faces::pace_for(self.in_front, !self.in_front, worked, worked, false);
        let answer = self.session.execute("faces.pump", &json!({"pace": pace})).unwrap_or(Value::Null);
        // what Settings ▸ Faces shows of it (the state event carries it)
        self.faces_scan = (answer["active"] == true).then(
            || json!({"pendingPhotos": answer["pendingPhotos"].as_u64().unwrap_or(0), "indexedFaces": answer["indexedFaces"].as_u64().unwrap_or(0)}),
        );
        let wait = crate::faces::next_pump(&answer);
        if wait > 0.0 && wait < IDLE_TICK.as_secs_f64() {
            // photos waiting for the workers: look again soon (the loop ticks fast meanwhile)
            self.faces_next = now + Duration::from_secs_f64(wait);
            self.busy = true;
        } else {
            // idle or off: every idle tick is soon enough (switching it on is seen at once)
            self.faces_next = now + Duration::from_secs_f64(wait.max(IDLE_TICK.as_secs_f64()));
        }
    }

    fn snapshot(&mut self) -> HostState {
        let (visible_gen, visible) = self.session.visible_shared();
        HostState {
            catalog_revision: self.session.catalog.revision,
            visible_gen,
            visible_count: visible.len(),
            library_generation: self.session.library_generation(),
            presets_generation: presets_generation(&self.session),
            active: self.session.active(),
            selection_count: self.session.selection.ids.len(),
            undo: self.session.undo.last().map(|u| u.label.clone()),
            redo: self.session.redo.last().map(|u| u.label.clone()),
            interaction: self.session.interaction.as_ref().map(|i| i.label.clone()),
            active_mask: self.session.active_mask,
            active_spot: self.session.active_spot,
            unsaved: self.session.unsaved().map_or(0, |(n, _)| n),
            sync: crate::sync::summary(&self.session),
            faces: self.faces_scan.clone(),
        }
    }

    /// Write everything that isn't on disk yet: an app in the background may be ended without notice.
    fn save(&mut self) {
        if let Err(e) = self.session.persist() {
            log::error!("saving the library: {e}");
        }
        self.session.save_view();
    }

    /// Bring back what an external editor saved to the files Edit In handed it, and say so.
    fn check_external_edits(&mut self) -> Result<Value, lightcraft_engine::EngineError> {
        let (r, text) = self.edits.check(&mut self.session)?;
        self.note_external_edits(text);
        Ok(r)
    }

    /// Tell the frontend what came back from an external editor (nothing: `None`).
    fn note_external_edits(&mut self, text: Option<String>) {
        if let Some(text) = text {
            self.events.push(Event::Message { text, level: "info".into() }.to_value());
            self.busy = true;
        }
    }

    fn lifecycle(&mut self, event: &str) -> Result<(), String> {
        log::info!("lifecycle: {event}");
        match event {
            "resignActive" => {
                self.in_front = false;
                self.save();
            }
            "background" => {
                self.in_front = false;
                self.save();
                // no GPU in the background, and a much smaller memory limit
                lightcraft_engine::gpu::pause();
                let freed = self.session.release_memory();
                log::info!("background: {} MB of decoded photos released", freed >> 20);
            }
            "foreground" | "active" => {
                self.in_front = true;
                lightcraft_engine::gpu::resume();
                // (an editor saved while the app was away)
                if let Err(e) = self.check_external_edits() {
                    log::warn!("checking the external edits: {e}");
                }
            }
            "memoryWarning" => {
                let freed = self.session.release_memory();
                log::warn!("memory warning: {} MB of decoded photos released", freed >> 20);
            }
            "terminate" => {
                self.save();
                if !self.demo
                    && let Err(e) = self.session.close_library()
                {
                    log::error!("closing the library: {e}");
                }
            }
            other => return Err(format!("unknown lifecycle event `{other}`")),
        }
        Ok(())
    }

    fn shutdown(&mut self) {
        self.sched.set_notify(None);
        self.sched.shutdown(Duration::from_secs(2));
        self.save();
        if !self.demo
            && let Err(e) = self.session.close_library()
        {
            log::error!("closing the library: {e}");
        }
        for (_, (id, reply)) in self.ui_pending.drain() {
            let _ = reply.send(Event::err(id, ErrorBody::new("closed", "the engine has stopped")));
        }
        self.events.push(Event::Closed.to_value());
    }
}

/// Where `path` (from a script) points: relative paths are next to the script.
pub(crate) fn beside(script: &Path, path: &str) -> String {
    let p = Path::new(path);
    if p.is_relative()
        && let Some(dir) = script.parent()
    {
        return dir.join(p).to_string_lossy().to_string();
    }
    path.to_string()
}

/// `render.view`'s `overlay`: `{kind: "mask", id, view?, color?: [r,g,b], opacity?}`, every visible
/// mask at once `{kind: "allMasks", view?, color?, opacity?}`, `{kind: "spots", threshold?}`,
/// `{kind: "pointColor", index}` or `{kind: "hdr"}`; anything else draws none.
fn overlay(v: Option<&Value>) -> lightcraft_pipeline::Overlay {
    use lightcraft_pipeline::{MaskView, Overlay};
    let Some(v) = v else { return Overlay::None };
    let num = |k: &str, max: f64, d: f64| v.get(k).and_then(Value::as_f64).filter(|n| n.is_finite()).unwrap_or(d).round().clamp(0.0, max);
    let c = v.get("color").and_then(Value::as_array);
    let ch = |i: usize, d: u8| c.and_then(|c| c.get(i)).and_then(Value::as_u64).map_or(d, |n| n.min(255) as u8);
    let view = v.get("view").and_then(Value::as_str).and_then(MaskView::parse).unwrap_or_default();
    match v.get("kind").and_then(Value::as_str) {
        Some("mask") => {
            let Some(id) = v.get("id").and_then(Value::as_u64) else { return Overlay::None };
            Overlay::Mask {
                id: id.min(u64::from(u16::MAX)) as u16,
                view,
                color: [ch(0, 230), ch(1, 30), ch(2, 40)],
                opacity: num("opacity", 100.0, 50.0) as u8,
            }
        }
        Some("allMasks") => Overlay::AllMasks { view, color: [ch(0, 230), ch(1, 30), ch(2, 40)], opacity: num("opacity", 100.0, 50.0) as u8 },
        Some("spots") => Overlay::Spots(num("threshold", 100.0, 50.0) as u8),
        Some("pointColor") => v.get("index").and_then(Value::as_u64).map_or(Overlay::None, |i| Overlay::PointColorRange(i.min(255) as u8)),
        Some("hdr") => Overlay::HdrRange,
        Some("depth") => Overlay::Depth,
        _ => Overlay::None,
    }
}

/// The develop settings undoing `op` (an undo step) gives photo `id` back, if it changes them.
fn undone_develop(op: &lightcraft_catalog::Op, id: PhotoId, depth: u8) -> Option<Arc<lightcraft_engine::develop::DevelopSettings>> {
    use lightcraft_catalog::Op;
    match op {
        Op::SetDevelop { id: i, settings, .. } if *i == id => Some(settings.clone()),
        // (batches nest a few deep: a step's ops, a photo's develop + history)
        Op::Batch { ops } if depth < 8 => ops.iter().find_map(|o| undone_develop(o, id, depth + 1)),
        _ => None,
    }
}

/// The photos a `host.merge.*` request names (`ids`), else the selection.
fn merge_ids(s: &Session, p: &Value) -> Vec<PhotoId> {
    match p.get("ids").and_then(Value::as_array) {
        Some(a) => a.iter().filter_map(Value::as_u64).map(PhotoId).collect(),
        None => s.selected(),
    }
}

/// A `host.merge.*` request's options: `options` (the dialog's), for `command`.
fn merge_options(p: &Value) -> crate::merge::MergeOptions {
    let mut o: crate::merge::MergeOptions = p.get("options").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default();
    if let Some(c) = p.get("command").and_then(Value::as_str) {
        o.command = c.to_string();
    }
    o
}

/// The state's `presetsGeneration`: the preset list's generation, moved also by the imported (LUT)
/// profiles, imported here or brought by the sync (the profile browser lists them, and a preset that
/// uses one looks different once it is here), and by the presets and groups hidden and the groups' order.
fn presets_generation(s: &Session) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.presets.generation().hash(&mut h);
    s.hidden_presets.hash(&mut h);
    s.preset_group_order.hash(&mut h);
    for p in &s.lut_profiles {
        (&p.id, &p.name, &p.group, &p.file).hash(&mut h);
    }
    h.finish()
}

#[cfg(test)]
mod tests;
