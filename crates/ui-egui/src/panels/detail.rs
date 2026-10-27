//! The Detail (loupe) view: the developed photo, before/after, zoom & pan, the filmstrip, and the
//! on-canvas tools (crop, brush/gradient masks, remove spots, white-balance picker).

use egui::{Align2, Color32, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2, pos2, vec2};
use lightcraft_catalog::PhotoId;
use lightcraft_develop::{DevelopSettings, MaskShape, mask_handles};
use lightcraft_geom::{Affine, Point};
use lightcraft_pipeline::geometry::Frame;
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::haptics::{self, Haptic};
use crate::render::Slot;
use crate::state::{BeforeAfter, RightPanel, ToolOverlay, Zoom};
use crate::theme::Tokens;
use crate::widgets::{register, register_geometry};

mod crop;
mod filmstrip;
mod geometry;
mod info;
mod masks;
mod retouch;
mod window;

pub(crate) use crop::crop_angle_label;
use crop::crop_overlay;
#[cfg(test)]
pub(crate) use crop::readout_rect;
#[cfg(test)]
use crop::{flipped_corners, outside_crop_mesh, rotate_cursor_turn};
#[cfg(test)]
use filmstrip::film_label;
#[cfg(test)]
pub(crate) use filmstrip::film_spoken;
pub(crate) use filmstrip::{film_height, filmstrip, filmstrip_rising};
use filmstrip::navigator;
pub use geometry::CanvasMap;
#[cfg(test)]
pub(crate) use geometry::focus_box;
use geometry::{LOUPE_ZOOM_S, animated_rect, loupe_from, middle_pan};
pub(crate) use geometry::{fit_rect, fit_texture_rect, focus_pan, navigate_gesture, output_px, pan_image, snap_fit_rect, texture_side};
#[cfg(test)]
use info::RENDERING_AFTER_SECS;
use info::{
    FILTER_PILL_HEIGHT, FILTER_PILL_INSET, brief_info, filter_pill, filter_pill_shown, info_overlay, info_overlay_extent, label_badge_rect,
    preview_only_pill, quick_name,
};
pub(crate) use info::{draw_focus_point, failed_pill, loupe_pill, rendering_overdue};
use masks::{RegionEdit, color_range_pick, lens_brush, long_edge_points, mask_overlay, region_overlay};
use retouch::{eye_overlay, remove_overlay};
use window::{WindowCtx, WindowView, draw_window, request_window};

/// An in-progress on-canvas gesture.
#[derive(Clone, Debug)]
pub enum Gesture {
    Brush {
        points: Vec<Point>,
        /// Pen pressure (0..1) at each point; empty when no pen pressure was reported (a mouse) or the
        /// brush's Pen Pressure switch is off.
        pressure: Vec<f32>,
    },
    Spot {
        points: Vec<Point>,
    },
    /// Remove tool: dragging spot `spot`'s target (or its source).
    SpotMove {
        spot: usize,
        source: bool,
    },
    CropHandle {
        handle: u8,
        start: lightcraft_geom::Rect,
        angle: f64,
        /// The screen ↔ photo mapping when the drag started: with Generative Expand the view can
        /// widen as the crop grows, and the pointer must keep meaning what it meant (else the crop
        /// would chase the widening view).
        map: CanvasMap,
    },
    /// Straighten tool: a line drawn along something that should be level (or plumb).
    StraightenLine {
        a: Pos2,
    },
    CropRotate {
        start_angle: f64,
        a0: f32,
    },
    Pan {
        start: (f32, f32),
        at: Pos2,
    },
    /// Dragging component `comp` of `mask`: its pin (`handle` 0) or a linear gradient's start (1)
    /// or end (2).
    MaskHandle {
        mask: u32,
        comp: usize,
        handle: u8,
        original_shape: MaskShape,
    },
    /// Guided Upright: a guide being drawn from `a` (normalized transformed coordinates).
    Guide {
        a: Point,
    },
    /// Guided Upright: dragging the endpoint `end` (0 = `guide.0`, 1 = `guide.1`) of guide `i`,
    /// from where the pointer went down (`from` — egui forgets the press origin at the release).
    GuideEndpoint {
        i: usize,
        end: usize,
        from: Pos2,
    },
    /// Red Eye: an ellipse being dragged from corner `a` (normalized).
    Eye {
        a: Point,
    },
    /// Targeted adjustment drag from `at` (normalized); `acc` = vertical travel not yet applied.
    Targeted {
        at: Point,
        acc: f32,
    },
    /// A drag of one of the histogram's tonal zones (see [`super::histogram`]): the control the
    /// press locked and the unrounded value its sideways travel has reached. It holds no canvas
    /// geometry: the zone stays locked wherever the pointer goes.
    HistogramDrag {
        control: &'static str,
        value: f64,
    },
}

