//! App packs (`pack.set`): an app's own looks, as profiles and built-in presets that are never saved or synced.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::Session;

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-pack-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A 5-point LUT that makes red `strength` brighter and blue dimmer.
fn write_cube(path: &Path, title: &str, strength: f32) {
    let n = 5;
    let mut cube = format!("TITLE \"{title}\"\nLUT_3D_SIZE {n}\n");
    for b in 0..n {
        for g in 0..n {
            for r in 0..n {
                let f = |v: usize| v as f32 / (n - 1) as f32;
                cube.push_str(&format!("{} {} {}\n", (f(r) * (1.0 + strength)).min(1.0), f(g), f(b) * (1.0 - strength)));
            }
        }
    }
    std::fs::write(path, cube).unwrap();
}

fn write_presets(path: &Path, names: &[&str], pack: &str) {
    let presets: Vec<Value> = names
        .iter()
        .map(|n| {
            json!({"id": format!("t.{}", crate::presets::slug(n)), "name": n, "group": "Test +", "favorite": false, "builtin": false,
                   "settings": {"profile": {"id": super::cmd::packs::profile_id(pack, n), "amount": 100.0}, "grain": {"amount": 10.0}}})
        })
        .collect();
    std::fs::write(path, json!({"format": "lightcraft.preset", "version": 1, "presets": presets}).to_string()).unwrap();
}

fn names(s: &mut Session, group: &str) -> Vec<String> {
    let list = s.execute("presets.list", &json!({})).unwrap();
    list.as_array().unwrap().iter().filter(|p| p["group"] == group).map(|p| p["name"].as_str().unwrap().to_string()).collect()
}

