//! Tests of the phone layout's iOS 26 look: the springs, the select bar's glass capsule, and the
//! screen stepping back behind a page.

use std::time::Duration;

use egui::Color32;
use lightcraft_engine::Session;
use serde_json::json;

use crate::glass;
use crate::headless::Headless;
use crate::state::ViewMode;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(30);
const SETTLE: Duration = Duration::from_secs(120);

fn phone(size: [f32; 2]) -> Headless {
    let app = LightcraftApp::new(Session::with_demo(), Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, size, 1.0);
    h.settle(SETTLE);
    h
}

fn run(h: &mut Headless, command: &str, params: serde_json::Value) {
    let r = h.request("engine.execute", json!({"command": command, "params": params}), T);
    assert_eq!(r["ok"], true, "{command}: {r}");
    h.settle(SETTLE);
}

fn widget(h: &Headless, id: &str) -> egui::Rect {
    h.app.widgets.iter().find(|(w, _)| w == id).map(|(_, r)| *r).unwrap_or_else(|| panic!("no {id} on screen"))
}

/// The bounce curve starts at 0, ends at exactly 1 and overshoots a little on the way; a spring
/// settles on its target (a lively one after going past it), and on a new target from there.
#[test]
fn springs_settle_and_bounce() {
    assert_eq!(glass::bounce(0.0), 0.0);
    assert!((glass::bounce(1.0) - 1.0).abs() < 1e-6);
    let peak = (0..=100).map(|i| glass::bounce(i as f32 / 100.0)).fold(0.0_f32, f32::max);
    assert!(peak > 1.01 && peak < 1.15, "a small overshoot: {peak}");
    let ctx = egui::Context::default();
    let id = egui::Id::new("test-spring");
    assert_eq!(glass::spring(&ctx, id, 5.0, 0.4, 0.6), 5.0, "starts at its first target");
    let mut most: f32 = 0.0;
    let mut x = 0.0;
    for _ in 0..240 {
        x = glass::spring(&ctx, id, 10.0, 0.4, 0.6);
        most = most.max(x);
    }
    assert_eq!(x, 10.0, "settled");
    assert!(most > 10.0, "went past the target first: {most}");
    // a broken state (NaN) starts over at the target rather than spreading
    ctx.data_mut(|d| d.insert_temp(id, (f32::NAN, 0.0_f32)));
    assert_eq!(glass::spring(&ctx, id, 3.0, 0.4, 0.6), 3.0);
}

/// Choosing photos, their actions sit side by side in the floating glass capsule at the bottom
/// (laid out top to bottom, all but the first fell off the screen).
#[test]
fn the_select_bar_lays_its_actions_out_in_a_row() {
    let mut h = phone([390.0, 844.0]);
    h.app.ui.view = ViewMode::PhotoGrid;
    run(&mut h, "view.selectMode", json!({"on": true}));
    run(&mut h, "library.selectAll", json!({}));
    let (rate, delete) = (widget(&h, "icon:selRate"), widget(&h, "icon:selDelete"));
    assert!(rate.height() > 40.0 && delete.height() > 40.0, "{rate:?} {delete:?}");
    assert_eq!(rate.top(), delete.top(), "one row");
    assert!(rate.right() <= delete.left() && delete.right() <= 390.0, "left to right, on screen: {rate:?} {delete:?}");
}

