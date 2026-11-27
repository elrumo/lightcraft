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

    // a frontend that applies the table itself is told the file: a pack's profile has one, an unknown id none
    let file = s.execute("profile.lutFile", &json!({"id": warm})).unwrap();
    let file = file["file"].as_str().expect("a pack's profile has a file");
    assert!(file.ends_with("Warm.cube") && Path::new(file).is_file(), "{file}");
    assert!(s.execute("profile.lutFile", &json!({"id": "lut:nothing"})).unwrap()["file"].is_null());
    assert!(s.execute("profile.lutFile", &json!({})).is_err(), "an id is required");

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

/// With a sync server connected, what the server's admin hides (built-in presets and groups, by id and by name) and what
/// another device synced as hidden never reaches an app pack's presets: they are listed, and not hidden, and cannot be hidden.
#[test]
fn a_server_and_the_hide_lists_never_hide_a_packs_presets() {
    let dir = temp_dir("srv");
    let pack = dir.join("Test +");
    std::fs::create_dir_all(&pack).unwrap();
    write_cube(&pack.join("Warm.cube"), "Warm", 0.2);
    write_presets(&pack.join("Test +.lcpreset"), &["Warm", "Cool"], "srvpack");
    let mut s = Session::new();
    s.execute("pack.set", &json!({"id": "srvpack", "dir": pack.to_string_lossy()})).unwrap();
    let (warm, cool) = (id_of(&mut s, "Warm"), id_of(&mut s, "Cool"));
    // the server hides everything it can by name and id, and shares a preset of its own
    let hidden = crate::presets::Hidden { presets: vec![warm.clone(), cool.clone()], groups: vec!["Test +".into()] };
    let own = lightcraft_develop::Preset {
        id: "srv.mine".into(),
        name: "Shared".into(),
        group: "Studio".into(),
        settings: json!({"grain": {"amount": 5.0}}),
        favorite: false,
        builtin: true,
    };
    s.set_server_presets(vec![own], hidden.clone());
    assert_eq!(names(&mut s, "Test +"), ["Warm", "Cool"], "listed with a server's hide lists in force");
    assert_eq!(names(&mut s, "Studio"), ["Shared"]);
    // and another device synced them as hidden
    s.hidden_presets = hidden;
    let list = s.execute("presets.list", &json!({})).unwrap();
    assert!(list.as_array().unwrap().iter().filter(|p| p["group"] == "Test +").all(|p| p["hidden"] == false), "{list}");
    let groups = s.execute("presets.groups", &json!({"details": true})).unwrap();
    assert_eq!(groups.as_array().unwrap().iter().find(|g| g["name"] == "Test +").unwrap()["hidden"], false, "{groups}");
    // the person cannot hide them either, but can take a stale hide off
    assert!(s.execute("preset.hide", &json!({"id": warm, "hidden": true})).is_err());
    assert!(s.execute("preset.hideGroup", &json!({"group": "Test +", "hidden": true})).is_err());
    s.execute("preset.hide", &json!({"id": warm, "hidden": false})).unwrap();
    s.execute("preset.hideGroup", &json!({"group": "Test +", "hidden": false})).unwrap();
    assert!(!s.hidden_presets.presets.contains(&warm) && !s.hidden_presets.groups.contains(&"Test +".to_string()));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_retired_profile_keeps_rendering_through_its_alias() {
    let dir = temp_dir("alias");
    let lib = temp_dir("alias-lib");
    let pack = dir.join("Test +");
    std::fs::create_dir_all(&pack).unwrap();
    write_cube(&pack.join("Red.cube"), "Red", 0.4);
    let mut s = Session::new().with_fs();
    s.open_library(&lib, true).unwrap();

    // "Old Red" left the pack; photos edited with it draw Red's table under the old id
    let red = pack.join("Red.cube").to_string_lossy().to_string();
    let r = s
        .execute(
            "pack.set",
            &json!({"id": "testpack", "dir": pack.to_string_lossy(), "aliases": {"Old Red": red, "Red": red, "Gone": "/nowhere.cube"}}),
        )
        .unwrap();
    assert_eq!((r["profiles"].as_u64(), r["aliases"].as_u64(), r["failed"].as_array().map(Vec::len)), (Some(1), Some(1), Some(1)), "{r}");
    let old = super::cmd::packs::profile_id("testpack", "Old Red");
    assert!(lightcraft_pipeline::lut::get(&old).is_some(), "the old id is registered with the renderer");
    assert_eq!(s.profile_info(&old), Some(("Old Red", "Test +")), "the photo still names its look");
    assert_eq!(s.execute("profile.lutFile", &json!({"id": old})).unwrap()["file"], json!(red), "a frontend reads the same table");
    let menu = s.profile_menu().to_string();
    assert!(!menu.contains("Old Red"), "a retired profile is not offered: {menu}");
    assert!(s.execute("pack.set", &json!({"id": "testpack", "dir": pack.to_string_lossy(), "aliases": ["x"]})).is_err(), "aliases is an object");

    // taking the pack out drops the alias too
    s.execute("pack.set", &json!({"id": "testpack"})).unwrap();
    assert!(lightcraft_pipeline::lut::get(&old).is_none());
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&lib);
}

