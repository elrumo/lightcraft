//! Possibly sensitive photos (nudity and the like): a classifier ([`Classifier`]; Falconsai's ViT with the
//! `vision` feature) scores each photo from a neutral rendering in the same background pass as the
//! description, the text and the faces, and the photos it scores at least [`SENSITIVE_MIN`] are listed
//! (`library.source {kind: "sensitive"}`) and can be blurred in the grid. Nothing is hidden or changed by
//! it: the person reviews the list and hides what they want (Hide All).
//!
//! It is opt-in and its own download, like the face models. The scores are derived data in the library's
//! `search/` folder; a device that can't run the model (iOS, the web) lists the photos the sync server
//! flagged (`GET /api/sensitive`).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, PoisonError};

use lightcraft_catalog::PhotoId;
use lightcraft_vision::{Classifier, Key, SensitiveIndex};
use serde_json::{Value, json};
use web_time::Instant;

use super::server::{Aux, why};
use super::{IndexJob, READ_FAILURES, Shared, Vision};
use crate::cloud_sync::{Body, Done};
use crate::{Session, guard, memory};

/// The classifier's licence (not LightCraft's): shown before downloading.
pub const SENSITIVE_LICENSE_NAME: &str = "Apache License 2.0 (Falcons.ai, nsfw_image_detection)";
pub const SENSITIVE_LICENSE_URL: &str = "https://huggingface.co/Falconsai/nsfw_image_detection";
/// Download size of the classifier.
pub const SENSITIVE_BYTES: u64 = 87_333_629;
/// How errors about a missing classifier start (the UI offers the download on it).
pub const SENSITIVE_NOT_INSTALLED: &str = "The sensitive-content model is not installed";
/// Folder of the classifier inside [`Vision::dir`].
pub const SENSITIVE_DIR: &str = "nsfw";
/// Photos scored at least this are listed as possibly sensitive.
// ponytail: one fixed cut-off, not calibrated on a real library; make it a setting if it flags too much or too little.
pub const SENSITIVE_MIN: f32 = 0.7;
/// How long the server's list is used before it is asked again.
const SERVER_TTL: web_time::Duration = web_time::Duration::from_secs(60);

/// Where the scores live and which classifier made them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ClassSpec {
    pub path: Option<PathBuf>,
    pub engine: String,
}

/// How a worker thread gets the classifier.
#[derive(Clone)]
pub(super) struct ClassifierProvider {
    injected: Option<Arc<dyn Classifier>>,
    dir: Option<PathBuf>,
}

/// What the list of possibly sensitive photos was worked out from (the library, the scores here, the server's).
pub(super) type ListKey = (u64, u64, u64);

/// The flagged photos the server listed, as of when it was asked.
#[derive(Default)]
pub(crate) struct ServerFlags {
    pub flagged: Option<(Instant, Arc<HashMap<Key, f32>>)>,
    asked: Option<Instant>,
    /// Bumped when a new answer arrives.
    pub rev: u64,
}

impl Vision {
    /// Use `classifier` instead of loading the model from the model folder (tests, other hosts).
    pub fn set_classifier(&mut self, classifier: Arc<dyn Classifier>) {
        self.injected_classifier = Some(classifier);
    }

    /// Where the classifier lives (inside the model folder).
    pub fn sensitive_dir(&self) -> Option<PathBuf> {
        self.dir.as_ref().map(|d| d.join(SENSITIVE_DIR))
    }

    /// Whether this device can check photos itself (this build has the model's code, or a host supplied one).
    pub fn sensitive_available(&self) -> bool {
        (cfg!(feature = "vision") && !self.thin) || self.injected_classifier.is_some()
    }

    /// Whether the classifier's file is in place.
    pub fn sensitive_installed(&self) -> bool {
        if self.injected_classifier.is_some() {
            return true;
        }
        #[cfg(feature = "vision")]
        if let Some(dir) = self.sensitive_dir() {
            return lightcraft_vision::nsfw::is_model_dir(&dir);
        }
        false
    }

    /// Checking photos is wanted (the user's choice) and this device can do it.
    pub fn sensitive_ready(&self) -> bool {
        self.sensitive && self.sensitive_installed()
    }

    /// The sync server checks this user's photos (an admin turned it on).
    pub fn server_sensitive(&self) -> bool {
        self.remote.supported() && self.remote.status.as_ref().is_some_and(|s| s["sensitive"]["enabled"].as_bool() == Some(true))
    }

