use super::colour::{PROFILE_PROXY, PROXY, file_local_look, spectral};
use super::mono::{fit_tone_only_with, monochrome};
use super::pairs::{Attempt, EDGE_CONTRAST, TONE_FITS, collect_pairs, fit_attempt, fit_partial, luma, search_ordered};
use super::proxy::{CLIP_TOLERANCE, LensGeometry, MIN_PREVIEW_FRACTION, alignment, proxies, proxies_from, structure_agrees, with_jpeg_vignetting};
use super::tone::{HighlightCheck, highlights, median};
use super::hue_sat::{fit_hue_sat};
use super::*;

/// The colour a raw's camera most likely has when its file has no colour matrices, as the matrix that takes what the
/// fallback transform `t` gives (white-balanced camera RGB read as sRGB) to what the camera's measured matrices give,
/// or, for a camera nobody measured, its maker's typical camera ([`lightcraft_raw::spectral::generic`]). Used where
/// the camera JPEG gives no look to fit, in place of "camera RGB is sRGB": leaving each of 52 measured cameras out in
/// turn, the typical camera is within CIEDE2000 2.0 of the camera's own best matrix (camera RGB as sRGB: 8.7). It
/// keeps white, so neutrals stay neutral. `None` when `t` has matrices of its own.
pub(super) fn prior(raw: &RawImage, t: &CameraTransform) -> Option<Mat3> {
    if !t.matrix_is_fallback {
        return None;
    }
    let camera = spectral(raw).unwrap_or_else(|| lightcraft_raw::spectral::generic(raw.metadata.make.as_deref()));
    let mut color = ColorData { profile: Default::default(), ..raw.color.clone() };
    if !camera.fill(&mut color) {
        return None;
    }
    let g = lightcraft_raw::color::camera_transform_of(&color, lightcraft_raw::color::as_shot_white_of(&color, raw.wb_multipliers));
    // both matrices act on white-balanced camera RGB; the multipliers are the file's, so they agree, but a file
    // without any is balanced by each model's own daylight, which the composition below carries
    let total = |t: &CameraTransform| t.matrix.mul(&Mat3::diag(f64::from(t.wb[0]), f64::from(t.wb[1]), f64::from(t.wb[2])));
    let to = total(&g).mul(&total(t).inverse()?);
    to.0.iter().flatten().all(|v| v.is_finite() && v.abs() < 16.0).then_some(to)
}

/// The look fitted to the file's camera JPEG, and whether that JPEG is black and white ([`monochrome`]).
pub(crate) fn fit_preview(raw: &RawImage, bytes: &[u8], transform: &CameraTransform, lens: Option<&LensGeometry>) -> (Option<CameraLook>, bool) {
    if !transform.matrix_is_fallback || !file_local_look(raw.format) {
        return (None, false);
    }
    let Some((sensor, reference, clipped)) = proxies(raw, bytes, transform, PROXY, 0.0, lens) else {
        if lightcraft_pipeline::profiling() {
            eprintln!("[profile] {:?} camera look: no proxies to fit (see above)", raw.format);
        }
        return (None, false);
    };
    let sensor = with_jpeg_vignetting(raw, sensor, &reference, &clipped);
    let black_and_white = monochrome(&reference);
    // A camera profile pooled from many photos knows colours this photo shows too little of;
    // try its colour first and fit tone/chroma per photo (picture styles vary).
    let profile = raw.metadata.model.as_deref().and_then(crate::camera_profiles::get);
    let colour = profile.as_ref().and_then(|p| Some((p.matrix().mul(&transform.matrix.inverse()?), p.hue_sat.clone())));
    // Sony's Dynamic Range Optimizer (on by default) brightens the camera JPEG's darker regions,
    // not the raw: keep the colour fitted to the JPEG, take the tone curve without DRO.
    let dro = lightcraft_raw::embedded_preview_dynamic_range_optimized(bytes) == Some(true);
    let look = match fit_look_with(&sensor, &reference, &clipped, colour.clone(), dro) {
        Some(look) => look,
        // A black-and-white JPEG has no colour to fit: the profile's colour and the JPEG's tone curve
        // (`monochrome_look` does it without a profile).
        None if colour.is_some() && black_and_white => {
            let unclipped = without_clipped(&sensor, &clipped);
            let bright = HighlightCheck { pairs: highlights(&sensor, &reference), dro, grey: true };
            let Some(look) = fit_tone_only_with(unclipped.as_ref().unwrap_or(&sensor), &reference, colour, &bright) else { return (None, true) };
            look
        }
        None => return (None, black_and_white),
    };
    let look = if dro { with_dro_off_tone(look) } else { look };
    if lightcraft_pipeline::profiling() {
        eprintln!(
            "[profile] {:?} camera look: {:?}, {:?}, hue/sat table {}, camera profile available {}, DRO tone replaced {dro}",
            raw.format,
            look.matrix.0,
            look.tone,
            look.hue_sat.is_some(),
            profile.is_some()
        );
    }
    (Some(look), black_and_white)
}