/// A demo library with pack `id` set: the look "Here" with its table, "Later" and "Unused" declared absent.
fn with_absent_looks(tag: &str, id: &str) -> (Session, PathBuf, lightcraft_catalog::PhotoId, lightcraft_catalog::PhotoId) {
    let dir = temp_dir(tag);
    write_cube(&dir.join("Here.cube"), "Here", 0.3);
    write_presets(&dir.join("looks.lcpreset"), &["Here", "Later"], id);
    let mut s = Session::with_demo();
    let r =
        s.execute("pack.set", &json!({"id": id, "dir": dir.to_string_lossy(), "group": "Test +", "absent": ["Later", "Unused", "Here"]})).unwrap();
    assert_eq!((r["profiles"].as_u64(), r["absent"].as_u64()), (Some(3), Some(2)), "a look with a table in the call is not absent: {r}");
    let ids = s.visible_cloned();
    (s, dir, ids[0], ids[1])
}

#[test]
fn an_absent_look_is_a_listed_profile_with_no_file() {
    let (mut s, dir, ..) = with_absent_looks("absent-list", "absentlist");
    let later = super::cmd::packs::profile_id("absentlist", "Later");
    assert_eq!(s.profile_info(&later), Some(("Later", "Test +")), "a photo that uses it still names its look");
    assert_eq!(s.execute("profile.lutFile", &json!({"id": later})).unwrap()["file"], Value::Null);
    assert!(lightcraft_pipeline::lut::get(&later).is_none() && !lightcraft_pipeline::lut::is_registered(&later));

    // listed in its group by name, among the looks that are here, and marked
    let menu = s.profile_menu();
    let group = menu["groups"].as_array().unwrap().iter().find(|g| g["name"] == "Test +").unwrap().clone();
    let listed: Vec<(String, bool)> =
        group["profiles"].as_array().unwrap().iter().map(|p| (p["name"].as_str().unwrap().to_string(), p["absent"] == true)).collect();
    assert_eq!(listed, [("Here".to_string(), false), ("Later".to_string(), true), ("Unused".to_string(), true)]);
    let all = s.execute("profiles.list", &json!({})).unwrap();
    let of = |name: &str| all.as_array().unwrap().iter().find(|p| p["name"] == name && p["pack"] == true).unwrap().clone();
    assert_eq!((of("Later")["absent"].clone(), of("Here").get("absent").cloned()), (json!(true), None));
    // a preset says which pack look it sets, and whether that look is still to be fetched
    let presets = s.execute("presets.list", &json!({})).unwrap();
    let preset = |name: &str| presets.as_array().unwrap().iter().find(|p| p["name"] == name && p["group"] == "Test +").unwrap().clone();
    assert_eq!((preset("Later")["look"].clone(), preset("Later")["absent"].clone()), (json!(later), json!(true)));
    assert_eq!(preset("Here").get("absent"), None, "{}", preset("Here"));
    assert!(preset("Here")["look"].is_string());

    // the same order once the table has come, from another folder
    let other = temp_dir("absent-list-fetched");
    write_cube(&other.join("Later.cube"), "Later", 0.5);
    let r = s
        .execute(
            "pack.set",
            &json!({"id": "absentlist", "dir": dir.to_string_lossy(), "files": [other.join("Later.cube").to_string_lossy()], "group": "Test +", "absent": ["Unused"]}),
        )
        .unwrap();
    assert_eq!(r["arrived"], json!([later]), "the call says which tables are new: {r}");
    let menu = s.profile_menu();
    let group = menu["groups"].as_array().unwrap().iter().find(|g| g["name"] == "Test +").unwrap().clone();
    let names: Vec<&str> = group["profiles"].as_array().unwrap().iter().filter_map(|p| p["name"].as_str()).collect();
    assert_eq!(names, ["Here", "Later", "Unused"]);
    assert!(lightcraft_pipeline::lut::get(&later).is_some());

    // wrong kinds are refused, and a pack of absent looks alone is a pack
    assert!(s.execute("pack.set", &json!({"id": "absentlist", "absent": "Later"})).is_err());
    assert!(s.execute("pack.set", &json!({"id": "absentlist", "absent": [1]})).is_err());
    let r = s.execute("pack.set", &json!({"id": "absentlist", "absent": ["Later", " "]})).unwrap();
    assert_eq!((r["profiles"].as_u64(), r["absent"].as_u64()), (Some(1), Some(1)), "{r}");
    s.execute("pack.set", &json!({"id": "absentlist"})).unwrap();
    assert!(s.packs.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&other);
}

