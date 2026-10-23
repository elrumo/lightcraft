//! Lighter videos end to end: the admin settings and queue routes, the `video` blob kind (digest, resumable upload, quota,
//! gc), the queue worked through by a fake encoder, by the real pure-Rust one and by a claiming helper's Rust surface.
//! Every clip is synthetic (`lightcraft_video::testdata`).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use lightcraft_server::video::{Codec, EncodeError, Encoded, FailKind, Failure, Mark, Settings, VideoEncoder, Worker};
use lightcraft_server::{Config, Server, accounts, gc};
use lightcraft_video::testdata;
use serde_json::{Value, json};

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-server-video-it-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder().http_status_as_error(false).build().into()
}

type Answer = (u16, Vec<u8>, Vec<(String, String)>);

fn call(method: &str, url: &str, token: &str, body: Option<&[u8]>, headers: &[(&str, &str)]) -> Answer {
    let a = agent();
    let auth = format!("Bearer {token}");
    let r = match method {
        "GET" | "HEAD" => {
            let mut req = if method == "GET" { a.get(url) } else { a.head(url) }.header("Authorization", &auth);
            for (k, v) in headers {
                req = req.header(*k, *v);
            }
            req.call()
        }
        _ => {
            let mut req = if method == "PUT" { a.put(url) } else { a.post(url) }.header("Authorization", &auth);
            for (k, v) in headers {
                req = req.header(*k, *v);
            }
            req.send(body.unwrap_or(&[]))
        }
    };
    let mut r = r.unwrap();
    let hs = r.headers().iter().map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string())).collect();
    let status = r.status().as_u16();
    (status, r.body_mut().read_to_vec().unwrap_or_default(), hs)
}

fn json_of(b: &[u8]) -> Value {
    serde_json::from_slice(b).unwrap_or(Value::Null)
}

fn header_of(hs: &[(String, String)], name: &str) -> Option<String> {
    hs.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.clone())
}

