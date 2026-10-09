//! Search by description on the server: it turns each user's photos into vectors from the previews
//! it already keeps, answers `GET /api/search?q=…` with the best matches, and takes vectors that a
//! device computed (a desktop with a GPU is far quicker than the server's CPU) so nothing is done
//! twice. Clients (the web build, iOS, other desktops) send a sentence and get photo ids back; none
//! of them needs the model.
//!
//! ```text
//! GET  /api/search/status            {available, installed, model, dim, indexed, total, text: {installed, engine, indexed}}
//! GET  /api/search?q=…&limit=…       {query, ids: [photo id], scores: [f32], indexed, total}
//! GET  /api/index/embeddings/keys    {model, dim, keys: [content hash]} the server already has
//! POST /api/index/embeddings         records a device computed (see `lightcraft_vision::EmbeddingIndex::export`)
//! GET  /api/index/text/keys          {engine, keys: [content hash]} whose text the server has read
//! POST /api/index/text               text a device read (see `lightcraft_vision::TextIndex::export`)
//! GET  /api/people/status            {available, enabled, installed, engine, faces, photos, total}
//! GET  /api/people/clusters          {engine, faces, photos, clusters: [{id, members: [[photo, face, x0, y0, x1, y1, score]]}]}
//! GET  /api/index/faces/keys         {engine, keys: [content hash]} the server has looked at
//! POST /api/index/faces              faces a device found (see `lightcraft_vision::faces::index::FaceIndex::export`)
//! DELETE /api/index/faces            forget every face (always allowed)
//! ```
//!
//! Vectors are filed by content hash, from a neutral (unedited) rendering of the mini preview, in
//! `<data>/users/<name>/search/<model>.bin`: derived data, safe to delete. The model is not part
//! of LightCraft: an admin installs it (`lightcraft-server model download --accept-licences`)
//! and it loads on first use and unloads when idle (~1.5 GB while loaded). Until then every route
//! says so instead of failing.
//!
//! Faces are personal data and off by default: an admin turns finding people on for a user
//! (`lightcraft-server user faces NAME on`) after installing the face models (`model download
//! --faces`); then the server finds the faces in that user's smart previews, and devices ask for
//! the people it grouped them into (`GET /api/people/clusters`) to show and name them (names are
//! ordinary catalog edits, made on the device). A user can delete everything the server found
//! (`DELETE /api/index/faces`), and a desktop can send the faces it found instead
//! (`POST /api/index/faces`).
//!
//! The same goes for the text printed in photos (`model download --text`, PP-OCRv6, ~31 MB): once
//! installed, the server reads each photo's smart preview in the background, a few at a time so
//! describing new photos is never kept waiting, and `GET /api/search` puts the photos that have
//! the query's words in them (scores from `TEXT_SCORE_BASE`) before the ones that look like it.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use lightcraft_catalog::Photo;
use lightcraft_engine::guard;
use lightcraft_vision::faces::index::FaceIndex;
use lightcraft_vision::{Embedder, EmbeddingIndex, Error as VisionError, FaceEngine, Key, TextIndex, TextReader};
use serde_json::json;
use tiny_http::Request;

use crate::State;
use crate::accounts;
use crate::api::{self, Resp, UserLib, error};

/// Long edge of the rendering a photo is embedded from.
const THUMB_EDGE: usize = 512;
/// Long edge of the rendering the text in a photo is read from.
const TEXT_EDGE: usize = lightcraft_engine::vision::TEXT_EDGE;
/// Photos whose text is read in one pass over a user (then the worker looks at the others'
/// photos and at new ones before coming back).
const READ_CHUNK: usize = 25;
/// Photos embedded in one model pass.
const BATCH: usize = 8;
/// How often the index is checked for photos that need a vector, when nothing woke it.
const TICK: Duration = Duration::from_secs(30);
/// The model and the indexes leave memory after this long without use.
const IDLE_UNLOAD: Duration = Duration::from_secs(10 * 60);
const DEFAULT_LIMIT: usize = 200;
const MAX_LIMIT: usize = 2000;
const MAX_QUERY: usize = 512;
/// Largest upload of vectors (a device sends them in pieces of a few thousand).
const UPLOAD_MAX: u64 = 64 << 20;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The search model, the users' indexes and the worker that fills them.
pub struct Search {
    /// Where the model's files are (`<data>/models/siglip2`).
    dir: PathBuf,
    /// A model supplied by the host (tests) instead of SigLIP 2 from `dir`.
    injected: Option<Arc<dyn Embedder>>,
    /// A reader supplied by the host (tests) instead of PP-OCRv6 from `<dir>/ocr`.
    injected_reader: Option<Arc<dyn TextReader>>,
    /// A finder supplied by the host (tests) instead of YuNet and SFace from `<dir>/faces`.
    injected_finder: Option<Arc<dyn FaceEngine>>,
    finder: Mutex<Option<Arc<dyn FaceEngine>>>,
    faces: Mutex<HashMap<String, Arc<Mutex<FaceIndex>>>>,
    /// The people each user's faces make, as the JSON sent to devices, while the faces don't change.
    people: Mutex<HashMap<String, ((usize, usize, u64), Arc<String>)>>,
    model: Mutex<Option<Arc<dyn Embedder>>>,
    reader: Mutex<Option<Arc<dyn TextReader>>>,
    indexes: Mutex<HashMap<String, Arc<Mutex<EmbeddingIndex>>>>,
    texts: Mutex<HashMap<String, Arc<Mutex<TextIndex>>>>,
    used: Mutex<Option<Instant>>,
    wanted: Mutex<bool>,
    wake: Condvar,
    stop: AtomicBool,
    /// Photos that could not be rendered or embedded (not retried until the server restarts).
    failed: Mutex<HashSet<(String, Key)>>,
    /// Photos whose text could not be read (likewise).
    failed_text: Mutex<HashSet<(String, Key)>>,
    /// Photos that could not be looked at for faces (likewise).
    failed_faces: Mutex<HashSet<(String, Key)>>,
    /// The last problem indexing a user's photos.
    problems: Mutex<HashMap<String, String>>,
}

