//! JPEG XL tiles (compression 52546) and the carry-over of a rewrite: write, decode, compare with the source.

use craft_tiff::Tiff;
use lightcraft_raw::{
    BlackLevel, Cfa, ColorData, DngCarry, DngCompression, DngWriteOptions, Mat3, OpcodeLists, Orientation, RawData, RawFormat, RawImage, Rect,
    SemanticMask, verify_rewrite, write_dng, write_dng_carrying, write_dng_smaller,
};

/// A `w × h` three-sample 16-bit linear picture: smooth gradients with a little noise, like a ProRAW.
fn linear(w: usize, h: usize) -> RawImage {
    let mut seed = 0x1234_5678u32;
    let data: Vec<u16> = (0..w * h * 3)
        .map(|i| {
            let (px, c) = (i / 3, i % 3);
            let (x, y) = ((px % w) as f32 / w as f32, (px / w) as f32 / h as f32);
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = ((seed >> 20) & 7) as f32 - 4.0;
            (2000.0 + 40000.0 * x * (0.6 + 0.2 * c as f32) * (1.0 - 0.5 * y) + 9000.0 * y + noise).clamp(0.0, 65535.0) as u16
        })
        .collect();
    let color = ColorData {
        illuminant: [21, 0],
        color_matrix: [Some(Mat3([[0.7, -0.1, -0.05], [-0.4, 1.2, 0.2], [-0.1, 0.2, 0.6]])), None],
        as_shot_neutral: Some([0.5, 1.0, 0.7]),
        ..Default::default()
    };
    RawImage {
        format: RawFormat::Dng,
        width: w,
        height: h,
        cpp: 3,
        data: RawData::U16(data),
        cfa: None,
        bits: 16,
        black: BlackLevel::uniform(0.0),
        white: vec![65535.0],
        active_area: Rect::new(0, 0, w, h),
        crop: Rect::new(0, 0, w, h),
        orientation: Orientation::Rotate90,
        color,
        wb_multipliers: None,
        linearized: false,
        opcodes: OpcodeLists::default(),
        metadata: Default::default(),
        lens_metadata: Default::default(),
        pixel_aspect: 1.0,
    }
}

fn opts(compression: DngCompression) -> DngWriteOptions {
    DngWriteOptions { compression, ..Default::default() }
}

fn jxl(tile: u32, distance: Option<f32>) -> DngCompression {
    DngCompression::Jxl { tile, distance, slow: false }
}

fn samples(raw: &RawImage) -> &[u16] {
    match &raw.data {
        RawData::U16(v) => v,
        RawData::F32(_) => &[],
    }
}

#[test]
fn lossless_jxl_tiles_give_the_samples_back_exactly() {
    for (w, h, tile) in [(77, 51, 32), (64, 48, 64), (20, 20, 4096), (33, 17, 16)] {
        let raw = linear(w, h);
        let bytes = write_dng(&raw, &opts(jxl(tile, None))).unwrap();
        let tiff = Tiff::parse(&bytes).unwrap();
        assert_eq!(tiff.ifds[0].u16(craft_tiff::tags::COMPRESSION), Some(52546), "{w}x{h} tile {tile}");
        let back = lightcraft_raw::decode(&bytes).unwrap();
        assert_eq!((back.width, back.height, back.cpp), (w, h, 3));
        assert_eq!(samples(&back), samples(&raw), "{w}x{h} tile {tile}");
        assert_eq!(back.orientation, raw.orientation);
        assert_eq!(back.color.as_shot_neutral, raw.color.as_shot_neutral);
        let err = verify_rewrite(&raw, &bytes, jxl(tile, None), None, 0).unwrap();
        assert_eq!((err.max, err.mean), (0.0, 0.0));
    }
}

