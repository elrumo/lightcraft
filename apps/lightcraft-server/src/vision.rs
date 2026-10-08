//! Search by description on the server: it turns each user's photos into vectors from the previews
//! it already keeps, answers `GET /api/search?q=…` with the best matches, and takes vectors that a
//! device computed (a desktop with a GPU is far quicker than the server's CPU) so nothing is done
//! twice. Clients (the web build, iOS, other desktops) send a sentence and get photo ids back; none
//! of them needs the model.
//!
//! ```text
//! GET  /api/search/status            {available, installed, model, dim, indexed, total}
//! GET  /api/search?q=…&limit=…       {query, ids: [photo id], scores: [f32], indexed, total}
//! GET  /api/index/embeddings/keys    {model, dim, keys: [content hash]} the server already has
//! POST /api/index/embeddings         records a device computed (see `lightcraft_vision::EmbeddingIndex::export`)
//! ```
//!
//! Vectors are filed by content hash, from a neutral (unedited) rendering of the mini preview, in
//! `<data>/users/<name>/search/<model>.bin`: derived data, safe to delete. The model is not part
//! of LightCraft: an admin installs it (`lightcraft-server model download --accept-licences`)
//! and it loads on first use and unloads when idle (~1.5 GB while loaded). Until then every route
//! says so instead of failing.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use lightcraft_catalog::Photo;
use lightcraft_engine::guard;
use lightcraft_vision::{Embedder, EmbeddingIndex, Error as VisionError, Key};
use serde_json::json;
use tiny_http::Request;

use crate::State;
use crate::accounts;
use crate::api::{self, Resp, UserLib, error};

/// Long edge of the rendering a photo is embedded from.
const THUMB_EDGE: usize = 512;
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
    model: Mutex<Option<Arc<dyn Embedder>>>,
    indexes: Mutex<HashMap<String, Arc<Mutex<EmbeddingIndex>>>>,
    used: Mutex<Option<Instant>>,
    wanted: Mutex<bool>,
    wake: Condvar,
    stop: AtomicBool,
    /// Photos that could not be rendered or embedded (not retried until the server restarts).
    failed: Mutex<HashSet<(String, Key)>>,
    /// The last problem indexing a user's photos.
    problems: Mutex<HashMap<String, String>>,
}

impl Search {
    pub fn new(dir: PathBuf, injected: Option<Arc<dyn Embedder>>) -> Search {
        Search {
            dir,
            injected,
            model: Mutex::new(None),
            indexes: Mutex::new(HashMap::new()),
            used: Mutex::new(None),
            wanted: Mutex::new(false),
            wake: Condvar::new(),
            stop: AtomicBool::new(false),
            failed: Mutex::new(HashSet::new()),
            problems: Mutex::new(HashMap::new()),
        }
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
        if v.installed() {
            for user in accounts::read_users(&st.data).map(|f| f.users.into_keys().collect::<Vec<_>>()).unwrap_or_default() {
                match index_user(st, &user) {
                    Ok(()) => {
                        lock(&v.problems).remove(&user);
                    }
                    Err(e) => {
                        if lock(&v.problems).insert(user.clone(), e.clone()).as_deref() != Some(e.as_str()) {
                            log::warn!("search index of {user}: {e}");
                        }
                    }
                }
            }
        }
        v.unload_if_idle();
    }
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
    let model = match v.model() {
        Ok(m) => m,
        Err(e) => return unavailable(e),
    };
    let hits = match guard::catch("a search", || -> Result<_, String> {
        let vector = model.encode_text(&query).map_err(|e| e.to_string())?;
        v.touch();
        lock(&index).search(&vector, limit).map_err(|e| e.to_string())
    }) {
        Ok(Ok(h)) => h,
        Ok(Err(e)) | Err(e) => return error(500, e),
    };
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

/// What `lightcraft-server model status` says.
pub fn model_status(dir: &Path) -> String {
    let (name, url) = lightcraft_vision::models::SIGLIP_LICENCE;
    format!(
        "search model (SigLIP 2): {} in {}\nlicence: {name}, {url}\ndownload: {:.1} GB",
        if lightcraft_vision::siglip::is_model_dir(dir) { "installed" } else { "not installed" },
        dir.display(),
        (lightcraft_vision::models::SIGLIP_WEIGHTS_SIZE as f64 + 34_363_039.0) / 1e9
    )
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
