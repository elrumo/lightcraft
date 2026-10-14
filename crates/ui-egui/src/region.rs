//! Sharp zoomed views (issue #323). The loupe renders the whole frame in one texture, which can't
//! be as large as a 1:1 or 8:1 view of a big photo. Past that size it also renders just the window
//! of the frame that is on screen, at the zoom scale, and draws it over the whole-frame render.
//!
//! When a window is needed and which one is planned in [`lightcraft_apphost::region`] (shared with
//! the SwiftUI app's host); this module adds what is this app's: the record of a window asked for,
//! holding the sizes while a pinch runs, and where each side of a Before/After view may be drawn.

use lightcraft_engine::pipeline::PixelWindow;

pub use lightcraft_apphost::region::{LoupePlan, MAX_MARGIN, MAX_SPAN, SNAP, ViewSizes, WINDOW_RATIO, margin_for, max_span, same_aspect, window_for};

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
    until: f64,
}

impl SizeHold {
    /// The plan to render this frame: `fresh` normally; the plan from before the gesture while
    /// one is running (`gesturing`) and for [`HOLD_SECS`] after. `now` is in seconds.
    pub fn apply(&mut self, photo: lightcraft_catalog::PhotoId, now: f64, gesturing: bool, fresh: LoupePlan) -> LoupePlan {
        if self.photo != Some(photo) {
            *self = SizeHold { photo: Some(photo), plan: Some(fresh), until: f64::NEG_INFINITY };
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

    /// Whether sizes are being held at `now` (the caller asks for a repaint when it ends).
    pub fn holding(&self, now: f64) -> bool {
        now < self.until
    }
}

/// Where a window of one side of a Before/After view may be drawn: its own pane in side by side
/// and stacked views (a window carries margin beyond what its pane shows, which would otherwise
/// spill into the neighbour), its side of the line in a wipe, else the canvas.
pub fn window_clip(mode: crate::state::BeforeAfter, before_side: bool, pane: egui::Rect, canvas: egui::Rect, img: egui::Rect) -> egui::Rect {
    use crate::state::BeforeAfter;
    let (min, max) = (canvas.min, canvas.max);
    let clip = match (mode, before_side) {
        (BeforeAfter::Split, true) => egui::Rect::from_min_max(min, egui::pos2(img.center().x, max.y)),
        (BeforeAfter::SplitTopBottom, true) => egui::Rect::from_min_max(min, egui::pos2(max.x, img.center().y)),
        (BeforeAfter::Split, false) => egui::Rect::from_min_max(egui::pos2(img.center().x, min.y), max),
        (BeforeAfter::SplitTopBottom, false) => egui::Rect::from_min_max(egui::pos2(min.x, img.center().y), max),
        (BeforeAfter::SideBySide | BeforeAfter::TopBottom, _) => pane,
        _ => canvas,
    };
    clip.intersect(canvas)
}

/// Whether the Before window waits for the After's. Both are cut from the same decoded original;
/// asked for together while it isn't held yet, each worker decodes its own copy (two full-size
/// decodes, two uploads, one thrown away). The Before follows once the original is in.
pub fn defer_before_window(before_side: bool, after_pending: bool, original_held: bool) -> bool {
    before_side && after_pending && !original_held
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppSettings;

    const BIG: usize = 16384;

    fn sizes(drawn: f32, canvas: f32, native: usize) -> ViewSizes {
        ViewSizes { drawn_long: drawn, canvas_long: canvas, native_long: native, texture_side: BIG, draft_scale: 1.0, windows: true }
    }

    fn auto() -> AppSettings {
        AppSettings::default()
    }

    // Given a fit view on a Retina Mac (about 2800 px), the preview is enough: no original decoded
    #[test]
    fn a_retina_fit_view_needs_no_window() {
        let p = plan(&auto(), sizes(2800.0, 2800.0, 6000));
        assert_eq!(p.main_edge, 2560, "the preview source level");
        assert_eq!(p.window_edge, None);
    }

    // Given a 4K display, the fit view is 1.5x the preview: the window makes it sharp
    #[test]
    fn a_large_fit_view_gets_a_window() {
        let p = plan(&auto(), sizes(3840.0, 3840.0, 6000));
        assert_eq!((p.main_edge, p.window_edge), (2560, Some(3840)));
    }

    // Given 1:1 on a 24 MP photo in a 2800 px canvas, the whole frame stays canvas-sized and the
    // window is cut from the photo's own pixels
    #[test]
    fn one_to_one_renders_the_canvas_and_a_native_window() {
        let p = plan(&auto(), sizes(6000.0, 2800.0, 6000));
        assert_eq!((p.main_edge, p.window_edge), (2560, Some(6000)));
    }

    // Given 800 %, the window is still at 100 % (the GPU magnifies it): the cost does not grow
    #[test]
    fn beyond_one_to_one_the_window_stays_at_native_resolution() {
        let p = plan(&auto(), sizes(48000.0, 2800.0, 6000));
        assert_eq!((p.main_edge, p.window_edge), (2560, Some(6000)));
        let q = plan(&auto(), sizes(480.0 * 100.0, 2800.0, 6000));
        assert_eq!(q.window_edge, Some(6000));
    }

    // Given a small photo zoomed in, the whole-frame render already holds all its pixels
    #[test]
    fn a_small_photo_needs_no_window() {
        let p = plan(&auto(), sizes(8000.0, 1400.0, 1000));
        assert_eq!((p.main_edge, p.window_edge), (1000, None));
    }

    // Given a slider drag at 400 % (drafts at 0.6 scale), the window plan does not change: it is
    // the whole-frame draft that gets cheaper
    #[test]
    fn a_drag_shrinks_the_whole_frame_draft_not_the_window() {
        let still = plan(&auto(), sizes(24000.0, 2800.0, 6000));
        let drag = plan(&auto(), ViewSizes { draft_scale: 0.6, ..sizes(24000.0, 2800.0, 6000) });
        assert!(drag.main_edge < still.main_edge, "{drag:?} vs {still:?}");
        assert_eq!(drag.window_edge, still.window_edge);
    }

    // Given no window (a whole-frame diagnostic or soft proofing), the view at rest renders the
    // whole frame at the size it is drawn, but a slider drag drafts at the canvas size like the
    // windowed path
    #[test]
    fn without_a_window_a_drag_drafts_at_the_canvas_size() {
        let no_window = |draft_scale: f32| ViewSizes { windows: false, draft_scale, ..sizes(24000.0, 2800.0, 6000) };
        let still = plan(&auto(), no_window(1.0));
        let drag = plan(&auto(), no_window(0.6));
        assert_eq!(still.window_edge, None);
        assert!(still.main_edge > 2800, "at rest the zoomed frame: {still:?}");
        assert!(drag.main_edge <= 2800, "a drag drafts at most the canvas: {drag:?}");
    }

    // Given the user capped the preview size, the whole-frame render obeys it, and a fit view
    // stays on it (no original decoded); a zoomed view still gets its sharp window
    #[test]
    fn a_preview_limit_caps_the_whole_frame_render_and_spares_the_fit_view() {
        let s = AppSettings { preview_limit: 1600, ..Default::default() };
        let fit = plan(&s, sizes(2800.0, 2800.0, 6000));
        assert_eq!((fit.main_edge, fit.window_edge), (1600, None));
        let zoomed = plan(&s, sizes(6000.0, 2800.0, 6000));
        assert_eq!((zoomed.main_edge, zoomed.window_edge), (1600, Some(6000)));
    }

    // Given a GPU with 2048 px textures, the whole-frame render obeys it, the window frame need not
    // (a window is cut to the texture side separately)
    #[test]
    fn the_texture_side_caps_the_whole_frame_render_only() {
        let p = plan(&auto(), ViewSizes { texture_side: 2048, ..sizes(6000.0, 2800.0, 6000) });
        assert_eq!((p.main_edge, p.window_edge), (2048, Some(6000)));
    }

    // Given a picture that is not the frame (an embedded JPEG cropped differently), no window
    #[test]
    fn a_window_needs_a_picture_with_the_frames_aspect() {
        assert!(same_aspect(1.5, 1.5) && same_aspect(1.51, 1.5));
        assert!(!same_aspect(1.5, 1.0) && !same_aspect(f32::NAN, 1.0) && !same_aspect(1.0, 0.0));
    }

    // Given a drag at fit on a 4K or 5K display (where the fit view has a window), the window stays:
    // it is only the whole-frame draft that gets smaller
    #[test]
    fn a_drag_at_fit_on_a_large_display_keeps_the_window() {
        for drawn in [3300.0, 3840.0, 4096.0, 5120.0] {
            let still = plan(&auto(), sizes(drawn, drawn, 6000));
            let drag = plan(&auto(), ViewSizes { draft_scale: 0.6, ..sizes(drawn, drawn, 6000) });
            assert!(still.window_edge.is_some(), "{drawn}: {still:?}");
            assert_eq!(drag.window_edge, still.window_edge, "{drawn}");
        }
    }

    // Given a mode whose picture can't be a window (soft proofing, a whole-frame diagnostic, or a
    // spot read too far for one render), the whole-frame render is as big as the view
    #[test]
    fn without_windows_the_whole_frame_is_rendered_at_the_drawn_size() {
        let p = plan(&auto(), ViewSizes { windows: false, ..sizes(6000.0, 2800.0, 6000) });
        assert_eq!((p.main_edge, p.window_edge), (6000, None));
        let fit = plan(&auto(), ViewSizes { windows: false, ..sizes(2800.0, 2800.0, 6000) });
        assert_eq!((fit.main_edge, fit.window_edge), (2816, None));
        let deep = plan(&auto(), ViewSizes { windows: false, ..sizes(48000.0, 2800.0, 6000) });
        assert_eq!(deep.main_edge, 6000, "never above the photo's own pixels");
        let small = plan(&auto(), ViewSizes { windows: false, texture_side: 2048, ..sizes(6000.0, 2800.0, 6000) });
        assert_eq!(small.main_edge, 2048);
    }

    // Given a user who chose a preview size above the preview source level (3840 or 5120 on a big
    // display), the whole-frame render is that big
    #[test]
    fn a_large_preview_limit_is_honoured() {
        for limit in [3840u32, 5120] {
            let s = AppSettings { preview_limit: limit, ..Default::default() };
            let p = plan(&s, sizes(5120.0, 5120.0, 6000));
            assert_eq!(p.main_edge, limit as usize, "{limit}");
            assert_eq!(p.window_edge, None, "a fit view at the limit needs no window");
        }
        let s = AppSettings { preview_limit: 2560, ..Default::default() };
        assert_eq!(plan(&s, sizes(5120.0, 5120.0, 6000)).main_edge, 2560);
    }

    // Given a medium format photo wider than 8192 px, 1:1 is its own pixels
    #[test]
    fn a_window_frame_can_be_as_big_as_the_photo() {
        let p = plan(&auto(), sizes(40000.0, 2800.0, 11648));
        assert_eq!(p.window_edge, Some(11648));
    }

    // Given a drag at fit on a Retina Mac, the window does not switch on (it would flicker with the drag)
    #[test]
    fn a_drag_at_fit_does_not_switch_the_window_on() {
        let p = plan(&auto(), ViewSizes { draft_scale: 0.6, ..sizes(2800.0, 2800.0, 6000) });
        assert_eq!(p.window_edge, None);
    }

    // Given a pinch from fit to 400 %, the sizes of the first frame hold until the gesture is quiet
    #[test]
    fn a_pinch_holds_the_render_sizes() {
        use lightcraft_catalog::PhotoId;
        let (fit, zoomed) = (plan(&auto(), sizes(1400.0, 1400.0, 6000)), plan(&auto(), sizes(24000.0, 1400.0, 6000)));
        assert_ne!(fit, zoomed);
        let mut hold = SizeHold::default();
        let p = PhotoId(1);
        assert_eq!(hold.apply(p, 0.0, false, fit), fit);
        // the pinch runs for a second: every frame would plan something else
        for k in 1..60 {
            assert_eq!(hold.apply(p, k as f64 / 60.0, true, zoomed), fit, "frame {k}");
        }
        // it stopped at t = 59/60: still held for a moment (no flicker between two events)…
        assert!(hold.holding(1.1));
        assert_eq!(hold.apply(p, 1.1, false, zoomed), fit);
        // …then the view is planned for where it ended
        assert!(!hold.holding(59.0 / 60.0 + HOLD_SECS + 0.01));
        assert_eq!(hold.apply(p, 1.3, false, zoomed), zoomed);
        assert_eq!(hold.apply(p, 1.4, false, zoomed), zoomed);
    }

    // Given another photo, the held sizes are forgotten
    #[test]
    fn another_photo_is_planned_afresh() {
        use lightcraft_catalog::PhotoId;
        let (fit, zoomed) = (plan(&auto(), sizes(1400.0, 1400.0, 6000)), plan(&auto(), sizes(24000.0, 1400.0, 6000)));
        let mut hold = SizeHold::default();
        hold.apply(PhotoId(1), 0.0, false, fit);
        assert_eq!(hold.apply(PhotoId(2), 0.1, true, zoomed), zoomed);
    }

    // Given side by side, a window is drawn in its pane only; given a wipe, on its side of the line
    #[test]
    fn a_window_is_clipped_to_its_pane_or_its_side_of_the_wipe() {
        use crate::state::BeforeAfter::*;
        let canvas = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1000.0, 600.0));
        let (left, right) = (
            egui::Rect::from_min_max(egui::pos2(24.0, 24.0), egui::pos2(494.0, 576.0)),
            egui::Rect::from_min_max(egui::pos2(506.0, 24.0), egui::pos2(976.0, 576.0)),
        );
        assert_eq!(window_clip(SideBySide, true, left, canvas, left), left);
        assert_eq!(window_clip(SideBySide, false, right, canvas, right), right);
        let img = egui::Rect::from_min_max(egui::pos2(100.0, 50.0), egui::pos2(900.0, 550.0));
        let before = window_clip(Split, true, canvas, canvas, img);
        let after = window_clip(Split, false, canvas, canvas, img);
        assert_eq!((before.max.x, after.min.x), (500.0, 500.0), "the line is the image's middle");
        assert_eq!(window_clip(SplitTopBottom, true, canvas, canvas, img).max.y, 300.0);
        assert_eq!(window_clip(SplitTopBottom, false, canvas, canvas, img).min.y, 300.0);
        assert_eq!(window_clip(Off, false, canvas, canvas, img), canvas);
        assert_eq!(window_clip(Original, true, canvas, canvas, img), canvas);
    }

