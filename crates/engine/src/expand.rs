//! **Generative Expand**: the crop's Expand lets the frame reach past the photo's edges, and
//! Generate paints the new area on this device. The answer is stored with the photo's settings
//! ([`lightcraft_develop::ExpandFill`]), so renders and exports never run a model
//! (`lightcraft_pipeline::expand`).
//!
//! What paints, offered by the device's profile ([`ExpandModel::fits`],
//! [`crate::hardware::DeviceProfile`]):
//!
//! - **MI-GAN** (`migan`, ~30 MB, MIT): the AI Remove model. Fast everywhere: the desktop and
//!   phone apps and the browser.
//! - **LaMa** (`lama`, 208 MB, Apache-2.0): continues long structures across a wide strip best.
//!   Computers, and phones that may use 2 GB or more.
//! - **Stable Diffusion 1.5 Inpainting** (`sd15-inpaint`, 2.1 GB, CreativeML OpenRAIL-M): a
//!   diffusion model that draws new detail at 512 pixels. Computers with 8 GB or more, and phones
//!   with a GPU that may use 4 GB.
//! - **Kandinsky 2.2 Inpaint** (`kandinsky22-inpaint`, 9 GB, Apache-2.0): draws new detail at 768
//!   pixels, conditioned on the photo itself. Computers with 16 GB or more.
//! - **SDXL Inpainting** (`sdxl-inpaint`, 6.9 GB, CreativeML Open RAIL++-M): the most detail, at
//!   1024 pixels. Computers with a GPU and 16 GB, or 32 GB without one.
//! - **Classic** (`classic`): no model and no download; texture synthesized from the photo (the
//!   panorama's Fill Edges). Good on sky, water, grass and walls; it can't invent shapes.
//!
//! The user chooses one in the crop tool or in Settings → AI Models, or leaves it on Automatic: the
//! best model for this device ([`Session::recommended_expand_model`]). The diffusion models run on
//! the GPU (Metal) when there is one, else on the CPU, where they take minutes.
//
//! One more way lives outside the engine: **Apple Image Playground** (`appleCloud`), in a native
//! frontend on devices with Apple Intelligence. It is not on the lists above: it runs in Apple's cloud (the
//! app asks every time), through a sheet the user works in, so it is never the default. The engine
//! gives the app the canvas as a picture ([`Session::expand_canvas`], `crop.expandCanvas`) and takes
//! the picture the sheet made ([`Session::apply_expand_picture`], `crop.applyExpandPicture`): the
//! photo stays as it is, the new area comes from the picture in the photo's tones, joined to the
//! photo's edge, with its grain, and is stored like any other fill.
//!
//! The models are optional and never part of LightCraft: the user downloads them (Settings → AI
//! Models, or the crop tool's offer; agents `models.download {"id": "lama", "acknowledged":
//! true}`), and each file is checked against its pinned size and SHA-256 before use.
//!
//! The work runs off the UI thread with a row in the activity stack ([`Session::start_expand`]),
//! or inline for scripts and agents ([`Session::expand_now`]). Either way the result lands as one
//! undo step, "Generative Expand".

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::sync::{Arc, Mutex, PoisonError};

use lightcraft_catalog::PhotoId;
use lightcraft_develop::ExpandFill;
use lightcraft_erase::Painter;
use lightcraft_erase::expand::Monitor;
use lightcraft_erase::lama::Lama;
use lightcraft_pipeline::expand::ExpandInput;
use serde::Serialize;

use crate::Session;
use crate::activity::{Cancel, Unit};
use crate::hardware::{DeviceClass, DeviceProfile, GB};
use crate::media::SourceLevel;

/// The start of the error when the chosen model isn't here: the apps offer the download on it.
pub const NOT_INSTALLED: &str = "The Generative Expand model is not downloaded";

/// Painters outside the engine that answer with a picture (`crop.applyExpandPicture`), by the id
/// the fill is stored with.
pub const EXTERNAL_MODELS: [&str; 1] = ["appleCloud"];

/// What a painter outside the engine is given: the canvas as a picture.
pub struct ExternalCanvas {
    /// Names the crop the canvas was made for: [`Session::apply_expand_picture`] refuses an answer for another.
    pub key: u64,
    pub width: usize,
    pub height: usize,
    /// The canvas as a PNG: the photo where it is and a placeholder over the new area (the Classic fill).
    pub png: Vec<u8>,
}

/// Names photo `id`'s crop (FNV-1a over its settings, in 53 bits: a number every JSON reader keeps whole).
fn expand_key(id: PhotoId, d: &lightcraft_develop::DevelopSettings) -> u64 {
    let text = format!("{}:{}", id.0, serde_json::to_string(&d.crop).unwrap_or_default());
    text.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)) & ((1 << 53) - 1)
}

