//! The phone's photo grid by touch: a long press starts choosing photos while the finger is still
//! down, and the drag that follows chooses a run of them.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use lightcraft_engine::Session;
use serde_json::json;

use crate::haptics::Haptic;
use crate::headless::Headless;
use crate::state::ViewMode;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(30);
const SETTLE: Duration = Duration::from_secs(120);

pub(crate) fn grid(size: [f32; 2]) -> Headless {
    let mut app = LightcraftApp::new(Session::with_demo(), Services { png: None, ..Default::default() });
    app.ui.view = ViewMode::SquareGrid;
    let mut h = Headless::new(app, size, 1.0);
    h.settle(SETTLE);
    h
}

pub(crate) fn has(h: &Headless, id: &str) -> bool {
    h.app.widgets.iter().any(|(w, _)| w == id)
}

/// Where a widget is drawn.
pub(crate) fn rect_of(h: &Headless, id: &str) -> egui::Rect {
    h.app.widgets.iter().find(|(w, _)| w == id).map(|(_, r)| *r).unwrap_or_else(|| panic!("{id} is not on screen"))
}

pub(crate) fn click(h: &mut Headless, id: &str) {
    let r = h.request("ui.clickWidget", json!({"id": id}), T);
    assert_eq!(r["ok"], true, "{id}: {r}");
    h.settle(SETTLE);
}

pub(crate) fn button(pressed: bool, pos: egui::Pos2) -> egui::Event {
    egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE }
}

/// With `LIGHTCRAFT_SNAP_DIR` set, write what the last frame shows to `<dir>/<name>.png` (to look at
/// the screens these tests drive; nothing is written otherwise).
pub(crate) fn snap(h: &Headless, name: &str) {
    let Some(dir) = std::env::var_os("LIGHTCRAFT_SNAP_DIR") else { return };
    let img = h.paint();
    let data: Vec<[u8; 4]> = img.pixels.iter().map(|c| c.to_array()).collect();
    let rgba = lightcraft_raster::Rgba8 { width: img.size[0], height: img.size[1], data };
    let png = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&rgba), &lightcraft_codecs::EncodeMeta::default()).unwrap();
    std::fs::write(std::path::Path::new(&dir).join(format!("{name}.png")), png).unwrap();
}

fn frames(h: &mut Headless, n: u32) {
    for _ in 0..n {
        h.step();
    }
}

fn chosen(h: &Headless) -> Vec<u64> {
    h.app.session.selection.ids.iter().map(|i| i.0).collect()
}

/// iOS Photos: half a second's press on a photo starts choosing with it (with a firm tap of the
/// haptics) before the finger lifts; sliding on from there chooses the photos passed over, and
/// lifting the finger neither opens the photo nor unchooses it.
#[test]
fn a_long_press_starts_choosing_while_the_finger_is_still_down() {
    let mut h = grid([390.0, 844.0]);
    let log = Rc::new(RefCell::new(Vec::new()));
    let sink = log.clone();
    h.app.services.haptic = Some(Box::new(move |t| sink.borrow_mut().push(t)));
    let ids: Vec<u64> = h.app.session.visible().iter().take(3).map(|id| id.0).collect();
    let a = rect_of(&h, &format!("thumb:{}", ids[0])).center();
    h.step_with([egui::Event::PointerMoved(a), button(true, a)]);
    // a third of a second: nothing yet (a slow tap is still a tap)
    frames(&mut h, 20);
    assert!(!h.app.ui.select_mode, "too soon");
    frames(&mut h, 15);
    assert!(h.app.ui.select_mode, "half a second in, with the finger still down, choosing has started");
    assert_eq!(chosen(&h), [ids[0]]);
    assert!(log.borrow().contains(&Haptic::Medium), "{:?}", log.borrow());
    // the drag that follows chooses the run to the photo under the finger (a scroll would not)
    let c = rect_of(&h, &format!("thumb:{}", ids[2])).center();
    h.step_with([egui::Event::PointerMoved(a.lerp(c, 0.5))]);
    h.step_with([egui::Event::PointerMoved(c)]);
    assert_eq!(chosen(&h), ids, "the run from the pressed photo to the one under the finger");
    h.step_with([button(false, c)]);
    h.settle(SETTLE);
    assert_eq!(h.app.ui.view, ViewMode::SquareGrid, "lifting the finger opened nothing");
    assert!(h.app.ui.select_mode);
    assert_eq!(chosen(&h), ids, "and unchose nothing");
}

