//! Lighter videos: a smaller copy the server keeps beside a clip's original, so a library of phone videos takes a fraction
//! of the room when it is streamed or downloaded over a poor connection. The original is never touched.
//!
//! - **Settings** (`<data>/video.json`, [`Settings`]): server-wide, validated, set from the admin page. A damaged file
//!   means the defaults, never an error that stops the server. Off until an admin turns it on.
//! - **The copy** is the blob kind `video` (`blobs/video/<xx>/<hash of the original>`, an MP4), and a small record beside it
//!   outside `blobs` (`users/<name>/video/<hash>.json`, [`Record`]): the copy's codec, height, bitrate and sizes, or the
//!   mark "already efficient" (a copy less than 15% smaller than the clip is not kept) or why a clip failed.
//! - **The queue** ([`Videos`]) is every video in a user's library, existing and new, that has no copy and no mark: counts
//!   (have, total, left), an ETA from the pace so far, pause and resume, a list of failures to retry, and the state a clip
//!   waits in when nothing can encode it. Two kinds of worker take its jobs ([`Videos::next_job`]): a Mac helper over the
//!   claim route (V12: the Rust surface is here, the routes are not), and the server itself when `serverFallback` is on, with
//!   the pure-Rust encoder behind the [`VideoEncoder`] trait, one clip at a time on a thread of the lowest priority.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use lightcraft_catalog::sync::proto::Progress;
use lightcraft_catalog::{Catalog, MediaKind, Photo};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Value, json};

use crate::State;
use crate::accounts;
use crate::api::{self, Resp, UserLib, error, header_value};
use crate::http::Request;

/// The settings file in the data folder.
pub const FILE: &str = "video.json";
/// Bumped when the way copies are made changes enough that older ones are worth making again.
pub const PROFILE_VERSION: u32 = 1;
/// A copy is kept only when it is at most this many percent of the original's size (15% smaller).
pub const KEEP_AT_MOST: u64 = 85;
/// Checking for work when nothing woke the worker.
const TICK: Duration = Duration::from_secs(30);
/// How long a helper keeps a clip before it is offered again.
const LEASE: Duration = Duration::from_secs(600);
/// A helper that asked for work this recently is "there".
const HELPER_FRESH: u64 = 900;
/// A user's counts are reused this long (the activity route is asked every few seconds).
const COUNTS_TTL: Duration = Duration::from_secs(2);
/// Failures listed on the admin page.
const FAILURES_LISTED: usize = 200;
/// The most a header describing a copy may be.
const META_MAX: usize = 1024;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---- the settings --------------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Codec {
    H264,
    Hevc,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Quality {
    Smaller,
    Balanced,
    Better,
}

/// The tallest a copy is made: as the original is, or at most 1080, 720 or 480 lines (a clip is never made taller).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaxHeight {
    Original,
    Lines(u32),
}

impl MaxHeight {
    const LINES: [u32; 3] = [1080, 720, 480];
}

impl Serialize for MaxHeight {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            MaxHeight::Original => s.serialize_str("original"),
            MaxHeight::Lines(n) => s.serialize_u32(*n),
        }
    }
}

impl<'de> Deserialize<'de> for MaxHeight {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let bad = || serde::de::Error::custom("must be original, 1080, 720 or 480");
        match Value::deserialize(d)? {
            Value::String(w) if w == "original" => Ok(MaxHeight::Original),
            Value::Number(n) => {
                n.as_u64().and_then(|n| u32::try_from(n).ok()).filter(|n| MaxHeight::LINES.contains(n)).map(MaxHeight::Lines).ok_or_else(bad)
            }
            _ => Err(bad()),
        }
    }
}

/// What an admin chose (`<data>/video.json`, `GET` / `PUT /api/admin/video`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Settings {
    /// Make lighter copies of the library's videos, existing and new.
    pub enabled: bool,
    pub codec: Codec,
    pub max_height: MaxHeight,
    pub quality: Quality,
    /// Keep no copy that is not at least 15% smaller than the original; mark the clip "already efficient".
    pub skip_if_smaller: bool,
    /// The server encodes by itself (pure Rust, slow, one clip at a time at the lowest priority) when no helper does.
    pub server_fallback: bool,
    /// A Mac helper may claim clips to encode.
    pub helper: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            enabled: false,
            codec: Codec::H264,
            max_height: MaxHeight::Lines(1080),
            quality: Quality::Balanced,
            skip_if_smaller: true,
            server_fallback: false,
            helper: true,
        }
    }
}

/// What a copy is made with, kept in its record to tell a copy made with other settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub codec: Codec,
    pub max_height: MaxHeight,
    pub quality: Quality,
}

/// What one clip is encoded as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    pub codec: Codec,
    pub width: u32,
    pub height: u32,
    pub kbps: u32,
}

/// The bitrate (kbit/s) of 30 pictures a second at "balanced" in H.264, by the height of the picture: the points are
/// 5000 at 1080 and 2500 at 720 (docs/video.md); in between it is interpolated.
const LADDER: [(u32, u32); 7] = [(240, 400), (360, 700), (480, 1200), (720, 2500), (1080, 5000), (1440, 9000), (2160, 16_000)];

fn ladder(height: u32) -> f64 {
    let h = f64::from(height.max(1));
    let mut before: Option<(f64, f64)> = None;
    for &(lh, lk) in &LADDER {
        let (lh, lk) = (f64::from(lh), f64::from(lk));
        if h <= lh {
            return match before {
                Some((ph, pk)) => pk + (lk - pk) * (h - ph) / (lh - ph),
                None => (lk * h / lh).max(200.0),
            };
        }
        before = Some((lh, lk));
    }
    // taller than the ladder: by the picture's area, up to a bound
    before.map_or(5000.0, |(lh, lk)| (lk * (h / lh).powi(2)).min(80_000.0))
}

impl Settings {
    pub fn profile(&self) -> Profile {
        Profile { codec: self.codec, max_height: self.max_height, quality: self.quality }
    }

    /// The size and bitrate a clip of `width` × `height` (as displayed) and `fps` pictures a second is encoded as.
    pub fn target(&self, width: u32, height: u32, fps: f64) -> Target {
        let (width, height) = (width.max(2), height.max(2));
        let target_h = match self.max_height {
            MaxHeight::Lines(n) if n < height => n,
            _ => height,
        };
        let target_w = if target_h == height { width } else { (f64::from(width) * f64::from(target_h) / f64::from(height)).round() as u32 };
        // (the encoders need even sizes)
        let (w, h) = ((target_w & !1).max(2), (target_h & !1).max(2));
        let fps_factor = if fps.is_finite() && fps > 0.0 { (fps / 30.0).clamp(0.5, 2.0) } else { 1.0 };
        let quality = match self.quality {
            Quality::Smaller => 0.6,
            Quality::Balanced => 1.0,
            Quality::Better => 1.5,
        };
        // (HEVC reaches the same picture with about 30% fewer bits)
        let codec = if self.codec == Codec::Hevc { 0.7 } else { 1.0 };
        let kbps = (ladder(h) * fps_factor * quality * codec / 10.0).round() as u32 * 10;
        Target { codec: self.codec, width: w, height: h, kbps: kbps.max(100) }
    }

