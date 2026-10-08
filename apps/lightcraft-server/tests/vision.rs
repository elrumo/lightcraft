//! Search by description on the server end to end, over real HTTP: photos in a library folder get
//! their previews built and are indexed, a sentence finds them, a device sends vectors it computed,
//! and everything that must be refused is. The model is a deterministic stand-in (colours).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use lightcraft_server::{Config, Server, accounts};
use lightcraft_vision::fake::FakeReader;
use lightcraft_vision::fake::{FAKE_DIM, FAKE_ID, FakeEmbedder};
use lightcraft_vision::{Embedder, EmbeddingIndex, Key, TextIndex, TextReader};
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
    start_with(data, model, None)
}

fn start_with(data: &Path, model: Option<Arc<dyn Embedder>>, reader: Option<Arc<dyn TextReader>>) -> Server {
    let mut cfg = Config::new(data, "127.0.0.1:0");
    cfg.scan_interval = None;
    cfg.preview_threads = 1;
    cfg.embedder = model;
    cfg.text_reader = reader;
    // (never the machine's real model folder)
    cfg.vision_dir = Some(data.join("no-model-here"));
    Server::start(cfg).unwrap()
}

/// A server with `ann`, whose library folder holds a red, a green, a blue and a grey photo, and `bob`.
fn fixture(tag: &str, model: Option<Arc<dyn Embedder>>) -> Fixture {
    fixture_with(tag, model, None)
}

