//! Headless tests of the phone's export flow (`panels::export`): the share sheet (which photos, and
//! where to send them), the export options, the rest of the options, the progress card and Save to
//! Photos, with hosts that have a share sheet and a photo library (iOS) and one that has neither.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use lightcraft_engine::Session;
use serde_json::json;

use crate::headless::Headless;
use crate::state::{Dialog, ExportPage, ExportThen, RightPanel, ViewMode};
use crate::{LightcraftApp, SaveDone, SaveToPhotos, Services, ShareExports};

const T: Duration = Duration::from_secs(30);
const SETTLE: Duration = Duration::from_secs(120);

type Log = Rc<RefCell<Vec<Vec<String>>>>;

/// A phone with what the iOS host offers: files go to a staging folder and the share sheet and the photo
/// library are called with them (the logs). With `held`, writing a file waits until [`Phone::release`].
struct Phone {
    h: Headless,
    root: PathBuf,
    shared: Log,
    saved: Log,
    gate: Arc<AtomicBool>,
}

impl Drop for Phone {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn write(path: &str, bytes: &[u8]) -> Result<(), String> {
    if let Some(d) = Path::new(path).parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, bytes).map_err(|e| e.to_string())
}

fn phone_with(tag: &str, save: Option<Result<(), &'static str>>, held: bool) -> Phone {
    let root = std::env::temp_dir().join(format!("lc-ui-export-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let staging = root.join("Exports");
    std::fs::create_dir_all(&staging).unwrap();
    let (shared, saved): (Log, Log) = (Rc::default(), Rc::default());
    let sink = shared.clone();
    let log = saved.clone();
    let save_to_photos: Option<SaveToPhotos> = save.map(|result| {
        Box::new(move |files: &[String], done: SaveDone| {
            log.borrow_mut().push(files.to_vec());
            done(result.map(|()| files.len()).map_err(str::to_string));
        }) as SaveToPhotos
    });
    let gate = Arc::new(AtomicBool::new(!held));
    let wait = gate.clone();
    let services = Services {
        png: None,
        write: Some(Box::new(write)),
        write_shared: Some(Arc::new(move |path: &str, bytes: &[u8]| {
            while !wait.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(5));
            }
            write(path, bytes)
        })),
        share_exports: Some(ShareExports {
            dir: staging.to_string_lossy().to_string(),
            share: Box::new(move |files| sink.borrow_mut().push(files.to_vec())),
        }),
        save_to_photos,
        ..Default::default()
    };
    let mut h = Headless::new(LightcraftApp::new(Session::with_demo(), services), [390.0, 844.0], 1.0);
    h.settle(SETTLE);
    let r = h.request("engine.execute", json!({"command": "library.select", "params": {"ids": [1]}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.app.ui.view = ViewMode::Detail;
    h.app.ui.right = RightPanel::None;
    h.settle(SETTLE);
    assert!(h.app.compact);
    Phone { h, root, shared, saved, gate }
}

fn phone(tag: &str) -> Phone {
    phone_with(tag, Some(Ok(())), false)
}

impl Phone {
    fn has(&self, id: &str) -> bool {
        self.h.app.widgets.iter().any(|(w, _)| w == id)
    }

    fn click(&mut self, id: &str) {
        let r = self.h.request("ui.clickWidget", json!({"id": id}), T);
        assert_eq!(r["ok"], true, "{id}: {r}");
        self.h.settle(SETTLE);
    }

    fn open(&mut self) {
        let r = self.h.request("engine.execute", json!({"command": "dialog.export"}), T);
        assert_eq!(r["ok"], true, "{r}");
        self.frames(40);
    }

    /// `n` frames: pages slide for a quarter of a second.
    fn frames(&mut self, n: u32) {
        for _ in 0..n {
            self.h.step();
        }
    }

    /// Wait until no export is running (and what it handed on has been seen).
    fn finish(&mut self) {
        for _ in 0..600 {
            self.h.settle(SETTLE);
            if self.h.app.export.is_none() {
                break;
            }
            self.h.step();
        }
        self.frames(5);
        assert!(self.h.app.export.is_none(), "the export finished");
    }

    /// Let the files that are waiting be written.
    fn release(&self) {
        self.gate.store(true, Ordering::Relaxed);
    }

    fn ids(&self) -> Vec<u64> {
        match &self.h.app.ui.dialog {
            Some(Dialog::Export { ids, .. }) => ids.clone(),
            other => panic!("no export dialog: {other:?}"),
        }
    }

    fn page(&self) -> ExportPage {
        match &self.h.app.ui.dialog {
            Some(Dialog::Export { page, .. }) => *page,
            other => panic!("no export dialog: {other:?}"),
        }
    }

    fn options(&self) -> lightcraft_engine::export::ExportOptions {
        match &self.h.app.ui.dialog {
            Some(Dialog::Export { opts, .. }) => opts.clone(),
            other => panic!("no export dialog: {other:?}"),
        }
    }

    fn toast(&self) -> String {
        self.h.app.ui.toast.as_ref().map(|(t, _)| t.clone()).unwrap_or_default()
    }

    fn photo(&self, n: usize) -> u64 {
        self.h.app.session.catalog.photos().nth(n).map(|p| p.id.0).expect("a demo photo")
    }
}

/// The share sheet starts with the open photo chosen, and lists where it can go.
#[test]
fn the_share_sheet_opens_with_the_open_photo_chosen() {
    let mut p = phone("sheet");
    p.open();
    assert_eq!(p.page(), ExportPage::Share);
    assert_eq!(p.ids(), vec![1]);
    for id in [
        "sheet:dialog",
        "button:shareClose",
        "button:shareSelectAll",
        "button:shareShare",
        "button:shareExportAs",
        "button:shareSave",
        "icon:shareSave",
        "label:shareCount",
        "thumb:share-1",
        "check:shareItem-1",
    ] {
        assert!(p.has(id), "{id} is on the sheet: {:?}", p.h.app.widgets.iter().map(|(w, _)| w.as_str()).collect::<Vec<_>>());
    }
    assert!(!p.has("sheet:dialogOptions") && !p.has("sheet:dialogMore"), "the other pages aren't up");
    // the sheet covers the screen like any page
    let rect = p.h.app.widgets.iter().find(|(w, _)| w == "sheet:dialog").map(|(_, r)| *r).unwrap();
    assert!(rect.width() >= 389.0 && rect.bottom() <= 844.5, "{rect:?}");
    p.click("button:shareClose");
    assert!(p.h.app.ui.dialog.is_none(), "✕ closes it");
    assert!(p.shared.borrow().is_empty() && p.saved.borrow().is_empty(), "and nothing was sent");
}

/// A tap on a photo, or on the box under it, chooses or unchooses it; Select All takes the lot; with
/// none chosen nothing can be sent.
#[test]
fn photos_are_chosen_with_a_tap_and_select_all() {
    let mut p = phone("choose");
    p.open();
    let other = p.photo(2);
    assert_ne!(other, 1);
    // (a photo further along the strip may be off screen: it is scrolled into view by the first
    // one being centred, so look for any other on screen)
    let on_screen: Vec<u64> =
        p.h.app.widgets.iter().filter_map(|(w, _)| w.strip_prefix("thumb:share-")).filter_map(|n| n.parse().ok()).filter(|n| *n != 1).collect();
    assert!(!on_screen.is_empty(), "photos next to the first show");
    let next = on_screen[0];
    p.click(&format!("check:shareItem-{next}"));
    assert_eq!(p.ids(), vec![1, next], "its box chose it");
    p.click(&format!("thumb:share-{next}"));
    assert_eq!(p.ids(), vec![1], "a tap on the photo unchose it");
    p.click("button:shareSelectAll");
    let all = p.h.app.session.visible().len();
    assert_eq!(p.ids().len(), all, "Select All chose every photo of the view");
    p.click("button:shareSelectAll");
    assert!(p.ids().is_empty(), "and the same button, now Deselect All, none");
    p.click("button:shareShare");
    p.click("button:shareSave");
    p.click("button:shareExportAs");
    assert_eq!(p.page(), ExportPage::Share, "with nothing chosen none of them does anything");
    assert!(p.h.app.export.is_none() && p.shared.borrow().is_empty() && p.saved.borrow().is_empty());
}

/// Share exports the chosen photos (in the order of the grid) to the staging folder, opens the share
/// sheet with them and closes the dialog.
#[test]
fn share_exports_the_chosen_photos_and_opens_the_share_sheet() {
    let mut p = phone("share");
    p.open();
    let (a, b) = (p.photo(0), p.photo(1));
    if let Some(Dialog::Export { ids, .. }) = &mut p.h.app.ui.dialog {
        // (chosen out of order)
        *ids = vec![b, a];
    }
    p.click("button:shareShare");
    assert!(p.h.app.ui.dialog.is_none(), "the dialog is closed");
    p.finish();
    let shared = p.shared.borrow().clone();
    assert_eq!(shared.len(), 1, "one share sheet: {shared:?}");
    assert_eq!(shared[0].len(), 2, "with both photos: {shared:?}");
    assert!(shared[0].iter().all(|f| Path::new(f).is_file() && f.starts_with(p.root.join("Exports").to_str().unwrap())), "{shared:?}");
    assert!(p.saved.borrow().is_empty(), "and nothing was saved");
    // the files come out in the order of the grid, not in the order they were chosen in
    let files = p.h.app.last_export_result.as_ref().unwrap()["files"].as_array().unwrap().clone();
    let names: Vec<&str> = files.iter().filter_map(|f| f["path"].as_str()).collect();
    let mut grid: Vec<u64> = vec![a, b];
    grid.sort_by_key(|id| p.h.app.session.visible().iter().position(|v| v.0 == *id));
    let stem = |id: u64| {
        let name = p.h.app.session.catalog.photo(lightcraft_catalog::PhotoId(id)).unwrap().file_name.clone();
        name.rsplit_once('.').map_or(name.clone(), |(s, _)| s.to_string())
    };
    assert!(names[0].contains(&stem(grid[0])) && names[1].contains(&stem(grid[1])), "{names:?} for {grid:?}");
}

/// Save to Photos hands the written files to the host's photo library and says how it went.
#[test]
fn save_to_photos_hands_the_files_to_the_library() {
    let mut p = phone("save");
    p.open();
    p.click("button:shareSave");
    assert!(p.h.app.ui.dialog.is_none());
    p.finish();
    let saved = p.saved.borrow().clone();
    assert_eq!(saved.len(), 1, "{saved:?}");
    assert_eq!(saved[0].len(), 1);
    assert!(Path::new(&saved[0][0]).is_file());
    assert!(p.shared.borrow().is_empty(), "no share sheet for a save");
    assert_eq!(p.toast(), "Saved 1 photo to Photos");
    // the settings it used are remembered, but where it went isn't
    let last = p.h.app.session.last_export.clone().unwrap();
    assert!(last.get("sendTo").is_none(), "{last}");
}

/// A library that says no (no permission, a format it can't keep) is told to the user.
#[test]
fn a_refused_save_is_told() {
    let mut p = phone_with("refused", Some(Err("LightCraft may not add photos: allow it in Settings")), false);
    p.open();
    p.click("button:shareSave");
    p.finish();
    assert_eq!(p.saved.borrow().len(), 1, "the library was asked");
    assert_eq!(p.toast(), "LightCraft may not add photos: allow it in Settings");
}

/// The gear on the Save row opens the options; their check mark then saves instead of sharing.
#[test]
fn the_gear_opens_the_options_for_saving() {
    let mut p = phone("gear");
    p.open();
    p.click("icon:shareSave");
    assert_eq!(p.page(), ExportPage::Options);
    assert!(p.has("sheet:dialogOptions"));
    assert!(matches!(&p.h.app.ui.dialog, Some(Dialog::Export { then: ExportThen::Save, .. })));
    p.click("button:optionsDone");
    p.finish();
    assert_eq!(p.saved.borrow().len(), 1);
    assert!(p.shared.borrow().is_empty());
}

/// Export As… has the file type, size and quality as pull-down fields and the watermark as a switch;
/// the check mark exports with them.
#[test]
fn export_as_has_the_options_and_its_check_mark_exports() {
    let mut p = phone("options");
    p.open();
    p.click("button:shareExportAs");
    p.frames(40);
    assert_eq!(p.page(), ExportPage::Options);
    for id in [
        "sheet:dialogOptions",
        "button:optionsClose",
        "button:optionsDone",
        "label:exportCount",
        "button:exportFileType",
        "button:exportSize",
        "button:exportQuality",
        "check:exportWatermark",
        "button:exportMore",
    ] {
        assert!(p.has(id), "{id}");
    }
    // a pull-down: tap the field, then a row
    p.click("button:exportFileType");
    p.frames(20);
    assert!(p.has("actions:exportFileType"), "the menu is open");
    p.click("button:exportFileType-1");
    assert_eq!(p.options().format, lightcraft_engine::export::ExportFormat::Png);
    p.frames(40);
    assert!(!p.has("button:exportQuality"), "a PNG has no quality");
    p.click("button:exportFileType");
    p.frames(20);
    p.click("button:exportFileType-0");
    p.frames(40);
    assert!(p.has("button:exportQuality"), "a JPEG has");
    p.click("button:exportQuality");
    p.frames(20);
    p.click("button:exportQuality-1");
    assert_eq!(p.options().quality, 95);
    // sizes: the named ones, and full size
    p.click("button:exportSize");
    p.frames(20);
    p.click("button:exportSize-0");
    p.frames(40);
    match &p.h.app.ui.dialog {
        Some(Dialog::Export { full_size, resize, .. }) => assert!(!*full_size && resize.value == 1080.0, "{resize:?}"),
        other => panic!("{other:?}"),
    }
    p.click("button:exportSize");
    p.frames(20);
    p.click("button:exportSize-full");
    p.frames(40);
    assert!(matches!(&p.h.app.ui.dialog, Some(Dialog::Export { full_size: true, .. })));
    // the watermark, with its text
    assert!(p.options().watermark.is_none());
    p.click("check:exportWatermark");
    assert!(p.options().watermark.is_some());
    p.frames(5);
    assert!(p.has("field:exportWatermarkText"));
    // ✓ exports with all that
    p.click("button:optionsDone");
    assert!(p.h.app.ui.dialog.is_none());
    p.finish();
    let last = p.h.app.session.last_export.clone().unwrap();
    assert_eq!(last["quality"], 95, "{last}");
    assert_eq!(last["format"], "jpeg", "{last}");
    assert!(last.get("watermark").is_some(), "{last}");
    assert_eq!(last["longEdge"], 0, "full size: {last}");
    assert_eq!(p.shared.borrow().len(), 1, "and the share sheet opened");
}

/// ✕ on the options goes back to the share sheet (the choices stay), and More Options has everything else.
#[test]
fn more_options_has_the_rest_and_back_comes_back() {
    let mut p = phone("more");
    p.open();
    p.click("button:shareExportAs");
    p.frames(40);
    p.click("button:exportMore");
    p.frames(40);
    assert_eq!(p.page(), ExportPage::More);
    for id in
        ["sheet:dialogMore", "button:moreBack", "button:exportPreset", "check:exportDontEnlarge", "label:exportShareHelp", "check:exportFullSize"]
    {
        assert!(p.has(id), "{id}: {:?}", p.h.app.widgets.iter().map(|(w, _)| w.as_str()).collect::<Vec<_>>());
    }
    assert!(!p.has("button:exportFormat-0"), "the file type is on the options page, not in the legacy chips");
    assert!(!p.has("button:exportFolder") && !p.has("button:exportChooseFolder"), "no folder on iOS");
    // presets are a pull-down here
    p.click("button:exportPreset");
    p.frames(20);
    assert!(p.has("actions:exportPreset"));
    p.click("button:exportPreset-0");
    p.frames(5);
    p.click("button:moreBack");
    p.frames(40);
    assert_eq!(p.page(), ExportPage::Options);
    assert!(!p.has("sheet:dialogMore"), "the page slid away");
    p.click("button:optionsClose");
    p.frames(40);
    assert_eq!(p.page(), ExportPage::Share);
    assert_eq!(p.ids(), vec![1]);
    assert!(!p.has("sheet:dialogOptions"));
}

/// A host with neither a share sheet nor a photo library starts at the options: the files go to a folder.
#[test]
fn a_host_without_a_share_sheet_starts_at_the_options() {
    let app = LightcraftApp::new(Session::with_demo(), Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [390.0, 844.0], 1.0);
    h.settle(SETTLE);
    h.app.ui.view = ViewMode::Detail;
    let r = h.request("engine.execute", json!({"command": "library.select", "params": {"ids": [1]}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    let r = h.request("engine.execute", json!({"command": "dialog.export"}), T);
    assert_eq!(r["ok"], true, "{r}");
    for _ in 0..40 {
        h.step();
    }
    let has = |h: &Headless, id: &str| h.app.widgets.iter().any(|(w, _)| w == id);
    assert!(matches!(&h.app.ui.dialog, Some(Dialog::Export { page: ExportPage::Options, .. })));
    assert!(has(&h, "sheet:dialog") && has(&h, "button:exportFileType") && !has(&h, "button:shareShare"), "the options are the first page");
    assert!(!has(&h, "icon:save"), "and the photo has no save button");
    let r = h.request("ui.clickWidget", json!({"id": "button:optionsClose"}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert!(h.app.ui.dialog.is_none(), "✕ closes the dialog");
}

/// While photos are exported a card shows which one and how many, with a Cancel that stops it.
#[test]
fn a_card_shows_the_progress_and_cancel_stops_it() {
    let mut p = phone_with("progress", Some(Ok(())), true);
    p.open();
    let ids: Vec<u64> = (0..4).map(|n| p.photo(n)).collect();
    if let Some(Dialog::Export { ids: chosen, .. }) = &mut p.h.app.ui.dialog {
        *chosen = ids;
    }
    p.click("button:shareShare");
    p.frames(20);
    assert!(p.h.app.export.is_some(), "it is running");
    for id in ["dialog:exportProgress", "progress:export", "button:exportCancel"] {
        assert!(p.has(id), "{id}");
    }
    // (a second one while it runs is refused, and says so)
    let again = p.h.app.run("app.export", json!({"format": "jpeg", "longEdge": 64}));
    assert!(again.as_ref().is_err_and(|e| e.contains("already running")), "{again:?}");
    p.click("button:exportCancel");
    p.release();
    p.finish();
    assert_eq!(p.h.app.last_export_result.as_ref().unwrap()["cancelled"], true);
    assert!(!p.has("dialog:exportProgress"), "the card is gone");
}

/// An export that can't start (a file type the photo library doesn't keep) is said, and the page stays
/// for another try.
#[test]
fn an_export_that_cannot_start_keeps_the_page_open() {
    let mut p = phone("refuse");
    p.open();
    p.click("icon:shareSave");
    p.frames(40);
    p.click("button:exportFileType");
    p.frames(20);
    p.click("button:exportFileType-3");
    assert_eq!(p.options().format, lightcraft_engine::export::ExportFormat::Webp);
    p.frames(40);
    p.click("button:optionsDone");
    p.frames(5);
    assert!(p.h.app.ui.dialog.is_some(), "the page is still up");
    assert_eq!(p.page(), ExportPage::Options);
    assert!(p.toast().contains("JPEG, PNG, TIFF and DNG"), "{}", p.toast());
    assert!(p.h.app.export.is_none() && p.saved.borrow().is_empty(), "nothing was started");
    // another file type and the same check mark go through
    p.click("button:exportFileType");
    p.frames(20);
    p.click("button:exportFileType-0");
    p.frames(40);
    p.click("button:optionsDone");
    p.finish();
    assert_eq!(p.saved.borrow().len(), 1);
}

/// `sendTo` says where the files go; unknown values and a missing library are errors.
#[test]
fn send_to_is_checked() {
    let mut p = phone_with("sendto", None, false);
    let bad = p.h.app.run("app.export", json!({"ids": [1], "format": "jpeg", "longEdge": 64, "sendTo": "pigeon"}));
    assert!(bad.as_ref().is_err_and(|e| e.contains("sendTo")), "{bad:?}");
    let none = p.h.app.run("app.export", json!({"ids": [1], "format": "jpeg", "longEdge": 64, "sendTo": "photos"}));
    assert!(none.as_ref().is_err_and(|e| e.contains("photo library")), "{none:?}");
    let r = p.h.app.run("app.saveToPhotos", json!({}));
    assert!(r.as_ref().is_err_and(|e| e.contains("photo library")), "{r:?}");
    assert!(p.h.app.run("app.export", json!({"ids": [1], "format": "jpeg", "longEdge": 64, "sendTo": "share"})).is_ok());
    // with a library it is a command like the others
    let mut p = phone("sendto2");
    // (the photo library keeps JPEG, PNG, TIFF and DNG: nothing else is exported to be refused afterwards)
    for format in ["webp", "avif", "original"] {
        let r = p.h.app.run("app.export", json!({"ids": [1], "format": format, "sendTo": "photos"}));
        assert!(r.as_ref().is_err_and(|e| e.contains("JPEG, PNG, TIFF and DNG")), "{format}: {r:?}");
    }
    assert!(p.h.app.export.is_none() && p.saved.borrow().is_empty());
    assert!(crate::menus::ui_enabled(&p.h.app, "app.saveToPhotos"));
    let r = p.h.app.run("app.saveToPhotos", json!({"longEdge": 128}));
    assert_eq!(r.as_ref().map(|v| v["background"].clone()).ok(), Some(json!(true)), "{r:?}");
    p.finish();
    assert_eq!(p.saved.borrow().len(), 1);
    let last = p.h.app.session.last_export.clone().unwrap();
    assert_eq!(last["longEdge"], 128, "{last}");
}

/// The photo's top bar has a Save button where there is a photo library; it saves with one tap.
#[test]
fn the_photo_bar_saves_with_one_tap() {
    let mut p = phone("bar");
    assert!(p.has("icon:save") && p.has("icon:share"));
    p.click("icon:save");
    p.finish();
    assert_eq!(p.saved.borrow().len(), 1, "one tap, the library has the photo");
    assert!(p.h.app.ui.dialog.is_none(), "no dialog");
    assert_eq!(p.toast(), "Saved 1 photo to Photos");
    p.click("icon:share");
    p.frames(40);
    assert!(p.has("sheet:dialog") && p.has("button:shareShare"), "the share button opens the sheet");
}

/// From the grid, Export in the choose-photos bar opens the sheet with the chosen photos.
#[test]
fn the_grid_exports_the_chosen_photos() {
    let mut p = phone("grid");
    p.h.app.ui.view = ViewMode::PhotoGrid;
    p.h.settle(SETTLE);
    p.click("button:select");
    let (a, b) = (p.photo(0), p.photo(1));
    let r = p.h.request("engine.execute", json!({"command": "library.select", "params": {"ids": [a, b]}}), T);
    assert_eq!(r["ok"], true, "{r}");
    p.h.settle(SETTLE);
    p.click("icon:selExport");
    p.frames(40);
    let mut want = vec![a, b];
    want.sort_by_key(|id| p.h.app.session.visible().iter().position(|v| v.0 == *id));
    assert_eq!(p.ids(), want, "both, in the grid's order");
    assert!(p.has("button:shareShare"));
}
