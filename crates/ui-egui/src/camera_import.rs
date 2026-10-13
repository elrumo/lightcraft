//! Importing from a camera, phone or memory card that the host reads itself ([`CameraHost`],
//! `Services::cameras`; iOS: everything plugged into the iPhone or iPad, macOS: cameras in PTP mode
//! and iPhones, both through ImageCaptureCore), and opening the import when a camera or card is
//! connected (Settings ▸ Import, on by default as Lightroom Classic's "Show import dialog when a
//! memory card is detected"), for these and for cards mounted as disks alike.
//!
//! A camera's files fill the import review as the camera catalogues them (two thousand photos take
//! a few seconds), grouped by the day they were taken, with thumbnails the camera makes. Those the
//! library already has (same name and size, and capture time: `devices::LibraryIndex`) are greyed
//! out and can't be chosen. Import copies the chosen files off the device into a staging folder of
//! the host's (a row in the activity stack, with ✕), then imports the copies as the host's pickers'
//! are imported: moved into the library's Originals/, with the review's album, preset and keywords.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};

use lightcraft_engine::devices::{CameraFile, LibraryIndex};
use lightcraft_engine::import::ImportCandidate;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::import::ImportDialog;
use crate::render::Slot;

/// What a host camera is (for its wording).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CameraKind {
    #[default]
    Camera,
    /// An iPhone or iPad.
    Phone,
    /// A memory card in a reader (or a camera in mass-storage mode).
    Card,
}

/// A camera, phone or card connected to the host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct HostCamera {
    /// Stable while it stays connected.
    pub id: String,
    pub name: String,
    pub kind: CameraKind,
}

/// What an opened host camera holds, as far as it has catalogued.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HostContents {
    pub files: Vec<CameraFile>,
    /// Catalogued completely.
    pub complete: bool,
    /// Why it can't be read (no permission, unplugged…).
    pub error: Option<String>,
}

/// Progress of a copy off a host camera, shared with the host: `done` of `total` files finished;
/// `cancel` stops it before the next file (the activity row's ✕).
#[derive(Debug, Default)]
pub struct HostTransfer {
    pub total: AtomicUsize,
    pub done: Arc<AtomicUsize>,
    pub cancel: Arc<AtomicBool>,
}

/// How a copy off a host camera ended.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HostCopied {
    /// The copies (paths in the folder the copy was given).
    pub files: Vec<String>,
    /// One message per file that couldn't be copied.
    pub failed: Vec<String>,
    pub cancelled: bool,
}

/// Called once, on any thread, when a copy has ended.
pub type CopyDone = Box<dyn FnOnce(HostCopied) + Send + 'static>;

/// Cameras, phones and cards the host reads itself (iOS and macOS: `lightcraft-capture`). Every
/// call is made on the UI thread.
pub trait CameraHost {
    /// Connected now.
    fn cameras(&self) -> Vec<HostCamera>;
    /// Start reading what is on `camera` (the first time on iOS, the system asks the user).
    fn open(&self, camera: &str) -> Result<(), String>;
    /// What is on an opened camera, when it changed since revision `seen`: the new revision and the
    /// contents (`None`: unchanged, or not opened).
    fn contents(&self, camera: &str, seen: u64) -> Option<(u64, HostContents)>;
    /// A thumbnail of `file` (encoded, a JPEG): `None` until it has arrived (keep asking, a frame at
    /// a time), then once.
    fn thumbnail(&self, camera: &str, file: &str) -> Option<Result<Vec<u8>, String>>;
    /// The folder copies are made in before they are imported (each copy gets a subfolder).
    fn staging(&self) -> PathBuf;
    /// Copy `files` off `camera` into `dir`; `done` is called once when it has ended.
    fn download(&self, camera: &str, files: &[String], dir: &Path, transfer: Arc<HostTransfer>, done: CopyDone) -> Result<(), String>;
    /// Stop reading `camera`.
    fn close(&self, camera: &str);
}