impl Search {
    pub fn new(dir: PathBuf, injected: Option<Arc<dyn Embedder>>) -> Search {
        Search {
            dir,
            injected,
            injected_reader: None,
            injected_finder: None,
            finder: Mutex::new(None),
            faces: Mutex::new(HashMap::new()),
            people: Mutex::new(HashMap::new()),
            model: Mutex::new(None),
            reader: Mutex::new(None),
            indexes: Mutex::new(HashMap::new()),
            texts: Mutex::new(HashMap::new()),
            used: Mutex::new(None),
            wanted: Mutex::new(false),
            wake: Condvar::new(),
            stop: AtomicBool::new(false),
            failed: Mutex::new(HashSet::new()),
            failed_text: Mutex::new(HashSet::new()),
            failed_faces: Mutex::new(HashSet::new()),
            problems: Mutex::new(HashMap::new()),
        }
    }

    /// Finds faces with `finder` instead of YuNet and SFace from the model folder (tests).
    pub fn with_finder(mut self, finder: Option<Arc<dyn FaceEngine>>) -> Search {
        self.injected_finder = finder;
        self
    }

    /// Where the face models' files are (inside the model folder).
    pub fn faces_dir(&self) -> PathBuf {
        self.dir.join(lightcraft_engine::vision::FACES_DIR)
    }

    /// Whether the face models' files are in place.
    pub fn faces_installed(&self) -> bool {
        self.injected_finder.is_some() || lightcraft_vision::faces::model::is_model_dir(&self.faces_dir())
    }

    /// The finder's engine id (known without loading it).
    fn faces_engine(&self) -> String {
        match &self.injected_finder {
            Some(f) => f.engine().to_string(),
            None => lightcraft_vision::faces::ENGINE.to_string(),
        }
    }

    /// The face finder, loaded first if needed.
    fn finder(&self) -> Result<Arc<dyn FaceEngine>, String> {
        if let Some(f) = &self.injected_finder {
            return Ok(f.clone());
        }
        let mut slot = lock(&self.finder);
        if let Some(f) = slot.as_ref() {
            self.touch();
            return Ok(f.clone());
        }
        if !self.faces_installed() {
            return Err(format!(
                "the face models are not installed on this server (run `lightcraft-server model download --accept-licences --faces`, or put them in {})",
                self.faces_dir().display()
            ));
        }
        let started = Instant::now();
        log::info!("loading the face models from {}", self.faces_dir().display());
        let loaded = guard::catch("loading the face models", || lightcraft_vision::faces::model::Faces::load(&self.faces_dir()))?
            .map_err(|e| e.to_string())?;
        let f: Arc<dyn FaceEngine> = Arc::new(loaded);
        *slot = Some(f.clone());
        self.touch();
        log::info!("the face models are loaded ({:.1} s)", started.elapsed().as_secs_f64());
        Ok(f)
    }