/// Sensor level (normalised, white = 1) at or above which a raw channel counts as clipped, as in
/// the render's highlight reconstruction.
pub(super) const SENSOR_CLIP: f32 = 0.99;

/// [`fit_look_with`] for a photo without Sony's DRO.
#[cfg(test)]
pub(super) fn fit_look(sensor: &Rgb32f, reference: &Rgb32f, clipped: &[bool], colour: Option<(Mat3, Option<HsvTable>)>) -> Option<CameraLook> {
    fit_look_with(sensor, reference, clipped, colour, false)
}

/// The photo's look, refitted without the proxy pixels whose sensor values are clipped.
///
/// A clipped raw pixel no longer records the scene's colour: with one channel held at the clip
/// level while the others keep rising, a white overcast sky comes out magenta or blue through the
/// camera matrix, while the camera JPEG shows it white. As correspondences such pixels teach the
/// colour matrix to pull colours toward grey, and the chroma curve to remove colour at their
/// display luminance (on a Z 6II forest under a clipped sky the curve cut colourfulness above
/// display luminance 0.4 to 13–19%, and every one of its samples there was clipped). The render
/// rebuilds clipped highlights separately (`lightcraft_raw::highlight`), so the look is fitted on
/// what the sensor measured.
///
/// The search on all pixels decides, as before, whether the photo gets a look and which attempt
/// of [`search_ordered`] gives it; only that attempt is refitted without the clipped pixels, and
/// when the refit misses the acceptance gates the look on all pixels is kept. So no photo loses
/// its look and none switches attempt (a refit that just clears the gate on all pixels must not
/// replace a look fitted away from edges). A photo that had no look gets one when the search
/// without the clipped pixels finds it, and otherwise the [`fit_partial`] look when that is clearly
/// closer to the camera JPEG than the neutral fallback.
pub(super) fn fit_look_with(
    sensor: &Rgb32f,
    reference: &Rgb32f,
    clipped: &[bool],
    colour: Option<(Mat3, Option<HsvTable>)>,
    dro: bool,
) -> Option<CameraLook> {
    let unclipped = without_clipped(sensor, clipped);
    // from the proxies as they are: the retries' filters (clipped pixels, edges) don't remove them
    let bright = HighlightCheck { pairs: highlights(sensor, reference), dro, grey: false };
    let Some((look, attempt)) = search_ordered(sensor, reference, colour.clone(), TONE_FITS, &bright) else {
        return unclipped
            .as_ref()
            .and_then(|u| search_ordered(u, reference, colour, TONE_FITS, &bright))
            .map(|(look, _)| look)
            .or_else(|| fit_partial(unclipped.as_ref().unwrap_or(sensor), reference, &bright));
    };
    let Some(unclipped) = unclipped else { return Some(look) };
    let refit = fit_attempt(&unclipped, reference, &colour, attempt, &bright);
    if refit.is_none() && lightcraft_pipeline::profiling() {
        eprintln!("[profile] camera look: no fit without the clipped pixels on the accepted attempt, keeping the fit on all pixels");
    }
    Some(refit.unwrap_or(look))
}

/// `sensor` with its clipped pixels set to NaN (left out of every training pair), or `None` when
/// none is clipped.
pub(super) fn without_clipped(sensor: &Rgb32f, clipped: &[bool]) -> Option<Rgb32f> {
    if clipped.len() != sensor.data.len() || !clipped.iter().any(|c| *c) {
        return None;
    }
    let mut out = sensor.clone();
    for (p, c) in out.data.iter_mut().zip(clipped) {
        if *c {
            *p = [f32::NAN; 3];
        }
    }
    if lightcraft_pipeline::profiling() {
        eprintln!(
            "[profile] camera look: {} of {} proxy pixels have a clipped sensor channel",
            clipped.iter().filter(|c| **c).count(),
            clipped.len()
        );
    }
    Some(out)
}

