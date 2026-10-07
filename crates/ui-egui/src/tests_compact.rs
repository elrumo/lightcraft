//! Headless tests of the compact (phone) layout: it replaces the desktop panels below
//! `COMPACT_BELOW_PT`, shows the tools as a bottom tab bar and the active tool as a sheet.

use std::time::Duration;

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
    h.settle(SETTLE);
    h
}

fn has(h: &Headless, id: &str) -> bool {
    h.app.widgets.iter().any(|(w, _)| w == id)
}

#[test]
fn phone_width_uses_tab_bar_and_sheet_desktop_width_does_not() {
    let h = detail([390.0, 844.0]);
    assert!(h.app.compact);
    assert!(has(&h, "icon:masking"), "tab bar tools are drawn");
    assert!(!has(&h, "icon:presets"), "the desktop tool strip is not");
    assert!(!has(&h, "histogram"), "the sheet leaves the histogram out");
    let h = detail([1200.0, 800.0]);
    assert!(!h.app.compact);
    assert!(has(&h, "icon:presets"));
}

#[test]
fn tapping_the_open_tool_closes_the_sheet() {
    let mut h = detail([390.0, 844.0]);
    let r = h.request("ui.clickWidget", json!({"id": "icon:edit"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert_eq!(h.app.ui.right, RightPanel::None);
}

#[test]
fn swiping_a_fitted_photo_changes_photo_and_a_tap_does_not_zoom() {
    let mut h = detail([390.0, 844.0]);
    let first_in_view = h.app.session.visible()[0];
    let r = h.request("engine.execute", json!({"command": "library.select", "params": {"ids": [first_in_view.0]}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    let first = h.app.session.active();
    assert_eq!(first, Some(first_in_view));
    // a tap on the fitted photo leaves it fitted (desktop would zoom to 2:1)
    let r = h.request("ui.pointer", json!({"events": [{"kind": "down", "x": 0.5, "y": 0.5}, {"kind": "up", "x": 0.5, "y": 0.5}]}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert_eq!(h.app.ui.zoom, crate::state::Zoom::Fit);
    // a long sideways drag goes to the next photo
    let ev: Vec<_> = [("down", 0.9), ("drag", 0.5), ("drag", 0.0), ("drag", -0.5), ("up", -0.5)]
        .iter()
        .map(|(k, x)| json!({"kind": k, "x": x, "y": 0.5}))
        .collect();
    let r = h.request("ui.pointer", json!({"events": ev}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert_ne!(h.app.session.active(), first, "swipe left shows the next photo");
}
