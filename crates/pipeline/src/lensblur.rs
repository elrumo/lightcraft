//! **Lens Blur**: a shallow depth of field made after the fact from the photo's depth.
//!
//! The depth is *nearness* over the EXIF-oriented source (1 nearest, 0 farthest, linear in inverse
//! distance), carried by the source's [`Mattes`]: the photo's own depth map (a portrait HEIC's,
//! a DNG's), or the estimate Lens Blur stored in the settings ([`mattes_for`]). Each output pixel
//! gets a *circle of confusion*: zero inside the focal range, growing linearly with the distance
//! in nearness outside it, as a lens's blur grows with the difference in inverse distance. Its size
//! is relative to the photo's long edge, so previews and exports match.
//!
//! The blur is rendered by gathering: each pixel collects the neighbours whose own blur shape
//! (the bokeh: a disc, a pentagon, a ring…) reaches it, weighted by the energy each spreads over its
//! shape. Neighbours nearer than the pixel are kept apart and laid over it with the coverage they
//! reach (a blurred foreground spills over a sharp subject); neighbours farther away can't spill
//! over a pixel sharper than them (a sharp subject keeps its edge against a blurred background).
//! Large blurs are gathered at a reduced size where the biggest circle is about [`R_WORK_FULL`]
//! pixels (the result is smooth there anyway); the transitions near the focal range are gathered
//! at full size; both are joined by circle size. Highlights bloom when Boost is up. The depth's
//! edges are snapped to the photo's with a guided filter first, so the subject's outline stays
//! sharp although the depth map is coarse.

use std::sync::Arc;

use lightcraft_develop::{Bokeh, DevelopSettings, MaskShape};
use lightcraft_raster::resample::{Filter, resize};
use lightcraft_raster::{Plane, Rgb32f};

use crate::for_rows;
use crate::geometry::Frame;
use crate::masks::{Mattes, guided_cross, source_to_out};

/// Blur radius at Blur Amount 100 between the nearest and the farthest part of the photo, as a
/// fraction of the source's long edge.
pub const MAX_RADIUS: f64 = 0.04;
/// The largest circle (pixels) a full-quality render gathers at its working size.
pub const R_WORK_FULL: f32 = 16.0;
/// … and a draft (interactive drags).
const R_WORK_DRAFT: f32 = 8.0;
/// Most taps a pixel gathers (full quality, draft).
const TAPS_FULL: usize = 160;
const TAPS_DRAFT: usize = 64;
/// Taps per square pixel of the gathering disc.
const DENSITY: f32 = 0.7;
/// Highlight gain at Boost 100.
const BOOST_MAX: f32 = 4.0;
/// Tile size of the local circle bound.
const TILE: usize = 16;
/// How far toward the frame's centre a cat's-eye shape is clipped at the corners (circle radii).
const CAT_EYE: f32 = 0.9;
/// Largest output (pixels) the depth's edges are snapped at; bigger renders enlarge the result.
const GUIDE_MAX_PIXELS: usize = 4 << 20;

/// Normalize a relative inverse depth or disparity map to nearness 0..1: its 1st percentile
/// becomes 0, its 99th 1, the rest is clamped (a few stray values don't squash the range).
pub fn normalize(p: &mut Plane) {
    let mut v: Vec<f32> = p.data.iter().copied().filter(|v| v.is_finite()).step_by((p.data.len() / 65536).max(1)).collect();
    if v.is_empty() {
        p.data.iter_mut().for_each(|x| *x = 0.0);
        return;
    }
    v.sort_by(f32::total_cmp);
    let at = |q: f32| v.get(((v.len() - 1) as f32 * q).round() as usize).copied().unwrap_or(0.0);
    let (lo, hi) = (at(0.01), at(0.99));
    let span = (hi - lo).max(1e-6);
    for x in p.data.iter_mut() {
        *x = if x.is_finite() { ((*x - lo) / span).clamp(0.0, 1.0) } else { 0.0 };
    }
}

/// Whether a render of `s` reads depth: Lens Blur is on, or a visible mask has a Depth Range.
pub fn wants_depth(s: &DevelopSettings) -> bool {
    s.lens_blur.active() || s.masks.iter().filter(|m| m.visible).flat_map(|m| &m.components).any(|c| matches!(c.shape, MaskShape::DepthRange { .. }))
}

/// The scene data a render of `s` uses: the source's `mattes`, with Lens Blur's stored depth
/// estimate in place of the source's depth when the settings have one (and need depth).
pub fn mattes_for(s: &DevelopSettings, source: Option<&Arc<Mattes>>) -> Option<Arc<Mattes>> {
    let stored = s.lens_blur.depth.as_ref().filter(|_| wants_depth(s)).and_then(|d| {
        let v = d.nearness()?;
        Some(Arc::new(Plane { width: d.w as usize, height: d.h as usize, data: v }))
    });
    match stored {
        Some(d) => Some(Arc::new(source.map_or_else(Mattes::default, |m| (**m).clone()).with_depth(d))),
        None => source.cloned(),
    }
}

