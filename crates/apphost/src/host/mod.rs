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
//!   applyCrop?}`, `render.prefetch {photos, width, height}`, `render.cancel {slots}`,
//!   `render.retain {thumbs, max?}` — pictures arrive as `image` events.
//! - `ui.reply {id, result | error}` — the frontend's answer to a `uiRequest` (automation).

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
use lightcraft_raster::Rgba8;
use serde_json::{Value, json};

pub use leases::{BytesRef, ImageRef, Leases};
pub use protocol::{ErrorBody, Event, HostState, OpenConfig, Request};

use crate::render::{Present, Scheduler, Slot};

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
/// Thumbnails kept known to the scheduler beyond what `render.retain` lists.
const MAX_THUMBS: usize = 600;

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
struct Collect(Vec<(Slot, Rgba8)>);

impl Present<()> for Collect {
    fn make(&mut self, slot: Slot, image: Rgba8) {
        self.0.push((slot, image));
    }
    fn update(&mut self, slot: Slot, image: Rgba8, _: &mut ()) {
        self.0.push((slot, image));
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
}

impl Engine {
    fn start(config: &OpenConfig, events: Arc<Events>, leases: Arc<Leases>, wake: Sender<Msg>) -> Result<Engine, String> {
        let available = config.available_memory.and_then(|m| usize::try_from(m).ok()).or_else(lightcraft_sysmem::available_memory);
        crate::startup::set_memory_budget(available);
        let tmp = config.tmp.as_deref().map_or_else(std::env::temp_dir, PathBuf::from);
        crate::startup::clean_staging(&tmp);
        let mut warnings = Vec::new();
        let (mut session, demo) = if config.demo {
            (Session::with_demo().with_fs().with_system_clock(), true)
        } else {
            let dir = crate::startup::library_dir(config.library.clone().map(Into::into), config.home.clone().map(Into::into));
            let (session, fallback) = crate::startup::open_session(dir.as_deref());
            let demo = fallback.is_some();
            warnings.extend(fallback);
            (session, demo)
        };
        let home = config.home.as_deref().map(std::ffi::OsStr::new);
        crate::startup::configure_enhancer(&mut session.enhancer, home, available);
        let sched = match config.render_threads {
            Some(n) => Scheduler::with_worker_threads(n),
            None => Scheduler::default(),
        };
        sched.set_notify(Some(Arc::new(move || {
            let _ = wake.send(Msg::Wake);
        })));
        let library = session.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.display().to_string());
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
        if req.method.starts_with("ui.") && req.method != "ui.reply" && req.method != "ui.render" && req.method != "ui.menu.invoke" {
            let Some(reply) = call else {
                return Some(Event::err(id, ErrorBody::new("unknownMethod", format!("`{}` is answered by the UI itself", req.method))));
            };
            self.next_ui += 1;
            self.ui_pending.insert(self.next_ui, (id, reply.clone()));
            self.events.push(Event::UiRequest { id: self.next_ui, method: req.method, params: req.params }.to_value());
            return None;
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
                if cmd == "app.export" {
                    return crate::export::export_now(&mut self.session, &params, &mut |_, _, _| true).map_err(|e| ErrorBody::new("other", e));
                }
                self.session.execute(cmd, &params).map_err(|e| ErrorBody::engine(&e))
            }
            "engine.commands" => {
                let mut v: Vec<Value> = self.session.commands().into_iter().map(|c| serde_json::to_value(c).unwrap_or_default()).collect();
                v.push(crate::export::command_info(&self.session));
                Ok(Value::Array(v))
            }
            "host.state" => Ok(serde_json::to_value(self.snapshot()).unwrap_or_default()),
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
                self.busy = true;
                Ok(json!({"slot": Slot::Thumb(id).name()}))
            }
            "render.view" => {
                let id = photo(p, &self.session).ok_or_else(|| bad("no photo"))?;
                let (w, h) =
                    (size(p.get("width")).ok_or_else(|| bad("missing `width`"))?, size(p.get("height")).ok_or_else(|| bad("missing `height`"))?);
                let apply_crop = p.get("applyCrop").and_then(Value::as_bool).unwrap_or(true);
                let draft = p.get("draft").and_then(Value::as_bool).unwrap_or(false);
                let slot = if p.get("before").and_then(Value::as_bool).unwrap_or(false) {
                    let job = self.session.render_job(id, w, h, true, apply_crop).ok_or_else(|| bad("no such photo"))?;
                    self.sched.request(Slot::Before, job, 90);
                    Slot::Before
                } else {
                    let job = self.session.loupe_job(id, w, h, apply_crop).ok_or_else(|| bad("no such photo"))?;
                    self.sched.request(Slot::Main, if draft { job.draft() } else { job }, 100);
                    // until the loupe has this photo: its cached render, embedded preview or thumbnail
                    if self.sched.textures.get(&Slot::Main).is_none_or(|t| t.photo != id)
                        && let Some(q) = self.session.quick_view_job(id, w.max(h), apply_crop)
                    {
                        self.sched.request_quick(Slot::Preview, q, 110);
                    }
                    Slot::Main
                };
                self.busy = true;
                Ok(json!({"slot": slot.name()}))
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
        let mut collect = Collect::default();
        let polled = self.sched.poll(&mut self.session, &mut collect);
        for (slot, image) in collect.0 {
            let Some(shown) = self.sched.textures.get(&slot) else { continue };
            let (width, height) = (u32::try_from(image.width).unwrap_or(u32::MAX), u32::try_from(image.height).unwrap_or(u32::MAX));
            let name = slot.name();
            let token = self.leases.put_image(&name, Arc::new(image));
            let histogram = (slot == Slot::Main).then(|| shown.histogram.as_ref().map(|h| serde_json::to_value(h).unwrap_or_default())).flatten();
            let quick = shown.quick.map(|q| serde_json::to_value(q).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default());
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
            };
            self.events.push(event.to_value());
        }
        for (slot, photo, key, error) in polled.failed {
            self.events.push(Event::RenderFailed { slot: slot.name(), photo, key, error }.to_value());
        }
        self.busy = polled.wake_after.is_some_and(|d| d <= BUSY_TICK);
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

    fn snapshot(&mut self) -> HostState {
        let (visible_gen, visible) = self.session.visible_shared();
        HostState {
            catalog_revision: self.session.catalog.revision,
            visible_gen,
            visible_count: visible.len(),
            library_generation: self.session.library_generation(),
            active: self.session.active(),
            selection_count: self.session.selection.ids.len(),
            undo: self.session.undo.last().map(|u| u.label.clone()),
            redo: self.session.redo.last().map(|u| u.label.clone()),
            interaction: self.session.interaction.as_ref().map(|i| i.label.clone()),
            unsaved: self.session.unsaved().map_or(0, |(n, _)| n),
        }
    }

    /// Write everything that isn't on disk yet: an app in the background may be ended without notice.
    fn save(&mut self) {
        if let Err(e) = self.session.persist() {
            log::error!("saving the library: {e}");
        }
        self.session.save_view();
    }

    fn lifecycle(&mut self, event: &str) -> Result<(), String> {
        log::info!("lifecycle: {event}");
        match event {
            "resignActive" => self.save(),
            "background" => {
                self.save();
                // no GPU in the background, and a much smaller memory limit
                lightcraft_engine::gpu::pause();
                let freed = self.session.release_memory();
                log::info!("background: {} MB of decoded photos released", freed >> 20);
            }
            "foreground" | "active" => lightcraft_engine::gpu::resume(),
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

#[cfg(test)]
mod tests;