/// Draw the Develop loupe: the optional filmstrip, the fitted or zoomed photo (before/after and
/// full-screen aware), crop overlays and the render requests for the current view, or, when no
/// photo is active, the empty-library message if the library is empty and "No photo selected"
/// otherwise.
pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let full = ui.max_rect();
    let fullscreen = app.ui.fullscreen;
    // the compact layout has no filmstrip (it would eat the loupe)
    let film_h = film_height(ui.ctx(), app.ui.filmstrip, !fullscreen && !app.compact);
    let canvas = Rect::from_min_max(full.min, pos2(full.right(), full.bottom() - film_h));
    app.canvas_rect = Some(canvas);
    filmstrip_rising(app, ui, full, film_h);
    let Some(id) = app.session.active() else {
        if super::empty_library::shown(&app.session) {
            super::empty_library::show(app, ui, canvas);
        } else {
            super::empty_message(ui, canvas, "No photo selected", "Choose a photo in the grid or filmstrip");
        }
        return;
    };
    let Some(photo) = app.session.catalog.photo(id).cloned() else { return };
    brief_info(app, ui.ctx(), id);
    let d = (*photo.develop).clone();
    // the full-screen preview shows the photo only: no tool overlays
    let right = if fullscreen { RightPanel::None } else { app.ui.right };
    let crop_tool = right == RightPanel::Crop;
    let frame = Frame::with_lens(photo.width.max(1) as usize, photo.height.max(1) as usize, &d, !crop_tool, app.session.embedded_lens(id).as_ref());
    let aspect = frame.aspect() as f32;
    let ppp = ui.ctx().pixels_per_point();
    // review mode (phone): the bars are away and the readout of the stars and flag is over the photo
    let review = app.compact && app.ui.review && !fullscreen;
    let margin = if fullscreen || review {
        0.0
    } else if crop_tool {
        // (a phone's handles sit just outside the frame, and the panels under the photo are tall)
        if app.compact { 22.0 } else { 48.0 }
    } else {
        24.0
    };
    // (on the phone the margin follows the bars away)
    let margin = ui.ctx().animate_value_with_time(egui::Id::unique("loupe-margin"), margin, if app.compact { 0.2 } else { 0.0 });
    let area = canvas.shrink(margin);
    let native = output_px(&frame);
    let texture_side = texture_side(ui.ctx());
    // two views (before, after): side by side or stacked
    let split = matches!(app.ui.before_after, BeforeAfter::SideBySide | BeforeAfter::TopBottom);
    let split_view = matches!(app.ui.before_after, BeforeAfter::Split | BeforeAfter::SplitTopBottom);
    let areas: Vec<Rect> = match app.ui.before_after {
        BeforeAfter::SideBySide => {
            let half = area.width() / 2.0 - 6.0;
            vec![
                Rect::from_min_size(area.min, vec2(half, area.height())),
                Rect::from_min_size(pos2(area.center().x + 6.0, area.top()), vec2(half, area.height())),
            ]
        }
        BeforeAfter::TopBottom => {
            let half = area.height() / 2.0 - 14.0;
            vec![
                Rect::from_min_size(area.min, vec2(area.width(), half)),
                Rect::from_min_size(pos2(area.left(), area.center().y + 14.0), vec2(area.width(), half)),
            ]
        }
        _ => vec![area],
    };
    let main_area = *areas.last().unwrap_or(&area);
    // A previous crop/rotation can stay visible while its replacement renders. Its pixels
    // must not override the current frame geometry, even when it is the same photo.
    let current_texture = |t: &&crate::render::Tex| t.photo == id && t.look == Some((crate::render::geometry_look(&d), !crop_tool));
    let visible_texture_aspect = app
        .renderer
        .textures
        .get(&Slot::Main)
        .filter(current_texture)
        .or_else(|| app.renderer.textures.get(&Slot::Preview).filter(current_texture))
        .or_else(|| app.renderer.thumb(id).filter(current_texture))
        .map(|tex| tex.size[0].max(1) as f32 / tex.size[1].max(1) as f32);
    // Follow the texture only where it really differs (an unsupported raw's embedded JPEG with
    // another crop): a texture's whole-pixel size is a little off the exact aspect, and the crop
    // tool's overlays need the frame's own geometry.
    let display_aspect = visible_texture_aspect.filter(|a| !crop_tool && (a / aspect - 1.0).abs() > 0.02).unwrap_or(aspect);
    let resp = ui.interact(canvas, egui::Id::unique("loupe"), Sense::click_and_drag());
    crate::widgets::announce(&resp, crate::i18n::tr("Photo"));
    let mut target_rect = fit_rect(main_area, display_aspect, app.ui.zoom, native, ppp, app.ui.pan);
    let mut img_rect = animated_rect(ui.ctx(), &mut app.ui.zoom_anim, target_rect);
    let in_pane = |pane: Rect, image: Rect| image.translate(pane.center() - main_area.center());
    let mut navigating = false;
    for &view_area in &areas {
        let view_img = in_pane(view_area, img_rect);
        if navigate_gesture(app, ui, &resp, view_area, view_img, native) || middle_pan(app, ui, &resp, view_area, view_img) {
            navigating = true;
            target_rect = fit_rect(main_area, display_aspect, app.ui.zoom, native, ppp, app.ui.pan);
            img_rect = target_rect;
            break;
        }
    }
    let before_img = in_pane(areas[0], img_rect);
    let before_target = in_pane(areas[0], target_rect);
    app.image_rect = Some(img_rect);
    // the phone's carousel: a drag across a fitted photo carries it (and its neighbours) along
    let resp = ui.interact(canvas, egui::Id::unique("loupe"), Sense::click_and_drag());
    let zoomed = img_rect.width() > canvas.width() + 1.0 || img_rect.height() > canvas.height() + 1.0;
    let tool_free = !matches!(right, RightPanel::Crop | RightPanel::Masking | RightPanel::Remove | RightPanel::RedEye) && app.ui.tool.is_empty();
    // a finger held on the photo shows the Before until it lifts (side by side, it is on screen)
    let hold = before_hold(ui, &resp, tool_free && !split);
    let held = hold.on || hold.lifted;
    let live = app.compact && tool_free && !zoomed && !app.ui.zoom_anim && !held && !ui.input(|i| i.multi_touch().is_some());
    let moved = if app.compact { swipe(app, ui, &resp, canvas, id, live) } else { Moved::default() };
    let slide = moved.slide;
    let mut img_rect = img_rect.translate(vec2(slide, 0.0));
    // pulled down: the photo follows the finger, smaller the further it goes; the grid shows through
    // behind it (`compact::content`)
    let pull_k = (moved.pull.y / canvas.height().max(1.0)).clamp(0.0, 1.0);
    if moved.pull != Vec2::ZERO {
        img_rect = Rect::from_center_size(img_rect.center() + moved.pull, img_rect.size() * (1.0 - PULL_SHRINK * pull_k));
    }
    ui.ctx().data_mut(|d| d.insert_temp(egui::Id::unique("loupe-pull"), if moved.dismiss { 0.0 } else { pull_k }));
    if moved.dismiss {
        // let go: back to the grid, the photo flying to its place there
        let now = ui.input(|i| i.time);
        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::unique("loupe-homing"), Homing { photo: id.0, from: img_rect, since: now }));
        let _ = app.run("view.back", json!({}));
    }
    // request renders: the loupe at display resolution (drafts during drags)
    let interacting = app.session.interaction.is_some();
    let overlay = view_overlay(app, &d);
    // a drag drafts at full size where the GPU draws the view (by the GPU's own rule, `handles`;
    // `ready` first: `available` would wait for a device still being created)
    let gpu = interacting
        && lightcraft_engine::gpu::handles(
            lightcraft_pipeline::OutputDepth::U8,
            app.ui.soft_proof && lightcraft_pipeline::proof_profile(app.ui.proof.profile).is_some(),
            overlay,
        )
        && lightcraft_engine::gpu::ready()
        && lightcraft_engine::gpu::available();
    let shown = app.renderer.textures.get(&Slot::Main).filter(|t| t.photo == id && t.quick.is_none()).map(|t| (t.key, t.ms));
    let drag = app.session.interaction.as_ref().map(|i| (i.photo, i.label.as_str()));
    let now = ui.input(|i| i.time);
    let drafting = app.drag_scale.scale(drag, gpu, shown.map(|(key, _)| key), now);
    // render at the final size: a click-zoom animation only changes how the result is drawn
    let native_long = native[0].max(native[1]);
    let drawn_long = target_rect.width().max(target_rect.height()) * ppp;
    // the whole frame at about canvas size; the zoomed part of it is a window render (below)
    let look = d.hash64();
    // where a drag can't draft at full size (no GPU: the browser, a LUT profile…), a draft is smaller the
    // slower this host renders
    let full_edge = app.ui.settings.loupe_edge(drawn_long.min(canvas.width().max(canvas.height()) * ppp), native_long, texture_side) as f64;
    let long_over_short = f64::from(aspect).max(1e-3).max(1.0 / f64::from(aspect).max(1e-3));
    // (each slider learns its own draft size: `DraftBudget`)
    let budget = app.renderer.draft_scale(drafting.map(|_| drag.map_or("", |(_, label)| label)), full_edge * full_edge / long_over_short);
    let draft = drafting.map(|s| if s >= 1.0 { s } else { s.min(budget) });
    // Diagnostic views that need whole-frame context, or reads too far for one window, still
    // render the whole frame at the size it is drawn. Per-pixel ones (a mask, clipping) and soft
    // proofing are window-safe.
    let sizes = crate::region::ViewSizes {
        drawn_long,
        canvas_long: canvas.width().max(canvas.height()) * ppp,
        native_long,
        texture_side,
        draft,
        windows: lightcraft_apphost::region::overlay_supports_window(overlay),
    };
    let mut plan = crate::region::plan(&app.ui.settings, sizes);
    if plan.window_edge.is_some_and(|edge| app.window_refused == Some((id, look, edge))) {
        plan = crate::region::plan(&app.ui.settings, crate::region::ViewSizes { windows: false, ..sizes });
    }
    // time the full-size drafts where they are what shows: under a window the whole-frame draft
    // is small anyway, and the frame's time is the window's upload
    if let Some((key, ms)) = shown.filter(|_| plan.window_edge.is_none()) {
        app.drag_scale.observe(key, ms, app.perf.host_frame_ms);
    }
    // a pinch changes the zoom every frame: keep the sizes of before it until it is quiet
    let plan = app.size_hold.apply(id, now, navigating, plan);
    // the drawn size in pixels, clamped first: a NaN or negative float must not become a size
    let drawn_px = |points: f32| (points * ppp).round().clamp(0.0, 65_536.0) as usize;
    let fit_px = app.size_hold.display_fit(now, navigating, [drawn_px(target_rect.width()), drawn_px(target_rect.height())]);
    if app.size_hold.holding(now) {
        ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(crate::region::HOLD_SECS));
    }
    // At rest the whole frame renders at no less than the preview source level: the source is then
    // copied 1:1 (no resample in the pipeline, whose prefilter and bilinear pass soften detail near
    // the output's limit) and shrunk once, by the Lanczos display fit below.
    // (zoomed in under a window render, the whole frame is hidden: it stays canvas-sized)
    let want = if interacting || plan.window_edge.is_some() {
        plan.main_edge
    } else {
        // (a user's explicit Preview size below the preview level still caps it)
        let limit = if app.ui.settings.preview_limit == 0 { usize::MAX } else { app.ui.settings.preview_limit as usize };
        let floor = lightcraft_engine::SourceLevel::Preview.max_edge().min(limit).min(native_long);
        plan.main_edge.max(floor).min(texture_side.max(crate::state::MIN_TEXTURE_SIDE))
    };
    // The short side is rounded, not truncated: a truncated box (2560 x 1706 for a 2560 x 1707
    // source) made the render a slightly anisotropic resample of its source, a 2-tap blur along
    // one axis only. A box that matches the source to rounding is rendered at exactly its size.
    let short = |e: usize, a: f32| ((e as f32 / a).round() as usize).max(1);
    let (rw, rh) = if aspect >= 1.0 { (want, short(want, aspect)) } else { (short(want, 1.0 / aspect), want) };
    // hover and before renders are stand-ins: the preview size is plenty and keeps them cheap
    let se = app.ui.settings.stand_in_edge(want, texture_side);
    let (sw, sh) = if aspect >= 1.0 { (se, short(se, aspect)) } else { (short(se, 1.0 / aspect), se) };
    // (bars sliding in or out resize the canvas every frame: the render on show is drawn scaled
    // meanwhile, and the photo is rendered once, at the size it ends up)
    let resizing = ui.data_mut(|d| {
        let last = d.get_temp::<Vec2>(egui::Id::unique("loupe-canvas"));
        d.insert_temp(egui::Id::unique("loupe-canvas"), canvas.size());
        last.is_some_and(|l| (l - canvas.size()).abs().max_elem() > 0.5)
    });
    let drawn = app.renderer.textures.get(&Slot::Main).is_some_and(|t| t.photo == id);
    if !(resizing && drawn)
        && let Some(job) = app.session.loupe_job(id, rw.max(8), rh.max(8), !crop_tool)
    {
        let job = if interacting { job.draft() } else { job };
        // at rest the render is shrunk to exactly the size it is drawn at with a Lanczos filter on
        // the worker, so the window draws it texel for texel instead of shrinking it with 2-tap
        // sampling (the render is up to a 64 px step larger, or the preview level)
        let job = if interacting { job } else { job.with_display_fit(fit_px[0], fit_px[1]) };
        let job = job.with_overlay(overlay).with_proof(app.ui.soft_proof.then_some(app.ui.proof));
        let job = job.with_hidden_grading(app.ui.grading_hidden_for(id));
        app.renderer.request(Slot::Main, job, 100);
        // a drag's draft that renders in a few ms (GPU) is drawn in this frame, not the next (zoomed
        // in, what changes on screen is the window's draft)
        #[cfg(not(target_arch = "wasm32"))]
        if interacting && plan.window_edge.is_none() {
            let frame = ui.input(|i| i.predicted_dt).clamp(1.0 / 240.0, 1.0 / 30.0);
            // (a frame time that is not a number waits for nothing)
            let budget = std::time::Duration::try_from_secs_f32(frame * 0.4).unwrap_or_default();
            app.renderer.wait_for_draft(ui.ctx(), &mut app.session, budget);
        }
    }
    // hovering a preset or profile: the photo with that look, shown instead of the loupe render
    // once it is ready (nothing is committed)
    let hover_key = match app.hover_preview.clone() {
        Some(h) if !interacting => app.session.preview_job(id, sw.max(8), sh.max(8), !crop_tool, &h.settings).map(|job| {
            // (the same display fit as the After view, drawn texel for texel like it)
            let job = job.with_display_fit(fit_px[0], fit_px[1]);
            let key = job.key;
            app.renderer.request(Slot::Hover, job, 105);
            (key, h.label)
        }),
        _ => None,
    };
    // once this photo is on screen: prepare its neighbours in filmstrip order (source decoded and
    // kept, view render cached) so stepping to them is instant; the phone also keeps their renders to
    // slide them in, and `near` is where each is drawn (a photo's width and a gap beside this one).
    // Not while the bars slide: a render at each size on the way would hold up this photo's own.
    let idle =
        !interacting && !resizing && !app.renderer.is_pending(Slot::Main) && app.renderer.textures.get(&Slot::Main).is_some_and(|t| t.photo == id);
    app.renderer.keep_neighbours = app.compact;
    let mut near = Vec::new();
    if idle || slide != 0.0 {
        let ids = app.session.visible_shared().1;
        if let Some(i) = ids.iter().position(|p| *p == id) {
            let next = ids.get(i + 1).copied();
            let prev = i.checked_sub(1).and_then(|j| ids.get(j)).copied();
            for (n, nid) in [next, prev].into_iter().enumerate() {
                let Some(nid) = nid else { continue };
                let Some(np) = app.session.catalog.photo(nid).cloned() else { continue };
                let nf = Frame::with_lens(
                    np.width.max(1) as usize,
                    np.height.max(1) as usize,
                    &np.develop,
                    !crop_tool,
                    app.session.embedded_lens(nid).as_ref(),
                );
                let na = nf.aspect() as f32;
                let nr = fit_rect(main_area, na, app.ui.zoom, output_px(&nf), ppp, app.ui.pan);
                let beside = (canvas.width() + SLIDE_GAP) * if n == 0 { 1.0 } else { -1.0 };
                near.push((nid, nr.translate(vec2(slide + beside, 0.0))));
                let nw =
                    app.ui.settings.prefetch_edge(nr.width().max(nr.height()) * ppp, output_px(&nf).into_iter().max().unwrap_or(1), texture_side);
                let (w, h) = if na >= 1.0 { (nw, (nw as f32 / na) as usize) } else { ((nw as f32 * na) as usize, nw) };
                if idle && let Some(job) = app.session.loupe_job(nid, w.max(8), h.max(8), !crop_tool) {
                    app.renderer.prefetch(Slot::Prefetch(n as u8), job, PREFETCH_PRIORITY);
                }
            }
        }
    }
    // until the loupe has this photo: show its cached render / embedded preview / a thumbnail
    if app.renderer.textures.get(&Slot::Main).is_none_or(|t| t.photo != id)
        && let Some(q) = app.session.quick_view_job(id, want.max(8), !crop_tool)
    {
        app.renderer.request_quick(Slot::Preview, q, 110);
    }
    let show_before = app.ui.before_after == BeforeAfter::Original || hold.on || ui.input(|i| i.key_down(egui::Key::Backslash));
    app.loupe_before = show_before && !split;
    if (split || show_before || split_view)
        && let Some(job) = app.session.render_job(id, sw.max(8), sh.max(8), true, !crop_tool)
    {
        // (the same display fit as the After view, so both panes are the same texture size)
        let job = if interacting { job } else { job.with_display_fit(fit_px[0], fit_px[1]) };
        app.renderer.request(Slot::Before, job, 90);
    }
    // zoomed past what the whole-frame render holds: also render the part on screen, at no more
    // than 100 % (the GPU magnifies beyond that), for each side that is shown. Mask overlays and
    // soft proofing use the same window; diagnostic overlays that need whole-frame context do not.
    let windowable_view =
        hover_key.is_none() && lightcraft_apphost::region::overlay_supports_window(overlay) && crate::region::same_aspect(display_aspect, aspect);
    let window_ctx = WindowCtx {
        id,
        overlay,
        frame_edge: plan.window_edge.filter(|_| windowable_view),
        drawn_long,
        aspect,
        ppp,
        texture_side,
        crop_tool,
        interacting,
        draft_scale: crate::render::DraftBudget::window_scale(draft.unwrap_or(1.0)),
        look,
        holding: navigating || app.size_hold.holding(now),
    };
    // (slot, before side?, rect to draw in, rect it is requested for, area whose pixels show)
    let mut window_views: Vec<WindowView> = Vec::new();
    if window_ctx.frame_edge.is_some() {
        let after = WindowView { slot: Slot::Region, before: false, img: img_rect, target: target_rect, visible: main_area };
        if split {
            window_views.push(WindowView { slot: Slot::RegionBefore, before: true, img: before_img, target: before_target, visible: areas[0] });
            window_views.push(after);
        } else if show_before {
            window_views.push(WindowView { slot: Slot::RegionBefore, before: true, img: img_rect, target: target_rect, visible: canvas });
        } else if split_view {
            window_views.push(WindowView { slot: Slot::RegionBefore, before: true, img: img_rect, target: target_rect, visible: canvas });
            window_views.push(WindowView { visible: canvas, ..after });
        } else {
            window_views.push(WindowView { visible: canvas, ..after });
        }
    }
    let mut windows: Vec<(WindowView, crate::region::RegionView)> = Vec::new();
    for v in window_views {
        if let Some(view) = request_window(app, &window_ctx, &v) {
            windows.push((v, view));
        }
    }
    for (slot, before) in [(Slot::Region, false), (Slot::RegionBefore, true)] {
        let shown = windows.iter().find(|(v, _)| v.before == before).map(|(_, view)| *view);
        if before {
            app.region_before_view = shown;
        } else {
            app.region_view = shown;
        }
        if shown.is_none() {
            app.region_tiles.retain(|(b, _), _| *b != before);
            app.renderer.release(slot);
        }
    }
    let p = ui.painter_at(canvas);
    // an HDR picture was drawn this frame (the window's surface goes to HDR output for it)
    let hdr_drawn = std::cell::Cell::new(false);
    // what a view slot shows: its own render of this photo, else the stand-ins (no blank frame
    // between photos, and the full render replaces them in place)
    let draw = |slot: Slot, r: Rect, clip: Rect| -> &'static str {
        let mine = |s: Slot| app.renderer.textures.get(&s).filter(|t| t.photo == id);
        let (tex, what) = if let Some(t) = mine(slot) {
            (t, "render")
        } else if let Some(t) = mine(Slot::Prefetch(0)).or_else(|| mine(Slot::Prefetch(1))) {
            (t, "neighbour")
        } else if let Some(t) = mine(Slot::Preview) {
            (t, t.quick.map(quick_name).unwrap_or("preview"))
        } else if let Some(t) = app.renderer.thumb(id) {
            (t, "thumb")
        } else {
            return "none";
        };
        let image_rect = snap_fit_rect(fit_texture_rect(r, tex.size), tex.size, ppp);
        p.with_clip_rect(clip).image(tex.paint_id(), image_rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        hdr_drawn.set(hdr_drawn.get() || tex.hdr.is_some());
        what
    };
    // soft proofing: a paper-white surround and the proof's name, as Lightroom shows it
    if app.ui.soft_proof && !fullscreen {
        p.rect_filled(canvas, 0.0, Color32::from_gray(238));
        let label = crate::i18n::tr_format!("Proof Preview · {}", app.ui.proof.space.label());
        p.text(pos2(canvas.right() - 16.0, canvas.top() + 14.0), Align2::RIGHT_CENTER, label, t.font(12.5), Color32::from_gray(60));
    }
    // Generative Expand: the part of the frame past the photo that nothing fills yet is
    // transparent in the render; a checkerboard shows through it, as in Lightroom
    if frame.expands() && !fullscreen {
        crate::expand::checkerboard(&p.with_clip_rect(canvas), img_rect.intersect(canvas));
    }
    let shown;
    // the Before label's pill, painted once the window renders are down (they would cover it)
    let mut before_badge: Option<(Rect, std::sync::Arc<egui::Galley>)> = None;
    // below what else lives in the canvas' top-left corner: the filter pill, the info overlay
    let pill_inset = if !fullscreen && filter_pill_shown(app) { FILTER_PILL_INSET + FILTER_PILL_HEIGHT } else { 0.0 };
    let badge_top_inset = pill_inset.max(info_overlay_extent(app, &p, &photo));
    if split {
        // each view stays in its pane: zoomed in, a photo reaches past it (and would cover the
        // other view's margins with its whole-frame render, magnified and blurred)
        let br = before_img;
        draw(Slot::Before, br, areas[0]);
        shown = draw(Slot::Main, img_rect, main_area);
        // the labels under each photo, or under its pane when the photo reaches past it
        let label_at = |r: Rect, pane: Rect| pos2(r.left().max(pane.left()), r.bottom().min(pane.bottom()) + 14.0);
        p.text(label_at(br, areas[0]), Align2::LEFT_CENTER, crate::i18n::tr("Before"), t.font(12.0), t.text_dim);
        p.text(label_at(img_rect, main_area), Align2::LEFT_CENTER, crate::i18n::tr("After"), t.font(12.0), t.text_dim);
    } else if show_before {
        shown = draw(Slot::Before, img_rect, canvas);
        // on the photo itself, so it gets the hover label's dark pill to stay readable over light
        // pictures; painted after the window renders below, which would cover it zoomed in
        let g = p.layout_no_wrap(crate::i18n::tr("Before").to_string(), t.font(12.0), Color32::WHITE);
        before_badge = Some((label_badge_rect(img_rect, canvas, g.size(), badge_top_inset), g));
    } else if let Some((_key, label)) = &hover_key
        && let Some(tex) = app.renderer.textures.get(&Slot::Hover).filter(|t| t.photo == id)
    {
        let hover_rect = fit_texture_rect(img_rect, tex.size);
        p.image(tex.tex.id(), hover_rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        let g = p.layout_no_wrap(label.clone(), t.font(12.0), Color32::WHITE);
        let bg = label_badge_rect(hover_rect, canvas, g.size(), badge_top_inset);
        p.rect_filled(bg, 4.0, Color32::from_black_alpha(160));
        p.galley(bg.min + vec2(8.0, 4.0), g, Color32::WHITE);
        shown = "hover";
    } else {
        shown = draw(Slot::Main, img_rect, canvas);
        if shown == "none" {
            match app.renderer.failure_of(Slot::Main, id) {
                Some(e) => {
                    // a synced photo whose preview is still on its way isn't broken
                    let waiting = app.session.awaiting_preview(&photo);
                    p.text(
                        canvas.center() - vec2(0.0, 10.0),
                        Align2::CENTER_CENTER,
                        crate::i18n::tr(if waiting { "Downloading previews" } else { "This photo can't be opened" }),
                        t.semibold(14.0),
                        t.text,
                    );
                    p.text(canvas.center() + vec2(0.0, 12.0), Align2::CENTER_CENTER, e, t.font(12.0), t.text_dim);
                }
                None if app.ui.settings.loupe_messages => {
                    p.text(canvas.center(), Align2::CENTER_CENTER, crate::i18n::tr("Rendering…"), t.font(13.0), t.text_dim);
                }
                None => {}
            }
        }
    }
    app.loupe_shown = Some((id, shown));
    // an HDR picture on screen: the window's surface shows highlights above SDR white while it is (the
    // Before alone, a hover preview or a stand-in is not one)
    app.hdr_on_screen = hdr_drawn.get();
    // (neighbours wait beside a fitted photo only: zoomed, their rects, at this zoom, are far wider
    // than the canvas and would be drawn over the photo)
    for (nid, r) in near.iter().filter(|(_, r)| !zoomed && r.intersects(canvas)) {
        if let Some(t) = tex_of(&app.renderer, *nid) {
            p.image(t.tex.id(), *r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
    }
    // a wipe shows the Before whole-frame render on its side of the line…
    let line = crate::region::wipe_line(img_rect, canvas);
    if app.ui.before_after == BeforeAfter::Split
        && let Some(tex) = app.renderer.textures.get(&Slot::Before).filter(|t| t.photo == id)
    {
        let before_rect = fit_texture_rect(img_rect, tex.size);
        let u = ((line.x - before_rect.left()) / before_rect.width().max(1.0)).clamp(0.0, 1.0);
        let left = Rect::from_min_max(before_rect.min, pos2(before_rect.left() + u * before_rect.width(), before_rect.bottom()));
        p.image(tex.tex.id(), left, Rect::from_min_max(pos2(0.0, 0.0), pos2(u, 1.0)), Color32::WHITE);
    }
    if app.ui.before_after == BeforeAfter::SplitTopBottom
        && let Some(tex) = app.renderer.textures.get(&Slot::Before).filter(|t| t.photo == id)
    {
        let before_rect = fit_texture_rect(img_rect, tex.size);
        let v = ((line.y - before_rect.top()) / before_rect.height().max(1.0)).clamp(0.0, 1.0);
        let top = Rect::from_min_max(before_rect.min, pos2(before_rect.right(), before_rect.top() + v * before_rect.height()));
        p.image(tex.tex.id(), top, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, v)), Color32::WHITE);
    }
    // …and the window renders go over the whole-frame ones, all of them, where they belong in the
    // frame (clipped to their pane, or their side of the line)
    for (v, view) in &windows {
        // (holding `\` in a wipe shows the Before everywhere: it is the Before view alone then)
        let mode = if show_before { BeforeAfter::Original } else { app.ui.before_after };
        let clip = crate::region::window_clip(mode, v.before, v.visible, canvas, img_rect);
        draw_window(&p.with_clip_rect(clip), app, &window_ctx, v, view);
    }
    if let Some((bg, g)) = before_badge {
        p.rect_filled(bg, 4.0, Color32::from_black_alpha(160));
        p.galley(bg.min + vec2(8.0, 4.0), g, Color32::WHITE);
        register(ui, "canvas:beforeLabel", bg);
    }
    // the line of a wipe over both
    if app.ui.before_after == BeforeAfter::Split {
        p.line_segment([pos2(line.x, img_rect.top()), pos2(line.x, img_rect.bottom())], Stroke::new(1.5, Color32::WHITE));
    }
    if app.ui.before_after == BeforeAfter::SplitTopBottom {
        p.line_segment([pos2(img_rect.left(), line.y), pos2(img_rect.right(), line.y)], Stroke::new(1.5, Color32::WHITE));
    }
    if app.ui.show_clipping && !show_before && !fullscreen {
        clipping_overlay(app, &p, img_rect);
    }
    // View ▸ Loupe Overlay ▸ Layout Image, over the photo (the After view in a before/after)
    super::layout_overlay::draw(app, &p, img_rect);
    register(ui, "canvas:image", img_rect);
    let map = CanvasMap::new(&frame, img_rect);
    if app.compact {
        review_hud(ui, &p, canvas, &photo, review);
    }
    info_overlay(app, &p, canvas, &photo);
    if !fullscreen && !review {
        filter_pill(app, ui, canvas);
    }
    if app.compact {
        // a phone: the faces are framed with their names under them, a tap names one (not in review
        // mode, where the photo is alone, nor with a drawing tool in hand)
        let drawing = matches!(right, RightPanel::Crop | RightPanel::Masking | RightPanel::Remove | RightPanel::RedEye) || !app.ui.tool.is_empty();
        if (app.ui.face_boxes || app.ui.tag_face) && !review && !fullscreen && !drawing && !navigating {
            let screen = |x: f64, y: f64| map.screen(Point::new(x, y));
            let norm = |p: Pos2| {
                let n = map.norm(p);
                (n.x, n.y)
            };
            super::people_mobile::loupe_faces(app, ui, &screen, &norm, img_rect, &photo, d.orientation);
        }
    } else if app.ui.face_boxes {
        // a name box left open on another photo is dropped
        if app.ui.name_edit.as_ref().is_some_and(|e| e.photo != id.0) {
            app.ui.name_edit = None;
        }
        let hints = super::faces::hints_for(app, id.0);
        let people = if app.ui.name_edit.is_some() { app.caches.person_names(&app.session.catalog) } else { Default::default() };
        // while the name box is open and recognition is not set up, the box offers the next step
        let setup = if app.ui.name_edit.is_some() { super::faces::setup(app, ui.ctx()) } else { super::faces::Setup::Running };
        match region_overlay(ui, &p, &map, &photo, d.orientation, hints.as_deref(), &people, &mut app.ui.name_edit, setup) {
            Some(RegionEdit::SetUp) => {
                app.ui.name_edit = None;
                super::faces::take_step(app, ui.ctx(), setup);
            }
            Some(RegionEdit::Remove(index)) => {
                let _ = app.run("photo.removeRegion", json!({"id": id.0, "index": index}));
            }
            Some(RegionEdit::Name(index, name)) => {
                app.ui.name_edit = None;
                let _ = app.run("faces.setName", json!({"id": id.0, "index": index, "name": name}));
            }
            Some(RegionEdit::Resize(index, r)) => {
                let _ = app.run("photo.setRegion", json!({"id": id.0, "index": index, "rect": {"x0": r.x0, "y0": r.y0, "x1": r.x1, "y1": r.y1}}));
            }
            None => {}
        }
    }
    if app.ui.settings.focus_point && !crop_tool {
        draw_focus_point(&p.with_clip_rect(img_rect.intersect(canvas)), img_rect, &photo, "canvas:focusPoint");
    }
    // a fine grid while a transform (geometry) slider is dragged, to judge verticals
    if app.ui.dragging_control.as_deref().is_some_and(|c| c.starts_with("geometry.")) {
        let n = 12;
        let stroke = Stroke::new(1.0, Color32::from_white_alpha(70));
        for i in 1..n {
            let fx = img_rect.left() + img_rect.width() * i as f32 / n as f32;
            p.line_segment([pos2(fx, img_rect.top()), pos2(fx, img_rect.bottom())], stroke);
        }
        let rows = ((n as f32) * img_rect.height() / img_rect.width()).round().max(2.0) as usize;
        for i in 1..rows {
            let fy = img_rect.top() + img_rect.height() * i as f32 / rows as f32;
            p.line_segment([pos2(img_rect.left(), fy), pos2(img_rect.right(), fy)], stroke);
        }
    }
    // Cursor selection must also run on gesture frames, when tool input is suppressed.
    if !matches!(right, RightPanel::Crop | RightPanel::Masking | RightPanel::Remove | RightPanel::RedEye) {
        general_cursor(app, ui, &resp, img_rect, main_area);
    }
    // (the phone's Geometry tab shows the photo as the sliders change it, with no crop frame)
    let crop_frame = !(app.compact && app.ui.crop_geometry && app.ui.tool.is_empty());
    if !navigating {
        match right {
            RightPanel::Crop if crop_frame => crop_overlay(app, ui, &resp, &map, &frame, &d, id),
            RightPanel::Masking => mask_overlay(app, ui, &resp, &map, &d),
            RightPanel::Remove => remove_overlay(app, ui, &resp, &map, &d),
            RightPanel::RedEye => eye_overlay(app, ui, &resp, &map, &d),
            _ => {
                // side by side / stacked: Before shows the same frame, so a click in its view (white
                // balance, colour samples, targeted adjustment) means the same point of the photo.
                // Zoomed in, a view's image reaches past its pane: what counts is the pane clicked.
                // A click-zoom there centres both views on the point clicked (#1742).
                let pointer = resp.interact_pointer_pos().or(resp.hover_pos());
                let (map, img, pane) = match (split, areas.first(), pointer) {
                    (true, Some(before), Some(q)) if before.contains(q) => (CanvasMap::new(&frame, before_img), before_img, *before),
                    _ => (map, img_rect, main_area),
                };
                general_interaction(app, ui, &resp, &map, img, pane, native, aspect, held)
            }
        }
    }
    // drawn and hit-tested above the loupe and its tools: clicks on it pan
    let nav = navigator(app, ui, canvas, img_rect, id);
    // what is on screen isn't the render on its way (a stand-in, the last render after an edit,
    // a magnified frame under its window): say so once that has lasted a moment
    let whole: &[Slot] = if show_before && !split {
        &[Slot::Before]
    } else if split || split_view {
        &[Slot::Main, Slot::Before]
    } else {
        &[Slot::Main]
    };
    let coming = !matches!(shown, "none" | "hover")
        && whole.iter().chain(windows.iter().map(|(v, _)| &v.slot)).any(|s| app.renderer.is_pending(*s) && !app.renderer.retrying(*s));
    let keys =
        [Slot::Main, Slot::Before, Slot::Region, Slot::RegionBefore].map(|s| app.renderer.textures.get(&s).filter(|t| t.photo == id).map(|t| t.key));
    let rendering = ui.ctx().data_mut(|d| rendering_overdue(d.get_temp_mut_or(egui::Id::unique("loupe-rendering"), None), keys, coming, now));
    // the render failed while a stand-in is up: what is on screen is all there will be (also while
    // a library change retries it)
    let failed = (!rendering
        && (!show_before || split)
        && !matches!(shown, "render" | "none" | "hover")
        && (!app.renderer.is_pending(Slot::Main) || app.renderer.retrying(Slot::Main)))
    .then(|| app.renderer.failure_of(Slot::Main, id))
    .flatten();
    if rendering || failed.is_some() {
        // over the photo (the After in two views); in the crop tool's wide margin, under it
        let over = if crop_tool { canvas } else { img_rect.intersect(if split { main_area } else { canvas }) };
        let over = if over.is_positive() { over } else { canvas };
        match failed {
            Some(e) => failed_pill(ui, app, &photo, canvas, over, nav, "loupe:renderFailed", e),
            // View Options' Show message when loading or rendering photos
            None if app.ui.settings.loupe_messages => {
                loupe_pill(ui, canvas, over, nav, "loupe:rendering", crate::i18n::tr("Rendering…"), false);
            }
            None => {}
        }
    }
    if let Some(why) = photo.preview_only.as_deref().filter(|_| !fullscreen && !review) {
        preview_only_pill(ui, canvas, why);
    }
    // (a finger held on the photo shows the Before: egui's long touch opens no menu then)
    if hold.on && resp.long_touched() {
        return;
    }
    crate::widgets::context_menu(&resp, |ui| {
        crate::widgets::menu_button(ui, crate::i18n::tr("Zoom"), |ui| {
            for (label, cmd) in [("Fit", "view.zoomFit"), ("100%", "view.zoom100"), ("Zoom In", "view.zoomIn"), ("Zoom Out", "view.zoomOut")] {
                if ui.button(crate::i18n::tr(label)).clicked() {
                    let _ = app.run(cmd, json!({}));
                }
            }
        });
        ui.separator();
        super::grid::context_menu(app, ui, id);
    });
}

