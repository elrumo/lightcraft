//! Natural-language search: describe a photo ("a dog on a beach at sunset") and the library
//! shows the best matches. A photo and a sentence are both turned into a vector by an image-text
//! model ([`Embedder`]; SigLIP 2 with the `vision` feature, the desktop app enables it), and the
//! photos whose vectors are nearest the sentence's come first.
//!
//! **Nothing requires the model.** It is not part of LightCraft: the user downloads it when they
//! first use search and agree to the licence (`vision.model.download`), or puts the files in the
//! model folder. Without it, search reports that it isn't installed and everything else works.
//!
//! Each photo's vector is kept in the library's `search/` folder, keyed by the photo's content
//! hash ([`Key`]) and computed from a neutral, unedited rendering, so edits never invalidate it
//! and the same file embeds the same on any device. It is derived data: deleting the folder
//! only means indexing again. It never enters the catalog (which syncs whole to every device).
//!
//! Everything slow runs off the session's thread: loading the model (seconds), indexing, and
//! each search. With [`Vision::background`] (the desktop app) commands return at once and
//! [`Session::vision_poll`] applies the results; without it (CLI, MCP, tests) they wait.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use lightcraft_catalog::PhotoId;
use lightcraft_develop::DevelopSettings;
use lightcraft_raster::Rgba8;
use lightcraft_vision::{Embedder, EmbeddingIndex, Hit, Key};
use serde_json::{Value, json};

use crate::media::{RenderJob, content_key};
use crate::{Session, guard, memory};

/// Long edge of the rendering a photo is embedded from.
const THUMB_EDGE: usize = 512;
/// Photos embedded in one model pass.
const BATCH: usize = 8;
/// Unload the model (and the in-memory index) after this long without use.
pub const IDLE_UNLOAD: Duration = Duration::from_secs(10 * 60);
pub const DEFAULT_LIMIT: usize = 200;
pub const MAX_LIMIT: usize = 5000;
/// Longest query used, in characters.
pub const MAX_QUERY: usize = 512;
/// How errors about a missing model start (the UI offers the download on it).
pub const NOT_INSTALLED: &str = "The search model is not installed";
/// The model's licence (not LightCraft's): shown before downloading.
pub const LICENSE_NAME: &str = "Apache License 2.0 (Google, SigLIP 2)";
pub const LICENSE_URL: &str = "https://huggingface.co/google/siglip2-base-patch16-224";
/// Download size of the model.
pub const MODEL_BYTES: u64 = 1_500_800_904 + 34_363_039;

/// Where an index lives and what it holds.
#[derive(Clone, Debug, PartialEq, Eq)]
struct IndexSpec {
    /// `None`: in memory only (a library that isn't a folder).
    path: Option<PathBuf>,
    model: String,
    dim: usize,
}

/// How a worker thread gets the model.
#[derive(Clone)]
struct Provider {
    /// A model the host supplied (tests, other hosts); otherwise SigLIP 2 is loaded from `dir`.
    injected: Option<Arc<dyn Embedder>>,
    dir: Option<PathBuf>,
}

/// What worker threads and the session share.
#[derive(Default)]
struct Shared {
    model: Mutex<Option<Arc<dyn Embedder>>>,
    index: Mutex<Option<(Option<PathBuf>, EmbeddingIndex)>>,
    /// The index's length, readable without its lock.
    indexed: AtomicUsize,
    /// The index has been opened (so `indexed` means something).
    opened: AtomicBool,
    loaded: AtomicBool,
    loading: AtomicBool,
    used: Mutex<Option<Instant>>,
    /// Indexing and searches running now.
    busy: AtomicUsize,
}

impl Shared {
    fn touch(&self) {
        *self.used.lock().unwrap_or_else(PoisonError::into_inner) = Some(Instant::now());
    }
}

/// Decrements a counter when dropped (also when unwinding).
struct Busy(Arc<Shared>);

impl Busy {
    fn new(s: &Arc<Shared>) -> Busy {
        s.busy.fetch_add(1, Ordering::SeqCst);
        Busy(s.clone())
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        let _ = self.0.busy.try_update(Ordering::SeqCst, Ordering::SeqCst, |n| Some(n.saturating_sub(1)));
    }
}

