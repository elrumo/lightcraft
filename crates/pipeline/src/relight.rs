//! **Relight** and **Depth Map**: two tools that read the photo's depth (its own depth map, or the
//! estimate Lens Blur stored in the settings, [`crate::lensblur::mattes_for`]) as a surface.
//!
//! **Relight** adds a point light to the scene. The depth is a height field over the EXIF-oriented
//! source, in units of the depth map's long edge: across, down, and toward the camera
//! ([`SCENE_DEPTH`] from the farthest part of the photo to its nearest). Each of its pixels gets a
//! normal: the surface model's where the settings keep one (`relight.normals`, MoGe-2: the shape of a
//! face, a floor seen from above), else from the slope of a smoother copy of the depth
//! ([`normal_sigma`]); either is tilted more or less by Surface. And from the light: a wrapped Lambert
//! term (Softness wraps the light around shapes), an inverse-square falloff whose reach Spread
//! sets, a soft shadow from a ray marched through the height field toward the light (Shadows, with
//! a penumbra Softness widens), and a Blinn-Phong highlight (Shine). Those two planes, the light
//! and the highlight, are made at the depth map's own size (a pixel of the depth is the finest
//! detail the light can have) and sampled through the frame like the picture, their edges snapped
//! to the photo's by the guided filter Lens Blur snaps its depth with ([`crate::lensblur::nearness_out`]:
//! a light that stops at a face stops at its outline, not a few depth pixels off it); every output pixel is
//! then `colour × (Ambient + Brightness × light colour × light) + Shine × light colour × highlight`
//! in scene-linear Rec. 2020, after noise reduction and before Lens Blur, so the blur sees the new
//! light and every tone and colour tool works on it.
//!
//! **Depth Map** draws the depth itself in place of the photo: each output pixel's depth (0 the
//! nearest, 100 the farthest, as Lens Blur's) is placed between the Near and Far ends, inverted if
//! asked, bent by Curve, and looked up in the palette (evenly spaced stops, far to near, display
//! sRGB); Contours darkens thin lines along equal depths (their width a fraction of the long edge,
//! found from the depth's slope at each pixel); Amount mixes the result over the photo. It
//! runs before Relight (a lit depth map is a shaded relief) and everything after it edits it as a
//! photo.
//!
//! Both read the depth smoothed by a small Gaussian ([`smoothed`]): a stored depth is 8-bit, and
//! its steps would show as terraces in the light and bands in the map. The native Apple renderer
//! implements the same formulas (`RelightKernel`, `DepthLookKernel` in the app), so every constant
//! here is public.

use std::sync::{Arc, Mutex, PoisonError};

use lightcraft_develop::{DepthPalette, DevelopSettings};
use lightcraft_geom::Orientation;
use lightcraft_raster::{Image, Plane, Rgb32f};

use crate::for_rows;
use crate::geometry::Frame;
use crate::masks::{Mattes, source_to_out};

/// Sigma (pixels of the depth map) of the Gaussian that smooths the depth before it is read.
pub const SMOOTH_SIGMA: f32 = 1.5;
/// … and of the one the surface's normals are taken from, as a fraction of the depth map's long side (at least
/// [`SMOOTH_SIGMA`]): wider, so the steps of an 8-bit depth on a gentle slope (a floor, a wall) don't show as ripples
/// in the light, while a face or a body keeps its shape. About 4 pixels of a Depth Anything estimate.
pub const NORMAL_SCALE: f32 = 0.004;