#[test]
fn a_pack_brings_profiles_and_built_in_presets_that_are_not_saved() {
    let dir = temp_dir("set");
    let lib = temp_dir("lib");
    std::fs::create_dir_all(dir.join("Test +")).unwrap();
    let pack = dir.join("Test +");
    write_cube(&pack.join("Warm.cube"), "Warm", 0.2);
    write_cube(&pack.join("Warmer.cube"), "Warmer", 0.4);
    std::fs::write(pack.join("broken.cube"), "not a table\n").unwrap();
    write_presets(&pack.join("Test +.lcpreset"), &["Warm", "Warmer"], "testpack");

    let mut s = Session::new().with_fs();
    s.open_library(&lib, true).unwrap();
    let before = names(&mut s, "Test +");
    assert!(before.is_empty());
    let r = s.execute("pack.set", &json!({"id": "testpack", "dir": pack.to_string_lossy()})).unwrap();
    assert_eq!((r["profiles"].as_u64(), r["presets"].as_u64(), r["failed"].as_array().map(Vec::len)), (Some(2), Some(2), Some(1)), "{r}");
    let warm = super::cmd::packs::profile_id("testpack", "Warm");
    assert_eq!(warm, "lut:pack-testpack-warm");
    assert!(lightcraft_pipeline::lut::get(&warm).is_some(), "registered with the renderer");
    assert_eq!(s.profile_info(&warm), Some(("Warm", "Test +")), "the group is the folder's");
    assert_eq!(names(&mut s, "Test +"), ["Warm", "Warmer"]);
    let listed = s.execute("presets.list", &json!({})).unwrap();
    let warm_preset = listed.as_array().unwrap().iter().find(|p| p["name"] == "Warm").unwrap().clone();
    assert_eq!(warm_preset["builtin"], true, "built in: can be hidden, not edited or deleted");
    assert!(s.execute("preset.delete", &json!({"id": warm_preset["id"]})).is_err());
    assert!(s.execute("profile.deleteImported", &json!({"id": warm})).is_err(), "a pack's profile is not an imported one");
    let menu = s.profile_menu();
    let group = menu["groups"].as_array().unwrap().iter().find(|g| g["name"] == "Test +").unwrap();
    assert_eq!(group["profiles"].as_array().unwrap().len(), 2, "{menu}");
    assert!(group.get("imported").is_none(), "{group}");

    // the preset applies the look: the photo renders differently
    let png_dir = temp_dir("png");
    let png = png_dir.join("a.png");
    let (w, h) = (48usize, 32usize);
    let data: Vec<[u8; 4]> = (0..w * h).map(|i| [(i % w * 5) as u8, (i / w * 7) as u8, 90, 255]).collect();
    let img = lightcraft_raster::Rgba8 { width: w, height: h, data };
    std::fs::write(
        &png,
        lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap(),
    )
    .unwrap();
    s.execute("library.import", &json!({"paths": [png.to_string_lossy()]})).unwrap();
    let photo = s.catalog.photos().next().unwrap().id;
    s.execute("library.select", &json!({"ids": [photo.0]})).unwrap();
    let base = s.render_now(photo, 48, 32).unwrap().image;
    s.execute("preset.apply", &json!({"id": warm_preset["id"]})).unwrap();
    assert_eq!(s.develop_of(photo).unwrap().profile.id, warm);
    assert_ne!(s.render_now(photo, 48, 32).unwrap().image.data, base.data);

    // switching libraries keeps the app's looks
    let other = temp_dir("lib2");
    s.open_library(&other, true).unwrap();
    assert_eq!(names(&mut s, "Test +"), ["Warm", "Warmer"], "the pack is the app's, not the library's");
    s.open_library(&lib, false).unwrap();
    let _ = std::fs::remove_dir_all(&other);

    // a favourite survives a restart, though the pack is set after the library is read
    s.execute("preset.favorite", &json!({"id": warm_preset["id"], "favorite": true})).unwrap();
    s.execute("profile.favorite", &json!({"id": warm})).unwrap();
    drop(s);
    let mut s = Session::new().with_fs();
    s.open_library(&lib, false).unwrap();
    assert!(names(&mut s, "Test +").is_empty(), "nothing of the pack is in the library");
    assert!(s.profile_favorites.contains(&warm), "a pack's favourite profile is kept");
    s.execute("pack.set", &json!({"id": "testpack", "dir": pack.to_string_lossy()})).unwrap();
    let listed = s.execute("presets.list", &json!({})).unwrap();
    let warm_again = listed.as_array().unwrap().iter().find(|p| p["name"] == "Warm").unwrap();
    assert_eq!(warm_again["favorite"], true, "{warm_again}");

    // a new version of the pack replaces the old: one preset fewer, one profile fewer
    std::fs::remove_file(pack.join("Warmer.cube")).unwrap();
    write_presets(&pack.join("Test +.lcpreset"), &["Warm"], "testpack");
    s.execute("pack.set", &json!({"id": "testpack", "dir": pack.to_string_lossy()})).unwrap();
    assert_eq!(names(&mut s, "Test +"), ["Warm"]);
    assert!(lightcraft_pipeline::lut::get(&super::cmd::packs::profile_id("testpack", "Warmer")).is_none(), "the old LUT is dropped");
    // a list of files does as well as a folder (an app's bundle has no folder of its own), under the group given
    let listed = json!([pack.join("Warm.cube").to_string_lossy(), pack.join("Test +.lcpreset").to_string_lossy()]);
    s.execute("pack.set", &json!({"id": "testpack", "files": listed, "group": "Listed"})).unwrap();
    assert_eq!(s.profile_info(&warm), Some(("Warm", "Listed")));
    assert_eq!(names(&mut s, "Test +"), ["Warm"], "the presets keep the group their file gives them");
    // and without a folder or files it is taken out
    s.execute("pack.set", &json!({"id": "testpack"})).unwrap();
    assert!(names(&mut s, "Test +").is_empty());
    assert!(lightcraft_pipeline::lut::get(&warm).is_none());
    assert!(s.packs.is_empty());

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&lib);
    let _ = std::fs::remove_dir_all(&png_dir);
}

#[test]
fn a_missing_folder_is_an_error_not_a_panic() {
    let mut s = Session::new();
    assert!(s.execute("pack.set", &json!({"id": "x", "dir": "/no/such/folder"})).is_err());
    assert!(s.execute("pack.set", &json!({})).is_err());
}

fn favorite_of(s: &mut Session, name: &str) -> bool {
    let list = s.execute("presets.list", &json!({})).unwrap();
    list.as_array().unwrap().iter().find(|p| p["name"] == name).map(|p| p["favorite"] == true).unwrap()
}

fn id_of(s: &mut Session, name: &str) -> String {
    let list = s.execute("presets.list", &json!({})).unwrap();
    list.as_array().unwrap().iter().find(|p| p["name"] == name).unwrap()["id"].as_str().unwrap().to_string()
}

