//! Double exposure (card #452) end to end: made from a selection as a virtual copy of the base, its layers edited as
//! undo steps, rendered over the base's picture, and a layer whose photo is gone named as such while the rest draws.

use serde_json::json;

use crate::{PhotoId, Session};

fn select(s: &mut Session, ids: &[PhotoId], active: PhotoId) {
    s.execute("library.select", &json!({"ids": ids.iter().map(|i| i.0).collect::<Vec<_>>()})).unwrap();
    s.selection.active = Some(active);
}

fn mean(s: &mut Session, id: PhotoId) -> f64 {
    let r = s.render_now(id, 64, 64).unwrap();
    r.image.data.iter().map(|p| f64::from(p[0]) + f64::from(p[1]) + f64::from(p[2])).sum::<f64>() / r.image.data.len() as f64
}

#[test]
fn a_double_exposure_is_a_virtual_copy_with_the_others_as_layers() {
    let mut s = Session::with_demo();
    let ids: Vec<PhotoId> = s.catalog.photos().map(|p| p.id).take(3).collect();
    let (base, others) = (ids[1], [ids[0], ids[2]]);
    let photos = s.catalog.len();
    select(&mut s, &ids, base);
    let r = s.execute("photo.doubleExposure", &json!({"name": "Doppelbelichtung"})).unwrap();
    let copy = PhotoId(r["id"].as_u64().unwrap());
    assert_eq!(r["base"].as_u64(), Some(base.0));
    let p = s.catalog.photo(copy).unwrap().clone();
    assert_eq!((p.copy_of, p.copy_name.as_deref()), (Some(base), Some("Doppelbelichtung")));
    let layered: Vec<u64> = p.develop.exposure_layers.iter().map(|l| l.photo).collect();
    assert_eq!(layered, [others[0].0, others[1].0]);
    assert_eq!(s.active(), Some(copy));
    assert!(p.is_edited(), "a double exposure counts as edited");
    // no pixels written: the base is as it was, and the copy shares its file
    assert!(s.catalog.photo(base).unwrap().develop.exposure_layers.is_empty());

    // screened layers lighten the base; hidden, the copy draws as the base does
    let (b, c) = (mean(&mut s, base), mean(&mut s, copy));
    assert!(c > b + 1.0, "screened layers lighten: base {b}, double {c}");
    for i in 0..2 {
        s.execute("layer.visible", &json!({"layer": i, "visible": false})).unwrap();
    }
    assert_eq!(s.render_now(copy, 64, 64).unwrap().image.data, s.render_now(base, 64, 64).unwrap().image.data);
    s.execute("layer.visible", &json!({"layer": 0})).unwrap();
    s.execute("layer.blend", &json!({"layer": 0, "blend": "multiply"})).unwrap();
    assert!(mean(&mut s, copy) < b - 1.0, "a multiplied layer darkens");
    // the sliders are indexed controls, kept in range
    s.execute("develop.set", &json!({"control": "exposureLayer.0.scale", "value": 9000})).unwrap();
    s.execute("develop.set", &json!({"control": "exposureLayer.0.x", "value": 30})).unwrap();
    let l = s.develop_of(copy).unwrap().exposure_layers[0].clone();
    assert_eq!((l.scale, l.x), (400.0, 30.0));
    s.execute("layer.move", &json!({"layer": 0, "to": 5})).unwrap();
    assert_eq!(s.develop_of(copy).unwrap().exposure_layers[1].photo, others[0].0);
    s.execute("layer.delete", &json!({"layer": 1})).unwrap();
    assert_eq!(s.develop_of(copy).unwrap().exposure_layers.len(), 1);

    // the whole double exposure goes with one undo per step, its making with one
    for _ in 0..8 {
        s.execute("edit.undo", &json!({})).unwrap();
    }
    assert_eq!(s.develop_of(copy).unwrap().exposure_layers.len(), 2);
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(s.catalog.photo(copy).is_none_or(|p| p.deleted), "one undo takes the copy away");
    assert_eq!(s.catalog.photos().filter(|p| !p.deleted).count(), photos);
}

