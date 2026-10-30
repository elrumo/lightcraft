//! Recorded tracks through `Session::execute`: adding, importing, listing, reading, renaming and deleting them, and
//! the library's `tracks.json` (kept across a reopen, set aside when damaged).

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::Session;

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-tracks-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn open(dir: &Path) -> Session {
    let mut s = Session::new().with_fs();
    s.open_library(dir, false).unwrap();
    s
}

/// A walk of `n` points, 5 s and about 11 m apart, from `t0` (ms).
fn walk(n: usize, t0: i64) -> Vec<Value> {
    (0..n).map(|i| json!([t0 + i as i64 * 5000, 40.4168 + i as f64 * 1e-4, -3.7038, 650.0 + i as f64 * 0.1, 5.0])).collect()
}

const T0: i64 = 1_777_629_600_000;

const GPX: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<gpx version="1.1" creator="test" xmlns="http://www.topografix.com/GPX/1/1">
  <trk><name>Walk</name>
    <trkseg>
      <trkpt lat="46.0000" lon="7.0000"><ele>1600</ele><time>2026-05-01T10:00:00Z</time></trkpt>
      <trkpt lat="46.0010" lon="7.0020"><ele>1610</ele><time>2026-05-01T10:01:00Z</time></trkpt>
      <trkpt lat="46.0020" lon="7.0040"><time>2026-05-01T10:02:00.500Z</time></trkpt>
      <trkpt lat="46.5" lon="7.5"/>
    </trkseg>
    <trkseg>
      <trkpt lat="46.0030" lon="7.0050"><time>2026-05-01T10:30:00Z</time></trkpt>
      <trkpt lat="46.0040" lon="7.0060"><time>2026-05-01T10:31:00Z</time></trkpt>
    </trkseg>
  </trk>
  <trk><name>Later</name><trkseg>
      <trkpt lat="46.0050" lon="7.0070"><time>2026-05-01T11:00:00Z</time></trkpt>
      <trkpt lat="46.0050" lon="7.0070"><time>2026-05-01T11:00:00Z</time></trkpt>
  </trkseg></trk>
</gpx>"#;

