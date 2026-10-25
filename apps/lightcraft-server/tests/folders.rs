//! Library folders end to end: photo folders on the server read in place, synced to a device
//! over real HTTP, and what scans do when files are added, moved, changed, removed or unmounted.

use std::path::{Path, PathBuf};
use std::time::Duration;

use lightcraft_engine::Session;
use lightcraft_engine::catalog::Source;
use lightcraft_engine::catalog::sync::proto::Usage;
use lightcraft_server::{Config, Server, accounts, folders};
use serde_json::{Value, json};

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-server-folders-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn png_bytes(seed: u8) -> Vec<u8> {
    let (w, h) = (80usize, 60usize);
    let data: Vec<[u8; 4]> = (0..w * h).map(|i| [(i % w * 3) as u8, (i / w * 4) as u8, seed, 255]).collect();
    let img = lightcraft_raster::Rgba8 { width: w, height: h, data };
    lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap()
}

fn write_png(path: &Path, seed: u8) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, png_bytes(seed)).unwrap();
}

const XMP: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmlns:dc="http://purl.org/dc/elements/1.1/" xmp:Rating="4"><dc:subject><rdf:Bag><rdf:li>beach</rdf:li></rdf:Bag></dc:subject></rdf:Description></rdf:RDF></x:xmpmeta>"#;

/// Wait for the scan after `after` (and its previews) to finish.
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

fn by_place<'a>(s: &'a Session, place: &str) -> Option<&'a lightcraft_engine::catalog::Photo> {
    s.catalog.photos().map(|p| &**p).find(|p| p.server_path.as_deref() == Some(place))
}

fn files_in(dir: &Path) -> usize {
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    rd.flatten().map(|e| if e.path().is_dir() { files_in(&e.path()) } else { 1 }).sum()
}

#[test]
fn library_folders_are_read_in_place() {
    let root = temp("main");
    let photos = root.join("nas/photos");
    write_png(&photos.join("2024/Trip/a.png"), 1);
    write_png(&photos.join("2024/Trip/b.png"), 2);
    std::fs::write(photos.join("2024/Trip/b.xmp"), XMP).unwrap();
    write_png(&photos.join("2025/c.png"), 3);
    write_png(&photos.join(".hidden/x.png"), 4);
    std::fs::write(photos.join("2025/notes.txt"), "not a photo").unwrap();
    let data = root.join("data");
    accounts::set_user(&data, "ann", "correct horse", false).unwrap();
    // only real folders outside the server's data
    assert!(accounts::add_folder(&data, "ann", &root.join("nope").to_string_lossy(), None).is_err());
    assert!(accounts::add_folder(&data, "ann", &data.to_string_lossy(), None).is_err());
    assert!(accounts::add_folder(&data, "ann", &root.to_string_lossy(), None).is_err(), "holds the data");
    let f = accounts::add_folder(&data, "ann", &photos.to_string_lossy(), Some("Photos")).unwrap();
    assert_eq!(f.name, "Photos");
    assert!(accounts::add_folder(&data, "ann", &photos.join("2024").to_string_lossy(), None).is_err(), "inside one");

    let mut cfg = Config::new(&data, "127.0.0.1:0");
    cfg.scan_interval = None;
    cfg.preview_threads = Some(1);
    let server = Server::start(cfg).unwrap();
    let url = format!("http://{}", server.addr());
    let st = scanned(&server, 0);
    assert_eq!((st.files, st.added, st.failed), (3, 3, 0), "{st:?}");

    // a device sees them in their folders, with their sidecar's rating and keywords
    let mut dev = Session::new().with_fs();
    dev.open_library(root.join("device"), false).unwrap();
    dev.execute("sync.signIn", &json!({"server": url, "user": "ann", "password": "correct horse", "device": "test"})).unwrap();
    sync(&mut dev);
    assert_eq!(dev.catalog.len(), 3);
    // the storage numbers tell what the server stores from what it only reads
    let u = dev.execute("sync.usage", &json!({"refresh": true})).unwrap();
    let on_disk: u64 = ["2024/Trip/a.png", "2024/Trip/b.png", "2025/c.png"].iter().map(|f| std::fs::metadata(photos.join(f)).unwrap().len()).sum();
    assert_eq!((u["server"]["folders"]["files"].as_u64(), u["server"]["folders"]["bytes"].as_u64()), (Some(3), Some(on_disk)), "{u}");
    assert_eq!(u["server"]["original"]["files"], 0, "folder photos are never copied: {u}");
    assert_eq!(u["server"]["mini"]["files"], 3, "{u}");
    // ...but they are the library's photos and take that room: its total counts them, on top of the
    // previews the server keeps of them
    let usage: Usage = serde_json::from_value(u["server"].clone()).unwrap();
    assert!(usage.stored() > 0, "the previews: {u}");
    assert_eq!(usage.total(), usage.stored() + on_disk, "{u}");
    if usage.disk.is_some() {
        // (these folders are on the disk of the server's data)
        assert_eq!(usage.folders_on_disk, on_disk, "{u}");
        assert_eq!(usage.on_disk(), usage.total(), "{u}");
    }
    let b = by_place(&dev, "Photos/2024/Trip/b.png").expect("b in its folder");
    assert_eq!((b.rating, b.meta.keywords.clone()), (4, vec!["beach".to_string()]));
    assert!(matches!(&b.source, Source::File { path } if path.starts_with("web/")));
    let folders = lightcraft_engine::catalog::query::server_folders(&dev.catalog);
    assert_eq!(folders.get("Photos"), Some(&3));
    assert_eq!(folders.get("Photos/2024/Trip"), Some(&2));
    let (a_id, b_id) = (by_place(&dev, "Photos/2024/Trip/a.png").unwrap().id, b.id);
    // (a small preview comes down once a grid shows the photo)
    dev.thumb_job(a_id, 256);
    sync(&mut dev);
    assert!(dev.render_now(a_id, 64, 64).is_ok(), "renders from the previews the server built");

    // originals come straight from the folder; nothing was copied
    let blobs = data.join("users/ann/blobs");
    assert_eq!(files_in(&blobs.join("original")), 0);
    assert_eq!((files_in(&blobs.join("smart")), files_in(&blobs.join("mini"))), (3, 3));
    let hash = by_place(&dev, "Photos/2025/c.png").unwrap().content_hash.clone().unwrap();
    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
    let token = serde_json::from_slice::<serde_json::Value>(&std::fs::read(root.join("device/sync.json")).unwrap()).unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();
    let mut r = agent.get(format!("{url}/api/blobs/original/{hash}")).header("Authorization", format!("Bearer {token}")).call().unwrap();
    assert_eq!(r.status().as_u16(), 200);
    assert_eq!(r.body_mut().read_to_vec().unwrap(), std::fs::read(photos.join("2025/c.png")).unwrap());

    // edits made on the device stay with the photo; an album holds folder photos
    dev.selection = lightcraft_engine::Selection::single(a_id);
    dev.execute("develop.set", &json!({"control": "light.exposure", "value": 0.5})).unwrap();
    dev.execute("library.select", &json!({"ids": [a_id.0, b_id.0]})).unwrap();
    dev.execute("album.create", &json!({"name": "Best", "addSelected": true})).unwrap();
    sync(&mut dev);

    // moved on the server: the same photo, edits and album kept; removed on the device: stays
    // removed; changed in place: new content, same photo; a new file: a new photo
    std::fs::create_dir_all(photos.join("2026")).unwrap();
    std::fs::rename(photos.join("2024/Trip/a.png"), photos.join("2026/a.png")).unwrap();
    let c_id = by_place(&dev, "Photos/2025/c.png").unwrap().id;
    dev.execute("photo.delete", &json!({"ids": [c_id.0]})).unwrap();
    dev.execute("photo.deletePermanently", &json!({"ids": [c_id.0]})).unwrap();
    sync(&mut dev);
    write_png(&photos.join("2024/Trip/b.png"), 9);
    write_png(&photos.join("2026/d.png"), 5);
    // a copy of a file that is in the library: a photo of its own
    std::fs::copy(photos.join("2024/Trip/b.png"), photos.join("2026/b copy.png")).unwrap();
    server.scan("ann");
    let st = scanned(&server, 1);
    assert_eq!((st.added, st.moved, st.changed), (2, 1, 1), "{st:?}");
    sync(&mut dev);
    let a = by_place(&dev, "Photos/2026/a.png").expect("moved");
    assert_eq!((a.id, a.develop.light.exposure), (a_id, 0.5));
    let b = dev.catalog.photo(b_id).unwrap();
    assert_ne!(b.content_hash.as_deref(), None);
    assert_eq!(b.rating, 4, "edits stay when the content changes");
    assert!(by_place(&dev, "Photos/2025/c.png").is_none(), "removed photos aren't brought back");
    assert!(by_place(&dev, "Photos/2026/d.png").is_some());
    let copy = by_place(&dev, "Photos/2026/b copy.png").expect("the copy is a photo");
    assert_eq!(copy.content_hash, dev.catalog.photo(b_id).unwrap().content_hash);
    assert_eq!(dev.catalog.len(), 4);

    // the disk goes away: nothing is taken from the library
    std::fs::rename(&photos, root.join("nas/unmounted")).unwrap();
    server.scan("ann");
    let st = scanned(&server, 2);
    assert_eq!(st.errors.len(), 1, "{st:?}");
    std::fs::create_dir_all(&photos).unwrap();
    server.scan("ann");
    let st = scanned(&server, 3);
    assert!(st.errors.iter().any(|e| e.contains("empty")), "{st:?}");
    sync(&mut dev);
    assert_eq!(dev.catalog.len(), 4);
    let album = dev.catalog.albums().find(|al| al.name == "Best").unwrap();
    assert!(album.photos.contains(&a_id) && album.photos.contains(&b_id), "{album:?}");
    drop(server);
    let _ = std::fs::remove_dir_all(&root);
}

