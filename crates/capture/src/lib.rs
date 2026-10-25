//! Cameras, phones and memory cards through Apple's ImageCaptureCore, on iOS and macOS: which are
//! connected (watched from the moment [`watch`] is called, so a camera plugged in later appears
//! with a call to the app's wake-up), what is on one ([`open`], then [`contents`]: names, sizes,
//! capture times, as the device catalogues them), thumbnails of its photos ([`thumbnail`]) and
//! copying chosen files off it into a folder ([`download`]).
//!
//! On iOS this is how an app reaches a camera or a card reader plugged into the iPhone or iPad
//! (there is no file system path to them), the first time with the system's permission prompt
//! (`NSCameraUsageDescription`). On macOS it reaches what doesn't mount as a disk: cameras in
//! PTP mode and iPhones (memory cards and cameras in mass-storage mode mount under `/Volumes`,
//! where `lightcraft_engine::devices` finds them, so they are left out here).
//!
//! The Objective-C calls go through `objc2` (no C or Objective-C is compiled) and live in `apple`
//! (iOS and macOS only). This crate is allowed `unsafe` for them, as `lightcraft-ios-host` is for
//! UIKit (`CLAUDE.md`, Never crash): every `unsafe` block says why it is sound, the API here is
//! safe, and failures come back as errors. Elsewhere every call says it is unavailable, and the
//! state below (what the delegates record, what the app reads) is what the tests exercise.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError};

#[cfg(any(target_os = "ios", target_os = "macos"))]
#[allow(unsafe_code)]
mod apple;

/// What kind of device it is (for its icon and wording).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Camera,
    /// An iPhone or iPad.
    Phone,
    /// A memory card (in a card reader) or a camera in mass-storage mode.
    Card,
}

/// A connected camera, phone or card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Camera {
    /// Stable while it stays connected.
    pub id: String,
    /// "Canon PowerShot V1", "EOS_DIGITAL".
    pub name: String,
    pub kind: Kind,
}

/// A file on a device.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Item {
    /// Unique on its device while it is open.
    pub id: String,
    pub name: String,
    /// Bytes.
    pub size: u64,
    /// When it was taken: local time, ISO 8601 (`2026-10-03T14:22:05`), from its EXIF date or,
    /// without one, the file's.
    pub captured: Option<String>,
    /// `captured` is the EXIF date (not the file's).
    pub exif_dated: bool,
    pub raw: bool,
    /// Pixels, 0 when the device doesn't say.
    pub width: u32,
    pub height: u32,
}

/// What an open device holds, as far as it has been catalogued.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Contents {
    pub items: Vec<Item>,
    /// The device has finished cataloguing (until then items keep arriving).
    pub complete: bool,
    /// Why it can't be read (no permission, the session failed, unplugged).
    pub error: Option<String>,
}

/// Progress and cancellation of a [`download`]: `total` files, `done` of them finished (copied or
/// failed); setting `cancel` stops it before the next file.
#[derive(Debug, Default)]
pub struct Transfer {
    pub total: AtomicUsize,
    /// (shareable with the app's own progress)
    pub done: Arc<AtomicUsize>,
    pub cancel: Arc<AtomicBool>,
}

/// How a [`download`] ended.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Copied {
    /// The copies, in the folder given.
    pub files: Vec<PathBuf>,
    /// One message per file that couldn't be copied.
    pub failed: Vec<String>,
    /// Stopped by [`Transfer::cancel`] (the files copied before stay).
    pub cancelled: bool,
}

/// Called once, on any thread, when a download has ended.
pub type Done = Box<dyn FnOnce(Copied) + Send + 'static>;

/// Called on any thread whenever something here changed (a device came or went, items or a
/// thumbnail arrived): the app repaints and reads again.
pub type Wake = Arc<dyn Fn() + Send + Sync + 'static>;

/// Thumbnails asked of a device at once; the rest wait until those arrive (a camera answers one
/// at a time, and a grid scrolled fast would queue thousands).
pub const THUMBNAILS_IN_FLIGHT: usize = 6;
/// Long edge asked of thumbnails (pixels): sharp in a phone's three-across grid.
pub const THUMBNAIL_EDGE: u32 = 400;

#[cfg_attr(any(target_os = "ios", target_os = "macos"), allow(dead_code))]
const UNAVAILABLE: &str = "cameras and cards are read this way only on iOS and macOS";

/// Start watching for cameras, phones and cards (call on the main thread; later calls only
/// replace `wake`).
pub fn watch(wake: Wake) -> Result<(), String> {
    state().wake = Some(wake);
    #[cfg(any(target_os = "ios", target_os = "macos"))]
    {
        apple::watch()
    }
    #[cfg(not(any(target_os = "ios", target_os = "macos")))]
    Err(UNAVAILABLE.into())
}

