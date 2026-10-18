//! A device as the sync server's processing unit. A user can move the model work on their photos
//! (search vectors, text, faces, sensitive content) off the server onto this device, which has the
//! faster processor, the GPU and the memory, without moving the photos. The helper claims a few photos
//! from the server (`POST /api/work/claim`), fetches only their previews into a private scratch folder,
//! renders and processes each one in memory, deletes the preview, sends the results back through the
//! routes the server already has for them (`POST /api/index/…`), and forgets the photos. Nothing of the
//! user's library is kept here, and this device needs no copy of the library to do it; editing stays on
//! the device, always. Server side: `apps/lightcraft-server/src/work.rs`.
//!
//! The helper is one thread per session ([`Session::vision_helper_set`] starts it, the frame loop's
//! upkeep restarts it after a failure or a sign-in) and a row in the activity stack for its whole run.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use lightcraft_catalog::Photo;
use lightcraft_raster::Rgba8;
use lightcraft_vision::faces::index::FaceIndex;
use lightcraft_vision::{EmbeddingIndex, Key, SensitiveIndex, TextIndex};
use serde_json::{Value, json};

use super::sensitive::{ClassifierProvider, acquire_classifier};
use super::server::why;
use super::{
    BATCH, Busy, FinderProvider, Provider, ReaderProvider, SENSITIVE_EDGE, Shared, THUMB_EDGE, acquire, acquire_finder, acquire_reader,
    neutral_from_preview, next_id,
};
use crate::activity::TaskGuard;
use crate::cloud_sync::{self, Body, Done, Task};
use crate::memory;

/// Photos asked for in one claim.
const CLAIM: usize = 16;
/// How long the helper waits when the server has nothing for it.
const IDLE: Duration = Duration::from_secs(30);
/// How long it waits after the server couldn't be reached.
const RETRY: Duration = Duration::from_secs(30);

/// The work a helper can take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HelperPart {
    Search,
    Text,
    Faces,
    Sensitive,
}

impl HelperPart {
    pub const ALL: [HelperPart; 4] = [HelperPart::Search, HelperPart::Text, HelperPart::Faces, HelperPart::Sensitive];

    /// The name the server and the commands go by.
    pub fn key(self) -> &'static str {
        match self {
            HelperPart::Search => "search",
            HelperPart::Text => "text",
            HelperPart::Faces => "faces",
            HelperPart::Sensitive => "sensitive",
        }
    }

    pub fn from_key(k: &str) -> Option<HelperPart> {
        HelperPart::ALL.into_iter().find(|p| p.key() == k)
    }

    fn slot(self) -> usize {
        self as usize
    }

    /// Where the server takes this part's results.
    fn upload(self) -> &'static str {
        match self {
            HelperPart::Search => "/api/index/embeddings",
            HelperPart::Text => "/api/index/text",
            HelperPart::Faces => "/api/index/faces",
            HelperPart::Sensitive => "/api/index/sensitive",
        }
    }
}

/// The model that does one part, and how to reach it from the helper's thread.
pub(super) enum Runner {
    /// The model's id and vector length.
    Search(Provider, String, usize),
    Text(ReaderProvider, String),
    Faces(FinderProvider, String),
    Sensitive(ClassifierProvider, String),
}

impl Runner {
    pub(super) fn part(&self) -> HelperPart {
        match self {
            Runner::Search(..) => HelperPart::Search,
            Runner::Text(..) => HelperPart::Text,
            Runner::Faces(..) => HelperPart::Faces,
            Runner::Sensitive(..) => HelperPart::Sensitive,
        }
    }

    /// The engine whose results these are (the server keeps only its own).
    fn engine(&self) -> &str {
        match self {
            Runner::Search(_, m, _) => m,
            Runner::Text(_, e) | Runner::Faces(_, e) | Runner::Sensitive(_, e) => e,
        }
    }
}

/// What a helper thread works with.
pub(super) struct Config {
    pub server: String,
    pub token: String,
    pub runners: Vec<Runner>,
    /// Long edge of the renderings text and faces are read from.
    pub text_edge: usize,
}