#[test]
fn a_layer_whose_photo_is_gone_is_named_and_the_rest_draws() {
    let mut s = Session::with_demo();
    let ids: Vec<PhotoId> = s.catalog.photos().map(|p| p.id).take(2).collect();
    select(&mut s, &ids, ids[0]);
    let copy = PhotoId(s.execute("photo.doubleExposure", &json!({})).unwrap()["id"].as_u64().unwrap());
    let plain = s.render_now(ids[0], 48, 48).unwrap().image.data;
    // a layer of a photo that is not in the library any more (from another device, or removed)
    let mut d = (*s.develop_of(copy).unwrap()).clone();
    d.exposure_layers[0].photo = 987_654;
    s.set_develop(copy, d, "test").unwrap();
    let list = s.execute("layer.list", &json!({})).unwrap();
    assert_eq!(list["layers"][0]["state"], "missing");
    assert_eq!(list["layers"][0]["drawn"], false);
    assert_eq!(s.render_now(copy, 48, 48).unwrap().image.data, plain, "the base draws without it");
    // the person can put another photo in its place, or remove it
    s.execute("layer.replace", &json!({"layer": 0, "photo": ids[1].0})).unwrap();
    assert_eq!(s.execute("layer.list", &json!({})).unwrap()["layers"][0]["state"], "ok");

    // what can't be a layer is refused with a reason, never a panic
    for bad in [json!({"photo": 987_654}), json!({"photo": copy.0}), json!({"photo": "x"}), json!({})] {
        assert!(s.execute("layer.add", &bad).is_err(), "{bad}");
    }
    assert!(s.execute("layer.delete", &json!({"layer": 9})).is_err());
    assert!(s.execute("layer.blend", &json!({"layer": 0, "blend": "plasma"})).is_err());
    select(&mut s, &ids[..1], ids[0]);
    assert!(s.execute("photo.doubleExposure", &json!({})).is_err(), "one photo is not a double exposure");
    // the photos a frontend names are taken whatever the engine's selection is (its grid's selection is its own)
    let named = s.execute("photo.doubleExposure", &json!({"ids": [ids[1].0, ids[0].0], "base": ids[1].0})).unwrap();
    assert_eq!(named["base"].as_u64(), Some(ids[1].0));
    let list = s.execute("layer.list", &json!({"id": named["id"]})).unwrap();
    assert_eq!(list["layers"][0]["photo"].as_u64(), Some(ids[0].0));
}

#[test]
fn copy_settings_presets_and_auto_sync_leave_the_layers() {
    let mut d = lightcraft_develop::DevelopSettings::default();
    assert!(d.to_json().get("exposure_layers").is_none(), "left out while there are none");
    d.exposure_layers.push(lightcraft_develop::ExposureLayer::new(3));
    let back = lightcraft_develop::DevelopSettings::from_json(&d.to_json()).unwrap();
    assert_eq!(back.exposure_layers, d.exposure_layers);
    let all = lightcraft_develop::extract_groups(&d, &lightcraft_develop::SettingsGroup::ALL);
    assert!(all.get("exposure_layers").is_none(), "no copyable group carries them");
}

