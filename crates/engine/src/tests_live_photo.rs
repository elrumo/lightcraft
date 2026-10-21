//! Live Photos (`live.*`, `photoLibrary.open {motion}`, `library.import {motions}`): a photo's motion clip is kept in
//! the library's `Motion/` folder, listed in `live-photos.json`, and tidied away once its photo is gone for good.

use std::path::{Path, PathBuf};

use lightcraft_catalog::{MediaKind, PhotoId};
use serde_json::{Value, json};

use crate::Session;

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-live-photo-{tag}-{}", std::process::id()));
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

/// A 2 s clip, as a phone's motion is about.
fn clip(p: &Path) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, lightcraft_video::testdata::clip(20, 10, 0)).unwrap();
}

/// A session with a library in `root/lib` and the app's staging folder `root/staging`.
fn session(root: &Path) -> Session {
    let mut s = Session::new().with_fs();
    s.open_library(root.join("lib"), false).unwrap();
    s.set_photo_library_staging(Some(root.join("staging")));
    s
}

fn import(s: &mut Session, path: &Path) -> PhotoId {
    let r = s.execute("library.import", &json!({"paths": [path.to_string_lossy()]})).unwrap();
    PhotoId(r["imported"][0].as_u64().unwrap())
}

fn summary(s: &mut Session, id: PhotoId) -> Value {
    let r = s.execute("catalog.query", &json!({"filter": {}, "limit": 1000})).unwrap();
    r["photos"].as_array().unwrap().iter().find(|p| p["id"] == json!(id.0)).cloned().unwrap_or(Value::Null)
}

fn motion_file(s: &mut Session, id: PhotoId) -> Option<PathBuf> {
    s.execute("live.info", &json!({"id": id.0})).unwrap()["path"].as_str().map(PathBuf::from)
}

#[test]
fn a_staged_clip_becomes_the_photos_motion() {
    let root = temp_dir("attach");
    let mut s = session(&root);
    png(&root.join("IMG_1.png"), 1);
    let id = import(&mut s, &root.join("IMG_1.png"));
    assert_eq!(summary(&mut s, id).get("live"), None, "a still");
    let staged = root.join("staging").join("IMG_1.MOV");
    clip(&staged);
    let r = s.execute("live.attach", &json!({"id": id.0, "path": staged.to_string_lossy(), "contentId": "3F2A-11", "stillTime": 1.0})).unwrap();
    assert!(!staged.exists(), "the app's staged copy is moved");
    let path = PathBuf::from(r["path"].as_str().unwrap());
    assert!(path.starts_with(root.join("lib").join("Motion")) && path.is_file(), "{r}");
    assert!(r["duration"].as_f64().is_some_and(|d| (d - 2.0).abs() < 0.05), "{r}");
    assert_eq!((r["contentId"].as_str(), r["stillTime"].as_f64()), (Some("3F2A-11"), Some(1.0)));
    assert_eq!(summary(&mut s, id)["live"], json!(true));
    // the clip is no photo of its own, and no undo step
    assert_eq!(s.catalog.photos().count(), 1);
    assert_eq!(s.undo.len(), 1, "only the import");
    assert!(root.join("lib").join("live-photos.json").is_file());
}

#[test]
fn a_clip_of_the_persons_own_is_copied() {
    let root = temp_dir("copy");
    let mut s = session(&root);
    png(&root.join("IMG_1.png"), 1);
    let id = import(&mut s, &root.join("IMG_1.png"));
    let mine = root.join("IMG_1.MOV");
    clip(&mine);
    s.execute("live.attach", &json!({"id": id.0, "path": mine.to_string_lossy()})).unwrap();
    assert!(mine.exists(), "never removed from outside the staging folder");
    assert!(motion_file(&mut s, id).is_some_and(|p| p.is_file()));
}

