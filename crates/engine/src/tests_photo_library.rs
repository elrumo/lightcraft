//! Photos opened from the system photo library (`photoLibrary.*`): a Local record until the
//! user changes it, then a library photo in the same undo step; forgotten, file and all, when
//! left unchanged.

use std::path::{Path, PathBuf};

use lightcraft_catalog::{Op, OriginKind, PhotoId, Source};
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
fn a_rating_does_not_bring_it_into_the_library() {
    let root = temp_dir("adopt");
    let mut s = session(&root);
    let id = id_of(&open(&mut s, "A1", &staged(&root, "IMG_1.png", 1)));
    s.execute("photo.rate", &json!({"rating": 4})).unwrap();
    assert!(s.catalog.photo(id).unwrap().local, "rating alone does not add it");
    assert!(!s.visible().contains(&id));
    assert!(s.execute("photoLibrary.added", &json!({})).unwrap().as_object().unwrap().is_empty());
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
fn leaving_a_rated_photo_does_not_promote_it() {
    let root = temp_dir("keep");
    let mut s = session(&root);
    let id = id_of(&open(&mut s, "A1", &staged(&root, "IMG_1.png", 1)));
    s.execute("photo.flag", &json!({"flag": "pick"})).unwrap();
    let r = s.execute("photoLibrary.close", &json!({"id": id.0})).unwrap();
    assert_eq!(r, json!({"promoted": false, "forgotten": false}));
    assert!(s.catalog.photo(id).is_some_and(|p| p.local));
    assert!(Path::new(&file_of(&s, id)).exists());
}

#[test]
fn resetting_before_leaving_does_not_promote_a_photo() {
    let root = temp_dir("reset-before-leave");
    let mut s = session(&root);
    let id = id_of(&open(&mut s, "A1", &staged(&root, "IMG_1.png", 1)));
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 1.0})).unwrap();
    s.execute("develop.reset", &json!({})).unwrap();
    let r = s.execute("photoLibrary.close", &json!({"id": id.0})).unwrap();
    assert_eq!(r, json!({"promoted": false, "forgotten": false}));
    assert!(s.catalog.photo(id).is_some_and(|p| p.local && !p.is_edited()));
    assert!(s.execute("photoLibrary.added", &json!({})).unwrap().as_object().unwrap().is_empty());
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
    assert_eq!(s.execute("photoLibrary.close", &json!({"id": mine.0})).unwrap(), json!({"promoted": false, "forgotten": false}));
    assert!(s.catalog.photo(mine).is_some());
}

#[test]
fn opening_the_library_again_forgets_a_rated_but_unedited_photo() {
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
    assert!(s.catalog.photo(kept).is_none(), "rating did not add the photo to the library");
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
    assert_eq!((id_of(&r), r["local"].as_bool()), (id, Some(true)), "{r}");
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

/// Keep Originals Where They Are: the engine's copy goes to the cache, not `Originals/`, and the photo carries where its
/// original is.
#[test]
fn a_kept_photo_is_copied_to_the_cache_not_the_originals() {
    let root = temp_dir("kept-cache");
    let mut s = session(&root);
    s.execute("library.preferences", &json!({"keepInPlace": true})).unwrap();
    let r = s.execute("photoLibrary.open", &json!({"asset": "A1", "path": staged(&root, "IMG_1.png", 1), "cloud": "C1"})).unwrap();
    let id = id_of(&r);
    let file = file_of(&s, id);
    assert!(file.starts_with(&root.join("lib").join(crate::photo_library::CACHE_DIR).to_string_lossy().to_string()), "{file}");
    assert!(!root.join("lib/Originals").exists(), "nothing in the library's originals");
    let o = s.catalog.photo(id).unwrap().origin.clone().unwrap();
    assert_eq!((o.kind, o.cloud.as_deref()), (OriginKind::Photos, Some("C1")));
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 0.5})).unwrap();
    let p = s.catalog.photo(id).unwrap();
    assert!(!p.local && p.origin.is_some(), "brought in by the edit, with its origin");
}

