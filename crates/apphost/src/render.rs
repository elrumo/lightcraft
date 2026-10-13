//! Background rendering, shared by every frontend: engine [`RenderJob`]s run on a [`JobPool`];
//! results become whatever the frontend shows ([`Present`]: egui textures, image leases for the
//! SwiftUI app).
//!
//! Each visible thing that needs pixels is a *slot* (a grid thumbnail, the loupe, the "before"
//! image…). Requests are deduplicated per slot (a newer request replaces a queued older one) and
//! prioritised (loupe first, then on-screen thumbnails, then prefetch); thumbnails scrolled far
//! out of view are dropped from the queue. Thumbnail jobs hit the engine's preview cache (memory +
//! disk) before rendering. On wasm the jobs run inline, one per frame, unless the host installs a
//! [`RenderOffload`] (the browser build's Web Workers): then queued jobs are handed to it as it has
//! room, and its results are collected every frame.
//!
//! Stand-ins ([`QuickJob`]s, once per photo and settings): [`Slot::Preview`] holds what the loupe
//! shows until [`Slot::Main`] has the photo's render (cached view render, embedded camera JPEG or
//! a thumbnail render); [`Slot::ThumbQuick`] holds a raw's embedded preview in the grid until its
//! rendered thumbnail arrives (a cached thumbnail found by the quick job becomes the
//! [`Slot::Thumb`] texture directly).
//!
//! A render that panics is a failed render of its slot (the panic guard is inside each job), never
//! a lost worker or a slot left pending for good.

use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::time::Duration;

use lightcraft_catalog::PhotoId;
use lightcraft_engine::Session;
use lightcraft_engine::media::{QuickJob, QuickSource, RenderJob, RenderResult};
use lightcraft_engine::pipeline::StageCache;
use lightcraft_preview::JobPool;
use lightcraft_raster::{Histogram, Rgba8};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Slot {
    Thumb(PhotoId),
    /// A thumbnail's stand-in (embedded preview of an unedited raw).
    ThumbQuick(PhotoId),
    Main,
    /// The loupe zoomed past what `Main` can hold: the window of the frame that is on screen,
    /// rendered at the zoom scale and drawn over `Main` (the window geometry: `ui-egui`'s `region`).
    Region,
    /// The Before side of a Before/After view zoomed the same way (see [`Slot::Region`]).
    RegionBefore,
    /// The loupe's stand-in until `Main` has the photo.
    Preview,
    Before,
    Compare(u8),
    /// Compare or Reference zoomed past what `Compare(i)` holds: the window of pane `i`'s frame
    /// that is on screen, drawn over it (`ui-egui`'s `region` and `panels::compare`).
    CompareRegion(u8),
    /// Background preparation of a neighbouring photo: its decoded source and view render are
    /// cached by the engine, and a texture is kept only for the phone loupe
    /// ([`Scheduler::keep_neighbours`]). 0 = next, 1 = previous.
    Prefetch(u8),
    /// A variant thumbnail (profile / preset browsers), by its job key ([`Session::variant_job`]).
    /// Kept in a bounded LRU ([`VARIANT_TEXTURES`]).
    Variant(u64),
    /// The loupe while hovering a preset or profile: the photo with the hovered look (nothing is
    /// committed).
    Hover,
    /// An import candidate's thumbnail in the import review dialog (by index).
    Import(u32),
    /// A thumbnail a camera made of one of its files, in the import review (by a hash of the
    /// camera's and the file's ids: the review's order changes as the camera catalogues). Shown
    /// as the frontend puts it there; never rendered.
    Camera(u64),
    /// The second window's view of the active photo.
    Second,
}

/// What a slot shows: the render's facts, and `view`, the frontend's form of its pixels (an egui
/// texture, an image lease…). Derefs to `view`, so a frontend reads its own fields directly.
pub struct Shown<V> {
    pub key: u64,
    pub photo: PhotoId,
    pub size: [usize; 2],
    pub histogram: Option<Histogram>,
    pub ms: f64,
    /// Set for stand-ins: where the image came from.
    pub quick: Option<QuickSource>,
    /// The request this picture answered (see [`lightcraft_preview::next_tick`]).
    pub request_id: u64,
    /// Rendered at draft quality (a slider drag in progress).
    pub draft: bool,
    pub view: V,
}

impl<V> Deref for Shown<V> {
    type Target = V;
    fn deref(&self) -> &V {
        &self.view
    }
}

impl<V> DerefMut for Shown<V> {
    fn deref_mut(&mut self) -> &mut V {
        &mut self.view
    }
}

/// Turns finished renders into what a frontend shows. Called on the thread that polls.
pub trait Present<V> {
    /// The first picture for `slot`.
    fn make(&mut self, slot: Slot, image: Rgba8) -> V;
    /// A newer picture for `slot`, which already shows `view`.
    fn update(&mut self, slot: Slot, image: Rgba8, view: &mut V);
}

/// What one [`Scheduler::poll`] did.
#[derive(Debug, Default)]
pub struct Polled {
    /// Slots whose picture changed, in the order they finished.
    pub shown: Vec<Slot>,
    /// Renders that failed (not background preparation): slot, photo, job key, why.
    pub failed: Vec<(Slot, PhotoId, u64, String)>,
    /// Poll again within this long: renders are in flight, or the idle trim is due.
    pub wake_after: Option<Duration>,
}

impl Polled {
    /// Did any slot get a new picture?
    pub fn changed(&self) -> bool {
        !self.shown.is_empty()
    }
}

/// Runs render jobs outside this thread's job pool (the browser build: Web Workers, each with its
/// own wasm instance). The [`Scheduler`] keeps the queue (priorities, per-slot de-duplication) and
/// hands jobs over one at a time.
pub trait RenderOffload {
    /// Start `job` if an executor is free; otherwise give it back (`Some`: it stays queued).
    fn try_start(&mut self, slot: Slot, job: RenderJob) -> Option<RenderJob>;
    /// Finished jobs since the last call: (slot, result, run time in ms).
    fn finished(&mut self) -> Vec<(Slot, RenderResult, f64)>;
}

struct Queued {
    slot: Slot,
    priority: u32,
    seq: u64,
    job: RenderJob,
}

/// How large a draft (the loupe's render while a slider drags) may be, learned from how long this
/// host takes to render one. The old fixed 0.6 scale is a few ms on a GPU for Exposure or
/// Highlights, but Temperature and Noise Reduction rebuild every spatial plane (30–100 ms even on
/// the GPU, `render_bench`), and the browser build has no GPU at all: it renders on one CPU thread at
/// 100–300 ns per pixel, where that scale is a few frames a second.
/// The scale is chosen once per drag and only goes down during it: a draft of another size starts
/// from a cold stage cache, so a scale that moved with every sample would be slower than either.
#[derive(Debug, Default)]
pub struct DraftBudget {
    /// Smoothed render time per output pixel (ms) of drafts that ran on a warm stage cache.
    ms_per_px: Option<f64>,
    /// Size of the last draft that finished: the next one of the same size was warm.
    last: Option<[usize; 2]>,
    /// The scale the drag in progress renders at.
    frozen: Option<f32>,
    /// The scale the latest drag rendered at (diagnostics).
    last_scale: Option<f32>,
}

impl DraftBudget {
    /// What a draft should take (ms): about two frames at 60 Hz. (A browser renders the drafts of a
    /// drag on several workers in turn, so this is the lag behind the slider, not the update rate.)
    const TARGET_MS: f64 = 40.0;
    /// Linear scale limits. The ceiling is the scale drafts always had.
    const MAX: f32 = 0.6;
    const MIN: f32 = 0.2;
    /// Before anything is measured: a CPU-only host (the browser) starts small.
    const GUESS: f32 = if cfg!(target_arch = "wasm32") { 0.35 } else { Self::MAX };

    /// A draft of `size` finished in `ms`.
    pub fn note(&mut self, size: [usize; 2], ms: f64) {
        let warm = self.last == Some(size);
        self.last = Some(size);
        let px = size[0] as f64 * size[1] as f64;
        if warm && ms > 0.0 && px > 0.0 {
            let c = ms / px;
            self.ms_per_px = Some(self.ms_per_px.map_or(c, |old| old + (c - old) * 0.3));
        }
    }

    /// The linear scale of a draft of a render that is `full_px` pixels at full size: 1 when no
    /// slider is dragging.
    pub fn scale(&mut self, interacting: bool, full_px: f64) -> f32 {
        if !interacting {
            // (the full-size render that follows replaces the stage cache: the next drag starts cold)
            self.frozen = None;
            self.last = None;
            return 1.0;
        }
        let want = match self.ms_per_px {
            Some(c) if c > 0.0 => ((Self::TARGET_MS / c / full_px.max(1.0)).sqrt() as f32).clamp(Self::MIN, Self::MAX),
            _ => Self::GUESS,
        };
        // (steps of 0.05, so the render size doesn't change with every sample)
        let want = if want.is_finite() { (want * 20.0).round() / 20.0 } else { Self::GUESS };
        let scale = match self.frozen {
            Some(f) if want >= f * 0.75 => f,
            _ => want,
        };
        self.frozen = Some(scale);
        self.last_scale = Some(scale);
        scale
    }

