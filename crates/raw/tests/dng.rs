//! Synthetic DNG round trips: every storage variant we decode is written with our own TIFF writer (and LJ92
//! encoder) and must decode bit-exactly.

mod jxl_data;

use lightcraft_color::{D65, Mat3, Xy};
use lightcraft_raw::opcodes::{Area, Opcode};
use lightcraft_raw::{
    BlackLevel, Cfa, ColorData, DngCompression, DngWriteOptions, Method, OpcodeLists, Orientation, RawData, RawFormat, RawImage, Rect, decode,
    embedded_preview, probe, probe_info, write_dng,
};
use lightcraft_tiff::tags::{self as t, compression, photometric};
use lightcraft_tiff::{ByteOrder, IfdBuilder, ImageData, TiffWriter, Value};

fn scene_value(x: usize, y: usize, c: u8, max: f32) -> f32 {
    let base = [0.55, 0.8, 0.4][c as usize];
    let v = base * (0.2 + 0.7 * ((x as f32 * 0.13).sin() * 0.5 + 0.5) * ((y as f32 * 0.07).cos() * 0.3 + 0.7));
    v * max
}

fn synthetic(w: usize, h: usize, cfa: Option<Cfa>, cpp: usize) -> RawImage {
    let black = 256.0;
    let white = 16000.0;
    let mut data = Vec::with_capacity(w * h * cpp);
    for y in 0..h {
        for x in 0..w {
            for s in 0..cpp {
                let c = match &cfa {
                    Some(p) => p.color_at(x, y),
                    None => s as u8,
                };
                data.push((black + scene_value(x, y, c, white - black)).round() as u16);
            }
        }
    }
    let color = ColorData {
        illuminant: [17, 21],
        color_matrix: [
            Some(Mat3([[0.9, 0.2, -0.15], [-0.3, 1.25, 0.08], [0.02, -0.12, 0.85]])),
            Some(Mat3([[0.7, 0.3, -0.1], [-0.35, 1.3, 0.1], [0.05, -0.2, 1.0]])),
        ],
        as_shot_neutral: Some([0.5, 1.0, 0.7]),
        baseline_exposure: 0.35,
        ..Default::default()
    };
    let metadata = lightcraft_meta::Metadata { make: Some("Synth".into()), model: Some("Cam 1".into()), rating: Some(3), ..Default::default() };
    RawImage {
        format: RawFormat::Dng,
        width: w,
        height: h,
        cpp,
        data: RawData::U16(data),
        cfa,
        bits: 14,
        black: BlackLevel::uniform(black),
        white: vec![white],
        active_area: Rect::new(2, 1, w - 4, h - 2),
        crop: Rect::new(3, 2, w - 12, h - 8),
        orientation: Orientation::Rotate90,
        color,
        wb_multipliers: None,
        linearized: false,
        opcodes: OpcodeLists::default(),
        metadata,
    }
}

fn assert_same(a: &RawImage, b: &RawImage) {
    assert_eq!((a.width, a.height, a.cpp), (b.width, b.height, b.cpp));
    assert_eq!(a.data, b.data);
    assert_eq!(a.cfa, b.cfa);
    assert_eq!(a.active_area, b.active_area);
    assert_eq!(a.crop, b.crop);
    assert_eq!(a.orientation, b.orientation);
    assert!((0..a.cpp).all(|s| a.white_at(s) == b.white_at(s)));
    assert_eq!(a.black.values, b.black.values.iter().take(a.black.values.len()).copied().collect::<Vec<_>>());
    for i in 0..2 {
        let (x, y) = (a.color.color_matrix[i].unwrap(), b.color.color_matrix[i].unwrap());
        for r in 0..3 {
            for c in 0..3 {
                assert!((x.0[r][c] - y.0[r][c]).abs() < 1e-6);
            }
        }
    }
    assert_eq!(a.color.illuminant, b.color.illuminant);
    let (n1, n2) = (a.color.as_shot_neutral.unwrap(), b.color.as_shot_neutral.unwrap());
    assert!((0..3).all(|i| (n1[i] - n2[i]).abs() < 1e-6));
    assert!((a.color.baseline_exposure - b.color.baseline_exposure).abs() < 1e-6);
    assert_eq!(b.metadata.make.as_deref(), Some("Synth"));
    assert_eq!(b.metadata.rating, Some(3));
}

#[test]
fn writer_roundtrip_all_layouts() {
    for (w, h) in [(64, 48), (70, 45), (33, 17)] {
        for cfa in [Some(Cfa::bayer("RGGB").unwrap()), Some(Cfa::bayer("GBRG").unwrap()), Some(Cfa::xtrans()), None] {
            let cpp = if cfa.is_some() { 1 } else { 3 };
            let raw = synthetic(w, h, cfa, cpp);
            for comp in [DngCompression::Uncompressed, DngCompression::Lj92 { tile: 16 }, DngCompression::Lj92 { tile: 256 }] {
                for order in [ByteOrder::Little, ByteOrder::Big] {
                    let bytes = write_dng(&raw, &DngWriteOptions { compression: comp, order, xmp: None }).unwrap();
                    assert_eq!(probe(&bytes), Some(RawFormat::Dng));
                    let back = decode(&bytes).unwrap();
                    assert_same(&raw, &back);
                    // the header-only probe describes the same image
                    assert_eq!(lightcraft_raw::probe_info(&bytes).unwrap(), back.info());
                }
            }
        }
    }
}

