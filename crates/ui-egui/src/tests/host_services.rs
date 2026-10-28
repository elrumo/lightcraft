//! What a touch host (iOS) gives the app besides a file dialog: pickers of its own whose photos
//! arrive as copies in a temporary folder (staged imports), the share sheet and the photo library
//! for exports, haptic feedback and the grid's scroll to top.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;

use crate::haptics::{Haptic, tap};
use crate::headless::Headless;
use crate::menus::menubar::{MenuNode, menu_bar};
use crate::state::Dialog;
use crate::{LightcraftApp, PickSource, SaveDone, Services, ShareExports};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

/// A scratch folder that goes away with the test, however it ends.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new(tag: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("lc-ui-host-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }

    fn path(&self, rel: &str) -> std::path::PathBuf {
        self.0.join(rel)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn write_png(path: &std::path::Path, seed: u8) {
    let (w, h) = (24usize, 16usize);
    let data: Vec<[u8; 4]> = (0..w * h).map(|i| [(i % w * 9) as u8, (i / w * 13) as u8, seed, 255]).collect();
    let img = lightcraft_raster::Rgba8 { width: w, height: h, data };
    let bytes = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn files_under(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                todo.push(p);
            } else {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// The ids of the menu bar's items (submenus included).
fn menu_ids(app: &LightcraftApp) -> Vec<String> {
    fn walk(nodes: &[MenuNode], out: &mut Vec<String>) {
        for n in nodes {
            match n {
                MenuNode::Item { id, .. } => out.push(id.clone()),
                MenuNode::Submenu { children, .. } => walk(children, out),
                MenuNode::Separator => {}
            }
        }
    }
    let mut out = Vec::new();
    for (_, items) in menu_bar(app) {
        walk(&items, &mut out);
    }
    out
}

/// The photos the host's picker copied into its temporary folder; the library is a new empty
/// one on disk.
fn staged_app(dir: &Scratch) -> (Headless, Vec<String>) {
    let staged: Vec<String> = (0..3u8)
        .map(|i| {
            let p = dir.path(&format!("tmp/picked/IMG_{i}.png"));
            write_png(&p, i * 40);
            p.to_string_lossy().to_string()
        })
        .collect();
    let mut app = LightcraftApp::new(lightcraft_engine::Session::new().with_fs(), Services { png: None, ..Default::default() });
    app.run("app.newLibrary", json!({"path": dir.path("lib").to_string_lossy(), "name": "Phone"})).unwrap();
    assert!(app.session.library.as_ref().unwrap().on_disk);
    (Headless::new(app, [1300.0, 900.0], 1.0), staged)
}

#[test]
fn a_staged_import_moves_the_hosts_copies_into_the_library() {
    let dir = Scratch::new("staged");
    let (mut h, staged) = staged_app(&dir);
    let r = h.app.run("file.addPhotos", json!({"paths": staged, "staged": true})).unwrap();
    assert_eq!(r["scanning"], true, "{r}");
    assert!(h.step_until(SETTLE, |h| matches!(h.app.ui.dialog, Some(Dialog::Import { .. }))), "the review opens");
    let Some(Dialog::Import { opts }) = &h.app.ui.dialog else { panic!("no import review") };
    assert_eq!(opts.candidates.len(), 3);
    assert!(opts.staged && opts.copy && opts.move_files, "the host's copies are moved in, not referenced");
    // nothing to choose about the source or the transfer: the host's folder means nothing to the user
    h.settle(T);
    assert!(h.app.widgets.iter().all(|(w, _)| w != "label:importSource" && w != "button:importAdd"));
    assert!(h.app.widgets.iter().any(|(w, _)| w == "label:importModeHelp"));
    let r = h.request("ui.dialog.confirm", json!({}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert!(h.app.import.is_none(), "finished");
    assert_eq!(h.app.session.catalog.len(), 3);
    assert!(staged.iter().all(|p| !std::path::Path::new(p).exists()), "the temporary copies are gone");
    let library = h.app.session.library.as_ref().unwrap().dir.clone();
    let originals = files_under(&library.join("Originals"));
    assert_eq!(originals.len(), 3, "{originals:?}");
    h.app.session.close_library().unwrap();
}

#[test]
fn a_staged_import_needs_a_library_on_disk() {
    let dir = Scratch::new("staged-memory");
    let photo = dir.path("tmp/one.png");
    write_png(&photo, 9);
    let mut app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), Default::default());
    let e = app.run("file.addPhotos", json!({"paths": [photo.to_string_lossy()], "staged": true})).unwrap_err();
    assert!(e.contains("isn't saved on this device"), "{e}");
    assert!(app.scan.is_none(), "nothing was scanned");
    assert!(photo.exists(), "and nothing was touched");
}

#[test]
fn the_hosts_pickers_are_commands_only_where_the_host_has_them() {
    let picked: Arc<Mutex<Vec<PickSource>>> = Default::default();
    let log = picked.clone();
    let mut app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), Default::default());
    // a desktop host: no such commands, and the file dialogs stay in the menu
    for id in ["file.importFromPhotos", "file.importFromFiles", "file.importFolderFromFiles"] {
        assert!(!crate::menus::ui_enabled(&app, id), "{id}");
        assert_eq!(app.run(id, json!({})).unwrap_err(), "this app has no photo picker");
    }
    let ids = menu_ids(&app);
    assert!(ids.iter().any(|i| i == "file.addPhotos") && ids.iter().any(|i| i == "file.addFolder"));
    assert!(!ids.iter().any(|i| i.starts_with("file.importFrom") || i == "file.importFolderFromFiles"));
    // an iOS host: its pickers replace the file dialogs
    app.services.host_pick = Some(Box::new(move |source| {
        log.lock().unwrap().push(source);
        Ok(())
    }));
    for (id, source) in [
        ("file.importFromPhotos", PickSource::Photos),
        ("file.importFromFiles", PickSource::Files),
        ("file.importFolderFromFiles", PickSource::Folder),
    ] {
        assert!(crate::menus::ui_enabled(&app, id), "{id}");
        assert_eq!(app.run(id, json!({})).unwrap(), json!({"picking": true}));
        assert_eq!(picked.lock().unwrap().last(), Some(&source));
    }
    let ids = menu_ids(&app);
    assert!(ids.iter().any(|i| i == "file.importFromPhotos") && ids.iter().any(|i| i == "file.importFromFiles"));
    assert!(!ids.iter().any(|i| i == "file.addPhotos" || i == "file.addFolder"), "{ids:?}");
    // a picker that can't be shown says why
    app.services.host_pick = Some(Box::new(|_| Err("no window".into())));
    assert_eq!(app.run("file.importFromPhotos", json!({})).unwrap_err(), "no window");
}

/// A session whose photos export with the sync writer, and the share sheet's recorded hand-offs.
fn sharing_app(dir: &Scratch) -> (Headless, Arc<Mutex<Vec<Vec<String>>>>) {
    let shared: Arc<Mutex<Vec<Vec<String>>>> = Default::default();
    let log = shared.clone();
    let staging = dir.path("staging");
    std::fs::create_dir_all(&staging).unwrap();
    let services = Services {
        png: None,
        write: Some(Box::new(lightcraft_engine::export::write_file)),
        share_exports: Some(ShareExports {
            dir: staging.to_string_lossy().to_string(),
            share: Box::new(move |files| log.lock().unwrap().push(files.to_vec())),
        }),
        ..Default::default()
    };
    let mut h = Headless::new(LightcraftApp::new(lightcraft_engine::Session::with_demo(), services), [1300.0, 900.0], 1.0);
    h.settle(T);
    (h, shared)
}

fn first_photo(h: &mut Headless) -> u64 {
    h.app.session.visible_cloned().first().map(|p| p.0).unwrap()
}

#[test]
fn an_export_goes_through_the_share_sheet_from_an_emptied_staging_folder() {
    let dir = Scratch::new("share");
    let (mut h, shared) = sharing_app(&dir);
    let stale = dir.path("staging/old.jpg");
    std::fs::write(&stale, b"left by an earlier export").unwrap();
    let elsewhere = dir.path("elsewhere");
    let id = first_photo(&mut h);
    // (a folder asked for is the desktop's way: here the staging folder is the only place)
    let r = h.app.run("app.export", json!({"ids": [id], "dir": elsewhere.to_string_lossy(), "longEdge": 48})).unwrap();
    assert_eq!(r["files"].as_array().map(Vec::len), Some(1), "{r}");
    assert!(!stale.exists(), "the earlier export's files are gone");
    let written = files_under(&dir.path("staging"));
    assert_eq!(written.len(), 1, "{written:?}");
    assert!(!elsewhere.exists(), "nothing was written to the folder asked for");
    assert_eq!(*shared.lock().unwrap(), vec![vec![written[0].to_string_lossy().to_string()]], "the share sheet is offered the file");
    // Export with Previous must not remember a staging folder (it moves with an app update)
    let last = h.app.session.last_export.clone().unwrap();
    assert!(last.get("dir").is_none() && last.get("sendTo").is_none(), "{last}");
}

#[test]
fn the_export_dialog_has_no_folder_to_choose_where_the_share_sheet_takes_over() {
    let dir = Scratch::new("share-dialog");
    let (mut h, _) = sharing_app(&dir);
    h.app.run("dialog.export", json!({})).unwrap();
    h.settle(T);
    assert!(matches!(h.app.ui.dialog, Some(Dialog::Export { .. })));
    assert!(h.app.widgets.iter().any(|(w, _)| w == "label:exportShareHelp"));
    assert!(h.app.widgets.iter().all(|(w, _)| w != "exportChooseFolder" && w != "exportDestinationMode"));
}

#[test]
fn save_to_photos_hands_the_files_to_the_photo_library_and_reports_back() {
    let dir = Scratch::new("photos");
    let (mut h, shared) = sharing_app(&dir);
    let saves: Arc<Mutex<Vec<(Vec<String>, SaveDone)>>> = Default::default();
    let log = saves.clone();
    h.app.services.save_to_photos = Some(Box::new(move |files, done| log.lock().unwrap().push((files.to_vec(), done))));
    assert!(crate::menus::ui_enabled(&h.app, "app.saveToPhotos"));
    assert!(menu_ids(&h.app).iter().any(|i| i == "app.saveToPhotos"));
    let id = first_photo(&mut h);
    h.app.run("app.saveToPhotos", json!({"ids": [id]})).unwrap();
    // (this host can't write from a worker: the export ran on the spot)
    assert_eq!(saves.lock().unwrap().len(), 1, "the photo library is asked once");
    assert!(shared.lock().unwrap().is_empty(), "and the share sheet is not");
    let (files, done) = saves.lock().unwrap().pop().unwrap();
    assert_eq!(files.len(), 1);
    assert!(std::path::Path::new(&files[0]).exists());
    // the library has the photo, on the host's own thread: the app says so on its next frame
    done(Ok(1));
    h.step();
    assert!(h.app.saved.lock().unwrap().is_empty(), "taken");
    assert_eq!(h.app.ui.toast.as_ref().map(|t| t.0.as_str()), Some("Saved 1 photo to Photos"));
}

#[test]
fn save_to_photos_refuses_what_the_photo_library_does_not_keep_and_hosts_without_one() {
    let dir = Scratch::new("photos-refused");
    let (mut h, _) = sharing_app(&dir);
    let id = first_photo(&mut h);
    // no photo library on this host
    assert!(!crate::menus::ui_enabled(&h.app, "app.saveToPhotos"));
    assert!(!menu_ids(&h.app).iter().any(|i| i == "app.saveToPhotos"));
    let e = h.app.run("app.export", json!({"ids": [id], "sendTo": "photos"})).unwrap_err();
    assert!(e.contains("no photo library"), "{e}");
    h.app.services.save_to_photos = Some(Box::new(|_, done| done(Ok(0))));
    let e = h.app.run("app.export", json!({"ids": [id], "sendTo": "photos", "format": "webp"})).unwrap_err();
    assert!(e.contains("Photos keeps JPEG, PNG, TIFF and DNG"), "{e}");
    let e = h.app.run("app.export", json!({"ids": [id], "sendTo": "dropbox"})).unwrap_err();
    assert!(e.contains("unknown `sendTo`"), "{e}");
    assert!(files_under(&dir.path("staging")).is_empty(), "nothing was exported");
    // a library that refuses (permission) says why
    h.app.saved.lock().unwrap().push(Err("Photos access is off for LightCraft".into()));
    h.step();
    assert_eq!(h.app.ui.toast.as_ref().map(|t| t.0.as_str()), Some("Photos access is off for LightCraft"));
}

#[test]
fn haptic_taps_asked_for_in_a_frame_reach_the_host_once_the_frame_is_done() {
    let played: Arc<Mutex<Vec<Haptic>>> = Default::default();
    let sink = played.clone();
    let mut h = Headless::new(LightcraftApp::new(lightcraft_engine::Session::with_demo(), Default::default()), [1300.0, 900.0], 1.0);
    h.app.services.haptic = Some(Box::new(move |t| sink.lock().unwrap().push(t)));
    h.settle(T);
    played.lock().unwrap().clear();
    tap(&h.view.ctx, Haptic::Warning);
    tap(&h.view.ctx, Haptic::Warning);
    tap(&h.view.ctx, Haptic::Selection);
    h.step();
    assert_eq!(*played.lock().unwrap(), [Haptic::Warning, Haptic::Selection]);
    h.step();
    assert_eq!(played.lock().unwrap().len(), 2, "each tap is played once");
}

#[test]
fn scroll_to_top_scrolls_the_grid_back_up() {
    let mut h = Headless::new(LightcraftApp::new(lightcraft_engine::Session::with_demo(), Default::default()), [900.0, 600.0], 1.0);
    h.app.ui.view = crate::state::ViewMode::PhotoGrid;
    h.settle(T);
    assert!(crate::menus::ui_enabled(&h.app, "view.scrollToTop"));
    h.app.run("view.scrollToTop", json!({})).unwrap();
    assert!(h.app.ui.scroll_top);
    h.step();
    assert!(!h.app.ui.scroll_top, "the grid took it");
}