/// Neighbour prefetch: below on-screen thumbnails, above background thumbnail refreshes.
const PREFETCH_PRIORITY: u32 = 4;

/// The best the renderer has to show of `id` right now: a render of it (the loupe's or a
/// neighbour's), else its stand-in or thumbnail.
fn tex_of(r: &crate::render::Renderer, id: PhotoId) -> Option<&crate::render::Tex> {
    [Slot::Main, Slot::Prefetch(0), Slot::Prefetch(1), Slot::Preview]
        .iter()
        .find_map(|s| r.textures.get(s).filter(|t| t.photo == id))
        .or_else(|| r.thumb(id))
}

/// Review mode's readout over the photo: its stars at the bottom left and its flag at the bottom
/// right. It fades in as the bars go. A swipe shows its stars and flag as the finger makes them
/// (`Swipe::shown`): each star fills with a little pop as it is reached, the flag changes into the
/// next one, the pill pops at every step and the side under the finger swells.
fn review_hud(ui: &egui::Ui, p: &egui::Painter, canvas: Rect, photo: &lightcraft_catalog::Photo, on: bool) {
    use crate::icons::{Icon, paint};
    let ctx = ui.ctx();
    let fade = ctx.animate_bool_with_time(egui::Id::unique("review-hud"), on, 0.25);
    if fade <= 0.0 {
        return;
    }
    let t = Tokens::get(ctx);
    let (s, now) = (ui.data(|d| d.get_temp::<Swipe>(egui::Id::unique("loupe-swipe"))).unwrap_or_default(), ui.input(|i| i.time));
    let (rating, flag) = s.shown.unwrap_or_else(|| stars_and_flag(photo));
    // how much bigger than its size at rest a side is: a pop after a change, a swell under the finger
    let size = |flag: bool| {
        let u = s.pop.filter(|(_, f)| *f == flag).map_or(1.0, |(at, _)| (now - at) / POP_S);
        let pop = if u < 1.0 { (u as f32 * std::f32::consts::PI).sin() } else { 0.0 };
        let touched = s.sideways == Some(false) && (s.from.x > canvas.center().x) == flag;
        let swell = ctx.animate_value_with_time(egui::Id::unique(("review-swell", flag)), if touched { 0.12 } else { 0.0 }, 0.12);
        if u < 1.0 || swell > 0.0 {
            ctx.request_repaint();
        }
        1.0 + 0.35 * pop + swell
    };
    let (h, y) = (44.0, canvas.bottom() - 16.0 - 22.0);
    // (the glyphs' colours follow the appearance: dark pills under light ones, light under dark)
    let fill = if Tokens::is_dark(ctx) { Color32::from_black_alpha((150.0 * fade) as u8) } else { Color32::from_white_alpha((210.0 * fade) as u8) };
    let pill = |c: Pos2, w: f32, k: f32| p.rect_filled(Rect::from_center_size(c, vec2(w, h) * k), h / 2.0 * k, fill);
    // the stars
    let (star, gap) = (26.0, 6.0);
    let w = 5.0 * star + 4.0 * gap + 28.0;
    let k = size(false);
    // (a pill grows from its screen edge, not out of it)
    let c = pos2(canvas.left() + 16.0 + w * k / 2.0, y);
    pill(c, w, k);
    crate::widgets::register_geometry(ctx, "review:stars", Rect::from_center_size(c, vec2(w, h) * k));
    for i in 0..5u8 {
        // 0 → 1 as the star is reached; it swells on the way (and shrinks back as it is given up)
        let f = ctx.animate_bool_with_time(egui::Id::unique(("review-star", i)), rating > i, 0.14);
        let bump = 1.0 + 1.6 * f * (1.0 - f);
        let at = c + vec2((f32::from(i) - 2.0) * (star + gap), 0.0) * k;
        let r = Rect::from_center_size(at, vec2(star, star) * k * bump);
        paint(p, r, Icon::Star, t.text_dim.gamma_multiply(fade * (1.0 - f)));
        paint(p, r, Icon::StarFilled, t.star.gamma_multiply(fade * f));
    }
    // the flag: the three states cross-fade
    let k = size(true);
    let c = pos2(canvas.right() - 16.0 - h * k / 2.0, y);
    pill(c, h, k);
    crate::widgets::register_geometry(ctx, "review:flag", Rect::from_center_size(c, vec2(h, h) * k));
    for (ix, icon, color) in [(0u8, Icon::FlagReject, t.reject), (1, Icon::FlagPick, t.text_dim), (2, Icon::FlagPick, t.pick)] {
        let f = ctx.animate_bool_with_time(egui::Id::unique(("review-flag", ix)), flag == ix, 0.14);
        if f > 0.0 {
            paint(p, Rect::from_center_size(c, vec2(star, star) * k * (0.8 + 0.2 * f)), icon, color.gamma_multiply(fade * f));
        }
    }
}

