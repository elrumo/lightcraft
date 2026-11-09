//! Relight and Depth Map commands (`lightcraft_pipeline::relight`). Their sliders are develop controls
//! (`relight.*` and `depthMap.*` through `develop.set`; the light's place is `relight.x` and `relight.y`, so a
//! drag on the photo is a slider drag); these do what a slider can't: turn them on (estimating the depth when the
//! photo has none of its own, as Lens Blur does, and keeping it where Lens Blur keeps it), pick the light's
//! colour, the palette and its colours.

use lightcraft_develop::{DepthLook, DepthMap, DepthPalette, DevelopSettings, NormalMap, Relight};
use serde_json::{Value, json};

use super::{CommandSpec, always, bad, bool_or, cmd, has_active, str_param};
use crate::catalog::PhotoId;
use crate::media::SourceLevel;
use crate::{Result, Session};

/// The longest side `depthMap.export` writes, and the most pixels (a 32-bit float plane of this many is 400 MB).
const EXPORT_MAX_SIDE: usize = 16_384;
const EXPORT_MAX_PIXELS: usize = 100_000_000;

/// `depthMap.export`: photo `id`'s depth (nearness, white near) written to `path` as a 16-bit PNG or a 32-bit float TIFF,
/// framed as an export of the photo is (its crop unless `crop` is false) at `long` pixels along its long side (the
/// original's own when 0). `invert`: white far. → {path, width, height, format, white, depth}
fn export_depth(s: &mut Session, id: PhotoId, p: &Value) -> Result<Value> {
    const C: &str = "depthMap.export";
    let path = str_param(p, "path").filter(|x| !x.trim().is_empty()).ok_or_else(|| bad(C, "missing `path` (the file to write)"))?.to_string();
    let format = str_param(p, "format").unwrap_or("png").to_ascii_lowercase();
    if !matches!(format.as_str(), "png" | "tiff") {
        return Err(bad(C, format!("unknown format `{format}` (png: 16-bit; tiff: 32-bit float)")));
    }
    let invert = bool_or(p, "invert", false);
    let crop = bool_or(p, "crop", true);
    let asked = p.get("long").and_then(Value::as_u64).unwrap_or(0);
    let d = s.develop_of(id).ok_or_else(|| bad(C, "no such photo"))?;
    let src = s.source_now(id, SourceLevel::Preview).map_err(|e| bad(C, e))?;
    let info = s.source_info(id);
    let own = info.native_long.unwrap_or(0).max(src.width.max(src.height)).max(1);
    let long = if asked == 0 { own } else { usize::try_from(asked).unwrap_or(EXPORT_MAX_SIDE) }.clamp(1, EXPORT_MAX_SIDE);
    let plane = lightcraft_pipeline::relight::depth_out(&src, &info, &d, long, long, crop)
        .ok_or_else(|| bad(C, "This photo has no depth yet: apply Depth Map, Relight or Lens Blur first, which estimate it"))?;
    let (w, h) = (plane.width, plane.height);
    if w == 0 || h == 0 || w.saturating_mul(h) > EXPORT_MAX_PIXELS {
        return Err(bad(C, format!("a depth map of {w} × {h} is too large to write: ask for a smaller `long`")));
    }
    let value = |v: f32| {
        let v = if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
        if invert { 1.0 - v } else { v }
    };
    let meta = lightcraft_codecs::EncodeMeta::default();
    let bytes = if format == "png" {
        let samples: Vec<u16> = plane.data.iter().map(|v| (value(*v) * 65535.0).round() as u16).collect();
        let img = lightcraft_codecs::EncodeImage::new(w as u32, h as u32, 1, lightcraft_codecs::Samples::U16(&samples));
        lightcraft_codecs::encode_png(&img, &meta)
    } else {
        let samples: Vec<f32> = plane.data.iter().map(|v| value(*v)).collect();
        let img = lightcraft_codecs::EncodeImage::new(w as u32, h as u32, 1, lightcraft_codecs::Samples::F32(&samples));
        lightcraft_codecs::encode_tiff(&img, lightcraft_codecs::TiffCompression::Deflate, &meta)
    }
    .map_err(|e| bad(C, e.to_string()))?;
    std::fs::write(&path, bytes).map_err(|e| bad(C, format!("{path}: {e}")))?;
    Ok(json!({
        "path": path,
        "width": w,
        "height": h,
        "format": format,
        "white": if invert { "far" } else { "near" },
        "depth": s.depth_source(id).map(crate::depth::DepthSource::name),
    }))
}