#[test]
fn a_recording_is_added_once_and_read_back() {
    let mut s = Session::new();
    let mut points = walk(100, T0);
    points[50].as_array_mut().unwrap().push(json!(1));
    let r = s.execute("track.create", &json!({"points": points, "name": "  Morning walk ", "device": "iPhone", "tz": 120})).unwrap();
    let session = r["session"].as_str().unwrap().to_string();
    assert!(session.starts_with('s') && session.len() == 17, "{session}");
    assert_eq!((r["existing"].clone(), r["created"].as_array().unwrap().len()), (json!(false), 1));
    let chunk = &r["created"][0];
    assert_eq!(chunk["id"], format!("{session}-0"));
    assert!(chunk.get("data").is_none() && chunk["bytes"].as_u64().unwrap() > 100, "{chunk}");
    assert_eq!(
        (chunk["name"].clone(), chunk["source"].clone(), chunk["tz"].clone(), chunk["points"].clone()),
        (json!("Morning walk"), json!("phone"), json!(120), json!(100))
    );
    assert_eq!(r["dropped"], json!({"invalid": 0, "outOfOrder": 0, "duplicate": 0}));
    assert!(r["docBytes"].as_u64().unwrap() > chunk["bytes"].as_u64().unwrap() && r["docItems"] == 1, "{r}");
    // the same points again: one session
    let again = s.execute("track.create", &json!({"points": walk(100, T0)})).unwrap();
    assert_eq!((again["session"].as_str(), again["existing"].clone(), again["created"].clone()), (Some(session.as_str()), json!(true), json!([])));
    assert_eq!(again["docItems"], 1);
    // read back: points, segment break and both ends, at most maxPoints
    let got = s.execute("track.get", &json!({"session": session, "points": true})).unwrap();
    let pts = got["points"].as_array().unwrap();
    assert_eq!((pts.len(), got["simplified"].clone()), (100, json!(false)));
    assert_eq!(pts[0], json!([T0, 40.4168, -3.7038, 650.0, 5.0, 0]));
    assert_eq!(pts.iter().filter(|p| p[5] == 1).count(), 1);
    assert_eq!(pts[50][5], 1);
    let few = s.execute("track.get", &json!({"session": session, "points": true, "maxPoints": 10})).unwrap();
    let fp = few["points"].as_array().unwrap();
    assert!(fp.len() <= 10 && few["simplified"] == true);
    assert_eq!((fp[0][0].as_i64(), fp.last().unwrap()[0].as_i64()), (Some(T0), Some(T0 + 99 * 5000)));
    assert!(fp.iter().any(|p| p[5] == 1), "the break survives");
    let part = s.execute("track.get", &json!({"session": session, "points": true, "fromMs": T0 + 10 * 5000, "toMs": T0 + 19 * 5000})).unwrap();
    assert_eq!(part["points"].as_array().unwrap().len(), 10);
    let by_id = s.execute("track.get", &json!({"id": format!("{session}-0")})).unwrap();
    assert_eq!((by_id["session"].as_str(), by_id["chunks"].as_array().map(Vec::len)), (Some(session.as_str()), Some(1)));
    assert_eq!(got["name"], "Morning walk");
    assert_eq!(got["device"], "iPhone");
    // a packed track is a recording too
    let data = {
        use base64::Engine as _;
        let p: Vec<lightcraft_geo::track::Point> = (0..20)
            .map(|i| lightcraft_geo::track::Point {
                time_ms: T0 * 2 + i * 1000,
                lat: 1.0,
                lon: 2.0 + i as f64 * 1e-4,
                ele_m: None,
                acc_m: None,
                new_segment: false,
            })
            .collect();
        base64::engine::general_purpose::STANDARD.encode(lightcraft_geo::track::pack(&p).unwrap())
    };
    let packed = s.execute("track.create", &json!({"data": data, "source": "watch"})).unwrap();
    assert_eq!((packed["created"][0]["points"].clone(), packed["created"][0]["source"].clone()), (json!(20), json!("watch")));
    assert_eq!(s.execute("track.list", &json!({})).unwrap()["sessions"].as_array().unwrap().len(), 2);
}

#[test]
fn points_that_are_not_real_go_back_or_repeat_are_left_out() {
    let mut s = Session::new();
    let points = json!([
        [T0, 10.0, 10.0],
        [T0 + 1000, 10.0, 10.001, null, null, 0],
        "not a point",
        [T0 + 500, 10.0, 10.0],
        [T0 + 1000, 10.0, 10.0],
        [T0 + 2000, 91.0, 0.0],
        [T0 + 2000, "x", 0.0],
        [-5, 10.0, 10.0],
        [T0 + 3000, 10.0, 10.002, 1e9, -4.0, 1],
        [T0 + 4000, 10.0, 10.003],
    ]);
    let r = s.execute("track.create", &json!({"points": points})).unwrap();
    assert_eq!(r["dropped"], json!({"invalid": 4, "outOfOrder": 1, "duplicate": 1}));
    let got = s.execute("track.get", &json!({"session": r["session"], "points": true})).unwrap();
    assert_eq!(
        got["points"],
        json!([
            [T0, 10.0, 10.0, null, null, 0],
            [T0 + 1000, 10.0, 10.001, null, null, 0],
            [T0 + 3000, 10.0, 10.002, null, null, 1],
            [T0 + 4000, 10.0, 10.003, null, null, 0]
        ]),
        "an elevation or accuracy that isn't real leaves only itself out"
    );
}

