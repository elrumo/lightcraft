//! Object selections that come from outside the engine as a coverage picture (Apple Vision in the
//! SwiftUI app): `mask.inputImage` shows the photo as a selection model sees it, `mask.add` /
//! `mask.setObject` take the answer. None of it needs the SAM 3 model.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use lightcraft_develop::{MaskShape, SegMask};
use serde_json::{Value, json};

use crate::Session;

/// A `w × h` coverage picture with the columns left of `edge` in, as a `bitmap` parameter.
fn bitmap(w: usize, h: usize, edge: usize) -> Value {
    let cov: Vec<u8> = (0..w * h).map(|i| if i % w < edge { 255 } else { 0 }).collect();
    json!({"width": w, "height": h, "data": STANDARD.encode(cov)})
}

/// The Object selections of the active photo's masks: (taps in, taps out, stored segmentation).
fn objects(s: &Session) -> Vec<(usize, usize, Option<SegMask>)> {
    let d = s.develop_of(s.active().unwrap()).unwrap();
    d.masks
        .iter()
        .flat_map(|m| m.components.iter())
        .filter_map(|c| match &c.shape {
            MaskShape::Object { hint, exclude, seg, .. } => Some((hint.len(), exclude.len(), seg.clone())),
            _ => None,
        })
        .collect()
}

/// Logit of `seg` at grid cell column `x` of the middle row.
fn logit_at(seg: &SegMask, x: usize) -> f32 {
    let side = seg.side as usize;
    seg.logits().unwrap()[side / 2 * side + x]
}

#[test]
fn the_input_image_is_a_png_of_the_photo_within_the_size_asked() {
    let mut s = Session::with_demo();
    let r = s.execute("mask.inputImage", &json!({"maxEdge": 200})).unwrap();
    let (w, h) = (r["width"].as_u64().unwrap(), r["height"].as_u64().unwrap());
    assert!((16..=200).contains(&w) && (16..=200).contains(&h), "{w}×{h}");
    let png = STANDARD.decode(r["png"].as_str().unwrap()).unwrap();
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    // IHDR: width and height, big endian
    assert_eq!(u64::from(u32::from_be_bytes(png[16..20].try_into().unwrap())), w);
    assert_eq!(u64::from(u32::from_be_bytes(png[20..24].try_into().unwrap())), h);
    assert_eq!(r["photo"].as_u64(), s.active().map(|id| id.0));
}

#[test]
fn a_bitmap_becomes_an_object_mask_without_any_model() {
    let mut s = Session::with_demo();
    s.execute(
        "mask.add",
        &json!({"kind": "object", "name": "Subject", "bitmap": bitmap(64, 32, 40), "points": [[0.2, 0.5]], "exclude": [[0.9, 0.5]]}),
    )
    .unwrap();
    let o = objects(&s);
    assert_eq!(o.len(), 1);
    let (taps_in, taps_out, seg) = &o[0];
    assert_eq!((*taps_in, *taps_out), (1, 1), "the taps are kept for their dots");
    let seg = seg.as_ref().unwrap();
    // 40 of 64 columns in: 62.5 % of the 512 cells across
    assert!(logit_at(seg, 10) > 5.0 && logit_at(seg, 300) > 5.0, "{} {}", logit_at(seg, 10), logit_at(seg, 300));
    assert!(logit_at(seg, 330) < -5.0 && logit_at(seg, 511) < -5.0, "{} {}", logit_at(seg, 330), logit_at(seg, 511));
}

#[test]
fn set_object_replaces_the_selection_in_one_undo_step() {
    let mut s = Session::with_demo();
    s.execute("mask.add", &json!({"kind": "object", "bitmap": bitmap(32, 32, 8), "points": [[0.1, 0.5]]})).unwrap();
    s.execute("mask.setObject", &json!({"bitmap": bitmap(32, 32, 24), "points": [[0.1, 0.5], [0.6, 0.5]], "exclude": [[0.95, 0.5]]})).unwrap();
    let o = objects(&s);
    assert_eq!(o.len(), 1, "replaced, not added");
    assert_eq!((o[0].0, o[0].1), (2, 1));
    let seg = o[0].2.clone().unwrap();
    assert!(logit_at(&seg, 300) > 5.0 && logit_at(&seg, 450) < -5.0, "the wider selection replaced the narrow one");
    s.execute("edit.undo", &json!({})).unwrap();
    let o = objects(&s);
    assert_eq!((o[0].0, o[0].1), (1, 0), "one undo step goes back to the first selection");
    assert!(logit_at(o[0].2.as_ref().unwrap(), 300) < -5.0);
}

