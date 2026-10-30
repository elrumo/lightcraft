//! Trips and their notes through iCloud and a cloud folder: one record per trip and per note, the albums named by the
//! uid their `album` record travels under; and the references following the library when it changes how it syncs.

use super::*;

fn trip_get(d: &mut Device, id: &Value) -> Value {
    d.s.execute("trip.get", &json!({"id": id})).unwrap()
}

fn photos(d: &mut Device, id: &Value) -> Vec<PhotoId> {
    let ids = d.s.execute("trip.photos", &json!({"id": id})).unwrap()["ids"].clone();
    sorted(ids.as_array().unwrap().iter().map(id_of_num).collect())
}

fn id_of_num(v: &Value) -> PhotoId {
    PhotoId(v.as_u64().unwrap())
}

#[test]
fn a_trip_names_its_albums_by_the_uid_they_travel_under() {
    let root = temp_dir("trips");
    let mut cloud = Cloud::default();
    let (mut a, mut b) = (Device::new(&root, "a"), Device::new(&root, "b"));
    let on_a: Vec<PhotoId> = (1..=3).map(|s| import(&mut a, s)).collect();
    let on_b: Vec<PhotoId> = (1..=3).map(|s| import(&mut b, s)).collect();
    let spain = make_album(&mut a, "Spain", &on_a[..2], None, false);
    let r = a.s.execute("album.ref", &json!({"id": spain.0})).unwrap()["ref"].as_str().unwrap().to_string();
    let uid = r.strip_prefix("u:").expect("by uid through iCloud").to_string();
    assert_eq!(a.s.execute("album.ref", &json!({"id": spain.0})).unwrap()["ref"], json!(r), "made once");
    let trip = a.s.execute("trip.create", &json!({"name": "Spain", "albums": [spain.0]})).unwrap()["trip"].clone();
    assert_eq!(trip["albums"], json!([r]));
    a.s.execute("trip.note.add", &json!({"trip": trip["id"], "text": "Tapas", "photo": on_a[0].0})).unwrap();
    a.sync(&mut cloud);
    b.sync(&mut cloud);
    // the album's record carries the same uid
    assert!(cloud.records.values().any(|x| x.kind == "album" && x.key == uid), "{:?}", cloud.records.values().map(|x| &x.kind).collect::<Vec<_>>());
    assert_eq!(cloud.records.values().filter(|x| x.kind == "trips" || x.kind == "trip-notes").count(), 2, "a record each");
    // on b: b's album of the same name, b's photos of the same pictures
    let b_spain = album_named(&b, "Spain").unwrap();
    let got = trip_get(&mut b, &trip["id"]);
    assert_eq!(got["linked"][0]["album"], json!({"id": b_spain.id.0, "name": "Spain"}));
    assert_eq!(photos(&mut b, &trip["id"]), sorted(on_b[..2].to_vec()));
    assert_eq!(b.s.execute("album.ref", &json!({"id": b_spain.id.0})).unwrap()["ref"], json!(r), "the same on every device");
    assert_eq!(b.s.execute("album.trips", &json!({"id": b_spain.id.0})).unwrap()["trips"][0]["id"], trip["id"]);
    assert_eq!(b.s.execute("trip.notes", &json!({"trip": trip["id"]})).unwrap()["notes"][0]["photoId"], on_b[0].0);
    // edited on b later: a has it
    b.at(2_000);
    b.s.execute("trip.update", &json!({"id": trip["id"], "name": "España"})).unwrap();
    b.sync(&mut cloud);
    a.sync(&mut cloud);
    assert_eq!(trip_get(&mut a, &trip["id"])["trip"]["name"], "España");
    // deleted on a: gone on b with its notes, and from the cloud
    a.at(3_000);
    a.s.execute("trip.delete", &json!({"id": trip["id"]})).unwrap();
    a.sync(&mut cloud);
    b.sync(&mut cloud);
    assert!(b.s.doc_items("trips").is_empty() && b.s.doc_items("trip-notes").is_empty());
    assert_eq!(cloud.records.values().filter(|x| x.kind == "trips" || x.kind == "trip-notes").count(), 0);
    for d in [&mut a, &mut b] {
        assert_eq!(d.s.icloud_changes(), Default::default(), "nothing left to send");
    }
}

