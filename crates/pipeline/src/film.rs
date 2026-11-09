//! Film effects (`docs/film-effects.md`): a frame the photo sits in (a 35mm strip with its sprocket holes, a filed-out
//! negative carrier's black edge, an instant print's frame) and a stack of procedural film marks (light leaks and
//! streaks, a burnt-out edge, dust and hairs, scratches), drawn over the finished picture.
//!
//! Everything is placed in units of the whole output's long edge, from the pixel's place in the whole output, so a
//! window, a preview, a thumbnail and an export of the same photo draw the same marks; shapes smaller than a pixel fade
//! by their area, as they would in a downsized export. The maths works in linear light of the output's primaries
//! (SDR white = 1); the colours below are linear sRGB, taken to the output's primaries once.
//!
//! A native (non-Rust) frontend draws the same with its own kernel: the integer hash, the noise and every constant
//! here are its specification.

use std::f32::consts::{PI, TAU};

use lightcraft_develop::{DevelopSettings, FilmFrame, FilmKind, FrameTone};
use lightcraft_raster::Rgba8;
use rayon::prelude::*;

use crate::output::{DeepImage, DeepSamples, OutputSpace, OutputTrc};

// ---------- noise

/// A 32-bit integer hash of a lattice point and a seed (wrapping arithmetic: the native kernel's `uint` maths).
pub fn hash(x: i32, y: i32, seed: u32) -> u32 {
    let mut h = seed ^ (x as u32).wrapping_mul(0x8da6_b343) ^ (y as u32).wrapping_mul(0xd816_3841);
    h ^= h >> 16;
    h = h.wrapping_mul(0x7feb_352d);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846c_a68b);
    h ^ (h >> 16)
}

/// `hash` as a number in 0..1.
fn rand(x: i32, y: i32, seed: u32) -> f32 {
    (hash(x, y, seed) >> 8) as f32 / 16_777_216.0
}

/// The `n`th random number of lattice point `(x, y)` (several per point: a speck's place, size, shape…).
fn rand_n(x: i32, y: i32, seed: u32, n: u32) -> f32 {
    rand(x, y, seed.wrapping_add(n.wrapping_mul(0x9e37_79b9)))
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = if e1 == e0 { if x < e0 { 0.0 } else { 1.0 } } else { ((x - e0) / (e1 - e0)).clamp(0.0, 1.0) };
    t * t * (3.0 - 2.0 * t)
}

fn mix(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [mix(a[0], b[0], t), mix(a[1], b[1], t), mix(a[2], b[2], t)]
}

/// Value noise in 0..1: random values on the integer lattice, joined by a quintic.
fn value_noise(x: f32, y: f32, seed: u32) -> f32 {
    let (fx, fy) = (x.floor(), y.floor());
    // (a lattice index past what an i32 holds would wrap: the noise repeats, it never fails)
    let (ix, iy) = (fx as i32, fy as i32);
    let (tx, ty) = (x - fx, y - fy);
    let fade = |t: f32| t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
    let (ux, uy) = (fade(tx), fade(ty));
    let a = rand(ix, iy, seed);
    let b = rand(ix.wrapping_add(1), iy, seed);
    let c = rand(ix, iy.wrapping_add(1), seed);
    let d = rand(ix.wrapping_add(1), iy.wrapping_add(1), seed);
    mix(mix(a, b, ux), mix(c, d, ux), uy)
}

/// Fractal value noise in 0..1 (`octaves` doublings). `footprint` is a pixel's size in the noise's units: an octave
/// finer than a pixel fades to its mean, so a small render shows what a downsized large one would.
fn fbm(x: f32, y: f32, seed: u32, octaves: u32, footprint: f32) -> f32 {
    let (mut sum, mut norm, mut amp, mut freq) = (0.0, 0.0, 1.0, 1.0);
    for o in 0..octaves {
        let seen = 1.0 - smoothstep(0.25, 1.0, footprint * freq);
        let n = value_noise(x * freq, y * freq, seed.wrapping_add(o.wrapping_mul(0x632b_e5ab)));
        sum += amp * mix(0.5, n, seen);
        norm += amp;
        amp *= 0.5;
        freq *= 2.0;
    }
    if norm > 0.0 { sum / norm } else { 0.5 }
}

/// FNV-1a of a photo's id (the one sync uses): what a photo mixes into its film marks' seeds while they vary per photo.
pub fn salt_of(photo_id: &str) -> u32 {
    photo_id.bytes().fold(0x811c_9dc5u32, |h, b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193))
}

/// A layer's seed as drawn: mixed with the photo's salt while the marks vary per photo.
pub fn effective_seed(seed: u32, salt: u32, vary: bool) -> u32 {
    if vary { hash(seed as i32, salt as i32, 0x68e3_1da4) } else { seed }
}

// ---------- colours (linear sRGB)

/// Film's response to leaked light per channel (`1 − e^(−I·k)`): the red layer fills first, so weak light reads deep
/// red, then orange and yellow, and white where it burns. Color turns it toward magenta red (−1) or yellow (1).
fn response_k(color: f32) -> [f32; 3] {
    const BASE: [f32; 3] = [1.0, 0.42, 0.15];
    const MAGENTA: [f32; 3] = [1.0, 0.18, 0.30];
    const YELLOW: [f32; 3] = [1.0, 0.85, 0.22];
    if color < 0.0 { mix3(BASE, MAGENTA, -color) } else { mix3(BASE, YELLOW, color) }
}

