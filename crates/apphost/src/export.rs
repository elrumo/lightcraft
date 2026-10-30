//! `app.export` without a window: Export Now with the desktop app's parameters, for frontends that
//! have no export dialog of their own yet (the MCP server's headless backend, native frontends).

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::{Arc, Mutex};

use lightcraft_catalog::PhotoId;
use lightcraft_engine::Session;
use lightcraft_engine::activity::{Cancel, TaskGuard, TaskHandle};
use lightcraft_engine::export::{
    AfterExport, Destination, ExportFormat, ExportOptions, HostFile, HostPixels, OutputSpace, PreparedExport, Resize, prepare_batch, run_batch,
};
use serde_json::{Value, json};

/// The parameters `app.export` takes (as `engine.commands` lists them).
pub const PARAMS: &str = "{path?: output file (.jpg/.png/.tif/.webp/.avif/.jxl/.psd/.dng) | dir?, ids?, preset?, format?: jpeg|png|tiff|webp|avif|jxl|heic|psd|dng|original (heic is written by the Mac and iPhone apps only), longEdge?|shortEdge?|width?|height?|megapixels?|percent? (default longEdge 3000; longEdge 0 = full size), fit?: dimensions|widthHeight (with width + height: dimensions fits either orientation, widthHeight limits the width to `width` and the height to `height`), dontEnlarge?, ppi?, quality?: 1..100, jxlLossless?, jxlEffort?: fast|normal, jxlProgressive?, limitKb?, colorSpace?: srgb|displayP3|adobeRgb|proPhoto|rec2020, bitDepth?: 8|10|16|32, hdr? (HDR output for HDR edits: gain map JPEG, PQ AVIF / PNG / JPEG XL, float TIFF), sharpen?: none|screen|matte|glossy, sharpenAmount?: low|standard|high, metadata?: all|allExceptCamera|copyright|none, removeLocation?, naming?, startNumber?, destinationMode?: specificFolder|sameAsOriginal (sameAsOriginal = each photo into its original's folder; `dir` unused), subfolder?, conflict?: unique|overwrite|skip, tiffCompression?: none|lzw|zip, dngCompression?: lossless|deflate|uncompressed, addToPhotos?: bool (macOS: then add the files written to Apple Photos; outcome in `applePhotos`), photosAlbum?: top-level album name (made when missing), onServer?: render on the sync server, hostPixels?: {\"<photo id>\": {file: path of width × height pixels of opaque RGBA8 (4 bytes each) or RGBA16 (8 bytes, native byte order), width, height, depth?: 8|16 (8), space?: any colorSpace value (srgb)} (pictures a frontend's own renderer drew for some of the photos; the export encodes and places them as it does its own render, and renders any photo whose picture does not fit it: another size, another colour space or bit depth than the file is written in, HDR, an unreadable file), hostFiles?: {\"<photo id>\": {file: path of the finished file in the export's format, width, height}} (files a frontend with an encoder of its own wrote for some of the photos, the pictures and everything done to them included: the export names and places them, and does nothing to their bytes. A photo with none is the export's own; for heic that is an error), watermark?: text | {text?, vertical? (upright columns, right to left), size? (text height, 0.005..0.5 of the short edge; default 0.035), opacity? (0..1; 0.7), anchor?: topLeft|top|topRight|left|center|right|bottomLeft|bottom|bottomRight, insetX? (margin from the left/right edge, 0..0.4 of the short edge; 0.025), insetY? (from the top/bottom edge; 0.025), inset? (sets both), rotation?: 0|90|180|270 (degrees clockwise; 0), color? [r,g,b] sRGB, shadow?, image? (graphic drawn instead of the text), imageWidth? (0.01..1 of the photo's width; 0.2)}} — an unknown parameter, an out-of-range watermark size or a value of the wrong kind is an error, not a default";

/// `app.export` as an entry of `engine.commands`.
pub fn command_info(session: &Session) -> Value {
    json!({"id": "app.export", "label": "Export Now", "menu": [], "shortcut": null, "params": PARAMS, "enabled": session.active().is_some()})
}

/// What [`Prepared::start`] and [`export_now`] write: the photos ready to render, the options, where
/// the files go, and what happens to them after (Add to Apple Photos).
pub struct Prepared {
    pub items: Vec<PreparedExport>,
    pub opts: ExportOptions,
    pub to: Destination,
    pub after: AfterExport,
}

