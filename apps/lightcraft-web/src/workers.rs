//! Render workers: dedicated Web Workers, each running a second instance of this wasm module
//! (`worker.js` → [`worker_main`]). The main thread compiles the module once and posts the
//! compiled `WebAssembly.Module` to every worker, so a worker starts in a few milliseconds.
//!
//! This doesn't need wasm threads (shared memory, `+atomics`, a nightly `build-std`), only
//! module workers: the main thread posts a [`WireJob`] as JSON, the worker renders it and
//! transfers the RGBA bytes back (no copy across the boundary).
//!
//! Main side: [`Workers`] implements the UI's [`RenderOffload`]: one job per worker at a time,
//! with affinity (a photo goes back to the worker that already decoded it), the rendered-
//! thumbnail memory cache checked before dispatch, and the thumbnail storage index
//! ([`ThumbIndex`]) kept and pruned here. If no worker comes up, jobs run inline.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use js_sys::{Array, Object, Reflect, Uint8Array, Uint32Array};
use lightcraft_engine::catalog::PhotoId;
use lightcraft_engine::media::{RenderJob, RenderResult, SourceLevel};
use lightcraft_engine::pipeline::{Quality, Rendered};
use lightcraft_preview::{Hash128, PreviewCache};
use lightcraft_raster::{Histogram, Rgba8};
use lightcraft_ui_egui::render::{RenderOffload, Slot};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

use crate::backend::Backend;
use crate::health::{MAX_RESTARTS, restart_delay_ms};
use crate::wire::{CacheWatch, ExportWire, MAX_PIPELINE, ThumbIndex, WireJob, WorkerCore, WorkerSlots, choose_worker, thumb_storage_key};

/// Storage key of the thumbnail index.
pub const THUMB_INDEX: &str = "thumbs/index.json";

pub(crate) fn set(o: &Object, k: &str, v: &JsValue) {
    let _ = Reflect::set(o, &k.into(), v);
}

pub(crate) fn get(o: &JsValue, k: &str) -> JsValue {
    Reflect::get(o, &k.into()).unwrap_or(JsValue::UNDEFINED)
}

pub(crate) fn now() -> f64 {
    let perf = get(&js_sys::global(), "performance");
    let f = get(&perf, "now");
    f.dyn_ref::<js_sys::Function>().and_then(|f| f.call0(&perf).ok()).and_then(|v| v.as_f64()).unwrap_or(0.0)
}

// ---------------------------------------------------------------------------------------------
// worker side

thread_local! {
    static CORE: RefCell<WorkerCore> = RefCell::new(WorkerCore::default());
}

/// Entry point of a render worker (called by `worker.js` after instantiating the module).
/// `store` is the backend kind the page uses ("opfs", "idb" or "memory").
#[wasm_bindgen]
pub fn worker_main(store: String) {
    eframe::WebLogger::init(log::LevelFilter::Info).ok();
    let scope: web_sys::DedicatedWorkerGlobalScope = js_sys::global().unchecked_into();
    // The main thread sends jobs only after "ready", i.e. once storage is open, and one at a time.
    wasm_bindgen_futures::spawn_local(async move {
        let backend = match store.as_str() {
            "memory" => None,
            _ => Backend::open(store == "idb").await.map_err(|e| log::warn!("render worker: no storage ({e})")).ok(),
        };
        let s = scope.clone();
        let onmessage = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |e: web_sys::MessageEvent| {
            // (`web.crashWorker`, to see the page start a worker again: the exception ends the worker
            // and reaches the page as the Worker's `error` event)
            if get(&e.data(), "crash").is_truthy() {
                wasm_bindgen::throw_str("render worker: crash requested");
            }
            let (s, b) = (s.clone(), backend.clone());
            wasm_bindgen_futures::spawn_local(async move { handle(&s, b, e.data()).await });
        });
        scope.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
        onmessage.forget();
        let ready = Object::new();
        set(&ready, "ready", &true.into());
        let _ = scope.post_message(&ready);
    });
}