const DUST_LIGHT: [f32; 3] = [0.92, 0.92, 0.90];
const DUST_DARK: [f32; 3] = [0.006, 0.006, 0.006];
const BACKLIGHT: [f32; 3] = [0.95, 0.94, 0.90];
const SLIDE_BASE: [f32; 3] = [0.006, 0.006, 0.007];
const NEGATIVE_BASE: [f32; 3] = [0.70, 0.27, 0.09];
const SLIDE_PRINT: [f32; 3] = [0.85, 0.42, 0.06];
const NEGATIVE_PRINT: [f32; 3] = [0.12, 0.04, 0.015];
const FILED_BLACK: [f32; 3] = [0.004, 0.004, 0.004];
const PAPER_WHITE: [f32; 3] = [0.80, 0.79, 0.76];
const PAPER_BLACK: [f32; 3] = [0.010, 0.010, 0.011];

// ---------- the 35mm strip (millimetres, the public 35mm film dimensions)

const FILM_WIDTH: f32 = 35.0;
/// The frame's image area and its pitch along the film (8 perforations).
const IMAGE_W: f32 = 36.0;
const IMAGE_H: f32 = 24.0;
const FRAME_PITCH: f32 = 38.0;
const PERF_PITCH: f32 = 4.75;
/// A perforation: along the film × across it, its corner radius, and its outer edge's distance from the film's edge.
const PERF_ALONG: f32 = 1.98;
const PERF_ACROSS: f32 = 2.79;
const PERF_RADIUS: f32 = 0.5;
const PERF_MARGIN: f32 = 2.01;
const WINDOW_RADIUS: f32 = 0.3;
/// Edge print: a 5 × 7 dot matrix, dot pitch in mm.
const DOT_PITCH: f32 = 0.17;

/// 5 × 7 dot glyphs (rows top to bottom, bit 4 the leftmost dot): 0–9, A, and an arrow.
const GLYPHS: [[u8; 7]; 12] = [
    [0x0e, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0e],
    [0x04, 0x0c, 0x04, 0x04, 0x04, 0x04, 0x0e],
    [0x0e, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1f],
    [0x1f, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0e],
    [0x02, 0x06, 0x0a, 0x12, 0x1f, 0x02, 0x02],
    [0x1f, 0x10, 0x1e, 0x01, 0x01, 0x11, 0x0e],
    [0x06, 0x08, 0x10, 0x1e, 0x11, 0x11, 0x0e],
    [0x1f, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
    [0x0e, 0x11, 0x11, 0x0e, 0x11, 0x11, 0x0e],
    [0x0e, 0x11, 0x11, 0x0f, 0x01, 0x02, 0x0c],
    [0x0e, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11],
    [0x10, 0x18, 0x1c, 0x1e, 0x1c, 0x18, 0x10],
];
const GLYPH_A: usize = 10;
const GLYPH_ARROW: usize = 11;

/// Signed distance to a rounded rectangle of half-size `(hx, hy)` and corner radius `r`, centred on the origin.
fn rounded_rect(x: f32, y: f32, hx: f32, hy: f32, r: f32) -> f32 {
    let (qx, qy) = (x.abs() - hx + r, y.abs() - hy + r);
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    outside + qx.max(qy).min(0.0) - r
}

// ---------- setup

/// One mark as drawn: the settings' numbers in their working units.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Layer {
    kind: FilmKind,
    seed: u32,
    /// 0..1 each, except angle (radians), offset, color and tone (−1..1).
    amount: f32,
    size: f32,
    angle: f32,
    offset: f32,
    color: f32,
    texture: f32,
    tone: f32,
    fibres: f32,
}

/// What a render draws for a photo's film settings.
#[derive(Clone, Debug, PartialEq)]
pub struct Setup {
    frame: FilmFrame,
    tone: FrameTone,
    border: f32,
    markings: bool,
    /// The strip's frame number, 1..36.
    number: u32,
    layers: Vec<Layer>,
    /// Linear sRGB → linear output RGB.
    to_out: [[f32; 3]; 3],
}

/// Where the photo goes in a frame: its window in the whole output's pixels, `[x0, y0, x1, y1)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    pub x0: usize,
    pub y0: usize,
    pub x1: usize,
    pub y1: usize,
}

fn unit(v: f64) -> f32 {
    if v.is_finite() { (v / 100.0).clamp(0.0, 1.0) as f32 } else { 0.0 }
}

fn signed(v: f64) -> f32 {
    if v.is_finite() { (v / 100.0).clamp(-1.0, 1.0) as f32 } else { 0.0 }
}

impl Setup {
    /// The film effects of `s` for a photo whose id hashes to `salt` ([`salt_of`]), drawn into `space`; `None` when
    /// there is nothing to draw (no frame and no visible mark, or the Film eye off).
    /// `frame`: whether the frame is drawn (not in an uncropped view, nor for a screen that maps taps onto the photo).
    pub fn new(s: &DevelopSettings, salt: u32, space: OutputSpace, frame: bool) -> Option<Setup> {
        if !s.section_enabled("film") {
            return None;
        }
        let f = &s.film;
        let layers: Vec<Layer> = f
            .layers
            .iter()
            .take(lightcraft_develop::MAX_FILM_LAYERS)
            .filter(|l| l.visible && l.amount.is_finite() && l.amount > 0.0)
            .map(|l| Layer {
                kind: l.kind,
                seed: effective_seed(l.seed, salt, f.vary),
                amount: unit(l.amount),
                size: unit(l.size),
                angle: if l.angle.is_finite() { (l.angle.clamp(-180.0, 180.0) as f32).to_radians() } else { 0.0 },
                offset: signed(l.offset),
                color: signed(l.color),
                texture: unit(l.texture),
                tone: signed(l.tone),
                fibres: unit(l.fibres),
            })
            .collect();
        let frame = if frame { f.frame } else { FilmFrame::None };
        if frame == FilmFrame::None && layers.is_empty() {
            return None;
        }
        let number = 1 + effective_seed(0x2545_f491, salt, f.vary) % 36;
        let to_out = OutputSpace::Srgb.rgb_space().to_space(&space.rgb_space()).to_f32();
        Some(Setup { frame, tone: f.frame_tone, border: unit(f.border), markings: f.markings, number, layers, to_out })
    }