/// The cameras, phones and cards the system reads (iOS and macOS: ImageCaptureCore, through
/// `lightcraft-capture`), watched from now on, with a repaint whenever one comes or goes or sends
/// something: the host's `Services::cameras`. Call on the main thread. `None` elsewhere, or when
/// watching can't start. `staging` is where copies are made before they are imported.
pub fn system_cameras(ctx: &egui::Context, staging: PathBuf) -> Option<Box<dyn CameraHost>> {
    #[cfg(any(target_os = "ios", target_os = "macos"))]
    {
        let wake = ctx.clone();
        match lightcraft_capture::watch(Arc::new(move || wake.request_repaint())) {
            Ok(()) => Some(Box::new(system::Cameras { staging })),
            Err(e) => {
                log::warn!("cameras: {e}");
                None
            }
        }
    }
    #[cfg(not(any(target_os = "ios", target_os = "macos")))]
    {
        let _ = (ctx, staging);
        None
    }
}

/// [`CameraHost`] over `lightcraft-capture`.
#[cfg(any(target_os = "ios", target_os = "macos"))]
mod system {
    use super::*;

    pub struct Cameras {
        pub staging: PathBuf,
    }

    fn kind(k: lightcraft_capture::Kind) -> CameraKind {
        match k {
            lightcraft_capture::Kind::Camera => CameraKind::Camera,
            lightcraft_capture::Kind::Phone => CameraKind::Phone,
            lightcraft_capture::Kind::Card => CameraKind::Card,
        }
    }

    impl CameraHost for Cameras {
        fn cameras(&self) -> Vec<HostCamera> {
            lightcraft_capture::cameras().into_iter().map(|c| HostCamera { id: c.id, name: c.name, kind: kind(c.kind) }).collect()
        }
        fn open(&self, camera: &str) -> Result<(), String> {
            lightcraft_capture::open(camera)
        }
        fn contents(&self, camera: &str, seen: u64) -> Option<(u64, HostContents)> {
            let (rev, c) = lightcraft_capture::contents(camera, seen)?;
            let files = c
                .items
                .into_iter()
                .map(|i| CameraFile { id: i.id, name: i.name, size: i.size, captured: i.captured, raw: i.raw, width: i.width, height: i.height })
                .collect();
            Some((rev, HostContents { files, complete: c.complete, error: c.error }))
        }
        fn thumbnail(&self, camera: &str, file: &str) -> Option<Result<Vec<u8>, String>> {
            lightcraft_capture::thumbnail(camera, file)
        }
        fn staging(&self) -> PathBuf {
            self.staging.clone()
        }
        fn download(&self, camera: &str, files: &[String], dir: &Path, transfer: Arc<HostTransfer>, done: CopyDone) -> Result<(), String> {
            let t = Arc::new(lightcraft_capture::Transfer {
                total: AtomicUsize::new(files.len()),
                done: transfer.done.clone(),
                cancel: transfer.cancel.clone(),
            });
            lightcraft_capture::download(
                camera,
                files,
                dir,
                t,
                Box::new(move |c| {
                    done(HostCopied {
                        files: c.files.iter().map(|f| f.to_string_lossy().to_string()).collect(),
                        failed: c.failed,
                        cancelled: c.cancelled,
                    })
                }),
            )
        }
        fn close(&self, camera: &str) {
            lightcraft_capture::close(camera);
        }
    }
}

/// The import review's camera, when the photos are on one ([`ImportDialog::camera`]).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CameraSource {
    pub id: String,
    pub name: String,
    pub kind: CameraKind,
    /// The camera has catalogued all its files.
    pub complete: bool,
    /// Why it can't be read.
    pub error: Option<String>,
    /// The host's revision of the contents last read.
    #[serde(skip)]
    pub seen: u64,
}

/// A copy off a camera in progress; its files are imported when it ends.
pub struct CameraCopy {
    /// The review that started it (album, preset, keywords… for the import).
    dialog: ImportDialog,
    name: String,
    dir: PathBuf,
    transfer: Arc<HostTransfer>,
    rx: mpsc::Receiver<HostCopied>,
    guard: lightcraft_engine::activity::TaskGuard,
}

impl CameraCopy {
    /// `{done, total}` for `ui.inspect`.
    pub fn status(&self) -> Value {
        json!({"done": self.transfer.done.load(Ordering::Relaxed), "total": self.transfer.total.load(Ordering::Relaxed), "camera": self.name})
    }
}