/// Largest picture file taken from a painter outside the engine (bytes).
const MAX_PICTURE_BYTES: u64 = 64 << 20;

/// Whether this build runs in a browser (one pass at 512, on the page's thread).
const WEB: bool = cfg!(target_arch = "wasm32");

/// What paints the new area.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ExpandModel {
    MiGan,
    Lama,
    Sd15,
    Kandinsky,
    Sdxl,
    Classic,
}

impl ExpandModel {
    /// Every model, in the order the pickers list them (the networks from the smallest).
    pub const ALL: [ExpandModel; 6] =
        [ExpandModel::MiGan, ExpandModel::Lama, ExpandModel::Sd15, ExpandModel::Kandinsky, ExpandModel::Sdxl, ExpandModel::Classic];

    /// The id in commands and in the stored fill (the model's id in Settings → AI Models).
    pub fn id(self) -> &'static str {
        match self {
            ExpandModel::MiGan => lightcraft_erase::ID,
            ExpandModel::Lama => lightcraft_erase::lama::ID,
            ExpandModel::Sd15 => "sd15-inpaint",
            ExpandModel::Kandinsky => "kandinsky22-inpaint",
            ExpandModel::Sdxl => "sdxl-inpaint",
            ExpandModel::Classic => "classic",
        }
    }

    pub fn from_id(id: &str) -> Option<ExpandModel> {
        ExpandModel::ALL.into_iter().find(|m| m.id() == id)
    }

    /// Its name in the app.
    pub fn label(self) -> &'static str {
        match self {
            ExpandModel::MiGan => "MI-GAN",
            ExpandModel::Lama => "LaMa",
            ExpandModel::Sd15 => "Stable Diffusion 1.5",
            ExpandModel::Kandinsky => "Kandinsky 2.2",
            ExpandModel::Sdxl => "SDXL",
            ExpandModel::Classic => "Classic",
        }
    }

    /// A diffusion model (`lightcraft-diffuse`): draws new detail, takes much longer, and needs the
    /// enhancement models of this build.
    pub fn diffusion(self) -> bool {
        matches!(self, ExpandModel::Sd15 | ExpandModel::Kandinsky | ExpandModel::Sdxl)
    }

    /// Whether a device with profile `p` can run it (downloading it is another matter: builds
    /// without the model downloader take files put in the models folder by hand, and the browser
    /// fetches MI-GAN itself, `Session::install_eraser`).
    pub fn fits(self, p: &DeviceProfile) -> bool {
        let computer = p.class == DeviceClass::Computer;
        let phone = p.class == DeviceClass::Phone;
        // (unknown memory on a computer: offered, the user knows their machine)
        let diffusion = cfg!(feature = "enhance") && !WEB;
        match self {
            ExpandModel::Classic | ExpandModel::MiGan => true,
            ExpandModel::Lama => !WEB && (computer || (phone && p.has_memory(2 * GB, false))),
            ExpandModel::Sd15 => diffusion && ((computer && p.has_memory(8 * GB, true)) || (phone && p.gpu && p.has_memory(4 * GB, false))),
            ExpandModel::Kandinsky => diffusion && computer && p.has_memory(16 * GB, true),
            ExpandModel::Sdxl => diffusion && computer && ((p.gpu && p.has_memory(16 * GB, true)) || p.has_memory(32 * GB, true)),
        }
    }

    /// The model Automatic picks on a device with profile `p`: the most capable one that runs
    /// there in reasonable time. A phone: Stable Diffusion 1.5 when it fits (a recent iPhone or
    /// iPad), else LaMa, else MI-GAN. A computer with a GPU: SDXL with 32 GB or more, else Stable
    /// Diffusion 1.5. A computer without one: Stable Diffusion 1.5 with 12 threads and 16 GB or
    /// more (a few minutes a Generate), else LaMa. The browser: MI-GAN.
    pub fn recommended(p: &DeviceProfile) -> ExpandModel {
        let pick = |order: &[ExpandModel]| order.iter().copied().find(|m| m.fits(p)).unwrap_or(ExpandModel::MiGan);
        match p.class {
            DeviceClass::Browser => ExpandModel::MiGan,
            DeviceClass::Phone => pick(&[ExpandModel::Sd15, ExpandModel::Lama]),
            DeviceClass::Computer if p.gpu && p.has_memory(32 * GB, false) => pick(&[ExpandModel::Sdxl, ExpandModel::Sd15, ExpandModel::Lama]),
            DeviceClass::Computer if p.gpu => pick(&[ExpandModel::Sd15, ExpandModel::Lama]),
            DeviceClass::Computer if p.cores >= 12 && p.has_memory(16 * GB, false) => pick(&[ExpandModel::Sd15, ExpandModel::Lama]),
            DeviceClass::Computer => pick(&[ExpandModel::Lama]),
        }
    }

    /// Download size (0 for Classic).
    pub fn download_bytes(self) -> u64 {
        match self {
            ExpandModel::MiGan => lightcraft_erase::FILE_SIZE,
            ExpandModel::Lama => lightcraft_erase::lama::FILE_SIZE,
            #[cfg(feature = "enhance")]
            ExpandModel::Sd15 => lightcraft_diffuse::models::SD15.bytes(),
            #[cfg(feature = "enhance")]
            ExpandModel::Kandinsky => lightcraft_diffuse::models::KANDINSKY.bytes(),
            #[cfg(feature = "enhance")]
            ExpandModel::Sdxl => lightcraft_diffuse::models::SDXL.bytes(),
            #[cfg(not(feature = "enhance"))]
            ExpandModel::Sd15 | ExpandModel::Kandinsky | ExpandModel::Sdxl => 0,
            ExpandModel::Classic => 0,
        }
    }

    /// What it is good at, for the pickers.
    pub fn note(self) -> &'static str {
        match self {
            ExpandModel::MiGan => "Fast, on every device",
            ExpandModel::Lama => "Best for wide areas of sky, water or texture",
            ExpandModel::Sd15 => "Draws new detail; slower",
            ExpandModel::Kandinsky => "Draws new detail, guided by the photo; slow",
            ExpandModel::Sdxl => "The most detail; slow",
            ExpandModel::Classic => "No download: texture from the photo",
        }
    }

    /// The licence and credit the pickers show (none for Classic).
    pub fn licence(self) -> (&'static str, &'static str) {
        match self {
            ExpandModel::MiGan => (lightcraft_erase::LICENCE, lightcraft_erase::CREDIT),
            ExpandModel::Lama => (lightcraft_erase::lama::LICENCE, lightcraft_erase::lama::CREDIT),
            #[cfg(feature = "enhance")]
            ExpandModel::Sd15 => (lightcraft_diffuse::models::SD15.licence, lightcraft_diffuse::models::SD15.credit),
            #[cfg(feature = "enhance")]
            ExpandModel::Kandinsky => (lightcraft_diffuse::models::KANDINSKY.licence, lightcraft_diffuse::models::KANDINSKY.credit),
            #[cfg(feature = "enhance")]
            ExpandModel::Sdxl => (lightcraft_diffuse::models::SDXL.licence, lightcraft_diffuse::models::SDXL.credit),
            #[cfg(not(feature = "enhance"))]
            ExpandModel::Sd15 | ExpandModel::Kandinsky | ExpandModel::Sdxl => ("", ""),
            ExpandModel::Classic => ("", ""),
        }
    }

    /// The licence restricts how the model may be used (an OpenRAIL licence).
    pub fn restricted(self) -> bool {
        matches!(self, ExpandModel::Sd15 | ExpandModel::Sdxl)
    }
}