/// The bytes `job` needs from storage when the worker has not decoded its source yet: the
/// original, else (a synced photo whose original is on the server) its downloaded previews.
async fn read_original(backend: &Option<Backend>, job: &WireJob) -> Result<Option<Vec<u8>>, String> {
    let Some(hash) = CORE.with(|c| c.borrow_mut().needs_original(job)) else { return Ok(None) };
    let Some(b) = backend else { return Ok(None) };
    if let Some(bytes) = b.read(&crate::store::storage_key(&hash)).await? {
        return Ok(Some(bytes));
    }
    for key in crate::wire::proxy_keys(&hash, job.max_edge) {
        if let Some(bytes) = b.read(&key).await? {
            return Ok(Some(bytes));
        }
    }
    Ok(None)
}

/// Run an export message (`{id, export}`: an [`ExportWire`] as JSON): render the photo at its output
/// size, encode it with the engine's encoder, and post `{id, ok, bytes, w, h, ms, heap}` with the
/// file's bytes transferred. `heap`: the module's memory, which only grows, so the page can see
/// what an export costs a worker.
async fn handle_export(scope: &web_sys::DedicatedWorkerGlobalScope, backend: Option<Backend>, id: JsValue, text: String) {
    let t0 = now();
    let out = Object::new();
    set(&out, "id", &id);
    let result: Result<lightcraft_engine::export::Exported, String> = async {
        let wire: ExportWire = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        let original = read_original(&backend, &wire.job).await?;
        CORE.with(|c| c.borrow_mut().export(&wire, original.as_deref()))
    }
    .await;
    let mut transfer = Array::new();
    match result {
        Ok(file) => {
            let bytes = Uint8Array::from(file.bytes.as_slice());
            set(&out, "ok", &true.into());
            set(&out, "w", &(file.width as u32).into());
            set(&out, "h", &(file.height as u32).into());
            set(&out, "bytes", &bytes);
            transfer = Array::of1(&bytes.buffer());
        }
        Err(e) => {
            set(&out, "ok", &false.into());
            set(&out, "err", &e.into());
        }
    }
    let heap = wasm_bindgen::memory().unchecked_into::<js_sys::WebAssembly::Memory>().buffer().unchecked_into::<js_sys::ArrayBuffer>();
    set(&out, "heap", &heap.byte_length().into());
    set(&out, "ms", &(now() - t0).into());
    if let Err(e) = scope.post_message_with_transfer(&out, &transfer) {
        log::error!("export worker: post: {e:?}");
    }
}

/// Run one job message and post the result.
async fn handle(scope: &web_sys::DedicatedWorkerGlobalScope, backend: Option<Backend>, msg: JsValue) {
    if let Some(text) = get(&msg, "export").as_string() {
        return handle_export(scope, backend, get(&msg, "id"), text).await;
    }
    let id = get(&msg, "id");
    let t0 = now();
    let out = Object::new();
    set(&out, "id", &id);
    let job: Result<WireJob, String> =
        get(&msg, "job").as_string().ok_or("no job".to_string()).and_then(|j| serde_json::from_str(&j).map_err(|e| e.to_string()));
    let mut transfer = Array::new();
    let result: Result<(Rendered, u32), String> = async {
        let job = job?;
        let mut cache_miss = false;
        // cached thumbnail?
        if job.thumb_cached
            && let (Some(b), Some(key)) = (&backend, job.thumb_key())
        {
            match b.read(&key).await.ok().flatten().and_then(|bytes| lightcraft_preview::decode_jpeg(&bytes)) {
                Some(image) => {
                    let histogram = Histogram::of_srgb8(&image);
                    return Ok((Rendered { image, histogram, deep: None }, 0));
                }
                None => cache_miss = true,
            }
        }
        set(&out, "cacheMiss", &cache_miss.into());
        let original = read_original(&backend, &job).await?;
        let rendered = CORE.with(|c| c.borrow_mut().render(&job, original.as_deref()))?;
        let mut stored = 0;
        if let (Some(b), Some(key)) = (&backend, job.thumb_key())
            && let Some(jpeg) = lightcraft_preview::encode_jpeg(&rendered.image)
        {
            match b.write(&key, &jpeg).await {
                Ok(()) => stored = jpeg.len() as u32,
                Err(e) => log::warn!("thumbnail cache write: {e}"),
            }
        }
        Ok((rendered, stored))
    }
    .await;
    match result {
        Ok((r, stored)) => {
            let rgba = Uint8Array::from(r.image.as_bytes().as_ref());
            let h = &r.histogram;
            let hist = Uint32Array::from(h.to_words().as_slice());
            set(&out, "ok", &true.into());
            set(&out, "w", &(r.image.width as u32).into());
            set(&out, "h", &(r.image.height as u32).into());
            set(&out, "rgba", &rgba);
            set(&out, "hist", &hist);
            set(&out, "stored", &stored.into());
            transfer = Array::of2(&rgba.buffer(), &hist.buffer());
        }
        Err(e) => {
            set(&out, "ok", &false.into());
            set(&out, "err", &e.into());
        }
    }
    set(&out, "ms", &(now() - t0).into());
    if let Err(e) = scope.post_message_with_transfer(&out, &transfer) {
        log::error!("render worker: post: {e:?}");
    }
}

