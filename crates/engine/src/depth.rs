//! **Lens Blur's depth**: how far each part of a photo is. A photo that carries depth of its own
//! (a portrait HEIC or JPEG, a DNG with a depth map) uses it; it is read with the photo
//! (`files.rs`) and rides on the source's mattes, never stored. Any other photo (iPhone ProRAW
//! included: it stores mattes but no depth) gets an estimate from Depth Anything V2 Small
//! (`lightcraft-depth`), stored in its settings ([`lightcraft_develop::LensBlur::depth`]), so
//! renders and exports never run the model.
//!
//! The model is optional (about 27 MB, Apache-2.0) and never part of LightCraft. The desktop and
//! phone apps download it like the other models, after the user agreed (`models.download {"id":
//! "depth-anything-v2-small", "acknowledged": true}`, Settings → AI Models), into
//! `<models>/depth-anything-v2-small/`; the browser build fetches it itself and hands the bytes
//! over ([`Session::install_depth_model`]). Either way the file must be exactly the pinned one.

use std::sync::{Arc, Mutex, PoisonError};

use lightcraft_catalog::PhotoId;
use lightcraft_depth::Estimator;
use lightcraft_develop::DepthMap;
use lightcraft_raster::Rgb32f;
use lightcraft_raster::resample::{Filter, resize};

use crate::Session;
use crate::media::SourceLevel;

pub use lightcraft_depth::{BASE_URL, CREDIT, FILE, FILE_SIZE, ID, LICENCE, verify};

/// The start of the error when the model isn't here: the apps offer the download on it.
pub const NOT_INSTALLED: &str = "The depth model (Depth Anything V2 Small) is not downloaded";

/// Long edge of the picture handed to the model (it works on ~690 × 518 itself).
const INPUT_EDGE: usize = 1036;

/// The loaded model, kept for the session once loaded.
#[derive(Default)]
pub struct DepthModel {
    loaded: Mutex<Option<Arc<Estimator>>>,
}

/// Where a photo's depth comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DepthSource {
    /// The file's own depth map.
    Photo,
    /// The model's estimate, stored in the settings.
    Estimated,
}

impl DepthSource {
    pub fn name(self) -> &'static str {
        match self {
            DepthSource::Photo => "photo",
            DepthSource::Estimated => "estimated",
        }
    }
}

/// `src` (scene-linear Rec. 2020) as the model wants it: display-encoded sRGB, exposed so its
/// median sits at mid-grey, highlights rolled off, at most [`INPUT_EDGE`] pixels long.
fn model_input(src: &Rgb32f) -> Rgb32f {
    let (w, h) = (src.width.max(1), src.height.max(1));
    let k = (INPUT_EDGE as f64 / w.max(h) as f64).min(1.0);
    let small = resize(src, ((w as f64 * k).round() as usize).max(1), ((h as f64 * k).round() as usize).max(1), Filter::Box);
    let lum = |c: &[f32; 3]| 0.2627 * c[0] + 0.6780 * c[1] + 0.0593 * c[2];
    let mut l: Vec<f32> = small.data.iter().map(lum).filter(|v| v.is_finite()).collect();
    l.sort_by(f32::total_cmp);
    let median = l.get(l.len() / 2).copied().unwrap_or(0.18).max(1e-4);
    let gain = (0.18 / median).clamp(0.05, 64.0);
    let m = lightcraft_color::REC2020.to_space(&lightcraft_color::SRGB).to_f32();
    let mut out = small;
    for p in out.data.iter_mut() {
        let c = p.map(|v| if v.is_finite() { v.max(0.0) * gain } else { 0.0 });
        *p = std::array::from_fn(|r| {
            let x = (m[r][0] * c[0] + m[r][1] * c[1] + m[r][2] * c[2]).max(0.0);
            // (an extended Reinhard roll-off, white at 4)
            lightcraft_color::transfer::linear_to_srgb((x * (1.0 + x / 16.0) / (1.0 + x)).min(1.0))
        });
    }
    out
}

impl Session {
    /// The depth model's file in the models folder (desktop and phone apps).
    fn depth_model_file(&self) -> Option<std::path::PathBuf> {
        self.enhancer.model_dir(ID).map(|d| d.join(FILE))
    }

    /// Whether depth can be estimated now: the model is loaded, or its file is in place, and the
    /// user hasn't turned it off.
    pub fn depth_model_ready(&self) -> bool {
        !self.enhancer.disabled.contains(ID)
            && (self.depth_model.loaded.lock().unwrap_or_else(PoisonError::into_inner).is_some()
                || self.depth_model_file().is_some_and(|f| std::fs::metadata(f).is_ok_and(|m| m.len() == FILE_SIZE)))
    }

