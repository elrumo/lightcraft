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
fn the_tools_panel_slides_out_and_back_in() {
    let mut h = demo(json!({"view": "detail", "right": "edit"}));
    let full = widget(&h, "panel:right_panel").expect("the tools panel").width();
    set(&mut h, json!({"right": "none"}));
    steps(&mut h, 4);
    let going = widget(&h, "panel:right_panel").map_or(0.0, |r| r.width());
    assert!(going > 1.0 && going < full - 1.0, "on its way out: {going} of {full}");
    h.settle(SETTLE);
    assert!(widget(&h, "panel:right_panel").is_none(), "gone");
    set(&mut h, json!({"right": "edit"}));
    steps(&mut h, 2);
    let coming = widget(&h, "panel:right_panel").map_or(0.0, |r| r.width());
    assert!(coming > 1.0 && coming < full - 1.0, "on its way in: {coming} of {full}");
    h.settle(SETTLE);
    assert_eq!(widget(&h, "panel:right_panel").map(|r| r.width()), Some(full));
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

// TEMP-FRAMES: throwaway frame capture for looking at the transitions; removed before commit
#[test]
#[ignore]
fn temp_frames() {
    let dir = std::env::var("FRAMES_DIR").unwrap();
    let save = |h: &Headless, name: &str| {
        let img = h.paint();
        let rgba: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
        let r = lightcraft_raster::Rgba8::from_bytes(img.size[0], img.size[1], &rgba).unwrap();
        let png = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&r), &lightcraft_codecs::EncodeMeta::default()).unwrap();
        std::fs::write(format!("{dir}/{name}.png"), png).unwrap();
    };
    for (tag, size, view) in [("desk", [1400.0, 900.0], "photoGrid"), ("phone", [390.0, 844.0], "squareGrid")] {
        let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), Services { png: None, ..Default::default() });
        let mut h = Headless::new(app, size, 1.0);
        set(&mut h, json!({"view": view, "right": "none", "leftPanel": tag == "desk"}));
        let r = h.request("engine.execute", json!({"command": "library.select", "params": {"ids": [3]}}), T);
        assert_eq!(r["ok"], true, "{r}");
        h.settle(SETTLE);
        let shoot = |h: &mut Headless, name: &str, frames: usize, every: usize| {
            for f in 0..frames {
                if f % every == 0 {
                    save(h, &format!("{tag}-{name}-{f:02}"));
                }
                h.step();
            }
        };
        set(&mut h, json!({"view": "detail"}));
        shoot(&mut h, "a-open", 16, 3);
        h.settle(SETTLE);
        if tag == "desk" {
            set(&mut h, json!({"right": "edit"}));
            shoot(&mut h, "b-tools", 14, 3);
            h.settle(SETTLE);
            h.request("ui.clickWidget", json!({"id": "section:color"}), T);
            shoot(&mut h, "c-color", 12, 3);
            h.settle(SETTLE);
            set(&mut h, json!({"leftPanel": false, "filmstrip": false}));
            shoot(&mut h, "d-left", 14, 3);
            h.settle(SETTLE);
        }
        set(&mut h, json!({"view": view, "right": "none"}));
        shoot(&mut h, "e-back", 22, 3);
    }
}
