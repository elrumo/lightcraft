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
//! `layer.style` gives any layer a border, a shadow, rounded corners and an effect (a sticker, a holographic foil), kept
//! here and drawn by a native frontend's renderer too. `layer.subject` says what covers a layer behind the subject (the
//! subject found, the people, or objects chosen with taps, added and taken away one at a time), `layer.rename` names a
//! layer, and removing, showing, copying and putting behind take several layers at once (`layers`), as one undo step.
//!
//! Groups: `layer.group` puts layers in a group, whose header is a layer of its own just above them (kind `group`: a
//! name, an eye that hides them all and an opacity for them together); `layer.ungroup` takes a layer out or dissolves a
//! group. A group moves, is copied and is removed whole by its header, and every edit leaves each group's layers just
//! under its header (`tidy_layers`), so a layer is still named by its place. Groups hold layers, not groups.

use lightcraft_catalog::{MediaKind, PhotoId};
use lightcraft_develop::{
    ExposureLayer, LayerBlend, LayerDrawing, LayerShape, LayerStyle, LayerSubject, LayerText, MAX_EXPOSURE_LAYERS, MAX_LAYER_DRAWING,
    MAX_LAYER_FAMILY, MAX_LAYER_NAME, MAX_LAYER_TEXT, MAX_LAYERS, MAX_SUBJECT_POINTS, layer_header, next_group_id, tidy_layers,
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

/// The layers a call names: `layers` (several indices) or `layer` (one), each once, bottom first, all of them there.
fn layers_param(p: &Value, layers: &[ExposureLayer], c: &str) -> Result<Vec<usize>> {
    let mut picked: Vec<usize> = match p.get("layers").filter(|v| !v.is_null()) {
        Some(v) => {
            let list = v.as_array().ok_or_else(|| bad(c, "`layers` must be a list of layer indices"))?;
            let read = |i: &Value| {
                i.as_u64().and_then(|n| usize::try_from(n).ok()).ok_or_else(|| bad(c, "`layers` must be a list of layer indices (0 = bottom)"))
            };
            list.iter().map(read).collect::<Result<Vec<_>>>()?
        }
        None => vec![layer_param(p, "layer", c)?],
    };
    picked.sort_unstable();
    picked.dedup();
    if picked.is_empty() {
        return Err(bad(c, "`layers` names no layer"));
    }
    for i in &picked {
        at(layers, *i, c)?;
    }
    Ok(picked)
}

/// A point of a call (`[x, y]`, 0..1 of the frame from its top left), in range.
fn point(v: &Value, key: &str, c: &str) -> Result<[f64; 2]> {
    let xy: Vec<f64> = v.as_array().map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
    let [x, y] = xy[..] else { return Err(bad(c, format!("`{key}` must be [x, y], 0..1 of the frame"))) };
    if !x.is_finite() || !y.is_finite() {
        return Err(bad(c, format!("`{key}` must be numbers")));
    }
    Ok([x.clamp(0.0, 1.0), y.clamp(0.0, 1.0)])
}

/// The subject fields a call sets over `su`: `kind`; `include` / `exclude` (the whole list of taps); `addInclude` /
/// `addExclude` (one more tap, the oldest dropped past the most kept; an added tap makes the subject `objects`);
/// `clear` (no taps); `feather`, `shift`. Checked and in range.
fn subject_params(p: &Value, mut su: LayerSubject, c: &str) -> Result<LayerSubject> {
    if bool_or(p, "clear", false) {
        su.include.clear();
        su.exclude.clear();
    }
    for (key, list) in [("include", &mut su.include), ("exclude", &mut su.exclude)] {
        if let Some(v) = p.get(key).filter(|v| !v.is_null()) {
            let points = v.as_array().ok_or_else(|| bad(c, format!("`{key}` must be a list of [x, y] points")))?;
            *list = points.iter().map(|xy| point(xy, key, c)).collect::<Result<Vec<_>>>()?;
        }
    }
    for (key, list) in [("addInclude", &mut su.include), ("addExclude", &mut su.exclude)] {
        if let Some(v) = p.get(key).filter(|v| !v.is_null()) {
            list.push(point(v, key, c)?);
            if list.len() > MAX_SUBJECT_POINTS {
                list.remove(0);
            }
            su.kind = "objects".into();
        }
    }
    if let Some(v) = p.get("kind").filter(|v| !v.is_null()) {
        let kind = v.as_str().filter(|k| LayerSubject::KINDS.contains(k)).ok_or_else(|| bad(c, "`kind` must be auto, people or objects"))?;
        su.kind = kind.into();
    }
    if let Some(v) = num(p, "feather", c)? {
        su.feather = v;
    }
    if let Some(v) = num(p, "shift", c)? {
        su.shift = v;
    }
    Ok(su.sane())
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
    // (every group's layers just under its header again, whatever moved)
    tidy_layers(&mut d.exposure_layers);
    let n = d.exposure_layers.len();
    s.set_develop(id, d, label)?;
    Ok(json!({"id": id.0, "count": n}))
}

fn at(layers: &[ExposureLayer], i: usize, c: &str) -> Result<()> {
    if i < layers.len() { Ok(()) } else { Err(bad(c, format!("no layer {i} (the photo has {})", layers.len()))) }
}

/// Room for one more layer (`photo`: a photo layer, of which a photo holds fewer).
fn room(layers: &[ExposureLayer], photo: bool, c: &str) -> Result<()> {
    room_for(layers, 1, usize::from(photo), c)
}

/// Room for `n` more layers (a group's header is one), `photos` of them photo layers.
fn room_for(layers: &[ExposureLayer], n: usize, photos: usize, c: &str) -> Result<()> {
    if layers.len().saturating_add(n) > MAX_LAYERS {
        return Err(bad(c, format!("a photo holds at most {MAX_LAYERS} layers")));
    }
    if photos > 0 && layers.iter().filter(|l| l.is_photo()).count().saturating_add(photos) > MAX_EXPOSURE_LAYERS {
        return Err(bad(c, format!("a photo holds at most {MAX_EXPOSURE_LAYERS} photo layers")));
    }
    Ok(())
}

/// Layer `i` is a layer, not a group's header (which has no picture to mix, dress or put behind).
fn plain(layers: &[ExposureLayer], i: usize, c: &str) -> Result<()> {
    at(layers, i, c)?;
    if layers[i].is_group() { Err(bad(c, format!("layer {i} is a group"))) } else { Ok(()) }
}

/// The group a call names by its header's place (`key`), as its id.
fn group_param(p: &Value, key: &str, layers: &[ExposureLayer], c: &str) -> Result<u32> {
    let h = layer_param(p, key, c)?;
    at(layers, h, c)?;
    layers[h].group.ok_or_else(|| bad(c, format!("layer {h} is not a group")))
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

/// The style fields a call sets (`border`, `borderColor`, `shadow`, `shadowBlur`, `shadowX`, `shadowY`, `radius`,
/// `effect`) over `st`, checked and in range.
fn style_params(p: &Value, mut st: LayerStyle, c: &str) -> Result<LayerStyle> {
    for (key, field) in [
        ("border", &mut st.border),
        ("shadow", &mut st.shadow),
        ("shadowBlur", &mut st.shadow_blur),
        ("shadowX", &mut st.shadow_x),
        ("shadowY", &mut st.shadow_y),
        ("radius", &mut st.radius),
    ] {
        if let Some(v) = num(p, key, c)? {
            *field = v;
        }
    }
    if let Some(v) = p.get("borderColor").filter(|v| !v.is_null()) {
        let rgb: Vec<f64> = v.as_array().map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
        let [r, g, b] = rgb[..] else { return Err(bad(c, "`borderColor` must be [r, g, b], sRGB 0..1")) };
        st.border_color = [r, g, b];
    }
    if let Some(v) = p.get("effect").filter(|v| !v.is_null()) {
        let effect = v.as_str().filter(|e| LayerStyle::EFFECTS.contains(e)).ok_or_else(|| bad(c, "`effect` must be none, sticker or holo"))?;
        st.effect = effect.into();
    }
    Ok(st.sane())
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
    if let Some(v) = p.get("family").filter(|v| !v.is_null()) {
        let family = v.as_str().ok_or_else(|| bad(c, "`family` must be a font family's name, or empty for the system font"))?;
        t.family = family.chars().take(MAX_LAYER_FAMILY).collect();
    }
    for (key, field) in [("bold", &mut t.bold), ("italic", &mut t.italic), ("underline", &mut t.underline), ("strike", &mut t.strike)] {
        if let Some(v) = p.get(key).filter(|v| !v.is_null()) {
            *field = v.as_bool().ok_or_else(|| bad(c, format!("`{key}` must be true or false")))?;
        }
    }
    if let Some(v) = p.get("weight").filter(|v| !v.is_null()) {
        let weight = v.as_str().filter(|w| w.is_empty() || LayerText::WEIGHTS.contains(w));
        t.weight = weight.ok_or_else(|| bad(c, format!("`weight` must be one of {}", LayerText::WEIGHTS.join(", "))))?.into();
    }
    if let Some(v) = p.get("align").filter(|v| !v.is_null()) {
        let align = v.as_str().filter(|a| LayerText::ALIGNS.contains(a)).ok_or_else(|| bad(c, "`align` must be left, center, right or justify"))?;
        t.align = align.into();
    }
    if let Some(v) = num(p, "tracking", c)? {
        t.tracking = v;
    }
    if let Some(v) = num(p, "leading", c)? {
        t.leading = v;
    }
    if let Some(rgb) = color(p, c)? {
        t.color = rgb;
    }
    Ok(t.sane())
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
            let name = match (&l.name, &l.text, &l.shape, &l.drawing) {
                // (the name it was given)
                (Some(given), ..) => Some(given.clone()),
                _ if l.is_group() => Some("group".to_string()),
                // (a text layer is named by its first line, a shape by its kind)
                (_, Some(t), _, _) => Some(t.text.lines().next().unwrap_or_default().chars().take(60).collect::<String>()),
                (_, _, Some(sh), _) => Some(sh.sane().kind),
                (_, _, _, Some(_)) => Some("drawing".to_string()),
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
                // (a group's header: how many layers it holds; a layer in a group: where its header is)
                if let Some(g) = l.group {
                    o.insert("members".into(), json!(d.exposure_layers.iter().filter(|m| m.parent == Some(g) && !m.is_group()).count()));
                }
                if let Some(h) = layer_header(&d.exposure_layers, l) {
                    o.insert("header".into(), json!(h));
                }
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
                        plain(layers, i, C)?;
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
            ..cmd!(
                "layer.delete",
                "Remove Layer",
                [],
                None,
                "{layer: index | layers: [index], id?} — one undo step; a group goes with its layers (`layer.ungroup` keeps them)",
                has_active,
                |s, p| {
                    const C: &str = "layer.delete";
                    let id = base_of(s, p, C)?;
                    layers_edit(s, id, "Remove Layer", |layers| {
                        let mut picked = layers_param(p, layers, C)?;
                        // (a group goes with what is in it)
                        let groups: Vec<u32> = picked.iter().filter_map(|i| layers[*i].group).collect();
                        picked.extend(layers.iter().enumerate().filter(|(_, l)| l.parent.is_some_and(|g| groups.contains(&g))).map(|(i, _)| i));
                        picked.sort_unstable();
                        picked.dedup();
                        // (top first, so the places below stay as they were named)
                        for i in picked.into_iter().rev() {
                            layers.remove(i);
                        }
                        Ok(())
                    })
                }
            )
        },
        CommandSpec {
            explicit_targets: true,
            ..cmd!(
                "layer.visible",
                "Show/Hide Layer",
                [],
                None,
                "{layer: index | layers: [index], visible?: bool, id?} — toggles when `visible` is left out (several: all shown unless all are, then all hidden)",
                has_active,
                |s, p| {
                    const C: &str = "layer.visible";
                    let id = base_of(s, p, C)?;
                    layers_edit(s, id, "Show/Hide Layer", |layers| {
                        let picked = layers_param(p, layers, C)?;
                        let all_shown = picked.iter().all(|i| layers[*i].visible);
                        let visible = bool_or(p, "visible", !all_shown);
                        for i in picked {
                            layers[i].visible = visible;
                        }
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
                "{layer: index, to: index (0 = bottom), group?: a group's index (the layer joins it) or -1 (it is in none), id?} — layers draw bottom to top; a group moves whole, its header landing on `to`; without `group` a layer stays in its group, at the nearest place in it",
                has_active,
                |s, p| {
                    const C: &str = "layer.move";
                    let id = base_of(s, p, C)?;
                    let i = layer_param(p, "layer", C)?;
                    let to = layer_param(p, "to", C)?;
                    let group = match p.get("group").filter(|v| !v.is_null()) {
                        None => None,
                        Some(v) => Some(v.as_i64().ok_or_else(|| bad(C, "`group` must be a group's index, or -1 for none"))?),
                    };
                    layers_edit(s, id, "Reorder Layers", |layers| {
                        at(layers, i, C)?;
                        let joins = match group {
                            None => None,
                            Some(g) if g < 0 => Some(None),
                            Some(_) => Some(Some(group_param(p, "group", layers, C)?)),
                        };
                        // what moves: a layer, or a group whole (its layers under its header, which lands on `to`)
                        let block: Vec<ExposureLayer> = if let Some(gid) = layers[i].group {
                            if matches!(joins, Some(Some(_))) {
                                return Err(bad(C, format!("layer {i} is a group: a group holds layers, not groups")));
                            }
                            let inside = |l: &ExposureLayer| l.parent == Some(gid) && !l.is_group();
                            let mut block: Vec<ExposureLayer> = layers.iter().filter(|l| inside(l)).cloned().collect();
                            block.push(layers[i].clone());
                            layers.retain(|l| !inside(l) && l.group != Some(gid));
                            block
                        } else {
                            let mut l = layers.remove(i);
                            if let Some(parent) = joins {
                                l.parent = parent;
                            }
                            vec![l]
                        };
                        let n = block.len();
                        let start = to.saturating_sub(n.saturating_sub(1)).min(layers.len());
                        let free = block.last().is_some_and(|l| l.parent.is_none());
                        layers.splice(start..start, block);
                        // Among another group's layers without joining it, going up: on past that group, over its header
                        // (going down, `tidy_layers` puts it under the group).
                        let among = |below: Option<&ExposureLayer>, above: Option<&ExposureLayer>| {
                            let g = below.and_then(|l| l.parent)?;
                            above.filter(|a| a.parent == Some(g) || a.group == Some(g)).map(|_| g)
                        };
                        let below = start.checked_sub(1).and_then(|b| layers.get(b));
                        if let Some(g) = among(below, layers.get(start + n)).filter(|_| free && to > i) {
                            let moved: Vec<ExposureLayer> = layers.drain(start..start + n).collect();
                            let over = layers.iter().position(|l| l.group == Some(g)).map_or(layers.len(), |h| h + 1);
                            layers.splice(over..over, moved);
                        }
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
                        plain(layers, i, C)?;
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
                "{text?, font?: system|serif|rounded|mono, family?, bold?, weight?, italic?, underline?, strike?, align?, tracking?, leading?, color?: [r, g, b] (sRGB 0..1), x?, y?, scale?, angle?, opacity?, blend?, id?} — a text layer on top of the photo (white bold words in the middle, half the frame wide, unless placed; drawn by a native frontend), as one undo step; no copy is made",
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
                "{layer: index, text?, font?: system|serif|rounded|mono, family?: an installed font family (empty: the system font), bold?, weight?: ultraLight…black, italic?, underline?, strike?, align?: left|center|right|justify, tracking?: thousandths of the size, leading?: a multiple of the line height, color?, id?} — a text layer's words and type, the fields left out kept",
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
                "{layer: index | layers: [index], id?} — a copy of each layer just above it (in its group, if it is in one), moved a little so both show; a group is copied with its layers; one undo step",
                has_active,
                |s, p| {
                    const C: &str = "layer.duplicate";
                    let id = base_of(s, p, C)?;
                    layers_edit(s, id, "Duplicate Layer", |layers| {
                        let nudged = |mut l: ExposureLayer| {
                            l.x = (l.x + 4.0).clamp(-100.0, 100.0);
                            l.y = (l.y + 4.0).clamp(-100.0, 100.0);
                            l
                        };
                        let picked = layers_param(p, layers, C)?;
                        let groups: Vec<u32> = picked.iter().filter_map(|i| layers[*i].group).collect();
                        // (top first, so a copy put in does not move the places still to copy)
                        for i in picked.into_iter().rev() {
                            if let Some(gid) = layers[i].group {
                                // (a group: its layers and its header, as a group of its own just above)
                                let new = next_group_id(layers);
                                let mut block: Vec<ExposureLayer> = layers
                                    .iter()
                                    .filter(|l| l.parent == Some(gid))
                                    .cloned()
                                    .map(|l| ExposureLayer { parent: Some(new), ..nudged(l) })
                                    .collect();
                                room_for(layers, block.len() + 1, block.iter().filter(|l| l.is_photo()).count(), C)?;
                                block.push(ExposureLayer { group: Some(new), ..layers[i].clone() });
                                layers.splice(i + 1..i + 1, block);
                            } else if !layers[i].parent.is_some_and(|g| groups.contains(&g)) {
                                // (not one copied with its group already)
                                room(layers, layers[i].is_photo(), C)?;
                                layers.insert(i + 1, nudged(layers[i].clone()));
                            }
                        }
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
                "{layer: index | layers: [index], behind?: bool, id?} — the photo's subject covers the layer (toggles when `behind` is left out); drawn by a native frontend, this core draws the layer over the subject",
                has_active,
                |s, p| {
                    const C: &str = "layer.behind";
                    let id = base_of(s, p, C)?;
                    layers_edit(s, id, "Behind Subject", |layers| {
                        // (a group's header has no picture to put behind: its layers are named themselves)
                        let picked: Vec<usize> = layers_param(p, layers, C)?.into_iter().filter(|i| !layers[*i].is_group()).collect();
                        if picked.is_empty() {
                            return Err(bad(C, "a group can't go behind the subject: name its layers"));
                        }
                        let all_behind = picked.iter().all(|i| layers[*i].behind);
                        let behind = bool_or(p, "behind", !all_behind);
                        for i in picked {
                            layers[i].behind = behind;
                        }
                        Ok(())
                    })
                }
            )
        },
        CommandSpec {
            explicit_targets: true,
            ..cmd!(
                "layer.subject",
                "Layer Subject",
                [],
                None,
                "{layer: index, kind?: auto|people|objects, include?: [[x, y]], exclude?: [[x, y]] (taps, 0..1 of the frame from its top left; the whole list), addInclude?: [x, y], addExclude?: [x, y] (one more tap: the subject becomes `objects`), clear?: bool (no taps), feather?: 0..100, shift?: -100..100, id?} — what covers the layer when it is behind the subject: the subject a native frontend finds, the people, or the objects under the included taps without those under the excluded ones; a tap added puts the layer behind. The fields left out are kept",
                has_active,
                |s, p| {
                    const C: &str = "layer.subject";
                    let id = base_of(s, p, C)?;
                    let i = layer_param(p, "layer", C)?;
                    layers_edit(s, id, "Layer Subject", |layers| {
                        plain(layers, i, C)?;
                        let subject = subject_params(p, layers[i].subject.clone().unwrap_or_default().sane(), C)?;
                        // (an object tapped is something to sit behind)
                        if p.get("addInclude").is_some_and(|v| !v.is_null()) {
                            layers[i].behind = true;
                        }
                        layers[i].subject = if subject.is_plain() { None } else { Some(subject) };
                        Ok(())
                    })
                }
            )
        },
        CommandSpec {
            explicit_targets: true,
            ..cmd!(
                "layer.rename",
                "Rename Layer",
                [],
                None,
                "{layer: index, name: string (empty: named by what it is again), id?}",
                has_active,
                |s, p| {
                    const C: &str = "layer.rename";
                    let id = base_of(s, p, C)?;
                    let i = layer_param(p, "layer", C)?;
                    let name = str_param(p, "name")
                        .ok_or_else(|| bad(C, "`name` must be a string"))?
                        .trim()
                        .chars()
                        .take(MAX_LAYER_NAME)
                        .collect::<String>();
                    layers_edit(s, id, "Rename Layer", |layers| {
                        at(layers, i, C)?;
                        layers[i].name = if name.is_empty() { None } else { Some(name) };
                        Ok(())
                    })
                }
            )
        },
        CommandSpec {
            explicit_targets: true,
            ..cmd!(
                "layer.style",
                "Layer Style",
                [],
                None,
                "{layer: index, border?: width in % of the layer's short side (0: none), borderColor?: [r, g, b], shadow?: strength 0..100 (0: none), shadowBlur?, shadowX?, shadowY?: % of the short side, radius?: the corners' in % of the short side, effect?: none|sticker|holo, id?} — a layer's border, shadow, rounded corners and effect, the fields left out kept (drawn by a native frontend; this core draws the layer plain)",
                has_active,
                |s, p| {
                    const C: &str = "layer.style";
                    let id = base_of(s, p, C)?;
                    let i = layer_param(p, "layer", C)?;
                    layers_edit(s, id, "Layer Style", |layers| {
                        plain(layers, i, C)?;
                        let style = style_params(p, layers[i].style.clone().unwrap_or_default().sane(), C)?;
                        // (a layer with nothing of it showing keeps none: its settings are those of a plain layer again)
                        layers[i].style = if style.is_plain() { None } else { Some(style) };
                        Ok(())
                    })
                }
            )
        },
        CommandSpec {
            explicit_targets: true,
            ..cmd!(
                "layer.group",
                "Group Layers",
                [],
                None,
                "{layer: index | layers: [index], into?: a group's index, name?, id?} — the layers become a group, whose header (a layer of kind `group`: its name, its eye and an opacity for them together) goes just above the topmost of them; with `into` they join that group instead. One undo step; groups hold layers, not groups",
                has_active,
                |s, p| {
                    const C: &str = "layer.group";
                    let id = base_of(s, p, C)?;
                    let name = str_param(p, "name").map(|n| n.trim().chars().take(MAX_LAYER_NAME).collect::<String>()).filter(|n| !n.is_empty());
                    layers_edit(s, id, "Group Layers", |layers| {
                        let picked = layers_param(p, layers, C)?;
                        if let Some(i) = picked.iter().find(|i| layers[**i].is_group()) {
                            return Err(bad(C, format!("layer {i} is a group: a group holds layers, not groups")));
                        }
                        let gid = if p.get("into").is_some_and(|v| !v.is_null()) {
                            group_param(p, "into", layers, C)?
                        } else {
                            room(layers, false, C)?;
                            let gid = next_group_id(layers);
                            // (above the topmost of them: the places named are all below it, and stay)
                            let top = picked.last().copied().unwrap_or(0);
                            layers.insert(top + 1, ExposureLayer { name, ..ExposureLayer::group(gid) });
                            gid
                        };
                        for i in picked {
                            layers[i].parent = Some(gid);
                        }
                        Ok(())
                    })
                }
            )
        },
        CommandSpec {
            explicit_targets: true,
            ..cmd!(
                "layer.ungroup",
                "Ungroup Layers",
                [],
                None,
                "{layer: index | layers: [index], id?} — a group named by its header is dissolved (its layers stay where they are, in no group); a layer in a group leaves it, to just under the group. One undo step",
                has_active,
                |s, p| {
                    const C: &str = "layer.ungroup";
                    let id = base_of(s, p, C)?;
                    layers_edit(s, id, "Ungroup Layers", |layers| {
                        let picked = layers_param(p, layers, C)?;
                        if let Some(i) = picked.iter().find(|i| !layers[**i].is_group() && layers[**i].parent.is_none()) {
                            return Err(bad(C, format!("layer {i} is in no group")));
                        }
                        let groups: Vec<u32> = picked.iter().filter_map(|i| layers[*i].group).collect();
                        for i in &picked {
                            layers[*i].parent = None;
                        }
                        // (a header gone leaves its layers in no group: `tidy_layers`)
                        layers.retain(|l| !l.group.is_some_and(|g| groups.contains(&g)));
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
                "{id?} → {id, layers: [{index, kind: photo|text|shape|drawing|group, photo, text?, shape?, drawing?, behind?, style?, subject?, group?: its id (a group's header), members?: how many layers it holds, parent?: its group's id, header?: its group's index, visible, blend, opacity, x, y, scale, angle, name, state: ok|missing|deleted|video|local, drawn}], max, maxPhotos, shown} — the photo's layers, each with what its photo is now (a text layer is named by its first line, a shape by its kind); a group's layers are the ones just under its header",
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
