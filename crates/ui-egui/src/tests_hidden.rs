//! The Hidden album in the window: Hide from the menu takes a photo out of the grid, the Hidden row asks
//! for a password (choosing one the first time), a wrong one keeps the dialog open, and Show Hidden mixes
//! hidden photos back in after the password. Possibly sensitive photos are drawn obscured while Blur is on.

use std::sync::Arc;
use std::time::Duration;

use lightcraft_engine::{LibrarySource, Session};
use lightcraft_raster::Rgba8;
use lightcraft_vision::{Classifier, Error};
use serde_json::json;

use crate::headless::Headless;
use crate::state::{Dialog, HiddenPrompt};
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

/// Flags every photo as possibly sensitive.
struct FlagAll;

impl Classifier for FlagAll {
    fn engine(&self) -> &str {
        "test-all"
    }
    fn score(&self, _: &Rgba8) -> Result<f32, Error> {
        Ok(0.9)
    }
}

/// Flags the photos whose corner is bright (some of the demo photos, so a grid shows both kinds).
struct Bright;

impl Classifier for Bright {
    fn engine(&self) -> &str {
        "test-bright"
    }
    fn score(&self, img: &Rgba8) -> Result<f32, Error> {
        let p = img.data.first().copied().unwrap_or([0; 4]);
        Ok(if u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2]) > 3 * 128 { 0.95 } else { 0.05 })
    }
}

fn window(session: Session) -> Headless {
    window_with(session, Services { png: None, ..Default::default() })
}

fn window_with(session: Session, services: Services) -> Headless {
    let mut app = LightcraftApp::new(session, services);
    // (the albums sidebar, where the Hidden and Possibly Sensitive rows are)
    app.ui.left_panel = true;
    let mut h = Headless::new(app, [1400.0, 900.0], 1.0);
    // (the app opens on the loupe; these are grid screens)
    h.app.run("view.photoGrid", json!({})).unwrap();
    h.settle(SETTLE);
    h
}

/// How many grid cells are drawn obscured.
fn blurred(h: &Headless) -> usize {
    h.app.widgets.iter().filter(|(w, _)| w.starts_with("blurred:")).count()
}

fn has(h: &Headless, id: &str) -> bool {
    h.app.widgets.iter().any(|(w, _)| w == id)
}

fn click(h: &mut Headless, id: &str) {
    let r = h.request("ui.clickWidget", json!({"id": id}), T);
    assert_eq!(r["ok"], true, "{id}: {r}");
    h.settle(SETTLE);
}

fn type_in(h: &mut Headless, field: &str, text: &str) {
    click(h, field);
    h.request("ui.text", json!({"text": text}), T);
    h.step();
}

#[test]
fn a_hidden_photo_needs_the_password_to_be_seen_again() {
    let mut h = window(Session::with_demo());
    let total = h.app.session.visible().len();
    let id = h.app.session.visible()[0];
    h.app.run("library.select", json!({"ids": [id.0]})).unwrap();
    h.app.run("photo.hide", json!({})).unwrap();
    h.settle(SETTLE);
    assert_eq!(h.app.session.visible().len(), total - 1);

    // the first time: choose a password (twice), then the album opens
    click(&mut h, "source:hidden");
    assert!(matches!(h.app.ui.dialog, Some(Dialog::HiddenPassword { mode: HiddenPrompt::Create, show: false, .. })), "{:?}", h.app.ui.dialog);
    type_in(&mut h, "field:hiddenPassword", "open sesame");
    type_in(&mut h, "field:hiddenConfirm", "open sesame");
    click(&mut h, "button:dialogOk");
    assert!(h.app.ui.dialog.is_none());
    assert_eq!(h.app.session.source, LibrarySource::Hidden);
    assert_eq!(h.app.session.visible(), [id]);

    // back to All Photos: locked again; Show Hidden asks, and a wrong password keeps the dialog open
    h.app.run("library.source", json!({"kind": "all"})).unwrap();
    assert!(!h.app.session.hidden.unlocked());
    crate::panels::dialogs::open_hidden(&mut h.app, true).unwrap();
    h.settle(SETTLE);
    assert!(matches!(h.app.ui.dialog, Some(Dialog::HiddenPassword { mode: HiddenPrompt::Unlock, show: true, .. })));
    type_in(&mut h, "field:hiddenPassword", "wrong");
    click(&mut h, "button:dialogOk");
    assert!(h.app.ui.dialog.is_some(), "a wrong password: still asking");
    assert!(!h.app.session.hidden.showing());
    if let Some(Dialog::HiddenPassword { password, .. }) = h.app.ui.dialog.as_mut() {
        password.clear();
    }
    type_in(&mut h, "field:hiddenPassword", "open sesame");
    click(&mut h, "button:dialogOk");
    assert!(h.app.session.hidden.showing());
    assert_eq!(h.app.session.visible().len(), total);
    // the password never reaches the saved window state
    let saved = serde_json::to_string(&h.app.ui).unwrap();
    assert!(!saved.contains("open sesame"));
}

