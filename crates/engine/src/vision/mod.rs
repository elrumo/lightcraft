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
use lightcraft_vision::faces::index::FaceIndex;
use lightcraft_vision::{Embedder, EmbeddingIndex, FaceEngine, Hit, Key, TextIndex, TextReader};
use serde_json::{Value, json};

use crate::media::{RenderJob, SourceLevel, content_key};
use crate::{Session, guard, memory};

mod people;
mod server;
pub use people::{Cluster, FaceRef, clusters_of};
pub(crate) use server::Aux;

/// A number that is never used twice in this process (temporary file names).
pub(crate) fn next_id() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Long edge of the rendering a photo is embedded from.
const THUMB_EDGE: usize = 512;
/// Long edge of the rendering the text in a photo is read from (small print needs the pixels).
pub const TEXT_EDGE: usize = 1280;
/// Photos that fail to be read one after the other before the text reader is given up on.
const READ_FAILURES: usize = 8;
pub use lightcraft_vision::textindex::TEXT_SCORE_BASE;
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
/// The text-reading models' licence (not LightCraft's): shown before downloading.
pub const TEXT_LICENSE_NAME: &str = "Apache License 2.0 (Baidu, PaddleOCR PP-OCRv6)";
pub const TEXT_LICENSE_URL: &str = "https://huggingface.co/PaddlePaddle/PP-OCRv6_small_rec_onnx";
/// Download size of the text-reading models.
pub const TEXT_BYTES: u64 = 9_880_512 + 21_159_378 + 150_579;
/// How errors about missing text-reading models start (the UI offers the download on it).
pub const TEXT_NOT_INSTALLED: &str = "The text-reading models are not installed";
/// Folder of the text-reading models inside [`Vision::dir`].
pub const TEXT_DIR: &str = "ocr";
/// The face models' licences (not LightCraft's): shown before downloading.
pub const FACE_LICENSES: [(&str, &str); 2] = [
    ("MIT License (Shiqi Yu, YuNet)", "https://huggingface.co/opencv/face_detection_yunet"),
    ("Apache License 2.0 (BUPT, SFace)", "https://huggingface.co/opencv/face_recognition_sface"),
];
/// Download size of the face models.
pub const FACE_BYTES: u64 = 232_589 + 38_696_353;
/// How errors about missing face models start (the UI offers the download on it).
pub const FACES_NOT_INSTALLED: &str = "The face models are not installed";
/// Folder of the face models inside [`Vision::dir`].
pub const FACES_DIR: &str = "faces";

/// Where an index lives and what it holds.
#[derive(Clone, Debug, PartialEq, Eq)]
struct IndexSpec {
    /// `None`: in memory only (a library that isn't a folder).
    path: Option<PathBuf>,
    model: String,
    dim: usize,
}

/// Where the text index lives and which reader filled it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct TextSpec {
    path: Option<PathBuf>,
    engine: String,
}

/// How a worker thread gets the model.
#[derive(Clone)]
struct Provider {
    /// A model the host supplied (tests, other hosts); otherwise SigLIP 2 is loaded from `dir`.
    injected: Option<Arc<dyn Embedder>>,
    dir: Option<PathBuf>,
}

/// How a worker thread gets the text reader.
#[derive(Clone)]
struct ReaderProvider {
    injected: Option<Arc<dyn TextReader>>,
    dir: Option<PathBuf>,
}

/// Where the face index lives and which models filled it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct FaceSpec {
    path: Option<PathBuf>,
    engine: String,
}

/// How a worker thread gets the face models.
#[derive(Clone)]
struct FinderProvider {
    injected: Option<Arc<dyn FaceEngine>>,
    dir: Option<PathBuf>,
}

/// What worker threads and the session share.
#[derive(Default)]
struct Shared {
    finder: Mutex<Option<Arc<dyn FaceEngine>>>,
    face_ix: Mutex<Option<(Option<PathBuf>, FaceIndex)>>,
    /// Faces and photos looked at in the face index, readable without its lock.
    face_len: AtomicUsize,
    face_photos: AtomicUsize,
    faces_opened: AtomicBool,
    reader: Mutex<Option<Arc<dyn TextReader>>>,
    text: Mutex<Option<(Option<PathBuf>, TextIndex)>>,
    /// The text index's length, readable without its lock.
    text_len: AtomicUsize,
    text_opened: AtomicBool,
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
    /// The run is reading the text in photos (after describing them).
    pub reading: AtomicBool,
    /// The run is looking for faces in photos (after reading them).
    pub finding: AtomicBool,
    error: Mutex<Option<String>>,
}