#[test]
fn lossy_jxl_is_smaller_the_further_the_distance_and_stays_inside_the_tolerance() {
    let raw = linear(480, 360);
    let lossless = write_dng(&raw, &opts(jxl(64, None))).unwrap().len();
    let mut last = usize::MAX;
    for d in [0.5f32, 2.0] {
        let c = jxl(64, Some(d));
        let bytes = write_dng(&raw, &opts(c)).unwrap();
        // the 128 kB LinearizationTable is the same in every file and dwarfs the payload of this small picture
        let payload = bytes.len() - 131_072;
        assert!(payload < last, "distance {d}: payload {payload} bytes, previous {last}");
        assert!(payload < lossless, "distance {d}: payload {payload} bytes, lossless {lossless}");
        last = payload;
        let err = verify_rewrite(&raw, &bytes, c, None, 0).unwrap_or_else(|e| panic!("distance {d}: {e}"));
        eprintln!("distance {d}: {} bytes (lossless {lossless}), error max {} mean {}", bytes.len(), err.max, err.mean);
        assert!(err.mean < 0.02, "distance {d}: mean {}", err.mean);
        let back = lightcraft_raw::decode(&bytes).unwrap();
        assert!(back.linearized, "the table is applied on reading");
    }
}

#[test]
fn a_damaged_lossy_file_fails_the_check() {
    let raw = linear(64, 48);
    let c = jxl(32, Some(1.0));
    let bytes = write_dng(&raw, &opts(c)).unwrap();
    // the same picture, darker: not what the source holds
    let mut other = raw.clone();
    if let RawData::U16(v) = &mut other.data {
        v.iter_mut().for_each(|s| *s /= 2);
    }
    assert!(verify_rewrite(&other, &bytes, c, None, 0).is_err());
    // lossless output compared with a changed source
    let lossless = write_dng(&raw, &opts(jxl(32, None))).unwrap();
    assert!(verify_rewrite(&other, &lossless, jxl(32, None), None, 0).is_err());
}

#[test]
fn hostile_tiles_and_distances_never_panic() {
    let raw = linear(24, 24);
    for tile in [0, 1, 15, 17, u32::MAX] {
        let bytes = write_dng(&raw, &opts(jxl(tile, None))).unwrap();
        assert_eq!(samples(&lightcraft_raw::decode(&bytes).unwrap()), samples(&raw), "tile {tile}");
    }
    for d in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert!(write_dng(&raw, &opts(jxl(16, Some(d)))).is_err(), "{d}");
    }
    for d in [-3.0f32, 0.0, 1e-9, 1e9] {
        let bytes = write_dng(&raw, &opts(jxl(16, Some(d)))).unwrap();
        assert!(lightcraft_raw::decode(&bytes).is_ok(), "{d}");
    }
}

#[test]
fn mosaic_and_float_data_are_refused_not_written_wrong() {
    let mut mosaic = linear(16, 16);
    mosaic.cpp = 1;
    mosaic.cfa = Cfa::bayer("RGGB");
    mosaic.data = RawData::U16(vec![100; 16 * 16]);
    assert!(write_dng(&mosaic, &opts(jxl(16, None))).is_err());
    let mut float = linear(16, 16);
    float.data = RawData::F32(vec![0.5; 16 * 16 * 3]);
    float.white = vec![1.0];
    assert!(write_dng(&float, &opts(jxl(16, None))).is_err());
}

fn preview() -> Vec<u8> {
    let px: Vec<u8> = (0..16 * 12 * 3).map(|i| (i * 5 % 251) as u8).collect();
    let mut out = Vec::new();
    jpeg_encoder::Encoder::new(&mut out, 90).encode(&px, 16, 12, jpeg_encoder::ColorType::Rgb).unwrap();
    out
}

fn mask(name: &str, id: Option<&str>, w: usize, h: usize) -> SemanticMask {
    SemanticMask {
        name: name.into(),
        instance_id: id.map(String::from),
        width: w,
        height: h,
        sub_area: None,
        data: (0..w * h).map(|i| (i * 977 % 65536) as u16).collect(),
    }
}

