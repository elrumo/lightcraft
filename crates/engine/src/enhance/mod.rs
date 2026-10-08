//! AI **Super Resolution**: enlarge a photo 2× with a neural network (`lightcraft-enhance`) and
//! add the result to the library, stacked on the original, like Lightroom's Enhance.
//!
//! The model is optional (Nomos Uni SPAN 2×, a ~4.5 MB download): it is not part of LightCraft,
//! nothing requires it, and without it the feature says so. The download needs the user's
//! consent (`enhance.model.download {acknowledged: true}`) and is verified before it is used
//! (`lightcraft-models`).
//!
//! The photo is rendered with its settings at full size (sRGB, 16 bit), enlarged tile by tile,
//! and written next to the original as a 16-bit TIFF `<name>-SR.tif` (never replacing a file),
//! then imported with neutral settings: its pixels already carry the edits and the crop.
//! [`Session::super_res_job`] plans the work, [`SuperResJob::run`] does the slow part (on any
//! thread, with progress and cancellation) and [`Session::finish_super_res`] adds the photo.

use std::path::PathBuf;

use lightcraft_catalog::{PhotoId, Source};
use serde::Serialize;
use serde_json::{Value, json};

use crate::media::RenderJob;
use crate::merge::ProgressFn;
use crate::segment::DownloadStatus;
use crate::{EngineError, Result, Session};

/// The model Super Resolution uses.
pub const SUPER_RES_MODEL: &str = "nomos-span-2x";
/// How much that model enlarges.
pub const SUPER_RES_SCALE: usize = 2;
/// Most pixels the enlarged image may have unless the app says otherwise (a 25 MP photo).
pub const DEFAULT_MAX_OUTPUT_PIXELS: usize = 100_000_000;

/// One downloadable model as the apps show it.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelStatus {
    pub id: String,
    pub label: String,
    pub installed: bool,
    pub size_bytes: u64,
    pub licence: String,
    pub licence_url: String,
    /// Credit the licence asks for.
    pub credit: String,
    pub home_url: String,
    /// The model's folder.
    pub dir: Option<String>,
    /// Download locations known (the user's own and the built-in ones).
    pub mirrors: usize,
    /// This model is the one being downloaded (or the last one downloaded).
    pub downloading: bool,
}

/// The enhancement models and what they need from the app.
#[derive(Default)]
pub struct Enhancer {
    /// The models folder: a model lives in `<dir>/<id>/`, its extra mirrors in
    /// `<dir>/<id>-mirrors.txt`. Set by the app; `None`: no AI enhancement here.
    pub dir: Option<PathBuf>,
    /// Most pixels an enlarged image may have (`None`: [`DEFAULT_MAX_OUTPUT_PIXELS`]). Phones
    /// set less.
    pub max_output_pixels: Option<usize>,
    /// Download only from the user's own mirrors (the environment variable and
    /// `<id>-mirrors.txt`), never from the built-in location. For tests and for people who want
    /// no connection to any host they didn't choose.
    pub no_builtin_mirrors: bool,
    #[cfg(feature = "enhance")]
    download: lightcraft_models::download::Downloader,
    /// The model the downloader was last started for.
    #[cfg(feature = "enhance")]
    fetching: std::sync::Mutex<Option<&'static str>>,
}

impl Enhancer {
    /// Whether this build can enlarge photos at all.
    pub const AVAILABLE: bool = cfg!(feature = "enhance");

    /// The folder of model `id`.
    pub fn model_dir(&self, id: &str) -> Option<PathBuf> {
        self.dir.as_ref().map(|d| d.join(id))
    }

