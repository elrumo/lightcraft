//! Standard image codecs for LightCraft (layer L1).
//!
//! - [`sniff`] detects the container format (and flags camera raws for `lightcraft-raw`).
//! - [`decode`] decodes JPEG, PNG, TIFF, WebP, GIF, BMP, PSD (merged composite), JPEG XL and HEIC into
//!   **linear-light** [`Rgb32f`] in the source's own primaries, described by [`SourceSpace`]; ICC
//!   profiles (matrix/TRC exactly, LUT/CMYK via the `moxcms` CMS), 16-bit and float precision are
//!   preserved. EXIF/XMP/ICC blobs are passed through untouched; orientation is reported, never applied.
//! - [`to_working`] converts a decode result to linear Rec.2020 D65 (the pipeline working space).
//! - [`read_header`] reads a file's stored dimensions and orientation from its headers, without
//!   decoding pixels (import probes).
//! - [`decode_thumbnail`] is the fast path for grid thumbnails (EXIF thumbnail or DCT-scaled decode).
//! - [`encode`] writes JPEG, PNG, TIFF, lossless WebP and, on native targets, AVIF (feature `avif`) and
//!   JPEG XL ([`encode_jxl`], feature `jxl`), embedding
//!   ICC/EXIF/XMP. [`icc::write_matrix_trc`] builds profiles for export.
//! - [`video`] encodes AV1 video (native, feature `avif`) and writes it as MP4 or WebM.
//!
//! No decoder panics on malformed input (property-tested).
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod convert;
pub mod depth;
pub mod display;
pub mod encode;
pub mod exif;
pub mod gainmap;
mod heif;
pub mod icc;
mod jpeg;
pub mod jpeg_par;
#[cfg(feature = "jxl")]
mod jxl;
mod jxl_encode;
mod other;
mod png_codec;
pub mod print;
pub mod proof;
mod psd;
mod sniff;
pub mod space;
mod system;
mod tiff_codec;
pub mod video;
mod webp;

pub use encode::{
    ChromaSubsampling, EncodeImage, EncodeMeta, HDR_REFERENCE_WHITE_NITS, HLG_REFERENCE_PEAK_NITS, HdrTransfer, Samples, TiffCompression,
    encode_avif, encode_avif_hdr, encode_avif_pq, encode_jpeg, encode_png, encode_png_pq, encode_psd, encode_tiff, encode_webp_lossless, hlg_signal,
};
pub use jxl_encode::{JxlColour, JxlEffort, JxlOptions, encode_jxl};
pub use sniff::{Format, sniff};
pub use space::{NamedSpace, SourceSpace, SpaceOrigin, Trc};
pub use system::{SystemDecodeFn, SystemImage, SystemPixels, clear_system_decoder, has_system_decoder, set_system_decoder};

use lightcraft_raster::{Plane, Rgb32f, Rgba8};
use serde::{Deserialize, Serialize};

