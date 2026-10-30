//! Trips and their notes through a sync server: the `trips` and `trip-notes` documents, merged item by item and field by
//! field, with album references by id (a server shares album ids).

use super::*;

fn trip_named(s: &mut Session, name: &str) -> Option<Value> {
    s.doc_items("trips").into_iter().find(|t| t["name"] == name)
}

fn notes_of(s: &Session, trip: &Value) -> Vec<String> {
    let mut v: Vec<String> =
        s.doc_items("trip-notes").iter().filter(|n| n["trip"] == *trip).map(|n| n["text"].as_str().unwrap().to_string()).collect();
    v.sort();
    v
}

/// Sync both devices until they agree.
fn settle(a: &mut Session, b: &mut Session, f: &mut Fake) {
    sync(a, f);
    sync(b, f);
    sync(a, f);
}

fn update(s: &mut Session, p: Value) {
    s.execute("trip.update", &p).unwrap();
}

#[test]
fn trips_and_their_notes_follow_the_user_to_their_other_devices() {
    let mut f = Fake::new();
    let (mut a, mut b, root) = two_devices("trips", &mut f);
    let photos: Vec<u64> = a.catalog.photos().map(|p| p.id.0).collect();
    let alps = a.execute("album.create", &json!({"name": "Alps"})).unwrap()["id"].as_u64().unwrap();
    a.execute("album.addPhotos", &json!({"id": alps, "ids": photos})).unwrap();
    let lakes = a.execute("album.create", &json!({"name": "Lakes"})).unwrap()["id"].as_u64().unwrap();
    let trip = a.execute("trip.create", &json!({"name": "Alps", "albums": [alps]})).unwrap()["trip"].clone();
    assert_eq!(trip["albums"], json!([format!("a:{alps}")]), "a server shares album ids");
    a.execute("trip.note.add", &json!({"trip": trip["id"], "text": "From A", "photo": photos[0]})).unwrap();
    settle(&mut a, &mut b, &mut f);
    // the same trip, the same album, the same photos
    let got = b.execute("trip.get", &json!({"id": trip["id"]})).unwrap();
    assert_eq!(got["trip"], trip);
    assert_eq!(got["linked"][0]["album"], json!({"id": alps, "name": "Alps"}));
    let photos_of = |s: &mut Session| s.execute("trip.photos", &json!({"id": trip["id"]})).unwrap()["ids"].clone();
    assert_eq!(photos_of(&mut b), photos_of(&mut a));
    assert_eq!(b.execute("album.trips", &json!({"id": alps})).unwrap()["trips"][0]["id"], trip["id"]);
    assert_eq!(b.execute("trip.notes", &json!({"trip": trip["id"]})).unwrap()["notes"][0]["photoId"], photos[0], "the note's photo, by content");
    // notes added on both while apart: all kept
    b.execute("trip.note.add", &json!({"trip": trip["id"], "text": "From B"})).unwrap();
    a.execute("trip.note.add", &json!({"trip": trip["id"], "text": "Again from A"})).unwrap();
    settle(&mut a, &mut b, &mut f);
    for s in [&a, &b] {
        assert_eq!(notes_of(s, &trip["id"]), vec!["Again from A", "From A", "From B"]);
    }
    // different fields of one trip changed on both: both changes kept
    update(&mut a, json!({"id": trip["id"], "name": "Alps 2026"}));
    update(&mut b, json!({"id": trip["id"], "range": {"fromMs": 1, "toMs": 2}, "photosInRange": true}));
    settle(&mut a, &mut b, &mut f);
    for s in [&mut a, &mut b] {
        let t = trip_named(s, "Alps 2026").expect("renamed");
        assert_eq!((t["range"].clone(), t["photosInRange"].clone()), (json!({"fromMs": 1, "toMs": 2}), json!(true)));
    }
    // the same field on both: one of the two, the same everywhere
    update(&mut a, json!({"id": trip["id"], "name": "Named on A"}));
    update(&mut b, json!({"id": trip["id"], "name": "Named on B"}));
    settle(&mut a, &mut b, &mut f);
    assert_eq!(a.doc_items("trips"), b.doc_items("trips"));
    assert!(["Named on A", "Named on B"].contains(&a.doc_items("trips")[0]["name"].as_str().unwrap()));
    // a list (the albums) changed on both: one value, one side kept whole (accepted for now)
    a.execute("trip.link", &json!({"id": trip["id"], "album": lakes})).unwrap();
    b.execute("trip.unlink", &json!({"id": trip["id"], "album": alps})).unwrap();
    settle(&mut a, &mut b, &mut f);
    let albums = a.doc_items("trips")[0]["albums"].clone();
    assert_eq!(albums, b.doc_items("trips")[0]["albums"]);
    assert!(albums == json!([format!("a:{alps}"), format!("a:{lakes}")]) || albums == json!([]), "{albums}");
    // quiet when nothing changed
    let before = f.requests.len();
    settle(&mut a, &mut b, &mut f);
    assert!(f.requests[before..].iter().all(|r| !r.starts_with("PUT /api/docs/trip")), "{:?}", &f.requests[before..]);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_trip_deleted_on_one_device_goes_with_its_notes_everywhere() {
    let mut f = Fake::new();
    let (mut a, mut b, root) = two_devices("trips-delete", &mut f);
    let gone = a.execute("trip.create", &json!({"name": "Gone"})).unwrap()["trip"]["id"].clone();
    let kept = a.execute("trip.create", &json!({"name": "Kept"})).unwrap()["trip"]["id"].clone();
    for t in [&gone, &kept] {
        a.execute("trip.note.add", &json!({"trip": t, "text": "a"})).unwrap();
    }
    settle(&mut a, &mut b, &mut f);
    b.execute("trip.note.add", &json!({"trip": gone, "text": "b"})).unwrap();
    sync(&mut b, &mut f);
    a.execute("trip.delete", &json!({"id": gone})).unwrap();
    // and meanwhile b adds one more note to it
    b.execute("trip.note.add", &json!({"trip": gone, "text": "late"})).unwrap();
    settle(&mut a, &mut b, &mut f);
    sync(&mut b, &mut f);
    for s in [&mut a, &mut b] {
        assert!(trip_named(s, "Gone").is_none());
        assert!(notes_of(s, &gone).is_empty(), "every note of it, the late one too");
        assert_eq!(notes_of(s, &kept), vec!["a"]);
    }
    assert!(f.docs["trip-notes"].items.as_array().unwrap().iter().all(|n| n["trip"] != gone));
    // deleted on one, edited on the other meanwhile: the edit wins and the trip stays (the notes the deletion took
    // stay gone)
    b.execute("trip.note.add", &json!({"trip": kept, "text": "b"})).unwrap();
    settle(&mut a, &mut b, &mut f);
    a.execute("trip.delete", &json!({"id": kept})).unwrap();
    update(&mut b, json!({"id": kept, "name": "Still here"}));
    sync(&mut a, &mut f);
    settle(&mut b, &mut a, &mut f);
    for s in [&mut a, &mut b] {
        assert!(trip_named(s, "Still here").is_some());
        assert!(notes_of(s, &kept).is_empty());
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn trips_from_the_server_are_checked_item_by_item() {
    let mut f = Fake::new();
    let (mut a, root, _) = first_device("trips-hostile", &mut f);
    let good = a.execute("trip.create", &json!({"name": "Good"})).unwrap()["trip"].clone();
    sync(&mut a, &mut f);
    let mut items =
        vec![good.clone(), json!({"id": "t../x", "name": "a", "createdMs": 1}), json!({"id": "tN", "name": ["x"], "createdMs": 1}), json!(7)];
    items.extend((0..3000).map(|i| json!({"id": format!("tMany{i}"), "name": "m", "createdMs": i})));
    let version = f.docs["trips"].version;
    f.docs.insert("trips".into(), proto::Doc { version: version + 1, items: Value::Array(items) });
    let st = sync(&mut a, &mut f);
    assert_eq!(st["state"], "idle", "{st}");
    let kept = a.doc_items("trips");
    // (the first 2000 items the server sent, of which three are not trips)
    assert_eq!((kept.len(), kept[0].clone()), (1997, good));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_older_server_leaves_the_trips_alone() {
    let mut f = Fake::new();
    f.unknown_docs = vec!["trips", "trip-notes"];
    let (mut a, mut b, root) = two_devices("trips-old", &mut f);
    let t = a.execute("trip.create", &json!({"name": "Here"})).unwrap()["trip"].clone();
    a.execute("trip.note.add", &json!({"trip": t["id"], "text": "x"})).unwrap();
    a.execute("export.savePreset", &json!({"name": "Web", "params": {"format": "jpeg"}})).unwrap();
    let st = sync(&mut a, &mut f);
    assert_eq!(st["state"], "idle", "{st}");
    sync(&mut b, &mut f);
    assert!(b.export_presets.iter().any(|p| p.name == "Web"), "the other documents sync as ever");
    assert!(b.doc_items("trips").is_empty() && b.doc_items("trip-notes").is_empty());
    assert_eq!(a.doc_items("trips"), vec![t], "kept on the device");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn signing_out_takes_the_trips_with_the_account() {
    let mut f = Fake::new();
    let (mut a, root, _) = first_device("trips-out", &mut f);
    let t = a.execute("trip.create", &json!({"name": "Mine"})).unwrap()["trip"]["id"].clone();
    a.execute("trip.note.add", &json!({"trip": t, "text": "x"})).unwrap();
    a.execute("sync.signOut", &json!({"confirm": true})).unwrap();
    a.sync_sign_out_wait_with(10_000, &mut |t| f.handle(t));
    assert_eq!((f.docs["trips"].items.as_array().unwrap().len(), f.docs["trip-notes"].items.as_array().unwrap().len()), (1, 1), "sent first");
    assert!(a.doc_items("trips").is_empty() && a.doc_items("trip-notes").is_empty(), "then gone from the library");
    let _ = std::fs::remove_dir_all(&root);
}
