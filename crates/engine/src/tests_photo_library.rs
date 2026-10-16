//! Photos opened from the system photo library (`photoLibrary.*`): a Local record until the
//! user changes it, then a library photo in the same undo step; forgotten, file and all, when
//! left unchanged.

use std::path::{Path, PathBuf};

use lightcraft_catalog::{Op, PhotoId, Source};
use serde_json::{Value, json};

use crate::{Selection, Session};

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-photo-library-{tag}-{}", std::process::id()));
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

/// A session with a library in `root/lib` and the app's staging folder `root/staging`.
fn session(root: &Path) -> Session {
    let mut s = Session::new().with_fs();
    s.open_library(root.join("lib"), false).unwrap();
    s.set_photo_library_staging(Some(root.join("staging")));
    s
}

/// The app fetched asset `asset`'s original into the staging folder.
fn staged(root: &Path, name: &str, seed: u8) -> String {
    let p = root.join("staging").join(format!("{seed}")).join(name);
    png(&p, seed);
    p.to_string_lossy().to_string()
}

fn open(s: &mut Session, asset: &str, path: &str) -> Value {
    s.execute("photoLibrary.open", &json!({"asset": asset, "path": path})).unwrap()
}

fn id_of(v: &Value) -> PhotoId {
    PhotoId(v["id"].as_u64().unwrap())
}

fn file_of(s: &Session, id: PhotoId) -> String {
    match &s.catalog.photo(id).unwrap().source {
        Source::File { path } => path.clone(),
        other => panic!("not a file: {other:?}"),
    }
}

#[test]
fn an_opened_photo_is_shown_but_stays_out_of_the_library() {
    let root = temp_dir("open");
    let mut s = session(&root);
    let path = staged(&root, "IMG_1.png", 1);
    let r = open(&mut s, "A1/L0/001", &path);
    let id = id_of(&r);
    assert_eq!((r["local"].as_bool(), r["known"].as_bool()), (Some(true), Some(false)), "{r}");
    assert!(s.catalog.photo(id).unwrap().local);
    assert_eq!(s.selection.active, Some(id), "it is the photo shown");
    assert!(!s.visible().contains(&id), "not in All Photos");
    assert!(s.undo.is_empty(), "opening is no step to undo");
    assert!(!Path::new(&path).exists(), "the staged copy is moved");
    let file = file_of(&s, id);
    assert!(file.starts_with(&root.join("lib").join("Originals").to_string_lossy().to_string()), "{file}");
    assert!(Path::new(&file).exists());
    assert_eq!(s.execute("photoLibrary.added", &json!({})).unwrap(), json!({}), "nothing in the library yet");
}

#[test]
fn a_change_brings_it_into_the_library_in_the_same_step() {
    let root = temp_dir("adopt");
    let mut s = session(&root);
    let id = id_of(&open(&mut s, "A1", &staged(&root, "IMG_1.png", 1)));
    s.execute("photo.rate", &json!({"rating": 4})).unwrap();
    assert!(!s.catalog.photo(id).unwrap().local, "rated: in the library");
    assert_eq!(s.undo.len(), 1, "one step");
    assert!(s.visible().contains(&id));
    assert_eq!(s.execute("photoLibrary.added", &json!({})).unwrap(), json!({"A1": id.0}));
    // one step: undoing it undoes the rating, then takes the photo back out (the promotion went first: a server
    // learns of the photo before its rating)
    match s.undo.last().map(|u| &u.op) {
        Some(Op::Batch { ops }) => {
            assert!(matches!(ops.last(), Some(Op::Batch { ops: p }) if matches!(p.first(), Some(Op::SetLocal { local: true, .. }))), "{ops:?}")
        }
        other => panic!("{other:?}"),
    }
    s.execute("edit.undo", &json!({})).unwrap();
    let p = s.catalog.photo(id).unwrap();
    assert!(p.local && p.rating == 0, "undo takes it back out with its rating");
    s.execute("edit.redo", &json!({})).unwrap();
    assert!(!s.catalog.photo(id).unwrap().local);
}

#[test]
fn a_slider_drag_brings_it_in_once() {
    let root = temp_dir("drag");
    let mut s = session(&root);
    let id = id_of(&open(&mut s, "A1", &staged(&root, "IMG_1.png", 1)));
    s.execute("develop.beginInteraction", &json!({"label": "Exposure"})).unwrap();
    for v in [0.2, 0.5, 0.8] {
        s.execute("develop.set", &json!({"control": "light.exposure", "value": v})).unwrap();
        assert!(s.catalog.photo(id).unwrap().local, "not while dragging");
    }
    s.execute("develop.endInteraction", &json!({})).unwrap();
    assert!(!s.catalog.photo(id).unwrap().local);
    assert_eq!(s.undo.len(), 1);
}