    /// A user's face index, opened if needed (other models', or damaged, is derived data: replaced).
    /// Without `create`, a user who has none yet gets `None` and no file is made.
    fn face_index(&self, data: &Path, user: &str, create: bool) -> Result<Option<Arc<Mutex<FaceIndex>>>, String> {
        let mut all = lock(&self.faces);
        if let Some(ix) = all.get(user) {
            return Ok(Some(ix.clone()));
        }
        let dir = accounts::user_dir(data, user).join("search");
        let path = dir.join(format!("faces-{}.bin", self.faces_engine()));
        if !create && !path.is_file() {
            return Ok(None);
        }
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let ix = match FaceIndex::open(&path) {
            Ok(ix) => ix,
            Err(VisionError::Mismatch { .. } | VisionError::Format(_)) => {
                log::info!("{}: the face index is for other models or damaged: starting over", path.display());
                let _ = std::fs::remove_file(&path);
                FaceIndex::open(&path).map_err(|e| format!("{}: {e}", path.display()))?
            }
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        let ix = Arc::new(Mutex::new(ix));
        all.insert(user.to_string(), ix.clone());
        Ok(Some(ix))
    }

    /// Reads text with `reader` instead of PP-OCRv6 from the model folder (tests).
    pub fn with_reader(mut self, reader: Option<Arc<dyn TextReader>>) -> Search {
        self.injected_reader = reader;
        self
    }

    /// Where the text-reading models' files are (inside the model folder).
    pub fn text_dir(&self) -> PathBuf {
        self.dir.join(lightcraft_engine::vision::TEXT_DIR)
    }

    /// Whether the text-reading models' files are in place.
    pub fn text_installed(&self) -> bool {
        self.injected_reader.is_some() || lightcraft_vision::ocr::is_model_dir(&self.text_dir())
    }

    /// The reader's engine id (known without loading it).
    fn engine(&self) -> String {
        match &self.injected_reader {
            Some(r) => r.engine().to_string(),
            None => lightcraft_vision::ocr::ENGINE.to_string(),
        }
    }

    /// The text reader, loaded first if needed.
    fn reader(&self) -> Result<Arc<dyn TextReader>, String> {
        if let Some(r) = &self.injected_reader {
            return Ok(r.clone());
        }
        let mut slot = lock(&self.reader);
        if let Some(r) = slot.as_ref() {
            self.touch();
            return Ok(r.clone());
        }
        if !self.text_installed() {
            return Err(format!(
                "the text-reading models are not installed on this server (run `lightcraft-server model download --accept-licences --text`, or put them in {})",
                self.text_dir().display()
            ));
        }
        let started = Instant::now();
        log::info!("loading the text-reading models from {}", self.text_dir().display());
        let loaded = guard::catch("loading the text reader", || lightcraft_vision::ocr::Ocr::load(&self.text_dir()))?.map_err(|e| e.to_string())?;
        let r: Arc<dyn TextReader> = Arc::new(loaded);
        *slot = Some(r.clone());
        self.touch();
        log::info!("the text-reading models are loaded ({:.1} s)", started.elapsed().as_secs_f64());
        Ok(r)
    }

    /// A user's text index, opened if needed (another reader's, or damaged, is derived data:
    /// replaced). Without `create`, a user who has none yet (nothing read, nothing sent) gets
    /// `None` and no file is made.
    fn text_index(&self, data: &Path, user: &str, create: bool) -> Result<Option<Arc<Mutex<TextIndex>>>, String> {
        let mut all = lock(&self.texts);
        if let Some(ix) = all.get(user) {
            return Ok(Some(ix.clone()));
        }
        let engine = self.engine();
        let dir = accounts::user_dir(data, user).join("search");
        let path = dir.join(format!("text-{engine}.bin"));
        if !create && !path.is_file() {
            return Ok(None);
        }
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let ix = match TextIndex::open(&path, &engine) {
            Ok(ix) => ix,
            Err(VisionError::Mismatch { .. } | VisionError::Format(_)) => {
                log::info!("{}: the text index is for another reader or damaged: starting over", path.display());
                let _ = std::fs::remove_file(&path);
                TextIndex::open(&path, &engine).map_err(|e| format!("{}: {e}", path.display()))?
            }
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        let ix = Arc::new(Mutex::new(ix));
        all.insert(user.to_string(), ix.clone());
        Ok(Some(ix))
    }

    /// Look for photos to index soon (a preview arrived, a scan finished).
    pub fn wake(&self) {
        *lock(&self.wanted) = true;
        self.wake.notify_all();
    }

    pub(crate) fn shut_down(&self) {
        self.stop.store(true, Ordering::SeqCst);
        self.wake.notify_all();
    }

    /// Whether the model's files are in place.
    pub fn installed(&self) -> bool {
        self.injected.is_some() || lightcraft_vision::siglip::is_model_dir(&self.dir)
    }

    /// The model's id and vector length (known without loading it).
    fn spec(&self) -> (String, usize) {
        match &self.injected {
            Some(m) => (m.model_id().to_string(), m.dim()),
            None => (lightcraft_vision::siglip::MODEL_ID.to_string(), lightcraft_vision::siglip::DIM),
        }
    }

    fn touch(&self) {
        *lock(&self.used) = Some(Instant::now());
    }

    /// The model, loaded first if needed (seconds; others wait for the one load).
    fn model(&self) -> Result<Arc<dyn Embedder>, String> {
        if let Some(m) = &self.injected {
            return Ok(m.clone());
        }
        let mut slot = lock(&self.model);
        if let Some(m) = slot.as_ref() {
            self.touch();
            return Ok(m.clone());
        }
        if !self.installed() {
            return Err(format!(
                "the search model is not installed on this server (run `lightcraft-server model download --accept-licences`, or put model.safetensors and tokenizer.json in {})",
                self.dir.display()
            ));
        }
        let started = Instant::now();
        log::info!("loading the search model from {}", self.dir.display());
        let loaded = guard::catch("loading the search model", || lightcraft_vision::siglip::SigLip::load(&self.dir))?.map_err(|e| e.to_string())?;
        let m: Arc<dyn Embedder> = Arc::new(loaded);
        *slot = Some(m.clone());
        self.touch();
        log::info!("the search model is loaded ({:.1} s)", started.elapsed().as_secs_f64());
        Ok(m)
    }

    /// A user's index, opened if needed. A file written for another model or damaged is derived
    /// data: it is replaced.
    fn index(&self, data: &Path, user: &str) -> Result<Arc<Mutex<EmbeddingIndex>>, String> {
        let mut all = lock(&self.indexes);
        if let Some(ix) = all.get(user) {
            return Ok(ix.clone());
        }
        let (model, dim) = self.spec();
        let dir = accounts::user_dir(data, user).join("search");
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let path = dir.join(format!("{model}.bin"));
        let ix = match EmbeddingIndex::open(&path, &model, dim) {
            Ok(ix) => ix,
            Err(VisionError::Mismatch { .. } | VisionError::Format(_)) => {
                log::info!("{}: the search index is for another model or damaged: starting over", path.display());
                let _ = std::fs::remove_file(&path);
                EmbeddingIndex::open(&path, &model, dim).map_err(|e| format!("{}: {e}", path.display()))?
            }
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        let ix = Arc::new(Mutex::new(ix));
        all.insert(user.to_string(), ix.clone());
        Ok(ix)
    }

    fn unload_if_idle(&self) {
        let idle = lock(&self.used).is_some_and(|t| t.elapsed() >= IDLE_UNLOAD);
        if idle {
            // (a request holding an index keeps it until it is done)
            if lock(&self.model).take().is_some() {
                log::info!("the search model was unloaded after {} minutes without use", IDLE_UNLOAD.as_secs() / 60);
            }
            lock(&self.indexes).clear();
            if lock(&self.reader).take().is_some() {
                log::info!("the text-reading models were unloaded after {} minutes without use", IDLE_UNLOAD.as_secs() / 60);
            }
            lock(&self.texts).clear();
            if lock(&self.finder).take().is_some() {
                log::info!("the face models were unloaded after {} minutes without use", IDLE_UNLOAD.as_secs() / 60);
            }
            lock(&self.faces).clear();
            *lock(&self.used) = None;
        }
    }
}

/// Distinct photos of `lib` that have a content hash, by key.
fn hashed(lib: &UserLib) -> Vec<(Key, Arc<Photo>)> {
    let mut seen = HashSet::new();
    lib.core
        .catalog()
        .photos()
        .filter(|p| p.in_library())
        .filter_map(|p| Some((Key::from_hex(p.content_hash.as_deref()?)?, p.clone())))
        .filter(|(k, _)| seen.insert(*k))
        .collect()
}

/// The worker thread: finds photos that have a mini preview and no vector, embeds them, and
/// unloads the model when it has been idle.
pub(crate) fn run(st: &Arc<State>) {
    let v = &st.vision;
    while !v.stop.load(Ordering::SeqCst) {
        {
            let mut w = lock(&v.wanted);
            if !*w {
                w = v.wake.wait_timeout(w, TICK).unwrap_or_else(PoisonError::into_inner).0;
            }
            *w = false;
        }
        if v.stop.load(Ordering::SeqCst) {
            break;
        }
        let report = |key: String, what: &str, r: Result<(), String>| match r {
            Ok(()) => {
                lock(&v.problems).remove(&key);
            }
            Err(e) => {
                if lock(&v.problems).insert(key.clone(), e.clone()).as_deref() != Some(e.as_str()) {
                    log::warn!("{what} of {key}: {e}");
                }
            }
        };
        for user in accounts::read_users(&st.data).map(|f| f.users.into_keys().collect::<Vec<_>>()).unwrap_or_default() {
            if v.installed() {
                report(user.clone(), "search index", index_user(st, &user));
            }
            if v.faces_installed()
                && !v.stop.load(Ordering::SeqCst)
                && accounts::read_users(&st.data).is_ok_and(|f| f.users.get(&user).is_some_and(|u| u.faces))
            {
                match scan_user(st, &user) {
                    Ok(more) => {
                        lock(&v.problems).remove(&format!("{user}:faces"));
                        if more {
                            v.wake();
                        }
                    }
                    Err(e) => report(format!("{user}:faces"), "face index", Err(e)),
                }
            }
            if v.text_installed() && !v.stop.load(Ordering::SeqCst) {
                match read_user(st, &user) {
                    // (more to read: come back after looking at new photos)
                    Ok(more) => {
                        lock(&v.problems).remove(&format!("{user}:text"));
                        if more {
                            v.wake();
                        }
                    }
                    Err(e) => report(format!("{user}:text"), "text index", Err(e)),
                }
            }
        }
        v.unload_if_idle();
    }
}

/// Looks for the faces in up to [`READ_CHUNK`] of a user's photos that have a smart preview and
/// haven't been looked at. True when there are more.
fn scan_user(st: &Arc<State>, user: &str) -> Result<bool, String> {
    let v = &st.vision;
    let lib = api::lib(st, user)?;
    let (photos, blobs) = {
        let l = lock(&lib);
        (hashed(&l), l.dir.join("blobs"))
    };
    if photos.is_empty() {
        return Ok(false);
    }
    let index = v.face_index(&st.data, user, true)?.ok_or("the face index is unavailable")?;
    let failed = lock(&v.failed_faces).clone();
    let mut work: Vec<(Key, Arc<Photo>, PathBuf)> = Vec::new();
    {
        let ix = lock(&index);
        for (key, photo) in photos {
            if ix.scanned(&key) || failed.contains(&(user.to_string(), key)) {
                continue;
            }
            let Some(path) = photo.content_hash.as_deref().and_then(|h| api::blob_path(&blobs, "smart", h)).filter(|p| p.is_file()) else { continue };
            work.push((key, photo, path));
        }
    }
    if work.is_empty() {
        return Ok(false);
    }
    let more = work.len() > READ_CHUNK;
    work.truncate(READ_CHUNK);
    let finder = v.finder()?;
    let started = Instant::now();
    let (mut done, mut failures) = (0, 0);
    for (key, photo, path) in &work {
        if v.stop.load(Ordering::SeqCst) {
            return Ok(false);
        }
        let found = lightcraft_engine::vision::neutral_from_preview(photo, path, TEXT_EDGE)
            .and_then(|img| guard::catch("finding faces", || finder.faces(&img)).and_then(|t| t.map_err(|e| e.to_string())));
        match found {
            Ok(faces) => {
                failures = 0;
                if lock(&index).insert_photo(*key, &faces).is_ok() {
                    done += 1;
                } else {
                    lock(&v.failed_faces).insert((user.to_string(), *key));
                }
            }
            Err(e) => {
                log::debug!("{user}: can't look for faces in {}: {e}", path.display());
                lock(&v.failed_faces).insert((user.to_string(), *key));
                failures += 1;
                if failures >= 8 {
                    // models that fail this often will keep failing
                    return Err(format!("finding faces keeps failing: {e}"));
                }
            }
        }
        v.touch();
    }
    log::info!("{user}: faces found in {done} of {} photo(s) in {:.1} s", work.len(), started.elapsed().as_secs_f64());
    lightcraft_engine::memory::release();
    Ok(more)
}

/// Reads the text of up to [`READ_CHUNK`] of a user's photos that have a smart preview and no
/// text yet. True when there are more.
fn read_user(st: &Arc<State>, user: &str) -> Result<bool, String> {
    let v = &st.vision;
    let lib = api::lib(st, user)?;
    let (photos, blobs) = {
        let l = lock(&lib);
        (hashed(&l), l.dir.join("blobs"))
    };
    if photos.is_empty() {
        return Ok(false);
    }
    let index = v.text_index(&st.data, user, true)?.ok_or("the text index is unavailable")?;
    let have: HashSet<Key> = lock(&index).keys().copied().collect();
    let failed = lock(&v.failed_text).clone();
    // (only a smart preview has the pixels small print needs)
    let mut work: Vec<(Key, Arc<Photo>, PathBuf)> = Vec::new();
    for (key, photo) in photos {
        if have.contains(&key) || failed.contains(&(user.to_string(), key)) {
            continue;
        }
        let Some(path) = photo.content_hash.as_deref().and_then(|h| api::blob_path(&blobs, "smart", h)).filter(|p| p.is_file()) else { continue };
        work.push((key, photo, path));
    }
    if work.is_empty() {
        return Ok(false);
    }
    let more = work.len() > READ_CHUNK;
    work.truncate(READ_CHUNK);
    let reader = v.reader()?;
    let started = Instant::now();
    let (mut done, mut failures) = (0, 0);
    for (key, photo, path) in &work {
        if v.stop.load(Ordering::SeqCst) {
            return Ok(false);
        }
        let read = lightcraft_engine::vision::neutral_from_preview(photo, path, TEXT_EDGE)
            .and_then(|img| guard::catch("reading text", || reader.text(&img)).and_then(|t| t.map_err(|e| e.to_string())));
        match read {
            Ok(text) => {
                failures = 0;
                if lock(&index).insert(*key, &text).is_ok() {
                    done += 1;
                } else {
                    lock(&v.failed_text).insert((user.to_string(), *key));
                }
            }
            Err(e) => {
                log::debug!("{user}: can't read the text of {}: {e}", path.display());
                lock(&v.failed_text).insert((user.to_string(), *key));
                failures += 1;
                if failures >= 8 {
                    // a reader that fails this often will keep failing
                    return Err(format!("reading text keeps failing: {e}"));
                }
            }
        }
        v.touch();
    }
    log::info!("{user}: text read from {done} of {} photo(s) in {:.1} s", work.len(), started.elapsed().as_secs_f64());
    lightcraft_engine::memory::release();
    Ok(more)
}

/// Embeds a user's photos that have a mini preview and no vector yet.
fn index_user(st: &Arc<State>, user: &str) -> Result<(), String> {
    let v = &st.vision;
    let lib = api::lib(st, user)?;
    let (photos, blobs) = {
        let l = lock(&lib);
        (hashed(&l), l.dir.join("blobs"))
    };
    if photos.is_empty() {
        return Ok(());
    }
    let index = v.index(&st.data, user)?;
    let mut have: HashSet<Key> = lock(&index).keys().copied().collect();
    let failed = lock(&v.failed).clone();
    let mut work: Vec<(Key, Arc<Photo>, PathBuf)> = Vec::new();
    for (key, photo) in photos {
        if have.contains(&key) || failed.contains(&(user.to_string(), key)) {
            continue;
        }
        let Some(path) = photo.content_hash.as_deref().and_then(|h| api::blob_path(&blobs, "mini", h)).filter(|p| p.is_file()) else { continue };
        have.insert(key);
        work.push((key, photo, path));
    }
    if work.is_empty() {
        return Ok(());
    }
    log::info!("{user}: {} photo(s) to index for search", work.len());
    let model = v.model()?;
    let started = Instant::now();
    let total = work.len();
    let mut done = 0;
    for chunk in work.chunks(BATCH) {
        if v.stop.load(Ordering::SeqCst) {
            return Ok(());
        }
        let (mut keys, mut images) = (Vec::new(), Vec::new());
        for (key, photo, path) in chunk {
            match lightcraft_engine::vision::neutral_from_preview(photo, path, THUMB_EDGE) {
                Ok(img) => {
                    keys.push(*key);
                    images.push(img);
                }
                Err(e) => {
                    log::debug!("{user}: can't render {} for search: {e}", path.display());
                    lock(&v.failed).insert((user.to_string(), *key));
                }
            }
        }
        if images.is_empty() {
            continue;
        }
        let refs: Vec<&_> = images.iter().collect();
        let vectors = guard::catch("embedding photos", || model.encode_images(&refs))?.map_err(|e| e.to_string())?;
        let mut ix = lock(&index);
        for (k, vec) in keys.iter().zip(&vectors) {
            if ix.insert(*k, vec).is_err() {
                lock(&v.failed).insert((user.to_string(), *k));
            }
        }
        done += keys.len();
        v.touch();
    }
    log::info!("{user}: {done} of {total} photo(s) indexed for search in {:.1} s", started.elapsed().as_secs_f64());
    lightcraft_engine::memory::release();
    Ok(())
}

/// `%xx` and `+` decoded (a query string value).
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while let Some(&c) = b.get(i) {
        let hex = |at: usize| b.get(at).and_then(|d| (*d as char).to_digit(16));
        if c == b'+' {
            out.push(b' ');
        } else if let (b'%', Some(h), Some(l)) = (c, hex(i + 1), hex(i + 2)) {
            out.push((h * 16 + l) as u8);
            i += 2;
        } else {
            out.push(c);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn unavailable(msg: String) -> Resp {
    api::json(503, &json!({"error": msg, "installed": false}))
}

/// `GET /api/search/status`
pub(crate) fn status(st: &State, l: &Mutex<UserLib>, user: &str) -> Resp {
    let v = &st.vision;
    let (model, dim) = v.spec();
    let total = hashed(&lock(l)).len();
    let indexed = v.index(&st.data, user).map(|ix| lock(&ix).len()).unwrap_or(0);
    let read = v.text_index(&st.data, user, false).ok().flatten().map_or(0, |ix| lock(&ix).len());
    api::json(
        200,
        &json!({
            "available": true,
            "installed": v.installed(),
            "model": model,
            "dim": dim,
            "indexed": indexed,
            "total": total,
            "problem": lock(&v.problems).get(user),
            "text": {
                "installed": v.text_installed(),
                "engine": v.engine(),
                "indexed": read,
                "problem": lock(&v.problems).get(&format!("{user}:text")),
            },
            // (finding people: whether an admin turned it on for this user, and how far it is)
            "faces": {
                "enabled": accounts::read_users(&st.data).is_ok_and(|f| f.users.get(user).is_some_and(|u| u.faces)),
                "installed": v.faces_installed(),
                "engine": v.faces_engine(),
                "photos": v.face_index(&st.data, user, false).ok().flatten().map_or(0, |ix| lock(&ix).photos()),
            },
        }),
    )
}

/// `GET /api/search?q=…&limit=…`
pub(crate) fn search(st: &State, l: &Mutex<UserLib>, user: &str, q: &str) -> Resp {
    let v = &st.vision;
    let query: String = api::query(q, "q").map(percent_decode).unwrap_or_default().trim().chars().take(MAX_QUERY).collect();
    if query.is_empty() {
        return error(400, "give the description to search for as ?q=…");
    }
    let limit = api::query(q, "limit").and_then(|n| n.parse::<usize>().ok()).unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let index = match v.index(&st.data, user) {
        Ok(i) => i,
        Err(e) => return error(500, e),
    };
    let indexed = lock(&index).len();
    let total = hashed(&lock(l)).len();
    // the photos with the query's words in them come first, then the ones that look like it
    let words = match v.text_index(&st.data, user, false) {
        Ok(Some(ix)) => lock(&ix).search(&query, limit),
        Ok(None) => Vec::new(),
        Err(e) => return error(500, e),
    };
    let looks = match v.model() {
        Ok(model) => match guard::catch("a search", || -> Result<_, String> {
            let vector = model.encode_text(&query).map_err(|e| e.to_string())?;
            v.touch();
            lock(&index).search(&vector, limit).map_err(|e| e.to_string())
        }) {
            Ok(Ok(h)) => h,
            Ok(Err(e)) | Err(e) => return error(500, e),
        },
        // no description model: the words alone still answer
        Err(_) if !words.is_empty() || v.text_installed() => Vec::new(),
        Err(e) => return unavailable(e),
    };
    let hits = lightcraft_vision::textindex::merge(&words, looks, limit);
    // photos by content: every photo (virtual copies too) with a hit's hash
    let by_key: HashMap<Key, Vec<u64>> = {
        let mut m: HashMap<Key, Vec<u64>> = HashMap::new();
        for p in lock(l).core.catalog().photos().filter(|p| p.in_library()) {
            if let Some(k) = p.content_hash.as_deref().and_then(Key::from_hex) {
                m.entry(k).or_default().push(p.id.0);
            }
        }
        m
    };
    let (mut ids, mut scores) = (Vec::new(), Vec::new());
    for h in &hits {
        for id in by_key.get(&h.key).into_iter().flatten() {
            ids.push(*id);
            scores.push((f64::from(h.score) * 1e4).round() / 1e4);
        }
    }
    api::json(200, &json!({"query": query, "ids": ids, "scores": scores, "indexed": indexed, "total": total}))
}

/// `GET /api/index/embeddings/keys`
pub(crate) fn keys(st: &State, user: &str) -> Resp {
    let v = &st.vision;
    let (model, dim) = v.spec();
    let index = match v.index(&st.data, user) {
        Ok(i) => i,
        Err(e) => return error(500, e),
    };
    // (JSON: the sync transport carries text; 100k photos are ~3.5 MB)
    let keys: Vec<String> = lock(&index).keys().map(Key::to_hex).collect();
    api::json(200, &json!({"model": model, "dim": dim, "keys": keys}))
}

/// `POST /api/index/embeddings`: vectors a device computed. Only photos in the user's library are
/// kept; the whole upload is refused when it is for another model or damaged.
pub(crate) fn upload(st: &State, req: &mut Request, l: &Mutex<UserLib>, user: &str) -> Resp {
    let v = &st.vision;
    let body = match api::read_body(req, UPLOAD_MAX) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let index = match v.index(&st.data, user) {
        Ok(i) => i,
        Err(e) => return error(500, e),
    };
    let mine: HashSet<Key> = hashed(&lock(l)).into_iter().map(|(k, _)| k).collect();
    let r = lock(&index).import(&body, |k| mine.contains(k));
    match r {
        Ok(done) => {
            v.touch();
            api::json(200, &json!({"added": done.added, "skipped": done.skipped}))
        }
        Err(e @ VisionError::Mismatch { .. }) => error(409, e),
        Err(e) => error(422, e),
    }
}

/// `GET /api/index/text/keys`
pub(crate) fn text_keys(st: &State, user: &str) -> Resp {
    let v = &st.vision;
    let keys: Vec<String> = match v.text_index(&st.data, user, false) {
        Ok(Some(ix)) => lock(&ix).keys().map(Key::to_hex).collect(),
        Ok(None) => Vec::new(),
        Err(e) => return error(500, e),
    };
    api::json(200, &json!({"engine": v.engine(), "keys": keys}))
}

/// `POST /api/index/text`: text a device read. Only photos in the user's library are kept; the
/// whole upload is refused when it is from another reader or damaged.
pub(crate) fn text_upload(st: &State, req: &mut Request, l: &Mutex<UserLib>, user: &str) -> Resp {
    let v = &st.vision;
    let body = match api::read_body(req, UPLOAD_MAX) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let index = match v.text_index(&st.data, user, true) {
        Ok(Some(i)) => i,
        Ok(None) => return error(500, "the text index is unavailable"),
        Err(e) => return error(500, e),
    };
    let mine: HashSet<Key> = hashed(&lock(l)).into_iter().map(|(k, _)| k).collect();
    let r = lock(&index).import(&body, |k| mine.contains(k));
    match r {
        Ok(done) => {
            v.touch();
            api::json(200, &json!({"added": done.added, "skipped": done.skipped}))
        }
        Err(e @ VisionError::Mismatch { .. }) => error(409, e),
        Err(e) => error(422, e),
    }
}

/// Why finding people is not available to `user`, or `None` when it is (an admin turned it on).
fn faces_refused(st: &State, user: &str) -> Option<Resp> {
    let on = accounts::read_users(&st.data).is_ok_and(|f| f.users.get(user).is_some_and(|u| u.faces));
    (!on).then(|| {
        api::json(
            403,
            &json!({"error": "finding people is off for this user: an admin turns it on with `lightcraft-server user faces NAME on`", "enabled": false}),
        )
    })
}

/// `GET /api/people/status`
pub(crate) fn people_status(st: &State, l: &Mutex<UserLib>, user: &str) -> Resp {
    let v = &st.vision;
    let enabled = accounts::read_users(&st.data).is_ok_and(|f| f.users.get(user).is_some_and(|u| u.faces));
    let (faces, photos) = v.face_index(&st.data, user, false).ok().flatten().map_or((0, 0), |ix| {
        let ix = lock(&ix);
        (ix.len(), ix.photos())
    });
    api::json(
        200,
        &json!({
            "available": true,
            "enabled": enabled,
            "installed": v.faces_installed(),
            "engine": v.faces_engine(),
            "faces": faces,
            "photos": photos,
            "total": hashed(&lock(l)).len(),
            "problem": lock(&v.problems).get(&format!("{user}:faces")),
        }),
    )
}

/// `GET /api/people/clusters`: the people the faces make, for a device to name and show.
pub(crate) fn people_clusters(st: &State, l: &Mutex<UserLib>, user: &str) -> Resp {
    if let Some(r) = faces_refused(st, user) {
        return r;
    }
    let v = &st.vision;
    let index = match v.face_index(&st.data, user, false) {
        Ok(Some(i)) => i,
        Ok(None) => return api::json(200, &json!({"engine": v.faces_engine(), "faces": 0, "photos": 0, "clusters": []})),
        Err(e) => return error(500, e),
    };
    let at = {
        let ix = lock(&index);
        (ix.len(), ix.photos(), ix.revision())
    };
    if let Some((k, body)) = lock(&v.people).get(user)
        && *k == at
    {
        return api::raw_json(200, body.as_str().to_string());
    }
    // a face by the first photo that has its content (virtual copies share it)
    let mut photo_of: HashMap<Key, u64> = HashMap::new();
    for p in lock(l).core.catalog().photos().filter(|p| p.in_library()) {
        if let Some(k) = p.content_hash.as_deref().and_then(Key::from_hex) {
            photo_of.entry(k).and_modify(|id| *id = (*id).min(p.id.0)).or_insert(p.id.0);
        }
    }
    let clusters: Vec<serde_json::Value> = lightcraft_engine::vision::clusters_of(&lock(&index))
        .iter()
        .filter_map(|c| {
            let members: Vec<serde_json::Value> = c
                .members
                .iter()
                .filter_map(|m| {
                    let r = |x: f32| (f64::from(x) * 1e5).round() / 1e5;
                    photo_of.get(&m.key).map(|id| json!([id, m.index, r(m.rect[0]), r(m.rect[1]), r(m.rect[2]), r(m.rect[3]), r(m.score)]))
                })
                .collect();
            (!members.is_empty()).then(|| json!({"id": c.id, "members": members}))
        })
        .collect();
    let body = json!({"engine": v.faces_engine(), "faces": at.0, "photos": at.1, "clusters": clusters}).to_string();
    lock(&v.people).insert(user.to_string(), (at, Arc::new(body.clone())));
    api::raw_json(200, body)
}

/// `GET /api/index/faces/keys`
pub(crate) fn face_keys(st: &State, user: &str) -> Resp {
    if let Some(r) = faces_refused(st, user) {
        return r;
    }
    let v = &st.vision;
    let keys: Vec<String> = match v.face_index(&st.data, user, false) {
        Ok(Some(ix)) => lock(&ix).keys().map(Key::to_hex).collect(),
        Ok(None) => Vec::new(),
        Err(e) => return error(500, e),
    };
    api::json(200, &json!({"engine": v.faces_engine(), "keys": keys}))
}

/// `POST /api/index/faces`: faces a device found. Only photos in the user's library are kept; the
/// whole upload is refused when it is from other models or damaged.
pub(crate) fn face_upload(st: &State, req: &mut Request, l: &Mutex<UserLib>, user: &str) -> Resp {
    if let Some(r) = faces_refused(st, user) {
        return r;
    }
    let v = &st.vision;
    let body = match api::read_body(req, UPLOAD_MAX) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let index = match v.face_index(&st.data, user, true) {
        Ok(Some(i)) => i,
        Ok(None) => return error(500, "the face index is unavailable"),
        Err(e) => return error(500, e),
    };
    let mine: HashSet<Key> = hashed(&lock(l)).into_iter().map(|(k, _)| k).collect();
    let r = lock(&index).import(&body, |k| mine.contains(k));
    match r {
        Ok(done) => {
            v.touch();
            api::json(200, &json!({"added": done.added, "skipped": done.skipped}))
        }
        Err(e @ VisionError::Mismatch { .. }) => error(409, e),
        Err(e) => error(422, e),
    }
}

/// `DELETE /api/index/faces`: forget every face (whether or not finding people is on).
pub(crate) fn face_delete(st: &State, user: &str) -> Resp {
    let v = &st.vision;
    let (faces, photos) = match v.face_index(&st.data, user, false) {
        Ok(Some(ix)) => {
            let mut ix = lock(&ix);
            let counts = (ix.len(), ix.photos());
            if let Err(e) = ix.clear() {
                return error(500, e);
            }
            counts
        }
        Ok(None) => (0, 0),
        Err(e) => return error(500, e),
    };
    lock(&v.people).remove(user);
    lock(&v.failed_faces).retain(|(u, _)| u != user);
    api::json(200, &json!({"faces": faces, "photos": photos}))
}

/// `lightcraft-server model download --faces`: fetches the face models into `<dir>/faces`.
pub fn download_face_models(dir: &Path, mirrors_file: Option<&Path>) -> Result<(), String> {
    use lightcraft_vision::models::{self, Options, Progress};
    let env = std::env::var(models::MIRRORS_ENV).ok();
    let cancel = AtomicBool::new(false);
    let mut last = Instant::now() - Duration::from_secs(10);
    let mut report = |p: &Progress| {
        if last.elapsed() >= Duration::from_secs(2) {
            last = Instant::now();
            eprintln!("{}: {:.1} / {:.1} MB", p.file, p.done as f64 / 1e6, p.total as f64 / 1e6);
        }
    };
    for (files, mirrors, to) in models::face_downloads(&dir.join(lightcraft_engine::vision::FACES_DIR), env.as_deref(), mirrors_file) {
        models::download(files, &mirrors, &to, &Options::default(), &cancel, &mut report).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// What `lightcraft-server model status` says.
pub fn model_status(dir: &Path) -> String {
    let (name, url) = lightcraft_vision::models::SIGLIP_LICENCE;
    let (tname, turl) = lightcraft_vision::models::OCR_LICENCE;
    let text = dir.join(lightcraft_engine::vision::TEXT_DIR);
    let faces = dir.join(lightcraft_engine::vision::FACES_DIR);
    format!(
        "search model (SigLIP 2): {} in {}\nlicence: {name}, {url}\ndownload: {:.1} GB\n\nface models (YuNet, SFace): {} in {}\nlicences: {}, {}\ndownload: {:.0} MB (add --faces; then `user faces NAME on` for each user whose people the server may find)\n\ntext-reading models (PP-OCRv6): {} in {}\nlicence: {tname}, {turl}\ndownload: {:.0} MB (add --text)",
        if lightcraft_vision::siglip::is_model_dir(dir) { "installed" } else { "not installed" },
        dir.display(),
        (lightcraft_vision::models::SIGLIP_WEIGHTS_SIZE as f64 + 34_363_039.0) / 1e9,
        if lightcraft_vision::faces::model::is_model_dir(&faces) { "installed" } else { "not installed" },
        faces.display(),
        lightcraft_vision::models::FACE_LICENCES[0].0,
        lightcraft_vision::models::FACE_LICENCES[1].0,
        lightcraft_vision::models::FACE_BYTES as f64 / 1e6,
        if lightcraft_vision::ocr::is_model_dir(&text) { "installed" } else { "not installed" },
        text.display(),
        lightcraft_vision::models::OCR_BYTES as f64 / 1e6
    )
}

/// `lightcraft-server model download --text`: fetches the text-reading models into `<dir>/ocr`.
pub fn download_text_models(dir: &Path, mirrors_file: Option<&Path>) -> Result<(), String> {
    use lightcraft_vision::models::{self, Options, Progress};
    let env = std::env::var(models::MIRRORS_ENV).ok();
    let cancel = AtomicBool::new(false);
    let mut last = Instant::now() - Duration::from_secs(10);
    let mut report = |p: &Progress| {
        if last.elapsed() >= Duration::from_secs(2) {
            last = Instant::now();
            eprintln!("{}: {:.1} / {:.1} MB", p.file, p.done as f64 / 1e6, p.total as f64 / 1e6);
        }
    };
    for (files, mirrors, to) in models::ocr_downloads(&dir.join(lightcraft_engine::vision::TEXT_DIR), env.as_deref(), mirrors_file) {
        models::download(files, &mirrors, &to, &Options::default(), &cancel, &mut report).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// `lightcraft-server model download`: fetches the model into `dir` (resuming, verified).
pub fn download_model(dir: &Path, mirrors_file: Option<&Path>) -> Result<(), String> {
    use lightcraft_vision::models::{self, Options, Progress};
    let env = std::env::var(models::MIRRORS_ENV).ok();
    let mirrors = models::mirrors(env.as_deref(), mirrors_file);
    let cancel = AtomicBool::new(false);
    let mut last = Instant::now() - Duration::from_secs(10);
    let mut report = |p: &Progress| {
        if last.elapsed() >= Duration::from_secs(2) {
            last = Instant::now();
            eprintln!("{}: {:.2} / {:.2} GB", p.file, p.done as f64 / 1e9, p.total as f64 / 1e9);
        }
    };
    models::download(models::SIGLIP_FILES, &mirrors, dir, &Options::default(), &cancel, &mut report).map_err(|e| e.to_string())
}
