//! Decoded 4:2:0 pictures → linear Rec.2020 (the develop working space).
//!
//! The conversion follows the picture's own colour description (ITU-T H.273 code points from the stream's VUI or the
//! container's `colr` box): the matrix and range that turn Y'CbCr into R'G'B', the transfer curve that linearises it,
//! the primaries that map it into the working space. Chroma is upsampled by nearest neighbour; a picture larger than the
//! caller needs is reduced during the conversion by averaging whole blocks of luma, so a 4K frame wanted at 512 px never
//! exists as a 4K float image.

use lightcraft_color::{DISPLAY_P3, REC2020, RgbSpace, SRGB, WORKING, transfer};
use lightcraft_raster::{Rgb32f, par_rows};

use crate::{Error, Result};

/// Largest picture accepted (width × height).
const MAX_PIXELS: usize = 1 << 26;
/// Largest side accepted.
const MAX_SIDE: usize = 16384;

/// One plane of samples: 8-bit streams give bytes, deeper ones 16-bit words.
pub(crate) enum Plane {
    U8(Vec<u8>),
    U16(Vec<u16>),
}

impl Plane {
    fn len(&self) -> usize {
        match self {
            Plane::U8(v) => v.len(),
            Plane::U16(v) => v.len(),
        }
    }

    #[inline]
    fn at(&self, i: usize) -> u32 {
        match self {
            Plane::U8(v) => v.get(i).map_or(0, |s| u32::from(*s)),
            Plane::U16(v) => v.get(i).map_or(0, |s| u32::from(*s)),
        }
    }
}

/// How the samples are to be read (H.273 code points; 2 = unspecified).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Color {
    pub full_range: bool,
    pub primaries: u16,
    pub transfer: u16,
    pub matrix: u16,
}

impl Color {
    pub const UNSPECIFIED: Color = Color { full_range: false, primaries: 2, transfer: 2, matrix: 2 };

    /// This description with the unspecified parts taken from `other` (the container's, when the stream says nothing).
    pub fn or(self, other: Color) -> Color {
        let pick = |a: u16, b: u16| if a == 2 || a == 0 { b } else { a };
        Color {
            full_range: self.full_range || (self.matrix == 2 && other.full_range),
            primaries: pick(self.primaries, other.primaries),
            transfer: pick(self.transfer, other.transfer),
            matrix: pick(self.matrix, other.matrix),
        }
    }
}

/// A decoded picture: planar 4:2:0, cropped, `pts` as the caller gave it.
pub(crate) struct Frame {
    pub width: usize,
    pub height: usize,
    pub chroma_width: usize,
    pub chroma_height: usize,
    pub bit_depth: u8,
    pub y: Plane,
    pub u: Plane,
    pub v: Plane,
    pub y_stride: usize,
    pub uv_stride: usize,
    pub pts: i64,
    pub color: Color,
}

impl Frame {
    /// Is the picture's geometry sound, so that no sample read below can fall outside a plane?
    pub fn check(&self) -> Result<()> {
        let (w, h) = (self.width, self.height);
        let bad = |what: &str| Err(Error::Malformed(format!("the decoder returned a picture with {what}")));
        if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE || w.saturating_mul(h) > MAX_PIXELS {
            return bad("an impossible size");
        }
        if !(8..=12).contains(&self.bit_depth) {
            return bad("an unsupported bit depth");
        }
        if self.chroma_width < w.div_ceil(2) || self.chroma_height < h.div_ceil(2) {
            return bad("chroma planes too small");
        }
        let need = |stride: usize, rows: usize, cols: usize| rows.saturating_sub(1).saturating_mul(stride).saturating_add(cols);
        if self.y_stride < w || self.uv_stride < self.chroma_width {
            return bad("short strides");
        }
        if self.y.len() < need(self.y_stride, h, w)
            || self.u.len() < need(self.uv_stride, self.chroma_height, self.chroma_width)
            || self.v.len() < need(self.uv_stride, self.chroma_height, self.chroma_width)
        {
            return bad("short planes");
        }
        Ok(())
    }

    /// Mean luma from black 0 to white 1, read with the picture's range (every 4th sample each way: a black-frame
    /// test, not a measurement).
    pub fn mean_luma(&self) -> f32 {
        let (mut sum, mut n) = (0u64, 0u64);
        for y in (0..self.height).step_by(4) {
            for x in (0..self.width).step_by(4) {
                sum += u64::from(self.y.at(y * self.y_stride + x));
                n += 1;
            }
        }
        if n == 0 {
            return 0.0;
        }
        let mean = sum as f32 / n as f32;
        let scale = (1u32 << self.bit_depth.saturating_sub(8)) as f32;
        let (off, rng) = if self.color.full_range { (0.0, ((1u32 << self.bit_depth) - 1) as f32) } else { (16.0 * scale, 219.0 * scale) };
        ((mean - off) / rng).clamp(0.0, 1.0)
    }
}

