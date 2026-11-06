//! The global tone map: scene luminance → display-linear luminance.
//!
//! Built in the log domain around middle grey (0.18): contrast scales log-exposure about grey,
//! whites move the shoulder (white point), blacks move the toe. The shoulder is an extended
//! Reinhard curve so highlights roll off smoothly instead of clipping.
//!
//! Rendered (display-referred) sources such as JPEGs use [`ToneMap::display`] instead: identity at
//! neutral settings (an unedited JPEG renders exactly as the file), with contrast/whites/blacks as
//! shifts in a gamma-2.2 perceptual domain fitted to Lightroom's response and a short shoulder above 0.95.

pub const GREY: f32 = 0.18;
/// The tone LUT spans `LUT_MIN_EV..LUT_MAX_EV` around grey in `LUT_N` steps.
pub const LUT_MIN_EV: f32 = -14.0;
pub const LUT_MAX_EV: f32 = 10.0;
pub const LUT_N: usize = 4096;
/// Nodes of a camera chroma curve, evenly spaced over display luminance 0..=1.
pub const CHROMA_N: usize = 8;
const NO_CHROMA: [f32; CHROMA_N] = [1.0; CHROMA_N];

/// A file-local camera look, fitted independently of the scene-linear colour transform.
/// Knots are scene/display-linear luminance pairs. Keeping this in the finish stage preserves
/// RAW exposure and highlight headroom; it is never baked into the decoded sensor pixels.
/// `chroma` scales colourfulness by display luminance after the curve (a camera's per-channel
/// curve saturates shadows and bleaches highlights toward white, which a luminance curve can't).
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct CameraTone {
    knots: [[f32; 2]; 32],
    chroma: [f32; CHROMA_N],
}

impl<'de> serde::Deserialize<'de> for CameraTone {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
        struct Wire {
            knots: [[f32; 2]; 32],
            // absent in smart previews written before the chroma curve existed
            #[serde(default)]
            chroma: Option<[f32; CHROMA_N]>,
        }
        let w = Wire::deserialize(d)?;
        let tone = Self::new(w.knots).ok_or_else(|| serde::de::Error::custom("invalid camera tone curve"))?;
        match w.chroma {
            Some(c) => tone.with_chroma(c).ok_or_else(|| serde::de::Error::custom("invalid camera chroma curve")),
            None => Ok(tone),
        }
    }
}

impl CameraTone {
    pub fn new(knots: [[f32; 2]; 32]) -> Option<Self> {
        let mut previous = [0.0, 0.0];
        for p in knots {
            if !p.iter().all(|v| v.is_finite()) || p[0] <= previous[0] || p[1] < previous[1] || p[1] >= 1.0 {
                return None;
            }
            previous = p;
        }
        Some(Self { knots, chroma: NO_CHROMA })
    }

    /// The curve with chroma scales at display luminance 0, 1/7 … 1 (each finite, 0..=4).
    pub fn with_chroma(self, chroma: [f32; CHROMA_N]) -> Option<Self> {
        chroma.iter().all(|k| k.is_finite() && (0.0..=4.0).contains(k)).then_some(Self { chroma, ..self })
    }

    pub fn chroma(&self) -> &[f32; CHROMA_N] {
        &self.chroma
    }

    /// The curve with the chroma scales it would show applied to each RGB channel, as a DNG
    /// `ProfileToneCurve` is ([`channel_chroma`]), instead of to luminance alone.
    pub fn with_channel_chroma(self) -> Self {
        let chroma = channel_chroma(|y| self.apply(y));
        Self { chroma, ..self }
    }

    /// The (scene, display) luminance knots, in increasing order.
    pub fn knots(&self) -> &[[f32; 2]; 32] {
        &self.knots
    }

    pub fn apply(&self, y: f32) -> f32 {
        if !y.is_finite() || y <= 0.0 {
            return 0.0;
        }
        let mut previous = [0.0, 0.0];
        for p in self.knots {
            if y <= p[0] {
                let t = (y - previous[0]) / (p[0] - previous[0]);
                return previous[1] + t * (p[1] - previous[1]);
            }
            previous = p;
        }
        let a = self.knots[30];
        let b = self.knots[31];
        // Extend beyond observed (unclipped) highlights with a continuous, bounded shoulder.
        let slope = ((b[1] - a[1]) / (b[0] - a[0])).clamp(0.1, 16.0);
        1.0 - (1.0 - b[1]) * (-(y - b[0]) * slope / (1.0 - b[1]).max(0.01)).exp()
    }
}

/// Chroma scales (at display luminance `CHROMA_N` nodes, see [`ToneMap::chroma_scale`]) that make the
/// luminance curve `f` (scene → display linear, increasing) colour a picture as `f` applied to each RGB
/// channel would. The finish stage scales RGB by `f(y) / y`, which keeps linear channel ratios; a
/// per-channel curve spreads channels by its slope in log-log terms instead: more colour where it's
/// steeper than linear (midtones of a contrast curve), less where it compresses (highlights). Measured
/// on an iPhone 17 Pro ProRAW, the luminance-only render had 74% of the colourfulness of Apple's own.
/// The slope is a finite difference over ±20% (a saturated channel spread), not a derivative.
pub fn channel_chroma(f: impl Fn(f32) -> f32) -> [f32; CHROMA_N] {
    const S: f32 = 0.2;
    let (lo, hi) = (GREY * 2f32.powf(LUT_MIN_EV), GREY * 2f32.powf(LUT_MAX_EV));
    std::array::from_fn(|i| {
        // nodes at display 0 and 1 are reached only in the limit: take them just inside
        let o = (i as f32 / (CHROMA_N - 1) as f32).clamp(0.02, 0.98);
        // scene luminance with f(x) = o, by bisection in log scene luminance
        let (mut a, mut b) = (lo.ln(), hi.ln());
        for _ in 0..48 {
            let m = 0.5 * (a + b);
            if f(m.exp()) < o { a = m } else { b = m }
        }
        let x = (0.5 * (a + b)).exp();
        let y = f(x);
        let k = (f(x * (1.0 + S)) - f(x * (1.0 - S))) / (2.0 * S * y);
        if k.is_finite() { k.clamp(0.0, 4.0) } else { 1.0 }
    })
}