    /// Whether a frame is drawn (the photo then shrinks into its window, so a render needs the whole output).
    pub fn has_frame(&self) -> bool {
        self.frame != FilmFrame::None
    }

    fn out(&self, c: [f32; 3]) -> [f32; 3] {
        let m = &self.to_out;
        std::array::from_fn(|i| m[i][0] * c[0] + m[i][1] * c[1] + m[i][2] * c[2])
    }

    /// The long-edge units per millimetre of film of a 35mm strip in a `w × h` output (long edge 1): the film's width
    /// fills the short side, unless the frame and its gaps would not fit along the long side.
    fn strip_scale(w: usize, h: usize) -> f32 {
        let long = w.max(h).max(1) as f32;
        let short = w.min(h).max(1) as f32 / long;
        (short / FILM_WIDTH).min(1.0 / FRAME_PITCH)
    }

    /// The frame's window in a `w × h` output (pixels), `None` without a frame.
    pub fn window(&self, w: usize, h: usize) -> Option<Window> {
        let (wf, hf) = (w.max(1) as f32, h.max(1) as f32);
        let long = wf.max(hf);
        // a rectangle in long-edge units → whole pixels (rounded half up, as the native kernel does), at least one
        let rect = |x0: f32, y0: f32, x1: f32, y1: f32| {
            let px = |v: f32, max: f32| (v * long + 0.5).floor().clamp(0.0, max) as usize;
            let (x0, y0) = (px(x0, wf - 1.0), px(y0, hf - 1.0));
            let (x1, y1) = (px(x1, wf).max(x0 + 1), px(y1, hf).max(y0 + 1));
            Window { x0, y0, x1, y1 }
        };
        let (uw, uh) = (wf / long, hf / long);
        match self.frame {
            FilmFrame::None => None,
            FilmFrame::Strip => {
                let k = Self::strip_scale(w, h);
                let (ha, hc) = (IMAGE_W / 2.0 * k, IMAGE_H / 2.0 * k);
                let (hx, hy) = if w >= h { (ha, hc) } else { (hc, ha) };
                Some(rect(uw / 2.0 - hx, uh / 2.0 - hy, uw / 2.0 + hx, uh / 2.0 + hy))
            }
            FilmFrame::Filed => {
                let b = self.filed_border();
                Some(rect(b, b, uw - b, uh - b))
            }
            FilmFrame::Instant => {
                let (side, bottom) = self.instant_borders(uh);
                Some(rect(side, side, uw - side, uh - bottom))
            }
        }
    }

    fn filed_border(&self) -> f32 {
        0.012 + 0.10 * self.border
    }

    /// An instant frame's side (and top) border and its bottom, long-edge units; the window keeps at least 30 % of the
    /// output's height.
    fn instant_borders(&self, uh: f32) -> (f32, f32) {
        let side = 0.02 + 0.07 * self.border;
        let bottom = (side * 3.5).min((uh * 0.7 - side).max(side));
        (side, bottom)
    }

    // ---------- drawing

    /// Draw onto an 8-bit picture (display-encoded with `trc`) that is the window at `origin` of a `full` output.
    pub fn apply_rgba8(&self, img: &mut Rgba8, origin: [usize; 2], full: [usize; 2], trc: OutputTrc) {
        let lut: Vec<f32> = (0..256).map(|i| trc.decode(i as f32 / 255.0)).collect();
        let mut lin: Vec<[f32; 3]> = img.data.iter().map(|p| [p[0], p[1], p[2]].map(|v| lut.get(v as usize).copied().unwrap_or(0.0))).collect();
        self.apply_linear(&mut lin, img.width, img.height, origin, full);
        for (p, c) in img.data.iter_mut().zip(&lin) {
            let e = c.map(|v| (trc.encode(v) * 255.0 + 0.5) as u8);
            *p = [e[0], e[1], e[2], p[3]];
        }
    }

    /// Draw onto a high-bit-depth picture (16-bit display-encoded, or linear float) at `origin` of a `full` output.
    pub fn apply_deep(&self, img: &mut DeepImage, origin: [usize; 2], full: [usize; 2]) {
        let trc = img.space.trc();
        let (w, h) = (img.width, img.height);
        match &mut img.samples {
            DeepSamples::U16(v) => {
                let mut lin: Vec<[f32; 3]> = v.as_chunks::<3>().0.iter().map(|c| c.map(|x| trc.decode(x as f32 / 65535.0))).collect();
                self.apply_linear(&mut lin, w, h, origin, full);
                for (o, c) in v.as_chunks_mut::<3>().0.iter_mut().zip(&lin) {
                    *o = c.map(|x| (trc.encode(x) * 65535.0 + 0.5) as u16);
                }
            }
            DeepSamples::F32(v) => {
                let mut lin: Vec<[f32; 3]> = v.as_chunks::<3>().0.to_vec();
                self.apply_linear(&mut lin, w, h, origin, full);
                for (o, c) in v.as_chunks_mut::<3>().0.iter_mut().zip(&lin) {
                    *o = *c;
                }
            }
        }
    }

