//! Sharp zoomed views (issue #323). The loupe renders the whole frame in one texture, which can't
//! be as large as a 1:1 or 8:1 view of a big photo. Past that size it also renders just the window
//! of the frame that is on screen, at the zoom scale, and draws it over the whole-frame render.
//!
//! When a window is needed and which one is planned in [`lightcraft_apphost::region`] (shared with
//! the engine host); this module adds what is this app's: the record of a window asked for,
//! holding the sizes while a pinch runs, and where each side of a Before/After view may be drawn.

use lightcraft_engine::pipeline::PixelWindow;

pub use lightcraft_apphost::region::{
    CPU_DRAFT_SCALE, LoupePlan, MAX_MARGIN, MAX_SPAN, SNAP, ViewSizes, WINDOW_RATIO, margin_for, max_span, same_aspect, window_for,
};

/// A window render the loupe asked for: which photo, the size of the zoomed frame it is a window
/// of, and the window. The texture that comes back is drawn at this place of the frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RegionView {
    pub photo: lightcraft_catalog::PhotoId,
    /// The Before side of a Before/After view (the photo without its edits).
    pub before: bool,
    pub key: u64,
    pub full: (usize, usize),
    pub window: PixelWindow,
    /// Hash of the render state it was rendered with (develop settings plus any window-safe overlay).
    pub settings: u64,
}

impl RegionView {
    /// Whether this window's pixels still belong in a loupe showing `photo` with `settings` in a
    /// frame of `full` pixels (they are drawn until the next window arrives, if so).
    ///
    /// While a slider is dragged (`drafting`) a window made for an earlier value of it still
    /// belongs: drafts follow the drag, and waiting for an exact match would show none.
    pub fn is_current(&self, photo: lightcraft_catalog::PhotoId, full: (usize, usize), settings: u64, drafting: bool) -> bool {
        self.photo == photo && self.full == full && (drafting || self.settings == settings)
    }
}

/// The slider budget (ms): a drag whose full-size drafts take longer gets smaller drafts next time.
/// A draft's time is its render plus the frame that shows it (the texture upload is part of it).
pub const DRAFT_BUDGET_MS: f64 = 16.0;

/// Full-size drafts this many times over budget give up on full size in the middle of the drag:
/// a drag at 200 ms a step is worse than one jump in resolution.
const GIVE_UP_FACTOR: f64 = 2.0;

/// The full-size drafts (after the first) a drag times before it may give up on full size.
const GIVE_UP_AFTER: usize = 3;

/// The most (photo, control) pairs [`DragScale`] keeps a verdict on; past it, the one remembered
/// longest ago is forgotten.
const VERDICTS_KEPT: usize = 256;

/// A host frame time (ms) longer than this is not a texture upload but a window that was hidden,
/// occluded or stalled: it counts as unknown, like a frame time from no host at all.
const FRAME_MS_KNOWN: f64 = 1000.0;

/// A drag counts against its (photo, control) pair only when its full-size drafts took longer than
/// this (ms): a quarter over the budget, so a drag that only just misses it keeps full size.
const VERDICT_MS: f64 = DRAFT_BUDGET_MS * 1.25;

/// How long (s) a pair found slow drafts small before a drag of it tries full size again: the
/// machine may only have been busy. Each retry that is slow too doubles the wait, up to
/// [`RETRY_MAX_S`]. A retry costs at most a few slow drafts before the drag gives up on full size.
const RETRY_AFTER_S: f64 = 30.0;

/// The longest wait (s) before a slow pair tries full size again.
const RETRY_MAX_S: f64 = 240.0;

/// What [`DragScale`] remembers about the drags of one control on one photo.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Verdict {
    /// One drag was over budget at full size: maybe a busy moment, so the next stays full size.
    Suspect,
    /// Two drags in a row were: drafts are small until `until` (the host's clock, s), then a drag
    /// tries full size again; `wait` is how long this verdict holds (s).
    Slow { until: f64, wait: f64 },
}