/// Scene EV over grey where [`ToneMap::hdr`] starts lifting highlights above the SDR curve.
const HDR_FROM_EV: f32 = 1.5;

/// Knots of a display-tone shift table, at p = 0, 0.05, …, 1 (`p` = display luminance^(1/2.2)).
const SHIFT_N: usize = 21;
/// How one of Contrast, Whites and Blacks moves `p` in [`ToneMap::display`]: for an amount
/// `a` = |slider| / 100 the shift is `A(p)·a + B(p)·a²`, `[[A, B]; 2]` for a positive then a
/// negative slider. Fitted to Lightroom's exports of one raw at ±54 and ±100 (its display
/// luminance before and after each slider alone, by median per tone band), so it is exact at
/// those amounts and smooth between them; `A` keeps `B`'s direction, so a small amount never
/// moves the wrong way.
type Shift = [[[f32; SHIFT_N]; 2]; 2];

const CONTRAST: Shift = [
    [
        [
            0.0000, -0.0198, -0.0418, -0.0659, -0.0807, -0.0911, -0.0971, -0.0997, -0.0967, -0.0849, -0.0659, -0.0407, -0.0102, 0.0206, 0.0441,
            0.0616, 0.0684, 0.0654, 0.0554, 0.0315, 0.0000,
        ],
        [
            0.0000, -0.0033, -0.0069, -0.0108, -0.0125, -0.0116, -0.0091, -0.0052, -0.0024, -0.0015, -0.0005, 0.0003, 0.0003, -0.0002, -0.0008,
            -0.0011, -0.0002, -0.0003, -0.0022, -0.0020, 0.0000,
        ],
    ],
    [
        [
            0.0000, 0.0186, 0.0392, 0.0622, 0.0764, 0.0854, 0.0897, 0.0893, 0.0841, 0.0730, 0.0552, 0.0325, 0.0070, -0.0209, -0.0420, -0.0566,
            -0.0632, -0.0596, -0.0491, -0.0291, 0.0000,
        ],
        [
            0.0000, 0.0039, 0.0078, 0.0087, 0.0065, 0.0036, -0.0001, -0.0040, -0.0074, -0.0093, -0.0083, -0.0052, -0.0011, 0.0050, 0.0078, 0.0069,
            0.0034, -0.0026, -0.0060, -0.0044, 0.0000,
        ],
    ],
];
const WHITES: Shift = [
    [
        [
            0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0155, 0.0487, 0.0854,
            0.1063, 0.1092, 0.0915, 0.0000,
        ],
        [
            0.0000, 0.0017, 0.0046, 0.0145, 0.0297, 0.0525, 0.0797, 0.1049, 0.1277, 0.1523, 0.1793, 0.2013, 0.2146, 0.2180, 0.1971, 0.1464, 0.0856,
            0.0332, -0.0097, -0.0434, 0.0000,
        ],
    ],
    [
        [
            0.0000, -0.0004, -0.0011, -0.0036, -0.0074, -0.0132, -0.0210, -0.0304, -0.0396, -0.0484, -0.0578, -0.0692, -0.0832, -0.0973, -0.1078,
            -0.1137, -0.1157, -0.1172, -0.1063, -0.0637, -0.0071,
        ],
        [
            0.0000, 0.0003, 0.0007, 0.0023, 0.0047, 0.0082, 0.0129, 0.0184, 0.0236, 0.0283, 0.0330, 0.0379, 0.0439, 0.0472, 0.0472, 0.0447, 0.0395,
            0.0306, 0.0117, -0.0234, -0.0590,
        ],
    ],
];
const BLACKS: Shift = [
    [
        [
            0.0000, 0.0128, 0.0274, 0.0481, 0.0640, 0.0771, 0.0912, 0.1002, 0.1001, 0.0908, 0.0816, 0.0765, 0.0721, 0.0612, 0.0487, 0.0360, 0.0244,
            0.0170, 0.0113, 0.0040, 0.0000,
        ],
        [
            0.0000, 0.0031, 0.0067, 0.0100, 0.0110, 0.0117, 0.0066, -0.0012, -0.0068, -0.0065, -0.0051, -0.0064, -0.0098, -0.0109, -0.0106, -0.0078,
            -0.0040, -0.0035, -0.0038, -0.0018, 0.0000,
        ],
    ],
    [
        [
            0.0000, -0.0864, -0.1527, -0.0831, -0.0102, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000,
            0.0000, 0.0000, 0.0000, 0.0000, 0.0000,
        ],
        [
            0.0000, 0.0444, 0.0634, -0.0673, -0.1833, -0.2117, -0.2159, -0.2155, -0.2100, -0.1983, -0.1784, -0.1535, -0.1311, -0.1117, -0.0919,
            -0.0700, -0.0511, -0.0346, -0.0203, -0.0066, 0.0000,
        ],
    ],
];

