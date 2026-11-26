//! What a DNG rewrite carries over from its source, and the check that the rewrite is what was asked for.
//!
//! [`write_dng`](crate::write_dng) writes what a [`RawImage`] holds: pixels, levels, colour and the profile look. A
//! DNG from another maker holds more: Apple ProRAW keeps its Exif IFD (capture date, GPS, lens, maker note), DNG private
//! data, noise profile, semantic masks (people, skin, sky …) and a JPEG preview. [`DngCarry::from_dng`] reads those
//! from the source file and [`write_dng_carrying`](crate::write_dng_carrying) writes them again; [`write_dng_smaller`]
//! does both for a whole file and checks the result by decoding it again, as `photo.convertToDng` does.

use crate::{DngCompression, RawData, RawError, RawImage, Result};
use craft_tiff::tags::{self as t, compression, photometric};
use craft_tiff::{ByteOrder, Ifd, IfdBuilder, ImageData, ParseOptions, Tiff, Value};

/// Most bytes of one carried value: a hostile or damaged file cannot make a rewrite copy gigabytes.
const MAX_VALUE_BYTES: usize = 8 << 20;
/// Most bytes of everything carried but the masks and the preview.
const MAX_CARRY_BYTES: usize = 24 << 20;
/// Largest preview carried (a camera preview is a few hundred kB to a few MB).
const MAX_PREVIEW_BYTES: usize = 32 << 20;

/// Tags of the raw IFD that a [`RawImage`] has no field for and the writer never writes itself, carried as they are:
/// `BayerGreenSplit`, `LinearResponseLimit`, `CameraSerialNumber`, `LensInfo`, `ChromaBlurRadius`, `AntiAliasStrength`,
/// `ShadowScale`, `BaselineNoise`, `DNGPrivateData`, `NoiseProfile`.
const IFD0_TAGS: [u16; 10] = [
    t::BAYER_GREEN_SPLIT,
    t::LINEAR_RESPONSE_LIMIT,
    t::CAMERA_SERIAL_NUMBER,
    t::LENS_INFO,
    t::CHROMA_BLUR_RADIUS,
    t::ANTI_ALIAS_STRENGTH,
    t::SHADOW_SCALE,
    t::BASELINE_NOISE,
    t::DNG_PRIVATE_DATA,
    t::NOISE_PROFILE,
];

/// Tags that point at another IFD (rebuilt as children, never copied as a number).
const POINTER_TAGS: [u16; 4] = [t::EXIF_IFD, t::GPS_IFD, t::INTEROP_IFD, t::SUB_IFDS];

/// The parts of a DNG a rewrite keeps besides the pixels.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DngCarry {
    /// Entries of the Exif IFD (capture date, exposure, lens, maker note …); empty: none is written.
    pub exif: Vec<(u16, Value)>,
    /// Entries of the GPS IFD; empty: none is written.
    pub gps: Vec<(u16, Value)>,
    /// Raw-IFD tags in [`IFD0_TAGS`] (DNG private data, noise profile …).
    pub ifd0: Vec<(u16, Value)>,
    /// The semantic masks (`craft_raw::semantic_masks`), written back when the picture keeps its geometry.
    pub masks: Vec<crate::SemanticMask>,
    /// The source's JPEG preview, as it is.
    pub preview: Option<Vec<u8>>,
}

fn entries_of(ifd: &Ifd, budget: &mut usize) -> Vec<(u16, Value)> {
    let mut out = Vec::new();
    for e in &ifd.entries {
        if POINTER_TAGS.contains(&e.tag) || matches!(e.value, Value::Ifd(_) | Value::Ifd8(_)) {
            continue;
        }
        let bytes = e.value.count().saturating_mul(8);
        if bytes > MAX_VALUE_BYTES || bytes > *budget {
            continue;
        }
        *budget -= bytes;
        out.push((e.tag, e.value.clone()));
    }
    out
}

impl DngCarry {
    /// What `source` (a DNG file) holds beyond a decoded [`RawImage`]. A file that is not a TIFF, or that holds none of it,
    /// gives an empty carry (never an error: the rewrite then simply has nothing to add). When the file has no Exif IFD,
    /// one is made from `metadata` (what decoding read from it).
    pub fn from_dng(source: &[u8], metadata: &lightcraft_meta::Metadata) -> Self {
        let mut carry = Self::default();
        let opts = ParseOptions { max_ifds: 256, ..Default::default() };
        let Ok(tiff) = Tiff::parse_with(source, &opts) else { return carry };
        let mut budget = MAX_CARRY_BYTES;
        for tag in IFD0_TAGS {
            if let Some(e) = tiff.find(tag)
                && e.value.count().saturating_mul(8) <= MAX_VALUE_BYTES.min(budget)
            {
                budget -= e.value.count().saturating_mul(8);
                carry.ifd0.push((tag, e.value.clone()));
            }
        }
        carry.exif = tiff.exif().map(|i| entries_of(i, &mut budget)).unwrap_or_default();
        carry.gps = tiff.gps().map(|i| entries_of(i, &mut budget)).unwrap_or_default();
        if carry.exif.is_empty() && carry.gps.is_empty() {
            // no Exif IFD in the source: the capture date, GPS and lens that decoding read go into one
            if let Ok(made) = Tiff::parse(&lightcraft_meta::write_exif(metadata)) {
                carry.exif = made.exif().map(|i| entries_of(i, &mut budget)).unwrap_or_default();
                carry.gps = made.gps().map(|i| entries_of(i, &mut budget)).unwrap_or_default();
            }
        }
        carry.masks = craft_raw::semantic_masks(source);
        carry.preview = craft_raw::embedded_preview(source).filter(|p| p.len() <= MAX_PREVIEW_BYTES && craft_raw::preview::jpeg_frame(p).is_some());
        carry
    }

