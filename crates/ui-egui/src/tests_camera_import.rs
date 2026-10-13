//! Headless tests of importing from a camera the host reads itself (`Services::cameras`; iOS and
//! macOS: ImageCaptureCore): the review lists its files by day as it catalogues them, greys out
//! those the library already has, copies the chosen ones off it and imports the copies; a camera
//! connected later opens the import.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use lightcraft_engine::Session;
use lightcraft_engine::devices::CameraFile;
use serde_json::json;

use crate::headless::Headless;
use crate::{CameraHost, CameraKind, CopyDone, HostCamera, HostContents, HostCopied, HostTransfer, LightcraftApp, Services};

const T: Duration = Duration::from_secs(30);
const SETTLE: Duration = Duration::from_secs(120);

fn base(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-ui-camera-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn jpeg(seed: u8) -> Vec<u8> {
    let img = lightcraft_raster::Rgba8::from_fn(24, 16, |x, y| [(x * 9) as u8 ^ seed, (y * 12) as u8, seed, 255]);
    let o = lightcraft_engine::export::ExportOptions { format: lightcraft_engine::export::ExportFormat::Jpeg, ..Default::default() };
    lightcraft_engine::export::encode_image(&img, &o).unwrap()
}

/// What the fake camera holds and what it was asked.
#[derive(Default)]
struct Camera {
    cameras: Vec<HostCamera>,
    /// Its files and their bytes.
    files: Vec<(CameraFile, Vec<u8>)>,
    complete: bool,
    rev: u64,
    staging: PathBuf,
    opened: Vec<String>,
    thumbnails: usize,
    copied: Vec<String>,
    /// Copies wait for [`Camera::release`] (to see the copy in progress).
    hold: bool,
    held: Option<(Vec<String>, PathBuf, Arc<HostTransfer>, CopyDone)>,
}

impl Camera {
    /// Copy the files of a held download, and say it is done.
    fn copy(&mut self, files: &[String], dir: &Path, transfer: &HostTransfer) -> Result<HostCopied, String> {
        let mut out = HostCopied::default();
        for id in files {
            let (f, bytes) = self.files.iter().find(|(f, _)| &f.id == id).cloned().ok_or("no such file")?;
            let to = dir.join(&f.name);
            std::fs::write(&to, bytes).map_err(|e| e.to_string())?;
            out.files.push(to.to_string_lossy().to_string());
            self.copied.push(f.name.clone());
            transfer.done.fetch_add(1, Ordering::Relaxed);
        }
        Ok(out)
    }

    fn release(&mut self) {
        if let Some((files, dir, transfer, done)) = self.held.take() {
            let out = self.copy(&files, &dir, &transfer).unwrap();
            done(out);
        }
    }

    fn add(&mut self, name: &str, captured: &str, seed: u8) {
        let bytes = jpeg(seed);
        let f = CameraFile {
            id: format!("id-{name}"),
            name: name.into(),
            size: bytes.len() as u64,
            captured: Some(captured.into()),
            ..Default::default()
        };
        self.files.push((f, bytes));
        self.rev += 1;
    }
}

/// A camera host that is a [`Camera`] in memory: thumbnails at once, copies written on the spot.
struct Fake(Rc<RefCell<Camera>>);

impl CameraHost for Fake {
    fn cameras(&self) -> Vec<HostCamera> {
        self.0.borrow().cameras.clone()
    }
    fn open(&self, camera: &str) -> Result<(), String> {
        self.0.borrow_mut().opened.push(camera.into());
        Ok(())
    }
    fn contents(&self, _camera: &str, seen: u64) -> Option<(u64, HostContents)> {
        let c = self.0.borrow();
        (c.rev != seen).then(|| (c.rev, HostContents { files: c.files.iter().map(|(f, _)| f.clone()).collect(), complete: c.complete, error: None }))
    }
    fn thumbnail(&self, _camera: &str, file: &str) -> Option<Result<Vec<u8>, String>> {
        let mut c = self.0.borrow_mut();
        c.thumbnails += 1;
        c.files.iter().find(|(f, _)| f.id == file).map(|(_, b)| Ok(b.clone()))
    }
    fn staging(&self) -> PathBuf {
        self.0.borrow().staging.clone()
    }
    fn download(&self, _camera: &str, files: &[String], dir: &Path, transfer: Arc<HostTransfer>, done: CopyDone) -> Result<(), String> {
        let mut c = self.0.borrow_mut();
        if c.hold {
            c.held = Some((files.to_vec(), dir.to_path_buf(), transfer, done));
            return Ok(());
        }
        let out = c.copy(files, dir, &transfer)?;
        done(out);
        Ok(())
    }
    fn close(&self, _camera: &str) {}
}

fn app_with_camera(root: &Path, size: [f32; 2]) -> (Headless, Rc<RefCell<Camera>>) {
    let state = Rc::new(RefCell::new(Camera { staging: root.join("staging"), ..Default::default() }));
    let mut s = Session::new().with_fs();
    s.open_library(root.join("Library"), false).unwrap();
    // (a PNG encoder for the screenshots `shot` takes)
    let png: Box<dyn Fn(&lightcraft_raster::Rgba8) -> Vec<u8>> = Box::new(|img| {
        lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(img), &lightcraft_codecs::EncodeMeta::default()).unwrap_or_default()
    });
    let services = Services {
        png: Some(png),
        write: Some(Box::new(lightcraft_engine::export::write_file)),
        cameras: Some(Box::new(Fake(state.clone()))),
        ..Default::default()
    };
    let app = LightcraftApp::new(s, services);
    let mut h = Headless::new(app, size, 1.0);
    h.settle(SETTLE);
    (h, state)
}

