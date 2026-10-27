//! Focus points: `view.focus` (where the camera focused, in the photo as shown), and for photos already in the library
//! `photo.readFocusPoints` (Photo ▸ Read Focus Points) and the per-frame `focus.pump` that keeps the background job
//! going. The job itself is `crate::focus_scan`; the mapping through rotation and crop is `crate::focus_view`.

use serde_json::{Value, json};

use super::{CommandSpec, cmd};
use crate::{Result, Session};

#[cfg(not(target_arch = "wasm32"))]
fn read_points(s: &mut Session, p: &Value) -> Result<Value> {
    crate::focus_scan::read_points(s, p)
}

#[cfg(target_arch = "wasm32")]
fn read_points(_: &mut Session, _: &Value) -> Result<Value> {
    Err(super::bad("photo.readFocusPoints", "reading focus points needs the photos' files on disk"))
}

#[cfg(not(target_arch = "wasm32"))]
fn pump(s: &mut Session, p: &Value) -> Result<Value> {
    crate::focus_scan::pump(s, p)
}

#[cfg(target_arch = "wasm32")]
fn pump(_: &mut Session, _: &Value) -> Result<Value> {
    Ok(serde_json::json!({"active": false}))
}

/// `view.focus`: the camera's focus box mapped to the photo as shown, or null.
fn view_focus(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "view.focus";
    let id = super::photo_id(p, c)?.or(s.active()).ok_or_else(|| super::bad(c, "no photo (pass `id`)"))?;
    let photo = s.catalog.photo(id).ok_or_else(|| super::bad(c, format!("no photo {}", id.0)))?;
    let Some(point) = photo.meta.focus.as_ref() else { return Ok(Value::Null) };
    let lens = s.embedded_lens(id);
    let Some(r) = crate::focus_view::focus_box(photo, lens.as_ref()) else { return Ok(Value::Null) };
    let mut out = json!({"rect": [r.x0, r.y0, r.x1, r.y1], "center": [(r.x0 + r.x1) / 2.0, (r.y0 + r.y1) / 2.0]});
    for (key, value) in [("mode", &point.mode), ("area", &point.area), ("tracking", &point.tracking)] {
        if let (Some(o), Some(v)) = (out.as_object_mut(), value) {
            o.insert(key.into(), json!(v));
        }
    }
    Ok(out)
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "view.focus",
            "Focus Point",
            [],
            None,
            "{id?} → {rect: [x0, y0, x1, y1], center: [x, y], mode?, area?, tracking?} | null — where the camera focused, in the photo as it is shown: the box normalized to the displayed image (y down) after the user's Rotate, flips and crop, `null` when the photo has no focus point or it is cropped away. The box may reach past an edge (clip it when drawing). A frontend draws it over the loupe (View ▸ Focus Point) and centres Toggle Zoom on `center` (View ▸ Zoom to Focus Point)",
            super::always,
            view_focus
        ),
        cmd!(
            "photo.readFocusPoints",
            "Read Focus Points",
            ["Photo"],
            None,
            "{ids?, wait?: false} → {queued, running, last?: {total, done, found, none, unavailable, bytesRead, seconds, cancelled}} — read the camera's focus point for photos imported before it was read (Sony α1 and A7S III raws and JPEGs), from the few KB of each file that hold it, on a background job with a row in the activity stack. Without `ids`, every photo in the library not looked at yet; with `ids`, those photos, looked at again. Photos that have a point keep it. No undo step and no sidecar write. `wait: true` returns when all are read (scripts)",
            super::always,
            read_points
        ),
        cmd!(
            query "focus.pump",
            "Read Focus Points in the Background",
            [],
            None,
            "{pace?: pause | light | normal} → {active, running?: {done, total, found}, last?, lookedAt} — cheap to call every frame (the app does): takes in what the background focus-point reader has read and commits it in batches; reads nothing new while paused and rests between files when light. Starts the reader by itself once per library opening when photos are left to look at",
            super::always,
            pump
        ),
    ]
}
