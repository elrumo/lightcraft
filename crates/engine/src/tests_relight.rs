//! Relight and Depth Map commands: without depth or the model they say so and change nothing; a photo's own depth
//! is used as it is, a stored estimate is shared with Lens Blur; colours, palettes and the light's place are undo
//! steps; the light's drag is one; a copy leaves the depth behind.

use std::path::{Path, PathBuf};

use lightcraft_develop::{DepthMap, DevelopSettings};
use serde_json::{Value, json};

use crate::Session;
use crate::catalog::PhotoId;

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-relight-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn jpeg(w: usize, h: usize, px: impl Fn(usize, usize) -> [u8; 3], xmp: Option<&str>) -> Vec<u8> {
    let data: Vec<[u8; 4]> = (0..w * h).map(|i| px(i % w, i / w)).map(|[r, g, b]| [r, g, b, 255]).collect();
    let img = lightcraft_raster::Rgba8 { width: w, height: h, data };
    let meta = lightcraft_codecs::EncodeMeta { xmp, ..Default::default() };
    lightcraft_codecs::encode_jpeg(&lightcraft_codecs::EncodeImage::rgba8(&img), 95, Default::default(), &meta).unwrap()
}

/// A Portrait JPEG (the photo, an MPF index, a disparity JPEG marked by `apdi` XMP): the left half is near, the right
/// half far.
fn write_portrait(path: &Path) {
    let mut photo = jpeg(96, 64, |_, _| [120, 120, 120], None);
    let mpf = [&[0xFF, 0xE2, 0x00, 0x0A][..], b"MPF\0MM\0\0"].concat();
    photo.splice(2..2, mpf);
    let xmp = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description xmlns:apdi="http://ns.apple.com/pixeldatainfo/1.0/"><apdi:AuxiliaryImageType>disparity</apdi:AuxiliaryImageType><apdi:FloatMinValue>0.25</apdi:FloatMinValue><apdi:FloatMaxValue>1.0</apdi:FloatMaxValue><apdi:IntMinValue>0</apdi:IntMinValue><apdi:IntMaxValue>255</apdi:IntMaxValue></rdf:Description></rdf:RDF></x:xmpmeta>"#;
    let depth = jpeg(48, 32, |x, _| if x < 24 { [255; 3] } else { [0; 3] }, Some(xmp));
    std::fs::write(path, [photo, depth].concat()).unwrap();
}

fn open(s: &mut Session, path: &Path) -> PhotoId {
    let r = s.execute("library.import", &json!({"paths": [path.to_string_lossy()]})).unwrap();
    let id = r["imported"][0].as_u64().unwrap_or_else(|| panic!("{r}"));
    s.execute("library.select", &json!({"ids": [id], "active": id})).unwrap();
    PhotoId(id)
}

fn dev(s: &Session) -> DevelopSettings {
    (*s.develop_of(s.active().unwrap()).unwrap()).clone()
}

/// A demo photo with a stored depth estimate (near at the bottom, far at the top), as Lens Blur leaves one.
fn demo_with_depth() -> Session {
    let mut s = Session::with_demo();
    let mut d = dev(&s);
    let v: Vec<f32> = (0..32 * 24).map(|i| 0.1 + 0.8 * (i / 32) as f32 / 23.0).collect();
    d.lens_blur.depth = DepthMap::from_nearness(32, 24, &v);
    s.execute("develop.merge", &json!({"settings": d.to_json()})).unwrap();
    s
}

fn luma(p: [u8; 4]) -> f64 {
    0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2])
}

fn half_lumas(img: &lightcraft_raster::Rgba8) -> (f64, f64) {
    let (mut l, mut r) = (0.0, 0.0);
    for y in 0..img.height {
        for x in 0..img.width {
            let v = luma(img.data[y * img.width + x]);
            if x < img.width / 2 { l += v } else { r += v }
        }
    }
    let n = (img.width / 2 * img.height) as f64;
    (l / n, r / n)
}

#[test]
fn without_depth_or_model_turning_on_says_so_and_changes_nothing() {
    let mut s = Session::with_demo();
    let before = dev(&s);
    for c in ["relight.apply", "depthMap.apply"] {
        let e = s.execute(c, &json!({})).unwrap_err().to_string();
        assert!(e.contains(crate::depth::NOT_INSTALLED), "{c}: {e}");
        assert_eq!(dev(&s), before, "{c}");
    }
    let i = s.execute("relight.info", &json!({})).unwrap();
    assert_eq!((i["enabled"].as_bool(), i["depth"].is_null(), i["modelReady"].as_bool()), (Some(false), true, Some(false)), "{i}");
    let i = s.execute("depthMap.info", &json!({})).unwrap();
    assert_eq!(i["palettes"].as_array().map(Vec::len), Some(6), "{i}");
    assert_eq!(i["palettes"][0]["id"], "gray");
    // turning them off needs no depth
    s.execute("relight.apply", &json!({"on": false})).unwrap();
    s.execute("depthMap.apply", &json!({"on": false})).unwrap();
}

