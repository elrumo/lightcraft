//! Reading the pixel data of a TIFF IFD (strips or tiles; uncompressed, lossless JPEG, lossy JPEG, Deflate, JPEG XL),
//! in parallel.

use crate::unpack::*;
use crate::{MAX_SAMPLES, RawData, RawError, Result, jxl, ljpeg};
use lightcraft_tiff::image::{Chunk, ImageInfo, chunk_bytes};
use lightcraft_tiff::{ByteOrder, tags::compression as comp};
use rayon::prelude::*;

/// Bit packing for uncompressed integer data with bit depths other than 8/16.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // vendor decoders use the other variants
pub enum Packing {
    /// TIFF standard: MSB-first, rows start on byte boundaries.
    Msb,
    /// LSB-first little-endian bit stream (some vendor raws), rows byte aligned.
    Lsb,
    /// Each sample stored in 16 bits in file byte order, regardless of `bits`.
    Word16,
}

enum ChunkPx {
    U16(Vec<u16>),
    F32(Vec<f32>),
}

/// The checks [`read_image`] makes before decoding anything (sample format, size limits, data
/// present); returns the number of samples.
pub fn check_image(data: &[u8], info: &ImageInfo) -> Result<usize> {
    let (w, h) = (info.width as usize, info.height as usize);
    let cpp = info.samples_per_pixel as usize;
    let total = w.checked_mul(h).and_then(|v| v.checked_mul(cpp)).ok_or(RawError::Limit("image too large"))?;
    if total > MAX_SAMPLES || total == 0 {
        return Err(RawError::Limit("image too large"));
    }
    let bits = info.bits() as u32;
    let float = info.sample_format == 3;
    if float && !matches!(bits, 16 | 24 | 32) {
        return Err(RawError::Unsupported(format!("{bits}-bit floating point samples")));
    }
    if !float && !(1..=16).contains(&bits) {
        return Err(RawError::Unsupported(format!("{bits}-bit integer samples")));
    }
    let chunks = info.chunks(data.len() as u64);
    if chunks.is_empty() {
        return Err(RawError::Corrupt("no image data chunks".into()));
    }
    // Plausibility bound before allocating: no supported coding but JPEG XL stores more than ~2000 samples per
    // byte (Deflate of constant data is the extreme; JPEG XL codes a constant tile in a few bytes), so tiny files
    // cannot trigger huge allocations.
    let ratio = if info.compression == comp::JPEG_XL { 1 << 16 } else { 2048 };
    let available: u64 = chunks.iter().map(|c| chunk_bytes(data, c).map_or(0, |s| s.len() as u64)).sum();
    if available.saturating_mul(ratio) < total as u64 {
        return Err(RawError::Corrupt(format!("{available} bytes of image data cannot hold {total} samples")));
    }
    Ok(total)
}

/// Read the samples of `info` in `mode`: [`Mode::Header`] only checks them ([`check_image`]) and
/// returns no samples (floating-point data as an empty `F32`). `white` is the DNG `WhiteLevel`, which decides how
/// JPEG XL tiles are scaled to integers.
pub(crate) fn read_image_in(
    mode: crate::Mode,
    data: &[u8],
    info: &ImageInfo,
    order: ByteOrder,
    packing: Packing,
    white: Option<f32>,
) -> Result<RawData> {
    match mode {
        crate::Mode::Full => read_image_with(data, info, order, packing, white),
        crate::Mode::Header => {
            check_image(data, info)?;
            // a JPEG XL tile the decoder rejects (wrong channel count, bigger than its chunk) must fail here too
            if info.compression == comp::JPEG_XL
                && let Some(c) = info.chunks(data.len() as u64).first()
                && let Some(src) = chunk_bytes(data, c)
            {
                let cpp = if info.planar == 2 { 1 } else { info.samples_per_pixel as usize };
                let (cw, ch) = (c.width as usize, c.height as usize);
                let h = jxl::header(src, cw.saturating_mul(ch).saturating_mul(cpp))?;
                if h.channels != cpp || h.width > cw || h.height > ch {
                    return Err(RawError::Corrupt(format!("JPEG XL tile of {}×{}×{} in a {cw}×{ch}×{cpp} chunk", h.width, h.height, h.channels)));
                }
            }
            // a lossless-JPEG layout the decoder rejects (subsampled components, e.g. Sony's
            // lossless M/S sizes) must fail here too, as the full decode will
            if info.compression == 7
                && let Some(src) = info.chunks(data.len() as u64).first().and_then(|c| chunk_bytes(data, c))
            {
                ljpeg::frame_info(src)?;
            }
            Ok(if info.sample_format == 3 { RawData::F32(Vec::new()) } else { RawData::U16(Vec::new()) })
        }
    }
}

