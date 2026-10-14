//! Sharp zoomed views (issue #323). The loupe renders the whole frame in one texture, which can't
//! be as large as a 1:1 or 8:1 view of a big photo. Past that size it also renders just the window
//! of the frame that is on screen, at the zoom scale, and draws it over the whole-frame render.
//!
//! This module is the planning every frontend shares: how large the whole-frame render is, when a
//! window is needed and which one. Windows are snapped to a grid and carry a margin, so panning by
//! a few pixels re-uses the render and the spatial stages (clarity, dehaze…) have the context they
//! read around the visible edge. (The egui app keeps its gesture hold and pane clipping in
//! `ui-egui`'s `region`; the engine host plans `render.view {zoom}` with this.)

use lightcraft_engine::pipeline::PixelWindow;

/// Windows start and end on multiples of this many pixels of the zoomed frame.
pub const SNAP: usize = 256;
/// The most context rendered beyond the visible pixels (a multiple of [`SNAP`]).
pub const MAX_MARGIN: usize = 768;
/// The most a window may span on either axis: a bound on its texture whatever the window size.
pub const MAX_SPAN: usize = if cfg!(target_arch = "wasm32") { 3072 } else { 6144 };

/// The most the loupe ever renders in one go (memory and GPU texture size), whatever the setting.
pub const LOUPE_EDGE_CEILING: u32 = 8192;
/// The most a zoomed frame (the one a window is cut from) may be along its long edge: far more
/// than the photos there are (a window holds only what is on screen, so the frame costs nothing).
pub const WINDOW_FRAME_CEILING: usize = 65536;
/// The smallest GPU texture side the loupe plans for, whatever the host reports.
pub const MIN_TEXTURE_SIDE: usize = 512;
/// Loupe render sizes move in steps of this many pixels.
pub const LOUPE_EDGE_STEP: usize = 64;

/// Long edge (pixels) to render the loupe at when it is drawn `wanted_px` wide on screen: that
/// size (rounded up to [`LOUPE_EDGE_STEP`], so resizing a window doesn't re-render at every pixel),
/// never above the photo's own `native_long_edge`, the user's `preview_limit` (0: none), the
/// ceiling or `texture_side`, the largest texture the GPU allows (egui_glow, i.e. the browser
/// build, panics on a bigger one, and many WebGL devices allow only 2048 or 4096).
pub fn loupe_edge(preview_limit: u32, wanted_px: f32, native_long_edge: usize, texture_side: usize) -> usize {
    let limit = if preview_limit == 0 { LOUPE_EDGE_CEILING } else { preview_limit.clamp(512, LOUPE_EDGE_CEILING) } as usize;
    let ceiling = limit.min(texture_side.max(MIN_TEXTURE_SIDE));
    let wanted = if wanted_px.is_nan() { 8.0 } else { wanted_px.clamp(8.0, ceiling as f32) } as usize;
    let stepped = wanted.div_ceil(LOUPE_EDGE_STEP) * LOUPE_EDGE_STEP;
    stepped.min(native_long_edge.max(8)).min(ceiling)
}

/// Long edge of the zoomed frame a window render is cut from, for a photo drawn `drawn_long` px
/// along its long edge: as drawn, but never above the photo's own pixels (beyond 100 % the GPU
/// magnifies the window) or the ceiling. The preview size limit is not applied: it is about the
/// whole-frame render (see [`plan`]).
pub fn window_frame_edge(drawn_long: f32, native_long_edge: usize) -> usize {
    let wanted = if drawn_long.is_nan() { 8.0 } else { drawn_long.clamp(8.0, WINDOW_FRAME_CEILING as f32) } as usize;
    wanted.min(native_long_edge.max(8)).min(WINDOW_FRAME_CEILING)
}

/// Context to render around the visible pixels of a frame `full_long` pixels along: enough for the
/// wide stages (base, clarity and dehaze planes have sigmas up to 0.02 of it) up to a cap.
pub fn margin_for(full_long: usize) -> usize {
    let want = (full_long as f64 * 0.045).min(MAX_MARGIN as f64) as usize;
    want.div_ceil(SNAP).max(1) * SNAP
}

/// The widest a window may be on a host whose GPU textures are at most `texture_side` px: a
/// window is one texture, and a texture over the limit makes some backends (egui_glow, i.e. the
/// browser build) panic. A multiple of [`SNAP`], at least one grid step, at most [`MAX_SPAN`].
pub fn max_span(texture_side: usize) -> usize {
    (texture_side.min(MAX_SPAN) / SNAP * SNAP).max(SNAP)
}

/// A window render is worth it only when the whole-frame render is this much smaller than the
/// zoomed frame: a little softness (a Retina fit view of 2800 px from the 2560 px preview) is not
/// worth decoding the original for.
pub const WINDOW_RATIO: f32 = 1.25;

/// How big the loupe's view of the photo is, and what the host allows.
#[derive(Clone, Copy, Debug)]
pub struct ViewSizes {
    /// Long edge of the photo as drawn (physical px): the zoomed frame.
    pub drawn_long: f32,
    /// Long edge of the canvas it is drawn in (physical px).
    pub canvas_long: f32,
    /// The photo's own long edge as shown (after crop and rotation).
    pub native_long: usize,
    /// The largest texture the GPU takes.
    pub texture_side: usize,
    /// 1.0, or less while a slider is dragged (a draft of the whole-frame render).
    pub draft_scale: f32,
    /// Whether a window can be drawn over the whole-frame render (not with a diagnostic overlay
    /// that needs whole-frame context, soft proofing, or a read too far for one window). Mask
    /// overlays are window-safe.
    pub windows: bool,
}