/// Codec errors.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("unrecognised image format")]
    UnknownFormat,
    #[error("{0:?} decoding is not supported: {1}")]
    Unsupported(Format, &'static str),
    #[error("malformed {0:?} data: {1}")]
    Malformed(Format, String),
    #[error("image too large: {0}×{1}")]
    TooLarge(u64, u64),
    #[error("encode error: {0}")]
    Encode(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Decode options.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct DecodeOptions {
    /// Fit the result within this box (never upscales). JPEG uses DCT-domain scaling (1/2, 1/4,
    /// 1/8) before a final linear-light resample; other formats decode fully then resample.
    pub max_size: Option<(u32, u32)>,
    /// Refuse images with more pixels than this (default 1 Gpx).
    pub max_pixels: u64,
    /// Give the picture in the working space in half floats ([`Decoded::half`]) where the decode can make it straight
    /// from the file's samples (no alpha, no CMYK, nothing to fit): a big picture then never exists in floats, which
    /// take twice the memory. Elsewhere the decode is as usual.
    #[serde(default)]
    pub half: bool,
}

impl Default for DecodeOptions {
    fn default() -> Self {
        DecodeOptions { max_size: None, max_pixels: 1 << 30, half: false }
    }
}

impl DecodeOptions {
    pub fn fit(w: u32, h: u32) -> Self {
        DecodeOptions { max_size: Some((w, h)), ..Default::default() }
    }
}

/// A decoded image.
#[derive(Clone, Debug)]
pub struct Decoded {
    pub format: Format,
    /// Linear light, in the primaries described by [`Decoded::space`] (empty when the picture is in [`Self::half`]).
    pub image: Rgb32f,
    /// The picture in the working space (linear Rec.2020 D65, what [`Self::into_working`] gives) in half floats, when
    /// [`DecodeOptions::half`] asked for it and the decode made it.
    pub half: Option<lightcraft_raster::Rgb16f>,
    /// Straight (unassociated) alpha 0..1, when the source has transparency.
    pub alpha: Option<Plane>,
    pub space: SourceSpace,
    /// Bits per sample in the file (8, 16, 32, …).
    pub bit_depth: u8,
    /// Sample format in the file was floating point.
    pub float: bool,
    pub has_alpha: bool,
    /// The source is grayscale (expanded to RGB in `image`).
    pub grayscale: bool,
    /// The source was CMYK (converted).
    pub cmyk: bool,
    /// EXIF orientation value 1..=8 (1 when absent). **Not applied** to `image`.
    pub orientation: u16,
    /// TIFF-structured EXIF bytes (starting at the `II`/`MM` byte-order mark), if embedded.
    /// For TIFF files the file itself is the TIFF structure and this is `None`.
    pub exif: Option<Vec<u8>>,
    pub xmp: Option<String>,
    pub icc: Option<Vec<u8>>,
    /// Dimensions of `image`.
    pub width: u32,
    pub height: u32,
    /// Full-resolution dimensions of the source (differs from `width`/`height` when scaled).
    pub source_width: u32,
    pub source_height: u32,
}

impl Decoded {
    /// Pixels converted to linear Rec.2020 D65.
    pub fn to_working(&self) -> Rgb32f {
        to_working(self)
    }

    /// The picture in the working space as it was decoded: [`Self::half`] when there is one, else
    /// [`Self::into_working`].
    pub fn into_working_pixels(self) -> lightcraft_raster::SourcePixels {
        match self.half {
            Some(h) => h.into(),
            None => self.into_working().into(),
        }
    }

    /// [`Self::to_working`], converting the decoded pixels where they are instead of next to a copy: a full-size
    /// decode then holds one float picture, not two (a 267 MP JPEG: 3.2 GB less at its peak).
    pub fn into_working(self) -> Rgb32f {
        let Decoded { mut image, alpha, space, .. } = self;
        working_in_place(&mut image, alpha.as_ref(), &space);
        image
    }

    /// Display-encoded 8-bit sRGB (gamut clipped), with alpha.
    pub fn to_srgb8(&self) -> Rgba8 {
        let m = self.space.to_space(&lightcraft_color::SRGB);
        let ident = m == lightcraft_color::Mat3::IDENTITY;
        let m = m.to_f32();
        let enc = lightcraft_color::transfer::encode_srgb8;
        let mut out = Rgba8::new(self.image.width, self.image.height);
        let alpha = self.alpha.as_ref();
        lightcraft_raster::par_rows(&mut out.data, self.image.width, |y, row| {
            let src = self.image.row(y);
            let a = alpha.map(|a| a.row(y));
            for (x, (o, p)) in row.iter_mut().zip(src).enumerate() {
                let q = if ident { *p } else { mat_apply(&m, *p) };
                let al = a.map(|a| (a[x].clamp(0.0, 1.0) * 255.0 + 0.5) as u8).unwrap_or(255);
                *o = [enc(q[0]), enc(q[1]), enc(q[2]), al];
            }
        });
        out
    }
}

#[inline]
pub(crate) fn mat_apply(m: &[[f32; 3]; 3], p: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * p[0] + m[0][1] * p[1] + m[0][2] * p[2],
        m[1][0] * p[0] + m[1][1] * p[1] + m[1][2] * p[2],
        m[2][0] * p[0] + m[2][1] * p[1] + m[2][2] * p[2],
    ]
}

/// Decode any supported format (see [`Format::can_decode`]).
///
/// Malformed input yields an error, never a panic: on unwinding targets a panic inside a third-party
/// decoder is caught and reported as [`Error::Malformed`].
pub fn decode(bytes: &[u8], opts: DecodeOptions) -> Result<Decoded> {
    let format = sniff(bytes).ok_or(Error::UnknownFormat)?;
    std::panic::catch_unwind(|| decode_unguarded(bytes, format, &opts)).unwrap_or_else(|_| Err(Error::Malformed(format, "decoder panicked".into())))
}

/// A PNG's samples as stored, `(width, height, pixels)` with each sample 0..1: no colour management, no gamma, alpha dropped, gray
/// expanded. For an image that carries numbers rather than a picture, such as a Hald CLUT. An image of more than `max_pixels`
/// pixels is refused before it is inflated. Malformed input yields an error, never a panic.
pub fn decode_png_codes(bytes: &[u8], max_pixels: u64) -> Result<(u32, u32, Vec<[f32; 3]>)> {
    std::panic::catch_unwind(|| png_codec::codes(bytes, max_pixels)).unwrap_or_else(|_| Err(Error::Malformed(Format::Png, "decoder panicked".into())))
}

/// What a file's headers say about its image: enough to catalogue it without decoding pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub format: Format,
    /// Full-resolution stored dimensions, orientation **not** applied ([`Decoded::source_width`]).
    pub width: u32,
    pub height: u32,
    /// EXIF/TIFF orientation 1..=8, 1 when absent ([`Decoded::orientation`]).
    pub orientation: u16,
}