fn fixture_with(tag: &str, model: Option<Arc<dyn Embedder>>, reader: Option<Arc<dyn TextReader>>) -> Fixture {
    let root = temp(tag);
    let nas = root.join("nas");
    for (name, rgb) in [("red", [220, 20, 20]), ("green", [20, 200, 40]), ("blue", [20, 40, 220]), ("grey", [128, 128, 128])] {
        write_flat(&nas.join(format!("{name}.png")), rgb);
    }
    let data = root.join("data");
    accounts::set_user(&data, "ann", "correct horse", false).unwrap();
    accounts::set_user(&data, "bob", "battery staple", false).unwrap();
    accounts::add_folder(&data, "ann", &nas.to_string_lossy(), Some("Photos")).unwrap();
    let server = start_with(&data, model, reader);
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
    let (status, keys) = get(&f.ann, "/api/index/embeddings/keys");
    assert_eq!((status, keys["model"].as_str(), keys["dim"].as_u64()), (200, Some(FAKE_ID), Some(FAKE_DIM as u64)), "{keys}");
    assert_eq!(keys["keys"].as_array().unwrap().len(), 4);
    assert!(keys["keys"].as_array().unwrap().iter().all(|k| Key::from_hex(k.as_str().unwrap()).is_some()));

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

// ---------------------------------------------------------------- devices (the engine's side)

use lightcraft_engine::Session;
use lightcraft_vision::Error as VisionError;
use lightcraft_vision::siglip::{DIM as SIGLIP_DIM, MODEL_ID as SIGLIP_ID};

/// The colour stand-in at SigLIP 2's size and name: a device with it matches a server that has no
/// model installed (which is a SigLIP 2 server), so the two can exchange vectors.
struct Padded;

fn pad(mut v: Vec<f32>) -> Vec<f32> {
    v.resize(SIGLIP_DIM, 0.0);
    v
}

impl Embedder for Padded {
    fn model_id(&self) -> &str {
        SIGLIP_ID
    }
    fn dim(&self) -> usize {
        SIGLIP_DIM
    }
    fn encode_text(&self, t: &str) -> Result<Vec<f32>, VisionError> {
        FakeEmbedder.encode_text(t).map(pad)
    }
    fn encode_images(&self, i: &[&lightcraft_raster::Rgba8]) -> Result<Vec<Vec<f32>>, VisionError> {
        FakeEmbedder.encode_images(i).map(|v| v.into_iter().map(pad).collect())
    }
}

/// A device signed in to `f`'s server, with its library in `dir`.
fn device_in(f: &Fixture, dir: &Path, model: Option<Arc<dyn Embedder>>) -> Session {
    let mut s = Session::new().with_fs();
    s.open_library(dir, false).unwrap();
    match model {
        Some(m) => s.vision.set_embedder(m),
        // (a device with no model of its own: the web build, iOS; whatever this build could do)
        None => s.vision.thin = true,
    }
    s.execute("sync.signIn", &json!({"server": f.ann.url, "user": "ann", "password": "correct horse", "device": "test"})).unwrap();
    sync(&mut s);
    assert_eq!(s.catalog.len(), 4, "the library arrived");
    s
}

fn device(f: &Fixture, tag: &str, model: Option<Arc<dyn Embedder>>) -> Session {
    device_in(f, &temp(tag).join("device"), model)
}

fn sync(s: &mut Session) {
    let st = s.execute("sync.now", &json!({"wait": true})).unwrap();
    assert_eq!(st["state"], "idle", "{st}");
}

fn photo_named(s: &Session, name: &str) -> lightcraft_engine::catalog::PhotoId {
    s.catalog.photos().find(|p| p.file_name == format!("{name}.png")).map(|p| p.id).unwrap_or_else(|| panic!("no photo {name}"))
}

#[test]
fn a_thin_client_searches_through_the_server() {
    let f = fixture("thin", Some(Arc::new(FakeEmbedder)));
    indexed(&f, 4);
    // a device with no model of its own (the web build, iOS)
    let mut s = device(&f, "thin-device", None);
    assert!(!s.vision.available(), "nothing to search with until the server says it can");
    let e = s.execute("library.search", &json!({"q": "red", "wait": true})).unwrap_err().to_string();
    assert!(e.contains("server"), "{e}");

    // it asks, the answer comes with the next sync round, and now the switch would show
    s.vision_probe_server();
    sync(&mut s);
    assert!(s.vision.available() && s.vision.server_ready(), "{:?}", s.vision.server_status());
    let st = s.execute("vision.model.status", &json!({})).unwrap();
    assert_eq!(
        (st["localAvailable"].clone(), st["server"]["ready"].clone(), st["server"]["status"]["indexed"].clone()),
        (json!(false), json!(true), json!(4)),
        "{st}"
    );

    for (q, want) in [("red", "red"), ("a GREEN field", "green"), ("猫 blue", "blue")] {
        let r = s.execute("library.search", &json!({"q": q, "wait": true})).unwrap();
        assert_eq!(r["source"], "server", "{r}");
        let first = r["photos"][0]["id"].as_u64().unwrap();
        assert_eq!(first, photo_named(&s, want).0, "{q}: {r}");
        assert_eq!(r["photos"].as_array().unwrap().len(), 4);
        // it is the view's filter, in the server's order
        assert_eq!(s.visible_cloned().first().map(|i| i.0), Some(first));
        assert_eq!(s.filter.semantic.as_deref(), Some(q));
    }
    // this device can't search on its own
    let e = s.execute("library.search", &json!({"q": "red", "wait": true, "source": "local"})).unwrap_err().to_string();
    assert!(e.contains("server") && e.contains("model"), "{e}");
    assert!(s.execute("library.search", &json!({"q": "red", "wait": true, "source": "nowhere"})).is_err());

    // a server that can't search (no model) says why, in words
    let none = fixture("thin-none", None);
    let mut s2 = device(&none, "thin-device-2", None);
    s2.vision_probe_server();
    sync(&mut s2);
    assert!(s2.vision.available() && !s2.vision.server_ready(), "it can, once its admin installs the model");
    let e = s2.execute("library.search", &json!({"q": "red", "wait": true, "source": "server"})).unwrap_err().to_string();
    assert!(e.contains("not installed"), "{e}");
}

#[test]
fn a_desktop_sends_the_server_the_vectors_it_computed() {
    // the server has no model: it can only be given vectors
    let f = fixture("share", None);
    let mut s = device(&f, "desktop", Some(Arc::new(Padded)));
    let r = s.execute("vision.index", &json!({"wait": true})).unwrap();
    assert_eq!((r["done"].clone(), r["failed"].clone()), (json!(4), json!(0)), "{r}");
    s.vision_probe_server();
    sync(&mut s);

    // nothing leaves the device unless the user asked
    assert!(!s.vision.share_with_server);
    s.vision_poll();
    sync(&mut s);
    assert_eq!(get(&f.ann, "/api/search/status").1["indexed"], 0, "not sent by itself");

    // `vision.share` sends them
    let r = s.execute("vision.share", &json!({"wait": true})).unwrap();
    assert_eq!((r["running"].clone(), r["sent"].clone(), r["error"].clone()), (json!(false), json!(4), Value::Null), "{r}");
    let (_, st) = get(&f.ann, "/api/search/status");
    assert_eq!((st["indexed"].clone(), st["installed"].clone()), (json!(4), json!(false)), "{st}");
    // and only what the server lacks
    let r = s.execute("vision.share", &json!({"wait": true})).unwrap();
    assert_eq!(r["sent"], 0, "{r}");

    // a device whose model isn't the server's is refused before anything is sent
    let mut s3 = device(&f, "desktop-3", Some(Arc::new(FakeEmbedder)));
    s3.execute("vision.index", &json!({"wait": true})).unwrap();
    let r = s3.execute("vision.share", &json!({"wait": true})).unwrap();
    assert_eq!((r["running"].clone(), r["sent"].clone()), (json!(false), json!(0)), "{r}");
    assert!(r["error"].as_str().unwrap().contains("nothing was sent"), "{r}");
    assert_eq!(get(&f.ann, "/api/search/status").1["indexed"], 4);

    // no photos indexed: nothing to send
    let mut s4 = device(&f, "desktop-4", Some(Arc::new(Padded)));
    assert!(s4.execute("vision.share", &json!({"wait": true})).is_err());
}

#[test]
fn a_device_that_turned_sharing_on_sends_by_itself_and_remembers_the_choice() {
    let f = fixture("auto-share", None);
    let dir = temp("auto-share-device").join("device");
    let mut s = device_in(&f, &dir, Some(Arc::new(Padded)));
    s.execute("vision.setShare", &json!({"on": true})).unwrap();
    s.execute("vision.index", &json!({"wait": true})).unwrap();
    s.vision_probe_server();
    sync(&mut s);
    // the frame loop's upkeep notices the new vectors and queues the send; the sync round runs it
    s.vision_poll();
    sync(&mut s);
    assert!(!s.vision.sharing(), "{:?}", s.vision.share_error());
    assert_eq!(get(&f.ann, "/api/search/status").1["indexed"], 4, "sent without being asked each time");
    assert_eq!(s.execute("vision.model.status", &json!({})).unwrap()["share"]["enabled"], true);

    // the choice is saved with the library
    s.close_library().unwrap();
    drop(s);
    let mut again = Session::new().with_fs();
    again.open_library(&dir, false).unwrap();
    assert!(again.vision.share_with_server);
    again.execute("vision.setShare", &json!({"on": false})).unwrap();
    assert!(!again.vision.share_with_server);
}

// ---------------------------------------------------------------- the text printed in photos

/// Wait until the text of `want` of `ann`'s photos has been read (or sent).
fn read(f: &Fixture, want: u64) -> Value {
    for _ in 0..1200 {
        f.server.index_for_search();
        let (_, s) = get(&f.ann, "/api/search/status");
        if s["text"]["indexed"].as_u64() == Some(want) {
            return s;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("text not read: {:?}", get(&f.ann, "/api/search/status"));
}

fn scores(r: &Value) -> Vec<f64> {
    r["scores"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect()
}

/// The stand-in reader under the real reader's name: a server without a reader of its own expects
/// that engine.
struct Ocrish;

impl TextReader for Ocrish {
    fn engine(&self) -> &str {
        lightcraft_vision::ocr::ENGINE
    }
    fn text(&self, img: &lightcraft_raster::Rgba8) -> Result<String, VisionError> {
        FakeReader.text(img)
    }
}

#[test]
fn words_printed_in_photos_are_read_and_found_first() {
    let f = fixture_with("words", Some(Arc::new(FakeEmbedder)), Some(Arc::new(FakeReader)));
    let s = read(&f, 4);
    indexed(&f, 4);
    assert_eq!((s["text"]["installed"].clone(), s["text"]["engine"].as_str()), (json!(true), Some("fake-reader")), "{s}");
    let ids = ids_by_name(&f);

    // "stop" is printed on the red photo only: it is first, as a text match
    let (status, r) = get(&f.ann, "/api/search?q=stop");
    assert_eq!(status, 200, "{r}");
    let (found, sc) = (ranked(&r), scores(&r));
    assert_eq!(found[0], ids["red"], "{r}");
    assert!(sc[0] >= 2.0 && sc[1..].iter().all(|s| *s < 2.0), "only the first is a text match: {sc:?}");
    assert_eq!(found.len(), 4, "the others follow, by how they look");

    // case, and one slip in a long word
    let (_, r) = get(&f.ann, "/api/search?q=HARBOR%20hotel");
    assert_eq!(ranked(&r)[0], ids["blue"], "{r}");
    // the word and the colour agree: nothing is listed twice
    let (_, r) = get(&f.ann, "/api/search?q=stop%20red");
    let found = ranked(&r);
    assert_eq!(found.len(), 4, "{r}");
    assert_eq!(found.iter().collect::<std::collections::BTreeSet<_>>().len(), 4);

    // words nobody printed: only look-alikes
    let (_, r) = get(&f.ann, "/api/search?q=xylophone");
    assert!(scores(&r).iter().all(|s| *s < 2.0), "{r}");
    // the text is on disk, filed by the user and the reader
    assert!(f.data.join("users/ann/search/text-fake-reader.bin").is_file());
}

#[test]
fn a_server_that_only_reads_text_still_answers() {
    let f = fixture_with("text-only", None, Some(Arc::new(FakeReader)));
    let s = read(&f, 4);
    assert_eq!((s["installed"].clone(), s["text"]["installed"].clone()), (json!(false), json!(true)), "{s}");
    let ids = ids_by_name(&f);
    let (status, r) = get(&f.ann, "/api/search?q=stop");
    assert_eq!((status, ranked(&r)), (200, vec![ids["red"]]), "{r}");
    // no words found and nothing that could look like it: an empty answer, not an error
    let (status, r) = get(&f.ann, "/api/search?q=xylophone");
    assert_eq!((status, r["ids"].clone()), (200, json!([])), "{r}");
}

#[test]
fn a_device_sends_the_text_it_read_and_the_server_checks_it() {
    // a server with no models at all: nothing to read with, but it keeps what a device sends
    let f = fixture("text-upload", None);
    let (_, snap) = get(&f.ann, "/api/snapshot");
    let hashes: HashMap<String, String> = snap["catalog"]["photos"]
        .as_object()
        .unwrap()
        .values()
        .map(|p| (p["file_name"].as_str().unwrap().trim_end_matches(".png").to_string(), p["content_hash"].as_str().unwrap().to_string()))
        .collect();
    let ids = ids_by_name(&f);
    let engine = lightcraft_vision::ocr::ENGINE;
    let (_, s) = get(&f.ann, "/api/search/status");
    assert_eq!((s["text"]["installed"].clone(), s["text"]["indexed"].clone()), (json!(false), json!(0)), "{s}");
    assert!(!f.data.join("users/ann/search").join(format!("text-{engine}.bin")).exists(), "asking creates nothing");
    let (status, k) = get(&f.ann, "/api/index/text/keys");
    assert_eq!((status, k["engine"].as_str(), k["keys"].clone()), (200, Some(engine), json!([])), "{k}");

    let mut device = TextIndex::in_memory(engine).unwrap();
    device.insert(Key::from_hex(&hashes["grey"]).unwrap(), "Pharmacy open late").unwrap();
    device.insert(Key::of("not-in-the-library"), "secret").unwrap();
    let good = device.export(|_| false, 100);
    let (status, r) = post(&f.ann, "/api/index/text", &good);
    assert_eq!((status, r["added"].clone(), r["skipped"].clone()), (200, json!(1), json!(1)), "{r}");
    let (_, k) = get(&f.ann, "/api/index/text/keys");
    assert_eq!(k["keys"].as_array().unwrap().len(), 1, "{k}");
    // the server finds it by the words, with no model of its own
    let (status, r) = get(&f.ann, "/api/search?q=PHARMACY");
    assert_eq!((status, ranked(&r)), (200, vec![ids["grey"]]), "{r}");
    // sending again changes nothing
    let (_, r) = post(&f.ann, "/api/index/text", &good);
    assert_eq!((r["added"].clone(), r["skipped"].clone()), (json!(0), json!(2)), "{r}");

    // refused whole: another reader's, cut short, damaged, empty
    let mut other = TextIndex::in_memory("other-reader").unwrap();
    other.insert(Key::of("x"), "text").unwrap();
    let up = |bytes: &[u8]| post(&f.ann, "/api/index/text", bytes).0;
    assert_eq!(up(&other.export(|_| false, 10)), 409, "another reader");
    assert_eq!(up(&good[..good.len() - 3]), 422, "truncated");
    assert_eq!(up(b"not an index"), 422);
    assert_eq!(up(b""), 422);
    assert_eq!(get(&f.ann, "/api/search/status").1["text"]["indexed"], 1);
    // and nobody else sees it
    let bob = login(&f.server, "bob", "battery staple");
    // (bob has no text and the server no models: nothing to answer with)
    let (status, r) = get(&bob, "/api/search?q=pharmacy");
    assert!(status == 503 && r["ids"].is_null(), "{status} {r}");
    assert_eq!(get(&bob, "/api/index/text/keys").1["keys"], json!([]));
}

#[test]
fn a_desktop_sends_the_server_the_text_it_read() {
    let f = fixture("text-share", None);
    let mut s = device(&f, "text-desktop", None);
    s.vision.set_text_reader(Arc::new(Ocrish));
    s.vision.text_edge = 192;
    // text search is off until it is turned on; nothing to send before it has been read
    s.execute("vision.setText", &json!({"on": true})).unwrap();
    assert!(s.execute("vision.share", &json!({"wait": true})).is_err(), "nothing read yet");
    let r = s.execute("vision.index", &json!({"wait": true})).unwrap();
    assert_eq!((r["done"].clone(), r["failed"].clone(), r["error"].clone()), (json!(4), json!(0), Value::Null), "{r}");
    s.vision_probe_server();
    sync(&mut s);

    let r = s.execute("vision.share", &json!({"wait": true})).unwrap();
    assert_eq!((r["running"].clone(), r["sent"].clone(), r["error"].clone()), (json!(false), json!(4), Value::Null), "{r}");
    let (_, st) = get(&f.ann, "/api/search/status");
    assert_eq!(st["text"]["indexed"], 4, "{st}");
    // and only what the server lacks
    let r = s.execute("vision.share", &json!({"wait": true})).unwrap();
    assert_eq!(r["sent"], 0, "{r}");

    // another device finds the photo by its words through the server, with no reader of its own
    let mut thin = device(&f, "text-thin", None);
    thin.vision_probe_server();
    sync(&mut thin);
    assert!(thin.vision.server_ready(), "a server that has text can search: {:?}", thin.vision.server_status());
    let r = thin.execute("library.search", &json!({"q": "stop", "wait": true})).unwrap();
    assert_eq!(r["source"], "server", "{r}");
    assert_eq!(r["photos"][0]["id"].as_u64(), Some(photo_named(&thin, "red").0), "{r}");
    assert_eq!(r["photos"][0]["text"], true, "{r}");

    // a reader the server doesn't use is refused before anything is sent
    let mut odd = device(&f, "text-odd", None);
    odd.vision.set_text_reader(Arc::new(FakeReader));
    odd.vision.text_edge = 192;
    odd.execute("vision.setText", &json!({"on": true})).unwrap();
    odd.execute("vision.index", &json!({"wait": true})).unwrap();
    let r = odd.execute("vision.share", &json!({"wait": true})).unwrap();
    assert!(r["error"].as_str().is_some_and(|e| e.contains("nothing was sent")), "{r}");
}

// ---------------------------------------------------------------- the people in photos

/// Wait until the server has looked at `want` of `ann`'s photos for faces.
fn looked(f: &Fixture, want: u64) -> Value {
    for _ in 0..1200 {
        f.server.index_for_search();
        let (_, s) = get(&f.ann, "/api/people/status");
        if s["photos"].as_u64() == Some(want) {
            return s;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("faces not found: {:?}", get(&f.ann, "/api/people/status"));
}

fn delete(w: &Who, path: &str) -> (u16, Value) {
    let mut r = agent().delete(format!("{}{path}", w.url)).header("Authorization", format!("Bearer {}", w.token)).call().unwrap();
    let status = r.status().as_u16();
    (status, json_body(&mut r))
}

fn fixture_full(
    tag: &str,
    model: Option<Arc<dyn Embedder>>,
    reader: Option<Arc<dyn TextReader>>,
    finder: Option<Arc<dyn lightcraft_vision::FaceEngine>>,
) -> Fixture {
    let root = temp(tag);
    let nas = root.join("nas");
    for (name, rgb) in [("red", [220, 20, 20]), ("green", [20, 200, 40]), ("blue", [20, 40, 220]), ("grey", [128, 128, 128])] {
        write_flat(&nas.join(format!("{name}.png")), rgb);
    }
    let data = root.join("data");
    accounts::set_user(&data, "ann", "correct horse", false).unwrap();
    accounts::set_user(&data, "bob", "battery staple", false).unwrap();
    accounts::add_folder(&data, "ann", &nas.to_string_lossy(), Some("Photos")).unwrap();
    let mut cfg = Config::new(&data, "127.0.0.1:0");
    cfg.scan_interval = None;
    cfg.preview_threads = 1;
    cfg.embedder = model;
    cfg.text_reader = reader;
    cfg.face_finder = finder;
    cfg.vision_dir = Some(data.join("no-model-here"));
    let server = Server::start(cfg).unwrap();
    let ann = login(&server, "ann", "correct horse");
    let f = Fixture { server, ann, data, nas };
    wait_scan(&f);
    f
}

/// The stand-in finder under the real models' name (a server without a finder of its own expects it).
struct Yunetish;

impl lightcraft_vision::FaceEngine for Yunetish {
    fn engine(&self) -> &str {
        lightcraft_vision::faces::ENGINE
    }
    fn faces(&self, img: &lightcraft_raster::Rgba8) -> Result<Vec<lightcraft_vision::faces::FaceFound>, VisionError> {
        lightcraft_vision::fake::FakeFaces.faces(img)
    }
}

#[test]
fn faces_are_found_only_for_a_user_an_admin_turned_them_on() {
    let f = fixture_full("faces-flag", None, None, Some(Arc::new(lightcraft_vision::fake::FakeFaces)));
    let (status, s) = get(&f.ann, "/api/people/status");
    assert_eq!((status, s["enabled"].clone(), s["installed"].clone(), s["photos"].clone()), (200, json!(false), json!(true), json!(0)), "{s}");
    // off: nothing is looked at, nothing is served, and the user can still delete
    f.server.index_for_search();
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(get(&f.ann, "/api/people/status").1["photos"], 0, "no faces were looked for");
    for path in ["/api/people/clusters", "/api/index/faces/keys"] {
        let (status, r) = get(&f.ann, path);
        assert_eq!(status, 403, "{path}: {r}");
        assert!(r["error"].as_str().unwrap().contains("user faces"), "{r}");
    }
    assert_eq!(post(&f.ann, "/api/index/faces", b"x").0, 403);
    assert_eq!(delete(&f.ann, "/api/index/faces").0, 200);
    // the search status says it too (that is what devices ask)
    assert_eq!(get(&f.ann, "/api/search/status").1["faces"]["enabled"], false);

    // an admin turns it on for ann only
    accounts::set_faces(&f.data, "ann", true).unwrap();
    let s = looked(&f, 4);
    assert_eq!((s["enabled"].clone(), s["engine"].as_str()), (json!(true), Some("fake-faces")), "{s}");
    assert_eq!(get(&f.ann, "/api/search/status").1["faces"]["enabled"], true);
    let (status, c) = get(&f.ann, "/api/people/clusters");
    assert_eq!(status, 200, "{c}");
    let clusters = c["clusters"].as_array().unwrap();
    let members: usize = clusters.iter().map(|c| c["members"].as_array().unwrap().len()).sum();
    assert_eq!((members as u64, c["faces"].as_u64()), (c["faces"].as_u64().unwrap(), Some(8)), "two faces in each of four photos: {c}");
    assert_eq!(clusters.len(), 3, "red, green, and blue with grey: {c}");
    let m = &clusters[0]["members"][0];
    assert!(m[0].as_u64().is_some() && m[5].as_f64().unwrap() > m[3].as_f64().unwrap(), "photo, face, box, score: {m}");
    assert_eq!(get(&f.ann, "/api/index/faces/keys").1["keys"].as_array().unwrap().len(), 4);
    assert!(f.data.join("users/ann/search/faces-fake-faces.bin").is_file());

    // bob's is off, and his own
    let bob = login(&f.server, "bob", "battery staple");
    assert_eq!(get(&bob, "/api/people/clusters").0, 403);
    assert_eq!(get(&bob, "/api/people/status").1["photos"], 0);

    // forgetting empties it (the flag is off first, or the sweep would look again)
    accounts::set_faces(&f.data, "ann", false).unwrap();
    let (status, r) = delete(&f.ann, "/api/index/faces");
    assert_eq!((status, r["photos"].clone()), (200, json!(4)), "{r}");
    f.server.index_for_search();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(get(&f.ann, "/api/people/status").1["photos"], 0);
}

#[test]
fn a_device_sends_the_faces_it_found_and_the_server_checks_them() {
    // a server with no face models: it keeps what a device sends (for a user it may find people for)
    let f = fixture_full("faces-upload", None, None, None);
    accounts::set_faces(&f.data, "ann", true).unwrap();
    let mut s = device(&f, "faces-desktop", None);
    s.vision.set_face_finder(Arc::new(Yunetish));
    s.vision.text_edge = 192;
    s.execute("vision.setFaces", &json!({"on": true})).unwrap();
    assert!(s.execute("vision.share", &json!({"wait": true})).is_err(), "nothing found yet");
    let r = s.execute("vision.index", &json!({"wait": true})).unwrap();
    assert_eq!((r["done"].clone(), r["failed"].clone(), r["error"].clone()), (json!(4), json!(0), Value::Null), "{r}");
    s.vision_probe_server();
    sync(&mut s);
    assert!(s.vision.server_faces(), "the server says it finds people for this user: {:?}", s.vision.server_status());

    let r = s.execute("vision.share", &json!({"wait": true})).unwrap();
    assert_eq!((r["running"].clone(), r["sent"].clone(), r["error"].clone()), (json!(false), json!(4), Value::Null), "{r}");
    assert_eq!(get(&f.ann, "/api/people/status").1["photos"], 4);
    // and only what the server lacks
    assert_eq!(s.execute("vision.share", &json!({"wait": true})).unwrap()["sent"], 0);

    // refused whole: other models, cut short, damaged, empty
    use lightcraft_vision::faces::index::FaceIndex;
    let mut other = FaceIndex::in_memory();
    other.insert_photo(Key::of("x"), &[]).unwrap();
    let good = {
        let mut ix = FaceIndex::in_memory();
        let e: Vec<f32> = (0..128).map(|i| if i == 0 { 1.0 } else { 0.0 }).collect();
        ix.insert_photo(Key::of("y"), &[lightcraft_vision::faces::FaceFound { rect: [0.1, 0.1, 0.5, 0.5], score: 0.9, embedding: e }]).unwrap();
        ix.export(|_| false, 10)
    };
    let up = |bytes: &[u8]| post(&f.ann, "/api/index/faces", bytes).0;
    assert_eq!(up(&good[..good.len() - 3]), 422, "truncated");
    assert_eq!(up(b"not an index"), 422);
    assert_eq!(up(b""), 422);
    let mut wrong = good.clone();
    wrong[16] = b'Z';
    assert_eq!(up(&wrong), 409, "another pair of models");
    // a photo that is not in the library is skipped, never kept
    let (_, r) = post(&f.ann, "/api/index/faces", &good);
    assert_eq!((r["added"].clone(), r["skipped"].clone()), (json!(0), json!(1)), "{r}");
    assert_eq!(get(&f.ann, "/api/people/status").1["photos"], 4);
}

#[test]
fn a_thin_client_lists_and_names_the_servers_people_and_can_make_it_forget() {
    let f = fixture_full("faces-thin", None, None, Some(Arc::new(lightcraft_vision::fake::FakeFaces)));
    accounts::set_faces(&f.data, "ann", true).unwrap();
    looked(&f, 4);

    // a device with no face models of its own (the web build, iOS)
    let mut s = device(&f, "faces-thin-device", None);
    s.vision_probe_server();
    sync(&mut s);
    assert!(s.vision.server_faces());
    let r = s.execute("people.list", &json!({"wait": true})).unwrap();
    assert_eq!(r["source"], "server", "{r}");
    let people = r["people"].as_array().unwrap();
    assert_eq!(people.len(), 3, "{r}");
    assert!(people.iter().all(|p| p["name"].is_null() && p["faces"].as_u64().unwrap() >= 2));
    // the faces are of photos this device has
    let ours: std::collections::HashSet<u64> = s.catalog.photos().map(|p| p.id.0).collect();
    assert!(people.iter().all(|p| ours.contains(&p["cover"]["photo"].as_u64().unwrap())));

    // naming is an ordinary catalog edit, made here
    let red = people.iter().find(|p| p["photoIds"].as_array().unwrap().iter().any(|i| i.as_u64() == Some(photo_named(&s, "red").0))).unwrap();
    let named = s.execute("people.name", &json!({"cluster": red["id"], "name": "Ada"})).unwrap();
    assert_eq!((named["faces"].clone(), named["photos"].clone()), (json!(2), json!(1)), "{named}");
    assert!(s.catalog.people().iter().any(|p| p.name == "Ada"));
    let shown = s.execute("people.show", &json!({"cluster": red["id"]})).unwrap();
    assert_eq!(shown["person"], "Ada");

    // and the name reaches the server's catalog with the next sync
    sync(&mut s);
    let (_, snap) = get(&f.ann, "/api/snapshot");
    let has_ada = snap["catalog"]["photos"].as_object().unwrap().values().any(|p| p["meta"]["regions"].to_string().contains("Ada"));
    assert!(has_ada, "the server's catalog has the region");

    // forgetting reaches the server too
    let r = s.execute("people.deleteData", &json!({"wait": true})).unwrap();
    assert_eq!(r["server"], true, "{r}");
    accounts::set_faces(&f.data, "ann", false).unwrap();
    assert_eq!(get(&f.ann, "/api/people/status").1["photos"], 0, "the server forgot");
}