/// Kr, Kb of a matrix code point; unspecified follows the usual size rule (HD is BT.709).
fn coefficients(matrix: u16, height: usize) -> (f32, f32) {
    match matrix {
        1 => (0.2126, 0.0722),
        9 | 10 => (0.2627, 0.0593),
        4 => (0.30, 0.11),
        5 | 6 => (0.299, 0.114),
        7 => (0.212, 0.087),
        _ if height >= 720 => (0.2126, 0.0722),
        _ => (0.299, 0.114),
    }
}

fn primaries(code: u16) -> &'static RgbSpace {
    match code {
        9 => &REC2020,
        12 => &DISPLAY_P3,
        _ => &SRGB,
    }
}

/// The soft shoulder that brings an HDR highlight into the SDR range a poster is shown in: linear up to 0.8, then
/// approaching 1.
fn knee(x: f32) -> f32 {
    if x <= 0.8 { x } else { 0.8 + 0.2 * (1.0 - (-(x - 0.8) / 0.4).exp()) }
}

/// R'G'B' signal 0..1 → linear light, 1.0 = SDR white. SDR curves are read as sRGB (what a phone's own gallery does);
/// PQ and HLG get a plain mapping to SDR white with a soft shoulder.
fn linear(transfer_code: u16, v: f32) -> f32 {
    match transfer_code {
        16 => knee((transfer::pq_decode(f64::from(v)) * 10000.0 / 203.0) as f32),
        18 => {
            // HLG: scene light normalised so that the 75 % signal (reference white) is 1.0
            let white = transfer::hlg_decode(0.75);
            knee((transfer::hlg_decode(f64::from(v)) / white) as f32)
        }
        8 => v,
        _ => transfer::srgb_to_linear(v),
    }
}

/// `f` as linear Rec.2020 in display orientation with its larger side at most about `2 * max_edge` (never smaller than
/// `max_edge`, unless the picture is): the caller finishes with a proper resize.
pub(crate) fn to_working(f: &Frame, max_edge: usize, rotation: u8) -> Result<Rgb32f> {
    f.check()?;
    let long = f.width.max(f.height);
    let factor = (long / max_edge.max(1).saturating_mul(2)).max(1);
    let (ow, oh) = ((f.width / factor).max(1), (f.height / factor).max(1));
    let (kr, kb) = coefficients(f.color.matrix, f.height);
    let kg = 1.0 - kr - kb;
    let (cr_r, cb_b) = (2.0 * (1.0 - kr), 2.0 * (1.0 - kb));
    let (cr_g, cb_g) = (cr_r * kr / kg, cb_b * kb / kg);
    let m = primaries(f.color.primaries).to_space(&WORKING).to_f32();
    let scale = (1u32 << (f.bit_depth - 8)) as f32;
    let (y_off, y_rng, c_mid, c_rng) = if f.color.full_range {
        (0.0, ((1u32 << f.bit_depth) - 1) as f32, 128.0 * scale, ((1u32 << f.bit_depth) - 1) as f32)
    } else {
        (16.0 * scale, 219.0 * scale, 128.0 * scale, 224.0 * scale)
    };
    let tf = f.color.transfer;
    let mut out = Rgb32f::new(ow, oh);
    par_rows(&mut out.data, ow, |oy, row| {
        for (ox, px) in row.iter_mut().enumerate() {
            // the block of luma this output pixel covers
            let (x0, y0) = (ox * factor, oy * factor);
            let (x1, y1) = ((x0 + factor).min(f.width), (y0 + factor).min(f.height));
            let (mut sum, mut n) = (0u64, 0u64);
            for sy in y0..y1 {
                for sx in x0..x1 {
                    sum += u64::from(f.y.at(sy * f.y_stride + sx));
                    n += 1;
                }
            }
            let luma = if n == 0 { 0.0 } else { sum as f32 / n as f32 };
            let (cx, cy) = (((x0 + x1) / 2) / 2, ((y0 + y1) / 2) / 2);
            let ci = cy * f.uv_stride + cx;
            let (cb, cr) = (f.u.at(ci) as f32, f.v.at(ci) as f32);
            let yn = ((luma - y_off) / y_rng).clamp(-0.07, 1.09);
            let (pb, pr) = ((cb - c_mid) / c_rng, (cr - c_mid) / c_rng);
            let r = (yn + cr_r * pr).clamp(0.0, 1.0);
            let g = (yn - cb_g * pb - cr_g * pr).clamp(0.0, 1.0);
            let b = (yn + cb_b * pb).clamp(0.0, 1.0);
            let lin = [linear(tf, r), linear(tf, g), linear(tf, b)];
            *px = [
                m[0][0] * lin[0] + m[0][1] * lin[1] + m[0][2] * lin[2],
                m[1][0] * lin[0] + m[1][1] * lin[1] + m[1][2] * lin[2],
                m[2][0] * lin[0] + m[2][1] * lin[1] + m[2][2] * lin[2],
            ];
        }
    });
    for _ in 0..(rotation % 4) {
        out = out.rotate_cw();
    }
    Ok(out)
}