#[test]
fn a_trip_that_comes_before_its_album_keeps_the_reference() {
    let root = temp_dir("trips-early");
    let mut cloud = Cloud::default();
    let mut b = Device::new(&root, "b");
    let mine: Vec<PhotoId> = (1..=2).map(|s| import(&mut b, s)).collect();
    let mut put = |kind: &str, key: &str, data: String| {
        let name = crate::icloud::record_name(kind, key);
        cloud.records.insert(name.clone(), Stored { kind: kind.into(), key: key.into(), data, modified: 5, tag: 1 });
        cloud.change(&name, false);
    };
    let trip = json!({"id": "tEarly", "name": "Early", "createdMs": 1, "albums": ["u:feedface00000000"], "tracks": "range", "overrides": []});
    put("trips", "tEarly", trip.to_string());
    b.sync(&mut cloud);
    let got = trip_get(&mut b, &json!("tEarly"));
    assert_eq!((got["linked"][0]["album"].clone(), got["photos"].clone()), (Value::Null, json!(0)), "{got}");
    assert_eq!(b.s.execute("album.resolve", &json!({"ref": "u:feedface00000000"})).unwrap(), json!({"id": null}));
    // its album comes later
    let keys: Vec<String> = mine.iter().map(|id| key_of(&b, *id)).collect();
    let album = json!({"name": "Later", "folder": false, "parent": null, "order": null, "smart": null, "cover": null, "photos": keys});
    let mut put = |kind: &str, key: &str, data: String| {
        let name = crate::icloud::record_name(kind, key);
        cloud.records.insert(name.clone(), Stored { kind: kind.into(), key: key.into(), data, modified: 6, tag: 1 });
        cloud.change(&name, false);
    };
    put("album", "feedface00000000", album.to_string());
    b.sync(&mut cloud);
    let later = album_named(&b, "Later").unwrap();
    assert_eq!(trip_get(&mut b, &json!("tEarly"))["linked"][0]["album"], json!({"id": later.id.0, "name": "Later"}));
    assert_eq!(photos(&mut b, &json!("tEarly")), sorted(mine));
    assert_eq!(b.s.doc_items("trips")[0]["albums"], json!(["u:feedface00000000"]), "the reference itself never changed");
}

/// The album references of trip `id`.
fn refs(s: &mut Session, id: &Value) -> Value {
    s.execute("trip.get", &json!({"id": id})).unwrap()["trip"]["albums"].clone()
}