/// A photo the scan finds is "added" the day it was taken (the day of the scan only when undated).
#[test]
fn scanned_photos_are_added_the_day_they_were_taken() {
    let root = temp("taken");
    let photos = root.join("photos");
    write_png(&photos.join("dated.png"), 1);
    std::fs::write(
        photos.join("dated.xmp"),
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:exif="http://ns.adobe.com/exif/1.0/" exif:DateTimeOriginal="2020-02-03T04:05:06"/></rdf:RDF></x:xmpmeta>"#,
    )
    .unwrap();
    write_png(&photos.join("undated.png"), 2);
    let data = root.join("data");
    accounts::set_user(&data, "ann", "correct horse", false).unwrap();
    let mut cfg = Config::new(&data, "127.0.0.1:0");
    cfg.scan_interval = None;
    cfg.preview_threads = Some(1);
    let server = Server::start(cfg).unwrap();
    accounts::add_folder(&data, "ann", &photos.to_string_lossy(), None).unwrap();
    server.scan("ann");
    scanned(&server, 0);
    let mut dev = Session::new().with_fs();
    dev.open_library(root.join("device"), false).unwrap();
    let url = format!("http://{}", server.addr());
    dev.execute("sync.signIn", &json!({"server": url, "user": "ann", "password": "correct horse", "device": "test"})).unwrap();
    sync(&mut dev);
    let dated = by_place(&dev, "photos/dated.png").expect("dated");
    assert_eq!((dated.captured.as_deref(), dated.imported.as_str()), (Some("2020-02-03T04:05:06"), "2020-02-03T04:05:06"));
    let undated = by_place(&dev, "photos/undated.png").expect("undated");
    assert_eq!(undated.captured, None);
    assert!(undated.imported.as_str() > "2026", "an undated photo is added when the scan found it: {}", undated.imported);
    drop(server);
    let _ = std::fs::remove_dir_all(&root);
}

/// A device's upload of a file that is in a library folder: the folder's copy is used, no second
/// photo, no copy of the original on the server.
#[test]
fn uploads_of_folder_files_are_not_duplicated() {
    let root = temp("dup");
    let photos = root.join("photos");
    write_png(&photos.join("a.png"), 1);
    let data = root.join("data");
    accounts::set_user(&data, "ann", "correct horse", false).unwrap();
    let mut cfg = Config::new(&data, "127.0.0.1:0");
    cfg.scan_interval = None;
    cfg.preview_threads = Some(1);
    let server = Server::start(cfg).unwrap();
    let url = format!("http://{}", server.addr());
    // a device with the same photo (and another) uploads its library first
    std::fs::create_dir_all(root.join("in")).unwrap();
    std::fs::copy(photos.join("a.png"), root.join("in/a.png")).unwrap();
    write_png(&root.join("in/z.png"), 7);
    let mut dev = Session::new().with_fs();
    dev.open_library(root.join("device"), false).unwrap();
    dev.execute("library.import", &json!({"paths": [root.join("in").to_string_lossy()]})).unwrap();
    dev.execute("sync.signIn", &json!({"server": url, "user": "ann", "password": "correct horse", "device": "test"})).unwrap();
    sync(&mut dev);
    accounts::add_folder(&data, "ann", &photos.to_string_lossy(), None).unwrap();
    server.scan("ann");
    let st = scanned(&server, 0);
    assert_eq!((st.added, st.linked), (0, 1), "{st:?}");
    sync(&mut dev);
    assert_eq!(dev.catalog.len(), 2);
    let a = by_place(&dev, "photos/a.png").expect("the uploaded photo has its place in the folder");
    assert!(!lightcraft_engine::cloud_sync::is_remote(a), "the device keeps its own file");

    drop(server);
    let _ = std::fs::remove_dir_all(&root);
}