/// 1 where any channel of a camera-RGB pixel is at or above [`SENSOR_CLIP`], else 0.
pub(super) fn clip_mask(camera: &Rgb32f) -> Rgb32f {
    let mut mask = camera.clone();
    mask.map_in_place(|p| if p.iter().any(|v| *v >= SENSOR_CLIP) { [1.0; 3] } else { [0.0; 3] });
    mask
}

/// LightCraft's neutral raw tone, for a camera JPEG that is no neutral reference ([`neutral_look`]): scene luminance
/// `x` raised to a contrast power about grey, `t = grey · (x · 2^e / grey)^c`, rolled off to white as `o = t / (1 + t)`
/// and given a soft black point ([`NEUTRAL_TOE`]). Exposure `e` (EV) and contrast `c` are the curve's two parameters, set so that on 19 Canon EOS R6
/// photos (custom picture styles, Auto Lighting Optimizer on) the whole render, vignetting correction included, matches
/// Lightroom's default export: near-neutral pixels within 1.3 L* of it in every band of Lightroom's lightness
/// (`docs/camera-preview-colour.md`; first set to 1.494 / 1.65 on frames without the correction, which then rendered
/// 2.7 L* too bright). Lightroom's output was measured, nothing of Adobe's read.
const NEUTRAL_TONE: (f32, f32) = (1.30, 1.80);
/// Width of the neutral tone's soft black point: `o` becomes `soft(o)`, `soft(v) = (v + √(v² + w²)) / 2` renormalised
/// so 0 and 1 stay put (`ToneMap::hdr`'s toe at its zero offset), which darkens the deepest shadows a little.
const NEUTRAL_TOE: f32 = 0.004;
/// Colourfulness of the neutral tone: `s · k^a` of the per-channel chroma `k` (`lightcraft_pipeline::tone::channel_chroma`): a curve
/// that steep applied to each channel over-colours shadows (+34% against the same Lightroom exports) and a
/// luminance-only curve keeps highlights too colourful (+76%); this lands within 10% at every brightness.
const NEUTRAL_CHROMA: (f32, f32) = (0.926, 0.765);

/// LightCraft's neutral starting look on a Canon camera's measured matrices ([`neutral_look_for`] for Canon).
#[cfg(test)]
fn neutral_look(model: Option<&str>) -> Option<CameraLook> {
    neutral_look_for(model, Some("Canon"))
}

/// LightCraft's neutral starting look for camera `model` of `make`: the tone at the exposure and contrast fitted for the
/// model (`camera_profiles::neutral_table`, `lightcraft-cli calibrate --references`) or, without a fit, at the maker's
/// exposure ([`maker_exposure`]) and [`NEUTRAL_TONE`]'s contrast, with [`NEUTRAL_CHROMA`], and the model's hue/saturation
/// table when it has one ([`neutral_pairs`]). `None` only if the built-in curve were invalid (a test checks it).
pub(super) fn neutral_look_for(model: Option<&str>, make: Option<&str>) -> Option<CameraLook> {
    let table = model.and_then(crate::camera_profiles::neutral_table);
    let e = table.as_ref().and_then(|t| t.exposure).unwrap_or_else(|| maker_exposure(make));
    let c = table.as_ref().and_then(|t| t.contrast).unwrap_or(NEUTRAL_TONE.1);
    Some(CameraLook { hue_sat: table.map(|t| t.hue_sat.clone()), ..neutral_curve(e, c)? })
}

/// The neutral tone's display value (linear) at scene luminance `x`, exposure `e` EV and contrast `c`.
fn neutral_value(x: f32, e: f32, c: f32) -> f32 {
    let t = lightcraft_pipeline::tone::GREY * (x * 2f32.powf(e) / lightcraft_pipeline::tone::GREY).powf(c);
    let soft = |v: f32| 0.5 * (v + (v * v + NEUTRAL_TOE * NEUTRAL_TOE).sqrt());
    (soft(t / (1.0 + t)) - soft(0.0)) / (soft(1.0) - soft(0.0))
}

