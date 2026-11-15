//! JPEG XL adapter to the shared `craft-jxl` codec.
//!
//! A PQ file (our own HDR export, an HDR10 JPEG XL) is asked for in its own encoding and linearised here, SDR white
//! at 203 cd/m²: asked for as linear, jxl-oxide tone maps anything declaring a peak above 255 cd/m² down to 255 (a
//! Rec. 2408 SDR rendition), which dropped the whole range above SDR white. (Not for an XYB file, a lossy one: it is
//! rendered through sRGB primaries, where a PQ target would need negative out-of-gamut samples encoded; those are
//! still tone mapped to 255 cd/m².)

use crate::convert::{Buf, Meta, Model, Raw, finish};
use crate::space::{NamedSpace, SourceSpace, SpaceOrigin, Trc};
use crate::{DecodeOptions, Decoded, Error, Format, Result};

const F: Format = Format::Jxl;

fn error(e: craft_jxl::Error) -> Error {
    match e {
        craft_jxl::Error::PixelLimitExceeded { width, height, .. } => Error::TooLarge(u64::from(width), u64::from(height)),
        craft_jxl::Error::Malformed(message) | craft_jxl::Error::LimitExceeded(message) | craft_jxl::Error::InvalidImage(message) => {
            Error::Malformed(F, message)
        }
        craft_jxl::Error::Unsupported(message) | craft_jxl::Error::Encode(message) => Error::Malformed(F, message),
    }
}

/// Bytes handed to the header parser at a time, and the most it is given in all.
const HEADER_CHUNK: usize = 4096;
const MAX_HEADER: usize = 1 << 20;

/// Whether a JPEG XL declares Rec. 2100 PQ as the enum colour encoding of RGB samples, not XYB and with no ICC
/// profile: from its headers alone (no frame is read).
fn is_enum_pq(bytes: &[u8]) -> bool {
    use jxl_oxide::color::{ColourEncoding, TransferFunction};
    use jxl_oxide::{InitializeResult, JxlImage};
    let mut uninit = JxlImage::builder().build_uninit();
    let (mut rest, mut window, mut fed) = (bytes, HEADER_CHUNK, 0usize);
    loop {
        let Ok(consumed) = uninit.feed_bytes(rest.get(..window).unwrap_or(rest)) else { return false };
        rest = rest.get(consumed..).unwrap_or_default();
        fed = fed.saturating_add(consumed);
        match uninit.try_init() {
            Ok(InitializeResult::Initialized(img)) => {
                let m = &img.image_header().metadata;
                return !m.xyb_encoded
                    && !m.grayscale()
                    && img.original_icc().is_none()
                    && matches!(&m.colour_encoding, ColourEncoding::Enum(e) if matches!(e.tf, TransferFunction::Pq));
            }
            Ok(InitializeResult::NeedMoreData(more)) => uninit = more,
            Err(_) => return false,
        }
        if fed > MAX_HEADER || rest.is_empty() {
            return false;
        }
        if consumed == 0 {
            window = window.saturating_mul(2);
        }
    }
}

/// A PQ file (see the module docs) decoded in its own encoding and linearised: `None` when it isn't one, or the
/// decoder rendered it in anything but Rec. 2100 PQ RGB (the caller then decodes it to linear as usual).
fn decode_pq(bytes: &[u8], limits: craft_jxl::DecodeLimits) -> Result<Option<craft_jxl::Decoded>> {
    if !is_enum_pq(bytes) {
        return Ok(None);
    }
    let mut d = craft_jxl::decode_with_color(bytes, limits, craft_jxl::OutputColor::DecoderDefault).map_err(error)?;
    // CICP primaries 9 (BT.2020), transfer 16 (PQ); no alpha to unpremultiply
    if !matches!(d.layout, craft_jxl::Layout::Rgb) || !matches!(d.rendered_cicp, Some([9, 16, ..])) {
        return Ok(None);
    }
    for v in &mut d.pixels {
        *v = Trc::Pq.to_linear(*v);
    }
    d.pixels_linear_rec2020 = true;
    Ok(Some(d))
}

pub(crate) fn decode(bytes: &[u8], opts: &DecodeOptions) -> Result<Decoded> {
    let limits = craft_jxl::DecodeLimits { max_pixels: opts.max_pixels, ..Default::default() };
    let decoded = match decode_pq(bytes, limits)? {
        Some(d) => d,
        None => craft_jxl::decode_with_color(bytes, limits, craft_jxl::OutputColor::LinearRec2020).map_err(error)?,
    };

    let gray = matches!(decoded.layout, craft_jxl::Layout::Gray | craft_jxl::Layout::GrayAlpha);
    let alpha = decoded.layout.has_alpha();
    let pixels_linear_rec2020 = decoded.pixels_linear_rec2020;
    let rendered_icc = if !pixels_linear_rec2020 && decoded.metadata.icc.is_none() && !decoded.rendered_icc.is_empty() {
        Some(decoded.rendered_icc.clone())
    } else {
        None
    };
    let mut meta = Meta {
        orientation: Some(1),
        icc: decoded.metadata.icc.or(rendered_icc),
        exif: decoded.metadata.exif,
        xmp: decoded.metadata.xmp.map(|packet| String::from_utf8_lossy(&packet).trim_end_matches('\0').to_owned()),
        ..Default::default()
    };
    if pixels_linear_rec2020 {
        let mut space = SourceSpace::named(NamedSpace::Rec2020, SpaceOrigin::Container);
        space.trc = Some([Trc::Linear, Trc::Linear, Trc::Linear]);
        meta.already_linear = Some(space);
    } else if meta.icc.is_none() {
        return Err(Error::Malformed(F, "shared decoder did not report the rendered JPEG XL color profile".into()));
    }
    let raw = Raw {
        width: decoded.width as usize,
        height: decoded.height as usize,
        model: if gray { Model::Gray } else { Model::Rgb },
        alpha,
        premultiplied: decoded.pixels_premultiplied,
        buf: Buf::F32(decoded.pixels),
        bit_depth: decoded.bit_depth,
    };
    let (source_width, source_height) =
        if (5..=8).contains(&decoded.source_orientation) { (decoded.height, decoded.width) } else { (decoded.width, decoded.height) };
    let mut result = finish(F, raw, meta, (source_width, source_height), opts)?;
    result.float = decoded.float_samples;
    Ok(result)
}
