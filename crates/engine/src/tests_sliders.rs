//! FB-SLIDER: every develop control goes through every road a slider value takes (the `develop.set` command, the
//! `develop.controls` listing a UI draws its slider from, undo and redo, Copy and Paste Settings, reset, the settings
//! JSON a catalog, an XMP sidecar and a preset keep) and comes out as it went in. One test walks `CONTROLS`, so a
//! control added later is covered the day it is added.

use lightcraft_catalog::PhotoId;
use lightcraft_develop::controls::INDEXED;
use lightcraft_develop::{CONTROLS, DevelopSettings, SettingsGroup, controls};
use serde_json::{Value, json};

use crate::Session;

fn dev(s: &Session, id: PhotoId) -> DevelopSettings {
    (*s.develop_of(id).unwrap()).clone()
}

/// `d` as a render reads it: a white balance at As Shot carries numbers and a scale that As Shot never reads (a Temp
/// dragged back to the as-shot white leaves them normalised), so it counts as the default one.
fn norm(mut d: DevelopSettings) -> DevelopSettings {
    if d.wb.mode == lightcraft_develop::WbMode::AsShot {
        d.wb = lightcraft_develop::WhiteBalance::default();
    }
    d
}

/// The control listing's entry for `id` (`develop.controls`), as a slider is drawn from.
fn listed(s: &mut Session, id: &str) -> Value {
    let all = s.execute("develop.controls", &json!({})).unwrap();
    all.as_array().unwrap().iter().find(|c| c["id"] == id).cloned().unwrap_or_else(|| panic!("`{id}` is not listed"))
}

/// The value a sweep moves `c` to: its far end, or the near one when it already sits at the far end.
fn target(c: &controls::ControlSpec) -> f64 {
    if c.default == c.max { c.min } else { c.max }
}

/// The top-level settings key a control lives in (`light.exposure` → `light`), found by moving it.
fn json_key(c: &controls::ControlSpec) -> String {
    let mut s = DevelopSettings::default();
    controls::set(&mut s, c.id, target(c));
    let (a, b) = (DevelopSettings::default().to_json(), s.to_json());
    let (a, b) = (a.as_object().unwrap(), b.as_object().unwrap());
    b.iter().find(|(k, v)| a.get(*k) != Some(v)).map(|(k, _)| k.clone()).unwrap_or_else(|| panic!("`{}` changes nothing in the settings JSON", c.id))
}

#[test]
fn every_control_survives_the_commands_undo_copy_paste_and_reset() {
    let mut s = Session::with_demo();
    let vis = s.visible_cloned();
    let (a, b) = (vis[0], vis[1]);
    s.execute("library.select", &json!({"ids": [a.0], "active": a.0})).unwrap();
    let untouched = dev(&s, a);
    let copied: Vec<SettingsGroup> = SettingsGroup::default_copy();
    for c in CONTROLS {
        let want = c.clamp(target(c));
        let wb = c.id == "wb.temp" || c.id == "wb.tint";
        // develop.set → the settings and the listing a slider reads agree
        s.execute("develop.set", &json!({"control": c.id, "value": target(c)})).unwrap();
        let d = dev(&s, a);
        if !wb {
            assert_eq!(controls::get(&d, c.id), Some(want), "{}: develop.set", c.id);
            assert_eq!(listed(&mut s, c.id)["value"], json!(want), "{}: develop.controls", c.id);
        }
        assert_ne!(d, untouched, "{}: moving a control changes the settings", c.id);
        // the listing carries the spec the UI draws (range, default, step) unchanged; a rendered photo lists its
        // white balance on the relative scale (`wb.tempRel`, `wb.tintRel`, centred on 0)
        let l = listed(&mut s, if wb { if c.id == "wb.temp" { "wb.tempRel" } else { "wb.tintRel" } } else { c.id });
        if !wb {
            assert_eq!(
                (l["min"].as_f64(), l["max"].as_f64(), l["default"].as_f64(), l["step"].as_f64()),
                (Some(c.min), Some(c.max), Some(c.default), Some(c.step)),
                "{}",
                c.id
            );
        } else {
            assert_eq!(
                (l["min"].as_f64(), l["max"].as_f64(), l["default"].as_f64()),
                (Some(-100.0), Some(100.0), Some(0.0)),
                "{}: relative white balance",
                c.id
            );
        }
        // undo / redo
        s.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(norm(dev(&s, a)), norm(untouched.clone()), "{}: undo", c.id);
        s.execute("edit.redo", &json!({})).unwrap();
        assert_eq!(dev(&s, a), d, "{}: redo", c.id);
        // settings JSON (catalog, sidecar): exactly what was set comes back
        assert_eq!(DevelopSettings::from_json(&d.to_json()).unwrap(), d, "{}: JSON round trip", c.id);
        // a preset or `develop.merge` of the whole settings
        assert_eq!(untouched.merged(&d.to_json()).unwrap(), d, "{}: merged", c.id);
        // Copy and Paste Settings: the group that holds the control is copied by default (crop.* is the
        // Crop group, which Lightroom leaves out unless ticked, and which has its own tests)
        if !c.id.starts_with("crop.") {
            let key = json_key(c);
            assert!(
                copied.iter().any(|g| g.keys().contains(&key.as_str())),
                "{}: its settings key `{key}` is in no group Copy Settings copies by default",
                c.id
            );
            s.execute("develop.copy", &json!({})).unwrap();
            s.execute("develop.paste", &json!({"ids": [b.0]})).unwrap();
            if c.id != "wb.temp" && c.id != "wb.tint" {
                assert_eq!(controls::get(&dev(&s, b), c.id), Some(want), "{}: pasted", c.id);
            }
            s.execute("edit.undo", &json!({})).unwrap();
            assert_eq!(norm(dev(&s, b)), norm(untouched.clone()), "{}: undo of the paste", c.id);
            // (the undo above reverted the paste on `b`; select `a` again, where the redo history is)
            s.execute("library.select", &json!({"ids": [a.0], "active": a.0})).unwrap();
        }
        // reset: the control alone goes back, and the settings are the untouched ones again
        s.execute("develop.resetControl", &json!({"control": c.id})).unwrap();
        let r = dev(&s, a);
        if c.id == "wb.temp" || c.id == "wb.tint" {
            // (a reset of white balance goes back to the photo's As Shot, which a rendered photo has no number for)
            assert_eq!(r.wb.mode, untouched.wb.mode, "{}: reset puts white balance back to As Shot", c.id);
        }
        assert_eq!(norm(r), norm(untouched.clone()), "{}: reset gives the untouched settings back", c.id);
    }
}

