//! Lens Blur commands: a photo's own depth is used without the model, the model's estimate is
//! stored, focus and bokeh are one undo step each, and a missing model says so.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::Session;

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-lensblur-{tag}-{}", std::process::id()));
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

/// A Portrait JPEG laid out as Apple writes one: the photo with an MPF index, then a greyscale
/// JPEG of its disparity marked by `apdi` XMP. The left half is near, the right half far.
fn write_portrait(path: &Path) {
    let mut photo = jpeg(96, 64, |x, y| [(x * 2) as u8, (y * 3) as u8, if (x / 8 + y / 8) % 2 == 0 { 200 } else { 40 }], None);
    // (the reader only needs the index to be there)
    let mpf = [&[0xFF, 0xE2, 0x00, 0x0A][..], b"MPF\0MM\0\0"].concat();
    photo.splice(2..2, mpf);
    let xmp = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description xmlns:apdi="http://ns.apple.com/pixeldatainfo/1.0/"><apdi:AuxiliaryImageType>disparity</apdi:AuxiliaryImageType><apdi:FloatMinValue>0.25</apdi:FloatMinValue><apdi:FloatMaxValue>1.0</apdi:FloatMaxValue><apdi:IntMinValue>0</apdi:IntMinValue><apdi:IntMaxValue>255</apdi:IntMaxValue></rdf:Description></rdf:RDF></x:xmpmeta>"#;
    let depth = jpeg(48, 32, |x, _| if x < 24 { [255; 3] } else { [0; 3] }, Some(xmp));
    std::fs::write(path, [photo, depth].concat()).unwrap();
}

fn open(s: &mut Session, path: &Path) -> u64 {
    let r = s.execute("library.import", &json!({"paths": [path.to_string_lossy()]})).unwrap();
    let id = r["imported"][0].as_u64().unwrap_or_else(|| panic!("{r}"));
    s.execute("library.select", &json!({"ids": [id], "active": id})).unwrap();
    id
}

fn info(s: &mut Session) -> Value {
    s.execute("lensBlur.info", &json!({})).unwrap()
}

#[test]
fn without_depth_or_model_apply_says_so_and_changes_nothing() {
    let mut s = Session::with_demo();
    let before = s.develop_of(s.active().unwrap()).unwrap();
    let e = s.execute("lensBlur.apply", &json!({})).unwrap_err().to_string();
    assert!(e.contains(crate::depth::NOT_INSTALLED), "{e}");
    assert_eq!(s.develop_of(s.active().unwrap()).unwrap(), before);
    let i = info(&mut s);
    assert_eq!((i["enabled"].as_bool(), i["depth"].is_null(), i["modelReady"].as_bool()), (Some(false), true, Some(false)));
    assert!(s.install_depth_model(b"not the model").is_err());
    assert!(s.execute("lensBlur.focus", &json!({"x": 0.5, "y": 0.5})).is_err(), "no depth to focus on");
    assert!(s.execute("lensBlur.bokeh", &json!({"shape": "hexagon"})).is_err());
    // turning it off needs no depth
    s.execute("lensBlur.apply", &json!({"on": false})).unwrap();
}

#[test]
fn a_portraits_own_depth_is_used_without_the_model() {
    let dir = temp_dir("portrait");
    let path = dir.join("portrait.jpg");
    write_portrait(&path);
    let mut s = Session::new().with_fs();
    let id = open(&mut s, &path);
    let i = s.execute("lensBlur.apply", &json!({})).unwrap();
    assert_eq!(i["depth"], "photo", "{i}");
    assert_eq!(i["enabled"], true);
    let d = s.develop_of(crate::catalog::PhotoId(id)).unwrap();
    assert!(d.lens_blur.depth.is_none(), "the file's depth isn't copied into the settings");
    assert_eq!((d.lens_blur.amount, d.lens_blur.boost), (50.0, 50.0));
    // focusing on the far right half moves the focal distance far, the left half near
    let far = s.execute("lensBlur.focus", &json!({"x": 0.85, "y": 0.5})).unwrap()["focalDistance"].as_f64().unwrap();
    let near = s.execute("lensBlur.focus", &json!({"x": 0.15, "y": 0.5})).unwrap()["focalDistance"].as_f64().unwrap();
    assert!(far > 80.0 && near < 20.0, "far {far}, near {near}");
    s.execute("lensBlur.bokeh", &json!({"shape": "blade5"})).unwrap();
    assert_eq!(info(&mut s)["bokeh"], "blade5");
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(info(&mut s)["bokeh"], "circle", "one undo step");
    // a Depth Range mask selects by it
    s.execute("mask.add", &json!({"kind": "depthRange", "lo": 0.0, "hi": 0.4})).unwrap();
    let _ = std::fs::remove_dir_all(dir);
}