#[test]
fn bad_parameters_are_errors_never_crashes() {
    let mut s = Session::new();
    let err = |s: &mut Session, p: Value| s.execute("track.create", &p).unwrap_err().to_string();
    assert!(err(&mut s, json!({})).contains("missing `points` or `data`"));
    assert!(err(&mut s, json!({"points": [], "data": "AAAA"})).contains("not both"));
    assert!(err(&mut s, json!({"points": []})).contains("no usable points"));
    assert!(err(&mut s, json!({"points": [[1, 2]], "name": 7})).contains("`name` is text"));
    assert!(err(&mut s, json!({"points": walk(3, T0), "name": "n".repeat(201)})).contains("at most 200"));
    assert!(err(&mut s, json!({"points": walk(3, T0), "source": "car"})).contains("`source` is one of"));
    assert!(err(&mut s, json!({"points": walk(3, T0), "tz": 5000})).contains("`tz`"));
    assert!(err(&mut s, json!({"points": "lots"})).contains("`points` is a list"));
    assert!(err(&mut s, json!({"data": 5})).contains("base64"));
    assert!(err(&mut s, json!({"data": "%%%%"})).contains("not base64"));
    assert!(err(&mut s, json!({"data": "AAAAAAAA"})).contains("track data"));
    {
        use base64::Engine as _;
        let bomb = miniz_oxide::deflate::compress_to_vec(&vec![0u8; lightcraft_geo::track::MAX_INFLATED + 1], 9);
        let e = err(&mut s, json!({"data": base64::engine::general_purpose::STANDARD.encode(bomb)}));
        assert!(e.contains("can't decompress"), "{e}");
    }
    let e = err(&mut s, json!({"points": vec![Value::Null; crate::tracks::SESSION_POINTS_MAX + 1]}));
    assert!(e.starts_with("limit: "), "{e}");
    for (cmd, p, want) in [
        ("track.get", json!({}), "missing `session`"),
        ("track.get", json!({"session": "s0000000000000000"}), "no such track: s0000000000000000"),
        ("track.get", json!({"id": "x/../y"}), "no such track: x/../y"),
        ("track.get", json!({"session": "a", "maxPoints": 1}), "maxPoints"),
        ("track.list", json!({"fromMs": "yesterday"}), "fromMs"),
        ("track.rename", json!({"session": "s0000000000000000", "name": "x"}), "no such track"),
        ("track.rename", json!({"session": "s0000000000000000"}), "missing `name`"),
        ("track.delete", json!({}), "missing `session`"),
        ("track.delete", json!({"session": "s0000000000000000"}), "no such track"),
        ("track.import", json!({}), "missing `path`"),
        ("track.import", json!({"gpx": "<kml/>"}), "not a GPX file"),
        ("track.import", json!({"path": "/nonexistent/x.gpx"}), "can't read"),
    ] {
        let e = s.execute(cmd, &p).unwrap_err().to_string();
        assert!(e.contains(want), "{cmd} {p}: {e}");
    }
    let e = s.execute("track.get", &json!({"session": "s".repeat(1 << 20)})).unwrap_err().to_string();
    assert!(e.len() < 100, "a huge id isn't echoed whole");
}

