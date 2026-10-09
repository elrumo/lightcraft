//! Finding people with the deterministic stand-in (halves of a photo "have a face" when bright, and
//! look like their dominant colour): when faces are looked for, how people are listed, named and
//! shown, and failures that must not take the session down.

use std::sync::Arc;

use lightcraft_raster::Rgba8;
use lightcraft_vision::faces::FaceFound;
use lightcraft_vision::fake::FakeFaces;
use lightcraft_vision::{Error, FaceEngine};
use serde_json::{Value, json};

use crate::Session;

fn demo() -> Session {
    let mut s = Session::with_demo();
    s.vision.set_face_finder(Arc::new(FakeFaces));
    s.vision.text_edge = 192;
    s
}

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("lc-vision-people-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

/// Turns finding on and looks at every photo.
fn scanned(s: &mut Session) -> Value {
    s.execute("vision.setFaces", &json!({"on": true})).unwrap();
    s.execute("vision.index", &json!({"wait": true})).unwrap()
}

fn people(s: &mut Session, all: bool) -> Vec<Value> {
    s.execute("people.list", &json!({"all": all})).unwrap()["people"].as_array().unwrap().clone()
}

fn named_faces(s: &Session, name: &str) -> usize {
    s.catalog.photos().flat_map(|p| p.meta.regions.iter()).filter(|r| r.name.as_deref() == Some(name)).count()
}

#[test]
fn faces_are_looked_for_only_after_the_user_turns_them_on() {
    let mut s = demo();
    assert!(!s.vision.faces, "off by default");
    s.vision.set_embedder(Arc::new(lightcraft_vision::fake::FakeEmbedder));
    let r = s.execute("vision.index", &json!({"wait": true})).unwrap();
    assert_eq!(r["phase"], "describing", "{r}");
    assert_eq!(s.vision.faces_scanned(), 0);
    assert_eq!(s.execute("vision.model.status", &json!({})).unwrap()["faces"]["found"], Value::Null, "no face index was opened");

    assert_eq!(s.execute("vision.setFaces", &json!({"on": true})).unwrap()["enabled"], true);
    let r = s.execute("vision.index", &json!({"wait": true})).unwrap();
    let photos = s.vision_keys().len();
    assert_eq!(
        (r["total"].as_u64(), r["failed"].as_u64(), r["error"].clone()),
        (Some(photos as u64), Some(0), Value::Null),
        "only faces are left: {r}"
    );
    assert_eq!(r["phase"], "finding");
    assert_eq!(s.vision.faces_scanned(), photos, "every photo was looked at, with or without faces");
    let st = s.execute("vision.model.status", &json!({})).unwrap();
    assert_eq!(
        (st["faces"]["enabled"].clone(), st["faces"]["installed"].clone(), st["faces"]["scanned"].clone()),
        (json!(true), json!(true), json!(photos)),
        "{st}"
    );
    assert!(s.vision.faces_found() > 0, "the demo photos have bright halves");

    // nothing is looked at twice
    assert_eq!(s.execute("vision.index", &json!({"wait": true})).unwrap()["total"], 0);
}

#[test]
fn faces_that_look_alike_are_one_person() {
    let mut s = demo();
    scanned(&mut s);
    let all = people(&mut s, true);
    assert!(!all.is_empty());
    let faces: u64 = all.iter().map(|p| p["faces"].as_u64().unwrap()).sum();
    assert_eq!(faces as usize, s.vision.faces_found(), "every face is somebody");
    let sizes: Vec<u64> = all.iter().map(|p| p["faces"].as_u64().unwrap()).collect();
    assert!(sizes.windows(2).all(|w| w[0] >= w[1]), "biggest first: {sizes:?}");
    for p in &all {
        assert!(p["name"].is_null(), "nobody is named yet");
        assert_eq!(p["pending"], p["faces"], "none of their faces is written to a photo");
        let cover = &p["cover"];
        assert!(cover["photo"].as_u64().is_some() && cover["rect"]["x1"].as_f64().unwrap() > cover["rect"]["x0"].as_f64().unwrap());
        assert!(!p["photoIds"].as_array().unwrap().is_empty());
    }
    let ids: std::collections::HashSet<&str> = all.iter().map(|p| p["id"].as_str().unwrap()).collect();
    assert_eq!(ids.len(), all.len(), "each person has an id of their own");
    // a limit
    assert_eq!(s.execute("people.list", &json!({"limit": 1})).unwrap()["people"].as_array().unwrap().len(), 1);
}

#[test]
fn naming_a_person_names_their_faces_in_the_catalog() {
    let mut s = demo();
    scanned(&mut s);
    let first = people(&mut s, false)[0].clone();
    let id = first["id"].as_str().unwrap();
    let (faces, photos) = (first["faces"].as_u64().unwrap() as usize, first["photos"].as_u64().unwrap() as usize);

    assert_eq!(named_faces(&s, "Ada Lovelace"), 0);
    let r = s.execute("people.name", &json!({"cluster": id, "name": "  Ada   Lovelace "})).unwrap();
    assert_eq!(
        (r["name"].as_str(), r["faces"].as_u64(), r["photos"].as_u64(), r["keptOtherName"].as_u64()),
        (Some("Ada Lovelace"), Some(faces as u64), Some(photos as u64), Some(0)),
        "{r}"
    );
    assert!(named_faces(&s, "Ada Lovelace") >= faces.min(photos), "face regions with her name are on the photos");
    // the People view that already exists now has her, and the person: search finds her photos
    let named = s.catalog.people();
    let ada = named.iter().find(|p| p.name == "Ada Lovelace").expect("a named person");
    assert_eq!(ada.count, photos);
    s.execute("library.filter", &json!({"person": "ada lovelace"})).unwrap();
    assert_eq!(s.visible_cloned().len(), photos);
    s.execute("library.filter", &json!({"person": null})).unwrap();

    // she is no longer an unnamed person, but is there when everyone is asked for
    assert!(people(&mut s, false).iter().all(|p| p["id"] != id));
    let again = people(&mut s, true);
    let ada = again.iter().find(|p| p["id"] == id).unwrap();
    assert_eq!((ada["name"].as_str(), ada["pending"].as_u64()), (Some("Ada Lovelace"), Some(0)), "{ada}");

    // one undo takes the names back
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(named_faces(&s, "Ada Lovelace"), 0);
    assert!(people(&mut s, false).iter().any(|p| p["id"] == id), "unnamed again");
}

#[test]
fn a_face_that_has_another_name_keeps_it_and_confirming_names_the_rest() {
    let mut s = demo();
    scanned(&mut s);
    let first = people(&mut s, false)[0].clone();
    let id = first["id"].as_str().unwrap().to_string();

    // somebody already wrote "Bob" on one of these faces: the cover face's photo
    let cover = first["cover"]["photo"].as_u64().unwrap();
    let rect = &first["cover"]["rect"];
    let mut meta = s.catalog.photo(lightcraft_catalog::PhotoId(cover)).unwrap().meta.clone();
    meta.regions.push(lightcraft_meta::Region {
        rect: lightcraft_geom::Rect {
            x0: rect["x0"].as_f64().unwrap(),
            y0: rect["y0"].as_f64().unwrap(),
            x1: rect["x1"].as_f64().unwrap(),
            y1: rect["y1"].as_f64().unwrap(),
        },
        kind: lightcraft_meta::RegionKind::Face,
        name: Some("Bob".into()),
        description: None,
    });
    s.commit("Name Bob", lightcraft_catalog::Op::SetMeta { id: lightcraft_catalog::PhotoId(cover), meta: Box::new(meta) }).unwrap();

    let r = s.execute("people.name", &json!({"cluster": id, "name": "Ada"})).unwrap();
    assert_eq!(r["keptOtherName"], 1, "{r}");
    assert_eq!(named_faces(&s, "Bob"), 1, "Bob is still Bob");
    // the person is Bob's cluster by vote or Ada's: either way one name; confirming again adds nothing
    let again = s.execute("people.name", &json!({"cluster": id, "name": "Ada"})).unwrap();
    assert_eq!(again["faces"], 0, "{again}");

    // a face region removed by hand shows up as a face waiting to be confirmed, and naming again adds it
    let ada_photo = s.catalog.photos().find(|p| p.meta.regions.iter().any(|r| r.name.as_deref() == Some("Ada"))).map(|p| p.id).unwrap();
    let index = s.catalog.photo(ada_photo).unwrap().meta.regions.iter().position(|r| r.name.as_deref() == Some("Ada")).unwrap();
    s.execute("photo.removeRegion", &json!({"id": ada_photo.0, "index": index})).unwrap();
    let all = people(&mut s, false);
    let p = all.iter().find(|p| p["id"] == id).expect("listed again: a face is waiting");
    assert_eq!((p["name"].as_str(), p["pending"].as_u64()), (Some("Ada"), Some(1)), "{p}");
    let back = s.execute("people.name", &json!({"cluster": id, "name": "Ada"})).unwrap();
    assert_eq!(back["faces"], 1, "{back}");
    assert!(people(&mut s, false).iter().all(|p| p["id"] != id), "all confirmed");
}

#[test]
fn a_person_shows_their_photos_and_a_bad_request_is_an_error() {
    let mut s = demo();
    scanned(&mut s);
    let first = people(&mut s, false)[0].clone();
    let id = first["id"].as_str().unwrap();
    let r = s.execute("people.show", &json!({"cluster": id})).unwrap();
    let shown: Vec<u64> = r["photos"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect();
    assert_eq!(shown.len(), first["photos"].as_u64().unwrap() as usize);
    assert_eq!(s.visible_cloned().iter().map(|i| i.0).collect::<Vec<_>>(), shown);
    assert_eq!(s.filter.semantic.as_deref(), Some("an unnamed person"));
    s.execute("library.filter", &json!({"semantic": null, "only": []})).unwrap();

    for (name, why) in [("", "empty"), ("   ", "blank"), ("a\u{7}b", "a control character"), (&"x".repeat(500), "too long")] {
        assert!(s.execute("people.name", &json!({"cluster": id, "name": name})).is_err(), "{why}");
    }
    assert!(s.execute("people.name", &json!({"cluster": "nonsense", "name": "Ada"})).is_err());
    assert!(s.execute("people.name", &json!({"name": "Ada"})).is_err());
    assert!(s.execute("people.show", &json!({"cluster": "ffffffffffffffffffffffffffffffff.0"})).is_err());
    assert_eq!(named_faces(&s, "Ada"), 0, "nothing was written");
}

#[test]
fn faces_survive_a_restart_and_can_be_forgotten() {
    let dir = temp_dir("persist");
    let mut s = Session::new();
    s.open_library(&dir, true).unwrap();
    s.vision.set_face_finder(Arc::new(FakeFaces));
    s.vision.text_edge = 192;
    scanned(&mut s);
    let (found, photos) = (s.vision.faces_found(), s.vision.faces_scanned());
    assert!(found > 0 && dir.join("search").join("faces-fake-faces.bin").is_file());
    let before: Vec<String> = people(&mut s, true).iter().map(|p| p["id"].as_str().unwrap().to_string()).collect();
    s.close_library().ok();
    drop(s);

    let mut again = Session::new();
    again.open_library(&dir, true).unwrap();
    assert!(again.vision.faces, "the choice was saved with the library");
    again.vision.set_face_finder(Arc::new(FakeFaces));
    again.vision.text_edge = 192;
    assert_eq!(again.execute("vision.index", &json!({"wait": true})).unwrap()["total"], 0, "nothing is looked at twice");
    assert_eq!((again.vision.faces_found(), again.vision.faces_scanned()), (found, photos));
    let after: Vec<String> = people(&mut again, true).iter().map(|p| p["id"].as_str().unwrap().to_string()).collect();
    assert_eq!(after, before, "the same people");

    // forgetting empties it, on disk too
    let r = again.execute("people.deleteData", &json!({})).unwrap();
    assert_eq!((r["faces"].as_u64(), r["photos"].as_u64()), (Some(found as u64), Some(photos as u64)));
    assert_eq!((again.vision.faces_found(), again.vision.faces_scanned()), (0, 0));
    assert!(people(&mut again, true).is_empty());
    assert_eq!(std::fs::metadata(dir.join("search").join("faces-fake-faces.bin")).unwrap().len() as usize, lightcraft_vision::store::HEADER_LEN);
    // and photos can be looked at again
    assert!(again.execute("vision.index", &json!({"wait": true})).unwrap()["total"].as_u64().unwrap() > 0);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A finder that fails, or panics, on every photo.
struct Broken {
    panic: bool,
}

impl FaceEngine for Broken {
    fn engine(&self) -> &str {
        "broken-faces"
    }
    fn faces(&self, _: &Rgba8) -> Result<Vec<FaceFound>, Error> {
        if self.panic { panic!("faces exploded") } else { Err(Error::Model("no faces today".into())) }
    }
}

#[test]
fn a_failing_or_panicking_finder_is_an_error_not_a_crash() {
    for panic in [false, true] {
        let mut s = Session::with_demo();
        s.vision.set_face_finder(Arc::new(Broken { panic }));
        s.vision.text_edge = 192;
        s.execute("vision.setFaces", &json!({"on": true})).unwrap();
        let r = s.execute("vision.index", &json!({"wait": true})).unwrap();
        assert_eq!(r["running"], json!(false), "the run ends: {r}");
        assert!(r["error"].as_str().is_some_and(|e| e.contains("stopped") && (e.contains("today") || e.contains("exploded"))), "{r}");
        assert_eq!(s.vision.faces_scanned(), 0);
        assert!(people(&mut s, true).is_empty());
        assert!(s.execute("photo.rate", &json!({"rating": 3})).is_ok(), "and the session carries on");
    }
}

#[test]
fn the_face_models_are_downloaded_only_when_the_user_agreed() {
    let mut s = demo();
    let e = s.execute("vision.faces.download", &json!({})).unwrap_err().to_string();
    assert!(e.contains("acknowledged") && e.contains("MIT") && e.contains("Apache"), "{e}");
    assert_eq!(s.execute("vision.faces.download", &json!({"acknowledged": true})).unwrap()["started"], json!(false), "installed already");
    assert_eq!(s.execute("vision.faces.cancel", &json!({})).unwrap()["cancelled"], json!(false));

    let mut plain = Session::with_demo();
    let st = plain.execute("vision.model.status", &json!({})).unwrap()["faces"].clone();
    assert_eq!((st["installed"].clone(), st["enabled"].clone()), (json!(false), json!(false)), "{st}");
    // finding people is not on for a library that never asked
    assert!(plain.execute("people.list", &json!({})).unwrap()["people"].as_array().unwrap().is_empty());
    if !cfg!(feature = "vision") {
        assert!(plain.execute("vision.faces.download", &json!({"acknowledged": true})).is_err());
    }
}
