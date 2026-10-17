//! Remove / Heal / Clone spots, rendered at output resolution in scene-linear light.
//!
//! - **Clone** copies the source patch with a feathered edge.
//! - **Heal** copies the source's *detail* and keeps the target's *tone*: the source patch is matched to the photo along
//!   the edge of the spot and that match is spread over the patch as a membrane ([`boundary_match`]), so no seam is left.
//! - **Remove** fills the stroke with texture synthesized from around it ([`crate::inpaint`]), seeded
//!   by and searching the spot's source too; it heals from the source (matched like Heal's patch) when nothing around fits.
//!   With **AI Remove** the spot carries what a model painted under it ([`SpotFill`], made by
//!   [`erase_input`] and [`EraseInput::finish`] around the model), matched to the photo along the stroke's edge as Heal
//!   does, so a later white balance change follows.
//!   Automatic sources: candidate offsets on rings around the spot are scored by how well the
//!   source's surrounding annulus matches the target's.
//! - **Fill** (content-aware) rebuilds the spot from patches of its surroundings instead of one
//!   source: PatchMatch-based completion ([`lightcraft_inpaint::complete`]), coarse to fine, so the
//!   structure is chosen at a scale set by the spot's size (the same at preview and export size).
//!   Deterministic; the source offset is ignored.

use lightcraft_develop::{DevelopSettings, Spot, SpotFill, SpotMode};
use lightcraft_geom::{Affine, CropGeometry, Point, Rect};
use lightcraft_raster::Rgb32f;
use lightcraft_raster::resample::{Filter, fit};

use crate::geometry::Frame;

#[inline]
fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    // Zero feather is a hard edge, including when the pixel lies exactly on the radius.
    // Dividing 0 by 0 here would give that pixel a NaN alpha, even at zero opacity.
    if e0 >= e1 {
        return if x < e1 { 0.0 } else { 1.0 };
    }
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Most a spot's size counts for, in long edges: a size read from a sidecar isn't held to the slider's range, and one
/// far past the picture (or infinite) made the radius, the blur it heals with and the patch it reads from unbounded
/// (issue #1519). Two long edges cover the whole frame from anywhere in it.
const MAX_SPOT_SIZE: f64 = 2.0;

/// A spot's radius in output pixels, `ppl` output pixels to the long edge: at least a pixel (a size that is not a
/// number too), at most [`MAX_SPOT_SIZE`] long edges.
fn radius_px(size: f64, ppl: f64) -> f64 {
    let size = if size.is_nan() { 0.0 } else { size.min(MAX_SPOT_SIZE) };
    (size * ppl).max(1.0)
}

/// Mean colour of `img` in a disc/annulus.
fn ring_stats(img: &Rgb32f, c: (f32, f32), r0: f32, r1: f32) -> Option<Vec<[f32; 3]>> {
    let mut out = Vec::new();
    let n = 24;
    for i in 0..n {
        let a = i as f32 / n as f32 * std::f32::consts::TAU;
        for rr in [r0, (r0 + r1) / 2.0, r1] {
            let (x, y) = (c.0 + a.cos() * rr, c.1 + a.sin() * rr);
            if x < 0.0 || y < 0.0 || x >= img.width as f32 || y >= img.height as f32 {
                return None;
            }
            out.push(img.sample_bilinear(x, y));
        }
    }
    Some(out)
}

fn score(img: &Rgb32f, target: (f32, f32), source: (f32, f32), r: f32) -> f32 {
    let (Some(a), Some(b)) = (ring_stats(img, target, r * 1.05, r * 1.6), ring_stats(img, source, r * 1.05, r * 1.6)) else { return f32::MAX };
    // also compare the inside of the source against the target's surroundings (texture continuity)
    let inside = ring_stats(img, source, r * 0.2, r * 0.9);
    let mut s = 0.0;
    for (p, q) in a.iter().zip(&b) {
        for c in 0..3 {
            let (u, v) = (p[c].max(0.0).sqrt(), q[c].max(0.0).sqrt());
            s += (u - v) * (u - v);
        }
    }
    if let Some(ins) = inside {
        let mean_a: f32 = a.iter().map(|p| p[1].max(0.0).sqrt()).sum::<f32>() / a.len() as f32;
        let var: f32 = ins.iter().map(|p| (p[1].max(0.0).sqrt() - mean_a).powi(2)).sum::<f32>() / ins.len() as f32;
        s += var * a.len() as f32 * 0.5;
    }
    s
}

/// Candidate source offsets (output pixels) for a spot of radius `r` at `target`, best first
/// (candidates reaching outside the image are left out).
pub fn ranked_sources(img: &Rgb32f, target: (f32, f32), r: f32) -> Vec<(f32, f32)> {
    let mut c: Vec<(f32, (f32, f32))> = Vec::new();
    for ring in [2.4f32, 3.2, 4.2] {
        for i in 0..16 {
            let a = i as f32 / 16.0 * std::f32::consts::TAU;
            let d = (a.cos() * r * ring, a.sin() * r * ring);
            let s = score(img, target, (target.0 + d.0, target.1 + d.1), r);
            if s < f32::MAX {
                c.push((s, d));
            }
        }
    }
    c.sort_by(|a, b| a.0.total_cmp(&b.0));
    c.into_iter().map(|(_, d)| d).collect()
}

/// Pick a source offset (in output pixels) for a remove/heal spot.
pub fn auto_source(img: &Rgb32f, target: (f32, f32), r: f32) -> (f32, f32) {
    ranked_sources(img, target, r).first().copied().unwrap_or((r * 2.5, 0.0))
}

/// A source offset (normalized, as [`Spot::source_offset`]) for `spot` on `src` developed with
/// `s`: the best match, or with `avoid` (the current offset) the best one at least a spot radius
/// away from it ("refresh source"). `None` when no candidate fits in the image.
pub fn pick_source(
    src: &Rgb32f,
    info: &crate::SourceInfo,
    s: &lightcraft_develop::DevelopSettings,
    spot: &Spot,
    avoid: Option<Point>,
) -> Option<Point> {
    let frame = crate::frame_for(src, info, s, true);
    let (w, h) = frame.fit(512, 512);
    if w == 0 || h == 0 {
        return None;
    }
    let img = frame.sample(src, w, h);
    let (to_out, to_norm) = (frame.norm_to_out(w, h), frame.out_to_norm(w, h));
    let t = to_out.apply(*spot.points.first()?);
    let r = radius_px(spot.size, frame.px_per_long(w)) as f32;
    let avoid = avoid.map(|o| {
        let (a, b) = (to_out.apply(Point::new(0.0, 0.0)), to_out.apply(o));
        ((b.x - a.x) as f32, (b.y - a.y) as f32)
    });
    let far = |d: &(f32, f32)| avoid.is_none_or(|a| ((d.0 - a.0).powi(2) + (d.1 - a.1).powi(2)).sqrt() > r);
    let d = ranked_sources(&img, (t.x as f32, t.y as f32), r).into_iter().find(far)?;
    let (n0, n1) = (to_norm.apply(t), to_norm.apply(Point::new(t.x + d.0 as f64, t.y + d.1 as f64)));
    Some(Point::new(n1.x - n0.x, n1.y - n0.y))
}

