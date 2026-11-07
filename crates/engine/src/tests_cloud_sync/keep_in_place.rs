//! Keep Originals Where They Are with a sync server: a photo added in place sends its record, edits and previews,
//! never its original; the other devices show and edit it from the previews and never ask for the original.

use lightcraft_catalog::OriginKind;

use super::*;

fn key_of(s: &Session, id: PhotoId) -> String {
    blob_key(s.catalog.photo(id).unwrap()).unwrap()
}

#[test]
fn photos_kept_in_place_send_previews_never_the_original() {
    let mut f = Fake::new();
    let root = temp_dir("kept-in-place");
    write_png(&root.join("in/kept.png"), 1);
    write_png(&root.join("in/copied.png"), 2);
    let mut a = open(&root.join("a"));
    a.execute("library.preferences", &json!({"keepInPlace": true})).unwrap();
    sign_in(&mut a);
    sync(&mut a, &mut f);
    a.execute("library.import", &json!({"paths": [root.join("in/kept.png").to_string_lossy()]})).unwrap();
    a.execute("library.import", &json!({"paths": [root.join("in/copied.png").to_string_lossy()], "mode": "copy"})).unwrap();
    let id_of = |s: &Session, name: &str| s.catalog.photos().find(|p| p.file_name == name).unwrap().id;
    let (kept, copied) = (id_of(&a, "kept.png"), id_of(&a, "copied.png"));
    let origin = a.catalog.photo(kept).unwrap().origin.clone().expect("added in place: kept where it is");
    assert_eq!(origin.kind, OriginKind::Folder);
    assert_ne!(origin.device, 0, "the device that has it");
    assert!(a.catalog.photo(copied).unwrap().origin.is_none(), "a copy is the library's to keep");
    let st = sync(&mut a, &mut f);
    assert_eq!(st["state"], "idle", "{st}");

    let (k, c) = (key_of(&a, kept), key_of(&a, copied));
    let has = |kind: &str, key: &str| f.blobs.contains_key(&(kind.to_string(), key.to_string()));
    assert!(has("smart", &k) && has("mini", &k), "its previews are on the server");
    assert!(!has("original", &k), "its original is not");
    assert!(!f.requests.iter().any(|r| r.contains(&format!("/blobs/original/{k}"))), "never even asked about: {:?}", f.requests);
    assert!(has("original", &c), "the copied photo's original is");
    assert_eq!(f.core.catalog().photo(kept).unwrap().origin.as_ref(), Some(&origin), "the server knows where it is");
    let before = f.requests.len();
    sync(&mut a, &mut f);
    assert!(f.requests[before..].iter().all(|r| !r.starts_with("PUT") && !r.starts_with("HEAD")), "nothing left to send: {:?}", &f.requests[before..]);

    // the other device shows it from its previews, edits it, and never asks for the original
    let mut b = open(&root.join("b"));
    sign_in(&mut b);
    sync(&mut b, &mut f);
    assert_eq!(b.catalog.photo(kept).unwrap().origin.as_ref(), Some(&origin));
    assert_eq!(b.sync_want_originals(&[kept, copied]), 1, "only the copied photo's original can come");
    b.selection = crate::Selection::single(kept);
    b.thumb_job(kept, 128);
    sync(&mut b, &mut f);
    assert_eq!(b.original_status(kept), Some(json!({"state": "preview", "kept": origin})));
    assert!(b.render_now(kept, 128, 128).is_ok(), "drawn from its previews");
    b.execute("develop.set", &json!({"control": "light.exposure", "value": 0.5})).unwrap();
    sync(&mut b, &mut f);
    sync(&mut a, &mut f);
    assert_eq!(exposure(&a, kept), 0.5, "the edit reaches the device that has the original");
    assert!(!f.requests.iter().any(|r| r == &format!("GET /api/blobs/original/{k}")), "{:?}", f.requests);
    let _ = std::fs::remove_dir_all(&root);
}

/// A server older than `Photo.origin` would drop it, and the next reload from it would leave the photo with no origin:
/// photos added then keep the library's own copy and upload as before.
#[test]
fn a_server_too_old_for_origins_gets_originals_as_before() {
    let mut f = Fake::new();
    f.format = Some(12);
    let root = temp_dir("kept-old-server");
    write_png(&root.join("in/a.png"), 1);
    let mut a = open(&root.join("a"));
    a.execute("library.preferences", &json!({"keepInPlace": true})).unwrap();
    sign_in(&mut a);
    sync(&mut a, &mut f);
    assert!(!a.keeps_in_place());
    a.execute("library.import", &json!({"paths": [root.join("in/a.png").to_string_lossy()]})).unwrap();
    let id = a.catalog.photos().next().unwrap().id;
    assert!(a.catalog.photo(id).unwrap().origin.is_none());
    sync(&mut a, &mut f);
    assert!(f.blobs.contains_key(&("original".to_string(), key_of(&a, id))));
    let _ = std::fs::remove_dir_all(&root);
}
