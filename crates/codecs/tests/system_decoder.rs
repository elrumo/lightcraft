//! HEIC / HEIF and AVIF have no pure-Rust decoder: a host with a system decoder (iOS: ImageIO)
//! installs it, and those files then decode like any other. Without one they stay unsupported.
//! One test, sequential: the decoder is process-wide.

use lightcraft_codecs::{DecodeOptions, Error, Format, SystemImage, SystemPixels, decode, sniff};

const HEIC: &[u8] = b"\0\0\0\x18ftypheic\0\0\0\0mif1heic\0\0\0\x08free";

fn fake(_bytes: &[u8], _max: Option<(u32, u32)>) -> Result<SystemImage, String> {
    // 4 × 2, opaque, pure red then pure green rows, as stored (rotated 90° CW by the EXIF value)
    let mut px = Vec::new();
    for y in 0..2 {
        for _ in 0..4 {
            px.extend_from_slice(if y == 0 { &[255, 0, 0, 255] } else { &[0, 255, 0, 255] });
        }
    }
    Ok(SystemImage {
        width: 4,
        height: 2,
        pixels: SystemPixels::Rgba8(px),
        premultiplied: true,
        bit_depth: 10,
        icc: None,
        exif: None,
        orientation: 6,
        source_width: 4032,
        source_height: 3024,
    })
}

fn short(_bytes: &[u8], _max: Option<(u32, u32)>) -> Result<SystemImage, String> {
    Ok(SystemImage { pixels: SystemPixels::Rgba16(vec![0; 7]), ..fake(&[], None)? })
}

fn failing(_bytes: &[u8], _max: Option<(u32, u32)>) -> Result<SystemImage, String> {
    Err("ImageIO: no image in the file".into())
}

#[test]
fn heic_decodes_through_the_system_decoder_when_the_host_has_one() {
    assert_eq!(sniff(HEIC), Some(Format::Heif));
    assert!(!Format::Heif.can_decode());
    assert!(matches!(decode(HEIC, DecodeOptions::default()), Err(Error::Unsupported(Format::Heif, _))));

    lightcraft_codecs::set_system_decoder(&[Format::Heif], fake);
    assert!(Format::Heif.can_decode());
    assert!(!Format::Avif.can_decode(), "only the formats it was installed for");
    let d = decode(HEIC, DecodeOptions::default()).unwrap();
    assert_eq!((d.format, d.width, d.height), (Format::Heif, 4, 2));
    assert_eq!((d.source_width, d.source_height), (4032, 3024));
    assert_eq!(d.orientation, 6, "orientation is reported, not applied");
    assert_eq!(d.bit_depth, 10);
    assert!(d.alpha.is_none() && !d.has_alpha, "an opaque photo has no alpha plane");
    let red = d.image.data[0];
    let green = d.image.data[4];
    assert!(red[0] > 0.9 && red[1] < 0.05, "{red:?}");
    assert!(green[1] > 0.9 && green[0] < 0.05, "{green:?}");
    // fitting into a box resamples what the decoder returned
    let small = decode(HEIC, DecodeOptions::fit(2, 2)).unwrap();
    assert_eq!((small.width, small.height), (2, 1));

    // a decoder that returns too few samples or fails: an error, never a panic
    lightcraft_codecs::set_system_decoder(&[Format::Heif], short);
    assert!(matches!(decode(HEIC, DecodeOptions::default()), Err(Error::Malformed(Format::Heif, _))));
    lightcraft_codecs::set_system_decoder(&[Format::Heif], failing);
    let e = decode(HEIC, DecodeOptions::default()).unwrap_err();
    assert!(e.to_string().contains("no image in the file"), "{e}");

    lightcraft_codecs::clear_system_decoder();
    assert!(!Format::Heif.can_decode());
}