/// The connected cameras the host reads itself (none without a host that does).
pub fn cameras(app: &LightcraftApp) -> Vec<HostCamera> {
    app.services.cameras.as_ref().map(|h| h.cameras()).unwrap_or_default()
}

/// Open the import review for host camera `id`: File ▸ Import from Device ▸ the camera, the
/// phone's add button ▸ Connected Camera/Card, or a camera just connected.
pub fn open(app: &mut LightcraftApp, id: &str) -> Result<Value, String> {
    let Some(host) = app.services.cameras.as_ref() else { return Err("this app can't read cameras".into()) };
    let Some(camera) = host.cameras().into_iter().find(|c| c.id == id) else { return Err("that camera or card isn't connected".into()) };
    if app.import.is_some() || app.scan.is_some() || app.camera_copy.is_some() {
        return Err("an import is running".into());
    }
    if !app.session.library.as_ref().is_some_and(|l| l.on_disk) {
        return Err("photos can't be imported: the library isn't saved on this device".into());
    }
    host.open(id)?;
    app.renderer.forget_imports();
    app.camera_thumbs.clear();
    app.camera_thumbs_failed.clear();
    let mut d = ImportDialog::new(Vec::new());
    d.camera = Some(CameraSource { id: camera.id.clone(), name: camera.name.clone(), kind: camera.kind, ..Default::default() });
    d.device = camera.name.clone();
    // the copies are the app's own, in its staging folder: moved into the library
    d.staged = true;
    d.copy = true;
    d.move_files = true;
    refresh(app, &mut d);
    app.ui.dialog = Some(crate::state::Dialog::Import { opts: Box::new(d) });
    Ok(json!({"camera": camera.id, "name": camera.name}))
}

/// Bring a camera review's files up to date with what the camera has catalogued (each frame; only
/// a change costs anything).
pub fn refresh(app: &mut LightcraftApp, d: &mut ImportDialog) {
    let Some(src) = d.camera.as_mut() else { return };
    let Some(host) = app.services.cameras.as_ref() else { return };
    let Some((rev, contents)) = host.contents(&src.id, src.seen) else { return };
    src.seen = rev;
    src.complete = contents.complete;
    src.error = contents.error.clone();
    let index = app.caches.library_index(&app.session.catalog);
    d.set_camera_files(contents.files, |f| index.find(f).map(|p| p.0));
}

/// The camera file as an import candidate: its id for a path; the library photo it is, if any.
pub fn candidate(f: CameraFile, existing: Option<u64>) -> ImportCandidate {
    let format = Path::new(&f.name).extension().map(|e| e.to_string_lossy().to_uppercase()).unwrap_or_default();
    ImportCandidate {
        path: f.id,
        name: f.name,
        format,
        width: f.width,
        height: f.height,
        file_size: f.size,
        captured: f.captured,
        duplicate: existing.map(|_| "library".to_string()),
        existing,
        ..Default::default()
    }
}

/// Camera thumbnails kept as textures at once (the ones scrolled away longest go first, and are
/// asked of the camera again when they come back into view): a card of two thousand photos must
/// not hold them all in a phone's memory.
pub const MAX_THUMBNAILS: usize = 240;
/// Long edge of a camera thumbnail's texture (pixels): a phone's three-across cell at 2×.
const THUMBNAIL_EDGE: u32 = 256;

