//! AI Super Resolution in the background: the enlargement runs on a worker thread with its
//! activity row (progress, and a ✕ that stops it), and the result is written, added to the library
//! and selected when it finishes ([`Session::finish_super_res`]). The egui app's Enhance dialog
//! and the engine host (`engine.execute {command: "enhance.superRes"}`) both run it this way.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use lightcraft_engine::Session;
use lightcraft_engine::activity::{Cancel, TaskGuard};
use lightcraft_engine::catalog::PhotoId;
use lightcraft_engine::enhance::SuperResDone;
use serde_json::Value;

/// An enlargement under way.
pub struct Running {
    pub photo: PhotoId,
    cancel: Arc<AtomicBool>,
    rx: Receiver<Result<SuperResDone, String>>,
    /// Its activity row (`activity.cancel` sets `cancel`); goes with the job.
    _guard: TaskGuard,
}

impl Running {
    /// Plan the enlargement of photo `id` (the model must be installed, the result within the
    /// size limit) and start it on a worker thread. `dir`: where a photo without a file of its own
    /// is written (demo photos).
    pub fn start(session: &mut Session, id: PhotoId, dir: Option<&str>) -> Result<Running, String> {
        let job = session.super_res_job(id, dir)?;
        let cancel = Arc::new(AtomicBool::new(false));
        let guard = session.activity.start("superRes", "Enlarging", Cancel::Flag(cancel.clone()));
        guard.progress(0, 100);
        let (tx, rx) = channel();
        let (c, row) = (cancel.clone(), guard.handle());
        let work = move || {
            let run = || {
                job.run(&|f, _| {
                    row.progress((f.clamp(0.0, 1.0) * 100.0) as u64, 100);
                    !c.load(Ordering::Relaxed)
                })
            };
            // a panicking job reports an error instead of leaving "Enlarging" up for ever
            let r =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(run)).unwrap_or_else(|_| Err("Super Resolution failed unexpectedly".into()));
            let _ = tx.send(r);
        };
        #[cfg(not(target_arch = "wasm32"))]
        std::thread::Builder::new().name("super-resolution".into()).spawn(work).map_err(|e| e.to_string())?;
        #[cfg(target_arch = "wasm32")]
        work();
        Ok(Running { photo: id, cancel, rx, _guard: guard })
    }

    /// Stop it (nothing is written).
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// Once it has finished: the enlargement added to the library and selected
    /// (`{id, path, original, width, height}`), or why not (`"cancelled"` when it was stopped).
    pub fn poll(&self, session: &mut Session) -> Option<Result<Value, String>> {
        let done = match self.rx.try_recv() {
            Ok(r) => r,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => Err("Super Resolution stopped unexpectedly".into()),
        };
        Some(done.and_then(|d| session.execute_fn("enhance.superRes", |s| s.finish_super_res(d)).map_err(|e| e.to_string())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightcraft_engine::enhance::{Enhancer, SUPER_RES_MODEL};
    use serde_json::json;

    /// With the model in place, an enlargement runs on its worker with its activity row, then is
    /// added stacked on the original and selected; a stopped one adds nothing.
    #[test]
    fn super_resolution_runs_in_the_background() {
        if !Enhancer::AVAILABLE {
            return;
        }
        let dir = std::env::temp_dir().join(format!("lc-apphost-superres-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (w, h) = (60usize, 40usize);
        let rgb: Vec<u8> = (0..w * h).flat_map(|i| [(i % w * 255 / w) as u8, (i / w * 255 / h) as u8, 90]).collect();
        let png = lightcraft_codecs::encode_png(
            &lightcraft_codecs::EncodeImage::new(w as u32, h as u32, 3, lightcraft_codecs::Samples::U8(&rgb)),
            &lightcraft_codecs::EncodeMeta::default(),
        )
        .unwrap();
        let path = dir.join("IMG_7.png");
        std::fs::write(&path, png).unwrap();
        let mut s = Session::new().with_fs();
        let r = s.execute("library.import", &json!({"paths": [path]})).unwrap();
        let id = PhotoId(r["imported"][0].as_u64().unwrap());
        s.enhancer.dir = Some(dir.join("models"));
        s.enhancer.no_builtin_mirrors = true;
        // no model: refused at once
        assert!(Running::start(&mut s, id, None).is_err());
        let file = s.enhancer.model_file(SUPER_RES_MODEL).unwrap();
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        lightcraft_enhance::testing::write_synthetic_model(&file, 8, 2).unwrap();
        // stopped: nothing added
        let job = Running::start(&mut s, id, None).unwrap();
        job.cancel();
        let r = loop {
            if let Some(r) = job.poll(&mut s) {
                break r;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        assert!(r.is_err(), "{r:?}");
        assert_eq!(s.catalog.photos().count(), 1);
        drop(job);
        // run to the end: a row while it works, the photo added after
        let job = Running::start(&mut s, id, None).unwrap();
        assert!(s.activity.list().iter().any(|t| t.kind == "superRes"), "its activity row");
        let t = std::time::Instant::now();
        let v = loop {
            if let Some(r) = job.poll(&mut s) {
                break r.unwrap();
            }
            assert!(t.elapsed() < std::time::Duration::from_secs(60), "it never finished");
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        let new = PhotoId(v["id"].as_u64().unwrap());
        assert_eq!(s.catalog.photo(new).map(|p| (p.width, p.height)), Some((120, 80)));
        assert_eq!(s.selection.active, Some(new));
        drop(job);
        assert!(!s.activity.list().iter().any(|t| t.kind == "superRes"), "the row goes with the job");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
