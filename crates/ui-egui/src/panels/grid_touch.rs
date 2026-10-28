//! Touch on the grid (the compact layout): choosing photos by a long press and a drag, the scroll
//! handle that runs through the whole library, and the check mark a chosen photo wears. The grid
//! (`grid.rs`) calls these where its cells and rows are laid out.

use egui::{Color32, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use lightcraft_catalog::{DateRun, PhotoId};
use serde_json::json;

use super::grid::{GridLayout, HEADER_H, rows_between};
use crate::LightcraftApp;
use crate::haptics::{self, Haptic};
use crate::theme::Tokens;
use crate::widgets::register;

/// The scroll offset that shows `jump` (a photo, or the first of a month or year) near the top.
pub(super) fn jump_offset(app: &LightcraftApp, ids: &[PhotoId], lay: &GridLayout, runs: &[DateRun], jump: &crate::state::GridJump) -> Option<f32> {
    match jump {
        crate::state::GridJump::Photo(id) => {
            let i = ids.iter().position(|x| x.0 == *id)?;
            // (a little below the top, with its date header above it)
            lay.cells.get(i).map(|c| (c.top() - HEADER_H - 8.0).max(0.0))
        }
        crate::state::GridJump::Date(d) => {
            if let Some(g) = runs.iter().position(|r| r.key.starts_with(d.as_str()))
                && let Some(h) = lay.headers.get(g)
            {
                return Some(h.top().max(0.0));
            }
            let i = ids
                .iter()
                .position(|id| app.session.catalog.photo(*id).and_then(|p| p.captured.as_deref()).is_some_and(|c| c.starts_with(d.as_str())))?;
            lay.cells.get(i).map(|c| (c.top() - HEADER_H - 8.0).max(0.0))
        }
    }
}

/// Memory key of a [`DragChoose`] in progress.
pub(super) const DRAG_CHOOSE: &str = "grid-drag-choose";

/// A sideways drag over the phone's grid while choosing photos.
#[derive(Clone, Debug)]
pub(super) struct DragChoose {
    /// Where the drag began (index into the grid's photos).
    start: usize,
    /// Choosing (the first photo wasn't chosen) or unchoosing.
    adding: bool,
    /// What was chosen before the drag.
    base: Vec<u64>,
    /// The photo under the finger last frame.
    last: Option<usize>,
}

/// Choosing photos by touch, as iOS Photos does: a drag that sets off sideways from a photo
/// chooses the run of photos from it to the one under the finger (or unchooses them when the
/// first was chosen); an up or down drag scrolls instead.
pub(super) fn drag_choose(app: &mut LightcraftApp, ui: &egui::Ui, ids: &[PhotoId], lay: &GridLayout, origin: egui::Pos2) {
    let key = egui::Id::unique(DRAG_CHOOSE);
    let (down, press, pos) = ui.input(|i| (i.pointer.primary_down(), i.pointer.press_origin(), i.pointer.interact_pos()));
    let at = |p: egui::Pos2| cell_at(lay, origin, p);
    let mut state: Option<DragChoose> = ui.data(|d| d.get_temp(key));
    if !down {
        if state.is_some() {
            ui.data_mut(|d| d.remove::<DragChoose>(key));
        }
        return;
    }
    if state.is_none()
        && let (Some(o), Some(p)) = (press, pos)
    {
        let d = p - o;
        if d.x.abs() >= 12.0
            && d.x.abs() >= 2.0 * d.y.abs()
            && let Some(start) = at(o)
            && let Some(first) = ids.get(start)
        {
            let adding = !app.session.selection.contains(*first);
            state = Some(DragChoose { start, adding, base: app.session.selection.ids.iter().map(|i| i.0).collect(), last: None });
        }
    }
    let Some(mut s) = state else { return };
    if let Some(cur) = pos.and_then(at)
        && s.last != Some(cur)
    {
        s.last = Some(cur);
        haptics::tap(ui.ctx(), Haptic::Selection);
        // (the run on top of what was chosen before is the engine's, shared with native frontends)
        let _ = app.run("library.chooseRun", json!({"base": s.base, "from": s.start, "to": cur, "adding": s.adding}));
    }
    ui.data_mut(|d| d.insert_temp(key, s));
}

/// The photo (index into the grid's photos) whose cell holds screen point `p`; `origin` is where the
/// grid's content starts on screen.
pub(super) fn cell_at(lay: &GridLayout, origin: egui::Pos2, p: egui::Pos2) -> Option<usize> {
    let local = p - origin.to_vec2();
    rows_between(&lay.rows, local.y, local.y).iter().flat_map(|r| r.start..r.end).find(|i| lay.cells.get(*i).is_some_and(|c| c.contains(local)))
}

/// Memory key: a long press was answered during the press in progress (its release is not a tap).
const LONG_PRESS: &str = "grid-long-press";

/// How long a finger rests on a photo before choosing starts: iOS's long press.
pub const LONG_PRESS_SECS: f64 = 0.5;

/// How far (points) a finger may wander and still be holding still.
const LONG_PRESS_SLOP: f32 = 10.0;

/// Whether the press in progress (or the one just released) was a long press: its tap is not one.
pub(super) fn long_pressed(ui: &egui::Ui) -> bool {
    ui.data(|d| d.get_temp::<bool>(egui::Id::unique(LONG_PRESS))).unwrap_or(false)
}

/// A long press on a phone's photo, answered while the finger is still down, as iOS Photos does:
/// it starts choosing photos with that one (while choosing, it chooses or unchooses it), and a drag
/// that follows, in any direction, chooses the run to the photo under the finger (`drag_choose`).
/// egui's own long touch waits 0.8 s, needs a touch screen, and fires only if a frame happens to
/// run then; this one asks for that frame, and a mouse held down works the same.
pub(super) fn long_press(app: &mut LightcraftApp, ui: &egui::Ui, ids: &[PhotoId], lay: &GridLayout, origin: egui::Pos2) {
    let key = egui::Id::unique(LONG_PRESS);
    let (pressed, down, press, start, pos, now, touches) = ui.input(|i| {
        (
            i.pointer.any_pressed(),
            i.pointer.primary_down(),
            i.pointer.press_origin(),
            i.pointer.press_start_time(),
            i.pointer.interact_pos(),
            i.time,
            i.multi_touch().is_some(),
        )
    });
    if pressed {
        ui.data_mut(|d| d.remove::<bool>(key));
    }
    if !down || touches || long_pressed(ui) {
        return;
    }
    let (Some(o), Some(start), Some(p)) = (press, start, pos) else { return };
    // (a finger held on the scroll handle is scrubbing, not pressing the photo under it)
    if (p - o).length() > LONG_PRESS_SLOP || scrubber_hit(ui).is_some_and(|r| r.contains(o)) {
        return;
    }
    let Some(i) = cell_at(lay, origin, o) else { return };
    let Some(&id) = ids.get(i) else { return };
    let held = now - start;
    if held < LONG_PRESS_SECS {
        // nothing else may come in while the finger rests: wake up when the time is up
        ui.ctx().request_repaint_after(std::time::Duration::try_from_secs_f64(LONG_PRESS_SECS - held).unwrap_or_default());
        return;
    }
    ui.data_mut(|d| d.insert_temp(key, true));
    haptics::tap(ui.ctx(), Haptic::Medium);
    let base: Vec<u64> = if app.ui.select_mode { app.session.selection.ids.iter().map(|p| p.0).collect() } else { Vec::new() };
    let adding = !(app.ui.select_mode && app.session.selection.contains(id));
    if app.ui.select_mode {
        let _ = app.run("library.select", json!({"ids": [id.0], "mode": "toggle"}));
    } else {
        let _ = app.run("view.selectMode", json!({"on": true, "id": id.0}));
    }
    ui.data_mut(|d| d.insert_temp(egui::Id::unique(DRAG_CHOOSE), DragChoose { start: i, adding, base, last: Some(i) }));
}

/// The scroll handle's size (points): a capsule at the grid's right edge, as Lightroom's mobile app
/// and iOS's long lists have it.
const SCRUB_W: f32 = 6.0;

const SCRUB_H: f32 = 48.0;

/// How long the handle stays after the grid stops moving (s).
const SCRUB_LINGER: f64 = 1.4;

/// The scroll handle between frames: the offset last seen, when the grid last moved, and where on
/// the handle the finger holds it (while dragged).
#[derive(Clone, Copy, Debug)]
struct Scrub {
    offset: f32,
    moved_at: f64,
    grab: Option<f32>,
}

impl Default for Scrub {
    fn default() -> Self {
        // (a grid that hasn't moved since it was first drawn shows no handle)
        Scrub { offset: 0.0, moved_at: f64::NEG_INFINITY, grab: None }
    }
}

fn scrub_id() -> egui::Id {
    egui::Id::unique("grid-scrubber")
}

/// Where the scroll handle can be taken hold of (last frame), while it shows.
fn scrubber_hit(ui: &egui::Ui) -> Option<Rect> {
    ui.data(|d| d.get_temp::<Option<Rect>>(scrub_id().with("hit"))).flatten()
}

/// The phone grid's scroll handle (widget `grid:scrubber`): it shows while the grid moves and for a
/// moment after, and dragging it runs through the whole library at once, the month of the photos
/// at the top of the screen in a bubble beside it, as iOS's Photos and Lightroom's mobile app do.
/// It drives egui's own scroll state (`scroll_with_delta_animation`, which also stops a fling), so
/// a tap on the top bar or the status bar still slides back to the top. It only takes a finger on
/// the handle itself, never on the photos at the grid's edge.
pub(super) fn scrubber(app: &LightcraftApp, ui: &mut egui::Ui, viewport: Rect, content_h: f32, ids: &[PhotoId], lay: &GridLayout) {
    let t = Tokens::get(ui.ctx());
    let key = scrub_id();
    let clip = ui.clip_rect();
    let max = content_h - viewport.height();
    let mut st: Scrub = ui.data(|d| d.get_temp(key)).unwrap_or_default();
    let now = ui.input(|i| i.time);
    // nothing to run through: a screen or two of photos scroll well enough by hand
    if max < viewport.height() || !max.is_finite() {
        ui.data_mut(|d| {
            d.insert_temp(key, Scrub::default());
            d.insert_temp(key.with("hit"), None::<Rect>);
        });
        return;
    }
    if (viewport.top() - st.offset).abs() > 0.5 {
        (st.offset, st.moved_at) = (viewport.top(), now);
    }
    // (it stops above the home indicator, which the grid runs on under)
    let bottom = clip.bottom().min(ui.ctx().content_rect().bottom());
    let travel = (clip.top() + 8.0)..=(bottom - 8.0 - SCRUB_H).max(clip.top() + 8.0);
    let span = (travel.end() - travel.start()).max(1.0);
    let y = travel.start() + (viewport.top() / max).clamp(0.0, 1.0) * span;
    let lingering = now - st.moved_at < SCRUB_LINGER;
    let shown = ui.ctx().animate_bool_with_time(key.with("shown"), lingering || st.grab.is_some(), 0.2);
    if lingering {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
    }
    let hit = Rect::from_min_max(pos2(clip.right() - 44.0, y - 12.0), pos2(clip.right(), y + SCRUB_H + 12.0));
    let live = shown > 0.3 || st.grab.is_some();
    if live {
        let resp = ui.interact(hit, key.with("drag"), Sense::drag());
        register(ui, "grid:scrubber", hit);
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ScrollBar, true, crate::i18n::tr("Scroll")));
        let pointer = ui.input(|i| i.pointer.interact_pos());
        if resp.drag_started()
            && let Some(p) = pointer
        {
            st.grab = Some(p.y - y);
            haptics::tap(ui.ctx(), Haptic::Selection);
        }
        if resp.dragged()
            && let (Some(p), Some(g)) = (pointer, st.grab)
        {
            let top = (p.y - g).clamp(*travel.start(), *travel.end());
            let want = (top - travel.start()) / span * max;
            // (an instant scroll by the difference: egui keeps the offset, and a fling still under
            // way is stopped)
            ui.scroll_with_delta_animation(vec2(0.0, viewport.top() - want), egui::style::ScrollAnimation::none());
            st.moved_at = now;
        }
        if resp.drag_stopped() || !ui.input(|i| i.pointer.primary_down()) {
            st.grab = None;
        }
    } else {
        st.grab = None;
    }
    ui.data_mut(|d| {
        d.insert_temp(key, st);
        d.insert_temp(key.with("hit"), live.then_some(hit));
    });
    if shown <= 0.0 {
        return;
    }
    // the handle: a capsule that widens while held
    let held = ui.ctx().animate_bool_with_time(key.with("held"), st.grab.is_some(), 0.15);
    let w = SCRUB_W + 3.0 * held;
    let handle = Rect::from_min_size(pos2(clip.right() - 5.0 - w, y), vec2(w, SCRUB_H));
    let p = ui.painter();
    let ink = if crate::glass::is_dark(ui.ctx()) { Color32::WHITE } else { Color32::from_gray(30) };
    p.rect_filled(handle.expand(1.0), w / 2.0 + 1.0, Color32::from_black_alpha((60.0 * shown) as u8));
    p.rect_filled(handle, w / 2.0, ink.gamma_multiply(0.85 * shown));
    // while held: the month at the top of the screen beside it
    if held > 0.0
        && let Some(row) = rows_between(&lay.rows, viewport.top(), viewport.top() + 1.0).first()
        && let Some(captured) = ids.get(row.start).and_then(|id| app.session.catalog.photo(*id)).and_then(|p| p.captured.clone())
        && let Some(month) = captured.get(..7)
    {
        let label = crate::i18n::date_group_label(month, false);
        let g = p.layout_no_wrap(label, t.semibold(15.0), t.text);
        let bubble = Rect::from_min_size(pos2(handle.left() - 14.0 - g.size().x - 28.0, handle.center().y - 18.0), vec2(g.size().x + 28.0, 36.0));
        let mut p = p.clone().with_layer_id(egui::LayerId::new(egui::Order::Foreground, key.with("bubble")));
        p.set_opacity(held);
        crate::glass::paint(&p, bubble, 18.0, None, 0.0);
        p.galley(bubble.center() - g.size() / 2.0, g, t.text);
    }
}

