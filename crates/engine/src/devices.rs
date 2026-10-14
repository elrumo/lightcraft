//! Cameras and memory cards: mounted volumes with a `DCIM` folder (the DCF layout every camera
//! writes). Listed by `library.devices`; the app offers them under File → Import from Device, and
//! the import review copies from them into the library. On Linux that includes cameras and phones
//! that GVfs mounts over PTP or MTP (`$XDG_RUNTIME_DIR/gvfs/gphoto2:host=…`, `mtp:host=…`), whose
//! `DCIM` is one folder down, in a storage folder.
//!
//! Menus are rebuilt every frame (even with their popups closed), so [`devices`] never touches the
//! disk: it serves the last scan and, when that is older than [`MAX_AGE`], starts one background
//! rescan (coalesced; no lock is held while it runs). A changed result calls the [`on_change`]
//! hook, which the UI uses to repaint, so hot-plugged cards still appear. [`devices_now`] scans
//! synchronously for explicit requests (`library.devices`, Add from Device without a path).
//!
//! Cameras that don't mount as a disk (PTP cameras and phones on macOS, every camera and card on
//! iOS) are read by the host itself (ImageCaptureCore); their files are [`CameraFile`]s, not paths,
//! until they are copied off. [`LibraryIndex`] tells which of them the library already has without
//! reading them; [`review`] (`library.cameraFiles`) lays them out for an import review that a host
//! draws itself (the SwiftUI app).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lightcraft_catalog::{Catalog, PhotoId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Device {
    /// The volume's name ("EOS_DIGITAL", "Untitled").
    pub name: String,
    /// Its `DCIM` folder (what gets imported).
    pub path: String,
    /// The volume's mount point.
    pub root: String,
}

/// How long a scan is served before [`devices`] starts another one.
pub const MAX_AGE: Duration = Duration::from_secs(2);

/// Folders whose children are mounted volumes on this platform. `LIGHTCRAFT_DEVICE_ROOTS`
/// (paths joined like `PATH`) replaces them (tests, unusual mounts).
fn mount_parents() -> Vec<std::path::PathBuf> {
    if let Some(v) = std::env::var_os("LIGHTCRAFT_DEVICE_ROOTS") {
        return std::env::split_paths(&v).collect();
    }
    let mut out = Vec::new();
    if cfg!(target_os = "macos") {
        out.push("/Volumes".into());
    } else if cfg!(any(target_os = "linux", target_os = "freebsd")) {
        // Linux desktops mount under /media/$USER or /run/media/$USER; FreeBSD's automounter
        // (automount/autofs, or a desktop's) under /media. Missing folders are skipped.
        if let Ok(user) = std::env::var("USER") {
            out.push(format!("/media/{user}").into());
            out.push(format!("/run/media/{user}").into());
        }
        out.push("/media".into());
        out.push("/mnt".into());
        // cameras (PTP) and phones (MTP) that GNOME's GVfs mounts through FUSE
        if let Some(run) = std::env::var_os("XDG_RUNTIME_DIR") {
            out.push(std::path::PathBuf::from(run).join("gvfs"));
        }
    }
    out
}

/// A GVfs mount of a camera or phone (`gphoto2:host=Canon_Inc._Canon_Digital_Camera`,
/// `mtp:host=Google_Pixel_7_2A1B…`): its `DCIM` is in a storage folder below the root.
fn gvfs_device(root: &std::path::Path) -> bool {
    root.file_name().is_some_and(|n| {
        let n = n.to_string_lossy();
        ["gphoto2:", "mtp:"].iter().any(|p| n.starts_with(p)) && n.contains("host=")
    })
}

