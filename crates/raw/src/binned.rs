//! Reduced-size development straight from the mosaic ("superpixel" binning).
//!
//! When the caller needs an image at most half (or a third, a quarter…) of the sensor size — a
//! 2560 px loupe preview or a grid thumbnail of a 24 MP raw — demosaicing the full mosaic and then
//! downscaling throws most of the work away. Instead every `k × k` block of CFA samples becomes one
//! output pixel whose channels are the means of the block's samples of that colour. Every block must
//! contain all three colours (Bayer: `k` even; X-Trans: `k` a multiple of 3). There is no
//! interpolation, so there are no demosaicing artefacts; the colour planes are offset by less than
//! one output pixel, which is invisible at these scales.
//!
//! Linear raws (`LinearRaw`: three samples per pixel, no mosaic — Apple ProRAW, Adobe's linear DNGs) bin the same
//! way with any `k ≥ 2`: each output channel is the mean of its `k × k` samples, so a 2560 px preview of a 48 MP file
//! never needs a full-size float copy.
//!
//! Clipping is preserved for [`crate::highlight::reconstruct`]: a channel whose block contains a
//! sample at or above `clip` takes that block's maximum sample of the colour (instead of a mean that
//! would fall just below the clip level and escape reconstruction).

use crate::{Normalized, RawData, RawImage, Result, Rgb32f};
use rayon::prelude::*;

impl RawImage {
    /// Can `k × k` blocks be binned (single-plane CFA data, every `k × k` window containing R, G
    /// and B; Bayer: `k` even, so that every block has the same layout)?
    pub fn can_bin(&self, k: usize) -> bool {
        if self.cfa.is_none() {
            return k >= 2 && self.cpp == 3;
        }
        let Some(cfa) = &self.cfa else { return false };
        if k < 2 || self.cpp != 1 || !cfa.valid() || (cfa.is_bayer() && !k.is_multiple_of(2)) {
            return false;
        }
        // every window position relative to the pattern
        (0..cfa.height).all(|j| (0..cfa.width).all(|i| block_layout(cfa, i, j, k).1.iter().all(|&n| n > 0)))
    }

