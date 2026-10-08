//! Canon CRX lossy mode ("C-RAW"): three levels of an integer 5/3 wavelet per Bayer plane, entropy coded.
//!
//! Sources: as for [`super::crx`] (Laurent Clévy's CR3 notes for the box and record layout, ITU-T T.87 for the run-mode
//! vocabulary, JPEG 2000 part 1 for the reversible 5/3 lifting steps), and black-box analysis of CC0 sample files from
//! the EOS R, 90D and M50: the rules below were recovered by testing hypotheses against their bitstreams, and the
//! output of a reference decoder (LibRaw's `unprocessed_raw`) served as a local test oracle (the reconstructions of
//! all planes of these files equal it sample for sample). No source code of any other decoder was read.
//!
//! A plane (the samples of one Bayer position of a tile) is cut into ten bands, in this order: `LL3`, `HL3`, `LH3`,
//! `HH3`, `HL2`, `LH2`, `HH2`, `HL1`, `LH1`, `HH1` (`H`/`L`: high / low pass across a row, then down a column; the
//! digit is the level, 1 = finest). Each band is an independent bit stream, stored back to back after the tile's
//! header records; the record of a band says how many bytes it has and carries a quantiser value `x` (the band
//! index in the top four bits, `x` in the low twelve) from which the step is `floor(2^((x - 32) / 48))`: 1, 1, 1, 1,
//! 4, 4, 8, 12, 12 and 25 in the files seen. A sample is the coefficient divided by the step (towards zero); the
//! decoder multiplies it back (no offset).
//!
//! # Geometry
//!
//! A tile that has a neighbour on its right (`First`) or on its left (`Last`) carries the coefficients its edge needs
//! from the neighbour instead of mirroring the signal there, so its bands are wider than the plane's halves:
//! - alone (`Only`): a band has `⌈n/2⌉` low and `⌊n/2⌋` high columns, `n` being the low-pass width of the level above
//!   (`n = plane width` for level 1), and the usual mirrored ends;
//! - with a right neighbour (`First`): `⌊n/2⌋ + 1` high columns and as many low ones, except that a low band of
//!   level 1 or 2 is made odd (one more column when that number is even); `n` is the previous *coded* low width;
//! - with a left neighbour (`Last`): `⌈n/2⌉` low and `⌊n/2⌋ + 1` high columns, the first high one being the
//!   coefficient to the left of the tile's first sample. Rows are never extended: `⌈h/2⌉` low and `⌊h/2⌋` high.
//!
//! # Entropy coding
//!
//! `LL3` is coded exactly like a lossless plane ([`super::crx`]), over its coded width. The other bands code their
//! coefficients with no prediction, row by row, with the Golomb-Rice code `v` of the zigzag-mapped coefficient, a
//! parameter `k` (a single state per band, starting at 0 and carried across rows) and the zero-run mode below:
//! - after a sample with code value `v`, `k` follows [`next_k`] with `w = v`; then, in rows after the first, `k` grows
//!   by one more when the context of the sample two columns to the right in the row above (see below) is at least
//!   `k + 2`. The context two columns right of the last column is the final context of that row;
//! - context: each position of a row has one, kept for the next row. A coded sample's context is the `k` it was coded
//!   with. The sample where run mode starts has the `k` in effect, the rest of the run has 0, and so does the sample
//!   that ends a run; a sample that follows an empty run keeps the `k`. A row's final context is its last `k`, or 0
//!   when a run ended the row;
//! - run mode starts, as in the lossless coder, where the decoded left, above and above-right values are all zero
//!   (not in the last column; with no row above, the zero row), with the same empty / non-empty bit and run length
//!   code. The sample that ends a run is non-zero and is coded as `zigzag(v + 1)`; `k` is updated from its `v`. The
//!   run state `ri` grows after a block that ends exactly at the row end and is unchanged when the final block
//!   overshoots.
//!
//! # Reconstruction
//!
//! Per level, the rows of the low and high bands are first inverted horizontally (`x[2k] = s[k] - ((d[k-1] + d[k] +
//! 2) >> 2)`, `x[2k+1] = d[k] + ((x[2k] + x[2k+2]) >> 1)`), the results of the low and the high rows are then inverted
//! vertically in the same way, and the output is `LL` for the next level up. Mirrored ends repeat the nearest
//! coefficient; the explicit extra coefficients of a tile replace the mirroring at its seams. The finest level gives
//! the plane, clamped to the sample range.

use super::crx::{self, Bits, MAX_K, MAX_TRAILING, next_k, read_run, read_value, zigzag};
use crate::{MAX_SAMPLES, RawError, Result};
use rayon::prelude::*;

/// Bands per plane (three levels of three, and the final approximation).
pub(crate) const BANDS: usize = 10;
const LEVELS: usize = 3;
/// Largest quantiser step we accept (real ones are a few tens).
const MAX_STEP: i32 = 1 << 16;

/// Where a tile sits among the tiles of its row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TileEdge {
    /// The only tile.
    Only,
    /// The left tile of two: it has a neighbour on its right.
    First,
    /// The right tile of two: it has a neighbour on its left.
    Last,
}

/// Columns and rows of a band as coded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Dim {
    pub cols: usize,
    pub rows: usize,
}

/// The coded shape of the ten bands of a plane and the widths of the intermediate approximations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Layout {
    pub bands: [Dim; BANDS],
    /// Extra high-pass columns on the left of each high band (1 for the `Last` tile).
    pub left: usize,
    /// Widths of the approximations after the first and second inverse level.
    pub ll_widths: [usize; 2],
}

