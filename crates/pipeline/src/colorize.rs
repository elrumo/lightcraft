//! Colorize ([`lightcraft_develop::Colorize`]): the colours a model gave a black-and-white photo, laid over the
//! white-balanced picture in the scene-linear stage.
//!
//! The model (DDColor) answers CIE Lab a and b for the photo's lightness. Each pixel keeps its own luminance and is
//! multiplied by the colour that (L, a, b) is, divided by its luminance: a grey pixel becomes that colour, while a white
//! balance change, a print's tone or a colour already there scales with it, and every colour control after this stage
//! acts on the result as on any photo. L is the pixel's own (its luminance with white at 1, before exposure). The native
//! Apple renderer implements the same formula (`ColorizeKernel` in the app), so its constants are public.
//!
//! A model lays some colour on everything, and a tint that is nothing in a mid tone is plain to see on a white wall, a
//! cloud or a deep shadow, where a photograph has almost none: [`tone_keep`] holds the model's colour back toward white
//! and toward black (Neutral Highlights, Neutral Shadows), which is most of what makes a coloured photo look painted.
//!
//! ponytail: the map is read bilinear, scaled up from at most 512 pixels; guided upsampling (Lens Blur's filter) if
//! colour is seen to bleed over edges.

use lightcraft_develop::DevelopSettings;
use lightcraft_raster::{Plane, Rgb32f};

use crate::for_rows;
use crate::geometry::Frame;
use crate::masks::source_to_out;

/// The largest factor a channel is multiplied by (deep shadows of a saturated colour ask for more).
pub const MAX_GAIN: f32 = 8.0;
/// Neutral Highlights holds colour back from this lightness (CIE L) up to white (100), by a smooth step…
pub const HIGHLIGHTS_FROM: f32 = 70.0;
/// … and Neutral Shadows from black (0) up to this lightness.
pub const SHADOWS_TO: f32 = 25.0;