/// The guided filter's radius (output px) that snaps a depth map of long side `depth_long` to the
/// photo at `px_per_long` output pixels per source long edge: about two depth pixels; `None` when
/// the depth is as fine as the output.
fn snap_sigma(px_per_long: f64, depth_long: usize) -> Option<f64> {
    let k = px_per_long / depth_long.max(1) as f64;
    (k >= 1.5).then_some(2.0 * k)
}

/// Nearness at output resolution, its edges snapped to the photo's: the depth sampled like the
/// image, then guided by `guide` (log luminance at output resolution) over about two of the
/// depth map's own pixels. Above [`GUIDE_MAX_PIXELS`] the snapping is done smaller and enlarged.
pub fn nearness_out(depth: &Plane, frame: &Frame, w: usize, h: usize, guide: &Plane) -> Plane {
    if (guide.width, guide.height) != (w, h) || depth.width == 0 || w == 0 || h == 0 {
        return source_to_out(depth, frame, w, h);
    }
    let f = ((w * h) as f64 / GUIDE_MAX_PIXELS as f64).sqrt().max(1.0);
    let (gw, gh) = (((w as f64 / f).round() as usize).max(1), ((h as f64 / f).round() as usize).max(1));
    let sigma = snap_sigma(frame.px_per_long(w), depth.width.max(depth.height));
    let near = if (gw, gh) == (w, h) { source_to_out(depth, frame, w, h) } else { resize(&source_to_out(depth, frame, w, h), gw, gh, Filter::Box) };
    let Some(sigma) = sigma else {
        return if (gw, gh) == (w, h) { near } else { resize(&near, w, h, Filter::Bilinear) };
    };
    let g = if (gw, gh) == (w, h) { std::borrow::Cow::Borrowed(guide) } else { std::borrow::Cow::Owned(resize(guide, gw, gh, Filter::Box)) };
    let snapped = guided_cross(&g, &near, (sigma / f) as f32, 0.04);
    if (gw, gh) == (w, h) { snapped } else { resize(&snapped, w, h, Filter::Bilinear) }
}

/// The nearness (0..1) at normalized image point `p` (the coordinates of masks and spots) of
/// photo `src` rendered with `s`: the median over a few pixels around it. `None` without depth
/// (the photo's own or a stored estimate) or outside the photo.
pub fn nearness_at(src: &Rgb32f, info: &crate::SourceInfo, s: &DevelopSettings, p: lightcraft_geom::Point) -> Option<f32> {
    let mut with_depth = s.clone();
    with_depth.lens_blur.enabled = true;
    with_depth.lens_blur.amount = with_depth.lens_blur.amount.max(1.0);
    let req = crate::RenderRequest { apply_crop: false, ..crate::RenderRequest::fit(512, 512) };
    let plan = crate::plan(src, info, &with_depth, &req);
    let depth = plan.mattes.as_deref().and_then(Mattes::depth)?;
    let near = source_to_out(depth, &plan.frame, plan.w, plan.h);
    let o = plan.frame.norm_to_out(plan.w, plan.h).apply(p);
    let (cx, cy) = (o.x.floor() as i64, o.y.floor() as i64);
    if cx < 0 || cy < 0 || cx >= plan.w as i64 || cy >= plan.h as i64 {
        return None;
    }
    let mut v: Vec<f32> = Vec::with_capacity(25);
    for y in (cy - 2).max(0)..=(cy + 2).min(plan.h as i64 - 1) {
        for x in (cx - 2).max(0)..=(cx + 2).min(plan.w as i64 - 1) {
            v.extend(near.data.get(y as usize * plan.w + x as usize));
        }
    }
    v.sort_by(f32::total_cmp);
    v.get(v.len() / 2).copied()
}

/// The focal distance (0–100) that puts the photo's subject in focus, from `depth` (nearness over
/// the oriented source) and the source's `mattes`: the median depth under its person matte when
/// it has one (a portrait's), else the nearer part of the middle of the frame (the 75th percentile
/// of the nearness of its central half), where a subject usually is.
pub fn subject_focus(depth: &Plane, mattes: Option<&Mattes>) -> f64 {
    let (w, h) = (depth.width, depth.height);
    if w == 0 || h == 0 {
        return 0.0;
    }
    let people = mattes.map(|m| m.for_shape(&MaskShape::Subject)).unwrap_or_default();
    let mut v: Vec<f32> = Vec::new();
    for (i, n) in depth.data.iter().enumerate() {
        let (fx, fy) = (((i % w) as f32 + 0.5) / w as f32, ((i / w) as f32 + 0.5) / h as f32);
        let picked = if people.is_empty() {
            (0.25..0.75).contains(&fx) && (0.25..0.75).contains(&fy)
        } else {
            people.iter().any(|m| {
                let (mx, my) = (((fx * m.width as f32) as usize).min(m.width - 1), ((fy * m.height as f32) as usize).min(m.height - 1));
                m.data.get(my * m.width + mx).is_some_and(|&a| a >= 128)
            })
        };
        if picked {
            v.push(*n);
        }
    }
    if v.is_empty() {
        v = depth.data.clone();
    }
    v.sort_by(f32::total_cmp);
    let q = if people.is_empty() { 0.75 } else { 0.5 };
    let n = v.get(((v.len() - 1) as f32 * q).round() as usize).copied().unwrap_or(1.0);
    (100.0 * (1.0 - f64::from(n))).clamp(0.0, 100.0)
}

