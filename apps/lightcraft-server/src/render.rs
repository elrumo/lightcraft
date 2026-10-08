//! `POST /api/render`: render a photo the server keeps, with the edits a device sends, at the size it asks for.
//!
//! This is how a device that can't hold a full-size render in memory (a phone, a 48 MP photo) exports at full size:
//! the server — which has the memory — decodes the original, runs the same develop pipeline and export encoder the
//! apps use, and sends the image back. It renders in a throwaway in-memory session, like `lightcraft-cli render`, so
//! nothing touches the user's library or originals: the original is linked into a temporary folder (never copied
//! when the file system allows it) and the folder is removed when the render ends.
//!
//! Renders are heavy, so only [`Slots`] of them run at once (`LIGHTCRAFT_RENDER_THREADS`, default 1); the others wait
//! a while and then get `503` — the device tries again later.

use std::path::Path;
use std::sync::{Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use lightcraft_catalog::sync::proto;
use lightcraft_engine::Session;
use lightcraft_engine::catalog::PhotoId;
use lightcraft_engine::export::{ExportOptions, export_photo};
use serde_json::json;

/// How many renders run at once.
pub struct Slots {
    max: usize,
    running: Mutex<usize>,
    freed: Condvar,
}

/// A running render's place, free again on drop.
pub struct Slot<'a>(&'a Slots);

impl Slots {
    pub fn new(max: usize) -> Slots {
        Slots { max: max.max(1), running: Mutex::new(0), freed: Condvar::new() }
    }

    /// Take a place, waiting up to `wait` for one; `None` when all stay taken.
    pub fn acquire(&self, wait: Duration) -> Option<Slot<'_>> {
        let end = Instant::now() + wait;
        let mut running = self.running.lock().unwrap_or_else(PoisonError::into_inner);
        while *running >= self.max {
            let left = end.checked_duration_since(Instant::now())?;
            running = self.freed.wait_timeout(running, left).unwrap_or_else(PoisonError::into_inner).0;
        }
        *running += 1;
        Some(Slot(self))
    }

    /// Renders running now.
    pub fn running(&self) -> usize {
        *self.running.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        *self.0.running.lock().unwrap_or_else(PoisonError::into_inner) -= 1;
        self.0.freed.notify_one();
    }
}

/// An encoded render.
pub struct Rendered {
    pub bytes: Vec<u8>,
    pub width: usize,
    pub height: usize,
    pub content_type: &'static str,
}

fn content_type(extension: &str) -> &'static str {
    match extension {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "tif" | "tiff" => "image/tiff",
        "webp" => "image/webp",
        "avif" => "image/avif",
        _ => "application/octet-stream",
    }
}

/// An extension the decoder can be told about: letters and digits only, short.
fn safe_extension(name: &str) -> &str {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .filter(|e| !e.is_empty() && e.len() <= 8 && e.bytes().all(|b| b.is_ascii_alphanumeric()))
        .unwrap_or("bin")
}

/// Render `original` (a file on this server) as `r` asks. `scratch` is a folder for the temporary link.
pub fn render_file(original: &Path, scratch: &Path, r: &proto::Render) -> Result<Rendered, String> {
    let dir = scratch.join(format!("render-{}", crate::accounts::random_hex(8)?));
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let result = render_in(original, &dir, r);
    let _ = std::fs::remove_dir_all(&dir);
    result
}

fn render_in(original: &Path, dir: &Path, r: &proto::Render) -> Result<Rendered, String> {
    let file = dir.join(format!("photo.{}", safe_extension(&r.name)));
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(original, &file).is_ok();
    #[cfg(not(unix))]
    let linked = false;
    if !linked {
        std::fs::copy(original, &file).map_err(|e| format!("{}: {e}", original.display()))?;
    }
    // (a request to render on a server is rendered here, by this server, and not passed on)
    let options = ExportOptions { on_server: false, ..ExportOptions::from_json(&r.export) };
    if !options.format.is_rendered() {
        return Err("the server renders JPEG, PNG, TIFF, WebP or AVIF".into());
    }
    let mut s = Session::new().with_fs();
    let imported = s.execute("library.import", &json!({"paths": [file.to_string_lossy()]})).map_err(|e| e.to_string())?;
    let id = imported["imported"][0].as_u64().ok_or("the server can't read this photo")?;
    let run = |s: &mut Session, cmd: &str, p: serde_json::Value| s.execute(cmd, &p).map(|_| ()).map_err(|e| e.to_string());
    run(&mut s, "library.select", json!({"ids": [id], "active": id}))?;
    run(&mut s, "develop.merge", json!({"settings": r.settings}))?;
    let e = export_photo(&mut s, PhotoId(id), &options, 1)?;
    let extension = Path::new(&e.file_name).extension().and_then(|x| x.to_str()).unwrap_or("").to_ascii_lowercase();
    Ok(Rendered { content_type: content_type(&extension), bytes: e.bytes, width: e.width, height: e.height })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_limit_and_release() {
        let s = Slots::new(2);
        let (a, b) = (s.acquire(Duration::ZERO).unwrap(), s.acquire(Duration::ZERO).unwrap());
        assert_eq!(s.running(), 2);
        assert!(s.acquire(Duration::from_millis(20)).is_none(), "a third waits, then gives up");
        drop(a);
        let c = s.acquire(Duration::from_millis(20)).expect("a place is free again");
        assert_eq!(s.running(), 2);
        drop((b, c));
        assert_eq!(s.running(), 0);
    }

    #[test]
    fn extensions_are_safe() {
        assert_eq!(safe_extension("IMG_0001.DNG"), "DNG");
        assert_eq!(safe_extension("../../etc/passwd"), "bin");
        assert_eq!(safe_extension("a.b/../c"), "bin");
        assert_eq!(safe_extension("x.tool-long-extension"), "bin");
        assert_eq!(safe_extension("noext"), "bin");
    }
}
