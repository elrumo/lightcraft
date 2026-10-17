//! Export without freezing the page. A photo's render and encode take seconds on the CPU pipeline
//! (a 24 MP raw: decode, develop, JPEG), and the page has one thread: run on it, they stop every
//! repaint and every click for the length of the batch. Here each photo goes to a Web Worker of its
//! own instead (the same module as the render workers, `worker.js` → `handle_export` in
//! `workers.rs`): the worker reads the original from storage, renders it at the output size and
//! encodes it with the engine's one encoder
//! ([`lightcraft_engine::export::encode_render_export`]), and posts the file's bytes back. The page
//! only builds the job, places and downloads the file, and moves on to the next photo, so a frame
//! never holds more than that.
//!
//! The worker is an extra one, started for the batch and ended with it: the render workers keep
//! drawing thumbnails and the loupe, the memory a 24 MP export grows the worker's heap by (wasm
//! memory only grows) goes back to the browser when it ends, and ✕ in the activity stack ends the
//! worker at once instead of waiting for the photo in hand.
//!
//! What can't go to a worker runs on the page, one photo per turn of the event loop: copying an
//! original or writing a DNG (they are not renders), and everything when no worker can start
//! (`?store=memory`, which gives a worker no storage to read the originals from).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use js_sys::{Object, Promise, Uint8Array};
use lightcraft_apphost::export::{Pending, Prepared};
use lightcraft_engine::export::{BatchPlacer, Exported, PreparedExport};
use lightcraft_ui_egui::ExportRunner;
use serde_json::json;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

use crate::wire::ExportWire;
use crate::workers::{get, now, set};

/// How often the wait for a worker looks at ✕ (ms).
const CANCEL_POLL_MS: i32 = 100;

thread_local! {
    static STATS: RefCell<Stats> = RefCell::new(Stats::default());
    /// `web.crashExportWorker`: the next photo's worker throws instead of exporting.
    static CRASH_NEXT: Cell<bool> = const { Cell::new(false) };
}

/// Make the export worker of the next photo throw (`web.crashExportWorker`, for seeing a photo fail
/// and the next one get a new worker, as `web.crashWorker` does for the render workers).
pub fn crash_next() {
    CRASH_NEXT.with(|c| c.set(true));
}

/// What exports have done on this page (`web.stats` → `export`).
#[derive(Default)]
struct Stats {
    batches: u64,
    on_worker: u64,
    inline: u64,
    failed: u64,
    cancelled: u64,
    worker_starts: u64,
    /// The latest photo's time in the worker (ms) and the largest heap a worker reported (bytes).
    last_worker_ms: f64,
    peak_heap: f64,
}

fn count(f: impl FnOnce(&mut Stats)) {
    STATS.with(|s| f(&mut s.borrow_mut()));
}

/// `web.stats` → `export`.
pub fn stats() -> serde_json::Value {
    STATS.with(|s| {
        let s = s.borrow();
        json!({
            "batches": s.batches,
            "photosOnWorker": s.on_worker,
            "photosInline": s.inline,
            "failed": s.failed,
            "cancelled": s.cancelled,
            "workerStarts": s.worker_starts,
            "lastWorkerMs": s.last_worker_ms,
            "peakWorkerHeapMb": (s.peak_heap / 1048576.0).round(),
        })
    })
}

/// The page's [`ExportRunner`]: the batch goes to a task that goes on between frames. `store` is the
/// storage kind the render workers are started with.
pub fn runner(store: String, ctx: egui::Context) -> ExportRunner {
    Box::new(move |prepared, pending| {
        wasm_bindgen_futures::spawn_local(run(prepared, pending, store.clone(), ctx.clone()));
        Ok(())
    })
}

/// Let the browser draw and handle input before going on (a turn of the event loop).
async fn yield_to_page() {
    let promise = Promise::new(&mut |resolve, _| {
        if let Some(w) = web_sys::window() {
            let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, 0);
        }
    });
    let _ = JsFuture::from(promise).await;
}

/// How one photo of the batch came out.
enum Made {
    Done(Exported),
    Failed(String),
    /// ✕ was pressed: the batch ends here.
    Cancelled,
}

