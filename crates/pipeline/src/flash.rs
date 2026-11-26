//! **Flash**: the look of a photo taken with a xenon flash on the camera, made from the photo's depth (its own depth
//! map, or the estimate Lens Blur stored in the settings, [`crate::lensblur::mattes_for`]).
//!
//! A flash lights what is near the lens and dies off with the inverse square of the distance, so what is far goes dark
//! and what is near is hot. The depth is a nearness `n` (1 nearest, 0 farthest) that is linear in inverse distance, so
//! the distance of a pixel in units of the subject's is `inv_subject / inv`, with `inv = 1/R + n (1 - 1/R)` and `R` how
//! deep the scene is in flash distances (Falloff). At every output pixel:
//!
//! ```text
//! E     = min((inv / inv_subject) ^ 2, 6) * reach(depth)
//! flash = Power * E * lambert * hotspot * (1 - Shadow * occlusion) * flash colour
//! out   = colour * (ambient * tint + flash) / (1 + ambient)     ambient = 2 ^ -stops, kept more where the pixel is a lamp
//! out  += Gloss * glint * flash colour                          a surface that faces the lens, near it
//! ```
//!
//! then a shoulder (a soft one to a hard clip), a contrast curve in display encoding, and noise where the flash does
//! not reach. The surface faces the lens by the slope of the depth (jumps in it are the edges of objects, not surfaces
//! turned away, and are dropped); the occlusion is a ray marched through the depth toward the flash's place beside the
//! lens (Shadow Offset, Flash Angle). The planes of depth, occlusion and facing are made at the depth map's own size
//! and sampled through the frame like the picture, their edges snapped to the photo's by the guided filter Lens Blur
//! snaps its depth with ([`crate::lensblur::nearness_out`]); Flash runs in scene-linear Rec. 2020 after Relight and before
//! Lens Blur, so every tone and colour tool works on it.
//!
//! The depth is prepared once ([`Prepared`]): eroded a few pixels (the estimate's edges are soft, and a halo of flash on
//! the background is the one artefact people see), with the far field flattened (a sky's relief is noise, and the
//! inverse square would swing the flash by it). A native (non-Rust) frontend draws the same formulas on the GPU, so every constant
//! here is public.

use std::sync::{Arc, Mutex, PoisonError};

use lightcraft_develop::DevelopSettings;
use lightcraft_geom::Orientation;
use lightcraft_raster::{Plane, Rgb32f};

use crate::for_rows;
use crate::geometry::Frame;
use crate::masks::Mattes;
use crate::relight::{RANGE_FEATHER, depth_identity, depth_key, levelled, normal_sigma, sample, smooth, smoothed, smoothed_by, srgb_to_working};