/// The size of a slider drag's whole-frame draft, chosen when the drag starts and kept until it
/// ends (a change in the middle would make the resolution jump), unless its full-size drafts are
/// far over budget. A drag renders at full size when the GPU draws the view and the same control
/// on this photo isn't known to be too slow at that size; otherwise at [`CPU_DRAFT_SCALE`] (issue
/// #330). Whether the GPU drew a render is not enough on its own: some views (a LUT profile,
/// refined masks, a photo past a GPU limit) fall back to the CPU per render, and a large draft
/// costs a texture upload too, which only the time a draft took to show tells.
///
/// A drag's time is wall-clock time, what the user waits for. So that a moment the machine was busy
/// (another app, a backup) can't make a slider draft small for the rest of the session, a pair is
/// slow only after two drags in a row over [`VERDICT_MS`], and only for a while: after
/// [`RETRY_AFTER_S`] a drag tries full size again, a fast one forgets the verdict and a slow one
/// doubles the wait. A verdict is for its photo only: the same control on another photo (another
/// size, other edits) is timed afresh.
#[derive(Clone, Debug, Default)]
pub struct DragScale {
    /// The drag running now: (photo, control) and its scale.
    current: Option<((lightcraft_catalog::PhotoId, String), f32)>,
    /// How long the current drag's full-size drafts took to show (ms).
    samples: Vec<f64>,
    /// Key of the whole-frame render last timed (each render is timed once).
    seen: Option<u64>,
    /// The render time (ms) of a draft that reached the screen this frame: the time of the frame
    /// that uploads it is only known on the next one.
    pending: Option<f64>,
    /// Pairs whose full-size drafts went over budget, the one remembered longest ago first.
    verdicts: std::collections::VecDeque<((lightcraft_catalog::PhotoId, String), Verdict)>,
}

impl DragScale {
    /// The draft scale for this frame, or `None` without a drag. `drag` is the photo and control
    /// being dragged; `gpu` whether the GPU draws this view; `shown` the key of the whole-frame
    /// render on screen (a render from before the drag is not one of its drafts); `now` the host's
    /// clock (s; egui's `input.time`).
    pub fn scale(&mut self, drag: Option<(lightcraft_catalog::PhotoId, &str)>, gpu: bool, shown: Option<u64>, now: f64) -> Option<f32> {
        let Some((photo, control)) = drag else {
            self.finish(now);
            return None;
        };
        match &self.current {
            Some(((p, c), s)) if *p == photo && c == control => return Some(*s),
            Some(_) => self.finish(now),
            None => {}
        }
        let key = (photo, control.to_string());
        let small = matches!(self.verdict(&key), Some(Verdict::Slow { until, .. }) if now < until);
        let s = if gpu && !small { 1.0 } else { CPU_DRAFT_SCALE };
        self.current = Some((key, s));
        self.seen = shown;
        Some(s)
    }

    /// Once per frame: `key` identifies the whole-frame render on screen and `render_ms` is how
    /// long it took; `last_frame_ms` is how long the previous frame took, painting included (the
    /// host's report; `None` without one, e.g. headless). Times the current drag's full-size
    /// drafts, each as its render plus the frame that uploaded and showed it, and gives up on full
    /// size when they are far over budget.
    pub fn observe(&mut self, key: u64, render_ms: f64, last_frame_ms: Option<f64>) {
        let full = matches!(self.current, Some((_, s)) if s >= 1.0);
        if !full {
            self.pending = None;
            return;
        }
        if let Some(render) = self.pending.take() {
            let frame = last_frame_ms.filter(|ms| ms.is_finite() && (0.0..=FRAME_MS_KNOWN).contains(ms)).unwrap_or(0.0);
            if self.samples.len() < 1024 {
                self.samples.push(render + frame);
            }
            if self.samples.len() > GIVE_UP_AFTER
                && median_after_first(&self.samples).is_some_and(|m| m > DRAFT_BUDGET_MS * GIVE_UP_FACTOR)
                && let Some((_, s)) = self.current.as_mut()
            {
                *s = CPU_DRAFT_SCALE;
                return;
            }
        }
        if self.seen == Some(key) {
            return;
        }
        self.seen = Some(key);
        if render_ms.is_finite() && render_ms >= 0.0 {
            self.pending = Some(render_ms);
        }
    }

