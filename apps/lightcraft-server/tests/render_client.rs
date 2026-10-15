//! A device exporting on the sync server: a real engine session signs in, its photo is uploaded, and
//! `ExportOptions::on_server` has the server render it. The result is the same picture as exporting here.

use std::path::{Path, PathBuf};
use std::time::Duration;

use lightcraft_engine::Session;
use lightcraft_engine::export::{ExportFormat, ExportOptions, Resize, ResizeMode, export_photo};
use lightcraft_server::{Config, Server, accounts};
use serde_json::json;

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-server-render-client-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write_png(path: &Path) {
    let (w, h) = (160usize, 120usize);
    let data: Vec<[u8; 4]> = (0..w * h).map(|i| [(40 + i % w / 2) as u8, (60 + i / w) as u8, 90, 255]).collect();
    let img = lightcraft_raster::Rgba8 { width: w, height: h, data };
    let bytes = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn start(root: &Path) -> Server {
    let data = root.join("data");
    accounts::set_user(&data, "ann", "correct horse", false).unwrap();
    let mut cfg = Config::new(data, "127.0.0.1:0");
    cfg.scan_interval = None;
    cfg.preview_threads = Some(1);
    Server::start(cfg).unwrap()
}

fn mean(bytes: &[u8]) -> (Vec<f32>, (usize, usize)) {
    let img = lightcraft_codecs::decode(bytes, lightcraft_codecs::DecodeOptions::fit(4096, 4096)).unwrap().to_working();
    (img.data.iter().flat_map(|p| *p).collect(), (img.width, img.height))
}

fn long_edge(n: u32) -> Option<Resize> {
    Some(Resize { mode: ResizeMode::LongEdge, value: n as f32, height: 0, dont_enlarge: true })
}

#[test]
fn an_export_the_server_renders_is_the_picture_exporting_here_gives() {
    let root = temp("on-server");
    let server = start(&root);
    let url = format!("http://{}", server.addr());
    write_png(&root.join("in/a.png"));
    let mut s = Session::new().with_fs();
    s.open_library(root.join("lib"), false).unwrap();
    s.execute("library.import", &json!({"paths": [root.join("in").to_string_lossy()]})).unwrap();
    let id = s.catalog.photos().next().unwrap().id;
    s.execute("sync.signIn", &json!({"server": url, "user": "ann", "password": "correct horse", "device": "t"})).unwrap();
    assert_eq!(s.execute("sync.now", &json!({"wait": true})).unwrap()["state"], "idle");
    s.selection = lightcraft_engine::Selection::single(id);
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 0.8})).unwrap();

    let here = ExportOptions { format: ExportFormat::Jpeg, resize: long_edge(64), ..Default::default() };
    let there = ExportOptions { on_server: true, ..here.clone() };
    let local = export_photo(&mut s, id, &here, 1).unwrap();
    let remote = export_photo(&mut s, id, &there, 1).unwrap();
    assert_eq!((local.file_name.as_str(), remote.file_name.as_str()), ("a.jpg", "a.jpg"));
    let ((a, size_a), (b, size_b)) = (mean(&local.bytes), mean(&remote.bytes));
    assert_eq!((size_a, size_b), ((64, 48), (64, 48)));
    let diff: f32 = a.iter().zip(&b).map(|(x, y)| (x - y).abs()).sum::<f32>() / a.len() as f32;
    assert!(diff < 0.01, "the server's picture differs from the local one by {diff}");
    // the edit was applied there too: the unedited picture is darker
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 0.0})).unwrap();
    let (flat, _) = mean(&export_photo(&mut s, id, &there, 1).unwrap().bytes);
    assert!(b.iter().sum::<f32>() > flat.iter().sum::<f32>() * 1.05);

    // a photo the server doesn't have (not uploaded) says so instead of failing in the dark
    write_png(&root.join("in2/b.png"));
    let mut lone = Session::new().with_fs();
    lone.open_library(root.join("lib2"), false).unwrap();
    lone.execute("library.import", &json!({"paths": [root.join("in2").to_string_lossy()]})).unwrap();
    let lone_id = lone.catalog.photos().next().unwrap().id;
    let err = export_photo(&mut lone, lone_id, &there, 1).err().unwrap();
    assert!(err.contains("sync server"), "{err}");
    drop(server);
    let _ = std::fs::remove_dir_all(&root);
}

/// Files under `dir` with this extension.
fn count_files(dir: &Path, ext: &str) -> usize {
    std::fs::read_dir(dir).map_or(0, |d| {
        d.flatten()
            .map(|e| {
                let p = e.path();
                if p.is_dir() {
                    count_files(&p, ext)
                } else {
                    // (an empty `ext` counts every file: blobs have no extension)
                    usize::from(ext.is_empty() && p.extension().is_none() || p.extension().is_some_and(|x| x == ext))
                }
            })
            .sum()
    })
}

