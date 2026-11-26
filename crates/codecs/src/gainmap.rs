//! Gain map JPEGs (ISO 21496-1): an SDR base image plus a gain map that a decoder uses to rebuild
//! the HDR rendition, so the file looks right everywhere and lights up on HDR displays.
//!
//! The metadata formats and the arithmetic ([`compute`], [`apply`], [`GainMapMeta`]) are
//! `craft-gainmap`'s, re-exported here. This module lays the two images out in a JPEG:
//!
//! - [`encode_jpeg`] writes a gain map JPEG: the base with an MPF index (CIPA DC-007), the ISO
//!   21496-1 marker and Adobe/Google container XMP, followed by the gain map JPEG carrying the ISO
//!   21496-1 metadata, the Adobe `hdrgm` XMP (Android, Chrome) and Apple's description of the same
//!   map (what survives an iMessage send).
//! - [`read_jpeg`] finds and decodes the gain map of a gain map JPEG (ours, cameras, Lightroom),
//!   from its ISO 21496-1 metadata or, failing that, its `hdrgm` XMP; [`read`] also finds a HEIC's
//!   (an iPhone photo's Apple gain map, or an ISO 21496-1 tone-map item).
//! - [`apply`] is `craft-gainmap`'s arithmetic run a row at a time on every core.

pub use craft_gainmap::{GainImage, GainMapMeta, GainMapOptions, ISO_URN, compute};

use crate::encode::{EncodeImage, EncodeMeta, Samples, jpeg_app_segments, xmp_segment};
use crate::{ChromaSubsampling, Error, Format, Result, jpeg};

/// Encode a gain map JPEG: `base` (8-bit SDR, 3 or 4 channels, colour-managed by `meta.icc`) and
/// its gain map, both at `quality`. `meta.xmp` is kept, with the gain map container added.
pub fn encode_jpeg(base: &EncodeImage, gain: &GainImage, m: &GainMapMeta, quality: u8, sub: ChromaSubsampling, meta: &EncodeMeta) -> Result<Vec<u8>> {
    let Samples::U8(data) = base.samples else {
        return Err(Error::Encode("gain map JPEG needs an 8-bit base image".into()));
    };
    let (w, h) = (base.width as usize, base.height as usize);
    if !matches!(base.channels, 3 | 4) {
        return Err(Error::Encode("gain map JPEG needs an RGB base image".into()));
    }
    let Some(base_bytes) = w.checked_mul(h).and_then(|pixels| pixels.checked_mul(base.channels as usize)) else {
        return Err(Error::Encode("gain map JPEG needs an RGB base image".into()));
    };
    if data.len() < base_bytes {
        return Err(Error::Encode("gain map JPEG needs an RGB base image".into()));
    }
    if w == 0 || h == 0 || w > u16::MAX as usize || h > u16::MAX as usize {
        return Err(Error::Encode("JPEG dimensions are limited to 65535".into()));
    }
    if !matches!(gain.channels, 1 | 3) || gain.width == 0 || gain.height == 0 {
        return Err(Error::Encode("gain map image is malformed".into()));
    }
    // Check the JPEG frame-header limit before multiplying dimensions, then validate the source
    // buffer with checked arithmetic on both 32- and 64-bit targets.
    if gain.width > u16::MAX as usize || gain.height > u16::MAX as usize {
        return Err(Error::Encode("JPEG dimensions are limited to 65535".into()));
    }
    let Some(gain_bytes) = gain.width.checked_mul(gain.height).and_then(|pixels| pixels.checked_mul(gain.channels)) else {
        return Err(Error::Encode("gain map image is malformed".into()));
    };
    if gain.data.len() < gain_bytes {
        return Err(Error::Encode("gain map image is malformed".into()));
    }
    if !m.is_valid() || m.multichannel != (gain.channels == 3) {
        return Err(Error::Encode("gain map metadata doesn't match the map".into()));
    }
    let gain_segs = vec![(0xE1, xmp_segment(&m.to_gain_map_xmp())?), (0xE2, m.to_iso())];
    let gain_jpeg =
        craft_jpeg::encode_baseline(&gain.data, gain.width, gain.height, gain.channels, quality, craft_jpeg::ChromaSubsampling::S444, &gain_segs)
            .map_err(|e| Error::Encode(e.to_string()))?;

    let xmp = craft_gainmap::primary_xmp(meta.xmp, gain_jpeg.len());
    let mut segs = jpeg_app_segments(&EncodeMeta { xmp: Some(&xmp), ..*meta })?;
    segs.push((0xE2, craft_gainmap::primary_iso_marker()));
    let sub = match sub {
        ChromaSubsampling::S444 => craft_jpeg::ChromaSubsampling::S444,
        ChromaSubsampling::S422 => craft_jpeg::ChromaSubsampling::S420,
        ChromaSubsampling::S420 => craft_jpeg::ChromaSubsampling::S420,
    };
    let primary = craft_jpeg::encode_baseline(data, w, h, base.channels as usize, quality, sub, &segs).map_err(|e| Error::Encode(e.to_string()))?;
    craft_metadata::mpf::write(&primary, &[(jpeg::MPF_GAIN_MAP, &gain_jpeg)]).map_err(|error| Error::Encode(format!("gain map JPEG: {error}")))
}

