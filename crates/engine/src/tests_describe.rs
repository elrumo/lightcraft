//! `catalog.describe`: what a system index (Spotlight, Shortcuts) is told of the library's photos, and never of a
//! hidden, deleted or Local one.

use std::path::{Path, PathBuf};

use lightcraft_catalog::PhotoId;
use serde_json::{Value, json};

use crate::Session;

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-describe-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn png(p: &Path, seed: u8) {
    let img = lightcraft_raster::Rgba8::from_fn(16, 12, |x, y| [(x * 9) as u8, (y * 11) as u8, seed, 255]);
    let b =
        crate::export::encode_image(&img, &crate::export::ExportOptions { format: crate::export::ExportFormat::Png, ..Default::default() }).unwrap();
    std::fs::write(p, b).unwrap();
}

fn ids(v: &Value) -> Vec<u64> {
    v["photos"].as_array().unwrap().iter().map(|p| p["id"].as_u64().unwrap()).collect()
}

/// Four photos: one titled and placed, one hidden, one deleted, one plain.
fn library(root: &Path) -> (Session, [PhotoId; 4]) {
    let mut s = Session::new().with_fs();
    s.open_library(root.join("lib"), false).unwrap();
    let mut out = Vec::new();
    for i in 0..4u8 {
        let f = root.join(format!("IMG_{i}.png"));
        png(&f, i);
        let r = s.execute("library.import", &json!({"paths": [f.to_string_lossy()]})).unwrap();
        out.push(PhotoId(r["imported"][0].as_u64().unwrap()));
    }
    let [titled, hidden, deleted, plain] = [out[0], out[1], out[2], out[3]];
    s.execute("photo.setMeta", &json!({"ids": [titled.0], "title": "Harbour at dusk", "caption": "Boats", "city": "Porto", "keywords": ["sea"]}))
        .unwrap();
    s.execute("photo.hide", &json!({"ids": [hidden.0]})).unwrap();
    s.execute("photo.delete", &json!({"ids": [deleted.0]})).unwrap();
    (s, [titled, hidden, deleted, plain])
}

#[test]
fn it_lists_the_library_and_never_a_hidden_or_deleted_photo() {
    let root = temp_dir("list");
    let (mut s, [titled, hidden, deleted, plain]) = library(&root);
    let r = s.execute("catalog.describe", &json!({})).unwrap();
    assert_eq!(ids(&r), vec![titled.0, plain.0], "{r}");
    assert_eq!(r["total"], json!(2));
    let first = &r["photos"][0];
    assert_eq!(
        (first["title"].as_str(), first["caption"].as_str(), first["city"].as_str()),
        (Some("Harbour at dusk"), Some("Boats"), Some("Porto")),
        "{first}"
    );
    assert_eq!(first["keywords"], json!(["sea"]));
    assert_eq!(
        (first["live"].as_bool(), first["sensitive"].as_bool(), first["lens"].clone()),
        (Some(false), Some(false), Value::Null),
        "empty text is null"
    );
    // by id: the same rule, whatever is asked for
    let r = s.execute("catalog.describe", &json!({"ids": [hidden.0, deleted.0, plain.0, 9_999, "x", -3]})).unwrap();
    assert_eq!(ids(&r), vec![plain.0]);
    // unlocked or not, Hidden stays out of a system index
    s.execute("library.showHidden", &json!({"on": true})).ok();
    assert!(!ids(&s.execute("catalog.describe", &json!({})).unwrap()).contains(&hidden.0));
}

#[test]
fn it_pages_in_id_order_and_says_when_the_library_changed() {
    let root = temp_dir("page");
    let (mut s, [titled, _, _, plain]) = library(&root);
    let a = s.execute("catalog.describe", &json!({"limit": 1})).unwrap();
    let b = s.execute("catalog.describe", &json!({"limit": 1, "offset": 1})).unwrap();
    assert_eq!((ids(&a), ids(&b)), (vec![titled.0], vec![plain.0]));
    assert!(ids(&s.execute("catalog.describe", &json!({"offset": u64::MAX})).unwrap()).is_empty());
    let before = a["revision"].as_u64().unwrap();
    s.execute("photo.rate", &json!({"ids": [plain.0], "rating": 3})).unwrap();
    let after = s.execute("catalog.describe", &json!({})).unwrap();
    assert!(after["revision"].as_u64().unwrap() > before);
    assert_eq!(after["photos"][1]["rating"], json!(3));
}

#[test]
fn bad_arguments_are_errors() {
    let mut s = Session::with_demo();
    for p in [json!({"limit": 0}), json!({"limit": 1001}), json!({"limit": "x"}), json!({"ids": 5}), json!({"ids": vec![1; 1001]})] {
        assert!(s.execute("catalog.describe", &p).is_err(), "{p}");
    }
    assert!(s.execute("catalog.describe", &json!({"limit": null, "ids": null})).is_ok());
}