    /// The linear scale of a zoomed view's window draft, given the whole-frame draft's `scale`: windows
    /// hold only what is on screen and were always drafted at full size, which a fast host still does
    /// (`scale` at its ceiling); a slower one shrinks them in proportion to how far the frame's draft went down.
    pub fn window_scale(scale: f32) -> f32 {
        if scale.is_finite() { (scale / Self::MAX).clamp(Self::MIN / Self::MAX, 1.0) } else { 1.0 }
    }

    /// For diagnostics: the scale of the latest drag, and the learned render cost (ms per megapixel).
    pub fn report(&self) -> (Option<f32>, Option<f64>) {
        (self.last_scale, self.ms_per_px.map(|c| c * 1e6))
    }
}

/// How far over the budget the open photo's window and before caches may run before they are cut.
const HARD_FACTOR: usize = 4;

/// Which stage caches to clear, in order, so that the rest fit `budget` bytes.
///
/// What nothing is looking at goes first, the cheapest to rebuild and least reused first (hover,
/// compare, second window, background work). The open photo's own views are what a slider drag
/// reuses on every tick, and a cache cleared every frame is rebuilt every frame (a drag on a
/// 2800 px canvas got 3.6x slower), so they stay — counted and reported — up to
/// [`HARD_FACTOR`] times the budget; past that (a runaway: 8:1 on a huge canvas, both sides of
/// Before/After) the before side's and the zoom window's go, the before side first. The
/// whole-frame view's cache is never named. Empty caches are never named.
pub fn stage_trim_order(sizes: &[(Slot, usize)], budget: usize) -> Vec<Slot> {
    let idle_rank = |s: Slot| match s {
        Slot::Hover => 0,
        Slot::Compare(_) | Slot::CompareRegion(_) => 2,
        Slot::Second => 3,
        _ => 5,
    };
    let view_rank = |s: Slot| match s {
        Slot::RegionBefore => Some(0),
        Slot::Before => Some(1),
        Slot::Region => Some(2),
        _ => None,
    };
    let is_view = |s: Slot| matches!(s, Slot::Main) || view_rank(s).is_some();
    let mut held: usize = sizes.iter().map(|(_, b)| *b).fold(0, usize::saturating_add);
    let mut out = Vec::new();
    let mut idle: Vec<(Slot, usize)> = sizes.iter().copied().filter(|(s, b)| *b > 0 && !is_view(*s)).collect();
    idle.sort_by_key(|(s, _)| idle_rank(*s));
    for (slot, bytes) in idle {
        if held <= budget {
            break;
        }
        held = held.saturating_sub(bytes);
        out.push(slot);
    }
    let hard = budget.saturating_mul(HARD_FACTOR);
    let mut views: Vec<(Slot, usize, u8)> = sizes.iter().filter_map(|(s, b)| view_rank(*s).filter(|_| *b > 0).map(|r| (*s, *b, r))).collect();
    views.sort_by_key(|(_, _, r)| *r);
    for (slot, bytes, _) in views {
        if held <= hard {
            break;
        }
        held = held.saturating_sub(bytes);
        out.push(slot);
    }
    out
}

/// The share of the memory budget the per-view stage caches (the loupe's, the zoom window's, the
/// before / hover renders') may hold together before the idle ones are cleared (see
/// [`stage_trim_order`] for the open photo's own): they are the decoded-image sized buffers that the
/// engine's own caches do not count.
const STAGE_BUDGET_SHARE: usize = 4;

/// Variant thumbnails kept as textures (LRU), when nothing asks for more.
pub const VARIANT_TEXTURES: usize = 96;
/// The most a view can raise that to ([`Scheduler::want_variants`]): a screenful of small faces is a few hundred pictures of
/// a few tens of kilobytes each.
pub const MAX_VARIANT_TEXTURES: usize = 800;
/// Priority of variant thumbnail jobs: below on-screen grid thumbnails and the loupe.
const VARIANT_PRIORITY: u32 = 6;
/// Rendered thumbnails replacing embedded previews: after everything on screen.
pub const BACKGROUND_THUMB_PRIORITY: u32 = 3;
/// A thumbnail in the grid's reach above and below the screen (everything above this is on screen or wanted now).
pub const PREFETCH_THUMB_PRIORITY: u32 = 5;

/// Whether a photo's grid thumbnail is already the picture it should show, so the render of the raw
/// behind it is skipped. `embedded_is_final` is the engine's answer
/// ([`lightcraft_engine::media::Session::embedded_thumb_is_final`]: the photo is an unedited raw
/// whose camera JPEG the viewer is showing) and `from_embedded` says the stand-in actually came from
/// the embedded preview rather than a cached render of the file.
///
/// Skipping matters where the file is expensive to read: decoding a 56 MB Fuji RAF to draw a 512 px
/// thumbnail — which the 4 416 × 2 944 embedded JPEG already covers — cost 45 s a photo over a
/// 1.2 MB/s share, and the queue it built delayed the render the user was waiting for.
pub fn skips_the_real_render(embedded_is_final: bool, from_embedded: bool) -> bool {
    embedded_is_final && from_embedded
}

/// The render queue of a frontend: what each slot asked for, what it shows (`textures`, in the
/// frontend's form `V`), the stage caches of its views, and the job pool that runs the renders.
pub struct Scheduler<V> {
    request_ids: HashMap<Slot, u64>,
    /// Request id of the pixels each interactive view (loupe, before, hover) shows: a draft that
    /// finishes after a newer request was made still replaces older pixels, so a long slider drag
    /// keeps updating the loupe instead of waiting for the drag to pause.
    shown_ids: HashMap<Slot, u64>,
    /// Requests made before this id belong to a library or preview cache that is gone.
    epoch: u64,
    preview_generation: Option<(std::sync::Weak<lightcraft_preview::PreviewCache>, u64)>,
    /// Last thumbnail inputs; weak identities do not retain photo histories or force deep clones.
    /// (photo, size bucket, job key, whether the request went through an embedded stand-in)
    thumb_inputs: HashMap<PhotoId, (std::sync::Weak<lightcraft_catalog::Photo>, usize, u64, bool)>,
    /// Photos whose grid thumbnail is the camera's embedded preview itself (no render behind it).
    embedded_thumbs: HashMap<PhotoId, (std::sync::Weak<lightcraft_catalog::Photo>, usize, u64)>,
    pool: JobPool<Slot, RenderResult>,
    /// Jobs run elsewhere (see [`RenderOffload`]); `queue` holds the ones not started yet.
    offload: Option<Box<dyn RenderOffload>>,
    queue: Vec<Queued>,
    seq: u64,
    /// Slot → (key, priority) of the request in flight (queued or running).
    pending: HashMap<Slot, (u64, u32)>,
    /// What each slot shows.
    pub textures: HashMap<Slot, Shown<V>>,
    pub last_main_ms: f64,
    /// How big the loupe's drafts may be on this host (see [`DraftBudget`]).
    draft: DraftBudget,
    /// Request ids of the loupe's latest drafts: their results teach `draft` the host's speed.
    draft_ids: Vec<u64>,
    /// Request ids of the latest draft renders of any slot (a picture says whether it is one).
    drafted: Vec<u64>,
    /// Jobs finished since start (for inspect/perf).
    pub completed: u64,
    /// Keep the renders of the neighbouring photos ([`Slot::Prefetch`]) as textures, for the phone
    /// loupe to slide in; elsewhere they only warm the engine's caches.
    pub keep_neighbours: bool,
    /// Per-view intermediate results (loupe and "before"), so slider drags redo only what changed.
    stages: HashMap<Slot, Arc<StageCache>>,
    /// Slot → key of the last quick job requested for it (each is tried once).
    quick_tried: HashMap<Slot, u64>,
    /// Jobs that failed (unreadable or missing file…), by slot and job key, with the error: not
    /// requested again until the key changes (an edit, another size) — the grid asks every frame.
    failed: HashMap<Slot, (u64, String, u64)>,
    /// The catalog revision seen at the last poll: a failure is retried once the catalog changes
    /// (a relinked or re-imported file).
    catalog_rev: u64,
    /// Prefetch slot → key of the last job submitted (each runs once).
    prefetched: HashMap<Slot, u64>,
    /// Variant key → frame it was last asked for (LRU of [`Slot::Variant`] textures).
    variant_used: HashMap<u64, u64>,
    /// How many variants the last frame's views showed at once (see [`Self::want_variants`]).
    variant_want: usize,
    /// Frames polled so far.
    frame: u64,
    /// Stage caches cleared to stay in budget (see [`stage_trim_order`]).
    stages_trimmed: u64,
    /// A budget other than the memory budget's share (tests).
    pub stage_budget_override: Option<usize>,
    /// The monitor profile textures are made for (`app.displayProfile`; `None`: sRGB).
    display: Option<Arc<lightcraft_engine::display::Display>>,
    /// Since when nothing has been pending, and whether the GPU pool was trimmed since.
    #[cfg(not(target_arch = "wasm32"))]
    idle: Option<(std::time::Instant, bool)>,
}

