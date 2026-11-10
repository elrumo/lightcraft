//! Colorize commands (`lightcraft_pipeline::colorize`). The model runs in the frontend (DDColor through Core ML in a
//! native frontend): `colorize.input` gives it the photo, its answer comes back to `colorize.apply` and is stored with
//! the settings, so rendering, exporting and other devices never need the model. The amount is a develop control
//! (`colorize.amount` through `develop.set`).

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use lightcraft_develop::ChromaMap;
use lightcraft_develop::segmask::MAX_CHROMA_SIDE;
use serde_json::{Value, json};

use super::{CommandSpec, bad, cmd, has_active, photo_id, str_param};
use crate::catalog::PhotoId;
use crate::media::SourceLevel;
use crate::{Result, Session};

/// The input's long side when none is asked for (what DDColor takes).
const INPUT_EDGE: usize = 512;

fn target(s: &Session, p: &Value, c: &str) -> Result<PhotoId> {
    match photo_id(p, c)? {
        Some(id) => Ok(id),
        None => s.active().ok_or_else(|| bad(c, "no active photo")),
    }
}

/// Photo `id`'s Colorize as the apps show it.
fn info(s: &Session, id: PhotoId) -> Value {
    let c = s.develop_of(id).unwrap_or_default().colorize.clone();
    json!({
        "photo": id.0,
        "enabled": c.enabled,
        "amount": c.amount,
        "model": (!c.model.is_empty()).then_some(c.model),
        "colors": c.map.as_ref().map(|m| json!({"width": m.w, "height": m.h})),
    })
}