/// Decode all chunks of `info` into one buffer of `width × height × cpp` samples.
pub fn read_image(data: &[u8], info: &ImageInfo, order: ByteOrder, packing: Packing) -> Result<RawData> {
    read_image_with(data, info, order, packing, None)
}

/// [`read_image`] with the DNG `WhiteLevel`, which decides how JPEG XL tiles are scaled to integers.
fn read_image_with(data: &[u8], info: &ImageInfo, order: ByteOrder, packing: Packing, white: Option<f32>) -> Result<RawData> {
    let (w, h) = (info.width as usize, info.height as usize);
    let total = check_image(data, info)?;
    let cpp = info.samples_per_pixel as usize;
    let float = info.sample_format == 3;
    let planar = info.planar == 2 && cpp > 1;
    // Each chunk is copied into the image as soon as it is decoded, so only the chunks being decoded are alive
    // next to the result (collecting them all first would hold a second copy of the whole image).
    let out = std::sync::Mutex::new(Placed {
        u16: if float { Vec::new() } else { vec![0u16; total] },
        f32: if float { vec![0f32; total] } else { Vec::new() },
    });
    visit_chunks(data, info, order, packing, white, |c, px| {
        let mut o = out.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let Placed { u16, f32 } = &mut *o;
        match &px {
            ChunkPx::U16(v) => place(u16, v, c, (w, h), cpp, planar),
            ChunkPx::F32(v) => place(f32, v, c, (w, h), cpp, planar),
        }
    })?;
    let o = out.into_inner().unwrap_or_else(std::sync::PoisonError::into_inner);
    Ok(if float { RawData::F32(o.f32) } else { RawData::U16(o.u16) })
}

/// Decode the chunks of `info` in parallel and hand each to `sink` as soon as it is decoded (on the worker
/// thread, so the sink locks only what it must). Fails with the error of the first chunk that failed when none
/// decoded; chunks that fail among others are skipped, as a damaged tile shouldn't lose the photo.
fn visit_chunks(
    data: &[u8],
    info: &ImageInfo,
    order: ByteOrder,
    packing: Packing,
    white: Option<f32>,
    sink: impl Fn(&Chunk, ChunkPx) + Sync,
) -> Result<()> {
    let cpp = info.samples_per_pixel as usize;
    let planar = info.planar == 2 && cpp > 1;
    let ccpp = if planar { 1 } else { cpp };
    let (bits, float) = (info.bits() as u32, info.sample_format == 3);
    // (chunks decoded, the error of the first chunk that failed, in chunk order)
    let status = std::sync::Mutex::new((0usize, None::<(usize, RawError)>));
    info.chunks(data.len() as u64).par_iter().for_each(|c| {
        let r = decode_chunk(data, info, c, order, packing, ccpp, bits, float, white);
        let ok = r.is_ok();
        match r {
            Ok(px) => sink(c, px),
            Err(e) => {
                let mut st = status.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                if st.1.as_ref().is_none_or(|(i, _)| c.index < *i) {
                    st.1 = Some((c.index, e));
                }
            }
        }
        if ok {
            status.lock().unwrap_or_else(std::sync::PoisonError::into_inner).0 += 1;
        }
    });
    let (ok, err) = status.into_inner().unwrap_or_else(std::sync::PoisonError::into_inner);
    if ok == 0 {
        return Err(err.map(|(_, e)| e).unwrap_or_else(|| RawError::Corrupt("no decodable chunks".into())));
    }
    Ok(())
}

