//! Render to DNG: a linear DNG of a rendered image reads back as the same pixels and colours.

use lightcraft_color::{Mat3, PROPHOTO, REC2020, SRGB};
use lightcraft_raw::{LinearDngLook, Method, color, write_linear_dng, write_rendered_dng};

fn image(w: usize, h: usize) -> Vec<f32> {
    (0..w * h).flat_map(|i| [(i % w) as f32 / w as f32, (i / w) as f32 / h as f32, 0.25 + (i % 7) as f32 * 0.25]).collect()
}

fn close(a: &Mat3, b: &Mat3, tol: f64) -> bool {
    a.0.iter().flatten().zip(b.0.iter().flatten()).all(|(x, y)| (x - y).abs() < tol)
}

#[test]
fn rendered_dng_round_trips_pixels_and_colour() {
    let (w, h) = (300usize, 70usize);
    let rgb = image(w, h);
    for space in [SRGB, PROPHOTO] {
        let meta = lightcraft_meta::Metadata { make: Some("Maker".into()), model: Some("Model".into()), ..Default::default() };
        let bytes = write_rendered_dng(w, h, &rgb, &space, &meta, None).unwrap();
        let raw = lightcraft_raw::decode(&bytes).unwrap();
        assert_eq!((raw.width, raw.height, raw.cpp, raw.cfa.is_none()), (w, h, 3, true));
        assert_eq!(raw.metadata.model.as_deref(), Some("Model"));
        let dev = raw.develop(Method::Ahd).unwrap();
        for (p, q) in rgb.as_chunks::<3>().0.iter().zip(&dev.data) {
            for k in 0..3 {
                // above-white samples (1.25, 1.5, 1.75) are clipped by white level 1 on develop
                let want = p[k].min(1.0);
                assert!((q[k].min(1.0) - want).abs() <= want * 1e-3 + 1e-4, "{space:?} {p:?} {q:?}");
            }
        }
        // the camera transform is the space → Rec. 2020 matrix under a neutral white
        let t = color::camera_transform(&raw, color::as_shot_white_xy(&raw));
        assert!(!t.matrix_is_fallback);
        assert!(t.wb.iter().all(|v| (v - 1.0).abs() < 1e-3), "{:?}", t.wb);
        assert!(close(&t.matrix, &space.to_space(&REC2020), 2e-3), "{}: {:?} vs {:?}", space.name, t.matrix, space.to_space(&REC2020));
    }
}

#[test]
fn rendered_dng_rejects_bad_input_without_panicking() {
    let m = lightcraft_meta::Metadata::default();
    assert!(write_rendered_dng(0, 4, &[], &SRGB, &m, None).is_err());
    assert!(write_rendered_dng(4, 4, &[0.5; 10], &SRGB, &m, None).is_err());
    assert!(write_rendered_dng(usize::MAX, 3, &[0.5; 10], &SRGB, &m, None).is_err());
    let odd = [f32::NAN, f32::INFINITY, -2.0, 70000.0, 0.5, 1.0];
    let raw = lightcraft_raw::decode(&write_rendered_dng(2, 1, &odd, &SRGB, &m, None).unwrap()).unwrap();
    let dev = raw.develop(Method::Bilinear).unwrap();
    assert!(dev.data.iter().flatten().all(|v| v.is_finite() && *v >= 0.0));
}

#[test]
fn linear_dng_keeps_highlights_above_white_and_says_how_to_read_them() {
    let (w, h) = (64usize, 16usize);
    // a ramp that runs to 5.5 (scene-referred highlights), as a decoder's linear output can
    let rgb: Vec<f32> = (0..w * h).flat_map(|i| [i as f32 / (w * h) as f32 * 5.5; 3]).collect();
    let meta = lightcraft_meta::Metadata::default();
    let look = LinearDngLook {
        baseline_exposure: 0.5,
        identity_tone: false,
        headroom: true,
        preview: None,
        compact: false,
        as_shot_white: None,
        tone_curve: None,
    };
    let raw = lightcraft_raw::decode(&write_linear_dng(w, h, &rgb, &SRGB, &meta, None, look).unwrap()).unwrap();
    // stored divided by 8 (the smallest power of two above 5.5), the stops put back in BaselineExposure
    assert!((raw.color.baseline_exposure - 3.5).abs() < 1e-9, "{}", raw.color.baseline_exposure);
    assert!(raw.color.profile.tone_curve.is_none());
    let dev = raw.develop(Method::Ahd).unwrap();
    for (p, q) in rgb.as_chunks::<3>().0.iter().zip(&dev.data) {
        // nothing clipped at the white level: value x 8 is what went in (half-float precision)
        assert!((q[0] * 8.0 - p[0]).abs() <= p[0] * 2e-3 + 1e-3, "{p:?} {q:?}");
    }
    // without headroom the same ramp clips at 1 and no stops are added
    let flat = LinearDngLook {
        baseline_exposure: 0.0,
        identity_tone: true,
        headroom: false,
        preview: None,
        compact: false,
        as_shot_white: None,
        tone_curve: None,
    };
    let raw = lightcraft_raw::decode(&write_linear_dng(w, h, &rgb, &SRGB, &meta, None, flat).unwrap()).unwrap();
    assert!(raw.color.baseline_exposure.abs() < 1e-9 && raw.color.profile.tone_curve.is_some());
    // a baseline exposure that isn't a number is not written
    let nan = LinearDngLook {
        baseline_exposure: f64::NAN,
        identity_tone: true,
        headroom: true,
        preview: None,
        compact: false,
        as_shot_white: None,
        tone_curve: None,
    };
    let raw = lightcraft_raw::decode(&write_linear_dng(2, 1, &[0.5; 6], &SRGB, &meta, None, nan).unwrap()).unwrap();
    assert!(raw.color.baseline_exposure.is_finite());
}

