//! Video export: a clip written again with its edits as an H.264 MP4: the kept part (trim), the develop settings applied to
//! every picture, a size and a quality, and the clip's sound copied as it is. Runs in the background with a row in the
//! activity stack (`video.export`, `video.exportStatus`); a cancelled or failed export leaves nothing behind.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use lightcraft_catalog::{MediaKind, PhotoId, Source};
use lightcraft_develop::DevelopSettings;
use lightcraft_pipeline::{RenderRequest, SourceInfo};
use lightcraft_raster::Rgba8;
use serde_json::{Value, json};

use crate::slideshow_export::PartFile;

/// How small the file is made: the encoder's constant quality (lower is better and bigger).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    Smaller,
    Balanced,
    Better,
}

impl Quality {
    fn parse(s: &str) -> Option<Quality> {
        match s {
            "smaller" => Some(Quality::Smaller),
            "balanced" => Some(Quality::Balanced),
            "better" => Some(Quality::Better),
            _ => None,
        }
    }

    fn crf(self) -> f32 {
        match self {
            Quality::Smaller => 28.0,
            Quality::Balanced => 23.0,
            Quality::Better => 19.0,
        }
    }
}

/// A video export ready to run: everything it needs, detached from the session.
pub struct Prepared {
    pub path: String,
    source: String,
    settings: Arc<DevelopSettings>,
    info: SourceInfo,
    start: f64,
    end: f64,
    long_edge: usize,
    quality: Quality,
    apply_edits: bool,
    guard: crate::originals::OriginalGuard,
    /// The pictures the part kept holds, at the clip's own rate.
    pub frames: u64,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
}

/// Prepare the export `p` asks for (`id?`, `path`, `maxEdge?`, `quality?`, `edits?`, `trim?`).
pub fn prepare(s: &mut crate::Session, p: &Value) -> Result<Prepared, String> {
    let path = p.get("path").and_then(Value::as_str).map(str::trim).filter(|x| !x.is_empty()).ok_or("missing `path`")?;
    if !path.to_lowercase().ends_with(".mp4") {
        return Err("the file must be an .mp4".into());
    }
    let id = match p.get("id").filter(|v| !v.is_null()) {
        Some(v) => PhotoId(v.as_u64().ok_or("`id` must be a photo id")?),
        None => s.active().ok_or("no photo selected: pass `id`")?,
    };
    let photo = s.catalog.photo(id).ok_or_else(|| format!("no photo {}", id.0))?.clone();
    if photo.kind != MediaKind::Video {
        return Err(format!("photo {} is not a video", id.0));
    }
    let Source::File { path: source } = &photo.source else { return Err("this clip is not a file".into()) };
    if source.starts_with(lightcraft_catalog::sync::PATH_PREFIX) || !Path::new(source).is_file() {
        return Err("the clip's file is not on this computer (download the original first)".into());
    }
    let clip = lightcraft_video::Clip::open_path(Path::new(source)).map_err(|e| e.to_string())?;
    let v = clip.info().clone();
    if let Some(why) = &v.unsupported {
        return Err(why.clone());
    }
    let quality = match p.get("quality").and_then(Value::as_str) {
        None => Quality::Balanced,
        Some(q) => Quality::parse(q).ok_or("`quality` is smaller, balanced or better")?,
    };
    let long_edge = match p.get("maxEdge").filter(|v| !v.is_null()) {
        None => v.width.max(v.height) as usize,
        Some(e) => {
            let e = e.as_f64().filter(|e| e.is_finite() && (64.0..=8192.0).contains(e)).ok_or("`maxEdge` must be between 64 and 8192")?;
            e as usize
        }
    };
    let trim = p.get("trim").and_then(Value::as_bool).unwrap_or(true);
    let apply_edits = p.get("edits").and_then(Value::as_bool).unwrap_or(true);
    let edit = photo.develop.video.filter(|_| trim).unwrap_or_default();
    let length = photo.duration.filter(|d| d.is_finite() && *d > 0.0).unwrap_or(v.duration);
    let (start, end) = (edit.trim_start.unwrap_or(0.0), edit.trim_end.unwrap_or(length).min(length));
    if end - start < 0.05 {
        return Err("the kept part of the clip is empty".into());
    }
    let fps = if v.fps > 1.0 { v.fps } else { 30.0 };
    // (the output is the size the crop and the long edge give; the first picture says exactly)
    let scale = (long_edge as f64 / f64::from(v.width.max(v.height)).max(1.0)).min(1.0);
    s.check_write_target(path)?;
    Ok(Prepared {
        path: path.to_string(),
        source: source.clone(),
        settings: photo.develop.clone(),
        info: crate::media::source_info(&photo),
        start,
        end,
        long_edge,
        quality,
        apply_edits,
        guard: s.original_guard(),
        frames: ((end - start) * fps).round().max(1.0) as u64,
        width: (f64::from(v.width) * scale).round() as u32,
        height: (f64::from(v.height) * scale).round() as u32,
        fps,
    })
}

const CANCELLED: &str = "Video export cancelled";

/// A decoded picture (linear Rec.2020) as 8-bit sRGB: with the develop settings applied and the crop (`edits`), the way
/// an export shows it, else as recorded.
pub(crate) fn finish_picture(
    picture: lightcraft_raster::Rgb32f,
    info: &SourceInfo,
    settings: &DevelopSettings,
    req: &RenderRequest,
    edits: bool,
) -> Rgba8 {
    if edits {
        return crate::media::develop(&Arc::new(picture), info, settings, req, None, true).image;
    }
    let m = lightcraft_color::WORKING.to_space(&lightcraft_color::SRGB).to_f32();
    let enc = lightcraft_color::transfer::encode_srgb8;
    Rgba8::from_fn(picture.width, picture.height, |x, y| {
        let px = picture.get(x, y);
        let q = [0, 1, 2].map(|r| m[r][0] * px[0] + m[r][1] * px[1] + m[r][2] * px[2]);
        [enc(q[0]), enc(q[1]), enc(q[2]), 255]
    })
}