/// A camera file's thumbnail as a texture (asked of the host the first time it is in view).
pub fn thumbnail(app: &mut LightcraftApp, ctx: &egui::Context, camera: &str, file: &str) -> Option<(egui::TextureId, [usize; 2])> {
    let slot = Slot::Camera(lightcraft_preview::Hasher128::new().str(camera).str(file).finish().0 as u64);
    if let Some(t) = app.renderer.textures.get(&slot) {
        let found = (t.tex.id(), t.size);
        // (most recently drawn last)
        if app.camera_thumbs.back() != Some(&slot) {
            app.camera_thumbs.retain(|s| *s != slot);
            app.camera_thumbs.push_back(slot);
        }
        return Some(found);
    }
    if app.camera_thumbs_failed.contains(&slot) {
        return None;
    }
    let got = app.services.cameras.as_ref()?.thumbnail(camera, file)?;
    let img = got.and_then(|bytes| lightcraft_codecs::decode_thumbnail(&bytes, THUMBNAIL_EDGE).map_err(|e| e.to_string()));
    match img {
        Ok(t) => {
            let img = t.image.into_oriented(lightcraft_geom::Orientation::from_exif(t.orientation));
            app.renderer.put_image(ctx, slot, &img);
            app.camera_thumbs.push_back(slot);
            while app.camera_thumbs.len() > MAX_THUMBNAILS {
                if let Some(old) = app.camera_thumbs.pop_front() {
                    app.renderer.textures.remove(&old);
                }
            }
            app.renderer.textures.get(&slot).map(|t| (t.tex.id(), t.size))
        }
        Err(e) => {
            log::debug!("camera thumbnail {file}: {e}");
            app.camera_thumbs_failed.insert(slot);
            None
        }
    }
}

/// A new, empty folder under `staging` for one copy (`camera-1`, `camera-2`…).
fn batch_dir(staging: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(staging).map_err(|e| format!("{}: {e}", staging.display()))?;
    for i in 1..10_000u32 {
        let dir = staging.join(format!("camera-{i}"));
        match std::fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("{}: {e}", dir.display())),
        }
    }
    Err(format!("{}: no free folder for the copies", staging.display()))
}

/// The review's Import on a camera: copy the checked files off it, then import them.
pub fn start(app: &mut LightcraftApp, d: &ImportDialog) -> Result<Value, String> {
    crate::import::busy_synchronizing(app)?;
    let Some(src) = d.camera.as_ref() else { return Err("not a camera".into()) };
    if app.camera_copy.is_some() || app.import.is_some() {
        return Err("an import is running".into());
    }
    let files = d.selected_paths();
    if files.is_empty() {
        return Err("no photos selected".into());
    }
    let Some(host) = app.services.cameras.as_ref() else { return Err("this app can't read cameras".into()) };
    let dir = batch_dir(&host.staging())?;
    let transfer = Arc::new(HostTransfer::default());
    transfer.total.store(files.len(), Ordering::Relaxed);
    let (tx, rx) = mpsc::channel();
    let started = host.download(
        &src.id,
        &files,
        &dir,
        transfer.clone(),
        Box::new(move |c| {
            let _ = tx.send(c);
        }),
    );
    if let Err(e) = started {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(e);
    }
    let guard = app.session.activity.start("import", "Copying from camera", lightcraft_engine::activity::Cancel::Flag(transfer.cancel.clone()));
    guard.detail(&src.name);
    app.camera_copy = Some(CameraCopy { dialog: d.clone(), name: src.name.clone(), dir, transfer, rx, guard });
    app.renderer.forget_imports();
    Ok(json!({"copying": files.len()}))
}

/// Advance a copy off a camera (each frame), and open the import when a camera or card is
/// connected.
pub fn poll(app: &mut LightcraftApp, ctx: &egui::Context) {
    connections(app, ctx);
    let Some(task) = app.camera_copy.as_ref() else { return };
    ctx.request_repaint_after(std::time::Duration::from_millis(100));
    let copied = match task.rx.try_recv() {
        Ok(c) => c,
        Err(mpsc::TryRecvError::Empty) => {
            let (done, total) = (task.transfer.done.load(Ordering::Relaxed), task.transfer.total.load(Ordering::Relaxed));
            task.guard.progress(done as u64, total as u64);
            return;
        }
        Err(mpsc::TryRecvError::Disconnected) => HostCopied { failed: vec!["the copy stopped".into()], ..Default::default() },
    };
    let Some(task) = app.camera_copy.take() else { return };
    let failed = copied.failed.len();
    for f in &copied.failed {
        log::warn!("copying from {}: {f}", task.name);
    }
    if copied.files.is_empty() {
        let _ = std::fs::remove_dir_all(&task.dir);
        let msg = if copied.cancelled {
            crate::i18n::tr("Import cancelled").to_string()
        } else {
            crate::i18n::tr_format!("No photos could be copied from {name}", name = task.name)
        };
        app.toast(ctx, msg);
        return;
    }
    // (the new album the review names is created now: one undo step with the import)
    let undo0 = app.session.undo.len();
    let started = crate::import::import_params(app, &task.dialog).map(|params| {
        let mut t = crate::import::ImportTask::new(copied.files, params, undo0, false);
        t.cleanup = Some(task.dir.clone());
        app.import = Some(t);
    });
    if let Err(e) = started {
        let _ = std::fs::remove_dir_all(&task.dir);
        app.toast(ctx, crate::i18n::tr_format!("Import failed: {e}", e = e));
        return;
    }
    if failed > 0 {
        app.toast(ctx, crate::i18n::tr_format!("Photos not copied from {name}: {n}", name = task.name, n = failed));
    }
}

