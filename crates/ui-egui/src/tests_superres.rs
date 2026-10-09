//! Photo ▸ Enhance ▸ Super Resolution in the app: the dialog offers the model's download and
//! never starts it without a yes (and never reaches the internet here); with a model in place,
//! Enlarge runs in the background and adds the photo, stacked on the original.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use lightcraft_catalog::PhotoId;
use lightcraft_engine::enhance::{Enhancer, SUPER_RES_MODEL};
use serde_json::json;

use crate::headless::Headless;
use crate::state::Dialog;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);

pub(crate) fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-ui-superres-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// The app with one photo on disk, selected, and the models folder inside `dir`.
pub(crate) fn app(dir: &std::path::Path) -> (Headless, PhotoId) {
    let (w, h) = (60usize, 40usize);
    let rgb: Vec<u8> = (0..w * h).flat_map(|i| [(i % w * 255 / w) as u8, (i / w * 255 / h) as u8, 90]).collect();
    let png = lightcraft_codecs::encode_png(
        &lightcraft_codecs::EncodeImage::new(w as u32, h as u32, 3, lightcraft_codecs::Samples::U8(&rgb)),
        &lightcraft_codecs::EncodeMeta::default(),
    )
    .unwrap();
    let path = dir.join("IMG_7.png");
    std::fs::write(&path, png).unwrap();
    let mut session = lightcraft_engine::Session::new().with_fs();
    let r = session.execute("library.import", &json!({"paths": [path]})).unwrap();
    let id = PhotoId(r["imported"][0].as_u64().unwrap());
    session.selection = lightcraft_engine::Selection::single(id);
    session.enhancer.dir = Some(dir.join("models"));
    // never the real download location
    session.enhancer.no_builtin_mirrors = true;
    let app = LightcraftApp::new(session, Services { png: None, ..Default::default() });
    (Headless::new(app, [1200.0, 800.0], 1.0), id)
}

/// A few frames, so the dialog's window is laid out where its buttons are drawn.
pub(crate) fn settle(h: &mut Headless) {
    for _ in 0..4 {
        h.step();
    }
}

#[test]
fn the_dialog_asks_before_downloading_and_says_why_it_cant() {
    let dir = tmp("ask");
    let (mut h, _) = app(&dir);
    let opened = crate::superres::open(&mut h.app);
    if !Enhancer::AVAILABLE {
        // a build without AI enhancement says so and shows no dialog
        assert!(opened.is_err() && h.app.ui.dialog.is_none());
        return;
    }
    assert!(opened.is_ok());
    assert_eq!(h.app.ui.dialog, Some(Dialog::SuperRes { error: None }));
    settle(&mut h);
    assert!(!h.app.session.enhancer.download_status().1.running, "nothing downloads without a yes");
    // no download location is configured: pressing Download says why, and the dialog stays
    let r = h.request("ui.clickWidget", json!({"id": "button:dialogOk"}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert!(
        matches!(&h.app.ui.dialog, Some(Dialog::SuperRes { error: Some(e) }) if e.contains("LIGHTCRAFT_NOMOS_SPAN_MIRRORS")),
        "{:?}",
        h.app.ui.dialog
    );
    assert!(!h.app.session.enhancer.download_status().1.running);
    settle(&mut h);
    let r = h.request("ui.clickWidget", json!({"id": "button:dialogCancel"}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(h.app.ui.dialog, None);
    // an agent confirming a closed dialog, or running the command with no model: errors at once
    let e = h.app.run("enhance.superRes", json!({})).unwrap_err();
    assert!(e.contains("not installed"), "{e}");
    assert_eq!(h.app.session.catalog.photos().count(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn enlarge_runs_in_the_background_and_adds_the_photo() {
    if !Enhancer::AVAILABLE {
        return;
    }
    let dir = tmp("run");
    let (mut h, id) = app(&dir);
    let file = h.app.session.enhancer.model_file(SUPER_RES_MODEL).unwrap();
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    lightcraft_enhance::testing::write_synthetic_model(&file, 8, 2).unwrap();
    crate::superres::open(&mut h.app).unwrap();
    settle(&mut h);
    // with the model in place the button enlarges, and the dialog closes while the job runs
    let r = h.request("ui.clickWidget", json!({"id": "button:dialogOk"}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(h.app.ui.dialog, None);
    let t = Instant::now();
    while h.app.superres.busy() {
        assert!(t.elapsed() < Duration::from_secs(60), "the job never finished");
        h.step();
        std::thread::sleep(Duration::from_millis(20));
    }
    let new = PhotoId(h.app.superres.last_result.as_ref().and_then(|v| v["id"].as_u64()).expect("a result"));
    let p = h.app.session.catalog.photo(new).unwrap();
    assert_eq!((p.width, p.height), (120, 80));
    assert_eq!(h.app.session.selection.active, Some(new));
    assert!(h.app.session.catalog.stack_of(new).is_some_and(|s| s.photos.contains(&id)));
    assert!(dir.join("IMG_7-SR.tif").exists());
    // nothing running: Cancel says so
    assert!(!crate::superres::cancel(&mut h.app));
    let _ = std::fs::remove_dir_all(&dir);
}
