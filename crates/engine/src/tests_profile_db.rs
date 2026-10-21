use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use super::*;

const LENS_XML: &str = r#"<lensdatabase>
  <camera><maker>Testcam</maker><model>TC-1</model><mount>T</mount><cropfactor>1.5</cropfactor></camera>
  <lens><maker>Testcam</maker><model>Testcam 18-55mm f/3.5-5.6</model><mount>T</mount><cropfactor>1.5</cropfactor>
    <calibration><distortion model="poly3" focal="18" k1="-0.03" /></calibration>
  </lens></lensdatabase>"#;

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-profile-db-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn camera_json(model: &str) -> Vec<u8> {
    let p = CameraProfile::new(model, 3, 900, lightcraft_color::Mat3([[1.1, -0.1, 0.0], [0.0, 1.0, 0.0], [0.0, -0.1, 1.1]]), None);
    serde_json::to_vec(&p).unwrap()
}

fn entry(kind: &str, name: &str, model: Option<&str>, bytes: &[u8]) -> Entry {
    Entry {
        kind: kind.into(),
        name: name.into(),
        model: model.map(Into::into),
        source: "test".into(),
        sha256: sha256_hex(bytes),
        size: bytes.len() as u64,
    }
}

fn manifest(serial: u64, files: Vec<Entry>) -> Manifest {
    let source = Source {
        id: "test".into(),
        title: "Test profiles".into(),
        licence: "CC0-1.0".into(),
        url: String::new(),
        credit: String::new(),
        revision: String::new(),
    };
    Manifest { format: FORMAT, serial, published: "2026-10-21".into(), sources: vec![source], files }
}

/// A feed folder: `index.json` and `files/<sha256>`.
fn feed(dir: &std::path::Path, m: &Manifest, files: &[&[u8]]) {
    std::fs::create_dir_all(dir.join("files")).unwrap();
    for f in files {
        std::fs::write(dir.join("files").join(sha256_hex(f)), f).unwrap();
    }
    std::fs::write(dir.join("index.json"), serde_json::to_vec(m).unwrap()).unwrap();
}

/// A local server for the feed folder `root` (std `TcpListener`; never the internet). Counts the file requests.
fn serve(root: PathBuf) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/feed", l.local_addr().unwrap());
    let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = count.clone();
    std::thread::spawn(move || {
        for mut s in l.incoming().flatten() {
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut first = String::new();
            let _ = r.read_line(&mut first);
            loop {
                let mut line = String::new();
                if r.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                    break;
                }
            }
            let path = first.split_whitespace().nth(1).unwrap_or_default().trim_start_matches("/feed/").to_string();
            if path.starts_with("files/") {
                counted.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            match std::fs::read(root.join(&path)) {
                Ok(body) => {
                    let _ = s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes());
                    let _ = s.write_all(&body);
                }
                Err(_) => {
                    let _ = s.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                }
            }
        }
    });
    (base, count)
}

#[test]
fn manifest_checks_reject_bad_indexes() {
    let lens = LENS_XML.as_bytes();
    let good = manifest(1, vec![entry(KIND_LENS, "test.xml", None, lens)]);
    assert!(Manifest::parse(&serde_json::to_vec(&good).unwrap()).is_ok());
    let newer = serde_json::json!({"format": FORMAT + 1, "whatever": true});
    assert!(Manifest::parse(newer.to_string().as_bytes()).unwrap_err().contains("update the app"));
    assert!(Manifest::parse(b"not json").is_err());
    assert!(Manifest::parse(&vec![b' '; MAX_INDEX as usize + 1]).is_err());
    let mut bad = good.clone();
    bad.files[0].sha256 = "../../etc/passwd".into();
    assert!(Manifest::parse(&serde_json::to_vec(&bad).unwrap()).unwrap_err().contains("SHA-256"));
    let mut bad = good.clone();
    bad.files[0].sha256 = bad.files[0].sha256.to_uppercase();
    assert!(Manifest::parse(&serde_json::to_vec(&bad).unwrap()).is_err(), "hashes are lower-case hex");
    let mut bad = good.clone();
    bad.files[0].size = MAX_FILE + 1;
    assert!(Manifest::parse(&serde_json::to_vec(&bad).unwrap()).is_err());
    let mut bad = good.clone();
    bad.files[0].source = "elsewhere".into();
    assert!(Manifest::parse(&serde_json::to_vec(&bad).unwrap()).is_err(), "every file names a listed source");
    let cam = camera_json("TC-1");
    let bad = manifest(1, vec![entry(KIND_CAMERA, "a.json", Some("TC-1"), &cam), entry(KIND_CAMERA, "b.json", Some(" TC-1 "), &cam)]);
    assert!(Manifest::parse(&serde_json::to_vec(&bad).unwrap()).unwrap_err().contains("two camera profiles"));
    let bad = manifest(1, vec![entry(KIND_CAMERA, "a.json", None, &cam)]);
    assert!(Manifest::parse(&serde_json::to_vec(&bad).unwrap()).is_err(), "a camera profile names its model");
    // a kind this build doesn't read is allowed and skipped
    let other = manifest(1, vec![entry("dcp-free-future-kind", "x.bin", None, b"x")]);
    let parsed = Manifest::parse(&serde_json::to_vec(&other).unwrap()).unwrap();
    assert_eq!(parsed.known().count(), 0);
}