/// After this long without renders the GPU renderer's pool of recycled buffers is freed.
#[cfg(not(target_arch = "wasm32"))]
const IDLE_TRIM: std::time::Duration = std::time::Duration::from_secs(3);

impl Slot {
    /// The slot's name in the host protocol (`thumb:42`, `main`, `compare:1`…; [`Self::parse`]).
    pub fn name(self) -> String {
        match self {
            Slot::Thumb(id) => format!("thumb:{}", id.0),
            Slot::ThumbQuick(id) => format!("thumbQuick:{}", id.0),
            Slot::Main => "main".into(),
            Slot::Region => "region".into(),
            Slot::RegionBefore => "regionBefore".into(),
            Slot::Preview => "preview".into(),
            Slot::Before => "before".into(),
            Slot::Compare(i) => format!("compare:{i}"),
            Slot::CompareRegion(i) => format!("compareRegion:{i}"),
            Slot::Prefetch(i) => format!("prefetch:{i}"),
            Slot::Variant(k) => format!("variant:{k}"),
            Slot::Hover => "hover".into(),
            Slot::Import(i) => format!("import:{i}"),
            Slot::Camera(k) => format!("camera:{k}"),
            Slot::Second => "second".into(),
        }
    }

    /// The slot [`Self::name`] names.
    pub fn parse(name: &str) -> Option<Slot> {
        let (kind, n) = name.split_once(':').unwrap_or((name, ""));
        let num = || n.parse::<u64>().ok();
        let small = || n.parse::<u8>().ok();
        Some(match kind {
            "thumb" => Slot::Thumb(PhotoId(num()?)),
            "thumbQuick" => Slot::ThumbQuick(PhotoId(num()?)),
            "main" => Slot::Main,
            "region" => Slot::Region,
            "regionBefore" => Slot::RegionBefore,
            "preview" => Slot::Preview,
            "before" => Slot::Before,
            "compare" => Slot::Compare(small()?),
            "compareRegion" => Slot::CompareRegion(small()?),
            "prefetch" => Slot::Prefetch(small()?),
            "variant" => Slot::Variant(num()?),
            "hover" => Slot::Hover,
            "import" => Slot::Import(n.parse().ok()?),
            "camera" => Slot::Camera(num()?),
            "second" => Slot::Second,
            _ => return None,
        })
    }

    /// An interactive view of the open photo: slider drags redo only the stages they feed, and a
    /// draft that finishes late still replaces older pixels.
    pub fn is_view(self) -> bool {
        matches!(self, Slot::Main | Slot::Region | Slot::RegionBefore | Slot::Before | Slot::Hover)
    }
}

impl<V> Default for Scheduler<V> {
    fn default() -> Self {
        let threads = if cfg!(target_arch = "wasm32") { 0 } else { JobPool::<Slot, RenderResult>::default_threads().min(6) };
        Self::with_worker_threads(threads)
    }
}

impl<V> Scheduler<V> {
    /// Explicit worker count for hosts and bounded validation runs (one to six on native).
    /// Browser hosts continue to use inline jobs or their installed worker offload.
    pub fn with_worker_threads(threads: usize) -> Self {
        let threads = if cfg!(target_arch = "wasm32") { 0 } else { threads.clamp(1, 6) };
        Scheduler {
            request_ids: HashMap::new(),
            shown_ids: HashMap::new(),
            epoch: 0,
            preview_generation: None,
            pool: JobPool::new(threads),
            thumb_inputs: HashMap::new(),
            embedded_thumbs: HashMap::new(),
            offload: None,
            queue: Vec::new(),
            seq: 0,
            pending: HashMap::new(),
            textures: HashMap::new(),
            last_main_ms: 0.0,
            draft: DraftBudget::default(),
            draft_ids: Vec::new(),
            drafted: Vec::new(),
            completed: 0,
            keep_neighbours: false,
            stages: HashMap::new(),
            quick_tried: HashMap::new(),
            failed: HashMap::new(),
            catalog_rev: 0,
            prefetched: HashMap::new(),
            variant_used: HashMap::new(),
            variant_want: 0,
            frame: 0,
            stages_trimmed: 0,
            stage_budget_override: None,
            display: None,
            #[cfg(not(target_arch = "wasm32"))]
            idle: None,
        }
    }
}

impl<V> Scheduler<V> {
    /// Call `notify` (from a worker thread) whenever a render finishes, so a host that sleeps
    /// between polls wakes up for it (see [`JobPool::set_notify`]).
    pub fn set_notify(&self, notify: Option<lightcraft_preview::pool::Notify>) {
        self.pool.set_notify(notify);
    }

    /// Would asking for this thumbnail again (at `priority`) change nothing?
    pub fn thumb_current(&self, photo: &Arc<lightcraft_catalog::Photo>, bucket: usize, priority: u32) -> bool {
        let Some(&(ref old, size, key, quick)) = self.thumb_inputs.get(&photo.id) else { return false };
        if old.as_ptr() != Arc::as_ptr(photo) || size != bucket {
            return false;
        }
        let slot = Slot::Thumb(photo.id);
        if quick && !self.textures.contains_key(&slot) {
            // Stand-in path (`grid::request_thumb`): the embedded preview is still on its way, or
            // the real thumbnail is requested at the background priority whatever the caller's.
            return self.pending.get(&Slot::ThumbQuick(photo.id)).is_some_and(|p| p.0 == key)
                || !self.request_needed(slot, key, BACKGROUND_THUMB_PRIORITY);
        }
        !self.request_needed(slot, key, priority)
    }

    /// Request a grid/filmstrip thumbnail of `id` (`size` px) at `priority`. An unedited raw without a
    /// thumbnail gets its embedded camera preview, and the render of the raw behind it follows in
    /// the background — except where that render would only redraw what the embedded JPEG already
    /// shows (see [`lightcraft_engine::media::Session::embedded_thumb_is_final`]), which for a folder
    /// of raws on a network share is the difference between reading every file whole and reading a
    /// preview each. Cheap to call every frame for every cell on screen. Returns whether a render
    /// job was built.
    pub fn request_thumb(&mut self, session: &mut Session, id: PhotoId, size: usize, priority: u32) -> bool {
        // a slider drag changes the photo every frame: its thumbnail keeps what it shows (a render per
        // tick would take a worker from the loupe) and catches up once the drag ends
        if session.interaction.as_ref().is_some_and(|i| i.photo == id) && self.textures.contains_key(&Slot::Thumb(id)) {
            return false;
        }
        let bucket = lightcraft_engine::media::thumb_bucket(size);
        let shown = priority > PREFETCH_THUMB_PRIORITY;
        // A synced photo whose preview hasn't come down yet has no picture, and the render that failed for want of it
        // is remembered (`thumb_current`): without this the grid would ask for it once and the download would be
        // forgotten a few frames later, leaving the cell empty for good while it stays on screen.
        if self.thumb(id).is_none() {
            session.note_thumb_ask(id, shown);
        }
        if let Some(photo) = session.catalog.photo(id)
            && self.thumb_current(photo, bucket, priority)
        {
            return false;
        }
        // The camera's embedded preview is this photo's thumbnail (unedited raw): once its quick render
        // has arrived there is nothing left to ask for. Without this the grid would rebuild the render
        // job — and its cache key — on every frame, which is what the embedded-thumbnail record settles.
        if let Some(photo) = session.catalog.photo(id)
            && (self.thumb_quick_is_final(photo, bucket, priority) || self.thumb_quick_pending(photo, bucket))
        {
            return false;
        }
        let Some(job) = session.thumb_job_for(id, size, shown) else { return false };
        let quick = if self.textures.contains_key(&Slot::Thumb(id)) { None } else { session.quick_thumb_job(&job) };
        // A cached render is a stand-in for whatever comes next; an embedded camera preview of an
        // unedited raw is the picture itself, so no sensor decode is scheduled behind it.
        let embedded_is_final =
            quick.is_some() && session.catalog.photo(id).is_some_and(|p| skips_the_real_render(session.embedded_thumb_is_final(p), true));
        if let Some(photo) = session.catalog.photo(id) {
            if embedded_is_final {
                self.remember_embedded_thumb(photo, bucket, job.key);
            } else {
                self.remember_thumb(photo, bucket, job.key, quick.is_some());
            }
        }
        match quick {
            Some(q) => {
                self.request_quick(Slot::ThumbQuick(id), q, priority + 1);
                if !embedded_is_final && !self.is_pending(Slot::ThumbQuick(id)) {
                    self.request(Slot::Thumb(id), job, BACKGROUND_THUMB_PRIORITY);
                }
            }
            None => self.request(Slot::Thumb(id), job, priority),
        }
        true
    }