    /// Photos checked here (0 until the scores have been opened).
    pub fn sensitive_checked(&self) -> usize {
        self.shared.sensitive_len.load(Ordering::Relaxed)
    }

    /// Changes whenever the flagged photos may have (a view's cache key).
    pub fn sensitive_rev(&self) -> u64 {
        self.shared.sensitive_rev.load(Ordering::Relaxed).wrapping_add(self.remote.sensitive.rev << 32)
    }

    /// The classifier's download state.
    pub fn sensitive_download_status(&self) -> Value {
        #[cfg(feature = "vision")]
        {
            let s = self.sensitive_download.status();
            json!({"running": s.running, "done": s.done, "total": s.total, "file": s.file, "error": s.error, "cancelled": s.cancelled, "finished": s.finished})
        }
        #[cfg(not(feature = "vision"))]
        json!({"running": false, "done": 0, "total": 0, "file": "", "error": null, "cancelled": false, "finished": false})
    }

    /// Start downloading the classifier on a background thread. `Ok(false)` when it is installed or
    /// downloading already.
    pub fn start_sensitive_download(&self, activity: &crate::activity::Activity) -> Result<bool, String> {
        if !self.sensitive_available() {
            return Err("checking photos for sensitive content is not available in this build".into());
        }
        if self.sensitive_installed() {
            return Ok(false);
        }
        #[cfg(feature = "vision")]
        {
            let dir = self.sensitive_dir().ok_or("no folder is set for the search models")?;
            let env = std::env::var(lightcraft_vision::models::MIRRORS_ENV).ok();
            let parts = lightcraft_vision::models::nsfw_downloads(&dir, env.as_deref(), self.mirrors_file.as_deref());
            self.sensitive_download.start_parts("nsfw", parts, lightcraft_vision::models::Options::default(), activity, "Sensitive-content model")
        }
        #[cfg(not(feature = "vision"))]
        {
            let _ = activity;
            Ok(false)
        }
    }

    /// Stop a running download (its partial file stays, to resume). False when none runs.
    pub fn cancel_sensitive_download(&self) -> bool {
        #[cfg(feature = "vision")]
        return self.sensitive_download.cancel();
        #[cfg(not(feature = "vision"))]
        false
    }

    /// What checks photos and how to reach it from a worker thread, with the engine's id, or why it can't be.
    pub(super) fn classifier_provider(&self) -> Result<(ClassifierProvider, String), String> {
        if let Some(c) = &self.injected_classifier {
            return Ok((ClassifierProvider { injected: Some(c.clone()), dir: None }, c.engine().to_string()));
        }
        #[cfg(feature = "vision")]
        {
            if !self.sensitive_installed() {
                if self.sensitive_download_status()["running"].as_bool() == Some(true) {
                    return Err(format!("{SENSITIVE_NOT_INSTALLED} yet: it is downloading."));
                }
                return Err(format!(
                    "{SENSITIVE_NOT_INSTALLED}. Download it (about {:.0} MB, Apache 2.0 licence) with `vision.sensitive.download {{\"acknowledged\": true}}`.",
                    SENSITIVE_BYTES as f64 / 1e6
                ));
            }
            Ok((ClassifierProvider { injected: None, dir: self.sensitive_dir() }, lightcraft_vision::nsfw::ENGINE.to_string()))
        }
        #[cfg(not(feature = "vision"))]
        Err("checking photos for sensitive content is not available in this build".into())
    }
}

/// The classifier, loading it first if needed.
pub(super) fn acquire_classifier(shared: &Shared, provider: &ClassifierProvider) -> Result<Arc<dyn Classifier>, String> {
    if let Some(c) = &provider.injected {
        shared.touch();
        return Ok(c.clone());
    }
    let mut slot = shared.classifier.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(c) = slot.as_ref() {
        shared.touch();
        return Ok(c.clone());
    }
    shared.loading.store(true, Ordering::SeqCst);
    let loaded = load_classifier(provider.dir.as_deref());
    shared.loading.store(false, Ordering::SeqCst);
    let c = loaded?;
    *slot = Some(c.clone());
    shared.loaded.store(true, Ordering::SeqCst);
    shared.touch();
    Ok(c)
}

fn load_classifier(dir: Option<&std::path::Path>) -> Result<Arc<dyn Classifier>, String> {
    #[cfg(feature = "vision")]
    {
        let dir = dir.ok_or("no folder is set for the search models")?;
        let model = guard::catch("loading the sensitive-content model", || lightcraft_vision::nsfw::Nsfw::load(dir))?.map_err(|e| e.to_string())?;
        Ok(Arc::new(model))
    }
    #[cfg(not(feature = "vision"))]
    {
        let _ = dir;
        Err("checking photos for sensitive content is not available in this build".into())
    }
}