/// Remember an export's options for Export with Previous (and to prefill the dialog): not the
/// photos or a file name, and the folder only when the user chose it (`dir`), not a staging one.
pub fn remember(session: &mut Session, p: &Value, dir: Option<&str>) {
    let mut last = p.clone();
    if let Some(o) = last.as_object_mut() {
        o.remove("ids");
        o.remove("id");
        o.remove("path");
        o.remove("sendTo");
        o.remove("hostPixels");
        o.remove("hostFiles");
        match dir {
            Some(d) => {
                o.insert("dir".into(), serde_json::json!(d));
            }
            None => {
                o.remove("dir");
            }
        }
    }
    session.last_export = Some(last);
    let _ = session.save_prefs();
}

/// Get the photos `p` names ready to export (`ids`; else the selection, in grid order, when more than
/// one photo is selected; else `id` or the active photo) with the options in `p` (Export Now's
/// parameters). Cheap: the rendering is [`run_batch`]'s.
pub fn prepare(session: &mut Session, p: &Value) -> Result<Prepared, String> {
    let mut p = session.export_params_or(p, ExportFormat::Png)?;
    // pictures a frontend drew (see [`HostPixels`]): not an export option
    let host = p.as_object_mut().and_then(|o| o.remove("hostPixels"));
    // files a frontend wrote (see [`HostFile`]): not an export option either
    let host_files = p.as_object_mut().and_then(|o| o.remove("hostFiles"));
    let p = &p;
    // PNG for a `path` that names no format; `bitDepth` is checked against the format written
    let mut opts = ExportOptions::from_params_or(p, ExportFormat::Png).map_err(|e| e.to_string())?;
    if !ExportOptions::has_size_param(p) {
        opts.resize = Some(Resize::long_edge(3000));
    }
    let exact = p.get("path").and_then(Value::as_str);
    let ids: Vec<PhotoId> = match p.get("ids").and_then(Value::as_array) {
        Some(a) => a.iter().filter_map(|v| serde_json::from_value(v.clone()).ok()).collect(),
        // like the desktop app: the selection (in grid order), else the given / active photo
        None if p.get("id").is_none() && session.selection.ids.len() > 1 => {
            let sel: std::collections::HashSet<_> = session.selection.ids.iter().copied().collect();
            session.visible_cloned().into_iter().filter(|id| sel.contains(id)).collect()
        }
        None => {
            let id = p.get("id").and_then(Value::as_u64).map(PhotoId).or(session.active());
            vec![id.ok_or("no photo (import one or pass `id`)")?]
        }
    };
    let dir = p.get("dir").and_then(Value::as_str).unwrap_or("");
    // Add to Apple Photos: refused before anything is written when it can't be done
    let after = session.after_export(&opts)?;
    let mut items = prepare_batch(session, &ids, &opts)?;
    // (a file replaces the picture: it is the finished export)
    if let Some(host) = &host {
        apply_host_pixels(&mut items, host)?;
    }
    if let Some(files) = &host_files {
        apply_host_files(&mut items, files)?;
    }
    Ok(Prepared { items, opts, to: Destination { dir: dir.to_string(), exact: exact.map(str::to_string) }, after })
}

/// Give each photo of `items` the picture `host` (`hostPixels`) holds for it, when it can use it
/// ([`PreparedExport::with_host_pixels`]); the others are rendered by the export. A malformed entry is an error.
fn apply_host_pixels(items: &mut [PreparedExport], host: &Value) -> Result<(), String> {
    let entries = host.as_object().ok_or("`hostPixels` must be an object of photo id to {file, width, height}")?;
    for item in items {
        let Some(entry) = entries.get(&item.photo.0.to_string()) else { continue };
        let size = |key: &str| entry.get(key).and_then(Value::as_u64).and_then(|n| usize::try_from(n).ok());
        let (Some(file), Some(width), Some(height)) = (entry.get("file").and_then(Value::as_str), size("width"), size("height")) else {
            return Err(format!("`hostPixels` for photo {} needs file, width and height", item.photo.0));
        };
        let depth = match entry.get("depth") {
            None => 8,
            Some(d) => {
                d.as_u64().filter(|d| matches!(d, 8 | 16)).ok_or_else(|| format!("`hostPixels` for photo {}: depth is 8 or 16", item.photo.0))? as u8
            }
        };
        let space = match entry.get("space") {
            None => OutputSpace::Srgb,
            Some(v) => {
                v.as_str().and_then(OutputSpace::parse).ok_or_else(|| format!("`hostPixels` for photo {}: unknown colour space", item.photo.0))?
            }
        };
        item.with_host_pixels(HostPixels { file: file.to_string(), width, height, depth, space });
    }
    Ok(())
}