#[test]
fn what_is_no_motion_is_refused() {
    let root = temp_dir("refuse");
    let mut s = session(&root);
    png(&root.join("IMG_1.png"), 1);
    let id = import(&mut s, &root.join("IMG_1.png"));
    // a picture named as a clip, a file that isn't there, a text file
    png(&root.join("fake.mov"), 2);
    std::fs::write(root.join("notes.txt"), b"hello").unwrap();
    for bad in [root.join("fake.mov"), root.join("gone.mov"), root.join("notes.txt")] {
        assert!(s.execute("live.attach", &json!({"id": id.0, "path": bad.to_string_lossy()})).is_err(), "{}", bad.display());
    }
    // a clip cut short (its header unreadable)
    let cut = lightcraft_video::testdata::clip(20, 10, 0);
    std::fs::write(root.join("cut.mov"), &cut[..cut.len() / 8]).unwrap();
    assert!(s.execute("live.attach", &json!({"id": id.0, "path": root.join("cut.mov").to_string_lossy()})).is_err());
    // a video is no still; a photo that isn't there; ids and paths of the wrong shape
    clip(&root.join("clip.mp4"));
    let video = import(&mut s, &root.join("clip.mp4"));
    assert_eq!(s.catalog.photo(video).unwrap().kind, MediaKind::Video);
    clip(&root.join("m.mov"));
    let m = root.join("m.mov").to_string_lossy().to_string();
    for p in [
        json!({"id": video.0, "path": m}),
        json!({"id": 999_999, "path": m}),
        json!({"id": "x", "path": m}),
        json!({"id": -1, "path": m}),
        json!({"id": id.0}),
        json!({"id": id.0, "path": 7}),
    ] {
        assert!(s.execute("live.attach", &p).is_err(), "{p}");
    }
    assert_eq!(summary(&mut s, id).get("live"), None);
    // hostile extras are left out, not stored
    let r = s.execute("live.attach", &json!({"id": id.0, "path": m, "contentId": "a\u{0}b", "stillTime": f64::MAX})).unwrap();
    assert_eq!((r["contentId"].clone(), r["stillTime"].clone()), (Value::Null, Value::Null));
    assert!(s.execute("live.info", &json!({"id": "nope"})).is_err());
    assert_eq!(s.execute("live.info", &json!({"id": 424_242})).unwrap(), Value::Null);
}

#[test]
fn a_library_without_a_folder_keeps_no_motion() {
    let root = temp_dir("demo");
    let mut s = Session::with_demo();
    let id = s.visible_cloned()[0];
    clip(&root.join("m.mov"));
    assert!(s.execute("live.attach", &json!({"id": id.0, "path": root.join("m.mov").to_string_lossy()})).is_err());
    assert_eq!(s.execute("live.info", &json!({"id": id.0})).unwrap(), Value::Null);
}

#[test]
fn an_asset_opened_with_its_motion_is_a_live_photo_and_takes_it_when_forgotten() {
    let root = temp_dir("open");
    let mut s = session(&root);
    let still = root.join("staging").join("a").join("IMG_1.png");
    png(&still, 1);
    let motion = root.join("staging").join("a").join("IMG_1.MOV");
    clip(&motion);
    let r = s.execute("photoLibrary.open", &json!({"asset": "A1", "path": still.to_string_lossy(), "motion": motion.to_string_lossy()})).unwrap();
    let id = PhotoId(r["id"].as_u64().unwrap());
    assert_eq!(r["live"], json!(true), "{r}");
    assert!(!motion.exists(), "moved from the staging folder");
    let file = motion_file(&mut s, id).unwrap();
    assert!(file.is_file());
    // opened again: no file needed, and it says it is live
    let again = s.execute("photoLibrary.open", &json!({"asset": "A1"})).unwrap();
    assert_eq!((again["known"].as_bool(), again["live"].as_bool()), (Some(true), Some(true)), "{again}");
    // left unchanged: the record, its copy and its motion go
    let r = s.execute("photoLibrary.close", &json!({"id": id.0})).unwrap();
    assert_eq!(r["forgotten"], json!(true));
    assert!(!file.exists(), "the motion goes with the record");
}

