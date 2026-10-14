//! **Generative Expand** in the render: the crop's Expand lets the frame reach past the photo's
//! edges, and the area there shows what a model painted ([`ExpandFill`], made off the render path
//! by the engine with `lightcraft-erase`), or nothing yet.
//!
//! - **Where it goes.** The fill is put in at the resample ([`Frame::sample`]), in the resampled
//!   source's own light, before white balance: every edit after that (white balance, tone,
//!   colour, masks, Remove spots, grain…) applies to the new area exactly as to the photo, and a
//!   later change carries over. The photo's own pixels never change.
//! - **The edge.** The fill holds the photo's pixels for a few pixels inside its edge, so the
//!   bilinear lookup at the edge reads the photo; without a lens or perspective warp, the last
//!   fill pixel outside the edge fades from the photo's edge pixels into the fill.
//! - **Nothing there yet.** Area past the photo with no fill (Expand on, Generate not run, or the
//!   crop moved past the fill) renders as the light neutral of an empty canvas ([`BLANK`]) and
//!   is transparent in the 8-bit result ([`mark_missing`]), which the crop tool shows as a
//!   checkerboard.
//! - **What the model sees** ([`expand_input`]): the crop's bounding box (and a little more) at up
//!   to 1024 pixels a side, resampled like any render and display-encoded as AI Remove does; its
//!   answer is kept over the new area and a thin ring of the photo ([`ExpandInput::finish`]).
//!
//! With the crop tool open (an uncropped render), the frame shows the photo, a margin around it
//! for room to drag into, and wherever the crop and the fill reach ([`view_rect`]).

use std::sync::{Arc, OnceLock};

use lightcraft_develop::{DevelopSettings, ExpandFill, SpotFill};
use lightcraft_geom::{Affine, CropGeometry, EXPAND_MAX, Point, Rect};
use lightcraft_raster::{Rgb32f, Rgba8, par_rows};

use crate::geometry::{BLANK, Frame};

/// Longest side of the canvas the models are given, and of the stored fill
/// ([`lightcraft_develop::segmask::MAX_FILL_SIDE`]).
pub const MAX_CANVAS: usize = lightcraft_develop::segmask::MAX_FILL_SIDE;
/// The crop tool's room around the photo with Expand on, in photo widths / heights per side.
pub const VIEW_MARGIN: f64 = 0.25;
/// Context the model gets around the crop, as a fraction of the crop's bounding box per side.
const CONTEXT: f64 = 0.03;
/// Pixels of the photo kept in a stored fill inside its edge (for the lookup and blend there).
const RING: usize = 3;

/// Generative Expand on a frame: the stored fill, if any (decoded once, on first use).
#[derive(Clone)]
pub struct Expansion {
    inner: Arc<Inner>,
}

struct Inner {
    fill: Option<ExpandFill>,
    /// Identifies the fill in the stage cache's key (see `Frame`'s `Debug`).
    key: u64,
    decoded: OnceLock<Option<Decoded>>,
}

struct Decoded {
    w: usize,
    h: usize,
    rect: [f64; 4],
    px: Vec<[f32; 3]>,
}

impl std::fmt::Debug for Expansion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Expansion({:016x})", self.inner.key)
    }
}

impl Expansion {
    /// Expand with `fill` (or none yet).
    pub fn new(fill: Option<&ExpandFill>) -> Expansion {
        let key = fill.map_or(0, |f| {
            use std::hash::{BuildHasher, BuildHasherDefault, DefaultHasher};
            BuildHasherDefault::<DefaultHasher>::default().hash_one((
                &f.fill.data,
                f.fill.w,
                f.fill.h,
                f.fill.rect.map(f64::to_bits),
                f.fill.scale.to_bits(),
            ))
        });
        Expansion { inner: Arc::new(Inner { fill: fill.cloned(), key, decoded: OnceLock::new() }) }
    }

    /// Whether there is a stored fill.
    pub fn has_fill(&self) -> bool {
        self.inner.fill.is_some()
    }

    /// The part of the transformed image the fill covers, normalized (`None`: no fill, or damaged).
    pub fn fill_bounds(&self) -> Option<[f64; 4]> {
        self.inner.fill.as_ref().and_then(|f| f.fill.bounds())
    }