#[test]
fn a_bad_bitmap_is_a_clear_error_and_changes_nothing() {
    let mut s = Session::with_demo();
    let short = json!({"width": 8, "height": 8, "data": STANDARD.encode([255u8; 10])});
    let e = s.execute("mask.add", &json!({"kind": "object", "bitmap": short})).unwrap_err().to_string();
    assert!(e.contains("width × height"), "{e}");
    let e = s.execute("mask.add", &json!({"kind": "object", "bitmap": {"width": 2, "height": 2, "data": "not base64 !"}})).unwrap_err().to_string();
    assert!(e.contains("base64"), "{e}");
    let e = s.execute("mask.add", &json!({"kind": "object", "bitmap": {"width": -1, "height": 2, "data": ""}})).unwrap_err().to_string();
    assert!(e.contains("width"), "{e}");
    let huge = json!({"width": 100_000, "height": 1, "data": ""});
    assert!(s.execute("mask.add", &json!({"kind": "object", "bitmap": huge})).is_err());
    assert!(objects(&s).is_empty());
    // setObject needs a bitmap, and a mask that has an Object selection
    assert!(s.execute("mask.setObject", &json!({})).unwrap_err().to_string().contains("bitmap"));
    s.execute("mask.add", &json!({"kind": "radial"})).unwrap();
    let e = s.execute("mask.setObject", &json!({"bitmap": bitmap(4, 4, 2)})).unwrap_err().to_string();
    assert!(e.contains("Object selection"), "{e}");
}

/// The active photo's spots.
fn spots(s: &Session) -> Vec<lightcraft_develop::Spot> {
    s.develop_of(s.active().unwrap()).unwrap().spots.clone()
}

#[test]
fn a_tapped_object_becomes_a_spot_with_its_shape() {
    let mut s = Session::with_demo();
    // the block of columns 16..24 (of 64) and rows 8..16 (of 32) is what was tapped
    let (w, h) = (64usize, 32usize);
    let cov: Vec<u8> = (0..w * h).map(|i| if (16..24).contains(&(i % w)) && (8..16).contains(&(i / w)) { 255 } else { 0 }).collect();
    let bitmap = json!({"width": w, "height": h, "data": STANDARD.encode(&cov)});
    let r = s.execute("spot.add", &json!({"mode": "remove", "bitmap": bitmap})).unwrap();
    assert_eq!(r["index"], 0);
    let sp = &spots(&s)[0];
    // the centre and the corners of the object's box: columns 16–24 and rows 8–16
    let near = |p: &lightcraft_geom::Point, x: f64, y: f64| (p.x - x).abs() < 1e-9 && (p.y - y).abs() < 1e-9;
    assert_eq!(sp.points.len(), 3);
    assert!(near(&sp.points[0], 20.0 / 64.0, 12.0 / 32.0), "{:?}", sp.points[0]);
    assert!(near(&sp.points[1], 16.0 / 64.0, 8.0 / 32.0) && near(&sp.points[2], 24.0 / 64.0, 16.0 / 32.0), "{:?}", sp.points);
    assert!(sp.size < 0.01, "a hair past the edge: {}", sp.size);
    let shape = sp.shape.as_ref().unwrap();
    // its grid covers the box and a margin, and says the object is in the middle
    let r = shape.rect.unwrap();
    assert!(r[0] < 0.25 && r[2] > 0.375 && r[1] < 0.25 && r[3] > 0.5, "{r:?}");
    let side = shape.side as usize;
    assert!(shape.logits().unwrap()[side / 2 * side + side / 2] > 5.0);
    // moving the spot moves its shape along
    s.execute("spot.update", &json!({"index": 0, "move": [0.1, 0.05]})).unwrap();
    let moved = spots(&s)[0].shape.as_ref().unwrap().rect.unwrap();
    assert!((moved[0] - r[0] - 0.1).abs() < 1e-9 && (moved[3] - r[3] - 0.05).abs() < 1e-9, "{moved:?} vs {r:?}");
    // one undo step per change; an object tapped in this photo is not copied to another
    let copied = lightcraft_develop::extract_groups(&s.develop_of(s.active().unwrap()).unwrap(), &[lightcraft_develop::SettingsGroup::Spots]);
    assert!(copied["spots"].as_array().unwrap().is_empty(), "{copied}");
    s.execute("edit.undo", &json!({})).unwrap();
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(spots(&s).is_empty());
}

#[test]
fn a_tap_that_selected_nothing_adds_no_spot() {
    let mut s = Session::with_demo();
    let bitmap = json!({"width": 8, "height": 8, "data": STANDARD.encode([0u8; 64])});
    let e = s.execute("spot.add", &json!({"bitmap": bitmap})).unwrap_err().to_string();
    assert!(e.contains("selects nothing"), "{e}");
    assert!(spots(&s).is_empty());
    assert!(s.execute("spot.add", &json!({})).unwrap_err().to_string().contains("missing points"));
}