/// Band shapes of a plane of `pw × ph` samples in a tile of the given kind.
pub(crate) fn layout(pw: usize, ph: usize, edge: TileEdge) -> Layout {
    let (mut low_rows, mut high_rows) = ([0usize; LEVELS + 1], [0usize; LEVELS + 1]);
    let mut h = ph;
    for l in 1..=LEVELS {
        low_rows[l] = h.div_ceil(2);
        high_rows[l] = h / 2;
        h = low_rows[l];
    }
    let (mut low, mut high) = ([0usize; LEVELS + 1], [0usize; LEVELS + 1]);
    let mut n = pw;
    for l in 1..=LEVELS {
        match edge {
            TileEdge::Only => {
                low[l] = n.div_ceil(2);
                high[l] = n / 2;
            }
            TileEdge::First => {
                high[l] = n / 2 + 1;
                low[l] = if l == LEVELS || high[l] % 2 == 1 { high[l] } else { high[l] + 1 };
            }
            TileEdge::Last => {
                low[l] = n.div_ceil(2);
                high[l] = n / 2 + 1;
            }
        }
        n = low[l];
    }
    let dim = |cols: usize, rows: usize| Dim { cols, rows };
    Layout {
        bands: [
            dim(low[3], low_rows[3]),
            dim(high[3], low_rows[3]),
            dim(low[3], high_rows[3]),
            dim(high[3], high_rows[3]),
            dim(high[2], low_rows[2]),
            dim(low[2], high_rows[2]),
            dim(high[2], high_rows[2]),
            dim(high[1], low_rows[1]),
            dim(low[1], high_rows[1]),
            dim(high[1], high_rows[1]),
        ],
        left: usize::from(edge == TileEdge::Last),
        ll_widths: [low[1], low[2]],
    }
}

/// Quantiser step from the 12-bit value of a band record.
pub(crate) fn band_step(x: u16) -> i32 {
    let e = (f64::from(x & 0x0fff) - 32.0) / 48.0;
    (e.exp2().floor() as i32).clamp(1, MAX_STEP)
}

/// The context of the sample `i + 2` in the row above (see the module docs), or `None` where there is none.
#[inline]
fn bump_context(above: &[u8], i: usize, w: usize) -> Option<u32> {
    let j = i + 2;
    if j > w { None } else { above.get(j).map(|&c| c as u32) }
}

/// Decode a band of `w × h` coefficients that is not the approximation.
pub(crate) fn decode_detail(data: &[u8], w: usize, h: usize) -> Result<Vec<i32>> {
    let total = w.checked_mul(h).filter(|t| *t <= MAX_SAMPLES).ok_or(RawError::Limit("CRX band too large"))?;
    let mut out = vec![0i32; total];
    if total == 0 {
        return Ok(out);
    }
    let mut br = Bits::new(data);
    let (mut k, mut ri) = (0u32, 0usize);
    let mut above: Vec<u8> = Vec::new();
    let mut ctx = vec![0u8; w + 1];
    for y in 0..h {
        let (done, rest) = out.split_at_mut(y * w);
        let up: &[i32] = if y > 0 { done.get((y - 1) * w..).unwrap_or(&[]) } else { &[] };
        let cur = rest.get_mut(..w).ok_or_else(|| RawError::Corrupt("CRX row".into()))?;
        let mut ended_by_run = false;
        // the next `k`, with the extra step the row above asks for
        let advance = |k: u32, v: u32, i: usize, above: &[u8]| -> u32 {
            let mut kp = next_k(k, v);
            if y > 0 && bump_context(above, i, w).is_some_and(|c| c >= kp + 2) {
                kp = (kp + 1).min(MAX_K);
            }
            kp
        };
        let mut i = 0usize;
        while i < w {
            let b = up.get(i).copied().unwrap_or(0);
            let d = up.get((i + 1).min(w - 1)).copied().unwrap_or(0);
            let a = if i > 0 { cur.get(i - 1).copied().unwrap_or(0) } else { b };
            if i + 1 < w && a == 0 && b == 0 && d == 0 {
                // run mode: the sample where the flag is read keeps the current k as its context
                let (run, eol) = if br.bit()? == 1 { read_run(&mut br, &mut ri, w - i)? } else { (0, false) };
                if let Some(c) = ctx.get_mut(i) {
                    *c = k as u8;
                }
                if let Some(s) = ctx.get_mut(i + 1..i + run.max(1)) {
                    s.fill(0);
                }
                i += run;
                if eol || i >= w {
                    ended_by_run = true;
                    break;
                }
                // the sample that ended the run (it is never zero)
                if run > 0 {
                    if let Some(c) = ctx.get_mut(i) {
                        *c = 0;
                    }
                } else if let Some(c) = ctx.get_mut(i) {
                    *c = k as u8;
                }
                let v = read_value(&mut br, k)?;
                if let Some(s) = cur.get_mut(i) {
                    *s = zigzag(v.saturating_add(1));
                }
                k = advance(k, v, i, &above);
                i += 1;
            } else {
                if let Some(c) = ctx.get_mut(i) {
                    *c = k as u8;
                }
                let v = read_value(&mut br, k)?;
                if let Some(s) = cur.get_mut(i) {
                    *s = zigzag(v);
                }
                k = advance(k, v, i, &above);
                i += 1;
            }
        }
        if let Some(c) = ctx.get_mut(w) {
            *c = if ended_by_run { 0 } else { k as u8 };
        }
        std::mem::swap(&mut above, &mut ctx);
        ctx.resize(w + 1, 0);
    }
    if br.bytes_consumed() + MAX_TRAILING < data.len() {
        return Err(RawError::Corrupt(format!("CRX band ends {} bytes before its data does", data.len() - br.bytes_consumed())));
    }
    Ok(out)
}