    /// Remember drags of `control` on `photo` as too slow for full-size drafts, for good (tests).
    #[cfg(test)]
    pub fn remember_slow(&mut self, photo: lightcraft_catalog::PhotoId, control: &str) {
        self.set_verdict((photo, control.to_string()), Some(Verdict::Slow { until: f64::INFINITY, wait: RETRY_MAX_S }));
    }

    fn verdict(&self, key: &(lightcraft_catalog::PhotoId, String)) -> Option<Verdict> {
        self.verdicts.iter().find(|(k, _)| k == key).map(|(_, v)| *v)
    }

    /// Set or (`None`) forget a pair's verdict. A new pair forgets the one remembered longest ago
    /// when the list is full (not all of them: each forgotten pair costs one more slow drag).
    fn set_verdict(&mut self, key: (lightcraft_catalog::PhotoId, String), verdict: Option<Verdict>) {
        let at = self.verdicts.iter().position(|(k, _)| *k == key);
        match (at, verdict) {
            (Some(i), Some(v)) => {
                if let Some(entry) = self.verdicts.get_mut(i) {
                    entry.1 = v;
                }
            }
            (Some(i), None) => {
                self.verdicts.remove(i);
            }
            (None, Some(v)) => {
                while self.verdicts.len() >= VERDICTS_KEPT {
                    self.verdicts.pop_front();
                }
                self.verdicts.push_back((key, v));
            }
            (None, None) => {}
        }
    }

    /// The drag ended. A drag timed at full size updates its pair's verdict: over [`VERDICT_MS`]
    /// a first time makes it suspect, a second slow for [`RETRY_AFTER_S`], and a slow retry
    /// doubles the wait; within it, the verdict is forgotten. A drag never timed at full size (it
    /// drafted small, or was too short) changes nothing.
    fn finish(&mut self, now: f64) {
        if let Some((key, _)) = self.current.take()
            && let Some(median) = median_after_first(&self.samples)
        {
            let verdict = if median > VERDICT_MS {
                let wait = match self.verdict(&key) {
                    None => None,
                    Some(Verdict::Suspect) => Some(RETRY_AFTER_S),
                    Some(Verdict::Slow { wait, .. }) => Some((wait * 2.0).min(RETRY_MAX_S)),
                };
                Some(wait.map_or(Verdict::Suspect, |wait| Verdict::Slow { until: now + wait, wait }))
            } else {
                None
            };
            self.set_verdict(key, verdict);
        }
        self.samples.clear();
        self.seen = None;
        self.pending = None;
    }
}

/// The median of a drag's draft times (ms), leaving out the first draft (it rebuilds the stage
/// caches the others reuse); `None` before a second draft.
fn median_after_first(samples: &[f64]) -> Option<f64> {
    let mut rest: Vec<f64> = samples.iter().skip(1).copied().collect();
    rest.sort_by(f64::total_cmp);
    rest.get(rest.len() / 2).copied()
}

/// The loupe's render plan with the app's preview size limit ([`lightcraft_apphost::region::plan`]).
pub fn plan(settings: &crate::state::AppSettings, v: ViewSizes) -> LoupePlan {
    lightcraft_apphost::region::plan(settings.preview_limit, v)
}

/// How long the render sizes stay as they were after a pinch or two-finger scroll stops (s).
pub const HOLD_SECS: f64 = 0.25;