async fn run(prepared: Prepared, pending: Pending, store: String, ctx: egui::Context) {
    let Prepared { items, opts, to, after } = prepared;
    let total = items.len();
    count(|s| s.batches += 1);
    let mut worker: Option<ExportWorker> = None;
    let mut cancelled = false;
    // (the downloads are made here, on the page: a file is offered to the browser as it is placed)
    let mut write = |path: &str, bytes: &[u8]| crate::web::download(path, bytes);
    let exists = |_: &str| false;
    let mut placer = BatchPlacer::new(&opts, &to, &mut write, &exists, false, total);
    let mut error = None;
    for (i, item) in items.into_iter().enumerate() {
        if pending.cancel.load(Ordering::Relaxed) {
            cancelled = true;
            break;
        }
        pending.set_progress(i, &item.file_name);
        ctx.request_repaint();
        // a turn for the page to draw the row, whatever the photo needs
        yield_to_page().await;
        let head = item.head();
        let made = make(item, &mut worker, &store, &pending.cancel).await;
        let result = match made {
            Made::Done(file) => Ok(file),
            Made::Failed(e) => {
                count(|s| s.failed += 1);
                Err(e)
            }
            Made::Cancelled => {
                cancelled = true;
                break;
            }
        };
        if let Err(e) = placer.place(&head, result) {
            error = Some(e);
            break;
        }
    }
    let files = placer.finish();
    if cancelled {
        count(|s| s.cancelled += 1);
    }
    // (ends the worker, and with it the memory the batch grew)
    drop(worker);
    pending.set_progress(files.len(), "");
    let outcome = match error {
        Some(e) => Err(e),
        None => {
            let photos = after.run(&files, cancelled);
            Ok((files, photos, cancelled))
        }
    };
    pending.finish(outcome);
    ctx.request_repaint();
}

/// The compiled module the render workers are started with, if the page has one and a worker can
/// read the originals from the storage `store` names.
fn worker_module(store: &str) -> Option<JsValue> {
    let module = get(&js_sys::global(), "lightcraftModule");
    (store != "memory" && !module.is_undefined()).then_some(module)
}

/// Make `item`'s file: on the export worker when it is a render, else (or when no worker can be had)
/// here. The worker is started with the first photo that needs it and kept for the batch, unless
/// it fails: the next photo then gets a new one.
async fn make(item: PreparedExport, worker: &mut Option<ExportWorker>, store: &str, cancel: &Arc<AtomicBool>) -> Made {
    let Some(parts) = item.render_parts() else { return run_here(item) };
    let Some(module) = worker_module(store) else { return run_here(item) };
    if worker.is_none() {
        match ExportWorker::start(&module, store).await {
            Ok(w) => {
                count(|s| s.worker_starts += 1);
                *worker = Some(w);
            }
            Err(e) => {
                log::warn!("export worker: {e}; exporting on the page");
                return run_here(item);
            }
        }
    }
    let Some(w) = worker.as_ref() else { return run_here(item) };
    let name = item.file_name.clone();
    let Ok(text) = serde_json::to_string(&ExportWire::from_parts(&name, &parts)) else {
        return Made::Failed(format!("{name}: the export could not be described to the worker"));
    };
    drop(parts);
    match w.call(&text, cancel).await {
        Ok(reply) if get(&reply, "ok").is_truthy() => {
            let (width, height) = (get(&reply, "w").as_f64().unwrap_or(0.0) as usize, get(&reply, "h").as_f64().unwrap_or(0.0) as usize);
            let bytes = Uint8Array::new(&get(&reply, "bytes")).to_vec();
            count(|s| {
                s.on_worker += 1;
                s.last_worker_ms = get(&reply, "ms").as_f64().unwrap_or(0.0);
                s.peak_heap = s.peak_heap.max(get(&reply, "heap").as_f64().unwrap_or(0.0));
            });
            Made::Done(Exported { file_name: name, bytes, width, height, sidecars: Vec::new(), gpu_fallback: None })
        }
        // the worker is well and says why this photo failed
        Ok(reply) => Made::Failed(get(&reply, "err").as_string().unwrap_or_else(|| format!("{name}: the export worker failed"))),
        // it was ended (✕) or it died (out of memory, a trap): a new one for the next photo
        Err(e) => {
            *worker = None;
            if cancel.load(Ordering::Relaxed) {
                Made::Cancelled
            } else {
                Made::Failed(format!("{name}: the export worker stopped ({e}); if the photo is very large, export it smaller"))
            }
        }
    }
}

/// Run `item` on the page.
fn run_here(item: PreparedExport) -> Made {
    count(|s| s.inline += 1);
    match item.run() {
        Ok(file) => Made::Done(file),
        Err(e) => Made::Failed(e),
    }
}

/// A Web Worker running this module for one batch, taking one export at a time.
struct ExportWorker {
    worker: web_sys::Worker,
    /// `(resolve, reject)` of the message being waited for (the worker's "ready", then each reply).
    waiting: Rc<RefCell<Option<(js_sys::Function, js_sys::Function)>>>,
    next_id: Cell<u32>,
    _on_message: Closure<dyn FnMut(web_sys::MessageEvent)>,
    _on_error: Closure<dyn FnMut(JsValue)>,
}

