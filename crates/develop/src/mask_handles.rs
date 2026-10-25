//! Masks on the photo: where a component's handles and pin are, what a press takes and the shape a
//! drag leaves. Shared by the egui loupe and the engine's `mask.handles` / `mask.grab` /
//! `mask.drag` (native frontends). Points are normalized image coordinates; a radial mask's radii are
//! in long-edge units, so the functions take `long`: normalized units per long-edge unit in x and y
//! ([`long_units`]).

use crate::{Mask, MaskShape};
use lightcraft_geom::Point;

/// Normalized units per long-edge unit, in x and y, for a photo `w` × `h` (as oriented).
pub fn long_units(w: f64, h: f64) -> (f64, f64) {
    let (w, h) = (if w.is_finite() { w.max(1e-9) } else { 1.0 }, if h.is_finite() { h.max(1e-9) } else { 1.0 });
    let l = w.max(h);
    (l / w, l / h)
}

/// How far the rotation grip sits outside a radial's edge (long-edge units).
const ROTATE_ARM: f64 = 0.08;

/// A radial's point at `a` (radians, in its own frame) `r` long-edge units out along `rx`/`ry`.
fn radial_at(center: Point, rx: f64, ry: f64, angle: f64, a: f64, long: (f64, f64)) -> Point {
    let (s, co) = angle.to_radians().sin_cos();
    let (x, y) = (rx * a.cos(), ry * a.sin());
    Point::new(center.x + (x * co - y * s) * long.0, center.y + (x * s + y * co) * long.1)
}

/// The handles of a selected component: a radial's edge at 0°, 90°, 180° and 270° of its own frame
/// (1–4: 1 and 3 set `rx`, 2 and 4 `ry`) and its rotation grip (5); a linear gradient's start (1) and
/// end (2). Other shapes have none.
pub fn grips(shape: &MaskShape, long: (f64, f64)) -> Vec<(u8, Point)> {
    match shape {
        MaskShape::Radial { center, rx, ry, angle, .. } => {
            let mut out: Vec<(u8, Point)> =
                (0..4u8).map(|i| (i + 1, radial_at(*center, *rx, *ry, *angle, f64::from(i) * std::f64::consts::FRAC_PI_2, long))).collect();
            out.push((5, radial_at(*center, rx + ROTATE_ARM, 0.0, *angle, 0.0, long)));
            out
        }
        MaskShape::Linear { start, end } => vec![(1, *start), (2, *end)],
        _ => Vec::new(),
    }
}

/// A radial's ellipse as `n` points (empty for other shapes).
pub fn outline(shape: &MaskShape, long: (f64, f64), n: usize) -> Vec<Point> {
    let MaskShape::Radial { center, rx, ry, angle, .. } = shape else { return Vec::new() };
    (0..n).map(|i| radial_at(*center, *rx, *ry, *angle, i as f64 / n as f64 * std::f64::consts::TAU, long)).collect()
}

/// Where a component's pin sits, for shapes that have a place on the photo.
pub fn pin(shape: &MaskShape) -> Option<Point> {
    match shape {
        MaskShape::Radial { center, .. } => Some(*center),
        MaskShape::Linear { start, end } => Some(Point::new((start.x + end.x) / 2.0, (start.y + end.y) / 2.0)),
        MaskShape::Brush { strokes } => strokes.iter().find(|s| !s.erase).and_then(|s| s.points.first()).copied(),
        _ => None,
    }
}

/// Whether `at` is inside a radial's ellipse (whatever its feather or inversion).
pub fn body_contains(shape: &MaskShape, at: Point, long: (f64, f64)) -> bool {
    let MaskShape::Radial { center, rx, ry, angle, .. } = shape else { return false };
    if !rx.is_finite() || !ry.is_finite() || *rx <= 0.0 || *ry <= 0.0 {
        return false;
    }
    let dx = (at.x - center.x) / long.0;
    let dy = (at.y - center.y) / long.1;
    let (s, co) = angle.to_radians().sin_cos();
    let x = (dx * co + dy * s) / rx;
    let y = (-dx * s + dy * co) / ry;
    x * x + y * y <= 1.0
}

