//! Generative Expand in the app: the crop tool's Expand switch, dragging the frame past the photo,
//! the choice of what paints the new area (with its download offer, never downloading without a
//! yes), and Generate running in the background into one undo step. Classic needs no model, so
//! this runs everywhere.

use std::time::{Duration, Instant};

use lightcraft_engine::Session;
use serde_json::json;

use crate::headless::Headless;
use crate::state::{RightPanel, ViewMode};
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(30);
const SETTLE: Duration = Duration::from_secs(120);

fn detail(size: [f32; 2]) -> Headless {
    let app = LightcraftApp::new(Session::with_demo(), Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, size, 1.0);
    h.settle(SETTLE);
    let r = h.request("engine.execute", json!({"command": "library.select", "params": {"ids": [1]}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.app.ui.view = ViewMode::Detail;
    h.app.ui.right = RightPanel::Edit;
    // never the real download location
    h.app.session.enhancer.no_builtin_mirrors = true;
    h.settle(SETTLE);
    h
}

fn has(h: &Headless, id: &str) -> bool {
    h.app.widgets.iter().any(|(w, _)| w == id)
}

fn rect(h: &Headless, id: &str) -> egui::Rect {
    h.app.widgets.iter().find(|(w, _)| w == id).map(|(_, r)| *r).unwrap_or_else(|| panic!("{id} is not on screen"))
}

fn click(h: &mut Headless, id: &str) {
    let r = h.request("ui.clickWidget", json!({"id": id}), T);
    assert_eq!(r["ok"], true, "{id}: {r}");
    h.settle(SETTLE);
}

fn settings(h: &Headless) -> lightcraft_develop::DevelopSettings {
    let id = h.app.session.active().unwrap();
    (*h.app.session.develop_of(id).unwrap()).clone()
}

/// Run frames until the background Generate has stored its fill (or failed).
fn finish_generate(h: &mut Headless) {
    let t = Instant::now();
    while h.app.session.expand_running().is_some() {
        assert!(t.elapsed() < Duration::from_secs(120), "Generate never finished");
        h.step();
        std::thread::sleep(Duration::from_millis(5));
    }
    h.settle(SETTLE);
}

#[test]
fn desktop_expand_drags_past_the_photo_and_generate_fills_it() {
    let mut h = detail([1400.0, 900.0]);
    h.app.run("panel.crop", json!({})).unwrap();
    h.settle(SETTLE);
    assert!(has(&h, "check:cropExpand"));
    assert!(!has(&h, "button:cropGenerate"), "Generate shows with Expand on");
    click(&mut h, "check:cropExpand");
    assert!(settings(&h).crop.expand);
    for w in ["button:expandModel-migan", "button:expandModel-lama", "button:expandModel-classic", "button:cropGenerate"] {
        assert!(has(&h, w), "{w}");
    }
    // the default model isn't on this device: its download is offered (and only on a yes)
    if lightcraft_engine::enhance::Enhancer::AVAILABLE {
        assert!(has(&h, "button:expandDownload"), "MI-GAN's offer");
        click(&mut h, "button:expandDownload");
        assert!(!h.app.session.enhancer.download_status().1.running, "no download location is configured here");
    }
    // the crop tool shows room around the photo (the photo is the middle of the view): the left
    // handle drags past the photo's edge
    let left = rect(&h, "cropHandle:7").center();
    let img = h.app.image_rect.unwrap();
    // (a quarter of the photo's width each side: the photo's left edge is a sixth into the view)
    assert!((left.x - (img.left() + img.width() / 6.0)).abs() < 2.0, "{left:?} in {img:?}");
    let nx = (left.x - img.left()) / img.width();
    let ny = (left.y - img.top()) / img.height();
    let r = h.request(
        "ui.pointer",
        json!({"events": [{"kind": "down", "x": nx, "y": ny}, {"kind": "drag", "x": nx - 0.01, "y": ny}, {"kind": "drag", "x": nx - 0.04, "y": ny}]}),
        T,
    );
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    let r = h.request("ui.pointer", json!({"events": [{"kind": "up", "x": nx - 0.04, "y": ny}]}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    let x0 = settings(&h).crop.geometry.rect.x0;
    assert!(x0 < -0.02, "the crop reaches past the photo: x0 = {x0}");
    let steps = h.app.session.undo.len();
    // Classic paints without a download
    click(&mut h, "button:expandModel-classic");
    assert_eq!(h.app.ui.expand_model, "classic");
    click(&mut h, "button:cropGenerate");
    finish_generate(&mut h);
    let d = settings(&h);
    assert_eq!(d.expand_fill.as_ref().map(|f| f.model.as_str()), Some("classic"), "status {}", h.app.ui.status);
    assert_eq!(h.app.session.undo.len(), steps + 1, "one undo step");
    assert!(h.app.ui.toast.as_ref().is_some_and(|(t, _, _)| t.contains("Classic")), "{:?}", h.app.ui.toast);
    // turning Expand off takes the crop back inside the photo
    click(&mut h, "check:cropExpand");
    let d = settings(&h);
    assert!(!d.crop.expand && d.expand_fill.is_none() && d.crop.geometry.rect.x0 >= 0.0);
}

#[test]
fn phone_expand_tile_generate_and_fill_with() {
    let mut h = detail([390.0, 844.0]);
    click(&mut h, "icon:crop");
    assert!(has(&h, "button:cropExpand"), "the Aspect tab starts with Expand");
    assert!(!has(&h, "button:cropGenerate") && !has(&h, "button:cropFillWith"));
    click(&mut h, "button:cropExpand");
    assert!(settings(&h).crop.expand);
    assert!(has(&h, "button:cropGenerate") && has(&h, "button:cropFillWith"));
    // the bar names what paints; a tap chooses it
    click(&mut h, "button:cropFillWith");
    for m in ["migan", "classic"] {
        assert!(has(&h, &format!("button:cropFillWith-{m}")), "{m} is offered on a phone");
    }
    click(&mut h, "button:cropFillWith-classic");
    assert_eq!(h.app.ui.expand_model, "classic");
    // a square from a landscape photo: canvas above and below the whole photo
    click(&mut h, "button:cropRatios");
    click(&mut h, "button:cropAspect-1x1");
    let d = settings(&h);
    assert!(d.crop.geometry.rect.y0 < 0.0 && d.crop.geometry.rect.y1 > 1.0, "{:?}", d.crop.geometry);
    click(&mut h, "button:cropGenerate");
    finish_generate(&mut h);
    assert_eq!(settings(&h).expand_fill.map(|f| f.model), Some("classic".to_string()));
    // ✕ undoes everything the screen did, the fill too
    click(&mut h, "icon:cropCancel");
    let d = settings(&h);
    assert!(!d.crop.expand && d.expand_fill.is_none());
}

/// LaMa is a computer's: a phone doesn't list it (the engine says it isn't available there).
#[test]
fn the_models_follow_the_device() {
    let h = detail([1200.0, 800.0]);
    let ids: Vec<&str> = lightcraft_engine::expand::ExpandModel::ALL.into_iter().filter(|m| m.available()).map(|m| m.id()).collect();
    if cfg!(any(target_os = "ios", target_os = "android")) {
        assert_eq!(ids, ["migan", "classic"]);
    } else {
        assert_eq!(ids, ["migan", "lama", "classic"]);
    }
    // with nothing downloaded the picker offers MI-GAN (agents get Classic: the best one ready)
    assert_eq!(crate::expand::chosen(&h.app), lightcraft_engine::expand::ExpandModel::MiGan);
    assert_eq!(h.app.session.default_expand_model(), lightcraft_engine::expand::ExpandModel::Classic);
}
