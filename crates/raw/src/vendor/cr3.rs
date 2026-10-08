//! Canon CR3: container glue around the CRX codec in [`super::crx`].
//!
//! Source: Laurent Clévy's CR3 notes (prose: the ISO-BMFF box layout, the `CRAW` raw tracks with their `CMP1` coding
//! header and `CDI1`/`IAD1` image-area box, the `CMT1..4` Exif blocks), parsed by [`lightcraft_meta::cr3`]. The
//! tile / plane / subband records are described in [`super::crx`].
//!
//! - The full-size raw track is the largest `CRAW` raw entry; its sample is `CMP1.header_size` bytes of tile
//!   headers followed by the tiles' data. Tiles tile the sensor data (`CMP1.width × height`, masked borders
//!   included) in row-major order, each holding the four Bayer planes of its area.
//! - Geometry: `IAD1` gives the optically black columns / rows and the active area (zero-based, inclusive; it can
//!   overflow the data by a few pixels, so it is clamped) and the recommended crop, which is absolute and made relative
//!   to the active area here. The Bayer layout is `CMP1`'s `cfaLayout` (0 = RGGB).
//! - Black level is measured from the optically black columns on the left; white from the data.
//! - Variants not handled return [`RawError::Unsupported`]: the lossy wavelet mode (`C-RAW`), roll-burst
//!   (`encType 3`) and files that do not code four planes.

use super::{black_from_columns, crx, white_from_data};
use crate::{BlackLevel, Cfa, ColorData, Mode, OpcodeLists, RawData, RawError, RawFormat, RawImage, Rect, Result};
use lightcraft_geom::Orientation;
use lightcraft_meta::cr3::{Cmp1, Cr3TrackKind, Iad1Areas, Iad1Rect, parse_cr3};
use lightcraft_tiff::{Tiff, tags as t};
use rayon::prelude::*;
use std::ops::Range;

/// Largest tile count we accept (real files have 1–2).
const MAX_TILES: usize = 1024;

fn be32(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at.checked_add(4)?).map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}
fn be16(b: &[u8], at: usize) -> Option<u16> {
    b.get(at..at.checked_add(2)?).map(|s| u16::from_be_bytes([s[0], s[1]]))
}

/// A `ffXX 0008` header record: its 32-bit size field.
fn record(sample: &[u8], at: usize, marker: u16) -> Result<u32> {
    if be16(sample, at) != Some(marker) || be16(sample, at + 2) != Some(8) {
        return Err(RawError::Corrupt(format!("CRX header record {marker:#06x} missing")));
    }
    be32(sample, at + 4).ok_or_else(|| RawError::Corrupt("CRX header truncated".into()))
}

/// Byte ranges of the four planes of each tile, within the sample.
fn plane_ranges(sample: &[u8], cmp1: &Cmp1, tiles: usize) -> Result<Vec<[Range<usize>; 4]>> {
    let mut at = 0usize;
    let mut sizes = Vec::with_capacity(tiles);
    for _ in 0..tiles {
        let tile_size = record(sample, at, 0xff01)? as usize;
        at += 12;
        let mut planes = [0usize; 4];
        for p in &mut planes {
            *p = record(sample, at, 0xff02)? as usize;
            at += 12;
            let sub = record(sample, at, 0xff03)? as usize;
            at += 12;
            if sub != *p {
                return Err(RawError::Unsupported("CRX plane with several subbands".into()));
            }
        }
        if planes.iter().try_fold(0usize, |s, &p| s.checked_add(p)) != Some(tile_size) {
            return Err(RawError::Corrupt("CRX tile size differs from its planes".into()));
        }
        sizes.push(planes);
    }
    let mut start = cmp1.header_size as usize;
    if start < at {
        return Err(RawError::Corrupt("CRX header size smaller than its records".into()));
    }
    let mut out = Vec::with_capacity(tiles);
    for planes in sizes {
        let mut r: [Range<usize>; 4] = [0..0, 0..0, 0..0, 0..0];
        for (range, size) in r.iter_mut().zip(planes) {
            let end =
                start.checked_add(size).filter(|e| *e <= sample.len()).ok_or_else(|| RawError::Corrupt("CRX plane outside the sample".into()))?;
            *range = start..end;
            start = end;
        }
        out.push(r);
    }
    Ok(out)
}

