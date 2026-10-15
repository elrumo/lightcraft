//! Issue #781: a dialog taller than the window keeps its title and buttons on screen; its options
//! scroll. On a 1366×768 screen the Export dialog ran off both ends and couldn't be confirmed.

use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);

fn demo(size: [f32; 2]) -> Headless {
    let services = Services { png: None, write_shared: Some(std::sync::Arc::new(|_: &str, _: &[u8]| Ok(()))), ..Default::default() };
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), services);
    let mut h = Headless::new(app, size, 1.0);
    // let the first renders finish (a render still running at exit can crash in GPU teardown)
    h.settle(T);
    h
}

fn rect(h: &Headless, id: &str) -> egui::Rect {
    h.app.widgets.iter().find(|(w, _)| w == id).map(|(_, r)| *r).unwrap_or_else(|| panic!("no widget {id}"))
}

fn open(h: &mut Headless, command: &str) {
    let r = h.request("engine.execute", json!({"command": command, "params": {}}), T);
    assert_eq!(r["ok"], true, "{command}: {r}");
    assert!(h.app.ui.dialog.is_some(), "{command} opens a dialog");
    h.settle(T);
}

/// The dialog lies inside the window, and so do its buttons (`button:dialogCancel` when it has one).
fn assert_fits(h: &Headless, what: &str) {
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, h.size);
    let dialog = rect(h, "dialog:window");
    assert!(screen.contains_rect(dialog), "{what}: dialog {dialog:?} runs off the {screen:?} window");
    let mut buttons = vec!["button:dialogOk"];
    if h.app.widgets.iter().any(|(w, _)| w == "button:dialogCancel") {
        buttons.push("button:dialogCancel");
    }
    for b in buttons {
        let r = rect(h, b);
        assert!(dialog.contains_rect(r), "{what}: {b} {r:?} is outside the dialog {dialog:?}");
    }
}

/// The Export dialog in a window of `size`, set to write small JPEGs to a made-up folder.
fn export_dialog(size: [f32; 2]) -> Headless {
    let mut h = demo(size);
    open(&mut h, "dialog.export");
    if let Some(crate::state::Dialog::Export { full_size, resize, dir, .. }) = &mut h.app.ui.dialog {
        *full_size = false;
        *resize = lightcraft_engine::export::Resize::long_edge(64);
        *dir = "/lc-test-out".into();
    }
    h.settle(T);
    h
}

// Scenario: on a 1366×768 screen (and a shorter window) the Export dialog's buttons are on screen and work
#[test]
fn export_dialog_fits_a_short_window_and_its_button_exports() {
    for size in [[1366.0, 690.0], [1366.0, 520.0]] {
        let mut h = export_dialog(size);
        assert_fits(&h, &format!("Export at {size:?}"));
        let r = h.request("ui.clickWidget", json!({"id": "button:dialogOk"}), T);
        assert_eq!(r["ok"], true, "{r}");
        h.step();
        h.step();
        assert!(h.app.ui.dialog.is_none(), "Export closed the dialog at {size:?}");
        assert!(h.app.export.is_some() || h.app.last_export_result.is_some(), "Export started at {size:?}");
        assert!(h.step_until(Duration::from_secs(60), |h| h.app.export.is_none()));
    }
}

// Scenario: Cancel is on screen too, and closes the dialog
#[test]
fn export_dialog_cancel_is_on_screen_in_a_short_window() {
    let mut h = export_dialog([1366.0, 690.0]);
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, h.size);
    assert!(screen.contains_rect(rect(&h, "button:dialogCancel")));
    h.request("ui.clickWidget", json!({"id": "button:dialogCancel"}), T);
    h.step();
    h.step();
    assert!(h.app.ui.dialog.is_none());
    assert!(h.app.export.is_none() && h.app.last_export_result.is_none(), "nothing exported");
}

// Scenario: a window with room for it shows the Export dialog as before (no scrolling, same size)
#[test]
fn export_dialog_is_unchanged_in_a_tall_window() {
    let h = export_dialog([1600.0, 1000.0]);
    assert_fits(&h, "Export at 1600×1000");
    let dialog = rect(&h, "dialog:window");
    // (898 tall: the whole dialog, nothing scrolled away, as before #781 plus 12 since its two text
    // boxes got roomier, plus the Export to row since #956 and 8 since its footer's buttons are the
    // button component's medium size, 30 points, as the welcome's and every dialog's; 615 wide
    // since the format row gained JPEG XL, #780; 30 more on macOS for its Add to Apple Photos row)
    // (this fork's footer buttons are 4 points shorter than upstream's: 894 and 924)
    let tall = if cfg!(target_os = "macos") { 924.0 } else { 894.0 };
    assert!((dialog.height() - tall).abs() <= 1.0, "{dialog:?}");
    assert!((dialog.width() - 615.2).abs() <= 1.0, "{dialog:?}");
}

