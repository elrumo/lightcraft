//! Settings ▸ AI Models: the models in one place; downloads and deletes ask first; turning a model
//! off is remembered and stops its feature. No test touches the internet.

use std::time::{Duration, Instant};

use lightcraft_engine::enhance::{Enhancer, SUPER_RES_MODEL};
use serde_json::{Value, json};

use crate::headless::Headless;
use crate::state::{Dialog, ModelStep};
use crate::tests_superres::{app, settle, tmp};

const T: Duration = Duration::from_secs(20);

fn open_tab(h: &mut Headless) {
    h.app.ui.dialog = Some(Dialog::Settings { tab: "models".into() });
    settle(h);
}

fn click(h: &mut Headless, id: &str) -> Value {
    h.request("ui.clickWidget", json!({"id": id}), T)
}

fn has(h: &mut Headless, id: &str) -> bool {
    h.request("ui.clickWidget", json!({"id": id, "dryRun": true}), T)["ok"] == true || {
        let r = h.request("ui.widgets", json!({}), T);
        r["result"].as_array().is_some_and(|w| w.iter().any(|x| x["id"] == id))
    }
}

fn model(h: &Headless, id: &str) -> lightcraft_engine::models::ModelInfo {
    h.app.session.models().into_iter().find(|m| m.id == id).unwrap_or_else(|| panic!("no model {id}"))
}

