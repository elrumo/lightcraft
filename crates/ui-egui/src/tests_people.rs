//! Finding people in the window: the bar at the top of the People view (ask first, then look), the
//! cards for people found in photos, and naming one. The models are deterministic stand-ins.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use lightcraft_engine::Session;
use lightcraft_vision::fake::FakeFaces;
use serde_json::json;

use crate::headless::Headless;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

fn window(finder: bool) -> Headless {
    let mut session = Session::with_demo();
    if finder {
        session.vision.set_face_finder(Arc::new(FakeFaces));
        session.vision.text_edge = 192;
    } else {
        // (never the user's real model folder)
        session.vision.dir = Some(std::env::temp_dir().join(format!("lc-ui-no-face-models-{}", std::process::id())));
    }
    let app = LightcraftApp::new(session, Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1400.0, 900.0], 1.0);
    h.app.run("view.people", json!({})).unwrap();
    h.settle(SETTLE);
    h
}

fn has(h: &Headless, id: &str) -> bool {
    h.app.widgets.iter().any(|(w, _)| w == id)
}

fn first_with(h: &Headless, prefix: &str) -> Option<String> {
    h.app.widgets.iter().find(|(w, _)| w.starts_with(prefix)).map(|(w, _)| w.clone())
}

fn click(h: &mut Headless, id: &str) {
    let r = h.request("ui.clickWidget", json!({"id": id}), T);
    assert_eq!(r["ok"], true, "{id}: {r}");
    h.step();
}

fn until(h: &mut Headless, what: &str, done: impl Fn(&Headless) -> bool) {
    let end = Instant::now() + SETTLE;
    while !done(h) {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        h.step();
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn nothing_is_looked_at_until_the_user_asks_and_then_people_appear() {
    let mut h = window(true);
    assert!(has(&h, "button:peopleFind"), "the bar offers it");
    assert!(!h.app.session.vision.faces && h.app.session.vision.job().is_none(), "nothing runs before");
    assert!(first_with(&h, "person:found:").is_none());

    click(&mut h, "button:peopleFind");
    assert!(h.app.session.vision.faces, "the models are in place: it starts");
    until(&mut h, "the faces", |h| h.app.session.vision.job().is_some_and(|j| j.finished.load(Ordering::Relaxed)));
    h.settle(Duration::from_secs(5));
    assert_eq!(h.app.session.vision.faces_scanned(), h.app.session.vision_photo_count());
    let card = first_with(&h, "person:found:").expect("people found in the photos have cards");
    let id = card.trim_start_matches("person:found:").to_string();
    let field = format!("field:personName:{id}");
    assert!(has(&h, &field), "an unnamed person has a field for their name");

    // type a name, press Return: the person is named in the catalog
    click(&mut h, &field);
    let r = h.request("ui.text", json!({"text": "Ada Lovelace"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.request("ui.key", json!({"key": "Enter"}), T);
    until(&mut h, "the name", |h| h.app.session.catalog.people().iter().any(|p| p.name == "Ada Lovelace"));
    h.settle(Duration::from_secs(3));
    assert!(has(&h, "person:Ada Lovelace"), "she is a named person now");
    assert!(first_with(&h, &card).is_none(), "and no longer an unnamed one");

    // the bar says what was found, and everything can be forgotten
    assert!(has(&h, "button:peopleForget"));
    click(&mut h, "button:peopleForget");
    assert!(!h.app.session.vision.faces);
    assert_eq!(h.app.session.vision.faces_found(), 0);
    h.settle(Duration::from_secs(3));
    assert!(first_with(&h, "person:found:").is_none());
    assert!(has(&h, "person:Ada Lovelace"), "names written to photos stay");
}

#[test]
fn a_click_on_a_person_shows_their_photos() {
    let mut h = window(true);
    click(&mut h, "button:peopleFind");
    until(&mut h, "the faces", |h| h.app.session.vision.job().is_some_and(|j| j.finished.load(Ordering::Relaxed)));
    h.settle(Duration::from_secs(5));
    let card = first_with(&h, "person:found:").expect("a person");
    click(&mut h, &card);
    h.settle(Duration::from_secs(3));
    assert!(h.app.session.filter.semantic.is_some() && !h.app.session.filter.only.is_empty(), "the grid shows that person");
    assert!(!matches!(h.app.ui.view, crate::state::ViewMode::People));
}

#[test]
fn the_face_models_are_offered_with_their_licences_and_nothing_downloads() {
    let mut h = window(false);
    if !h.app.session.vision.faces_available() {
        // a build without the models' code has no bar
        assert!(!has(&h, "button:peopleFind"));
        return;
    }
    click(&mut h, "button:peopleFind");
    h.step();
    h.step();
    assert!(h.app.ui.faces_offer && !h.app.session.vision.faces, "asked, not turned on yet");
    assert!(has(&h, "button:peopleDownload"));
    assert!(h.app.widgets.iter().filter(|(w, _)| w.starts_with("link:peopleLicense:")).count() >= 2, "both licences are there to read");
    assert_eq!(h.app.session.vision.faces_download_status()["running"], json!(false), "nothing downloads without a click");
    click(&mut h, "button:peopleLater");
    h.step();
    assert!(!h.app.ui.faces_offer && !has(&h, "button:peopleDownload"));
    assert!(has(&h, "button:peopleFind"));
}