/// What a render's Lens Blur does, resolved from the settings and the depth.
#[derive(Clone, Debug)]
pub struct Setup {
    depth: Arc<Plane>,
    /// The sharp range in nearness, `[lo, hi]`.
    focus: [f32; 2],
    /// Circle radius (output px) per unit of nearness outside the focal range.
    radius: f32,
    /// The largest circle in the photo (output px).
    max_coc: f32,
    /// How far the depth's edge snapping reads (output px).
    snap_reach: f32,
    boost: f32,
    bokeh: Bokeh,
    /// The exposure gain (highlights bloom by their exposed brightness).
    gain: f32,
    /// Stage-cache key of all of the above.
    pub key: u64,
}

impl Setup {
    /// Lens Blur for settings `s` with the scene data `mattes`, at `px_per_long` output pixels
    /// per unit of the source's long edge; `None` when it blurs nothing (off, no depth, or nothing
    /// outside the focal range).
    pub fn new(s: &DevelopSettings, mattes: Option<&Mattes>, px_per_long: f64) -> Option<Setup> {
        let lb = &s.lens_blur;
        if !lb.active() {
            return None;
        }
        let depth = mattes?.depth()?.clone();
        let [near, far] = lb.focus_bounds();
        let focus = [1.0 - far as f32 / 100.0, 1.0 - near as f32 / 100.0];
        let radius = (lb.amount.clamp(0.0, 100.0) / 100.0 * MAX_RADIUS * px_per_long) as f32;
        let (lo, hi) = depth.data.iter().fold((1.0f32, 0.0f32), |(a, b), &v| (a.min(v), b.max(v)));
        let max_coc = radius * (hi - focus[1]).max(focus[0] - lo).max(0.0);
        if !(max_coc >= 0.5) {
            return None;
        }
        let boost = (lb.boost.clamp(0.0, 100.0) / 100.0) as f32;
        let gain = if boost > 0.0 { (s.light.exposure as f32).exp2() } else { 1.0 };
        // the depth by content: a stored estimate is decoded afresh for every render
        let depth_id = match &lb.depth {
            Some(d) => crate::hash_of(&d.data),
            None => crate::hash_of(Arc::as_ptr(&depth) as usize),
        };
        let key = crate::hash_of((
            depth_id,
            depth.width,
            depth.height,
            focus.map(f32::to_bits),
            radius.to_bits(),
            boost.to_bits(),
            lb.bokeh.name(),
            gain.to_bits(),
        ));
        // (a Gaussian reads about three of its radii)
        let snap_reach = snap_sigma(px_per_long, depth.width.max(depth.height)).map_or(0.0, |s| 3.0 * s as f32);
        Some(Setup { depth, focus, radius, max_coc, snap_reach, boost, bokeh: lb.bokeh, gain, key })
    }

    /// Pixels the blur reads beyond a pixel (for windowed renders): its largest circle, and the
    /// depth's edge snapping before it.
    pub fn reach(&self) -> usize {
        (self.max_coc + self.snap_reach).ceil() as usize + 4
    }

    /// The signed circle radius (output px) at nearness `n`: positive nearer than the focal range,
    /// negative beyond it.
    fn coc(&self, n: f32) -> f32 {
        let [lo, hi] = self.focus;
        if n > hi {
            (n - hi) * self.radius
        } else if n < lo {
            (n - lo) * self.radius
        } else {
            0.0
        }
    }
}

#[inline]
fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[inline]
fn luma(c: [f32; 3]) -> f32 {
    0.2627 * c[0] + 0.6780 * c[1] + 0.0593 * c[2]
}

/// Log luminance of `img` (the guide for the depth's edges).
fn log_luma(img: &Rgb32f) -> Plane {
    let mut p = Plane::new(img.width, img.height);
    let w = img.width;
    for_rows(&mut p.data, w, |y, row| {
        for (x, v) in row.iter_mut().enumerate() {
            *v = luma(img.data[y * w + x]).max(1e-6).log2();
        }
    });
    p
}

