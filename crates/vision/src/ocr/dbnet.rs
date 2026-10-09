//! The detector's output turned into text boxes (the DB post-processing of PaddleOCR, written
//! here from its description): threshold the probability map, take each connected blob, fit its
//! smallest rotated rectangle, keep it if the map is confident inside, and grow it a little (the
//! network predicts a shrunken core of each line).

/// DB post-processing settings (the model's own, from its `inference.yml`).
#[derive(Clone, Copy, Debug)]
pub struct Params {
    pub thresh: f32,
    pub box_thresh: f32,
    pub unclip_ratio: f32,
    /// Shortest side of a box, in map pixels, before and (+2) after growing.
    pub min_size: f32,
    pub max_candidates: usize,
}

impl Default for Params {
    fn default() -> Self {
        Params { thresh: 0.2, box_thresh: 0.45, unclip_ratio: 1.4, min_size: 3.0, max_candidates: 3000 }
    }
}

/// A rotated rectangle. `u` is the reading direction (a unit vector, along the longer side),
/// `v = (-u.y, u.x)` is "down" for the text; `half` is half the extent along `u` and `v`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub c: [f32; 2],
    pub u: [f32; 2],
    pub half: [f32; 2],
}

impl Rect {
    pub fn v(&self) -> [f32; 2] {
        [-self.u[1], self.u[0]]
    }

    /// The corners, top-left first in reading order.
    pub fn corners(&self) -> [[f32; 2]; 4] {
        let (u, v) = (self.u, self.v());
        let at = |a: f32, b: f32| [self.c[0] + a * u[0] + b * v[0], self.c[1] + a * u[1] + b * v[1]];
        [at(-self.half[0], -self.half[1]), at(self.half[0], -self.half[1]), at(self.half[0], self.half[1]), at(-self.half[0], self.half[1])]
    }

    fn contains(&self, p: [f32; 2]) -> bool {
        let (d, v) = ([p[0] - self.c[0], p[1] - self.c[1]], self.v());
        (d[0] * self.u[0] + d[1] * self.u[1]).abs() <= self.half[0] && (d[0] * v[0] + d[1] * v[1]).abs() <= self.half[1]
    }
}

/// The text boxes of `prob` (a `w × h` map of probabilities, row-major), in map pixels.
pub fn boxes(prob: &[f32], w: usize, h: usize, p: &Params) -> Vec<Rect> {
    let mut out = Vec::new();
    if w == 0 || h == 0 || w.checked_mul(h) != Some(prob.len()) {
        return out;
    }
    let on = |x: usize, y: usize| prob.get(y * w + x).is_some_and(|&v| v > p.thresh);
    let mut seen = vec![false; prob.len()];
    let mut stack: Vec<(usize, usize)> = Vec::new();
    let mut edge: Vec<[f32; 2]> = Vec::new();
    for y0 in 0..h {
        for x0 in 0..w {
            let i0 = y0 * w + x0;
            if seen.get(i0).copied().unwrap_or(true) || !on(x0, y0) {
                continue;
            }
            // flood fill the blob (8-connected), keeping the pixels on its outline
            stack.clear();
            edge.clear();
            stack.push((x0, y0));
            if let Some(s) = seen.get_mut(i0) {
                *s = true;
            }
            let mut count = 0usize;
            while let Some((x, y)) = stack.pop() {
                count += 1;
                let mut inner = true;
                for (dx, dy) in [(-1i32, -1i32), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
                    let (nx, ny) = (x as i64 + i64::from(dx), y as i64 + i64::from(dy));
                    if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                        inner = false;
                        continue;
                    }
                    let (nx, ny) = (nx as usize, ny as usize);
                    if !on(nx, ny) {
                        inner = false;
                    } else if let Some(s) = seen.get_mut(ny * w + nx)
                        && !*s
                    {
                        *s = true;
                        stack.push((nx, ny));
                    }
                }
                if !inner {
                    edge.push([x as f32 + 0.5, y as f32 + 0.5]);
                }
            }
            if count < 4 || out.len() >= p.max_candidates {
                continue;
            }
            let hull = convex_hull(&mut edge);
            let Some(r) = min_area_rect(&hull) else { continue };
            if 2.0 * r.half[0].min(r.half[1]) < p.min_size {
                continue;
            }
            // how sure the map is inside the box (all of it, not just the pixels over the threshold)
            let score = mean_inside(prob, w, h, &r);
            if score < p.box_thresh {
                continue;
            }
            let (hu, hv) = (r.half[0], r.half[1]);
            let dist = (4.0 * hu * hv) * p.unclip_ratio / (4.0 * (hu + hv)).max(f32::EPSILON);
            let grown = Rect { half: [hu + dist, hv + dist], ..r };
            if 2.0 * grown.half[0].min(grown.half[1]) < p.min_size + 2.0 {
                continue;
            }
            out.push(grown);
        }
    }
    out
}

