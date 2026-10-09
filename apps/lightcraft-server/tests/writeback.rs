//! Library folders the server may write in: XMP sidecars beside the photos, and uploaded photos filed into
//! an imports folder. By default (and in every test that doesn't say otherwise) a folder is only read.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use lightcraft_engine::Selection;
use lightcraft_engine::Session;
use lightcraft_server::{Config, Server, accounts, folders};
use serde_json::{Value, json};

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-server-writeback-{tag}-{}", std::process::id()));
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

const XMP: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmp:Rating="4"></rdf:Description></rdf:RDF></x:xmpmeta>"#;

fn scanned(server: &Server, after: u64) -> folders::Status {
    for _ in 0..1200 {
        let s = server.folder_status("ann");
        if s.scans > after && !s.scanning && s.previews == 0 {
            return s;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("no scan: {:?}", server.folder_status("ann"));
}

fn sync(s: &mut Session) {
    let st = s.execute("sync.now", &json!({"wait": true})).unwrap();
    assert_eq!(st["state"], "idle", "{st}");
}

/// Wait up to `secs` for `f` to hold.
fn until(secs: u64, what: &str, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("timed out waiting for {what}");
}

/// Every file below `dir`, relative.
fn tree(dir: &Path) -> BTreeSet<String> {
    fn walk(dir: &Path, base: &Path, out: &mut BTreeSet<String>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            if e.path().is_dir() {
                walk(&e.path(), base, out);
            } else {
                out.insert(e.path().strip_prefix(base).unwrap().to_string_lossy().to_string());
            }
        }
    }
    let mut out = BTreeSet::new();
    walk(dir, dir, &mut out);
    out
}

fn rating_of(sidecar: &Path) -> Option<u8> {
    let text = std::fs::read_to_string(sidecar).ok()?;
    lightcraft_engine::sidecar::parse_sidecar(&text, false).ok()?.rating
}

struct Fx {
    root: PathBuf,
    photos: PathBuf,
    server: Server,
    url: String,
}

/// A user whose folder `Photos` holds a.png and b.png (b with a sidecar saying 4 stars).
fn fixture(tag: &str, writable: bool, imports: bool) -> Fx {
    let root = temp(tag);
    let photos = root.join("nas/photos");
    write_png(&photos.join("a.png"), 1);
    write_png(&photos.join("b.png"), 2);
    std::fs::write(photos.join("b.xmp"), XMP).unwrap();
    let data = root.join("data");
    accounts::set_user(&data, "ann", "correct horse", false).unwrap();
    accounts::add_folder(&data, "ann", &photos.to_string_lossy(), Some("Photos")).unwrap();
    if writable || imports {
        accounts::set_folder_mode(&data, "ann", "Photos", Some(true), imports.then_some(true)).unwrap();
    }
    let mut cfg = Config::new(&data, "127.0.0.1:0");
    cfg.scan_interval = None;
    cfg.preview_threads = 1;
    let server = Server::start(cfg).unwrap();
    let url = format!("http://{}", server.addr());
    scanned(&server, 0);
    Fx { root, photos, server, url }
}

fn device(fx: &Fx, name: &str) -> Session {
    let mut dev = Session::new().with_fs();
    dev.open_library(fx.root.join(name), false).unwrap();
    dev.execute("sync.signIn", &json!({"server": fx.url, "user": "ann", "password": "correct horse", "device": name})).unwrap();
    sync(&mut dev);
    dev
}

fn id_at(s: &Session, place: &str) -> lightcraft_engine::catalog::PhotoId {
    s.catalog.photos().find(|p| p.server_path.as_deref() == Some(place)).unwrap_or_else(|| panic!("no photo at {place}")).id
}

#[test]
fn a_folder_is_only_read_unless_an_admin_makes_it_writable() {
    let fx = fixture("readonly", false, false);
    let before = tree(&fx.photos);
    let mut dev = device(&fx, "d1");
    dev.selection = Selection::single(id_at(&dev, "Photos/a.png"));
    dev.execute("photo.rate", &json!({"rating": 3})).unwrap();
    sync(&mut dev);
    // longer than the server waits before writing
    std::thread::sleep(Duration::from_secs(5));
    assert_eq!(tree(&fx.photos), before, "nothing was written in a read-only folder");
    drop(fx.server);
    let _ = std::fs::remove_dir_all(&fx.root);
}

#[test]
fn a_writable_folder_gets_sidecars_and_never_overwrites_another_programs_edit() {
    let fx = fixture("sidecars", true, false);
    let a_bytes = std::fs::read(fx.photos.join("a.png")).unwrap();
    let mut dev = device(&fx, "d1");
    let (a, b) = (id_at(&dev, "Photos/a.png"), id_at(&dev, "Photos/b.png"));
    assert_eq!(dev.catalog.photo(b).unwrap().rating, 4, "read from b.xmp");
    // an edit made on a device: a sidecar appears beside the photo, with the rating
    dev.selection = Selection::single(a);
    dev.execute("photo.rate", &json!({"rating": 3})).unwrap();
    dev.execute("develop.set", &json!({"control": "light.exposure", "value": 0.7})).unwrap();
    sync(&mut dev);
    until(20, "a.xmp", || rating_of(&fx.photos.join("a.xmp")) == Some(3));
    assert_eq!(std::fs::read(fx.photos.join("a.png")).unwrap(), a_bytes, "the photo itself is never touched");
    let text = std::fs::read_to_string(fx.photos.join("a.xmp")).unwrap();
    assert!(text.contains("exposure") || text.contains("Exposure"), "the edit is in it: {text}");
    // the server's own sidecar is not an outside edit: the next scan reads nothing from it
    fx.server.scan("ann");
    let st = scanned(&fx.server, 1);
    assert_eq!((st.added, st.sidecars), (0, 0), "{st:?}");

    // another program edits b's sidecar; then a device rates b: the server doesn't overwrite what it hasn't read
    std::thread::sleep(Duration::from_millis(50));
    std::fs::write(fx.photos.join("b.xmp"), XMP.replace("Rating=\"4\"", "Rating=\"2\"")).unwrap();
    dev.selection = Selection::single(b);
    dev.execute("photo.rate", &json!({"rating": 5})).unwrap();
    sync(&mut dev);
    std::thread::sleep(Duration::from_secs(6));
    assert_eq!(rating_of(&fx.photos.join("b.xmp")), Some(2), "left alone");
    // it asked for a scan, which read the sidecar: what the other program wrote wins
    until(30, "the scan to read b.xmp", || fx.server.folder_status("ann").scans >= 3 && !fx.server.folder_status("ann").scanning);
    sync(&mut dev);
    assert_eq!(dev.catalog.photo(b).unwrap().rating, 2);
    // a later change is written (merged into that file)
    dev.execute("photo.rate", &json!({"rating": 1})).unwrap();
    sync(&mut dev);
    until(20, "b.xmp to follow", || rating_of(&fx.photos.join("b.xmp")) == Some(1));
    drop(fx.server);
    let _ = std::fs::remove_dir_all(&fx.root);
}

#[test]
fn uploads_are_filed_into_the_imports_folder_and_the_servers_copy_goes() {
    let root = temp("imports");
    let incoming = root.join("nas/incoming");
    std::fs::create_dir_all(&incoming).unwrap();
    let data = root.join("data");
    accounts::set_user(&data, "ann", "correct horse", false).unwrap();
    accounts::add_folder(&data, "ann", &incoming.to_string_lossy(), Some("Incoming")).unwrap();
    accounts::set_folder_mode(&data, "ann", "Incoming", Some(true), Some(true)).unwrap();
    let mut cfg = Config::new(&data, "127.0.0.1:0");
    cfg.scan_interval = None;
    cfg.preview_threads = 1;
    let server = Server::start(cfg).unwrap();
    let url = format!("http://{}", server.addr());
    scanned(&server, 0);

    // a device with a photo signs in and uploads it
    write_png(&root.join("in/x.png"), 5);
    let bytes = std::fs::read(root.join("in/x.png")).unwrap();
    let mut dev = Session::new().with_fs();
    dev.open_library(root.join("d1"), false).unwrap();
    dev.execute("library.import", &json!({"paths": [root.join("in").to_string_lossy()]})).unwrap();
    dev.execute("sync.signIn", &json!({"server": url, "user": "ann", "password": "correct horse", "device": "d1"})).unwrap();
    sync(&mut dev);
    // filed as <year>/<date>/<name>, and the photo's place on the server is that file
    until(30, "the upload to be filed", || tree(&incoming).iter().any(|f| f.ends_with("x.png")));
    let files = tree(&incoming);
    let filed = files.iter().find(|f| f.ends_with("x.png")).unwrap().clone();
    let parts: Vec<&str> = filed.split('/').collect();
    assert_eq!(parts.len(), 3, "{filed}");
    assert!(parts[1].starts_with(parts[0]) && parts[1].len() == 10, "year, then the date: {filed}");
    assert_eq!(std::fs::read(incoming.join(&filed)).unwrap(), bytes, "the same bytes");
    assert_eq!(files.len(), 1, "nothing else is left: {files:?}");
    let blobs = data.join("users/ann/blobs");
    until(10, "the server's copy to go", || tree(&blobs.join("original")).is_empty());
    assert!(!tree(&blobs.join("smart")).is_empty() && !tree(&blobs.join("mini")).is_empty(), "previews stay in the server's store");

    // another device sees it as a photo in the folder, and can download the original from there
    let mut other = Session::new().with_fs();
    other.open_library(root.join("d2"), false).unwrap();
    other.execute("sync.signIn", &json!({"server": url, "user": "ann", "password": "correct horse", "device": "d2"})).unwrap();
    until(30, "the photo's place", || {
        sync(&mut other);
        other.catalog.photos().any(|p| p.server_path.as_deref() == Some(&format!("Incoming/{filed}")))
    });
    let p = other.catalog.photos().next().unwrap();
    let hash = p.content_hash.clone().unwrap();
    let token = other.sync_state().unwrap().config.token.clone();
    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
    let mut r = agent.get(format!("{url}/api/blobs/original/{hash}")).header("Authorization", format!("Bearer {token}")).call().unwrap();
    assert_eq!(r.status().as_u16(), 200);
    assert_eq!(r.body_mut().read_to_vec().unwrap(), bytes);
    // the first device still has its own file, and a later scan finds nothing new to add
    server.scan("ann");
    let st = scanned(&server, 1);
    assert_eq!((st.added, st.failed), (0, 0), "{st:?}");
    assert_eq!(tree(&incoming).len(), 1);
    let _: Value = json!({});
    drop(server);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn folder_modes_are_checked() {
    let root = temp("modes");
    let data = root.join("data");
    let a = root.join("nas/a");
    let b = root.join("nas/b");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    accounts::set_user(&data, "ann", "correct horse", false).unwrap();
    accounts::add_folder(&data, "ann", &a.to_string_lossy(), Some("A")).unwrap();
    accounts::add_folder(&data, "ann", &b.to_string_lossy(), Some("B")).unwrap();
    let folder = |n: &str| accounts::read_users(&data).unwrap().users["ann"].folders.iter().find(|f| f.name == n).cloned().unwrap();
    assert!(!folder("A").writable && !folder("A").imports, "read-only by default");
    // imports implies writable, and there is only one
    accounts::set_folder_mode(&data, "ann", "A", None, Some(true)).unwrap();
    assert!(folder("A").writable && folder("A").imports);
    accounts::set_folder_mode(&data, "ann", "B", None, Some(true)).unwrap();
    assert!(folder("B").imports && !folder("A").imports && folder("A").writable);
    // read-only again takes the imports with it
    accounts::set_folder_mode(&data, "ann", "B", Some(false), None).unwrap();
    assert!(!folder("B").writable && !folder("B").imports);
    assert!(accounts::set_folder_mode(&data, "ann", "Nope", Some(true), None).is_err());
    // a folder the server can't write in is refused (not when running as a user who can write anywhere)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&b, std::fs::Permissions::from_mode(0o555)).unwrap();
        if std::fs::write(b.join("probe"), b"x").is_err() {
            let e = accounts::set_folder_mode(&data, "ann", "B", Some(true), None).unwrap_err();
            assert!(e.contains("can't write"), "{e}");
            assert!(!folder("B").writable, "the refusal changed nothing");
        }
        std::fs::set_permissions(&b, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let _ = std::fs::remove_dir_all(&root);
}
