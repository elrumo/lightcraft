//! Checking each user's photos for sensitive content (nudity and the like) from the smart previews the
//! server keeps, so a device that can't run the model (iOS, the web) can still list and blur them.
//!
//! Off until an admin downloads the model on the admin page (**Processing ▸ Sensitive content**) or with
//! `lightcraft-server model download --accept-licences --sensitive`; the download turns it on, and
//! `model disable --sensitive` (or the page's switch) turns it off, keeping the scores. The worker scores
//! [`CHUNK`] photos of a user at a time, after the search, the faces and the text.
//!
//! ```text
//! GET /api/sensitive    {engine, min, keys: [[content hash, score]], checked, total}   503 until installed
//! ```

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use lightcraft_catalog::Photo;
use lightcraft_engine::guard;
use lightcraft_vision::{Classifier, Error as VisionError, Key, SensitiveIndex};
use serde_json::json;

use crate::State;
use crate::accounts;
use crate::api::{self, Resp, UserLib, error};
use crate::http::Request;
use crate::switch::Part;

/// Photos checked in one pass over a user.
const CHUNK: usize = 25;
/// Largest upload of scores (a device sends them in pieces).
const UPLOAD_MAX: u64 = 16 << 20;
/// Long edge of the rendering a photo is checked from (the model reads 224×224).
const EDGE: usize = lightcraft_engine::vision::SENSITIVE_EDGE;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The classifier and the users' scores.
pub struct Sensitive {
    /// Where the model's file is (`<models>/nsfw`).
    dir: PathBuf,
    /// A classifier supplied by the host (tests) instead of the model from `dir`.
    injected: Option<Arc<dyn Classifier>>,
    classifier: Mutex<Option<Arc<dyn Classifier>>>,
    indexes: Mutex<HashMap<String, Arc<Mutex<SensitiveIndex>>>>,
    /// Photos that could not be checked (not retried until the server restarts).
    failed: Mutex<HashSet<(String, Key)>>,
}

impl Sensitive {
    pub fn new(models: &Path) -> Sensitive {
        Sensitive {
            dir: models.join(lightcraft_engine::vision::SENSITIVE_DIR),
            injected: None,
            classifier: Mutex::new(None),
            indexes: Mutex::new(HashMap::new()),
            failed: Mutex::new(HashSet::new()),
        }
    }

