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
    // a phone: the photos in a grid, the options folded away under a disclosure
    assert!(has(&h, "import:2") && !has(&h, "label:importModeHelp"));
    let r = h.request("ui.clickWidget", json!({"id": "button:importOptions"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
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
    // a background export, and another one asked for meanwhile: refused before it could empty
    // the staging folder the first one is writing to
    let shared_before = shared.borrow().len();
    let all: Vec<u64> = h.app.session.catalog.photos().take(3).map(|p| p.id.0).collect();
    h.app.services.write_shared = Some(std::sync::Arc::new(|path: &str, bytes: &[u8]| {
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(path, bytes).map_err(|e| e.to_string())
    }));
    let r = h.app.run("app.export", json!({"ids": all, "format": "jpeg", "longEdge": 64, "background": true})).unwrap();
    assert_eq!(r["background"], true, "{r}");
    let again = h.app.run("app.export", json!({"ids": [all[0]], "format": "jpeg", "longEdge": 64}));
    assert!(again.as_ref().is_err_and(|e| e.contains("already running")), "{again:?}");
    h.settle(SETTLE);
    assert!(h.app.export.is_none(), "finished");
    assert_eq!(shared.borrow().len(), shared_before + 1, "the background export opened the share sheet");
    let last = shared.borrow().last().cloned().unwrap();
    assert_eq!(last.len(), 3);
    assert!(last.iter().all(|p| Path::new(p).is_file()), "nothing of it was removed: {last:?}");
    drop(h);
    let _ = std::fs::remove_dir_all(&root);
}

fn menu_ids(app: &LightcraftApp) -> Vec<String> {
    fn walk(nodes: &[crate::menubar::MenuNode], out: &mut Vec<String>) {
        for n in nodes {
            match n {
                crate::menubar::MenuNode::Item { id, .. } => out.push(id.clone()),
                crate::menubar::MenuNode::Submenu { label, children } => {
                    out.push(format!("@{label}"));
                    walk(children, out);
                }
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    for (_, items) in crate::menubar::menu_bar(app) {
        walk(&items, &mut out);
    }
    out
}

fn picking_app() -> (LightcraftApp, Rc<RefCell<Vec<crate::PickSource>>>) {
    let asked: Rc<RefCell<Vec<crate::PickSource>>> = Rc::default();
    let log = asked.clone();
    let services = Services {
        png: None,
        host_pick: Some(Box::new(move |s| {
            log.borrow_mut().push(s);
            Ok(())
        })),
        ..Default::default()
    };
    (LightcraftApp::new(Session::with_demo(), services), asked)
}

/// The host's pickers replace the desktop's open dialogs in File (and only there).
#[test]
fn host_pickers_are_in_the_file_menu_instead_of_the_open_dialogs() {
    let desktop = LightcraftApp::new(Session::with_demo(), Services { png: None, ..Default::default() });
    let ids = menu_ids(&desktop);
    assert!(ids.iter().any(|i| i == "file.addPhotos") && ids.iter().any(|i| i == "@Import from Device"));
    assert!(!ids.iter().any(|i| i.starts_with("file.importFrom") || i == "file.importFolderFromFiles"), "{ids:?}");
    let mut desktop = desktop;
    assert!(desktop.run("file.importFromPhotos", json!({})).is_err(), "no picker without the host");

    let (mut app, asked) = picking_app();
    let ids = menu_ids(&app);
    for id in ["file.importFromPhotos", "file.importFromFiles", "file.importFolderFromFiles"] {
        assert!(ids.iter().any(|i| i == id), "{id} in {ids:?}");
    }
    for id in ["file.addPhotos", "file.addFolder", "@Import from Device"] {
        assert!(!ids.iter().any(|i| i == id), "{id} hidden: {ids:?}");
    }
    assert_eq!(app.run("file.importFromPhotos", json!({})).unwrap()["picking"], true);
    app.run("file.importFromFiles", json!({})).unwrap();
    app.run("file.importFolderFromFiles", json!({})).unwrap();
    use crate::PickSource as P;
    assert_eq!(*asked.borrow(), [P::Photos, P::Files, P::Folder]);
}

/// The phone grid's + button offers the pickers; the loupe has none.
#[test]
fn the_phone_grid_has_an_add_button() {
    let (mut app, asked) = picking_app();
    app.ui.view = crate::state::ViewMode::PhotoGrid;
    let mut h = Headless::new(app, [390.0, 844.0], 1.0);
    h.settle(SETTLE);
    assert!(h.app.compact && has(&h, "button:addPhotos"));
    let r = h.request("ui.clickWidget", json!({"id": "button:addPhotos"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    let r = h.request("ui.clickWidget", json!({"id": "button:file.importFromFiles"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert_eq!(*asked.borrow(), [crate::PickSource::Files]);
    h.app.ui.view = crate::state::ViewMode::Detail;
    h.settle(SETTLE);
    assert!(!has(&h, "button:addPhotos"));
    // without host pickers (desktop-built compact window): no button
    let mut h = Headless::new(LightcraftApp::new(Session::with_demo(), Services { png: None, ..Default::default() }), [390.0, 844.0], 1.0);
    h.app.ui.view = crate::state::ViewMode::PhotoGrid;
    h.settle(SETTLE);
    assert!(!has(&h, "button:addPhotos"));
}

/// A dialog's text field keeps the keyboard up: focus is asked for once, not every frame (each
/// request interrupts IME composition, which on iOS hides the keyboard and shows it again).
#[test]
fn dialog_fields_dont_restart_the_keyboard_every_frame() {
    let mut h = Headless::new(LightcraftApp::new(Session::with_demo(), Services::default()), [390.0, 844.0], 3.0);
    h.app.run("dialog.newAlbum", json!({})).unwrap();
    for _ in 0..3 {
        h.step();
    }
    let ime = h.view.ime.expect("the name field has the keyboard");
    assert!(!ime.should_interrupt_composition);
    h.step();
    assert!(h.view.ime.as_ref().is_some_and(|i| !i.should_interrupt_composition));
}
