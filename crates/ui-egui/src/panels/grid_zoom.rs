//! Pinching the phone's grid, as iOS's Photos and Lightroom's mobile app do: spreading two fingers
//! makes the photos bigger (fewer across), pinching makes them smaller (more across). While the
//! fingers move, the grid is drawn larger or smaller about the point between them, so the photo under
//! them stays under them; whenever the size passes from one number of columns to the next, the grid
//! is laid out again for it and every photo glides from where it was to its new place; when the
//! fingers lift the grid settles to its size. A change of columns by command (`view.gridColumns`)
//! glides the same way. The grid (`grid.rs`) asks [`update`] once a frame, before laying out, and
//! draws each photo where [`Zoom::place`] says.

use std::collections::HashMap;

use egui::{Pos2, Rect, Vec2};

use crate::LightcraftApp;
use crate::haptics::{self, Haptic};

/// The photos across a row the phone's grid offers.
pub const STEPS: [u8; 8] = [1, 2, 3, 4, 5, 7, 10, 14];
/// How long photos glide to their new places, and the grid settles after a pinch (s).
const GLIDE_S: f64 = 0.3;
const SETTLE_S: f64 = 0.25;
/// How far past the midway (in log scale) a pinch goes before the next number of columns is taken,
/// so that fingers resting near the boundary don't flicker between two.
const HYSTERESIS: f32 = 0.06;

/// `cols` as the nearest number the grid offers.
pub fn snap(cols: u8) -> u8 {
    nearest(f32::from(cols.max(1)))
}

/// The number of columns `by` steps from `cols` (positive: more, smaller photos).
pub fn step(cols: u8, by: i64) -> u8 {
    let at = STEPS.iter().position(|s| *s == snap(cols)).unwrap_or(2);
    let to = (at as i64).saturating_add(by).clamp(0, STEPS.len() as i64 - 1);
    STEPS.get(usize::try_from(to).unwrap_or(0)).copied().unwrap_or(3)
}

fn nearest(c: f32) -> u8 {
    let c = if c.is_finite() { c.max(0.1) } else { 3.0 };
    STEPS.iter().copied().min_by(|a, b| (f32::from(*a).ln() - c.ln()).abs().total_cmp(&(f32::from(*b).ln() - c.ln()).abs())).unwrap_or(3)
}

/// The columns for `c` (continuous, from the fingers) when `cur` is shown: the nearest step, but
/// only once `c` is clearly nearer to it than to `cur`.
pub fn columns_for(c: f32, cur: u8) -> u8 {
    let n = nearest(c);
    let far = |s: u8| (f32::from(s).ln() - c.max(0.1).ln()).abs();
    if n != cur && far(n) + HYSTERESIS < far(cur) { n } else { cur }
}

/// A pinch in progress (or settling after the fingers lifted).
#[derive(Clone, Copy, Debug)]
struct Pinch {
    /// The columns it began from (as a continuous value: a pinch that starts while the last one
    /// settles starts from what is shown), and how much the fingers have spread since (> 1: apart).
    from: f32,
    scale: f32,
    /// The point between the fingers (screen).
    focal: Pos2,
    /// How much bigger than its layout the grid is drawn.
    v: f32,
    /// The fingers lifted at this time, the grid then drawn `v` times its size.
    lifted: Option<(f64, f32)>,
}

/// Where every photo was drawn last frame (index into the grid's photos → screen rect), with the
/// view's generation and columns, for gliding from there when the columns change.
#[derive(Clone, Debug, Default)]
struct Drawn {
    generation: u64,
    columns: u8,
    rects: Vec<(usize, Rect)>,
}

/// Photos gliding from where they were to their new places.
#[derive(Clone, Debug)]
struct Glide {
    generation: u64,
    from: HashMap<usize, Rect>,
    since: f64,
}

fn pinch_id() -> egui::Id {
    egui::Id::new("grid-pinch")
}
fn drawn_id() -> egui::Id {
    egui::Id::new("grid-drawn")
}
fn glide_id() -> egui::Id {
    egui::Id::new("grid-glide")
}