    /// Read the file in `data` (nothing there, or damaged: the defaults).
    pub fn load(data: &Path) -> Settings {
        std::fs::read(data.join(FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    /// These settings with the fields of `patch` (a JSON object) changed: a field that is not one, or has a value that
    /// is not one of its choices, is refused.
    pub fn patched(&self, patch: &Value) -> Result<Settings, String> {
        let Some(patch) = patch.as_object() else { return Err("the settings are a JSON object".into()) };
        let mut v = serde_json::to_value(self).map_err(|e| e.to_string())?;
        for (k, x) in patch {
            if let Some(o) = v.as_object_mut() {
                o.insert(k.clone(), x.clone());
            }
            // (one field at a time, so that the answer names the one that is wrong)
            serde_json::from_value::<Settings>(v.clone()).map_err(|e| format!("{k}: {e}"))?;
        }
        serde_json::from_value(v).map_err(|e| e.to_string())
    }
}

/// Whether a copy of `dst` bytes is worth keeping for an original of `src` bytes.
pub fn worth_keeping(skip_if_smaller: bool, src: u64, dst: u64) -> bool {
    !skip_if_smaller || src == 0 || u128::from(dst) * 100 <= u128::from(src) * u128::from(KEEP_AT_MOST)
}

// ---- what is known of a clip ---------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mark {
    /// There is a copy (also in `blobs/video`).
    Copy,
    /// The copy would be less than 15% smaller: none is kept.
    Efficient,
    /// It could not be done (until a retry).
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FailKind {
    Unsupported,
    Unreadable,
    Encoder,
}

/// Why a clip could not be encoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub kind: FailKind,
    pub reason: String,
}

impl Failure {
    fn of(kind: FailKind, reason: impl std::fmt::Display) -> Failure {
        Failure { kind, reason: reason.to_string() }
    }
}

/// `users/<name>/video/<hash>.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Record {
    pub state: Mark,
    pub codec: String,
    pub height: u32,
    /// kbit/s.
    pub bitrate: u32,
    pub profile_version: u32,
    pub src_bytes: u64,
    pub dst_bytes: u64,
    /// The encoder was a hardware one.
    pub hardware: bool,
    /// When this was decided (seconds since 1970).
    pub encoded_at: u64,
    /// The settings it was made with.
    pub profile: Option<Profile>,
    /// The copy's digest (what the upload named).
    pub digest: Option<String>,
    /// The clip's file name when it failed (to list it).
    pub name: String,
    pub kind: Option<FailKind>,
    pub reason: String,
}

impl Default for Record {
    fn default() -> Self {
        Record {
            state: Mark::Failed,
            codec: String::new(),
            height: 0,
            bitrate: 0,
            profile_version: PROFILE_VERSION,
            src_bytes: 0,
            dst_bytes: 0,
            hardware: false,
            encoded_at: 0,
            profile: None,
            digest: None,
            name: String::new(),
            kind: None,
            reason: String::new(),
        }
    }
}

impl Record {
    /// A copy of `dst` bytes would not have been enough smaller than the clip's `src`: none is kept.
    fn efficient(name: &str, src: u64, dst: u64, profile: Profile) -> Record {
        Record {
            state: Mark::Efficient,
            name: name.to_string(),
            src_bytes: src,
            dst_bytes: dst,
            encoded_at: accounts::now(),
            profile: Some(profile),
            ..Record::default()
        }
    }
}

fn records_dir(data: &Path, user: &str) -> PathBuf {
    accounts::user_dir(data, user).join("video")
}

fn record_path(data: &Path, user: &str, hash: &str) -> PathBuf {
    records_dir(data, user).join(format!("{hash}.json"))
}

/// Every record of a user's (an unreadable file is not one).
fn read_records(data: &Path, user: &str) -> HashMap<String, Record> {
    let Ok(rd) = std::fs::read_dir(records_dir(data, user)) else { return HashMap::new() };
    rd.flatten()
        .filter_map(|e| {
            let name = e.file_name().to_str()?.strip_suffix(".json")?.to_string();
            let record = serde_json::from_slice(&std::fs::read(e.path()).ok()?).ok()?;
            Some((name, record))
        })
        .collect()
}

/// A video in a user's library.
#[derive(Clone, Debug, PartialEq)]
pub struct Clip {
    pub hash: String,
    pub name: String,
    pub bytes: u64,
    pub duration: Option<f64>,
    pub width: u32,
    pub height: u32,
}

fn hash_of(p: &Photo) -> Option<String> {
    Some(p.content_hash.as_deref()?.split(':').next()?.to_ascii_lowercase()).filter(|h| !h.is_empty())
}

/// The videos in the library (not in Recently Deleted, not virtual copies, not only browsed: the rule of `stats::tally`)
/// that have a content hash, in the order they were added.
fn clips(c: &Catalog) -> Vec<(u64, Clip)> {
    let mut out: Vec<(u64, Clip)> = c
        .photos()
        .filter(|p| p.kind == MediaKind::Video && p.in_library() && p.copy_of.is_none())
        .filter_map(|p| {
            let clip =
                Clip { hash: hash_of(p)?, name: p.file_name.clone(), bytes: p.file_size, duration: p.duration, width: p.width, height: p.height };
            Some((p.id.0, clip))
        })
        .collect();
    out.sort_by_key(|(id, _)| *id);
    out
}

// ---- encoders ------------------------------------------------------------------------------------------------------

/// What an encode made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Encoded {
    pub codec: Codec,
    pub width: u32,
    pub height: u32,
    pub kbps: u32,
    pub hardware: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EncodeError {
    /// Stopped on request (paused, turned off, the server stopping): not a failure of the clip.
    Cancelled,
    Failed(Failure),
}

/// Something that makes the lighter copy of a clip: the server's own software encoder, or a fake in the tests.
pub trait VideoEncoder: Send + Sync {
    /// The codecs it can make.
    fn codecs(&self) -> &[Codec];
    /// Write the copy of the clip at `src` to `dst` (a file this call owns), as `settings` ask; `progress(done, total)`
    /// after each picture; stop soon after `cancel` is set.
    fn encode(&self, src: &Path, dst: &Path, settings: &Settings, cancel: &AtomicBool, progress: &dyn Fn(u64, u64)) -> Result<Encoded, EncodeError>;
}

/// FilmCraft's pure-Rust H.264 encoder through [`lightcraft_video::lighter_copy`]: on one thread, for a server with no Mac
/// to do it (docs/video.md: 140 to 310 times the CPU of a Mac's hardware encoder).
pub struct SoftwareEncoder;

fn failure_of(e: &lightcraft_video::Error) -> Failure {
    match e {
        lightcraft_video::Error::Unsupported(m) => Failure::of(FailKind::Unsupported, m),
        other => Failure::of(FailKind::Unreadable, other),
    }
}

impl VideoEncoder for SoftwareEncoder {
    fn codecs(&self) -> &[Codec] {
        &[Codec::H264]
    }

