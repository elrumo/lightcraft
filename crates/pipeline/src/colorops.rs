//! Colour tools in OkLCh on display-linear Rec.2020 values: vibrance, saturation, the 8-band colour
//! mixer (its Saturation / Hue / Luminance as HSV operations, [`mix_hsv`]), B&W mix, and 3-way colour
//! grading; plus the camera-calibration matrix (scene linear).

use std::f32::consts::{PI, TAU};
use std::sync::OnceLock;

use lightcraft_color::Map3;
use lightcraft_color::perceptual::{hsv_to_rgb, lab_to_lch, lch_to_lab, oklab_from_2020, oklab_to_2020, rgb_to_hsv};
use lightcraft_color::{Mat3, REC2020, SRGB};
use lightcraft_develop::{Calibration, DevelopSettings, MIXER_HUES, PointColor};

/// OkLCh hue angle (radians) of a pure sRGB colour with HSV hue `deg`.
pub fn oklch_hue_of_srgb_hue(deg: f64) -> f32 {
    let c = hsv_to_rgb(deg as f32, 1.0, 1.0);
    let lin = c.map3(lightcraft_color::transfer::srgb_to_linear);
    let m = SRGB.to_space(&REC2020).apply_f32(lin);
    lab_to_lch(oklab_from_2020(m))[2]
}

/// OkLCh hue (radians) of each colour-mixer band centre.
pub fn band_hues() -> &'static [f32; 8] {
    static H: OnceLock<[f32; 8]> = OnceLock::new();
    H.get_or_init(|| MIXER_HUES.map(oklch_hue_of_srgb_hue))
}

#[inline]
fn wrap(a: f32) -> f32 {
    (a + PI).rem_euclid(TAU) - PI
}

/// Partition-of-unity weights of hue `h` over the 8 bands (raised cosine between neighbours).
#[inline]
pub fn band_weights(h: f32) -> [f32; 8] {
    let hues = band_hues();
    let mut w = [0.0f32; 8];
    for i in 0..8 {
        let a = hues[i];
        let b = hues[(i + 1) % 8];
        let span = wrap(b - a).rem_euclid(TAU);
        let d = wrap(h - a).rem_euclid(TAU);
        if d <= span {
            let t = d / span;
            let s = 0.5 - 0.5 * (t * PI).cos();
            w[i] += 1.0 - s;
            w[(i + 1) % 8] += s;
            break;
        }
    }
    w
}

// The mixer's band centres ([`MIXER_HUES`]) and the constants below are fitted to Lightroom's
// single-slider exports (Hue +50, Saturation −50, Luminance −50 of Orange, Green and Blue on an
// iPhone ProRAW and two EOS R6 CR3s, 2026-10-20; `docs/parity-checklist.md` → LR-EDIT-COLOR-MIXER-HSL). Only Orange
// and Blue had pixels to measure: the other bands keep the earlier values, and the slider halves
// that weren't exported (Hue −, Saturation +, Luminance +) follow the fitted half.

/// Hue rotation (radians) of each colour-mixer band at ±100.
pub const MIXER_HUE: [f32; 8] = [1.0, 0.8, 1.0, 1.0, 1.0, 1.26, 1.0, 1.0];
/// Share of a Hue shift's luminance change that is undone (at the same brightest channel): a sky
/// keeps its lightness as it turns purple, an orange lightens towards yellow as plain HSV does.
pub const MIXER_HUE_KEEP_Y: [f32; 8] = [0.0, 0.0, 0.0, 0.0, 0.0, 0.5, 0.0, 0.0];
/// Gain on the encoded colour of a colour-mixer band at Luminance ±100 (`1 ± MIXER_LUM`).
pub const MIXER_LUM: [f32; 8] = [0.4, 0.475, 0.4, 0.4, 0.4, 0.725, 0.4, 0.4];
/// How much Luminance also reshapes the channels under the brightest one at ±100 (exponent
/// `exp(−k·d)`): darkening a sky deepens its blue, brightening washes it out.
pub const MIXER_LUM_SHAPE: [f32; 8] = [0.0, 0.31, 0.0, 0.0, 0.0, 1.19, 0.0, 0.0];
/// Lowering Saturation moves a colour towards a grey this far from its brightest channel towards
/// its luminance (0 keeps the brightest channel, 1 the luminance).
pub const MIXER_SAT_GREY: f32 = 0.53;
/// Encoding the mixer works in (display-linear → gamma, as the HSL panel's own model).
const MIXER_GAMMA: f32 = 2.2;

/// The colour mixer's step on a display-linear colour: Saturation, Hue and Luminance as HSV
/// operations on the gamma-encoded colour, not as OkLCh ones, as Lightroom's HSL panel behaves.
/// Lowering a band's Saturation moves it halfway from its brightest channel to its luminance, so a
/// blue sky turns paler and somewhat lighter; a Hue shift keeps the colour's value (blue towards
/// aqua gets lighter), `keep` of its luminance change undone; Luminance is a gain on the encoded
/// colour plus a reshaping under its brightest channel (exponent `exp(−dp)`), so a darkened sky is
/// also a deeper blue. `dh` is the hue rotation in radians, `ds` the saturation change (−1…), `dl`
/// the gain − 1 (all already weighted by the bands).
#[inline]
pub fn mix_hsv(rgb: [f32; 3], dh: f32, ds: f32, dl: f32, dp: f32, keep: f32) -> [f32; 3] {
    let y = lightcraft_color::luminance_2020;
    let max3 = |c: [f32; 3]| c[0].max(c[1]).max(c[2]);
    let lin = rgb.map(|v| v.max(0.0));
    let mut g = lin.map(|v| v.powf(1.0 / MIXER_GAMMA));
    if ds != 0.0 {
        let vmax = max3(g);
        let grey = vmax + (y(lin).max(0.0).powf(1.0 / MIXER_GAMMA) - vmax) * MIXER_SAT_GREY;
        g = g.map(|x| (grey + (x - grey) * (1.0 + ds)).max(0.0));
    }
    let mut lin = g.map(|x| x.powf(MIXER_GAMMA));
    if dh != 0.0 {
        let [h, s, v] = rgb_to_hsv(g);
        if s > 1e-6 {
            let y0 = y(lin);
            let mut out = hsv_to_rgb(h + dh.to_degrees(), s, v).map(|x| x.max(0.0).powf(MIXER_GAMMA));
            let vmax = max3(out);
            let den = vmax - y(out);
            if keep > 0.0 && den > 1e-6 {
                let k = 1.0 + keep * (((vmax - y0) / den).clamp(0.0, 3.0) - 1.0);
                out = out.map(|x| (vmax + (x - vmax) * k).max(0.0));
            }
            lin = out;
        }
    }
    let vmax = max3(lin);
    if dp != 0.0 && vmax > 1e-9 {
        let e = (-dp).exp();
        lin = lin.map(|x| vmax * (x / vmax).powf(e));
    }
    let gain = (1.0 + dl).max(0.0).powf(MIXER_GAMMA);
    lin.map(|x| x * gain)
}

/// A colour-grading wheel as an OkLab offset (a, b) and a lightness shift.
#[derive(Clone, Copy, Debug)]
pub struct WheelK {
    pub a: f32,
    pub b: f32,
    pub lum: f32,
}

/// Point Color against Lightroom's exports of one sample on six cameras (`test-raws/light/xcam7`, 35 cases each, the
/// constants fitted together: card #536, `docs/lightroom-preset-fit.md`). The range box's half-widths `(at Range 0, per
/// unit of Range 0..1)`, each scaled by its own range slider (50 is × 1): hue in radians, OkLab chroma, OkLab
/// lightness. Lightroom's Range hardly widens the hues a sample takes (22° at 0, 26° at 100): it widens the chroma
/// and the lightness it takes.
pub const POINT_HUE_WIDTH: (f32, f32) = (0.38, 0.08);
pub const POINT_CHROMA_WIDTH: (f32, f32) = (0.05, 0.09);
pub const POINT_LUM_WIDTH: (f32, f32) = (0.155, 0.35);
/// The share of a half-width inside which a colour is fully selected; the selection falls off smoothly from there.
pub const POINT_INNER: f32 = 0.32;
/// The shifts at ±100 on a fully selected colour: Hue turns it (radians), Luminance scales its lightness and chroma
/// by `e^±POINT_LUM` (the whole colour gets lighter or darker, as Lightroom's does), Saturation leaves
/// `1 − POINT_SAT.0` of its chroma at −100 and gives it `1 + POINT_SAT.1` at +100.
pub const POINT_HUE: f32 = 0.78;
pub const POINT_LUM: f32 = 0.2;
pub const POINT_SAT: (f32, f32) = (0.36, 0.89);