#[test]
fn feed_base_accepts_https_and_loopback_only() {
    assert_eq!(feed_base("https://example.com/profiles/").as_deref(), Some("https://example.com/profiles"));
    assert_eq!(feed_base(" https://example.com/p/index.json ").as_deref(), Some("https://example.com/p"));
    assert_eq!(feed_base("http://127.0.0.1:8080/feed").as_deref(), Some("http://127.0.0.1:8080/feed"));
    for bad in ["http://example.com/p", "http://127.0.0.1.example.com/p", "https://", "https:///x", "ftp://x", "https://x/a b", "https://x/?q=1", ""]
    {
        assert_eq!(feed_base(bad), None, "{bad}");
    }
}

#[test]
fn install_load_and_garbage() {
    let dir = scratch("install");
    let store = Store::at(dir.join("db"));
    assert!(store.load().unwrap().manifest.is_none(), "nothing installed");
    let (lens, cam) = (LENS_XML.as_bytes(), camera_json("TC-1"));
    let m = manifest(1, vec![entry(KIND_LENS, "test.xml", None, lens), entry(KIND_CAMERA, "TC-1.json", Some("TC-1"), &cam)]);
    let index = serde_json::to_vec(&m).unwrap();
    assert_eq!(store.missing(&m).len(), 2);
    assert!(store.install(&index, &m).is_err(), "files must be in place first");
    std::fs::create_dir_all(store.files_dir()).unwrap();
    std::fs::write(store.files_dir().join(sha256_hex(lens)), lens).unwrap();
    std::fs::write(store.files_dir().join(sha256_hex(&cam)), b"tampered with, same length?").unwrap();
    assert!(store.install(&index, &m).is_err(), "a file that doesn't match its hash is refused");
    assert!(!store.files_dir().join(sha256_hex(&cam)).exists(), "and deleted");
    std::fs::write(store.files_dir().join(sha256_hex(&cam)), &cam).unwrap();
    std::fs::write(store.files_dir().join("stray.part"), b"x").unwrap();
    store.install(&index, &m).unwrap();
    assert!(!store.files_dir().join("stray.part").exists(), "unlisted files are collected");
    let db = store.load().unwrap();
    assert_eq!((db.serial(), db.lenses.lenses.len(), db.lenses.cameras.len()), (1, 1, 1));
    assert!(db.cameras.contains_key("TC-1") && db.camera_key != 0 && db.skipped.is_empty(), "{:?}", db.skipped);
    // a damaged file is skipped when read, not fatal
    std::fs::write(store.files_dir().join(sha256_hex(lens)), b"<lensdatabase/>").unwrap();
    let db = store.load().unwrap();
    assert_eq!((db.lenses.lenses.len(), db.skipped.len()), (0, 1));
    store.remove().unwrap();
    assert!(!store.dir().exists());
    store.remove().unwrap();
}

