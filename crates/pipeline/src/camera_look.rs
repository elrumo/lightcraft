//! The Camera Matching look ([`CameraMatch`]) on single pixels: what the engine puts on a raw's developed source
//! when the photo has the Camera Matching profile (`lightcraft_engine`'s `camera_matched` calls [`MatchLook::apply_image`]),
//! and what a render puts on a generated fill stored in the photo's plain light (Generative Expand at the resample, AI
//! Remove at the retouching stage), so the fill gets the look exactly as the photo beside it did.

use lightcraft_color::{D50, D65, Mat3, PROPHOTO, REC2020, bradford, luminance_2020};
use lightcraft_raster::Rgb32f;
use lightcraft_raw::profile::HsvTable;

use crate::CameraMatch;

/// Linear Rec.2020 to linear ProPhoto RGB (D50), where DNG hue/saturation tables apply.
pub fn to_prophoto() -> Mat3 {
    PROPHOTO.from_xyz().mul(&bradford(D65, D50)).mul(&REC2020.to_xyz())
}

/// [`HueSat::apply`] restores the luminance in full only if the table kept at least this share of it. A colour
/// the table darkens to [`NO_LUMINANCE_KEPT`] or less (only near-black noise, whose channels go negative) keeps
/// its colour; in between the two are blended, so noisy shadows show no step.
pub const MIN_LUMINANCE_KEPT: f32 = 0.25;

pub const NO_LUMINANCE_KEPT: f32 = 0.15;

/// How much of the table's colour a pixel gets when the table kept `share` of its luminance: 0 up to
/// [`NO_LUMINANCE_KEPT`], 1 from [`MIN_LUMINANCE_KEPT`], a smoothstep between.
pub fn restore_weight(share: f32) -> f32 {
    if !share.is_finite() {
        return 0.0;
    }
    let t = ((share - NO_LUMINANCE_KEPT) / (MIN_LUMINANCE_KEPT - NO_LUMINANCE_KEPT)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A fitted camera look's hue/saturation table ([`CameraMatch::hue_sat`]), ready to apply to linear Rec.2020 pixels. It changes
/// hue and saturation only: luminance is restored, the camera tone curve owns it.
pub struct HueSat<'a> {
    table: &'a HsvTable,
    pub to: [[f32; 3]; 3],
    pub from: [[f32; 3]; 3],
}

impl<'a> HueSat<'a> {
    pub fn new(table: &'a HsvTable) -> Option<HueSat<'a>> {
        let to = to_prophoto();
        Some(HueSat { table, to: to.to_f32(), from: to.inverse()?.to_f32() })
    }

    #[inline]
    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        let mul = |m: &[[f32; 3]; 3], v: [f32; 3]| -> [f32; 3] { std::array::from_fn(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2]) };
        let out = mul(&self.from, self.table.apply(mul(&self.to, rgb)));
        let (before, after) = (luminance_2020(rgb), luminance_2020(out));
        // restoring the luminance divides by the table's: near black, where noise makes some channels
        // negative, the table can bring it close to 0, and the gain would turn the pixel white (#820)
        if before <= 0.0 || after <= 0.0 || !out.iter().all(|v| v.is_finite()) {
            return rgb;
        }
        let w = restore_weight(after / before);
        if w >= 1.0 {
            return out.map(|v| v * before / after);
        }
        if w <= 0.0 {
            return rgb;
        }
        std::array::from_fn(|i| rgb[i] + (out[i] * before / after - rgb[i]) * w)
    }

    /// The colour that [`Self::apply`] turns into `rgb`: the table undone in its own terms ([`HsvTable::undo`]: hue
    /// and saturation), at the brightness that gives `rgb`'s luminance (found in a few steps: the table's value axis
    /// reads it). Where the table clipped saturation, the least saturated colour it clipped; colours whose luminance
    /// the table moved too far to be restored in full are not quite undone.
    pub fn undo(&self, rgb: [f32; 3]) -> [f32; 3] {
        let mul = |m: &[[f32; 3]; 3], v: [f32; 3]| -> [f32; 3] { std::array::from_fn(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2]) };
        let lum = luminance_2020(rgb);
        if !(lum > 0.0 && rgb.iter().all(|v| v.is_finite())) {
            return rgb;
        }
        let out = mul(&self.to, rgb);
        let (mut value, mut x) = (out[0].max(out[1]).max(out[2]), rgb);
        for _ in 0..UNDO_STEPS {
            let c = mul(&self.from, self.table.undo(out, value));
            let k = lum / luminance_2020(c);
            if !(k > 0.0 && k.is_finite() && c.iter().all(|v| v.is_finite())) {
                break;
            }
            (x, value) = (c.map(|v| v * k), value * k);
        }
        x
    }

    /// [`Self::apply`] to `N` pixels at once (`rgb[channel][lane]`), bit for bit, through
    /// [`HsvTable::apply_lanes`], with the guards as selects.
    #[inline]
    pub fn apply_lanes<const N: usize>(&self, rgb: [[f32; N]; 3]) -> [[f32; N]; 3] {
        let mul = |m: &[[f32; 3]; 3], v: &[[f32; N]; 3]| {
            let mut out = [[0.0f32; N]; 3];
            for (row, out) in m.iter().zip(&mut out) {
                for l in 0..N {
                    out[l] = row[0] * v[0][l] + row[1] * v[1][l] + row[2] * v[2][l];
                }
            }
            out
        };
        let out = mul(&self.from, &self.table.apply_lanes(mul(&self.to, &rgb)));
        let mut restored = rgb;
        for l in 0..N {
            let (input, out) = ([rgb[0][l], rgb[1][l], rgb[2][l]], [out[0][l], out[1][l], out[2][l]]);
            let (before, after) = (luminance_2020(input), luminance_2020(out));
            let untouched = (before <= 0.0) | (after <= 0.0) | !(out[0].is_finite() & out[1].is_finite() & out[2].is_finite());
            let w = restore_weight(after / before);
            for (c, restored) in restored.iter_mut().enumerate() {
                let full = out[c] * before / after;
                let blend = input[c] + (full - input[c]) * w;
                // (`w >= 1` and `w <= 0` exclude each other)
                restored[l] = if untouched | (w <= 0.0) {
                    input[c]
                } else if w >= 1.0 {
                    full
                } else {
                    blend
                };
            }
        }
        restored
    }
}