/// Targeted adjustment tool: dragging up/down on the photo raises/lowers the tone-curve region or
/// the colour-mixer bands under the press point (`develop.targeted`, one call per whole step, all
/// in one interaction = one undo step).
fn targeted_drag(app: &mut LightcraftApp, resp: &egui::Response, map: &CanvasMap, target: &str) {
    if resp.drag_started()
        && let Some(q) = resp.interact_pointer_pos()
    {
        let _ = app.run("develop.beginInteraction", json!({"label": "Targeted Adjustment"}));
        app.gesture = Some(Gesture::Targeted { at: map.norm(q), acc: 0.0 });
    }
    if resp.dragged()
        && let Some(Gesture::Targeted { at, acc }) = &mut app.gesture
    {
        *acc -= resp.drag_delta().y * 0.5;
        let step = acc.trunc();
        if step != 0.0 {
            *acc -= step;
            let at = *at;
            let mut p = json!({"target": target, "x": at.x, "y": at.y, "delta": step});
            if target == "curve" && app.ui.curve_channel != "parametric" {
                p["channel"] = json!(app.ui.curve_channel);
            }
            let _ = app.run("develop.targeted", p);
        }
    }
    if resp.drag_stopped() && matches!(app.gesture, Some(Gesture::Targeted { .. })) {
        app.gesture = None;
        let _ = app.run("develop.endInteraction", json!({}));
    }
}