#[test]
fn leaving_it_unchanged_forgets_the_record_and_the_file() {
    let root = temp_dir("leave");
    let mut s = session(&root);
    png(&root.join("mine.png"), 9);
    s.execute("library.import", &json!({"paths": [root.join("mine.png").to_string_lossy()]})).unwrap();
    let mine = s.catalog.photos().next().unwrap().id;
    s.selection = Selection::single(mine);
    let id = id_of(&open(&mut s, "A1", &staged(&root, "IMG_1.png", 1)));
    let file = file_of(&s, id);
    let r = s.execute("photoLibrary.close", &json!({"id": id.0})).unwrap();
    assert_eq!(r, json!({"promoted": false, "forgotten": true}));
    assert!(s.catalog.photo(id).is_none() && !Path::new(&file).exists(), "record and file gone");
    assert_eq!(s.selection.active, Some(mine), "the library's photo is selected again");
    assert!(s.execute("photoLibrary.added", &json!({})).unwrap().as_object().unwrap().is_empty());
}

#[test]
fn leaving_it_changed_keeps_it_and_says_so() {
    let root = temp_dir("keep");
    let mut s = session(&root);
    let id = id_of(&open(&mut s, "A1", &staged(&root, "IMG_1.png", 1)));
    s.execute("photo.flag", &json!({"flag": "pick"})).unwrap();
    let r = s.execute("photoLibrary.close", &json!({"id": id.0})).unwrap();
    assert_eq!(r, json!({"promoted": true, "forgotten": false}));
    assert!(Path::new(&file_of(&s, id)).exists());
}

#[test]
fn an_asset_opened_again_is_the_same_photo() {
    let root = temp_dir("again");
    let mut s = session(&root);
    let first = id_of(&open(&mut s, "A1", &staged(&root, "IMG_1.png", 1)));
    s.execute("photo.rate", &json!({"rating": 2})).unwrap();
    let again = staged(&root, "IMG_1.png", 1);
    let r = open(&mut s, "A1", &again);
    assert_eq!((id_of(&r), r["known"].as_bool()), (first, Some(true)));
    assert!(!Path::new(&again).exists(), "the second fetch is dropped");
    assert_eq!(s.catalog.photos().count(), 1);
}

#[test]
fn a_photo_the_library_already_has_opens_as_that_photo() {
    let root = temp_dir("dup");
    let mut s = session(&root);
    png(&root.join("picked.png"), 3);
    s.execute("library.import", &json!({"paths": [root.join("picked.png").to_string_lossy()]})).unwrap();
    let mine = s.catalog.photos().next().unwrap().id;
    let path = staged(&root, "IMG_3.png", 3);
    let r = open(&mut s, "A3", &path);
    assert_eq!((id_of(&r), r["local"].as_bool()), (mine, Some(false)), "{r}");
    assert_eq!(s.catalog.photos().count(), 1, "no second record");
    assert!(!Path::new(&path).exists());
    assert_eq!(s.execute("photoLibrary.added", &json!({})).unwrap(), json!({"A3": mine.0}));
    // leaving it never removes a library photo
    assert_eq!(s.execute("photoLibrary.close", &json!({"id": mine.0})).unwrap(), json!({"promoted": true, "forgotten": false}));
    assert!(s.catalog.photo(mine).is_some());
}

#[test]
fn opening_the_library_again_tidies_what_was_left_open() {
    let root = temp_dir("tidy");
    let (left, kept, left_file) = {
        let mut s = session(&root);
        let kept = id_of(&open(&mut s, "A2", &staged(&root, "IMG_2.png", 2)));
        s.execute("photo.rate", &json!({"rating": 1})).unwrap();
        let left = id_of(&open(&mut s, "A1", &staged(&root, "IMG_1.png", 1)));
        let file = file_of(&s, left);
        s.persist().unwrap();
        (left, kept, file)
    };
    let s = session(&root);
    assert!(s.catalog.photo(left).is_none() && !Path::new(&left_file).exists(), "the untouched one is gone");
    assert!(s.catalog.photo(kept).is_some_and(|p| !p.local), "the rated one stays in the library");
}