/// A GVfs mount's name as the device's: `Canon Inc. Canon Digital Camera`.
fn gvfs_name(mount: &str) -> String {
    let host = mount.split_once("host=").map_or(mount, |(_, h)| h);
    // (MTP hosts are URL-escaped: `%5Busb%3A001%2C004%5D`; such a name has nothing better)
    let host = host.split(',').next().unwrap_or(host).replace('_', " ");
    let name = host.trim();
    if name.is_empty() || name.contains('%') { "Camera".to_string() } else { name.to_string() }
}

/// How many folders below a GVfs mount are searched for its `DCIM` (storages: a phone's internal
/// storage and an SD card, a camera's two card slots).
const MAX_STORAGES: usize = 8;

type Scanner = dyn Fn() -> Vec<Device> + Send + Sync;
type Notify = dyn Fn() + Send + Sync;

#[derive(Default)]
struct WatchState {
    list: Vec<Device>,
    /// When the served list was scanned (`None`: never). No clock (and no threads) on wasm, where
    /// there are no mounted volumes either.
    #[cfg(not(target_arch = "wasm32"))]
    scanned: Option<std::time::Instant>,
    /// A background scan is running.
    refreshing: bool,
}

/// A cached device list, refreshed off the calling thread.
pub struct DeviceWatch {
    scanner: Box<Scanner>,
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    max_age: Duration,
    state: Mutex<WatchState>,
    notify: Mutex<Option<Arc<Notify>>>,
}

impl DeviceWatch {
    pub fn new(scanner: impl Fn() -> Vec<Device> + Send + Sync + 'static, max_age: Duration) -> Arc<Self> {
        Arc::new(DeviceWatch { scanner: Box::new(scanner), max_age, state: Mutex::default(), notify: Mutex::new(None) })
    }

    fn state(&self) -> std::sync::MutexGuard<'_, WatchState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Called (on the scanning thread) whenever a scan changes the list.
    pub fn on_change(&self, f: impl Fn() + Send + Sync + 'static) {
        *self.notify.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(f));
    }

    /// The last scan, immediately; starts a background rescan when it is stale and none is running.
    pub fn get(self: &Arc<Self>) -> Vec<Device> {
        let mut s = self.state();
        #[cfg(not(target_arch = "wasm32"))]
        if !s.refreshing && s.scanned.is_none_or(|t| t.elapsed() >= self.max_age) {
            s.refreshing = true;
            let me = self.clone();
            // the scan runs without the state lock; `get` only ever waits for a list clone
            let spawned = std::thread::Builder::new().name("lightcraft-devices".into()).spawn(move || {
                let list = (me.scanner)();
                me.store(list);
            });
            if spawned.is_err() {
                s.refreshing = false;
            }
        }
        s.list.clone()
    }

    /// Scan now on this thread, and serve the result to later [`DeviceWatch::get`] calls.
    pub fn scan_now(&self) -> Vec<Device> {
        let list = (self.scanner)();
        self.store(list.clone());
        list
    }

    fn store(&self, list: Vec<Device>) {
        let changed = {
            let mut s = self.state();
            s.refreshing = false;
            #[cfg(not(target_arch = "wasm32"))]
            {
                s.scanned = Some(std::time::Instant::now());
            }
            let changed = s.list != list;
            s.list = list;
            changed
        };
        if changed {
            let f = self.notify.lock().unwrap_or_else(|e| e.into_inner()).clone();
            if let Some(f) = f {
                f();
            }
        }
    }

    /// A background scan is running.
    pub fn refreshing(&self) -> bool {
        self.state().refreshing
    }
}

fn global() -> &'static Arc<DeviceWatch> {
    static WATCH: std::sync::OnceLock<Arc<DeviceWatch>> = std::sync::OnceLock::new();
    WATCH.get_or_init(|| DeviceWatch::new(scan, MAX_AGE))
}

/// Mounted volumes that look like a camera or a memory card, without blocking: the last scan,
/// refreshed in the background when older than [`MAX_AGE`] (empty until the first scan lands).
pub fn devices() -> Vec<Device> {
    global().get()
}