/// What can be connected: a mounted card (`file.addFromDevice {path}`) or a host camera.
#[derive(Clone, Debug, PartialEq)]
enum Plugged {
    Mounted(String),
    Camera(String),
}

/// How long after the app starts looking (s) what is found counts as already there: devices
/// connected before the app started are reported in its first moments, and don't open the import.
const CONNECT_GRACE: f64 = 3.0;

/// Open the import for a camera or card just connected (the setting on, nothing else open or
/// running); otherwise a toast says it is there.
fn connections(app: &mut LightcraftApp, ctx: &egui::Context) {
    let mut found: Vec<(String, String, Plugged)> = Vec::new();
    // (only desktops mount cards; elsewhere there is nothing to scan)
    if cfg!(any(target_os = "macos", target_os = "linux", target_os = "freebsd", windows)) {
        found.extend(lightcraft_engine::devices::devices().into_iter().map(|d| (format!("path:{}", d.path), d.name, Plugged::Mounted(d.path))));
    }
    found.extend(cameras(app).into_iter().map(|c| (format!("camera:{}", c.id), c.name, Plugged::Camera(c.id))));
    let now = ctx.input(|i| i.time);
    let keys: HashSet<String> = found.iter().map(|(k, _, _)| k.clone()).collect();
    let Some((since, seen)) = app.devices_seen.as_mut() else {
        app.devices_seen = Some((now, keys));
        return;
    };
    if now - *since < CONNECT_GRACE {
        seen.extend(keys);
        return;
    }
    let new: Vec<(String, Plugged)> = found.into_iter().filter(|(k, _, _)| !seen.contains(k)).map(|(_, n, p)| (n, p)).collect();
    // (one unplugged and plugged in again counts as new)
    *seen = keys;
    let Some((name, plugged)) = new.into_iter().next() else { return };
    let busy = app.ui.dialog.is_some() || app.import.is_some() || app.scan.is_some() || app.camera_copy.is_some();
    if !app.ui.settings.import_on_connect || busy {
        // (where to find it: the phone has no File menu)
        let hint = if app.compact { " · + ▸ Connected Camera/Card" } else { " · File ▸ Import from Device" };
        app.toast(ctx, crate::i18n::tr_format!("{name} connected", name = name) + crate::i18n::tr(hint));
        return;
    }
    let r = match plugged {
        Plugged::Mounted(path) => app.run("file.addFromDevice", json!({"path": path})),
        Plugged::Camera(id) => app.run("file.addFromDevice", json!({"camera": id})),
    };
    if let Err(e) = r {
        log::warn!("{name} connected: {e}");
    }
}

/// The library's photos keyed by name, size and capture time, by library revision.
#[derive(Default)]
pub struct IndexCache(Option<(u64, Arc<LibraryIndex>)>);

impl IndexCache {
    pub fn get(&mut self, cat: &lightcraft_catalog::Catalog) -> Arc<LibraryIndex> {
        match &self.0 {
            Some((r, ix)) if *r == cat.revision => ix.clone(),
            _ => {
                let ix = Arc::new(LibraryIndex::new(cat));
                self.0 = Some((cat.revision, ix.clone()));
                ix
            }
        }
    }
}

/// Which files of a camera review were checked, by file id (kept as more arrive and the list is
/// sorted again).
pub(crate) fn checked_by_id(d: &ImportDialog) -> HashMap<String, bool> {
    d.candidates.iter().zip(&d.checked).map(|(c, on)| (c.path.clone(), *on)).collect()
}
