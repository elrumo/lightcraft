//! Tiled enlargement. SPAN has no strides, windows or global operations, so a window cut from the
//! image gives the same pixels as the whole image wherever it is further than the network's
//! receptive field (21 input pixels) from the cut. Each tile is processed with a halo of
//! [`HALO`] pixels of real context and only its centre is kept; where a window reaches the
//! image's edge it ends there, exactly as a pass over the whole image would.
//!
//! **Tone anchor.** With the reference normalisation, SPAN's output is about 1.5 % darker than
//! its input in broad areas (measured on photos, against the input and against a second model
//! trained alike). An enlargement must not change an image's tones, so the broad tones of each
//! tile's result are replaced by those of the input enlarged by bicubic interpolation
//! (`out + blur(up) − blur(out)`); the network's detail is untouched. The blur is local, so
//! tiles still match a pass over the whole image.

use candle_core::Tensor;
use lightcraft_raster::Rgb32f;

use crate::span::Span;
use crate::{Error, Result};

/// Context around each tile, in input pixels: SPAN's 21-pixel receptive field, plus what the
/// tone anchor reads (its blur and the bicubic kernel), rounded up.
pub const HALO: usize = 32;

/// Radius of the tone anchor's blur, in input pixels.
const ANCHOR_RADIUS: usize = 4;

/// Tiling and size limits.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Side of the input area each tile produces, in input pixels (16…2048).
    pub tile: usize,
    /// Most output pixels accepted (the result is 12 bytes per pixel in memory).
    pub max_output_pixels: usize,
}

impl Default for Options {
    fn default() -> Self {
        Options { tile: 384, max_output_pixels: 1 << 28 }
    }
}

impl Span {
    /// The enlarged size of a `w` × `h` picture, or why it can't be enlarged.
    fn enlarged_size(&self, (w, h): (usize, usize), opts: &Options) -> Result<(usize, usize)> {
        let s = self.scale();
        if w == 0 || h == 0 {
            return Err(Error::Image(format!("a {w} × {h} image can't be enlarged")));
        }
        let (ow, oh) = (w.checked_mul(s), h.checked_mul(s));
        let out_px = ow.zip(oh).and_then(|(a, b)| a.checked_mul(b));
        let (Some(ow), Some(oh), Some(out_px)) = (ow, oh, out_px) else { return Err(Error::Image("the enlarged image is too large".into())) };
        if out_px > opts.max_output_pixels {
            return Err(Error::Image(format!(
                "the enlarged image would be {:.0} megapixels (at most {:.0})",
                out_px as f64 / 1e6,
                opts.max_output_pixels as f64 / 1e6
            )));
        }
        Ok((ow, oh))
    }

    /// Enlarge `img` (sRGB-encoded, 0…1) by [`Span::scale`]. `progress` is called before every
    /// tile with the fraction done; it returns `false` to stop ([`Error::Cancelled`]).
    pub fn upscale(&self, img: &Rgb32f, opts: &Options, progress: &mut dyn FnMut(f32) -> bool) -> Result<Rgb32f> {
        let (w, h) = (img.width, img.height);
        if Some(img.data.len()) != w.checked_mul(h) {
            return Err(Error::Image(format!("a {w} × {h} image can't be enlarged")));
        }
        let (ow, oh) = self.enlarged_size((w, h), opts)?;
        let mut out = Rgb32f::new(ow, oh);
        let read = |x: usize, y: usize| img.data.get(y * w + x).copied().unwrap_or_default();
        let mut write = |x: usize, y: usize, p: [f32; 3]| {
            if let Some(d) = out.data.get_mut(y * ow + x) {
                *d = p;
            }
        };
        self.upscale_with((w, h), &read, &mut write, opts, progress)?;
        Ok(out)
    }

    /// [`Self::upscale`] for pictures held in any layout (for instance 16-bit samples, which take
    /// half the memory of a float copy): `read(x, y)` gives the pixel at `x < w, y < h` (sRGB,
    /// 0…1), and `write(x, y, pixel)` receives every pixel of the enlargement (`x < w·scale`),
    /// each once, in tile order, already within 0…1.
    pub fn upscale_with(
        &self,
        (w, h): (usize, usize),
        read: &dyn Fn(usize, usize) -> [f32; 3],
        write: &mut dyn FnMut(usize, usize, [f32; 3]),
        opts: &Options,
        progress: &mut dyn FnMut(f32) -> bool,
    ) -> Result<()> {
        self.enlarged_size((w, h), opts)?;
        let s = self.scale();
        let tile = opts.tile.clamp(16, 2048);
        let (tiles_x, tiles_y) = (w.div_ceil(tile), h.div_ceil(tile));
        let total = tiles_x * tiles_y;
        for n in 0..total {
            if !progress(n as f32 / total as f32) {
                return Err(Error::Cancelled);
            }
            let (x0, y0) = ((n % tiles_x) * tile, (n / tiles_x) * tile);
            let (x1, y1) = ((x0 + tile).min(w), (y0 + tile).min(h));
            let (wx0, wy0) = (x0.saturating_sub(HALO), y0.saturating_sub(HALO));
            let (wx1, wy1) = ((x1 + HALO).min(w), (y1 + HALO).min(h));
            let (ww, wh) = (wx1 - wx0, wy1 - wy0);

            // planar input window
            let mut planes = vec![0f32; 3 * ww * wh];
            for (y, dst) in (wy0..wy1).zip(0..) {
                for (x, src) in (wx0..wx1).zip(0..) {
                    for (c, v) in read(x, y).iter().enumerate() {
                        if let Some(d) = planes.get_mut(c * ww * wh + dst * ww + src) {
                            // (NaN / inf in a damaged source must not poison the whole tile)
                            *d = if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
                        }
                    }
                }
            }
            let input = Tensor::from_vec(planes.clone(), (1, 3, wh, ww), self.device())?;
            let mut result = self.forward(&input)?.squeeze(0)?.flatten_all()?.to_vec1::<f32>()?;

            // keep the tile's centre
            let (rw, rh) = (ww * s, wh * s);
            if result.len() != 3 * rw * rh {
                return Err(Error::Model(format!("the network returned {} values for a {rw} × {rh} tile", result.len())));
            }
            anchor_tones(&mut result, &planes, (ww, wh), s);
            for y in y0 * s..y1 * s {
                for x in x0 * s..x1 * s {
                    let src = (y - wy0 * s) * rw + (x - wx0 * s);
                    let mut px = [0f32; 3];
                    for (c, v) in px.iter_mut().enumerate() {
                        let r = result.get(c * rw * rh + src).copied().unwrap_or(0.0);
                        // (also maps NaN to 0)
                        *v = if r > 0.0 { r.min(1.0) } else { 0.0 };
                    }
                    write(x, y, px);
                }
            }
        }
        progress(1.0);
        Ok(())
    }
}