/// Apply Lens Blur to `img` (the scene-linear image at output resolution, before exposure)
/// rendered through `frame`. `origin`: where `img` sits in the whole output frame (a zoomed
/// window's top-left corner, else zero), so that a window is gathered exactly like the same pixels
/// of the whole frame. `draft`: fewer taps on a coarser grid (interactive drags).
pub fn apply(img: &mut Rgb32f, setup: &Setup, frame: &Frame, origin: [usize; 2], draft: bool) {
    let (w, h) = (img.width, img.height);
    if w == 0 || h == 0 || img.data.len() != w * h {
        return;
    }
    crate::timed("lens blur", || {
        let coc: Vec<f32> = nearness_out(&setup.depth, frame, w, h, &log_luma(img)).data.into_iter().map(|n| setup.coc(n)).collect();
        let shape = Shape::new(setup.bokeh, frame, w, h);
        let r_work = if draft { R_WORK_DRAFT } else { R_WORK_FULL };
        let taps_max = if draft { TAPS_DRAFT } else { TAPS_FULL };
        let k = (r_work / setup.max_coc).min(1.0);
        let src = img.clone();
        let full = Src::new(&src.data, &coc, w, h, origin, setup);
        let to_out = (1.0, [0.0, 0.0]);
        if k >= 0.999 {
            // small blurs: one pass at full size
            let g = Gatherer::new(&full, &shape, to_out, taps_max, setup.max_coc, None);
            for_rows(&mut img.data, w, |y, row| {
                for (x, p) in row.iter_mut().enumerate() {
                    if let Some(c) = g.at(x, y) {
                        *p = c.out();
                    }
                }
            });
            return;
        }
        // the large blurs on the working grid: working pixel J of the whole frame covers output
        // pixels [J / k, (J + 1) / k)
        let first = origin.map(|o| (o as f32 * k).floor() as usize);
        let cw = (((origin[0] + w) as f32 * k).ceil() as usize).saturating_sub(first[0]).max(1);
        let ch = (((origin[1] + h) as f32 * k).ceil() as usize).saturating_sub(first[1]).max(1);
        let (small, small_coc) = shrink(&src, &coc, origin, first, cw, ch, k);
        let small_src = Src::new(&small.data, &small_coc, cw, ch, first, setup);
        let coarse_out = (1.0 / k, [first[0] as f32 / k - origin[0] as f32, first[1] as f32 / k - origin[1] as f32]);
        let coarse = Coarse::gather(&small_src, &shape, coarse_out, taps_max, setup.max_coc * k);
        // the transitions near the focal range at full size: circles up to two working pixels
        let r_fine = 2.0 / k;
        let fine = Gatherer::new(&full, &shape, to_out, taps_max, r_fine, Some(r_fine));
        for_rows(&mut img.data, w, |y, row| {
            for (x, p) in row.iter_mut().enumerate() {
                let c = coc[y * w + x];
                let t = smooth(0.5 * r_fine, r_fine, c.abs());
                let f = if t < 1.0 { fine.at(x, y).map_or(*p, |g| g.out()) } else { *p };
                let at = |o: usize, l: usize, f: usize| ((o + l) as f32 + 0.5) * k - 0.5 - f as f32;
                let (bg, fg, a) = coarse.upsample(at(origin[0], x, first[0]), at(origin[1], y, first[1]), c * k);
                let base: [f32; 3] = std::array::from_fn(|j| f[j] + (bg[j] - f[j]) * t);
                *p = std::array::from_fn(|j| base[j] + (fg[j] - base[j]) * a);
            }
        });
    });
}

/// `img` and `coc` (output pixels from `origin` of the whole frame) on the working grid: `cw × ch`
/// working pixels from `first`, each the box of output pixels it covers, each colour weighted by
/// how blurred it is, so a sharp subject's colour doesn't bleed into the blurred surroundings at
/// the coarse size. The circles are scaled by `k` to working pixels.
fn shrink(img: &Rgb32f, coc: &[f32], origin: [usize; 2], first: [usize; 2], cw: usize, ch: usize, k: f32) -> (Rgb32f, Vec<f32>) {
    let (w, h) = (img.width, img.height);
    // the output pixels working pixel `j` covers, clipped to the image (at least the nearest one)
    let span = |j: usize, f: usize, o: usize, n: usize| {
        let lo = (((j + f) as f32 / k).floor() as usize).saturating_sub(o).min(n - 1);
        let hi = ((((j + f + 1) as f32 / k).ceil() as usize).saturating_sub(o)).clamp(lo + 1, n);
        lo..hi
    };
    let mut out = vec![([0f32; 3], 0f32); cw * ch];
    for_rows(&mut out, cw, |y, row| {
        let ys = span(y, first[1], origin[1], h);
        for (x, o) in row.iter_mut().enumerate() {
            let (mut acc, mut cacc, mut wsum) = ([0f32; 3], 0f32, 0f32);
            for yy in ys.clone() {
                for xx in span(x, first[0], origin[0], w) {
                    let i = yy * w + xx;
                    let c = coc[i] * k;
                    let wt = 0.02 + smooth(0.5, 2.0, c.abs());
                    let p = img.data[i];
                    for j in 0..3 {
                        acc[j] += p[j] * wt;
                    }
                    cacc += c * wt;
                    wsum += wt;
                }
            }
            let ws = wsum.max(1e-9);
            *o = (acc.map(|v| v / ws), cacc / ws);
        }
    });
    let img = Rgb32f { width: cw, height: ch, data: out.iter().map(|o| o.0).collect() };
    (img, out.iter().map(|o| o.1).collect())
}

