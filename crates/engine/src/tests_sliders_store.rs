//! FB-SLIDER: every control moved at once, kept by the catalog (the library closed and opened again) and by the XMP
//! sidecar (the library deleted and its folder imported again): the settings come back equal.

use lightcraft_develop::{CONTROLS, DevelopSettings, controls};
use serde_json::json;

use crate::Session;
use crate::tests_xmp::temp_dir;

#[test]
fn all_controls_survive_the_catalog_and_the_sidecar() {
    let (src, lib) = (temp_dir("sliders-src"), temp_dir("sliders-lib"));
    let (w, h) = (40usize, 24usize);
    let data: Vec<[u8; 4]> = (0..w * h).map(|i| [(i % w * 6) as u8, (i / w * 9) as u8, 7, 255]).collect();
    let img = lightcraft_raster::Rgba8 { width: w, height: h, data };
    let png = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap();
    std::fs::write(src.join("all.png"), png).unwrap();
    let mut all = DevelopSettings::default();
    for c in CONTROLS {
        controls::set(&mut all, c.id, if c.default == c.max { c.min } else { c.max });
    }
    // (the crop's angle moves the crop rect with it: the crop has its own tests)
    all.crop.geometry.angle = 0.0;
    let dev = |s: &Session| {
        let id = s.catalog.photos().next().unwrap().id;
        (*s.develop_of(id).unwrap()).clone()
    };
    let want = {
        let mut s = Session::new().with_fs();
        s.open_library(&lib, false).unwrap();
        s.execute("library.import", &json!({"paths": [src.join("all.png").to_string_lossy()]})).unwrap();
        let id = s.catalog.photos().next().unwrap().id;
        s.execute("library.select", &json!({"ids": [id.0], "active": id.0})).unwrap();
        s.execute("develop.merge", &json!({"settings": all.to_json()})).unwrap();
        s.execute("photo.saveMetadataToFile", &json!({})).unwrap();
        dev(&s)
    };
    for c in CONTROLS.iter().filter(|c| !c.id.starts_with("wb.") && c.id != "crop.angle") {
        let far = c.clamp(if c.default == c.max { c.min } else { c.max });
        assert_eq!(controls::get(&want, c.id), Some(far), "{}: kept by develop.merge", c.id);
    }
    // the catalog: the same library opened again
    {
        let mut s = Session::new().with_fs();
        s.open_library(&lib, false).unwrap();
        assert_eq!(dev(&s), want, "the catalog gives the settings back");
    }
    // the sidecar: a new library imports the folder
    std::fs::remove_dir_all(&lib).unwrap();
    let mut s = Session::new().with_fs();
    s.open_library(&lib, false).unwrap();
    s.execute("library.import", &json!({"paths": [src.to_string_lossy()]})).unwrap();
    assert_eq!(dev(&s), want, "the sidecar gives the settings back");
    let _ = std::fs::remove_dir_all(&src);
    let _ = std::fs::remove_dir_all(&lib);
}