#[test]
fn pack_needed_names_the_absent_looks_photos_use() {
    let (mut s, dir, a, b) = with_absent_looks("needed", "neededpack");
    let (here, later) = (super::cmd::packs::profile_id("neededpack", "Here"), super::cmd::packs::profile_id("neededpack", "Later"));
    assert_eq!(s.execute("pack.needed", &json!({})).unwrap(), json!({"profiles": []}), "nothing uses a look yet");
    s.execute("library.select", &json!({"ids": [a.0]})).unwrap();
    s.execute("develop.profile", &json!({"id": later})).unwrap();
    s.execute("library.select", &json!({"ids": [b.0]})).unwrap();
    s.execute("develop.profile", &json!({"id": here})).unwrap();
    assert_eq!(s.execute("pack.needed", &json!({})).unwrap(), json!({"profiles": [later]}), "a look that is here is not needed");
    assert_eq!(s.execute("pack.needed", &json!({"ids": [b.0]})).unwrap(), json!({"profiles": []}));
    assert_eq!(s.execute("pack.needed", &json!({"ids": [a.0, 999_999, "x"]})).unwrap(), json!({"profiles": [later]}), "unknown ids are skipped");
    assert!(s.execute("pack.needed", &json!({"ids": 3})).is_err());

    // a version keeps its settings: its look is needed too, though the photo has moved on
    s.execute("library.select", &json!({"ids": [a.0]})).unwrap();
    s.execute("version.create", &json!({"name": "with the look"})).unwrap();
    s.execute("develop.profile", &json!({"id": here})).unwrap();
    assert_eq!(s.execute("pack.needed", &json!({"ids": [a.0]})).unwrap(), json!({"profiles": [later]}));

    // a retired look whose table is not here is needed under its own id
    s.execute(
        "pack.set",
        &json!({"id": "neededpack", "dir": dir.to_string_lossy(), "group": "Test +", "absent": ["Later"], "aliases": {"Old": null}}),
    )
    .unwrap();
    let old = super::cmd::packs::profile_id("neededpack", "Old");
    assert_eq!(s.profile_info(&old), Some(("Old", "Test +")));
    assert!(!s.profile_menu().to_string().contains("\"Old\""), "a retired look is not offered");
    s.execute("develop.profile", &json!({"id": old})).unwrap();
    assert_eq!(s.execute("pack.needed", &json!({})).unwrap(), json!({"profiles": [later, old]}));
    s.execute("pack.set", &json!({"id": "neededpack"})).unwrap();
    assert_eq!(s.execute("pack.needed", &json!({})).unwrap(), json!({"profiles": []}), "a pack that is not set declares nothing");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_final_output_is_refused_while_its_look_is_absent() {
    use crate::export::{ExportOptions, export_photo};
    let (mut s, dir, a, b) = with_absent_looks("refuse", "refusepack");
    let later = super::cmd::packs::profile_id("refusepack", "Later");
    s.execute("library.select", &json!({"ids": [a.0]})).unwrap();
    s.execute("develop.profile", &json!({"id": later})).unwrap();
    let small = ExportOptions { resize: Some(crate::export::Resize::long_edge(64)), ..Default::default() };

    let photo = s.catalog.photo(a).unwrap().clone();
    let refusal = s.look_here(&photo).unwrap_err();
    assert_eq!(refusal.kind(), crate::ErrorKind::LookMissing, "a frontend tells it by its kind");
    assert!(matches!(&refusal, crate::EngineError::LookMissing { profile, .. } if *profile == later));
    assert!(refusal.to_string().contains("Later"), "and the message names the look: {refusal}");
    let said = export_photo(&mut s, a, &small, 1).err().unwrap();
    assert_eq!(said, refusal.to_string(), "the export is refused, not written without the look");
    assert!(s.export_job(a, 64, 64, Default::default(), Default::default()).is_err(), "so is every other full render (print, contact sheet)");
    assert!(export_photo(&mut s, b, &small, 1).is_ok(), "a photo without the look exports");
    // (the original's bytes need no look)
    let original = ExportOptions { format: crate::export::ExportFormat::Original, ..Default::default() };
    assert!(crate::export::prepare_export(&mut s, a, &original, 1).err().is_none_or(|e| e != refusal.to_string()));

    // a look nothing is declared about behaves as before: drawn without the table (the pack set without `absent`)
    s.execute("pack.set", &json!({"id": "refusepack", "dir": dir.to_string_lossy(), "group": "Test +"})).unwrap();
    assert!(s.absent_look(&later).is_none());
    assert!(export_photo(&mut s, a, &small, 1).is_ok());
    s.execute("pack.set", &json!({"id": "refusepack", "dir": dir.to_string_lossy(), "group": "Test +", "absent": ["Later"]})).unwrap();
    assert!(export_photo(&mut s, a, &small, 1).is_err());

    // once the table is here the photo exports
    let fetched = temp_dir("refuse-fetched");
    write_cube(&fetched.join("Later.cube"), "Later", 0.5);
    s.execute(
        "pack.set",
        &json!({"id": "refusepack", "dir": dir.to_string_lossy(), "files": [fetched.join("Later.cube").to_string_lossy()], "group": "Test +"}),
    )
    .unwrap();
    assert!(export_photo(&mut s, a, &small, 1).is_ok());
    s.execute("pack.set", &json!({"id": "refusepack"})).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&fetched);
}

