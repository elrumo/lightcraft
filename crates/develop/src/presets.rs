//! Presets and copy/paste subsets.
//!
//! A preset is a *partial* settings object (JSON) — only the settings it names. Applying it changes
//! exactly the settings it names (a leaf: a number, a flag, a name; an array is one leaf): each is
//! computed from the photo's *original* state (its import settings, when known) and the preset's
//! value, and replaces the photo's value; every other setting keeps the photo's value (issue
//! #2159). `amount` (0..200 %) moves each named number between the original and the preset's
//! value. Copy/paste/sync use the same group selection ([`SettingsGroup`]) and merge into the
//! current settings as before.
//!
//! The rendering process ([`crate::ProcessVersion`]) is in no group: copy, paste, sync and presets
//! made from a photo never carry it, so the photos they change keep their own. A partial that names
//! `process` itself (Apply Settings JSON) sets it like any other field.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::borrow::Cow;

use crate::DevelopSettings;

/// Groups shown in "Choose settings to copy" / "Create preset" (Lightroom's checklist).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SettingsGroup {
    Profile,
    Treatment,
    WhiteBalance,
    Light,
    ToneCurve,
    Color,
    ColorMixer,
    ColorGrading,
    Effects,
    Vignette,
    Grain,
    Detail,
    Optics,
    Geometry,
    Crop,
    Masks,
    Spots,
    RedEye,
    LensBlur,
    Calibration,
    Relight,
    DepthMap,
    Film,
}

impl SettingsGroup {
    pub const ALL: [SettingsGroup; 23] = [
        SettingsGroup::Profile,
        SettingsGroup::Treatment,
        SettingsGroup::WhiteBalance,
        SettingsGroup::Light,
        SettingsGroup::ToneCurve,
        SettingsGroup::Color,
        SettingsGroup::ColorMixer,
        SettingsGroup::ColorGrading,
        SettingsGroup::Effects,
        SettingsGroup::Vignette,
        SettingsGroup::Grain,
        SettingsGroup::Detail,
        SettingsGroup::Optics,
        SettingsGroup::Geometry,
        SettingsGroup::Crop,
        SettingsGroup::Masks,
        SettingsGroup::Spots,
        SettingsGroup::RedEye,
        SettingsGroup::LensBlur,
        SettingsGroup::Calibration,
        SettingsGroup::Relight,
        SettingsGroup::DepthMap,
        SettingsGroup::Film,
    ];

    /// Groups included by default when copying (Lightroom excludes crop, masks and spots by default).
    pub fn default_copy() -> Vec<SettingsGroup> {
        SettingsGroup::ALL
            .into_iter()
            .filter(|g| !matches!(g, SettingsGroup::Crop | SettingsGroup::Masks | SettingsGroup::Spots | SettingsGroup::RedEye))
            .collect()
    }

    pub fn label(self) -> &'static str {
        match self {
            SettingsGroup::Profile => "Profile",
            SettingsGroup::Treatment => "Treatment",
            SettingsGroup::WhiteBalance => "White Balance",
            SettingsGroup::Light => "Light",
            SettingsGroup::ToneCurve => "Tone Curve",
            SettingsGroup::Color => "Color",
            SettingsGroup::ColorMixer => "Color Mixer",
            SettingsGroup::ColorGrading => "Color Grading",
            SettingsGroup::Effects => "Effects",
            SettingsGroup::Vignette => "Vignette",
            SettingsGroup::Grain => "Grain",
            SettingsGroup::Detail => "Detail",
            SettingsGroup::Optics => "Optics",
            SettingsGroup::Geometry => "Geometry",
            SettingsGroup::Crop => "Crop",
            SettingsGroup::Masks => "Masking",
            SettingsGroup::Spots => "Remove",
            SettingsGroup::RedEye => "Red Eye",
            SettingsGroup::LensBlur => "Lens Blur",
            SettingsGroup::Calibration => "Calibration",
            SettingsGroup::Relight => "Relight",
            SettingsGroup::DepthMap => "Depth Map",
            SettingsGroup::Film => "Film Effects",
        }
    }

    /// Top-level JSON keys of `DevelopSettings` that belong to the group.
    pub fn keys(self) -> &'static [&'static str] {
        match self {
            SettingsGroup::Profile => &["profile"],
            SettingsGroup::Treatment => &["treatment"],
            SettingsGroup::WhiteBalance => &["wb"],
            SettingsGroup::Light => &["light", "hdr"],
            SettingsGroup::ToneCurve => &["curve"],
            SettingsGroup::Color => &["color"],
            SettingsGroup::ColorMixer => &["mixer", "bw_mix", "point_colors"],
            SettingsGroup::ColorGrading => &["grading"],
            // (Glow sits in the Effects panel, as Lightroom has it)
            SettingsGroup::Effects => &["effects", "glow"],
            SettingsGroup::Vignette => &["vignette"],
            SettingsGroup::Grain => &["grain"],
            SettingsGroup::Detail => &["detail", "enhance"],
            SettingsGroup::Optics => &["optics"],
            SettingsGroup::Geometry => &["geometry"],
            SettingsGroup::Crop => &["crop", "orientation"],
            SettingsGroup::Masks => &["masks"],
            SettingsGroup::Spots => &["spots"],
            SettingsGroup::RedEye => &["red_eye"],
            SettingsGroup::LensBlur => &["lens_blur"],
            SettingsGroup::Calibration => &["calibration"],
            SettingsGroup::Relight => &["relight"],
            SettingsGroup::DepthMap => &["depth_map"],
            SettingsGroup::Film => &["film"],
        }
    }
}

