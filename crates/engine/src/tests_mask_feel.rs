//! Masking feel (beta feedback #24): states the Masking screen left behind that did not match what
//! the render showed, and controls it was missing.

use lightcraft_develop::{MaskOp, MaskShape};
use serde_json::json;

use crate::Session;

fn masks(s: &Session) -> Vec<lightcraft_develop::Mask> {
    s.develop_of(s.active().unwrap()).unwrap().masks.clone()
}

/// The strokes of each brush part of the first mask.
fn strokes(s: &Session) -> Vec<usize> {
    masks(s)[0]
        .components
        .iter()
        .map(|c| match &c.shape {
            MaskShape::Brush { strokes } => strokes.len(),
            _ => 99,
        })
        .collect()
}

#[test]
fn deleting_the_first_part_does_not_leave_a_subtract_first() {
    let mut s = Session::with_demo();
    s.execute("mask.add", &json!({"kind": "radial"})).unwrap();
    s.execute("mask.addComponent", &json!({"kind": "linear", "op": "subtract"})).unwrap();
    s.execute("mask.component", &json!({"component": 0, "action": "delete"})).unwrap();
    let m = &masks(&s)[0];
    assert_eq!(m.components.len(), 1);
    assert_eq!(m.components[0].op, MaskOp::Add, "the first part always adds (the render reads it so)");
    // and the first part's mode cannot be set to something else
    let e = s.execute("mask.component", &json!({"component": 0, "action": "op", "op": "intersect"}));
    assert!(e.is_err(), "the first part always adds");
    assert_eq!(masks(&s)[0].components[0].op, MaskOp::Add);
}

#[test]
fn undoing_a_new_mask_leaves_none_selected() {
    let mut s = Session::with_demo();
    s.execute("mask.add", &json!({"kind": "radial"})).unwrap();
    assert!(s.active_mask.is_some());
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(masks(&s).is_empty());
    assert_eq!(s.active_mask, None, "no ghost selection");
    // redo brings the mask back, unselected
    s.execute("edit.redo", &json!({})).unwrap();
    assert_eq!(masks(&s).len(), 1);
    assert_eq!(s.active_mask, None);
}

#[test]
fn erasing_before_painting_makes_no_mask() {
    let mut s = Session::with_demo();
    s.execute("mask.select", &json!({"id": null})).unwrap();
    s.execute("mask.brushStroke", &json!({"points": [[0.4, 0.5], [0.6, 0.5]], "erase": true})).unwrap();
    assert!(masks(&s).is_empty(), "{:?}", masks(&s).iter().map(|m| &m.name).collect::<Vec<_>>());
    assert!(s.undo.is_empty(), "nothing to undo");
    // with paint on the mask, erasing is a stroke as before
    s.execute("mask.brushStroke", &json!({"points": [[0.4, 0.5], [0.6, 0.5]]})).unwrap();
    s.execute("mask.brushStroke", &json!({"points": [[0.5, 0.5]], "erase": true})).unwrap();
    assert_eq!(strokes(&s), [2]);
}

#[test]
fn a_stroke_goes_to_the_brush_part_asked_for() {
    let mut s = Session::with_demo();
    s.execute("mask.select", &json!({"id": null})).unwrap();
    s.execute("mask.brushStroke", &json!({"points": [[0.2, 0.5], [0.3, 0.5]]})).unwrap();
    s.execute("mask.addComponent", &json!({"kind": "brush", "op": "subtract"})).unwrap();
    // without `component`, the last brush part
    s.execute("mask.brushStroke", &json!({"points": [[0.4, 0.5], [0.5, 0.5]]})).unwrap();
    assert_eq!(strokes(&s), [1, 1]);
    // the first part, when asked for
    s.execute("mask.brushStroke", &json!({"points": [[0.6, 0.5], [0.7, 0.5]], "component": 0})).unwrap();
    assert_eq!(strokes(&s), [2, 1]);
    // a part that is not a brush is an error, not a stroke somewhere else
    s.execute("mask.addComponent", &json!({"kind": "linear", "op": "intersect"})).unwrap();
    assert!(s.execute("mask.brushStroke", &json!({"points": [[0.5, 0.5]], "component": 2})).is_err());
    assert!(s.execute("mask.brushStroke", &json!({"points": [[0.5, 0.5]], "component": 9})).is_err());
}

