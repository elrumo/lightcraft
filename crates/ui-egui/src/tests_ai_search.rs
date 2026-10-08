//! Search by description in the window: the Describe switch, the indexing it starts, Return, and
//! the panel under the search field. The model is a deterministic stand-in.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use lightcraft_engine::Session;
use lightcraft_vision::fake::FakeEmbedder;
use serde_json::json;

use crate::headless::Headless;
use crate::state::ViewMode;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

fn window(model: bool) -> Headless {
    let mut session = Session::with_demo();
    if model {
        session.vision.set_embedder(Arc::new(FakeEmbedder));
    } else {
        // (never the user's real model folder)
        session.vision.dir = Some(std::env::temp_dir().join(format!("lc-ui-no-search-model-{}", std::process::id())));
    }
    let mut app = LightcraftApp::new(session, Services { png: None, ..Default::default() });
    app.ui.view = ViewMode::PhotoGrid;
    let mut h = Headless::new(app, [1400.0, 900.0], 1.0);
    h.settle(SETTLE);
    h
}

fn has(h: &Headless, id: &str) -> bool {
    h.app.widgets.iter().any(|(w, _)| w == id)
}

fn click(h: &mut Headless, id: &str) {
    let r = h.request("ui.clickWidget", json!({"id": id}), T);
    assert_eq!(r["ok"], true, "{id}: {r}");
    h.step();
}

