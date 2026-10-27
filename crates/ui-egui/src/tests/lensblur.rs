//! Headless tests of the Edit panel's Lens Blur section: Apply on a photo with depth of its own,
//! the Refine brush painted on the photo, the focal range bar and the focus picker.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

use crate::headless::Headless;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(30);

fn jpeg(w: usize, h: usize, px: impl Fn(usize, usize) -> [u8; 3], xmp: Option<&str>) -> Vec<u8> {
    let data: Vec<[u8; 4]> = (0..w * h).map(|i| px(i % w, i / w)).map(|[r, g, b]| [r, g, b, 255]).collect();
    let img = lightcraft_raster::Rgba8 { width: w, height: h, data };
    let meta = lightcraft_codecs::EncodeMeta { xmp, ..Default::default() };
    lightcraft_codecs::encode_jpeg(&lightcraft_codecs::EncodeImage::rgba8(&img), 95, Default::default(), &meta).unwrap()
}

/// A Portrait JPEG as Apple lays one out (see the engine's `tests_lensblur`): the left half near.
fn write_portrait(path: &Path) {
    let mut photo = jpeg(240, 160, |x, y| [(x % 256) as u8, (y * 2 % 256) as u8, if (x / 12 + y / 12) % 2 == 0 { 200 } else { 40 }], None);
    photo.splice(2..2, [&[0xFF, 0xE2, 0x00, 0x0A][..], b"MPF\0MM\0\0"].concat());
    let xmp = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description xmlns:apdi="http://ns.apple.com/pixeldatainfo/1.0/"><apdi:AuxiliaryImageType>disparity</apdi:AuxiliaryImageType></rdf:Description></rdf:RDF></x:xmpmeta>"#;
    let depth = jpeg(48, 32, |x, _| if x < 24 { [255; 3] } else { [0; 3] }, Some(xmp));
    std::fs::write(path, [photo, depth].concat()).unwrap();
}

fn temp_dir() -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-ui-lensblur-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn ok(h: &mut Headless, method: &str, params: Value) -> Value {
    let r = h.request(method, params.clone(), T);
    assert_eq!(r["ok"], true, "{method} {params}: {r}");
    r["result"].clone()
}

fn lens_blur(h: &Headless) -> lightcraft_develop::LensBlur {
    let id = h.app.session.active().expect("active photo");
    h.app.session.develop_of(id).unwrap_or_default().lens_blur.clone()
}

#[test]
fn apply_paint_refine_and_drag_the_focal_range() {
    let dir = temp_dir();
    let path = dir.join("portrait.jpg");
    write_portrait(&path);
    let mut session = lightcraft_engine::Session::new().with_fs();
    let r = session.execute("library.import", &json!({"paths": [path.to_string_lossy()]})).unwrap();
    let id = r["imported"][0].as_u64().unwrap();
    session.execute("library.select", &json!({"ids": [id], "active": id})).unwrap();
    let app = LightcraftApp::new(session, Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1200.0, 800.0], 1.0);
    ok(&mut h, "ui.set", json!({"view": "detail", "right": "edit", "openSections": ["lensBlur"], "histogram": false}));
    // Apply: the photo's own depth, no model needed
    ok(&mut h, "ui.clickWidget", json!({"id": "button:lensBlurApply"}));
    let lb = lens_blur(&h);
    assert!(lb.enabled && lb.depth.is_none(), "{lb:?}");
    // the Focus brush is a tool on the photo; a stroke across the far half is one undo step
    ok(&mut h, "ui.clickWidget", json!({"id": "button:lensBlurBrush-focus"}));
    assert_eq!(h.app.ui.tool, crate::lensblur::BRUSH_TOOL);
    ok(
        &mut h,
        "ui.pointer",
        json!({"events": [{"kind": "down", "x": 0.7, "y": 0.5}, {"kind": "drag", "x": 0.8, "y": 0.5}, {"kind": "drag", "x": 0.9, "y": 0.5}, {"kind": "up", "x": 0.9, "y": 0.5}]}),
    );
    let lb = lens_blur(&h);
    assert_eq!((lb.focus.len(), lb.blur.len()), (1, 0), "{lb:?}");
    let p = lb.focus[0].points.first().copied().unwrap();
    assert!((0.68..0.92).contains(&p.x) && (p.y - 0.5).abs() < 0.02, "on the stroke's path: {p:?}");
    // ⌥ paints with Erase
    ok(&mut h, "ui.pointer", json!({"events": [{"kind": "down", "x": 0.75, "y": 0.5}, {"kind": "up", "x": 0.75, "y": 0.5}], "alt": true}));
    let lb = lens_blur(&h);
    assert!(lb.focus.len() == 2 && lb.focus[1].erase, "{lb:?}");
    ok(&mut h, "engine.execute", json!({"command": "edit.undo"}));
    assert_eq!(lens_blur(&h).focus.len(), 1);
    // choosing the brush again puts it down
    ok(&mut h, "ui.clickWidget", json!({"id": "button:lensBlurBrush-focus"}));
    assert!(h.app.ui.tool.is_empty());
    // dragging the far end of the focal range nearer narrows it, one undo step
    let before = lens_blur(&h);
    ok(&mut h, "ui.dragWidget", json!({"id": "lensBlurRange:far", "dx": -30.0, "steps": 6}));
    let after = lens_blur(&h);
    assert!(after.focus_bounds()[1] < before.focus_bounds()[1] - 5.0, "{:?} → {:?}", before.focus_bounds(), after.focus_bounds());
    assert!((after.focus_bounds()[0] - before.focus_bounds()[0]).abs() < 1e-6, "the near end stays");
    ok(&mut h, "engine.execute", json!({"command": "edit.undo"}));
    assert_eq!(lens_blur(&h).focus_bounds(), before.focus_bounds());
    // the focus picker: a click on the far half focuses far
    ok(&mut h, "ui.clickWidget", json!({"id": "icon:lensFocus"}));
    assert_eq!(h.app.ui.tool, crate::lensblur::FOCUS_TOOL);
    ok(&mut h, "ui.pointer", json!({"events": [{"kind": "down", "x": 0.85, "y": 0.5}, {"kind": "up", "x": 0.85, "y": 0.5}]}));
    assert!(lens_blur(&h).focal_distance > 80.0, "{:?}", lens_blur(&h));
    let _ = std::fs::remove_dir_all(dir);
}
