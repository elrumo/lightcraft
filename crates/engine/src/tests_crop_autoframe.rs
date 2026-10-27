//! `crop.autoFrame`: a native frontend's Vision finds the subjects and faces, the engine frames them (the ML auto crop).

use serde_json::json;

use crate::Session;

fn landscape_photo() -> Session {
    let mut s = Session::with_demo();
    let id = s.catalog.photos().find(|p| p.width > p.height && !p.develop.orientation.swaps_axes()).map(|p| p.id).unwrap();
    s.selection = crate::Selection { ids: vec![id], active: Some(id) };
    s
}

fn crop(s: &Session) -> lightcraft_geom::CropGeometry {
    s.catalog.photo(s.active().unwrap()).unwrap().develop.crop.geometry
}

#[test]
fn a_subject_is_framed_in_one_undo_step() {
    let mut s = landscape_photo();
    let before = crop(&s);
    let r = s.execute("crop.autoFrame", &json!({"regions": [[0.4, 0.3, 0.6, 0.6]]})).unwrap();
    assert_eq!(r["changed"], true, "{r}");
    let c = crop(&s);
    assert!(c.rect.width() < 0.97 && c.rect.x0 <= 0.4 && c.rect.x1 >= 0.6 && c.rect.y0 <= 0.3 && c.rect.y1 >= 0.6, "{c:?}");
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(crop(&s), before);
}

#[test]
fn a_locked_aspect_and_the_angle_are_kept() {
    let mut s = landscape_photo();
    s.execute("crop.aspect", &json!({"aspect": [1, 1]})).unwrap();
    s.execute("crop.autoFrame", &json!({"regions": [[0.45, 0.4, 0.55, 0.6]], "angle": 2.5})).unwrap();
    let c = crop(&s);
    let p = s.catalog.photo(s.active().unwrap()).unwrap();
    let (w, h) = (p.width as f64, p.height as f64);
    let r = c.rect.scale(w, h);
    assert!((r.width() / r.height() - 1.0).abs() < 0.02, "{c:?}");
    assert_eq!(c.angle, 2.5);
    assert!(c.is_within_image(w, h), "{c:?}");
}

#[test]
fn a_photo_framed_already_is_not_changed_and_bad_regions_are_an_error() {
    let mut s = landscape_photo();
    let r = s.execute("crop.autoFrame", &json!({"regions": [[0.02, 0.02, 0.98, 0.98]]})).unwrap();
    assert_eq!(r["changed"], false, "{r}");
    assert_eq!(crop(&s), lightcraft_geom::CropGeometry::default());
    assert!(s.execute("crop.autoFrame", &json!({})).is_err());
    assert!(s.execute("crop.autoFrame", &json!({"regions": [[0.1, 0.1]]})).is_err());
}
