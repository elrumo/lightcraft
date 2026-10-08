//! Canon CRX (the codec inside CR3 raw tracks): the lossless ("RAW", no wavelet) mode.
//!
//! Sources: Laurent Clévy's CR3 notes (prose: the `CMP1` header, the tile / plane / subband record layout and that
//! the lossless mode is a JPEG-LS-like Golomb-Rice coder with run mode and median-edge prediction), ITU-T T.87
//! (JPEG-LS: the run-mode vocabulary and its `J[]` table), and black-box analysis of CC0 sample files: the
//! entropy-coder rules below were recovered by testing hypotheses against the bitstreams of EOS R / R5 / R6 / 90D
//! files. The output of a reference decoder (LibRaw's `unprocessed_raw`) was used as a local test oracle; no source
//! code of any other decoder was read.
//!
//! Stream layout (all integers big-endian, bits MSB first): a tile is a `ff01 0008` record (tile data size), then
//! per plane a `ff02 0008` record (plane data size) and one `ff03 0008` record (subband size and flags); the tile's
//! plane streams follow the headers back to back. Four planes hold the Bayer quads (plane `p` is the mosaic
//! position `(p & 1, p >> 1)` of each 2×2 cell).
//!
//! A plane stream (`w × h` samples):
//! - The first sample is coded against `1 << (bits - 1)` with the escape code (below).
//! - Samples are coded as residuals `x - pred`, mapped to `v` by zigzag (`v = 2e` for `e ≥ 0`, `2|e| - 1` for
//!   `e < 0`). Prediction: the left neighbour in the first row; the sample above for the first column; the
//!   median-edge predictor of left / above / above-left elsewhere.
//! - `v` is written as a Golomb-Rice code: `v >> k` zeros, a one, then the low `k` bits. 41 or more zeros and a one
//!   escape to the raw 21-bit `v` (the encoder writes 42 for the first sample of a plane, 41 for later ones).
//! - `k` is a single adaptive state per plane (starts at 2, carries across rows). After each coded symbol with value
//!   `v`, let `g = |above-right − above|` (`(v + 1) >> 1` in the first row and the last column, which have no such
//!   pair) and `w = (v >> 1) + g`; then `q = w >> k`: `q ≥ 6` raises `k` by two, `q ≥ 3` by one, and `w < 2^(k-1)`
//!   lowers it by one. Samples inside a run do not touch `k`.
//! - Run mode starts, in rows after the first, at a sample that is not in the last column and whose left, above and
//!   above-right neighbours are equal (in the first column the left one is taken to be the above one). One bit says
//!   whether the run is empty (`0`, the state below is untouched) or not (`1`); a non-empty run of `R` samples equal
//!   to the left one is then coded exactly like the run of T.87 A.7 for `R − 1`: blocks of `2^J[ri]` (a one bit each,
//!   `ri` grows), a zero bit and `J[ri]` bits of remainder (MSB first) for the rest, after which `ri` shrinks by one
//!   (not below 0). A run that reaches the end of the row has no zero bit and no remainder, and `ri` is not shrunk.
//!   `ri` is one state per plane (carried across rows, capped at 31). The sample that ended the run is coded as a
//!   residual against the above sample (the left one when they are equal); it is never equal to the run's value
//!   but the code does not exploit that.
//!
//! Not covered: the lossy wavelet mode (`C-RAW`), roll-burst (`encType 3`), dual-pixel data.

use crate::{RawError, Result};

/// Run-mode block exponents (T.87 `J[]`).
const J: [u8; 32] = [0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 9, 10, 11, 12, 13, 14, 15];
/// Highest run-mode state.
const MAX_RI: usize = J.len() - 1;
/// Unary zeros that introduce the escape code, and the width of its raw value.
const ESCAPE_ZEROS: u32 = 41;
/// Longest unary prefix we accept (the first sample of a plane uses one zero more than later escapes).
const MAX_ZEROS: u32 = 42;
const ESCAPE_BITS: u32 = 21;
/// Largest Golomb parameter the adaptation may reach (keeps shifts defined on hostile streams).
const MAX_K: u32 = 24;