/// The shift of `p` that `slider` (−100..100) of `table` makes.
fn shift(table: &Shift, slider: f32, p: f32) -> f32 {
    let a = (slider.abs() / 100.0).min(1.0);
    if a == 0.0 || !a.is_finite() {
        return 0.0;
    }
    let [ka, kb] = if slider > 0.0 { &table[0] } else { &table[1] };
    knots(ka, p) * a + knots(kb, p) * a * a
}

/// A shift table at `p` (a Catmull-Rom spline through the knots, so the tone curve has no kinks).
fn knots(k: &[f32; SHIFT_N], p: f32) -> f32 {
    let x = p.clamp(0.0, 1.0) * (SHIFT_N - 1) as f32;
    let i = (x as usize).min(SHIFT_N - 2);
    let t = x - i as f32;
    let at = |j: usize| k.get(j.min(SHIFT_N - 1)).copied().unwrap_or(0.0);
    let (p0, p1, p2, p3) = (at(i.saturating_sub(1)), at(i), at(i + 1), at(i + 2));
    0.5 * (2.0 * p1 + (p2 - p0) * t + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t * t + (3.0 * p1 - p0 - 3.0 * p2 + p3) * t * t * t)
}

/// Spread (EV each side) of the log-log slope that scales colour with an edited curve ([`edit_chroma`]).
const SLOPE_SPREAD_EV: f32 = 0.5;
/// How strongly colour follows that slope, and the most it may scale it either way (fitted to Lightroom's
/// single-slider exports: `docs/lightroom-light-sliders.md`).
const SLOPE_CHROMA_POWER: f32 = 0.85;
const SLOPE_CHROMA_MAX: f32 = 2.0;

/// Chroma scales at the tone LUT's entries (scene EV): `base` at the unedited curve's display luminance, times how much
/// steeper in log-log terms the edited curve `lut` is than the unedited `neutral` over ±0.5 EV there, to the power 0.85,
/// within 0.5..2. A curve applied to each RGB channel spreads them by its slope, and Lightroom's sliders colour a
/// picture that way (Contrast +100 about 1.5 times the chroma in the mid tones, Blacks −100 about twice), where a
/// luminance curve keeps the channels' ratios. An unedited curve gives `base` alone.
fn edit_chroma(lut: &[f32], neutral: &[f32], base: impl Fn(f32) -> f32) -> Vec<f32> {
    let h = (SLOPE_SPREAD_EV * (LUT_N - 1) as f32 / (LUT_MAX_EV - LUT_MIN_EV)).round() as usize;
    let ln = |v: &[f32], i: usize| v.get(i).copied().unwrap_or(0.0).max(1e-7).ln();
    let edited = lut != neutral;
    (0..LUT_N)
        .map(|i| {
            let k = base(neutral.get(i).copied().unwrap_or(0.0));
            if !edited {
                return k;
            }
            let (a, b) = (i.saturating_sub(h), (i + h).min(LUT_N - 1));
            let (se, sn) = (ln(lut, b) - ln(lut, a), ln(neutral, b) - ln(neutral, a));
            let f = if sn > 1e-6 && se.is_finite() {
                (se.max(0.0) / sn).powf(SLOPE_CHROMA_POWER).clamp(1.0 / SLOPE_CHROMA_MAX, SLOPE_CHROMA_MAX)
            } else {
                1.0
            };
            k * f
        })
        .collect()
}

/// The part of an edit whose slope colours the picture ([`edit_chroma`]): Contrast, Whites that lift and Blacks that
/// crush. Lightroom keeps the colour where Blacks lift or Whites lower the curve (its exports change chroma by under
/// 5 % there), so those leave it alone; `None` when nothing is left.
fn colour_edit(contrast: f64, whites: f64, blacks: f64) -> Option<(f64, f64, f64)> {
    let e = (contrast, whites.max(0.0), blacks.min(0.0));
    (e != (0.0, 0.0, 0.0)).then_some(e)
}

/// A camera chroma curve's scale at display luminance `o` (linear between its `CHROMA_N` nodes).
fn chroma_node(chroma: &[f32; CHROMA_N], o: f32) -> f32 {
    if !o.is_finite() {
        return 1.0;
    }
    let f = o.clamp(0.0, 1.0) * (CHROMA_N - 1) as f32;
    let i = (f as usize).min(CHROMA_N - 2);
    let t = f - i as f32;
    let (a, b) = (chroma.get(i).copied().unwrap_or(1.0), chroma.get(i + 1).copied().unwrap_or(1.0));
    if a == b { a } else { a + (b - a) * t }
}

#[derive(Clone, Debug)]
pub struct ToneMap {
    lut: Vec<f32>,
    /// The camera look's chroma curve by display luminance ([`ToneMap::chroma_scale`], for fitting looks).
    chroma: [f32; CHROMA_N],
    /// What a render scales chroma by at each of `lut`'s entries ([`ToneMap::chroma_at`], [`edit_chroma`]).
    chroma_ev: Vec<f32>,
}

impl ToneMap {
    pub fn camera(curve: &CameraTone, contrast: f64, whites: f64, blacks: f64) -> ToneMap {
        ToneMap::camera_hdr(curve, contrast, whites, blacks, 1.0)
    }

