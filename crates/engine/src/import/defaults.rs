use super::*;

/// The develop settings a photo gets on import: raws start from their as-shot white balance with
/// default sharpening and the colour noise reduction the user chose ([`ImportDefaults::color_noise_reduction`]),
/// and file-embedded lens corrections on (as the camera intended); a user default preset
/// ([`ImportDefaults`]) goes on top.
pub fn import_defaults(p: &Photo) -> lightcraft_develop::DevelopSettings {
    p.import_defaults()
}

/// Does the photo still look as the camera rendered it (its embedded camera preview is then a
/// fair stand-in)? Not when a default preset changed it on import.
pub fn has_import_look(p: &Photo) -> bool {
    // the colour noise reduction a raw was imported with ([`ImportDefaults::color_noise_reduction`]) smooths chroma noise
    // only, which the camera's own preview doesn't show either way; moving it afterwards is an edit
    let mut camera = p.camera_defaults();
    camera.detail.nr_color = p.import_defaults().detail.nr_color;
    p.develop_is(camera)
}

/// The colour noise reduction (0–100) a raw developed from its sensor data is imported with when Settings → Import
/// doesn't say otherwise. (The camera defaults give a raw 25, Lightroom's; this is what [`ImportDefaults`] starts at.)
pub const DEFAULT_COLOR_NOISE_REDUCTION: f64 = 100.0;

/// User defaults applied on import (Settings → Import; saved in the library's `prefs.json`).
/// Presets are referenced by id; a preset that no longer exists is ignored.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ImportDefaults {
    /// Preset applied to raw files (`None` = the LightCraft default).
    pub raw_preset: Option<String>,
    /// Use a camera's own default (below) when the photo's camera has one.
    pub per_camera: bool,
    /// Per-camera raw defaults, keyed by the camera's make + model as shown in Info
    /// (`"Canon EOS R5"`).
    pub cameras: Vec<CameraDefault>,
    /// Preset applied to non-raw images (JPEG, PNG, TIFF, HEIC…; `None` = none).
    pub other_preset: Option<String>,
    /// Copyright notice given to imported photos that don't carry one (empty = none).
    pub copyright: String,
    /// Creator given to imported photos that don't name one (empty = none).
    pub creator: String,
    /// Metadata preset applied to every imported photo (`metadata.*`; `None` = none).
    pub metadata_preset: Option<String>,
    /// Auto import: a watched folder whose new photos are added as they arrive (`None` = off),
    /// copied into the library's Originals instead of added in place when `auto_copy`, and an
    /// album they go to.
    pub auto_folder: Option<String>,
    pub auto_copy: bool,
    pub auto_album: Option<String>,
    /// Auto import also looks in the watched folder's subfolders (off: its top level only).
    pub auto_subfolders: bool,
    /// Folder and file names scanning and importing skip, wherever they are below the folder
    /// chosen ([`is_ignored`]): `*.fcpbundle`, `Proxy Media`.
    pub ignore: Vec<String>,
    /// Open the import review when a camera or memory card is connected.
    pub show_on_card: bool,
    /// Colour noise reduction (0–100) a raw developed from its sensor data starts with, under the raw preset (a preset
    /// that sets Detail wins). It is part of the photo's import look, so it isn't an edit and Reset returns to it;
    /// photos already in the library keep what they have. Read through [`ImportDefaults::color_noise_reduction`].
    pub color_noise_reduction: f64,
}