    /// Remember the inputs of the thumbnail just requested (`quick`: through a stand-in).
    /// Whether the camera's embedded preview *is* this photo's thumbnail: its quick render arrived
    /// (so `Slot::ThumbQuick` holds the picture) and no edit has moved the photo on to a settings
    /// hash of its own. The grid stops here instead of decoding the raw behind a thumbnail the
    /// embedded JPEG already covers — see `grid::skips_the_real_render`.
    pub fn thumb_quick_is_final(&self, photo: &Arc<lightcraft_catalog::Photo>, bucket: usize, priority: u32) -> bool {
        let Some((old, size, key)) = self.embedded_thumbs.get(&photo.id) else { return false };
        old.as_ptr() == Arc::as_ptr(photo) && *size == bucket && !self.request_needed(Slot::ThumbQuick(photo.id), *key, priority)
    }

    /// Whether this photo's embedded-preview thumbnail is being fetched right now: the grid has
    /// asked, and neither the picture nor a render behind it is owed.
    pub fn thumb_quick_pending(&self, photo: &Arc<lightcraft_catalog::Photo>, bucket: usize) -> bool {
        let Some((old, size, _, quick)) = self.thumb_inputs.get(&photo.id) else { return false };
        old.as_ptr() == Arc::as_ptr(photo) && *size == bucket && *quick && self.pending.contains_key(&Slot::ThumbQuick(photo.id))
    }

    /// Remember that the camera's embedded preview is this photo's final thumbnail, so the grid's
    /// per-frame requests stop at it (the quick render's key, which [`Self::thumb_quick_is_final`]
    /// compares against).
    pub fn remember_embedded_thumb(&mut self, photo: &Arc<lightcraft_catalog::Photo>, bucket: usize, key: u64) {
        self.remember_thumb(photo, bucket, key, true);
        self.embedded_thumbs.insert(photo.id, (Arc::downgrade(photo), bucket, key));
    }

    pub fn remember_thumb(&mut self, photo: &Arc<lightcraft_catalog::Photo>, bucket: usize, key: u64, quick: bool) {
        if self.thumb_inputs.len() >= 1024 && !self.thumb_inputs.contains_key(&photo.id) {
            self.thumb_inputs.retain(|_, (p, _, _, _)| p.strong_count() > 0);
            if self.thumb_inputs.len() >= 1024
                && let Some(id) = self.thumb_inputs.keys().next().copied()
            {
                self.thumb_inputs.remove(&id);
            }
        }
        self.thumb_inputs.insert(photo.id, (Arc::downgrade(photo), bucket, key, quick));
    }

    /// Views that render straight into the display profile's primaries (wide gamut); everything
    /// else is rendered for sRGB and converted ([`lightcraft_engine::display::present`]).
    fn renders_for_display(slot: Slot) -> bool {
        matches!(
            slot,
            Slot::Main | Slot::Region | Slot::Before | Slot::RegionBefore | Slot::Hover | Slot::Compare(_) | Slot::CompareRegion(_) | Slot::Second
        )
    }

    /// `job` as [`Self::request`] would run it for `slot` (its key included): views render for
    /// the display profile in use.
    pub fn job_for(&self, slot: Slot, job: RenderJob) -> RenderJob {
        if Self::renders_for_display(slot) { job.with_display(self.display.clone()) } else { job }
    }

    /// The display profile in use (id).
    pub fn display_id(&self) -> Option<u64> {
        self.display.as_ref().map(|d| d.id())
    }

    /// The display profile textures are made for (`None`: sRGB).
    pub fn display(&self) -> Option<&Arc<lightcraft_engine::display::Display>> {
        self.display.as_ref()
    }

    /// Show previews through `display` from now on (`app.displayProfile`): when it changes, every
    /// texture is made again for the new one.
    pub fn set_display(&mut self, display: Option<Arc<lightcraft_engine::display::Display>>) {
        if display.as_ref().map(|d| d.id()) != self.display_id() {
            self.forget_all();
        }
        self.display = display;
    }

    fn request_needed(&self, slot: Slot, key: u64, priority: u32) -> bool {
        // A matching old texture must not mask a different newer request (e.g. undo).
        if let Some(&(k, p)) = self.pending.get(&slot) {
            return k != key || (p != priority && self.is_queued(slot));
        }
        if self.textures.get(&slot).is_some_and(|t| t.key == key) || self.failed.get(&slot).is_some_and(|f| f.0 == key && f.2 == self.catalog_rev) {
            return false;
        }
        true
    }
    /// Stop rendering for `slot` and forget what it shows: its queued job is dropped, a running
    /// one's result is ignored, and its stage cache freed (a view the frontend closed).
    pub fn cancel(&mut self, slot: Slot) {
        self.pool.reprioritize(|s, p| if *s == slot { None } else { Some(p) });
        self.queue.retain(|q| q.slot != slot);
        self.pending.remove(&slot);
        self.request_ids.remove(&slot);
        self.shown_ids.remove(&slot);
        self.quick_tried.remove(&slot);
        self.prefetched.remove(&slot);
        self.release(slot);
    }

    /// Free a view slot's texture and stage cache (the view is no longer shown).
    pub fn release(&mut self, slot: Slot) {
        self.textures.remove(&slot);
        self.stages.remove(&slot);
    }

    /// Run jobs through `offload` from now on (instead of the job pool / inline).
    pub fn set_offload(&mut self, offload: Box<dyn RenderOffload>) {
        self.offload = Some(offload);
    }

    fn is_queued(&self, slot: Slot) -> bool {
        self.queue.iter().any(|q| q.slot == slot) || self.pool.is_queued(slot)
    }

    /// The linear scale the loupe renders at: 1 unless a slider is dragging, then what this host
    /// renders in time (see [`DraftBudget`]). `full_px`: the pixels of the undrafted render.
    pub fn draft_scale(&mut self, interacting: bool, full_px: f64) -> f32 {
        self.draft.scale(interacting, full_px)
    }

    /// What the draft budget has learned (see [`DraftBudget::report`]).
    pub fn draft_report(&self) -> (Option<f32>, Option<f64>) {
        self.draft.report()
    }

    /// Is the slot's current texture (or pending request) already for `key`?
    /// Key of what `slot` shows or is rendering next (the pending request wins).
    pub fn wanted(&self, slot: Slot) -> Option<u64> {
        self.pending.get(&slot).map(|p| p.0).or_else(|| self.textures.get(&slot).map(|t| t.key))
    }

    pub fn is_current(&self, slot: Slot, key: u64) -> bool {
        self.wanted(slot) == Some(key)
    }

    /// Request a render for `slot` (no-op if already current or pending at the same priority).
    pub fn request(&mut self, slot: Slot, job: RenderJob, priority: u32) {
        let mut job = self.job_for(slot, job);
        if !self.request_needed(slot, job.key, priority) {
            return;
        }
        self.pending.insert(slot, (job.key, priority));
        job.request_id = lightcraft_preview::next_tick();
        self.request_ids.insert(slot, job.request_id);
        if job.request.quality == lightcraft_engine::pipeline::Quality::Draft {
            if self.drafted.len() >= 64 {
                self.drafted.remove(0);
            }
            self.drafted.push(job.request_id);
            if slot == Slot::Main {
                if self.draft_ids.len() >= 32 {
                    self.draft_ids.remove(0);
                }
                self.draft_ids.push(job.request_id);
            }
        }
        // (an offload keeps its own per-view stage caches; the flag tells it to)
        let job = if slot.is_view() { job.with_stages(self.stages.entry(slot).or_default().clone()) } else { job };
        if self.offload.is_some() {
            self.seq += 1;
            self.queue.retain(|q| q.slot != slot);
            self.queue.push(Queued { slot, priority, seq: self.seq, job });
            self.dispatch(); // an idle worker starts now rather than next frame
            return;
        }
        let key = job.key;
        let background = matches!(slot, Slot::Thumb(_) | Slot::ThumbQuick(_) | Slot::Prefetch(_));
        let display = self.display.clone();
        let failed = Failed::of(&job);
        self.pool.submit(
            slot,
            key,
            priority,
            Box::new(move || {
                let run = || if background { lightcraft_engine::memory::in_background(|| job.run()) } else { job.run() };
                run_for(failed, run, &display)
            }),
        );
    }

    /// Request a stand-in for `slot` (once per job key).
    pub fn request_quick(&mut self, slot: Slot, mut job: QuickJob, priority: u32) {
        if self.quick_tried.get(&slot) == Some(&job.key) {
            return;
        }
        self.quick_tried.insert(slot, job.key);
        self.pending.insert(slot, (job.key, priority));
        job.request_id = lightcraft_preview::next_tick();
        self.request_ids.insert(slot, job.request_id);
        let key = job.key;
        let display = self.display.clone();
        let failed =
            Failed { request_id: job.request_id, source_key: None, photo: job.photo, level: lightcraft_engine::media::SourceLevel::Thumb, key };
        self.pool.submit(slot, key, priority, Box::new(move || run_for(failed, || job.run(), &display)));
    }

