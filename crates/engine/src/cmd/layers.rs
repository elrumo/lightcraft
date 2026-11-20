//! Double exposure (card #452): other photos of the library drawn over a photo (`exposure_layers` in its develop
//! settings). `photo.doubleExposure` makes one from a selection as a virtual copy of the base photo; the layer
//! commands change the active photo's layers, each as one undo step; each layer's sliders are indexed controls
//! (`exposureLayer.<i>.<opacity|x|y|scale|angle>` with `develop.set`). No pixels are written: a layer is a photo id.
//! Every one takes the photos a call names (`ids`, `id`) over the selection and checks them itself (`explicit_targets`):
//! a frontend's grid selection is not necessarily the engine's.
//!
//! Compositing (card #446) adds text layers (`layer.addText`, `layer.text`: words instead of a photo, on any photo, no
//! copy made), shape layers (`layer.addShape`, `layer.shape`), drawing layers (`layer.addDrawing`, `layer.drawing`:
//! a PencilKit drawing kept as its data), `layer.duplicate` and `layer.behind` (the photo's subject covers the layer).
//! This core stores them and leaves them to a native frontend's renderer: it draws no words, shapes or drawings, and
//! draws a layer that is behind over the subject. A new layer can be placed where it goes (`x`, `y`, `scale`, `angle`).

use lightcraft_catalog::{MediaKind, PhotoId};
use lightcraft_develop::{
    ExposureLayer, LayerBlend, LayerDrawing, LayerShape, LayerText, MAX_EXPOSURE_LAYERS, MAX_LAYER_DRAWING, MAX_LAYER_TEXT, MAX_LAYERS,
};
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

/// Room for one more layer (`photo`: a photo layer, of which a photo holds fewer).
fn room(layers: &[ExposureLayer], photo: bool, c: &str) -> Result<()> {
    if layers.len() >= MAX_LAYERS {
        return Err(bad(c, format!("a photo holds at most {MAX_LAYERS} layers")));
    }
    if photo && layers.iter().filter(|l| l.is_photo()).count() >= MAX_EXPOSURE_LAYERS {
        return Err(bad(c, format!("a photo holds at most {MAX_EXPOSURE_LAYERS} photo layers")));
    }
    Ok(())
}

/// A number field of a call, `None` when left out.
fn num(p: &Value, key: &str, c: &str) -> Result<Option<f64>> {
    match p.get(key).filter(|v| !v.is_null()) {
        None => Ok(None),
        Some(v) => v.as_f64().filter(|x| x.is_finite()).map(Some).ok_or_else(|| bad(c, format!("`{key}` must be a number"))),
    }
}

/// An sRGB colour field of a call, `None` when left out.
fn color(p: &Value, c: &str) -> Result<Option<[f64; 3]>> {
    let Some(v) = p.get("color").filter(|v| !v.is_null()) else { return Ok(None) };
    let rgb: Vec<f64> = v.as_array().map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
    let [r, g, b] = rgb[..] else { return Err(bad(c, "`color` must be [r, g, b], sRGB 0..1")) };
    if ![r, g, b].iter().all(|x| x.is_finite()) {
        return Err(bad(c, "`color` must be numbers"));
    }
    Ok(Some([r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0)]))
}

/// Where a new layer goes and how it mixes, from the fields a call gives (`x`, `y`, `scale`, `angle`, `opacity`,
/// `blend`), in range; the rest kept.
fn place_params(p: &Value, mut l: ExposureLayer, c: &str) -> Result<ExposureLayer> {
    if let Some(v) = num(p, "x", c)? {
        l.x = v.clamp(-100.0, 100.0);
    }
    if let Some(v) = num(p, "y", c)? {
        l.y = v.clamp(-100.0, 100.0);
    }
    if let Some(v) = num(p, "scale", c)? {
        l.scale = v.clamp(10.0, 400.0);
    }
    if let Some(v) = num(p, "angle", c)? {
        l.angle = v.clamp(-180.0, 180.0);
    }
    if let Some(v) = num(p, "opacity", c)? {
        l.opacity = v.clamp(0.0, 100.0);
    }
    if let Some(v) = p.get("blend").filter(|v| !v.is_null()) {
        l.blend = blend_param(v, c)?;
    }
    Ok(l)
}

