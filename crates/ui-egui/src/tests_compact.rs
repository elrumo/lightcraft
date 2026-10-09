//! Headless tests of the compact (phone) layout: it replaces the desktop panels below
//! `COMPACT_BELOW_PT`, shows the tools as a bottom tab bar and the active tool as a sheet.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use lightcraft_engine::Session;
use serde_json::json;

use crate::haptics::Haptic;
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

/// Run `n` frames: the headless clock moves 1/60 s a frame, so animations and the double-tap wait
/// play out (`settle` stops after twelve quiet ones).
fn frames(h: &mut Headless, n: u32) {
    for _ in 0..n {
        h.step();
    }
}

/// Where a widget is drawn.
fn rect_of(h: &Headless, id: &str) -> egui::Rect {
    h.app.widgets.iter().find(|(w, _)| w == id).map(|(_, r)| *r).unwrap_or_else(|| panic!("{id} is not on screen"))
}

/// One-finger events in the photo's normalized coordinates, one a frame: `("down" | "drag" | "up", x, y)`.
fn pointer(h: &mut Headless, events: &[(&str, f32, f32)]) {
    let ev: Vec<_> = events.iter().map(|(k, x, y)| json!({"kind": k, "x": x, "y": y})).collect();
    let r = h.request("ui.pointer", json!({"events": ev}), T);
    assert_eq!(r["ok"], true, "{r}");
}

