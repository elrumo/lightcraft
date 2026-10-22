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
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
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

/// Photos asked for in one claim (a claim, the counts, the results and the release are four round trips to the server:
/// more photos for each spreads them out).
const CLAIM: usize = 64;
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
    /// Not a model: building the smart and mini previews of photos (the device fetches each original, builds them
    /// and puts them on the server).
    Previews,
}

impl HelperPart {
    pub const ALL: [HelperPart; 5] = [HelperPart::Search, HelperPart::Text, HelperPart::Faces, HelperPart::Sensitive, HelperPart::Previews];

    /// The name the server and the commands go by.
    pub fn key(self) -> &'static str {
        match self {
            HelperPart::Search => "search",
            HelperPart::Text => "text",
            HelperPart::Faces => "faces",
            HelperPart::Sensitive => "sensitive",
            HelperPart::Previews => "previews",
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
            HelperPart::Previews => "/api/blobs",
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
    /// Builds previews: needs no model.
    Previews,
}

impl Runner {
    pub(super) fn part(&self) -> HelperPart {
        match self {
            Runner::Search(..) => HelperPart::Search,
            Runner::Text(..) => HelperPart::Text,
            Runner::Faces(..) => HelperPart::Faces,
            Runner::Sensitive(..) => HelperPart::Sensitive,
            Runner::Previews => HelperPart::Previews,
        }
    }

