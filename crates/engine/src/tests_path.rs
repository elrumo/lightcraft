//! Paths from the stored tracks through `Session::execute`: `photo.autoTagTracks` (the same answers as
//! `photo.autoTagTracklog` on the same track).

use serde_json::{Value, json};

use lightcraft_catalog::{Op, PhotoId};

use crate::Session;

/// A demo library of a few photos.
fn library(tag: &str) -> (Session, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("lc-path-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut s = Session::new();
    s.open_library(&dir, true).unwrap();
    (s, dir)
}

fn photos(s: &mut Session, n: usize) -> Vec<u64> {
    let ids: Vec<u64> = s.visible_cloned().iter().take(n).map(|p| p.0).collect();
    assert_eq!(ids.len(), n, "the demo library has photos");
    // none has a position to begin with
    s.execute("photo.setMeta", &json!({"ids": ids, "gps": null})).unwrap();
    ids
}

fn set_time(s: &mut Session, id: u64, t: &str) {
    s.commit("t", Op::SetCaptured { id: PhotoId(id), captured: Some(t.into()) }).unwrap();
}

fn gps(s: &Session, id: u64) -> Option<(f64, f64)> {
    s.catalog.photo(PhotoId(id)).unwrap().meta.gps
}

/// Two stretches of a walk (positions to the millionth of a degree, as stored), then a second track an hour later.
const GPX: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<gpx version="1.1" creator="test" xmlns="http://www.topografix.com/GPX/1/1">
  <trk><trkseg>
      <trkpt lat="46.000000" lon="7.000000"><time>2026-05-01T10:00:00Z</time></trkpt>
      <trkpt lat="46.001000" lon="7.002000"><time>2026-05-01T10:01:00Z</time></trkpt>
      <trkpt lat="46.002000" lon="7.004000"><time>2026-05-01T10:02:00.500Z</time></trkpt>
    </trkseg><trkseg>
      <trkpt lat="46.003000" lon="7.005000"><time>2026-05-01T10:30:00Z</time></trkpt>
      <trkpt lat="46.004000" lon="7.006000"><time>2026-05-01T10:31:00Z</time></trkpt>
  </trkseg></trk>
</gpx>"#;

/// What two auto-tag answers share (the tracks' answer also names its track per photo).
fn common(r: &Value) -> Value {
    let photos: Vec<Value> = r["photos"].as_array().unwrap().iter().map(|p| json!({"id": p["id"], "gps": p["gps"], "match": p["match"]})).collect();
    json!({"tagged": r["tagged"], "interpolated": r["interpolated"], "nearest": r["nearest"], "skipped": r["skipped"], "assumedUtc": r["assumedUtc"], "photos": photos})
}

#[test]
fn auto_tag_tracks_answers_as_the_tracklog_does() {
    let (mut s, dir) = library("parity");
    let ids = photos(&mut s, 6);
    set_time(&mut s, ids[0], "2026-05-01T12:00:30"); // no zone: +02:00 given as `offset`
    set_time(&mut s, ids[1], "2026-05-01T12:01:00+02:00"); // on the second point
    set_time(&mut s, ids[2], "2026-05-01T10:16:00+00:00"); // between the stretches: never interpolated
    set_time(&mut s, ids[3], "2026-05-01T10:30:30+00:00"); // inside the second stretch
    set_time(&mut s, ids[4], "2026-05-01T15:00:00+00:00"); // hours after
    set_time(&mut s, ids[5], "2026-05-01T10:01:30+00:00");
    s.execute("photo.setMeta", &json!({"ids": [ids[5]], "gps": [10.0, 20.0]})).unwrap();
    let r = s.execute("track.import", &json!({"gpx": GPX})).unwrap();
    let session = r["session"].as_str().unwrap().to_string();
    for params in [
        json!({"ids": ids, "offset": "+02:00"}),
        json!({"ids": ids, "offset": 2, "maxGap": 1200}),
        json!({"ids": ids, "replace": true, "maxGap": 3600}),
        json!({"ids": ids, "maxGap": 0}),
    ] {
        let mut p = params.clone();
        p["dryRun"] = json!(true);
        let mut log = p.clone();
        log["gpx"] = json!(GPX);
        let a = s.execute("photo.autoTagTracklog", &log).unwrap();
        let b = s.execute("photo.autoTagTracks", &p).unwrap();
        assert_eq!(common(&a), common(&b), "{params}");
        assert!(b["photos"].as_array().unwrap().iter().all(|p| p["track"] == session.as_str()));
        assert_eq!((b["tracks"].as_u64(), b["points"].as_u64()), (Some(1), Some(5)));
        assert_eq!((b["start"].as_f64(), b["startIso"].as_str()), (Some(1_777_629_600.0), Some("2026-05-01T10:00:00Z")));
    }
    let r = s.execute("photo.autoTagTracks", &json!({"ids": ids, "offset": "+02:00", "maxGap": 3600, "dryRun": true})).unwrap();
    let matched: Vec<(u64, &str)> =
        r["photos"].as_array().unwrap().iter().map(|p| (p["id"].as_u64().unwrap(), p["match"].as_str().unwrap())).collect();
    assert!(matched.contains(&(ids[2], "nearest")), "a break is never interpolated across: {matched:?}");
    assert!(matched.contains(&(ids[3], "interpolated")));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn auto_tag_tracks_is_one_undo_step_and_a_dry_run_changes_nothing() {
    let (mut s, dir) = library("undo");
    let ids = photos(&mut s, 3);
    set_time(&mut s, ids[0], "2026-05-01T10:00:30+00:00");
    set_time(&mut s, ids[1], "2026-05-01T12:30:30+02:00");
    set_time(&mut s, ids[2], "2026-05-02T10:00:00+00:00");
    s.execute("track.import", &json!({"gpx": GPX})).unwrap();
    let before: Vec<_> = ids.iter().map(|i| gps(&s, *i)).collect();
    let undo = s.undo.len();
    let r = s.execute("photo.autoTagTracks", &json!({"ids": ids, "dryRun": true})).unwrap();
    assert_eq!((r["tagged"].as_u64(), r["skipped"]["outside"].as_u64()), (Some(2), Some(1)), "{r}");
    assert_eq!((ids.iter().map(|i| gps(&s, *i)).collect::<Vec<_>>(), s.undo.len()), (before.clone(), undo));
    s.execute("photo.autoTagTracks", &json!({"ids": ids})).unwrap();
    assert_eq!(s.undo.len(), undo + 1);
    assert_eq!(gps(&s, ids[0]), Some((46.0005, 7.001)));
    assert_eq!(gps(&s, ids[1]), Some((46.0035, 7.0055)));
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(ids.iter().map(|i| gps(&s, *i)).collect::<Vec<_>>(), before);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn auto_tag_tracks_never_joins_two_sessions_and_names_its_errors() {
    let (mut s, dir) = library("sessions");
    let ids = photos(&mut s, 1);
    let e = s.execute("photo.autoTagTracks", &json!({"ids": ids})).unwrap_err().to_string();
    assert!(e.contains("no such track: none stored"), "{e}");
    // two recordings back to back: a photo in the half minute between them is near one, not between both
    let t0 = 1_777_629_600_000i64;
    let a = s.execute("track.create", &json!({"points": [[t0, 46.0, 7.0], [t0 + 60_000, 46.001, 7.0]]})).unwrap();
    let b = s.execute("track.create", &json!({"points": [[t0 + 90_000, 46.002, 7.0], [t0 + 150_000, 46.003, 7.0]]})).unwrap();
    set_time(&mut s, ids[0], "2026-05-01T10:01:10+00:00");
    let r = s.execute("photo.autoTagTracks", &json!({"ids": ids, "dryRun": true})).unwrap();
    assert_eq!(r["photos"][0]["match"], "nearest");
    assert_eq!(r["photos"][0]["track"], a["session"]);
    assert_eq!(r["tracks"], 2);
    // only the sessions named
    let r = s.execute("photo.autoTagTracks", &json!({"ids": ids, "tracks": [b["session"]], "dryRun": true})).unwrap();
    assert_eq!((r["photos"][0]["track"].clone(), r["tracks"].as_u64()), (b["session"].clone(), Some(1)));
    let e = s.execute("photo.autoTagTracks", &json!({"ids": ids, "tracks": ["s0000000000000000"]})).unwrap_err().to_string();
    assert!(e.contains("no such track: s0000000000000000"), "{e}");
    for bad in [json!({"tracks": []}), json!({"tracks": "all"}), json!({"tracks": [1]}), json!({"offset": "noon"}), json!({"maxGap": -1})] {
        let mut p = bad.clone();
        p["ids"] = json!(ids);
        assert!(s.execute("photo.autoTagTracks", &p).is_err(), "{bad}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
