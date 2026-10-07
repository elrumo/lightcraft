//! The server end to end: real HTTP on a loopback port, two LightCraft libraries syncing through
//! it with the engine's own transport, and the API's refusals.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use lightcraft_engine::Session;
use lightcraft_engine::catalog::{PhotoId, Source};
use lightcraft_server::{Config, Server, accounts, gc};
use serde_json::{Value, json};

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-server-e2e-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write_png(path: &Path, seed: u8) {
    let (w, h) = (80usize, 60usize);
    let data: Vec<[u8; 4]> = (0..w * h).map(|i| [(i % w * 3) as u8, (i / w * 4) as u8, seed, 255]).collect();
    let img = lightcraft_raster::Rgba8 { width: w, height: h, data };
    let bytes = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn start(root: &Path) -> Server {
    let data = root.join("data");
    accounts::set_user(&data, "ann", "correct horse", false).unwrap();
    let web = root.join("web");
    std::fs::create_dir_all(&web).unwrap();
    std::fs::write(web.join("index.html"), "<!doctype html>web build").unwrap();
    Server::start(Config { data, listen: "127.0.0.1:0".into(), web: Some(web), max_requests: 32 }).unwrap()
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder().http_status_as_error(false).build().into()
}

fn call(method: &str, url: &str, token: &str, body: Option<&[u8]>, headers: &[(&str, &str)]) -> (u16, Vec<u8>, Vec<(String, String)>) {
    let a = agent();
    let auth = format!("Bearer {token}");
    let r = match method {
        "GET" => {
            let mut req = a.get(url).header("Authorization", &auth);
            for (k, v) in headers {
                req = req.header(*k, *v);
            }
            req.call()
        }
        "HEAD" => a.head(url).header("Authorization", &auth).call(),
        "PUT" => a.put(url).header("Authorization", &auth).send(body.unwrap_or(&[])),
        _ => a.post(url).header("Authorization", &auth).send(body.unwrap_or(&[])),
    };
    let mut r = r.unwrap();
    let hs = r.headers().iter().map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string())).collect();
    let status = r.status().as_u16();
    let body = r.body_mut().read_to_vec().unwrap_or_default();
    (status, body, hs)
}

fn json_of(b: &[u8]) -> Value {
    serde_json::from_slice(b).unwrap_or(Value::Null)
}

fn open(dir: &Path) -> Session {
    let mut s = Session::new().with_fs();
    s.open_library(dir, false).unwrap();
    s
}

fn sign_in(s: &mut Session, url: &str) {
    s.execute("sync.signIn", &json!({"server": url, "user": "ann", "password": "correct horse", "device": "test"})).unwrap();
}

