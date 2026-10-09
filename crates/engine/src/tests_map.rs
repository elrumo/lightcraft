//! The map commands and place search, through `Session::execute` on a real library.

use serde_json::json;

use crate::Session;

fn open(tag: &str) -> (Session, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("lc-map-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut s = Session::new();
    s.open_library(&dir, true).unwrap();
    (s, dir)
}

/// Three photos in Madrid, one in Paris, one in Sydney; the rest of the library without a position.
fn geotag(s: &mut Session) -> Vec<u64> {
    let ids: Vec<u64> = s.visible_cloned().iter().map(|i| i.0).collect();
    assert!(ids.len() >= 6, "the demo library has photos");
    for (i, gps) in ["40.4169, -3.7035", "40.4170, -3.7036", "40.5400, -3.6400", "48.8584, 2.2945", "-33.8568, 151.2153"].iter().enumerate() {
        s.execute("photo.setMeta", &json!({"ids": [ids[i]], "gps": gps})).unwrap();
    }
    ids
}

#[test]
fn map_points_groups_the_visible_photos() {
    let (mut s, dir) = open("points");
    let ids = geotag(&mut s);
    let world = s.execute("map.points", &json!({"zoom": 2})).unwrap();
    assert_eq!(world["located"], 5);
    assert_eq!(world["unlocated"].as_u64().unwrap() + 5, ids.len() as u64);
    let sizes: Vec<u64> = world["markers"].as_array().unwrap().iter().map(|m| m["count"].as_u64().unwrap()).collect();
    assert_eq!(sizes.iter().sum::<u64>(), 5);
    assert!(sizes.contains(&3), "Madrid and its suburb are one marker at world zoom: {sizes:?}");
    let city = s.execute("map.points", &json!({"zoom": 16, "bbox": [40.0, -4.0, 41.0, -3.0]})).unwrap();
    let in_box: u64 = city["markers"].as_array().unwrap().iter().map(|m| m["count"].as_u64().unwrap()).sum();
    assert_eq!(in_box, 3, "the box keeps Madrid's photos only");
    assert!(city["markers"][0]["ids"].is_array(), "small markers list their photos");
    // the map follows the filter: only 5-star photos
    s.execute("photo.rate", &json!({"ids": [ids[3]], "rating": 5})).unwrap();
    s.execute("library.filter", &json!({"rating": 5})).unwrap();
    let rated = s.execute("map.points", &json!({"zoom": 2})).unwrap();
    assert_eq!(rated["located"], 1);
    assert_eq!(rated["markers"][0]["front"], ids[3]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn map_points_rejects_nonsense() {
    let (mut s, dir) = open("bad");
    geotag(&mut s);
    for p in [
        json!({"zoom": -1}),
        json!({"zoom": 99}),
        json!({"zoom": "x", "cell": 1}),
        json!({"cell": 0}),
        json!({"bbox": [1, 2, 3]}),
        json!({"bbox": "everywhere"}),
    ] {
        let r = s.execute("map.points", &p);
        assert!(r.is_err() || p.get("zoom") == Some(&json!("x")), "{p}: {r:?}");
    }
    assert!(s.execute("map.points", &json!({"bbox": [10, 20, 30, 40], "ids": 0})).is_ok());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn bounds_and_places() {
    let (mut s, dir) = open("bounds");
    geotag(&mut s);
    let b = s.execute("map.bounds", &json!({})).unwrap();
    assert_eq!(b["located"], 5);
    assert!(b["south"].as_f64().unwrap() < -33.0 && b["east"].as_f64().unwrap() > 151.0);
    let hits = s.execute("map.place", &json!({"query": "madrid"})).unwrap();
    assert!(hits[0]["label"].as_str().unwrap().starts_with("Madrid, Spain"), "{hits}");
    assert_eq!(hits[0]["kind"], "city");
    assert_eq!(s.execute("map.place", &json!({"query": "españa"})).unwrap()[0]["kind"], "country");
    assert_eq!(s.execute("map.place", &json!({"query": "zzzzqq"})).unwrap(), json!([]));
    assert!(s.execute("map.place", &json!({})).is_err());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn photo_place_names_where_it_was_taken() {
    let (mut s, dir) = open("place");
    let ids = geotag(&mut s);
    let r = s.execute("photo.place", &json!({"id": ids[0]})).unwrap();
    assert_eq!(r["gps"], json!([40.4169, -3.7035]));
    assert_eq!(r["place"]["city"], "Madrid");
    assert_eq!(r["place"]["country"], "Spain");
    assert_eq!(r["place"]["fromGps"], true);
    let none = s.execute("photo.place", &json!({"id": ids[5]})).unwrap();
    assert!(none["gps"].is_null(), "no position, so nothing is said about where");
    assert!(s.execute("photo.place", &json!({"id": 99999999})).is_err());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn searching_by_place_through_the_library_filter() {
    let (mut s, dir) = open("search");
    let ids = geotag(&mut s);
    s.execute("library.filter", &json!({"text": "photos in madrid"})).unwrap();
    let mut got: Vec<u64> = s.visible_cloned().iter().map(|i| i.0).collect();
    got.sort();
    let mut want = ids[..3].to_vec();
    want.sort();
    assert_eq!(got, want);
    s.execute("library.filter", &json!({"text": "españa or sydney"})).unwrap();
    assert_eq!(s.visible_cloned().len(), 4);
    s.execute("library.filter", &json!({"text": ""})).unwrap();
    assert_eq!(s.visible_cloned().len(), ids.len());
    let _ = std::fs::remove_dir_all(dir);
}