#[test]
fn a_portraits_own_depth_lights_and_draws_without_the_model() {
    let dir = temp_dir("portrait");
    let path = dir.join("portrait.jpg");
    write_portrait(&path);
    let mut s = Session::new().with_fs();
    let id = open(&mut s, &path);
    let plain = s.render_now(id, 96, 64).unwrap().image;
    let i = s.execute("depthMap.apply", &json!({})).unwrap();
    assert_eq!((i["depth"].as_str(), i["enabled"].as_bool()), (Some("photo"), Some(true)), "{i}");
    assert!(dev(&s).lens_blur.depth.is_none(), "the file's depth isn't copied into the settings");
    let drawn = s.render_now(id, 96, 64).unwrap().image;
    let (near, far) = half_lumas(&drawn);
    assert!(near > far + 100.0, "near (left) {near} vs far (right) {far}");
    s.execute("depthMap.apply", &json!({"on": false})).unwrap();
    assert_eq!(s.render_now(id, 96, 64).unwrap().image, plain);
    // a light on the right, close to the far side
    s.execute("relight.apply", &json!({})).unwrap();
    s.execute("develop.set", &json!({"values": {"relight.x": 1.2, "relight.y": 0.5, "relight.distance": 40.0, "relight.shadows": 0.0}})).unwrap();
    let lit = s.render_now(id, 96, 64).unwrap().image;
    assert_ne!(lit, plain);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_stored_estimate_is_shared_and_every_change_is_one_undo_step() {
    let mut s = demo_with_depth();
    let a = s.active().unwrap();
    let plain = s.render_now(a, 160, 120).unwrap().image;
    let i = s.execute("relight.apply", &json!({})).unwrap();
    assert_eq!((i["depth"].as_str(), i["enabled"].as_bool()), (Some("estimated"), Some(true)), "{i}");
    assert_ne!(s.render_now(a, 160, 120).unwrap().image, plain, "the light shows");
    s.execute("relight.color", &json!({"color": "#ff8040"})).unwrap();
    assert_eq!(dev(&s).relight.color, [1.0, 128.0 / 255.0, 64.0 / 255.0]);
    assert!(s.execute("relight.color", &json!({"color": [1, 2]})).is_err());
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(dev(&s).relight.color, [1.0; 3], "one undo step");
    // the light dragged across the photo: one step
    s.execute("develop.beginInteraction", &json!({"label": "Light"})).unwrap();
    for x in [0.4, 0.5, 0.6] {
        s.execute("develop.set", &json!({"values": {"relight.x": x, "relight.y": 0.7}})).unwrap();
    }
    s.execute("develop.endInteraction", &json!({})).unwrap();
    assert_eq!((dev(&s).relight.x, dev(&s).relight.y), (0.6, 0.7));
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!((dev(&s).relight.x, dev(&s).relight.y), (0.3, 0.25), "the drag is one step");
    // a place far past the frame is held to the control's range
    s.execute("develop.set", &json!({"control": "relight.x", "value": 9.0})).unwrap();
    assert_eq!(dev(&s).relight.x, 1.5);
    s.execute("relight.reset", &json!({})).unwrap();
    assert_eq!(dev(&s).relight, lightcraft_develop::Relight { enabled: true, ..Default::default() });
    // Depth Map shares the depth, and its palette, colours and invert are steps of their own
    s.execute("depthMap.apply", &json!({})).unwrap();
    assert!(s.execute("depthMap.palette", &json!({"palette": "aurora"})).is_err());
    let i = s.execute("depthMap.palette", &json!({"palette": "thermal"})).unwrap();
    assert_eq!(i["palette"], "thermal");
    let i = s.execute("depthMap.colors", &json!({"near": [1.0, 0.0, 0.0]})).unwrap();
    assert_eq!((i["palette"].as_str(), i["nearColor"].clone()), (Some("custom"), json!([1.0, 0.0, 0.0])), "{i}");
    assert_eq!(i["palettes"][5]["stops"][1], json!([1.0, 0.0, 0.0]), "the custom swatch shows the colours");
    assert!(s.execute("depthMap.colors", &json!({})).is_err());
    s.execute("depthMap.invert", &json!({})).unwrap();
    assert!(dev(&s).depth_map.invert);
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(!dev(&s).depth_map.invert);
    s.execute("depthMap.reset", &json!({})).unwrap();
    assert_eq!(dev(&s).depth_map, lightcraft_develop::DepthLook { enabled: true, ..Default::default() });
    // the eye of a section turns it off in the render
    s.execute("depthMap.apply", &json!({"on": false})).unwrap();
    let lit = s.render_now(a, 160, 120).unwrap().image;
    let mut d = dev(&s);
    d.set_section_enabled("relight", false);
    s.execute("develop.merge", &json!({"settings": d.to_json()})).unwrap();
    assert_eq!(s.render_now(a, 160, 120).unwrap().image, plain);
    assert_ne!(lit, plain);
}

#[test]
fn a_copy_of_the_settings_leaves_the_depth_behind() {
    let mut s = demo_with_depth();
    s.execute("relight.apply", &json!({})).unwrap();
    s.execute("depthMap.apply", &json!({})).unwrap();
    let d = dev(&s);
    let groups =
        [lightcraft_develop::SettingsGroup::Relight, lightcraft_develop::SettingsGroup::DepthMap, lightcraft_develop::SettingsGroup::LensBlur];
    let v: Value = lightcraft_develop::extract_groups(&d, &groups);
    assert_eq!(v["relight"]["enabled"], true);
    assert_eq!(v["depth_map"]["enabled"], true);
    assert!(v["lens_blur"].get("depth").is_none(), "{v}");
    assert!(lightcraft_develop::SettingsGroup::default_copy().contains(&lightcraft_develop::SettingsGroup::Relight));
}
