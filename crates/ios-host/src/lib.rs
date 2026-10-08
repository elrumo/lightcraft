//! LightCraft on iOS: what the app needs from the system that egui and winit don't give it — the
//! Photos and Files pickers, the share sheet, the pasteboard, haptic feedback, the app's lifecycle
//! (backgrounding, memory warnings, time to finish work in the background) and ImageIO's decoder for
//! HEIC / HEIF (the iPhone's own photos), whose formats have no pure-Rust decoder
//! (`lightcraft_codecs::set_system_decoder`).
//!
//! The Objective-C calls go through `objc2` (Rust bindings to the system frameworks; no C or
//! Objective-C code is compiled) and live in `ios` (compiled for iOS only). This crate is allowed
//! `unsafe` for them, as `lightcraft-sysmem` is for its two libSystem calls (`CLAUDE.md`, Never
//! crash): every `unsafe` block says why it is sound, the API here is safe, and failures come
//! back as errors. On other platforms every call returns an "only on iOS" error and the portable
//! helpers below (file names, folder walks) are what the tests exercise.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
mod ios;

/// What a picker handed over: copies of the chosen files in the staging folder (the app's to
/// move into its library), and what couldn't be copied.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Picked {
    pub files: Vec<PathBuf>,
    /// One message per item that couldn't be copied.
    pub failed: Vec<String>,
    /// The user closed the picker without choosing.
    pub cancelled: bool,
}

/// Which picker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickKind {
    /// The Photos library (PHPicker: needs no permission; originals, HEIC and ProRAW included).
    Photos,
    /// Image files from Files (iCloud Drive, On My iPhone, external drives, other apps' storage).
    Files,
    /// A folder from Files: the photos in it and its subfolders.
    Folder,
}

/// Called once, on any thread, with everything the picker handed over.
pub type Deliver = Box<dyn FnOnce(Picked) + Send + 'static>;

/// Which files a folder pick copies (by name; the app knows its formats).
pub type Accept = fn(&Path) -> bool;

/// Show a picker over the app (call on the main thread). The chosen items are copied into a new
/// subfolder of `staging` and handed to `deliver` when all are copied (or cancelled). Errors:
/// not on the main thread, no window to present over, or not iOS.
pub fn pick(kind: PickKind, staging: &Path, accept: Accept, deliver: Deliver) -> Result<(), String> {
    #[cfg(target_os = "ios")]
    {
        ios::pickers::pick(kind, staging, accept, deliver)
    }
    #[cfg(not(target_os = "ios"))]
    {
        let _ = (kind, staging, accept, deliver);
        Err(ONLY_IOS.into())
    }
}

/// Offer files to the system's share sheet (Save Image to Photos, Save to Files, AirDrop, Mail,
/// other apps). Call on the main thread; `paths` must stay until the sheet is closed.
pub fn share(paths: &[PathBuf]) -> Result<(), String> {
    #[cfg(target_os = "ios")]
    {
        ios::share::share(paths)
    }
    #[cfg(not(target_os = "ios"))]
    {
        let _ = paths;
        Err(ONLY_IOS.into())
    }
}

/// A tap on the Taptic Engine, as UIKit's feedback generators give them (the system's Haptics
/// switch in Settings still decides whether the phone buzzes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Haptic {
    /// A light tick: a step, a detent, a choice.
    Selection,
    /// A small thing landed.
    Light,
    /// A mode began.
    Medium,
    /// Something is going away.
    Warning,
}

/// Play `haptic` (call on the main thread; elsewhere, and off the phone, it does nothing: feedback
/// is never worth an error).
pub fn haptic(haptic: Haptic) {
    #[cfg(target_os = "ios")]
    ios::haptics::play(haptic);
    #[cfg(not(target_os = "ios"))]
    let _ = haptic;
}

/// The system pasteboard's text (Paste in a text field's edit menu); `None` when it has none, or
/// off iOS. iOS may first ask the user to allow the paste.
pub fn pasteboard_text() -> Option<String> {
    #[cfg(target_os = "ios")]
    {
        ios::pasteboard::text()
    }
    #[cfg(not(target_os = "ios"))]
    None
}

/// Put `text` on the system pasteboard (what the app copies; does nothing off iOS).
pub fn set_pasteboard_text(text: &str) {
    #[cfg(target_os = "ios")]
    ios::pasteboard::set_text(text);
    #[cfg(not(target_os = "ios"))]
    let _ = text;
}