/// One way to paint, as the apps and agents see it (`crop.expandModels`).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpandChoice {
    pub id: &'static str,
    pub label: &'static str,
    /// What it is good at, for the picker.
    pub note: &'static str,
    /// This build and device can run it.
    pub available: bool,
    /// It can run now: on the device and not turned off.
    pub ready: bool,
    /// The user turned its model off in Settings → AI Models.
    pub disabled: bool,
    pub download_bytes: u64,
    pub licence: &'static str,
    pub credit: &'static str,
    /// Its licence restricts how it may be used (OpenRAIL): the download says so.
    pub restricted: bool,
    /// A diffusion model: draws new detail, takes longer.
    pub diffusion: bool,
    /// What Automatic picks on this device.
    pub recommended: bool,
    /// What the pickers show as chosen (the user's choice, else Automatic's).
    pub chosen: bool,
    /// What Generate uses when no model is named.
    pub default: bool,
}

/// The models and the running job, kept for the session.
#[derive(Default)]
pub struct ExpandState {
    lama: Arc<Mutex<Option<Arc<Lama>>>>,
    task: Mutex<Option<ExpandTask>>,
    /// The model the user chose (`None`: Automatic, the best for this device).
    pub preferred: Option<ExpandModel>,
}

/// A job on its worker thread.
struct ExpandTask {
    photo: PhotoId,
    model: ExpandModel,
    cancel: Arc<AtomicBool>,
    rx: Receiver<Result<ExpandFill, String>>,
}

/// What paints, resolved before the work starts (LaMa and the diffusion models load on the
/// worker).
enum Brush {
    Net(Arc<dyn Painter>),
    LamaFile {
        file: std::path::PathBuf,
        slot: Arc<Mutex<Option<Arc<Lama>>>>,
    },
    /// A diffusion model's folder: loaded for the job and dropped after it (gigabytes).
    #[cfg_attr(not(feature = "enhance"), allow(dead_code))]
    Diffusion(std::path::PathBuf),
    Classic,
}

