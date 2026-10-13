//! The phone's albums sheet (Lightroom mobile's: tiles, counts under the names, ✕, Create New),
//! sheets that close when pulled down, albums carried into folders by a long press, adding library
//! photos to an album, and Delete's choice inside an album.

use std::time::Duration;

use egui::{Pos2, vec2};
use lightcraft_catalog::AlbumId;
use lightcraft_engine::{LibrarySource, Session};
use serde_json::json;

use crate::headless::Headless;
use crate::state::{Dialog, ViewMode};
use crate::tests_grid_touch::{button, click, has, rect_of, snap};
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(30);
const SETTLE: Duration = Duration::from_secs(120);

fn phone() -> Headless {
    let mut app = LightcraftApp::new(Session::with_demo(), Services { png: None, ..Default::default() });
    app.ui.view = ViewMode::SquareGrid;
    // (tall enough that every album row is clear of Create New)
    let mut h = Headless::new(app, [402.0, 1200.0], 1.0);
    h.settle(SETTLE);
    h
}

fn album(h: &Headless, name: &str) -> AlbumId {
    h.app.session.catalog.albums().find(|a| a.name == name).map(|a| a.id).unwrap()
}

fn open_albums(h: &mut Headless) {
    click(h, "button:collections");
    assert!(h.app.ui.left_panel && has(h, "sheet:albums"));
}

/// A finger (the mouse, here) pressed at `from`, moved to `to` over a few frames, then lifted.
fn pull(h: &mut Headless, from: Pos2, to: Pos2) {
    h.step_with([egui::Event::PointerMoved(from), button(true, from)]);
    for k in 1..=6 {
        h.step_with([egui::Event::PointerMoved(from.lerp(to, k as f32 / 6.0))]);
    }
    h.step_with([button(false, to)]);
    h.settle(SETTLE);
}

#[test]
fn the_albums_sheet_has_tiles_counts_a_close_button_and_create_new() {
    let mut h = phone();
    open_albums(&mut h);
    let garden = album(&h, "Garden");
    // a row is a tile and two lines: the count is under the name, not beside it
    let (name, count) = (rect_of(&h, "label:all"), rect_of(&h, "count:all"));
    assert!(count.top() > name.bottom() - 1.0 && (count.left() - name.left()).abs() < 1.0, "{name:?} {count:?}");
    assert!(has(&h, &format!("more:album:{}", garden.0)), "an album row has its …");
    snap(&h, "albums-sheet");
    // Create New offers the album kinds as a sheet
    click(&mut h, "button:createNew");
    assert!(has(&h, "actions:newAlbum") && has(&h, "button:newFolder"));
    snap(&h, "albums-create-new");
    click(&mut h, "button:newFolder");
    assert!(matches!(h.app.ui.dialog, Some(Dialog::NewAlbum { folder: true, .. })), "{:?}", h.app.ui.dialog);
    h.app.ui.dialog = None;
    h.settle(SETTLE);
    // the ✕ at the top right closes the sheet
    click(&mut h, "button:sheetCancel");
    assert!(!h.app.ui.left_panel);
}

#[test]
fn a_page_pulled_down_far_enough_closes_and_a_short_pull_springs_back() {
    let mut h = phone();
    open_albums(&mut h);
    // (at rest: done sliding in)
    for _ in 0..30 {
        h.step();
    }
    let sheet = rect_of(&h, "sheet:albums");
    let bar = sheet.center_top() + vec2(-60.0, 26.0);
    // a short pull: it follows the finger, then springs back
    h.step_with([egui::Event::PointerMoved(bar), button(true, bar)]);
    for k in 1..=4 {
        h.step_with([egui::Event::PointerMoved(bar + vec2(0.0, 15.0 * k as f32))]);
    }
    let followed = rect_of(&h, "sheet:albums").top() - sheet.top();
    snap(&h, "albums-pulled");
    assert!(followed > 30.0, "the sheet follows the finger ({followed})");
    // (held still a moment: let go slowly, not flicked)
    for _ in 0..12 {
        h.step();
    }
    h.step_with([button(false, bar + vec2(0.0, 60.0))]);
    // (a quarter of a second's spring back: `settle` may call it quiet sooner)
    for _ in 0..30 {
        h.step();
    }
    assert!(h.app.ui.left_panel, "a short pull doesn't close it");
    let back = rect_of(&h, "sheet:albums").top() - sheet.top();
    assert!(back.abs() < 1.0, "it springs back ({back})");
    // far enough: closed
    pull(&mut h, bar, bar + vec2(0.0, 300.0));
    assert!(!h.app.ui.left_panel, "pulled down, it closes");
}

#[test]
fn an_action_sheet_pulled_down_closes() {
    let mut h = phone();
    open_albums(&mut h);
    click(&mut h, "button:createNew");
    let sheet = rect_of(&h, "actions:newAlbum");
    let from = sheet.center_top() + vec2(0.0, 30.0);
    pull(&mut h, from, from + vec2(0.0, 200.0));
    assert!(!crate::panels::mobile::actions_open(&h.view.ctx, "newAlbum"));
    assert!(h.app.ui.left_panel, "only the action sheet closed");
}

