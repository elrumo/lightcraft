//! Glow (Effects panel, under Grain): light spread from the photo's bright parts, as a lens, a diffusion filter or film
//! scatters it. Added in scene-linear light right after Lens Blur and before the local tone and the tone map, so the
//! glow is part of the light the rest of the pipeline develops: it rolls off into the highlights as scattered light
//! does, and an HDR render keeps it above SDR white. The method and the numbers are in `docs/glow.md`; the Apple
//! renderer of the native frontend draws the same steps.
//!
//! 1. **Bright pass.** Each pixel's light (before Exposure: the stage runs on the un-exposed image, Exposure's gain is
//!    folded into the knee) is weighted by where its exposed luminance sits on a soft knee in stops below white: Range
//!    moves the knee from the brightest highlights (0) down into the midtones and shadows (100).
//! 2. **Spread.** That light is blurred: Bloom by a narrow and a wide Gaussian (a bright core and a long tail),
//!    Diffusion by two closer ones, Halation by one tight one, Motion by a box along a line, Streak by three boxes
//!    along a line (a peaked kernel: a bright core and a long thin tail). Sizes are fractions of the long edge, so a
//!    preview and an export look alike.
//! 3. **Colour and add.** The spread light is coloured by Warmth (cool to warm; Halation's glow is the light's
//!    luminance alone in yellow-orange to red, as film's red layer records it) and added back. Diffusion also takes
//!    part of the bright light away from where it was, so the photo softens rather than only brightening.

use lightcraft_color::{LUMA_2020, REC2020, SRGB};
use lightcraft_develop::{DevelopSettings, GlowStyle};
use lightcraft_raster::{Image, Rgb32f, blur, par_rows};

/// Spread 0 → 100 takes the blur's sigma from `SIGMA_MIN` to `SIGMA_MIN · 2^SIGMA_STOPS` of the long edge.
pub const SIGMA_MIN: f32 = 0.003;
pub const SIGMA_STOPS: f32 = 5.5;

/// Everything a render needs of the glow, resolved for its size: the same numbers the Apple renderer derives.
#[derive(Clone, Debug, PartialEq)]
pub struct Setup {
    pub style: GlowStyle,
    /// Glow 0..1.
    pub amount: f32,
    /// The bright pass's knee: where it starts and where it is whole, in stops of exposed luminance below SDR white.
    pub knee: [f32; 2],
    /// Exposure's gain: the bright pass weighs the exposed light, while the stage runs before Exposure.
    pub gain: f32,
    /// The glow's colour, linear Rec. 2020 with luminance 1.
    pub tint: [f32; 3],
    /// The base blur size, output pixels: each style's blurs are multiples of it (`blurs`).
    pub sigma: f32,
    /// Unit direction of Motion and Streak in output pixels (y down), from Angle (counter-clockwise, 0: horizontal).
    pub dir: [f32; 2],
    /// How much of the spread light is added, and how much of the bright light is taken away where it was.
    pub add: f32,
    pub take: f32,
}

/// The constants of a style (`docs/glow.md`, table "Styles").
pub struct StyleSpec {
    /// The knee's start at Range 0 (stops below white), how far Range 100 moves it down (stops) and its width.
    pub knee_start: f32,
    pub range_stops: f32,
    pub knee_width: f32,
    /// Gaussian blurs as (sigma in base sigmas, weight), weights summing to 1; or for a line style, box half-lengths in base
    /// sigmas, averaged.
    pub blurs: &'static [(f32, f32)],
    /// The spread light's gain, and the share of the bright light taken from where it was.
    pub add: f32,
    pub take: f32,
    /// Halation: the glow is the light's luminance alone, in the halation colour.
    pub mono: bool,
}