/// Make an even size of `img` (the encoder needs one) by dropping a last odd column or row.
fn even(img: Rgba8) -> Rgba8 {
    let (w, h) = (img.width & !1, img.height & !1);
    if (w, h) == (img.width, img.height) || w == 0 || h == 0 {
        return img;
    }
    Rgba8::from_fn(w, h, |x, y| img.get(x, y))
}

impl Prepared {
    /// Write the file: `progress(done, total)` after each picture; stops (leaving nothing) when `cancel` is set.
    pub fn run(self, progress: &dyn Fn(u64, u64), cancel: &AtomicBool) -> Result<Value, String> {
        let clip = lightcraft_video::Clip::open_path(Path::new(&self.source)).map_err(|e| e.to_string())?;
        let sound = clip.audio_track();
        let (part, file) = PartFile::new(Path::new(&self.path))?;
        let mut encoder: Option<lightcraft_video::Mp4Encoder<std::io::BufWriter<std::fs::File>>> = None;
        let mut file = Some(std::io::BufWriter::new(file));
        let (mut done, mut size) = (0u64, (0usize, 0usize));
        let req = RenderRequest::fit(self.long_edge, self.long_edge);
        let settings = self.settings.clone();
        clip.for_each_frame(self.start, self.end, self.long_edge, |seconds, picture| {
            if cancel.load(Ordering::Relaxed) {
                return Err(lightcraft_video::Error::Io(CANCELLED.into()));
            }
            let frame = finish_picture(picture, &self.info, &settings, &req, self.apply_edits);
            let frame = even(frame);
            if encoder.is_none() {
                size = (frame.width, frame.height);
                let options =
                    lightcraft_video::EncodeOptions { width: frame.width, height: frame.height, fps: self.fps, crf: self.quality.crf(), bframes: 2 };
                let out = file.take().ok_or_else(|| lightcraft_video::Error::Io("the file is gone".into()))?;
                encoder = Some(lightcraft_video::Mp4Encoder::new(out, options, sound.as_ref())?);
            }
            if (frame.width, frame.height) != size {
                return Err(lightcraft_video::Error::Malformed("the pictures of the clip changed size".into()));
            }
            if let Some(e) = &mut encoder {
                e.push(&frame, (seconds - self.start).max(0.0))?;
            }
            done += 1;
            progress(done, self.frames.max(done));
            Ok(true)
        })
        .map_err(|e| match e {
            lightcraft_video::Error::Io(m) if m == CANCELLED => CANCELLED.to_string(),
            e => e.to_string(),
        })?;
        let mut encoder = encoder.ok_or("the clip has no pictures in the part kept")?;
        let copied = match &sound {
            Some(track) => encoder.copy_audio(&clip, track, self.start, self.end).map_err(|e| e.to_string())?,
            None => 0,
        };
        encoder.finish().map_err(|e| e.to_string())?;
        part.commit(&self.guard)?;
        let bytes = std::fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0);
        Ok(
            json!({"path": self.path, "frames": done, "width": size.0, "height": size.1, "seconds": self.end - self.start, "bytes": bytes, "audio": copied > 0}),
        )
    }
}

/// Run `prepared` to the end, recording progress and the outcome in `run` (and the row).
pub fn run_to_end(prepared: Prepared, run: &crate::slideshow_export::ExportRun, task: &crate::activity::TaskGuard) {
    let handle = task.handle();
    let progress = |done: u64, total: u64| {
        run.done.store(done, Ordering::Relaxed);
        run.total.store(total, Ordering::Relaxed);
        handle.progress(done, total);
    };
    // a panic in a decoder, the pipeline or the encoder is an error, never a crash (the part file goes with it)
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| prepared.run(&progress, &run.cancel)))
        .unwrap_or_else(|_| Err("video export failed unexpectedly".into()));
    *run.result.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(r);
    run.finished.store(true, Ordering::Relaxed);
}

/// The `video.export` command's body; `wait` runs it here (scripts, tests).
pub fn start(s: &mut crate::Session, p: &Value, wait: bool) -> Result<Value, String> {
    if s.video_export.as_ref().is_some_and(|r| r.running()) {
        return Err("a video export is already running (activity.cancel stops it)".into());
    }
    let prepared = prepare(s, p)?;
    let cancel = Arc::new(AtomicBool::new(false));
    let task = s.activity.start("export", "Exporting video", crate::activity::Cancel::Flag(cancel.clone()));
    task.progress(0, prepared.frames);
    task.detail(&prepared.path);
    let run = Arc::new(crate::slideshow_export::ExportRun {
        task_id: task.id(),
        format: "mp4".into(),
        path: prepared.path.clone(),
        total: prepared.frames.into(),
        cancel,
        ..Default::default()
    });
    s.video_export = Some(run.clone());
    let started = json!({"taskId": task.id(), "background": !wait, "path": prepared.path, "frames": prepared.frames, "width": prepared.width, "height": prepared.height});
    if wait {
        run_to_end(prepared, &run, &task);
        drop(task);
        return match run.result() {
            Some(r) => r,
            None => Err("the export did not finish".into()),
        };
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        // the row goes when the worker ends, however it ends
        let r = run.clone();
        std::thread::Builder::new()
            .name("lc-video-export".into())
            .spawn(move || run_to_end(prepared, &r, &task))
            .map_err(|e| format!("could not start: {e}"))?;
    }
    Ok(started)
}