/// `shape` dragged from `from` to `at` by `handle` (0 = the whole component, see [`grips`]); `alt`
/// keeps a radial's proportions while resizing. Radii snap to a circle within 5 %.
pub fn moved(shape: &MaskShape, handle: u8, from: Point, at: Point, long: (f64, f64), alt: bool) -> MaskShape {
    let mv = |p: Point| Point::new(p.x + at.x - from.x, p.y + at.y - from.y);
    // A handle follows the pointer by how far it moved, not to where it is: a press lands up to the reach
    // (a finger's width) off the handle's centre, and the handle must not jump that far at the first move.
    let at = match grips(shape, long).into_iter().find(|(h, _)| *h == handle && handle > 0) {
        Some((_, grip)) => Point::new(at.x + grip.x - from.x, at.y + grip.y - from.y),
        None => at,
    };
    match shape.clone() {
        MaskShape::Radial { center, rx, ry, angle, feather, invert } => {
            let dx = (at.x - center.x) / long.0;
            let dy = (at.y - center.y) / long.1;
            let (s, co) = angle.to_radians().sin_cos();
            match handle {
                1 | 3 => {
                    let new_rx = (dx * co + dy * s).abs().max(0.01);
                    let mut new_ry = if alt { ry * (new_rx / rx.max(0.01)) } else { ry };
                    if (new_rx - new_ry).abs() / new_rx.max(new_ry) < 0.05 {
                        new_ry = new_rx;
                    }
                    MaskShape::Radial { center, rx: new_rx, ry: new_ry, angle, feather, invert }
                }
                2 | 4 => {
                    let new_ry = (-dx * s + dy * co).abs().max(0.01);
                    let mut new_rx = if alt { rx * (new_ry / ry.max(0.01)) } else { rx };
                    if (new_rx - new_ry).abs() / new_rx.max(new_ry) < 0.05 {
                        new_rx = new_ry;
                    }
                    MaskShape::Radial { center, rx: new_rx, ry: new_ry, angle, feather, invert }
                }
                5 => MaskShape::Radial { center, rx, ry, angle: dy.atan2(dx).to_degrees(), feather, invert },
                _ => MaskShape::Radial { center: mv(center), rx, ry, angle, feather, invert },
            }
        }
        MaskShape::Linear { start, end } => match handle {
            1 => MaskShape::Linear { start: at, end },
            2 => MaskShape::Linear { start, end: at },
            _ => MaskShape::Linear { start: mv(start), end: mv(end) },
        },
        MaskShape::Brush { mut strokes } => {
            for st in &mut strokes {
                st.points.iter_mut().for_each(|p| *p = mv(*p));
            }
            MaskShape::Brush { strokes }
        }
        s => s,
    }
}

/// Something on the photo a press can take: a mask's component and its handle (0 = the pin, or the
/// whole component).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grip {
    pub mask: u32,
    pub component: usize,
    pub handle: u8,
    pub at: Point,
}

/// Everything that can be grabbed: the selected mask's handles, and with `pins` every component's
/// pin.
pub fn all_grips(masks: &[Mask], active: Option<u32>, long: (f64, f64), pins: bool) -> Vec<Grip> {
    let mut out = Vec::new();
    for m in masks {
        for (ci, c) in m.components.iter().enumerate() {
            if Some(m.id) == active {
                out.extend(grips(&c.shape, long).into_iter().map(|(handle, at)| Grip { mask: m.id, component: ci, handle, at }));
            }
            if pins && let Some(at) = pin(&c.shape) {
                out.push(Grip { mask: m.id, component: ci, handle: 0, at });
            }
        }
    }
    out
}

/// The grip within `reach` (long-edge units) of `at`: the selected mask's first, then the nearest.
pub fn grip_at(grips: &[Grip], active: Option<u32>, at: Point, reach: f64, long: (f64, f64)) -> Option<Grip> {
    let dist = |g: &Grip| ((g.at.x - at.x) / long.0).hypot((g.at.y - at.y) / long.1);
    grips
        .iter()
        .filter(|g| dist(g) < reach)
        .min_by(|a, b| (Some(a.mask) != active, dist(a)).partial_cmp(&(Some(b.mask) != active, dist(b))).unwrap_or(std::cmp::Ordering::Equal))
        .copied()
}