/// What a ProRAW holds beyond its pixels.
fn proraw_carry() -> DngCarry {
    use craft_tiff::{Value, tags as t};
    let mut carry = DngCarry {
        exif: vec![
            (t::EXPOSURE_TIME, Value::Rational(vec![(1, 120)])),
            (t::ISO_SPEED_RATINGS_EXT, Value::Short(vec![64])),
            (t::DATE_TIME_ORIGINAL, Value::Ascii("2026:09:20 05:40:00".into())),
            (t::LENS_MODEL, Value::Ascii("iPhone 12 Pro back camera 4.2mm f/1.6".into())),
            (t::MAKER_NOTE, Value::Undefined((0..300u32).map(|i| (i * 7) as u8).collect())),
        ],
        gps: vec![
            (1, Value::Ascii("N".into())),
            (2, Value::Rational(vec![(43, 1), (47, 1), (25, 1)])),
            (3, Value::Ascii("W".into())),
            (4, Value::Rational(vec![(110, 1), (40, 1), (54, 1)])),
        ],
        ifd0: vec![
            (t::DNG_PRIVATE_DATA, Value::Byte(b"Apple private \x00\x01\x02".to_vec())),
            (t::NOISE_PROFILE, Value::Double(vec![1.5e-5, 2.0e-7, 1.6e-5, 2.1e-7, 1.4e-5, 1.9e-7])),
        ],
        masks: vec![mask("Skin", None, 12, 9), mask("Hair", Some("A"), 7, 5)],
        preview: Some(preview()),
    };
    // a parsed IFD lists its entries by tag
    carry.exif.sort_by_key(|(tag, _)| *tag);
    carry
}

fn assert_carried(bytes: &[u8], want: &DngCarry) {
    let got = DngCarry::from_dng(bytes, &Default::default());
    assert_eq!(got.exif, want.exif, "Exif IFD");
    assert_eq!(got.gps, want.gps, "GPS IFD");
    assert_eq!(got.ifd0, want.ifd0, "private data and noise profile");
    assert_eq!(got.masks, want.masks, "semantic masks");
    assert_eq!(got.preview, want.preview, "preview");
}

#[test]
fn a_rewrite_carries_exif_maker_note_private_data_noise_masks_and_preview() {
    let raw = linear(48, 40);
    let want = proraw_carry();
    for compression in [jxl(32, None), jxl(32, Some(1.0)), DngCompression::Lj92 { tile: 32 }, DngCompression::Deflate { tile: 32, half: false }] {
        let bytes = write_dng_carrying(&raw, &opts(compression), &want).unwrap();
        assert_carried(&bytes, &want);
        assert_eq!(craft_raw::embedded_preview(&bytes), want.preview);
        assert_eq!(lightcraft_raw::decode(&bytes).unwrap().width, 48);
    }
    // nothing carried without a carry
    let plain = write_dng(&raw, &opts(jxl(32, None))).unwrap();
    assert_eq!(DngCarry::from_dng(&plain, &Default::default()).ifd0, Vec::new());
    assert!(Tiff::parse(&plain).unwrap().exif().is_none());
}

#[test]
fn rewriting_a_whole_file_keeps_everything_and_checks_the_result() {
    let raw = linear(48, 40);
    let want = proraw_carry();
    let source = write_dng_carrying(&raw, &opts(DngCompression::Deflate { tile: 32, half: false }), &want).unwrap();
    for compression in [jxl(32, None), jxl(32, Some(0.8))] {
        let out = write_dng_smaller(&source, compression).unwrap();
        assert_eq!(out.masks, 2);
        assert_carried(&out.bytes, &want);
        if matches!(compression, DngCompression::Jxl { distance: None, .. }) {
            assert_eq!((out.error.max, out.error.mean), (0.0, 0.0));
        }
    }
    // a geometry-changing compression leaves the masks out and still carries the rest
    let lossy = write_dng_smaller(&source, DngCompression::LossyJpeg { tile: 32, quality: 90, long_edge: Some(24) }).unwrap();
    assert_eq!(lossy.masks, 0);
    assert_eq!(DngCarry::from_dng(&lossy.bytes, &Default::default()).exif, want.exif);
}

