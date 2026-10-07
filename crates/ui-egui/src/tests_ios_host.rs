//! Headless tests of what the iOS host hands the UI: photos its pickers copied into a temporary
//! folder (moved into the library by the import review) and exports offered to the share sheet
//! instead of written to a folder the user picks.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use lightcraft_engine::Session;
use serde_json::json;

use crate::headless::Headless;
use crate::{LightcraftApp, Services, ShareExports};

const T: Duration = Duration::from_secs(30);
const SETTLE: Duration = Duration::from_secs(120);

fn base(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-ui-ios-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write_png(path: &Path, seed: u8) {
    let img = lightcraft_raster::Rgba8 { width: 8, height: 8, data: vec![[seed, 3, 9, 255]; 64] };
    let png = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap();
    std::fs::write(path, png).unwrap();
}

fn has(h: &Headless, id: &str) -> bool {
    h.app.widgets.iter().any(|(w, _)| w == id)
}

fn library_session(dir: &Path) -> Session {
    let mut s = Session::new().with_fs();
    s.open_library(dir, false).unwrap();
    s
}

/// The pickers' copies are in the app's tmp folder, which iOS may empty: the review moves them
/// into the library's Originals/ and offers no "add in place" (nor a source path to show).
#[test]
fn staged_picks_are_moved_into_the_library() {
    let root = base("staged");
    let (lib, staging) = (root.join("Documents").join("LightCraft Library"), root.join("tmp").join("Import"));
    std::fs::create_dir_all(&staging).unwrap();
    let picked: Vec<String> = (0..3u8)
        .map(|i| {
            let p = staging.join(format!("IMG_{i:04}.png"));
            write_png(&p, i * 40);
            p.to_string_lossy().to_string()
        })
        .collect();
    let app = LightcraftApp::new(library_session(&lib), Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [390.0, 844.0], 1.0);
    h.settle(SETTLE);
    let r = h.request("engine.execute", json!({"command": "file.addPhotos", "params": {"paths": picked, "staged": true}}), T);
    assert_eq!(r["result"]["scanning"], true, "{r}");
    h.settle(SETTLE);
    let Some(crate::state::Dialog::Import { opts }) = &h.app.ui.dialog else { panic!("no import review") };
    assert!(opts.staged && opts.copy && opts.move_files, "{opts:?}");
    assert_eq!(opts.candidates.len(), 3);
    assert!(has(&h, "label:importModeHelp"));
    assert!(!has(&h, "button:importAdd"), "no add in place for temporary copies");
    assert!(!has(&h, "label:importSource"), "the host's temporary folder isn't shown");
    let r = h.request("ui.dialog.confirm", json!({}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert!(h.app.import.is_none(), "finished");
    let paths: Vec<String> = h
        .app
        .session
        .catalog
        .photos()
        .filter_map(|p| match &p.source {
            lightcraft_catalog::Source::File { path } => Some(path.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(paths.len(), 3, "{paths:?}");
    for p in &paths {
        assert!(Path::new(p).starts_with(lib.join("Originals")) && Path::new(p).is_file(), "{p}");
    }
    for p in &picked {
        assert!(!Path::new(p).exists(), "{p} was moved, not left in tmp");
    }
    drop(h);
    let _ = std::fs::remove_dir_all(&root);
}

/// Without a library on disk (the in-memory fallback) there is nowhere to move them: an error
/// the user can read, not an import that silently references tmp files.
#[test]
fn staged_picks_need_a_saved_library() {
    let root = base("staged-nolib");
    let p = root.join("IMG_0001.png");
    write_png(&p, 7);
    let mut app = LightcraftApp::new(Session::with_demo(), Services { png: None, ..Default::default() });
    let r = app.run("file.addPhotos", json!({"paths": [p.to_string_lossy()], "staged": true}));
    assert!(r.as_ref().is_err_and(|e| e.contains("library")), "{r:?}");
    assert!(app.scan.is_none());
    let _ = std::fs::remove_dir_all(&root);
}

/// iOS: exports are written to the host's staging folder (emptied first: the previous export was
/// shared or dismissed) and handed to the share sheet; the dialog has no folder to choose and the
/// staging folder isn't remembered as the user's export folder.
#[test]
fn exports_go_to_the_share_sheet() {
    let root = base("share");
    let staging = root.join("Exports");
    std::fs::create_dir_all(&staging).unwrap();
    std::fs::write(staging.join("left-over.jpg"), b"an earlier export").unwrap();
    let shared: Rc<RefCell<Vec<Vec<String>>>> = Rc::default();
    let sink = shared.clone();
    let services = Services {
        png: None,
        write: Some(Box::new(|path: &str, bytes: &[u8]| {
            if let Some(d) = Path::new(path).parent() {
                std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
            }
            std::fs::write(path, bytes).map_err(|e| e.to_string())
        })),
        share_exports: Some(ShareExports {
            dir: staging.to_string_lossy().to_string(),
            share: Box::new(move |files| sink.borrow_mut().push(files.to_vec())),
        }),
        ..Default::default()
    };
    let mut app = LightcraftApp::new(Session::with_demo(), services);
    let ids: Vec<u64> = app.session.catalog.photos().take(2).map(|p| p.id.0).collect();
    let r = app.run("app.export", json!({"ids": ids, "format": "jpeg", "longEdge": 64, "dir": "/somewhere/else", "subfolder": "sub"})).unwrap();
    let written: Vec<String> = r["files"].as_array().unwrap().iter().filter_map(|f| f["path"].as_str()).map(str::to_string).collect();
    assert_eq!(written.len(), 2, "{r}");
    assert_eq!(shared.borrow().as_slice(), std::slice::from_ref(&written), "one share sheet with every file");
    for p in &written {
        assert_eq!(Path::new(p).parent(), Some(staging.as_path()), "{p}: in the staging folder, no subfolder");
    }
    assert!(!staging.join("left-over.jpg").exists(), "the staging folder is emptied first");
    let last = app.session.last_export.clone().unwrap();
    assert!(last.get("dir").is_none(), "{last}");

    // the dialog: no folder field, a line about the share sheet
    let mut h = Headless::new(app, [390.0, 844.0], 1.0);
    h.settle(SETTLE);
    let r = h.request("engine.execute", json!({"command": "dialog.export", "params": {}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert!(has(&h, "label:exportShareHelp"));
    assert!(!has(&h, "button:exportChooseFolder"));
    drop(h);
    let _ = std::fs::remove_dir_all(&root);
}