/// How deep the scene is, in flash distances (the distance of the farthest part over the nearest), at Falloff 0 and 100,
/// exponential between.
pub const R_MIN: f32 = 1.3;
pub const R_MAX: f32 = 14.0;
/// The most light a near surface gets, against the subject's.
pub const E_CAP: f32 = 6.0;
/// The distance law: 2 is the inverse square.
pub const EXPONENT: f32 = 2.0;
/// How far Lambert's term wraps (the flash is a small, near source).
pub const WRAP: f32 = 0.35;
/// Stops the ambient is below the flash-lit part at Ambient 0 (the ambient is `2^(-AMBIENT_STOPS * (1 - Ambient / 100))`).
pub const AMBIENT_STOPS: f32 = 6.0;
/// Stops added at the end, so a subject lit by the flash stays as bright as it was.
pub const EXPOSURE_STOPS: f32 = 0.1;
/// The beam's fall: `exp(-Hotspot * HOT_K * r^2)`, `r` the distance from the frame's centre over the half diagonal.
pub const HOT_K: f32 = 2.2;
/// A glint's strength at Gloss 100.
pub const GLOSS_K: f32 = 0.9;
/// A glint is small: where the surface faces the lens most (`nz` from `GLINT_FROM` over `GLINT_SPAN`, squared) and the skin already shows a
/// highlight, a pixel brighter than its surround: `PEAK_K` per stop above the local mean (`SURROUND` long edges, two box passes each way);
/// `PEAK_FLOOR` of the glint shows without one.
pub const GLINT_FROM: f32 = 0.965;
pub const GLINT_SPAN: f32 = 0.035;
pub const PEAK_K: f32 = 2.0;
pub const PEAK_FLOOR: f32 = 0.25;
pub const SURROUND: f32 = 0.02;
/// How far the flash sits from the lens at Shadow Offset 100, in long edges (where a ray rises a whole depth range).
pub const OFFSET_MAX: f32 = 0.12;
/// Steps of a shadow ray, full quality and draft.
pub const SHADOW_STEPS: usize = 28;
pub const SHADOW_STEPS_DRAFT: usize = 14;
/// How hard a nearer neighbour shadows a pixel: occlusion per unit of nearness above the ray.
pub const SHADOW_GAIN: f32 = 14.0;
/// An occluder has to stand well clear of what it shades (a nose does not shadow a cheek): the shadow is none under `GAP_FROM` of nearness between them,
/// all of it `GAP_SPAN` above. And what it falls on has to be lit by the flash at all (a wall far behind the subject gets little of it, so a shadow
/// there is not seen): none under `VISIBLE_FROM` of `E`, all of it `VISIBLE_SPAN` above.
pub const GAP_FROM: f32 = 0.12;
pub const GAP_SPAN: f32 = 0.25;
pub const VISIBLE_FROM: f32 = 0.25;
pub const VISIBLE_SPAN: f32 = 0.45;
/// Fill Shadows: a flash is a fill, so the shade the photo's own light left on what it lights (a cheek under the hair, a neck) is lifted toward what is around
/// it (`FILL_RADIUS` long edges, the log luminance's local mean, two box passes each way): up to `FILL_MAX_STOPS` stops below it, `FILL_K` of them at Fill 100,
/// on what the flash lights (`E` from `FILL_FROM` to full over `FILL_SPAN`).
pub const FILL_RADIUS: f32 = 0.05;
pub const FILL_MAX_STOPS: f32 = 1.4;
pub const FILL_K: f32 = 0.8;
pub const FILL_FROM: f32 = 0.5;
pub const FILL_SPAN: f32 = 0.5;
/// The erosion of the near part and the box radius that flattens the far field, in depth pixels at a depth map 960 pixels
/// long (they scale with its long side).
pub const ERODE_PX: f32 = 3.0;
pub const FLAT_PX: f32 = 42.0;
/// The far field is flattened fully where the nearness is under `FLAT_START`, and not at all over `FLAT_START + FLAT_SPAN`
/// (a ramp between).
pub const FLAT_START: f32 = 0.35;
pub const FLAT_SPAN: f32 = 0.3;
/// The depth's slope (per long edge) at Surface 100 turns a surface by `atan(SLOPE_K * long * gradient)`.
pub const SLOPE_K: f32 = 0.275;
/// The slope above which a depth step is the edge of an object, not a surface (soft cut: `exp(-(slope / SLOPE_CUT)^4)`).
pub const SLOPE_CUT: f32 = 2.0;
/// Rec. 2020 luminance weights.
pub const LUMA: [f32; 3] = [0.2627, 0.6780, 0.0593];
/// The noise's strength at Noise 100 (display encoding) and the size of its grain, in long edges per grain at most 1/1500.
pub const GRAIN_K: f32 = 0.045;
pub const GRAIN_CELLS: f32 = 1500.0;
/// The depth percentile of the middle of the frame that is the subject when none is picked.
pub const SUBJECT_PERCENTILE: f32 = 0.7;

type V3 = [f32; 3];

/// Whether a render of `s` flashes it (on, its section's eye on, with something to do).
pub fn flashes(s: &DevelopSettings) -> bool {
    s.flash.active() && s.section_enabled("flash")
}

fn num(v: f64, lo: f64, hi: f64, default: f64) -> f32 {
    (if v.is_finite() { v.clamp(lo, hi) } else { default }) as f32
}

