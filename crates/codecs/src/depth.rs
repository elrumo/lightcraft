//! Depth maps stored in photo files, for Lens Blur and Depth Range masks: iPhone (and Apple
//! ImageIO) Portrait photos. A HEIC carries the map as a depth auxiliary image; a JPEG as a second,
//! greyscale JPEG after the photo (indexed by MPF). Both describe it in Apple's `apdi` XMP: whether
//! the samples are disparity (1 / metres) or depth (metres), and the float range their integer
//! codes span. Checked on files Apple's own ImageIO wrote (disparity and depth, HEIC and JPEG,
//! rotated); no camera-written Portrait file is in the corpus yet.
//!
//! [`depth`] returns *disparity* (larger is nearer, any scale) as stored: not yet oriented by the
//! file's EXIF orientation (a HEIC's container rotation is applied, as for its picture).

/// A depth map as relative disparity: `width × height`, row-major, larger values nearer.
#[derive(Clone, Debug, PartialEq)]
pub struct Disparity {
    pub width: usize,
    pub height: usize,
    pub data: Vec<f32>,
}

/// Most samples a stored depth map may have (they are small: an iPhone's is 768 × 576).
const MAX_SAMPLES: usize = 1 << 24;
/// Most embedded JPEG streams looked at for the depth map.
const MAX_STREAMS: usize = 32;

/// What Apple's `apdi` XMP says about an auxiliary image.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Apdi {
    /// Depth in metres (else disparity).
    depth: bool,
    float: [f32; 2],
    int: [f32; 2],
}

/// The text of XMP property `apdi:<name>`, as an element or an attribute.
fn apdi_value<'a>(xmp: &'a str, name: &str) -> Option<&'a str> {
    let tag = format!("apdi:{name}");
    let at = xmp.find(&tag)?;
    let rest = xmp.get(at + tag.len()..)?;
    let rest = rest.trim_start();
    let v = if let Some(r) = rest.strip_prefix('>') {
        r.split('<').next()?
    } else {
        let r = rest.strip_prefix('=')?.trim_start();
        let q = r.chars().next()?;
        r.get(1..)?.split(q).next()?
    };
    Some(v.trim())
}

/// Apple's description of a depth or disparity image; `None` for any other auxiliary image (a
/// matte, a gain map) or no `apdi` XMP.
fn apdi(xmp: &str) -> Option<Apdi> {
    let kind = apdi_value(xmp, "AuxiliaryImageType")?;
    let depth = match kind {
        k if k.ends_with("disparity") => false,
        k if k.ends_with("depth") => true,
        _ => return None,
    };
    let num = |n: &str, d: f32| apdi_value(xmp, n).and_then(|v| v.parse::<f32>().ok()).filter(|v| v.is_finite()).unwrap_or(d);
    Some(Apdi { depth, float: [num("FloatMinValue", 0.0), num("FloatMaxValue", 1.0)], int: [num("IntMinValue", 0.0), num("IntMaxValue", 255.0)] })
}

/// Integer codes (0..=`max`) as disparity, by Apple's description (or, without one, as codes that
/// grow nearer, as MPEG's depth images do).
fn to_disparity(codes: impl Iterator<Item = u16>, max: u16, desc: Option<Apdi>) -> Vec<f32> {
    match desc {
        Some(a) => {
            let span = (a.int[1] - a.int[0]).max(1.0);
            codes
                .map(|c| {
                    let f = a.float[0] + (f32::from(c) - a.int[0]) / span * (a.float[1] - a.float[0]);
                    if a.depth { 1.0 / f.max(1e-3) } else { f }
                })
                .collect()
        }
        None => codes.map(|c| f32::from(c) / f32::from(max.max(1))).collect(),
    }
}

/// The depth map a photo file stores, as disparity (see the module docs); `None` when it has none
/// or it can't be read.
pub fn depth(bytes: &[u8]) -> Option<Disparity> {
    match crate::sniff(bytes)? {
        crate::Format::Jpeg => jpeg_depth(bytes),
        #[cfg(feature = "heif")]
        crate::Format::Heif => heif_depth(bytes),
        _ => None,
    }
}

#[cfg(feature = "heif")]
fn heif_depth(bytes: &[u8]) -> Option<Disparity> {
    let opts = lightcraft_heif::Options { max_pixels: MAX_SAMPLES as u64, apply_transforms: true };
    let d = lightcraft_heif::depth(bytes, &opts).ok()??;
    let desc = d.xmp.as_deref().and_then(apdi);
    let (w, h) = (d.width as usize, d.height as usize);
    (d.data.len() == w * h && w > 0 && h > 0).then(|| Disparity { width: w, height: h, data: to_disparity(d.data.iter().copied(), d.max, desc) })
}

/// The marker segments of the JPEG stream starting at `start` (its SOI), up to its scan: each
/// segment's marker and payload.
fn segments(bytes: &[u8], start: usize) -> Vec<(u8, &[u8])> {
    let mut out = Vec::new();
    let mut i = start + 2;
    while let (Some(&0xFF), Some(&m)) = (bytes.get(i), bytes.get(i + 1)) {
        if m == 0xDA || m == 0xD9 || out.len() > 64 {
            break;
        }
        let Some(len) = bytes.get(i + 2..i + 4).map(|b| usize::from(u16::from_be_bytes([b[0], b[1]]))) else { break };
        let Some(body) = bytes.get(i + 4..i + 2 + len.max(2)) else { break };
        out.push((m, body));
        i += 2 + len.max(2);
    }
    out
}