/// The sigma (pixels) of the normals' Gaussian for a depth map whose long side is `long` pixels.
pub fn normal_sigma(long: usize) -> f32 {
    (NORMAL_SCALE * long as f32).max(SMOOTH_SIGMA)
}
/// How deep the scene is, farthest to nearest, in units of the depth map's long edge.
pub const SCENE_DEPTH: f32 = 0.5;
/// The light's gain at Brightness 100 on a surface that faces it, close by.
pub const BRIGHTNESS_MAX: f32 = 4.0;
/// The highlight's light (scene-linear) at Shine and Brightness 100.
pub const SHINE_MAX: f32 = 0.25;
/// The highlight's exponent (Blinn-Phong).
pub const SHININESS: f32 = 32.0;
/// How far the light wraps past a surface's edge (in `N·L`) at Softness 100.
pub const WRAP_MAX: f32 = 0.75;
/// The light's reach (long edges: where its light has halved) at Spread 0 and 100, exponential between.
pub const REACH_MIN: f32 = 0.15;
pub const REACH_MAX: f32 = 3.0;
/// Steps of a shadow ray, full quality and draft.
pub const SHADOW_STEPS: usize = 32;
pub const SHADOW_STEPS_DRAFT: usize = 16;
/// The shadow's edge: sharp at Softness 0 (this factor on the ray's clearance over its length), soft at 100.
pub const PENUMBRA_HARD: f32 = 12.0;
pub const PENUMBRA_SOFT: f32 = 1.5;
/// How far in front of the nearest part of the scene the light is at Distance 100, in scene depths ([`SCENE_DEPTH`]):
/// Distance 0–50 moves it from the back of the scene to its nearest part, 50–100 out in front of it, quadratically.
pub const FRONT_MAX: f32 = 4.0;
/// How far above the surface a shadow ray is held (in nearness, two steps of the 8-bit depth): a surface
/// doesn't shadow itself.
pub const RAY_BIAS: f32 = 2.0 / 255.0;
/// How dark a contour line is.
pub const CONTOUR_DARK: f32 = 0.8;
/// Half the width of a contour line, as a fraction of the long edge. A line thinner than a pixel is drawn a pixel
/// wide and fainter by as much, so its ink is the same at every output size.
pub const CONTOUR_HALF_WIDTH: f32 = 0.0008;
/// Contour lines across the whole depth range at Contours 0+ and 100.
pub const CONTOURS_MIN: f32 = 2.0;
pub const CONTOURS_MAX: f32 = 30.0;

/// Whether a render of `s` relights it (on, its section's eye on, with something to light).
pub fn relights(s: &DevelopSettings) -> bool {
    s.relight.active() && s.section_enabled("relight")
}

/// Whether a render of `s` draws its depth.
pub fn draws_depth(s: &DevelopSettings) -> bool {
    s.depth_map.active() && s.section_enabled("depthMap")
}

/// A number from the settings, held to `lo..=hi` (`default` when it isn't one).
fn num(v: f64, lo: f64, hi: f64, default: f64) -> f32 {
    (if v.is_finite() { v.clamp(lo, hi) } else { default }) as f32
}

/// Half of a normalized Gaussian of `sigma` pixels: the centre's weight first, then the weight at ±1, ±2, …
/// out to three sigmas.
pub fn gaussian_weights(sigma: f32) -> Vec<f32> {
    let r = (3.0 * sigma).ceil().max(1.0) as usize;
    let w: Vec<f32> = (0..=r).map(|k| (-((k * k) as f32) / (2.0 * sigma * sigma)).exp()).collect();
    let sum = w.iter().skip(1).fold(w.first().copied().unwrap_or(1.0), |a, v| a + 2.0 * v);
    w.into_iter().map(|v| v / sum).collect()
}

/// `depth` blurred by a Gaussian of [`SMOOTH_SIGMA`] pixels.
pub fn smoothed(depth: &Plane) -> Plane {
    smoothed_by(depth, SMOOTH_SIGMA)
}

/// `depth` blurred by a Gaussian of `sigma` pixels, across then down, its edges extended.
pub fn smoothed_by(depth: &Plane, sigma: f32) -> Plane {
    let (w, h) = (depth.width, depth.height);
    if w == 0 || h == 0 || depth.data.len() != w * h {
        return depth.clone();
    }
    let weights = gaussian_weights(sigma);
    let pass = |src: &Plane, across: bool| {
        let mut out = Plane::new(w, h);
        for_rows(&mut out.data, w, |y, row| {
            for (x, v) in row.iter_mut().enumerate() {
                let (x, y) = (x as isize, y as isize);
                let at = |k: isize| if across { src.get_clamped(x + k, y) } else { src.get_clamped(x, y + k) };
                let mut acc = 0.0;
                for (k, wt) in weights.iter().enumerate() {
                    let k = k as isize;
                    acc += if k == 0 { wt * at(0) } else { wt * (at(-k) + at(k)) };
                }
                *v = acc;
            }
        });
        out
    };
    pass(&pass(depth, true), false)
}