/// A page (Settings) pushes the screen behind it back, as an iPhone shows a sheet: black shows
/// round the shrunken screen, where the light top bar was; closing the page brings it back.
#[test]
fn a_page_pushes_the_screen_behind_back() {
    let mut h = phone([390.0, 844.0]);
    run(&mut h, "app.appearance.light", json!({}));
    let corner = |h: &mut Headless| {
        let img = h.paint();
        img.pixels[2 * img.size[0] + 2]
    };
    assert_eq!(corner(&mut h), Color32::WHITE, "the light top bar");
    run(&mut h, "app.settings", json!({}));
    assert_eq!(corner(&mut h), Color32::BLACK, "behind the page, the screen stepped back");
    let r = h.request("ui.dialog.cancel", json!({}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    h.settle(SETTLE);
    assert_eq!(corner(&mut h), Color32::WHITE, "back in place");
}

/// The same on a phone with a status bar: the black reaches over the status bar's strip (a layer's
/// painter stops at the safe area), and the top of the screen behind shows above the page.
#[test]
fn a_page_pushes_the_screen_back_under_the_status_bar() {
    let mut h = phone([402.0, 874.0]);
    h.safe_area = Some(egui::SafeAreaInsets(egui::epaint::MarginF32 { left: 0.0, right: 0.0, top: 62.0, bottom: 34.0 }));
    run(&mut h, "app.appearance.light", json!({}));
    run(&mut h, "app.settings", json!({}));
    let img = h.paint();
    let px = |x: usize, y: usize| img.pixels[y * img.size[0] + x];
    assert_eq!(px(2, 2), Color32::BLACK, "black round the screen, over the status bar");
    let strip = px(img.size[0] / 2, 64);
    assert!(strip.r() > 100, "the screen's top shows between the status bar and the page: {strip:?}");
}

/// A window too small for any of it (the springs, the glass, a page stepping the screen back)
/// draws without a panic.
#[test]
fn a_tiny_window_draws_the_glass_without_a_panic() {
    let mut h = phone([40.0, 60.0]);
    h.app.ui.view = ViewMode::Detail;
    h.settle(SETTLE);
    run(&mut h, "app.settings", json!({}));
    let _ = h.paint();
}

/// Esc closes a photo's "…" menu and does nothing else: the shortcut behind it (Back to Grid) used
/// to fire too. Same for a page (Settings) open over the photo.
#[test]
fn escape_closes_a_menu_and_stays_on_the_photo() {
    let mut h = phone([390.0, 844.0]);
    run(&mut h, "library.select", json!({"ids": [1]}));
    h.app.ui.view = ViewMode::Detail;
    h.app.ui.right = crate::state::RightPanel::None;
    h.settle(SETTLE);
    let r = h.request("ui.clickWidget", json!({"id": "icon:photoMore"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert!(crate::panels::mobile::actions_open(&h.view.ctx, "photoMore"));
    let r = h.request("ui.key", json!({"key": "Escape"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert!(!crate::panels::mobile::actions_open(&h.view.ctx, "photoMore"), "closed");
    assert_eq!(h.app.ui.view, ViewMode::Detail, "still on the photo");
    run(&mut h, "app.settings", json!({}));
    let r = h.request("ui.key", json!({"key": "Escape"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert!(h.app.ui.dialog.is_none(), "the page closed");
    assert_eq!(h.app.ui.view, ViewMode::Detail, "and the photo stayed");
}

/// A page is opaque while it slides up (egui fades a new area in by default, which let the grid
/// show through the Albums page as it came up).
#[test]
fn a_page_slides_up_opaque() {
    let mut h = phone([390.0, 844.0]);
    run(&mut h, "app.appearance.light", json!({}));
    h.app.ui.view = ViewMode::PhotoGrid;
    h.settle(SETTLE);
    h.app.ui.left_panel = true;
    for _ in 0..4 {
        h.step();
    }
    let img = h.paint();
    // the page's left margin, a little above the bottom of the screen
    let px = img.pixels[800 * img.size[0] + 4];
    assert_eq!(px, crate::theme::Tokens::ios_for(false).chrome, "the page's own colour, not the grid through it");
}

/// The tool bar floats: its capsule stops short of the bottom of the safe area, and the sheet's
/// colour carries on under it to the bottom of the screen (it used to sit on the safe area's edge,
/// with the window's colour below).
#[test]
fn the_tool_bar_floats_over_the_sheets_colour() {
    let mut h = phone([402.0, 874.0]);
    h.set_safe_area(62.0, 34.0);
    run(&mut h, "app.appearance.light", json!({}));
    run(&mut h, "library.select", json!({"ids": [1]}));
    h.app.ui.view = ViewMode::Detail;
    h.app.ui.right = crate::state::RightPanel::Edit;
    h.settle(SETTLE);
    h.settle(SETTLE);
    let edit = widget(&h, "icon:edit");
    assert!(edit.bottom() <= 874.0 - 34.0 - 8.0, "a gap under the capsule: {edit:?}");
    let img = h.paint();
    let chrome = crate::theme::Tokens::ios_for(false).chrome;
    // (beside the capsule, clear of its shadow, and at the bottom of the screen)
    for (x, y) in [(4, (edit.bottom() + 4.0) as usize), (200, img.size[1] - 4)] {
        assert_eq!(img.pixels[y * img.size[0] + x], chrome, "the sheet's colour under the bar at {x}, {y}");
    }
}
