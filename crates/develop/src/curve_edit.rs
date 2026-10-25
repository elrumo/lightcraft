//! Editing a point curve with a pointer or a finger: which point a press takes, where a new one goes, how far a
//! point can move, which points can be removed. Pure functions over the points of one channel (x, y in 0..1,
//! y up), shared by every frontend, so the egui panel and a native one pick, add, move and remove alike.
//!
//! The first and last point are the curve's ends (black and white): they move, but are never removed.

use lightcraft_color::spline::MonotoneCurve;
use lightcraft_geom::Point;

/// Closest two neighbouring points may be in x. Keeps the curve a function (monotone-safe) however a point is dragged.
pub const MIN_GAP: f64 = 0.01;

/// The points as an editor shows them: finite, in the order the curve runs, the two corners when the curve is straight.
pub fn shown(points: &[Point]) -> Vec<Point> {
    let mut v: Vec<Point> =
        points.iter().filter(|p| p.x.is_finite() && p.y.is_finite()).map(|p| Point::new(p.x.clamp(0.0, 1.0), p.y.clamp(0.0, 1.0))).collect();
    v.sort_by(|a, b| a.x.total_cmp(&b.x));
    if v.len() < 2 { vec![Point::new(0.0, 0.0), Point::new(1.0, 1.0)] } else { v }
}

/// The index of the point nearest to `(x, y)` when it is within `reach` (the graph is square, so one distance serves
/// both axes). A press at the left or bottom edge finds the black point as well as one at the middle finds its own.
pub fn nearest(points: &[Point], x: f64, y: f64, reach: f64) -> Option<usize> {
    let (best, d) =
        points.iter().enumerate().map(|(i, p)| (i, (p.x - x).hypot(p.y - y))).filter(|(_, d)| d.is_finite()).min_by(|a, b| a.1.total_cmp(&b.1))?;
    (d <= reach).then_some(best)
}

/// What a press took: the points afterwards and the index of the one to drag.
#[derive(Clone, Debug, PartialEq)]
pub struct Grab {
    pub points: Vec<Point>,
    pub index: usize,
    /// The press made a new point.
    pub added: bool,
}

/// A press at `(x, y)`: the point within `reach`, else a new point. A press near the curve adds its point on the curve
/// (so a tap changes nothing until the point is moved), one further away adds it where the press is.
pub fn grab(points: &[Point], x: f64, y: f64, reach: f64) -> Grab {
    let pts = shown(points);
    if !x.is_finite() || !y.is_finite() {
        return Grab { points: pts, index: 0, added: false };
    }
    let (x, y) = (x.clamp(0.0, 1.0), y.clamp(0.0, 1.0));
    if let Some(index) = nearest(&pts, x, y, reach) {
        return Grab { points: pts, index, added: false };
    }
    let on_curve = MonotoneCurve::new(&pts.iter().map(|p| (p.x, p.y)).collect::<Vec<_>>()).eval(x).clamp(0.0, 1.0);
    let y = if (on_curve - y).abs() <= reach { on_curve } else { y };
    let mut v = pts;
    let index = v.iter().position(|p| p.x > x).unwrap_or(v.len());
    v.insert(index, Point::new(x, y));
    // (the new point keeps its distance from the neighbours too)
    let v = move_point(&v, index, x, y);
    Grab { points: v, index, added: true }
}

/// Point `i` moved to `(x, y)`: kept inside the graph and at least [`MIN_GAP`] from its neighbours in x, so the points
/// stay in order. The ends keep their side: the first point cannot pass the second, the last cannot pass the one before.
/// Neighbours already closer than twice the gap (two taps side by side, a curve from a file) leave no room, and the
/// point then stays midway between them. `i` out of range changes nothing.
pub fn move_point(points: &[Point], i: usize, x: f64, y: f64) -> Vec<Point> {
    let mut v = points.to_vec();
    let Some(cur) = v.get(i).copied() else { return v };
    let (x, y) = if x.is_finite() && y.is_finite() { (x, y) } else { (cur.x, cur.y) };
    let lo = if i == 0 { 0.0 } else { v[i - 1].x + MIN_GAP };
    let hi = if i + 1 >= v.len() { 1.0 } else { v[i + 1].x - MIN_GAP };
    let x = if i > 0 && i + 1 < v.len() && lo > hi {
        (v[i - 1].x + v[i + 1].x) / 2.0
    } else {
        // (an end next to a neighbour that is itself at the edge: the gap cannot be kept, stay put)
        x.max(lo).min(hi).clamp(0.0, 1.0)
    };
    v[i] = Point::new(x, y.clamp(0.0, 1.0));
    v
}

/// Point `i` moved by `(dx, dy)` (a key nudge).
pub fn nudge(points: &[Point], i: usize, dx: f64, dy: f64) -> Vec<Point> {
    match points.get(i) {
        Some(p) => move_point(points, i, p.x + dx, p.y + dy),
        None => points.to_vec(),
    }
}

