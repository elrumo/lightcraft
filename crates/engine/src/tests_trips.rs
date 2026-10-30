//! Trips and their notes through `Session::execute` (`trip.*`, `album.ref`, `album.resolve`, `album.trips`), and the
//! library's `trips.json` and `trip-notes.json`. Sync through a server, iCloud and a cloud folder: `tests_cloud_sync`
//! and `tests_icloud`.

use std::path::{Path, PathBuf};

use lightcraft_catalog::{Album, AlbumId, Op, Photo, PhotoId, Source};
use serde_json::{Value, json};

use crate::Session;

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-trips-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn open(dir: &Path) -> Session {
    let mut s = Session::new().with_fs();
    s.open_library(dir, false).unwrap();
    s
}

/// A photo with content hash `n`, taken at `captured`, at `gps`.
fn photo(s: &mut Session, n: u64, captured: Option<&str>, gps: Option<(f64, f64)>) -> PhotoId {
    let id = s.catalog.alloc_photo_id();
    let mut p = Photo::new(id, Source::Demo { scene: 0 }, &format!("p{n}.jpg"), "JPEG", 10, 10, "2026-01-01");
    p.captured = captured.map(str::to_string);
    p.meta.gps = gps;
    p.content_hash = Some(format!("{n:032x}"));
    s.apply_system(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    id
}

fn album(s: &mut Session, name: &str, photos: &[PhotoId]) -> AlbumId {
    let id = s.catalog.alloc_album_id();
    let mut a = Album::new(id, name);
    a.photos = photos.to_vec();
    s.apply_system(Op::AddAlbum { album: a }).unwrap();
    id
}

fn ms(iso: &str) -> i64 {
    lightcraft_meta::DateTime::parse_iso(iso).unwrap().unix_seconds() * 1000
}

fn ids(v: &Value) -> Vec<u64> {
    v.as_array().unwrap().iter().map(|x| x.as_u64().unwrap()).collect()
}

fn err(s: &mut Session, id: &str, p: Value) -> String {
    s.execute(id, &p).unwrap_err().to_string()
}

/// A week in the Alps: two photos in an album, others around it.
struct Alps {
    s: Session,
    root: PathBuf,
    alps: AlbumId,
    p: Vec<PhotoId>,
}

fn alps(tag: &str) -> Alps {
    let root = temp_dir(tag);
    let mut s = open(&root);
    let at = Some((46.0, 7.5)); // (zone +1 h by longitude, when a time has none)
    let p = vec![
        photo(&mut s, 1, Some("2026-07-02T10:00:00+02:00"), at),
        photo(&mut s, 2, Some("2026-07-01T09:00:00+02:00"), None),
        // located, inside the week: by the range
        photo(&mut s, 3, Some("2026-07-03T12:00:00"), at),
        // located, after it
        photo(&mut s, 4, Some("2026-07-20T12:00:00Z"), at),
        // inside it without a position: not by the range
        photo(&mut s, 5, Some("2026-07-03T12:00:00Z"), None),
        // inside it and located, but deleted; another hidden
        photo(&mut s, 6, Some("2026-07-03T13:00:00Z"), at),
        photo(&mut s, 7, Some("2026-07-03T14:00:00Z"), at),
        // no time at all
        photo(&mut s, 8, None, at),
    ];
    s.apply_system(Op::SetDeleted { id: p[5], deleted: true }).unwrap();
    s.apply_system(Op::SetHidden { id: p[6], hidden: true }).unwrap();
    let alps = album(&mut s, "Alps", &[p[0], p[1], p[7]]);
    Alps { s, root, alps, p }
}

fn week() -> Value {
    json!({"fromMs": ms("2026-07-01T00:00:00Z"), "toMs": ms("2026-07-08T00:00:00Z")})
}

#[test]
fn a_trip_is_its_albums_and_the_located_photos_in_its_range() {
    let Alps { mut s, root, alps, p } = alps("photos");
    let r = s
        .execute("trip.create", &json!({"name": "  Alps 2026 ", "albums": [alps.0, alps.0], "range": week(), "photosInRange": true, "cover": p[1].0}))
        .unwrap();
    let trip = &r["trip"];
    let id = trip["id"].as_str().unwrap().to_string();
    assert!(id.starts_with('t') && id.len() == 17, "{id}");
    assert_eq!(trip["name"], "Alps 2026");
    assert_eq!(trip["albums"], json!([format!("a:{}", alps.0)]), "once, by id in a library that syncs with no one");
    assert_eq!((trip["tracks"].clone(), trip["overrides"].clone(), trip["photosInRange"].clone()), (json!("range"), json!([]), json!(true)));
    assert_eq!(trip["cover"], json!({"key": format!("{:032x}", 2)}), "a cover is named by its content");
    assert!(trip["createdMs"].as_i64().unwrap() > ms("2026-01-01T00:00:00Z"));
    assert_eq!(r["linked"], json!([{"ref": format!("a:{}", alps.0), "album": {"id": alps.0, "name": "Alps"}}]));
    assert_eq!((r["coverPhoto"].as_u64(), r["photos"].as_u64(), r["notes"].as_u64()), (Some(p[1].0), Some(4), Some(0)));
    // the album's photos and the one in the range, by time (the one without a time last)
    let got = s.execute("trip.photos", &json!({"id": id})).unwrap();
    assert_eq!(ids(&got["ids"]), vec![p[1].0, p[0].0, p[2].0, p[7].0]);
    assert_eq!((got["fromAlbums"].as_u64(), got["fromRange"].as_u64()), (Some(3), Some(1)));
    assert_eq!(s.trip_photos(&id).unwrap(), vec![p[1], p[0], p[2], p[7]]);
    assert_eq!(s.trip_range(&id).unwrap(), Some((ms("2026-07-01T00:00:00Z"), ms("2026-07-08T00:00:00Z"))));
    // a range holds both its ends
    let at = ms("2026-07-03T11:00:00Z");
    let edge = s.execute("trip.create", &json!({"name": "Edge", "range": {"fromMs": at, "toMs": at}, "photosInRange": true})).unwrap();
    assert_eq!(ids(&s.execute("trip.photos", &json!({"id": edge["trip"]["id"]})).unwrap()["ids"]), vec![p[2].0]);
    s.execute("trip.delete", &json!({"id": edge["trip"]["id"]})).unwrap();
    // without the range's photos
    s.execute("trip.update", &json!({"id": id, "photosInRange": false})).unwrap();
    assert_eq!(ids(&s.execute("trip.photos", &json!({"id": id})).unwrap()["ids"]), vec![p[1].0, p[0].0, p[7].0]);
    // a member deleted later leaves it; the album's own change reaches the trip with nothing copied
    s.apply_system(Op::SetDeleted { id: p[7], deleted: true }).unwrap();
    s.apply_system(Op::SetAlbumPhotos { id: alps, photos: vec![p[0], p[1], p[3], p[7]] }).unwrap();
    assert_eq!(ids(&s.execute("trip.photos", &json!({"id": id})).unwrap()["ids"]), vec![p[1].0, p[0].0, p[3].0]);
    // the album knows its trip; the reference reads back
    assert_eq!(s.execute("album.trips", &json!({"id": alps.0})).unwrap(), json!({"trips": [{"id": id, "name": "Alps 2026"}]}));
    let other = album(&mut s, "Other", &[]);
    assert_eq!(s.execute("album.trips", &json!({"id": other.0})).unwrap(), json!({"trips": []}));
    let alps_ref = s.execute("album.ref", &json!({"id": alps.0})).unwrap()["ref"].clone();
    assert_eq!(s.execute("album.resolve", &json!({"ref": alps_ref})).unwrap(), json!({"id": alps.0, "name": "Alps"}));
    for gone in ["a:999", "u:0123456789abcdef", "a:", "a:x", "nothing", ""] {
        assert_eq!(s.execute("album.resolve", &json!({"ref": gone})).unwrap(), json!({"id": null}), "{gone}");
    }
    // the list
    let list = s.execute("trip.list", &json!({})).unwrap();
    let row = &list["trips"][0];
    assert_eq!(
        (row["id"].as_str(), row["photos"].as_u64(), row["albums"].as_u64(), row["cover"].as_u64()),
        (Some(id.as_str()), Some(3), Some(1), Some(p[1].0))
    );
    assert_eq!((row["startMs"].as_i64(), row["endMs"].as_i64()), (Some(ms("2026-07-01T00:00:00Z")), Some(ms("2026-07-08T00:00:00Z"))));
    assert_eq!(row["range"], week());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn folders_smart_albums_and_the_quick_collection_are_not_linked() {
    let Alps { mut s, root, alps, .. } = alps("refused");
    let folder = s.catalog.alloc_album_id();
    s.apply_system(Op::AddAlbum { album: Album { folder: true, ..Album::new(folder, "Travel") } }).unwrap();
    let smart = s.execute("album.createSmart", &json!({"name": "Five stars", "rules": {"rating": 5}})).unwrap()["id"].as_u64().unwrap();
    s.execute("album.setTarget", &json!({"id": null})).unwrap();
    let quick = s.catalog.quick_collection().map(|q| q.0);
    for (id, why) in [(folder.0, "folder"), (smart, "smart album")].into_iter().chain(quick.map(|q| (q, "Quick Collection"))) {
        let e = err(&mut s, "album.ref", json!({"id": id}));
        assert!(e.contains(why) && e.contains("can't be linked"), "{e}");
        assert!(err(&mut s, "trip.create", json!({"name": "x", "albums": [id]})).contains(why));
    }
    assert!(err(&mut s, "album.ref", json!({"id": 9999})).contains("no such album: 9999"));
    let t = s.execute("trip.create", &json!({"name": "x", "albums": [alps.0]})).unwrap()["trip"]["id"].clone();
    assert!(err(&mut s, "trip.link", json!({"id": t, "album": folder.0})).contains("folder"));
    assert!(err(&mut s, "trip.fromAlbum", json!({"album": smart})).contains("smart album"));
    assert_eq!(s.execute("trip.list", &json!({})).unwrap()["trips"].as_array().unwrap().len(), 1, "nothing made by a refusal");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_trip_is_edited_linked_and_unlinked() {
    let Alps { mut s, root, alps, p } = alps("update");
    let id = s.execute("trip.create", &json!({"name": "Alps"})).unwrap()["trip"]["id"].as_str().unwrap().to_string();
    let other = album(&mut s, "Lakes", &[p[3]]);
    // link: once, whatever the number of times
    for _ in 0..2 {
        s.execute("trip.link", &json!({"id": id, "album": alps.0})).unwrap();
    }
    let r = s.execute("trip.link", &json!({"id": id, "album": other.0})).unwrap();
    assert_eq!(r["trip"]["albums"], json!([format!("a:{}", alps.0), format!("a:{}", other.0)]));
    // a linked album that is deleted stays linked (it resolves to nothing) and can be unlinked by its reference
    s.apply_system(Op::RemoveAlbum { id: other }).unwrap();
    let r = s.execute("trip.get", &json!({"id": id})).unwrap();
    assert_eq!(r["linked"][1], json!({"ref": format!("a:{}", other.0), "album": null}));
    s.execute("trip.unlink", &json!({"id": id, "ref": format!("a:{}", other.0)})).unwrap();
    let r = s.execute("trip.unlink", &json!({"id": id, "album": alps.0})).unwrap();
    assert_eq!(r["trip"]["albums"], json!([]));
    // each field; null clears
    let set = json!({"id": id, "name": "Alps again", "albums": [alps.0], "range": week(), "photosInRange": true,
                     "tracks": ["s0123456789abcdef", "s0123456789abcdef"], "overrides": [{"atMs": 5, "mode": "flight"}], "cover": p[0].0});
    let t = s.execute("trip.update", &set).unwrap()["trip"].clone();
    assert_eq!(
        (t["name"].clone(), t["albums"].clone(), t["range"].clone(), t["photosInRange"].clone(), t["tracks"].clone(), t["overrides"].clone()),
        (
            json!("Alps again"),
            json!([format!("a:{}", alps.0)]),
            week(),
            json!(true),
            json!(["s0123456789abcdef"]),
            json!([{"atMs": 5, "mode": "flight"}])
        )
    );
    assert_eq!(s.trip_overrides(&id).unwrap(), vec![(5, "flight".to_string())]);
    let gen0 = s.library.as_ref().unwrap().prefs_gen();
    let cleared = json!({"id": id, "albums": null, "range": null, "photosInRange": null, "tracks": null, "overrides": null, "cover": null});
    let t = s.execute("trip.update", &cleared).unwrap()["trip"].clone();
    assert!(s.library.as_ref().unwrap().prefs_gen() > gen0, "the sync looks at this");
    assert_eq!(
        (t["albums"].clone(), t["tracks"].clone(), t["overrides"].clone(), t["photosInRange"].clone()),
        (json!([]), json!("range"), json!([]), json!(false))
    );
    assert!(t.get("range").is_none() && t.get("cover").is_none(), "{t}");
    assert_eq!(t["name"], "Alps again", "what isn't given stays");
    // the same again changes nothing, not even the generation
    let gen1 = s.library.as_ref().unwrap().prefs_gen();
    s.execute("trip.update", &json!({"id": id, "name": "Alps again"})).unwrap();
    assert_eq!(s.library.as_ref().unwrap().prefs_gen(), gen1);
    // refusals change nothing
    for (params, want) in [
        (json!({"id": id, "name": null}), "can't be cleared"),
        (json!({"id": id, "name": "  "}), "can't be empty"),
        (json!({"id": id, "name": "x".repeat(201)}), "200"),
        (json!({"id": id, "name": 5}), "`name` is text"),
        (json!({"id": id, "range": {"fromMs": 9, "toMs": 1}}), "fromMs not after toMs"),
        (json!({"id": id, "range": {"fromMs": "a", "toMs": 1}}), "range"),
        (json!({"id": id, "photosInRange": "yes"}), "photosInRange"),
        (json!({"id": id, "tracks": "all"}), "tracks"),
        (json!({"id": id, "tracks": ["../x"]}), "tracks"),
        (json!({"id": id, "overrides": [{"atMs": 1, "mode": "boat"}]}), "walk, bike, road, flight"),
        (json!({"id": id, "overrides": {"atMs": 1}}), "overrides"),
        (json!({"id": id, "cover": 99999}), "no such photo"),
        (json!({"id": id, "cover": "x"}), "photo id"),
        (json!({"id": id, "albums": [alps.0, 99999]}), "no such album"),
        (json!({"id": id, "albums": "x"}), "album ids"),
        (json!({"id": "t-nope"}), "no such trip: t-nope"),
        (json!({}), "missing `id`"),
    ] {
        let e = err(&mut s, "trip.update", params.clone());
        assert!(e.contains(want), "{params}: {e}");
    }
    assert_eq!(s.execute("trip.get", &json!({"id": id})).unwrap()["trip"]["albums"], json!([]), "the album check came first");
    for (cmd, params, want) in [
        ("trip.link", json!({"id": id}), "album"),
        ("trip.link", json!({"id": "t1", "album": alps.0}), "no such trip"),
        ("trip.unlink", json!({"id": id}), "give `album`"),
        ("trip.get", json!({"id": 5}), "missing `id`"),
        ("trip.photos", json!({"id": "nope"}), "no such trip"),
        ("trip.delete", json!({"id": "t2"}), "no such trip"),
        ("trip.toAlbum", json!({"id": "t3"}), "no such trip"),
        ("trip.create", json!({}), "missing `name`"),
        ("trip.create", json!({"name": "x", "photosInRange": 1}), "photosInRange"),
        ("trip.fromAlbum", json!({"album": "x"}), "album id"),
        ("album.resolve", json!({}), "missing `ref`"),
        ("album.trips", json!({"id": -1}), "album id"),
    ] {
        let e = err(&mut s, cmd, params.clone());
        assert!(e.contains(want), "{cmd} {params}: {e}");
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_trip_from_an_album_and_an_album_from_a_trip() {
    let Alps { mut s, root, alps, p } = alps("albums");
    let t = s.execute("trip.fromAlbum", &json!({"album": alps.0})).unwrap();
    let trip = &t["trip"];
    assert_eq!(
        (trip["name"].clone(), trip["albums"].clone(), trip["photosInRange"].clone()),
        (json!("Alps"), json!([format!("a:{}", alps.0)]), json!(false))
    );
    // the span of its photos' capture times (the one without a time doesn't count)
    assert_eq!(trip["range"], json!({"fromMs": ms("2026-07-01T07:00:00Z"), "toMs": ms("2026-07-02T08:00:00Z")}));
    let named = s.execute("trip.fromAlbum", &json!({"album": alps.0, "name": "Summer"})).unwrap();
    assert_eq!(named["trip"]["name"], "Summer");
    // an album with no timed photo: no range
    let empty = album(&mut s, "Empty", &[p[7]]);
    assert!(s.execute("trip.fromAlbum", &json!({"album": empty.0})).unwrap()["trip"].get("range").is_none());
    // and back: a new album with the trip's photos, one undo step
    let id = trip["id"].as_str().unwrap();
    s.execute("trip.update", &json!({"id": id, "photosInRange": true, "range": week(), "cover": p[2].0})).unwrap();
    let albums_before = s.catalog.albums().count();
    let made = s.execute("trip.toAlbum", &json!({"id": id})).unwrap();
    assert_eq!(made["photos"], 4);
    let new = AlbumId(made["album"].as_u64().unwrap());
    let a = s.catalog.album(new).unwrap().clone();
    assert_eq!((a.name.as_str(), a.photos.clone(), a.cover), ("Alps", vec![p[1], p[0], p[2], p[7]], Some(p[2])));
    assert_eq!(s.execute("trip.get", &json!({"id": id})).unwrap()["trip"]["albums"].as_array().unwrap().len(), 1, "the trip doesn't link it");
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(s.catalog.album(new).is_none() && s.catalog.albums().count() == albums_before, "undone in one step");
    s.execute("edit.redo", &json!({})).unwrap();
    assert_eq!(s.catalog.album(new).map(|a| a.photos.len()), Some(4));
    let named = s.execute("trip.toAlbum", &json!({"id": id, "name": "Best of"})).unwrap();
    assert_eq!(s.catalog.album(AlbumId(named["album"].as_u64().unwrap())).unwrap().name, "Best of");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn notes_are_kept_by_time_and_go_with_their_trip() {
    let Alps { mut s, root, p, .. } = alps("notes");
    let id = s.execute("trip.create", &json!({"name": "Alps"})).unwrap()["trip"]["id"].as_str().unwrap().to_string();
    let a = s.execute("trip.note.add", &json!({"trip": id, "text": "Summit", "tMs": 50, "lat": 46.5, "lon": 7.9})).unwrap()["note"].clone();
    assert!(a["id"].as_str().unwrap().starts_with('n'));
    assert_eq!(
        (a["trip"].as_str(), a["tMs"].as_i64(), a["lat"].as_f64(), a["text"].as_str()),
        (Some(id.as_str()), Some(50), Some(46.5), Some("Summit"))
    );
    // a note on a photo: when the photo was taken, the photo named by its content
    let b = s.execute("trip.note.add", &json!({"trip": id, "text": "", "photo": p[0].0})).unwrap()["note"].clone();
    assert_eq!(
        (b["tMs"].as_i64(), b["photo"].as_str(), b["photoId"].as_u64()),
        (Some(ms("2026-07-02T08:00:00Z")), Some(format!("{:032x}", 1).as_str()), Some(p[0].0))
    );
    let c = s.execute("trip.note.add", &json!({"trip": id, "text": "Now"})).unwrap()["note"].clone();
    assert!(c["tMs"].as_i64().unwrap() > ms("2026-10-01T00:00:00Z"), "else now");
    let long = "🏔".repeat(10_000);
    let d = s.execute("trip.note.add", &json!({"trip": id, "text": long, "tMs": 10})).unwrap()["note"].clone();
    let order: Vec<Value> =
        s.execute("trip.notes", &json!({"trip": id})).unwrap()["notes"].as_array().unwrap().iter().map(|n| n["id"].clone()).collect();
    assert_eq!(order, vec![d["id"].clone(), a["id"].clone(), b["id"].clone(), c["id"].clone()], "by time");
    assert_eq!(s.execute("trip.get", &json!({"id": id})).unwrap()["notes"], 4);
    // edited: the place cleared, the text changed
    let e = s.execute("trip.note.update", &json!({"id": a["id"], "text": "Summit!", "lat": null, "lon": null})).unwrap()["note"].clone();
    assert_eq!(e["text"], "Summit!");
    assert!(e.get("lat").is_none() && e.get("lon").is_none() && e["tMs"] == 50, "{e}");
    for (cmd, params, want) in [
        ("trip.note.add", json!({"trip": id}), "missing `text`"),
        ("trip.note.add", json!({"trip": id, "text": "x".repeat(10_001)}), "10000"),
        ("trip.note.add", json!({"trip": id, "text": 5}), "`text` is text"),
        ("trip.note.add", json!({"trip": id, "text": "x", "lat": 1.0}), "together"),
        ("trip.note.add", json!({"trip": id, "text": "x", "lat": 91.0, "lon": 0.0}), "-90..90"),
        ("trip.note.add", json!({"trip": id, "text": "x", "tMs": "noon"}), "tMs"),
        ("trip.note.add", json!({"trip": id, "text": "x", "photo": 99999}), "no such photo"),
        ("trip.note.add", json!({"trip": "t-gone", "text": "x"}), "no such trip"),
        ("trip.note.update", json!({"id": "n-gone"}), "no such note: n-gone"),
        ("trip.note.update", json!({"id": a["id"], "lon": 5.0}), "together"),
        ("trip.note.delete", json!({"id": "n-gone"}), "no such note"),
        ("trip.notes", json!({"trip": "t-gone"}), "no such trip"),
    ] {
        let e = err(&mut s, cmd, params.clone());
        assert!(e.contains(want), "{cmd} {params}: {e}");
    }
    assert_eq!(s.execute("trip.note.delete", &json!({"id": c["id"]})).unwrap(), json!({"removed": true}));
    // a note of another trip stays when this one goes
    let other = s.execute("trip.create", &json!({"name": "Other"})).unwrap()["trip"]["id"].as_str().unwrap().to_string();
    s.execute("trip.note.add", &json!({"trip": other, "text": "Mine"})).unwrap();
    assert_eq!(s.execute("trip.delete", &json!({"id": id})).unwrap(), json!({"removed": true, "notes": 3}));
    assert!(err(&mut s, "trip.get", json!({"id": id})).contains("no such trip"));
    let left = s.doc_items("trip-notes");
    assert_eq!(left.len(), 1, "{left:?}");
    assert_eq!(left[0]["trip"], json!(other));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn new_trips_and_notes_are_refused_past_the_limits() {
    let root = temp_dir("limits");
    let mut s = open(&root);
    let trips: Vec<Value> = (0..crate::trips::TRIPS_LIMIT).map(|i| json!({"id": format!("t{i}"), "name": "x", "createdMs": i})).collect();
    s.set_doc_items("trips", &trips).unwrap();
    let e = err(&mut s, "trip.create", json!({"name": "One more"}));
    assert!(e.starts_with("limit: ") && e.contains("500"), "{e}");
    s.execute("trip.delete", &json!({"id": "t0"})).unwrap();
    s.execute("trip.create", &json!({"name": "The 500th"})).unwrap();
    // notes: by count
    let notes: Vec<Value> =
        (0..crate::trips::NOTES_LIMIT).map(|i| json!({"id": format!("n{i}"), "trip": "t1", "tMs": i, "text": "x", "createdMs": i})).collect();
    s.set_doc_items("trip-notes", &notes).unwrap();
    let e = err(&mut s, "trip.note.add", json!({"trip": "t1", "text": "one more"}));
    assert!(e.starts_with("limit: ") && e.contains("1500"), "{e}");
    // and by size: long notes up to 16 MiB
    let long = "🏔".repeat(crate::trips::TEXT_MAX);
    let big: Vec<Value> = (0..430).map(|i| json!({"id": format!("n{i}"), "trip": "t1", "tMs": i, "text": long, "createdMs": i})).collect();
    s.set_doc_items("trip-notes", &big).unwrap();
    assert_eq!(s.doc_items("trip-notes").len(), 430, "taken from elsewhere up to a server's 24 MiB");
    let e = err(&mut s, "trip.note.add", json!({"trip": "t1", "text": "short"}));
    assert!(e.starts_with("limit: ") && e.contains("16 MiB"), "{e}");
    assert_eq!(s.doc_items("trip-notes").len(), 430, "nothing added");
    // (deleting still works there, and so does a change that makes it smaller)
    s.execute("trip.note.delete", &json!({"id": "n0"})).unwrap();
    s.execute("trip.note.update", &json!({"id": "n1", "text": "shorter"})).unwrap();
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn hostile_documents_are_taken_item_by_item_and_never_crash_a_command() {
    let Alps { mut s, root, alps, .. } = alps("hostile");
    let good = json!({"id": "tGood", "name": "Good", "createdMs": 1, "albums": [format!("a:{}", alps.0), "a:77777", "u:nowhere", "z:future"],
                      "range": {"fromMs": 0, "toMs": 10}, "photosInRange": true, "tracks": ["s-not-here"]});
    let mut items = vec![
        good.clone(),
        json!({"id": "tGood", "name": "Repeated id", "createdMs": 2}),
        json!({"id": "t/../x", "name": "a", "createdMs": 1}),
        json!({"id": "t\nx", "name": "a", "createdMs": 1}),
        json!({"id": "tBig", "name": "x".repeat(5000), "createdMs": 1}),
        json!({"id": "tNum", "name": 5, "createdMs": 1}),
        json!({"id": "tAlb", "name": "a", "createdMs": 1, "albums": [1, 2]}),
        json!({"id": "tNewer", "fmt": 3, "whatever": [1]}),
        json!(["not", "an", "item"]),
        Value::Null,
    ];
    items.extend((0..10_000).map(|i| json!({"id": format!("tMany{i}"), "name": "m", "createdMs": i})));
    s.set_doc_items("trips", &items).unwrap();
    let kept = s.doc_items("trips");
    assert_eq!(kept.len(), 2000, "a server's limit");
    assert_eq!((kept[0].clone(), kept[1]["id"].clone()), (good.clone(), json!("tNewer")));
    // notes pointing at no trip are kept (theirs may come later); notes that aren't notes are not
    let notes = vec![
        json!({"id": "nOrphan", "trip": "tLater", "tMs": 1, "text": "early", "createdMs": 1}),
        json!({"id": "nBad", "trip": "not a trip", "tMs": 1, "text": "x", "createdMs": 1}),
        json!({"id": "nHuge", "trip": "tGood", "tMs": 1, "text": "x".repeat(20_000), "createdMs": 1}),
        json!({"id": "nPos", "trip": "tGood", "tMs": 1, "text": "x", "createdMs": 1, "lat": "north", "lon": 1}),
        json!({"id": "nOk", "trip": "tGood", "tMs": 1, "text": "x", "createdMs": 1, "photo": "ffff"}),
    ];
    s.set_doc_items("trip-notes", &notes).unwrap();
    assert_eq!(s.doc_items("trip-notes").iter().map(|n| n["id"].clone()).collect::<Vec<_>>(), vec![json!("nOrphan"), json!("nOk")]);
    // a trip pointing at nothing: listed, its albums unresolved, no photos, no crash
    let r = s.execute("trip.get", &json!({"id": "tGood"})).unwrap();
    let resolved: Vec<Value> = r["linked"].as_array().unwrap().iter().map(|l| l["album"]["id"].clone()).collect();
    assert_eq!(resolved, vec![json!(alps.0), Value::Null, Value::Null, Value::Null]);
    assert_eq!(s.trip_tracks("tGood").unwrap(), Vec::<String>::new(), "a session that isn't here");
    assert_eq!(s.execute("trip.notes", &json!({"trip": "tGood"})).unwrap()["notes"][0]["photoId"], Value::Null);
    let list = s.execute("trip.list", &json!({})).unwrap();
    let newer = list["trips"].as_array().unwrap().iter().find(|t| t["id"] == "tNewer").unwrap().clone();
    assert_eq!(newer["unreadable"], true);
    // a newer version's trip is kept, shown and deletable, never changed by this one
    assert_eq!(s.execute("trip.get", &json!({"id": "tNewer"})).unwrap()["unreadable"], true);
    for cmd in ["trip.update", "trip.link", "trip.unlink"] {
        let e = err(&mut s, cmd, json!({"id": "tNewer", "album": alps.0, "name": "x"}));
        assert!(e.contains("newer version"), "{cmd}: {e}");
    }
    s.execute("trip.delete", &json!({"id": "tNewer"})).unwrap();
    // every command, with junk
    for spec in
        crate::cmd::command_specs().iter().filter(|c| c.id.starts_with("trip.") || ["album.ref", "album.resolve", "album.trips"].contains(&c.id))
    {
        for p in
            [json!(null), json!({}), json!({"id": [1], "trip": {}, "album": "x", "ref": 5, "text": null}), json!({"id": "tGood", "albums": {"a": 1}})]
        {
            let _ = s.execute(spec.id, &p);
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_trip_that_leaves_from_elsewhere_takes_its_notes() {
    let root = temp_dir("gone");
    let mut s = open(&root);
    let keep = s.execute("trip.create", &json!({"name": "Keep"})).unwrap()["trip"].clone();
    let go = s.execute("trip.create", &json!({"name": "Go"})).unwrap()["trip"].clone();
    for t in [&keep, &go] {
        s.execute("trip.note.add", &json!({"trip": t["id"], "text": "a"})).unwrap();
    }
    // a note whose trip hasn't come yet stays
    let mut notes = s.doc_items("trip-notes");
    notes.push(json!({"id": "nEarly", "trip": "tLater", "tMs": 1, "text": "x", "createdMs": 1}));
    s.set_doc_items("trip-notes", &notes).unwrap();
    s.set_doc_items("trips", std::slice::from_ref(&keep)).unwrap();
    let left: Vec<Value> = s.doc_items("trip-notes").iter().map(|n| n["trip"].clone()).collect();
    assert_eq!(left, vec![keep["id"].clone(), json!("tLater")]);
    let _ = std::fs::remove_dir_all(root);
}

fn corrupt_copies(root: &Path, name: &str) -> usize {
    std::fs::read_dir(root).unwrap().filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().starts_with(&format!("{name}.corrupt-"))).count()
}

#[test]
fn trips_stay_with_the_library_and_damaged_files_are_set_aside() {
    let root = temp_dir("files");
    let mut s = open(&root);
    let t = s.execute("trip.create", &json!({"name": "Kept"})).unwrap()["trip"].clone();
    s.execute("trip.note.add", &json!({"trip": t["id"], "text": "too"})).unwrap();
    s.close_library().unwrap();
    drop(s);
    let mut s = open(&root);
    assert_eq!(s.doc_items("trips"), vec![t.clone()]);
    assert_eq!(s.doc_items("trip-notes").len(), 1);
    s.close_library().unwrap();
    drop(s);
    // damaged items: the file as it was is kept aside, the rest opens
    let mut items: Vec<Value> = serde_json::from_slice(&std::fs::read(root.join("trips.json")).unwrap()).unwrap();
    items.push(json!({"id": "../../escape"}));
    std::fs::write(root.join("trips.json"), serde_json::to_vec(&items).unwrap()).unwrap();
    std::fs::write(root.join("trip-notes.json"), b"{not json").unwrap();
    let mut s = open(&root);
    let warnings = s.take_library_warnings();
    assert!(warnings.iter().any(|w| w.contains("trips.json") && w.contains("1 of its items can't be read")), "{warnings:?}");
    assert!(warnings.iter().any(|w| w.contains("trip-notes.json")), "{warnings:?}");
    assert_eq!((corrupt_copies(&root, "trips.json"), corrupt_copies(&root, "trip-notes.json")), (1, 1));
    assert_eq!(s.doc_items("trips"), vec![t.clone()]);
    assert!(s.doc_items("trip-notes").is_empty());
    s.execute("trip.note.add", &json!({"trip": t["id"], "text": "again"})).unwrap();
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_trips_tracks_are_its_list_or_those_that_overlap_it() {
    let Alps { mut s, root, alps, .. } = alps("tracks");
    let walk = |t0: i64| -> Value { Value::Array((0..20).map(|i| json!([t0 + i * 60_000, 46.0 + i as f64 * 1e-4, 7.5])).collect()) };
    let inside = s.execute("track.create", &json!({"points": walk(ms("2026-07-02T06:00:00Z"))})).unwrap()["session"].as_str().unwrap().to_string();
    let outside = s.execute("track.create", &json!({"points": walk(ms("2026-08-02T06:00:00Z"))})).unwrap()["session"].as_str().unwrap().to_string();
    let ranged = s.execute("trip.create", &json!({"name": "Ranged", "range": week()})).unwrap()["trip"]["id"].as_str().unwrap().to_string();
    assert_eq!(s.trip_tracks(&ranged).unwrap(), vec![inside.clone()]);
    // without a range: the span of its photos (1 to 2 July)
    let spanned = s.execute("trip.create", &json!({"name": "Spanned", "albums": [alps.0]})).unwrap()["trip"]["id"].as_str().unwrap().to_string();
    assert_eq!(s.trip_tracks(&spanned).unwrap(), vec![inside.clone()]);
    let none = s.execute("trip.create", &json!({"name": "None"})).unwrap()["trip"]["id"].as_str().unwrap().to_string();
    assert!(s.trip_tracks(&none).unwrap().is_empty());
    let listed = s.execute("trip.create", &json!({"name": "Listed", "tracks": [outside, "s00000000000000ff"]})).unwrap()["trip"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(s.trip_tracks(&listed).unwrap(), vec![outside.clone()], "the ones this device has");
    assert!(s.trip_tracks("t-gone").unwrap_err().to_string().contains("no such trip"));
    let _ = std::fs::remove_dir_all(root);
}