    /// Prepare a photo in the background (a [`Slot::Prefetch`] job runs once per key; a newer one
    /// replaces it while queued). Memory stays bounded by the engine's source caches.
    pub fn prefetch(&mut self, slot: Slot, mut job: RenderJob, priority: u32) {
        if self.prefetched.get(&slot) == Some(&job.key) {
            return;
        }
        self.prefetched.insert(slot, job.key);
        self.pending.insert(slot, (job.key, priority));
        job.request_id = lightcraft_preview::next_tick();
        self.request_ids.insert(slot, job.request_id);
        let key = job.key;
        let failed = Failed::of(&job);
        self.pool.submit(slot, key, priority, Box::new(move || run_for(failed, || lightcraft_engine::memory::in_background(|| job.run()), &None)));
    }

    /// Is a request for `slot` queued or running?
    /// Drop every texture and cached stage (another library was opened: photo ids changed meaning).
    pub fn forget_all(&mut self) {
        self.request_ids.clear();
        self.shown_ids.clear();
        self.epoch = lightcraft_preview::next_tick();
        self.preview_generation = None;
        self.pool.reprioritize(|_, _| None);
        self.thumb_inputs.clear();
        self.textures.clear();
        self.stages.clear();
        self.quick_tried.clear();
        self.failed.clear();
        self.prefetched.clear();
        self.pending.clear();
        self.queue.clear();
    }

    pub fn is_pending(&self, slot: Slot) -> bool {
        self.pending.contains_key(&slot)
    }

    /// The app is closing: drop the jobs that have not started and wait up to `timeout` for the
    /// ones running on the worker threads. Returns whether all of them have ended. Nothing is
    /// rendered afterwards.
    pub fn shutdown(&mut self, timeout: std::time::Duration) -> bool {
        self.queue.clear();
        self.pool.shutdown(timeout)
    }

    /// Worker threads that are still alive (0 before the first render).
    pub fn live_workers(&self) -> usize {
        self.pool.live_workers()
    }

    /// The texture to show for a grid/filmstrip thumbnail: the rendered one, else its stand-in.
    pub fn thumb(&self, id: PhotoId) -> Option<&Shown<V>> {
        self.textures.get(&Slot::Thumb(id)).or_else(|| self.textures.get(&Slot::ThumbQuick(id)))
    }

    pub fn queued(&self) -> usize {
        self.pool.queued() + self.queue.len()
    }

    /// Hand queued jobs to the offload (best first) while it has room.
    fn dispatch(&mut self) {
        let Some(off) = self.offload.as_mut() else { return };
        while let Some(i) = self.queue.iter().enumerate().max_by_key(|(_, q)| (q.priority, q.seq)).map(|(i, _)| i) {
            let q = self.queue.swap_remove(i);
            if let Some(job) = off.try_start(q.slot, q.job) {
                self.queue.push(Queued { job, ..q });
                break;
            }
        }
    }

    /// Requests queued or running.
    pub fn in_flight(&self) -> usize {
        self.pending.len()
    }

    /// Why the last render for `slot` failed (until a render with another key succeeds).
    pub fn failure(&self, slot: Slot) -> Option<&str> {
        self.failed.get(&slot).map(|f| f.1.as_str())
    }

    /// The slots with a render pending (diagnostics: `ui.inspect`).
    pub fn pending_slots(&self) -> Vec<String> {
        self.pending.keys().map(|s| format!("{s:?}")).collect()
    }

    /// Thumbnail textures currently loaded.
    /// A variant thumbnail ([`Slot::Variant`]): its texture once rendered; until then the job is
    /// queued at low priority (below on-screen grid thumbnails). Call it every frame the variant
    /// is visible: variants not asked for during the last frames leave the queue, and textures
    /// beyond [`VARIANT_TEXTURES`] are evicted least recently used first.
    pub fn variant(&mut self, job: RenderJob) -> Option<&Shown<V>> {
        let key = job.key;
        let slot = Slot::Variant(key);
        self.variant_used.insert(key, self.frame);
        if !self.textures.contains_key(&slot) {
            self.request(slot, job, VARIANT_PRIORITY);
        }
        self.textures.get(&slot)
    }

    /// A view that shows `n` variants at once (the People view's faces) says so each frame, so the cache holds them all:
    /// with a fixed budget a screen of more than [`VARIANT_TEXTURES`] pictures evicts, and re-requests, the same few every
    /// frame, and those tiles stay blank. The budget is the most asked for in the last frame, a quarter more, up to
    /// [`MAX_VARIANT_TEXTURES`].
    pub fn want_variants(&mut self, n: usize) {
        self.variant_want = self.variant_want.max(n);
    }

    /// How many variant textures are kept now.
    fn variant_budget(&self) -> usize {
        VARIANT_TEXTURES.max(self.variant_want + self.variant_want / 4).min(MAX_VARIANT_TEXTURES)
    }

    /// Variant thumbnail textures currently loaded.
    pub fn variant_textures(&self) -> usize {
        self.textures.keys().filter(|s| matches!(s, Slot::Variant(_))).count()
    }

    /// Drop variant jobs nobody asked for in the last frames, and the least recently used
    /// variant textures beyond the budget.
    fn evict_variants(&mut self) {
        let now = self.frame;
        let stale = |used: Option<&u64>| used.is_none_or(|f| now.saturating_sub(*f) > 2);
        let used = &self.variant_used;
        let dropped = self.pool.reprioritize(|s, p| match s {
            Slot::Variant(k) if stale(used.get(k)) => None,
            _ => Some(p),
        });
        for s in dropped {
            self.pending.remove(&s);
            self.request_ids.remove(&s);
        }
        let request_ids = &mut self.request_ids;
        let pending = &mut self.pending;
        self.queue.retain(|q| match q.slot {
            Slot::Variant(k) if stale(used.get(&k)) => {
                pending.remove(&q.slot);
                request_ids.remove(&q.slot);
                false
            }
            _ => true,
        });
        let budget = self.variant_budget();
        self.variant_want = 0;
        let n = self.variant_textures();
        if n > budget {
            let mut have: Vec<(u64, u64)> = self
                .textures
                .keys()
                .filter_map(|s| if let Slot::Variant(k) = s { Some((self.variant_used.get(k).copied().unwrap_or(0), *k)) } else { None })
                .collect();
            have.sort_unstable();
            for (_, k) in have.into_iter().take(n - budget) {
                self.textures.remove(&Slot::Variant(k));
                self.variant_used.remove(&k);
            }
        }
        if self.variant_used.len() > 4 * budget {
            let textures = &self.textures;
            let pending = &self.pending;
            self.variant_used.retain(|k, _| textures.contains_key(&Slot::Variant(*k)) || pending.contains_key(&Slot::Variant(*k)));
        }
    }

    pub fn thumb_textures(&self) -> usize {
        self.textures.keys().filter(|s| matches!(s, Slot::Thumb(_) | Slot::ThumbQuick(_))).count()
    }

    /// A synced photo's preview came down from the server: the render of its thumbnail that failed for want of it
    /// ("still downloading") is no longer what the grid should remember — asking again builds the job with its new
    /// render key, and the cell draws the picture. (The library didn't change: a revision of it is what ends other failures.)
    fn preview_arrived(&mut self, session: &mut Session) {
        match session.media.take_arrived() {
            Some(ids) => {
                for id in ids {
                    self.failed.remove(&Slot::Thumb(id));
                    self.thumb_inputs.remove(&id);
                }
            }
            None => {
                self.failed.retain(|slot, _| !matches!(slot, Slot::Thumb(_)));
                self.thumb_inputs.clear();
            }
        }
    }