    fn decoded(&self) -> Option<&Decoded> {
        self.inner
            .decoded
            .get_or_init(|| {
                let f = &self.inner.fill.as_ref()?.fill;
                let rect = f.bounds()?;
                let px = f.pixels()?;
                Some(Decoded { w: f.w as usize, h: f.h as usize, rect, px })
            })
            .as_ref()
    }
}

impl Decoded {
    /// The fill at normalized point `n` (bilinear, edge-clamped inside its rect); `None` outside it.
    fn at(&self, n: Point) -> Option<[f32; 3]> {
        let [x0, y0, x1, y1] = self.rect;
        if !(n.x >= x0 && n.x <= x1 && n.y >= y0 && n.y <= y1) {
            return None;
        }
        let fx = ((n.x - x0) / (x1 - x0) * self.w as f64 - 0.5) as f32;
        let fy = ((n.y - y0) / (y1 - y0) * self.h as f64 - 0.5) as f32;
        let (ix, iy) = (fx.floor(), fy.floor());
        let (tx, ty) = (fx - ix, fy - iy);
        let px = |x: f32, y: f32| {
            let (x, y) = ((x.max(0.0) as usize).min(self.w - 1), (y.max(0.0) as usize).min(self.h - 1));
            self.px.get(y * self.w + x).copied().unwrap_or(BLANK)
        };
        let (a, b, c, d) = (px(ix, iy), px(ix + 1.0, iy), px(ix, iy + 1.0), px(ix + 1.0, iy + 1.0));
        Some(std::array::from_fn(|i| {
            let top = a[i] + (b[i] - a[i]) * tx;
            let bot = c[i] + (d[i] - c[i]) * tx;
            top + (bot - top) * ty
        }))
    }

    /// One fill pixel, in pixels of the transformed image (`ow × oh`).
    fn pixel(&self, ow: f64, oh: f64) -> f64 {
        let [x0, y0, x1, y1] = self.rect;
        ((x1 - x0) * ow / self.w as f64).max((y1 - y0) * oh / self.h as f64)
    }
}

/// What an output pixel shows.
enum Px {
    /// The photo.
    Photo,
    /// Past the photo, at normalized point `n`, `d` transformed pixels past its edge (`None`: a
    /// warped frame, where the distance isn't known).
    Past { n: Point, d: Option<f64> },
}

/// Classify output pixel `(x, y)` of a `w × h` render of `frame`, through `o2t` (output px →
/// transformed px).
fn classify(frame: &Frame, o2t: &Affine, x: usize, y: usize) -> Px {
    let p = o2t.apply(Point::new(x as f64 + 0.5, y as f64 + 0.5));
    let (ow, oh) = (frame.ow, frame.oh);
    let n = Point::new(p.x / ow, p.y / oh);
    match &frame.warp {
        Some(wp) => {
            if wp.covers(o2t, x, y) {
                Px::Photo
            } else {
                Px::Past { n, d: None }
            }
        }
        None => {
            let d = (-p.x).max(p.x - ow).max(-p.y).max(p.y - oh);
            if d <= 0.0 { Px::Photo } else { Px::Past { n, d: Some(d) } }
        }
    }
}

/// Put the stored fill into `out` (a `Frame::sample` result of `frame`) wherever it shows no photo;
/// [`BLANK`] where there is no fill.
pub(crate) fn composite(frame: &Frame, ex: &Expansion, out: &mut Rgb32f) {
    let (w, h) = (out.width, out.height);
    let o2t = frame.out_to_oriented(w, h);
    let fill = ex.decoded();
    let band = fill.map_or(1.0, |f| f.pixel(frame.ow, frame.oh)).max(1e-9);
    par_rows(&mut out.data, w, |y, row| {
        for (x, px) in row.iter_mut().enumerate() {
            let Px::Past { n, d } = classify(frame, &o2t, x, y) else { continue };
            *px = match fill.and_then(|f| f.at(n)) {
                Some(f) => {
                    // the photo's edge pixels (edge-clamped by the resample) fade into the fill
                    let t = d.map_or(1.0, |d| (d / band).clamp(0.0, 1.0)) as f32;
                    let t = t * t * (3.0 - 2.0 * t);
                    std::array::from_fn(|c| px[c] + (f[c].max(0.0) - px[c]) * t)
                }
                None => BLANK,
            };
        }
    });
}