/// Rebuild the HDR rendition in `base` (linear, `width × height`) from its gain map, `weight` of the way
/// (0 = SDR, 1 = the full alternate): `craft_gainmap::apply`'s arithmetic, a row at a time on every core
/// (a 48 MP HEIC's HDR picture is rebuilt in a fraction of the time one core takes).
pub fn apply(base: &mut [[f32; 3]], width: usize, height: usize, gain: &GainImage, m: &GainMapMeta, weight: f32) -> Result<()> {
    let bad = |why: &str| Error::Encode(format!("gain map: {why}"));
    let n = width.checked_mul(height).ok_or_else(|| bad("image too large"))?;
    if base.len() < n
        || gain.width == 0
        || gain.height == 0
        || !matches!(gain.channels, 1 | 3)
        || gain.width.checked_mul(gain.height).and_then(|n| n.checked_mul(gain.channels)).is_none_or(|n| gain.data.len() < n)
    {
        return Err(bad("base and gain map sizes don't match"));
    }
    if !m.is_valid() {
        return Err(bad("unusable metadata"));
    }
    let weight = if weight.is_finite() { weight.clamp(0.0, 1.0) } else { 1.0 };
    let (sx, sy) = (gain.width as f32 / width as f32, gain.height as f32 / height as f32);
    let inv_gamma = m.gamma.map(|g| 1.0 / g);
    let Some(base) = base.get_mut(..n) else { return Err(bad("base and gain map sizes don't match")) };
    lightcraft_raster::par_rows(base, width, |row, px| {
        for (col, p) in px.iter_mut().enumerate() {
            let (gx, gy) = ((col as f32 + 0.5) * sx, (row as f32 + 0.5) * sy);
            let mut g = [0f32; 3];
            for (c, gc) in g.iter_mut().enumerate() {
                let src = if gain.channels == 3 { c } else { 0 };
                let norm = (sample(gain, gx, gy, src) / 255.0).clamp(0.0, 1.0).powf(inv_gamma[c]);
                *gc = m.min[c] + (m.max[c] - m.min[c]) * norm;
            }
            for c in 0..3 {
                let v = (p[c] + m.base_offset[c]) * (g[c] * weight).exp2() - m.alternate_offset[c];
                p[c] = v.max(0.0);
            }
        }
    });
    Ok(())
}

