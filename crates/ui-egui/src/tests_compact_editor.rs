//! Headless tests of the phone's editor screens: the sheet under the photo (its grabber and its
//! three heights, shrinking to its content), slider rows, and the tool panels' touch-sized controls
//! (iOS gap A2.4 / A2.5).

use std::time::Duration;

use egui::{Rect, vec2};
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

fn rect(h: &Headless, id: &str) -> Rect {
    h.app.widgets.iter().find(|(w, _)| w == id).map(|(_, r)| *r).unwrap_or_else(|| panic!("{id} is not on screen"))
}

fn has(h: &Headless, id: &str) -> bool {
    h.app.widgets.iter().any(|(w, _)| w == id)
}

fn click(h: &mut Headless, id: &str) {
    let r = h.request("ui.clickWidget", json!({"id": id}), T);
    assert_eq!(r["ok"], true, "{id}: {r}");
    h.settle(SETTLE);
}

/// A drag of the finger from `from` to `to` in steps.
fn drag(h: &mut Headless, from: egui::Pos2, to: egui::Pos2) {
    let r = h.request("ui.drag", json!({"x": from.x, "y": from.y, "toX": to.x, "toY": to.y, "steps": 12}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
}

/// The top of the sheet (the grabber's top edge): lower on the screen is a shorter sheet. The sheet
/// eases to its height, so this runs frames until it has stopped.
fn sheet_top(h: &mut Headless) -> f32 {
    let mut last = rect(h, "sheet:grabber").top();
    for _ in 0..240 {
        h.step();
        let now = rect(h, "sheet:grabber").top();
        if (now - last).abs() < 0.01 {
            return now;
        }
        last = now;
    }
    last
}

/// The sheet has a grabber: a tap steps through its three heights, a drag takes it to the nearest
/// and a drag right down puts the tool away.
#[test]
fn the_sheet_has_a_grabber_with_three_heights() {
    let mut h = detail([390.0, 844.0]);
    assert!(has(&h, "sheet:grabber"));
    assert!(rect(&h, "sheet:grabber").height() >= 24.0, "a finger can hit it: {:?}", rect(&h, "sheet:grabber"));
    assert_eq!(h.app.ui.sheet_detent, 1, "it opens at the middle height");
    let middle = sheet_top(&mut h);
    // a tap steps up to the tallest, then round to the shortest
    click(&mut h, "sheet:grabber");
    assert_eq!(h.app.ui.sheet_detent, 2);
    let tall = sheet_top(&mut h);
    assert!(tall < middle - 80.0, "taller: {tall} < {middle}");
    click(&mut h, "sheet:grabber");
    assert_eq!(h.app.ui.sheet_detent, 0);
    let short = sheet_top(&mut h);
    assert!(short > middle + 40.0, "shorter: {short} > {middle}");
    // a drag up from the shortest lets go nearest the tallest
    let g = rect(&h, "sheet:grabber").center();
    drag(&mut h, g, g - vec2(0.0, 300.0));
    assert_eq!(h.app.ui.sheet_detent, 2, "dragged up: the tallest");
    assert!((sheet_top(&mut h) - tall).abs() < 1.5, "and it settled there: {} vs {tall} (middle {middle}, short {short})", sheet_top(&mut h));
    // a drag down to the bottom of the screen closes the tool
    let g = rect(&h, "sheet:grabber").center();
    drag(&mut h, g, g + vec2(0.0, 500.0));
    assert_eq!(h.app.ui.right, RightPanel::None, "dragged right down: the sheet is put away");
    // (it slides off the screen)
    for _ in 0..120 {
        if !has(&h, "sheet:grabber") {
            break;
        }
        h.step();
    }
    assert!(!has(&h, "sheet:grabber"), "and is gone");
}

/// A group with one row (Profile) needs less sheet than Light: the sheet is only as tall as its
/// content, and the photo gets the rest.
#[test]
fn the_sheet_is_only_as_tall_as_its_content() {
    let mut h = detail([390.0, 844.0]);
    let light = sheet_top(&mut h);
    h.app.ui.edit_group = "profile".into();
    h.settle(SETTLE);
    let profile = sheet_top(&mut h);
    assert!(profile > light + 40.0, "Profile's sheet is shorter: {profile} vs {light}");
    let photo = h.app.image_rect.expect("loupe");
    assert!(photo.bottom() <= profile, "the photo stays above the sheet: {photo:?}");
}

/// Slider rows on a phone are 46 pt apart (they were 60), with the whole row grabbing a finger.
#[test]
fn phone_slider_rows_are_compact() {
    let h = detail([390.0, 1000.0]);
    let exposure = rect(&h, "slider:light.exposure");
    let contrast = rect(&h, "slider:light.contrast");
    assert!((contrast.center().y - exposure.center().y - crate::widgets::TOUCH_SLIDER_ROW_H).abs() < 0.5, "{exposure:?} {contrast:?}");
    assert!(exposure.left() <= 16.5 && exposure.right() >= 390.0 - 16.5, "tracks run the width but for the 16 pt margins: {exposure:?}");
}

/// A drag that starts on a slider's label moves the slider, as the whole row is its grab zone.
#[test]
fn a_drag_on_a_sliders_label_slides_it() {
    let mut h = detail([390.0, 1000.0]);
    let id = h.app.session.active().expect("active photo");
    let track = rect(&h, "slider:light.exposure");
    // the label line is above the track: well outside a desktop slider's grab zone
    let from = egui::pos2(track.left() + track.width() * 0.5, track.center().y - 18.0);
    drag(&mut h, from, from + vec2(track.width() * 0.1, 0.0));
    let exposure = h.app.session.develop_of(id).map(|d| d.light.exposure).unwrap_or_default();
    assert!(exposure > 0.5, "exposure {exposure}");
}

/// The Crop tool on a phone is a screen of its own (Lightroom's mobile app): the tool bar and the
/// sheet make way for a title, the angle dial, four round buttons, the Aspect / Geometry tabs and a
/// bar with ✕ and ✓; every control is a finger wide.
#[test]
fn phone_crop_is_a_screen_of_its_own() {
    let mut h = detail([390.0, 844.0]);
    let id = h.app.session.active().expect("active photo");
    click(&mut h, "icon:crop");
    assert_eq!(h.app.ui.right, RightPanel::Crop);
    assert!(!has(&h, "icon:edit") && !has(&h, "sheet:grabber"), "no tool bar or sheet while cropping");
    for w in ["icon:cropCancel", "icon:cropDone", "icon:undo", "icon:cropAuto", "icon:cropLock", "icon:cropRotate", "icon:cropMore"] {
        let r = rect(&h, w);
        assert!(r.width() >= 43.9 && r.height() >= 43.9, "{w} is a finger wide: {r:?}");
    }
    for w in [
        "dial:angle",
        "button:cropTab-aspect",
        "button:cropTab-geometry",
        "button:cropOriginal",
        "button:cropRatios",
        "button:cropInstagram",
        "button:cropTikTok",
    ] {
        assert!(has(&h, w), "{w} is on the Aspect tab");
    }
    // untouched, the photo is cropped to its own shape (locked or not: the lock is open)
    assert!(h.app.session.develop_of(id).expect("settings").crop.aspect.is_none());
    // the Ratios button opens a menu of them; choosing one locks the crop to it
    click(&mut h, "button:cropRatios");
    assert!(has(&h, "button:cropAspect-4x5"), "the ratios are listed");
    click(&mut h, "button:cropAspect-4x5");
    assert_eq!(h.app.session.develop_of(id).expect("settings").crop.aspect, Some((400, 500)), "4 × 5 is locked");
    // the lock frees it again
    click(&mut h, "icon:cropLock");
    assert_eq!(h.app.session.develop_of(id).expect("settings").crop.aspect, None, "unlocked: free");
    // Instagram's formats are in a menu of their own
    click(&mut h, "button:cropInstagram");
    click(&mut h, "button:cropAspect-100x191");
    assert_eq!(h.app.session.develop_of(id).expect("settings").crop.aspect, Some((10000, 19100)), "Landscape is 1.91 : 1");
}

/// Dragging the dial straightens the photo (one undo step); the round buttons rotate and flip.
#[test]
fn the_crop_dial_straightens_and_the_buttons_rotate() {
    let mut h = detail([390.0, 844.0]);
    let id = h.app.session.active().expect("active photo");
    click(&mut h, "icon:crop");
    let angle = |h: &Headless| h.app.session.develop_of(id).expect("settings").crop.geometry.angle;
    assert_eq!(angle(&h), 0.0);
    let steps = h.app.session.undo.len();
    let dial = rect(&h, "dial:angle").center();
    // the ruler follows the finger: 70 pt to the left reads about 10° higher
    drag(&mut h, dial, dial - vec2(70.0, 0.0));
    assert!((angle(&h) - 10.0).abs() < 1.5, "angle {}", angle(&h));
    assert_eq!(h.app.session.undo.len(), steps + 1, "the drag is one undo step");
    click(&mut h, "icon:cropRotate");
    assert!(h.app.session.develop_of(id).expect("settings").orientation.swaps_axes(), "turned a quarter");
    click(&mut h, "icon:cropMore");
    click(&mut h, "button:cropFlipH");
    assert!(h.app.session.develop_of(id).expect("settings").crop.flip_h);
    click(&mut h, "icon:cropMore");
    click(&mut h, "button:cropStraighten");
    assert_eq!(h.app.ui.tool, "straighten", "the line tool is on");
    assert!(has(&h, "button:straightenCancel"), "and says how to leave it");
    click(&mut h, "button:straightenCancel");
    assert!(h.app.ui.tool.is_empty());
}

/// ✓ keeps what was done; ✕ undoes all of it (the aspect, the angle, the turn) and leaves what was
/// done before the screen opened alone. Both go back to Edit.
#[test]
fn the_crop_screens_cross_undoes_it_and_its_tick_keeps_it() {
    let mut h = detail([390.0, 844.0]);
    let id = h.app.session.active().expect("active photo");
    let r = h.request("engine.execute", json!({"command": "develop.set", "params": {"control": "light.exposure", "value": 0.7}}), T);
    assert_eq!(r["ok"], true, "{r}");
    let before = h.app.session.develop_of(id).expect("settings");
    click(&mut h, "icon:crop");
    click(&mut h, "button:cropRatios");
    click(&mut h, "button:cropAspect-1x1");
    let dial = rect(&h, "dial:angle").center();
    drag(&mut h, dial, dial - vec2(35.0, 0.0));
    click(&mut h, "icon:cropRotate");
    assert_ne!(*h.app.session.develop_of(id).expect("settings"), *before, "the crop changed the photo");
    click(&mut h, "icon:cropCancel");
    assert_eq!(h.app.ui.right, RightPanel::Edit, "back to Edit");
    assert_eq!(*h.app.session.develop_of(id).expect("settings"), *before, "everything since the screen opened is undone, the exposure kept");
    assert!(!has(&h, "dial:angle") && has(&h, "icon:edit"), "the tool bar is back");
    // and ✓ keeps it
    click(&mut h, "icon:crop");
    click(&mut h, "button:cropRatios");
    click(&mut h, "button:cropAspect-1x1");
    click(&mut h, "icon:cropDone");
    assert_eq!(h.app.ui.right, RightPanel::Edit);
    assert_eq!(h.app.session.develop_of(id).expect("settings").crop.aspect, Some((100, 100)), "the square crop stays");
}

/// The Geometry tab: Upright's five modes and the lens / perspective sliders, with no crop frame,
/// dial or round buttons; a tap on the chosen mode turns it off.
#[test]
fn the_crop_screens_geometry_tab_has_upright_and_sliders() {
    let mut h = detail([390.0, 844.0]);
    let id = h.app.session.active().expect("active photo");
    click(&mut h, "icon:crop");
    click(&mut h, "button:cropTab-geometry");
    assert!(h.app.ui.crop_geometry);
    for w in ["button:upright-auto", "button:upright-level", "button:upright-vertical", "button:upright-full", "button:upright-guided"] {
        assert!(rect(&h, w).width() >= 43.9, "{w}");
    }
    for w in ["slider:optics.distortion", "slider:geometry.vertical", "slider:geometry.horizontal"] {
        assert!(has(&h, w), "{w}");
    }
    assert!(!has(&h, "dial:angle") && !has(&h, "icon:cropLock"), "the dial and the round buttons belong to Aspect");
    assert!(!h.app.widgets.iter().any(|(w, _)| w.starts_with("cropHandle:")), "no crop frame over the photo");
    click(&mut h, "button:upright-level");
    assert_eq!(h.app.session.develop_of(id).expect("settings").geometry.upright, lightcraft_develop::Upright::Level);
    click(&mut h, "button:upright-level");
    assert_eq!(h.app.session.develop_of(id).expect("settings").geometry.upright, lightcraft_develop::Upright::Off, "a second tap turns it off");
    click(&mut h, "button:cropTab-aspect");
    assert!(has(&h, "dial:angle") && has(&h, "cropHandle:0"), "back on Aspect, with the frame");
}

/// Held sideways the crop screen's title, panel and bar are a column on the right and the dial shares
/// a row with the round buttons under the photo.
#[test]
fn the_crop_screen_fits_a_phone_held_sideways() {
    let mut h = detail([844.0, 390.0]);
    click(&mut h, "icon:crop");
    let (cancel, done, dial, auto, photo) =
        (rect(&h, "icon:cropCancel"), rect(&h, "icon:cropDone"), rect(&h, "dial:angle"), rect(&h, "icon:cropAuto"), h.app.image_rect.expect("loupe"));
    assert!(cancel.left() > 844.0 - 330.0 && done.right() <= 844.0, "the bar is in the column on the right: {cancel:?} {done:?}");
    assert!(
        auto.center().y > photo.bottom() && (dial.center().y - auto.center().y).abs() < 30.0,
        "the dial and the buttons share a row under the photo"
    );
    assert!(photo.height() > 200.0, "the photo keeps its room: {photo:?}");
}

/// Remove on a phone: Remove / Heal / Clone are one segmented control.
#[test]
fn phone_remove_modes_are_a_segmented_control() {
    let mut h = detail([390.0, 1000.0]);
    h.app.ui.right = RightPanel::Remove;
    h.app.ui.sheet_detent = 2;
    h.settle(SETTLE);
    let (heal, clone) = (rect(&h, "button:removeMode-heal"), rect(&h, "button:removeMode-clone"));
    assert!((heal.center().y - clone.center().y).abs() < 0.5 && heal.height() >= 34.0, "{heal:?} {clone:?}");
    click(&mut h, "button:removeMode-clone");
    assert_eq!(h.app.ui.tool, "clone");
}

/// Masking on a phone: the ten kinds of mask are tiles across the whole width, folded away once a
/// mask exists, and the mask list has finger-high rows with the eye always shown.
#[test]
fn phone_masking_tiles_fill_the_width_and_the_list_is_finger_high() {
    let mut h = detail([390.0, 1000.0]);
    h.app.ui.right = RightPanel::Masking;
    h.app.ui.sheet_detent = 2;
    h.settle(SETTLE);
    let (first, last) = (rect(&h, "maskNew:object"), rect(&h, "maskNew:background"));
    assert!((first.center().y - last.center().y).abs() < 0.5, "five to a row: {first:?} {last:?}");
    assert!(last.right() > 390.0 - 24.0 && first.width() >= 60.0, "across the width: {first:?} {last:?}");
    click(&mut h, "maskNew:radial");
    let mask = h.app.session.develop_of(h.app.session.active().expect("photo")).expect("settings").masks[0].id;
    assert!(has(&h, "flyout:maskNew") && !has(&h, "maskNew:object"), "the kinds fold away once there is a mask");
    assert!(rect(&h, &format!("mask:{mask}")).height() >= 44.0);
    let eye = rect(&h, &format!("maskVisible:{mask}"));
    assert!(eye.width() >= 44.0 && eye.height() >= 44.0, "shown without a hover, and big enough: {eye:?}");
    // the folded row opens them again
    click(&mut h, "flyout:maskNew");
    assert!(has(&h, "maskNew:object"));
}

/// iPad portrait keeps the tools in a panel on the right: the compact rows and margins are used
/// there too, and there is no grabber.
#[test]
fn the_tablet_panel_has_no_grabber_but_the_same_rows() {
    let h = detail([820.0, 1180.0]);
    assert!(!has(&h, "sheet:grabber"));
    let (exposure, contrast) = (rect(&h, "slider:light.exposure"), rect(&h, "slider:light.contrast"));
    assert!((contrast.center().y - exposure.center().y - crate::widgets::TOUCH_SLIDER_ROW_H).abs() < 0.5);
}

/// Presets on a phone: the groups are chips and the chosen group's presets a row of thumbnails of
/// the photo that scrolls sideways, all in a short sheet; a tap on a tile applies the preset.
#[test]
fn phone_presets_are_a_strip_of_thumbnails_under_group_chips() {
    let mut h = detail([390.0, 844.0]);
    let id = h.app.session.active().expect("active photo");
    let r = h.request("engine.execute", json!({"command": "panel.presets"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    let chips =
        |h: &Headless| h.app.widgets.iter().filter(|(w, _)| w.starts_with("button:presetGroup:")).map(|(w, r)| (w.clone(), *r)).collect::<Vec<_>>();
    let tiles = |h: &Headless| h.app.widgets.iter().filter(|(w, _)| w.starts_with("preset:")).map(|(w, r)| (w.clone(), *r)).collect::<Vec<_>>();
    let chips_now = chips(&h);
    assert!(chips_now.len() >= 3, "a chip for each group: {chips_now:?}");
    let first_tiles = tiles(&h);
    assert!(first_tiles.len() >= 3, "{first_tiles:?}");
    let y = first_tiles[0].1.center().y;
    assert!(first_tiles.iter().all(|(_, r)| (r.center().y - y).abs() < 0.5 && r.width() >= 80.0), "one row of tiles: {first_tiles:?}");
    assert!(h.app.renderer.variant_textures() > 0, "the tiles show the photo with each preset");
    // another group's presets replace them
    click(&mut h, &chips_now[1].0);
    let other = tiles(&h);
    assert_ne!(other[0].0, first_tiles[0].0, "the second group's presets: {other:?}");
    // a tap applies one
    let before = format!("{:?}", h.app.session.develop_of(id));
    click(&mut h, &other[0].0);
    assert_ne!(format!("{:?}", h.app.session.develop_of(id)), before, "the preset was applied");
}

/// A phone held sideways: the tools are a rail down the left edge and the Edit groups sit at the
/// top of the panel on the right, so that the photo keeps the height and gets wider.
#[test]
fn a_phone_held_sideways_has_a_tool_rail_and_the_groups_in_the_panel() {
    let mut h = detail([750.0, 380.0]);
    let (presets, crop, edit) = (rect(&h, "icon:presets"), rect(&h, "icon:crop"), rect(&h, "icon:edit"));
    assert!(presets.right() <= 61.0 && edit.right() <= 61.0, "the rail is down the left edge: {presets:?} {edit:?}");
    assert!(presets.center().y < crop.center().y && crop.center().y < edit.center().y, "stacked, top to bottom");
    assert!(edit.bottom() <= 380.0, "all of them fit: {edit:?}");
    let (light, exposure) = (rect(&h, "button:group-light"), rect(&h, "slider:light.exposure"));
    assert!(light.left() > 380.0 && light.top() < 120.0, "the groups are at the top of the panel on the right: {light:?}");
    assert!(exposure.top() >= light.bottom() - 1.0 && exposure.left() > 380.0, "with the sliders under them: {exposure:?}");
    let photo = h.app.image_rect.expect("loupe");
    // (it was 275 × 184 with the bars along the bottom)
    assert!(photo.width() > 300.0 && photo.height() > 200.0, "the photo has the room the rail and the panel leave: {photo:?}");
    // choosing a group works from there, and tapping the open tool closes the panel
    click(&mut h, "button:group-color");
    assert_eq!(h.app.ui.edit_group, "color");
    assert!(has(&h, "slider:color.vibrance"));
    click(&mut h, "icon:edit");
    assert_eq!(h.app.ui.right, RightPanel::None);
}

/// Colour grading on a phone is one big wheel at a time, chosen with a segmented control.
#[test]
fn phone_color_grading_is_one_big_wheel_at_a_time() {
    let mut h = detail([390.0, 1400.0]);
    h.app.ui.edit_group = "color".into();
    h.app.ui.toggle_flyout("grading");
    h.app.ui.sheet_detent = 2;
    h.settle(SETTLE);
    let wheel = rect(&h, "wheel:midtones");
    assert!(wheel.width() >= 180.0, "a wheel a finger can use: {wheel:?}");
    assert!(!has(&h, "wheel:shadows") && !has(&h, "wheel:highlights"), "the others are one tap away");
    click(&mut h, "button:gradingRange-shadows");
    let wheel = rect(&h, "wheel:shadows");
    assert!(!has(&h, "wheel:midtones"));
    // a touch at the wheel's right edge: hue 0°, fully saturated
    let id = h.app.session.active().expect("active photo");
    let r = h.request("ui.click", json!({"x": wheel.right() - 2.0, "y": wheel.center().y}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    let shadows = h.app.session.develop_of(id).expect("settings").grading.shadows;
    assert!(shadows.sat > 90.0 && (shadows.hue < 5.0 || shadows.hue > 355.0), "{shadows:?}");
}

/// The colour mixer's eight swatches share the phone's width, about 40 pt each.
#[test]
fn phone_mixer_swatches_share_the_width() {
    let mut h = detail([390.0, 1400.0]);
    h.app.ui.edit_group = "color".into();
    h.app.ui.toggle_flyout("mixer");
    h.app.ui.sheet_detent = 2;
    h.settle(SETTLE);
    let (first, last) = (rect(&h, "mixerBand:red"), rect(&h, "mixerBand:magenta"));
    assert!(first.width() >= 36.0 && first.height() >= 36.0, "{first:?}");
    assert!(last.right() <= 390.0 - 15.0 && last.right() > 390.0 - 30.0, "across the whole width: {last:?}");
}

/// A curve's points are taken from a finger's width away (24 pt, the desktop's pointer takes 10), so
/// dragging near one moves it rather than adding another.
#[test]
fn phone_curve_points_are_taken_from_a_finger_away() {
    let mut h = detail([390.0, 1400.0]);
    h.app.ui.toggle_flyout("curve");
    h.app.ui.curve_channel = "master".into();
    h.app.ui.sheet_detent = 2;
    h.settle(SETTLE);
    let id = h.app.session.active().expect("active photo");
    let canvas = rect(&h, "curve");
    let at = |x: f32, y: f32| egui::pos2(canvas.left() + canvas.width() * x, canvas.bottom() - canvas.height() * y);
    let master = |h: &Headless| h.app.session.develop_of(id).expect("settings").curve.master.clone();
    // a tap adds a point at (0.5, 0.5)... (the curve's identity)
    let p = at(0.5, 0.5);
    let r = h.request("ui.click", json!({"x": p.x, "y": p.y}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert_eq!(master(&h).len(), 3, "a tap adds a point: {:?}", master(&h));
    // a drag that starts 18 pt from it takes it and moves it up
    let from = p + egui::vec2(18.0, 0.0);
    drag(&mut h, from, from + vec2(0.0, -60.0));
    let pts = master(&h);
    assert_eq!(pts.len(), 3, "no new point was made: {pts:?}");
    assert!(pts[1].y > 0.6, "the point moved up: {pts:?}");
}

/// A window too short for the bars (a phone's keyboard up in landscape, a tiny Slide Over) leaves
/// the sheet no room: it must still draw and its grabber still drag, with no panic (`clamp` panics
/// when its range is upside down).
#[test]
fn a_window_too_short_for_the_bars_does_not_panic() {
    let mut h = detail([390.0, 130.0]);
    if has(&h, "sheet:grabber") {
        let g = rect(&h, "sheet:grabber").center();
        drag(&mut h, g, g + vec2(0.0, -80.0));
        drag(&mut h, g, g + vec2(0.0, 80.0));
    }
    let r = h.request("ui.resize", json!({"width": 390, "height": 60}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
}

/// Each tool's sheet starts at its top: scrolling Crop doesn't leave Masking scrolled too.
#[test]
fn each_tool_starts_at_the_top_of_its_sheet() {
    let mut h = detail([390.0, 844.0]);
    click(&mut h, "icon:crop");
    let angle = rect(&h, "slider:crop.angle");
    let from = angle.center() - vec2(0.0, 6.0);
    drag(&mut h, from, from - vec2(4.0, 120.0));
    assert!(rect(&h, "slider:crop.angle").top() < angle.top() - 40.0, "Crop's sheet scrolled");
    click(&mut h, "icon:masking");
    let grabber = rect(&h, "sheet:grabber");
    let object = rect(&h, "maskNew:object");
    assert!(object.top() >= grabber.bottom() - 1.0, "Masking starts at its first row: {object:?} under {grabber:?}");
}

/// Settings on a phone is a list of pages (iOS's Settings app): no page's controls run past the
/// screen's edge.
#[test]
fn phone_settings_stay_inside_the_screen() {
    let mut h = detail([390.0, 844.0]);
    let r = h.request("engine.execute", json!({"command": "app.settings"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    for tab in ["general", "import", "performance", "interface", "sync"] {
        click(&mut h, &format!("button:settingsTab-{tab}"));
        let outside: Vec<_> = h
            .app
            .widgets
            .iter()
            .filter(|(id, _)| ["button:settings", "check:", "field:", "combo:", "label:sync"].iter().any(|p| id.starts_with(p)))
            .filter(|(_, r)| r.left() < -0.5 || r.right() > 390.5)
            .collect();
        assert!(outside.is_empty(), "{tab}: {outside:?}");
        // back to the list
        click(&mut h, "button:sheetCancel");
    }
}

/// An empty collection's message wraps inside a phone's width (it was one line that ran off both
/// edges of the screen).
#[test]
fn the_empty_grid_message_fits_a_phone() {
    let mut app = LightcraftApp::new(Session::new(), Services { png: None, ..Default::default() });
    app.ui.view = ViewMode::PhotoGrid;
    let mut h = Headless::new(app, [390.0, 844.0], 1.0);
    h.settle(SETTLE);
    let img = h.paint();
    let w = img.size[0];
    let background = img.pixels[300 * w];
    // the rows the message is on: nothing is drawn in the 6 px at either edge
    let touched: Vec<_> = (300..560usize).filter(|y| (0..6).chain(w - 6..w).any(|x| img.pixels[y * w + x] != background)).collect();
    assert!(touched.is_empty(), "the message is cut off at the screen's edges, rows {touched:?}");
}

/// A yes-or-no question on a phone is an iOS alert (a card in the middle with Cancel and the
/// action), not a page that covers the screen.
#[test]
fn deleting_photos_asks_with_an_alert_on_a_phone() {
    let mut h = detail([390.0, 844.0]);
    let id = h.app.session.active().expect("active photo");
    h.app.ui.dialog = Some(crate::state::Dialog::ConfirmDelete { count: 1 });
    h.settle(SETTLE);
    let card = rect(&h, "alert:dialog");
    assert!(card.width() <= 300.0 && card.height() < 300.0, "a card, not a page: {card:?}");
    assert!((card.center().x - 195.0).abs() < 2.0, "in the middle: {card:?}");
    assert!(!has(&h, "sheet:dialog"), "no page");
    assert!(rect(&h, "button:alertOk").center().x > rect(&h, "button:alertCancel").center().x, "Cancel on the left, the action on the right");
    click(&mut h, "button:alertCancel");
    assert!(h.app.ui.dialog.is_none() && !h.app.session.catalog.photo(id).expect("photo").deleted, "Cancel keeps the photo");
    h.app.ui.dialog = Some(crate::state::Dialog::ConfirmDelete { count: 1 });
    h.settle(SETTLE);
    click(&mut h, "button:alertOk");
    assert!(h.app.ui.dialog.is_none() && h.app.session.catalog.photo(id).expect("photo").deleted, "the action moves it to Recently Deleted");
}