fn rect_of(r: Iad1Rect, width: usize, height: usize) -> Option<Rect> {
    let (x, y) = (r.left as usize, r.top as usize);
    if x >= width || y >= height {
        return None;
    }
    let right = (r.right as usize).min(width - 1);
    let bottom = (r.bottom as usize).min(height - 1);
    (right >= x && bottom >= y).then(|| Rect::new(x, y, right - x + 1, bottom - y + 1))
}

/// Active area and default crop (relative to the active area) from `IAD1`.
fn geometry(areas: Option<&Iad1Areas>, width: usize, height: usize) -> (Rect, Rect) {
    let whole = Rect::new(0, 0, width, height);
    let Some(a) = areas else { return (whole, whole) };
    let active = rect_of(a.active, width, height).unwrap_or(whole);
    let crop = rect_of(a.crop, width, height)
        .and_then(|c| {
            let (x, y) = (c.x.checked_sub(active.x)?, c.y.checked_sub(active.y)?);
            (x + c.width <= active.width && y + c.height <= active.height).then(|| Rect::new(x, y, c.width, c.height))
        })
        .unwrap_or(Rect::new(0, 0, active.width, active.height));
    (active, crop)
}

fn cfa_of(layout: u8) -> Cfa {
    Cfa::bayer_static(match layout {
        1 => "GRBG",
        2 => "GBRG",
        3 => "BGGR",
        _ => "RGGB",
    })
}