/// The image being assembled from its chunks.
struct Placed {
    u16: Vec<u16>,
    f32: Vec<f32>,
}

/// What a binned read ([`read_binned`]) reduces the image to: the `bw × bh` blocks of `k × k` samples whose
/// top-left block starts at `origin`.
pub(crate) struct BinSpec<'a> {
    pub k: usize,
    pub origin: (usize, usize),
    pub size: (usize, usize),
    /// The DNG linearization table, applied to every sample before it is averaged (it is not linear).
    pub table: Option<&'a [u16]>,
    /// Per channel, the sample value from which a block keeps its maximum instead of its mean (so highlight
    /// reconstruction still sees clipped channels).
    pub clip_at: [f32; 3],
}

/// Read a three-samples-per-pixel image already reduced to `spec`'s blocks: each block is the mean of its samples,
/// or their maximum where one reaches the clip level. The full-size samples are never held: each chunk is added into
/// the (much smaller) block sums as soon as it is decoded.
pub(crate) fn read_binned(data: &[u8], info: &ImageInfo, order: ByteOrder, white: Option<f32>, spec: &BinSpec) -> Result<RawData> {
    check_image(data, info)?;
    let (k, (bw, bh)) = (spec.k, spec.size);
    let n = bw.checked_mul(bh).and_then(|v| v.checked_mul(3)).ok_or(RawError::Limit("image too large"))?;
    // sums of up to k² 16-bit samples fit 32 bits for k ≤ 255
    let acc = std::sync::Mutex::new((vec![0u32; n], vec![0u16; n]));
    visit_chunks(data, info, order, Packing::Msb, white, |c, px| {
        let ChunkPx::U16(v) = px else { return };
        let (cw, ch) = (c.width as usize, c.height as usize);
        let (cx, cy) = (c.x as usize, c.y as usize);
        let (ox, oy) = spec.origin;
        // the part of the chunk inside the binned region, and the blocks it touches
        let xs = cx.max(ox)..(cx + cw).min(ox + bw * k);
        let ys = cy.max(oy)..(cy + ch).min(oy + bh * k);
        if xs.is_empty() || ys.is_empty() {
            return;
        }
        let (bx0, by0) = ((xs.start - ox) / k, (ys.start - oy) / k);
        let (lw, lh) = ((xs.end - 1 - ox) / k + 1 - bx0, (ys.end - 1 - oy) / k + 1 - by0);
        let (mut sum, mut max) = (vec![0u32; lw * lh * 3], vec![0u16; lw * lh * 3]);
        let last = spec.table.map_or(0, |t| t.len().saturating_sub(1));
        for y in ys.clone() {
            let row = ((y - oy) / k - by0) * lw;
            for x in xs.clone() {
                let l = (row + (x - ox) / k - bx0) * 3;
                let Some(px) = v.get(((y - cy) * cw + (x - cx)) * 3..).and_then(|s| s.get(..3)) else { continue };
                for (ch, &raw) in px.iter().enumerate() {
                    let s = spec.table.and_then(|t| t.get((raw as usize).min(last)).copied()).unwrap_or(raw);
                    sum[l + ch] += s as u32;
                    max[l + ch] = max[l + ch].max(s);
                }
            }
        }
        let mut g = acc.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let (all_sum, all_max) = &mut *g;
        for by in 0..lh {
            for bx in 0..lw {
                let (l, o) = ((by * lw + bx) * 3, ((by0 + by) * bw + bx0 + bx) * 3);
                for ch in 0..3 {
                    if let (Some(s), Some(m)) = (all_sum.get_mut(o + ch), all_max.get_mut(o + ch)) {
                        *s += sum[l + ch];
                        *m = (*m).max(max[l + ch]);
                    }
                }
            }
        }
    })?;
    let (sum, max) = acc.into_inner().unwrap_or_else(std::sync::PoisonError::into_inner);
    let area = (k * k) as u32;
    let out = sum
        .iter()
        .zip(&max)
        .enumerate()
        .map(|(i, (&s, &m))| if m as f32 >= spec.clip_at[i % 3] { m } else { ((s + area / 2) / area) as u16 })
        .collect();
    Ok(RawData::U16(out))
}