/// Bilinear sample of `p` at pixel coordinates (centres at +0.5), its edges extended.
#[inline]
pub fn sample(p: &Plane, x: f32, y: f32) -> f32 {
    let (fx, fy) = (x.clamp(-1.0, p.width as f32 + 1.0) - 0.5, y.clamp(-1.0, p.height as f32 + 1.0) - 0.5);
    let (x0, y0) = (fx.floor(), fy.floor());
    let (tx, ty) = (fx - x0, fy - y0);
    let (x0, y0) = (x0 as isize, y0 as isize);
    let a = p.get_clamped(x0, y0) + (p.get_clamped(x0 + 1, y0) - p.get_clamped(x0, y0)) * tx;
    let b = p.get_clamped(x0, y0 + 1) + (p.get_clamped(x0 + 1, y0 + 1) - p.get_clamped(x0, y0 + 1)) * tx;
    a + (b - a) * ty
}

/// Linear sRGB to linear Rec. 2020 (the pipeline's working space).
fn srgb_to_working() -> [[f32; 3]; 3] {
    lightcraft_color::SRGB.to_space(&lightcraft_color::REC2020).to_f32()
}

/// Display-encoded sRGB (0..1) to the working space by the matrix `m` ([`srgb_to_working`]).
fn working(m: &[[f32; 3]; 3], c: [f32; 3]) -> [f32; 3] {
    let l = c.map(|v| lightcraft_color::transfer::srgb_to_linear(if v.is_finite() { v.clamp(0.0, 1.0) } else { 1.0 }));
    std::array::from_fn(|r| m[r][0] * l[0] + m[r][1] * l[1] + m[r][2] * l[2])
}

/// The light's height over the back of the scene for Distance `d` (0–100), in scene depths: 0 at the back, 1 at the
/// nearest part (Distance 50), `1 + FRONT_MAX` at Distance 100.
pub fn light_height(d: f32) -> f32 {
    if d <= 50.0 {
        d / 50.0
    } else {
        let f = (d - 50.0) / 50.0;
        1.0 + FRONT_MAX * f * f
    }
}

fn depth_key(s: &DevelopSettings, depth: &Arc<Plane>) -> u64 {
    match &s.lens_blur.depth {
        Some(d) => crate::hash_of(&d.data),
        None => crate::hash_of(Arc::as_ptr(depth) as usize),
    }
}

type V3 = [f32; 3];

#[inline]
fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[inline]
fn normalized(a: V3) -> V3 {
    let l = dot(a, a).sqrt();
    if l > 1e-12 { a.map(|v| v / l) } else { [0.0, 0.0, 1.0] }
}

/// How many decoded normals and smoothed depths are kept ([`Normals::of`], [`Setup::planes`]): a few photos' worth.
pub const KEPT: usize = 3;

static NORMALS_KEPT: Mutex<Vec<(u64, Arc<Normals>)>> = Mutex::new(Vec::new());
/// The depth smoothed for the surface and the smoother copy the normals come from, per depth.
static SMOOTHED_KEPT: Mutex<Vec<(u64, Arc<(Plane, Plane)>)>> = Mutex::new(Vec::new());

/// `value` kept under `key` in `list`, the newest first, at most [`KEPT`].
fn keep<T>(list: &Mutex<Vec<(u64, Arc<T>)>>, key: u64, value: Arc<T>) {
    let mut l = list.lock().unwrap_or_else(PoisonError::into_inner);
    l.retain(|(k, _)| *k != key);
    l.insert(0, (key, value));
    l.truncate(KEPT);
}

/// A depth's identity for [`SMOOTHED_KEPT`]: where it comes from (`depth_key`), its size, and a sample of its values
/// (a photo's own depth is known by its address, which a later depth may reuse).
fn depth_identity(source: u64, depth: &Plane) -> u64 {
    let n = depth.data.len();
    let step = (n / 256).max(1);
    let sample: Vec<u32> = depth.data.iter().step_by(step).map(|v| v.to_bits()).collect();
    crate::hash_of((source, depth.width, depth.height, sample))
}

/// Unit normals over the stored source (`relight.normals` decoded), row-major.
#[derive(Clone, Debug)]
pub struct Normals {
    pub width: usize,
    pub height: usize,
    pub data: Vec<V3>,
}