/// Change the active photo's settings with `f` as one undo step labelled `label`, then answer with `info`.
fn edit(
    s: &mut Session,
    c: &str,
    label: &str,
    info: fn(&mut Session, &str) -> Result<Value>,
    f: impl FnOnce(&mut DevelopSettings) -> Result<()>,
) -> Result<Value> {
    let id = s.active().ok_or_else(|| bad(c, "no active photo"))?;
    let mut d = (*s.develop_of(id).unwrap_or_default()).clone();
    f(&mut d)?;
    s.set_develop(id, d, label)?;
    info(s, c)
}

/// The depth both tools read, for turning one on: `None` when the photo already has depth (its own, or an estimate
/// stored with its settings), else the model's estimate to store (an error the apps offer the download on when the
/// model isn't here).
fn depth_to_store(s: &mut Session, c: &str) -> Result<Option<DepthMap>> {
    let id = s.active().ok_or_else(|| bad(c, "no active photo"))?;
    match s.depth_source(id) {
        Some(_) => Ok(None),
        None => Ok(Some(s.estimate_depth(id).map_err(|e| bad(c, e))?)),
    }
}

/// Where the active photo's depth comes from, and whether the model could estimate it.
fn depth_fields(s: &mut Session, c: &str) -> Result<Value> {
    let id = s.active().ok_or_else(|| bad(c, "no active photo"))?;
    Ok(json!({
        "depth": s.depth_source(id).map(crate::depth::DepthSource::name),
        "modelReady": s.depth_model_ready(),
        "model": s.depth_size().id(),
    }))
}

fn merged(mut a: Value, b: Value) -> Value {
    if let (Some(a), Value::Object(b)) = (a.as_object_mut(), b) {
        a.extend(b);
    }
    a
}

/// The active photo's Relight as the apps show it.
fn relight_info(s: &mut Session, c: &str) -> Result<Value> {
    let id = s.active().ok_or_else(|| bad(c, "no active photo"))?;
    let r = s.develop_of(id).unwrap_or_default().relight.clone();
    let depth = depth_fields(s, c)?;
    Ok(merged(
        json!({
            "enabled": r.enabled,
            "x": r.x,
            "y": r.y,
            "color": r.color,
            "brightness": r.brightness,
            "ambient": r.ambient,
            "distance": r.distance,
            "spread": r.spread,
            "softness": r.softness,
            "shadows": r.shadows,
            "shine": r.shine,
            "surface": r.surface,
            "normals": r.normals.as_ref().map(|_| "model"),
            "surfaceReady": s.surface_model_ready(),
            "surfaceModel": lightcraft_depth::surface::ID,
        }),
        depth,
    ))
}

/// Relight's surface for the active photo when the surface model is here and the photo has none yet (`None`
/// otherwise): Relight turned on with the model downloaded lights the photo by its shape at once.
fn surface_to_store(s: &mut Session, c: &str) -> Result<Option<NormalMap>> {
    let id = s.active().ok_or_else(|| bad(c, "no active photo"))?;
    if !s.surface_model_ready() || s.develop_of(id).is_some_and(|d| d.relight.normals.is_some()) {
        return Ok(None);
    }
    s.estimate_surface(id).map(Some).map_err(|e| bad(c, e))
}

/// The active photo's Depth Map as the apps show it, with every palette's colours (far to near) for the pickers.
fn depth_map_info(s: &mut Session, c: &str) -> Result<Value> {
    let id = s.active().ok_or_else(|| bad(c, "no active photo"))?;
    let d = s.develop_of(id).unwrap_or_default().depth_map;
    let depth = depth_fields(s, c)?;
    let palettes: Vec<Value> = DepthPalette::ALL
        .iter()
        .map(|p| {
            let stops: Vec<[f64; 3]> = match p {
                DepthPalette::Custom => vec![d.far_color, d.near_color],
                p => p.stops().iter().map(|c| c.map(f64::from)).collect(),
            };
            json!({"id": p.name(), "label": p.label(), "stops": stops})
        })
        .collect();
    Ok(merged(
        json!({
            "enabled": d.enabled,
            "palette": d.palette.name(),
            "palettes": palettes,
            "nearColor": d.near_color,
            "farColor": d.far_color,
            "near": d.near,
            "far": d.far,
            "curve": d.curve,
            "contours": d.contours,
            "amount": d.amount,
            "invert": d.invert,
        }),
        depth,
    ))
}

/// A colour parameter: `[r, g, b]` (0..1, display-encoded sRGB) or `"#rrggbb"`; `None` when it is neither.
fn colour(v: &Value) -> Option<[f64; 3]> {
    if let Some(hex) = v.as_str() {
        let hex = hex.strip_prefix('#').unwrap_or(hex);
        if hex.len() != 6 || !hex.is_ascii() {
            return None;
        }
        let byte = |i: usize| hex.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok()).map(|b| f64::from(b) / 255.0);
        return Some([byte(0)?, byte(2)?, byte(4)?]);
    }
    let a = v.as_array()?;
    if a.len() != 3 {
        return None;
    }
    let c = [a.first()?.as_f64()?, a.get(1)?.as_f64()?, a.get(2)?.as_f64()?];
    c.iter().all(|v| v.is_finite()).then(|| c.map(|v| v.clamp(0.0, 1.0)))
}