#[test]
fn a_picture_drawn_without_the_table_is_not_kept_for_the_look() {
    let (mut s, dir, a, b) = with_absent_looks("salt", "saltpack");
    let (here, later) = (super::cmd::packs::profile_id("saltpack", "Here"), super::cmd::packs::profile_id("saltpack", "Later"));
    s.execute("library.select", &json!({"ids": [a.0]})).unwrap();
    s.execute("develop.profile", &json!({"id": later})).unwrap();
    s.execute("library.select", &json!({"ids": [b.0]})).unwrap();
    s.execute("develop.profile", &json!({"id": here})).unwrap();
    let no_table = super::cmd::packs::NO_TABLE_SALT;
    assert_ne!(s.media.camera_profile_salt(&here), no_table, "a look whose table is here is keyed by its table");
    assert_eq!(s.media.camera_profile_salt(&later), no_table);
    assert_eq!(s.media.camera_profile_salt("lc.color"), 0);
    let keys = |s: &mut Session| {
        let thumb = s.thumb_job(a, 256).unwrap();
        (s.render_job(a, 300, 300, false, true).unwrap().key, thumb.cache.map(|c| c.1), s.thumb_job(b, 256).unwrap().cache.map(|c| c.1))
    };
    let without = keys(&mut s);
    assert!(s.media.take_rebuilt().is_empty());

    let fetched = temp_dir("salt-fetched");
    write_cube(&fetched.join("Later.cube"), "Later", 0.5);
    s.execute(
        "pack.set",
        &json!({"id": "saltpack", "dir": dir.to_string_lossy(), "files": [fetched.join("Later.cube").to_string_lossy()], "group": "Test +"}),
    )
    .unwrap();
    let with = keys(&mut s);
    assert_ne!(without.0, with.0, "the loupe's render is asked for again");
    assert_ne!(without.1, with.1, "and the thumbnail kept on disk is another one");
    assert_eq!(without.2, with.2, "a photo whose look was here all along keeps its thumbnail");
    assert_eq!(s.media.take_rebuilt(), [a], "a host draws the thumbnails of the photos that use the look again");

    // setting the same pack again draws nothing again
    s.execute(
        "pack.set",
        &json!({"id": "saltpack", "dir": dir.to_string_lossy(), "files": [fetched.join("Later.cube").to_string_lossy()], "group": "Test +"}),
    )
    .unwrap();
    assert!(s.media.take_rebuilt().is_empty());
    assert_eq!(keys(&mut s), with);
    s.execute("pack.set", &json!({"id": "saltpack"})).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&fetched);
}