#[test]
fn text_layers_and_behind_are_stored_listed_and_undone() {
    let mut s = Session::with_demo();
    let id = s.catalog.photos().map(|p| p.id).next().unwrap();
    s.execute("library.select", &json!({"ids": [id.0]})).unwrap();
    let plain = s.render_now(id, 48, 48).unwrap().image.data;
    let photos = s.catalog.len();

    // a text layer on the photo itself: no copy made, drawn by a native frontend (this core leaves it out)
    s.execute("layer.addText", &json!({"text": "COVER", "font": "serif", "color": [1.0, 0.2, 9.0]})).unwrap();
    assert_eq!(s.catalog.len(), photos);
    let l = s.develop_of(id).unwrap().exposure_layers[0].clone();
    let t = l.text.clone().unwrap();
    assert_eq!((t.text.as_str(), t.font.as_str(), t.bold, t.color), ("COVER", "serif", true, [1.0, 0.2, 1.0]));
    assert_eq!((l.photo, l.scale, l.blend), (0, 50.0, lightcraft_develop::LayerBlend::Normal));
    assert_eq!(s.render_now(id, 48, 48).unwrap().image.data, plain, "the core draws no text");

    let list = s.execute("layer.list", &json!({})).unwrap();
    assert_eq!((list["layers"][0]["kind"].as_str(), list["layers"][0]["state"].as_str()), (Some("text"), Some("ok")));
    assert_eq!(list["layers"][0]["name"], "COVER");

    // edits keep what they leave out; a photo layer is not text
    s.execute("layer.text", &json!({"layer": 0, "text": "Line one\nLine two", "bold": false})).unwrap();
    let t = s.develop_of(id).unwrap().exposure_layers[0].text.clone().unwrap();
    assert_eq!((t.text.as_str(), t.font.as_str(), t.bold), ("Line one\nLine two", "serif", false));
    assert_eq!(s.execute("layer.list", &json!({})).unwrap()["layers"][0]["name"], "Line one");
    assert!(s.execute("layer.text", &json!({"layer": 0, "font": "Comic"})).is_err());
    assert!(s.execute("layer.text", &json!({"layer": 0, "color": [1, "x", 0]})).is_err());
    assert!(s.execute("layer.text", &json!({"layer": 3, "text": "x"})).is_err());
    let long = "é".repeat(lightcraft_develop::MAX_LAYER_TEXT + 40);
    s.execute("layer.text", &json!({"layer": 0, "text": long})).unwrap();
    assert_eq!(s.develop_of(id).unwrap().exposure_layers[0].text.as_ref().unwrap().text.chars().count(), lightcraft_develop::MAX_LAYER_TEXT);

    s.execute("layer.behind", &json!({"layer": 0})).unwrap();
    assert!(s.develop_of(id).unwrap().exposure_layers[0].behind);
    s.execute("layer.behind", &json!({"layer": 0, "behind": false})).unwrap();
    assert!(!s.develop_of(id).unwrap().exposure_layers[0].behind);
    // its sliders are the layers' indexed controls
    s.execute("develop.set", &json!({"control": "exposureLayer.0.angle", "value": -12})).unwrap();
    assert_eq!(s.develop_of(id).unwrap().exposure_layers[0].angle, -12.0);

    // a photo layer given to a text layer makes it a photo layer
    let other = s.catalog.photos().map(|p| p.id).nth(1).unwrap();
    s.execute("layer.replace", &json!({"layer": 0, "photo": other.0})).unwrap();
    let l = s.develop_of(id).unwrap().exposure_layers[0].clone();
    assert_eq!((l.photo, l.text.is_none()), (other.0, true));
    assert!(s.execute("layer.text", &json!({"layer": 0, "text": "x"})).is_err(), "a photo layer is not text");

    // one undo per step, back to no layers
    for _ in 0..7 {
        s.execute("edit.undo", &json!({})).unwrap();
    }
    assert!(s.develop_of(id).unwrap().exposure_layers.is_empty());
}

#[test]
fn hostile_layer_json_reads_without_a_panic() {
    // (a photo layer's JSON is as before: a double exposure keeps its hash and cached previews)
    let v = serde_json::to_value(lightcraft_develop::ExposureLayer::new(3)).unwrap();
    assert!(v.get("text").is_none() && v.get("behind").is_none(), "{v}");
    // (from a file or another device: wrong types, huge or broken numbers, an unknown font)
    let d: lightcraft_develop::DevelopSettings = serde_json::from_value(json!({"exposure_layers": [
        {"photo": 0, "text": {"text": "x".repeat(5000), "font": "Papyrus", "color": [f64::MAX, -3, 0.5]}, "behind": true, "scale": 1e308},
        {"text": null, "behind": "yes"},
    ]}))
    .unwrap_or_default();
    for l in &d.exposure_layers {
        let _ = lightcraft_pipeline::layers::draws(l);
        if let Some(t) = &l.text {
            let t = t.sane();
            assert!(t.text.chars().count() <= lightcraft_develop::MAX_LAYER_TEXT && t.font == "system");
            assert!(t.color.iter().all(|c| (0.0..=1.0).contains(c)));
        }
    }
    let mut s = Session::with_demo();
    let id = s.catalog.photos().map(|p| p.id).next().unwrap();
    s.set_develop(id, d, "test").unwrap();
    s.render_now(id, 32, 32).unwrap();
    s.execute("layer.list", &json!({"id": id.0})).unwrap();
}