#[test]
fn deflate_roundtrips_integer_and_float() {
    for (w, h) in [(64, 48), (70, 45)] {
        for cfa in [Some(Cfa::bayer("RGGB").unwrap()), None] {
            let cpp = if cfa.is_some() { 1 } else { 3 };
            let raw = synthetic(w, h, cfa, cpp);
            for order in [ByteOrder::Little, ByteOrder::Big] {
                let bytes =
                    write_dng(&raw, &DngWriteOptions { compression: DngCompression::Deflate { tile: 32, half: false }, order, xmp: None }).unwrap();
                assert_same(&raw, &decode(&bytes).unwrap());
            }
        }
    }
    // linear float data: 32-bit exact, 16-bit within half precision
    let mut raw = synthetic(70, 45, None, 3);
    let vals: Vec<f32> = (0..70 * 45 * 3).map(|i| ((i % 97) as f32 / 97.0).powi(3) * 0.9 + 1e-4).collect();
    raw.data = RawData::F32(vals.clone());
    raw.black = BlackLevel::uniform(0.0);
    raw.white = vec![1.0];
    for half in [false, true] {
        let bytes = write_dng(&raw, &DngWriteOptions { compression: DngCompression::Deflate { tile: 32, half }, ..Default::default() }).unwrap();
        let back = decode(&bytes).unwrap();
        let RawData::F32(b) = &back.data else { panic!("float data expected") };
        for (a, b) in vals.iter().zip(b) {
            let tol = if half { a * 1e-3 + 1e-7 } else { 0.0 };
            assert!((a - b).abs() <= tol, "{a} vs {b} (half: {half})");
        }
    }
    assert_eq!(lightcraft_raw::dngwrite::f32_to_f16(1.0), 0x3c00);
    assert_eq!(lightcraft_raw::dngwrite::f32_to_f16(-2.0), 0xc000);
    assert_eq!(lightcraft_raw::dngwrite::f32_to_f16(1e9), 0x7c00);
    assert_eq!(lightcraft_raw::dngwrite::f32_to_f16(2f32.powi(-24)), 0x0001);
}

#[test]
fn normalized_and_develop() {
    let mut raw = synthetic(40, 30, Some(Cfa::bayer("RGGB").unwrap()), 1);
    let n = raw.normalized().unwrap();
    assert_eq!((n.width, n.height), (36, 28));
    // the CFA is re-anchored at the active area origin (2, 1): RGGB shifted by (2,1) → GBRG
    assert_eq!(n.cfa.as_ref().unwrap().name(), "GBRG");
    let RawData::U16(d) = &raw.data else { unreachable!() };
    let expect = (d[40 + 2] as f32 - 256.0) / (16000.0 - 256.0);
    assert!((n.data[0] - expect).abs() < 1e-6);
    let rgb = raw.develop(Method::Ahd).unwrap();
    assert_eq!((rgb.width, rgb.height), (28, 22));
    // constant scene → constant output, exactly 0.5 after normalisation
    if let RawData::U16(d) = &mut raw.data {
        d.iter_mut().for_each(|v| *v = 256 + (16000 - 256) / 2);
    }
    for m in [Method::Bilinear, Method::Ppg, Method::Ahd] {
        let rgb = raw.develop(m).unwrap();
        assert!(rgb.data.iter().all(|p| p.iter().all(|v| (v - 0.5).abs() < 1e-4)), "{m:?}");
    }
}

// ---------------------------------------------------------------- hand-built variants

fn base_ifd(w: usize, h: usize, bits: u16, cpp: usize, cfa: bool) -> IfdBuilder {
    let mut ifd = IfdBuilder::new();
    ifd.set(t::NEW_SUBFILE_TYPE, Value::Long(vec![0]));
    ifd.set(t::IMAGE_WIDTH, Value::Long(vec![w as u32]));
    ifd.set(t::IMAGE_LENGTH, Value::Long(vec![h as u32]));
    ifd.set(t::BITS_PER_SAMPLE, Value::Short(vec![bits; cpp]));
    ifd.set(t::SAMPLES_PER_PIXEL, Value::Short(vec![cpp as u16]));
    ifd.set(t::PHOTOMETRIC, Value::Short(vec![if cfa { photometric::CFA } else { photometric::LINEAR_RAW }]));
    if cfa {
        ifd.set(t::CFA_REPEAT_PATTERN_DIM, Value::Short(vec![2, 2]));
        ifd.set(t::CFA_PATTERN_EP, Value::Byte(vec![1, 0, 2, 1]));
    }
    ifd
}

fn dng(raw: IfdBuilder, order: ByteOrder) -> Vec<u8> {
    dng_with(raw, order, |_| {})
}

/// [`dng`] with extra IFD 0 tags (where DNG writers put camera-profile tags).
fn dng_with(raw: IfdBuilder, order: ByteOrder, extra: impl FnOnce(&mut IfdBuilder)) -> Vec<u8> {
    let mut ifd0 = IfdBuilder::new();
    ifd0.set(t::NEW_SUBFILE_TYPE, Value::Long(vec![1]));
    ifd0.set(t::DNG_VERSION, Value::Byte(vec![1, 6, 0, 0]));
    ifd0.set(t::MAKE, Value::Ascii("Hand".into()));
    ifd0.set(t::COLOR_MATRIX_1, Value::SRational(vec![(1, 1), (0, 1), (0, 1), (0, 1), (1, 1), (0, 1), (0, 1), (0, 1), (1, 1)]));
    ifd0.set(t::CALIBRATION_ILLUMINANT_1, Value::Short(vec![21]));
    ifd0.set(t::AS_SHOT_WHITE_XY, Value::Rational(vec![(3127, 10000), (3290, 10000)]));
    // an 8-bit RGB thumbnail in IFD0 (must not be mistaken for the raw)
    ifd0.set(t::IMAGE_WIDTH, Value::Long(vec![4]));
    ifd0.set(t::IMAGE_LENGTH, Value::Long(vec![2]));
    ifd0.set(t::BITS_PER_SAMPLE, Value::Short(vec![8, 8, 8]));
    ifd0.set(t::SAMPLES_PER_PIXEL, Value::Short(vec![3]));
    ifd0.set(t::PHOTOMETRIC, Value::Short(vec![photometric::RGB]));
    ifd0.set_image(ImageData::Strips { rows_per_strip: 2, strips: vec![vec![128; 24]] });
    let mut exif = IfdBuilder::new();
    exif.set(t::ISO_SPEED, Value::Short(vec![200]));
    ifd0.set_child(t::EXIF_IFD, exif);
    ifd0.add_sub_ifd(raw);
    extra(&mut ifd0);
    TiffWriter::new(order, false).write(&[ifd0]).unwrap()
}