/// A running (or finished) helper.
#[derive(Default)]
pub struct HelperJob {
    pub(super) cancel: Arc<AtomicBool>,
    pub(super) finished: AtomicBool,
    /// The server and token this helper works for (a change of either ends it).
    pub(super) server: String,
    pub(super) token: String,
    done: [AtomicUsize; 4],
    failed: AtomicUsize,
    state: Mutex<&'static str>,
    error: Mutex<Option<String>>,
    /// Parts the server refused to hand out, and why.
    refused: Mutex<Vec<(String, String)>>,
    pub(super) unavailable: Vec<(String, String)>,
    pub(super) parts: Vec<HelperPart>,
    /// The private folder the previews being worked on are in (it is deleted when the helper ends).
    pub(super) scratch: PathBuf,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl HelperJob {
    pub(super) fn error(&self) -> Option<String> {
        lock(&self.error).clone()
    }

    fn set_error(&self, e: Option<String>) {
        *lock(&self.error) = e;
    }

    fn set_state(&self, s: &'static str) {
        *lock(&self.state) = s;
    }

    pub fn done(&self, part: HelperPart) -> usize {
        self.done[part.slot()].load(Ordering::Relaxed)
    }

    pub fn json(&self) -> Value {
        let done: serde_json::Map<String, Value> = HelperPart::ALL.iter().map(|p| (p.key().to_string(), json!(self.done(*p)))).collect();
        let pairs = |v: &[(String, String)]| Value::Object(v.iter().map(|(k, e)| (k.clone(), json!(e))).collect());
        json!({
            "running": !self.finished.load(Ordering::Relaxed),
            "scratch": self.scratch,
            "active": self.parts.iter().map(|p| p.key()).collect::<Vec<_>>(),
            "state": *lock(&self.state),
            "done": done,
            "failed": self.failed.load(Ordering::Relaxed),
            "error": self.error(),
            "unavailable": pairs(&self.unavailable),
            "refused": pairs(&lock(&self.refused)),
        })
    }
}

/// A request to the server, here and now.
fn call(cfg: &Config, method: &'static str, path: &str, body: Body, save_to: Option<String>) -> Done {
    let url = format!("{}{path}", cfg.server.trim_end_matches('/'));
    cloud_sync::run(&Task::Http { id: next_id(), method, url, token: cfg.token.clone(), body, save_to })
}

/// Waits `d`, ending early when the helper is cancelled.
fn nap(job: &HelperJob, d: Duration) {
    let mut left = d;
    while !left.is_zero() && !job.cancel.load(Ordering::Relaxed) {
        let step = left.min(Duration::from_millis(250));
        std::thread::sleep(step);
        left -= step;
    }
}

/// A photo the server handed out.
struct Item {
    part: HelperPart,
    key: Key,
    /// Which preview to fetch.
    kind: String,
    photo: Photo,
}

fn parse_items(reply: &Value) -> Vec<Item> {
    reply["items"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|it| {
                    Some(Item {
                        part: HelperPart::from_key(it["part"].as_str()?)?,
                        key: Key::from_hex(it["key"].as_str()?)?,
                        kind: it["kind"].as_str()?.to_string(),
                        photo: serde_json::from_value(it["photo"].clone()).ok()?,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The helper thread: claims photos until it is cancelled or the server turns it away.
pub(super) fn run(shared: Arc<Shared>, cfg: Config, job: Arc<HelperJob>, guard: TaskGuard) {
    let scratch = job.scratch.clone();
    let _ = std::fs::remove_dir_all(&scratch);
    if let Err(e) = std::fs::create_dir_all(&scratch) {
        job.set_error(Some(format!("{}: {e}", scratch.display())));
        job.finished.store(true, Ordering::Relaxed);
        return;
    }
    let asked: serde_json::Map<String, Value> = cfg.runners.iter().map(|r| (r.part().key().to_string(), json!(r.engine()))).collect();
    let body = json!({"parts": asked, "max": CLAIM}).to_string();
    while !job.cancel.load(Ordering::Relaxed) {
        let claim = call(&cfg, "POST", "/api/work/claim", Body::Json(body.clone()), None);
        if !claim.ok() {
            job.set_error(Some(why(&claim)));
            job.set_state("waiting");
            // (not signed in any more, or a server that can't hand out work: no point asking again)
            if matches!(claim.status, 401 | 403 | 404) {
                break;
            }
            nap(&job, RETRY);
            continue;
        }
        let reply: Value = serde_json::from_str(&claim.body).unwrap_or(Value::Null);
        *lock(&job.refused) = reply["refused"]
            .as_object()
            .map(|o| o.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string())).collect())
            .unwrap_or_default();
        let items = parse_items(&reply);
        if items.is_empty() {
            job.set_error(None);
            job.set_state("waiting");
            guard.detail("");
            nap(&job, IDLE);
            continue;
        }
        job.set_state("working");
        for runner in &cfg.runners {
            let mine: Vec<&Item> = items.iter().filter(|i| i.part == runner.part()).collect();
            if mine.is_empty() || job.cancel.load(Ordering::Relaxed) {
                continue;
            }
            guard.detail(runner.part().key());
            match work_part(&shared, &cfg, &job, &scratch, runner, &mine) {
                Ok(()) => job.set_error(None),
                Err(e) => {
                    job.set_error(Some(e));
                    nap(&job, RETRY);
                }
            }
        }
    }
    let _ = std::fs::remove_dir_all(&scratch);
    job.set_state("stopped");
    job.finished.store(true, Ordering::Relaxed);
    drop(guard);
}

/// Fetches one preview into the scratch folder, renders it and deletes the file.
fn fetch_and_render(cfg: &Config, scratch: &Path, it: &Item, edge: usize) -> Result<Rgba8, String> {
    let hash = it.photo.content_hash.clone().ok_or("the photo has no content hash")?;
    let file: PathBuf = scratch.join(format!("{}-{}", it.part.key(), it.key.to_hex()));
    let got = call(cfg, "GET", &format!("/api/blobs/{}/{hash}", it.kind), Body::Empty, Some(file.to_string_lossy().into_owned()));
    let rendered = if got.ok() { memory::in_background(|| neutral_from_preview(&it.photo, &file, edge)) } else { Err(why(&got)) };
    let _ = std::fs::remove_file(&file);
    rendered
}

/// Processes `items` (all of one part) with `runner`, sends the results and releases the photos.
fn work_part(shared: &Arc<Shared>, cfg: &Config, job: &HelperJob, scratch: &Path, runner: &Runner, items: &[&Item]) -> Result<(), String> {
    let _busy = Busy::new(shared);
    let part = runner.part();
    let (mut stored, mut failed): (Vec<Key>, Vec<Key>) = (Vec::new(), Vec::new());
    let cancelled = || job.cancel.load(Ordering::Relaxed);
    let bytes = match runner {
        Runner::Search(provider, model, dim) => {
            let m = acquire(shared, provider)?;
            let mut ix = EmbeddingIndex::in_memory(model, *dim).map_err(|e| e.to_string())?;
            for chunk in items.chunks(BATCH) {
                if cancelled() {
                    break;
                }
                let (mut keys, mut images) = (Vec::new(), Vec::new());
                for it in chunk {
                    match fetch_and_render(cfg, scratch, it, THUMB_EDGE) {
                        Ok(img) => {
                            keys.push(it.key);
                            images.push(img);
                        }
                        Err(e) => {
                            log::debug!("helper: can't render a photo for search: {e}");
                            failed.push(it.key);
                        }
                    }
                }
                let refs: Vec<&Rgba8> = images.iter().collect();
                let embed =
                    |imgs: &[&Rgba8]| crate::guard::catch("embedding photos", || m.encode_images(imgs)).and_then(|t| t.map_err(|e| e.to_string()));
                // (a photo the model can't take must not fail the others: then they go one at a time)
                let vectors: Vec<Result<Vec<f32>, String>> = match embed(&refs) {
                    Ok(all) if all.len() == refs.len() => all.into_iter().map(Ok).collect(),
                    _ => refs
                        .iter()
                        .map(|img| embed(std::slice::from_ref(img)).and_then(|mut one| one.pop().ok_or_else(|| "no vector".to_string())))
                        .collect(),
                };
                for (k, v) in keys.into_iter().zip(vectors) {
                    match v.and_then(|v| ix.insert(k, &v).map_err(|e| e.to_string())) {
                        Ok(_) => stored.push(k),
                        Err(_) => failed.push(k),
                    }
                }
            }
            ix.export(|_| false, usize::MAX)
        }
        Runner::Text(provider, engine) => {
            let reader = acquire_reader(shared, provider)?;
            let mut ix = TextIndex::in_memory(engine).map_err(|e| e.to_string())?;
            each(cfg, scratch, items, cfg.text_edge, &cancelled, &mut failed, &mut stored, |key, img| {
                let text = crate::guard::catch("reading text", || reader.text(img)).and_then(|t| t.map_err(|e| e.to_string()))?;
                ix.insert(key, &text).map(|_| ()).map_err(|e| e.to_string())
            });
            ix.export(|_| false, usize::MAX)
        }
        Runner::Faces(provider, _) => {
            let finder = acquire_finder(shared, provider)?;
            let mut ix = FaceIndex::in_memory();
            each(cfg, scratch, items, cfg.text_edge, &cancelled, &mut failed, &mut stored, |key, img| {
                let faces = crate::guard::catch("finding faces", || finder.faces(img)).and_then(|t| t.map_err(|e| e.to_string()))?;
                ix.insert_photo(key, &faces).map(|_| ()).map_err(|e| e.to_string())
            });
            ix.export(|_| false, usize::MAX)
        }
        Runner::Sensitive(provider, engine) => {
            let classifier = acquire_classifier(shared, provider)?;
            let mut ix = SensitiveIndex::in_memory(engine);
            each(cfg, scratch, items, SENSITIVE_EDGE, &cancelled, &mut failed, &mut stored, |key, img| {
                let score = crate::guard::catch("checking a photo", || classifier.score(img)).and_then(|t| t.map_err(|e| e.to_string()))?;
                ix.insert(key, score).map_err(|e| e.to_string())
            });
            ix.export(|_| false, usize::MAX)
        }
    };
    if !stored.is_empty() {
        let file = scratch.join(format!("up-{}", next_id()));
        std::fs::write(&file, &bytes).map_err(|e| format!("{}: {e}", file.display()))?;
        let sent = call(cfg, "POST", part.upload(), Body::File(file.to_string_lossy().into_owned()), None);
        let _ = std::fs::remove_file(&file);
        if !sent.ok() {
            return Err(why(&sent));
        }
        job.done[part.slot()].fetch_add(stored.len(), Ordering::Relaxed);
    }
    job.failed.fetch_add(failed.len(), Ordering::Relaxed);
    // (the photos are the server's again; the ones this device can't do are not offered again)
    let released: Vec<Value> = stored
        .iter()
        .map(|k| json!({"part": part.key(), "key": k.to_hex()}))
        .chain(failed.iter().map(|k| json!({"part": part.key(), "key": k.to_hex(), "failed": true})))
        .collect();
    let _ = call(cfg, "POST", "/api/work/release", Body::Json(json!({"items": released}).to_string()), None);
    Ok(())
}

/// Fetches, renders and processes `items` one at a time with `f`, keeping which photos worked.
#[allow(clippy::too_many_arguments)]
fn each(
    cfg: &Config,
    scratch: &Path,
    items: &[&Item],
    edge: usize,
    cancelled: &dyn Fn() -> bool,
    failed: &mut Vec<Key>,
    stored: &mut Vec<Key>,
    mut f: impl FnMut(Key, &Rgba8) -> Result<(), String>,
) {
    for it in items {
        if cancelled() {
            break;
        }
        match fetch_and_render(cfg, scratch, it, edge).and_then(|img| f(it.key, &img)) {
            Ok(()) => stored.push(it.key),
            Err(e) => {
                log::debug!("helper: can't process a photo for {}: {e}", it.part.key());
                failed.push(it.key);
            }
        }
    }
}

impl super::Vision {
    /// For each part, `true` when this device can run its model now, else why it can't.
    fn helper_availability(&self) -> serde_json::Map<String, Value> {
        let mut out = serde_json::Map::new();
        for part in HelperPart::ALL {
            let made = match part {
                HelperPart::Search => self.provider().map(|_| ()),
                HelperPart::Text => self.reader_provider().map(|_| ()),
                HelperPart::Faces => self.finder_provider().map(|_| ()),
                HelperPart::Sensitive => self.classifier_provider().map(|_| ()),
            };
            out.insert(part.key().to_string(), made.map_or_else(|e| json!(e), |()| json!(true)));
        }
        out
    }
}

impl crate::Session {
    /// The parts this device processes for its server.
    pub fn vision_helper_parts(&self) -> &[String] {
        &self.vision.helper_parts
    }

    /// Choose which work this device does for its server (`vision.setHelper`): the server stops doing
    /// those parts for this user, and this device claims their photos, a few at a time.
    pub fn vision_helper_set(&mut self, parts: &[String]) -> Result<Value, String> {
        let mut chosen = Vec::new();
        for name in parts {
            let p = HelperPart::from_key(name).ok_or_else(|| format!("`{name}` is not work a device can take (search, text, faces, sensitive)"))?;
            if !chosen.contains(&p) {
                chosen.push(p);
            }
        }
        chosen.sort_by_key(|p| p.slot());
        let old = std::mem::replace(&mut self.vision.helper_parts, chosen.iter().map(|p| p.key().to_string()).collect());
        self.save_prefs().map_err(|e| e.to_string())?;
        let dropped: Vec<&String> = old.iter().filter(|o| !self.vision.helper_parts.contains(o)).collect();
        if !dropped.is_empty() && self.vision_signed_in() {
            // (the server does them again)
            let _ = self.vision_aux("PUT", "/api/work/settings", Body::Json(json!({"remove": dropped}).to_string()), super::Aux::Work);
        }
        if let Some(job) = &self.vision.helper {
            // (a running helper has the old choice: it ends, and the upkeep starts one with the new)
            job.cancel.store(true, Ordering::Relaxed);
        }
        self.vision.helper_retry = None;
        self.vision_helper_upkeep();
        Ok(self.vision_helper_status())
    }

    pub fn vision_helper_status(&self) -> Value {
        let mut v = self.vision.helper.as_ref().map_or_else(|| json!({"running": false, "state": "stopped"}), |j| j.json());
        v["parts"] = json!(self.vision.helper_parts);
        v["supported"] = json!(true);
        // what this device can run now: true, or why not (no model code in this build, or not downloaded yet)
        v["available"] = json!(self.vision.helper_availability());
        if v["error"].is_null() {
            v["error"] = json!(self.vision.helper_error);
        }
        v
    }

    /// Frame-loop upkeep: ends a helper that has nothing to work for, reports one that stopped, and
    /// starts one when parts are chosen and this library is signed in to a server.
    pub(super) fn vision_helper_upkeep(&mut self) {
        let signed_in = self.vision_signed_in();
        let (server, token) = self.sync.as_ref().map(|st| (st.config.server.clone(), st.config.token.clone())).unwrap_or_default();
        let wanted = signed_in && !self.vision.helper_parts.is_empty();
        if let Some(job) = self.vision.helper.clone() {
            if job.finished.load(Ordering::Relaxed) {
                self.vision.helper_error = job.error();
                self.vision.helper = None;
                self.vision.helper_retry = Some(web_time::Instant::now() + RETRY * 2);
            } else if !wanted || job.server != server || job.token != token {
                job.cancel.store(true, Ordering::Relaxed);
            }
        }
        if !wanted || self.vision.helper.is_some() || self.vision.helper_retry.is_some_and(|t| t > web_time::Instant::now()) {
            return;
        }
        match self.vision_helper_start(server, token) {
            Ok(()) => self.vision.helper_error = None,
            Err(e) => {
                self.vision.helper_error = Some(e);
                self.vision.helper_retry = Some(web_time::Instant::now() + RETRY * 2);
            }
        }
    }

    fn vision_helper_start(&mut self, server: String, token: String) -> Result<(), String> {
        let v = &self.vision;
        let (mut runners, mut unavailable) = (Vec::new(), Vec::new());
        for name in &v.helper_parts {
            let Some(part) = HelperPart::from_key(name) else { continue };
            let made = match part {
                HelperPart::Search => v.provider().map(|(p, model, dim)| Runner::Search(p, model, dim)),
                HelperPart::Text => v.reader_provider().map(|(p, e)| Runner::Text(p, e)),
                HelperPart::Faces => v.finder_provider().map(|(p, e)| Runner::Faces(p, e)),
                HelperPart::Sensitive => v.classifier_provider().map(|(p, e)| Runner::Sensitive(p, e)),
            };
            match made {
                Ok(r) => runners.push(r),
                Err(e) => unavailable.push((part.key().to_string(), e)),
            }
        }
        if runners.is_empty() {
            return Err(unavailable.first().map_or_else(|| "no work chosen".to_string(), |(_, e)| e.clone()));
        }
        let parts: Vec<HelperPart> = runners.iter().map(Runner::part).collect();
        let cancel = Arc::new(AtomicBool::new(false));
        let guard = self.activity.start("helper", "Processing photos for your server", crate::activity::Cancel::Flag(cancel.clone()));
        let scratch = std::env::temp_dir().join(format!("lightcraft-helper-{}-{}", std::process::id(), next_id()));
        let job = Arc::new(HelperJob {
            cancel,
            server: server.clone(),
            token: token.clone(),
            unavailable,
            parts: parts.clone(),
            scratch,
            ..HelperJob::default()
        });
        let cfg = Config { server, token, runners, text_edge: self.vision.text_edge };
        let (shared, thread_job) = (self.vision.shared.clone(), job.clone());
        std::thread::Builder::new()
            .name("lc-vision-helper".into())
            .spawn(move || run(shared, cfg, thread_job, guard))
            .map_err(|e| format!("can't start the helper: {e}"))?;
        self.vision.helper = Some(job);
        // (the server leaves these parts to this device from now on)
        let names: Vec<&str> = parts.iter().map(|p| p.key()).collect();
        let _ = self.vision_aux("PUT", "/api/work/settings", Body::Json(json!({"add": names}).to_string()), super::Aux::Work);
        Ok(())
    }
}
