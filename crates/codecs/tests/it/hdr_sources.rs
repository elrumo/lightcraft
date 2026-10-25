//! HDR sources: the gain map of an iPhone HEIC (Apple's auxiliary image, its maker note) and of an
//! ISO tone-map HEIC, read from synthetic files.

use lightcraft_codecs::gainmap::{self, GainMapMeta};
use lightcraft_heif::testdata::{Spec, build};

fn grey(x: u32, y: u32) -> [u16; 3] {
    [((x * 7 + y * 13) % 220 + 20) as u16, 128, 128]
}

/// EXIF (big-endian TIFF) whose Exif IFD holds an Apple maker note with tags 33 and 48 (signed
/// rationals) set to `headroom` and `gain`.
fn apple_exif(headroom: (i32, i32), gain: (i32, i32)) -> Vec<u8> {
    let mut note = b"Apple iOS\0\0\x01MM".to_vec();
    note.extend_from_slice(&2u16.to_be_bytes());
    for (tag, at) in [(33u16, 44u32), (48, 52)] {
        note.extend_from_slice(&tag.to_be_bytes());
        note.extend_from_slice(&10u16.to_be_bytes());
        note.extend_from_slice(&1u32.to_be_bytes());
        note.extend_from_slice(&at.to_be_bytes());
    }
    note.extend_from_slice(&0u32.to_be_bytes());
    for (n, d) in [headroom, gain] {
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

#[test]
fn apple_maker_note_gives_the_gain_maps_stops() {
    // Apple's recipe for a headroom of at least 1 and a gain above 0.01: 2.303 - 0.303 * gain
    let stops = gainmap::apple_stops(&apple_exif((3, 2), (1, 2))).unwrap();
    assert!((stops - 2.1515).abs() < 1e-4, "{stops}");
    let low = gainmap::apple_stops(&apple_exif((1, 2), (0, 1))).unwrap();
    assert!((low - 1.8).abs() < 1e-5, "{low}");
    // not an Apple note, a missing rational, a zero denominator
    assert_eq!(gainmap::apple_stops(b"MM\0\x2a\0\0\0\x08\0\0\0\0\0\0"), None);
    assert_eq!(gainmap::apple_stops(&apple_exif((3, 0), (1, 2))), None);
    assert_eq!(gainmap::apple_stops(&[]), None);
}

#[test]
fn an_iphone_heic_gain_map_is_read_with_the_headroom_of_its_maker_note() {
    let gain = |x: u32, _: u32| (x * 4) as u16;
    let bytes = build(&Spec { exif: Some(apple_exif((3, 2), (1, 2))), gain: Some(&gain), gain_size: (48, 32), ..Spec::new(96, 64, &grey) });
    let g = gainmap::read(&bytes).unwrap();
    assert_eq!((g.gain.width, g.gain.height, g.gain.channels), (48, 32, 1));
    assert_eq!(g.gain.data[10], 40);
    assert!((g.meta.alternate_headroom - 2.1515).abs() < 1e-4);
    assert_eq!((g.meta.min[0], g.meta.base_offset[0], g.meta.gamma[0]), (0.0, 0.0, 1.0));
    // a code of 255 is the full headroom, 0 leaves the base alone
    let mut px = [[0.25f32; 3]; 1];
    let full = gainmap::GainImage { width: 1, height: 1, channels: 1, data: vec![255] };
    gainmap::apply(&mut px, 1, 1, &full, &g.meta, 1.0).unwrap();
    assert!((px[0][1] - 0.25 * 2.1515f32.exp2()).abs() < 1e-3, "{px:?}");
    // no maker note: the map can't be scaled, so the file stays SDR
    let bare = build(&Spec { gain: Some(&gain), ..Spec::new(96, 64, &grey) });
    assert!(gainmap::read(&bare).is_none());
    // no gain map at all
    assert!(gainmap::read(&build(&Spec::new(96, 64, &grey))).is_none());
}

#[test]
fn an_iso_tone_map_heic_brings_its_own_metadata() {
    let meta = GainMapMeta { alternate_headroom: 3.0, max: [3.0; 3], ..GainMapMeta::default() };
    let iso = meta.to_iso()[gainmap::ISO_URN.len()..].to_vec();
    let gain = |x: u32, _: u32| (x * 4) as u16;
    let bytes = build(&Spec { gain: Some(&gain), tmap: Some(iso), ..Spec::new(96, 64, &grey) });
    let g = gainmap::read(&bytes).unwrap();
    assert!((g.meta.alternate_headroom - 3.0).abs() < 1e-5 && (g.meta.max[0] - 3.0).abs() < 1e-5);
    // unusable metadata leaves the file SDR
    let bad = build(&Spec { gain: Some(&gain), tmap: Some(vec![9, 9, 9]), ..Spec::new(96, 64, &grey) });
    assert!(gainmap::read(&bad).is_none());
}

#[test]
fn hostile_bytes_have_no_gain_map_and_no_panic() {
    let gain = |x: u32, _: u32| x as u16;
    let bytes = build(&Spec { exif: Some(apple_exif((3, 2), (1, 2))), gain: Some(&gain), ..Spec::new(96, 64, &grey) });
    for cut in (0..bytes.len()).step_by(13) {
        let _ = gainmap::read(&bytes[..cut]);
    }
    for junk in [&[][..], &[0xFF, 0xD8, 0xFF], &[0xFF; 100]] {
        assert!(gainmap::read(junk).is_none());
    }
    let mut exif = apple_exif((3, 2), (1, 2));
    for i in 0..exif.len() {
        exif[i] ^= 0xA5;
        let _ = gainmap::apple_stops(&exif);
        exif[i] ^= 0xA5;
    }
}