    /// Checks photos with `classifier` instead of the model from the model folder (tests).
    pub fn with_classifier(mut self, classifier: Option<Arc<dyn Classifier>>) -> Sensitive {
        self.injected = classifier;
        self
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Whether the model's file is in place (whether or not an admin has checking on).
    pub fn files_installed(&self) -> bool {
        self.injected.is_some() || lightcraft_vision::nsfw::is_model_dir(&self.dir)
    }

    /// The classifier's engine id (known without loading it).
    pub fn engine(&self) -> String {
        match &self.injected {
            Some(c) => c.engine().to_string(),
            None => lightcraft_vision::nsfw::ENGINE.to_string(),
        }
    }

    /// The classifier, loaded first if needed.
    fn classifier(&self) -> Result<Arc<dyn Classifier>, String> {
        if let Some(c) = &self.injected {
            return Ok(c.clone());
        }
        let mut slot = lock(&self.classifier);
        if let Some(c) = slot.as_ref() {
            return Ok(c.clone());
        }
        if !self.files_installed() {
            return Err(format!(
                "the sensitive-content model is not installed on this server (run `lightcraft-server model download --accept-licences --sensitive`, or put it in {})",
                self.dir.display()
            ));
        }
        log::info!("loading the sensitive-content model from {}", self.dir.display());
        let loaded =
            guard::catch("loading the sensitive-content model", || lightcraft_vision::nsfw::Nsfw::load(&self.dir))?.map_err(|e| e.to_string())?;
        let c: Arc<dyn Classifier> = Arc::new(loaded);
        *slot = Some(c.clone());
        Ok(c)
    }

    /// A user's scores, opened if needed (another model's, or damaged, are derived data: replaced). Without
    /// `create`, a user who has none yet gets `None` and no file is made.
    pub(crate) fn index(&self, data: &Path, user: &str, create: bool) -> Result<Option<Arc<Mutex<SensitiveIndex>>>, String> {
        let mut all = lock(&self.indexes);
        if let Some(ix) = all.get(user) {
            return Ok(Some(ix.clone()));
        }
        let engine = self.engine();
        let dir = accounts::user_dir(data, user).join("search");
        let path = dir.join(format!("sensitive-{engine}.bin"));
        if !create && !path.is_file() {
            return Ok(None);
        }
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let ix = match SensitiveIndex::open(&path, &engine) {
            Ok(ix) => ix,
            Err(VisionError::Mismatch { .. } | VisionError::Format(_)) => {
                log::info!("{}: the sensitive-content scores are another model's or damaged: starting over", path.display());
                let _ = std::fs::remove_file(&path);
                SensitiveIndex::open(&path, &engine).map_err(|e| format!("{}: {e}", path.display()))?
            }
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        let ix = Arc::new(Mutex::new(ix));
        all.insert(user.to_string(), ix.clone());
        Ok(Some(ix))
    }

    /// The photos that could not be checked, by user.
    pub(crate) fn failed(&self) -> &Mutex<HashSet<(String, Key)>> {
        &self.failed
    }

    /// Drop the model and the scores from memory (they come back on the next use).
    pub(crate) fn unload(&self) {
        if lock(&self.classifier).take().is_some() {
            log::info!("the sensitive-content model was unloaded");
        }
        lock(&self.indexes).clear();
    }
}

/// Checks up to [`CHUNK`] of a user's photos that have a smart preview and no score yet. True when there
/// are more.
pub(crate) fn check_user(st: &Arc<State>, user: &str) -> Result<bool, String> {
    let v = &st.vision;
    let s = &v.sensitive;
    let lib = api::lib(st, user)?;
    let (photos, blobs) = {
        let l = lock(&lib);
        (crate::vision::hashed(&l), l.dir.join("blobs"))
    };
    if photos.is_empty() {
        return Ok(false);
    }
    let index = s.index(&st.data, user, true)?.ok_or("the sensitive-content scores are unavailable")?;
    let failed = lock(&s.failed).clone();
    let mut work: Vec<(Key, Arc<Photo>, PathBuf)> = Vec::new();
    let mut waiting = 0;
    {
        let ix = lock(&index);
        for (key, photo) in photos {
            if ix.contains(&key) || failed.contains(&(user.to_string(), key)) || v.work.leased(user, Part::Sensitive, &key) {
                continue;
            }
            let Some(path) = photo.content_hash.as_deref().and_then(|h| api::blob_path(&blobs, "smart", h)).filter(|p| p.is_file()) else {
                waiting += 1;
                continue;
            };
            work.push((key, photo, path));
        }
    }
    v.switch.set_waiting(Part::Sensitive, user, waiting);
    if work.is_empty() {
        return Ok(false);
    }
    let more = work.len() > CHUNK;
    work.truncate(CHUNK);
    let classifier = s.classifier()?;
    let started = Instant::now();
    let (mut done, mut failures) = (0, 0);
    for (key, photo, path) in &work {
        if v.stopping() {
            return Ok(false);
        }
        let one = Instant::now();
        let scored = lightcraft_engine::vision::neutral_from_preview(photo, path, EDGE)
            .and_then(|img| guard::catch("checking a photo", || classifier.score(&img)).and_then(|t| t.map_err(|e| e.to_string())));
        match scored {
            Ok(score) => {
                failures = 0;
                if lock(&index).insert(*key, score).is_ok() {
                    done += 1;
                    v.switch.record(Part::Sensitive, user, 1, one.elapsed().as_secs_f64());
                } else {
                    lock(&s.failed).insert((user.to_string(), *key));
                }
            }
            Err(e) => {
                log::debug!("{user}: can't check {}: {e}", path.display());
                lock(&s.failed).insert((user.to_string(), *key));
                failures += 1;
                if failures >= 8 {
                    // a model that fails this often will keep failing
                    return Err(format!("checking photos keeps failing: {e}"));
                }
            }
        }
        v.touch();
    }
    log::info!("{user}: {done} of {} photo(s) checked for sensitive content in {:.1} s", work.len(), started.elapsed().as_secs_f64());
    lightcraft_engine::memory::release();
    Ok(more)
}

/// `POST /api/index/sensitive`: scores a device computed. Only photos in the user's library are kept; the
/// whole upload is refused when it is from another model or damaged.
pub(crate) fn upload(st: &State, req: &mut Request, l: &Mutex<UserLib>, user: &str) -> Resp {
    let v = &st.vision;
    if !v.switch.sensitive_enabled() {
        return api::json(403, &json!({"error": "this server doesn't check photos for sensitive content", "installed": false}));
    }
    let body = match api::read_body(req, UPLOAD_MAX) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let index = match v.sensitive.index(&st.data, user, true) {
        Ok(Some(i)) => i,
        Ok(None) => return error(500, "the sensitive-content scores are unavailable"),
        Err(e) => return error(500, e),
    };
    let mine: HashSet<Key> = crate::vision::hashed(&lock(l)).into_iter().map(|(k, _)| k).collect();
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

/// `GET /api/sensitive`: the user's photos scored at least the engine's cut-off, as content hashes.
pub(crate) fn list(st: &State, l: &Mutex<UserLib>, user: &str) -> Resp {
    let v = &st.vision;
    if !v.sensitive_installed() {
        return api::json(503, &json!({"error": "this server doesn't check photos for sensitive content", "installed": false}));
    }
    let total = crate::vision::hashed(&lock(l)).len();
    let min = lightcraft_engine::vision::SENSITIVE_MIN;
    let (checked, keys) = match v.sensitive.index(&st.data, user, false) {
        Ok(Some(ix)) => {
            let ix = lock(&ix);
            (ix.len(), ix.flagged(min).into_iter().map(|(k, s)| json!([k.to_hex(), (f64::from(s) * 1e3).round() / 1e3])).collect::<Vec<_>>())
        }
        Ok(None) => (0, Vec::new()),
        Err(e) => return error(500, e),
    };
    api::json(200, &json!({"engine": v.sensitive.engine(), "min": min, "keys": keys, "checked": checked, "total": total}))
}