    /// Like [`RawImage::develop`] at `1/k` of the size (camera RGB, white = 1.0, default crop applied,
    /// not oriented). `None` when the data cannot be binned ([`RawImage::can_bin`]) or carries an
    /// `OpcodeList3` (whose operations are defined at full resolution); callers then demosaic.
    pub fn develop_binned(&self, k: usize, clip: f32) -> Result<Option<Rgb32f>> {
        self.validate()?;
        if !self.can_bin(k) || !self.opcodes.list3.is_empty() {
            return Ok(None);
        }
        let Some(cfa) = &self.cfa else { return self.develop_binned_linear(k, clip) };
        let a = self.active_area;
        // bin the default crop (relative to the active area) only
        let c = self.crop.clipped(a.width, a.height);
        let c = if c.width < k || c.height < k { crate::Rect::new(0, 0, a.width, a.height) } else { c };
        let (bw, bh) = (c.width / k, c.height / k);
        if bw == 0 || bh == 0 {
            return Ok(None);
        }
        // Opcode lists 1/2 need the normalised copy; plain files are normalised row by row.
        let norm: Option<Normalized> = if self.opcodes.list1.is_empty() && self.opcodes.list2.is_empty() { None } else { Some(self.normalized()?) };
        let cfa = cfa.shifted(a.x, a.y);
        let range = self.white_at(0) - self.black.mean();
        let scale = if range > 0.0 { 1.0 / range } else { 1.0 };
        let uniform_black =
            self.black.delta_h.is_empty() && self.black.delta_v.is_empty() && self.black.values.iter().all(|&v| v == self.black.values[0]);
        let black0 = self.black.at(0, 0, 0, 1);
        // colour of every sample of a block, per pattern phase of the block's corner
        let (pw, ph) = (cfa.width, cfa.height);
        let layouts: Vec<(Vec<u8>, [f32; 3])> = (0..ph)
            .flat_map(|j| (0..pw).map(move |i| (i, j)))
            .map(|(i, j)| {
                let (l, n) = block_layout(&cfa, i, j, k);
                (l, n.map(|n| 1.0 / n.max(1) as f32))
            })
            .collect();
        let wlen = bw * k;
        let mut out = Rgb32f::new(bw, bh);
        out.data.par_chunks_mut(bw).enumerate().for_each(|(by, row)| {
            // the block row's k sample rows, normalised
            let mut rows = vec![0f32; k * wlen];
            for dy in 0..k {
                let y = c.y + by * k + dy;
                let dst = &mut rows[dy * wlen..(dy + 1) * wlen];
                match (&norm, &self.data) {
                    (Some(nm), _) => dst.copy_from_slice(&nm.data[y * nm.width + c.x..][..wlen]),
                    (None, RawData::U16(d)) if uniform_black => {
                        let src = &d[(a.y + y) * self.width + a.x + c.x..][..wlen];
                        for (o, &v) in dst.iter_mut().zip(src) {
                            *o = (v as f32 - black0) * scale;
                        }
                    }
                    (None, data) => {
                        let base = (a.y + y) * self.width + a.x + c.x;
                        for (x, o) in dst.iter_mut().enumerate() {
                            *o = (data.get(base + x) - self.black.at(c.x + x, y, 0, 1)) * scale;
                        }
                    }
                }
            }
            let py = (c.y + by * k) % ph;
            for (bx, px) in row.iter_mut().enumerate() {
                let (layout, inv) = &layouts[py * pw + (c.x + bx * k) % pw];
                let (mut sum, mut max) = ([0f32; 3], [f32::MIN; 3]);
                for dy in 0..k {
                    let src = &rows[dy * wlen + bx * k..][..k];
                    for (&v, &col) in src.iter().zip(&layout[dy * k..(dy + 1) * k]) {
                        let col = col as usize;
                        sum[col] += v;
                        max[col] = max[col].max(v);
                    }
                }
                for ch in 0..3 {
                    px[ch] = if max[ch] >= clip { max[ch] } else { sum[ch] * inv[ch] };
                }
            }
        });
        Ok(Some(out))
    }
}

impl RawImage {
    /// [`RawImage::develop_binned`] for three-sample-per-pixel data.
    fn develop_binned_linear(&self, k: usize, clip: f32) -> Result<Option<Rgb32f>> {
        let a = self.active_area;
        let c = self.crop.clipped(a.width, a.height);
        let c = if c.width < k || c.height < k { crate::Rect::new(0, 0, a.width, a.height) } else { c };
        let (bw, bh) = (c.width / k, c.height / k);
        if bw == 0 || bh == 0 {
            return Ok(None);
        }
        let norm: Option<Normalized> = if self.opcodes.list1.is_empty() && self.opcodes.list2.is_empty() { None } else { Some(self.normalized()?) };
        let scale: [f32; 3] = std::array::from_fn(|s| {
            let range = self.white_at(s) - self.black.mean();
            if range > 0.0 { 1.0 / range } else { 1.0 }
        });
        let flat_black = self.black.delta_h.is_empty() && self.black.delta_v.is_empty() && self.black.repeat_rows <= 1 && self.black.repeat_cols <= 1;
        let black: [f32; 3] = std::array::from_fn(|s| self.black.at(0, 0, s, 3));
        let inv = 1.0 / (k * k) as f32;
        let wlen = bw * k * 3;
        let mut out = Rgb32f::new(bw, bh);
        out.data.par_chunks_mut(bw).enumerate().for_each(|(by, row)| {
            // the block row's k sample rows, normalised
            let mut rows = vec![0f32; k * wlen];
            for dy in 0..k {
                let y = c.y + by * k + dy;
                let dst = &mut rows[dy * wlen..(dy + 1) * wlen];
                match (&norm, &self.data) {
                    (Some(nm), _) => dst.copy_from_slice(&nm.data[(y * nm.width + c.x) * 3..][..wlen]),
                    (None, RawData::U16(d)) if flat_black => {
                        let src = &d[((a.y + y) * self.width + a.x + c.x) * 3..][..wlen];
                        for (i, (o, &v)) in dst.iter_mut().zip(src).enumerate() {
                            *o = (v as f32 - black[i % 3]) * scale[i % 3];
                        }
                    }
                    (None, data) => {
                        let base = ((a.y + y) * self.width + a.x + c.x) * 3;
                        for (i, o) in dst.iter_mut().enumerate() {
                            *o = (data.get(base + i) - self.black.at(c.x + i / 3, y, i % 3, 3)) * scale[i % 3];
                        }
                    }
                }
            }
            for (bx, px) in row.iter_mut().enumerate() {
                let (mut sum, mut max) = ([0f32; 3], [f32::MIN; 3]);
                for dy in 0..k {
                    for v in rows[dy * wlen + bx * k * 3..][..k * 3].as_chunks::<3>().0 {
                        for ch in 0..3 {
                            sum[ch] += v[ch];
                            max[ch] = max[ch].max(v[ch]);
                        }
                    }
                }
                for ch in 0..3 {
                    px[ch] = if max[ch] >= clip { max[ch] } else { sum[ch] * inv };
                }
            }
        });
        Ok(Some(out))
    }
}