/// Scan the mounted volumes now (blocking), for explicit requests.
pub fn devices_now() -> Vec<Device> {
    global().scan_now()
}

/// Run `f` whenever a scan changes the device list (the UI requests a repaint).
pub fn on_change(f: impl Fn() + Send + Sync + 'static) {
    global().on_change(f);
}

fn scan() -> Vec<Device> {
    let mut extra = Vec::new();
    if cfg!(windows) && std::env::var_os("LIGHTCRAFT_DEVICE_ROOTS").is_none() {
        extra.extend((b'D'..=b'Z').map(|c| std::path::PathBuf::from(format!("{}:\\", c as char))));
    }
    devices_in(&mount_parents(), extra)
}

/// Devices among the children of `parents`, plus the volume roots `extra`.
pub fn devices_in(parents: &[std::path::PathBuf], extra: Vec<std::path::PathBuf>) -> Vec<Device> {
    let mut roots: Vec<std::path::PathBuf> = Vec::new();
    for parent in parents {
        if let Ok(rd) = std::fs::read_dir(parent) {
            let mut v: Vec<_> = rd.flatten().map(|e| e.path()).collect();
            v.sort();
            // the startup disk shows up in /Volumes as a symlink to /
            roots.extend(v.into_iter().filter(|p| !p.is_symlink()));
        }
    }
    roots.extend(extra);
    let mut out: Vec<Device> = Vec::new();
    for root in roots {
        let name = root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| root.to_string_lossy().to_string());
        let found: Vec<(String, std::path::PathBuf)> = if gvfs_device(&root) {
            let device = gvfs_name(&name);
            let storages = gvfs_storages(&root);
            let several = storages.len() > 1;
            storages.into_iter().map(|(storage, dcim)| (if several { format!("{device} — {storage}") } else { device.clone() }, dcim)).collect()
        } else {
            dcim_in(&root).map(|d| (name, d)).into_iter().collect()
        };
        for (name, dcim) in found {
            let path = dcim.to_string_lossy().to_string();
            if !out.iter().any(|d| d.path == path) {
                out.push(Device { name, path, root: root.to_string_lossy().to_string() });
            }
        }
    }
    out
}

/// The storages of a GVfs camera or phone that have a `DCIM` folder: (storage name, its `DCIM`).
fn gvfs_storages(root: &std::path::Path) -> Vec<(String, std::path::PathBuf)> {
    let Ok(rd) = std::fs::read_dir(root) else { return Vec::new() };
    let mut dirs: Vec<std::path::PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).take(MAX_STORAGES).collect();
    dirs.sort();
    dirs.into_iter()
        .filter_map(|d| {
            let dcim = dcim_in(&d)?;
            Some((d.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(), dcim))
        })
        .collect()
}

/// The volume's `DCIM` folder: the usual spellings are checked directly, so a volume's root is
/// only listed (other spellings on a case-sensitive volume) off Windows, whose drives aren't.
pub fn dcim_in(root: &std::path::Path) -> Option<std::path::PathBuf> {
    if let Some(p) = ["DCIM", "dcim"].iter().map(|n| root.join(n)).find(|p| p.is_dir()) {
        return Some(p);
    }
    if cfg!(windows) {
        return None;
    }
    std::fs::read_dir(root).ok()?.flatten().map(|e| e.path()).find(|p| p.file_name().is_some_and(|n| n.eq_ignore_ascii_case("DCIM")) && p.is_dir())
}

/// A file on a camera, phone or card that the host reads itself (iOS and macOS: ImageCaptureCore).
/// It has no path: it is copied off the device only when it is imported.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CameraFile {
    /// The host's handle for it, unique on its device.
    pub id: String,
    /// Its name on the device (`IMG_0042.CR3`).
    pub name: String,
    /// Bytes.
    pub size: u64,
    /// When it was taken, as the library keeps capture times: local time, ISO 8601
    /// (`2026-10-03T14:22:05`).
    pub captured: Option<String>,
    /// `captured` is the date in the file's metadata (EXIF), not the device's date for the file.
    pub exif_dated: bool,
    /// A raw file (the device says so, or its extension does).
    pub raw: bool,
    /// Pixels, when the device knows (0 otherwise).
    pub width: u32,
    pub height: u32,
}