/// Value of `v[i]`, 0 outside (indices here are computed from sizes this module chose, but a
/// slip must not panic).
fn at(v: &[f32], i: usize) -> f32 {
    v.get(i).copied().unwrap_or(0.0)
}

/// Move the broad tones of `result` (planar RGB, `s` times the size of `input`) to those of
/// `input` enlarged by bicubic interpolation: `result += blur(up) − blur(result)`.
fn anchor_tones(result: &mut [f32], input: &[f32], (w, h): (usize, usize), s: usize) {
    let (rw, rh) = (w * s, h * s);
    for (out, inp) in result.chunks_exact_mut(rw * rh).zip(input.chunks_exact(w * h)) {
        let low_up = box_blur(&bicubic(inp, (w, h), s), (rw, rh), ANCHOR_RADIUS * s);
        let low_out = box_blur(out, (rw, rh), ANCHOR_RADIUS * s);
        for (o, (u, l)) in out.iter_mut().zip(low_up.iter().zip(&low_out)) {
            *o += u - l;
        }
    }
}

/// Mean over a `(2r + 1)`-wide square, edges extended.
pub(crate) fn box_blur(plane: &[f32], (w, h): (usize, usize), r: usize) -> Vec<f32> {
    let n = (2 * r + 1) as f64;
    let clamp = |i: isize, len: usize| i.clamp(0, len as isize - 1) as usize;
    let r = r as isize;
    // rows
    let mut tmp = vec![0f32; w * h];
    for y in 0..h {
        let row = |x: isize| f64::from(at(plane, y * w + clamp(x, w)));
        let mut sum: f64 = (-r..=r).map(row).sum();
        for x in 0..w as isize {
            if let Some(d) = tmp.get_mut(y * w + x as usize) {
                *d = (sum / n) as f32;
            }
            sum += row(x + r + 1) - row(x - r);
        }
    }
    // columns
    let mut out = vec![0f32; w * h];
    for x in 0..w {
        let col = |y: isize| f64::from(at(&tmp, clamp(y, h) * w + x));
        let mut sum: f64 = (-r..=r).map(col).sum();
        for y in 0..h as isize {
            if let Some(d) = out.get_mut(y as usize * w + x) {
                *d = (sum / n) as f32;
            }
            sum += col(y + r + 1) - col(y - r);
        }
    }
    out
}

/// Catmull-Rom enlargement by the integer factor `s`, edges extended.
pub(crate) fn bicubic(plane: &[f32], (w, h): (usize, usize), s: usize) -> Vec<f32> {
    let weights = |t: f32| {
        [
            -0.5 * t * t * t + t * t - 0.5 * t,
            1.5 * t * t * t - 2.5 * t * t + 1.0,
            -1.5 * t * t * t + 2.0 * t * t + 0.5 * t,
            0.5 * t * t * t - 0.5 * t * t,
        ]
    };
    // for each output coordinate: the four source indices and their weights
    let axis = |len: usize| -> Vec<([usize; 4], [f32; 4])> {
        (0..len * s)
            .map(|o| {
                let f = (o as f32 + 0.5) / s as f32 - 0.5;
                let i = f.floor();
                let idx = [-1isize, 0, 1, 2].map(|k| (i as isize + k).clamp(0, len as isize - 1) as usize);
                (idx, weights(f - i))
            })
            .collect()
    };
    let (xs, ys) = (axis(w), axis(h));
    let mut out = Vec::with_capacity(xs.len() * ys.len());
    for (yi, yw) in &ys {
        for (xi, xw) in &xs {
            let mut acc = 0f32;
            for (j, wj) in yi.iter().zip(yw) {
                for (i, wi) in xi.iter().zip(xw) {
                    acc += wi * wj * at(plane, j * w + i);
                }
            }
            out.push(acc);
        }
    }
    out
}