/// MSB-first bit reader over a byte slice that errors at the end of data.
struct Bits<'a> {
    data: &'a [u8],
    next: usize,
    acc: u64,
    n: u32,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Self {
        Bits { data, next: 0, acc: 0, n: 0 }
    }

    #[inline]
    fn refill(&mut self) {
        while self.n <= 56 {
            let Some(&b) = self.data.get(self.next) else { break };
            self.acc |= (b as u64) << (56 - self.n);
            self.n += 8;
            self.next += 1;
        }
    }

    #[inline]
    fn eof() -> RawError {
        RawError::Corrupt("CRX stream ends inside a code".into())
    }

    /// One bit.
    #[inline]
    fn bit(&mut self) -> Result<u32> {
        if self.n == 0 {
            self.refill();
            if self.n == 0 {
                return Err(Self::eof());
            }
        }
        let b = (self.acc >> 63) as u32;
        self.acc <<= 1;
        self.n -= 1;
        Ok(b)
    }

    /// The next `n ≤ 32` bits as a number (0 for `n == 0`).
    #[inline]
    fn bits(&mut self, n: u32) -> Result<u32> {
        if n == 0 {
            return Ok(0);
        }
        if self.n < n {
            self.refill();
            if self.n < n {
                return Err(Self::eof());
            }
        }
        let v = (self.acc >> (64 - n)) as u32;
        self.acc <<= n;
        self.n -= n;
        Ok(v)
    }

    /// Number of zero bits before the next one bit (which is consumed). Errors after more than `max` zeros.
    #[inline]
    fn zeros(&mut self, max: u32) -> Result<u32> {
        let mut count = 0u32;
        loop {
            if self.n == 0 {
                self.refill();
                if self.n == 0 {
                    return Err(Self::eof());
                }
            }
            let lz = self.acc.leading_zeros().min(self.n);
            if lz < self.n {
                count += lz;
                self.acc = self.acc.checked_shl(lz + 1).unwrap_or(0);
                self.n -= lz + 1;
                return if count > max { Err(RawError::Corrupt("CRX unary code too long".into())) } else { Ok(count) };
            }
            count += self.n;
            self.acc = 0;
            self.n = 0;
            if count > max {
                return Err(RawError::Corrupt("CRX unary code too long".into()));
            }
        }
    }
}

#[inline]
fn zigzag(v: u32) -> i32 {
    ((v >> 1) as i32) ^ -((v & 1) as i32)
}

#[inline]
fn med(a: i32, b: i32, c: i32) -> i32 {
    if c >= a.max(b) {
        a.min(b)
    } else if c <= a.min(b) {
        a.max(b)
    } else {
        a + b - c
    }
}

/// Golomb parameter after a symbol (see the module docs).
#[inline]
fn next_k(k: u32, w: u32) -> u32 {
    let q = w >> k;
    if q >= 6 {
        (k + 2).min(MAX_K)
    } else if q >= 3 {
        (k + 1).min(MAX_K)
    } else if k > 0 && w < (1 << (k - 1)) {
        k - 1
    } else {
        k
    }
}

/// `|above-right − above|` of column `i`; the last column has no above-right neighbour and uses the residual.
#[inline]
fn gradient(up: &[u16], i: usize, w: usize, v: u32) -> u32 {
    if i + 1 >= w {
        return (v + 1) >> 1;
    }
    let b = up.get(i).copied().unwrap_or(0) as i32;
    let d = up.get(i + 1).copied().unwrap_or(0) as i32;
    b.abs_diff(d)
}

/// One Golomb-Rice coded value `v` with parameter `k`.
#[inline]
fn read_value(br: &mut Bits, k: u32) -> Result<u32> {
    let q = br.zeros(MAX_ZEROS)?;
    if q >= ESCAPE_ZEROS {
        return br.bits(ESCAPE_BITS);
    }
    let low = br.bits(k)?;
    Ok((q << k) | low)
}

/// A run's length (≥ 1) after the "non-empty" bit, and whether it reaches the end of the row. `room` is the number of
/// samples left in the row, `ri` the run state.
#[inline]
fn read_run(br: &mut Bits, ri: &mut usize, room: usize) -> Result<(usize, bool)> {
    let mut run = 1usize;
    loop {
        let jr = J.get(*ri).copied().unwrap_or(0) as u32;
        if br.bit()? == 1 {
            run += 1usize << jr;
            *ri = (*ri + 1).min(MAX_RI);
            if run >= room {
                return Ok((room, true));
            }
        } else {
            run += br.bits(jr)? as usize;
            *ri = ri.saturating_sub(1);
            return if run > room { Err(RawError::Corrupt("CRX run beyond the row".into())) } else { Ok((run, run == room)) };
        }
    }
}