pub fn style_spec(style: GlowStyle) -> StyleSpec {
    match style {
        GlowStyle::Bloom => {
            StyleSpec { knee_start: -1.0, range_stops: 4.0, knee_width: 1.5, blurs: &[(1.0, 0.6), (4.0, 0.4)], add: 1.5, take: 0.0, mono: false }
        }
        GlowStyle::Diffusion => {
            StyleSpec { knee_start: -2.5, range_stops: 4.0, knee_width: 3.0, blurs: &[(0.5, 0.5), (2.0, 0.5)], add: 0.8, take: 0.5, mono: false }
        }
        // (the brightest lights only, a tight halo, and less of it: the halo's colour is strong)
        GlowStyle::Halation => {
            StyleSpec { knee_start: -0.5, range_stops: 2.5, knee_width: 1.0, blurs: &[(0.5, 1.0)], add: 0.8, take: 0.0, mono: true }
        }
        GlowStyle::Motion => {
            StyleSpec { knee_start: -1.0, range_stops: 4.0, knee_width: 1.5, blurs: &[(3.0, 1.0)], add: 1.5, take: 0.0, mono: false }
        }
        GlowStyle::Streak => StyleSpec {
            knee_start: -0.5,
            range_stops: 2.5,
            knee_width: 1.0,
            blurs: &[(8.0, 1.0), (2.0, 1.0), (0.5, 1.0)],
            add: 2.0,
            take: 0.0,
            mono: false,
        },
    }
}

/// The glow's colour for Warmth `w` (−1..1), linear sRGB before it is scaled to luminance 1: a cool-to-warm filter over the
/// light's own colour, or for Halation (`mono`) the colour of the halo itself, yellow-orange (−1) to red (1).
pub fn tint_srgb(w: f32, mono: bool) -> [f32; 3] {
    let w = if w.is_finite() { w.clamp(-1.0, 1.0) } else { 0.0 };
    if mono {
        let t = (w + 1.0) / 2.0;
        let (a, b) = ([1.0, 0.5, 0.18], [1.0, 0.2, 0.08]);
        std::array::from_fn(|k| a[k] + (b[k] - a[k]) * t)
    } else {
        [1.0 + 0.45 * w, 1.0 + 0.05 * w, 1.0 - 0.55 * w]
    }
}

impl Setup {
    /// The glow of `s` for a render `px_per_long` pixels along the photo's long edge; `None` when it adds nothing (Glow at 0,
    /// the Effects eye off, a value that is not a number).
    pub fn new(s: &DevelopSettings, px_per_long: f64) -> Option<Setup> {
        let g = &s.glow;
        let finite = [g.amount, g.range, g.spread, g.warmth, g.angle, s.light.exposure, px_per_long].iter().all(|v| v.is_finite());
        if !finite || g.amount <= 0.0 || !s.section_enabled("effects") || px_per_long <= 0.0 {
            return None;
        }
        let spec = style_spec(g.style);
        let range = (g.range / 100.0).clamp(0.0, 1.0) as f32;
        let start = spec.knee_start - spec.range_stops * range;
        let spread = (g.spread / 100.0).clamp(0.0, 1.0) as f32;
        let sigma = SIGMA_MIN * (SIGMA_STOPS * spread).exp2() * px_per_long as f32;
        let tint = {
            let c = SRGB.to_space(&REC2020).apply_f32(tint_srgb((g.warmth / 100.0) as f32, spec.mono));
            let y = luma(c).max(1e-6);
            c.map(|v| v / y)
        };
        let a = (g.angle.clamp(-90.0, 90.0) as f32).to_radians();
        Some(Setup {
            style: g.style,
            amount: (g.amount / 100.0).clamp(0.0, 1.0) as f32,
            knee: [start, start + spec.knee_width],
            gain: (s.light.exposure.clamp(-10.0, 10.0) as f32).exp2(),
            tint,
            sigma: sigma.max(0.0),
            dir: [a.cos(), -a.sin()],
            add: spec.add,
            take: spec.take,
        })
    }

