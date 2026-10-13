//! Photo ▸ Edit In round trips: Edit Original / Edit a Copy for image files, and what the
//! external editor saves coming back (`photo.checkExternalEdits`): changed files re-read, a Save
//! As beside a file stacked on top of its photo, the watch list kept across restarts by the host.

use std::path::Path;
use std::time::{Duration, SystemTime};

use lightcraft_catalog::PhotoId;
use serde_json::{Value, json};

use crate::Session;
use crate::tests_xmp::temp_dir;

/// A flat 16-bit PSD of `w` × `h`, every sample `v`.
fn write_psd(path: &Path, w: u32, h: u32, v: u16) {
    let px = vec![v; (w * h * 3) as usize];
    let img = lightcraft_codecs::EncodeImage::new(w, h, 3, lightcraft_codecs::Samples::U16(&px));
    std::fs::write(path, lightcraft_codecs::encode_psd(&img, &Default::default()).unwrap()).unwrap();
}

/// A 16-bit PNG of `w` × `h`, every sample `v`.
fn write_png(path: &Path, w: u32, h: u32, v: u16) {
    let px = vec![v; (w * h * 3) as usize];
    let img = lightcraft_codecs::EncodeImage::new(w, h, 3, lightcraft_codecs::Samples::U16(&px));
    std::fs::write(path, lightcraft_codecs::encode_png(&img, &Default::default()).unwrap()).unwrap();
}

fn import(s: &mut Session, path: &Path) -> PhotoId {
    let r = s.execute("library.import", &json!({"paths": [path.to_string_lossy()]})).unwrap();
    PhotoId(r["imported"][0].as_u64().unwrap_or_else(|| panic!("{r}")))
}

fn check(s: &mut Session, files: &Value) -> Value {
    s.execute("photo.checkExternalEdits", &json!({"files": files})).unwrap()
}

/// Give a file an older modification time (a file that was there before the editor opened).
fn age(path: &Path, by: Duration) {
    let f = std::fs::File::options().write(true).open(path).unwrap();
    f.set_modified(SystemTime::now() - by).unwrap();
}