#[test]
fn api_refuses_what_it_should() {
    let root = temp("api");
    let server = start(&root);
    let base = format!("http://{}", server.addr());
    // the web build, cross-origin isolated; no way out of its folder
    let (s, body, hs) = call("GET", &format!("{base}/"), "", None, &[]);
    assert_eq!((s, String::from_utf8_lossy(&body).contains("web build")), (200, true));
    assert!(hs.iter().any(|(k, v)| k.eq_ignore_ascii_case("cross-origin-opener-policy") && v == "same-origin"));
    assert_eq!(call("GET", &format!("{base}/../data/users.json"), "", None, &[]).0, 404);
    assert_eq!(call("GET", &format!("{base}/%2e%2e/data/users.json"), "", None, &[]).0, 404);
    // nothing without signing in
    assert_eq!(call("GET", &format!("{base}/api/snapshot"), "", None, &[]).0, 401);
    assert_eq!(call("GET", &format!("{base}/api/snapshot"), "made-up", None, &[]).0, 401);
    let login = |pw: &str| {
        call("POST", &format!("{base}/api/login"), "", Some(json!({"user": "ann", "password": pw, "device": "t"}).to_string().as_bytes()), &[])
    };
    assert_eq!(login("nope").0, 401);
    assert_eq!(call("POST", &format!("{base}/api/login"), "", Some(b"{garbage"), &[]).0, 400);
    let (s, body, _) = login("correct horse");
    assert_eq!(s, 200);
    let dev = json_of(&body);
    let token = dev["token"].as_str().unwrap().to_string();
    assert_eq!(dev["space"], 1);
    // photo files: the hash must be one, and an original must be what it names
    let png = root.join("x.png");
    write_png(&png, 7);
    let bytes = std::fs::read(&png).unwrap();
    // (the hash an import gives a file)
    let h = lightcraft_preview::hash_bytes(&bytes).to_string();
    assert_eq!(call("PUT", &format!("{base}/api/blobs/original/..%2f..%2fusers.json"), &token, Some(&bytes), &[]).0, 400);
    assert_eq!(call("PUT", &format!("{base}/api/blobs/original/{}", "0".repeat(32)), &token, Some(&bytes), &[]).0, 422);
    assert_eq!(call("PUT", &format!("{base}/api/blobs/mini/{h}"), &token, Some(b"not a preview"), &[]).0, 422);
    assert_eq!(call("HEAD", &format!("{base}/api/blobs/original/{h}"), &token, None, &[]).0, 404);
    assert_eq!(call("PUT", &format!("{base}/api/blobs/original/{h}"), &token, Some(&bytes), &[]).0, 200);
    assert_eq!(call("HEAD", &format!("{base}/api/blobs/original/{h}"), &token, None, &[]).0, 200);
    let (s, part, hs) = call("GET", &format!("{base}/api/blobs/original/{h}"), &token, None, &[("Range", "bytes=4-11")]);
    assert_eq!((s, part.as_slice()), (206, &bytes[4..12]));
    assert!(hs.iter().any(|(k, v)| k.eq_ignore_ascii_case("content-range") && v == &format!("bytes 4-11/{}", bytes.len())));
    assert_eq!(call("GET", &format!("{base}/api/blobs/original/{h}"), &token, None, &[("Range", "bytes=999999-")]).0, 416);
    let (s, all, _) = call("GET", &format!("{base}/api/blobs/original/{h}"), &token, None, &[]);
    assert_eq!((s, all), (200, bytes.clone()));
    // pushes are validated: device-local ops and garbage are refused, nothing applied
    let push = |body: Value| call("POST", &format!("{base}/api/ops"), &token, Some(body.to_string().as_bytes()), &[]);
    let local = lightcraft_engine::catalog::Op::SetBrowsed { folder: "/x".into(), at: None };
    let (s, body, _) = push(json!({"base": 0, "ops": [local]}));
    assert_eq!((s, json_of(&body)["index"].clone()), (422, json!(0)));
    assert_eq!(push(json!({"base": 5, "ops": []})).0, 409);
    assert_eq!(push(json!({"nope": 1})).0, 400);
    let (s, body, _) = call("GET", &format!("{base}/api/ops?since=0&limit=99999999999"), &token, None, &[]);
    assert_eq!((s, json_of(&body)["head"].clone()), (200, json!(0)));
    // presets: a stale version is refused with the current one
    let put = |v: Value| call("PUT", &format!("{base}/api/presets"), &token, Some(v.to_string().as_bytes()), &[]);
    assert_eq!(put(json!({"version": 0, "presets": [{"id": "p1"}]})).0, 200);
    let (s, body, _) = put(json!({"version": 0, "presets": []}));
    assert_eq!((s, json_of(&body)["version"].clone()), (412, json!(1)));
    // signing out ends the token
    assert_eq!(call("POST", &format!("{base}/api/logout"), &token, None, &[]).0, 200);
    assert_eq!(call("GET", &format!("{base}/api/me"), &token, None, &[]).0, 401);
    drop(server);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn two_libraries_sync_through_the_server() {
    let root = temp("sync");
    let server = start(&root);
    let url = format!("http://{}", server.addr());
    write_png(&root.join("in/a.png"), 1);
    write_png(&root.join("in/b.png"), 2);
    let mut a = open(&root.join("a"));
    a.execute("library.import", &json!({"paths": [root.join("in").to_string_lossy()]})).unwrap();
    let ids: Vec<PhotoId> = a.catalog.photos().map(|p| p.id).collect();
    sign_in(&mut a, &url);
    let st = a.execute("sync.now", &json!({"wait": true})).unwrap();
    assert_eq!(st["state"], "idle", "{st}");

    // every photo's three files are on the server
    let blobs = root.join("data/users/ann/blobs");
    for kind in ["original", "smart", "mini"] {
        let n: usize = std::fs::read_dir(blobs.join(kind)).unwrap().flatten().map(|d| std::fs::read_dir(d.path()).unwrap().count()).sum();
        assert_eq!(n, 2, "{kind}");
    }

    let mut b = open(&root.join("b"));
    sign_in(&mut b, &url);
    b.execute("sync.now", &json!({"wait": true})).unwrap();
    assert_eq!(b.catalog.len(), 2);
    assert!(b.catalog.photos().all(|p| matches!(&p.source, Source::File { path } if path.starts_with("web/"))));
    assert!(b.render_now(ids[0], 200, 200).is_ok(), "renders from the downloaded previews");

    // edits on both, merged
    a.selection = lightcraft_engine::Selection::single(ids[0]);
    a.execute("develop.set", &json!({"control": "light.exposure", "value": 0.5})).unwrap();
    b.selection = lightcraft_engine::Selection::single(ids[0]);
    b.execute("develop.set", &json!({"control": "light.contrast", "value": -20})).unwrap();
    b.execute("photo.rate", &json!({"rating": 2})).unwrap();
    for s in [&mut a, &mut b] {
        s.execute("sync.now", &json!({"wait": true})).unwrap();
    }
    a.execute("sync.now", &json!({"wait": true})).unwrap();
    for s in [&a, &b] {
        let p = s.catalog.photo(ids[0]).unwrap();
        assert_eq!((p.develop.light.exposure, p.develop.light.contrast, p.rating), (0.5, -20.0, 2));
    }

    // gc keeps everything referenced, and new files whatever they are
    let data = root.join("data");
    assert_eq!(gc::run(&data, false).unwrap().removed, 0);
    let stray = blobs.join("mini/ff").join("ff".repeat(16));
    std::fs::create_dir_all(stray.parent().unwrap()).unwrap();
    std::fs::write(&stray, b"LCSP1\nx").unwrap();
    let old = SystemTime::now() - Duration::from_secs(3 * 24 * 3600);
    std::fs::File::options().write(true).open(&stray).unwrap().set_modified(old).unwrap();
    let kept = blobs.join("original").read_dir().unwrap().flatten().next().unwrap().path().read_dir().unwrap().flatten().next().unwrap().path();
    std::fs::File::options().write(true).open(&kept).unwrap().set_modified(old).unwrap();
    let r = gc::run(&data, true).unwrap();
    assert_eq!(r.removed, 1);
    assert!(stray.exists(), "dry run");
    assert_eq!(gc::run(&data, false).unwrap().removed, 1);
    assert!(!stray.exists() && kept.exists());
    drop(server);
    let _ = std::fs::remove_dir_all(&root);
}