#[test]
fn camera_key_follows_camera_profiles_only() {
    let (lens, cam, cam2) = (LENS_XML.as_bytes(), camera_json("TC-1"), camera_json("TC-2"));
    let only_lens = manifest(1, vec![entry(KIND_LENS, "test.xml", None, lens)]);
    let with_cam = manifest(2, vec![entry(KIND_LENS, "test.xml", None, lens), entry(KIND_CAMERA, "TC-1.json", Some("TC-1"), &cam)]);
    let more_lens = manifest(3, vec![entry(KIND_LENS, "test2.xml", None, b"<lensdatabase/>"), entry(KIND_CAMERA, "TC-1.json", Some("TC-1"), &cam)]);
    let other_cam = manifest(4, vec![entry(KIND_CAMERA, "TC-2.json", Some("TC-2"), &cam2)]);
    assert_eq!(camera_key_of(&only_lens), 0, "renders keep their keys without database camera profiles");
    assert_ne!(camera_key_of(&with_cam), 0);
    assert_eq!(camera_key_of(&with_cam), camera_key_of(&more_lens), "a lens-only change leaves renders alone");
    assert_ne!(camera_key_of(&with_cam), camera_key_of(&other_cam));
}

#[test]
fn update_downloads_only_what_is_new_and_refuses_older_feeds() {
    let dir = scratch("update");
    let root = dir.join("feed");
    let (lens, cam) = (LENS_XML.as_bytes(), camera_json("TC-1"));
    let v1 = manifest(1, vec![entry(KIND_LENS, "test.xml", None, lens)]);
    feed(&root, &v1, &[lens]);
    let (base, requests) = serve(root.clone());
    let store = Store::at(dir.join("db"));
    let cancel = AtomicBool::new(false);
    let mut seen = Vec::new();
    let got = update(&store, &format!("{base}/index.json"), &cancel, &mut |d, t| seen.push((d, t))).unwrap();
    assert_eq!(got, Outcome::Updated { serial: 1, files: 1, bytes: lens.len() as u64 });
    assert_eq!(seen.last(), Some(&(lens.len() as u64, lens.len() as u64)));
    assert_eq!(update(&store, &base, &cancel, &mut |_, _| ()).unwrap(), Outcome::UpToDate { serial: 1 });
    assert_eq!(requests.load(std::sync::atomic::Ordering::Relaxed), 1, "an unchanged feed downloads no files");
    // a new camera profile: only it is downloaded
    let v2 = manifest(2, vec![entry(KIND_LENS, "test.xml", None, lens), entry(KIND_CAMERA, "TC-1.json", Some("TC-1"), &cam)]);
    feed(&root, &v2, &[&cam]);
    assert_eq!(update(&store, &base, &cancel, &mut |_, _| ()).unwrap(), Outcome::Updated { serial: 2, files: 1, bytes: cam.len() as u64 });
    assert_eq!(requests.load(std::sync::atomic::Ordering::Relaxed), 2);
    assert!(store.load().unwrap().cameras.contains_key("TC-1"));
    // the server goes back to an older set: refused, the device keeps its own
    feed(&root, &v1, &[]);
    assert!(update(&store, &base, &cancel, &mut |_, _| ()).unwrap_err().contains("older"));
    assert_eq!(store.manifest().unwrap().map(|m| m.serial), Some(2));
    // a file the server lacks (or serves wrong) fails the update and leaves the installed set as it was
    let v3 = manifest(3, vec![entry(KIND_LENS, "test.xml", None, b"<lensdatabase></lensdatabase>")]);
    feed(&root, &v3, &[]);
    assert!(update(&store, &base, &cancel, &mut |_, _| ()).is_err());
    assert_eq!(store.manifest().unwrap().map(|m| m.serial), Some(2));
    assert!(update(&store, "http://example.com/feed", &cancel, &mut |_, _| ()).is_err(), "https only");
}