/// An indexing run.
#[derive(Debug, Default)]
pub struct IndexJob {
    pub total: usize,
    pub done: AtomicUsize,
    pub failed: AtomicUsize,
    pub cancel: AtomicBool,
    pub finished: AtomicBool,
    error: Mutex<Option<String>>,
}

impl IndexJob {
    pub fn error(&self) -> Option<String> {
        self.error.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn fail(&self, e: String) {
        self.error.lock().unwrap_or_else(PoisonError::into_inner).get_or_insert(e);
    }

    pub fn json(&self) -> Value {
        json!({
            "total": self.total,
            "done": self.done.load(Ordering::Relaxed),
            "failed": self.failed.load(Ordering::Relaxed),
            "running": !self.finished.load(Ordering::Relaxed),
            "cancelled": self.cancel.load(Ordering::Relaxed),
            "error": self.error(),
        })
    }
}

/// A finished search, on its way from a worker thread to the session.
struct Searched {
    seq: u64,
    query: String,
    hits: Result<Vec<Hit>, String>,
}

/// What [`Session::vision_poll`] did.
#[derive(Debug, Default)]
pub struct VisionPolled {
    /// The view's filter changed (a search finished).
    pub changed: bool,
    /// Errors and notices from background work, for the user.
    pub messages: Vec<String>,
}

/// The search model, its index and the work in flight (`Session::vision`).
pub struct Vision {
    /// Where the model's files live (set by the app; `None`: no model folder).
    pub dir: Option<PathBuf>,
    /// The user's list of download mirrors (one base URL per line), if any.
    pub mirrors_file: Option<PathBuf>,
    /// Run work in the background and apply results in [`Session::vision_poll`] (the desktop
    /// app); otherwise commands wait for their result (CLI, MCP, tests).
    pub background: bool,
    injected: Option<Arc<dyn Embedder>>,
    shared: Arc<Shared>,
    job: Option<Arc<IndexJob>>,
    job_reported: bool,
    search_seq: u64,
    searching: Arc<AtomicUsize>,
    results: (Sender<Searched>, Receiver<Searched>),
    opening: Arc<AtomicBool>,
    messages: Vec<String>,
    #[cfg(feature = "vision")]
    download: crate::download::Downloader,
}

impl Default for Vision {
    fn default() -> Self {
        Vision {
            dir: None,
            mirrors_file: None,
            background: false,
            injected: None,
            shared: Arc::new(Shared::default()),
            job: None,
            job_reported: true,
            search_seq: 0,
            searching: Arc::new(AtomicUsize::new(0)),
            results: mpsc::channel(),
            opening: Arc::new(AtomicBool::new(false)),
            messages: Vec::new(),
            #[cfg(feature = "vision")]
            download: crate::download::Downloader::default(),
        }
    }
}

impl Vision {
    /// Whether this build can search by description at all (or a host supplied a model).
    pub fn available(&self) -> bool {
        cfg!(feature = "vision") || self.injected.is_some()
    }

    /// Use `model` instead of loading SigLIP 2 from the model folder (tests, other hosts).
    pub fn set_embedder(&mut self, model: Arc<dyn Embedder>) {
        self.injected = Some(model);
    }

    /// Whether the model's files are in place.
    pub fn installed(&self) -> bool {
        if self.injected.is_some() {
            return true;
        }
        #[cfg(feature = "vision")]
        if let Some(dir) = &self.dir {
            return lightcraft_vision::siglip::is_model_dir(dir);
        }
        false
    }

    /// Whether the model is in memory (it is unloaded after a while without use).
    pub fn loaded(&self) -> bool {
        self.injected.is_some() || self.shared.loaded.load(Ordering::SeqCst)
    }

    /// Photos in the index (0 until it has been opened).
    pub fn indexed(&self) -> usize {
        self.shared.indexed.load(Ordering::Relaxed)
    }

    /// Whether indexing or a search is running.
    pub fn busy(&self) -> bool {
        self.shared.busy.load(Ordering::SeqCst) > 0
    }

    /// Whether a search is running.
    pub fn searching(&self) -> bool {
        self.searching.load(Ordering::SeqCst) > 0
    }

    /// The indexing run in progress or the last one.
    pub fn job(&self) -> Option<&Arc<IndexJob>> {
        self.job.as_ref()
    }

    /// Notices from background work, for the user (taken: each is returned once).
    pub fn take_messages(&mut self) -> Vec<String> {
        std::mem::take(&mut self.messages)
    }