/// A depth made ready for the flash: kept per depth, so a drag of a slider redraws without redoing it.
#[derive(Debug)]
pub struct Prepared {
    /// The smoothed nearness ([`smoothed`]).
    pub d: Plane,
    /// The nearness the flash reads: `d` eroded and its far field flattened.
    pub n: Plane,
    /// The depth's slope per long edge, scaled so that Surface 100 is 1: `SLOPE_K * long * |grad|` of the levelled depth.
    pub slope: Plane,
    /// The nearness of the near part of the middle of the frame ([`SUBJECT_PERCENTILE`]), the subject when none is picked.
    pub subject: f32,
}

static PREPARED_KEPT: Mutex<Vec<(u64, Arc<Prepared>)>> = Mutex::new(Vec::new());

/// `p` blurred by a box of radius `r` pixels across or down, its edges extended.
fn box_pass(p: &Plane, r: usize, across: bool) -> Plane {
    let (w, h) = (p.width, p.height);
    let mut out = Plane::new(w, h);
    if w == 0 || h == 0 || p.data.len() != w * h {
        return p.clone();
    }
    let (lines, len) = if across { (h, w) } else { (w, h) };
    let at = |line: usize, k: isize| {
        let k = k.clamp(0, len as isize - 1) as usize;
        if across { p.data[line * w + k] } else { p.data[k * w + line] }
    };
    let r = r as isize;
    let norm = 1.0 / (2 * r + 1) as f32;
    for line in 0..lines {
        let mut sum: f32 = (-r..=r).map(|k| at(line, k)).sum();
        for i in 0..len as isize {
            let v = sum * norm;
            if across {
                out.data[line * w + i as usize] = v;
            } else {
                out.data[i as usize * w + line] = v;
            }
            sum += at(line, i + r + 1) - at(line, i - r);
        }
    }
    out
}

/// `n` box passes each way: three are close to a Gaussian of sigma `r * 1.15`.
fn blur_n(p: &Plane, r: usize, n: usize) -> Plane {
    let mut q = p.clone();
    for _ in 0..n {
        q = box_pass(&box_pass(&q, r, true), r, false);
    }
    q
}

/// `d` with every pixel the least of those within `r` pixels (a disc).
fn erode(d: &Plane, r: isize) -> Plane {
    let (w, h) = (d.width, d.height);
    let mut out = Plane::new(w, h);
    let disc: Vec<(isize, isize)> = (-r..=r).flat_map(|y| (-r..=r).map(move |x| (x, y))).filter(|(x, y)| x * x + y * y <= r * r).collect();
    for_rows(&mut out.data, w, |y, row| {
        for (x, v) in row.iter_mut().enumerate() {
            *v = disc.iter().map(|(dx, dy)| d.get_clamped(x as isize + dx, y as isize + dy)).fold(f32::INFINITY, f32::min);
        }
    });
    out
}

impl Prepared {
    fn new(depth: &Plane) -> Prepared {
        let (w, h) = (depth.width, depth.height);
        let long = w.max(h) as f32;
        let d = smoothed(depth);
        let shape = smoothed_by(&levelled(depth), normal_sigma(w.max(h)));
        // the near part pulled in, then the far field flattened (only far pixels feed the smoothing, or the subject's
        // nearness would smear into a glow around it)
        let eroded = erode(&d, (ERODE_PX * long / 960.0).round().max(1.0) as isize);
        let far = Plane { width: w, height: h, data: eroded.data.iter().map(|n| 1.0 - ((n - FLAT_START) / FLAT_SPAN).clamp(0.0, 1.0)).collect() };
        let r = (FLAT_PX * long / 960.0).round().max(1.0) as usize;
        let num = blur_n(&Plane { width: w, height: h, data: eroded.data.iter().zip(&far.data).map(|(n, f)| n * f).collect() }, r, 3);
        let den = blur_n(&far, r, 3);
        let n = Plane {
            width: w,
            height: h,
            data: (0..w * h)
                .map(|i| (eroded.data[i] * (1.0 - far.data[i]) + far.data[i] * num.data[i] / den.data[i].max(1e-3)).clamp(0.0, 1.0))
                .collect(),
        };
        let mut slope = Plane::new(w, h);
        for_rows(&mut slope.data, w, |y, row| {
            for (x, v) in row.iter_mut().enumerate() {
                let (xi, yi) = (x as isize, y as isize);
                let gx = (shape.get_clamped(xi + 1, yi) - shape.get_clamped(xi - 1, yi)) / 2.0;
                let gy = (shape.get_clamped(xi, yi + 1) - shape.get_clamped(xi, yi - 1)) / 2.0;
                *v = SLOPE_K * long * (gx * gx + gy * gy).sqrt();
            }
        });
        // the subject: the SUBJECT_PERCENTILE of the middle of the frame, from a histogram
        let mut bins = [0u32; 1024];
        let mut count = 0u32;
        for y in h / 5..(h * 4 / 5).max(h / 5 + 1) {
            for x in w / 5..(w * 4 / 5).max(w / 5 + 1) {
                let v = n.get_clamped(x as isize, y as isize);
                bins[((v * 1023.0).round() as usize).min(1023)] += 1;
                count += 1;
            }
        }
        let want = (count as f32 * SUBJECT_PERCENTILE) as u32;
        let (mut acc, mut subject) = (0u32, 0.5f32);
        for (i, b) in bins.iter().enumerate() {
            acc += b;
            if acc >= want {
                subject = i as f32 / 1023.0;
                break;
            }
        }
        Prepared { d, n, slope, subject }
    }