    /// A key for the render caches: two setups with the same key draw the same glow.
    pub fn key(&self) -> u64 {
        use std::hash::{BuildHasher, BuildHasherDefault, DefaultHasher};
        let words =
            [self.amount, self.knee[0], self.knee[1], self.gain, self.tint[0], self.tint[1], self.tint[2], self.sigma, self.dir[0], self.dir[1]];
        BuildHasherDefault::<DefaultHasher>::default().hash_one((self.style as u8, words.map(f32::to_bits)))
    }

    /// How far (output px) the glow reaches: what a windowed render reads around its window, at most an eighth of the
    /// long edge `long` (a wider blur's far tail is cut at the window's margin: `docs/glow.md`, "Windows").
    pub fn reach(&self, long: usize) -> usize {
        let spec = style_spec(self.style);
        let far = spec.blurs.iter().map(|b| b.0).fold(0.0f32, f32::max) * self.sigma;
        let far = if self.style.directional() { far } else { 3.0 * far };
        let cap = (long / 8).max(1) as f32;
        far.min(cap).ceil().max(0.0) as usize
    }
}

fn luma(c: [f32; 3]) -> f32 {
    c[0] * LUMA_2020[0] + c[1] * LUMA_2020[1] + c[2] * LUMA_2020[2]
}

fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0).max(1e-6)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The bright pass's weight of a pixel of (un-exposed) light `c`.
pub fn weight(g: &Setup, c: [f32; 3]) -> f32 {
    let y = luma(c) * g.gain;
    if y <= 0.0 || !y.is_finite() {
        return 0.0;
    }
    smooth(g.knee[0], g.knee[1], y.log2())
}

/// `img` with the glow of `g` added (the image is scene-linear Rec. 2020 before Exposure, as the scene-linear stage
/// leaves it).
pub fn apply(img: &Rgb32f, g: &Setup) -> Rgb32f {
    let spec = style_spec(g.style);
    // 1. the bright pass (Halation: luminance only)
    let bright = img.map(|c| {
        let c = c.map(|v| if v.is_finite() { v.max(0.0) } else { 0.0 });
        let w = weight(g, c);
        if spec.mono { [luma(c) * w; 3] } else { c.map(|v| v * w) }
    });
    // 2. the spread
    let spread = if g.style.directional() { line_blurs(&bright, g.dir, g.sigma, spec.blurs) } else { gaussians(&bright, g.sigma, spec.blurs) };
    // 3. colour and add
    let (k_add, k_take) = (g.amount * g.add, g.amount * g.take);
    let mut out = img.clone();
    for ((o, b), s) in out.data.iter_mut().zip(&bright.data).zip(&spread.data) {
        *o = std::array::from_fn(|k| o[k] + k_add * g.tint[k] * s[k] - k_take * b[k]);
    }
    out
}

/// The weighted sum of Gaussian blurs of `img` (sigmas in multiples of `sigma`).
fn gaussians(img: &Rgb32f, sigma: f32, blurs: &[(f32, f32)]) -> Rgb32f {
    let mut out = Rgb32f::new(img.width, img.height);
    for &(k, weight) in blurs {
        let b = blur::gaussian(img, sigma * k);
        for (o, v) in out.data.iter_mut().zip(&b.data) {
            *o = std::array::from_fn(|c| o[c] + weight * v[c]);
        }
    }
    out
}

/// The average of box blurs of `img` along the line through each pixel in direction `dir`, of half-lengths `blurs[i].0 ·
/// sigma` (the second number is not read: the boxes count alike). The line is made a row by shearing the image (a
/// mostly vertical line by transposing it first), the rows are box-blurred with running sums and the image is sheared
/// back: linear in the pixels whatever the length.
fn line_blurs(img: &Rgb32f, dir: [f32; 2], sigma: f32, blurs: &[(f32, f32)]) -> Rgb32f {
    let [dx, dy] = dir;
    if !(dx.is_finite() && dy.is_finite()) || img.width == 0 || img.height == 0 {
        return img.clone();
    }
    if dx.abs() >= dy.abs() {
        sheared_blur(img, dy / dx, dx.abs(), sigma, blurs)
    } else {
        sheared_blur(&img.transpose(), dx / dy, dy.abs(), sigma, blurs).transpose()
    }
}

