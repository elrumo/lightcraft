//! **AI Remove**: a Remove spot filled by MI-GAN (`lightcraft-erase`) with what the photo most
//! likely shows behind the stroke, instead of texture copied from around it. The model's answer
//! is stored with the spot ([`lightcraft_develop::SpotFill`]), so renders and exports never run it.
//!
//! The model is optional (about 30 MB, MIT) and never part of LightCraft. The desktop and phone
//! apps download it like the other models, after the user agreed (`models.download {"id":
//! "migan", "acknowledged": true}`, Settings → AI Models), into `<models>/migan/`; the browser
//! build fetches it itself, keeps it in the site's storage and hands the bytes over
//! ([`Session::install_eraser`]). Either way the file must be exactly the pinned one.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::sync::{Arc, Mutex, PoisonError};

use lightcraft_catalog::PhotoId;
use lightcraft_develop::{DevelopSettings, Spot, SpotFill, SpotMode};
use lightcraft_erase::expand::Monitor;
use lightcraft_erase::{Eraser, Painter, fill_with_watched};
use lightcraft_geom::Point;
use lightcraft_pipeline::spots::EraseInput;

use crate::Session;
use crate::activity::{Cancel, Unit};
use crate::expand::ExpandModel;
use crate::media::SourceLevel;

/// High quality's model, LaMa: what the apps show before downloading it.
pub use lightcraft_erase::lama::{CREDIT as LAMA_CREDIT, FILE_SIZE as LAMA_FILE_SIZE, ID as LAMA_ID, LICENCE as LAMA_LICENCE};
pub use lightcraft_erase::{BASE_URL, CREDIT, FILE, FILE_SIZE, ID, LICENCE, verify};

/// The start of the error when the model isn't here: the apps offer the download on it.
pub const NOT_INSTALLED: &str = "The AI Remove model (MI-GAN) is not downloaded";

/// The loaded model, kept for the session once loaded, and the fills being painted in the background.
#[derive(Default)]
pub struct EraseModel {
    loaded: Mutex<Option<Arc<Eraser>>>,
    jobs: Mutex<Vec<EraseTask>>,
    /// Held while a network paints: one at a time, however many strokes wait (a big network takes seconds and
    /// hundreds of megabytes).
    pub(crate) turn: Arc<Mutex<()>>,
    /// Since when nothing has been painting (the networks stay loaded a while for the next stroke, see [`KEEP_LOADED`]).
    idle_since: Mutex<Option<web_time::Instant>>,
}

/// How long MI-GAN and LaMa stay loaded after the last stroke was painted: the next one usually follows within seconds,
/// and loading LaMa again takes a few. Then they are dropped (about 200 MB for LaMa, more once it has run), which a
/// phone cannot afford to keep for the rest of the session.
const KEEP_LOADED: web_time::Duration = web_time::Duration::from_secs(45);

/// How AI Remove paints: the small network or the larger one. A fill is stored with its spot whichever made it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RemoveQuality {
    /// MI-GAN: about 30 MB, a fraction of a second on a computer.
    #[default]
    Fast,
    /// LaMa: about 208 MB, seconds on a computer; continues textures and larger areas better.
    High,
    /// Experimental, offered by no app: a diffusion model (Stable Diffusion 1.5 Inpainting, or Kandinsky 2.2 / SDXL
    /// where the device runs them), the ones Generative Expand offers. Gigabytes, tens of seconds. On `remove_eval`
    /// strokes over real people (2026-10-17) Stable Diffusion 1.5 was no better than LaMa and painted a person into a
    /// hole; a model made for removal could change that.
    Ultra,
}

impl RemoveQuality {
    /// The name in commands (`quality`).
    pub fn id(self) -> &'static str {
        match self {
            RemoveQuality::Fast => "fast",
            RemoveQuality::High => "high",
            RemoveQuality::Ultra => "ultra",
        }
    }

    pub fn from_id(id: &str) -> Option<RemoveQuality> {
        match id {
            "fast" => Some(RemoveQuality::Fast),
            "high" => Some(RemoveQuality::High),
            "ultra" => Some(RemoveQuality::Ultra),
            _ => None,
        }
    }

    /// The model that paints at this quality (its id in `models.list`); for Ultra the one the device offers first
    /// (see [`Session::ultra_model`] for the one that paints).
    pub fn model_id(self) -> &'static str {
        match self {
            RemoveQuality::Fast => lightcraft_erase::ID,
            RemoveQuality::High => lightcraft_erase::lama::ID,
            RemoveQuality::Ultra => ExpandModel::Sd15.id(),
        }
    }
}