    /// The depth `depth` (stored, identified by `id`) prepared, kept for the last few.
    pub fn of(depth: &Plane, id: u64) -> Arc<Prepared> {
        if let Some(hit) = PREPARED_KEPT.lock().unwrap_or_else(PoisonError::into_inner).iter().find(|(k, _)| *k == id) {
            return hit.1.clone();
        }
        let made = Arc::new(Prepared::new(depth));
        let mut l = PREPARED_KEPT.lock().unwrap_or_else(PoisonError::into_inner);
        l.retain(|(k, _)| *k != id);
        l.insert(0, (id, made.clone()));
        l.truncate(crate::relight::KEPT);
        made
    }
}

/// `m` (sRGB-linear light colour) in the working space.
fn to_working(m: &[[f32; 3]; 3], c: V3) -> V3 {
    std::array::from_fn(|r| m[r][0] * c[0] + m[r][1] * c[1] + m[r][2] * c[2])
}

/// The flash's colour at `kelvin` against a daylight (6500 K) white balance, as an sRGB-linear gain.
pub fn flash_gain(kelvin: f32) -> V3 {
    let x = (6500.0 - kelvin) / 4000.0;
    [1.0 + 0.35 * x, 1.0, 1.0 - 0.55 * x]
}

/// The ambient's colour as an sRGB-linear gain: warmer (toward tungsten) for `tint` above 0, cooler below.
pub fn ambient_gain(tint: f32) -> V3 {
    let (warm, cool) = ((tint / 100.0).max(0.0), (-tint / 100.0).max(0.0));
    let (w, c) = ([1.12f32, 1.0, 0.65], [0.75f32, 0.95, 1.10]);
    std::array::from_fn(|k| (w[k] * warm + (1.0 - warm)) * (c[k] * cool + (1.0 - cool)))
}

/// What a render's Flash does, resolved from the settings and the depth.
#[derive(Clone, Debug)]
pub struct Setup {
    prepared: Arc<Prepared>,
    /// The direction of the flash from the lens on the stored source, in long edges (unit length).
    pub direction: [f32; 2],
    /// How far a ray travels while it rises a whole depth range, in long edges.
    pub reach_len: f32,
    pub shadow: f32,
    pub surface: f32,
    /// `1 / R`, and the nearness the flash is exposed for.
    pub inv_r: f32,
    pub subject: f32,
    pub power: f32,
    /// The ambient, linear.
    pub ambient: f32,
    pub flash_col: V3,
    pub tint: V3,
    pub hotspot: f32,
    pub gloss: f32,
    pub clip: f32,
    pub contrast: f32,
    pub emit: f32,
    pub grain: f32,
    pub fill: f32,
    pub near: f32,
    pub far: f32,
    pub ranged: bool,
    pub key: u64,
}

