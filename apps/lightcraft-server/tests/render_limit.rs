//! An export that wouldn't fit this device's memory goes to the sync server — or fails with a message — instead of
//! being tried. The limit is the session's own: the server, in this process, renders without one.

use std::path::{Path, PathBuf};

use lightcraft_engine::Session;
use lightcraft_engine::export::{ExportFormat, ExportOptions, export_photo};
use lightcraft_server::{Config, Server, accounts};
use serde_json::json;

fn write_png(path: &Path) {
    let (w, h) = (160usize, 120usize);
    let data: Vec<[u8; 4]> = (0..w * h).map(|i| [(40 + i % w / 2) as u8, (60 + i / w) as u8, 90, 255]).collect();
    let img = lightcraft_raster::Rgba8 { width: w, height: h, data };
    let bytes = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn a_render_over_the_limit_goes_to_the_server_or_is_refused_clearly() {
    let root: PathBuf = std::env::temp_dir().join(format!("lc-server-render-limit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let data = root.join("data");
    accounts::set_user(&data, "ann", "correct horse", false).unwrap();
    let mut cfg = Config::new(data, "127.0.0.1:0");
    cfg.scan_interval = None;
    cfg.preview_threads = 1;
    let server = Server::start(cfg).unwrap();
    let url = format!("http://{}", server.addr());

    write_png(&root.join("in/a.png"));
    let mut s = Session::new().with_fs();
    s.open_library(&root.join("lib"), false).unwrap();
    s.execute("library.import", &json!({"paths": [root.join("in").to_string_lossy()]})).unwrap();
    let id = s.catalog.photos().next().unwrap().id;
    s.execute("sync.signIn", &json!({"server": url, "user": "ann", "password": "correct horse", "device": "t"})).unwrap();
    s.execute("sync.now", &json!({"wait": true})).unwrap();

    let o = ExportOptions { format: ExportFormat::Jpeg, ..Default::default() };
    // plenty of room: rendered here
    s.set_export_limit(Some(1 << 30));
    assert!(export_photo(&mut s, id, &o, 1).is_ok());
    // a device that can only spare a kilobyte: the signed-in one lets the server render, the other says what to do
    s.set_export_limit(Some(1024));
    let e = export_photo(&mut s, id, &o, 1).unwrap();
    assert_eq!((e.width, e.height), (160, 120));
    assert!(e.bytes.starts_with(&[0xff, 0xd8]), "a JPEG from the server");

    write_png(&root.join("in2/b.png"));
    let mut lone = Session::new().with_fs();
    lone.open_library(&root.join("lib2"), false).unwrap();
    lone.execute("library.import", &json!({"paths": [root.join("in2").to_string_lossy()]})).unwrap();
    let lone_id = lone.catalog.photos().next().unwrap().id;
    lone.set_export_limit(Some(1024));
    let err = export_photo(&mut lone, lone_id, &o, 1).err().unwrap();
    assert!(err.contains("needs about") && err.contains("sync server"), "{err}");
    // no limit: the same export is rendered here again
    lone.set_export_limit(None);
    assert!(export_photo(&mut lone, lone_id, &o, 1).is_ok());
    drop(server);
    let _ = std::fs::remove_dir_all(&root);
}