fn wait_until(what: &str, mut ok: impl FnMut() -> bool) {
    for _ in 0..2400 {
        if ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("gave up waiting for {what}");
}

/// A fake encoder: writes `percent` of the original's size as the copy, refuses the files it is told to, and can be
/// held until released (or cancelled).
struct Fake {
    percent: AtomicUsize,
    refuse: Mutex<Vec<String>>,
    hold: AtomicBool,
    started: AtomicUsize,
    made: AtomicUsize,
    /// The rayon threads and the thread's name the last encode saw.
    ran_on: Mutex<(usize, String)>,
}

impl Fake {
    fn new(percent: usize) -> Arc<Fake> {
        Arc::new(Fake {
            percent: AtomicUsize::new(percent),
            refuse: Mutex::new(Vec::new()),
            hold: AtomicBool::new(false),
            started: AtomicUsize::new(0),
            made: AtomicUsize::new(0),
            ran_on: Mutex::new((0, String::new())),
        })
    }
}

impl VideoEncoder for Fake {
    fn codecs(&self) -> &[Codec] {
        &[Codec::H264, Codec::Hevc]
    }

    fn encode(&self, src: &Path, dst: &Path, settings: &Settings, cancel: &AtomicBool, progress: &dyn Fn(u64, u64)) -> Result<Encoded, EncodeError> {
        self.started.fetch_add(1, Ordering::SeqCst);
        *self.ran_on.lock().unwrap() = (rayon::current_num_threads(), std::thread::current().name().unwrap_or_default().to_string());
        while self.hold.load(Ordering::SeqCst) {
            if cancel.load(Ordering::SeqCst) {
                return Err(EncodeError::Cancelled);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let name = src.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if self.refuse.lock().unwrap().contains(&name) {
            return Err(EncodeError::Failed(Failure { kind: FailKind::Unsupported, reason: "no thanks".into() }));
        }
        let len = std::fs::metadata(src).unwrap().len() as usize * self.percent.load(Ordering::SeqCst) / 100;
        std::fs::write(dst, vec![7u8; len]).unwrap();
        progress(1, 1);
        self.made.fetch_add(1, Ordering::SeqCst);
        Ok(Encoded { codec: settings.codec, width: 64, height: 48, kbps: 100, hardware: false })
    }
}

struct Lab {
    root: PathBuf,
    data: PathBuf,
    server: Server,
    base: String,
    admin: String,
    token: String,
    /// The clips: file name, content hash, bytes.
    clips: Vec<(String, String, Vec<u8>)>,
}

fn scanned(server: &Server) {
    wait_until("the scan", || {
        let s = server.folder_status("ann");
        s.scans > 0 && !s.scanning && s.previews == 0
    });
}

/// A server whose library folder holds `clips` (file name, bytes) and `extra` (other files), scanned.
fn lab_with(tag: &str, clips: Vec<(&str, Vec<u8>)>, extra: Vec<(&str, Vec<u8>)>, encoder: Option<Arc<dyn VideoEncoder>>) -> Lab {
    let root = temp(tag);
    let folder = root.join("nas/phone");
    std::fs::create_dir_all(&folder).unwrap();
    for (name, bytes) in clips.iter().chain(extra.iter()) {
        std::fs::write(folder.join(name), bytes).unwrap();
    }
    let data = root.join("data");
    accounts::set_user(&data, "ann", "correct horse", false).unwrap();
    accounts::set_admin(&data, "ann", true).unwrap();
    accounts::add_folder(&data, "ann", &folder.to_string_lossy(), Some("Phone")).unwrap();
    let mut cfg = Config::new(&data, "127.0.0.1:0");
    cfg.scan_interval = None;
    cfg.preview_threads = Some(1);
    cfg.video_encoder = encoder;
    // (a thread of the lowest priority gets almost no time on a machine other work has saturated, and a test waits a minute:
    // LC_TEST_VIDEO_LOW_PRIORITY=1 runs them the way a server does)
    cfg.video_low_priority = std::env::var_os("LC_TEST_VIDEO_LOW_PRIORITY").is_some();
    let server = Server::start(cfg).unwrap();
    scanned(&server);
    let base = format!("http://{}", server.addr());
    let (s, b, _) =
        call("POST", &format!("{base}/api/admin/login"), "", Some(json!({"user": "ann", "password": "correct horse"}).to_string().as_bytes()), &[]);
    assert_eq!(s, 200);
    let admin = json_of(&b)["token"].as_str().unwrap().to_string();
    let body = json!({"user": "ann", "password": "correct horse", "device": "t"}).to_string();
    let (s, b, _) = call("POST", &format!("{base}/api/login"), "", Some(body.as_bytes()), &[]);
    assert_eq!(s, 200);
    let token = json_of(&b)["token"].as_str().unwrap().to_string();
    let clips = clips.into_iter().map(|(n, b)| (n.to_string(), lightcraft_preview::hash_bytes(&b).to_string(), b)).collect();
    Lab { root, data, server, base, admin, token, clips }
}

fn lab(tag: &str, clips: Vec<(&str, Vec<u8>)>, encoder: Option<Arc<dyn VideoEncoder>>) -> Lab {
    lab_with(tag, clips, Vec::new(), encoder)
}

impl Lab {
    fn admin(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let body = body.map(|b| b.to_string());
        let (s, b, _) = call(method, &format!("{}/api/admin/{path}", self.base), &self.admin, body.as_deref().map(str::as_bytes), &[]);
        (s, json_of(&b))
    }

    fn status(&self) -> Value {
        let (s, v) = self.admin("GET", "video", None);
        assert_eq!(s, 200, "{v}");
        v
    }

    fn set(&self, settings: Value) -> Value {
        let (s, v) = self.admin("PUT", "video", Some(settings));
        assert_eq!(s, 200, "{v}");
        v
    }

    fn blobs(&self) -> PathBuf {
        self.data.join("users/ann/blobs")
    }

    fn url(&self, hash: &str) -> String {
        format!("{}/api/blobs/video/{hash}", self.base)
    }

    fn copies(&self) -> usize {
        files_in(&self.blobs().join("video"))
    }

    fn record(&self, hash: &str) -> Option<Value> {
        std::fs::read(self.data.join("users/ann/video").join(format!("{hash}.json"))).ok().map(|b| json_of(&b))
    }

    /// The copy a device would upload: the clip written again, smaller.
    fn rendition(&self, i: usize, kbps: u32) -> Vec<u8> {
        let src = self.root.join("nas/phone").join(&self.clips[i].0);
        let dst = self.root.join(format!("rendition-{i}.mp4"));
        let plan = lightcraft_video::Plan { width: 64, height: 48, kbps, threads: 1 };
        lightcraft_video::lighter_copy(&src, &dst, plan, &AtomicBool::new(false), &|_, _| {}).unwrap();
        let copy = std::fs::read(&dst).unwrap();
        assert!(
            copy.len() * 100 <= self.clips[i].2.len() * 70,
            "the test needs a copy that is worth keeping: {} of {} bytes",
            copy.len(),
            self.clips[i].2.len()
        );
        assert!(copy.len() > 2000, "and one big enough to cut in two: {}", copy.len());
        copy
    }

    /// `PUT` a copy of clip `i`; `digest` is what the device says its bytes hash to.
    fn put(&self, i: usize, bytes: &[u8], digest: Option<&str>) -> (u16, Value) {
        let mut hs = Vec::new();
        if let Some(d) = digest {
            hs.push(("X-LightCraft-Digest", d));
        }
        let (s, b, _) = call("PUT", &self.url(&self.clips[i].1), &self.token, Some(bytes), &hs);
        (s, json_of(&b))
    }

    fn finish(self) {
        drop(self.server);
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn files_in(dir: &Path) -> usize {
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    rd.flatten().map(|e| if e.path().is_dir() { files_in(&e.path()) } else { 1 }).sum()
}

fn digest_of(bytes: &[u8]) -> String {
    lightcraft_preview::hash_bytes(bytes).to_string()
}

/// Textured clips, each different (a different length).
fn clips(n: usize) -> Vec<(&'static str, Vec<u8>)> {
    ["a.mp4", "b.mp4", "c.mp4", "bad.mp4"].into_iter().take(n).enumerate().map(|(i, name)| (name, testdata::clip_textured(20 + i, 10, 0))).collect()
}

fn old(path: &Path) {
    let t = SystemTime::now() - Duration::from_secs(3 * 24 * 3600);
    std::fs::File::options().write(true).open(path).unwrap().set_modified(t).unwrap();
}

// ---- settings --------------------------------------------------------------------------------------------------------

/// Off by default, changed field by field and kept, refused with the field named, and for an admin session only.
#[test]
fn the_settings_routes_validate_and_need_an_admin() {
    let lab = lab("settings", clips(2), None);
    let s = lab.status();
    assert_eq!((s["state"].clone(), s["settings"]["enabled"].clone(), s["settings"]["maxHeight"].clone()), (json!("off"), json!(false), json!(1080)));
    assert_eq!((s["total"].clone(), s["have"].clone(), s["left"].clone()), (json!(2), json!(0), json!(2)), "{s}");

    let s = lab.set(json!({"enabled": true, "codec": "hevc", "maxHeight": "original", "quality": "better"}));
    assert_eq!(
        (s["settings"]["codec"].clone(), s["settings"]["maxHeight"].clone(), s["settings"]["quality"].clone()),
        (json!("hevc"), json!("original"), json!("better"))
    );
    assert_eq!(s["settings"]["skipIfSmaller"], json!(true), "what the request leaves out stays");
    let saved: Value = json_of(&std::fs::read(lab.data.join("video.json")).unwrap());
    assert_eq!(saved["codec"], json!("hevc"), "kept in video.json");
    for (bad, field) in [
        (json!({"codec": "av1"}), "codec"),
        (json!({"maxHeight": 900}), "maxHeight"),
        (json!({"quality": 3}), "quality"),
        (json!({"serverFallback": "yes"}), "serverFallback"),
        (json!({"replaceOriginals": true}), "replaceOriginals"),
    ] {
        let (s, v) = lab.admin("PUT", "video", Some(bad.clone()));
        assert_eq!(s, 400, "{bad}");
        assert!(v["error"].as_str().unwrap().contains(field), "{bad}: {v}");
    }
    assert_eq!(lab.status()["settings"]["codec"], json!("hevc"), "a refused change changes nothing");

    // the actions: pause and resume are seen, an unknown one is not a route, and "apply" wants the switch on
    assert_eq!(lab.admin("POST", "video/pause", None).1["paused"], json!(true));
    assert_eq!(lab.status()["state"], json!("paused"));
    assert_eq!(lab.admin("POST", "video/resume", None).1["paused"], json!(false));
    assert_eq!(lab.admin("POST", "video/explode", None).0, 404);
    lab.set(json!({"enabled": false}));
    assert_eq!(lab.admin("POST", "video/apply", None).0, 409);
    assert_eq!(lab.admin("POST", "video/retry-failed", None).0, 200);

    // not for a user who is no admin, nor without signing in
    let url = format!("{}/api/admin/video", lab.base);
    assert_eq!(call("GET", &url, &lab.token, None, &[]).0, 401);
    assert_eq!(call("GET", &url, "", None, &[]).0, 401);
    assert_eq!(call("PUT", &url, &lab.token, Some(b"{\"enabled\": true}"), &[]).0, 401);
    // and a copy is not taken while the switch is off
    assert_eq!(lab.put(0, b"x", Some(&digest_of(b"x"))).0, 409);
    lab.finish();
}

// ---- the copy ---------------------------------------------------------------------------------------------------------

/// A copy is taken with its digest, checked, kept beside the original's name, served with ranges, counted once among the
/// lighter copies, and its record sits outside `blobs/video`.
#[test]
fn a_lighter_copy_is_checked_stored_and_served() {
    let lab = lab("copy", clips(2), None);
    lab.set(json!({"enabled": true, "skipIfSmaller": true}));
    let (name, hash, original) = lab.clips[0].clone();
    let copy = lab.rendition(0, 40);
    let digest = digest_of(&copy);

    // nothing without the copy's own digest, nothing that doesn't match it, nothing that isn't a video
    assert_eq!(lab.put(0, &copy, None).0, 400);
    assert_eq!(lab.put(0, &copy, Some("not a digest")).0, 400);
    let (s, v) = lab.put(0, &copy, Some(&digest_of(b"something else")));
    assert_eq!(s, 422, "{v}");
    assert!(v["error"].as_str().unwrap().contains("digest"), "{v}");
    let junk = vec![9u8; 4000];
    let (s, v) = lab.put(0, &junk, Some(&digest_of(&junk)));
    assert_eq!(s, 422, "{v}");
    assert!(v["error"].as_str().unwrap().contains("video"), "{v}");
    // the digest of a copy of another clip is not this clip's... but a hash that names no video in the library is refused first
    let nobody = digest_of(b"nobody");
    let (s, _, _) = call("PUT", &lab.url(&nobody), &lab.token, Some(&copy), &[("X-LightCraft-Digest", &digest)]);
    assert_eq!(s, 404);
    assert_eq!(lab.copies(), 0, "none of that kept anything");
    assert_eq!(files_in(&lab.blobs().join("tmp")), 0, "nor left a partial file");

    let (s, v) = lab.put(0, &copy, Some(&digest));
    assert_eq!((s, v), (200, json!({})));
    assert_eq!(lab.copies(), 1);
    let (s, bytes, hs) = call("GET", &lab.url(&hash), &lab.token, None, &[("Range", "bytes=0-99")]);
    assert_eq!((s, bytes), (206, copy[..100].to_vec()));
    assert_eq!(header_of(&hs, "content-type").as_deref(), Some("video/mp4"));
    assert_eq!(header_of(&hs, "content-range"), Some(format!("bytes 0-99/{}", copy.len())));
    assert_eq!(call("GET", &lab.url(&hash), &lab.token, None, &[]).1, copy);
    assert_eq!(call("HEAD", &lab.url(&hash), &lab.token, None, &[]).0, 200);
    // a copy that isn't there is not made from the original (nothing queues previews for it)
    let (s, v, _) = call("GET", &lab.url(&lab.clips[1].1), &lab.token, None, &[]);
    assert_eq!(s, 404);
    assert!(json_of(&v).get("previews").is_none(), "{}", String::from_utf8_lossy(&v));

    // its record, and the counts: one copy (the record is not another), what it saved
    let r = lab.record(&hash).unwrap();
    assert_eq!(
        (r["state"].clone(), r["codec"].clone(), r["height"].clone(), r["digest"].clone()),
        (json!("copy"), json!("h264"), json!(48), json!(digest)),
        "{r}"
    );
    assert_eq!((r["srcBytes"].as_u64(), r["dstBytes"].as_u64()), (Some(original.len() as u64), Some(copy.len() as u64)));
    assert_eq!(r["name"], json!(name));
    let (_, u, _) = call("GET", &format!("{}/api/usage", lab.base), &lab.token, None, &[]);
    let u = json_of(&u);
    assert_eq!(u["video"]["encoded"], json!({"files": 1, "bytes": copy.len()}), "{u}");
    assert_eq!(u["video"]["saved"].as_u64(), Some((original.len() - copy.len()) as u64), "{u}");
    let s = lab.status();
    assert_eq!((s["have"].clone(), s["left"].clone(), s["total"].clone()), (json!(1), json!(1), json!(2)), "{s}");
    assert_eq!(s["bytes"]["encoded"].as_u64(), Some(copy.len() as u64));

    // the helper's surface: the clip with a copy is done, and the server vouches for the digest
    assert_eq!(lab.server.finish_video_job("ann", &hash, &digest), Ok(Mark::Copy));
    assert!(lab.server.finish_video_job("ann", &hash, &digest_of(b"other")).is_err());
    lab.finish();
}

/// A connection that breaks part-way leaves what arrived; the upload goes on from there, and a copy whose first part was
/// another file is caught by the digest.
#[test]
fn an_interrupted_copy_carries_on_and_a_wrong_start_is_caught() {
    use std::io::Write;
    let lab = lab("resume", clips(1), None);
    lab.set(json!({"enabled": true}));
    let hash = lab.clips[0].1.clone();
    let copy = lab.rendition(0, 40);
    let digest = digest_of(&copy);
    let url = lab.url(&hash);
    let head = |lab: &Lab| {
        let (s, _, hs) = call("HEAD", &lab.url(&hash), &lab.token, None, &[]);
        (s, header_of(&hs, "upload-offset").and_then(|v| v.parse::<u64>().ok()))
    };
    let wait_for_offset = |lab: &Lab, want: u64| {
        wait_until("the partial upload", || head(lab) == (404, Some(want)));
        std::thread::sleep(Duration::from_millis(100));
    };
    let broken = |sent: &[u8], digest: &str| {
        let mut c = std::net::TcpStream::connect(lab.server.addr()).unwrap();
        let req = format!(
            "PUT /api/blobs/video/{hash} HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {}\r\nX-LightCraft-Digest: {digest}\r\nContent-Length: {}\r\n\r\n",
            lab.token,
            copy.len()
        );
        c.write_all(req.as_bytes()).unwrap();
        c.write_all(sent).unwrap();
        c.flush().unwrap();
    };
    assert_eq!(head(&lab), (404, Some(0)));

    let cut = copy.len() / 2;
    broken(&copy[..cut], &digest);
    wait_for_offset(&lab, cut as u64);
    assert!(lab.blobs().join("tmp").join(format!("video-{hash}.part")).is_file());
    assert_eq!(lab.copies(), 0, "not kept as the copy yet");
    let range = |first: usize| format!("bytes {first}-{}/{}", copy.len() - 1, copy.len());
    let with = |body: &[u8], range: &str| {
        let (s, b, _) = call("PUT", &url, &lab.token, Some(body), &[("X-LightCraft-Digest", &digest), ("Content-Range", range)]);
        (s, json_of(&b))
    };
    let (s, v) = with(&copy[100..], &range(100));
    assert_eq!((s, v["offset"].clone()), (409, json!(cut)), "going on from the wrong place says where the server is: {v}");
    let (s, v) = with(&copy[cut..], &range(cut));
    assert_eq!((s, v), (200, json!({})));
    assert_eq!(call("GET", &url, &lab.token, None, &[]).1, copy);
    assert_eq!(files_in(&lab.blobs().join("tmp")), 0, "the partial file is gone");
    lab.finish();

    // a start that was another file: the whole hashes wrong, the partial is dropped
    let lab = self::lab("resume-bad", clips(1), None);
    lab.set(json!({"enabled": true}));
    let hash = lab.clips[0].1.clone();
    let copy = lab.rendition(0, 40);
    let digest = digest_of(&copy);
    let mut c = std::net::TcpStream::connect(lab.server.addr()).unwrap();
    let garbage = vec![3u8; 1000];
    let req = format!(
        "PUT /api/blobs/video/{hash} HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {}\r\nX-LightCraft-Digest: {digest}\r\nContent-Length: {}\r\n\r\n",
        lab.token,
        copy.len()
    );
    c.write_all(req.as_bytes()).unwrap();
    c.write_all(&garbage).unwrap();
    c.flush().unwrap();
    wait_until("the partial upload", || {
        let (s, _, hs) = call("HEAD", &lab.url(&hash), &lab.token, None, &[]);
        (s, header_of(&hs, "upload-offset")) == (404, Some("1000".into()))
    });
    drop(c);
    let range = format!("bytes 1000-{}/{}", copy.len() - 1, copy.len());
    // (the request that was writing lets go of the file a moment after the connection closes)
    let (mut s, mut b) = (409, Vec::new());
    for _ in 0..100 {
        (s, b, _) = call("PUT", &lab.url(&hash), &lab.token, Some(&copy[1000..]), &[("X-LightCraft-Digest", &digest), ("Content-Range", &range)]);
        if s != 409 {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(s, 422, "{}", String::from_utf8_lossy(&b));
    assert_eq!((lab.copies(), files_in(&lab.blobs().join("tmp"))), (0, 0));
    // and the copy put whole is taken
    assert_eq!(lab.put(0, &copy, Some(&digest)).0, 200);
    lab.finish();
}

/// A copy over the user's storage limit is refused with 507, as an original is.
#[test]
fn a_copy_over_the_storage_limit_is_refused() {
    let lab = lab("quota", clips(1), None);
    lab.set(json!({"enabled": true}));
    let copy = lab.rendition(0, 40);
    let digest = digest_of(&copy);
    // (below what ann keeps already: set in the file, as the admin route refuses limits this small)
    let mut f: Value = json_of(&std::fs::read(lab.data.join("users.json")).unwrap());
    f["users"]["ann"]["quota"] = json!(1000);
    std::fs::write(lab.data.join("users.json"), serde_json::to_vec(&f).unwrap()).unwrap();
    let (s, v) = lab.put(0, &copy, Some(&digest));
    assert_eq!(s, 507, "{v}");
    assert_eq!(lab.copies(), 0);
    assert_eq!(lab.admin("PUT", "users/ann/quota", Some(json!({"quota": null}))).0, 200);
    assert_eq!(lab.put(0, &copy, Some(&digest)).0, 200);
    lab.finish();
}

/// A copy that is not 15% smaller is not kept (the clip is marked "already efficient" and not queued again), unless the
/// setting is off; a copy not as long as the clip is refused.
#[test]
fn a_copy_that_is_not_smaller_enough_is_not_kept_and_the_clip_is_marked() {
    let lab = lab("efficient", clips(2), None);
    lab.set(json!({"enabled": true, "skipIfSmaller": true}));
    let (hash, original) = (lab.clips[0].1.clone(), lab.clips[0].2.clone());
    // the original itself is a "copy" of the same length and no smaller
    let (s, v) = lab.put(0, &original, Some(&digest_of(&original)));
    assert_eq!((s, v["kept"].clone(), v["efficient"].clone()), (200, json!(false), json!(true)), "{v}");
    assert_eq!(lab.copies(), 0, "no copy was kept");
    assert_eq!(files_in(&lab.blobs().join("tmp")), 0);
    let r = lab.record(&hash).unwrap();
    assert_eq!((r["state"].clone(), r["dstBytes"].as_u64()), (json!("efficient"), Some(original.len() as u64)), "{r}");
    let s = lab.status();
    assert_eq!((s["efficient"].clone(), s["have"].clone(), s["left"].clone()), (json!(1), json!(0), json!(1)), "{s}");
    assert_eq!(lab.server.finish_video_job("ann", &hash, &digest_of(&original)), Ok(Mark::Efficient));

    // with the setting off, the same bytes are kept
    lab.set(json!({"skipIfSmaller": false}));
    assert_eq!(lab.put(1, &lab.clips[1].2.clone(), Some(&digest_of(&lab.clips[1].2))).0, 200);
    assert_eq!(lab.copies(), 1);

    // a copy that stops short of the clip's length is no copy of it
    let short = testdata::clip_textured(8, 10, 0);
    let (s, v) = lab.put(0, &short, Some(&digest_of(&short)));
    assert_eq!(s, 422, "{v}");
    assert!(v["error"].as_str().unwrap().contains("lasts"), "{v}");
    lab.finish();
}

/// `gc` removes a copy, and its record, once its video has been out of the library for a day; one younger, or still in the
/// library, stays.
#[test]
fn gc_removes_a_copy_a_day_after_its_video_is_gone() {
    let lab = lab("gc", clips(1), None);
    lab.set(json!({"enabled": true}));
    let copy = lab.rendition(0, 40);
    let hash = lab.clips[0].1.clone();
    assert_eq!(lab.put(0, &copy, Some(&digest_of(&copy))).0, 200);
    let blob = lab.blobs().join("video").join(&hash[..2]).join(&hash);
    let record = lab.data.join("users/ann/video").join(format!("{hash}.json"));
    assert!(blob.is_file() && record.is_file());
    // (made-up copies of videos no library has: an old one, a new one)
    let (gone, fresh) = (digest_of(b"gone"), digest_of(b"fresh"));
    for h in [&gone, &fresh] {
        let dir = lab.blobs().join("video").join(&h[..2]);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(h), b"copy").unwrap();
        std::fs::create_dir_all(lab.data.join("users/ann/video")).unwrap();
        std::fs::write(lab.data.join("users/ann/video").join(format!("{h}.json")), br#"{"state":"copy"}"#).unwrap();
    }
    for p in [&blob, &record] {
        old(p);
    }
    for p in [lab.blobs().join("video").join(&gone[..2]).join(&gone), lab.data.join("users/ann/video").join(format!("{gone}.json"))] {
        old(&p);
    }
    let r = gc::run(&lab.data, true).unwrap();
    assert_eq!(r.removed, 2, "a dry run counts: the old copy and its record, not the one in the library, not the new one: {r:?}");
    assert!(lab.blobs().join("video").join(&gone[..2]).join(&gone).is_file(), "a dry run removes nothing");
    let r = gc::run(&lab.data, false).unwrap();
    assert_eq!((r.removed, r.errors.len()), (2, 0), "{r:?}");
    assert!(!lab.blobs().join("video").join(&gone[..2]).join(&gone).exists());
    assert!(!lab.data.join("users/ann/video").join(format!("{gone}.json")).exists());
    assert!(blob.is_file() && record.is_file() && lab.blobs().join("video").join(&fresh[..2]).join(&fresh).is_file());
    lab.finish();
}

// ---- the queue --------------------------------------------------------------------------------------------------------

/// The queue worked through by the server's own (fake) encoder: counts, pause and resume, the failed list with a reason,
/// retry, what was saved, the devices' activity; and settings applied again make the old copies again.
#[test]
fn the_queue_counts_pauses_retries_and_applies_again() {
    let fake = Fake::new(40);
    fake.refuse.lock().unwrap().push("bad.mp4".into());
    let lab = lab("queue", clips(4), Some(fake.clone()));
    let s = lab.status();
    assert_eq!((s["state"].clone(), s["total"].clone(), s["have"].clone(), s["left"].clone()), (json!("off"), json!(4), json!(0), json!(4)), "{s}");

    // paused, then on: nothing is encoded
    lab.admin("POST", "video/pause", None);
    lab.set(json!({"enabled": true, "serverFallback": true, "skipIfSmaller": true}));
    std::thread::sleep(Duration::from_millis(300));
    let s = lab.status();
    assert_eq!((s["state"].clone(), s["left"].clone()), (json!("paused"), json!(4)), "{s}");
    assert_eq!(fake.started.load(Ordering::SeqCst), 0);

    // on again: three are made, one fails with its reason
    lab.admin("POST", "video/resume", None);
    wait_until("the queue to empty", || lab.status()["left"] == json!(0));
    let s = lab.status();
    assert_eq!(
        (s["have"].clone(), s["failed"].clone(), s["efficient"].clone(), s["total"].clone(), s["state"].clone()),
        (json!(3), json!(1), json!(0), json!(4), json!("idle")),
        "{s}"
    );
    let f = &s["failures"][0];
    assert_eq!(
        (f["name"].clone(), f["kind"].clone(), f["reason"].clone(), f["user"].clone()),
        (json!("bad.mp4"), json!("unsupported"), json!("no thanks"), json!("ann")),
        "{s}"
    );
    assert_eq!(lab.copies(), 3);
    assert_eq!(*fake.ran_on.lock().unwrap(), (1, "lc-video-encode".to_string()), "an encode has a pool of one thread to itself");
    let (sizes, copies): (u64, u64) = lab.clips[..3].iter().map(|c| c.2.len() as u64).fold((0, 0), |(a, b), n| (a + n, b + n * 40 / 100));
    assert_eq!((s["bytes"]["encoded"].as_u64(), s["bytes"]["saved"].as_u64()), (Some(copies), Some(sizes - copies)), "{s}");
    let r = lab.record(&lab.clips[0].1).unwrap();
    assert_eq!(
        (r["state"].clone(), r["profile"]["quality"].clone(), r["profile"]["maxHeight"].clone()),
        (json!("copy"), json!("balanced"), json!(1080)),
        "{r}"
    );
    let r = lab.record(&lab.clips[3].1).unwrap();
    assert_eq!((r["state"].clone(), r["kind"].clone()), (json!("failed"), json!("unsupported")), "{r}");

    // the devices' status popover
    let (_, a, _) = call("GET", &format!("{}/api/activity", lab.base), &lab.token, None, &[]);
    assert_eq!(json_of(&a)["video"]["done"], json!(3), "{}", String::from_utf8_lossy(&a));
    assert_eq!(json_of(&a)["video"]["total"], json!(3), "the failed clip is not in the batch");

    // retry: it fails again; then the encoder takes it
    let (_, s) = lab.admin("POST", "video/retry-failed", None);
    assert!(s["failed"].as_u64().is_some());
    wait_until("the second failure", || {
        let s = lab.status();
        s["failed"] == json!(1) && s["left"] == json!(0)
    });
    assert_eq!(fake.started.load(Ordering::SeqCst), 5, "three, the failure, the failure again");
    fake.refuse.lock().unwrap().clear();
    lab.admin("POST", "video/retry-failed", None);
    wait_until("all four", || lab.status()["have"] == json!(4));
    assert_eq!(lab.status()["failed"], json!(0));

    // the settings changed: nothing is redone until they are applied, then every older copy is made again
    lab.set(json!({"quality": "smaller", "maxHeight": 720}));
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(lab.status()["left"], json!(0), "a change alone redoes nothing");
    let (s, v) = lab.admin("POST", "video/apply", None);
    assert_eq!(s, 200);
    assert!(v["queuedAgain"].as_u64().unwrap() <= 4 && v["left"].as_u64().is_some(), "{v}");
    wait_until("the copies to be made again", || {
        let s = lab.status();
        s["left"] == json!(0) && s["queuedAgain"] == json!(0)
    });
    let r = lab.record(&lab.clips[0].1).unwrap();
    assert_eq!((r["profile"]["quality"].clone(), r["profile"]["maxHeight"].clone()), (json!("smaller"), json!(720)), "{r}");
    assert_eq!(lab.status()["have"], json!(4));
    lab.finish();
}

/// The server's own encoder never keeps a copy that is not smaller enough, and a clip it marked is not tried again.
#[test]
fn the_servers_own_copies_are_dropped_when_not_smaller_enough() {
    let fake = Fake::new(95);
    let lab = lab("efficient-queue", clips(2), Some(fake.clone()));
    lab.set(json!({"enabled": true, "serverFallback": true, "skipIfSmaller": true}));
    wait_until("both to be judged", || lab.status()["efficient"] == json!(2));
    let s = lab.status();
    assert_eq!((s["have"].clone(), s["left"].clone(), s["failed"].clone(), s["state"].clone()), (json!(0), json!(0), json!(0), json!("idle")), "{s}");
    assert_eq!((lab.copies(), files_in(&lab.blobs().join("tmp"))), (0, 0), "no copy and no leftover");
    assert_eq!(fake.made.load(Ordering::SeqCst), 2);
    assert_eq!(lab.record(&lab.clips[0].1).unwrap()["state"], json!("efficient"));
    // asked again, nothing is encoded: the mark stands
    lab.admin("POST", "video/apply", None);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(fake.started.load(Ordering::SeqCst), 2);
    // with the setting off the apply keeps them
    lab.set(json!({"skipIfSmaller": false}));
    lab.admin("POST", "video/apply", None);
    wait_until("the copies to be kept", || lab.status()["have"] == json!(2));
    assert_eq!(lab.copies(), 2);
    lab.finish();
}

/// A pause stops the encode in flight and gives the clip back (it is not a failure); going on finishes it.
#[test]
fn a_pause_stops_the_encode_in_flight() {
    let fake = Fake::new(40);
    fake.hold.store(true, Ordering::SeqCst);
    let lab = lab("pause", clips(1), Some(fake.clone()));
    lab.set(json!({"enabled": true, "serverFallback": true}));
    wait_until("the encode to start", || lab.status()["encoding"].as_array().is_some_and(|e| e.len() == 1));
    let s = lab.status();
    assert_eq!((s["state"].clone(), s["encoding"][0]["name"].clone(), s["left"].clone()), (json!("encoding"), json!("a.mp4"), json!(1)), "{s}");
    lab.admin("POST", "video/pause", None);
    wait_until("the encode to stop", || lab.status()["encoding"].as_array().is_some_and(Vec::is_empty));
    let s = lab.status();
    assert_eq!((s["failed"].clone(), s["left"].clone(), s["have"].clone()), (json!(0), json!(1), json!(0)), "given back, not failed: {s}");
    fake.hold.store(false, Ordering::SeqCst);
    lab.admin("POST", "video/resume", None);
    wait_until("the copy", || lab.status()["have"] == json!(1));
    lab.finish();
}

/// With nobody to encode, the queue says it waits for a helper; a helper's claim is the Rust surface V12 puts routes on:
/// each clip once, a failure with its reason, a finished upload vouched for.
#[test]
fn with_no_encoder_the_queue_waits_for_a_helper_and_hands_clips_to_one() {
    let lab = lab("helper", clips(4), None);
    lab.set(json!({"enabled": true, "serverFallback": false, "helper": true}));
    let s = lab.status();
    assert_eq!(
        (s["state"].clone(), s["waitingFor"].clone(), s["left"].clone(), s["serverCanEncode"].clone()),
        (json!("waiting"), json!("helper"), json!(4), json!(false)),
        "{s}"
    );
    assert_eq!(s["helper"]["seen"], Value::Null);
    assert!(lab.server.next_video_job("ann", Worker::Server).is_none(), "the server does not encode with the fallback off");

    // a helper claims: every clip once, then none
    let jobs: Vec<_> = (0..4).map(|_| lab.server.next_video_job("ann", Worker::Helper).expect("a clip to encode")).collect();
    let mut hashes: Vec<&str> = jobs.iter().map(|j| j.clip.hash.as_str()).collect();
    hashes.sort();
    hashes.dedup();
    assert_eq!(hashes.len(), 4, "no clip is handed out twice");
    assert!(lab.server.next_video_job("ann", Worker::Helper).is_none());
    assert_eq!((jobs[0].user.as_str(), jobs[0].settings.codec), ("ann", Codec::H264));
    let s = lab.status();
    assert_eq!((s["state"].clone(), s["helper"]["there"].clone(), s["left"].clone()), (json!("encoding"), json!(true), json!(4)), "{s}");
    assert!(s["helper"]["seen"].as_u64().is_some());

    // one fails: listed with its reason until retried (and it is handed out again then)
    let bad = jobs.iter().find(|j| j.clip.name == "bad.mp4").unwrap();
    lab.server.fail_video_job(bad, Failure { kind: FailKind::Unreadable, reason: "the file is cut short".into() });
    let s = lab.status();
    assert_eq!((s["failed"].clone(), s["left"].clone()), (json!(1), json!(3)), "{s}");
    assert_eq!((s["failures"][0]["reason"].clone(), s["failures"][0]["kind"].clone()), (json!("the file is cut short"), json!("unreadable")), "{s}");

    // one is uploaded and vouched for
    let a = jobs.iter().position(|j| j.clip.name == "a.mp4").unwrap();
    let copy = lab.rendition(0, 40);
    assert_eq!(lab.put(0, &copy, Some(&digest_of(&copy))).0, 200);
    assert_eq!(lab.server.finish_video_job("ann", &jobs[a].clip.hash, &digest_of(&copy)), Ok(Mark::Copy));
    let s = lab.status();
    assert_eq!((s["have"].clone(), s["left"].clone()), (json!(1), json!(2)), "{s}");

    // a pause hands nothing out; retrying puts the failed clip back in the queue
    lab.admin("POST", "video/pause", None);
    assert!(lab.server.next_video_job("ann", Worker::Helper).is_none());
    lab.admin("POST", "video/resume", None);
    assert_eq!(lab.admin("POST", "video/retry-failed", None).1["failed"], json!(0));
    let again = lab.server.next_video_job("ann", Worker::Helper).expect("the failed clip again");
    assert_eq!(again.clip.name, "bad.mp4");

    // no helper and no fallback: nothing can encode
    lab.set(json!({"helper": false}));
    let s = lab.status();
    assert_eq!((s["state"].clone(), s["waitingFor"].clone()), (json!("waiting"), json!("nothing")), "{s}");
    assert!(lab.server.next_video_job("ann", Worker::Helper).is_none());
    lab.finish();
}

/// The real pure-Rust encoder, end to end: a clip in a library folder gets a smaller H.264 copy of the same length.
#[test]
fn the_servers_software_encoder_makes_a_real_copy() {
    let lab = lab("software", clips(1), None);
    lab.set(json!({"enabled": true, "serverFallback": true, "helper": false, "maxHeight": 480, "skipIfSmaller": false}));
    wait_until("the copy", || lab.status()["have"] == json!(1) || lab.status()["failed"] == json!(1));
    let s = lab.status();
    assert_eq!((s["have"].clone(), s["failed"].clone()), (json!(1), json!(0)), "{s}");
    let hash = &lab.clips[0].1;
    let blob = lab.blobs().join("video").join(&hash[..2]).join(hash);
    let info = lightcraft_video::probe_path(&blob).unwrap();
    assert_eq!((info.width, info.height, info.codec, info.frames), (128, 96, "h264", 20), "no clip is made bigger than it is");
    assert!((info.duration - 2.0).abs() < 0.15, "{}", info.duration);
    let r = lab.record(hash).unwrap();
    assert_eq!((r["codec"].clone(), r["hardware"].clone(), r["height"].clone()), (json!("h264"), json!(false), json!(96)), "{r}");
    // and the copy is served
    let (s, bytes, _) = call("GET", &lab.url(hash), &lab.token, None, &[]);
    assert_eq!((s, bytes), (200, std::fs::read(&blob).unwrap()));
    // HEVC is not something this encoder makes: the clips wait for a helper that does
    lab.set(json!({"codec": "hevc", "helper": true}));
    let s = lab.status();
    assert_eq!((s["serverCanEncode"].clone(), s["settings"]["codec"].clone()), (json!(false), json!("hevc")), "{s}");
    lab.finish();
}

fn png_bytes() -> Vec<u8> {
    let (w, h) = (80usize, 60usize);
    let data: Vec<[u8; 4]> = (0..w * h).map(|i| [(i % w * 3) as u8, (i / w * 4) as u8, 9, 255]).collect();
    let img = lightcraft_raster::Rgba8 { width: w, height: h, data };
    lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap()
}

/// A device that took over building previews is given photos, never clips: a clip's original can be gigabytes.
#[test]
fn clips_are_never_given_to_a_device_to_build_previews_of() {
    let png = png_bytes();
    let lab = lab_with("previews", clips(2), vec![("pic.png", png.clone())], None);
    // (the previews the scan built are removed: the photo and both clips lack them now)
    std::fs::remove_dir_all(lab.blobs().join("smart")).unwrap();
    std::fs::remove_dir_all(lab.blobs().join("mini")).unwrap();
    let (s, b, _) = call(
        "POST",
        &format!("{}/api/work/claim", lab.base),
        &lab.token,
        Some(json!({"parts": {"previews": "any"}, "max": 20}).to_string().as_bytes()),
        &[],
    );
    assert_eq!(s, 200, "{}", String::from_utf8_lossy(&b));
    let keys: Vec<String> = json_of(&b)["items"].as_array().unwrap().iter().map(|i| i["key"].as_str().unwrap().to_string()).collect();
    assert_eq!(keys, vec![digest_of(&png)], "the picture, and neither clip");
    lab.finish();
}