    /// Collect finished jobs into what the slots show (through `present`). The answer says what
    /// changed and when to poll again.
    pub fn poll(&mut self, session: &mut Session, present: &mut dyn Present<V>) -> Polled {
        let cache = &session.media.rendered;
        let generation = cache.generation();
        if self.preview_generation.as_ref().is_some_and(|(old, g)| old.as_ptr() != Arc::as_ptr(cache) || *g != generation) {
            self.forget_all();
        }
        self.preview_generation = Some((Arc::downgrade(cache), generation));
        self.catalog_rev = session.catalog.revision;
        self.preview_arrived(session);
        self.frame += 1;
        self.trim_stages();
        // wasm: run one job per frame on this thread, timed with the host clock
        #[cfg(target_arch = "wasm32")]
        let inline_ms = {
            let t0 = web_time::Instant::now();
            self.pool.run_inline(1);
            t0.elapsed().as_secs_f64() * 1000.0
        };
        #[cfg(not(target_arch = "wasm32"))]
        let inline_ms = 0.0;
        let mut finished = Vec::new();
        while let Some(done) = self.pool.try_recv() {
            finished.push((done.slot, done.result, if done.ms > 0.0 { done.ms } else { inline_ms }));
        }
        if let Some(off) = self.offload.as_mut() {
            finished.extend(off.finished());
        }
        self.dispatch();
        let mut polled = Polled::default();
        for (mut slot, mut r, ms) in finished {
            self.completed += 1;
            // made for another display profile than the one in use: obsolete
            if r.display.is_some_and(|id| Some(id) != self.display_id()) {
                continue;
            }
            // Ignore obsolete pixels, failures and decoded sources, even for the same render key;
            // an interactive view still takes a superseded draft that is newer than what it
            // shows, and the source it decoded: a drag supersedes every request before it
            // finishes, and a source thrown away with them is decoded again for every tick.
            if self.request_ids.get(&slot) != Some(&r.request_id) {
                let newer_draft = slot.is_view()
                    && self.request_ids.contains_key(&slot)
                    && r.request_id >= self.epoch
                    && self.shown_ids.get(&slot).is_none_or(|shown| r.request_id > *shown)
                    && r.rendered.is_ok();
                if !newer_draft {
                    continue;
                }
                session.accept(&r);
            } else {
                self.request_ids.remove(&slot);
                session.accept(&r);
            }
            if slot.is_view() {
                self.shown_ids.insert(slot, r.request_id);
            }
            // (results from an offload, which doesn't know the display profile: converted here)
            if let Some(d) = &self.display {
                lightcraft_engine::display::present(&mut r, d);
            }
            if self.pending.get(&slot).is_some_and(|p| p.0 == r.key) {
                self.pending.remove(&slot);
            }
            let rendered = match r.rendered {
                Ok(x) => {
                    self.failed.remove(&slot);
                    x
                }
                Err(e) => {
                    if !matches!(slot, Slot::Prefetch(_)) {
                        log::warn!("render {slot:?}: {e}");
                        polled.failed.push((slot, r.photo, r.key, e.clone()));
                        self.failed.insert(slot, (r.key, e, self.catalog_rev));
                    }
                    continue;
                }
            };
            if matches!(slot, Slot::Prefetch(_)) && !self.keep_neighbours {
                continue;
            }
            if let Slot::ThumbQuick(id) = slot {
                if self.textures.contains_key(&Slot::Thumb(id)) {
                    continue; // the real thumbnail won the race
                }
                if r.quick == Some(QuickSource::Cached) {
                    // the photo's own cached thumbnail: final
                    slot = Slot::Thumb(id);
                }
            }
            if let Slot::Thumb(id) = slot {
                self.textures.remove(&Slot::ThumbQuick(id));
            }
            let size = [rendered.image.width, rendered.image.height];
            let draft = self.drafted.contains(&r.request_id);
            match self.textures.get_mut(&slot) {
                Some(t) => {
                    present.update(slot, rendered.image, &mut t.view);
                    t.key = r.key;
                    t.photo = r.photo;
                    t.size = size;
                    t.histogram = Some(rendered.histogram);
                    t.ms = ms;
                    t.quick = r.quick;
                    t.request_id = r.request_id;
                    t.draft = draft;
                }
                None => {
                    let view = present.make(slot, rendered.image);
                    let shown = Shown {
                        key: r.key,
                        photo: r.photo,
                        size,
                        histogram: Some(rendered.histogram),
                        ms,
                        quick: r.quick,
                        request_id: r.request_id,
                        draft,
                        view,
                    };
                    self.textures.insert(slot, shown);
                }
            }
            if slot == Slot::Main {
                self.last_main_ms = ms;
                if let Some(i) = self.draft_ids.iter().position(|id| *id == r.request_id) {
                    self.draft_ids.swap_remove(i);
                    self.draft.note(size, ms);
                }
            }
            polled.shown.push(slot);
        }
        self.evict_variants();
        #[cfg(not(target_arch = "wasm32"))]
        {
            polled.wake_after = self.trim_when_idle();
        }
        if !self.pending.is_empty() {
            polled.wake_after = Some(Duration::from_millis(16));
        }
        polled
    }

    /// Free the GPU renderer's recycled buffers once nothing has rendered for [`IDLE_TRIM`] (they
    /// are reallocated by the next render; while working, renders reuse them). Returns how long
    /// until the trim is due, while it is still to come.
    #[cfg(not(target_arch = "wasm32"))]
    fn trim_when_idle(&mut self) -> Option<Duration> {
        if !self.pending.is_empty() {
            self.idle = None;
            return None;
        }
        let now = std::time::Instant::now();
        let (since, trimmed) = *self.idle.get_or_insert((now, false));
        if trimmed {
            return None;
        }
        let waited = now.duration_since(since);
        if waited >= IDLE_TRIM {
            lightcraft_engine::gpu::trim_pool(0);
            lightcraft_engine::memory::release();
            self.idle = Some((since, true));
            None
        } else {
            Some(IDLE_TRIM - waited)
        }
    }

    /// Bytes the per-view stage caches (CPU images and GPU buffers) may hold together.
    pub fn stage_budget(&self) -> usize {
        #[cfg(test)]
        if let Some(b) = self.stage_budget_override {
            return b;
        }
        lightcraft_engine::memory::budget() / STAGE_BUDGET_SHARE
    }

    /// Clear stage caches, least useful first, until what they hold fits [`Self::stage_budget`].
    fn trim_stages(&mut self) {
        let sizes: Vec<(Slot, usize)> = self.stages.iter().map(|(slot, c)| (*slot, c.bytes() + lightcraft_engine::gpu::stage_bytes(c))).collect();
        for slot in stage_trim_order(&sizes, self.stage_budget()) {
            if let Some(c) = self.stages.get(&slot) {
                c.clear();
                self.stages_trimmed += 1;
            }
        }
    }

    /// What the scheduler holds: per-view stage caches (CPU images and GPU buffers) and what the
    /// slots show (`textures`, at four bytes a pixel).
    pub fn memory(&self) -> serde_json::Value {
        let cpu: usize = self.stages.values().map(|s| s.bytes()).sum();
        let gpu: usize = self.stages.values().map(|s| lightcraft_engine::gpu::stage_bytes(s)).sum();
        let tex: usize = self.textures.values().map(|t| t.size[0] * t.size[1] * 4).sum();
        serde_json::json!({
            "stageCaches": {"count": self.stages.len(), "cpuBytes": cpu, "gpuBytes": gpu, "budgetBytes": self.stage_budget(), "trimmed": self.stages_trimmed, "sharedSourceBytes": lightcraft_engine::gpu::shared_source_bytes()},
            "textures": {"count": self.textures.len(), "bytes": tex},
        })
    }

    /// Drop the import review thumbnails (a new review, or the dialog closed).
    pub fn forget_imports(&mut self) {
        let review = |s: &Slot| matches!(s, Slot::Import(_) | Slot::Camera(_));
        self.pool.reprioritize(|s, p| if review(s) { None } else { Some(p) });
        self.queue.retain(|q| !review(&q.slot));
        self.pending.retain(|s, _| !review(s));
        self.request_ids.retain(|s, _| !review(s));
        self.textures.retain(|s, _| !review(s));
        self.quick_tried.retain(|s, _| !review(s));
    }

    /// Drop thumbnails that aren't in `keep` (bounded memory for huge libraries), and queued
    /// thumbnail jobs for photos that scrolled out of `keep`.
    pub fn evict_thumbs(&mut self, keep: &std::collections::HashSet<PhotoId>, max: usize) {
        self.thumb_inputs.retain(|id, _| keep.contains(id));
        let dropped = self.pool.reprioritize(|s, p| match s {
            Slot::Thumb(id) | Slot::ThumbQuick(id) if !keep.contains(id) && p <= 11 => None,
            _ => Some(p),
        });
        for s in dropped {
            self.pending.remove(&s);
            self.request_ids.remove(&s);
            self.quick_tried.remove(&s);
        }
        let request_ids = &mut self.request_ids;
        let pending = &mut self.pending;
        self.queue.retain(|q| match q.slot {
            Slot::Thumb(id) if !keep.contains(&id) && q.priority <= 10 => {
                pending.remove(&q.slot);
                request_ids.remove(&q.slot);
                false
            }
            _ => true,
        });
        let thumbs = self.thumb_textures();
        if thumbs <= max {
            return;
        }
        self.textures.retain(|s, _| !matches!(s, Slot::Thumb(id) | Slot::ThumbQuick(id) if !keep.contains(id)));
        self.quick_tried.retain(|s, _| !matches!(s, Slot::ThumbQuick(id) if !keep.contains(id)));
    }
}

/// The identity of a render job, kept to answer for it if it panics.
#[derive(Clone, Copy)]
struct Failed {
    request_id: u64,
    source_key: Option<lightcraft_preview::Hash128>,
    photo: PhotoId,
    level: lightcraft_engine::media::SourceLevel,
    key: u64,
}

impl Failed {
    fn of(job: &RenderJob) -> Self {
        Failed { request_id: job.request_id, source_key: job.source_key, photo: job.photo, level: job.level, key: job.key }
    }

    fn result(self, why: String) -> RenderResult {
        RenderResult {
            request_id: self.request_id,
            source_key: self.source_key,
            photo: self.photo,
            level: self.level,
            key: self.key,
            rendered: Err(why),
            gpu_fallback: None,
            loaded: None,
            quick: None,
            display: None,
        }
    }
}

