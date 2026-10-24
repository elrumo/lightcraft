//! `app.export` without a window: Export Now with the desktop app's parameters, for frontends that
//! have no export dialog of their own yet (the MCP server's headless backend, native frontends).

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::{Arc, Mutex};

use lightcraft_catalog::PhotoId;
use lightcraft_engine::Session;
use lightcraft_engine::activity::{Cancel, TaskGuard, TaskHandle};
use lightcraft_engine::export::{AfterExport, Destination, ExportFormat, ExportOptions, PreparedExport, Resize, prepare_batch, run_batch};
use serde_json::{Value, json};

/// The parameters `app.export` takes (as `engine.commands` lists them).
pub const PARAMS: &str = "{path?: output file (.jpg/.png/.tif/.webp/.avif/.jxl/.psd/.dng) | dir?, ids?, preset?, format?: jpeg|png|tiff|webp|avif|jxl|psd|dng|original, longEdge?|shortEdge?|width?|height?|megapixels?|percent? (default longEdge 3000; longEdge 0 = full size), fit?: dimensions|widthHeight (with width + height: dimensions fits either orientation, widthHeight limits the width to `width` and the height to `height`), dontEnlarge?, ppi?, quality?: 1..100, jxlLossless?, jxlEffort?: fast|normal, jxlProgressive?, limitKb?, colorSpace?: srgb|displayP3|adobeRgb|proPhoto|rec2020, bitDepth?: 8|10|16|32, hdr? (HDR output for HDR edits: gain map JPEG, PQ AVIF / PNG / JPEG XL, float TIFF), sharpen?: none|screen|matte|glossy, sharpenAmount?: low|standard|high, metadata?: all|allExceptCamera|copyright|none, removeLocation?, naming?, startNumber?, destinationMode?: specificFolder|sameAsOriginal (sameAsOriginal = each photo into its original's folder; `dir` unused), subfolder?, conflict?: unique|overwrite|skip, tiffCompression?: none|lzw|zip, dngCompression?: lossless|deflate|uncompressed, addToPhotos?: bool (macOS: then add the files written to Apple Photos; outcome in `applePhotos`), photosAlbum?: top-level album name (made when missing), onServer?: render on the sync server, watermark?: text | {text?, vertical? (upright columns, right to left), size? (text height, 0.005..0.5 of the short edge; default 0.035), opacity? (0..1; 0.7), anchor?: topLeft|top|topRight|left|center|right|bottomLeft|bottom|bottomRight, insetX? (margin from the left/right edge, 0..0.4 of the short edge; 0.025), insetY? (from the top/bottom edge; 0.025), inset? (sets both), rotation?: 0|90|180|270 (degrees clockwise; 0), color? [r,g,b] sRGB, shadow?, image? (graphic drawn instead of the text), imageWidth? (0.01..1 of the photo's width; 0.2)}} — an unknown parameter, an out-of-range watermark size or a value of the wrong kind is an error, not a default";

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
    let p = &session.export_params_or(p, ExportFormat::Png)?;
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
    let items = prepare_batch(session, &ids, &opts)?;
    Ok(Prepared { items, opts, to: Destination { dir: dir.to_string(), exact: exact.map(str::to_string) }, after })
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
