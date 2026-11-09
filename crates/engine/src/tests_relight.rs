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

#[test]
fn the_depth_exports_as_data_framed_as_the_photo() {
    let dir = temp_dir("export");
    let mut s = demo_with_depth();
    let a = s.active().unwrap();
    let png = dir.join("depth.png");
    let r = s.execute("depthMap.export", &json!({"path": png.to_string_lossy(), "long": 300})).unwrap();
    assert_eq!((r["format"].as_str(), r["white"].as_str(), r["depth"].as_str()), (Some("png"), Some("near"), Some("estimated")), "{r}");
    let (w, h) = (r["width"].as_u64().unwrap() as usize, r["height"].as_u64().unwrap() as usize);
    assert_eq!(w.max(h), 300, "{r}");
    let bytes = std::fs::read(&png).unwrap();
    // 16-bit greyscale (IHDR: bit depth 16, colour type 0)
    assert_eq!((bytes[24], bytes[25]), (16, 0));
    let (pw, ph, px) = lightcraft_codecs::decode_png_codes(&bytes, 1 << 20).unwrap();
    assert_eq!((pw as usize, ph as usize), (w, h));
    // the stored depth runs from far (top) to near (bottom): white near
    let row = |y: usize| px[y * w..(y + 1) * w].iter().map(|p| p[0]).sum::<f32>() / w as f32;
    assert!(row(h - 2) > row(1) + 0.5, "{} vs {}", row(h - 2), row(1));
    // inverted, and as 32-bit float TIFF
    let tiff = dir.join("depth.tif");
    let r = s.execute("depthMap.export", &json!({"path": tiff.to_string_lossy(), "format": "tiff", "invert": true, "long": 120})).unwrap();
    assert_eq!(r["white"], "far");
    let d = lightcraft_codecs::decode(&std::fs::read(&tiff).unwrap(), Default::default()).unwrap();
    assert_eq!((d.width as u64, d.height as u64), (r["width"].as_u64().unwrap(), r["height"].as_u64().unwrap()));
    // a crop frames it as an export of the photo is, unless asked for the whole photo
    let mut dv = dev(&s);
    dv.crop.geometry.rect = lightcraft_geom::Rect::new(0.0, 0.0, 0.5, 1.0);
    s.execute("develop.merge", &json!({"settings": dv.to_json()})).unwrap();
    let cropped = s.execute("depthMap.export", &json!({"path": png.to_string_lossy(), "long": 300})).unwrap();
    let whole = s.execute("depthMap.export", &json!({"path": png.to_string_lossy(), "long": 300, "crop": false})).unwrap();
    assert!(cropped["width"].as_u64() < whole["width"].as_u64(), "{cropped} {whole}");
    // without depth: says so, writes nothing
    let mut plain = Session::with_demo();
    let gone = dir.join("none.png");
    let e = plain.execute("depthMap.export", &json!({"path": gone.to_string_lossy()})).unwrap_err().to_string();
    assert!(e.contains("no depth"), "{e}");
    assert!(!gone.exists());
    assert!(s.execute("depthMap.export", &json!({"path": png.to_string_lossy(), "format": "exr"})).is_err());
    assert!(s.execute("depthMap.export", &json!({})).is_err(), "a path is needed");
    let _ = a;
    let _ = std::fs::remove_dir_all(&dir);
}

/// Normals that face left on the left half of the photo and right on the right, as the surface model leaves them.
fn surface() -> lightcraft_develop::NormalMap {
    let n: Vec<[f32; 3]> = (0..16 * 12).map(|i| if i % 16 < 8 { [-0.6, 0.0, 0.8] } else { [0.6, 0.0, 0.8] }).collect();
    lightcraft_develop::NormalMap::from_normals(16, 12, &n).unwrap()
}

#[test]
fn the_surface_model_is_offered_kept_through_reset_and_taken_away_on_request() {
    let mut s = demo_with_depth();
    s.execute("relight.apply", &json!({})).unwrap();
    let info = s.execute("relight.info", &json!({})).unwrap();
    assert_eq!(
        (info["normals"].clone(), info["surfaceReady"].clone(), info["surfaceModel"].clone()),
        (Value::Null, json!(false), json!("moge-2-vits-normal")),
        "{info}"
    );
    // without the model Estimate Surface says so (the apps offer the download on it) and changes nothing
    let before = dev(&s);
    let e = s.execute("relight.surface", &json!({})).unwrap_err().to_string();
    assert!(e.contains(crate::depth::SURFACE_NOT_INSTALLED), "{e}");
    assert_eq!(dev(&s), before);
    // the model is listed with its download, licence and size (in builds with the downloadable models)
    if cfg!(feature = "enhance") {
        let list = s.execute("models.list", &json!({})).unwrap();
        let m = list["models"].as_array().unwrap().iter().find(|m| m["id"] == "moge-2-vits-normal").cloned().unwrap_or_else(|| panic!("{list}"));
        assert_eq!((m["licence"].as_str(), m["installed"].as_bool()), (Some("MIT"), Some(false)), "{m}");
        assert!(m["downloadBytes"].as_u64().is_some_and(|b| b > 100_000_000), "{m}");
    }
    // a surface kept with the settings shades the photo: a light on the left lights the half that faces left
    let mut d = dev(&s);
    d.relight.normals = Some(surface());
    d.relight.ambient = 0.0;
    d.relight.shadows = 0.0;
    d.relight.spread = 100.0;
    d.relight.y = 0.5;
    d.relight.x = -0.2;
    s.execute("develop.merge", &json!({"settings": d.to_json()})).unwrap();
    assert_eq!(s.execute("relight.info", &json!({})).unwrap()["normals"], "model");
    let img = s.render_now(s.active().unwrap(), 96, 64).unwrap().image;
    let (l, rgt) = half_lumas(&img);
    assert!(l > rgt + 10.0, "{l} {rgt}");
    // Reset keeps it, as it keeps the depth
    s.execute("relight.reset", &json!({})).unwrap();
    assert!(dev(&s).relight.normals.is_some() && dev(&s).lens_blur.depth.is_some());
    // {on: false} goes back to the depth's slope, as one undo step
    s.execute("relight.surface", &json!({"on": false})).unwrap();
    assert!(dev(&s).relight.normals.is_none());
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(dev(&s).relight.normals.is_some());
}