#[test]
fn shape_and_drawing_layers_are_placed_listed_duplicated_and_undone() {
    let mut s = Session::with_demo();
    let ids: Vec<PhotoId> = s.catalog.photos().map(|p| p.id).take(6).collect();
    let id = ids[0];
    s.execute("library.select", &json!({"ids": [id.0]})).unwrap();
    let plain = s.render_now(id, 48, 48).unwrap().image.data;

    // a shape placed where it was drawn: kind, colour, fill, stroke, box shape, corner, all in range
    s.execute("layer.addShape", &json!({"kind": "ellipse", "color": [0.2, 0.4, 2.0], "fill": false, "stroke": 900, "aspect": 2, "x": -30, "y": 250, "scale": 40, "angle": 15})).unwrap();
    let l = s.develop_of(id).unwrap().exposure_layers[0].clone();
    let sh = l.shape.clone().unwrap();
    assert_eq!((sh.kind.as_str(), sh.color, sh.fill, sh.stroke, sh.aspect), ("ellipse", [0.2, 0.4, 1.0], false, 50.0, 2.0));
    assert_eq!((l.x, l.y, l.scale, l.angle, l.photo, l.kind()), (-30.0, 100.0, 40.0, 15.0, 0, "shape"));
    s.execute("layer.shape", &json!({"layer": 0, "kind": "arrow", "corner": 12})).unwrap();
    let sh = s.develop_of(id).unwrap().exposure_layers[0].shape.clone().unwrap();
    assert_eq!((sh.kind.as_str(), sh.fill, sh.corner), ("arrow", false, 12.0));
    assert!(s.execute("layer.shape", &json!({"layer": 0, "kind": "star"})).is_err());
    assert!(s.execute("layer.addShape", &json!({"scale": "big"})).is_err());

    // a drawing keeps its PencilKit data as given, and refuses what is not base64
    s.execute("layer.addDrawing", &json!({"pencil": "d3Jk", "aspect": 1.5})).unwrap();
    let d = s.develop_of(id).unwrap().exposure_layers[1].drawing.clone().unwrap();
    assert_eq!((d.pencil.as_str(), d.aspect), ("d3Jk", 1.5));
    s.execute("layer.drawing", &json!({"layer": 1, "pencil": "bW9yZQ=="})).unwrap();
    assert_eq!(s.develop_of(id).unwrap().exposure_layers[1].drawing.as_ref().unwrap().pencil, "bW9yZQ==");
    assert!(s.execute("layer.drawing", &json!({"layer": 1, "pencil": "not base64!"})).is_err());
    assert!(s.execute("layer.drawing", &json!({"layer": 0, "pencil": ""})).is_err(), "a shape is not a drawing");
    assert_eq!(s.render_now(id, 48, 48).unwrap().image.data, plain, "the core draws no shapes or drawings");

    let list = s.execute("layer.list", &json!({})).unwrap();
    let kinds: Vec<&str> = list["layers"].as_array().unwrap().iter().filter_map(|l| l["kind"].as_str()).collect();
    assert_eq!(kinds, ["shape", "drawing"]);
    assert_eq!((list["layers"][0]["name"].as_str(), list["layers"][1]["name"].as_str()), (Some("arrow"), Some("drawing")));

    // a duplicate goes just above, moved a little
    s.execute("layer.duplicate", &json!({"layer": 0})).unwrap();
    let layers = s.develop_of(id).unwrap().exposure_layers.clone();
    assert_eq!((layers.len(), layers[1].kind(), layers[1].x), (3, "shape", -26.0));

    // a picked image is a photo layer placed and mixed as asked
    s.execute("layer.add", &json!({"photo": ids[1].0, "blend": "normal", "scale": 50, "x": 10})).unwrap();
    let l = s.develop_of(id).unwrap().exposure_layers[3].clone();
    assert_eq!((l.kind(), l.blend, l.scale, l.x), ("photo", lightcraft_develop::LayerBlend::Normal, 50.0, 10.0));

    // at most four photo layers, among more layers of other kinds
    for other in &ids[2..5] {
        s.execute("layer.add", &json!({"photo": other.0})).unwrap();
    }
    assert!(s.execute("layer.add", &json!({"photo": ids[5].0})).is_err(), "a fifth photo layer");
    s.execute("layer.addText", &json!({"text": "still room", "x": 20})).unwrap();
    assert_eq!(s.develop_of(id).unwrap().exposure_layers.last().unwrap().x, 20.0);
    while s.develop_of(id).unwrap().exposure_layers.len() < lightcraft_develop::MAX_LAYERS {
        s.execute("layer.addShape", &json!({})).unwrap();
    }
    assert!(s.execute("layer.addShape", &json!({})).is_err(), "past the most layers a photo holds");

    // one undo per step, back to no layers
    while !s.develop_of(id).unwrap().exposure_layers.is_empty() {
        s.execute("edit.undo", &json!({})).unwrap();
    }
}