/// Colours of the `k × k` samples of a block whose corner sits at pattern position `(i, j)`
/// (row-major) and the count of each colour.
fn block_layout(cfa: &crate::Cfa, i: usize, j: usize, k: usize) -> (Vec<u8>, [u32; 3]) {
    let mut n = [0u32; 3];
    let l: Vec<u8> = (0..k * k)
        .map(|t| {
            let c = cfa.color_at(i + t % k, j + t / k).min(2);
            n[c as usize] += 1;
            c
        })
        .collect();
    (l, n)
}

#[cfg(test)]
mod tests {
    use crate::demosaic::mosaic_from_rgb;
    use crate::*;

    fn raw_from(n: &Normalized, black: f32, white: f32) -> RawImage {
        let data = n.data.iter().map(|&v| (v * (white - black) + black).round() as u16).collect();
        RawImage {
            format: RawFormat::Dng,
            width: n.width,
            height: n.height,
            cpp: 1,
            data: RawData::U16(data),
            cfa: n.cfa.clone(),
            bits: 14,
            black: BlackLevel::uniform(black),
            white: vec![white],
            active_area: Rect::new(0, 0, n.width, n.height),
            crop: Rect::new(0, 0, n.width, n.height),
            orientation: Orientation::Normal,
            color: ColorData::default(),
            wb_multipliers: None,
            linearized: false,
            opcodes: OpcodeLists::default(),
            metadata: Metadata::default(),
        }
    }

    #[test]
    fn binning_matches_block_means_and_keeps_clipping() {
        let img = Rgb32f::from_fn(24, 18, |x, y| [0.1 + x as f32 * 0.01, 0.2 + y as f32 * 0.01, 0.3]);
        let mut raw = raw_from(&mosaic_from_rgb(&img, &Cfa::bayer("GRBG").unwrap()), 512.0, 16383.0);
        assert!(raw.can_bin(2) && raw.can_bin(4) && !raw.can_bin(3));
        let b = raw.develop_binned(2, 0.99).unwrap().unwrap();
        assert_eq!((b.width, b.height), (12, 9));
        let p = b.get(3, 2);
        // red sample of block (3,2) under GRBG is at (7,4); blue at (6,5); greens average (6,4)+(7,5)
        assert!((p[0] - img.get(7, 4)[0]).abs() < 1e-3, "{p:?}");
        assert!((p[2] - 0.3).abs() < 1e-3);
        assert!((p[1] - (img.get(6, 4)[1] + img.get(7, 5)[1]) / 2.0).abs() < 1e-3);
        // a single clipped green sample keeps the block's green clipped
        if let RawData::U16(d) = &mut raw.data {
            d[4 * 24 + 6] = 16383;
        }
        assert!(raw.develop_binned(2, 0.99).unwrap().unwrap().get(3, 2)[1] >= 0.99);
        // default crop is honoured at the binned scale
        raw.crop = Rect::new(2, 2, 20, 12);
        let c = raw.develop_binned(2, 0.99).unwrap().unwrap();
        assert_eq!((c.width, c.height), (10, 6));
        // X-Trans: 3×3 blocks hold all colours, 2×2 don't
        let x = raw_from(&mosaic_from_rgb(&img, &Cfa::xtrans()), 0.0, 1000.0);
        assert!(x.can_bin(3) && x.can_bin(6) && !x.can_bin(2));
        let xb = x.develop_binned(3, 0.99).unwrap().unwrap();
        assert_eq!((xb.width, xb.height), (8, 6));
        assert!((xb.get(4, 3)[2] - 0.3).abs() < 2e-3);
    }