/// What the person starred stays starred when the pack is set again or the list is rebuilt (a sync's server presets),
/// and what they un-starred does not come back from what the library once said.
#[test]
fn favourites_follow_the_person_not_the_last_load() {
    let dir = temp_dir("fav");
    let lib = temp_dir("favlib");
    let pack = dir.join("Test +");
    std::fs::create_dir_all(&pack).unwrap();
    write_cube(&pack.join("Warm.cube"), "Warm", 0.2);
    write_cube(&pack.join("Cool.cube"), "Cool", 0.1);
    write_presets(&pack.join("Test +.lcpreset"), &["Warm", "Cool"], "favpack");
    let set = json!({"id": "favpack", "dir": pack.to_string_lossy()});
    let mut s = Session::new().with_fs();
    s.open_library(&lib, true).unwrap();
    s.execute("pack.set", &set).unwrap();
    let (warm, cool) = (id_of(&mut s, "Warm"), id_of(&mut s, "Cool"));
    s.execute("preset.favorite", &json!({"id": warm, "favorite": true})).unwrap();
    // set again, and the list rebuilt by the server's presets arriving: still starred
    s.execute("pack.set", &set).unwrap();
    assert!(favorite_of(&mut s, "Warm") && !favorite_of(&mut s, "Cool"));
    s.set_server_presets(Vec::new(), Default::default());
    assert!(favorite_of(&mut s, "Warm") && !favorite_of(&mut s, "Cool"), "kept through a rebuild of the list");
    // starred at the last load, un-starred since: not starred again by a later set
    s.execute("preset.favorite", &json!({"id": cool, "favorite": true})).unwrap();
    drop(s);
    let mut s = Session::new().with_fs();
    s.open_library(&lib, false).unwrap();
    s.execute("pack.set", &set).unwrap();
    assert!(favorite_of(&mut s, "Warm") && favorite_of(&mut s, "Cool"));
    s.execute("preset.favorite", &json!({"id": cool, "favorite": false})).unwrap();
    s.execute("pack.set", &set).unwrap();
    assert!(favorite_of(&mut s, "Warm") && !favorite_of(&mut s, "Cool"), "an un-starred preset stays so");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&lib);
}

/// A pack's files that clash with what is already there, or are of the wrong kind, are reported or refused, and a
/// pack that cannot take a built-in preset's id leaves that preset alone.
#[test]
fn a_pack_cannot_take_a_built_in_presets_id_and_bad_parameters_are_refused() {
    let dir = temp_dir("clash");
    let pack = dir.join("Clash");
    std::fs::create_dir_all(&pack).unwrap();
    write_cube(&pack.join("A.cube"), "Same", 0.2);
    write_cube(&pack.join("B.cube"), "Same", 0.3);
    // a license text longer than the header the engine reads, then a table
    let mut long = String::from("TITLE \"Long\"\n");
    long.push_str(&"# a long licence line that goes on and on\n".repeat(200));
    long.push_str("LUT_3D_SIZE 2\n");
    for _ in 0..8 {
        long.push_str("0 0 0\n");
    }
    std::fs::write(pack.join("long.cube"), long).unwrap();
    let builtin_id = crate::presets::builtin()[0].id.clone();
    let presets: Vec<Value> = [(builtin_id.as_str(), "Takes a built-in id"), ("clash.fine", "Fine"), ("clash.fine", "Twin")]
        .iter()
        .map(|(id, name)| json!({"id": id, "name": name, "group": "Clash", "favorite": false, "builtin": false, "settings": {"grain": {"amount": 5.0}}}))
        .collect();
    std::fs::write(pack.join("p.lcpreset"), json!({"format": "lightcraft.preset", "version": 1, "presets": presets}).to_string()).unwrap();
    let mut s = Session::new();
    let before = s.presets.len();
    let r = s.execute("pack.set", &json!({"id": "clash", "dir": pack.to_string_lossy()})).unwrap();
    assert_eq!((r["profiles"].as_u64(), r["presets"].as_u64(), r["failed"].as_array().map(Vec::len)), (Some(2), Some(1), Some(3)), "{r}");
    assert_eq!(s.presets.len(), before + 1, "only the fine one came in");
    assert!(s.presets.iter().any(|p| p.id == builtin_id && p.group != "Clash"), "the built-in preset is untouched");
    s.execute("pack.set", &json!({"id": "clash"})).unwrap();
    assert!(s.presets.iter().any(|p| p.id == builtin_id), "and still there when the pack goes");
    // the wrong kinds of parameters are errors, they don't take the pack out
    s.execute("pack.set", &json!({"id": "clash", "dir": pack.to_string_lossy()})).unwrap();
    for bad in [json!({"id": "clash", "files": "A.cube"}), json!({"id": "clash", "dir": 5}), json!({"id": "clash", "files": [1, 2]})] {
        assert!(s.execute("pack.set", &bad).is_err(), "{bad}");
    }
    assert_eq!(s.packs.len(), 1, "the pack is still set");
    let _ = std::fs::remove_dir_all(&dir);
}