/// The copy of a kept photo can go (trimmed, or the device's storage cleared): opening it again asks for the file,
/// and the file the app fetches again is checked and taken, edits and all.
#[test]
fn a_kept_photo_whose_copy_is_gone_is_fetched_again() {
    let root = temp_dir("kept-again");
    let mut s = session(&root);
    s.execute("library.preferences", &json!({"keepInPlace": true})).unwrap();
    let id = id_of(&open(&mut s, "A1", &staged(&root, "IMG_1.png", 1)));
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 0.5})).unwrap();
    s.execute("photoLibrary.close", &json!({"id": id.0})).unwrap();
    // (the leave may have trimmed it already)
    let _ = std::fs::remove_file(file_of(&s, id));
    assert!(crate::cmd::missing::missing(&s).is_empty(), "not a missing photo: its original is in the photo library");
    let undo = s.undo.len();
    assert_eq!(s.execute("photoLibrary.open", &json!({"asset": "A1"})).unwrap(), json!({"needsFile": true}));
    // another picture under that asset is refused, and changes nothing
    assert!(s.execute("photoLibrary.open", &json!({"asset": "A1", "path": staged(&root, "IMG_1.png", 2)})).is_err());
    assert!(!Path::new(&file_of(&s, id)).exists());
    let r = open(&mut s, "A1", &staged(&root, "IMG_1.png", 1));
    assert_eq!((id_of(&r), r["known"].as_bool()), (id, Some(true)));
    assert!(Path::new(&file_of(&s, id)).is_file());
    assert_eq!(s.catalog.photo(id).unwrap().develop.light.exposure, 0.5);
    assert_eq!(s.undo.len(), undo, "no step to undo");
}

/// Over the cache's budget, the copies of photos left go once their smart preview can stand in for them.
#[test]
fn leaving_a_kept_photo_trims_its_copy_once_its_smart_preview_is_there() {
    let root = temp_dir("kept-trim");
    let mut s = session(&root);
    s.execute("library.preferences", &json!({"keepInPlace": true})).unwrap();
    let id = id_of(&open(&mut s, "A1", &staged(&root, "IMG_1.png", 1)));
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 0.5})).unwrap();
    s.execute("library.smartPreviews", &json!({"ids": [id.0]})).unwrap();
    let file = file_of(&s, id);
    s.execute("photoLibrary.close", &json!({"id": id.0})).unwrap();
    assert!(!Path::new(&file).exists(), "trimmed (the test budget is a byte)");
    assert!(s.render_now(id, 64, 64).is_ok(), "drawn from its smart preview");
}

/// Added in place from a folder with Keep Originals Where They Are on: the photo says so; a copy doesn't.
#[test]
fn files_added_in_place_are_kept_where_they_are() {
    let root = temp_dir("kept-folder");
    let mut s = session(&root);
    s.execute("library.preferences", &json!({"keepInPlace": true})).unwrap();
    png(&root.join("Pictures/a.png"), 1);
    png(&root.join("Pictures/b.png"), 2);
    s.execute("library.import", &json!({"paths": [root.join("Pictures/a.png").to_string_lossy()]})).unwrap();
    s.execute("library.import", &json!({"paths": [root.join("Pictures/b.png").to_string_lossy()], "mode": "copy"})).unwrap();
    let origin = |name: &str| s.catalog.photos().find(|p| p.file_name == name).unwrap().origin.clone();
    assert_eq!(origin("a.png").map(|o| (o.kind, o.device)), Some((OriginKind::Folder, 0)));
    assert_eq!(origin("b.png"), None);
    // its edits are written beside it, with *Automatically write changes into XMP* off
    assert!(!s.xmp.auto_write);
    let a = s.catalog.photos().find(|p| p.file_name == "a.png").unwrap().id;
    s.execute("photo.rate", &json!({"rating": 3, "ids": [a.0]})).unwrap();
    assert!(root.join("Pictures/a.xmp").exists());
    // the setting is saved with the library
    s.close_library().unwrap();
    // (a new session: the old one lets go of the library first)
    drop(s);
    let s = session(&root);
    assert!(s.keep_in_place);
}

/// A kept photo shown in the library (not opened from the photo library) whose copy is gone: the app asks which asset
/// to fetch and hands its original over without changing what is shown.
#[test]
fn the_library_asks_for_a_kept_photos_original_and_takes_it() {
    let root = temp_dir("kept-attach");
    let mut s = session(&root);
    s.execute("library.preferences", &json!({"keepInPlace": true})).unwrap();
    let id = id_of(&s.execute("photoLibrary.open", &json!({"asset": "A1", "path": staged(&root, "IMG_1.png", 1), "cloud": "C1"})).unwrap());
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 0.5})).unwrap();
    s.execute("photoLibrary.close", &json!({"id": id.0})).unwrap();
    let _ = std::fs::remove_file(file_of(&s, id));
    assert_eq!(s.execute("photoLibrary.wanted", &json!({"id": id.0})).unwrap(), json!({"asset": "A1", "cloud": "C1"}));
    s.selection = Selection::single(id);
    let r = s.execute("photoLibrary.attach", &json!({"id": id.0, "asset": "A1", "path": staged(&root, "IMG_1.png", 1)})).unwrap();
    assert_eq!(r, json!({"attached": true}));
    assert!(Path::new(&file_of(&s, id)).is_file());
    assert_eq!(s.execute("photoLibrary.wanted", &json!({"id": id.0})).unwrap(), json!({}), "nothing wanted now");
    assert_eq!(s.selection, Selection::single(id), "what is shown is as it was");
    assert!(s.photo_library_return.is_none(), "not a visit to the photo library");
}
