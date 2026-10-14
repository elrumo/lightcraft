//! The HDR picture of the loupe (LR-VIEW-HDR-DISPLAY): the float HDR render of an HDR edit, packed
//! for a display that can show the range above SDR white.
//!
//! A frontend whose display can do that asks for it ([`crate::RenderJob::with_hdr_display`]); the
//! render then comes with an [`HdrPicture`] beside its 8-bit picture, which every other view and a
//! display without headroom keep using. The pixels are the same render as the export's HDR output
//! ([`OutputDepth::F32Hdr`](lightcraft_pipeline::OutputDepth::F32Hdr)), so what the loupe shows is
//! what an HDR export holds.

use lightcraft_pipeline::{DeepImage, DeepSamples};

/// Display P3 RGBA as IEEE half floats, encoded with the sRGB curve extended past 1 (that is the
/// *extended Display P3* colour space): SDR white is 1.0, highlights reach the encoding of `peak`.
/// (P3 is what the screens that have HDR headroom show; the encoding is what egui writes to its
/// surface and what CoreGraphics' `extendedDisplayP3` space holds, so no frontend converts.) The bit
/// patterns are the texture's `Rgba16Float` / CoreGraphics' `kCGBitmapFloatComponents` +
/// `kCGBitmapByteOrder16Little` data as they are (every native target is little-endian).
#[derive(Clone, Debug, PartialEq)]
pub struct HdrPicture {
    pub width: usize,
    pub height: usize,
    /// `width * height * 4` half floats: R, G, B (encoded), then 1.0, row by row.
    pub rgba16f: Vec<u16>,
    /// The headroom limit of the edit this shows, relative to SDR white (linear light).
    pub peak: f32,
}

/// The half-float bit patterns of the sRGB encoding of every non-negative half float (0 for the
/// rest): a lookup instead of a `powf` for each of millions of samples.
fn encode_table() -> &'static [u16] {
    static TABLE: std::sync::OnceLock<Vec<u16>> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        (0..=u16::MAX)
            .map(|bits| {
                let v = half::f16::from_bits(bits).to_f32();
                if v.is_finite() && v >= 0.0 { half::f16::from_f32(lightcraft_color::transfer::linear_to_srgb(v)).to_bits() } else { 0 }
            })
            .collect()
    })
}

impl HdrPicture {
    /// The linear float render `deep` ([`OutputDepth::F32Hdr`](lightcraft_pipeline::OutputDepth::F32Hdr)),
    /// every sample held to `0..=peak` and encoded. `None` for any other kind of render, or a buffer
    /// that isn't `width × height` pixels.
    pub fn from_deep(deep: &DeepImage, peak: f32) -> Option<HdrPicture> {
        let DeepSamples::F32(v) = &deep.samples else { return None };
        let pixels = deep.width.checked_mul(deep.height)?;
        if v.len() != pixels.checked_mul(3)? {
            return None;
        }
        // (a non-finite peak is no limit worth a bad picture: 1 keeps SDR white)
        let peak = if peak.is_finite() { peak.max(1.0) } else { 1.0 };
        let one = half::f16::ONE.to_bits();
        let table = encode_table();
        let mut rgba16f = Vec::with_capacity(pixels.saturating_mul(4));
        for c in v.as_chunks::<3>().0 {
            // (`max` takes the other side of a NaN: black, never a NaN pixel on screen, which
            // `clamp` would pass on; the linear half float indexes the table of its encoding)
            #[allow(clippy::manual_clamp)]
            rgba16f.extend(c.map(|x| table.get(usize::from(half::f16::from_f32(x.max(0.0).min(peak)).to_bits())).copied().unwrap_or(0)));
            rgba16f.push(one);
        }
        Some(HdrPicture { width: deep.width, height: deep.height, rgba16f, peak })
    }

    /// Bytes the pixels take.
    pub fn bytes(&self) -> usize {
        self.rgba16f.len().saturating_mul(2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightcraft_pipeline::OutputSpace;

    fn deep(w: usize, h: usize, samples: DeepSamples) -> DeepImage {
        DeepImage { width: w, height: h, space: OutputSpace::Srgb, samples }
    }

    #[test]
    fn highlights_survive_as_encoded_half_floats_with_opaque_alpha() {
        use lightcraft_color::transfer::linear_to_srgb as enc;
        let d = deep(2, 1, DeepSamples::F32(vec![0.5, 1.0, 4.0, 0.0, 8.0, 0.25]));
        let p = HdrPicture::from_deep(&d, 8.0).unwrap();
        assert_eq!((p.width, p.height, p.rgba16f.len()), (2, 1, 8));
        let f = |i: usize| half::f16::from_bits(p.rgba16f[i]).to_f32();
        let close = |a: f32, b: f32| (a - b).abs() < 2e-3 * b.max(1.0);
        // SDR white is 1.0 either way; above it, the sRGB curve carries on
        assert_eq!(f(1), 1.0);
        for (got, linear) in [(f(0), 0.5), (f(2), 4.0), (f(4), 0.0), (f(5), 8.0), (f(6), 0.25)] {
            assert!(close(got, enc(linear)), "{got} for linear {linear}");
        }
        assert!(f(2) > 1.5 && f(5) > f(2), "the highlights are above white and ordered");
        assert_eq!([f(3), f(7)], [1.0, 1.0], "opaque");
        assert_eq!(p.bytes(), 16);
    }

    #[test]
    fn samples_are_held_to_the_headroom_and_never_nan() {
        let d = deep(1, 1, DeepSamples::F32(vec![f32::NAN, -3.0, 100.0]));
        let p = HdrPicture::from_deep(&d, 4.0).unwrap();
        let f = |i: usize| half::f16::from_bits(p.rgba16f[i]).to_f32();
        // (NaN and negative: black; 100 held to the peak, 4, and encoded)
        assert_eq!([f(0), f(1)], [0.0, 0.0]);
        assert!((f(2) - lightcraft_color::transfer::linear_to_srgb(4.0)).abs() < 2e-3);
        // a hostile peak is no limit at all, and never a NaN pixel
        let p = HdrPicture::from_deep(&d, f32::NAN).unwrap();
        assert!(p.rgba16f.iter().all(|b| half::f16::from_bits(*b).is_finite()));
    }

    #[test]
    fn only_the_float_render_of_the_right_size_makes_a_picture() {
        assert!(HdrPicture::from_deep(&deep(1, 1, DeepSamples::U16(vec![0; 3])), 2.0).is_none());
        assert!(HdrPicture::from_deep(&deep(2, 2, DeepSamples::F32(vec![0.0; 3])), 2.0).is_none());
        assert!(HdrPicture::from_deep(&deep(usize::MAX, 2, DeepSamples::F32(vec![])), 2.0).is_none());
    }
}
