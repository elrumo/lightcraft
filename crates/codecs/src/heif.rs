//! HEIC / HEIF (iPhone photos) through `heic-rs` (pure Rust, MIT OR Apache-2.0, HEVC intra
//! decoder written from ITU-T H.265).
//!
//! We drive its public pieces instead of `heic_rs::decode` for one reason: `heic-rs` 0.1 ignores
//! the colour description in the HEVC stream's VUI and assumes BT.709 limited range when the file
//! has no `nclx` box. iPhones store full-range BT.601 that way (an ICC profile, no `nclx`), so
//! every highlight would clip and the contrast drop. HEIF says the VUI applies then; we read it.

use heic_rs::context::Context;
use heic_rs::hevc::Frame;
use heic_rs::{MatrixCoefficients, Nclx, PixelLayout, Range};

use crate::convert::{Buf, Meta, Model, Raw, check_size, finish};
use crate::{DecodeOptions, Decoded, Error, Format, Result};

const F: Format = Format::Heif;

pub(crate) fn decode(bytes: &[u8], opts: &DecodeOptions) -> Result<Decoded> {
    let err = |e: heic_rs::Error| Error::Malformed(F, e.to_string());
    let ctx = Context::open(bytes).map_err(err)?;
    let id = ctx.meta.primary;
    let p = ctx.props(id).map_err(err)?;
    let (cw, ch) = ctx.coded_size(id, &p).map_err(err)?;
    check_size(F, cw.into(), ch.into(), opts)?;
    // ponytail: no alpha (an `auxC` alpha item); photos and screenshots have none.
    let (frame, vui) = match ctx.grid(id).map_err(err)? {
        Some((grid, tiles)) => {
            let decoded = map_tiles(&tiles, |t| coded(&ctx, t)).map_err(err)?;
            let vui = decoded.first().and_then(|d| d.1);
            let frames: Vec<Frame> = decoded.into_iter().map(|d| d.0).collect();
            (heic_rs::grid::compose(&grid, &frames, opts.max_pixels).map_err(err)?, vui)
        }
        None => coded(&ctx, id).map_err(err)?,
    };
    let nclx = p.nclx.or(vui).unwrap_or_default();
    let wide = frame.bit_depth > 8;
    let layout = if wide { PixelLayout::Rgb16 } else { PixelLayout::Rgb8 };
    let img = heic_rs::color::convert(&frame, None, nclx, layout, opts.max_pixels, None).map_err(err)?;
    let img = heic_rs::transform::apply_all(img, &p.transforms).map_err(err)?;
    let (w, h) = (img.width, img.height);
    let buf = if wide { Buf::U16(bytemuck::pod_collect_to_vec(&img.data)) } else { Buf::U8(img.data) };
    let raw = Raw {
        width: w as usize,
        height: h as usize,
        model: Model::Rgb,
        alpha: false,
        premultiplied: false,
        buf,
        bit_depth: if wide { 16 } else { 8 },
    };
    let meta = Meta {
        icc: ctx.icc(&p).map(<[u8]>::to_vec),
        exif: ctx.exif(id).ok().flatten().map(<[u8]>::to_vec),
        // `irot` / `imir` are applied above; HEIF says the EXIF orientation is not to be used
        orientation: Some(1),
        hint: crate::png_codec::cicp_space(nclx.primaries, nclx.transfer),
        ..Default::default()
    };
    let mut d = finish(F, raw, meta, (w, h), opts)?;
    d.bit_depth = frame.bit_depth;
    Ok(d)
}

/// Decode one coded picture item, with its colour description (`nclx`, else its stream's VUI).
fn coded(ctx: &Context<'_>, item: u32) -> heic_rs::Result<(Frame, Option<Nclx>)> {
    let p = ctx.props(item)?;
    let hvcc = p.hvcc.as_ref().ok_or(heic_rs::Error::MissingBox("hvcC"))?;
    let data = ctx.item_data(item)?;
    let params = hvcc.parameter_sets();
    let frame = heic_rs::hevc::decode_still(&params, &hvcc.split_nals(&data)?)?;
    frame.validate()?;
    Ok((frame, p.nclx.or_else(|| params.iter().find_map(|nal| vui_nclx(nal)))))
}