// ---------------------------------------------------------------------------------------------
// main side

struct Busy {
    disk_key: Option<String>,
    namespace: u64,
    request_id: u64,
    cache_generation: u64,
    id: u32,
    slot: Slot,
    photo: PhotoId,
    level: SourceLevel,
    key: u64,
    cache: Option<(Arc<PreviewCache>, Hash128)>,
    /// A slideshow slide: the worker renders the photo, it is composed here when it comes back.
    slide: Option<Arc<lightcraft_engine::slide::SlideCompose>>,
}

struct W {
    worker: web_sys::Worker,
    ready: bool,
    dead: bool,
    busy: Option<Busy>,
    /// Which worker has had this place: a new one gets the next number, and what an older one
    /// still says (a message in flight, an error after `terminate`) is ignored.
    generation: u32,
    /// Failures in a row, until a job comes back.
    restarts: u32,
    /// When a dead worker may be started again (ms on the page's clock).
    retry_at: f64,
}

struct Inner {
    /// The rendered-thumbnail cache the stored thumbnails belong to.
    preview_generation: CacheWatch,
    workers: Vec<W>,
    done: Vec<(Slot, RenderResult, f64)>,
    next_id: u32,
    /// (photo, level) → worker that last decoded it.
    affinity: HashMap<(PhotoId, SourceLevel), usize>,
    /// (photo, level) → the workers that decoded it, in the order they took it: a drag's drafts are
    /// rendered by them in turn ([`choose_worker`]).
    warm: HashMap<(PhotoId, SourceLevel), Vec<usize>>,
    index: ThumbIndex,
    /// The session's thumbnail cache budget in bytes. Kept here because the prune happens on a
    /// worker message, where the session is out of reach; the app refreshes it every frame.
    budget: u64,
    backend: Option<Backend>,
    ctx: egui::Context,
    /// What a worker is started with (the compiled module and the storage kind), kept to start
    /// another in the place of one that died.
    module: JsValue,
    store: String,
    /// Workers started again (diagnostics).
    restarted: u64,
    /// Jobs finished by workers / inline (for diagnostics).
    remote_done: u64,
    inline_done: u64,
    inline_this_frame: bool,
}

impl Inner {
    fn observe_cache(&mut self, cache: &Arc<PreviewCache>, generation: u64) {
        let gone = self.preview_generation.observe(cache, generation, &mut self.index);
        if !gone.is_empty()
            && let Some(backend) = self.backend.clone()
        {
            wasm_bindgen_futures::spawn_local(async move {
                for key in gone {
                    let _ = backend.remove(&thumb_storage_key(&key)).await;
                }
            });
        }
    }
}

/// The main thread's handle on the render workers.
#[derive(Clone)]
pub struct Workers(Rc<RefCell<Inner>>);