/// The subset of `s` covering `groups`, as a partial JSON object. AI Remove fills and Lens Blur's
/// estimated depth are pictures of this photo, so copies leave them behind (on another photo a
/// spot fills from around it, and Lens Blur uses that photo's depth), and spots that are the shape
/// of an object here (Tap to Remove) are not copied at all.
pub fn extract_groups(s: &DevelopSettings, groups: &[SettingsGroup]) -> Value {
    let full = s.to_json_for_merge();
    let mut out = Map::new();
    for g in groups {
        for k in g.keys() {
            if let Some(v) = full.get(*k) {
                out.insert((*k).to_string(), v.clone());
            }
        }
    }
    // (no film effects is a choice too: a copy or preset that has the group clears the film of the photo it lands on, which
    // a group left out of the JSON while it is the default would not)
    if groups.contains(&SettingsGroup::Film)
        && !out.contains_key("film")
        && let Ok(v) = serde_json::to_value(crate::settings::Film::default())
    {
        out.insert("film".into(), v);
    }
    // (and film's grain, left out while it is the style, replaces the noise of the photo it lands on)
    if let Some(grain) = out.get_mut("grain").and_then(Value::as_object_mut) {
        grain.entry("style").or_insert(Value::from("film"));
    }
    // (and the Color group's Density, left out while 0, takes the Density of the photo it lands on back to 0)
    if let Some(color) = out.get_mut("color").and_then(Value::as_object_mut) {
        color.entry("density").or_insert(Value::from(0.0));
    }
    // (and the HDR group's SDR to HDR, left out while 0, takes that of the photo it lands on back to 0)
    if let Some(hdr) = out.get_mut("hdr").and_then(Value::as_object_mut) {
        hdr.entry("expand").or_insert(Value::from(0.0));
    }
    if let Some(Value::Array(spots)) = out.get_mut("spots") {
        // (a Tap to Remove spot is the shape of an object in this photo: nowhere else it means anything)
        spots.retain(|spot| spot.get("shape").is_none());
        for spot in spots.iter_mut().filter_map(Value::as_object_mut) {
            spot.remove("fill");
        }
    }
    // (a pasted Lens Blur keeps the depth of the photo it lands on)
    if let Some(blur) = out.get_mut("lens_blur").and_then(Value::as_object_mut) {
        blur.remove("depth");
    }
    // (and a pasted Relight the surface of the photo it lands on)
    if let Some(relight) = out.get_mut("relight").and_then(Value::as_object_mut) {
        relight.remove("normals");
    }
    // (an Object or Describe selection's picture is of the photo it was made on: the clicks and the words stay, the
    // other photo's selection is made again from them, see [`ai_work`])
    if let Some(Value::Array(masks)) = out.get_mut("masks") {
        for shape in
            masks.iter_mut().filter_map(|m| m.get_mut("components")?.as_array_mut()).flatten().filter_map(|c| c.get_mut("shape")?.as_object_mut())
        {
            if matches!(shape.get("kind").and_then(Value::as_str), Some("object" | "prompt")) {
                shape.remove("seg");
                shape.remove("detail");
            }
        }
    }
    Value::Object(out)
}

/// What a copy of some settings leaves to be made again on the photo it is pasted onto: the pictures of the first photo
/// that [`extract_groups`] drops. A model's work is not carried from one photo to another, it is done again.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AiWork {
    /// Object and Describe selections: `(mask id, component index)`.
    pub masks: Vec<(u32, usize)>,
    /// Remove spots an AI model filled, as indexes into the copied spots (the ones [`extract_groups`] keeps).
    pub spots: Vec<usize>,
    /// Generative Expand painted the area past the edges.
    pub expand: bool,
}

impl AiWork {
    pub fn is_empty(&self) -> bool {
        self.masks.is_empty() && self.spots.is_empty() && !self.expand
    }

    /// What of this is made again when only `groups` are pasted.
    pub fn only(&self, groups: &[SettingsGroup]) -> AiWork {
        AiWork {
            masks: if groups.contains(&SettingsGroup::Masks) { self.masks.clone() } else { Vec::new() },
            spots: if groups.contains(&SettingsGroup::Spots) { self.spots.clone() } else { Vec::new() },
            expand: self.expand && groups.contains(&SettingsGroup::Crop),
        }
    }
}

/// The model work in `s` that a copy of `groups` leaves behind (see [`AiWork`]).
pub fn ai_work(s: &DevelopSettings, groups: &[SettingsGroup]) -> AiWork {
    let mut work = AiWork::default();
    if groups.contains(&SettingsGroup::Masks) {
        for m in &s.masks {
            for (k, c) in m.components.iter().enumerate() {
                let done = match &c.shape {
                    crate::MaskShape::Object { seg, detail, .. } | crate::MaskShape::Prompt { seg, detail, .. } => {
                        seg.is_some() || !detail.is_empty()
                    }
                    _ => false,
                };
                if done {
                    work.masks.push((m.id, k));
                }
            }
        }
    }
    if groups.contains(&SettingsGroup::Spots) {
        // (as `extract_groups` keeps them: the spots that are not an object's shape)
        work.spots = s.spots.iter().filter(|sp| sp.shape.is_none()).enumerate().filter(|(_, sp)| sp.fill.is_some()).map(|(i, _)| i).collect();
    }
    work.expand = groups.contains(&SettingsGroup::Crop) && s.crop.expand && s.expand_fill.is_some();
    work
}