    fn encode(&self, src: &Path, dst: &Path, settings: &Settings, cancel: &AtomicBool, progress: &dyn Fn(u64, u64)) -> Result<Encoded, EncodeError> {
        let info = lightcraft_video::probe_path(src).map_err(|e| EncodeError::Failed(failure_of(&e)))?;
        if let Some(why) = &info.unsupported {
            return Err(EncodeError::Failed(Failure::of(FailKind::Unsupported, why)));
        }
        let t = settings.target(info.width, info.height, info.fps);
        let plan = lightcraft_video::Plan { width: t.width as usize, height: t.height as usize, kbps: t.kbps, threads: 1 };
        match lightcraft_video::lighter_copy(src, dst, plan, cancel, progress) {
            Ok(_) => Ok(Encoded { codec: Codec::H264, width: t.width, height: t.height, kbps: t.kbps, hardware: false }),
            Err(lightcraft_video::LighterError::Cancelled) => Err(EncodeError::Cancelled),
            Err(lightcraft_video::LighterError::Source(e)) => Err(EncodeError::Failed(failure_of(&e))),
            Err(lightcraft_video::LighterError::Encoder(e)) => Err(EncodeError::Failed(Failure::of(FailKind::Encoder, e))),
        }
    }
}

// ---- the queue -----------------------------------------------------------------------------------------------------

/// Who takes a job.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Worker {
    /// The server's own encoder.
    Server,
    /// A Mac helper (V12 adds the claim routes).
    Helper,
}

/// A clip handed to a worker.
#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub user: String,
    pub clip: Clip,
    /// The settings in force when it was handed out (the copy is made, and its record written, as these say).
    pub settings: Settings,
}

/// Where a user's clips stand.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// Videos in the library with a content hash.
    pub total: usize,
    /// With a lighter copy.
    pub have: usize,
    /// Marked "already efficient": no copy is worth keeping.
    pub efficient: usize,
    /// Failed, waiting for a retry.
    pub failed: usize,
    /// Whose original this server does not have (a device has not uploaded it).
    pub no_original: usize,
    /// Still to do (clips a re-apply of the settings queued again are among them).
    pub left: usize,
}

impl Counts {
    fn plus(self, o: Counts) -> Counts {
        Counts {
            total: self.total + o.total,
            have: self.have + o.have,
            efficient: self.efficient + o.efficient,
            failed: self.failed + o.failed,
            no_original: self.no_original + o.no_original,
            left: self.left + o.left,
        }
    }
}

struct Lease {
    /// When a helper's claim lapses (`None`: the server's own, held while it works).
    until: Option<Instant>,
}

#[derive(Default)]
struct Book {
    marks: HashMap<String, Record>,
    leases: HashMap<String, Lease>,
    /// Clips whose original is not here, as of this `library_revision` (looked for again when the library changes).
    unavailable: (u64, HashSet<String>),
    /// Clips with a copy that the settings were applied to again: they are to do until they are.
    requeued: HashSet<String>,
}

/// How fast clips are being finished since the work began (a pause or the end of the queue starts it over).
#[derive(Clone, Copy, Debug, Default)]
struct Pace {
    started: Option<Instant>,
    done: u64,
}

impl Pace {
    /// Seconds to finish `left` clips at the pace so far (`None` before one is done).
    fn eta_at(&self, now: Instant, left: usize) -> Option<u64> {
        let elapsed = now.checked_duration_since(self.started?)?.as_secs_f64();
        (self.done > 0 && elapsed > 0.0).then(|| (left as f64 * elapsed / self.done as f64).ceil() as u64)
    }

    fn per_minute_at(&self, now: Instant) -> Option<f64> {
        let elapsed = now.checked_duration_since(self.started?)?.as_secs_f64();
        (self.done > 0 && elapsed > 0.0).then(|| self.done as f64 * 60.0 / elapsed)
    }
}

/// An encode the server is running now.
struct Active {
    user: String,
    name: String,
    done: Arc<(AtomicU64, AtomicU64)>,
}

#[derive(Default)]
struct Queue {
    books: HashMap<String, Book>,
    pace: Pace,
    /// The last time a helper asked for work (seconds since 1970).
    helper_seen: Option<u64>,
    encoding: Vec<Active>,
    counts: HashMap<String, (Instant, Counts)>,
}

/// What the queue is doing, in a word, and what it waits for when it waits.
pub fn standing(s: &Settings, paused: bool, left: usize, server_can: bool, helper_there: bool, busy: bool) -> (&'static str, Option<&'static str>) {
    if !s.enabled {
        ("off", None)
    } else if paused {
        ("paused", None)
    } else if left == 0 {
        ("idle", None)
    } else if busy || server_can || (s.helper && helper_there) {
        ("encoding", None)
    } else if s.helper {
        ("waiting", Some("helper"))
    } else {
        ("waiting", Some("nothing"))
    }
}

/// The queue of lighter copies, the settings, and the server's own worker.
pub struct Videos {
    data: PathBuf,
    settings: Mutex<Settings>,
    paused: AtomicBool,
    encoder: Arc<dyn VideoEncoder>,
    queue: Mutex<Queue>,
    wanted: Mutex<bool>,
    bell: Condvar,
    stop: AtomicBool,
    /// Stops the encode in flight (a pause, a setting that makes it moot, shutting down).
    cancel: AtomicBool,
    /// The encoder, and the work it leaves to rayon (the look-ahead, the scaling and the colour conversion), run on this
    /// pool's one thread, so that all of it has the priority [`Videos::low_priority`] gives it, not the global pool's.
    pool: Option<rayon::ThreadPool>,
    /// The server's own encoding runs at the lowest priority ([`Config::video_low_priority`](crate::Config)).
    low_priority: bool,
}

/// The pool the server's own encodes run on: one thread, at the lowest priority when `low`. Nothing else uses it, so a
/// rayon call anywhere in the encode (FilmCraft's look-ahead, the scaler, the colour conversion) stays on that thread
/// instead of going to the global pool at normal priority.
fn encode_pool(low: bool) -> Option<rayon::ThreadPool> {
    rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .thread_name(|_| "lc-video-encode".into())
        .start_handler(move |_| {
            if low {
                lightcraft_sysmem::lower_thread_priority();
            }
        })
        .build()
        .ok()
}