/// Which pixels of a `w × h` render of `frame` show nothing (past the photo, where the fill doesn't
/// reach), row-major; `None` when there are none (or the frame doesn't expand).
pub fn missing(frame: &Frame, w: usize, h: usize) -> Option<Vec<bool>> {
    let ex = frame.expand.as_ref()?;
    let o2t = frame.out_to_oriented(w, h);
    let fill = ex.decoded();
    let mut m = vec![false; w.checked_mul(h)?];
    par_rows(&mut m, w, |y, row| {
        for (x, v) in row.iter_mut().enumerate() {
            if let Px::Past { n, .. } = classify(frame, &o2t, x, y) {
                *v = fill.and_then(|f| f.at(n)).is_none();
            }
        }
    });
    m.contains(&true).then_some(m)
}

/// Make the pixels of `img` (a render of `frame`) that show nothing transparent.
pub fn mark_missing(img: &mut Rgba8, frame: &Frame) {
    if let Some(m) = missing(frame, img.width, img.height) {
        for (p, &gone) in img.data.iter_mut().zip(&m) {
            if gone {
                p[3] = 0;
            }
        }
    }
}

/// The crop's quad in normalized transformed coordinates, for a `ow × oh` transformed image.
fn crop_quad(g: &CropGeometry, ow: f64, oh: f64) -> [Point; 4] {
    let r = g.rect_px(ow, oh);
    let back = Affine::rotate_about(-g.angle.to_radians(), Point::new(ow / 2.0, oh / 2.0));
    r.corners().map(|p| {
        let q = back.apply(p);
        Point::new(q.x / ow, q.y / oh)
    })
}

fn bounds(pts: &[Point]) -> Option<Rect> {
    let first = pts.first()?;
    let r =
        pts.iter().fold(Rect::new(first.x, first.y, first.x, first.y), |r, p| Rect::new(r.x0.min(p.x), r.y0.min(p.y), r.x1.max(p.x), r.y1.max(p.y)));
    [r.x0, r.y0, r.x1, r.y1].iter().all(|v| v.is_finite()).then_some(r)
}

/// Keep `r` within the grown photo (`-EXPAND_MAX ..= 1 + EXPAND_MAX`).
fn within_limit(r: Rect) -> Rect {
    let c = |v: f64| v.clamp(-EXPAND_MAX, 1.0 + EXPAND_MAX);
    Rect::new(c(r.x0), c(r.y0), c(r.x1), c(r.y1))
}

/// What the crop tool shows with Expand on (normalized transformed coordinates): the photo with
/// [`VIEW_MARGIN`] around it, and wherever the crop or the fill reach beyond that.
pub fn view_rect(s: &DevelopSettings, ow: f64, oh: f64) -> Rect {
    let m = VIEW_MARGIN;
    let mut r = Rect::new(-m, -m, 1.0 + m, 1.0 + m);
    if let Some(q) = bounds(&crop_quad(&s.crop.geometry, ow, oh)) {
        r = r.union(&q);
    }
    if let Some([x0, y0, x1, y1]) = s.expand_fill.as_ref().and_then(|f| f.fill.bounds()) {
        r = r.union(&Rect::new(x0, y0, x1, y1));
    }
    within_limit(r)
}

/// What Generative Expand gives a model for photo settings `s`, and what it needs to store the
/// answer.
pub struct ExpandInput {
    /// The canvas, display-encoded (`(v / scale)^(1/2.2)`, 0..=1); 0 where `known` isn't set.
    pub image: Rgb32f,
    /// The canvas pixels that show the photo (the rest is what the model paints).
    pub known: Vec<bool>,
    /// The canvas as resampled, linear (before white balance).
    lin: Rgb32f,
    scale: f32,
    /// The canvas, normalized `[x0, y0, x1, y1]` of the transformed image.
    rect: [f64; 4],
    /// The canvas's longest side in pixels of the source it was rendered from.
    pub native: f64,
}