/// The devices connected now (empty before [`watch`]).
pub fn cameras() -> Vec<Camera> {
    state().cameras.clone()
}

/// Start reading what is on `device` (call on the main thread). On iOS the first time asks the
/// user's permission. [`contents`] then fills as the device catalogues its files.
pub fn open(device: &str) -> Result<(), String> {
    {
        let mut s = state();
        if !s.cameras.iter().any(|c| c.id == device) {
            return Err("that camera or card isn't connected any more".into());
        }
        s.thumbs.retain(|(d, _), _| d != device);
        s.in_flight.remove(device);
        let rev = s.next_rev();
        s.contents.insert(device.to_string(), (rev, Contents::default()));
    }
    #[cfg(any(target_os = "ios", target_os = "macos"))]
    {
        apple::open(device)
    }
    #[cfg(not(any(target_os = "ios", target_os = "macos")))]
    Err(UNAVAILABLE.into())
}

/// What is on an opened device, when it changed since revision `seen` (0: always): the new
/// revision and the contents. `None`: unchanged, or not opened.
pub fn contents(device: &str, seen: u64) -> Option<(u64, Contents)> {
    let s = state();
    let (rev, c) = s.contents.get(device)?;
    (*rev != seen).then(|| (*rev, c.clone()))
}

/// A thumbnail of `item` (JPEG bytes): the first calls ask the device for it (at most
/// [`THUMBNAILS_IN_FLIGHT`] at a time: call again on later frames) and return `None`; the call
/// after it arrived returns it and forgets it (the app keeps what it drew; asking again later, once
/// the app has let it go, asks the device again). Call on the main thread.
pub fn thumbnail(device: &str, item: &str) -> Option<Result<Vec<u8>, String>> {
    let key = (device.to_string(), item.to_string());
    {
        let mut s = state();
        match s.thumbs.get(&key) {
            Some(Thumb::Ready(_) | Thumb::Failed(_)) => {
                return match s.thumbs.remove(&key) {
                    Some(Thumb::Ready(bytes)) => Some(Ok(bytes)),
                    Some(Thumb::Failed(e)) => Some(Err(e)),
                    _ => None,
                };
            }
            Some(Thumb::InFlight) => return None,
            None | Some(Thumb::Waiting) => {}
        }
        let busy = s.in_flight.get(device).copied().unwrap_or(0);
        if busy >= THUMBNAILS_IN_FLIGHT {
            s.thumbs.insert(key, Thumb::Waiting);
            return None;
        }
        s.in_flight.insert(device.to_string(), busy + 1);
        s.thumbs.insert(key, Thumb::InFlight);
    }
    #[cfg(any(target_os = "ios", target_os = "macos"))]
    let asked = apple::request_thumbnail(device, item);
    #[cfg(not(any(target_os = "ios", target_os = "macos")))]
    let asked: Result<(), String> = Err(UNAVAILABLE.into());
    if let Err(e) = asked {
        thumbnail_arrived(device, item, Err(e));
    }
    None
}

/// Copy `items` (ids from [`contents`]) off `device` into `dir`, one after another, under their own
/// names (made safe and unique: [`save_names`]); `done` is called once at the end, also when the
/// device is unplugged half way. Call on the main thread.
pub fn download(device: &str, items: &[String], dir: &Path, transfer: Arc<Transfer>, done: Done) -> Result<(), String> {
    transfer.total.store(items.len(), std::sync::atomic::Ordering::Relaxed);
    #[cfg(any(target_os = "ios", target_os = "macos"))]
    {
        apple::download(device, items, dir, transfer, done)
    }
    #[cfg(not(any(target_os = "ios", target_os = "macos")))]
    {
        let _ = (device, items, dir, done);
        Err(UNAVAILABLE.into())
    }
}

/// Stop reading `device` (its session is closed; what was read of it is forgotten).
pub fn close(device: &str) {
    {
        let mut s = state();
        s.contents.remove(device);
        s.thumbs.retain(|(d, _), _| d != device);
        s.in_flight.remove(device);
    }
    #[cfg(any(target_os = "ios", target_os = "macos"))]
    apple::close(device);
}

/// Where a thumbnail stands.
#[derive(Debug)]
enum Thumb {
    /// Asked for while [`THUMBNAILS_IN_FLIGHT`] others were on their way.
    Waiting,
    InFlight,
    Ready(Vec<u8>),
    Failed(String),
}