/// The indexed controls (a point colour's sliders, a red eye correction's) take the same roads.
#[test]
fn indexed_controls_are_set_listed_and_reset() {
    let mut s = Session::with_demo();
    let a = s.visible_cloned()[0];
    s.execute("library.select", &json!({"ids": [a.0], "active": a.0})).unwrap();
    let eye = serde_json::to_value(lightcraft_develop::RedEye::default()).unwrap();
    s.execute("develop.merge", &json!({"settings": {"point_colors": [{"lum": 0.4, "chroma": 0.1, "hue": 120.0}], "red_eye": [eye]}})).unwrap();
    let base = dev(&s, a);
    assert_eq!((base.point_colors.len(), base.red_eye.len()), (1, 1), "the sample and the eye exist");
    for spec in INDEXED {
        let (fam, field) = spec.id.split_once('.').unwrap();
        let id = format!("{fam}.0.{field}");
        let to = if spec.default == spec.max { spec.min } else { spec.max };
        s.execute("develop.set", &json!({"control": id, "value": to})).unwrap();
        assert_eq!(controls::get(&dev(&s, a), &id), Some(to), "{id}");
        assert_eq!(listed(&mut s, &id)["value"], json!(to), "{id}: listed");
        s.execute("develop.resetControl", &json!({"control": id})).unwrap();
        assert_eq!(dev(&s, a), base, "{id}: reset");
    }
}

/// Everything moved at once survives the catalog's and the sidecar's JSON, and a section's reset puts the
/// controls of the section back to their defaults.
#[test]
fn all_controls_at_once_round_trip_and_sections_reset() {
    let mut all = DevelopSettings::default();
    for c in CONTROLS {
        controls::set(&mut all, c.id, target(c));
    }
    assert_eq!(DevelopSettings::from_json(&all.to_json()).unwrap(), all);
    for section in [
        lightcraft_develop::Section::Light,
        lightcraft_develop::Section::Curve,
        lightcraft_develop::Section::Color,
        lightcraft_develop::Section::Mixer,
        lightcraft_develop::Section::BwMix,
        lightcraft_develop::Section::Grading,
        lightcraft_develop::Section::Effects,
        lightcraft_develop::Section::Vignette,
        lightcraft_develop::Section::Grain,
        lightcraft_develop::Section::Detail,
        lightcraft_develop::Section::Optics,
        lightcraft_develop::Section::Geometry,
        lightcraft_develop::Section::Profile,
        lightcraft_develop::Section::Calibration,
        lightcraft_develop::Section::LensBlur,
    ] {
        let mut s = all.clone();
        s.reset_section(section);
        for c in CONTROLS.iter().filter(|c| c.section == section) {
            assert_eq!(controls::get(&s, c.id), Some(c.default), "{}: reset with its section {section:?}", c.id);
        }
        for c in CONTROLS.iter().filter(|c| c.section != section && !(section == lightcraft_develop::Section::Curve && c.section == section)) {
            assert_eq!(controls::get(&s, c.id), controls::get(&all, c.id), "{}: kept by the reset of {section:?}", c.id);
        }
    }
}

