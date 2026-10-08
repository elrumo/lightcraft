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
//! - As-shot white balance: Canon's `ColorData` array (maker-note tag `0x4001`, found in the timed-metadata sample, see
//!   [`lightcraft_meta::cr3::Cr3::color_data`]). Its first value is a layout version; the as-shot block is
//!   `R, G1, G2, B, colour temperature` (levels relative to 1024 for green) at index 71 on the versions seen on the
//!   M50 (16), EOS R (17) and 90D (19) and at 85 on the R5 and R6 (33); other versions are probed at both places and
//!   accepted only when the block looks like white-balance levels. ExifTool's Canon tag-name documentation names the
//!   array; the offsets were found by reading files (the presets that follow the as-shot block include the known
//!   5200 K daylight and 3200 K tungsten levels) and checked against the grey-world gains of the scenes.
//! - Lossy files (`C-RAW`, three wavelet levels) are decoded by [`super::crx_wavelet`]; their tile header holds ten band
//!   records per plane. One or two tiles across are supported (the seam between two tiles is described there).
//! - Variants not handled return [`RawError::Unsupported`]: the newer (R5 / R6 generation, `CMP1` version 2) lossy
//!   layout, lossy files in more tiles, roll-burst (`encType 3`) and files that do not code four planes.

use super::crx_wavelet::{self, BANDS, PlaneBands, TileEdge};
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

/// A stream of a plane (the whole plane when lossless, one band in the wavelet mode) with the quantiser value of its
/// record.
#[derive(Clone, Debug)]
struct Stream {
    range: Range<usize>,
    quant: u16,
}