/// What the delegates have recorded (they run on the main thread, thumbnails and downloads on the
/// framework's queues) and what the app reads.
#[derive(Default)]
struct State {
    wake: Option<Wake>,
    cameras: Vec<Camera>,
    contents: HashMap<String, (u64, Contents)>,
    thumbs: HashMap<(String, String), Thumb>,
    /// Thumbnails on their way, per device.
    in_flight: HashMap<String, usize>,
    rev: u64,
}

impl State {
    fn next_rev(&mut self) -> u64 {
        self.rev = self.rev.wrapping_add(1).max(1);
        self.rev
    }

    /// Change an opened device's contents (nothing when it isn't open).
    #[cfg_attr(not(any(target_os = "ios", target_os = "macos")), allow(dead_code))]
    fn edit(&mut self, device: &str, f: impl FnOnce(&mut Contents)) {
        let rev = self.next_rev();
        if let Some((r, c)) = self.contents.get_mut(device) {
            f(c);
            *r = rev;
        }
    }
}

fn state() -> MutexGuard<'static, State> {
    static STATE: LazyLock<Mutex<State>> = LazyLock::new(Mutex::default);
    STATE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Ask the app to look again (outside the state lock).
fn wake() {
    let w = state().wake.clone();
    if let Some(w) = w {
        w();
    }
}

/// A device was connected (or is already there when watching starts).
#[cfg_attr(not(any(target_os = "ios", target_os = "macos")), allow(dead_code))]
pub(crate) fn camera_added(c: Camera) {
    {
        let mut s = state();
        match s.cameras.iter_mut().find(|x| x.id == c.id) {
            Some(x) => *x = c,
            None => s.cameras.push(c),
        }
    }
    wake();
}

/// A device was unplugged: it leaves the list, and an open one says so.
#[cfg_attr(not(any(target_os = "ios", target_os = "macos")), allow(dead_code))]
pub(crate) fn camera_removed(device: &str) {
    {
        let mut s = state();
        s.cameras.retain(|c| c.id != device);
        s.edit(device, |c| {
            c.error = Some("the camera or card was disconnected".into());
            c.complete = true;
        });
        s.thumbs.retain(|(d, _), _| d != device);
        s.in_flight.remove(device);
    }
    wake();
}

/// Items catalogued on an open device (more may follow until [`catalogued`]).
#[cfg_attr(not(any(target_os = "ios", target_os = "macos")), allow(dead_code))]
pub(crate) fn items_added(device: &str, items: Vec<Item>) {
    if items.is_empty() {
        return;
    }
    state().edit(device, |c| {
        let known: HashSet<String> = c.items.iter().map(|i| i.id.clone()).collect();
        c.items.extend(items.into_iter().filter(|i| !known.contains(&i.id)));
    });
    wake();
}

/// Items deleted from an open device (by the camera, or another app).
#[cfg_attr(not(any(target_os = "ios", target_os = "macos")), allow(dead_code))]
pub(crate) fn items_removed(device: &str, ids: &[String]) {
    let gone: HashSet<&String> = ids.iter().collect();
    state().edit(device, |c| c.items.retain(|i| !gone.contains(&i.id)));
    wake();
}

/// The device has catalogued everything: `items` is all of it.
#[cfg_attr(not(any(target_os = "ios", target_os = "macos")), allow(dead_code))]
pub(crate) fn catalogued(device: &str, items: Vec<Item>) {
    state().edit(device, |c| {
        c.items = items;
        c.complete = true;
    });
    wake();
}

/// Reading an open device failed.
#[cfg_attr(not(any(target_os = "ios", target_os = "macos")), allow(dead_code))]
pub(crate) fn failed(device: &str, error: String) {
    state().edit(device, |c| {
        c.error = Some(error);
        c.complete = true;
    });
    wake();
}

/// A thumbnail arrived (or won't).
#[cfg_attr(not(any(target_os = "ios", target_os = "macos")), allow(dead_code))]
pub(crate) fn thumbnail_arrived(device: &str, item: &str, r: Result<Vec<u8>, String>) {
    {
        let mut s = state();
        let key = (device.to_string(), item.to_string());
        if matches!(s.thumbs.get(&key), Some(Thumb::InFlight)) {
            if let Some(n) = s.in_flight.get_mut(device) {
                *n = n.saturating_sub(1);
            }
            s.thumbs.insert(
                key,
                match r {
                    Ok(b) => Thumb::Ready(b),
                    Err(e) => Thumb::Failed(e),
                },
            );
        }
    }
    wake();
}