/// A file the server couldn't read is read again when it changes, or once after the server
/// restarts (a newer server may read it), not on every scan.
#[test]
fn files_that_failed_are_read_again_after_a_restart() {
    let root = temp("retry");
    let photos = root.join("photos");
    write_png(&photos.join("a.png"), 1);
    std::fs::write(photos.join("b.jpg"), b"not a photo").unwrap();
    let data = root.join("data");
    accounts::set_user(&data, "ann", "correct horse", false).unwrap();
    accounts::add_folder(&data, "ann", &photos.to_string_lossy(), None).unwrap();
    let start = || {
        let mut cfg = Config::new(&data, "127.0.0.1:0");
        cfg.scan_interval = None;
        cfg.preview_threads = Some(1);
        Server::start(cfg).unwrap()
    };
    let server = start();
    let st = scanned(&server, 0);
    assert_eq!((st.added, st.failed), (1, 1), "{st:?}");
    server.scan("ann");
    let st = scanned(&server, 1);
    assert_eq!((st.todo, st.failed), (0, 0), "not read again: {st:?}");
    drop(server);

    let server = start();
    let st = scanned(&server, 0);
    assert_eq!((st.todo, st.failed), (1, 1), "read again after a restart: {st:?}");
    drop(server);
    let _ = std::fs::remove_dir_all(&root);
}

/// Names a user asks to leave out (`*.fcpbundle`) are skipped with everything inside them; the
/// photos already read from them stay in the library, their files showing as missing.
#[test]
fn ignored_names_are_left_out_of_the_scan() {
    let root = temp("ignore");
    let photos = root.join("nas/photos");
    write_png(&photos.join("Gigs/Joe Bonamassa/Joe Bonamassa.fcpbundle/2-11-2015/Original Media/Joe Bonamassa-9.png"), 1);
    write_png(&photos.join("Gigs/Joe Bonamassa/stills/a.png"), 2);
    let data = root.join("data");
    accounts::set_user(&data, "ann", "correct horse", false).unwrap();
    accounts::add_folder(&data, "ann", &photos.to_string_lossy(), Some("Photos")).unwrap();
    let mut cfg = Config::new(&data, "127.0.0.1:0");
    cfg.scan_interval = None;
    cfg.preview_threads = Some(1);
    let server = Server::start(cfg).unwrap();
    let st = scanned(&server, 0);
    assert_eq!((st.files, st.added, st.missing), (2, 2, 0), "{st:?}");

    // a name is not a path, and the user has to exist
    assert!(accounts::set_ignore(&data, "ann", &["Gigs/Joe Bonamassa".into()]).is_err());
    assert!(accounts::set_ignore(&data, "bob", &["*.fcpbundle".into()]).is_err());
    let kept = accounts::set_ignore(&data, "ann", &[" *.fcpbundle ".into(), "".into(), "*.FCPBUNDLE".into()]).unwrap();
    assert_eq!(kept, ["*.fcpbundle"]);
    // changing the password doesn't lose it
    accounts::set_user(&data, "ann", "another horse", true).unwrap();
    assert_eq!(accounts::read_users(&data).unwrap().users["ann"].ignore, ["*.fcpbundle"]);

    server.scan("ann");
    let st = scanned(&server, 1);
    assert_eq!((st.files, st.added, st.missing), (1, 0, 1), "the bundle's photo is no longer found: {st:?}");

    accounts::set_ignore(&data, "ann", &[]).unwrap();
    server.scan("ann");
    let st = scanned(&server, 2);
    assert_eq!((st.files, st.added, st.missing), (2, 0, 0), "found again where it was: {st:?}");
    drop(server);
    let _ = std::fs::remove_dir_all(&root);
}

/// A Lightroom-style sidecar: a rating and an exposure.
fn lr_xmp(rating: u8, exposure: f64) -> String {
    format!(
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" xmp:Rating="{rating}" crs:Exposure2012="{exposure:+.2}"/></rdf:RDF></x:xmpmeta>"#
    )
}

/// Write a sidecar as Lightroom would, stamped `secs` after 1970.
fn write_sidecar(path: &Path, xmp: &str, secs: u64) {
    std::fs::write(path, xmp).unwrap();
    let f = std::fs::File::options().write(true).open(path).unwrap();
    f.set_modified(std::time::UNIX_EPOCH + Duration::from_secs(secs)).unwrap();
}