/// Holds the loupe's render sizes while a gesture runs (a continuous pinch changes the zoom every
/// frame; re-planning each time would be a stream of renders of every size in between). The view
/// keeps being drawn from the renders it has, magnified by the GPU, and is re-planned once the
/// gesture has been quiet for [`HOLD_SECS`].
#[derive(Clone, Debug, Default)]
pub struct SizeHold {
    photo: Option<lightcraft_catalog::PhotoId>,
    plan: Option<LoupePlan>,
    fit: Option<[usize; 2]>,
    until: f64,
}

impl SizeHold {
    /// The plan to render this frame: `fresh` normally; the plan from before the gesture while
    /// one is running (`gesturing`) and for [`HOLD_SECS`] after. `now` is in seconds.
    pub fn apply(&mut self, photo: lightcraft_catalog::PhotoId, now: f64, gesturing: bool, fresh: LoupePlan) -> LoupePlan {
        if self.photo != Some(photo) {
            *self = SizeHold { photo: Some(photo), plan: Some(fresh), fit: None, until: f64::NEG_INFINITY };
            return fresh;
        }
        if gesturing {
            self.until = now + HOLD_SECS;
        }
        match self.plan {
            Some(held) if gesturing || now < self.until => held,
            _ => {
                self.plan = Some(fresh);
                fresh
            }
        }
    }

    /// The size the whole frame is shrunk to at rest (the drawn size): the one from before the
    /// gesture while sizes are held (call after [`Self::apply`] in the same frame), else `fresh`.
    pub fn display_fit(&mut self, now: f64, gesturing: bool, fresh: [usize; 2]) -> [usize; 2] {
        match self.fit {
            Some(held) if gesturing || now < self.until => held,
            _ => {
                self.fit = Some(fresh);
                fresh
            }
        }
    }

    /// Whether sizes are being held at `now` (the caller asks for a repaint when it ends).
    pub fn holding(&self, now: f64) -> bool {
        now < self.until
    }
}

/// Where the line of a wipe crosses: the middle of the part of the photo that is on screen. At
/// Fit that is the photo's middle; zoomed in, the photo's own middle is mostly off screen, and a
/// line there left the whole view to one side (#1351).
pub fn wipe_line(img: egui::Rect, canvas: egui::Rect) -> egui::Pos2 {
    let shown = img.intersect(canvas);
    if shown.is_positive() { shown.center() } else { img.center() }
}

/// Where a window of one side of a Before/After view may be drawn: its own pane in side by side
/// and stacked views (a window carries margin beyond what its pane shows, which would otherwise
/// spill into the neighbour), its side of the line in a wipe, else the canvas.
pub fn window_clip(mode: crate::state::BeforeAfter, before_side: bool, pane: egui::Rect, canvas: egui::Rect, img: egui::Rect) -> egui::Rect {
    use crate::state::BeforeAfter;
    let (min, max) = (canvas.min, canvas.max);
    let line = wipe_line(img, canvas);
    let clip = match (mode, before_side) {
        (BeforeAfter::Split, true) => egui::Rect::from_min_max(min, egui::pos2(line.x, max.y)),
        (BeforeAfter::SplitTopBottom, true) => egui::Rect::from_min_max(min, egui::pos2(max.x, line.y)),
        (BeforeAfter::Split, false) => egui::Rect::from_min_max(egui::pos2(line.x, min.y), max),
        (BeforeAfter::SplitTopBottom, false) => egui::Rect::from_min_max(egui::pos2(min.x, line.y), max),
        (BeforeAfter::SideBySide | BeforeAfter::TopBottom, _) => pane,
        _ => canvas,
    };
    clip.intersect(canvas)
}

/// Whether a window waits for the one already being rendered. All windows are cut from the same
/// decoded original; asked for while it isn't held yet (the Before and After together, in either
/// order, or a new window while the first is still decoding), each worker decodes its own copy
/// (two or three full-size decodes and uploads, all but one thrown away). The waiting window
/// follows once the original is in.
pub fn defer_window(window_pending: bool, original_held: bool) -> bool {
    window_pending && !original_held
}

#[cfg(test)]
mod tests;