    // Given Before and After windows asked for at once at first open, only the After's goes: the
    // Before's waits for the decoded original instead of decoding another copy
    #[test]
    fn the_before_window_waits_for_the_original_the_after_is_decoding() {
        assert!(defer_before_window(true, true, false));
        assert!(!defer_before_window(true, true, true), "the original is in");
        assert!(!defer_before_window(true, false, false), "nothing is decoding it");
        assert!(!defer_before_window(false, true, false), "the After never waits");
    }

    // Hostile numbers give a plan, never a panic
    #[test]
    fn hostile_sizes_give_a_usable_plan() {
        for v in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -3.0, 0.0] {
            let p = plan(&auto(), ViewSizes { drawn_long: v, canvas_long: v, draft_scale: v, ..sizes(1.0, 1.0, 6000) });
            assert!((8..=2560).contains(&p.main_edge), "{v}: {p:?}");
        }
        let p = plan(&auto(), sizes(1000.0, 1000.0, 0));
        assert!(p.main_edge >= 8);
    }

    // Given a visible area in the middle of a big frame, the window covers it with a margin, on the grid
    #[test]
    fn the_window_covers_the_visible_area_with_a_margin_on_the_grid() {
        let w = window_for(48_000, 32_000, (20_000.0, 10_000.0, 21_400.0, 10_900.0), MAX_SPAN).unwrap();
        let m = margin_for(48_000);
        assert!(w.x + m <= 20_000 && w.x + w.w >= 21_400 + m, "{w:?}");
        assert!(w.y + m <= 10_000 && w.y + w.h >= 10_900 + m, "{w:?}");
        assert_eq!((w.x % SNAP, w.y % SNAP, w.w % SNAP, w.h % SNAP), (0, 0, 0, 0));
    }

    // Given a small pan, the same window is kept (no re-render); a big pan changes it
    #[test]
    fn small_pans_keep_the_window() {
        let a = window_for(48_000, 32_000, (20_000.0, 10_000.0, 21_400.0, 10_900.0), MAX_SPAN).unwrap();
        let b = window_for(48_000, 32_000, (20_040.0, 10_030.0, 21_440.0, 10_930.0), MAX_SPAN).unwrap();
        assert_eq!(a, b);
        let c = window_for(48_000, 32_000, (26_000.0, 10_000.0, 27_400.0, 10_900.0), MAX_SPAN).unwrap();
        assert_ne!(a, c);
    }

    // Given a frame smaller than the margin, or a view at its edge, the window is clamped into it
    #[test]
    fn the_window_stays_inside_the_frame() {
        let w = window_for(3000, 2000, (-500.0, -500.0, 900.0, 700.0), MAX_SPAN).unwrap();
        assert_eq!((w.x, w.y), (0, 0));
        let w = window_for(3000, 2000, (2500.0, 1500.0, 9000.0, 9000.0), MAX_SPAN).unwrap();
        assert!(w.x + w.w <= 3000 && w.y + w.h <= 2000, "{w:?}");
        assert_eq!((w.x + w.w, w.y + w.h), (3000, 2000));
    }

    // Given a deeper zoom, the margin grows with the wide stages' kernels (3σ of the base, clarity
    // and dehaze planes are 0.045 of the frame), up to a cap that bounds the window
    #[test]
    fn the_margin_grows_with_the_zoom_up_to_a_cap() {
        assert_eq!(margin_for(1000), SNAP, "never less than one grid step");
        assert!(margin_for(8000) > margin_for(3000));
        assert_eq!(margin_for(48_000), MAX_MARGIN);
        assert_eq!(margin_for(0), SNAP);
        assert!(margin_for(usize::MAX) <= MAX_MARGIN);
        for l in [100, 3000, 8000, 48_000, 400_000] {
            assert_eq!(margin_for(l) % SNAP, 0);
        }
    }

    // Given a tile rendered for other settings, another photo or another zoom, it is not drawn
    #[test]
    fn a_tile_is_drawn_only_while_it_shows_what_the_loupe_shows() {
        use lightcraft_catalog::PhotoId;
        let v = RegionView {
            photo: PhotoId(1),
            before: false,
            key: 7,
            full: (6000, 4000),
            window: PixelWindow { x: 0, y: 0, w: 256, h: 256 },
            settings: 11,
        };
        assert!(v.is_current(PhotoId(1), (6000, 4000), 11, false));
        assert!(!v.is_current(PhotoId(2), (6000, 4000), 11, false), "another photo");
        assert!(!v.is_current(PhotoId(1), (6000, 4001), 11, false), "another zoom");
        assert!(!v.is_current(PhotoId(1), (6000, 4000), 12, false), "another look (an edit, an undo)");
        assert!(v.is_current(PhotoId(1), (6000, 4000), 12, true), "a drag's drafts follow the value");
        assert!(!v.is_current(PhotoId(2), (6000, 4000), 12, true), "…of this photo only");
    }

    // Given a GPU that allows 2048 px textures, no window is wider than that
    #[test]
    fn a_window_fits_the_texture_side() {
        for tex in [512, 2048, 4096] {
            let span = max_span(tex);
            assert!(span <= tex.max(SNAP) && span.is_multiple_of(SNAP), "{tex}: {span}");
            let w = window_for(48_000, 32_000, (20_000.0, 10_000.0, 21_400.0, 10_900.0), span).unwrap();
            assert!(w.w <= span && w.h <= span, "{tex}: {w:?}");
        }
        assert_eq!(max_span(100_000), MAX_SPAN);
        assert_eq!(max_span(0), SNAP);
    }

    // Given nothing visible, or hostile numbers, there is no window (never a panic)
    #[test]
    fn nothing_visible_or_hostile_input_is_no_window() {
        assert_eq!(window_for(1000, 1000, (2000.0, 0.0, 3000.0, 100.0), MAX_SPAN), None);
        assert_eq!(window_for(1000, 1000, (10.0, 10.0, 10.0, 90.0), MAX_SPAN), None);
        assert_eq!(window_for(0, 1000, (0.0, 0.0, 10.0, 10.0), MAX_SPAN), None);
        for v in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(window_for(1000, 1000, (v, 0.0, 10.0, 10.0), MAX_SPAN), None);
        }
        let w = window_for(usize::MAX / 2, usize::MAX / 2, (0.0, 0.0, 1e30, 1e30), MAX_SPAN).unwrap();
        assert!(w.w <= MAX_SPAN && w.h <= MAX_SPAN, "{w:?}");
    }

    // Given a view of an enormous area, the window is bounded
    #[test]
    fn a_window_is_bounded() {
        let w = window_for(100_000, 100_000, (0.0, 0.0, 90_000.0, 90_000.0), MAX_SPAN).unwrap();
        assert!(w.w <= MAX_SPAN && w.h <= MAX_SPAN);
    }
}
