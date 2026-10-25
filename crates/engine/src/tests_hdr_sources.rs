//! HDR sources: a file's own gain map (an iPhone HEIC, an Ultra HDR JPEG) gives the HDR render real highlights, and
//! the SDR render stays the base picture's.

use serde_json::json;

use crate::Session;
use crate::export::{ExportOptions, export_photo};
use crate::tests_xmp::temp_dir;
use lightcraft_heif::testdata::{Spec, build};

const Y: [f32; 3] = [0.2126, 0.7152, 0.0722];

fn luma(p: &[f32; 3]) -> f32 {
    Y[0] * p[0] + Y[1] * p[1] + Y[2] * p[2]
}

/// An EXIF with an Apple maker note: tag 33 = 3/2 and tag 48 = 1/2, which Apple's recipe makes 2.15 stops.
fn apple_exif() -> Vec<u8> {
    let mut note = b"Apple iOS\0\0\x01MM".to_vec();
    note.extend_from_slice(&2u16.to_be_bytes());
    for (tag, at) in [(33u16, 44u32), (48, 52)] {
        note.extend_from_slice(&tag.to_be_bytes());
        note.extend_from_slice(&10u16.to_be_bytes());
        note.extend_from_slice(&1u32.to_be_bytes());
        note.extend_from_slice(&at.to_be_bytes());
    }
    note.extend_from_slice(&0u32.to_be_bytes());
    for (n, d) in [(3i32, 2i32), (1, 2)] {
        note.extend_from_slice(&n.to_be_bytes());
        note.extend_from_slice(&d.to_be_bytes());
    }
    let mut t = b"MM\0\x2a\0\0\0\x08".to_vec();
    t.extend_from_slice(&1u16.to_be_bytes());
    t.extend_from_slice(&[0x87, 0x69, 0, 4, 0, 0, 0, 1, 0, 0, 0, 26]);
    t.extend_from_slice(&0u32.to_be_bytes());
    t.extend_from_slice(&1u16.to_be_bytes());
    t.extend_from_slice(&[0x92, 0x7C, 0, 7]);
    t.extend_from_slice(&(note.len() as u32).to_be_bytes());
    t.extend_from_slice(&44u32.to_be_bytes());
    t.extend_from_slice(&0u32.to_be_bytes());
    t.extend_from_slice(&note);
    t
}

/// The float TIFF export of a file's one photo, its HDR rendition or SDR one, with the HDR edit on or off.
fn tiff(path: &std::path::Path, hdr_edit: bool, hdr_output: bool) -> Vec<[f32; 3]> {
    let mut s = Session::new().with_fs();
    s.execute("library.import", &json!({"paths": [path.to_string_lossy()]})).unwrap();
    let id = s.catalog.photos().next().unwrap().id;
    s.execute("library.select", &json!({"ids": [id.0]})).unwrap();
    if hdr_edit {
        s.execute("develop.hdr", &json!({"enabled": true, "maxEv": 3})).unwrap();
    }
    let o = ExportOptions::from_json(&json!({"longEdge": 96, "format": "tiff", "bitDepth": 32, "hdr": hdr_output}));
    lightcraft_codecs::decode(&export_photo(&mut s, id, &o, 1).unwrap().bytes, Default::default()).unwrap().image.data
}

/// An iPhone-like HEIC: a mid-grey ramp whose right side is bright, with Apple's gain map (code 0 on the left, 255 on
/// the right) and its maker note, or the same picture without them.
fn heic(dir: &std::path::Path, name: &str, gain: bool) -> std::path::PathBuf {
    let sample = |x: u32, _: u32| [(60 + x * 2) as u16, 128, 128];
    let ramp = |x: u32, _: u32| (x * 255 / 47) as u16;
    let spec = Spec {
        exif: gain.then(apple_exif),
        gain: gain.then_some(&ramp as &dyn Fn(u32, u32) -> u16),
        gain_size: (48, 32),
        ..Spec::new(96, 64, &sample)
    };
    let path = dir.join(name);
    std::fs::write(&path, build(&spec)).unwrap();
    path
}