/// The diagnostic overlay the loupe shows (an Alt-drag preview, the selected mask, Point Color's
/// visualized range, Visualize Spots).
pub(crate) fn view_overlay(app: &LightcraftApp, d: &DevelopSettings) -> lightcraft_pipeline::Overlay {
    use lightcraft_pipeline::{MaskView, Overlay};
    if app.ui.fullscreen {
        return Overlay::None;
    }
    // Lightroom's temporary previews: clipping, greyscale, sharpening detail / mask
    if app.ui.right == RightPanel::Edit
        && app.ui.alt_held
        && let Some(o) = app.ui.dragging_control.as_deref().and_then(Overlay::for_control)
    {
        return o;
    }
    // Show every visible mask's region at once (Ctrl+Shift+O / Cmd+Shift+O), ahead of the selected-mask overlay
    // so the shortcut is not shadowed by the always-on single-mask overlay.
    if app.ui.right == RightPanel::Masking && app.ui.mask_overlay_all && d.masks.iter().any(|m| m.visible && !m.components.is_empty()) {
        return Overlay::AllMasks {
            view: MaskView::parse(&app.ui.mask_overlay_mode).unwrap_or_default(),
            color: app.ui.mask_overlay_color,
            opacity: app.ui.mask_overlay_opacity.clamp(0.0, 100.0).round() as u8,
        };
    }
    if app.ui.right == RightPanel::Masking
        && app.ui.mask_overlay
        && let Some(m) = app.session.active_mask.and_then(|id| d.masks.iter().find(|m| m.id == id))
        && !m.components.is_empty()
    {
        return Overlay::Mask {
            id: m.id.min(u16::MAX as u32) as u16,
            view: MaskView::parse(&app.ui.mask_overlay_mode).unwrap_or_default(),
            color: app.ui.mask_overlay_color,
            opacity: app.ui.mask_overlay_opacity.clamp(0.0, 100.0).round() as u8,
        };
    }
    if app.ui.right == RightPanel::Remove && app.ui.visualize_spots {
        return Overlay::Spots(app.ui.spots_threshold.clamp(0.0, 100.0).round() as u8);
    }
    let edit = app.ui.right == RightPanel::Edit;
    if edit && app.ui.point_color_visualize && app.ui.flyout_open("pointColor") && app.ui.point_color < d.point_colors.len() {
        return Overlay::PointColorRange(app.ui.point_color as u8);
    }
    if edit && app.ui.hdr_visualize && d.hdr.enabled {
        return Overlay::HdrRange;
    }
    // (painting the Refine brush shows the depth it changes)
    if edit && (app.ui.depth_visualize || app.ui.tool == crate::lensblur::BRUSH_TOOL) && d.lens_blur.enabled {
        return Overlay::Depth;
    }
    Overlay::None
}

fn clipping_overlay(app: &LightcraftApp, p: &egui::Painter, r: Rect) {
    // Highlight clipped regions using the histogram's extremes isn't spatial; show a subtle frame hint.
    if let Some((h, photo)) = app.renderer.textures.get(&Slot::Main).and_then(|t| t.histogram.as_ref().map(|h| (h, t.photo))) {
        let limit = app.session.develop_of(photo).map_or(f64::NAN, |d| d.hdr.max_ev);
        let (lo, hi) = h.clipping_at(limit);
        if hi > 0.003 {
            p.rect_stroke(r, 0.0, Stroke::new(2.0, Color32::from_rgb(255, 60, 60)), StrokeKind::Outside);
        }
        if lo > 0.003 {
            p.rect_stroke(r.expand(3.0), 0.0, Stroke::new(2.0, Color32::from_rgb(60, 120, 255)), StrokeKind::Outside);
        }
    }
}

/// How far (points) from a crop handle a finger still grabs it (a handle is 44 pt wide with margin).
const TOUCH_HANDLE_REACH: f32 = 28.0;

/// How far (points) an up / down swipe must travel for each star, and for each step of the flag
/// (reject, none, pick); half of it already counts, so the readout answers the first movement.
const STAR_STEP_PT: f32 = 44.0;
const FLAG_STEP_PT: f32 = 56.0;
/// The gap (points) between photos in the phone loupe's carousel.
const SLIDE_GAP: f32 = 16.0;
/// A sideways drag settles on the next photo when it carried the photo this fraction of its width
/// across, or was this fast (points per second) and went a few points.
const SLIDE_FRACTION: f32 = 0.25;
const SLIDE_FLING: f32 = 600.0;
/// How quickly a let-go photo settles: the distance left shrinks by e^-10 each second.
const SLIDE_SETTLE: f32 = 10.0;
/// A tap shows or hides the bars once no second finger has come down this many seconds after it
/// lifted (a double tap's second comes sooner, and zooms).
const TAP_GAP_S: f64 = 0.2;
/// How long the ring that answers a tap on the phone loupe lasts (seconds).
const TAP_RING_S: f64 = 0.35;
/// A finger held still this long on the photo shows the Before until it lifts (a slow tap is well
/// under it; iOS's own long press takes half a second).
const HOLD_S: f64 = 0.4;
/// How long the review readout's pop lasts (seconds).
const POP_S: f64 = 0.45;

/// A one-finger drag on the phone loupe, kept from frame to frame.
#[derive(Clone, Copy, Default)]
struct Swipe {
    /// Where the photo is, as points from rest, and where it was when the finger came down.
    off: f32,
    base: f32,
    /// The finger: how far it has moved, how fast it was going sideways, where it came down, and
    /// whether the drag is sideways (the carousel) or up / down (rating), known once it has moved.
    d: Vec2,
    v: f32,
    from: Pos2,
    sideways: Option<bool>,
    /// The last frame this ran, and the last rating / flag change (when, was it the flag?).
    t: f64,
    pop: Option<(f64, bool)>,
    /// A rating / flag swipe: the (stars, flag) the photo had when it began, and what the finger
    /// has made of them so far, which the readout shows (and which is kept for the frame the finger
    /// lifts, when the photo's own are not yet updated). The flag is 0 reject, 1 none, 2 pick.
    start: (u8, u8),
    shown: Option<(u8, u8)>,
    /// The photo this ran for last, to slide in a change made by anything but the finger.
    id: u64,
    /// A pull down (back to the grid): the photo's offset from rest, and whether the finger has it.
    pull: Vec2,
    pulling: bool,
}

/// What a one-finger drag on the phone loupe made of the photo this frame.
#[derive(Clone, Copy, Default)]
struct Moved {
    /// Sideways from rest (the carousel), in points.
    slide: f32,
    /// Pulled down (and sideways with it) from rest, in points.
    pull: Vec2,
    /// Let go far or fast enough down: back to the grid.
    dismiss: bool,
}

/// A pull down this far (points), or a flick down this fast (points per second), goes back to the grid.
const PULL_DISMISS: f32 = 110.0;
const PULL_FLING: f32 = 700.0;
/// How much smaller the photo is pulled all the way down the screen.
const PULL_SHRINK: f32 = 0.45;

/// How far the photo is pulled down this frame, as a part of the screen (0..1): the grid shows
/// through behind it and the bars fade as it goes (`compact::content`, `compact::top_bar`).
pub fn pulled(ctx: &egui::Context) -> f32 {
    ctx.data(|d| d.get_temp::<f32>(egui::Id::unique("loupe-pull"))).unwrap_or(0.0)
}

/// A photo let go on its way back to the grid: it flies from where it was to its place there.
#[derive(Clone, Copy, Debug)]
pub struct Homing {
    pub photo: u64,
    pub from: Rect,
    pub since: f64,
}

/// How long the photo takes to fly back to its place in the grid (s).
pub const HOMING_S: f64 = 0.32;

/// Where the active photo's thumbnail was drawn in the grid, and in which frame: the whole photo's
/// rect (past the cell's edges where the cell shows only its middle). A photo opened from the grid
/// grows out of it.
#[derive(Clone, Copy, Debug)]
struct Tile {
    photo: u64,
    rect: Rect,
    frame: u64,
}

/// The grid drew the active photo `id`'s thumbnail at `shown`, the part `uv` of the photo.
pub(crate) fn note_tile(ctx: &egui::Context, id: PhotoId, shown: Rect, uv: Rect) {
    let size = vec2(shown.width() / uv.width().max(1e-3), shown.height() / uv.height().max(1e-3));
    let rect = Rect::from_min_size(shown.min - vec2(uv.min.x * size.x, uv.min.y * size.y), size);
    let frame = ctx.cumulative_frame_nr();
    ctx.data_mut(|d| d.insert_temp(egui::Id::unique("grid-active-tile"), Tile { photo: id.0, rect, frame }));
}

