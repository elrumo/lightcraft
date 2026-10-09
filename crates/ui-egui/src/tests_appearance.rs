//! Headless tests of the light / dark / system appearance of the phone layout.

use std::time::Duration;

use egui::Color32;
use lightcraft_engine::Session;
use serde_json::json;

use crate::headless::Headless;
use crate::state::{Appearance, RightPanel, ViewMode};
use crate::theme::Tokens;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(30);
const SETTLE: Duration = Duration::from_secs(120);

fn phone() -> Headless {
    let app = LightcraftApp::new(Session::with_demo(), Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [390.0, 844.0], 1.0);
    h.settle(SETTLE);
    let r = h.request("engine.execute", json!({"command": "library.select", "params": {"ids": [1]}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.app.ui.view = ViewMode::Detail;
    h.app.ui.right = RightPanel::Edit;
    h.settle(SETTLE);
    h
}

fn run(h: &mut Headless, command: &str) {
    let r = h.request("engine.execute", json!({"command": command}), T);
    assert_eq!(r["ok"], true, "{command}: {r}");
    h.settle(SETTLE);
}

/// Where the phone's background is drawn: a pixel in the top bar's empty middle.
fn bar_pixel(h: &mut Headless) -> Color32 {
    let img = h.paint();
    let (w, y) = (img.size[0], 12);
    img.pixels[y * w + w / 2]
}

/// "System" follows what the host says in `RawInput::system_theme` (dark when it says nothing, as the
/// app always was); Light and Dark ignore it; the desktop layout is dark whatever.
#[test]
fn the_phone_follows_the_setting_and_the_system() {
    let mut h = phone();
    assert_eq!(h.app.ui.appearance, Appearance::System);
    assert!(h.app.dark && Tokens::is_dark(&h.view.ctx), "no word from the host: dark");
    assert_eq!(Tokens::get(&h.view.ctx).canvas, Color32::BLACK);
    // the system says light, and the phone is light
    h.system_theme = Some(egui::Theme::Light);
    h.settle(SETTLE);
    assert!(!h.app.dark && !Tokens::is_dark(&h.view.ctx));
    assert_eq!(Tokens::get(&h.view.ctx).canvas, Color32::WHITE);
    assert_eq!(bar_pixel(&mut h), Color32::WHITE, "drawn light");
    // Dark ignores the system; Light ignores it the other way
    run(&mut h, "app.appearance.dark");
    assert_eq!(h.app.ui.appearance, Appearance::Dark);
    assert!(h.app.dark);
    assert_eq!(bar_pixel(&mut h), Color32::BLACK, "drawn dark");
    h.system_theme = Some(egui::Theme::Dark);
    run(&mut h, "app.appearance.light");
    assert!(!h.app.dark);
    assert_eq!(bar_pixel(&mut h), Color32::WHITE);
    // back to the system: dark again
    run(&mut h, "app.appearance.system");
    assert!(h.app.dark);
    // the desktop layout is dark whatever the setting
    run(&mut h, "app.appearance.light");
    let r = h.request("ui.resize", json!({"width": 1200, "height": 800}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert!(!h.app.compact && h.app.dark && Tokens::is_dark(&h.view.ctx), "the desktop layout stays dark");
    assert_eq!(Tokens::get(&h.view.ctx).canvas, Tokens::default().canvas);
}

/// The sheet's rounded top corners show the photo's canvas through them, whatever the window was
/// cleared to (the headless renderer clears to near black, the iOS host to the bars' colour): a
/// black wedge in each corner of a light sheet was this.
#[test]
fn the_sheets_rounded_corners_show_the_canvas() {
    let mut h = phone();
    run(&mut h, "app.appearance.light");
    let img = h.paint();
    let top = h.app.widgets.iter().find(|(w, _)| w == "sheet:grabber").map(|(_, r)| r.top()).expect("the sheet is up");
    // the far left of the sheet's second row is outside its 12 pt arc
    let y = top.ceil() as usize + 1;
    assert_eq!(img.pixels[y * img.size[0]], Color32::WHITE, "a light sheet's corner is the canvas, not the clear colour");
}

/// The setting is a command (menu, All Commands, control channel, MCP), checked in the menu, in the
/// UI state the host saves.
#[test]
fn the_appearance_is_a_command_and_is_saved_with_the_ui_state() {
    let mut h = phone();
    let r = h.request("engine.execute", json!({"command": "app.appearance.light"}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(r["result"], "light");
    assert!(crate::menubar::checked(&h.app, "app.appearance.light") == Some(true));
    assert!(crate::menubar::checked(&h.app, "app.appearance.dark") == Some(false));
    let saved = serde_json::to_value(&h.app.ui).expect("ui state");
    assert_eq!(saved["appearance"], "light");
    // set through the control channel's ui.set as well, and read back from a saved state
    let r = h.request("ui.set", json!({"appearance": "dark"}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(h.app.ui.appearance, Appearance::Dark);
    let again: crate::UiState = serde_json::from_value(serde_json::to_value(&h.app.ui).expect("ui state")).expect("round trip");
    assert_eq!(again.appearance, Appearance::Dark);
    // a state saved before the setting existed starts at System
    let old: crate::UiState = serde_json::from_value(json!({"view": "detail"})).expect("old state");
    assert_eq!(old.appearance, Appearance::System);
}

/// Settings ▸ Interface has the choice on a phone, and it applies at once.
#[test]
fn settings_has_an_appearance_choice() {
    let mut h = phone();
    run(&mut h, "app.settings");
    // the list of pages, the Interface page, the Appearance row's menu, its second choice
    for id in ["button:settingsTab-interface", "button:settingsAppearance", "button:settingsAppearance-1"] {
        let r = h.request("ui.clickWidget", json!({"id": id}), T);
        assert_eq!(r["ok"], true, "{id}: {r}");
        h.settle(SETTLE);
    }
    assert_eq!(h.app.ui.appearance, Appearance::Light, "the second choice is Light");
    assert!(!Tokens::is_dark(&h.view.ctx));
}
