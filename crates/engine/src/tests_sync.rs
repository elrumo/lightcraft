//! Self-hosted sync, device side: real libraries on disk talking to an in-process server (the
//! catalog's [`ServerCore`] plus a blob map), so every request the engine makes is answered the
//! way `lightcraft-server` answers it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use lightcraft_catalog::sync::{PushError, ServerCore, proto};
use lightcraft_catalog::{MemStore, Op, PhotoId, Source};
use serde_json::{Value, json};

use crate::Session;
use crate::sync::{Body, Done, Task, is_remote, server_address};

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-sync-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write_png(path: &Path, seed: u8) {
    let (w, h) = (96usize, 64usize);
    let data: Vec<[u8; 4]> = (0..w * h).map(|i| [(i % w * 2) as u8, (i / w * 3) as u8, seed, 255]).collect();
    let img = lightcraft_raster::Rgba8 { width: w, height: h, data };
    let bytes = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

/// The server, in process.
struct Fake {
    core: ServerCore,
    blobs: HashMap<(String, String), Vec<u8>>,
    library: String,
    spaces: u32,
    presets: proto::Presets,
    /// Answer nothing (the network is down).
    down: bool,
    /// A server from before `GET /api/usage`.
    no_usage: bool,
    /// Originals whose upload broke: what the server kept of each, by (kind, hash).
    partials: HashMap<(String, String), Vec<u8>>,
    /// Answer a `HEAD` with this offset whatever is kept (an answer that has gone out of date).
    stale_offset: Option<u64>,
    requests: Vec<String>,
}

impl Fake {
    fn new() -> Fake {
        Fake {
            core: ServerCore::open(Box::new(MemStore::new())).unwrap(),
            blobs: HashMap::new(),
            library: "lib-1".into(),
            spaces: 0,
            presets: proto::Presets { version: 0, presets: json!([]) },
            down: false,
            no_usage: false,
            partials: HashMap::new(),
            stale_offset: None,
            requests: vec![],
        }
    }

    fn handle(&mut self, t: &Task) -> Done {
        let (id, method, url, body, save_to) = match t {
            Task::Proxies { .. } => return crate::sync::run(t),
            Task::Http { id, method, url, body, save_to, .. } => (*id, *method, url, body, save_to),
        };
        if self.down {
            return Done::failed(id, "connection refused");
        }
        let path = url.strip_prefix("http://fake").unwrap();
        let from = if let Body::FileFrom(_, at) = body { format!(" from {at}") } else { String::new() };
        self.requests.push(format!("{method} {path}{from}"));
        let ok = |v: Value| Done { id, status: 200, body: v.to_string() };
        let status = |s: u16, v: Value| Done { id, status: s, body: v.to_string() };
        let json_body = || match body {
            Body::Json(j) => j.clone(),
            other => panic!("{other:?}"),
        };
        match (method, path) {
            ("POST", "/api/login") => {
                let l: proto::Login = serde_json::from_str(&json_body()).unwrap();
                if l.password != "pw" {
                    return status(401, json!({"error": "wrong user name or password"}));
                }
                self.spaces += 1;
                let dev = proto::Device {
                    token: format!("t{}", self.spaces),
                    device: u64::from(self.spaces),
                    space: self.spaces,
                    library: self.library.clone(),
                };
                ok(serde_json::to_value(dev).unwrap())
            }
            ("GET", "/api/snapshot") => {
                let (seq, snap) = self.core.snapshot();
                let catalog: Value = serde_json::from_str(&snap).unwrap();
                ok(json!({"library": self.library, "seq": seq, "catalog": catalog}))
            }
            ("GET", p) if p.starts_with("/api/ops?since=") => {
                let q = p.trim_start_matches("/api/ops?since=");
                let (since, limit) = q.split_once("&limit=").unwrap();
                match self.core.since(since.parse().unwrap(), limit.parse().unwrap()).unwrap() {
                    lightcraft_catalog::sync::Pull::Ops(ops) => {
                        ok(serde_json::to_value(proto::Ops { head: self.core.head(), ops, presets: self.presets.version }).unwrap())
                    }
                    lightcraft_catalog::sync::Pull::Gone => status(410, json!({"error": "gone"})),
                }
            }
            ("POST", "/api/ops") => {
                let push: proto::Push = serde_json::from_str(&json_body()).unwrap();
                match self.core.push(push.base, &push.ops) {
                    Ok(head) => ok(json!({"head": head})),
                    Err(PushError::Behind { head }) => status(409, json!({"head": head})),
                    Err(PushError::Rejected { index, error }) => status(422, json!({"index": index, "error": error})),
                    Err(PushError::Storage(e)) => status(500, json!({"error": e})),
                }
            }
            ("GET", "/api/usage") if !self.no_usage => {
                let kind = |k: &str| {
                    let sizes: Vec<u64> = self.blobs.iter().filter(|((kind, _), _)| kind == k).map(|(_, b)| b.len() as u64).collect();
                    proto::Files { files: sizes.len() as u64, bytes: sizes.iter().sum() }
                };
                let u = proto::Usage {
                    photos: self.core.catalog().len() as u64,
                    devices: u64::from(self.spaces),
                    original: kind("original"),
                    smart: kind("smart"),
                    mini: kind("mini"),
                    disk: Some(proto::Disk { total: 1000, free: 400 }),
                    ..Default::default()
                };
                ok(serde_json::to_value(u).unwrap())
            }
            ("GET", "/api/presets") => ok(serde_json::to_value(&self.presets).unwrap()),
            ("PUT", "/api/presets") => {
                let p: proto::Presets = serde_json::from_str(&json_body()).unwrap();
                if p.version != self.presets.version {
                    return status(412, serde_json::to_value(&self.presets).unwrap());
                }
                self.presets = proto::Presets { version: p.version + 1, presets: p.presets };
                ok(json!({"version": self.presets.version}))
            }
            (m, p) if p.starts_with("/api/blobs/") => {
                let (kind, hash) = p.trim_start_matches("/api/blobs/").split_once('/').unwrap();
                let k = (kind.to_string(), hash.to_string());
                match m {
                    "HEAD" if self.blobs.contains_key(&k) => status(200, Value::Null),
                    // (a HEAD has no body: the engine reads the Upload-Offset the host puts there)
                    "HEAD" => match self.partials.get(&k) {
                        Some(p) => Done { id, status: 404, body: self.stale_offset.unwrap_or(p.len() as u64).to_string() },
                        None => Done { id, status: 404, body: String::new() },
                    },
                    "PUT" => match body {
                        Body::File(f) => {
                            self.partials.remove(&k);
                            self.blobs.insert(k, std::fs::read(f).unwrap());
                            ok(Value::Null)
                        }
                        Body::FileFrom(f, at) => {
                            let have = self.partials.get(&k).map_or(0, |p| p.len() as u64);
                            if *at != have {
                                return status(409, json!({"error": "not where the server is", "offset": have}));
                            }
                            let all = std::fs::read(f).unwrap();
                            let mut whole = self.partials.remove(&k).unwrap_or_default();
                            whole.extend_from_slice(&all[*at as usize..]);
                            self.blobs.insert(k, whole);
                            ok(Value::Null)
                        }
                        other => panic!("{other:?}"),
                    },
                    "GET" => match self.blobs.get(&k) {
                        Some(b) => {
                            let dest = save_to.as_ref().unwrap();
                            std::fs::create_dir_all(Path::new(dest).parent().unwrap()).unwrap();
                            std::fs::write(dest, b).unwrap();
                            Done { id, status: 200, body: String::new() }
                        }
                        None => status(404, json!({"error": "not here"})),
                    },
                    _ => status(405, Value::Null),
                }
            }
            _ => status(404, json!({"error": "no such route"})),
        }
    }
}

fn sync(s: &mut Session, f: &mut Fake) -> Value {
    s.sync_now_with(10_000, &mut |t| f.handle(t))
}

fn open(dir: &Path) -> Session {
    let mut s = Session::new().with_fs();
    s.open_library(dir, false).unwrap();
    s
}

fn sign_in(s: &mut Session) {
    s.execute("sync.signIn", &json!({"server": "http://fake", "user": "ann", "password": "pw", "device": "test"})).unwrap();
}

fn exposure(s: &Session, id: PhotoId) -> f64 {
    s.catalog.photo(id).unwrap().develop.light.exposure
}

/// A device with two imported photos, signed in to an empty server: it uploads its library.
fn first_device(tag: &str, f: &mut Fake) -> (Session, PathBuf, Vec<PhotoId>) {
    let root = temp_dir(tag);
    write_png(&root.join("in/a.png"), 1);
    write_png(&root.join("in/b.png"), 2);
    let mut a = open(&root.join("a"));
    a.execute("library.import", &json!({"paths": [root.join("in").to_string_lossy()]})).unwrap();
    let ids: Vec<PhotoId> = a.catalog.photos().map(|p| p.id).collect();
    assert_eq!(ids.len(), 2);
    sign_in(&mut a);
    let st = sync(&mut a, f);
    assert_eq!(st["state"], "idle", "{st}");
    (a, root, ids)
}

#[test]
fn two_devices_share_photos_edits_and_files() {
    let mut f = Fake::new();
    let (mut a, root, ids) = first_device("share", &mut f);
    // the library and its files are on the server: originals, smart and mini previews
    assert_eq!(f.core.catalog().len(), 2);
    assert_eq!(f.blobs.len(), 6, "{:?}", f.blobs.keys().collect::<Vec<_>>());
    let p = f.core.catalog().photo(ids[0]).unwrap();
    assert!(matches!(&p.source, Source::File { path } if path.starts_with("web/")), "paths on A's disk stay there: {:?}", p.source);
    assert!(p.history.is_empty(), "History stays on the device");
    assert!(!is_remote(a.catalog.photo(ids[0]).unwrap()), "A keeps its own files");

    // a second device joins with a new library and gets everything
    let mut b = open(&root.join("b"));
    sign_in(&mut b);
    sync(&mut b, &mut f);
    assert_eq!(b.catalog.len(), 2);
    assert!(b.catalog.photos().all(|p| is_remote(p)));
    // grid thumbnails render from the mini previews, the loupe from the smart preview once here
    let thumb = b.render_now(ids[0], 128, 128).unwrap();
    assert!(thumb.image.width > 0);
    let smart_dir = root.join("b/Smart Previews");
    let minis = std::fs::read_dir(&smart_dir).unwrap().flatten().filter(|e| e.path().extension().is_some_and(|x| x == "lcsm")).count();
    assert_eq!(minis, 2);
    assert!(crate::cmd::missing::missing(&b).is_empty(), "originals on the server aren't Missing Photos");

    // concurrent edits: exposure here, contrast there, ratings and an album
    b.selection = crate::Selection::single(ids[0]);
    b.execute("develop.set", &json!({"control": "light.exposure", "value": 1.0})).unwrap();
    a.selection = crate::Selection::single(ids[0]);
    a.execute("develop.set", &json!({"control": "light.contrast", "value": 25.0})).unwrap();
    a.selection = crate::Selection::single(ids[1]);
    a.execute("photo.rate", &json!({"rating": 4})).unwrap();
    b.selection = crate::Selection::single(ids[1]);
    let album = b.execute("album.create", &json!({"name": "Trip", "addSelected": true})).unwrap();
    sync(&mut b, &mut f);
    sync(&mut a, &mut f);
    sync(&mut b, &mut f);
    for s in [&a, &b] {
        let d = &s.catalog.photo(ids[0]).unwrap().develop;
        assert_eq!((d.light.exposure, d.light.contrast), (1.0, 25.0));
        assert_eq!(s.catalog.photo(ids[1]).unwrap().rating, 4);
        let trip: Vec<_> = s.catalog.albums().filter(|a| a.name == "Trip").collect();
        assert_eq!((trip.len(), trip[0].photos.clone()), (1, vec![ids[1]]));
    }
    // ids allocated on B are in B's space, never A's
    let album_id = album["id"].as_u64().unwrap();
    assert_eq!(album_id >> 32, u64::from(b.sync_state().unwrap().config.space));

    // the active photo's smart preview comes down for the loupe
    assert!(smart_dir.join(crate::smart::file_name(b.catalog.photo(ids[1]).unwrap())).exists());
    // and an original on request: the photo then points at the downloaded file (here only)
    b.execute("sync.downloadOriginals", &json!({"ids": [ids[1].0]})).unwrap();
    sync(&mut b, &mut f);
    let p = b.catalog.photo(ids[1]).unwrap();
    let Source::File { path } = &p.source else { panic!() };
    assert!(path.contains("sync") && Path::new(path).exists(), "{path}");
    assert!(b.sync_state().unwrap().outbox.is_empty(), "the relink stays on this device");
    assert!(is_remote(f.core.catalog().photo(ids[1]).unwrap()));

    // reopened: the same sync state, nothing to redo
    let cursor = b.sync_state().unwrap().config.cursor;
    b.close_library().unwrap();
    drop(b);
    let mut b = open(&root.join("b"));
    assert_eq!(b.sync_state().unwrap().config.cursor, cursor);
    let before = f.requests.len();
    sync(&mut b, &mut f);
    assert!(f.requests[before..].iter().all(|r| !r.starts_with("PUT") && !r.contains("/blobs/")), "{:?}", &f.requests[before..]);
    assert_eq!(exposure(&b, ids[0]), 1.0);
    let _ = std::fs::remove_dir_all(&root);
}

/// A new library seeded with the demo photos joins: their ids mean other photos than the
/// server's, so nothing of them carries over (file names, sources, the selection).
#[test]
fn joining_from_a_demo_library_takes_the_servers_photos_whole() {
    let mut f = Fake::new();
    let (a, root, ids) = first_device("demojoin", &mut f);
    let mut b = Session::new().with_fs();
    b.open_library(root.join("b"), true).unwrap();
    assert!(b.catalog.len() > 2, "seeded with the demo photos");
    sign_in(&mut b);
    sync(&mut b, &mut f);
    assert_eq!(b.catalog.len(), 2);
    for id in &ids {
        let (pa, pb) = (a.catalog.photo(*id).unwrap(), b.catalog.photo(*id).unwrap());
        assert_eq!(pb.file_name, pa.file_name);
        assert!(is_remote(pb), "{:?}", pb.source);
    }
    assert!(b.selection.active.is_none_or(|id| ids.contains(&id)));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn demo_photos_alone_are_never_uploaded() {
    let mut f = Fake::new();
    let root = temp_dir("demofirst");
    let mut a = Session::new().with_fs();
    a.open_library(root.join("a"), true).unwrap();
    assert!(a.catalog.len() > 2, "seeded with the demo photos");
    sign_in(&mut a);
    sync(&mut a, &mut f);
    assert_eq!(a.catalog.len(), 0, "the empty server library replaces the demo photos");
    assert_eq!(f.core.head(), 0, "nothing was uploaded");
    // the device's own photos go up as usual
    write_png(&root.join("in/a.png"), 1);
    a.execute("library.import", &json!({"paths": [root.join("in").to_string_lossy()]})).unwrap();
    sync(&mut a, &mut f);
    assert!(f.core.head() > 0);
    let mut b = open(&root.join("b"));
    sign_in(&mut b);
    sync(&mut b, &mut f);
    assert_eq!(b.catalog.len(), 1);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn signing_in_counts_as_signed_in_while_the_request_is_out() {
    let root = temp_dir("signing");
    let mut s = open(&root.join("a"));
    sign_in(&mut s);
    let tasks = s.sync_tasks();
    assert_eq!(tasks.len(), 1, "the sign-in request");
    assert!(s.sync_state().unwrap().signed_in());
    assert!(s.execute("sync.now", &json!({})).is_ok(), "Sync Now right after Sign In");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn changes_made_offline_survive_a_restart() {
    let mut f = Fake::new();
    let (mut a, root, ids) = first_device("offline", &mut f);
    f.down = true;
    a.selection = crate::Selection::single(ids[0]);
    a.execute("photo.rate", &json!({"rating": 5})).unwrap();
    let st = sync(&mut a, &mut f);
    assert_eq!(st["state"], "error");
    assert_eq!(st["pending"], 1);
    // a crash: nothing but what's on disk is left
    drop(a);
    let mut a = open(&root.join("a"));
    assert_eq!(a.sync_state().unwrap().outbox.len(), 1, "the change waits in sync.outbox");
    f.down = false;
    let st = sync(&mut a, &mut f);
    assert_eq!(st["state"], "idle", "{st}");
    assert_eq!(f.core.catalog().photo(ids[0]).unwrap().rating, 5);
    assert_eq!(a.catalog.photo(ids[0]).unwrap().rating, 5);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_library_with_photos_cannot_join_another() {
    let mut f = Fake::new();
    let (_a, root, _) = first_device("join", &mut f);
    write_png(&root.join("in2/c.png"), 3);
    let mut c = open(&root.join("c"));
    c.execute("library.import", &json!({"paths": [root.join("in2").to_string_lossy()]})).unwrap();
    sign_in(&mut c);
    let st = sync(&mut c, &mut f);
    assert_eq!(st["signedIn"], false, "{st}");
    assert!(st["error"].as_str().unwrap().contains("new library"), "{st}");
    assert_eq!(c.catalog.len(), 1, "nothing changed here");
    assert_eq!(f.core.catalog().len(), 2, "nor there");
    // a wrong password says so
    let mut d = open(&root.join("d"));
    d.execute("sync.signIn", &json!({"server": "http://fake", "user": "ann", "password": "nope"})).unwrap();
    let st = sync(&mut d, &mut f);
    assert!(st["error"].as_str().unwrap().contains("password"), "{st}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn remote_changes_wait_for_a_slider_drag() {
    let mut f = Fake::new();
    let (mut a, root, ids) = first_device("drag", &mut f);
    let mut b = open(&root.join("b"));
    sign_in(&mut b);
    sync(&mut b, &mut f);
    b.selection = crate::Selection::single(ids[0]);
    b.execute("photo.rate", &json!({"rating": 3})).unwrap();
    sync(&mut b, &mut f);
    a.selection = crate::Selection::single(ids[0]);
    a.begin_interaction("Exposure").unwrap();
    let mut d = (*a.catalog.photo(ids[0]).unwrap().develop).clone();
    d.light.exposure = 0.5;
    a.set_develop(ids[0], d, "Exposure").unwrap();
    sync(&mut a, &mut f);
    assert_eq!(a.catalog.photo(ids[0]).unwrap().rating, 0, "held during the drag");
    a.end_interaction().unwrap();
    sync(&mut a, &mut f);
    assert_eq!(a.catalog.photo(ids[0]).unwrap().rating, 3);
    sync(&mut b, &mut f);
    assert_eq!(exposure(&b, ids[0]), 0.5);
    let _ = std::fs::remove_dir_all(&root);
}

/// A library with two imported photos, not signed in yet: where it is, the session, their ids.
fn two_photos(tag: &str) -> (PathBuf, Session, Vec<PhotoId>) {
    let root = temp_dir(tag);
    write_png(&root.join("in/a.png"), 1);
    write_png(&root.join("in/b.png"), 2);
    let mut a = open(&root.join("a"));
    a.execute("library.import", &json!({"paths": [root.join("in").to_string_lossy()]})).unwrap();
    let ids: Vec<PhotoId> = a.catalog.photos().map(|p| p.id).collect();
    assert_eq!(ids.len(), 2);
    (root, a, ids)
}

/// The server's key for a photo's files and the bytes of its original.
fn original_of(s: &Session, id: PhotoId) -> (String, Vec<u8>) {
    let p = s.catalog.photo(id).unwrap();
    let Source::File { path } = &p.source else { panic!("{:?}", p.source) };
    (crate::sync::blob_key(p).unwrap(), std::fs::read(path).unwrap())
}

/// An upload that broke is not sent again from the start: `HEAD` says how much the server kept,
/// and the rest is sent from there. The other photos upload as usual.
#[test]
fn an_upload_that_broke_goes_on_where_the_server_has_it() {
    let mut f = Fake::new();
    let (root, mut a, ids) = two_photos("resume");
    let (key, bytes) = original_of(&a, ids[0]);
    let cut = bytes.len() / 3;
    assert!(cut > 0);
    f.partials.insert(("original".into(), key.clone()), bytes[..cut].to_vec());
    sign_in(&mut a);
    let st = sync(&mut a, &mut f);
    assert_eq!(st["state"], "idle", "{st}");
    let put = format!("PUT /api/blobs/original/{key}");
    assert!(f.requests.contains(&format!("{put} from {cut}")), "{:?}", f.requests);
    assert!(!f.requests.contains(&put), "not from the start: {:?}", f.requests);
    assert_eq!(f.blobs.get(&("original".into(), key)), Some(&bytes), "the server holds the whole file");
    assert!(f.partials.is_empty());
    let (other, _) = original_of(&a, ids[1]);
    assert!(f.requests.contains(&format!("PUT /api/blobs/original/{other}")), "the other photo went whole");
    let _ = std::fs::remove_dir_all(&root);
}

/// The server has more (or less) of the file than the device was told: it says so and the device
/// carries on from there, without waiting a minute to try again.
#[test]
fn an_upload_corrects_its_start_when_the_server_says_so() {
    let mut f = Fake::new();
    let (root, mut a, ids) = two_photos("resume-409");
    let (key, bytes) = original_of(&a, ids[0]);
    let kept = bytes.len() / 2;
    f.partials.insert(("original".into(), key.clone()), bytes[..kept].to_vec());
    f.stale_offset = Some(10);
    sign_in(&mut a);
    let st = sync(&mut a, &mut f);
    assert_eq!(st["state"], "idle", "{st}");
    let put = format!("PUT /api/blobs/original/{key}");
    let sent: Vec<_> = f.requests.iter().filter(|r| r.starts_with(&put)).cloned().collect();
    assert_eq!(sent, [format!("{put} from 10"), format!("{put} from {kept}")]);
    assert_eq!(f.blobs.get(&("original".into(), key)), Some(&bytes));
    let _ = std::fs::remove_dir_all(&root);
}

/// An original that comes down wrong (a resumed download spliced onto a changed file, a folder
/// file edited on the server) is not kept as the photo's file.
#[test]
fn a_downloaded_original_that_is_not_the_photo_is_not_kept() {
    let mut f = Fake::new();
    let (mut a, root, ids) = first_device("badget", &mut f);
    let (key, bytes) = original_of(&a, ids[1]);
    let mut b = open(&root.join("b"));
    sign_in(&mut b);
    sync(&mut b, &mut f);
    let mut wrong = bytes.clone();
    wrong.push(0);
    f.blobs.insert(("original".into(), key), wrong);
    b.selection = crate::Selection::single(ids[1]);
    b.execute("sync.downloadOriginals", &json!({"ids": [ids[1].0]})).unwrap();
    sync(&mut b, &mut f);
    let p = b.catalog.photo(ids[1]).unwrap();
    assert!(is_remote(p), "still a photo of the server's: {:?}", p.source);
    let dir = root.join("b/sync/originals");
    assert!(
        std::fs::read_dir(&dir).map_or(true, |d| d.flatten().all(|e| std::fs::read_dir(e.path()).map_or(true, |f| f.count() == 0))),
        "nothing was left in {dir:?}"
    );
    let _ = a.close_library();
    let _ = std::fs::remove_dir_all(&root);
}

fn snapshots(f: &Fake) -> usize {
    f.requests.iter().filter(|r| *r == "GET /api/snapshot").count()
}

/// The server compacts its log while a device is away for long: it can't pull what's gone, so it
/// reloads the library — and the changes it made meanwhile (the outbox) are replayed on top, not
/// lost.
#[test]
fn a_device_further_behind_than_the_server_keeps_reloads_and_keeps_its_changes() {
    let mut f = Fake::new();
    f.core.set_compact_bytes(3000);
    let (mut a, root, ids) = first_device("gone", &mut f);
    let mut b = open(&root.join("b"));
    sign_in(&mut b);
    sync(&mut b, &mut f);
    let behind = b.sync_state().unwrap().config.cursor;
    // B changes a photo and goes away; A keeps working until the log has been compacted past B
    b.selection = crate::Selection::single(ids[1]);
    b.execute("photo.rate", &json!({"rating": 3})).unwrap();
    a.selection = crate::Selection::single(ids[0]);
    for i in 0..80 {
        a.execute("photo.rate", &json!({"rating": i % 5 + 1})).unwrap();
        sync(&mut a, &mut f);
    }
    assert_eq!(f.core.since(behind, 10).unwrap(), lightcraft_catalog::sync::Pull::Gone, "the log still goes back to B");
    let before = snapshots(&f);
    let st = sync(&mut b, &mut f);
    assert_eq!(st["state"], "idle", "{st}");
    assert!(snapshots(&f) > before, "B reloaded the library: {:?}", f.requests);
    let last = (79 % 5 + 1) as u8;
    assert_eq!(b.catalog.photo(ids[0]).unwrap().rating, last, "A's changes came with the snapshot");
    assert_eq!(b.catalog.photo(ids[1]).unwrap().rating, 3, "B's own change survived the reload");
    assert_eq!(f.core.catalog().photo(ids[1]).unwrap().rating, 3, "and reached the server");
    sync(&mut a, &mut f);
    assert_eq!(a.catalog.photo(ids[1]).unwrap().rating, 3);
    let _ = std::fs::remove_dir_all(&root);
}

/// A compaction leaves the newest ops in the log: a device that is only a little behind it keeps
/// pulling instead of reloading the whole library.
#[test]
fn a_device_one_op_behind_a_compaction_keeps_pulling() {
    let mut f = Fake::new();
    f.core.set_compact_bytes(3000);
    let (mut a, root, ids) = first_device("tail", &mut f);
    let mut b = open(&root.join("b"));
    sign_in(&mut b);
    sync(&mut b, &mut f);
    a.selection = crate::Selection::single(ids[0]);
    // until the push that compacts: B has everything before it
    let mut compacted = false;
    for i in 0..200 {
        sync(&mut b, &mut f);
        a.execute("photo.rate", &json!({"rating": i % 5 + 1})).unwrap();
        sync(&mut a, &mut f);
        if f.core.since(0, 1).unwrap() == lightcraft_catalog::sync::Pull::Gone {
            compacted = true;
            break;
        }
    }
    assert!(compacted, "the server never compacted");
    let before = snapshots(&f);
    let st = sync(&mut b, &mut f);
    assert_eq!(st["state"], "idle", "{st}");
    assert_eq!(snapshots(&f), before, "B pulled the newest ops, no reload: {:?}", f.requests);
    assert_eq!(b.catalog.photo(ids[0]).unwrap().rating, a.catalog.photo(ids[0]).unwrap().rating);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn hostile_answers_are_errors_not_crashes() {
    let root = temp_dir("hostile");
    let mut s = open(&root.join("a"));
    sign_in(&mut s);
    let answers = [
        (200, "not json".to_string()),
        (200, json!({"token": "t", "device": 1, "space": 0, "library": "x"}).to_string()),
        (200, json!({"token": "t", "device": 1, "space": u32::MAX, "library": "x"}).to_string()),
        (500, "<html>".to_string()),
        (200, json!({"token": "t", "device": 1, "space": 3, "library": "x"}).to_string()),
    ];
    for (status, body) in answers {
        sign_in(&mut s);
        let mut f = |t: &Task| Done { id: t.id(), status, body: body.clone() };
        s.sync_now_with(5, &mut f);
    }
    // signed in now: garbage snapshots, ops and push answers
    let stack =
        json!({"library": "x", "seq": 1, "catalog": {"photos": {}, "albums": {}, "stacks": {"1": {"id": 1, "photos": [], "collapsed": false}}}});
    for body in ["{}".to_string(), stack.to_string(), json!({"head": 1, "ops": [[1, {"Nope": {}}]]}).to_string(), "[]".into()] {
        let mut f = |t: &Task| Done { id: t.id(), status: 200, body: body.clone() };
        s.sync_now_with(5, &mut f);
        s.sync_soon();
    }
    assert!(s.sync_state().is_some());
    let _ = std::fs::remove_dir_all(&root);
}

fn preset_names(s: &Session) -> Vec<String> {
    let mut v: Vec<String> = s.presets.iter().filter(|p| !p.builtin).map(|p| p.name.clone()).collect();
    v.sort();
    v
}

#[test]
fn presets_sync_and_deletions_stick() {
    let mut f = Fake::new();
    let (mut a, root, ids) = first_device("presets", &mut f);
    a.selection = crate::Selection::single(ids[0]);
    a.execute("preset.create", &json!({"name": "Warm"})).unwrap();
    a.execute("preset.create", &json!({"name": "Cool"})).unwrap();
    sync(&mut a, &mut f);
    assert_eq!(f.presets.version, 1);
    let mut b = open(&root.join("b"));
    sign_in(&mut b);
    sync(&mut b, &mut f);
    assert_eq!(preset_names(&b), ["Cool", "Warm"]);
    // both change them at once: B deletes Warm and adds Matte, A renames Cool
    let warm = b.presets.iter().find(|p| p.name == "Warm").unwrap().id.clone();
    b.execute("preset.delete", &json!({"id": warm})).unwrap();
    b.selection = crate::Selection::single(ids[1]);
    let matte = b.execute("preset.create", &json!({"name": "Matte"})).unwrap();
    assert!(matte["id"].as_str().unwrap().contains(&format!("s{}-", b.sync_state().unwrap().config.space)), "{matte}");
    let cool = a.presets.iter().find(|p| p.name == "Cool").unwrap().id.clone();
    a.execute("preset.rename", &json!({"id": cool, "name": "Cooler"})).unwrap();
    sync(&mut b, &mut f);
    sync(&mut a, &mut f);
    sync(&mut b, &mut f);
    assert_eq!(preset_names(&a), ["Cooler", "Matte"], "the deletion stuck, both other changes landed");
    assert_eq!(preset_names(&b), ["Cooler", "Matte"]);
    // reopened: nothing to send again
    drop(a);
    let mut a = open(&root.join("a"));
    let before = f.requests.len();
    sync(&mut a, &mut f);
    assert!(!f.requests[before..].iter().any(|r| r.contains("presets")), "{:?}", &f.requests[before..]);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn merging_preset_lists() {
    use crate::sync::merge_presets;
    let p = |id: &str, name: &str| json!({"id": id, "name": name, "group": "User Presets", "settings": {}});
    let base = [p("a", "A"), p("b", "B"), p("c", "C")];
    // here: b deleted, c edited, d added; there: a deleted, c edited elsewhere, e added
    let ours = [p("a", "A"), json!({"id": "c", "name": "C", "group": "Mine", "settings": {}}), p("d", "D")];
    let theirs = [p("b", "B"), json!({"id": "c", "name": "C2", "group": "User Presets", "settings": {}}), p("e", "E")];
    let m = merge_presets(&base, &ours, &theirs);
    let ids: Vec<&str> = m.iter().map(|v| v["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["c", "e", "d"]);
    assert_eq!((m[0]["name"].as_str(), m[0]["group"].as_str()), (Some("C2"), Some("Mine")), "both edits of c");
    // deleted on one side but changed on the other: the change stays
    let m = merge_presets(&[p("x", "X")], &[], &[p("x", "X2")]);
    assert_eq!(m.len(), 1);
}

#[test]
fn removals_win_over_pending_edits() {
    let mut f = Fake::new();
    let (mut a, root, ids) = first_device("remove", &mut f);
    let mut b = open(&root.join("b"));
    sign_in(&mut b);
    sync(&mut b, &mut f);
    b.selection = crate::Selection::single(ids[1]);
    b.execute("photo.rate", &json!({"rating": 2})).unwrap();
    a.commit("Delete", a.catalog.delete_permanently_ops(ids[1])).unwrap();
    sync(&mut a, &mut f);
    sync(&mut b, &mut f);
    assert!(b.catalog.photo(ids[1]).is_none());
    assert!(b.sync_state().unwrap().outbox.is_empty());
    assert!(!b.undo.iter().any(|e| matches!(e.op, Op::SetRating { .. })), "undo can't bring back a removed photo's edit");
    let _ = std::fs::remove_dir_all(&root);
}

/// A browser: the library in memory stores, photo files as keys in the host's storage
/// ([`crate::sync::BrowserStore`]), requests answered by the same server.
#[test]
fn a_browser_shares_the_library_through_its_storage() {
    use crate::library::LibraryStores;
    use crate::sync::BrowserStore;
    let mut f = Fake::new();
    let (mut a, root, ids) = first_device("browser", &mut f);
    let mut b = Session::new();
    let stores = LibraryStores { dir: "browser:test".into(), catalog: Box::new(MemStore::new()), files: Box::new(MemStore::new()), on_disk: false };
    b.open_library_in(stores, false).unwrap();
    b.sync_store = Some(BrowserStore { prefix: "proxies/".into(), ..Default::default() });
    sign_in(&mut b);
    // the host: storage keys instead of files, previews built from stored originals
    let mut storage: HashMap<String, Vec<u8>> = HashMap::new();
    let browser = |f: &mut Fake, storage: &mut HashMap<String, Vec<u8>>, t: &Task| -> Done {
        match t {
            Task::Proxies { id, original, smart, mini } => {
                let (s, m) = crate::smart::encode_pair(&storage[original], crate::sync::MINI_EDGE).unwrap();
                storage.insert(smart.clone(), s);
                storage.insert(mini.clone(), m);
                Done { id: *id, status: 200, body: String::new() }
            }
            Task::Http { id, method, url, token, body: Body::File(key), save_to } => {
                let tmp = root.join("upload.tmp");
                std::fs::write(&tmp, &storage[key]).unwrap();
                let t = Task::Http {
                    id: *id,
                    method,
                    url: url.clone(),
                    token: token.clone(),
                    body: Body::File(tmp.to_string_lossy().to_string()),
                    save_to: save_to.clone(),
                };
                f.handle(&t)
            }
            Task::Http { id, method, url, token, body, save_to: Some(key) } => {
                let tmp = root.join("download.tmp");
                let t = Task::Http {
                    id: *id,
                    method,
                    url: url.clone(),
                    token: token.clone(),
                    body: body.clone(),
                    save_to: Some(tmp.to_string_lossy().to_string()),
                };
                let d = f.handle(&t);
                if d.status == 200 {
                    storage.insert(key.clone(), std::fs::read(&tmp).unwrap());
                }
                d
            }
            t => f.handle(t),
        }
    };
    b.sync_now_with(10_000, &mut |t| browser(&mut f, &mut storage, t));
    assert_eq!(b.catalog.len(), 2);
    // opening a photo brings its smart preview down
    b.selection = crate::Selection::single(ids[0]);
    b.sync_now_with(10_000, &mut |t| browser(&mut f, &mut storage, t));
    let key = |s: &Session, id: PhotoId| crate::sync::blob_key(s.catalog.photo(id).unwrap()).unwrap();
    for id in &ids {
        assert!(storage.contains_key(&format!("proxies/{}.lcsm", key(&b, *id))), "mini previews in the browser's storage");
    }
    assert!(storage.contains_key(&format!("proxies/{}.lcsp", key(&b, ids[0]))), "the open photo's smart preview");
    assert!(!storage.keys().any(|k| k.starts_with("originals/")), "no originals unless asked");
    assert_eq!(b.media.synced_tiers.len(), 2, "both are photos without their original here");

    // a photo imported in the browser (stored there by content) is uploaded with its previews
    let png = root.join("c.png");
    write_png(&png, 9);
    let bytes = std::fs::read(&png).unwrap();
    let hash = lightcraft_preview::hash_bytes(&bytes).to_string();
    storage.insert(BrowserStore::original_key(&hash), bytes);
    let id = b.catalog.alloc_photo_id();
    let mut p = lightcraft_catalog::Photo::new(
        id,
        Source::File { path: lightcraft_catalog::sync::original_path(&hash, "c.png") },
        "c.png",
        "PNG",
        96,
        64,
        "t",
    );
    p.content_hash = Some(hash.clone());
    b.commit("Import", Op::AddPhoto { photo: Box::new(p) }).unwrap();
    b.sync_store.as_mut().unwrap().originals.insert(hash.clone());
    b.sync_now_with(10_000, &mut |t| browser(&mut f, &mut storage, t));
    for kind in ["original", "smart", "mini"] {
        assert!(f.blobs.contains_key(&(kind.to_string(), hash.clone())), "{kind} uploaded");
    }
    sync(&mut a, &mut f);
    assert!(a.catalog.photo(id).is_some());
    assert!(root.join("a/Smart Previews").join(crate::sync::mini_file_name(a.catalog.photo(id).unwrap())).exists(), "A gets its preview");
    // an original asked for lands in storage, where the photo's path already points
    b.execute("sync.downloadOriginals", &json!({"ids": [ids[1].0]})).unwrap();
    b.sync_now_with(10_000, &mut |t| browser(&mut f, &mut storage, t));
    assert!(storage.contains_key(&BrowserStore::original_key(&key(&b, ids[1]))));
    assert!(is_remote(b.catalog.photo(ids[1]).unwrap()), "no relink in the browser");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn server_addresses_as_a_phone_keyboard_types_them() {
    let ok = |typed: &str| server_address(typed).unwrap_or_else(|| panic!("{typed}"));
    assert_eq!(ok("photos.example.com"), "https://photos.example.com");
    assert_eq!(ok(" Https://Photos.Example.com/ "), "https://photos.example.com");
    assert_eq!(ok("HTTP://Pi:8080"), "http://pi:8080");
    assert_eq!(ok("http://100.64.0.1:8080/lightcraft/"), "http://100.64.0.1:8080/lightcraft");
    assert_eq!(ok("http://[FD7A::1]:8080"), "http://[fd7a::1]:8080");
    // at home or on the tailnet, without a scheme: plain http (no certificate there)
    assert_eq!(ok("127.0.0.1:8080"), "http://127.0.0.1:8080");
    assert_eq!(ok("192.168.1.20:8080"), "http://192.168.1.20:8080");
    assert_eq!(ok("100.101.102.103:8080"), "http://100.101.102.103:8080");
    assert_eq!(ok("Nas:8080"), "http://nas:8080");
    assert_eq!(ok("photos.local"), "http://photos.local");
    assert_eq!(ok("[fd7a::1]:8080"), "http://[fd7a::1]:8080");
    assert_eq!(ok("8.8.8.8"), "https://8.8.8.8");
    assert_eq!(ok("photos.example.com:8443"), "https://photos.example.com:8443");
    for bad in [
        "",
        "   ",
        "https://",
        "https:",
        "ftp://x",
        "a b.com",
        "https://user@host",
        "https:///path",
        "/",
        "mailto:x@y",
        "host:",
        "host:80a",
        "[::1",
        "[::1]:",
        "[::1]x",
        "[]",
        "https://x.com/?q",
    ] {
        assert_eq!(server_address(bad), None, "{bad:?}");
    }
    // a library signed in to a server stays that server's however its address is typed
    let root = temp_dir("address");
    let mut s = open(&root.join("a"));
    s.sync_sign_in("Photos.Example.com", "ann", "pw", "Phone").unwrap();
    assert_eq!(s.sync_state().unwrap().config.server, "https://photos.example.com");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn storage_numbers_come_from_the_server_and_from_this_computer() {
    let mut f = Fake::new();
    let (mut a, root, _) = first_device("usage", &mut f);
    assert!(a.sync_state().unwrap().usage().is_none(), "nobody asked yet");
    a.sync_fetch_usage_with(&mut |t| f.handle(t));
    let u = a.sync_state().unwrap().usage().cloned().expect("the server answered");
    let bytes = |k: &str| f.blobs.iter().filter(|((kind, _), _)| kind == k).map(|(_, b)| b.len() as u64).sum::<u64>();
    assert_eq!((u.original.files, u.smart.files, u.mini.files), (2, 2, 2));
    assert_eq!(u.original.bytes, bytes("original"));
    assert_eq!(u.stored(), bytes("original") + bytes("smart") + bytes("mini"));
    assert_eq!((u.photos, u.disk), (2, Some(proto::Disk { total: 1000, free: 400 })));
    assert!(a.sync_state().unwrap().usage_error().is_none());

    // the command says both sides: this device made the previews it uploaded
    let r = a.execute("sync.usage", &json!({})).unwrap();
    assert_eq!(r["server"]["original"]["files"], 2, "{r}");
    assert_eq!(r["photos"], json!({"total": 2, "onlyPreviewsHere": 0}));
    assert_eq!(r["local"]["smart"]["files"], 2, "{r}");
    assert_eq!(r["local"]["mini"]["files"], 2, "{r}");
    assert!(r["local"]["library"].as_u64().unwrap() > 0, "the catalog takes room: {r}");

    // a second device has only previews of the photos: the grid's small ones
    let mut b = open(&root.join("b"));
    sign_in(&mut b);
    sync(&mut b, &mut f);
    assert_eq!(b.photo_counts(), (2, 2));
    let r = b.execute("sync.usage", &json!({})).unwrap();
    assert_eq!(r["photos"], json!({"total": 2, "onlyPreviewsHere": 2}));
    assert_eq!(r["local"]["mini"]["files"], 2, "{r}");
    assert_eq!(r["local"]["downloaded"]["files"], 0, "{r}");
    // an original on request is counted as downloaded
    let ids: Vec<u64> = b.catalog.photos().map(|p| p.id.0).collect();
    b.selection = crate::Selection::single(PhotoId(ids[0]));
    b.execute("sync.downloadOriginals", &json!({"ids": ids})).unwrap();
    sync(&mut b, &mut f);
    let r = b.execute("sync.usage", &json!({})).unwrap();
    assert_eq!(r["local"]["downloaded"]["files"], 2, "{r}");
    assert_eq!(r["photos"]["onlyPreviewsHere"], 0, "{r}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn storage_is_asked_for_only_while_someone_looks_and_not_too_often() {
    let mut f = Fake::new();
    let (mut a, root, _) = first_device("usage-ask", &mut f);
    let usage_tasks = |s: &mut Session| {
        s.sync_tasks().into_iter().filter(|t| matches!(t, Task::Http { url, .. } if url.ends_with("/api/usage"))).collect::<Vec<_>>()
    };
    assert!(usage_tasks(&mut a).is_empty(), "nobody is looking");
    a.sync_want_usage();
    let t = usage_tasks(&mut a);
    assert_eq!(t.len(), 1);
    a.sync_want_usage();
    assert!(usage_tasks(&mut a).is_empty(), "one request at a time");
    a.sync_done(f.handle(&t[0]));
    assert!(a.sync_state().unwrap().usage().is_some());
    a.sync_want_usage();
    assert!(usage_tasks(&mut a).is_empty(), "the answer is fresh");
    a.sync_refresh_usage();
    assert_eq!(usage_tasks(&mut a).len(), 1, "Refresh asks at once");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_server_without_the_storage_route_says_so_and_syncing_goes_on() {
    let mut f = Fake::new();
    let (mut a, root, ids) = first_device("usage-old", &mut f);
    f.no_usage = true;
    a.sync_fetch_usage_with(&mut |t| f.handle(t));
    let st = a.sync_state().unwrap();
    assert!(st.usage().is_none());
    assert!(st.usage_error().unwrap().contains("too old"), "{:?}", st.usage_error());
    a.selection = crate::Selection::single(ids[0]);
    a.execute("photo.rate", &json!({"rating": 3})).unwrap();
    let st = sync(&mut a, &mut f);
    assert_eq!(st["state"], "idle", "not an error of the sync itself: {st}");
    // the network being down is told apart too, and the older numbers stay
    f.no_usage = false;
    a.sync_fetch_usage_with(&mut |t| f.handle(t));
    assert!(a.sync_state().unwrap().usage().is_some());
    f.down = true;
    a.sync_fetch_usage_with(&mut |t| f.handle(t));
    let st = a.sync_state().unwrap();
    assert!(st.usage().is_some(), "the last numbers stay");
    assert!(st.usage_error().unwrap().contains("can't reach"), "{:?}", st.usage_error());
    let _ = std::fs::remove_dir_all(&root);
}