/// What tells a spot apart while its fill is being painted: strokes have no ids, and the user may move, resize or
/// delete spots meanwhile.
#[derive(Clone, Debug, PartialEq)]
struct SpotKey {
    points: Vec<Point>,
    size: f64,
    shape: Option<lightcraft_develop::SegMask>,
}

impl SpotKey {
    fn of(s: &Spot) -> SpotKey {
        SpotKey { points: s.points.clone(), size: s.size, shape: s.shape.clone() }
    }

    fn is(&self, s: &Spot) -> bool {
        s.mode == SpotMode::Remove && s.points == self.points && s.size == self.size && s.shape == self.shape
    }
}

/// A fill on its worker thread.
struct EraseTask {
    photo: PhotoId,
    key: SpotKey,
    rx: Receiver<Result<SpotFill, String>>,
}

/// What paints a fill: a network that is loaded, or a diffusion model's folder (gigabytes: loaded on the worker for the
/// job and dropped after it).
#[derive(Clone)]
pub(crate) enum Brush {
    Net(Arc<dyn Painter>),
    Diffusion { model: ExpandModel, dir: std::path::PathBuf },
}

/// Everything painting one spot's fill needs, ready to run off the UI thread.
pub struct EraseJob {
    photo: PhotoId,
    key: SpotKey,
    input: EraseInput,
    brush: Brush,
}

impl EraseJob {
    /// Paint the fill. `cancelled` is asked before the network starts (and between a diffusion model's steps);
    /// `progress` hears the fraction done (a diffusion model's).
    fn run(&self, cancelled: &(dyn Fn() -> bool + Sync), progress: &(dyn Fn(f32) + Sync)) -> Result<SpotFill, String> {
        let paint = |p: &dyn Painter, mon: &Monitor<'_>| fill_with_watched(p, &self.input.image, &self.input.hole, mon).map_err(|e| e.to_string());
        let filled = match &self.brush {
            Brush::Net(p) => paint(p.as_ref(), &Monitor { cancelled, progress })?,
            Brush::Diffusion { model, dir } => {
                // loading the model is the first tenth
                let load = |f: f32| progress(0.1 * f);
                let p = crate::expand::load_diffusion_for(*model, dir, &Monitor { cancelled, progress: &load }, true)?;
                let steps = |f: f32| progress(0.1 + 0.9 * f);
                paint(&*p, &Monitor { cancelled, progress: &steps })?
            }
        };
        self.input.finish(&filled).ok_or_else(|| "AI Remove made no fill".to_string())
    }
}

impl Session {
    /// Use the models in the desktop app's folder (`<config>/models`) unless the host chose
    /// another: the command line and the headless MCP server then find a model the app
    /// downloaded (AI Remove, Super Resolution).
    pub fn with_models(mut self) -> Self {
        if self.enhancer.dir.is_none() {
            self.enhancer.dir = crate::camera_profiles::config_dir().map(|d| d.join("models"));
        }
        self
    }

    /// The model's file in the models folder (desktop and phone apps).
    fn eraser_file(&self) -> Option<std::path::PathBuf> {
        self.enhancer.model_dir(lightcraft_erase::ID).map(|d| d.join(lightcraft_erase::FILE))
    }

    /// Whether AI Remove can run now: the model is loaded, or its file is in place, and the user
    /// hasn't turned it off.
    pub fn eraser_ready(&self) -> bool {
        !self.enhancer.disabled.contains(lightcraft_erase::ID)
            && (self.eraser.loaded.lock().unwrap_or_else(PoisonError::into_inner).is_some()
                || self.eraser_file().is_some_and(|f| std::fs::metadata(f).is_ok_and(|m| m.len() == lightcraft_erase::FILE_SIZE)))
    }