fn blend_param(v: &Value, c: &str) -> Result<LayerBlend> {
    let name = v.as_str().ok_or_else(|| bad(c, "`blend` must be a blend mode's name"))?;
    LayerBlend::ALL
        .into_iter()
        .find(|m| serde_json::to_value(m).ok().as_ref().and_then(Value::as_str) == Some(name))
        .ok_or_else(|| bad(c, format!("no blend mode `{name}`")))
}

/// The shape fields a call sets (`kind`, `color`, `fill`, `stroke`, `aspect`, `corner`) over `sh`, checked.
fn shape_params(p: &Value, mut sh: LayerShape, c: &str) -> Result<LayerShape> {
    if let Some(v) = p.get("kind").filter(|v| !v.is_null()) {
        let kind = v.as_str().filter(|k| LayerShape::KINDS.contains(k)).ok_or_else(|| bad(c, "`kind` must be rectangle, ellipse, line or arrow"))?;
        sh.kind = kind.into();
    }
    if let Some(rgb) = color(p, c)? {
        sh.color = rgb;
    }
    if let Some(v) = p.get("fill").filter(|v| !v.is_null()) {
        sh.fill = v.as_bool().ok_or_else(|| bad(c, "`fill` must be true or false"))?;
    }
    if let Some(v) = num(p, "stroke", c)? {
        sh.stroke = v;
    }
    if let Some(v) = num(p, "aspect", c)? {
        sh.aspect = v;
    }
    if let Some(v) = num(p, "corner", c)? {
        sh.corner = v;
    }
    Ok(sh.sane())
}

/// The drawing fields a call sets (`pencil`: PencilKit data, base64; `aspect`) over `d`, checked.
fn drawing_params(p: &Value, mut d: LayerDrawing, c: &str) -> Result<LayerDrawing> {
    if let Some(v) = p.get("pencil").filter(|v| !v.is_null()) {
        let pencil = v.as_str().ok_or_else(|| bad(c, "`pencil` must be a PencilKit drawing's data, base64"))?;
        if pencil.len() > MAX_LAYER_DRAWING {
            return Err(bad(c, format!("the drawing is too big to keep ({} characters, at most {MAX_LAYER_DRAWING})", pencil.len())));
        }
        d.pencil = pencil.into();
        if d.sane().pencil.len() != pencil.len() {
            return Err(bad(c, "`pencil` must be base64"));
        }
    }
    if let Some(v) = num(p, "aspect", c)? {
        d.aspect = v;
    }
    Ok(d.sane())
}