fn pattern(w: usize, h: usize, cpp: usize, bits: u32) -> Vec<u16> {
    let max = (1u32 << bits) - 1;
    (0..w * h * cpp).map(|i| ((i as u32).wrapping_mul(2654435761u32) >> 7) % (max + 1)).map(|v| v as u16).collect()
}

fn pack_msb(v: &[u16], bits: u32, w: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for row in v.chunks(w) {
        let mut acc = 0u64;
        let mut n = 0;
        for &s in row {
            acc = (acc << bits) | s as u64;
            n += bits;
            while n >= 8 {
                out.push((acc >> (n - 8)) as u8);
                n -= 8;
            }
        }
        if n > 0 {
            out.push((acc << (8 - n)) as u8);
        }
    }
    out
}

fn u16_bytes(v: &[u16], order: ByteOrder) -> Vec<u8> {
    let mut b = Vec::new();
    v.iter().for_each(|&x| order.put_u16(&mut b, x));
    b
}

#[test]
fn packed_bit_depths() {
    let (w, h) = (13, 9);
    for bits in [10u32, 12, 14] {
        let px = pattern(w, h, 1, bits);
        let mut raw = base_ifd(w, h, bits as u16, 1, true);
        raw.set(t::COMPRESSION, Value::Short(vec![1]));
        let strips: Vec<Vec<u8>> = px.chunks(w * 4).map(|c| pack_msb(c, bits, w)).collect();
        raw.set_image(ImageData::Strips { rows_per_strip: 4, strips });
        let img = decode(&dng(raw, ByteOrder::Big)).unwrap();
        assert_eq!(img.data, RawData::U16(px), "{bits} bits");
        assert_eq!(img.white, vec![((1u32 << bits) - 1) as f32]);
        assert_eq!(img.cfa.unwrap().name(), "GRBG");
        assert_eq!(img.metadata.iso, Some(200));
        assert_eq!(img.color.as_shot_white_xy, Some(Xy::new(0.3127, 0.329)));
    }
}

#[test]
fn deflate_with_integer_predictors() {
    let (w, h, cpp) = (10, 6, 3);
    let px = pattern(w, h, cpp, 16);
    for (pred, factor) in [(1u16, 0usize), (2, 1), (34892, 2), (34893, 4)] {
        for order in [ByteOrder::Little, ByteOrder::Big] {
            let mut raw = base_ifd(w, h, 16, cpp, false);
            raw.set(t::COMPRESSION, Value::Short(vec![compression::ADOBE_DEFLATE]));
            raw.set(t::PREDICTOR, Value::Short(vec![pred]));
            raw.set_image(ImageData::Tiles {
                tile_width: 16,
                tile_height: 16,
                tiles: vec![{
                    // one 16×16 tile: re-lay the 10×6 image into the tile grid
                    let mut tile = vec![0u16; 16 * 16 * cpp];
                    for y in 0..h {
                        for x in 0..w * cpp {
                            tile[y * 16 * cpp + x] = px[y * w * cpp + x];
                        }
                    }
                    let mut te = tile.clone();
                    if factor > 0 {
                        for row in te.chunks_mut(16 * cpp) {
                            for i in (cpp * factor..row.len()).rev() {
                                row[i] = row[i].wrapping_sub(row[i - cpp * factor]);
                            }
                        }
                    }
                    miniz_oxide::deflate::compress_to_vec_zlib(&u16_bytes(&te, order), 6)
                }],
            });
            let img = decode(&dng(raw, order)).unwrap();
            assert_eq!(img.data, RawData::U16(px.clone()), "predictor {pred} {order:?}");
            assert!(img.cfa.is_none());
            assert_eq!(img.cpp, 3);
        }
    }
}

fn fp_encode(vals: &[f32], bytes_per: usize, stride: usize, w_samples: usize) -> Vec<u8> {
    // big-endian byte planes per row, then byte differencing
    let mut out = Vec::new();
    for row in vals.chunks(w_samples) {
        let be: Vec<[u8; 4]> = row
            .iter()
            .map(|&v| match bytes_per {
                4 => v.to_be_bytes(),
                2 => {
                    let h = f32_to_f16(v).to_be_bytes();
                    [h[0], h[1], 0, 0]
                }
                _ => {
                    let b = f32_to_f24(v).to_be_bytes();
                    [b[1], b[2], b[3], 0]
                }
            })
            .collect();
        let mut planes = vec![0u8; row.len() * bytes_per];
        for (i, b) in be.iter().enumerate() {
            for k in 0..bytes_per {
                planes[k * row.len() + i] = b[k];
            }
        }
        for i in (stride..planes.len()).rev() {
            planes[i] = planes[i].wrapping_sub(planes[i - stride]);
        }
        out.extend(planes);
    }
    out
}

fn f32_to_f16(v: f32) -> u16 {
    // exact for the test values (multiples of 1/64 in [0, 4))
    let b = v.to_bits();
    let sign = ((b >> 16) & 0x8000) as u16;
    if v == 0.0 {
        return sign;
    }
    let exp = ((b >> 23) & 0xff) as i32 - 127 + 15;
    let man = ((b >> 13) & 0x3ff) as u16;
    sign | ((exp as u16) << 10) | man
}

fn f32_to_f24(v: f32) -> u32 {
    let b = v.to_bits();
    if v == 0.0 {
        return 0;
    }
    let sign = (b >> 31) << 23;
    let exp = (((b >> 23) & 0xff) as i32 - 127 + 63) as u32;
    sign | (exp << 16) | ((b >> 7) & 0xffff)
}

