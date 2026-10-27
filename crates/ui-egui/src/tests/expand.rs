//! Generative Expand in the app: the crop tool's Expand switch, dragging the frame past the photo,
//! the choice of what paints the new area (with its download offer, never downloading without a
//! yes), and Generate running in the background into one undo step. Classic needs no model, so
//! this runs everywhere.

use std::time::{Duration, Instant};

use lightcraft_engine::Session;
use lightcraft_engine::expand::ExpandModel;
use lightcraft_engine::hardware::{DeviceClass, DeviceProfile, GB};
use serde_json::json;

use crate::headless::Headless;
use crate::state::{RightPanel, ViewMode};
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(30);
const SETTLE: Duration = Duration::from_secs(120);

/// A computer with 16 GB and no GPU (the tests don't depend on the machine).
const COMPUTER: DeviceProfile = DeviceProfile { class: DeviceClass::Computer, memory_bytes: Some(16 * GB), gpu: false, cores: 8 };
/// A recent phone: a GPU, 5 GB the app may use.
const PHONE: DeviceProfile = DeviceProfile { class: DeviceClass::Phone, memory_bytes: Some(5 * GB), gpu: true, cores: 6 };

fn detail(size: [f32; 2]) -> Headless {
    detail_on(size, COMPUTER)
}