#[test]
fn a_depth_range_is_picked_from_the_photo_and_its_depths_are_counted() {
    let mut s = Session::with_demo();
    s.execute("mask.add", &json!({"kind": "depthRange", "lo": 0.0, "hi": 0.3})).unwrap();
    // no depth: picking says so, and the histogram has nothing to count
    let e = s.execute("mask.pickDepth", &json!({"x": 0.5, "y": 0.5})).unwrap_err().to_string();
    assert!(e.contains("no depth"), "{e}");
    let h = s.execute("depth.histogram", &json!({})).unwrap();
    assert_eq!((h["bins"].as_array().map(Vec::len), h["depth"].clone()), (Some(0), Value::Null), "{h}");
    // with depth (near at the bottom, far at the top): a click picks a band around the depth there
    let mut s = demo_with_depth();
    s.execute("mask.add", &json!({"kind": "depthRange", "lo": 0.0, "hi": 0.3})).unwrap();
    let r = s.execute("mask.pickDepth", &json!({"x": 0.5, "y": 0.9})).unwrap();
    let (lo, hi) = (r["lo"].as_f64().unwrap(), r["hi"].as_f64().unwrap());
    // (nearness 0.1 + 0.8 · 0.9 = 0.82 there: a depth of about 0.18)
    assert!((lo + hi) / 2.0 > 0.1 && (lo + hi) / 2.0 < 0.26 && (hi - lo - 0.1).abs() < 0.02, "{r}");
    let part = dev(&s).masks.last().unwrap().components.last().unwrap().shape.clone();
    let lightcraft_develop::MaskShape::DepthRange { lo: l, hi: hh, feather } = part else { panic!("{part:?}") };
    assert_eq!((l, hh, feather), (lo, hi, 0.1), "the smoothness is kept");
    // a box from the top to the middle covers the far half's depths
    let r = s.execute("mask.pickDepth", &json!({"x": 0.2, "y": 0.02, "x2": 0.8, "y2": 0.5})).unwrap();
    let (lo, hi) = (r["lo"].as_f64().unwrap(), r["hi"].as_f64().unwrap());
    assert!(lo > 0.4 && lo < 0.6 && hi > 0.8, "{r}");
    // one undo step each
    s.execute("edit.undo", &json!({})).unwrap();
    let lightcraft_develop::MaskShape::DepthRange { lo: back, .. } = dev(&s).masks.last().unwrap().components.last().unwrap().shape.clone() else {
        panic!()
    };
    assert!((back - 0.13).abs() < 0.06, "{back}");
    // a part that isn't a Depth Range is refused
    s.execute("mask.add", &json!({"kind": "linear"})).unwrap();
    let e = s.execute("mask.pickDepth", &json!({"x": 0.5, "y": 0.5})).unwrap_err().to_string();
    assert!(e.contains("no Depth Range"), "{e}");
    // the histogram counts the photo's depths, near to far, the largest bin 1
    let h = s.execute("depth.histogram", &json!({"bins": 16})).unwrap();
    let bins: Vec<f64> = h["bins"].as_array().unwrap().iter().filter_map(Value::as_f64).collect();
    assert_eq!((bins.len(), h["depth"].as_str()), (16, Some("estimated")), "{h}");
    assert!(bins.iter().all(|b| (0.0..=1.0).contains(b)) && bins.contains(&1.0), "{bins:?}");
    // (depths run from 0.1 to 0.9: none at the very nearest or the very farthest)
    assert!(bins[0] == 0.0 && bins[15] == 0.0 && bins[8] > 0.0, "{bins:?}");
}

#[test]
fn auto_sync_leaves_each_photos_depth_and_surface_behind() {
    let mut s = demo_with_depth();
    let ids: Vec<u64> = s.catalog.photos().take(2).map(|p| p.id.0).collect();
    let active = s.active().unwrap().0;
    let other = *ids.iter().find(|i| **i != active).unwrap();
    s.execute("library.select", &json!({"ids": [active, other]})).unwrap();
    s.selection.active = Some(PhotoId(active));
    s.execute("develop.autoSync", &json!({"on": true})).unwrap();
    let mut d = dev(&s);
    d.relight.enabled = true;
    d.relight.brightness = 70.0;
    d.relight.normals = Some(surface());
    d.lens_blur.depth = DepthMap::from_nearness(2, 2, &[0.1, 0.2, 0.3, 0.4]);
    s.set_develop(PhotoId(active), d, "Relight").unwrap();
    let o = s.develop_of(PhotoId(other)).unwrap();
    assert!(o.relight.enabled && o.relight.brightness == 70.0, "the look syncs");
    assert!(o.relight.normals.is_none() && o.lens_blur.depth.is_none(), "the pictures of the first photo don't");
}
