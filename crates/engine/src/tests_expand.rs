//! Generative Expand: the crop's Expand and Generate (`crop.expand`, `crop.generate`,
//! `crop.expandModels`). Classic needs no download, so most of this runs everywhere; the model
//! paths run when a copy of MI-GAN (`$LIGHTCRAFT_MIGAN`) or LaMa (`$LIGHTCRAFT_LAMA`) is given.

use lightcraft_geom::{CropGeometry, EXPAND_MAX, Rect};
use serde_json::json;

use crate::Session;
use crate::expand::ExpandModel;

fn crop(s: &Session) -> CropGeometry {
    s.develop_of(s.active().unwrap()).unwrap().crop.geometry
}

fn dims(s: &Session) -> (f64, f64) {
    let p = s.catalog.photo(s.active().unwrap()).unwrap();
    let (w, h) = (p.width as f64, p.height as f64);
    if p.develop.orientation.swaps_axes() { (h, w) } else { (w, h) }
}

fn has_fill(s: &Session) -> bool {
    s.develop_of(s.active().unwrap()).unwrap().expand_fill.is_some()
}

#[test]
fn expand_lets_the_crop_reach_past_the_photo() {
    let mut s = Session::with_demo();
    let (w, h) = dims(&s);
    // off: a drag stops at the photo's edge
    s.execute("crop.drag", &json!({"handle": "left", "from": [0.0, 0.5], "to": [-0.3, 0.5]})).unwrap();
    assert!(crop(&s).rect.x0 >= 0.0);
    // on: the same drag goes past it, up to the limit
    assert_eq!(s.execute("crop.expand", &json!({})).unwrap()["on"], true);
    s.execute("crop.drag", &json!({"handle": "left", "from": [0.0, 0.5], "to": [-0.3, 0.5]})).unwrap();
    assert!((crop(&s).rect.x0 + 0.3).abs() < 1e-6, "{:?}", crop(&s));
    s.execute("crop.drag", &json!({"handle": "right", "from": [1.0, 0.5], "to": [9.0, 0.5]})).unwrap();
    assert!((crop(&s).rect.x1 - (1.0 + EXPAND_MAX)).abs() < 1e-6);
    assert!(crop(&s).reaches_past(w, h));
    // crop.set keeps a rect within the grown photo, and shrinks one beyond it
    s.execute("crop.set", &json!({"rect": [-0.2, -0.1, 1.1, 1.0]})).unwrap();
    assert_eq!(crop(&s).rect, Rect::new(-0.2, -0.1, 1.1, 1.0));
    s.execute("crop.set", &json!({"rect": [-5.0, 0.0, 1.0, 1.0]})).unwrap();
    assert!(crop(&s).is_within(w, h, true));
    // straightening keeps the crop (the corners it uncovers are for the fill)
    s.execute("crop.set", &json!({"rect": [0.0, 0.0, 1.0, 1.0]})).unwrap();
    s.execute("crop.straighten", &json!({"angle": 8.0})).unwrap();
    assert_eq!(crop(&s).rect, Rect::UNIT);
    // choosing an aspect adds canvas around the whole photo
    s.execute("crop.straighten", &json!({"angle": 0.0})).unwrap();
    s.execute("crop.aspect", &json!({"aspect": "1x1"})).unwrap();
    let r = crop(&s).rect_px(w, h);
    assert!((r.width() - r.height()).abs() < 1e-6 && r.width() >= w.max(h) - 1e-6, "{r:?}");
    // off: back inside the photo, in one undo step
    assert_eq!(s.execute("crop.expand", &json!({"on": false})).unwrap()["changed"], true);
    assert!(crop(&s).is_within_image(w, h));
    assert_eq!(s.execute("crop.expand", &json!({"on": false})).unwrap()["changed"], false);
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(s.develop_of(s.active().unwrap()).unwrap().crop.expand);
}