    /// Draw onto linear pixels (`w × h`, row-major) that sit at `origin` of a `full` output. With a frame the photo is
    /// first shrunk into the frame's window, which needs the whole output (`origin` 0 and `full` the picture's size): a
    /// window of a framed render is cut from the whole one ([`crate::Plan`] grows it).
    pub fn apply_linear(&self, px: &mut [[f32; 3]], w: usize, h: usize, origin: [usize; 2], full: [usize; 2]) {
        if w == 0 || h == 0 || px.len() < w.saturating_mul(h) {
            return;
        }
        let whole = origin == [0, 0] && full == [w, h];
        let window = if whole { self.window(w, h) } else { None };
        if let Some(win) = window {
            let inset = inset(px, w, h, win);
            for (d, s) in px.iter_mut().zip(&inset) {
                *d = *s;
            }
        }
        let long = full[0].max(full[1]).max(1) as f32;
        let ctx = Ctx {
            uw: full[0].max(1) as f32 / long,
            uh: full[1].max(1) as f32 / long,
            pu: 1.0 / long,
            window: window.map(|r| [r.x0 as f32 / long, r.y0 as f32 / long, r.x1 as f32 / long, r.y1 as f32 / long]),
            landscape: full[0] >= full[1],
            strip_k: Self::strip_scale(full[0], full[1]),
        };
        px.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            let v = (origin[1] + y) as f32 + 0.5;
            for (x, c) in row.iter_mut().enumerate() {
                let u = (origin[0] + x) as f32 + 0.5;
                *c = self.shade(&ctx, u / long, v / long, *c);
            }
        });
    }

    /// One pixel at `(u, v)` (long-edge units from the output's top-left) over the photo's colour `c`.
    fn shade(&self, ctx: &Ctx, u: f32, v: f32, c: [f32; 3]) -> [f32; 3] {
        let mut c = match self.frame {
            FilmFrame::None => c,
            FilmFrame::Strip => self.strip(ctx, u, v, c),
            FilmFrame::Filed => self.filed(ctx, u, v, c),
            FilmFrame::Instant => self.instant(ctx, u, v, c),
        };
        let q = [u - ctx.uw / 2.0, v - ctx.uh / 2.0];
        for l in &self.layers {
            c = match l.kind {
                FilmKind::Leak => self.light(c, leak(ctx, l, q), l.color),
                FilmKind::Streak => self.light(c, streak(ctx, l, q), l.color),
                FilmKind::Burn => self.light(c, burn(ctx, l, q, u, v), l.color),
                FilmKind::Dust => self.marks(c, dust(ctx, l, q)),
                FilmKind::Scratches => self.marks(c, scratches(ctx, l, q)),
            };
        }
        c
    }

    /// Leaked light of intensity `i` over `c`: a screen in SDR, nothing added over white in an HDR render.
    fn light(&self, c: [f32; 3], i: f32, color: f32) -> [f32; 3] {
        if i.is_nan() || i <= 1e-4 {
            return c;
        }
        let k = response_k(color);
        let l = self.out([0, 1, 2].map(|j| 1.0 - (-i * k[j]).exp()));
        std::array::from_fn(|j| c[j] + l[j] * (1.0 - c[j].clamp(0.0, 1.0)))
    }

    /// Light and dark marks (coverage 0..1 each) over `c`.
    fn marks(&self, c: [f32; 3], (light, dark): (f32, f32)) -> [f32; 3] {
        let c = if light > 0.0 { mix3(c, self.out(DUST_LIGHT), light) } else { c };
        if dark > 0.0 { mix3(c, self.out(DUST_DARK), dark) } else { c }
    }

    fn strip(&self, ctx: &Ctx, u: f32, v: f32, c: [f32; 3]) -> [f32; 3] {
        let k = ctx.strip_k;
        let (du, dv) = (u - ctx.uw / 2.0, v - ctx.uh / 2.0);
        // along and across the film, millimetres from the frame's centre
        let (a, x) = if ctx.landscape { (du / k, dv / k) } else { (dv / k, du / k) };
        // (a scan is never quite sharp: edges soften over at least 0.06 mm)
        let aa = (ctx.pu / k * 0.75).max(0.06);
        let (base, print) = match self.tone {
            FrameTone::Dark => (SLIDE_BASE, SLIDE_PRINT),
            FrameTone::Light => (NEGATIVE_BASE, NEGATIVE_PRINT),
        };
        // the base is never quite even
        let density = fbm(a / 30.0, x / 30.0, 0x7f4a_7c15, 2, ctx.pu / k / 30.0) - 0.5;
        let mut f = base.map(|b| b * (1.0 + 0.12 * density));
        // past the film's edges: the scanner's light
        let edge = x.abs() - FILM_WIDTH / 2.0;
        f = mix3(f, BACKLIGHT, smoothstep(-aa, aa, edge));
        // the perforations, backlit
        let j = (a / PERF_PITCH - 0.5).round();
        let pa = a - (j + 0.5) * PERF_PITCH;
        let pc = x.abs() - (FILM_WIDTH / 2.0 - PERF_MARGIN - PERF_ACROSS / 2.0);
        let hole = rounded_rect(pa, pc, PERF_ALONG / 2.0, PERF_ACROSS / 2.0, PERF_RADIUS);
        // the backlight glows a little into the base round each hole
        f = mix3(f, BACKLIGHT, 0.1 * (-hole.max(0.0) / 0.25).exp());
        f = mix3(f, BACKLIGHT, smoothstep(aa, -aa, hole));
        // the edge print, between the holes and the film's edge on one side
        if self.markings && x > 0.0 {
            let dot = self.edge_print(a, x, aa);
            f = mix3(f, print, dot * 0.9);
        }
        let window = rounded_rect(a, x, IMAGE_W / 2.0, IMAGE_H / 2.0, WINDOW_RADIUS);
        let inside = smoothstep(aa, -aa, window) * self.in_window(ctx, u, v);
        mix3(self.out(f), c, inside)
    }

    /// The dot coverage of the strip's edge print at `(a, x)` mm: an arrow and the frame number, then the number and "A"
    /// half a frame on.
    fn edge_print(&self, a: f32, x: f32, aa: f32) -> f32 {
        let top = FILM_WIDTH / 2.0 - PERF_MARGIN / 2.0 - 3.5 * DOT_PITCH;
        let row = ((x - top) / DOT_PITCH).floor();
        if !(0.0..7.0).contains(&row) {
            return 0.0;
        }
        let n = self.number;
        let mut digits = [0usize; 2];
        let len = if n >= 10 {
            digits = [(n / 10) as usize, (n % 10) as usize];
            2
        } else {
            digits[0] = n as usize;
            1
        };
        let first: Vec<usize> = std::iter::once(GLYPH_ARROW).chain(digits.iter().take(len).copied()).collect();
        let second: Vec<usize> = digits.iter().take(len).copied().chain(std::iter::once(GLYPH_A)).collect();
        let mut cov = 0.0f32;
        for (start, text) in [(-15.0f32, &first), (4.0f32, &second)] {
            let col_f = (a - start) / DOT_PITCH;
            if col_f < 0.0 {
                continue;
            }
            let col = col_f.floor();
            let (g, dx) = ((col / 6.0).floor() as usize, (col % 6.0) as usize);
            let Some(&glyph) = text.get(g) else { continue };
            let bits = GLYPHS.get(glyph).and_then(|rows| rows.get(row as usize)).copied().unwrap_or(0);
            if dx < 5 && bits & (0x10 >> dx) != 0 {
                let (cx, cy) = (start + (col + 0.5) * DOT_PITCH, top + (row + 0.5) * DOT_PITCH);
                let d = ((a - cx).powi(2) + (x - cy).powi(2)).sqrt();
                let r = 0.42 * DOT_PITCH;
                // (a dot smaller than a pixel fades by its area)
                let area = (r / aa).min(1.0).powi(2);
                cov = cov.max(smoothstep(r + aa, r - aa, d) * area);
            }
        }
        cov
    }

    /// 1 inside the frame's window (in whole pixels, where the photo was put), 0 outside.
    fn in_window(&self, ctx: &Ctx, u: f32, v: f32) -> f32 {
        match ctx.window {
            Some([x0, y0, x1, y1]) => {
                let d = (u - x0).min(x1 - u).min(v - y0).min(y1 - v);
                smoothstep(-ctx.pu * 0.5, ctx.pu * 0.5, d)
            }
            None => 1.0,
        }
    }

    fn filed(&self, ctx: &Ctx, u: f32, v: f32, c: [f32; 3]) -> [f32; 3] {
        let b = self.filed_border();
        let Some([x0, y0, x1, y1]) = ctx.window else { return c };
        let seed = 0x51ed_270b;
        // each side's edge, filed by hand: a slow wander and fine teeth, into the photo by up to a third of the border
        let side = |d: f32, along: f32, n: u32| {
            let wander = fbm(along / (b * 4.0), n as f32 * 7.1, seed ^ n, 3, ctx.pu / (b * 4.0)) - 0.5;
            let teeth = fbm(along / (b * 0.18), n as f32 * 3.7, seed.wrapping_add(n), 3, ctx.pu / (b * 0.18)) - 0.5;
            d - b * (0.2 + 0.45 * wander + 0.03 * teeth)
        };
        let d = side(u - x0, v, 1).min(side(x1 - u, v, 2)).min(side(v - y0, u, 3)).min(side(y1 - v, u, 4));
        let inside = smoothstep(-ctx.pu * 0.75, ctx.pu * 0.75, d);
        mix3(self.out(FILED_BLACK), c, inside)
    }

    fn instant(&self, ctx: &Ctx, u: f32, v: f32, c: [f32; 3]) -> [f32; 3] {
        let Some([x0, y0, x1, y1]) = ctx.window else { return c };
        let paper = match self.tone {
            FrameTone::Light => PAPER_WHITE,
            FrameTone::Dark => PAPER_BLACK,
        };
        // a faint paper texture
        let grain = fbm(u * 300.0, v * 300.0, 0x1b87_3593, 3, ctx.pu * 300.0) - 0.5;
        let paper = paper.map(|p| p * (1.0 + 0.06 * grain));
        let d = (u - x0).min(x1 - u).min(v - y0).min(y1 - v);
        let inside = smoothstep(-ctx.pu * 0.5, ctx.pu * 0.5, d);
        // the picture sits a little below the frame: a soft shadow along its top and left edges
        let shade = 1.0 - 0.22 * (-((u - x0).min(v - y0).max(0.0)) / 0.003).exp();
        let c = c.map(|x| x * shade);
        mix3(self.out(paper), c, inside)
    }
}