/// The canvas a model needs to fill the area past the photo under `s`'s crop: the crop's bounding
/// box with a little context, resampled from `src` at its own resolution up to `max_side` pixels
/// (at most [`MAX_CANVAS`]). `None` when Expand is off or the crop shows no area past the photo.
pub fn expand_input(src: &Rgb32f, info: &crate::SourceInfo, s: &DevelopSettings, max_side: usize) -> Option<ExpandInput> {
    if !s.crop.expand {
        return None;
    }
    let frame = crate::frame_for(src, info, s, true);
    let (ow, oh) = (frame.ow, frame.oh);
    let q = bounds(&crop_quad(&frame.crop, ow, oh))?;
    // a little more on every side, past the photo only where the crop already is
    let (mx, my) = (CONTEXT * q.width(), CONTEXT * q.height());
    let lo = |v: f64, m: f64| if v < 0.0 { v - m } else { (v - m).max(0.0) };
    let hi = |v: f64, m: f64| if v > 1.0 { v + m } else { (v + m).min(1.0) };
    let r = within_limit(Rect::new(lo(q.x0, mx), lo(q.y0, my), hi(q.x1, mx), hi(q.y1, my)));
    if r.is_empty() || r.width() <= 0.0 || r.height() <= 0.0 {
        return None;
    }
    // the canvas alone: a crop of exactly it, unturned, no flips, no old fill
    let mut area = s.clone();
    area.crop.geometry = CropGeometry { rect: r, angle: 0.0 };
    (area.crop.flip_h, area.crop.flip_v) = (false, false);
    area.expand_fill = None;
    let native = (r.width() * ow).max(r.height() * oh);
    let side = native.min(max_side.min(MAX_CANVAS) as f64).round().max(1.0) as usize;
    let p = crate::plan(src, info, &area, &crate::RenderRequest::fit(side, side));
    let lin = p.frame.sample(src, p.w, p.h);
    let gone = missing(&p.frame, p.w, p.h)?;
    let known: Vec<bool> = gone.iter().map(|g| !g).collect();
    if !known.contains(&true) {
        return None;
    }
    // exposure for the model: the photo's bright end (99th percentile) near white
    let mut bright: Vec<f32> =
        lin.data.iter().zip(&known).filter(|(_, k)| **k).map(|(p, _)| p[0].max(p[1]).max(p[2])).filter(|v| v.is_finite()).collect();
    let at = (bright.len() * 99 / 100).min(bright.len().saturating_sub(1));
    let scale = if bright.is_empty() { 1.0 } else { *bright.select_nth_unstable_by(at, f32::total_cmp).1 }.max(1e-4);
    let mut image = lin.map(|p| p.map(|v| (v / scale).clamp(0.0, 1.0).powf(1.0 / 2.2)));
    for (o, &k) in image.data.iter_mut().zip(&known) {
        if !k {
            *o = [0.0; 3];
        }
    }
    Some(ExpandInput { image, known, lin, scale, rect: [r.x0, r.y0, r.x1, r.y1], native })
}

impl ExpandInput {
    /// The canvas size.
    pub fn size(&self) -> (usize, usize) {
        (self.image.width, self.image.height)
    }

    /// The stored fill from a model's answer `filled` (the canvas, display-encoded): its pixels over
    /// the new area, the photo's for [`RING`] pixels inside the photo's edge, black elsewhere.
    /// `None` when `filled` doesn't match the canvas.
    pub fn finish(&self, filled: &Rgb32f, model: &str) -> Option<ExpandFill> {
        let (w, h) = self.size();
        if filled.width != w || filled.height != h || filled.data.len() != w * h {
            return None;
        }
        let near = near_new(&self.known, w, h, RING);
        let px: Vec<[f32; 3]> = (0..w * h)
            .map(|i| {
                if !self.known.get(i).copied().unwrap_or(true) {
                    filled.data.get(i).map_or([0.0; 3], |e| e.map(|c| self.scale * c.clamp(0.0, 1.0).powf(2.2)))
                } else if near.get(i).copied().unwrap_or(false) {
                    self.lin.data.get(i).map_or([0.0; 3], |p| p.map(|v| v.max(0.0)))
                } else {
                    [0.0; 3]
                }
            })
            .collect();
        SpotFill::from_linear(w, h, self.rect, &px).map(|fill| ExpandFill { fill, model: model.to_string() })
    }

