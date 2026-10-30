//! Trips and their notes between two libraries through a real server on a loopback port, with the engine's own
//! transport: the `trips` and `trip-notes` documents, and an album reference by id (docs/sync.md, "Trips").

use std::path::{Path, PathBuf};

use lightcraft_engine::Session;
use lightcraft_server::{Config, Server, accounts};
use serde_json::json;

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-server-trips-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn start(root: &Path) -> Server {
    let data = root.join("data");
    accounts::set_user(&data, "ann", "correct horse", false).unwrap();
    let mut cfg = Config::new(data, "127.0.0.1:0");
    cfg.scan_interval = None;
    cfg.preview_threads = Some(1);
    Server::start(cfg).unwrap()
}

fn open(dir: &Path) -> Session {
    let mut s = Session::new().with_fs();
    s.open_library(dir, false).unwrap();
    s
}

fn sign_in(s: &mut Session, url: &str) {
    s.execute("sync.signIn", &json!({"server": url, "user": "ann", "password": "correct horse", "device": "test"})).unwrap();
}

fn sync(s: &mut Session) {
    let st = s.execute("sync.now", &json!({"wait": true})).unwrap();
    assert_eq!(st["state"], "idle", "{st}");
}

#[test]
fn trips_and_their_notes_travel_between_two_libraries_through_the_server() {
    let root = temp("two");
    let server = start(&root);
    let url = format!("http://{}", server.addr());
    let mut a = open(&root.join("a"));
    sign_in(&mut a, &url);
    sync(&mut a);
    let mut b = open(&root.join("b"));
    sign_in(&mut b, &url);
    sync(&mut b);

    // a trip on A that links an album: the album, the trip and its note reach B, the reference by the shared id
    let album = a.execute("album.create", &json!({"name": "Alps"})).unwrap()["id"].as_u64().unwrap();
    let trip = a.execute("trip.create", &json!({"name": "Alps", "albums": [album]})).unwrap()["trip"].clone();
    assert_eq!(trip["albums"], json!([format!("a:{album}")]));
    a.execute("trip.note.add", &json!({"trip": trip["id"], "text": "Summit", "tMs": 5})).unwrap();
    sync(&mut a);
    sync(&mut b);
    let got = b.execute("trip.get", &json!({"id": trip["id"]})).unwrap();
    assert_eq!(got["trip"], trip);
    assert_eq!(got["linked"][0]["album"], json!({"id": album, "name": "Alps"}));
    assert_eq!(b.execute("trip.notes", &json!({"trip": trip["id"]})).unwrap()["notes"][0]["text"], "Summit");
    assert_eq!(b.execute("album.trips", &json!({"id": album})).unwrap()["trips"][0]["id"], trip["id"]);

    // a note added on each: both kept everywhere
    a.execute("trip.note.add", &json!({"trip": trip["id"], "text": "From A"})).unwrap();
    b.execute("trip.note.add", &json!({"trip": trip["id"], "text": "From B"})).unwrap();
    sync(&mut a);
    sync(&mut b);
    sync(&mut a);
    for s in [&mut a, &mut b] {
        assert_eq!(s.execute("trip.get", &json!({"id": trip["id"]})).unwrap()["notes"], 3);
    }

    // deleted on B: gone on A
    assert_eq!(b.execute("trip.delete", &json!({"id": trip["id"]})).unwrap(), json!({"removed": true, "notes": 3}));
    sync(&mut b);
    sync(&mut a);
    assert!(a.execute("trip.list", &json!({})).unwrap()["trips"].as_array().unwrap().is_empty());
    drop(server);
    let _ = std::fs::remove_dir_all(&root);
}