/// Opens the scores at `spec` (another model's, or damaged, is derived data: started over).
fn open_index(spec: &ClassSpec) -> Result<SensitiveIndex, String> {
    use lightcraft_vision::Error;
    let Some(path) = &spec.path else { return Ok(SensitiveIndex::memory()) };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("can't create {}: {e}", parent.display()))?;
    }
    match SensitiveIndex::open(path, &spec.engine) {
        Ok(ix) => Ok(ix),
        Err(Error::Mismatch { .. } | Error::Format(_)) => {
            log::info!("sensitive-content scores {} are another model's or damaged: starting over", path.display());
            let _ = std::fs::remove_file(path);
            SensitiveIndex::open(path, &spec.engine).map_err(|e| e.to_string())
        }
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Runs `f` on the scores at `spec`, opening them first (or again, when they are other ones).
pub(super) fn with_sensitive<R>(shared: &Shared, spec: &ClassSpec, f: impl FnOnce(&mut SensitiveIndex) -> R) -> Result<R, String> {
    let mut slot = shared.sensitive_ix.lock().unwrap_or_else(PoisonError::into_inner);
    if slot.as_ref().is_none_or(|(p, _)| *p != spec.path) {
        let ix = open_index(spec)?;
        *slot = Some((spec.path.clone(), ix));
        shared.sensitive_rev.fetch_add(1, Ordering::Relaxed);
    }
    let (_, ix) = slot.as_mut().ok_or("the sensitive-content scores are unavailable")?;
    let before = ix.revision();
    let r = f(ix);
    if ix.revision() != before {
        shared.sensitive_rev.fetch_add(1, Ordering::Relaxed);
    }
    shared.sensitive_len.store(ix.len(), Ordering::Relaxed);
    shared.sensitive_opened.store(true, Ordering::Relaxed);
    Ok(r)
}

/// Scores `work`'s photos, one at a time, and keeps the scores.
pub(super) fn check_photos(
    shared: &Arc<Shared>,
    provider: &ClassifierProvider,
    spec: &ClassSpec,
    work: Vec<(Key, crate::media::RenderJob)>,
    job: &IndexJob,
) -> Result<(), String> {
    if work.is_empty() {
        return Ok(());
    }
    let classifier = acquire_classifier(shared, provider)?;
    let mut failures = 0;
    for (key, render) in work {
        if job.cancel.load(Ordering::Relaxed) {
            break;
        }
        let scored = memory::in_background(|| render.run().rendered)
            .and_then(|r| guard::catch("checking a photo", || classifier.score(&r.image)).and_then(|t| t.map_err(|e| e.to_string())));
        let stopped = match scored {
            Ok(score) => {
                failures = 0;
                if with_sensitive(shared, spec, |ix| ix.insert(key, score).is_err())? {
                    job.failed.fetch_add(1, Ordering::Relaxed);
                }
                None
            }
            Err(e) => {
                log::debug!("sensitive content: can't check a photo: {e}");
                job.failed.fetch_add(1, Ordering::Relaxed);
                failures += 1;
                (failures >= READ_FAILURES).then(|| format!("checking photos stopped after {READ_FAILURES} photos in a row failed: {e}"))
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

impl Session {
    /// The scores file for `engine`: in the library's `search/` folder, else none (memory only).
    pub(super) fn vision_sensitive_path(&self, engine: &str) -> Option<PathBuf> {
        let lib = self.library.as_ref().filter(|l| l.on_disk)?;
        Some(lib.dir.join("search").join(format!("sensitive-{engine}.bin")))
    }

    /// The scores of the photos checked here, when the user wants them checked and the model is here.
    fn local_sensitive_scores(&self) -> HashMap<Key, f32> {
        if !self.vision.sensitive_ready() {
            return HashMap::new();
        }
        let Ok((_, engine)) = self.vision.classifier_provider() else { return HashMap::new() };
        let spec = ClassSpec { path: self.vision_sensitive_path(&engine), engine };
        with_sensitive(&self.vision.shared, &spec, |ix| ix.flagged(SENSITIVE_MIN).into_iter().collect()).unwrap_or_default()
    }

    /// The library's possibly sensitive photos, most likely first: the ones scored here, and the ones the
    /// sync server flagged.
    pub fn sensitive_photos(&mut self) -> Arc<Vec<(PhotoId, f32)>> {
        let key: ListKey = (self.catalog.revision, self.vision.shared.sensitive_rev.load(Ordering::Relaxed), self.vision.remote.sensitive.rev);
        if let Some((k, list, _)) = &self.vision.sensitive_list
            && *k == key
        {
            return list.clone();
        }
        let mut scores = self.local_sensitive_scores();
        if let Some((_, theirs)) = &self.vision.remote.sensitive.flagged {
            for (k, s) in theirs.iter() {
                let mine = scores.entry(*k).or_insert(*s);
                *mine = mine.max(*s);
            }
        }
        let mut list: Vec<(PhotoId, f32)> = Vec::new();
        if !scores.is_empty() {
            for (key, ids) in self.vision_keys() {
                if let Some(s) = scores.get(&key) {
                    // (a photo already hidden is dealt with: not listed, counted or blurred)
                    list.extend(ids.into_iter().filter(|id| self.catalog.photo(*id).is_some_and(|p| !p.hidden)).map(|id| (id, *s)));
                }
            }
        }
        list.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        let set: HashSet<PhotoId> = list.iter().map(|(id, _)| *id).collect();
        let list = Arc::new(list);
        self.vision.sensitive_list = Some((key, list.clone(), Arc::new(set)));
        list
    }

    /// The possibly sensitive photos, as a set (what the grid blurs; cached like the list).
    pub fn sensitive_ids(&mut self) -> Arc<HashSet<PhotoId>> {
        self.sensitive_photos();
        self.vision.sensitive_list.as_ref().map(|(_, _, set)| set.clone()).unwrap_or_default()
    }

    /// Asks the server for the photos it flagged, unless it was asked lately (the answer is
    /// [`Session::sensitive_arrived`]'s). `wait` runs the request here (no frame loop to do it).
    pub(crate) fn vision_fetch_sensitive(&mut self, wait: bool) {
        let r = &self.vision.remote.sensitive;
        let stale = r.flagged.as_ref().is_none_or(|(at, _)| at.elapsed() >= SERVER_TTL);
        let asking = r.asked.is_some_and(|at| at.elapsed() < SERVER_TTL);
        if !self.vision.server_sensitive() || !self.vision_signed_in() || (!stale && !wait) || (asking && !wait) {
            return;
        }
        if self.vision_aux("GET", "/api/sensitive", Body::Empty, Aux::Sensitive).is_ok() {
            self.vision.remote.sensitive.asked = Some(Instant::now());
            if wait {
                self.vision_drive();
            }
        }
    }

    /// The server's flagged photos arrived (`{engine, min, keys: [[content hash, score]], …}`).
    pub(super) fn sensitive_arrived(&mut self, d: &Done) {
        let r = &mut self.vision.remote.sensitive;
        r.asked = None;
        if !d.ok() {
            log::warn!("asking the server for the sensitive photos: {}", why(d));
            return;
        }
        let Ok(v) = serde_json::from_str::<Value>(&d.body) else { return };
        let flagged: HashMap<Key, f32> = v["keys"]
            .as_array()
            .into_iter()
            .flatten()
            .take(1_000_000)
            .filter_map(|k| {
                let k = k.as_array()?;
                let key = Key::from_hex(k.first()?.as_str()?)?;
                let score = k.get(1)?.as_f64()? as f32;
                score.is_finite().then_some((key, score.clamp(0.0, 1.0)))
            })
            .collect();
        r.flagged = Some((Instant::now(), Arc::new(flagged)));
        r.rev = r.rev.wrapping_add(1);
    }

    /// The `sensitive` block of `vision.model.status`.
    pub fn sensitive_status(&self) -> Value {
        let v = &self.vision;
        let flagged = self.vision.sensitive_list.as_ref().map(|(_, l, _)| l.len());
        json!({
            "available": v.sensitive_available(),
            "enabled": v.sensitive,
            "installed": v.sensitive_installed(),
            "server": v.server_sensitive(),
            "dir": v.sensitive_dir().map(|d| d.display().to_string()),
            "checked": v.shared.sensitive_opened.load(Ordering::Relaxed).then(|| v.sensitive_checked()),
            "flagged": flagged,
            "min": SENSITIVE_MIN,
            "sizeBytes": SENSITIVE_BYTES,
            "license": SENSITIVE_LICENSE_NAME,
            "licenseUrl": SENSITIVE_LICENSE_URL,
            "download": v.sensitive_download_status(),
        })
    }
}