impl Setup {
    /// Flash for settings `s` with the scene data `mattes` and the user's orientation `orient` (the flash is placed on the
    /// oriented photo, the depth is over the stored one); `None` when it does nothing: off, its eye off, or no depth.
    pub fn new(s: &DevelopSettings, mattes: Option<&Mattes>, orient: Orientation) -> Option<Setup> {
        if !flashes(s) {
            return None;
        }
        let depth = mattes?.depth()?.clone();
        if depth.width == 0 || depth.height == 0 || depth.data.len() != depth.width * depth.height {
            return None;
        }
        let f = &s.flash;
        let id = depth_identity(depth_key(s, &depth), &depth);
        let prepared = Prepared::of(&depth, id);
        let angle = f64::from(num(f.angle, 0.0, 360.0, 35.0)).to_radians();
        // (the flash is placed on the photo as shown, the depth is over the stored one: the direction through the inverse
        // orientation, on a unit square, which is an isometry)
        let inv = orient.inverse();
        let (a, b) = (inv.map(0.5, 0.5, 1.0, 1.0), inv.map(0.5 + angle.cos(), 0.5 - angle.sin(), 1.0, 1.0));
        let (dx, dy) = ((b.0 - a.0) as f32, (b.1 - a.1) as f32);
        let norm = (dx * dx + dy * dy).sqrt().max(1e-6);
        let r = R_MIN * (R_MAX / R_MIN).powf(num(f.falloff, 0.0, 100.0, 55.0) / 100.0);
        let m = srgb_to_working();
        let (near, far) = (num(f.near, 0.0, 100.0, 0.0), num(f.far, 0.0, 100.0, 100.0));
        let setup = Setup {
            direction: [dx / norm, dy / norm],
            reach_len: (num(f.offset, 0.0, 100.0, 35.0) / 100.0 * OFFSET_MAX).max(1e-3),
            shadow: num(f.shadow, 0.0, 100.0, 50.0) / 100.0,
            surface: num(f.surface, 0.0, 100.0, 50.0) / 100.0,
            inv_r: 1.0 / r,
            subject: f.subject.map_or(prepared.subject, |v| num(v, 0.0, 1.0, 0.5)),
            power: num(f.power, 0.0, 100.0, 50.0) / 50.0,
            ambient: (-AMBIENT_STOPS * (1.0 - num(f.ambient, 0.0, 100.0, 47.0) / 100.0)).exp2(),
            flash_col: to_working(&m, flash_gain(num(f.temp, 3200.0, 10000.0, 5600.0))),
            tint: to_working(&m, ambient_gain(num(f.tint, -100.0, 100.0, 20.0))),
            hotspot: num(f.hotspot, 0.0, 100.0, 35.0) / 100.0,
            gloss: num(f.gloss, 0.0, 100.0, 10.0) / 100.0,
            clip: num(f.clip, 0.0, 100.0, 30.0) / 100.0,
            contrast: num(f.contrast, 0.0, 100.0, 25.0) / 100.0,
            emit: num(f.emit, 0.0, 100.0, 20.0) / 100.0,
            grain: num(f.grain, 0.0, 100.0, 20.0) / 100.0,
            fill: num(f.fill, 0.0, 100.0, 70.0) / 100.0,
            near,
            far,
            ranged: near > 0.0 || far < 100.0,
            key: 0,
            prepared,
        };
        let key = crate::hash_of((
            id,
            setup.direction.map(f32::to_bits),
            [setup.reach_len, setup.shadow, setup.surface, setup.inv_r, setup.subject, setup.power, setup.ambient].map(f32::to_bits),
            setup.flash_col.map(f32::to_bits),
            setup.tint.map(f32::to_bits),
            [setup.hotspot, setup.gloss, setup.clip, setup.contrast, setup.emit, setup.grain, setup.fill, setup.near, setup.far].map(f32::to_bits),
        ));
        Some(Setup { key, ..setup })
    }

    /// How much of the flash reaches the (smoothed) nearness `d` by the depth range: 1 between Near and Far, fading to 0
    /// over [`RANGE_FEATHER`] past each end.
    pub fn reach_at(&self, d: f32) -> f32 {
        if !self.ranged {
            return 1.0;
        }
        let depth = 100.0 * (1.0 - d.clamp(0.0, 1.0));
        smooth(self.near - RANGE_FEATHER, self.near, depth) * (1.0 - smooth(self.far, self.far + RANGE_FEATHER, depth))
    }