#[test]
fn a_table_replaced_under_its_look_draws_the_photos_that_use_it_again() {
    let (mut s, dir, a, b) = with_absent_looks("swap", "swappack");
    let (here, later) = (super::cmd::packs::profile_id("swappack", "Here"), super::cmd::packs::profile_id("swappack", "Later"));
    let fetched = temp_dir("swap-fetched");
    write_cube(&fetched.join("Later.cube"), "Later", 0.5);
    let set = |s: &mut Session| {
        s.execute(
            "pack.set",
            &json!({"id": "swappack", "dir": dir.to_string_lossy(), "files": [fetched.join("Later.cube").to_string_lossy()], "group": "Test +"}),
        )
        .unwrap()
    };
    set(&mut s);
    s.execute("library.select", &json!({"ids": [a.0]})).unwrap();
    s.execute("develop.profile", &json!({"id": later})).unwrap();
    s.execute("library.select", &json!({"ids": [b.0]})).unwrap();
    s.execute("develop.profile", &json!({"id": here})).unwrap();
    let red = || lightcraft_pipeline::lut::get(&later).unwrap().apply([0.5, 0.5, 0.5])[0];
    assert!((red() - 0.75).abs() < 1e-4);
    let keys = |s: &mut Session| {
        let thumb = s.thumb_job(a, 256).unwrap();
        (s.render_job(a, 300, 300, false, true).unwrap().key, thumb.cache.map(|c| c.1), s.thumb_job(b, 256).unwrap().cache.map(|c| c.1))
    };
    let old = keys(&mut s);
    s.media.take_rebuilt();

    // the server's new table, written where the old one was (as the app keeps a fetched table), and the pack set again
    std::thread::sleep(std::time::Duration::from_millis(20));
    write_cube(&fetched.join("Later.cube"), "Later", 0.9);
    let answer = set(&mut s);
    assert_eq!((answer["changed"].clone(), answer["arrived"].clone()), (json!([later]), json!([])), "{answer}");
    assert!((red() - 0.95).abs() < 1e-4, "the new table is the one drawn");
    let new = keys(&mut s);
    assert_ne!(old.0, new.0, "the loupe's render is asked for again");
    assert_ne!(old.1, new.1, "and the thumbnail kept on disk is another one");
    assert_eq!(old.2, new.2, "a photo with a look whose table stayed keeps its thumbnail");
    assert_eq!(s.media.take_rebuilt(), [a], "a host draws the thumbnails of the photos that use the look again");

    // the same files again: nothing changed, nothing drawn again
    let answer = set(&mut s);
    assert_eq!(answer["changed"], json!([]));
    assert!(s.media.take_rebuilt().is_empty());
    assert_eq!(keys(&mut s), new);
    s.execute("pack.set", &json!({"id": "swappack"})).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&fetched);
}