#[test]
fn the_server_builds_the_previews_of_an_upload_when_the_device_asks_it_to() {
    let root = temp("previews");
    let server = start(&root);
    let url = format!("http://{}", server.addr());
    write_png(&root.join("in/a.png"));
    let mut s = Session::new().with_fs();
    s.open_library(root.join("lib"), false).unwrap();
    s.execute("library.import", &json!({"paths": [root.join("in").to_string_lossy()]})).unwrap();
    s.execute("sync.signIn", &json!({"server": url, "user": "ann", "password": "correct horse", "device": "t"})).unwrap();
    let st = s.execute("sync.serverPreviews", &json!({"on": true})).unwrap();
    assert_eq!(st["serverPreviews"], true);
    assert_eq!(s.execute("sync.now", &json!({"wait": true})).unwrap()["state"], "idle");

    // the original went up and the server made both previews; this device built none
    let blobs = root.join("data/users/ann/blobs");
    let on_server = |kind: &str| count_files(&blobs.join(kind), "");
    assert!(on_server("original") >= 1);
    let waited = std::time::Instant::now();
    while (on_server("smart") == 0 || on_server("mini") == 0) && waited.elapsed() < Duration::from_secs(60) {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(on_server("smart") >= 1 && on_server("mini") >= 1, "the server built the previews");
    assert_eq!(count_files(&root.join("lib"), "lcsp") + count_files(&root.join("lib"), "lcsm"), 0, "and this device built none");

    // without the option the device builds (and uploads) them itself, as before (on a server of its own: v1 doesn't merge libraries)
    let control = temp("previews-control");
    let other = start(&control);
    write_png(&control.join("in2/b.png"));
    let mut t = Session::new().with_fs();
    t.open_library(control.join("lib2"), false).unwrap();
    t.execute("library.import", &json!({"paths": [control.join("in2").to_string_lossy()]})).unwrap();
    t.execute("sync.signIn", &json!({"server": format!("http://{}", other.addr()), "user": "ann", "password": "correct horse", "device": "t2"}))
        .unwrap();
    t.execute("sync.serverPreviews", &json!({"on": false})).unwrap();
    assert_eq!(t.execute("sync.now", &json!({"wait": true})).unwrap()["state"], "idle");
    assert!(count_files(&control.join("lib2"), "lcsp") >= 1, "built here");
    drop(other);
    let _ = std::fs::remove_dir_all(&control);
    drop(server);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_export_waits_for_a_busy_server_and_then_succeeds() {
    use lightcraft_engine::catalog::sync::proto::Render;
    use lightcraft_engine::cloud_sync::{RenderWait, render_on_server};

    let root = temp("busy");
    let data = root.join("data");
    accounts::set_user(&data, "ann", "correct horse", false).unwrap();
    let mut cfg = Config::new(data, "127.0.0.1:0");
    cfg.scan_interval = None;
    cfg.preview_threads = Some(1);
    cfg.render_threads = Some(1);
    // answers 503 (Retry-After: 60) soon instead of after 45 s
    cfg.render_wait = Some(Duration::from_millis(100));
    let server = Server::start(cfg).unwrap();
    write_png(&root.join("in/a.png"));
    let mut s = Session::new().with_fs();
    s.open_library(root.join("lib"), false).unwrap();
    s.execute("library.import", &json!({"paths": [root.join("in").to_string_lossy()]})).unwrap();
    let id = s.catalog.photos().next().unwrap().id;
    let url = format!("http://{}", server.addr());
    s.execute("sync.signIn", &json!({"server": url, "user": "ann", "password": "correct horse", "device": "t"})).unwrap();
    assert_eq!(s.execute("sync.now", &json!({"wait": true})).unwrap()["state"], "idle");
    let photo = s.catalog.photo(id).unwrap();
    let target = s.sync_render_target(photo).unwrap();
    let request = Render {
        hash: target.hash.clone(),
        name: photo.file_name.clone(),
        settings: json!({}),
        export: ExportOptions { format: ExportFormat::Jpeg, ..Default::default() }.to_json(),
    };
    let pacing = || RenderWait { max_pause: Duration::from_millis(200), give_up: Duration::from_secs(30), ..RenderWait::new(s.activity.clone()) };
    let waiting = |s: &Session| s.activity.list().iter().any(|t| t.kind == "export" && t.label == "Waiting for the server");

    // one render holds the only place: a second export waits (with a row), and renders once the place is free
    let held = server.hold_render_slot().expect("the place is free");
    let (t, r, w) = (target.clone(), request.clone(), pacing());
    let export = std::thread::spawn(move || render_on_server(&t, &r, &w));
    let start = std::time::Instant::now();
    while !waiting(&s) {
        assert!(start.elapsed() < Duration::from_secs(20), "the export shows its wait");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!export.is_finished(), "still waiting while the place is taken");
    drop(held);
    let bytes = export.join().unwrap().expect("the export succeeds once the place is free");
    assert!(bytes.starts_with(&[0xff, 0xd8]), "a JPEG");
    assert!(!waiting(&s), "the row goes with the wait");

    // the export's cancel ends the wait
    let held = server.hold_render_slot().expect("free again");
    let w = pacing();
    let cancel = w.cancel.clone();
    let (t, r) = (target.clone(), request.clone());
    let export = std::thread::spawn(move || render_on_server(&t, &r, &w));
    while !waiting(&s) {
        std::thread::sleep(Duration::from_millis(20));
    }
    cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(export.join().unwrap().err().as_deref(), Some("cancelled"));

    // a server that stays busy is given up on, saying so
    let w = RenderWait { give_up: Duration::from_millis(500), ..pacing() };
    let err = render_on_server(&target, &request, &w).err().unwrap();
    assert!(err.contains("busy") && err.contains("later"), "{err}");
    drop(held);
    drop(server);
    let _ = std::fs::remove_dir_all(&root);
}
