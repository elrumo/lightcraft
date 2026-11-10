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
fn status_flags_keep_the_requested_order_and_sensitive_state() {
    let root = temp_dir("flags");
    let (mut s, [titled, _, _, plain]) = library(&root);
    s.execute("photo.markSensitive", &json!({"ids": [titled.0]})).unwrap();

    let flags = s.execute("catalog.flags", &json!({"ids": [plain.0, titled.0]})).unwrap();
    assert_eq!(
        flags["photos"],
        json!([
            {"id": plain.0, "live": false, "sensitive": false},
            {"id": titled.0, "live": false, "sensitive": true},
        ])
    );
}

#[test]
fn bad_arguments_are_errors() {
    let mut s = Session::with_demo();
    for p in [json!({"limit": 0}), json!({"limit": 1001}), json!({"limit": "x"}), json!({"ids": 5}), json!({"ids": vec![1; 1001]})] {
        assert!(s.execute("catalog.describe", &p).is_err(), "{p}");
    }
    assert!(s.execute("catalog.describe", &json!({"limit": null, "ids": null})).is_ok());
    for p in
        [json!({}), json!({"ids": 5}), json!({"ids": [1, "x"]}), json!({"ids": [1, 1]}), json!({"ids": [u64::MAX]}), json!({"ids": vec![1; 1001]})]
    {
        assert!(s.execute("catalog.flags", &p).is_err(), "{p}");
    }
    assert_eq!(s.execute("catalog.flags", &json!({"ids": []})).unwrap(), json!({"photos": []}));
}

#[test]
fn a_photos_position_is_its_offset_in_the_view() {
    // (HB-287: the photo screen opened from Years or Months, before the grid read the page that holds the photo)
    let mut s = Session::with_demo();
    let view = s.visible_cloned();
    assert!(view.len() > 2, "the demo library has photos");
    let last = view.len() - 1;
    for i in [0, 1, last] {
        let r = s.execute("catalog.position", &json!({"id": view[i].0})).unwrap();
        assert_eq!(r, json!({"index": i}));
        // (the offset `catalog.query` pages from)
        let q = s.execute("catalog.query", &json!({"offset": i, "limit": 1})).unwrap();
        assert_eq!(ids(&q), vec![view[i].0]);
    }
    // one the view does not show: none
    s.execute("library.filter", &json!({"rating": 5})).unwrap();
    let hidden = view.iter().find(|id| !s.visible_cloned().contains(id)).copied().unwrap();
    assert_eq!(s.execute("catalog.position", &json!({"id": hidden.0})).unwrap(), json!({"index": null}));
    assert_eq!(s.execute("catalog.position", &json!({"id": u64::MAX})).unwrap(), json!({"index": null}));
    for p in [json!({}), json!({"id": "x"}), json!({"id": -1})] {
        assert!(s.execute("catalog.position", &p).is_err(), "{p}");
    }
}

/// `n` photos added straight to the catalog, ids 1 to n with gaps: every 7th hidden, every 11th deleted, every 13th Local.
fn synthetic(n: u64) -> (Session, Vec<u64>) {
    use lightcraft_catalog::{Op, Photo, Source};
    let mut s = Session::new();
    let mut shown = Vec::new();
    for i in 1..=n {
        // (ids with gaps, as deletes and other devices' id spaces leave them)
        let id = i * 3;
        let name = format!("IMG_{i}.jpg");
        let mut p = Photo::new(PhotoId(id), Source::File { path: name.clone() }, &name, "JPG", 16, 12, "2026-11-10T00:00:00Z");
        (p.hidden, p.deleted, p.local) = (i % 7 == 0, i % 11 == 0, i % 13 == 0);
        if p.visible() {
            shown.push(id);
        }
        s.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    }
    (s, shown)
}

/// A whole pass by the cursor, `limit` at a time: the ids, and how many requests it took.
fn cursor_pass(s: &mut Session, limit: usize) -> (Vec<u64>, usize) {
    let (mut out, mut after, mut pages) = (Vec::new(), json!(0), 0);
    loop {
        let r = s.execute("catalog.describe", &json!({"after": after, "limit": limit})).unwrap();
        pages += 1;
        assert!(r.get("total").is_none(), "a cursor page counts nothing: {}", r["next"]);
        out.extend(ids(&r));
        if r["next"].is_null() {
            return (out, pages);
        }
        assert_eq!(r["next"], json!(out.last().unwrap()), "the cursor is the page's last photo");
        after = r["next"].clone();
    }
}