/// Pixels to gather from: colours, their signed circles (in these pixels), where they sit in the
/// whole frame's grid, and the highlight boost.
struct Src<'a> {
    px: &'a [[f32; 3]],
    coc: &'a [f32],
    w: usize,
    h: usize,
    /// The whole frame's index of pixel (0, 0) (on this grid).
    origin: [usize; 2],
    boost: f32,
    gain: f32,
    /// The colours averaged over blocks of 2, 4, 8… pixels of the whole frame's grid, so that
    /// sparse taps read light averaged over the space between them: a small highlight then
    /// spreads into an even disc.
    mips: Vec<Rgb32f>,
}

/// Block averages kept for sparse taps (taps up to ~8 px apart).
const MIPS: usize = 3;

impl<'a> Src<'a> {
    fn new(px: &'a [[f32; 3]], coc: &'a [f32], w: usize, h: usize, origin: [usize; 2], setup: &Setup) -> Src<'a> {
        let mut mips: Vec<Rgb32f> = Vec::with_capacity(MIPS);
        for l in 1..=MIPS {
            let s = 1usize << l;
            if w < s || h < s {
                break;
            }
            // block `b` covers the pixels [b·s − r, (b + 1)·s − r), aligned to the whole frame
            let r = origin.map(|o| o % s);
            let (mw, mh) = ((w + r[0]).div_ceil(s), (h + r[1]).div_ceil(s));
            let mut m = Rgb32f::new(mw, mh);
            for_rows(&mut m.data, mw, |by, row| {
                let ys = (by * s).saturating_sub(r[1])..((by + 1) * s - r[1]).min(h);
                for (bx, o) in row.iter_mut().enumerate() {
                    let xs = (bx * s).saturating_sub(r[0])..((bx + 1) * s - r[0]).min(w);
                    let mut acc = [0f32; 3];
                    let mut n = 0.0;
                    for y in ys.clone() {
                        for x in xs.clone() {
                            let p = px.get(y * w + x).copied().unwrap_or([0.0; 3]);
                            for j in 0..3 {
                                acc[j] += p[j];
                            }
                            n += 1.0;
                        }
                    }
                    *o = acc.map(|v| v / f32::max(n, 1.0));
                }
            });
            mips.push(m);
        }
        Src { px, coc, w, h, origin, boost: setup.boost, gain: setup.gain, mips }
    }

    /// `c` brightened as a highlight blooms when its circle is `r` (Boost).
    #[inline]
    fn bloom(&self, c: [f32; 3], r: f32) -> [f32; 3] {
        if self.boost <= 0.0 || r < 1.0 {
            return c;
        }
        let g = 1.0 + BOOST_MAX * self.boost * smooth(0.7, 1.6, luma(c) * self.gain) * smooth(1.0, 3.0, r);
        c.map(|v| v * g)
    }

    /// Colour at a continuous position (pixel centres at +0.5), edges extended, averaged over
    /// about `spacing` pixels (trilinear between the block averages).
    #[inline]
    fn fetch(&self, x: f32, y: f32, spacing: f32) -> [f32; 3] {
        let lod = spacing.max(1.0).log2().min(self.mips.len() as f32);
        let l = lod.floor() as usize;
        let t = lod - l as f32;
        let a = self.fetch_level(l, x, y);
        if t < 0.05 {
            return a;
        }
        let b = self.fetch_level(l + 1, x, y);
        std::array::from_fn(|j| a[j] + (b[j] - a[j]) * t)
    }

    /// Bilinear colour of block level `l` (0 = the pixels themselves) at position `(x, y)`.
    #[inline]
    fn fetch_level(&self, l: usize, x: f32, y: f32) -> [f32; 3] {
        let (data, w, h, s, r) = match l.checked_sub(1).and_then(|i| self.mips.get(i)) {
            Some(m) => {
                let s = 1usize << l;
                (&m.data[..], m.width, m.height, s as f32, self.origin.map(|o| (o % s) as f32))
            }
            None => (self.px, self.w, self.h, 1.0, [0.0; 2]),
        };
        let (fx, fy) = (((x + r[0]) / s - 0.5).clamp(0.0, (w - 1) as f32), ((y + r[1]) / s - 0.5).clamp(0.0, (h - 1) as f32));
        let (x0, y0) = (fx as usize, fy as usize);
        let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let px = |x: usize, y: usize| data.get(y * w + x).copied().unwrap_or([0.0; 3]);
        let (a, b, c, d) = (px(x0, y0), px(x1, y0), px(x0, y1), px(x1, y1));
        std::array::from_fn(|j| {
            let top = a[j] + (b[j] - a[j]) * tx;
            let bottom = c[j] + (d[j] - c[j]) * tx;
            top + (bottom - top) * ty
        })
    }
}