    #[test]
    fn linear_raw_bins_to_block_means_and_keeps_clipping() {
        let (w, h) = (30usize, 20usize);
        let sample = |x: usize, y: usize, c: usize| ((x * 37 + y * 91 + c * 500) % 4000) as u16 + 100;
        let data: Vec<u16> = (0..w * h * 3).map(|i| sample(i / 3 % w, i / 3 / w, i % 3)).collect();
        let mut raw = RawImage {
            format: RawFormat::Dng,
            width: w,
            height: h,
            cpp: 3,
            data: RawData::U16(data),
            cfa: None,
            bits: 16,
            black: BlackLevel::uniform(100.0),
            white: vec![4100.0],
            active_area: Rect::new(0, 0, w, h),
            crop: Rect::new(0, 0, w, h),
            orientation: Orientation::Normal,
            color: ColorData::default(),
            wb_multipliers: None,
            linearized: false,
            opcodes: OpcodeLists::default(),
            metadata: Metadata::default(),
        };
        assert!(raw.can_bin(2) && raw.can_bin(3) && raw.can_bin(8) && !raw.can_bin(1));
        let b = raw.develop_binned(3, 2.0).unwrap().unwrap();
        assert_eq!((b.width, b.height), (10, 6));
        for (bx, by, c) in [(0, 0, 0), (4, 2, 1), (9, 5, 2)] {
            let mean: f32 = (0..9).map(|t| (sample(bx * 3 + t % 3, by * 3 + t / 3, c) as f32 - 100.0) / 4000.0).sum::<f32>() / 9.0;
            assert!((b.get(bx, by)[c] - mean).abs() < 1e-5, "({bx},{by}) channel {c}");
        }
        // a sample at the clip level keeps its block's channel clipped
        if let RawData::U16(d) = &mut raw.data {
            d[(4 * w + 7) * 3 + 1] = 4100;
        }
        assert!(raw.develop_binned(3, 0.99).unwrap().unwrap().get(2, 1)[1] >= 0.99);
        // the default crop is honoured
        raw.crop = Rect::new(3, 3, 18, 12);
        let c = raw.develop_binned(3, 2.0).unwrap().unwrap();
        assert_eq!((c.width, c.height), (6, 4));
        assert!((c.get(0, 0)[0] - (0..9).map(|t| (sample(3 + t % 3, 3 + t / 3, 0) as f32 - 100.0) / 4000.0).sum::<f32>() / 9.0).abs() < 1e-5);
    }

    #[test]
    fn binning_matches_demosaic_downscaled_on_smooth_images() {
        let img = Rgb32f::from_fn(64, 48, |x, y| [0.2 + 0.3 * (x as f32 / 64.0), 0.4 - 0.2 * (y as f32 / 48.0), 0.25]);
        let raw = raw_from(&mosaic_from_rgb(&img, &Cfa::bayer("RGGB").unwrap()), 0.0, 4095.0);
        let b = raw.develop_binned(2, 0.99).unwrap().unwrap();
        let full = lightcraft_raster::resample::half(&raw.develop(Method::Ahd).unwrap());
        for y in 2..b.height - 2 {
            for x in 2..b.width - 2 {
                for c in 0..3 {
                    assert!((b.get(x, y)[c] - full.get(x, y)[c]).abs() < 0.01, "({x},{y})");
                }
            }
        }
    }
}