/// Number of `f32`s per Point Color sample in [`PointK::words`] (the GPU parameter layout).
pub const POINT_WORDS: usize = 10;

/// A Point Color sample resolved for the per-pixel stage: the sample (OkLab lightness, chroma, hue
/// in radians), the adjustment (hue rotation in radians, chroma scale − 1, the log of the scale Luminance gives
/// lightness and chroma, variance factor − 1) and the half-widths of the range box (hue radians, chroma, lightness).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointK {
    pub l: f32,
    pub c: f32,
    pub h: f32,
    pub dh: f32,
    pub sat: f32,
    pub dl: f32,
    pub var: f32,
    pub wh: f32,
    pub wc: f32,
    pub wl: f32,
}

impl PointK {
    pub fn new(p: &PointColor) -> PointK {
        let r = (p.range.clamp(0.0, 100.0) / 100.0) as f32;
        // (never zero: the windows divide by it)
        let width = |(at0, per): (f32, f32), axis: f64| ((at0 + per * r) * (axis.clamp(0.0, 100.0) / 50.0) as f32).max(1e-3);
        let shift = |v: f64| (v.clamp(-100.0, 100.0) / 100.0) as f32;
        let sat = shift(p.sat_shift);
        PointK {
            l: p.lum as f32,
            c: p.chroma.max(0.0) as f32,
            h: (p.hue as f32).to_radians(),
            dh: shift(p.hue_shift) * POINT_HUE,
            sat: sat * if sat < 0.0 { POINT_SAT.0 } else { POINT_SAT.1 },
            dl: shift(p.lum_shift) * POINT_LUM,
            var: shift(p.variance) * 0.8,
            wh: width(POINT_HUE_WIDTH, p.hue_range),
            wc: width(POINT_CHROMA_WIDTH, p.sat_range),
            wl: width(POINT_LUM_WIDTH, p.lum_range),
        }
    }

    pub fn words(&self) -> [f32; POINT_WORDS] {
        [self.l, self.c, self.h, self.dh, self.sat, self.dl, self.var, self.wh, self.wc, self.wl]
    }

    /// How much an OkLCh colour belongs to this sample's range (0..1, soft edges).
    #[inline]
    pub fn weight(&self, l: f32, c: f32, h: f32) -> f32 {
        // hue is meaningless for near-neutral colours: a chromatic sample ignores them, a
        // near-neutral sample selects by chroma and lightness only
        let wh = if self.c < 0.02 { 1.0 } else { (1.0 - smooth(POINT_INNER * self.wh, self.wh, wrap(h - self.h).abs())) * smooth(0.005, 0.025, c) };
        if wh <= 0.0 {
            return 0.0;
        }
        let wc = 1.0 - smooth(POINT_INNER * self.wc, self.wc, (c - self.c).abs());
        let wl = 1.0 - smooth(POINT_INNER * self.wl, self.wl, (l - self.l).abs());
        wh * wc * wl
    }

    /// Apply this sample's adjustment to an OkLCh colour.
    #[inline]
    pub fn apply(&self, lch: [f32; 3]) -> [f32; 3] {
        let [mut l, mut c, mut h] = lch;
        let w = self.weight(l, c, h);
        if w <= 0.0 {
            return lch;
        }
        h += w * (self.var * wrap(h - self.h) + self.dh);
        c += w * self.var * (c - self.c);
        c = (c * (1.0 + w * self.sat)).max(0.0);
        l += w * self.var * (l - self.l);
        // Luminance scales the whole colour: lightness and chroma alike
        let lum = (w * self.dl).exp();
        [l * lum, c * lum, h]
    }
}

/// The colour tools' parameters, resolved once per render (fields are read by the GPU kernel).
#[derive(Clone, Debug)]
pub struct ColorOps {
    pub vibrance: f32,
    pub saturation: f32,
    /// Density (−1..1), [`deepen`].
    pub density: f32,
    pub hue: [f32; 8],
    pub sat: [f32; 8],
    pub lum: [f32; 8],
    /// Luminance's reshaping per band ([`MIXER_LUM_SHAPE`] × the slider).
    pub lum_shape: [f32; 8],
    pub mixer: bool,
    /// Point Color samples that change something (applied after the mixer).
    pub points: Vec<PointK>,
    pub bw: Option<[f32; 8]>,
    /// Wheels (shadows, midtones, highlights, global), then Blending and Balance as the per-pixel stage reads them:
    /// the share of the wide windows ([`grading_mix`]) and the exponent on lightness ([`grading_gamma`]).
    pub grading: Option<([WheelK; 4], f32, f32)>,
    /// OkLCh hue (radians) at the middle of the colours a raised Vibrance protects ([`VIBRANCE_PROTECT_HUE`]).
    pub skin: f32,
}

// Vibrance is fitted to Lightroom's exports of six cameras' raws with Vibrance alone at +50 and −50 (cards #386 / #388,
// `docs/lightroom-preset-fit.md`). There −50 scales the chroma of every colour by about a half, weak or strong alike,
// and +50 raises it by 30 to 45 % with reds to oranges held to 10 to 13 %; the earlier `1 ± v · 1.2 · (1 − c / 0.22)²`
// left strong colours almost alone both ways (−50 kept 0.64 to 0.94 of the chroma). Lightroom's Vibrance also moves
// lightness with the hue (−50 darkens yellows and greens by 3 to 5 L* and lightens blues by up to 6): not modelled.

/// Chroma gain of a weak colour at Vibrance +100 (`1 + VIBRANCE_UP`), away from the protected hues.
pub const VIBRANCE_UP: f32 = 0.66;
/// OkLab chroma at which a raised Vibrance would no longer add anything (the gain falls linearly towards it: Lightroom's
/// falls only a little with chroma, 1.41 for weak yellows against 1.30 for strong ones).
pub const VIBRANCE_CHROMA: f32 = 1.0;
/// Lowering Vibrance scales chroma by `exp(v · VIBRANCE_DOWN)`: 0.58 at −50, a third at −100.
pub const VIBRANCE_DOWN: f32 = 1.1;
/// Middle of the hues a raised Vibrance protects (OkLCh, radians: reds to oranges, skin among them).
pub const VIBRANCE_PROTECT_HUE: f32 = 0.35;
/// Half-width of the protected hues (radians): protected in full inside half of it, not at all outside it.
pub const VIBRANCE_PROTECT_WIDTH: f32 = 1.2;
/// How much of a raised Vibrance the protected hues lose.
pub const VIBRANCE_PROTECT: f32 = 0.7;

/// Vibrance's chroma scale (`vib` −1..1) for a colour of OkLab chroma `c` and hue `h` (radians), `skin` the middle of
/// the protected hues.
#[inline]
pub fn vibrance_scale(vib: f32, c: f32, h: f32, skin: f32) -> f32 {
    if vib > 0.0 {
        let protect = VIBRANCE_PROTECT * (1.0 - smooth(0.5 * VIBRANCE_PROTECT_WIDTH, VIBRANCE_PROTECT_WIDTH, wrap(h - skin).abs()));
        1.0 + vib * VIBRANCE_UP * (1.0 - c / VIBRANCE_CHROMA).clamp(0.0, 1.0) * (1.0 - protect)
    } else {
        (vib * VIBRANCE_DOWN).exp()
    }
}

/// How much darker Density at +100 makes a fully colourful pixel (OkLab lightness × `1 − DENSITY_DEPTH`), and how far
/// toward white −100 lifts it.
pub const DENSITY_DEPTH: f32 = 0.3;
/// OkLab chroma at which Density acts in full; greys (chroma 0) are untouched.
pub const DENSITY_CHROMA: f32 = 0.2;

