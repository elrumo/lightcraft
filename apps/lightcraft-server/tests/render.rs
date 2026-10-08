//! `POST /api/render` over real HTTP: a device sends a photo's content hash with its edits and export options, and gets
//! the rendered image back — the way a phone exports at a size it can't hold in memory.

use std::path::{Path, PathBuf};

use lightcraft_server::{Config, Server, accounts};
use serde_json::{Value, json};

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-server-render-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A 160 × 120 PNG: a mid-grey gradient, so exposure changes show.
fn png() -> Vec<u8> {
    let (w, h) = (160usize, 120usize);
    let data: Vec<[u8; 4]> = (0..w * h).map(|i| [(40 + i % w / 2) as u8, (60 + i / w) as u8, 90, 255]).collect();
    let img = lightcraft_raster::Rgba8 { width: w, height: h, data };
    lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap()
}

fn start(root: &Path, render_threads: usize) -> Server {
    let data = root.join("data");
    accounts::set_user(&data, "ann", "correct horse", false).unwrap();
    let mut cfg = Config::new(data, "127.0.0.1:0");
    cfg.scan_interval = None;
    cfg.preview_threads = 1;
    cfg.render_threads = render_threads;
    Server::start(cfg).unwrap()
}

fn call(method: &str, url: &str, token: &str, body: Option<&[u8]>) -> (u16, Vec<u8>, Vec<(String, String)>) {
    let a: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
    let auth = format!("Bearer {token}");
    let r = match method {
        "PUT" => a.put(url).header("Authorization", &auth).send(body.unwrap_or(&[])),
        _ => a.post(url).header("Authorization", &auth).send(body.unwrap_or(&[])),
    };
    let mut r = r.unwrap();
    let hs = r.headers().iter().map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string())).collect();
    let status = r.status().as_u16();
    (status, r.body_mut().read_to_vec().unwrap_or_default(), hs)
}

/// The mean of a rendered image's channels, 0…1, and its size.
fn mean_and_size(bytes: &[u8]) -> (f32, (usize, usize)) {
    let d = lightcraft_codecs::decode(bytes, lightcraft_codecs::DecodeOptions::fit(4096, 4096)).unwrap();
    let img = d.to_working();
    let sum: f32 = img.data.iter().map(|p| p[0] + p[1] + p[2]).sum();
    (sum / (img.data.len() * 3) as f32, (img.width, img.height))
}

#[test]
fn the_server_renders_a_photo_it_keeps_with_the_edits_it_is_sent() {
    let root = temp("route");
    let server = start(&root, 1);
    let base = format!("http://{}", server.addr());
    let (s, body, _) = call(
        "POST",
        &format!("{base}/api/login"),
        "",
        Some(json!({"user": "ann", "password": "correct horse", "device": "t"}).to_string().as_bytes()),
    );
    assert_eq!(s, 200);
    let token = serde_json::from_slice::<Value>(&body).unwrap()["token"].as_str().unwrap().to_string();
    let original = png();
    let hash = lightcraft_preview::hash_bytes(&original).to_string();
    let render = |body: Value| call("POST", &format!("{base}/api/render"), &token, Some(body.to_string().as_bytes()));
    let ask = |settings: Value, export: Value| render(json!({"hash": hash, "name": "photo.png", "settings": settings, "export": export}));

    // no original yet: nothing to render
    assert_eq!(ask(json!({}), json!({"format": "jpg"})).0, 404);
    assert_eq!(call("PUT", &format!("{base}/api/blobs/original/{hash}"), &token, Some(&original)).0, 200);

    // rendered at the size asked, as the format asked, and the edits change it
    let (s, plain, hs) = ask(json!({}), json!({"format": "jpg", "quality": 90, "longEdge": 80}));
    assert_eq!(s, 200, "{}", String::from_utf8_lossy(&plain));
    assert!(hs.iter().any(|(k, v)| k.eq_ignore_ascii_case("content-type") && v == "image/jpeg"));
    assert!(hs.iter().any(|(k, v)| k.eq_ignore_ascii_case("x-lightcraft-size") && v == "80x60"), "{hs:?}");
    let (plain_mean, size) = mean_and_size(&plain);
    assert_eq!(size, (80, 60));
    let (s, bright, _) = ask(json!({"light": {"exposure": 1.5}}), json!({"format": "jpg", "quality": 90, "longEdge": 80}));
    assert_eq!(s, 200);
    assert!(mean_and_size(&bright).0 > plain_mean + 0.05, "+1.5 EV is brighter: {plain_mean} → {}", mean_and_size(&bright).0);
    let (s, png_out, hs) = ask(json!({}), json!({"format": "png"}));
    assert_eq!(s, 200);
    assert!(hs.iter().any(|(k, v)| k.eq_ignore_ascii_case("content-type") && v == "image/png"));
    assert_eq!(mean_and_size(&png_out).1, (160, 120), "no size asked: the full size");

    // what it refuses
    assert_eq!(render(json!({"hash": "not-a-hash", "name": "p.png", "settings": {}, "export": {}})).0, 400);
    assert_eq!(ask(json!({}), json!({"format": "original"})).0, 422, "only rendered formats");
    assert_eq!(render(json!({"hash": hash})).0, 400, "a request that isn't one");
    assert_eq!(call("POST", &format!("{base}/api/render"), "", Some(b"{}")).0, 401);
    assert_eq!(call("POST", &format!("{base}/api/render"), "made-up", Some(b"{}")).0, 401);
    // the scratch links are gone
    let scratch = root.join("data").join("tmp");
    assert!(std::fs::read_dir(&scratch).map_or(true, |d| d.flatten().all(|e| !e.file_name().to_string_lossy().starts_with("render-"))));
}