impl CameraFile {
    /// The day it was taken (`2026-10-03`), if known.
    pub fn day(&self) -> Option<&str> {
        capture_day(self.captured.as_deref())
    }
}

/// The `YYYY-MM-DD` of an ISO 8601 capture time.
pub fn capture_day(captured: Option<&str>) -> Option<&str> {
    let d = captured?.get(..10)?;
    let b = d.as_bytes();
    let digits = |r: std::ops::Range<usize>| b.get(r).is_some_and(|s| s.iter().all(u8::is_ascii_digit));
    (digits(0..4) && b.get(4) == Some(&b'-') && digits(5..7) && b.get(7) == Some(&b'-') && digits(8..10)).then_some(d)
}

/// A capture time down to the second, `T` between date and time (`2026-10-03T14:22:05`), for
/// comparing times written slightly differently (a space instead of `T`, fractions, a zone).
fn capture_second(captured: Option<&str>) -> Option<String> {
    let s = captured?.trim();
    let head = s.get(..19)?;
    capture_day(Some(head))?;
    Some(head.replacen(' ', "T", 1))
}

/// `img_0042.cr3` for `IMG_0042.CR3`, and the name import gives a copy whose name was taken in the
/// destination folder (`IMG_0042-1.CR3`) for the same original.
fn name_keys(name: &str) -> Vec<String> {
    let lower = name.trim().to_lowercase();
    let mut keys = vec![lower.clone()];
    let (stem, ext) = match lower.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s, format!(".{e}")),
        _ => (lower.as_str(), String::new()),
    };
    if let Some((base, n)) = stem.rsplit_once('-')
        && !base.is_empty()
        && !n.is_empty()
        && n.bytes().all(|b| b.is_ascii_digit())
    {
        keys.push(format!("{base}{ext}"));
    }
    keys
}

/// The library's photos keyed to spot a camera file it already has without reading the file
/// (which may be on a camera at USB 2 speed): the same size and the same name (a copy renamed
/// `IMG_0042-1.CR3` because the name was taken counts as `IMG_0042.CR3`), and the same capture time
/// to the second when both sides know it (the camera's from the file's EXIF date). Photos in
/// Recently Deleted don't count: importing the
/// file again brings them back (the import's `onDeleted`). A miss costs little: the import still
/// recognises a file whose bytes the library has, and skips it.
#[derive(Debug, Default)]
pub struct LibraryIndex {
    by_key: HashMap<(String, u64), Vec<(PhotoId, Option<String>)>>,
}

impl LibraryIndex {
    pub fn new(catalog: &Catalog) -> Self {
        let mut by_key: HashMap<(String, u64), Vec<(PhotoId, Option<String>)>> = HashMap::new();
        for p in catalog.photos() {
            if p.deleted || p.local || p.copy_of.is_some() || p.file_size == 0 {
                continue;
            }
            for key in name_keys(&p.file_name) {
                by_key.entry((key, p.file_size)).or_default().push((p.id, capture_second(p.captured.as_deref())));
            }
        }
        LibraryIndex { by_key }
    }

    /// The library photo `file` is, if it has it.
    pub fn find(&self, file: &CameraFile) -> Option<PhotoId> {
        // Only a date from the file's metadata is the one the library keeps: without one, import
        // dates the photo by its copy's file date, which is when it was copied.
        let when = capture_second(file.captured.as_deref().filter(|_| file.exif_dated));
        let key = (file.name.trim().to_lowercase(), file.size);
        self.by_key.get(&key)?.iter().find(|(_, t)| t.is_none() || when.is_none() || *t == when).map(|(id, _)| *id)
    }
}