impl IndexJob {
    pub fn error(&self) -> Option<String> {
        self.error.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// What the run is doing now: `describing`, `reading` (text) or `finding` (faces).
    pub fn phase(&self) -> &'static str {
        if self.finding.load(Ordering::Relaxed) {
            "finding"
        } else if self.reading.load(Ordering::Relaxed) {
            "reading"
        } else {
            "describing"
        }
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
            "phase": self.phase(),
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
    /// Send this library's search vectors to the server it syncs with, so the server and the
    /// user's other devices needn't compute them again. Off until the user turns it on
    /// (persisted in the library's prefs.json).
    pub share_with_server: bool,
    /// Also read the text in photos and search it. Off until the user turns it on (persisted in
    /// the library's prefs.json): reading is slow and needs its own download.
    pub text: bool,
    /// Find the faces in photos and group them into people. Off until the user turns it on
    /// (persisted in the library's prefs.json): it needs its own download and the user's yes.
    pub faces: bool,
    /// This device runs none of the models even if this build could (a thin client; tests of one):
    /// it searches and lists people through the sync server.
    #[doc(hidden)]
    pub thin: bool,
    /// Long edge of the renderings text and faces are read from ([`TEXT_EDGE`]; smaller in tests).
    #[doc(hidden)]
    pub text_edge: usize,
    injected: Option<Arc<dyn Embedder>>,
    injected_reader: Option<Arc<dyn TextReader>>,
    injected_finder: Option<Arc<dyn FaceEngine>>,
    people: people::Cache,
    remote: server::Remote,
    shared: Arc<Shared>,
    job: Option<Arc<IndexJob>>,
    job_reported: bool,
    search_seq: u64,
    searching: Arc<AtomicUsize>,
    results: (Sender<Searched>, Receiver<Searched>),
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    opening: Arc<AtomicBool>,
    messages: Vec<String>,
    #[cfg(feature = "vision")]
    download: crate::download::Downloader,
    #[cfg(feature = "vision")]
    text_download: crate::download::Downloader,
    #[cfg(feature = "vision")]
    faces_download: crate::download::Downloader,
}

impl Default for Vision {
    fn default() -> Self {
        Vision {
            dir: None,
            mirrors_file: None,
            background: false,
            share_with_server: false,
            text: false,
            faces: false,
            thin: false,
            text_edge: TEXT_EDGE,
            injected: None,
            injected_reader: None,
            injected_finder: None,
            people: people::Cache::default(),
            remote: server::Remote::default(),
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
            #[cfg(feature = "vision")]
            text_download: crate::download::Downloader::default(),
            #[cfg(feature = "vision")]
            faces_download: crate::download::Downloader::default(),
        }
    }
}

impl Vision {
    /// Whether this device can run the search model (this build has it, or a host supplied one).
    pub fn local_available(&self) -> bool {
        (cfg!(feature = "vision") && !self.thin) || self.injected.is_some()
    }

    /// Whether searching by description is possible here: with the model on this device, or
    /// through the server the library syncs with (which says so when asked).
    pub fn available(&self) -> bool {
        self.local_available() || self.remote.supported()
    }

    /// Takes `status` as what the sync server said about its search, as if it had just answered
    /// (tests of a UI that has no server to ask).
    #[doc(hidden)]
    pub fn set_server_status(&mut self, status: Option<Value>) {
        self.remote.set_status(status);
    }

    /// The sync server can search now (its model is installed).
    pub fn server_ready(&self) -> bool {
        self.remote.ready()
    }

    /// What the sync server last said about its search: `{available, installed, indexed, total, …}`.
    pub fn server_status(&self) -> Option<&Value> {
        self.remote.status.as_ref()
    }

    /// The local vectors are being sent to the server.
    pub fn sharing(&self) -> bool {
        self.remote.share.running
    }