/// [`line_blurs`] for a line `y = y0 + x · m` with |m| ≤ 1: `along` is how far one pixel along x goes along the line
/// (its direction's x), so a half-length `L` along the line is `L · along` pixels along x.
fn sheared_blur(img: &Rgb32f, m: f32, along: f32, sigma: f32, blurs: &[(f32, f32)]) -> Rgb32f {
    let (w, h) = (img.width, img.height);
    let m = if m.is_finite() { m.clamp(-1.0, 1.0) } else { 0.0 };
    // the lines' intercepts run from `y0` over `rows` rows
    let span = (w.saturating_sub(1)) as f32 * m;
    // (a line rising to the right meets the top edge at the far end: the lowest intercept is `-span`)
    let y0 = span.max(0.0);
    let rows = (h as f32 + span.abs()).ceil() as usize + 1;
    // sheared: row j of `q` is the line with intercept `j - y0`
    let mut q: Rgb32f = Image::new(w, rows);
    par_rows(&mut q.data, w, |j, row| {
        for (x, px) in row.iter_mut().enumerate() {
            *px = sample_column(img, x, j as f32 - y0 + x as f32 * m);
        }
    });
    // the boxes along the rows, averaged
    let long = w.max(h) as f32;
    let n = blurs.len().max(1) as f32;
    let mut sum: Rgb32f = Image::new(w, rows);
    for &(k, _) in blurs {
        let r = (k * sigma * along).min(long).round().max(0.0) as usize;
        let mut b = q.clone();
        par_rows(&mut b.data, w, |_, row| box_row(row, r));
        for (s, v) in sum.data.iter_mut().zip(&b.data) {
            *s = std::array::from_fn(|c| s[c] + v[c] / n);
        }
    }
    // sheared back
    let mut out: Rgb32f = Image::new(w, h);
    par_rows(&mut out.data, w, |y, row| {
        for (x, px) in row.iter_mut().enumerate() {
            *px = sample_column(&sum, x, y as f32 + y0 - x as f32 * m);
        }
    });
    out
}

/// Column `x` of `img` at fractional row `y`, linearly between rows, the edge rows repeated beyond the image.
fn sample_column(img: &Rgb32f, x: usize, y: f32) -> [f32; 3] {
    let last = img.height.saturating_sub(1);
    let y = if y.is_finite() { y.clamp(0.0, last as f32) } else { 0.0 };
    let y_lo = (y.floor() as usize).min(last);
    let y_hi = (y_lo + 1).min(last);
    let t = y - y_lo as f32;
    let (a, b) = (img.data.get(y_lo * img.width + x).copied().unwrap_or_default(), img.data.get(y_hi * img.width + x).copied().unwrap_or_default());
    std::array::from_fn(|c| a[c] + (b[c] - a[c]) * t)
}