#[test]
fn hostile_shape_and_drawing_json_reads_without_a_panic() {
    let v = serde_json::to_value(lightcraft_develop::ExposureLayer::new(3)).unwrap();
    assert!(v.get("shape").is_none() && v.get("drawing").is_none(), "{v}");
    let d: lightcraft_develop::DevelopSettings = serde_json::from_value(json!({"exposure_layers": [
        {"shape": {"kind": "hexagon", "color": [f64::MAX, -3, 0.5], "stroke": -1, "aspect": 0, "corner": 1e300}, "scale": 1e308},
        {"drawing": {"pencil": "<script>", "aspect": -4}},
        {"drawing": {"pencil": "A".repeat(lightcraft_develop::MAX_LAYER_DRAWING + 4)}},
    ]}))
    .unwrap_or_default();
    for l in &d.exposure_layers {
        assert!(!lightcraft_pipeline::layers::draws(l), "the core draws none of them");
        if let Some(sh) = &l.shape {
            let sh = sh.sane();
            assert_eq!(sh.kind, "rectangle");
            assert!(sh.color.iter().all(|c| (0.0..=1.0).contains(c)) && (0.5..=50.0).contains(&sh.stroke) && (0.05..=20.0).contains(&sh.aspect));
        }
        if let Some(dr) = &l.drawing {
            let dr = dr.sane();
            assert!(dr.pencil.is_empty() && (0.05..=20.0).contains(&dr.aspect));
        }
    }
    let mut s = Session::with_demo();
    let id = s.catalog.photos().map(|p| p.id).next().unwrap();
    s.set_develop(id, d, "test").unwrap();
    s.render_now(id, 32, 32).unwrap();
    s.execute("layer.list", &json!({"id": id.0})).unwrap();
}

#[test]
fn a_text_layers_type_options_are_set_checked_and_kept() {
    let mut s = Session::with_demo();
    let id = s.catalog.photos().map(|p| p.id).next().unwrap();
    s.execute("library.select", &json!({"ids": [id.0]})).unwrap();
    s.execute("layer.addText", &json!({"text": "Caption", "family": "Helvetica Neue", "weight": "light", "italic": true, "align": "left"})).unwrap();
    let t = s.develop_of(id).unwrap().exposure_layers[0].text.clone().unwrap();
    assert_eq!((t.family.as_str(), t.weight.as_str(), t.italic, t.align.as_str()), ("Helvetica Neue", "light", true, "left"));
    // the rest kept as it was, the numbers brought into range
    s.execute("layer.text", &json!({"layer": 0, "underline": true, "strike": true, "tracking": 5000, "leading": 0.1})).unwrap();
    let t = s.develop_of(id).unwrap().exposure_layers[0].text.clone().unwrap();
    assert_eq!((t.family.as_str(), t.underline, t.strike, t.tracking, t.leading), ("Helvetica Neue", true, true, 800.0, 0.5));
    assert!(s.execute("layer.text", &json!({"layer": 0, "weight": "extraBold"})).is_err());
    assert!(s.execute("layer.text", &json!({"layer": 0, "align": "middle"})).is_err());
    assert!(s.execute("layer.text", &json!({"layer": 0, "italic": "yes"})).is_err());
    // back to the system font, a family name cut to its most
    s.execute("layer.text", &json!({"layer": 0, "family": "x".repeat(400)})).unwrap();
    assert_eq!(s.develop_of(id).unwrap().exposure_layers[0].text.as_ref().unwrap().family.chars().count(), lightcraft_develop::MAX_LAYER_FAMILY);
    s.execute("layer.text", &json!({"layer": 0, "family": ""})).unwrap();
    assert!(s.develop_of(id).unwrap().exposure_layers[0].text.as_ref().unwrap().family.is_empty());
    // from a file or another device: hostile values read and are brought into range
    let d: lightcraft_develop::DevelopSettings = serde_json::from_value(json!({"exposure_layers": [
        {"text": {"text": "x", "family": "A\u{0}B", "weight": "mega", "align": "diagonal", "tracking": f64::MAX, "leading": -1}},
    ]}))
    .unwrap_or_default();
    let t = d.exposure_layers[0].text.as_ref().unwrap().sane();
    assert_eq!((t.family.as_str(), t.weight.as_str(), t.align.as_str(), t.tracking, t.leading), ("AB", "", "center", 800.0, 0.5));
}