/// The app's lifecycle, as UIKit announces it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lifecycle {
    /// About to lose focus (a call, Control Centre, the app switcher). Save now: the app may be
    /// ended without another word once it is in the background.
    WillResignActive,
    /// In the background: no GPU work, a much smaller memory limit.
    DidEnterBackground,
    WillEnterForeground,
    DidBecomeActive,
    /// The system is short of memory: give some back or be ended.
    MemoryWarning,
    WillTerminate,
}

/// Call `f` on the main thread for every lifecycle notification from now on (call on the main
/// thread, once).
pub fn observe_lifecycle(f: Box<dyn Fn(Lifecycle)>) -> Result<(), String> {
    #[cfg(target_os = "ios")]
    {
        ios::lifecycle::observe(f)
    }
    #[cfg(not(target_os = "ios"))]
    {
        let _ = f;
        Err(ONLY_IOS.into())
    }
}

/// Extra time to finish work after the app went to the background (an export, a save), until
/// dropped or until the system's allowance runs out.
pub struct BackgroundTask {
    /// Ends the task when dropped.
    #[cfg(target_os = "ios")]
    _inner: ios::lifecycle::Task,
}

impl BackgroundTask {
    /// Ask for the time (call on the main thread). `None`: not granted, or not iOS.
    pub fn begin(name: &str) -> Option<BackgroundTask> {
        #[cfg(target_os = "ios")]
        {
            ios::lifecycle::Task::begin(name).map(|t| BackgroundTask { _inner: t })
        }
        #[cfg(not(target_os = "ios"))]
        {
            let _ = name;
            None
        }
    }
}

impl std::fmt::Debug for BackgroundTask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BackgroundTask")
    }
}

/// An image decoded by ImageIO: as stored (orientation not applied), RGBA, alpha premultiplied,
/// in the colour space `icc` describes (sRGB when `None`).
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub pixels: Pixels,
    /// Bits per sample in the file.
    pub bit_depth: u8,
    pub icc: Option<Vec<u8>>,
    /// EXIF orientation 1..=8.
    pub orientation: u16,
    /// Size of the full image (`width` × `height` are smaller when it was decoded to fit a box).
    pub source_width: u32,
    pub source_height: u32,
}

/// RGBA samples, row by row.
#[derive(Clone, Debug, PartialEq)]
pub enum Pixels {
    Rgba8(Vec<u8>),
    Rgba16(Vec<u16>),
}

/// Decode an image file's bytes with ImageIO (HEIC / HEIF, AVIF on iOS 16+, and anything else it
/// reads), fitting it in `max` when given (a thumbnail: ImageIO decodes it smaller, much faster).
/// Safe to call from any thread.
pub fn decode_image(bytes: &[u8], max: Option<(u32, u32)>) -> Result<Image, String> {
    #[cfg(target_os = "ios")]
    {
        ios::imageio::decode(bytes, max)
    }
    #[cfg(not(target_os = "ios"))]
    {
        let _ = (bytes, max);
        Err(ONLY_IOS.into())
    }
}

/// One line to the system log (NSLog: the device console, `idevicesyslog` on Linux, Console.app on
/// a Mac), through the app's Objective-C host (`lightcraft_host_log`,
/// `apps/lightcraft-ios/xtool/Sources/LightCraftHost`). Elsewhere: nothing.
pub fn console(line: &str) {
    #[cfg(target_os = "ios")]
    ios::console(line);
    #[cfg(not(target_os = "ios"))]
    let _ = line;
}

/// Return taps on the on-screen keyboard that the app hasn't taken yet.
static RETURN_PRESSES: AtomicU32 = AtomicU32::new(0);
/// How far the on-screen keyboard reaches up over the app, in points (`f32` bits; 0 = hidden).
static KEYBOARD: AtomicU32 = AtomicU32::new(0);

/// The on-screen keyboard's Return was tapped (on iOS, the Objective-C host's `-insertText:` hook
/// calls this through `lightcraft_host_return_key`).
pub fn return_pressed() {
    RETURN_PRESSES.fetch_add(1, Ordering::Relaxed);
}

/// Return taps since the last call. winit hands Return over as a `"\n"` character, which egui
/// drops, so the app turns these into Enter key presses (which confirm a text field).
pub fn take_return_presses() -> u32 {
    RETURN_PRESSES.swap(0, Ordering::Relaxed)
}

