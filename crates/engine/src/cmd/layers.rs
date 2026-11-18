//! Double exposure (card #452): other photos of the library drawn over a photo (`exposure_layers` in its develop
//! settings). `photo.doubleExposure` makes one from a selection as a virtual copy of the base photo; the layer
//! commands change the active photo's layers, each as one undo step; each layer's sliders are indexed controls
//! (`exposureLayer.<i>.<opacity|x|y|scale|angle>` with `develop.set`). No pixels are written: a layer is a photo id.
//! Every one takes the photos a call names (`ids`, `id`) over the selection and checks them itself (`explicit_targets`):
//! a frontend's grid selection is not necessarily the engine's.

use lightcraft_catalog::{MediaKind, PhotoId};
use lightcraft_develop::{ExposureLayer, LayerBlend, MAX_EXPOSURE_LAYERS};
use serde_json::{Value, json};

use super::{CommandSpec, bad, bool_or, cmd, has_active, has_selection, str_param};
use crate::{Result, Selection, Session};

/// Why photo `id` can't be a layer (or a double exposure's base), in words for the person who chose it; `None`: it can.
fn unusable(s: &Session, id: PhotoId) -> Option<&'static str> {
    match s.catalog.photo(id) {
        None => Some("the photo is not in the library any more"),
        Some(p) if p.deleted => Some("the photo is in Recently Deleted"),
        Some(p) if p.kind == MediaKind::Video => Some("a video can't be part of a double exposure"),
        Some(p) if p.local => Some("the photo is not in the library yet: add it to the library first"),
        Some(_) => None,
    }
}

/// A layer's state for `layer.list`: `ok`, `missing` (not in the library), `deleted` (in Recently Deleted), `video` or
/// `local` (browsed, not added to the library).
fn state(s: &Session, id: PhotoId) -> &'static str {
    match s.catalog.photo(id) {
        None => "missing",
        Some(p) if p.deleted => "deleted",
        Some(p) if p.kind == MediaKind::Video => "video",
        Some(p) if p.local => "local",
        Some(_) => "ok",
    }
}

fn photo_param(p: &Value, key: &str, c: &str) -> Result<PhotoId> {
    p.get(key).and_then(Value::as_u64).map(PhotoId).ok_or_else(|| bad(c, format!("`{key}` must be a photo id")))
}

fn layer_param(p: &Value, key: &str, c: &str) -> Result<usize> {
    p.get(key)
        .and_then(Value::as_u64)
        .and_then(|v| usize::try_from(v).ok())
        .ok_or_else(|| bad(c, format!("`{key}` must be a layer's index (0 = bottom)")))
}

/// The photo the layer commands change: `id`, else the active one.
fn base_of(s: &Session, p: &Value, c: &str) -> Result<PhotoId> {
    match p.get("id").filter(|v| !v.is_null()) {
        Some(v) => v.as_u64().map(PhotoId).filter(|id| s.catalog.photo(*id).is_some()).ok_or_else(|| bad(c, "`id` must be a photo of the library")),
        None => s.active().ok_or_else(|| bad(c, "no active photo")),
    }
}

/// A layer of `photo` for base `base`, checked.
fn new_layer(s: &Session, base: PhotoId, photo: PhotoId, c: &str) -> Result<ExposureLayer> {
    if let Some(why) = unusable(s, photo) {
        return Err(bad(c, format!("photo {} can't be a layer: {why}", photo.0)));
    }
    if photo == base {
        return Err(bad(c, "a photo can't be a layer of itself"));
    }
    Ok(ExposureLayer::new(photo.0))
}

/// Edit photo `id`'s layers as one undo step named `label`.
fn layers_edit(s: &mut Session, id: PhotoId, label: &str, f: impl FnOnce(&mut Vec<ExposureLayer>) -> Result<()>) -> Result<Value> {
    let mut d = (*s.develop_of(id).unwrap_or_default()).clone();
    f(&mut d.exposure_layers)?;
    let n = d.exposure_layers.len();
    s.set_develop(id, d, label)?;
    Ok(json!({"id": id.0, "count": n}))
}

fn at(layers: &[ExposureLayer], i: usize, c: &str) -> Result<()> {
    if i < layers.len() { Ok(()) } else { Err(bad(c, format!("no layer {i} (the photo has {})", layers.len()))) }
}

fn two_selected(s: &Session) -> std::result::Result<(), String> {
    has_selection(s)?;
    if s.selected().len() < 2 { Err("select two or more photos".into()) } else { Ok(()) }
}