/// A phone with the first photo of the view open (a next one, no previous) and no tool in hand.
fn first_photo(size: [f32; 2]) -> Headless {
    let mut h = detail(size);
    h.app.ui.right = RightPanel::None;
    let first = h.app.session.visible()[0];
    let r = h.request("engine.execute", json!({"command": "library.select", "params": {"ids": [first.0]}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert_eq!(h.app.session.active(), Some(first));
    h
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
    frames(&mut h, 40);
    assert_eq!(h.app.ui.zoom, crate::state::Zoom::Fit);
    assert!(h.app.ui.review, "a single tap hides the bars instead");
    let r = h.request("engine.execute", json!({"command": "view.reviewMode", "params": {"on": false}}), T);
    assert_eq!(r["ok"], true, "{r}");
    frames(&mut h, 40);
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

/// A finger drags a slider from well above its track, by how far it moves (not to where it is);
/// a tap leaves the value alone.
#[test]
fn compact_sliders_follow_a_drag_from_above_the_track_and_ignore_taps() {
    let mut h = detail([390.0, 1500.0]);
    let id = h.app.session.active().expect("active photo");
    let before = format!("{:?}", h.app.session.develop_of(id));
    let track = h.app.widgets.iter().find(|(w, _)| w == "slider:light.exposure").map(|(_, r)| *r).expect("exposure slider in the sheet");
    // 16 pt above the track centre: outside a desktop slider's hit area, inside the touch one
    let (x, y) = (track.left() + track.width() * 0.8, track.center().y - 16.0);
    let r = h.request("ui.click", json!({"x": x, "y": y}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert_eq!(format!("{:?}", h.app.session.develop_of(id)), before, "a tap doesn't move a touch slider");
    let r = h.request("ui.drag", json!({"x": x, "y": y, "toX": x + track.width() * 0.1, "toY": y}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    let exposure = h.app.session.develop_of(id).map(|d| d.light.exposure).unwrap_or_default();
    // a tenth of the track is a tenth of the range (−5…+5 EV), from 0, not the 0.8 the finger is at
    assert!(exposure > 0.5 && exposure < 1.5, "exposure {exposure}");
}

/// A drag that sets off up or down on a slider scrolls the sheet; the slider keeps its value.
#[test]
fn compact_sliders_let_a_vertical_drag_scroll_the_sheet() {
    let mut h = detail([390.0, 844.0]);
    let id = h.app.session.active().expect("active photo");
    let before = format!("{:?}", h.app.session.develop_of(id));
    let slider =
        |h: &Headless| h.app.widgets.iter().find(|(w, _)| w == "slider:light.contrast").map(|(_, r)| *r).expect("contrast slider in the sheet");
    let track = slider(&h);
    let (x, y) = (track.center().x, track.center().y - 10.0);
    let r = h.request("ui.drag", json!({"x": x, "y": y, "toX": x + 4.0, "toY": y - 120.0, "steps": 20}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert_eq!(format!("{:?}", h.app.session.develop_of(id)), before, "the slider kept its value");
    assert!(slider(&h).top() < track.top() - 40.0, "the sheet scrolled: {:?} → {:?}", track, slider(&h));
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

/// A phone shows dialogs as pages covering the screen, with Cancel and the action in a bar.
#[test]
fn dialogs_are_pages_on_a_phone() {
    let mut h = detail([390.0, 844.0]);
    let r = h.request("engine.execute", json!({"command": "dialog.export"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    let rect = h.app.widgets.iter().find(|(w, _)| w == "sheet:dialog").map(|(_, r)| *r).expect("the export dialog is a page");
    assert!(rect.width() >= 389.0 && rect.left() >= 0.0 && rect.top() >= 0.0 && rect.bottom() <= 844.5, "it covers the screen: {rect:?}");
    assert!(has(&h, "button:sheetOk") && has(&h, "button:sheetCancel"));
    click(&mut h, "button:sheetCancel");
    assert!(h.app.ui.dialog.is_none(), "Cancel closes it");
}

/// On a phone a dialog's on / off choice is an iOS switch spanning the row: a tap anywhere on it toggles.
#[test]
fn phone_dialogs_use_switches_for_on_off_choices() {
    let mut h = detail([390.0, 844.0]);
    let r = h.request("engine.execute", json!({"command": "dialog.export"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    let row = h.app.widgets.iter().find(|(w, _)| w == "check:exportDontEnlarge").map(|(_, r)| *r).expect("Don't enlarge");
    assert!(row.width() > 300.0 && row.height() >= 44.0, "a full-width row: {row:?}");
    let enlarge = |h: &Headless| match &h.app.ui.dialog {
        Some(crate::state::Dialog::Export { resize, .. }) => resize.dont_enlarge,
        _ => panic!("export dialog"),
    };
    let before = enlarge(&h);
    // the far right of the row, on the switch
    let r = h.request("ui.click", json!({"x": row.right() - 20.0, "y": row.center().y}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert_ne!(enlarge(&h), before, "the switch toggled");
}

/// On a phone every preset is shown on the photo, in finger-sized rows.
#[test]
fn phone_presets_show_the_photo_with_each_preset() {
    let mut h = detail([390.0, 844.0]);
    let r = h.request("engine.execute", json!({"command": "panel.presets"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    let row = h.app.widgets.iter().find(|(w, _)| w.starts_with("preset:")).map(|(_, r)| *r).expect("a preset row");
    assert!(row.height() >= 60.0, "{row:?}");
    assert!(h.app.renderer.variant_textures() > 0, "the presets' previews of this photo were rendered");
}

/// A finger's up or down drag scrolls the phone grid (its photos take only taps).
#[test]
fn a_drag_scrolls_the_phone_grid() {
    let mut h = grid([390.0, 844.0]);
    let before = h.app.grid_scroll.unwrap_or(0.0);
    let r = h.request("ui.drag", json!({"x": 200.0, "y": 650.0, "toX": 204.0, "toY": 250.0, "steps": 20}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert!(h.app.grid_scroll.unwrap_or(0.0) > before + 100.0, "{before} → {:?}", h.app.grid_scroll);
    assert!(h.app.ui.view != ViewMode::Detail, "the drag opened nothing");
}

/// iOS gap A2.8: choosing photos, a sideways drag from a photo chooses the run to the one under
/// the finger; from a chosen photo it unchooses them.
#[test]
fn a_sideways_drag_chooses_a_run_of_photos() {
    let mut h = grid([390.0, 844.0]);
    h.app.ui.view = ViewMode::SquareGrid;
    h.settle(SETTLE);
    click(&mut h, "button:select");
    let ids: Vec<u64> = h.app.session.visible().iter().take(3).map(|id| id.0).collect();
    let cell = |h: &Headless, id: u64| h.app.widgets.iter().find(|(w, _)| *w == format!("thumb:{id}")).map(|(_, r)| *r).expect("cell");
    let (a, c) = (cell(&h, ids[0]), cell(&h, ids[2]));
    assert!((a.center().y - c.center().y).abs() < 1.0, "one row of three");
    let r = h.request("ui.drag", json!({"x": a.center().x, "y": a.center().y, "toX": c.center().x, "toY": c.center().y + 3.0, "steps": 16}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    let mut chosen: Vec<u64> = h.app.session.selection.ids.iter().map(|i| i.0).collect();
    chosen.sort_unstable();
    let mut want = ids.clone();
    want.sort_unstable();
    assert_eq!(chosen, want, "the run of three is chosen");
    // from a chosen photo back to the first: those two are unchosen
    let b = cell(&h, ids[1]);
    let r = h.request("ui.drag", json!({"x": b.center().x, "y": b.center().y, "toX": a.center().x, "toY": a.center().y, "steps": 16}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    let chosen: Vec<u64> = h.app.session.selection.ids.iter().map(|i| i.0).collect();
    assert_eq!(chosen, vec![ids[2]], "{chosen:?}");
}

/// iOS gap A2.9: swiping up or down on a photo rates it (left half) or flags it (right half).
#[test]
fn swiping_up_or_down_on_a_photo_rates_and_flags_it() {
    let mut h = detail([390.0, 844.0]);
    h.app.ui.right = RightPanel::None;
    h.settle(SETTLE);
    let id = h.app.session.active().unwrap();
    h.app.run("photo.flag", json!({"ids": [id.0], "flag": "none"})).unwrap();
    let rating = h.app.session.catalog.photo(id).unwrap().rating;
    let img = h.app.image_rect.expect("loupe");
    // outside review mode an up / down swipe leaves the photo alone
    let (x, y) = (img.left() + img.width() * 0.25, img.center().y);
    let r = h.request("ui.drag", json!({"x": x, "y": y, "toX": x + 2.0, "toY": y - 120.0, "steps": 12}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert_eq!(h.app.session.catalog.photo(id).unwrap().rating, rating, "no rating outside review mode");
    // a tap (after the wait for a second one) enters it; the bars slide away and the photo grows
    let before = h.app.image_rect.expect("loupe");
    pointer(&mut h, &[("down", 0.5, 0.5), ("up", 0.5, 0.5)]);
    frames(&mut h, 60);
    assert!(h.app.ui.review && !has(&h, "icon:edit") && has(&h, "review:stars") && has(&h, "review:flag"));
    assert!(h.app.image_rect.expect("loupe").height() > before.height() * 1.1, "the photo takes the bars' room");
    let img = h.app.image_rect.expect("loupe");
    let swipe = |h: &mut Headless, x: f32, dy: f32| {
        let y = img.center().y;
        let r = h.request("ui.drag", json!({"x": x, "y": y, "toX": x + 2.0, "toY": y + dy, "steps": 12}), T);
        assert_eq!(r["ok"], true, "{r}");
        h.settle(SETTLE);
    };
    let left = img.left() + img.width() * 0.25;
    let right = img.left() + img.width() * 0.75;
    swipe(&mut h, left, -50.0);
    assert_eq!(h.app.session.catalog.photo(id).unwrap().rating, (rating + 1).min(5), "up on the left: a star more");
    // the popping readout grows from the screen's edge, never off it
    frames(&mut h, 8);
    let canvas = h.app.canvas_rect.expect("canvas");
    assert!(rect_of(&h, "review:stars").left() >= canvas.left() + 15.5, "{:?}", rect_of(&h, "review:stars"));
    swipe(&mut h, left, 50.0);
    assert_eq!(h.app.session.catalog.photo(id).unwrap().rating, rating, "down: a star less");
    swipe(&mut h, right, 50.0);
    assert_eq!(h.app.session.catalog.photo(id).unwrap().flag, lightcraft_catalog::Flag::Reject);
    swipe(&mut h, right, -50.0);
    assert_eq!(h.app.session.catalog.photo(id).unwrap().flag, lightcraft_catalog::Flag::None, "up from rejected: unflagged");
    swipe(&mut h, right, -50.0);
    assert_eq!(h.app.session.catalog.photo(id).unwrap().flag, lightcraft_catalog::Flag::Pick);
    assert_eq!(h.app.session.active(), Some(id), "still the same photo");
    // the readout of what changed pops, then rests
    frames(&mut h, 60);
    let at_rest = rect_of(&h, "review:flag").width();
    swipe(&mut h, right, 50.0);
    frames(&mut h, 8);
    assert!(rect_of(&h, "review:flag").width() > at_rest * 1.1, "the flag pops");
    assert!(rect_of(&h, "review:flag").right() <= canvas.right() - 15.5, "{:?}", rect_of(&h, "review:flag"));
    frames(&mut h, 60);
    assert!((rect_of(&h, "review:flag").width() - at_rest).abs() < 0.5, "and settles");
    // tapping again, or Back, brings the bars back
    pointer(&mut h, &[("down", 0.5, 0.5), ("up", 0.5, 0.5)]);
    frames(&mut h, 60);
    assert!(!h.app.ui.review && has(&h, "icon:edit"));
}

/// iOS: the stars and the flag follow the finger while it is down, a step for each stretch of
/// movement, and the photo takes what they show when it lifts.
#[test]
fn the_stars_and_flag_follow_the_finger_while_it_swipes() {
    let mut h = first_photo([390.0, 844.0]);
    let id = h.app.session.active().unwrap();
    h.app.run("photo.rate", json!({"ids": [id.0], "rating": 0})).unwrap();
    h.app.run("photo.flag", json!({"ids": [id.0], "flag": "none"})).unwrap();
    let r = h.request("engine.execute", json!({"command": "view.reviewMode", "params": {"on": true}}), T);
    assert_eq!(r["ok"], true, "{r}");
    frames(&mut h, 60);
    let tall = h.app.image_rect.expect("loupe").height();
    let up = |pt: f32| 0.7 - pt / tall; // the finger's height, in the photo's coordinates
    let shown = |h: &Headless| crate::panels::detail::swiping(&h.view.ctx);
    let stored = |h: &Headless| h.app.session.catalog.photo(id).map(|p| (p.rating, p.flag));
    // the left half: stars, one for each ~44 pt, shown while the finger is still down
    pointer(&mut h, &[("down", 0.25, 0.7), ("drag", 0.25, up(10.0))]);
    frames(&mut h, 2);
    assert_eq!(shown(&h), Some((0, 1)), "a little movement is no star yet");
    pointer(&mut h, &[("drag", 0.25, up(50.0))]);
    frames(&mut h, 2);
    assert_eq!(shown(&h), Some((1, 1)), "a star more, under the finger");
    assert_eq!(stored(&h), Some((0, lightcraft_catalog::Flag::None)), "and not yet stored");
    pointer(&mut h, &[("drag", 0.25, up(100.0))]);
    frames(&mut h, 2);
    assert_eq!(shown(&h), Some((2, 1)));
    // the star that was reached grows on its way in, then rests
    frames(&mut h, 30);
    pointer(&mut h, &[("drag", 0.25, up(30.0))]);
    frames(&mut h, 2);
    assert_eq!(shown(&h), Some((1, 1)), "back down: a star less");
    pointer(&mut h, &[("drag", 0.25, up(140.0)), ("up", 0.25, up(140.0))]);
    frames(&mut h, 2);
    assert_eq!(stored(&h), Some((3, lightcraft_catalog::Flag::None)), "lifting stores the three stars");
    frames(&mut h, 5);
    assert_eq!(shown(&h), None, "and the readout is the photo's again");
    // the right half: the flag goes reject, none, pick as the finger goes up
    pointer(&mut h, &[("down", 0.75, 0.7), ("drag", 0.75, up(60.0))]);
    frames(&mut h, 2);
    assert_eq!(shown(&h), Some((3, 2)), "up: pick");
    pointer(&mut h, &[("drag", 0.75, up(-60.0))]);
    frames(&mut h, 2);
    assert_eq!(shown(&h), Some((3, 0)), "down: reject");
    pointer(&mut h, &[("up", 0.75, up(-60.0))]);
    frames(&mut h, 2);
    assert_eq!(stored(&h), Some((3, lightcraft_catalog::Flag::Reject)));
    // a swipe that comes back to where it began changes nothing
    pointer(&mut h, &[("down", 0.25, 0.7), ("drag", 0.25, up(100.0)), ("drag", 0.25, up(0.0)), ("up", 0.25, up(0.0))]);
    frames(&mut h, 4);
    assert_eq!(stored(&h), Some((3, lightcraft_catalog::Flag::Reject)));
}

/// iOS: the tool's sheet and the group row slide away and back instead of jumping, and give the
/// photo their room as they go.
#[test]
fn the_tool_sheet_slides_away_and_back() {
    let mut h = detail([390.0, 844.0]);
    let with_sheet = h.app.canvas_rect.expect("canvas").height();
    let r = h.request("ui.clickWidget", json!({"id": "icon:edit"}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(h.app.ui.right, RightPanel::None);
    let mid = h.app.canvas_rect.expect("canvas").height();
    frames(&mut h, 40);
    let without = h.app.canvas_rect.expect("canvas").height();
    assert!(with_sheet < mid && mid < without, "still sliding: {with_sheet} < {mid} < {without}");
    // and back in
    let r = h.request("ui.clickWidget", json!({"id": "icon:edit"}), T);
    assert_eq!(r["ok"], true, "{r}");
    let mid = h.app.canvas_rect.expect("canvas").height();
    frames(&mut h, 40);
    let again = h.app.canvas_rect.expect("canvas").height();
    assert!(again < mid && mid < without, "{again} < {mid} < {without}");
    assert!((again - with_sheet).abs() < 1.0, "the sheet is the size it was");
}

/// iOS: going back to the grid, the bars below the photo slide down with the sheet.
#[test]
fn the_bars_slide_in_with_a_photo_and_out_with_the_grid() {
    let mut h = grid([390.0, 844.0]);
    assert!(!has(&h, "icon:edit"));
    let grid_h = h.app.canvas_rect.expect("canvas").height();
    let r = h.request("engine.execute", json!({"command": "library.select", "params": {"ids": [1]}}), T);
    assert_eq!(r["ok"], true, "{r}");
    let r = h.request("engine.execute", json!({"command": "view.detail"}), T);
    assert_eq!(r["ok"], true, "{r}");
    frames(&mut h, 2);
    let mid = h.app.canvas_rect.expect("canvas").height();
    frames(&mut h, 60);
    let photo_h = h.app.canvas_rect.expect("canvas").height();
    assert!(photo_h < mid && mid < grid_h, "the tool bar comes up: {photo_h} < {mid} < {grid_h}");
    let tabs_top = rect_of(&h, "icon:edit").top();
    assert!(tabs_top > 600.0, "it is at the bottom: {tabs_top}");
    let r = h.request("engine.execute", json!({"command": "view.photoGrid"}), T);
    assert_eq!(r["ok"], true, "{r}");
    frames(&mut h, 2);
    assert!(h.app.canvas_rect.expect("canvas").height() < grid_h - 1.0, "the bars are still leaving");
    frames(&mut h, 60);
    assert!((h.app.canvas_rect.expect("canvas").height() - grid_h).abs() < 1.0, "and gone");
}

/// iOS: a page slides back down when it closes, and a menu fades; neither vanishes.
#[test]
fn pages_and_menus_leave_the_way_they_came() {
    let mut h = detail([390.0, 844.0]);
    let r = h.request("engine.execute", json!({"command": "dialog.export"}), T);
    assert_eq!(r["ok"], true, "{r}");
    frames(&mut h, 60);
    let top = rect_of(&h, "sheet:dialog").top();
    let r = h.request("ui.clickWidget", json!({"id": "button:sheetCancel"}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert!(h.app.ui.dialog.is_none(), "the dialog is closed at once");
    let mid = rect_of(&h, "sheet:dialog").top();
    assert!(mid > top, "but its page is still on its way down: {top} < {mid}");
    frames(&mut h, 40);
    assert!(!has(&h, "sheet:dialog"), "and then gone");
    // a menu
    let r = h.request("ui.clickWidget", json!({"id": "icon:photoMore"}), T);
    assert_eq!(r["ok"], true, "{r}");
    frames(&mut h, 30);
    assert!(has(&h, "actions:photoMore"));
    let r = h.request("ui.clickWidget", json!({"id": "button:beforeAfter"}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert!(!crate::panels::mobile::actions_open(&h.view.ctx, "photoMore"), "closed at once");
    assert!(has(&h, "actions:photoMore"), "but still fading out");
    frames(&mut h, 40);
    assert!(!has(&h, "actions:photoMore"), "then gone");
}

/// iOS: a change of photo that the finger didn't make (a button, an arrow key) slides in from the
/// side it lies on, as a swipe would have.
#[test]
fn stepping_to_the_next_photo_slides_it_in() {
    let mut h = first_photo([390.0, 844.0]);
    let rest = h.app.image_rect.expect("loupe");
    let at = |h: &Headless| rect_of(h, "canvas:image").center().x - rest.center().x;
    let r = h.request("engine.execute", json!({"command": "library.next"}), T);
    assert_eq!(r["ok"], true, "{r}");
    frames(&mut h, 2);
    assert!(at(&h) > 100.0, "the next photo comes in from the right: {}", at(&h));
    frames(&mut h, 90);
    assert!(at(&h).abs() < 0.5, "and rests");
    let r = h.request("engine.execute", json!({"command": "library.previous"}), T);
    assert_eq!(r["ok"], true, "{r}");
    frames(&mut h, 2);
    assert!(at(&h) < -100.0, "the previous one from the left: {}", at(&h));
}

/// iOS: a drag across a fitted photo carries it with the finger, its neighbour alongside, and
/// letting go settles on the neighbour (far enough, or flicked) or back.
#[test]
fn a_sideways_drag_carries_the_photo_like_a_carousel() {
    let mut h = first_photo([390.0, 844.0]);
    let first = h.app.session.active().unwrap();
    let next = h.app.session.visible()[1];
    let rest = h.app.image_rect.expect("loupe");
    assert_eq!(h.app.renderer.textures.get(&crate::render::Slot::Prefetch(0)).map(|t| t.photo), Some(next), "the next photo's render is kept");
    assert!((rect_of(&h, "canvas:image").center().x - rest.center().x).abs() < 0.5, "at rest");
    // the finger moves half a photo's width left: the photo is exactly that far along
    pointer(&mut h, &[("down", 0.9, 0.5), ("drag", 0.4, 0.5)]);
    frames(&mut h, 3);
    let moved = rect_of(&h, "canvas:image").center().x - rest.center().x;
    assert!((moved + 0.5 * rest.width()).abs() < 1.5, "follows the finger: moved {moved}");
    assert_eq!(h.app.session.active(), Some(first), "not yet changed");
    // let go past a quarter of the width: the next photo, arriving from the right
    pointer(&mut h, &[("up", 0.4, 0.5)]);
    frames(&mut h, 2);
    assert_eq!(h.app.session.active(), Some(next));
    let c = rect_of(&h, "canvas:image").center().x - rest.center().x;
    assert!(c > 0.0 && c < 0.8 * (rest.width() + 16.0), "the new photo slides in from the right: {c}");
    frames(&mut h, 90);
    assert!((rect_of(&h, "canvas:image").center().x - rest.center().x).abs() < 0.5, "and rests");
    // a short, slow drag settles back to the same photo
    pointer(&mut h, &[("down", 0.9, 0.5), ("drag", 0.75, 0.5)]);
    frames(&mut h, 12);
    pointer(&mut h, &[("up", 0.75, 0.5)]);
    frames(&mut h, 90);
    assert_eq!(h.app.session.active(), Some(next), "a short slow drag stays");
    assert!((rect_of(&h, "canvas:image").center().x - rest.center().x).abs() < 0.5);
    // a short flick goes on
    pointer(&mut h, &[("down", 0.9, 0.5), ("drag", 0.8, 0.5), ("drag", 0.7, 0.5), ("up", 0.7, 0.5)]);
    frames(&mut h, 90);
    assert_ne!(h.app.session.active(), Some(next), "a flick changes photo");
}

/// iOS: before the first photo there is nothing to carry in: the photo gives way grudgingly and
/// comes back.
#[test]
fn the_first_photo_resists_a_drag_towards_nothing() {
    let mut h = first_photo([390.0, 844.0]);
    let first = h.app.session.active().unwrap();
    let rest = h.app.image_rect.expect("loupe");
    pointer(&mut h, &[("down", 0.1, 0.5), ("drag", 0.9, 0.5)]);
    frames(&mut h, 3);
    let moved = rect_of(&h, "canvas:image").center().x - rest.center().x;
    assert!(moved > 20.0 && moved < 0.4 * 0.8 * rest.width(), "{moved}");
    pointer(&mut h, &[("up", 0.9, 0.5)]);
    frames(&mut h, 90);
    assert_eq!(h.app.session.active(), Some(first));
    assert!((rect_of(&h, "canvas:image").center().x - rest.center().x).abs() < 0.5);
}

/// iOS: a double tap zooms and leaves the bars alone; Back leaves review mode before anything else.
#[test]
fn a_double_tap_zooms_and_does_not_hide_the_bars() {
    let mut h = first_photo([390.0, 844.0]);
    pointer(&mut h, &[("down", 0.5, 0.5), ("up", 0.5, 0.5), ("down", 0.5, 0.5), ("up", 0.5, 0.5)]);
    frames(&mut h, 60);
    assert_ne!(h.app.ui.zoom, crate::state::Zoom::Fit, "zoomed");
    assert!(!h.app.ui.review && has(&h, "icon:edit"), "the bars stayed");
    let r = h.request("engine.execute", json!({"command": "view.reviewMode", "params": {"on": true}}), T);
    assert_eq!(r["ok"], true, "{r}");
    frames(&mut h, 30);
    assert!(h.app.ui.review);
    let r = h.request("engine.execute", json!({"command": "view.back", "params": {}}), T);
    assert_eq!(r["ok"], true, "{r}");
    frames(&mut h, 30);
    assert!(!h.app.ui.review && h.app.ui.view == ViewMode::Detail && has(&h, "icon:edit"));
    // and a tool takes it over: Crop ends review mode
    let r = h.request("engine.execute", json!({"command": "view.reviewMode", "params": {"on": true}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.app.ui.right = RightPanel::Crop;
    frames(&mut h, 30);
    assert!(!h.app.ui.review);
}

/// iOS: a tap on the photo is felt the moment the finger lifts, and the bars go as soon as no
/// second tap has begun (not after a fixed wait); a double tap whose second finger came later than
/// that still zooms, and puts the bars back.
#[test]
fn a_tap_is_felt_at_once_and_the_bars_go_soon_after() {
    let mut h = first_photo([390.0, 844.0]);
    let log = record_haptics(&mut h);
    pointer(&mut h, &[("down", 0.5, 0.5), ("up", 0.5, 0.5)]);
    assert_eq!(heard(&log), [Haptic::Light], "felt as the finger lifts");
    assert!(!h.app.ui.review, "not yet: it may be a double tap's first");
    frames(&mut h, 14);
    assert!(h.app.ui.review, "gone within a quarter second");
    assert_eq!(heard(&log), [], "felt once");
    // a slow double tap: the bars have just gone when the second finger comes down
    let r = h.request("engine.execute", json!({"command": "view.reviewMode", "params": {"on": false}}), T);
    assert_eq!(r["ok"], true, "{r}");
    frames(&mut h, 60);
    pointer(&mut h, &[("down", 0.5, 0.5), ("up", 0.5, 0.5)]);
    for _ in 0..20 {
        if h.app.ui.review {
            break;
        }
        h.step();
    }
    assert!(h.app.ui.review);
    pointer(&mut h, &[("down", 0.5, 0.5), ("up", 0.5, 0.5)]);
    frames(&mut h, 60);
    assert_ne!(h.app.ui.zoom, crate::state::Zoom::Fit, "zoomed");
    assert!(!h.app.ui.review && has(&h, "icon:edit"), "and the bars are back");
}

/// No menu bar on a phone: the grid's and the photo's "…" menus, and every other command in the
/// searchable All Commands list.
#[test]
fn a_phone_has_menus_instead_of_a_menu_bar() {
    let mut h = grid([390.0, 844.0]);
    assert!(!h.app.widgets.iter().any(|(w, _)| w.starts_with("menu:")), "no menu bar");
    click(&mut h, "icon:more");
    assert!(has(&h, "actions:more") && has(&h, "button:newAlbum"));
    click(&mut h, "button:allCommands");
    assert!(h.app.ui.all_commands && has(&h, "sheet:allCommands"));
    // every menu command is there; searching narrows the list
    assert!(has(&h, "button:cmd:library.selectAll"));
    click(&mut h, "field:allCommandsSearch");
    let r = h.request("ui.text", json!({"text": "select all"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert!(!has(&h, "button:cmd:dialog.newAlbum"), "the search hides the others");
    click(&mut h, "button:cmd:library.selectAll");
    assert!(!h.app.ui.all_commands && h.app.session.selection.ids.len() > 1, "the command ran and the list closed");
    // the collection title opens the albums list
    click(&mut h, "button:collections");
    assert!(h.app.ui.left_panel && has(&h, "sheet:albums"));
}

fn grid(size: [f32; 2]) -> Headless {
    let mut app = LightcraftApp::new(Session::with_demo(), Services { png: None, ..Default::default() });
    app.ui.view = ViewMode::PhotoGrid;
    let mut h = Headless::new(app, size, 1.0);
    h.settle(SETTLE);
    h
}

fn click(h: &mut Headless, id: &str) {
    let r = h.request("ui.clickWidget", json!({"id": id}), T);
    assert_eq!(r["ok"], true, "{id}: {r}");
    h.settle(SETTLE);
}

/// iOS gap A2.8 / A2.9: photos are chosen by touch (Select, then taps; a long press starts it
/// with that photo) and rated, flagged, labelled, put in an album or deleted from the action bar.
#[test]
fn photos_are_chosen_by_touch_and_acted_on_from_the_action_bar() {
    let mut h = grid([390.0, 844.0]);
    assert!(h.app.compact && has(&h, "button:select") && !has(&h, "icon:selRate"));
    click(&mut h, "button:select");
    assert!(h.app.ui.select_mode && h.app.session.selection.ids.is_empty(), "choosing starts with none");
    assert!(has(&h, "button:selectCancel") && has(&h, "icon:selRate") && has(&h, "icon:selDelete"));
    let ids: Vec<u64> = h.app.session.visible().iter().take(2).map(|id| id.0).collect();
    // taps add and remove; nothing opens
    for id in &ids {
        click(&mut h, &format!("thumb:{id}"));
    }
    assert_eq!(h.app.ui.view, ViewMode::PhotoGrid, "a tap chooses instead of opening the photo");
    let chosen: Vec<u64> = h.app.session.selection.ids.iter().map(|i| i.0).collect();
    assert_eq!(chosen, ids);
    let photo = |h: &Headless, id: u64| h.app.session.catalog.photo(lightcraft_catalog::PhotoId(id)).unwrap().clone();
    // rating, flag, label
    click(&mut h, "icon:selRate");
    click(&mut h, "button:rate4");
    click(&mut h, "icon:selFlag");
    click(&mut h, "button:flag-pick");
    click(&mut h, "icon:selLabel");
    click(&mut h, "button:label-green");
    for id in &ids {
        let p = photo(&h, *id);
        assert_eq!((p.rating, p.flag, p.label), (4, lightcraft_catalog::Flag::Pick, Some(lightcraft_catalog::ColorLabel::Green)), "{id}");
    }
    // into an album
    let album = h.app.run("album.create", json!({"name": "Trip"})).unwrap()["id"].as_u64().unwrap();
    click(&mut h, "icon:selAlbum");
    click(&mut h, &format!("button:album-{album}"));
    let in_album = h.app.session.catalog.album(lightcraft_catalog::AlbumId(album)).unwrap().photos.len();
    assert_eq!(in_album, 2);
    // one photo off again, then copy its settings and paste them on the other
    click(&mut h, &format!("thumb:{}", ids[1]));
    assert_eq!(h.app.session.selection.ids.len(), 1);
    h.app.run("develop.set", json!({"ids": [ids[0]], "values": {"light.exposure": 0.7}})).unwrap();
    click(&mut h, "icon:selMore");
    click(&mut h, "button:copySettings");
    assert!(h.app.session.clipboard.is_some());
    click(&mut h, &format!("thumb:{}", ids[0]));
    click(&mut h, &format!("thumb:{}", ids[1]));
    click(&mut h, "icon:selMore");
    click(&mut h, "button:pasteSettings");
    assert!((photo(&h, ids[1]).develop.light.exposure - 0.7).abs() < 1e-6);
    // Select All, Deselect All, Cancel
    click(&mut h, "button:selectAll");
    assert_eq!(h.app.session.selection.ids.len(), h.app.session.visible().len());
    click(&mut h, "button:selectNone");
    assert!(h.app.session.selection.ids.is_empty());
    click(&mut h, "button:selectCancel");
    assert!(!h.app.ui.select_mode && has(&h, "button:select"));
    // a long press starts choosing with that photo (the command it runs)
    let r = h.request("engine.execute", json!({"command": "view.selectMode", "params": {"on": true, "id": ids[0]}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert_eq!(h.app.session.selection.ids.iter().map(|i| i.0).collect::<Vec<_>>(), [ids[0]]);
    // delete (asks first), into Recently Deleted
    click(&mut h, "icon:selDelete");
    if h.app.ui.dialog.is_some() {
        let r = h.request("ui.dialog.confirm", json!({}), T);
        assert_eq!(r["ok"], true, "{r}");
        h.settle(SETTLE);
    }
    assert!(photo(&h, ids[0]).deleted);
    // Back leaves choosing; so does opening another view
    let r = h.request("engine.execute", json!({"command": "view.back", "params": {}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert!(!h.app.ui.select_mode);
}

/// The loupe rates, flags and labels the photo from its "…" menu (no right click on a phone).
#[test]
fn the_loupe_rates_flags_and_labels_by_touch() {
    let mut h = detail([390.0, 844.0]);
    h.app.ui.right = RightPanel::None;
    h.settle(SETTLE);
    click(&mut h, "icon:photoMore");
    click(&mut h, "button:rate3");
    let id = h.app.session.active().unwrap();
    assert_eq!(h.app.session.catalog.photo(id).unwrap().rating, 3);
    click(&mut h, "icon:photoMore");
    click(&mut h, "button:flag-reject");
    click(&mut h, "icon:photoMore");
    click(&mut h, "button:label-none");
    let p = h.app.session.catalog.photo(id).unwrap();
    assert_eq!((p.flag, p.label), (lightcraft_catalog::Flag::Reject, None));
    // the desktop layout has none of this
    let h = detail([1200.0, 800.0]);
    assert!(!has(&h, "icon:photoMore") && !has(&h, "button:select"));
}

/// Record the haptics the app asks the host for.
fn record_haptics(h: &mut Headless) -> Rc<RefCell<Vec<Haptic>>> {
    let log = Rc::new(RefCell::new(Vec::new()));
    let sink = log.clone();
    h.app.services.haptic = Some(Box::new(move |t| sink.borrow_mut().push(t)));
    log
}

/// What was recorded since the last call.
fn heard(log: &Rc<RefCell<Vec<Haptic>>>) -> Vec<Haptic> {
    std::mem::take(&mut *log.borrow_mut())
}

/// iOS gap A2.12: a slider ticks as a finger crosses its default (the zero of most sliders) and
/// taps at the end of its range; leaving the default is silent; the desktop layout never buzzes.
#[test]
fn a_touch_slider_ticks_at_its_default_and_taps_at_the_ends() {
    let mut h = detail([390.0, 1500.0]);
    let log = record_haptics(&mut h);
    let track = rect_of(&h, "slider:light.exposure");
    let y = track.center().y - 10.0;
    let drag = |h: &mut Headless, from: f32, by: f32| {
        let x = track.left() + track.width() * from;
        let r = h.request("ui.drag", json!({"x": x, "y": y, "toX": x + track.width() * by, "toY": y, "steps": 20}), T);
        assert_eq!(r["ok"], true, "{r}");
        h.settle(SETTLE);
    };
    drag(&mut h, 0.5, 0.1); // 0 → +1 EV: away from the default
    assert_eq!(heard(&log), [], "leaving the default is silent");
    drag(&mut h, 0.6, -0.2); // +1 → −1 EV: across it
    assert!(heard(&log).contains(&Haptic::Selection), "crossing the default ticks");
    drag(&mut h, 0.0, 1.0); // −1 → the end of the range
    let got = heard(&log);
    assert!(got.contains(&Haptic::Light), "reaching the end taps: {got:?}");
    // a double tap resets it: a light tap
    let (x, y) = (track.center().x, track.center().y - 10.0);
    let r = h.request("ui.click", json!({"x": x, "y": y, "count": 2}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    let got = heard(&log);
    assert!(h.app.session.active().and_then(|id| h.app.session.develop_of(id)).is_some_and(|d| d.light.exposure == 0.0), "reset: {got:?}");
    // the desktop layout has the same sliders and no haptics
    let mut h = detail([1200.0, 800.0]);
    let log = record_haptics(&mut h);
    let track = rect_of(&h, "slider:light.exposure");
    let y = track.center().y;
    let r = h.request("ui.drag", json!({"x": track.left() + 2.0, "y": y, "toX": track.right() - 2.0, "toY": y, "steps": 20}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert_eq!(heard(&log), []);
}

/// iOS gap A2.12 / A2.27: a swipe in review mode ticks at each star or flag step and taps when the
/// finger lifts and the photo keeps it.
#[test]
fn review_swipes_tick_at_each_step_and_tap_when_kept() {
    let mut h = first_photo([390.0, 844.0]);
    let id = h.app.session.active().unwrap();
    h.app.run("photo.rate", json!({"ids": [id.0], "rating": 0})).unwrap();
    let r = h.request("engine.execute", json!({"command": "view.reviewMode", "params": {"on": true}}), T);
    assert_eq!(r["ok"], true, "{r}");
    frames(&mut h, 60);
    let log = record_haptics(&mut h);
    let tall = h.app.image_rect.expect("loupe").height();
    let up = |pt: f32| 0.7 - pt / tall;
    pointer(&mut h, &[("down", 0.25, 0.7), ("drag", 0.25, up(10.0))]);
    frames(&mut h, 2);
    assert_eq!(heard(&log), [], "no step yet");
    pointer(&mut h, &[("drag", 0.25, up(50.0))]);
    frames(&mut h, 2);
    assert_eq!(heard(&log), [Haptic::Selection], "one star");
    pointer(&mut h, &[("drag", 0.25, up(100.0))]);
    frames(&mut h, 2);
    assert_eq!(heard(&log), [Haptic::Selection], "two");
    pointer(&mut h, &[("drag", 0.25, up(104.0))]);
    frames(&mut h, 2);
    assert_eq!(heard(&log), [], "moving within a step is silent");
    pointer(&mut h, &[("up", 0.25, up(104.0))]);
    frames(&mut h, 4);
    assert_eq!(h.app.session.catalog.photo(id).map(|p| p.rating), Some(2));
    assert_eq!(heard(&log), [Haptic::Light], "kept");
    // a swipe that changed nothing taps nothing
    pointer(&mut h, &[("down", 0.25, 0.7), ("drag", 0.25, up(10.0)), ("up", 0.25, up(10.0))]);
    frames(&mut h, 4);
    assert_eq!(heard(&log), []);
}

/// iOS gap A2.12: choosing photos, picking a tool and deleting each have their own feel.
#[test]
fn choices_tools_and_deleting_have_their_own_haptics() {
    let mut h = grid([390.0, 844.0]);
    let log = record_haptics(&mut h);
    click(&mut h, "button:select");
    assert_eq!(heard(&log), [], "the Select button is silent");
    let id = h.app.session.visible()[0].0;
    click(&mut h, &format!("thumb:{id}"));
    assert_eq!(heard(&log), [Haptic::Selection], "a photo chosen");
    click(&mut h, "icon:selDelete");
    assert!(heard(&log).contains(&Haptic::Warning), "delete warns");
    h.app.ui.dialog = None;
    // a photo open: picking a tool ticks, and so does a group of the Edit tool
    let mut h = detail([390.0, 844.0]);
    let log = record_haptics(&mut h);
    click(&mut h, "icon:masking");
    assert_eq!(heard(&log), [Haptic::Selection]);
    click(&mut h, "icon:edit");
    let _ = heard(&log);
    click(&mut h, "button:group-color");
    assert_eq!(heard(&log), [Haptic::Selection], "a group");
    // undo taps lightly
    h.app.run("develop.set", json!({"values": {"light.exposure": 0.7}})).unwrap();
    h.settle(SETTLE);
    click(&mut h, "icon:undo");
    assert_eq!(heard(&log), [Haptic::Light]);
}

/// A phone in its grid, nothing open.
fn phone(services: Services) -> Headless {
    let mut h = Headless::new(LightcraftApp::new(Session::with_demo(), Services { png: None, ..services }), [390.0, 844.0], 1.0);
    h.settle(SETTLE);
    h
}

fn tap(h: &mut Headless, id: &str) {
    let r = h.request("ui.clickWidget", json!({"id": id}), T);
    assert_eq!(r["ok"], true, "{id}: {r}");
    h.settle(SETTLE);
}

/// Settings on a phone, as iOS's Settings app: a list of the sections, each pushed as a page (its
/// back button returns to the list); choices are pull-down menus, on / off settings switches; Done
/// closes it.
#[test]
fn phone_settings_is_a_list_of_pages() {
    use crate::state::{Dialog, StartupView};
    let mut h = phone(Services::default());
    let r = h.request("ui.menu.invoke", json!({"id": "app.settings"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert_eq!(h.app.ui.dialog, Some(Dialog::Settings { tab: String::new() }));
    assert!(!has(&h, "button:sheetCancel"), "the list has no back button");
    for tab in ["general", "import", "performance", "interface", "sync"] {
        assert!(has(&h, &format!("button:settingsTab-{tab}")), "{tab}");
    }
    tap(&mut h, "button:settingsTab-general");
    assert_eq!(h.app.ui.dialog, Some(Dialog::Settings { tab: "general".into() }));
    assert!(!has(&h, "button:settingsStartup-2"), "the menu opens on a tap");
    tap(&mut h, "button:settingsStartup");
    tap(&mut h, "button:settingsStartup-2");
    assert_eq!(h.app.ui.settings.startup_view, StartupView::Detail);
    let before = h.app.ui.settings.confirm_delete;
    tap(&mut h, "check:settings.confirmDelete");
    assert_eq!(h.app.ui.settings.confirm_delete, !before);
    tap(&mut h, "button:sheetCancel");
    assert_eq!(h.app.ui.dialog, Some(Dialog::Settings { tab: String::new() }), "back to the list");
    tap(&mut h, "button:settingsTab-interface");
    assert!(!has(&h, "check:settings.filmNames"), "no filmstrip on a phone");
    tap(&mut h, "button:sheetOk");
    assert_eq!(h.app.ui.dialog, None);
}

/// A text field's edit menu on a touch screen: the first tap gives the field the keyboard, the next
/// one opens Select All / Paste; Select All offers Cut and Copy; Paste puts the clipboard's text in
/// the field, which keeps the keyboard throughout.
#[test]
fn a_tap_on_a_focused_field_opens_its_edit_menu() {
    let mut h = phone(Services { clipboard_text: Some(Box::new(|| Some("crop".into()))), ..Default::default() });
    h.app.ui.all_commands = true;
    h.settle(SETTLE);
    let field = rect_of(&h, "field:allCommandsSearch");
    h.app.synthetic.push(egui::Event::Touch {
        device_id: egui::TouchDeviceId(0),
        id: egui::TouchId(0),
        phase: egui::TouchPhase::Start,
        pos: field.center(),
        force: None,
    });
    h.settle(SETTLE);
    tap(&mut h, "field:allCommandsSearch");
    assert!(!has(&h, "button:edit-paste"), "a first tap only focuses");
    let r = h.request("ui.text", json!({"text": "export"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    tap(&mut h, "field:allCommandsSearch");
    assert!(has(&h, "button:edit-selectAll") && has(&h, "button:edit-paste"), "{:?}", h.app.widgets);
    tap(&mut h, "button:edit-selectAll");
    assert!(has(&h, "button:edit-copy") && has(&h, "button:edit-cut"), "a selection can be copied");
    tap(&mut h, "button:edit-paste");
    let query = h.view.ctx.data(|d| d.get_temp::<String>(egui::Id::new("lc-all-commands-query")));
    assert_eq!(query.as_deref(), Some("crop"), "the pasted text replaced the selection");
    assert!(h.view.ctx.memory(|m| m.focused()).is_some(), "the field kept the keyboard");
    assert!(!has(&h, "button:edit-paste"), "a choice closes the menu");
}
