//! Remove ▸ Generative AI in the app: without the model the switch offers it (never downloading without
//! a yes, never reaching the internet here) and strokes are filled from around them; the browser
//! build's fetch hands the model over; with the model a stroke gets the model's fill.

use std::time::{Duration, Instant};

use lightcraft_engine::enhance::Enhancer;
use serde_json::json;

use crate::Services;
use crate::headless::Headless;
use crate::tests_superres::{app, settle, tmp};

const T: Duration = Duration::from_secs(20);

/// The Remove tool open on the photo with Generative AI on.
fn remove_tool(h: &mut Headless) {
    h.app.ui.view = crate::state::ViewMode::Detail;
    h.app.run("panel.remove", json!({})).unwrap();
    h.app.ui.remove_ai = true;
    settle(h);
}

/// A stroke across the middle of the photo.
fn stroke(h: &mut Headless) {
    let r = h.request(
        "ui.pointer",
        json!({"events": [{"kind": "down", "x": 0.4, "y": 0.5}, {"kind": "drag", "x": 0.5, "y": 0.5}, {"kind": "up", "x": 0.5, "y": 0.5}]}),
        T,
    );
    assert_eq!(r["ok"], true, "{r}");
}

fn spots(h: &Headless) -> Vec<lightcraft_develop::Spot> {
    let id = h.app.session.active().unwrap();
    h.app.session.develop_of(id).unwrap().spots.clone()
}

#[test]
fn without_the_model_the_switch_offers_it_and_strokes_fill_from_around_them() {
    let dir = tmp("erase-offer");
    let (mut h, _) = app(&dir);
    remove_tool(&mut h);
    let has = |h: &Headless, id: &str| h.app.widgets.iter().any(|(w, _)| w == id);
    assert!(has(&h, "check:removeAi"));
    assert_eq!(has(&h, "button:removeAiDownload"), Enhancer::AVAILABLE, "the offer, where this build can download");
    if Enhancer::AVAILABLE {
        // no download location is configured here: pressing Download says why and starts nothing
        let r = h.request("ui.clickWidget", json!({"id": "button:removeAiDownload"}), T);
        assert_eq!(r["ok"], true, "{r}");
        assert!(!h.app.session.enhancer.download_status().1.running);
    }
    // a stroke without the model: filled from around it, and a toast says how to get AI
    stroke(&mut h);
    settle(&mut h);
    let s = spots(&h);
    assert_eq!(s.len(), 1);
    assert!(s[0].fill.is_none());
    assert!(h.app.ui.toast.as_ref().is_some_and(|(t, _, _)| t.contains("model")), "{:?}", h.app.ui.toast);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The browser build's fetch: the model isn't stored, so the switch offers the download; what the
/// download brings is checked before it is used.
#[test]
fn the_browser_fetch_is_asked_first_then_offers_the_download() {
    let dir = tmp("erase-web");
    let (mut h, _) = app(&dir);
    let asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = asked.clone();
    h.app.services = Services {
        erase_model: Some(Box::new(move |download, tx, _ctx| {
            log.lock().unwrap().push(download);
            let _ = tx.send(if download { Ok(b"not the model".to_vec()) } else { Err(crate::erase::NOT_STORED.into()) });
        })),
        ..Default::default()
    };
    remove_tool(&mut h);
    assert_eq!(*asked.lock().unwrap(), [false], "storage is looked in once, nothing is downloaded");
    assert!(h.app.widgets.iter().any(|(w, _)| w == "button:removeAiDownload"));
    let r = h.request("ui.clickWidget", json!({"id": "button:removeAiDownload"}), T);
    assert_eq!(r["ok"], true, "{r}");
    settle(&mut h);
    assert_eq!(*asked.lock().unwrap(), [false, true]);
    assert!(!h.app.session.eraser_ready(), "a damaged download is refused");
    let _ = std::fs::remove_dir_all(&dir);
}

/// With the model (when a copy is at `$LIGHTCRAFT_MIGAN`; it is never part of the repository) a
/// stroke shows "Removing with AI…" and then carries the model's fill.
#[test]
fn with_the_model_a_stroke_gets_the_models_fill() {
    let Some(path) = std::env::var_os("LIGHTCRAFT_MIGAN") else {
        lightcraft_testkit::skip!(Model, "LIGHTCRAFT_MIGAN names no MI-GAN file");
        return;
    };
    let dir = tmp("erase-run");
    let (mut h, _) = app(&dir);
    h.app.session.install_eraser(&std::fs::read(path).unwrap()).unwrap();
    remove_tool(&mut h);
    assert!(!h.app.widgets.iter().any(|(w, _)| w == "button:removeAiDownload"), "nothing to offer");
    stroke(&mut h);
    let t = Instant::now();
    // (the notice, then the stroke at once, then the fill from the worker)
    while crate::erase::busy(&h.app) {
        assert!(t.elapsed() < Duration::from_secs(60), "the fill never ran");
        h.step();
    }
    assert_eq!(spots(&h).len(), 1, "the stroke is there before its fill");
    while h.app.session.erase_running() > 0 || spots(&h).iter().all(|s| s.fill.is_none()) {
        assert!(t.elapsed() < Duration::from_secs(60), "the fill never arrived");
        h.step();
        std::thread::sleep(Duration::from_millis(5));
    }
    let s = spots(&h);
    assert_eq!(s.len(), 1);
    assert!(s[0].fill.is_some(), "the model's fill is stored with the spot");
    let _ = std::fs::remove_dir_all(&dir);
}

/// High quality is offered where the device runs LaMa; on, without the model, it offers the download and strokes use the
/// fast model.
#[test]
fn high_quality_is_offered_where_the_device_runs_lama() {
    use lightcraft_engine::erase::RemoveQuality;
    use lightcraft_engine::hardware::{DeviceClass, DeviceProfile, GB};
    let dir = tmp("erase-hq");
    let (mut h, _) = app(&dir);
    let has = |h: &Headless, id: &str| h.app.widgets.iter().any(|(w, _)| w == id);
    h.app.session.set_device(DeviceProfile { class: DeviceClass::Phone, memory_bytes: Some(GB), gpu: false, cores: 4 });
    remove_tool(&mut h);
    assert!(has(&h, "check:removeAi") && !has(&h, "check:removeAiHq"), "a phone with 1 GB doesn't run LaMa");
    h.app.session.set_device(DeviceProfile { class: DeviceClass::Computer, memory_bytes: Some(16 * GB), gpu: false, cores: 8 });
    settle(&mut h);
    assert!(has(&h, "check:removeAiHq"));
    assert!(!has(&h, "button:removeAiHqDownload"), "the offer shows once it is on");
    let r = h.request("ui.clickWidget", json!({"id": "check:removeAiHq"}), T);
    assert_eq!(r["ok"], true, "{r}");
    settle(&mut h);
    assert!(h.app.ui.remove_hq);
    assert_eq!(has(&h, "button:removeAiHqDownload"), Enhancer::AVAILABLE, "the offer, where this build can download");
    assert_eq!(crate::erase::quality(&h.app), RemoveQuality::Fast, "until LaMa is here strokes use the fast model");
    assert_eq!(crate::erase::ai_params(RemoveQuality::High)["quality"], "high");
    let _ = std::fs::remove_dir_all(&dir);
}