#[test]
fn a_radial_gradient_has_a_feather_control() {
    let mut s = Session::with_demo();
    s.execute("mask.add", &json!({"kind": "radial", "feather": 50})).unwrap();
    let c = s.execute("mask.shapeControls", &json!({})).unwrap();
    let ctl = &c["components"][0]["controls"][0];
    assert_eq!((ctl["id"].as_str(), ctl["value"].as_f64()), (Some("feather"), Some(50.0)), "{c}");
    s.execute("mask.setShapeControl", &json!({"component": 0, "control": "feather", "value": 120})).unwrap();
    let MaskShape::Radial { feather, .. } = masks(&s)[0].components[0].shape.clone() else { panic!("radial") };
    assert_eq!(feather, 100.0, "clamped");
}

#[test]
fn new_gradients_go_in_the_picture_the_photo_shows() {
    let mut s = Session::with_demo();
    let id = s.active().unwrap();
    // whole photo: where they always were
    s.execute("mask.add", &json!({"kind": "radial"})).unwrap();
    s.execute("mask.add", &json!({"kind": "linear"})).unwrap();
    let whole = masks(&s);
    let MaskShape::Radial { center, rx, ry, .. } = whole[0].components[0].shape.clone() else { panic!("radial") };
    assert!([center.x - 0.5, center.y - 0.5, rx - 0.22, ry - 0.16].iter().all(|d| d.abs() < 1e-9), "{center:?} {rx} {ry}");
    let MaskShape::Linear { start, end } = whole[1].components[0].shape.clone() else { panic!("linear") };
    assert!([start.x - 0.5, start.y - 0.25, end.x - 0.5, end.y - 0.6].iter().all(|d| d.abs() < 1e-9), "{start:?} {end:?}");
    s.execute("mask.deleteAll", &json!({})).unwrap();
    // cropped to the lower right quarter: in the middle of that, half the size
    let mut d = (*s.develop_of(id).unwrap()).clone();
    d.crop.geometry.rect = lightcraft_geom::Rect::from_center(lightcraft_geom::Point::new(0.75, 0.75), 0.5, 0.5);
    s.set_develop(id, d, "Crop").unwrap();
    s.execute("mask.add", &json!({"kind": "radial"})).unwrap();
    s.execute("mask.add", &json!({"kind": "linear"})).unwrap();
    let cropped = masks(&s);
    let MaskShape::Radial { center, rx, ry, .. } = cropped[0].components[0].shape.clone() else { panic!("radial") };
    assert!((center.x - 0.75).abs() < 1e-6 && (center.y - 0.75).abs() < 1e-6, "{center:?}");
    assert!((rx - 0.11).abs() < 1e-6 && (ry - 0.08).abs() < 1e-6, "{rx} {ry}");
    let MaskShape::Linear { start, end } = cropped[1].components[0].shape.clone() else { panic!("linear") };
    assert!((start.x - 0.75).abs() < 1e-6 && (start.y - 0.625).abs() < 1e-6, "{start:?}");
    assert!((end.x - 0.75).abs() < 1e-6 && (end.y - 0.8).abs() < 1e-6, "{end:?}");
    // a caller that places it is obeyed
    s.execute("mask.add", &json!({"kind": "radial", "center": [0.1, 0.2], "rx": 0.3})).unwrap();
    let MaskShape::Radial { center, rx, .. } = masks(&s)[2].components[0].shape.clone() else { panic!("radial") };
    assert_eq!((center.x, center.y, rx), (0.1, 0.2, 0.3));
}

#[test]
fn a_mask_can_start_inverted_for_select_background() {
    use base64::Engine as _;
    let mut s = Session::with_demo();
    // the left half selected, on an 8 × 8 coverage picture
    let cov: Vec<u8> = (0..64).map(|i| if i % 8 < 4 { 255 } else { 0 }).collect();
    let bitmap = json!({"width": 8, "height": 8, "data": base64::engine::general_purpose::STANDARD.encode(cov)});
    s.execute("mask.add", &json!({"kind": "object", "bitmap": bitmap, "invert": true})).unwrap();
    s.execute("mask.add", &json!({"kind": "object", "bitmap": bitmap})).unwrap();
    let m = masks(&s);
    assert_eq!((m[0].components[0].invert, m[1].components[0].invert), (true, false));
    let (w, h) = (40, 20);
    let frame = lightcraft_pipeline::geometry::Frame::new(w, h, &lightcraft_develop::DevelopSettings::default(), true);
    let (img, l) = (lightcraft_raster::Rgb32f::new(w, h), lightcraft_raster::Plane::new(w, h));
    let alpha = |mask: &lightcraft_develop::Mask| lightcraft_pipeline::masks::evaluate_one(mask, &frame, w, h, &img, &l, 0.0, None);
    let (back, subject) = (alpha(&m[0]), alpha(&m[1]));
    assert!(subject.get(5, 10) > 0.99 && subject.get(35, 10) < 0.01);
    assert!(back.get(5, 10) < 0.01 && back.get(35, 10) > 0.99, "the background is what the subject is not");
}