    /// A model's answer `ai` (the canvas, display-encoded, with the network's 512 pixels of
    /// detail) with the photo's grain and fine texture added over the new area: `blur(ai) +
    /// synth − blur(synth)`, the fill synthesized from the photo giving what is finer than about a
    /// network pixel and a half, as AI Remove does for its spots (the blur also takes out the
    /// networks' faint upsampling grid). The photo's pixels stay as they are.
    pub fn with_texture(&self, ai: &Rgb32f) -> Rgb32f {
        let (w, h) = self.size();
        if ai.width != w || ai.height != h {
            return ai.clone();
        }
        self.textured(ai, &self.synthesized(), 1.5 * self.network_px())
    }

    /// The new area filled without a model: the photo's edge colours diffused outward for the
    /// structure (a sky or a sea goes on as it ends), with texture synthesized from the photo (the
    /// Remove tool's content-aware fill, as the panorama's Fill Edges uses) for what is finer than a
    /// few network pixels. Synthesis alone, coarse to fine, copies whole bands of the photo (a
    /// sunset's glow) into the new area. Display-encoded like [`Self::image`].
    pub fn classic(&self) -> Rgb32f {
        let (w, h) = self.size();
        let hole: Vec<bool> = self.known.iter().map(|k| !k).collect();
        let flat: Vec<f32> = self.lin.data.iter().flat_map(|p| p.iter().copied()).collect();
        let out = lightcraft_inpaint::membrane_fill(w, h, 3, &flat, &hole);
        let diffused = Rgb32f::from_fn(w, h, |x, y| {
            let i = (y * w + x) * 3;
            [0, 1, 2].map(|c| out.get(i + c).copied().unwrap_or(0.0))
        });
        self.textured(&self.encode(&diffused), &self.synthesized(), 4.0 * self.network_px())
    }

    /// One network pixel (the networks see the canvas at 512 pixels), in canvas pixels.
    fn network_px(&self) -> f32 {
        let (w, h) = self.size();
        (w.max(h) as f32 / 512.0).max(1.0)
    }

    /// Linear canvas pixels display-encoded like [`Self::image`].
    fn encode(&self, lin: &Rgb32f) -> Rgb32f {
        lin.map(|p| p.map(|v| (v / self.scale).clamp(0.0, 1.0).powf(1.0 / 2.2)))
    }

    /// Texture synthesized from the photo over the new area (display-encoded); the canvas as it is
    /// where nothing can be copied.
    fn synthesized(&self) -> Rgb32f {
        let (w, h) = self.size();
        let hole: Vec<bool> = self.known.iter().map(|k| !k).collect();
        match crate::inpaint::fill(&self.lin, 0, 0, w, h, &hole, reach(&hole, w, h), (0.0, 0.0), 0) {
            Some(data) => self.encode(&Rgb32f { width: w, height: h, data }),
            None => self.image.clone(),
        }
    }

    /// `base` over the new area with `texture`'s detail finer than `sigma` pixels.
    fn textured(&self, base: &Rgb32f, texture: &Rgb32f, sigma: f32) -> Rgb32f {
        let (bl, tl) = (lightcraft_raster::blur::gaussian(base, sigma), lightcraft_raster::blur::gaussian(texture, sigma));
        let mut out = self.image.clone();
        for (i, o) in out.data.iter_mut().enumerate() {
            if self.known.get(i).copied().unwrap_or(true) {
                continue;
            }
            if let (Some(b), Some(t), Some(l)) = (bl.data.get(i), texture.data.get(i), tl.data.get(i)) {
                *o = std::array::from_fn(|c| (b[c] + t[c] - l[c]).clamp(0.0, 1.0));
            }
        }
        out
    }
}