impl Workers {
    /// Start `n` workers (none: every job runs inline). `store` is the backend kind for workers.
    /// `index` lists the stored thumbnails of `cache`, the session's rendered-thumbnail cache
    /// now: watching it from the start, a clear before the first render request still counts.
    pub fn start(
        n: usize,
        store: &str,
        backend: Option<Backend>,
        index: ThumbIndex,
        cache: &Arc<PreviewCache>,
        budget: u64,
        ctx: egui::Context,
    ) -> Workers {
        let module = get(&js_sys::global(), "lightcraftModule");
        let w = Workers(Rc::new(RefCell::new(Inner {
            preview_generation: CacheWatch::new(cache),
            workers: Vec::new(),
            done: Vec::new(),
            next_id: 0,
            affinity: HashMap::new(),
            warm: HashMap::new(),
            index,
            budget,
            backend,
            ctx,
            module: module.clone(),
            store: store.to_string(),
            restarted: 0,
            remote_done: 0,
            inline_done: 0,
            inline_this_frame: false,
        })));
        if module.is_undefined() {
            log::warn!("render workers: no compiled module on the page (window.lightcraftModule); rendering on the main thread");
            return w;
        }
        let mut slots = WorkerSlots::default();
        for attempt in 0..n {
            // The callbacks name their worker by its place in `workers`; a failed attempt takes none.
            let i = slots.peek();
            let worker = match w.launch(i, 0, attempt, &module, store) {
                Ok(x) => x,
                Err(e) => {
                    log::warn!("render worker {attempt}: {e}");
                    continue;
                }
            };
            w.0.borrow_mut().workers.push(W { worker, ready: false, dead: false, busy: None, generation: 0, restarts: 0, retry_at: 0.0 });
            slots.commit();
        }
        w
    }