    /// The engine whose results these are (the server keeps only its own).
    fn engine(&self) -> &str {
        match self {
            Runner::Search(_, m, _) => m,
            Runner::Text(_, e) | Runner::Faces(_, e) | Runner::Sensitive(_, e) => e,
            Runner::Previews => "previews",
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
    /// The last claim came back with no photos and no part refused: nothing is left for this helper to do now.
    idle: AtomicBool,
    /// The server and token this helper works for (a change of either ends it).
    pub(super) server: String,
    pub(super) token: String,
    done: [AtomicUsize; HelperPart::ALL.len()],
    /// Per part, as the server last said: the photos in the library, and how many had a result before this
    /// helper's own `done` (so the live count is `base + done`).
    total: [AtomicUsize; HelperPart::ALL.len()],
    base: [AtomicUsize; HelperPart::ALL.len()],
    /// Per part, the milliseconds spent working on it (the rate the time left goes by).
    worked_ms: [AtomicU64; HelperPart::ALL.len()],
    /// Per part, when the batch being worked on began (its time counts in the rate before the batch is done, or a
    /// slow first batch would show no speed or time left until its last photo).
    batch_began: Mutex<[Option<web_time::Instant>; HelperPart::ALL.len()]>,
    failed: AtomicUsize,
    state: Mutex<&'static str>,
    error: Mutex<Option<String>>,
    /// Parts the server refused to hand out, and why.
    refused: Mutex<Vec<(String, String)>>,
    /// Parts an admin took back from this user's devices: the server does them now, and this device stops asking.
    taken_back: Mutex<Vec<String>>,
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

    /// What the server said (`/api/search/status`): `done` of `total` photos have a result for `part`.
    fn set_progress(&self, part: HelperPart, done: usize, total: usize) {
        self.total[part.slot()].store(total, Ordering::Relaxed);
        self.base[part.slot()].store(done.saturating_sub(self.done(part)), Ordering::Relaxed);
    }

    /// How far `part` is now, and the seconds left at this device's rate (once it has one); `None` before the
    /// server has said.
    pub fn progress(&self, part: HelperPart) -> Option<(usize, usize, Option<u64>)> {
        let total = self.total[part.slot()].load(Ordering::Relaxed);
        if total == 0 {
            return None;
        }
        let mine = self.done(part);
        let done = self.base[part.slot()].load(Ordering::Relaxed).saturating_add(mine).min(total);
        let eta = self.per_second(part).map(|rate| ((total - done) as f64 / rate).round() as u64);
        Some((done, total, eta))
    }

    /// Photos a second this device does for `part`, over the time it spent working on it (once there are a few to tell by).
    pub fn per_second(&self, part: HelperPart) -> Option<f64> {
        let running = lock(&self.batch_began)[part.slot()].map_or(0.0, |t| t.elapsed().as_secs_f64());
        let worked = self.worked_ms[part.slot()].load(Ordering::Relaxed) as f64 / 1000.0 + running;
        let mine = self.done(part);
        (mine >= 3 && worked >= 3.0).then(|| mine as f64 / worked)
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
            "progress": self.parts.iter().filter_map(|p| {
                let (done, total, eta) = self.progress(*p)?;
                let per_minute = self.per_second(*p).map(|r| (r * 60.0).round() as u64);
                Some((p.key().to_string(), json!({"done": done, "total": total, "etaSecs": eta, "perMinute": per_minute})))
            }).collect::<serde_json::Map<String, Value>>(),
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
pub(super) fn run(shared: Arc<Shared>, cfg: Config, job: Arc<HelperJob>, guards: Vec<(HelperPart, TaskGuard)>) {
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
        *lock(&job.taken_back) =
            reply["takenBack"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default();
        // (how far each part is: the server's own counts, which every server has; the previews' come with the claim)
        let status = call(&cfg, "GET", "/api/search/status", Body::Empty, None);
        let status: Value = if status.ok() { serde_json::from_str(&status.body).unwrap_or(Value::Null) } else { Value::Null };
        for (part, _) in &guards {
            let (done, total) = match part {
                HelperPart::Search => (&status["indexed"], &status["total"]),
                HelperPart::Text => (&status["text"]["indexed"], &status["total"]),
                HelperPart::Faces => (&status["faces"]["photos"], &status["total"]),
                HelperPart::Sensitive => (&status["sensitive"]["checked"], &status["total"]),
                HelperPart::Previews => (&reply["progress"]["previews"]["done"], &reply["progress"]["previews"]["total"]),
            };
            if let (Some(done), Some(total)) = (done.as_u64(), total.as_u64()) {
                job.set_progress(*part, usize::try_from(done).unwrap_or(usize::MAX), usize::try_from(total).unwrap_or(usize::MAX));
            }
        }
        publish(&job, &guards);
        let items = parse_items(&reply);
        job.idle.store(items.is_empty() && lock(&job.refused).is_empty(), Ordering::Relaxed);
        if items.is_empty() {
            job.set_error(None);
            job.set_state("waiting");
            nap(&job, IDLE);
            continue;
        }
        job.set_state("working");
        // (every task at once: the ones on the GPU keep it busy while the ones on the CPU use the processor, and each
        // waits on its own downloads)
        let results: Vec<Result<(), String>> = std::thread::scope(|scope| {
            let mut running = Vec::new();
            for runner in &cfg.runners {
                let mine: Vec<&Item> = items.iter().filter(|i| i.part == runner.part()).collect();
                if mine.is_empty() || job.cancel.load(Ordering::Relaxed) {
                    continue;
                }
                let (shared, cfg, job, scratch, guards) = (&shared, &cfg, &job, &scratch, &guards);
                let task = move || {
                    let began = web_time::Instant::now();
                    lock(&job.batch_began)[runner.part().slot()] = Some(began);
                    let worked = match runner {
                        Runner::Previews => work_previews(shared, cfg, job, scratch, &mine, &|| publish(job, guards)),
                        _ => work_part(shared, cfg, job, scratch, runner, &mine),
                    };
                    // (added and cleared together: the rate never counts a batch twice)
                    let mut began_at = lock(&job.batch_began);
                    job.worked_ms[runner.part().slot()].fetch_add(u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX), Ordering::Relaxed);
                    began_at[runner.part().slot()] = None;
                    drop(began_at);
                    publish(job, guards);
                    worked
                };
                running.push(
                    std::thread::Builder::new()
                        .name(format!("lc-vision-helper-{}", runner.part().key()))
                        .spawn_scoped(scope, task)
                        .map_err(|e| format!("can't start a thread for {}: {e}", runner.part().key())),
                );
            }
            running.into_iter().map(|h| h.and_then(|h| h.join().unwrap_or_else(|_| Err("a helper task stopped unexpectedly".to_string())))).collect()
        });
        let problems: Vec<String> = results.into_iter().filter_map(Result::err).collect();
        if problems.is_empty() {
            job.set_error(None);
        } else {
            job.set_error(Some(problems.join("; ")));
            nap(&job, RETRY);
        }
    }
    let _ = std::fs::remove_dir_all(&scratch);
    job.set_state("stopped");
    job.finished.store(true, Ordering::Relaxed);
    drop(guards);
}

/// Shows each part's progress in its activity row (the row works out the rate and the time left).
fn publish(job: &HelperJob, guards: &[(HelperPart, TaskGuard)]) {
    for (part, guard) in guards {
        if let Some((done, total, _)) = job.progress(*part) {
            guard.progress(done as u64, total as u64);
            // (a batch takes seconds to minutes: the speed over the time spent working, not the meter's)
            guard.set_rate(job.per_second(*part));
        }
    }
}

/// How many photos are worked on at once, for each task. Fetching, decoding and building previews wait on the network and the disk
/// and use one core each, and a small model can't keep a machine's cores busy by itself.
fn workers() -> usize {
    std::thread::available_parallelism().map_or(4, |n| n.get() / 2).clamp(4, 10)
}

/// Runs `f` on each of `items`, [`workers`] at a time, and gives the results in the items' order. An item not started
/// because the helper was cancelled is `None`; one whose `f` panicked is an error (a panic must not end the helper).
fn par<T: Send>(items: &[&Item], cancelled: &(dyn Fn() -> bool + Sync), f: &(dyn Fn(&Item) -> T + Sync)) -> Vec<Option<Result<T, String>>> {
    let out: Mutex<Vec<Option<Result<T, String>>>> = Mutex::new(items.iter().map(|_| None).collect());
    let next = AtomicUsize::new(0);
    let work = || {
        while !cancelled() {
            let i = next.fetch_add(1, Ordering::Relaxed);
            let Some(it) = items.get(i) else { break };
            let r = crate::guard::catch("a photo of the helper", || f(it));
            if let Some(slot) = lock(&out).get_mut(i) {
                *slot = Some(r);
            }
        }
    };
    std::thread::scope(|scope| {
        let mut started = 0;
        for n in 1..workers().min(items.len()) {
            // (a thread that can't start: fewer work at once; this one works too)
            if std::thread::Builder::new().name(format!("lc-vision-helper-{n}")).spawn_scoped(scope, work).is_ok() {
                started += 1;
            }
        }
        log::trace!("helper: {} photos on {} threads", items.len(), started + 1);
        work();
    });
    out.into_inner().unwrap_or_else(PoisonError::into_inner)
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
                // (fetched and decoded a few at a time; the model takes them together)
                let rendered = par(chunk, &cancelled, &|it| fetch_and_render(cfg, scratch, it, THUMB_EDGE));
                for (it, r) in chunk.iter().zip(rendered) {
                    match r.map(|r| r.and_then(|r| r)) {
                        Some(Ok(img)) => {
                            keys.push(it.key);
                            images.push(img);
                        }
                        Some(Err(e)) => {
                            log::debug!("helper: can't render a photo for search: {e}");
                            failed.push(it.key);
                        }
                        None => {}
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
            let read = each(cfg, scratch, items, cfg.text_edge, &cancelled, &|img| {
                crate::guard::catch("reading text", || reader.text(img)).and_then(|t| t.map_err(|e| e.to_string()))
            });
            keep(read, HelperPart::Text, &mut stored, &mut failed, |key, text| ix.insert(key, &text).map(|_| ()).map_err(|e| e.to_string()));
            ix.export(|_| false, usize::MAX)
        }
        Runner::Faces(provider, _) => {
            let finder = acquire_finder(shared, provider)?;
            let mut ix = FaceIndex::in_memory();
            let found = each(cfg, scratch, items, cfg.text_edge, &cancelled, &|img| {
                crate::guard::catch("finding faces", || finder.faces(img)).and_then(|t| t.map_err(|e| e.to_string()))
            });
            keep(found, HelperPart::Faces, &mut stored, &mut failed, |key, faces| {
                ix.insert_photo(key, &faces).map(|_| ()).map_err(|e| e.to_string())
            });
            ix.export(|_| false, usize::MAX)
        }
        Runner::Sensitive(provider, engine) => {
            let classifier = acquire_classifier(shared, provider)?;
            let mut ix = SensitiveIndex::in_memory(engine);
            let scored = each(cfg, scratch, items, SENSITIVE_EDGE, &cancelled, &|img| {
                crate::guard::catch("checking a photo", || classifier.score(img)).and_then(|t| t.map_err(|e| e.to_string()))
            });
            keep(scored, HelperPart::Sensitive, &mut stored, &mut failed, |key, score| ix.insert(key, score).map_err(|e| e.to_string()));
            ix.export(|_| false, usize::MAX)
        }
        // (built by `work_previews`)
        Runner::Previews => return Err("previews are built by their own worker".into()),
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
    release(cfg, part, &stored, &failed);
    Ok(())
}

/// The photos are the server's again; the ones this device can't do are not offered again.
fn release(cfg: &Config, part: HelperPart, stored: &[Key], failed: &[Key]) {
    let released: Vec<Value> = stored
        .iter()
        .map(|k| json!({"part": part.key(), "key": k.to_hex()}))
        .chain(failed.iter().map(|k| json!({"part": part.key(), "key": k.to_hex(), "failed": true})))
        .collect();
    let _ = call(cfg, "POST", "/api/work/release", Body::Json(json!({"items": released}).to_string()), None);
}

/// Builds the previews of `items`: each original is fetched into the scratch folder, its smart and mini previews are
/// built there and put on the server, and the three files are deleted. A photo is done when both are on the server.
fn work_previews(
    shared: &Arc<Shared>,
    cfg: &Config,
    job: &HelperJob,
    scratch: &Path,
    items: &[&Item],
    tick: &(dyn Fn() + Sync),
) -> Result<(), String> {
    let _busy = Busy::new(shared);
    let (mut stored, mut failed): (Vec<Key>, Vec<Key>) = (Vec::new(), Vec::new());
    let mut problem = None;
    // (a few at a time: each waits on the network for its original, then decodes it)
    let built = par(items, &|| job.cancel.load(Ordering::Relaxed), &|it| {
        let r = build_and_put(cfg, scratch, it);
        if r.is_ok() {
            job.done[HelperPart::Previews.slot()].fetch_add(1, Ordering::Relaxed);
            tick();
        }
        r
    });
    let mut back = Vec::new();
    for (it, r) in items.iter().zip(built) {
        match r {
            // (cancelled before it started: it stays with this device until its time is up)
            None => {}
            Some(Ok(Ok(()))) => stored.push(it.key),
            // (the server can't be reached: the photo is not at fault, it is given back unmarked)
            Some(Ok(Err(Failure::Server(e)))) => {
                problem.get_or_insert(e);
                back.push(it.key);
            }
            Some(Ok(Err(Failure::Photo(e))) | Err(e)) => {
                log::debug!("helper: can't build the previews of {}: {e}", it.key.to_hex());
                failed.push(it.key);
            }
        }
    }
    job.failed.fetch_add(failed.len(), Ordering::Relaxed);
    stored.extend(back);
    release(cfg, HelperPart::Previews, &stored, &failed);
    problem.map_or(Ok(()), Err)
}

/// Why a photo's previews weren't built here.
enum Failure {
    /// The server wouldn't give the original or take the previews.
    Server(String),
    /// The original can't be read or decoded.
    Photo(String),
}

fn build_and_put(cfg: &Config, scratch: &Path, it: &Item) -> Result<(), Failure> {
    let hash = it.photo.content_hash.clone().ok_or_else(|| Failure::Photo("the photo has no content hash".into()))?;
    let name = |what: &str| scratch.join(format!("previews-{}-{what}", it.key.to_hex()));
    let (original, smart, mini) = (name("original"), name("smart"), name("mini"));
    let text = |p: &Path| p.to_string_lossy().into_owned();
    let result = (|| {
        let got = call(cfg, "GET", &format!("/api/blobs/original/{hash}"), Body::Empty, Some(text(&original)));
        if !got.ok() {
            return Err(Failure::Server(why(&got)));
        }
        memory::in_background(|| cloud_sync::build_proxies(&text(&original), &text(&smart), &text(&mini))).map_err(Failure::Photo)?;
        // (the original is not needed any more: it can be many hundred megabytes)
        let _ = std::fs::remove_file(&original);
        for (kind, file) in [("smart", &smart), ("mini", &mini)] {
            let sent = call(cfg, "PUT", &format!("/api/blobs/{kind}/{hash}"), Body::File(text(file)), None);
            if !sent.ok() {
                return Err(Failure::Server(why(&sent)));
            }
        }
        Ok(())
    })();
    for f in [&original, &smart, &mini] {
        let _ = std::fs::remove_file(f);
    }
    result
}

/// Puts what the photos came to in the results (in order, one thread), and sorts the photos into the ones that worked
/// and the ones that did not.
fn keep<T>(
    results: Vec<(Key, Result<T, String>)>,
    part: HelperPart,
    stored: &mut Vec<Key>,
    failed: &mut Vec<Key>,
    mut put: impl FnMut(Key, T) -> Result<(), String>,
) {
    for (key, r) in results {
        match r.and_then(|v| put(key, v)) {
            Ok(()) => stored.push(key),
            Err(e) => {
                log::debug!("helper: can't process a photo for {}: {e}", part.key());
                failed.push(key);
            }
        }
    }
}

/// Fetches, renders and processes `items` a few at a time with `f`: each photo that was worked on, with what came
/// of it (photos left because the helper was cancelled are not in the answer).
fn each<T: Send>(
    cfg: &Config,
    scratch: &Path,
    items: &[&Item],
    edge: usize,
    cancelled: &(dyn Fn() -> bool + Sync),
    f: &(dyn Fn(&Rgba8) -> Result<T, String> + Sync),
) -> Vec<(Key, Result<T, String>)> {
    let done = par(items, cancelled, &|it| fetch_and_render(cfg, scratch, it, edge).and_then(|img| f(&img)));
    items.iter().zip(done).filter_map(|(it, r)| Some((it.key, r?.and_then(|r| r)))).collect()
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
                // (it needs no model)
                HelperPart::Previews => Ok(()),
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
            let p = HelperPart::from_key(name)
                .ok_or_else(|| format!("`{name}` is not work a device can take (search, text, faces, sensitive, previews)"))?;
            if !chosen.contains(&p) {
                chosen.push(p);
            }
        }
        chosen.sort_by_key(|p| p.slot());
        let old = std::mem::replace(&mut self.vision.helper_parts, chosen.iter().map(|p| p.key().to_string()).collect());
        self.vision.helper_paused.retain(|p| self.vision.helper_parts.contains(p));
        self.vision.helper_once.retain(|p| self.vision.helper_parts.contains(p));
        self.save_prefs().map_err(|e| e.to_string())?;
        let dropped: Vec<&String> = old.iter().filter(|o| !self.vision.helper_parts.contains(o)).collect();
        let added: Vec<&String> = self.vision.helper_parts.iter().filter(|p| !old.contains(p)).collect();
        if !added.is_empty() && self.vision_signed_in() {
            // (a choice: the server's taking the part back no longer holds)
            let _ = self.vision_aux("PUT", "/api/work/settings", Body::Json(json!({"add": added, "chosen": true}).to_string()), super::Aux::Work);
        }
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

    /// Do `part` for the server until nothing is left to claim, then give it back (`vision.helperOnce`).
    pub fn vision_helper_once(&mut self, part: &str) -> Result<Value, String> {
        HelperPart::from_key(part).ok_or_else(|| format!("`{part}` is not work a device can take (search, text, faces, sensitive, previews)"))?;
        let mut parts = self.vision.helper_parts.clone();
        if !parts.iter().any(|p| p == part) {
            parts.push(part.to_string());
        }
        // (taking it up again from a pause too: asking for it is asking for it to run)
        self.vision.helper_paused.retain(|p| p != part);
        if !self.vision.helper_once.iter().any(|p| p == part) {
            self.vision.helper_once.push(part.to_string());
        }
        // (one change of the choice: each ends the running helper, and a second one right after would leave the new
        // helper waiting out the retry delay)
        self.vision_helper_set(&parts)
    }

    /// Gives back the `once` parts when the helper has found nothing left for them.
    fn vision_helper_finish_once(&mut self) {
        if self.vision.helper_once.is_empty()
            || !self.vision.helper.as_ref().is_some_and(|j| j.idle.load(Ordering::Relaxed) && !j.cancel.load(Ordering::Relaxed))
        {
            return;
        }
        let once = std::mem::take(&mut self.vision.helper_once);
        let rest: Vec<String> = self.vision.helper_parts.iter().filter(|p| !once.contains(p)).cloned().collect();
        let _ = self.vision_helper_set(&rest);
    }

    /// Hold back (or take up again) some of the work this device does for its server (`vision.setHelperPaused`): a paused
    /// part is still this device's, but no photos are claimed for it until it is resumed.
    pub fn vision_helper_set_paused(&mut self, paused: &[String]) -> Result<Value, String> {
        for name in paused {
            HelperPart::from_key(name).ok_or_else(|| format!("`{name}` is not work a device can take (search, text, faces, sensitive, previews)"))?;
        }
        self.vision.helper_paused = paused.iter().filter(|p| self.vision.helper_parts.contains(p)).cloned().collect();
        self.vision.helper_paused.dedup();
        self.save_prefs().map_err(|e| e.to_string())?;
        if let Some(job) = &self.vision.helper {
            // (the running helper has the old choice: it ends, and the upkeep starts one with the new)
            job.cancel.store(true, Ordering::Relaxed);
        }
        self.vision.helper_retry = None;
        self.vision_helper_upkeep();
        Ok(self.vision_helper_status())
    }

    /// Whether every part this device does for its server is held back.
    fn vision_helper_all_paused(&self) -> bool {
        !self.vision.helper_parts.is_empty() && self.vision.helper_parts.iter().all(|p| self.vision.helper_paused.contains(p))
    }

    pub fn vision_helper_status(&self) -> Value {
        let mut v = self.vision.helper.as_ref().map_or_else(|| json!({"running": false, "state": "stopped"}), |j| j.json());
        v["paused"] = json!(self.vision.helper_paused);
        v["once"] = json!(self.vision.helper_once);
        if self.vision_helper_all_paused() {
            v["state"] = json!("paused");
        }
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
        self.vision_helper_finish_once();
        // (the server took parts back from this user's devices: this device's choice follows, and stays off them)
        let taken = self.vision.helper.as_ref().map(|j| lock(&j.taken_back).clone()).unwrap_or_default();
        if self.vision.helper_parts.iter().any(|p| taken.contains(p)) {
            let rest: Vec<String> = self.vision.helper_parts.iter().filter(|p| !taken.contains(p)).cloned().collect();
            let _ = self.vision_helper_set(&rest);
            return;
        }
        let signed_in = self.vision_signed_in();
        let (server, token) = self.sync.as_ref().map(|st| (st.config.server.clone(), st.config.token.clone())).unwrap_or_default();
        let wanted = signed_in && !self.vision.helper_parts.is_empty() && !self.vision_helper_all_paused();
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
            if v.helper_paused.contains(name) {
                continue;
            }
            let made = match part {
                HelperPart::Search => v.provider().map(|(p, model, dim)| Runner::Search(p, model, dim)),
                HelperPart::Text => v.reader_provider().map(|(p, e)| Runner::Text(p, e)),
                HelperPart::Faces => v.finder_provider().map(|(p, e)| Runner::Faces(p, e)),
                HelperPart::Sensitive => v.classifier_provider().map(|(p, e)| Runner::Sensitive(p, e)),
                HelperPart::Previews => Ok(Runner::Previews),
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
        // (a row for each part, so each shows its own count and time left; any Cancel stops the helper)
        let guards: Vec<(HelperPart, TaskGuard)> = parts
            .iter()
            .map(|p| {
                let label = match p {
                    HelperPart::Search => "Describing photos for your server",
                    HelperPart::Text => "Reading text in photos for your server",
                    HelperPart::Faces => "Finding people for your server",
                    HelperPart::Sensitive => "Checking sensitive content for your server",
                    HelperPart::Previews => "Building previews for your server",
                };
                (*p, self.activity.start("helper", label, crate::activity::Cancel::Flag(cancel.clone())))
            })
            .collect();
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
            .spawn(move || run(shared, cfg, thread_job, guards))
            .map_err(|e| format!("can't start the helper: {e}"))?;
        self.vision.helper = Some(job);
        // (the server leaves these parts to this device from now on)
        let names: Vec<&str> = parts.iter().map(|p| p.key()).collect();
        let _ = self.vision_aux("PUT", "/api/work/settings", Body::Json(json!({"add": names}).to_string()), super::Aux::Work);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rate_counts_the_batch_being_worked_on() {
        let job = HelperJob::default();
        let p = HelperPart::Previews;
        job.set_progress(p, 0, 100);
        assert_eq!(job.per_second(p), None, "nothing done yet");
        job.done[p.slot()].store(10, Ordering::Relaxed);
        lock(&job.batch_began)[p.slot()] = web_time::Instant::now().checked_sub(Duration::from_secs(10));
        let rate = job.per_second(p).expect("10 photos in 10 s, batch still running");
        assert!((0.9..1.1).contains(&rate), "{rate}");
        assert!(job.progress(p).and_then(|(_, _, eta)| eta).is_some_and(|e| (80..100).contains(&e)));
    }
}