/// The neutral tone at exposure `e` EV and contrast `c` with [`NEUTRAL_CHROMA`] and no table.
fn neutral_curve(e: f32, c: f32) -> Option<CameraLook> {
    use lightcraft_pipeline::tone::{GREY, channel_chroma};
    let (mut e, mut c) = (e, c);
    if let Ok(v) = std::env::var("LC_NEUTRAL_TONE") {
        let mut it = v.split(',').filter_map(|x| x.parse::<f32>().ok());
        e = it.next().unwrap_or(e);
        c = it.next().unwrap_or(c);
    }
    // 32 knots, evenly spaced in log luminance from 12 stops under grey to 5 over (beyond: the bounded shoulder)
    let knots = std::array::from_fn(|i| {
        let x = GREY * 2f32.powf(-12.0 + 17.0 * i as f32 / 31.0);
        [x, neutral_value(x, e, c)]
    });
    let tone = CameraTone::new(knots)?;
    let (s, a) = NEUTRAL_CHROMA;
    let tone = tone.with_chroma(channel_chroma(|y| tone.apply(y)).map(|k| (s * k.powf(a)).clamp(0.0, 4.0)))?;
    Some(CameraLook { matrix: Mat3::IDENTITY, tone, hue_sat: None })
}

/// The exposure ([`NEUTRAL_TONE`]'s, in EV) at which a maker's raws start when nothing else says: the tone a camera
/// JPEG puts mid-grey at differs between makers by up to 0.8 EV, so the neutral tone is moved by how far the maker's
/// median is from Canon's, whose 1.30 EV was fitted to Lightroom's exports. The medians are those of the neutral
/// curve's exposure (contrast 1.80) best matching each fitted camera look of the CC0 archive's raws (Canon 106 files
/// 1.23, Sony 34 1.18, Nikon 75 1.24, Fujifilm 70 1.00, Panasonic and Leica 15 1.07, Olympus 17 0.61, Pentax 15 0.46,
/// Samsung 6 0.67, Epson 3 0.68, Minolta 3 0.49; 354 files 1.12) and it assumes the camera JPEG stands to
/// Lightroom's rendering as Canon's does (Sony's, measured on 24 ILCE-7CR photos with DRO off, does:
/// Lightroom is +0.15 L* from the camera JPEG). An estimate, until a maker's own Lightroom exports are fitted.
fn maker_exposure(make: Option<&str>) -> f32 {
    let make = make.unwrap_or_default().to_uppercase();
    let has = |names: &[&str]| names.iter().any(|n| make.contains(n));
    let median = if has(&["CANON"]) {
        CANON_MEDIAN_EXPOSURE
    } else if has(&["SONY"]) {
        1.18
    } else if has(&["NIKON"]) {
        1.24
    } else if has(&["FUJI"]) {
        1.00
    } else if has(&["PANASONIC", "LEICA"]) {
        1.07
    } else if has(&["OLYMPUS", "OM DIGITAL"]) {
        0.61
    } else if has(&["PENTAX", "RICOH"]) {
        0.46
    } else if has(&["SAMSUNG"]) {
        0.67
    } else if has(&["EPSON"]) {
        0.68
    } else if has(&["MINOLTA", "KONICA"]) {
        0.49
    } else {
        1.12
    };
    NEUTRAL_TONE.0 + (median - CANON_MEDIAN_EXPOSURE)
}

/// The Canon median of [`maker_exposure`]'s table.
const CANON_MEDIAN_EXPOSURE: f32 = 1.23;

/// What a raw starts from when its camera JPEG gave no look to fit (no JPEG, one that does not line up with the sensor,
/// or a scene the fit's gates refuse): the colour of its camera when that is known (measured matrices, already in `t`)
/// or else its maker's typical camera ([`prior`]), and the neutral look for its model ([`neutral_look_for`]: the tone at
/// the maker's exposure, [`maker_exposure`], unless the model was fitted to Lightroom's renders).
/// Without it such a photo rendered camera RGB as sRGB with the default tone, about 11 L* too dark and 0.4 as colourful
/// as the camera's own picture on the CC0 archive's Sony A33, A500 and A580 and Samsung NX20 and NX200.
pub(super) fn typical_look(raw: &RawImage, t: &CameraTransform) -> Option<CameraLook> {
    // (a DNG without matrices is camera RGB on purpose: a linear export, not a camera's own raw)
    if raw.format == RawFormat::Dng {
        return None;
    }
    let matrix = if t.matrix_is_fallback { prior(raw, t)? } else { Mat3::IDENTITY };
    Some(CameraLook { matrix, ..neutral_look_for(raw.metadata.model.as_deref(), raw.metadata.make.as_deref())? })
}