/// The bokeh shape and what it needs to know about the frame.
struct Shape {
    bokeh: Bokeh,
    /// The photo's centre (output px) and the reciprocal of its half-diagonal: a cat's eye is
    /// clipped toward the centre by how far out it is.
    centre: [f32; 2],
    inv_half_diag: f32,
}

/// Unit normals of a pentagon with a flat side at the bottom (90° + 72°·k).
const PENTAGON: [[f32; 2]; 5] =
    [[0.0, 1.0], [-0.951_056_5, 0.309_017], [-0.587_785_24, -0.809_017], [0.587_785_24, -0.809_017], [0.951_056_5, 0.309_017]];

impl Shape {
    fn new(bokeh: Bokeh, frame: &Frame, w: usize, h: usize) -> Shape {
        let c = frame.norm_to_out(w, h).apply(lightcraft_geom::Point::new(0.5, 0.5));
        let ppl = frame.px_per_long(w) as f32;
        let (ow, oh) = (frame.ow as f32, frame.oh as f32);
        let half_diag = 0.5 * (ow * ow + oh * oh).sqrt() / ow.max(oh).max(1.0) * ppl;
        Shape { bokeh, centre: [c.x as f32, c.y as f32], inv_half_diag: 1.0 / half_diag.max(1.0) }
    }

    /// How much of a sample at output position `q` whose shape has radius `r` (≥ 0.5, in the
    /// gathered grid's pixels) lands on a pixel `v` away from it: coverage 0..1 with a soft
    /// one-pixel edge, times the shape's brightness there, over the shape's area relative to a
    /// disc of radius `r` (so every shape spreads the same energy).
    #[inline]
    fn weight(&self, v: [f32; 2], r: f32, q: [f32; 2]) -> f32 {
        let d = (v[0] * v[0] + v[1] * v[1]).sqrt();
        match self.bokeh {
            Bokeh::Circle => (r - d + 0.5).clamp(0.0, 1.0),
            Bokeh::Bubble => {
                let u = (d / r).min(1.0);
                (r - d + 0.5).clamp(0.0, 1.0) * (0.6 + 1.2 * u * u * u * u)
            }
            Bokeh::Blade5 => {
                // a pentagon of the disc's area: circumradius 1.149 r, apothem cos 36° of that
                let a = r * 1.149 * 0.809_017;
                let e = PENTAGON.iter().map(|n| n[0] * v[0] + n[1] * v[1]).fold(f32::MIN, f32::max);
                (a - e + 0.5).clamp(0.0, 1.0)
            }
            Bokeh::Ring => {
                // an annulus of the disc's area, its hole 0.6 of its radius
                let ro = r * 1.25;
                (ro - d + 0.5).clamp(0.0, 1.0) * (d - 0.6 * ro + 0.5).clamp(0.0, 1.0)
            }
            Bokeh::CatEye => {
                // the disc cut by a second one shifted toward the frame's centre
                let off = [q[0] - self.centre[0], q[1] - self.centre[1]];
                let e = [off[0] * self.inv_half_diag * CAT_EYE, off[1] * self.inv_half_diag * CAT_EYE];
                let delta = (e[0] * e[0] + e[1] * e[1]).sqrt().min(1.9);
                let v2 = [v[0] + e[0] * r, v[1] + e[1] * r];
                let d2 = (v2[0] * v2[0] + v2[1] * v2[1]).sqrt();
                // the overlap of two unit circles `delta` apart, as a share of one
                let share = (2.0 * (delta / 2.0).acos() - delta / 2.0 * (4.0 - delta * delta).max(0.0).sqrt()) / std::f32::consts::PI;
                (r - d + 0.5).clamp(0.0, 1.0) * (r - d2 + 0.5).clamp(0.0, 1.0) / share.max(0.05)
            }
        }
    }
}

/// One pixel's gathered light: the background-and-focus colour, the nearer colour laid over it
/// and that one's coverage.
#[derive(Clone, Copy, Default)]
struct Cell {
    bg: [f32; 3],
    fg: [f32; 3],
    alpha: f32,
}

impl Cell {
    fn out(&self) -> [f32; 3] {
        std::array::from_fn(|j| self.bg[j] + (self.fg[j] - self.bg[j]) * self.alpha)
    }
}