/// `map: {width, height, data}`: base64 of `width × height` pairs of signed bytes (Lab a, then b), row by row.
fn map_param(p: &Value, c: &str) -> Result<Option<ChromaMap>> {
    let Some(m) = p.get("map").filter(|m| !m.is_null()) else { return Ok(None) };
    let dim = |k: &str| {
        m.get(k)
            .and_then(Value::as_u64)
            .and_then(|v| usize::try_from(v).ok())
            .filter(|v| (1..=MAX_CHROMA_SIDE).contains(v))
            .ok_or_else(|| bad(c, format!("map needs a whole number `{k}` from 1 to {MAX_CHROMA_SIDE}")))
    };
    let (w, h) = (dim("width")?, dim("height")?);
    let data = m.get("data").and_then(Value::as_str).ok_or_else(|| bad(c, "map needs base64 `data`"))?;
    let bytes = STANDARD.decode(data.as_bytes()).map_err(|e| bad(c, format!("map `data` isn't base64: {e}")))?;
    if bytes.len() != w * h * 2 {
        return Err(bad(c, format!("map `data` has {} bytes; {w} × {h} pairs are {}", bytes.len(), w * h * 2)));
    }
    let ab: Vec<[f32; 2]> = bytes.as_chunks::<2>().0.iter().map(|[a, b]| [f32::from(*a as i8), f32::from(*b as i8)]).collect();
    ChromaMap::from_ab(w, h, &ab).map(Some).ok_or_else(|| bad(c, "the map is empty or too large"))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "colorize.info", "Colorize Info", [], None, "{photo?} → {photo, enabled, amount, model: string|null, colors: {width, height}|null}", has_active, |s, p| {
            let id = target(s, p, "colorize.info")?;
            Ok(info(s, id))
        }),
        cmd!(
            query "colorize.input",
            "Colorize Input Image",
            [],
            None,
            "{photo?, maxEdge?: 512} → {photo, width, height, png: base64}. The photo as a colourise model sees it: uncropped, as its file is oriented (the user's rotation not applied), without edits, exposed so its middle tone sits at mid-grey, sRGB, at most `maxEdge` long. The model's colours for it go back to colorize.apply at any size of the same shape",
            has_active,
            |s, p| {
                const C: &str = "colorize.input";
                let id = target(s, p, C)?;
                let edge = (super::f64_or(p, "maxEdge", INPUT_EDGE as f64) as usize).clamp(16, MAX_CHROMA_SIDE);
                let src = s.source_now(id, SourceLevel::Preview).map_err(|e| bad(C, e))?;
                let img = crate::depth::model_input_at(&src, edge);
                let bytes: Vec<u8> =
                    img.data.iter().flat_map(|c| c.map(|v| if v.is_finite() { (v.clamp(0.0, 1.0) * 255.0).round() as u8 } else { 0 })).collect();
                let enc = lightcraft_codecs::EncodeImage::new(img.width as u32, img.height as u32, 3, lightcraft_codecs::Samples::U8(&bytes));
                let png = lightcraft_codecs::encode_png(&enc, &lightcraft_codecs::EncodeMeta::default()).map_err(|e| bad(C, e.to_string()))?;
                Ok(json!({"photo": id.0, "width": img.width, "height": img.height, "png": STANDARD.encode(png)}))
            }
        ),
        cmd!(
            "colorize.apply",
            "Colorize",
            [],
            None,
            "{photo?, on?: bool (default true), map?: {width, height, data: base64 of signed byte pairs, Lab a then b, over the photo colorize.input gave}, model?: string} — stores the model's colours for the photo and turns Colorize on (one undo step); without `map`, turns it on or off with the colours it has (an error when there are none) → colorize.info",
            has_active,
            |s, p| {
                const C: &str = "colorize.apply";
                let id = target(s, p, C)?;
                let on = p.get("on").and_then(Value::as_bool).unwrap_or(true);
                let map = map_param(p, C)?;
                let mut d = (*s.develop_of(id).ok_or_else(|| bad(C, "no such photo"))?).clone();
                if let Some(m) = map {
                    d.colorize.map = Some(m);
                    d.colorize.model = str_param(p, "model").unwrap_or_default().chars().take(64).collect();
                } else if on && d.colorize.map.is_none() {
                    return Err(bad(C, "this photo has no colours yet: run the model and send its `map`"));
                }
                d.colorize.enabled = on;
                s.set_develop(id, d, "Colorize")?;
                Ok(info(s, id))
            }
        ),
        cmd!(
            "colorize.clear",
            "Remove Colors",
            [],
            None,
            "{photo?} — turns Colorize off and forgets the model's colours (one undo step) → colorize.info",
            has_active,
            |s, p| {
                const C: &str = "colorize.clear";
                let id = target(s, p, C)?;
                let mut d = (*s.develop_of(id).ok_or_else(|| bad(C, "no such photo"))?).clone();
                d.colorize = lightcraft_develop::Colorize { amount: d.colorize.amount, ..Default::default() };
                s.set_develop(id, d, "Remove Colors")?;
                Ok(info(s, id))
            }
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(w: usize, h: usize, a: i8, b: i8) -> Value {
        let bytes: Vec<u8> = (0..w * h).flat_map(|_| [a as u8, b as u8]).collect();
        json!({"width": w, "height": h, "data": STANDARD.encode(bytes)})
    }

    #[test]
    fn a_map_is_checked_before_it_is_stored() {
        let ok = map_param(&json!({"map": map(3, 2, 20, -40)}), "t").unwrap().unwrap();
        assert_eq!((ok.w, ok.h), (3, 2));
        assert_eq!(ok.ab().unwrap(), vec![[20.0, -40.0]; 6]);
        assert!(map_param(&json!({}), "t").unwrap().is_none());
        assert!(map_param(&json!({"map": null}), "t").unwrap().is_none());
        for bad_map in [
            json!({"width": 3, "height": 2, "data": STANDARD.encode([0u8; 11])}),
            json!({"width": 0, "height": 2, "data": ""}),
            json!({"width": MAX_CHROMA_SIDE + 1, "height": 1, "data": ""}),
            json!({"width": -3, "height": 2, "data": ""}),
            json!({"width": 2, "height": 2, "data": "not base64!"}),
            json!({"width": 2, "height": 2}),
            json!("a string"),
        ] {
            assert!(map_param(&json!({"map": bad_map}), "t").is_err(), "{bad_map}");
        }
    }
}