    /// A new render worker for place `i` of the list (`generation`: how many have had it), named
    /// `lightcraft-render-{name}`. Its messages and errors come back to [`Self::on_message`] and
    /// [`Self::on_error`] with the place and the generation.
    fn launch(&self, i: usize, generation: u32, name: usize, module: &JsValue, store: &str) -> Result<web_sys::Worker, String> {
        let opts = web_sys::WorkerOptions::new();
        opts.set_type(web_sys::WorkerType::Module);
        opts.set_name(&format!("lightcraft-render-{name}"));
        let worker = web_sys::Worker::new_with_options("./worker.js", &opts).map_err(|e| format!("{e:?}"))?;
        let me = self.clone();
        let onmessage = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |e: web_sys::MessageEvent| me.on_message(i, generation, e.data()));
        worker.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
        onmessage.forget();
        let me = self.clone();
        let onerror = Closure::<dyn FnMut(JsValue)>::new(move |e: JsValue| me.on_error(i, generation, e));
        worker.set_onerror(Some(onerror.as_ref().unchecked_ref()));
        onerror.forget();
        let init = Object::new();
        set(&init, "module", module);
        set(&init, "store", &store.into());
        if let Err(e) = worker.post_message(&init) {
            // It never joins the list, so it must not answer to the index of the next worker.
            worker.set_onmessage(None);
            worker.set_onerror(None);
            worker.terminate();
            return Err(format!("{e:?}"));
        }
        Ok(worker)
    }

    fn on_error(&self, i: usize, generation: u32, e: JsValue) {
        let msg = get(&e, "message").as_string().unwrap_or_else(|| format!("{e:?}"));
        let mut g = self.0.borrow_mut();
        // (an error of a worker that has been replaced, or that is retired already, is not news)
        if g.workers.get(i).is_none_or(|w| w.generation != generation || w.dead) {
            return;
        }
        log::error!("render worker {i} failed: {msg}");
        for holders in g.warm.values_mut() {
            holders.retain(|&j| j != i);
        }
        g.affinity.retain(|_, worker| *worker != i);
        let now = now();
        if let Some(w) = g.workers.get_mut(i) {
            w.dead = true;
            w.ready = false;
            w.worker.terminate();
            w.retry_at = now + restart_delay_ms(w.restarts);
            w.restarts = w.restarts.saturating_add(1);
            if let Some(b) = w.busy.take() {
                let r = RenderResult {
                    request_id: b.request_id,
                    source_key: None,
                    photo: b.photo,
                    level: b.level,
                    key: b.key,
                    rendered: Err(msg),
                    gpu_fallback: None,
                    loaded: None,
                    quick: None,
                    display: None,
                    hdr: None,
                };
                g.done.push((b.slot, r, 0.0));
            }
        }
        g.ctx.request_repaint();
    }

    /// Once a frame: start a worker again in the place of one that died. Without this one error
    /// (an out-of-memory trap, say) leaves the page with fewer workers for good, and with none every
    /// render runs on the page's own thread, which freezes it for the length of each one.
    fn supervise(&self) {
        let now = now();
        let starts: Vec<(usize, u32)> = {
            let g = self.0.borrow();
            g.workers
                .iter()
                .enumerate()
                .filter(|(_, w)| w.dead && w.restarts < MAX_RESTARTS && now >= w.retry_at)
                .map(|(i, w)| (i, w.generation.wrapping_add(1)))
                .collect()
        };
        for (i, generation) in starts {
            let (module, store) = {
                let g = self.0.borrow();
                (g.module.clone(), g.store.clone())
            };
            let started = self.launch(i, generation, i, &module, &store);
            let mut g = self.0.borrow_mut();
            let g = &mut *g;
            let Some(w) = g.workers.get_mut(i) else { continue };
            match started {
                Ok(worker) => {
                    log::info!("render worker {i}: started again");
                    w.worker = worker;
                    w.generation = generation;
                    w.dead = false;
                    w.busy = None;
                    g.restarted += 1;
                }
                Err(e) => {
                    log::warn!("render worker {i}: can't start again: {e}");
                    w.retry_at = now + restart_delay_ms(w.restarts);
                    w.restarts = w.restarts.saturating_add(1);
                }
            }
        }
    }

    /// How many times a worker was started again (diagnostics: `web.stats`).
    pub fn restarted(&self) -> u64 {
        self.0.borrow().restarted
    }

    /// Make worker `i` throw (`web.crashWorker`, for trying the restart): the page sees the error
    /// and starts another. `false` when there is no such worker alive.
    pub fn crash(&self, i: usize) -> bool {
        let g = self.0.borrow();
        let Some(w) = g.workers.get(i).filter(|w| !w.dead) else { return false };
        let msg = Object::new();
        set(&msg, "crash", &true.into());
        w.worker.post_message(&msg).is_ok()
    }

    fn on_message(&self, i: usize, generation: u32, data: JsValue) {
        let mut g = self.0.borrow_mut();
        let g = &mut *g;
        g.ctx.request_repaint();
        let Some(w) = g.workers.get_mut(i) else { return };
        // (what a worker that has been replaced still had on its way)
        if w.generation != generation || w.dead {
            return;
        }
        if get(&data, "ready").is_truthy() {
            w.ready = true;
            return;
        }
        let id = get(&data, "id").as_f64().unwrap_or(-1.0) as u32;
        let Some(b) = w.busy.take_if(|b| b.id == id) else { return };
        // a worker that answers is well: its failures in a row start again from none
        w.restarts = 0;
        let ms = get(&data, "ms").as_f64().unwrap_or(0.0);
        let rendered = if get(&data, "ok").is_truthy() {
            let (wd, ht) = (get(&data, "w").as_f64().unwrap_or(0.0) as usize, get(&data, "h").as_f64().unwrap_or(0.0) as usize);
            let rgba = Uint8Array::new(&get(&data, "rgba")).to_vec();
            let hist = Uint32Array::new(&get(&data, "hist")).to_vec();
            match (Rgba8::from_bytes(wd, ht, &rgba), Histogram::from_words(&hist)) {
                (Some(image), Some(histogram)) => Ok(Rendered { image, histogram, deep: None }),
                _ => Err("render worker: bad image".to_string()),
            }
        } else {
            Err(get(&data, "err").as_string().unwrap_or_else(|| "render worker failed".into()))
        };
        if let Ok(r) = &rendered
            && let Some((cache, key)) = &b.cache
            && cache.generation() == b.cache_generation
            && b.namespace == g.index.namespace()
        {
            cache.put_at(b.cache_generation, *key, Arc::new(r.image.clone()));
            let hex = b.disk_key.clone().unwrap_or_else(|| key.to_string());
            let stored = get(&data, "stored").as_f64().unwrap_or(0.0) as u64;
            if get(&data, "cacheMiss").is_truthy() {
                g.index.remove(&hex);
            }
            if stored > 0 {
                g.index.insert(&hex, stored);
                // The session's own budget (`library.preferences ▸ cacheMb`), not the native
                // default: the browser accepted the setting, so it has to be the one enforced
                // (#245).
                let gone = g.index.prune(g.budget);
                if !gone.is_empty()
                    && let Some(be) = g.backend.clone()
                {
                    log::info!("thumbnail cache: pruning {} files", gone.len());
                    wasm_bindgen_futures::spawn_local(async move {
                        for k in gone {
                            let _ = be.remove(&thumb_storage_key(&k)).await;
                        }
                    });
                }
            }
        }
        g.affinity.insert((b.photo, b.level), i);
        // A cleared worker may finish writing after the initial deletion. Its old namespace
        // is never reused, so removing this file cannot delete a replacement's cache entry.
        if let Some((cache, _)) = &b.cache
            && (cache.generation() != b.cache_generation || b.namespace != g.index.namespace())
            && let (Some(backend), Some(key)) = (g.backend.clone(), b.disk_key.clone())
        {
            wasm_bindgen_futures::spawn_local(async move {
                let _ = backend.remove(&thumb_storage_key(&key)).await;
            });
        }
        g.remote_done += 1;
        let rendered = match (rendered, &b.slide) {
            (Ok(mut r), Some(slide)) => {
                r.image = slide.compose(&r.image);
                Ok(r)
            }
            (r, _) => r,
        };
        g.done.push((
            b.slot,
            RenderResult {
                request_id: b.request_id,
                source_key: None,
                photo: b.photo,
                level: b.level,
                key: b.key,
                rendered,
                gpu_fallback: None,
                loaded: None,
                quick: None,
                display: None,
                hdr: None,
            },
            ms,
        ));
    }

    /// The thumbnail index as JSON if it changed since the last call (the host saves it).
    pub fn take_index_if_dirty(&self) -> Option<Vec<u8>> {
        let mut g = self.0.borrow_mut();
        if !g.index.dirty {
            return None;
        }
        g.index.dirty = false;
        Some(g.index.to_json())
    }

    /// (workers alive, workers ready, jobs done remotely, jobs done inline).
    pub fn stats(&self) -> (usize, usize, u64, u64) {
        let g = self.0.borrow();
        let alive = g.workers.iter().filter(|w| !w.dead).count();
        let ready = g.workers.iter().filter(|w| w.ready && !w.dead).count();
        (alive, ready, g.remote_done, g.inline_done)
    }

    /// The budget the thumbnail index is pruned to. The app refreshes it from the session every
    /// frame, so `library.preferences ▸ cacheMb` applies to the next stored thumbnail.
    pub fn set_budget(&self, bytes: u64) {
        self.0.borrow_mut().budget = bytes;
    }
}

