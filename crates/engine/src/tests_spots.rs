//! Remove tool commands: add (automatic source), select, move target/source, edit, refresh, delete.

use serde_json::json;

use crate::Session;

/// `library.up` / `library.down` (#1171): move by the given number of photos, clamped at the ends.
#[test]
fn up_and_down_move_by_rows_and_stop_at_the_ends() {
    let mut s = Session::with_demo();
    let vis = s.visible_cloned();
    assert!(vis.len() >= 6);
    s.selection = crate::Selection::single(vis[0]);
    s.execute("library.down", &json!({"rows": 4})).unwrap();
    assert_eq!(s.selection.active, Some(vis[4]));
    s.execute("library.up", &json!({})).unwrap();
    assert_eq!(s.selection.active, Some(vis[3]), "default is one photo (the filmstrip)");
    s.execute("library.up", &json!({"rows": 99})).unwrap();
    assert_eq!(s.selection.active, Some(vis[0]), "clamped at the first photo");
    s.execute("library.down", &json!({"rows": u64::MAX})).unwrap();
    assert_eq!(s.selection.active, vis.last().copied(), "hostile counts clamp, never overflow");
}

fn spots(s: &Session) -> Vec<lightcraft_develop::Spot> {
    s.develop_of(s.active().unwrap()).unwrap().spots.clone()
}

#[test]
fn spots_select_move_edit_refresh_and_delete() {
    let mut s = Session::with_demo();
    // a remove spot gets a resolved source (a pin to show) and is selected
    let r = s.execute("spot.add", &json!({"points": [[0.4, 0.5]], "size": 0.02})).unwrap();
    assert_eq!(r["index"], 0);
    assert_eq!(s.active_spot, Some(0));
    let src0 = spots(&s)[0].source_offset.expect("automatic source resolved");
    assert!(src0.x.hypot(src0.y) > 0.02, "the source is outside the spot: {src0:?}");
    s.execute("spot.add", &json!({"mode": "clone", "points": [[0.7, 0.3]], "source": [0.1, 0.0]})).unwrap();
    assert_eq!(s.active_spot, Some(1));
    assert_eq!(s.execute("library.state", &json!({})).unwrap()["activeSpot"], 1);
    // select
    s.execute("spot.select", &json!({"index": 0})).unwrap();
    assert_eq!(s.active_spot, Some(0));
    assert!(s.execute("spot.select", &json!({"index": 9})).is_err());
    // move the target (the source follows: its offset is relative), then the source alone
    s.execute("spot.update", &json!({"move": [0.05, -0.1]})).unwrap();
    let sp = &spots(&s)[0];
    assert!((sp.points[0].x - 0.45).abs() < 1e-9 && (sp.points[0].y - 0.4).abs() < 1e-9);
    assert_eq!(sp.source_offset, Some(src0));
    s.execute("spot.update", &json!({"moveSource": [0.01, 0.02]})).unwrap();
    let o = spots(&s)[0].source_offset.unwrap();
    assert!((o.x - src0.x - 0.01).abs() < 1e-9 && (o.y - src0.y - 0.02).abs() < 1e-9);
    // size / feather / opacity of the selected spot (clamped), one undo step each
    s.execute("spot.update", &json!({"size": 0.03, "feather": 20, "opacity": 150})).unwrap();
    let sp = &spots(&s)[0];
    assert_eq!((sp.size, sp.feather, sp.opacity), (0.03, 20.0, 100.0));
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(spots(&s)[0].size, 0.02);
    // refresh: a different source, at least a spot radius away
    let before = spots(&s)[0].source_offset.unwrap();
    let r = s.execute("spot.refreshSource", &json!({})).unwrap();
    let after = spots(&s)[0].source_offset.unwrap();
    assert!((after.x - before.x).hypot(after.y - before.y) > 0.01, "{r}");
    // delete the selected spot: the selection clears; deleting before a selected one shifts it
    s.execute("spot.delete", &json!({})).unwrap();
    assert_eq!((spots(&s).len(), s.active_spot), (1, None));
    assert_eq!(spots(&s)[0].mode, lightcraft_develop::SpotMode::Clone);
    assert!(s.execute("spot.delete", &json!({})).is_err(), "nothing selected");
    s.execute("spot.add", &json!({"points": [[0.2, 0.2]], "source": [0.05, 0.0]})).unwrap();
    s.execute("spot.delete", &json!({"index": 0})).unwrap();
    assert_eq!(s.active_spot, Some(0));
    // another photo: no selection
    s.execute("library.next", &json!({})).unwrap();
    assert_eq!(s.active_spot, None);
}