/// How much of the bottom of the screen the on-screen keyboard covers, in points (0 when it is
/// hidden): the app keeps its content above it.
pub fn keyboard_height() -> f32 {
    f32::from_bits(KEYBOARD.load(Ordering::Relaxed))
}

/// Set by the keyboard notifications ([`observe_keyboard`]); negative or not finite means hidden.
pub fn set_keyboard_height(points: f32) {
    let h = if points.is_finite() { points.max(0.0) } else { 0.0 };
    KEYBOARD.store(h.to_bits(), Ordering::Relaxed);
}

/// Call `f` on the main thread whenever the on-screen keyboard appears, moves or hides, after
/// [`keyboard_height`] has the new height (call on the main thread, once).
pub fn observe_keyboard(f: Box<dyn Fn()>) -> Result<(), String> {
    #[cfg(target_os = "ios")]
    {
        ios::lifecycle::observe_keyboard(f)
    }
    #[cfg(not(target_os = "ios"))]
    {
        let _ = f;
        Err(ONLY_IOS.into())
    }
}

#[cfg_attr(target_os = "ios", allow(dead_code))]
const ONLY_IOS: &str = "only available in the iOS app";

/// Largest number of files a folder pick copies (a folder of a whole drive must not fill the
/// device).
pub const MAX_FOLDER_FILES: usize = 5000;
/// How deep a folder pick looks into subfolders.
pub const MAX_FOLDER_DEPTH: usize = 12;

/// A file name safe to create in the staging folder: the last path component, without
/// separators or control characters, not empty, not hidden, at most 200 bytes (cut at a char
/// boundary, keeping the extension).
pub fn safe_file_name(name: &str, fallback: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let mut s: String = base.chars().map(|c| if c.is_control() || c == ':' { '_' } else { c }).collect();
    s = s.trim().trim_start_matches('.').to_string();
    if s.is_empty() {
        s = fallback.to_string();
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

/// `dir/name`, or `dir/name-1.ext`, `-2`… when taken.
pub fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((a, b)) if !a.is_empty() => (a, format!(".{b}")),
        _ => (name, String::new()),
    };
    (1..100_000).map(|i| dir.join(format!("{stem}-{i}{ext}"))).find(|p| !p.exists()).unwrap_or(first)
}

/// A new, empty subfolder of `staging` for one pick (so names from different picks never meet).
pub fn new_batch_dir(staging: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(staging).map_err(|e| format!("{}: {e}", staging.display()))?;
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let dir = unique_path(staging, &format!("pick-{stamp}"));
    std::fs::create_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    Ok(dir)
}

/// Copy `src` into `dir` under `name` (made safe and unique); returns where it went.
pub fn copy_into(dir: &Path, src: &Path, name: &str) -> Result<PathBuf, String> {
    let to = unique_path(dir, &safe_file_name(name, "Photo"));
    std::fs::copy(src, &to).map_err(|e| format!("{}: {e}", src.display()))?;
    Ok(to)
}