/// Everything a Generate needs, ready to run off the UI thread.
pub struct ExpandJob {
    pub photo: PhotoId,
    pub model: ExpandModel,
    input: ExpandInput,
    brush: Brush,
}

/// What a finished Generate did (`crop.generate`'s answer).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpandDone {
    pub photo: u64,
    pub model: &'static str,
    /// Network passes made (0 for Classic).
    pub passes: usize,
    /// The canvas the model painted, in pixels.
    pub width: usize,
    pub height: usize,
    /// Size of the stored fill in the settings (bytes of its encoded picture).
    pub stored_bytes: usize,
}

impl ExpandJob {
    /// Network passes the job makes (one for a model, none for Classic).
    pub fn passes(&self) -> usize {
        match self.brush {
            Brush::Classic => 0,
            _ => 1,
        }
    }

    /// Paint the new area. `cancelled` is asked between passes (and a diffusion model's steps);
    /// `progress` hears the fraction done.
    pub fn run(&self, cancelled: &(dyn Fn() -> bool + Sync), progress: &(dyn Fn(f32) + Sync)) -> Result<ExpandFill, String> {
        // (the network's pass is most of a model's time on a phone, the texture after it on a computer)
        let net_progress = |f: f32| progress(0.6 * f);
        let mon = Monitor { cancelled, progress: &net_progress };
        let outpaint = |p: &dyn Painter, mon: &Monitor<'_>| {
            lightcraft_erase::expand::outpaint(p, &self.input.image, &self.input.known, mon).map_err(|e| e.to_string())
        };
        let filled = match &self.brush {
            Brush::Classic => {
                if cancelled() {
                    return Err("cancelled".into());
                }
                let f = self.input.classic();
                progress(1.0);
                f
            }
            // the network's structure, the photo's grain on it
            Brush::Net(p) => self.input.with_texture(&outpaint(&**p, &mon)?),
            Brush::LamaFile { file, slot } => {
                let lama = load_lama(file, slot)?;
                self.input.with_texture(&outpaint(&*lama, &mon)?)
            }
            // a diffusion model's answer has its own detail; loading it is the first fifth
            Brush::Diffusion(dir) => {
                let load = |f: f32| progress(0.2 * f);
                let painter = load_diffusion(self.model, dir, &Monitor { cancelled, progress: &load })?;
                let paint = |f: f32| progress(0.2 + 0.8 * f);
                outpaint(&*painter, &Monitor { cancelled, progress: &paint })?
            }
        };
        progress(1.0);
        self.input.finish(&filled, self.model.id()).ok_or_else(|| "Generative Expand made no fill".to_string())
    }
}

/// Diffusion model `model` from its folder `dir` (on the GPU when there is one).
fn load_diffusion(model: ExpandModel, dir: &std::path::Path, mon: &Monitor<'_>) -> Result<Box<dyn Painter>, String> {
    load_diffusion_for(model, dir, mon, false)
}

/// [`load_diffusion`], asked to paint what AI Remove needs (`removal`: the background behind an object, no new object)
/// instead of what Generative Expand needs (more of the scene).
#[cfg(feature = "enhance")]
pub(crate) fn load_diffusion_for(model: ExpandModel, dir: &std::path::Path, mon: &Monitor<'_>, removal: bool) -> Result<Box<dyn Painter>, String> {
    use lightcraft_diffuse::{Target, kandinsky, sd};
    let target = Target::best();
    let gpu = target.gpu();
    let recipe = |kind| if removal { sd::Recipe::for_removal(kind, gpu) } else { sd::Recipe::for_kind(kind, gpu) };
    let r: Result<Box<dyn Painter>, lightcraft_diffuse::Error> = match model {
        ExpandModel::Sd15 => {
            sd::StableDiffusion::load(sd::Kind::Sd15, dir, target, recipe(sd::Kind::Sd15), mon).map(|m| Box::new(m) as Box<dyn Painter>)
        }
        ExpandModel::Sdxl => {
            sd::StableDiffusion::load(sd::Kind::Sdxl, dir, target, recipe(sd::Kind::Sdxl), mon).map(|m| Box::new(m) as Box<dyn Painter>)
        }
        ExpandModel::Kandinsky => kandinsky::Kandinsky::load(dir, target, kandinsky::Recipe::new(gpu), mon).map(|m| Box::new(m) as Box<dyn Painter>),
        _ => return Err(format!("{} isn't a diffusion model", model.label())),
    };
    r.map_err(|e| e.into_erase(model.label()).to_string())
}

#[cfg(not(feature = "enhance"))]
pub(crate) fn load_diffusion_for(model: ExpandModel, _dir: &std::path::Path, _mon: &Monitor<'_>, _removal: bool) -> Result<Box<dyn Painter>, String> {
    Err(format!("{} isn't available in this build", model.label()))
}