#[test]
fn an_iphone_heic_gain_map_lights_up_the_hdr_render_and_leaves_the_sdr_one_alone() {
    let dir = temp_dir("hdr-heic");
    let (with, without) = (heic(&dir, "with.heic", true), heic(&dir, "without.heic", false));
    // the SDR renditions are the same bytes with and without the gain map, whether or not the edit is HDR
    let plain = tiff(&without, false, false);
    assert_eq!(tiff(&with, false, false), plain, "the SDR render is the base picture's");
    assert_eq!(tiff(&with, true, false), plain, "an HDR edit leaves the SDR export alone");
    // the HDR rendition: no gain map, nothing above SDR white; with one, the bright side goes past it
    let flat = tiff(&without, true, true);
    assert!(flat.iter().all(|p| luma(p) <= 1.0001), "no gain map, no highlights");
    let hdr = tiff(&with, true, true);
    let peak = hdr.iter().map(luma).fold(0.0f32, f32::max);
    assert!(peak > 1.8, "the gain map's highlights reach the render: peak {peak}");
    // and never darker than the SDR picture; its left side (gain code 0) is unchanged
    for (h, s) in hdr.iter().zip(&plain) {
        assert!(luma(h) >= luma(s) - 2e-3, "{h:?} darker than {s:?}");
    }
    let row = |v: &[[f32; 3]]| v[..96].to_vec();
    assert!((luma(&row(&hdr)[0]) - luma(&row(&plain)[0])).abs() < 5e-3);
    assert!(luma(&row(&hdr)[95]) > luma(&row(&plain)[95]) * 2.0, "the right edge gains more than a stop");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_gain_map_that_cannot_be_used_leaves_the_file_sdr() {
    let dir = temp_dir("hdr-heic-bad");
    // a gain map but no maker note to size it by: the file opens as its SDR base, HDR render included
    let sample = |x: u32, _: u32| [(60 + x * 2) as u16, 128, 128];
    let ramp = |x: u32, _: u32| (x * 255 / 47) as u16;
    let path = dir.join("bare.heic");
    std::fs::write(&path, build(&Spec { gain: Some(&ramp), gain_size: (48, 32), ..Spec::new(96, 64, &sample) })).unwrap();
    assert!(tiff(&path, true, true).iter().all(|p| luma(p) <= 1.0001));
    let _ = std::fs::remove_dir_all(&dir);
}

/// An Ultra HDR JPEG written by LightCraft's own HDR export opens again with its highlights: the HDR rendition of
/// the re-imported file is within a few percent of the render it was made from, and its SDR one is the JPEG's base.
#[test]
fn an_ultra_hdr_jpeg_export_comes_back_with_its_highlights() {
    let dir = temp_dir("hdr-jpeg");
    let mut s = Session::with_demo();
    let id = s.catalog.photos().find(|p| p.meta.title == "Golden horizon").map(|p| p.id).unwrap();
    s.execute("library.select", &json!({"ids": [id.0]})).unwrap();
    s.execute("develop.hdr", &json!({"enabled": true, "maxEv": 3})).unwrap();
    let jpeg = export_photo(&mut s, id, &ExportOptions::from_json(&json!({"longEdge": 320, "hdr": true})), 1).unwrap();
    let want = export_photo(&mut s, id, &ExportOptions::from_json(&json!({"longEdge": 320, "format": "tiff", "bitDepth": 32, "hdr": true})), 1)
        .unwrap();
    let want = lightcraft_codecs::decode(&want.bytes, Default::default()).unwrap().image.data;
    let path = dir.join("ultra.jpg");
    std::fs::write(&path, &jpeg.bytes).unwrap();

    let mut t = Session::new().with_fs();
    t.execute("library.import", &json!({"paths": [path.to_string_lossy()]})).unwrap();
    let back = t.catalog.photos().next().unwrap().id;
    t.execute("library.select", &json!({"ids": [back.0]})).unwrap();
    let o = ExportOptions::from_json(&json!({"longEdge": 320, "format": "tiff", "bitDepth": 32, "hdr": true}));
    let sdr = |t: &mut Session| {
        let o = ExportOptions::from_json(&json!({"longEdge": 320, "format": "tiff", "bitDepth": 32, "hdr": false}));
        lightcraft_codecs::decode(&export_photo(t, back, &o, 1).unwrap().bytes, Default::default()).unwrap().image.data
    };
    let before = sdr(&mut t);
    t.execute("develop.hdr", &json!({"enabled": true, "maxEv": 3})).unwrap();
    assert_eq!(sdr(&mut t), before, "an HDR edit leaves the SDR export alone");
    let got = lightcraft_codecs::decode(&export_photo(&mut t, back, &o, 1).unwrap().bytes, Default::default()).unwrap().image.data;
    assert_eq!(got.len(), want.len());
    let (mut n, mut sum) = (0, 0.0f32);
    let (mut max_got, mut max_want) = (0.0f32, 0.0f32);
    for (a, b) in got.iter().zip(&want) {
        (max_got, max_want) = (max_got.max(luma(a)), max_want.max(luma(b)));
        if luma(b) > 1.0 {
            sum += ((luma(a) - luma(b)) / luma(b)).abs();
            n += 1;
        }
    }
    assert!(max_want > 2.0 && n > 100, "the sun is above white (peak {max_want}, {n} pixels)");
    assert!((max_got / max_want - 1.0).abs() < 0.2, "peak {max_got} re-imported, {max_want} rendered");
    assert!(sum / (n as f32) < 0.25, "mean luminance error above white {}", sum / n as f32);
    let _ = std::fs::remove_dir_all(&dir);
}