/// `photo.doubleExposure`: a virtual copy of the base with the other photos as its layers, as one undo step.
fn create(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "photo.doubleExposure";
    let mut photos = s.targets(p)?;
    photos.dedup();
    let base = match p.get("base").filter(|v| !v.is_null()) {
        Some(_) => photo_param(p, "base", C)?,
        None => s.active().filter(|a| photos.contains(a)).or(photos.first().copied()).ok_or_else(|| bad(C, "no photos"))?,
    };
    if let Some(why) = unusable(s, base) {
        return Err(bad(C, format!("photo {} can't be the base: {why}", base.0)));
    }
    let others: Vec<PhotoId> = photos.into_iter().filter(|id| *id != base).collect();
    if others.is_empty() {
        return Err(bad(C, "select two or more photos: the base and at least one to lay over it"));
    }
    if others.len() > MAX_EXPOSURE_LAYERS {
        return Err(bad(C, format!("a double exposure holds at most {} photos ({MAX_EXPOSURE_LAYERS} over the base)", MAX_EXPOSURE_LAYERS + 1)));
    }
    let layers = others.iter().map(|o| new_layer(s, base, *o, C)).collect::<Result<Vec<_>>>()?;
    // (the copy's name, in the language of the app that asks)
    let name = str_param(p, "name").map(str::trim).filter(|n| !n.is_empty()).unwrap_or("Double Exposure").to_string();
    let copy = super::organize::virtual_copy(s, base, &name)?.ok_or_else(|| bad(C, "the base photo is not in the library"))?;
    let mut d = (*s.develop_of(copy).unwrap_or_default()).clone();
    d.exposure_layers = layers;
    s.set_develop(copy, d, "Double Exposure")?;
    s.merge_undo(2, "Create Double Exposure");
    s.selection = Selection::single(copy);
    Ok(json!({"id": copy.0, "base": base.0, "layers": others.iter().map(|o| o.0).collect::<Vec<_>>()}))
}