#[test]
fn sidecars_changed_by_lightroom_are_read_again() {
    let root = temp("xmp");
    let photos = root.join("photos");
    write_png(&photos.join("a.png"), 1);
    write_png(&photos.join("b.png"), 2);
    write_sidecar(&photos.join("a.xmp"), &lr_xmp(2, 0.5), 1_000);
    write_sidecar(&photos.join("b.xmp"), &lr_xmp(3, 0.0), 1_000);
    let data = root.join("data");
    accounts::set_user(&data, "ann", "correct horse", false).unwrap();
    accounts::add_folder(&data, "ann", &photos.to_string_lossy(), None).unwrap();
    let start = || {
        let mut cfg = Config::new(&data, "127.0.0.1:0");
        cfg.scan_interval = None;
        cfg.preview_threads = Some(1);
        Server::start(cfg).unwrap()
    };
    let device = |server: &Server, name: &str| {
        let mut dev = Session::new().with_fs();
        dev.open_library(root.join(name), false).unwrap();
        let url = format!("http://{}", server.addr());
        dev.execute("sync.signIn", &json!({"server": url, "user": "ann", "password": "correct horse", "device": name})).unwrap();
        sync(&mut dev);
        dev
    };
    let server = start();
    let st = scanned(&server, 0);
    assert_eq!((st.added, st.sidecars), (2, 0), "{st:?}");
    let mut dev = device(&server, "device");
    let look = |dev: &Session, place: &str| {
        let p = by_place(dev, place).unwrap();
        (p.rating, p.develop.light.exposure)
    };
    assert_eq!(look(&dev, "photos/a.png"), (2, 0.5));

    // b rated on the device: its unchanged sidecar isn't read again
    dev.selection = lightcraft_engine::Selection::single(by_place(&dev, "photos/b.png").unwrap().id);
    dev.execute("photo.rate", &json!({"rating": 5})).unwrap();
    sync(&mut dev);
    // Lightroom edits a: the next scan reads its sidecar again, without reading the photo
    write_sidecar(&photos.join("a.xmp"), &lr_xmp(4, 1.25), 2_000);
    server.scan("ann");
    let st = scanned(&server, 1);
    assert_eq!((st.sidecars, st.todo, st.changed), (1, 0, 0), "{st:?}");
    sync(&mut dev);
    assert_eq!(look(&dev, "photos/a.png"), (4, 1.25));
    assert_eq!(look(&dev, "photos/b.png").0, 5);

    // an index from before sidecar times were kept: its first scan only notes them, so an edit
    // made on a device isn't replaced by a sidecar that may be older; the next change is read
    drop(server);
    let index = data.join("users/ann/folders.json");
    let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(&index).unwrap()).unwrap();
    v.as_object_mut().unwrap().remove("sidecars");
    for e in v["files"].as_object_mut().unwrap().values_mut() {
        e.as_object_mut().unwrap().remove("sidecar");
    }
    std::fs::write(&index, serde_json::to_vec(&v).unwrap()).unwrap();
    write_sidecar(&photos.join("b.xmp"), &lr_xmp(1, 0.0), 3_000);
    let server = start();
    let st = scanned(&server, 0);
    assert_eq!(st.sidecars, 0, "{st:?}");
    let mut dev = device(&server, "device2");
    assert_eq!(look(&dev, "photos/b.png").0, 5);
    write_sidecar(&photos.join("b.xmp"), &lr_xmp(1, 0.0), 4_000);
    server.scan("ann");
    assert_eq!(scanned(&server, 1).sidecars, 1);
    sync(&mut dev);
    assert_eq!(look(&dev, "photos/b.png").0, 1);

    drop(server);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_scan_of_library_folders_is_reported_to_devices() {
    let root = temp("activity");
    let photos = root.join("nas/photos");
    write_png(&photos.join("a.png"), 1);
    write_png(&photos.join("2025/b.png"), 2);
    // a file that can't be read as a photo is a problem the device is told about
    std::fs::write(photos.join("broken.jpg"), b"not a jpeg").unwrap();
    let data = root.join("data");
    accounts::set_user(&data, "ann", "correct horse", false).unwrap();
    accounts::add_folder(&data, "ann", &photos.to_string_lossy(), Some("Photos")).unwrap();
    let mut cfg = Config::new(&data, "127.0.0.1:0");
    cfg.scan_interval = None;
    cfg.preview_threads = Some(1);
    let server = Server::start(cfg).unwrap();
    let url = format!("http://{}", server.addr());
    let st = scanned(&server, 0);
    assert_eq!(st.files, 3, "{st:?}");

    let mut dev = Session::new().with_fs();
    dev.open_library(root.join("device"), false).unwrap();
    dev.execute("sync.signIn", &json!({"server": url, "user": "ann", "password": "correct horse", "device": "test"})).unwrap();
    sync(&mut dev);
    let r = dev.execute("sync.activity", &json!({"refresh": true})).unwrap();
    let scan = &r["server"]["scan"];
    assert_eq!((&scan["scanning"], &scan["files"]), (&json!(false), &json!(3)), "{r}");
    assert!(scan["lastScan"].as_u64().unwrap_or(0) > 0, "when it ended: {r}");
    assert!(scan["problems"].as_u64().unwrap_or(0) >= 1, "the unreadable file: {r}");
    assert_eq!(r["server"]["previews"]["total"], 0, "all previews are built: {r}");
    drop(server);
    let _ = std::fs::remove_dir_all(&root);
}

