//! HB-532: files hashed from now on get the new content hash (XXH3-128); photos hashed before keep the old one
//! (SipHash-1-3) forever. A file the library has is found under either, a server that knows only the old one gets the
//! old one, and the same file under both hashes is one photo when two libraries join.

use super::*;

/// The old content hash of the file at `path`.
fn old_hash(path: &Path) -> String {
    lightcraft_preview::hash_bytes(&std::fs::read(path).unwrap()).to_string()
}

fn new_hash(path: &Path) -> String {
    lightcraft_preview::content_hash(&std::fs::read(path).unwrap()).to_string()
}

fn hash_of(s: &Session, id: PhotoId) -> String {
    s.catalog.photo(id).unwrap().content_hash.clone().unwrap()
}

/// Give every photo of `s` the old content hash of its file, as an import before HB-532 did (journaled, as it was).
fn hashed_before(s: &mut Session) {
    let photos: Vec<_> = s.catalog.photos().map(|p| (**p).clone()).collect();
    for p in photos {
        let Source::File { path } = &p.source else { continue };
        let op = Op::SetContent {
            id: p.id,
            width: p.width,
            height: p.height,
            file_size: p.file_size,
            content_hash: Some(old_hash(Path::new(path))),
            preview_only: p.preview_only.clone(),
            preview_checked: p.preview_checked.clone(),
        };
        s.apply_system(op).unwrap();
    }
}