    /// The model download's state.
    pub fn download_status(&self) -> Value {
        #[cfg(feature = "vision")]
        {
            let s = self.download.status();
            json!({"running": s.running, "done": s.done, "total": s.total, "file": s.file, "error": s.error, "finished": s.finished})
        }
        #[cfg(not(feature = "vision"))]
        json!({"running": false, "done": 0, "total": 0, "file": "", "error": null, "finished": false})
    }

    /// Where the model is downloaded from, in order (the user's mirrors first).
    pub fn mirrors(&self) -> Vec<String> {
        #[cfg(feature = "vision")]
        {
            let env = std::env::var(lightcraft_vision::models::MIRRORS_ENV).ok();
            lightcraft_vision::models::mirrors(env.as_deref(), self.mirrors_file.as_deref())
        }
        #[cfg(not(feature = "vision"))]
        Vec::new()
    }

    /// Start downloading the model on a background thread. `Ok(false)` when it is installed or
    /// downloading already.
    pub fn start_download(&self) -> Result<bool, String> {
        if !self.available() {
            return Err("natural-language search is not available in this build".into());
        }
        if self.installed() {
            return Ok(false);
        }
        #[cfg(feature = "vision")]
        {
            let dir = self.dir.clone().ok_or("no folder is set for the search model")?;
            let mirrors = self.mirrors();
            if mirrors.is_empty() {
                return Err(format!(
                    "no download location is configured for the search model in this build (set {}, or put the files in {})",
                    lightcraft_vision::models::MIRRORS_ENV,
                    dir.display()
                ));
            }
            self.download.start("search", lightcraft_vision::models::SIGLIP_FILES, mirrors, dir, lightcraft_vision::models::Options::default())
        }
        #[cfg(not(feature = "vision"))]
        Ok(false)
    }

    /// Stop a running download (its partial files stay, to resume). False when none runs.
    pub fn cancel_download(&self) -> bool {
        #[cfg(feature = "vision")]
        return self.download.cancel();
        #[cfg(not(feature = "vision"))]
        false
    }

    /// What the model is and how to reach it from a worker thread, or why it can't be.
    fn provider(&self) -> Result<(Provider, String, usize), String> {
        if let Some(m) = &self.injected {
            return Ok((Provider { injected: Some(m.clone()), dir: None }, m.model_id().to_string(), m.dim()));
        }
        #[cfg(feature = "vision")]
        {
            if !self.installed() {
                return Err(self.not_installed());
            }
            Ok((Provider { injected: None, dir: self.dir.clone() }, lightcraft_vision::siglip::MODEL_ID.to_string(), lightcraft_vision::siglip::DIM))
        }
        #[cfg(not(feature = "vision"))]
        Err("natural-language search is not available in this build".into())
    }