/// The points without `i`; `None` for an end (black and white stay) or when only two points would be left.
pub fn remove(points: &[Point], i: usize) -> Option<Vec<Point>> {
    if i == 0 || i + 1 >= points.len() || points.len() <= 2 {
        return None;
    }
    let mut v = points.to_vec();
    v.remove(i);
    Some(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: f64, y: f64) -> Point {
        Point::new(x, y)
    }

    fn straight() -> Vec<Point> {
        vec![p(0.0, 0.0), p(1.0, 1.0)]
    }

    #[test]
    fn a_press_on_the_black_point_takes_it() {
        // (the graph's left edge: a press at or beside the corner, never a new point beside it)
        for (x, y) in [(0.0, 0.0), (0.0, 0.04), (0.05, 0.0), (0.03, 0.03)] {
            let g = grab(&shown(&[]), x, y, 0.08);
            assert!(!g.added && g.index == 0 && g.points.len() == 2, "{x},{y}: {g:?}");
        }
    }

    #[test]
    fn a_press_at_the_left_edge_above_the_black_point_adds_a_point_beside_it() {
        let g = grab(&straight(), 0.0, 0.6, 0.08);
        assert!(g.added, "{g:?}");
        assert_eq!(g.points[0], p(0.0, 0.0), "the black point stays");
        assert_eq!(g.index, 1);
        assert!((g.points[1].x - MIN_GAP).abs() < 1e-12, "kept a gap from the black point: {:?}", g.points);
        assert!(g.points.windows(2).all(|w| w[0].x < w[1].x), "{:?}", g.points);
    }

    #[test]
    fn the_black_point_moves_up_and_right_but_not_out_of_the_graph_or_past_its_neighbour() {
        let pts = vec![p(0.0, 0.0), p(0.3, 0.3), p(1.0, 1.0)];
        assert_eq!(move_point(&pts, 0, -0.5, 0.2)[0], p(0.0, 0.2));
        assert_eq!(move_point(&pts, 0, 0.1, -0.5)[0], p(0.1, 0.0));
        assert!((move_point(&pts, 0, 0.9, 0.0)[0].x - (0.3 - MIN_GAP)).abs() < 1e-12);
        assert!((move_point(&pts, 2, 0.0, 1.0)[2].x - (0.3 + MIN_GAP)).abs() < 1e-12);
    }

    #[test]
    fn the_ends_are_never_removed() {
        let pts = vec![p(0.0, 0.0), p(0.3, 0.4), p(1.0, 1.0)];
        assert!(remove(&pts, 0).is_none(), "black stays");
        assert!(remove(&pts, 2).is_none(), "white stays");
        assert_eq!(remove(&pts, 1), Some(straight()));
        assert!(remove(&straight(), 1).is_none());
        assert!(remove(&pts, 9).is_none());
    }

    #[test]
    fn a_press_near_the_curve_adds_the_point_on_the_curve() {
        let g = grab(&straight(), 0.5, 0.55, 0.08);
        assert!(g.added);
        assert_eq!(g.points[g.index], p(0.5, 0.5));
        // far from it: where the press is
        let g = grab(&straight(), 0.5, 0.9, 0.08);
        assert_eq!(g.points[g.index], p(0.5, 0.9));
    }

    #[test]
    fn neighbours_closer_than_the_gap_leave_the_point_midway_and_in_order() {
        let pts = vec![p(0.0, 0.0), p(0.5, 0.5), p(0.508, 0.6), p(0.515, 0.3), p(1.0, 1.0)];
        let v = move_point(&pts, 2, 0.9, 0.2);
        assert!(v.windows(2).all(|w| w[0].x <= w[1].x), "{v:?}");
        assert!(v[2].x > 0.5 && v[2].x < 0.515, "{v:?}");
    }

    #[test]
    fn bad_input_changes_nothing_and_never_panics() {
        let pts = vec![p(0.0, 0.0), p(0.5, 0.5), p(1.0, 1.0)];
        assert_eq!(move_point(&pts, 7, 0.1, 0.1), pts);
        assert_eq!(move_point(&pts, 1, f64::NAN, 0.1), pts);
        assert_eq!(nudge(&pts, 9, 0.1, 0.1), pts);
        let g = grab(&[], f64::NAN, 0.5, 0.08);
        assert!(g.points.len() == 2 && g.index == 0 && !g.added, "{g:?}");
        assert_eq!(nearest(&[], 0.0, 0.0, 1.0), None);
        let mixed = [p(f64::NAN, 0.2), p(0.8, 0.9), p(0.2, 0.1)];
        assert_eq!(shown(&mixed), vec![p(0.2, 0.1), p(0.8, 0.9)]);
    }
}