#[test]
fn auto_advance_stays_on_a_photo_library_photo() {
    let root = temp_dir("advance");
    let mut s = session(&root);
    for (i, n) in ["a.png", "b.png"].iter().enumerate() {
        png(&root.join(n), 20 + i as u8);
        s.execute("library.import", &json!({"paths": [root.join(n).to_string_lossy()]})).unwrap();
    }
    let id = id_of(&open(&mut s, "A1", &staged(&root, "IMG_1.png", 1)));
    s.execute("photo.rate", &json!({"rating": 3, "advance": true})).unwrap();
    // (rated, it is in the library now, but still the photo the photo-library screen shows)
    assert_eq!(s.selection.active, Some(id), "Auto Advance doesn't jump into the library's grid");
    s.execute("library.next", &json!({})).unwrap();
    assert_eq!(s.selection.active, Some(id));
    s.execute("photoLibrary.close", &json!({"id": id.0})).unwrap();
    let other = id_of(&open(&mut s, "A9", &staged(&root, "IMG_9.png", 9)));
    s.execute("library.next", &json!({})).unwrap();
    assert_eq!(s.selection.active, Some(other), "next doesn't jump into the library");
}

/// The photo selected when a photo-library photo is opened may itself be one brought in earlier (the library's
/// last photo): Auto Advance still stays on the one shown, and leaving gives the selection back.
#[test]
fn auto_advance_stays_when_the_selected_photo_came_from_the_photo_library_too() {
    let root = temp_dir("advance-again");
    let mut s = session(&root);
    for (i, n) in ["a.png", "b.png"].iter().enumerate() {
        png(&root.join(n), 30 + i as u8);
        s.execute("library.import", &json!({"paths": [root.join(n).to_string_lossy()]})).unwrap();
    }
    let first = id_of(&open(&mut s, "A1", &staged(&root, "IMG_1.png", 1)));
    s.execute("photo.rate", &json!({"rating": 2})).unwrap();
    s.execute("photoLibrary.close", &json!({"id": first.0})).unwrap();
    s.selection = Selection::single(first);
    let second = id_of(&open(&mut s, "A2", &staged(&root, "IMG_2.png", 2)));
    s.execute("photo.flag", &json!({"flag": "pick", "advance": true})).unwrap();
    assert_eq!(s.selection.active, Some(second));
    s.execute("photoLibrary.close", &json!({"id": second.0})).unwrap();
    assert_eq!(s.selection.active, Some(first), "the selection comes back");
}

#[test]
fn a_file_from_outside_the_staging_folder_is_copied() {
    let root = temp_dir("copy");
    let mut s = session(&root);
    let theirs = root.join("Pictures").join("IMG_5.png");
    png(&theirs, 5);
    let id = id_of(&open(&mut s, "A5", &theirs.to_string_lossy()));
    assert!(theirs.exists(), "a file that isn't the app's own is never removed");
    assert_ne!(file_of(&s, id), theirs.to_string_lossy());
}

#[test]
fn bad_params_are_errors() {
    let root = temp_dir("bad");
    let mut s = session(&root);
    let path = staged(&root, "IMG_1.png", 1);
    std::fs::write(root.join("staging").join("notes.txt"), b"hello").unwrap();
    let long = "x".repeat(10_000);
    for p in [
        json!({"path": path}),
        json!({"asset": "", "path": path}),
        json!({"asset": long, "path": path}),
        json!({"asset": "a\nb", "path": path}),
        json!({"asset": "A1", "path": root.join("nothing.png").to_string_lossy()}),
        json!({"asset": "A1", "path": root.join("staging").to_string_lossy()}),
        json!({"asset": "A1", "path": root.join("staging").join("notes.txt").to_string_lossy()}),
    ] {
        assert!(s.execute("photoLibrary.open", &p).is_err(), "{p}");
    }
    assert!(Path::new(&path).exists(), "a refused open leaves the staged file");
    assert!(s.execute("photoLibrary.close", &json!({})).is_err());
    assert_eq!(s.execute("photoLibrary.close", &json!({"id": 999})).unwrap(), json!({"promoted": false, "forgotten": false}));
    assert_eq!(s.catalog.photos().count(), 0);
}

#[test]
fn a_known_asset_opens_without_its_file_and_an_unknown_one_asks_for_it() {
    let root = temp_dir("nofile");
    let mut s = session(&root);
    assert_eq!(s.execute("photoLibrary.open", &json!({"asset": "A1"})).unwrap(), json!({"needsFile": true}));
    let id = id_of(&open(&mut s, "A1", &staged(&root, "IMG_1.png", 1)));
    s.execute("photo.label", &json!({"label": "red"})).unwrap();
    let r = s.execute("photoLibrary.open", &json!({"asset": "A1"})).unwrap();
    assert_eq!((id_of(&r), r["local"].as_bool()), (id, Some(false)), "{r}");
}