    /// The error for a missing model: what it is and how to get it.
    #[cfg(feature = "vision")]
    fn not_installed(&self) -> String {
        let d = self.download_status();
        if d.get("running").and_then(Value::as_bool) == Some(true) {
            let (done, total) = (d["done"].as_u64().unwrap_or(0), d["total"].as_u64().unwrap_or(0));
            return format!("{NOT_INSTALLED} yet: it is downloading ({} %).", done.saturating_mul(100).checked_div(total).unwrap_or(0));
        }
        let place = self.dir.as_ref().map(|d| format!(" or put model.safetensors and tokenizer.json in {}", d.display())).unwrap_or_default();
        format!(
            "{NOT_INSTALLED}. Download it (about {:.1} GB, {LICENSE_NAME}) when LightCraft offers it, with `vision.model.download {{\"acknowledged\": true}}`{place}.",
            MODEL_BYTES as f64 / 1e9
        )
    }
}

/// The model, loading it first if needed (seconds; the model lock is held meanwhile, so
/// concurrent requests wait for the one load).
fn acquire(shared: &Shared, provider: &Provider) -> Result<Arc<dyn Embedder>, String> {
    if let Some(m) = &provider.injected {
        shared.touch();
        return Ok(m.clone());
    }
    let mut slot = shared.model.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(m) = slot.as_ref() {
        shared.touch();
        return Ok(m.clone());
    }
    shared.loading.store(true, Ordering::SeqCst);
    let loaded = load_model(provider.dir.as_deref());
    shared.loading.store(false, Ordering::SeqCst);
    let m = loaded?;
    *slot = Some(m.clone());
    shared.loaded.store(true, Ordering::SeqCst);
    shared.touch();
    Ok(m)
}

fn load_model(dir: Option<&std::path::Path>) -> Result<Arc<dyn Embedder>, String> {
    #[cfg(feature = "vision")]
    {
        let dir = dir.ok_or("no folder is set for the search model")?;
        let model = guard::catch("loading the search model", || lightcraft_vision::siglip::SigLip::load(dir))?.map_err(|e| e.to_string())?;
        Ok(Arc::new(model))
    }
    #[cfg(not(feature = "vision"))]
    {
        let _ = dir;
        Err("natural-language search is not available in this build".into())
    }
}

/// Opens the index at `spec`. A file written for another model or layout, or damaged, is derived
/// data: it is deleted and started over.
fn open_index(spec: &IndexSpec) -> Result<EmbeddingIndex, String> {
    use lightcraft_vision::Error;
    let Some(path) = &spec.path else { return EmbeddingIndex::in_memory(&spec.model, spec.dim).map_err(|e| e.to_string()) };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("can't create {}: {e}", parent.display()))?;
    }
    match EmbeddingIndex::open(path, &spec.model, spec.dim) {
        Ok(ix) => Ok(ix),
        Err(Error::Mismatch { .. } | Error::Format(_)) => {
            log::info!("search index {} is for another model or damaged: starting over", path.display());
            let _ = std::fs::remove_file(path);
            EmbeddingIndex::open(path, &spec.model, spec.dim).map_err(|e| e.to_string())
        }
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Runs `f` on the index at `spec`, opening it first (or again, when it is another one).
fn with_index<R>(shared: &Shared, spec: &IndexSpec, f: impl FnOnce(&mut EmbeddingIndex) -> R) -> Result<R, String> {
    let mut slot = shared.index.lock().unwrap_or_else(PoisonError::into_inner);
    let stale = slot.as_ref().is_none_or(|(p, ix)| *p != spec.path || ix.model() != spec.model || ix.dim() != spec.dim);
    if stale {
        let ix = open_index(spec)?;
        *slot = Some((spec.path.clone(), ix));
    }
    let (_, ix) = slot.as_mut().ok_or("the search index is unavailable")?;
    let r = f(ix);
    shared.indexed.store(ix.len(), Ordering::Relaxed);
    shared.opened.store(true, Ordering::Relaxed);
    Ok(r)
}

/// Embeds `work`'s photos in batches and stores their vectors.
fn run_index(shared: &Arc<Shared>, provider: &Provider, spec: &IndexSpec, work: Vec<(Key, RenderJob)>, job: &IndexJob) {
    let _busy = Busy::new(shared);
    let result = guard::catch("indexing photos", || -> Result<(), String> {
        let model = acquire(shared, provider)?;
        let mut work = work.into_iter();
        while !job.cancel.load(Ordering::Relaxed) {
            let batch: Vec<(Key, RenderJob)> = work.by_ref().take(BATCH).collect();
            if batch.is_empty() {
                break;
            }
            let n = batch.len();
            let (mut keys, mut images) = (Vec::with_capacity(n), Vec::<Rgba8>::with_capacity(n));
            for (key, render) in batch {
                match memory::in_background(|| render.run().rendered) {
                    Ok(r) => {
                        keys.push(key);
                        images.push(r.image);
                    }
                    Err(e) => {
                        log::debug!("search index: can't render a photo: {e}");
                        job.failed.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
            if !images.is_empty() {
                let refs: Vec<&Rgba8> = images.iter().collect();
                match model.encode_images(&refs) {
                    Ok(vectors) => {
                        let stored = with_index(shared, spec, |ix| keys.iter().zip(&vectors).filter(|(k, v)| ix.insert(**k, v).is_err()).count())?;
                        job.failed.fetch_add(stored, Ordering::Relaxed);
                    }
                    Err(e) => {
                        // a model that fails once will fail again: stop, the rest counts as failed
                        job.fail(e.to_string());
                        return Ok(());
                    }
                }
            }
            job.done.fetch_add(n, Ordering::Relaxed);
        }
        Ok(())
    });
    match result {
        Ok(Ok(())) => {}
        Ok(Err(e)) | Err(e) => job.fail(e),
    }
    if job.error().is_some() {
        // the run ended early: whatever it didn't get to did not work
        let left = job.total.saturating_sub(job.done.load(Ordering::Relaxed));
        job.failed.fetch_add(left, Ordering::Relaxed);
        job.done.store(job.total, Ordering::Relaxed);
    }
    shared.touch();
    job.finished.store(true, Ordering::Relaxed);
}

/// The photos nearest `query`.
fn run_search(shared: &Arc<Shared>, provider: &Provider, spec: &IndexSpec, query: &str, limit: usize) -> Result<Vec<Hit>, String> {
    let _busy = Busy::new(shared);
    guard::catch("searching", || -> Result<Vec<Hit>, String> {
        let model = acquire(shared, provider)?;
        let v = model.encode_text(query).map_err(|e| e.to_string())?;
        shared.touch();
        with_index(shared, spec, |ix| ix.search(&v, limit).map_err(|e| e.to_string()))?
    })?
}

/// A neutral (unedited) rendering of `photo` from a smart or mini preview file, at most `edge` on
/// its long edge: what search embeds, so a photo gets the same vector wherever it is indexed (a
/// device renders its thumbnail source the same way, with default settings). The server uses it on
/// the previews it keeps.
#[cfg(not(target_arch = "wasm32"))]
pub fn neutral_from_preview(photo: &lightcraft_catalog::Photo, preview: &std::path::Path, edge: usize) -> Result<Rgba8, String> {
    use crate::media::{SourceLevel, SourceRef, source_info};
    let edge = edge.clamp(16, SourceLevel::Thumb.max_edge());
    let job = RenderJob {
        photo: photo.id,
        level: SourceLevel::Thumb,
        source: SourceRef::Smart { path: preview.to_path_buf() },
        origin: photo.source.clone(),
        info: source_info(photo),
        settings: Arc::new(DevelopSettings::default()),
        request: lightcraft_pipeline::RenderRequest { apply_crop: true, ..lightcraft_pipeline::RenderRequest::fit(edge, edge) },
        key: 0,
        cache: None,
        stages: None,
        view_cache: None,
    };
    guard::catch("rendering a photo for search", || job.run().rendered.map(|r| r.image))?
}

impl Session {
    /// The index file for `model`: in the library's `search/` folder, else none (memory only).
    fn vision_index_path(&self, model: &str) -> Option<PathBuf> {
        let lib = self.library.as_ref().filter(|l| l.on_disk)?;
        Some(lib.dir.join("search").join(format!("{model}.bin")))
    }

    /// Photos of the library by the key their vectors are filed under.
    fn vision_keys(&self) -> HashMap<Key, Vec<PhotoId>> {
        let mut map: HashMap<Key, Vec<PhotoId>> = HashMap::new();
        for p in self.catalog.photos().filter(|p| p.in_library()) {
            map.entry(Key::of(&content_key(p))).or_default().push(p.id);
        }
        map
    }

    /// How many photos the index can hold: the library's photos, distinct by content.
    pub fn vision_photo_count(&self) -> usize {
        self.vision_keys().len()
    }

    /// Status for the UI and agents (`vision.model.status`).
    pub fn vision_status(&self) -> Value {
        let v = &self.vision;
        let photos = self.catalog.photos().filter(|p| p.in_library()).count();
        json!({
            "available": v.available(),
            "installed": v.installed(),
            "dir": v.dir.as_ref().map(|d| d.display().to_string()),
            "loaded": v.loaded(),
            "loading": v.shared.loading.load(Ordering::SeqCst),
            "busy": v.busy(),
            "searching": v.searching(),
            "indexed": v.shared.opened.load(Ordering::Relaxed).then(|| v.indexed()),
            "photos": photos,
            "sizeBytes": MODEL_BYTES,
            "license": LICENSE_NAME,
            "licenseUrl": LICENSE_URL,
            "mirrors": v.mirrors().len(),
            "download": v.download_status(),
            "index": v.job.as_ref().map(|j| j.json()),
        })
    }

    /// Embed the library's photos that aren't in the index yet (`vision.index`): all of them, or
    /// `ids`. Runs on a thread (progress by [`IndexJob`], cancel by flag) or, with `wait`, here.
    pub fn vision_index(&mut self, ids: Option<Vec<PhotoId>>, wait: bool) -> std::result::Result<Value, String> {
        if self.vision.job.as_ref().is_some_and(|j| !j.finished.load(Ordering::Relaxed)) {
            return Err("photos are already being indexed".into());
        }
        let (provider, model, dim) = self.vision.provider()?;
        let spec = IndexSpec { path: self.vision_index_path(&model), model, dim };
        let have: HashSet<Key> = with_index(&self.vision.shared, &spec, |ix| ix.keys().copied().collect())?;
        let want: Vec<PhotoId> = match ids {
            Some(ids) => ids,
            None => self.catalog.photos().filter(|p| p.in_library()).map(|p| p.id).collect(),
        };
        let mut seen = have;
        let mut work: Vec<(Key, RenderJob)> = Vec::new();
        let mut failed = 0;
        for id in want {
            let Some(key) = self.catalog.photo(id).filter(|p| p.in_library()).map(|p| Key::of(&content_key(p))) else { continue };
            if !seen.insert(key) {
                continue;
            }
            // a neutral rendering: the photo as its file looks, whatever the user did to it; not
            // cached, so indexing doesn't fill the thumbnail cache with a second set
            match self.variant_job(id, &DevelopSettings::default(), THUMB_EDGE) {
                Some(mut job) => {
                    job.cache = None;
                    work.push((key, job));
                }
                None => failed += 1,
            }
        }
        let job = Arc::new(IndexJob { total: work.len() + failed, ..Default::default() });
        job.done.store(failed, Ordering::Relaxed);
        job.failed.store(failed, Ordering::Relaxed);
        self.vision.job = Some(job.clone());
        self.vision.job_reported = wait;
        let shared = self.vision.shared.clone();
        if wait || !self.vision.background || cfg!(target_arch = "wasm32") {
            run_index(&shared, &provider, &spec, work, &job);
            self.vision.job_reported = true;
            return Ok(job.json());
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let j = job.clone();
            let started = std::thread::Builder::new().name("lc-vision-index".into()).spawn(move || run_index(&shared, &provider, &spec, work, &j));
            if let Err(e) = started {
                job.finished.store(true, Ordering::Relaxed);
                return Err(format!("could not start indexing: {e}"));
            }
        }
        Ok(job.json())
    }

    /// Search by description (`library.search`): the best `limit` photos, best first, as the
    /// view's filter. With `wait` (or without [`Vision::background`]) the result is applied here;
    /// otherwise a thread computes it and [`Session::vision_poll`] applies it.
    #[cfg_attr(target_arch = "wasm32", allow(unused_variables))]
    pub fn vision_search(&mut self, query: &str, limit: usize, wait: bool) -> std::result::Result<Value, String> {
        let query: String = query.trim().chars().take(MAX_QUERY).collect();
        if query.is_empty() {
            return Err("describe the photo you are looking for".into());
        }
        let (provider, model, dim) = self.vision.provider()?;
        let spec = IndexSpec { path: self.vision_index_path(&model), model, dim };
        let known = with_index(&self.vision.shared, &spec, |ix| ix.len())?;
        if known == 0 {
            return Err("no photos are indexed for search yet: run `vision.index` first".into());
        }
        let limit = limit.clamp(1, MAX_LIMIT);
        self.vision.search_seq += 1;
        let seq = self.vision.search_seq;
        let shared = self.vision.shared.clone();
        if wait || !self.vision.background || cfg!(target_arch = "wasm32") {
            let hits = run_search(&shared, &provider, &spec, &query, limit)?;
            return Ok(self.apply_hits(&query, &hits));
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (tx, searching, q) = (self.vision.results.0.clone(), self.vision.searching.clone(), query.clone());
            searching.fetch_add(1, Ordering::SeqCst);
            let started = std::thread::Builder::new().name("lc-vision-search".into()).spawn(move || {
                let hits = run_search(&shared, &provider, &spec, &q, limit);
                let _ = tx.send(Searched { seq, query: q, hits });
                let _ = searching.try_update(Ordering::SeqCst, Ordering::SeqCst, |n| Some(n.saturating_sub(1)));
            });
            if let Err(e) = started {
                self.vision.searching.fetch_sub(1, Ordering::SeqCst);
                return Err(format!("could not start the search: {e}"));
            }
        }
        Ok(json!({"query": query, "status": "searching", "indexed": known}))
    }

    /// Sets the view to `hits` (best first) for `query`; photos by content, so virtual copies of
    /// a hit come with it. An empty result is an empty view, not "everything".
    fn apply_hits(&mut self, query: &str, hits: &[Hit]) -> Value {
        let by_key = self.vision_keys();
        let mut found: Vec<(PhotoId, f32)> = Vec::new();
        for h in hits {
            for id in by_key.get(&h.key).into_iter().flatten() {
                found.push((*id, h.score));
            }
        }
        self.filter.only = found.iter().map(|(id, _)| *id).collect();
        self.filter.semantic = Some(query.to_string());
        json!({
            "query": query,
            "photos": found.iter().map(|(id, score)| json!({"id": id.0, "score": (f64::from(*score) * 1e4).round() / 1e4})).collect::<Vec<_>>(),
            "indexed": self.vision.indexed(),
            "libraryPhotos": by_key.values().map(Vec::len).sum::<usize>(),
        })
    }

    /// Applies finished background work and unloads an idle model: call it often (the app does
    /// each frame). Cheap when nothing happened.
    pub fn vision_poll(&mut self) -> VisionPolled {
        let mut out = VisionPolled::default();
        while let Ok(done) = self.vision.results.1.try_recv() {
            // only the newest search counts: an older one is already out of date
            if done.seq != self.vision.search_seq {
                continue;
            }
            match done.hits {
                Ok(hits) => {
                    self.apply_hits(&done.query, &hits);
                    out.changed = true;
                }
                Err(e) => out.messages.push(e),
            }
        }
        if let Some(job) = self.vision.job.clone()
            && job.finished.load(Ordering::Relaxed)
            && !self.vision.job_reported
        {
            self.vision.job_reported = true;
            let (done, failed) = (job.done.load(Ordering::Relaxed), job.failed.load(Ordering::Relaxed));
            out.messages.push(match job.error() {
                Some(e) => format!("Indexing photos for search stopped: {e}"),
                None if job.cancel.load(Ordering::Relaxed) => format!("Indexing photos for search was cancelled after {done} photos."),
                None if failed > 0 => format!("Indexed {} photos for search; {failed} could not be read.", done.saturating_sub(failed)),
                None => format!("Indexed {done} photos for search."),
            });
        }
        out.messages.append(&mut self.vision.messages);
        self.vision_open_in_background();
        self.vision_unload_if_idle();
        out
    }

    /// Opens the library's index on a thread when it hasn't been, so its size is known without
    /// the first search or indexing run having to wait for the file.
    #[cfg(not(target_arch = "wasm32"))]
    fn vision_open_in_background(&mut self) {
        let sh = &self.vision.shared;
        if sh.opened.load(Ordering::Relaxed) || self.vision.opening.load(Ordering::SeqCst) || !self.vision.background {
            return;
        }
        let Ok((_, model, dim)) = self.vision.provider() else { return };
        let spec = IndexSpec { path: self.vision_index_path(&model), model, dim };
        if spec.path.as_ref().is_none_or(|p| !p.is_file()) {
            return;
        }
        let (shared, opening) = (sh.clone(), self.vision.opening.clone());
        opening.store(true, Ordering::SeqCst);
        let started = std::thread::Builder::new().name("lc-vision-open".into()).spawn(move || {
            let _ = with_index(&shared, &spec, |_| ());
            opening.store(false, Ordering::SeqCst);
        });
        if started.is_err() {
            self.vision.opening.store(false, Ordering::SeqCst);
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn vision_open_in_background(&mut self) {}

    /// Drops the model and the index from memory after [`IDLE_UNLOAD`] without use (the model is
    /// ~1.5 GB, the index ~150 MB per 100k photos). Both reload on the next request.
    fn vision_unload_if_idle(&mut self) {
        let sh = &self.vision.shared;
        if sh.busy.load(Ordering::SeqCst) > 0 || !sh.loaded.load(Ordering::SeqCst) {
            return;
        }
        let idle = sh.used.try_lock().ok().and_then(|u| u.map(|t| t.elapsed())).is_some_and(|d| d >= IDLE_UNLOAD);
        if !idle {
            return;
        }
        if let (Ok(mut m), Ok(mut ix)) = (sh.model.try_lock(), sh.index.try_lock()) {
            *m = None;
            *ix = None;
            sh.loaded.store(false, Ordering::SeqCst);
            sh.opened.store(false, Ordering::Relaxed);
            log::info!("the search model was unloaded after {} minutes without use", IDLE_UNLOAD.as_secs() / 60);
        }
    }
}

#[cfg(test)]
mod tests;