/// A band (or intermediate image) as rows of `w` values.
#[derive(Clone, Debug, Default)]
struct Grid {
    w: usize,
    h: usize,
    v: Vec<i32>,
}

impl Grid {
    fn new(w: usize, h: usize, v: Vec<i32>) -> Result<Grid> {
        if w.checked_mul(h) != Some(v.len()) {
            return Err(RawError::Corrupt("CRX band size".into()));
        }
        Ok(Grid { w, h, v })
    }
    fn row(&self, y: usize) -> &[i32] {
        self.w.checked_mul(y).and_then(|s| self.v.get(s..s.checked_add(self.w)?)).unwrap_or(&[])
    }
}

/// Inverse 5/3 of one row. `low` has `⌈n/2⌉` samples, `high` the same or one fewer, plus `left` extra coefficients in
/// front of it (the coefficient left of the first output). `out` receives the first `out.len()` samples.
fn inverse_row(low: &[i32], high: &[i32], left: usize, tmp: &mut Vec<i32>, out: &mut [i32]) {
    let nl = low.len();
    tmp.clear();
    tmp.resize(2 * nl + 1, 0);
    let d = |k: isize| -> i32 {
        if high.is_empty() {
            return 0;
        }
        let j = (k + left as isize).clamp(0, high.len() as isize - 1) as usize;
        high.get(j).copied().unwrap_or(0)
    };
    for (k, &s) in low.iter().enumerate() {
        let corr = d(k as isize - 1).wrapping_add(d(k as isize)).wrapping_add(2) >> 2;
        if let Some(x) = tmp.get_mut(2 * k) {
            *x = s.wrapping_sub(corr);
        }
    }
    for k in 0..nl {
        let l = tmp.get(2 * k).copied().unwrap_or(0);
        let r = if k + 1 < nl { tmp.get(2 * k + 2).copied().unwrap_or(l) } else { l };
        if let Some(x) = tmp.get_mut(2 * k + 1) {
            *x = d(k as isize).wrapping_add(l.wrapping_add(r) >> 1);
        }
    }
    let n = out.len().min(tmp.len());
    if let (Some(dst), Some(src)) = (out.get_mut(..n), tmp.get(..n)) {
        dst.copy_from_slice(src);
    }
}

/// Invert the rows of a low and a high band horizontally into rows of `out_w` samples.
fn inverse_rows(low: &Grid, high: &Grid, left: usize, out_w: usize) -> Result<Grid> {
    if low.h != high.h {
        return Err(RawError::Corrupt("CRX band rows differ".into()));
    }
    let total = out_w.checked_mul(low.h).filter(|t| *t <= MAX_SAMPLES).ok_or(RawError::Limit("CRX band too large"))?;
    let mut out = vec![0i32; total];
    let mut tmp = Vec::new();
    if out_w > 0 {
        for (y, row) in out.chunks_exact_mut(out_w).enumerate() {
            inverse_row(low.row(y), high.row(y), left, &mut tmp, row);
        }
    }
    Grid::new(out_w, low.h, out)
}

/// Row `k` of `g`, repeating the first / last one outside it (`zero` stands in for a grid without rows).
fn clamped_row<'a>(g: &'a Grid, zero: &'a [i32], k: isize) -> &'a [i32] {
    if g.h == 0 {
        return zero;
    }
    g.row(k.clamp(0, g.h as isize - 1) as usize)
}

/// Row `k` of `w`-wide data.
fn row_of(v: &[i32], w: usize, k: usize) -> &[i32] {
    k.checked_mul(w).and_then(|s| v.get(s..s.checked_add(w)?)).unwrap_or(&[])
}

/// Invert vertically: `l` holds the even rows (`⌈n/2⌉` of them), `h` the odd ones.
fn inverse_columns(l: &Grid, h: &Grid) -> Result<Grid> {
    if l.w != h.w || !(l.h == h.h || l.h == h.h + 1) {
        return Err(RawError::Corrupt("CRX band shapes do not fit".into()));
    }
    let (w, nl, nh) = (l.w, l.h, h.h);
    let total = w.checked_mul(nl + nh).filter(|t| *t <= MAX_SAMPLES).ok_or(RawError::Limit("CRX band too large"))?;
    let zero = vec![0i32; w];
    // the even rows
    let mut even = Vec::with_capacity(w * nl);
    for k in 0..nl {
        let (a, b) = (clamped_row(h, &zero, k as isize - 1), clamped_row(h, &zero, k as isize));
        even.extend(l.row(k).iter().zip(a.iter().zip(b)).map(|(&s, (&p, &q))| s.wrapping_sub(p.wrapping_add(q).wrapping_add(2) >> 2)));
    }
    let mut out = Vec::with_capacity(total);
    for k in 0..nl {
        let e0 = row_of(&even, w, k);
        out.extend_from_slice(e0);
        if k < nh {
            let e1 = if k + 1 < nl { row_of(&even, w, k + 1) } else { e0 };
            out.extend(h.row(k).iter().zip(e0.iter().zip(e1)).map(|(&d, (&x0, &x1))| d.wrapping_add(x0.wrapping_add(x1) >> 1)));
        }
    }
    Grid::new(w, nl + nh, out)
}