/// Colour pairs for a neutral-look table of a CR3's camera model ([`neutral_look`]): the scene colour the table sees
/// (white-balanced, through the model's spectral matrices, linear Rec.2020) → `reference`, an upright render of the
/// same photo by another raw converter (encoded image file bytes), with the neutral look's chroma curve taken out
/// ([`without_chroma`]), so the table makes up only what the tone and chroma curves leave. The render may be cropped
/// slightly or lens-corrected: the pairs line up as for a camera JPEG ([`align_cr3_framing`]), away from edges, and
/// clipped sensor pixels are left out. `None` for other files or renders that don't line up with it
/// ([`MIN_REFERENCE_ALIGNMENT`]). The pairs are whole: [`fit_neutral`] takes the neutral tone out of them first.
pub(crate) fn neutral_pairs(raw: &mut RawImage, reference: &[u8]) -> Option<Vec<([f64; 3], [f64; 3])>> {
    if lightcraft_raw::color::has_matrix(&raw.color) || !file_local_look(raw.format) {
        return None;
    }
    // the colour the render starts from: the model's measured matrices, else its maker's typical camera
    spectral(raw).unwrap_or_else(|| lightcraft_raw::spectral::generic(raw.metadata.make.as_deref())).fill(&mut raw.color);
    let transform = lightcraft_raw::color::camera_transform(raw, lightcraft_raw::color::as_shot_white_xy(raw));
    let edge = (2 * PROFILE_PROXY).max(384) as u32;
    let decoded =
        lightcraft_codecs::decode(reference, lightcraft_codecs::DecodeOptions { max_size: Some((edge, edge)), max_pixels: FIT_PREVIEW_MAX_PIXELS })
            .ok()?;
    let upright = decoded.to_working().into_oriented(lightcraft_geom::Orientation::from_exif(decoded.orientation));
    // back into the sensor's orientation, where the camera JPEG would be
    let (sensor, reference, clipped) = proxies_from(raw, upright.into_oriented(raw.orientation.inverse()), &transform, PROFILE_PROXY, None)?;
    // a render of another photo (a name mix-up) or turned the wrong way would only add noise
    if alignment(&sensor, &reference) < MIN_REFERENCE_ALIGNMENT {
        return None;
    }
    let sensor = without_clipped(&sensor, &clipped).unwrap_or(sensor);
    let (pairs, _) = collect_pairs(&sensor, &reference, 0.05, Some(EDGE_CONTRAST), false)?;
    // An sRGB render clips colours outside sRGB to its edge (a deep blue sky loses its red channel): there its
    // saturation is the file's limit, not the converter's look, and fitted it cut the sky's colour by a fifth.
    let to_srgb = lightcraft_color::SRGB.from_xyz().mul(&REC2020.to_xyz());
    let inside = |y: &[f64; 3]| to_srgb.apply(*y).iter().all(|v| (SRGB_EDGE..=1.0 - SRGB_EDGE).contains(v));
    Some(pairs.into_iter().filter(|(_, y)| inside(y)).collect())
}

/// How close to 0 or 1 a linear sRGB channel of a reference render may come before the pair is left out (about 3 of
/// 255 in sRGB near black, where JPEG noise leaves a clipped channel).
const SRGB_EDGE: f64 = 0.001;

/// How well a reference render must line up with the sensor ([`alignment`], high-passed log luminance) for
/// [`neutral_pairs`]: Lightroom's exports of 19 EOS R6 photos reached 0.10 to 0.81, the same exports turned upside down
/// or of another photo at most 0.03. (The whole-picture [`structure_agrees`] refused a beach of sky and sand, whose
/// large-scale brightness the render's own vignetting correction changes.)
const MIN_REFERENCE_ALIGNMENT: f64 = 0.05;

/// A displayed colour with `tone`'s chroma curve undone: the colour that, scaled by the curve's chroma at its
/// luminance, displays as `y`. The same luminance and hue; `None` where the curve keeps too little chroma to undo
/// (near white) or the colour is black or white.
fn without_chroma(y: [f64; 3], tone: &ToneMap) -> Option<[f64; 3]> {
    let o = luma(y);
    let k = f64::from(tone.chroma_scale(o as f32));
    (o > 0.0 && o < 1.0 && k >= 0.25).then(|| y.map(|v| o + (v - o) / k))
}