/// Copy the files under `folder` that `accept` takes into `dir` (at most [`MAX_FOLDER_FILES`],
/// [`MAX_FOLDER_DEPTH`] levels deep, hidden files and symbolic links skipped).
pub fn copy_folder(folder: &Path, dir: &Path, accept: Accept) -> Picked {
    let mut out = Picked::default();
    let mut stack = vec![(folder.to_path_buf(), 0usize)];
    while let Some((d, depth)) = stack.pop() {
        let entries = match std::fs::read_dir(&d) {
            Ok(e) => e,
            Err(e) => {
                out.failed.push(format!("{}: {e}", d.display()));
                continue;
            }
        };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            let Ok(t) = e.file_type() else { continue };
            let p = e.path();
            if t.is_dir() {
                if depth < MAX_FOLDER_DEPTH {
                    stack.push((p, depth + 1));
                }
            } else if t.is_file() && accept(&p) {
                if out.files.len() >= MAX_FOLDER_FILES {
                    out.failed.push(format!("only the first {MAX_FOLDER_FILES} photos of {} were copied", folder.display()));
                    return out;
                }
                match copy_into(dir, &p, &name) {
                    Ok(to) => out.files.push(to),
                    Err(e) => out.failed.push(e),
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lc-ios-host-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Return taps are counted until the app takes them; the keyboard height never goes negative.
    #[test]
    fn return_taps_and_keyboard_height() {
        let _ = take_return_presses();
        return_pressed();
        return_pressed();
        assert_eq!(take_return_presses(), 2);
        assert_eq!(take_return_presses(), 0);
        set_keyboard_height(336.5);
        assert_eq!(keyboard_height(), 336.5);
        set_keyboard_height(-4.0);
        assert_eq!(keyboard_height(), 0.0);
        set_keyboard_height(f32::NAN);
        assert_eq!(keyboard_height(), 0.0);
        assert!(observe_keyboard(Box::new(|| {})).is_err(), "only on iOS");
    }

    #[test]
    fn file_names_from_pickers_are_made_safe() {
        assert_eq!(safe_file_name("IMG_0001.HEIC", "Photo"), "IMG_0001.HEIC");
        assert_eq!(safe_file_name("../../etc/passwd", "Photo"), "passwd");
        assert_eq!(safe_file_name("a/b\\c.jpg", "Photo"), "c.jpg");
        assert_eq!(safe_file_name(".hidden.jpg", "Photo"), "hidden.jpg");
        assert_eq!(safe_file_name("", "Photo"), "Photo");
        assert_eq!(safe_file_name("..", "Photo"), "Photo");
        assert_eq!(safe_file_name("a\u{0}b:c.png", "Photo"), "a_b_c.png");
        let long = format!("{}.heic", "é".repeat(300));
        let s = safe_file_name(&long, "Photo");
        assert!(s.len() <= 200 && s.ends_with(".heic"), "{} bytes", s.len());
    }

    #[test]
    fn copies_never_overwrite() {
        let d = temp("unique");
        let src = d.join("src.jpg");
        std::fs::write(&src, b"one").unwrap();
        let batch = new_batch_dir(&d.join("staging")).unwrap();
        let a = copy_into(&batch, &src, "IMG.jpg").unwrap();
        let b = copy_into(&batch, &src, "IMG.jpg").unwrap();
        let c = copy_into(&batch, &src, "noext").unwrap();
        let c2 = copy_into(&batch, &src, "noext").unwrap();
        assert_eq!(a.file_name().unwrap(), "IMG.jpg");
        assert_eq!(b.file_name().unwrap(), "IMG-1.jpg");
        assert_eq!((c.file_name().unwrap(), c2.file_name().unwrap()), ("noext".as_ref(), "noext-1".as_ref()));
        let other = new_batch_dir(&d.join("staging")).unwrap();
        assert_ne!(other, batch, "each pick gets its own folder");
        assert!(copy_into(&batch, &d.join("missing.jpg"), "x.jpg").is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_folder_pick_copies_the_photos_in_it() {
        let d = temp("folder");
        let src = d.join("card");
        std::fs::create_dir_all(src.join("DCIM/100APPLE")).unwrap();
        std::fs::create_dir_all(src.join(".Trashes")).unwrap();
        for (p, b) in
            [("DCIM/100APPLE/IMG_1.HEIC", "a"), ("DCIM/100APPLE/IMG_1.AAE", "x"), ("top.dng", "b"), (".Trashes/old.jpg", "c"), ("._top.dng", "d")]
        {
            std::fs::write(src.join(p), b).unwrap();
        }
        let accept: Accept = |p| p.extension().is_some_and(|e| matches!(e.to_ascii_lowercase().to_str(), Some("heic" | "dng" | "jpg")));
        let batch = new_batch_dir(&d.join("staging")).unwrap();
        let got = copy_folder(&src, &batch, accept);
        let mut names: Vec<String> = got.files.iter().map(|p| p.file_name().unwrap().to_string_lossy().to_string()).collect();
        names.sort();
        assert_eq!(names, ["IMG_1.HEIC", "top.dng"], "{got:?}");
        assert!(got.failed.is_empty(), "{got:?}");
        let missing = copy_folder(&d.join("nope"), &batch, accept);
        assert!(missing.files.is_empty() && missing.failed.len() == 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn everything_else_says_it_needs_ios() {
        if cfg!(target_os = "ios") {
            return;
        }
        assert!(share(&[]).is_err());
        assert!(decode_image(b"", None).is_err());
        assert!(observe_lifecycle(Box::new(|_| {})).is_err());
        assert!(BackgroundTask::begin("x").is_none());
        haptic(Haptic::Selection); // (nothing to play on, nothing to fail)
        let d = temp("stub");
        assert!(pick(PickKind::Photos, &d, |_| true, Box::new(|_| {})).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}