/// Bilinear code of channel `c` of `gain` at gain map coordinates (`x`, `y`) (pixel centres at +0.5), as
/// `craft_gainmap` samples it (its own sampler is private).
fn sample(gain: &GainImage, x: f32, y: f32, c: usize) -> f32 {
    let (w, h) = (gain.width, gain.height);
    let fx = (x - 0.5).clamp(0.0, w.saturating_sub(1) as f32);
    let fy = (y - 0.5).clamp(0.0, h.saturating_sub(1) as f32);
    let (x0, y0) = (fx as usize, fy as usize);
    let (x1, y1) = ((x0 + 1).min(w.saturating_sub(1)), (y0 + 1).min(h.saturating_sub(1)));
    let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
    let at = |x: usize, y: usize| gain.data.get((y * w + x) * gain.channels + c).copied().unwrap_or(0) as f32;
    let a = at(x0, y0) + (at(x1, y0) - at(x0, y0)) * tx;
    let b = at(x0, y1) + (at(x1, y1) - at(x0, y1)) * tx;
    a + (b - a) * ty
}

/// The gain map of a gain map JPEG.
#[derive(Clone, Debug, PartialEq)]
pub struct JpegGainMap {
    pub gain: GainImage,
    pub meta: GainMapMeta,
}

/// Find and decode the gain map of a JPEG (`None` when it has none or it's unusable). The
/// primary image is decoded as usual ([`crate::decode`]); combine the two with [`apply`].
pub fn read_jpeg(bytes: &[u8]) -> Option<JpegGainMap> {
    let m = jpeg::parse_markers(bytes)?;
    let marked = m.iso_gainmap.is_some() || m.xmp.as_deref().is_some_and(|x| x.contains("hdrgm:Version"));
    if !marked {
        return None;
    }
    for e in jpeg::mpf_images(&m) {
        let Some(data) = bytes.get(e.offset..e.offset.saturating_add(e.len)) else { continue };
        let Some(gm) = jpeg::parse_markers(data) else { continue };
        let meta = gm.iso_gainmap.as_deref().and_then(GainMapMeta::from_iso).or_else(|| gm.xmp.as_deref().and_then(GainMapMeta::from_hdrgm_xmp));
        let Some(meta) = meta else { continue };
        let Ok((w, h, ch, codes)) = jpeg::decode_codes8(data) else { continue };
        if ch == 3 && !meta.multichannel {
            // a single-channel map stored as RGB: use its first channel
            let g: Vec<u8> = codes.as_chunks::<3>().0.iter().map(|c| c[0]).collect();
            return Some(JpegGainMap { gain: GainImage { width: w, height: h, channels: 1, data: g }, meta });
        }
        let mut meta = meta;
        if ch == 1 && meta.multichannel {
            meta.multichannel = false;
            meta.fill_from_first();
        }
        return Some(JpegGainMap { gain: GainImage { width: w, height: h, channels: ch, data: codes }, meta });
    }
    None
}

/// Apple's gain map parameters for a map that reaches `stops` (log2 headroom of the HDR rendition
/// over SDR white): the codes span 0..`stops` stops of gain, no offsets, no gamma (how Skia and
/// Chrome read an iPhone HDR photo).
pub fn apple_meta(stops: f32) -> Option<GainMapMeta> {
    let m = GainMapMeta {
        multichannel: false,
        use_base_colour_space: true,
        base_headroom: 0.0,
        alternate_headroom: stops,
        min: [0.0; 3],
        max: [stops; 3],
        gamma: [1.0; 3],
        base_offset: [0.0; 3],
        alternate_offset: [0.0; 3],
    };
    m.is_valid().then_some(m)
}

