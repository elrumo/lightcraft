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