#[test]
fn floating_point_dngs() {
    let (w, h) = (12, 5);
    let vals: Vec<f32> = (0..w * h).map(|i| ((i * 37) % 200) as f32 / 64.0).collect();
    for (bits, pred, stride_factor) in [(32u16, 3u16, 1usize), (32, 34894, 2), (32, 34895, 4), (16, 3, 1), (24, 3, 1), (24, 34894, 2)] {
        let bp = bits as usize / 8;
        let mut raw = base_ifd(w, h, bits, 1, true);
        raw.set(t::SAMPLE_FORMAT, Value::Short(vec![3]));
        raw.set(t::COMPRESSION, Value::Short(vec![compression::DEFLATE]));
        raw.set(t::PREDICTOR, Value::Short(vec![pred]));
        let z = miniz_oxide::deflate::compress_to_vec_zlib(&fp_encode(&vals, bp, stride_factor, w), 6);
        raw.set_image(ImageData::Strips { rows_per_strip: h as u32, strips: vec![z] });
        let img = decode(&dng(raw, ByteOrder::Little)).unwrap();
        assert_eq!(img.data, RawData::F32(vals.clone()), "{bits}-bit predictor {pred}");
        assert_eq!(img.white, vec![1.0]);
    }
    // uncompressed half floats, big-endian file
    let mut raw = base_ifd(w, h, 16, 1, true);
    raw.set(t::SAMPLE_FORMAT, Value::Short(vec![3]));
    raw.set(t::COMPRESSION, Value::Short(vec![1]));
    let bytes: Vec<u8> = vals.iter().flat_map(|&v| f32_to_f16(v).to_be_bytes()).collect();
    raw.set_image(ImageData::Strips { rows_per_strip: h as u32, strips: vec![bytes] });
    assert_eq!(decode(&dng(raw, ByteOrder::Big)).unwrap().data, RawData::F32(vals));
}

#[test]
fn lj92_strips_and_levels_and_linearization() {
    let (w, h) = (20, 12);
    let px = pattern(w, h, 1, 12);
    let mut raw = base_ifd(w, h, 12, 1, true);
    raw.set(t::COMPRESSION, Value::Short(vec![compression::JPEG]));
    let strips: Vec<Vec<u8>> = px.chunks(w * 5).map(|c| lightcraft_raw::ljpeg::encode(c, w, c.len() / w, 1, 12, 6, 0)).collect();
    raw.set_image(ImageData::Strips { rows_per_strip: 5, strips });
    let table: Vec<u16> = (0..4096u32).map(|i| (i * i / 4096) as u16).collect();
    raw.set(t::LINEARIZATION_TABLE, Value::Short(table.clone()));
    raw.set(t::BLACK_LEVEL_REPEAT_DIM, Value::Short(vec![2, 2]));
    raw.set(t::BLACK_LEVEL, Value::Rational(vec![(10, 1), (11, 1), (12, 1), (13, 1)]));
    raw.set(t::BLACK_LEVEL_DELTA_H, Value::SRational(vec![(1, 2); 16]));
    raw.set(t::BLACK_LEVEL_DELTA_V, Value::SRational(vec![(-1, 1); 10]));
    raw.set(t::WHITE_LEVEL, Value::Short(vec![4000]));
    raw.set(t::ACTIVE_AREA, Value::Long(vec![1, 2, 11, 18]));
    raw.set(t::DEFAULT_CROP_ORIGIN, Value::Rational(vec![(1, 1), (2, 1)]));
    raw.set(t::DEFAULT_CROP_SIZE, Value::Rational(vec![(12, 1), (6, 1)]));
    let img = decode(&dng(raw, ByteOrder::Little)).unwrap();
    let expect: Vec<u16> = px.iter().map(|&v| table[v as usize]).collect();
    assert_eq!(img.data, RawData::U16(expect.clone()));
    assert!(img.linearized);
    assert_eq!(img.active_area, Rect::new(2, 1, 16, 10));
    assert_eq!(img.crop, Rect::new(1, 2, 12, 6));
    assert_eq!(img.black.at(1, 1, 0, 1), 13.0 + 0.5 - 1.0);
    let n = img.normalized().unwrap();
    let v = expect[(1 + 1) * w + 2 + 1] as f32;
    let b = 13.0 + 0.5 - 1.0;
    assert!((n.data[16 + 1] - (v - b) / (4000.0 - 11.5)).abs() < 1e-5);
    let rgb = img.develop(Method::Ppg).unwrap();
    assert_eq!((rgb.width, rgb.height), (12, 6));
}

#[test]
fn opcodes_are_parsed_and_applied() {
    let mut raw = synthetic(32, 24, Some(Cfa::bayer("RGGB").unwrap()), 1);
    let area = Area { top: 0, left: 0, bottom: 24, right: 32, plane: 0, planes: 1, row_pitch: 1, col_pitch: 1 };
    raw.opcodes.list2 = vec![Opcode::ScalePerRow { area, scales: vec![2.0; 24] }];
    raw.opcodes.list3 = vec![Opcode::FixVignetteRadial { k: [0.0; 5], center: [0.5, 0.5] }, Opcode::Unknown { id: 99, flags: 1, params: vec![] }];
    let bytes = write_dng(&raw, &DngWriteOptions::default()).unwrap();
    let back = decode(&bytes).unwrap();
    assert_eq!(back.opcodes, raw.opcodes);
    let plain = {
        let mut r = raw.clone();
        r.opcodes = Default::default();
        r.normalized().unwrap()
    };
    let scaled = back.normalized().unwrap();
    for (a, b) in plain.data.iter().zip(&scaled.data) {
        assert!((b - 2.0 * a).abs() < 1e-5);
    }
}

#[test]
fn preview_and_probe() {
    let raw = synthetic(16, 16, Some(Cfa::bayer("RGGB").unwrap()), 1);
    let bytes = write_dng(&raw, &DngWriteOptions::default()).unwrap();
    // the raw tiles are lossless JPEG: never offered as a preview
    assert!(embedded_preview(&bytes).is_none());
    assert_eq!(probe(&bytes), Some(RawFormat::Dng));
}

#[test]
fn colour_transform_from_decoded_dng() {
    let raw = decode(&write_dng(&synthetic(16, 16, Some(Cfa::bayer("RGGB").unwrap()), 1), &DngWriteOptions::default()).unwrap()).unwrap();
    let xy = lightcraft_raw::color::as_shot_white_xy(&raw);
    let t = lightcraft_raw::color::camera_transform(&raw, xy);
    assert!(!t.matrix_is_fallback);
    // as-shot neutral (0.5, 1, 0.7) → multipliers (2, 1, 1/0.7) up to normalisation
    assert!((t.wb[0] / t.wb[1] - 2.0).abs() < 1e-3, "{:?}", t.wb);
    assert!((t.wb[2] / t.wb[1] - 1.0 / 0.7).abs() < 1e-3);
    assert!((t.baseline_exposure - 0.35).abs() < 1e-6);
    let (m, _) = lightcraft_raw::color::camera_to_rec2020(&raw, D65);
    let o = m.apply([1.0; 3]);
    assert!(o.iter().all(|v| (v - 1.0).abs() < 1e-9));
}