fn list(s: &Session, p: &Value) -> Result<Value> {
    const C: &str = "layer.list";
    let id = base_of(s, p, C)?;
    let d = s.develop_of(id).unwrap_or_default();
    let layers: Vec<Value> = d
        .exposure_layers
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let photo = s.catalog.photo(PhotoId(l.photo));
            let name = photo.map(|ph| match &ph.copy_name {
                Some(c) => format!("{} ({c})", ph.file_name),
                None => ph.file_name.clone(),
            });
            let mut v = serde_json::to_value(l).unwrap_or(Value::Null);
            if let Some(o) = v.as_object_mut() {
                o.insert("index".into(), json!(i));
                o.insert("state".into(), json!(state(s, PhotoId(l.photo))));
                o.insert("name".into(), json!(name));
                // (past the most a picture draws: kept, not drawn)
                o.insert("drawn".into(), json!(i < MAX_EXPOSURE_LAYERS && state(s, PhotoId(l.photo)) == "ok"));
            }
            v
        })
        .collect();
    Ok(json!({"id": id.0, "layers": layers, "max": MAX_EXPOSURE_LAYERS, "shown": d.section_enabled("layers")}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            explicit_targets: true,
            ..cmd!(
                "photo.doubleExposure",
                "Create Double Exposure",
                ["Photo"],
                None,
                "{ids?, base?: photoId (default: the active photo among them, else the first), name?: the copy's name (default \"Double Exposure\")} → {id, base, layers} — a virtual copy of the base with the other photos (at most 4) screened over it as layers; one undo step, the copy selected",
                two_selected,
                create
            )
        },
        CommandSpec {
            explicit_targets: true,
            ..cmd!(
                "layer.add",
                "Add Layer",
                [],
                None,
                "{photo, id?: the double exposure (default: the active photo)} — a layer of `photo` on top, screened over the frame",
                has_active,
                |s, p| {
                    const C: &str = "layer.add";
                    let id = base_of(s, p, C)?;
                    let photo = photo_param(p, "photo", C)?;
                    let layer = new_layer(s, id, photo, C)?;
                    layers_edit(s, id, "Add Layer", |layers| {
                        if layers.len() >= MAX_EXPOSURE_LAYERS {
                            return Err(bad(C, format!("a photo holds at most {MAX_EXPOSURE_LAYERS} layers")));
                        }
                        layers.push(layer);
                        Ok(())
                    })
                }
            )
        },
        CommandSpec {
            explicit_targets: true,
            ..cmd!(
                "layer.replace",
                "Replace Layer Photo",
                [],
                None,
                "{layer: index, photo, id?} — the layer draws another photo, its settings kept",
                has_active,
                |s, p| {
                    const C: &str = "layer.replace";
                    let id = base_of(s, p, C)?;
                    let i = layer_param(p, "layer", C)?;
                    let photo = photo_param(p, "photo", C)?;
                    new_layer(s, id, photo, C)?;
                    layers_edit(s, id, "Replace Layer Photo", |layers| {
                        at(layers, i, C)?;
                        layers[i].photo = photo.0;
                        Ok(())
                    })
                }
            )
        },
        CommandSpec {
            explicit_targets: true,
            ..cmd!("layer.delete", "Remove Layer", [], None, "{layer: index, id?}", has_active, |s, p| {
                const C: &str = "layer.delete";
                let id = base_of(s, p, C)?;
                let i = layer_param(p, "layer", C)?;
                layers_edit(s, id, "Remove Layer", |layers| {
                    at(layers, i, C)?;
                    layers.remove(i);
                    Ok(())
                })
            })
        },
        CommandSpec {
            explicit_targets: true,
            ..cmd!(
                "layer.visible",
                "Show/Hide Layer",
                [],
                None,
                "{layer: index, visible?: bool, id?} — toggles when `visible` is left out",
                has_active,
                |s, p| {
                    const C: &str = "layer.visible";
                    let id = base_of(s, p, C)?;
                    let i = layer_param(p, "layer", C)?;
                    layers_edit(s, id, "Show/Hide Layer", |layers| {
                        at(layers, i, C)?;
                        layers[i].visible = bool_or(p, "visible", !layers[i].visible);
                        Ok(())
                    })
                }
            )
        },
        CommandSpec {
            explicit_targets: true,
            ..cmd!(
                "layer.move",
                "Reorder Layers",
                [],
                None,
                "{layer: index, to: index (0 = bottom), id?} — layers draw bottom to top",
                has_active,
                |s, p| {
                    const C: &str = "layer.move";
                    let id = base_of(s, p, C)?;
                    let i = layer_param(p, "layer", C)?;
                    let to = layer_param(p, "to", C)?;
                    layers_edit(s, id, "Reorder Layers", |layers| {
                        at(layers, i, C)?;
                        let l = layers.remove(i);
                        layers.insert(to.min(layers.len()), l);
                        Ok(())
                    })
                }
            )
        },
        CommandSpec {
            explicit_targets: true,
            ..cmd!(
                "layer.blend",
                "Layer Blend Mode",
                [],
                None,
                "{layer: index, blend: normal|screen|multiply|overlay|softLight|lighten|darken|difference, id?}",
                has_active,
                |s, p| {
                    const C: &str = "layer.blend";
                    let id = base_of(s, p, C)?;
                    let i = layer_param(p, "layer", C)?;
                    let name = p.get("blend").and_then(Value::as_str).ok_or_else(|| bad(C, "`blend` must be a blend mode's name"))?;
                    let blend = LayerBlend::ALL
                        .into_iter()
                        .find(|m| serde_json::to_value(m).ok().as_ref().and_then(Value::as_str) == Some(name))
                        .ok_or_else(|| bad(C, format!("no blend mode `{name}`")))?;
                    layers_edit(s, id, "Layer Blend Mode", |layers| {
                        at(layers, i, C)?;
                        layers[i].blend = blend;
                        Ok(())
                    })
                }
            )
        },
        CommandSpec {
            explicit_targets: true,
            ..cmd!(
                query "layer.list",
                "List Layers",
                [],
                None,
                "{id?} → {id, layers: [{index, photo, visible, blend, opacity, x, y, scale, angle, name, state: ok|missing|deleted|video|local, drawn}], max, shown} — the double exposure's layers, each with what its photo is now",
                has_active,
                |s, p| list(s, p)
            )
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blend_names_parse() {
        for m in LayerBlend::ALL {
            let v = serde_json::to_value(m).unwrap();
            assert_eq!(serde_json::from_value::<LayerBlend>(v).unwrap(), m);
        }
        // (a name from a newer app reads as Screen)
        assert_eq!(serde_json::from_value::<LayerBlend>(json!("hueShift")).unwrap(), LayerBlend::Screen);
    }
}
