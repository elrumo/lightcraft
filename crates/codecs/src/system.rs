//! Decoders the operating system provides: AVIF, which has no pure-Rust, permissively licensed
//! decoder yet, and HEIC / HEIF, where the system's (hardware) decoder is faster than ours
//! (`crate::heif`). The product stays pure Rust: a host that has such a decoder (the iOS app:
//! ImageIO) installs it with [`set_system_decoder`]; without one AVIF stays
//! [`crate::Error::Unsupported`] and [`crate::Format::can_decode`] says so.

use std::sync::RwLock;

use crate::convert::{Buf, Meta, Model, Raw, check_size, finish};
use crate::{DecodeOptions, Decoded, Error, Format, Result};

/// Pixels as a system decoder returns them: as stored (orientation **not** applied), RGBA with
/// straight or premultiplied alpha, in the colour space `icc` describes (sRGB when `None`).
#[derive(Clone, Debug, PartialEq)]
pub struct SystemImage {
    pub width: u32,
    pub height: u32,
    pub pixels: SystemPixels,
    /// Alpha is premultiplied into the colour channels.
    pub premultiplied: bool,
    /// Significant bits per sample in the file (8, 10, 12, 16).
    pub bit_depth: u8,
    pub icc: Option<Vec<u8>>,
    /// TIFF-structured EXIF, if the decoder returns it.
    pub exif: Option<Vec<u8>>,
    /// EXIF orientation 1..=8 (1 when absent).
    pub orientation: u16,
    /// Full-resolution size of the source (differs from `width` × `height` when the decoder
    /// scaled it for `max_size`).
    pub source_width: u32,
    pub source_height: u32,
}

/// RGBA samples, row by row.
#[derive(Clone, Debug, PartialEq)]
pub enum SystemPixels {
    Rgba8(Vec<u8>),
    Rgba16(Vec<u16>),
}

/// A system decoder: the file's bytes and the box to fit in (`None`: full size; the decoder may
/// return a larger image, it is resampled) → pixels.
pub type SystemDecodeFn = fn(&[u8], Option<(u32, u32)>) -> std::result::Result<SystemImage, String>;

static DECODER: RwLock<Option<(&'static [Format], SystemDecodeFn)>> = RwLock::new(None);

/// Decode `formats` with `f` from now on (the iOS host: ImageIO for HEIC / HEIF and AVIF). Only
/// formats without a decoder of our own are handed over.
pub fn set_system_decoder(formats: &'static [Format], f: SystemDecodeFn) {
    *DECODER.write().unwrap_or_else(|e| e.into_inner()) = Some((formats, f));
}

/// Remove the system decoder (tests).
#[doc(hidden)]
pub fn clear_system_decoder() {
    *DECODER.write().unwrap_or_else(|e| e.into_inner()) = None;
}

fn decoder_for(format: Format) -> Option<SystemDecodeFn> {
    let d = DECODER.read().unwrap_or_else(|e| e.into_inner());
    d.as_ref().filter(|(formats, _)| formats.contains(&format)).map(|(_, f)| *f)
}

/// Is a system decoder installed for `format`?
pub fn has_system_decoder(format: Format) -> bool {
    decoder_for(format).is_some()
}

/// Decode with the system decoder for `format`, if one is installed.
pub(crate) fn decode(bytes: &[u8], format: Format, opts: &DecodeOptions) -> Option<Result<Decoded>> {
    let f = decoder_for(format)?;
    Some(f(bytes, opts.max_size).map_err(|e| Error::Malformed(format, e)).and_then(|img| to_decoded(format, img, opts)))
}

fn to_decoded(format: Format, img: SystemImage, opts: &DecodeOptions) -> Result<Decoded> {
    let (w, h) = (img.width, img.height);
    check_size(format, w as u64, h as u64, opts)?;
    let n = (w as usize).checked_mul(h as usize).and_then(|n| n.checked_mul(4)).ok_or(Error::TooLarge(w as u64, h as u64))?;
    let (buf, bits) = match img.pixels {
        SystemPixels::Rgba8(v) => (Buf::U8(v), 8),
        SystemPixels::Rgba16(v) => (Buf::U16(v), 16),
    };
    let len = match &buf {
        Buf::U8(v) => v.len(),
        Buf::U16(v) => v.len(),
        Buf::F32(v) => v.len(),
    };
    if len < n {
        return Err(Error::Malformed(format, format!("the system decoder returned {len} samples for {w}×{h} RGBA")));
    }
    let raw = Raw { width: w as usize, height: h as usize, model: Model::Rgb, alpha: true, premultiplied: img.premultiplied, buf, bit_depth: bits };
    let meta = Meta { icc: img.icc, exif: img.exif, orientation: Some(img.orientation.clamp(1, 8)), ..Default::default() };
    let source = (img.source_width.max(w), img.source_height.max(h));
    let mut d = finish(format, raw, meta, source, opts)?;
    d.bit_depth = img.bit_depth.clamp(1, 16);
    // decoders hand back opaque photos as RGBA: no alpha plane unless something is transparent
    if d.alpha.as_ref().is_some_and(|a| a.data.iter().all(|v| *v >= 1.0)) {
        d.alpha = None;
        d.has_alpha = false;
    }
    Ok(d)
}