/// Call each frame before the view is drawn: when it has just changed between the grid and the
/// photo, a photo opened from the grid grows out of its thumbnail, and one put away flies back to
/// its place there ([`homing`], drawn by the grid).
pub(crate) fn view_changed(app: &mut LightcraftApp, ctx: &egui::Context) {
    use crate::state::ViewMode;
    let key = egui::Id::unique("view-shown");
    let now = app.ui.view;
    let was = ctx
        .data_mut(|d| {
            let was = d.get_temp::<ViewMode>(key);
            d.insert_temp(key, now);
            was
        })
        .unwrap_or(now);
    let grid = |v: ViewMode| matches!(v, ViewMode::PhotoGrid | ViewMode::SquareGrid);
    let active = app.session.active();
    if grid(was) && now == ViewMode::Detail {
        let tile: Option<Tile> = ctx.data(|d| d.get_temp(egui::Id::unique("grid-active-tile")));
        // (drawn in the frame before: the grid was on show with this photo in it)
        if let Some(tile) = tile.filter(|t| Some(PhotoId(t.photo)) == active && t.frame + 2 >= ctx.cumulative_frame_nr()) {
            loupe_from(ctx, tile.rect);
            app.ui.zoom_anim = true;
            let since = ctx.input(|i| i.time);
            ctx.data_mut(|d| d.insert_temp(egui::Id::unique("loupe-growing"), (tile.photo, since)));
        }
    } else if was == ViewMode::Detail
        && grid(now)
        && homing(ctx).is_none()
        && let (Some(id), Some(from)) = (active, app.image_rect)
    {
        let since = ctx.input(|i| i.time);
        ctx.data_mut(|d| d.insert_temp(egui::Id::unique("loupe-homing"), Homing { photo: id.0, from, since }));
    }
}

/// The photo growing out of its thumbnail as it opens, while it does, and how far it has (0 … 1).
pub fn growing(ctx: &egui::Context) -> Option<(PhotoId, f32)> {
    let (photo, since): (u64, f64) = ctx.data(|d| d.get_temp(egui::Id::unique("loupe-growing")))?;
    let k = (ctx.input(|i| i.time) - since) / LOUPE_ZOOM_S;
    (0.0..1.0).contains(&k).then_some((PhotoId(photo), k as f32))
}

/// The photo flying back to the grid, while it does.
pub fn homing(ctx: &egui::Context) -> Option<Homing> {
    let h: Homing = ctx.data(|d| d.get_temp(egui::Id::unique("loupe-homing")))?;
    let now = ctx.input(|i| i.time);
    (now - h.since < HOMING_S).then_some(h)
}

/// The phone loupe's last tap: when and where it lifted, whether the bars are still to change for
/// it, and whether they have.
#[derive(Clone, Copy, Default)]
struct LoupeTap {
    at: f64,
    pos: Pos2,
    pending: bool,
    toggled: bool,
}

/// A finger held on the photo to see the Before, kept from frame to frame.
#[derive(Clone, Copy, Default)]
struct BeforeHold {
    /// The press this follows (when it came down), and whether it can no longer become a hold.
    press: Option<f64>,
    void: bool,
    /// The Before is showing.
    on: bool,
    /// The finger lifted from a hold this frame: its release is no tap.
    lifted: bool,
}

/// What a rating / flag swipe in progress (or just lifted) is showing: (stars, flag).
#[cfg(test)]
pub(crate) fn swiping(ctx: &egui::Context) -> Option<(u8, u8)> {
    ctx.data(|d| d.get_temp::<Swipe>(egui::Id::unique("loupe-swipe"))).and_then(|s| s.shown)
}

/// The stars and flag (0 reject, 1 none, 2 pick) of a photo, as a swipe changes them.
fn stars_and_flag(photo: &lightcraft_catalog::Photo) -> (u8, u8) {
    use lightcraft_catalog::Flag;
    (
        photo.rating,
        match photo.flag {
            Flag::Reject => 0,
            Flag::None => 1,
            Flag::Pick => 2,
        },
    )
}

/// What `dy` points of drag (up is negative) make of `start` on one side: the stars (left) or the
/// flag (right), a step for each stretch of the finger.
fn swiped(start: (u8, u8), flag_side: bool, dy: f32) -> (u8, u8) {
    let up = -dy;
    if flag_side {
        (start.0, (f32::from(start.1) + (up / FLAG_STEP_PT).round()).clamp(0.0, 2.0) as u8)
    } else {
        ((f32::from(start.0) + (up / STAR_STEP_PT).round()).clamp(0.0, 5.0) as u8, start.1)
    }
}

/// Is there a photo before / after `id` in view order?
fn has_neighbours(app: &mut LightcraftApp, id: PhotoId) -> (bool, bool) {
    let ids = app.session.visible();
    ids.iter().position(|p| *p == id).map_or((false, false), |i| (i > 0, i + 1 < ids.len()))
}

/// What a one-finger drag does on the phone loupe, run before the photo is drawn so it follows the
/// finger: sideways across a fitted photo carries it, with its neighbours alongside, and letting go
/// settles on the next / previous photo (far or fast enough) or back; up / down, in review mode,
/// rates (left half) or flags (right half). Returns the photo's offset from rest, in points.
fn swipe(app: &mut LightcraftApp, ui: &egui::Ui, resp: &egui::Response, canvas: Rect, id: PhotoId, live: bool) -> Moved {
    let key = egui::Id::unique("loupe-swipe");
    let mut s: Swipe = ui.data(|d| d.get_temp(key)).unwrap_or_default();
    let now = ui.input(|i| i.time);
    if now - s.t > 0.25 {
        (s.off, s.id) = (0.0, id.0); // (it was off screen a while: it starts at rest)
        (s.pull, s.pulling) = (Vec2::ZERO, false);
    }
    let mut dismiss = false;
    s.t = now;
    let w = canvas.width() + SLIDE_GAP;
    if !resp.dragged() && !resp.drag_stopped() {
        s.shown = None;
    }
    // another photo than last time, and not by the finger (a button, an arrow key): it slides in
    // from the side it lies on
    if s.id != id.0 {
        let ids = app.session.visible();
        let at = |n: u64| ids.iter().position(|p| p.0 == n);
        if let (Some(a), Some(b)) = (at(s.id), at(id.0))
            && a.abs_diff(b) == 1
        {
            s.off = if b > a { w } else { -w };
        }
        s.id = id.0;
    }
    if resp.drag_started() {
        // a photo still settling can be caught
        (s.base, s.d, s.sideways) = (s.off, Vec2::ZERO, None);
        s.from = ui.input(|i| i.pointer.press_origin()).unwrap_or(canvas.center());
    }
    // (the photo drawn this frame: the one before a change of photo, which comes next frame)
    let mut drawn = None;
    if live && resp.dragged() {
        s.d = resp.total_drag_delta().unwrap_or(s.d);
        s.v = ui.input(|i| i.pointer.velocity().x);
        if s.sideways.is_none() && s.d.length() > 8.0 {
            s.sideways = Some(s.d.x.abs() > s.d.y.abs());
            // down, with no rating to give: the photo is pulled back towards the grid
            s.pulling = s.sideways == Some(false) && !app.ui.review && s.d.y > 0.0;
        }
        if s.sideways == Some(true) {
            let (prev, next) = has_neighbours(app, id);
            let want = s.base + s.d.x;
            // nothing beyond the first and last photo: it gives way only grudgingly
            s.off = if (want > 0.0 && !prev) || (want < 0.0 && !next) { want * 0.3 } else { want };
        } else if s.pulling {
            // the photo follows the finger down (and sideways with it); back up past where it began
            // it gives way only grudgingly
            s.pull = vec2(s.d.x, if s.d.y >= 0.0 { s.d.y } else { s.d.y * 0.3 });
        } else if s.sideways == Some(false) && app.ui.review {
            // up / down: the stars (left half) or the flag (right half) follow the finger
            if s.shown.is_none() {
                s.start = app.session.catalog.photo(id).map(|p| stars_and_flag(p)).unwrap_or_default();
            }
            let flag_side = s.from.x > canvas.center().x;
            let now_shown = swiped(s.start, flag_side, s.d.y);
            if s.shown != Some(now_shown) && s.shown.is_some() {
                s.pop = Some((now, flag_side));
            }
            // a tick for each star or flag step the finger reaches (or gives up)
            if s.shown.unwrap_or(s.start) != now_shown {
                haptics::tap(ui.ctx(), Haptic::Selection);
            }
            s.shown = Some(now_shown);
        }
    } else {
        if resp.drag_stopped() && s.pulling {
            let vy = ui.input(|i| i.pointer.velocity().y);
            dismiss = s.pull.y > PULL_DISMISS || (vy > PULL_FLING && s.pull.y > 24.0);
            if dismiss {
                haptics::tap(ui.ctx(), Haptic::Light);
            }
            (s.pulling, s.sideways) = (false, None);
        }
        if resp.drag_stopped() {
            match s.sideways.filter(|_| live) {
                Some(true) => {
                    let (prev, next) = has_neighbours(app, id);
                    let dir = s.off.signum(); // -1: towards the next photo
                    let far = s.off.abs() > w * SLIDE_FRACTION || (s.v * dir > SLIDE_FLING && s.off.abs() > 24.0);
                    if far && if dir < 0.0 { next } else { prev } {
                        drawn = Some(s.off);
                        let _ = app.run(if dir < 0.0 { "library.next" } else { "library.previous" }, json!({}));
                        haptics::tap(ui.ctx(), Haptic::Light);
                        s.off -= dir * w; // (the new photo is where its neighbour was)
                        s.id = app.session.active().map_or(s.id, |p| p.0);
                    }
                }
                Some(false) if app.ui.review => {
                    // (the readout has been showing it all along; now it is kept)
                    if let Some(end) = s.shown.filter(|end| *end != s.start) {
                        let flag_side = s.from.x > canvas.center().x;
                        rate_to(app, id, flag_side, end);
                        haptics::tap(ui.ctx(), Haptic::Light);
                        s.pop = Some((now, flag_side));
                        ui.ctx().request_repaint();
                    }
                }
                _ => {}
            }
            s.sideways = None;
        }
        if drawn.is_none() && s.off != 0.0 {
            s.off *= (-SLIDE_SETTLE * ui.input(|i| i.stable_dt)).exp();
            if s.off.abs() < 0.5 {
                s.off = 0.0;
            }
        }
        // a pull let go short of the grid springs back (one that went back is drawn where it was)
        if !s.pulling && s.pull != Vec2::ZERO && !dismiss {
            s.pull *= (-SLIDE_SETTLE * 1.4 * ui.input(|i| i.stable_dt)).exp();
            if s.pull.length() < 0.5 {
                s.pull = Vec2::ZERO;
            }
        }
    }
    if s.off != 0.0 || s.pull != Vec2::ZERO {
        ui.ctx().request_repaint();
    }
    let pull = s.pull;
    if dismiss {
        s.pull = Vec2::ZERO;
    }
    ui.data_mut(|d| d.insert_temp(key, s));
    Moved { slide: drawn.unwrap_or(s.off), pull, dismiss }
}

