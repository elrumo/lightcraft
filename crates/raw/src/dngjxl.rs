//! JPEG XL tiles for a DNG (compression 52546), written with the pure-Rust `jixel` encoder.
//!
//! Only three-sample 16-bit linear data (Apple ProRAW and the other `LinearRaw` DNGs) is tiled: `jixel` codes a
//! one-plane image as three channels, so a mosaic tile would only grow. [`crate::DngCompression::Jxl`] is lossless
//! for `distance: None`. For `Some(distance)` (a butteraugli distance; 1 is visually lossless) the samples are first
//! put on a gamma curve and a 65536-entry `LinearizationTable` maps the decoded codes back to linear: lossy coding of
//! linear values would spend its error budget on the highlights and wreck the shadows. The reader
//! (`craft_raw::jxl`) returns a lossy tile's samples as stored, so the table is applied on top, as for lossy JPEG.

use crate::{RawError, Result};
use rayon::prelude::*;

/// Largest tile edge: tile buffers grow with the square (see `dngwrite::MAX_DEFLATE_TILE`).
pub(crate) const MAX_TILE: u32 = 2048;
/// Distance range a request is clamped to. Below the minimum the file is not smaller than lossless; above the
/// maximum the picture falls apart.
pub(crate) const DISTANCE_RANGE: (f32, f32) = (0.05, 15.0);
/// Gamma of the lossy codes (the same power curve as the lossy JPEG DNG).
const GAMMA: f64 = 2.2;

/// Most distinct sample values a lossless tile DNG recodes as a palette (see [`palette`]).
const MAX_PALETTE: usize = 4096;

/// A sparse sample set as dense codes: when `samples` hold few distinct values (Apple ProRAW's 16-bit samples sit on a
/// lattice of about 1600 values, the 11-bit codes of its own linearization table) the values are replaced by their rank
/// and a `LinearizationTable` (rank → value) maps them back exactly, as the DNG that came out of the camera did. The
/// coder then sees 11-bit codes, not a 16-bit lattice whose gaps it would have to learn: the lossless file is about a
/// third of the size. `None` when the values are too many (or all the same) for that to pay.
pub(crate) fn palette(samples: &[u16]) -> Option<(Vec<u16>, Vec<u16>)> {
    let mut present = vec![false; 65536];
    for &v in samples {
        present[usize::from(v)] = true;
    }
    let table: Vec<u16> = (0..=65535u16).filter(|&v| present[usize::from(v)]).collect();
    if table.len() < 2 || table.len() > MAX_PALETTE {
        return None;
    }
    let mut rank = vec![0u16; 65536];
    for (i, &v) in table.iter().enumerate() {
        rank[usize::from(v)] = i as u16;
    }
    let codes = samples.iter().map(|&v| rank[usize::from(v)]).collect();
    Some((table, codes))
}

/// `tile` aligned down to a multiple of 16 and clamped to [`MAX_TILE`].
pub(crate) fn tile_edge(tile: u32) -> usize {
    (tile.max(16) & !15).min(MAX_TILE) as usize
}

/// [`tile_edge`] made small enough for the picture: the readers (this one included) refuse a file whose tile grid holds
/// more than four times the picture's samples, so a tile edge far above the picture's is halved until the grid fits.
pub(crate) fn fit_tile(tile: u32, w: usize, h: usize) -> usize {
    let mut edge = tile_edge(tile);
    let area = w.saturating_mul(h);
    while edge > 16 && w.div_ceil(edge).saturating_mul(h.div_ceil(edge)).saturating_mul(edge * edge) > area.saturating_mul(4) {
        edge = (edge / 2) & !15;
        edge = edge.max(16);
    }
    edge
}

/// The distance a request means: clamped, and an error for NaN or infinity (which no clamp can read as intent).
pub(crate) fn clean_distance(distance: f32) -> Result<f32> {
    if distance.is_finite() {
        Ok(distance.clamp(DISTANCE_RANGE.0, DISTANCE_RANGE.1))
    } else {
        Err(RawError::Corrupt(format!("JPEG XL distance {distance} is not a number")))
    }
}

/// Linear sample (0..=65535) → lossy code.
fn code_of(v: u16) -> u16 {
    ((f64::from(v) / 65535.0).powf(1.0 / GAMMA) * 65535.0).round().clamp(0.0, 65535.0) as u16
}

/// The `LinearizationTable` of a lossy tile DNG: code → linear sample.
pub(crate) fn linearization() -> Vec<u16> {
    (0..=65535u32).map(|c| ((f64::from(c) / 65535.0).powf(GAMMA) * 65535.0).round().clamp(0.0, 65535.0) as u16).collect()
}

/// One tile (`edge × edge` RGB samples, 16-bit) as a JPEG XL codestream.
#[cfg(not(target_arch = "wasm32"))]
fn encode_tile(buf: &[u16], edge: usize, distance: Option<f32>, slow: bool) -> Result<Vec<u8>> {
    use jixel::{ColorEncoding, EncodeConfig, Speed};
    let config = EncodeConfig::default().with_color_encoding(ColorEncoding::srgb()).with_speed(if slow { Speed::Slow } else { Speed::Fast });
    let config = match distance {
        None => config.with_lossless(true),
        Some(d) => config.with_distance(d),
    };
    // the encoder is a dependency: an escaped panic becomes an error, as in craft-jxl
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| jixel::encode_image_16bit(buf, edge, edge, &config)))
        .map_err(|_| RawError::Corrupt("the JPEG XL encoder failed on a tile".into()))?
        .map_err(|e| RawError::Corrupt(format!("JPEG XL tile: {e}")))
}

#[cfg(target_arch = "wasm32")]
fn encode_tile(_: &[u16], _: usize, _: Option<f32>, _: bool) -> Result<Vec<u8>> {
    Err(RawError::Unsupported("JPEG XL tiles are not available on this target".into()))
}

/// The tiles of `w × h` interleaved RGB `samples`, row-major, the last row and column of tiles padded by
/// replicating the edge (as the other tile writers do). `distance` `Some`: the samples are coded on the gamma curve.
pub(crate) fn encode_tiles(samples: &[u16], w: usize, h: usize, tile: usize, distance: Option<f32>, slow: bool) -> Result<Vec<Vec<u8>>> {
    let (ta, td) = (w.div_ceil(tile), h.div_ceil(tile));
    let count = ta.checked_mul(td).ok_or(RawError::Limit("too many tiles"))?;
    let expected = w.checked_mul(h).and_then(|n| n.checked_mul(3)).ok_or(RawError::Limit("image too large"))?;
    if w == 0 || h == 0 || samples.len() < expected {
        return Err(RawError::Corrupt("JPEG XL tiles: the samples do not fill the image".into()));
    }
    let codes: Option<Vec<u16>> = distance.map(|_| (0..=65535u32).map(|v| code_of(v as u16)).collect());
    (0..count)
        .into_par_iter()
        .map(|i| {
            let (tx, ty) = (i % ta * tile, i / ta * tile);
            let mut buf = Vec::with_capacity(tile * tile * 3);
            for y in 0..tile {
                let sy = (ty + y).min(h - 1);
                for x in 0..tile {
                    let at = (sy * w + (tx + x).min(w - 1)) * 3;
                    for s in 0..3 {
                        let v = samples.get(at + s).copied().unwrap_or(0);
                        buf.push(codes.as_ref().and_then(|c| c.get(usize::from(v)).copied()).unwrap_or(v));
                    }
                }
            }
            encode_tile(&buf, tile, distance, slow)
        })
        .collect()
}