/// How many stops of headroom an iPhone photo's gain map holds, from its EXIF: Apple's maker note
/// (EXIF tag 0x927C, "Apple iOS", big-endian IFD whose offsets count from the note's start) keeps
/// two numbers, tag 33 and tag 48 (signed rationals), that the published recipe turns into stops.
pub fn apple_stops(exif: &[u8]) -> Option<f32> {
    use crate::exif::Tiff;
    let t = Tiff::new(exif)?;
    let (ifd0, _) = t.ifd(t.first_ifd()?)?;
    let exif_ifd = t.uint(ifd0.iter().find(|e| e.tag == 0x8769)?)? as usize;
    let (entries, _) = t.ifd(exif_ifd)?;
    let note = t.bytes(entries.iter().find(|e| e.tag == 0x927C)?)?;
    if !note.starts_with(b"Apple iOS\0") || note.get(12..14)? != b"MM" {
        return None;
    }
    let n = Tiff::with_order(note, false);
    let (tags, _) = n.ifd(14)?;
    let rational = |tag: u16| -> Option<f32> {
        let e = tags.iter().find(|e| e.tag == tag && e.typ == 10 && e.count == 1)?;
        let at = n.u32_at(e.value_pos)? as usize;
        let (num, den) = (n.u32_at(at)? as i32, n.u32_at(at.checked_add(4)?)? as i32);
        (den != 0).then(|| num as f32 / den as f32)
    };
    let (headroom, gain) = (rational(33)?, rational(48)?);
    let stops = match (headroom < 1.0, gain <= 0.01) {
        (true, true) => -20.0 * gain + 1.8,
        (true, false) => -0.101 * gain + 1.601,
        (false, true) => -70.0 * gain + 3.0,
        (false, false) => -0.303 * gain + 2.303,
    };
    (stops.is_finite() && stops > 0.0).then_some(stops.min(8.0))
}

/// The ISO 21496-1 payload of a `tmap` item's data. The item starts with its own version byte (0), which the payload
/// ([`GainMapMeta::from_iso`]: `minimum_version`, `writer_version`, flags...) does not have; `None` for another version.
#[cfg(feature = "heif")]
fn tmap_payload(item: &[u8]) -> Option<&[u8]> {
    match item.split_first() {
        Some((0, payload)) => Some(payload),
        _ => None,
    }
}

#[cfg(feature = "heif")]
fn read_heif(bytes: &[u8]) -> Option<JpegGainMap> {
    let opts = craft_heif::Options { max_pixels: 1 << 24, apply_transforms: true, ..craft_heif::Options::default() };
    let g = craft_heif::gain_map(bytes, &opts).ok()??;
    let (w, h) = (g.map.width as usize, g.map.height as usize);
    if w == 0 || h == 0 || g.map.data.len() != w.checked_mul(h)? {
        return None;
    }
    let meta = match &g.iso {
        Some(item) => GainMapMeta::from_iso(tmap_payload(item)?)?,
        None => apple_meta(apple_stops(g.exif.as_deref()?)?)?,
    };
    let max = f32::from(g.map.max.max(1));
    let data = g.map.data.iter().map(|&v| (f32::from(v) / max * 255.0).round().clamp(0.0, 255.0) as u8).collect();
    Some(JpegGainMap { gain: GainImage { width: w, height: h, channels: 1, data }, meta })
}

