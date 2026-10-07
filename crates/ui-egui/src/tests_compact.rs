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
    assert!(!has(&h, "icon:keywords"), "the desktop tool strip is not");
    assert!(!has(&h, "histogram"), "the sheet leaves the histogram out");
    let h = detail([1200.0, 800.0]);
    assert!(!h.app.compact);
    assert!(has(&h, "icon:keywords"));
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

#[test]
fn compact_sliders_take_a_touch_well_above_the_track() {
    let mut h = detail([390.0, 1500.0]);
    let id = h.app.session.active().expect("active photo");
    let before = format!("{:?}", h.app.session.develop_of(id));
    let track = h.app.widgets.iter().find(|(w, _)| w == "slider:light.exposure").map(|(_, r)| *r).expect("exposure slider in the sheet");
    // 18 pt above the track centre: outside a desktop slider's hit area, inside the touch one
    let (x, y) = (track.left() + track.width() * 0.8, track.center().y - 18.0);
    let r = h.request("ui.click", json!({"x": x, "y": y}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert_ne!(format!("{:?}", h.app.session.develop_of(id)), before, "the tap moved the slider");
}

#[test]
fn compact_layout_raises_egui_rows_to_touch_size_and_desktop_restores_them() {
    let mut h = detail([390.0, 844.0]);
    assert!(h.view.ctx.global_style().spacing.interact_size.y >= crate::TOUCH_ROW_H);
    // growing to a desktop window puts the original size back
    let r = h.request("ui.resize", json!({"width": 1200, "height": 800}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert!(!h.app.compact);
    assert!(h.view.ctx.global_style().spacing.interact_size.y < crate::TOUCH_ROW_H);
}

#[test]
fn presets_are_a_tab_and_the_sidebar_is_a_page_that_closes_on_choosing() {
    let mut h = detail([390.0, 844.0]);
    // presets: one sheet at a time with the tools
    let r = h.request("ui.clickWidget", json!({"id": "icon:presets"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert!(h.app.ui.presets);
    let r = h.request("ui.clickWidget", json!({"id": "icon:crop"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert!(!h.app.ui.presets);
    assert_eq!(h.app.ui.right, RightPanel::Crop);
    // the sidebar: a page; picking a source closes it
    h.app.ui.view = ViewMode::PhotoGrid;
    h.app.ui.left_panel = true;
    h.settle(SETTLE);
    assert!(has(&h, "source:picks"), "the sidebar page is drawn");
    let r = h.request("ui.clickWidget", json!({"id": "source:picks"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert!(!h.app.ui.left_panel);
    assert_eq!(h.app.session.source, lightcraft_engine::LibrarySource::Picks);
}

#[test]
fn compact_crop_handles_are_grabbed_from_a_finger_width_away() {
    let mut h = detail([390.0, 1000.0]);
    h.app.ui.right = RightPanel::Crop;
    h.settle(SETTLE);
    let id = h.app.session.active().expect("active photo");
    let corner = h.app.widgets.iter().find(|(w, _)| w == "cropHandle:0").map(|(_, r)| r.center()).expect("top-left handle");
    // 25 pt inside the corner: a desktop press there moves the whole box, a finger takes the corner
    let from = corner + egui::vec2(18.0, 18.0);
    let r = h.request("ui.drag", json!({"x": from.x, "y": from.y, "toX": from.x + 40.0, "toY": from.y + 40.0, "steps": 6}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    let rect = h.app.session.develop_of(id).expect("settings").crop.geometry.rect;
    assert!(rect.x0 > 0.05 && rect.y0 > 0.02, "the corner moved in: {rect:?}");
    assert!((rect.x1 - 1.0).abs() < 1e-6 && (rect.y1 - 1.0).abs() < 1e-6, "the opposite corner stayed: {rect:?}");
}

#[test]
fn ipad_portrait_is_compact_with_the_tools_on_the_right() {
    let h = detail([820.0, 1180.0]);
    assert!(h.app.compact, "820 pt is below the desktop layout's width");
    let img = h.app.image_rect.expect("loupe drawn");
    assert!(img.right() < 820.0 - 300.0, "the tool panel sits beside the photo, not below it: {img:?}");
    assert!(img.width() > 400.0, "and it fills the room beside it: {img:?}");
}

#[test]
fn modal_dialogs_fit_a_phone_screen() {
    let mut h = detail([390.0, 844.0]);
    let r = h.request("engine.execute", json!({"command": "dialog.export"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    let rect = h.view.ctx.memory(|m| m.area_rect(egui::Id::new("lightcraft-dialog"))).expect("the export dialog is open");
    assert!(rect.width() <= 390.0 && rect.height() <= 844.0, "the dialog fits the screen: {rect:?}");
    assert!(rect.left() >= 0.0 && rect.top() >= 0.0, "and starts on it: {rect:?}");
}