/// LaMa from `file`, loaded once into `slot`.
fn load_lama(file: &std::path::Path, slot: &Mutex<Option<Arc<Lama>>>) -> Result<Arc<Lama>, String> {
    let mut g = slot.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(l) = g.as_ref() {
        return Ok(l.clone());
    }
    let missing = || lama_missing();
    let bytes = std::fs::read(file).map_err(|_| missing())?;
    if !lightcraft_erase::lama::verify(&bytes) {
        return Err(format!("{} is damaged or not the expected file: delete it in Settings → AI Models and download it again", file.display()));
    }
    let l = Arc::new(Lama::load(&bytes).map_err(|e| e.to_string())?);
    *g = Some(l.clone());
    Ok(l)
}

fn lama_missing() -> String {
    format!(
        "{NOT_INSTALLED} (LaMa). Download it (about 208 MB, Apache-2.0 licence) when LightCraft offers it, or with `models.download {{\"id\": \"{}\", \"acknowledged\": true}}`.",
        lightcraft_erase::lama::ID
    )
}

/// Longest canvas side a model paints on this device.
fn canvas_side(model: ExpandModel) -> usize {
    match model {
        // one pass on the page's thread
        _ if WEB => lightcraft_erase::SIDE,
        _ => lightcraft_pipeline::expand::MAX_CANVAS,
    }
}

impl Session {
    /// Set this device's profile (a host that knows better than detection, or a test).
    pub fn set_device(&mut self, p: DeviceProfile) {
        self.device = p;
        self.vision.unfit = crate::vision::search_unfit(&self.device);
    }

    /// On a phone: what the system lets the app allocate (`os_proc_available_memory` at launch),
    /// the memory the profile counts (which Generative Expand models run here, and whether the search model does).
    pub fn set_app_memory(&mut self, bytes: Option<usize>) {
        if self.device.class == DeviceClass::Phone {
            self.device.memory_bytes = bytes.map(|b| b as u64);
            self.vision.unfit = crate::vision::search_unfit(&self.device);
        }
    }

    /// Whether `model` runs on this device.
    pub fn expand_available(&self, model: ExpandModel) -> bool {
        model.fits(&self.device)
    }

    /// Whether LaMa's file is in place and the user hasn't turned it off.
    pub(crate) fn lama_ready(&self) -> bool {
        self.expand_available(ExpandModel::Lama)
            && !self.enhancer.disabled.contains(lightcraft_erase::lama::ID)
            && (self.expand.lama.lock().unwrap_or_else(PoisonError::into_inner).is_some()
                || self.lama_file().is_some_and(|f| std::fs::metadata(f).is_ok_and(|m| m.len() == lightcraft_erase::lama::FILE_SIZE)))
    }

    /// LaMa, loaded once (from the models folder the first time).
    pub(crate) fn lama(&self) -> Result<Arc<Lama>, String> {
        if self.enhancer.disabled.contains(lightcraft_erase::lama::ID) {
            return Err("LaMa is turned off: turn its model on in Settings → AI Models.".into());
        }
        load_lama(&self.lama_file().ok_or_else(lama_missing)?, &self.expand.lama)
    }

    /// Drop LaMa when no job holds it and its file is there to load again. Whether it was loaded.
    pub(crate) fn unload_lama(&self) -> bool {
        let mut slot = self.expand.lama.lock().unwrap_or_else(PoisonError::into_inner);
        if slot.as_ref().is_some_and(|l| Arc::strong_count(l) == 1) && self.lama_file().is_some_and(|f| f.exists()) {
            *slot = None;
            return true;
        }
        false
    }

    fn lama_file(&self) -> Option<std::path::PathBuf> {
        self.enhancer.model_dir(lightcraft_erase::lama::ID).map(|d| d.join(lightcraft_erase::lama::FILE))
    }

    /// Whether `model` can paint now.
    pub fn expand_ready(&self, model: ExpandModel) -> bool {
        match model {
            ExpandModel::Classic => true,
            ExpandModel::MiGan => self.expand_available(model) && self.eraser_ready(),
            ExpandModel::Lama => self.lama_ready(),
            m => self.expand_available(m) && !self.enhancer.disabled.contains(m.id()) && self.enhancer.installed(m.id()),
        }
    }

    /// What Automatic picks on this device (it may still need its download).
    pub fn recommended_expand_model(&self) -> ExpandModel {
        ExpandModel::recommended(&self.device)
    }

    /// What the pickers show as chosen: the user's choice when this device runs it, else
    /// Automatic's.
    pub fn chosen_expand_model(&self) -> ExpandModel {
        self.expand.preferred.filter(|m| self.expand_available(*m)).unwrap_or_else(|| self.recommended_expand_model())
    }

