//! DNG depth maps (DNG 1.5: an IFD with NewSubFileType 8 and the `DepthFormat`, `DepthNear`,
//! `DepthFar` tags). Read separately from [`crate::decode`], like the semantic masks: only Lens
//! Blur and Depth Range masks use it. No camera-written DNG with a depth map is in the corpus (an
//! iPhone's ProRAW has none: only semantic mattes), so this follows the specification alone; the
//! map is taken to span the developed picture (the default crop), as a portrait's does its photo.

use crate::tiffraw::{Packing, read_image};
use crate::{RawData, Result};
use lightcraft_tiff::Tiff;
use lightcraft_tiff::tags as t;

/// NewSubFileType of a depth map IFD (and of a preview's depth map, with the preview bit).
pub const SUBFILE_DEPTH_MAP: u32 = 8;
pub const DEPTH_FORMAT: u16 = 51177;
pub const DEPTH_NEAR: u16 = 51178;
pub const DEPTH_FAR: u16 = 51179;
/// Most samples a depth map may have (a depth map is low resolution; this is 8192 × 4096).
pub const MAX_DEPTH_SAMPLES: usize = 1 << 25;

/// A DNG's depth map as relative disparity (larger is nearer, any scale), as stored (not oriented).
#[derive(Clone, Debug, PartialEq)]
pub struct DngDepth {
    pub width: usize,
    pub height: usize,
    pub disparity: Vec<f32>,
}

/// The depth map of a DNG (the full-size one before a preview's), or `None`.
pub fn depth_map(bytes: &[u8]) -> Option<DngDepth> {
    let tiff = Tiff::parse(bytes).ok()?;
    let ifds = tiff.all_ifds();
    let pick = |kind: u32| ifds.iter().find(|i| i.u32(t::NEW_SUBFILE_TYPE) == Some(kind));
    let ifd = pick(SUBFILE_DEPTH_MAP).or_else(|| pick(SUBFILE_DEPTH_MAP | 1))?;
    read(bytes, &tiff, ifd).ok().flatten()
}

fn read(bytes: &[u8], tiff: &Tiff, ifd: &lightcraft_tiff::Ifd) -> Result<Option<DngDepth>> {
    let info = ifd.image()?;
    let (w, h) = (info.width as usize, info.height as usize);
    if info.samples_per_pixel != 1 || w == 0 || h == 0 || w.checked_mul(h).is_none_or(|n| n > MAX_DEPTH_SAMPLES) {
        return Ok(None);
    }
    let bits = info.bits() as u32;
    // the samples as 0..1, 0 nearest
    let v: Vec<f32> = match read_image(bytes, &info, tiff.order, Packing::Msb)? {
        RawData::U16(v) if (1..=16).contains(&bits) => {
            let max = ((1u32 << bits) - 1) as f32;
            v.into_iter().map(|s| f32::from(s) / max).collect()
        }
        RawData::F32(v) => v,
        RawData::U16(_) => return Ok(None),
    };
    if v.len() != w * h {
        return Ok(None);
    }
    let format = ifd.u32(DEPTH_FORMAT).unwrap_or(0);
    let near = ifd.f64(DEPTH_NEAR).map(|v| v as f32).filter(|v| v.is_finite() && *v > 0.0);
    let far = ifd.f64(DEPTH_FAR).map(|v| v as f32).filter(|v| v.is_finite() && *v > 0.0);
    let disparity = v
        .into_iter()
        .map(|s| {
            let s = if s.is_finite() { s.clamp(0.0, 1.0) } else { 1.0 };
            match (format, near, far) {
                // linear: depth = near + s · (far − near)
                (1, Some(n), Some(f)) if f > n => 1.0 / (n + s * (f - n)),
                // inverse: linear in 1 / depth, from 1 / near (s = 0) to 1 / far (s = 1)
                (2, Some(n), Some(f)) if f > n => 1.0 / n + s * (1.0 / f - 1.0 / n),
                // no range given: only the order is known
                _ => 1.0 - s,
            }
        })
        .collect();
    Ok(Some(DngDepth { width: w, height: h, disparity }))
}

#[cfg(test)]
mod tests {
    use super::*;

    use lightcraft_tiff::{ByteOrder, IfdBuilder, ImageData, TiffWriter, Value};

    /// A TIFF whose IFD0 is a main picture with one sub-IFD: a 4 × 1 8-bit depth map `samples`.
    fn dng(samples: [u8; 4], format: Option<u16>, range: Option<(f64, f64)>) -> Vec<u8> {
        let mut depth = IfdBuilder::new()
            .with(t::NEW_SUBFILE_TYPE, Value::Long(vec![SUBFILE_DEPTH_MAP]))
            .with(256, Value::Long(vec![4]))
            .with(257, Value::Long(vec![1]))
            .with(258, Value::Short(vec![8]))
            .with(259, Value::Short(vec![1]))
            .with(262, Value::Short(vec![32803]))
            .with(277, Value::Short(vec![1]));
        if let Some(f) = format {
            depth.set(DEPTH_FORMAT, Value::Short(vec![f]));
        }
        if let Some((n, f)) = range {
            depth.set(DEPTH_NEAR, Value::Rational(vec![lightcraft_tiff::writer::rational(n)]));
            depth.set(DEPTH_FAR, Value::Rational(vec![lightcraft_tiff::writer::rational(f)]));
        }
        depth.set_image(ImageData::Strips { rows_per_strip: 1, strips: vec![samples.to_vec()] });
        let mut main = IfdBuilder::new()
            .with(t::NEW_SUBFILE_TYPE, Value::Long(vec![0]))
            .with(256, Value::Long(vec![1]))
            .with(257, Value::Long(vec![1]))
            .with(258, Value::Short(vec![8]))
            .with(259, Value::Short(vec![1]))
            .with(262, Value::Short(vec![1]))
            .with(277, Value::Short(vec![1]));
        main.set_image(ImageData::Strips { rows_per_strip: 1, strips: vec![vec![0]] });
        main.add_sub_ifd(depth);
        TiffWriter::new(ByteOrder::Little, false).write(&[main]).unwrap()
    }

    #[test]
    fn a_depth_ifd_reads_as_disparity() {
        // inverse format, 1 m to 4 m: disparity goes linearly from 1 to 0.25
        let d = depth_map(&dng([0, 85, 170, 255], Some(2), Some((1.0, 4.0)))).unwrap();
        assert_eq!((d.width, d.height), (4, 1));
        let want = [1.0, 0.75, 0.5, 0.25];
        assert!(d.disparity.iter().zip(want).all(|(a, b)| (a - b).abs() < 0.01), "{:?}", d.disparity);
        // linear format: the middle sample is 2.5 m away
        let d = depth_map(&dng([0, 128, 128, 255], Some(1), Some((1.0, 4.0)))).unwrap();
        assert!((d.disparity[1] - 1.0 / 2.506).abs() < 0.01, "{:?}", d.disparity);
        // no range: only the order, nearest first
        let d = depth_map(&dng([0, 255, 0, 255], None, None)).unwrap();
        assert_eq!(d.disparity, vec![1.0, 0.0, 1.0, 0.0]);
    }

    #[test]
    fn files_without_a_depth_map_have_none() {
        assert!(depth_map(b"").is_none());
        assert!(depth_map(b"II*\0\x08\0\0\0\0\0").is_none());
    }
}