/// The pixels set in `known` within `r` pixels (square neighbourhood) of one that isn't.
fn near_new(known: &[bool], w: usize, h: usize, r: usize) -> Vec<bool> {
    // rows, then columns: a separable dilation of the new area
    let n = w.saturating_mul(h);
    let rows: Vec<bool> = (0..n)
        .map(|i| {
            let (x, y) = (i % w.max(1), i / w.max(1));
            let (a, b) = (x.saturating_sub(r), (x + r).min(w.saturating_sub(1)));
            (a..=b).any(|xx| !known.get(y * w + xx).copied().unwrap_or(true))
        })
        .collect();
    (0..n)
        .map(|i| {
            let (x, y) = (i % w.max(1), i / w.max(1));
            let (a, b) = (y.saturating_sub(r), (y + r).min(h.saturating_sub(1)));
            known.get(i).copied().unwrap_or(false) && (a..=b).any(|yy| rows.get(yy * w + x).copied().unwrap_or(false))
        })
        .collect()
}

/// How far (px) the hole reaches from the nearest pixel outside it, on a grid of at most 512 cells
/// a side (it sizes the synthesis pyramid).
fn reach(hole: &[bool], w: usize, h: usize) -> f32 {
    let f = (w.max(h) / 512).max(1);
    let (gw, gh) = (w.div_ceil(f), h.div_ceil(f));
    let mut d = vec![u32::MAX; gw * gh];
    let mut queue = std::collections::VecDeque::new();
    for (i, _) in hole.iter().enumerate().filter(|(_, m)| !**m) {
        let c = (i / w / f) * gw + (i % w) / f;
        if let Some(v) = d.get_mut(c)
            && *v != 0
        {
            *v = 0;
            queue.push_back(c);
        }
    }
    let mut far = 0;
    while let Some(c) = queue.pop_front() {
        let (x, y) = (c % gw, c / gw);
        let next = d.get(c).copied().unwrap_or(0).saturating_add(1);
        far = far.max(next - 1);
        for (nx, ny) in [(x.wrapping_sub(1), y), (x + 1, y), (x, y.wrapping_sub(1)), (x, y + 1)] {
            if nx < gw && ny < gh {
                let j = ny * gw + nx;
                if let Some(v) = d.get_mut(j)
                    && *v == u32::MAX
                {
                    *v = next;
                    queue.push_back(j);
                }
            }
        }
    }
    ((far as usize * f) as f32).max(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RenderRequest, SourceInfo, render};

    fn photo(w: usize, h: usize) -> Rgb32f {
        Rgb32f::from_fn(w, h, |x, y| [0.1 + 0.6 * x as f32 / w as f32, 0.3, 0.2 + 0.5 * y as f32 / h as f32])
    }

    fn expanded(rect: Rect) -> DevelopSettings {
        let mut s = DevelopSettings::default();
        s.crop.expand = true;
        s.crop.geometry.rect = rect;
        s
    }

    /// A fill of one flat colour over the canvas the settings ask for.
    fn flat_fill(src: &Rgb32f, s: &DevelopSettings, v: f32) -> ExpandFill {
        let input = expand_input(src, &SourceInfo::default(), s, 256).unwrap();
        let (w, h) = input.size();
        let painted = Rgb32f::from_fn(w, h, |x, y| if input.known[y * w + x] { input.image.get(x, y) } else { [v; 3] });
        input.finish(&painted, "test").unwrap()
    }

    #[test]
    fn without_a_fill_the_new_area_is_blank_and_transparent() {
        let src = photo(120, 80);
        let s = expanded(Rect::new(-0.25, 0.0, 1.0, 1.0));
        let frame = Frame::new(120, 80, &s, true);
        assert!(frame.expand.is_some());
        let out = frame.sample(&src, 150, 80);
        assert_eq!(out.get(5, 40), BLANK);
        assert!((out.get(100, 40)[0] - src.get(70, 40)[0]).abs() < 0.02);
        let gone = missing(&frame, 150, 80).unwrap();
        assert!(gone[40 * 150 + 5] && !gone[40 * 150 + 100]);
        let mut img = render(&src, &SourceInfo::default(), &s, &RenderRequest::fit(150, 80)).image;
        assert_eq!((img.width, img.height), (150, 80));
        assert_eq!(img.get(5, 40)[3], 0);
        assert_eq!(img.get(100, 40)[3], 255);
        mark_missing(&mut img, &frame);
        // Expand off: the same crop smears the photo's edge as before, and nothing is transparent
        let mut off = s.clone();
        off.crop.expand = false;
        let f = Frame::new(120, 80, &off, true);
        assert!(f.expand.is_none() && missing(&f, 150, 80).is_none());
    }

    #[test]
    fn the_fill_shows_past_the_edge_and_the_photo_is_unchanged() {
        let src = photo(120, 80);
        let mut s = expanded(Rect::new(-0.25, -0.1, 1.0, 1.0));
        s.expand_fill = Some(flat_fill(&src, &s, 0.8));
        let frame = Frame::new(120, 80, &s, true);
        let (w, h) = frame.fit(1000, 1000);
        let out = frame.sample(&src, w, h);
        let at = |n: Point| {
            let p = frame.norm_to_out(w, h).apply(n);
            out.get(p.x as usize, p.y as usize)
        };
        // the painted value is display-encoded: 0.8^2.2 × scale in linear light
        let far = at(Point::new(-0.2, 0.5));
        assert!((far[0] - far[1]).abs() < 1e-3 && far[0] > 0.3, "{far:?}");
        // inside the photo: the photo
        let inside = at(Point::new(0.5, 0.5));
        assert!((inside[0] - src.get(60, 40)[0]).abs() < 0.02, "{inside:?}");
        assert!(missing(&frame, w, h).is_none());
        // the fill is in the stage cache key: a new fill is a new resample
        let other = {
            let mut o = s.clone();
            o.expand_fill = Some(flat_fill(&src, &o, 0.3));
            Frame::new(120, 80, &o, true)
        };
        assert_ne!(format!("{frame:?}"), format!("{other:?}"));
        // the crop moved past the fill: the part beyond it shows nothing
        let mut moved = s.clone();
        moved.crop.geometry.rect = Rect::new(-0.6, 0.0, 1.0, 1.0);
        let f = Frame::new(120, 80, &moved, true);
        let (w2, h2) = f.fit(400, 400);
        let gone = missing(&f, w2, h2).unwrap();
        assert!(gone[(h2 / 2) * w2 + 1]);
    }

    #[test]
    fn the_stored_fill_keeps_only_the_new_area_and_a_ring() {
        let src = photo(200, 100);
        let s = expanded(Rect::new(0.0, 0.0, 1.3, 1.0));
        let input = expand_input(&src, &SourceInfo::default(), &s, 512).unwrap();
        let (w, h) = input.size();
        assert!(w <= 512 && h <= 512 && input.known.iter().any(|k| !k) && input.known.iter().any(|k| *k));
        let painted = Rgb32f::filled(w, h, [0.5; 3]);
        let f = input.finish(&painted, "migan").unwrap();
        assert_eq!(f.model, "migan");
        let px = f.fill.pixels().unwrap();
        // deep inside the photo: black (not stored); at its edge and past it: values
        let row = h / 2;
        assert_eq!(px[row * w + 5], [0.0; 3]);
        let edge = (0..w).find(|&x| !input.known[row * w + x]).unwrap();
        assert!(px[row * w + edge - 1][0] > 0.0 && px[row * w + edge + 2][0] > 0.0);
        // a canvas over the photo only: nothing to fill
        let inside = expanded(Rect::new(0.1, 0.1, 0.9, 0.9));
        assert!(expand_input(&src, &SourceInfo::default(), &inside, 512).is_none());
        assert!(expand_input(&src, &SourceInfo::default(), &DevelopSettings::default(), 512).is_none());
        assert!(input.finish(&Rgb32f::new(3, 3), "x").is_none());
    }

    #[test]
    fn the_classic_fill_continues_the_photo() {
        let src = photo(160, 100);
        let s = expanded(Rect::new(0.0, -0.2, 1.0, 1.2));
        let input = expand_input(&src, &SourceInfo::default(), &s, 200).unwrap();
        let out = input.classic();
        let (w, h) = input.size();
        assert_eq!((out.width, out.height), (w, h));
        // the strip above the photo continues its colours (blue rises downwards, so it's low there)
        let top = out.get(w / 2, 1);
        assert!(top.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v)), "{top:?}");
        assert!((top[1] - input.image.get(w / 2, h / 2)[1]).abs() < 0.1, "{top:?}");
    }

    #[test]
    fn texture_adds_the_photos_grain_to_a_smooth_answer() {
        // a noisy photo: the new strip, painted flat, gets grain like it
        let src = Rgb32f::from_fn(300, 200, |x, y| {
            let n = ((x * 7919 + y * 104_729) % 97) as f32 / 97.0;
            [0.3 + 0.1 * n, 0.3, 0.3]
        });
        let s = expanded(Rect::new(0.0, 0.0, 1.0, 1.3));
        let input = expand_input(&src, &SourceInfo::default(), &s, 512).unwrap();
        let (w, h) = input.size();
        let flat = Rgb32f::from_fn(w, h, |x, y| if input.known[y * w + x] { input.image.get(x, y) } else { [0.6, 0.58, 0.58] });
        let out = input.with_texture(&flat);
        let strip: Vec<f32> = (0..w).map(|x| out.get(x, h - 5)[0]).collect();
        let mean = strip.iter().sum::<f32>() / w as f32;
        let var = strip.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / w as f32;
        assert!(var > 1e-5, "grain in the new strip: {var}");
        assert!((mean - 0.6).abs() < 0.05, "the answer's colour stays: {mean}");
        // the photo's own pixels are untouched
        for (i, k) in input.known.iter().enumerate() {
            if *k {
                assert_eq!(out.data[i], flat.data[i]);
            }
        }
        assert_eq!(input.with_texture(&Rgb32f::new(2, 2)).width, 2);
    }

    #[test]
    fn the_crop_tool_view_holds_the_photo_the_crop_and_the_fill() {
        let src = photo(120, 80);
        let mut s = expanded(Rect::new(-0.6, 0.0, 1.0, 1.0));
        let v = view_rect(&s, 120.0, 80.0);
        assert!(v.x0 <= -0.6 && v.y0 <= -VIEW_MARGIN && v.x1 >= 1.0 + VIEW_MARGIN);
        let f = Frame::new(120, 80, &s, false);
        assert_eq!(f.crop.rect, v);
        // the photo shows inside the view, the rest is empty until generated
        let (w, h) = f.fit(600, 600);
        let out = f.sample(&src, w, h);
        let c = f.norm_to_out(w, h).apply(Point::new(0.5, 0.5));
        assert!((out.get(c.x as usize, c.y as usize)[0] - src.get(60, 40)[0]).abs() < 0.02);
        assert!(missing(&f, w, h).is_some());
        // a fill beyond the margin widens the view; hostile numbers stay within the limit
        s.crop.geometry.rect = Rect::new(f64::NAN, 0.0, 9.0, 1.0);
        let v = view_rect(&s, 120.0, 80.0);
        assert!(v.x1 <= 1.0 + EXPAND_MAX && v.x0 >= -EXPAND_MAX && v.x0.is_finite());
    }

    #[test]
    fn hostile_fills_render_blank_not_panic() {
        let src = photo(60, 40);
        let mut s = expanded(Rect::new(-0.5, 0.0, 1.0, 1.0));
        let mut f = flat_fill(&src, &s, 0.5);
        f.fill.data = "!!".into();
        s.expand_fill = Some(f.clone());
        let out = Frame::new(60, 40, &s, true).sample(&src, 90, 40);
        assert_eq!(out.get(2, 20), BLANK);
        f.fill.w = 100_000;
        f.fill.rect = [f64::NAN, 0.0, 1.0, 1.0];
        s.expand_fill = Some(f);
        let _ = render(&src, &SourceInfo::default(), &s, &RenderRequest::fit(90, 40));
        assert!(near_new(&[], 0, 0, 3).is_empty());
        assert_eq!(reach(&[], 0, 0), 1.0);
    }
}