fn mean_inside(prob: &[f32], w: usize, h: usize, r: &Rect) -> f32 {
    let cs = r.corners();
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for c in cs {
        (x0, y0, x1, y1) = (x0.min(c[0]), y0.min(c[1]), x1.max(c[0]), y1.max(c[1]));
    }
    let (xa, ya) = (x0.floor().max(0.0) as usize, y0.floor().max(0.0) as usize);
    let (xb, yb) = ((x1.ceil().max(0.0) as usize).min(w), (y1.ceil().max(0.0) as usize).min(h));
    let (mut sum, mut n) = (0f64, 0u64);
    for y in ya..yb {
        for x in xa..xb {
            if r.contains([x as f32 + 0.5, y as f32 + 0.5])
                && let Some(&v) = prob.get(y * w + x)
            {
                sum += f64::from(v);
                n += 1;
            }
        }
    }
    if n == 0 { 0.0 } else { (sum / n as f64) as f32 }
}

/// Andrew's monotone chain (counter-clockwise, no repeated first point). Sorts `pts`.
fn convex_hull(pts: &mut Vec<[f32; 2]>) -> Vec<[f32; 2]> {
    pts.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    pts.dedup();
    if pts.len() < 3 {
        return pts.clone();
    }
    let cross = |o: [f32; 2], a: [f32; 2], b: [f32; 2]| (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);
    let mut hull: Vec<[f32; 2]> = Vec::with_capacity(pts.len().min(64) * 2);
    for pass in 0..2 {
        let start = hull.len();
        let iter: Box<dyn Iterator<Item = &[f32; 2]>> = if pass == 0 { Box::new(pts.iter()) } else { Box::new(pts.iter().rev()) };
        for &p in iter {
            while hull.len() >= start + 2 && cross(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0.0 {
                hull.pop();
            }
            hull.push(p);
        }
        hull.pop();
    }
    hull
}

/// The smallest rectangle around a convex polygon (one side flush with one of its edges), turned
/// so `u` runs along the longer side, to the right (or, for a mostly vertical box, downwards).
fn min_area_rect(hull: &[[f32; 2]]) -> Option<Rect> {
    if hull.len() < 3 {
        return None;
    }
    let mut best: Option<(f32, Rect)> = None;
    for i in 0..hull.len() {
        let (a, b) = (hull[i], hull[(i + 1) % hull.len()]);
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let len = dx.hypot(dy);
        if len < 1e-6 {
            continue;
        }
        let (u, v) = ([dx / len, dy / len], [-dy / len, dx / len]);
        let (mut pu0, mut pu1, mut pv0, mut pv1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for p in hull {
            let (pu, pv) = (p[0] * u[0] + p[1] * u[1], p[0] * v[0] + p[1] * v[1]);
            (pu0, pu1, pv0, pv1) = (pu0.min(pu), pu1.max(pu), pv0.min(pv), pv1.max(pv));
        }
        let (hu, hv) = ((pu1 - pu0) / 2.0, (pv1 - pv0) / 2.0);
        let area = hu * hv;
        if best.as_ref().is_none_or(|(a, _)| area < *a) {
            let (cu, cv) = ((pu0 + pu1) / 2.0, (pv0 + pv1) / 2.0);
            let c = [cu * u[0] + cv * v[0], cu * u[1] + cv * v[1]];
            best = Some((area, Rect { c, u, half: [hu, hv] }));
        }
    }
    let (_, r) = best?;
    Some(canonical(r))
}

/// `u` along the longer side, pointing right (a mostly vertical box: down), so that text reads
/// along it and `v` is below it. A vertical box is read rotated a quarter turn, like PaddleOCR.
fn canonical(r: Rect) -> Rect {
    let (mut u, mut half) = (r.u, r.half);
    if half[0] < half[1] {
        let v = r.v();
        u = v;
        half = [half[1], half[0]];
    }
    let vertical = u[1].abs() > u[0].abs();
    if (vertical && u[1] < 0.0) || (!vertical && u[0] < 0.0) {
        u = [-u[0], -u[1]];
    }
    Rect { c: r.c, u, half }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `w × h` map with `p` inside the rectangle `[x0, x1) × [y0, y1)`.
    fn map(w: usize, h: usize, rects: &[(usize, usize, usize, usize)], p: f32) -> Vec<f32> {
        let mut m = vec![0.0; w * h];
        for &(x0, y0, x1, y1) in rects {
            for y in y0..y1 {
                for x in x0..x1 {
                    m[y * w + x] = p;
                }
            }
        }
        m
    }

    #[test]
    fn a_horizontal_line_becomes_a_box_a_little_bigger_than_its_core() {
        let m = map(200, 100, &[(20, 40, 180, 52)], 0.9);
        let b = boxes(&m, 200, 100, &Params::default());
        assert_eq!(b.len(), 1);
        let r = b[0];
        assert!((r.c[0] - 100.0).abs() < 1.5 && (r.c[1] - 46.0).abs() < 1.5, "{r:?}");
        assert!(r.u[0] > 0.99 && r.u[1].abs() < 0.05, "reads left to right: {r:?}");
        // grown by area × 1.4 / perimeter on each side
        assert!(r.half[0] > 80.0 && r.half[1] > 6.0 + 3.0, "{r:?}");
        let cs = r.corners();
        assert!(cs[0][0] < cs[1][0] && cs[0][1] < cs[3][1], "top-left first: {cs:?}");
    }

    #[test]
    fn two_lines_are_two_boxes_and_noise_and_weak_blobs_are_dropped() {
        let mut m = map(200, 100, &[(10, 10, 190, 22), (10, 60, 190, 72)], 0.9);
        // a speck, and a blob the map isn't sure about
        for (i, v) in m.iter_mut().enumerate() {
            if i == 5 * 200 + 5 {
                *v = 0.99;
            }
        }
        let weak = map(200, 100, &[(10, 30, 100, 40)], 0.3);
        for (v, w) in m.iter_mut().zip(&weak) {
            *v = v.max(*w);
        }
        let b = boxes(&m, 200, 100, &Params::default());
        assert_eq!(b.len(), 2, "{b:?}");
    }

    #[test]
    fn a_tilted_line_keeps_its_angle() {
        // a band rotated ~20 degrees
        let (w, h) = (300usize, 200usize);
        let mut m = vec![0.0; w * h];
        let (a, c, s) = (20f32.to_radians(), (150.0f32, 100.0f32), 0.0);
        let _ = s;
        for y in 0..h {
            for x in 0..w {
                let (dx, dy) = (x as f32 - c.0, y as f32 - c.1);
                let (u, v) = (dx * a.cos() + dy * a.sin(), -dx * a.sin() + dy * a.cos());
                if u.abs() < 100.0 && v.abs() < 7.0 {
                    m[y * w + x] = 0.9;
                }
            }
        }
        let b = boxes(&m, w, h, &Params::default());
        assert_eq!(b.len(), 1);
        let angle = b[0].u[1].atan2(b[0].u[0]).to_degrees();
        assert!((angle - 20.0).abs() < 4.0, "{angle}");
    }

    #[test]
    fn a_vertical_box_reads_downwards() {
        let m = map(100, 200, &[(40, 20, 52, 180)], 0.9);
        let b = boxes(&m, 100, 200, &Params::default());
        assert_eq!(b.len(), 1);
        assert!(b[0].u[1] > 0.99 && b[0].u[0].abs() < 0.05, "{:?}", b[0]);
        assert!(b[0].half[0] > b[0].half[1]);
    }

    #[test]
    fn nonsense_maps_give_no_boxes_and_no_panic() {
        let p = Params::default();
        assert!(boxes(&[], 0, 0, &p).is_empty());
        assert!(boxes(&[0.9; 10], 3, 3, &p).is_empty(), "length doesn't match");
        assert!(boxes(&[0.9; 4], usize::MAX, 2, &p).is_empty());
        assert!(boxes(&vec![0.0; 100], 10, 10, &p).is_empty());
        assert!(boxes(&vec![f32::NAN; 100], 10, 10, &p).is_empty());
        // everything on: one huge blob, still fine
        assert!(boxes(&vec![1.0; 64 * 64], 64, 64, &p).len() <= 1);
        // a thin line is below the minimum size
        assert!(boxes(&map(100, 100, &[(10, 50, 90, 52)], 0.9), 100, 100, &p).is_empty());
    }
}