/// A box of radius `r` along `row` (running sums, the edge pixels repeated beyond the row).
fn box_row(row: &mut [[f32; 3]], r: usize) {
    let w = row.len();
    if r == 0 || w == 0 {
        return;
    }
    let src = row.to_vec();
    let last = w - 1;
    let at = |i: isize| src.get(i.clamp(0, last as isize) as usize).copied().unwrap_or_default();
    let inv = 1.0 / (2 * r + 1) as f32;
    let r = r as isize;
    let mut acc = [0.0f32; 3];
    for i in -r..=r {
        let v = at(i);
        acc = std::array::from_fn(|c| acc[c] + v[c]);
    }
    for (x, px) in row.iter_mut().enumerate() {
        *px = acc.map(|v| v * inv);
        let (add, sub) = (at(x as isize + r + 1), at(x as isize - r));
        acc = std::array::from_fn(|c| acc[c] + add[c] - sub[c]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightcraft_develop::Glow;

    fn settings(glow: Glow) -> DevelopSettings {
        DevelopSettings { glow, ..Default::default() }
    }

    /// A dark picture with one bright square in the middle (linear Rec. 2020, before Exposure).
    fn lamp(w: usize, h: usize) -> Rgb32f {
        Image::from_fn(
            w,
            h,
            |x, y| if (x as isize - w as isize / 2).abs() < 3 && (y as isize - h as isize / 2).abs() < 3 { [4.0; 3] } else { [0.02; 3] },
        )
    }

    #[test]
    fn no_glow_without_an_amount_or_with_the_effects_eye_off() {
        assert!(Setup::new(&settings(Glow::default()), 1000.0).is_none());
        let mut s = settings(Glow { amount: 50.0, ..Default::default() });
        assert!(Setup::new(&s, 1000.0).is_some());
        s.set_section_enabled("effects", false);
        assert!(Setup::new(&s, 1000.0).is_none(), "the Effects eye switches it off");
        let nan = settings(Glow { amount: 50.0, spread: f64::NAN, ..Default::default() });
        assert!(Setup::new(&nan, 1000.0).is_none(), "a value that is not a number draws nothing");
    }

    #[test]
    fn every_style_lights_the_dark_round_a_lamp_and_leaves_far_corners_alone() {
        let img = lamp(96, 64);
        for style in GlowStyle::ALL {
            let s = settings(Glow { amount: 80.0, style, spread: 60.0, ..Default::default() });
            let g = Setup::new(&s, 96.0).unwrap();
            let out = apply(&img, &g);
            // (just beside the lamp, along the line of the line styles: at 0 degrees, the same row; Halation's halo is
            // tight, a pixel or two at this size)
            let near = out.get(48 + 3, 32)[1];
            assert!(near > 0.02 + 1e-3, "{style:?}: the dark beside the lamp glows ({near})");
            let corner = out.get(1, 1)[1];
            assert!((corner - 0.02).abs() < 2e-3, "{style:?}: a far corner stays ({corner})");
            assert!(out.data.iter().flatten().all(|v| v.is_finite()), "{style:?}: finite");
        }
    }

    #[test]
    fn range_reaches_further_down_the_tones() {
        // a grey ramp: Range 100 glows more of it than Range 0
        let img: Rgb32f = Image::from_fn(64, 16, |x, _| [0.01 * (x as f32 + 1.0); 3]);
        let total = |range: f64| {
            let g = Setup::new(&settings(Glow { amount: 60.0, range, ..Default::default() }), 64.0).unwrap();
            let out = apply(&img, &g);
            out.data.iter().zip(&img.data).map(|(o, i)| o[1] - i[1]).sum::<f32>()
        };
        assert!(total(100.0) > total(50.0) && total(50.0) > total(0.0), "{} {} {}", total(0.0), total(50.0), total(100.0));
    }

    #[test]
    fn exposure_brings_more_of_the_photo_over_the_knee() {
        let at = |ev: f64| {
            let mut s = settings(Glow { amount: 60.0, range: 0.0, ..Default::default() });
            s.light.exposure = ev;
            weight(&Setup::new(&s, 32.0).unwrap(), [0.1; 3])
        };
        assert_eq!(at(0.0), 0.0, "grey is under Range 0's knee");
        assert!(at(3.5) > 0.5, "three and a half stops brighter it is over it");
    }

    #[test]
    fn warmth_colours_the_glow_and_halation_is_red() {
        let warm = tint_srgb(1.0, false);
        let cool = tint_srgb(-1.0, false);
        assert!(warm[0] > warm[2] && cool[2] > cool[0]);
        let red = tint_srgb(1.0, true);
        let orange = tint_srgb(-1.0, true);
        assert!(red[1] < orange[1] && red[0] >= orange[0], "Halation runs from yellow-orange to red");
        let g = Setup::new(&settings(Glow { amount: 50.0, warmth: 70.0, ..Default::default() }), 100.0).unwrap();
        assert!((luma(g.tint) - 1.0).abs() < 1e-4, "the tint keeps the light's luminance");
    }

    #[test]
    fn a_line_glow_runs_along_its_angle() {
        let img = lamp(80, 80);
        let along = |angle: f64| {
            let g = Setup::new(&settings(Glow { amount: 80.0, style: GlowStyle::Streak, angle, spread: 70.0, ..Default::default() }), 80.0).unwrap();
            let out = apply(&img, &g);
            (out.get(40 + 15, 40)[1], out.get(40, 40 - 15)[1], out.get(40 + 11, 40 - 11)[1])
        };
        let (h, v, d) = along(0.0);
        assert!(h > 0.03 && v < 0.021 && d < 0.021, "horizontal: {h} {v} {d}");
        let (h, v, _) = along(90.0);
        assert!(v > 0.03 && h < 0.021, "vertical: {h} {v}");
        let (h, v, d) = along(45.0);
        assert!(d > 0.03 && h < 0.021 && v < 0.021, "45 degrees, up and to the right: {h} {v} {d}");
        let (_, v, _) = along(-90.0);
        assert!(v > 0.03, "-90 is vertical too");
    }

    #[test]
    fn diffusion_softens_the_bright_parts_it_spreads_and_bloom_does_not() {
        // bright stripes a pixel wide: Diffusion takes part of each one's light and spreads it, so they lose contrast
        let img: Rgb32f = Image::from_fn(64, 64, |x, _| if x % 2 == 0 { [4.0; 3] } else { [2.0; 3] });
        let stripes = |style: GlowStyle| {
            let g = Setup::new(&settings(Glow { amount: 100.0, style, ..Default::default() }), 64.0).unwrap();
            let out = apply(&img, &g);
            out.get(32, 32)[1] - out.get(33, 32)[1]
        };
        assert!(stripes(GlowStyle::Diffusion) < 1.5, "Diffusion softens: {}", stripes(GlowStyle::Diffusion));
        assert!(stripes(GlowStyle::Bloom) > 1.9, "Bloom only adds light: {}", stripes(GlowStyle::Bloom));
    }

    #[test]
    fn the_glow_is_the_same_size_at_any_resolution() {
        // a quarter-size render's glow at a point is close to the full-size render's at the same place
        let at = |n: usize| {
            let img = Image::from_fn(n, n, |x, y| if x < n / 2 && y < n / 2 { [2.0; 3] } else { [0.0; 3] });
            let g = Setup::new(&settings(Glow { amount: 60.0, ..Default::default() }), n as f64).unwrap();
            apply(&img, &g).get(n * 5 / 8, n / 4)[1]
        };
        let (small, large) = (at(128), at(512));
        assert!((small - large).abs() < 0.1 * large.max(1e-3), "{small} vs {large}");
    }

    #[test]
    fn hostile_numbers_and_tiny_pictures_do_not_panic() {
        for style in GlowStyle::ALL {
            let s = settings(Glow { amount: 100.0, style, spread: 100.0, angle: 33.0, ..Default::default() });
            let g = Setup::new(&s, 1e9).unwrap();
            for (w, h) in [(1, 1), (1, 7), (7, 1), (3, 2)] {
                let img: Rgb32f = Image::from_fn(w, h, |x, _| if x == 0 { [f32::INFINITY, f32::NAN, -1.0] } else { [1.0; 3] });
                let out = apply(&img, &g);
                assert_eq!((out.width, out.height), (w, h));
            }
            let _ = g.reach(0);
        }
    }
}