/// Where a pixel is: the whole output's size in long-edge units, a pixel's size, the frame's window.
struct Ctx {
    uw: f32,
    uh: f32,
    pu: f32,
    window: Option<[f32; 4]>,
    landscape: bool,
    strip_k: f32,
}

/// The direction light comes from (a unit vector, image axes, y down) for an angle counter-clockwise from the right.
fn direction(angle: f32) -> [f32; 2] {
    [angle.cos(), -angle.sin()]
}

/// Depth `t` into the frame from the edge (or corner) light at `dir` comes from, and the place `s` along that edge.
fn from_edge(ctx: &Ctx, dir: [f32; 2], q: [f32; 2]) -> (f32, f32) {
    let ext = dir[0].abs() * ctx.uw / 2.0 + dir[1].abs() * ctx.uh / 2.0;
    (ext - (q[0] * dir[0] + q[1] * dir[1]), -q[0] * dir[1] + q[1] * dir[0])
}

fn texture_mod(texture: f32, n: f32) -> f32 {
    1.0 - texture + texture * 1.6 * smoothstep(0.2, 0.8, n)
}

/// A light leak's intensity: light fogging the film from an edge, strongest at the edge with a long tail, uneven
/// along the edge and in bands along its path.
fn leak(ctx: &Ctx, l: &Layer, q: [f32; 2]) -> f32 {
    let dir = direction(l.angle);
    let (t, s) = from_edge(ctx, dir, q);
    let reach = 0.05 + 0.6 * l.size;
    let edge_len = dir[1].abs() * ctx.uw + dir[0].abs() * ctx.uh;
    let centre = (rand_n(0, 0, l.seed, 1) - 0.5) * 0.9 * edge_len;
    let spread = reach * (0.6 + 0.8 * rand_n(0, 0, l.seed, 2)) + 0.08;
    // (the light spreads as it goes in: the patch widens away from the edge)
    let width = spread * (1.0 + 0.8 * t / reach);
    let envelope = (-((s - centre) / width).powi(2)).exp();
    let falloff = 0.65 * (-t / (0.25 * reach)).exp() + 0.35 * (-2.0 * (t / reach).powi(2)).exp();
    // soft bands along the light's path, bent by a slower warp
    let warp = fbm(s * 1.5 / reach, t * 0.8 / reach, l.seed ^ 0x1111, 2, ctx.pu * 1.5 / reach) - 0.5;
    let n = fbm((s + 0.6 * reach * warp) * 1.8 / reach, t * 0.5 / reach, l.seed, 2, ctx.pu * 1.8 / reach);
    l.amount * 3.0 * falloff * envelope * texture_mod(l.texture, n)
}