/// The streams of the four planes of each tile, as byte ranges within the sample.
fn plane_streams(sample: &[u8], cmp1: &Cmp1, tiles: usize) -> Result<Vec<[Vec<Stream>; 4]>> {
    let per_plane = if cmp1.wavelet_levels == 0 { 1 } else { BANDS };
    let mut at = 0usize;
    // sizes first (the records of all tiles come before any data)
    let mut sizes: Vec<[Vec<(usize, u16)>; 4]> = Vec::with_capacity(tiles);
    for _ in 0..tiles {
        let tile_size = record(sample, at, 0xff01)? as usize;
        at += 12;
        let mut planes: [Vec<(usize, u16)>; 4] = Default::default();
        let mut tile_sum = 0usize;
        for plane in &mut planes {
            let plane_size = record(sample, at, 0xff02)? as usize;
            at += 12;
            let mut sum = 0usize;
            for band in 0..per_plane {
                let size = record(sample, at, 0xff03)? as usize;
                let field = be16(sample, at + 8).ok_or_else(|| RawError::Corrupt("CRX header truncated".into()))?;
                at += 12;
                if per_plane > 1 && (field >> 12) as usize != band {
                    return Err(RawError::Corrupt("CRX subband records out of order".into()));
                }
                sum = sum.checked_add(size).ok_or_else(|| RawError::Corrupt("CRX plane size".into()))?;
                plane.push((size, field));
            }
            if sum != plane_size {
                return Err(RawError::Corrupt("CRX plane size differs from its subbands".into()));
            }
            tile_sum = tile_sum.checked_add(plane_size).ok_or_else(|| RawError::Corrupt("CRX tile size".into()))?;
        }
        if tile_sum != tile_size {
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
        let mut tile: [Vec<Stream>; 4] = Default::default();
        for (streams, bands) in tile.iter_mut().zip(planes) {
            for (size, quant) in bands {
                let end =
                    start.checked_add(size).filter(|e| *e <= sample.len()).ok_or_else(|| RawError::Corrupt("CRX plane outside the sample".into()))?;
                streams.push(Stream { range: start..end, quant });
                start = end;
            }
        }
        out.push(tile);
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

/// As-shot white-balance multipliers (R, G, B with G = 1) from the `ColorData` array (see the module docs).
fn wb_from_color_data(v: &[u16]) -> Option<[f32; 3]> {
    let block = |at: usize| -> Option<[f32; 3]> {
        let q = v.get(at..at + 5)?;
        let (r, g1, g2, b, temp) = (q[0] as f32, q[1] as f32, q[2] as f32, q[3] as f32, q[4]);
        let plausible = g1 >= 256.0
            && (g1 - g2).abs() <= 0.02 * g1
            && r >= 0.25 * g1
            && b >= 0.25 * g1
            && r <= 6.0 * g1
            && b <= 6.0 * g1
            && (1500..=16000).contains(&temp);
        plausible.then(|| {
            let g = (g1 + g2) / 2.0;
            [r / g, 1.0, b / g]
        })
    };
    let first = match v.first()? {
        16 | 17 | 19 => 71,
        33 => 85,
        _ => 0,
    };
    [first, 71, 85].into_iter().filter(|&at| at > 0).find_map(block)
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
    let track = cr3.raw_track().ok_or_else(|| RawError::Unsupported("CR3 without a raw track".into()))?;
    let (Cr3TrackKind::Raw { cmp1: Some(cmp1), iad1, .. }, Some((at, len))) = (&track.kind, track.data) else {
        return Err(RawError::Unsupported("CR3 raw track without a coding header".into()));
    };
    if cmp1.enc_type != 0 {
        return Err(RawError::Unsupported("Canon roll-burst raw (CRX encType 3)".into()));
    }
    if cmp1.wavelet_levels != 0 && (cmp1.wavelet_levels != 3 || cmp1.version != 0x100) {
        return Err(RawError::Unsupported(format!(
            "Canon C-RAW (lossy CRX version {:#x} with {} wavelet levels)",
            cmp1.version, cmp1.wavelet_levels
        )));
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
    if cmp1.wavelet_levels != 0 && (tiles_y != 1 || tiles_x > 2) {
        return Err(RawError::Unsupported(format!("Canon C-RAW in {tiles_x} × {tiles_y} tiles")));
    }
    let sample = at.checked_add(len).and_then(|end| bytes.get(at..end)).ok_or_else(|| RawError::Corrupt("CRX sample outside the file".into()))?;
    // no real file packs a mosaic into less than a few thousandth of a bit per sample; this bounds what a header can make us allocate
    if width * height / (sample.len() + 1) > 4096 {
        return Err(RawError::Corrupt("CRX sample too small for its geometry".into()));
    }
    let streams = plane_streams(sample, cmp1, tiles)?;

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
                let planes = streams.get(tile).and_then(|t| t.get(p)).ok_or_else(|| RawError::Corrupt("CRX plane".into()))?;
                let slice = |s: &Stream| sample.get(s.range.clone()).ok_or_else(|| RawError::Corrupt("CRX plane".into()));
                // an unseen stream variant (or damage) falls back to the embedded preview like other unsupported raws
                let plane = if cmp1.wavelet_levels == 0 {
                    planes
                        .first()
                        .ok_or_else(|| RawError::Corrupt("CRX plane".into()))
                        .and_then(|s| crx::decode_plane(slice(s)?, w / 2, h / 2, cmp1.bits as u32))
                } else {
                    let mut input = PlaneBands { streams: [&[]; BANDS], steps: [1; BANDS] };
                    for ((s, stream), step) in planes.iter().zip(input.streams.iter_mut()).zip(input.steps.iter_mut()) {
                        *stream = slice(s)?;
                        *step = crx_wavelet::band_step(s.quant);
                    }
                    let edge = match (tiles_x, tx) {
                        (1, _) => TileEdge::Only,
                        (_, 0) => TileEdge::First,
                        _ => TileEdge::Last,
                    };
                    crx_wavelet::decode_plane(&input, w / 2, h / 2, edge, cmp1.bits as u32)
                };
                plane.map_err(|e| match e {
                    RawError::Corrupt(why) => RawError::Unsupported(format!("Canon CRX data not decoded ({why})")),
                    other => other,
                })
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
    let wb_multipliers = cr3.color_data(bytes).and_then(|v| wb_from_color_data(&v));
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
        wb_multipliers,
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
    fn cmp1(tile_w: usize, header_size: u32, cfa: u8, enc: u8, levels: u8, version: u8) -> Vec<u8> {
        let mut b = vec![0xff, 0, 0, 0x30, version, 0, 0, 0];
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
        build(m, tile_w, cfa, enc, levels, 1, None)
    }

    /// The same as a wavelet (C-RAW) file whose band records carry these quantiser values.
    fn lossy_file(m: &[u16], tile_w: usize, quant: [u16; BANDS]) -> Vec<u8> {
        build(m, tile_w, 0, 0, 3, 1, Some(quant))
    }

    fn build(m: &[u16], tile_w: usize, cfa: u8, enc: u8, levels: u8, version: u8, lossy: Option<[u16; BANDS]>) -> Vec<u8> {
        let tiles = W.div_ceil(tile_w);
        // each tile is a list of streams per plane: one for lossless, ten bands for the wavelet mode
        let mut streams: Vec<[Vec<Vec<u8>>; 4]> = Vec::new();
        for t in 0..tiles {
            let x0 = t * tile_w;
            let tw = (W - x0).min(tile_w);
            streams.push(std::array::from_fn(|p| match lossy {
                None => vec![encode_plane(&plane_of(m, x0, tw, p), tw / 2, H / 2, 14)],
                Some(quant) => {
                    let edge = match (tiles, t) {
                        (1, _) => TileEdge::Only,
                        (_, 0) => TileEdge::First,
                        _ => TileEdge::Last,
                    };
                    let steps: [i32; BANDS] = std::array::from_fn(|b| crx_wavelet::band_step(quant[b]));
                    let global = plane_of(m, 0, W, p);
                    crx_wavelet::testenc::encode_tile_plane(&global, W / 2, H / 2, x0 / 2, tw / 2, edge, 14, &steps).into_iter().collect()
                }
            }));
        }
        let per_tile = 12 + 4 * (12 + 12 * streams.first().and_then(|t| t.first()).map_or(1, Vec::len));
        let header_size = (tiles * per_tile).next_multiple_of(8);
        let mut sample = Vec::new();
        for s in &streams {
            sample.extend([0xff, 0x01, 0, 8]);
            sample.extend((s.iter().flatten().map(Vec::len).sum::<usize>() as u32).to_be_bytes());
            sample.extend([0; 4]);
            for (p, plane) in s.iter().enumerate() {
                sample.extend([0xff, 0x02, 0, 8]);
                sample.extend((plane.iter().map(Vec::len).sum::<usize>() as u32).to_be_bytes());
                sample.extend([(p as u8) << 4 | 8, 0, 0, 0]);
                for (b, band) in plane.iter().enumerate() {
                    sample.extend([0xff, 0x03, 0, 8]);
                    sample.extend((band.len() as u32).to_be_bytes());
                    match lossy {
                        None => sample.extend([0, 0x20, 0, 5]),
                        Some(quant) => sample.extend([((b as u16) << 12 | quant[b]).to_be_bytes(), [0, 5]].concat()),
                    }
                }
            }
        }
        sample.resize(header_size, 0);
        for s in &streams {
            for band in s.iter().flatten() {
                sample.extend(band);
            }
        }
        // crop, left black, top black, active (inclusive; the crop is absolute)
        let areas = [[20, 12, 91, 59], [0, 0, 15, 63], [16, 0, 95, 7], [16, 8, 95, 63]];
        let payload_at = 4096u64;
        let mut f = bx(b"ftyp", b"crx \0\0\0\x01crx isom");
        let trak = raw_trak(&cmp1(tile_w, header_size as u32, cfa, enc, levels, version), &areas, payload_at, sample.len() as u32);
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
    fn white_balance_comes_from_the_as_shot_block() {
        let mut v = vec![0u16; 120];
        v[85..90].copy_from_slice(&[1577, 1024, 1024, 2503, 3726]);
        // the R5 / R6 layout (version 33) reads index 85
        v[0] = 33;
        let wb = wb_from_color_data(&v).unwrap();
        assert!((wb[0] - 1577.0 / 1024.0).abs() < 1e-5 && wb[1] == 1.0 && (wb[2] - 2503.0 / 1024.0).abs() < 1e-5);
        // an unknown version is probed and accepted when the block is plausible
        v[0] = 99;
        assert_eq!(wb_from_color_data(&v), Some(wb));
        // the M50 / R / 90D layout reads index 71
        let mut w = vec![0u16; 120];
        w[0] = 17;
        w[71..76].copy_from_slice(&[2001, 1024, 1025, 1582, 4833]);
        assert!((wb_from_color_data(&w).unwrap()[0] - 2001.0 / 1024.5).abs() < 1e-5);
        // implausible blocks (unequal greens, a temperature outside the range) and short arrays give nothing
        w[72] = 600;
        assert_eq!(wb_from_color_data(&w), None);
        assert_eq!(wb_from_color_data(&[33; 10]), None);
        assert_eq!(wb_from_color_data(&[]), None);
    }

    #[test]
    fn wavelet_tiles_round_trip_with_geometry() {
        let m = mosaic();
        // unit steps make the integer wavelet reversible; a single tile and a pair with a seam
        for tile_w in [W, W / 2] {
            let img = decode(&lossy_file(&m, tile_w, [0x20; BANDS]), Mode::Full).unwrap();
            let RawData::U16(d) = &img.data else { panic!("float data") };
            let bad: Vec<(usize, usize, u16, u16)> =
                d.iter().zip(&m).enumerate().filter(|(_, (a, b))| a != b).map(|(i, (&a, &b))| (i % W, i / W, a, b)).collect();
            assert!(bad.is_empty(), "tile width {tile_w}: {} wrong, first {:?}", bad.len(), &bad[..bad.len().min(12)]);
            assert_eq!((img.width, img.height, img.bits), (W, H, 14));
            assert_eq!(img.active_area, Rect::new(16, 8, 80, 56));
        }
        // with the quantiser values of the cameras the mosaic is only close
        let img = decode(&lossy_file(&m, W / 2, [0x20, 0x20, 0x20, 0x20, 0x80, 0x80, 0xb0, 0xd0, 0xd0, 0x100]), Mode::Full).unwrap();
        let RawData::U16(d) = &img.data else { panic!("float data") };
        let (worst, mean) = d.iter().zip(&m).fold((0i32, 0i64), |(w, s), (&a, &b)| {
            let e = (a as i32 - b as i32).abs();
            (w.max(e), s + e as i64)
        });
        assert!(worst > 0 && worst < 200 && mean / (d.len() as i64) < 20, "worst {worst} mean {}", mean / d.len() as i64);
    }

    #[test]
    fn unsupported_variants_say_so() {
        let m = mosaic();
        // two levels, a newer stream version, three tiles across, roll-burst
        let cases = [
            (file(&m, W, 0, 0, 2), "C-RAW"),
            (build(&m, W, 0, 0, 3, 2, Some([0x20; BANDS])), "C-RAW"),
            (lossy_file(&m, 32, [0x20; BANDS]), "tiles"),
            (file(&m, W, 0, 3, 0), "roll-burst"),
        ];
        for (f, what) in cases {
            let r = decode(&f, Mode::Full);
            assert!(matches!(r, Err(RawError::Unsupported(ref s)) if s.contains(what)), "{what}: {r:?}");
        }
        // header-only mode never needs the wavelet data
        assert!(decode(&lossy_file(&m, W / 2, [0x20; BANDS]), Mode::Header).is_ok());
    }

    #[test]
    fn wavelet_records_must_be_consistent() {
        let m = mosaic();
        let mut f = lossy_file(&m, W, [0x20; BANDS]);
        // the sample starts at 4096: tile record, plane record, then the band records; swap the first two band indices
        let band0 = 4096 + 12 + 12;
        f[band0 + 8] = 0x10;
        let r = decode(&f, Mode::Full);
        assert!(matches!(r, Err(RawError::Corrupt(_))), "{r:?}");
    }

    #[test]
    fn damaged_wavelet_files_are_errors_not_panics() {
        let m = mosaic();
        let f = lossy_file(&m, W / 2, [0x20, 0x20, 0x20, 0x20, 0x80, 0x80, 0xb0, 0xd0, 0xd0, 0x100]);
        for n in (0..f.len()).step_by(53) {
            let _ = decode(&f[..n], Mode::Full);
        }
        let damage = (0..600).chain(4096..4096 + 1100).chain((4096 + 1100..f.len()).step_by(61));
        for i in damage {
            for v in [0u8, 0x7f, 0xff] {
                let mut g = f.clone();
                g[i] = v;
                let _ = decode(&g, Mode::Full);
            }
        }
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
