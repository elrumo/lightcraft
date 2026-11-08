//! Rebuild Previews (`sync.rebuildPreviews`): this device builds the smart and mini previews of photos the user
//! chose again, from their originals, and puts them on the sync server in place of the ones it has (a preview built
//! by an older decoder, a wrong frame, a blank one). The original is this device's own file when it has it, else it
//! is fetched into a scratch file and deleted after. The new previews are also this device's own, so its grid shows
//! them at once. A few photos are worked on at once, off the UI thread, under one row in the activity stack.

use std::collections::{HashSet, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use lightcraft_catalog::{AlbumId, PhotoId, Source};
use serde_json::{Value, json};

use super::{Body, Done, NEXT_TASK, PATH_PREFIX, Task, blob_key, why};
use crate::activity::{Cancel, TaskGuard};
use crate::{EngineError, Result, Session};

/// The most photo ids `sync.status` lists as being rebuilt (the ones next in line; the count has them all).
const LISTED: usize = 2000;
/// The most albums a folder's photos are gathered from (a loop in a broken catalog ends there).
const ALBUMS_MAX: usize = 100_000;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One photo to build again.
struct Work {
    id: PhotoId,
    /// The server's key for its original.
    key: String,
    /// Its file name's extension, for the scratch copy of the original ([`super::scratch_ext`]).
    ext: String,
    /// This device's copy of the original (`None`: fetched from the server).
    original: Option<String>,
    /// Where this device keeps its previews.
    smart: String,
    mini: String,
    catalogued: crate::media::Catalogued,
}

/// The photos being rebuilt and the threads doing it.
#[derive(Default)]
pub(crate) struct Rebuild {
    server: String,
    token: String,
    queue: Mutex<VecDeque<Work>>,
    /// Queued or being built (what the grid marks).
    pending: Mutex<HashSet<PhotoId>>,
    /// Built and on the server, not yet drawn again ([`Session::sync_rebuild_upkeep`]).
    built: Mutex<Vec<PhotoId>>,
    /// Of this run (they start again from 0 when a run ends).
    total: AtomicUsize,
    done: AtomicUsize,
    failed: AtomicUsize,
    /// Threads working; changed only with `queue` locked, so a photo added as the last one ends is never left behind.
    workers: AtomicUsize,
    cancel: Arc<AtomicBool>,
    /// The activity row, while threads work.
    guard: Mutex<Option<TaskGuard>>,
    error: Mutex<Option<String>>,
}

/// The sync state's hold on the rebuild: dropping it (signed out, another library) stops the threads.
pub(crate) struct Handle(pub(crate) Arc<Rebuild>);

impl Drop for Handle {
    fn drop(&mut self) {
        self.0.cancel.store(true, Ordering::Relaxed);
    }
}

impl Rebuild {
    /// The photos queued or being built, next in line first (at most [`LISTED`]), and how many there are.
    pub(crate) fn status(&self) -> Value {
        // (the queue first, as everywhere: the other order could wait on a worker that waits on this)
        let queue = lock(&self.queue);
        let pending = lock(&self.pending);
        let queued: HashSet<PhotoId> = queue.iter().map(|w| w.id).collect();
        let ids: Vec<u64> = pending.iter().filter(|id| !queued.contains(id)).chain(queue.iter().map(|w| &w.id)).take(LISTED).map(|id| id.0).collect();
        json!({
            "ids": ids,
            "count": pending.len(),
            "done": self.done.load(Ordering::Relaxed),
            "failed": self.failed.load(Ordering::Relaxed),
            "error": *lock(&self.error),
        })
    }

    fn progress(&self) {
        let (done, failed, total) = (self.done.load(Ordering::Relaxed), self.failed.load(Ordering::Relaxed), self.total.load(Ordering::Relaxed));
        if let Some(g) = lock(&self.guard).as_ref() {
            g.progress((done + failed) as u64, total as u64);
            if failed > 0 {
                g.detail(&format!("{failed} couldn't be built"));
            }
        }
    }

    /// A request to the server, here and now.
    fn call(&self, method: &'static str, path: &str, body: Body, save_to: Option<String>) -> Done {
        let url = format!("{}{path}", self.server.trim_end_matches('/'));
        super::run(&Task::Http { id: NEXT_TASK.fetch_add(1, Ordering::Relaxed), method, url, token: self.token.clone(), body, save_to })
    }
}

/// How many photos are built at once: each waits on the network for its original, then decodes it (a raw takes a core
/// and a few hundred megabytes for a moment).
fn workers() -> usize {
    std::thread::available_parallelism().map_or(2, |n| n.get() / 2).clamp(1, 4)
}

/// A worker: builds photos until none are left or the rebuild is cancelled.
fn work(job: Arc<Rebuild>) {
    loop {
        let w = {
            let mut queue = lock(&job.queue);
            let next = if job.cancel.load(Ordering::Relaxed) { None } else { queue.pop_front() };
            let Some(w) = next else {
                if job.cancel.load(Ordering::Relaxed) {
                    queue.clear();
                    lock(&job.pending).clear();
                }
                if job.workers.fetch_sub(1, Ordering::Relaxed) == 1 {
                    // (the last one out: the row goes, and the next run counts from 0)
                    lock(&job.guard).take();
                    for n in [&job.total, &job.done, &job.failed] {
                        n.store(0, Ordering::Relaxed);
                    }
                }
                return;
            };
            w
        };
        match crate::guard::catch("rebuilding a photo's previews", || build(&job, &w)).and_then(|r| r) {
            Ok(()) => {
                job.done.fetch_add(1, Ordering::Relaxed);
                lock(&job.built).push(w.id);
            }
            Err(e) => {
                log::warn!("sync: can't rebuild the previews of photo {}: {e}", w.id.0);
                job.failed.fetch_add(1, Ordering::Relaxed);
                *lock(&job.error) = Some(e);
            }
        }
        lock(&job.pending).remove(&w.id);
        job.progress();
    }
}

/// Builds `w`'s previews into this device's preview files and puts them on the server.
fn build(job: &Rebuild, w: &Work) -> std::result::Result<(), String> {
    // (named as the photo is: Apple's raw developer, which draws a raw's previews when the frontend gives one, goes by the name)
    let scratch = std::env::temp_dir().join(format!("lightcraft-rebuild-{}-{}{}", std::process::id(), w.key, w.ext));
    let original = match &w.original {
        Some(path) => path.clone(),
        None => {
            let dest = scratch.to_string_lossy().into_owned();
            let got = job.call("GET", &format!("/api/blobs/original/{}", w.key), Body::Empty, Some(dest.clone()));
            if !got.ok() {
                let _ = std::fs::remove_file(&scratch);
                return Err(why(&got));
            }
            dest
        }
    };
    let built = crate::memory::in_background(|| super::build_proxies(&original, &w.smart, &w.mini, w.catalogued));
    // (the original is not needed any more: it can be many hundred megabytes)
    if w.original.is_none() {
        let _ = std::fs::remove_file(&scratch);
    }
    built?;
    for (kind, file) in [("smart", &w.smart), ("mini", &w.mini)] {
        let sent = job.call("PUT", &format!("/api/blobs/{kind}/{}", w.key), Body::File(file.clone()), None);
        if !sent.ok() {
            return Err(why(&sent));
        }
    }
    Ok(())
}

impl Session {
    /// The photos of `album` (a folder: of every album inside it, at any depth).
    pub fn album_photos_deep(&self, album: AlbumId) -> Vec<PhotoId> {
        let mut seen = HashSet::new();
        let mut todo = vec![album];
        let mut out = Vec::new();
        let mut taken = HashSet::new();
        while let Some(a) = todo.pop() {
            if seen.len() >= ALBUMS_MAX || !seen.insert(a) {
                continue;
            }
            out.extend(self.catalog.album_photos(a).into_iter().filter(|id| taken.insert(*id)));
            todo.extend(self.catalog.album_children(Some(a)).into_iter().map(|c| c.id));
        }
        out
    }

    /// Build the previews of `ids` again on this device and put them on the server (`sync.rebuildPreviews`). Photos
    /// already queued are left where they are. Returns how many were added.
    pub fn sync_rebuild_previews(&mut self, ids: &[PhotoId]) -> Result<usize> {
        let st = self.sync.as_ref().ok_or_else(|| EngineError::Other("this library isn't synced".into()))?;
        if !st.signed_in() {
            return Err(EngineError::Other("not signed in to the sync server".into()));
        }
        let (server, token) = (st.config.server.clone(), st.config.token.clone());
        // (a new server or account: the old rebuild ends, its photos are not this one's)
        let current =
            st.rebuild.as_ref().map(|h| h.0.clone()).filter(|j| j.server == server && j.token == token && !j.cancel.load(Ordering::Relaxed));
        let job = current.unwrap_or_else(|| Arc::new(Rebuild { server, token, ..Rebuild::default() }));
        let mut items = Vec::new();
        {
            let pending = lock(&job.pending);
            for id in ids {
                let Some(p) = self.catalog.photo(*id) else { continue };
                if pending.contains(id) || p.deleted || !p.in_library() {
                    continue;
                }
                let (Some(key), Some((smart, mini))) = (blob_key(p), self.proxy_paths(*id)) else { continue };
                // (this device's own file when it has it: nothing to fetch)
                let original = match &p.source {
                    Source::File { path } if !path.starts_with(PATH_PREFIX) && Path::new(path).is_file() => Some(path.clone()),
                    _ => None,
                };
                items.push(Work { id: *id, key, ext: super::scratch_ext(p), original, smart, mini, catalogued: Some((p.width, p.height)) });
            }
        }
        let added = items.len();
        if added == 0 {
            return Ok(0);
        }
        let start = {
            let mut queue = lock(&job.queue);
            lock(&job.pending).extend(items.iter().map(|w| w.id));
            queue.extend(items);
            job.total.fetch_add(added, Ordering::Relaxed);
            let running = job.workers.load(Ordering::Relaxed);
            let want = workers().min(running + queue.len()).saturating_sub(running);
            job.workers.fetch_add(want, Ordering::Relaxed);
            want
        };
        if start > 0 && lock(&job.guard).is_none() {
            let guard = self.activity.start("sync", "Rebuilding previews", Cancel::Flag(job.cancel.clone()));
            *lock(&job.guard) = Some(guard);
        }
        job.progress();
        for n in 0..start {
            let j = job.clone();
            if std::thread::Builder::new().name(format!("lc-rebuild-{n}")).spawn(move || work(j)).is_err() {
                // (a thread that can't start: the ones that did take its share; none at all ends the run here)
                let mut queue = lock(&job.queue);
                if job.workers.fetch_sub(1, Ordering::Relaxed) == 1 {
                    queue.clear();
                    lock(&job.pending).clear();
                    lock(&job.guard).take();
                    return Err(EngineError::Other("can't start a thread to build previews".into()));
                }
            }
        }
        if let Some(st) = self.sync.as_mut()
            && !st.rebuild.as_ref().is_some_and(|h| Arc::ptr_eq(&h.0, &job))
        {
            st.rebuild = Some(Handle(job));
        }
        Ok(added)
    }

    /// Frame upkeep: the photos whose new previews have arrived on this device are drawn again from them.
    pub(super) fn sync_rebuild_upkeep(&mut self) {
        let Some(job) = self.sync.as_ref().and_then(|st| st.rebuild.as_ref()).map(|h| h.0.clone()) else { return };
        let built = std::mem::take(&mut *lock(&job.built));
        for id in built {
            if let Some((smart, mini)) = self.proxy_paths(id) {
                let names: Vec<String> = [&smart, &mini]
                    .into_iter()
                    .inspect(|p| self.media.availability.forget(p))
                    .filter_map(|p| Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned()))
                    .collect();
                if let Some(have) = self.sync.as_mut().and_then(|st| st.have.as_mut()) {
                    have.extend(names);
                }
            }
            if let Some(p) = self.catalog.photo(id).cloned() {
                self.forget_renders(&p);
            }
            self.media.thumb_asks.remove(&id);
            self.media.note_rebuilt(id);
        }
    }
}