#[test]
fn a_content_aware_spot_has_no_source() {
    let mut s = Session::with_demo();
    s.execute("spot.add", &json!({"mode": "fill", "points": [[0.4, 0.5]], "size": 0.03})).unwrap();
    let sp = &spots(&s)[0];
    assert_eq!((sp.mode, sp.source_offset), (lightcraft_develop::SpotMode::Fill, None));
    assert_eq!(serde_json::to_value(sp).unwrap()["mode"], "fill");
    assert!(s.execute("spot.update", &json!({"mode": "nonsense"})).is_err());
}

/// AI Remove without its model says so (the apps offer the download on that message) and adds
/// nothing; a model that isn't the pinned file is refused.
#[test]
fn ai_remove_without_its_model_says_so_and_adds_nothing() {
    let mut s = Session::with_demo();
    let e = s.execute("spot.add", &json!({"points": [[0.4, 0.5]], "ai": true})).unwrap_err().to_string();
    assert!(e.contains(crate::erase::NOT_INSTALLED), "{e}");
    assert!(spots(&s).is_empty());
    assert!(!s.eraser_ready());
    assert!(s.execute("spot.add", &json!({"mode": "heal", "points": [[0.4, 0.5]], "ai": true})).is_err(), "AI fills Remove spots only");
    assert!(s.install_eraser(b"not the model").is_err());
    s.execute("spot.add", &json!({"mode": "clone", "points": [[0.4, 0.5]], "source": [0.1, 0.0]})).unwrap();
    assert!(s.execute("spot.erase", &json!({"index": 0})).is_err(), "a Clone spot");
}

/// With the model (when a copy is at `$LIGHTCRAFT_MIGAN`; it is never part of the repository) an
/// AI spot carries its fill, one undo step; moving the spot moves the fill until it is redone.
#[test]
fn ai_remove_stores_the_models_fill_with_the_spot() {
    let Some(path) = std::env::var_os("LIGHTCRAFT_MIGAN") else {
        lightcraft_testkit::skip!(Model, "LIGHTCRAFT_MIGAN names no MI-GAN file");
        return;
    };
    let mut s = Session::with_demo();
    s.install_eraser(&std::fs::read(path).unwrap()).unwrap();
    assert!(s.eraser_ready());
    let r = s.execute("spot.add", &json!({"points": [[0.4, 0.5], [0.45, 0.52]], "size": 0.03, "ai": true})).unwrap();
    assert_eq!(r["index"], 0);
    let sp = &spots(&s)[0];
    let f = sp.fill.as_ref().expect("a fill");
    let [x0, y0, x1, y1] = f.bounds().unwrap();
    assert!(x0 < 0.37 && x1 > 0.48 && y0 < 0.47 && y1 > 0.55, "the fill covers the stroke: {:?}", f.rect);
    assert!(f.w.max(f.h) <= 512 && f.pixels().is_some());
    assert_eq!(sp.source_offset, None, "no source pin for an AI spot");
    // it renders without the model being asked again (the fill is stored), and exports the same
    s.execute("spot.update", &json!({"move": [0.1, 0.0]})).unwrap();
    let moved = spots(&s)[0].fill.clone().unwrap();
    assert!((moved.rect[0] - x0 - 0.1).abs() < 1e-9);
    s.execute("spot.erase", &json!({})).unwrap();
    let redone = spots(&s)[0].fill.clone().unwrap();
    assert_ne!(redone.data, moved.data, "painted again at the new place");
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(spots(&s)[0].fill.as_ref().map(|f| f.rect), Some(moved.rect));
}