/// `{on?}` as a bool (default on).
fn on(p: &Value) -> bool {
    p.get("on").and_then(Value::as_bool).unwrap_or(true)
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "relight.info", "Relight Info", [], None, "→ {enabled, x, y, color: [r,g,b], brightness, ambient, distance, spread, softness, shadows, shine, surface, normals: model|null (the surface model's), surfaceReady, surfaceModel, depth: photo|estimated|null, modelReady, model}", has_active, |s, _| relight_info(s, "relight.info")),
        cmd!(
            "relight.apply",
            "Relight",
            [],
            None,
            "{on?: bool (default true)} — turns Relight on (or off): a light placed in the scene by the photo's depth (move it with develop.set of relight.x / relight.y, normalized image coordinates). The photo's own depth is used when its file has one; otherwise the depth model estimates it and the estimate is stored with the settings, where Lens Blur keeps it (fails with \"The depth model … is not downloaded\" when it isn't) → relight.info",
            has_active,
            |s, p| {
                const C: &str = "relight.apply";
                if !on(p) {
                    return edit(s, C, "Relight", relight_info, |d| {
                        d.relight.enabled = false;
                        Ok(())
                    });
                }
                let estimate = depth_to_store(s, C)?;
                let normals = surface_to_store(s, C)?;
                edit(s, C, "Relight", relight_info, |d| {
                    d.relight.enabled = true;
                    if estimate.is_some() {
                        d.lens_blur.depth = estimate;
                    }
                    if normals.is_some() {
                        d.relight.normals = normals;
                    }
                    Ok(())
                })
            }
        ),
        cmd!(
            "relight.surface",
            "Estimate Surface",
            [],
            None,
            "{on?: bool (default true)} — (re)estimates which way each part of the active photo faces with the surface model (MoGe-2) and stores it with the settings: Relight then shades by it in place of the depth's slope, and still casts its shadows through the depth (fails with \"The surface model … is not downloaded\" when it isn't). {on: false}: back to the depth's slope → relight.info",
            has_active,
            |s, p| {
                const C: &str = "relight.surface";
                if !on(p) {
                    return edit(s, C, "Remove Surface", relight_info, |d| {
                        d.relight.normals = None;
                        Ok(())
                    });
                }
                let id = s.active().ok_or_else(|| bad(C, "no active photo"))?;
                let normals = s.estimate_surface(id).map_err(|e| bad(C, e))?;
                edit(s, C, "Estimate Surface", relight_info, |d| {
                    d.relight.normals = Some(normals);
                    Ok(())
                })
            }
        ),
        cmd!(
            "relight.color",
            "Light Color",
            [],
            None,
            "{color: [r,g,b] 0..1 or \"#rrggbb\"} — the light's colour → relight.info",
            has_active,
            |s, p| {
                const C: &str = "relight.color";
                let c = p.get("color").and_then(colour).ok_or_else(|| bad(C, "give `color` as [r, g, b] (0..1) or \"#rrggbb\""))?;
                edit(s, C, "Light Color", relight_info, |d| {
                    d.relight.color = c;
                    Ok(())
                })
            }
        ),
        cmd!(
            "relight.reset",
            "Reset Relight",
            [],
            None,
            "{} — the light back to where and how it starts (the depth and the surface are kept) → relight.info",
            has_active,
            |s, _| {
                edit(s, "relight.reset", "Reset Relight", relight_info, |d| {
                    d.relight = Relight { enabled: d.relight.enabled, normals: d.relight.normals.take(), ..Relight::default() };
                    Ok(())
                })
            }
        ),
        cmd!(query "depthMap.info", "Depth Map Info", [], None, "→ {enabled, palette, palettes: [{id, label, stops: [[r,g,b] far … near]}], nearColor, farColor, near, far, curve, contours, amount, invert, depth: photo|estimated|null, modelReady, model}", has_active, |s, _| depth_map_info(s, "depthMap.info")),
        cmd!(
            "depthMap.apply",
            "Depth Map",
            [],
            None,
            "{on?: bool (default true)} — draws the photo as its depth (or stops): the tone, colour and effects tools then edit it as any photo. The depth comes as for Relight → depthMap.info",
            has_active,
            |s, p| {
                const C: &str = "depthMap.apply";
                if !on(p) {
                    return edit(s, C, "Depth Map", depth_map_info, |d| {
                        d.depth_map.enabled = false;
                        Ok(())
                    });
                }
                let estimate = depth_to_store(s, C)?;
                edit(s, C, "Depth Map", depth_map_info, |d| {
                    d.depth_map.enabled = true;
                    if estimate.is_some() {
                        d.lens_blur.depth = estimate;
                    }
                    Ok(())
                })
            }
        ),
        cmd!(
            "depthMap.palette",
            "Depth Palette",
            [],
            None,
            "{palette: gray|thermal|ocean|spectrum|sunset|custom} → depthMap.info",
            has_active,
            |s, p| {
                const C: &str = "depthMap.palette";
                let name = str_param(p, "palette").ok_or_else(|| bad(C, "missing `palette`"))?;
                let palette = DepthPalette::parse(name)
                    .ok_or_else(|| bad(C, format!("unknown palette `{name}` (gray|thermal|ocean|spectrum|sunset|custom)")))?;
                edit(s, C, "Depth Palette", depth_map_info, |d| {
                    d.depth_map.palette = palette;
                    Ok(())
                })
            }
        ),
        cmd!(
            "depthMap.colors",
            "Depth Colors",
            [],
            None,
            "{near?: [r,g,b] | \"#rrggbb\", far?: …} — the Custom palette's colours (the palette becomes Custom) → depthMap.info",
            has_active,
            |s, p| {
                const C: &str = "depthMap.colors";
                let read = |key: &str| -> Result<Option<[f64; 3]>> {
                    match p.get(key) {
                        None | Some(Value::Null) => Ok(None),
                        Some(v) => colour(v).map(Some).ok_or_else(|| bad(C, format!("give `{key}` as [r, g, b] (0..1) or \"#rrggbb\""))),
                    }
                };
                let (near, far) = (read("near")?, read("far")?);
                if near.is_none() && far.is_none() {
                    return Err(bad(C, "give `near`, `far` or both"));
                }
                edit(s, C, "Depth Colors", depth_map_info, |d| {
                    d.depth_map.palette = DepthPalette::Custom;
                    if let Some(c) = near {
                        d.depth_map.near_color = c;
                    }
                    if let Some(c) = far {
                        d.depth_map.far_color = c;
                    }
                    Ok(())
                })
            }
        ),
        cmd!(
            "depthMap.invert",
            "Invert Depth",
            [],
            None,
            "{on?: bool (default true)} — the palette runs the other way (its near colour far away) → depthMap.info",
            has_active,
            |s, p| {
                let invert = on(p);
                edit(s, "depthMap.invert", "Invert Depth", depth_map_info, |d| {
                    d.depth_map.invert = invert;
                    Ok(())
                })
            }
        ),
        cmd!(
            query "depthMap.export",
            "Export Depth Map",
            [],
            None,
            "{path, id?: photo (the active one), format?: png (16-bit, the default) | tiff (32-bit float), invert?: bool (white far; white is near by default), long?: pixels along the long side (the original's own), crop?: bool (default true: framed as an export of the photo)} — writes the photo's depth as data for other programs: relative inverse depth (nearness, 0 farthest to 1 nearest), smoothed as Depth Map draws it, one value per pixel of the framed photo. The photo must have depth (its own, or the estimate Lens Blur, Relight or Depth Map stored) → {path, width, height, format, white: near|far, depth: photo|estimated}",
            always,
            |s, p| {
                let id = p
                    .get("id")
                    .and_then(Value::as_u64)
                    .map(PhotoId)
                    .or_else(|| s.active())
                    .ok_or_else(|| bad("depthMap.export", "no photo: give `id` or select one"))?;
                export_depth(s, id, p)
            }
        ),
        cmd!(
            "depthMap.reset",
            "Reset Depth Map",
            [],
            None,
            "{} — the palette, colours and range back to how they start (the depth is kept) → depthMap.info",
            has_active,
            |s, _| {
                edit(s, "depthMap.reset", "Reset Depth Map", depth_map_info, |d| {
                    d.depth_map = DepthLook { enabled: d.depth_map.enabled, ..DepthLook::default() };
                    Ok(())
                })
            }
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_read_as_lists_or_hex() {
        assert_eq!(colour(&json!([1.0, 0.5, 0.0])), Some([1.0, 0.5, 0.0]));
        assert_eq!(colour(&json!([2.0, -1.0, 0.25])), Some([1.0, 0.0, 0.25]));
        assert_eq!(colour(&json!("#ff8000")), Some([1.0, 128.0 / 255.0, 0.0]));
        assert_eq!(colour(&json!("ff8000")), Some([1.0, 128.0 / 255.0, 0.0]));
        for bad in [json!([1.0, 0.5]), json!("#ff80"), json!("#ggggggg"), json!("#ffé000"), json!(3), json!(null), json!([1.0, "a", 0.0])] {
            assert_eq!(colour(&bad), None, "{bad}");
        }
    }
}