pub(crate) fn decode(bytes: &[u8], mode: Mode) -> Result<RawImage> {
    let cr3 = parse_cr3(bytes).ok_or_else(|| RawError::Corrupt("CR3 container".into()))?;
    let track = cr3.raw_track().ok_or_else(|| RawError::Corrupt("CR3 without a raw track".into()))?;
    let (Cr3TrackKind::Raw { cmp1: Some(cmp1), iad1, .. }, Some((at, len))) = (&track.kind, track.data) else {
        return Err(RawError::Corrupt("CR3 raw track without coding header".into()));
    };
    if cmp1.enc_type != 0 {
        return Err(RawError::Unsupported("Canon roll-burst raw (CRX encType 3)".into()));
    }
    if cmp1.wavelet_levels != 0 {
        return Err(RawError::Unsupported("Canon C-RAW (lossy CRX)".into()));
    }
    if cmp1.planes != 4 || !(8..=16).contains(&cmp1.bits) {
        return Err(RawError::Unsupported(format!("CRX with {} planes at {} bits", cmp1.planes, cmp1.bits)));
    }
    let (width, height) = (cmp1.width as usize, cmp1.height as usize);
    let (tw, th) = (cmp1.tile_width as usize, cmp1.tile_height as usize);
    if width == 0 || height == 0 || tw == 0 || th == 0 || tw % 2 != 0 || th % 2 != 0 || width % 2 != 0 || height % 2 != 0 {
        return Err(RawError::Corrupt("bad CRX geometry".into()));
    }
    if width.checked_mul(height).is_none_or(|n| n > crate::MAX_SAMPLES) {
        return Err(RawError::Limit("CR3 image larger than expected"));
    }
    let (tiles_x, tiles_y) = (width.div_ceil(tw), height.div_ceil(th));
    let tiles = tiles_x.checked_mul(tiles_y).filter(|n| *n <= MAX_TILES).ok_or(RawError::Limit("too many CRX tiles"))?;
    let sample = at.checked_add(len).and_then(|end| bytes.get(at..end)).ok_or_else(|| RawError::Corrupt("CRX sample outside the file".into()))?;
    let ranges = plane_ranges(sample, cmp1, tiles)?;

    let (active, crop) = geometry(iad1.as_ref().and_then(|i| i.areas.as_ref()), width, height);
    let mut data = Vec::new();
    if mode == Mode::Full {
        // every plane is an independent stream
        let jobs: Vec<(usize, usize)> = (0..tiles).flat_map(|t| (0..4).map(move |p| (t, p))).collect();
        let planes: Vec<Vec<u16>> = jobs
            .par_iter()
            .map(|&(tile, p)| {
                let (tx, ty) = (tile % tiles_x, tile / tiles_x);
                let (w, h) = ((width - tx * tw).min(tw), (height - ty * th).min(th));
                let span = ranges
                    .get(tile)
                    .and_then(|r| r.get(p))
                    .and_then(|r| sample.get(r.clone()))
                    .ok_or_else(|| RawError::Corrupt("CRX plane".into()))?;
                crx::decode_plane(span, w / 2, h / 2, cmp1.bits as u32)
            })
            .collect::<Result<_>>()?;
        data = vec![0u16; width * height];
        for (&(tile, p), plane) in jobs.iter().zip(&planes) {
            let (tx, ty) = (tile % tiles_x, tile / tiles_x);
            let (x0, y0) = (tx * tw, ty * th);
            let (w, h) = ((width - x0).min(tw), (height - y0).min(th));
            let pw = w / 2;
            for (yp, row) in plane.chunks_exact(pw.max(1)).enumerate().take(h / 2) {
                let base = (y0 + 2 * yp + (p >> 1)) * width + x0 + (p & 1);
                for (xp, &v) in row.iter().enumerate() {
                    if let Some(s) = data.get_mut(base + 2 * xp) {
                        *s = v;
                    }
                }
            }
        }
    }

    let black = if mode == Mode::Full && active.x >= 8 {
        black_from_columns(&data, width, 2..active.x - 2, active.y..active.y + active.height, active)
    } else {
        BlackLevel::uniform(0.0)
    };
    let white = if mode == Mode::Full { white_from_data(&data, cmp1.bits as u32) } else { ((1u32 << cmp1.bits) - 1) as f32 };

    let orientation = cr3
        .cmt
        .first()
        .copied()
        .flatten()
        .and_then(|b| Tiff::parse(b).ok())
        .and_then(|tf| tf.ifds.first().and_then(|i| i.u16(t::ORIENTATION)))
        .map(Orientation::from_exif)
        .unwrap_or(Orientation::Normal);
    let mut metadata = lightcraft_meta::extract(bytes);
    metadata.width = Some(crop.width as u32);
    metadata.height = Some(crop.height as u32);
    let img = RawImage {
        format: RawFormat::Cr3,
        width,
        height,
        cpp: 1,
        data: RawData::U16(data),
        cfa: Some(cfa_of(cmp1.cfa_layout)),
        bits: cmp1.bits as u32,
        black,
        white: vec![white],
        active_area: active,
        crop,
        orientation,
        color: ColorData::default(),
        wb_multipliers: None,
        linearized: false,
        opcodes: OpcodeLists::default(),
        metadata,
    };
    img.validate_for(mode)?;
    Ok(img)
}

#[cfg(test)]
mod tests {
    use super::super::crx::testenc::encode_plane;
    use super::*;

    const W: usize = 96;
    const H: usize = 64;
    /// Optically black columns on the left and rows on top, as in the real files.
    const BLACK_COLS: usize = 16;
    const BLACK_ROWS: usize = 8;

    fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(body);
        v
    }
    fn full(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut b = vec![0u8; 4];
        b.extend_from_slice(body);
        bx(kind, &b)
    }

    /// A `CRAW` raw track entry with `CMP1` and `CDI1`/`IAD1` children, whose sample is `size` bytes at `offset`.
    fn raw_trak(cmp1: &[u8], areas: &[[u16; 4]; 4], offset: u64, size: u32) -> Vec<u8> {
        let mut iad = vec![0u8; 4];
        for v in [W as u16, H as u16, 1, 2, 1, 0] {
            iad.extend(v.to_be_bytes());
        }
        for v in areas.concat() {
            iad.extend(v.to_be_bytes());
        }
        let children = [bx(b"CMP1", cmp1), full(b"CDI1", &bx(b"IAD1", &iad))].concat();
        let mut entry = vec![0u8; 82];
        entry[24..26].copy_from_slice(&(W as u16).to_be_bytes());
        entry[26..28].copy_from_slice(&(H as u16).to_be_bytes());
        entry.extend(children);
        let mut stsd = 1u32.to_be_bytes().to_vec();
        stsd.extend(bx(b"CRAW", &entry));
        let mut stsz = 0u32.to_be_bytes().to_vec();
        stsz.extend(1u32.to_be_bytes());
        stsz.extend(size.to_be_bytes());
        let mut co64 = 1u32.to_be_bytes().to_vec();
        co64.extend(offset.to_be_bytes());
        let stbl = [full(b"stsd", &stsd), full(b"stsz", &stsz), full(b"co64", &co64)].concat();
        bx(b"trak", &bx(b"mdia", &bx(b"minf", &bx(b"stbl", &stbl))))
    }

    /// A `CMP1` payload: 14-bit, four planes, `cfa` layout, `levels` wavelet levels, `enc` type.
    fn cmp1(tile_w: usize, header_size: u32, cfa: u8, enc: u8, levels: u8) -> Vec<u8> {
        let mut b = vec![0xff, 0, 0, 0x30, 1, 0, 0, 0];
        for v in [W, H, tile_w, H] {
            b.extend((v as u32).to_be_bytes());
        }
        b.extend([14, 0x40 | cfa, enc << 4 | levels, if tile_w < W { 0x80 } else { 0 }]);
        b.extend(header_size.to_be_bytes());
        b.extend([1, 1, 0, 0].repeat(4));
        b
    }

    /// A smooth mosaic with flat patches (runs) and a noisy optical-black frame around the left / top.
    fn mosaic() -> Vec<u16> {
        let mut state = 12345u64;
        let mut rnd = move || {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (state >> 33) as u32
        };
        let mut m = vec![0u16; W * H];
        for y in 0..H {
            for x in 0..W {
                let black = x < BLACK_COLS || y < BLACK_ROWS;
                m[y * W + x] = if black {
                    (512 + rnd() % 3) as u16
                } else if (x / 9 + y / 7) % 3 == 0 {
                    4000
                } else {
                    (900 + x * 40 + y * 25 + (rnd() % 30) as usize) as u16
                };
            }
        }
        m
    }

    /// Plane `p` of the tile at `x0` of width `tw`.
    fn plane_of(m: &[u16], x0: usize, tw: usize, p: usize) -> Vec<u16> {
        let (pw, ph) = (tw / 2, H / 2);
        (0..ph).flat_map(|yp| (0..pw).map(move |xp| m[(2 * yp + (p >> 1)) * W + x0 + 2 * xp + (p & 1)])).collect()
    }

    /// A whole file: `tile_w` wide tiles of `m`, the full-size raw last.
    fn file(m: &[u16], tile_w: usize, cfa: u8, enc: u8, levels: u8) -> Vec<u8> {
        let tiles = W.div_ceil(tile_w);
        let mut streams: Vec<[Vec<u8>; 4]> = Vec::new();
        for t in 0..tiles {
            let x0 = t * tile_w;
            let tw = (W - x0).min(tile_w);
            streams.push(std::array::from_fn(|p| encode_plane(&plane_of(m, x0, tw, p), tw / 2, H / 2, 14)));
        }
        let header_size = (tiles * 108).next_multiple_of(8);
        let mut sample = Vec::new();
        for s in &streams {
            sample.extend([0xff, 0x01, 0, 8]);
            sample.extend((s.iter().map(Vec::len).sum::<usize>() as u32).to_be_bytes());
            sample.extend([0; 4]);
            for (p, plane) in s.iter().enumerate() {
                sample.extend([0xff, 0x02, 0, 8]);
                sample.extend((plane.len() as u32).to_be_bytes());
                sample.extend([(p as u8) << 4 | 8, 0, 0, 0]);
                sample.extend([0xff, 0x03, 0, 8]);
                sample.extend((plane.len() as u32).to_be_bytes());
                sample.extend([0, 0x20, 0, 5]);
            }
        }
        sample.resize(header_size, 0);
        for s in &streams {
            for plane in s {
                sample.extend(plane);
            }
        }
        // crop, left black, top black, active (inclusive; the crop is absolute)
        let areas = [[20, 12, 91, 59], [0, 0, 15, 63], [16, 0, 95, 7], [16, 8, 95, 63]];
        let payload_at = 4096u64;
        let mut f = bx(b"ftyp", b"crx \0\0\0\x01crx isom");
        let trak = raw_trak(&cmp1(tile_w, header_size as u32, cfa, enc, levels), &areas, payload_at, sample.len() as u32);
        f.extend(bx(b"moov", &trak));
        f.resize(payload_at as usize, 0);
        f.extend(sample);
        f
    }

    #[test]
    fn lossless_tiles_round_trip_with_geometry() {
        let m = mosaic();
        for tile_w in [W, W / 2, 32] {
            let f = file(&m, tile_w, 0, 0, 0);
            let img = decode(&f, Mode::Full).unwrap();
            let RawData::U16(d) = &img.data else { panic!("float data") };
            assert_eq!(*d, m, "tile width {tile_w}");
            assert_eq!((img.width, img.height, img.bits), (W, H, 14));
            assert_eq!(img.format, RawFormat::Cr3);
            assert_eq!(img.active_area, Rect::new(16, 8, 80, 56));
            assert_eq!(img.crop, Rect::new(4, 4, 72, 48));
            assert_eq!(img.cfa.as_ref().map(|c| c.name()), Some("RGGB".to_string()));
            // black measured from the masked columns (512..514)
            assert!(img.black.values.iter().all(|v| (511.5..514.5).contains(v)), "{:?}", img.black.values);
            // header-only mode agrees without decoding any plane
            let info = decode(&f, Mode::Header).unwrap();
            assert!(info.data.is_empty() && info.info().active_area == img.info().active_area && info.info().crop == img.info().crop);
        }
        for (layout, name) in [(1u8, "GRBG"), (2, "GBRG"), (3, "BGGR")] {
            let img = decode(&file(&m, W, layout, 0, 0), Mode::Full).unwrap();
            assert_eq!(img.cfa.as_ref().map(|c| c.name()), Some(name.to_string()));
        }
    }

    #[test]
    fn lossy_and_roll_burst_variants_are_unsupported() {
        let m = mosaic();
        let lossy = decode(&file(&m, W, 0, 0, 3), Mode::Full);
        assert!(matches!(lossy, Err(RawError::Unsupported(ref s)) if s.contains("C-RAW")), "{lossy:?}");
        let roll = decode(&file(&m, W, 0, 3, 0), Mode::Full);
        assert!(matches!(roll, Err(RawError::Unsupported(_))), "{roll:?}");
    }

    #[test]
    fn damaged_files_are_errors_not_panics() {
        let m = mosaic();
        let f = file(&m, 32, 0, 0, 0);
        // every truncation, and single-byte damage across the headers and the start of the data
        for n in (0..f.len()).step_by(37) {
            let _ = decode(&f[..n], Mode::Full);
        }
        assert!(decode(&f[..f.len() - 40], Mode::Full).is_err(), "a short last plane");
        // the box tree (before the zero padding) and the tile headers, then the plane data
        let damage = (0..600).chain(4096..4096 + 330).chain((4096 + 330..f.len()).step_by(97));
        for i in damage {
            for v in [0u8, 0x7f, 0xff] {
                let mut g = f.clone();
                g[i] = v;
                let _ = decode(&g, Mode::Full);
            }
        }
    }
}
