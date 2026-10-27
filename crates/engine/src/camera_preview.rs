//! Estimate the starting look of a raw without a camera colour matrix (Sony ARW, Nikon NEF, Panasonic
//! RW2, Fujifilm RAF, Canon CR2/CR3, Pentax PEF, Samsung SRW, Olympus ORF) from its own JPEG. Colour and luminance are fitted separately; the JPEG supplies correspondences only,
//! never output pixels or a replacement for RAW editing.
//! A global matrix can't follow the camera's hue-dependent rendering (the best matrix rendered a
//! lime shirt olive that the camera kept lime): a hue/saturation table fitted to the residuals
//! (applied like a DNG `ProfileHueSatMap`) corrects that when it also improves the held-out pixels.
use lightcraft_color::{D50, D65, Mat3, PROPHOTO, REC2020, Xy, bradford, luminance_2020};
use lightcraft_pipeline::CameraMatch;
use lightcraft_pipeline::tone::{CHROMA_N, CameraTone, ToneMap};
use lightcraft_raster::{
    Rgb32f,
    resample::{Filter, fit, resize},
};
use lightcraft_raw::{
    ColorData, RawFormat, RawImage,
    color::CameraTransform,
    profile::{HsvTable, ProfileLook, ToneCurve},
};

mod colour;
mod hue_sat;
mod look_fit;
mod mono;
mod pairs;
mod proxy;
mod tone;

#[cfg(test)]
pub(crate) use colour::{PROFILE_PROXY, dng_colour, dng_tone_eligible};
#[cfg(test)]
use colour::{PROXY, starting_colour_with};
pub(crate) use colour::{
    camera_matched, dng_tone_curve, file_local_look, previewless_tone, starting_colour, starting_colour_tags, with_starting_colour,
};
pub(crate) use hue_sat::HueSat;
#[cfg(test)]
use hue_sat::{MIN_LUMINANCE_KEPT, NO_LUMINANCE_KEPT, fit_hue_sat, hue_saturation, restore_weight};
#[cfg(test)]
pub(crate) use look_fit::fit_preview;
#[cfg(test)]
use look_fit::{
    FIT_PREVIEW_MAX_PIXELS, SENSOR_CLIP, at_most, clips_more, correct_vignetting, fit_look, fit_look_with, sony_dro_off_tone, with_dro_off_tone,
    without_clipped,
};
pub(crate) use look_fit::{fit_neutral, neutral_pairs};
#[cfg(test)]
use mono::{fit_dark_tone, fit_tone_only, monochrome, monochrome_look, to_prophoto};
#[cfg(test)]
use pairs::{
    Attempt, Pairs, TONE_FITS, collect_pairs, colour_pair, displayed, fit_attempt, fit_pairs, fit_pairs_ordered, fit_pairs_with, fit_partial,
    fit_tone_with_shadows, luma, rank_correlation_ties, search_ordered,
};
#[cfg(test)]
pub(crate) use proxy::LensGeometry;
#[cfg(test)]
use proxy::{
    Cr3Framing, MIN_PREVIEW_FRACTION, alignment, estimate_cr3_framing, into_lens_geometry, proxies, reference_at, sensor_proxy, structure_agrees,
};
pub(crate) use proxy::{decode_fits, fit_profile, forget_fits, lens_geometry, profile_pairs};
#[cfg(test)]
pub(crate) use proxy::{jpeg_vignetting, sharing_proxies};
#[cfg(test)]
use tone::{HighlightCheck, ToneFit, fit_tone, fit_tone_conditional, highlights};

/// The version of the camera-look fit: how a raw's colour matrix, hue/saturation table and tone
/// and chroma curves are chosen. Smart previews record it in their header (`look_version`), because
/// they keep the curve and the colour the fit produced; one written by an older version is rebuilt
/// from its original when that is online, and kept as it is while it is offline.
///
/// **Bump this whenever a change makes the fit give a different result for the same file** (the
/// fit, its gates, the fallbacks, the camera profiles' effect on it). Do not bump it for changes
/// that leave the fitted look alone. `RENDER_CACHE_VERSION` covers renders; this covers proxies.
///
/// 1: the fit as of #499's follow-up; 2: Sony DRO (tone curve lowered to Sony's curve without DRO)
/// and the ILCE-7CR profile (#528, #583, #568, #616); 3: the camera JPEG read in the colour space the raw's EXIF
/// states, for every make (#778).
/// 4: a tone curve from black-and-white camera JPEGs (#577).
/// 5: dark JPEG luminance pairs for the camera toe (#1307).
/// 6: the ILCA-99M2 profile.
/// 7: the sensor proxy follows the file's lens warp when the camera JPEG is lens-corrected.
/// 8: the bundled NIKON Z 6_2 profile.
/// 9: the bundled NIKON Z 8 and NIKON D5300 profiles.
/// 10: a DNG `ProfileToneCurve` carries the chroma of a per-channel curve (`files::dng_tone_curve`); from this
/// version every raw's proxy, DNGs too, goes stale on a bump.
/// 11: a CR3 of a model with measured matrices starts from the neutral look, not a fit to its camera JPEG.
/// 12: the EOS R6's neutral look adds its hue/saturation table (`camera_profiles::NEUTRAL_BUNDLED`).
/// 13: that table refitted with pooled pairs in a fixed order (`camera_profiles::in_order`).
/// 14: such a CR3's proxy also carries the look fitted to its camera JPEG (the Camera Matching profile).
/// 15: a CR3's look is fitted to the sensor with the vignetting correction its camera applied to the JPEG
/// (`jpeg_vignetting`, which the render applies too).
/// 16: the median Canon tone for a CR3 without an embedded JPEG (`previewless_tone`).
/// 17: the neutral tone refitted on frames with that vignetting correction ([`NEUTRAL_TONE`]), the EOS R6 table with it.
/// 18: a DNG with a look table and a 1024 px (Lightroom) preview gets its tone fitted too (upstream's look version 10).
/// 19: upstream's look versions 11 and 12: tone curves of low-contrast scenes (range limit 1.2, with the bright-area
/// check for curves under 1.5, #699); a tone-only fallback for near-black scenes with too few colour pairs (#1587); DNG
/// tone fitting uses a lens-corrected proxy when the file carries lens opcodes; validated A7CR distortion, including
/// the lens data stored with each smart preview.
/// 20: a raw whose camera JPEG gives no look starts from its camera's typical colour and a neutral tone at its maker's
/// exposure ([`typical_look`]), not from camera RGB as sRGB with the default tone.
pub const LOOK_VERSION: u32 = 20;

#[derive(Clone, Debug)]
pub(crate) struct CameraLook {
    pub matrix: Mat3,
    pub tone: CameraTone,
    /// Hue/saturation correction after `matrix`, in linear ProPhoto RGB (DNG `ProfileHueSatMap`).
    pub hue_sat: Option<HsvTable>,
}

#[cfg(test)]
mod tests;