/// A long press while choosing takes the pressed photo out (or puts it in), and a tap after it is a
/// tap again.
#[test]
fn a_long_press_while_choosing_toggles_that_photo_and_the_next_tap_still_counts() {
    let mut h = grid([390.0, 844.0]);
    click(&mut h, "button:select");
    let ids: Vec<u64> = h.app.session.visible().iter().take(2).map(|id| id.0).collect();
    click(&mut h, &format!("thumb:{}", ids[0]));
    assert_eq!(chosen(&h), [ids[0]]);
    let a = rect_of(&h, &format!("thumb:{}", ids[0])).center();
    h.step_with([egui::Event::PointerMoved(a), button(true, a)]);
    frames(&mut h, 40);
    h.step_with([button(false, a)]);
    h.settle(SETTLE);
    assert!(chosen(&h).is_empty(), "the long press took it out, and its release did not put it back");
    click(&mut h, &format!("thumb:{}", ids[1]));
    assert_eq!(chosen(&h), [ids[1]], "an ordinary tap chooses again");
}

/// A finger that moves before the half second is up is scrolling, not pressing.
#[test]
fn a_finger_that_moves_is_not_a_long_press() {
    let mut h = grid([390.0, 844.0]);
    let id = h.app.session.visible()[0].0;
    let a = rect_of(&h, &format!("thumb:{id}")).center();
    h.step_with([egui::Event::PointerMoved(a), button(true, a)]);
    h.step_with([egui::Event::PointerMoved(a + egui::vec2(0.0, 30.0))]);
    frames(&mut h, 45);
    h.step_with([button(false, a + egui::vec2(0.0, 30.0))]);
    h.settle(SETTLE);
    assert!(!h.app.ui.select_mode);
}