/// Density on OkLCh lightness and chroma: Saturation makes colour louder, Density deeper. Above 0 a colourful pixel gets
/// darker at its chroma, as a denser dye would print it (the cloth's red sits deeper, a blue shadow keeps its blue);
/// below 0 it gets lighter and paler, thinner. The weight grows with chroma, so greys and skin move least.
#[inline]
pub fn deepen(l: f32, c: f32, density: f32) -> (f32, f32) {
    let k = density * (c / DENSITY_CHROMA).min(1.0);
    if k >= 0.0 { (l * (1.0 - DENSITY_DEPTH * k), c) } else { (l - (1.0 - l).max(0.0) * DENSITY_DEPTH * k, c * (1.0 + DENSITY_DEPTH * k)) }
}

// Colour grading is fitted to Lightroom's exports of six cameras' raws (Sony ILCE-6000 and ILCE-6700, Canon EOS RP,
// Fujifilm X-A5, Nikon D70, Olympus E-1) with one wheel at a time: the global wheel at four hues, shadows at Blending
// 0 / 50 / 100 and Balance +50, highlights at Blending 50 / 100 and Balance −50, midtones, and Shadows Luminance
// (cards #386 / #388, `docs/lightroom-preset-fit.md`). What they show, and the earlier model (an OkLab offset towards
// the wheel's sRGB hue, the same at every lightness) did not have:
// - a wheel's hue is an HSV hue on ProPhoto's primaries: the tint's direction matches that on seven hues within 3°
//   (sRGB's are 10 to 30° away), and so does its strength from hue to hue ([`grading_tint`]);
// - the tint fades towards black and white ([`grading_fade`]);
// - Blending mixes two fixed windows per range, a narrow one (0) and a wide one (100), and at 50 the wide ones have
//   under a quarter of the say; Balance moves the ranges like a gamma on lightness ([`grading_weights`]);
// - the midtone window is a bump of its own, and the shadow and highlight wheels are stronger than the global one.
// A file without `ColorGradeBlending` is Blending 100 there (the older Split Toning panel).

/// OkLab chroma the global wheel adds at saturation 100, averaged over the hues, where the tint is in full.
pub const GRADING_CHROMA: f32 = 0.114;
/// Strength of the shadows, midtones, highlights and global wheels against [`GRADING_CHROMA`].
pub const GRADING_GAIN: [f32; 4] = [1.425, 0.686, 2.064, 1.0];
/// The tint's fade towards black and white: in full at this OkLab lightness, and `1 − u^q` away from it, `u` the
/// share of the way to black (exponent `.1`) or to white (`.2`).
pub const GRADING_FADE: (f32, f32, f32) = (0.669, 1.634, 2.624);
/// The shadows window's fall (smoothstep edges on lightness) at Blending 0 and at Blending 100.
pub const GRADING_SHADOWS: [(f32, f32); 2] = [(0.072, 0.784), (0.408, 1.03)];
/// The highlights window's rise at Blending 0 and at Blending 100.
pub const GRADING_HIGHLIGHTS: [(f32, f32); 2] = [(0.542, 1.265), (0.108, 1.437)];
/// The midtones window: its middle and half-width in lightness.
pub const GRADING_MIDTONES: (f32, f32) = (0.634, 0.467);
/// The wide windows' share is Blending (0..1) to this power.
pub const GRADING_BLEND_POWER: f32 = 2.067;
/// Balance (−1..1) raises lightness to `2^(−Balance · GRADING_BALANCE)` before the windows read it.
pub const GRADING_BALANCE: f32 = 1.519;
/// OkLab lightness a wheel's Luminance adds at ±100 (Lightroom's Shadows Luminance +50 lifts its range by 5 to 7 L*).
pub const GRADING_LUM: f32 = 0.12;

/// OkLab (a, b) of a small tint from grey towards a colour given on ProPhoto's primaries (row-major 2 × 3, applied to
/// the wheel's HSV colour), scaled so that its mean length over the hues is 1.
pub fn grading_tint() -> &'static [[f32; 3]; 2] {
    static A: OnceLock<[[f32; 3]; 2]> = OnceLock::new();
    A.get_or_init(|| {
        let [to_lms, _, to_lab, _] = lightcraft_color::perceptual::oklab_matrices();
        let (_, from_wide, _) = crate::finish::curve_space();
        let mul = |a: &[[f32; 3]; 3], b: &[[f32; 3]; 3]| -> [[f32; 3]; 3] {
            std::array::from_fn(|r| std::array::from_fn(|c| a[r][0] * b[0][c] + a[r][1] * b[1][c] + a[r][2] * b[2][c]))
        };
        // (at a grey the cube roots of L, M and S are equal, so the slope there is these matrices up to one factor)
        let m = mul(&to_lab, &mul(&to_lms, &from_wide));
        let mut a = [m[1], m[2]];
        let len = |a: &[[f32; 3]; 2], deg: f32| {
            let t = hsv_to_rgb(deg, 1.0, 1.0);
            dot3(a[0], t).hypot(dot3(a[1], t))
        };
        let mean = (0..72).map(|i| len(&a, i as f32 * 5.0)).sum::<f32>() / 72.0;
        if mean > 0.0 && mean.is_finite() {
            a = a.map(|row| row.map(|v| v / mean));
        }
        a
    })
}

#[inline]
fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The wide windows' share for a Blending slider (0..100), as [`grading_weights`] takes it.
pub fn grading_mix(blending: f64) -> f32 {
    let b = (blending / 100.0) as f32;
    if b.is_finite() { b.clamp(0.0, 1.0).powf(GRADING_BLEND_POWER) } else { 0.0 }
}

/// The exponent on lightness for a Balance slider (−100..100), as [`grading_weights`] takes it.
pub fn grading_gamma(balance: f64) -> f32 {
    let b = (balance / 100.0) as f32;
    if b.is_finite() { (-b.clamp(-1.0, 1.0) * GRADING_BALANCE).exp2() } else { 1.0 }
}

/// How much the shadows, midtones and highlights wheels act at OkLab lightness `l`: `mix` the wide windows' share
/// ([`grading_mix`]), `gamma` the Balance exponent ([`grading_gamma`]).
#[inline]
pub fn grading_weights(l: f32, mix: f32, gamma: f32) -> [f32; 3] {
    let x = l.clamp(0.0, 1.0).powf(gamma);
    let ([sn, sw], [hn, hw]) = (GRADING_SHADOWS, GRADING_HIGHLIGHTS);
    let ws = 1.0 - (smooth(sn.0, sn.1, x) * (1.0 - mix) + smooth(sw.0, sw.1, x) * mix);
    let wh = smooth(hn.0, hn.1, x) * (1.0 - mix) + smooth(hw.0, hw.1, x) * mix;
    let wm = 1.0 - smooth(0.0, 1.0, (x - GRADING_MIDTONES.0).abs() / GRADING_MIDTONES.1);
    [ws, wm, wh]
}

/// How much of a wheel's tint a colour of OkLab lightness `l` takes: all of it in the mid tones, none at black or white.
#[inline]
pub fn grading_fade(l: f32) -> f32 {
    let l = l.clamp(0.0, 1.0);
    let (c, dark, bright) = GRADING_FADE;
    if l < c { 1.0 - ((c - l) / c).powf(dark) } else { 1.0 - ((l - c) / (1.0 - c)).powf(bright) }
}

fn wheel(w: &lightcraft_develop::Wheel, gain: f32) -> WheelK {
    let t = hsv_to_rgb(w.hue as f32, 1.0, 1.0);
    let a = grading_tint();
    let s = (w.sat / 100.0) as f32 * GRADING_CHROMA * gain;
    WheelK { a: s * dot3(a[0], t), b: s * dot3(a[1], t), lum: (w.lum / 100.0) as f32 * GRADING_LUM }
}