impl Videos {
    /// `low_priority`: the server's own encoding runs at the lowest priority (always, but in tests that need it to finish
    /// on a machine other work has saturated: a thread of the lowest priority gets almost no time there).
    pub fn new(data: &Path, encoder: Option<Arc<dyn VideoEncoder>>, low_priority: bool) -> Videos {
        Videos {
            data: data.to_path_buf(),
            settings: Mutex::new(Settings::load(data)),
            paused: AtomicBool::new(false),
            encoder: encoder.unwrap_or_else(|| Arc::new(SoftwareEncoder)),
            queue: Mutex::new(Queue::default()),
            wanted: Mutex::new(false),
            bell: Condvar::new(),
            stop: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
            pool: encode_pool(low_priority),
            low_priority,
        }
    }

    pub fn settings(&self) -> Settings {
        *lock(&self.settings)
    }

    /// Change the settings (`patch`: the fields to change), keep them in `video.json` and put them to use at once.
    pub fn set(&self, patch: &Value) -> Result<Settings, String> {
        let old = self.settings();
        let new = old.patched(patch)?;
        accounts::write_private_json(&self.data.join(FILE), &new)?;
        *lock(&self.settings) = new;
        // an encode under settings that no longer hold is moot
        if !new.enabled || !new.server_fallback || new.profile() != old.profile() {
            self.cancel.store(true, Ordering::SeqCst);
        }
        lock(&self.queue).counts.clear();
        self.wake();
        Ok(new)
    }

    pub fn paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    pub fn pause(&self, pause: bool) {
        self.paused.store(pause, Ordering::SeqCst);
        let mut q = lock(&self.queue);
        // the pace is counted from when the work goes on
        q.pace = Pace::default();
        q.counts.clear();
        drop(q);
        if pause {
            self.cancel.store(true, Ordering::SeqCst);
        } else {
            self.wake();
        }
    }

    pub(crate) fn wake(&self) {
        *lock(&self.wanted) = true;
        self.bell.notify_all();
    }

    pub(crate) fn shut_down(&self) {
        self.stop.store(true, Ordering::SeqCst);
        self.cancel.store(true, Ordering::SeqCst);
        self.wake();
    }

    /// The server's encoder can make `codec`.
    fn server_can(&self, s: &Settings) -> bool {
        s.server_fallback && self.encoder.codecs().contains(&s.codec)
    }