/// The text-layer fields a call sets (`text`, `font`, `bold`, `color`) over `t`, checked; the text cut to
/// [`MAX_LAYER_TEXT`] characters.
fn text_params(p: &Value, mut t: LayerText, c: &str) -> Result<LayerText> {
    if let Some(v) = p.get("text").filter(|v| !v.is_null()) {
        let text = v.as_str().ok_or_else(|| bad(c, "`text` must be a string"))?;
        t.text = text.chars().take(MAX_LAYER_TEXT).collect();
    }
    if let Some(v) = p.get("font").filter(|v| !v.is_null()) {
        let font = v.as_str().ok_or_else(|| bad(c, "`font` must be system, serif, rounded or mono"))?;
        if !["system", "serif", "rounded", "mono"].contains(&font) {
            return Err(bad(c, format!("no font `{font}`: system, serif, rounded or mono")));
        }
        t.font = font.into();
    }
    if let Some(v) = p.get("bold").filter(|v| !v.is_null()) {
        t.bold = v.as_bool().ok_or_else(|| bad(c, "`bold` must be true or false"))?;
    }
    if let Some(rgb) = color(p, c)? {
        t.color = rgb;
    }
    Ok(t)
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
            let name = match (&l.text, &l.shape, &l.drawing) {
                // (a text layer is named by its first line, a shape by its kind)
                (Some(t), _, _) => Some(t.text.lines().next().unwrap_or_default().chars().take(60).collect::<String>()),
                (_, Some(sh), _) => Some(sh.sane().kind),
                (_, _, Some(_)) => Some("drawing".to_string()),
                _ => photo.map(|ph| match &ph.copy_name {
                    Some(c) => format!("{} ({c})", ph.file_name),
                    None => ph.file_name.clone(),
                }),
            };
            let st = if l.is_photo() { state(s, PhotoId(l.photo)) } else { "ok" };
            // (the photo layers past the most a picture draws are kept, not drawn)
            let photos_below = d.exposure_layers.iter().take(i).filter(|o| o.is_photo()).count();
            let mut v = serde_json::to_value(l).unwrap_or(Value::Null);
            if let Some(o) = v.as_object_mut() {
                o.insert("index".into(), json!(i));
                o.insert("kind".into(), json!(l.kind()));
                o.insert("state".into(), json!(st));
                o.insert("name".into(), json!(name));
                o.insert("drawn".into(), json!((!l.is_photo() || photos_below < MAX_EXPOSURE_LAYERS) && st == "ok"));
            }
            v
        })
        .collect();
    Ok(json!({"id": id.0, "layers": layers, "max": MAX_LAYERS, "maxPhotos": MAX_EXPOSURE_LAYERS, "shown": d.section_enabled("layers")}))
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
                "{photo, id?: the double exposure (default: the active photo), x?, y?, scale?, angle?, opacity?, blend?} — a layer of `photo` on top, screened over the frame unless placed and mixed otherwise",
                has_active,
                |s, p| {
                    const C: &str = "layer.add";
                    let id = base_of(s, p, C)?;
                    let photo = photo_param(p, "photo", C)?;
                    let layer = place_params(p, new_layer(s, id, photo, C)?, C)?;
                    layers_edit(s, id, "Add Layer", |layers| {
                        room(layers, true, C)?;
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
                        // (words, a shape or a drawing given a photo become a photo layer)
                        layers[i].text = None;
                        layers[i].shape = None;
                        layers[i].drawing = None;
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
                    let blend = blend_param(p.get("blend").unwrap_or(&Value::Null), C)?;
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
                "layer.addText",
                "Add Text",
                [],
                None,
                "{text?, font?: system|serif|rounded|mono, bold?, color?: [r, g, b] (sRGB 0..1), x?, y?, scale?, angle?, opacity?, blend?, id?} — a text layer on top of the photo (white bold words in the middle, half the frame wide, unless placed; drawn by a native frontend), as one undo step; no copy is made",
                has_active,
                |s, p| {
                    const C: &str = "layer.addText";
                    let id = base_of(s, p, C)?;
                    let text = text_params(p, LayerText { text: "Text".into(), ..LayerText::default() }, C)?;
                    let layer = place_params(p, ExposureLayer::text(text), C)?;
                    layers_edit(s, id, "Add Text", |layers| {
                        room(layers, false, C)?;
                        layers.push(layer);
                        Ok(())
                    })
                }
            )
        },
        CommandSpec {
            explicit_targets: true,
            ..cmd!(
                "layer.text",
                "Edit Text",
                [],
                None,
                "{layer: index, text?, font?, bold?, color?, id?} — a text layer's words and type, the fields left out kept",
                has_active,
                |s, p| {
                    const C: &str = "layer.text";
                    let id = base_of(s, p, C)?;
                    let i = layer_param(p, "layer", C)?;
                    layers_edit(s, id, "Edit Text", |layers| {
                        at(layers, i, C)?;
                        let Some(t) = layers[i].text.clone() else { return Err(bad(C, format!("layer {i} is a photo, not text"))) };
                        layers[i].text = Some(text_params(p, t.sane(), C)?);
                        Ok(())
                    })
                }
            )
        },
        CommandSpec {
            explicit_targets: true,
            ..cmd!(
                "layer.addShape",
                "Add Shape",
                [],
                None,
                "{kind?: rectangle|ellipse|line|arrow, color?: [r, g, b], fill?, stroke?: % of the box's width, aspect?: width/height, corner?: % of the short side, x?, y?, scale?, angle?, opacity?, blend?, id?} — a shape layer on top (a white square in the middle, a third of the frame wide, unless set; drawn by a native frontend), one undo step",
                has_active,
                |s, p| {
                    const C: &str = "layer.addShape";
                    let id = base_of(s, p, C)?;
                    let layer = place_params(p, ExposureLayer::shape(shape_params(p, LayerShape::default(), C)?), C)?;
                    layers_edit(s, id, "Add Shape", |layers| {
                        room(layers, false, C)?;
                        layers.push(layer);
                        Ok(())
                    })
                }
            )
        },
        CommandSpec {
            explicit_targets: true,
            ..cmd!(
                "layer.shape",
                "Edit Shape",
                [],
                None,
                "{layer: index, kind?, color?, fill?, stroke?, aspect?, corner?, id?} — a shape layer's look, the fields left out kept",
                has_active,
                |s, p| {
                    const C: &str = "layer.shape";
                    let id = base_of(s, p, C)?;
                    let i = layer_param(p, "layer", C)?;
                    layers_edit(s, id, "Edit Shape", |layers| {
                        at(layers, i, C)?;
                        let Some(sh) = layers[i].shape.clone() else { return Err(bad(C, format!("layer {i} is not a shape"))) };
                        layers[i].shape = Some(shape_params(p, sh.sane(), C)?);
                        Ok(())
                    })
                }
            )
        },
        CommandSpec {
            explicit_targets: true,
            ..cmd!(
                "layer.addDrawing",
                "Add Drawing",
                [],
                None,
                "{pencil?: a PencilKit drawing's data, base64, in a space 1000 units wide over the frame, aspect?: the frame's width/height, x?, y?, scale?, angle?, opacity?, blend?, id?} — a drawing layer on top, over the whole frame (drawn by a native frontend), one undo step",
                has_active,
                |s, p| {
                    const C: &str = "layer.addDrawing";
                    let id = base_of(s, p, C)?;
                    let layer = place_params(p, ExposureLayer::drawing(drawing_params(p, LayerDrawing::default(), C)?), C)?;
                    layers_edit(s, id, "Add Drawing", |layers| {
                        room(layers, false, C)?;
                        layers.push(layer);
                        Ok(())
                    })
                }
            )
        },
        CommandSpec {
            explicit_targets: true,
            ..cmd!(
                "layer.drawing",
                "Draw",
                [],
                None,
                "{layer: index, pencil?, aspect?, id?} — a drawing layer's strokes, replaced whole (one undo step a change)",
                has_active,
                |s, p| {
                    const C: &str = "layer.drawing";
                    let id = base_of(s, p, C)?;
                    let i = layer_param(p, "layer", C)?;
                    layers_edit(s, id, "Draw", |layers| {
                        at(layers, i, C)?;
                        let Some(d) = layers[i].drawing.clone() else { return Err(bad(C, format!("layer {i} is not a drawing"))) };
                        layers[i].drawing = Some(drawing_params(p, d, C)?);
                        Ok(())
                    })
                }
            )
        },
        CommandSpec {
            explicit_targets: true,
            ..cmd!(
                "layer.duplicate",
                "Duplicate Layer",
                [],
                None,
                "{layer: index, id?} — a copy of the layer just above it, moved a little so both show",
                has_active,
                |s, p| {
                    const C: &str = "layer.duplicate";
                    let id = base_of(s, p, C)?;
                    let i = layer_param(p, "layer", C)?;
                    layers_edit(s, id, "Duplicate Layer", |layers| {
                        at(layers, i, C)?;
                        room(layers, layers[i].is_photo(), C)?;
                        let mut copy = layers[i].clone();
                        copy.x = (copy.x + 4.0).clamp(-100.0, 100.0);
                        copy.y = (copy.y + 4.0).clamp(-100.0, 100.0);
                        layers.insert(i + 1, copy);
                        Ok(())
                    })
                }
            )
        },
        CommandSpec {
            explicit_targets: true,
            ..cmd!(
                "layer.behind",
                "Behind Subject",
                [],
                None,
                "{layer: index, behind?: bool, id?} — the photo's subject covers the layer (toggles when `behind` is left out); drawn by a native frontend, this core draws the layer over the subject",
                has_active,
                |s, p| {
                    const C: &str = "layer.behind";
                    let id = base_of(s, p, C)?;
                    let i = layer_param(p, "layer", C)?;
                    layers_edit(s, id, "Behind Subject", |layers| {
                        at(layers, i, C)?;
                        layers[i].behind = bool_or(p, "behind", !layers[i].behind);
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
                "{id?} → {id, layers: [{index, kind: photo|text|shape|drawing, photo, text?, shape?, drawing?, behind?, visible, blend, opacity, x, y, scale, angle, name, state: ok|missing|deleted|video|local, drawn}], max, maxPhotos, shown} — the photo's layers, each with what its photo is now (a text layer is named by its first line, a shape by its kind)",
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