    /// Use the model in `bytes` (the browser build's copy). They must be exactly the pinned file.
    pub fn install_eraser(&self, bytes: &[u8]) -> Result<(), String> {
        if !lightcraft_erase::verify(bytes) {
            return Err("the AI Remove model file is damaged or not the expected one: download it again".into());
        }
        let e = Eraser::load(bytes).map_err(|e| e.to_string())?;
        *self.eraser.loaded.lock().unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(e));
        Ok(())
    }

    /// The model, loaded once (from the models folder the first time).
    pub(crate) fn eraser(&self) -> Result<Arc<Eraser>, String> {
        if self.enhancer.disabled.contains(lightcraft_erase::ID) {
            return Err("AI Remove is turned off: turn its model on in Settings → AI Models.".into());
        }
        let mut slot = self.eraser.loaded.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(e) = slot.as_ref() {
            return Ok(e.clone());
        }
        let missing = || {
            format!(
                "{NOT_INSTALLED}. Download it (about 30 MB, MIT licence) when LightCraft offers it, or with `models.download {{\"id\": \"{}\", \"acknowledged\": true}}`.",
                lightcraft_erase::ID
            )
        };
        let file = self.eraser_file().ok_or_else(missing)?;
        let bytes = std::fs::read(&file).map_err(|_| missing())?;
        if !lightcraft_erase::verify(&bytes) {
            return Err(format!("{} is damaged or not the expected file: delete it in Settings → AI Models and download it again", file.display()));
        }
        let e = Arc::new(Eraser::load(&bytes).map_err(|e| e.to_string())?);
        *slot = Some(e.clone());
        Ok(e)
    }

    /// Whether AI Remove can paint at `quality` now: the model is in place, the device runs it and the user hasn't
    /// turned it off.
    pub fn remove_ready(&self, quality: RemoveQuality) -> bool {
        match quality {
            RemoveQuality::Fast => self.eraser_ready(),
            RemoveQuality::High => self.lama_ready(),
            RemoveQuality::Ultra => self.ultra_model().is_some(),
        }
    }

    /// The diffusion models Ultra can paint with, the one Generative Expand's picker chose first when it is one, then
    /// the fastest.
    fn ultra_candidates(&self) -> Vec<ExpandModel> {
        let chosen = self.chosen_expand_model();
        let mut out: Vec<ExpandModel> = if chosen.diffusion() { vec![chosen] } else { Vec::new() };
        for m in [ExpandModel::Sd15, ExpandModel::Kandinsky, ExpandModel::Sdxl] {
            if !out.contains(&m) {
                out.push(m);
            }
        }
        out.retain(|m| self.expand_available(*m));
        out
    }

    /// Whether this device runs a diffusion model for Ultra (it may still need its download).
    pub fn ultra_available(&self) -> bool {
        !self.ultra_candidates().is_empty()
    }

    /// The diffusion model Ultra paints with now: downloaded, on, and run by this device.
    pub fn ultra_model(&self) -> Option<ExpandModel> {
        self.ultra_candidates().into_iter().find(|m| !self.enhancer.disabled.contains(m.id()) && self.enhancer.installed(m.id()))
    }

    /// The diffusion model Ultra asks for: the one that paints now, else the one to download first (`None`: this
    /// device runs none). What an app's Ultra switch offers.
    pub fn ultra_pick(&self) -> Option<ExpandModel> {
        self.ultra_model().or_else(|| self.ultra_candidates().first().copied())
    }

    /// What paints at `quality`: a network, loaded once, or the folder of a diffusion model.
    fn erase_brush(&self, quality: RemoveQuality) -> Result<Brush, String> {
        match quality {
            RemoveQuality::Fast => Ok(Brush::Net(self.eraser()? as Arc<dyn Painter>)),
            RemoveQuality::High => {
                if !self.expand_available(ExpandModel::Lama) {
                    return Err(format!("High quality AI Remove (LaMa) doesn't run on this device ({})", self.device.describe()));
                }
                Ok(Brush::Net(self.lama()? as Arc<dyn Painter>))
            }
            RemoveQuality::Ultra => {
                if !self.ultra_available() {
                    return Err(format!("Ultra quality AI Remove (a diffusion model) doesn't run on this device ({})", self.device.describe()));
                }
                let Some(model) = self.ultra_model() else {
                    let m = self.ultra_candidates().first().copied().unwrap_or(ExpandModel::Sd15);
                    return Err(format!(
                        "{NOT_INSTALLED} ({}). Download it (about {:.1} GB, {} licence) when LightCraft offers it, or with `models.download {{\"id\": \"{}\", \"acknowledged\": true}}`.",
                        m.label(),
                        m.download_bytes() as f64 / 1e9,
                        m.licence().0,
                        m.id()
                    ));
                };
                let dir = self.enhancer.model_dir(model.id()).ok_or("the models folder is not set")?;
                Ok(Brush::Diffusion { model, dir })
            }
        }
    }

    /// The network that paints at `quality`, loaded now (tools and tests; a job loads a diffusion model on its worker).
    pub fn remove_painter(&self, quality: RemoveQuality) -> Result<Arc<dyn Painter>, String> {
        match self.erase_brush(quality)? {
            Brush::Net(p) => Ok(p),
            Brush::Diffusion { model, dir } => Ok(Arc::from(crate::expand::load_diffusion_for(model, &dir, &Monitor::NONE, true)?)),
        }
    }

    /// Render the area around spot `index` of `d` (photo `id`'s settings) for the network of `quality`.
    pub fn erase_job(&mut self, id: PhotoId, d: &DevelopSettings, index: usize, quality: RemoveQuality) -> Result<EraseJob, String> {
        let brush = self.erase_brush(quality)?;
        self.erase_job_with(id, d, index, brush)
    }

    /// [`Session::erase_job`] for a given brush.
    pub(crate) fn erase_job_with(&mut self, id: PhotoId, d: &DevelopSettings, index: usize, brush: Brush) -> Result<EraseJob, String> {
        let info = self.source_info(id);
        let mut src = self.source_now(id, SourceLevel::Preview)?;
        let mut input = lightcraft_pipeline::spots::erase_input(&src, &info, d, index).ok_or("the stroke is outside the photo")?;
        // a small stroke on a big photo: the preview has too few pixels around it, read the original
        let full = self.catalog.photo(id).map_or(0, |p| p.width.max(p.height) as usize);
        if input.native < lightcraft_erase::SIDE as f64 && full > src.width.max(src.height) {
            src = self.source_now(id, SourceLevel::Full)?;
            input = lightcraft_pipeline::spots::erase_input(&src, &info, d, index).ok_or("the stroke is outside the photo")?;
        }
        let key = SpotKey::of(d.spots.get(index).ok_or("no such spot")?);
        Ok(EraseJob { photo: id, key, input, brush })
    }

    /// Fill spot `index` of `d` (photo `id`'s settings) with AI Remove here and now (scripts, agents, the browser):
    /// render the area around the stroke, let the network paint it, store the answer in the spot.
    pub fn erase_spot_with(&mut self, id: PhotoId, d: &mut DevelopSettings, index: usize, quality: RemoveQuality) -> Result<(), String> {
        let fill = self.erase_job(id, d, index, quality)?.run(&|| false, &|_| {})?;
        let spot = d.spots.get_mut(index).ok_or("no such spot")?;
        spot.fill = Some(fill);
        Ok(())
    }

    /// [`Session::erase_spot_with`] at the default quality.
    pub fn erase_spot(&mut self, id: PhotoId, d: &mut DevelopSettings, index: usize) -> Result<(), String> {
        self.erase_spot_with(id, d, index, RemoveQuality::Fast)
    }

    /// Paint the fill of spot `index` of `d` on a worker thread, with a row in the activity stack (✕ stops it before
    /// it starts); [`Session::poll_erase`] stores it. In the browser it runs inline.
    pub fn start_erase(&mut self, id: PhotoId, d: &DevelopSettings, index: usize, quality: RemoveQuality) -> Result<(), String> {
        let job = self.erase_job(id, d, index, quality)?;
        let label = match quality {
            RemoveQuality::Fast => "Removing with AI",
            RemoveQuality::High => "Removing with AI (high quality)",
            RemoveQuality::Ultra => "Removing with AI (ultra quality)",
        };
        self.spawn_erase(job, label)
    }

    pub(crate) fn spawn_erase(&mut self, job: EraseJob, label: &str) -> Result<(), String> {
        let cancel = Arc::new(AtomicBool::new(false));
        let guard = self.activity.start("erase", label, Cancel::Flag(cancel.clone()));
        guard.set_unit(Unit::Percent);
        let (tx, rx) = channel();
        let (photo, key) = (job.photo, job.key.clone());
        let turn = self.eraser.turn.clone();
        let work = move || {
            let _turn = turn.lock().unwrap_or_else(PoisonError::into_inner);
            let h = guard.handle();
            let r = if cancel.load(Ordering::Relaxed) {
                Err("cancelled".to_string())
            } else {
                let run = || job.run(&|| cancel.load(Ordering::Relaxed), &|f| h.progress((f * 100.0).round() as u64, 100));
                // a panicking network reports an error instead of leaving the row up forever
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(run)).unwrap_or_else(|_| Err("AI Remove failed unexpectedly".into()))
            };
            let _ = tx.send(r);
            // (the stroke's pictures and the network's working memory go back to the system now, not at the next
            // memory warning)
            drop(job);
            crate::memory::release();
            drop(guard);
        };
        #[cfg(not(target_arch = "wasm32"))]
        std::thread::Builder::new().name("ai-remove".into()).spawn(work).map_err(|e| e.to_string())?;
        #[cfg(target_arch = "wasm32")]
        work();
        self.eraser.jobs.lock().unwrap_or_else(PoisonError::into_inner).push(EraseTask { photo, key, rx });
        Ok(())
    }

    /// How many fills are being painted or wait their turn.
    pub fn erase_running(&self) -> usize {
        self.eraser.jobs.lock().unwrap_or_else(PoisonError::into_inner).len()
    }

    /// Store the fills that are done (call every tick), one undo step each. An error for each that failed; a fill
    /// whose stroke was moved, resized or deleted meanwhile, or that was cancelled, is dropped without one.
    pub fn poll_erase(&mut self) -> Vec<String> {
        self.unload_idle_erasers();
        let done: Vec<(PhotoId, SpotKey, Result<SpotFill, String>)> = {
            let mut jobs = self.eraser.jobs.lock().unwrap_or_else(PoisonError::into_inner);
            let mut done = Vec::new();
            jobs.retain(|t| match t.rx.try_recv() {
                Err(TryRecvError::Empty) => true,
                Ok(r) => {
                    done.push((t.photo, t.key.clone(), r));
                    false
                }
                Err(TryRecvError::Disconnected) => {
                    done.push((t.photo, t.key.clone(), Err("AI Remove stopped unexpectedly".into())));
                    false
                }
            });
            done
        };
        let mut errors = Vec::new();
        for (photo, key, r) in done {
            match r {
                Ok(fill) => {
                    if let Err(e) = self.finish_erase(photo, &key, fill) {
                        errors.push(e);
                    }
                }
                Err(e) if e == "cancelled" => {}
                Err(e) => errors.push(e),
            }
        }
        errors
    }

    /// Drop the networks once nothing has painted for [`KEEP_LOADED`] (call every tick: it is a clock read when
    /// nothing is loaded or something is painting).
    fn unload_idle_erasers(&self) {
        let mut since = self.eraser.idle_since.lock().unwrap_or_else(PoisonError::into_inner);
        if self.erase_running() > 0 || self.expand_running().is_some() {
            *since = None;
            return;
        }
        let now = web_time::Instant::now();
        if now.duration_since(*since.get_or_insert(now)) >= KEEP_LOADED && self.unload_erasers() {
            *since = None;
        }
    }

    /// Drop MI-GAN and LaMa when nothing is painting or using them and they can be read from disk again, and hand
    /// their memory back. Whether any was loaded.
    pub fn unload_erasers(&self) -> bool {
        if self.erase_running() > 0 || self.expand_running().is_some() {
            return false;
        }
        let mut dropped = false;
        {
            let mut slot = self.eraser.loaded.lock().unwrap_or_else(PoisonError::into_inner);
            // (the browser build is handed the file and has no copy to read again)
            if slot.as_ref().is_some_and(|e| Arc::strong_count(e) == 1) && self.eraser_file().is_some_and(|f| f.exists()) {
                *slot = None;
                dropped = true;
            }
        }
        dropped |= self.unload_lama();
        if dropped {
            log::info!("memory: the AI Remove networks were unloaded");
            crate::memory::release();
        }
        dropped
    }

    /// Put `fill` in the spot `key` names, if it is still there: one undo step.
    fn finish_erase(&mut self, photo: PhotoId, key: &SpotKey, fill: SpotFill) -> Result<(), String> {
        let Some(mut d) = self.develop_of(photo).map(|d| (*d).clone()) else { return Ok(()) };
        let Some(spot) = d.spots.iter_mut().find(|s| key.is(s)) else { return Ok(()) };
        spot.fill = Some(fill);
        self.set_develop(photo, d, "AI Remove").map_err(|e| e.to_string())
    }
}