/// Give each photo of `items` the finished file `files` (`hostFiles`) holds for it, when it can stand in ([`PreparedExport::with_host_file`]).
/// A malformed entry is an error.
fn apply_host_files(items: &mut [PreparedExport], files: &Value) -> Result<(), String> {
    let entries = files.as_object().ok_or("`hostFiles` must be an object of photo id to {file, width, height}")?;
    for item in items {
        let Some(entry) = entries.get(&item.photo.0.to_string()) else { continue };
        let size = |key: &str| entry.get(key).and_then(Value::as_u64).and_then(|n| usize::try_from(n).ok());
        let (Some(file), Some(width), Some(height)) = (entry.get("file").and_then(Value::as_str), size("width"), size("height")) else {
            return Err(format!("`hostFiles` for photo {} needs file, width and height", item.photo.0));
        };
        item.with_host_file(HostFile { file: file.to_string(), width, height });
    }
    Ok(())
}

/// Export the photos `p` names with the options in `p`, as the desktop app's Export Now does, on
/// this thread ([`prepare`]). `progress(done, total, next name)` returning false stops before the
/// next photo (the answer is then the error `cancelled`). Without a size, the long edge is 3000 px.
pub fn export_now(session: &mut Session, p: &Value, progress: &mut dyn FnMut(usize, usize, &str) -> bool) -> Result<Value, String> {
    let Prepared { items, opts, to, after } = prepare(session, p)?;
    let write = &mut lightcraft_engine::export::write_file;
    let total = items.len();
    let mut cancelled = false;
    let files = run_batch(items, &opts, &to, write, &|path| Path::new(path).exists(), true, &mut |done, name| {
        cancelled = !progress(done, total, name);
        !cancelled
    })?;
    if !cancelled {
        cancelled = !progress(total, total, "");
    }
    if cancelled {
        return Err("cancelled".into());
    }
    let photos = after.run(&files, false);
    Ok(answer(files, photos))
}

/// `app.export`'s answer: the files (single-photo exports also report path, width and height at the
/// top level) and what Apple Photos did with them.
pub fn answer(files: Vec<Value>, apple_photos: Option<Value>) -> Value {
    let mut out = files.first().cloned().unwrap_or_else(|| json!({}));
    if let Some(photos) = apple_photos {
        out["applePhotos"] = photos;
    }
    out["files"] = json!(files);
    out
}

/// A finished batch: the files written (per photo: path… or error / skipped), what Apple Photos did
/// with them, and whether it was cancelled.
pub type Outcome = Result<(Vec<Value>, Option<Value>, bool), String>;

/// Something that writes a file (the platform's writer: sandbox access, a staging folder…), from any thread.
pub type SharedWrite = Arc<dyn Fn(&str, &[u8]) -> Result<(), String> + Send + Sync>;

/// An export running on a thread of its own, so the frontend (and the engine thread) keeps
/// answering: its row in the activity stack (✕ stops it before the next photo), its progress, and
/// its outcome. Shared by the egui app's background exports and the engine host's `app.export`.
pub struct Batch {
    pub total: usize,
    /// Photos started so far and the file in progress.
    pub progress: Arc<Mutex<(usize, String)>>,
    pub cancel: Arc<AtomicBool>,
    pub rx: Receiver<Outcome>,
    /// Its activity row (`activity.cancel` sets `cancel`); goes with the batch.
    pub guard: TaskGuard,
}

/// The other end of a [`Batch::deferred`]: what the host's own executor updates while it works (the
/// file in progress, the stack's ✕ as `cancel`) and the outcome it ends with.
pub struct Pending {
    pub progress: Arc<Mutex<(usize, String)>>,
    pub cancel: Arc<AtomicBool>,
    /// The batch's activity row.
    pub row: TaskHandle,
    tx: Sender<Outcome>,
}