#[test]
fn the_tab_lists_the_models_and_asks_before_downloading_or_deleting() {
    use std::io::{BufRead, BufReader, Write};
    if !Enhancer::AVAILABLE {
        // a build without AI models says so
        let dir = tmp("none");
        let (mut h, _) = app(&dir);
        open_tab(&mut h);
        assert!(h.app.session.models().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }
    // a local mirror with nothing on it
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/nomos", l.local_addr().unwrap());
    std::thread::spawn(move || {
        for mut c in l.incoming().flatten() {
            let mut r = BufReader::new(c.try_clone().unwrap());
            let mut line = String::new();
            while r.read_line(&mut line).unwrap_or(0) > 2 {
                line.clear();
            }
            let _ = c.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 13\r\nConnection: close\r\n\r\n<h1>404</h1>\n");
        }
    });
    let dir = tmp("tab");
    std::fs::create_dir_all(dir.join("models")).unwrap();
    std::fs::write(dir.join("models").join(format!("{SUPER_RES_MODEL}-mirrors.txt")), format!("{base}\n")).unwrap();
    let (mut h, _) = app(&dir);
    open_tab(&mut h);
    let id = SUPER_RES_MODEL;

    // not downloaded: the card says so, and where it would come from
    assert!(has(&mut h, &format!("label:modelStatus-{id}")) && has(&mut h, &format!("label:modelSource-{id}")));
    assert!(!model(&h, id).installed && model(&h, id).sources.len() == 1);

    // Download… only asks: nothing starts until the card's own Download is pressed
    assert_eq!(click(&mut h, &format!("button:modelDownload-{id}"))["ok"], true);
    assert_eq!(h.app.ui.model_confirm, Some((id.to_string(), ModelStep::Download)));
    settle(&mut h);
    assert!(model(&h, id).download.is_none(), "nothing downloads without a yes");
    // cancelling the question starts nothing either
    assert_eq!(click(&mut h, &format!("button:modelStepCancel-{id}"))["ok"], true);
    assert_eq!(h.app.ui.model_confirm, None);
    settle(&mut h);

    // ask again and say yes: it starts (here it fails: the mirror has nothing) and says why
    click(&mut h, &format!("button:modelDownload-{id}"));
    settle(&mut h);
    assert_eq!(click(&mut h, &format!("button:modelDownloadConfirm-{id}"))["ok"], true);
    assert_eq!(h.app.ui.model_confirm, None);
    let t = Instant::now();
    while model(&h, id).download.as_ref().is_none_or(|d| d.running) {
        assert!(t.elapsed() < Duration::from_secs(30), "the download never ended");
        h.step();
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(model(&h, id).download.and_then(|d| d.error).unwrap_or_default().contains("not found"));
    assert!(!model(&h, id).installed);

    // with the model in place: on this device, its size, and deleting asks first
    let file = h.app.session.enhancer.model_file(id).unwrap();
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    lightcraft_enhance::testing::write_synthetic_model(&file, 8, 2).unwrap();
    settle(&mut h);
    assert!(model(&h, id).installed && model(&h, id).disk_bytes > 0);
    assert_eq!(click(&mut h, &format!("button:modelDelete-{id}"))["ok"], true);
    assert_eq!(h.app.ui.model_confirm, Some((id.to_string(), ModelStep::Delete)));
    settle(&mut h);
    assert!(file.exists(), "nothing is deleted without a yes");
    assert_eq!(click(&mut h, &format!("button:modelDeleteConfirm-{id}"))["ok"], true);
    settle(&mut h);
    assert!(!file.exists() && !model(&h, id).installed && model(&h, id).disk_bytes == 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn turning_a_model_off_is_saved_with_the_settings_and_stops_its_feature() {
    if !Enhancer::AVAILABLE {
        return;
    }
    let dir = tmp("toggle");
    let (mut h, photo) = app(&dir);
    let id = SUPER_RES_MODEL;
    let file = h.app.session.enhancer.model_file(id).unwrap();
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    lightcraft_enhance::testing::write_synthetic_model(&file, 8, 2).unwrap();
    open_tab(&mut h);
    assert_eq!(h.app.ui.settings.models_disabled, Vec::<String>::new());
    assert_eq!(click(&mut h, &format!("check:modelEnabled-{id}"))["ok"], true);
    settle(&mut h);
    // the session knows, and the saved settings follow
    assert_eq!(h.app.session.models_disabled(), vec![id.to_string()]);
    assert_eq!(h.app.ui.settings.models_disabled, vec![id.to_string()]);
    assert!(!model(&h, id).enabled && model(&h, id).installed);
    let e = h.app.run("enhance.superRes", json!({"id": photo.0})).unwrap_err();
    assert!(e.contains("turned off"), "{e}");
    // the menu command says so at once, instead of opening a dialog that can't enlarge
    let e = crate::superres::open(&mut h.app).unwrap_err();
    assert!(e.contains("turned off") && e.contains("Settings"), "{e}");
    assert_eq!(h.app.ui.dialog, Some(Dialog::Settings { tab: "models".into() }), "the open Settings stays as it is");
    // on again
    click(&mut h, &format!("check:modelEnabled-{id}"));
    settle(&mut h);
    assert!(h.app.session.models_disabled().is_empty() && h.app.ui.settings.models_disabled.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

/// The saved choice (loaded after the app is built) is applied on the first frame.
#[test]
fn a_saved_choice_is_applied_on_the_first_frame() {
    let dir = tmp("saved");
    let (mut h, _) = app(&dir);
    h.app.ui.settings.models_disabled = vec![SUPER_RES_MODEL.to_string(), "from-the-future".to_string()];
    h.step();
    if Enhancer::AVAILABLE {
        assert_eq!(h.app.session.models_disabled(), vec![SUPER_RES_MODEL.to_string()]);
        // ids this version doesn't know are dropped from the saved settings once the session's
        // choice is what counts
        h.step();
        assert_eq!(h.app.ui.settings.models_disabled, vec![SUPER_RES_MODEL.to_string()]);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn sizes_read_naturally() {
    use crate::panels::ai_models::size;
    assert_eq!(size(0), "0 KB");
    assert_eq!(size(1), "1 KB");
    assert_eq!(size(230_000), "230 KB");
    assert_eq!(size(4_461_056), "4.5 MB");
    assert_eq!(size(3_439_938_512), "3.44 GB");
}