/// The part of a `w` × `h` texture that fills a square (the middle of its long side).
pub fn cover_uv(w: f32, h: f32) -> Rect {
    let (w, h) = (w.max(1.0), h.max(1.0));
    if w > h {
        let k = h / w;
        Rect::from_min_max(pos2((1.0 - k) / 2.0, 0.0), pos2((1.0 + k) / 2.0, 1.0))
    } else {
        let k = w / h;
        Rect::from_min_max(pos2(0.0, (1.0 - k) / 2.0), pos2(1.0, (1.0 + k) / 2.0))
    }
}

/// Choosing photos by touch: a circle at the cell's top right, filled with a check when chosen.
pub(super) fn check_badge(ui: &egui::Ui, img: Rect, selected: bool, t: &Tokens, id: egui::Id) {
    let p = ui.painter();
    let c = pos2(img.right() - 15.0, img.top() + 15.0);
    // the circle fills and the tick pops in (and back out) as the photo is chosen
    let k = ui.ctx().animate_bool_with_time(id, selected, 0.18);
    p.circle(c, 10.0, Color32::from_black_alpha(70).lerp_to_gamma(t.accent, k), Stroke::new(1.5, Color32::WHITE));
    if k > 0.0 {
        p.rect_stroke(img, 0.0, Stroke::new(3.0 * k, t.accent.gamma_multiply(k)), StrokeKind::Inside);
        let s = k * (1.0 + 0.25 * (k * std::f32::consts::PI).sin());
        let tick = [c + vec2(-4.5, 0.0) * s, c + vec2(-1.5, 3.5) * s, c + vec2(4.5, -3.5) * s];
        let stroke = Stroke::new(2.0, Color32::WHITE.gamma_multiply(k));
        p.line_segment([tick[0], tick[1]], stroke);
        p.line_segment([tick[1], tick[2]], stroke);
    }
}
