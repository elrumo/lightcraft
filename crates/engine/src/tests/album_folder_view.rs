//! A folder of albums as the grid's source (`library.source {kind: albumFolder}`): its albums' photos, album after
//! album in the sidebar's order (the folders inside it too), each photo once, and `library.groups` heading each
//! album's run with its name.

use serde_json::{Value, json};

use crate::Session;

fn make(s: &mut Session, name: &str, parent: Option<u64>, folder: bool) -> u64 {
    let mut p = json!({"name": name, "folder": folder});
    if let Some(parent) = parent {
        p["parent"] = json!(parent);
    }
    s.execute("album.create", &p).unwrap()["id"].as_u64().unwrap()
}

fn add(s: &mut Session, album: u64, ids: &[u64]) {
    s.execute("album.addPhotos", &json!({"id": album, "ids": ids})).unwrap();
}

/// Given folder "2022" with albums "B" and "A" and a folder "Inner" holding "C", when the folder is shown, then the
/// grid lists Inner's C first (folders first), then A, then B, a photo in both A and B only under A, and the headers
/// say so; an album, a smart filter and a bad id behave.
#[test]
fn a_folder_shows_its_albums_photos_album_after_album() {
    let mut s = Session::with_demo();
    let ids: Vec<u64> = s.visible_cloned().iter().map(|p| p.0).take(6).collect();
    assert_eq!(ids.len(), 6, "the demo library has photos");
    let folder = make(&mut s, "2022", None, true);
    let b = make(&mut s, "B", Some(folder), false);
    let a = make(&mut s, "A", Some(folder), false);
    let inner = make(&mut s, "Inner", Some(folder), true);
    let c = make(&mut s, "C", Some(inner), false);
    let _empty = make(&mut s, "Empty", Some(folder), false);
    add(&mut s, a, &ids[0..2]);
    add(&mut s, b, &ids[1..4]);
    add(&mut s, c, &ids[4..6]);

    let count = s.execute("library.source", &json!({"kind": "albumFolder", "id": folder})).unwrap()["count"].as_u64();
    assert_eq!(count, Some(6), "each photo once");
    let groups: Vec<Value> = serde_json::from_value(s.execute("library.groups", &json!({"by": "year"})).unwrap()).unwrap();
    let heads: Vec<(String, u64, u64)> =
        groups.iter().map(|g| (g["label"].as_str().unwrap().to_string(), g["start"].as_u64().unwrap(), g["count"].as_u64().unwrap())).collect();
    assert_eq!(heads, [("C".into(), 0, 2), ("A".into(), 2, 2), ("B".into(), 4, 2)], "the empty album has no header");
    assert_eq!(groups[1]["key"], json!(format!("album:{a}")));
    let shown: Vec<u64> = s.visible_cloned().iter().map(|p| p.0).collect();
    assert!(shown[4..6].iter().all(|id| ids[2..4].contains(id)), "B keeps only what A didn't show");
    assert_eq!(s.source_total(), Some(6));

    // the filter bar narrows each album
    s.execute("library.filter", &json!({"only": [ids[0], ids[5]]})).unwrap();
    assert_eq!(s.visible_cloned().len(), 2);
    s.execute("library.filter", &json!({"only": []})).unwrap();

    // an album is no folder, nor is an unknown id
    assert!(s.execute("library.source", &json!({"kind": "albumFolder", "id": a})).is_err());
    assert!(s.execute("library.source", &json!({"kind": "albumFolder", "id": 987_654})).is_err());
    // an album shown has date headers again, not album ones
    s.execute("library.source", &json!({"kind": "album", "id": a})).unwrap();
    let groups = s.execute("library.groups", &json!({})).unwrap();
    assert!(groups.as_array().unwrap().iter().all(|g| !g["key"].as_str().unwrap_or("").starts_with("album:")));

    // deleting the folder shown goes back to everything
    s.execute("library.source", &json!({"kind": "albumFolder", "id": folder})).unwrap();
    s.execute("album.delete", &json!({"id": folder})).unwrap();
    assert!(
        s.execute("library.groups", &json!({})).unwrap().as_array().unwrap().iter().all(|g| !g["key"].as_str().unwrap_or("").starts_with("album:"))
    );
}