/// The grid's filter button opens the Filter page: rating (with its comparison), flags and labels
/// at the top with the number of photos shown, then the lists (Type, Camera, People, Location,
/// Keyword, Edited), each a page of choices that comes back once one is picked; Clear resets.
#[test]
fn the_filter_button_opens_the_filter_page_and_its_lists() {
    let mut h = grid([390.0, 844.0]);
    let all = h.app.session.visible().len();
    let first = h.app.session.visible()[0].0;
    let r = h.request("engine.execute", json!({"command": "photo.setMeta", "params": {"ids": [first], "city": "Sitges", "country": "Spain"}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.app.run("photo.rate", json!({"ids": [first], "rating": 4})).unwrap();
    click(&mut h, "icon:filter");
    assert_eq!(h.app.ui.filter_sheet.as_deref(), Some(""));
    assert!(has(&h, "sheet:filter") && has(&h, "filter:star5") && has(&h, "filter:pick") && has(&h, "filter:label-red"));
    for (id, _, _) in crate::panels::filter_sheet::LISTS {
        assert!(has(&h, &format!("button:filterList-{id}")), "{id}");
    }
    assert!(h.painted_text().contains(&all.to_string()), "the rows count the photos shown");
    snap(&h, "filter-page");
    // stars, then the comparison
    click(&mut h, "filter:star4");
    assert_eq!(h.app.session.filter.rating, 4);
    let rated = h.app.session.catalog.photos().filter(|p| p.in_library() && p.rating >= 4).count();
    assert!(h.app.session.visible().iter().any(|i| i.0 == first));
    assert_eq!(h.app.session.visible().len(), rated);
    assert!(h.painted_text().contains(&rated.to_string()), "the count follows");
    click(&mut h, "filter:ratingOp");
    assert_eq!(h.app.session.filter.rating_op, lightcraft_catalog::RatingOp::Exactly);
    click(&mut h, "filter:star4");
    assert_eq!(h.app.session.filter.rating, 0, "a tap on the chosen star clears it");
    // a flag, on and off
    click(&mut h, "filter:reject");
    assert_eq!(h.app.session.filter.flag, Some(lightcraft_catalog::Flag::Reject));
    click(&mut h, "filter:reject");
    assert_eq!(h.app.session.filter.flag, None);
    // Location: the places named in the photos, with their counts
    click(&mut h, "button:filterList-location");
    assert_eq!(h.app.ui.filter_sheet.as_deref(), Some("location"));
    assert!(h.painted_text().iter().any(|s| s == "Sitges"), "{:?}", h.painted_text());
    let places = h.app.caches.filter_choices(&h.app.session.catalog);
    assert!(places.places.iter().any(|(name, n)| name == "Sitges" && *n == 1), "Sitges, one photo");
    assert!(has(&h, "button:filterPick-location-0"), "Any comes first");
    // a long list is narrowed by typing
    assert!(places.places.len() > 12 && has(&h, "field:filterSearch"));
    click(&mut h, "field:filterSearch");
    let r = h.request("ui.text", json!({"text": "sitg"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert!(!has(&h, "button:filterPick-location-2"), "only Any and Sitges are left");
    snap(&h, "filter-location");
    click(&mut h, "button:filterPick-location-1");
    assert_eq!(h.app.ui.filter_sheet.as_deref(), Some(""), "back to the first list");
    assert_eq!(h.app.session.filter.place.as_deref(), Some("Sitges"));
    assert_eq!(h.app.session.visible().iter().map(|i| i.0).collect::<Vec<_>>(), [first], "{:?}", h.app.session.filter);
    // Type, then back without choosing
    click(&mut h, "button:filterList-type");
    click(&mut h, "button:filterBack");
    assert_eq!(h.app.ui.filter_sheet.as_deref(), Some(""));
    click(&mut h, "button:filterList-edited");
    click(&mut h, "button:filterPick-edited-2");
    assert_eq!(h.app.session.filter.edited, Some(false));
    // Clear, then close
    click(&mut h, "button:filterClear");
    assert_eq!(h.app.session.filter, lightcraft_catalog::Filter::default());
    assert_eq!(h.app.session.visible().len(), all);
    click(&mut h, "button:filterBack");
    assert_eq!(h.app.ui.filter_sheet, None);
    h.settle(SETTLE);
    assert!(!has(&h, "sheet:filter"), "the page slid away");
}

/// The grid's scroll position (the top of what is on screen, in the grid's own points).
fn offset(h: &Headless) -> f32 {
    h.app.grid_scroll.unwrap_or(f32::NAN)
}

/// The phone grid's scroll handle shows while the grid moves, and dragging it runs through the
/// whole grid (its own taps never reach the photos under it); a tap on the top bar between its
/// buttons slides back to the top (`view.scrollToTop`, which iOS's status-bar tap also runs).
#[test]
fn the_scroll_handle_runs_through_the_grid_and_the_top_bar_goes_back_up() {
    let mut h = grid([390.0, 520.0]);
    assert!(!has(&h, "grid:scrubber"), "hidden until the grid moves");
    let middle = egui::pos2(195.0, 300.0);
    h.step_with([egui::Event::PointerMoved(middle)]);
    let r = h.request("ui.scroll", json!({"dy": -200.0}), T);
    assert_eq!(r["ok"], true, "{r}");
    frames(&mut h, 20);
    assert!(offset(&h) > 50.0, "{}", offset(&h));
    assert!(has(&h, "grid:scrubber"), "it shows while the grid moves");
    let handle = rect_of(&h, "grid:scrubber");
    let at = handle.center();
    let bottom = egui::pos2(at.x, 520.0);
    h.step_with([egui::Event::PointerMoved(at), button(true, at)]);
    for k in 1..=10 {
        h.step_with([egui::Event::PointerMoved(at.lerp(bottom, k as f32 / 10.0))]);
    }
    h.step_with([button(false, bottom)]);
    frames(&mut h, 4);
    let low = offset(&h);
    assert!(!h.app.ui.select_mode && h.app.ui.view == ViewMode::SquareGrid, "the photos under it saw nothing");
    snap(&h, "grid-scrubbed");
    // the end of the grid: scrolling further by hand moves no more
    h.step_with([egui::Event::PointerMoved(middle)]);
    let r = h.request("ui.scroll", json!({"dy": -5000.0}), T);
    assert_eq!(r["ok"], true, "{r}");
    frames(&mut h, 30);
    assert!((offset(&h) - low).abs() < 2.0, "the handle took the grid to its end: {low} vs {}", offset(&h));
    // the top bar's empty middle goes back up
    click(&mut h, "bar:top");
    frames(&mut h, 30);
    assert!(offset(&h) < 1.0, "{}", offset(&h));
    // in the far future the handle has gone again
    frames(&mut h, 120);
    assert!(!has(&h, "grid:scrubber"));
}