/// A light streak's intensity: a band across the frame with a hot core, uneven along its length.
fn streak(ctx: &Ctx, l: &Layer, q: [f32; 2]) -> f32 {
    let d = direction(l.angle);
    let n = [-d[1], d[0]];
    let ext = n[0].abs() * ctx.uw / 2.0 + n[1].abs() * ctx.uh / 2.0;
    let dist = q[0] * n[0] + q[1] * n[1] - l.offset * ext;
    let along = q[0] * d[0] + q[1] * d[1];
    let width = 0.01 + 0.3 * l.size.powf(1.5);
    let band = (-2.0 * (dist / width).powi(2)).exp() + 0.6 * (-2.0 * (dist / (0.25 * width)).powi(2)).exp();
    let noise = fbm(along / (3.0 * width), dist / width, l.seed, 4, ctx.pu / width);
    l.amount * 2.4 * band * texture_mod(l.texture, noise)
}

/// An edge burn's intensity: the film blown out from an edge to a ragged, speckled boundary, with a glow past it.
fn burn(ctx: &Ctx, l: &Layer, q: [f32; 2], u: f32, v: f32) -> f32 {
    let dir = direction(l.angle);
    let (t, s) = from_edge(ctx, dir, q);
    let depth = 0.006 + 0.22 * l.size.powf(1.6);
    let rough = l.texture;
    // tongues of light reaching in, uneven along the edge
    let scale = 1.5 * depth + 0.01;
    let jag = fbm(s / scale, 0.5, l.seed, 3, ctx.pu / scale) - 0.5;
    let edge = depth * (1.0 + rough * 1.6 * jag).max(0.15);
    // and a fine, grainy boundary
    let speck = (fbm(u / 0.0015, v / 0.0015, l.seed ^ 0x2222, 2, ctx.pu / 0.0015) - 0.5) * rough;
    let soft = (ctx.pu * 0.75).max(0.12 * depth);
    let inside = smoothstep(edge + soft, edge - soft, t + speck * 0.6 * depth);
    let glow = (-(t - edge).max(0.0) / (0.8 * depth + 0.015)).exp();
    l.amount * (5.0 * inside + 1.2 * glow * (1.0 - inside))
}