/// The admin page's storage per user is what their photos take on the server: the files kept for them
/// (the previews of their library folders' photos) and the photos in those folders, which are only read.
/// It used to be the first alone, so a library of terabytes read in place showed as a few gigabytes.
#[test]
fn the_admin_page_counts_the_photos_in_library_folders_in_a_users_storage() {
    let root = temp("admin-storage");
    let photos = root.join("nas/photos");
    write_png(&photos.join("2024/a.png"), 1);
    write_png(&photos.join("2024/b.png"), 2);
    write_png(&photos.join("c.png"), 3);
    let data = root.join("data");
    accounts::set_user(&data, "ann", "correct horse", false).unwrap();
    accounts::set_admin(&data, "ann", true).unwrap();
    accounts::set_user(&data, "bob", "another horse", false).unwrap();
    accounts::add_folder(&data, "ann", &photos.to_string_lossy(), Some("Photos")).unwrap();
    let start = || {
        let mut cfg = Config::new(&data, "127.0.0.1:0");
        cfg.scan_interval = None;
        cfg.preview_threads = Some(1);
        Server::start(cfg).unwrap()
    };
    // (the files kept for a user are counted when the server starts and then every minute: let it
    // build the previews first and count them on the next start)
    let server = start();
    let st = scanned(&server, 0);
    assert_eq!((st.files, st.added), (3, 3), "{st:?}");
    drop(server);
    let server = start();
    let url = format!("http://{}", server.addr());
    scanned(&server, 0);

    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
    let json_of = |mut r: ureq::http::Response<ureq::Body>| -> Value { serde_json::from_slice(&r.body_mut().read_to_vec().unwrap()).unwrap() };
    let login =
        agent.post(format!("{url}/api/admin/login")).send(json!({"user": "ann", "password": "correct horse"}).to_string().as_bytes()).unwrap();
    let token = json_of(login)["token"].as_str().unwrap().to_string();
    let users =
        || -> Value { json_of(agent.get(format!("{url}/api/admin/users")).header("Authorization", format!("Bearer {token}")).call().unwrap()) };
    // (null until counted)
    let mut seen = users();
    for _ in 0..400 {
        if !seen[0]["bytes"].is_null() && !seen[1]["bytes"].is_null() {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
        seen = users();
    }
    let size = |f: &str| std::fs::metadata(photos.join(f)).unwrap().len();
    let in_folders = size("2024/a.png") + size("2024/b.png") + size("c.png");
    let kept: u64 = ["smart", "mini"].iter().map(|k| bytes_in(&data.join("users/ann/blobs").join(k))).sum();
    assert!(kept > 0, "the server kept previews of the three photos");
    let (ann, bob) = (&seen[0], &seen[1]);
    assert_eq!((ann["name"].as_str(), bob["name"].as_str()), (Some("ann"), Some("bob")), "{seen}");
    assert_eq!(ann["folderBytes"].as_u64(), Some(in_folders), "{seen}");
    assert_eq!(ann["bytes"].as_u64(), Some(kept + in_folders), "previews and photos: {seen}");
    // a user without library folders: only what the server keeps for them (nothing here)
    assert_eq!((bob["bytes"].as_u64(), bob["folderBytes"].as_u64()), (Some(0), Some(0)), "{seen}");
    // a photo that goes missing from its folder stops counting at once (the previews stay until a count)
    std::fs::remove_file(photos.join("c.png")).unwrap();
    server.scan("ann");
    scanned(&server, 1);
    assert_eq!(users()[0]["folderBytes"].as_u64(), Some(size("2024/a.png") + size("2024/b.png")), "two photos are left");
    drop(server);
    let _ = std::fs::remove_dir_all(&root);
}

/// Bytes of the files under `dir`.
fn bytes_in(dir: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    rd.flatten().map(|e| if e.path().is_dir() { bytes_in(&e.path()) } else { e.metadata().map_or(0, |m| m.len()) }).sum()
}

/// A device that shows a photo whose preview isn't built yet is told the server is making it, and the server makes it
/// before the rest of its queue: the answer to the next ask is the preview.
#[test]
fn a_preview_a_device_asks_for_is_built_at_once() {
    let root = temp("rush");
    let photos = root.join("nas/photos");
    for (i, name) in ["a.png", "b.png", "c.png"].iter().enumerate() {
        write_png(&photos.join(name), i as u8 + 1);
    }
    let data = root.join("data");
    accounts::set_user(&data, "ann", "correct horse", false).unwrap();
    accounts::add_folder(&data, "ann", &photos.to_string_lossy(), Some("Photos")).unwrap();
    let mut cfg = Config::new(&data, "127.0.0.1:0");
    cfg.scan_interval = None;
    cfg.preview_threads = Some(1);
    let server = Server::start(cfg).unwrap();
    let url = format!("http://{}", server.addr());
    scanned(&server, 0);
    let mut dev = Session::new().with_fs();
    dev.open_library(root.join("device"), false).unwrap();
    dev.execute("sync.signIn", &json!({"server": url, "user": "ann", "password": "correct horse", "device": "test"})).unwrap();
    sync(&mut dev);
    let hash = by_place(&dev, "Photos/b.png").unwrap().content_hash.clone().unwrap();
    let token = serde_json::from_slice::<serde_json::Value>(&std::fs::read(root.join("device/sync.json")).unwrap()).unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();
    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
    let call = |method: &str, kind: &str, hash: &str| {
        let req = agent.run(
            ureq::http::Request::builder()
                .method(method)
                .uri(format!("{url}/api/blobs/{kind}/{hash}"))
                .header("Authorization", format!("Bearer {token}"))
                .body(())
                .unwrap(),
        );
        let mut r = req.unwrap();
        let retry = r.headers().get("Retry-After").and_then(|v| v.to_str().ok()).map(str::to_string);
        (r.status().as_u16(), r.body_mut().read_to_vec().unwrap(), retry)
    };
    let blobs = data.join("users/ann/blobs");
    let files = |kind: &str| blobs.join(kind).join(&hash[..2]).join(&hash);
    let said = |body: &[u8]| serde_json::from_slice::<serde_json::Value>(body).ok().and_then(|v| v["previews"].as_str().map(str::to_string));

    // the previews are gone (cleaned away, or never built): asking for one is what builds them
    for kind in ["smart", "mini"] {
        std::fs::remove_file(files(kind)).unwrap();
    }
    let (status, body, _) = call("HEAD", "mini", &hash);
    assert_eq!((status, said(&body)), (404, None), "a HEAD only looks");
    std::thread::sleep(Duration::from_millis(300));
    assert!(!files("mini").exists(), "…and builds nothing");
    let (status, body, retry_after) = call("GET", "mini", &hash);
    assert_eq!((status, said(&body).as_deref(), retry_after.as_deref()), (404, Some("queued"), Some("3")), "{}", String::from_utf8_lossy(&body));
    let mut served = None;
    for _ in 0..200 {
        let (status, body, _) = call("GET", "mini", &hash);
        if status == 200 {
            served = Some(body);
            break;
        }
        assert_eq!(said(&body).as_deref(), Some("queued"), "still being made: {}", String::from_utf8_lossy(&body));
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(served.is_some_and(|b| !b.is_empty()), "the preview comes once it is built");
    assert!(lightcraft_engine::smart::is_valid(&files("smart")), "both previews are built together");

    // a hash the server has no photo for has nothing to wait for
    let (status, body, retry) = call("GET", "mini", &"ab".repeat(16));
    assert_eq!((status, said(&body), retry), (404, None, None));
    drop(server);
    let _ = std::fs::remove_dir_all(&root);
}

/// An admin's `POST /api/admin/<path>` (a bearer token from the login), answered as (status, JSON).
fn admin_post(url: &str, token: &str, path: &str) -> (u16, Value) {
    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
    let mut r = agent.post(format!("{url}/api/admin/{path}")).header("Authorization", format!("Bearer {token}")).send(&b""[..]).unwrap();
    let status = r.status().as_u16();
    (status, serde_json::from_slice(&r.body_mut().read_to_vec().unwrap()).unwrap_or(Value::Null))
}

fn admin_token(url: &str) -> String {
    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
    let mut r =
        agent.post(format!("{url}/api/admin/login")).send(json!({"user": "ann", "password": "correct horse"}).to_string().as_bytes()).unwrap();
    let v: Value = serde_json::from_slice(&r.body_mut().read_to_vec().unwrap()).unwrap();
    v["token"].as_str().unwrap().to_string()
}

/// Scan for changes reads the files that are new or changed (by size and time); Rescan everything reads every file
/// again: it finds a change a scan trusts the index too much to see, and tries again what could not be read.
#[test]
fn rescan_reads_every_file_again_and_a_scan_reads_only_what_changed() {
    let root = temp("rescan");
    let photos = root.join("nas/photos");
    write_png(&photos.join("a.png"), 1);
    write_png(&photos.join("b.png"), 2);
    std::fs::write(photos.join("broken.jpg"), b"not a photo").unwrap();
    let data = root.join("data");
    accounts::set_user(&data, "ann", "correct horse", false).unwrap();
    accounts::set_admin(&data, "ann", true).unwrap();
    accounts::add_folder(&data, "ann", &photos.to_string_lossy(), Some("Photos")).unwrap();
    let mut cfg = Config::new(&data, "127.0.0.1:0");
    cfg.scan_interval = None;
    cfg.preview_threads = Some(1);
    let server = Server::start(cfg).unwrap();
    let url = format!("http://{}", server.addr());
    let token = admin_token(&url);
    let st = scanned(&server, 0);
    assert_eq!((st.files, st.added, st.failed, st.full), (3, 2, 1, false), "{st:?}");

    // a file gets other content of the same size, and its time is put back: nothing a scan looks at changed
    let a = photos.join("a.png");
    let modified = std::fs::metadata(&a).unwrap().modified().unwrap();
    let (old, new) = (png_bytes(1), (2..=255u8).map(png_bytes).find(|b| b.len() == png_bytes(1).len()).expect("a picture of the same size"));
    assert_ne!(old, new);
    std::fs::write(&a, &new).unwrap();
    std::fs::File::options().write(true).open(&a).unwrap().set_modified(modified).unwrap();

    // Scan for changes: the index is trusted. Nothing is read, the failed file is not tried again
    let (status, _) = admin_post(&url, &token, "users/ann/scan");
    assert_eq!(status, 200);
    let st = scanned(&server, 1);
    assert_eq!((st.todo, st.changed, st.failed, st.full), (0, 0, 0, false), "{st:?}");

    // Rescan everything: every file is read, the change is found, the failed file is tried again (and fails again)
    let (status, answer) = admin_post(&url, &token, "users/ann/rescan");
    assert_eq!(status, 200);
    assert!(answer["scan"].is_object(), "answers with the status like a scan does: {answer}");
    let st = scanned(&server, 2);
    assert_eq!((st.files, st.todo, st.full), (3, 3, true), "{st:?}");
    assert_eq!((st.changed, st.added, st.moved, st.failed), (1, 0, 0, 1), "{st:?}");

    // and the next ordinary scan is an ordinary one again
    let (status, _) = admin_post(&url, &token, "users/ann/scan");
    assert_eq!(status, 200);
    let st = scanned(&server, 3);
    assert_eq!((st.todo, st.full), (0, false), "{st:?}");
    // a user who isn't an admin can't ask (a device token is no admin session)
    assert_eq!(admin_post(&url, "nonsense", "users/ann/rescan").0, 401);
    drop(server);
    let _ = std::fs::remove_dir_all(&root);
}

/// The smart and mini previews on disk: path and time of each.
fn previews_on_disk(data: &Path) -> std::collections::BTreeMap<PathBuf, std::time::SystemTime> {
    fn walk(dir: &Path, out: &mut std::collections::BTreeMap<PathBuf, std::time::SystemTime>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            if e.path().is_dir() {
                walk(&e.path(), out);
            } else if let Ok(t) = e.metadata().and_then(|m| m.modified()) {
                out.insert(e.path(), t);
            }
        }
    }
    let mut out = std::collections::BTreeMap::new();
    for kind in ["smart", "mini"] {
        walk(&data.join("users/ann/blobs").join(kind), &mut out);
    }
    out
}

/// Build missing previews leaves the ones there alone; Rebuild all previews builds every one again, each replacing the
/// old only once the new is written, so a photo that can't be read now keeps the preview it has.
#[test]
fn rebuilding_previews_replaces_the_ones_there_and_generating_does_not() {
    let root = temp("rebuild");
    let photos = root.join("nas/photos");
    write_png(&photos.join("a.png"), 1);
    write_png(&photos.join("b.png"), 2);
    write_png(&photos.join("c.png"), 3);
    let data = root.join("data");
    accounts::set_user(&data, "ann", "correct horse", false).unwrap();
    accounts::set_admin(&data, "ann", true).unwrap();
    accounts::add_folder(&data, "ann", &photos.to_string_lossy(), Some("Photos")).unwrap();
    let mut cfg = Config::new(&data, "127.0.0.1:0");
    cfg.scan_interval = None;
    cfg.preview_threads = Some(1);
    let server = Server::start(cfg).unwrap();
    let url = format!("http://{}", server.addr());
    let token = admin_token(&url);
    scanned(&server, 0);
    let first = previews_on_disk(&data);
    assert_eq!(first.len(), 6, "a smart and a mini preview of each photo: {first:?}");
    // (file times have a resolution: let the next ones be later)
    std::thread::sleep(Duration::from_millis(1100));

    // Build missing previews: all are there, so nothing is built (waited for: it is the scanner that looks)
    assert_eq!(admin_post(&url, &token, "users/ann/previews/generate").0, 200);
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(previews_on_disk(&data), first, "nothing was built again");
    assert!(!server.folder_status("ann").previews_rebuilding);

    // The Processing page does the same for everyone at once, and reports every user's previews together
    let (status, all) = admin_post(&url, &token, "previews/generate");
    assert_eq!(status, 200);
    assert_eq!(all["previews"]["paused"], false);
    assert_eq!(all["previews"]["users"][0]["name"], "ann");
    assert_eq!(admin_post(&url, &token, "previews/pause").1["previews"]["paused"], true);
    assert_eq!(admin_post(&url, &token, "previews/resume").1["previews"]["paused"], false);
    assert_eq!(admin_post(&url, &token, "previews/nonsense").0, 404);

    // c.png can't be read now (the index still thinks it can: no scan ran): its preview must stay
    std::fs::write(photos.join("c.png"), b"gone bad").unwrap();
    let (status, _) = admin_post(&url, &token, "users/ann/previews/rebuild");
    assert_eq!(status, 200);
    let mut st = server.folder_status("ann");
    for _ in 0..600 {
        if st.preview_errors.len() == 1 && st.previews == 0 && st.previews_last_done == Some(3) {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
        st = server.folder_status("ann");
    }
    assert_eq!((st.previews, st.previews_last_done, st.preview_errors.len()), (0, Some(3), 1), "{st:?}");
    assert!(!st.previews_rebuilding, "{st:?}");
    let after = previews_on_disk(&data);
    assert_eq!(after.len(), 6, "{after:?}");
    let newer = after.iter().filter(|(p, t)| first.get(*p).is_some_and(|before| *t > before)).count();
    assert_eq!(newer, 4, "the previews of a and b were replaced, c's stayed: {after:?}");
    drop(server);
    let _ = std::fs::remove_dir_all(&root);
}

/// Check files (admin page → Folders): a photo file cut short (a copy that stopped part-way) is listed with its
/// path, a whole one isn't, and the report stays for the page.
#[test]
fn check_files_finds_a_photo_file_cut_short() {
    let root = temp("check");
    let photos = root.join("nas/photos");
    write_png(&photos.join("whole.png"), 1);
    let cut = png_bytes(2);
    std::fs::create_dir_all(&photos).unwrap();
    std::fs::write(photos.join("cut.png"), &cut[..cut.len() / 2]).unwrap();
    let data = root.join("data");
    accounts::set_user(&data, "ann", "correct horse", false).unwrap();
    accounts::set_admin(&data, "ann", true).unwrap();
    accounts::add_folder(&data, "ann", &photos.to_string_lossy(), Some("Photos")).unwrap();
    let mut cfg = Config::new(&data, "127.0.0.1:0");
    cfg.scan_interval = None;
    cfg.preview_threads = Some(1);
    let server = Server::start(cfg).unwrap();
    let url = format!("http://{}", server.addr());
    let token = admin_token(&url);
    scanned(&server, 0);
    let (status, started) = admin_post(&url, &token, "users/ann/check");
    assert_eq!(status, 200, "{started}");
    let get = || -> Value {
        let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
        let mut r = agent.get(format!("{url}/api/admin/users/ann/check")).header("Authorization", format!("Bearer {token}")).call().unwrap();
        serde_json::from_slice(&r.body_mut().read_to_vec().unwrap()).unwrap()
    };
    let mut st = get();
    for _ in 0..200 {
        if st["running"] == false && !st["report"].is_null() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
        st = get();
    }
    let report = &st["report"];
    assert_eq!(report["files"], 2, "{st}");
    let cut = report["cut"].as_array().unwrap();
    assert_eq!(cut.len(), 1, "{st}");
    assert_eq!(cut[0]["path"], "Photos/cut.png");
    // (too damaged to import: the scan made no photo of it, and the check still lists it)
    assert_eq!(cut[0]["photos"], json!([]));
    assert_eq!(admin_post(&url, &token, "users/nobody/check").0, 404);
    drop(server);
    let _ = std::fs::remove_dir_all(&root);
}

/// Clips in a library folder are photos of kind Video with their length, shown by a poster frame the server builds
/// (it never reads a clip whole), and a clip nobody can decode is still catalogued instead of being dropped.
#[test]
fn video_clips_in_a_library_folder_are_listed_with_posters() {
    let root = temp("video");
    let photos = root.join("nas/phone");
    std::fs::create_dir_all(&photos).unwrap();
    let clip = lightcraft_video::testdata::clip(20, 10, 2);
    std::fs::write(photos.join("IMG_0001.MP4"), &clip).unwrap();
    std::fs::write(photos.join("IMG_0002.MOV"), &clip).unwrap();
    write_png(&photos.join("pic.png"), 1);
    // a damaged clip: reported as a failure, the scan goes on
    std::fs::write(photos.join("cut.mp4"), &clip[..clip.len() / 3]).unwrap();
    let data = root.join("data");
    accounts::set_user(&data, "ann", "correct horse", false).unwrap();
    accounts::set_admin(&data, "ann", true).unwrap();
    accounts::add_folder(&data, "ann", &photos.to_string_lossy(), Some("Phone")).unwrap();
    let mut cfg = Config::new(&data, "127.0.0.1:0");
    cfg.scan_interval = None;
    cfg.preview_threads = Some(1);
    let server = Server::start(cfg).unwrap();
    let url = format!("http://{}", server.addr());
    let st = scanned(&server, 0);
    assert_eq!((st.files, st.added, st.failed), (4, 3, 1), "{st:?}");

    let mut dev = Session::new().with_fs();
    dev.open_library(root.join("device"), false).unwrap();
    dev.execute("sync.signIn", &json!({"server": url, "user": "ann", "password": "correct horse", "device": "test"})).unwrap();
    sync(&mut dev);
    assert_eq!(dev.catalog.len(), 3);
    let mp4 = by_place(&dev, "Phone/IMG_0001.MP4").expect("the clip is a photo");
    assert_eq!(mp4.kind, lightcraft_engine::catalog::MediaKind::Video);
    assert_eq!((mp4.width, mp4.height, mp4.format.as_str()), (128, 96, "MP4"));
    assert!(mp4.duration.is_some_and(|d| (d - 2.0).abs() < 0.01), "{:?}", mp4.duration);
    let mov = by_place(&dev, "Phone/IMG_0002.MOV").expect("the .mov too");
    assert_eq!(mov.kind, lightcraft_engine::catalog::MediaKind::Video);
    assert_eq!(by_place(&dev, "Phone/pic.png").unwrap().kind, lightcraft_engine::catalog::MediaKind::Image);
    let (id, hash) = (mp4.id, mp4.content_hash.clone().unwrap());

    // the library counted by kind: the clips are not among the photos, their own files add up, and the server's lighter copy
    // of one (made up here: 1000 bytes under blobs/video) is among what it keeps and shows what it saved
    let copy_dir = data.join("users/ann/blobs/video").join(&hash[..2]);
    std::fs::create_dir_all(&copy_dir).unwrap();
    std::fs::write(copy_dir.join(&hash), vec![0u8; 1000]).unwrap();
    let u = dev.execute("sync.usage", &json!({"refresh": true})).unwrap();
    let server_says = &u["server"];
    assert_eq!((server_says["photos"].as_u64(), server_says["videos"].as_u64()), (Some(1), Some(2)), "{u}");
    assert_eq!(server_says["video"]["originals"], json!({"files": 2, "bytes": 2 * clip.len()}), "{u}");
    assert_eq!(server_says["video"]["encoded"], json!({"files": 1, "bytes": 1000}), "{u}");
    // (both clips are the same bytes: one copy stands for them, and saves what it is smaller than the clip once)
    assert_eq!(server_says["video"]["saved"].as_u64(), Some(clip.len() as u64 - 1000), "{u}");
    let usage: Usage = serde_json::from_value(server_says.clone()).unwrap();
    assert!(usage.stored() >= 1000, "the lighter copy is among the files the server keeps: {u}");
    // the admin page's Users list says the same
    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
    let mut r = agent.get(format!("{url}/api/admin/users")).header("Authorization", format!("Bearer {}", admin_token(&url))).call().unwrap();
    let users: Value = serde_json::from_slice(&r.body_mut().read_to_vec().unwrap()).unwrap();
    let ann = users.as_array().unwrap().iter().find(|x| x["name"] == "ann").unwrap();
    assert_eq!((ann["photos"].as_u64(), ann["videos"].as_u64()), (Some(1), Some(2)), "{users}");
    assert_eq!((ann["videoBytes"].as_u64(), ann["videoFiles"].as_u64()), (Some(2 * clip.len() as u64), Some(2)), "{users}");
    assert_eq!((ann["videoCopies"].as_u64(), ann["videoCopyBytes"].as_u64()), (Some(1), Some(1000)), "{users}");
    assert_eq!(ann["videoSaved"].as_u64(), Some(clip.len() as u64 - 1000), "{users}");

    // the server built both previews of the clips from their poster frames, and copied nothing
    let blobs = data.join("users/ann/blobs");
    assert_eq!((files_in(&blobs.join("original")), files_in(&blobs.join("smart")), files_in(&blobs.join("mini"))), (0, 2, 2));
    dev.thumb_job(id, 256);
    sync(&mut dev);
    assert!(dev.render_now(id, 64, 64).is_ok(), "renders from the poster the server built");
    // the clip is served from its folder, a range at a time
    let hash = dev.catalog.photo(id).unwrap().content_hash.clone().unwrap();
    let token =
        serde_json::from_slice::<Value>(&std::fs::read(root.join("device/sync.json")).unwrap()).unwrap()["token"].as_str().unwrap().to_string();
    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
    let mut r = agent
        .get(format!("{url}/api/blobs/original/{hash}"))
        .header("Authorization", format!("Bearer {token}"))
        .header("Range", "bytes=0-99")
        .call()
        .unwrap();
    assert_eq!(r.status().as_u16(), 206);
    assert_eq!(r.body_mut().read_to_vec().unwrap(), clip[..100].to_vec());
    drop(server);
    let _ = std::fs::remove_dir_all(&root);
}

/// A synthetic Olympus file (`lightcraft_engine::testdata`): a 48 x 36 sensor, shot at the aspect ratio `frame` states.
/// `compressed` is a strip of the compressed coding's size class (preview-only), else uncompressed 16-bit words (a raw);
/// `seed` makes the content differ (the library keeps one photo of a content).
fn olympus(compressed: bool, frame: Option<[u16; 4]>, seed: u32) -> Vec<u8> {
    use lightcraft_codecs::{ChromaSubsampling, EncodeImage, EncodeMeta, Samples, encode_jpeg};
    let px: Vec<u8> = (0..40 * 30).flat_map(|i| [(i % 251) as u8, 128, 200]).collect();
    let preview = encode_jpeg(&EncodeImage::new(40, 30, 3, Samples::U8(&px)), 90, ChromaSubsampling::S444, &EncodeMeta::default()).unwrap();
    let strip: Vec<u8> = if compressed {
        [&[0u8, 0, 0, 0, 1, 0, 0][..], &[seed as u8; 57]].concat()
    } else {
        (0..48u32 * 36).flat_map(|i| (((200 + i * 37 + seed * 5) % 4000) as u16).to_le_bytes()).collect()
    };
    lightcraft_engine::testdata::OlympusOrf { frame, ..lightcraft_engine::testdata::OlympusOrf::new(48, 36, 16, strip, preview) }.build()
}

/// `.ori` files are found in a library folder, and an Olympus body's aspect-ratio setting becomes the photo's import crop
/// there as it does on a desktop (the same `ProbeInfo::default_crop`, applied by `import::apply_default_crop`): the crop
/// is part of the photo's import look, not an edit, and reaches the devices.
#[test]
fn olympus_originals_in_a_library_folder_keep_the_cameras_aspect_crop() {
    let root = temp("olympus");
    let photos = root.join("photos");
    std::fs::create_dir_all(&photos).unwrap();
    // 16:9 of the 48 x 36 area: rows 4 to 30
    let wide = Some([0, 4, 47, 30]);
    let files = [
        ("wide.ori", olympus(false, wide, 1)),
        ("wide-compressed.ORI", olympus(true, wide, 2)),
        ("wide.orf", olympus(false, wide, 3)),
        ("plain.ori", olympus(false, None, 4)),
        ("plain.orf", olympus(true, None, 5)),
    ];
    for (name, bytes) in &files {
        std::fs::write(photos.join(name), bytes).unwrap();
    }
    // the desktop's own import of the same files, for comparison
    let mut desktop = Session::new().with_fs();
    desktop.execute("library.import", &json!({"paths": [photos.to_string_lossy()]})).unwrap();
    assert_eq!(desktop.catalog.len(), files.len());

    let data = root.join("data");
    accounts::set_user(&data, "ann", "correct horse", false).unwrap();
    let mut cfg = Config::new(&data, "127.0.0.1:0");
    cfg.scan_interval = None;
    cfg.preview_threads = Some(1);
    let server = Server::start(cfg).unwrap();
    accounts::add_folder(&data, "ann", &photos.to_string_lossy(), Some("Photos")).unwrap();
    server.scan("ann");
    let st = scanned(&server, 0);
    assert_eq!((st.files, st.added, st.failed), (files.len(), files.len(), 0), "{st:?}");
    let mut dev = Session::new().with_fs();
    dev.open_library(root.join("device"), false).unwrap();
    let url = format!("http://{}", server.addr());
    dev.execute("sync.signIn", &json!({"server": url, "user": "ann", "password": "correct horse", "device": "test"})).unwrap();
    sync(&mut dev);
    assert_eq!(dev.catalog.len(), files.len());
    let whole = |p: &lightcraft_engine::catalog::Photo| {
        let r = p.develop.crop.geometry.rect;
        (r.x0, r.y0, r.x1, r.y1) == (0.0, 0.0, 1.0, 1.0)
    };
    for (name, _) in &files {
        let on_server = by_place(&dev, &format!("Photos/{name}")).unwrap_or_else(|| panic!("{name} is not in the library"));
        let on_desktop = desktop.catalog.photos().find(|p| p.file_name == *name).unwrap();
        assert_eq!(on_server.format, on_desktop.format, "{name}");
        assert_eq!(on_server.preview_only.is_some(), on_desktop.preview_only.is_some(), "{name}");
        assert_eq!(on_server.develop.crop.geometry.rect, on_desktop.develop.crop.geometry.rect, "{name}: the desktop's crop");
        assert_eq!(on_server.import_look.is_some(), on_desktop.import_look.is_some(), "{name}");
        let framed = name.starts_with("wide");
        assert_eq!(!whole(on_server), framed, "{name}: {:?}", on_server.develop.crop.geometry.rect);
        assert!(!on_server.is_edited(), "{name}: an import look, not an edit");
    }
    let wide = by_place(&dev, "Photos/wide.ori").unwrap();
    let r = wide.develop.crop.geometry.rect;
    assert!(wide.develops_raw() && (wide.width, wide.height) == (48, 36), "the whole area is developed");
    assert!((r.x0, r.x1) == (0.0, 1.0) && (r.y0 - 4.0 / 36.0).abs() < 1e-9 && (r.y1 - 31.0 / 36.0).abs() < 1e-9, "{r:?}");
    drop(server);
    let _ = std::fs::remove_dir_all(&root);
}