/// What [`fit_neutral`] gives for a camera model.
#[derive(Clone, Debug)]
pub(crate) struct NeutralFit {
    /// Exposure (EV) and contrast of the neutral tone.
    pub exposure: f32,
    pub contrast: f32,
    pub hue_sat: HsvTable,
}

/// The neutral look of a camera model fitted to [`neutral_pairs`] pooled from many photos: the exposure and contrast
/// at which the neutral tone best follows the render's brightness ([`fit_neutral_tone`]; the maker's exposure and
/// [`NEUTRAL_TONE`]'s contrast where the pairs say too little), then, with that tone's chroma curve taken out of the
/// render's colours, the hue/saturation table that makes up what the tone and chroma curve leave.
pub(crate) fn fit_neutral(pairs: &[([f64; 3], [f64; 3])], make: Option<&str>) -> Option<NeutralFit> {
    let (exposure, contrast) = fit_neutral_tone(pairs).unwrap_or((maker_exposure(make), NEUTRAL_TONE.1));
    let tone = ToneMap::camera(&neutral_curve(exposure, contrast)?.tone, 0.0, 0.0, 0.0);
    let stripped: Vec<_> = pairs.iter().filter_map(|(x, y)| Some((*x, without_chroma(*y, &tone)?))).collect();
    Some(NeutralFit { exposure, contrast, hue_sat: fit_hue_sat(&stripped, &Mat3::IDENTITY)? })
}

/// The exposure and contrast of the neutral tone that best follow a render's brightness: the median gamma-encoded
/// luminance of the render per quarter stop of scene luminance (bins of at least 25 pairs, 8 or more of them), compared
/// with the tone's at the bin's centre over a grid of exposures (-0.5 to 3 EV) and contrasts (1 to 2.6). That is how
/// the Canon values were first set, by hand, against 19 EOS R6 photos. `None` when the pairs do not span the tones.
fn fit_neutral_tone(pairs: &[([f64; 3], [f64; 3])]) -> Option<(f32, f32)> {
    let encoded = |v: f64| v.max(0.0).powf(1.0 / 2.2);
    let mut bins: std::collections::BTreeMap<i32, Vec<f64>> = Default::default();
    for (x, y) in pairs {
        let (lx, ly) = (luma(*x), luma(*y));
        if lx > 0.002 && lx < 2.0 && ly > 0.002 && ly < 0.97 {
            bins.entry((lx.log2() * 4.0).floor() as i32).or_default().push(encoded(ly));
        }
    }
    let points: Vec<(f32, f64)> = bins
        .into_iter()
        .filter(|(_, v)| v.len() >= 25)
        .filter_map(|(bin, mut v)| Some((2f32.powf((bin as f32 + 0.5) / 4.0), median(&mut v)?)))
        .collect();
    if points.len() < 8 {
        return None;
    }
    let cost = |e: f32, c: f32| -> f64 { points.iter().map(|&(x, m)| (encoded(f64::from(neutral_value(x, e, c))) - m).powi(2)).sum() };
    let (mut best, mut at) = (f64::INFINITY, (NEUTRAL_TONE.0, NEUTRAL_TONE.1));
    for ie in 0..=175 {
        for ic in 0..=80 {
            let (e, c) = (-0.5 + 0.02 * ie as f32, 1.0 + 0.02 * ic as f32);
            let k = cost(e, c);
            if k < best {
                (best, at) = (k, (e, c));
            }
        }
    }
    best.is_finite().then_some(at)
}

/// Sony's global tone curve with the Dynamic Range Optimizer off (Standard creative style), pooled
/// from 12 CC0 raw.pixls.us ARWs of 10 interchangeable-lens bodies and their camera JPEGs; the file
/// lists them and how the curve was derived (`assets/ATTRIBUTION.md`). Pooled the same way from 55
/// private ILCE-7CR photos with DRO off, the curve differs by at most 1.2 L* between scene luminance
/// 0.01 and 0.5. Issue #244, `docs/camera-preview-colour.md`.
const SONY_DRO_OFF_TONE: &str = include_str!("../../../../assets/camera-tone/sony-dro-off.json");

/// [`SONY_DRO_OFF_TONE`]'s curve (`None` only if the built-in file were invalid; a test checks it).
pub(super) fn sony_dro_off_tone() -> Option<CameraTone> {
    #[derive(serde::Deserialize)]
    struct File {
        knots: [[f32; 2]; 32],
    }
    static TONE: std::sync::OnceLock<Option<CameraTone>> = std::sync::OnceLock::new();
    *TONE.get_or_init(|| serde_json::from_str::<File>(SONY_DRO_OFF_TONE).ok().and_then(|f| CameraTone::new(f.knots)))
}

