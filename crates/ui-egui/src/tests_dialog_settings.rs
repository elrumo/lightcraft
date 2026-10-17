//! The Settings window is one size in one place whatever section is open, and lies inside the app
//! window; a long folder path is cut short (its tooltip has all of it) and stays on one line beside
//! the buttons of its row.
//!
//! (This fork's Settings is a sidebar beside the open section, not a tab bar: its own window tests
//! are in `tests_dialog_fit`.)

use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::panels::settings::TABS;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);

/// The app window sizes tried: roomy, and the desktop minimum.
const SIZES: [[f32; 2]; 2] = [[1600.0, 1000.0], [900.0, 560.0]];

fn headless(session: lightcraft_engine::Session, size: [f32; 2]) -> Headless {
    let mut h = Headless::new(LightcraftApp::new(session, Services::default()), size, 1.0);
    // let the first renders finish (a render still running at exit can crash in GPU teardown)
    h.settle(T);
    h
}

fn demo(size: [f32; 2]) -> Headless {
    headless(lightcraft_engine::Session::with_demo(), size)
}

/// Given Settings open on `tab`, drawn until its layout has settled.
fn show(h: &mut Headless, tab: &str) {
    let r = h.request("engine.execute", json!({"command": "app.settings", "params": {"tab": tab}}), T);
    assert_eq!(r["ok"], true, "app.settings {tab}: {r}");
    h.settle(T);
    for _ in 0..4 {
        h.step();
    }
}

/// A folder under the system's temporary one, removed again when the test ends, passed or not.
struct TempFolder(std::path::PathBuf);

impl TempFolder {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("lc-ui-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Self(dir)
    }
}

impl Drop for TempFolder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn rect(h: &Headless, id: &str) -> egui::Rect {
    h.app.widgets.iter().find(|(w, _)| w == id).map(|(_, r)| *r).unwrap_or_else(|| panic!("no widget {id}"))
}

/// Same height and place from the top: the fork's Settings window is as wide as the page it shows needs (and a
/// scrolling page has a scroll bar), so a tab may be wider than another, but never taller or lower.
fn same(a: egui::Rect, b: egui::Rect) -> bool {
    (a.min.y - b.min.y).abs() < 0.5 && (a.max.y - b.max.y).abs() < 0.5
}

#[test]
fn the_window_keeps_its_size_and_place_from_tab_to_tab() {
    for size in SIZES {
        let mut h = demo(size);
        // Given the first tab, when every other tab is opened in turn
        show(&mut h, "general");
        let first = rect(&h, "dialog:window");
        for (tab, _) in TABS {
            show(&mut h, tab);
            // Then the window has not moved or changed size
            let now = rect(&h, "dialog:window");
            assert!(same(first, now), "{size:?}: Settings is {now:?} on {tab}, and was {first:?} on general");
        }
        // And going back to the first tab finds it as it was
        show(&mut h, "general");
        assert!(same(first, rect(&h, "dialog:window")), "{size:?}: general again");
    }
}

#[test]
fn a_long_library_folder_is_cut_short_and_its_tooltip_has_all_of_it() {
    // Given a library in a folder with a very long path
    let base = TempFolder::new("settings-window");
    let dir =
        base.0.join("photographs-from-2019-to-2026-kept-on-the-second-external-drive-that-nobody-wants-to-type-twice").join("the-master-library");
    std::fs::create_dir_all(&dir).unwrap();
    let mut session = lightcraft_engine::Session::new().with_fs();
    session.open_library(&dir, true).unwrap();
    let path = dir.display().to_string();

    let mut h = headless(session, [1600.0, 1000.0]);
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, h.size);
    // When Settings opens on General, which shows where the library is
    show(&mut h, "general");
    let general = rect(&h, "dialog:window");
    // Then the window stays inside the screen however long the path is
    assert!(screen.contains_rect(general), "Settings {general:?} runs off the {screen:?} window");
    // And the path is cut short, inside the window
    let location = rect(&h, "text:settingsLocation");
    let room = rect(&h, "settings:pageRoom");
    assert!(location.right() <= room.right() + 0.5, "the path {location:?} stays inside the room for the page {room:?}");
    let texts = h.painted_text();
    assert!(!texts.iter().any(|text| text.contains(&path)), "the whole path is not drawn in the row");
    // And what is left of it is its end, which says which library this is
    let name = "the-master-library";
    assert!(path.ends_with(name));
    assert!(
        texts.iter().any(|text| text.starts_with('…') && text.ends_with(name)),
        "the row keeps the end of the path: {:?}",
        texts.iter().filter(|t| t.contains("library")).collect::<Vec<_>>()
    );
    // And resting the pointer on it shows the whole path
    assert_eq!(h.request("ui.hoverWidget", json!({"id": "text:settingsLocation"}), T)["ok"], true);
    assert!(h.step_until(T, |h| h.painted_text().iter().any(|text| text.contains(&path))), "the tooltip has the whole path");

    // And the same holds for the folder Auto Import watches, which stays on one line
    let watched = h.app.run("library.autoImport", json!({"folder": path}));
    assert!(watched.is_ok(), "{watched:?}");
    show(&mut h, "import");
    // (the row is at the foot of the page: scrolled to, so that it is drawn however long the page is)
    let window = rect(&h, "dialog:window");
    h.request("ui.move", json!({"x": window.center().x, "y": window.center().y}), T);
    for _ in 0..6 {
        h.request("ui.scroll", json!({"dy": -400.0}), T);
        h.step();
    }
    h.settle(T);
    let (folder, room) = (rect(&h, "text:settingsAutoFolder"), rect(&h, "settings:pageRoom"));
    assert!(room.intersects(folder), "the watched folder {folder:?} is in view in {room:?}");
    assert!(folder.height() < 24.0 && folder.right() <= room.right(), "the watched folder {folder:?} is one line inside {room:?}");
    assert!(h.painted_text().iter().any(|text| text.starts_with('…') && text.ends_with(name)), "and keeps its end");
    drop(h);
}

#[test]
fn a_short_path_is_shown_whole_and_has_no_tooltip() {
    // Given the in-memory library, whose Location is a short sentence
    let mut h = demo([1600.0, 1000.0]);
    show(&mut h, "general");
    assert!(h.painted_text().iter().any(|text| text == "In memory — nothing is saved"), "it is all there");
    let before = h.painted_text().len();
    // When the pointer rests on it
    assert_eq!(h.request("ui.hoverWidget", json!({"id": "text:settingsLocation"}), T)["ok"], true);
    let _ = h.step_until(Duration::from_secs(2), |h| h.painted_text().len() > before);
    // Then nothing more is drawn: there is nothing a tooltip could add
    assert_eq!(h.painted_text().len(), before, "no tooltip for a path that is all there");
}
