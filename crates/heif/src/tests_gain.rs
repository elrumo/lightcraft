//! HDR gain maps (Apple's auxiliary image, ISO tone-map items) read from synthetic files.

use crate::testdata::{Prop, Spec, build};
use crate::{Options, gain_map};

fn grey(x: u32, y: u32) -> [u16; 3] {
    let v = (60 + x * 2 + y) as u16;
    [v, 128, 128]
}

#[test]
fn apple_gain_map_is_found_and_turned_like_the_picture() {
    let gain = |x: u32, y: u32| ((x * 3 + y) % 256) as u16;
    let plain = build(&Spec::new(64, 32, &grey));
    assert_eq!(gain_map(&plain, &Options::default()).unwrap(), None);
    let bytes = build(&Spec { gain: Some(&gain), gain_size: (32, 16), ..Spec::new(64, 32, &grey) });
    let g = gain_map(&bytes, &Options::default()).unwrap().unwrap();
    assert_eq!((g.map.width, g.map.height, g.map.max, g.iso), (32, 16, 255, None));
    assert_eq!((g.map.data[0], g.map.data[5 + 2 * 32]), (gain(0, 0), gain(5, 2)));
    // a quarter turn swaps the map's axes with the picture's
    let turned = build(&Spec { props: vec![Prop::Irot(1)], gain: Some(&gain), gain_size: (32, 16), ..Spec::new(64, 32, &grey) });
    let g = gain_map(&turned, &Options::default()).unwrap().unwrap();
    assert_eq!((g.map.width, g.map.height), (16, 32));
}

#[test]
fn iso_tone_map_item_gives_the_gain_map_and_its_metadata() {
    let gain = |x: u32, _: u32| (x * 4) as u16;
    let iso = vec![0, 0, 0, 0, 0, 1, 2, 3];
    let bytes = build(&Spec { gain: Some(&gain), tmap: Some(iso.clone()), ..Spec::new(64, 32, &grey) });
    let g = gain_map(&bytes, &Options::default()).unwrap().unwrap();
    assert_eq!((g.map.width, g.map.height, g.iso), (64, 32, Some(iso)));
    assert_eq!(g.map.data[10], 40);
}

#[test]
fn damaged_files_have_no_gain_map_and_no_panic() {
    let gain = |x: u32, _: u32| x as u16;
    let bytes = build(&Spec { gain: Some(&gain), ..Spec::new(64, 32, &grey) });
    for cut in (0..bytes.len()).step_by(11) {
        let _ = gain_map(&bytes[..cut], &Options::default());
    }
    assert!(gain_map(&[0xFF; 64], &Options::default()).is_err());
}