#[test]
fn compact_linear_dng_keeps_the_pixels_and_carries_a_preview() {
    let (w, h) = (512usize, 256usize);
    // a ramp with sensor-like noise
    let mut seed = 12345u32;
    let mut noise = move || {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        1.0 + ((seed >> 16) as f32 / 65535.0 - 0.5) * 0.06
    };
    let rgb: Vec<f32> =
        (0..w * h).flat_map(|i| [(i % w) as f32 / w as f32 * 3.0 * noise(), (i / w) as f32 / h as f32 * 3.0 * noise(), 0.4 * noise()]).collect();
    let meta = lightcraft_meta::Metadata::default();
    let jpeg = {
        let px: Vec<u8> = (0..16 * 12).flat_map(|i| [(i % 16 * 15) as u8, 90, 160]).collect();
        let mut out = Vec::new();
        jpeg_encoder::Encoder::new(&mut out, 90).encode(&px, 16, 12, jpeg_encoder::ColorType::Rgb).unwrap();
        out
    };
    let look = |compact| LinearDngLook {
        baseline_exposure: 0.0,
        identity_tone: false,
        headroom: true,
        preview: Some(&jpeg),
        compact,
        as_shot_white: None,
        tone_curve: None,
    };
    let float = write_linear_dng(w, h, &rgb, &SRGB, &meta, None, look(false)).unwrap();
    let ints = write_linear_dng(w, h, &rgb, &SRGB, &meta, None, look(true)).unwrap();
    let raw = lightcraft_raw::decode(&ints).unwrap();
    assert!((raw.color.baseline_exposure - 2.0).abs() < 1e-9, "peak 3 is stored under 4: {}", raw.color.baseline_exposure);
    let dev = raw.develop(Method::Ahd).unwrap();
    for (p, q) in rgb.as_chunks::<3>().0.iter().zip(&dev.data) {
        for k in 0..3 {
            assert!((q[k] * 4.0 - p[k]).abs() <= 4.0 / 65535.0 * 2.0 + p[k] * 1e-4, "{p:?} {q:?}");
        }
    }
    // the preview is the JPEG that went in, found where readers look, in both
    for bytes in [&ints, &float] {
        assert_eq!(lightcraft_raw::embedded_preview(bytes).as_deref(), Some(jpeg.as_slice()));
    }
    // something that isn't a JPEG is refused, not embedded
    let bad = LinearDngLook { preview: Some(b"nope"), ..look(true) };
    assert!(write_linear_dng(w, h, &rgb, &SRGB, &meta, None, bad).is_err());
}

#[test]
fn linear_dng_keeps_the_scene_white_it_was_balanced_for() {
    let (w, h) = (32usize, 8usize);
    let grey = vec![0.5f32; w * h * 3];
    let meta = lightcraft_meta::Metadata::default();
    let warm = lightcraft_color::Xy::new(0.40, 0.38);
    let look = |white| LinearDngLook {
        baseline_exposure: 0.0,
        identity_tone: false,
        headroom: false,
        preview: None,
        compact: false,
        as_shot_white: white,
        tone_curve: None,
    };
    // balanced (white D65): grey stays grey and the as-shot white is D65
    let plain = lightcraft_raw::decode(&write_linear_dng(w, h, &grey, &SRGB, &meta, None, look(None)).unwrap()).unwrap();
    assert!((color::as_shot_white_xy(&plain).x - lightcraft_color::D65.x).abs() < 1e-3);
    // a picture balanced for a warm light is stored as the raw it came from: the as-shot white is that light, the grey is
    // not grey any more, and the reader's white balance turns it back to grey
    let raw = lightcraft_raw::decode(&write_linear_dng(w, h, &grey, &SRGB, &meta, None, look(Some(warm))).unwrap()).unwrap();
    let white = color::as_shot_white_xy(&raw);
    assert!((white.x - warm.x).abs() < 2e-3 && (white.y - warm.y).abs() < 2e-3, "{white:?}");
    let px = raw.develop(Method::Bilinear).unwrap().data[0];
    assert!((px[0] - px[2]).abs() > 0.1, "stored unbalanced: {px:?}");
    let t = color::camera_transform(&raw, white);
    let balanced = [px[0] * t.wb[0], px[1] * t.wb[1], px[2] * t.wb[2]];
    let spread = balanced.iter().cloned().fold(f32::MIN, f32::max) - balanced.iter().cloned().fold(f32::MAX, f32::min);
    assert!(spread < 0.01 * balanced[1], "{balanced:?}");
    // a white that is not a colour is ignored, not written
    for bad in [lightcraft_color::Xy::new(f64::NAN, 0.3), lightcraft_color::Xy::new(0.0, 0.0), lightcraft_color::Xy::new(0.3, 5.0)] {
        let raw = lightcraft_raw::decode(&write_linear_dng(w, h, &grey, &SRGB, &meta, None, look(Some(bad))).unwrap()).unwrap();
        assert!((color::as_shot_white_xy(&raw).x - lightcraft_color::D65.x).abs() < 1e-3, "{bad:?}");
    }
}