fn inverse_level(ll: &Grid, hl: &Grid, lh: &Grid, hh: &Grid, left: usize, out_w: usize) -> Result<Grid> {
    let lo = inverse_rows(ll, hl, left, out_w)?;
    let hi = inverse_rows(lh, hh, left, out_w)?;
    inverse_columns(&lo, &hi)
}

/// The ten band streams of a plane and their quantiser steps.
pub(crate) struct PlaneBands<'a> {
    pub streams: [&'a [u8]; BANDS],
    pub steps: [i32; BANDS],
}

/// Decode a plane of `pw × ph` samples of `bits` significant bits from its ten bands.
pub(crate) fn decode_plane(bands: &PlaneBands, pw: usize, ph: usize, edge: TileEdge, bits: u32) -> Result<Vec<u16>> {
    if pw == 0 || ph == 0 || !(8..=16).contains(&bits) || pw.checked_mul(ph).is_none_or(|n| n > MAX_SAMPLES) {
        return Err(RawError::Corrupt("bad CRX plane geometry".into()));
    }
    let lay = layout(pw, ph, edge);
    let decoded: Vec<Vec<i32>> = (0..BANDS)
        .into_par_iter()
        .map(|b| -> Result<Vec<i32>> {
            let (dim, data) = (lay.bands.get(b).copied().unwrap_or(Dim { cols: 0, rows: 0 }), bands.streams.get(b).copied().unwrap_or(&[]));
            let step = bands.steps.get(b).copied().unwrap_or(1);
            let mut v = if dim.cols == 0 || dim.rows == 0 {
                Vec::new()
            } else if b == 0 {
                crx::decode_approximation(data, dim.cols, dim.rows, bits)?
            } else {
                decode_detail(data, dim.cols, dim.rows)?
            };
            if step != 1 {
                v.iter_mut().for_each(|x| *x = x.wrapping_mul(step));
            }
            Ok(v)
        })
        .collect::<Result<_>>()?;
    let mut grids = Vec::with_capacity(BANDS);
    for (v, dim) in decoded.into_iter().zip(&lay.bands) {
        grids.push(Grid::new(dim.cols, dim.rows, v)?);
    }
    let g = |b: usize| grids.get(b).ok_or_else(|| RawError::Corrupt("CRX bands".into()));
    let ll2 = inverse_level(g(0)?, g(1)?, g(2)?, g(3)?, lay.left, lay.ll_widths[1])?;
    let ll1 = inverse_level(&ll2, g(4)?, g(5)?, g(6)?, lay.left, lay.ll_widths[0])?;
    let plane = inverse_level(&ll1, g(7)?, g(8)?, g(9)?, lay.left, pw)?;
    if plane.w != pw || plane.h != ph {
        return Err(RawError::Corrupt("CRX plane shape".into()));
    }
    let maxv = (1i32 << bits) - 1;
    Ok(plane.v.iter().map(|&x| x.clamp(0, maxv) as u16).collect())
}

/// A test-only wavelet encoder (the rules of the module docs, written independently of the decoder's control flow),
/// shared with the container tests of [`super::cr3`].
#[cfg(test)]
pub(crate) mod testenc {
    use super::super::crx::testenc::{BitWriter, encode_plane, unzigzag};
    use super::*;
    use crate::vendor::crx::{J, MAX_RI};

    /// Samples a tile window extends past its seam (a multiple of eight keeps the phase of every level).
    const MARGIN: usize = 32;

    fn put_value(bw: &mut BitWriter, v: u32, k: u32) {
        let q = v >> k;
        if q >= 41 {
            for _ in 0..41 {
                bw.put(0, 1);
            }
            bw.put(1, 1);
            bw.put(v, 21);
        } else {
            for _ in 0..q {
                bw.put(0, 1);
            }
            bw.put(1, 1);
            bw.put(v & ((1u32 << k) - 1), k);
        }
    }

    /// Encode a `w × h` band of coefficients (not the approximation).
    pub(crate) fn encode_detail(vals: &[i32], w: usize, h: usize) -> Vec<u8> {
        let mut bw = BitWriter::default();
        let (mut k, mut ri) = (0u32, 0usize);
        let mut above: Vec<u8> = Vec::new();
        let mut ctx = vec![0u8; w + 1];
        let at = |x: usize, y: usize| vals[y * w + x];
        for y in 0..h {
            let advance = |k: u32, v: u32, i: usize, above: &[u8]| -> u32 {
                let mut kp = next_k(k, v);
                if y > 0 && i + 2 <= w && above[i + 2] as u32 >= kp + 2 {
                    kp += 1;
                }
                kp
            };
            let mut ended = false;
            let mut i = 0usize;
            while i < w {
                let b = if y > 0 { at(i, y - 1) } else { 0 };
                let d = if y > 0 { at((i + 1).min(w - 1), y - 1) } else { 0 };
                let a = if i > 0 { at(i - 1, y) } else { b };
                if i + 1 < w && a == 0 && b == 0 && d == 0 {
                    ctx[i] = k as u8;
                    let mut run = 0usize;
                    while i + run < w && at(i + run, y) == 0 {
                        run += 1;
                    }
                    if run == 0 {
                        bw.put(0, 1);
                    } else {
                        bw.put(1, 1);
                        let room = w - i;
                        if run == room {
                            // to the end of the row: ones until the blocks cover it
                            let mut covered = 1usize;
                            loop {
                                let step = 1usize << J[ri];
                                bw.put(1, 1);
                                if covered + step >= room {
                                    if covered + step == room {
                                        ri = (ri + 1).min(MAX_RI);
                                    }
                                    break;
                                }
                                covered += step;
                                ri = (ri + 1).min(MAX_RI);
                            }
                        } else {
                            let mut n = run - 1;
                            while n >= (1usize << J[ri]) {
                                bw.put(1, 1);
                                n -= 1usize << J[ri];
                                ri = (ri + 1).min(MAX_RI);
                            }
                            bw.put(0, 1);
                            bw.put(n as u32, J[ri] as u32);
                            ri = ri.saturating_sub(1);
                        }
                    }
                    for t in 1..run {
                        ctx[i + t] = 0;
                    }
                    i += run;
                    if i >= w {
                        ended = true;
                        break;
                    }
                    ctx[i] = if run > 0 { 0 } else { k as u8 };
                    let v = unzigzag(at(i, y)) - 1;
                    put_value(&mut bw, v, k);
                    k = advance(k, v, i, &above);
                    i += 1;
                } else {
                    ctx[i] = k as u8;
                    let v = unzigzag(at(i, y));
                    put_value(&mut bw, v, k);
                    k = advance(k, v, i, &above);
                    i += 1;
                }
            }
            ctx[w] = if ended { 0 } else { k as u8 };
            std::mem::swap(&mut above, &mut ctx);
            ctx.resize(w + 1, 0);
        }
        bw.finish()
    }