impl Normals {
    /// The settings' normals, decoded; `None` without them or when they are damaged. The last few decoded are kept
    /// ([`KEPT`]): every draft of a drag reads the same ones.
    pub fn of(s: &DevelopSettings) -> Option<Arc<Normals>> {
        let m = s.relight.normals.as_ref()?;
        let key = crate::hash_of((&m.data, m.w, m.h));
        if let Some(hit) = NORMALS_KEPT.lock().unwrap_or_else(PoisonError::into_inner).iter().find(|(k, _)| *k == key) {
            return Some(hit.1.clone());
        }
        let data = m.normals()?;
        let made = Arc::new(Normals { width: m.w as usize, height: m.h as usize, data });
        keep(&NORMALS_KEPT, key, made.clone());
        Some(made)
    }

    /// The normal at normalized point (`u`, `v`) of the source (0..1 across and down), bilinear between pixel
    /// centres, edges extended, made one long again.
    pub fn at(&self, u: f32, v: f32) -> V3 {
        let (w, h) = (self.width, self.height);
        if w == 0 || h == 0 || self.data.len() != w * h {
            return [0.0, 0.0, 1.0];
        }
        let fx = (u * w as f32 - 0.5).clamp(0.0, (w - 1) as f32);
        let fy = (v * h as f32 - 0.5).clamp(0.0, (h - 1) as f32);
        let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let p = |x: usize, y: usize| self.data[y * w + x];
        let (a, b, c, d) = (p(x0, y0), p(x1, y0), p(x0, y1), p(x1, y1));
        normalized(std::array::from_fn(|k| (a[k] + (b[k] - a[k]) * tx) * (1.0 - ty) + (c[k] + (d[k] - c[k]) * tx) * ty))
    }
}

/// What a render's Relight does, resolved from the settings and the depth.
#[derive(Clone, Debug)]
pub struct Setup {
    depth: Arc<Plane>,
    /// The depth's identity for the kept smoothed copies ([`depth_identity`]).
    depth_id: u64,
    /// The surface model's normals (x right, y down, z toward the camera), when the settings keep them.
    normals: Option<Arc<Normals>>,
    /// The light in units of the depth map's long edge over the stored (EXIF-oriented) source: across, down,
    /// and toward the camera from the back of the scene.
    pub light: V3,
    /// How strongly the depth's slope turns the surface (0 flat, 1 as the depth says, 2 twice that).
    pub surface: f32,
    pub wrap: f32,
    /// Where the light has halved (long edges).
    pub reach: f32,
    /// 0..1: how much a cast shadow takes away.
    pub shadows: f32,
    pub penumbra: f32,
    /// What each output pixel keeps of its own light, and what the light adds (times its colour).
    pub ambient: f32,
    pub gain: V3,
    /// The highlight's light (working space).
    pub shine: V3,
    /// Stage-cache key of all of the above.
    pub key: u64,
}