/// With the model (when a copy is at `$LIGHTCRAFT_DEPTH_MODEL`; it is never part of the
/// repository): the estimate is stored with the settings and focuses on the subject.
#[test]
fn the_models_estimate_is_stored_with_the_settings() {
    let Some(path) = std::env::var_os("LIGHTCRAFT_DEPTH_MODEL") else { return };
    let mut s = Session::with_demo();
    s.install_depth_model(&std::fs::read(path).unwrap()).unwrap();
    assert!(s.depth_model_ready());
    let id = s.active().unwrap();
    let i = s.execute("lensBlur.apply", &json!({})).unwrap();
    assert_eq!(i["depth"], "estimated", "{i}");
    let d = s.develop_of(id).unwrap();
    let map = d.lens_blur.depth.clone().expect("a stored estimate");
    assert!(map.w.max(map.h) <= 1036 && map.nearness().is_some());
    assert!((0.0..=100.0).contains(&d.lens_blur.focal_distance));
    // undo takes the whole step back, estimate included
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(s.develop_of(id).unwrap().lens_blur.depth.is_none());
}

/// A look at real photos (`LIGHTCRAFT_LENSBLUR_PHOTOS`: paths separated by `:`; with
/// `LIGHTCRAFT_DEPTH_MODEL` and `LIGHTCRAFT_TEST_OUT`): each is opened, Lens Blur applied and the
/// result written as `<name>-blur.png` with its depth view `<name>-depth.png`.
#[test]
fn real_photos_render_with_lens_blur() {
    let (Some(photos), Some(model), Some(out)) =
        (std::env::var_os("LIGHTCRAFT_LENSBLUR_PHOTOS"), std::env::var_os("LIGHTCRAFT_DEPTH_MODEL"), std::env::var_os("LIGHTCRAFT_TEST_OUT"))
    else {
        return;
    };
    let out = PathBuf::from(out);
    let mut s = Session::new().with_fs();
    s.install_depth_model(&std::fs::read(model).unwrap()).unwrap();
    for p in std::env::split_paths(&photos) {
        let id = crate::catalog::PhotoId(open(&mut s, &p));
        let t = std::time::Instant::now();
        let i = s.execute("lensBlur.apply", &json!({})).unwrap();
        eprintln!("{}: apply {:.0} ms → {i}", p.display(), t.elapsed().as_secs_f64() * 1e3);
        let src = s.source_now(id, crate::media::SourceLevel::Preview).unwrap();
        let info = s.source_info(id);
        let d = s.develop_of(id).unwrap();
        let name = p.file_stem().unwrap().to_string_lossy().to_string();
        for (suffix, overlay) in [("blur", lightcraft_pipeline::Overlay::None), ("depth", lightcraft_pipeline::Overlay::Depth)] {
            let req = lightcraft_pipeline::RenderRequest { overlay, ..lightcraft_pipeline::RenderRequest::fit(1600, 1600) };
            let t = std::time::Instant::now();
            let img = lightcraft_pipeline::render(&src, &info, &d, &req).image;
            eprintln!("  render {suffix}: {:.0} ms", t.elapsed().as_secs_f64() * 1e3);
            let png = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &Default::default()).unwrap();
            std::fs::write(out.join(format!("{name}-{suffix}.png")), png).unwrap();
        }
    }
}

#[test]
fn refine_strokes_are_painted_and_cleared_as_undo_steps() {
    let dir = temp_dir("brush");
    let path = dir.join("portrait.jpg");
    write_portrait(&path);
    let mut s = Session::new().with_fs();
    open(&mut s, &path);
    s.execute("lensBlur.apply", &json!({})).unwrap();
    let i = s.execute("lensBlur.brushStroke", &json!({"mode": "focus", "points": [[0.8, 0.5], [0.9, 0.5]], "size": 0.05})).unwrap();
    assert_eq!((i["focusStrokes"].as_u64(), i["blurStrokes"].as_u64()), (Some(1), Some(0)));
    let i = s.execute("lensBlur.brushStroke", &json!({"mode": "blur", "points": [[0.2, 0.5]]})).unwrap();
    assert_eq!(i["blurStrokes"], 1);
    // erasing goes on both painted lists
    let i = s.execute("lensBlur.brushStroke", &json!({"mode": "erase", "points": [[0.2, 0.5]]})).unwrap();
    assert_eq!((i["focusStrokes"].as_u64(), i["blurStrokes"].as_u64()), (Some(2), Some(2)));
    assert!(s.execute("lensBlur.brushStroke", &json!({"mode": "paint", "points": [[0.2, 0.5]]})).is_err());
    assert!(s.execute("lensBlur.brushStroke", &json!({"mode": "focus", "points": []})).is_err());
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(info(&mut s)["blurStrokes"], 1, "one undo step per stroke");
    let i = s.execute("lensBlur.clearBrush", &json!({"mode": "blur"})).unwrap();
    assert_eq!((i["focusStrokes"].as_u64(), i["blurStrokes"].as_u64()), (Some(1), Some(0)));
    let _ = std::fs::remove_dir_all(dir);
}