    /// The facing of a surface whose slope (per long edge, Surface 100 is 1) is `slope`: 1 when it faces the lens.
    fn facing(&self, slope: f32) -> f32 {
        let mag = self.surface * slope;
        let keep = (-(mag / SLOPE_CUT).powi(4)).exp();
        let m = mag * keep;
        1.0 / (1.0 + m * m).sqrt()
    }

    /// The light the flash's cast shadow leaves at depth pixel (`x`, `y`), 1 when nothing stands between the point and the
    /// flash: a ray marched toward it through the height field (the nearness), `steps` steps.
    fn unshadowed(&self, n: &Plane, x: usize, y: usize, steps: usize) -> f32 {
        if self.shadow <= 0.0 {
            return 1.0;
        }
        let long = n.width.max(n.height).max(1) as f32;
        let s_px = self.reach_len * long;
        let step_px = (2.0 * long / 960.0).max(s_px / 14.0);
        let here = n.get_clamped(x as isize, y as isize);
        // (what it falls on has to be lit by the flash at all: none far behind the subject; decided here, at the depth's size, so the edge of the subject's soft
        // nearness is not a rim of shadow)
        let inv_s = self.inv_r + self.subject * (1.0 - self.inv_r);
        let e = ((self.inv_r + here.clamp(0.0, 1.0) * (1.0 - self.inv_r)) / inv_s.max(1e-6)).powf(EXPONENT).min(E_CAP);
        let visible = ((e - VISIBLE_FROM) / VISIBLE_SPAN).clamp(0.0, 1.0);
        if visible <= 0.0 {
            return 1.0;
        }
        let mut occ = 0.0f32;
        for i in 1..=steps.max(1) {
            // (the full ray is 28 steps; a draft takes every other one, twice as long, to reach as far)
            let t = i as f32 * step_px * (SHADOW_STEPS as f32 / steps.max(1) as f32);
            let q = sample(n, x as f32 + 0.5 + self.direction[0] * t, y as f32 + 0.5 + self.direction[1] * t);
            let over = q - (here + t / s_px);
            let gap = ((q - here - GAP_FROM) / GAP_SPAN).clamp(0.0, 1.0);
            occ = occ.max((over * SHADOW_GAIN).clamp(0.0, 1.0) * gap);
        }
        1.0 - occ * visible
    }

    /// The planes at the depth map's size: the nearness, the light the shadow leaves, the facing and, when the depth range
    /// leaves part of the scene out, how much of the flash reaches each depth; what [`apply`] samples through the frame.
    pub fn planes(&self, steps: usize) -> [Option<Plane>; 4] {
        let p = &self.prepared;
        let (w, h) = (p.n.width, p.n.height);
        let mut lit = Plane::new(w, h);
        for_rows(&mut lit.data, w, |y, row| {
            for (x, v) in row.iter_mut().enumerate() {
                *v = self.unshadowed(&p.n, x, y, steps);
            }
        });
        let facing = Plane { width: w, height: h, data: p.slope.data.iter().map(|s| self.facing(*s)).collect() };
        let reach = self.ranged.then(|| Plane { width: w, height: h, data: p.d.data.iter().map(|d| self.reach_at(*d)).collect() });
        [Some(p.n.clone()), Some(lit), Some(facing), reach]
    }
}

/// A uniform-ish number in -sqrt(3)..sqrt(3) with unit variance, from the grain cell (`x`, `y`) (sum of four bytes of a hash).
pub fn grain_noise(x: u32, y: u32) -> f32 {
    let mut h = x.wrapping_mul(0x9E37_79B1) ^ y.wrapping_mul(0x85EB_CA77);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^= h >> 15;
    let sum = (h & 255) + ((h >> 8) & 255) + ((h >> 16) & 255) + (h >> 24);
    (sum as f32 / 255.0 - 2.0) * 1.732_050_8
}