impl Setup {
    /// Relight for settings `s` with the scene data `mattes` and the user's orientation `orient` (the light
    /// is placed on the oriented photo, the depth is over the stored one); `None` when it lights nothing:
    /// off, its eye off, or no depth.
    pub fn new(s: &DevelopSettings, mattes: Option<&Mattes>, orient: Orientation) -> Option<Setup> {
        if !relights(s) {
            return None;
        }
        let depth = mattes?.depth()?.clone();
        let (w, h) = (depth.width, depth.height);
        if w == 0 || h == 0 || depth.data.len() != w * h {
            return None;
        }
        let r = &s.relight;
        let long = w.max(h) as f32;
        let (x, y) = (num(r.x, -0.5, 1.5, 0.3), num(r.y, -0.5, 1.5, 0.25));
        let (u, v) = orient.inverse().map(f64::from(x), f64::from(y), 1.0, 1.0);
        let light = [u as f32 * w as f32 / long, v as f32 * h as f32 / long, SCENE_DEPTH * light_height(num(r.distance, 0.0, 100.0, 70.0))];
        let softness = num(r.softness, 0.0, 100.0, 30.0) / 100.0;
        let reach = REACH_MIN * (REACH_MAX / REACH_MIN).powf(num(r.spread, 0.0, 100.0, 50.0) / 100.0);
        let colour = working(&srgb_to_working(), r.color.map(|c| c as f32));
        let top = colour[0].max(colour[1]).max(colour[2]);
        let colour = if top > 1e-6 { colour.map(|c| c / top) } else { [0.0; 3] };
        let brightness = num(r.brightness, 0.0, 100.0, 50.0) / 100.0 * BRIGHTNESS_MAX;
        let shine = num(r.shine, 0.0, 100.0, 0.0) / 100.0 * SHINE_MAX * brightness;
        let setup = Setup {
            surface: num(r.surface, 0.0, 100.0, 50.0) / 50.0,
            wrap: softness * WRAP_MAX,
            reach,
            shadows: num(r.shadows, 0.0, 100.0, 60.0) / 100.0,
            penumbra: PENUMBRA_HARD + (PENUMBRA_SOFT - PENUMBRA_HARD) * softness,
            ambient: num(r.ambient, 0.0, 100.0, 60.0) / 100.0,
            gain: colour.map(|c| c * brightness),
            shine: colour.map(|c| c * shine),
            light,
            key: 0,
            depth_id: depth_identity(depth_key(s, &depth), &depth),
            depth,
            normals: Normals::of(s),
        };
        let key = crate::hash_of((
            setup.depth_id,
            r.normals.as_ref().map(|n| crate::hash_of(&n.data)),
            w,
            h,
            setup.light.map(f32::to_bits),
            [setup.surface, setup.wrap, setup.reach, setup.shadows, setup.penumbra, setup.ambient].map(f32::to_bits),
            setup.gain.map(f32::to_bits),
            setup.shine.map(f32::to_bits),
        ));
        Some(Setup { key, ..setup })
    }

    /// The depth smoothed for the surface ([`smoothed`]) and the smoother copy the normals come from ([`normal_sigma`]),
    /// kept for the last few depths: a drag of the light redraws with the same depth.
    fn smoothed(&self) -> Arc<(Plane, Plane)> {
        if let Some(hit) = SMOOTHED_KEPT.lock().unwrap_or_else(PoisonError::into_inner).iter().find(|(k, _)| *k == self.depth_id) {
            return hit.1.clone();
        }
        let made = Arc::new((smoothed(&self.depth), smoothed_by(&self.depth, normal_sigma(self.depth.width.max(self.depth.height)))));
        keep(&SMOOTHED_KEPT, self.depth_id, made.clone());
        made
    }

    /// Whether the surface model's normals shade the photo (else the depth's slope).
    pub fn has_normals(&self) -> bool {
        self.normals.is_some()
    }

    /// The surface's normal at pixel (`i`, `j`) of the smoothed depth `n`: the surface model's (sampled at the pixel's
    /// centre) when the settings keep it, else from the slope of the smoother depth `m` (the same size); tilted by Surface.
    pub fn normal(&self, n: &Plane, m: &Plane, i: usize, j: usize) -> V3 {
        if let Some(model) = &self.normals {
            let (u, v) = ((i as f32 + 0.5) / n.width.max(1) as f32, (j as f32 + 0.5) / n.height.max(1) as f32);
            let c = model.at(u, v);
            return normalized([self.surface * c[0], self.surface * c[1], c[2]]);
        }
        let px = 1.0 / n.width.max(n.height).max(1) as f32;
        let (xi, yi) = (i as isize, j as isize);
        let gx = (m.get_clamped(xi + 1, yi) - m.get_clamped(xi - 1, yi)) / (2.0 * px);
        let gy = (m.get_clamped(xi, yi + 1) - m.get_clamped(xi, yi - 1)) / (2.0 * px);
        let k = self.surface * SCENE_DEPTH;
        normalized([-k * gx, -k * gy, 1.0])
    }