/// What a frame of the grid draws differently because of a pinch or a change of columns.
#[derive(Clone, Debug)]
pub struct Zoom {
    /// The grid is drawn this many times its layout's size about `focal`.
    pub v: f32,
    pub focal: Pos2,
    /// The columns changed since the last frame: scroll so that the photo under `focal` stays.
    pub changed: bool,
    /// The photo under `focal` last frame, and where in it `focal` was (0..1 each way).
    pub anchor: Option<(usize, Vec2)>,
    /// Photos gliding: where each was, and how far along they are (0..1, eased).
    glide: Option<(HashMap<usize, Rect>, f32)>,
    /// A finger pinch is under way (the grid must not scroll with the first finger meanwhile).
    pub pinching: bool,
}

impl Zoom {
    /// Nothing in play.
    pub fn none() -> Zoom {
        Zoom { v: 1.0, focal: Pos2::ZERO, changed: false, anchor: None, glide: None, pinching: false }
    }

    /// Whether the grid is drawn other than at its layout's places (date headers stay hidden).
    pub fn moving(&self) -> bool {
        (self.v - 1.0).abs() > 1e-3 || self.glide.is_some()
    }

    /// Where photo `i`, at `r` in the layout (screen), is drawn this frame.
    pub fn place(&self, i: usize, r: Rect) -> Rect {
        let r = scale_about(r, self.focal, self.v);
        match &self.glide {
            Some((from, k)) => match from.get(&i) {
                Some(f) => lerp_rect(*f, r, *k),
                None => r,
            },
            None => r,
        }
    }

    /// The photos that were drawn and are gliding (their old places), for drawing the ones that leave
    /// the screen on their way out.
    pub fn gliding(&self) -> impl Iterator<Item = usize> + '_ {
        self.glide.iter().flat_map(|(from, _)| from.keys().copied())
    }

    /// The part of the layout (`viewport`, the grid's own coordinates, `origin` where they start on
    /// screen) that shows on screen this frame: more of it when the grid is drawn smaller.
    pub fn visible(&self, viewport: Rect, origin: Pos2) -> Rect {
        if self.v >= 1.0 || self.v <= 0.0 {
            return viewport;
        }
        let f = self.focal - origin.to_vec2();
        scale_about(viewport, f, 1.0 / self.v).union(viewport)
    }
}

fn scale_about(r: Rect, c: Pos2, k: f32) -> Rect {
    if (k - 1.0).abs() < 1e-4 {
        return r;
    }
    Rect::from_min_max(c + (r.min - c) * k, c + (r.max - c) * k)
}

fn lerp_rect(a: Rect, b: Rect, k: f32) -> Rect {
    Rect::from_min_max(a.min.lerp(b.min, k), a.max.lerp(b.max, k))
}