impl Default for ImportDefaults {
    fn default() -> Self {
        // (every field named, so a new one can't be forgotten here)
        ImportDefaults {
            raw_preset: None,
            per_camera: false,
            cameras: Vec::new(),
            other_preset: None,
            copyright: String::new(),
            creator: String::new(),
            metadata_preset: None,
            auto_folder: None,
            auto_copy: false,
            auto_album: None,
            auto_subfolders: false,
            ignore: Vec::new(),
            show_on_card: false,
            color_noise_reduction: DEFAULT_COLOR_NOISE_REDUCTION,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CameraDefault {
    /// Make + model (`Meta::camera`).
    pub camera: String,
    /// Preset id; `None` = the LightCraft default for this camera.
    pub preset: Option<String>,
}

impl ImportDefaults {
    /// [`ImportDefaults::color_noise_reduction`] as a develop slider takes it: a value from a hand-edited or synced
    /// `prefs.json` that is out of range, or not a number, is brought back (not a number: the default).
    pub fn color_noise_reduction(&self) -> f64 {
        if self.color_noise_reduction.is_finite() { self.color_noise_reduction.clamp(0.0, 100.0) } else { DEFAULT_COLOR_NOISE_REDUCTION }
    }

    /// The preset id that applies to a photo of this kind and camera, if any.
    pub fn preset_for(&self, raw: bool, camera: &str) -> Option<&str> {
        if !raw {
            return self.other_preset.as_deref();
        }
        if self.per_camera
            && let Some(c) = self.cameras.iter().find(|c| !camera.is_empty() && c.camera.eq_ignore_ascii_case(camera))
        {
            return c.preset.as_deref();
        }
        self.raw_preset.as_deref()
    }
}

/// Whether a photo that comes with `sidecar` may keep the whole sensor of a raw its camera crops
/// ([`crate::media::ProbeInfo::camera_crop`]). Settings made elsewhere (Lightroom's, in a sidecar) are relative to the
/// camera's frame, which is the picture Lightroom shows: a photo that comes with any keeps that frame, as every photo
/// imported before has it.
pub(super) fn keeps_whole_sensor(sidecar: &Option<crate::sidecar::SidecarData>) -> bool {
    sidecar.as_ref().is_none_or(|sc| sc.develop.is_none() && sc.upright.is_none())
}

/// Give a photo the crop its file asks for ([`crate::media::ProbeInfo::default_crop`]) as part of its import look: the
/// photo isn't edited by it, Reset returns to it, and the crop tool can widen it to the whole picture the camera
/// recorded. Call it after [`apply_import_defaults`] (or whatever else sets the photo's starting settings).
pub fn apply_default_crop(p: &mut Photo, crop: Option<[f64; 4]>) {
    let Some([x0, y0, x1, y1]) = crop.filter(|c| c.iter().all(|v| v.is_finite()) && c[0] < c[2] && c[1] < c[3]) else { return };
    let mut d = (*p.develop).clone();
    d.crop.geometry.rect = lightcraft_geom::Rect::new(x0, y0, x1, y1);
    let look = std::sync::Arc::new(d);
    p.develop = look.clone();
    p.import_look = Some(look);
}

/// Give a freshly imported photo its default settings: the camera defaults, then the matching
/// default preset ([`ImportDefaults::preset_for`]), remembered as the photo's import look.
pub fn apply_import_defaults(s: &Session, p: &mut Photo) {
    // metadata defaults fill gaps only: the file's own copyright / creator win
    let d = &s.import_defaults;
    if p.meta.copyright.trim().is_empty() && !d.copyright.trim().is_empty() {
        p.meta.copyright = d.copyright.trim().to_string();
    }
    if p.meta.creator.trim().is_empty() && !d.creator.trim().is_empty() {
        p.meta.creator = d.creator.trim().to_string();
    }
    if let Some(mp) = d.metadata_preset.as_ref().and_then(|n| s.metadata_presets.iter().find(|m| m.name.eq_ignore_ascii_case(n))) {
        crate::cmd::metadata::apply_to(&mut p.meta, &mp.fields);
    }
    let camera = p.camera_defaults();
    // a raw shown from its embedded JPEG gets the rendered-file default, not the raw one
    let raw = p.develops_raw();
    // a raw's colour noise reduction is the user's (Settings → Import): under the preset, and part of the import look
    let mut base = camera.clone();
    if raw {
        base.detail.nr_color = d.color_noise_reduction();
    }
    let preset = s.import_defaults.preset_for(raw, &p.meta.camera).and_then(|id| s.presets.iter().find(|x| x.id == id));
    let look = match preset {
        // a preset carries no Upright correction (an old one may still hold one: not this photo's)
        Some(pr) => std::sync::Arc::new(crate::upright_transfer::apply_preset(p, &base, pr, 1.0)),
        None => std::sync::Arc::new(base),
    };
    p.develop = look.clone();
    // remembered only when it differs from what the camera gives: a photo with no user default has none
    p.import_look = (*look != camera).then_some(look);
}