#[test]
fn build_writes_a_feed_the_app_installs() {
    let dir = scratch("build");
    let (lenses, cameras, out) = (dir.join("lensfun"), dir.join("cameras"), dir.join("out"));
    std::fs::create_dir_all(&lenses).unwrap();
    std::fs::create_dir_all(&cameras).unwrap();
    std::fs::write(lenses.join("mil-test.xml"), LENS_XML).unwrap();
    std::fs::write(lenses.join("README.txt"), "not a database").unwrap();
    std::fs::write(cameras.join("TC-1.json"), camera_json("TC-1")).unwrap();
    std::fs::write(cameras.join("TC-1.neutral.json"), b"{}").unwrap();
    let own = Source {
        id: "own".into(),
        title: "Own profiles".into(),
        licence: "MIT OR Apache-2.0".into(),
        url: String::new(),
        credit: "Me".into(),
        revision: String::new(),
    };
    let inputs = vec![
        Input { source: lensfun_source("abc123"), kind: KIND_LENS, dir: lenses.clone() },
        Input { source: own.clone(), kind: KIND_CAMERA, dir: cameras.clone() },
    ];
    let built = build(&out, &inputs, None, "2026-10-21", false).unwrap();
    assert_eq!((built.serial, built.files, built.lenses, built.cameras.as_slice(), built.added), (1, 2, 1, ["TC-1".to_string()].as_slice(), 2));
    assert_eq!(built.skipped.len(), 1, "{:?}", built.skipped);
    // the next build counts on, reuses unchanged files and keeps old ones unless pruned
    std::fs::write(cameras.join("TC-1.json"), camera_json("TC-1").iter().chain(b"\n").copied().collect::<Vec<u8>>()).unwrap();
    let again = build(&out, &inputs, None, "2026-10-22", false).unwrap();
    assert_eq!((again.serial, again.added, again.pruned), (2, 1, 0));
    assert_eq!(std::fs::read_dir(out.join("files")).unwrap().count(), 3);
    assert!(build(&out, &inputs, Some(2), "", false).unwrap_err().contains("must grow"));
    assert_eq!(build(&out, &inputs, None, "", true).unwrap().pruned, 1);
    // a damaged input stops the build and leaves the published feed as it was
    std::fs::write(lenses.join("broken.xml"), "<<<").unwrap();
    assert!(build(&out, &inputs, None, "", false).is_err());
    assert_eq!(Store::at(&out).manifest().unwrap().unwrap().serial, 3);
    std::fs::remove_file(lenses.join("broken.xml")).unwrap();
    assert!(build(&out, &[Input { source: own, kind: KIND_LENS, dir: cameras }], None, "", false).unwrap_err().contains("no xml files"));
    // the feed is what a device downloads
    let (base, _) = serve(out.clone());
    let store = Store::at(dir.join("device"));
    let got = update(&store, &base, &AtomicBool::new(false), &mut |_, _| ()).unwrap();
    assert!(matches!(got, Outcome::Updated { serial: 3, files: 2, .. }), "{got:?}");
    let db = store.load().unwrap();
    assert_eq!((db.lenses.lenses.len(), db.cameras.len(), db.manifest.unwrap().sources.len()), (1, 1, 2));
}

#[test]
fn lens_profile_says_where_a_photos_correction_comes_from() {
    use lightcraft_catalog::Op;
    use lightcraft_develop::{EmbeddedLens, LensOrigin};
    use serde_json::json;
    let mut s = crate::Session::with_demo();
    let id = s.catalog.photos().next().unwrap().id;
    let ask = |s: &mut crate::Session| s.execute("photo.lensProfile", &json!({"id": id.0})).unwrap();
    let r = ask(&mut s);
    assert_eq!((r["origin"].clone(), r["raw"].clone()), (json!(null), json!(false)), "{r}");
    let measured = EmbeddedLens { vignette: Some(Default::default()), origin: LensOrigin::Measured, ..Default::default() };
    s.catalog.apply(Op::SetEmbeddedLens { id, lens: Some(Box::new(measured)) }).unwrap();
    let r = ask(&mut s);
    assert_eq!((r["origin"].clone(), r["vignetting"].clone(), r["distortion"].clone()), (json!("measured"), json!(true), json!(false)));
    let profile = EmbeddedLens { warp: Some(Default::default()), origin: LensOrigin::Database { serial: 4 }, ..Default::default() };
    s.catalog.apply(Op::SetEmbeddedLens { id, lens: Some(Box::new(profile)) }).unwrap();
    let r = ask(&mut s);
    assert_eq!((r["origin"].clone(), r["serial"].clone(), r["distortion"].clone()), (json!("database"), json!(4), json!(true)));
    assert!(s.execute("photo.lensProfile", &json!({"id": u64::MAX})).is_err());
    // the brief status doesn't read the database
    let brief = s.execute("profileDb.status", &json!({"brief": true})).unwrap();
    assert!(brief.get("running").is_some() && brief.get("lenses").is_none(), "{brief}");
}