#[test]
fn a_bad_motion_never_fails_the_open() {
    let root = temp_dir("open-bad");
    let mut s = session(&root);
    let still = root.join("staging").join("a").join("IMG_1.png");
    png(&still, 1);
    let fake = root.join("staging").join("a").join("IMG_1.MOV");
    png(&fake, 2);
    let r = s.execute("photoLibrary.open", &json!({"asset": "A1", "path": still.to_string_lossy(), "motion": fake.to_string_lossy()})).unwrap();
    assert_eq!(r["live"], json!(false), "{r}");
    assert!(!fake.exists(), "the staged file is dropped");
    let r = s.execute("photoLibrary.open", &json!({"asset": "A2", "path": 1, "motion": {}}));
    assert!(r.is_ok(), "a path that isn't one: needs a file");
}

#[test]
fn an_import_keeps_each_stills_motion_and_not_the_clip_as_a_video() {
    let root = temp_dir("import");
    let mut s = session(&root);
    let pick = root.join("staging").join("pick-1");
    png(&pick.join("IMG_1.png"), 1);
    png(&pick.join("IMG_2.png"), 2);
    clip(&pick.join("IMG_1.MOV"));
    let still = pick.join("IMG_1.png").to_string_lossy().to_string();
    let other = pick.join("IMG_2.png").to_string_lossy().to_string();
    let motions = json!({still.clone(): pick.join("IMG_1.MOV").to_string_lossy()});
    let r = s.execute("library.import", &json!({"paths": [still, other], "mode": "move", "motions": motions})).unwrap();
    assert_eq!(r["imported"].as_array().map(Vec::len), Some(2), "{r}");
    assert_eq!(r["live"], json!(1), "{r}");
    assert!(!pick.join("IMG_1.MOV").exists(), "moved with the still");
    let live: Vec<PhotoId> = s.catalog.photos().filter(|p| s.live_photos.is_live(p)).map(|p| p.id).collect();
    assert_eq!(live.len(), 1);
    assert_eq!(s.catalog.photo(live[0]).unwrap().file_name, "IMG_1.png");
    assert!(s.catalog.photos().all(|p| p.kind == MediaKind::Image), "the clip is no video of its own");
    // the same still picked again with its motion: a duplicate, but the photo had none: it gains it
    let mut s2 = s;
    let id2 = s2.catalog.photos().find(|p| p.file_name == "IMG_2.png").unwrap().id;
    let again = root.join("staging").join("pick-2");
    png(&again.join("IMG_2.png"), 2);
    clip(&again.join("IMG_2.MOV"));
    let still2 = again.join("IMG_2.png").to_string_lossy().to_string();
    let r = s2
        .execute(
            "library.import",
            &json!({"paths": [still2.clone()], "mode": "move", "motions": {still2: again.join("IMG_2.MOV").to_string_lossy()}}),
        )
        .unwrap();
    assert_eq!(r["imported"].as_array().map(Vec::len), Some(0), "{r}");
    assert_eq!(r["live"], json!(1), "{r}");
    assert_eq!(summary(&mut s2, id2)["live"], json!(true));
}

#[test]
fn import_motions_of_the_wrong_shape_are_refused() {
    let root = temp_dir("import-bad");
    let mut s = session(&root);
    png(&root.join("IMG_1.png"), 1);
    let still = root.join("IMG_1.png").to_string_lossy().to_string();
    for motions in [json!([1, 2]), json!("x"), json!({still.clone(): 3}), json!({still.clone(): ""})] {
        assert!(s.execute("library.import", &json!({"paths": [still.clone()], "motions": motions})).is_err(), "{motions}");
    }
    // a clip that can't be kept: the still is imported all the same
    let r = s.execute("library.import", &json!({"paths": [still.clone()], "motions": {still: root.join("gone.mov").to_string_lossy()}})).unwrap();
    assert_eq!(r["imported"].as_array().map(Vec::len), Some(1));
    assert_eq!(r.get("live"), None);
}