    /// Mirror an index into `0..n` (whole-sample symmetric).
    fn refl(i: isize, n: usize) -> usize {
        if n <= 1 {
            return 0;
        }
        let period = 2 * (n as isize - 1);
        let m = i.rem_euclid(period);
        (if m >= n as isize { period - m } else { m }) as usize
    }

    /// Forward 5/3 of a row or column: (low, high).
    fn fwd_1d(x: &[i32]) -> (Vec<i32>, Vec<i32>) {
        let n = x.len();
        let nh = n / 2;
        let nl = n - nh;
        let d: Vec<i32> = (0..nh).map(|k| x[2 * k + 1] - ((x[refl(2 * k as isize, n)] + x[refl(2 * k as isize + 2, n)]) >> 1)).collect();
        let s: Vec<i32> = (0..nl)
            .map(|k| {
                let dl = if k == 0 { d.first().copied().unwrap_or(0) } else { d[k - 1] };
                let dr = if k < nh { d[k] } else { d.get(nh.wrapping_sub(1)).copied().unwrap_or(0) };
                x[2 * k] + ((dl + dr + 2) >> 2)
            })
            .collect();
        (s, d)
    }

    /// One 2-D level, vertical first: `[LL, HL, LH, HH]`.
    fn fwd_2d(g: &Grid) -> [Grid; 4] {
        let (w, h) = (g.w, g.h);
        let (nh, nl) = (h / 2, h - h / 2);
        let (mut lo, mut hi) = (vec![0i32; w * nl], vec![0i32; w * nh]);
        for x in 0..w {
            let col: Vec<i32> = (0..h).map(|y| g.v[y * w + x]).collect();
            let (s, d) = fwd_1d(&col);
            for (y, v) in s.into_iter().enumerate() {
                lo[y * w + x] = v;
            }
            for (y, v) in d.into_iter().enumerate() {
                hi[y * w + x] = v;
            }
        }
        let rows = |src: &[i32], hh: usize| -> (Grid, Grid) {
            let (wl, wh) = (w - w / 2, w / 2);
            let (mut l, mut hg) = (vec![0i32; wl * hh], vec![0i32; wh * hh]);
            for y in 0..hh {
                let (s, d) = fwd_1d(&src[y * w..(y + 1) * w]);
                l[y * wl..(y + 1) * wl].copy_from_slice(&s);
                hg[y * wh..(y + 1) * wh].copy_from_slice(&d);
            }
            (Grid { w: wl, h: hh, v: l }, Grid { w: wh, h: hh, v: hg })
        };
        let (ll, hl) = rows(&lo, nl);
        let (lh, hh) = rows(&hi, nh);
        [ll, hl, lh, hh]
    }

    /// The ten band streams of the plane of a tile that covers columns `x0..x0 + pw` of the `gw × gh` image `img`
    /// (the neighbouring columns supply the tile's seam coefficients).
    pub(crate) fn encode_tile_plane(
        img: &[u16],
        gw: usize,
        gh: usize,
        x0: usize,
        pw: usize,
        edge: TileEdge,
        bits: u32,
        steps: &[i32; BANDS],
    ) -> [Vec<u8>; BANDS] {
        let lay = layout(pw, gh, edge);
        let q = quantised_bands(img, gw, gh, x0, pw, edge, steps);
        std::array::from_fn(|b| {
            let dim = lay.bands[b];
            if b == 0 { encode_plane(&q[b], dim.cols, dim.rows, bits) } else { encode_detail(&q[b], dim.cols, dim.rows) }
        })
    }