/// Read this frame's pinch over `area` (the grid on screen) and move `app.ui.grid_columns` with it.
/// Call once a frame before laying the grid out; `generation` is the view's (gliding is dropped when
/// the photos change).
pub fn update(app: &mut LightcraftApp, ui: &egui::Ui, area: Rect, generation: u64) -> Zoom {
    let ctx = ui.ctx();
    let now = ui.input(|i| i.time);
    let (multi, delta, hover) = ui.input(|i| (i.multi_touch(), i.zoom_delta(), i.pointer.hover_pos()));
    let mut pinch: Option<Pinch> = ctx.data(|d| d.get_temp(pinch_id()));
    let drawn: Drawn = ctx.data(|d| d.get_temp(drawn_id())).unwrap_or_default();
    let cur = snap(app.ui.grid_columns);
    app.ui.grid_columns = cur;
    let focal = multi.map(|m| m.center_pos).or(hover).filter(|p| area.contains(*p));
    let fingers = multi.is_some();
    let zooming = (delta - 1.0).abs() > 1e-4 && delta.is_finite() && delta > 0.0;
    let mut zoom = Zoom::none();
    zoom.focal = pinch.map_or(area.center(), |p| p.focal);
    if (fingers || zooming)
        && let Some(f) = focal.or(pinch.map(|p| p.focal))
    {
        let p = match pinch {
            // (a new pinch while the last settles starts from what is shown)
            Some(p) if p.lifted.is_none() => p,
            Some(p) => Pinch { from: f32::from(cur) / p.v.max(0.05), scale: 1.0, focal: f, v: p.v, lifted: None },
            None => Pinch { from: f32::from(cur), scale: 1.0, focal: f, v: 1.0, lifted: None },
        };
        let scale = (p.scale * if zooming { delta } else { 1.0 }).clamp(0.05, 20.0);
        let c = (p.from / scale).clamp(0.5, 20.0);
        let next = columns_for(c, cur);
        if next != cur {
            app.ui.grid_columns = next;
            haptics::tap(ctx, Haptic::Selection);
        }
        let v = (f32::from(next) / c).clamp(0.35, 2.8);
        pinch = Some(Pinch { from: p.from, scale, focal: f, v, lifted: None });
        zoom.v = v;
        zoom.focal = f;
        zoom.pinching = fingers;
    } else if let Some(mut p) = pinch {
        // the fingers are up: the grid settles to its layout's size about the same point
        let (t0, v0) = *p.lifted.get_or_insert((now, p.v));
        let k = (((now - t0) / SETTLE_S) as f32).clamp(0.0, 1.0);
        p.v = v0 + (1.0 - v0) * egui::emath::easing::cubic_out(k);
        zoom.v = p.v;
        zoom.focal = p.focal;
        pinch = (k < 1.0).then_some(p);
        ctx.request_repaint();
    }
    ctx.data_mut(|d| match pinch {
        Some(p) => {
            d.insert_temp(pinch_id(), p);
        }
        None => d.remove::<Pinch>(pinch_id()),
    });
    // a change of columns (by the fingers or a command): glide from where the photos were drawn
    let mut glide: Option<Glide> = ctx.data(|d| d.get_temp(glide_id()));
    if drawn.generation == generation && drawn.columns != 0 && drawn.columns != app.ui.grid_columns {
        zoom.changed = true;
        if pinch.is_none() {
            zoom.focal = area.center();
        }
        zoom.anchor = anchor(&drawn.rects, zoom.focal);
        glide = Some(Glide { generation, from: drawn.rects.iter().copied().collect(), since: now });
    }
    if let Some(g) = glide.take().filter(|g| g.generation == generation) {
        let k = ((now - g.since) / GLIDE_S) as f32;
        if (0.0..1.0).contains(&k) {
            zoom.glide = Some((g.from.clone(), egui::emath::easing::cubic_out(k)));
            glide = Some(g);
            ctx.request_repaint();
        }
    }
    ctx.data_mut(|d| match glide {
        Some(g) => {
            d.insert_temp(glide_id(), g);
        }
        None => d.remove::<Glide>(glide_id()),
    });
    zoom
}

/// A pinch on a desktop trackpad (or ⌘ / Ctrl + wheel) over the grid at `area`: the thumbnails grow
/// as the fingers spread and shrink as they close, the photo under the pointer staying under it.
/// `cells` and `scroll` are last frame's layout (the grid's own coordinates) and scroll offset.
pub fn pinch_thumbs(app: &mut LightcraftApp, ui: &egui::Ui, area: Rect, cells: &[Rect], scroll: f32) -> Zoom {
    let mut zoom = Zoom::none();
    let (delta, hover) = ui.input(|i| (i.zoom_delta(), i.pointer.hover_pos()));
    let Some(at) = hover.filter(|p| area.contains(*p)) else { return zoom };
    // (the slider's range)
    let size = (app.ui.thumb_size * delta).clamp(90.0, 480.0);
    if !delta.is_finite() || delta <= 0.0 || (size - app.ui.thumb_size).abs() < 1e-3 {
        return zoom;
    }
    app.ui.thumb_size = size;
    let shift = egui::vec2(area.left(), area.top() - scroll);
    let shown: Vec<(usize, Rect)> =
        cells.iter().enumerate().map(|(i, c)| (i, c.translate(shift))).filter(|(_, r)| r.intersects(area)).collect();
    (zoom.changed, zoom.focal, zoom.anchor) = (true, at, anchor(&shown, at));
    zoom
}