#[test]
fn rejects_unsupported_and_broken() {
    let (w, h) = (8, 8);
    let mut raw = base_ifd(w, h, 8, 1, true);
    raw.set(t::COMPRESSION, Value::Short(vec![compression::LOSSY_JPEG]));
    raw.set_image(ImageData::Strips { rows_per_strip: 8, strips: vec![vec![0; 10]] });
    // lossy JPEG DNGs decode now; a strip that isn't a JPEG is reported as corrupt
    assert!(matches!(decode(&dng(raw, ByteOrder::Little)), Err(lightcraft_raw::RawError::Corrupt(_))));
    let mut raw = base_ifd(w, h, 16, 1, true);
    raw.set(t::CFA_PATTERN_EP, Value::Byte(vec![0, 1, 3, 1]));
    raw.set(t::COMPRESSION, Value::Short(vec![1]));
    raw.set_image(ImageData::Strips { rows_per_strip: 8, strips: vec![vec![0; 128]] });
    assert!(decode(&dng(raw, ByteOrder::Little)).is_err());
}

/// A Lightroom-style DNG: profile look tags in IFD 0, LinearRaw data in a SubIFD (issue #138).
fn profile_dng(order: ByteOrder, extra: impl FnOnce(&mut IfdBuilder)) -> Vec<u8> {
    let (w, h) = (8, 6);
    let px = pattern(w, h, 3, 16);
    let mut raw = base_ifd(w, h, 16, 3, false);
    raw.set(t::COMPRESSION, Value::Short(vec![1]));
    raw.set_image(ImageData::Strips { rows_per_strip: h as u32, strips: vec![u16_bytes(&px, order)] });
    dng_with(raw, order, extra)
}

fn hsv_floats(h: usize, s: usize, v: usize, f: impl Fn(usize, usize, usize) -> [f32; 3]) -> Vec<f32> {
    (0..v).flat_map(|vi| (0..h).flat_map(move |hi| (0..s).map(move |si| (vi, hi, si)))).flat_map(|(vi, hi, si)| f(vi, hi, si)).collect()
}

#[test]
fn profile_look_tags_are_read_from_ifd0() {
    for order in [ByteOrder::Little, ByteOrder::Big] {
        let bytes = profile_dng(order, |ifd0| {
            ifd0.set(t::PROFILE_HUE_SAT_MAP_DIMS, Value::Long(vec![6, 3, 1]));
            ifd0.set(t::PROFILE_HUE_SAT_MAP_DATA_1, Value::Float(hsv_floats(6, 3, 1, |_, h, s| [h as f32, 1.0 + s as f32 * 0.1, 1.0])));
            ifd0.set(t::PROFILE_HUE_SAT_MAP_DATA_2, Value::Float(hsv_floats(6, 3, 1, |_, _, _| [0.0, 1.0, 1.0])));
            ifd0.set(t::PROFILE_LOOK_TABLE_DIMS, Value::Long(vec![4, 2, 3]));
            ifd0.set(t::PROFILE_LOOK_TABLE_DATA, Value::Float(hsv_floats(4, 2, 3, |v, _, _| [0.0, 1.0, 1.0 - v as f32 * 0.05])));
            ifd0.set(t::PROFILE_LOOK_TABLE_ENCODING, Value::Long(vec![1]));
            ifd0.set(t::PROFILE_TONE_CURVE, Value::Float(vec![0.0, 0.0, 0.25, 0.15, 0.5, 0.55, 1.0, 1.0]));
        });
        let img = decode(&bytes).unwrap();
        assert_eq!(img.cpp, 3);
        let p = &img.color.profile;
        let hsm = p.hue_sat_map[0].as_ref().unwrap();
        assert_eq!((hsm.hue_divisions, hsm.sat_divisions, hsm.val_divisions), (6, 3, 1));
        // value-major, hue-middle, saturation-minor: entry (h = 2, s = 1) is at 2·3 + 1
        assert_eq!(hsm.data[7], [2.0, 1.1, 1.0]);
        assert!(p.hue_sat_map[1].is_some());
        let look = p.look_table.as_ref().unwrap();
        assert_eq!((look.hue_divisions, look.sat_divisions, look.val_divisions, look.srgb_value), (4, 2, 3, true));
        assert_eq!(look.data[2 * 4 * 2], [0.0, 1.0, 0.9]);
        assert_eq!(p.tone_curve.as_ref().unwrap().points.len(), 4);
        // the header-only probe sees the same profile
        assert_eq!(lightcraft_raw::probe_info(&bytes).unwrap().color.profile, *p);
        // our DNG writer keeps the look with the data (conversions, smart previews)
        let again = decode(&write_dng(&img, &DngWriteOptions::default()).unwrap()).unwrap();
        assert_eq!(again.color.profile, *p);
    }
}

#[test]
fn malformed_profile_look_tags_are_ignored() {
    let bytes = profile_dng(ByteOrder::Little, |ifd0| {
        // data count doesn't match the dimensions
        ifd0.set(t::PROFILE_HUE_SAT_MAP_DIMS, Value::Long(vec![6, 3, 1]));
        ifd0.set(t::PROFILE_HUE_SAT_MAP_DATA_1, Value::Float(vec![0.0, 1.0, 1.0]));
        // one saturation division is not allowed
        ifd0.set(t::PROFILE_LOOK_TABLE_DIMS, Value::Long(vec![2, 1, 1]));
        ifd0.set(t::PROFILE_LOOK_TABLE_DATA, Value::Float(vec![0.0, 1.0, 1.0, 0.0, 1.0, 1.0]));
        // decreasing inputs and a NaN
        ifd0.set(t::PROFILE_TONE_CURVE, Value::Float(vec![0.0, 0.0, 0.6, 0.5, 0.4, f32::NAN, 1.0, 1.0]));
    });
    let img = decode(&bytes).unwrap();
    assert!(img.color.profile.is_empty(), "{:?}", img.color.profile);
    // and without any of the tags there is nothing to apply
    let plain = decode(&profile_dng(ByteOrder::Little, |_| {})).unwrap();
    assert!(plain.color.profile.is_empty());
}

