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
    let mut cfg = Config::new(data, "127.0.0.1:0");
    cfg.web = Some(web);
    cfg.scan_interval = None;
    cfg.preview_threads = 1;
    Server::start(cfg).unwrap()
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
        "DELETE" => a.delete(url).header("Authorization", &auth).call(),
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
fn storage_is_reported_per_user_and_matches_the_disk() {
    let root = temp("usage");
    let server = start(&root);
    let url = format!("http://{}", server.addr());
    assert_eq!(call("GET", &format!("{url}/api/usage"), "", None, &[]).0, 401, "only for signed-in devices");
    write_png(&root.join("in/a.png"), 1);
    write_png(&root.join("in/b.png"), 2);
    let mut a = open(&root.join("a"));
    a.execute("library.import", &json!({"paths": [root.join("in").to_string_lossy()]})).unwrap();
    sign_in(&mut a, &url);
    a.execute("sync.now", &json!({"wait": true})).unwrap();

    // what the server says is what its blobs folder holds
    let blobs = root.join("data/users/ann/blobs");
    let on_disk = |kind: &str| -> (u64, u64) {
        let mut found = (0, 0);
        for prefix in std::fs::read_dir(blobs.join(kind)).unwrap().flatten() {
            for f in std::fs::read_dir(prefix.path()).unwrap().flatten() {
                found = (found.0 + 1, found.1 + f.metadata().unwrap().len());
            }
        }
        found
    };
    let r = a.execute("sync.usage", &json!({"refresh": true})).unwrap();
    for kind in ["original", "smart", "mini"] {
        let (files, bytes) = on_disk(kind);
        assert_eq!((r["server"][kind]["files"].as_u64(), r["server"][kind]["bytes"].as_u64()), (Some(files), Some(bytes)), "{kind}: {r}");
        assert_eq!(files, 2, "{kind}");
    }
    assert_eq!((r["server"]["photos"].clone(), r["server"]["devices"].clone()), (json!(2), json!(1)), "{r}");
    assert_eq!(r["serverError"], Value::Null, "{r}");
    assert!(r["server"]["disk"]["total"].as_u64().unwrap_or(0) > 0, "the server's disk is known: {r}");
    // this computer: the library folder and its previews are measured too
    assert!(r["local"]["library"].as_u64().unwrap() > 0 && r["local"]["smart"]["files"].as_u64().unwrap() > 0, "{r}");
    // a second user's files don't count
    accounts::set_user(&root.join("data"), "bob", "another horse", false).unwrap();
    let (s, body, _) = call("GET", &format!("{url}/api/usage"), a.sync_state().map(|st| st.config.token.clone()).unwrap().as_str(), None, &[]);
    assert_eq!((s, json_of(&body)["original"]["files"].clone()), (200, json!(2)));
    drop(server);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_servers_work_is_reported_to_signed_in_devices_and_is_idle_when_there_is_none() {
    let root = temp("activity");
    let server = start(&root);
    let url = format!("http://{}", server.addr());
    assert_eq!(call("GET", &format!("{url}/api/activity"), "", None, &[]).0, 401, "only for signed-in devices");
    write_png(&root.join("in/a.png"), 1);
    write_png(&root.join("in/b.png"), 2);
    let mut a = open(&root.join("a"));
    a.execute("library.import", &json!({"paths": [root.join("in").to_string_lossy()]})).unwrap();
    sign_in(&mut a, &url);
    // the server builds the previews of what this device uploads (as a phone asks it to)
    a.execute("sync.serverPreviews", &json!({"on": true})).unwrap();
    a.execute("sync.now", &json!({"wait": true})).unwrap();
    let token = a.sync_state().map(|st| st.config.token.clone()).unwrap();
    // the previews are built by the server's own threads: it is idle once they are
    let mut seen = json!(null);
    for _ in 0..400 {
        let (s, body, _) = call("GET", &format!("{url}/api/activity"), &token, None, &[]);
        assert_eq!(s, 200);
        seen = json_of(&body);
        if seen["previews"]["total"] == 0 && root.join("data/users/ann/blobs/mini").exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    assert_eq!(seen["previews"]["total"], 0, "idle: {seen}");
    // no library folders and no search models here: only what is true is said
    for field in ["scan", "search", "text", "faces"] {
        assert_eq!(seen[field], Value::Null, "{field}: {seen}");
    }
    // the same through the command a device (or an agent) uses
    let r = a.execute("sync.activity", &json!({"refresh": true})).unwrap();
    assert_eq!(r["server"]["previews"]["total"], 0, "{r}");
    assert_eq!(r["serverError"], Value::Null, "{r}");
    assert_eq!(r["transfers"]["uploads"], 0, "{r}");
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

#[test]
fn admin_page_sets_up_and_manages_users() {
    let root = temp("admin");
    let server = Server::start(Config::new(root.join("data"), "127.0.0.1:0")).unwrap();
    let base = format!("http://{}", server.addr());
    let req = |m: &str, path: &str, token: &str, body: Option<Value>| {
        let (s, b, h) = call(m, &format!("{base}{path}"), token, body.map(|v| v.to_string()).as_deref().map(str::as_bytes), &[]);
        (s, json_of(&b), h)
    };
    // the page, locked down
    let (s, _, h) = req("GET", "/admin", "", None);
    assert_eq!(s, 200);
    assert!(
        h.iter().any(|(k, v)| k.eq_ignore_ascii_case("content-security-policy")
            && v.contains("script-src 'self'")
            && v.contains("frame-ancestors 'none'"))
    );
    assert_eq!(req("GET", "/admin/admin.js", "", None).0, 200);
    let (s, _, h) = req("GET", "/admin/icon.png", "", None);
    assert_eq!(s, 200);
    assert!(h.iter().any(|(k, v)| k.eq_ignore_ascii_case("content-type") && v == "image/png"));
    assert_eq!(req("GET", "/admin/../users.json", "", None).0, 404);
    // first run: the setup code (from the log) makes the first admin, once
    assert_eq!(req("GET", "/api/admin/state", "", None).1["setup"], true);
    let code = server.setup_code().unwrap();
    let setup = |code: &str| req("POST", "/api/admin/setup", "", Some(json!({"code": code, "user": "ann", "password": "correct horse"})));
    assert_eq!(setup("NOPE-0000").0, 403);
    assert_eq!(setup(&code.to_lowercase()).0, 200, "codes are case-insensitive");
    assert_eq!(setup(&code).0, 409, "only while there is no admin");
    assert_eq!(req("GET", "/api/admin/state", "", None).1["setup"], false);
    assert!(server.setup_code().is_none());
    // admin sessions are not device tokens, and the other way round
    let login = |user: &str, pw: &str| req("POST", "/api/admin/login", "", Some(json!({"user": user, "password": pw})));
    assert_eq!(login("ann", "wrong").0, 401);
    let ann = login("ann", "correct horse").1["token"].as_str().unwrap().to_string();
    let device = |user: &str, pw: &str| req("POST", "/api/login", "", Some(json!({"user": user, "password": pw, "device": "Phone"})));
    let ann_device = device("ann", "correct horse").1["token"].as_str().unwrap().to_string();
    assert_eq!(req("GET", "/api/admin/status", &ann_device, None).0, 401);
    assert_eq!(req("GET", "/api/snapshot", &ann, None).0, 401);
    let (s, st, _) = req("GET", "/api/admin/status", &ann, None);
    assert_eq!((s, st["me"].as_str(), st["users"].as_u64()), (200, Some("ann"), Some(1)));
    // users: add, list, reset a password
    assert_eq!(req("POST", "/api/admin/users", &ann, Some(json!({"name": "../x", "password": "long enough"}))).0, 400);
    assert_eq!(req("POST", "/api/admin/users", &ann, Some(json!({"name": "bob", "password": "long enough"}))).0, 200);
    assert_eq!(req("POST", "/api/admin/users", &ann, Some(json!({"name": "bob", "password": "long enough"}))).0, 400, "no silent overwrite");
    let users = req("GET", "/api/admin/users", &ann, None).1;
    let names: Vec<&str> = users.as_array().unwrap().iter().map(|u| u["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["ann", "bob"]);
    assert_eq!(users[0]["admin"], true);
    assert_eq!(login("bob", "long enough").0, 401, "not an admin");
    let bob_device = device("bob", "long enough").1;
    let bob_token = bob_device["token"].as_str().unwrap().to_string();
    let list = req("GET", "/api/admin/users/bob/devices", &ann, None).1;
    assert_eq!(list[0]["name"], "Phone");
    assert!(list[0]["lastSeen"].as_u64().is_some(), "signing in counts as seen: {list}");
    assert_eq!(req("GET", "/api/me", &bob_token, None).0, 200);
    // signing a device out ends its token
    let id = bob_device["device"].as_u64().unwrap();
    assert_eq!(req("DELETE", &format!("/api/admin/users/bob/devices/{id}"), &ann, None).0, 200);
    assert_eq!(req("GET", "/api/me", &bob_token, None).0, 401);
    assert_eq!(req("PUT", "/api/admin/users/bob/password", &ann, Some(json!({"password": "battery staple"}))).0, 200);
    assert_eq!(device("bob", "battery staple").0, 200);
    // names the scan of their library folders leaves out (kept through the password change above)
    let (st, r, _) = req("PUT", "/api/admin/users/bob/ignore", &ann, Some(json!({"ignore": ["*.fcpbundle", " "]})));
    assert_eq!((st, &r["ignore"]), (200, &json!(["*.fcpbundle"])), "{r}");
    assert_eq!(req("PUT", "/api/admin/users/bob/ignore", &ann, Some(json!({"ignore": ["Gigs/2015"]}))).0, 400, "a name, not a path");
    assert_eq!(req("PUT", "/api/admin/users/nobody/ignore", &ann, Some(json!({"ignore": []}))).0, 400);
    assert_eq!(req("GET", "/api/admin/users/bob/folders", &ann, None).1["ignore"], json!(["*.fcpbundle"]));
    assert_eq!(req("PUT", "/api/admin/users/bob/password", &ann, Some(json!({"password": "battery staple"}))).0, 200);
    assert_eq!(req("GET", "/api/admin/users/bob/folders", &ann, None).1["ignore"], json!(["*.fcpbundle"]));
    // there is always an admin
    assert_eq!(req("PUT", "/api/admin/users/ann/admin", &ann, Some(json!({"admin": false}))).0, 409);
    assert_eq!(req("DELETE", "/api/admin/users/ann", &ann, None).0, 409);
    assert_eq!(req("PUT", "/api/admin/users/bob/admin", &ann, Some(json!({"admin": true}))).0, 200);
    assert_eq!(req("PUT", "/api/admin/users/ann/admin", &ann, Some(json!({"admin": false}))).0, 200);
    assert_eq!(req("GET", "/api/admin/status", &ann, None).0, 401, "no longer an admin: the session ends");
    let bob = login("bob", "battery staple").1["token"].as_str().unwrap().to_string();
    // removing a user keeps their files; storage clean-up runs
    let (s, r, _) = req("DELETE", "/api/admin/users/ann", &bob, None);
    assert_eq!(s, 200);
    assert!(std::path::Path::new(r["files"].as_str().unwrap()).exists());
    assert_eq!(device("ann", "correct horse").0, 401);
    let (s, r, _) = req("POST", "/api/admin/gc", &bob, Some(json!({"dryRun": true})));
    assert_eq!((s, r["dryRun"].as_bool()), (200, Some(true)));
    assert_eq!(req("POST", "/api/admin/logout", &bob, None).0, 200);
    assert_eq!(req("GET", "/api/admin/users", &bob, None).0, 401);
    drop(server);
    let _ = std::fs::remove_dir_all(&root);
}

/// What anyone can send without signing in is small.
#[test]
fn unauthenticated_bodies_are_capped() {
    let root = temp("cap");
    let server = start(&root);
    let base = format!("http://{}", server.addr());
    let big = vec![b' '; 200 << 10];
    assert_eq!(call("POST", &format!("{base}/api/login"), "", Some(&big), &[]).0, 413);
    assert_eq!(call("POST", &format!("{base}/api/admin/login"), "", Some(&big), &[]).0, 413);
    assert_eq!(call("POST", &format!("{base}/api/admin/setup"), "", Some(&big), &[]).0, 413);
    let (s, _, hs) = call("GET", &format!("{base}/"), "", None, &[]);
    assert_eq!(s, 200);
    assert!(hs.iter().any(|(k, v)| k.eq_ignore_ascii_case("x-frame-options") && v == "DENY"));
    drop(server);
    let _ = std::fs::remove_dir_all(&root);
}

// ---- resumable uploads ----

/// Bytes that don't repeat (a stand-in for a raw file).
fn noise(n: usize, seed: u64) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect()
}

fn sign_in_raw(base: &str) -> String {
    let body = json!({"user": "ann", "password": "correct horse", "device": "t"}).to_string();
    let (s, b, _) = call("POST", &format!("{base}/api/login"), "", Some(body.as_bytes()), &[]);
    assert_eq!(s, 200);
    json_of(&b)["token"].as_str().unwrap().to_string()
}

/// A `PUT` with extra headers.
fn put_with(url: &str, token: &str, body: &[u8], headers: &[(&str, &str)]) -> (u16, Value) {
    let mut req = agent().put(url).header("Authorization", &format!("Bearer {token}"));
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let mut r = req.send(body).unwrap();
    let status = r.status().as_u16();
    (status, json_of(&r.body_mut().read_to_vec().unwrap_or_default()))
}

/// A `PUT` that announces `declared` bytes and sends only `sent`, then (if `hold` is false) hangs up:
/// the connection a phone loses.
fn broken_put(addr: std::net::SocketAddr, path: &str, token: &str, declared: usize, sent: &[u8], hold: bool) -> Option<std::net::TcpStream> {
    use std::io::Write;
    let mut c = std::net::TcpStream::connect(addr).unwrap();
    let head = format!("PUT {path} HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\nContent-Length: {declared}\r\n\r\n");
    c.write_all(head.as_bytes()).unwrap();
    c.write_all(sent).unwrap();
    c.flush().unwrap();
    hold.then_some(c)
}

/// What `HEAD` says about a file: the status and `Upload-Offset`.
fn head(url: &str, token: &str) -> (u16, Option<u64>) {
    let (s, _, hs) = call("HEAD", url, token, None, &[]);
    (s, hs.iter().find(|(k, _)| k.eq_ignore_ascii_case("upload-offset")).and_then(|(_, v)| v.parse().ok()))
}

/// Wait until `HEAD` reports `want` bytes of an unfinished upload.
fn wait_for_offset(url: &str, token: &str, want: u64) {
    for _ in 0..200 {
        if head(url, token) == (404, Some(want)) {
            std::thread::sleep(Duration::from_millis(50)); // (the request thread lets go of the file)
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("the server never reported {want} bytes: {:?}", head(url, token));
}

#[test]
fn an_interrupted_upload_carries_on_where_it_stopped() {
    let root = temp("resume");
    let server = start(&root);
    let base = format!("http://{}", server.addr());
    let token = sign_in_raw(&base);
    let bytes = noise(400_000, 1);
    let h = lightcraft_preview::hash_bytes(&bytes).to_string();
    let url = format!("{base}/api/blobs/original/{h}");
    assert_eq!(head(&url, &token), (404, Some(0)));

    // the connection breaks 40 % of the way: what arrived stays
    let cut = 160_000;
    broken_put(server.addr(), &format!("/api/blobs/original/{h}"), &token, bytes.len(), &bytes[..cut], false);
    wait_for_offset(&url, &token, cut as u64);
    assert!(std::fs::read_dir(root.join("data/users/ann/blobs/original")).map_or(true, |d| d.count() == 0), "nothing is kept as the file yet");

    // going on from the wrong place is refused, and says where the server is
    let rest = &bytes[cut..];
    let range = |first: usize| format!("bytes {first}-{}/{}", bytes.len() - 1, bytes.len());
    let (s, body) = put_with(&url, &token, &bytes[1000..], &[("Content-Range", &range(1000))]);
    assert_eq!((s, body["offset"].clone()), (409, json!(cut)), "{body}");
    // a range that stops short of the end, a body that isn't the range's length, garbage, other kinds
    let short = format!("bytes {cut}-{}/{}", bytes.len() - 2, bytes.len());
    assert_eq!(put_with(&url, &token, rest, &[("Content-Range", &short)]).0, 400);
    assert_eq!(put_with(&url, &token, &rest[1..], &[("Content-Range", &range(cut))]).0, 400);
    assert_eq!(put_with(&url, &token, rest, &[("Content-Range", "bytes lots")]).0, 400);
    let huge = format!("bytes {cut}-{}/{}", (1u64 << 40) - 1, 1u64 << 40);
    assert_eq!(put_with(&url, &token, rest, &[("Content-Range", &huge)]).0, 400);
    assert_eq!(put_with(&format!("{base}/api/blobs/mini/{h}"), &token, rest, &[("Content-Range", &range(cut))]).0, 400);
    assert_eq!(head(&url, &token), (404, Some(cut as u64)), "none of that touched what is kept");

    // the rest, from where it stopped: the whole file hashes right and is kept
    let (s, body) = put_with(&url, &token, rest, &[("Content-Range", &range(cut))]);
    assert_eq!((s, body), (200, json!({})));
    assert_eq!(head(&url, &token).0, 200);
    let (s, all, _) = call("GET", &url, &token, None, &[]);
    assert_eq!((s, all == bytes), (200, true));
    let tmp = root.join("data/users/ann/blobs/tmp");
    assert!(std::fs::read_dir(&tmp).map_or(true, |d| d.count() == 0), "the partial file is gone");
    drop(server);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_damaged_partial_is_dropped_and_the_upload_starts_again() {
    let root = temp("resume-bad");
    let server = start(&root);
    let base = format!("http://{}", server.addr());
    let token = sign_in_raw(&base);
    let bytes = noise(50_000, 2);
    let h = lightcraft_preview::hash_bytes(&bytes).to_string();
    let url = format!("{base}/api/blobs/original/{h}");
    // 20 000 bytes that are not the start of this file arrived (another file under this name, a
    // bad disk sector, anything): the hash decides
    let wrong = noise(20_000, 3);
    broken_put(server.addr(), &format!("/api/blobs/original/{h}"), &token, bytes.len(), &wrong, false);
    wait_for_offset(&url, &token, 20_000);
    let range = format!("bytes 20000-{}/{}", bytes.len() - 1, bytes.len());
    let (s, body) = put_with(&url, &token, &bytes[20_000..], &[("Content-Range", &range)]);
    assert_eq!(s, 422, "{body}");
    assert_eq!(head(&url, &token), (404, Some(0)), "the damaged partial is gone");
    // the device then sends the whole file
    assert_eq!(put_with(&url, &token, &bytes, &[]).0, 200);
    let (_, all, _) = call("GET", &url, &token, None, &[]);
    assert_eq!(all, bytes);
    // a plain upload over a partial starts over too
    let other = noise(30_000, 4);
    let h2 = lightcraft_preview::hash_bytes(&other).to_string();
    let url2 = format!("{base}/api/blobs/original/{h2}");
    broken_put(server.addr(), &format!("/api/blobs/original/{h2}"), &token, other.len(), &other[..10_000], false);
    wait_for_offset(&url2, &token, 10_000);
    assert_eq!(put_with(&url2, &token, &other, &[]).0, 200);
    assert_eq!(call("GET", &url2, &token, None, &[]).1, other);
    drop(server);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn one_request_writes_a_file_at_a_time() {
    let root = temp("resume-busy");
    let server = start(&root);
    let base = format!("http://{}", server.addr());
    let token = sign_in_raw(&base);
    let bytes = noise(80_000, 5);
    let h = lightcraft_preview::hash_bytes(&bytes).to_string();
    let url = format!("{base}/api/blobs/original/{h}");
    // an upload that is still going (it has sent half and waits)
    let open = broken_put(server.addr(), &format!("/api/blobs/original/{h}"), &token, bytes.len(), &bytes[..40_000], true);
    wait_for_offset(&url, &token, 40_000);
    let (s, body) = put_with(&url, &token, &bytes, &[]);
    assert_eq!((s, body["offset"].clone()), (409, json!(40_000)), "{body}");
    // once it hangs up, the file can be finished
    drop(open);
    let mut done = 409;
    for _ in 0..100 {
        let range = format!("bytes 40000-{}/{}", bytes.len() - 1, bytes.len());
        done = put_with(&url, &token, &bytes[40_000..], &[("Content-Range", &range)]).0;
        if done != 409 {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(done, 200);
    assert_eq!(call("GET", &url, &token, None, &[]).1, bytes);
    drop(server);
    let _ = std::fs::remove_dir_all(&root);
}

/// A preview is whole or nothing: a body that ends early is refused, not kept (before, the
/// magic number at its start was all that was checked).
#[test]
fn a_truncated_preview_is_not_kept() {
    let root = temp("resume-preview");
    let server = start(&root);
    let base = format!("http://{}", server.addr());
    let token = sign_in_raw(&base);
    let h = lightcraft_preview::hash_bytes(b"x").to_string();
    let mut preview = b"LCSP1\n".to_vec();
    preview.extend(noise(10_000, 6));
    for kind in ["mini", "smart"] {
        let url = format!("{base}/api/blobs/{kind}/{h}");
        broken_put(server.addr(), &format!("/api/blobs/{kind}/{h}"), &token, preview.len(), &preview[..4_000], false);
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(head(&url, &token).0, 404, "{kind}");
        assert_eq!(put_with(&url, &token, &preview, &[]).0, 200, "{kind}: a whole one is kept");
    }
    drop(server);
    let _ = std::fs::remove_dir_all(&root);
}

// ---- the HTTP layer ----

/// Stalled uploads can't use the server up: with every place taken the health check still
/// answers, everything else is told to try again, and the places are free once the uploads stop.
/// (Before, 64 stalled requests made the server answer `503` to everything, health check included.)
#[test]
fn stalled_uploads_do_not_starve_the_server() {
    let root = temp("starve");
    let data = root.join("data");
    accounts::set_user(&data, "ann", "correct horse", false).unwrap();
    let mut cfg = Config::new(data, "127.0.0.1:0");
    cfg.scan_interval = None;
    cfg.preview_threads = 1;
    cfg.max_requests = 2;
    let server = Server::start(cfg).unwrap();
    let base = format!("http://{}", server.addr());
    let token = sign_in_raw(&base);
    let (a, b) = (lightcraft_preview::hash_bytes(b"a").to_string(), lightcraft_preview::hash_bytes(b"b").to_string());
    // two uploads that began and went quiet hold the two places
    let held: Vec<_> = [a, b].iter().map(|h| broken_put(server.addr(), &format!("/api/blobs/original/{h}"), &token, 1000, &[7; 10], true)).collect();
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(call("GET", &format!("{base}/api/health"), "", None, &[]).0, 200, "the health check never waits for a place");
    let (s, _, hs) = call("GET", &format!("{base}/api/me"), &token, None, &[]);
    assert_eq!(s, 503, "no place left");
    assert!(hs.iter().any(|(k, _)| k.eq_ignore_ascii_case("retry-after")));
    drop(held);
    let mut answered = 0;
    for _ in 0..100 {
        answered = call("GET", &format!("{base}/api/me"), &token, None, &[]).0;
        if answered == 200 {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(answered, 200, "the places are free again");
    drop(server);
    let _ = std::fs::remove_dir_all(&root);
}

/// A request as raw text, the answer as text (the server closes the connection after it).
fn raw_request(addr: std::net::SocketAddr, request: &str) -> String {
    use std::io::{Read, Write};
    let mut c = std::net::TcpStream::connect(addr).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    c.write_all(request.as_bytes()).unwrap();
    let mut got = String::new();
    let _ = c.read_to_string(&mut got);
    got
}

/// A web build served from another site can call the API only if the server was told that site
/// (`--cors-origin`): the browser's preflight is answered without signing in, the answer to a real
/// call says the site may read it, and nothing is said to other sites or about the admin page.
#[test]
fn only_sites_the_server_was_told_about_may_call_it_from_a_browser() {
    let root = temp("cors");
    let data = root.join("data");
    accounts::set_user(&data, "ann", "correct horse", false).unwrap();
    let mut cfg = Config::new(data, "127.0.0.1:0");
    cfg.scan_interval = None;
    cfg.preview_threads = 1;
    cfg.cors_origins = vec!["https://photos.example.com/".into()];
    let server = Server::start(cfg).unwrap();
    let addr = server.addr();
    let preflight = |origin: &str, path: &str| {
        raw_request(
            addr,
            &format!(
                "OPTIONS {path} HTTP/1.1\r\nHost: x\r\nOrigin: {origin}\r\nAccess-Control-Request-Method: PUT\r\nAccess-Control-Request-Headers: authorization, content-range\r\nConnection: close\r\n\r\n"
            ),
        )
    };
    let ok = preflight("https://photos.example.com", "/api/blobs/original/0123456789abcdef0123456789abcdef");
    assert!(ok.starts_with("HTTP/1.1 204"), "{ok}");
    for want in [
        "Access-Control-Allow-Origin: https://photos.example.com",
        "Access-Control-Allow-Headers: Authorization, Content-Type, Range, Content-Range",
        "Access-Control-Allow-Methods:",
        "Vary: Origin",
    ] {
        assert!(ok.contains(want), "{want} in {ok}");
    }
    let other = preflight("https://evil.example.com", "/api/me");
    assert!(other.starts_with("HTTP/1.1 403") && !other.contains("Access-Control-Allow-Origin"), "{other}");
    assert!(
        !preflight("https://photos.example.com", "/api/admin/state").contains("Access-Control-Allow-Origin"),
        "the admin page is not for other sites"
    );
    // a real call: the allowed site may read the answer, including what the client needs from it
    let call = |origin: &str| raw_request(addr, &format!("GET /api/health HTTP/1.1\r\nHost: x\r\n{origin}Connection: close\r\n\r\n"));
    let got = call("Origin: https://photos.example.com\r\n");
    assert!(got.contains("Access-Control-Allow-Origin: https://photos.example.com") && got.contains("Upload-Offset"), "{got}");
    assert!(!call("Origin: https://evil.example.com\r\n").contains("Access-Control-Allow-Origin"));
    assert!(!call("").contains("Access-Control-Allow-Origin"), "no Origin, no CORS headers");
    drop(server);
    // without any site listed nothing is allowed
    let root2 = temp("cors-none");
    let server = start(&root2);
    let got = raw_request(server.addr(), "OPTIONS /api/me HTTP/1.1\r\nHost: x\r\nOrigin: https://photos.example.com\r\nConnection: close\r\n\r\n");
    assert!(got.starts_with("HTTP/1.1 403"), "{got}");
    drop(server);
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&root2);
}

// ---- shared documents ----

/// The shared documents (`/api/docs/<name>`): a name the server keeps, versions, `412` with the current
/// document when another device wrote first, and what is refused.
#[test]
fn shared_documents_are_versioned_and_only_known_names_are_kept() {
    let root = temp("docs");
    let server = start(&root);
    let base = format!("http://{}", server.addr());
    let token = sign_in_raw(&base);
    let doc = |name: &str| format!("{base}/api/docs/{name}");
    let put = |name: &str, v: Value| put_with(&doc(name), &token, v.to_string().as_bytes(), &[]);
    let (s, d) = {
        let (s, b, _) = call("GET", &doc("export-presets"), &token, None, &[]);
        (s, json_of(&b))
    };
    assert_eq!((s, d), (200, json!({"version": 0, "items": []})));
    assert_eq!(put("export-presets", json!({"version": 0, "items": [{"name": "Web"}]})), (200, json!({"version": 1})));
    // another device that hadn't seen it: the current document comes back
    let (s, cur) = put("export-presets", json!({"version": 0, "items": []}));
    assert_eq!((s, cur), (412, json!({"version": 1, "items": [{"name": "Web"}]})));
    assert_eq!(put("export-presets", json!({"version": 1, "items": []})).0, 200);
    // the versions come with every pull
    let (_, b, _) = call("GET", &format!("{base}/api/ops?since=0"), &token, None, &[]);
    assert_eq!(json_of(&b)["docs"], json!({"export-presets": 2}));
    // what is refused: other names (no path tricks), items that aren't a list, junk, too much
    assert_eq!(call("GET", &doc("users"), &token, None, &[]).0, 404);
    assert_eq!(call("GET", &doc("..%2fpresets"), &token, None, &[]).0, 404);
    assert_eq!(put("presets", json!({"version": 0, "items": []})).0, 404);
    assert_eq!(put("prefs", json!({"version": 0, "items": {"a": 1}})).0, 400);
    assert_eq!(put_with(&doc("prefs"), &token, b"{garbage", &[]).0, 400);
    assert_eq!(call("GET", &doc("prefs"), "", None, &[]).0, 401, "only for signed-in devices");
    // another user has their own
    accounts::set_user(&root.join("data"), "bob", "battery staple", false).unwrap();
    let body = json!({"user": "bob", "password": "battery staple", "device": "t"}).to_string();
    let bob = json_of(&call("POST", &format!("{base}/api/login"), "", Some(body.as_bytes()), &[]).1)["token"].as_str().unwrap().to_string();
    let (_, b, _) = call("GET", &doc("export-presets"), &bob, None, &[]);
    assert_eq!(json_of(&b)["version"], 0);
    // it survives a restart
    drop(server);
    let mut cfg = Config::new(root.join("data"), "127.0.0.1:0");
    cfg.scan_interval = None;
    cfg.preview_threads = 1;
    let server = Server::start(cfg).unwrap();
    let base = format!("http://{}", server.addr());
    let token = sign_in_raw(&base);
    let (_, b, _) = call("GET", &format!("{base}/api/docs/export-presets"), &token, None, &[]);
    assert_eq!(json_of(&b)["version"], 2);
    drop(server);
    let _ = std::fs::remove_dir_all(&root);
}