/// Run `job` (on the worker thread) and make its image ready for `display`; a panic in it is that
/// render's error.
fn run_for(failed: Failed, job: impl FnOnce() -> RenderResult, display: &Option<Arc<lightcraft_engine::display::Display>>) -> RenderResult {
    let mut r = lightcraft_engine::guard::catch("the render", job).unwrap_or_else(|why| failed.result(why));
    if let Some(d) = display {
        lightcraft_engine::display::present(&mut r, d);
    }
    r
}

#[cfg(test)]
mod thumbnail_tests {
    use super::*;
    use lightcraft_catalog::{Catalog, Op, Photo, Source};

    type Sched = Scheduler<()>;

    struct Blocked;
    impl RenderOffload for Blocked {
        fn try_start(&mut self, _: Slot, job: RenderJob) -> Option<RenderJob> {
            Some(job)
        }
        fn finished(&mut self) -> Vec<(Slot, RenderResult, f64)> {
            vec![]
        }
    }
    fn photo() -> Arc<Photo> {
        Arc::new(Photo::new(PhotoId(1), Source::File { path: "audit.jpg".into() }, "audit.jpg", "JPEG", 6000, 4000, "2026-01-01"))
    }
    #[test]
    fn snapshot_tracks_cow_edits_undo_reload_and_replacement() {
        let mut catalog = Catalog::default();
        catalog.apply(Op::AddPhoto { photo: Box::new((*photo()).clone()) }).unwrap();
        let old = catalog.photo(PhotoId(1)).unwrap().clone();
        let mut r = Sched::default();
        r.remember_thumb(&old, 384, 77, false);
        r.pending.insert(Slot::Thumb(PhotoId(1)), (77, 10));
        assert!(r.thumb_current(&old, 384, 10));
        assert!(!r.thumb_current(&old, 512, 10));
        let mut settings = lightcraft_develop::DevelopSettings::default();
        settings.light.exposure = 1.0;
        let undo = catalog.apply(Op::SetDevelop { id: PhotoId(1), settings: Arc::new(settings), label: "edit".into(), edited: None }).unwrap();
        assert!(!r.thumb_current(catalog.photo(PhotoId(1)).unwrap(), 384, 10));
        catalog.apply(undo).unwrap();
        assert!(!r.thumb_current(catalog.photo(PhotoId(1)).unwrap(), 384, 10));
        catalog
            .apply(Op::SetContent {
                id: PhotoId(1),
                width: 6000,
                height: 4000,
                file_size: 99,
                content_hash: Some("changed".into()),
                preview_only: None,
            })
            .unwrap();
        assert!(!r.thumb_current(catalog.photo(PhotoId(1)).unwrap(), 384, 10));
        assert!(!r.thumb_current(&photo(), 384, 10));
        r.forget_all();
        assert!(r.thumb_inputs.is_empty());
    }
    #[test]
    fn queue_promotions_running_work_and_cancellation() {
        let p = photo();
        let slot = Slot::Thumb(p.id);
        let mut session = Session::new();
        session.catalog.apply(Op::AddPhoto { photo: Box::new((*p).clone()) }).unwrap();
        let job = session.thumb_job(p.id, 384).unwrap();
        let key = job.key;
        let mut r = Sched::default();
        r.set_offload(Box::new(Blocked));
        r.remember_thumb(&p, 384, key, false);
        r.request(slot, job, 5);
        assert!(r.thumb_current(&p, 384, 5));
        assert!(!r.thumb_current(&p, 384, 10));
        r.request(slot, session.thumb_job(p.id, 384).unwrap(), 10);
        assert_eq!(r.pending.get(&slot), Some(&(key, 10)));
        r.queue.clear(); // running work: cannot reprioritize, just as Scheduler::request
        assert!(r.thumb_current(&p, 384, 5));
        r.pending.clear();
        assert!(!r.thumb_current(&p, 384, 10));
        r.evict_thumbs(&Default::default(), 600);
        assert!(r.thumb_inputs.is_empty());
    }
    #[test]
    fn failures_retry_after_revision_and_quick_only_is_not_final() {
        let p = photo();
        let slot = Slot::Thumb(p.id);
        let mut r = Sched::default();
        r.remember_thumb(&p, 384, 77, false);
        r.pending.insert(Slot::ThumbQuick(p.id), (77, 11));
        assert!(!r.thumb_current(&p, 384, 10));
        r.failed.insert(slot, (77, "no decoder".into(), 0));
        assert!(r.thumb_current(&p, 384, 10));
        r.catalog_rev = 1;
        assert!(!r.thumb_current(&p, 384, 10));
    }
}

#[cfg(test)]
mod stage_budget_tests {
    use super::*;

    const MB: usize = 1 << 20;

    // Given stage caches within the budget, nothing is cleared
    #[test]
    fn nothing_is_trimmed_within_the_budget() {
        let sizes = [(Slot::Main, 300 * MB), (Slot::Region, 100 * MB)];
        assert_eq!(stage_trim_order(&sizes, 512 * MB), vec![]);
    }

    // Given too much held, the caches nothing is looking at go first (a hover render's, a compare
    // tile's: rebuilt cheaply), the least useful first, and only as many as needed
    #[test]
    fn the_idle_caches_go_first_and_only_as_many_as_needed() {
        let sizes =
            [(Slot::Main, 300 * MB), (Slot::Hover, 100 * MB), (Slot::Second, 80 * MB), (Slot::Compare(0), 60 * MB), (Slot::Compare(1), 60 * MB)];
        // 600 held, 500 allowed: the hover render's 100 is enough
        assert_eq!(stage_trim_order(&sizes, 500 * MB), vec![Slot::Hover]);
        // 400 allowed: the compare tiles' too (380 left), then, at 350, the second window's
        assert_eq!(stage_trim_order(&sizes, 400 * MB), vec![Slot::Hover, Slot::Compare(0), Slot::Compare(1)]);
        assert_eq!(stage_trim_order(&sizes, 350 * MB), vec![Slot::Hover, Slot::Compare(0), Slot::Compare(1), Slot::Second]);
    }

    // Given the open photo's views over the budget, they are not trimmed: they are what a slider
    // drag reuses on every tick, and clearing them every frame costs far more than the memory
    // (issue #323 review: 3.6x slower drags on a 2800 px canvas). They are counted and reported,
    // and only a runaway (more than HARD_FACTOR times the budget) cuts the window and before
    // caches, never the whole-frame view's.
    #[test]
    fn the_open_photos_views_are_trimmed_only_in_a_runaway_and_never_the_whole_frame() {
        let views = [(Slot::Main, 400 * MB), (Slot::Region, 500 * MB), (Slot::RegionBefore, 500 * MB), (Slot::Before, 100 * MB)];
        assert_eq!(stage_trim_order(&views, 400 * MB), vec![], "1.5 GB against a 400 MB budget is a busy Before/After, not a runaway");
        // against a 100 MB budget (HARD_FACTOR 4 = 400 MB) it is: the before side goes first, then the window
        assert_eq!(stage_trim_order(&views, 100 * MB), vec![Slot::RegionBefore, Slot::Before, Slot::Region]);
        let mut with_idle = views.to_vec();
        with_idle.push((Slot::Hover, 50 * MB));
        assert_eq!(stage_trim_order(&with_idle, 400 * MB), vec![Slot::Hover]);
        // a whole-frame view of any size stays
        assert_eq!(stage_trim_order(&[(Slot::Main, 9000 * MB)], MB), vec![]);
    }

    // Given caches of slots that hold nothing, they are never "trimmed"
    #[test]
    fn empty_caches_are_left_alone() {
        assert_eq!(stage_trim_order(&[(Slot::Hover, 0), (Slot::Second, 900 * MB)], 100 * MB), vec![Slot::Second]);
    }

    // Given real stage caches over the budget, the scheduler clears them (the open photo's last)
    #[test]
    fn the_renderer_trims_real_stage_caches() {
        use lightcraft_engine::pipeline::{RenderRequest, SourceInfo, render_cached};
        let src = std::sync::Arc::new(lightcraft_raster::Rgb32f::from_fn(320, 240, |x, y| [x as f32 / 320.0, y as f32 / 240.0, 0.3]));
        let mut r = Scheduler::<()>::default();
        for slot in [Slot::Main, Slot::Hover] {
            let cache: Arc<StageCache> = Default::default();
            let mut s = lightcraft_engine::develop::DevelopSettings::default();
            s.effects.clarity = 50.0;
            render_cached(&src, &SourceInfo::default(), &s, &RenderRequest::fit(320, 240), &cache);
            assert!(cache.bytes() > 0);
            r.stages.insert(slot, cache);
        }
        let total: usize = r.stages.values().map(|c| c.bytes()).sum();
        r.stage_budget_override = Some(total * 3 / 4);
        r.trim_stages();
        assert_eq!(r.stages[&Slot::Hover].bytes(), 0, "the hover render's cache went");
        assert!(r.stages[&Slot::Main].bytes() > 0, "the open photo's stayed");
        assert_eq!(r.stages_trimmed, 1);
        r.stage_budget_override = Some(0);
        r.trim_stages();
        assert!(r.stages[&Slot::Main].bytes() > 0, "the open photo's view is never trimmed");
        r.trim_stages();
        assert_eq!(r.stages_trimmed, 1, "and nothing is trimmed again");
    }