    /// Why the last send of the local vectors failed.
    pub fn share_error(&self) -> Option<&str> {
        self.remote.share.error.as_deref()
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
        self.searching.load(Ordering::SeqCst) > 0 || self.remote.pending_seq.is_some()
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
        if !self.local_available() {
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

    /// Use `reader` instead of loading the text-reading models from the model folder (tests,
    /// other hosts).
    pub fn set_text_reader(&mut self, reader: Arc<dyn TextReader>) {
        self.injected_reader = Some(reader);
    }

    /// Where the text-reading models live (inside the model folder).
    pub fn text_dir(&self) -> Option<PathBuf> {
        self.dir.as_ref().map(|d| d.join(TEXT_DIR))
    }

    /// Whether this device can read the text in photos (this build has the readers, or a host
    /// supplied one).
    pub fn text_available(&self) -> bool {
        (cfg!(feature = "vision") && !self.thin) || self.injected_reader.is_some()
    }

    /// Whether the text-reading models' files are in place.
    pub fn text_installed(&self) -> bool {
        if self.injected_reader.is_some() {
            return true;
        }
        #[cfg(feature = "vision")]
        if let Some(dir) = self.text_dir() {
            return lightcraft_vision::ocr::is_model_dir(&dir);
        }
        false
    }

    /// Text in photos is wanted (the user's choice) and this device can read it.
    pub fn text_ready(&self) -> bool {
        self.text && self.text_installed()
    }

    /// Photos whose text has been read (0 until the text index has been opened).
    pub fn text_indexed(&self) -> usize {
        self.shared.text_len.load(Ordering::Relaxed)
    }

    /// The text-reading models' download state.
    pub fn text_download_status(&self) -> Value {
        #[cfg(feature = "vision")]
        {
            let s = self.text_download.status();
            json!({"running": s.running, "done": s.done, "total": s.total, "file": s.file, "error": s.error, "finished": s.finished})
        }
        #[cfg(not(feature = "vision"))]
        json!({"running": false, "done": 0, "total": 0, "file": "", "error": null, "finished": false})
    }

    /// Start downloading the text-reading models on a background thread. `Ok(false)` when they
    /// are installed or downloading already.
    pub fn start_text_download(&self) -> Result<bool, String> {
        if !self.text_available() {
            return Err("reading the text in photos is not available in this build".into());
        }
        if self.text_installed() {
            return Ok(false);
        }
        #[cfg(feature = "vision")]
        {
            let dir = self.text_dir().ok_or("no folder is set for the search models")?;
            let env = std::env::var(lightcraft_vision::models::MIRRORS_ENV).ok();
            let parts = lightcraft_vision::models::ocr_downloads(&dir, env.as_deref(), self.mirrors_file.as_deref());
            self.text_download.start_parts("text", parts, lightcraft_vision::models::Options::default())
        }
        #[cfg(not(feature = "vision"))]
        Ok(false)
    }

    /// Stop a running text-model download (its partial files stay, to resume). False when none runs.
    pub fn cancel_text_download(&self) -> bool {
        #[cfg(feature = "vision")]
        return self.text_download.cancel();
        #[cfg(not(feature = "vision"))]
        false
    }

    /// What reads the text and how to reach it from a worker thread, with the engine's id, or why
    /// it can't be.
    fn reader_provider(&self) -> Result<(ReaderProvider, String), String> {
        if let Some(r) = &self.injected_reader {
            return Ok((ReaderProvider { injected: Some(r.clone()), dir: None }, r.engine().to_string()));
        }
        #[cfg(feature = "vision")]
        {
            if !self.text_installed() {
                let d = self.text_download_status();
                if d["running"].as_bool() == Some(true) {
                    return Err(format!("{TEXT_NOT_INSTALLED} yet: they are downloading."));
                }
                return Err(format!(
                    "{TEXT_NOT_INSTALLED}. Download them (about {:.0} MB, {TEXT_LICENSE_NAME}) with `vision.text.download {{\"acknowledged\": true}}`.",
                    TEXT_BYTES as f64 / 1e6
                ));
            }
            Ok((ReaderProvider { injected: None, dir: self.text_dir() }, lightcraft_vision::ocr::ENGINE.to_string()))
        }
        #[cfg(not(feature = "vision"))]
        Err("reading the text in photos is not available in this build".into())
    }

    /// Use `finder` instead of loading the face models from the model folder (tests, other hosts).
    pub fn set_face_finder(&mut self, finder: Arc<dyn FaceEngine>) {
        self.injected_finder = Some(finder);
    }

    /// Where the face models live (inside the model folder).
    pub fn faces_dir(&self) -> Option<PathBuf> {
        self.dir.as_ref().map(|d| d.join(FACES_DIR))
    }

    /// Whether this device can find faces (this build has the models' code, or a host supplied a
    /// finder).
    pub fn faces_available(&self) -> bool {
        (cfg!(feature = "vision") && !self.thin) || self.injected_finder.is_some()
    }

    /// Whether the face models' files are in place.
    pub fn faces_installed(&self) -> bool {
        if self.injected_finder.is_some() {
            return true;
        }
        #[cfg(feature = "vision")]
        if let Some(dir) = self.faces_dir() {
            return lightcraft_vision::faces::model::is_model_dir(&dir);
        }
        false
    }

    /// Finding people is wanted (the user's choice) and this device can do it.
    pub fn faces_ready(&self) -> bool {
        self.faces && self.faces_installed()
    }

    /// The sync server finds the faces in this user's photos (so it can list the people).
    pub fn server_faces(&self) -> bool {
        self.remote.faces_enabled()
    }

    /// Changes whenever the people the server listed change (a view's cache key).
    pub fn people_rev(&self) -> u64 {
        self.remote.people_rev
    }

    /// Faces in the index (0 until it has been opened).
    pub fn faces_found(&self) -> usize {
        self.shared.face_len.load(Ordering::Relaxed)
    }

    /// Photos looked at for faces (0 until the index has been opened).
    pub fn faces_scanned(&self) -> usize {
        self.shared.face_photos.load(Ordering::Relaxed)
    }

    /// The face models' download state.
    pub fn faces_download_status(&self) -> Value {
        #[cfg(feature = "vision")]
        {
            let s = self.faces_download.status();
            json!({"running": s.running, "done": s.done, "total": s.total, "file": s.file, "error": s.error, "finished": s.finished})
        }
        #[cfg(not(feature = "vision"))]
        json!({"running": false, "done": 0, "total": 0, "file": "", "error": null, "finished": false})
    }

    /// Start downloading the face models on a background thread. `Ok(false)` when they are
    /// installed or downloading already.
    pub fn start_faces_download(&self) -> Result<bool, String> {
        if !self.faces_available() {
            return Err("finding people in photos is not available in this build".into());
        }
        if self.faces_installed() {
            return Ok(false);
        }
        #[cfg(feature = "vision")]
        {
            let dir = self.faces_dir().ok_or("no folder is set for the search models")?;
            let env = std::env::var(lightcraft_vision::models::MIRRORS_ENV).ok();
            let parts = lightcraft_vision::models::face_downloads(&dir, env.as_deref(), self.mirrors_file.as_deref());
            self.faces_download.start_parts("faces", parts, lightcraft_vision::models::Options::default())
        }
        #[cfg(not(feature = "vision"))]
        Ok(false)
    }

    /// Stop a running face-model download (its partial files stay, to resume). False when none runs.
    pub fn cancel_faces_download(&self) -> bool {
        #[cfg(feature = "vision")]
        return self.faces_download.cancel();
        #[cfg(not(feature = "vision"))]
        false
    }

    /// What finds faces and how to reach it from a worker thread, with the engine's id, or why it
    /// can't be.
    fn finder_provider(&self) -> Result<(FinderProvider, String), String> {
        if let Some(f) = &self.injected_finder {
            return Ok((FinderProvider { injected: Some(f.clone()), dir: None }, f.engine().to_string()));
        }
        #[cfg(feature = "vision")]
        {
            if !self.faces_installed() {
                if self.faces_download_status()["running"].as_bool() == Some(true) {
                    return Err(format!("{FACES_NOT_INSTALLED} yet: they are downloading."));
                }
                return Err(format!(
                    "{FACES_NOT_INSTALLED}. Download them (about {:.0} MB, MIT and Apache 2.0 licences) with `vision.faces.download {{\"acknowledged\": true}}`.",
                    FACE_BYTES as f64 / 1e6
                ));
            }
            Ok((FinderProvider { injected: None, dir: self.faces_dir() }, lightcraft_vision::faces::ENGINE.to_string()))
        }
        #[cfg(not(feature = "vision"))]
        Err("finding people in photos is not available in this build".into())
    }

    /// The local model's id and vector length, installed or not (what an index on disk was made with).
    fn spec_only(&self) -> Result<(String, usize), String> {
        if let Some(m) = &self.injected {
            return Ok((m.model_id().to_string(), m.dim()));
        }
        #[cfg(feature = "vision")]
        return Ok((lightcraft_vision::siglip::MODEL_ID.to_string(), lightcraft_vision::siglip::DIM));
        #[cfg(not(feature = "vision"))]
        Err("this device has no search data to send".into())
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

/// The text reader, loading it first if needed (like [`acquire`]).
fn acquire_reader(shared: &Shared, provider: &ReaderProvider) -> Result<Arc<dyn TextReader>, String> {
    if let Some(r) = &provider.injected {
        shared.touch();
        return Ok(r.clone());
    }
    let mut slot = shared.reader.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(r) = slot.as_ref() {
        shared.touch();
        return Ok(r.clone());
    }
    shared.loading.store(true, Ordering::SeqCst);
    let loaded = load_reader(provider.dir.as_deref());
    shared.loading.store(false, Ordering::SeqCst);
    let r = loaded?;
    *slot = Some(r.clone());
    shared.loaded.store(true, Ordering::SeqCst);
    shared.touch();
    Ok(r)
}

fn load_reader(dir: Option<&std::path::Path>) -> Result<Arc<dyn TextReader>, String> {
    #[cfg(feature = "vision")]
    {
        let dir = dir.ok_or("no folder is set for the search models")?;
        let ocr = guard::catch("loading the text reader", || lightcraft_vision::ocr::Ocr::load(dir))?.map_err(|e| e.to_string())?;
        Ok(Arc::new(ocr))
    }
    #[cfg(not(feature = "vision"))]
    {
        let _ = dir;
        Err("reading the text in photos is not available in this build".into())
    }
}

/// Opens the text index at `spec` (another reader's, or damaged, is derived data: started over).
fn open_text_index(spec: &TextSpec) -> Result<TextIndex, String> {
    use lightcraft_vision::Error;
    let Some(path) = &spec.path else { return TextIndex::in_memory(&spec.engine).map_err(|e| e.to_string()) };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("can't create {}: {e}", parent.display()))?;
    }
    match TextIndex::open(path, &spec.engine) {
        Ok(ix) => Ok(ix),
        Err(Error::Mismatch { .. } | Error::Format(_)) => {
            log::info!("text index {} is for another reader or damaged: starting over", path.display());
            let _ = std::fs::remove_file(path);
            TextIndex::open(path, &spec.engine).map_err(|e| e.to_string())
        }
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Runs `f` on the text index at `spec`, opening it first (or again, when it is another one).
fn with_text<R>(shared: &Shared, spec: &TextSpec, f: impl FnOnce(&mut TextIndex) -> R) -> Result<R, String> {
    let mut slot = shared.text.lock().unwrap_or_else(PoisonError::into_inner);
    let stale = slot.as_ref().is_none_or(|(p, ix)| *p != spec.path || ix.engine() != spec.engine);
    if stale {
        let ix = open_text_index(spec)?;
        *slot = Some((spec.path.clone(), ix));
    }
    let (_, ix) = slot.as_mut().ok_or("the text index is unavailable")?;
    let r = f(ix);
    shared.text_len.store(ix.len(), Ordering::Relaxed);
    shared.text_opened.store(true, Ordering::Relaxed);
    Ok(r)
}

/// The face finder, loading it first if needed (like [`acquire`]).
fn acquire_finder(shared: &Shared, provider: &FinderProvider) -> Result<Arc<dyn FaceEngine>, String> {
    if let Some(f) = &provider.injected {
        shared.touch();
        return Ok(f.clone());
    }
    let mut slot = shared.finder.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(f) = slot.as_ref() {
        shared.touch();
        return Ok(f.clone());
    }
    shared.loading.store(true, Ordering::SeqCst);
    let loaded = load_finder(provider.dir.as_deref());
    shared.loading.store(false, Ordering::SeqCst);
    let f = loaded?;
    *slot = Some(f.clone());
    shared.loaded.store(true, Ordering::SeqCst);
    shared.touch();
    Ok(f)
}

fn load_finder(dir: Option<&std::path::Path>) -> Result<Arc<dyn FaceEngine>, String> {
    #[cfg(feature = "vision")]
    {
        let dir = dir.ok_or("no folder is set for the search models")?;
        let faces = guard::catch("loading the face models", || lightcraft_vision::faces::model::Faces::load(dir))?.map_err(|e| e.to_string())?;
        Ok(Arc::new(faces))
    }
    #[cfg(not(feature = "vision"))]
    {
        let _ = dir;
        Err("finding people in photos is not available in this build".into())
    }
}

/// Opens the face index at `spec` (another pair of models', or damaged, is derived data: started over).
fn open_face_index(spec: &FaceSpec) -> Result<FaceIndex, String> {
    use lightcraft_vision::Error;
    let Some(path) = &spec.path else { return Ok(FaceIndex::in_memory()) };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("can't create {}: {e}", parent.display()))?;
    }
    match FaceIndex::open(path) {
        Ok(ix) => Ok(ix),
        Err(Error::Mismatch { .. } | Error::Format(_)) => {
            log::info!("face index {} is for other models or damaged: starting over", path.display());
            let _ = std::fs::remove_file(path);
            FaceIndex::open(path).map_err(|e| e.to_string())
        }
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Runs `f` on the face index at `spec`, opening it first (or again, when it is another one).
fn with_faces<R>(shared: &Shared, spec: &FaceSpec, f: impl FnOnce(&mut FaceIndex) -> R) -> Result<R, String> {
    let mut slot = shared.face_ix.lock().unwrap_or_else(PoisonError::into_inner);
    if slot.as_ref().is_none_or(|(p, _)| *p != spec.path) {
        let ix = open_face_index(spec)?;
        *slot = Some((spec.path.clone(), ix));
    }
    let (_, ix) = slot.as_mut().ok_or("the face index is unavailable")?;
    let r = f(ix);
    shared.face_len.store(ix.len(), Ordering::Relaxed);
    shared.face_photos.store(ix.photos(), Ordering::Relaxed);
    shared.faces_opened.store(true, Ordering::Relaxed);
    Ok(r)
}

/// What an indexing run does: embed photos (describe), read their text, find faces, or all.
struct Plan {
    semantic: Option<(Provider, IndexSpec)>,
    text: Option<(ReaderProvider, TextSpec)>,
    faces: Option<(FinderProvider, FaceSpec)>,
    embed: Vec<(Key, RenderJob)>,
    read: Vec<(Key, RenderJob)>,
    scan: Vec<(Key, RenderJob)>,
}

/// Embeds `plan.embed`'s photos in batches and stores their vectors, then reads the text of
/// `plan.read`'s and stores that.
fn run_index(shared: &Arc<Shared>, plan: Plan, job: &IndexJob) {
    let _busy = Busy::new(shared);
    let Plan { semantic, text, faces, embed, read, scan } = plan;
    let result = guard::catch("indexing photos", || -> Result<(), String> {
        if let Some((provider, spec)) = &semantic {
            describe_photos(shared, provider, spec, embed, job)?;
        }
        if let Some((provider, spec)) = &text
            && !job.cancel.load(Ordering::Relaxed)
            && job.error().is_none()
        {
            job.reading.store(true, Ordering::Relaxed);
            read_photos(shared, provider, spec, read, job)?;
        }
        if let Some((provider, spec)) = &faces
            && !job.cancel.load(Ordering::Relaxed)
            && job.error().is_none()
        {
            job.finding.store(true, Ordering::Relaxed);
            find_faces(shared, provider, spec, scan, job)?;
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

/// Embeds `work`'s photos in batches and stores their vectors.
fn describe_photos(shared: &Arc<Shared>, provider: &Provider, spec: &IndexSpec, work: Vec<(Key, RenderJob)>, job: &IndexJob) -> Result<(), String> {
    if work.is_empty() {
        return Ok(());
    }
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
}

/// Reads the text of `work`'s photos, one at a time, and stores it (a photo with no text is
/// stored as empty, so it isn't read again).
fn read_photos(shared: &Arc<Shared>, provider: &ReaderProvider, spec: &TextSpec, work: Vec<(Key, RenderJob)>, job: &IndexJob) -> Result<(), String> {
    if work.is_empty() {
        return Ok(());
    }
    let reader = acquire_reader(shared, provider)?;
    let mut failures = 0;
    for (key, render) in work {
        if job.cancel.load(Ordering::Relaxed) {
            break;
        }
        let read = memory::in_background(|| render.run().rendered)
            .and_then(|r| guard::catch("reading text", || reader.text(&r.image)).and_then(|t| t.map_err(|e| e.to_string())));
        let stopped = match read {
            Ok(text) => {
                failures = 0;
                if with_text(shared, spec, |ix| ix.insert(key, &text).is_err())? {
                    job.failed.fetch_add(1, Ordering::Relaxed);
                }
                None
            }
            Err(e) => {
                log::debug!("text index: can't read a photo: {e}");
                job.failed.fetch_add(1, Ordering::Relaxed);
                failures += 1;
                // a reader that fails this often will keep failing
                (failures >= READ_FAILURES).then(|| format!("reading text stopped after {READ_FAILURES} photos in a row failed: {e}"))
            }
        };
        job.done.fetch_add(1, Ordering::Relaxed);
        shared.touch();
        if let Some(e) = stopped {
            job.fail(e);
            return Ok(());
        }
    }
    Ok(())
}

/// Looks for the faces in `work`'s photos, one at a time, and stores them (a photo with none is
/// stored as looked at, so it isn't looked at again).
fn find_faces(shared: &Arc<Shared>, provider: &FinderProvider, spec: &FaceSpec, work: Vec<(Key, RenderJob)>, job: &IndexJob) -> Result<(), String> {
    if work.is_empty() {
        return Ok(());
    }
    let finder = acquire_finder(shared, provider)?;
    let mut failures = 0;
    for (key, render) in work {
        if job.cancel.load(Ordering::Relaxed) {
            break;
        }
        let found = memory::in_background(|| render.run().rendered)
            .and_then(|r| guard::catch("finding faces", || finder.faces(&r.image)).and_then(|t| t.map_err(|e| e.to_string())));
        let stopped = match found {
            Ok(faces) => {
                failures = 0;
                if with_faces(shared, spec, |ix| ix.insert_photo(key, &faces).is_err())? {
                    job.failed.fetch_add(1, Ordering::Relaxed);
                }
                None
            }
            Err(e) => {
                log::debug!("face index: can't look at a photo: {e}");
                job.failed.fetch_add(1, Ordering::Relaxed);
                failures += 1;
                (failures >= READ_FAILURES).then(|| format!("finding faces stopped after {READ_FAILURES} photos in a row failed: {e}"))
            }
        };
        job.done.fetch_add(1, Ordering::Relaxed);
        shared.touch();
        if let Some(e) = stopped {
            job.fail(e);
            return Ok(());
        }
    }
    Ok(())
}

/// Where a search looks: the description model's index, the photos' text, or both.
struct Where {
    semantic: Option<(Provider, IndexSpec)>,
    text: Option<TextSpec>,
}

/// The photos best matching `query`: those with its words in them first, then those that look
/// like it.
fn run_search(shared: &Arc<Shared>, at: &Where, query: &str, limit: usize) -> Result<Vec<Hit>, String> {
    let _busy = Busy::new(shared);
    guard::catch("searching", || -> Result<Vec<Hit>, String> {
        let words = match &at.text {
            Some(spec) => with_text(shared, spec, |ix| ix.search(query, limit))?,
            None => Vec::new(),
        };
        let mut looks = Vec::new();
        if let Some((provider, spec)) = &at.semantic {
            let model = acquire(shared, provider)?;
            let v = model.encode_text(query).map_err(|e| e.to_string())?;
            shared.touch();
            looks = with_index(shared, spec, |ix| ix.search(&v, limit).map_err(|e| e.to_string()))??;
        }
        Ok(lightcraft_vision::textindex::merge(&words, looks, limit))
    })?
}

/// A neutral (unedited) rendering of `photo` from a smart or mini preview file, at most `edge` on
/// its long edge: what search embeds, so a photo gets the same vector wherever it is indexed (a
/// device renders its thumbnail source the same way, with default settings), or the larger one the
/// text in it is read from (`edge` up to a smart preview's 2560). The server uses it on
/// the previews it keeps.
#[cfg(not(target_arch = "wasm32"))]
pub fn neutral_from_preview(photo: &lightcraft_catalog::Photo, preview: &std::path::Path, edge: usize) -> Result<Rgba8, String> {
    use crate::media::{SourceRef, source_info};
    let edge = edge.clamp(16, SourceLevel::Preview.max_edge());
    let level = SourceLevel::for_size(edge);
    let job = RenderJob {
        photo: photo.id,
        level,
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

    /// The text index file for `engine`: in the library's `search/` folder, else none (memory only).
    fn vision_text_path(&self, engine: &str) -> Option<PathBuf> {
        let lib = self.library.as_ref().filter(|l| l.on_disk)?;
        Some(lib.dir.join("search").join(format!("text-{engine}.bin")))
    }

    /// The face index file for `engine`: in the library's `search/` folder, else none (memory only).
    fn vision_faces_path(&self, engine: &str) -> Option<PathBuf> {
        let lib = self.library.as_ref().filter(|l| l.on_disk)?;
        Some(lib.dir.join("search").join(format!("faces-{engine}.bin")))
    }

    /// Where to look for words in photos: when the user wants that, the models are installed and
    /// something has been read.
    fn vision_text_where(&self) -> Option<TextSpec> {
        if !self.vision.text_ready() {
            return None;
        }
        let (_, engine) = self.vision.reader_provider().ok()?;
        let spec = TextSpec { path: self.vision_text_path(&engine), engine };
        (with_text(&self.vision.shared, &spec, |ix| ix.len()).ok()? > 0).then_some(spec)
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
            "localAvailable": v.local_available(),
            "server": {"supported": v.remote.supported(), "ready": v.remote.ready(), "status": v.remote.status},
            "share": {"enabled": v.share_with_server, "running": v.remote.share.running, "sent": v.remote.share.sent, "error": v.remote.share.error},
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
            "faces": {
                "available": v.faces_available(),
                "enabled": v.faces,
                "installed": v.faces_installed(),
                "dir": v.faces_dir().map(|d| d.display().to_string()),
                "found": v.shared.faces_opened.load(Ordering::Relaxed).then(|| v.faces_found()),
                "scanned": v.shared.faces_opened.load(Ordering::Relaxed).then(|| v.faces_scanned()),
                "sizeBytes": FACE_BYTES,
                "licenses": FACE_LICENSES.iter().map(|(n, u)| json!({"name": n, "url": u})).collect::<Vec<_>>(),
                "download": v.faces_download_status(),
            },
            "text": {
                "available": v.text_available(),
                "enabled": v.text,
                "installed": v.text_installed(),
                "dir": v.text_dir().map(|d| d.display().to_string()),
                "indexed": v.shared.text_opened.load(Ordering::Relaxed).then(|| v.text_indexed()),
                "sizeBytes": TEXT_BYTES,
                "license": TEXT_LICENSE_NAME,
                "licenseUrl": TEXT_LICENSE_URL,
                "download": v.text_download_status(),
            },
        })
    }

    /// Embed the library's photos that aren't in the index yet (`vision.index`): all of them, or
    /// `ids`. Runs on a thread (progress by [`IndexJob`], cancel by flag) or, with `wait`, here.
    pub fn vision_index(&mut self, ids: Option<Vec<PhotoId>>, wait: bool) -> std::result::Result<Value, String> {
        if self.vision.job.as_ref().is_some_and(|j| !j.finished.load(Ordering::Relaxed)) {
            return Err("photos are already being indexed".into());
        }
        // describing needs the search model; reading text needs the user's yes and its own models.
        // What can't run is left out; only when nothing can is that an error.
        let semantic = self.vision.provider().map(|(p, model, dim)| {
            let spec = IndexSpec { path: self.vision_index_path(&model), model, dim };
            (p, spec)
        });
        let text = if self.vision.text {
            self.vision.reader_provider().map(|(p, engine)| (p, TextSpec { path: self.vision_text_path(&engine), engine }))
        } else {
            Err(String::new())
        };
        let faces = if self.vision.faces {
            self.vision.finder_provider().map(|(p, engine)| (p, FaceSpec { path: self.vision_faces_path(&engine), engine }))
        } else {
            Err(String::new())
        };
        if semantic.is_err() && text.is_err() && faces.is_err() {
            return Err(semantic.err().unwrap_or_default());
        }
        let (semantic, text, faces) = (semantic.ok(), text.ok(), faces.ok());
        let shared = self.vision.shared.clone();
        let mut seen: HashSet<Key> = match &semantic {
            Some((_, spec)) => with_index(&shared, spec, |ix| ix.keys().copied().collect())?,
            None => HashSet::new(),
        };
        let mut seen_text: HashSet<Key> = match &text {
            Some((_, spec)) => with_text(&shared, spec, |ix| ix.keys().copied().collect())?,
            None => HashSet::new(),
        };
        let mut seen_faces: HashSet<Key> = match &faces {
            Some((_, spec)) => with_faces(&shared, spec, |ix| ix.keys().copied().collect())?,
            None => HashSet::new(),
        };
        let want: Vec<PhotoId> = match ids {
            Some(ids) => ids,
            None => self.catalog.photos().filter(|p| p.in_library()).map(|p| p.id).collect(),
        };
        let (mut embed, mut read, mut scan): (Vec<(Key, RenderJob)>, Vec<(Key, RenderJob)>, Vec<(Key, RenderJob)>) =
            (Vec::new(), Vec::new(), Vec::new());
        let (mut failed, edge) = (0, self.vision.text_edge);
        for id in want {
            let Some(key) = self.catalog.photo(id).filter(|p| p.in_library()).map(|p| Key::of(&content_key(p))) else { continue };
            // (a photo is described and read once per content, whatever copies it has)
            let describe = semantic.is_some() && seen.insert(key);
            let reads = text.is_some() && seen_text.insert(key);
            let looks = faces.is_some() && seen_faces.insert(key);
            // a neutral rendering: the photo as its file looks, whatever the user did to it; not
            // cached, so indexing doesn't fill the thumbnail cache with a second set
            if describe {
                match self.variant_job(id, &DevelopSettings::default(), THUMB_EDGE) {
                    Some(mut job) => {
                        job.cache = None;
                        embed.push((key, job));
                    }
                    None => failed += 1,
                }
            }
            for (wanted, list) in [(reads, &mut read), (looks, &mut scan)] {
                if !wanted {
                    continue;
                }
                match self.variant_job_at(id, &DevelopSettings::default(), edge, SourceLevel::for_size(edge)) {
                    Some(mut job) => {
                        job.cache = None;
                        list.push((key, job));
                    }
                    None => failed += 1,
                }
            }
        }
        let job = Arc::new(IndexJob { total: embed.len() + read.len() + scan.len() + failed, ..Default::default() });
        job.done.store(failed, Ordering::Relaxed);
        job.failed.store(failed, Ordering::Relaxed);
        self.vision.job = Some(job.clone());
        self.vision.job_reported = wait;
        let plan = Plan { semantic, text, faces, embed, read, scan };
        if wait || !self.vision.background || cfg!(target_arch = "wasm32") {
            run_index(&shared, plan, &job);
            self.vision.job_reported = true;
            return Ok(job.json());
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let j = job.clone();
            let started = std::thread::Builder::new().name("lc-vision-index".into()).spawn(move || run_index(&shared, plan, &j));
            if let Err(e) = started {
                job.finished.store(true, Ordering::Relaxed);
                return Err(format!("could not start indexing: {e}"));
            }
        }
        Ok(job.json())
    }

    /// Whether this device's own index holds a vector for every photo of the library.
    fn vision_local_covers(&self) -> bool {
        let Ok((_, model, dim)) = self.vision.provider() else { return false };
        let spec = IndexSpec { path: self.vision_index_path(&model), model, dim };
        let total = self.vision_photo_count();
        total > 0 && with_index(&self.vision.shared, &spec, |ix| ix.len()).is_ok_and(|n| n >= total)
    }

    /// Search by description (`library.search`): the best `limit` photos, best first, as the
    /// view's filter. `source`: `local` (this device's model and index), `server` (the sync
    /// server's) or `auto` (this device's when its index covers the whole library, else the
    /// server's, else whatever of this device's there is). With `wait` (or without
    /// [`Vision::background`]) the result is applied here; otherwise it is computed in the
    /// background and [`Session::vision_poll`] applies it.
    #[cfg_attr(target_arch = "wasm32", allow(unused_variables))]
    pub fn vision_search(&mut self, query: &str, limit: usize, wait: bool, source: &str) -> std::result::Result<Value, String> {
        let query: String = query.trim().chars().take(MAX_QUERY).collect();
        if query.is_empty() {
            return Err("describe the photo you are looking for".into());
        }
        let limit = limit.clamp(1, MAX_LIMIT);
        let local = self.vision.local_available() && self.vision.installed();
        let server = self.vision.remote.ready() && self.vision_signed_in();
        let use_server = match source {
            "local" => false,
            "server" => true,
            "auto" | "" => !(local && self.vision_local_covers()) && server,
            other => return Err(format!("unknown source `{other}` (auto|local|server)")),
        };
        if use_server {
            return self.vision_server_search(&query, limit, wait);
        }
        // the words printed in photos need no description model; the look of them does
        let text = self.vision_text_where();
        let semantic: Result<(Provider, IndexSpec, usize), String> = if !local {
            Err(match (self.vision.local_available(), self.vision_signed_in()) {
                (true, _) => self.vision.provider().err().unwrap_or_default(),
                (false, true) => "this server can't search by description yet: its search model isn't installed".into(),
                (false, false) => {
                    "search by description needs a LightCraft server with the search model installed, and this library isn't signed in to one".into()
                }
            })
        } else {
            self.vision.provider().and_then(|(provider, model, dim)| {
                let spec = IndexSpec { path: self.vision_index_path(&model), model, dim };
                let known = with_index(&self.vision.shared, &spec, |ix| ix.len())?;
                if known == 0 {
                    return Err("no photos are indexed for search yet: run `vision.index` first".into());
                }
                Ok((provider, spec, known))
            })
        };
        if text.is_none() && semantic.is_err() {
            return Err(semantic.err().unwrap_or_default());
        }
        let known = semantic.as_ref().map_or_else(|_| self.vision.text_indexed(), |(_, _, n)| *n);
        let at = Where { semantic: semantic.ok().map(|(p, spec, _)| (p, spec)), text };
        self.vision.search_seq += 1;
        let seq = self.vision.search_seq;
        let shared = self.vision.shared.clone();
        if wait || !self.vision.background || cfg!(target_arch = "wasm32") {
            let hits = run_search(&shared, &at, &query, limit)?;
            return Ok(self.apply_hits(&query, &hits));
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (tx, searching, q) = (self.vision.results.0.clone(), self.vision.searching.clone(), query.clone());
            searching.fetch_add(1, Ordering::SeqCst);
            let started = std::thread::Builder::new().name("lc-vision-search".into()).spawn(move || {
                let hits = run_search(&shared, &at, &q, limit);
                let _ = tx.send(Searched { seq, query: q, hits });
                let _ = searching.try_update(Ordering::SeqCst, Ordering::SeqCst, |n| Some(n.saturating_sub(1)));
            });
            if let Err(e) = started {
                self.vision.searching.fetch_sub(1, Ordering::SeqCst);
                return Err(format!("could not start the search: {e}"));
            }
        }
        Ok(json!({"query": query, "status": "searching", "source": "local", "indexed": known}))
    }

    /// This device's hits as photos (by content, so virtual copies of a hit come with it).
    fn apply_hits(&mut self, query: &str, hits: &[Hit]) -> Value {
        let by_key = self.vision_keys();
        let mut found: Vec<(PhotoId, f32)> = Vec::new();
        for h in hits {
            for id in by_key.get(&h.key).into_iter().flatten() {
                found.push((*id, h.score));
            }
        }
        self.apply_ids(query, &found, "local")
    }

    /// Sets the view to `found` (best first) for `query`. An empty result is an empty view, not
    /// "everything".
    fn apply_ids(&mut self, query: &str, found: &[(PhotoId, f32)], source: &str) -> Value {
        self.filter.only = found.iter().map(|(id, _)| *id).collect();
        self.filter.semantic = Some(query.to_string());
        json!({
            "query": query,
            "source": source,
            // (a score from TEXT_SCORE_BASE up is a match on words printed in the photo)
            "photos": found
                .iter()
                .map(|(id, score)| json!({"id": id.0, "score": (f64::from(*score) * 1e4).round() / 1e4, "text": *score >= TEXT_SCORE_BASE}))
                .collect::<Vec<_>>(),
            "indexed": self.vision.indexed(),
            "libraryPhotos": self.vision_photo_count(),
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
        self.vision_server_poll(&mut out);
        self.vision_open_in_background();
        self.vision_unload_if_idle();
        out
    }

    /// Opens the library's index on a thread when it hasn't been, so its size is known without
    /// the first search or indexing run having to wait for the file.
    #[cfg(not(target_arch = "wasm32"))]
    fn vision_open_in_background(&mut self) {
        let sh = &self.vision.shared;
        if self.vision.opening.load(Ordering::SeqCst) || !self.vision.background {
            return;
        }
        // (an index that has no file yet has nothing to wait for)
        let semantic = (!sh.opened.load(Ordering::Relaxed))
            .then(|| self.vision.provider().ok())
            .flatten()
            .map(|(_, model, dim)| IndexSpec { path: self.vision_index_path(&model), model, dim })
            .filter(|s| s.path.as_ref().is_some_and(|p| p.is_file()));
        let text = (!sh.text_opened.load(Ordering::Relaxed) && self.vision.text_ready())
            .then(|| self.vision.reader_provider().ok())
            .flatten()
            .map(|(_, engine)| TextSpec { path: self.vision_text_path(&engine), engine })
            .filter(|s| s.path.as_ref().is_some_and(|p| p.is_file()));
        let faces = (!sh.faces_opened.load(Ordering::Relaxed) && self.vision.faces_ready())
            .then(|| self.vision.finder_provider().ok())
            .flatten()
            .map(|(_, engine)| FaceSpec { path: self.vision_faces_path(&engine), engine })
            .filter(|s| s.path.as_ref().is_some_and(|p| p.is_file()));
        if semantic.is_none() && text.is_none() && faces.is_none() {
            return;
        }
        let (shared, opening) = (sh.clone(), self.vision.opening.clone());
        opening.store(true, Ordering::SeqCst);
        let started = std::thread::Builder::new().name("lc-vision-open".into()).spawn(move || {
            if let Some(spec) = &semantic {
                let _ = with_index(&shared, spec, |_| ());
            }
            if let Some(spec) = &text {
                let _ = with_text(&shared, spec, |_| ());
            }
            if let Some(spec) = &faces {
                let _ = with_faces(&shared, spec, |_| ());
            }
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
        if let (Ok(mut m), Ok(mut ix), Ok(mut r), Ok(mut tx), Ok(mut fd), Ok(mut fx)) =
            (sh.model.try_lock(), sh.index.try_lock(), sh.reader.try_lock(), sh.text.try_lock(), sh.finder.try_lock(), sh.face_ix.try_lock())
        {
            *m = None;
            *ix = None;
            *r = None;
            *tx = None;
            *fd = None;
            *fx = None;
            sh.faces_opened.store(false, Ordering::Relaxed);
            sh.loaded.store(false, Ordering::SeqCst);
            sh.opened.store(false, Ordering::Relaxed);
            sh.text_opened.store(false, Ordering::Relaxed);
            log::info!("the search model was unloaded after {} minutes without use", IDLE_UNLOAD.as_secs() / 60);
        }
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_people;
#[cfg(test)]
mod tests_text;
