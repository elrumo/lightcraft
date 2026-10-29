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

/// The five geotagged photos on a made-up trip: two at one spot in Madrid, its suburb two hours later,
/// Paris three hours after that, and Sydney a week later.
fn on_a_trip(s: &mut Session) -> Vec<u64> {
    let ids = geotag(s);
    for (i, t) in
        ["2026-03-01T09:00:00", "2026-03-01T09:05:00", "2026-03-01T11:00:00", "2026-03-01T14:00:00", "2026-03-09T09:00:00"].iter().enumerate()
    {
        s.execute("photo.setCaptureTime", &json!({"ids": [ids[i]], "time": t})).unwrap();
    }
    ids
}

#[test]
fn map_route_estimates_a_path_through_the_photos_in_time_order() {
    let (mut s, dir) = open("route");
    let ids = on_a_trip(&mut s);
    let r = s.execute("map.route", &json!({})).unwrap();
    assert_eq!(r["estimated"], true);
    assert_eq!((r["photos"].as_u64(), r["zoneEstimated"].as_u64()), (Some(5), Some(5)));
    assert_eq!(r["skipped"]["noGps"].as_u64().unwrap() + 5, ids.len() as u64);
    let routes = r["routes"].as_array().unwrap();
    assert_eq!(routes.len(), 2, "Sydney is a week later: {r}");
    let trip = &routes[0];
    assert_eq!((trip["startIso"].as_str(), trip["endIso"].as_str()), (Some("2026-03-01T09:00:00Z"), Some("2026-03-01T14:00:00Z")));
    assert_eq!(trip["stops"].as_array().unwrap().len(), 3, "the two photos at one spot are one stop");
    assert_eq!(trip["stops"][0]["photos"], json!([ids[0], ids[1]]));
    assert_eq!(trip["stops"][0]["count"], 2);
    let (bike, flight) = (&trip["legs"][0], &trip["legs"][1]);
    assert_eq!((bike["mode"].as_str(), bike["arc"].as_bool()), (Some("bike"), Some(false)));
    assert_eq!((flight["mode"].as_str(), flight["arc"].as_bool()), (Some("flight"), Some(true)));
    assert!(flight["distanceKm"].as_f64().unwrap() > 1000.0);
    let line = flight["lines"][0].as_array().unwrap();
    assert!(line.len() > 10, "{}", line.len());
    assert_eq!(line[0], json!([trip["stops"][1]["lat"], trip["stops"][1]["lon"]]), "the line leaves the stop");
    assert_eq!(line[line.len() - 1], json!([trip["stops"][2]["lat"], trip["stops"][2]["lon"]]), "and reaches the next");
    assert_eq!((routes[1]["stops"].as_array().unwrap().len(), routes[1]["legs"].as_array().unwrap().len()), (1, 0));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn map_route_takes_photos_parameters_and_follows_the_filter() {
    let (mut s, dir) = open("route-params");
    let ids = on_a_trip(&mut s);
    // photos given out of order are put in time order, and one listed twice counts once
    let r = s.execute("map.route", &json!({"ids": [ids[3], ids[2], ids[2]]})).unwrap();
    assert_eq!((r["routes"].as_array().unwrap().len(), r["photos"].as_u64()), (1, Some(2)));
    assert_eq!(r["routes"][0]["stops"][0]["photos"], json!([ids[2]]));
    let gone = s.execute("map.route", &json!({"ids": [999_999_999u64]})).unwrap();
    assert_eq!((gone["skipped"]["unknown"].as_u64(), gone["routes"].as_array().map(Vec::len)), (Some(1), Some(0)));
    // a pause of an hour ends a route: the suburb and Paris are each alone
    let split = s.execute("map.route", &json!({"gapHours": 1})).unwrap();
    assert_eq!(split["routes"].as_array().unwrap().len(), 4);
    // a wide stop radius takes the suburb into the stop of the two photos
    let wide = s.execute("map.route", &json!({"stopRadius": 50_000})).unwrap();
    assert_eq!((wide["routes"][0]["stops"].as_array().unwrap().len(), wide["routes"][0]["stops"][0]["count"].as_u64()), (2, Some(3)));
    // a coarse line has three points
    let coarse = s.execute("map.route", &json!({"detail": 2})).unwrap();
    assert!(coarse["routes"][0]["legs"][1]["lines"][0].as_array().unwrap().len() <= 3);
    // the route follows the filter, as the markers do
    s.execute("photo.rate", &json!({"ids": [ids[3]], "rating": 5})).unwrap();
    s.execute("library.filter", &json!({"rating": 5})).unwrap();
    let one = s.execute("map.route", &json!({})).unwrap();
    assert_eq!(
        (one["photos"].as_u64(), one["routes"][0]["stops"].as_array().map(Vec::len), one["routes"][0]["legs"].as_array().map(Vec::len)),
        (Some(1), Some(1), Some(0))
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn map_route_rejects_nonsense() {
    let (mut s, dir) = open("route-bad");
    on_a_trip(&mut s);
    for p in [
        json!({"ids": "all"}),
        json!({"ids": [1, "x"]}),
        json!({"stopRadius": -1}),
        json!({"stopRadius": 1e9}),
        json!({"gapHours": 0}),
        json!({"arcKm": -5}),
        json!({"detail": 1}),
        json!({"detail": 257}),
        json!({"detail": u64::MAX}),
    ] {
        assert!(s.execute("map.route", &p).is_err(), "{p}");
    }
    assert!(s.execute("map.route", &json!({"ids": [], "detail": 256, "arcKm": 0, "stopRadius": 0, "gapHours": 0.5})).is_ok());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn route_fixes_read_the_capture_time_with_or_without_a_zone() {
    use lightcraft_catalog::{Op, PhotoId};
    let (mut s, dir) = open("route-fixes");
    let ids = geotag(&mut s);
    for (i, c) in [Some("2026-03-01T09:00:00+02:00"), Some("2026-03-01T09:00:00"), Some("2026-03-01"), None, Some("2026-03-01T20:00:00")]
        .into_iter()
        .enumerate()
    {
        s.catalog.apply(Op::SetCaptured { id: PhotoId(ids[i]), captured: c.map(str::to_string) }).unwrap();
    }
    let visible = s.visible_cloned();
    let found = crate::map::route_fixes(&s.catalog, &visible);
    // a date alone and no time at all can't place a photo
    assert_eq!((found.fixes.len(), found.no_time, found.zone_estimated), (3, 2, 2));
    let at = |i: usize| found.fixes.iter().find(|f| f.id == ids[i]).map(|f| f.time);
    let t = |iso: &str| lightcraft_meta::DateTime::parse_iso(iso).unwrap().unix_seconds() as f64;
    assert_eq!(at(0), Some(t("2026-03-01T07:00:00Z")), "a recorded offset is exact");
    assert_eq!(at(1), Some(t("2026-03-01T09:00:00Z")), "Madrid's longitude is zone 0");
    assert_eq!(at(4), Some(t("2026-03-01T10:00:00Z")), "Sydney's longitude is zone +10");
    let _ = std::fs::remove_dir_all(dir);
}