#[test]
fn without_an_exif_ifd_the_decoded_metadata_makes_one() {
    let raw = linear(32, 32);
    let source = write_dng(&raw, &opts(DngCompression::Lj92 { tile: 32 })).unwrap();
    let meta = lightcraft_raw::Metadata {
        capture_time: lightcraft_meta::DateTime::parse_iso("2026-09-20T05:40:00"),
        gps: Some(lightcraft_meta::Gps { latitude: 43.79, longitude: -110.68, altitude: None }),
        lens_model: Some("35mm F2".into()),
        ..Default::default()
    };
    let carry = DngCarry::from_dng(&source, &meta);
    assert!(!carry.exif.is_empty() && !carry.gps.is_empty());
    let bytes = write_dng_carrying(&raw, &opts(jxl(32, None)), &carry).unwrap();
    let back = lightcraft_raw::decode(&bytes).unwrap();
    assert_eq!(back.metadata.lens_model.as_deref(), Some("35mm F2"));
    assert_eq!(back.metadata.capture_time.map(|d| d.to_exif()), meta.capture_time.map(|d| d.to_exif()));
    assert!(back.metadata.gps.is_some());
}

#[test]
fn a_damaged_source_gives_an_empty_carry_never_a_panic() {
    let raw = linear(32, 32);
    let source = write_dng_carrying(&raw, &opts(jxl(32, None)), &proraw_carry()).unwrap();
    for len in (0..source.len()).step_by(97) {
        let _ = DngCarry::from_dng(&source[..len], &Default::default());
        let _ = write_dng_smaller(&source[..len], jxl(32, None));
    }
    assert_eq!(DngCarry::from_dng(b"not a dng at all", &Default::default()).exif, Vec::new());
    assert_eq!(DngCarry::from_dng(&[], &Default::default()), DngCarry::default());
    // flipped bytes
    for at in (0..source.len()).step_by(211) {
        let mut bad = source.clone();
        bad[at] ^= 0xa5;
        let _ = DngCarry::from_dng(&bad, &Default::default());
        let _ = write_dng_smaller(&bad, jxl(32, Some(1.0)));
    }
}

/// The real ProRAW of the test-raws set (an iPhone 12 Pro, 12 MP): sizes and errors of the rewrites. Skipped without it.
#[test]
fn a_real_proraw_rewrites_smaller() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps/lightbites-swift/test-raws/Apple/iPhone 12 Pro/IMG_1361.DNG");
    let Ok(source) = std::fs::read(&path) else {
        lightcraft_testkit::skip!(Corpus, "the iPhone 12 Pro ProRAW is not in apps/lightbites-swift/test-raws");
        return;
    };
    let src_carry = DngCarry::from_dng(&source, &Default::default());
    eprintln!(
        "source {} bytes: exif {} entries, gps {}, ifd0 extras {:?}, {} masks, preview {:?} bytes",
        source.len(),
        src_carry.exif.len(),
        src_carry.gps.len(),
        src_carry.ifd0.iter().map(|(t, _)| *t).collect::<Vec<_>>(),
        src_carry.masks.len(),
        src_carry.preview.as_ref().map(Vec::len)
    );
    for compression in [
        jxl(1024, None),
        DngCompression::Jxl { tile: 1024, distance: None, slow: true },
        jxl(1024, Some(0.5)),
        jxl(1024, Some(1.0)),
        jxl(1024, Some(1.5)),
        DngCompression::Lj92 { tile: 256 },
    ] {
        let started = std::time::Instant::now();
        let out = write_dng_smaller(&source, compression).unwrap();
        eprintln!(
            "{compression:?}: {} bytes ({:.1} % of the source), error max {:.5} mean {:.6}, {} masks, {:.1} s",
            out.bytes.len(),
            100.0 * out.bytes.len() as f64 / source.len() as f64,
            out.error.max,
            out.error.mean,
            out.masks,
            started.elapsed().as_secs_f64()
        );
        let back = DngCarry::from_dng(&out.bytes, &Default::default());
        assert_eq!(back.exif, src_carry.exif);
        assert_eq!(back.ifd0, src_carry.ifd0);
        assert_eq!(back.masks.len(), src_carry.masks.len());
        assert_eq!(back.preview.is_some(), src_carry.preview.is_some());
    }
}