/// Distance from `q` to the stroke through `pts`: to its segments, so that a stroke painted fast
/// (a finger's samples far apart) heals along its whole length, not in dabs; a lone point is a dab.
fn dist_to_path(pts: &[(f32, f32)], q: (f32, f32)) -> f32 {
    let seg = |a: (f32, f32), b: (f32, f32)| {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len2 = dx * dx + dy * dy;
        let t = if len2 > 0.0 { (((q.0 - a.0) * dx + (q.1 - a.1) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
        ((q.0 - a.0 - t * dx).powi(2) + (q.1 - a.1 - t * dy).powi(2)).sqrt()
    };
    match pts {
        [] => f32::MAX,
        [p] => seg(*p, *p),
        _ => pts.windows(2).filter_map(|w| if let [a, b] = w { Some(seg(*a, *b)) } else { None }).fold(f32::MAX, f32::min),
    }
}

/// Distance, in pixels, from each pixel of a `w × h` box to the nearest pixel that is `inside` (0 in it): a
/// two-pass chamfer, close enough to round for a spot's feathered edge.
pub(crate) fn distance_to_inside(w: usize, h: usize, inside: &[bool]) -> Vec<f32> {
    const DIAG: f32 = std::f32::consts::SQRT_2;
    let mut d: Vec<f32> = inside.iter().map(|i| if *i { 0.0 } else { f32::MAX / 4.0 }).collect();
    let at = |d: &[f32], x: usize, y: usize, add: f32| d.get(y * w + x).map_or(f32::MAX, |v| v + add);
    for y in 0..h {
        for x in 0..w {
            let mut v = at(&d, x, y, 0.0);
            if x > 0 {
                v = v.min(at(&d, x - 1, y, 1.0));
            }
            if y > 0 {
                v = v.min(at(&d, x, y - 1, 1.0));
                if x > 0 {
                    v = v.min(at(&d, x - 1, y - 1, DIAG));
                }
                if x + 1 < w {
                    v = v.min(at(&d, x + 1, y - 1, DIAG));
                }
            }
            if let Some(c) = d.get_mut(y * w + x) {
                *c = v;
            }
        }
    }
    for y in (0..h).rev() {
        for x in (0..w).rev() {
            let mut v = at(&d, x, y, 0.0);
            if x + 1 < w {
                v = v.min(at(&d, x + 1, y, 1.0));
            }
            if y + 1 < h {
                v = v.min(at(&d, x, y + 1, 1.0));
                if x + 1 < w {
                    v = v.min(at(&d, x + 1, y + 1, DIAG));
                }
                if x > 0 {
                    v = v.min(at(&d, x - 1, y + 1, DIAG));
                }
            }
            if let Some(c) = d.get_mut(y * w + x) {
                *c = v;
            }
        }
    }
    d
}

/// The most a window grows to take in the spots that touch it.
const MAX_SPOT_SPAN: usize = if cfg!(target_arch = "wasm32") { 4096 } else { 8192 };
/// Strokes with more dabs than this are not examined dab by dab (the render would be too slow anyway).
const MAX_DABS_CONSIDERED: usize = 200_000;

/// `win` (a window of the `full_w × full_h` output of `frame`) grown to hold, for every spot (or
/// Auto Mask stroke) that reaches into it, the whole spot and the patch it reads from: [`apply`] works on the pixels it
/// is given, so a spot cut by the window's edge, or whose source lies outside it, would come out
/// different from the same spot in the whole render. Spots with an automatic source (`None`)
/// search up to 4.2 radii around themselves. Growth that would pass [`MAX_SPOT_SPAN`] pixels is not
/// done (the window comes back as asked; see [`window_for_reads_checked`] to know).
pub fn window_for_reads(
    s: &lightcraft_develop::DevelopSettings,
    frame: &Frame,
    full_w: usize,
    full_h: usize,
    ppl: f64,
    win: crate::PixelWindow,
) -> crate::PixelWindow {
    window_for_reads_checked(s, frame, full_w, full_h, ppl, win).unwrap_or(win)
}

/// [`window_for_reads`], or `None` when holding everything the window reads would take more than
/// [`MAX_SPOT_SPAN`] pixels along an axis: a caller that can show something else (the loupe's
/// whole-frame render) must, because the window alone would come out wrong.
pub fn window_for_reads_checked(
    s: &lightcraft_develop::DevelopSettings,
    frame: &Frame,
    full_w: usize,
    full_h: usize,
    ppl: f64,
    win: crate::PixelWindow,
) -> Option<crate::PixelWindow> {
    let spots = &s.spots;
    let autos: Vec<&lightcraft_develop::BrushStroke> = s
        .masks
        .iter()
        .filter(|m| m.visible)
        .flat_map(|m| &m.components)
        .filter_map(|c| match &c.shape {
            lightcraft_develop::MaskShape::Brush { strokes } => Some(strokes),
            _ => None,
        })
        .flatten()
        .filter(|st| st.auto_mask)
        .collect();
    if spots.is_empty() && autos.is_empty() {
        return Some(win);
    }
    let to_out = frame.norm_to_out(full_w, full_h);
    let origin = to_out.apply(Point::new(0.0, 0.0));
    // (target box, source box) of each spot, in output pixels: [x0, y0, x1, y1]
    let boxes: Vec<([f64; 4], [f64; 4])> = spots
        .iter()
        .filter_map(|spot| {
            let r = radius_px(spot.size, ppl);
            let pts: Vec<_> = spot.points.iter().map(|p| to_out.apply(*p)).collect();
            let first = pts.first()?;
            let (mut x0, mut y0, mut x1, mut y1) = (first.x, first.y, first.x, first.y);
            for p in &pts {
                (x0, y0, x1, y1) = (x0.min(p.x), y0.min(p.y), x1.max(p.x), y1.max(p.y));
            }
            // the box `apply` works in: the stroke plus a radius, plus a radius of padding; Remove's
            // synthesis also reads two radii and a patch around that box ([`crate::inpaint::fill`])
            let m = if spot.mode == SpotMode::Remove { 4.0 * r + 5.0 } else { 2.0 * r };
            let t = [x0 - m, y0 - m, x1 + m, y1 + m];
            let s = match spot.source_offset {
                Some(o) => {
                    let b = to_out.apply(o);
                    let (dx, dy) = (b.x - origin.x, b.y - origin.y);
                    [t[0] + dx, t[1] + dy, t[2] + dx, t[3] + dy]
                }
                None => [t[0] - 4.2 * r, t[1] - 4.2 * r, t[2] + 4.2 * r, t[3] + 4.2 * r],
            };
            Some((t, s)).filter(|(t, s)| t.iter().chain(s).all(|v| v.is_finite()))
        })
        .collect();
    // Auto Mask strokes weigh each dab by its similarity to the pixel under the dab's centre and
    // refine the stroke over a couple of radii: each dab matters to pixels within 2.5 radii
    let mut dab_boxes: Vec<[f64; 4]> = Vec::new();
    for st in autos {
        let dabs = crate::masks::brush_dabs(st, frame, full_w, full_h);
        let r = dabs.r;
        if dabs.dabs.len() > MAX_DABS_CONSIDERED {
            continue;
        }
        for d in &dabs.dabs {
            let b = [d.x - 2.5 * r, d.y - 2.5 * r, d.x + 2.5 * r, d.y + 2.5 * r];
            if b.iter().all(|v| v.is_finite()) {
                dab_boxes.push(b);
            }
        }
    }
    let (fw, fh) = (full_w as f64, full_h as f64);
    let mut r = [win.x as f64, win.y as f64, (win.x + win.w) as f64, (win.y + win.h) as f64];
    let touches = |r: &[f64; 4], b: &[f64; 4]| b[0] < r[2] && b[2] > r[0] && b[1] < r[3] && b[3] > r[1];
    // a dab reaching the requested window needs its surroundings, once: the pixels that brings in
    // are only read, not drawn on (unlike spots, whose patches other spots may read)
    let asked = r;
    for b in dab_boxes.iter().filter(|b| touches(&asked, b)) {
        r = [r[0].min(b[0]), r[1].min(b[1]), r[2].max(b[2]), r[3].max(b[3])];
    }
    r = [r[0].max(0.0), r[1].max(0.0), r[2].min(fw), r[3].min(fh)];
    if r[2] - r[0] > MAX_SPOT_SPAN as f64 || r[3] - r[1] > MAX_SPOT_SPAN as f64 {
        return None;
    }
    for _ in 0..=boxes.len() {
        let mut grown = r;
        for (t, s) in &boxes {
            if touches(&r, t) {
                for b in [t, s] {
                    grown = [grown[0].min(b[0]), grown[1].min(b[1]), grown[2].max(b[2]), grown[3].max(b[3])];
                }
            }
        }
        let grown = [grown[0].max(0.0), grown[1].max(0.0), grown[2].min(fw), grown[3].min(fh)];
        if grown == r {
            break;
        }
        if grown[2] - grown[0] > MAX_SPOT_SPAN as f64 || grown[3] - grown[1] > MAX_SPOT_SPAN as f64 {
            return None;
        }
        r = grown;
    }
    let (x, y) = (r[0].floor() as usize, r[1].floor() as usize);
    Some(
        crate::PixelWindow { x, y, w: (r[2].ceil() as usize).saturating_sub(x).max(1), h: (r[3].ceil() as usize).saturating_sub(y).max(1) }
            .clamped(full_w, full_h),
    )
}

/// Content-aware fill of the box: `src` becomes `tgt` with every pixel under the stroke (`alpha > 0`)
/// rebuilt from the rest of the box. Falls back to a smooth membrane fill when the box holds no whole
/// patch to copy from.
fn fill_box(src: &mut [[f32; 3]], tgt: &[[f32; 3]], alpha: &[f32], bw: usize, bh: usize) {
    let hole: Vec<bool> = alpha.iter().map(|a| *a > 0.0).collect();
    let flat: Vec<f32> = tgt.iter().flatten().copied().collect();
    let out = lightcraft_inpaint::complete(bw, bh, 3, &flat, &hole, &lightcraft_inpaint::CompleteParams::default())
        .unwrap_or_else(|| lightcraft_inpaint::membrane_fill(bw, bh, 3, &flat, &hole));
    let (pixels, _) = out.as_chunks::<3>();
    for (d, v) in src.iter_mut().zip(pixels) {
        *d = *v;
    }
}

/// How far the correction looks past the edge of the filled area, and how wide the band of the area along the edge that
/// carries it is, in radii (at least a pixel). Measured with `remove_eval` (Heal, Remove and AI Remove on the two CC0
/// photos and the demo scenes): the smaller, the less of the photo's own gradient across the edge counts as a mismatch,
/// and the median seam error fell from 0.116 (the Gaussian offset this replaced) to 0.037 for Heal at this size, against
/// 0.058 at four times it. Smaller still lets the grain decide.
const EDGE_REACH: f32 = 0.03;

/// How far, in log gain (0.45 is about ±55 % at one spread, softly), the mismatch at one place on the edge may differ from the edge's usual
/// one before it is held back. The usual one (the median) is applied whole: it is the light the fill was made in
/// against the photo's now (an exposure or white balance change). A place far from it is a different thing meeting
/// the edge (a neighbour's coat, what is left of the object), and forcing the fill to match that colours the whole area.
const GAIN_SPREAD: f32 = 0.45;

/// Change `src`, the fill, so it meets the photo `tgt` all along the edge of the area it covers (where `alpha` is not 0),
/// and spread what that takes over the area as a membrane (harmonic: no seam is left, however the photo's tones change
/// along the edge). The fill just inside the edge is compared with the photo just outside it, each averaged over
/// [`EDGE_REACH`] radii `r` so grain doesn't decide; the difference is a gain per channel (a ratio, so a white balance
/// or exposure change made after a fill was baked, or a source in other light, is followed in the shadows and the
/// highlights alike), applied to the fill's tones and not to its grain. `false` when there is no edge to match.
fn boundary_match(src: &mut [[f32; 3]], tgt: &[[f32; 3]], alpha: &[f32], (bw, bh): (usize, usize), r: f32) -> bool {
    let n = bw * bh;
    if src.len() != n || tgt.len() != n || alpha.len() != n {
        return false;
    }
    let hole: Vec<bool> = alpha.iter().map(|a| *a >= 0.02).collect();
    let covered_px = hole.iter().filter(|h| **h).count();
    if covered_px == 0 || covered_px == n {
        return false;
    }
    let reach = (r * EDGE_REACH).max(1.0);
    let blur = |v: &[[f32; 3]], sigma: f32| lightcraft_raster::blur::gaussian(&Rgb32f { width: bw, height: bh, data: v.to_vec() }, sigma);
    let blur_plane = |w: Vec<f32>, sigma: f32| lightcraft_raster::blur::gaussian(&lightcraft_raster::Plane { width: bw, height: bh, data: w }, sigma);
    // the average of `v` over the pixels that are (`keep`) in the area or not, at every pixel (normalized convolution)
    let average = |v: &[[f32; 3]], keep: bool| -> Vec<[f32; 3]> {
        let w: Vec<f32> = hole.iter().map(|h| if *h == keep { 1.0 } else { 0.0 }).collect();
        let weighted: Vec<[f32; 3]> = v.iter().zip(&w).map(|(p, w)| p.map(|c| c * w)).collect();
        let (num, den) = (blur(&weighted, reach), blur_plane(w, reach));
        num.data.iter().zip(&den.data).map(|(p, d)| p.map(|c| c / d.max(1e-4))).collect()
    };
    let (around, covered) = (average(tgt, false), average(src, true));
    // the band along the edge carries the mismatch; the rest of the area is solved for
    let outside: Vec<bool> = hole.iter().map(|h| !*h).collect();
    let to_edge = distance_to_inside(bw, bh, &outside);
    let interior: Vec<bool> = hole.iter().zip(&to_edge).map(|(h, d)| *h && *d > reach).collect();
    let band: Vec<bool> = hole.iter().zip(&interior).map(|(h, i)| *h && !*i).collect();
    const EPS: f32 = 1e-4;
    let mut gain: Vec<[f32; 3]> = band
        .iter()
        .zip(around.iter().zip(&covered))
        .map(|(b, (a, c))| if *b { std::array::from_fn(|k| (a[k].max(EPS) / c[k].max(EPS)).ln()) } else { [0.0; 3] })
        .collect();
    let median: [f32; 3] = std::array::from_fn(|k| {
        let mut v: Vec<f32> = gain.iter().zip(&band).filter(|(_, b)| **b).map(|(g, _)| g[k]).collect();
        let mid = v.len() / 2;
        v.select_nth_unstable_by(mid, f32::total_cmp).1.to_owned()
    });
    for (g, _) in gain.iter_mut().zip(&band).filter(|(_, b)| **b) {
        for k in 0..3 {
            g[k] = median[k] + GAIN_SPREAD * ((g[k] - median[k]) / GAIN_SPREAD).tanh();
        }
    }
    // The solver takes every pixel that isn't solved for as a known value, and averages them at its coarse levels, so the
    // photo outside the area must carry the gain too (the edge's, carried outward) or it would pull the area's towards 0.
    let spread = (r * 0.1).max(2.0);
    let (num, den) = (blur(&gain, spread), blur_plane(band.iter().map(|b| if *b { 1.0 } else { 0.0 }).collect(), spread));
    let total = num.data.iter().fold([0f32; 3], |a, p| [a[0] + p[0], a[1] + p[1], a[2] + p[2]]);
    let mean = total.map(|t| t / den.data.iter().sum::<f32>().max(1e-4));
    for ((g, h), (nu, d)) in gain.iter_mut().zip(&hole).zip(num.data.iter().zip(&den.data)) {
        if !*h {
            *g = if *d > 1e-3 { nu.map(|c| c / d) } else { mean };
        }
    }
    let solved = lightcraft_inpaint::membrane_fill(bw, bh, 3, gain.as_flattened(), &interior);
    let (solved, _) = solved.as_chunks::<3>();
    // the gain is for the tones, not the grain: a gain would make a darker source's grain louder than the photo's
    let smooth = blur(src, (r * 0.1).max(3.0));
    for ((p, h), (low, g)) in src.iter_mut().zip(&hole).zip(smooth.data.iter().zip(solved)) {
        if *h {
            for k in 0..3 {
                p[k] = low[k] * g[k].exp() + (p[k] - low[k]);
            }
        }
    }
    true
}

/// Apply all spots to `img` (output resolution). `ppl` = output pixels per long-edge unit.
pub fn apply(img: &mut Rgb32f, spots: &[Spot], frame: &Frame, ppl: f64) {
    if spots.is_empty() {
        return;
    }
    let (w, h) = (img.width, img.height);
    let to_out = frame.norm_to_out(w, h);
    for spot in spots {
        let r = radius_px(spot.size, ppl) as f32;
        let pts: Vec<(f32, f32)> = spot
            .points
            .iter()
            .map(|p| {
                let q = to_out.apply(*p);
                (q.x as f32, q.y as f32)
            })
            .collect();
        let Some(&first) = pts.first() else { continue };
        let fill = spot.mode == SpotMode::Fill;
        let offset = match spot.source_offset {
            _ if fill => (0.0, 0.0),
            Some(o) => {
                let a = to_out.apply(Point::new(0.0, 0.0));
                let b = to_out.apply(o);
                ((b.x - a.x) as f32, (b.y - a.y) as f32)
            }
            None => auto_source(img, first, r),
        };
        // Bounding box of the stroke.
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for p in &pts {
            x0 = x0.min(p.0 - r);
            y0 = y0.min(p.1 - r);
            x1 = x1.max(p.0 + r);
            y1 = y1.max(p.1 + r);
        }
        // content-aware fill reads the spot's surroundings (the window holds 2 radii around it)
        let pad = if fill { 2.0 * r } else { r };
        let (bx0, by0) = (((x0 - pad).floor().max(0.0)) as usize, ((y0 - pad).floor().max(0.0)) as usize);
        let (bx1, by1) = (((x1 + pad).ceil() as usize).min(w), ((y1 + pad).ceil() as usize).min(h));
        if bx1 <= bx0 || by1 <= by0 {
            continue;
        }
        let (bw, bh) = (bx1 - bx0, by1 - by0);
        let feather = (spot.feather / 100.0).clamp(0.0, 1.0) as f32;
        let opacity = (spot.opacity / 100.0).clamp(0.0, 1.0) as f32;
        // alpha of the stroke and source patch in the box
        let mut alpha = vec![0.0f32; bw * bh];
        let remove = spot.mode == SpotMode::Remove;
        let mut hole = vec![false; if remove { bw * bh } else { 0 }];
        let mut src = vec![[0.0f32; 3]; bw * bh];
        let mut tgt = vec![[0.0f32; 3]; bw * bh];
        // Tap to Remove: the object's own shape is the area, so the distance is to it, not to a path
        // (and the fill is synthesized at the scale of the hole it leaves: a big object is not a small spot)
        let mut fill_r = r;
        let shape_dist = match &spot.shape {
            Some(seg) => match crate::masks::SegGrid::new(seg) {
                Some(grid) => {
                    let to_norm = frame.out_to_norm(w, h);
                    let inside: Vec<bool> = (0..bw * bh)
                        .map(|i| {
                            let n = to_norm.apply(Point::new((bx0 + i % bw) as f64 + 0.5, (by0 + i / bw) as f64 + 0.5));
                            grid.contains(n) && grid.logit(n) > 0.0
                        })
                        .collect();
                    fill_r = fill_r.max((inside.iter().filter(|i| **i).count() as f32 / std::f32::consts::PI).sqrt());
                    Some(distance_to_inside(bw, bh, &inside))
                }
                // (a damaged shape selects nothing)
                None => continue,
            },
            None => None,
        };
        for y in 0..bh {
            for x in 0..bw {
                let (px, py) = ((bx0 + x) as f32 + 0.5, (by0 + y) as f32 + 0.5);
                let d = match &shape_dist {
                    Some(sd) => sd.get(y * bw + x).copied().unwrap_or(f32::MAX),
                    None => dist_to_path(&pts, (px, py)),
                };
                let inner = r * (1.0 - feather * 0.8);
                alpha[y * bw + x] = (1.0 - smooth(inner, r, d)) * opacity;
                if let Some(m) = hole.get_mut(y * bw + x) {
                    *m = d < r;
                }
                src[y * bw + x] = img.sample_bilinear(px + offset.0, py + offset.1);
                tgt[y * bw + x] = img.get(bx0 + x, by0 + y);
            }
        }
        if fill {
            fill_box(&mut src, &tgt, &alpha, bw, bh);
        }
        // AI Remove: the stored fill, matched to the photo along its edge below like a heal
        let baked = if remove { spot.fill.as_ref().and_then(|f| sample_fill(f, &frame.out_to_norm(w, h), (bx0, by0, bw), &tgt)) } else { None };
        let model_fill = baked.is_some();
        // Remove: synthesize; the seed follows the spot, so a fill doesn't change when others do
        let seed = spot.points.first().map_or(0, |p| p.x.to_bits() ^ p.y.to_bits().rotate_left(32));
        let filled = match baked {
            // the model's fill gives the structure, the synthesized one the photo's grain and fine texture
            Some(ai) => Some(match crate::inpaint::fill(img, bx0, by0, bw, bh, &hole, fill_r, offset, seed) {
                Some(pm) => with_detail(ai, pm, bw, bh, (r * DETAIL).max(1.0)),
                None => ai,
            }),
            None if remove => crate::inpaint::fill(img, bx0, by0, bw, bh, &hole, fill_r, offset, seed),
            None => None,
        };
        // A fill synthesized from around the stroke already continues what is there; the others (a heal's patch, a
        // model's picture, the patch a failed synthesis falls back to) come from elsewhere and are matched to the
        // photo along the edge of the area they cover. Clone stays as copied.
        let from_elsewhere = model_fill || filled.is_none();
        if let Some(f) = filled {
            src = f;
        }
        if from_elsewhere && matches!(spot.mode, SpotMode::Remove | SpotMode::Heal) {
            boundary_match(&mut src, &tgt, &alpha, (bw, bh), r);
        }
        for y in 0..bh {
            for x in 0..bw {
                let i = y * bw + x;
                let a = alpha[i];
                if a <= 0.0 {
                    continue;
                }
                let v = src[i];
                let t = tgt[i];
                img.set(bx0 + x, by0 + y, [t[0] + (v[0] - t[0]) * a, t[1] + (v[1] - t[1]) * a, t[2] + (v[2] - t[2]) * a].map(|c| c.max(0.0)));
            }
        }
    }
}

/// Context AI Remove gives the model on every side of a spot's box, in box long sides. Less makes
/// the hole a big part of what the model sees, and in a crowd it paints another person into it
/// (checked on CC0 beach photos: 0.25 and 0.5 did, 1.5 and 2 didn't); more costs resolution.
const CONTEXT: f64 = 1.5;
/// Below this scale (in spot radii) a baked fill keeps the model's picture; above it, the grain
/// and fine texture come from the fill synthesized from the photo, which the model (at 512 × 512
/// over an area larger than the spot) smooths away.
const DETAIL: f32 = 0.1;

/// `ai`'s broad structure with `pm`'s fine detail (both `bw × bh`): `blur(ai) + pm − blur(pm)`.
fn with_detail(ai: Vec<[f32; 3]>, pm: Vec<[f32; 3]>, bw: usize, bh: usize, sigma: f32) -> Vec<[f32; 3]> {
    let (ai, pm) = (Rgb32f { width: bw, height: bh, data: ai }, Rgb32f { width: bw, height: bh, data: pm });
    let (al, pl) = (lightcraft_raster::blur::gaussian(&ai, sigma), lightcraft_raster::blur::gaussian(&pm, sigma));
    al.data.iter().zip(&pm.data).zip(&pl.data).map(|((a, p), l)| std::array::from_fn(|c| a[c] + p[c] - l[c])).collect()
}

/// A stored AI fill over a box of output pixels (`bx0`, `by0`, `bw` wide, `tgt.len() / bw` high):
/// `tgt` where the fill doesn't reach; `None` when the fill is damaged.
fn sample_fill(f: &SpotFill, to_norm: &Affine, (bx0, by0, bw): (usize, usize, usize), tgt: &[[f32; 3]]) -> Option<Vec<[f32; 3]>> {
    let r = f.bounds()?;
    let img = Rgb32f { width: f.w as usize, height: f.h as usize, data: f.pixels()? };
    let (fw, fh) = (img.width as f64, img.height as f64);
    let (sx, sy) = (fw / (r[2] - r[0]), fh / (r[3] - r[1]));
    let bw = bw.max(1);
    let out = tgt
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let n = to_norm.apply(Point::new((bx0 + i % bw) as f64 + 0.5, (by0 + i / bw) as f64 + 0.5));
            let (x, y) = ((n.x - r[0]) * sx, (n.y - r[1]) * sy);
            if (0.0..=fw).contains(&x) && (0.0..=fh).contains(&y) { img.sample_bilinear(x as f32, y as f32) } else { *t }
        })
        .collect();
    Some(out)
}

/// Longest side of the area around a stroke that AI Remove renders for the model (it sees 512 ×
/// 512 pixels; a little more keeps the photo around the stroke sharp in the stored fill).
const ERASE_RENDER: f64 = 1024.0;
/// Longest side of a stored fill (the model's own resolution).
const ERASE_STORE: usize = 512;

/// What AI Remove gives the model for one spot, and what it needs to store the answer.
pub struct EraseInput {
    /// The area around the stroke, display-encoded (`(v / scale)^(1/2.2)`, 0..=1) for the model.
    pub image: Rgb32f,
    /// The pixels to fill (the stroke), one per pixel of `image`.
    pub hole: Vec<bool>,
    /// That area at the retouching stage, linear: white balanced, earlier spots applied.
    lin: Rgb32f,
    scale: f32,
    /// The area, normalized `[x0, y0, x1, y1]`.
    rect: [f64; 4],
    /// The spot's box in `image` (what the spot renders over): x0, y0, x1, y1.
    keep: [usize; 4],
    /// The area's longest side in pixels of the source it was rendered from.
    pub native: f64,
}

/// The input AI Remove needs for spot `index` of `s` on `src`: the stroke and the photo around
/// it (the stroke's box and [`CONTEXT`] times its long side on every side, at most 2:1), rendered to
/// the retouching stage with the spots before this one, at the source's resolution up to 1024
/// pixels. `None` for a spot without points or outside the image.
pub fn erase_input(src: &Rgb32f, info: &crate::SourceInfo, s: &DevelopSettings, index: usize) -> Option<EraseInput> {
    let spot = s.spots.get(index)?;
    let frame = crate::frame_for(src, info, s, false);
    let (ow, oh) = (frame.ow, frame.oh);
    let r = (spot.size * ow.max(oh)).max(1.0);
    let pts: Vec<(f64, f64)> = spot.points.iter().map(|p| (p.x * ow, p.y * oh)).collect();
    if pts.is_empty() || !pts.iter().all(|p| p.0.is_finite() && p.1.is_finite()) || !r.is_finite() {
        return None;
    }
    // the spot's box as `apply` paints it (the stroke and a radius around it), in the image
    let fold = |f: fn(f64, f64) -> f64, init: f64, k: fn(&(f64, f64)) -> f64| pts.iter().map(k).fold(init, f);
    let bx = [
        fold(f64::min, f64::MAX, |p| p.0) - 2.0 * r,
        fold(f64::min, f64::MAX, |p| p.1) - 2.0 * r,
        fold(f64::max, f64::MIN, |p| p.0) + 2.0 * r,
        fold(f64::max, f64::MIN, |p| p.1) + 2.0 * r,
    ];
    let bx = [bx[0].max(0.0), bx[1].max(0.0), bx[2].min(ow), bx[3].min(oh)];
    if bx[2] - bx[0] < 1.0 || bx[3] - bx[1] < 1.0 {
        return None;
    }
    // context on every side, then no flatter than 2:1
    let m = CONTEXT * (bx[2] - bx[0]).max(bx[3] - bx[1]);
    let (mut cw, mut ch) = (bx[2] - bx[0] + 2.0 * m, bx[3] - bx[1] + 2.0 * m);
    cw = cw.max(ch / 2.0).min(ow);
    ch = ch.max(cw / 2.0).min(oh);
    let (cx, cy) = ((bx[0] + bx[2]) / 2.0, (bx[1] + bx[3]) / 2.0);
    let x0 = (cx - cw / 2.0).clamp(0.0, ow - cw);
    let y0 = (cy - ch / 2.0).clamp(0.0, oh - ch);
    let rect = [x0 / ow, y0 / oh, (x0 + cw) / ow, (y0 + ch) / oh];
    // render the area alone: a crop of exactly it, no flips, the spots before this one
    let mut area = s.clone();
    area.crop.geometry = CropGeometry { rect: Rect { x0: rect[0], y0: rect[1], x1: rect[2], y1: rect[3] }, angle: 0.0 };
    (area.crop.flip_h, area.crop.flip_v) = (false, false);
    area.spots.truncate(index);
    let native = cw.max(ch);
    let side = native.min(ERASE_RENDER).round().max(1.0) as usize;
    let p = crate::plan(src, info, &area, &crate::RenderRequest::fit(side, side));
    let mut lin = p.frame.sample(src, p.w, p.h);
    crate::lin_cpu(&mut lin, info, &p);
    let (w, h) = (lin.width, lin.height);
    let (kx, ky) = (cw / w as f64, ch / h as f64);
    // the stroke, half a pixel wider so its edge is filled too
    let reach = (r + 0.5 * kx.max(ky)) as f32;
    let pts32: Vec<(f32, f32)> = pts.iter().map(|p| (p.0 as f32, p.1 as f32)).collect();
    let hole: Vec<bool> = match &spot.shape {
        // Tap to Remove: the object's shape, widened by the spot's reach
        Some(seg) => {
            let grid = crate::masks::SegGrid::new(seg)?;
            let inside: Vec<bool> = (0..w * h)
                .map(|i| {
                    let n = Point::new((x0 + ((i % w) as f64 + 0.5) * kx) / ow, (y0 + ((i / w) as f64 + 0.5) * ky) / oh);
                    grid.contains(n) && grid.logit(n) > 0.0
                })
                .collect();
            let dist = distance_to_inside(w, h, &inside);
            dist.iter().map(|d| (*d as f64) * kx.max(ky) < f64::from(reach)).collect()
        }
        None => (0..w * h)
            .map(|i| dist_to_path(&pts32, ((x0 + ((i % w) as f64 + 0.5) * kx) as f32, (y0 + ((i / w) as f64 + 0.5) * ky) as f32)) < reach)
            .collect(),
    };
    // exposure for the model: the area's bright end (99th percentile) near white
    let mut bright: Vec<f32> = lin.data.iter().map(|p| p[0].max(p[1]).max(p[2])).filter(|v| v.is_finite()).collect();
    let at = (bright.len() * 99 / 100).min(bright.len().saturating_sub(1));
    let scale = if bright.is_empty() { 1.0 } else { *bright.select_nth_unstable_by(at, f32::total_cmp).1 }.max(1e-4);
    let image = lin.map(|p| p.map(|v| (v / scale).clamp(0.0, 1.0).powf(1.0 / 2.2)));
    let px = |v: f64, k: f64, n: usize| ((v / k).max(0.0) as usize).min(n);
    let keep = [px(bx[0] - x0, kx, w), px(bx[1] - y0, ky, h), px((bx[2] - x0) / kx + 0.999, 1.0, w), px((bx[3] - y0) / ky + 0.999, 1.0, h)];
    (keep[2] > keep[0] && keep[3] > keep[1]).then_some(EraseInput { image, hole, lin, scale, rect, keep, native })
}

impl EraseInput {
    /// The spot's fill from the model's answer `filled` (the same size as [`Self::image`], display
    /// encoded): its pixels in the stroke, the photo's around it, over the spot's box at no more
    /// than 512 pixels a side. `None` when `filled` doesn't match.
    pub fn finish(&self, filled: &Rgb32f) -> Option<SpotFill> {
        let (w, h) = (self.image.width, self.image.height);
        if filled.width != w || filled.height != h || filled.data.len() != w * h {
            return None;
        }
        let [x0, y0, x1, y1] = self.keep;
        let boxed = Rgb32f::from_fn(x1 - x0, y1 - y0, |x, y| {
            let (x, y) = (x0 + x, y0 + y);
            if self.hole.get(y * w + x).copied().unwrap_or(false) {
                filled.get(x, y).map(|e| self.scale * e.clamp(0.0, 1.0).powf(2.2))
            } else {
                self.lin.get(x, y)
            }
        });
        let small = fit(&boxed, ERASE_STORE, ERASE_STORE, Filter::Mitchell);
        let [r0, r1, r2, r3] = self.rect;
        let (nx, ny) = ((r2 - r0) / w as f64, (r3 - r1) / h as f64);
        let rect = [r0 + x0 as f64 * nx, r1 + y0 as f64 * ny, r0 + x1 as f64 * nx, r1 + y1 as f64 * ny];
        let px: Vec<[f32; 3]> = small.data.iter().map(|p| p.map(|v| v.max(0.0))).collect();
        SpotFill::from_linear(small.width, small.height, rect, &px)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hard_edge_spots_at_zero_opacity_leave_every_pixel_unchanged() {
        for mode in [SpotMode::Clone, SpotMode::Heal, SpotMode::Remove, SpotMode::Fill] {
            for (cx, cy) in [(32.5, 24.5), (0.5, 0.5)] {
                let original = Rgb32f::from_fn(64, 48, |x, y| [0.2 + x as f32 / 128.0, 0.3 + y as f32 / 128.0, 0.4]);
                let mut img = original.clone();
                let frame = Frame::new(64, 48, &DevelopSettings::default(), true);
                let spot = Spot {
                    mode,
                    points: vec![Point::new(cx / 64.0, cy / 48.0)],
                    size: 5.0 / 64.0,
                    feather: 0.0,
                    opacity: 0.0,
                    source_offset: Some(Point::new(8.0 / 64.0, 0.0)),
                    fill: None,
                    shape: None,
                };
                apply(&mut img, &[spot], &frame, frame.px_per_long(64));
                for (i, (actual, expected)) in img.data.iter().zip(&original.data).enumerate() {
                    assert_eq!(actual, expected, "zero-opacity {mode:?} at ({cx}, {cy}), pixel {i}");
                }
            }
        }
    }

    #[test]
    fn hard_edge_clone_copies_inside_and_preserves_pixels_on_the_radius() {
        for (w, h, cx, cy, radius) in [(64, 48, 32.5, 24.5, 5.0), (128, 96, 64.5, 48.5, 10.0)] {
            let original = Rgb32f::from_fn(w, h, |x, y| [0.2 + x as f32 / w as f32, y as f32 / h as f32, 0.4]);
            let mut img = original.clone();
            let frame = Frame::new(w, h, &DevelopSettings::default(), true);
            let spot = Spot {
                mode: SpotMode::Clone,
                points: vec![Point::new(cx / w as f64, cy / h as f64)],
                size: radius / w as f64,
                feather: 0.0,
                opacity: 100.0,
                source_offset: Some(Point::new(0.125, 0.0)),
                fill: None,
                shape: None,
            };
            apply(&mut img, &[spot], &frame, frame.px_per_long(w));
            let (x, y) = (cx as usize, cy as usize);
            assert_eq!(img.get(x, y), original.get(x + w / 8, y), "inside copies the source at {w}px");
            assert_eq!(img.get(x + radius as usize, y), original.get(x + radius as usize, y), "radius stays unchanged at {w}px");
            assert!(img.data.iter().flatten().all(|v| v.is_finite()));
        }
    }

    #[test]
    fn remove_erases_a_dot() {
        // flat grey with a gentle gradient and a black dot in the middle
        let mut img = Rgb32f::from_fn(120, 80, |x, _| [0.2 + x as f32 * 0.001; 3]);
        for y in 36..44 {
            for x in 56..64 {
                img.set(x, y, [0.0; 3]);
            }
        }
        let frame = Frame::new(120, 80, &DevelopSettings::default(), true);
        let spot = Spot { points: vec![Point::new(0.5, 0.5)], size: 8.0 / 120.0, feather: 30.0, ..Default::default() };
        apply(&mut img, &[spot], &frame, frame.px_per_long(120));
        let c = img.get(60, 40);
        assert!((c[0] - 0.26).abs() < 0.03, "{c:?}");
    }

    /// A spot with a shape (Tap to Remove) fills the object's own outline, not a stroke: an L-shaped
    /// blot is gone, and so is nothing far from it.
    #[test]
    fn a_shape_spot_removes_the_shape_it_has() {
        use lightcraft_develop::SegMask;
        let (w, h) = (160usize, 120usize);
        let blot = |x: usize, y: usize| ((60..70).contains(&x) && (40..80).contains(&y)) || ((60..100).contains(&x) && (70..80).contains(&y));
        let mut img = Rgb32f::from_fn(w, h, |x, _| [0.3 + x as f32 * 0.001; 3]);
        for y in 0..h {
            for x in 0..w {
                if blot(x, y) {
                    img.set(x, y, [0.0; 3]);
                }
            }
        }
        let cov: Vec<u8> = (0..w * h).map(|i| if blot(i % w, i / w) { 255 } else { 0 }).collect();
        let sel = SegMask::from_coverage_cropped(128, w, h, &cov).unwrap();
        let spot = Spot {
            points: vec![Point::new(sel.center.0, sel.center.1), Point::new(sel.bbox[0], sel.bbox[1]), Point::new(sel.bbox[2], sel.bbox[3])],
            size: 0.01,
            feather: 0.0,
            shape: Some(sel.mask),
            ..Default::default()
        };
        let frame = Frame::new(w, h, &DevelopSettings::default(), true);
        apply(&mut img, &[spot], &frame, frame.px_per_long(w));
        for (x, y) in [(64, 50), (64, 75), (90, 75)] {
            assert!(img.get(x, y)[0] > 0.2, "({x}, {y}) is still the blot: {:?}", img.get(x, y));
        }
        // the corner the L doesn't cover was never touched (a path through its box would have been)
        let c = img.get(90, 50);
        assert!((c[0] - (0.3 + 90.0 * 0.001)).abs() < 1e-4, "{c:?}");
    }

    #[test]
    fn a_shape_spot_with_a_damaged_shape_does_nothing() {
        use lightcraft_develop::SegMask;
        let mut img = Rgb32f::from_fn(40, 40, |_, _| [0.5; 3]);
        let before = img.clone();
        let mut shape = SegMask::from_logits(4, &[5.0; 16]);
        shape.data = "not base64 !".into();
        let spot = Spot { points: vec![Point::new(0.5, 0.5)], shape: Some(shape), ..Default::default() };
        let frame = Frame::new(40, 40, &DevelopSettings::default(), true);
        apply(&mut img, &[spot], &frame, frame.px_per_long(40));
        assert_eq!(img.data, before.data);
    }

    #[test]
    fn the_distance_to_a_shape_counts_pixels() {
        // a single pixel in the middle of 7 × 7
        let mut inside = vec![false; 49];
        inside[3 * 7 + 3] = true;
        let d = distance_to_inside(7, 7, &inside);
        assert_eq!(d[3 * 7 + 3], 0.0);
        assert!((d[3 * 7 + 6] - 3.0).abs() < 1e-5);
        assert!((d[0] - 3.0 * std::f32::consts::SQRT_2).abs() < 1e-4, "{}", d[0]);
        // nothing inside: far from everything
        assert!(distance_to_inside(3, 3, &[false; 9]).iter().all(|v| *v > 1e30));
    }

    /// Four quadrants meet under the spot: healing from one offset can't continue all four edges,
    /// the synthesized fill does.
    #[test]
    fn remove_continues_the_structure_around_it() {
        let q = [[0.05; 3], [0.8, 0.1, 0.1], [0.1, 0.6, 0.1], [0.1, 0.1, 0.9]];
        let mut img = Rgb32f::from_fn(160, 160, |x, y| q[usize::from(x >= 80) + 2 * usize::from(y >= 80)]);
        let frame = Frame::new(160, 160, &DevelopSettings::default(), true);
        let spot = Spot {
            points: vec![Point::new(0.5, 0.5)],
            size: 14.0 / 160.0,
            feather: 0.0,
            source_offset: Some(Point::new(0.3, 0.3)),
            ..Default::default()
        };
        apply(&mut img, &[spot], &frame, frame.px_per_long(160));
        for (x, y, want) in [(72, 72, 0), (88, 72, 1), (72, 88, 2), (88, 88, 3)] {
            let got = img.get(x, y);
            let err: f32 = (0..3).map(|c| (got[c] - q[want][c]).abs()).sum();
            assert!(err < 0.05, "({x}, {y}): {got:?} vs {:?}", q[want]);
        }
    }

    /// A stroke painted fast has its samples far apart (a finger on a phone): it covers the line
    /// between them, not just a dab at each end.
    #[test]
    fn a_stroke_covers_the_gaps_between_its_samples() {
        let mut img = Rgb32f::from_fn(100, 100, |x, _| if x < 50 { [0.1; 3] } else { [0.9; 3] });
        let frame = Frame::new(100, 100, &DevelopSettings::default(), true);
        let spot = Spot {
            mode: SpotMode::Clone,
            points: vec![Point::new(0.1, 0.5), Point::new(0.4, 0.5)],
            size: 0.03,
            feather: 0.0,
            source_offset: Some(Point::new(0.5, 0.0)),
            ..Default::default()
        };
        apply(&mut img, &[spot], &frame, frame.px_per_long(100));
        assert!(img.get(25, 50)[0] > 0.8, "half way between the samples: {:?}", img.get(25, 50));
    }

    /// Vertical stripes (period `period` px) with a black blot over the middle.
    fn striped_with_blot(w: usize, h: usize, period: usize) -> Rgb32f {
        let mut img = Rgb32f::from_fn(w, h, |x, _| if (x / period).is_multiple_of(2) { [0.2; 3] } else { [0.7; 3] });
        let (cx, cy, r) = (w as f32 / 2.0, h as f32 / 2.0, w as f32 * 0.04);
        for y in 0..h {
            for x in 0..w {
                if (x as f32 + 0.5 - cx).hypot(y as f32 + 0.5 - cy) < r {
                    img.set(x, y, [0.0; 3]);
                }
            }
        }
        img
    }

    fn fill_spot() -> Spot {
        // a source offset is ignored by Fill
        Spot {
            mode: SpotMode::Fill,
            points: vec![Point::new(0.5, 0.5)],
            size: 0.06,
            feather: 0.0,
            source_offset: Some(Point::new(0.3, 0.0)),
            ..Default::default()
        }
    }

    #[test]
    fn fill_rebuilds_structure_deterministically() {
        let (w, h) = (200, 160);
        let mut img = striped_with_blot(w, h, 8);
        let frame = Frame::new(w, h, &DevelopSettings::default(), true);
        apply(&mut img, &[fill_spot()], &frame, frame.px_per_long(w));
        // the blot is gone and the stripes continue through it
        let mut err = 0.0f32;
        for x in 90..110 {
            let want = if (x / 8) % 2 == 0 { 0.2 } else { 0.7 };
            err += (img.get(x, 80)[0] - want).abs();
        }
        assert!(err / 20.0 < 0.12, "mean error {}", err / 20.0);
        let mut again = striped_with_blot(w, h, 8);
        apply(&mut again, &[fill_spot()], &frame, frame.px_per_long(w));
        assert_eq!(img.data, again.data);
    }

    #[test]
    fn fill_matches_between_preview_and_export_size() {
        // the same photo at two sizes: the half-size fill matches the full-size one, downsampled
        let (mut big, mut small) = (striped_with_blot(320, 240, 16), striped_with_blot(160, 120, 8));
        let (fb, fs) = (Frame::new(320, 240, &DevelopSettings::default(), true), Frame::new(160, 120, &DevelopSettings::default(), true));
        apply(&mut big, &[fill_spot()], &fb, fb.px_per_long(320));
        apply(&mut small, &[fill_spot()], &fs, fs.px_per_long(160));
        let (mut err, mut n) = (0.0f32, 0);
        for y in 50..70 {
            for x in 70..90 {
                let b: f32 = [(0, 0), (1, 0), (0, 1), (1, 1)].iter().map(|(dx, dy)| big.get(2 * x + dx, 2 * y + dy)[0]).sum::<f32>() / 4.0;
                err += (b - small.get(x, y)[0]).abs();
                n += 1;
            }
        }
        assert!(err / n as f32 <= 0.08, "mean difference {}", err / n as f32);
    }

    #[test]
    fn fill_survives_hostile_spots() {
        let frame = Frame::new(40, 30, &DevelopSettings::default(), true);
        for (p, size) in [((0.0, 0.0), 2.0), ((-5.0, 9.0), 0.1), ((0.5, 0.5), 0.0), ((f64::NAN, 0.5), 0.1), ((0.5, 0.5), f64::INFINITY)] {
            let mut img = Rgb32f::from_fn(40, 30, |x, _| [x as f32 / 40.0; 3]);
            let spot = Spot { mode: SpotMode::Fill, points: vec![Point::new(p.0, p.1)], size, ..Default::default() };
            apply(&mut img, &[spot], &frame, frame.px_per_long(40));
            assert!(img.data.iter().flatten().all(|v| v.is_finite()));
        }
    }

    /// A stand-in for the model: the hole gets the mean of the display values around it.
    fn flat_fill(input: &EraseInput) -> Rgb32f {
        let (mut sum, mut n) = ([0.0f32; 3], 0.0f32);
        for (p, h) in input.image.data.iter().zip(&input.hole) {
            if !h {
                (0..3).for_each(|c| sum[c] += p[c]);
                n += 1.0;
            }
        }
        let mean = sum.map(|v| v / n.max(1.0));
        let mut out = input.image.clone();
        out.data.iter_mut().zip(&input.hole).filter(|(_, h)| **h).for_each(|(p, _)| *p = mean);
        out
    }

    /// Grey with a black square in the middle, and a Remove spot over it baked with `flat_fill`.
    fn baked(n: usize) -> (Rgb32f, DevelopSettings) {
        let img = Rgb32f::from_fn(
            n,
            n,
            |x, y| if (n * 2 / 5..n * 3 / 5).contains(&x) && (n * 2 / 5..n * 3 / 5).contains(&y) { [0.0; 3] } else { [0.3; 3] },
        );
        let mut s = DevelopSettings::default();
        s.spots.push(Spot { points: vec![Point::new(0.5, 0.5)], size: 0.16, feather: 0.0, ..Default::default() });
        let input = erase_input(&img, &crate::SourceInfo::default(), &s, 0).unwrap();
        assert!(input.hole.iter().any(|h| *h) && input.hole.iter().any(|h| !*h));
        assert_eq!(input.hole.len(), input.image.width * input.image.height);
        s.spots[0].fill = input.finish(&flat_fill(&input));
        assert!(s.spots[0].fill.is_some());
        (img, s)
    }

    #[test]
    fn an_ai_fill_is_painted_and_follows_the_photos_tones() {
        let (img, s) = baked(200);
        let frame = Frame::new(200, 200, &s, true);
        // the white balance of the stage doesn't touch a neutral grey; compare with the grey around
        let around = {
            let mut probe = img.clone();
            crate::local::white_balance(&mut probe, &crate::SourceInfo::default(), &s);
            probe.get(10, 10)[1]
        };
        let mut out = img.clone();
        crate::local::white_balance(&mut out, &crate::SourceInfo::default(), &s);
        apply(&mut out, &s.spots, &frame, frame.px_per_long(200));
        let c = out.get(100, 100);
        assert!((c[1] - around).abs() < 0.03 * around.max(0.01), "the square is filled: {c:?} vs {around}");
        // a brighter photo (exposure or white balance before the retouching stage): the fill follows
        let mut bright = img.map(|p| p.map(|v| v * 2.0));
        crate::local::white_balance(&mut bright, &crate::SourceInfo::default(), &s);
        apply(&mut bright, &s.spots, &frame, frame.px_per_long(200));
        let c = bright.get(100, 100);
        assert!((c[1] - 2.0 * around).abs() < 0.06 * around.max(0.01), "tones follow: {c:?} vs {}", 2.0 * around);
        // at another output size the same fill lands in the same place
        let mut small = lightcraft_raster::resample::resize(&img, 100, 100, Filter::Box);
        crate::local::white_balance(&mut small, &crate::SourceInfo::default(), &s);
        apply(&mut small, &s.spots, &frame, frame.px_per_long(100));
        assert!((small.get(50, 50)[1] - around).abs() < 0.03 * around.max(0.01), "{:?}", small.get(50, 50));
    }

    #[test]
    fn a_damaged_ai_fill_falls_back_to_the_usual_remove() {
        let (img, mut s) = baked(120);
        if let Some(f) = s.spots[0].fill.as_mut() {
            f.data = "damaged!".into();
        }
        let frame = Frame::new(120, 120, &s, true);
        let mut out = img.clone();
        apply(&mut out, &s.spots, &frame, frame.px_per_long(120));
        assert!(out.get(60, 60)[1] > 0.2, "filled from around it: {:?}", out.get(60, 60));
    }

    /// A photo that brightens left to right, a disc of it to fill, and a fill of the wrong tone.
    fn gradient_and_disc(n: usize, r: f32) -> (Vec<[f32; 3]>, Vec<f32>) {
        let tgt: Vec<[f32; 3]> = (0..n * n).map(|i| [0.2 + 0.4 * (i % n) as f32 / n as f32; 3]).collect();
        let alpha: Vec<f32> = (0..n * n)
            .map(|i| if ((i % n) as f32 + 0.5 - n as f32 / 2.0).hypot((i / n) as f32 + 0.5 - n as f32 / 2.0) < r { 1.0 } else { 0.0 })
            .collect();
        (tgt, alpha)
    }

    #[test]
    fn a_fill_is_matched_to_the_photo_along_its_whole_edge() {
        let (n, r) = (120, 30.0);
        let (tgt, alpha) = gradient_and_disc(n, r);
        let mut src = vec![[0.35f32; 3]; n * n];
        assert!(boundary_match(&mut src, &tgt, &alpha, (n, n), r));
        // just inside the edge, all the way round, the fill is what the photo is just outside it
        for k in 0..16 {
            let a = k as f32 / 16.0 * std::f32::consts::TAU;
            let at = |rad: f32| ((n as f32 / 2.0 + a.cos() * rad) as usize, (n as f32 / 2.0 + a.sin() * rad) as usize);
            let ((ix, iy), (ox, oy)) = (at(r - 1.5), at(r + 1.5));
            let (inside, outside) = (src[iy * n + ix][1], tgt[oy * n + ox][1]);
            assert!((inside - outside).abs() < 0.02, "edge at {k}: {inside} vs {outside}");
        }
        // nothing outside the area changes, and the middle is the photo's tone there, not the fill's
        assert_eq!(src[0], [0.35; 3]);
        let mid = src[(n / 2) * n + n / 2][1];
        assert!((mid - 0.4).abs() < 0.03, "{mid}");
    }

    #[test]
    fn a_fill_in_other_light_gets_the_photos_tones_by_a_gain_and_keeps_its_grain() {
        let (n, r) = (120, 30.0);
        let (tgt, alpha) = gradient_and_disc(n, r);
        // the same tones, darker by half, with a grain of its own
        let grain = |i: usize| if (i * 7919) % 5 < 2 { 0.01f32 } else { -0.01 };
        let mut src: Vec<[f32; 3]> = tgt.iter().enumerate().map(|(i, p)| p.map(|v| v * 0.5 + grain(i))).collect();
        let before = src.clone();
        assert!(boundary_match(&mut src, &tgt, &alpha, (n, n), r));
        let c = (n / 2) * n + n / 2;
        assert!((src[c][1] - tgt[c][1]).abs() < 0.03, "{} vs {}", src[c][1], tgt[c][1]);
        // the grain keeps its strength (a gain on the whole fill would double it)
        let hp = |v: &[[f32; 3]]| {
            (0..n).map(|x| (v[(n / 2) * n + x][1] - v[(n / 2) * n + x.saturating_sub(1)][1]).abs()).take(n / 2 + 8).skip(n / 2 - 8).sum::<f32>()
        };
        assert!(hp(&src) < 1.3 * hp(&before), "{} vs {}", hp(&src), hp(&before));
    }

    #[test]
    fn an_edge_that_meets_something_else_does_not_colour_the_whole_fill() {
        let (n, r) = (120, 30.0);
        let (_, alpha) = gradient_and_disc(n, r);
        // grey all round, but a red coat hugs the disc on its right
        let tgt: Vec<[f32; 3]> = (0..n * n).map(|i| if i % n >= 80 && alpha[i] == 0.0 { [0.9, 0.1, 0.1] } else { [0.5; 3] }).collect();
        let mut src = vec![[0.5f32; 3]; n * n];
        assert!(boundary_match(&mut src, &tgt, &alpha, (n, n), r));
        // the edge by the coat meets it as far as the spread allows ...
        let by_coat = src[(n / 2) * n + 89];
        assert!(by_coat[1] < 0.4, "{by_coat:?}");
        // ... and the middle keeps most of its own tones (all the way to the coat's, it would be 0.33)
        let mid = src[(n / 2) * n + n / 2];
        assert!(mid[1] > 0.4, "the middle took the coat's colour: {mid:?}");
    }

    #[test]
    fn there_is_no_edge_to_match_on_nothing_or_everything() {
        let (n, r) = (40, 10.0);
        let (tgt, _) = gradient_and_disc(n, r);
        let mut src = vec![[0.3f32; 3]; n * n];
        assert!(!boundary_match(&mut src, &tgt, &vec![0.0; n * n], (n, n), r));
        assert!(!boundary_match(&mut src, &tgt, &vec![1.0; n * n], (n, n), r));
        assert!(!boundary_match(&mut src, &tgt, &[1.0], (n, n), r), "sizes that don't match");
        assert!(src.iter().all(|p| *p == [0.3; 3]));
    }

    #[test]
    fn erase_input_refuses_what_it_cannot_fill() {
        let img = Rgb32f::filled(50, 40, [0.2; 3]);
        let info = crate::SourceInfo::default();
        let mut s = DevelopSettings::default();
        assert!(erase_input(&img, &info, &s, 0).is_none(), "no spot");
        s.spots.push(Spot { points: vec![], ..Default::default() });
        assert!(erase_input(&img, &info, &s, 0).is_none(), "no points");
        s.spots[0].points = vec![Point::new(f64::NAN, 0.5)];
        assert!(erase_input(&img, &info, &s, 0).is_none(), "not a number");
        s.spots[0].points = vec![Point::new(9.0, 9.0)];
        assert!(erase_input(&img, &info, &s, 0).is_none(), "outside the image");
        s.spots[0].points = vec![Point::new(0.5, 0.5)];
        let input = erase_input(&img, &info, &s, 0).unwrap();
        assert!(input.finish(&Rgb32f::new(3, 3)).is_none(), "an answer of the wrong size");
    }

    #[test]
    fn clone_copies_source() {
        let mut img = Rgb32f::from_fn(100, 100, |x, _| if x < 50 { [0.1; 3] } else { [0.9; 3] });
        let frame = Frame::new(100, 100, &DevelopSettings::default(), true);
        let spot = Spot {
            mode: SpotMode::Clone,
            points: vec![Point::new(0.25, 0.5)],
            size: 0.05,
            feather: 0.0,
            source_offset: Some(Point::new(0.5, 0.0)),
            ..Default::default()
        };
        apply(&mut img, &[spot], &frame, frame.px_per_long(100));
        assert!(img.get(25, 50)[0] > 0.8);
        assert!(img.get(5, 50)[0] < 0.2);
    }

    fn with_spots(spots: Vec<Spot>) -> DevelopSettings {
        DevelopSettings { spots, ..Default::default() }
    }

    fn spot_at(x: f64, y: f64, size: f64, offset: Option<Point>) -> Spot {
        Spot { points: vec![Point::new(x, y)], size, source_offset: offset, ..Default::default() }
    }

    #[test]
    fn a_window_grows_to_hold_the_spots_that_touch_it_and_their_sources() {
        use crate::PixelWindow;
        let frame = Frame::new(4000, 3000, &DevelopSettings::default(), true);
        let ppl = frame.px_per_long(4000);
        let win = PixelWindow { x: 1000, y: 1000, w: 400, h: 300 };
        // none: the window is left alone
        assert_eq!(window_for_reads(&with_spots(vec![]), &frame, 4000, 3000, ppl, win), win);
        // a spot far away is no business of this window
        let far = spot_at(0.9, 0.9, 0.01, Some(Point::new(0.02, 0.0)));
        assert_eq!(window_for_reads(&with_spots(vec![far]), &frame, 4000, 3000, ppl, win), win);
        // a spot inside it, reading from 300 px to the right, widens it to the source
        let near = spot_at(0.30, 0.38, 0.01, Some(Point::new(0.075, 0.0)));
        let g = window_for_reads(&with_spots(vec![near]), &frame, 4000, 3000, ppl, win);
        assert!(g.x <= win.x && g.x + g.w >= 1200 + 300 + 80 && g.y <= win.y && g.y + g.h >= win.y + win.h, "{g:?}");
        // a spot that only reaches in with its edge counts too
        let edge = spot_at(0.24, 0.38, 0.01, Some(Point::new(-0.05, 0.0)));
        assert!(window_for_reads(&with_spots(vec![edge]), &frame, 4000, 3000, ppl, win).x < win.x);
    }

    #[test]
    fn spot_windows_are_bounded_and_survive_hostile_spots() {
        use crate::PixelWindow;
        let frame = Frame::new(40_000, 30_000, &DevelopSettings::default(), true);
        let ppl = frame.px_per_long(40_000);
        let win = PixelWindow { x: 1000, y: 1000, w: 400, h: 300 };
        // a spot whose source is a long way off would need an enormous window: not granted
        let wide = spot_at(0.03, 0.04, 0.01, Some(Point::new(0.6, 0.0)));
        let g = window_for_reads(&with_spots(vec![wide]), &frame, 40_000, 30_000, ppl, win);
        assert!(g.w <= MAX_SPOT_SPAN && g.h <= MAX_SPOT_SPAN, "{g:?}");
        for bad in [f64::NAN, f64::INFINITY, 1e300, -1e300] {
            let s = spot_at(bad, bad, bad, Some(Point::new(bad, bad)));
            let g = window_for_reads(&with_spots(vec![s]), &frame, 40_000, 30_000, ppl, win);
            assert_eq!(g, win);
        }
        let empty = Spot { points: vec![], ..Default::default() };
        assert_eq!(window_for_reads(&with_spots(vec![empty]), &frame, 40_000, 30_000, ppl, win), win);
    }

    // Given an Auto Mask stroke reaching into the window, the window grows to hold all of it
    #[test]
    fn a_window_grows_to_hold_an_auto_mask_stroke() {
        use crate::PixelWindow;
        use lightcraft_develop::{BrushStroke, Mask, MaskComponent, MaskOp, MaskShape};
        let frame = Frame::new(4000, 3000, &DevelopSettings::default(), true);
        let ppl = frame.px_per_long(4000);
        let win = PixelWindow { x: 1000, y: 1000, w: 400, h: 300 };
        let stroke =
            |auto| BrushStroke { points: vec![Point::new(0.30, 0.38), Point::new(0.5, 0.38)], size: 0.01, auto_mask: auto, ..Default::default() };
        let mask = |auto| Mask {
            components: vec![MaskComponent { name: None, op: MaskOp::Add, invert: false, shape: MaskShape::Brush { strokes: vec![stroke(auto)] } }],
            ..Default::default()
        };
        let plain = DevelopSettings { masks: vec![mask(false)], ..Default::default() };
        assert_eq!(window_for_reads(&plain, &frame, 4000, 3000, ppl, win), win, "a plain stroke reads nothing around it");
        let auto = DevelopSettings { masks: vec![mask(true)], ..Default::default() };
        let g = window_for_reads(&auto, &frame, 4000, 3000, ppl, win);
        // the dabs near the window, not the whole stroke (it runs on to x = 2000)
        assert!(g.x <= 1000 && g.x + g.w > 1400 && g.x + g.w < 1700, "{g:?}");
        // a window at the far end of a long stroke does not grow back along all of it
        let far = window_for_reads(&auto, &frame, 4000, 3000, ppl, PixelWindow { x: 1900, ..win });
        assert!(far.x > 1500, "{far:?}");
        // a hidden mask reads nothing
        let hidden = DevelopSettings { masks: vec![Mask { visible: false, ..auto.masks[0].clone() }], ..Default::default() };
        assert_eq!(window_for_reads(&hidden, &frame, 4000, 3000, ppl, win), win);
    }

    // Given a window that would have to grow past the cap, there is no answer (never a wrong one)
    #[test]
    fn a_window_that_cannot_hold_what_it_reads_is_refused() {
        use crate::PixelWindow;
        let frame = Frame::new(40_000, 30_000, &DevelopSettings::default(), true);
        let ppl = frame.px_per_long(40_000);
        let win = PixelWindow { x: 1000, y: 1000, w: 400, h: 300 };
        let far = with_spots(vec![spot_at(0.03, 0.04, 0.01, Some(Point::new(0.6, 0.0)))]);
        assert_eq!(window_for_reads_checked(&far, &frame, 40_000, 30_000, ppl, win), None);
        let near = with_spots(vec![spot_at(0.03, 0.04, 0.01, Some(Point::new(0.02, 0.0)))]);
        assert!(window_for_reads_checked(&near, &frame, 40_000, 30_000, ppl, win).is_some());
        // a spot that doesn't touch the window is no business of it, however far it reads
        let elsewhere = with_spots(vec![spot_at(0.9, 0.9, 0.01, Some(Point::new(-0.8, 0.0)))]);
        assert_eq!(window_for_reads_checked(&elsewhere, &frame, 40_000, 30_000, ppl, win), Some(win));
    }
}
