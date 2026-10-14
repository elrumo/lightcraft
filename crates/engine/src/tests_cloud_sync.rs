//! Self-hosted sync, device side: real libraries on disk talking to an in-process server (the
//! catalog's [`ServerCore`] plus a blob map), so every request the engine makes is answered the
//! way `lightcraft-server` answers it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use lightcraft_catalog::sync::{PushError, ServerCore, proto};
use lightcraft_catalog::{MemStore, Op, PhotoId, Source};
use serde_json::{Value, json};

use crate::Session;
use crate::cloud_sync::{BUILDING_RETRY, Body, Done, Task, blob_key, is_remote, server_address};

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
    /// The other shared documents (`/api/docs/<name>`), by name; `no_docs`: a server from before them.
    docs: HashMap<String, proto::Doc>,
    no_docs: bool,
    /// Originals whose upload broke: what the server kept of each, by (kind, hash).
    partials: HashMap<(String, String), Vec<u8>>,
    /// Answer a `HEAD` with this offset whatever is kept (an answer that has gone out of date).
    stale_offset: Option<u64>,
    /// What the server says it is doing (`GET /api/activity`), and a server from before the route.
    activity: proto::Activity,
    no_activity: bool,
    /// Photo files can't be stored (the disk is full): why.
    put_fails: Option<String>,
    /// Photos per page of the library (small, so reads take several pages); `whole_snapshot`: a
    /// server from before paging (the whole library in one answer).
    page: usize,
    whole_snapshot: bool,
    /// Photos (by hash) whose previews the server is still making: asked for, it says so (`404`, `previews: "queued"`).
    building: std::collections::HashSet<String>,
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
            docs: HashMap::new(),
            no_docs: false,
            partials: HashMap::new(),
            stale_offset: None,
            activity: proto::Activity::default(),
            no_activity: false,
            put_fails: None,
            page: 3,
            whole_snapshot: false,
            building: Default::default(),
            requests: vec![],
        }
    }

    fn handle(&mut self, t: &Task) -> Done {
        let (id, method, url, body, save_to, token) = match t {
            Task::Proxies { .. } => return crate::cloud_sync::run(t),
            Task::Http { id, method, url, body, save_to, token } => (*id, *method, url, body, save_to, token),
        };
        if self.down {
            return Done::failed(id, "connection refused");
        }
        let path = url.strip_prefix("http://fake").unwrap();
        let from = if let Body::FileFrom(_, at) = body { format!(" from {at}") } else { String::new() };
        self.requests.push(format!("{method} {path}{from}"));
        let ok = |v: Value| Done { id, status: 200, body: v.to_string() };
        let status = |s: u16, v: Value| Done { id, status: s, body: v.to_string() };
        // (like the real server: everything but signing in and the health check wants a token)
        if token.is_empty() && !matches!(path, "/api/login" | "/api/health") {
            return status(401, json!({"error": "sign in first"}));
        }
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
            ("GET", p) if p.starts_with("/api/snapshot") => {
                let q = p.split_once('?').map_or("", |(_, q)| q);
                let arg = |k: &str| q.split('&').filter_map(|kv| kv.split_once('=')).find(|(key, _)| *key == k).map(|(_, v)| v);
                match arg("limit").filter(|_| !self.whole_snapshot) {
                    Some(limit) => match self.core.snapshot_page(arg("cursor"), limit.parse::<usize>().unwrap().min(self.page)) {
                        Some(page) => {
                            let page = proto::Snapshot { library: self.library.clone(), ..page };
                            assert_eq!(arg("compact"), Some("1"), "devices read compact pages");
                            // (the text as the server writes it: `defaults` first)
                            let body = serde_json::to_string(&lightcraft_catalog::sync::compact_page(page).unwrap()).unwrap();
                            Done { id, status: 200, body }
                        }
                        None => status(410, json!({"error": "gone"})),
                    },
                    None => {
                        let (seq, snap) = self.core.snapshot();
                        let catalog: Value = serde_json::from_str(&snap).unwrap();
                        ok(json!({"library": self.library, "seq": seq, "catalog": catalog}))
                    }
                }
            }
            ("GET", p) if p.starts_with("/api/ops?since=") => {
                let q = p.trim_start_matches("/api/ops?since=");
                let (since, limit) = q.split_once("&limit=").unwrap();
                match self.core.since(since.parse().unwrap(), limit.parse().unwrap()).unwrap() {
                    lightcraft_catalog::sync::Pull::Ops(ops) => {
                        let docs = self.docs.iter().map(|(n, d)| (n.clone(), d.version)).collect();
                        ok(serde_json::to_value(proto::Ops { head: self.core.head(), ops, presets: self.presets.version, docs }).unwrap())
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
            ("GET", p) if p.starts_with("/api/docs/") && !self.no_docs => {
                let doc = self.docs.get(p.trim_start_matches("/api/docs/")).cloned().unwrap_or(proto::Doc { version: 0, items: json!([]) });
                ok(serde_json::to_value(doc).unwrap())
            }
            ("PUT", p) if p.starts_with("/api/docs/") && !self.no_docs => {
                let name = p.trim_start_matches("/api/docs/").to_string();
                let sent: proto::Doc = serde_json::from_str(&json_body()).unwrap();
                let current = self.docs.get(&name).cloned().unwrap_or(proto::Doc { version: 0, items: json!([]) });
                if sent.version != current.version {
                    return status(412, serde_json::to_value(current).unwrap());
                }
                let version = current.version + 1;
                self.docs.insert(name, proto::Doc { version, items: sent.items });
                ok(json!({"version": version}))
            }
            ("GET", "/api/activity") if !self.no_activity => ok(serde_json::to_value(&self.activity).unwrap()),
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
                    "PUT" if self.put_fails.is_some() => status(500, json!({"error": self.put_fails.clone()})),
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
                    "GET" if kind != "original" && self.building.contains(hash) => {
                        status(404, json!({"error": "the server is making this preview", "previews": "queued"}))
                    }
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
            ("GET", "/api/shares") => ok(json!({"shares": []})),
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
    // grid thumbnails render from the mini previews, downloaded as the grid shows them; the loupe
    // from the smart preview once here
    for id in &ids {
        b.thumb_job(*id, 128);
    }
    sync(&mut b, &mut f);
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

/// Another computer's library: `c.png` is its own, `a-again.png` is the file the first device has as `a.png`.
fn second_library(root: &Path) -> Session {
    write_png(&root.join("in2/c.png"), 3);
    write_png(&root.join("in2/a-again.png"), 1);
    let mut c = open(&root.join("c"));
    c.execute("library.import", &json!({"paths": [root.join("in2").to_string_lossy()]})).unwrap();
    assert_eq!(c.catalog.len(), 2);
    c
}

fn photo_named(s: &Session, name: &str) -> PhotoId {
    s.catalog.photos().find(|p| p.file_name == name).unwrap_or_else(|| panic!("no photo {name}")).id
}

#[test]
fn a_library_with_photos_signing_in_to_a_server_that_has_one_waits_for_a_choice() {
    let mut f = Fake::new();
    let (_a, root, _) = first_device("conflict", &mut f);
    let mut c = second_library(&root);
    sign_in(&mut c);
    let before = f.requests.len();
    let st = sync(&mut c, &mut f);
    assert_eq!(st["state"], "conflict", "{st}");
    assert_eq!(st["signedIn"], true, "signed in, not turned away: {st}");
    assert_eq!(st["error"], Value::Null, "{st}");
    let info = &st["conflict"];
    assert_eq!((&info["here"]["photos"], &info["there"]["photos"], &info["shared"]), (&json!(2), &json!(2), &json!(1)), "{info}");
    assert_eq!((info["server"].as_str(), info["user"].as_str()), (Some("fake"), Some("ann")));
    assert_eq!(c.catalog.len(), 2, "nothing changed here");
    assert_eq!(f.core.catalog().len(), 2, "nor there");
    // only the sign-in and the server's library were asked for: nothing pulled, pushed or sent, however long it waits
    assert_eq!(f.requests[before..], ["POST /api/login", "GET /api/snapshot?limit=2000&compact=1"]);
    sync(&mut c, &mut f);
    assert_eq!(f.requests.len(), before + 2);
    // closing the library doesn't lose the question: the server's library is fetched again and asks again
    drop(c);
    let mut c = open(&root.join("c"));
    let st = sync(&mut c, &mut f);
    assert_eq!(st["state"], "conflict", "{st}");
    assert_eq!(c.catalog.len(), 2);
    assert!(f.requests.last().is_some_and(|r| r.starts_with("GET /api/snapshot")), "{:?}", f.requests);
    // a wrong password says so
    let mut d = open(&root.join("d"));
    d.execute("sync.signIn", &json!({"server": "http://fake", "user": "ann", "password": "nope"})).unwrap();
    let st = sync(&mut d, &mut f);
    assert!(st["error"].as_str().unwrap().contains("password"), "{st}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn choosing_to_upload_adds_this_library_to_the_servers_and_keeps_a_copy() {
    let mut f = Fake::new();
    let (mut a, root, ids) = first_device("upload", &mut f);
    let mut c = second_library(&root);
    // what this library knows: its own photo is edited, and it rated the one the server has too
    let own = photo_named(&c, "c.png");
    c.selection = crate::Selection::single(own);
    c.execute("develop.set", &json!({"control": "light.exposure", "value": 0.5})).unwrap();
    let again = photo_named(&c, "a-again.png");
    c.selection = crate::Selection::single(again);
    c.execute("photo.rate", &json!({"rating": 4})).unwrap();
    c.execute("album.create", &json!({"name": "Mine", "addSelected": true})).unwrap();
    sign_in(&mut c);
    assert_eq!(sync(&mut c, &mut f)["state"], "conflict");

    let r = c.execute("sync.resolveConflict", &json!({"choice": "upload"})).unwrap();
    assert_eq!(r["resolved"]["choice"], "upload", "{r}");
    assert_eq!((&r["resolved"]["joined"]["photosAdded"], &r["resolved"]["joined"]["photosShared"]), (&json!(1), &json!(1)), "{r}");
    let backup = r["resolved"]["backup"].as_str().unwrap();
    assert!(root.join("c").join(backup).is_file(), "a copy of the library as it was is kept: {backup}");
    assert_eq!(r["state"], "syncing", "{r}");
    let st = sync(&mut c, &mut f);
    assert_eq!(st["state"], "idle", "{st}");

    // the server has the first device's two photos and this one's own: the shared file once
    assert_eq!(f.core.catalog().len(), 3);
    assert_eq!(c.catalog.len(), 3);
    let on_server = |name: &str| f.core.catalog().photos().find(|p| p.file_name == name).map(|p| p.id);
    let c_id = on_server("c.png").expect("c.png reached the server");
    assert_eq!(c_id.0 >> 32, u64::from(c.sync_state().unwrap().config.space), "under an id of this device's space");
    // …with the edit, and the rating the server's copy lacked; the album with the shared photo in it
    assert_eq!(f.core.catalog().photo(c_id).unwrap().develop.light.exposure, 0.5);
    assert_eq!(f.core.catalog().photo(ids[0]).unwrap().rating, 4);
    let mine: Vec<_> = f.core.catalog().albums().filter(|a| a.name == "Mine").collect();
    assert_eq!((mine.len(), mine[0].photos.clone()), (1, vec![ids[0]]));
    // the new photo's files went up (original and both previews), the shared one's didn't need to
    assert!(f.blobs.keys().filter(|(k, _)| k == "original").count() == 3, "{:?}", f.blobs.keys().collect::<Vec<_>>());
    // this device still shows its own files, and the first device gets the new photo
    assert!(!is_remote(c.catalog.photo(c_id).unwrap()), "own file");
    assert!(!is_remote(c.catalog.photo(ids[0]).unwrap()), "the shared photo shows from this device's file");
    sync(&mut a, &mut f);
    assert_eq!(a.catalog.len(), 3);
    assert_eq!(a.catalog.photo(c_id).unwrap().file_name, "c.png");
    assert!(is_remote(a.catalog.photo(c_id).unwrap()));
    assert!(c.sync_state().unwrap().outbox.is_empty());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn choosing_the_servers_library_replaces_this_one_and_keeps_a_copy() {
    let mut f = Fake::new();
    let (_a, root, ids) = first_device("useserver", &mut f);
    let mut c = second_library(&root);
    let (blobs, head) = (f.blobs.len(), f.core.head());
    // an earlier copy where this one would go (a host whose clock stands still): never overwritten
    let earlier = root.join("c/catalog-before-sync-2026-09-30T12-00-00.snap");
    std::fs::write(&earlier, b"an earlier copy").unwrap();
    sign_in(&mut c);
    assert_eq!(sync(&mut c, &mut f)["state"], "conflict");
    let r = c.execute("sync.resolveConflict", &json!({"choice": "useServer"})).unwrap();
    assert_eq!(r["resolved"]["joined"], Value::Null, "{r}");
    let backup = r["resolved"]["backup"].as_str().unwrap().to_string();
    assert!(backup.ends_with("-2.snap"), "the copy has a name of its own: {backup}");
    assert_eq!(std::fs::read(&earlier).unwrap(), b"an earlier copy");
    let st = sync(&mut c, &mut f);
    assert_eq!(st["state"], "idle", "{st}");
    // this library is the server's: its two photos, nothing of this library's, nothing uploaded
    let mut names: Vec<String> = c.catalog.photos().map(|p| p.file_name.clone()).collect();
    names.sort();
    assert_eq!(names, ["a.png", "b.png"]);
    assert!(c.catalog.photos().all(|p| is_remote(p)));
    assert_eq!((f.blobs.len(), f.core.head()), (blobs, head), "nothing was uploaded");
    assert!(c.catalog.photo(ids[0]).is_some());
    // the old library is in the copy, which opens as a library of its own
    let bytes = std::fs::read(root.join("c").join(&backup)).unwrap();
    let mut store = MemStore::new();
    lightcraft_catalog::Store::write_atomic(&mut store, lightcraft_catalog::journal::SNAPSHOT, &bytes).unwrap();
    let (_, old, _) = lightcraft_catalog::Journal::open(Box::new(store)).unwrap();
    let mut old_names: Vec<String> = old.photos().map(|p| p.file_name.clone()).collect();
    old_names.sort();
    assert_eq!(old_names, ["a-again.png", "c.png"]);
    // the choice is made: asking again says so
    assert!(c.execute("sync.resolveConflict", &json!({"choice": "upload"})).is_err());

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn cancelling_the_choice_signs_out_and_leaves_this_library_alone() {
    let mut f = Fake::new();
    let (_a, root, _) = first_device("cancel", &mut f);
    let mut c = second_library(&root);
    sign_in(&mut c);
    assert_eq!(sync(&mut c, &mut f)["state"], "conflict");
    let before = f.requests.len();
    let r = c.execute("sync.resolveConflict", &json!({"choice": "cancel"})).unwrap();
    assert_eq!((&r["state"], &r["signedIn"], &r["error"]), (&json!("signedOut"), &json!(false), &Value::Null), "{r}");
    assert_eq!(c.catalog.len(), 2);
    assert!(c.sync_state().unwrap().config.token.is_empty(), "the device's token is forgotten");
    assert_eq!(f.requests.len(), before, "it just stops");
    // nothing was copied: nothing changed
    assert!(!std::fs::read_dir(root.join("c")).unwrap().flatten().any(|e| e.file_name().to_string_lossy().starts_with("catalog-before-sync")));
    // signing in again asks again; an unknown choice is an error
    sign_in(&mut c);
    assert_eq!(sync(&mut c, &mut f)["state"], "conflict");
    assert!(c.execute("sync.resolveConflict", &json!({"choice": "merge-ish"})).is_err());
    assert!(c.execute("sync.resolveConflict", &json!({})).is_err());
    assert_eq!(c.sync_conflict().map(|i| i.here.photos), Some(2), "still waiting");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn there_is_nothing_to_resolve_without_a_waiting_choice() {
    let mut f = Fake::new();
    let (mut a, root, _) = first_device("noconflict", &mut f);
    assert!(a.execute("sync.resolveConflict", &json!({"choice": "upload"})).is_err());
    let c = second_library(&root);
    // not signed in at all
    let mut c = c;
    assert!(c.execute("sync.resolveConflict", &json!({"choice": "cancel"})).is_err());
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
    (crate::cloud_sync::blob_key(p).unwrap(), std::fs::read(path).unwrap())
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

/// Where the browser has a synced photo's preview in memory but not its original, the commands that read pixels
/// (auto tone, auto white balance…) read the preview instead of failing "the original is still loading".
#[test]
fn pixel_commands_read_the_preview_when_the_original_is_not_here() {
    use crate::media::SourceLevel;
    let mut f = Fake::new();
    let (_a, root, ids) = first_device("proxyonly", &mut f);
    let mut b = open(&root.join("b"));
    sign_in(&mut b);
    sync(&mut b, &mut f);
    b.selection = crate::Selection::single(ids[0]);
    sync(&mut b, &mut f);
    let smart = std::fs::read(root.join("b/Smart Previews").join(crate::smart::file_name(b.catalog.photo(ids[0]).unwrap()))).unwrap();
    // the browser: no previews folder, and no decoder for the original, which isn't in memory
    b.media.smart_dir = None;
    assert!(b.source_now(ids[0], SourceLevel::Thumb).is_err(), "nothing to read without the preview");
    let bytes: std::sync::Arc<[u8]> = smart.into();
    let asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = asked.clone();
    b.media.proxy_bytes = Some(std::sync::Arc::new(move |hash: &str, edge: usize| {
        log.lock().unwrap().push((hash.to_string(), edge));
        Some(bytes.clone())
    }));
    let img = b.source_now(ids[0], SourceLevel::Thumb).unwrap();
    assert_eq!((img.width, img.height), (96, 64), "the photo's own size: it is smaller than the preview's edge");
    let key = crate::cloud_sync::blob_key(b.catalog.photo(ids[0]).unwrap()).unwrap();
    assert_eq!(asked.lock().unwrap().first(), Some(&(key, 512)), "asked by content hash and the edge wanted");
    // a preview that doesn't decode is the original error, not a crash
    b.media.proxy_bytes = Some(std::sync::Arc::new(|_: &str, _: usize| Some(std::sync::Arc::from(&b"not a preview"[..]))));
    assert!(b.source_now(ids[1], SourceLevel::Preview).is_err());
    let _ = std::fs::remove_dir_all(&root);
}

/// A browser exports a photo it holds only the preview of: up to the preview's size from the preview, and
/// beyond it the message says what to do (instead of failing deep in the render).
#[test]
fn a_browser_exports_from_the_preview_and_says_what_to_do_beyond_its_size() {
    use crate::export::{ExportFormat, ExportOptions, Resize, export_photo};
    let mut f = Fake::new();
    let (_a, root, ids) = first_device("browserexport", &mut f);
    let mut b = open(&root.join("b"));
    sign_in(&mut b);
    sync(&mut b, &mut f);
    b.selection = crate::Selection::single(ids[0]);
    sync(&mut b, &mut f);
    let smart = std::fs::read(root.join("b/Smart Previews").join(crate::smart::file_name(b.catalog.photo(ids[0]).unwrap()))).unwrap();
    // the browser: no previews folder, no originals in its storage, the preview in memory
    b.media.smart_dir = None;
    b.sync_store = Some(crate::cloud_sync::BrowserStore { prefix: "proxies/".into(), ..Default::default() });
    let bytes: std::sync::Arc<[u8]> = smart.into();
    b.media.proxy_bytes = Some(std::sync::Arc::new(move |_: &str, _: usize| Some(bytes.clone())));
    let small = ExportOptions { format: ExportFormat::Jpeg, resize: Some(Resize { value: 48.0, ..Default::default() }), ..Default::default() };
    let e = export_photo(&mut b, ids[0], &small, 1).unwrap();
    assert!(e.width <= 48 && e.width > 0 && e.bytes.starts_with(&[0xFF, 0xD8]), "{}x{}", e.width, e.height);
    let big = ExportOptions { resize: Some(Resize { value: 4000.0, dont_enlarge: false, ..Default::default() }), ..small };
    let err = export_photo(&mut b, ids[0], &big, 1).err().unwrap();
    assert!(err.contains("Download Originals") && err.contains("2560"), "{err}");
    let _ = std::fs::remove_dir_all(&root);
}

fn snapshots(f: &Fake) -> usize {
    f.requests.iter().filter(|r| r.starts_with("GET /api/snapshot") && !r.contains("cursor=")).count()
}

/// The server's library is read in pages, all from the copy the first page came from: a change
/// another device makes meanwhile arrives after it, as a change. A copy that is gone is read again
/// from the start, and a server from before paging answers it whole.
#[test]
fn the_library_is_read_in_pages() {
    let mut f = Fake::new();
    f.page = 1;
    let (_a, root, ids) = first_device("pages", &mut f);
    let reads = |f: &Fake, from: usize| f.requests[from..].iter().filter(|r| r.starts_with("GET /api/snapshot")).cloned().collect::<Vec<_>>();
    let mut b = open(&root.join("b"));
    let before = f.requests.len();
    sign_in(&mut b);
    // (syncs `s`, calling `half_way` while the library is half read; how far it got)
    fn read(s: &mut Session, f: &mut Fake, half_way: &mut dyn FnMut(&mut Fake)) -> Option<(usize, usize)> {
        let mut progress = None;
        s.sync_soon();
        for _ in 0..100 {
            let tasks = s.sync_tasks();
            if tasks.is_empty() {
                break;
            }
            for t in tasks {
                let d = f.handle(&t);
                s.sync_done(d);
                if let Some(r) = s.sync_state().unwrap().reading() {
                    progress = Some(r);
                    half_way(f);
                }
            }
        }
        progress
    }
    // another device rates a photo half-way through
    let progress = read(&mut b, &mut f, &mut |f: &mut Fake| {
        if f.core.catalog().photo(ids[1]).unwrap().rating == 0 {
            f.core.push(f.core.head(), &[Op::SetRating { id: ids[1], rating: 5 }]).unwrap();
        }
    });
    assert_eq!(progress, Some((1, 2)), "the status says how far it got");
    let r = reads(&f, before);
    assert_eq!(r.len(), 2, "{r:?}");
    assert!(r[1].contains("&cursor="), "{r:?}");
    assert_eq!(b.sync_state().unwrap().reading(), None);
    assert_eq!(b.catalog.photos().filter(|p| p.in_library()).count(), 2);
    assert_eq!(b.catalog.photo(ids[1]).unwrap().rating, 5, "the change made half-way came after");
    // the copy being read goes (newer readers took its place): read again from the start
    let mut c = open(&root.join("c"));
    sign_in(&mut c);
    let before = f.requests.len();
    read(&mut c, &mut f, &mut |f: &mut Fake| {
        if f.core.catalog().photo(ids[0]).unwrap().rating == 0 {
            for rating in 1..=4 {
                f.core.push(f.core.head(), &[Op::SetRating { id: ids[0], rating }]).unwrap();
                f.core.snapshot_page(None, 1).unwrap();
            }
        }
    });
    let r = reads(&f, before);
    assert_eq!(r.iter().filter(|r| !r.contains("cursor=")).count(), 2, "{r:?}");
    assert_eq!(c.sync_state().unwrap().error(), None);
    assert_eq!(c.catalog.photo(ids[0]).unwrap().rating, 4);
    // a server from before paging
    f.whole_snapshot = true;
    let mut d = open(&root.join("d"));
    sign_in(&mut d);
    let before = f.requests.len();
    let st = sync(&mut d, &mut f);
    assert_eq!(st["state"], "idle", "{st}");
    assert_eq!(reads(&f, before).len(), 1);
    assert_eq!((d.catalog.photo(ids[0]).unwrap().rating, d.catalog.photo(ids[1]).unwrap().rating), (4, 5));
    let _ = std::fs::remove_dir_all(&root);
}

/// Sync `s` until `stop` holds (or there is nothing left to do).
fn sync_until(s: &mut Session, f: &mut Fake, stop: &dyn Fn(&Session) -> bool) {
    s.sync_soon();
    for _ in 0..100 {
        let tasks = s.sync_tasks();
        if tasks.is_empty() {
            return;
        }
        for t in tasks {
            let d = f.handle(&t);
            s.sync_done(d);
            if stop(s) {
                return;
            }
        }
    }
}

/// A device joining with nothing of its own fills in from the server's library page by page:
/// photos show (and their previews download) as they come; a restart carries on where it was;
/// changes made meanwhile survive the read starting over. A device with photos of its own reads
/// the whole library before it is asked what to do with them.
#[test]
fn a_library_fills_in_as_it_comes() {
    let mut f = Fake::new();
    f.page = 1;
    let (_a, root, ids) = first_device("fill", &mut f);
    let in_library = |s: &Session| s.catalog.photos().filter(|p| p.in_library()).count();
    let mut b = open(&root.join("b"));
    sign_in(&mut b);
    let before = f.requests.len();
    sync_until(&mut b, &mut f, &|s| s.sync_state().unwrap().reading().is_some());
    assert_eq!((in_library(&b), b.sync_state().unwrap().reading()), (1, Some((1, 2))), "the first page shows");
    assert_eq!(b.sync_state().unwrap().state(), "syncing");
    // its preview downloads alongside the rest of the library once the grid shows it
    let shown = b.catalog.photos().find(|p| p.in_library()).unwrap().id;
    b.thumb_job(shown, 128);
    let next = b.sync_tasks();
    let urls: Vec<&String> = next.iter().filter_map(|t| if let Task::Http { url, .. } = t { Some(url) } else { None }).collect();
    assert!(urls.iter().any(|u| u.contains("cursor=")) && urls.iter().any(|u| u.contains("/api/blobs/mini/")), "{urls:?}");
    for t in next {
        let d = f.handle(&t);
        b.sync_done(d);
    }
    sync_until(&mut b, &mut f, &|_| false);
    assert!(f.requests[before..].iter().any(|q| q.contains("cursor=")));
    assert_eq!((in_library(&b), b.sync_state().unwrap().reading()), (2, None));
    assert_eq!(b.sync_state().unwrap().config.cursor, f.core.head());

    // the app quits after the first page: it carries on from the second
    let mut c = open(&root.join("c"));
    sign_in(&mut c);
    sync_until(&mut c, &mut f, &|s| s.sync_state().unwrap().reading().is_some());
    drop(c);
    let mut c = open(&root.join("c"));
    assert_eq!((in_library(&c), c.sync_state().unwrap().reading()), (1, Some((1, 2))), "kept on disk");
    let before = f.requests.len();
    assert_eq!(sync(&mut c, &mut f)["state"], "idle");
    let r: Vec<_> = f.requests[before..].iter().filter(|q| q.contains("/api/snapshot")).collect();
    assert!(r.len() == 1 && r[0].contains("cursor="), "{r:?}");
    assert_eq!(in_library(&c), 2);

    // a change made while the library fills in survives the read starting over
    let mut d = open(&root.join("d"));
    sign_in(&mut d);
    sync_until(&mut d, &mut f, &|s| s.sync_state().unwrap().reading().is_some());
    let first = d.catalog.photos().find(|p| p.in_library()).unwrap().id;
    d.selection = crate::Selection::single(first);
    d.execute("photo.rate", &json!({"rating": 2})).unwrap();
    let other = if first == ids[0] { ids[1] } else { ids[0] };
    for rating in 1..=4 {
        // (newer readers: the copy D reads is dropped)
        f.core.push(f.core.head(), &[Op::SetRating { id: other, rating }]).unwrap();
        f.core.snapshot_page(None, 1).unwrap();
    }
    assert_eq!(sync(&mut d, &mut f)["state"], "idle");
    assert_eq!((d.catalog.photo(first).unwrap().rating, d.catalog.photo(other).unwrap().rating), (2, 4));
    assert_eq!(f.core.catalog().photo(first).unwrap().rating, 2, "and it reached the server");

    // photos of its own: the whole library first, then the choice
    let mut e = open(&root.join("e"));
    write_png(&root.join("in-e/x.png"), 9);
    e.execute("library.import", &json!({"paths": [root.join("in-e").to_string_lossy()]})).unwrap();
    sign_in(&mut e);
    let st = sync(&mut e, &mut f);
    assert_eq!((st["state"].as_str(), st["conflict"]["there"]["photos"].as_u64()), (Some("conflict"), Some(2)), "{st}");
    assert_eq!(in_library(&e), 1, "nothing of the server's shows before the choice");
    let _ = std::fs::remove_dir_all(&root);
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

/// An XMP preset named `name` (hand-written for these tests).
fn xmp_look(name: &str, exposure: &str) -> String {
    format!(
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
      <rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:PresetType="Normal"
        crs:Name="{name}" crs:Group="Film" crs:Exposure2012="{exposure}"/></rdf:RDF></x:xmpmeta>"#
    )
}

fn import_look(s: &mut Session, name: &str, exposure: &str) -> String {
    let files = crate::inline_files::to_param([("look.xmp", xmp_look(name, exposure).as_bytes())]);
    let r = s.execute("preset.import", &json!({"files": files})).unwrap();
    r["imported"][0]["id"].as_str().map(str::to_string).unwrap_or_default()
}

/// Imported presets reach the other devices. Two devices importing different presets that share a
/// name keep both, and the same preset imported on both stays one.
#[test]
fn imported_presets_sync_without_replacing_each_other() {
    let mut f = Fake::new();
    let (mut a, mut b, root) = two_devices("presets-imported", &mut f);
    let portra = import_look(&mut a, "Portra", "+0.30");
    sync(&mut a, &mut f);
    sync(&mut b, &mut f);
    let on_b = b.presets.iter().find(|p| p.id == portra).expect("A's import reached B");
    assert_eq!((on_b.name.as_str(), on_b.group.as_str()), ("Portra", "Film"));
    // both import a "Gold" of their own before syncing, and the same "Velvia"
    let gold_a = import_look(&mut a, "Gold", "+0.10");
    let gold_b = import_look(&mut b, "Gold", "+0.90");
    assert_ne!(gold_a, gold_b);
    assert_eq!(import_look(&mut a, "Velvia", "-0.20"), import_look(&mut b, "Velvia", "-0.20"));
    sync(&mut a, &mut f);
    sync(&mut b, &mut f);
    sync(&mut a, &mut f);
    for s in [&a, &b] {
        assert_eq!(preset_names(s), ["Gold", "Gold", "Portra", "Velvia"]);
        let gold: Vec<f64> = s.presets.iter().filter(|p| p.name == "Gold").filter_map(|p| p.settings["light"]["exposure"].as_f64()).collect();
        assert!(gold.contains(&0.1) && gold.contains(&0.9), "each keeps its own settings: {gold:?}");
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// A preset collection imported whole (a Lightroom one is ~11 000 presets): `n` small presets in
/// groups of 100.
fn collection(n: usize) -> Vec<lightcraft_develop::Preset> {
    (0..n)
        .map(|i| lightcraft_develop::Preset {
            id: String::new(),
            name: format!("Look {i}"),
            group: format!("Collection {}", i / 100),
            settings: json!({"light": {"exposure": (i % 40) as f64 / 10.0 - 2.0, "contrast": (i % 50) as f64}}),
            favorite: false,
            builtin: false,
        })
        .collect()
}

fn user_preset_count(s: &Session) -> usize {
    s.presets.iter().filter(|p| !p.builtin).count()
}

/// A library that holds a whole preset collection (more than the 10 000 presets a device used to
/// take from the server) imports more: they reach the other device, and nothing is deleted anywhere.
/// Before, the other device kept the first 10 000, sent that back, and the server and the first
/// device deleted the rest: the newest imports first.
#[test]
fn presets_beyond_ten_thousand_reach_the_other_devices_and_none_are_lost() {
    let mut f = Fake::new();
    let (mut a, mut b, root) = two_devices("presets-collection", &mut f);
    assert_eq!(a.add_presets(collection(10_000)).len(), 10_000);
    a.persist().unwrap();
    sync(&mut a, &mut f);
    sync(&mut b, &mut f);
    assert_eq!(user_preset_count(&b), 10_000);
    // more presets imported on the desktop, after the collection
    let portra = import_look(&mut a, "Portra", "+0.30");
    sync(&mut a, &mut f);
    sync(&mut b, &mut f);
    sync(&mut a, &mut f);
    assert!(b.presets.iter().any(|p| p.id == portra), "the new import reached B");
    assert_eq!(f.presets.presets.as_array().map(Vec::len), Some(10_001), "the server lost none");
    assert_eq!((user_preset_count(&a), user_preset_count(&b)), (10_001, 10_001));
    let _ = std::fs::remove_dir_all(&root);
}

/// A preset a device can't hold (past its limit, not readable as a preset, a built-in's id) is left
/// out there but sent back as it came: it stays on the server for the other devices. One it held and
/// deleted is deleted; its own presets always stay.
#[test]
fn presets_a_device_leaves_out_are_sent_back_not_deleted() {
    use std::collections::HashSet;

    use crate::cloud_sync::{hold_presets, presets_left_out};
    let p = |id: &str| json!({"id": id, "name": id.to_uppercase(), "group": "G", "settings": {}});
    let builtin: HashSet<String> = ["lc.warm".to_string()].into();
    let unreadable = json!({"id": "x", "name": 3});
    let doc = vec![p("a"), p("b"), unreadable.clone(), p("lc.warm"), p("c"), p("d"), p("a")];
    // holding at most 3 with `a` already its own: a, b, c (d is past the limit; the second `a` repeats)
    let own: HashSet<String> = ["a".to_string()].into();
    let held: Vec<String> = hold_presets(&doc, &builtin, &own, 3).into_iter().map(|p| p.id).collect();
    assert_eq!(held, ["a", "b", "c"]);
    let own: HashSet<String> = held.iter().cloned().collect();
    let left = presets_left_out(&doc, &builtin, &own, 3);
    assert_eq!(left, vec![unreadable.clone(), p("lc.warm"), p("d")], "sent back as they came");
    // b deleted here: not sent back (it's within the limit), the others still are
    let own: HashSet<String> = ["a".to_string(), "c".to_string()].into();
    assert_eq!(presets_left_out(&doc, &builtin, &own, 3), vec![unreadable, p("lc.warm"), p("d")]);
    // own presets stay even past the limit
    let own: HashSet<String> = ["c".to_string(), "d".to_string()].into();
    let held: Vec<String> = hold_presets(&doc, &builtin, &own, 2).into_iter().map(|p| p.id).collect();
    assert_eq!(held, ["c", "d"]);
}

/// The same through a device: what it can't read stays on the server when it sends its own changes.
#[test]
fn a_device_keeps_on_the_server_what_it_cannot_read() {
    let mut f = Fake::new();
    let (mut a, root, ids) = first_device("presets-unreadable", &mut f);
    // (from a newer app, say: not a preset this version can read)
    let odd = json!({"id": "user.future", "name": "Future", "group": "Later", "settings": {}, "kind": 7, "favorite": "yes"});
    f.presets = proto::Presets { version: 1, presets: json!([odd.clone()]) };
    sync(&mut a, &mut f);
    assert_eq!(user_preset_count(&a), 0, "nothing this device can hold");
    a.selection = crate::Selection::single(ids[0]);
    a.execute("preset.create", &json!({"name": "Warm"})).unwrap();
    sync(&mut a, &mut f);
    let on_server = f.presets.presets.as_array().cloned().unwrap_or_default();
    assert!(on_server.contains(&odd), "{on_server:?}");
    assert!(on_server.iter().any(|v| v["name"] == "Warm"), "{on_server:?}");
    // and nothing more to send after that
    let before = f.requests.len();
    sync(&mut a, &mut f);
    assert!(!f.requests[before..].iter().any(|r| r.starts_with("PUT /api/presets")), "{:?}", &f.requests[before..]);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn merging_preset_lists() {
    use crate::cloud_sync::merge_presets;
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

/// Mini previews download as the grid asks for them, the cells drawn last first, and only those: a
/// cell scrolled out of the grid's reach before its turn isn't downloaded.
#[test]
fn only_the_previews_the_grid_shows_download() {
    let mut f = Fake::new();
    let root = temp_dir("onscreen");
    for i in 0..8u8 {
        write_png(&root.join(format!("in/{i}.png")), i * 20);
    }
    let mut a = open(&root.join("a"));
    a.execute("library.import", &json!({"paths": [root.join("in").to_string_lossy()]})).unwrap();
    sign_in(&mut a);
    assert_eq!(sync(&mut a, &mut f)["state"], "idle");
    let mut b = open(&root.join("b"));
    sign_in(&mut b);
    let before = f.requests.len();
    assert_eq!(sync(&mut b, &mut f)["state"], "idle");
    assert!(!f.requests[before..].iter().any(|r| r.contains("/blobs/")), "nothing shown, nothing downloaded: {:?}", &f.requests[before..]);
    let mini = |t: &Task| match t {
        Task::Http { url, .. } => url.split_once("/api/blobs/mini/").map(|(_, key)| key.to_string()),
        Task::Proxies { .. } => None,
    };
    let ids: Vec<PhotoId> = b.catalog.photos().map(|p| p.id).collect();
    let keys = |s: &Session, ids: &[PhotoId]| -> Vec<String> { ids.iter().map(|id| blob_key(s.catalog.photo(*id).unwrap()).unwrap()).collect() };
    // the grid draws six cells: the first four go at once, in the order drawn
    for id in &ids[..6] {
        b.thumb_job(*id, 128);
    }
    let held = b.sync_tasks();
    assert_eq!(held.iter().filter_map(mini).collect::<Vec<_>>(), keys(&b, &ids[..4]));
    // while they come, the grid scrolls on: the fifth and sixth cells leave its reach, the last two come
    for _ in 0..40 {
        b.thumb_job(ids[6], 128);
        b.thumb_job(ids[7], 128);
        assert!(b.sync_tasks().iter().all(|t| mini(t).is_none()), "four at a time");
    }
    // a preview arriving changes no photo: the library's lists (a sort of all of it) stay as they are
    let (revision, generation) = (b.catalog.revision, b.visible_shared().0);
    for t in held {
        let d = f.handle(&t);
        b.sync_done(d);
    }
    b.thumb_job(ids[6], 128);
    b.thumb_job(ids[7], 128);
    let next = b.sync_tasks();
    assert_eq!((b.catalog.revision, b.visible_shared().0), (revision, generation));
    assert_eq!(next.iter().filter_map(mini).collect::<Vec<_>>(), keys(&b, &ids[6..]));
    for t in next {
        let d = f.handle(&t);
        b.sync_done(d);
    }
    let tiers: Vec<Option<u8>> = ids.iter().map(|id| b.synced_tier(b.catalog.photo(*id).unwrap())).collect();
    assert_eq!(tiers, [1, 1, 1, 1, 0, 0, 1, 1].map(Some));
    let _ = std::fs::remove_dir_all(&root);
}

/// The grid builds a cell's render job once; the render that fails for want of the preview is remembered, so it
/// isn't asked for again by itself. A cell that is still shown keeps its download through [`Session::note_thumb_ask`],
/// every frame, however long the ones before it take.
#[test]
fn cells_still_shown_are_downloaded_in_turn_however_slow_the_queue() {
    let mut f = Fake::new();
    let root = temp_dir("shown-all");
    for i in 0..8u8 {
        write_png(&root.join(format!("in/{i}.png")), i * 20);
    }
    let mut a = open(&root.join("a"));
    a.execute("library.import", &json!({"paths": [root.join("in").to_string_lossy()]})).unwrap();
    sign_in(&mut a);
    assert_eq!(sync(&mut a, &mut f)["state"], "idle");
    let mut b = open(&root.join("b"));
    sign_in(&mut b);
    assert_eq!(sync(&mut b, &mut f)["state"], "idle");
    let mini = |t: &Task| matches!(t, Task::Http { url, .. } if url.contains("/api/blobs/mini/"));
    let ids: Vec<PhotoId> = b.catalog.photos().map(|p| p.id).collect();
    for id in &ids {
        b.thumb_job(*id, 128);
    }
    let mut held = b.sync_tasks();
    assert_eq!(held.iter().filter(|t| mini(t)).count(), 4);
    // each batch of four takes forty frames, in which the grid asks for nothing but what it still shows
    for round in 0.. {
        assert!(round < 8, "eight photos, four at a time");
        for _ in 0..40 {
            for id in &ids {
                b.note_thumb_ask(*id, true);
            }
            assert!(b.sync_tasks().iter().all(|t| !mini(t)), "four at a time");
        }
        for t in std::mem::take(&mut held) {
            let d = f.handle(&t);
            b.sync_done(d);
        }
        for id in &ids {
            b.note_thumb_ask(*id, true);
        }
        held = b.sync_tasks();
        if held.is_empty() {
            break;
        }
    }
    let tiers: Vec<Option<u8>> = ids.iter().map(|id| b.synced_tier(b.catalog.photo(*id).unwrap())).collect();
    assert_eq!(tiers, vec![Some(1); 8], "every cell that stayed on screen got its preview");
    // and a photo that has its preview is no ask, whatever the host does
    b.note_thumb_ask(ids[0], true);
    assert!(b.media.thumb_asks.is_empty(), "{:?}", b.media.thumb_asks);
    // a host that remembers a render that failed for want of a preview is told which ones came, once (the library
    // itself didn't change)
    let revision = b.catalog.revision;
    let mut arrived = b.media.take_arrived().expect("a few");
    arrived.sort();
    let mut want = ids.clone();
    want.sort();
    assert_eq!(arrived, want);
    assert_eq!(b.media.take_arrived(), Some(vec![]));
    assert_eq!(b.catalog.revision, revision);
    let _ = std::fs::remove_dir_all(&root);
}

/// The cells on screen come down before the ones only in the grid's reach above and below, whatever was drawn first.
#[test]
fn cells_on_screen_download_before_those_in_the_grids_reach() {
    let mut f = Fake::new();
    let root = temp_dir("shown-first");
    for i in 0..6u8 {
        write_png(&root.join(format!("in/{i}.png")), i * 30);
    }
    let mut a = open(&root.join("a"));
    a.execute("library.import", &json!({"paths": [root.join("in").to_string_lossy()]})).unwrap();
    sign_in(&mut a);
    assert_eq!(sync(&mut a, &mut f)["state"], "idle");
    let mut b = open(&root.join("b"));
    sign_in(&mut b);
    assert_eq!(sync(&mut b, &mut f)["state"], "idle");
    let ids: Vec<PhotoId> = b.catalog.photos().map(|p| p.id).collect();
    let keys: Vec<String> = ids.iter().map(|id| blob_key(b.catalog.photo(*id).unwrap()).unwrap()).collect();
    // the grid draws its reach above the screen first, then the screen, then the reach below it
    for (i, id) in ids.iter().enumerate() {
        b.thumb_job_for(*id, 128, (2..5).contains(&i));
    }
    let asked: Vec<String> = b
        .sync_tasks()
        .iter()
        .filter_map(|t| match t {
            Task::Http { url, .. } => url.split_once("/api/blobs/mini/").map(|(_, key)| key.to_string()),
            Task::Proxies { .. } => None,
        })
        .collect();
    assert_eq!(asked, [&keys[2], &keys[3], &keys[4], &keys[0]].map(Clone::clone), "the screen's three, then the first of the reach");
    let _ = std::fs::remove_dir_all(&root);
}

/// A preview the server says it is making is not a failed transfer: nothing is reported, and it is asked for again
/// after seconds, not after the minute a file the server doesn't have waits. A server from before the answer (a plain
/// `404`) is waited out the old way.
#[test]
fn a_preview_the_server_is_building_is_asked_for_again_soon() {
    let mut f = Fake::new();
    let (_a, root, ids) = first_device("building", &mut f);
    let mut b = open(&root.join("b"));
    sign_in(&mut b);
    sync(&mut b, &mut f);
    let key = |s: &Session, id: PhotoId| blob_key(s.catalog.photo(id).unwrap()).unwrap();
    // the server is making the first photo's previews, and never heard of the second's
    f.building.insert(key(&b, ids[0]));
    f.blobs.remove(&("mini".to_string(), key(&b, ids[1])));
    let minis = |tasks: &[Task]| tasks.iter().filter(|t| matches!(t, Task::Http { url, .. } if url.contains("/api/blobs/mini/"))).count();
    for id in &ids {
        b.thumb_job(*id, 128);
    }
    let held = b.sync_tasks();
    assert_eq!(minis(&held), 2);
    for t in &held {
        let d = f.handle(t);
        b.sync_done(d);
    }
    let t = b.sync_transfers();
    assert_eq!((t.failing, t.error.is_some()), (1, true), "only the photo the server doesn't have failed: {t:?}");
    let wait = b.sync_building_wait(ids[0]).expect("waits");
    assert!(wait > Duration::from_secs(1) && wait <= BUILDING_RETRY, "{wait:?}");
    // (the cells are drawn on: nothing is asked for before the wait is over)
    for id in &ids {
        b.note_thumb_ask(*id, true);
    }
    assert_eq!(minis(&b.sync_tasks()), 0);
    // the server made them meanwhile
    f.building.clear();
    b.sync_end_building_waits();
    for id in &ids {
        b.note_thumb_ask(*id, true);
    }
    let next = b.sync_tasks();
    assert_eq!(minis(&next), 1, "the first photo's preview is asked for again; the second waits its minute");
    for t in &next {
        let d = f.handle(t);
        b.sync_done(d);
    }
    assert_eq!(b.synced_tier(b.catalog.photo(ids[0]).unwrap()), Some(1));
    assert_eq!(b.sync_transfers().failing, 1);
    let _ = std::fs::remove_dir_all(&root);
}

/// A browser: the library in memory stores, photo files as keys in the host's storage
/// ([`crate::cloud_sync::BrowserStore`]), requests answered by the same server.
#[test]
fn a_browser_shares_the_library_through_its_storage() {
    use crate::cloud_sync::BrowserStore;
    use crate::library::LibraryStores;
    let mut f = Fake::new();
    let (mut a, root, ids) = first_device("browser", &mut f);
    let mut b = Session::new();
    let stores = LibraryStores { dir: "browser:test".into(), catalog: Box::new(MemStore::new()), files: Box::new(MemStore::new()), on_disk: false };
    b.open_library_in(stores, false).unwrap();
    b.sync_store = Some(BrowserStore { prefix: "proxies/".into(), ..Default::default() });
    sign_in(&mut b);
    // (every mini preview, not only the grid's)
    b.execute("sync.allPreviews", &json!({"on": true})).unwrap();
    // the host: storage keys instead of files, previews built from stored originals
    let mut storage: HashMap<String, Vec<u8>> = HashMap::new();
    let browser = |f: &mut Fake, storage: &mut HashMap<String, Vec<u8>>, t: &Task| -> Done {
        match t {
            Task::Proxies { id, original, smart, mini } => {
                let (s, m) = crate::smart::encode_pair(&storage[original], crate::cloud_sync::MINI_EDGE).unwrap();
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
    let key = |s: &Session, id: PhotoId| crate::cloud_sync::blob_key(s.catalog.photo(id).unwrap()).unwrap();
    for id in &ids {
        assert!(storage.contains_key(&format!("proxies/{}.lcsm", key(&b, *id))), "mini previews in the browser's storage");
    }
    assert!(storage.contains_key(&format!("proxies/{}.lcsp", key(&b, ids[0]))), "the open photo's smart preview");
    assert!(!storage.keys().any(|k| k.starts_with("originals/")), "no originals unless asked");
    let tiers: Vec<Option<u8>> = ids.iter().map(|id| b.synced_tier(b.catalog.photo(*id).unwrap())).collect();
    assert_eq!(tiers, [Some(2), Some(1)], "both are photos without their original here: the open one has its smart preview");

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
    a.thumb_job(id, 128);
    sync(&mut a, &mut f);
    assert!(root.join("a/Smart Previews").join(crate::cloud_sync::mini_file_name(a.catalog.photo(id).unwrap())).exists(), "A gets its preview");
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
    let mini_bytes = bytes("mini");
    assert_eq!((u.original.files, u.smart.files, u.mini.files), (2, 2, 2));
    assert_eq!(u.original.bytes, bytes("original"));
    assert_eq!(u.stored(), bytes("original") + bytes("smart") + bytes("mini"));
    assert_eq!(u.total(), u.stored(), "this server reads no library folders");
    assert_eq!((u.photos, u.disk), (2, Some(proto::Disk { total: 1000, free: 400 })));
    assert!(a.sync_state().unwrap().usage_error().is_none());

    // the command says both sides: this device made the previews it uploaded
    let r = a.execute("sync.usage", &json!({})).unwrap();
    assert_eq!(r["server"]["original"]["files"], 2, "{r}");
    assert_eq!(r["serverTotal"].as_u64(), Some(u.total()), "{r}");
    assert_eq!(r["photos"], json!({"total": 2, "onlyPreviewsHere": 0}));
    assert_eq!(r["local"]["smart"]["files"], 2, "{r}");
    assert_eq!(r["local"]["mini"]["files"], 2, "{r}");
    assert!(r["local"]["library"].as_u64().unwrap() > 0, "the catalog takes room: {r}");

    assert_eq!(r["previewsLeft"]["photos"], 0, "{r}");

    // a second device has only previews of the photos: the grid's small ones, once it shows them
    let mut b = open(&root.join("b"));
    sign_in(&mut b);
    sync(&mut b, &mut f);
    assert_eq!(b.photo_counts(), (2, 2));
    let r = b.execute("sync.usage", &json!({})).unwrap();
    assert_eq!(r["photos"], json!({"total": 2, "onlyPreviewsHere": 2}));
    assert_eq!(r["local"]["mini"]["files"], 0, "nothing shown, nothing downloaded: {r}");
    // all of them on request, after an estimate at what the server's weigh
    b.sync_fetch_usage_with(&mut |t| f.handle(t));
    let r = b.execute("sync.usage", &json!({})).unwrap();
    assert_eq!(r["previewsLeft"], json!({"photos": 2, "bytes": 2 * (mini_bytes / 2)}), "{r}");
    assert_eq!(b.execute("sync.allPreviews", &json!({"on": true})).unwrap()["allPreviews"], true);
    sync(&mut b, &mut f);
    let r = b.execute("sync.usage", &json!({})).unwrap();
    assert_eq!(r["local"]["mini"]["files"], 2, "{r}");
    assert_eq!(r["previewsLeft"]["photos"], 0, "{r}");
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

// ---- the budget for downloaded originals ----

/// Two devices, B with the originals of both photos downloaded (`sync/originals/<hash>/<name>`).
fn device_with_originals(tag: &str) -> (Fake, Session, PathBuf, Vec<PhotoId>) {
    let mut f = Fake::new();
    let (a, root, ids) = first_device(tag, &mut f);
    drop(a);
    let mut b = open(&root.join("b"));
    sign_in(&mut b);
    sync(&mut b, &mut f);
    b.selection = crate::Selection::single(ids[0]);
    b.execute("sync.downloadOriginals", &json!({"ids": [ids[0].0, ids[1].0]})).unwrap();
    sync(&mut b, &mut f);
    for id in &ids {
        let Source::File { path } = &b.catalog.photo(*id).unwrap().source else { panic!() };
        assert!(path.contains("originals") && Path::new(path).exists(), "{path}");
    }
    (f, b, root, ids)
}

fn here(s: &Session, id: PhotoId) -> bool {
    matches!(&s.catalog.photo(id).unwrap().source, Source::File { path } if Path::new(path).exists())
}

/// Over its budget a device deletes the originals used longest ago, keeps the one it is working on, and the
/// photo is the server's again (nothing is missing, nothing downloads again by itself).
#[test]
fn downloaded_originals_go_over_the_budget_least_recently_used_first() {
    let (mut f, mut b, root, ids) = device_with_originals("budget");
    // the photo being worked on was used last
    b.selection = crate::Selection::single(ids[1]);
    sync(&mut b, &mut f);
    // a budget under the two files' size, over one's
    let sizes: Vec<u64> = ids
        .iter()
        .map(|id| {
            std::fs::metadata(match &b.catalog.photo(*id).unwrap().source {
                Source::File { path } => path.clone(),
                _ => String::new(),
            })
            .unwrap()
            .len()
        })
        .collect();
    let one = sizes.iter().copied().max().unwrap();
    assert!(one < (1 << 20), "test photos are small");
    // (megabytes are whole: use the smallest budget and make the two files big enough to matter)
    for id in &ids {
        let Source::File { path } = &b.catalog.photo(*id).unwrap().source else { panic!() };
        let f = std::fs::OpenOptions::new().append(true).open(path).unwrap();
        f.set_len(700 << 10).unwrap();
    }
    b.execute("sync.originalsBudget", &json!({"mb": 1})).unwrap();
    sync(&mut b, &mut f);
    assert!(
        !here(&b, ids[0]) && here(&b, ids[1]),
        "the one not worked on went: {:?} {:?}",
        b.catalog.photo(ids[0]).unwrap().source,
        b.catalog.photo(ids[1]).unwrap().source
    );
    // it is the server's again: a remote photo, not a missing one, and not downloaded again
    assert!(is_remote(b.catalog.photo(ids[0]).unwrap()));
    assert!(crate::cmd::missing::missing(&b).is_empty());
    let before = f.requests.len();
    sync(&mut b, &mut f);
    assert!(!f.requests[before..].iter().any(|r| r.contains("/blobs/original/")), "{:?}", &f.requests[before..]);
    // the server still has it, and asking for it brings it back
    b.execute("sync.downloadOriginals", &json!({"ids": [ids[0].0]})).unwrap();
    b.selection = crate::Selection::single(ids[0]);
    sync(&mut b, &mut f);
    assert!(here(&b, ids[0]));
    let _ = std::fs::remove_dir_all(&root);
}

/// An original the server doesn't have, one that is open or selected, or available offline is never deleted, and
/// nothing is without a budget or with every original kept.
#[test]
fn the_budget_never_deletes_the_only_copy_or_what_is_in_use() {
    let (mut f, mut b, root, ids) = device_with_originals("budget-safe");
    let grow = |b: &Session| {
        for id in &ids {
            let Source::File { path } = &b.catalog.photo(*id).unwrap().source else { return };
            if let Ok(f) = std::fs::OpenOptions::new().append(true).open(path) {
                let _ = f.set_len(700 << 10);
            }
        }
    };
    grow(&b);
    // no budget: nothing goes
    sync(&mut b, &mut f);
    assert!(here(&b, ids[0]) && here(&b, ids[1]));
    // every original kept: the budget has no effect
    b.execute("sync.storeOriginalsLocally", &json!({"on": true})).unwrap();
    b.execute("sync.originalsBudget", &json!({"mb": 1})).unwrap();
    sync(&mut b, &mut f);
    assert!(here(&b, ids[0]) && here(&b, ids[1]));
    b.execute("sync.storeOriginalsLocally", &json!({"on": false})).unwrap();
    // the photos in use: ids[0] selected, ids[1] available offline
    b.selection = crate::Selection::single(ids[0]);
    b.execute("photo.makeAvailableOffline", &json!({"ids": [ids[1].0], "on": true})).unwrap();
    sync(&mut b, &mut f);
    assert!(here(&b, ids[0]) && here(&b, ids[1]), "in use");
    // the server doesn't have it: the only copy stays
    b.execute("photo.makeAvailableOffline", &json!({"ids": [ids[1].0], "on": false})).unwrap();
    let key = crate::cloud_sync::blob_key(b.catalog.photo(ids[1]).unwrap()).unwrap();
    b.sync.as_mut().unwrap().uploaded.remove(&key);
    b.selection = crate::Selection::single(ids[0]);
    f.blobs.retain(|(_, h), _| *h != key);
    sync(&mut b, &mut f);
    assert!(here(&b, ids[1]), "the server has no copy of it");
    // a path outside the originals folder is never touched whatever the budget
    assert!(Path::new(&root).exists());
    let _ = std::fs::remove_dir_all(&root);
}

// ---- the other shared documents ----

fn names(v: &Value) -> Vec<String> {
    let mut n: Vec<String> = v.as_array().map(|a| a.iter().filter_map(|p| p["name"].as_str().map(str::to_string)).collect()).unwrap_or_default();
    n.sort();
    n
}

/// Two signed-in devices of one user, B joined after A uploaded (nothing but the library to share yet).
fn two_devices(tag: &str, f: &mut Fake) -> (Session, Session, PathBuf) {
    let (a, root, _) = first_device(tag, f);
    let mut b = open(&root.join("b"));
    sign_in(&mut b);
    sync(&mut b, f);
    (a, b, root)
}

/// Export and filter presets follow the user to their other devices: ones added on two devices are all kept,
/// a deletion sticks, and the same name edited on both ends up as one.
#[test]
fn presets_and_sets_follow_the_user_to_their_other_devices() {
    let mut f = Fake::new();
    let (mut a, mut b, root) = two_devices("docs", &mut f);
    a.execute("export.savePreset", &json!({"name": "Web", "params": {"format": "jpeg", "quality": 80}})).unwrap();
    sync(&mut a, &mut f);
    sync(&mut b, &mut f);
    assert_eq!(names(&b.execute("export.presets", &json!({})).unwrap()).iter().filter(|n| *n == "Web").count(), 1, "B has A's preset");
    // each adds one; both end up with both
    b.execute("export.savePreset", &json!({"name": "Print", "params": {"format": "tiff"}})).unwrap();
    a.execute("export.savePreset", &json!({"name": "Mail", "params": {"format": "jpeg", "quality": 60}})).unwrap();
    sync(&mut a, &mut f);
    sync(&mut b, &mut f);
    sync(&mut a, &mut f);
    for s in [&mut a, &mut b] {
        let user: Vec<String> = s.export_presets.iter().map(|p| p.name.clone()).collect();
        let mut user = user;
        user.sort();
        assert_eq!(user, ["Mail", "Print", "Web"]);
    }
    // a deletion sticks
    a.execute("export.deletePreset", &json!({"name": "Web"})).unwrap();
    sync(&mut a, &mut f);
    sync(&mut b, &mut f);
    assert!(b.export_presets.iter().all(|p| p.name != "Web"), "{:?}", b.export_presets);
    // the same name edited on both ends up one preset
    a.execute("export.savePreset", &json!({"name": "Print", "params": {"format": "tiff", "ppi": 300}})).unwrap();
    b.execute("export.savePreset", &json!({"name": "print", "params": {"format": "tiff", "ppi": 240}})).unwrap();
    sync(&mut a, &mut f);
    sync(&mut b, &mut f);
    sync(&mut a, &mut f);
    assert_eq!(a.export_presets.iter().filter(|p| p.name.eq_ignore_ascii_case("print")).count(), 1);
    assert_eq!(a.export_presets.len(), b.export_presets.len());
    // the other lists use the same mechanism: keyword and label sets, filters, metadata
    a.execute("metadata.savePreset", &json!({"name": "Wedding", "fields": {"creator": "Ann"}})).unwrap();
    a.keyword_sets.push(crate::cmd::keywords::KeywordSet { name: "Trip".into(), keywords: vec!["beach".into()] });
    a.save_prefs().unwrap();
    sync(&mut a, &mut f);
    sync(&mut b, &mut f);
    assert!(b.metadata_presets.iter().any(|p| p.name == "Wedding") && b.keyword_sets.iter().any(|k| k.name == "Trip" && k.keywords == ["beach"]));
    // and they are quiet when nothing changed
    let before = f.requests.len();
    sync(&mut a, &mut f);
    sync(&mut b, &mut f);
    assert!(f.requests[before..].iter().all(|r| !r.starts_with("PUT /api/docs/")), "{:?}", &f.requests[before..]);
    let _ = std::fs::remove_dir_all(&root);
}

const CUBE: &str = "TITLE \"Warm\"\nLUT_3D_SIZE 2\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n";

/// A LUT profile travels with its `.cube` text: the other device writes it into its own `Profiles` folder
/// and registers it; removing it removes it there.
#[test]
fn lut_profiles_travel_with_their_files() {
    let mut f = Fake::new();
    let (mut a, mut b, root) = two_devices("docs-lut", &mut f);
    let cube = root.join("Warm.cube");
    std::fs::write(&cube, CUBE).unwrap();
    let r = a.execute("profile.import", &json!({"paths": [cube.to_string_lossy()]})).unwrap();
    let id = r["imported"][0]["id"].as_str().unwrap().to_string();
    sync(&mut a, &mut f);
    sync(&mut b, &mut f);
    assert_eq!(b.lut_profiles.len(), 1);
    let on_b = &b.lut_profiles[0];
    assert_eq!((&on_b.id, on_b.name.as_str()), (&id, "Warm"));
    assert!(on_b.file.starts_with(root.join("b").to_string_lossy().as_ref()) && on_b.file.ends_with(".cube"), "{}", on_b.file);
    assert_eq!(std::fs::read_to_string(&on_b.file).unwrap(), CUBE);
    // removed on A: gone on B, file and all
    let file_on_b = on_b.file.clone();
    a.execute("profile.deleteImported", &json!({"id": id})).unwrap();
    sync(&mut a, &mut f);
    sync(&mut b, &mut f);
    assert!(b.lut_profiles.is_empty() && !Path::new(&file_on_b).exists());
    let _ = std::fs::remove_dir_all(&root);
}

/// The import defaults travel, except what names a place on one computer (the watched folder).
#[test]
fn import_defaults_travel_but_not_the_watched_folder() {
    let mut f = Fake::new();
    let (mut a, mut b, root) = two_devices("docs-prefs", &mut f);
    b.import_defaults.auto_folder = Some("/Users/b/Watched".into());
    b.save_prefs().unwrap();
    sync(&mut b, &mut f);
    a.import_defaults.auto_folder = Some("/home/a/Incoming".into());
    a.execute("library.preferences", &json!({"import": {"copyright": "(c) Ann", "creator": "Ann"}})).unwrap();
    sync(&mut a, &mut f);
    sync(&mut b, &mut f);
    assert_eq!((b.import_defaults.copyright.as_str(), b.import_defaults.creator.as_str()), ("(c) Ann", "Ann"));
    assert_eq!(b.import_defaults.auto_folder.as_deref(), Some("/Users/b/Watched"), "its own folder stays");
    assert_eq!(a.import_defaults.auto_folder.as_deref(), Some("/home/a/Incoming"));
    let _ = std::fs::remove_dir_all(&root);
}

/// The order the user drags the preset groups into follows them to their other devices, and is kept
/// when the library is reopened.
#[test]
fn the_order_of_the_preset_groups_follows_the_user() {
    let mut f = Fake::new();
    let (mut a, mut b, root) = two_devices("preset-groups", &mut f);
    let first = a.preset_groups();
    let last = first.last().unwrap().clone();
    a.execute("preset.moveGroup", &json!({"group": last, "to": 0})).unwrap();
    sync(&mut a, &mut f);
    sync(&mut b, &mut f);
    assert_eq!(b.preset_groups()[0], last);
    assert_eq!(b.preset_groups(), a.preset_groups());
    // B puts it back at the end: A follows, and keeps that order when reopened
    b.execute("preset.moveGroup", &json!({"group": last, "to": 999})).unwrap();
    sync(&mut b, &mut f);
    sync(&mut a, &mut f);
    assert_eq!(a.preset_groups(), first);
    drop(a);
    let mut a = open(&root.join("a"));
    assert_eq!(a.preset_groups(), first);
    let before = f.requests.len();
    sync(&mut a, &mut f);
    assert!(!f.requests[before..].iter().any(|r| r.starts_with("PUT /api/docs/")), "nothing new to send: {:?}", &f.requests[before..]);
    let _ = std::fs::remove_dir_all(&root);
}

/// A server from before the documents answers 404: the rest of the sync goes on, with no error and no
/// retrying every second.
#[test]
fn a_server_without_the_documents_does_not_stall_syncing() {
    let mut f = Fake::new();
    f.no_docs = true;
    let (mut a, mut b, root) = two_devices("docs-old", &mut f);
    a.execute("export.savePreset", &json!({"name": "Web", "params": {"format": "jpeg"}})).unwrap();
    a.selection = crate::Selection::single(a.catalog.photos().next().unwrap().id);
    let id = a.selection.active.unwrap();
    a.execute("photo.rate", &json!({"rating": 4})).unwrap();
    let st = sync(&mut a, &mut f);
    assert_eq!(st["state"], "idle", "{st}");
    sync(&mut b, &mut f);
    assert_eq!(b.catalog.photo(id).unwrap().rating, 4, "photos sync as ever");
    let asked = f.requests.iter().filter(|r| r.contains("/api/docs/")).count();
    sync(&mut a, &mut f);
    sync(&mut a, &mut f);
    assert_eq!(f.requests.iter().filter(|r| r.contains("/api/docs/")).count(), asked, "not asked again this session");
    let _ = std::fs::remove_dir_all(&root);
}

/// What a server (or another device) sends is data: junk items, repeated names, a LUT id that would leave the
/// Profiles folder, a huge name — none of it panics, and only what is usable is kept.
#[test]
fn shared_documents_from_elsewhere_are_checked() {
    let mut f = Fake::new();
    let (mut a, root, _) = first_device("docs-hostile", &mut f);
    let long = "x".repeat(5000);
    f.docs.insert(
        "export-presets".into(),
        proto::Doc {
            version: 1,
            items: json!([
                {"name": "Fine", "params": {"format": "png"}},
                {"name": "fine", "params": {}},
                {"name": "", "params": {}},
                {"name": long, "params": {}},
                {"nope": 1},
                7,
                "text",
                null
            ]),
        },
    );
    f.docs.insert(
        "lut-profiles".into(),
        proto::Doc {
            version: 1,
            items: json!([
                {"id": "lut:../../escape", "name": "bad", "group": "g", "cube": CUBE},
                {"id": "lut:a/b", "name": "bad", "group": "g", "cube": CUBE},
                {"id": "nolut", "name": "bad", "group": "g", "cube": CUBE},
                {"id": "lut:notacube", "name": "bad", "group": "g", "cube": "garbage"},
                {"id": "lut:fine", "name": "Fine", "group": "g", "cube": CUBE}
            ]),
        },
    );
    f.docs.insert("keyword-sets".into(), proto::Doc { version: 1, items: json!({"not": "a list"}) });
    let st = sync(&mut a, &mut f);
    assert_eq!(st["state"], "idle", "{st}");
    assert_eq!(a.export_presets.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["Fine"]);
    assert_eq!(a.lut_profiles.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["lut:fine"]);
    assert!(a.keyword_sets.is_empty());
    let profiles = root.join("a/Profiles");
    let files: Vec<_> = std::fs::read_dir(&profiles).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
    assert_eq!(files, ["fine.cube"], "nothing written outside the Profiles folder");
    assert!(!root.join("escape.cube").exists() && !root.join("a/escape.cube").exists());
    let _ = std::fs::remove_dir_all(&root);
}

// ---- merging a library into the server's ----

fn by_name(s: &Session, name: &str) -> PhotoId {
    s.catalog.photos().find(|p| p.file_name == name).unwrap_or_else(|| panic!("no {name}")).id
}

/// A library that already has photos joins a server that has some, when asked: the photos both have are matched by
/// what they are (their ids differ — and collide), the rest comes across with its albums, edits made on this side
/// reach the server and the other device, and nothing of the server's is lost.
#[test]
fn a_library_with_photos_merges_into_the_servers_when_asked() {
    let mut f = Fake::new();
    let (mut a, root, ids) = first_device("merge", &mut f); // a.png and b.png, ids 1 and 2
    a.selection = crate::Selection::single(ids[0]);
    a.execute("photo.rate", &json!({"rating": 4})).unwrap();
    sync(&mut a, &mut f);
    // another library: b.png again (the same file) and c.png, numbered 1 and 2 as well
    write_png(&root.join("in2/b.png"), 2);
    write_png(&root.join("in2/c.png"), 3);
    let mut c = open(&root.join("c"));
    c.execute("library.import", &json!({"paths": [root.join("in2").to_string_lossy()]})).unwrap();
    let (cb, cc) = (by_name(&c, "b.png"), by_name(&c, "c.png"));
    assert!(ids.contains(&cb) && ids.contains(&cc), "the two libraries number their photos alike");
    c.selection = crate::Selection::single(cb);
    c.execute("develop.set", &json!({"control": "light.exposure", "value": 1.0})).unwrap();
    c.execute("photo.rate", &json!({"rating": 2})).unwrap();
    c.selection = crate::Selection::single(cc);
    c.execute("photo.rate", &json!({"rating": 5})).unwrap();
    c.execute("album.create", &json!({"name": "Mine", "addSelected": true})).unwrap();

    // not asked: the sign-in waits for a choice, and nothing changed on either side
    sign_in(&mut c);
    let st = sync(&mut c, &mut f);
    assert_eq!(st["state"], "conflict", "{st}");
    assert_eq!((c.catalog.len(), f.core.catalog().len()), (2, 2), "nothing changed on either side");

    // asked: combined
    c.execute("sync.signIn", &json!({"server": "http://fake", "user": "ann", "password": "pw", "device": "test", "merge": true})).unwrap();
    let st = sync(&mut c, &mut f);
    assert_eq!(st["state"], "idle", "{st}");
    assert_eq!((st["merged"]["matched"].clone(), st["merged"]["added"].clone()), (json!(1), json!(1)), "{st}");
    // the server: its three photos, the shared one with this side's edits, the new one and its album
    let server = f.core.catalog();
    assert_eq!(server.len(), 3);
    let sb = server.photos().find(|p| p.file_name == "b.png").unwrap();
    assert_eq!((sb.rating, sb.develop.light.exposure), (2, 1.0));
    let sa = server.photos().find(|p| p.file_name == "a.png").unwrap();
    assert_eq!(sa.rating, 4, "what was there stays");
    let sc = server.photos().find(|p| p.file_name == "c.png").unwrap();
    assert_eq!(sc.rating, 5);
    assert_eq!(sc.id.0 >> 32, u64::from(c.sync_state().unwrap().config.space), "new ids are in this device's space");
    let mine: Vec<_> = server.albums().filter(|al| al.name == "Mine").collect();
    assert_eq!((mine.len(), mine[0].photos.clone()), (1, vec![sc.id]));
    assert!(f.blobs.keys().any(|(k, h)| k == "original" && Some(h.as_str()) == crate::cloud_sync::blob_key(sc).as_deref()), "its file went up");
    // this device: all three, its own files where it has them, the server's photo only as previews
    assert_eq!(c.catalog.len(), 3);
    let (lb, lc, la) = (by_name(&c, "b.png"), by_name(&c, "c.png"), by_name(&c, "a.png"));
    assert!(matches!(&c.catalog.photo(lb).unwrap().source, Source::File { path } if path.starts_with(root.join("in2").to_string_lossy().as_ref())));
    assert!(matches!(&c.catalog.photo(lc).unwrap().source, Source::File { path } if path.starts_with(root.join("in2").to_string_lossy().as_ref())));
    assert!(is_remote(c.catalog.photo(la).unwrap()));
    assert_eq!(c.catalog.albums().filter(|al| al.name == "Mine").count(), 1);
    assert!(c.sync_state().unwrap().outbox.is_empty());
    // the first device gets it all
    sync(&mut a, &mut f);
    assert_eq!(a.catalog.len(), 3);
    assert_eq!(a.catalog.photo(a.catalog.photos().find(|p| p.file_name == "b.png").unwrap().id).unwrap().develop.light.exposure, 1.0);
    assert_eq!(a.catalog.albums().filter(|al| al.name == "Mine").count(), 1);
    // signing in again later finds nothing to merge and nothing to redo
    let before = f.core.head();
    sync(&mut c, &mut f);
    assert_eq!(f.core.head(), before);
    let _ = std::fs::remove_dir_all(&root);
}

/// Settings ▸ Sync asks for the album links on every frame, including the ones while the sign-in is still on its
/// way. That request had no token yet, the server answered 401, and the device was signed out the moment it signed
/// in (found on the iOS simulator, where the sign-in is slow enough for a frame to draw first).
#[test]
fn asking_for_links_while_signing_in_does_not_sign_the_device_out() {
    let mut f = Fake::new();
    let root = temp_dir("shares-early");
    let mut a = open(&root.join("a"));
    sign_in(&mut a);
    a.sync_refresh_shares(false);
    let st = sync(&mut a, &mut f);
    assert_eq!(st["state"], "idle", "{st}");
    assert_eq!(st["signedIn"], true, "{st}");
    assert!(f.requests.iter().all(|r| r != "GET /api/shares"), "nothing asked before there was a token: {:?}", f.requests);
    a.sync_refresh_shares(false);
    let st = sync(&mut a, &mut f);
    assert_eq!(st["state"], "idle", "{st}");
    assert_eq!(f.requests.iter().filter(|r| *r == "GET /api/shares").count(), 1, "{:?}", f.requests);
    let (links, error) = a.sync_shares();
    assert!(links.is_empty() && error.is_none(), "{error:?}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn what_the_server_is_doing_is_asked_for_only_while_someone_looks() {
    let mut f = Fake::new();
    let (mut a, root, _) = first_device("activity-ask", &mut f);
    let asks = |s: &mut Session| {
        s.sync_tasks().into_iter().filter(|t| matches!(t, Task::Http { url, .. } if url.ends_with("/api/activity"))).collect::<Vec<_>>()
    };
    assert!(asks(&mut a).is_empty(), "nobody is looking");
    a.sync_want_activity();
    let t = asks(&mut a);
    assert_eq!(t.len(), 1);
    a.sync_want_activity();
    assert!(asks(&mut a).is_empty(), "one request at a time");
    a.sync_done(f.handle(&t[0]));
    assert!(a.sync_state().unwrap().activity().is_some());
    a.sync_want_activity();
    assert!(asks(&mut a).is_empty(), "the answer is fresh");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_servers_work_is_reported_and_an_older_server_says_so() {
    let mut f = Fake::new();
    let (mut a, root, _) = first_device("activity", &mut f);
    f.activity = proto::Activity {
        scan: Some(proto::Scan { scanning: true, phase: "reading".into(), done: 120, todo: 4000, files: 4120, ..Default::default() }),
        previews: proto::Progress { done: 3, total: 40, eta_secs: Some(90) },
        search: Some(proto::Progress { done: 10, total: 10, eta_secs: None }),
        ..Default::default()
    };
    a.sync_fetch_activity_with(&mut |t| f.handle(t));
    let r = a.execute("sync.activity", &json!({})).unwrap();
    assert_eq!(r["server"]["scan"]["phase"], "reading", "{r}");
    assert_eq!((&r["server"]["scan"]["done"], &r["server"]["scan"]["todo"]), (&json!(120), &json!(4000)), "{r}");
    assert_eq!(
        (&r["server"]["previews"]["done"], &r["server"]["previews"]["total"], &r["server"]["previews"]["etaSecs"]),
        (&json!(3), &json!(40), &json!(90))
    );
    assert!(a.sync_state().unwrap().activity().unwrap().busy());
    // idle: a finished batch isn't work
    f.activity = proto::Activity { previews: proto::Progress { done: 40, total: 40, eta_secs: None }, ..Default::default() };
    a.sync_fetch_activity_with(&mut |t| f.handle(t));
    assert!(!a.sync_state().unwrap().activity().unwrap().busy());
    // a server from before the route
    f.no_activity = true;
    a.sync_fetch_activity_with(&mut |t| f.handle(t));
    let st = a.sync_state().unwrap();
    assert!(st.activity().is_none());
    assert!(st.activity_error().unwrap().contains("too old"), "{:?}", st.activity_error());
    assert_eq!(sync(&mut a, &mut f)["state"], "idle", "not an error of the sync itself");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn what_is_being_sent_is_listed_with_its_progress() {
    let mut f = Fake::new();
    let root = temp_dir("transfers");
    write_png(&root.join("in/a.png"), 1);
    write_png(&root.join("in/b.png"), 2);
    let mut a = open(&root.join("a"));
    a.execute("library.import", &json!({"paths": [root.join("in").to_string_lossy()]})).unwrap();
    sign_in(&mut a);
    // step the sync by hand, looking at what it says at every step
    let (mut most_queued, mut most_done, mut named) = (0, 0, std::collections::BTreeSet::new());
    let mut one_of_two = false;
    for _ in 0..500 {
        let tasks = a.sync_tasks();
        let t = a.sync_transfers();
        most_queued = most_queued.max(t.uploads);
        for r in &t.running {
            assert!(r.upload, "{r:?}");
            named.insert((r.name.clone(), r.what));
        }
        if tasks.is_empty() {
            break;
        }
        for task in tasks {
            let d = f.handle(&task);
            a.sync_done(d);
            // (after each file: the first photo is finished while the second is still being sent)
            let t = a.sync_transfers();
            most_done = most_done.max(t.uploads_done);
            one_of_two |= (t.uploads_done, t.uploads) == (1, 1);
        }
    }
    assert_eq!(most_queued, 2, "both photos were queued");
    assert_eq!(most_done, 2);
    assert!(one_of_two, "the progress read 1 done, 1 left");
    assert!(named.contains(&("a.png".to_string(), "original")) && named.contains(&("b.png".to_string(), "original")), "{named:?}");
    assert!(named.iter().any(|(_, what)| *what == "smart preview"), "{named:?}");
    // all sent: nothing is left, and the next batch counts from zero
    let t = a.sync_transfers();
    assert_eq!((t.pending, t.uploads, t.uploads_done, t.downloads, t.running.len()), (0, 0, 0, 0, 0), "{t:?}");
    // a second device downloads previews: they are counted the same way
    let mut b = open(&root.join("b"));
    sign_in(&mut b);
    let mut most_down = 0;
    for _ in 0..500 {
        // (the grid shows every photo it has so far)
        let shown: Vec<PhotoId> = b.catalog.photos().map(|p| p.id).collect();
        for id in shown {
            b.thumb_job(id, 128);
        }
        let tasks = b.sync_tasks();
        let t = b.sync_transfers();
        most_down = most_down.max(t.downloads + t.downloads_done);
        if tasks.is_empty() {
            break;
        }
        for task in tasks {
            let d = f.handle(&task);
            b.sync_done(d);
        }
    }
    assert!(most_down >= 2, "two previews came down: {most_down}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn failing_transfers_say_why_and_are_forgotten_once_one_works() {
    let mut f = Fake::new();
    let root = temp_dir("failing");
    write_png(&root.join("in/a.png"), 1);
    let mut a = open(&root.join("a"));
    a.execute("library.import", &json!({"paths": [root.join("in").to_string_lossy()]})).unwrap();
    f.put_fails = Some("no space left on the server's disk".into());
    sign_in(&mut a);
    sync(&mut a, &mut f);
    let t = a.sync_transfers();
    assert_eq!(t.failing, 1, "{t:?}");
    assert!(t.error.as_deref().is_some_and(|e| e.contains("no space left")), "{t:?}");
    // room again: the next try works, and the complaint goes
    f.put_fails = None;
    sync(&mut a, &mut f);
    let t = a.sync_transfers();
    assert_eq!((t.failing, t.error), (0, None));
    assert_eq!(f.blobs.len(), 3, "the original and both previews are there now");
    let _ = std::fs::remove_dir_all(&root);
}

/// What a big synced library costs per frame and per edit (192k photos whose originals stay on the
/// server, like a NAS library): `cargo test -p lightcraft-engine --release big_synced_library -- --ignored --nocapture`.
#[test]
#[ignore = "timing"]
fn big_synced_library_timings() {
    use std::time::Instant;
    let n: usize = std::env::var("LC_BIG").ok().and_then(|v| v.parse().ok()).unwrap_or(192_000);
    let mut f = Fake::new();
    f.page = 2000;
    let ops: Vec<Op> = (0..n)
        .map(|i| {
            let id = f.core.alloc_photo_id(0);
            let hash = format!("{:032x}", (i as u128 + 1) * 7919);
            let path = lightcraft_catalog::sync::original_path(&hash, &format!("IMG_{i}.CR2"));
            let mut p = lightcraft_catalog::Photo::new(id, Source::File { path }, &format!("IMG_{i}.CR2"), "CR2", 6000, 4000, "2026-01-01T00:00:00Z");
            p.content_hash = Some(hash);
            Op::AddPhoto { photo: Box::new(p) }
        })
        .collect();
    for chunk in ops.chunks(10_000) {
        f.core.push(f.core.head(), chunk).unwrap();
    }
    let root = temp_dir("big");
    let mut b = open(&root.join("b"));
    sign_in(&mut b);
    // the library comes in; photo files never do (held in flight)
    let t0 = Instant::now();
    let mut held = Vec::new();
    for _ in 0..1000 {
        let tasks = b.sync_tasks();
        if tasks.is_empty() {
            break;
        }
        for t in tasks {
            if matches!(&t, Task::Http { url, .. } if url.contains("/api/blobs/")) {
                held.push(t);
            } else {
                let d = f.handle(&t);
                b.sync_done(d);
            }
        }
    }
    eprintln!("read {} photos in {:?}", b.catalog.len(), t0.elapsed());
    let time = |what: &str, b: &mut Session, f: &mut dyn FnMut(&mut Session)| {
        let t = Instant::now();
        f(b);
        eprintln!("{what}: {:.1} ms", t.elapsed().as_secs_f64() * 1e3);
    };
    time("visible (cold)", &mut b, &mut |b| {
        b.visible();
    });
    time("sync_tasks (no change)", &mut b, &mut |b| {
        b.sync_tasks();
    });
    time("state()", &mut b, &mut |b| {
        b.sync_state().unwrap().state();
    });
    time("sync_transfers()", &mut b, &mut |b| {
        b.sync_transfers();
    });
    let id = b.catalog.photos().next().unwrap().id;
    b.selection = crate::Selection::single(id);
    time("sync_tasks (active changed)", &mut b, &mut |b| {
        held.extend(b.sync_tasks());
    });
    // a grid frame: three screens of cells ask for their thumbnails
    let cells: Vec<PhotoId> = b.visible().iter().take(300).copied().collect();
    time("grid frame (300 thumb_job + sync_tasks)", &mut b, &mut |b| {
        for id in &cells {
            b.thumb_job(*id, 256);
        }
        held.extend(b.sync_tasks());
    });
    time("next grid frame (the same cells)", &mut b, &mut |b| {
        for id in &cells {
            b.thumb_job(*id, 256);
        }
        held.extend(b.sync_tasks());
    });
    let photos: Vec<std::sync::Arc<lightcraft_catalog::Photo>> = cells.iter().map(|id| b.catalog.photo(*id).unwrap().clone()).collect();
    time("300 synced_tier", &mut b, &mut |b| {
        for p in &photos {
            b.synced_tier(p);
        }
    });
    time("300 source_ref", &mut b, &mut |b| {
        for p in &photos {
            b.media.source_ref(p, crate::media::SourceLevel::Thumb);
        }
    });
    time("300 settings hash", &mut b, &mut |_| {
        for p in &photos {
            p.develop.hash64();
        }
    });
    time("sync_tasks after an edit elsewhere (plan)", &mut b, &mut |b| {
        b.catalog.revision += 1;
        b.sync_tasks();
    });
    // a preview lands
    let t = held.into_iter().find(|t| matches!(t, Task::Http { url, .. } if url.contains("/blobs/mini/"))).unwrap();
    let Task::Http { url, save_to, .. } = &t else { panic!() };
    let key = url.rsplit('/').next().unwrap().to_string();
    f.blobs.insert(("mini".into(), key), b"x".to_vec());
    let _ = save_to;
    let d = f.handle(&t);
    time("a preview lands (sync_done + sync_tasks + visible)", &mut b, &mut |b| {
        b.sync_done(d.clone());
        b.sync_tasks();
        b.visible();
    });
    time("slider frame (develop.set during a drag + sync_tasks + visible)", &mut b, &mut |b| {
        b.execute("develop.beginInteraction", &json!({"label": "Exposure"})).ok();
        b.execute("develop.set", &json!({"control": "light.exposure", "value": 0.5})).unwrap();
        b.sync_tasks();
        b.visible();
    });
    time("drag ends: commit", &mut b, &mut |b| {
        b.execute("develop.endInteraction", &json!({})).ok();
    });
    time("drag ends: sync_tasks", &mut b, &mut |b| {
        b.sync_tasks();
    });
    time("drag ends: visible", &mut b, &mut |b| {
        b.visible();
    });
    time("drag ends: source_total", &mut b, &mut |b| {
        b.source_total();
    });
    time("all previews on (sync_tasks)", &mut b, &mut |b| {
        b.execute("sync.allPreviews", &json!({"on": true})).unwrap();
        b.sync_tasks();
    });
    time("an edit with all previews on (develop.set + sync_tasks + visible)", &mut b, &mut |b| {
        b.execute("develop.set", &json!({"control": "light.exposure", "value": 0.7})).unwrap();
        b.sync_tasks();
        b.visible();
    });
    time("sync.usage (previews left, counted)", &mut b, &mut |b| {
        b.sync_previews_left();
    });
    let _ = std::fs::remove_dir_all(&root);
}