// ---------------------------------------------------------------- JPEG XL (DNG 1.7, Compression 52546)

/// A JPEG XL codestream wrapped the way Apple ProRAW tiles are: signature, `ftyp`, one `jxlc` box.
fn container(codestream: &[u8]) -> Vec<u8> {
    let mut v = vec![0, 0, 0, 12, b'J', b'X', b'L', b' ', 0x0d, 0x0a, 0x87, 0x0a];
    v.extend([0, 0, 0, 20, b'f', b't', b'y', b'p', b'j', b'x', b'l', b' ', 0, 0, 0, 0, b'j', b'x', b'l', b' ']);
    v.extend(((codestream.len() + 8) as u32).to_be_bytes());
    v.extend(b"jxlc");
    v.extend(codestream);
    v
}

/// A raw IFD of `w × h` pixels cut into square `tile`s, each one of `tiles` (row-major).
fn jxl_ifd(w: usize, h: usize, tile: usize, cpp: usize, cfa: bool, tiles: Vec<Vec<u8>>) -> IfdBuilder {
    let mut raw = base_ifd(w, h, 16, cpp, cfa);
    raw.set(t::COMPRESSION, Value::Short(vec![compression::JPEG_XL]));
    raw.set_image(ImageData::Tiles { tile_width: tile as u32, tile_height: tile as u32, tiles });
    raw
}

fn jxl_expected(w: usize, h: usize, tile: usize, cpp: usize, maxv: u32, f: impl Fn(u32) -> u16) -> Vec<u16> {
    (0..h)
        .flat_map(|y| (0..w).flat_map(move |x| (0..cpp).map(move |c| (x, y, c))))
        .map(|(x, y, c)| f(jxl_data::val(x % tile, y % tile, c, maxv)))
        .collect()
}

#[test]
fn jxl_linear_raw_tiles_bare_and_in_a_container() {
    // ProRAW style: three channels per pixel; the right-hand tile is cut by the image edge
    let (w, h) = (24, 16);
    let mut raw = jxl_ifd(w, h, 16, 3, false, vec![jxl_data::RGB16.to_vec(), container(jxl_data::RGB16)]);
    raw.set(t::WHITE_LEVEL, Value::Long(vec![65535]));
    let bytes = dng(raw, ByteOrder::Little);
    let img = decode(&bytes).unwrap();
    assert_eq!(img.data, RawData::U16(jxl_expected(w, h, 16, 3, 65535, |v| v as u16)));
    assert_eq!((img.cpp, img.white.clone()), (3, vec![65535.0]));
    // the import-time probe reads the tile headers and agrees
    let info = probe_info(&bytes).unwrap();
    assert_eq!((info.width, info.height, info.cpp), (w, h, 3));
}

#[test]
fn jxl_mosaic_tiles_keep_their_raw_values() {
    // a 12-bit stream under a white level of 4095 holds the raw values themselves
    let mut raw = jxl_ifd(16, 16, 16, 1, true, vec![jxl_data::GRAY12.to_vec()]);
    raw.set(t::WHITE_LEVEL, Value::Long(vec![4095]));
    let img = decode(&dng(raw, ByteOrder::Big)).unwrap();
    assert_eq!(img.data, RawData::U16(jxl_expected(16, 16, 16, 1, 4095, |v| v as u16)));
    assert_eq!(img.cfa.unwrap().name(), "GRBG");
}

#[test]
fn jxl_stream_depth_against_white_level() {
    let tiles = || vec![jxl_data::RGB10.to_vec()];
    // white level within the stream's 10 bits: raw values
    let mut raw = jxl_ifd(16, 16, 16, 3, false, tiles());
    raw.set(t::WHITE_LEVEL, Value::Long(vec![1023]));
    let img = decode(&dng(raw, ByteOrder::Little)).unwrap();
    assert_eq!(img.data, RawData::U16(jxl_expected(16, 16, 16, 3, 1023, |v| v as u16)));
    // Apple declares 10 bits but a white level of 65535: the full 16-bit range
    let mut raw = jxl_ifd(16, 16, 16, 3, false, tiles());
    raw.set(t::WHITE_LEVEL, Value::Long(vec![65535]));
    let img = decode(&dng(raw, ByteOrder::Little)).unwrap();
    assert_eq!(img.data, RawData::U16(jxl_expected(16, 16, 16, 3, 1023, |v| (v as f32 / 1023.0 * 65535.0).round() as u16)));
}

#[test]
fn jxl_codes_under_a_linearization_table_are_not_rescaled_by_the_white_level() {
    // iPhone ProRAW: 10-bit codes, a 1024-entry table up to 65535 and WhiteLevel 65535 (the level after the table)
    let table: Vec<u16> = (0..1024u32).map(|i| (i * 64).min(65535) as u16).collect();
    let mut raw = jxl_ifd(16, 16, 16, 3, false, vec![jxl_data::RGB10.to_vec()]);
    raw.set(t::LINEARIZATION_TABLE, Value::Short(table.clone()));
    raw.set(t::WHITE_LEVEL, Value::Long(vec![65535]));
    let img = decode(&dng(raw, ByteOrder::Little)).unwrap();
    assert_eq!(img.data, RawData::U16(jxl_expected(16, 16, 16, 3, 1023, |v| table[v as usize])));
    assert!(img.linearized);
}