// Scenario: other long dialogs fit a short window too, with their buttons on screen
#[test]
fn long_dialogs_fit_a_short_window() {
    let mut h = demo([1000.0, 320.0]);
    for command in [
        "dialog.copySettings",
        "dialog.pasteSettings",
        "dialog.createPreset",
        "dialog.contactSheet",
        "app.settings",
        "app.shortcuts",
        "app.whatsNew",
        "app.about",
        "app.systemInfo",
        "dialog.smartAlbum",
    ] {
        open(&mut h, command);
        assert_fits(&h, command);
        h.app.ui.dialog = None;
        h.settle(T);
    }
}

// Scenario: a list that scrolls on its own (the shortcuts) gets shorter instead of scrolling inside a scrolling dialog
#[test]
fn a_dialog_list_shrinks_to_fit_a_short_window() {
    let mut h = demo([1000.0, 320.0]);
    open(&mut h, "app.shortcuts");
    let shrink = h.view.ctx.data(|d| d.get_temp::<f32>(egui::Id::new("dialog-list-shrink"))).unwrap_or(0.0);
    assert!(shrink > 0.0, "the list gave up room");
    let dialog = rect(&h, "dialog:window");
    assert!(dialog.height() <= 320.0, "{dialog:?}");
}

// Scenario: Settings lists its sections in a sidebar beside the open one; a setting's control sits
// at the end of its row, a switch row toggles on a click anywhere on it, the section scrolls on its
// own while the sidebar stays put, and its text boxes have room around their text
#[test]
fn settings_sections_sit_in_a_sidebar_beside_the_open_one() {
    let mut h = demo([1300.0, 820.0]);
    open(&mut h, "app.settings");
    assert_fits(&h, "Settings");
    let ids = ["general", "interface", "display", "import", "performance", "faces", "denoise", "models", "sync"];
    let tabs: Vec<egui::Rect> = ids.iter().map(|id| rect(&h, &format!("button:settingsTab-{id}"))).collect();
    for (pair, id) in tabs.windows(2).zip(ids.iter().skip(1)) {
        assert!(pair[1].top() >= pair[0].bottom() - 0.5 && (pair[1].left() - pair[0].left()).abs() < 0.5, "{id} under the one before");
    }
    let dialog = rect(&h, "dialog:window");
    assert!(tabs[0].left() - dialog.left() < 40.0, "the sidebar is on the dialog's left: {:?} in {dialog:?}", tabs[0]);
    // the open section is right of the sidebar, its controls at the end of their rows
    let confirm = rect(&h, "check:settings.confirmDelete");
    assert!(confirm.left() > tabs[0].right() + 10.0, "{confirm:?}");
    let detail = rect(&h, "button:settingsStartup-2");
    assert!((detail.right() - confirm.right()).abs() <= 1.0, "Startup's choices end where the switch row does: {detail:?} {confirm:?}");
    // a text box has room around its text (egui's own is about 21 pt tall)
    assert!(rect(&h, "field:photocraftPath").height() >= 26.0, "{:?}", rect(&h, "field:photocraftPath"));
    // the section scrolls; the sidebar doesn't
    h.request("ui.hoverWidget", json!({"id": "check:settings.confirmDelete"}), T);
    h.request("ui.scroll", json!({"dy": -200.0}), T);
    h.settle(T);
    assert!(rect(&h, "check:settings.confirmDelete").top() < confirm.top() - 50.0, "the section scrolled");
    assert_eq!(rect(&h, "button:settingsTab-general"), tabs[0], "the sidebar stayed put");
    // a click anywhere on a switch row toggles it
    let before = h.app.ui.settings.confirm_delete;
    assert_eq!(h.request("ui.clickWidget", json!({"id": "check:settings.confirmDelete"}), T)["ok"], true);
    h.step();
    assert_ne!(h.app.ui.settings.confirm_delete, before);
    // and the sidebar opens a section
    assert_eq!(h.request("ui.clickWidget", json!({"id": "button:settingsTab-performance"}), T)["ok"], true);
    h.step();
    assert_eq!(h.app.ui.dialog, Some(crate::state::Dialog::Settings { tab: "performance".into() }));
}

/// Settings ▸ Performance: the GPU status line above the controls stays one line whatever it says
/// (without a driver it is a long wgpu message, e.g. on the FreeBSD CI), so the controls below it
/// don't move when the process-wide GPU state changes between two frames (a click aimed at
/// "Memory for caches" then landed on the gap above it).
#[test]
fn gpu_status_never_moves_the_performance_controls() {
    let mut h = demo([1300.0, 820.0]);
    let r = h.request("ui.key", json!({"key": ",", "cmd": true}), T);
    assert_eq!(r["ok"], true, "{r}");
    let r = h.request("ui.clickWidget", json!({"id": "button:settingsTab-performance"}), T);
    assert_eq!(r["ok"], true, "{r}");
    let mut seen = vec![];
    for gpu in [true, false, true] {
        // the setting is applied process-wide at the next frame (`app.gpu`), as other tests do
        h.app.ui.settings.gpu = gpu;
        h.step();
        h.step();
        seen.push((gpu, rect(&h, "button:settingsMemory-2"), lightcraft_engine::gpu::unavailable_reason()));
    }
    let first = seen[0].1;
    assert!(seen.iter().all(|(_, r, _)| *r == first), "the memory buttons moved: {seen:?}");
}