/// `look`, fitted to a camera JPEG brightened by Sony's Dynamic Range Optimizer, with its tone
/// curve lowered to Sony's without DRO ([`SONY_DRO_OFF_TONE`]) wherever that is darker. DRO only
/// brightens, so the fitted curve bounds the curve without it from above, everywhere: where Sony's
/// curve is brighter than the photo's own (the 1″ compacts map raw values darker), the photo keeps
/// its own. Its colour (matrix, hue/saturation table and chroma curve) stays as fitted.
pub(super) fn with_dro_off_tone(look: CameraLook) -> CameraLook {
    CameraLook { tone: dro_off(&look.tone), ..look }
}

/// The curve of [`with_dro_off_tone`]: `own` lowered to Sony's without DRO wherever that is darker,
/// or `own` unchanged when it can't be.
pub(super) fn dro_off(own_tone: &CameraTone) -> CameraTone {
    let Some(dro_off) = sony_dro_off_tone() else { return *own_tone };
    let own = own_tone.knots();
    let mut knots = own.map(|[x, y]| [x, y.min(dro_off.apply(x))]);
    // Past its last knot a curve goes on as a shoulder whose rate comes from its last two knots
    // (`CameraTone::apply`): lowering the second-last knot more than the last steepens it, and the
    // extended highlights would end up above the fitted curve. Raise the second-last knot (never
    // above the fitted curve) until the rate is at most the fitted curve's.
    let ([a, b], last) = ([own[30], own[31]], knots[31]);
    let rate = ((b[1] - a[1]) / (b[0] - a[0])).clamp(0.1, 16.0) / (1.0 - b[1]).max(0.01);
    let floor = last[1] - rate * (1.0 - last[1]).max(0.01) * (b[0] - a[0]);
    knots[30][1] = knots[30][1].max(floor).min(last[1]);
    let Some(tone) = CameraTone::new(knots).and_then(|tone| tone.with_chroma(*own_tone.chroma())) else { return *own_tone };
    if !at_most(&tone, own_tone) {
        return *own_tone;
    }
    tone
}

/// Whether curve `a` is nowhere brighter than `b` over the scene luminances the finish stage's tone
/// table covers ([`ToneMap::camera`]).
pub(super) fn at_most(a: &CameraTone, b: &CameraTone) -> bool {
    use lightcraft_pipeline::tone::{GREY, LUT_MAX_EV, LUT_MIN_EV, LUT_N};
    (0..LUT_N).all(|i| {
        let x = GREY * 2f32.powf(LUT_MIN_EV + (LUT_MAX_EV - LUT_MIN_EV) * i as f32 / (LUT_N - 1) as f32);
        a.apply(x) <= b.apply(x) + 1e-5
    })
}

/// Pixel cap for the embedded JPEG decoded for the fit. The codec checks it before decoding and
/// decodes at full size, so a 66.5 MP full-frame ILCE-7RM6 JPEG transiently needs ~200 MB (3 B/px);
/// 128 MP also admits 100 MP medium-format previews.
pub(super) const FIT_PREVIEW_MAX_PIXELS: u64 = 128_000_000;