    /// Use the model in `bytes` (the browser build's copy). They must be exactly the pinned file.
    pub fn install_depth_model(&self, bytes: &[u8]) -> Result<(), String> {
        if !verify(bytes) {
            return Err("the depth model file is damaged or not the expected one: download it again".into());
        }
        let e = Estimator::load(bytes).map_err(|e| e.to_string())?;
        *self.depth_model.loaded.lock().unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(e));
        Ok(())
    }

    /// The model, loaded once (from the models folder the first time).
    fn depth_model(&self) -> Result<Arc<Estimator>, String> {
        if self.enhancer.disabled.contains(ID) {
            return Err("Depth estimation is turned off: turn its model on in Settings → AI Models.".into());
        }
        let mut slot = self.depth_model.loaded.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(e) = slot.as_ref() {
            return Ok(e.clone());
        }
        let missing = || {
            format!(
                "{NOT_INSTALLED}. Download it (about 27 MB, Apache-2.0 licence) when LightCraft offers it, or with `models.download {{\"id\": \"{ID}\", \"acknowledged\": true}}`."
            )
        };
        let file = self.depth_model_file().ok_or_else(missing)?;
        let bytes = std::fs::read(&file).map_err(|_| missing())?;
        if !verify(&bytes) {
            return Err(format!("{} is damaged or not the expected file: delete it in Settings → AI Models and download it again", file.display()));
        }
        let e = Arc::new(Estimator::load(&bytes).map_err(|e| e.to_string())?);
        *slot = Some(e.clone());
        Ok(e)
    }

    /// Whether photo `id`'s file carries depth of its own (read with its source).
    pub fn photo_has_depth(&mut self, id: PhotoId) -> bool {
        if self.source_now(id, SourceLevel::Preview).is_err() {
            return false;
        }
        self.source_info(id).mattes.as_deref().and_then(lightcraft_pipeline::masks::Mattes::depth).is_some()
    }

    /// Where the depth photo `id` renders with comes from: a stored estimate, else the file's own.
    pub fn depth_source(&mut self, id: PhotoId) -> Option<DepthSource> {
        if self.develop_of(id).is_some_and(|d| d.lens_blur.depth.is_some()) {
            return Some(DepthSource::Estimated);
        }
        self.photo_has_depth(id).then_some(DepthSource::Photo)
    }

    /// Estimate photo `id`'s depth with the model: nearness over the whole uncropped photo, as
    /// stored in [`lightcraft_develop::LensBlur::depth`].
    pub fn estimate_depth(&mut self, id: PhotoId) -> Result<DepthMap, String> {
        let model = self.depth_model()?;
        let src = self.source_now(id, SourceLevel::Preview)?;
        let mut d = model.estimate(&model_input(&src)).map_err(|e| e.to_string())?;
        lightcraft_pipeline::lensblur::normalize(&mut d);
        DepthMap::from_nearness(d.width, d.height, &d.data).ok_or_else(|| "the depth model gave no usable answer".to_string())
    }

    /// The focal distance (0–100) on photo `id`'s subject, with `depth` (an estimate about to be
    /// stored) or the depth it renders with.
    pub fn subject_focus(&mut self, id: PhotoId, depth: Option<&DepthMap>) -> Option<f64> {
        let _ = self.source_now(id, SourceLevel::Preview);
        let info = self.source_info(id);
        let plane = match depth.or(self.develop_of(id).and_then(|d| d.lens_blur.depth.clone()).as_ref()) {
            Some(d) => Arc::new(lightcraft_raster::Plane { width: d.w as usize, height: d.h as usize, data: d.nearness()? }),
            None => info.mattes.as_deref().and_then(lightcraft_pipeline::masks::Mattes::depth)?.clone(),
        };
        Some(lightcraft_pipeline::lensblur::subject_focus(&plane, info.mattes.as_deref()))
    }

    /// The focal distance (0–100) at normalized image point `p` (the coordinates of masks and
    /// spots) of photo `id` with settings `d`.
    pub fn focus_at(&mut self, id: PhotoId, d: &lightcraft_develop::DevelopSettings, p: lightcraft_geom::Point) -> Option<f64> {
        let src = self.source_now(id, SourceLevel::Preview).ok()?;
        let info = self.source_info(id);
        let n = lightcraft_pipeline::lensblur::nearness_at(&src, &info, d, p)?;
        Some((100.0 * (1.0 - f64::from(n))).clamp(0.0, 100.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_model_input_is_display_encoded_and_bounded() {
        let src = Rgb32f::from_fn(3000, 2000, |x, _| [x as f32 / 3000.0 * 4.0, 0.1, f32::NAN]);
        let i = model_input(&src);
        assert_eq!(i.width.max(i.height), INPUT_EDGE);
        assert!(i.data.iter().flatten().all(|v| (0.0..=1.0).contains(v)), "0..1, NaN gone");
        // brighter stays brighter
        assert!(i.data[i.width - 1][0] > i.data[0][0]);
    }
}