/// The photo drawn under `focal` (else the nearest), and where in it `focal` is.
fn anchor(rects: &[(usize, Rect)], focal: Pos2) -> Option<(usize, Vec2)> {
    let (i, r) = rects
        .iter()
        .find(|(_, r)| r.contains(focal))
        .or_else(|| rects.iter().min_by(|a, b| a.1.center().distance_sq(focal).total_cmp(&b.1.center().distance_sq(focal))))?;
    let u = (focal - r.min) / r.size().max(egui::vec2(1.0, 1.0));
    Some((*i, egui::vec2(u.x.clamp(0.0, 1.0), u.y.clamp(0.0, 1.0))))
}

/// The scroll offset that puts point `u` of the cell `cell` (the new layout's own coordinates) at
/// `focal` on screen, the grid's content starting at `top` on screen.
pub fn offset_for(cell: Rect, u: Vec2, focal: Pos2, top: f32) -> f32 {
    (cell.top() + u.y * cell.height() - (focal.y - top)).max(0.0)
}

/// Remember where the photos were drawn this frame (for the next change of columns).
pub fn remember(ctx: &egui::Context, generation: u64, columns: u8, rects: Vec<(usize, Rect)>) {
    ctx.data_mut(|d| d.insert_temp(drawn_id(), Drawn { generation, columns, rects }));
}

/// The grid isn't drawn as a pinchable grid this frame (another span, the desktop): nothing glides
/// into it when it comes back.
pub fn forget(ctx: &egui::Context) {
    ctx.data_mut(|d| {
        d.remove::<Drawn>(drawn_id());
        d.remove::<Glide>(glide_id());
        d.remove::<Pinch>(pinch_id());
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::pos2;

    #[test]
    fn columns_snap_step_and_hold_near_the_boundary() {
        assert_eq!((snap(0), snap(3), snap(5), snap(6), snap(9), snap(200)), (1, 3, 5, 7, 10, 14));
        assert_eq!((step(3, 1), step(3, -1), step(1, -1), step(14, 1), step(3, i64::MIN)), (4, 2, 1, 14, 1));
        // halfway (in log scale) between 3 and 4 is about 3.46: nothing changes until clearly past it
        assert_eq!(columns_for(3.5, 3), 3);
        assert_eq!(columns_for(3.9, 3), 4);
        assert_eq!(columns_for(1.0, 3), 1);
        assert_eq!(columns_for(f32::NAN, 3), 3);
    }

    #[test]
    fn the_anchor_keeps_its_point_under_the_fingers() {
        let rects = [
            (0, Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(100.0, 100.0))),
            (1, Rect::from_min_size(pos2(100.0, 0.0), egui::vec2(100.0, 100.0))),
        ];
        let (i, u) = anchor(&rects, pos2(150.0, 25.0)).unwrap();
        assert_eq!((i, u), (1, egui::vec2(0.5, 0.25)));
        // the new cell is at 400..600 in the content, the grid starts at y 50 on screen: a quarter down
        // the cell (450) must land on the fingers at y 75
        let cell = Rect::from_min_size(pos2(0.0, 400.0), egui::vec2(200.0, 200.0));
        assert_eq!(offset_for(cell, u, pos2(150.0, 75.0), 50.0), 425.0);
        assert_eq!(offset_for(cell, egui::vec2(0.0, 0.0), pos2(0.0, 900.0), 0.0), 0.0, "never above the top");
    }
}