fn has(h: &Headless, id: &str) -> bool {
    h.app.widgets.iter().any(|(w, _)| w == id)
}

fn review(h: &Headless) -> &crate::import::ImportDialog {
    match &h.app.ui.dialog {
        Some(crate::state::Dialog::Import { opts }) => opts,
        d => panic!("no import review: {d:?}"),
    }
}

/// With `LIGHTCRAFT_TEST_SHOTS` set, a screenshot there (to look at the review).
fn shot(h: &mut Headless, name: &str) {
    if let Some(p) = std::env::var_os("LIGHTCRAFT_TEST_SHOTS") {
        let r = h.request("ui.screenshot", json!({"path": Path::new(&p).join(name).to_string_lossy(), "headless": true}), T);
        assert_eq!(r["ok"], true, "{r}");
    }
}

fn names(d: &crate::import::ImportDialog) -> Vec<&str> {
    d.candidates.iter().map(|c| c.name.as_str()).collect()
}

/// The whole flow, at a size: the camera's files arrive while the review is open and are listed
/// newest day first under a heading per day; a day's box unchecks it; Import copies the checked
/// files off the camera and imports them; opened again, those are greyed out in the library.
fn camera_import_at(size: [f32; 2], tag: &str) {
    let root = base(tag);
    let (mut h, cam) = app_with_camera(&root, size);
    let camera = HostCamera { id: "cam1".into(), name: "Canon PowerShot V1".into(), kind: CameraKind::Camera };
    {
        let mut c = cam.borrow_mut();
        c.cameras.push(camera.clone());
        c.add("IMG_0001.JPG", "2026-10-03T10:00:00", 10);
        c.add("IMG_0002.JPG", "2026-10-03T10:05:00", 60);
    }
    let r = h.request("engine.execute", json!({"command": "file.cameras"}), T);
    assert_eq!(r["result"][0]["id"], "cam1", "{r}");
    let r = h.request("engine.execute", json!({"command": "file.addFromDevice", "params": {"camera": "cam1"}}), T);
    assert_eq!(r["result"]["camera"], "cam1", "{r}");
    h.settle(SETTLE);
    assert_eq!(cam.borrow().opened, ["cam1"]);
    assert_eq!(names(review(&h)), ["IMG_0001.JPG", "IMG_0002.JPG"]);
    assert!(has(&h, "label:importCameraStatus"), "says it is still reading the camera");
    assert!(has(&h, "importDevice"), "the camera's own row");
    // the rest arrives: an older day, with a raw file
    {
        let mut c = cam.borrow_mut();
        c.add("IMG_0003.JPG", "2026-08-03T09:00:00", 110);
        c.add("IMG_0004.DNG", "2026-08-03T09:01:00", 160);
        c.add("notes.txt", "2026-08-03T09:02:00", 0);
        c.complete = true;
    }
    h.settle(SETTLE);
    let d = review(&h);
    assert_eq!(names(d), ["IMG_0001.JPG", "IMG_0002.JPG", "IMG_0003.JPG", "IMG_0004.DNG"], "by day, newest first; not the .txt");
    assert_eq!(d.groups().iter().map(|g| g.day.clone().unwrap_or_default()).collect::<Vec<_>>(), ["2026-10-03", "2026-08-03"]);
    assert_eq!(d.selected_paths().len(), 4, "new files arrive checked");
    assert!(crate::import::is_raw(&d.candidates[3]));
    assert!(!has(&h, "label:importCameraStatus"), "done reading");
    assert!(has(&h, "importDay:2026-10-03") && has(&h, "importDay:2026-08-03"), "a heading per day");
    // thumbnails come from the camera
    assert!(h.step_until(SETTLE, |h| h.app.renderer.textures.keys().filter(|s| matches!(s, crate::render::Slot::Camera(_))).count() >= 4));
    assert!(cam.borrow().thumbnails >= 4);
    shot(&mut h, &format!("camera-{tag}-review.png"));
    // the older day's box unchecks its photos
    let r = h.request("ui.clickWidget", json!({"id": "importDay:2026-08-03"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    let d = review(&h);
    assert_eq!(d.selected_paths(), ["id-IMG_0001.JPG", "id-IMG_0002.JPG"]);
    assert_eq!(d.range_checked(2..4), Some(false));
    // Import: copied off the camera (a row in the activity stack while it runs), then moved into
    // the library
    cam.borrow_mut().hold = true;
    let r = h.request("ui.dialog.confirm", json!({}), T);
    assert_eq!(r["ok"], true, "{r}");
    for _ in 0..5 {
        h.step();
    }
    let rows: Vec<String> = h.app.session.activity.list().iter().map(|t| t.label.clone()).collect();
    assert!(rows.iter().any(|l| l == "Copying from camera"), "{rows:?}");
    let r = h.request("ui.inspect", json!({}), T);
    assert_eq!(r["result"]["cameraCopy"]["total"], 2, "{r}");
    cam.borrow_mut().release();
    h.settle(SETTLE);
    assert!(h.step_until(SETTLE, |h| h.app.camera_copy.is_none() && h.app.import.is_none()));
    assert_eq!(cam.borrow().copied, ["IMG_0001.JPG", "IMG_0002.JPG"]);
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
    assert_eq!(paths.len(), 2, "{paths:?}");
    for p in &paths {
        assert!(Path::new(p).starts_with(root.join("Library").join("Originals")) && Path::new(p).is_file(), "{p}");
    }
    let staged: Vec<_> = std::fs::read_dir(root.join("staging")).unwrap().flatten().collect();
    assert!(staged.is_empty(), "the staging folder is emptied: {staged:?}");
    // opened again: what the library has is greyed out and can't be checked
    let r = h.request("engine.execute", json!({"command": "file.addFromDevice", "params": {"camera": "cam1"}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    let d = review(&h);
    assert_eq!(names(d), ["IMG_0001.JPG", "IMG_0002.JPG", "IMG_0003.JPG", "IMG_0004.DNG"]);
    assert!(!d.importable(0) && !d.importable(1), "{:?}", d.candidates);
    assert_eq!(d.candidates[0].duplicate.as_deref(), Some("library"));
    assert!(d.importable(2) && d.importable(3));
    assert_eq!(d.selected_paths(), ["id-IMG_0003.JPG", "id-IMG_0004.DNG"]);
    assert_eq!(d.range_checked(0..2), Some(false), "nothing to check on a day the library has");
    h.step_until(SETTLE, |h| h.app.renderer.textures.keys().filter(|s| matches!(s, crate::render::Slot::Camera(_))).count() >= 4);
    shot(&mut h, &format!("camera-{tag}-again.png"));
    let r = h.request("ui.clickWidget", json!({"id": "import:0"}), T);
    assert_eq!(r["ok"], true, "{r}");
    let r = h.request("ui.clickWidget", json!({"id": "importDay:2026-10-03"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert_eq!(review(&h).selected_paths().len(), 2, "photos in the library can't be chosen");
    drop(h);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn camera_import_on_the_desktop() {
    camera_import_at([1200.0, 900.0], "desktop");
}

#[test]
fn camera_import_on_a_phone() {
    camera_import_at([390.0, 844.0], "phone");
}

/// A camera connected while the app runs opens the import (the setting, on by default); with the
/// setting off, a toast says it is there. One connected before the app started looking doesn't.
#[test]
fn a_camera_connected_opens_the_import() {
    let root = base("connect");
    let (mut h, cam) = app_with_camera(&root, [1200.0, 900.0]);
    let camera = HostCamera { id: "cam1".into(), name: "EOS R5".into(), kind: CameraKind::Camera };
    assert!(h.app.ui.settings.import_on_connect, "on by default");
    // (past the moments after start, when what is found counts as already there)
    if let Some((since, _)) = h.app.devices_seen.as_mut() {
        *since = f64::NEG_INFINITY;
    }
    cam.borrow_mut().cameras.push(camera.clone());
    h.settle(SETTLE);
    let d = review(&h);
    assert_eq!(d.camera.as_ref().map(|c| c.id.as_str()), Some("cam1"));
    assert_eq!(d.device, "EOS R5");
    // unplugged, the setting turned off, plugged in again: no review, a toast
    h.app.ui.dialog = None;
    cam.borrow_mut().cameras.clear();
    h.settle(SETTLE);
    h.app.ui.settings.import_on_connect = false;
    cam.borrow_mut().cameras.push(camera);
    h.settle(SETTLE);
    assert!(h.app.ui.dialog.is_none());
    assert!(h.app.ui.toast.as_ref().is_some_and(|t| t.0.contains("EOS R5 connected")), "{:?}", h.app.ui.toast);
    drop(h);
    let _ = std::fs::remove_dir_all(&root);
}

/// The phone's + offers the connected camera; with none, it says how to connect one.
#[test]
fn the_phones_add_button_offers_the_connected_camera() {
    let root = base("plus");
    let (mut h, cam) = app_with_camera(&root, [390.0, 844.0]);
    // (the + button is the host pickers' too: an iOS host has both)
    h.app.services.host_pick = Some(Box::new(|_| Ok(())));
    h.app.ui.view = crate::state::ViewMode::PhotoGrid;
    h.settle(SETTLE);
    let r = h.request("ui.clickWidget", json!({"id": "button:addPhotos"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert!(has(&h, "button:connectedCamera"), "the + sheet offers Connected Camera/Card");
    let r = h.request("ui.clickWidget", json!({"id": "button:connectedCamera"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert!(h.app.ui.dialog.is_none());
    assert!(h.app.ui.toast.as_ref().is_some_and(|t| t.0.contains("No camera or card connected")), "{:?}", h.app.ui.toast);
    cam.borrow_mut().cameras.push(HostCamera { id: "c".into(), name: "SD Card".into(), kind: CameraKind::Card });
    if let Some((since, seen)) = h.app.devices_seen.as_mut() {
        // (connected before: no automatic import here)
        seen.insert("camera:c".into());
        *since = f64::NEG_INFINITY;
    }
    let r = h.request("ui.clickWidget", json!({"id": "button:addPhotos"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    let r = h.request("ui.clickWidget", json!({"id": "button:connectedCamera"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert_eq!(review(&h).device, "SD Card");
    drop(h);
    let _ = std::fs::remove_dir_all(&root);
}

/// A camera's thumbnails are held up to a bound, the ones drawn longest ago dropped first; one
/// dropped is asked of the camera again when it comes back into view.
#[test]
fn camera_thumbnails_are_bounded() {
    let root = base("thumbs");
    let state = Rc::new(RefCell::new(Camera { staging: root.join("staging"), ..Default::default() }));
    let n = crate::camera_import::MAX_THUMBNAILS + 20;
    for i in 0..n {
        state.borrow_mut().add(&format!("IMG_{i:04}.JPG"), "2026-10-03T10:00:00", i as u8);
    }
    let mut app = LightcraftApp::new(Session::new(), Services { png: None, cameras: Some(Box::new(Fake(state.clone()))), ..Default::default() });
    let ctx = egui::Context::default();
    let ids: Vec<String> = state.borrow().files.iter().map(|(f, _)| f.id.clone()).collect();
    for id in &ids {
        assert!(crate::camera_import::thumbnail(&mut app, &ctx, "cam", id).is_some(), "{id}");
    }
    let held = app.renderer.textures.keys().filter(|s| matches!(s, crate::render::Slot::Camera(_))).count();
    assert_eq!(held, crate::camera_import::MAX_THUMBNAILS);
    let asked = state.borrow().thumbnails;
    assert_eq!(asked, n);
    // the first one was dropped: drawn again, it is asked for again
    assert!(crate::camera_import::thumbnail(&mut app, &ctx, "cam", &ids[0]).is_some());
    assert_eq!(state.borrow().thumbnails, n + 1);
    // the last one is still held: no new request
    assert!(crate::camera_import::thumbnail(&mut app, &ctx, "cam", &ids[n - 1]).is_some());
    assert_eq!(state.borrow().thumbnails, n + 1);
    let _ = std::fs::remove_dir_all(&root);
}