/// Unit-disc tap patterns (Vogel spirals) of a few sizes.
fn spirals() -> &'static [Vec<[f32; 2]>] {
    static S: std::sync::OnceLock<Vec<Vec<[f32; 2]>>> = std::sync::OnceLock::new();
    S.get_or_init(|| {
        const SIZES: [usize; 10] = [8, 12, 16, 24, 32, 48, 64, 96, 128, 160];
        let golden = std::f32::consts::PI * (3.0 - 5f32.sqrt());
        SIZES
            .iter()
            .map(|&n| {
                (0..n)
                    .map(|i| {
                        let r = ((i as f32 + 0.5) / n as f32).sqrt();
                        let a = i as f32 * golden;
                        [r * a.cos(), r * a.sin()]
                    })
                    .collect()
            })
            .collect()
    })
}

/// The smallest spiral with at least `n` taps (at most `max`).
fn spiral_for(n: usize, max: usize) -> &'static [[f32; 2]] {
    let all = spirals();
    let n = n.min(max);
    all.iter().find(|s| s.len() >= n).or(all.last()).map_or(&[], |s| s.as_slice())
}

/// Gathers single pixels of a [`Src`]: each from the neighbours within the largest circle that
/// can reach its tile. Tiles and the per-pixel tap rotation follow the whole frame's grid.
struct Gatherer<'a> {
    src: &'a Src<'a>,
    shape: &'a Shape,
    /// From this grid's pixels to output pixels: `q · scale + offset` (the cat's eye's position).
    to_out: (f32, [f32; 2]),
    taps_max: usize,
    reach: f32,
    /// Only pixels whose own circle is under this (the full-size pass near the focal range).
    limit: Option<f32>,
    /// The whole frame's index of the first tile, and how many tiles across.
    tile0: [usize; 2],
    tiles_w: usize,
    bound: Vec<f32>,
}