#[test]
fn a_photo_deleted_for_good_keeps_its_motion_until_the_library_opens_again() {
    let root = temp_dir("delete");
    let (id, file) = {
        let mut s = session(&root);
        png(&root.join("IMG_1.png"), 1);
        let id = import(&mut s, &root.join("IMG_1.png"));
        clip(&root.join("staging").join("IMG_1.MOV"));
        s.execute("live.attach", &json!({"id": id.0, "path": root.join("staging").join("IMG_1.MOV").to_string_lossy()})).unwrap();
        let file = motion_file(&mut s, id).unwrap();
        s.execute("photo.delete", &json!({"ids": [id.0]})).unwrap();
        // (as Recently Deleted's Delete does it: on the photo chosen there)
        s.selection = crate::Selection::single(id);
        s.execute("photo.deletePermanently", &json!({"ids": [id.0]})).unwrap();
        assert!(s.catalog.photo(id).is_none());
        assert!(file.is_file(), "Undo could still bring it back");
        s.execute("edit.undo", &json!({})).unwrap();
        assert!(s.catalog.photo(id).is_some());
        // (back in Recently Deleted, with its motion)
        assert_eq!(motion_file(&mut s, id).as_deref(), Some(file.as_path()), "back with its motion");
        s.execute("edit.redo", &json!({})).unwrap();
        // a file nobody lists (a crash between placing a clip and saving the list)
        std::fs::write(root.join("lib").join("Motion").join("77.mov"), b"x").unwrap();
        s.persist().unwrap();
        (id, file)
    };
    let mut s = session(&root);
    assert!(!file.exists(), "gone once the library opens again");
    assert!(!root.join("lib").join("Motion").join("77.mov").exists(), "and so is the stray file");
    assert_eq!(s.execute("live.info", &json!({"id": id.0})).unwrap(), Value::Null);
}

#[test]
fn a_motion_survives_the_library_opening_again_and_detach_takes_it() {
    let root = temp_dir("reopen");
    let id = {
        let mut s = session(&root);
        png(&root.join("IMG_1.png"), 1);
        let id = import(&mut s, &root.join("IMG_1.png"));
        clip(&root.join("staging").join("IMG_1.MOV"));
        s.execute("live.attach", &json!({"id": id.0, "path": root.join("staging").join("IMG_1.MOV").to_string_lossy()})).unwrap();
        s.persist().unwrap();
        id
    };
    let mut s = session(&root);
    assert_eq!(summary(&mut s, id)["live"], json!(true));
    let file = motion_file(&mut s, id).unwrap();
    assert_eq!(s.execute("live.detach", &json!({"id": id.0})).unwrap(), json!({"removed": true}));
    assert!(!file.exists());
    assert_eq!(s.execute("live.detach", &json!({"id": id.0})).unwrap(), json!({"removed": false}));
    assert_eq!(summary(&mut s, id).get("live"), None);
}

#[test]
fn a_motion_whose_still_is_another_photo_now_is_stale() {
    let root = temp_dir("stale");
    let mut s = session(&root);
    png(&root.join("IMG_1.png"), 1);
    let id = import(&mut s, &root.join("IMG_1.png"));
    clip(&root.join("m.mov"));
    s.execute("live.attach", &json!({"id": id.0, "path": root.join("m.mov").to_string_lossy()})).unwrap();
    // (as after a sign-out, when an id names another account's photo)
    if let Some(m) = s.live_photos.photos.get_mut(&id.0) {
        m.hash = Some("another".into());
    }
    assert_eq!(summary(&mut s, id).get("live"), None);
    assert_eq!(s.execute("live.info", &json!({"id": id.0})).unwrap(), Value::Null);
    s.tidy_live_photos();
    assert!(std::fs::read_dir(root.join("lib").join("Motion")).unwrap().next().is_none(), "its file goes when tidied");
}

#[test]
fn a_damaged_list_reads_as_none() {
    for bytes in [&b"{"[..], b"[]", b"{\"photos\": {\"x\": 1}}", b"\xff\xfe", b""] {
        assert_eq!(crate::live_photo::LivePhotos::from_json(bytes), Default::default());
    }
}