/// Dust: light and dark coverage of the specks and hairs at `q`.
fn dust(ctx: &Ctx, l: &Layer, q: [f32; 2]) -> (f32, f32) {
    let (u, v) = (q[0] + ctx.uw / 2.0, q[1] + ctx.uh / 2.0);
    let scale = 0.45 + 1.4 * l.size;
    let light_share = (l.tone + 1.0) / 2.0;
    let (mut light, mut dark) = (0.0f32, 0.0f32);
    let mut add = |cov: f32, pick: f32| {
        if pick < light_share {
            light = light.max(cov);
        } else {
            dark = dark.max(cov);
        }
    };
    // three sizes of speck: (cell, chance per cell at Amount 100, smallest and largest radius in cells)
    const OCTAVES: [(f32, f32, f32, f32); 3] = [(1.0 / 10.0, 0.06, 0.03, 0.10), (1.0 / 28.0, 0.10, 0.03, 0.10), (1.0 / 75.0, 0.12, 0.04, 0.12)];
    for (o, &(cell, chance, rmin, rmax)) in OCTAVES.iter().enumerate() {
        let p = chance * l.amount;
        let seed = l.seed.wrapping_add((o as u32 + 1).wrapping_mul(0x27d4_eb2f));
        let (cx, cy) = ((u / cell).floor() as i32, (v / cell).floor() as i32);
        for jy in -1..=1 {
            for jx in -1..=1 {
                let (ix, iy) = (cx.wrapping_add(jx), cy.wrapping_add(jy));
                if rand_n(ix, iy, seed, 0) > p {
                    continue;
                }
                let r = |n| rand_n(ix, iy, seed, n);
                let centre = [(ix as f32 + 0.2 + 0.6 * r(1)) * cell, (iy as f32 + 0.2 + 0.6 * r(2)) * cell];
                // (most specks are small: the radius leans to its smallest)
                let radius = cell * (rmin + (rmax - rmin) * r(3).powi(3)) * scale;
                // a fleck: turned, squashed, with an irregular outline (noise read round a circle)
                let (sin, cos) = (r(4) * TAU).sin_cos();
                let (dx, dy) = (u - centre[0], v - centre[1]);
                let (px, py) = (dx * cos + dy * sin, (-dx * sin + dy * cos) * (1.0 + 2.0 * r(5).powi(2)));
                let phi = py.atan2(px);
                let ring = value_noise(1.4 * phi.cos() + 9.0 * r(6), 1.4 * phi.sin() + 9.0 * r(7), seed ^ 0x0bad_5eed);
                let rr = radius * (0.65 + 0.7 * ring);
                let aa = (ctx.pu * 0.75).max(l.texture * radius * 0.8);
                let area = (rr / (ctx.pu * 0.75)).min(1.0).powi(2);
                let cov = smoothstep(rr + aa, rr - aa, (px * px + py * py).sqrt()) * area * (0.55 + 0.45 * r(8));
                if cov > 0.0 {
                    add(cov, r(9));
                }
            }
        }
    }
    // hairs: thin curves, each shorter than its cell
    let p = (0.6 * l.amount * l.fibres).min(1.0);
    if p > 0.0 {
        let cell = 1.0 / 6.0;
        let seed = l.seed ^ 0x6c8e_9cf5;
        let (cx, cy) = ((u / cell).floor() as i32, (v / cell).floor() as i32);
        for jy in -1..=1 {
            for jx in -1..=1 {
                let (ix, iy) = (cx.wrapping_add(jx), cy.wrapping_add(jy));
                if rand_n(ix, iy, seed, 0) > p {
                    continue;
                }
                let r = |n| rand_n(ix, iy, seed, n);
                let centre = [(ix as f32 + r(1)) * cell, (iy as f32 + r(2)) * cell];
                let len = cell * (0.35 + 0.6 * r(3));
                let (sin, cos) = (r(4) * PI).sin_cos();
                // a cubic through two bends: a hair curls, it is not an arc
                let (b1, b2) = ((r(5) - 0.5) * 0.9 * len, (r(9) - 0.5) * 0.9 * len);
                let at = |a: f32, b: f32| [centre[0] + cos * a - sin * b, centre[1] + sin * a + cos * b];
                let pts = [at(-len / 2.0, 0.0), at(-len / 6.0, b1), at(len / 6.0, b2), at(len / 2.0, 0.0)];
                let width = (0.00025 + 0.0006 * r(6)) * scale;
                let (dist, t) = cubic_distance(pts, [u, v]);
                // thinner toward its ends
                let width = width * (0.35 + 0.65 * smoothstep(0.0, 0.25, t) * smoothstep(1.0, 0.75, t));
                let aa = (ctx.pu * 0.75).max(l.texture * width * 1.5);
                let thin = (width / (ctx.pu * 0.5)).min(1.0);
                let cov = smoothstep(width + aa, width - aa, dist) * thin * (0.6 + 0.4 * r(7));
                if cov > 0.0 {
                    add(cov, r(8));
                }
            }
        }
    }
    (light, dark)
}

/// Distance from `p` to the cubic Bézier through `c`, along 16 straight pieces, and where along it (0..1) it is nearest.
fn cubic_distance(c: [[f32; 2]; 4], p: [f32; 2]) -> (f32, f32) {
    let at = |t: f32| {
        let s = 1.0 - t;
        let (a, b, cc, d) = (s * s * s, 3.0 * s * s * t, 3.0 * s * t * t, t * t * t);
        [a * c[0][0] + b * c[1][0] + cc * c[2][0] + d * c[3][0], a * c[0][1] + b * c[1][1] + cc * c[2][1] + d * c[3][1]]
    };
    const PIECES: usize = 16;
    let (mut best, mut best_t) = (f32::MAX, 0.0);
    let mut prev = c[0];
    for i in 1..=PIECES {
        let next = at(i as f32 / PIECES as f32);
        let (ex, ey) = (next[0] - prev[0], next[1] - prev[1]);
        let len2 = ex * ex + ey * ey;
        let k = if len2 > 0.0 { (((p[0] - prev[0]) * ex + (p[1] - prev[1]) * ey) / len2).clamp(0.0, 1.0) } else { 0.0 };
        let (dx, dy) = (p[0] - prev[0] - k * ex, p[1] - prev[1] - k * ey);
        let d = (dx * dx + dy * dy).sqrt();
        if d < best {
            (best, best_t) = (d, (i as f32 - 1.0 + k) / PIECES as f32);
        }
        prev = next;
    }
    (best, best_t)
}

