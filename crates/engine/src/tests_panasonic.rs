//! A Panasonic raw shot at an aspect ratio other than the sensor's (a GH7 at 16:9): its embedded JPEG shows the whole
//! sensor area and the decoder develops the frame the setting cuts. A photo imported now keeps the whole area, with the
//! camera's frame as its import crop (so the Crop tool can widen it); a photo catalogued at the camera's frame (every one
//! imported before) is developed as it always was. Synthetic containers: `lightcraft_raw::testdata::PanasonicRw2`.

use std::path::PathBuf;

use lightcraft_catalog::{MediaKind, Op, Photo, PhotoId, Source};
use lightcraft_raw::testdata::PanasonicRw2;
use serde_json::json;

use crate::Session;
use crate::files::{load_bytes_for, probe_bytes};

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-panasonic-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A 100 × 74 sensor whose active area, 96 × 72 (4:3) after the 2 rows and 4 columns of border, is cut to 96 × 54 (16:9) by
/// the camera: the rows 9 to 62 of the area, the sensor rows 11 to 64.
fn sixteen_nine() -> PanasonicRw2 {
    PanasonicRw2 { crop: Some([11, 4, 65, 100]), ..PanasonicRw2::new(100, 74) }
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

/// What the probe says of the same file in the camera's frame and, new, of the whole area it was cut from.
#[test]
fn the_probe_describes_the_camera_frame_and_the_whole_area() {
    let info = probe_bytes("P1002298.RW2", &sixteen_nine().build()).unwrap();
    assert_eq!((info.width, info.height), (96, 54), "the camera's frame, as every photo before has it");
    let c = info.camera_crop.expect("a camera that crops");
    assert_eq!((c.whole, c.framed), ((96, 72), (96, 54)));
    let [x0, y0, x1, y1] = c.frame;
    assert!(near(x0, 0.0) && near(x1, 1.0) && near(y0, 9.0 / 72.0) && near(y1, 63.0 / 72.0), "{:?}", c.frame);

    // the camera held upright: the frame's rows are columns of the picture shown
    let upright = PanasonicRw2 { orientation: 6, ..sixteen_nine() };
    let info = probe_bytes("P1002299.RW2", &upright.build()).unwrap();
    assert_eq!((info.width, info.height), (54, 96));
    let c = info.camera_crop.unwrap();
    assert_eq!((c.whole, c.framed), ((72, 96), (54, 96)));
    let [x0, y0, x1, y1] = c.frame;
    assert!(near(y0, 0.0) && near(y1, 1.0) && near(x0, 1.0 - 63.0 / 72.0) && near(x1, 1.0 - 9.0 / 72.0), "{:?}", c.frame);

    // no crop, a crop that is the whole area, and another maker's file: nothing to keep
    assert!(probe_bytes("a.rw2", &PanasonicRw2::new(100, 74).build()).unwrap().camera_crop.is_none());
    let all = PanasonicRw2 { crop: Some([2, 4, 74, 100]), ..PanasonicRw2::new(100, 74) };
    assert!(probe_bytes("b.rw2", &all.build()).unwrap().camera_crop.is_none());
}

/// The size a photo was catalogued with says which frame its raw is developed in: the whole area for a photo
/// catalogued whole, the camera's frame for every other size (and for a file that is no photo's).
#[test]
fn the_loader_develops_the_frame_the_photo_was_catalogued_in() {
    let bytes = sixteen_nine().build();
    let size = |catalogued| load_bytes_for(&bytes, 512, catalogued).map(|(img, _)| (img.width, img.height));
    assert_eq!(size(None), Ok((96, 54)), "no photo: the camera's frame, as it always was");
    assert_eq!(size(Some((96, 54))), Ok((96, 54)), "catalogued in the camera's frame");
    assert_eq!(size(Some((96, 72))), Ok((96, 72)), "catalogued whole");
    assert_eq!(size(Some((1, 1))), Ok((96, 54)), "any other size");

    let upright = PanasonicRw2 { orientation: 6, ..sixteen_nine() }.build();
    let size = |catalogued| load_bytes_for(&upright, 512, catalogued).map(|(img, _)| (img.width, img.height));
    assert_eq!(size(Some((72, 96))), Ok((72, 96)), "catalogued whole, turned");
    assert_eq!(size(Some((54, 96))), Ok((54, 96)));
}

/// A photo imported now keeps the whole area: its size is the sensor's, the camera's frame is its crop as part of its import
/// look (not an edit; Reset returns to it), and the Crop tool's uncropped view shows the rest of the sensor.
#[test]
fn a_new_import_keeps_the_whole_sensor_with_the_frame_as_its_import_crop() {
    let src = temp_dir("import");
    std::fs::write(src.join("P1002298.RW2"), sixteen_nine().build()).unwrap();
    std::fs::write(src.join("P1002300.RW2"), PanasonicRw2::new(100, 74).build()).unwrap();
    let mut s = Session::new().with_fs();
    let r = s.execute("library.import", &json!({"paths": [src.to_string_lossy()]})).unwrap();
    assert_eq!(r["imported"].as_array().map(Vec::len), Some(2), "{r}");
    let photos: Vec<Photo> = s.catalog.photos().map(|p| (**p).clone()).collect();
    let photo = |name: &str| photos.iter().find(|p| p.file_name == name).unwrap().clone();

    let p = photo("P1002298.RW2");
    assert_eq!((p.kind, p.width, p.height), (MediaKind::Raw, 96, 72), "the whole area");
    let r = p.develop.crop.geometry.rect;
    assert!(near(r.x0, 0.0) && near(r.x1, 1.0) && near(r.y0, 0.125) && near(r.y1, 0.875), "{r:?}");
    assert!(!p.is_edited(), "an import look, not an edit");
    assert!(p.develop_is(p.import_defaults()));
    assert_eq!(p.shown_size(), (96.0, 54.0));
    // shown cropped, and the uncropped view the Crop tool works on has the rest
    let mut shown = |apply_crop| {
        let img = s.render_job(p.id, 96, 72, false, apply_crop).unwrap().run().rendered.unwrap().image;
        (img.width, img.height)
    };
    assert_eq!(shown(true), (96, 54));
    assert_eq!(shown(false), (96, 72));

    // a file with no crop of its own is as it always was
    let plain = photo("P1002300.RW2");
    assert_eq!((plain.width, plain.height), (96, 72));
    assert!(plain.import_look.is_none() && plain.develop.crop.geometry.rect == lightcraft_geom::Rect::UNIT);
    let _ = std::fs::remove_dir_all(&src);
}

/// Settings made elsewhere (Lightroom's, in a sidecar) are relative to the camera's frame, the picture Lightroom shows: a
/// photo that comes with any keeps that frame, as every photo imported before.
#[test]
fn a_photo_with_settings_from_a_sidecar_keeps_the_camera_frame() {
    let src = temp_dir("sidecar");
    std::fs::write(src.join("P1002298.RW2"), sixteen_nine().build()).unwrap();
    std::fs::write(
        src.join("P1002298.xmp"),
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:Exposure2012="+0.50"/></rdf:RDF></x:xmpmeta>"#,
    )
    .unwrap();
    let mut s = Session::new().with_fs();
    let r = s.execute("library.import", &json!({"paths": [src.join("P1002298.RW2").to_string_lossy()]})).unwrap();
    assert_eq!(r["imported"].as_array().map(Vec::len), Some(1), "{r}");
    let p = s.catalog.photos().next().unwrap().clone();
    assert_eq!((p.width, p.height), (96, 54), "the camera's frame");
    assert_eq!(p.develop.crop.geometry.rect, lightcraft_geom::Rect::UNIT);
    assert!(s.render_job(p.id, 96, 72, false, true).unwrap().run().rendered.is_ok());
    let _ = std::fs::remove_dir_all(&src);
}

/// A photo catalogued at the camera's frame (every one imported before) is developed in that frame and stays as it was.
#[test]
fn a_photo_catalogued_in_the_camera_frame_is_developed_in_it() {
    let src = temp_dir("legacy");
    let path = src.join("P1002298.RW2");
    std::fs::write(&path, sixteen_nine().build()).unwrap();
    let mut s = Session::new().with_fs();
    let mut p = Photo::new(PhotoId(1), Source::File { path: path.to_string_lossy().into() }, "P1002298.RW2", "RW2", 96, 54, "2026-01-01");
    p.kind = MediaKind::Raw;
    p.develop = std::sync::Arc::new(p.camera_defaults());
    s.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    let img = s.render_job(PhotoId(1), 96, 72, false, true).unwrap().run().rendered.unwrap().image;
    assert_eq!((img.width, img.height), (96, 54));
    let _ = std::fs::remove_dir_all(&src);
}
