//! Background rendering, shared by every frontend: engine [`RenderJob`]s run on a [`JobPool`];
//! results become whatever the frontend shows ([`Present`]: egui textures, image leases for a
//! native frontend).
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
pub use lightcraft_engine::hdr_view::HdrPicture;
use lightcraft_engine::media::{QuickJob, QuickSource, RenderJob, RenderResult, ScreenFrame};
use lightcraft_engine::pipeline::StageCache;
use lightcraft_engine::slideshow::{Item, Title};
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
    /// The loupe's stand-in for a photo the user is likely to step to next (the camera JPEG of an
    /// unedited raw, or a cached render), made ahead in the direction of travel so a step shows it
    /// at once: [`Scheduler::request_quick`] takes the texture over into [`Slot::Preview`].
    QuickAhead(u8),
    /// The loupe zoomed in on an unedited raw: its camera JPEG at full size, drawn under the window
    /// render until that arrives (a focus check at 100 % without waiting for the raw).
    QuickFull,
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
    /// A slideshow slide (the one up, the one fading out, the next ones prepared in advance): a
    /// photo's, or a title screen.
    Slide(lightcraft_engine::slideshow::Item),
    /// The small composed slide in Settings ▸ Slideshow (the active photo with the layout being
    /// edited).
    SlidePreview,
    /// A slider drag's live HDR histogram: a small full-quality render of an HDR edit, beside the
    /// drafts (which have no HDR histogram), with no picture ([`Scheduler::live_hdr_histogram`]).
    HdrHistogram,
}

/// What a slot shows: the render's facts, and `view`, the frontend's form of its pixels (an egui
/// texture, an image lease…). Derefs to `view`, so a frontend reads its own fields directly.
pub struct Shown<V> {
    /// Frame geometry and crop mode captured when these pixels were requested ([`geometry_look`]).
    pub look: Option<(u64, bool)>,
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

/// Settings that can change the rendered frame. Colour-only edits, including temporarily
/// hidden grading wheels, must not discard a decoded RAW's actual aspect ratio.
pub fn geometry_look(settings: &lightcraft_develop::DevelopSettings) -> u64 {
    let geometry = lightcraft_develop::DevelopSettings {
        crop: settings.crop,
        orientation: settings.orientation,
        geometry: settings.geometry.clone(),
        optics: settings.optics,
        disabled_sections: settings.disabled_sections.iter().filter(|s| matches!(s.as_str(), "geometry" | "optics")).cloned().collect(),
        ..Default::default()
    };
    geometry.hash64()
}

/// The geometry identity of a job's pixels: [`geometry_look`] and whether the crop applies.
fn look_of(job: &RenderJob) -> (u64, bool) {
    (geometry_look(&job.settings), job.request.apply_crop)
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
    /// `slot`'s picture, just made or updated, has an HDR rendition (`Some`) or none (`None`: an SDR
    /// render took its place). Only the loupe's slots ([`Scheduler::set_hdr_display`]) get one. A
    /// frontend that can't show HDR leaves this as it is.
    fn set_hdr(&mut self, _slot: Slot, _hdr: Option<Arc<HdrPicture>>, _view: &mut V) {}
    /// A render that stayed on the GPU ([`RenderResult::frame`]) for `slot`: show it in `view` (what
    /// the slot shows so far), or make a picture of it. [`FrameShown::Refused`] when it can't be shown:
    /// the scheduler then asks for pixels from now on ([`Scheduler::set_screen_frames`]). A frontend
    /// that never turned GPU-resident renders on gets none.
    fn show_frame(&mut self, _slot: Slot, _frame: &ScreenFrame, _view: Option<&mut V>) -> FrameShown<V> {
        FrameShown::Refused
    }
    /// Whether `view` is drawn from a GPU-resident render: dropped (and rendered again, as pixels)
    /// when the frontend can no longer show those.
    fn on_gpu(&self, _view: &V) -> bool {
        false
    }
}

/// What [`Present::show_frame`] did with a GPU-resident render.
pub enum FrameShown<V> {
    /// The slot's picture now shows it.
    Updated,
    /// A new picture for the slot, which had none.
    Made(V),
    /// It can't be shown.
    Refused,
}

/// How many [`Slot::QuickAhead`] stand-ins the loupe keeps ready.
pub const AHEAD_SLOTS: u8 = 6;

/// The camera-preview edge a loupe stand-in is made at (0: one that doesn't depend on the size).
fn quick_edge(job: &QuickJob) -> usize {
    job.embedded.as_ref().map_or(0, |e| e.2)
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

/// Immutable identity captured when a request is submitted, not when its result is collected.
pub struct Requested {
    /// Frame geometry and crop mode of the request ([`Shown::look`]).
    pub look: Option<(u64, bool)>,
    pub id: u64,
    /// The file or scene the request reads, for a failure's log line (`None`: not a photo's render).
    pub source: Option<lightcraft_catalog::Source>,
}

/// The most failure lines kept for [`Scheduler::failure_messages`].
const FAILURE_MESSAGES_KEPT: usize = 32;

/// How the log names a render: its slot, its photo and that photo's source, for every
/// catalog-backed render slot (#1890). Debug-format paths so whitespace/control characters cannot
/// forge another log entry.
pub fn render_subject(slot: Slot, photo: PhotoId, source: Option<&lightcraft_catalog::Source>) -> String {
    match source {
        Some(lightcraft_catalog::Source::File { path }) => format!("render {slot:?}, {photo:?}, file {path:?}"),
        Some(lightcraft_catalog::Source::Demo { scene }) => format!("render {slot:?}, {photo:?}, demo scene {scene}"),
        None => format!("render {slot:?}, {photo:?}, source unavailable"),
    }
}

/// Include the source identity in diagnostics ([`render_subject`]).
pub fn render_failure_message(slot: Slot, photo: PhotoId, source: Option<&lightcraft_catalog::Source>, error: &str) -> String {
    format!("{}: {error}", render_subject(slot, photo, source))
}

/// The log line for a request dropped before its result came back, named by the source it was
/// requested from. A grid slot names its photo; a view or slide slot shows whichever photo is open,
/// so its file names it.
pub fn abandoned_line(slot: Slot, request: Option<&Requested>, why: &str) -> String {
    let source = request.and_then(|r| r.source.as_ref());
    match (slot, source) {
        (Slot::Thumb(id) | Slot::ThumbQuick(id), _) => format!("{}: abandoned, {why}", render_subject(slot, id, source)),
        (_, Some(lightcraft_catalog::Source::File { path })) => format!("render {slot:?}, file {path:?}: abandoned, {why}"),
        (_, Some(lightcraft_catalog::Source::Demo { scene })) => format!("render {slot:?}, demo scene {scene}: abandoned, {why}"),
        (_, None) => format!("render {slot:?}: abandoned, {why}"),
    }
}

/// Which library `session` has open, for [`Scheduler::thumbs_made`]: a hash of its folder (the same
/// library opened again is the same), 0 with none.
fn library_key(session: &Session) -> u64 {
    use std::hash::{Hash, Hasher};
    let Some(lib) = session.library.as_ref() else { return 0 };
    let mut h = std::collections::hash_map::DefaultHasher::new();
    lib.dir.hash(&mut h);
    h.finish()
}

struct Queued {
    slot: Slot,
    priority: u32,
    seq: u64,
    job: RenderJob,
}

/// How large a draft (the loupe's render while a slider drags) may be, learned from how long this
/// host takes to render one. On a GPU most drags render the whole view in a few ms (an Apple M1 Max:
/// 3–5 ms for a 24 MP raw's Retina loupe, `examples/drag_bench.rs`), so a fast host drafts at the
/// view's own size and the photo stays sharp while it moves; Temperature and Noise Reduction rebuild
/// every spatial plane (30–100 ms on a slower GPU, `render_bench`), and the browser build has no GPU
/// at all: it renders on one CPU thread at 100–300 ns per pixel, where 0.6 is a few frames a second.
/// The scale is chosen once per drag and only goes down during it, and only when the drafts take
/// about three times the target: a draft of another size starts from a cold stage cache (a full
/// raw's: hundreds of ms), so a scale that moved with every sample would be slower than either.
/// SDR and HDR drafts (an HDR edit's loupe on a display with headroom renders its float HDR
/// picture) are learned apart: where the HDR one renders on the CPU (a LUT profile, a refined mask)
/// it costs ~10× the SDR one, and a drag must not start from the other kind's speed. So is each
/// slider (the drag's interaction label): Noise Reduction redoes far more per draft than Vibrance,
/// and a Vibrance drag after one started smaller (and so from a cold stage cache: 0.8–1.6 s for its
/// first picture in the app) and blurrier than it needed to.
#[derive(Debug, Default)]
pub struct DraftBudget {
    /// What SDR (`[0]`) and HDR (`[1]`) drafts cost, of whichever slider: where a slider has none of its
    /// own yet (off a GPU, the best guess there is).
    cost: [DraftCost; 2],
    /// What each slider's drafts cost (its drag's label, HDR or not), the latest [`Self::SLIDERS`].
    by_drag: Vec<(String, bool, DraftCost)>,
    /// The drag in progress (its label), whose drafts [`Self::note`] learns from.
    drag: Option<String>,
    /// The scale the drag in progress renders at.
    frozen: Option<f32>,
    /// The scale the latest drag rendered at, and whether it drafted HDR (diagnostics).
    last_scale: Option<f32>,
    last_hdr: bool,
    /// A GPU renders the drafts: a session's first SDR drag then drafts at the view's own size, as a GPU renders
    /// nearly every drag, from the stage cache the sharp picture left at that size (a smaller first draft started
    /// cold, 30–45 ms on an M1 Max, and blurred the first drag).
    pub gpu: bool,
}

#[derive(Debug, Default)]
struct DraftCost {
    /// Render time per output pixel (ms) of drafts that ran on a warm stage cache: the median of the
    /// latest few, so one draft held up by other work on the GPU doesn't shrink the drag.
    ms_per_px: Option<f64>,
    recent: Vec<f64>,
    /// Size of the last draft that finished: the next one of the same size was warm.
    last: Option<[usize; 2]>,
}

impl DraftBudget {
    /// What a draft should take (ms). Natively one draft renders at a time and the next waits for it,
    /// so this is the update rate: about 50 a second. (A browser renders the drafts of a drag on
    /// several workers in turn, so there it is the lag behind the slider: about two frames at 60 Hz.)
    const TARGET_MS: f64 = if cfg!(target_arch = "wasm32") { 40.0 } else { 20.0 };
    /// Linear scale limits: natively up to the view's own size; the browser keeps the old ceiling.
    const MAX: f32 = if cfg!(target_arch = "wasm32") { 0.6 } else { 1.0 };
    const MIN: f32 = 0.2;
    /// Before anything is measured (a session's first drag): the scale drafts always had, smaller on a
    /// CPU-only host (the browser).
    const GUESS: f32 = if cfg!(target_arch = "wasm32") { 0.35 } else { 0.6 };
    /// The frame's draft scale from which a zoomed view's windows are drafted at full size.
    const WINDOW_FULL: f32 = 0.6;
    /// How many warm drafts the cost is the median of.
    const SAMPLES: usize = 5;
    /// A drag's scale shrinks only below this share of it (the drafts take ~2.8 × the target).
    const SHRINK: f32 = 0.6;
    /// How many sliders' costs are kept.
    const SLIDERS: usize = 64;

    /// A draft of `size` (`hdr`: an HDR picture) of the drag in progress finished in `ms`.
    pub fn note(&mut self, size: [usize; 2], ms: f64, hdr: bool) {
        self.cost[usize::from(hdr)].note(size, ms);
        let Some(drag) = self.drag.as_deref() else { return };
        let i = match self.by_drag.iter().position(|(d, h, _)| d == drag && *h == hdr) {
            Some(i) => i,
            None => {
                if self.by_drag.len() >= Self::SLIDERS {
                    self.by_drag.remove(0);
                }
                self.by_drag.push((drag.to_string(), hdr, DraftCost::default()));
                self.by_drag.len() - 1
            }
        };
        if let Some((_, _, cost)) = self.by_drag.get_mut(i) {
            cost.note(size, ms);
        }
    }

    /// The linear scale of a draft of a render that is `full_px` pixels at full size (`hdr`: an HDR
    /// picture) while the slider `drag` (its interaction label) drags: 1 when none is (`None`).
    pub fn scale(&mut self, drag: Option<&str>, full_px: f64, hdr: bool) -> f32 {
        let Some(drag) = drag else {
            // (the full-size render that follows replaces the stage cache: the next drag starts cold)
            self.frozen = None;
            for c in self.cost.iter_mut().chain(self.by_drag.iter_mut().map(|(_, _, c)| c)) {
                c.last = None;
            }
            return 1.0;
        };
        if self.drag.as_deref() != Some(drag) {
            self.drag = Some(drag.to_string());
        }
        let own = self.by_drag.iter().find(|(d, h, _)| d == drag && *h == hdr).and_then(|(_, _, c)| c.ms_per_px);
        // (a slider not dragged yet: on a GPU the view's own size, as nearly every slider renders there in time;
        // elsewhere what drafts of any slider cost)
        let cost = match own {
            Some(c) => Some(c),
            None if self.gpu && !hdr => None,
            None => self.cost[usize::from(hdr)].ms_per_px,
        };
        let want = match cost {
            Some(c) if c > 0.0 => ((Self::TARGET_MS / c / full_px.max(1.0)).sqrt() as f32).clamp(Self::MIN, Self::MAX),
            _ if self.gpu && !hdr => Self::MAX,
            _ => Self::GUESS,
        };
        // (steps of 0.05, so the render size doesn't change with every sample)
        let want = if want.is_finite() { (want * 20.0).round() / 20.0 } else { Self::GUESS };
        let scale = match self.frozen {
            Some(f) if want >= f * Self::SHRINK => f,
            _ => want,
        };
        self.frozen = Some(scale);
        self.last_scale = Some(scale);
        self.last_hdr = hdr;
        scale
    }

    /// The linear scale of a zoomed view's window draft, given the whole-frame draft's `scale`: windows
    /// hold only what is on screen and were always drafted at full size, which a fast host still does
    /// (`scale` at [`Self::WINDOW_FULL`] or more); a slower one shrinks them in proportion to how far the
    /// frame's draft went below it.
    pub fn window_scale(scale: f32) -> f32 {
        if scale.is_finite() { (scale / Self::WINDOW_FULL).clamp(Self::MIN / Self::WINDOW_FULL, 1.0) } else { 1.0 }
    }

    /// For diagnostics: the scale of the latest drag, and the learned render cost (ms per megapixel) of its slider.
    pub fn report(&self) -> (Option<f32>, Option<f64>) {
        let own =
            self.drag.as_deref().and_then(|d| self.by_drag.iter().find(|(x, h, _)| x == d && *h == self.last_hdr)).and_then(|(_, _, c)| c.ms_per_px);
        (self.last_scale, own.or(self.cost[usize::from(self.last_hdr)].ms_per_px).map(|c| c * 1e6))
    }
}

impl DraftCost {
    /// A draft of `size` finished in `ms`: learnt from when the one before it was of the same size (warm).
    fn note(&mut self, size: [usize; 2], ms: f64) {
        let warm = self.last == Some(size);
        self.last = Some(size);
        let px = size[0] as f64 * size[1] as f64;
        let c = ms / px;
        if warm && c.is_finite() && c > 0.0 {
            if self.recent.len() >= DraftBudget::SAMPLES {
                self.recent.remove(0);
            }
            self.recent.push(c);
            let mut sorted = self.recent.clone();
            sorted.sort_by(f64::total_cmp);
            self.ms_per_px = sorted.get(sorted.len() / 2).copied();
        }
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
/// A neighbour's prefetch is skipped when the device lets the app allocate less than this (iOS ends an app past its
/// limit): its decode and render (~400 MB for a 24 MP raw) and the next swipe's, side by side, with room to spare.
/// ponytail: one fixed reserve; scale it with the photo's pixels if prefetch is skipped where it would fit.
const PREFETCH_MIN_LEFT: usize = 1 << 30;

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
    /// The request in flight for each slot (its id and what it reads). Public for the frontends' tests.
    pub requests: HashMap<Slot, Requested>,
    /// Geometry identity of every request still out, by request id: a superseded draft that is
    /// still shown keeps its own, never the newer request's (and an offload's results, which carry
    /// none, find theirs here).
    pub request_looks: HashMap<u64, (u64, bool)>,
    /// The latest failure lines logged (at most [`FAILURE_MESSAGES_KEPT`]), for tests.
    pub failure_messages: Vec<String>,
    /// Request id of the pixels each interactive view (loupe, before, hover) shows: a draft that
    /// finishes after a newer request was made still replaces older pixels, so a long slider drag
    /// keeps updating the loupe instead of waiting for the drag to pause.
    shown_ids: HashMap<Slot, u64>,
    /// Requests made before this id belong to a library or preview cache that is gone.
    epoch: u64,
    preview_generation: Option<(std::sync::Weak<lightcraft_preview::PreviewCache>, u64)>,
    /// Last thumbnail inputs; weak identities do not retain photo histories or force deep clones.
    /// (photo, size bucket, job key, whether the request went through an embedded stand-in, and the
    /// priority of the caller that queued it: a stand-in's background render runs below that)
    pub thumb_inputs: HashMap<PhotoId, (std::sync::Weak<lightcraft_catalog::Photo>, usize, u64, bool, u32)>,
    /// Photos whose grid thumbnail is the camera's embedded preview itself (no render behind it).
    pub embedded_thumbs: HashMap<PhotoId, (std::sync::Weak<lightcraft_catalog::Photo>, usize, u64)>,
    pub pool: JobPool<Slot, RenderResult>,
    /// Jobs run elsewhere (see [`RenderOffload`]); `queue` holds the ones not started yet.
    offload: Option<Box<dyn RenderOffload>>,
    queue: Vec<Queued>,
    seq: u64,
    /// Slot → (key, priority) of the request in flight (queued or running).
    pub pending: HashMap<Slot, (u64, u32)>,
    /// What each slot shows.
    pub textures: HashMap<Slot, Shown<V>>,
    /// The last [`Slot::HdrHistogram`] result: (photo, job key, histogram).
    pub live_hdr_histogram: Option<(PhotoId, u64, Histogram)>,
    pub last_main_ms: f64,
    /// How big the loupe's drafts may be on this host (see [`DraftBudget`]).
    draft: DraftBudget,
    /// Request ids of the loupe's latest drafts: their results teach `draft` the host's speed.
    draft_ids: Vec<u64>,
    /// Request ids of the latest draft renders of any slot (a picture says whether it is one).
    drafted: Vec<u64>,
    /// Jobs finished since start (for inspect/perf).
    pub completed: u64,
    /// Finished jobs thrown away because a newer request for their slot had replaced them.
    pub discarded: u64,
    /// Keep the renders of the neighbouring photos ([`Slot::Prefetch`]) as textures, for the phone
    /// loupe to slide in; elsewhere they only warm the engine's caches.
    pub keep_neighbours: bool,
    /// Per-view intermediate results (loupe and "before"), so slider drags redo only what changed.
    stages: HashMap<Slot, Arc<StageCache>>,
    /// Slot → key of the last quick job requested for it (each is tried once). Public for the frontends' tests.
    pub quick_tried: HashMap<Slot, u64>,
    /// The camera-preview edge each loupe stand-in ([`Slot::Preview`], [`Slot::QuickAhead`]) was
    /// asked for (0: one that doesn't depend on the size): a stand-in made ahead for a smaller loupe
    /// isn't taken over by a larger one.
    quick_edge: HashMap<Slot, usize>,
    /// Photos given a grid thumbnail since start (a render replacing a stand-in isn't counted again):
    /// unlike [`Scheduler::thumb_textures`] it never goes down when old thumbnails are evicted (for
    /// benchmarks).
    pub thumbs_made: u64,
    /// The photos [`Self::thumbs_made`] counted, by library (a hash of its folder) and id: a
    /// thumbnail made again after eviction, or after going to another library and back, is the
    /// same photo (photo ids are a library's own). Never cleared.
    thumbs_counted: std::collections::HashSet<(u64, PhotoId)>,
    /// Import review thumbnail jobs built ([`Self::import_preview`]; for tests).
    pub import_jobs_built: usize,
    /// Keep the render decisions logged ([`Self::decided`]) in [`Self::decisions`] (tests).
    pub record_decisions: bool,
    /// The render decisions logged while [`Self::record_decisions`] is set.
    pub decisions: Vec<String>,
    /// Interactive drafts of views may stay on the GPU: the frontend shows GPU-resident renders
    /// ([`Self::set_screen_frames`]).
    screen_frames: bool,
    /// Jobs that failed (unreadable or missing file…), by slot and job key, with the error and
    /// the photo: not requested again until the key changes (an edit, another size) — the grid
    /// asks every frame.
    pub failed: HashMap<Slot, (u64, String, u64, PhotoId)>,
    /// The catalog revision seen at the last poll: a failure is retried once the catalog changes
    /// its photo (a relinked or re-imported file), see [`Self::follow_catalog`].
    pub catalog_rev: u64,
    /// Prefetch slot → key of the last job submitted (each runs once).
    prefetched: HashMap<Slot, u64>,
    /// Prefetches asked for while the loupe's own render was under way: they start when it ends ([`Self::prefetch`]).
    deferred_prefetch: Vec<(Slot, RenderJob, u32)>,
    /// Variant key → frame it was last asked for (LRU of [`Slot::Variant`] textures).
    variant_used: HashMap<u64, u64>,
    /// How many variants the last frame's views showed at once (see [`Self::want_variants`]).
    variant_want: usize,
    /// Frames polled so far.
    frame: u64,
    /// Slideshow slides may be held ([`Self::release_slides`] has something to do).
    slides_live: bool,
    /// Stage caches cleared to stay in budget (see [`stage_trim_order`]).
    stages_trimmed: u64,
    /// A budget other than the memory budget's share (tests).
    pub stage_budget_override: Option<usize>,
    /// The monitor profile textures are made for (`app.displayProfile`; `None`: sRGB).
    display: Option<Arc<lightcraft_engine::display::Display>>,
    /// The display can show the range above SDR white: the loupe renders HDR edits in HDR
    /// ([`Self::set_hdr_display`]).
    hdr_display: bool,
    /// The loupe's latest picture was an HDR one: its drafts will be too (see [`DraftBudget`]).
    main_hdr: bool,
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
            Slot::QuickAhead(i) => format!("quickAhead:{i}"),
            Slot::QuickFull => "quickFull".into(),
            Slot::Variant(k) => format!("variant:{k}"),
            Slot::Hover => "hover".into(),
            Slot::Import(i) => format!("import:{i}"),
            Slot::Camera(k) => format!("camera:{k}"),
            Slot::Second => "second".into(),
            Slot::Slide(Item::Photo(id)) => format!("slide:photo:{}", id.0),
            Slot::Slide(Item::Title(t)) => format!("slide:title:{}", t.id()),
            Slot::SlidePreview => "slidePreview".into(),
            Slot::HdrHistogram => "hdrHistogram".into(),
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
            "quickAhead" => Slot::QuickAhead(small()?),
            "quickFull" => Slot::QuickFull,
            "variant" => Slot::Variant(num()?),
            "hover" => Slot::Hover,
            "import" => Slot::Import(n.parse().ok()?),
            "camera" => Slot::Camera(num()?),
            "second" => Slot::Second,
            "slide" => match n.split_once(':')? {
                ("photo", id) => Slot::Slide(Item::Photo(PhotoId(id.parse().ok()?))),
                ("title", id) => Slot::Slide(Item::Title(Title::parse(id)?)),
                _ => return None,
            },
            "slidePreview" => Slot::SlidePreview,
            "hdrHistogram" => Slot::HdrHistogram,
            _ => return None,
        })
    }