    fn book<'a>(&self, q: &'a mut Queue, user: &str) -> &'a mut Book {
        q.books.entry(user.to_string()).or_insert_with(|| Book { marks: read_records(&self.data, user), ..Book::default() })
    }

    /// Where a user's clips stand.
    pub(crate) fn counts(&self, st: &State, user: &str) -> Counts {
        if let Some((at, c)) = lock(&self.queue).counts.get(user)
            && at.elapsed() < COUNTS_TTL
        {
            return *c;
        }
        let Ok(lib) = api::lib(st, user) else { return Counts::default() };
        let (found, revision) = {
            let l = lock(&lib);
            (clips(l.catalog()), l.catalog().library_revision)
        };
        let copies = api::copy_names(st, user, &lib);
        let have: HashSet<&str> = copies.iter().map(|(n, _)| n.as_str()).collect();
        let now = Instant::now();
        let mut q = lock(&self.queue);
        let book = self.book(&mut q, user);
        let mut c = Counts::default();
        for (_, clip) in &found {
            c.total += 1;
            match class(book, &have, revision, &clip.hash, now) {
                Class::Have => c.have += 1,
                Class::Efficient => c.efficient += 1,
                Class::Failed => c.failed += 1,
                Class::NoOriginal => c.no_original += 1,
                Class::Left | Class::Leased => c.left += 1,
            }
        }
        q.counts.insert(user.to_string(), (Instant::now(), c));
        c
    }

    /// The next clip of `user`'s for `who` to encode, when the queue is on, not paused, and `who` may work (a helper only
    /// when settings allow one, the server only with the fallback on and an encoder that makes the codec). The clip is the
    /// worker's until it settles it ([`Videos::fail`], or an upload that is kept) or its lease lapses.
    pub fn next_job(&self, st: &State, user: &str, who: Worker) -> Option<Job> {
        let settings = self.settings();
        if !settings.enabled || self.paused() {
            return None;
        }
        match who {
            Worker::Server if !self.server_can(&settings) => return None,
            Worker::Helper if !settings.helper => return None,
            _ => {}
        }
        if who == Worker::Helper {
            lock(&self.queue).helper_seen = Some(accounts::now());
        }
        let lib = api::lib(st, user).ok()?;
        let (found, revision) = {
            let l = lock(&lib);
            (clips(l.catalog()), l.catalog().library_revision)
        };
        let copies = api::copy_names(st, user, &lib);
        let have: HashSet<&str> = copies.iter().map(|(n, _)| n.as_str()).collect();
        loop {
            // the first few that are left, decided without touching the disk
            let now = Instant::now();
            let candidates: Vec<&Clip> = {
                let mut q = lock(&self.queue);
                let book = self.book(&mut q, user);
                found.iter().map(|(_, c)| c).filter(|c| class(book, &have, revision, &c.hash, now) == Class::Left).take(32).collect()
            };
            if candidates.is_empty() {
                return None;
            }
            for clip in candidates {
                if original_path(st, &lib, user, &clip.hash).is_none() {
                    let mut q = lock(&self.queue);
                    let book = self.book(&mut q, user);
                    if book.unavailable.0 != revision {
                        book.unavailable = (revision, HashSet::new());
                    }
                    book.unavailable.1.insert(clip.hash.clone());
                    continue;
                }
                let mut q = lock(&self.queue);
                let book = self.book(&mut q, user);
                // (another worker may have taken it since)
                if class(book, &have, revision, &clip.hash, Instant::now()) != Class::Left {
                    continue;
                }
                let until = (who == Worker::Helper).then(|| Instant::now() + LEASE);
                book.leases.insert(clip.hash.clone(), Lease { until });
                q.pace.started.get_or_insert_with(Instant::now);
                q.counts.clear();
                return Some(Job { user: user.to_string(), clip: clip.clone(), settings });
            }
        }
    }

    /// A helper's lease on `hash` is over without an answer (it went away): the clip is offered again.
    pub fn release(&self, user: &str, hash: &str) {
        let mut q = lock(&self.queue);
        self.book(&mut q, user).leases.remove(hash);
        q.counts.clear();
    }

    /// A worker says it is done with `hash`, and the digest of the copy it uploaded: ends its lease if the copy (or the
    /// verdict that none is worth keeping) is what the server holds, and says which. `Err`: there is no such copy.
    pub fn finish(&self, user: &str, hash: &str, digest: &str) -> Result<Mark, String> {
        let mut q = lock(&self.queue);
        let book = self.book(&mut q, user);
        match book.marks.get(hash) {
            Some(r) if r.state == Mark::Copy && r.digest.as_deref() == Some(digest) => {}
            Some(r) if r.state == Mark::Efficient => {}
            _ => return Err("the server has no copy of this video with that digest".into()),
        }
        let state = book.marks.get(hash).map_or(Mark::Copy, |r| r.state);
        book.leases.remove(hash);
        Ok(state)
    }

    /// A worker could not encode `hash`: it is listed as failed until a retry.
    pub fn fail(&self, st: &State, job: &Job, f: Failure) {
        log::warn!("lighter video of {} ({}) for {}: {}", job.clip.name, job.clip.hash, job.user, f.reason);
        let rec = Record {
            state: Mark::Failed,
            name: job.clip.name.clone(),
            src_bytes: job.clip.bytes,
            kind: Some(f.kind),
            reason: f.reason,
            encoded_at: accounts::now(),
            ..Record::default()
        };
        self.settle(st, &job.user, &job.clip.hash, rec);
    }

    /// Write a clip's record and let the queue know.
    pub(crate) fn settle(&self, st: &State, user: &str, hash: &str, rec: Record) {
        if let Err(e) = accounts::write_private_json(&record_path(&st.data, user, hash), &rec) {
            log::error!("lighter video record of {hash}: {e}");
        }
        // (the listing of copies the usage numbers come from is made again)
        if let Ok(lib) = api::lib(st, user) {
            lock(&lib).forget_stats();
        }
        let mut q = lock(&self.queue);
        if rec.state != Mark::Failed {
            q.pace.done += 1;
        }
        let book = self.book(&mut q, user);
        book.leases.remove(hash);
        book.requeued.remove(hash);
        book.marks.insert(hash.to_string(), rec);
        q.counts.clear();
    }

    /// Clips that failed are tried again; how many.
    pub fn retry_failed(&self, st: &State) -> usize {
        let names: Vec<String> = accounts::read_users(&st.data).map(|f| f.users.into_keys().collect()).unwrap_or_default();
        let mut n = 0;
        let mut q = lock(&self.queue);
        for user in names {
            let book = self.book(&mut q, &user);
            let failed: Vec<String> = book.marks.iter().filter(|(_, r)| r.state == Mark::Failed).map(|(h, _)| h.clone()).collect();
            for h in failed {
                book.marks.remove(&h);
                let _ = std::fs::remove_file(record_path(&st.data, &user, &h));
                n += 1;
            }
        }
        q.counts.clear();
        drop(q);
        self.wake();
        n
    }

    /// Put the settings to use on the clips already done: copies made with other settings are made again, and the
    /// originals that were missing are looked for again. How many copies are queued again.
    pub fn apply(&self, st: &State) -> usize {
        let (profile, skip) = (self.settings().profile(), self.settings().skip_if_smaller);
        let names: Vec<String> = accounts::read_users(&st.data).map(|f| f.users.into_keys().collect()).unwrap_or_default();
        let mut n = 0;
        let mut q = lock(&self.queue);
        for user in names {
            let book = self.book(&mut q, &user);
            book.unavailable = (0, HashSet::new());
            // (a clip judged "already efficient" under settings that keep smaller copies, or other ones, is judged again)
            let stale: Vec<String> = book
                .marks
                .iter()
                .filter(|(_, r)| match r.state {
                    Mark::Copy => r.profile != Some(profile) || r.profile_version != PROFILE_VERSION,
                    Mark::Efficient => r.profile != Some(profile) || r.profile_version != PROFILE_VERSION || !skip,
                    Mark::Failed => false,
                })
                .map(|(h, _)| h.clone())
                .collect();
            n += stale.len();
            book.requeued.extend(stale);
        }
        q.counts.clear();
        drop(q);
        self.wake();
        n
    }

    /// The failures of everyone's, newest first, for the admin page.
    fn failures(&self, names: &[String]) -> Vec<Value> {
        let mut q = lock(&self.queue);
        let mut out: Vec<(u64, Value)> = Vec::new();
        for user in names {
            for (hash, r) in self.book(&mut q, user).marks.iter().filter(|(_, r)| r.state == Mark::Failed) {
                let kind = r.kind.and_then(|k| serde_json::to_value(k).ok());
                out.push((r.encoded_at, json!({"user": user, "hash": hash, "name": r.name, "kind": kind, "reason": r.reason, "at": r.encoded_at})));
            }
        }
        out.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
        out.into_iter().take(FAILURES_LISTED).map(|(_, v)| v).collect()
    }

    /// What the admin page shows: the settings, where the queue stands, the saved bytes, the failures.
    pub(crate) fn status(&self, st: &State) -> Value {
        let s = self.settings();
        let names: Vec<String> = accounts::read_users(&st.data).map(|f| f.users.into_keys().collect()).unwrap_or_default();
        let (mut all, mut users) = (Counts::default(), Vec::new());
        let (mut originals, mut encoded, mut saved) = (0u64, 0u64, 0u64);
        for name in &names {
            let c = self.counts(st, name);
            all = all.plus(c);
            users.push(json!({"name": name, "total": c.total, "have": c.have, "efficient": c.efficient, "failed": c.failed, "noOriginal": c.no_original, "left": c.left}));
            if let Ok(lib) = api::lib(st, name) {
                let (_, v) = api::library_stats(st, name, &lib);
                (originals, encoded, saved) = (originals + v.originals.bytes, encoded + v.encoded.bytes, saved + v.saved);
            }
        }
        let (busy, seen, eta, per_minute, encoding, queued_again) = {
            let mut q = lock(&self.queue);
            let now = Instant::now();
            let held = |b: &Book| b.leases.values().any(|l| l.until.is_some_and(|u| u > now));
            let busy = !q.encoding.is_empty() || (s.helper && q.books.values().any(held));
            let encoding: Vec<Value> = q
                .encoding
                .iter()
                .map(|a| json!({"user": a.user, "name": a.name, "done": a.done.0.load(Ordering::Relaxed), "total": a.done.1.load(Ordering::Relaxed)}))
                .collect();
            if all.left == 0 {
                q.pace = Pace::default();
            }
            let queued_again = q.books.values().map(|b| b.requeued.len()).sum::<usize>();
            (busy, q.helper_seen, q.pace.eta_at(now, all.left), q.pace.per_minute_at(now), encoding, queued_again)
        };
        let helper_there = seen.is_some_and(|t| accounts::now().saturating_sub(t) < HELPER_FRESH);
        let server_can = self.server_can(&s);
        let (state, waiting) = standing(&s, self.paused(), all.left, server_can, helper_there, busy);
        json!({
            "settings": s,
            "state": state, "waitingFor": waiting, "paused": self.paused(),
            "total": all.total, "have": all.have, "efficient": all.efficient, "failed": all.failed, "noOriginal": all.no_original, "left": all.left,
            "queuedAgain": queued_again,
            "etaSecs": eta, "perMinute": per_minute,
            "bytes": {"originals": originals, "encoded": encoded, "saved": saved},
            "serverCanEncode": server_can,
            "helper": {"seen": seen, "there": helper_there},
            "encoding": encoding,
            "users": users,
            "failures": self.failures(&names),
        })
    }

    /// A user's progress for their devices' status popover (`None`: nothing to show).
    pub(crate) fn progress(&self, st: &State, user: &str) -> Option<Progress> {
        let s = self.settings();
        if !s.enabled {
            return None;
        }
        let c = self.counts(st, user);
        let total = c.have + c.efficient + c.left;
        if total == 0 {
            return None;
        }
        let eta = (c.left > 0).then(|| lock(&self.queue).pace.eta_at(Instant::now(), c.left)).flatten();
        Some(Progress { done: (c.have + c.efficient) as u64, total: total as u64, eta_secs: eta })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    Have,
    Efficient,
    Failed,
    NoOriginal,
    /// Someone is working on it.
    Leased,
    Left,
}