/// Edit Original on a PSD: the file itself is handed over (no copy, nothing added, the selection
/// kept); what the editor saves over it comes back, under the photo's own edits.
#[test]
fn edit_original_hands_over_the_file_and_brings_back_its_saves() {
    let dir = temp_dir("extedit-original");
    let src = dir.join("pier.psd");
    write_psd(&src, 40, 24, 9000);
    let mut s = Session::new().with_fs();
    let id = import(&mut s, &src);
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 0.5})).unwrap();
    let n = s.catalog.len();
    let r = s.execute("photo.editExternal", &json!({"mode": "original"})).unwrap();
    assert_eq!((r["path"].as_str(), r["id"].as_u64(), r["mode"].as_str()), (Some(&*src.to_string_lossy()), Some(id.0), Some("original")), "{r}");
    assert_eq!(s.catalog.len(), n, "no copy");
    assert_eq!(s.active(), Some(id), "the selection is kept");
    let track = r["track"].clone();
    assert_eq!(track[0]["path"], r["path"]);
    assert!(track[0]["size"].as_u64().is_some_and(|n| n > 0) && track[0]["modified"].is_u64(), "{track}");
    // nothing saved yet
    let r = check(&mut s, &track);
    assert_eq!(r["reloaded"], json!([]), "{r}");
    let files = r["files"].clone();
    // the editor saves over it (cropped, so it has other dimensions)
    let before = s.render_now(id, 16, 16).unwrap().image.data;
    write_psd(&src, 30, 20, 60000);
    let r = check(&mut s, &files);
    assert_eq!(r["reloaded"], json!([id.0]), "{r}");
    let ph = s.catalog.photo(id).unwrap();
    assert_eq!((ph.width, ph.height), (30, 20));
    assert_eq!(ph.develop.light.exposure, 0.5, "the edits stay on top");
    assert_ne!(s.render_now(id, 16, 16).unwrap().image.data, before);
    // read once: the next check has nothing to do
    let r = check(&mut s, &r["files"].clone());
    assert_eq!((r["reloaded"].clone(), r["files"].as_array().map(Vec::len)), (json!([]), Some(1)), "{r}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Edit a Copy: a plain copy of the file (the same bytes) beside it, added without the edits,
/// stacked on top of the original and selected.
#[test]
fn edit_a_copy_copies_the_file_without_the_edits() {
    let dir = temp_dir("extedit-copy");
    let src = dir.join("pier.psd");
    write_psd(&src, 40, 24, 9000);
    let mut s = Session::new().with_fs();
    let id = import(&mut s, &src);
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 0.5})).unwrap();
    let r = s.execute("photo.editExternal", &json!({"mode": "copy"})).unwrap();
    let path = r["path"].as_str().unwrap().to_string();
    assert!(path.ends_with("pier-Edit.psd"), "{r}");
    assert_eq!(std::fs::read(&path).unwrap(), std::fs::read(&src).unwrap(), "the file as it is");
    let new = PhotoId(r["id"].as_u64().unwrap());
    let copy = s.catalog.photo(new).unwrap();
    assert!(!copy.is_edited() && copy.develop.light.exposure == 0.0, "without the edits");
    assert_eq!(s.catalog.photo(id).unwrap().develop.light.exposure, 0.5, "the original keeps them");
    let st = s.catalog.stack_of(new).expect("stacked");
    assert_eq!((st.top(), st.photos.contains(&id)), (new, true));
    assert_eq!(s.active(), Some(new));
    // a second copy doesn't overwrite the first
    s.execute("library.select", &json!({"ids": [id.0]})).unwrap();
    let r2 = s.execute("photo.editExternal", &json!({"mode": "copy"})).unwrap();
    assert!(r2["path"].as_str().unwrap().ends_with("pier-Edit-2.psd"), "{r2}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Only an image file can be handed over as it is: a generated photo (or a raw) is rendered.
#[test]
fn copy_and_original_take_image_files_only() {
    let mut s = Session::with_demo();
    for mode in ["copy", "original"] {
        let e = s.execute("photo.editExternal", &json!({"mode": mode})).unwrap_err().to_string();
        assert!(e.contains("use mode render"), "{e}");
    }
    assert!(s.execute("photo.editExternal", &json!({"mode": "inPlace"})).is_err());
    assert!(!crate::cmd::can_edit_original(s.catalog.photos().next().unwrap()));
}

/// The editor's Save As beside the copy (a PSD next to the TIFF it was given) is added on top of
/// the stack and watched too; a file that was there before, or under another name, is not.
#[test]
fn save_as_beside_the_copy_is_added_to_the_stack() {
    let dir = temp_dir("extedit-saveas");
    let src = dir.join("beach.psd");
    write_psd(&src, 40, 24, 9000);
    // there before the editor opened the copy
    let old = dir.join("beach-Edit.png");
    write_png(&old, 40, 24, 2000);
    age(&old, Duration::from_secs(3600));
    let mut s = Session::new().with_fs();
    let orig = import(&mut s, &src);
    let r = s.execute("photo.editExternal", &json!({})).unwrap();
    let copy = PhotoId(r["id"].as_u64().unwrap());
    let copy_path = r["path"].as_str().unwrap().to_string();
    assert!(copy_path.ends_with("beach-Edit.tif"), "{r}");
    let track = r["track"].clone();
    // the editor saves its work as a PSD, and something else under another name
    let saved = dir.join("beach-Edit.psd");
    write_psd(&saved, 40, 24, 30000);
    write_psd(&dir.join("other.psd"), 40, 24, 100);
    s.execute("library.select", &json!({"ids": [orig.0]})).unwrap();
    let r = check(&mut s, &track);
    let added = r["added"].as_array().unwrap();
    assert_eq!(added.len(), 1, "{r}");
    assert_eq!((added[0]["path"].as_str(), added[0]["original"].as_u64()), (Some(&*saved.to_string_lossy()), Some(copy.0)));
    let new = PhotoId(added[0]["id"].as_u64().unwrap());
    let st = s.catalog.stack_of(new).expect("stacked");
    assert_eq!(st.top(), new, "on top");
    assert!(st.photos.contains(&copy) && st.photos.contains(&orig), "with the copy and the original");
    assert_eq!(s.active(), Some(orig), "the selection is kept");
    let files = r["files"].as_array().unwrap().clone();
    let paths: Vec<&str> = files.iter().filter_map(|f| f["path"].as_str()).collect();
    assert_eq!(paths, [copy_path.as_str(), &*saved.to_string_lossy()], "the Save As is watched too");
    // added once
    let r = check(&mut s, &json!(files));
    assert_eq!(r["added"], json!([]), "{r}");
    // later saves to the PSD come back too
    write_psd(&saved, 32, 20, 50000);
    let r = check(&mut s, &r["files"].clone());
    assert_eq!(r["reloaded"], json!([new.0]), "{r}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The watch list: a file that is gone or hasn't changed in two weeks drops off; one that isn't a
/// photo of this library (another library has it) is left as it is, its Save As not imported here;
/// a file that can't be read (still being written) is read again next time; junk is an error.
#[test]
fn the_watch_list_is_kept_tidy() {
    let dir = temp_dir("extedit-list");
    let mine = dir.join("mine.psd");
    write_psd(&mine, 40, 24, 9000);
    let theirs = dir.join("theirs.psd");
    write_psd(&theirs, 40, 24, 100);
    let mut s = Session::new().with_fs();
    let id = import(&mut s, &mine);
    let r = s.execute("photo.editExternal", &json!({"mode": "original"})).unwrap();
    let entry = r["track"][0].clone();
    let fortnight = 15 * 24 * 3600 * 1000;
    let old = entry["touched"].as_u64().unwrap() - fortnight;
    let since = entry["since"].as_u64().unwrap() - 10_000;
    let files = json!([
        {"path": dir.join("gone.psd").to_string_lossy()},
        {"path": mine.to_string_lossy(), "touched": old, "since": old},
        {"path": theirs.to_string_lossy(), "since": since, "size": 1, "modified": 1},
    ]);
    write_psd(&dir.join("theirs.png"), 40, 24, 100);
    let r = check(&mut s, &files);
    assert_eq!((r["reloaded"].clone(), r["added"].clone()), (json!([]), json!([])), "{r}");
    let kept = r["files"].as_array().unwrap();
    assert_eq!(kept.len(), 1, "only the other library's file stays: {r}");
    for key in ["path", "since", "size", "modified"] {
        assert_eq!(kept[0][key], files[2][key], "{key} as it was");
    }
    assert!(kept[0]["touched"].is_u64(), "and it expires like the others");
    assert_eq!(s.catalog.len(), 1, "nothing imported");
    // half written: not read now, read again next time
    std::fs::write(&mine, b"8BPS").unwrap();
    let r = check(&mut s, &json!([entry]));
    assert_eq!(r["reloaded"], json!([]), "{r}");
    assert_eq!(r["files"][0]["size"], entry["size"], "the old stamp is kept: {r}");
    write_psd(&mine, 20, 20, 1);
    let r = check(&mut s, &r["files"].clone());
    assert_eq!(r["reloaded"], json!([id.0]), "{r}");
    // at most 200, the newest kept
    let many: Vec<Value> = (0..250).map(|i| json!({"path": theirs.to_string_lossy(), "since": i})).collect();
    let r = check(&mut s, &json!(many));
    assert_eq!(r["files"].as_array().map(Vec::len), Some(1), "one entry per file: {r}");
    assert_eq!(r["files"][0]["since"], json!(50), "the newest 200 are read");
    for junk in [json!("x"), json!([{"path": 3}]), json!([{"path": "a", "size": -1}])] {
        assert!(s.execute("photo.checkExternalEdits", &json!({"files": junk})).is_err(), "{junk}");
    }
    assert_eq!(check(&mut s, &json!([]))["files"], json!([]));
    let _ = std::fs::remove_dir_all(&dir);
}