/// Copy the decoded samples `src` of chunk `c` into the `w × h × cpp` image `dst` (clipped to the image; a planar
/// chunk holds one sample per pixel, for plane `c.plane`).
fn place<T: Copy>(dst: &mut [T], src: &[T], c: &Chunk, (w, h): (usize, usize), cpp: usize, planar: bool) {
    let (cw, ch) = (c.width as usize, c.height as usize);
    let (x0, y0) = (c.x as usize, c.y as usize);
    if x0 >= w || y0 >= h {
        return;
    }
    let (copy_w, copy_h) = (cw.min(w - x0), ch.min(h - y0));
    if planar {
        let plane = c.plane as usize;
        for y in 0..copy_h {
            for x in 0..copy_w {
                if let (Some(o), Some(&s)) = (dst.get_mut(((y0 + y) * w + x0 + x) * cpp + plane), src.get(y * cw + x)) {
                    *o = s;
                }
            }
        }
        return;
    }
    for y in 0..copy_h {
        let n = copy_w * cpp;
        let (s0, d0) = (y * cw * cpp, ((y0 + y) * w + x0) * cpp);
        if let (Some(o), Some(s)) = (dst.get_mut(d0..d0 + n), src.get(s0..s0 + n)) {
            o.copy_from_slice(s);
        } else if let Some(o) = dst.get_mut(d0..d0 + n) {
            // a chunk shorter than its nominal size: copy what it has
            let have = src.get(s0..).unwrap_or(&[]);
            let m = have.len().min(n);
            o.iter_mut().zip(have).take(m).for_each(|(o, &s)| *o = s);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn decode_chunk(
    data: &[u8],
    info: &ImageInfo,
    c: &Chunk,
    order: ByteOrder,
    packing: Packing,
    cpp: usize,
    bits: u32,
    float: bool,
    white: Option<f32>,
) -> Result<ChunkPx> {
    let src = chunk_bytes(data, c).ok_or_else(|| RawError::Corrupt("chunk offset past end of file".into()))?;
    let (cw, ch) = (c.width as usize, c.height as usize);
    let n = cw.checked_mul(ch).and_then(|v| v.checked_mul(cpp)).ok_or(RawError::Limit("chunk too large"))?;
    if n > MAX_SAMPLES {
        return Err(RawError::Limit("chunk too large"));
    }
    match info.compression {
        comp::NONE => unpack_chunk(src, order, packing, cw, ch, cpp, bits, float, 1),
        comp::JPEG => {
            let f = ljpeg::decode(src, n.saturating_mul(2).max(1 << 16))?;
            if f.data.len() < n {
                // Some writers encode edge tiles smaller than nominal: accept a frame that covers the rows it has.
                if f.data.is_empty() || f.data.len() % (cw * cpp) != 0 {
                    return Err(RawError::Corrupt(format!("lossless JPEG tile has {} samples, expected {n}", f.data.len())));
                }
            }
            let mut v = f.data;
            v.resize(n, 0);
            Ok(ChunkPx::U16(v))
        }
        comp::LOSSY_JPEG => {
            // lossy DNG: each chunk is a baseline (DCT) JPEG of 8-bit samples
            let (px, jw, jh, jc) = lossy_jpeg(src, cpp)?;
            let mut v = vec![0u16; n];
            for y in 0..ch.min(jh) {
                for x in 0..cw.min(jw) {
                    for k in 0..cpp {
                        v[(y * cw + x) * cpp + k] = px[(y * jw + x) * jc + k.min(jc - 1)] as u16;
                    }
                }
            }
            Ok(ChunkPx::U16(v))
        }
        comp::ADOBE_DEFLATE | comp::DEFLATE => {
            let bytes_per = if float { bits.div_ceil(8) as usize } else { 0 };
            let row_bytes = if float { cw * cpp * bytes_per } else { (cw * cpp * bits as usize).div_ceil(8) };
            let raw = inflate(src, row_bytes * ch)?;
            unpack_chunk(&raw, order, packing, cw, ch, cpp, bits, float, info.predictor)
        }
        comp::JPEG_XL => {
            let tile = jxl::decode(src, cw, ch, cpp)?;
            Ok(if float {
                ChunkPx::F32(tile.placed(cw, ch, cpp, |f| f))
            } else {
                let scale = tile.int_scale(white);
                ChunkPx::U16(tile.placed(cw, ch, cpp, |f| (f * scale).round().clamp(0.0, 65535.0) as u16))
            })
        }
        other => Err(RawError::Unsupported(format!("TIFF compression {other}"))),
    }
}

/// Decode one baseline JPEG chunk to 8-bit samples: (samples, width, height, channels).
fn lossy_jpeg(src: &[u8], cpp: usize) -> Result<(Vec<u8>, usize, usize, usize)> {
    use zune_core::bytestream::ZCursor;
    use zune_core::colorspace::ColorSpace;
    use zune_core::options::DecoderOptions;
    let cs = if cpp == 1 { ColorSpace::Luma } else { ColorSpace::RGB };
    let opts = DecoderOptions::default().set_max_width(1 << 16).set_max_height(1 << 16).set_strict_mode(false).jpeg_set_out_colorspace(cs);
    let mut d = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(src), opts);
    let px = d.decode().map_err(|e| RawError::Corrupt(format!("lossy JPEG tile: {e}")))?;
    let info = d.info().ok_or_else(|| RawError::Corrupt("lossy JPEG tile without a header".into()))?;
    let (w, h) = (info.width as usize, info.height as usize);
    let c = if cpp == 1 { 1 } else { 3 };
    if w == 0 || h == 0 || px.len() < w * h * c {
        return Err(RawError::Corrupt("lossy JPEG tile: short output".into()));
    }
    Ok((px, w, h, c))
}

#[allow(clippy::too_many_arguments)]
fn unpack_chunk(
    src: &[u8],
    order: ByteOrder,
    packing: Packing,
    cw: usize,
    ch: usize,
    cpp: usize,
    bits: u32,
    float: bool,
    predictor: u16,
) -> Result<ChunkPx> {
    let row_n = cw * cpp;
    let (factor, fp) = match predictor {
        1 => (0, false),
        2 => (1, false),
        3 => (1, true),
        34892 => (2, false),
        34893 => (4, false),
        34894 => (2, true),
        34895 => (4, true),
        p => return Err(RawError::Unsupported(format!("predictor {p}"))),
    };
    if float {
        let bp = bits.div_ceil(8) as usize;
        let row_bytes = row_n * bp;
        let mut out = vec![0f32; row_n * ch];
        let mut rowbuf = vec![0u8; row_bytes];
        for y in 0..ch {
            let s = src.get(y * row_bytes..).unwrap_or(&[]);
            let len = s.len().min(row_bytes);
            rowbuf[..len].copy_from_slice(&s[..len]);
            rowbuf[len..].fill(0);
            let big = if fp {
                undo_float_predictor(&mut rowbuf, row_n, bp, cpp * factor);
                true
            } else {
                if factor > 0 {
                    return Err(RawError::Unsupported("integer predictor on float data".into()));
                }
                order == ByteOrder::Big
            };
            for (i, o) in out[y * row_n..(y + 1) * row_n].iter_mut().enumerate() {
                let b = &rowbuf[i * bp..(i + 1) * bp];
                *o = match (bp, big) {
                    (2, true) => f16_to_f32(u16::from_be_bytes([b[0], b[1]])),
                    (2, false) => f16_to_f32(u16::from_le_bytes([b[0], b[1]])),
                    (3, true) => f24_to_f32(u32::from_be_bytes([0, b[0], b[1], b[2]])),
                    (3, false) => f24_to_f32(u32::from_le_bytes([b[0], b[1], b[2], 0])),
                    (4, true) => f32::from_be_bytes([b[0], b[1], b[2], b[3]]),
                    _ => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
                };
            }
        }
        return Ok(ChunkPx::F32(out));
    }
    if fp {
        return Err(RawError::Unsupported("floating-point predictor on integer data".into()));
    }
    let mut out = vec![0u16; row_n * ch];
    let (row_bytes, kind) = match (bits, packing) {
        (8, _) => (row_n, 0),
        (16, _) | (_, Packing::Word16) => (row_n * 2, 1),
        (_, Packing::Msb) => ((row_n * bits as usize).div_ceil(8), 2),
        (_, Packing::Lsb) => ((row_n * bits as usize).div_ceil(8), 3),
    };
    for y in 0..ch {
        let s = src.get(y * row_bytes..).unwrap_or(&[]);
        let s = &s[..s.len().min(row_bytes)];
        let row = &mut out[y * row_n..(y + 1) * row_n];
        match kind {
            0 => row.iter_mut().zip(s).for_each(|(o, &b)| *o = b as u16),
            1 => read_u16s(s, order, row),
            2 => unpack_msb(s, bits, row),
            _ => unpack_lsb(s, bits, row),
        }
        if factor > 0 {
            if bits == 8 {
                for i in cpp * factor..row.len() {
                    row[i] = (row[i] as u8).wrapping_add(row[i - cpp * factor] as u8) as u16;
                }
            } else {
                undo_diff_u16(row, cpp * factor);
            }
        }
    }
    Ok(ChunkPx::U16(out))
}

#[cfg(test)]
mod lossy_tests {
    use super::*;
    use lightcraft_tiff::image::Layout;

    /// A lossy-DNG-style image: two 16×8 tiles, each a baseline JPEG, the right one cut short
    /// by the image edge.
    #[test]
    fn lossy_jpeg_tiles_decode() {
        let (tw, th) = (16usize, 8usize);
        let tile = |shade: u8| {
            let px: Vec<u8> = (0..tw * th).flat_map(|i| [shade, (i % tw * 15) as u8, 200]).collect();
            let mut out = Vec::new();
            jpeg_encoder::Encoder::new(&mut out, 100).encode(&px, tw as u16, th as u16, jpeg_encoder::ColorType::Rgb).unwrap();
            out
        };
        let (a, b) = (tile(40), tile(220));
        let mut file = vec![0u8; 8];
        let oa = file.len() as u64;
        file.extend_from_slice(&a);
        let ob = file.len() as u64;
        file.extend_from_slice(&b);
        let info = ImageInfo {
            width: 24,
            height: 8,
            bits_per_sample: vec![8, 8, 8],
            samples_per_pixel: 3,
            compression: comp::LOSSY_JPEG,
            photometric: 34892,
            planar: 1,
            predictor: 1,
            sample_format: 1,
            new_subfile_type: 0,
            layout: Layout::Tiles { tile_width: tw as u32, tile_height: th as u32 },
            offsets: vec![oa, ob],
            byte_counts: vec![a.len() as u64, b.len() as u64],
        };
        let RawData::U16(v) = read_image(&file, &info, ByteOrder::Little, Packing::Msb).unwrap() else { panic!("integer samples") };
        assert_eq!(v.len(), 24 * 8 * 3);
        let px = |x: usize, y: usize| &v[(y * 24 + x) * 3..(y * 24 + x) * 3 + 3];
        assert!((px(2, 3)[0] as i32 - 40).abs() <= 3 && (px(2, 3)[2] as i32 - 200).abs() <= 3, "{:?}", px(2, 3));
        assert!((px(20, 3)[0] as i32 - 220).abs() <= 3, "second tile: {:?}", px(20, 3));
        assert!((px(10, 5)[1] as i32 - 150).abs() <= 6, "green ramp: {:?}", px(10, 5));
    }
}