#[test]
fn opening_keeps_the_steps_to_redo() {
    let root = temp_dir("redo");
    let mut s = session(&root);
    png(&root.join("mine.png"), 9);
    s.execute("library.import", &json!({"paths": [root.join("mine.png").to_string_lossy()]})).unwrap();
    s.execute("photo.rate", &json!({"rating": 5, "ids": [s.catalog.photos().next().unwrap().id.0]})).unwrap();
    s.execute("edit.undo", &json!({})).unwrap();
    let (undo, redo) = (s.undo.len(), s.redo.len());
    open(&mut s, "A1", &staged(&root, "IMG_1.png", 1));
    assert_eq!((s.undo.len(), s.redo.len()), (undo, redo));
}

#[test]
fn picking_an_open_photo_in_the_photos_picker_brings_it_in() {
    let root = temp_dir("picker");
    let mut s = session(&root);
    let id = id_of(&open(&mut s, "A1", &staged(&root, "IMG_1.png", 1)));
    let picked = root.join("Import").join("IMG_1.png");
    png(&picked, 1);
    let r = s.execute("library.import", &json!({"paths": [picked.to_string_lossy()], "mode": "move"})).unwrap();
    assert!(!s.catalog.photo(id).unwrap().local, "{r}");
    assert_eq!(s.catalog.photos().count(), 1, "no second copy: {r}");
}

#[test]
fn bookkeeping_doesnt_keep_a_photo_only_looked_at() {
    let root = temp_dir("bookkeeping");
    let mut s = session(&root);
    let id = id_of(&open(&mut s, "A1", &staged(&root, "IMG_1.png", 1)));
    s.apply_system(Op::SetAsShotWb { id, wb: Some((5000.0, 0.0)) }).unwrap();
    assert!(s.catalog.photo(id).unwrap().local, "not brought in by LightCraft's own bookkeeping");
    assert_eq!(s.execute("photoLibrary.close", &json!({"id": id.0})).unwrap()["forgotten"], true);
}

#[test]
fn a_photo_held_by_redo_stays_until_the_library_opens_again() {
    let root = temp_dir("held");
    let id = {
        let mut s = session(&root);
        let id = id_of(&open(&mut s, "A1", &staged(&root, "IMG_1.png", 1)));
        s.execute("photo.rate", &json!({"rating": 2})).unwrap();
        s.execute("edit.undo", &json!({})).unwrap();
        assert!(s.catalog.photo(id).unwrap().local);
        assert_eq!(s.execute("photoLibrary.close", &json!({"id": id.0})).unwrap()["forgotten"], false, "redo still names it");
        s.persist().unwrap();
        id
    };
    let s = session(&root);
    assert!(s.catalog.photo(id).is_none());
}

/// Local folders are edited in place (Lightroom's Local): a change is written to the photo's sidecar beside it with
/// *Automatically write changes into XMP* off, while just browsing writes nothing; a photo-library photo never gets one.
#[test]
fn local_folder_edits_go_to_sidecars_and_browsing_writes_none() {
    let root = temp_dir("sidecars");
    let mut s = session(&root);
    assert!(!s.xmp.auto_write);
    let folder = root.join("Pictures");
    png(&folder.join("a.png"), 1);
    png(&folder.join("b.png"), 2);
    s.execute("library.browse", &json!({"path": folder.to_string_lossy()})).unwrap();
    assert!(!folder.join("a.xmp").exists() && !folder.join("b.xmp").exists(), "looking writes nothing");
    let a = s.catalog.photos().find(|p| file_of_path(p).ends_with("a.png")).unwrap().id;
    s.execute("photo.rate", &json!({"rating": 3, "ids": [a.0]})).unwrap();
    assert!(folder.join("a.xmp").exists(), "the rating is saved beside the photo");
    assert!(!folder.join("b.xmp").exists());
    // a library photo (not Local) still follows the preference
    s.execute("photo.addToLibrary", &json!({"ids": [a.0]})).unwrap();
    std::fs::remove_file(folder.join("a.xmp")).unwrap();
    s.execute("photo.rate", &json!({"rating": 4, "ids": [a.0]})).unwrap();
    assert!(!folder.join("a.xmp").exists());
    // and a photo-library photo writes none: it is brought in, and the preference is off
    let id = id_of(&open(&mut s, "A1", &staged(&root, "IMG_1.png", 7)));
    s.execute("photo.rate", &json!({"rating": 1})).unwrap();
    assert!(!Path::new(&file_of(&s, id)).with_extension("xmp").exists());
}

fn file_of_path(p: &lightcraft_catalog::Photo) -> String {
    match &p.source {
        Source::File { path } => path.clone(),
        _ => String::new(),
    }
}