    /// Put the carried parts into `ifd` (the raw IFD, IFD0): the Exif and GPS IFDs (with the picture's final size in
    /// `PixelXDimension` / `PixelYDimension` where the source gave them), the raw-IFD tags the writer did not set, and
    /// the semantic masks when `keep_geometry`. Returns how many masks were written.
    pub(crate) fn apply(&self, ifd: &mut IfdBuilder, size: (usize, usize), keep_geometry: bool, order: ByteOrder) -> usize {
        for (tag, value) in &self.ifd0 {
            ifd.entries.entry(*tag).or_insert_with(|| value.clone());
        }
        if !self.exif.is_empty() {
            let mut exif = IfdBuilder::new();
            for (tag, value) in &self.exif {
                exif.set(*tag, value.clone());
            }
            for (tag, v) in [(t::PIXEL_X_DIMENSION, size.0), (t::PIXEL_X_DIMENSION + 1, size.1)] {
                if exif.entries.contains_key(&tag) {
                    exif.set(tag, Value::Long(vec![u32::try_from(v).unwrap_or(u32::MAX)]));
                }
            }
            ifd.set_child(t::EXIF_IFD, exif);
        }
        if !self.gps.is_empty() {
            let mut gps = IfdBuilder::new();
            for (tag, value) in &self.gps {
                gps.set(*tag, value.clone());
            }
            ifd.set_child(t::GPS_IFD, gps);
        }
        if !keep_geometry {
            return 0;
        }
        let mut written = 0;
        for mask in &self.masks {
            if let Some(sub) = mask_ifd(mask, order) {
                ifd.add_sub_ifd(sub);
                written += 1;
            }
        }
        written
    }
}

/// A semantic mask as the sub-IFD DNG 1.6 describes: one 16-bit plane, deflate strip, named.
fn mask_ifd(mask: &crate::SemanticMask, order: ByteOrder) -> Option<IfdBuilder> {
    let (w, h) = (u32::try_from(mask.width).ok()?, u32::try_from(mask.height).ok()?);
    if w == 0 || h == 0 || mask.data.len() != mask.width.checked_mul(mask.height)? {
        return None;
    }
    let mut bytes = Vec::with_capacity(mask.data.len().checked_mul(2)?);
    mask.data.iter().for_each(|&v| order.put_u16(&mut bytes, v));
    let mut sub = IfdBuilder::new();
    sub.set(t::NEW_SUBFILE_TYPE, Value::Long(vec![t::SUBFILE_SEMANTIC_MASK]));
    sub.set(t::IMAGE_WIDTH, Value::Long(vec![w]));
    sub.set(t::IMAGE_LENGTH, Value::Long(vec![h]));
    sub.set(t::BITS_PER_SAMPLE, Value::Short(vec![16]));
    sub.set(t::SAMPLES_PER_PIXEL, Value::Short(vec![1]));
    sub.set(t::PLANAR_CONFIGURATION, Value::Short(vec![1]));
    sub.set(t::PHOTOMETRIC, Value::Short(vec![photometric::MASK]));
    sub.set(t::COMPRESSION, Value::Short(vec![compression::ADOBE_DEFLATE]));
    sub.set(t::SEMANTIC_NAME, Value::Ascii(mask.name.clone()));
    if let Some(id) = &mask.instance_id {
        sub.set(t::SEMANTIC_INSTANCE_ID, Value::Ascii(id.clone()));
    }
    if let Some(a) = mask.sub_area {
        sub.set(t::MASK_SUB_AREA, Value::Long(vec![a.top, a.left, a.full_width, a.full_height]));
    }
    sub.set_image(ImageData::Strips { rows_per_strip: h, strips: vec![miniz_oxide::deflate::compress_to_vec_zlib(&bytes, 4)] });
    Some(sub)
}

/// How far a rewrite is from its source, as a share of full scale (0 for a lossless one).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RewriteError {
    pub max: f32,
    pub mean: f32,
}

/// A rewritten DNG and what the check found.
#[derive(Clone, Debug, PartialEq)]
pub struct SmallerDng {
    pub bytes: Vec<u8>,
    pub error: RewriteError,
    /// Semantic masks, as written (0 when the picture changed geometry).
    pub masks: usize,
}