    /// The enhancement models (not SAM 3, which has its own commands).
    #[cfg(feature = "enhance")]
    pub fn models() -> impl Iterator<Item = &'static lightcraft_models::ModelSpec> {
        lightcraft_models::registry::ALL.iter().copied().filter(|m| m.id != lightcraft_models::registry::SAM3.id)
    }

    #[cfg(feature = "enhance")]
    fn spec(id: &str) -> std::result::Result<&'static lightcraft_models::ModelSpec, String> {
        Self::models().find(|m| m.id == id).ok_or_else(|| {
            let known: Vec<&str> = Self::models().map(|m| m.id).collect();
            format!("no enhancement model `{id}` (known: {})", known.join(", "))
        })
    }

    /// Whether the model's files are in place (a hand-placed file is checked when it is loaded).
    pub fn installed(&self, id: &str) -> bool {
        #[cfg(feature = "enhance")]
        if let (Some(dir), Ok(spec)) = (self.model_dir(id), Self::spec(id)) {
            return spec.files.iter().all(|f| std::fs::metadata(dir.join(f.name)).is_ok_and(|m| m.is_file() && m.len() > 0 && m.len() <= f.max));
        }
        let _ = id;
        false
    }

    /// The model's first (for these models: only) file.
    pub fn model_file(&self, id: &str) -> Option<PathBuf> {
        #[cfg(feature = "enhance")]
        return Self::spec(id).ok().and_then(|s| s.files.first()).and_then(|f| self.model_dir(id).map(|d| d.join(f.name)));
        #[cfg(not(feature = "enhance"))]
        {
            let _ = id;
            None
        }
    }

    /// Where model `id` is downloaded from, in order: the user's, then the built-in ones.
    pub fn mirrors(&self, id: &str) -> Vec<String> {
        #[cfg(feature = "enhance")]
        if let Ok(spec) = Self::spec(id) {
            let env = std::env::var(spec.mirrors_env).ok();
            let file = self.dir.as_ref().map(|d| d.join(format!("{id}-mirrors.txt")));
            let builtin = if self.no_builtin_mirrors { &[] } else { spec.default_mirrors };
            return lightcraft_models::fetch::mirrors(env.as_deref(), file.as_deref(), builtin);
        }
        let _ = id;
        Vec::new()
    }

    /// The models and their state.
    pub fn status(&self) -> Vec<ModelStatus> {
        #[cfg(feature = "enhance")]
        {
            let fetching = self.fetching.lock().map(|f| *f).unwrap_or_else(|e| *e.into_inner());
            Self::models()
                .map(|m| ModelStatus {
                    id: m.id.into(),
                    label: m.label.into(),
                    installed: self.installed(m.id),
                    size_bytes: m.bytes(),
                    licence: m.licence.into(),
                    licence_url: m.licence_url.into(),
                    credit: m.credit.into(),
                    home_url: m.home_url.into(),
                    dir: self.model_dir(m.id).map(|d| d.display().to_string()),
                    mirrors: self.mirrors(m.id).len(),
                    downloading: fetching == Some(m.id),
                })
                .collect()
        }
        #[cfg(not(feature = "enhance"))]
        Vec::new()
    }

    /// The download's state, and which model it is for.
    pub fn download_status(&self) -> (Option<String>, DownloadStatus) {
        #[cfg(feature = "enhance")]
        {
            let s = self.download.status();
            let id = self.fetching.lock().map(|f| *f).unwrap_or_else(|e| *e.into_inner()).map(String::from);
            (id, DownloadStatus { running: s.running, done: s.done, total: s.total, file: s.file, error: s.error, finished: s.finished })
        }
        #[cfg(not(feature = "enhance"))]
        (None, DownloadStatus::default())
    }

    /// Start downloading model `id` on a background thread. `Ok(false)` when it is installed or
    /// a download is running already. The caller must have the user's consent.
    pub fn start_download(&self, id: &str) -> std::result::Result<bool, String> {
        #[cfg(feature = "enhance")]
        {
            let spec = Self::spec(id)?;
            let dir = self.model_dir(id).ok_or("no folder is set for the enhancement models")?;
            if self.installed(id) {
                return Ok(false);
            }
            let mirrors = self.mirrors(id);
            if mirrors.is_empty() {
                return Err(spec.no_mirrors_message());
            }
            let started = self.download.start(spec.files, mirrors, dir, lightcraft_models::fetch::Options::default())?;
            if started {
                *self.fetching.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(spec.id);
            }
            Ok(started)
        }
        #[cfg(not(feature = "enhance"))]
        {
            let _ = id;
            Err("AI enhancement is not available in this build".into())
        }
    }

    /// Stop a running download (what has arrived is kept, to resume). False when none runs.
    pub fn cancel_download(&self) -> bool {
        #[cfg(feature = "enhance")]
        return self.download.cancel();
        #[cfg(not(feature = "enhance"))]
        false
    }

    /// Why Super Resolution can't run now, or `Ok(())`.
    pub fn super_res_ready(&self) -> std::result::Result<(), String> {
        if !Self::AVAILABLE {
            return Err("AI Super Resolution is not available in this build".into());
        }
        if self.dir.is_none() {
            return Err("no folder is set for the enhancement models".into());
        }
        if self.installed(SUPER_RES_MODEL) {
            return Ok(());
        }
        let dir = self.model_dir(SUPER_RES_MODEL).map(|d| d.display().to_string()).unwrap_or_default();
        Err(format!(
            "The Super Resolution model is not installed. Download it (about 4.5 MB, CC-BY-4.0) when LightCraft offers it, with `enhance.model.download {{\"id\": \"{SUPER_RES_MODEL}\", \"acknowledged\": true}}`, or put the .safetensors file in {dir}."
        ))
    }
}