    /// An interactive view of the open photo: slider drags redo only the stages they feed, and a
    /// draft that finishes late still replaces older pixels.
    pub fn is_view(self) -> bool {
        matches!(self, Slot::Main | Slot::Region | Slot::RegionBefore | Slot::Before | Slot::Hover | Slot::HdrHistogram)
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
            requests: HashMap::new(),
            request_looks: HashMap::new(),
            failure_messages: Vec::new(),
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
            live_hdr_histogram: None,
            last_main_ms: 0.0,
            draft: DraftBudget::default(),
            draft_ids: Vec::new(),
            drafted: Vec::new(),
            completed: 0,
            discarded: 0,
            keep_neighbours: false,
            stages: HashMap::new(),
            quick_tried: HashMap::new(),
            quick_edge: HashMap::new(),
            thumbs_made: 0,
            thumbs_counted: Default::default(),
            import_jobs_built: 0,
            record_decisions: false,
            decisions: Vec::new(),
            screen_frames: false,
            failed: HashMap::new(),
            catalog_rev: 0,
            prefetched: HashMap::new(),
            deferred_prefetch: Vec::new(),
            variant_used: HashMap::new(),
            variant_want: 0,
            frame: 0,
            slides_live: false,
            stages_trimmed: 0,
            stage_budget_override: None,
            display: None,
            hdr_display: false,
            main_hdr: false,
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
        let Some(&(ref old, size, key, quick, _)) = self.thumb_inputs.get(&photo.id) else { return false };
        if old.as_ptr() != Arc::as_ptr(photo) {
            return false;
        }
        let slot = Slot::Thumb(photo.id);
        if size > bucket {
            // A larger thumbnail that has already arrived for this photo and settings serves a
            // smaller cell too (cells at different sizes share one photo). Asking for the smaller
            // bucket would replace the larger picture, and the larger cell would then ask for its
            // bucket back on the next frame: two print cells would alternate renders forever.
            // The photo identity above is the scheduler's `Weak<Photo>`: `Catalog` edits through
            // `Arc::make_mut`, which (per std) moves the value to a new allocation whenever a
            // `Weak` exists, even with one strong reference. An edit therefore breaks this match,
            // and the key keeps an older picture of the same allocation from being reused. A
            // larger request still renders once (below), and while it is in flight
            // [`Self::thumb_pending_other_bucket`] holds the smaller callers.
            return self.textures.get(&slot).is_some_and(|t| t.key == key);
        }
        if size != bucket {
            return false;
        }
        if quick && !self.textures.contains_key(&slot) {
            // Stand-in path (`grid::request_thumb`): the embedded preview is still on its way, or
            // the real thumbnail is requested at the background priority whatever the caller's.
            let quick_slot = Slot::ThumbQuick(photo.id);
            if let Some(&(k, p)) = self.pending.get(&quick_slot)
                && k == key
            {
                return p == priority.saturating_add(1) || !self.is_queued(quick_slot);
            }
            return !self.request_needed(slot, key, BACKGROUND_THUMB_PRIORITY);
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
        // (a preview that landed in this frame's sync step: its cell may draw it now, not a frame from now,
        // when an idle app may not run one)
        self.preview_arrived(session);
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
            && (self.thumb_quick_is_final(photo, bucket, priority) || self.thumb_quick_pending(photo, bucket, priority))
        {
            return false;
        }
        // Several cells can show one photo at different sizes (a print page's cells). Before any
        // thumbnail arrives, a caller at another bucket would replace the pending request every frame
        // and restart it, so none of them ever gets a picture: keep the request in flight. A higher
        // priority caller may take it over once; the caller it replaced then coalesces behind it.
        if let Some(photo) = session.catalog.photo(id)
            && self.thumb_pending_other_bucket(photo, bucket, priority)
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
                self.remember_embedded_thumb(photo, bucket, job.key, priority);
            } else {
                self.remember_thumb(photo, bucket, job.key, quick.is_some(), priority);
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
    /// embedded JPEG already covers — see `grid::skips_the_real_render`. A preview that arrived at
    /// a larger bucket and is still held is final for a smaller cell too, so print cells at
    /// different sizes do not replace it every frame.
    pub fn thumb_quick_is_final(&self, photo: &Arc<lightcraft_catalog::Photo>, bucket: usize, priority: u32) -> bool {
        let Some((old, size, key)) = self.embedded_thumbs.get(&photo.id) else { return false };
        if old.as_ptr() != Arc::as_ptr(photo) || *size < bucket {
            return false;
        }
        let slot = Slot::ThumbQuick(photo.id);
        if *size > bucket {
            // A larger preview serves a smaller cell only when its pixels are actually here for
            // the recorded key. The same-bucket failure memoization below says "no" after a failed
            // job, but a missing larger preview is not a final picture: the smaller cell must ask
            // for its own bucket rather than stay blank until the catalog revision changes.
            return self.textures.get(&slot).is_some_and(|t| t.key == *key);
        }
        // The requested bucket itself: keep the failure memoization (`failed`) unchanged.
        !self.request_needed(slot, *key, priority)
    }

    /// Whether this photo's embedded-preview thumbnail is pending at the requested priority, or
    /// already running. A queued priority change must reach the grid's stand-in request.
    pub fn thumb_quick_pending(&self, photo: &Arc<lightcraft_catalog::Photo>, bucket: usize, priority: u32) -> bool {
        let Some((old, size, key, quick, _)) = self.thumb_inputs.get(&photo.id) else { return false };
        let slot = Slot::ThumbQuick(photo.id);
        old.as_ptr() == Arc::as_ptr(photo)
            && *size == bucket
            && *quick
            && self.pending.contains_key(&slot)
            && !self.request_needed(slot, *key, priority.saturating_add(1))
    }

    /// Whether a request for this photo's thumbnail is already in flight at another bucket and a
    /// caller asking at `bucket` should leave it there. Several cells can show one photo at
    /// different sizes at once (a print page's cells), and replacing `Slot::Thumb(id)` from each of
    /// them every frame restarts the render, so none of them ever gets a picture.
    ///
    /// A caller with a higher priority than the one that queued the request takes it over once;
    /// from then on the original caller coalesces behind that higher priority instead of replacing
    /// it back. Equal priorities keep the request that is in flight (the print cells' case). This
    /// only chooses which pending bucket survives; after it completes, a larger bucket is reused by
    /// smaller callers ([`Self::thumb_current`]) rather than being replaced every frame.
    ///
    /// A full render queued behind a stand-in is submitted at [`BACKGROUND_THUMB_PRIORITY`],
    /// deliberately below an on-screen caller. That is not the caller's priority, so the comparison
    /// uses the priority recorded with the inputs: comparing the background slot's own priority
    /// would let two buckets keep replacing each other's background render every frame.
    ///
    /// A photo that changed (another `Arc<Photo>`) or a texture for the recorded job key is not
    /// coalesced: its callers ask for their own bucket as before. A stale texture from before an
    /// edit does not count: it is still displayed while the replacement renders, and comparing
    /// the key (not presence) keeps coalescing across buckets in the meantime.
    pub fn thumb_pending_other_bucket(&self, photo: &Arc<lightcraft_catalog::Photo>, bucket: usize, priority: u32) -> bool {
        let Some(&(ref old, size, key, _, requested_priority)) = self.thumb_inputs.get(&photo.id) else { return false };
        if old.as_ptr() != Arc::as_ptr(photo) || size == bucket {
            return false;
        }
        if self.textures.get(&Slot::Thumb(photo.id)).is_some_and(|t| t.key == key) {
            return false;
        }
        // (the inputs must belong to the request still in flight, not to one that already ended)
        let in_flight = self.pending.get(&Slot::ThumbQuick(photo.id)).is_some_and(|p| p.0 == key)
            || self.pending.get(&Slot::Thumb(photo.id)).is_some_and(|p| p.0 == key);
        in_flight && requested_priority >= priority
    }

    /// Remember that the camera's embedded preview is this photo's final thumbnail, so the grid's
    /// per-frame requests stop at it (the quick render's key, which [`Self::thumb_quick_is_final`]
    /// compares against).
    pub fn remember_embedded_thumb(&mut self, photo: &Arc<lightcraft_catalog::Photo>, bucket: usize, key: u64, priority: u32) {
        let slot = Slot::Thumb(photo.id);
        if self.pending.get(&slot).is_some_and(|(pending_key, _)| *pending_key != key) {
            // An edit can still be rendering when Undo restores this embedded-final thumbnail.
            // Retire its full-render identity before it can replace the correct stand-in.
            self.pending.remove(&slot);
            self.requests.remove(&slot);
            self.pool.reprioritize(|s, p| if *s == slot { None } else { Some(p) });
            self.queue.retain(|q| q.slot != slot);
        }
        self.remember_thumb(photo, bucket, key, true, priority);
        self.embedded_thumbs.insert(photo.id, (Arc::downgrade(photo), bucket, key));
    }

    pub fn remember_thumb(&mut self, photo: &Arc<lightcraft_catalog::Photo>, bucket: usize, key: u64, quick: bool, priority: u32) {
        if self.thumb_inputs.len() >= 1024 && !self.thumb_inputs.contains_key(&photo.id) {
            self.thumb_inputs.retain(|_, (p, _, _, _, _)| p.strong_count() > 0);
            if self.thumb_inputs.len() >= 1024
                && let Some(id) = self.thumb_inputs.keys().next().copied()
            {
                self.thumb_inputs.remove(&id);
            }
        }
        self.thumb_inputs.insert(photo.id, (Arc::downgrade(photo), bucket, key, quick, priority));
    }

    /// Views that render straight into the display profile's primaries (wide gamut); everything
    /// else is rendered for sRGB and converted ([`lightcraft_engine::display::present`]).
    fn renders_for_display(slot: Slot) -> bool {
        matches!(
            slot,
            Slot::Main
                | Slot::Region
                | Slot::Before
                | Slot::RegionBefore
                | Slot::Hover
                | Slot::Compare(_)
                | Slot::CompareRegion(_)
                | Slot::Second
                | Slot::Slide(_)
        )
    }

    /// The slots of the photo being edited, which show an HDR edit in HDR when the display can
    /// ([`Self::set_hdr_display`]): the loupe and its zoom window. The Before views show the
    /// unedited photo, and everything else is small SDR previews.
    fn renders_hdr(slot: Slot) -> bool {
        matches!(slot, Slot::Main | Slot::Region)
    }

    /// `job` as [`Self::request`] would run it for `slot` (its key included): views render for
    /// the display profile in use, and the loupe's HDR edits for an HDR display. Applying it again
    /// changes nothing.
    pub fn job_for(&self, slot: Slot, job: RenderJob) -> RenderJob {
        let job = if Self::renders_for_display(slot) { job.with_display(self.display.clone()) } else { job };
        let job = if self.hdr_display && Self::renders_hdr(slot) { job.with_hdr_display() } else { job };
        // a drag's drafts are only drawn: they may stay on the GPU when the frontend can show them
        // (`on_screen` changes no key)
        if self.screen_frames && Self::renders_for_display(slot) && job.request.quality == lightcraft_engine::pipeline::Quality::Draft {
            job.on_screen()
        } else {
            job
        }
    }

    /// The frontend can (or no longer can) show renders that stayed on the GPU ([`Present::show_frame`]):
    /// views' interactive drafts are then left there instead of read back as pixels.
    pub fn set_screen_frames(&mut self, on: bool) {
        self.screen_frames = on;
    }

    /// Whether views' drafts may stay on the GPU ([`Self::set_screen_frames`]).
    pub fn screen_frames(&self) -> bool {
        self.screen_frames
    }

    /// The display can (or no longer can) show the range above SDR white: the loupe of an HDR edit
    /// renders its HDR picture ([`Present::set_hdr`]), or goes back to the SDR rendition. Asked
    /// again when the window moves to another display.
    pub fn set_hdr_display(&mut self, on: bool) {
        self.hdr_display = on;
    }

    /// Whether the loupe renders HDR edits in HDR ([`Self::set_hdr_display`]).
    pub fn hdr_display(&self) -> bool {
        self.hdr_display
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

    pub fn request_needed(&self, slot: Slot, key: u64, priority: u32) -> bool {
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
        self.requests.remove(&slot);
        self.shown_ids.remove(&slot);
        self.quick_tried.remove(&slot);
        self.prefetched.remove(&slot);
        self.deferred_prefetch.retain(|(s, _, _)| *s != slot);
        self.release(slot);
    }

    /// Drop what a view slot's stage cache holds (its next render starts afresh).
    pub fn clear_stages(&mut self, slot: Slot) {
        if let Some(c) = self.stages.get(&slot) {
            c.clear();
        }
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

    /// Run jobs on the job pool (or inline) again, not through an offload (tests).
    pub fn clear_offload(&mut self) {
        self.offload = None;
    }

    fn is_queued(&self, slot: Slot) -> bool {
        self.queue.iter().any(|q| q.slot == slot) || self.pool.is_queued(slot)
    }

    /// The linear scale the loupe renders at: 1 unless a slider is dragging (`drag`: its interaction label),
    /// then what this host renders in time (see [`DraftBudget`]). `full_px`: the pixels of the undrafted render.
    pub fn draft_scale(&mut self, drag: Option<&str>, full_px: f64) -> f32 {
        // (asked without blocking: before the device is up, renders are on the CPU)
        self.draft.gpu = lightcraft_engine::gpu::ready() && lightcraft_engine::gpu::available();
        // (the picture on screen says which kind the drag's drafts are: a drag doesn't switch HDR)
        self.draft.scale(drag, full_px, self.main_hdr)
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
        if matches!(slot, Slot::Slide(_)) {
            self.slides_live = true;
        }
        let mut job = self.job_for(slot, job);
        if !self.request_needed(slot, job.key, priority) {
            return;
        }
        self.note_request(slot, job.key, priority, "requested", || render_subject(slot, job.photo, Some(&job.origin)));
        self.pending.insert(slot, (job.key, priority));
        job.request_id = lightcraft_preview::next_tick();
        let look = look_of(&job);
        self.note_look(job.request_id, look);
        self.requests.insert(slot, Requested { look: Some(look), id: job.request_id, source: Some(job.origin.clone()) });
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

    /// Render a slideshow title screen into `slot` (once per screen key, like [`Self::request`]):
    /// a slide with no photo, composed on a worker (on wasm: the job pool's inline runs, not the
    /// offload, which only knows photo renders).
    pub fn request_title(&mut self, slot: Slot, compose: Arc<lightcraft_engine::slide_title::TitleCompose>, priority: u32) {
        if matches!(slot, Slot::Slide(_)) {
            self.slides_live = true;
        }
        let key = compose.key();
        if !self.request_needed(slot, key, priority) {
            return;
        }
        self.note_request(slot, key, priority, "title screen requested", || format!("render {slot:?}"));
        self.pending.insert(slot, (key, priority));
        let request_id = lightcraft_preview::next_tick();
        self.requests.insert(slot, Requested { look: None, id: request_id, source: None });
        let display = self.display.clone();
        let failed = Failed { request_id, source_key: None, photo: PhotoId(0), level: lightcraft_engine::media::SourceLevel::Preview, key };
        self.pool.submit(slot, key, priority, Box::new(move || run_for(failed, || compose.result(request_id), &display)));
    }

    /// Drop the slideshow slides other than `keep`: their textures and their requests that have
    /// not started (a slide that is no longer near is not worth rendering).
    pub fn release_slides(&mut self, keep: &[Item]) {
        if !self.slides_live {
            return;
        }
        let gone = |s: &Slot| matches!(s, Slot::Slide(id) if !keep.contains(id));
        self.abandon_pending(gone, "no longer near the slide shown");
        self.textures.retain(|s, _| !gone(s));
        let _ = self.pool.reprioritize(|s, p| if gone(s) { None } else { Some(p) });
        self.queue.retain(|q| !gone(&q.slot));
        // (a render already running finishes unclaimed: its result is dropped)
        self.pending.retain(|s, _| !gone(s));
        self.requests.retain(|s, _| !gone(s));
        self.failed.retain(|s, _| !gone(s));
        self.slides_live = !keep.is_empty();
    }

    /// Prepare a visible import preview at its size bucket, retaining a sharper existing texture.
    pub fn import_preview(&mut self, session: &mut Session, candidate: &lightcraft_engine::import::ImportCandidate, index: u32, edge: usize) {
        let slot = Slot::Import(index);
        let edge = edge.clamp(64, 1024);
        if candidate.error.is_some() || self.textures.get(&slot).is_some_and(|tex| tex.size.into_iter().max().unwrap_or(0) >= edge) {
            return;
        }
        let key = lightcraft_engine::import::candidate_thumb_key(candidate, edge);
        if self.quick_tried.get(&slot) == Some(&key) {
            return;
        }
        self.import_jobs_built += 1;
        if let Some(job) = session.candidate_thumb_job(candidate, edge, u64::from(index)) {
            self.request_quick(slot, job, 4);
        }
    }

    /// Request a stand-in for `slot` (once per job key). The loupe's [`Slot::Preview`] takes over
    /// a stand-in already made ahead for the same photo ([`Slot::QuickAhead`]) instead of making it
    /// again: shown in this frame when it is ready, or as soon as that job finishes.
    pub fn request_quick(&mut self, slot: Slot, mut job: QuickJob, priority: u32) {
        if self.quick_tried.get(&slot) == Some(&job.key) {
            if self.pending.get(&slot).is_some_and(|p| p.0 == job.key && p.1 != priority) {
                // Scrolling changes a stand-in's priority too. Keep the original closure and
                // request identity; running or completed work must never be started again.
                let mut queued = false;
                self.pool.reprioritize(|s, p| {
                    if *s == slot {
                        queued = true;
                        Some(priority)
                    } else {
                        Some(p)
                    }
                });
                if queued && let Some(p) = self.pending.get_mut(&slot) {
                    p.1 = priority;
                    if self.logs_decisions() {
                        let source = self.requests.get(&slot).and_then(|r| r.source.as_ref());
                        let line = format!("{}: still waiting, moved to priority {priority}", render_subject(slot, job.photo, source));
                        self.decided(|| line);
                    }
                }
            }
            return;
        }
        self.quick_tried.insert(slot, job.key);
        if matches!(slot, Slot::Preview | Slot::QuickAhead(_)) {
            self.quick_edge.insert(slot, quick_edge(&job));
        }
        if slot == Slot::Preview && self.take_ahead(job.photo, job.key, quick_edge(&job)) {
            self.pending.remove(&slot);
            self.requests.remove(&slot);
            return;
        }
        let source = job
            .small
            .as_ref()
            .map(|job| job.origin.clone())
            .or_else(|| job.embedded.as_ref().map(|(path, _, _)| lightcraft_catalog::Source::File { path: path.clone() }));
        self.note_request(slot, job.key, priority, "stand-in requested", || render_subject(slot, job.photo, source.as_ref()));
        self.pending.insert(slot, (job.key, priority));
        job.request_id = lightcraft_preview::next_tick();
        let look = job.small.as_ref().map(|j| look_of(j));
        if let Some(look) = look {
            self.note_look(job.request_id, look);
        }
        self.requests.insert(slot, Requested { look, id: job.request_id, source });
        let key = job.key;
        let display = self.display.clone();
        let failed =
            Failed { request_id: job.request_id, source_key: None, photo: job.photo, level: lightcraft_engine::media::SourceLevel::Thumb, key };
        let background = matches!(slot, Slot::QuickAhead(_));
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

    /// Whether the stand-in asked for in `slot` is at least `edge` pixels (see `quick_edge`).
    fn ahead_covers(&self, slot: Slot, edge: usize) -> bool {
        self.quick_edge.get(&slot).copied().unwrap_or(0) >= edge
    }

    /// Move a [`Slot::QuickAhead`] texture of `photo` made for `key`, at least `edge` pixels, into
    /// [`Slot::Preview`].
    fn take_ahead(&mut self, photo: PhotoId, key: u64, edge: usize) -> bool {
        let Some(slot) = self
            .textures
            .iter()
            .find(|(s, t)| matches!(s, Slot::QuickAhead(_)) && t.photo == photo && t.key == key && self.ahead_covers(**s, edge))
            .map(|(s, _)| *s)
        else {
            return false;
        };
        let Some(tex) = self.textures.remove(&slot) else { return false };
        self.quick_tried.remove(&slot);
        self.textures.insert(Slot::Preview, tex);
        true
    }

    /// Make the loupe's stand-ins for the photos it is likely to be stepped to next (see
    /// [`Slot::QuickAhead`]), most likely first: `jobs[0]` gets `priority`, each next one less.
    /// A slot keeps a photo for as long as the photo stays wanted (made or being made, it is not
    /// made again when the user steps and the photos shift places); slots holding photos not
    /// wanted any more are freed for the new ones, and their queued jobs dropped.
    pub fn look_ahead(&mut self, jobs: Vec<QuickJob>, priority: u32) {
        let slots: Vec<Slot> = (0..AHEAD_SLOTS).map(Slot::QuickAhead).collect();
        let held = |r: &Self, s: Slot| r.textures.get(&s).map(|t| t.key).or_else(|| r.quick_tried.get(&s).copied());
        // (a stand-in asked for at a smaller loupe size isn't kept for a larger one)
        let wanted: HashMap<u64, usize> = jobs.iter().map(|j| (j.key, quick_edge(j))).collect();
        let mut free = Vec::new();
        for &s in &slots {
            let key = held(self, s);
            if key.is_none_or(|k| wanted.get(&k).is_none_or(|&edge| !self.ahead_covers(s, edge))) {
                self.textures.remove(&s);
                // a job already running: its result is thrown away when it arrives (not left as a
                // hidden stand-in), unless the loupe is waiting for just that one (a step to the
                // photo): then the slot stays reserved for it until it arrives
                let preview_edge = self.quick_edge.get(&Slot::Preview).copied().unwrap_or(0);
                let loupe_waits =
                    key.is_some_and(|k| self.pending.get(&Slot::Preview).is_some_and(|p| p.0 == k)) && self.ahead_covers(s, preview_edge);
                if !self.drop_queued(s) && loupe_waits {
                    continue;
                }
                self.quick_tried.remove(&s);
                self.pending.remove(&s);
                self.requests.remove(&s);
                free.push(s);
            }
        }
        let have: HashMap<u64, Slot> = slots.iter().filter_map(|&s| held(self, s).map(|k| (k, s))).collect();
        let mut free = free.into_iter();
        for (rank, job) in jobs.into_iter().enumerate() {
            let priority = priority.saturating_sub(u32::try_from(rank).unwrap_or(u32::MAX));
            if let Some(&s) = have.get(&job.key) {
                // kept: a job still queued moves to its new place in the order
                if self.quick_tried.get(&s) == Some(&job.key) {
                    self.request_quick(s, job, priority);
                }
                continue;
            }
            let Some(slot) = free.next() else { break };
            self.request_quick(slot, job, priority);
        }
    }

    /// Prepare a photo in the background (a [`Slot::Prefetch`] job runs once per key; a newer one
    /// replaces it while queued). Memory stays bounded by the engine's source caches.
    ///
    /// Not while the loupe's own render is under way: the two side by side (a 24 MP raw takes ~400 MB to decode and
    /// render) on top of the full caches were what ended the phone app for memory while swiping. It starts when that
    /// render ends, and is skipped when the device has too little memory left ([`PREFETCH_MIN_LEFT`]).
    pub fn prefetch(&mut self, slot: Slot, mut job: RenderJob, priority: u32) {
        if self.prefetched.get(&slot) == Some(&job.key) {
            return;
        }
        if self.pending.contains_key(&Slot::Main) {
            self.deferred_prefetch.retain(|(s, _, _)| *s != slot);
            self.deferred_prefetch.push((slot, job, priority));
            return;
        }
        self.prefetched.insert(slot, job.key);
        self.note_request(slot, job.key, priority, "prefetch requested", || render_subject(slot, job.photo, Some(&job.origin)));
        self.pending.insert(slot, (job.key, priority));
        job.request_id = lightcraft_preview::next_tick();
        let look = look_of(&job);
        self.note_look(job.request_id, look);
        self.requests.insert(slot, Requested { look: Some(look), id: job.request_id, source: Some(job.origin.clone()) });
        let key = job.key;
        let failed = Failed::of(&job);
        self.pool.submit(
            slot,
            key,
            priority,
            Box::new(move || {
                // (asked again when the job starts: memory goes while it waits in the queue)
                if lightcraft_sysmem::available_memory().is_some_and(|left| left < PREFETCH_MIN_LEFT) {
                    return failed.result("skipped: little memory left".into());
                }
                run_for(failed, || lightcraft_engine::memory::in_background(|| job.run()), &None)
            }),
        );
    }

    /// Prepared-ahead jobs ([`Slot::Prefetch`]) queued or running.
    pub fn prefetches_pending(&self) -> usize {
        self.pending.keys().filter(|s| matches!(s, Slot::Prefetch(_))).count()
    }

    /// Prepared-ahead slots asked for since the renderer last forgot everything.
    pub fn prefetch_slots_asked(&self) -> usize {
        self.prefetched.keys().filter(|s| matches!(s, Slot::Prefetch(_))).count()
    }

    /// Whether `slot` was last asked for the job of `key` (queued, running or done).
    pub fn prefetch_asked(&self, slot: Slot, key: u64) -> bool {
        self.prefetched.get(&slot) == Some(&key)
    }

    /// The prefetches put off while the loupe's render was under way, once it has ended.
    fn start_deferred_prefetch(&mut self) {
        if self.deferred_prefetch.is_empty() || self.pending.contains_key(&Slot::Main) {
            return;
        }
        for (slot, job, priority) in std::mem::take(&mut self.deferred_prefetch) {
            self.prefetch(slot, job, priority);
        }
    }

    /// Is a request for `slot` queued or running?
    /// Remember the geometry identity of request `id` until its result is collected.
    fn note_look(&mut self, id: u64, look: (u64, bool)) {
        // (bounded: results of cancelled jobs never come back to remove theirs)
        if self.request_looks.len() >= 4096 {
            self.request_looks.clear();
        }
        self.request_looks.insert(id, look);
    }

    /// Drop every texture and cached stage (another library was opened: photo ids changed meaning).
    pub fn forget_all(&mut self) {
        self.abandon_pending(|_| true, "the renderer started over (another library, or the previews were cleared)");
        self.requests.clear();
        self.request_looks.clear();
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
        self.deferred_prefetch.clear();
        self.pending.clear();
        self.queue.clear();
    }

    /// Changes when [`Self::forget_all`] drops every texture and request: what was asked for before is gone.
    pub fn epoch(&self) -> u64 {
        self.epoch
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

    /// Requests queued or running, excluding background preparation of neighbouring photos.
    pub fn in_flight_without_prefetch(&self) -> usize {
        self.pending.keys().filter(|slot| !matches!(slot, Slot::Prefetch(_))).count()
    }

    /// Yield to native render workers until a result is ready or the timeout expires.
    /// The result remains queued for the next poll. Browser and offloaded renderers do not wait.
    /// Any native pending job, including prefetch, can satisfy the wait. A zero-worker test pool
    /// must run its jobs inline first, or this call can only time out.
    pub fn wait_for_worker(&mut self, timeout: Duration) -> bool {
        if cfg!(target_arch = "wasm32") || self.offload.is_some() || self.pending.is_empty() {
            return false;
        }
        self.pool.wait(timeout)
    }

    /// Test seam: make the scheduler look busy without starting a job (capture deadlines).
    pub fn force_in_flight_for_test(&mut self, count: usize) {
        self.pending.clear();
        for i in 0..count {
            self.pending.insert(Slot::Prefetch((i % 256) as u8), (0, 0));
        }
    }

    /// Why the last render for `slot` failed (until a render with another key succeeds).
    pub fn failure(&self, slot: Slot) -> Option<&str> {
        self.failed.get(&slot).map(|f| f.1.as_str())
    }

    /// Why the last render of `photo` for `slot` failed: a view slot shows one photo after
    /// another, and another photo's error isn't this one's.
    pub fn failure_of(&self, slot: Slot, photo: PhotoId) -> Option<&str> {
        self.failed.get(&slot).filter(|f| f.3 == photo).map(|f| f.1.as_str())
    }

    /// Whether `slot`'s pending render asks again for the key that last failed (a library change
    /// to its photo retries it): what is on screen stays what the failure left until it answers.
    pub fn retrying(&self, slot: Slot) -> bool {
        self.pending.get(&slot).zip(self.failed.get(&slot)).is_some_and(|(p, f)| p.0 == f.0)
    }

    /// Follow the library to `catalog`'s revision. A failure stands while its photo is unchanged:
    /// only the photos the ops since the last poll touched are tried again, and every photo when
    /// the catalog can't say which. So faces found or metadata written for other photos don't
    /// send each undecodable file back to the decoder (issue #2682: 37 tries each in 154 s idle).
    /// A file `files` knows to be missing (its drive unplugged) is still tried at every change, so
    /// its photo comes back with the drive, as before.
    pub fn follow_catalog(&mut self, catalog: &lightcraft_catalog::Catalog, files: &lightcraft_engine::availability::Availability) {
        let rev = catalog.revision;
        if rev == self.catalog_rev {
            return;
        }
        if let Some(touched) = catalog.touched_since(self.catalog_rev) {
            let missing =
                |id| matches!(catalog.photo(id).map(|p| &p.source), Some(lightcraft_catalog::Source::File { path }) if files.is_offline(path));
            for f in self.failed.values_mut() {
                if f.2 == self.catalog_rev && touched.binary_search(&f.3).is_err() && !missing(f.3) {
                    f.2 = rev;
                }
            }
        }
        self.catalog_rev = rev;
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
            self.requests.remove(&s);
        }
        let requests = &mut self.requests;
        let pending = &mut self.pending;
        self.queue.retain(|q| match q.slot {
            Slot::Variant(k) if stale(used.get(&k)) => {
                pending.remove(&q.slot);
                requests.remove(&q.slot);
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
                for id in &ids {
                    self.failed.remove(&Slot::Thumb(*id));
                    self.thumb_inputs.remove(id);
                }
                // (a People face or a browser's variant cut from that preview too)
                self.failed.retain(|slot, f| !(matches!(slot, Slot::Variant(_)) && ids.contains(&f.3)));
            }
            None => {
                self.failed.retain(|slot, _| !matches!(slot, Slot::Thumb(_) | Slot::Variant(_)));
                self.thumb_inputs.clear();
            }
        }
        // previews built again on this device (Rebuild Previews): a thumbnail that was drawn is drawn again at the
        // size and priority it was asked at, and the new picture reaches the cell without it asking (its key
        // changed: `MediaCache::preview_gen`). The old picture stays until then. (Its remembered inputs go first: they
        // hold the old key, which `thumb_current` would find drawn, and nothing would be asked for.)
        for id in session.media.take_rebuilt() {
            self.failed.remove(&Slot::Thumb(id));
            if let Some((_, bucket, _, _, priority)) = self.thumb_inputs.remove(&id) {
                self.request_thumb(session, id, bucket, priority);
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
        self.follow_catalog(&session.catalog, &session.media.availability);
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
        let mut polled = self.collect(session, present, inline_ms);
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

    /// In a slider drag, give the loupe's draft (just requested) until `budget` to finish, so that it
    /// is drawn in this frame rather than picked up at the start of the next one. Only when the last
    /// loupe render took well under the budget (a GPU draft takes a few milliseconds; a CPU render
    /// that would only make the frame late is not waited for).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn wait_for_draft(&mut self, session: &mut Session, present: &mut dyn Present<V>, budget: Duration) {
        if self.offload.is_some() || self.last_main_ms * 1.5 >= budget.as_secs_f64() * 1e3 {
            return;
        }
        let Some(deadline) = std::time::Instant::now().checked_add(budget) else { return };
        while self.is_pending(Slot::Main) {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() || !self.pool.wait(left) {
                break;
            }
            self.collect(session, present, 0.0);
        }
    }

    /// Turn finished jobs into what the slots show.
    #[allow(clippy::cognitive_complexity)] // fork code that predates the lint; split it when it is next reworked
    fn collect(&mut self, session: &mut Session, present: &mut dyn Present<V>, inline_ms: f64) -> Polled {
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
            let completed_look = self.request_looks.remove(&r.request_id);
            // made for another display profile than the one in use: obsolete
            if r.display.is_some_and(|id| Some(id) != self.display_id()) {
                self.decided(|| {
                    let source = session.catalog.photo(r.photo).map(|p| &p.source);
                    format!("{}: abandoned, made for another display profile", render_subject(slot, r.photo, source))
                });
                continue;
            }
            // Ignore obsolete pixels, failures and decoded sources, even for the same render key;
            // an interactive view still takes a superseded draft that is newer than what it
            // shows, and the source it decoded: a drag supersedes every request before it
            // finishes, and a source thrown away with them is decoded again for every tick.
            let (source, look) = if self.requests.get(&slot).map(|request| request.id) != Some(r.request_id) {
                let newer_draft = slot.is_view()
                    && self.requests.contains_key(&slot)
                    && r.request_id >= self.epoch
                    && self.shown_ids.get(&slot).is_none_or(|shown| r.request_id > *shown)
                    && r.rendered.is_ok();
                if !newer_draft {
                    self.discarded += 1;
                    let replaced = self.requests.contains_key(&slot);
                    self.decided(|| {
                        let why = if replaced { "a newer request replaced it" } else { "no longer wanted when it finished" };
                        let source = session.catalog.photo(r.photo).map(|p| &p.source);
                        format!("{}: abandoned, {why}", render_subject(slot, r.photo, source))
                    });
                    continue;
                }
                session.accept(&r);
                // (no failure diagnostic for a superseded success; it keeps the geometry of its own pixels, never the newer request's)
                (None, completed_look)
            } else {
                let request = self.requests.remove(&slot);
                let look = request.as_ref().and_then(|request| request.look);
                let source = request.and_then(|request| request.source);
                session.accept(&r);
                (source, look)
            };
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
            slot = self.stand_in_slot(slot, &r);
            let hdr = r.hdr.take();
            let is_hdr = hdr.is_some();
            // a render that stayed on the GPU: drawn from its texture
            if let (Some(frame), Ok(rendered)) = (r.frame.as_ref(), r.rendered.as_ref()) {
                if self.show_frame(present, slot, frame, rendered, &r, look, ms) {
                    polled.shown.push(slot);
                }
                continue;
            }
            let rendered = match r.rendered {
                Ok(x) => {
                    self.failed.remove(&slot);
                    // (a stand-in that comes back after the thumbnail itself is thrown away below)
                    let unused = matches!(slot, Slot::ThumbQuick(id) if self.textures.contains_key(&Slot::Thumb(id)));
                    self.decided(|| {
                        let source = source.as_ref().or_else(|| session.catalog.photo(r.photo).map(|p| &p.source));
                        let subject = render_subject(slot, r.photo, source);
                        if unused {
                            format!("{subject}: abandoned, the thumbnail itself came first")
                        } else {
                            format!("{subject}: produced in {ms:.0} ms")
                        }
                    });
                    x
                }
                Err(e) => {
                    if matches!(slot, Slot::Prefetch(_) | Slot::QuickAhead(_)) {
                        // (a prefetch only prepares: its failure is no warning, but the trace says how it ended)
                        self.decided(|| format!("{}: prefetch failed: {e}", render_subject(slot, r.photo, source.as_ref())));
                    } else {
                        let message = render_failure_message(slot, r.photo, source.as_ref(), &e);
                        log::warn!("{message}");
                        if self.failure_messages.len() >= FAILURE_MESSAGES_KEPT {
                            self.failure_messages.remove(0);
                        }
                        self.failure_messages.push(message);
                        polled.failed.push((slot, r.photo, r.key, e.clone()));
                        self.failed.insert(slot, (r.key, e, self.catalog_rev, r.photo));
                    }
                    continue;
                }
            };
            if matches!(slot, Slot::Prefetch(_)) && !self.keep_neighbours {
                continue;
            }
            if slot == Slot::HdrHistogram {
                if rendered.histogram.hdr.is_some() {
                    self.live_hdr_histogram = Some((r.photo, r.key, rendered.histogram));
                    polled.shown.push(slot);
                }
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
            // (a photo's first thumbnail: not a render replacing its stand-in, nor one made again)
            if let Slot::Thumb(id) | Slot::ThumbQuick(id) = slot
                && self.thumbs_counted.insert((library_key(session), id))
            {
                self.thumbs_made += 1;
            }
            if let Slot::Thumb(id) = slot {
                self.textures.remove(&Slot::ThumbQuick(id));
            }
            let size = [rendered.image.width, rendered.image.height];
            let draft = self.drafted.contains(&r.request_id);
            match self.textures.get_mut(&slot) {
                Some(t) => {
                    present.update(slot, rendered.image, &mut t.view);
                    if Self::renders_hdr(slot) {
                        present.set_hdr(slot, hdr, &mut t.view);
                    }
                    t.look = look;
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
                    let mut view = present.make(slot, rendered.image);
                    if Self::renders_hdr(slot) {
                        present.set_hdr(slot, hdr, &mut view);
                    }
                    let shown = Shown {
                        look,
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
                // (a preview taken from the cache says nothing about how long a render takes)
                if r.quick.is_none() {
                    self.last_main_ms = ms;
                }
                self.main_hdr = is_hdr;
                if let Some(i) = self.draft_ids.iter().position(|id| *id == r.request_id) {
                    self.draft_ids.swap_remove(i);
                    self.draft.note(size, ms, is_hdr);
                }
            }
            polled.shown.push(slot);
        }
        self.start_deferred_prefetch();
        polled
    }

    /// The slot a finished job goes to. Made ahead for the photo the loupe is now waiting for, it is
    /// the loupe's stand-in (the loupe's own request for it, if one was started, is dropped when it
    /// arrives); also when the loupe's own request failed and it still wants that photo: the
    /// stand-in made ahead still shows, but never over another photo's request.
    fn stand_in_slot(&mut self, slot: Slot, r: &RenderResult) -> Slot {
        let waiting = self.pending.get(&Slot::Preview).is_some_and(|p| p.0 == r.key);
        let failed = !self.pending.contains_key(&Slot::Preview)
            && self.quick_tried.get(&Slot::Preview) == Some(&r.key)
            && self.failed.get(&Slot::Preview).is_some_and(|f| f.0 == r.key && f.3 == r.photo);
        if !matches!(slot, Slot::QuickAhead(_))
            || r.rendered.is_err()
            || !(waiting || failed)
            || !self.ahead_covers(slot, self.quick_edge.get(&Slot::Preview).copied().unwrap_or(0))
        {
            return slot;
        }
        self.pending.remove(&Slot::Preview);
        self.requests.remove(&Slot::Preview);
        self.quick_tried.remove(&slot);
        Slot::Preview
    }

    /// Show a render that stayed on the GPU in `slot` (through `present`); whether it could be. When
    /// it can't, views come back as pixels from then on, and the pictures drawn from the GPU go.
    #[allow(clippy::too_many_arguments)]
    fn show_frame(
        &mut self,
        present: &mut dyn Present<V>,
        slot: Slot,
        frame: &ScreenFrame,
        rendered: &lightcraft_engine::pipeline::Rendered,
        r: &RenderResult,
        look: Option<(u64, bool)>,
        ms: f64,
    ) -> bool {
        self.failed.remove(&slot);
        let size = [frame.width, frame.height];
        let draft = self.drafted.contains(&r.request_id);
        match present.show_frame(slot, frame, self.textures.get_mut(&slot).map(|t| &mut t.view)) {
            FrameShown::Updated => {
                if let Some(t) = self.textures.get_mut(&slot) {
                    t.look = look;
                    t.key = r.key;
                    t.photo = r.photo;
                    t.size = size;
                    t.histogram = Some(rendered.histogram.clone());
                    t.ms = ms;
                    t.quick = None;
                    t.request_id = r.request_id;
                    t.draft = draft;
                }
            }
            FrameShown::Made(view) => {
                let shown = Shown {
                    look,
                    key: r.key,
                    photo: r.photo,
                    size,
                    histogram: Some(rendered.histogram.clone()),
                    ms,
                    quick: None,
                    request_id: r.request_id,
                    draft,
                    view,
                };
                self.textures.insert(slot, shown);
            }
            FrameShown::Refused => {
                log::warn!("render {slot:?}: a GPU-resident render can't be shown; rendering pixels");
                self.screen_frames = false;
                // a view drawn from a texture the frontend let go would show nothing: it is rendered again, as pixels
                self.textures.retain(|_, t| !present.on_gpu(&t.view));
                return false;
            }
        }
        if slot == Slot::Main {
            self.last_main_ms = ms;
            if let Some(i) = self.draft_ids.iter().position(|id| *id == r.request_id) {
                self.draft_ids.swap_remove(i);
                self.draft.note(size, ms, false);
            }
        }
        true
    }

    /// Forget everything about `slot`: its texture, its queued job, and that its stand-in was asked
    /// for (so asking again makes it again).
    pub fn forget(&mut self, slot: Slot) {
        self.drop_queued(slot);
        self.pending.remove(&slot);
        self.requests.remove(&slot);
        self.quick_tried.remove(&slot);
        self.release(slot);
    }

    /// Drop `slot`'s job if it is still queued (a running one finishes). Returns whether one was.
    pub fn drop_queued(&mut self, slot: Slot) -> bool {
        let dropped = self.pool.reprioritize(|s, p| if *s == slot { None } else { Some(p) });
        let before = self.queue.len();
        self.queue.retain(|q| q.slot != slot);
        let any = !dropped.is_empty() || self.queue.len() != before;
        if any {
            self.pending.remove(&slot);
            self.requests.remove(&slot);
        }
        any
    }

    /// Log a render decision at debug level: each request, and how it ended when it ended without
    /// a picture or a failure (failures stay warnings). One "requested" line per request tells a
    /// file asked for 37 times from one asked once and failing inside (issue #2682). `line` is only
    /// built when the log takes debug lines (`LIGHTCRAFT_LOG=debug`, or
    /// `RUST_LOG=lightcraft_apphost::render=debug`) or [`Self::record_decisions`] is set, so the
    /// grid's requests cost nothing more otherwise.
    fn decided(&mut self, line: impl FnOnce() -> String) {
        if !self.logs_decisions() {
            return;
        }
        let line = line();
        log::debug!("{line}");
        if self.record_decisions {
            self.decisions.push(line);
        }
    }

    /// Whether render decisions are logged ([`Self::decided`]): the log takes debug lines from this
    /// module, or a test is looking.
    fn logs_decisions(&self) -> bool {
        self.record_decisions || log::log_enabled!(log::Level::Debug)
    }

    /// Log a new request for `slot`, and first what becomes of the one it replaces while that one
    /// still waits: a queue holds one job per slot, so a newer render drops it, and the same render
    /// is only moved to the new priority. One already running says so when its result comes back.
    fn note_request(&mut self, slot: Slot, key: u64, priority: u32, what: &str, subject: impl FnOnce() -> String) {
        if !self.logs_decisions() {
            return;
        }
        let waiting = self.pending.get(&slot).map(|p| p.0).filter(|_| self.is_queued(slot));
        if waiting == Some(key) {
            let line = format!("{}: still waiting, moved to priority {priority}", subject());
            self.decided(|| line);
            return;
        }
        if waiting.is_some() {
            let line = abandoned_line(slot, self.requests.get(&slot), "a newer request replaced it before it started");
            self.decided(|| line);
        }
        let again = if self.failed.get(&slot).is_some_and(|f| f.0 == key) { " again after a failure" } else { "" };
        let line = format!("{}: {what}{again}", subject());
        self.decided(|| line);
    }

    /// Log each request in `which` that still waits in a queue as abandoned (`why`), before they are
    /// dropped. One already running says so once, when its result comes back unclaimed.
    fn abandon_pending(&mut self, which: impl Fn(&Slot) -> bool, why: &str) {
        if !self.logs_decisions() {
            return;
        }
        let mut slots: Vec<Slot> = self.pending.keys().copied().filter(|s| which(s) && self.is_queued(*s)).collect();
        slots.sort_by_key(|s| format!("{s:?}"));
        for s in slots {
            let line = abandoned_line(s, self.requests.get(&s), why);
            self.decided(|| line);
        }
    }

    /// Free the GPU renderer's recycled buffers once nothing has rendered for [`IDLE_TRIM`] (they
    /// are reallocated by the next render; while working, renders reuse them). A zoomed view
    /// showing its windows is not idle: the device copy of the original they are cut from stays for
    /// the next pan (a 64 MP raw is 800 MB to upload again, over a second). Returns how long until
    /// the trim is due, while it is still to come.
    #[cfg(not(target_arch = "wasm32"))]
    fn trim_when_idle(&mut self) -> Option<Duration> {
        if !self.pending.is_empty() || self.textures.contains_key(&Slot::Region) || self.textures.contains_key(&Slot::RegionBefore) {
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

    /// Drop the import review thumbnails (a new review, or the dialog closed), a camera's included.
    pub fn forget_imports(&mut self) {
        self.retain_imports(&Default::default());
        let camera = |s: &Slot| matches!(s, Slot::Camera(_));
        self.textures.retain(|s, _| !camera(s));
        self.quick_tried.retain(|s, _| !camera(s));
    }

    /// Keep only the import tiles near the viewport, including queued work. Large thumbnails
    /// must not accumulate for every candidate while scrolling through a whole card.
    pub fn retain_imports(&mut self, keep: &std::collections::HashSet<u32>) {
        let retained = |s: &Slot| !matches!(s, Slot::Import(i) if !keep.contains(i));
        self.pool.reprioritize(|s, p| retained(s).then_some(p));
        self.queue.retain(|q| retained(&q.slot));
        self.pending.retain(|s, _| retained(s));
        self.requests.retain(|s, _| retained(s));
        self.textures.retain(|s, _| retained(s));
        self.quick_tried.retain(|s, _| retained(s));
        self.failed.retain(|s, _| retained(s));
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
            let request = self.requests.remove(&s);
            self.quick_tried.remove(&s);
            self.decided(|| abandoned_line(s, request.as_ref(), "scrolled out of view before it started"));
        }
        let mut gone = Vec::new();
        let requests = &mut self.requests;
        let pending = &mut self.pending;
        self.queue.retain(|q| match q.slot {
            Slot::Thumb(id) if !keep.contains(&id) && q.priority <= 10 => {
                pending.remove(&q.slot);
                gone.push((q.slot, requests.remove(&q.slot)));
                false
            }
            _ => true,
        });
        for (s, request) in gone {
            self.decided(|| abandoned_line(s, request.as_ref(), "scrolled out of view before it started"));
        }
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
            hdr: None,
            frame: None,
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
        r.remember_thumb(&old, 384, 77, false, 10);
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
                preview_checked: String::new(),
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
        r.remember_thumb(&p, 384, key, false, 5);
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
    fn a_rebuilt_preview_draws_the_thumbnail_again() {
        let p = photo();
        let slot = Slot::Thumb(p.id);
        let mut session = Session::new();
        session.catalog.apply(Op::AddPhoto { photo: Box::new((*p).clone()) }).unwrap();
        let mut r = Sched::default();
        r.set_offload(Box::new(Blocked));
        assert!(r.request_thumb(&mut session, p.id, 384, 5));
        let (old, _) = r.pending[&slot];
        assert!(!r.request_thumb(&mut session, p.id, 384, 5), "nothing new: nothing asked");
        // Rebuild Previews built this photo's previews again: the grid's thumbnail is drawn from them unasked
        session.media.note_rebuilt(p.id);
        r.preview_arrived(&mut session);
        let (new, priority) = r.pending[&slot];
        assert_ne!(new, old, "a render of the new previews");
        assert_eq!(priority, 5, "at the priority it was asked at");
    }
    #[test]
    fn render_failures_identify_the_file_for_thumbnail_and_view_slots() {
        let source = Source::File { path: "/photos/trip/IMG_0042.nef".into() };
        for slot in [Slot::Thumb(PhotoId(42)), Slot::Main, Slot::Before, Slot::Compare(1), Slot::Region] {
            let message = render_failure_message(slot, PhotoId(42), Some(&source), "unsupported compression");
            assert!(message.contains("/photos/trip/IMG_0042.nef"), "{message}");
            assert!(message.contains("PhotoId(42)"), "{message}");
            assert!(message.ends_with(": unsupported compression"), "{message}");
        }
    }

    #[test]
    fn render_failure_paths_are_escaped_and_missing_sources_still_identified() {
        let source = Source::File { path: "line\nbreak.nef".into() };
        let message = render_failure_message(Slot::Main, PhotoId(42), Some(&source), "decode failed");
        assert!(!message.contains('\n'));
        assert!(message.contains("line\\nbreak.nef"));
        assert!(render_failure_message(Slot::Main, PhotoId(42), None, "decode failed").contains("PhotoId(42), source unavailable"));
        let demo = Source::Demo { scene: 7 };
        assert!(render_failure_message(Slot::Main, PhotoId(42), Some(&demo), "decode failed").contains("demo scene 7"));
    }

    fn queued_quick_thumbnail(id: PhotoId) -> (Arc<Photo>, QuickJob) {
        let mut session = Session::new();
        session.media.preview_loader =
            Some(Arc::new(|_, _| Some(lightcraft_raster::Rgba8 { width: 4, height: 4, data: vec![[200, 100, 50, 255]; 16] })));
        let mut raw = (*photo()).clone();
        raw.id = id;
        raw.kind = lightcraft_catalog::MediaKind::Raw;
        raw.develop = Arc::new(raw.camera_defaults());
        session.catalog.apply(Op::AddPhoto { photo: Box::new(raw) }).unwrap();
        let p = session.catalog.photo(id).unwrap().clone();
        let job = session.thumb_job(id, 384).unwrap();
        (p, session.quick_thumb_job(&job).unwrap())
    }

    #[test]
    fn queued_standin_promotes_from_prefetch_to_visible_without_restarting() {
        let (p, quick) = queued_quick_thumbnail(PhotoId(1));
        let (_, other) = queued_quick_thumbnail(PhotoId(2));
        let slot = Slot::ThumbQuick(p.id);
        // Deterministic queue: no native worker starts.
        let mut r = Sched { pool: JobPool::new(0), ..Default::default() };
        r.remember_thumb(&p, 384, quick.key, true, 5);
        r.request_quick(slot, quick.clone(), 6); // grid prefetch: 5 + 1
        r.request_quick(Slot::ThumbQuick(PhotoId(2)), other, 9); // filmstrip: 8 + 1
        let request_id = r.requests[&slot].id;
        assert!(r.thumb_current(&p, 384, 5));
        assert!(!r.thumb_current(&p, 384, 10));
        r.request_quick(slot, quick.clone(), 11); // visible grid: 10 + 1
        assert!(r.thumb_current(&p, 384, 10));
        r.request_quick(slot, quick.clone(), 11);
        assert_eq!(r.requests[&slot].id, request_id);
        assert_eq!(r.pool.queued(), 2);
        r.pool.run_inline(1);
        let done = r.pool.try_recv().unwrap();
        assert_eq!(done.slot, slot);
        assert_eq!(done.result.request_id, request_id);
        assert_eq!(done.result.quick, Some(QuickSource::Embedded));
        // A finished result awaiting collection also must not start another extraction.
        r.request_quick(slot, quick, 6);
        assert!(r.thumb_current(&p, 384, 5));
        assert_eq!(r.pool.queued(), 1);
    }

    #[test]
    fn queued_standin_demotes_when_it_leaves_the_visible_grid() {
        let (p, quick) = queued_quick_thumbnail(PhotoId(1));
        let (_, other) = queued_quick_thumbnail(PhotoId(2));
        let slot = Slot::ThumbQuick(p.id);
        let mut r = Sched { pool: JobPool::new(0), ..Default::default() };
        r.remember_thumb(&p, 384, quick.key, true, 10);
        r.request_quick(slot, quick.clone(), 11);
        r.request_quick(Slot::ThumbQuick(PhotoId(2)), other, 9);
        assert!(!r.thumb_current(&p, 384, 5));
        r.request_quick(slot, quick, 6);
        assert!(r.thumb_current(&p, 384, 5));
        r.pool.run_inline(1);
        assert_eq!(r.pool.try_recv().unwrap().slot, Slot::ThumbQuick(PhotoId(2)));
    }

    #[test]
    fn failures_retry_after_revision_and_quick_only_is_not_final() {
        let p = photo();
        let slot = Slot::Thumb(p.id);
        let mut r = Sched::default();
        r.remember_thumb(&p, 384, 77, false, 10);
        r.pending.insert(Slot::ThumbQuick(p.id), (77, 11));
        assert!(!r.thumb_current(&p, 384, 10));
        r.failed.insert(slot, (77, "no decoder".into(), 0, p.id));
        assert!(r.thumb_current(&p, 384, 10));
        assert_eq!(r.failure_of(slot, p.id), Some("no decoder"));
        assert_eq!(r.failure_of(slot, PhotoId(p.id.0 + 1)), None, "another photo's error");
        r.catalog_rev = 1;
        assert!(!r.thumb_current(&p, 384, 10));
    }

    // Given a face that failed to render for want of its synced preview, the preview landing retries it
    /// A larger texture only serves a smaller bucket when it is the recorded request's picture
    /// (under a `contains_key`-only check the first assertion would return `true`).
    #[test]
    fn larger_texture_for_another_key_does_not_count_as_current() {
        let p = photo();
        let slot = Slot::Thumb(p.id);
        let tex =
            |key| Shown { look: None, key, photo: p.id, size: [1, 1], histogram: None, ms: 0.0, quick: None, request_id: 0, draft: false, view: () };
        let mut r = Sched::default();
        // A completed larger bucket: the remembered request is bucket 512, key 77.
        r.remember_thumb(&p, 512, 77, false, 10);
        // Another key's pixels (the picture from before an edit) may still be displayed, but they
        // are not the remembered request's picture.
        r.textures.insert(slot, tex(66));
        assert!(!r.thumb_current(&p, 256, 10), "a mismatched texture key must not serve a smaller bucket");
        r.textures.insert(slot, tex(77));
        assert!(r.thumb_current(&p, 256, 10), "the recorded key's larger texture serves the smaller bucket");
    }

    // Cells at other buckets leave an in-flight thumbnail alone; a higher priority caller takes it over once
    #[test]
    fn a_pending_thumbnail_coalesces_across_buckets() {
        let p = photo();
        let slot = Slot::Thumb(p.id);
        let mut r = Sched::default();
        r.remember_thumb(&p, 256, 77, false, 10);
        assert!(!r.thumb_pending_other_bucket(&p, 512, 10), "nothing in flight yet");
        r.pending.insert(slot, (77, 10));
        assert!(!r.thumb_pending_other_bucket(&p, 256, 10), "its own bucket asks as usual");
        assert!(r.thumb_pending_other_bucket(&p, 512, 10), "equal priority keeps the request in flight");
        assert!(r.thumb_pending_other_bucket(&p, 384, 8));
        assert!(!r.thumb_pending_other_bucket(&p, 512, 11), "a higher priority caller takes it over");
        r.pending.clear();
        assert!(!r.thumb_pending_other_bucket(&p, 512, 10), "the inputs must belong to a request still in flight");
    }

    #[test]
    fn a_landed_preview_retries_its_failed_faces() {
        let mut session = Session::new();
        let mut r = Sched::default();
        let (face, other) = (Slot::Variant(5), Slot::Variant(6));
        r.failed.insert(face, (5, "still downloading".into(), 0, PhotoId(1)));
        r.failed.insert(other, (6, "still downloading".into(), 0, PhotoId(2)));
        session.media.note_arrived(PhotoId(1));
        r.preview_arrived(&mut session);
        assert!(!r.failed.contains_key(&face));
        assert!(r.failed.contains_key(&other), "another photo's face still waits");
    }
}

#[cfg(test)]
mod stage_budget_tests {
    use super::*;

    const MB: usize = 1 << 20;

    // Given a zoomed view showing a window, the app idling does not free the GPU copy of the
    // original the next window is cut from; once the windows are gone the idle trim runs again
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_zoomed_view_is_not_trimmed_while_idle() {
        let mut r = Scheduler::<()>::default();
        let shown = Shown {
            look: None,
            key: 1,
            photo: PhotoId(1),
            size: [1, 1],
            histogram: None,
            ms: 0.0,
            quick: None,
            request_id: 0,
            draft: false,
            view: (),
        };
        r.textures.insert(Slot::Region, shown);
        let long_ago = std::time::Instant::now().checked_sub(IDLE_TRIM * 2).unwrap();
        r.idle = Some((long_ago, false));
        r.trim_when_idle();
        assert_eq!(r.idle, None, "a zoomed view is not idle");
        r.release(Slot::Region);
        r.idle = Some((long_ago, false));
        r.trim_when_idle();
        assert_eq!(r.idle, Some((long_ago, true)), "trimmed once the window is gone");
    }

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
    /// What an Exposure drag at `ns_per_px` teaches.
    fn learned(ns_per_px: f64) -> DraftBudget {
        let mut b = DraftBudget { drag: Some("Exposure".into()), ..Default::default() };
        let size = [1000, 700];
        for _ in 0..4 {
            b.note(size, ns_per_px * 1e-6 * 700_000.0, false);
        }
        b
    }

    #[test]
    fn nothing_is_drafted_when_no_slider_drags() {
        assert_eq!(learned(100.0).scale(None, 3e6, false), 1.0);
    }

    #[test]
    fn a_fast_host_drafts_at_full_size_and_a_slow_one_shrinks_the_draft() {
        // a GPU: a few ms for a 3 MP view, drafted at its own size (natively; the browser stops at 0.6)
        assert_eq!(learned(3.0).scale(Some("Exposure"), 3e6, false), DraftBudget::MAX);
        // a CPU at ~100 ns a pixel: what fits the target (0.2 MP natively, 0.4 MP in the browser)
        let s = learned(100.0).scale(Some("Exposure"), 3e6, false);
        let px = 3e6 * f64::from(s) * f64::from(s);
        let fits = DraftBudget::TARGET_MS / 1e-4;
        assert!((fits * 0.65..fits * 1.35).contains(&px), "scale {s} is {px} px, {fits} fit");
        // a hopeless host stops at the floor
        assert_eq!(learned(10_000.0).scale(Some("Exposure"), 3e6, false), DraftBudget::MIN);
    }

    #[test]
    fn a_draft_of_a_new_size_is_cold_and_teaches_nothing() {
        let mut b = DraftBudget::default();
        b.note([800, 600], 900.0, false);
        b.note([400, 300], 900.0, false);
        assert_eq!(b.scale(Some("Exposure"), 3e6, false), DraftBudget::GUESS, "no two drafts of one size yet");
        b.note([400, 300], 12.0, false);
        assert!(b.scale(None, 3e6, false) == 1.0 && b.cost[0].ms_per_px.is_some_and(|c| (c - 1e-4).abs() < 1e-9));
    }

    #[test]
    fn the_scale_holds_during_a_drag_and_only_goes_down() {
        let mut b = learned(3.0);
        let first = b.scale(Some("Exposure"), 3e6, false);
        assert_eq!(first, DraftBudget::MAX);
        // a little noise in the samples moves nothing, nor does one draft held up by other work
        b.note([1000, 700], 2.6, false);
        b.note([1000, 700], 70.0, false);
        assert_eq!(b.scale(Some("Exposure"), 3e6, false), first);
        // drafts twice as slow as the target keep the size too (a new size would start cold: a long stall)
        for _ in 0..6 {
            b.note([1000, 700], 9.5, false);
        }
        assert_eq!(b.scale(Some("Exposure"), 3e6, false), first);
        // a slider that turns out much slower (it redoes more per tick) shrinks the draft...
        for _ in 0..12 {
            b.note([1000, 700], 70.0, false);
        }
        let smaller = b.scale(Some("Exposure"), 3e6, false);
        assert!(smaller < first * 0.75 && smaller >= DraftBudget::MIN, "{smaller}");
        // ...and a fast tick afterwards doesn't grow it back mid-drag
        for _ in 0..30 {
            b.note([1000, 700], 2.0, false);
        }
        assert_eq!(b.scale(Some("Exposure"), 3e6, false), smaller);
        // the next drag starts afresh from what was learned last
        assert_eq!(b.scale(None, 3e6, false), 1.0);
        assert_eq!(b.scale(Some("Exposure"), 3e6, false), DraftBudget::MAX);
    }

    #[test]
    fn hdr_drafts_are_learned_apart_from_sdr_ones() {
        // SDR drafts on the GPU at full size; HDR ones on the CPU (a LUT profile), 30× slower
        let mut b = learned(3.0);
        assert_eq!(b.scale(Some("Exposure"), 3e6, false), DraftBudget::MAX);
        assert_eq!(b.scale(None, 3e6, false), 1.0);
        // the first HDR drag doesn't start from SDR's speed (a full-size picture of a CPU render: a stall)
        assert_eq!(b.scale(Some("Exposure"), 3e6, true), DraftBudget::GUESS);
        for _ in 0..4 {
            b.note([1000, 700], 90e-6 * 700_000.0, true);
        }
        assert_eq!(b.scale(None, 3e6, true), 1.0);
        let hdr = b.scale(Some("Exposure"), 3e6, true);
        assert!(hdr < 0.5, "{hdr}");
        assert!(b.report().1.is_some_and(|c| (c - 90.0).abs() < 1e-6), "the HDR cost is reported: {:?}", b.report());
        // ...and SDR's is still what it was
        assert_eq!(b.scale(None, 3e6, false), 1.0);
        assert_eq!(b.scale(Some("Exposure"), 3e6, false), DraftBudget::MAX);
    }

    #[test]
    fn a_gpu_hosts_first_drag_drafts_at_full_size_but_not_its_first_hdr_one() {
        let mut b = DraftBudget { gpu: true, ..Default::default() };
        assert_eq!(b.scale(Some("Exposure"), 3e6, false), DraftBudget::MAX);
        assert_eq!(b.scale(None, 3e6, false), 1.0);
        // (an HDR picture may render on the CPU, and reads back three times the bytes)
        assert_eq!(b.scale(Some("Exposure"), 3e6, true), DraftBudget::GUESS);
        // what drafts are learned to cost still decides
        let mut slow = learned(100.0);
        slow.gpu = true;
        assert!(slow.scale(Some("Exposure"), 3e6, false) < 0.5);
    }

    // Each slider learns its own size: a slow one doesn't shrink the next slider's drafts on a GPU (a new size starts
    // cold), and off a GPU a slider not dragged yet starts from what any slider cost
    #[test]
    fn a_slow_slider_shrinks_its_own_drafts_not_the_next_ones() {
        let mut b = DraftBudget { gpu: true, ..Default::default() };
        assert_eq!(b.scale(Some("Noise Reduction"), 3e6, false), DraftBudget::MAX);
        for _ in 0..6 {
            b.note([1000, 700], 150.0, false);
        }
        assert_eq!(b.scale(None, 3e6, false), 1.0);
        let nr = b.scale(Some("Noise Reduction"), 3e6, false);
        assert!(nr < 0.5, "{nr}");
        assert_eq!(b.scale(None, 3e6, false), 1.0);
        assert_eq!(b.scale(Some("Vibrance"), 3e6, false), DraftBudget::MAX, "not slowed by Noise Reduction");
        assert_eq!(b.scale(None, 3e6, false), 1.0);
        assert_eq!(b.scale(Some("Noise Reduction"), 3e6, false), nr, "and Noise Reduction still knows its own");
        // off a GPU (the browser) the cost of any slider is the best guess for a new one
        let mut cpu = learned(100.0);
        assert_eq!(cpu.scale(None, 3e6, false), 1.0);
        let other = cpu.scale(Some("Vibrance"), 3e6, false);
        assert!(other < 0.5 && other != DraftBudget::GUESS, "{other}");
    }

    // A fast host keeps drafting a zoomed view's windows at full size; a slower one shrinks them with the frame's draft
    #[test]
    fn windows_shrink_only_as_far_as_the_frame_draft_is_below_the_fast_hosts() {
        assert_eq!(DraftBudget::window_scale(DraftBudget::MAX), 1.0);
        assert_eq!(DraftBudget::window_scale(1.0), 1.0);
        assert!((DraftBudget::window_scale(0.3) - 0.5).abs() < 1e-6);
        assert!((DraftBudget::window_scale(DraftBudget::MIN) - DraftBudget::MIN / DraftBudget::WINDOW_FULL).abs() < 1e-6);
        assert_eq!(DraftBudget::window_scale(f32::NAN), 1.0);
        assert!(DraftBudget::window_scale(0.0) >= DraftBudget::MIN / DraftBudget::WINDOW_FULL);
    }

    #[test]
    fn hostile_numbers_give_a_scale_in_range() {
        for full in [0.0, -1.0, f64::NAN, f64::INFINITY, 1e300] {
            let s = learned(100.0).scale(Some("Exposure"), full, false);
            assert!((DraftBudget::MIN..=DraftBudget::MAX).contains(&s), "full_px {full}: {s}");
            let mut b = DraftBudget::default();
            b.note([0, 0], f64::NAN, false);
            b.note([0, 0], f64::INFINITY, false);
            assert!(b.scale(Some("Exposure"), full, false) > 0.0);
        }
    }
}

#[cfg(test)]
mod scheduler_tests {
    use super::*;
    use lightcraft_catalog::{Op, Photo, Source};

    /// Keeps the pixels it was given (what the engine host does with them).
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

    // Slideshow slots have names in the host protocol, and the ones that are no longer near go
    #[test]
    fn slideshow_slots_have_names_and_release_all_but_the_ones_kept() {
        let (photo, intro) = (Slot::Slide(Item::Photo(PhotoId(1))), Slot::Slide(Item::INTRO));
        for slot in [photo, intro, Slot::Slide(Item::ENDING), Slot::SlidePreview] {
            assert_eq!(Slot::parse(&slot.name()), Some(slot));
        }
        let shown = || Shown {
            look: None,
            key: 1,
            photo: PhotoId(1),
            size: [1, 1],
            histogram: None,
            ms: 0.0,
            quick: None,
            request_id: 1,
            draft: false,
            view: (),
        };
        let mut r = Scheduler::<()>::with_worker_threads(1);
        r.slides_live = true;
        for slot in [photo, intro, Slot::SlidePreview] {
            r.textures.insert(slot, shown());
        }
        r.release_slides(&[Item::INTRO]);
        assert!(r.textures.contains_key(&intro) && r.textures.contains_key(&Slot::SlidePreview) && !r.textures.contains_key(&photo));
    }

    /// Runs nothing itself: the test hands the jobs' results back.
    #[derive(Default)]
    struct Work {
        jobs: Vec<(Slot, RenderJob)>,
        finished: Vec<(Slot, RenderResult, f64)>,
    }
    struct Manual(std::rc::Rc<std::cell::RefCell<Work>>);
    impl RenderOffload for Manual {
        fn try_start(&mut self, slot: Slot, job: RenderJob) -> Option<RenderJob> {
            self.0.borrow_mut().jobs.push((slot, job));
            None
        }
        fn finished(&mut self) -> Vec<(Slot, RenderResult, f64)> {
            std::mem::take(&mut self.0.borrow_mut().finished)
        }
    }

    fn flat_session() -> Session {
        session(Arc::new(|_, _, _| {
            let mut image = lightcraft_raster::Rgb32f::new(8, 8);
            image.data.fill([0.3, 0.2, 0.1]);
            Ok((image, Default::default()))
        }))
    }

    // A superseded draft that is still shown keeps the geometry of its own pixels, never the newer
    // request's, and a completed request leaves nothing behind
    #[test]
    fn a_superseded_draft_keeps_the_geometry_of_its_own_pixels() {
        let mut s = flat_session();
        let mut r = Scheduler::<Arc<Rgba8>>::default();
        let work = std::rc::Rc::new(std::cell::RefCell::new(Work::default()));
        r.set_offload(Box::new(Manual(work.clone())));
        let old = s.loupe_job(PhotoId(1), 8, 8, true).unwrap().draft();
        let old_look = geometry_look(&old.settings);
        r.request(Slot::Main, old, 100);
        let mut settings = lightcraft_develop::DevelopSettings::default();
        settings.crop.geometry.rect = lightcraft_geom::Rect::new(0.0, 0.0, 0.5, 1.0);
        let new_look = geometry_look(&settings);
        assert_ne!(old_look, new_look);
        s.catalog.apply(Op::SetDevelop { id: PhotoId(1), settings: Arc::new(settings), label: "crop".into(), edited: None }).unwrap();
        let current = s.loupe_job(PhotoId(1), 8, 8, true).unwrap().draft();
        r.request(Slot::Main, current, 100);
        let jobs: Vec<RenderJob> = work.borrow_mut().jobs.drain(..).map(|(_, j)| j).collect();
        let mut jobs = jobs.into_iter();
        let (old, current) = (jobs.next().unwrap().run(), jobs.next().unwrap().run());
        work.borrow_mut().finished.push((Slot::Main, old, 0.0));
        r.poll(&mut s, &mut Keep);
        assert_eq!(r.textures[&Slot::Main].look, Some((old_look, true)), "the superseded pixels keep their own geometry");
        work.borrow_mut().finished.push((Slot::Main, current, 0.0));
        r.poll(&mut s, &mut Keep);
        assert_eq!(r.textures[&Slot::Main].look, Some((new_look, true)));
        assert!(r.request_looks.is_empty(), "completed requests are released");
    }

    // Hidden grading wheels change the job's settings but not the frame's geometry
    #[test]
    fn hidden_grading_keeps_the_completed_textures_geometry_identity() {
        let mut s = flat_session();
        let mut settings = lightcraft_develop::DevelopSettings::default();
        settings.grading.shadows.sat = 40.0;
        settings.crop.geometry.rect = lightcraft_geom::Rect::new(0.1, 0.2, 0.8, 0.7);
        s.catalog.apply(Op::SetDevelop { id: PhotoId(1), settings: Arc::new(settings.clone()), label: "grade".into(), edited: None }).unwrap();
        let expected = geometry_look(&settings);
        let job = s.loupe_job(PhotoId(1), 8, 8, true).unwrap().with_hidden_grading([true, false, false]);
        assert_ne!(job.settings.hash64(), settings.hash64(), "the view really hides a nonzero wheel");
        let mut r = Scheduler::<Arc<Rgba8>>::with_worker_threads(1);
        r.request(Slot::Main, job, 100);
        poll_until(&mut r, &mut s, Polled::changed);
        assert_eq!(r.textures[&Slot::Main].look, Some((expected, true)));
    }

    // Capture readiness: neighbours' preparation does not count, photo and thumbnail work does
    #[test]
    fn capture_readiness_excludes_prefetch_but_keeps_photo_and_thumbnail_work() {
        let mut s = flat_session();
        let mut r = Scheduler::<Arc<Rgba8>> { pool: JobPool::new(0), ..Default::default() };
        r.prefetch(Slot::Prefetch(0), s.thumb_job(PhotoId(1), 384).unwrap(), 1);
        r.prefetch(Slot::Prefetch(1), s.thumb_job(PhotoId(1), 384).unwrap(), 1);
        assert_eq!((r.in_flight(), r.in_flight_without_prefetch()), (2, 0));
        for (index, slot) in
            [Slot::Main, Slot::Thumb(PhotoId(1)), Slot::Variant(7), Slot::Preview, Slot::ThumbQuick(PhotoId(1))].into_iter().enumerate()
        {
            r.request(slot, s.thumb_job(PhotoId(1), 384).unwrap(), 10);
            assert_eq!(r.in_flight_without_prefetch(), index + 1);
            assert_eq!(r.in_flight(), index + 3);
        }
        assert_eq!(r.pool.run_inline(7), 7);
        // Waiting observes a completed job without swallowing the result needed by poll.
        assert!(r.wait_for_worker(Duration::ZERO));
        r.poll(&mut s, &mut Keep);
        assert_eq!((r.in_flight(), r.in_flight_without_prefetch()), (0, 0));
    }

    // A neighbour's prefetch waits for the loupe's own render: the two decodes side by side ended the phone app
    #[test]
    fn prefetch_waits_for_the_loupes_render() {
        let mut s = flat_session();
        let mut r = Scheduler::<Arc<Rgba8>> { pool: JobPool::new(0), ..Default::default() };
        r.request(Slot::Main, s.loupe_job(PhotoId(1), 8, 8, true).unwrap(), 100);
        r.prefetch(Slot::Prefetch(0), s.thumb_job(PhotoId(1), 384).unwrap(), 1);
        assert_eq!(r.pool.queued(), 1, "only the loupe's render is queued");
        assert!(!r.is_pending(Slot::Prefetch(0)));
        assert_eq!(r.pool.run_inline(1), 1);
        r.poll(&mut s, &mut Keep);
        assert!(!r.is_pending(Slot::Main));
        assert!(r.is_pending(Slot::Prefetch(0)), "the prefetch starts once the loupe's render has ended");
        assert_eq!(r.pool.queued(), 1);
        // (cancelled while it waited: it never starts)
        r.request(Slot::Main, s.loupe_job(PhotoId(1), 9, 9, true).unwrap(), 100);
        r.prefetch(Slot::Prefetch(1), s.thumb_job(PhotoId(1), 256).unwrap(), 1);
        r.cancel(Slot::Prefetch(1));
        assert!(r.deferred_prefetch.is_empty());
    }

    // A finished render becomes the slot's picture, with its facts, through the frontend's presenter
    #[test]
    fn a_render_becomes_what_its_slot_shows() {
        let mut s = session(Arc::new(|_, _, _| {
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

    /// Keeps the HDR picture of each slot it was given (what the egui window does with it).
    struct KeepHdr;
    impl Present<Option<Arc<HdrPicture>>> for KeepHdr {
        fn make(&mut self, _: Slot, _: Rgba8) -> Option<Arc<HdrPicture>> {
            None
        }
        fn update(&mut self, _: Slot, _: Rgba8, _: &mut Option<Arc<HdrPicture>>) {}
        fn set_hdr(&mut self, _: Slot, hdr: Option<Arc<HdrPicture>>, view: &mut Option<Arc<HdrPicture>>) {
            *view = hdr;
        }
    }

    // The loupe of an HDR edit gets its HDR picture when the display can show it, and only then; an
    // SDR render that takes its place takes the picture away (LR-VIEW-HDR-DISPLAY)
    #[test]
    fn the_loupe_of_an_hdr_edit_shows_hdr_only_on_an_hdr_display() {
        let mut s = session(Arc::new(|_, _, _| {
            let mut image = lightcraft_raster::Rgb32f::new(8, 8);
            image.data.fill([3.0, 2.0, 1.0]);
            Ok((image, Default::default()))
        }));
        let mut hdr_on = lightcraft_develop::DevelopSettings::default();
        hdr_on.hdr.enabled = true;
        s.catalog.apply(Op::SetDevelop { id: PhotoId(1), settings: Arc::new(hdr_on), label: "HDR".into(), edited: None }).unwrap();
        let mut r = Scheduler::<Option<Arc<HdrPicture>>>::with_worker_threads(1);
        let show = |r: &mut Scheduler<Option<Arc<HdrPicture>>>, s: &mut Session| {
            let job = s.loupe_job(PhotoId(1), 8, 8, true).unwrap();
            r.request(Slot::Main, job, 100);
            let t0 = std::time::Instant::now();
            while r.is_pending(Slot::Main) {
                r.poll(s, &mut KeepHdr);
                assert!(t0.elapsed() < Duration::from_secs(30), "no result");
                std::thread::sleep(Duration::from_millis(2));
            }
            r.poll(s, &mut KeepHdr);
            r.textures[&Slot::Main].view.clone()
        };
        // a display without headroom: the SDR rendition, no HDR picture
        assert!(!r.hdr_display());
        assert!(show(&mut r, &mut s).is_none());
        // a display with: the same edit renders its HDR picture, which has what is above SDR white
        r.set_hdr_display(true);
        let hdr = show(&mut r, &mut s).expect("the HDR picture");
        assert_eq!((hdr.width, hdr.height), (r.textures[&Slot::Main].size[0], r.textures[&Slot::Main].size[1]));
        // (a positive half float is above 1.0 exactly when its bits are above 1.0's, 0x3C00)
        assert!(hdr.rgba16f.chunks(4).any(|p| p[0] > 0x3C00));
        // the window moved to a display without: back to the SDR rendition, the picture gone
        r.set_hdr_display(false);
        assert!(show(&mut r, &mut s).is_none());
        // the small views (a grid thumbnail) never carry one
        r.set_hdr_display(true);
        assert!(!r.job_for(Slot::Thumb(PhotoId(1)), s.thumb_job(PhotoId(1), 64).unwrap()).hdr_view);
        assert!(!r.job_for(Slot::Before, s.loupe_job(PhotoId(1), 8, 8, true).unwrap()).hdr_view);
        assert!(r.job_for(Slot::Region, s.loupe_job(PhotoId(1), 8, 8, true).unwrap()).hdr_view);
    }

    // A synced photo's thumbnail failed for want of its preview: once the preview lands, the next ask
    // renders it, without a poll in between (the grid draws after the sync step, in the same frame)
    #[test]
    fn a_preview_that_lands_lets_the_failed_thumbnail_render_in_the_same_frame() {
        let landed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let l = landed.clone();
        let mut s = session(Arc::new(move |_, _, _| {
            if !l.load(std::sync::atomic::Ordering::SeqCst) {
                return Err("still downloading".into());
            }
            let mut image = lightcraft_raster::Rgb32f::new(8, 8);
            image.data.fill([0.3, 0.2, 0.1]);
            Ok((image, Default::default()))
        }));
        let mut r = Scheduler::<Arc<Rgba8>>::with_worker_threads(1);
        let id = PhotoId(1);
        assert!(r.request_thumb(&mut s, id, 64, 10));
        poll_until(&mut r, &mut s, |p| !p.failed.is_empty());
        assert!(!r.request_thumb(&mut s, id, 64, 10), "the failure is remembered");
        // the sync step puts the file there and says so; the grid draws before the next poll
        landed.store(true, std::sync::atomic::Ordering::SeqCst);
        s.media.note_arrived(id);
        assert!(r.request_thumb(&mut s, id, 64, 10), "asked again at once");
        let p = poll_until(&mut r, &mut s, Polled::changed);
        assert_eq!(p.shown, vec![Slot::Thumb(id)]);
        assert!(r.thumb(id).is_some());
    }

    // A render that panics is that slot's failure: it is reported, the slot is no longer pending,
    // and the worker renders the next one
    #[test]
    fn a_panicking_render_fails_its_slot_and_spares_the_worker() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let c = calls.clone();
        let mut s = session(Arc::new(move |_, _, _| {
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