impl ColorOps {
    pub fn new(s: &DevelopSettings) -> ColorOps {
        let bands = s.mixer.bands();
        let g = &s.grading;
        // (the black-and-white treatment would take the colours of a colorized photo back off)
        let bw = crate::is_bw(s) && !crate::colorize::active(s);
        ColorOps {
            vibrance: (s.color.vibrance / 100.0) as f32,
            saturation: (s.color.saturation / 100.0) as f32,
            density: (s.color.density / 100.0) as f32,
            hue: std::array::from_fn(|i| (bands[i].hue / 100.0) as f32 * MIXER_HUE[i]),
            sat: bands.map(|b| (b.sat / 100.0) as f32),
            lum: std::array::from_fn(|i| (bands[i].lum / 100.0) as f32 * MIXER_LUM[i]),
            lum_shape: std::array::from_fn(|i| (bands[i].lum / 100.0) as f32 * MIXER_LUM_SHAPE[i]),
            mixer: !s.mixer.is_neutral(),
            points: if bw {
                Vec::new()
            } else {
                s.point_colors.iter().take(lightcraft_develop::MAX_POINT_COLORS).filter(|p| !p.is_neutral()).map(PointK::new).collect()
            },
            bw: bw.then(|| s.bw_mix.bands().map(|v| (v / 100.0) as f32)),
            grading: (!g.is_neutral()).then(|| {
                let [shadows, midtones, highlights, global] = GRADING_GAIN;
                (
                    [wheel(&g.shadows, shadows), wheel(&g.midtones, midtones), wheel(&g.highlights, highlights), wheel(&g.global, global)],
                    grading_mix(g.blending),
                    grading_gamma(g.balance),
                )
            }),
            skin: VIBRANCE_PROTECT_HUE,
        }
    }

    pub fn is_identity(&self) -> bool {
        self.vibrance == 0.0
            && self.saturation == 0.0
            && self.density == 0.0
            && !self.mixer
            && self.points.is_empty()
            && self.bw.is_none()
            && self.grading.is_none()
    }

    /// `local_sat` (−1..1) and `local_hue` (radians) come from masks.
    #[inline]
    pub fn apply(&self, rgb: [f32; 3], local_sat: f32, local_hue: f32) -> [f32; 3] {
        if self.is_identity() && local_sat == 0.0 && local_hue == 0.0 {
            return rgb;
        }
        let lab = oklab_from_2020(rgb);
        // only chroma changes (vibrance / saturation, maybe grading): scale a, b directly — the same
        // result as the OkLCh round trip without its sin / cos (and atan2 unless vibrance needs the hue)
        if !self.mixer && self.points.is_empty() && self.bw.is_none() && local_hue == 0.0 {
            let mut lab = lab;
            let c0 = (lab[1] * lab[1] + lab[2] * lab[2]).sqrt();
            let mut c = c0;
            if self.vibrance != 0.0 {
                // (only a raised Vibrance reads the hue)
                let h = if self.vibrance > 0.0 { lab[2].atan2(lab[1]) } else { 0.0 };
                c *= vibrance_scale(self.vibrance, c, h, self.skin);
            }
            if self.saturation != 0.0 || local_sat != 0.0 {
                c *= (1.0 + self.saturation + local_sat).max(0.0);
            }
            if self.density != 0.0 {
                (lab[0], c) = deepen(lab[0], c, self.density);
            }
            if c0 > 0.0 {
                let k = c / c0;
                lab[1] *= k;
                lab[2] *= k;
            }
            return self.grade(lab);
        }
        let [mut l, mut c, mut h] = lab_to_lch(lab);
        if self.mixer {
            let w = band_weights(h);
            let (mut dh, mut ds, mut dl, mut dp, mut kn, mut kd) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
            for i in 0..8 {
                dh += w[i] * self.hue[i];
                ds += w[i] * self.sat[i];
                dl += w[i] * self.lum[i];
                dp += w[i] * self.lum_shape[i];
                let turn = (w[i] * self.hue[i]).abs();
                kn += turn * MIXER_HUE_KEEP_Y[i];
                kd += turn;
            }
            let keep = if kd > 0.0 { kn / kd } else { 0.0 };
            let chroma_w = (c / 0.12).min(1.0);
            [l, c, h] = lab_to_lch(oklab_from_2020(mix_hsv(rgb, dh * chroma_w, ds, dl * chroma_w, dp * chroma_w, keep)));
        }
        for p in &self.points {
            [l, c, h] = p.apply([l, c, h]);
        }
        if self.vibrance != 0.0 {
            c *= vibrance_scale(self.vibrance, c, h, self.skin);
        }
        if self.saturation != 0.0 || local_sat != 0.0 {
            c *= (1.0 + self.saturation + local_sat).max(0.0);
        }
        if self.density != 0.0 {
            (l, c) = deepen(l, c, self.density);
        }
        h += local_hue;
        if let Some(bw) = &self.bw {
            let w = band_weights(h);
            let mix: f32 = (0..8).map(|i| w[i] * bw[i]).sum();
            l = (l + mix * (c / 0.2).min(1.0) * 0.25).max(0.0);
            c = 0.0;
        }
        self.grade(lch_to_lab([l, c, h]))
    }

    /// Colour grading on OkLab, then back to linear Rec.2020.
    #[inline]
    fn grade(&self, mut lab: [f32; 3]) -> [f32; 3] {
        if let Some((wheels, mix, gamma)) = &self.grading {
            let [ws, wm, wh] = grading_weights(lab[0], *mix, *gamma);
            let fade = grading_fade(lab[0]);
            for (k, wt) in wheels.iter().zip([ws, wm, wh, 1.0]) {
                lab[1] += k.a * wt * fade;
                lab[2] += k.b * wt * fade;
                lab[0] += k.lum * wt;
            }
        }
        oklab_to_2020(lab)
    }
}

// The Calibration primaries are fitted to Lightroom's exports of six cameras' raws with one slider at ±50 (and of an
// EOS R6 raw with each slider at +50 and −50; cards #386 / #388, `docs/lightroom-preset-fit.md`). A white-preserving
// matrix fitted to each export says the same on every camera once it is written on ProPhoto's primaries: a primary's
// sliders leave its own channel alone and mix it into the other two. Red Saturation +50 is `G' = G + 0.18 (G − R)`,
// `B' = B + 0.15 (B − R)`; Red Hue +50 moves green towards red and blue away from it by about as much; −50 is the
// mirror image. So every Saturation slider raises the chroma of all colours (20 to 25 % at +50), and a Hue slider
// turns most of them (Blue Hue +50: yellows by 20 to 27°, blues by 30°). The earlier model turned each Rec. 2020
// primary's chromaticity about the white in CIE xy: its Hue sliders turned yellows and purples by 1 to 2° where
// Lightroom's turn them by 6 to 14°, and its Saturation +50 added about half of Lightroom's chroma.

/// How far the red, green and blue primary's Saturation at +100 moves the other two channels away from its own, on
/// linear ProPhoto (as a share of their distance to it). One value for red and green and one for blue fit the six
/// cameras as well as a value per channel pair does (mean effect ΔE 1.56 against 1.54; 2.20 before).
pub const CALIB_SAT: [f64; 3] = [0.31, 0.31, 0.41];
/// How far a primary's Hue at +100 moves the next channel (green for red, blue for green, red for blue) towards its
/// own and the one after away from it.
pub const CALIB_HUE: [f64; 3] = [0.25, 0.25, 0.34];

/// The calibration panel's primaries as a white-preserving 3×3 matrix on linear Rec.2020 (row-major): on ProPhoto's
/// primaries each slider mixes its own channel `p` into the two others, `q' = q + k (q − p)` with `k` from
/// [`CALIB_SAT`] and [`CALIB_HUE`], and the sliders' mixes add up. `None` when neutral.
pub fn calibration_matrix(c: &Calibration) -> Option<[[f32; 3]; 3]> {
    let prim = c.primaries();
    if prim.iter().all(|(h, s)| *h == 0.0 && *s == 0.0) {
        return None;
    }
    let mut m = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0f64]];
    for (i, (hue, sat)) in prim.iter().enumerate() {
        let (h, s) = (hue.clamp(-100.0, 100.0) / 100.0, sat.clamp(-100.0, 100.0) / 100.0);
        let (q, r) = ((i + 1) % 3, (i + 2) % 3);
        let (sat_k, hue_k) = (CALIB_SAT.get(i)?, CALIB_HUE.get(i)?);
        let kq = s * sat_k - h * hue_k;
        let kr = s * sat_k + h * hue_k;
        m[q][q] += kq;
        m[q][i] -= kq;
        m[r][r] += kr;
        m[r][i] -= kr;
    }
    if !m.iter().flatten().all(|v| v.is_finite()) {
        return None;
    }
    let to_wide = REC2020.to_space(&lightcraft_color::PROPHOTO);
    let from_wide = to_wide.inverse()?;
    Some(from_wide.mul(&Mat3(m)).mul(&to_wide).to_f32())
}