/// Scratches: light and dark coverage of long, wavering lines along the angle, broken in places.
fn scratches(ctx: &Ctx, l: &Layer, q: [f32; 2]) -> (f32, f32) {
    let d = direction(l.angle);
    let n = [-d[1], d[0]];
    let ext = n[0].abs() * ctx.uw / 2.0 + n[1].abs() * ctx.uh / 2.0;
    let across = q[0] * n[0] + q[1] * n[1];
    let along = q[0] * d[0] + q[1] * d[1];
    const SLOT: f32 = 0.025;
    let p = 0.5 * l.amount;
    let light_share = (l.tone + 1.0) / 2.0;
    let (mut light, mut dark) = (0.0f32, 0.0f32);
    let j0 = ((across + ext) / SLOT).floor() as i32;
    for j in [j0.wrapping_sub(1), j0, j0.wrapping_add(1)] {
        if rand_n(j, 0, l.seed, 0) > p {
            continue;
        }
        let r = |k| rand_n(j, 0, l.seed, k);
        let wobble = 0.006 * (value_noise(along * 2.5, j as f32 * 7.3, l.seed ^ 0x33) - 0.5)
            + 0.0015 * (value_noise(along * 14.0, j as f32 * 1.9, l.seed ^ 0x44) - 0.5);
        let centre = -ext + (j as f32 + r(1)) * SLOT + wobble;
        let width = (0.0002 + 0.0022 * l.size.powf(1.5)) * (0.5 + r(2));
        let dist = (across - centre).abs();
        let aa = ctx.pu * 0.75;
        let thin = (width / (ctx.pu * 0.5)).min(1.0);
        let line = smoothstep(width + aa, width - aa, dist) * thin;
        let glow = 0.25 * (-(dist / (4.0 * width + ctx.pu)).powi(2)).exp() * thin;
        let gate = l.texture * 0.75;
        let present = smoothstep(gate, gate + 0.08, value_noise(along * 3.0 + j as f32 * 17.0, 0.5, l.seed ^ 0x55));
        // a scratch runs part of the way (most of it, often), fading at its ends
        let reach = d[0].abs() * ctx.uw / 2.0 + d[1].abs() * ctx.uh / 2.0;
        let (mid, half) = ((r(5) - 0.5) * 2.0 * reach, reach * (0.3 + 1.2 * r(6)));
        let span = smoothstep(half, half * 0.85, (along - mid).abs());
        let cov = line.max(glow) * present * span * (0.5 + 0.5 * r(3));
        if cov > 0.0 {
            if r(4) < light_share {
                light = light.max(cov);
            } else {
                dark = dark.max(cov);
            }
        }
    }
    (light, dark)
}

/// The photo `px` (`w × h`) shrunk into `win`, covering it (centred): an area-weighted resample, in linear light. Pixels
/// outside the window repeat its nearest edge (the frame covers them).
fn inset(px: &[[f32; 3]], w: usize, h: usize, win: Window) -> Vec<[f32; 3]> {
    let (ww, wh) = (win.x1.saturating_sub(win.x0).max(1), win.y1.saturating_sub(win.y0).max(1));
    let s = (ww as f64 / w as f64).max(wh as f64 / h as f64);
    let (sw, sh) = (ww as f64 / s, wh as f64 / s);
    let (sx0, sy0) = ((w as f64 - sw) / 2.0, (h as f64 - sh) / 2.0);
    let cols = area_weights(sx0, sw / ww as f64, ww, w);
    let rows = area_weights(sy0, sh / wh as f64, wh, h);
    // horizontally: every source row into the window's width
    let mut tmp = vec![[0.0f32; 3]; ww * h];
    tmp.par_chunks_mut(ww).enumerate().for_each(|(y, out)| {
        let Some(src) = px.get(y * w..(y + 1) * w) else { return };
        for (o, taps) in out.iter_mut().zip(&cols) {
            *o = taps.iter().fold([0.0; 3], |acc, &(i, k)| {
                let c = src.get(i).copied().unwrap_or([0.0; 3]);
                [acc[0] + c[0] * k, acc[1] + c[1] * k, acc[2] + c[2] * k]
            });
        }
    });
    // vertically into the window, then out to the whole frame
    let mut out = vec![[0.0f32; 3]; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let wy = y.clamp(win.y0, win.y1.saturating_sub(1)) - win.y0;
        let Some(taps) = rows.get(wy) else { return };
        for (x, o) in row.iter_mut().enumerate() {
            let wx = x.clamp(win.x0, win.x1.saturating_sub(1)) - win.x0;
            *o = taps.iter().fold([0.0; 3], |acc, &(j, k)| {
                let c = tmp.get(j * ww + wx).copied().unwrap_or([0.0; 3]);
                [acc[0] + c[0] * k, acc[1] + c[1] * k, acc[2] + c[2] * k]
            });
        }
    });
    out
}

/// Source taps and weights for `n` destination pixels, each covering `step` source pixels from `start` (`len` source
/// pixels in all): the overlap of each source pixel with the destination pixel's span (at least one pixel wide).
fn area_weights(start: f64, step: f64, n: usize, len: usize) -> Vec<Vec<(usize, f32)>> {
    (0..n)
        .map(|i| {
            let (mut a, mut b) = (start + i as f64 * step, start + (i + 1) as f64 * step);
            if b - a < 1.0 {
                let m = (a + b) / 2.0;
                (a, b) = (m - 0.5, m + 0.5);
            }
            let (a, b) = (a.max(0.0), b.min(len as f64));
            let mut taps = Vec::new();
            let mut k = a.floor().max(0.0) as usize;
            while (k as f64) < b && k < len {
                let o = (b.min(k as f64 + 1.0) - a.max(k as f64)).max(0.0);
                if o > 0.0 {
                    taps.push((k, o as f32));
                }
                k += 1;
            }
            let sum: f32 = taps.iter().map(|t| t.1).sum();
            if sum > 0.0 {
                for t in &mut taps {
                    t.1 /= sum;
                }
            } else {
                taps.push((len.saturating_sub(1).min(a.max(0.0) as usize), 1.0));
            }
            taps
        })
        .collect()
}

#[cfg(test)]
mod tests;