/// Read the stored dimensions and orientation of any format [`decode`] supports, agreeing with
/// what a decode reports, without decoding pixels: JPEG, PNG, TIFF, WebP, PSD and HEIF are read
/// from their headers (markers, chunks, IFD, resources, boxes). GIF, BMP and JPEG XL fall back to
/// a small decode.
///
/// The header readers also refuse what a decode would refuse before reaching the pixels (an
/// unsupported layout, compression or frame type) and files whose image data is cut short
/// (truncated: the data runs past the end of the file, or a JPEG's markers don't reach its
/// end-of-image marker). Damage *inside* compressed data that is all present is only found by a
/// decode. Malformed input yields an error, never a panic (as for [`decode`], a panic inside a
/// third-party parser is caught on unwinding targets).
pub fn read_header(bytes: &[u8]) -> Result<Header> {
    let format = sniff(bytes).ok_or(Error::UnknownFormat)?;
    std::panic::catch_unwind(|| read_header_unguarded(bytes, format))
        .unwrap_or_else(|_| Err(Error::Malformed(format, "header reader panicked".into())))
}

/// [`read_header`] without the panic guard (for fuzzing our own code paths).
#[doc(hidden)]
pub fn read_header_unguarded(bytes: &[u8], format: Format) -> Result<Header> {
    let (width, height, orientation) = match format {
        Format::Jpeg => jpeg::header(bytes)?,
        Format::Png => png_codec::header(bytes)?,
        Format::Tiff => tiff_codec::header(bytes)?,
        Format::WebP => webp::header(bytes)?,
        Format::Psd => psd::header(bytes)?,
        Format::Heif => heif::header(bytes)?,
        // rare here (or, for JPEG XL, without a header reader yet): a small decode, as before
        _ => {
            let d = decode_unguarded(bytes, format, &DecodeOptions::fit(64, 64))?;
            (d.source_width, d.source_height, d.orientation)
        }
    };
    Ok(Header { format, width, height, orientation })
}

/// Decode a JPEG preview with its enclosing container's colour space as a fallback.
/// A JPEG carrying ICC or EXIF metadata keeps its own interpretation. The fallback is applied
/// before transfer decoding and linear-light resizing, never to already-resampled pixels.
pub fn decode_jpeg_with_fallback(bytes: &[u8], opts: DecodeOptions, fallback: NamedSpace) -> Result<Decoded> {
    std::panic::catch_unwind(|| jpeg::decode_with_fallback(bytes, &opts, Some(fallback)))
        .unwrap_or_else(|_| Err(Error::Malformed(Format::Jpeg, "decoder panicked".into())))
}

/// [`decode`] without the panic guard (for fuzzing our own code paths).
#[doc(hidden)]
pub fn decode_unguarded(bytes: &[u8], format: Format, opts: &DecodeOptions) -> Result<Decoded> {
    let opts = *opts;
    // The reason a format can't be decoded lives in one place, `Format::not_decodable`.
    let unsupported = || Err(Error::Unsupported(format, format.not_decodable().unwrap_or("not decodable in this build")));
    match format {
        Format::Jpeg => jpeg::decode(bytes, &opts),
        Format::Png => png_codec::decode(bytes, &opts),
        Format::Tiff => tiff_codec::decode(bytes, &opts),
        Format::WebP => webp::decode(bytes, &opts),
        Format::Gif | Format::Bmp => other::decode(bytes, format, &opts),
        Format::Psd => psd::decode(bytes, &opts),
        #[cfg(feature = "jxl")]
        Format::Jxl => jxl::decode(bytes, &opts),
        #[cfg(not(feature = "jxl"))]
        Format::Jxl => unsupported(),
        // the system's decoder first when the host installed one (iOS: ImageIO), see `system`
        Format::Heif => system::decode(bytes, format, &opts).unwrap_or_else(|| heif::decode(bytes, &opts)),
        Format::Avif => system::decode(bytes, format, &opts).unwrap_or_else(unsupported),
        Format::RawTiffLike | Format::RawOther => unsupported(),
    }
}