/// Where a clip stands. Pure: nothing is read from the disk.
fn class(book: &Book, copies: &HashSet<&str>, revision: u64, hash: &str, now: Instant) -> Class {
    if book.leases.get(hash).is_some_and(|l| l.until.is_none_or(|u| u > now)) {
        return Class::Leased;
    }
    if book.requeued.contains(hash) {
        return Class::Left;
    }
    match book.marks.get(hash).map(|r| r.state) {
        _ if copies.contains(hash) => Class::Have,
        // (the mark is written when the copy is, so it is right before the listing of copies is)
        Some(Mark::Copy) => Class::Have,
        Some(Mark::Efficient) => Class::Efficient,
        Some(Mark::Failed) => Class::Failed,
        _ if book.unavailable.0 == revision && book.unavailable.1.contains(hash) => Class::NoOriginal,
        _ => Class::Left,
    }
}

/// Where a user's original with this content is: kept here, or in a library folder.
fn original_path(st: &State, lib: &Mutex<UserLib>, user: &str, hash: &str) -> Option<PathBuf> {
    let blobs = lock(lib).dir.join("blobs");
    api::blob_path(&blobs, "original", hash).filter(|p| p.is_file()).or_else(|| api::folder_original_path(st, lib, user, hash))
}

/// Move a finished file into place and make the move durable.
fn place(part: &Path, to: &Path) -> Result<(), String> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    std::fs::rename(part, to).map_err(|e| format!("{}: {e}", to.display()))?;
    if let Some(parent) = to.parent()
        && let Ok(d) = std::fs::File::open(parent)
    {
        let _ = d.sync_all();
    }
    Ok(())
}

// ---- the server's own worker ---------------------------------------------------------------------------------------

/// The worker thread: takes the next clip of any user when the fallback is on, encodes it, files the result.
pub(crate) fn run(st: &Arc<State>) {
    let v = &st.video;
    // (the encoder is the machine's last priority: previews, renders and requests come first; it runs on the encode pool's
    // thread, and this one, which only looks for the next clip, goes with it)
    if v.low_priority {
        lightcraft_sysmem::lower_thread_priority();
    }
    while !v.stop.load(Ordering::SeqCst) {
        {
            let mut w = lock(&v.wanted);
            if !*w {
                w = v.bell.wait_timeout(w, TICK).unwrap_or_else(PoisonError::into_inner).0;
            }
            *w = false;
        }
        while !v.stop.load(Ordering::SeqCst) {
            let users: Vec<String> = accounts::read_users(&st.data).map(|f| f.users.into_keys().collect()).unwrap_or_default();
            let Some(job) = users.iter().find_map(|u| v.next_job(st, u, Worker::Server)) else { break };
            v.run_job(st, job);
        }
    }
}

impl Videos {
    fn run_job(&self, st: &State, job: Job) {
        let Ok(lib) = api::lib(st, &job.user) else { return self.release(&job.user, &job.clip.hash) };
        let Some(src) = original_path(st, &lib, &job.user, &job.clip.hash) else { return self.release(&job.user, &job.clip.hash) };
        let blobs = lock(&lib).dir.join("blobs");
        let (Some(to), Ok(r)) = (api::blob_path(&blobs, "video", &job.clip.hash), accounts::random_hex(6)) else {
            return self.release(&job.user, &job.clip.hash);
        };
        let part = blobs.join("tmp").join(format!("video-{}-{r}.part", job.clip.hash));
        if let Some(dir) = part.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let progress = Arc::new((AtomicU64::new(0), AtomicU64::new(0)));
        self.cancel.store(false, Ordering::SeqCst);
        lock(&self.queue).encoding.push(Active { user: job.user.clone(), name: job.clip.name.clone(), done: progress.clone() });
        // (a pause that came before the flag was cleared would be lost: look again)
        if self.paused() || !self.settings().enabled || self.stop.load(Ordering::SeqCst) {
            lock(&self.queue).encoding.retain(|a| !Arc::ptr_eq(&a.done, &progress));
            return self.release(&job.user, &job.clip.hash);
        }
        let result = lightcraft_engine::guard::catch("encoding a video", || {
            let encode = || {
                self.encoder.encode(&src, &part, &job.settings, &self.cancel, &|done, total| {
                    progress.0.store(done, Ordering::Relaxed);
                    progress.1.store(total, Ordering::Relaxed);
                })
            };
            match &self.pool {
                Some(pool) => pool.install(encode),
                None => encode(),
            }
        });
        lock(&self.queue).encoding.retain(|a| !Arc::ptr_eq(&a.done, &progress));
        match result {
            Ok(Ok(made)) => self.file_copy(st, &job, &part, &to, made),
            Ok(Err(EncodeError::Cancelled)) => {
                let _ = std::fs::remove_file(&part);
                self.release(&job.user, &job.clip.hash);
            }
            Ok(Err(EncodeError::Failed(f))) => {
                let _ = std::fs::remove_file(&part);
                self.fail(st, &job, f);
            }
            Err(panic) => {
                let _ = std::fs::remove_file(&part);
                self.fail(st, &job, Failure::of(FailKind::Encoder, panic));
            }
        }
    }