    /// [`ToneMap::camera`] for an HDR render reaching `peak` (linear, SDR white = 1; 1 = SDR
    /// exactly): the camera's look up to 1.5 EV over grey, so midtones match the SDR render (and a
    /// gain map only lifts highlights), then the highlights gain up to `peak − 1` more as in
    /// [`ToneMap::hdr`].
    pub fn camera_hdr(curve: &CameraTone, contrast: f64, whites: f64, blacks: f64, peak: f32) -> ToneMap {
        let peak = if peak.is_finite() { peak.max(1.0) } else { 1.0 };
        let adjustment = Self::display(contrast, whites, blacks);
        let neutral = contrast == 0.0 && whites == 0.0 && blacks == 0.0;
        let top = (2.9 - 1.6 * (whites as f32 / 100.0) + peak.log2() + 1.0).max(HDR_FROM_EV + 0.5);
        let table = |adjust: Option<&ToneMap>| -> Vec<f32> {
            (0..LUT_N)
                .map(|i| {
                    let ev = LUT_MIN_EV + (LUT_MAX_EV - LUT_MIN_EV) * i as f32 / (LUT_N - 1) as f32;
                    let y = curve.apply(GREY * 2f32.powf(ev));
                    let o = adjust.map_or(y, |a| a.apply(y));
                    if peak > 1.0 { (o + (peak - 1.0) * smooth(HDR_FROM_EV, top, ev)).clamp(0.0, peak) } else { o }
                })
                .collect()
        };
        let lut = table((!neutral).then_some(&adjustment));
        // (the unedited curve and the colour edit's as tone maps of their own: the HDR lift's top moves with Whites)
        let flat = if neutral { lut.clone() } else { Self::camera_hdr(curve, 0.0, 0.0, 0.0, peak).lut };
        let colour = match colour_edit(contrast, whites, blacks) {
            Some(e) if e == (contrast, whites, blacks) => lut.clone(),
            Some((c, w, b)) => Self::camera_hdr(curve, c, w, b, peak).lut,
            None => flat.clone(),
        };
        let chroma_ev = edit_chroma(&colour, &flat, |o| chroma_node(&curve.chroma, o));
        ToneMap { lut, chroma: curve.chroma, chroma_ev }
    }
    /// `contrast`, `whites`, `blacks` in −100..100 (Lightroom slider units).
    pub fn new(contrast: f64, whites: f64, blacks: f64) -> ToneMap {
        ToneMap::hdr(contrast, whites, blacks, 1.0)
    }

    /// [`ToneMap::new`] for an HDR render reaching `peak` (linear, SDR white = 1; 1 = SDR exactly).
    /// Below about half of SDR white the curve is the SDR one; above, the highlights gain up to
    /// `peak − 1` more along a smoothstep in scene EV, from 1.5 EV over grey to one stop past the
    /// white point moved up by the headroom, so the result stays monotone, never exceeds `peak`,
    /// and reaches it there (within the table's range).
    pub fn hdr(contrast: f64, whites: f64, blacks: f64, peak: f32) -> ToneMap {
        let peak = if peak.is_finite() { peak.max(1.0) } else { 1.0 };
        let c = (contrast / 100.0) as f32;
        let slope = if c >= 0.0 { 1.0 + 0.55 * c } else { 1.0 + 0.4 * c };
        // White point: scene luminance (after contrast) that maps to display 1.0.
        let white_ev = 2.9 - 1.6 * (whites as f32 / 100.0);
        let wl = GREY * 2f32.powf(white_ev);
        let pre = 1.0 + GREY / wl; // keep grey near grey
        let b = (blacks / 100.0) as f32;
        let lut: Vec<f32> = (0..LUT_N)
            .map(|i| {
                let ev = LUT_MIN_EV + (LUT_MAX_EV - LUT_MIN_EV) * i as f32 / (LUT_N - 1) as f32;
                let y = GREY * 2f32.powf(ev * slope) * pre;
                // extended Reinhard with white point wl: y(1 + y/wl²)/(1 + y)
                let mut o = y * (1.0 + y / (wl * wl)) / (1.0 + y);
                o = o.min(1.0);
                // Toe: blacks < 0 crushes, > 0 lifts.
                if b < 0.0 {
                    // Smooth max(0, o − k) (a soft knee), renormalized so 1 stays 1.
                    let k = -b * 0.035;
                    let e = 0.004;
                    let soft = |v: f32| ((v - k) + ((v - k) * (v - k) + e * e).sqrt()) * 0.5;
                    o = (soft(o) - soft(0.0)) / (soft(1.0) - soft(0.0));
                } else if b > 0.0 {
                    let k = b * 0.03;
                    o = k + (1.0 - k) * o;
                }
                if peak > 1.0 {
                    let top = (white_ev + peak.log2() + 1.0).max(HDR_FROM_EV + 0.5);
                    o += (peak - 1.0) * smooth(HDR_FROM_EV, top, ev * slope);
                }
                o.clamp(0.0, peak)
            })
            .collect();
        let chroma_ev = match colour_edit(contrast, whites, blacks) {
            Some(e) if e == (contrast, whites, blacks) => edit_chroma(&lut, &ToneMap::hdr(0.0, 0.0, 0.0, peak).lut, |_| 1.0),
            Some((c, w, b)) => ToneMap::hdr(c, w, b, peak).chroma_ev,
            None => vec![1.0; LUT_N],
        };
        ToneMap { lut, chroma: NO_CHROMA, chroma_ev }
    }