/// Press and hold: a finger held still on the photo for [`HOLD_S`] shows the Before, as holding `\`
/// does, until it lifts (as Lightroom's mobile app shows the original). A finger that moves first is
/// a swipe or a pan and a second one a pinch; touch only, since a mouse held down starts a pan. Run
/// before the photo is drawn; `allowed`: no tool takes the press.
fn before_hold(ui: &egui::Ui, resp: &egui::Response, allowed: bool) -> BeforeHold {
    let key = egui::Id::unique("loupe-hold");
    let mut h: BeforeHold = ui.data(|d| d.get_temp(key)).unwrap_or_default();
    let (touch, start, moved, fingers, now) = ui.input(|i| {
        let moved = i.pointer.total_drag_delta().map_or(0.0, |d| d.length());
        (i.any_touches(), i.pointer.press_start_time(), moved, i.multi_touch().is_some(), i.time)
    });
    let slop = ui.ctx().options(|o| o.input_options.max_click_dist);
    match start.filter(|_| touch) {
        // no finger down: the After is back
        None => h = BeforeHold { lifted: h.on, ..Default::default() },
        Some(t0) => {
            if h.press != Some(t0) {
                h = BeforeHold { press: Some(t0), ..Default::default() };
            }
            h.lifted = false;
            if !h.on && !h.void {
                let held = now - t0;
                if moved > slop || fingers || !allowed {
                    h.void = true;
                } else if held < HOLD_S {
                    ui.ctx().request_repaint_after(std::time::Duration::try_from_secs_f64(HOLD_S - held).unwrap_or_default());
                } else if resp.is_pointer_button_down_on() || resp.long_touched() {
                    // (it came down on the photo, not on a button over it)
                    h.on = true;
                    haptics::tap(ui.ctx(), Haptic::Medium);
                } else {
                    h.void = true;
                }
            }
        }
    }
    ui.data_mut(|d| d.insert_temp(key, h));
    h
}

/// Is a finger on a touch screen behind this frame's pointer input (rather than a mouse)? Also in
/// the frame a finger lifts: its touch has ended then, but the end comes with the pointer's release.
fn touch_input(ui: &egui::Ui) -> bool {
    ui.input(|i| i.any_touches() || i.events.iter().any(|e| matches!(e, egui::Event::Touch { .. })))
}

/// Two-finger gestures on the loupe: pinch zooms (back to Fit when it shrinks past the fitted
/// size), two fingers pan a zoomed photo. Returns whether two or more fingers are down, so the
/// one-finger pan and swipe stand down.
#[allow(clippy::too_many_arguments)]
fn touch_gestures(app: &mut LightcraftApp, ui: &egui::Ui, img: Rect, canvas: Rect, native: [usize; 2], aspect: f32, zoomed: bool) -> bool {
    let Some(mt) = ui.input(|i| i.multi_touch()) else { return false };
    let ppp = ui.ctx().pixels_per_point();
    if (mt.zoom_delta - 1.0).abs() > 1e-4 {
        let fit_w = fit_rect(canvas, aspect, Zoom::Fit, native, ppp, app.ui.pan).width();
        let w = img.width() * mt.zoom_delta;
        if w <= fit_w {
            app.ui.zoom = Zoom::Fit;
        } else {
            if !zoomed {
                // start the zoom under the fingers
                app.ui.pan =
                    (((mt.center_pos.x - img.left()) / img.width()).clamp(0.0, 1.0), ((mt.center_pos.y - img.top()) / img.height()).clamp(0.0, 1.0));
            }
            let pct = w * ppp / native[0].max(1) as f32 * 100.0;
            app.ui.zoom = Zoom::Percent(pct.clamp(5.0, 800.0));
        }
        app.ui.zoom_anim = false;
    }
    if zoomed {
        let d = mt.translation_delta;
        app.ui.pan.0 = (app.ui.pan.0 - d.x / img.width()).clamp(0.0, 1.0);
        app.ui.pan.1 = (app.ui.pan.1 - d.y / img.height()).clamp(0.0, 1.0);
    }
    true
}

fn general_cursor(app: &LightcraftApp, ui: &egui::Ui, resp: &egui::Response, img: Rect, canvas: Rect) {
    // A panel or overlay under the pointer keeps its own cursor.
    if !resp.dragged() && !resp.hovered() {
        return;
    }
    let cursor = match app.ui.tool.as_str() {
        "wbPicker" | "colorRange" | "pointColor" | crate::lensblur::FOCUS_TOOL => egui::CursorIcon::Crosshair,
        // (the brush draws its own outline)
        crate::lensblur::BRUSH_TOOL => return,
        tool if tool.starts_with("tat:") => egui::CursorIcon::ResizeVertical,
        _ if img.width() > canvas.width() + 1.0 || img.height() > canvas.height() + 1.0 => {
            if resp.dragged() {
                egui::CursorIcon::Grabbing
            } else {
                egui::CursorIcon::Grab
            }
        }
        _ => return,
    };
    ui.ctx().set_cursor_icon(cursor);
}

fn general_interaction(
    app: &mut LightcraftApp,
    ui: &mut egui::Ui,
    resp: &egui::Response,
    map: &CanvasMap,
    img: Rect,
    canvas: Rect,
    native: [usize; 2],
    aspect: f32,
    held: bool,
) {
    if app.ui.tool == "wbPicker" {
        if resp.clicked()
            && let Some(q) = resp.interact_pointer_pos()
        {
            let n = map.norm(q);
            let _ = app.run("develop.wbPick", json!({"x": n.x, "y": n.y}));
            app.ui.tool.clear();
        }
        return;
    }
    if let Some(target) = app.ui.tool.strip_prefix("tat:").map(str::to_string) {
        targeted_drag(app, resp, map, &target);
        return;
    }
    // Lens Blur's Refine brush
    if app.ui.tool == crate::lensblur::BRUSH_TOOL {
        lens_brush(app, ui, resp, map);
        return;
    }
    // Lens Blur's focus picker: a click focuses on the depth there (the tool stays on)
    if app.ui.tool == crate::lensblur::FOCUS_TOOL {
        if resp.clicked()
            && let Some(q) = resp.interact_pointer_pos()
        {
            let n = map.norm(q);
            if let Err(e) = app.run("lensBlur.focus", json!({"x": n.x, "y": n.y})) {
                app.toast_error(ui.ctx(), e);
            }
        }
        return;
    }
    if app.ui.tool == "colorRange" {
        color_range_pick(app, ui, resp, map);
        return;
    }
    if app.ui.tool == "pointColor" {
        if resp.clicked()
            && let Some(q) = resp.interact_pointer_pos()
        {
            let n = map.norm(q);
            if let Ok(r) = app.run("pointColor.pick", json!({"x": n.x, "y": n.y})) {
                app.ui.point_color = r["index"].as_u64().unwrap_or(0) as usize;
            }
            app.ui.tool.clear();
        }
        return;
    }
    let zoomed = img.width() > canvas.width() + 1.0 || img.height() > canvas.height() + 1.0;
    let two_fingers = touch_gestures(app, ui, img, canvas, native, aspect, zoomed);
    // (a finger lifting from a hold on the Before is no tap, and while it holds it doesn't pan)
    let (clicked, double_clicked) = (resp.clicked() && !held, resp.double_clicked() && !held);
    // a double-click leaves the editor for the grid (the first click's zoom is taken back, so the
    // photo isn't left magnified); on a touch layout the double tap zooms, below
    if double_clicked && !app.compact {
        app.ui.zoom = Zoom::Fit;
        app.ui.zoom_anim = false;
        let _ = app.run("view.back", json!({}));
        return;
    }
    // click toggles Fit ↔ the chosen click-zoom ratio at the clicked point; drag pans when zoomed.
    // On a touch layout only a double tap zooms (a single tap must not move the photo).
    if double_clicked || (clicked && !zoomed && !app.compact) {
        if let Some(q) = resp.interact_pointer_pos() {
            let u = ((q.x - img.left()) / img.width()).clamp(0.0, 1.0);
            let v = ((q.y - img.top()) / img.height()).clamp(0.0, 1.0);
            app.ui.pan = (u, v);
        }
        app.ui.zoom = if matches!(app.ui.zoom, Zoom::Fit) { Zoom::Percent(app.ui.click_zoom as f32) } else { Zoom::Fit };
        app.ui.zoom_anim = true;
    } else if clicked && zoomed && !app.compact {
        app.ui.zoom = Zoom::Fit;
        app.ui.zoom_anim = true;
    }
    if zoomed && resp.dragged() && !two_fingers && !held {
        pan_image(app, canvas, img, resp.drag_delta());
    }
    if app.compact {
        // a single tap shows or hides the bars (review mode) as soon as it can't be a double tap's
        // first, and is answered at once: a light haptic, and a ring where the finger lifted
        let (key, now) = (egui::Id::unique("loupe-tap"), ui.input(|i| i.time));
        let mut tap = ui.data(|m| m.get_temp(key)).unwrap_or(LoupeTap { at: f64::NEG_INFINITY, ..Default::default() });
        if double_clicked {
            // (a double tap slower than the wait has already changed the bars: they go back)
            if tap.toggled {
                let _ = app.run("view.reviewMode", json!({}));
            }
            (tap.pending, tap.toggled) = (false, false);
        } else if clicked {
            tap = LoupeTap { at: now, pos: resp.interact_pointer_pos().unwrap_or(canvas.center()), pending: true, toggled: false };
            haptics::tap(ui.ctx(), Haptic::Light);
        } else if resp.drag_started() {
            tap.pending = false; // (a tap then a swipe: the swipe)
        }
        // (a finger down on the photo again may be a double tap's second: wait for it to lift)
        if tap.pending && !resp.is_pointer_button_down_on() {
            if now - tap.at >= TAP_GAP_S {
                let _ = app.run("view.reviewMode", json!({}));
                (tap.pending, tap.toggled) = (false, true);
            } else {
                ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(tap.at + TAP_GAP_S - now));
            }
        }
        let u = ((now - tap.at) / TAP_RING_S) as f32;
        if (0.0..1.0).contains(&u) {
            // it swells (easing out) and fades; a dark halo keeps it seen on a light background
            let (r, a) = (18.0 + 14.0 * (1.0 - (1.0 - u).powi(3)), 1.0 - u);
            let p = ui.painter_at(canvas);
            p.circle(tap.pos, r, Color32::from_white_alpha((50.0 * a) as u8), Stroke::new(3.5, Color32::from_black_alpha((40.0 * a) as u8)));
            p.circle_stroke(tap.pos, r, Stroke::new(2.0, Color32::from_white_alpha((220.0 * a) as u8)));
            ui.ctx().request_repaint();
        }
        ui.data_mut(|m| m.insert_temp(key, tap));
    }
    let _ = (native, aspect);
}

/// Rating by swiping on a phone, as Lightroom's mobile app does when reviewing: up or down on the
/// photo's left half adds or takes stars, a stretch of finger for each; on its right half, up picks
/// and down rejects (towards the other flag from one of them first clears it). The review readout
/// follows the finger (`swiped`); this stores what it showed when the finger lifts.
fn rate_to(app: &mut LightcraftApp, id: PhotoId, flag_side: bool, (rating, flag): (u8, u8)) {
    if flag_side {
        let key = match flag {
            0 => "reject",
            2 => "pick",
            _ => "none",
        };
        let _ = app.run("photo.flag", json!({"ids": [id.0], "flag": key}));
    } else {
        let _ = app.run("photo.rate", json!({"ids": [id.0], "rating": rating}));
    }
}