    /// Choose what Generate paints with (`None`: Automatic). Refused for a model this device
    /// doesn't run.
    pub fn set_expand_preference(&mut self, model: Option<ExpandModel>) -> Result<(), String> {
        if let Some(m) = model.filter(|m| !self.expand_available(*m)) {
            return Err(format!("{} doesn't run on this device ({})", m.label(), self.device.describe()));
        }
        self.expand.preferred = model;
        Ok(())
    }

    /// What Generate uses when no model is named: the chosen model when it is here, else the most
    /// capable one that is (as Automatic orders them), else Classic.
    pub fn default_expand_model(&self) -> ExpandModel {
        let chosen = self.chosen_expand_model();
        if self.expand_ready(chosen) {
            return chosen;
        }
        [ExpandModel::Sdxl, ExpandModel::Kandinsky, ExpandModel::Sd15, ExpandModel::Lama, ExpandModel::MiGan]
            .into_iter()
            .filter(|m| self.expand_available(*m))
            .find(|m| self.expand_ready(*m))
            .unwrap_or(ExpandModel::Classic)
    }

    /// The ways to paint on this device, for the picker and agents.
    pub fn expand_choices(&self) -> Vec<ExpandChoice> {
        let (default, recommended, chosen) = (self.default_expand_model(), self.recommended_expand_model(), self.chosen_expand_model());
        ExpandModel::ALL
            .into_iter()
            .map(|m| {
                let (licence, credit) = m.licence();
                ExpandChoice {
                    id: m.id(),
                    label: m.label(),
                    note: m.note(),
                    available: self.expand_available(m),
                    ready: self.expand_ready(m),
                    disabled: m != ExpandModel::Classic && self.enhancer.disabled.contains(m.id()),
                    download_bytes: m.download_bytes(),
                    licence,
                    credit,
                    restricted: m.restricted(),
                    diffusion: m.diffusion(),
                    recommended: m == recommended,
                    chosen: m == chosen,
                    default: m == default,
                }
            })
            .collect()
    }

    fn diffusion_missing(&self, model: ExpandModel) -> String {
        format!(
            "{NOT_INSTALLED} ({}). Download it (about {:.1} GB, {} licence) when LightCraft offers it, or with `models.download {{\"id\": \"{}\", \"acknowledged\": true}}`.",
            model.label(),
            model.download_bytes() as f64 / 1e9,
            model.licence().0,
            model.id()
        )
    }

    /// Plan Generate for photo `id` with `model` (the default if `None`): check the model, render
    /// the canvas the model will see.
    pub fn expand_job(&mut self, id: PhotoId, model: Option<ExpandModel>) -> Result<ExpandJob, String> {
        let model = model.unwrap_or_else(|| self.default_expand_model());
        if !self.expand_available(model) {
            return Err(format!(
                "{} isn't available on this device ({}); use {} or Classic",
                model.label(),
                self.device.describe(),
                ExpandModel::MiGan.label()
            ));
        }
        let d = self.develop_of(id).ok_or("no such photo")?;
        if !d.crop.expand {
            return Err("Turn on Expand in the crop tool first, then drag the crop past the photo's edges".into());
        }
        let brush = match model {
            ExpandModel::Classic => Brush::Classic,
            ExpandModel::MiGan => Brush::Net(self.eraser().map_err(|e| e.replace("AI Remove", "Generative Expand"))?),
            ExpandModel::Lama => {
                if self.enhancer.disabled.contains(lightcraft_erase::lama::ID) {
                    return Err("LaMa is turned off: turn it on in Settings → AI Models.".into());
                }
                let loaded = self.expand.lama.lock().unwrap_or_else(PoisonError::into_inner).clone();
                match loaded {
                    Some(l) => Brush::Net(l),
                    None if self.lama_ready() => Brush::LamaFile { file: self.lama_file().ok_or_else(lama_missing)?, slot: self.expand.lama.clone() },
                    None => return Err(lama_missing()),
                }
            }
            m => {
                if self.enhancer.disabled.contains(m.id()) {
                    return Err(format!("{} is turned off: turn it on in Settings → AI Models.", m.label()));
                }
                match self.enhancer.model_dir(m.id()) {
                    Some(dir) if self.enhancer.installed(m.id()) => Brush::Diffusion(dir),
                    _ => return Err(self.diffusion_missing(m)),
                }
            }
        };
        let input = self.expand_canvas_input(id, canvas_side(model))?;
        Ok(ExpandJob { photo: id, model, input, brush })
    }