    /// Tone map for display-referred sources: identity at neutral settings.
    pub fn display(contrast: f64, whites: f64, blacks: f64) -> ToneMap {
        ToneMap::display_hdr(contrast, whites, blacks, 1.0)
    }

    /// [`ToneMap::display`] for an HDR render reaching `peak`: values above SDR white (an HDR
    /// source, e.g. a decoded gain-map image) pass through and roll off softly towards `peak`
    /// instead of clipping at 1 (as far as the edit moves them: neutral sliders keep them as they are, up to
    /// `peak`); SDR sources (≤ 1) render as with [`ToneMap::display`].
    pub fn display_hdr(contrast: f64, whites: f64, blacks: f64, peak: f32) -> ToneMap {
        let peak = if peak.is_finite() { peak.max(1.0) } else { 1.0 };
        let (c, w, b) = (contrast as f32, whites as f32, blacks as f32);
        // Neutral sliders are the identity up to white (issue #1945): the SDR shoulder below would
        // pull 1.0 down to 0.9875 (8-bit 255 to 254), so it only shapes an edited curve.
        let neutral = c == 0.0 && w == 0.0 && b == 0.0;
        // (a running maximum: the summed shifts may dip by a hair where two tables slope against each other)
        let mut floor = 0.0f32;
        let lut: Vec<f32> = (0..LUT_N)
            .map(|i| {
                let ev = LUT_MIN_EV + (LUT_MAX_EV - LUT_MIN_EV) * i as f32 / (LUT_N - 1) as f32;
                let y = GREY * 2f32.powf(ev);
                let mut p = y.powf(1.0 / 2.2);
                if p <= 1.0 {
                    p += shift(&CONTRAST, c, p) + shift(&WHITES, w, p) + shift(&BLACKS, b, p);
                } else {
                    p += w / 100.0 * 0.06;
                }
                let mut o = p.max(0.0).powf(2.2);
                // short shoulder (SDR, edited curves): slope 1 at 0.95, reaching 1.0 at 1.05
                if peak > 1.0 && o > 0.95 && !neutral {
                    // exponential shoulder: slope 1 at 0.95, asymptote at `peak`
                    let r = peak - 0.95;
                    o = 0.95 + r * (1.0 - (-(o - 0.95) / r).exp());
                } else if o > 0.95 && !neutral {
                    let d = (o - 0.95).min(0.1);
                    o = 0.95 + d - d * d / 0.2;
                }
                floor = o.clamp(floor, peak);
                floor
            })
            .collect();
        let chroma_ev = match colour_edit(contrast, whites, blacks) {
            Some(e) if e == (contrast, whites, blacks) => edit_chroma(&lut, &ToneMap::display_hdr(0.0, 0.0, 0.0, peak).lut, |_| 1.0),
            Some((c, w, b)) => ToneMap::display_hdr(c, w, b, peak).chroma_ev,
            None => vec![1.0; LUT_N],
        };
        ToneMap { lut, chroma: NO_CHROMA, chroma_ev }
    }

    /// The table (`LUT_N` entries, see [`ToneMap::apply`]).
    pub fn lut(&self) -> &[f32] {
        &self.lut
    }

    /// The chroma scales a render applies, at the tone table's entries (`LUT_N` of them, see [`ToneMap::chroma_at`]).
    pub fn chroma_lut(&self) -> &[f32] {
        &self.chroma_ev
    }

    /// The camera look's chroma scale at display luminance `o` (exactly 1 everywhere unless a camera look sets it); a
    /// render uses [`ToneMap::chroma_at`], which adds the edit's.
    #[inline]
    pub fn chroma_scale(&self, o: f32) -> f32 {
        chroma_node(&self.chroma, o)
    }

    /// The chroma scale a render applies to a pixel of scene luminance `y` (the tone map's input): the camera look's at
    /// the unedited curve's output, times the edit's slope ([`edit_chroma`]). 1 everywhere for an unedited picture
    /// without a camera look.
    #[inline]
    pub fn chroma_at(&self, y: f32) -> f32 {
        let ev = if y > 0.0 { (y / GREY).log2() } else { LUT_MIN_EV };
        let f = ((ev - LUT_MIN_EV) / (LUT_MAX_EV - LUT_MIN_EV)).clamp(0.0, 1.0) * (LUT_N - 1) as f32;
        let i = (f as usize).min(LUT_N - 2);
        let t = f - i as f32;
        let (a, b) = (self.chroma_ev.get(i).copied().unwrap_or(1.0), self.chroma_ev.get(i + 1).copied().unwrap_or(1.0));
        if a == b { a } else { a + (b - a) * t }
    }

    /// Scene luminance → display-linear luminance.
    #[inline]
    pub fn apply(&self, y: f32) -> f32 {
        if y <= 0.0 {
            return 0.0;
        }
        let ev = (y / GREY).log2();
        let f = ((ev - LUT_MIN_EV) / (LUT_MAX_EV - LUT_MIN_EV)).clamp(0.0, 1.0) * (LUT_N - 1) as f32;
        let i = (f as usize).min(LUT_N - 2);
        let t = f - i as f32;
        let v = self.lut[i] + (self.lut[i + 1] - self.lut[i]) * t;
        if ev < LUT_MIN_EV { v * (y / (GREY * 2f32.powf(LUT_MIN_EV))) } else { v }
    }
}

fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn knots() -> [[f32; 2]; 32] {
        std::array::from_fn(|i| {
            let x = 0.004 * 1.18f32.powi(i as i32);
            [x, 1.0 - (-2.0 * x).exp()]
        })
    }

    #[test]
    fn chroma_curve_is_identity_unless_set_and_survives_serde() {
        let plain = CameraTone::new(knots()).unwrap();
        let map = ToneMap::camera(&plain, 0.0, 0.0, 0.0);
        assert!([0.0, 0.3, 0.77, 1.0, 2.0].iter().all(|o| map.chroma_scale(*o) == 1.0));
        assert!([0.0, 0.5, 1.0].iter().all(|o| ToneMap::new(0.0, 0.0, 0.0).chroma_scale(*o) == 1.0));
        // smart previews written before the chroma curve existed still load (identity)
        let old = serde_json::json!({ "knots": knots() });
        assert_eq!(serde_json::from_value::<CameraTone>(old).unwrap(), plain);
        let tone = plain.with_chroma([1.4, 1.3, 1.1, 1.0, 0.7, 0.4, 0.25, 0.2]).unwrap();
        let back: CameraTone = serde_json::from_value(serde_json::to_value(tone).unwrap()).unwrap();
        assert_eq!(back, tone);
        let map = ToneMap::camera(&tone, 0.0, 0.0, 0.0);
        assert!((map.chroma_scale(0.0) - 1.4).abs() < 1e-6 && (map.chroma_scale(1.0) - 0.2).abs() < 1e-6);
        assert!((map.chroma_scale(0.5 / 7.0) - 1.35).abs() < 1e-5, "interpolates between nodes");
        assert_eq!(map.chroma_scale(f32::NAN), 1.0);
        // hostile values are rejected, also when deserialized
        assert!(plain.with_chroma([f32::NAN; CHROMA_N]).is_none());
        assert!(plain.with_chroma([-1.0; CHROMA_N]).is_none());
        let bad = serde_json::json!({ "knots": knots(), "chroma": [9.0, 1, 1, 1, 1, 1, 1, 1] });
        assert!(serde_json::from_value::<CameraTone>(bad).is_err());
    }

    #[test]
    fn channel_chroma_follows_the_curves_log_log_slope() {
        // linear: per-channel and luminance application agree
        assert!(channel_chroma(|y| 0.7 * y).iter().all(|k| (k - 1.0).abs() < 1e-3));
        // a contrast curve (log-log slope 2·(1 − f)): more colour in the midtones, little near white
        let k = channel_chroma(|y| y * y / (y * y + 0.05));
        assert!(k[2] > 1.2 && k[3] > 1.0, "{k:?}");
        assert!(k[7] < 0.3 && k.windows(2).skip(2).all(|w| w[1] <= w[0] + 1e-4), "{k:?}");
        // a compressive camera curve never adds colour; the scales stay valid for `with_chroma`
        let tone = CameraTone::new(knots()).unwrap().with_channel_chroma();
        assert!(tone.chroma().iter().all(|k| (0.0..=1.0 + 1e-3).contains(k)), "{:?}", tone.chroma());
        assert!(tone.with_chroma(*tone.chroma()).is_some());
        // hostile curves give finite, bounded scales
        assert!(channel_chroma(|_| f32::NAN).iter().chain(&channel_chroma(|_| 0.0)).all(|k| (0.0..=4.0).contains(k)));
    }

    #[test]
    fn camera_curve_preserves_black_and_extends_headroom() {
        let knots = std::array::from_fn(|i| {
            let x = 0.005 * 1.15f32.powi(i as i32);
            [x, 1.0 - (-3.0 * x).exp()]
        });
        let curve = CameraTone::new(knots).unwrap();
        let base = ToneMap::camera(&curve, 0.0, 0.0, 0.0);
        assert_eq!(base.apply(0.0), 0.0);
        assert!(base.apply(0.1) < base.apply(0.2));
        let mut previous = 0.0;
        for i in 0..2000 {
            let y = 1e-6 * 1.01f32.powi(i);
            let v = base.apply(y);
            assert!((0.0..=1.0).contains(&v));
            assert!(v >= previous - 1e-6);
            previous = v;
        }
        assert!((base.apply(0.1) - curve.apply(0.1)).abs() < 0.001);
        assert!(ToneMap::camera(&curve, 50.0, 0.0, 0.0).apply(0.3) > base.apply(0.3));
        let mut invalid = knots;
        invalid[1][0] = invalid[0][0];
        assert!(CameraTone::new(invalid).is_none());
        assert!(serde_json::from_value::<CameraTone>(serde_json::json!({"knots": invalid})).is_err());
    }

    #[test]
    fn monotone_and_bounded() {
        for (c, w, b) in [(0.0, 0.0, 0.0), (100.0, 100.0, -100.0), (-100.0, -100.0, 100.0), (50.0, -30.0, -40.0)] {
            let t = ToneMap::new(c, w, b);
            let mut prev = -1.0;
            for i in 0..2000 {
                let y = 1e-5 * 1.012f32.powi(i);
                let o = t.apply(y);
                assert!((0.0..=1.0).contains(&o));
                assert!(o >= prev - 1e-6, "{c} {w} {b} at {y}: {o} < {prev}");
                prev = o;
            }
        }
    }

    #[test]
    fn hdr_is_sdr_at_peak_one_and_reaches_peak() {
        for (c, w, b) in [(0.0, 0.0, 0.0), (100.0, 100.0, -100.0), (-100.0, -100.0, 100.0), (50.0, -30.0, -40.0)] {
            assert_eq!(ToneMap::hdr(c, w, b, 1.0).lut(), ToneMap::new(c, w, b).lut());
            for peak in [2.0f32, 8.0, 32.0] {
                let sdr = ToneMap::new(c, w, b);
                let t = ToneMap::hdr(c, w, b, peak);
                let mut prev = -1.0;
                for i in 0..2400 {
                    let y = 1e-5 * 1.01f32.powi(i);
                    let o = t.apply(y);
                    assert!((0.0..=peak).contains(&o), "{c} {w} {b} {peak} at {y}: {o}");
                    assert!(o >= prev - 1e-6, "{c} {w} {b} {peak} at {y}: {o} < {prev}");
                    prev = o;
                    if y <= GREY * 2f32.powf(0.9) {
                        assert!((o - sdr.apply(y)).abs() < 1e-6, "midtones differ at {y}");
                    }
                }
                assert!(t.apply(4.0) > sdr.apply(4.0));
                if (c, w, b) == (0.0, 0.0, 0.0) {
                    assert!(t.apply(GREY * 2f32.powf(LUT_MAX_EV)) > peak * 0.99, "{peak}");
                }
            }
        }
        // a broken peak falls back to SDR
        assert_eq!(ToneMap::hdr(0.0, 0.0, 0.0, f32::NAN).lut(), ToneMap::new(0.0, 0.0, 0.0).lut());
    }

    #[test]
    fn camera_hdr_keeps_the_camera_look_and_lifts_highlights() {
        let curve = CameraTone::new(knots()).unwrap().with_chroma([1.2, 1.1, 1.0, 1.0, 0.9, 0.8, 0.7, 0.6]).unwrap();
        for (c, w, b) in [(0.0, 0.0, 0.0), (40.0, -30.0, -20.0)] {
            let sdr = ToneMap::camera(&curve, c, w, b);
            assert_eq!(ToneMap::camera_hdr(&curve, c, w, b, 1.0).lut(), sdr.lut());
            assert_eq!(ToneMap::camera_hdr(&curve, c, w, b, f32::NAN).lut(), sdr.lut());
            for peak in [2.0f32, 8.0] {
                let t = ToneMap::camera_hdr(&curve, c, w, b, peak);
                // the camera look's chroma curve, and what a render applies below the HDR lift (1.5 EV over grey, less
                // the slope's half-stop spread)
                for o in 0..=10 {
                    assert_eq!(t.chroma_scale(o as f32 / 10.0), sdr.chroma_scale(o as f32 / 10.0));
                }
                for i in 0..1000 {
                    let y = GREY * 2f32.powf(-12.0 + 12.9 * i as f32 / 1000.0);
                    assert!((t.chroma_at(y) - sdr.chroma_at(y)).abs() < 1e-5, "{peak} at {y}");
                }
                let mut prev = -1.0;
                for i in 0..2400 {
                    let y = 1e-5 * 1.01f32.powi(i);
                    let o = t.apply(y);
                    assert!((0.0..=peak).contains(&o), "{peak} at {y}: {o}");
                    assert!(o >= prev - 1e-6, "{peak} at {y}: {o} < {prev}");
                    prev = o;
                    // the camera's midtones, not the generic HDR curve's (issue: dark HDR exports)
                    if y <= GREY * 2f32.powf(HDR_FROM_EV - 0.01) {
                        assert!((o - sdr.apply(y)).abs() < 1e-6, "midtones differ at {y}: {o} vs {}", sdr.apply(y));
                    }
                }
                assert!(t.apply(4.0) > 1.0 && t.apply(4.0) > sdr.apply(4.0) + 0.5, "{peak}: highlights lifted");
            }
        }
    }

    #[test]
    fn display_hdr_passes_hdr_sources_and_keeps_sdr() {
        assert_eq!(ToneMap::display_hdr(20.0, 10.0, -5.0, 1.0).lut(), ToneMap::display(20.0, 10.0, -5.0).lut());
        let t = ToneMap::display_hdr(0.0, 0.0, 0.0, 8.0);
        for i in 1..=90 {
            let y = i as f32 / 100.0;
            assert!((t.apply(y) - y).abs() < 2e-3, "{y} -> {}", t.apply(y));
        }
        let mut prev = 0.0;
        for i in 1..2000 {
            let o = t.apply(i as f32 / 100.0);
            assert!(o >= prev - 1e-6 && o <= 8.0);
            prev = o;
        }
        // (neutral sliders leave a source's own highlights as they are, up to the peak)
        for y in [1.0f32, 2.0, 3.0, 5.5, 7.5] {
            assert!((t.apply(y) - y).abs() < 0.02 * y, "{y} -> {}", t.apply(y));
        }
        assert_eq!(t.apply(20.0), 8.0, "past the peak it is held there");
        // an edited curve rolls them off softly towards the peak
        let e = ToneMap::display_hdr(20.0, 10.0, 0.0, 8.0);
        assert!(e.apply(3.0) > 2.0 && e.apply(3.0) < 3.0);
    }

    #[test]
    fn display_identity_and_monotone() {
        let t = ToneMap::display(0.0, 0.0, 0.0);
        // up to and including white (issue #1945: 1.0 came out as 0.9875, 8-bit 255 as 254); the
        // tolerance is the LUT's interpolation next to the clip at 1.0, far below an 8-bit level
        for i in 1..=100 {
            let y = i as f32 / 100.0;
            assert!((t.apply(y) - y).abs() < 1e-3, "{y} -> {}", t.apply(y));
        }
        assert_eq!(t.apply(4.0), 1.0, "neutral values pushed above white clip");
        for (c, w, b) in [(100.0, 100.0, -100.0), (-100.0, -100.0, 100.0), (60.0, -40.0, 30.0)] {
            let t = ToneMap::display(c, w, b);
            let mut prev = -1.0;
            for i in 0..1000 {
                let o = t.apply(i as f32 / 500.0);
                assert!(o >= prev - 1e-5, "{c} {w} {b}");
                prev = o;
            }
        }
        let c = ToneMap::display(60.0, 0.0, 0.0);
        assert!(c.apply(0.05) < 0.05 && c.apply(0.7) > 0.7);
    }

    #[test]
    fn display_sliders_follow_lightrooms_response() {
        // (p = luminance^(1/2.2) before and after each slider alone in Lightroom's exports of the fitted raw)
        let p = |t: &ToneMap, p: f32| t.apply(p.powf(2.2)).powf(1.0 / 2.2);
        for (c, w, b, before, after) in [
            (100.0, 0.0, 0.0, 0.32, 0.21),
            (-100.0, 0.0, 0.0, 0.32, 0.41),
            (0.0, 100.0, 0.0, 0.60, 0.82),
            (0.0, 54.0, 0.0, 0.76, 0.83),
            (0.0, -100.0, 0.0, 0.84, 0.76),
            (0.0, 0.0, 100.0, 0.36, 0.46),
            (0.0, 0.0, -100.0, 0.40, 0.19),
            (0.0, 0.0, -54.0, 0.40, 0.35),
        ] {
            let got = p(&ToneMap::display(c, w, b), before);
            assert!((got - after).abs() < 0.02, "{c} {w} {b}: p {before} -> {got}, Lightroom {after}");
        }
    }

    #[test]
    fn edits_colour_by_their_curves_slope() {
        // (Lightroom's exports of the fitted raw: Contrast +100 about 1.5 times the mid tones' chroma, −100 about 0.6,
        // Blacks −100 about twice; an unedited picture keeps its own)
        let mid = 0.2f32;
        assert!(ToneMap::display(0.0, 0.0, 0.0).chroma_lut().iter().all(|&k| k == 1.0));
        assert!(ToneMap::new(0.0, 0.0, 0.0).chroma_lut().iter().all(|&k| k == 1.0));
        let k = |t: ToneMap| t.chroma_at(t_input(&t, mid));
        let (up, down, crush) = (k(ToneMap::display(100.0, 0.0, 0.0)), k(ToneMap::display(-100.0, 0.0, 0.0)), k(ToneMap::display(0.0, 0.0, -100.0)));
        assert!((1.3..1.8).contains(&up) && (0.5..0.8).contains(&down) && (1.6..=2.0).contains(&crush), "{up} {down} {crush}");
        // a camera look's own scale stays under an unedited curve, and the edit multiplies it
        let curve = CameraTone::new(knots()).unwrap().with_chroma([1.2, 1.1, 1.0, 1.0, 0.9, 0.8, 0.7, 0.6]).unwrap();
        let plain = ToneMap::camera(&curve, 0.0, 0.0, 0.0);
        for y in [0.01f32, 0.18, 1.0] {
            assert!((plain.chroma_at(y) - plain.chroma_scale(curve.apply(y))).abs() < 1e-3, "{y}");
        }
        assert!(ToneMap::camera(&curve, 100.0, 0.0, 0.0).chroma_at(0.05) > 1.2 * plain.chroma_at(0.05));
    }

    /// The scene luminance `t` takes to display luminance `o` (by bisection).
    fn t_input(t: &ToneMap, o: f32) -> f32 {
        let (mut a, mut b) = (1e-6f32.ln(), 100f32.ln());
        for _ in 0..60 {
            let m = 0.5 * (a + b);
            if t.apply(m.exp()) < o { a = m } else { b = m }
        }
        (0.5 * (a + b)).exp()
    }

    #[test]
    fn display_shoulder_stays_on_edited_curves() {
        // The short shoulder is part of the look of edited curves;
        // only neutral settings are the identity.
        for (c, w, b) in [(20.0, 0.0, 0.0), (0.0, 10.0, 0.0), (0.0, 0.0, -5.0)] {
            let t = ToneMap::display(c, w, b);
            let o = t.apply(1.0);
            assert!(o < 0.999 && o > 0.95, "{c} {w} {b}: {o}");
        }
    }

    #[test]
    fn grey_stays_near_grey_and_highlights_roll_off() {
        let t = ToneMap::new(0.0, 0.0, 0.0);
        let g = t.apply(0.18);
        assert!((0.15..0.24).contains(&g), "{g}");
        assert!(t.apply(1.0) < 0.95 && t.apply(1.0) > 0.6);
        assert!(t.apply(8.0) > 0.97);
    }

    #[test]
    fn sliders_move_the_right_way() {
        let base = ToneMap::new(0.0, 0.0, 0.0);
        let contrast = ToneMap::new(60.0, 0.0, 0.0);
        assert!(contrast.apply(0.05) < base.apply(0.05));
        assert!(contrast.apply(0.8) > base.apply(0.8));
        assert!(ToneMap::new(0.0, 60.0, 0.0).apply(0.8) > base.apply(0.8));
        assert!(ToneMap::new(0.0, 0.0, -60.0).apply(0.01) < base.apply(0.01));
        assert!(ToneMap::new(0.0, 0.0, 60.0).apply(0.01) > base.apply(0.01));
    }
}