impl RenderOffload for Workers {
    fn try_start(&mut self, slot: Slot, job: RenderJob) -> Option<RenderJob> {
        let mut g = self.0.borrow_mut();
        let g = &mut *g;
        if let Some((cache, _)) = job.cache.as_ref().or(job.view_cache.as_ref()) {
            g.observe_cache(cache, job.cache_generation);
        }
        // rendered thumbnail in memory: no worker needed
        if let Some((cache, key)) = &job.cache
            && let Some(img) = cache.get_at(job.cache_generation, *key)
        {
            let image = Arc::unwrap_or_clone(img);
            let histogram = Histogram::of_srgb8(&image);
            let r = RenderResult {
                request_id: job.request_id,
                source_key: job.source_key,
                photo: job.photo,
                level: job.level,
                key: job.key,
                rendered: Ok(Rendered { image, histogram, deep: None }),
                gpu_fallback: None,
                loaded: None,
                quick: None,
                display: None,
                hdr: None,
            };
            g.done.push((slot, r, 0.0));
            return None;
        }
        if g.workers.iter().all(|w| w.dead) {
            // no workers: render here, one job per frame (as without workers)
            if g.inline_this_frame {
                return Some(job);
            }
            g.inline_this_frame = true;
            let t0 = now();
            let r = job.run();
            g.inline_done += 1;
            g.done.push((slot, r, now() - t0));
            return None;
        }
        let free: Vec<bool> = g.workers.iter().map(|w| w.ready && !w.dead && w.busy.is_none()).collect();
        let alive: Vec<bool> = g.workers.iter().map(|w| !w.dead).collect();
        let key = (job.photo, job.level);
        // a decoded preview-level source, or the loupe's stage cache, is worth waiting for; a
        // thumbnail isn't. A drag's drafts of a view don't wait: a render takes a few tens of ms
        // and one worker can't keep up with the slider, so the idle ones take turns (each holds its
        // own decoded copy and stage cache: a few tens of MB; an original, `Full`, is kept to one;
        // a small draft is cut from the thumbnail-level source)
        let strict = job.level == SourceLevel::Preview || job.stages.is_some();
        let pipelined = job.stages.is_some() && job.level != SourceLevel::Full && job.request.quality == Quality::Draft;
        let warm = g.warm.get(&key).map_or(&[][..], Vec::as_slice);
        let pick = choose_worker(&free, &alive, g.affinity.get(&key).copied(), warm, strict, pipelined, MAX_PIPELINE);
        let Some(i) = pick else { return Some(job) };
        // decided here, so the next job for this photo follows even before this one finishes
        g.affinity.insert(key, i);
        if g.warm.len() > 256 {
            g.warm.clear();
        }
        let holders = g.warm.entry(key).or_default();
        if !holders.contains(&i) {
            holders.push(i);
        }
        let disk_key = job.cache.as_ref().map(|(_, key)| g.index.cache_key(*key));
        let thumb_cached = disk_key.as_ref().is_some_and(|key| g.index.touch(key));
        let mut wire = WireJob::from_job(&job, thumb_cached, &format!("{slot:?}"));
        // The namespace also changes when a new library's cache starts again at generation zero.
        wire.cache_generation = g.index.namespace();
        wire.thumb = disk_key.clone();
        let Ok(text) = serde_json::to_string(&wire) else { return Some(job) };
        g.next_id = g.next_id.wrapping_add(1);
        let id = g.next_id;
        let msg = Object::new();
        set(&msg, "id", &id.into());
        set(&msg, "job", &text.into());
        let namespace = g.index.namespace();
        let w = &mut g.workers[i];
        if let Err(e) = w.worker.post_message(&msg) {
            log::warn!("render worker {i}: {e:?}");
            return Some(job);
        }
        w.busy = Some(Busy {
            disk_key,
            namespace,
            request_id: job.request_id,
            cache_generation: job.cache_generation,
            id,
            slot,
            photo: job.photo,
            level: job.level,
            key: job.key,
            cache: job.cache,
            slide: job.slide,
        });
        None
    }

    fn finished(&mut self) -> Vec<(Slot, RenderResult, f64)> {
        self.supervise();
        let mut g = self.0.borrow_mut();
        // Persist invalidation even when the cleared library has no visible render requests.
        if let Some(cache) = g.preview_generation.cache() {
            g.observe_cache(&cache, cache.generation());
        }
        g.inline_this_frame = false;
        std::mem::take(&mut g.done)
    }
}