/// Convert a decode result to linear Rec.2020 D65 (Bradford-adapted), the develop working space.
///
/// The working space has no alpha: transparency is flattened onto white, in linear light like the rest
/// of the pipeline (issue #1212).
pub fn to_working(d: &Decoded) -> Rgb32f {
    let mut img = d.image.clone();
    working_in_place(&mut img, d.alpha.as_ref(), &d.space);
    img
}

/// [`to_working`] in `img` itself.
fn working_in_place(img: &mut Rgb32f, alpha: Option<&Plane>, space: &SourceSpace) {
    if !space.is_working() {
        let m = space.to_working().to_f32();
        img.map_in_place(|p| mat_apply(&m, p));
    }
    // white is (1, 1, 1) in the working space too: the conversion is white-preserving
    if let Some(a) = alpha.filter(|a| a.width == img.width && a.height == img.height) {
        for (p, &av) in img.data.iter_mut().zip(&a.data) {
            let av = av.clamp(0.0, 1.0);
            *p = p.map(|c| c * av + (1.0 - av));
        }
    }
}

/// Where a thumbnail came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThumbnailSource {
    /// The JPEG thumbnail embedded in EXIF (IFD1).
    ExifThumbnail,
    /// An MPF (CIPA DC-007) preview image embedded in a JPEG.
    MpfPreview,
    /// A (DCT-)scaled decode of the main image.
    Scaled,
}

/// A small display-ready image.
#[derive(Clone, Debug)]
pub struct Thumbnail {
    /// 8-bit sRGB with alpha, fitted within `max_edge` (orientation **not** applied).
    pub image: Rgba8,
    pub orientation: u16,
    pub source: ThumbnailSource,
    pub source_width: u32,
    pub source_height: u32,
}

/// Thumbnail request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThumbnailOptions {
    /// Fit the result within `max_edge × max_edge`.
    pub max_edge: u32,
    /// Accept an embedded preview whose long edge is at least this (it is then returned at its own,
    /// smaller size). Defaults to `max_edge`, i.e. only previews that need no upscaling. Lower it
    /// (e.g. 160 for typical EXIF thumbnails) for an instant placeholder.
    pub min_embedded_edge: u32,
    /// Maximum source pixels for fallback decoding (default 64 MP). Non-JPEG formats
    /// decode at source resolution before resizing, so output size does not bound memory.
    #[serde(default = "thumbnail_max_pixels")]
    pub max_pixels: u64,
}

fn thumbnail_max_pixels() -> u64 {
    64_000_000
}

impl ThumbnailOptions {
    pub fn new(max_edge: u32) -> Self {
        ThumbnailOptions { max_edge, min_embedded_edge: max_edge, max_pixels: thumbnail_max_pixels() }
    }
}

/// Fast thumbnail: an embedded JPEG preview (EXIF IFD1 thumbnail or MPF preview) at least `max_edge`
/// on its long side if one exists, else a scaled decode. The result fits within `max_edge × max_edge`.
pub fn decode_thumbnail(bytes: &[u8], max_edge: u32) -> Result<Thumbnail> {
    decode_thumbnail_with(bytes, &ThumbnailOptions::new(max_edge))
}

/// [`decode_thumbnail`] with options.
pub fn decode_thumbnail_with(bytes: &[u8], opts: &ThumbnailOptions) -> Result<Thumbnail> {
    let max_edge = opts.max_edge.max(1);
    let min_edge = opts.min_embedded_edge.clamp(1, max_edge);
    if sniff(bytes) == Some(Format::Jpeg)
        && let Ok(Some(t)) = std::panic::catch_unwind(|| jpeg::embedded_thumbnail(bytes, max_edge, min_edge))
    {
        return Ok(t);
    }
    let d = decode(bytes, DecodeOptions { max_size: Some((max_edge, max_edge)), max_pixels: opts.max_pixels, half: false })?;
    Ok(Thumbnail {
        image: d.to_srgb8(),
        orientation: d.orientation,
        source: ThumbnailSource::Scaled,
        source_width: d.source_width,
        source_height: d.source_height,
    })
}

