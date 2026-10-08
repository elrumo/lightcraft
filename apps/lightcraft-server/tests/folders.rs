//! Library folders end to end: photo folders on the server read in place, synced to a device
//! over real HTTP, and what scans do when files are added, moved, changed, removed or unmounted.

use std::path::{Path, PathBuf};
use std::time::Duration;

use lightcraft_engine::Session;
use lightcraft_engine::catalog::Source;
use lightcraft_server::{Config, Server, accounts, folders};
use serde_json::json;

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-server-folders-{tag}-{}", std::process::id()));
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
    cfg.preview_threads = 1;
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
    let b = by_place(&dev, "Photos/2024/Trip/b.png").expect("b in its folder");
    assert_eq!((b.rating, b.meta.keywords.clone()), (4, vec!["beach".to_string()]));
    assert!(matches!(&b.source, Source::File { path } if path.starts_with("web/")));
    let folders = lightcraft_engine::catalog::query::server_folders(&dev.catalog);
    assert_eq!(folders.get("Photos"), Some(&3));
    assert_eq!(folders.get("Photos/2024/Trip"), Some(&2));
    let (a_id, b_id) = (by_place(&dev, "Photos/2024/Trip/a.png").unwrap().id, b.id);
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
    cfg.preview_threads = 1;
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
    assert!(!lightcraft_engine::sync::is_remote(a), "the device keeps its own file");

    drop(server);
    let _ = std::fs::remove_dir_all(&root);
}