    /// The light and the highlight falling on pixel (`i`, `j`) of the smoothed depth `n` (its normal from
    /// [`Setup::normal`]), with shadow rays of `steps` steps.
    pub fn shade(&self, n: &Plane, m: &Plane, i: usize, j: usize, steps: usize) -> (f32, f32) {
        let long = n.width.max(n.height).max(1) as f32;
        let px = 1.0 / long;
        let (xi, yi) = (i as isize, j as isize);
        let normal = self.normal(n, m, i, j);
        let p = [(i as f32 + 0.5) * px, (j as f32 + 0.5) * px, SCENE_DEPTH * n.get_clamped(xi, yi)];
        let to = [self.light[0] - p[0], self.light[1] - p[1], self.light[2] - p[2]];
        let dist = dot(to, to).sqrt().max(1e-6);
        let dir = to.map(|v| v / dist);
        let ndl = dot(normal, dir);
        let diffuse = ((ndl + self.wrap) / (1.0 + self.wrap)).max(0.0);
        let q = dist / self.reach;
        let fall = 1.0 / (1.0 + q * q);
        let lit = if self.shadows > 0.0 { 1.0 - self.shadows * (1.0 - self.clearance(n, p, to, dist, steps)) } else { 1.0 };
        let light = diffuse * fall * lit;
        let highlight = if self.shine != [0.0; 3] && ndl > 0.0 {
            let half = normalized([dir[0], dir[1], dir[2] + 1.0]);
            dot(normal, half).max(0.0).powf(SHININESS) * fall * lit
        } else {
            0.0
        };
        (light, highlight)
    }

    /// How much of the light reaches surface point `p` past the rest of the scene (1 all of it, 0 none): a
    /// ray marched from `p` along `to` (to the light, `dist` long) through the height field `n`, the least
    /// clearance over its length scaled by the penumbra; it stops where it leaves the depth map.
    fn clearance(&self, n: &Plane, p: V3, to: V3, dist: f32, steps: usize) -> f32 {
        let long = n.width.max(n.height).max(1) as f32;
        let (mw, mh) = (n.width as f32 / long, n.height as f32 / long);
        let across = (to[0] * to[0] + to[1] * to[1]).sqrt() * long;
        // (a light straight above: nothing stands between)
        if across < 1.0 {
            return 1.0;
        }
        let t0 = 1.5 / across;
        if t0 >= 1.0 {
            return 1.0;
        }
        let mut clear = 1.0f32;
        for k in 1..=steps.max(1) {
            let f = k as f32 / steps.max(1) as f32;
            let t = t0 + (1.0 - t0) * f * f;
            let q = [p[0] + to[0] * t, p[1] + to[1] * t, p[2] + to[2] * t];
            if q[0] < 0.0 || q[1] < 0.0 || q[0] > mw || q[1] > mh {
                break;
            }
            let ground = SCENE_DEPTH * (sample(n, q[0] * long, q[1] * long) + RAY_BIAS);
            clear = clear.min((self.penumbra * (q[2] - ground) / (t * dist)).clamp(0.0, 1.0));
            if clear <= 0.0 {
                break;
            }
        }
        clear
    }

    /// The light and the highlight over the whole depth map (smoothed): what [`apply`] samples through the frame.
    pub fn planes(&self, steps: usize) -> (Plane, Option<Plane>) {
        let smooth = self.smoothed();
        let (n, m) = (&smooth.0, &smooth.1);
        let (w, h) = (n.width, n.height);
        let mut both: Image<[f32; 2]> = Image::new(w, h);
        for_rows(&mut both.data, w, |y, row| {
            for (x, v) in row.iter_mut().enumerate() {
                let (l, s) = self.shade(n, m, x, y, steps);
                *v = [l, s];
            }
        });
        let light = Plane { width: w, height: h, data: both.data.iter().map(|v| v[0]).collect() };
        let highlight = (self.shine != [0.0; 3]).then(|| Plane { width: w, height: h, data: both.data.iter().map(|v| v[1]).collect() });
        (light, highlight)
    }
}

/// Relight `img` (scene-linear, the output of `frame`) as `setup` says; `draft`: shadow rays of fewer steps.
pub fn apply(img: &mut Rgb32f, setup: &Setup, frame: &Frame, draft: bool) {
    let (w, h) = (img.width, img.height);
    if w == 0 || h == 0 {
        return;
    }
    let (light, highlight) = setup.planes(if draft { SHADOW_STEPS_DRAFT } else { SHADOW_STEPS });
    // (both 0..1, as the guided filter keeps them)
    let guide = crate::lensblur::log_luma(img);
    let light = crate::lensblur::nearness_out(&light, frame, w, h, &guide);
    let highlight = highlight.map(|p| crate::lensblur::nearness_out(&p, frame, w, h, &guide));
    let (a, g, sh) = (setup.ambient, setup.gain, setup.shine);
    for_rows(&mut img.data, w, |y, row| {
        for (x, c) in row.iter_mut().enumerate() {
            let i = y * w + x;
            let l = light.data.get(i).copied().unwrap_or(0.0);
            let s = highlight.as_ref().and_then(|p| p.data.get(i).copied()).unwrap_or(0.0);
            *c = std::array::from_fn(|k| c[k] * (a + g[k] * l) + sh[k] * s);
        }
    });
}