    /// The canvas photo `id` shows for Generative Expand at up to `side` pixels a side: what every
    /// painter is given.
    fn expand_canvas_input(&mut self, id: PhotoId, side: usize) -> Result<ExpandInput, String> {
        let d = self.develop_of(id).ok_or("no such photo")?;
        if !d.crop.expand {
            return Err("Turn on Expand in the crop tool first, then drag the crop past the photo's edges".into());
        }
        // (the picture a render develops, not the plain source: the fill is put in at the resample, so it must be in
        // the light the photo is in there, with the camera's look and the Denoise already on it)
        let (mut src, mut info) = self.develop_source_now(id, SourceLevel::Preview)?;
        let nothing = "The crop shows nothing past the photo: drag its edges outward (or pick a wider aspect) first";
        let mut input = lightcraft_pipeline::expand::expand_input(&src, &info, &d, side).ok_or(nothing)?;
        // the preview has fewer pixels than the canvas wants: read the original
        let full = self.catalog.photo(id).map_or(0, |p| p.width.max(p.height) as usize);
        if input.native < side as f64 && full > src.width.max(src.height) {
            (src, info) = self.develop_source_now(id, SourceLevel::Full)?;
            input = lightcraft_pipeline::expand::expand_input(&src, &info, &d, side).ok_or(nothing)?;
        }
        Ok(input)
    }