/// A camera file as the import review shows it: its day, and the library photo it already is.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewFile {
    #[serde(flatten)]
    pub file: CameraFile,
    /// The day it was taken (`2026-10-03`), if known.
    pub day: Option<String>,
    /// The library photo it is (not imported again), if any.
    pub existing: Option<u64>,
}

/// A camera's files for its import review: the formats the library takes, newest day first
/// (undated last; within a day the camera's own order, as it numbers files as it takes them), each
/// with the library photo it already is.
pub fn review(catalog: &Catalog, files: Vec<CameraFile>) -> Vec<ReviewFile> {
    let index = LibraryIndex::new(catalog);
    let mut out: Vec<ReviewFile> = files
        .into_iter()
        .filter(|f| crate::import::is_supported(std::path::Path::new(&f.name)))
        .map(|f| ReviewFile { day: f.day().map(str::to_string), existing: index.find(&f).map(|p| p.0), file: f })
        .collect();
    out.sort_by(|a, b| match (&a.day, &b.day) {
        (Some(x), Some(y)) => y.cmp(x),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // A camera's files reviewed through the engine command: unsupported formats left out, newest day
    // first (undated last, the camera's order kept within a day), and the one the library has marked
    #[test]
    fn camera_files_are_reviewed_by_day_with_the_library_ones_marked() {
        let mut s = crate::Session::with_demo();
        let id = s.active().unwrap();
        let p = s.catalog.photo(id).unwrap();
        let (name, size) = (p.file_name.clone(), p.file_size);
        let files = serde_json::json!([
            {"id": "a", "name": "IMG_0001.JPG", "size": 10, "captured": "2026-09-01T10:00:00", "exifDated": true},
            {"id": "b", "name": "notes.txt", "size": 3},
            {"id": "c", "name": "IMG_0002.CR3", "size": 20, "raw": true},
            {"id": "d", "name": "IMG_0003.JPG", "size": 30, "captured": "2026-10-03T09:00:00", "exifDated": true},
            {"id": "e", "name": "IMG_0004.JPG", "size": 40, "captured": "2026-10-03T08:00:00", "exifDated": true},
            {"id": "f", "name": name.to_uppercase(), "size": size},
        ]);
        let r = s.execute("library.cameraFiles", &serde_json::json!({"files": files})).unwrap();
        let ids: Vec<&str> = r["files"].as_array().unwrap().iter().map(|f| f["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["d", "e", "a", "c", "f"], "{r}");
        assert_eq!(r["files"][0]["day"], "2026-10-03");
        assert!(r["files"][3]["day"].is_null() && r["files"][3]["raw"] == true);
        assert_eq!(r["files"][4]["existing"], id.0, "the library's own photo, found by name and size");
        assert!(r["files"][0]["existing"].is_null());
        assert!(s.execute("library.cameraFiles", &serde_json::json!({"files": "nope"})).is_err());
    }

    #[test]
    fn volumes_with_a_dcim_folder_are_devices() {
        let base = std::env::temp_dir().join(format!("lc-devices-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("EOS_DIGITAL/DCIM/100CANON")).unwrap();
        std::fs::create_dir_all(base.join("Backup/Photos")).unwrap();
        std::fs::create_dir_all(base.join("SD/dcim")).unwrap();
        std::fs::create_dir_all(base.join("X/Dcim")).unwrap();
        let d = devices_in(std::slice::from_ref(&base), Vec::new());
        let names: Vec<&str> = d.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, ["EOS_DIGITAL", "SD", "X"], "{d:?}");
        // the DCIM folder itself, in a spelling that opens on that volume
        for x in &d {
            let p = std::path::Path::new(&x.path);
            assert!(p.is_dir() && p.file_name().is_some_and(|n| n.eq_ignore_ascii_case("DCIM")), "{x:?}");
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    /// GVfs mounts of a camera (PTP) and a phone (MTP, two storages): each storage with a DCIM
    /// is a device, named after the camera, its DCIM one folder down.
    #[test]
    fn gvfs_cameras_and_phones_are_devices() {
        let base = std::env::temp_dir().join(format!("lc-devices-gvfs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("gphoto2:host=Canon_Inc._Canon_Digital_Camera/store_00010001/DCIM/100CANON")).unwrap();
        std::fs::create_dir_all(base.join("mtp:host=Google_Pixel_7_2A1B/Internal shared storage/DCIM/Camera")).unwrap();
        std::fs::create_dir_all(base.join("mtp:host=Google_Pixel_7_2A1B/SD card/DCIM")).unwrap();
        std::fs::create_dir_all(base.join("mtp:host=Google_Pixel_7_2A1B/SD card/Music")).unwrap();
        std::fs::create_dir_all(base.join("smb-share:server=nas,share=photos/DCIM")).unwrap();
        let d = devices_in(std::slice::from_ref(&base), Vec::new());
        let names: Vec<&str> = d.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Canon Inc. Canon Digital Camera",
                "Google Pixel 7 2A1B — Internal shared storage",
                "Google Pixel 7 2A1B — SD card",
                "smb-share:server=nas,share=photos"
            ],
            "{d:?}"
        );
        assert!(d[0].path.ends_with("store_00010001/DCIM"), "{:?}", d[0]);
        assert_eq!(gvfs_name("mtp:host=%5Busb%3A001%2C004%5D"), "Camera");
        let _ = std::fs::remove_dir_all(&base);
    }

    fn photo(id: u64, name: &str, size: u64, captured: Option<&str>) -> lightcraft_catalog::Photo {
        let mut p = lightcraft_catalog::Photo::new(
            PhotoId(id),
            lightcraft_catalog::Source::File { path: format!("/lib/{name}") },
            name,
            "CR3",
            6000,
            4000,
            "2026-10-03",
        );
        p.file_size = size;
        p.captured = captured.map(str::to_string);
        p
    }

    fn file(name: &str, size: u64, captured: Option<&str>) -> CameraFile {
        CameraFile {
            id: name.into(),
            name: name.into(),
            size,
            captured: captured.map(str::to_string),
            exif_dated: captured.is_some(),
            ..Default::default()
        }
    }

    /// Camera files the library has are found by name and size (a copy renamed `-1` counts), the
    /// capture time breaking ties; photos in Recently Deleted and Local don't count.
    #[test]
    fn camera_files_already_in_the_library_are_found() {
        let mut c = Catalog::new();
        let mut add = |p: lightcraft_catalog::Photo| {
            c.apply(lightcraft_catalog::Op::AddPhoto { photo: Box::new(p) }).unwrap();
        };
        add(photo(1, "IMG_0001.CR3", 30_000_000, Some("2026-10-03T14:22:05")));
        add(photo(2, "IMG_0002-1.CR3", 31_000_000, Some("2026-10-03 14:22:07.120")));
        add(photo(3, "DSC-7.JPG", 4_000_000, None));
        let mut gone = photo(4, "IMG_0004.CR3", 32_000_000, None);
        gone.deleted = true;
        add(gone);
        let mut local = photo(5, "IMG_0005.CR3", 33_000_000, None);
        local.local = true;
        add(local);
        let ix = LibraryIndex::new(&c);
        assert_eq!(ix.find(&file("IMG_0001.CR3", 30_000_000, Some("2026-10-03T14:22:05"))), Some(PhotoId(1)));
        assert_eq!(ix.find(&file("img_0001.cr3", 30_000_000, None)), Some(PhotoId(1)), "case and an unknown time");
        assert_eq!(ix.find(&file("IMG_0001.CR3", 30_000_001, None)), None, "another size");
        assert_eq!(ix.find(&file("IMG_0001.CR3", 30_000_000, Some("2026-10-03T14:22:06"))), None, "another second");
        // a file without an EXIF date: the camera gives its file date, the library the copy's
        let undated = CameraFile { exif_dated: false, ..file("IMG_0001.CR3", 30_000_000, Some("2026-10-01T08:00:00")) };
        assert_eq!(ix.find(&undated), Some(PhotoId(1)), "a file date is not compared");
        assert_eq!(ix.find(&file("IMG_0002.CR3", 31_000_000, Some("2026-10-03T14:22:07"))), Some(PhotoId(2)), "imported as -1");
        assert_eq!(ix.find(&file("DSC-7.JPG", 4_000_000, Some("2025-01-01T00:00:00"))), Some(PhotoId(3)), "a name with a dash");
        assert_eq!(ix.find(&file("IMG_0004.CR3", 32_000_000, None)), None, "in Recently Deleted");
        assert_eq!(ix.find(&file("IMG_0005.CR3", 33_000_000, None)), None, "Local, not in the library");
        assert_eq!(file("a", 1, Some("2026-08-03T10:00:00")).day(), Some("2026-08-03"));
        assert_eq!(file("a", 1, Some("garbage")).day(), None);
        assert_eq!(file("a", 1, Some("2026-ä")).day(), None, "not cut inside a character");
    }

    fn card(name: &str) -> Device {
        Device { name: name.into(), path: format!("/v/{name}/DCIM"), root: format!("/v/{name}") }
    }

    /// Wait (bounded) for `f`.
    fn until(f: impl Fn() -> bool) -> bool {
        let t = std::time::Instant::now();
        while t.elapsed() < Duration::from_secs(20) {
            if f() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        false
    }

    #[test]
    fn slow_scans_never_block_and_hot_plug_still_arrives() {
        // a scanner that takes 1.5 s (a sleeping network drive) and counts its runs
        const SLOW: Duration = Duration::from_millis(1500);
        let mounted = Arc::new(Mutex::new(Vec::<Device>::new()));
        let runs = Arc::new(AtomicUsize::new(0));
        let (m, r) = (mounted.clone(), runs.clone());
        let watch = DeviceWatch::new(
            move || {
                r.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(SLOW);
                m.lock().unwrap().clone()
            },
            Duration::from_millis(20),
        );
        let changes = Arc::new(AtomicUsize::new(0));
        let c = changes.clone();
        watch.on_change(move || {
            c.fetch_add(1, Ordering::SeqCst);
        });
        // cold: served at once (empty) while one scan runs in the background
        let t = std::time::Instant::now();
        for _ in 0..50 {
            assert!(watch.get().is_empty());
        }
        assert!(t.elapsed() < SLOW / 2, "get() waited for the scan: {:?}", t.elapsed());
        assert!(watch.refreshing());
        assert!(until(|| !watch.refreshing()));
        assert_eq!(runs.load(Ordering::SeqCst), 1, "requests while scanning are coalesced");
        assert_eq!(changes.load(Ordering::SeqCst), 0, "nothing changed");
        // a card is plugged in: the next stale request starts a rescan, still without waiting
        mounted.lock().unwrap().push(card("EOS_DIGITAL"));
        std::thread::sleep(Duration::from_millis(40));
        let t = std::time::Instant::now();
        assert!(watch.get().is_empty(), "the old list is served while rescanning");
        assert!(t.elapsed() < SLOW / 2, "warm get() waited: {:?}", t.elapsed());
        assert!(until(|| changes.load(Ordering::SeqCst) == 1), "a changed scan notifies");
        assert_eq!(watch.get(), [card("EOS_DIGITAL")]);
        // explicit requests scan synchronously
        assert!(until(|| !watch.refreshing()));
        mounted.lock().unwrap().clear();
        assert!(watch.scan_now().is_empty());
        assert_eq!(changes.load(Ordering::SeqCst), 2);
    }
}