/// What a render's Depth Map draws, resolved from the settings and the depth.
#[derive(Clone, Debug)]
pub struct Look {
    depth: Arc<Plane>,
    /// The palette's stops, far to near, display-encoded sRGB.
    pub stops: Vec<[f32; 3]>,
    /// The depths (0 nearest, 100 farthest) at the palette's near and far ends.
    pub near: f32,
    pub far: f32,
    pub invert: bool,
    /// The exponent Curve raises the palette position to.
    pub gamma: f32,
    /// Contour lines over the whole depth range (0: none).
    pub bands: f32,
    /// 0..1: how much of the photo the depth replaces.
    pub amount: f32,
    pub key: u64,
}

impl Look {
    /// Depth Map for settings `s` with the scene data `mattes`; `None` when it draws nothing: off, its eye
    /// off, Amount 0, or no depth.
    pub fn new(s: &DevelopSettings, mattes: Option<&Mattes>) -> Option<Look> {
        if !draws_depth(s) {
            return None;
        }
        let depth = mattes?.depth()?.clone();
        if depth.width == 0 || depth.height == 0 || depth.data.len() != depth.width * depth.height {
            return None;
        }
        let d = &s.depth_map;
        let stops = match d.palette {
            DepthPalette::Custom => vec![d.far_color.map(|c| c as f32), d.near_color.map(|c| c as f32)],
            p => p.stops().to_vec(),
        };
        let stops: Vec<[f32; 3]> = stops.into_iter().map(|c| c.map(|v| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 })).collect();
        let contours = num(d.contours, 0.0, 100.0, 0.0);
        let look = Look {
            stops,
            near: num(d.near, 0.0, 100.0, 0.0),
            far: num(d.far, 0.0, 100.0, 100.0),
            invert: d.invert,
            gamma: (-num(d.curve, -100.0, 100.0, 0.0) / 50.0).exp2(),
            bands: if contours > 0.0 { CONTOURS_MIN + (CONTOURS_MAX - CONTOURS_MIN) * contours / 100.0 } else { 0.0 },
            amount: num(d.amount, 0.0, 100.0, 100.0) / 100.0,
            key: 0,
            depth,
        };
        let key = crate::hash_of((
            depth_key(s, &look.depth),
            look.depth.width,
            look.depth.height,
            look.stops.iter().flat_map(|c| c.map(f32::to_bits)).collect::<Vec<_>>(),
            [look.near, look.far, look.gamma, look.bands, look.amount].map(f32::to_bits),
            look.invert,
        ));
        Some(Look { key, ..look })
    }

    /// The colour (display-encoded sRGB) of nearness `n` (1 nearest, 0 farthest), before any contour line.
    pub fn colour(&self, n: f32) -> [f32; 3] {
        let n = if n.is_finite() { n.clamp(0.0, 1.0) } else { 0.0 };
        let depth = 100.0 * (1.0 - n);
        let span = self.far - self.near;
        let span = if span.abs() < 0.5 { 0.5 } else { span };
        let mut t = ((self.far - depth) / span).clamp(0.0, 1.0);
        if self.invert {
            t = 1.0 - t;
        }
        let t = t.powf(self.gamma);
        let last = self.stops.len().saturating_sub(1);
        if last == 0 {
            return self.stops.first().copied().unwrap_or([0.0; 3]);
        }
        let pos = t * last as f32;
        let i = (pos.floor() as usize).min(last - 1);
        let f = pos - i as f32;
        let (a, b) = (self.stops.get(i).copied().unwrap_or([0.0; 3]), self.stops.get(i + 1).copied().unwrap_or([0.0; 3]));
        std::array::from_fn(|k| a[k] + (b[k] - a[k]) * f)
    }

    /// How much of a contour line covers a pixel at nearness `n` whose nearness changes by `slope` per output
    /// pixel, with lines `half` output pixels wide on either side (0: none, 1: on a line). Lines follow equal
    /// depths; where the depth is flat no line is drawn (it would cover the whole flat area). The edge is a pixel
    /// wide; a line thinner than a pixel is drawn a pixel wide and fainter by as much.
    pub fn line(&self, n: f32, slope: f32, half: f32) -> f32 {
        if self.bands <= 0.0 || !n.is_finite() || !slope.is_finite() || half.is_nan() || half <= 0.0 {
            return 0.0;
        }
        let f = n.clamp(0.0, 1.0) * self.bands;
        // (lines per output pixel)
        let rate = slope.abs() * self.bands;
        if rate < 1e-6 {
            return 0.0;
        }
        let px = (f - f.round()).abs() / rate;
        let drawn = half.max(0.5);
        (1.0 - smooth(drawn - 0.5, drawn + 0.5, px)) * (half / 0.5).min(1.0)
    }
}

