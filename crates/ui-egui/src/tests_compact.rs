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

/// The loupe rates, flags and labels the photo from the top bar (no right click on a phone).
#[test]
fn the_loupe_rates_flags_and_labels_by_touch() {
    let mut h = detail([390.0, 844.0]);
    h.app.ui.right = RightPanel::None;
    h.settle(SETTLE);
    click(&mut h, "icon:rateFlag");
    click(&mut h, "button:rate3");
    let id = h.app.session.active().unwrap();
    assert_eq!(h.app.session.catalog.photo(id).unwrap().rating, 3);
    click(&mut h, "icon:rateFlag");
    click(&mut h, "button:flag-reject");
    click(&mut h, "icon:rateFlag");
    click(&mut h, "button:label-none");
    let p = h.app.session.catalog.photo(id).unwrap();
    assert_eq!((p.flag, p.label), (lightcraft_catalog::Flag::Reject, None));
    // the desktop layout has none of this
    let h = detail([1200.0, 800.0]);
    assert!(!has(&h, "icon:rateFlag") && !has(&h, "button:select"));
}
