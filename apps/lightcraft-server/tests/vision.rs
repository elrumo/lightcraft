//! Search by description on the server end to end, over real HTTP: photos in a library folder get
//! their previews built and are indexed, a sentence finds them, a device sends vectors it computed,
//! and everything that must be refused is. The model is a deterministic stand-in (colours).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use lightcraft_server::{Config, Server, accounts};
use lightcraft_vision::fake::{FAKE_DIM, FAKE_ID, FakeEmbedder};
use lightcraft_vision::{Embedder, EmbeddingIndex, Key};
use serde_json::{Value, json};

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-server-vision-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write_flat(path: &Path, rgb: [u8; 3]) {
    let img = lightcraft_raster::Rgba8 { width: 64, height: 48, data: vec![[rgb[0], rgb[1], rgb[2], 255]; 64 * 48] };
    let bytes = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

/// Someone signed in: where and with which token.
struct Who {
    url: String,
    token: String,
}

struct Fixture {
    server: Server,
    ann: Who,
    data: PathBuf,
    nas: PathBuf,
}

fn start(data: &Path, model: Option<Arc<dyn Embedder>>) -> Server {
    let mut cfg = Config::new(data, "127.0.0.1:0");
    cfg.scan_interval = None;
    cfg.preview_threads = 1;
    cfg.embedder = model;
    // (never the machine's real model folder)
    cfg.vision_dir = Some(data.join("no-model-here"));
    Server::start(cfg).unwrap()
}

/// A server with `ann`, whose library folder holds a red, a green, a blue and a grey photo, and `bob`.
fn fixture(tag: &str, model: Option<Arc<dyn Embedder>>) -> Fixture {
    let root = temp(tag);
    let nas = root.join("nas");
    for (name, rgb) in [("red", [220, 20, 20]), ("green", [20, 200, 40]), ("blue", [20, 40, 220]), ("grey", [128, 128, 128])] {
        write_flat(&nas.join(format!("{name}.png")), rgb);
    }
    let data = root.join("data");
    accounts::set_user(&data, "ann", "correct horse", false).unwrap();
    accounts::set_user(&data, "bob", "battery staple", false).unwrap();
    accounts::add_folder(&data, "ann", &nas.to_string_lossy(), Some("Photos")).unwrap();
    let server = start(&data, model);
    let ann = login(&server, "ann", "correct horse");
    let f = Fixture { server, ann, data, nas };
    wait_scan(&f);
    f
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder().http_status_as_error(false).build().into()
}

fn login(server: &Server, user: &str, password: &str) -> Who {
    let url = format!("http://{}", server.addr());
    let body = json!({"user": user, "password": password, "device": "test"}).to_string();
    let mut r = agent().post(format!("{url}/api/login")).header("Content-Type", "application/json").send(body.as_bytes()).unwrap();
    assert_eq!(r.status().as_u16(), 200);
    let token = json_body(&mut r)["token"].as_str().unwrap().to_string();
    Who { url, token }
}

fn json_body(r: &mut ureq::http::Response<ureq::Body>) -> Value {
    serde_json::from_slice(&r.body_mut().read_to_vec().unwrap_or_default()).unwrap_or(Value::Null)
}

fn get(w: &Who, path: &str) -> (u16, Value) {
    let mut r = agent().get(format!("{}{path}", w.url)).header("Authorization", format!("Bearer {}", w.token)).call().unwrap();
    let status = r.status().as_u16();
    (status, json_body(&mut r))
}

fn post(w: &Who, path: &str, body: &[u8]) -> (u16, Value) {
    let mut r = agent().post(format!("{}{path}", w.url)).header("Authorization", format!("Bearer {}", w.token)).send(body).unwrap();
    let status = r.status().as_u16();
    (status, json_body(&mut r))
}

fn wait_scan(f: &Fixture) {
    for _ in 0..1200 {
        let s = f.server.folder_status("ann");
        if s.scans > 0 && !s.scanning && s.previews == 0 {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("no scan: {:?}", f.server.folder_status("ann"));
}

/// Wait until `ann` has `want` photos indexed.
fn indexed(f: &Fixture, want: u64) -> Value {
    for _ in 0..1200 {
        f.server.index_for_search();
        let (_, s) = get(&f.ann, "/api/search/status");
        if s["indexed"].as_u64() == Some(want) {
            return s;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("not indexed: {:?}", get(&f.ann, "/api/search/status"));
}

/// `file name → photo id`, from the snapshot.
fn ids_by_name(f: &Fixture) -> HashMap<String, u64> {
    let (_, snap) = get(&f.ann, "/api/snapshot");
    let photos = snap["catalog"]["photos"].as_object().expect("photos in the snapshot");
    photos.values().map(|p| (p["file_name"].as_str().unwrap().trim_end_matches(".png").to_string(), p["id"].as_u64().unwrap())).collect()
}

fn ranked(r: &Value) -> Vec<u64> {
    r["ids"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect()
}

#[test]
fn a_sentence_finds_the_photos_of_a_library_folder() {
    let f = fixture("find", Some(Arc::new(FakeEmbedder)));
    let s = indexed(&f, 4);
    assert_eq!((s["available"].clone(), s["installed"].clone(), s["total"].clone()), (json!(true), json!(true), json!(4)), "{s}");
    assert_eq!((s["model"].as_str(), s["dim"].as_u64()), (Some(FAKE_ID), Some(FAKE_DIM as u64)));
    let ids = ids_by_name(&f);
    assert_eq!(ids.len(), 4, "{ids:?}");

    for (q, want) in [("red", "red"), ("a%20GREEN+field", "green"), ("the%20blue%20sea", "blue")] {
        let (status, r) = get(&f.ann, &format!("/api/search?q={q}"));
        assert_eq!(status, 200, "{r}");
        let found = ranked(&r);
        assert_eq!(found.first(), Some(&ids[want]), "{q}: {r}");
        assert_eq!(found.len(), 4, "every indexed photo is ranked");
        let scores: Vec<f64> = r["scores"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
        assert!(scores.windows(2).all(|w| w[0] >= w[1]), "best first: {scores:?}");
        assert_eq!((r["indexed"].as_u64(), r["total"].as_u64()), (Some(4), Some(4)));
    }
    // a limit; a non-ASCII query is fine
    assert_eq!(ranked(&get(&f.ann, "/api/search?q=red&limit=2").1).len(), 2);
    assert_eq!(get(&f.ann, "/api/search?q=%E7%8C%AB%E3%81%AE%E5%86%99%E7%9C%9F").0, 200);
    // the vectors are on disk, filed by the user and the model
    assert!(f.data.join("users/ann/search").join(format!("{FAKE_ID}.bin")).is_file());
}

#[test]
fn search_is_refused_without_a_sign_in_a_query_or_a_model() {
    let f = fixture("refuse", Some(Arc::new(FakeEmbedder)));
    indexed(&f, 4);
    let a = agent();
    for path in ["/api/search?q=red", "/api/search/status", "/api/index/embeddings/keys"] {
        assert_eq!(a.get(format!("{}{path}", f.ann.url)).call().unwrap().status().as_u16(), 401, "{path}");
    }
    for path in ["/api/search", "/api/search?q=", "/api/search?q=%20%20", "/api/search?limit=3"] {
        let (status, r) = get(&f.ann, path);
        assert_eq!(status, 400, "{path}: {r}");
    }
    // hostile input is data: bad escapes, odd limits, a very long query
    for path in ["/api/search?q=%", "/api/search?q=%zz%f", "/api/search?q=a&limit=-1", "/api/search?q=a&limit=99999999999999999999999"] {
        assert_eq!(get(&f.ann, path).0, 200, "{path}");
    }
    assert_eq!(get(&f.ann, &format!("/api/search?q={}", "red%20".repeat(5000))).0, 200);
    assert!(ranked(&get(&f.ann, "/api/search?q=red&limit=9999").1).len() <= 2000);

    // another user has their own (empty) index and finds none of ann's photos
    let bob = login(&f.server, "bob", "battery staple");
    let (status, r) = get(&bob, "/api/search?q=red");
    assert_eq!((status, r["ids"].clone()), (200, json!([])), "{r}");
    assert_eq!(get(&bob, "/api/search/status").1["indexed"], 0);

    // no model installed: every route says so instead of failing
    let none = fixture("nomodel", None);
    let (status, s) = get(&none.ann, "/api/search/status");
    assert_eq!((status, s["installed"].clone(), s["indexed"].clone()), (200, json!(false), json!(0)), "{s}");
    let (status, r) = get(&none.ann, "/api/search?q=red");
    assert_eq!(status, 503, "{r}");
    assert!(r["error"].as_str().unwrap().contains("model download"), "{r}");
}

#[test]
fn a_device_sends_the_vectors_it_computed() {
    let f = fixture("upload", Some(Arc::new(FakeEmbedder)));
    indexed(&f, 4);
    let ids = ids_by_name(&f);
    assert_eq!(ids.len(), 4);

    // the keys the server has: four, for this model
    let mut keys =
        agent().get(format!("{}/api/index/embeddings/keys", f.ann.url)).header("Authorization", format!("Bearer {}", f.ann.token)).call().unwrap();
    assert_eq!(keys.headers().get("X-Model").unwrap().to_str().unwrap(), FAKE_ID);
    assert_eq!(keys.body_mut().read_to_vec().unwrap().len(), 4 * 16);

    // a photo that is not in ann's library is skipped, never kept
    let mut device = EmbeddingIndex::in_memory(FAKE_ID, FAKE_DIM).unwrap();
    device.insert(Key::of("not-in-the-library"), &[1.0, 0.0, 0.0, 0.05]).unwrap();
    let (status, r) = post(&f.ann, "/api/index/embeddings", &device.export(|_| false, 100));
    assert_eq!((status, r["added"].clone(), r["skipped"].clone()), (200, json!(0), json!(1)), "{r}");
    assert_eq!(get(&f.ann, "/api/search/status").1["indexed"], 4);
}

#[test]
fn a_device_fills_a_server_that_has_no_model() {
    // the server can't embed (no model) but takes vectors a device computed; it can't answer
    // searches without the model either, but keeps them for when it can
    let f = fixture("device-only", None);
    let ids = ids_by_name(&f);
    let (_, snap) = get(&f.ann, "/api/snapshot");
    // the device knows these photos by content hash
    let hashes: HashMap<String, String> = snap["catalog"]["photos"]
        .as_object()
        .unwrap()
        .values()
        .map(|p| (p["file_name"].as_str().unwrap().trim_end_matches(".png").to_string(), p["content_hash"].as_str().unwrap().to_string()))
        .collect();
    assert_eq!((ids.len(), hashes.len()), (4, 4));
    // (a server without a model is a SigLIP 2 server: vectors of that model's size)
    let mut device = EmbeddingIndex::in_memory(lightcraft_vision::siglip::MODEL_ID, lightcraft_vision::siglip::DIM).unwrap();
    for (i, name) in ["red", "blue"].into_iter().enumerate() {
        let mut v = vec![0f32; lightcraft_vision::siglip::DIM];
        v[i] = 1.0;
        device.insert(Key::from_hex(&hashes[name]).unwrap(), &v).unwrap();
    }
    let (status, r) = post(&f.ann, "/api/index/embeddings", &device.export(|_| false, 100));
    assert_eq!((status, r["added"].clone(), r["skipped"].clone()), (200, json!(2), json!(0)), "{r}");
    let (_, s) = get(&f.ann, "/api/search/status");
    assert_eq!((s["indexed"].clone(), s["installed"].clone()), (json!(2), json!(false)), "{s}");
    // sending them again changes nothing
    let (_, r) = post(&f.ann, "/api/index/embeddings", &device.export(|_| false, 100));
    assert_eq!((r["added"].clone(), r["skipped"].clone()), (json!(0), json!(2)), "{r}");
}

#[test]
fn an_upload_is_checked_before_it_is_kept() {
    let f = fixture("badup", Some(Arc::new(FakeEmbedder)));
    indexed(&f, 4);
    let up = |bytes: &[u8]| post(&f.ann, "/api/index/embeddings", bytes).0;
    let mut other = EmbeddingIndex::in_memory("other-model", FAKE_DIM).unwrap();
    other.insert(Key::of("x"), &[1.0, 0.0, 0.0, 0.05]).unwrap();
    assert_eq!(up(&other.export(|_| false, 10)), 409, "another model");
    let mut mine = EmbeddingIndex::in_memory(FAKE_ID, FAKE_DIM).unwrap();
    mine.insert(Key::of("y"), &[1.0, 0.0, 0.0, 0.05]).unwrap();
    let good = mine.export(|_| false, 10);
    assert_eq!(up(&good[..good.len() - 3]), 422, "truncated");
    assert_eq!(up(b"not an index"), 422);
    assert_eq!(up(b""), 422);
    let mut nan = good.clone();
    let n = nan.len();
    nan[n - 2..].copy_from_slice(&0x7e00u16.to_le_bytes());
    assert_eq!(up(&nan), 422, "not finite");
    // the server still has exactly its four
    assert_eq!(get(&f.ann, "/api/search/status").1["indexed"], 4);
}

#[test]
fn photos_added_later_are_indexed_and_restarts_keep_the_index() {
    let f = fixture("later", Some(Arc::new(FakeEmbedder)));
    indexed(&f, 4);
    write_flat(&f.nas.join("yellow.png"), [230, 220, 20]);
    f.server.scan("ann");
    indexed(&f, 5);
    let ids = ids_by_name(&f);
    let (_, r) = get(&f.ann, "/api/search?q=yellow");
    assert_eq!(ranked(&r).first(), Some(&ids["yellow"]), "{r}");
    let data = f.data.clone();
    drop(f);

    // a new server on the same data: nothing is indexed again, search works at once
    let server = start(&data, Some(Arc::new(FakeEmbedder)));
    let ann = login(&server, "ann", "correct horse");
    assert_eq!(get(&ann, "/api/search/status").1["indexed"], 5);
    assert_eq!(get(&ann, "/api/search?q=blue").0, 200);
}