impl Drop for ExportWorker {
    fn drop(&mut self) {
        self.worker.set_onmessage(None);
        self.worker.set_onerror(None);
        self.worker.terminate();
    }
}

impl ExportWorker {
    /// Start a worker and wait until it has opened storage.
    async fn start(module: &JsValue, store: &str) -> Result<ExportWorker, String> {
        let opts = web_sys::WorkerOptions::new();
        opts.set_type(web_sys::WorkerType::Module);
        opts.set_name("lightcraft-export");
        let worker = web_sys::Worker::new_with_options("./worker.js", &opts).map_err(|e| format!("{e:?}"))?;
        let waiting: Rc<RefCell<Option<(js_sys::Function, js_sys::Function)>>> = Rc::default();
        let w = waiting.clone();
        let on_message = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |e: web_sys::MessageEvent| {
            if let Some((resolve, _)) = w.borrow_mut().take() {
                let _ = resolve.call1(&JsValue::NULL, &e.data());
            }
        });
        worker.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        let w = waiting.clone();
        let on_error = Closure::<dyn FnMut(JsValue)>::new(move |e: JsValue| {
            let msg = get(&e, "message").as_string().unwrap_or_else(|| format!("{e:?}"));
            if let Some((_, reject)) = w.borrow_mut().take() {
                let _ = reject.call1(&JsValue::NULL, &msg.into());
            }
        });
        worker.set_onerror(Some(on_error.as_ref().unchecked_ref()));
        let this = ExportWorker { worker, waiting, next_id: Cell::new(0), _on_message: on_message, _on_error: on_error };
        let ready = this.expect_message();
        let init = Object::new();
        set(&init, "module", module);
        set(&init, "store", &store.into());
        this.worker.post_message(&init).map_err(|e| format!("{e:?}"))?;
        let hello = JsFuture::from(ready).await.map_err(|e| e.as_string().unwrap_or_else(|| format!("{e:?}")))?;
        if get(&hello, "ready").is_truthy() { Ok(this) } else { Err("the worker did not say it was ready".into()) }
    }

    /// A promise for the worker's next message (or error).
    fn expect_message(&self) -> Promise {
        let waiting = self.waiting.clone();
        Promise::new(&mut |resolve, reject| {
            *waiting.borrow_mut() = Some((resolve, reject));
        })
    }

    /// Send an export (`text`: an [`ExportWire`] as JSON) and wait for the worker's reply. `Err`: the
    /// worker failed or was ended because `cancel` was set (it is no use after that).
    async fn call(&self, text: &str, cancel: &Arc<AtomicBool>) -> Result<JsValue, String> {
        let reply = self.expect_message();
        let id = self.next_id.get().wrapping_add(1);
        self.next_id.set(id);
        let msg = Object::new();
        set(&msg, "id", &id.into());
        set(&msg, "export", &text.into());
        if CRASH_NEXT.with(|c| c.replace(false)) {
            set(&msg, "crash", &true.into());
        }
        let started = now();
        if let Err(e) = self.worker.post_message(&msg) {
            self.waiting.borrow_mut().take();
            return Err(format!("{e:?}"));
        }
        // ✕ ends the wait: the worker is stopped on the spot, not at the end of the photo
        let _watch = Watch::new(cancel.clone(), self.worker.clone(), self.waiting.clone());
        let r = JsFuture::from(reply).await.map_err(|e| e.as_string().unwrap_or_else(|| format!("{e:?}")));
        log::info!("export worker: {:.0} ms", now() - started);
        r
    }
}

/// An interval that ends the batch's worker when `cancel` is set, until it is dropped.
struct Watch {
    id: i32,
    _tick: Closure<dyn FnMut()>,
}

impl Watch {
    fn new(cancel: Arc<AtomicBool>, worker: web_sys::Worker, waiting: Rc<RefCell<Option<(js_sys::Function, js_sys::Function)>>>) -> Option<Watch> {
        let tick = Closure::<dyn FnMut()>::new(move || {
            if !cancel.load(Ordering::Relaxed) {
                return;
            }
            worker.terminate();
            if let Some((_, reject)) = waiting.borrow_mut().take() {
                let _ = reject.call1(&JsValue::NULL, &"cancelled".into());
            }
        });
        let id = web_sys::window()?.set_interval_with_callback_and_timeout_and_arguments_0(tick.as_ref().unchecked_ref(), CANCEL_POLL_MS).ok()?;
        Some(Watch { id, _tick: tick })
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        if let Some(w) = web_sys::window() {
            w.clear_interval_with_handle(self.id);
        }
    }
}