#[cfg(feature = "parallel")]
fn map_tiles<T: Send>(tiles: &[u32], f: impl Fn(u32) -> heic_rs::Result<T> + Sync) -> heic_rs::Result<Vec<T>> {
    use rayon::prelude::*;
    tiles.par_iter().map(|t| f(*t)).collect()
}

#[cfg(not(feature = "parallel"))]
fn map_tiles<T>(tiles: &[u32], f: impl Fn(u32) -> heic_rs::Result<T>) -> heic_rs::Result<Vec<T>> {
    tiles.iter().map(|t| f(*t)).collect()
}

/// The colour description in an SPS NAL unit's VUI (H.265 7.3.2.2, E.2.1); `None` for any other
/// NAL unit, an SPS without `video_signal_type`, or one we can't walk.
fn vui_nclx(nal: &[u8]) -> Option<Nclx> {
    // NAL header: forbidden bit, nal_unit_type (6 bits; 33 = SPS), layer id, temporal id
    if (nal.first()? >> 1) & 0x3f != 33 {
        return None;
    }
    let rbsp = unescape(nal.get(2..)?);
    let r = &mut Bits { b: &rbsp, pos: 0 };
    r.skip(4)?; // sps_video_parameter_set_id
    let sub_layers = r.u(3)? as usize; // sps_max_sub_layers_minus1
    r.skip(1)?;
    // profile_tier_level(1, sps_max_sub_layers_minus1)
    r.skip(88 + 8)?;
    let mut present = [(false, false); 8];
    for p in present.iter_mut().take(sub_layers) {
        *p = (r.flag()?, r.flag()?);
    }
    if sub_layers > 0 {
        r.skip(2 * (8 - sub_layers))?;
    }
    for &(profile, level) in present.iter().take(sub_layers) {
        r.skip(if profile { 88 } else { 0 } + if level { 8 } else { 0 })?;
    }
    r.ue()?; // sps_seq_parameter_set_id
    if r.ue()? == 3 {
        r.skip(1)?; // separate_colour_plane_flag
    }
    r.ue()?;
    r.ue()?; // pic width, height
    if r.flag()? {
        (0..4).try_for_each(|_| r.ue().map(drop))?; // conformance window
    }
    r.ue()?;
    r.ue()?; // bit depths
    let poc_lsb_bits = r.ue()?.checked_add(4)?;
    let first = if r.flag()? { 0 } else { sub_layers };
    for _ in first..=sub_layers {
        (0..3).try_for_each(|_| r.ue().map(drop))?; // dec_pic_buffering, num_reorder, max_latency
    }
    (0..6).try_for_each(|_| r.ue().map(drop))?; // block sizes, transform hierarchy depths
    if r.flag()? && r.flag()? {
        scaling_list_data(r)?;
    }
    r.skip(2)?; // amp, sample_adaptive_offset
    if r.flag()? {
        r.skip(8)?; // pcm sample bit depths
        r.ue()?;
        r.ue()?;
        r.skip(1)?;
    }
    let sets = r.ue()?;
    for i in 0..sets {
        // ponytail: inter-predicted reference picture sets need the delta POCs tracked; still
        // pictures have none, so give up (the default colour description) rather than misread.
        if i != 0 && r.flag()? {
            return None;
        }
        let n = r.ue()?.checked_add(r.ue()?)?; // num_negative_pics + num_positive_pics
        (0..n).try_for_each(|_| r.ue().and_then(|_| r.skip(1)))?;
    }
    if r.flag()? {
        // long-term reference pictures
        (0..r.ue()?).try_for_each(|_| r.skip((poc_lsb_bits as usize).saturating_add(1)))?;
    }
    r.skip(2)?; // sps_temporal_mvp, strong_intra_smoothing
    if !r.flag()? {
        return None; // no VUI
    }
    if r.flag()? && r.u(8)? == 255 {
        r.skip(32)?; // extended sample aspect ratio
    }
    if r.flag()? {
        r.skip(1)?; // overscan_appropriate_flag
    }
    if !r.flag()? {
        return None; // no video_signal_type
    }
    r.skip(3)?; // video_format
    let range = if r.flag()? { Range::Full } else { Range::Limited };
    let (primaries, transfer, matrix_code) = if r.flag()? { (r.u(8)?, r.u(8)?, r.u(8)?) } else { (2, 2, 2) };
    let (primaries, transfer, matrix_code) = (primaries as u16, transfer as u16, matrix_code as u16);
    Some(Nclx { primaries, transfer, matrix: MatrixCoefficients::from_code(matrix_code), matrix_code, range })
}