/// The slow part of Super Resolution, ready to run on any thread.
#[cfg_attr(not(feature = "enhance"), allow(dead_code))]
pub struct SuperResJob {
    render: RenderJob,
    export: crate::export::ExportOptions,
    meta: Option<lightcraft_meta::Metadata>,
    model: PathBuf,
    /// Folder and name stem of the output file.
    dir: PathBuf,
    stem: String,
    max_output_pixels: usize,
    /// The photo being enlarged.
    pub photo: PhotoId,
    /// Its size (what is rendered): the result is [`SUPER_RES_SCALE`] times that.
    pub size: (usize, usize),
}

/// A finished enlargement, written and waiting to be added to the library.
#[derive(Clone, Debug, PartialEq)]
pub struct SuperResDone {
    pub photo: PhotoId,
    pub path: String,
    pub size: (usize, usize),
}

impl Session {
    /// Plan Super Resolution for photo `id`: check the model and the size, and prepare the
    /// render. `dir` is the output folder for photos without a file of their own (demo photos).
    pub fn super_res_job(&mut self, id: PhotoId, dir: Option<&str>) -> std::result::Result<SuperResJob, String> {
        self.enhancer.super_res_ready()?;
        let photo = self.catalog.photo(id).cloned().ok_or("no such photo")?;
        if photo.preview_only.is_some() {
            return Err(format!("{} can only be shown from its embedded preview here, so it can't be enlarged", photo.file_name));
        }
        let (folder, stem) = match &photo.source {
            Source::File { path } => {
                let p = std::path::Path::new(path);
                (
                    p.parent().map(PathBuf::from).unwrap_or_default(),
                    p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "photo".into()),
                )
            }
            Source::Demo { .. } => match dir {
                Some(d) => (PathBuf::from(d), photo.file_name.rsplit_once('.').map_or(photo.file_name.clone(), |(s, _)| s.to_string())),
                None => return Err("a generated demo photo has no folder: give `dir`".into()),
            },
        };
        let export = crate::export::ExportOptions {
            format: crate::export::ExportFormat::Tiff,
            bit_depth: Some(16),
            // the model was trained on sRGB pictures
            color_space: lightcraft_pipeline::OutputSpace::Srgb,
            ..Default::default()
        };
        let (w, h) = crate::export::output_size(&photo, &export);
        let max = self.enhancer.max_output_pixels.unwrap_or(DEFAULT_MAX_OUTPUT_PIXELS);
        let out_px = w.checked_mul(h).and_then(|p| p.checked_mul(SUPER_RES_SCALE * SUPER_RES_SCALE)).unwrap_or(usize::MAX);
        if out_px > max {
            return Err(format!(
                "{} would become {:.0} megapixels; Super Resolution is limited to {:.0} here (crop the photo first, or ask for a larger limit with `maxMegapixels`)",
                photo.file_name,
                out_px as f64 / 1e6,
                max as f64 / 1e6
            ));
        }
        let model = self.enhancer.model_file(SUPER_RES_MODEL).ok_or("the Super Resolution model has no file")?;
        let meta = crate::export::export_metadata(&photo, &export);
        let render = self.export_job(id, w, h, lightcraft_pipeline::OutputSpace::Srgb, lightcraft_pipeline::OutputDepth::U16)?;
        Ok(SuperResJob { render, export, meta, model, dir: folder, stem, max_output_pixels: max, photo: id, size: (w, h) })
    }

    /// Add a finished enlargement to the library: import the file with neutral settings, stack
    /// it on top of the original and select it. → `{id, path, original, width, height}`; when the
    /// same enlargement is in the library already, that photo is selected instead and the new file
    /// removed → `{id, original, duplicate: true, width, height}`
    pub fn finish_super_res(&mut self, done: SuperResDone) -> Result<Value> {
        let r = self.execute("library.import", &json!({"paths": [done.path]}))?;
        let Some(new) = r["imported"].get(0).and_then(Value::as_u64) else {
            // an identical file is in the library already (the same photo, edits and clock second):
            // select that photo, and remove this file, which we wrote a moment ago and nothing
            // refers to. Only a copy by *content* at another path goes: a photo already imported
            // from this very path keeps its file.
            let same =
                r["duplicates"].as_array().into_iter().flatten().find_map(|x| x["existing"].as_u64().map(|e| (PhotoId(e), x["reason"] == "content")));
            if let Some((existing, by_content)) = same.filter(|(e, _)| self.catalog.photo(*e).is_some()) {
                let kept_path = matches!(&self.catalog.photo(existing).map(|p| &p.source), Some(Source::File { path }) if *path == done.path);
                if by_content && !kept_path {
                    let _ = std::fs::remove_file(&done.path);
                }
                self.selection = crate::Selection::single(existing);
                return Ok(json!({"id": existing.0, "original": done.photo.0, "duplicate": true, "width": done.size.0, "height": done.size.1}));
            }
            return Err(EngineError::Other(format!("the enlarged photo {} could not be added to the library", done.path)));
        };
        let new_id = PhotoId(new);
        // its pixels carry the original's edits and crop already; default import settings or
        // presets on top would apply them twice
        self.set_develop(new_id, lightcraft_develop::DevelopSettings::default(), "Super Resolution")?;
        // the enlargement on top of the original, expanded so both show
        let _ = self.execute("stack.group", &json!({"ids": [new, done.photo.0], "top": new, "collapsed": false}));
        self.selection = crate::Selection::single(new_id);
        Ok(json!({"id": new, "path": done.path, "original": done.photo.0, "width": done.size.0, "height": done.size.1}))
    }
}