/// Shadows-tint strength at ±100: the green channel is scaled by `exp(∓SHADOW_TINT · w)` (luminance kept), `w` the
/// weight of [`SHADOW_TINT_WINDOW`]. Fitted to Lightroom's exports of seven cameras' raws with Shadows Tint alone at
/// ±50 (cards #386 / #388, `docs/lightroom-preset-fit.md`). Lightroom's own strength differs from raw to raw: at +50
/// it moves a* in the dark tones by 10 on a Sony ILCE-6000 raw, 6 to 11 on an EOS R6's, 4 to 5 on a Nikon D70's, a
/// Sony ILCE-6700's and a Canon EOS RP's, and 2 on a Fujifilm X-A5's and an Olympus E-1's, following the raw's white
/// balance multipliers and exposure (it acts like an offset in the raw's own units). A fixed strength cannot follow
/// that: this one sits in the middle (the 0.45 fitted to the EOS R6 alone did two to five times too much on five of
/// the other six).
pub const SHADOW_TINT: f32 = 0.25;
/// Where the shadows tint acts, in stops from scene mid grey (0.18): fully below the first, not at all above the
/// second. Lightroom's tint is strongest in the darkest tones and falls off evenly up to the highlights.
pub const SHADOW_TINT_WINDOW: (f32, f32) = (-5.0, 3.0);

/// Calibration in scene-linear light (before tone mapping): the primaries matrix, then the shadows
/// tint (green ↔ magenta, weighted towards dark tones, luminance kept).
#[inline]
pub fn calibrate(c: [f32; 3], m: Option<&[[f32; 3]; 3]>, shadow_tint: f32) -> [f32; 3] {
    let mut c = c;
    if let Some(m) = m {
        c = std::array::from_fn(|r| (m[r][0] * c[0] + m[r][1] * c[1] + m[r][2] * c[2]).max(0.0));
    }
    if shadow_tint != 0.0 {
        let y0 = lightcraft_color::luminance_2020(c);
        let w = 1.0 - smooth(SHADOW_TINT_WINDOW.0, SHADOW_TINT_WINDOW.1, (y0.max(1e-7) / 0.18).log2());
        c[1] *= (-SHADOW_TINT * shadow_tint * w).exp();
        let y1 = lightcraft_color::luminance_2020(c).max(1e-9);
        c = c.map3(|v| v * y0 / y1);
    }
    c
}