#[test]
fn classic_generate_fills_the_new_area_as_one_undo_step() {
    let mut s = Session::with_demo();
    let id = s.active().unwrap();
    // nothing to fill yet: Expand off, then nothing past the photo
    let e = s.execute("crop.generate", &json!({"model": "classic"})).unwrap_err().to_string();
    assert!(e.contains("Expand"), "{e}");
    s.execute("crop.expand", &json!({"on": true})).unwrap();
    let e = s.execute("crop.generate", &json!({"model": "classic"})).unwrap_err().to_string();
    assert!(e.contains("nothing past the photo"), "{e}");
    assert!(s.execute("crop.generate", &json!({"model": "dall-e"})).is_err());
    s.execute("crop.set", &json!({"rect": [-0.15, 0.0, 1.15, 1.0]})).unwrap();
    // before: the new area is transparent in a render
    let before = s.render_now(id, 400, 400).unwrap().image;
    assert!(before.data.iter().any(|p| p[3] == 0));
    let done = s.execute("crop.generate", &json!({"model": "classic"})).unwrap();
    assert_eq!(done["model"], "classic");
    assert!(done["width"].as_u64().unwrap() > 0 && done["storedBytes"].as_u64().unwrap() > 0, "{done}");
    assert!(has_fill(&s));
    let d = s.develop_of(id).unwrap();
    let f = d.expand_fill.as_ref().unwrap();
    assert_eq!(f.model, "classic");
    let [x0, _, x1, _] = f.fill.bounds().unwrap();
    assert!(x0 <= -0.15 && x1 >= 1.15);
    // after: nothing transparent, and the history says what happened
    let after = s.render_now(id, 400, 400).unwrap().image;
    assert!(after.data.iter().all(|p| p[3] == 255));
    assert_eq!(s.catalog.photo(id).unwrap().history.last().map(|h| h.label.as_str()), Some("Generative Expand"));
    // the fill is a picture of this photo: Copy Settings leaves it behind
    let copied = lightcraft_develop::extract_groups(&d, &lightcraft_develop::SettingsGroup::ALL);
    assert!(copied.get("expand_fill").is_none());
    // one undo step
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(!has_fill(&s));
    s.execute("edit.redo", &json!({})).unwrap();
    assert!(has_fill(&s));
    // Expand off drops it; Reset Crop too
    s.execute("crop.expand", &json!({"on": false})).unwrap();
    assert!(!has_fill(&s));
    s.execute("edit.undo", &json!({})).unwrap();
    s.execute("crop.reset", &json!({})).unwrap();
    assert!(!has_fill(&s) && !s.develop_of(id).unwrap().crop.expand);
}