    /// The encoder made `part`: keep it, or note that it is not worth keeping.
    fn file_copy(&self, st: &State, job: &Job, part: &Path, to: &Path, made: Encoded) {
        let dst = std::fs::metadata(part).map(|m| m.len()).unwrap_or(0);
        if dst == 0 {
            let _ = std::fs::remove_file(part);
            return self.fail(st, job, Failure::of(FailKind::Encoder, "the encoder wrote nothing"));
        }
        if !worth_keeping(job.settings.skip_if_smaller, job.clip.bytes, dst) {
            let _ = std::fs::remove_file(part);
            let rec = Record::efficient(&job.clip.name, job.clip.bytes, dst, job.settings.profile());
            return self.settle(st, &job.user, &job.clip.hash, rec);
        }
        if let Err(e) = place(part, to) {
            let _ = std::fs::remove_file(part);
            return self.fail(st, job, Failure::of(FailKind::Encoder, e));
        }
        st.sizes.add(&job.user, dst);
        let rec = Record {
            state: Mark::Copy,
            codec: match made.codec {
                Codec::H264 => "h264",
                Codec::Hevc => "hevc",
            }
            .into(),
            height: made.height,
            bitrate: made.kbps,
            src_bytes: job.clip.bytes,
            dst_bytes: dst,
            hardware: made.hardware,
            encoded_at: accounts::now(),
            profile: Some(job.settings.profile()),
            name: job.clip.name.clone(),
            ..Record::default()
        };
        self.settle(st, &job.user, &job.clip.hash, rec);
    }
}

// ---- a copy uploaded by a helper ----------------------------------------------------------------------------------

/// What a device may say of the copy it uploads (`X-LightCraft-Video`, JSON, at most 1 KiB).
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct UploadMeta {
    hardware: bool,
}

/// `PUT /api/blobs/video/<hash of the original>`: the lighter copy of a clip in the user's library, resumable like an
/// original (`Content-Range`, `HEAD` answers `Upload-Offset`). A copy is not the original, so its bytes do not hash to the
/// name: the device sends their digest in `X-LightCraft-Digest` and the server checks what arrived against it. A copy that
/// is not an MP4 or MOV the server reads, or is not as long as the clip, is refused, and with `skipIfSmaller` one that is
/// not at least 15% smaller is dropped and the clip marked "already efficient" (`{"kept": false}`).
pub(crate) fn put(st: &State, req: &mut Request, l: &Mutex<UserLib>, user: &str, dir: &Path, hash: &str, path: &Path) -> Resp {
    let settings = st.video.settings();
    if !settings.enabled {
        return error(409, "lighter videos are off on this server");
    }
    let Some(digest) = header_value(req, "X-LightCraft-Digest").and_then(lightcraft_preview::Hash128::parse).map(|h| h.to_string()) else {
        return error(400, "X-LightCraft-Digest: the copy's own digest (32 hex digits) is needed");
    };
    let meta: UploadMeta =
        header_value(req, "X-LightCraft-Video").filter(|m| m.len() <= META_MAX).and_then(|m| serde_json::from_str(m).ok()).unwrap_or_default();
    let clip = {
        let l = lock(l);
        clips(l.catalog()).into_iter().map(|(_, c)| c).find(|c| c.hash == hash)
    };
    let Some(clip) = clip else { return error(404, "this library has no video with that content hash") };
    let incoming = req.body_length().map_or(0, |n| n as u64);
    let kept: Cell<Option<Record>> = Cell::new(None);
    let mut check = |part: &Path, total: u64| -> Result<Option<Value>, (u16, String)> {
        let info = lightcraft_video::probe_path(part).map_err(|e| (422, format!("not a video this server can read: {e}")))?;
        if let Some(d) = clip.duration.filter(|d| d.is_finite() && *d > 0.0) {
            let tolerance = (d * 0.01).max(0.5);
            if (info.duration - d).abs() > tolerance {
                return Err((422, format!("the copy lasts {:.1} s, the video {d:.1} s", info.duration)));
            }
        }
        if !worth_keeping(settings.skip_if_smaller, clip.bytes, total) {
            st.video.settle(st, user, hash, Record::efficient(&clip.name, clip.bytes, total, settings.profile()));
            return Ok(Some(json!({"kept": false, "efficient": true, "srcBytes": clip.bytes, "dstBytes": total})));
        }
        let kbps = if info.duration > 0.0 { (total as f64 * 8.0 / info.duration / 1000.0).round() as u32 } else { 0 };
        kept.set(Some(Record {
            state: Mark::Copy,
            codec: info.codec.to_string(),
            height: info.height,
            bitrate: kbps,
            src_bytes: clip.bytes,
            dst_bytes: total,
            hardware: meta.hardware,
            encoded_at: accounts::now(),
            profile: Some(settings.profile()),
            digest: Some(digest.clone()),
            name: clip.name.clone(),
            ..Record::default()
        }));
        Ok(None)
    };
    let resp = api::put_blob(st, req, dir, "video", path, api::Check::Digest(&digest), Some(&mut check));
    if resp.status_code().0 == 200
        && let Some(rec) = kept.take()
    {
        st.sizes.add(user, incoming);
        st.video.settle(st, user, hash, rec);
    }
    resp
}

