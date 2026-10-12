//! Remove / Heal / Clone spots, rendered at output resolution in scene-linear light.
//!
//! - **Clone** copies the source patch with a feathered edge.
//! - **Heal** copies the source's *detail* and keeps the target's *tone*: result = src + blur(target − src)
//!   (frequency separation — a fast, stable approximation of gradient-domain healing).
//! - **Remove** fills the stroke with texture synthesized from around it ([`crate::inpaint`]), seeded
//!   by and searching the spot's source too; it heals from the source when nothing around fits.
//!   With **AI Remove** the spot carries what a model painted under it ([`SpotFill`], made by
//!   [`erase_input`] and [`EraseInput::finish`] around the model), matched to the photo's current
//!   tones from the stroke's surroundings as Heal does, so a later white balance change follows.
//!   Automatic sources: candidate offsets on rings around the spot are scored by how well the
//!   source's surrounding annulus matches the target's.

use lightcraft_develop::{DevelopSettings, Spot, SpotFill, SpotMode};
use lightcraft_geom::{Affine, CropGeometry, Point, Rect};
use lightcraft_raster::Rgb32f;
use lightcraft_raster::resample::{Filter, fit};

use crate::geometry::Frame;

#[inline]
fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
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
    let r = (spot.size * frame.px_per_long(w)).max(1.0) as f32;
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

/// Apply all spots to `img` (output resolution). `ppl` = output pixels per long-edge unit.
pub fn apply(img: &mut Rgb32f, spots: &[Spot], frame: &Frame, ppl: f64) {
    if spots.is_empty() {
        return;
    }
    let (w, h) = (img.width, img.height);
    let to_out = frame.norm_to_out(w, h);
    for spot in spots {
        let r = (spot.size * ppl).max(1.0) as f32;
        let pts: Vec<(f32, f32)> = spot
            .points
            .iter()
            .map(|p| {
                let q = to_out.apply(*p);
                (q.x as f32, q.y as f32)
            })
            .collect();
        let Some(&first) = pts.first() else { continue };
        let offset = match spot.source_offset {
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
        let pad = r;
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
        for y in 0..bh {
            for x in 0..bw {
                let (px, py) = ((bx0 + x) as f32 + 0.5, (by0 + y) as f32 + 0.5);
                let d = dist_to_path(&pts, (px, py));
                let inner = r * (1.0 - feather * 0.8);
                alpha[y * bw + x] = (1.0 - smooth(inner, r, d)) * opacity;
                if let Some(m) = hole.get_mut(y * bw + x) {
                    *m = d < r;
                }
                src[y * bw + x] = img.sample_bilinear(px + offset.0, py + offset.1);
                tgt[y * bw + x] = img.get(bx0 + x, by0 + y);
            }
        }
        // AI Remove: the stored fill, tone-matched below like a heal
        let baked = if remove { spot.fill.as_ref().and_then(|f| sample_fill(f, &frame.out_to_norm(w, h), (bx0, by0, bw), &tgt)) } else { None };
        let tone_match = baked.is_some();
        // Remove: synthesize; the seed follows the spot, so a fill doesn't change when others do
        let seed = spot.points.first().map_or(0, |p| p.x.to_bits() ^ p.y.to_bits().rotate_left(32));
        let filled = match baked {
            // the model's fill gives the structure, the synthesized one the photo's grain and fine texture
            Some(ai) => Some(match crate::inpaint::fill(img, bx0, by0, bw, bh, &hole, r, offset, seed) {
                Some(pm) => with_detail(ai, pm, bw, bh, (r * DETAIL).max(1.0)),
                None => ai,
            }),
            None if remove => crate::inpaint::fill(img, bx0, by0, bw, bh, &hole, r, offset, seed),
            None => None,
        };
        let heal = match filled {
            Some(f) => {
                src = f;
                tone_match
            }
            None => spot.mode != SpotMode::Clone,
        };
        let low = if heal {
            // Tone correction from the spot's *surroundings* only: normalized convolution of
            // (target − source) over pixels outside the stroke (a membrane-like interpolation).
            let outside: Vec<f32> = alpha.iter().map(|a| if *a < 0.02 { 1.0 } else { 0.0 }).collect();
            let diff = Rgb32f {
                width: bw,
                height: bh,
                data: tgt.iter().zip(&src).zip(&outside).map(|((t, s), m)| [(t[0] - s[0]) * m, (t[1] - s[1]) * m, (t[2] - s[2]) * m]).collect(),
            };
            let wts = lightcraft_raster::Plane { width: bw, height: bh, data: outside };
            let sigma = (r * 0.6).max(1.0);
            let (bd, bm) = (lightcraft_raster::blur::gaussian(&diff, sigma), lightcraft_raster::blur::gaussian(&wts, sigma));
            Some(Rgb32f {
                width: bw,
                height: bh,
                data: bd
                    .data
                    .iter()
                    .zip(&bm.data)
                    .map(|(d, m)| {
                        let m = m.max(1e-4);
                        [d[0] / m, d[1] / m, d[2] / m]
                    })
                    .collect(),
            })
        } else {
            None
        };
        for y in 0..bh {
            for x in 0..bw {
                let i = y * bw + x;
                let a = alpha[i];
                if a <= 0.0 {
                    continue;
                }
                let mut v = src[i];
                if let Some(l) = &low {
                    let d = l.data[i];
                    v = [v[0] + d[0], v[1] + d[1], v[2] + d[2]];
                }
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
    let hole: Vec<bool> = (0..w * h)
        .map(|i| dist_to_path(&pts32, ((x0 + ((i % w) as f64 + 0.5) * kx) as f32, (y0 + ((i / w) as f64 + 0.5) * ky) as f32)) < reach)
        .collect();
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
}