#[test]
fn generate_in_the_background_shows_in_the_activity_stack() {
    let mut s = Session::with_demo();
    s.execute("crop.expand", &json!({"on": true})).unwrap();
    s.execute("crop.set", &json!({"rect": [0.0, -0.2, 1.0, 1.0]})).unwrap();
    let r = s.execute("crop.generate", &json!({"model": "classic", "wait": false})).unwrap();
    assert_eq!(r["started"], true);
    assert!(s.execute("crop.generate", &json!({"model": "classic", "wait": false})).is_err(), "one at a time");
    let models = s.execute("crop.expandModels", &json!({})).unwrap();
    assert_eq!(models["running"]["model"], "classic");
    let mut saw_row = false;
    let res = loop {
        saw_row |= s.activity.list().iter().any(|t| t.kind == "expand");
        if let Some(r) = s.poll_expand() {
            break r;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    assert_eq!(res, Ok(ExpandModel::Classic));
    assert!(saw_row, "the job had a row in the activity stack");
    assert!(s.activity.list().iter().all(|t| t.kind != "expand"), "the row is gone");
    assert!(has_fill(&s) && s.expand_running().is_none());
}

#[test]
fn a_cancelled_generate_stores_nothing() {
    let mut s = Session::with_demo();
    s.execute("crop.expand", &json!({"on": true})).unwrap();
    s.execute("crop.set", &json!({"rect": [-0.3, -0.3, 1.3, 1.3]})).unwrap();
    s.start_expand(s.active().unwrap(), Some(ExpandModel::Classic)).unwrap();
    assert!(s.cancel_expand());
    let res = loop {
        if let Some(r) = s.poll_expand() {
            break r;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    // (Classic checks once before it starts: a cancel that lands later lets it finish)
    if let Err(e) = res {
        assert!(e.contains("cancelled"), "{e}");
        assert!(!has_fill(&s));
    }
    assert!(!s.cancel_expand(), "nothing runs now");
}

#[test]
fn the_models_are_offered_per_device_and_a_missing_one_says_so() {
    let mut s = Session::with_demo();
    let list = s.execute("crop.expandModels", &json!({})).unwrap();
    let ids: Vec<&str> = list["models"].as_array().unwrap().iter().filter_map(|m| m["id"].as_str()).collect();
    assert_eq!(ids, ["migan", "lama", "classic"]);
    let classic = &list["models"][2];
    assert_eq!((classic["ready"].as_bool(), classic["default"].as_bool(), classic["downloadBytes"].as_u64()), (Some(true), Some(true), Some(0)));
    // on a computer LaMa is offered; no model is on this test's device
    assert_eq!(list["models"][1]["available"], true);
    assert_eq!(list["models"][1]["ready"], false);
    assert_eq!(s.default_expand_model(), ExpandModel::Classic);
    s.execute("crop.expand", &json!({"on": true})).unwrap();
    s.execute("crop.set", &json!({"rect": [-0.2, 0.0, 1.0, 1.0]})).unwrap();
    for m in ["migan", "lama"] {
        let e = s.execute("crop.generate", &json!({"model": m})).unwrap_err().to_string();
        assert!(e.contains("not downloaded") || e.contains("isn't available"), "{m}: {e}");
    }
    assert!(!has_fill(&s));
    // Settings → AI Models lists LaMa next to MI-GAN where the enhancement models are built in
    if crate::enhance::Enhancer::AVAILABLE {
        let ids: Vec<String> = s.models().into_iter().map(|m| m.id).collect();
        assert!(ids.iter().any(|i| i == "lama") && ids.iter().any(|i| i == "migan"), "{ids:?}");
    }
}

/// With the real models (never part of the repository): each paints the strip and stores it.
#[test]
fn the_models_paint_the_new_area() {
    let migan = std::env::var_os("LIGHTCRAFT_MIGAN");
    let lama = std::env::var_os("LIGHTCRAFT_LAMA");
    for (model, path) in [(ExpandModel::MiGan, migan), (ExpandModel::Lama, lama)] {
        let Some(path) = path else { continue };
        let mut s = Session::with_demo();
        match model {
            ExpandModel::MiGan => s.install_eraser(&std::fs::read(&path).unwrap()).unwrap(),
            _ => {
                let dir = std::env::temp_dir().join(format!("lc-expand-{}", std::process::id()));
                std::fs::create_dir_all(dir.join("lama")).unwrap();
                let file = dir.join("lama").join(lightcraft_erase::lama::FILE);
                if std::fs::hard_link(&path, &file).is_err() {
                    std::fs::copy(&path, &file).unwrap();
                }
                s.enhancer.dir = Some(dir);
            }
        }
        if !s.expand_ready(model) {
            continue;
        }
        s.execute("crop.expand", &json!({"on": true})).unwrap();
        s.execute("crop.aspect", &json!({"aspect": "1x1"})).unwrap();
        let done = s.execute("crop.generate", &json!({"model": model.id()})).unwrap();
        assert_eq!(done["model"], model.id());
        assert!(done["passes"].as_u64().unwrap() >= 1);
        let id = s.active().unwrap();
        assert!(s.render_now(id, 300, 300).unwrap().image.data.iter().all(|p| p[3] == 255));
    }
}

/// A picture of `rgb` pixels as PNG bytes.
fn png(w: usize, h: usize, f: impl Fn(usize, usize) -> [u8; 3]) -> Vec<u8> {
    let img = lightcraft_raster::Rgba8::from_fn(w, h, |x, y| {
        let p = f(x, y);
        [p[0], p[1], p[2], 255]
    });
    lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap()
}

fn b64(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn unb64(text: &str) -> Vec<u8> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.decode(text).unwrap()
}

/// A painter outside the engine (Apple Image Playground in the SwiftUI app) is given the canvas as a
/// picture and its answer becomes the fill, stored like any model's: one undo step, the photo as it is.
#[test]
fn an_outside_painter_gets_the_canvas_and_its_picture_becomes_the_fill() {
    let mut s = Session::with_demo();
    s.execute("crop.expand", &json!({})).unwrap();
    // nothing past the photo yet: no canvas
    let e = s.execute("crop.expandCanvas", &json!({})).unwrap_err().to_string();
    assert!(e.contains("nothing past the photo"), "{e}");
    s.execute("crop.set", &json!({"rect": [-0.2, 0.0, 1.0, 1.0]})).unwrap();
    let c = s.execute("crop.expandCanvas", &json!({"maxEdge": 400})).unwrap();
    let (w, h) = (c["width"].as_u64().unwrap() as usize, c["height"].as_u64().unwrap() as usize);
    assert!(w.max(h) <= 400 && w.min(h) > 16, "{w} x {h}");
    let canvas = lightcraft_codecs::decode(&unb64(c["png"].as_str().unwrap()), Default::default()).unwrap().to_srgb8();
    assert_eq!((canvas.width, canvas.height), (w, h));
    let key = c["key"].as_u64().unwrap();
    // its answer: the canvas again, bluer, at twice the size (a painter's own size)
    let answer = png(w * 2, h * 2, |x, y| {
        let p = canvas.get(x / 2, y / 2);
        [p[0] / 2, p[1], p[2].saturating_add(60)]
    });
    let done = s.execute("crop.applyExpandPicture", &json!({"key": key, "model": "appleCloud", "png": b64(&answer)})).unwrap();
    assert_eq!(done["model"], "appleCloud");
    assert_eq!(done["passes"], 0);
    let d = s.develop_of(s.active().unwrap()).unwrap();
    assert_eq!(d.expand_fill.as_ref().map(|f| f.model.as_str()), Some("appleCloud"));
    // the picture can come as a file, too
    let file = std::env::temp_dir().join(format!("lc-expand-picture-{}.png", std::process::id()));
    std::fs::write(&file, &answer).unwrap();
    s.execute("crop.applyExpandPicture", &json!({"key": key, "path": file.to_string_lossy()})).unwrap();
    let _ = std::fs::remove_file(&file);
    // each is one undo step, "Generative Expand"
    s.execute("edit.undo", &json!({})).unwrap();
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(!has_fill(&s));
}

#[test]
fn an_outside_picture_is_refused_when_it_does_not_fit() {
    let mut s = Session::with_demo();
    s.execute("crop.expand", &json!({})).unwrap();
    s.execute("crop.set", &json!({"rect": [-0.2, 0.0, 1.0, 1.0]})).unwrap();
    let c = s.execute("crop.expandCanvas", &json!({"maxEdge": 300})).unwrap();
    let (w, h, key) = (c["width"].as_u64().unwrap() as usize, c["height"].as_u64().unwrap() as usize, c["key"].as_u64().unwrap());
    let good = png(w, h, |_, _| [120, 130, 140]);
    let apply = |s: &mut Session, p: serde_json::Value| s.execute("crop.applyExpandPicture", &p).unwrap_err().to_string();
    // not a picture; a picture of another shape; a painter that isn't one
    assert!(apply(&mut s, json!({"key": key, "png": b64(b"not a picture")})).contains("couldn't be read"));
    assert!(apply(&mut s, json!({"key": key, "png": b64(&png(w / 4, h, |_, _| [1, 2, 3]))})).contains("shape"));
    assert!(apply(&mut s, json!({"key": key, "model": "migan", "png": b64(&good)})).contains("unknown painter"));
    assert!(apply(&mut s, json!({"png": b64(&good)})).contains("key"));
    assert!(apply(&mut s, json!({"key": key})).contains("path"));
    assert!(apply(&mut s, json!({"key": key, "path": "/definitely/not/here.png"})).contains("couldn't be read"));
    // the crop moved while the picture was being made
    s.execute("crop.set", &json!({"rect": [-0.3, 0.0, 1.0, 1.0]})).unwrap();
    assert!(apply(&mut s, json!({"key": key, "png": b64(&good)})).contains("crop changed"));
    assert!(!has_fill(&s));
    // Generate itself doesn't take it: the app's sheet does
    let e = s.execute("crop.generate", &json!({"model": "appleCloud"})).unwrap_err().to_string();
    assert!(e.contains("crop.applyExpandPicture"), "{e}");
}
