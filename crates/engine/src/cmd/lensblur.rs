//! Lens Blur commands (`crate::depth`, `lightcraft_pipeline::lensblur`). The sliders are develop
//! controls (`lensBlur.amount`, `lensBlur.focalDistance`, `lensBlur.focalRange`, `lensBlur.boost`
//! through `develop.set`); these do what a slider can't: turn it on (estimating the depth when the
//! photo has none of its own), focus on a point or the subject, and pick the bokeh.

use lightcraft_develop::{Bokeh, LensBlur};
use lightcraft_geom::Point;
use serde_json::{Value, json};

use super::{CommandSpec, bad, cmd, has_active, str_param};
use crate::{Result, Session};

/// Change the active photo's Lens Blur with `f` as one undo step labelled `label`.
fn edit(s: &mut Session, c: &str, label: &str, f: impl FnOnce(&mut LensBlur) -> Result<()>) -> Result<Value> {
    let id = s.active().ok_or_else(|| bad(c, "no active photo"))?;
    let mut d = (*s.develop_of(id).unwrap_or_default()).clone();
    f(&mut d.lens_blur)?;
    s.set_develop(id, d, label)?;
    lens_blur_info(s, c)
}

/// The active photo's Lens Blur as the apps show it.
fn lens_blur_info(s: &mut Session, c: &str) -> Result<Value> {
    let id = s.active().ok_or_else(|| bad(c, "no active photo"))?;
    let d = s.develop_of(id).unwrap_or_default();
    let lb = &d.lens_blur;
    let [near, far] = lb.focus_bounds();
    Ok(json!({
        "enabled": lb.enabled,
        "depth": s.depth_source(id).map(crate::depth::DepthSource::name),
        "modelReady": s.depth_model_ready(),
        "model": crate::depth::ID,
        "amount": lb.amount,
        "focalDistance": lb.focal_distance,
        "focalRange": lb.focal_range,
        "near": near,
        "far": far,
        "boost": lb.boost,
        "bokeh": lb.bokeh.name(),
        "bokehs": Bokeh::ALL.iter().map(|b| json!({"id": b.name(), "label": b.label()})).collect::<Vec<_>>(),
    }))
}

fn point(p: &Value) -> Option<Point> {
    let (x, y) = (p.get("x")?.as_f64()?, p.get("y")?.as_f64()?);
    (x.is_finite() && y.is_finite()).then(|| Point::new(x, y))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "lensBlur.info", "Lens Blur Info", [], None, "→ {enabled, depth: photo|estimated|null, modelReady, model, amount, focalDistance, focalRange, near, far, boost, bokeh, bokehs: [{id, label}]}", has_active, |s, _| lens_blur_info(s, "lensBlur.info")),
        cmd!(
            "lensBlur.apply",
            "Apply Lens Blur",
            [],
            None,
            "{on?: bool (default true)} — turns Lens Blur on (or off). The photo's own depth is used when its file has one; otherwise the depth model estimates it and the estimate is stored with the settings (fails with \"The depth model … is not downloaded\" when it isn't: `models.download {id: \"depth-anything-v2-small\", acknowledged: true}` after the user agreed). The first time it focuses on the subject → lensBlur.info",
            has_active,
            |s, p| {
                const C: &str = "lensBlur.apply";
                let on = p.get("on").and_then(Value::as_bool).unwrap_or(true);
                let id = s.active().ok_or_else(|| bad(C, "no active photo"))?;
                if !on {
                    return edit(s, C, "Lens Blur", |lb| {
                        lb.enabled = false;
                        Ok(())
                    });
                }
                let estimate = match s.depth_source(id) {
                    Some(_) => None,
                    None => Some(s.estimate_depth(id).map_err(|e| bad(C, e))?),
                };
                let fresh = s.develop_of(id).is_some_and(|d| d.lens_blur.amount == 0.0 && d.lens_blur.focal_range == 0.0);
                let focus = if fresh { s.subject_focus(id, estimate.as_ref()) } else { None };
                edit(s, C, "Lens Blur", |lb| {
                    if fresh {
                        let depth = lb.depth.take();
                        *lb = LensBlur { depth, ..LensBlur::applied(focus.unwrap_or(0.0)) };
                    }
                    lb.enabled = true;
                    if estimate.is_some() {
                        lb.depth = estimate;
                    }
                    Ok(())
                })
            }
        ),
        cmd!(
            "lensBlur.estimate",
            "Estimate Depth",
            [],
            None,
            "{} — (re)estimates the active photo's depth with the depth model and stores it, also for a photo with depth of its own (the estimate then wins) → lensBlur.info",
            has_active,
            |s, _| {
                const C: &str = "lensBlur.estimate";
                let id = s.active().ok_or_else(|| bad(C, "no active photo"))?;
                let depth = s.estimate_depth(id).map_err(|e| bad(C, e))?;
                edit(s, C, "Estimate Depth", |lb| {
                    lb.depth = Some(depth);
                    Ok(())
                })
            }
        ),
        cmd!(
            "lensBlur.focus",
            "Focus Lens Blur",
            [],
            None,
            "{x, y}: focus on the depth at that point (normalized image coordinates, as masks and spots use), or {subject: true}: on the photo's subject; the focal range keeps its width → lensBlur.info",
            has_active,
            |s, p| {
                const C: &str = "lensBlur.focus";
                let id = s.active().ok_or_else(|| bad(C, "no active photo"))?;
                let d = s.develop_of(id).unwrap_or_default();
                let focus = if p.get("subject").and_then(Value::as_bool).unwrap_or(false) {
                    s.subject_focus(id, None)
                } else {
                    let at = point(p).ok_or_else(|| bad(C, "give {x, y} or {subject: true}"))?;
                    s.focus_at(id, &d, at)
                };
                let focus = focus.ok_or_else(|| bad(C, "the photo has no depth here: apply Lens Blur first"))?;
                edit(s, C, "Lens Blur Focus", |lb| {
                    lb.focal_distance = focus;
                    Ok(())
                })
            }
        ),
        cmd!("lensBlur.bokeh", "Bokeh", [], None, "{shape: circle|bubble|blade5|ring|catEye} → lensBlur.info", has_active, |s, p| {
            const C: &str = "lensBlur.bokeh";
            let name = str_param(p, "shape").ok_or_else(|| bad(C, "missing `shape`"))?;
            let shape = Bokeh::parse(name).ok_or_else(|| bad(C, format!("unknown bokeh `{name}` (circle|bubble|blade5|ring|catEye)")))?;
            edit(s, C, "Bokeh", |lb| {
                lb.bokeh = shape;
                Ok(())
            })
        }),
    ]
}