fn detail_on(size: [f32; 2], device: DeviceProfile) -> Headless {
    let mut session = Session::with_demo();
    session.set_device(device);
    let app = LightcraftApp::new(session, Services { png: None, ..Default::default() });
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
    for w in ["button:expandModel-auto", "button:expandModel-migan", "button:expandModel-lama", "button:expandModel-classic", "button:cropGenerate"] {
        assert!(has(&h, w), "{w}");
    }
    // this computer runs Stable Diffusion 1.5 and Kandinsky, not SDXL (no GPU)
    if lightcraft_engine::enhance::Enhancer::AVAILABLE {
        assert!(has(&h, "button:expandModel-sd15-inpaint") && has(&h, "button:expandModel-kandinsky22-inpaint"));
        assert!(!has(&h, "button:expandModel-sdxl-inpaint"));
    }
    // Automatic's model (LaMa here) isn't on this device: its download is offered (and only on a yes)
    assert_eq!(crate::expand::chosen(&h.app), ExpandModel::Lama);
    if lightcraft_engine::enhance::Enhancer::AVAILABLE {
        assert!(has(&h, "button:expandDownload"), "LaMa's offer");
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
    assert_eq!(h.app.session.expand.preferred, Some(ExpandModel::Classic), "the session has the choice too (agents, Settings)");
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
    let mut h = detail_on([390.0, 844.0], PHONE);
    click(&mut h, "icon:crop");
    assert!(has(&h, "button:cropExpand"), "the Aspect tab starts with Expand");
    assert!(!has(&h, "button:cropGenerate") && !has(&h, "button:cropFillWith"));
    click(&mut h, "button:cropExpand");
    assert!(settings(&h).crop.expand);
    assert!(has(&h, "button:cropGenerate") && has(&h, "button:cropFillWith"));
    // the bar names what paints; a tap chooses it
    click(&mut h, "button:cropFillWith");
    // a recent phone gets the larger models too: LaMa, and Stable Diffusion 1.5 (its default)
    for m in ["auto", "migan", "lama", "classic"] {
        assert!(has(&h, &format!("button:cropFillWith-{m}")), "{m} is offered on this phone");
    }
    assert!(!has(&h, "button:cropFillWith-sdxl-inpaint") && !has(&h, "button:cropFillWith-kandinsky22-inpaint"), "computers only");
    if lightcraft_engine::enhance::Enhancer::AVAILABLE {
        assert!(has(&h, "button:cropFillWith-sd15-inpaint"));
        assert_eq!(crate::expand::chosen(&h.app), ExpandModel::Sd15);
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

/// The pickers list what this device runs; Automatic picks the best of it.
#[test]
fn the_models_follow_the_device() {
    let small = DeviceProfile { class: DeviceClass::Phone, memory_bytes: Some(GB), gpu: false, cores: 4 };
    let h = detail_on([1200.0, 800.0], small);
    assert_eq!(crate::expand::offered(&h.app), [ExpandModel::MiGan, ExpandModel::Classic]);
    assert_eq!(crate::expand::chosen(&h.app), ExpandModel::MiGan);
    // (agents get Classic while nothing is downloaded: the best one ready)
    assert_eq!(h.app.session.default_expand_model(), ExpandModel::Classic);
    let h = detail([1200.0, 800.0]);
    let ids: Vec<&str> = crate::expand::offered(&h.app).into_iter().map(|m| m.id()).collect();
    if lightcraft_engine::enhance::Enhancer::AVAILABLE {
        assert_eq!(ids, ["migan", "lama", "sd15-inpaint", "kandinsky22-inpaint", "classic"]);
    } else {
        assert_eq!(ids, ["migan", "lama", "classic"]);
    }
}

/// Settings → AI Models: the same choice as the crop tool's, with Automatic; it is saved.
#[test]
fn settings_choose_the_generative_expand_model() {
    let mut h = detail_on([1200.0, 1800.0], COMPUTER);
    h.app.ui.dialog = Some(crate::state::Dialog::Settings { tab: "models".into() });
    h.settle(SETTLE);
    for w in ["radio:expandModel-auto", "radio:expandModel-migan", "radio:expandModel-lama", "radio:expandModel-classic"] {
        assert!(has(&h, w), "{w}");
    }
    assert!(!has(&h, "radio:expandModel-sdxl-inpaint"), "SDXL doesn't run on this computer");
    let pick = if lightcraft_engine::enhance::Enhancer::AVAILABLE { "sd15-inpaint" } else { "migan" };
    click(&mut h, &format!("radio:expandModel-{pick}"));
    assert_eq!(h.app.session.expand.preferred.map(|m| m.id()), Some(pick));
    assert_eq!(h.app.ui.expand_model, pick);
    if lightcraft_engine::enhance::Enhancer::AVAILABLE {
        // its card is in the list, with what its licence asks
        assert!(has(&h, "label:modelStatus-sd15-inpaint") && has(&h, "label:modelStatus-sdxl-inpaint"));
    }
    click(&mut h, "radio:expandModel-auto");
    assert_eq!(h.app.session.expand.preferred, None);
    assert_eq!(h.app.ui.expand_model, "");
    // an agent's choice reaches the saved settings
    h.app.session.set_expand_preference(Some(ExpandModel::Classic)).unwrap();
    h.settle(SETTLE);
    assert_eq!(h.app.ui.expand_model, "classic");
}

/// The saved choice is applied when the app starts (the settings load after it is built).
#[test]
fn the_saved_choice_is_applied_at_start() {
    let mut session = Session::with_demo();
    session.set_device(COMPUTER);
    let mut app = LightcraftApp::new(session, Services { png: None, ..Default::default() });
    app.ui.expand_model = "classic".into();
    let mut h = Headless::new(app, [1200.0, 800.0], 1.0);
    h.settle(SETTLE);
    assert_eq!(h.app.session.expand.preferred, Some(ExpandModel::Classic));
    // a saved model this device doesn't run is dropped: Automatic
    let mut session = Session::with_demo();
    session.set_device(DeviceProfile { class: DeviceClass::Phone, memory_bytes: Some(GB), gpu: false, cores: 4 });
    let mut app = LightcraftApp::new(session, Services { png: None, ..Default::default() });
    app.ui.expand_model = "sdxl-inpaint".into();
    let mut h = Headless::new(app, [390.0, 844.0], 1.0);
    h.settle(SETTLE);
    assert_eq!(h.app.session.expand.preferred, None);
    assert_eq!(h.app.ui.expand_model, "");
}