/// Decode one plane stream of `w × h` samples with `bits` significant bits.
pub(crate) fn decode_plane(data: &[u8], w: usize, h: usize, bits: u32) -> Result<Vec<u16>> {
    if w == 0 || h == 0 || !(8..=16).contains(&bits) {
        return Err(RawError::Corrupt("bad CRX plane geometry".into()));
    }
    let total = w.checked_mul(h).filter(|t| *t <= crate::MAX_SAMPLES).ok_or(RawError::Limit("CRX plane too large"))?;
    let maxv = (1i32 << bits) - 1;
    let mut out = vec![0u16; total];
    let mut br = Bits::new(data);
    let mut k = 2u32;
    let mut ri = 0usize;

    // first sample: residual against mid-scale, escape coded; it does not adapt k
    let v0 = read_value(&mut br, k)?;
    let first = (1i32 << (bits - 1)) + zigzag(v0);
    let store = |x: i32, y: usize, i: usize| -> Result<u16> {
        if (0..=maxv).contains(&x) { Ok(x as u16) } else { Err(RawError::Corrupt(format!("CRX sample out of range ({x}) at row {y} column {i}"))) }
    };
    let mut prev_row_start = 0usize;
    for y in 0..h {
        let row_start = y * w;
        let (done, rest) = out.split_at_mut(row_start);
        let up = done.get(prev_row_start..).unwrap_or(&[]);
        let cur = rest.get_mut(..w).ok_or_else(|| RawError::Corrupt("CRX row".into()))?;
        let mut i = 0usize;
        while i < w {
            // neighbourhood
            let (a, b, d);
            let pred;
            if y == 0 {
                a = if i > 0 { cur.get(i - 1).copied().unwrap_or(0) as i32 } else { 0 };
                b = 0;
                d = 0;
                pred = if i == 0 { 1i32 << (bits - 1) } else { a };
            } else {
                b = up.get(i).copied().unwrap_or(0) as i32;
                if i == 0 {
                    a = b;
                    d = up.get(1.min(w - 1)).copied().unwrap_or(0) as i32;
                    pred = b;
                } else {
                    a = cur.get(i - 1).copied().unwrap_or(0) as i32;
                    let c = up.get(i - 1).copied().unwrap_or(0) as i32;
                    d = up.get((i + 1).min(w - 1)).copied().unwrap_or(0) as i32;
                    pred = med(a, b, c);
                }
            }
            if y > 0 && i + 1 < w && a == b && b == d {
                // run mode
                let (run, eol) = if br.bit()? == 1 { read_run(&mut br, &mut ri, w - i)? } else { (0, false) };
                let end = i + run;
                if let Some(s) = cur.get_mut(i..end) {
                    s.fill(a as u16);
                }
                i = end;
                if eol || i >= w {
                    break;
                }
                // the sample that broke the run
                let b2 = up.get(i).copied().unwrap_or(0) as i32;
                let p2 = if a != b2 { b2 } else { a };
                let v = read_value(&mut br, k)?;
                let x = p2 + zigzag(v);
                if let Some(s) = cur.get_mut(i) {
                    *s = store(x, y, i)?;
                }
                k = next_k(k, (v >> 1) + gradient(up, i, w, v));
                i += 1;
                continue;
            }
            let v = if y == 0 && i == 0 { v0 } else { read_value(&mut br, k)? };
            let x = if y == 0 && i == 0 { first } else { pred + zigzag(v) };
            if let Some(s) = cur.get_mut(i) {
                *s = store(x, y, i)?;
            }
            if !(y == 0 && i == 0) {
                k = next_k(k, (v >> 1) + if y == 0 { (v + 1) >> 1 } else { gradient(up, i, w, v) });
            }
            i += 1;
        }
        prev_row_start = row_start;
    }
    Ok(out)
}

/// A test-only CRX plane encoder (the rules of the module docs, written independently of the decoder's control flow),
/// shared with the container tests of [`super::cr3`].
#[cfg(test)]
pub(crate) mod testenc {
    use super::*;

    /// MSB-first bit writer for the test encoder.
    #[derive(Default)]
    struct BitWriter {
        out: Vec<u8>,
        acc: u64,
        n: u32,
    }