/// The preview a slider is judged by and the export of the same size are the same picture: every control, moved
/// through `develop.set`, renders alike through the preview job (`render_now`) and the export job (`render_export`).
#[test]
fn a_preview_and_an_export_of_the_same_size_agree_for_every_control() {
    use lightcraft_pipeline::{OutputDepth, OutputSpace};
    let mut s = Session::with_demo();
    let a = s.visible_cloned()[0];
    s.execute("library.select", &json!({"ids": [a.0], "active": a.0})).unwrap();
    let mut failures: Vec<String> = Vec::new();
    for c in CONTROLS {
        if c.id == "enhance.denoise" || c.id.starts_with("lensBlur.") {
            continue;
        }
        let mut base = dev(&s, a);
        controls::prepare_to_show(&mut base, c.id);
        controls::set(&mut base, c.id, c.default);
        s.execute("develop.merge", &json!({"settings": base.to_json()})).unwrap();
        s.execute("develop.set", &json!({"control": c.id, "value": target(c)})).unwrap();
        let preview = s.render_now(a, 240, 240).unwrap().image;
        let export = s.render_export(a, 240, 240, OutputSpace::Srgb, OutputDepth::U8).unwrap().image;
        if preview != export {
            failures.push(format!("{}: the preview and the export differ", c.id));
        }
        s.execute("develop.reset", &json!({})).unwrap();
    }
    assert!(failures.is_empty(), "{} problems:\n{}", failures.len(), failures.join("\n"));
}

/// A mask's sliders: the commands list the local specs (`mask.controls`), clamp to their ends (`mask.adjust`), undo,
/// and ride along when the masks are copied.
#[test]
fn every_local_slider_goes_through_the_mask_commands() {
    let mut s = Session::with_demo();
    let vis = s.visible_cloned();
    let (a, b) = (vis[0], vis[1]);
    s.execute("library.select", &json!({"ids": [a.0], "active": a.0})).unwrap();
    s.execute("mask.add", &json!({"kind": "radial"})).unwrap();
    let mask = dev(&s, a).masks[0].id;
    let specs: Vec<&controls::ControlSpec> = std::iter::once(&controls::LOCAL_AMOUNT).chain(controls::LOCAL.iter()).collect();
    let value = |s: &mut Session, id: &str| -> f64 {
        let list = s.execute("mask.controls", &json!({"id": mask})).unwrap();
        let c = list.as_array().unwrap().iter().find(|c| c["id"] == id).unwrap_or_else(|| panic!("`{id}` is not listed")).clone();
        c["value"].as_f64().unwrap()
    };
    let list = s.execute("mask.controls", &json!({"id": mask})).unwrap();
    assert_eq!(list.as_array().unwrap().len(), specs.len());
    for spec in specs {
        let c = list.as_array().unwrap().iter().find(|c| c["id"] == spec.id).unwrap_or_else(|| panic!("`{}` is not listed", spec.id));
        assert_eq!(
            (c["min"].as_f64(), c["max"].as_f64(), c["default"].as_f64(), c["step"].as_f64()),
            (Some(spec.min), Some(spec.max), Some(spec.default), Some(spec.step)),
            "{}",
            spec.id
        );
        assert_eq!(c["value"].as_f64(), Some(spec.default), "{}: a new mask holds the default", spec.id);
        // past either end is held at the end; the middle of the range goes in as it is
        for (asked, want) in [(spec.max + 1000.0, spec.max), (spec.min - 1000.0, spec.min), (spec.default, spec.default)] {
            s.execute("mask.adjust", &json!({"id": mask, "values": {spec.id: asked}})).unwrap();
            assert_eq!(value(&mut s, spec.id), want, "{}: asked for {asked}", spec.id);
        }
        // the far end, undone, then copied with the masks to another photo
        let far = if spec.default == spec.max { spec.min } else { spec.max };
        s.execute("mask.adjust", &json!({"id": mask, "values": {spec.id: far}})).unwrap();
        s.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(value(&mut s, spec.id), spec.default, "{}: undo", spec.id);
        s.execute("edit.redo", &json!({})).unwrap();
        s.execute("develop.copy", &json!({"groups": ["masks"]})).unwrap();
        s.execute("develop.paste", &json!({"ids": [b.0]})).unwrap();
        let got = dev(&s, b).masks.first().map(|m| serde_json::to_value(m.adjust).unwrap()[spec.id].as_f64().unwrap());
        assert_eq!(got, Some(far), "{}: pasted with the masks", spec.id);
        s.execute("edit.undo", &json!({})).unwrap();
        s.execute("library.select", &json!({"ids": [a.0], "active": a.0})).unwrap();
        s.execute("mask.adjust", &json!({"id": mask, "values": {spec.id: spec.default}})).unwrap();
    }
}