    /// The canvas for a painter outside the engine (`crop.expandCanvas`), as a picture of at most
    /// `max_side` pixels a side (the photo where it is, the Classic fill over the new area).
    pub fn expand_canvas(&mut self, id: PhotoId, max_side: usize) -> Result<ExternalCanvas, String> {
        let input = self.expand_canvas_input(id, max_side.clamp(64, lightcraft_pipeline::expand::MAX_CANVAS))?;
        let d = self.develop_of(id).ok_or("no such photo")?;
        let (width, height) = input.size();
        let guard = self.activity.start("expand", "Generative Expand", Cancel::No);
        let canvas = input.placeholder();
        drop(guard);
        let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        let rgba = lightcraft_raster::Rgba8::from_fn(width, height, |x, y| {
            let p = canvas.get(x, y);
            [byte(p[0]), byte(p[1]), byte(p[2]), 255]
        });
        let png = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&rgba), &lightcraft_codecs::EncodeMeta::default())
            .map_err(|e| format!("couldn't make the picture: {e}"))?;
        Ok(ExternalCanvas { key: expand_key(id, &d), width, height, png })
    }

    /// Store the picture a painter outside the engine made of the canvas (`bytes`: PNG, JPEG, HEIC…;
    /// `key` from [`Session::expand_canvas`]; `model` one of [`EXTERNAL_MODELS`]) as photo `id`'s
    /// fill: one undo step, "Generative Expand". Refused when the crop changed meanwhile or the
    /// picture isn't the canvas's shape.
    pub fn apply_expand_picture(&mut self, id: PhotoId, bytes: &[u8], key: u64, model: &str) -> Result<ExpandDone, String> {
        if !EXTERNAL_MODELS.contains(&model) {
            return Err(format!("unknown painter `{model}` (expected {})", EXTERNAL_MODELS.join(", ")));
        }
        let d = self.develop_of(id).ok_or("no such photo")?;
        if expand_key(id, &d) != key {
            return Err("The crop changed while the picture was being made: Generate again".into());
        }
        let options = lightcraft_codecs::DecodeOptions { max_size: Some((2048, 2048)), ..Default::default() };
        let decoded = lightcraft_codecs::decode(bytes, options).map_err(|e| format!("The picture couldn't be read: {e}"))?.to_srgb8();
        let (aw, ah) = (decoded.width, decoded.height);
        let answer = lightcraft_raster::Rgb32f::from_fn(aw, ah, |x, y| {
            let p = decoded.get(x, y);
            [p[0], p[1], p[2]].map(|v| f32::from(v) / 255.0)
        });
        let input = self.expand_canvas_input(id, lightcraft_pipeline::expand::MAX_CANVAS)?;
        let (width, height) = input.size();
        let guard = self.activity.start("expand", "Generative Expand", Cancel::No);
        guard.detail(model);
        let filled = input
            .from_picture(&answer)
            .ok_or_else(|| format!("The picture ({aw} x {ah}) doesn't have the shape of the area ({width} x {height}); Generate again"))?;
        let fill = input.finish(&filled, model).ok_or("Generative Expand made no fill")?;
        drop(guard);
        let stored_bytes = fill.fill.data.len();
        self.finish_expand(id, fill)?;
        Ok(ExpandDone {
            photo: id.0,
            model: EXTERNAL_MODELS.iter().copied().find(|m| *m == model).unwrap_or(EXTERNAL_MODELS[0]),
            passes: 0,
            width,
            height,
            stored_bytes,
        })
    }

    /// [`Session::apply_expand_picture`] with the picture in file `path` (at most 64 MB).
    pub fn apply_expand_picture_file(&mut self, id: PhotoId, path: &std::path::Path, key: u64, model: &str) -> Result<ExpandDone, String> {
        let len = std::fs::metadata(path).map_err(|e| format!("The picture couldn't be read: {e}"))?.len();
        if len > MAX_PICTURE_BYTES {
            return Err("The picture is too large".into());
        }
        let bytes = std::fs::read(path).map_err(|e| format!("The picture couldn't be read: {e}"))?;
        self.apply_expand_picture(id, &bytes, key, model)
    }

    /// Store a finished fill on photo `id`: one undo step. Refused when Expand was turned off
    /// meanwhile.
    pub fn finish_expand(&mut self, id: PhotoId, fill: ExpandFill) -> Result<(), String> {
        let mut d = (*self.develop_of(id).ok_or("the photo is gone")?).clone();
        if !d.crop.expand {
            return Err("Expand was turned off while the new area was painted".into());
        }
        d.expand_fill = Some(fill);
        self.set_develop(id, d, "Generative Expand").map_err(|e| e.to_string())
    }

    /// Generate for photo `id` here and now (scripts, agents, the browser), as one undo step.
    pub fn expand_now(&mut self, id: PhotoId, model: Option<ExpandModel>) -> Result<ExpandDone, String> {
        let job = self.expand_job(id, model)?;
        let passes = job.passes();
        let (width, height) = job.input.size();
        let guard = self.activity.start("expand", "Generative Expand", Cancel::No);
        guard.set_unit(Unit::Percent);
        guard.detail(job.model.label());
        let h = guard.handle();
        let fill = job.run(&|| false, &|f| h.progress((f * 100.0).round() as u64, 100))?;
        drop(guard);
        let stored_bytes = fill.fill.data.len();
        self.finish_expand(id, fill)?;
        Ok(ExpandDone { photo: id.0, model: job.model.id(), passes, width, height, stored_bytes })
    }

    /// Start Generate for photo `id` on a worker thread, with a row in the activity stack (✕
    /// cancels). The result is stored by [`Session::poll_expand`]. In the browser it runs inline.
    pub fn start_expand(&mut self, id: PhotoId, model: Option<ExpandModel>) -> Result<(), String> {
        if self.expand_running().is_some() {
            return Err("Generative Expand is already running".into());
        }
        let job = self.expand_job(id, model)?;
        let cancel = Arc::new(AtomicBool::new(false));
        let guard = self.activity.start("expand", "Generative Expand", Cancel::Flag(cancel.clone()));
        guard.set_unit(Unit::Percent);
        guard.detail(job.model.label());
        let (tx, rx) = channel();
        let (photo, model) = (job.photo, job.model);
        let flag = cancel.clone();
        let work = move || {
            let h = guard.handle();
            let run = || job.run(&|| flag.load(Ordering::Relaxed), &|f| h.progress((f * 100.0).round() as u64, 100));
            // a panicking job reports an error instead of leaving the row up forever
            let r =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(run)).unwrap_or_else(|_| Err("Generative Expand failed unexpectedly".into()));
            let _ = tx.send(r);
            drop(guard);
        };
        #[cfg(not(target_arch = "wasm32"))]
        std::thread::Builder::new().name("generative-expand".into()).spawn(work).map_err(|e| e.to_string())?;
        #[cfg(target_arch = "wasm32")]
        work();
        *self.expand.task.lock().unwrap_or_else(PoisonError::into_inner) = Some(ExpandTask { photo, model, cancel, rx });
        Ok(())
    }

    /// The photo and model of the running Generate, if one runs.
    pub fn expand_running(&self) -> Option<(PhotoId, ExpandModel)> {
        self.expand.task.lock().unwrap_or_else(PoisonError::into_inner).as_ref().map(|t| (t.photo, t.model))
    }

    /// Stop the running Generate. Whether one was running.
    pub fn cancel_expand(&self) -> bool {
        match self.expand.task.lock().unwrap_or_else(PoisonError::into_inner).as_ref() {
            Some(t) => {
                t.cancel.store(true, Ordering::Relaxed);
                true
            }
            None => false,
        }
    }

    /// Store the running Generate's result once it is done (call every frame). `None` while it
    /// runs or when none does; `Some(Ok(model))` when the fill was stored, `Some(Err(..))` when it
    /// failed or was cancelled (the message then starts with "cancelled").
    pub fn poll_expand(&mut self) -> Option<Result<ExpandModel, String>> {
        let (photo, model, r) = {
            let mut slot = self.expand.task.lock().unwrap_or_else(PoisonError::into_inner);
            let t = slot.as_ref()?;
            let r = match t.rx.try_recv() {
                Ok(r) => r,
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Disconnected) => Err("Generative Expand stopped unexpectedly".into()),
            };
            let t = slot.take()?;
            (t.photo, t.model, r)
        };
        Some(r.and_then(|fill| self.finish_expand(photo, fill)).map(|()| model))
    }
}