    // The scheduler counts and trims what it holds, and says so
    #[test]
    fn the_renderer_reports_its_stage_budget() {
        let r = Scheduler::<()>::default();
        let m = r.memory();
        assert!(m["stageCaches"]["budgetBytes"].as_u64().unwrap() > 0, "{m}");
        assert_eq!(m["stageCaches"]["trimmed"], 0);
        assert!(m["stageCaches"]["sharedSourceBytes"].is_u64(), "the one device copy of a big original is reported: {m}");
    }
}

#[cfg(test)]
mod draft_budget_tests {
    use super::*;

    /// A host that renders `ns_per_px` per pixel, after two drafts of one size (the first is cold).
    fn learned(ns_per_px: f64) -> DraftBudget {
        let mut b = DraftBudget::default();
        let size = [1000, 700];
        for _ in 0..4 {
            b.note(size, ns_per_px * 1e-6 * 700_000.0);
        }
        b
    }

    #[test]
    fn nothing_is_drafted_when_no_slider_drags() {
        assert_eq!(learned(100.0).scale(false, 3e6), 1.0);
    }

    #[test]
    fn a_fast_host_keeps_the_old_scale_and_a_slow_one_shrinks_the_draft() {
        // a GPU: a few ms for a 1.5 MP draft
        assert_eq!(learned(3.0).scale(true, 3e6), DraftBudget::MAX);
        // the browser's CPU worker: ~100 ns a pixel makes 0.4 MP the most that fits 40 ms
        let s = learned(100.0).scale(true, 3e6);
        let px = 3e6 * f64::from(s) * f64::from(s);
        assert!((0.25e6..0.5e6).contains(&px), "scale {s} is {px} px");
        // a hopeless host stops at the floor
        assert_eq!(learned(10_000.0).scale(true, 3e6), DraftBudget::MIN);
    }

    #[test]
    fn a_draft_of_a_new_size_is_cold_and_teaches_nothing() {
        let mut b = DraftBudget::default();
        b.note([800, 600], 900.0);
        b.note([400, 300], 900.0);
        assert_eq!(b.scale(true, 3e6), DraftBudget::GUESS, "no two drafts of one size yet");
        b.note([400, 300], 12.0);
        assert!(b.scale(false, 3e6) == 1.0 && b.ms_per_px.is_some_and(|c| (c - 1e-4).abs() < 1e-9));
    }

    #[test]
    fn the_scale_holds_during_a_drag_and_only_goes_down() {
        let mut b = learned(3.0);
        let first = b.scale(true, 3e6);
        assert_eq!(first, DraftBudget::MAX);
        // a little noise in the samples moves nothing
        b.note([1000, 700], 2.6);
        assert_eq!(b.scale(true, 3e6), first);
        // a slider that turns out much slower (it redoes more per tick) shrinks the draft...
        for _ in 0..12 {
            b.note([1000, 700], 70.0);
        }
        let smaller = b.scale(true, 3e6);
        assert!(smaller < first * 0.75 && smaller >= DraftBudget::MIN, "{smaller}");
        // ...and a fast tick afterwards doesn't grow it back mid-drag
        for _ in 0..30 {
            b.note([1000, 700], 2.0);
        }
        assert_eq!(b.scale(true, 3e6), smaller);
        // the next drag starts afresh from what was learned last
        assert_eq!(b.scale(false, 3e6), 1.0);
        assert_eq!(b.scale(true, 3e6), DraftBudget::MAX);
    }

    // A fast host keeps drafting a zoomed view's windows at full size; a slower one shrinks them with the frame's draft
    #[test]
    fn windows_shrink_only_as_far_as_the_frame_draft_is_below_the_fast_hosts() {
        assert_eq!(DraftBudget::window_scale(DraftBudget::MAX), 1.0);
        assert_eq!(DraftBudget::window_scale(1.0), 1.0);
        assert!((DraftBudget::window_scale(0.3) - 0.5).abs() < 1e-6);
        assert!((DraftBudget::window_scale(DraftBudget::MIN) - DraftBudget::MIN / DraftBudget::MAX).abs() < 1e-6);
        assert_eq!(DraftBudget::window_scale(f32::NAN), 1.0);
        assert!(DraftBudget::window_scale(0.0) >= DraftBudget::MIN / DraftBudget::MAX);
    }

    #[test]
    fn hostile_numbers_give_a_scale_in_range() {
        for full in [0.0, -1.0, f64::NAN, f64::INFINITY, 1e300] {
            let s = learned(100.0).scale(true, full);
            assert!((DraftBudget::MIN..=DraftBudget::MAX).contains(&s), "full_px {full}: {s}");
            let mut b = DraftBudget::default();
            b.note([0, 0], f64::NAN);
            b.note([0, 0], f64::INFINITY);
            assert!(b.scale(true, full) > 0.0);
        }
    }
}

#[cfg(test)]
mod scheduler_tests {
    use super::*;
    use lightcraft_catalog::{Op, Photo, Source};

    /// Keeps the pixels it was given (what the SwiftUI host does with them).
    struct Keep;
    impl Present<Arc<Rgba8>> for Keep {
        fn make(&mut self, _: Slot, image: Rgba8) -> Arc<Rgba8> {
            Arc::new(image)
        }
        fn update(&mut self, _: Slot, image: Rgba8, view: &mut Arc<Rgba8>) {
            *view = Arc::new(image);
        }
    }

    fn session(loader: lightcraft_engine::media::FileLoader) -> Session {
        let mut s = Session::new();
        s.media.file_loader = Some(loader);
        let photo = Photo::new(PhotoId(1), Source::File { path: "a.jpg".into() }, "a.jpg", "JPEG", 8, 8, "2026-01-01");
        s.catalog.apply(Op::AddPhoto { photo: Box::new(photo) }).unwrap();
        s
    }

    fn poll_until(r: &mut Scheduler<Arc<Rgba8>>, s: &mut Session, done: impl Fn(&Polled) -> bool) -> Polled {
        let t0 = std::time::Instant::now();
        loop {
            let p = r.poll(s, &mut Keep);
            if done(&p) {
                return p;
            }
            assert!(t0.elapsed() < Duration::from_secs(30), "no result");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    // A finished render becomes the slot's picture, with its facts, through the frontend's presenter
    #[test]
    fn a_render_becomes_what_its_slot_shows() {
        let mut s = session(Arc::new(|_, _| {
            let mut image = lightcraft_raster::Rgb32f::new(8, 8);
            image.data.fill([0.3, 0.2, 0.1]);
            Ok((image, Default::default()))
        }));
        let mut r = Scheduler::<Arc<Rgba8>>::with_worker_threads(1);
        let (tx, rx) = std::sync::mpsc::channel();
        r.set_notify(Some(Arc::new(move || {
            let _ = tx.send(());
        })));
        let job = s.loupe_job(PhotoId(1), 8, 8, true).unwrap().draft();
        let key = job.key;
        r.request(Slot::Main, job, 100);
        assert!(r.is_pending(Slot::Main));
        rx.recv_timeout(Duration::from_secs(30)).expect("the pool says when the render is done");
        let p = poll_until(&mut r, &mut s, Polled::changed);
        assert_eq!(p.shown, vec![Slot::Main]);
        let shown = &r.textures[&Slot::Main];
        assert_eq!((shown.key, shown.photo, shown.draft), (key, PhotoId(1), true));
        assert_eq!(shown.size, [shown.width, shown.height], "the view derefs to the presenter's pixels");
        assert!(shown.histogram.is_some());
        assert!(!r.is_pending(Slot::Main));
    }

    // A render that panics is that slot's failure: it is reported, the slot is no longer pending,
    // and the worker renders the next one
    #[test]
    fn a_panicking_render_fails_its_slot_and_spares_the_worker() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let c = calls.clone();
        let mut s = session(Arc::new(move |_, _| {
            if c.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                panic!("a decoder panics (expected in this test)");
            }
            Ok((lightcraft_raster::Rgb32f::new(8, 8), Default::default()))
        }));
        let mut r = Scheduler::<Arc<Rgba8>>::with_worker_threads(1);
        r.request(Slot::Thumb(PhotoId(1)), s.thumb_job(PhotoId(1), 64).unwrap(), 10);
        let p = poll_until(&mut r, &mut s, |p| !p.failed.is_empty());
        assert_eq!(p.failed.len(), 1);
        assert_eq!(p.failed[0].0, Slot::Thumb(PhotoId(1)));
        assert!(p.failed[0].3.contains("failed unexpectedly"), "{:?}", p.failed);
        assert!(!r.is_pending(Slot::Thumb(PhotoId(1))));
        assert_eq!(r.failure(Slot::Thumb(PhotoId(1))).map(|f| f.contains("panics")), Some(true));
        r.request(Slot::Main, s.loupe_job(PhotoId(1), 8, 8, true).unwrap(), 100);
        poll_until(&mut r, &mut s, |p| p.shown.contains(&Slot::Main));
        assert_eq!(r.live_workers(), 1);
    }
}