/// The noise at grain cell (`x`, `y`) per channel: a luminance part every channel shares and a colour part of its own (`CHROMA`), as a sensor's
/// noise in the dark is, of unit variance in all.
pub fn grain_noise3(x: u32, y: u32) -> V3 {
    let luma = grain_noise(x, y);
    std::array::from_fn(|k| {
        let m = k as u32 + 1;
        0.75 * luma + 0.5 * grain_noise(x.wrapping_add(7919u32.wrapping_mul(m)), y.wrapping_add(104_729u32.wrapping_mul(m)))
    })
}

/// sRGB's transfer function, on any channel (the flash's contrast and noise work on the display encoding).
fn encode(x: f32) -> f32 {
    let x = x.max(0.0);
    if x <= 0.003_130_8 { x * 12.92 } else { 1.055 * x.powf(1.0 / 2.4) - 0.055 }
}

fn decode(x: f32) -> f32 {
    if x <= 0.040_45 { x / 12.92 } else { ((x + 0.055) / 1.055).powf(2.4) }
}

/// `v` if it is a number, else `default`.
fn fin(v: f32, default: f32) -> f32 {
    if v.is_finite() { v } else { default }
}

/// One pixel: `c` (linear Rec. 2020) flashed. `n`, `lit0`, `nz` are the planes at the pixel, `reach` the depth range's share
/// (what is out of the range keeps the photo's own light); `r2` the squared distance from the frame's centre over the half
/// diagonal's; `z` the grain's noise there ([`grain_noise3`]); `mean_log` and `big_log` the log2 luminance of the pixel's near and wide surround.
#[allow(clippy::too_many_arguments)]
pub fn pixel(setup: &Setup, c: V3, n: f32, lit0: f32, nz: f32, reach: f32, r2: f32, z: V3, mean_log: f32, big_log: f32) -> V3 {
    let s = setup;
    let c: V3 = c.map(|v| fin(v, 0.0).clamp(0.0, 1.0e4));
    let (n, lit0, nz, reach) =
        (fin(n, 0.0).clamp(0.0, 1.0), fin(lit0, 1.0).clamp(0.0, 1.0), fin(nz, 1.0).clamp(0.0, 1.0), fin(reach, 1.0).clamp(0.0, 1.0));
    let (r2, z) = (fin(r2, 0.0).max(0.0), z.map(|v| fin(v, 0.0)));
    let inv_s = s.inv_r + s.subject * (1.0 - s.inv_r);
    let inv = s.inv_r + n * (1.0 - s.inv_r);
    let e = (inv / inv_s.max(1e-6)).powf(EXPONENT).min(E_CAP);
    let lit = 1.0 - s.shadow * (1.0 - lit0);
    // far from the lens the surface hardly matters, and its slope is noise
    let nz2 = 1.0 - (1.0 - nz) * e.clamp(0.0, 1.0);
    let lam = (1.0 - WRAP) * nz2 + WRAP;
    let hot = (-s.hotspot * HOT_K * r2).exp();
    let fl = s.power * e * lam * hot * lit;
    let lum = c[0] * LUMA[0] + c[1] * LUMA[1] + c[2] * LUMA[2];
    // the shade of the photo's own light on what the flash lights is lifted toward what is around it (Fill Shadows)
    let shade = (fin(big_log, -20.0) - lum.max(1e-4).log2()).clamp(0.0, FILL_MAX_STOPS);
    let lift = (s.fill * FILL_K * shade * ((e - FILL_FROM) / FILL_SPAN).clamp(0.0, 1.0)).exp2();
    // lamps and windows light themselves: a photo clips them, so nearly clipped pixels keep more of their light
    let g = ((lum - 0.30) / 0.55).clamp(0.0, 1.0);
    let a_eff = s.ambient + (1.0 - s.ambient) * (g * g * (3.0 - 2.0 * g) * s.emit);
    let mut out: V3 = std::array::from_fn(|k| c[k] * lift * (a_eff * s.tint[k] + fl * s.flash_col[k]) / (1.0 + a_eff));
    if s.gloss > 0.0 {
        let near_only = ((e - 0.55) / 0.6).clamp(0.0, 1.0);
        let facing = ((nz2 - GLINT_FROM) / GLINT_SPAN).clamp(0.0, 1.0);
        let peak = ((lum.max(1e-6).log2() - fin(mean_log, -20.0)) * PEAK_K).clamp(0.0, 1.0);
        let gate = ((lum - 0.04) * 6.0).clamp(0.0, 1.0);
        let glint = s.gloss * GLOSS_K * facing * facing * (PEAK_FLOOR + (1.0 - PEAK_FLOOR) * peak) * gate * e.clamp(0.0, 3.0) * near_only * hot * lit;
        for k in 0..3 {
            out[k] += glint * s.flash_col[k];
        }
    }
    let gain = EXPOSURE_STOPS.exp2();
    let mut d: V3 = out.map(|o| {
        let o = (o * gain).clamp(0.0, 1.0e4);
        let soft = if o < 0.6 { o } else { 0.6 + (o - 0.6) / (1.0 + (o - 0.6) * 1.2) };
        let o = s.clip * o.min(1.0) + (1.0 - s.clip) * soft;
        let e = encode(o).min(1.0);
        (1.0 - s.contrast) * e + s.contrast * e * e * (3.0 - 2.0 * e)
    });
    if s.grain > 0.0 {
        let mean = ((d[0] + d[1] + d[2]) / 3.0).clamp(0.0, 1.0);
        let amp = s.grain * GRAIN_K * (1.0 - (e / 0.8).clamp(0.0, 1.0)).powf(1.3) * (0.35 + 0.65 * mean.sqrt());
        d = std::array::from_fn(|k| (d[k] + amp * z[k]).clamp(0.0, 1.0));
    }
    // (what the depth range leaves out keeps the photo's own light)
    std::array::from_fn(|k| c[k] + (decode(d[k]) - c[k]) * reach)
}