#[test]
fn a_gpx_file_is_one_session_and_importing_it_again_adds_nothing() {
    let root = temp_dir("gpx");
    let file = root.join("Alps day.gpx");
    std::fs::write(&file, GPX).unwrap();
    let mut s = open(&root.join("lib"));
    let r = s.execute("track.import", &json!({"path": file.to_string_lossy()})).unwrap();
    assert_eq!((r["tracks"].clone(), r["untimed"].clone(), r["existing"].clone()), (json!(2), json!(1), json!(false)));
    assert_eq!(r["dropped"]["duplicate"], 1, "two points at the same moment");
    let chunk = &r["created"][0];
    assert_eq!((chunk["name"].clone(), chunk["source"].clone(), chunk["points"].clone()), (json!("Alps day"), json!("import"), json!(6)));
    let got = s.execute("track.get", &json!({"session": r["session"], "points": true})).unwrap();
    let segs: Vec<i64> = got["points"].as_array().unwrap().iter().map(|p| p[5].as_i64().unwrap()).collect();
    assert_eq!(segs, vec![0, 0, 0, 1, 0, 1], "each segment and each track begins a stretch");
    assert_eq!(got["points"][2][0], 1_777_629_720_500i64, "sub-second times kept to the millisecond");
    let again = s.execute("track.import", &json!({"gpx": GPX, "name": "Other name"})).unwrap();
    assert_eq!((again["session"].clone(), again["existing"].clone(), again["docItems"].clone()), (r["session"].clone(), json!(true), json!(1)));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_huge_gpx_file_is_refused_not_stored() {
    let pt = "<trkpt lat=\"1\" lon=\"2\"><time>2026-01-01T00:00:01Z</time></trkpt>";
    let gpx = format!("<gpx><trk><trkseg>{}</trkseg></trk></gpx>", pt.repeat(crate::tracks::SESSION_POINTS_MAX + 1));
    let mut s = Session::new();
    let e = s.execute("track.import", &json!({"gpx": gpx})).unwrap_err().to_string();
    assert!(e.starts_with("limit: a track holds at most 2000000 points"), "{e}");
    assert_eq!(s.tracks.size().1, 0);
}

#[test]
fn listing_renaming_and_deleting() {
    let mut s = Session::new();
    let a = s.execute("track.create", &json!({"points": walk(10, T0), "name": "A"})).unwrap()["session"].clone();
    let b = s.execute("track.create", &json!({"points": walk(10, T0 + 86_400_000)})).unwrap()["session"].clone();
    let list = s.execute("track.list", &json!({})).unwrap();
    assert_eq!(list["sessions"].as_array().unwrap().iter().map(|v| v["session"].clone()).collect::<Vec<_>>(), vec![a.clone(), b.clone()], "by start");
    assert_eq!(list["limits"], json!({"bytes": 16 << 20, "items": 1500}));
    assert!(list.get("chunks").is_none());
    let first = &list["sessions"][0];
    assert_eq!(
        (first["points"].clone(), first["chunks"].clone(), first["unreadable"].clone(), first["source"].clone()),
        (json!(10), json!(1), json!(false), json!("phone"))
    );
    assert!(first["distanceKm"].as_f64().unwrap() > 0.09 && first["bbox"]["south"] == 40.4168, "{first}");
    let day2 = s.execute("track.list", &json!({"fromMs": T0 + 3_600_000, "chunks": true})).unwrap();
    assert_eq!(day2["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(day2["chunks"].as_array().unwrap().len(), 1);
    assert_eq!(s.execute("track.list", &json!({"toMs": T0 + 45_000})).unwrap()["sessions"].as_array().unwrap().len(), 1, "the end counts");
    assert_eq!(s.execute("track.list", &json!({"toMs": T0 - 1})).unwrap()["sessions"], json!([]));
    // rename, clear
    assert_eq!(s.execute("track.rename", &json!({"session": b, "name": " Day two "})).unwrap(), json!({"session": b, "name": "Day two"}));
    assert_eq!(s.execute("track.get", &json!({"session": b})).unwrap()["name"], "Day two");
    s.execute("track.rename", &json!({"session": a, "name": ""})).unwrap();
    assert!(s.execute("track.get", &json!({"session": a})).unwrap().get("name").is_none());
    assert!(s.execute("track.rename", &json!({"session": a, "name": ["x"]})).is_err());
    // delete
    assert_eq!(s.execute("track.delete", &json!({"session": a})).unwrap(), json!({"removed": 1}));
    assert_eq!(s.execute("track.list", &json!({})).unwrap()["docItems"], 1);
    // none of it is an undo step, or in the command journal
    assert!(s.undo.is_empty());
    assert!(s.journal.iter().all(|(id, _)| !id.starts_with("track.")), "{:?}", s.journal);
}

#[test]
fn tracks_stay_with_the_library_and_each_change_is_noticed() {
    let root = temp_dir("keep");
    let mut s = open(&root);
    let gen0 = s.library.as_ref().unwrap().prefs_gen();
    let r = s.execute("track.create", &json!({"points": walk(30, T0), "name": "Kept"})).unwrap();
    let gen1 = s.library.as_ref().unwrap().prefs_gen();
    assert!(gen1 > gen0, "the sync looks at this");
    s.execute("track.rename", &json!({"session": r["session"], "name": "Kept too"})).unwrap();
    assert!(s.library.as_ref().unwrap().prefs_gen() > gen1);
    let file: Value = serde_json::from_slice(&std::fs::read(root.join("tracks.json")).unwrap()).unwrap();
    assert_eq!(file.as_array().unwrap().len(), 1);
    s.close_library().unwrap();
    drop(s);
    let mut s = open(&root);
    let got = s.execute("track.get", &json!({"session": r["session"], "points": true})).unwrap();
    assert_eq!((got["name"].clone(), got["points"].as_array().unwrap().len()), (json!("Kept too"), 30));
    assert!(s.take_library_warnings().is_empty());
    let _ = std::fs::remove_dir_all(&root);
}

fn corrupt_copies(root: &Path) -> usize {
    std::fs::read_dir(root).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with("tracks.json.corrupt-")).count()
}

#[test]
fn a_damaged_tracks_file_is_set_aside_and_the_library_opens() {
    for (i, bad) in [&b"[{\"id\": \"s0"[..], b"not json at all", b"{\"items\": []}", b"\x00\xff\xfe"].iter().enumerate() {
        let root = temp_dir(&format!("damaged-{i}"));
        std::fs::write(root.join("tracks.json"), bad).unwrap();
        let mut s = open(&root);
        let warnings = s.take_library_warnings();
        assert!(warnings.iter().any(|w| w.starts_with("tracks.json is damaged")), "{warnings:?}");
        assert_eq!(corrupt_copies(&root), 1);
        assert_eq!(s.execute("track.list", &json!({})).unwrap()["docItems"], 0);
        // a new track can be saved: the damaged file is kept aside
        s.execute("track.create", &json!({"points": walk(3, T0)})).unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[test]
fn damaged_pieces_are_left_out_and_the_rest_kept() {
    let root = temp_dir("pieces");
    let mut s = open(&root);
    let good = s.execute("track.create", &json!({"points": walk(10, T0)})).unwrap()["session"].clone();
    s.close_library().unwrap();
    drop(s);
    let mut items: Vec<Value> = serde_json::from_slice(&std::fs::read(root.join("tracks.json")).unwrap()).unwrap();
    let mut liar = items[0].clone();
    liar["id"] = json!("s1111111111111111-0");
    liar["session"] = json!("s1111111111111111");
    liar["points"] = json!(11);
    let mut newer = items[0].clone();
    newer["id"] = json!("s2222222222222222-0");
    newer["session"] = json!("s2222222222222222");
    newer["fmt"] = json!(2);
    newer["data"] = json!("whatever a later version packs");
    items.extend([liar, newer, json!({"id": "../escape"})]);
    std::fs::write(root.join("tracks.json"), serde_json::to_vec(&items).unwrap()).unwrap();
    let mut s = open(&root);
    assert!(s.take_library_warnings().iter().any(|w| w.contains("2 of its pieces can't be read")));
    assert_eq!(corrupt_copies(&root), 1, "the file as it was is kept");
    let list = s.execute("track.list", &json!({})).unwrap();
    let sessions = list["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 2, "{list}");
    assert_eq!(sessions.iter().find(|v| v["session"] == good).unwrap()["unreadable"], false);
    let newer = sessions.iter().find(|v| v["session"] == "s2222222222222222").unwrap();
    assert_eq!(newer["unreadable"], true, "kept, not read");
    let got = s.execute("track.get", &json!({"session": "s2222222222222222", "points": true})).unwrap();
    assert_eq!(got["points"], json!([]));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn tracking_settings_are_read_set_kept_and_checked() {
    let root = temp_dir("settings");
    let mut s = open(&root);
    assert_eq!(s.execute("tracking.settings", &json!({})).unwrap(), json!({"trim": false, "trimRadiusM": 300.0}));
    assert!(s.doc_items("tracks").is_empty(), "not sent until set");
    let gen0 = s.library.as_ref().unwrap().prefs_gen();
    assert_eq!(s.execute("tracking.settings", &json!({"trim": true})).unwrap(), json!({"trim": true, "trimRadiusM": 300.0}));
    assert!(s.library.as_ref().unwrap().prefs_gen() > gen0, "the sync looks at this");
    assert_eq!(s.execute("tracking.settings", &json!({"trimRadiusM": 750})).unwrap(), json!({"trim": true, "trimRadiusM": 750.0}));
    for (p, want) in [
        (json!({"trim": "yes"}), "`trim`"),
        (json!({"trimRadiusM": 49}), "trimRadiusM"),
        (json!({"trimRadiusM": 5001}), "trimRadiusM"),
        (json!({"trimRadiusM": "far"}), "trimRadiusM"),
        (json!({"trim": false, "trimRadiusM": 10}), "trimRadiusM"),
    ] {
        let e = s.execute("tracking.settings", &p).unwrap_err().to_string();
        assert!(e.contains(want), "{p}: {e}");
    }
    assert_eq!(s.execute("tracking.settings", &json!({})).unwrap(), json!({"trim": true, "trimRadiusM": 750.0}), "a refused change changes nothing");
    // one item of the tracks document, beside the tracks and not one of them
    s.execute("track.create", &json!({"points": walk(5, T0)})).unwrap();
    assert!(s.doc_items("tracks").contains(&json!({"id": "settings", "trim": true, "trimRadiusM": 750.0})));
    assert!(s.doc_items("prefs").iter().all(|v| v["id"] != "settings" && v["id"] != "tracking"), "nothing in `prefs`");
    let list = s.execute("track.list", &json!({})).unwrap();
    assert_eq!((list["sessions"].as_array().unwrap().len(), list["docItems"].clone()), (1, json!(2)), "listed: the track; counted: both");
    let track = s.doc_items("tracks").into_iter().find(|v| v["id"] != "settings").unwrap();
    // from the other devices: theirs, fields this build doesn't know kept; this one's when theirs isn't one; none: the defaults
    s.set_doc_items("tracks", &[track.clone(), json!({"id": "settings", "trim": false, "trimRadiusM": 120, "later": [1]})]).unwrap();
    assert_eq!(s.execute("tracking.settings", &json!({})).unwrap(), json!({"trim": false, "trimRadiusM": 120.0}));
    s.execute("tracking.settings", &json!({"trim": true})).unwrap();
    let item = s.doc_items("tracks").into_iter().find(|v| v["id"] == "settings").unwrap();
    assert_eq!(item, json!({"id": "settings", "trim": true, "trimRadiusM": 120.0, "later": [1]}), "changed in place");
    for bad in [
        json!({"id": "settings", "trim": "on", "trimRadiusM": 300}),
        json!({"id": "settings", "trim": false, "trimRadiusM": f64::MAX}),
        json!({"id": "settings"}),
    ] {
        s.set_doc_items("tracks", &[track.clone(), bad.clone()]).unwrap();
        assert_eq!(s.execute("tracking.settings", &json!({})).unwrap(), json!({"trim": true, "trimRadiusM": 120.0}), "{bad}");
    }
    // kept across a reopen
    s.close_library().unwrap();
    drop(s);
    let mut s = open(&root);
    assert_eq!(s.execute("tracking.settings", &json!({})).unwrap(), json!({"trim": true, "trimRadiusM": 120.0}));
    assert!(s.take_library_warnings().is_empty());
    s.set_doc_items("tracks", std::slice::from_ref(&track)).unwrap();
    assert_eq!(s.execute("tracking.settings", &json!({})).unwrap(), json!({"trim": false, "trimRadiusM": 300.0}), "none: the defaults");
    let _ = std::fs::remove_dir_all(&root);
}

/// What a newer build put in the shared `prefs` document stays, and goes on to the others.
#[test]
fn prefs_items_this_build_does_not_know_are_kept() {
    let root = temp_dir("prefs-kept");
    let mut s = open(&root);
    let import = s.doc_items("prefs")[0].clone();
    assert_eq!(import["id"], "import");
    let items = [
        import.clone(),
        json!({"id": "future", "x": [1]}),
        json!({"no": "id"}),
        json!({"id": "import", "copy": 1}),
        json!({"id": "big", "x": "y".repeat(100_000)}),
    ];
    s.set_doc_items("prefs", &items).unwrap();
    s.close_library().unwrap();
    drop(s);
    let mut s = open(&root);
    let ids: Vec<String> = s.doc_items("prefs").iter().map(|v| v["id"].as_str().unwrap_or("?").to_string()).collect();
    assert_eq!(ids, ["import", "future"]);
    s.set_doc_items("prefs", &[import]).unwrap();
    assert_eq!(s.doc_items("prefs").len(), 1, "gone when the others drop it");
    let _ = std::fs::remove_dir_all(&root);
}