/// How much of the model's colour a tone of lightness `l` (CIE L, 0..100) keeps: all of it in the middle, `1 − highlights`
/// of it at white and `1 − shadows` of it at black (both 0..1), along smooth steps ([`HIGHLIGHTS_FROM`], [`SHADOWS_TO`]).
pub fn tone_keep(l: f32, highlights: f32, shadows: f32) -> f32 {
    let smooth = |e0: f32, e1: f32, x: f32| {
        let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    if !l.is_finite() {
        return 1.0;
    }
    (1.0 - highlights * smooth(HIGHLIGHTS_FROM, 100.0, l)) * (1.0 - shadows * (1.0 - smooth(0.0, SHADOWS_TO, l)))
}

/// Whether `s` colours anything (settings alone: whether its map decodes is [`Setup::new`]'s question).
pub fn active(s: &DevelopSettings) -> bool {
    s.colorize.active() && s.section_enabled("colorize")
}

/// What a render's Colorize does: the model's a and b over the stored source, and how much of them.
#[derive(Clone, Debug)]
pub struct Setup {
    a: Plane,
    b: Plane,
    amount: f32,
    /// Neutral Highlights and Neutral Shadows, 0..1 ([`tone_keep`]).
    highlights: f32,
    shadows: f32,
    /// The colours go over the photo's black-and-white version ([`lightcraft_develop::ColorizeBase::Bw`]).
    mono: bool,
    /// Changes with the map and the sliders (the stage caches' key).
    pub key: u64,
}

impl Setup {
    /// `None` when nothing is coloured, or the stored map is damaged.
    pub fn new(s: &DevelopSettings) -> Option<Setup> {
        if !active(s) {
            return None;
        }
        let m = s.colorize.map.as_ref()?;
        let ab = m.ab()?;
        let (w, h) = (m.w as usize, m.h as usize);
        let plane = |k: usize| Plane { width: w, height: h, data: ab.iter().map(|p| p[k]).collect() };
        let unit = |v: f64| if v.is_finite() { (v / 100.0).clamp(0.0, 1.0) as f32 } else { 0.0 };
        let (amount, highlights, shadows) = (unit(s.colorize.amount), unit(s.colorize.highlights), unit(s.colorize.shadows));
        let mono = s.colorize.base == lightcraft_develop::ColorizeBase::Bw;
        let key = crate::hash_of((&m.data, m.w, m.h, [amount, highlights, shadows].map(f32::to_bits), mono));
        Some(Setup { a: plane(0), b: plane(1), amount, highlights, shadows, mono, key })
    }
}

/// Colour `img` (scene-linear, white-balanced, the output of `frame`) as `setup` says, in place.
pub fn apply(img: &mut Rgb32f, setup: &Setup, frame: &Frame) {
    let (w, h) = (img.width, img.height);
    if w == 0 || h == 0 {
        return;
    }
    let (a, b) = (source_to_out(&setup.a, frame, w, h), source_to_out(&setup.b, frame, w, h));
    let conv = Conv::new();
    let k = setup.amount;
    for_rows(&mut img.data, w, |y, row| {
        for (x, c) in row.iter_mut().enumerate() {
            let i = y * w + x;
            let (pa, pb) = (a.data.get(i).copied().unwrap_or(0.0), b.data.get(i).copied().unwrap_or(0.0));
            let lum = conv.luminance(*c);
            if setup.mono && lum.is_finite() {
                // (a scan's tint comes off, by Amount, before the model's colours go on)
                *c = c.map(|v| v + (lum - v) * k);
            }
            let keep = tone_keep(lightness(lum), setup.highlights, setup.shadows);
            let g = conv.gain(lum, pa * keep, pb * keep);
            *c = std::array::from_fn(|n| c[n] * (1.0 + (g[n] - 1.0) * k));
        }
    });
}

/// CIE Lab (D65) to the working space (linear Rec. 2020), and its luminance.
pub struct Conv {
    xyz_to_working: [[f32; 3]; 3],
    lum: [f32; 3],
    white: [f32; 3],
}

impl Default for Conv {
    fn default() -> Self {
        Self::new()
    }
}

impl Conv {
    pub fn new() -> Conv {
        let to_xyz = lightcraft_color::REC2020.to_xyz().to_f32();
        let white = lightcraft_color::D65.to_xyz().map(|v| v as f32);
        Conv { xyz_to_working: lightcraft_color::REC2020.from_xyz().to_f32(), lum: to_xyz[1], white }
    }

    /// The luminance (Y, white 1) of working-space `c`.
    pub fn luminance(&self, c: [f32; 3]) -> f32 {
        self.lum[0] * c[0] + self.lum[1] * c[1] + self.lum[2] * c[2]
    }

    /// The factors that turn a grey of luminance `y` into the colour Lab (L of `y`, `a`, `b`): that colour over its
    /// luminance, out-of-gamut parts at 0, each at most [`MAX_GAIN`]. No colour (1, 1, 1) for a black, an unknown
    /// luminance, or a colour with no light left.
    pub fn gain(&self, y: f32, a: f32, b: f32) -> [f32; 3] {
        if !(y.is_finite() && y > 0.0 && a.is_finite() && b.is_finite()) {
            return [1.0; 3];
        }
        let y = y.min(1.0);
        let fy = lab_f(y);
        let xyz = [self.white[0] * lab_f_inv(fy + a / 500.0), y, self.white[2] * lab_f_inv(fy - b / 200.0)];
        let m = &self.xyz_to_working;
        let rgb: [f32; 3] = std::array::from_fn(|r| (m[r][0] * xyz[0] + m[r][1] * xyz[1] + m[r][2] * xyz[2]).max(0.0));
        let l = self.luminance(rgb);
        if !(l.is_finite() && l > 1e-9) {
            return [1.0; 3];
        }
        rgb.map(|v| (v / l).min(MAX_GAIN))
    }
}

const EPSILON: f32 = 216.0 / 24389.0;
const KAPPA: f32 = 24389.0 / 27.0;

/// CIE L (0..100) of luminance `y` (white 1; brighter is 100, nothing or not a number is 0).
pub fn lightness(y: f32) -> f32 {
    if y.is_finite() && y > 0.0 { 116.0 * lab_f(y.min(1.0)) - 16.0 } else { 0.0 }
}

/// CIE Lab's f(t) for a ratio to white `t`.
fn lab_f(t: f32) -> f32 {
    if t > EPSILON { t.cbrt() } else { (KAPPA * t + 16.0) / 116.0 }
}

fn lab_f_inv(f: f32) -> f32 {
    let t = f * f * f;
    if t > EPSILON { t } else { (116.0 * f - 16.0) / KAPPA }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightcraft_develop::ChromaMap;

    fn settings(map: Option<ChromaMap>, amount: f64) -> DevelopSettings {
        let mut s = DevelopSettings::default();
        s.colorize.enabled = true;
        s.colorize.amount = amount;
        s.colorize.highlights = 0.0;
        s.colorize.shadows = 0.0;
        s.colorize.map = map;
        s
    }

    #[test]
    fn no_colour_is_no_change_and_a_grey_takes_the_colour_at_its_own_lightness() {
        let c = Conv::new();
        for y in [0.01, 0.18, 0.5, 1.0, 3.0] {
            let g = c.gain(y, 0.0, 0.0);
            assert!(g.iter().all(|v| (v - 1.0).abs() < 1e-3), "{y} {g:?}");
        }
        // a warm colour: red over blue, and the luminance is kept
        let g = c.gain(0.18, 20.0, 40.0);
        assert!(g[0] > 1.1 && g[2] < 0.9, "{g:?}");
        let out = g.map(|v| v * 0.18);
        assert!((c.luminance(out) - 0.18).abs() < 1e-3, "{out:?}");
        // nonsense in: nothing changes
        assert_eq!(c.gain(f32::NAN, 20.0, 40.0), [1.0; 3]);
        assert_eq!(c.gain(0.0, 20.0, 40.0), [1.0; 3]);
        assert_eq!(c.gain(0.18, f32::INFINITY, 0.0), [1.0; 3]);
        assert!(c.gain(1e-6, 127.0, -127.0).iter().all(|v| (0.0..=MAX_GAIN).contains(v)));
    }

    #[test]
    fn whites_and_blacks_keep_less_of_the_colour_than_mid_tones() {
        // the middle keeps all of it whatever the sliders say; the ends keep what the sliders leave
        assert_eq!(tone_keep(50.0, 1.0, 1.0), 1.0);
        assert_eq!(tone_keep(100.0, 0.0, 0.0), 1.0);
        assert!((tone_keep(100.0, 0.7, 0.0) - 0.3).abs() < 1e-6);
        assert!((tone_keep(0.0, 0.0, 0.4) - 0.6).abs() < 1e-6);
        assert!(tone_keep(85.0, 1.0, 0.0) > 0.4 && tone_keep(85.0, 1.0, 0.0) < 0.6);
        assert_eq!(tone_keep(f32::NAN, 1.0, 1.0), 1.0);
        assert!((lightness(0.18) - 49.5).abs() < 0.1 && lightness(1.0) == 100.0 && lightness(4.0) == 100.0 && lightness(-1.0) == 0.0);
        // in a render: a near-white and a near-black pixel take less colour with the sliders up, a mid tone the same
        let map = ChromaMap::from_ab(2, 2, &[[20.0, 40.0]; 4]);
        let mut held = settings(map.clone(), 100.0);
        held.colorize.highlights = 100.0;
        held.colorize.shadows = 100.0;
        let (plain, held) = (Setup::new(&settings(map, 100.0)).unwrap(), Setup::new(&held).unwrap());
        assert_ne!(plain.key, held.key);
        let frame = Frame::new(3, 1, &DevelopSettings::default(), true);
        let tones = [0.9, 0.18, 0.004];
        let run = |setup: &Setup| {
            let mut img = Rgb32f::new(3, 1);
            for (c, t) in img.data.iter_mut().zip(tones) {
                *c = [t; 3];
            }
            apply(&mut img, setup, &frame);
            img.data
        };
        let spread = |c: [f32; 3]| (c[0] - c[2]).abs() / c[1].max(1e-6);
        let (a, b) = (run(&plain), run(&held));
        assert!(spread(b[0]) < 0.5 * spread(a[0]), "white {:?} {:?}", a[0], b[0]);
        assert!((spread(b[1]) - spread(a[1])).abs() < 1e-3, "mid {:?} {:?}", a[1], b[1]);
        assert!(spread(b[2]) < 0.2 * spread(a[2]), "black {:?} {:?}", a[2], b[2]);
    }

    #[test]
    fn amount_mixes_from_the_photo_to_the_models_colour_at_any_size() {
        let map = ChromaMap::from_ab(4, 3, &[[20.0, 40.0]; 12]);
        assert!(Setup::new(&settings(None, 100.0)).is_none());
        assert!(Setup::new(&settings(map.clone(), 0.0)).is_none());
        let mut off = settings(map.clone(), 100.0);
        off.colorize.enabled = false;
        assert!(Setup::new(&off).is_none());
        let full = Setup::new(&settings(map.clone(), 100.0)).unwrap();
        let half = Setup::new(&settings(map, 50.0)).unwrap();
        assert_ne!(full.key, half.key);
        let want = Conv::new().gain(0.18, 20.0, 40.0).map(|v| v * 0.18);
        for (w, h) in [(5, 4), (60, 45)] {
            let frame = Frame::new(w, h, &DevelopSettings::default(), true);
            let mut img = Rgb32f::new(w, h);
            img.data.iter_mut().for_each(|c| *c = [0.18; 3]);
            let mut mixed = img.clone();
            apply(&mut img, &full, &frame);
            apply(&mut mixed, &half, &frame);
            for (c, m) in img.data.iter().zip(&mixed.data) {
                assert!(c.iter().zip(&want).all(|(p, q)| (p - q).abs() < 1e-3), "{w}x{h} {c:?} {want:?}");
                assert!(m.iter().zip(&want).all(|(p, q)| (p - (0.18 + (q - 0.18) * 0.5)).abs() < 1e-3), "{m:?}");
            }
        }
    }
}