/// Brightness steps of [`HueSat::undo`].
const UNDO_STEPS: usize = 3;

/// A [`CameraMatch`] ready to put on pixels (its tables' matrices worked out once).
pub struct MatchLook<'a> {
    start: Option<HueSat<'a>>,
    matrix: [[f32; 3]; 3],
    hue_sat: Option<HueSat<'a>>,
}

impl CameraMatch {
    /// The look ready to put on pixels.
    pub fn look(&self) -> MatchLook<'_> {
        MatchLook {
            start: self.start_hue_sat.as_ref().and_then(HueSat::new),
            matrix: self.matrix.to_f32(),
            hue_sat: self.hue_sat.as_ref().and_then(HueSat::new),
        }
    }
}

impl MatchLook<'_> {
    /// The look on one linear Rec.2020 pixel: the starting look's table undone, the matrix, the camera look's table, a
    /// floor at 0. The same bits as [`Self::apply_image`] gives the pixel.
    #[inline]
    pub fn apply(&self, p: [f32; 3]) -> [f32; 3] {
        let p = self.start.as_ref().map_or(p, |s| s.undo(p));
        let m = &self.matrix;
        let rgb: [f32; 3] = std::array::from_fn(|i| m[i][0] * p[0] + m[i][1] * p[1] + m[i][2] * p[2]);
        self.hue_sat.as_ref().map_or(rgb, |h| h.apply(rgb)).map(|v| v.max(0.0))
    }

    /// [`Self::apply`] on every pixel of `img`, the camera table `LANES` pixels at a time so it vectorises (same bits).
    pub fn apply_image(&self, img: &mut Rgb32f) {
        if let Some(start) = &self.start {
            lightcraft_raster::par_rows(&mut img.data, img.width, |_, row| row.iter_mut().for_each(|p| *p = start.undo(*p)));
        }
        let m = self.matrix;
        let Some(hue_sat) = &self.hue_sat else {
            img.map_in_place(|c| std::array::from_fn(|i| m[i][0] * c[0] + m[i][1] * c[1] + m[i][2] * c[2]).map(|v: f32| v.max(0.0)));
            return;
        };
        const LANES: usize = 16;
        lightcraft_raster::par_rows(&mut img.data, img.width, |_, row| {
            for chunk in row.chunks_mut(LANES) {
                let mut rgb = [[0.0f32; LANES]; 3];
                for (l, c) in chunk.iter().enumerate() {
                    for (i, rgb) in rgb.iter_mut().enumerate() {
                        rgb[l] = m[i][0] * c[0] + m[i][1] * c[1] + m[i][2] * c[2];
                    }
                }
                let out = hue_sat.apply_lanes(rgb);
                for (l, p) in chunk.iter_mut().enumerate() {
                    *p = [out[0][l], out[1][l], out[2][l]].map(|v| v.max(0.0));
                }
            }
        });
    }
}
