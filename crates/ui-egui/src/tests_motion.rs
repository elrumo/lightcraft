//! Headless tests of the transitions (`motion`): the side panels slide in and out, the filmstrip
//! rises and sinks, Edit sections fold, and a photo opened from the grid grows out of its thumbnail
//! and flies back into it.

use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

fn demo(ui: serde_json::Value) -> Headless {
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1400.0, 900.0], 1.0);
    set(&mut h, ui);
    h.settle(SETTLE);
    h
}

fn set(h: &mut Headless, ui: serde_json::Value) {
    let r = h.request("ui.set", ui, T);
    assert_eq!(r["ok"], true, "{r}");
}

fn widget(h: &Headless, id: &str) -> Option<egui::Rect> {
    h.app.widgets.iter().find(|(w, _)| w == id).map(|(_, r)| *r)
}

/// A few frames into a transition (a fifth of a second takes 12).
fn steps(h: &mut Headless, n: usize) {
    for _ in 0..n {
        h.step();
    }
}

#[test]
fn the_tools_panel_slides_in_and_back_out() {
    // shut from the start, the Presets panel too (where it was can't be restored: it opens afresh)
    let mut h = demo(json!({"view": "detail", "right": "none", "presets": false, "leftPanel": true}));
    assert!(widget(&h, "panel:right_panel").is_none());
    set(&mut h, json!({"right": "edit"}));
    steps(&mut h, 2);
    let full = crate::state::RIGHT_WIDTH.default;
    let coming = widget(&h, "panel:right_panel").map_or(0.0, |r| r.width());
    assert!(coming > 1.0 && coming < full - 1.0, "on its way in: {coming} of {full}");
    h.settle(SETTLE);
    // at its own width, not half the room
    assert_eq!(widget(&h, "panel:right_panel").map(|r| r.width()), Some(full));
    set(&mut h, json!({"right": "none"}));
    steps(&mut h, 4);
    let going = widget(&h, "panel:right_panel").map_or(0.0, |r| r.width());
    assert!(going > 1.0 && going < full - 1.0, "on its way out: {going} of {full}");
    h.settle(SETTLE);
    assert!(widget(&h, "panel:right_panel").is_none(), "gone");
}

#[test]
fn the_filmstrip_rises_and_sinks() {
    let mut h = demo(json!({"view": "detail", "right": "none", "filmstrip": true}));
    let with = h.app.canvas_rect.expect("the loupe").height();
    set(&mut h, json!({"filmstrip": false}));
    steps(&mut h, 4);
    let sinking = h.app.canvas_rect.unwrap().height();
    h.settle(SETTLE);
    let without = h.app.canvas_rect.unwrap().height();
    assert!(sinking > with + 1.0 && sinking < without - 1.0, "the photo gets the room bit by bit: {with} → {sinking} → {without}");
}

#[test]
fn an_edit_section_folds_open() {
    let mut h = demo(json!({"view": "detail", "right": "edit", "openSections": ["light"]}));
    let shut = widget(&h, "section:effects").expect("the Effects header").top();
    let r = h.request("ui.clickWidget", json!({"id": "section:color"}), T);
    assert_eq!(r["ok"], true, "{r}");
    steps(&mut h, 3);
    let opening = widget(&h, "section:effects").unwrap().top();
    h.settle(SETTLE);
    let open = widget(&h, "section:effects").unwrap().top();
    assert!(h.app.ui.section_open("color"));
    assert!(opening > shut + 1.0 && opening < open - 1.0, "Effects moves down as Color opens: {shut} → {opening} → {open}");
}

#[test]
fn a_photo_opened_from_the_grid_grows_out_of_its_thumbnail_and_flies_back() {
    let mut h = demo(json!({"view": "photoGrid", "right": "none"}));
    let id = *h.app.session.visible_cloned().first().expect("demo photos");
    let r = h.request("engine.execute", json!({"command": "library.select", "params": {"ids": [id.0]}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    let tile = widget(&h, &format!("thumb:{}", id.0)).expect("its cell");
    set(&mut h, json!({"view": "detail"}));
    let first = h.app.image_rect.expect("the photo");
    h.settle(SETTLE);
    let end = h.app.image_rect.unwrap();
    assert!(first.width() < end.width() * 0.8, "starts small: {first:?} → {end:?}");
    assert!(tile.contains(first.center()), "where its thumbnail was: {tile:?}, {first:?}");
    // back to the grid: it flies home, and its cell waits for it
    set(&mut h, json!({"view": "photoGrid"}));
    let flying = crate::panels::detail::homing(&h.view.ctx).expect("on its way back");
    assert_eq!(flying.photo, id.0);
    h.settle(SETTLE);
    assert!(crate::panels::detail::homing(&h.view.ctx).is_none(), "landed");
}