#[test]
fn a_cursor_pages_a_synthetic_catalog_as_offsets_do() {
    // (HB-282: the Spotlight pass pages a big library by cursor)
    let (mut s, shown) = synthetic(2_345);
    let all = s.execute("catalog.describe", &json!({"limit": 1000})).unwrap();
    assert_eq!(all["total"], json!(shown.len()), "no cursor: the reply is as before");
    assert_eq!(ids(&all), shown[..1000]);
    let mut by_offset = Vec::new();
    while by_offset.len() < shown.len() {
        let r = s.execute("catalog.describe", &json!({"offset": by_offset.len(), "limit": 300})).unwrap();
        by_offset.extend(ids(&r));
    }
    let (by_cursor, pages) = cursor_pass(&mut s, 300);
    assert_eq!(by_cursor, shown, "every photo a system index may show, once, in id order");
    assert_eq!(by_cursor, by_offset);
    assert_eq!(pages, shown.len().div_ceil(300), "a short last page ends the pass");
    // a page that ends on the last photo still says `next`; the one after it is empty and ends the pass
    let exact = shown.len() - 1;
    let r = s.execute("catalog.describe", &json!({"after": shown[exact - 1], "limit": 1})).unwrap();
    assert_eq!((ids(&r), r["next"].clone()), (vec![shown[exact]], json!(shown[exact])));
    let r = s.execute("catalog.describe", &json!({"after": shown[exact], "limit": 1})).unwrap();
    assert_eq!((ids(&r), r["next"].clone()), (vec![], Value::Null));
    // a cursor between ids, or on a hidden photo's, starts at the next photo shown
    let r = s.execute("catalog.describe", &json!({"after": shown[4] - 1, "limit": 2})).unwrap();
    assert_eq!(ids(&r), shown[4..6]);
    let r = s.execute("catalog.describe", &json!({"after": u64::MAX})).unwrap();
    assert_eq!((ids(&r), r["next"].clone()), (vec![], Value::Null));
    // `ids` still wins over a cursor
    let r = s.execute("catalog.describe", &json!({"ids": [shown[9]], "after": 0})).unwrap();
    assert_eq!(ids(&r), vec![shown[9]]);
    for p in [json!({"after": "x"}), json!({"after": -1}), json!({"after": 1.5}), json!({"after": 0, "offset": 3}), json!({"after": 0, "limit": 0})] {
        assert!(s.execute("catalog.describe", &p).is_err(), "{p}");
    }
    assert!(s.execute("catalog.describe", &json!({"after": null, "offset": null})).unwrap().get("total").is_some());
}

/// The card's measurement: `cargo test -p lightcraft-engine --release -- --ignored a_pass_over_200k --nocapture`.
#[test]
#[ignore = "timing, 200k photos"]
fn a_pass_over_200k_photos_walks_the_catalog_once_by_cursor() {
    let (mut s, shown) = synthetic(200_000);
    let t = std::time::Instant::now();
    let mut by_offset = 0;
    while by_offset < shown.len() {
        by_offset += ids(&s.execute("catalog.describe", &json!({"offset": by_offset, "limit": 500})).unwrap()).len();
    }
    let offset_ms = t.elapsed().as_secs_f64() * 1e3;
    let t = std::time::Instant::now();
    let (by_cursor, pages) = cursor_pass(&mut s, 500);
    let cursor_ms = t.elapsed().as_secs_f64() * 1e3;
    // the pages alone, without describing a photo: what the offsets' walks cost on top
    let t = std::time::Instant::now();
    for page in 0..pages {
        let _ = s.catalog.photos().filter(|p| p.visible()).count();
        let _ = s.catalog.photos().filter(|p| p.visible()).skip(page * 500).take(500).count();
    }
    let walks_ms = t.elapsed().as_secs_f64() * 1e3;
    eprintln!(
        "200k photos, {} shown, {pages} pages of 500: offset pass {offset_ms:.0} ms, cursor pass {cursor_ms:.0} ms (the offsets' walks alone {walks_ms:.0} ms)",
        shown.len()
    );
    assert_eq!(by_cursor.len(), shown.len());
    assert!(cursor_ms < offset_ms, "the cursor pass is the cheaper one");
}