/// A Portrait JPEG's depth map: the first greyscale JPEG after the photo whose XMP Apple marked as
/// depth or disparity.
fn jpeg_depth(bytes: &[u8]) -> Option<Disparity> {
    // (only a photo that indexes more pictures, with MPF, can have one: other JPEGs aren't scanned)
    if !segments(bytes, 0).iter().any(|(m, b)| *m == 0xE2 && b.starts_with(b"MPF\0")) {
        return None;
    }
    let starts = (2..bytes.len().saturating_sub(3)).filter(|&i| bytes.get(i..i + 3) == Some(&[0xFF, 0xD8, 0xFF][..])).take(MAX_STREAMS);
    for s in starts {
        let desc = segments(bytes, s).into_iter().filter(|(m, _)| *m == 0xE1).find_map(|(_, b)| std::str::from_utf8(b).ok().and_then(apdi));
        let Some(desc) = desc else { continue };
        if let Some(d) = decode_grey(bytes.get(s..)?, desc) {
            return Some(d);
        }
    }
    None
}

fn decode_grey(stream: &[u8], desc: Apdi) -> Option<Disparity> {
    use zune_core::bytestream::ZCursor;
    use zune_core::colorspace::ColorSpace;
    use zune_core::options::DecoderOptions;
    let opts =
        DecoderOptions::default().set_max_width(1 << 14).set_max_height(1 << 14).set_strict_mode(false).jpeg_set_out_colorspace(ColorSpace::Luma);
    let mut d = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(stream), opts);
    let px = d.decode().ok()?;
    let info = d.info()?;
    let (w, h) = (info.width as usize, info.height as usize);
    if w == 0 || h == 0 || w * h > MAX_SAMPLES || px.len() < w * h {
        return None;
    }
    Some(Disparity { width: w, height: h, data: to_disparity(px.iter().take(w * h).map(|&v| u16::from(v)), 255, Some(desc)) })
}

#[cfg(test)]
mod tests {
    use super::*;

    const XMP: &str = r#"<rdf:Description xmlns:apdi="http://ns.apple.com/pixeldatainfo/1.0/">
         <apdi:NativeFormat>1751411059</apdi:NativeFormat>
         <apdi:AuxiliaryImageType>depth</apdi:AuxiliaryImageType>
         <apdi:FloatMaxValue>4.000000</apdi:FloatMaxValue>
         <apdi:IntMinValue>0</apdi:IntMinValue>
         <apdi:FloatMinValue>1.000000</apdi:FloatMinValue>
         <apdi:IntMaxValue>255</apdi:IntMaxValue></rdf:Description>"#;

    #[test]
    fn apple_xmp_is_read_as_elements_or_attributes() {
        let a = apdi(XMP).unwrap();
        assert!(a.depth);
        assert_eq!((a.float, a.int), ([1.0, 4.0], [0.0, 255.0]));
        let attr = apdi(r#"<rdf:Description apdi:AuxiliaryImageType="urn:com:apple:photo:2018:aux:disparity" apdi:FloatMaxValue='2.5'/>"#).unwrap();
        assert!(!attr.depth);
        assert_eq!(attr.float, [0.0, 2.5]);
        // a matte or a gain map isn't depth
        assert!(apdi("<apdi:AuxiliaryImageType>urn:com:apple:photo:2018:aux:portraiteffectsmatte</apdi:AuxiliaryImageType>").is_none());
        assert!(apdi("<apdi:AuxiliaryImageType>").is_none());
    }

    #[test]
    fn depth_in_metres_becomes_disparity() {
        let d = to_disparity([0u16, 255].into_iter(), 255, apdi(XMP));
        assert!((d[0] - 1.0).abs() < 1e-6 && (d[1] - 0.25).abs() < 1e-6, "{d:?}");
        // without a description, codes grow nearer
        assert_eq!(to_disparity([0u16, 1023].into_iter(), 1023, None), vec![0.0, 1.0]);
    }

    #[test]
    fn files_without_depth_or_damaged_ones_have_none() {
        assert!(depth(b"").is_none());
        assert!(depth(&[0xFF, 0xD8, 0xFF, 0xE1, 0x00]).is_none());
        let mut fake = vec![0xFF, 0xD8, 0xFF, 0xD9];
        // a stream marked as depth that isn't a JPEG
        fake.extend([0xFF, 0xD8, 0xFF, 0xE1, 0x00, 0x40]);
        fake.extend(b"<apdi:AuxiliaryImageType>disparity</apdi:AuxiliaryImageType>".iter().chain([0u8; 8].iter()));
        assert!(depth(&fake).is_none());
    }

    /// With `LIGHTCRAFT_DEPTH_FILES` (a folder of `portrait-*.heic|jpg` written by Apple's ImageIO,
    /// see docs/lens-blur.md): every file has a depth map whose left half is nearer than its right,
    /// except where a near square sits in the top right quarter.
    #[test]
    fn apple_written_portraits_have_depth() {
        let Ok(dir) = std::env::var("LIGHTCRAFT_DEPTH_FILES") else { return };
        let mut seen = 0;
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            let name = p.file_name().unwrap().to_string_lossy().to_string();
            if !name.starts_with("portrait-") {
                continue;
            }
            let d = depth(&std::fs::read(&p).unwrap()).unwrap_or_else(|| panic!("{name}: no depth"));
            let at = |fx: f32, fy: f32| d.data[(fy * d.height as f32) as usize * d.width + (fx * d.width as f32) as usize];
            if name.contains("rot90") && name.ends_with(".heic") {
                // the container turned it a quarter: the near half is the top one
                assert!(at(0.5, 0.25) > at(0.5, 0.75), "{name}");
            } else {
                assert!(at(0.25, 0.75) > at(0.6, 0.75), "{name}");
                assert!(at(0.9, 0.1) > at(0.6, 0.75), "{name}: the near square");
            }
            seen += 1;
        }
        assert!(seen >= 4, "{seen} files");
    }
}