    /// The quantised coefficients of the ten bands of a tile plane (see [`encode_tile_plane`]).
    pub(crate) fn quantised_bands(
        img: &[u16],
        gw: usize,
        gh: usize,
        x0: usize,
        pw: usize,
        edge: TileEdge,
        steps: &[i32; BANDS],
    ) -> [Vec<i32>; BANDS] {
        let lay = layout(pw, gh, edge);
        let margin = MARGIN.min(x0) & !7;
        let (wx0, wx1) = match edge {
            TileEdge::Only => (x0, x0 + pw),
            TileEdge::First => (x0, (x0 + pw + MARGIN).min(gw)),
            TileEdge::Last => (x0 - margin, x0 + pw),
        };
        let ww = wx1 - wx0;
        let win = Grid { w: ww, h: gh, v: (0..gh).flat_map(|y| (wx0..wx1).map(move |x| img[y * gw + x] as i32)).collect() };
        let l1 = fwd_2d(&win);
        let l2 = fwd_2d(&l1[0]);
        let l3 = fwd_2d(&l2[0]);
        let levels = [&l1, &l2, &l3];
        // band -> (level, index in the level, high-pass across)
        let src: [(usize, usize, bool); BANDS] = [
            (3, 0, false),
            (3, 1, true),
            (3, 2, false),
            (3, 3, true),
            (2, 1, true),
            (2, 2, false),
            (2, 3, true),
            (1, 1, true),
            (1, 2, false),
            (1, 3, true),
        ];
        std::array::from_fn(|b| {
            let (level, idx, high) = src[b];
            let g = &levels[level - 1][idx];
            let dim = lay.bands[b];
            let off = if edge == TileEdge::Last { if high { (margin >> level) - 1 } else { margin >> level } } else { 0 };
            assert_eq!(g.h, dim.rows, "band {b} rows");
            assert!(off + dim.cols <= g.w, "band {b}: window too narrow ({} + {} > {})", off, dim.cols, g.w);
            (0..dim.rows).flat_map(|y| g.v[y * g.w + off..y * g.w + off + dim.cols].iter().map(|&c| c / steps[b]).collect::<Vec<_>>()).collect()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::testenc::{encode_detail, encode_tile_plane};
    use super::*;

    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u32 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (self.0 >> 33) as u32
        }
    }

    /// Coefficients: mostly zero, short and long zero runs, small values, the odd big one.
    fn coefficients(w: usize, h: usize, seed: u64, zero_percent: u32, big_percent: u32) -> Vec<i32> {
        let mut r = Lcg(seed);
        let mut v = vec![0i32; w * h];
        for y in 0..h {
            for x in 0..w {
                let quiet = (x / 13 + y / 5) % 4 == 0;
                let roll = r.next() % 100;
                v[y * w + x] = if quiet || roll < zero_percent {
                    0
                } else if roll < zero_percent + big_percent {
                    (r.next() % 90_000) as i32 - 45_000
                } else {
                    let m = 1 + (r.next() % 6) as i32 + if (x + 3 * y) % 17 == 0 { 40 } else { 0 };
                    if r.next().is_multiple_of(2) { m } else { -m }
                };
            }
        }
        v
    }

    #[test]
    fn detail_bands_round_trip() {
        for (w, h, zeros, big) in [
            (1, 1, 50, 10),
            (1, 9, 20, 0),
            (2, 2, 30, 20),
            (3, 7, 60, 5),
            (17, 9, 70, 2),
            (64, 33, 90, 1),
            (215, 40, 85, 0),
            (300, 12, 97, 1),
            (257, 21, 5, 0),
        ] {
            let v = coefficients(w, h, (w * 131 + h) as u64, zeros, big);
            let enc = encode_detail(&v, w, h);
            let dec = decode_detail(&enc, w, h).unwrap_or_else(|e| panic!("{w}x{h}: {e}"));
            assert_eq!(dec, v, "{w}x{h}");
        }
    }

    #[test]
    fn detail_bands_with_empty_rows_and_long_runs_round_trip() {
        // all-zero rows grow the run state through its table; a run reaching the row end exactly, and one short of it
        let (w, h) = (5000usize, 7usize);
        let mut v = vec![0i32; w * h];
        assert_eq!(decode_detail(&encode_detail(&v, w, h), w, h).unwrap(), v);
        v[w + 17] = 3;
        v[3 * w + w - 1] = -2;
        v[4 * w + w - 2] = 9;
        v[6 * w] = 1;
        assert_eq!(decode_detail(&encode_detail(&v, w, h), w, h).unwrap(), v);
        for tail in 1..30usize {
            let w = 30usize;
            let mut v = vec![0i32; w * 4];
            for x in (w - tail)..w {
                v[2 * w + x] = 2;
            }
            assert_eq!(decode_detail(&encode_detail(&v, w, 4), w, 4).unwrap(), v, "tail {tail}");
        }
    }

    #[test]
    fn empty_bands_decode_to_nothing() {
        assert!(decode_detail(&[], 0, 5).unwrap().is_empty());
        assert!(decode_detail(&[], 5, 0).unwrap().is_empty());
    }

    #[test]
    fn truncated_and_hostile_streams_are_errors_not_panics() {
        let v = coefficients(120, 30, 7, 60, 2);
        let enc = encode_detail(&v, 120, 30);
        for cut in [0, 1, 2, enc.len() / 2, enc.len() - 8] {
            assert!(decode_detail(&enc[..cut], 120, 30).is_err(), "cut at {cut}");
        }
        let mut r = Lcg(5);
        for round in 0..300 {
            let len = (r.next() % 300) as usize;
            let data: Vec<u8> = (0..len).map(|_| if round % 3 == 0 { 0xff } else { r.next() as u8 }).collect();
            let (w, h) = (1 + (r.next() % 60) as usize, 1 + (r.next() % 12) as usize);
            let _ = decode_detail(&data, w, h);
        }
        assert!(decode_detail(&[0u8; 64], 4, 4).is_err(), "an endless unary prefix is an error");
        assert!(decode_detail(&[], usize::MAX, 2).is_err());
    }

    #[test]
    fn long_tails_are_errors() {
        let v = coefficients(40, 8, 3, 60, 0);
        let mut enc = encode_detail(&v, 40, 8);
        enc.extend([0u8; MAX_TRAILING]);
        assert_eq!(decode_detail(&enc, 40, 8).unwrap(), v);
        enc.push(0);
        assert!(decode_detail(&enc, 40, 8).is_err());
    }

    #[test]
    fn quantiser_steps_match_the_files_seen() {
        let steps: Vec<i32> = [0x20u16, 0x20, 0x20, 0x20, 0x80, 0x80, 0xb0, 0xd0, 0xd0, 0x100].iter().map(|&x| band_step(x)).collect();
        assert_eq!(steps, [1, 1, 1, 1, 4, 4, 8, 12, 12, 25]);
        // the band index in the top bits does not matter, and extreme values stay finite
        assert_eq!(band_step(0x9100), 25);
        assert_eq!(band_step(0), 1);
        assert!((1..=MAX_STEP).contains(&band_step(0x0fff)));
    }

    #[test]
    fn layouts_match_the_files_seen() {
        let widths = |l: &Layout| l.bands.map(|d| d.cols);
        let rows = |l: &Layout| l.bands.map(|d| d.rows);
        // EOS R (tiles of 1722 × 2273 per plane)
        let first = layout(1722, 2273, TileEdge::First);
        assert_eq!(widths(&first), [217, 217, 217, 217, 432, 433, 432, 862, 863, 862]);
        assert_eq!(rows(&first), [285, 285, 284, 284, 569, 568, 568, 1137, 1136, 1136]);
        assert_eq!((first.left, first.ll_widths), (0, [863, 433]));
        let last = layout(1722, 2273, TileEdge::Last);
        assert_eq!(widths(&last), [216, 216, 216, 216, 431, 431, 431, 862, 861, 862]);
        assert_eq!((last.left, last.ll_widths), (1, [861, 431]));
        // 90D and M50
        assert_eq!(widths(&layout(1782, 2366, TileEdge::First)), [224, 224, 224, 224, 447, 447, 447, 892, 893, 892]);
        assert_eq!(widths(&layout(1782, 2366, TileEdge::Last)), [223, 224, 223, 224, 446, 446, 446, 892, 891, 892]);
        assert_eq!(widths(&layout(1572, 2028, TileEdge::First)), [198, 198, 198, 198, 394, 395, 394, 787, 787, 787]);
        assert_eq!(widths(&layout(1572, 2028, TileEdge::Last)), [197, 197, 197, 197, 394, 393, 394, 787, 786, 787]);
        // a single tile has the plane's own halves
        let only = layout(812, 540, TileEdge::Only);
        assert_eq!(widths(&only), [102, 101, 102, 101, 203, 203, 203, 406, 406, 406]);
        assert_eq!(rows(&only), [68, 68, 67, 67, 135, 135, 135, 270, 270, 270]);
    }

    /// Smooth shading, texture and noise, away from the sample limits.
    fn image(w: usize, h: usize, seed: u64) -> Vec<u16> {
        let mut r = Lcg(seed);
        (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f64, (i / w) as f64);
                let base = 6000.0 + 3000.0 * (x / 17.0).sin() + 2500.0 * (y / 23.0).cos() + 1200.0 * ((x + y) / 5.0).sin();
                let flat = (i % w / 40 + i / w / 9).is_multiple_of(5);
                (if flat { 5000.0 } else { base + (r.next() % 60) as f64 }) as u16
            })
            .collect()
    }

    fn tile_of(img: &[u16], gw: usize, h: usize, x0: usize, pw: usize) -> Vec<u16> {
        (0..h).flat_map(|y| img[y * gw + x0..y * gw + x0 + pw].iter().copied()).collect()
    }

    fn decode_streams(streams: &[Vec<u8>; BANDS], steps: [i32; BANDS], pw: usize, ph: usize, edge: TileEdge) -> Result<Vec<u16>> {
        let refs: [&[u8]; BANDS] = std::array::from_fn(|b| streams[b].as_slice());
        decode_plane(&PlaneBands { streams: refs, steps }, pw, ph, edge, 14)
    }

    #[test]
    fn single_tiles_reconstruct_exactly() {
        for (pw, ph) in [(1usize, 1usize), (2, 3), (5, 2), (8, 8), (13, 9), (31, 17), (64, 30), (97, 41), (150, 33)] {
            let img = image(pw, ph, (pw * 7 + ph) as u64);
            let streams = encode_tile_plane(&img, pw, ph, 0, pw, TileEdge::Only, 14, &[1; BANDS]);
            let out = decode_streams(&streams, [1; BANDS], pw, ph, TileEdge::Only).unwrap_or_else(|e| panic!("{pw}x{ph}: {e}"));
            assert_eq!(out, img, "{pw}x{ph}");
        }
    }

    #[test]
    fn neighbouring_tiles_reconstruct_exactly() {
        for (pw, ph) in [(40usize, 1usize), (40, 9), (41, 16), (64, 30), (77, 33), (100, 25), (123, 40)] {
            let gw = 2 * pw;
            let img = image(gw, ph, (pw * 11 + ph) as u64);
            for (edge, x0) in [(TileEdge::First, 0), (TileEdge::Last, pw)] {
                let streams = encode_tile_plane(&img, gw, ph, x0, pw, edge, 14, &[1; BANDS]);
                let out = decode_streams(&streams, [1; BANDS], pw, ph, edge).unwrap_or_else(|e| panic!("{pw}x{ph} {edge:?}: {e}"));
                assert_eq!(out, tile_of(&img, gw, ph, x0, pw), "{pw}x{ph} {edge:?}");
            }
        }
    }

    #[test]
    fn quantised_bands_reconstruct_closely() {
        let steps = [1, 1, 1, 1, 4, 4, 8, 12, 12, 25];
        let (pw, ph) = (96usize, 40usize);
        let img = image(pw, ph, 5);
        let streams = encode_tile_plane(&img, pw, ph, 0, pw, TileEdge::Only, 14, &steps);
        let out = decode_streams(&streams, steps, pw, ph, TileEdge::Only).unwrap();
        let worst = out.iter().zip(&img).map(|(&a, &b)| (a as i32 - b as i32).abs()).max().unwrap();
        assert!(worst > 0 && worst < 80, "worst error {worst}");
    }

    #[test]
    fn output_is_clamped_to_the_sample_range() {
        let (pw, ph) = (32usize, 16usize);
        let mut img = image(pw, ph, 9);
        // a full-scale spike that quantisation overshoots, and a floor
        img[5 * pw + 7] = 16383;
        img[9 * pw + 20] = 0;
        let steps = [1, 1, 1, 1, 4, 4, 8, 12, 12, 25];
        let streams = encode_tile_plane(&img, pw, ph, 0, pw, TileEdge::Only, 14, &steps);
        let out = decode_streams(&streams, steps, pw, ph, TileEdge::Only).unwrap();
        assert!(out.iter().all(|&v| v <= 16383));
    }

    #[test]
    fn bad_geometry_and_damaged_planes_are_errors() {
        let empty: [&[u8]; BANDS] = [&[]; BANDS];
        let bands = PlaneBands { streams: empty, steps: [1; BANDS] };
        assert!(decode_plane(&bands, 0, 4, TileEdge::Only, 14).is_err());
        assert!(decode_plane(&bands, 4, 0, TileEdge::Only, 14).is_err());
        assert!(decode_plane(&bands, 4, 4, TileEdge::Only, 7).is_err());
        assert!(decode_plane(&bands, usize::MAX, 2, TileEdge::Only, 14).is_err());
        assert!(decode_plane(&bands, 64, 32, TileEdge::First, 14).is_err(), "bands without data");
        // damaged streams of a real plane never panic
        let (pw, ph) = (48usize, 20usize);
        let img = image(pw, ph, 3);
        let streams = encode_tile_plane(&img, pw, ph, 0, pw, TileEdge::Only, 14, &[1; BANDS]);
        let mut r = Lcg(77);
        for _ in 0..200 {
            let mut s = streams.clone();
            let b = r.next() as usize % BANDS;
            if s[b].is_empty() {
                continue;
            }
            let i = r.next() as usize % s[b].len();
            s[b][i] = r.next() as u8;
            if r.next().is_multiple_of(4) {
                let cut = r.next() as usize % s[b].len();
                s[b].truncate(cut);
            }
            let _ = decode_streams(&s, [1; BANDS], pw, ph, TileEdge::Only);
        }
    }

    /// A bright patch against a dark frame drives the approximation band below zero (the low-pass filter overshoots);
    /// every band and the whole plane must still round-trip.
    #[test]
    fn sharp_edges_push_the_approximation_below_zero() {
        use super::testenc::quantised_bands;
        let (pw, ph) = (48usize, 32usize);
        let mut r = Lcg(12345);
        let img: Vec<u16> = (0..pw * ph)
            .map(|i| {
                let (x, y) = (i % pw, i / pw);
                if x < 8 || y < 4 {
                    (512 + r.next() % 3) as u16
                } else if (x / 5 + y / 4) % 3 == 0 {
                    4000
                } else {
                    (900 + x * 40 + y * 25 + (r.next() % 30) as usize) as u16
                }
            })
            .collect();
        let steps = [1; BANDS];
        let lay = layout(pw, ph, TileEdge::Only);
        let q = quantised_bands(&img, pw, ph, 0, pw, TileEdge::Only, &steps);
        for b in 1..BANDS {
            let enc = encode_detail(&q[b], lay.bands[b].cols, lay.bands[b].rows);
            let dec = decode_detail(&enc, lay.bands[b].cols, lay.bands[b].rows).unwrap();
            assert_eq!(dec, q[b], "band {b}");
        }
        // LL3 through the lossless-style coder
        let enc = crate::vendor::crx::testenc::encode_plane(&q[0], lay.bands[0].cols, lay.bands[0].rows, 14);
        let dec = crx::decode_approximation(&enc, lay.bands[0].cols, lay.bands[0].rows, 14).unwrap();
        assert_eq!(dec, q[0], "LL3");
        // the transform without any entropy coding
        let g = |b: usize| Grid::new(lay.bands[b].cols, lay.bands[b].rows, q[b].clone()).unwrap();
        let ll2 = inverse_level(&g(0), &g(1), &g(2), &g(3), 0, lay.ll_widths[1]).unwrap();
        let ll1 = inverse_level(&ll2, &g(4), &g(5), &g(6), 0, lay.ll_widths[0]).unwrap();
        let plane = inverse_level(&ll1, &g(7), &g(8), &g(9), 0, pw).unwrap();
        let back: Vec<i32> = plane.v.clone();
        let want: Vec<i32> = img.iter().map(|&v| v as i32).collect();
        assert_eq!(back, want, "transform only");
        let streams = encode_tile_plane(&img, pw, ph, 0, pw, TileEdge::Only, 14, &steps);
        let out = decode_streams(&streams, steps, pw, ph, TileEdge::Only).unwrap();
        assert_eq!(out, img);
    }
}