#[test]
fn jxl_lossy_tile_matches_libjxl() {
    let mut raw = jxl_ifd(32, 32, 32, 3, false, vec![jxl_data::LOSSY.to_vec()]);
    raw.set(t::WHITE_LEVEL, Value::Long(vec![65535]));
    let img = decode(&dng(raw, ByteOrder::Little)).unwrap();
    let RawData::U16(v) = &img.data else { panic!("integer samples") };
    for &(x, y, rgb) in jxl_data::LOSSY_REFERENCE {
        for c in 0..3 {
            let got = v[(y * 32 + x) * 3 + c] as i32;
            assert!((got - rgb[c] as i32).abs() <= 64, "({x},{y}) channel {c}: {got} vs libjxl {}", rgb[c]);
        }
    }
}

#[test]
fn jxl_float_tile() {
    let mut raw = jxl_ifd(8, 8, 8, 3, false, vec![container(jxl_data::FLOAT)]);
    raw.set(t::BITS_PER_SAMPLE, Value::Short(vec![32; 3]));
    raw.set(t::SAMPLE_FORMAT, Value::Short(vec![3; 3]));
    let img = decode(&dng(raw, ByteOrder::Little)).unwrap();
    let expect: Vec<f32> = (0..8)
        .flat_map(|y| (0..8).flat_map(move |x| (0..3).map(move |c| (x + 1) as f32 * 0.125 + y as f32 * 0.015625 + c as f32 * 0.0625)))
        .collect();
    assert_eq!(img.data, RawData::F32(expect));
}

#[test]
fn jxl_interleaved_mosaic_is_woven_back() {
    // the stored 16×16 holds 2 row fields (rows 0-7: even final rows, 8-15: odd) and 2 column fields likewise
    let stored = |x: usize, y: usize| jxl_data::val(x, y, 0, 4095) as u16;
    for (rows, cols) in [(2usize, 1usize), (1, 2), (2, 2)] {
        let mut raw = jxl_ifd(16, 16, 16, 1, true, vec![jxl_data::GRAY12.to_vec()]);
        raw.set(t::WHITE_LEVEL, Value::Long(vec![4095]));
        raw.set(t::ROW_INTERLEAVE_FACTOR, Value::Short(vec![rows as u16]));
        raw.set(t::COLUMN_INTERLEAVE_FACTOR, Value::Short(vec![cols as u16]));
        let img = decode(&dng(raw, ByteOrder::Little)).unwrap();
        let RawData::U16(v) = &img.data else { panic!("integer samples") };
        for fy in 0..16 {
            for fx in 0..16 {
                let sy = if rows == 2 { (fy % 2) * 8 + fy / 2 } else { fy };
                let sx = if cols == 2 { (fx % 2) * 8 + fx / 2 } else { fx };
                assert_eq!(v[fy * 16 + fx], stored(sx, sy), "rows {rows} cols {cols} at ({fx},{fy})");
            }
        }
    }
    // a factor larger than the image is a corrupt file, not a panic
    let mut raw = jxl_ifd(16, 16, 16, 1, true, vec![jxl_data::GRAY12.to_vec()]);
    raw.set(t::ROW_INTERLEAVE_FACTOR, Value::Long(vec![4_000_000_000]));
    assert!(matches!(decode(&dng(raw, ByteOrder::Little)), Err(lightcraft_raw::RawError::Corrupt(_))));
}

#[test]
fn jxl_bad_tiles_are_errors() {
    let corrupt = |raw: IfdBuilder| matches!(decode(&dng(raw, ByteOrder::Little)), Err(lightcraft_raw::RawError::Corrupt(_)));
    // one channel where the IFD says three
    assert!(corrupt(jxl_ifd(16, 16, 16, 3, false, vec![jxl_data::GRAY12.to_vec()])));
    // a 16×16 stream in an 8×8 tile
    assert!(corrupt(jxl_ifd(8, 8, 8, 3, false, vec![jxl_data::RGB16.to_vec()])));
    // truncated, and not JPEG XL at all
    assert!(corrupt(jxl_ifd(16, 16, 16, 3, false, vec![jxl_data::RGB16[..300].to_vec()])));
    assert!(corrupt(jxl_ifd(16, 16, 16, 3, false, vec![vec![0x5a; 200]])));
    // the header-only probe rejects what the full decode rejects
    assert!(probe_info(&dng(jxl_ifd(16, 16, 16, 3, false, vec![jxl_data::GRAY12.to_vec()]), ByteOrder::Little)).is_err());
}

#[test]
fn jxl_damaged_streams_never_panic() {
    for stream in [jxl_data::RGB16, jxl_data::LOSSY] {
        let (w, cpp) = if stream.len() == jxl_data::RGB16.len() { (16, 3) } else { (32, 3) };
        for i in 0..stream.len() {
            for flip in [0x01u8, 0x80, 0xff] {
                let mut bad = stream.to_vec();
                bad[i] ^= flip;
                let _ = decode(&dng(jxl_ifd(w, w, w, cpp, false, vec![bad]), ByteOrder::Little));
            }
        }
        for len in (0..stream.len()).step_by(7) {
            let _ = decode(&dng(jxl_ifd(w, w, w, cpp, false, vec![stream[..len].to_vec()]), ByteOrder::Little));
        }
    }
}

// ---------------------------------------------------------------- binned decode (decode_binned)

/// A LinearRaw DNG of uncompressed `tw × th` tiles holding `px` (`w × h × 3`, 16-bit), plus `extra` raw-IFD tags.
fn linear_tiles_dng(w: usize, h: usize, (tw, th): (usize, usize), px: &[u16], extra: impl FnOnce(&mut IfdBuilder)) -> Vec<u8> {
    let mut raw = base_ifd(w, h, 16, 3, false);
    raw.set(t::COMPRESSION, Value::Short(vec![1]));
    let tiles: Vec<Vec<u8>> = (0..h.div_ceil(th))
        .flat_map(|ty| (0..w.div_ceil(tw)).map(move |tx| (tx, ty)))
        .map(|(tx, ty)| {
            let mut tile = vec![0u16; tw * th * 3];
            for y in 0..th.min(h.saturating_sub(ty * th)) {
                for x in 0..tw.min(w.saturating_sub(tx * tw)) {
                    let src = ((ty * th + y) * w + tx * tw + x) * 3;
                    tile[(y * tw + x) * 3..][..3].copy_from_slice(&px[src..src + 3]);
                }
            }
            u16_bytes(&tile, ByteOrder::Little)
        })
        .collect();
    raw.set_image(ImageData::Tiles { tile_width: tw as u32, tile_height: th as u32, tiles });
    extra(&mut raw);
    dng(raw, ByteOrder::Little)
}