/// The part file of a copy being uploaded.
pub(crate) fn part_of(dir: &Path, hash: &str) -> PathBuf {
    dir.join("tmp").join(format!("video-{hash}.part"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lc-server-video-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Off by default, validated on the way in, kept across a restart, and a damaged file means the defaults.
    #[test]
    fn settings_are_validated_kept_and_a_damaged_file_means_the_defaults() {
        let dir = temp("settings");
        let v = Videos::new(&dir, None, false);
        let s = v.settings();
        assert_eq!((s.enabled, s.codec, s.max_height, s.quality), (false, Codec::H264, MaxHeight::Lines(1080), Quality::Balanced));
        assert_eq!((s.skip_if_smaller, s.server_fallback, s.helper), (true, false, true));

        let s = v.set(&json!({"enabled": true, "codec": "hevc", "maxHeight": "original", "quality": "smaller"})).unwrap();
        assert_eq!((s.enabled, s.codec, s.max_height, s.quality), (true, Codec::Hevc, MaxHeight::Original, Quality::Smaller));
        assert!(s.skip_if_smaller, "what the patch leaves out stays");
        assert_eq!(Videos::new(&dir, None, false).settings(), s, "kept in video.json");
        let on_disk: Value = serde_json::from_slice(&std::fs::read(dir.join(FILE)).unwrap()).unwrap();
        assert_eq!((on_disk["maxHeight"].clone(), on_disk["skipIfSmaller"].clone()), (json!("original"), json!(true)));

        for (bad, why) in [
            (json!({"codec": "av1"}), "codec"),
            (json!({"maxHeight": 900}), "maxHeight"),
            (json!({"maxHeight": "huge"}), "maxHeight"),
            (json!({"maxHeight": -1}), "maxHeight"),
            (json!({"quality": "best"}), "quality"),
            (json!({"enabled": "yes"}), "boolean"),
            (json!({"replaceOriginals": true}), "replaceOriginals"),
            (json!([1, 2]), "object"),
        ] {
            let e = v.set(&bad).unwrap_err();
            assert!(e.contains(why), "{bad}: {e}");
        }
        assert_eq!(v.settings(), s, "a refused change changes nothing");
        assert_eq!(v.set(&json!({"maxHeight": 480})).unwrap().max_height, MaxHeight::Lines(480));

        for damaged in [&b"{not json"[..], b"{\"codec\": \"av1\"}", b"[]", b""] {
            std::fs::write(dir.join(FILE), damaged).unwrap();
            assert_eq!(Videos::new(&dir, None, false).settings(), Settings::default());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 5000 kbit/s at 1080p30 and 2500 at 720p30 for Balanced, scaled by quality, rate and codec; never taller than the
    /// clip, never odd.
    #[test]
    fn a_clip_is_encoded_at_a_bitrate_for_its_size_and_rate() {
        let balanced = Settings { max_height: MaxHeight::Original, ..Settings::default() };
        assert_eq!(balanced.target(1920, 1080, 30.0).kbps, 5000);
        assert_eq!(balanced.target(1280, 720, 30.0).kbps, 2500);
        assert_eq!(balanced.target(1920, 1080, 60.0).kbps, 10_000);
        assert_eq!(balanced.target(1920, 1080, 24.0).kbps, 4000);
        assert_eq!(balanced.target(1920, 1080, 0.0).kbps, 5000, "a rate nobody knows counts as 30");
        assert_eq!(Settings { quality: Quality::Smaller, ..balanced }.target(1920, 1080, 30.0).kbps, 3000);
        assert_eq!(Settings { quality: Quality::Better, ..balanced }.target(1920, 1080, 30.0).kbps, 7500);
        assert_eq!(Settings { codec: Codec::Hevc, ..balanced }.target(1920, 1080, 30.0).kbps, 3500);
        let t = balanced.target(3840, 2160, 30.0);
        assert_eq!((t.width, t.height, t.kbps), (3840, 2160, 16_000));

        let capped = Settings { max_height: MaxHeight::Lines(720), ..Settings::default() };
        let t = capped.target(3840, 2160, 30.0);
        assert_eq!((t.width, t.height, t.kbps), (1280, 720, 2500));
        // portrait: the height is what is capped
        let t = capped.target(1080, 1920, 30.0);
        assert_eq!((t.width, t.height), (404, 720), "a 9:16 clip keeps its shape, on even sizes");
        let t = capped.target(640, 360, 30.0);
        assert_eq!((t.width, t.height, t.kbps), (640, 360, 700), "a smaller clip is not made bigger");
        let t = capped.target(641, 361, 30.0);
        assert_eq!((t.width % 2, t.height % 2), (0, 0));
        assert!(capped.target(2, 2, 1.0).kbps >= 100 && capped.target(16_000, 16_000, 1000.0).kbps < 200_000);
    }

    /// A copy has to be at least 15% smaller, unless the setting is off or the original's size is not known.
    #[test]
    fn a_copy_is_kept_only_when_it_is_15_percent_smaller() {
        assert!(worth_keeping(true, 1000, 850));
        assert!(!worth_keeping(true, 1000, 851));
        assert!(!worth_keeping(true, 1000, 1200));
        assert!(worth_keeping(false, 1000, 1200));
        assert!(worth_keeping(true, 0, 1200));
        assert!(!worth_keeping(true, u64::MAX, u64::MAX));
    }

    #[test]
    fn the_queue_says_what_it_waits_for() {
        let on = Settings { enabled: true, ..Settings::default() };
        assert_eq!(standing(&Settings::default(), false, 5, true, true, true).0, "off");
        assert_eq!(standing(&on, true, 5, true, true, true).0, "paused");
        assert_eq!(standing(&on, false, 0, false, false, false).0, "idle");
        assert_eq!(standing(&on, false, 5, false, false, false), ("waiting", Some("helper")), "no helper yet, no fallback");
        assert_eq!(standing(&Settings { helper: false, ..on }, false, 5, false, false, false), ("waiting", Some("nothing")));
        assert_eq!(standing(&on, false, 5, true, false, false).0, "encoding", "the server's own fallback");
        assert_eq!(standing(&on, false, 5, false, true, false).0, "encoding", "a helper that was just there");
        assert_eq!(standing(&on, false, 5, false, false, true).0, "encoding", "a job in hand");
    }

    /// The ETA is the pace so far: finished clips over the time since the work began.
    #[test]
    fn the_eta_is_the_pace_so_far() {
        let t0 = Instant::now();
        let mut p = Pace { started: Some(t0), done: 0 };
        assert_eq!(p.eta_at(t0 + Duration::from_secs(60), 10), None, "nothing is done yet");
        p.done = 3;
        assert_eq!(p.eta_at(t0 + Duration::from_secs(60), 10), Some(200));
        assert_eq!(p.per_minute_at(t0 + Duration::from_secs(60)), Some(3.0));
        assert_eq!(p.eta_at(t0 + Duration::from_secs(60), 0), Some(0));
        assert_eq!(Pace::default().eta_at(t0, 5), None);
    }

    /// Everything an encode asks of rayon stays on the encode pool's one thread (not the global pool's, which runs at normal
    /// priority), whether or not that thread is lowered.
    #[test]
    fn the_encode_runs_on_a_pool_of_one_thread_of_its_own() {
        use rayon::prelude::*;
        for low in [false, true] {
            let pool = encode_pool(low).unwrap();
            let (name, threads, seen) = pool.install(|| {
                let here = std::thread::current().id();
                // (what the scaler and the look-ahead do)
                let off: usize = (0..64usize).into_par_iter().filter(|_| std::thread::current().id() != here).count();
                (std::thread::current().name().map(str::to_string), rayon::current_num_threads(), off)
            });
            assert_eq!((name.as_deref(), threads, seen), (Some("lc-video-encode"), 1, 0), "low priority {low}");
        }
    }

    /// The record lives beside the copies, not among them: `stats::copies` lists `blobs/video` and must not see it.
    #[test]
    fn the_record_of_a_copy_is_not_counted_as_a_copy() {
        let data = temp("record");
        let hash = format!("{:032x}", 7);
        let blobs = accounts::user_dir(&data, "ann").join("blobs");
        let copy = api::blob_path(&blobs, "video", &hash).unwrap();
        std::fs::create_dir_all(copy.parent().unwrap()).unwrap();
        std::fs::write(&copy, vec![0u8; 500]).unwrap();
        let rec =
            Record { state: Mark::Copy, codec: "h264".into(), height: 720, bitrate: 2500, src_bytes: 5000, dst_bytes: 500, ..Record::default() };
        accounts::write_private_json(&record_path(&data, "ann", &hash), &rec).unwrap();
        std::fs::write(records_dir(&data, "ann").join("junk.json"), b"{broken").unwrap();
        assert_eq!(crate::stats::copies(&blobs), vec![(hash.clone(), 500)], "one copy, and the record is not another");
        let read = read_records(&data, "ann");
        assert_eq!(read.len(), 1, "an unreadable record is not one");
        assert_eq!(read.get(&hash), Some(&rec));
        let _ = std::fs::remove_dir_all(&data);
    }
}
