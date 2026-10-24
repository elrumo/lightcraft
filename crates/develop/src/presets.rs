//! Presets and copy/paste subsets.
//!
//! A preset is a *partial* settings object (JSON) — only the groups it includes. Applying merges it
//! into the photo's settings; `amount` (0..200 %) interpolates numeric values between the current
//! settings and the preset's. Copy/paste/sync use the same group selection ([`SettingsGroup`]).

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
}

impl SettingsGroup {
    pub const ALL: [SettingsGroup; 20] = [
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
            SettingsGroup::Effects => &["effects"],
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
        }
    }
}

/// The subset of `s` covering `groups`, as a partial JSON object. AI Remove fills and Lens Blur's
/// estimated depth are pictures of this photo, so copies leave them behind (on another photo a
/// spot fills from around it, and Lens Blur uses that photo's depth), and spots that are the shape
/// of an object here (Tap to Remove) are not copied at all.
pub fn extract_groups(s: &DevelopSettings, groups: &[SettingsGroup]) -> Value {
    let full = s.to_json();
    let mut out = Map::new();
    for g in groups {
        for k in g.keys() {
            if let Some(v) = full.get(*k) {
                out.insert((*k).to_string(), v.clone());
            }
        }
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
                out.insert(k.clone(), scale_patch(bv, pv, t));
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

/// Apply a partial settings object with an amount (1.0 = 100 %). Out-of-range values are clamped
/// through the control specs, as every merge does ([`DevelopSettings::merged`]).
pub fn apply_partial(s: &DevelopSettings, partial: &Value, amount: f64) -> DevelopSettings {
    let patch = if (amount - 1.0).abs() < 1e-9 { Cow::Borrowed(partial) } else { Cow::Owned(scale_patch(&s.to_json(), partial, amount)) };
    // (a white balance without a scale keeps its old meaning: `DevelopSettings::merged`)
    s.merged(&patch).unwrap_or_else(|_| s.clone())
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
    /// Apply at `amount` (1.0 = 100 %). Masks in the preset are added to the photo's own (ones it
    /// already has are skipped), their effect scaled by the amount.
    pub fn apply(&self, s: &DevelopSettings, amount: f64) -> DevelopSettings {
        let Some(masks) = self.settings.get("masks").and_then(Value::as_array).filter(|m| !m.is_empty()) else {
            return apply_partial(s, &self.settings, amount);
        };
        let mut rest = self.settings.clone();
        if let Some(o) = rest.as_object_mut() {
            o.remove("masks");
        }
        let mut out = apply_partial(s, &rest, amount);
        let mut next = out.masks.iter().map(|m| m.id).max().unwrap_or(0);
        for m in masks {
            let Ok(mut m) = serde_json::from_value::<crate::Mask>(m.clone()) else { continue };
            if out.masks.iter().any(|q| q.name == m.name && q.components == m.components) {
                continue;
            }
            next += 1;
            m.id = next;
            m.adjust.amount = (m.adjust.amount * amount).clamp(0.0, 200.0);
            out.masks.push(m);
        }
        out
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
}