#[inline]
fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightcraft_color::perceptual::lab_to_lch;

    #[test]
    fn weights_partition_unity() {
        for i in 0..360 {
            let h = (i as f32).to_radians() - PI;
            let w = band_weights(h);
            let s: f32 = w.iter().sum();
            assert!((s - 1.0).abs() < 1e-4, "{i}: {s}");
        }
        // at a band centre, that band dominates
        let w = band_weights(band_hues()[5]);
        assert!(w[5] > 0.99);
    }

    #[test]
    fn neutral_is_identity() {
        let ops = ColorOps::new(&DevelopSettings::default());
        assert!(ops.is_identity());
        assert_eq!(ops.apply([0.2, 0.3, 0.4], 0.0, 0.0), [0.2, 0.3, 0.4]);
    }

    #[test]
    fn saturation_and_bw() {
        let mut s = DevelopSettings::default();
        s.color.saturation = -100.0;
        let g = ColorOps::new(&s).apply([0.4, 0.1, 0.05], 0.0, 0.0);
        assert!((g[0] - g[1]).abs() < 1e-3 && (g[1] - g[2]).abs() < 1e-3, "{g:?}");
        let mut s = DevelopSettings { treatment: lightcraft_develop::Treatment::Bw, ..Default::default() };
        let ops = ColorOps::new(&s);
        let b = ops.apply([0.05, 0.1, 0.5], 0.0, 0.0);
        assert!((b[0] - b[2]).abs() < 1e-3);
        // raising blue in the B&W mix brightens blue things
        s.bw_mix.blue = 80.0;
        let b2 = ColorOps::new(&s).apply([0.05, 0.1, 0.5], 0.0, 0.0);
        assert!(b2[1] > b[1]);
    }

    #[test]
    fn density_deepens_colour_and_leaves_grey() {
        let lch = |rgb| lab_to_lch(oklab_from_2020(rgb));
        let red = [0.4, 0.05, 0.04];
        let grey = [0.2, 0.2, 0.2];
        let mut s = DevelopSettings::default();
        s.color.density = 100.0;
        let ops = ColorOps::new(&s);
        assert!(!ops.is_identity());
        let (r0, r1) = (lch(red), lch(ops.apply(red, 0.0, 0.0)));
        assert!(r1[0] < r0[0] * 0.8, "deeper: {r0:?} -> {r1:?}");
        assert!((r1[1] - r0[1]).abs() < 1e-3 && (r1[2] - r0[2]).abs() < 1e-3, "chroma and hue kept: {r0:?} -> {r1:?}");
        let g = ops.apply(grey, 0.0, 0.0);
        assert!(g.iter().zip(grey).all(|(a, b)| (a - b).abs() < 1e-4), "{g:?}");
        // (the mixer's path gives the same)
        s.mixer.green.sat = 10.0;
        let m = lch(ColorOps::new(&s).apply(red, 0.0, 0.0));
        assert!((m[0] - r1[0]).abs() < 2e-3, "{m:?} {r1:?}");
        // below 0, thinner: lighter and paler
        s.mixer.green.sat = 0.0;
        s.color.density = -100.0;
        let t = lch(ColorOps::new(&s).apply(red, 0.0, 0.0));
        assert!(t[0] > r0[0] && t[1] < r0[1], "{r0:?} -> {t:?}");
        // and at 0 the JSON is as it was before Density
        assert!(DevelopSettings::default().to_json()["color"].get("density").is_none());
    }

    #[test]
    fn mixer_targets_its_band() {
        let mut s = DevelopSettings::default();
        s.mixer.blue.sat = -100.0;
        let ops = ColorOps::new(&s);
        let blue = [0.02, 0.05, 0.4];
        let red = [0.4, 0.03, 0.02];
        let cb = lab_to_lch(oklab_from_2020(ops.apply(blue, 0.0, 0.0)))[1];
        let cr0 = lab_to_lch(oklab_from_2020(red))[1];
        let cr = lab_to_lch(oklab_from_2020(ops.apply(red, 0.0, 0.0)))[1];
        assert!(cb < 0.02, "{cb}");
        assert!((cr - cr0).abs() < 0.01);
    }

    #[test]
    fn mixer_works_on_the_encoded_value_like_lightroom() {
        // a sky: paler blue sits between the aqua and blue bands
        let sky = [0.12, 0.28, 0.55];
        let y = lightcraft_color::luminance_2020;
        let enc = |c: [f32; 3]| c.map(|v| v.powf(1.0 / MIXER_GAMMA));
        // lowering Saturation moves towards a grey between the brightest channel and the luminance:
        // the sky gets lighter, its blue channel a little darker (Lightroom: V × 0.93, L* + 3.7 at −50)
        let mut s = DevelopSettings::default();
        s.mixer.blue.sat = -60.0;
        s.mixer.aqua.sat = -60.0;
        let out = ColorOps::new(&s).apply(sky, 0.0, 0.0);
        assert!(enc(out)[2] < enc(sky)[2] - 0.02 && enc(out)[2] > enc(sky)[2] * 0.85, "{out:?}");
        assert!(y(out) > y(sky) * 1.05, "{} -> {}", y(sky), y(out));
        // Hue towards aqua keeps the value and lowers the OkLCh hue (blue → cyan)
        let mut s = DevelopSettings::default();
        s.mixer.blue.hue = -60.0;
        s.mixer.aqua.hue = -60.0;
        let out = ColorOps::new(&s).apply(sky, 0.0, 0.0);
        let value = |c: [f32; 3]| enc(c).into_iter().fold(0.0, f32::max);
        assert!((value(out) - value(sky)).abs() < 0.01, "{out:?}");
        let h = |c: [f32; 3]| lab_to_lch(oklab_from_2020(c))[2];
        assert!(h(out) < h(sky) - 0.1, "{} -> {}", h(sky), h(out));
        // Luminance darkens a sky and deepens its blue (Lightroom at Blue −50: V × 0.76, S × 1.9)
        let mut s = DevelopSettings::default();
        s.mixer.blue.lum = -50.0;
        s.mixer.aqua.lum = -50.0;
        let out = ColorOps::new(&s).apply(sky, 0.0, 0.0);
        assert!(y(out) < y(sky) * 0.8);
        let sat = |c: [f32; 3]| rgb_to_hsv(enc(c))[1];
        assert!(sat(out) > sat(sky) * 1.2, "{} -> {}", sat(sky), sat(out));
    }

    #[test]
    fn blue_hue_keeps_the_sky_light_orange_lightens() {
        let y = lightcraft_color::luminance_2020;
        let shift = |c: [f32; 3], band: fn(&mut DevelopSettings) -> &mut f64| {
            let mut s = DevelopSettings::default();
            *band(&mut s) = 50.0;
            ColorOps::new(&s).apply(c, 0.0, 0.0)
        };
        // Blue towards purple: half the luminance a plain HSV rotation loses comes back
        let sky: [f32; 3] = [0.1, 0.25, 0.65];
        let plain = {
            let enc = sky.map(|v| v.powf(1.0 / MIXER_GAMMA));
            let [h, s, v] = rgb_to_hsv(enc);
            let w = band_weights(lab_to_lch(oklab_from_2020(sky))[2])[5];
            let c = lab_to_lch(oklab_from_2020(sky))[1];
            hsv_to_rgb(h + (0.5 * MIXER_HUE[5] * w * (c / 0.12).min(1.0)).to_degrees(), s, v).map(|x| x.powf(MIXER_GAMMA))
        };
        let out = shift(sky, |s| &mut s.mixer.blue.hue);
        assert!(y(plain) < y(sky) * 0.95, "the plain rotation darkens: {} -> {}", y(sky), y(plain));
        assert!((y(out) - y(sky)).abs() < (y(plain) - y(sky)).abs() * 0.7, "{} -> {} (plain {})", y(sky), y(out), y(plain));
        // Orange towards yellow lightens, as plain HSV does
        let brick = [0.45, 0.16, 0.06];
        assert!(y(shift(brick, |s| &mut s.mixer.orange.hue)) > y(brick) * 1.03);
    }

    #[test]
    fn mixer_stays_finite_and_bounded_on_hdr_values() {
        // Luminance's reshaping works under the brightest channel, so values over 1 don't run away
        for rgb in [[4.0, 2.0, 1.0], [0.0, 0.0, 6.0], [1e-9, 0.0, 0.0], [0.0; 3]] {
            for (dh, ds, dl, dp, keep) in [(1.3, -1.0, -0.7, -1.2, 0.5), (-1.3, 1.0, 0.7, 1.2, 0.0), (0.6, 0.0, 0.0, 3.0, 1.0)] {
                let out = mix_hsv(rgb, dh, ds, dl, dp, keep);
                assert!(out.iter().all(|v| v.is_finite() && *v >= 0.0), "{rgb:?} {dh} {ds} {dl} {dp} -> {out:?}");
                // (raising Saturation may lift the brightest channel: its grey sits below it)
                let gain = (1.0f32 + dl).max(0.0).powf(MIXER_GAMMA);
                let top = rgb[0].max(rgb[1]).max(rgb[2]) * gain * 1.001 + 1e-6;
                assert!(ds > 0.0 || out.iter().all(|v| *v <= top), "{rgb:?} {dh} {ds} {dl} {dp} -> {out:?}");
            }
        }
    }

    /// What Lightroom's Calibration does to colours a primary does not hold: Blue Saturation raises a yellow's chroma
    /// without turning it, Red Hue turns an orange towards yellow.
    #[test]
    fn calibration_saturates_and_turns_the_colours_a_primary_doesnt_hold_like_lightroom() {
        let yellow = [0.45, 0.38, 0.06];
        let lch = |c: [f32; 3]| lab_to_lch(oklab_from_2020(c));
        let with = |cal: Calibration| lch(calibrate(yellow, calibration_matrix(&cal).as_ref(), 0.0));
        let [_, c0, h0] = lch(yellow);
        let [_, c1, h1] = with(Calibration { blue_sat: 50.0, ..Default::default() });
        assert!(c1 > c0 * 1.03 && (h1 - h0).abs() < 0.03, "Blue Saturation +50: chroma {c0} -> {c1}, hue {h0} -> {h1}");
        // (Lightroom's Red Hue +50 turns oranges by 13 to 18 degrees and yellows by 7 to 9; greenish yellows go the other way)
        let orange = [0.5, 0.25, 0.08];
        let turned =
            lch(calibrate(orange, calibration_matrix(&Calibration { red_hue: 50.0, ..Default::default() }).as_ref(), 0.0))[2] - lch(orange)[2];
        assert!(turned > 0.08, "Red Hue +50 turns an orange by {turned}");
    }

    #[test]
    fn calibration_keeps_white_and_moves_primaries() {
        assert!(calibration_matrix(&Calibration::default()).is_none());
        let cal = Calibration { red_hue: 60.0, blue_sat: -50.0, ..Default::default() };
        let m = calibration_matrix(&cal).unwrap();
        let grey = calibrate([0.3, 0.3, 0.3], Some(&m), 0.0);
        assert!(grey.iter().all(|v| (v - 0.3).abs() < 1e-4), "{grey:?}");
        // red rotates in hue, blue loses chroma
        let red = [0.4, 0.05, 0.03];
        let h0 = lab_to_lch(oklab_from_2020(red))[2];
        let h1 = lab_to_lch(oklab_from_2020(calibrate(red, Some(&m), 0.0)))[2];
        assert!(h1 - h0 > 0.05, "{h0} -> {h1}");
        let blue = [0.03, 0.05, 0.4];
        let c0 = lab_to_lch(oklab_from_2020(blue))[1];
        let c1 = lab_to_lch(oklab_from_2020(calibrate(blue, Some(&m), 0.0)))[1];
        assert!(c1 < c0 * 0.97, "{c0} -> {c1}");
    }

    #[test]
    fn shadow_tint_targets_shadows() {
        let dark = calibrate([0.01, 0.01, 0.01], None, 1.0);
        let bright = calibrate([1.5, 1.5, 1.5], None, 1.0);
        assert!(dark[1] < dark[0] * 0.85, "magenta shadows: {dark:?}");
        assert!((bright[1] - bright[0]).abs() < 1e-3, "{bright:?}");
        let y = |c: [f32; 3]| lightcraft_color::luminance_2020(c);
        assert!((y(dark) - 0.01).abs() < 1e-5);
        let green = calibrate([0.01, 0.01, 0.01], None, -1.0);
        assert!(green[1] > green[0]);
    }

    #[test]
    fn shadow_tint_is_as_strong_as_lightroom_and_symmetric() {
        // Lightroom's +-50 on seven cameras' raws moves a* in the dark tones by 2 to 11 (4 to 5 on the middle ones), the
        // same both ways, most in the darkest tones and less and less up to the highlights (cards #386 / #388). The
        // first tint (linear, 3 % of green here) was under all of them; the one fitted to an EOS R6 alone (0.45 over
        // -2..2 stops) was two to five times over five of the others
        let ratio = |grey: f32, t: f32| {
            let c = calibrate([grey; 3], None, t);
            c[1] / c[0]
        };
        // (a grey almost two stops under mid grey: a* +4.5 is green down by about 8 %)
        assert!(ratio(0.05, 0.5) < 0.95 && ratio(0.05, 0.5) > 0.88, "magenta at +50: {}", ratio(0.05, 0.5));
        assert!((ratio(0.05, 0.5) * ratio(0.05, -0.5) - 1.0).abs() < 1e-3, "{} {}", ratio(0.05, 0.5), ratio(0.05, -0.5));
        // strongest in the darkest tones, then mid grey, then the highlights
        let (dark, mid, light) = (ratio(0.005, 0.5), ratio(0.18, 0.5), ratio(0.6, 0.5));
        assert!(dark < ratio(0.05, 0.5) && ratio(0.05, 0.5) < mid && mid < light && light < 1.0, "{dark} {mid} {light}");
        assert!(mid < 0.98, "mid grey: {mid}");
        let bright = calibrate([1.5, 1.5, 1.5], None, 1.0);
        assert!((bright[1] / bright[0] - 1.0).abs() < 1e-3, "{bright:?}");
    }

    #[test]
    fn point_color_targets_its_range() {
        let skin = [0.35, 0.16, 0.09];
        let [l, c, h] = lab_to_lch(oklab_from_2020(skin));
        let sample =
            PointColor { lum: l as f64, chroma: c as f64, hue: (h as f64).to_degrees(), hue_shift: 60.0, sat_shift: -50.0, ..Default::default() };
        let mut s = DevelopSettings::default();
        s.point_colors.push(sample);
        let ops = ColorOps::new(&s);
        let out = lab_to_lch(oklab_from_2020(ops.apply(skin, 0.0, 0.0)));
        assert!(out[1] < c * 0.9, "chroma {c} -> {}", out[1]);
        assert!(wrap(out[2] - h) > 0.2, "hue {h} -> {}", out[2]);
        // far colours are untouched
        for far in [[0.02, 0.05, 0.4], [0.05, 0.3, 0.05], [0.3, 0.3, 0.3]] {
            let o = ops.apply(far, 0.0, 0.0);
            assert!(far.iter().zip(&o).all(|(a, b)| (a - b).abs() < 1e-4), "{far:?} -> {o:?}");
        }
        // weight: 1 at the sample, falling to 0 outside the range; a wider range reaches further
        let k = PointK::new(&sample);
        assert!((k.weight(l, c, h) - 1.0).abs() < 1e-6);
        assert_eq!(k.weight(l, c, h + 1.0), 0.0);
        let wide = PointK::new(&PointColor { range: 100.0, ..sample });
        assert!(wide.weight(l, c + 0.1, h) > k.weight(l, c + 0.1, h));
    }

    /// Lightroom's exports of one sample on six cameras (card #536): what each shift does to a fully selected colour, and
    /// how little Range widens the hues a sample takes.
    #[test]
    fn point_color_shifts_and_range_follow_lightroom_s() {
        let sample = PointColor { lum: 0.6, chroma: 0.06, hue: 240.0, ..Default::default() };
        let at = [0.6f32, 0.06, 240f32.to_radians()];
        let shifted = |p: PointColor| PointK::new(&p).apply(at);
        // Hue +-100 turns it by about 45 degrees (it was 29)
        let turned = shifted(PointColor { hue_shift: 100.0, ..sample })[2] - at[2];
        assert!((0.7..0.85).contains(&turned), "{turned}");
        // Luminance scales the whole colour: lightness and chroma by the same factor (it added a fixed step to lightness)
        for (v, k) in [(100.0, 0.2f32.exp()), (-100.0, (-0.2f32).exp())] {
            let o = shifted(PointColor { lum_shift: v, ..sample });
            assert!((o[0] / at[0] - k).abs() < 1e-4 && (o[1] / at[1] - k).abs() < 1e-4, "{v}: {o:?}");
        }
        // Saturation -100 leaves about two thirds of the chroma (it left none), +100 not quite doubles it
        let down = shifted(PointColor { sat_shift: -100.0, ..sample })[1] / at[1];
        let up = shifted(PointColor { sat_shift: 100.0, ..sample })[1] / at[1];
        assert!((0.6..0.7).contains(&down) && (1.8..2.0).contains(&up), "{down} {up}");
        // Range 0 still takes the sample's hue 20 degrees to each side; Range 100 only a little more; never a colour 30 degrees away
        let (narrow, wide) = (PointK::new(&PointColor { range: 0.0, ..sample }), PointK::new(&PointColor { range: 100.0, ..sample }));
        let off = |deg: f32| at[2] + deg.to_radians();
        assert!(narrow.weight(at[0], at[1], off(15.0)) > 0.3 && narrow.weight(at[0], at[1], off(25.0)) == 0.0);
        assert!(wide.weight(at[0], at[1], off(15.0)) > narrow.weight(at[0], at[1], off(15.0)) && wide.weight(at[0], at[1], off(30.0)) == 0.0);
        // what Range widens is the chroma and the lightness a sample takes
        assert!(narrow.weight(at[0] + 0.2, at[1], at[2]) == 0.0 && wide.weight(at[0] + 0.2, at[1], at[2]) > 0.5);
        assert!(narrow.weight(at[0], at[1] + 0.06, at[2]) == 0.0 && wide.weight(at[0], at[1] + 0.06, at[2]) > 0.3);
    }

    #[test]
    fn point_color_variance_compresses_towards_the_sample() {
        let p = PointColor { lum: 0.6, chroma: 0.1, hue: 40.0, variance: -100.0, range: 100.0, ..Default::default() };
        let k = PointK::new(&p);
        let (h0, near) = (40f32.to_radians(), 40f32.to_radians() + 0.1);
        let o = k.apply([0.62, 0.11, near]);
        assert!((o[2] - h0).abs() < 0.1 && (o[1] - 0.1).abs() < 0.01 && (o[0] - 0.6).abs() < 0.02, "{o:?}");
        let k = PointK::new(&PointColor { variance: 100.0, ..p });
        let o = k.apply([0.62, 0.11, near]);
        assert!(o[2] - h0 > 0.1, "{o:?}");
    }

    #[test]
    fn grading_tints_shadows_only() {
        let mut s = DevelopSettings::default();
        s.grading.shadows = lightcraft_develop::Wheel { hue: 220.0, sat: 60.0, lum: 0.0 };
        let ops = ColorOps::new(&s);
        let dark = ops.apply([0.01, 0.01, 0.01], 0.0, 0.0);
        let bright = ops.apply([0.8, 0.8, 0.8], 0.0, 0.0);
        assert!(dark[2] > dark[0]);
        assert!((bright[2] - bright[0]).abs() < 0.02);
    }

    /// CIELAB (D65) a and b of a linear Rec. 2020 colour, as the Lightroom exports were measured.
    fn cielab_ab(c: [f32; 3]) -> (f64, f64) {
        let xyz = REC2020.to_xyz().apply(c.map(f64::from));
        let f = |t: f64| if t > 0.008856 { t.cbrt() } else { 7.787 * t + 16.0 / 116.0 };
        let (fx, fy, fz) = (f(xyz[0] / 0.9505), f(xyz[1]), f(xyz[2] / 1.089));
        (500.0 * (fx - fy), 200.0 * (fy - fz))
    }

    /// A grey of CIE lightness `l` (0..100), linear.
    fn grey(l: f32) -> [f32; 3] {
        [((l + 16.0) / 116.0).powi(3); 3]
    }

    /// The chroma (CIELAB) a grading gives a grey of lightness `l`.
    fn tint_of(s: &DevelopSettings, l: f32) -> f64 {
        let (a, b) = cielab_ab(ColorOps::new(s).apply(grey(l), 0.0, 0.0));
        a.hypot(b)
    }

    fn graded(set: impl Fn(&mut DevelopSettings)) -> DevelopSettings {
        let mut s = DevelopSettings::default();
        set(&mut s);
        s
    }

    /// The direction of a wheel's tint on a mid grey, measured on Lightroom's exports of six cameras' raws (the CIELAB
    /// hue of the change on near-neutral pixels): a wheel's hue is an HSV hue on ProPhoto's primaries. sRGB's hues,
    /// which the wheels used before, land 10 to 30 degrees away (200 at 236, 220 at 271, 40 at 91).
    #[test]
    fn grading_wheel_hues_tint_where_lightroom_s_do() {
        for (hue, measured) in [(200.0, 210.8), (120.0, 153.9), (300.0, 334.4), (0.0, 12.4), (220.0, 244.0), (50.0, 83.0), (40.0, 64.8f64)] {
            let s = graded(|s| s.grading.global = lightcraft_develop::Wheel { hue, sat: 30.0, lum: 0.0 });
            let (a, b) = cielab_ab(ColorOps::new(&s).apply(grey(50.0), 0.0, 0.0));
            let angle = b.atan2(a).to_degrees().rem_euclid(360.0);
            let off = (angle - measured + 180.0).rem_euclid(360.0) - 180.0;
            assert!(off.abs() < 6.0, "wheel hue {hue}: tint at {angle:.1}, Lightroom's at {measured}");
        }
    }

    /// Where and how strongly the wheels act, against Lightroom's exports of six cameras' raws (chroma of the change
    /// on near-neutral pixels per lightness band, pooled over the six).
    #[test]
    fn grading_ranges_follow_lightroom_s() {
        use lightcraft_develop::Wheel;
        // the global wheel (hue 200, saturation 30): 9.7 in the mid tones, 2.6 near black and 2.9 near white
        let global = graded(|s| s.grading.global = Wheel { hue: 200.0, sat: 30.0, lum: 0.0 });
        let (dark, mid, bright) = (tint_of(&global, 5.0), tint_of(&global, 55.0), tint_of(&global, 95.0));
        assert!((7.5..12.0).contains(&mid), "global, mid tones: {mid}");
        assert!(dark > 0.1 * mid && dark < 0.6 * mid && bright > 0.1 * mid && bright < 0.6 * mid, "global: {dark} {mid} {bright}");
        // the shadows wheel (hue 220, saturation 60) at lightness 55: 17.5 at Blending 100, 5.6 at 50, 1.4 at 0
        let shadows = |blending: f64, balance: f64| {
            tint_of(
                &graded(|s| {
                    s.grading.shadows = Wheel { hue: 220.0, sat: 60.0, lum: 0.0 };
                    s.grading.blending = blending;
                    s.grading.balance = balance;
                }),
                55.0,
            )
        };
        let (b0, b50, b100) = (shadows(0.0, 0.0), shadows(50.0, 0.0), shadows(100.0, 0.0));
        assert!((12.0..23.0).contains(&b100), "shadows, Blending 100: {b100}");
        assert!(b50 > 0.2 * b100 && b50 < 0.45 * b100, "shadows, Blending 50 against 100: {b50} {b100}");
        assert!(b0 < 0.3 * b100 && b0 < 0.75 * b50, "shadows, Blending 0 against 50 and 100: {b0} {b50} {b100}");
        // Balance +50 halves it there (8.8 against 17.5)
        let balanced = shadows(100.0, 50.0);
        assert!(balanced > 0.3 * b100 && balanced < 0.7 * b100, "shadows, Balance +50: {balanced} {b100}");
        // the highlights wheel (hue 50, saturation 60) at lightness 75: 21.1 at Blending 100, 13.9 at 50; 2.1 at lightness 35
        let highlights = |blending: f64, l: f32| {
            tint_of(
                &graded(|s| {
                    s.grading.highlights = Wheel { hue: 50.0, sat: 60.0, lum: 0.0 };
                    s.grading.blending = blending;
                }),
                l,
            )
        };
        assert!((15.0..27.0).contains(&highlights(100.0, 75.0)), "highlights, Blending 100: {}", highlights(100.0, 75.0));
        let ratio = highlights(50.0, 75.0) / highlights(100.0, 75.0);
        assert!((0.45..0.85).contains(&ratio), "highlights, Blending 50 against 100: {ratio}");
        assert!(highlights(50.0, 35.0) < 0.25 * highlights(50.0, 75.0), "highlights reach the dark tones: {}", highlights(50.0, 35.0));
        // the midtones wheel (hue 40, saturation 40): 9.9 at lightness 55, 2.9 at 25, 1.6 at 85
        let midtones = graded(|s| s.grading.midtones = Wheel { hue: 40.0, sat: 40.0, lum: 0.0 });
        let (low, mid, high) = (tint_of(&midtones, 25.0), tint_of(&midtones, 55.0), tint_of(&midtones, 85.0));
        assert!((7.0..13.0).contains(&mid) && low < 0.5 * mid && high < 0.4 * mid, "midtones: {low} {mid} {high}");
    }

    /// Lightroom's Vibrance on six cameras' raws: −50 leaves every colour about half its chroma, weak or strong; +50
    /// adds 30 to 45 % to greens and blues and 10 to 13 % to reds and oranges.
    #[test]
    fn vibrance_follows_lightroom_s() {
        let chroma = |c: [f32; 3]| lab_to_lch(oklab_from_2020(c))[1];
        let with = |v: f64, c: [f32; 3]| {
            let mut s = DevelopSettings::default();
            s.color.vibrance = v;
            chroma(ColorOps::new(&s).apply(c, 0.0, 0.0)) / chroma(c)
        };
        let (weak_green, strong_green, sky, skin) = ([0.2, 0.26, 0.17], [0.1, 0.4, 0.08], [0.15, 0.25, 0.42], [0.42, 0.26, 0.18]);
        for c in [weak_green, strong_green, sky, skin] {
            let k = with(-50.0, c);
            assert!((0.42..0.6).contains(&k), "Vibrance -50 on {c:?}: chroma x {k}");
        }
        for c in [weak_green, sky] {
            let k = with(50.0, c);
            assert!((1.22..1.5).contains(&k), "Vibrance +50 on {c:?}: chroma x {k}");
        }
        let k = with(50.0, skin);
        assert!((1.03..1.22).contains(&k), "Vibrance +50 on skin: chroma x {k}");
        // a strong colour still gains (Lightroom: 1.30 at CIELAB chroma over 30)
        assert!(with(50.0, strong_green) > 1.12, "{}", with(50.0, strong_green));
    }

    /// Lightroom's Calibration on six cameras' raws, as the matrix fitted to each export on ProPhoto's primaries: a
    /// primary's Saturation leaves its own channel alone and moves the two others away from it (Red +50: green by
    /// 0.18, blue by 0.15 of their distance to red), so every colour gains chroma; Blue Hue +50 turns yellows by 20
    /// to 27 degrees.
    #[test]
    fn calibration_mixes_channels_on_prophoto_s_primaries_like_lightroom() {
        let (to_wide, _, _) = crate::finish::curve_space();
        let wide = |c: [f32; 3]| -> [f32; 3] { std::array::from_fn(|r| dot3(to_wide[r], c)) };
        let c = [0.2, 0.3, 0.12];
        let m = calibration_matrix(&Calibration { red_sat: 50.0, ..Default::default() }).unwrap();
        let (before, after) = (wide(c), wide(calibrate(c, Some(&m), 0.0)));
        assert!((after[0] - before[0]).abs() < 1e-4, "red's own channel: {before:?} -> {after:?}");
        let (green, blue) = ((after[1] - before[1]) / (before[1] - before[0]), (after[2] - before[2]) / (before[2] - before[0]));
        assert!((0.13..0.24).contains(&green) && (0.1..0.22).contains(&blue), "green by {green}, blue by {blue}");
        // every Saturation slider at +50 raises the chroma of a colour that is none of the primaries by 15 to 30 %
        let lch = |c: [f32; 3]| lab_to_lch(oklab_from_2020(c));
        let orange = [0.4, 0.25, 0.1];
        for cal in [
            Calibration { red_sat: 50.0, ..Default::default() },
            Calibration { green_sat: 50.0, ..Default::default() },
            Calibration { blue_sat: 50.0, ..Default::default() },
        ] {
            let k = lch(calibrate(orange, calibration_matrix(&cal).as_ref(), 0.0))[1] / lch(orange)[1];
            assert!((1.08..1.4).contains(&k), "{cal:?}: chroma x {k}");
        }
        // and -50 is +50's mirror image
        let up = calibration_matrix(&Calibration { green_hue: 50.0, ..Default::default() }).unwrap();
        let down = calibration_matrix(&Calibration { green_hue: -50.0, ..Default::default() }).unwrap();
        for (a, b) in
            up.iter().flatten().zip(down.iter().flatten()).zip([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0f32]).map(|((a, b), i)| (a - i, b - i))
        {
            assert!((a + b).abs() < 1e-5, "{up:?} {down:?}");
        }
        let yellow = [0.45, 0.38, 0.06];
        let turned =
            lch(calibrate(yellow, calibration_matrix(&Calibration { blue_hue: 50.0, ..Default::default() }).as_ref(), 0.0))[2] - lch(yellow)[2];
        assert!(turned > 0.15, "Blue Hue +50 turns a yellow by {turned}");
    }
}
