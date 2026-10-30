//! Recorded tracks between two libraries through a real server on a loopback port, with the engine's own transport:
//! the `tracks` document and its privacy trim item (docs/sync.md, "Recorded tracks").

use std::path::{Path, PathBuf};

use lightcraft_engine::Session;
use lightcraft_server::{Config, Server, accounts};
use serde_json::{Value, json};

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-server-tracks-{tag}-{}", std::process::id()));
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

fn sessions(s: &mut Session) -> Vec<Value> {
    s.execute("track.list", &json!({})).unwrap()["sessions"].as_array().unwrap().clone()
}

/// A walk in two stretches, 30 s apart, 300 points.
fn gpx() -> String {
    let seg = |from: usize| {
        (from..from + 150)
            .map(|i| {
                let (s, lat, lon) = (i * 30, 46.0 + i as f64 * 1e-4, 7.0 + i as f64 * 2e-4);
                format!(
                    "<trkpt lat=\"{lat:.6}\" lon=\"{lon:.6}\"><ele>{}</ele><time>2026-05-01T{:02}:{:02}:{:02}Z</time></trkpt>",
                    1600 + i,
                    8 + s / 3600,
                    s / 60 % 60,
                    s % 60
                )
            })
            .collect::<String>()
    };
    format!("<gpx version=\"1.1\"><trk><trkseg>{}</trkseg><trkseg>{}</trkseg></trk></gpx>", seg(0), seg(200))
}

#[test]
fn recorded_tracks_travel_between_two_libraries_through_the_server() {
    let root = temp("two");
    let server = start(&root);
    let url = format!("http://{}", server.addr());
    let mut a = open(&root.join("a"));
    sign_in(&mut a, &url);
    sync(&mut a);
    let mut b = open(&root.join("b"));
    sign_in(&mut b, &url);
    sync(&mut b);

    // recorded on A, with the privacy trim on: both reach B
    let r = a.execute("track.import", &json!({"gpx": gpx(), "name": "Ridge"})).unwrap();
    let session = r["session"].clone();
    a.execute("tracking.settings", &json!({"trim": true, "trimRadiusM": 100})).unwrap();
    sync(&mut a);
    sync(&mut b);
    let on_a = a.execute("track.get", &json!({"session": session, "points": true})).unwrap();
    let on_b = b.execute("track.get", &json!({"session": session, "points": true})).unwrap();
    assert_eq!((on_b["name"].clone(), on_b["points"].as_array().map(Vec::len)), (json!("Ridge"), Some(300)));
    assert_eq!(on_a["points"], on_b["points"], "point for point, the break included");
    assert_eq!(b.execute("tracking.settings", &json!({})).unwrap(), json!({"trim": true, "trimRadiusM": 100.0}));
    let export = |s: &mut Session| s.execute("track.export", &json!({"session": session})).unwrap();
    let (ea, eb) = (export(&mut a), export(&mut b));
    assert_eq!(ea["gpx"], eb["gpx"], "the same file from either library");
    assert!(eb["trimmed"]["start"].as_u64().unwrap() > 0, "trimmed by the shared setting: {}", eb["trimmed"]);

    // renamed on B, then deleted on A: both reach the other
    b.execute("track.rename", &json!({"session": session, "name": "Ridge walk"})).unwrap();
    sync(&mut b);
    sync(&mut a);
    assert_eq!(sessions(&mut a)[0]["name"], "Ridge walk");
    a.execute("track.delete", &json!({"session": session})).unwrap();
    sync(&mut a);
    sync(&mut b);
    assert!(sessions(&mut b).is_empty());
    // what the server keeps: the settings item alone
    let list = b.execute("track.list", &json!({})).unwrap();
    assert_eq!(list["docItems"], 1, "{list}");
    drop(server);
    let _ = std::fs::remove_dir_all(&root);
}