/// The streaming binned decode agrees with binning the fully decoded image.
fn assert_binned_matches_full(bytes: &[u8], k: usize, clip: f32) {
    let full = decode(bytes).unwrap();
    let want = full.develop_binned(k, clip).unwrap().unwrap();
    let binned = lightcraft_raw::decode_binned(bytes, k, clip).unwrap().expect("binnable");
    assert_eq!((binned.width, binned.height), (want.width, want.height), "k {k}");
    assert_eq!(binned.crop, Rect::new(0, 0, want.width, want.height));
    let got = binned.develop(Method::Bilinear).unwrap();
    assert_eq!((got.width, got.height), (want.width, want.height));
    // the mean is rounded to a whole raw value
    let tol = 1.0 / (full.white_at(0) - full.black.mean()) + 1e-6;
    for (a, b) in got.data.iter().zip(&want.data) {
        for c in 0..3 {
            assert!((a[c] - b[c]).abs() <= tol, "k {k}: {a:?} vs {b:?}");
        }
    }
    // metadata keeps the photo's own size
    assert_eq!(binned.metadata.width, full.metadata.width);
}

#[test]
fn binned_decode_matches_binning_the_full_image() {
    let (w, h) = (37, 29);
    let px: Vec<u16> = (0..w * h * 3).map(|i| ((i as u32).wrapping_mul(2654435761u32) >> 9) as u16 % 4096).collect();
    let table: Vec<u16> = (0..4096u32).map(|i| (i * i / 280).min(60000) as u16).collect();
    let tags = |raw: &mut IfdBuilder| {
        raw.set(t::LINEARIZATION_TABLE, Value::Short(table.clone()));
        raw.set(t::BLACK_LEVEL, Value::Rational(vec![(100, 1), (130, 1), (90, 1)]));
        raw.set(t::WHITE_LEVEL, Value::Long(vec![58000]));
        raw.set(t::ACTIVE_AREA, Value::Long(vec![1, 2, 28, 35]));
        raw.set(t::DEFAULT_CROP_ORIGIN, Value::Rational(vec![(1, 1), (3, 1)]));
        raw.set(t::DEFAULT_CROP_SIZE, Value::Rational(vec![(30, 1), (24, 1)]));
    };
    // tiles that don't line up with the blocks (11 × 7 against k = 2, 3, 4) and are cut by the image edge
    let bytes = linear_tiles_dng(w, h, (11, 7), &px, tags);
    for k in [2, 3, 4, 8] {
        assert_binned_matches_full(&bytes, k, 0.99);
    }
    // a clipped sample keeps its block's channel at the clipped value (the maximum, not the mean)
    let mut hot = px.clone();
    hot[(10 * w + 12) * 3 + 1] = 4095;
    let bytes = linear_tiles_dng(w, h, (11, 7), &hot, tags);
    assert_binned_matches_full(&bytes, 3, 0.5);
    let b = lightcraft_raw::decode_binned(&bytes, 3, 0.5).unwrap().unwrap();
    assert!(b.normalized().unwrap().data.iter().any(|v| *v >= 0.5), "a clipped channel survives binning");
}

#[test]
fn binned_decode_of_jpeg_xl_tiles() {
    let mut raw = jxl_ifd(32, 16, 16, 3, false, vec![jxl_data::RGB16.to_vec(), jxl_data::RGB16.to_vec()]);
    raw.set(t::WHITE_LEVEL, Value::Long(vec![65535]));
    let bytes = dng(raw, ByteOrder::Little);
    for k in [2, 4] {
        assert_binned_matches_full(&bytes, k, 0.99);
    }
}

#[test]
fn binned_decode_declines_what_it_cannot_bin() {
    let (w, h) = (16, 16);
    let px = pattern(w, h, 3, 12);
    let plain = linear_tiles_dng(w, h, (8, 8), &px, |_| {});
    assert!(lightcraft_raw::decode_binned(&plain, 2, 0.99).unwrap().is_some());
    // a block of one sample, or a silly one
    assert!(lightcraft_raw::decode_binned(&plain, 1, 0.99).unwrap().is_none());
    assert!(lightcraft_raw::decode_binned(&plain, 1000, 0.99).unwrap().is_none());
    // a mosaic, a black level that varies across the image, opcodes
    let mosaic = {
        let mut raw = base_ifd(w, h, 16, 1, true);
        raw.set(t::COMPRESSION, Value::Short(vec![1]));
        raw.set_image(ImageData::Strips { rows_per_strip: 16, strips: vec![vec![0; w * h * 2]] });
        dng(raw, ByteOrder::Little)
    };
    assert!(lightcraft_raw::decode_binned(&mosaic, 2, 0.99).unwrap().is_none());
    let varying = linear_tiles_dng(w, h, (8, 8), &px, |raw| {
        raw.set(t::BLACK_LEVEL_DELTA_H, Value::SRational(vec![(1, 2); 16]));
    });
    assert!(lightcraft_raw::decode_binned(&varying, 2, 0.99).unwrap().is_none());
    let opcode = linear_tiles_dng(w, h, (8, 8), &px, |raw| {
        let area = Area { top: 0, left: 0, bottom: 16, right: 16, plane: 0, planes: 3, row_pitch: 1, col_pitch: 1 };
        let list = lightcraft_raw::opcodes::write_list(&[Opcode::ScalePerRow { area, scales: vec![1.0; 16] }]);
        raw.set(t::OPCODE_LIST_2, Value::Undefined(list));
    });
    assert!(lightcraft_raw::decode_binned(&opcode, 2, 0.99).unwrap().is_none());
    // not a DNG at all
    assert!(lightcraft_raw::decode_binned(b"nothing", 2, 0.99).is_err());
}