/// Frames until `done`.
fn until(h: &mut Headless, what: &str, done: impl Fn(&Headless) -> bool) {
    let end = Instant::now() + SETTLE;
    while !done(h) {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        h.step();
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn indexed(h: &Headless) -> bool {
    h.app.session.vision.job().is_some_and(|j| j.finished.load(Ordering::Relaxed))
}

#[test]
fn the_switch_is_there_when_search_by_description_is_available() {
    let h = window(true);
    assert!(has(&h, "button:describeSearch"));
    assert!(!h.app.ui.ai_search, "off until the user turns it on");
    assert!(!has(&h, "progress:describeIndex"), "nothing runs before");
    assert!(h.app.session.vision.job().is_none());
}

#[test]
fn turning_it_on_indexes_and_return_searches() {
    let mut h = window(true);
    let all = h.app.session.visible_cloned().len();
    click(&mut h, "button:describeSearch");
    assert!(h.app.ui.ai_search);
    until(&mut h, "the index", indexed);
    h.settle(Duration::from_secs(5));
    assert_eq!(h.app.session.vision.indexed(), h.app.session.vision_photo_count());

    // typing alone doesn't search; Return does
    click(&mut h, "field:search");
    let r = h.request("ui.text", json!({"text": "blue"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    assert!(h.app.session.filter.semantic.is_none() && h.app.session.filter.text.is_empty(), "no search while typing");
    h.request("ui.key", json!({"key": "Enter"}), T);
    until(&mut h, "the search", |h| h.app.session.filter.semantic.is_some());
    assert_eq!(h.app.session.filter.semantic.as_deref(), Some("blue"));
    // best match first, and a chip says what it is
    let shown = h.app.session.visible_cloned();
    assert_eq!(shown, h.app.session.filter.only);
    assert!(shown.len() <= all && !shown.is_empty());
    let chips = lightcraft_engine::filter_chips(&h.app.session.filter, &h.app.session.catalog);
    assert_eq!(chips.iter().map(|c| c.label.as_str()).collect::<Vec<_>>(), ["Looks like: blue"]);

    // emptying the field leaves the results
    h.request("ui.key", json!({"key": "A", "cmd": true}), T);
    h.request("ui.key", json!({"key": "Backspace"}), T);
    h.settle(Duration::from_secs(5));
    assert!(h.app.ui.search.is_empty());
    assert_eq!(h.app.session.filter.semantic, None);
    assert_eq!(h.app.session.visible_cloned().len(), all);

    // turning the switch off after a search clears it too
    h.request("ui.text", json!({"text": "red"}), T);
    h.request("ui.key", json!({"key": "Enter"}), T);
    until(&mut h, "the second search", |h| h.app.session.filter.semantic.is_some());
    click(&mut h, "button:describeSearch");
    assert!(!h.app.ui.ai_search);
    assert_eq!(h.app.session.filter.semantic, None);
    assert!(h.app.ui.search.is_empty());
    assert_eq!(h.app.session.visible_cloned().len(), all);
}

#[test]
fn without_the_model_the_panel_asks_and_nothing_downloads() {
    let mut h = window(false);
    if !h.app.session.vision.available() {
        // a build without the feature has no switch
        assert!(!has(&h, "button:describeSearch"));
        return;
    }
    click(&mut h, "button:describeSearch");
    h.step();
    assert!(h.app.ui.ai_search);
    assert!(has(&h, "link:describeLicense"), "the licence is shown first");
    // (the real download button is never clicked here: it would fetch 1.5 GB)
    assert!(has(&h, "button:describeDownload") || h.app.session.vision.mirrors().is_empty());
    assert_eq!(h.app.session.vision.download_status()["running"], json!(false), "nothing downloads without a click");
    assert!(!h.app.session.vision.installed());
    // searching says why it can't, in the panel, instead of failing silently
    click(&mut h, "field:search");
    h.request("ui.text", json!({"text": "a dog"}), T);
    h.request("ui.key", json!({"key": "Enter"}), T);
    h.settle(Duration::from_secs(5));
    assert!(h.app.ui.ai_search_error.as_deref().is_some_and(|e| e.contains("not installed")), "{:?}", h.app.ui.ai_search_error);
    h.step();
    assert!(has(&h, "button:describeDismiss"));
    click(&mut h, "button:describeDismiss");
    assert_eq!(h.app.ui.ai_search_error, None);
    assert!(h.app.session.filter.semantic.is_none());
}

#[test]
fn an_error_shows_under_the_field_and_goes_away() {
    let mut h = window(true);
    click(&mut h, "button:describeSearch");
    until(&mut h, "the index", indexed);
    h.app.ui.ai_search_error = Some("something went wrong".into());
    h.step();
    h.step();
    assert!(has(&h, "button:describeDismiss"));
    click(&mut h, "button:describeDismiss");
    assert_eq!(h.app.ui.ai_search_error, None);
}

#[test]
fn a_device_without_the_model_searches_through_a_server_that_can() {
    let mut h = window(false);
    h.app.session.vision.set_server_status(Some(json!({"available": true, "installed": true, "indexed": 2, "total": 4})));
    h.step();
    // the switch is there because the server can search; turning it on offers no download
    assert!(has(&h, "button:describeSearch"));
    click(&mut h, "button:describeSearch");
    h.step();
    assert!(h.app.ui.ai_search);
    assert!(!has(&h, "button:describeDownload") && !has(&h, "link:describeLicense"), "the server has the model: nothing to install here");
    // it says how far the server has got
    assert!(
        has(&h, "panel:describe:serverIndexing"),
        "{:?}",
        h.app.widgets.iter().filter(|(w, _)| w.starts_with("panel:describe")).collect::<Vec<_>>()
    );
    // and once the server has everything, only the hint is left
    h.app.session.vision.set_server_status(Some(json!({"available": true, "installed": true, "indexed": 4, "total": 4})));
    h.step();
    h.step();
    assert!(has(&h, "panel:describe:hint"));
    assert!(!h.app.session.vision.installed());
}

#[test]
fn a_server_whose_model_is_not_installed_says_so() {
    let mut h = window(false);
    h.app.session.vision.set_server_status(Some(json!({"available": true, "installed": false, "indexed": 0, "total": 4})));
    h.step();
    click(&mut h, "button:describeSearch");
    h.step();
    if h.app.session.vision.local_available() {
        // this build could run the model itself: it offers that (not the server's)
        assert!(has(&h, "panel:describe:install"));
    } else {
        assert!(has(&h, "panel:describe:serverNoModel"));
        assert!(!has(&h, "button:describeDownload"), "nothing to download on a device that can't run the model");
    }
}

#[test]
fn a_server_that_cannot_search_adds_no_switch() {
    let mut h = window(false);
    h.app.session.vision.set_server_status(Some(json!({"available": false})));
    h.step();
    assert_eq!(has(&h, "button:describeSearch"), h.app.session.vision.local_available());
}

#[test]
fn the_choice_to_send_search_data_to_the_server_is_a_checkbox_where_it_applies() {
    let mut h = window(true);
    h.app.session.vision.set_server_status(Some(json!({"available": true, "installed": true, "indexed": 4, "total": 4})));
    click(&mut h, "button:describeSearch");
    until(&mut h, "the index", indexed);
    h.settle(Duration::from_secs(5));
    assert!(has(&h, "check:describeShare"), "this device has the model and the server can search");
    assert!(!h.app.session.vision.share_with_server, "off until the user turns it on");
    click(&mut h, "check:describeShare");
    assert!(h.app.session.vision.share_with_server);
    click(&mut h, "check:describeShare");
    assert!(!h.app.session.vision.share_with_server);
}