impl SuperResJob {
    /// Render, enlarge, and write the file. `progress(fraction, what)` is called often and
    /// returns `false` to stop (nothing is written then).
    pub fn run(self, progress: &ProgressFn) -> std::result::Result<SuperResDone, String> {
        #[cfg(feature = "enhance")]
        return self.run_enhance(progress);
        #[cfg(not(feature = "enhance"))]
        {
            let _ = progress;
            Err("AI Super Resolution is not available in this build".into())
        }
    }

    #[cfg(feature = "enhance")]
    fn run_enhance(self, progress: &ProgressFn) -> std::result::Result<SuperResDone, String> {
        use lightcraft_pipeline::{DeepImage, DeepSamples};
        use lightcraft_raster::Rgb32f;

        const CANCELLED: &str = "Super Resolution was cancelled";
        if !progress(0.0, "Rendering the photo") {
            return Err(CANCELLED.into());
        }
        let rendered = self.render.run().rendered?;
        let Some(deep) = rendered.deep else { return Err("the photo could not be rendered in 16 bit".into()) };
        drop(rendered.image);
        let DeepSamples::U16(samples) = deep.samples else { return Err("the photo was not rendered in 16 bit".into()) };
        let (w, h, space) = (deep.width, deep.height, deep.space);
        if samples.len() != w * h * 3 {
            return Err("the render has an unexpected size".into());
        }
        let input = Rgb32f { width: w, height: h, data: samples.as_chunks::<3>().0.iter().map(|p| p.map(|v| f32::from(v) / 65535.0)).collect() };
        drop(samples);

        if !progress(0.08, "Loading the model") {
            return Err(CANCELLED.into());
        }
        let span =
            lightcraft_enhance::Span::load(&self.model, &lightcraft_enhance::best_device()).map_err(|e| format!("{}: {e}", self.model.display()))?;
        if span.scale() != SUPER_RES_SCALE {
            return Err(format!("{} enlarges {}×, not {SUPER_RES_SCALE}×", self.model.display(), span.scale()));
        }
        let opts = lightcraft_enhance::Options { max_output_pixels: self.max_output_pixels, ..Default::default() };
        let enlarged = span
            .upscale(&input, &opts, &mut |p| progress(0.1 + 0.85 * p, "Enlarging"))
            .map_err(|e| if matches!(e, lightcraft_enhance::Error::Cancelled) { CANCELLED.to_string() } else { e.to_string() })?;
        drop(input);
        drop(span);

        if !progress(0.96, "Saving") {
            return Err(CANCELLED.into());
        }
        let samples: Vec<u16> = enlarged.data.iter().flat_map(|p| p.map(|v| (v.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16)).collect();
        let size = (enlarged.width, enlarged.height);
        drop(enlarged);
        let image = DeepImage { width: size.0, height: size.1, space, samples: DeepSamples::U16(samples) };
        let bytes = crate::export::encode_deep(&image, &self.export, self.meta.as_ref())?;
        drop(image);
        let (dir, stem) = (self.dir.clone(), self.stem.clone());
        let mut names = (1u64..).map(move |k| dir.join(if k == 1 { format!("{stem}-SR.tif") } else { format!("{stem}-SR-{k}.tif") }));
        let path = lightcraft_catalog::safe_file::write_new_unique(&mut names, &bytes)
            .map_err(|e| format!("could not write the enlarged photo next to the original: {e}"))?
            .to_string_lossy()
            .to_string();
        progress(1.0, "Done");
        Ok(SuperResDone { photo: self.photo, path, size })
    }
}