/// A network that fills the hole with the mean of the rest: enough to tell a fill was painted.
struct Flat;

impl lightcraft_erase::Painter for Flat {
    fn paint(&self, img: &lightcraft_raster::Rgb32f, hole: &[bool]) -> lightcraft_erase::Result<lightcraft_raster::Rgb32f> {
        let (mut sum, mut n) = ([0f32; 3], 0f32);
        for (p, h) in img.data.iter().zip(hole) {
            if !h {
                (0..3).for_each(|c| sum[c] += p[c]);
                n += 1.0;
            }
        }
        let mean = sum.map(|v| v / n.max(1.0));
        let mut out = img.clone();
        out.data.iter_mut().zip(hole).filter(|(_, h)| **h).for_each(|(p, _)| *p = mean);
        Ok(out)
    }

    fn label(&self) -> &'static str {
        "flat"
    }
}

/// Spot 0 of the demo photo and a job to fill it with [`Flat`].
fn flat_job(s: &mut Session) -> crate::erase::EraseJob {
    s.execute("spot.add", &json!({"points": [[0.4, 0.5]], "size": 0.03})).unwrap();
    let id = s.active().unwrap();
    let d = (*s.develop_of(id).unwrap()).clone();
    s.erase_job_with(id, &d, 0, crate::erase::Brush::Net(std::sync::Arc::new(Flat))).unwrap()
}