/// Recursively merge `patch` into `base` (objects merge; everything else replaces).
pub fn deep_merge(base: &mut Value, patch: &Value) {
    match (base, patch) {
        (Value::Object(b), Value::Object(p)) => {
            for (k, v) in p {
                match b.get_mut(k) {
                    Some(bv) if bv.is_object() && v.is_object() => deep_merge(bv, v),
                    _ => {
                        b.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (b, p) => *b = p.clone(),
    }
}

/// Interpolate numbers in `patch` towards/away from `base` by `t` (1 = patch as-is); non-numeric
/// values are taken from the patch when `t >= 0.5`.
fn scale_patch(base: &Value, patch: &Value, t: f64) -> Value {
    match (base, patch) {
        (Value::Object(b), Value::Object(p)) => {
            let mut out = Map::new();
            for (k, pv) in p {
                let bv = b.get(k).unwrap_or(&Value::Null);
                let v = if k == "profile" { scale_profile(bv, pv, t) } else { None };
                out.insert(k.clone(), v.unwrap_or_else(|| scale_patch(bv, pv, t)));
            }
            Value::Object(out)
        }
        // Integer fields (the grain seed, ids) are discrete like strings: an interpolated seed would
        // not deserialize as an integer, and the whole preset would silently not apply.
        (Value::Number(b), Value::Number(p)) if b.is_f64() || p.is_f64() => {
            let (b, p) = (b.as_f64().unwrap_or(0.0), p.as_f64().unwrap_or(0.0));
            serde_json::Number::from_f64(b + (p - b) * t).map(Value::Number).unwrap_or(Value::Null)
        }
        (b, p) => {
            if t >= 0.5 {
                p.clone()
            } else {
                b.clone()
            }
        }
    }
}

/// The profile at `t` when the preset's (`patch`) is another than the photo's (`base`) and one of the two is no look
/// (Color, or an amount of 0): the look's own amount runs from 0, so the Amount slider blends it in gradually instead
/// of switching profiles at 50 % (HB-445). `None`: the plain interpolation (the same profile: its amount; two looks:
/// the id switches halfway).
fn scale_profile(base: &Value, patch: &Value, t: f64) -> Option<Value> {
    let id = |v: &Value| v.get("id").and_then(Value::as_str).map(str::to_owned);
    let amount = |v: &Value| v.get("amount").and_then(Value::as_f64).unwrap_or(100.0);
    let plain = |v: &Value| matches!(id(v).as_deref(), None | Some("" | "lc.color")) || amount(v) == 0.0;
    let (bid, pid) = (id(base), id(patch));
    if pid.is_none() || bid == pid || plain(base) == plain(patch) {
        return None;
    }
    // (the look that is there: the preset's coming in, else the photo's going out)
    let (look, k) = if plain(base) {
        (patch, t)
    } else if t < 1.0 {
        (base, 1.0 - t)
    } else {
        return None;
    };
    let mut out = scale_patch(base, patch, t);
    if let Some(o) = out.as_object_mut() {
        o.insert("id".into(), look.get("id").cloned().unwrap_or(Value::Null));
        o.insert("amount".into(), serde_json::Number::from_f64(amount(look) * k).map_or(Value::Null, Value::Number));
    }
    Some(out)
}

/// Apply a partial settings object with an amount (1.0 = 100 %). Out-of-range values are clamped
/// through the control specs, as every merge does ([`DevelopSettings::merged`]).
pub fn apply_partial(s: &DevelopSettings, partial: &Value, amount: f64) -> DevelopSettings {
    let patch = if (amount - 1.0).abs() < 1e-9 { Cow::Borrowed(partial) } else { Cow::Owned(scale_patch(&s.to_json_full(), partial, amount)) };
    // (a white balance without a scale keeps its old meaning: `DevelopSettings::merged`)
    s.merged(&patch).unwrap_or_else(|_| s.clone())
}

/// Top-level settings keys that hold per-photo pixel repairs: a preset's named repairs compose on
/// the photo's own values instead of the original's (masks are added by [`Preset::apply_on`]), and
/// a preset without them leaves the photo's alone.
const REPAIR_KEYS: [&str; 3] = ["masks", "spots", "red_eye"];

/// The settings `settings` (a partial JSON object) names, applied to `current` at `amount`: each
/// named setting is computed from `original` (the photo's import state) and the preset's value,
/// and replaces the photo's value (issue #2159). Every setting the preset does not name keeps the
/// photo's current value, edits included. Pixel repairs compose on the photo's own values, as they
/// always have.
fn compose_named(current: &DevelopSettings, original: &DevelopSettings, settings: &Value, amount: f64) -> DevelopSettings {
    // At 100 % the preset's own values apply as they are, and `original` is not read. The presets
    // panel calls this for every visible row on every frame, so the scaling base is built below only.
    if (amount - 1.0).abs() < 1e-9 {
        return current.merged(settings).unwrap_or_else(|_| current.clone());
    }
    let cur = current.to_json();
    // every leaf: a group left out while it is the default (Glow, Film...) still scales from its value
    let mut base = original.to_json_full();
    if let (Some(b), Some(c)) = (base.as_object_mut(), cur.as_object()) {
        for k in REPAIR_KEYS {
            if let Some(v) = c.get(k) {
                b.insert(k.to_string(), v.clone());
            }
        }
    }
    let composed = match settings.as_object() {
        Some(patch) => Value::Object(interpolate_object(base.as_object(), patch, amount)),
        None => Value::Object(Map::new()),
    };
    // `merged` deep-merges: the named settings replace, everything else stays as `current` has it
    current.merged(&composed).unwrap_or_else(|_| current.clone())
}

/// The leaves of `patch` that apply at `t` (below 100 %), each computed from the same place in
/// `base`. A leaf the amount does not reach is left out, so the photo keeps its own value there.
fn interpolate_object(base: Option<&Map<String, Value>>, patch: &Map<String, Value>, t: f64) -> Map<String, Value> {
    let mut out = Map::new();
    for (k, pv) in patch {
        let bv = base.and_then(|b| b.get(k));
        let v = match pv {
            Value::Object(p) => Some(Value::Object(interpolate_object(bv.and_then(Value::as_object), p, t))),
            _ => interpolate_leaf(bv, pv, t),
        };
        if let Some(v) = v {
            out.insert(k.clone(), v);
        }
    }
    out
}

/// One leaf at `t`. Numbers move from `base` towards `patch`. Other values switch to the preset's
/// at half amount and up, and below it take the base's value. A null in the base is the baseline
/// value the import state recorded (a cleared option, such as a free crop aspect): below half
/// amount it applies and clears the photo's value, as the amount contract promises. A null the
/// preset writes is a value like any other, so it applies at half amount and up (a preset can
/// clear an option field such as a crop aspect).
fn interpolate_leaf(base: Option<&Value>, patch: &Value, t: f64) -> Option<Value> {
    match (base, patch) {
        // integer fields (the grain seed, ids) are discrete like strings: see `scale_patch`
        (Some(Value::Number(b)), Value::Number(p)) if b.is_f64() || p.is_f64() => {
            let (b, p) = (b.as_f64().unwrap_or(0.0), p.as_f64().unwrap_or(0.0));
            serde_json::Number::from_f64(b + (p - b) * t).map(Value::Number)
        }
        _ if t >= 0.5 => Some(patch.clone()),
        _ => base.cloned(),
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    pub id: String,
    pub name: String,
    pub group: String,
    /// Partial settings.
    pub settings: Value,
    #[serde(default)]
    pub favorite: bool,
    /// Built-in presets ship with the app and can't be deleted.
    #[serde(default)]
    pub builtin: bool,
}

impl Preset {
    pub fn from_settings(id: &str, name: &str, group: &str, s: &DevelopSettings, groups: &[SettingsGroup]) -> Preset {
        Preset { id: id.into(), name: name.into(), group: group.into(), settings: extract_groups(s, groups), favorite: false, builtin: false }
    }
    /// Apply at `amount` (1.0 = 100 %), with `s` as both the photo's current and its original
    /// settings (see [`Preset::apply_on`]): right for the import default preset and for scratch
    /// bases. Masks in the preset are added to the photo's own (ones it already has are skipped),
    /// their effect scaled by the amount.
    pub fn apply(&self, s: &DevelopSettings, amount: f64) -> DevelopSettings {
        self.apply_on(s, s, amount)
    }
    /// Apply at `amount` (1.0 = 100 %), composing on `original` (the photo's import state, or the
    /// photo's current settings when that is not known): each setting the preset names is computed
    /// from `original` and the preset's value, `amount` moving it between the two, and replaces the
    /// photo's value. Every setting the preset does not name keeps `current`'s value, and so do
    /// the photo's pixel repairs (masks, spots, red eye), which the preset patches; masks in the
    /// preset are added to the photo's own (ones it already has are skipped), their effect scaled
    /// by the amount. A second preset therefore restores what the first one changed in the
    /// settings it names (issue #2159), and nothing it leaves out.
    pub fn apply_on(&self, current: &DevelopSettings, original: &DevelopSettings, amount: f64) -> DevelopSettings {
        let Some(masks) = self.settings.get("masks").and_then(Value::as_array).filter(|m| !m.is_empty()) else {
            return compose_named(current, original, &self.settings, amount);
        };
        let mut rest = self.settings.clone();
        if let Some(o) = rest.as_object_mut() {
            o.remove("masks");
        }
        let mut out = compose_named(current, original, &rest, amount);
        add_masks(&mut out, masks, amount);
        out
    }
}

/// The preset's masks, added to the photo's own (ones it already has are skipped), their effect
/// scaled by the amount.
fn add_masks(out: &mut DevelopSettings, masks: &[Value], amount: f64) {
    // the masks by name, kept up to date: a preset of many masks compares each with its
    // namesakes only, not with every mask added before it
    let mut by_name: std::collections::HashMap<String, Vec<usize>> = std::collections::HashMap::new();
    for (i, q) in out.masks.iter().enumerate() {
        by_name.entry(q.name.clone()).or_default().push(i);
    }
    let mut next_id = out.next_mask_id();
    let mut used: std::collections::HashSet<u32> = out.masks.iter().map(|q| q.id).collect();
    for m in masks {
        let Ok(mut m) = serde_json::from_value::<crate::Mask>(m.clone()) else { continue };
        if by_name.get(&m.name).is_some_and(|ix| ix.iter().any(|&i| out.masks.get(i).is_some_and(|q| q.components == m.components))) {
            continue;
        }
        // ids run on from the first free one, past any a mask already has (a scan for the
        // next free id per mask would be another pass over them)
        while used.contains(&next_id) {
            next_id = next_id.checked_add(1).unwrap_or(1);
        }
        m.id = next_id;
        used.insert(next_id);
        m.adjust.amount = (m.adjust.amount * amount).clamp(0.0, 200.0);
        by_name.entry(m.name.clone()).or_default().push(out.masks.len());
        out.masks.push(m);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extract_and_apply_roundtrip() {
        let mut a = DevelopSettings::default();
        a.light.exposure = 1.0;
        a.effects.clarity = 30.0;
        a.crop.geometry.angle = 5.0;
        let p = Preset::from_settings("p", "P", "User", &a, &SettingsGroup::default_copy());
        assert!(p.settings.get("crop").is_none());
        let b = p.apply(&DevelopSettings::default(), 1.0);
        assert_eq!(b.light.exposure, 1.0);
        assert_eq!(b.effects.clarity, 30.0);
        assert_eq!(b.crop.geometry.angle, 0.0);
    }

    /// Object and Describe selections are pictures of their photo: a copy keeps the clicks and the words and finds them
    /// again with [`ai_work`], and nothing else is asked to be made again.
    #[test]
    fn copied_selections_leave_their_pictures_and_are_found_again() {
        let seg = Some(crate::SegMask::from_logits(4, &[3.0; 16]));
        let comp = |shape| crate::MaskComponent { name: None, op: Default::default(), invert: false, shape };
        let mut a = DevelopSettings::default();
        a.masks.push(crate::Mask {
            id: 7,
            components: vec![
                comp(crate::MaskShape::Sky),
                comp(crate::MaskShape::Object {
                    hint: vec![lightcraft_geom::Point::new(0.5, 0.5)],
                    exclude: vec![],
                    seg: seg.clone(),
                    detail: vec![],
                    edge: 0.0,
                }),
                comp(crate::MaskShape::Prompt { text: "the red car".into(), seg, detail: vec![], edge: 0.0 }),
            ],
            ..Default::default()
        });
        let fill = crate::SpotFill::from_linear(2, 2, [0.4, 0.4, 0.6, 0.6], &[[0.5; 3]; 4]);
        let at = lightcraft_geom::Point::new(0.5, 0.5);
        a.spots.push(crate::Spot { points: vec![at], ..Default::default() });
        a.spots.push(crate::Spot { points: vec![at], fill, ..Default::default() });
        let groups = [SettingsGroup::Masks, SettingsGroup::Spots];
        let v = extract_groups(&a, &groups);
        let parts = &v["masks"][0]["components"];
        for k in [1, 2] {
            assert!(parts[k]["shape"].get("seg").is_none(), "{v}");
        }
        assert_eq!(parts[2]["shape"]["text"], "the red car", "the words stay");
        assert_eq!(parts[1]["shape"]["hint"].as_array().map(Vec::len), Some(1), "the clicks stay");
        let work = ai_work(&a, &groups);
        assert_eq!((work.masks.clone(), work.spots.clone(), work.expand), (vec![(7, 1), (7, 2)], vec![1], false));
        assert!(ai_work(&a, &[SettingsGroup::Light]).is_empty());
        assert!(work.only(&[SettingsGroup::Spots]).masks.is_empty());
        assert!(DevelopSettings::default().masks.is_empty() && ai_work(&DevelopSettings::default(), &groups).is_empty());
    }

    #[test]
    fn copied_spots_leave_their_ai_fills_behind() {
        let mut a = DevelopSettings::default();
        let fill = crate::SpotFill::from_linear(2, 2, [0.4, 0.4, 0.6, 0.6], &[[0.5; 3]; 4]);
        a.spots.push(crate::Spot { points: vec![lightcraft_geom::Point::new(0.5, 0.5)], fill, ..Default::default() });
        let v = extract_groups(&a, &[SettingsGroup::Spots]);
        assert!(v["spots"][0].get("fill").is_none(), "{v}");
        let b = apply_partial(&DevelopSettings::default(), &v, 1.0);
        assert_eq!(b.spots.len(), 1);
        assert_eq!((b.spots[0].points.clone(), b.spots[0].fill.clone()), (a.spots[0].points.clone(), None));
    }

    #[test]
    fn copied_relight_leaves_its_surface_behind() {
        let mut a = DevelopSettings::default();
        a.relight.enabled = true;
        a.relight.brightness = 80.0;
        a.relight.normals = crate::NormalMap::from_normals(1, 1, &[[0.0, 0.0, 1.0]]);
        let v = extract_groups(&a, &[SettingsGroup::Relight]);
        assert!(v["relight"].get("normals").is_none(), "{v}");
        assert_eq!(v["relight"]["brightness"], 80.0);
        let mut b = DevelopSettings::default();
        b.relight.normals = crate::NormalMap::from_normals(1, 1, &[[0.6, 0.0, 0.8]]);
        let c = crate::apply_partial(&b, &v, 1.0);
        assert_eq!((c.relight.enabled, c.relight.brightness, c.relight.normals.clone()), (true, 80.0, b.relight.normals.clone()));
    }

    #[test]
    fn copied_lens_blur_leaves_its_depth_behind() {
        let mut a = DevelopSettings { lens_blur: crate::LensBlur::applied(30.0), ..Default::default() };
        a.lens_blur.depth = crate::DepthMap::from_nearness(2, 1, &[0.2, 0.8]);
        let v = extract_groups(&a, &[SettingsGroup::LensBlur]);
        assert!(v["lens_blur"].get("depth").is_none(), "{v}");
        // the photo it lands on keeps its own depth
        let mut b = DevelopSettings::default();
        b.lens_blur.depth = crate::DepthMap::from_nearness(1, 1, &[0.5]);
        let c = apply_partial(&b, &v, 1.0);
        assert_eq!((c.lens_blur.amount, c.lens_blur.focal_distance, c.lens_blur.depth.clone()), (50.0, 30.0, b.lens_blur.depth.clone()));
    }

    #[test]
    fn amount_scales_numbers() {
        let base = DevelopSettings::default();
        let partial = json!({"light": {"exposure": 1.0, "contrast": 40}});
        let half = apply_partial(&base, &partial, 0.5);
        assert!((half.light.exposure - 0.5).abs() < 1e-9);
        assert!((half.light.contrast - 20.0).abs() < 1e-9);
        let more = apply_partial(&base, &partial, 2.0);
        assert!((more.light.contrast - 80.0).abs() < 1e-9);
        // clamped through specs
        let lots = apply_partial(&base, &json!({"light": {"exposure": 4.0}}), 2.0);
        assert_eq!(lots.light.exposure, 5.0);
    }

    /// A look preset (a profile and grain, as the film packs are) at part of its amount used to switch the profile at
    /// 50 %: the Amount slider made the look all or nothing (HB-445). The look's own amount runs from 0 instead.
    #[test]
    fn amount_blends_a_profile_in_and_out() {
        let look = json!({"profile": {"id": "lut:pack-film-vivid", "amount": 100.0}, "grain": {"amount": 20.0}});
        let base = DevelopSettings::default();
        for t in [0.0, 0.3, 0.47, 0.5, 1.0, 1.5] {
            let out = apply_partial(&base, &look, t);
            assert_eq!(out.profile.id, "lut:pack-film-vivid", "{t}");
            assert!((out.profile.amount - 100.0 * t).abs() < 1e-9, "{t}: {}", out.profile.amount);
        }
        // a preset that sets Color takes the photo's look out gradually too
        let mut vivid = DevelopSettings::default();
        vivid.profile.id = "lc.vivid".into();
        vivid.profile.amount = 80.0;
        let plain = json!({"profile": {"id": "lc.color", "amount": 100.0}});
        let quarter = apply_partial(&vivid, &plain, 0.25);
        assert_eq!((quarter.profile.id.as_str(), quarter.profile.amount), ("lc.vivid", 60.0));
        assert_eq!(apply_partial(&vivid, &plain, 1.5).profile.id, "lc.color");
        // two looks: no blend between them, the id switches halfway as before
        assert_eq!(apply_partial(&vivid, &look, 0.4).profile.id, "lc.vivid");
        assert_eq!(apply_partial(&vivid, &look, 0.6).profile.id, "lut:pack-film-vivid");
    }

    /// A preset with the Grain group (its seed is an integer) used to do nothing at any amount
    /// other than 100 %: the interpolated seed failed to deserialize and the preset was dropped.
    #[test]
    fn amount_keeps_integer_fields_whole() {
        let mut a = DevelopSettings::default();
        a.light.exposure = 1.0;
        a.grain.amount = 40.0;
        a.grain.seed = 12345;
        let p = Preset::from_settings("p", "P", "User", &a, &SettingsGroup::default_copy());
        assert!(p.settings.pointer("/grain/seed").is_some_and(Value::is_u64), "the default copy groups include the grain seed");
        let base = DevelopSettings::default();
        for (amount, seed) in [(0.3, 0), (0.5, 12345), (1.5, 12345), (2.0, 12345)] {
            let out = p.apply(&base, amount);
            assert!((out.light.exposure - amount.min(5.0)).abs() < 1e-9, "{amount}: exposure {}", out.light.exposure);
            assert!((out.grain.amount - (40.0 * amount).min(100.0)).abs() < 1e-9, "{amount}: grain {}", out.grain.amount);
            assert_eq!(out.grain.seed, seed, "{amount}: seed");
        }
    }

    /// A group left out of the JSON while it is the default (Glow, Film…) used to make a preset that has it do nothing
    /// at any amount but 100 %: the photo had no value to scale from, the group became null and the merge failed.
    #[test]
    fn amount_scales_groups_the_photo_leaves_out() {
        let mut a = DevelopSettings::default();
        a.glow.amount = 60.0;
        a.light.exposure = 1.0;
        let p = Preset::from_settings("p", "P", "User", &a, &SettingsGroup::default_copy());
        let out = p.apply(&DevelopSettings::default(), 0.5);
        assert!((out.glow.amount - 30.0).abs() < 1e-9, "glow {}", out.glow.amount);
        assert!((out.light.exposure - 0.5).abs() < 1e-9);
        assert!(out.film.is_default());
    }

    /// The grain's styles as a frontend sends them through `develop.merge`; film's is left out of the JSON (settings
    /// from before the styles keep their hash) but written in a copy, so pasting film grain replaces digital noise.
    #[test]
    fn grain_style_wire_names_and_copies() {
        use crate::GrainStyle::*;
        for (id, style) in [("film", Film), ("digital", Digital), ("color", Color)] {
            let s: DevelopSettings = serde_json::from_value(json!({"grain": {"style": id}})).unwrap();
            assert_eq!(s.grain.style, style);
            assert_eq!(serde_json::to_value(s.grain.style).unwrap(), json!(id));
        }
        let mut film = DevelopSettings::default();
        assert!(film.to_json().pointer("/grain/style").is_none());
        // (the process is stated: JSON without one is read as an old photo's)
        assert_eq!(film.hash64(), DevelopSettings::from_json(&json!({"process": film.process})).unwrap().hash64());
        film.grain.amount = 30.0;
        let mut noisy = film.clone();
        noisy.grain.style = Digital;
        assert_ne!(noisy.hash64(), film.hash64());
        assert_eq!(DevelopSettings::from_json(&noisy.to_json()).unwrap(), noisy);
        let copy = Preset::from_settings("p", "P", "User", &film, &[SettingsGroup::Grain]);
        assert_eq!(copy.apply(&noisy, 1.0).grain.style, Film);
        assert_eq!(copy.apply(&noisy, 0.3).grain.style, Digital, "under half, a discrete value stays the photo's");
        assert_eq!(film.to_json_full().pointer("/grain/style"), Some(&json!("film")));
        // (and noise at a part of its amount lands on a photo whose JSON has no style: it used to make the style null, and
        // the whole preset did nothing)
        let noise = Preset::from_settings("n", "N", "User", &noisy, &[SettingsGroup::Grain]);
        let part = noise.apply(&DevelopSettings::default(), 0.3);
        assert!((part.grain.amount - 9.0).abs() < 1e-9 && part.grain.style == Film, "{:?}", part.grain);
        assert_eq!(noise.apply(&DevelopSettings::default(), 0.7).grain.style, Digital);
    }

    /// A preset changes only the settings it names: a setting in a group it styles but does not
    /// name keeps the photo's value, and so does a whole group it leaves out.
    #[test]
    fn apply_on_changes_only_the_settings_it_names() {
        let mut current = DevelopSettings::default();
        current.light.exposure = 2.0;
        current.light.blacks = -30.0;
        current.effects.clarity = 30.0;
        let original = DevelopSettings::default();
        let p = Preset {
            id: "p".into(),
            name: "P".into(),
            group: "G".into(),
            settings: json!({"light": {"contrast": 12.0}}),
            favorite: false,
            builtin: false,
        };
        let out = p.apply_on(&current, &original, 1.0);
        assert_eq!(out.light.contrast, 12.0);
        assert_eq!(out.light.exposure, 2.0, "a setting the preset doesn't name keeps the photo's value");
        assert_eq!(out.light.blacks, -30.0, "a setting in the same group, not named, keeps the photo's value");
        assert_eq!(out.effects.clarity, 30.0, "a group the preset doesn't name is untouched");
    }

    /// A partial preset that names one colour of the mixer leaves the other colours, and the
    /// unnamed settings of the colour it names, as the photo has them.
    #[test]
    fn apply_on_keeps_the_mixer_colours_it_does_not_name() {
        let mut current = DevelopSettings::default();
        current.mixer.red.sat = 20.0;
        current.mixer.blue.hue = 15.0;
        current.mixer.blue.sat = 5.0;
        let original = DevelopSettings::default();
        let p = Preset {
            id: "p".into(),
            name: "P".into(),
            group: "G".into(),
            settings: json!({"mixer": {"blue": {"sat": 15.0, "lum": -25.0}}}),
            favorite: false,
            builtin: false,
        };
        let out = p.apply_on(&current, &original, 1.0);
        assert_eq!(out.mixer.red.sat, 20.0, "a colour the preset doesn't name");
        assert_eq!(out.mixer.blue.hue, 15.0, "a setting of a named colour that the preset doesn't name");
        assert_eq!((out.mixer.blue.sat, out.mixer.blue.lum), (15.0, -25.0));
    }

    /// The white balance is not one setting: a preset that names only the temperature keeps the
    /// photo's tint.
    #[test]
    fn apply_on_keeps_the_white_balance_fields_it_does_not_name() {
        let mut current = DevelopSettings::default();
        current.wb.temp = 6000.0;
        current.wb.tint = 12.0;
        let original = DevelopSettings::default();
        let p = Preset {
            id: "p".into(),
            name: "P".into(),
            group: "G".into(),
            settings: json!({"wb": {"temp": 5000.0}}),
            favorite: false,
            builtin: false,
        };
        let out = p.apply_on(&current, &original, 1.0);
        assert_eq!((out.wb.temp, out.wb.tint), (5000.0, 12.0));
    }

    /// A preset's amount 0 changes nothing when the original is the photo's current settings (no
    /// import state is recorded for the photo), and any other amount moves the named settings
    /// from where the photo has them.
    #[test]
    fn apply_on_without_an_import_state_scales_from_the_current_settings() {
        let mut current = DevelopSettings::default();
        current.light.exposure = 2.0;
        current.light.blacks = -30.0;
        let p = Preset {
            id: "p".into(),
            name: "P".into(),
            group: "G".into(),
            settings: json!({"light": {"exposure": 0.45, "contrast": 12.0}}),
            favorite: false,
            builtin: false,
        };
        assert_eq!(p.apply_on(&current, &current, 0.0), current, "amount 0 applies nothing");
        let half = p.apply_on(&current, &current, 0.5);
        assert!((half.light.exposure - 1.225).abs() < 1e-9, "{}", half.light.exposure);
        assert_eq!(half.light.blacks, -30.0);
    }

    /// An unresolved `crs:Look` reference (its profile object has no id; the importer leaves
    /// `look` in it) changes nothing: the photo keeps its own profile.
    #[test]
    fn an_unresolved_profile_reference_keeps_the_photos_profile() {
        let mut current = DevelopSettings::default();
        current.profile.id = "lc.vivid".into();
        current.profile.amount = 60.0;
        let p = Preset {
            id: "p".into(),
            name: "P".into(),
            group: "G".into(),
            settings: json!({"profile": {"look": {"name": "Not imported", "amount": 100.0}}, "light": {"contrast": 12.0}}),
            favorite: false,
            builtin: false,
        };
        for amount in [1.0, 0.5] {
            let out = p.apply_on(&current, &current, amount);
            assert_eq!((out.profile.id.as_str(), out.profile.amount), ("lc.vivid", 60.0), "amount {amount}");
        }
    }

    /// The amount interpolates between the original and the preset, not between the visible
    /// values and the preset: a preset at 50 % looks the same however the photo was edited before.
    #[test]
    fn apply_on_scales_against_the_original() {
        let mut current = DevelopSettings::default();
        current.light.exposure = 2.0;
        let original = DevelopSettings::default();
        let p = Preset {
            id: "p".into(),
            name: "P".into(),
            group: "G".into(),
            settings: json!({"light": {"exposure": 0.45}}),
            favorite: false,
            builtin: false,
        };
        let out = p.apply_on(&current, &original, 0.5);
        assert!((out.light.exposure - 0.225).abs() < 1e-9, "{}", out.light.exposure);
    }

    /// Pixel repairs are the photo's own: a preset's are patched in, and one without them never
    /// restores the original's.
    #[test]
    fn apply_on_patches_pixel_repairs_into_the_current_ones() {
        let current = DevelopSettings {
            spots: serde_json::from_value(json!([{"mode": "heal", "points": [[0.5, 0.5]], "size": 0.05, "feather": 50.0, "opacity": 100.0}]))
                .unwrap(),
            ..Default::default()
        };
        let original = DevelopSettings::default();
        let none = Preset {
            id: "p".into(),
            name: "P".into(),
            group: "G".into(),
            settings: json!({"light": {"contrast": 10.0}}),
            favorite: false,
            builtin: false,
        };
        let out = none.apply_on(&current, &original, 1.0);
        assert_eq!(out.spots.len(), 1, "the photo's spot survives a preset without spots");
        let clearing = Preset {
            id: "q".into(),
            name: "Q".into(),
            group: "G".into(),
            settings: json!({"light": {"contrast": 10.0}, "spots": []}),
            favorite: false,
            builtin: false,
        };
        let out = clearing.apply_on(&current, &original, 1.0);
        assert_eq!(out.spots.len(), 0, "a preset that names spots replaces them, as before");
    }

    /// A null a preset writes into an option field (here a crop aspect: null is "free") is the
    /// preset's value, so it clears the photo's aspect from half amount up, as any value of the
    /// preset does; below half amount the import baseline applies, which here is null as well.
    #[test]
    fn a_preset_null_clears_an_option_field_from_half_amount() {
        let mut current = DevelopSettings::default();
        current.crop.aspect = Some((3, 2));
        let original = DevelopSettings::default(); // the import state has no aspect
        let p = Preset {
            id: "p".into(),
            name: "P".into(),
            group: "G".into(),
            settings: json!({"crop": {"aspect": null}}),
            favorite: false,
            builtin: false,
        };
        for amount in [1.0, 0.7, 0.5] {
            assert_eq!(p.apply_on(&current, &original, amount).crop.aspect, None, "amount {amount}: the preset's null clears the aspect");
        }
        assert_eq!(p.apply_on(&current, &original, 0.3).crop.aspect, None, "below half amount the import baseline (no aspect) applies");
    }

    /// The import state has no aspect, and that null is the baseline below half amount (the amount
    /// contract: a named setting moves between the import state and the preset's value); from half
    /// amount the preset's aspect applies.
    #[test]
    fn a_null_in_the_original_is_the_baseline_below_half_amount() {
        let mut current = DevelopSettings::default();
        current.crop.aspect = Some((3, 2));
        let original = DevelopSettings::default();
        let p = Preset {
            id: "p".into(),
            name: "P".into(),
            group: "G".into(),
            settings: json!({"crop": {"aspect": [16, 9]}}),
            favorite: false,
            builtin: false,
        };
        assert_eq!(p.apply_on(&current, &original, 0.4).crop.aspect, None, "40 %: the import baseline (no aspect) applies");
        assert_eq!(p.apply_on(&current, &original, 0.6).crop.aspect, Some((16, 9)), "60 %: the preset's aspect applies");
    }

    #[test]
    fn deep_merge_objects() {
        let mut a = json!({"x": {"a": 1, "b": 2}, "y": [1]});
        deep_merge(&mut a, &json!({"x": {"b": 3}, "y": [2, 3]}));
        assert_eq!(a, json!({"x": {"a": 1, "b": 3}, "y": [2, 3]}));
    }

    /// The vignette style ids the SwiftUI app sends through `develop.merge` (`EffectsGroup.styles`):
    /// "color" once failed with `unknown variant`, and each must come back out as it went in.
    #[test]
    fn vignette_style_wire_names() {
        use crate::VignetteStyle::*;
        for (id, style) in [("highlightPriority", HighlightPriority), ("colorPriority", ColorPriority), ("paintOverlay", PaintOverlay)] {
            let s: DevelopSettings = serde_json::from_value(json!({"vignette": {"style": id}})).unwrap();
            assert_eq!(s.vignette.style, style);
            assert_eq!(serde_json::to_value(s.vignette.style).unwrap(), json!(id));
        }
        assert!(serde_json::from_value::<DevelopSettings>(json!({"vignette": {"style": "color"}})).is_err());
    }

    #[test]
    fn preset_masks_get_unused_ids_after_a_mask_id_at_the_limit() {
        let s = DevelopSettings { masks: vec![crate::Mask { id: u32::MAX, name: "Theirs".into(), ..Default::default() }], ..Default::default() };
        let p = Preset {
            id: "p".into(),
            name: "P".into(),
            group: "User".into(),
            settings: json!({"masks": [{"id": 7, "name": "A"}, {"id": 7, "name": "B"}]}),
            favorite: false,
            builtin: false,
        };
        let out = p.apply(&s, 1.0);
        let mut ids: Vec<u32> = out.masks.iter().map(|m| m.id).collect();
        assert_eq!(ids.len(), 3);
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 3, "{:?}", out.masks);
    }

    #[test]
    fn preset_masks_skip_ids_an_existing_mask_has_after_the_first_free_one() {
        // the first free id is 1, and 2 is taken: the second added mask must not get 2
        let theirs = |id| crate::Mask { id, name: format!("T{id}"), ..Default::default() };
        let s = DevelopSettings { masks: vec![theirs(u32::MAX), theirs(2)], ..Default::default() };
        let p = Preset {
            id: "p".into(),
            name: "P".into(),
            group: "User".into(),
            settings: json!({"masks": [{"name": "A"}, {"name": "B"}]}),
            favorite: false,
            builtin: false,
        };
        let mut ids: Vec<u32> = p.apply(&s, 1.0).masks.iter().map(|m| m.id).collect();
        ids.sort_unstable();
        assert_eq!(ids, [1, 2, 3, u32::MAX]);
    }
}