#[test]
fn possibly_sensitive_photos_are_listed_and_drawn_obscured() {
    let mut session = Session::with_demo();
    session.vision.set_classifier(Arc::new(FlagAll));
    session.vision.sensitive = true;
    session.vision.text_edge = 192;
    session.vision_index(None, true).unwrap();
    let flagged = session.sensitive_photos().len();
    assert!(flagged > 0);
    let mut h = window(session);
    assert!(has(&h, "source:sensitive"));
    // (once the thumbnails are drawn)
    assert!(h.step_until(SETTLE, |h| blurred(h) > 0), "a flagged photo in the grid is obscured");
    h.app.run("library.setBlurSensitive", json!({"on": false})).unwrap();
    assert!(h.step_until(SETTLE, |h| blurred(h) == 0), "and shown as it is with Blur off");
    click(&mut h, "source:sensitive");
    assert_eq!(h.app.session.visible().len(), flagged);
    click(&mut h, "button:hideSensitive");
    assert!(h.app.session.visible().is_empty(), "Hide All empties the list");
}

/// Screenshots of the Hidden and sensitive-content screens for a person to look at (`LIGHTCRAFT_SHOTS` = a
/// folder; skipped without it).
#[test]
fn screenshots_of_hidden_and_sensitive_screens() {
    let Some(dir) = std::env::var_os("LIGHTCRAFT_SHOTS").map(std::path::PathBuf::from) else {
        lightcraft_testkit::skip!(Input, "LIGHTCRAFT_SHOTS not set (a folder for the screenshots)");
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    let shot = |h: &mut Headless, name: &str| {
        h.settle(SETTLE);
        let r = h.request("ui.screenshot", json!({"path": dir.join(name).to_string_lossy()}), T);
        assert_eq!(r["ok"], true, "{name}: {r}");
    };
    let mut session = Session::with_demo();
    session.vision.set_classifier(Arc::new(Bright));
    session.vision.sensitive = true;
    session.vision.text_edge = 192;
    // (screenshots are PNG files)
    let png = |img: &Rgba8| {
        lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(img), &lightcraft_codecs::EncodeMeta::default()).unwrap_or_default()
    };
    let services = Services {
        png: Some(Box::new(png)),
        write: Some(Box::new(|p: &str, b: &[u8]| std::fs::write(p, b).map_err(|e| e.to_string()))),
        ..Default::default()
    };
    let mut h = window_with(session, services);
    let ids: Vec<u64> = h.app.session.visible().iter().take(3).map(|id| id.0).collect();
    h.app.run("photo.hide", json!({"ids": ids})).unwrap();
    // (the app checks the photos itself once the model is there: wait for its pass and the thumbnails)
    assert!(h.step_until(SETTLE, |h| blurred(h) > 0), "some photos are drawn obscured");
    shot(&mut h, "1-grid-locked.png");
    crate::panels::dialogs::open_hidden(&mut h.app, false).unwrap();
    shot(&mut h, "2-create-password.png");
    h.app.ui.dialog = None;
    h.app.run("hidden.setPassword", json!({"password": "1234"})).unwrap();
    h.app.run("library.source", json!({"kind": "hidden"})).unwrap();
    shot(&mut h, "3-hidden-album.png");
    h.app.run("library.source", json!({"kind": "all"})).unwrap();
    crate::panels::dialogs::open_hidden(&mut h.app, true).unwrap();
    shot(&mut h, "4-unlock.png");
    h.app.ui.dialog = None;
    h.app.run("hidden.unlock", json!({"password": "1234"})).unwrap();
    h.app.run("library.showHidden", json!({"on": true})).unwrap();
    h.app.ui.filter_bar = true;
    shot(&mut h, "5-show-hidden.png");
    h.app.run("library.showHidden", json!({"on": false})).unwrap();
    h.app.ui.filter_bar = false;
    h.app.run("library.source", json!({"kind": "sensitive"})).unwrap();
    shot(&mut h, "6-possibly-sensitive.png");
}