/// Checked sample-buffer length `width × height × ch`, in u64: `None` on overflow or a zero
/// dimension. Image size is capped by `DecodeOptions::max_pixels` alone; once the buffer is known to
/// hold this many samples, every later `usize` product is bounded by an allocation that exists.
pub(crate) fn checked_sample_buf_len(width: usize, height: usize, ch: usize) -> Option<u64> {
    (width as u64).checked_mul(height as u64).and_then(|n| n.checked_mul(ch as u64)).filter(|&n| n > 0)
}

#[cfg(test)]
mod checked_arithmetic_tests {
    use super::*;

    #[test]
    fn checked_sample_buf_len_wrap_inputs_rejected() {
        assert_eq!(checked_sample_buf_len(usize::MAX, usize::MAX, 4), None);
        assert_eq!(checked_sample_buf_len(0, 1, 3), None);
    }

    #[test]
    fn checked_sample_buf_len_normal_accepted() {
        assert_eq!(checked_sample_buf_len(100, 1, 3), Some(300));
        assert_eq!(checked_sample_buf_len(1, 1, 1), Some(1));
        // 300 Mpx RGBA is within the default pixel limit, so it must not be refused by sample count.
        assert_eq!(checked_sample_buf_len(20_000, 15_000, 4), Some(1_200_000_000));
    }
}

#[cfg(test)]
mod working_tests {
    use super::*;

    /// Converting a decode in place gives what converting a copy of it gives: the colour matrix (an sRGB PNG is not in
    /// the working space) and transparency flattened onto white.
    #[test]
    fn into_working_is_to_working_without_the_copy() {
        let (w, h) = (37, 23);
        let data: Vec<[u8; 4]> = (0..w * h).map(|i| [(i * 7) as u8, (i * 13) as u8, (i * 29) as u8, (i * 3 % 256) as u8]).collect();
        let png = encode_png(&EncodeImage::rgba8(&Rgba8 { width: w, height: h, data }), &EncodeMeta::default()).unwrap();
        let d = decode(&png, DecodeOptions::default()).unwrap();
        assert!(d.alpha.is_some() && !d.space.is_working());
        let copy = d.to_working();
        assert_eq!(d.into_working().data, copy.data);
    }

    /// A decode asked for in half floats is the working picture's half floats, bit for bit: an 8-bit JPEG (in the
    /// file's colour, converted to the working space) and a 16-bit PNG; a picture with transparency to flatten, or one
    /// to fit, is decoded as usual.
    #[test]
    fn a_half_decode_is_the_working_pictures_half_floats() {
        let (w, h) = (41, 29);
        let rgba: Vec<[u8; 4]> = (0..w * h).map(|i| [(i * 7) as u8, (i * 13) as u8, (i * 29) as u8, 255]).collect();
        let img = Rgba8 { width: w, height: h, data: rgba };
        let jpeg = encode_jpeg(&EncodeImage::rgba8(&img), 90, ChromaSubsampling::S420, &EncodeMeta::default()).unwrap();
        let wide: Vec<u16> = (0..w * h * 3).map(|i| (i * 2731 % 65536) as u16).collect();
        let png16 = encode_png(&EncodeImage::new(w as u32, h as u32, 3, encode::Samples::U16(&wide)), &EncodeMeta::default()).unwrap();
        for (name, bytes) in [("JPEG", &jpeg), ("16-bit PNG", &png16)] {
            let half = DecodeOptions { half: true, ..Default::default() };
            let (a, b) = (decode(bytes, half).unwrap(), decode(bytes, DecodeOptions::default()).unwrap());
            assert!(a.image.is_empty() && (a.width, a.height) == (b.width, b.height), "{name}");
            let want = lightcraft_raster::Rgb16f::from_f32(&b.into_working());
            assert_eq!(a.into_working_pixels().view().to_f32().data, want.widen().data, "{name}");
        }
        // transparency: the floats, as ever
        let clear = Rgba8 { width: 4, height: 4, data: vec![[200, 100, 50, 128]; 16] };
        let png = encode_png(&EncodeImage::rgba8(&clear), &EncodeMeta::default()).unwrap();
        let d = decode(&png, DecodeOptions { half: true, ..Default::default() }).unwrap();
        assert!(d.half.is_none() && d.image.len() == 16);
        // to fit: the floats, fitted
        let d = decode(&jpeg, DecodeOptions { half: true, ..DecodeOptions::fit(16, 16) }).unwrap();
        assert!(d.half.is_none() && d.image.width == 16);
    }
}