fn scaling_list_data(r: &mut Bits) -> Option<()> {
    for size in 0..4u32 {
        for _ in (0..6).step_by(if size == 3 { 3 } else { 1 }) {
            if !r.flag()? {
                r.ue()?; // scaling_list_pred_matrix_id_delta
                continue;
            }
            let coefs = 64.min(1 << (4 + (size << 1))) + usize::from(size > 1); // + dc
            (0..coefs).try_for_each(|_| r.ue().map(drop))?; // se(v): same length as ue(v)
        }
    }
    Some(())
}

/// The RBSP of a NAL unit payload: drop each emulation-prevention `03` after `00 00`.
fn unescape(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(b.len());
    let mut zeros = 0;
    for &v in b {
        if zeros >= 2 && v == 3 {
            zeros = 0;
            continue;
        }
        zeros = if v == 0 { zeros + 1 } else { 0 };
        out.push(v);
    }
    out
}

/// MSB-first bit reader; `None` past the end.
struct Bits<'a> {
    b: &'a [u8],
    pos: usize,
}

impl Bits<'_> {
    fn u(&mut self, n: u32) -> Option<u32> {
        let mut v = 0u32;
        for _ in 0..n {
            let byte = *self.b.get(self.pos / 8)?;
            v = (v << 1) | u32::from((byte >> (7 - self.pos % 8)) & 1);
            self.pos += 1;
        }
        Some(v)
    }
    fn flag(&mut self) -> Option<bool> {
        Some(self.u(1)? == 1)
    }
    fn skip(&mut self, n: usize) -> Option<()> {
        self.pos = self.pos.checked_add(n)?;
        (self.pos <= self.b.len().saturating_mul(8)).then_some(())
    }
    /// ue(v) Exp-Golomb.
    fn ue(&mut self) -> Option<u32> {
        let mut zeros = 0;
        while !self.flag()? {
            zeros += 1;
            if zeros > 31 {
                return None;
            }
        }
        u32::try_from((1u64 << zeros) - 1 + u64::from(self.u(zeros)?)).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// MSB-first bit writer for building SPS fixtures.
    #[derive(Default)]
    struct W(Vec<bool>);
    impl W {
        fn u(&mut self, n: u32, v: u32) -> &mut Self {
            (0..n).rev().for_each(|i| self.0.push((v >> i) & 1 == 1));
            self
        }
        fn ue(&mut self, v: u32) -> &mut Self {
            let x = v + 1;
            let len = 32 - x.leading_zeros();
            self.u(len - 1, 0).u(len, x)
        }
        fn bytes(&self) -> Vec<u8> {
            self.0.chunks(8).map(|c| c.iter().enumerate().fold(0u8, |a, (i, b)| a | (u8::from(*b) << (7 - i)))).collect()
        }
    }

    /// An SPS like an iPhone's (512×512 tile, no reference picture sets) with the given VUI tail.
    fn sps(vui: impl Fn(&mut W)) -> Vec<u8> {
        let mut w = W::default();
        w.u(4, 0).u(3, 0).u(1, 1); // vps id, max_sub_layers_minus1, temporal_id_nesting
        w.u(32, 0x0160_0000).u(32, 0).u(24, 0).u(8, 90); // profile_tier_level, level 3
        w.ue(0).ue(1).ue(512).ue(512).u(1, 0); // sps id, 4:2:0, size, no conformance window
        w.ue(0).ue(0).ue(4).u(1, 1).ue(0).ue(0).ue(0); // bit depths, poc lsb, ordering info
        w.ue(0).ue(3).ue(0).ue(3).ue(0).ue(0); // block sizes, hierarchy depths
        w.u(1, 1).u(1, 1); // scaling lists enabled, present
        for size in 0..4 {
            for m in (0..6).step_by(if size == 3 { 3 } else { 1 }) {
                if m % 2 == 0 {
                    w.u(1, 0).ue(0); // predicted from the default
                } else {
                    w.u(1, 1);
                    let n = 64.min(1 << (4 + (size << 1))) + usize::from(size > 1);
                    (0..n).for_each(|i| _ = w.ue(i as u32 % 3)); // se(v) values 0, 1, -1
                }
            }
        }
        w.u(1, 0).u(1, 1).u(1, 0); // amp, sao, pcm
        w.ue(0).u(1, 0).u(1, 0).u(1, 1); // no st_rps, no long-term, tmvp, strong intra
        vui(&mut w);
        let mut nal = vec![33 << 1, 1];
        // escape like an encoder: 00 00 0x (x ≤ 3) gets a 03
        let mut zeros = 0;
        for b in w.bytes() {
            if zeros >= 2 && b <= 3 {
                nal.push(3);
                zeros = 0;
            }
            zeros = if b == 0 { zeros + 1 } else { 0 };
            nal.push(b);
        }
        nal
    }

    #[test]
    fn reads_the_vui_colour_description() {
        let nal = sps(|w| {
            w.u(1, 1).u(1, 1); // vui present; aspect ratio info
            w.u(8, 255).u(16, 1).u(16, 1); // extended SAR
            w.u(1, 0); // no overscan info
            w.u(1, 1).u(3, 5).u(1, 1).u(1, 1).u(8, 2).u(8, 2).u(8, 6); // full range BT.601, like an iPhone
            w.u(1, 0).u(3, 0).u(1, 0).u(1, 0).u(1, 0); // rest of the VUI
        });
        let n = vui_nclx(&nal).unwrap();
        assert_eq!((n.range, n.matrix, n.primaries, n.transfer), (Range::Full, MatrixCoefficients::Bt601, 2, 2));
    }

    #[test]
    fn no_colour_description_without_a_video_signal_type() {
        assert!(vui_nclx(&sps(|w| _ = w.u(1, 0))).is_none());
        assert!(vui_nclx(&sps(|w| _ = w.u(1, 1).u(1, 0).u(1, 0).u(1, 0))).is_none());
        let limited = vui_nclx(&sps(|w| _ = w.u(1, 1).u(1, 0).u(1, 0).u(1, 1).u(3, 5).u(1, 0).u(1, 0))).unwrap();
        assert_eq!((limited.range, limited.matrix_code), (Range::Limited, 2));
    }

    #[test]
    fn hostile_parameter_sets_are_none() {
        assert!(vui_nclx(&[]).is_none());
        assert!(vui_nclx(&[33 << 1]).is_none());
        assert!(vui_nclx(&[34 << 1, 1, 0xff]).is_none()); // a PPS
        let nal = sps(|w| _ = w.u(1, 1).u(1, 0).u(1, 0).u(1, 1).u(3, 5).u(1, 1).u(1, 0));
        for n in 0..nal.len() {
            let _ = vui_nclx(&nal[..n]);
        }
        assert!(vui_nclx(&[33 << 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]).is_none());
    }

    #[test]
    fn malformed_files_are_errors() {
        let mut ftyp = b"\0\0\0\x18ftypheic\0\0\0\0mif1heic".to_vec();
        assert!(decode(&ftyp, &DecodeOptions::default()).is_err());
        ftyp.extend_from_slice(b"\0\0\0\x10meta\0\0\0\0\0\0\0\0");
        assert!(decode(&ftyp, &DecodeOptions::default()).is_err());
    }
}