#[inline]
fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Draw `img` (scene-linear, the output of `frame` at `px_per_long` output pixels per long edge) as its depth, as
/// `look` says.
pub fn apply_look(img: &mut Rgb32f, look: &Look, frame: &Frame, px_per_long: f64) {
    let (w, h) = (img.width, img.height);
    if w == 0 || h == 0 {
        return;
    }
    let near = source_to_out(&smoothed(&look.depth), frame, w, h);
    let a = look.amount;
    let m = srgb_to_working();
    let half = CONTOUR_HALF_WIDTH * px_per_long as f32;
    for_rows(&mut img.data, w, |y, row| {
        let yi = y as isize;
        for (x, c) in row.iter_mut().enumerate() {
            let x = x as isize;
            let n = near.get_clamped(x, yi);
            let mut colour = look.colour(n);
            if look.bands > 0.0 {
                let gx = (near.get_clamped(x + 1, yi) - near.get_clamped(x - 1, yi)) / 2.0;
                let gy = (near.get_clamped(x, yi + 1) - near.get_clamped(x, yi - 1)) / 2.0;
                let line = look.line(n, (gx * gx + gy * gy).sqrt(), half);
                colour = colour.map(|v| v * (1.0 - CONTOUR_DARK * line));
            }
            let d = working(&m, colour);
            *c = std::array::from_fn(|k| c[k] + (d[k] - c[k]) * a);
        }
    });
}

/// The photo's depth as data, for other programs (`depthMap.export`): nearness (1 the nearest part, 0 the farthest,
/// relative inverse depth; smoothed as Depth Map draws it), one value per pixel of a render of `src` with settings `s`
/// fitted into `max_w × max_h`: framed as the photo is (`apply_crop`: its crop, straighten and flips; the user's
/// orientation and geometry always), so it lines up with an export of the photo at that size. Only the framing and the
/// depth of `s` are used. `None` without depth (the photo's own, or a stored estimate).
pub fn depth_out(src: &Rgb32f, info: &crate::SourceInfo, s: &DevelopSettings, max_w: usize, max_h: usize, apply_crop: bool) -> Option<Plane> {
    let mut framing = DevelopSettings {
        process: s.process,
        orientation: s.orientation,
        crop: s.crop,
        expand_fill: s.expand_fill.clone(),
        geometry: s.geometry.clone(),
        optics: s.optics,
        disabled_sections: s.disabled_sections.iter().filter(|x| *x == "optics" || *x == "geometry").cloned().collect(),
        ..DevelopSettings::default()
    };
    framing.lens_blur.depth = s.lens_blur.depth.clone();
    // (asked for depth, as Depth Map asks for it)
    framing.depth_map.enabled = true;
    let req = crate::RenderRequest { apply_crop, ..crate::RenderRequest::fit(max_w.max(1), max_h.max(1)) };
    let plan = crate::plan(src, info, &framing, &req);
    let depth = plan.mattes.as_deref().and_then(Mattes::depth)?;
    if depth.width == 0 || depth.height == 0 || depth.data.len() != depth.width * depth.height {
        return None;
    }
    Some(source_to_out(&smoothed(depth), &plan.frame, plan.w, plan.h))
}

#[cfg(test)]
mod tests;