/// Flash `img` (scene-linear, the output of `frame`) as `setup` says; `origin` and `full`: where `img` sits in the whole
/// output (a window's corner and the whole's size), `draft`: shadow rays of fewer steps.
pub fn apply(img: &mut Rgb32f, setup: &Setup, frame: &Frame, origin: [usize; 2], full: [usize; 2], draft: bool) {
    let (w, h) = (img.width, img.height);
    if w == 0 || h == 0 {
        return;
    }
    let [n, lit, nz, reach] = setup.planes(if draft { SHADOW_STEPS_DRAFT } else { SHADOW_STEPS });
    let guide = crate::lensblur::log_luma(img);
    // the surround's log luminance: what a glint stands out of
    let long = full[0].max(full[1]) as f32;
    let surround = blur_n(&guide, ((SURROUND * long).round() as usize).max(2), 2);
    let wide = blur_n(&guide, ((FILL_RADIUS * long).round() as usize).max(2), 2);
    let snap = |p: Option<Plane>| p.map(|p| crate::lensblur::nearness_out(&p, frame, w, h, &guide));
    let (n, lit, nz, reach) = (snap(n), snap(lit), snap(nz), snap(reach));
    let (fw, fh) = (full[0].max(1) as f32, full[1].max(1) as f32);
    let half_diag = (fw / 2.0).powi(2) + (fh / 2.0).powi(2);
    let cell = (fw.max(fh) / GRAIN_CELLS).round().max(1.0);
    for_rows(&mut img.data, w, |y, row| {
        for (x, c) in row.iter_mut().enumerate() {
            let i = y * w + x;
            let at = |p: &Option<Plane>, default: f32| p.as_ref().and_then(|p| p.data.get(i).copied()).unwrap_or(default);
            let (px, py) = ((origin[0] + x) as f32 + 0.5, (origin[1] + y) as f32 + 0.5);
            let r2 = ((px - fw / 2.0).powi(2) + (py - fh / 2.0).powi(2)) / half_diag;
            let z = if setup.grain > 0.0 { grain_noise3((px / cell) as u32, (py / cell) as u32) } else { [0.0; 3] };
            *c = pixel(
                setup,
                *c,
                at(&n, 0.0),
                at(&lit, 1.0),
                at(&nz, 1.0),
                at(&reach, 1.0),
                r2,
                z,
                surround.data.get(i).copied().unwrap_or(-20.0),
                wide.data.get(i).copied().unwrap_or(-20.0),
            );
        }
    });
}

#[cfg(test)]
mod tests;