/// A DNG's tone and chroma curves fitted to its own camera JPEG ([`dng_tone_eligible`]); the file's colour model
/// (matrix, profile tables, white balance) stays as it is: the proxy goes through the file's tables and the given
/// colour of the search is the identity. Same held-out acceptance gates as [`fit_look`]; the preview must not be a
/// thumbnail ([`MIN_PREVIEW_FRACTION`]), must show the same picture ([`structure_agrees`]), and the curve must not
/// clip more highlights than the plain render ([`clips_more`]).
pub(super) fn fit_dng_tone(
    raw: &RawImage,
    bytes: &[u8],
    transform: &CameraTransform,
    lens: Option<&lightcraft_develop::EmbeddedLens>,
) -> Option<CameraLook> {
    // Logged under LIGHTCRAFT_PROFILE so a corpus run can count the outcome per gate.
    let refuse = |why: &str| {
        if std::env::var_os("LIGHTCRAFT_PROFILE").is_some() {
            eprintln!("DNGTONE refused {why}");
        }
        None
    };
    let Some((mut sensor, reference, clipped)) = proxies(raw, bytes, transform, PROXY, MIN_PREVIEW_FRACTION, None) else { return refuse("preview") };
    // As the loader renders it: the profile's tables around the baseline exposure.
    let xy = lightcraft_raw::color::as_shot_white_xy(raw);
    if let Some(tables) = lightcraft_raw::profile::ProfileTables::new(&raw.color.profile, lightcraft_raw::color::illuminant_weight(&raw.color, xy)) {
        let gain = 2f32.powf(transform.baseline_exposure as f32);
        sensor.map_in_place(|p| tables.apply(p.map(|v| v / gain), gain));
    }
    correct_vignetting(&mut sensor, lens, raw.orientation);
    if (sensor.width, sensor.height) != (reference.width, reference.height) {
        return refuse("preview");
    }
    if !structure_agrees(&sensor, &reference) {
        return refuse("structure");
    }
    let unclipped = without_clipped(&sensor, &clipped);
    let fit_on = unclipped.as_ref().unwrap_or(&sensor);
    let colour = Some((Mat3::IDENTITY, None));
    let bright = HighlightCheck { pairs: highlights(&sensor, &reference), dro: false, grey: false };
    let look = TONE_FITS.iter().find_map(|&tone| {
        [false, true].into_iter().find_map(|away_from_edges| {
            fit_attempt(fit_on, &reference, &colour, Attempt { tone, profile: true, away_from_edges, dark: true }, &bright)
        })
    });
    let Some(look) = look else { return refuse("fit") };
    let plain = match raw.color.profile.tone_curve.as_ref().and_then(crate::files::dng_tone_curve) {
        Some(curve) => ToneMap::camera(&curve, 0.0, 0.0, 0.0),
        None => ToneMap::new(0.0, 0.0, 0.0),
    };
    let fitted = ToneMap::camera(&look.tone, 0.0, 0.0, 0.0);
    if clips_more(&sensor, &reference, &fitted, &plain) {
        return refuse("clip");
    }
    if std::env::var_os("LIGHTCRAFT_PROFILE").is_some() {
        eprintln!("DNGTONE accepted");
    }
    Some(look)
}

/// Whether the fitted tone map puts clearly more of the photo's proxy pixels at or near white than both the plain
/// render and the camera's own preview do ([`CLIP_TOLERANCE`]): a fit that clips what the camera clips too is the
/// camera's look, one that clips what neither does loses highlight detail.
pub(super) fn clips_more(sensor: &Rgb32f, reference: &Rgb32f, fitted: &ToneMap, plain: &ToneMap) -> bool {
    let near_white = |tone: &ToneMap| {
        let n = sensor.data.iter().filter(|p| p.iter().all(|v| v.is_finite()) && tone.apply(luminance_2020(p.map(|v| v.max(0.0)))) >= 0.98).count();
        n as f64 / sensor.data.len().max(1) as f64
    };
    let preview = reference.data.iter().filter(|p| luminance_2020(**p) >= 0.98).count() as f64 / reference.data.len().max(1) as f64;
    near_white(fitted) > near_white(plain).max(preview) + CLIP_TOLERANCE
}

/// Multiply the sensor proxy (sensor orientation, default crop) by the embedded vignetting correction, the gain the
/// pipeline applies to the render (`optics::embedded_vignette_gain`; `lens` is in the oriented, cropped frame): the
/// camera JPEG has it applied, so a tone fitted without it brightens the render's corners twice.
pub(super) fn correct_vignetting(sensor: &mut Rgb32f, lens: Option<&lightcraft_develop::EmbeddedLens>, orientation: lightcraft_geom::Orientation) {
    let Some(vignette) = lens.and_then(|l| l.vignette) else { return };
    let (w, h) = (sensor.width as f64, sensor.height as f64);
    let (ow, oh) = if orientation.swaps_axes() { (h, w) } else { (w, h) };
    let width = sensor.width.max(1);
    for (i, p) in sensor.data.iter_mut().enumerate() {
        let (x, y) = orientation.map((i % width) as f64 + 0.5, (i / width) as f64 + 0.5, w, h);
        let gain = lightcraft_pipeline::optics::embedded_vignette_gain(&vignette, lightcraft_geom::Point::new(x, y), ow, oh) as f32;
        *p = p.map(|v| v * gain);
    }
}
