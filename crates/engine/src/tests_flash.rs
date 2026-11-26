//! Flash commands: without depth or the model they say so and change nothing; a photo's own depth is used as it is, a
//! stored estimate is shared with Lens Blur; a preset and the subject pick are undo steps; a copy leaves the subject
//! behind.

use std::path::{Path, PathBuf};

use lightcraft_develop::{DepthMap, DevelopSettings, Flash};
use serde_json::json;

use crate::Session;
use crate::catalog::PhotoId;

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-flash-{tag}-{}", std::process::id()));
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
    let e = s.execute("flash.apply", &json!({})).unwrap_err().to_string();
    assert!(e.contains(crate::depth::NOT_INSTALLED), "{e}");
    assert_eq!(dev(&s), before);
    let i = s.execute("flash.info", &json!({})).unwrap();
    assert_eq!((i["enabled"].as_bool(), i["depth"].is_null(), i["modelReady"].as_bool()), (Some(false), true, Some(false)), "{i}");
    assert_eq!(i["presets"].as_array().map(Vec::len), Some(3), "{i}");
    assert!(i["subject"].is_null());
    // turning it off needs no depth
    s.execute("flash.apply", &json!({"on": false})).unwrap();
}

#[test]
fn a_portraits_own_depth_is_used_and_the_near_half_is_the_one_lit() {
    let dir = temp_dir("portrait");
    let path = dir.join("portrait.jpg");
    write_portrait(&path);
    let mut s = Session::new().with_fs();
    let id = open(&mut s, &path);
    let plain = s.render_now(id, 96, 64).unwrap().image;
    let i = s.execute("flash.apply", &json!({})).unwrap();
    assert_eq!((i["depth"].as_str(), i["enabled"].as_bool()), (Some("photo"), Some(true)), "{i}");
    assert!(dev(&s).lens_blur.depth.is_none(), "the file's depth isn't copied into the settings");
    let lit = s.render_now(id, 96, 64).unwrap().image;
    assert_ne!(lit, plain);
    let (near, far) = half_lumas(&lit);
    assert!(near > far + 20.0, "near (left) {near} vs far (right) {far}");
    s.execute("flash.apply", &json!({"on": false})).unwrap();
    assert_eq!(s.render_now(id, 96, 64).unwrap().image, plain);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_stored_estimate_is_shared_and_a_preset_is_one_undo_step() {
    let mut s = demo_with_depth();
    let a = s.active().unwrap();
    let plain = s.render_now(a, 160, 120).unwrap().image;
    let i = s.execute("flash.apply", &json!({})).unwrap();
    assert_eq!((i["depth"].as_str(), i["enabled"].as_bool()), (Some("estimated"), Some(true)), "{i}");
    let direct = s.render_now(a, 160, 120).unwrap().image;
    assert_ne!(direct, plain, "the flash shows");
    s.execute("flash.preset", &json!({"preset": "night"})).unwrap();
    let night = dev(&s).flash;
    assert_eq!((night.power, night.ambient, night.emit, night.enabled), (60.0, 27.0, 0.0, true));
    assert_ne!(s.render_now(a, 160, 120).unwrap().image, direct, "Night out is another picture");
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(dev(&s).flash, Flash { enabled: true, ..Flash::default() }, "one undo step");
    assert!(s.execute("flash.preset", &json!({"preset": "strobe"})).is_err());
    assert!(s.execute("flash.preset", &json!({})).is_err());
    // the sliders are develop controls: a drag of the flash's place is one undo step
    s.execute("develop.set", &json!({"control": "flash.angle", "value": 120.0})).unwrap();
    assert_eq!(dev(&s).flash.angle, 120.0);
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(dev(&s).flash.angle, 35.0);
    // Reset puts the sliders back and keeps it on, with the depth
    s.execute("flash.preset", &json!({"preset": "fill"})).unwrap();
    s.execute("flash.reset", &json!({})).unwrap();
    assert_eq!(dev(&s).flash, Flash { enabled: true, ..Flash::default() });
    assert!(dev(&s).lens_blur.depth.is_some());
}

#[test]
fn the_subject_is_picked_by_a_tap_set_back_to_auto_and_left_behind_by_a_copy() {
    let mut s = demo_with_depth();
    s.execute("flash.apply", &json!({})).unwrap();
    assert!(s.execute("flash.subject", &json!({})).is_err(), "needs a point or auto");
    // the bottom of the demo depth is near
    let i = s.execute("flash.subject", &json!({"x": 0.5, "y": 0.95})).unwrap();
    let near = i["subject"].as_f64().unwrap();
    let i = s.execute("flash.subject", &json!({"x": 0.5, "y": 0.05})).unwrap();
    let far = i["subject"].as_f64().unwrap();
    assert!(near > far + 0.3, "near {near} far {far}");
    assert_eq!(dev(&s).flash.subject, Some(far));
    // the picked depth belongs to this photo
    let groups = [lightcraft_develop::SettingsGroup::Flash];
    let copied = lightcraft_develop::extract_groups(&dev(&s), &groups);
    assert!(copied["flash"].get("subject").is_none(), "{copied}");
    assert_eq!(copied["flash"]["enabled"], true);
    assert!(lightcraft_develop::SettingsGroup::default_copy().contains(&lightcraft_develop::SettingsGroup::Flash));
    s.execute("flash.subject", &json!({"auto": true})).unwrap();
    assert_eq!(dev(&s).flash.subject, None);
}

#[test]
fn flash_settings_are_left_out_of_the_json_while_default_and_round_trip_when_not() {
    let mut s = DevelopSettings::default();
    assert!(s.to_json().get("flash").is_none());
    s.flash.enabled = true;
    s.flash.apply_preset("night");
    s.flash.far = 80.0;
    s.flash.subject = Some(0.7);
    assert_eq!(DevelopSettings::from_json(&s.to_json()).unwrap(), s);
    assert!(!Flash::default().active(), "off by default");
}

#[test]
fn flash_applies_to_a_named_photo_with_a_preset_and_changes_nothing_when_it_cannot() {
    let mut s = demo_with_depth();
    let a = s.active().unwrap();
    let other = s.visible_cloned().into_iter().find(|p| *p != a).unwrap();
    // the open photo, by id, with a preset in one step
    let r = s.execute("flash.apply", &json!({"id": a.0, "preset": "night"})).unwrap();
    assert_eq!(r["enabled"], true, "{r}");
    assert_eq!((dev(&s).flash.enabled, dev(&s).flash.power, dev(&s).flash.emit), (true, 60.0, 0.0));
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(!dev(&s).flash.enabled, "one undo step");
    // another photo that has no depth and no model: it says so and neither photo changes
    let before = s.develop_of(other).map(|d| (*d).clone());
    let e = s.execute("flash.apply", &json!({"id": other.0, "preset": "direct"})).unwrap_err().to_string();
    assert!(e.contains(crate::depth::NOT_INSTALLED), "{e}");
    assert_eq!(s.develop_of(other).map(|d| (*d).clone()), before);
    assert!(!dev(&s).flash.enabled);
    // a preset that is not one, and a photo that is not there
    assert!(s.execute("flash.apply", &json!({"preset": "strobe"})).is_err());
    assert!(s.execute("flash.apply", &json!({"id": 987_654_321u64})).is_err());
    // off by id
    s.execute("flash.apply", &json!({"id": a.0})).unwrap();
    s.execute("flash.apply", &json!({"id": a.0, "on": false})).unwrap();
    assert!(!dev(&s).flash.enabled);
}