/// Polls until nothing is being painted (a worker answers in well under a second here).
fn poll_all(s: &mut Session) -> Vec<String> {
    let mut errors = Vec::new();
    for _ in 0..4000 {
        errors.extend(s.poll_erase());
        if s.erase_running() == 0 {
            return errors;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("the fill never finished");
}

#[test]
fn a_fill_painted_on_a_worker_is_stored_with_its_spot_as_one_undo_step() {
    let mut s = Session::with_demo();
    let job = flat_job(&mut s);
    s.spawn_erase(job, "test").unwrap();
    assert_eq!(s.erase_running(), 1);
    assert!(poll_all(&mut s).is_empty());
    let fill = spots(&s)[0].fill.clone().expect("the fill arrived");
    assert!(fill.w.max(fill.h) <= 512 && fill.pixels().is_some());
    assert!(s.activity.list().iter().all(|t| t.kind != "erase"), "its row in the activity stack ended");
    s.execute("edit.undo", &json!({})).unwrap();
    let sp = &spots(&s)[0];
    assert!(sp.fill.is_none(), "undo takes the fill back, the stroke stays (it renders as a plain Remove)");
}

#[test]
fn a_fill_whose_stroke_changed_meanwhile_is_dropped() {
    let mut s = Session::with_demo();
    let job = flat_job(&mut s);
    s.spawn_erase(job, "test").unwrap();
    s.execute("spot.update", &json!({"move": [0.1, 0.0]})).unwrap();
    assert!(poll_all(&mut s).is_empty(), "no error for a fill nobody wants");
    assert!(spots(&s)[0].fill.is_none(), "the moved stroke gets no fill of the old place");
    // deleted: nothing to store, nothing breaks
    let job = flat_job(&mut s);
    s.spawn_erase(job, "test").unwrap();
    s.execute("spot.delete", &json!({"index": 1})).unwrap();
    assert!(poll_all(&mut s).is_empty());
    assert_eq!(spots(&s).len(), 1);
}

#[test]
fn a_fill_cancelled_in_the_activity_stack_before_its_turn_is_not_painted() {
    let mut s = Session::with_demo();
    let job = flat_job(&mut s);
    // another fill holds the turn, so this one waits with its row up
    let turn = s.eraser.turn.clone();
    let held = turn.lock().unwrap();
    s.spawn_erase(job, "test").unwrap();
    let row = s.activity.list().into_iter().find(|t| t.kind == "erase").expect("a row while it waits");
    s.activity.cancel(row.id).unwrap();
    drop(held);
    assert!(poll_all(&mut s).is_empty(), "cancelling is not an error");
    assert!(spots(&s)[0].fill.is_none());
}

#[test]
fn ai_remove_says_what_is_missing_and_adds_nothing() {
    let mut s = Session::with_demo();
    let before = spots(&s).len();
    for params in [
        json!({"points": [[0.4, 0.5]], "ai": true, "wait": false}),
        json!({"points": [[0.4, 0.5]], "ai": true, "quality": "high"}),
        json!({"points": [[0.4, 0.5]], "ai": true, "quality": "high", "wait": false}),
        json!({"points": [[0.4, 0.5]], "ai": true, "quality": "ultra"}),
    ] {
        let e = s.execute("spot.add", &params).unwrap_err().to_string();
        assert!(e.contains("not downloaded") || e.contains("LaMa") || e.contains("Ultra"), "{e}");
    }
    let e = s.execute("spot.add", &json!({"points": [[0.4, 0.5]], "ai": true, "quality": "best"})).unwrap_err().to_string();
    assert!(e.contains("unknown quality"), "{e}");
    assert_eq!(spots(&s).len(), before, "a refused command leaves no spot");
    assert!(!s.remove_ready(crate::erase::RemoveQuality::High) && !s.remove_ready(crate::erase::RemoveQuality::Fast));
    assert!(!s.remove_ready(crate::erase::RemoveQuality::Ultra), "no diffusion model is downloaded");
    assert_eq!(crate::erase::RemoveQuality::from_id("high").map(|q| q.model_id()), Some("lama"));
}

/// An app's Ultra switch asks `crop.expandModels` which diffusion model it would paint with: Stable Diffusion 1.5 on a
/// phone with a GPU that may use 4 GB, none on one that may not.
#[test]
fn ultra_names_the_diffusion_model_this_device_runs() {
    use crate::hardware::{DeviceClass, DeviceProfile, GB};
    let mut s = Session::with_demo();
    let mut ultra = |memory: u64| {
        s.set_device(DeviceProfile { class: DeviceClass::Phone, memory_bytes: Some(memory), gpu: true, cores: 6 });
        s.execute("crop.expandModels", &json!({})).unwrap()["ultra"].clone()
    };
    assert!(ultra(2 * GB).is_null(), "a phone that may use 2 GB runs no diffusion model");
    if crate::enhance::Enhancer::AVAILABLE {
        assert_eq!(ultra(5 * GB), "sd15-inpaint");
    }
}

/// With LaMa (a copy at `$LIGHTCRAFT_LAMA`; it is never part of the repository) `quality: high` paints the fill with it,
/// on a worker and as one undo step; the Fast model is not needed for it.
#[test]
fn high_quality_ai_remove_paints_with_lama_on_a_worker() {
    let Some(path) = std::env::var_os("LIGHTCRAFT_LAMA") else {
        lightcraft_testkit::skip!(Model, "LIGHTCRAFT_LAMA names no LaMa file");
        return;
    };
    let mut s = Session::with_demo();
    s.set_device(crate::hardware::DeviceProfile {
        class: crate::hardware::DeviceClass::Computer,
        memory_bytes: Some(32 * crate::hardware::GB),
        gpu: false,
        cores: 8,
    });
    let dir = std::env::temp_dir().join(format!("lc-remove-hq-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("lama")).unwrap();
    let file = dir.join("lama").join(lightcraft_erase::lama::FILE);
    if std::fs::hard_link(&path, &file).is_err() {
        std::fs::copy(&path, &file).unwrap();
    }
    s.enhancer.dir = Some(dir.clone());
    assert!(s.remove_ready(crate::erase::RemoveQuality::High) && !s.remove_ready(crate::erase::RemoveQuality::Fast));
    let r =
        s.execute("spot.add", &json!({"points": [[0.4, 0.5], [0.45, 0.52]], "size": 0.03, "ai": true, "quality": "high", "wait": false})).unwrap();
    assert_eq!(r["started"], true);
    assert!(spots(&s)[0].fill.is_none(), "the spot is there at once, the fill follows");
    assert!(s.activity.list().iter().any(|t| t.kind == "erase"));
    assert!(poll_all(&mut s).is_empty());
    let fill = spots(&s)[0].fill.clone().expect("LaMa's fill");
    assert!(fill.w.max(fill.h) <= 512 && fill.pixels().is_some());
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(spots(&s)[0].fill.is_none());
    let _ = std::fs::remove_dir_all(dir);
}

fn target_session() -> Session {
    let mut s = Session::with_demo();
    s.execute("mask.add", &json!({"kind": "colorRange"})).unwrap();
    s.execute("mask.add", &json!({"kind": "radial"})).unwrap();
    s.execute("mask.addComponent", &json!({"kind": "colorRange"})).unwrap();
    s.execute("mask.saveEffect", &json!({"id": 1, "name": "Existing"})).unwrap();
    s.execute("mask.adjust", &json!({"values": {"exposure": 1.0}})).unwrap();
    for x in [0.3, 0.7] {
        s.execute("spot.add", &json!({"points": [[x, 0.5]], "source": [0.1, 0.0]})).unwrap();
    }
    // A rejected command must preserve a redo branch as well as existing undo steps.
    s.execute("mask.rename", &json!({"name": "Temporary"})).unwrap();
    s.execute("edit.undo", &json!({})).unwrap();
    s
}

fn target_state(s: &Session) -> serde_json::Value {
    let photo = s.catalog.photo(s.active().unwrap()).unwrap();
    json!({
        "develop": photo.develop,
        "history": photo.history,
        "selection": s.selection,
        "activeMask": s.active_mask,
        "activeSpot": s.active_spot,
        "undo": format!("{:?}", s.undo),
        "redo": format!("{:?}", s.redo),
        "commits": s.commits(),
        "effects": s.mask_effects,
        "rememberedEffects": s.mask_tool_effects,
        "journal": s.journal,
        "revision": s.catalog.revision,
    })
}

fn rejects_target(s: &mut Session, command: &str, params: &serde_json::Value) {
    let before = target_state(s);
    let result = s.execute(command, params);
    let after = target_state(s);
    assert!(result.is_err(), "{command} {params} returned {result:?}; state unchanged: {}", before == after);
    assert_eq!(after, before, "{command} {params} changed edits, selection or history on rejection");
    for field in ["id", "component", "index"] {
        if let Some(value) = params.get(field)
            && !value.is_null()
            && value.as_u64().is_none()
        {
            assert!(
                result.unwrap_err().to_string().contains(&format!("`{field}`")),
                "{command} must reject the malformed {field} before doing other work"
            );
            break;
        }
    }
}

fn malformed_targets() -> Vec<serde_json::Value> {
    vec![json!(-1), json!(0.5), json!("1"), json!(true), json!([]), json!({}), json!(u64::MAX)]
}

#[test]
fn malformed_mask_ids_are_rejected_without_changes() {
    let mut s = target_session();
    let commands = [
        ("mask.delete", json!({})),
        ("mask.rename", json!({"name": "Wrong mask"})),
        ("mask.invert", json!({})),
        ("mask.visible", json!({"visible": false})),
        ("mask.duplicate", json!({})),
        ("mask.move", json!({"to": 0})),
        ("mask.adjust", json!({"values": {"exposure": 1.0}})),
        ("mask.update", json!({"shape": {"kind": "brush", "strokes": []}})),
        ("mask.addComponent", json!({"kind": "brush"})),
        ("mask.component", json!({"component": 0, "action": "invert"})),
        ("mask.brushStroke", json!({"points": [[0.5, 0.5]]})),
        ("mask.sampleColor", json!({"x": 0.5, "y": 0.5})),
        ("mask.objectPoint", json!({"x": 0.5, "y": 0.5})),
        ("mask.refineDetail", json!({})),
    ];
    for (command, mut params) in commands {
        for target in malformed_targets().into_iter().chain([json!(4_294_967_297_u64)]) {
            params["id"] = target;
            rejects_target(&mut s, command, &params);
        }
    }
}

#[test]
fn malformed_mask_selection_is_rejected_without_changes() {
    let mut s = target_session();
    for target in malformed_targets().into_iter().filter(|v| !v.is_null()).chain([json!(4_294_967_297_u64)]) {
        rejects_target(&mut s, "mask.select", &json!({"id": target}));
    }
}

#[test]
fn overflowing_mask_id_does_not_wrap_to_an_existing_mask() {
    let mut s = target_session();
    rejects_target(&mut s, "mask.delete", &json!({"id": 4_294_967_297_u64}));
}

#[test]
fn malformed_mask_effect_targets_are_rejected_without_changes() {
    let mut s = target_session();
    for (command, mut params) in [
        ("mask.saveEffect", json!({"name": "Wrong effect"})),
        ("mask.updateEffect", json!({"name": "Existing"})),
        ("mask.applyEffect", json!({"name": "Existing"})),
    ] {
        for target in malformed_targets().into_iter().chain([json!(4_294_967_297_u64)]) {
            params["id"] = target;
            rejects_target(&mut s, command, &params);
        }
    }
    for target in malformed_targets().into_iter().chain([json!([1, -1]), json!([1, null]), json!([1, 4_294_967_297_u64])]) {
        rejects_target(&mut s, "mask.applyEffect", &json!({"name": "Existing", "ids": target}));
    }
}

#[test]
fn mask_effect_targets_preserve_explicit_defaults_and_lists() {
    let mut s = target_session();
    s.execute("mask.saveEffect", &json!({"id": 1, "name": "Explicit"})).unwrap();
    assert_eq!(s.mask_effects.last().unwrap().adjust.exposure, 0.0);
    for (name, target) in [("Omitted", json!({})), ("Null", json!({"id": null}))] {
        let mut params = target;
        params["name"] = json!(name);
        s.execute("mask.saveEffect", &params).unwrap();
        assert_eq!(s.mask_effects.last().unwrap().adjust.exposure, 1.0);
        params["name"] = json!("Existing");
        s.execute("mask.updateEffect", &params).unwrap();
        assert_eq!(s.mask_effects[0].adjust.exposure, 1.0);
    }
    s.execute("mask.updateEffect", &json!({"id": 1, "name": "Existing"})).unwrap();
    assert_eq!(s.mask_effects[0].adjust.exposure, 0.0);
    for mut params in [json!({}), json!({"id": null}), json!({"ids": null})] {
        s.execute("mask.adjust", &json!({"values": {"exposure": 1.0}})).unwrap();
        params["name"] = json!("Existing");
        s.execute("mask.applyEffect", &params).unwrap();
        assert_eq!(s.develop_of(s.active().unwrap()).unwrap().masks[1].adjust.exposure, 0.0);
    }
    s.execute("mask.adjust", &json!({"id": 1, "values": {"exposure": 1.0}})).unwrap();
    s.execute("mask.adjust", &json!({"id": 2, "values": {"exposure": 2.0}})).unwrap();
    s.execute("mask.applyEffect", &json!({"name": "Existing", "ids": [1, 2]})).unwrap();
    assert!(s.develop_of(s.active().unwrap()).unwrap().masks.iter().all(|m| m.adjust.exposure == 0.0));
    rejects_target(&mut s, "mask.applyEffect", &json!({"name": "Existing", "ids": []}));
}

#[test]
fn malformed_mask_components_are_rejected_without_changes() {
    let mut s = target_session();
    for (command, mut params) in [
        ("mask.update", json!({"shape": {"kind": "brush", "strokes": []}})),
        ("mask.component", json!({"action": "invert"})),
        ("mask.sampleColor", json!({"x": 0.5, "y": 0.5})),
        ("mask.refineDetail", json!({})),
    ] {
        for target in malformed_targets().into_iter().chain([json!(4_294_967_296_u64)]) {
            params["component"] = target;
            rejects_target(&mut s, command, &params);
        }
    }
}

#[test]
fn malformed_spot_indices_are_rejected_without_changes() {
    let mut s = target_session();
    for (command, mut params) in [("spot.delete", json!({})), ("spot.update", json!({"size": 0.1})), ("spot.refreshSource", json!({}))] {
        for target in malformed_targets().into_iter().chain([json!(4_294_967_296_u64)]) {
            params["index"] = target;
            rejects_target(&mut s, command, &params);
        }
    }
}

#[test]
fn malformed_spot_selection_is_rejected_without_changes() {
    let mut s = target_session();
    for target in malformed_targets().into_iter().filter(|v| !v.is_null()).chain([json!(4_294_967_296_u64)]) {
        rejects_target(&mut s, "spot.select", &json!({"index": target}));
    }
}

#[test]
fn mask_and_spot_targets_preserve_explicit_defaults_and_selection_clearing() {
    use lightcraft_develop::MaskShape;
    let mut s = target_session();
    for target in [json!({}), json!({"id": null})] {
        let mut params = target;
        params["name"] = json!("Selected mask");
        s.execute("mask.rename", &params).unwrap();
        assert_eq!(s.develop_of(s.active().unwrap()).unwrap().masks[1].name, "Selected mask");
    }
    s.execute("mask.rename", &json!({"id": 1, "name": "Explicit mask"})).unwrap();
    assert_eq!(s.develop_of(s.active().unwrap()).unwrap().masks[0].name, "Explicit mask");
    assert_eq!(s.active_mask, Some(2));
    for params in [json!({"shape": {"kind": "brush", "strokes": []}}), json!({"component": null, "shape": {"kind": "brush", "strokes": []}})] {
        s.execute("mask.update", &params).unwrap();
        let d = s.develop_of(s.active().unwrap()).unwrap();
        assert!(matches!(d.masks[1].components[0].shape, MaskShape::Brush { .. }));
        assert!(matches!(d.masks[1].components[1].shape, MaskShape::ColorRange { .. }));
    }
    s.execute("mask.update", &json!({"id": 1, "component": 0, "shape": {"kind": "sky"}})).unwrap();
    assert!(matches!(s.develop_of(s.active().unwrap()).unwrap().masks[0].components[0].shape, MaskShape::Sky));
    for params in [json!({"points": [[0.2, 0.2]]}), json!({"id": null, "points": [[0.4, 0.4]]})] {
        s.execute("mask.brushStroke", &params).unwrap();
    }
    assert_eq!(s.develop_of(s.active().unwrap()).unwrap().masks.len(), 2);
    for params in [json!({"x": 0.5, "y": 0.5}), json!({"component": null, "x": 0.5, "y": 0.5})] {
        s.execute("mask.sampleColor", &params).unwrap();
        let d = s.develop_of(s.active().unwrap()).unwrap();
        let MaskShape::ColorRange { samples, .. } = &d.masks[1].components[1].shape else { panic!("color range") };
        assert_eq!(samples.len(), 1);
    }
    for params in [json!({"size": 0.04}), json!({"index": null, "size": 0.04})] {
        s.execute("spot.update", &params).unwrap();
        assert_eq!(spots(&s)[1].size, 0.04);
    }
    s.execute("spot.update", &json!({"index": 0, "size": 0.03})).unwrap();
    assert_eq!(spots(&s)[0].size, 0.03);
    assert_eq!(s.active_spot, Some(1));
    rejects_target(&mut s, "mask.component", &json!({"component": null, "action": "delete"}));
    for params in [json!({"id": null}), json!({})] {
        s.execute("mask.select", &json!({"id": 1})).unwrap();
        assert_eq!(s.active_mask, Some(1));
        s.execute("mask.select", &params).unwrap();
        assert_eq!(s.active_mask, None);
    }
    for params in [json!({"index": null}), json!({})] {
        s.execute("spot.select", &json!({"index": 0})).unwrap();
        assert_eq!(s.active_spot, Some(0));
        s.execute("spot.select", &params).unwrap();
        assert_eq!(s.active_spot, None);
    }
    rejects_target(&mut s, "mask.delete", &json!({}));
    rejects_target(&mut s, "spot.delete", &json!({}));
    // An omitted mask still creates a brush mask when no mask is selected.
    s.execute("mask.brushStroke", &json!({"points": [[0.5, 0.5]]})).unwrap();
    assert_eq!(s.develop_of(s.active().unwrap()).unwrap().masks.len(), 3);
}

/// Pasting masks and Remove spots a model made lists what is to be made again on each photo that changed, and the
/// selections arrive without the first photo's picture.
#[test]
fn paste_lists_the_model_work_to_make_again() {
    use lightcraft_develop::{Mask, MaskComponent, MaskShape, SegMask, Spot, SpotFill};
    use lightcraft_geom::Point;
    let mut s = Session::with_demo();
    let ids: Vec<_> = s.visible().iter().copied().take(2).collect();
    let mut d = (*s.develop_of(ids[0]).unwrap()).clone();
    let seg = Some(SegMask::from_logits(4, &[3.0; 16]));
    d.masks.push(Mask {
        id: 3,
        components: vec![MaskComponent {
            name: None,
            op: Default::default(),
            invert: false,
            shape: MaskShape::Object { hint: vec![Point::new(0.5, 0.5)], exclude: vec![], seg, detail: vec![], edge: 0.0 },
        }],
        ..Default::default()
    });
    let fill = SpotFill::from_linear(2, 2, [0.4, 0.4, 0.6, 0.6], &[[0.5; 3]; 4]);
    d.spots.push(Spot { points: vec![Point::new(0.5, 0.5)], fill, ..Default::default() });
    s.set_develop(ids[0], d, "test").unwrap();
    s.execute("library.select", &json!({"ids": [ids[0].0]})).unwrap();
    s.execute("develop.copy", &json!({"groups": ["masks", "spots"]})).unwrap();
    let r = s.execute("develop.paste", &json!({"ids": [ids[1].0]})).unwrap();
    assert_eq!(r["changed"], 1);
    assert_eq!(r["ai"], json!([{"photo": ids[1].0, "masks": [{"mask": 3, "component": 0}], "spots": [0], "expand": false}]));
    let b = s.develop_of(ids[1]).unwrap();
    assert!(
        matches!(&b.masks[0].components[0].shape, MaskShape::Object { seg: None, hint, .. } if hint.len() == 1),
        "the clicks came, the picture did not"
    );
    assert!(b.spots[0].fill.is_none());
    // only the Remove spots (on a photo with nothing yet): the selections are not made again
    s.execute("library.select", &json!({"ids": [ids[1].0]})).unwrap();
    s.execute("develop.reset", &json!({})).unwrap();
    s.execute("library.select", &json!({"ids": [ids[0].0]})).unwrap();
    let r = s.execute("develop.paste", &json!({"ids": [ids[1].0], "groups": ["spots"]})).unwrap();
    assert_eq!(r["ai"], json!([{"photo": ids[1].0, "masks": [], "spots": [0], "expand": false}]));
    // a plain copy has nothing to make again
    s.execute("develop.copy", &json!({"groups": ["light"]})).unwrap();
    s.execute("develop.set", &json!({"values": {"light.exposure": 0.5}})).unwrap();
    s.execute("develop.copy", &json!({"groups": ["light"]})).unwrap();
    let r = s.execute("develop.paste", &json!({"ids": [ids[1].0]})).unwrap();
    assert_eq!(r["ai"], json!([]));
}