/// What the loupe renders for a view: the whole frame at about canvas size, and, once the view is
/// zoomed past what that holds, the window on screen at no more than 100 %.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoupePlan {
    /// Long edge of the whole-frame render ([`Slot::Main`](crate::render::Slot::Main)): never more
    /// than the canvas shows, the user's preview limit, the texture side or the preview source level.
    pub main_edge: usize,
    /// Long edge of the zoomed frame the window is cut from (never above the photo's own pixels:
    /// beyond 100 % the GPU magnifies the window), or `None`: the whole-frame render is enough.
    pub window_edge: Option<usize>,
}

/// The loupe's render plan, with the user's `preview_limit` (0: automatic). The cost of a render
/// follows the canvas, not the zoom: the whole-frame render is canvas-sized, and the window holds
/// only the pixels on screen at no more than 100 %.
pub fn plan(preview_limit: u32, v: ViewSizes) -> LoupePlan {
    let canvas_long = if v.canvas_long.is_finite() { v.canvas_long.max(8.0) } else { 8.0 };
    let drawn_long = if v.drawn_long.is_finite() { v.drawn_long.max(8.0) } else { canvas_long };
    let scale = if v.draft_scale.is_finite() { v.draft_scale.clamp(0.1, 1.0) } else { 1.0 };
    if !v.windows {
        // the whole frame at the size it is drawn (soft proof or a whole-frame diagnostic has no
        // window); while a slider drags, a canvas-sized draft as with windows: a drag at 1:1 must
        // not render the photo's every pixel each frame
        let long = if scale < 1.0 { drawn_long.min(canvas_long) } else { drawn_long };
        let main_edge = loupe_edge(preview_limit, long * scale, v.native_long, v.texture_side);
        return LoupePlan { main_edge, window_edge: None };
    }
    // a user who chose a size above the preview source level (3840, 5120) means it; otherwise the
    // original is not decoded for the whole-frame render
    let source_cap = if preview_limit == 0 { lightcraft_engine::SourceLevel::Preview.max_edge() } else { preview_limit as usize };
    let edge_at = |scale: f32| loupe_edge(preview_limit, drawn_long.min(canvas_long) * scale, v.native_long, v.texture_side).min(source_cap);
    let main_edge = edge_at(scale);
    let frame_edge = window_frame_edge(drawn_long, v.native_long);
    // a user who capped the preview size has chosen speed over sharpness at fit
    let fit = drawn_long <= canvas_long * 1.05;
    // (against the undrafted whole-frame render: a drag must not switch the window off and on)
    let window_edge = (frame_edge as f32 > edge_at(1.0) as f32 * WINDOW_RATIO && !(fit && preview_limit != 0)).then_some(frame_edge);
    LoupePlan { main_edge, window_edge }
}

/// Whether a window render can carry `overlay` (one that reads only around each pixel: a mask's,
/// or none); the others need the whole frame, so the loupe renders that at the drawn size instead.
pub fn overlay_supports_window(overlay: lightcraft_engine::pipeline::Overlay) -> bool {
    matches!(overlay, lightcraft_engine::pipeline::Overlay::None | lightcraft_engine::pipeline::Overlay::Mask { .. })
}

/// Whether the picture on screen has the frame's aspect (to the one pixel a texture's whole-pixel
/// size is off by): a window is a part of the frame, so it can only be placed over a picture that
/// is the frame (an unsupported raw's embedded JPEG may be cropped differently).
pub fn same_aspect(shown: f32, frame: f32) -> bool {
    shown.is_finite() && frame.is_finite() && frame > 0.0 && (shown / frame - 1.0).abs() <= 0.02
}

/// The window of a `full_w × full_h` frame to render so that `visible` — `(x0, y0, x1, y1)` in the
/// frame's pixels, as much of the frame as is on screen — is covered: expanded by a margin,
/// snapped outwards to the [`SNAP`] grid (margin: see [`margin_for`]) and clamped into the frame,
/// at most `max_span` ([`max_span`]) wide or high. `None`: nothing is visible.
pub fn window_for(full_w: usize, full_h: usize, visible: (f32, f32, f32, f32), max_span: usize) -> Option<PixelWindow> {
    if full_w == 0 || full_h == 0 || [visible.0, visible.1, visible.2, visible.3].iter().any(|v| !v.is_finite()) {
        return None;
    }
    let margin = margin_for(full_w.max(full_h));
    let axis = |lo: f32, hi: f32, full: usize| -> Option<(usize, usize)> {
        let (lo, hi) = (lo.max(0.0), hi.min(full as f32));
        if hi <= lo {
            return None;
        }
        let a = (lo as usize).saturating_sub(margin) / SNAP * SNAP;
        let b = ((hi.ceil() as usize).saturating_add(margin)).div_ceil(SNAP).saturating_mul(SNAP).min(full);
        // a window larger than MAX_SPAN keeps the visible part (centred on it), snapped
        if b - a > max_span {
            let mid = ((lo + hi) / 2.0) as usize;
            let a = mid.saturating_sub(max_span / 2) / SNAP * SNAP;
            return Some((a, (a + max_span).min(full)));
        }
        Some((a, b))
    };
    let (x0, x1) = axis(visible.0, visible.2, full_w)?;
    let (y0, y1) = axis(visible.1, visible.3, full_h)?;
    Some(PixelWindow { x: x0, y: y0, w: x1 - x0, h: y1 - y0 })
}