/// The mean and largest sample error a lossy JPEG XL of `distance` may show (a share of full scale, on the linear
/// samples). The mean is what the distance controls (a ProRAW measured 0.0023 at distance 0.5 and 0.0037 at 1.5); a few
/// samples at a highlight edge are far off (0.11 and 0.28 there), so the largest error only has to catch a wrong curve or
/// a scrambled tile, not a lossy codec's edges.
fn lossy_tolerance(distance: f32) -> RewriteError {
    let d = distance.clamp(0.05, 15.0);
    RewriteError { mean: 0.004 + 0.006 * d, max: 0.6 }
}

/// Decode `written` again and compare it with `source`: lossless output must give the source's samples back exactly, a
/// lossy JPEG XL within [`lossy_tolerance`]; a lossy-JPEG rewrite (which develops and crops the picture) must decode to
/// a picture of the size it declares. The carried Exif IFD and masks must be there too.
pub fn verify_rewrite(
    source: &RawImage,
    written: &[u8],
    compression: DngCompression,
    carry: Option<&DngCarry>,
    masks: usize,
) -> Result<RewriteError> {
    let back = crate::decode(written)?;
    if let Some(c) = carry {
        let tiff = Tiff::parse(written)?;
        if !c.exif.is_empty() && tiff.exif().is_none() {
            return Err(RawError::Corrupt("the rewritten DNG lost its Exif IFD".into()));
        }
        let found = craft_raw::semantic_masks(written).len();
        if found != masks {
            return Err(RawError::Corrupt(format!("the rewritten DNG has {found} semantic masks, expected {masks}")));
        }
    }
    if matches!(compression, DngCompression::LossyJpeg { .. }) {
        return if back.width > 0 && back.height > 0 { Ok(RewriteError::default()) } else { Err(RawError::Corrupt("empty lossy DNG".into())) };
    }
    if (back.width, back.height, back.cpp) != (source.width, source.height, source.cpp) {
        return Err(RawError::Corrupt(format!(
            "the rewritten DNG is {}×{}×{}, the source {}×{}×{}",
            back.width, back.height, back.cpp, source.width, source.height, source.cpp
        )));
    }
    let tolerance = match compression {
        DngCompression::Jxl { distance: Some(d), .. } => Some(lossy_tolerance(d)),
        DngCompression::Deflate { half: true, .. } if matches!(source.data, RawData::F32(_)) => Some(RewriteError { mean: 2e-3, max: 2e-3 }),
        _ => None,
    };
    let (max, mean) = match (&source.data, &back.data) {
        (RawData::U16(a), RawData::U16(b)) if a.len() == b.len() => {
            let (mut max, mut sum) = (0u32, 0f64);
            for (x, y) in a.iter().zip(b) {
                let d = u32::from(x.abs_diff(*y));
                max = max.max(d);
                sum += f64::from(d);
            }
            (max as f32 / 65535.0, (sum / a.len().max(1) as f64 / 65535.0) as f32)
        }
        (RawData::F32(a), RawData::F32(b)) if a.len() == b.len() => {
            let white = source.white_at(0).max(f32::MIN_POSITIVE);
            let (mut max, mut sum) = (0f32, 0f64);
            for (x, y) in a.iter().zip(b) {
                let d = (x - y).abs() / white;
                max = max.max(d);
                sum += f64::from(d);
            }
            (max, (sum / a.len().max(1) as f64) as f32)
        }
        _ => return Err(RawError::Corrupt("the rewritten DNG holds another kind of samples".into())),
    };
    let ok = match tolerance {
        // the gamma domain error is what the tolerance bounds; the linear error of a mid-grey is smaller, a shadow's larger
        // in code steps but tiny in value, so the linear share is compared against the same bound
        Some(tol) => mean.is_finite() && max.is_finite() && mean <= tol.mean && max <= tol.max,
        None => max == 0.0,
    };
    if ok {
        Ok(RewriteError { max, mean })
    } else {
        Err(RawError::Corrupt(format!("the rewritten DNG differs from its source (largest error {max}, mean {mean})")))
    }
}

/// Rewrite the DNG `source` with `compression` (for a smaller file: [`DngCompression::Jxl`], or `Lj92` / `Deflate`
/// for lossless) keeping its Exif IFD, maker note, DNG private data, noise profile, semantic masks and preview, and
/// check the result by decoding it again ([`verify_rewrite`]). A failed check is an error and no bytes.
pub fn write_dng_smaller(source: &[u8], compression: DngCompression) -> Result<SmallerDng> {
    let raw = crate::decode(source)?;
    let carry = DngCarry::from_dng(source, &raw.metadata);
    let opts = crate::DngWriteOptions { compression, ..Default::default() };
    let (bytes, masks) = crate::dngwrite::write_dng_inner(&raw, &opts, carry.preview.as_deref(), Some(&carry))?;
    let error = verify_rewrite(&raw, &bytes, compression, Some(&carry), masks)?;
    Ok(SmallerDng { bytes, error, masks })
}