/// The HDR gain map of an image file (`None` when it has none, or it is unusable): a gain map JPEG
/// (Ultra HDR, Lightroom, cameras: [`read_jpeg`]) or a HEIC (an iPhone photo's Apple gain map, or an
/// ISO 21496-1 tone-map item). The map is as stored, turned like the picture for a HEIC (the
/// container's rotation) and not for a JPEG (EXIF orientation: the caller's).
pub fn read(bytes: &[u8]) -> Option<JpegGainMap> {
    match crate::sniff(bytes)? {
        Format::Jpeg => read_jpeg(bytes),
        #[cfg(feature = "heif")]
        Format::Heif => read_heif(bytes),
        _ => None,
    }
}
/// Whether `bytes` is a gain map JPEG with a usable gain map (cheap: no pixel decode).
pub fn is_gain_map_jpeg(bytes: &[u8]) -> bool {
    crate::sniff(bytes) == Some(Format::Jpeg)
        && jpeg::parse_markers(bytes).is_some_and(|m| {
            (m.iso_gainmap.is_some() || m.xmp.as_deref().is_some_and(|x| x.contains("hdrgm:Version")))
                && jpeg::mpf_images(&m)
                    .iter()
                    .any(|e| bytes.get(e.offset..e.offset.saturating_add(e.len)).and_then(jpeg::parse_markers).is_some_and(|g| g.is_gain_map()))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn renditions(w: usize, h: usize) -> (Vec<[f32; 3]>, Vec<[f32; 3]>) {
        // a ramp: SDR rolls off at 1, HDR keeps going to 6
        let hdr: Vec<[f32; 3]> = (0..w * h)
            .map(|i| {
                let t = (i % w) as f32 / (w - 1) as f32;
                let v = 0.01 + 6.0 * t * t;
                [v, v * 0.8, v * 0.6]
            })
            .collect();
        // a luminance tone map (one scale per pixel, as LightCraft's renders do), so a single
        // luminance gain can rebuild it exactly
        let sdr = hdr
            .iter()
            .map(|p| {
                let y = 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2];
                let k = 1.0 / (1.0 + y * 0.5);
                p.map(|v| v * k)
            })
            .collect();
        (sdr, hdr)
    }

    #[test]
    fn a_gain_map_wider_than_a_jpeg_allows_is_refused() {
        let base = [0u8; 3];
        let gain = GainImage { width: 65_536, height: 1, channels: 1, data: vec![0; 65_536] };
        let r = encode_jpeg(
            &EncodeImage::new(1, 1, 3, Samples::U8(&base)),
            &gain,
            &GainMapMeta::default(),
            90,
            ChromaSubsampling::S444,
            &EncodeMeta::default(),
        );
        assert!(matches!(r, Err(Error::Encode(m)) if m.contains("65535")));
    }

    fn encode_fixture(base: &EncodeImage<'_>, gain: &GainImage, meta: &GainMapMeta, sub: ChromaSubsampling) -> Result<Vec<u8>> {
        encode_jpeg(base, gain, meta, 90, sub, &EncodeMeta::default())
    }

    #[test]
    fn gain_map_encoder_rejects_malformed_buffers_and_metadata() {
        let base = [0u8; 3];
        let rgb = EncodeImage::new(1, 1, 3, Samples::U8(&base));
        let gain = GainImage { width: 1, height: 1, channels: 1, data: vec![128] };

        let short_base = EncodeImage::new(1, 1, 4, Samples::U8(&base));
        assert!(
            matches!(encode_fixture(&short_base, &gain, &GainMapMeta::default(), ChromaSubsampling::S444), Err(Error::Encode(m)) if m.contains("RGB base"))
        );

        let short_gain = GainImage { data: Vec::new(), ..gain.clone() };
        assert!(
            matches!(encode_fixture(&rgb, &short_gain, &GainMapMeta::default(), ChromaSubsampling::S444), Err(Error::Encode(m)) if m.contains("malformed"))
        );

        let wrong_metadata = GainMapMeta { multichannel: true, ..GainMapMeta::default() };
        assert!(matches!(encode_fixture(&rgb, &gain, &wrong_metadata, ChromaSubsampling::S444), Err(Error::Encode(m)) if m.contains("metadata")));
    }

    #[test]
    fn gain_map_jpeg_writer_preserves_supported_base_layouts_and_subsampling() {
        let gain = GainImage { width: 1, height: 1, channels: 1, data: vec![128] };
        let meta = GainMapMeta::default();
        let rgb = [10u8, 20, 30];
        let rgba = [10u8, 20, 30, 255];

        for (base, sub) in [
            (EncodeImage::new(1, 1, 3, Samples::U8(&rgb)), ChromaSubsampling::S444),
            (EncodeImage::new(1, 1, 3, Samples::U8(&rgb)), ChromaSubsampling::S422),
            (EncodeImage::new(1, 1, 3, Samples::U8(&rgb)), ChromaSubsampling::S420),
            (EncodeImage::new(1, 1, 4, Samples::U8(&rgba)), ChromaSubsampling::S420),
        ] {
            let encoded = encode_fixture(&base, &gain, &meta, sub).unwrap();
            assert!(is_gain_map_jpeg(&encoded));
            let decoded = crate::decode(&encoded, crate::DecodeOptions::default()).unwrap();
            assert_eq!((decoded.width, decoded.height), (1, 1));
            assert!(read_jpeg(&encoded).is_some());
        }
    }

    #[test]
    fn jpeg_round_trip_and_container_structure() {
        let (w, h) = (96, 64);
        let (sdr, hdr) = renditions(w, h);
        let (g, m) = compute(&sdr, &hdr, w, h, [0.2126, 0.7152, 0.0722], &GainMapOptions::default()).unwrap();
        let base: Vec<u8> =
            sdr.iter().flat_map(|p| p.map(|v| (lightcraft_color::transfer::linear_to_srgb(v.clamp(0.0, 1.0)) * 255.0).round() as u8)).collect();
        let user_xmp = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/"/></rdf:RDF></x:xmpmeta>"#;
        let meta = EncodeMeta { xmp: Some(user_xmp), ..EncodeMeta::default() };
        let file = encode_jpeg(&EncodeImage::new(w as u32, h as u32, 3, Samples::U8(&base)), &g, &m, 90, ChromaSubsampling::S444, &meta).unwrap();
        assert!(is_gain_map_jpeg(&file));
        // the primary decodes as an ordinary JPEG of the right size, and keeps the user's XMP
        let d = crate::decode(&file, crate::DecodeOptions::default()).unwrap();
        assert_eq!((d.width, d.height), (w as u32, h as u32));
        let x = d.xmp.unwrap();
        assert!(x.contains("xmlns:dc=") && x.contains("hdrgm:Version") && x.contains("Item:Semantic=\"GainMap\""));
        // the MPF index points at the gain map, typed as one
        let pm = jpeg::parse_markers(&file).unwrap();
        let e = jpeg::mpf_images(&pm);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].kind, jpeg::MPF_GAIN_MAP);
        assert_eq!(e[0].offset + e[0].len, file.len());
        // and the gain map reads back
        let r = read_jpeg(&file).unwrap();
        assert_eq!((r.gain.width, r.gain.height, r.gain.channels), (48, 32, 1));
        assert!((r.meta.max[0] - m.max[0]).abs() < 1e-5);
        // the gain map is never a thumbnail
        if let Some(t) = jpeg::embedded_thumbnail(&file, 32, 16) {
            assert_ne!(t.source, crate::ThumbnailSource::MpfPreview);
        }
    }

    #[test]
    fn garbage_never_panics() {
        assert!(read_jpeg(b"").is_none());
        assert!(read_jpeg(&[0xFF, 0xD8, 0xFF, 0xE2, 0x00, 0x04, 0, 0]).is_none());
        assert!(!is_gain_map_jpeg(&[0xFF, 0xD8]));
        let mut b = vec![0xFF, 0xD8, 0xFF, 0xE2, 0, 40];
        b.extend_from_slice(ISO_URN);
        b.extend_from_slice(&[0, 0, 0, 0, 0xFF, 1, 2, 3, 4]);
        assert!(read_jpeg(&b).is_none());
        let g = GainImage { width: 2, height: 2, channels: 1, data: vec![0; 3] };
        assert!(apply(&mut [[0.0; 3]; 4], 2, 2, &g, &GainMapMeta::default(), 1.0).is_err());
        assert!(compute(&[], &[], 0, 0, [0.3, 0.6, 0.1], &GainMapOptions::default()).is_err());
    }
}
