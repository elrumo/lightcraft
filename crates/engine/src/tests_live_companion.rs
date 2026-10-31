//! A Live Photo whose clip is imported as a video of its own (`live.companion`): the pair is found by name, and the
//! grid lists the photo alone.

use std::path::{Path, PathBuf};

use serde_json::json;

use crate::Session;

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-live-companion-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn png(p: &Path, seed: u8) {
    let img = lightcraft_raster::Rgba8::from_fn(16, 12, |x, y| [(x * 9) as u8, (y * 11) as u8, seed, 255]);
    let b =
        crate::export::encode_image(&img, &crate::export::ExportOptions { format: crate::export::ExportFormat::Png, ..Default::default() }).unwrap();
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, b).unwrap();
}

#[test]
fn a_clip_imported_beside_its_still_is_its_companion_and_leaves_the_grid() {
    let root = temp_dir("pair");
    let mut s = Session::new().with_fs();
    s.open_library(root.join("lib"), false).unwrap();
    let dir = root.join("phone");
    png(&dir.join("IMG_1.png"), 1);
    png(&dir.join("IMG_2.png"), 2);
    std::fs::write(dir.join("IMG_1.MOV"), lightcraft_video::testdata::clip(20, 10, 0)).unwrap();
    let r = s.execute("library.import", &json!({"paths": [dir.to_string_lossy()]})).unwrap();
    assert_eq!(r["imported"].as_array().map(Vec::len), Some(3), "{r}");

    let id = |name: &str| s.catalog.photos().find(|p| p.file_name == name).map(|p| p.id.0).unwrap();
    let (still, clip, other) = (id("IMG_1.png"), id("IMG_1.MOV"), id("IMG_2.png"));
    let c = s.execute("live.companion", &json!({"id": still})).unwrap();
    assert_eq!((c["clip"].as_u64(), c["available"].as_bool(), c["remote"].as_bool()), (Some(clip), Some(true), Some(false)), "{c}");
    assert!(c["path"].as_str().is_some_and(|p| p.ends_with("IMG_1.MOV")), "{c}");
    assert!(s.execute("live.companion", &json!({"id": other})).unwrap().is_null(), "a photo with no clip");

    let shown: Vec<u64> = s.visible().iter().map(|p| p.0).collect();
    assert!(shown.contains(&still) && shown.contains(&other) && !shown.contains(&clip), "the grid lists {shown:?}");
}
