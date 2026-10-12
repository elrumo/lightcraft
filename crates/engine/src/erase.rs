//! **AI Remove**: a Remove spot filled by MI-GAN (`lightcraft-erase`) with what the photo most
//! likely shows behind the stroke, instead of texture copied from around it. The model's answer
//! is stored with the spot ([`lightcraft_develop::SpotFill`]), so renders and exports never run it.
//!
//! The model is optional (about 30 MB, MIT) and never part of LightCraft. The desktop and phone
//! apps download it like the other models, after the user agreed (`models.download {"id":
//! "migan", "acknowledged": true}`, Settings → AI Models), into `<models>/migan/`; the browser
//! build fetches it itself, keeps it in the site's storage and hands the bytes over
//! ([`Session::install_eraser`]). Either way the file must be exactly the pinned one.

use std::sync::{Arc, Mutex, PoisonError};

use lightcraft_catalog::PhotoId;
use lightcraft_develop::DevelopSettings;
use lightcraft_erase::Eraser;

use crate::Session;
use crate::media::SourceLevel;

pub use lightcraft_erase::{BASE_URL, CREDIT, FILE, FILE_SIZE, ID, LICENCE, verify};

/// The start of the error when the model isn't here: the apps offer the download on it.
pub const NOT_INSTALLED: &str = "The AI Remove model (MI-GAN) is not downloaded";

/// The loaded model, kept for the session once loaded.
#[derive(Default)]
pub struct EraseModel {
    loaded: Mutex<Option<Arc<Eraser>>>,
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
    fn eraser(&self) -> Result<Arc<Eraser>, String> {
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

    /// Fill spot `index` of `d` (photo `id`'s settings) with AI Remove: render the area around
    /// the stroke, let the model paint it, store the answer in the spot.
    pub fn erase_spot(&mut self, id: PhotoId, d: &mut DevelopSettings, index: usize) -> Result<(), String> {
        let eraser = self.eraser()?;
        let info = self.source_info(id);
        let mut src = self.source_now(id, SourceLevel::Preview)?;
        let mut input = lightcraft_pipeline::spots::erase_input(&src, &info, d, index).ok_or("the stroke is outside the photo")?;
        // a small stroke on a big photo: the preview has too few pixels around it, read the original
        let full = self.catalog.photo(id).map_or(0, |p| p.width.max(p.height) as usize);
        if input.native < lightcraft_erase::SIDE as f64 && full > src.width.max(src.height) {
            src = self.source_now(id, SourceLevel::Full)?;
            input = lightcraft_pipeline::spots::erase_input(&src, &info, d, index).ok_or("the stroke is outside the photo")?;
        }
        let filled = eraser.fill(&input.image, &input.hole).map_err(|e| e.to_string())?;
        let fill = input.finish(&filled).ok_or("AI Remove made no fill")?;
        let spot = d.spots.get_mut(index).ok_or("no such spot")?;
        spot.fill = Some(fill);
        Ok(())
    }
}