impl Pending {
    /// The batch is over: [`Batch::poll`] reports `outcome`. (Dropped without this, it reports
    /// that the export stopped unexpectedly.)
    pub fn finish(self, outcome: Outcome) {
        let _ = self.tx.send(outcome);
    }

    /// Photos started so far and the file in progress, as [`Batch::status`] reports them.
    pub fn set_progress(&self, done: usize, current: &str) {
        if let Ok(mut g) = self.progress.lock() {
            *g = (done, current.to_string());
        }
    }
}

impl Batch {
    /// A batch whose work the host runs itself (a browser: no threads, so the work is a task that
    /// goes on between frames), with an activity row `label` of `total` steps. [`Batch::poll`]
    /// reports the outcome once [`Pending::finish`] is called.
    pub fn deferred(session: &Session, label: &str, total: usize) -> (Batch, Pending) {
        let progress = Arc::new(Mutex::new((0, String::new())));
        let cancel = Arc::new(AtomicBool::new(false));
        let guard = session.activity.start("export", label, Cancel::Flag(cancel.clone()));
        guard.progress(0, total as u64);
        let (tx, rx) = channel();
        let pending = Pending { progress: progress.clone(), cancel: cancel.clone(), row: guard.handle(), tx };
        (Batch { total, progress, cancel, rx, guard }, pending)
    }

    /// Run `work(progress, cancel, row)` on a worker thread (inline in a browser, which has no
    /// threads) with an activity row `label` of `total` steps.
    pub fn spawn(
        session: &Session,
        label: &str,
        total: usize,
        work: impl FnOnce(&Mutex<(usize, String)>, &Arc<AtomicBool>, TaskHandle) -> Outcome + Send + 'static,
    ) -> Result<Batch, String> {
        let (batch, pending) = Self::deferred(session, label, total);
        let (p, c, row) = (pending.progress.clone(), pending.cancel.clone(), pending.row.clone());
        let job = move || pending.finish(work(&p, &c, row));
        #[cfg(not(target_arch = "wasm32"))]
        std::thread::Builder::new().name("export".into()).spawn(job).map_err(|e| e.to_string())?;
        #[cfg(target_arch = "wasm32")]
        job();
        Ok(batch)
    }

    /// Export `prepared` in the background through `write`, then do its `after` (Add to Apple
    /// Photos: `adding` is what the row says meanwhile, in the frontend's language).
    pub fn start(session: &Session, prepared: Prepared, write: SharedWrite, adding: String) -> Result<Batch, String> {
        let Prepared { mut items, opts, to, after } = prepared;
        Self::spawn(session, "Exporting", items.len(), move |p, c, row| {
            items.iter_mut().for_each(|i| i.set_cancel(c.clone()));
            let mut w = |path: &str, bytes: &[u8]| write(path, bytes);
            let r = run_batch(items, &opts, &to, &mut w, &|path| Path::new(path).exists(), false, &mut |done, name| {
                if let Ok(mut g) = p.lock() {
                    *g = (done, name.to_string());
                }
                !c.load(Ordering::Relaxed)
            });
            r.map(|files| {
                if !after.is_none() {
                    // Photos can't be stopped once asked: no ✕ from here, before the last look at
                    // the cancel flag, so a cancel either stops the export before Photos or is refused
                    row.set_cancellable(false);
                    if let Ok(mut g) = p.lock() {
                        g.1 = adding;
                    }
                }
                let cancelled = c.load(Ordering::Relaxed);
                let photos = after.run(&files, cancelled);
                (files, photos, cancelled)
            })
        })
    }

    /// Keep the activity row current; the outcome once the batch has ended (`None` while it runs).
    pub fn poll(&self) -> Option<Outcome> {
        match self.rx.try_recv() {
            Ok(r) => Some(r),
            Err(TryRecvError::Empty) => {
                let (done, current) = self.progress.lock().map(|g| g.clone()).unwrap_or_default();
                self.guard.progress(done as u64, self.total as u64);
                self.guard.detail(&current);
                None
            }
            Err(TryRecvError::Disconnected) => Some(Err("Export stopped unexpectedly".into())),
        }
    }