/// What a drag starting at `at` takes: a grip within `reach`, else the selected mask's radial under
/// it, else (anywhere on the photo) the selected mask's linear gradient.
pub fn grab(masks: &[Mask], active: Option<u32>, at: Point, reach: f64, long: (f64, f64), pins: bool) -> Option<Grip> {
    if let Some(g) = grip_at(&all_grips(masks, active, long, pins), active, at, reach, long) {
        return Some(g);
    }
    let m = masks.iter().find(|m| Some(m.id) == active)?;
    if let Some(ci) = m.components.iter().position(|c| body_contains(&c.shape, at, long)) {
        return Some(Grip { mask: m.id, component: ci, handle: 0, at });
    }
    matches!(m.components.first()?.shape, MaskShape::Linear { .. }).then_some(Grip { mask: m.id, component: 0, handle: 0, at })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn radial() -> MaskShape {
        MaskShape::Radial { center: Point::new(0.5, 0.5), rx: 0.2, ry: 0.1, angle: 0.0, feather: 50.0, invert: false }
    }

    #[test]
    fn handles_resize_rotate_and_move_a_radial() {
        let long = long_units(3000.0, 2000.0);
        assert_eq!(long, (1.0, 1.5));
        let g = grips(&radial(), long);
        assert_eq!(g.len(), 5);
        // handle 1 sits on the right edge, rx out in long-edge units
        assert!((g[0].1.x - 0.7).abs() < 1e-9 && (g[0].1.y - 0.5).abs() < 1e-9);
        let wider = moved(&radial(), 1, g[0].1, Point::new(0.8, 0.5), long, false);
        assert!(matches!(wider, MaskShape::Radial { rx, ry, .. } if (rx - 0.3).abs() < 1e-9 && (ry - 0.1).abs() < 1e-9));
        let turned = moved(&radial(), 5, g[4].1, Point::new(0.5, 0.8), long, false);
        assert!(matches!(turned, MaskShape::Radial { angle, .. } if (angle - 90.0).abs() < 1e-6));
        let moved_c = moved(&radial(), 0, Point::new(0.5, 0.5), Point::new(0.6, 0.4), long, false);
        assert!(matches!(moved_c, MaskShape::Radial { center, .. } if (center.x - 0.6).abs() < 1e-9 && (center.y - 0.4).abs() < 1e-9));
        assert!(body_contains(&radial(), Point::new(0.65, 0.5), long));
        assert!(!body_contains(&radial(), Point::new(0.5, 0.7), long), "ry 0.1 long = 0.15 normalized");
    }

    /// A press off the handle's centre (a finger) moves the handle by the drag's length: it does not jump to the finger.
    #[test]
    fn a_handle_follows_the_drag_not_the_finger() {
        let long = (1.0, 1.0);
        // the right edge is at x 0.7; the finger came down 0.02 to its right, and went 0.05 further right
        let wider = moved(&radial(), 1, Point::new(0.72, 0.51), Point::new(0.77, 0.51), long, false);
        assert!(matches!(wider, MaskShape::Radial { rx, .. } if (rx - 0.25).abs() < 1e-9), "{wider:?}");
        let line = MaskShape::Linear { start: Point::new(0.2, 0.2), end: Point::new(0.2, 0.6) };
        let dragged = moved(&line, 2, Point::new(0.23, 0.58), Point::new(0.23, 0.78), long, false);
        assert!(matches!(dragged, MaskShape::Linear { end, .. } if (end.x - 0.2).abs() < 1e-9 && (end.y - 0.8).abs() < 1e-9), "{dragged:?}");
        // the rotation grip: the angle follows where the grip would be
        let turned = moved(&radial(), 5, Point::new(0.8, 0.52), Point::new(0.52, 0.82), long, false);
        assert!(matches!(turned, MaskShape::Radial { angle, .. } if (angle - 90.0).abs() < 1e-6), "{turned:?}");
    }

    #[test]
    fn a_press_takes_a_handle_then_the_body_then_a_gradient() {
        let long = (1.0, 1.0);
        let mask = |id, shape| Mask {
            id,
            name: String::new(),
            visible: true,
            invert: false,
            components: vec![crate::MaskComponent { name: None, op: Default::default(), invert: false, shape }],
            adjust: Default::default(),
            refine: 0.0,
        };
        let masks = vec![mask(1, radial()), mask(2, MaskShape::Linear { start: Point::new(0.1, 0.1), end: Point::new(0.1, 0.4) })];
        let g = grab(&masks, Some(1), Point::new(0.701, 0.5), 0.02, long, true).unwrap();
        assert_eq!((g.mask, g.component, g.handle), (1, 0, 1));
        let body = grab(&masks, Some(1), Point::new(0.55, 0.52), 0.02, long, true).unwrap();
        assert_eq!((body.mask, body.handle), (1, 0));
        // another mask's pin, with pins shown
        let other = grab(&masks, Some(1), Point::new(0.1, 0.25), 0.02, long, true).unwrap();
        assert_eq!((other.mask, other.handle), (2, 0));
        assert!(grab(&masks, Some(1), Point::new(0.9, 0.9), 0.02, long, true).is_none());
        // a selected linear gradient goes with a press anywhere
        assert_eq!(grab(&masks, Some(2), Point::new(0.9, 0.9), 0.02, long, false).map(|g| g.mask), Some(2));
    }
}