#[cfg(test)]
mod tests {
    /// The rotation pointer's arc bulges away from the side of the crop box it is on, and follows
    /// a box that is itself rotated.
    #[test]
    fn rotate_cursor_faces_away_from_the_crop_box() {
        use egui::{Pos2, pos2};
        use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, PI};
        let near = |a: f32, b: f32| ((a - b + PI).rem_euclid(2.0 * PI) - PI).abs() < 1e-3;
        let pts = [pos2(100.0, 100.0), pos2(300.0, 100.0), pos2(300.0, 200.0), pos2(100.0, 200.0)];
        let turn = |x: f32, y: f32| super::rotate_cursor_turn(&pts, pos2(x, y), false);
        assert!(near(turn(200.0, 80.0), 0.0), "above: as drawn");
        assert!(near(turn(320.0, 150.0), FRAC_PI_2), "right");
        assert!(near(turn(200.0, 230.0), PI), "below");
        assert!(near(turn(70.0, 150.0), -FRAC_PI_2), "left");
        assert!(near(turn(290.0, 60.0), 0.0), "above the top edge near its end: still up");
        // off each corner: diagonally out from it
        assert!(near(turn(80.0, 80.0), -FRAC_PI_4), "top-left corner");
        assert!(near(turn(320.0, 80.0), FRAC_PI_4), "top-right corner");
        assert!(near(turn(320.0, 220.0), 3.0 * FRAC_PI_4), "bottom-right corner");
        assert!(near(turn(80.0, 220.0), -3.0 * FRAC_PI_4), "bottom-left corner");
        assert!(near(super::rotate_cursor_turn(&pts, pos2(200.0, 110.0), true), 0.0), "dragged back over the top edge: still up");
        // a box turned 90°: its top edge is now on the right
        let turned = [pts[3], pts[0], pts[1], pts[2]].map(|p| pos2(400.0 - p.y, p.x));
        assert!(near(super::rotate_cursor_turn(&turned, pos2(420.0, 200.0), false), FRAC_PI_2));
        // a box turned 30°: straight out from its top edge is 30° clockwise of up
        let c = pos2(200.0, 150.0);
        let rot = |p: Pos2| {
            let (s, co) = (30f32.to_radians().sin(), 30f32.to_radians().cos());
            let d = p - c;
            pos2(c.x + d.x * co - d.y * s, c.y + d.x * s + d.y * co)
        };
        let tilted = pts.map(rot);
        assert!(near(super::rotate_cursor_turn(&tilted, rot(pos2(200.0, 60.0)), false), 30f32.to_radians()));
    }

    use super::{film_label, flipped_corners, outside_crop_mesh, texture_side};
    use egui::{Color32, Pos2, Rect, pos2};

    /// A rotated `w` × `h` box centred on `centre`.
    fn box_at(centre: Pos2, w: f32, h: f32, angle: f32) -> [Pos2; 4] {
        let (s, c) = angle.to_radians().sin_cos();
        [(-w, -h), (w, -h), (w, h), (-w, h)].map(|(x, y)| pos2(centre.x + (x * c - y * s) / 2.0, centre.y + (x * s + y * c) / 2.0))
    }

    /// Signed distances from a convex polygon's edges, positive inwards (its winding either way).
    fn sides(poly: &[Pos2], p: Pos2) -> Vec<f32> {
        let n = poly.len() as f32;
        let centre = pos2(poly.iter().map(|q| q.x).sum::<f32>() / n, poly.iter().map(|q| q.y).sum::<f32>() / n);
        (0..poly.len())
            .map(|i| {
                let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
                let nrm = egui::vec2(a.y - b.y, b.x - a.x).normalized();
                nrm.dot(p - a) * nrm.dot(centre - a).signum()
            })
            .collect()
    }

    /// Issue #731: the dimming is the photo minus the crop box at any angle. Every point of the
    /// photo outside the box lies in exactly one of the mesh's triangles; no point inside the box
    /// or outside the photo lies in any. (Four quads joining photo and box corners crossed and
    /// overlapped once the box was rotated.)
    #[test]
    fn the_dimming_outside_a_rotated_crop_is_exactly_the_photo_minus_the_box() {
        let img = Rect::from_min_max(pos2(100.0, 50.0), pos2(500.0, 650.0));
        // boxes inside the photo, poking out of it, and bigger than it
        for (w, h) in [(240.0, 360.0), (420.0, 300.0), (700.0, 900.0)] {
            for angle in [0.0f32, 5.0, 15.0, 30.0, -45.0, 90.0, 137.0, 200.0] {
                let quad = box_at(img.center() + egui::vec2(30.0, -20.0), w, h, angle);
                let mesh = outside_crop_mesh(img, &quad, Color32::BLACK);
                let tris: Vec<[Pos2; 3]> = mesh
                    .indices
                    .chunks(3)
                    .map(|t| [mesh.vertices[t[0] as usize].pos, mesh.vertices[t[1] as usize].pos, mesh.vertices[t[2] as usize].pos])
                    .collect();
                let (mut inside, mut outside) = (0, 0);
                let mut y = img.top() - 20.0;
                while y < img.bottom() + 20.0 {
                    let mut x = img.left() - 20.0;
                    while x < img.right() + 20.0 {
                        let p = pos2(x, y);
                        let in_box = sides(&quad, p);
                        let in_img = sides(&[img.left_top(), img.right_top(), img.right_bottom(), img.left_bottom()], p);
                        // a pixel on an edge may fall either way: skip the boundaries themselves
                        if in_box.iter().chain(&in_img).any(|d| d.abs() < 1.0) {
                            x += 7.0;
                            continue;
                        }
                        let expected = if in_img.iter().all(|d| *d > 0.0) && !in_box.iter().all(|d| *d > 0.0) { 1 } else { 0 };
                        // a sample on the seam between two of a strip's fan triangles is in both
                        // or in neither, depending on rounding: count it both ways
                        let covered = |eps: f32| tris.iter().filter(|t| sides(*t, p).iter().all(|d| *d > eps)).count();
                        let (at_least, at_most) = (covered(0.01), covered(-0.01));
                        assert!(
                            at_least <= expected && expected <= at_most,
                            "{w}×{h} box at {angle}°: ({x}, {y}) lies in {at_least}..={at_most} triangle(s), not {expected}"
                        );
                        if expected == 1 {
                            outside += 1
                        } else if in_img.iter().all(|d| *d > 0.0) {
                            inside += 1
                        }
                        x += 7.0;
                    }
                    y += 7.0;
                }
                // (a box bigger than the photo swallows it whole at some angles)
                assert!(inside > 50 && (outside > 50 || w > img.width()), "{w}×{h} at {angle}°: {outside} dimmed, {inside} clear samples");
            }
        }
    }

    /// A flat or broken box dims nothing rather than something arbitrary (or panicking).
    #[test]
    fn a_degenerate_crop_box_dims_nothing() {
        let img = Rect::from_min_max(pos2(0.0, 0.0), pos2(400.0, 300.0));
        let point = [pos2(10.0, 10.0); 4];
        let line = [pos2(10.0, 10.0), pos2(90.0, 10.0), pos2(90.0, 10.0), pos2(10.0, 10.0)];
        let nan = [pos2(f32::NAN, 10.0), pos2(90.0, 10.0), pos2(90.0, 50.0), pos2(10.0, 50.0)];
        for quad in [point, line, nan] {
            assert!(outside_crop_mesh(img, &quad, Color32::BLACK).indices.is_empty(), "{quad:?}");
        }
        assert!(outside_crop_mesh(Rect::NAN, &box_at(pos2(50.0, 50.0), 20.0, 20.0, 10.0), Color32::BLACK).indices.is_empty());
    }

    /// A raw's embedded JPEG shown while the raw decodes, or the last render while an edit renders,
    /// looked like the finished photo: the loupe says "Rendering…" once a render has been coming for
    /// a moment with nothing new on screen, and doesn't blink while slow drafts arrive.
    #[test]
    fn the_loupe_says_rendering_when_the_screen_falls_behind() {
        use super::{RENDERING_AFTER_SECS as WAIT, rendering_overdue};
        let key = |k| [Some(k), None, None, None];
        let mut wait = None;
        assert!(!rendering_overdue(&mut wait, key(1), false, 0.0), "nothing coming");
        // an edit after an idle spell: the wait starts now, not at the last frame drawn
        assert!(!rendering_overdue(&mut wait, key(1), true, 30.0), "a render just asked for");
        assert!(!rendering_overdue(&mut wait, key(1), true, 30.0 + WAIT * 0.8), "a quick render never shows it");
        assert!(rendering_overdue(&mut wait, key(1), true, 30.0 + WAIT * 1.2), "nothing new for the wait");
        assert!(rendering_overdue(&mut wait, key(2), true, 31.0 + WAIT), "once up, a slow draft doesn't blink it");
        assert!(!rendering_overdue(&mut wait, key(3), false, 32.0), "done");
        // a drag whose drafts keep up: always something coming, a new draft every 0.1 s
        for i in 1..30u32 {
            assert!(!rendering_overdue(&mut wait, key(u64::from(i)), true, 40.0 + f64::from(i) * 0.1), "draft {i}");
        }
    }

    /// Issue #652: a native host reports the GPU's texture limit in the first frame's input only.
    /// The loupe must still know it in every later frame (it read the raw input, found nothing and
    /// cut every window render to 2048 px).
    #[test]
    fn the_texture_limit_outlives_the_frame_it_was_reported_in() {
        let ctx = egui::Context::default();
        assert_eq!(texture_side(&ctx), 2048, "until the host says: the smallest limit there is");
        let mut seen = Vec::new();
        for reported in [Some(8192), None, None] {
            let raw = egui::RawInput { max_texture_side: reported, ..Default::default() };
            let mut out = ctx.run_ui(raw, |ui| seen.push(texture_side(ui.ctx())));
            out.textures_delta.clear();
        }
        assert_eq!(seen, [8192, 8192, 8192]);
    }

    #[test]
    fn film_labels_cut_on_characters_not_bytes() {
        // the name from #266: byte 13 falls inside '限'
        assert_eq!(film_label("202407層三限定訂閱圖(4)"), "202407層三限定訂閱圖…");
        assert_eq!(film_label("写真"), "写真");
        // exactly 14 characters (42 bytes) is shown whole
        assert_eq!(film_label("一二三四五六七八九十一二三四"), "一二三四五六七八九十一二三四");
        assert_eq!(film_label("IMG_20240712_153012"), "IMG_20240712_…");
        assert_eq!(film_label("DSC_0001"), "DSC_0001");
        assert_eq!(film_label(""), "");
    }

    /// With the photo flipped, the crop box's corners reach the screen mirrored; the dimming
    /// outside the box pairs each photo corner with the box corner on the same side of the screen
    /// (it used to pair them in data order, so a flipped photo was dimmed inside the box).
    #[test]
    fn flipped_crop_box_corners_come_in_screen_order() {
        let screen = [pos2(10.0, 10.0), pos2(90.0, 10.0), pos2(90.0, 50.0), pos2(10.0, 50.0)];
        // data order (top-left, top-right, bottom-right, bottom-left) as each flip shows it
        let h = [screen[1], screen[0], screen[3], screen[2]];
        let v = [screen[3], screen[2], screen[1], screen[0]];
        let hv = [screen[2], screen[3], screen[0], screen[1]];
        assert_eq!(flipped_corners(&screen, false, false), screen);
        assert_eq!(flipped_corners(&h, true, false), screen);
        assert_eq!(flipped_corners(&v, false, true), screen);
        assert_eq!(flipped_corners(&hv, true, true), screen);
    }
}