impl<'a> Gatherer<'a> {
    fn new(src: &'a Src<'a>, shape: &'a Shape, to_out: (f32, [f32; 2]), taps_max: usize, reach: f32, limit: Option<f32>) -> Gatherer<'a> {
        let (w, h, o) = (src.w, src.h, src.origin);
        let tile0 = o.map(|v| v / TILE);
        let (tw, th) = ((o[0] + w).div_ceil(TILE) - tile0[0], (o[1] + h).div_ceil(TILE) - tile0[1]);
        let mut tile = vec![0f32; tw * th];
        for (i, c) in src.coc.iter().enumerate() {
            let (x, y) = (i % w + o[0], i / w + o[1]);
            if let Some(t) = tile.get_mut((y / TILE - tile0[1]) * tw + x / TILE - tile0[0]) {
                *t = t.max(c.abs().min(reach));
            }
        }
        // the largest circle that can reach each tile: its neighbours' within the reach
        let span = (reach / TILE as f32).ceil() as isize;
        let mut bound = vec![0f32; tw * th];
        for ty in 0..th as isize {
            for tx in 0..tw as isize {
                let mut m = 0f32;
                for y in (ty - span).max(0)..=(ty + span).min(th as isize - 1) {
                    for x in (tx - span).max(0)..=(tx + span).min(tw as isize - 1) {
                        m = m.max(tile[y as usize * tw + x as usize]);
                    }
                }
                bound[ty as usize * tw + tx as usize] = m;
            }
        }
        Gatherer { src, shape, to_out, taps_max, reach, limit, tile0, tiles_w: tw, bound }
    }

    /// The light gathered at pixel `(x, y)`, or `None` where nothing blurs (or, with a limit, the
    /// pixel's own circle is past it).
    fn at(&self, x: usize, y: usize) -> Option<Cell> {
        let src = self.src;
        let (w, h) = (src.w, src.h);
        let i = y * w + x;
        let cp = *src.coc.get(i)?;
        if self.limit.is_some_and(|l| cp.abs() >= l) {
            return None;
        }
        let (gx, gy) = (x + src.origin[0], y + src.origin[1]);
        let tile = (gy / TILE - self.tile0[1]) * self.tiles_w + gx / TILE - self.tile0[0];
        let local = self.bound.get(tile).copied().unwrap_or(0.0).max(cp.abs().min(self.reach));
        if local < 0.5 {
            return None;
        }
        let rp = cp.abs().max(0.5);
        let taps = spiral_for((DENSITY * local * local).ceil() as usize, self.taps_max);
        // a rotation per pixel turns the pattern's structure into fine noise
        let ign = (52.982_918 * (0.067_110_56 * (gx % 4096) as f32 + 0.005_837_15 * (gy % 4096) as f32).fract()).fract();
        let (sn, cs) = (ign * std::f32::consts::TAU).sin_cos();
        // the space between taps: they read light averaged over it
        let spacing = local * (std::f32::consts::PI / taps.len().max(1) as f32).sqrt();
        let wc = 1.0 / (rp * rp);
        let centre = src.bloom(src.px.get(i).copied().unwrap_or([0.0; 3]), rp);
        let (mut bg, mut bw) = (centre.map(|v| v * wc), wc);
        let (mut fg, mut fw) = ([0f32; 3], 0f32);
        let p = [x as f32 + 0.5, y as f32 + 0.5];
        let (scale, off) = self.to_out;
        for t in taps {
            let o = [(t[0] * cs - t[1] * sn) * local, (t[0] * sn + t[1] * cs) * local];
            let q = [p[0] + o[0], p[1] + o[1]];
            if q[0] < 0.0 || q[1] < 0.0 || q[0] >= w as f32 || q[1] >= h as f32 {
                continue;
            }
            let cq = src.coc.get(q[1] as usize * w + q[0] as usize).copied().unwrap_or(0.0);
            let rq = cq.abs().max(0.5);
            // a farther sample can't spill over a pixel sharper than itself
            let spill = if cq < cp { rq.min(2.0 * rp) } else { rq };
            let cov = self.shape.weight([-o[0], -o[1]], spill, [q[0] * scale + off[0], q[1] * scale + off[1]]);
            if cov <= 0.0 {
                continue;
            }
            let wq = cov / (rq * rq);
            let c = src.bloom(src.fetch(q[0], q[1], spacing), rq);
            if cq > cp + 1.0 {
                for j in 0..3 {
                    fg[j] += c[j] * wq;
                }
                fw += wq;
            } else {
                for j in 0..3 {
                    bg[j] += c[j] * wq;
                }
                bw += wq;
            }
        }
        // each tap stands for π local² / taps square pixels, each spreading 1 / π r² of its light
        // per square pixel: the coverage is Σ cov · local² / (taps · r²)
        let alpha = (fw * local * local / taps.len().max(1) as f32).min(1.0);
        Some(Cell { bg: bg.map(|v| v / bw), fg: if fw > 0.0 { fg.map(|v| v / fw) } else { [0.0; 3] }, alpha })
    }
}

/// The large blurs, gathered on the working grid.
struct Coarse {
    w: usize,
    h: usize,
    cells: Vec<Cell>,
    coc: Vec<f32>,
}

impl Coarse {
    fn gather(src: &Src<'_>, shape: &Shape, to_out: (f32, [f32; 2]), taps_max: usize, reach: f32) -> Coarse {
        let g = Gatherer::new(src, shape, to_out, taps_max, reach, None);
        let mut cells = vec![Cell::default(); src.w * src.h];
        for_rows(&mut cells, src.w, |y, row| {
            for (x, c) in row.iter_mut().enumerate() {
                *c = g.at(x, y).unwrap_or(Cell { bg: src.px.get(y * src.w + x).copied().unwrap_or([0.0; 3]), ..Cell::default() });
            }
        });
        Coarse { w: src.w, h: src.h, cells, coc: src.coc.to_vec() }
    }

    /// The result at working-pixel position `(x, y)` for a full-size pixel whose circle is `c`
    /// working pixels: bilinear, the four neighbours weighted by how alike their circles are (a
    /// sharp subject's working pixels don't bleed into its blurred surroundings when enlarged).
    fn upsample(&self, x: f32, y: f32, c: f32) -> ([f32; 3], [f32; 3], f32) {
        let (x0, y0) = (x.floor(), y.floor());
        let (tx, ty) = (x - x0, y - y0);
        let (mut bg, mut fg, mut a, mut ws) = ([0f32; 3], [0f32; 3], 0f32, 0f32);
        let sigma = 1.0 + 0.3 * c.abs();
        for (dx, dy, bw) in [(0, 0, (1.0 - tx) * (1.0 - ty)), (1, 0, tx * (1.0 - ty)), (0, 1, (1.0 - tx) * ty), (1, 1, tx * ty)] {
            let xi = (x0 as isize + dx).clamp(0, self.w as isize - 1) as usize;
            let yi = (y0 as isize + dy).clamp(0, self.h as isize - 1) as usize;
            let i = yi * self.w + xi;
            let (Some(cell), Some(cc)) = (self.cells.get(i), self.coc.get(i)) else { continue };
            let dc = (cc - c) / sigma;
            let wt = bw * ((-0.5 * dc * dc).exp() + 1e-4);
            for j in 0..3 {
                bg[j] += cell.bg[j] * wt;
                fg[j] += cell.fg[j] * wt;
            }
            a += cell.alpha * wt;
            ws += wt;
        }
        let ws = ws.max(1e-12);
        (bg.map(|v| v / ws), fg.map(|v| v / ws), (a / ws).clamp(0.0, 1.0))
    }
}

#[cfg(test)]
mod tests;