#[test]
fn a_library_from_before_keeps_its_hashes_and_new_files_get_the_new_one() {
    let root = temp_dir("hb532-lib");
    write_png(&root.join("in/a.png"), 1);
    let mut s = open(&root.join("lib"));
    s.execute("library.import", &json!({"paths": [root.join("in").to_string_lossy()]})).unwrap();
    let a = photo_named(&s, "a.png");
    assert_eq!(hash_of(&s, a), new_hash(&root.join("in/a.png")), "a file imported now gets the new hash");
    hashed_before(&mut s);
    let old = old_hash(&root.join("in/a.png"));
    assert_eq!(hash_of(&s, a), old);
    // the same file again, elsewhere: found under its old hash, not a second photo
    std::fs::create_dir_all(root.join("again")).unwrap();
    std::fs::copy(root.join("in/a.png"), root.join("again/a-copy.png")).unwrap();
    let r = s.execute("library.import", &json!({"paths": [root.join("again").to_string_lossy()]})).unwrap();
    assert_eq!(s.catalog.len(), 1, "{r}");
    // read again from its file: still the hash it had
    s.execute("photo.reload", &json!({"ids": [a.0]})).unwrap();
    assert_eq!(hash_of(&s, a), old);
    // a new file: the new hash
    write_png(&root.join("more/b.png"), 2);
    s.execute("library.import", &json!({"paths": [root.join("more").to_string_lossy()]})).unwrap();
    let b = photo_named(&s, "b.png");
    assert_eq!(hash_of(&s, b), new_hash(&root.join("more/b.png")));
    // the library opened again: unchanged
    drop(s);
    let s = open(&root.join("lib"));
    assert_eq!((hash_of(&s, a), hash_of(&s, b)), (old, new_hash(&root.join("more/b.png"))));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_server_that_knows_only_the_old_hash_gets_it_for_new_files() {
    let mut f = Fake::new();
    f.old_hash = true;
    let (mut a, root, _) = first_device("hb532-oldserver", &mut f);
    assert!(a.new_files_get_old_hash(), "the server said nothing of the new hash");
    write_png(&root.join("more/c.png"), 3);
    a.execute("library.import", &json!({"paths": [root.join("more").to_string_lossy()]})).unwrap();
    let c = photo_named(&a, "c.png");
    assert_eq!(hash_of(&a, c), old_hash(&root.join("more/c.png")));
    assert_eq!(sync(&mut a, &mut f)["state"], "idle");
    assert!(f.blobs.contains_key(&("original".to_string(), old_hash(&root.join("more/c.png")))), "uploaded under the name the server checks");
    // the server is updated: new files get the new hash from its next answer on
    f.old_hash = false;
    sync(&mut a, &mut f);
    assert!(!a.new_files_get_old_hash());
    write_png(&root.join("more2/d.png"), 4);
    a.execute("library.import", &json!({"paths": [root.join("more2").to_string_lossy()]})).unwrap();
    assert_eq!(hash_of(&a, photo_named(&a, "d.png")), new_hash(&root.join("more2/d.png")));
    assert_eq!(hash_of(&a, c), old_hash(&root.join("more/c.png")), "and the photos before keep theirs");
    let _ = std::fs::remove_dir_all(&root);
}

/// The first device hashed its photos before HB-532; this library hashed the same file after. Joined, they are one
/// photo (the server's, under its old hash), with what this library knew about it: nothing is lost.
#[test]
fn the_same_file_under_both_hashes_is_one_photo_when_libraries_join() {
    let mut f = Fake::new();
    let root = temp_dir("hb532-join");
    write_png(&root.join("in/a.png"), 1);
    write_png(&root.join("in/b.png"), 2);
    let mut a = open(&root.join("a"));
    a.execute("library.import", &json!({"paths": [root.join("in").to_string_lossy()]})).unwrap();
    hashed_before(&mut a);
    sign_in(&mut a);
    assert_eq!(sync(&mut a, &mut f)["state"], "idle");
    let old = old_hash(&root.join("in/a.png"));
    let on_server = f.core.catalog().photos().find(|p| p.file_name == "a.png").map(|p| p.id).unwrap();
    assert_eq!(f.core.catalog().photo(on_server).unwrap().content_hash.as_deref(), Some(old.as_str()));

    // another library has the same file (`a-again.png`) under the new hash, rated, edited and in an album
    let mut c = second_library(&root);
    let again = photo_named(&c, "a-again.png");
    assert_eq!(hash_of(&c, again), new_hash(&root.join("in2/a-again.png")));
    c.selection = crate::Selection::single(again);
    c.execute("photo.rate", &json!({"rating": 4})).unwrap();
    c.execute("develop.set", &json!({"control": "light.exposure", "value": 0.5})).unwrap();
    c.execute("album.create", &json!({"name": "Mine", "addSelected": true})).unwrap();
    sign_in(&mut c);
    let st = sync(&mut c, &mut f);
    assert_eq!(st["state"], "conflict", "{st}");
    assert_eq!(st["conflict"]["shared"], 1, "the choice counts it as a photo both have: {st}");
    let r = c.execute("sync.resolveConflict", &json!({"choice": "upload"})).unwrap();
    assert_eq!((&r["resolved"]["joined"]["photosAdded"], &r["resolved"]["joined"]["photosShared"]), (&json!(1), &json!(1)), "{r}");
    assert_eq!(sync(&mut c, &mut f)["state"], "idle");

    // one photo for the file, the server's, under its old hash; with this library's rating, edit and album
    assert_eq!(f.core.catalog().len(), 3);
    assert_eq!(c.catalog.len(), 3);
    let p = f.core.catalog().photo(on_server).unwrap();
    assert_eq!((p.content_hash.as_deref(), p.rating, p.develop.light.exposure), (Some(old.as_str()), 4, 0.5));
    let mine: Vec<_> = f.core.catalog().albums().filter(|a| a.name == "Mine").collect();
    assert_eq!((mine.len(), mine[0].photos.clone()), (1, vec![on_server]));
    // here it shows from this device's file, and it is not sent up again
    let here = c.catalog.photo(on_server).unwrap();
    assert_eq!(here.content_hash.as_deref(), Some(old.as_str()));
    assert!(!is_remote(here), "this device's file");
    assert_eq!(f.blobs.keys().filter(|(k, _)| k == "original").count(), 3, "{:?}", f.blobs.keys().collect::<Vec<_>>());
    assert!(!f.blobs.contains_key(&("original".to_string(), new_hash(&root.join("in2/a-again.png")))));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn merging_at_sign_in_keeps_a_file_hashed_both_ways_as_one_photo_too() {
    let mut f = Fake::new();
    let root = temp_dir("hb532-merge");
    write_png(&root.join("in/a.png"), 1);
    let mut a = open(&root.join("a"));
    a.execute("library.import", &json!({"paths": [root.join("in").to_string_lossy()]})).unwrap();
    hashed_before(&mut a);
    sign_in(&mut a);
    assert_eq!(sync(&mut a, &mut f)["state"], "idle");
    let mut c = second_library(&root);
    let again = photo_named(&c, "a-again.png");
    c.selection = crate::Selection::single(again);
    c.execute("photo.rate", &json!({"rating": 3})).unwrap();
    c.execute("sync.signIn", &json!({"server": "http://fake", "user": "ann", "password": "pw", "device": "test", "merge": true})).unwrap();
    let st = sync(&mut c, &mut f);
    assert_ne!(st["state"], "conflict", "{st}");
    sync(&mut c, &mut f);
    assert_eq!(f.core.catalog().len(), 2, "a.png once, and c.png");
    let p = f.core.catalog().photos().find(|p| p.file_name == "a.png").unwrap();
    assert_eq!((p.content_hash.clone(), p.rating), (Some(old_hash(&root.join("in/a.png"))), 3));
    let _ = std::fs::remove_dir_all(&root);
}
