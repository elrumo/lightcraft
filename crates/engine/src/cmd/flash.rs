//! Flash commands (`lightcraft_pipeline::flash`). Its sliders are develop controls (`flash.*` through `develop.set`; the
//! flash's place around the lens is `flash.angle` and `flash.offset`, so a drag on the photo is a slider drag); these do
//! what a slider can't: turn it on (estimating the depth when the photo has none of its own, as Lens Blur does, and
//! keeping it where Lens Blur keeps it), set a preset, and pick the subject the flash is exposed for.

use lightcraft_develop::{DevelopSettings, Flash};
use lightcraft_geom::Point;
use serde_json::{Value, json};

use super::relight::{depth_fields, edit, merged, on};
use super::{CommandSpec, always, bad, cmd, has_active, str_param};
use crate::catalog::PhotoId;
use crate::{Result, Session};

/// The active photo's Flash as the apps show it.
fn flash_info(s: &mut Session, c: &str) -> Result<Value> {
    let id = s.active().ok_or_else(|| bad(c, "no active photo"))?;
    let f = s.develop_of(id).unwrap_or_default().flash.clone();
    let depth = depth_fields(s, c)?;
    let presets: Vec<Value> = Flash::PRESETS.iter().map(|(id, label)| json!({"id": id, "label": label})).collect();
    Ok(merged(
        json!({
            "enabled": f.enabled,
            "power": f.power,
            "falloff": f.falloff,
            "ambient": f.ambient,
            "tint": f.tint,
            "temp": f.temp,
            "hotspot": f.hotspot,
            "surface": f.surface,
            "shadow": f.shadow,
            "offset": f.offset,
            "angle": f.angle,
            "gloss": f.gloss,
            "clip": f.clip,
            "contrast": f.contrast,
            "emit": f.emit,
            "grain": f.grain,
            "fill": f.fill,
            "near": f.near,
            "far": f.far,
            "subject": f.subject,
            "presets": presets,
        }),
        depth,
    ))
}

fn flash_edit(s: &mut Session, c: &str, label: &str, f: impl FnOnce(&mut DevelopSettings) -> Result<()>) -> Result<Value> {
    edit(s, c, label, flash_info, f)
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "flash.info", "Flash Info", [], None, "→ {enabled, power, falloff, ambient, tint, temp (kelvin), hotspot, surface, shadow, offset, angle (degrees), gloss, clip, contrast, emit, grain, fill, near, far (the depths it reaches, 0 nearest to 100 farthest), subject (the nearness it is exposed for, 0..1, or null for the near part of the middle of the frame), presets: [{id, label}], depth: photo|estimated|null, modelReady, model}", has_active, |s, _| flash_info(s, "flash.info")),
        cmd!(
            "flash.apply",
            "Flash",
            [],
            None,
            "{on?: bool (default true), id?: a photo's id (default: the open photo), preset?: fill|direct|night}: turns Flash on (or off): the look of a xenon flash on the camera, made from the photo's depth (the sliders are develop.set of flash.*). The photo's own depth is used when its file has one; otherwise the depth model estimates it and the estimate is stored with the settings, where Lens Blur keeps it (fails with \"The depth model … is not downloaded\" when it isn't). `preset` sets every slider as flash.preset does. With `id` the photo need not be the open one (the camera applies it to the shot it just took) → flash.info for the open photo, {id, enabled} for another",
            always,
            |s, p| {
                const C: &str = "flash.apply";
                let named = p.get("id").and_then(Value::as_u64).map(PhotoId);
                let id = match named {
                    Some(id) => id,
                    None => s.active().ok_or_else(|| bad(C, "no active photo"))?,
                };
                let preset = str_param(p, "preset");
                if let Some(name) = preset.filter(|name| !Flash::PRESETS.iter().any(|(id, _)| id == name)) {
                    return Err(bad(C, format!("unknown preset `{name}` (fill|direct|night)")));
                }
                let enable = on(p);
                // (the photo's own depth, or the model's estimate to store: before the settings change, so a missing model changes nothing)
                let estimate = match (enable, s.depth_source(id)) {
                    (true, None) => Some(s.estimate_depth(id).map_err(|e| bad(C, e))?),
                    _ => None,
                };
                let mut d = (*s.develop_of(id).ok_or_else(|| bad(C, "no such photo"))?).clone();
                d.flash.enabled = enable;
                if enable {
                    if let Some(name) = preset {
                        d.flash.apply_preset(name);
                    }
                    if estimate.is_some() {
                        d.lens_blur.depth = estimate;
                    }
                }
                s.set_develop(id, d, "Flash")?;
                if named.is_some() && named != s.active() {
                    return Ok(json!({"id": id.0, "enabled": enable}));
                }
                flash_info(s, C)
            }
        ),
        cmd!(
            "flash.preset",
            "Flash Preset",
            [],
            None,
            "{preset: fill|direct|night}: sets every Flash slider to the preset's (the depth range and the subject stay) → flash.info",
            has_active,
            |s, p| {
                const C: &str = "flash.preset";
                let name = str_param(p, "preset").ok_or_else(|| bad(C, "missing `preset` (fill|direct|night)"))?;
                if !Flash::PRESETS.iter().any(|(id, _)| *id == name) {
                    return Err(bad(C, format!("unknown preset `{name}` (fill|direct|night)")));
                }
                flash_edit(s, C, "Flash Preset", |d| {
                    d.flash.apply_preset(name);
                    Ok(())
                })
            }
        ),
        cmd!(
            "flash.subject",
            "Flash Subject",
            [],
            None,
            "{x, y}: exposes the flash for the depth at that point (normalized image coordinates, as masks and spots use), or {auto: true}: back to the near part of the middle of the frame → flash.info",
            has_active,
            |s, p| {
                const C: &str = "flash.subject";
                let id = s.active().ok_or_else(|| bad(C, "no active photo"))?;
                if p.get("auto").and_then(Value::as_bool).unwrap_or(false) {
                    return flash_edit(s, C, "Flash Subject", |d| {
                        d.flash.subject = None;
                        Ok(())
                    });
                }
                let at = match (p.get("x").and_then(Value::as_f64), p.get("y").and_then(Value::as_f64)) {
                    (Some(x), Some(y)) if x.is_finite() && y.is_finite() => Point::new(x, y),
                    _ => return Err(bad(C, "give {x, y} or {auto: true}")),
                };
                let d = s.develop_of(id).unwrap_or_default();
                let focus = s.focus_at(id, &d, at).ok_or_else(|| bad(C, "the photo has no depth here: apply Flash first"))?;
                flash_edit(s, C, "Flash Subject", |d| {
                    d.flash.subject = Some((1.0 - focus / 100.0).clamp(0.0, 1.0));
                    Ok(())
                })
            }
        ),
        cmd!("flash.reset", "Reset Flash", [], None, "{}: the flash back to how it starts (the depth is kept) → flash.info", has_active, |s, _| {
            flash_edit(s, "flash.reset", "Reset Flash", |d| {
                d.flash = Flash { enabled: d.flash.enabled, ..Flash::default() };
                Ok(())
            })
        }),
    ]
}