    /// `{total, done, current}`, for automation.
    pub fn status(&self) -> Value {
        let (done, current) = self.progress.lock().map(|g| g.clone()).unwrap_or_default();
        json!({"total": self.total, "done": done, "current": current})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // In the background: an activity row while it runs, the files when it's done; ✕ stops it
    #[test]
    fn a_batch_runs_off_the_thread_and_can_be_stopped() {
        let dir = std::env::temp_dir().join(format!("lc-apphost-batch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = Session::with_demo();
        let ids: Vec<u64> = s.visible_cloned().iter().take(3).map(|id| id.0).collect();
        let p = json!({"dir": dir.to_string_lossy(), "ids": ids, "longEdge": 64, "format": "png"});
        let write: SharedWrite = Arc::new(lightcraft_engine::export::write_file);
        let prepared = prepare(&mut s, &p).unwrap();
        let batch = Batch::start(&s, prepared, write.clone(), "Adding…".into()).unwrap();
        assert_eq!(s.activity.list().len(), 1, "a row while it runs");
        let t0 = std::time::Instant::now();
        let outcome = loop {
            if let Some(o) = batch.poll() {
                break o;
            }
            assert!(t0.elapsed().as_secs() < 60, "the batch never ended");
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        let (files, _, cancelled) = outcome.unwrap();
        assert_eq!(files.len(), 3);
        assert!(!cancelled);
        assert!(Path::new(files[0]["path"].as_str().unwrap()).is_file());
        drop(batch);
        assert!(s.activity.list().is_empty(), "the row goes with the batch");
        // ✕ before it starts: nothing more is written
        let p = json!({"dir": dir.join("stopped").to_string_lossy(), "ids": ids, "longEdge": 64, "format": "png"});
        let prepared = prepare(&mut s, &p).unwrap();
        let batch = Batch::start(&s, prepared, write, String::new()).unwrap();
        s.activity.cancel(batch.guard.id()).unwrap();
        let (files, _, cancelled) = batch.rx.recv_timeout(std::time::Duration::from_secs(60)).unwrap().unwrap();
        assert!(cancelled);
        assert!(files.len() < 3, "{files:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // Where there are no threads the host runs the batch itself: a row, progress and ✕ as for a
    // thread, and the outcome when it says so (or "stopped unexpectedly" if it never does)
    #[test]
    fn a_deferred_batch_ends_when_the_host_says_so() {
        let s = Session::with_demo();
        let (batch, pending) = Batch::deferred(&s, "Exporting", 2);
        assert_eq!(s.activity.list().len(), 1, "a row while it runs");
        assert!(batch.poll().is_none());
        pending.set_progress(1, "b.jpg");
        assert_eq!(batch.status(), json!({"total": 2, "done": 1, "current": "b.jpg"}));
        s.activity.cancel(batch.guard.id()).unwrap();
        assert!(pending.cancel.load(Ordering::Relaxed), "the stack's ✕ reaches the host");
        pending.finish(Ok((vec![json!({"path": "a.jpg"})], None, true)));
        let (files, _, cancelled) = batch.poll().expect("ended").unwrap();
        assert_eq!((files.len(), cancelled), (1, true));
        drop(batch);
        assert!(s.activity.list().is_empty(), "the row goes with the batch");
        let (batch, pending) = Batch::deferred(&s, "Exporting", 1);
        drop(pending);
        assert_eq!(batch.poll().expect("ended").unwrap_err(), "Export stopped unexpectedly");
    }

    // A frontend's own picture stands in for the render (`hostPixels`): the file holds its pixels, and a photo it cannot stand in for
    // (another size, a wide colour space, an unreadable file) is rendered by the export itself, in the same batch
    #[test]
    fn a_hosts_picture_is_exported_in_place_of_the_render() {
        let dir = std::env::temp_dir().join(format!("lc-apphost-host-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut s = Session::with_demo();
        let ids: Vec<u64> = s.visible_cloned().iter().take(2).map(|id| id.0).collect();
        let base = json!({"ids": ids, "longEdge": 64, "format": "png"});
        let with = |extra: Value, sub: &str| {
            let mut p = base.clone();
            p["dir"] = json!(dir.join(sub).to_string_lossy());
            for (k, v) in extra.as_object().unwrap() {
                p[k] = v.clone();
            }
            p
        };
        let plain = export_now(&mut s, &with(json!({}), "plain"), &mut |_, _, _| true).unwrap();
        let (w, h) = (plain["files"][0]["width"].as_u64().unwrap() as usize, plain["files"][0]["height"].as_u64().unwrap() as usize);
        let solid = dir.join("solid.rgba");
        let pixels: Vec<u8> = std::iter::repeat_n([10u8, 200, 90, 128], w * h).flatten().collect();
        std::fs::write(&solid, &pixels).unwrap();
        let entry = |w: usize, h: usize, file: &std::path::Path| json!({"file": file.to_string_lossy(), "width": w, "height": h});
        // the pixels of a written file, upright and in sRGB
        let read = |out: &Value, i: usize| {
            let bytes = std::fs::read(out["files"][i]["path"].as_str().unwrap()).unwrap();
            let d = lightcraft_codecs::decode(&bytes, Default::default()).unwrap();
            (d.to_srgb8(), d.icc.is_some())
        };
        let is_solid = |img: &lightcraft_raster::Rgba8| img.data.iter().all(|p| *p == [10, 200, 90, 255]);
        assert!(!is_solid(&read(&plain, 0).0), "the demo photo is not a solid colour");

        // photo 1 from the host, photo 2 rendered by the export; alpha is ignored, the profile and the name are the export's
        let out = export_now(&mut s, &with(json!({"hostPixels": {ids[0].to_string(): entry(w, h, &solid)}}), "host"), &mut |_, _, _| true).unwrap();
        let (first, icc) = read(&out, 0);
        assert!(is_solid(&first) && icc, "the host's pixels, with the export's profile");
        assert_eq!((first.width, first.height), (w, h));
        assert_eq!(read(&out, 1).0, read(&plain, 1).0, "the other photo is the export's own render");
        assert_eq!(
            out["files"][0]["path"].as_str().map(|p| p.rsplit('/').next().unwrap().to_string()),
            plain["files"][0]["path"].as_str().map(|p| p.rsplit('/').next().unwrap().to_string()),
            "the name is the export's"
        );
        assert!(!s.last_export.as_ref().is_some_and(|l| l.get("hostPixels").is_some()), "hostPixels is not an option to remember");

        // a picture the export cannot use is not used: another size, a wide colour space, a file that is gone or the wrong length
        let fallbacks = [
            ("size", json!({"hostPixels": {ids[0].to_string(): entry(w + 5, h, &solid)}})),
            ("space", json!({"colorSpace": "displayP3", "hostPixels": {ids[0].to_string(): entry(w, h, &solid)}})),
            ("gone", json!({"hostPixels": {ids[0].to_string(): entry(w, h, &dir.join("nothing.rgba"))}})),
            ("short", json!({"hostPixels": {ids[0].to_string(): entry(w, h + 1, &solid)}})),
        ];
        for (name, extra) in fallbacks {
            let out = export_now(&mut s, &with(extra, name), &mut |_, _, _| true).unwrap();
            let (img, _) = read(&out, 0);
            assert!(!is_solid(&img), "{name}: rendered by the export");
        }
        // a malformed entry is refused, nothing is written
        let e = export_now(&mut s, &with(json!({"hostPixels": {ids[0].to_string(): {"file": "x"}}}), "bad"), &mut |_, _, _| true);
        assert!(e.unwrap_err().contains("hostPixels"));
        assert!(!dir.join("bad").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // A host's 16-bit picture in a wide colour space is written as it is (TIFF is 16-bit by default), and a picture whose depth or space is not
    // the file's is left to the export
    #[test]
    fn a_hosts_16_bit_picture_in_a_wide_space_is_exported_as_drawn() {
        let dir = std::env::temp_dir().join(format!("lc-apphost-host16-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut s = Session::with_demo();
        let id = s.visible_cloned()[0].0;
        let base = json!({"ids": [id], "longEdge": 64, "format": "tiff", "colorSpace": "displayP3"});
        let with = |extra: Value, sub: &str| {
            let mut p = base.clone();
            p["dir"] = json!(dir.join(sub).to_string_lossy());
            for (k, v) in extra.as_object().unwrap() {
                p[k] = v.clone();
            }
            p
        };
        let plain = export_now(&mut s, &with(json!({}), "plain"), &mut |_, _, _| true).unwrap();
        let (w, h) = (plain["files"][0]["width"].as_u64().unwrap() as usize, plain["files"][0]["height"].as_u64().unwrap() as usize);
        let sample = [0x1234u16, 0x8001, 0xF0F0];
        let file = dir.join("solid16.rgba");
        let bytes: Vec<u8> = std::iter::repeat_n([sample[0], sample[1], sample[2], 7], w * h).flatten().flat_map(u16::to_ne_bytes).collect();
        std::fs::write(&file, &bytes).unwrap();
        let entry = |depth: u64, space: &str| json!({"file": file.to_string_lossy(), "width": w, "height": h, "depth": depth, "space": space});
        let decoded = |out: &Value| {
            let bytes = std::fs::read(out["files"][0]["path"].as_str().unwrap()).unwrap();
            lightcraft_codecs::decode(&bytes, Default::default()).unwrap()
        };
        let is_solid = |d: &lightcraft_codecs::Decoded| {
            // (the sRGB curve, which Display P3 shares, decoded)
            let decode = |e: f32| if e <= 0.04045 { e / 12.92 } else { ((e + 0.055) / 1.055).powf(2.4) };
            let want = sample.map(|v| decode(f32::from(v) / 65_535.0));
            d.image.row(0).iter().all(|p| (0..3).all(|i| (p[i] - want[i]).abs() < 2e-4))
        };

        let out = export_now(&mut s, &with(json!({"hostPixels": {id.to_string(): entry(16, "displayP3")}}), "host"), &mut |_, _, _| true).unwrap();
        let d = decoded(&out);
        assert_eq!(d.bit_depth, 16);
        assert!(d.icc.is_some() && is_solid(&d), "the host's 16-bit samples, with the export's Display P3 profile");
        assert!(!is_solid(&decoded(&plain)), "the demo photo is not a solid colour");

        for (name, extra) in [
            ("depth8", json!({"hostPixels": {id.to_string(): entry(8, "displayP3")}})),
            ("srgb", json!({"hostPixels": {id.to_string(): entry(16, "srgb")}})),
            ("png8", json!({"format": "png", "hostPixels": {id.to_string(): entry(16, "displayP3")}})),
            ("float", json!({"bitDepth": 32, "hostPixels": {id.to_string(): entry(16, "displayP3")}})),
        ] {
            let out = export_now(&mut s, &with(extra, name), &mut |_, _, _| true).unwrap();
            assert!(!is_solid(&decoded(&out)), "{name}: rendered by the export");
        }
        for bad in [entry(12, "displayP3"), entry(16, "nowhere")] {
            let e = export_now(&mut s, &with(json!({"hostPixels": {id.to_string(): bad}}), "bad"), &mut |_, _, _| true);
            assert!(e.unwrap_err().contains("hostPixels"));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    // A frontend with an encoder of its own writes finished files (`hostFiles`): HEIC, which this build cannot encode, is placed as any other file, under the
    // export's naming and conflict policy; a photo it wrote none for is an error of that photo, not of the batch
    #[test]
    fn a_hosts_finished_file_is_placed_under_the_exports_name() {
        let dir = std::env::temp_dir().join(format!("lc-apphost-hostfile-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut s = Session::with_demo();
        let ids: Vec<u64> = s.visible_cloned().iter().take(2).map(|id| id.0).collect();
        let sized =
            export_now(&mut s, &json!({"ids": ids, "longEdge": 64, "format": "png", "dir": dir.join("size").to_string_lossy()}), &mut |_, _, _| true)
                .unwrap();
        // (each photo has its own size)
        let dims = |i: usize| (sized["files"][i]["width"].as_u64().unwrap(), sized["files"][i]["height"].as_u64().unwrap());
        let ((w, h), (w2, h2)) = (dims(0), dims(1));
        let heic = dir.join("one.heic");
        let heic2 = dir.join("two.heic");
        std::fs::write(&heic, b"ftypheic pretend").unwrap();
        std::fs::write(&heic2, b"ftypheic second").unwrap();
        let entry = |w: u64, h: u64, file: &std::path::Path| json!({"file": file.to_string_lossy(), "width": w, "height": h});
        let params = |extra: Value, sub: &str| {
            let mut p = json!({"ids": ids, "longEdge": 64, "format": "heic", "dir": dir.join(sub).to_string_lossy()});
            for (k, v) in extra.as_object().unwrap() {
                p[k] = v.clone();
            }
            p
        };
        let both = json!({"hostFiles": {ids[0].to_string(): entry(w, h, &heic), ids[1].to_string(): entry(w2, h2, &heic2)}});
        let out = export_now(&mut s, &params(both, "host"), &mut |_, _, _| true).unwrap();
        let files = out["files"].as_array().unwrap();
        let path = files[0]["path"].as_str().unwrap();
        assert!(path.ends_with(".heic"), "the export's name and extension: {path}");
        assert_eq!(std::fs::read(path).unwrap(), b"ftypheic pretend", "the host's bytes, untouched");
        assert_eq!(std::fs::read(files[1]["path"].as_str().unwrap()).unwrap(), b"ftypheic second");
        assert_eq!((files[0]["width"].as_u64(), files[0]["height"].as_u64(), files[0]["bytes"].as_u64()), (Some(w), Some(h), Some(16)));
        // a photo the host wrote no file for, another size, a file that is gone: the export has no HEIC encoder, so that photo is an error
        let one = json!({"hostFiles": {ids[0].to_string(): entry(w, h, &heic)}});
        let gone = json!({"hostFiles": {ids[0].to_string(): entry(w, h, &dir.join("no.heic")), ids[1].to_string(): entry(w2, h2, &heic2)}});
        let size = json!({"hostFiles": {ids[0].to_string(): entry(w + 9, h, &heic), ids[1].to_string(): entry(w2, h2, &heic2)}});
        for (name, extra, want) in [("none", one, "HEIC"), ("size", size, "HEIC"), ("gone", gone, "gone")] {
            let e = export_now(&mut s, &params(extra, name), &mut |_, _, _| true).unwrap_err();
            assert!(e.contains(want), "{name}: {e}");
        }
        let e = export_now(&mut s, &params(json!({"hostFiles": {ids[0].to_string(): {"file": "x"}}}), "bad"), &mut |_, _, _| true);
        assert!(e.unwrap_err().contains("hostFiles"));
        // a file for a format the export does encode is placed too (a frontend that encodes JPEG itself), and a second run of the same name is made unique
        let jpeg = dir.join("one.jpg");
        std::fs::write(&jpeg, b"\xff\xd8 pretend").unwrap();
        let mut twice = params(json!({"format": "jpeg", "hostFiles": {ids[0].to_string(): entry(w, h, &jpeg)}}), "jpeg");
        twice["ids"] = json!([ids[0]]);
        let first = export_now(&mut s, &twice, &mut |_, _, _| true).unwrap();
        let second = export_now(&mut s, &twice, &mut |_, _, _| true).unwrap();
        assert!(first["files"][0]["path"].as_str().unwrap().ends_with(".jpg") && second["files"][0]["path"].as_str().unwrap().contains("-2.jpg"));
        assert_eq!(std::fs::read(first["files"][0]["path"].as_str().unwrap()).unwrap(), b"\xff\xd8 pretend");
        assert!(!s.last_export.as_ref().is_some_and(|l| l.get("hostFiles").is_some()), "hostFiles is not an option to remember");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // Export Now writes the active photo at the default size, reports it, and can be stopped
    #[test]
    fn export_now_writes_reports_and_stops_when_asked() {
        let dir = std::env::temp_dir().join(format!("lc-apphost-export-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = Session::with_demo();
        let first = s.visible_cloned()[0];
        s.execute("library.select", &json!({"ids": [first.0]})).unwrap();
        let p = json!({"dir": dir.to_string_lossy(), "longEdge": 64, "format": "png"});
        let mut calls = Vec::new();
        let out = export_now(&mut s, &p, &mut |done, total, _| {
            calls.push((done, total));
            true
        })
        .unwrap();
        assert_eq!(out["files"].as_array().map(Vec::len), Some(1), "{out}");
        assert_eq!(out["files"][0]["photo"].as_u64(), Some(first.0), "each file says which photo it is of");
        assert!(Path::new(out["path"].as_str().unwrap()).is_file());
        assert_eq!(calls.last(), Some(&(1, 1)));
        assert!(command_info(&s)["enabled"].as_bool().unwrap());
        let e =
            export_now(&mut s, &json!({"dir": dir.to_string_lossy(), "longEdge": 64, "format": "png", "conflict": "unique"}), &mut |_, _, _| false);
        assert_eq!(e.unwrap_err(), "cancelled");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