/// Raw formats by extension (when the device doesn't say).
const RAW_EXTENSIONS: &[&str] = &[
    "3fr", "arw", "cr2", "cr3", "crw", "dcr", "dng", "erf", "fff", "iiq", "kdc", "mef", "mos", "mrw", "nef", "nrw", "orf", "ori", "pef", "raf",
    "raw", "rw2", "rwl", "sr2", "srf", "srw", "x3f",
];

/// `name` is a raw file by its extension.
pub fn raw_name(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(_, e)| RAW_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Local time `offset` seconds east of UTC at the instant `unix` seconds after 1970, as the
/// library writes capture times (`2026-10-03T14:22:05`). `None` for an instant that isn't a date
/// (NaN, or years outside 1..=9999).
pub fn local_time(unix: f64, offset: i64) -> Option<String> {
    if !unix.is_finite() || unix.abs() > 4.0e11 {
        return None;
    }
    let secs = (unix.floor() as i64).checked_add(offset)?;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant's civil_from_days
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    if !(1..=9999).contains(&y) {
        return None;
    }
    Some(format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}", rem / 3600, rem % 3600 / 60, rem % 60))
}

/// The names `names` are saved under in one folder: the last path component, without separators,
/// control characters or a leading dot, never empty, at most 200 bytes, and unique ignoring case
/// (two cards' `IMG_0001.JPG` become `IMG_0001.JPG` and `IMG_0001-1.JPG`).
pub fn save_names(names: &[String]) -> Vec<String> {
    let mut taken: HashSet<String> = HashSet::new();
    names
        .iter()
        .map(|n| {
            let safe = safe_name(n);
            let (stem, ext) = match safe.rsplit_once('.') {
                Some((a, b)) if !a.is_empty() => (a.to_string(), format!(".{b}")),
                _ => (safe.clone(), String::new()),
            };
            let mut name = safe.clone();
            let mut i = 1u32;
            while !taken.insert(name.to_lowercase()) {
                name = format!("{stem}-{i}{ext}");
                i = i.saturating_add(1);
            }
            name
        })
        .collect()
}

fn safe_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let mut s: String = base.chars().map(|c| if c.is_control() || c == ':' { '_' } else { c }).collect();
    s = s.trim().trim_start_matches('.').to_string();
    if s.is_empty() {
        s = "Photo".into();
    }
    const MAX: usize = 200;
    if s.len() > MAX {
        let (stem, ext) = match s.rsplit_once('.') {
            Some((a, b)) if !a.is_empty() && b.len() <= 16 => (a.to_string(), format!(".{b}")),
            _ => (s.clone(), String::new()),
        };
        let mut cut = MAX.saturating_sub(ext.len());
        while cut > 0 && !stem.is_char_boundary(cut) {
            cut -= 1;
        }
        s = format!("{}{ext}", stem.get(..cut).unwrap_or(""));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    fn item(id: &str) -> Item {
        Item { id: id.into(), name: format!("{id}.JPG"), size: 1, ..Default::default() }
    }

    /// Everything the delegates record, as the app reads it: devices coming and going, contents
    /// arriving in pieces, then whole; an unplugged device's contents say so. (One test: the state
    /// is process-wide.)
    #[test]
    fn delegates_record_what_the_app_reads() {
        let wakes = Arc::new(AtomicUsize::new(0));
        let w = wakes.clone();
        let _ = watch(Arc::new(move || {
            w.fetch_add(1, Ordering::SeqCst);
        }));
        assert!(cameras().is_empty());
        camera_added(Camera { id: "cam".into(), name: "Canon PowerShot V1".into(), kind: Kind::Camera });
        camera_added(Camera { id: "cam".into(), name: "Canon PowerShot V1".into(), kind: Kind::Camera });
        assert_eq!(cameras().len(), 1, "a device added twice is listed once");
        assert!(wakes.load(Ordering::SeqCst) >= 2);
        assert!(open("nope").is_err(), "not connected");
        // (off Apple platforms, and off the main thread there, the session can't start; the state
        // is there to fill all the same)
        let _ = open("cam");
        let (r0, c) = contents("cam", 0).unwrap();
        assert_eq!(c, Contents::default());
        assert!(contents("cam", r0).is_none(), "unchanged");
        items_added("cam", vec![item("a"), item("b")]);
        items_added("cam", vec![item("b"), item("c")]);
        let (r1, c) = contents("cam", r0).unwrap();
        assert_eq!(c.items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["a", "b", "c"]);
        assert!(!c.complete);
        items_removed("cam", &["b".to_string()]);
        catalogued("cam", vec![item("a"), item("c"), item("d")]);
        let (r2, c) = contents("cam", r1).unwrap();
        assert!(c.complete && c.items.len() == 3 && r2 != r1);
        items_added("other", vec![item("x")]);
        assert!(contents("other", 0).is_none(), "a device not opened has no contents");
        camera_removed("cam");
        assert!(cameras().is_empty());
        let (_, c) = contents("cam", r2).unwrap();
        assert!(c.error.is_some_and(|e| e.contains("disconnected")));
        close("cam");
        assert!(contents("cam", 0).is_none());
        failed("cam", "x".into());
        assert!(contents("cam", 0).is_none(), "a closed device stays closed");
        // thumbnails: off Apple platforms the request fails at once, and that is reported once
        camera_added(Camera { id: "card".into(), name: "EOS_DIGITAL".into(), kind: Kind::Card });
        let _ = open("card");
        if !cfg!(any(target_os = "ios", target_os = "macos")) {
            assert!(thumbnail("card", "a").is_none(), "asked");
            assert!(thumbnail("card", "a").is_some_and(|r| r.is_err()), "the failure");
            assert!(thumbnail("card", "a").is_none(), "taken: asked again");
            assert!(download("card", &["a".into()], Path::new("/tmp"), Arc::default(), Box::new(|_| {})).is_err());
        }
        // at most THUMBNAILS_IN_FLIGHT on their way: the rest wait, and start as those arrive
        {
            let mut s = state();
            s.in_flight.insert("card".into(), THUMBNAILS_IN_FLIGHT);
        }
        assert!(thumbnail("card", "w").is_none());
        assert!(matches!(state().thumbs.get(&("card".to_string(), "w".to_string())), Some(Thumb::Waiting)));
        {
            let mut s = state();
            s.thumbs.insert(("card".into(), "f".into()), Thumb::InFlight);
        }
        thumbnail_arrived("card", "f", Ok(vec![1, 2, 3]));
        assert_eq!(state().in_flight.get("card").copied(), Some(THUMBNAILS_IN_FLIGHT - 1));
        assert_eq!(thumbnail("card", "f"), Some(Ok(vec![1, 2, 3])));
        thumbnail_arrived("card", "f", Ok(vec![9]));
        assert!(!state().thumbs.contains_key(&("card".to_string(), "f".to_string())), "a late answer for a taken thumbnail is dropped");
        camera_removed("card");
        close("card");
    }

    #[test]
    fn raw_files_by_extension() {
        assert!(raw_name("IMG_0001.CR3") && raw_name("dsc.nef") && raw_name("a.b.DNG") && raw_name("P5048183.ORI"));
        assert!(!raw_name("IMG_0001.JPG") && !raw_name("CR3") && !raw_name("x.heic"));
    }

    #[test]
    fn capture_times_are_local_iso() {
        assert_eq!(local_time(0.0, 0).as_deref(), Some("1970-01-01T00:00:00"));
        // 2026-10-03 14:22:05 UTC, shown two hours east and seven west
        let t = 1_791_037_325.0;
        assert_eq!(local_time(t, 0).as_deref(), Some("2026-10-03T14:22:05"));
        assert_eq!(local_time(t + 0.9, 7200).as_deref(), Some("2026-10-03T16:22:05"));
        assert_eq!(local_time(t, -7 * 3600).as_deref(), Some("2026-10-03T07:22:05"));
        assert_eq!(local_time(951_782_400.0, 0).as_deref(), Some("2000-02-29T00:00:00"), "a leap day");
        assert_eq!(local_time(-86_400.0, 0).as_deref(), Some("1969-12-31T00:00:00"));
        assert_eq!(local_time(f64::NAN, 0), None);
        assert_eq!(local_time(f64::INFINITY, 0), None);
        assert_eq!(local_time(1e18, 0), None);
        assert_eq!(local_time(0.0, i64::MAX), None);
    }

    #[test]
    fn saved_names_are_safe_and_unique() {
        let names: Vec<String> =
            ["IMG_0001.JPG", "img_0001.jpg", "IMG_0001.JPG", "../../etc/passwd", "", ".hidden.CR3", "a\u{0}b:c.png", "NOEXT", "noext"]
                .iter()
                .map(|s| s.to_string())
                .collect();
        assert_eq!(
            save_names(&names),
            ["IMG_0001.JPG", "img_0001-1.jpg", "IMG_0001-2.JPG", "passwd", "Photo", "hidden.CR3", "a_b_c.png", "NOEXT", "noext-1"]
        );
        let long = format!("{}.heic", "é".repeat(300));
        let s = &save_names(&[long])[0];
        assert!(s.len() <= 200 && s.ends_with(".heic"), "{} bytes", s.len());
    }
}