    impl BitWriter {
        fn put(&mut self, value: u32, count: u32) {
            for t in (0..count).rev() {
                self.acc = (self.acc << 1) | ((value >> t) & 1) as u64;
                self.n += 1;
                if self.n == 8 {
                    self.out.push(self.acc as u8);
                    self.acc = 0;
                    self.n = 0;
                }
            }
        }
        fn finish(mut self) -> Vec<u8> {
            if self.n > 0 {
                let pad = 8 - self.n;
                self.put(0, pad);
            }
            self.out
        }
    }

    fn unzigzag(d: i32) -> u32 {
        if d >= 0 { (d as u32) << 1 } else { (((-d) as u32) << 1) - 1 }
    }

    fn put_value(bw: &mut BitWriter, v: u32, k: u32, first: bool) {
        let q = v >> k;
        if q >= ESCAPE_ZEROS {
            for _ in 0..(ESCAPE_ZEROS + first as u32) {
                bw.put(0, 1);
            }
            bw.put(1, 1);
            bw.put(v, ESCAPE_BITS);
        } else {
            for _ in 0..q {
                bw.put(0, 1);
            }
            bw.put(1, 1);
            bw.put(v & ((1u32 << k) - 1), k);
        }
    }

    pub(crate) fn encode_plane(px: &[u16], w: usize, h: usize, bits: u32) -> Vec<u8> {
        let mut bw = BitWriter::default();
        let mut k = 2u32;
        let mut ri = 0usize;
        let at = |x: usize, y: usize| px[y * w + x] as i32;
        put_value(&mut bw, unzigzag(at(0, 0) - (1i32 << (bits - 1))), k, true);
        let mut i = 1usize;
        for y in 0..h {
            while i < w {
                let b = if y > 0 { at(i, y - 1) } else { 0 };
                let a = if y == 0 {
                    at(i - 1, y)
                } else if i == 0 {
                    b
                } else {
                    at(i - 1, y)
                };
                let d = if y > 0 { at((i + 1).min(w - 1), y - 1) } else { 0 };
                if y > 0 && i + 1 < w && a == b && b == d {
                    let mut run = 0usize;
                    while i + run < w && at(i + run, y) == a {
                        run += 1;
                    }
                    if run == 0 {
                        bw.put(0, 1);
                    } else {
                        bw.put(1, 1);
                        let mut n = run - 1;
                        while n >= (1usize << J[ri]) {
                            bw.put(1, 1);
                            n -= 1usize << J[ri];
                            ri = (ri + 1).min(MAX_RI);
                        }
                        if i + run == w {
                            if n > 0 {
                                bw.put(1, 1); // partial last block
                                ri = (ri + 1).min(MAX_RI);
                            }
                            break;
                        }
                        bw.put(0, 1);
                        bw.put(n as u32, J[ri] as u32);
                        ri = ri.saturating_sub(1);
                    }
                    i += run;
                    let b2 = at(i, y - 1);
                    let p2 = if a != b2 { b2 } else { a };
                    let v = unzigzag(at(i, y) - p2);
                    put_value(&mut bw, v, k, false);
                    let g = if i + 1 >= w { (v + 1) >> 1 } else { at(i, y - 1).abs_diff(at(i + 1, y - 1)) };
                    k = next_k(k, (v >> 1) + g);
                    i += 1;
                    continue;
                }
                let pred = if y == 0 {
                    a
                } else if i == 0 {
                    b
                } else {
                    med(a, b, at(i - 1, y - 1))
                };
                let v = unzigzag(at(i, y) - pred);
                put_value(&mut bw, v, k, false);
                let g = if y == 0 || i + 1 >= w { (v + 1) >> 1 } else { at(i, y - 1).abs_diff(at(i + 1, y - 1)) };
                k = next_k(k, (v >> 1) + g);
                i += 1;
            }
            i = 0;
        }
        bw.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::testenc::encode_plane;
    use super::*;

    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u32 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (self.0 >> 33) as u32
        }
    }

    fn plane(w: usize, h: usize, bits: u32, seed: u64, noise: u32, flat: u32) -> Vec<u16> {
        let mut r = Lcg(seed);
        let max = (1u32 << bits) - 1;
        let mut px = vec![0u16; w * h];
        for y in 0..h {
            for x in 0..w {
                // smooth ramp, flat patches (long runs) and noise
                let base = ((x * 3 + y * 5) as u32 * (max / 2048 + 1)) % (max + 1);
                let patch = (x / 37 + y / 11) % 3 == 0;
                let n = if patch && r.next() % 100 < flat { 0 } else { r.next() % (noise + 1) };
                px[y * w + x] = (base + n).min(max) as u16;
            }
        }
        px
    }

    fn round_trip(px: &[u16], w: usize, h: usize, bits: u32) {
        let enc = encode_plane(px, w, h, bits);
        let dec = decode_plane(&enc, w, h, bits).unwrap_or_else(|e| panic!("{w}x{h}: {e}"));
        assert_eq!(dec, px, "{w}x{h} bits {bits}");
    }

    #[test]
    fn round_trips_noisy_and_flat_planes() {
        for (w, h, bits, noise, flat) in [(64, 32, 14, 6, 90), (257, 40, 14, 2, 98), (131, 29, 12, 40, 50), (300, 17, 16, 3, 100), (97, 23, 8, 1, 95)]
        {
            round_trip(&plane(w, h, bits, (w * h) as u64, noise, flat), w, h, bits);
        }
    }

    #[test]
    fn round_trips_tiny_planes() {
        for (w, h) in [(1, 1), (1, 5), (2, 2), (2, 7), (3, 3), (5, 1), (4, 9)] {
            for bits in [8, 14, 16] {
                round_trip(&plane(w, h, bits, (w * 31 + h) as u64, 3, 60), w, h, bits);
            }
        }
    }

    #[test]
    fn round_trips_long_runs_and_escapes() {
        // a flat plane grows the run state through its whole table; the extremes force the escape code
        let (w, h) = (6000usize, 6usize);
        let mut px = vec![9000u16; w * h];
        round_trip(&px, w, h, 14);
        px[w + 17] = 0;
        px[2 * w + 3000] = 16383;
        px[3 * w] = 16383;
        px[4 * w + w - 1] = 1;
        round_trip(&px, w, h, 14);
        let mut r = Lcg(5);
        let spiky: Vec<u16> = (0..64 * 16).map(|_| if r.next().is_multiple_of(2) { 0 } else { 65535 }).collect();
        round_trip(&spiky, 64, 16, 16);
    }

    #[test]
    fn run_ends_exactly_at_the_row_end_or_one_short() {
        // runs reaching the last column, one short of it, and one that leaves a single sample
        for tail in 1..40usize {
            let w = 40usize;
            let mut px = vec![500u16; w * 3];
            for x in (w - tail)..w {
                px[2 * w + x] = 501;
            }
            round_trip(&px, w, 3, 12);
        }
    }

    #[test]
    fn truncated_streams_error() {
        let (w, h) = (128usize, 24usize);
        let px = plane(w, h, 14, 3, 5, 90);
        let enc = encode_plane(&px, w, h, 14);
        for cut in [0, 1, 2, enc.len() / 3, enc.len() - 8] {
            assert!(decode_plane(&enc[..cut], w, h, 14).is_err(), "cut at {cut}");
        }
    }

    #[test]
    fn hostile_bytes_never_panic() {
        let mut r = Lcg(99);
        for round in 0..300 {
            let len = (r.next() % 400) as usize;
            let data: Vec<u8> = (0..len).map(|_| if round % 3 == 0 { 0 } else { r.next() as u8 }).collect();
            let (w, h) = (1 + (r.next() % 70) as usize, 1 + (r.next() % 20) as usize);
            let _ = decode_plane(&data, w, h, 8 + r.next() % 9);
        }
        assert!(decode_plane(&[0xff; 64], 0, 4, 14).is_err());
        assert!(decode_plane(&[0xff; 64], 4, 4, 7).is_err());
        assert!(decode_plane(&[0xff; 64], usize::MAX, 2, 14).is_err());
        assert!(decode_plane(&[0u8; 64], 4, 4, 14).is_err(), "an endless unary prefix is an error");
    }

    #[test]
    fn out_of_range_samples_are_errors() {
        let px = plane(40, 8, 14, 11, 20, 50);
        let enc = encode_plane(&px, 40, 8, 14);
        // read as 8-bit samples, the 14-bit values cannot fit
        assert!(decode_plane(&enc, 40, 8, 8).is_err());
    }

    #[test]
    fn bit_reader_unary_and_fields() {
        let mut br = Bits::new(&[0b0001_0110, 0b1000_0000]);
        assert_eq!(br.zeros(8).unwrap(), 3);
        assert_eq!(br.bits(3).unwrap(), 0b011);
        assert_eq!(br.bit().unwrap(), 0);
        assert_eq!(br.bits(0).unwrap(), 0);
        assert!(br.bits(9).is_err());
    }
}