#[test]
fn references_follow_the_library_when_it_changes_how_it_syncs() {
    let root = temp_dir("trips-mode");
    let mut s = Session::new().with_fs();
    s.open_library(root.join("lib"), false).unwrap();
    let add = |s: &mut Session, name: &str| {
        let id = s.catalog.alloc_album_id();
        s.apply_system(lightcraft_catalog::Op::AddAlbum { album: lightcraft_catalog::Album::new(id, name) }).unwrap();
        id
    };
    let (alps, gone) = (add(&mut s, "Alps"), add(&mut s, "Gone"));
    let t = s.execute("trip.create", &json!({"name": "Alps", "albums": [alps.0, gone.0]})).unwrap()["trip"]["id"].clone();
    s.apply_system(lightcraft_catalog::Op::RemoveAlbum { id: gone }).unwrap();
    let by_id = json!([format!("a:{}", alps.0), format!("a:{}", gone.0)]);
    assert_eq!(refs(&mut s, &t), by_id, "a library that syncs with no one: by id");
    // iCloud on: by uid, the one the album's record will carry; the deleted album's reference is kept as it was
    s.execute("icloud.enable", &json!({"on": true})).unwrap();
    let r = refs(&mut s, &t);
    let uid = r[0].as_str().unwrap().strip_prefix("u:").expect("by uid").to_string();
    assert_eq!(r[1], by_id[1]);
    let changes = s.icloud_changes();
    let records = s.icloud_records(&changes.save);
    assert!(records.iter().any(|x| x["kind"] == "album" && x["key"] == uid), "{records:?}");
    assert!(records.iter().any(|x| x["kind"] == "trips" && x["data"].as_str().unwrap().contains(&uid)), "sent by uid");
    assert_eq!(s.execute("album.resolve", &json!({"ref": r[0]})).unwrap()["id"], alps.0);
    assert_eq!(s.execute("album.resolve", &json!({"ref": by_id[0]})).unwrap()["id"], Value::Null, "an id means nothing through iCloud");
    // kept across a reopen
    s.close_library().unwrap();
    drop(s);
    let mut s = Session::new().with_fs();
    s.open_library(root.join("lib"), false).unwrap();
    assert_eq!(refs(&mut s, &t), r);
    // signing in to a server (which keeps the albums then): by id again, from the sign-in on
    s.execute("sync.signIn", &json!({"server": "http://fake", "user": "ann", "password": "pw", "device": "test"})).unwrap();
    assert_eq!(s.doc_items("trips")[0]["albums"], by_id);
    // a sign-in that went nowhere (signed out again): sent through iCloud by the same uid as before
    s.sync = None;
    let changes = s.icloud_changes();
    let sent = s.icloud_records(&changes.save);
    let trip = sent.iter().find(|x| x["kind"] == "trips").expect("the trip goes again");
    assert_eq!(serde_json::from_str::<Value>(trip["data"].as_str().unwrap()).unwrap()["albums"], r);
    assert_eq!(refs(&mut s, &t), r);
    // iCloud off: by id
    s.execute("icloud.enable", &json!({"on": false})).unwrap();
    assert_eq!(s.doc_items("trips")[0]["albums"], by_id);
    // on again through a cloud folder: by uid (a new one: turning sync on starts over)
    let shared = root.join("Dropbox");
    std::fs::create_dir_all(&shared).unwrap();
    s.execute("icloud.useFolder", &json!({"path": shared.to_string_lossy()})).unwrap();
    let again = refs(&mut s, &t);
    assert!(again[0].as_str().unwrap().starts_with("u:") && again[1] == by_id[1], "{again}");
    assert_eq!(s.execute("album.resolve", &json!({"ref": again[0]})).unwrap()["id"], alps.0);
}

#[test]
fn trips_travel_through_a_cloud_folder() {
    let root = temp_dir("trips-folder");
    let shared = root.join("Dropbox");
    std::fs::create_dir_all(&shared).unwrap();
    let (mut a, mut b) = (Device::new(&root, "a"), Device::new(&root, "b"));
    let on_a: Vec<PhotoId> = (1..=2).map(|s| import(&mut a, s)).collect();
    let on_b: Vec<PhotoId> = (1..=2).map(|s| import(&mut b, s)).collect();
    let album = make_album(&mut a, "Coast", &on_a, None, false);
    let t = a.s.execute("trip.create", &json!({"name": "Coast", "albums": [album.0]})).unwrap()["trip"]["id"].clone();
    a.s.execute("trip.note.add", &json!({"trip": t, "text": "Waves"})).unwrap();
    folder_pass(&mut a, &shared);
    folder_pass(&mut b, &shared);
    assert_eq!(photos(&mut b, &t), sorted(on_b));
    assert_eq!(b.s.execute("trip.notes", &json!({"trip": t})).unwrap()["notes"][0]["text"], "Waves");
    b.at(2_000);
    b.s.execute("trip.delete", &json!({"id": t})).unwrap();
    folder_pass(&mut b, &shared);
    folder_pass(&mut a, &shared);
    assert!(a.s.doc_items("trips").is_empty() && a.s.doc_items("trip-notes").is_empty(), "a removal sticks, notes and all");
}