#[test]
fn an_album_held_down_is_carried_into_a_folder() {
    let mut h = phone();
    open_albums(&mut h);
    let (garden, travel) = (album(&h, "Garden"), album(&h, "Travel 2026"));
    let at = rect_of(&h, &format!("source:album:{}", garden.0)).center();
    let into = rect_of(&h, &format!("source:folder:{}", travel.0)).center();
    h.step_with([egui::Event::PointerMoved(at), button(true, at)]);
    for _ in 0..20 {
        h.step();
    }
    assert!(h.app.ui.dragging_album.is_none(), "too soon: a slow tap is still a tap");
    for _ in 0..15 {
        h.step();
    }
    assert_eq!(h.app.ui.dragging_album, Some(garden.0), "held half a second, the album is picked up");
    for k in 1..=8 {
        h.step_with([egui::Event::PointerMoved(at.lerp(into, k as f32 / 8.0))]);
    }
    assert!(h.app.ui.left_panel, "carrying an album doesn't pull the sheet");
    snap(&h, "albums-carry");
    assert!(has(&h, "drag:album"), "its name rides over the finger");
    h.step_with([button(false, into)]);
    h.settle(SETTLE);
    let moved = h.app.session.catalog.album(garden).unwrap();
    assert_eq!(moved.parent, Some(travel), "dropped on the folder, it's inside it");
    assert_eq!(h.app.session.source, LibrarySource::All, "and the release didn't open it");
}

/// Let go where nothing takes it, a carried album goes back and the list scrolls again.
#[test]
fn an_album_let_go_over_nothing_is_put_back() {
    let mut h = phone();
    open_albums(&mut h);
    let garden = album(&h, "Garden");
    let at = rect_of(&h, &format!("source:album:{}", garden.0)).center();
    h.step_with([egui::Event::PointerMoved(at), button(true, at)]);
    for _ in 0..35 {
        h.step();
    }
    assert_eq!(h.app.ui.dragging_album, Some(garden.0));
    let away = at + vec2(0.0, 40.0);
    for k in 1..=4 {
        h.step_with([egui::Event::PointerMoved(at.lerp(away, k as f32 / 4.0))]);
    }
    h.step_with([button(false, away)]);
    h.settle(SETTLE);
    assert!(h.app.ui.dragging_album.is_none(), "the drag ended");
    assert_eq!(h.app.session.catalog.album(garden).unwrap().parent, None, "nothing moved");
}

#[test]
fn in_an_album_add_offers_lightcraft_and_add_puts_the_chosen_photos_in_it() {
    let mut h = phone();
    let garden = album(&h, "Garden");
    let r = h.request("engine.execute", json!({"command": "library.source", "params": {"kind": "album", "id": garden.0}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    let had = h.app.session.catalog.album(garden).unwrap().photos.clone();
    click(&mut h, "button:addPhotos");
    snap(&h, "album-add-menu");
    click(&mut h, "button:addFromLibrary");
    assert_eq!(h.app.ui.adding_to, Some(garden.0));
    assert!(h.app.ui.select_mode && h.app.session.source == LibrarySource::All, "the library to choose from");
    let pick = h.app.session.visible().iter().copied().find(|p| !had.contains(p)).unwrap();
    click(&mut h, &format!("thumb:{}", pick.0));
    snap(&h, "album-add-choosing");
    click(&mut h, "button:addToAlbum");
    assert!(h.app.session.catalog.album(garden).unwrap().photos.contains(&pick));
    assert!(!h.app.ui.select_mode && h.app.ui.adding_to.is_none());
    assert_eq!(h.app.session.source, LibrarySource::Album(garden), "back in the album");
}

#[test]
fn delete_in_an_album_offers_remove_or_delete_and_delete_asks_first() {
    let mut h = phone();
    let garden = album(&h, "Garden");
    let r = h.request("engine.execute", json!({"command": "library.source", "params": {"kind": "album", "id": garden.0}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    let photos = h.app.session.catalog.album(garden).unwrap().photos.clone();
    click(&mut h, "button:select");
    click(&mut h, &format!("thumb:{}", photos[0].0));
    click(&mut h, "icon:selDelete");
    assert!(has(&h, "button:removeFromAlbum") && has(&h, "button:deleteFromLibrary"));
    snap(&h, "album-delete-choice");
    click(&mut h, "button:removeFromAlbum");
    assert!(!h.app.session.catalog.album(garden).unwrap().photos.contains(&photos[0]), "out of the album");
    assert!(!h.app.session.catalog.photo(photos[0]).unwrap().deleted, "but still in the library");
    click(&mut h, &format!("thumb:{}", photos[1].0));
    click(&mut h, "icon:selDelete");
    click(&mut h, "button:deleteFromLibrary");
    assert_eq!(h.app.ui.dialog, Some(Dialog::ConfirmDelete { count: 1 }), "deleting asks first");
    assert!(!h.app.session.catalog.photo(photos[1]).unwrap().deleted);
    snap(&h, "album-delete-confirm");
    let r = h.request("ui.dialog.confirm", json!({}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert!(h.app.session.catalog.photo(photos[1]).unwrap().deleted);
}

#[test]
fn delete_outside_an_album_asks_first() {
    let mut h = phone();
    click(&mut h, "button:select");
    let id = h.app.session.visible()[0];
    click(&mut h, &format!("thumb:{}", id.0));
    click(&mut h, "icon:selDelete");
    assert!(!has(&h, "button:removeFromAlbum"), "no album to take it out of");
    assert_eq!(h.app.ui.dialog, Some(Dialog::ConfirmDelete { count: 1 }));
    assert!(!h.app.session.catalog.photo(id).unwrap().deleted);
}
