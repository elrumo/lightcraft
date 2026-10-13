//! Appearance Mode: Auto (follow the system), Dark or Light, each with its own saved theme (the
//! desktop layout); and the phone layout's own light / dark / system appearance ([`phone`]).

use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::state::{AppSettings, AppearanceMode, DarkTheme, LightTheme, UiState};
use crate::theme::ThemeKind;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);

fn app() -> LightcraftApp {
    LightcraftApp::new(lightcraft_engine::Session::new(), Services { png: None, ..Default::default() })
}

/// One frame of logic on `ctx`: where the theme is applied.
fn frame(app: &mut LightcraftApp, ctx: &egui::Context) {
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| app.logic(ui.ctx()));
    output.textures_delta.clear();
}

#[test]
fn new_users_keep_the_dark_look_and_old_settings_load_unchanged() {
    let s = AppSettings::default();
    assert_eq!((s.appearance_mode, s.dark_theme, s.light_theme), (AppearanceMode::Dark, DarkTheme::Charcoal, LightTheme::Silver));
    assert_eq!(s.theme(Some(egui::Theme::Light)), ThemeKind::Charcoal, "Auto is opt-in");
    // a ui.json saved before appearance modes existed
    let old: UiState = serde_json::from_str(r#"{"settings": {"gridBadges": "always", "confirmDelete": true}}"#).unwrap();
    assert_eq!(old.settings.appearance_mode, AppearanceMode::Dark);
    assert_eq!(old.settings.dark_theme, DarkTheme::Charcoal);
    assert_eq!(old.settings.light_theme, LightTheme::Silver);
    assert!(old.settings.confirm_delete);
    // and the Charcoal tokens are the ones LightCraft always had
    assert_eq!(crate::theme::Tokens::for_kind(ThemeKind::Charcoal).chrome, crate::theme::Tokens::default().chrome);
}

#[test]
fn choices_validate_and_survive_a_restart() {
    let mut a = app();
    let ctx = egui::Context::default();
    let set = |a: &mut LightcraftApp, p: serde_json::Value| {
        let (req, _) = crate::control::ControlRequest::new("ui.set", p);
        match crate::control::handle(a, &ctx, &req) {
            crate::control::Outcome::Done(reply) => reply,
            _ => panic!("expected a reply"),
        }
    };
    let ok = set(&mut a, json!({"settings": {"appearanceMode": "auto", "darkTheme": "midnight", "lightTheme": "paper"}}));
    assert_eq!(ok["ok"], true, "{ok}");
    assert_eq!(a.ui.settings.appearance_mode, AppearanceMode::Auto);
    for bad in [json!({"appearanceMode": "sepia"}), json!({"darkTheme": "paper"}), json!({"lightTheme": "midnight"}), json!({"appearanceMode": 3})] {
        let reply = set(&mut a, json!({ "settings": bad }));
        assert_eq!(reply["ok"], false, "{bad} accepted");
    }
    assert_eq!((a.ui.settings.dark_theme, a.ui.settings.light_theme), (DarkTheme::Midnight, LightTheme::Paper), "a rejected value changes nothing");
    let saved = serde_json::to_string(&a.ui).unwrap();
    assert!(saved.contains(r#""appearanceMode":"auto""#) && saved.contains(r#""darkTheme":"midnight""#) && saved.contains(r#""lightTheme":"paper""#));
    let restarted: UiState = serde_json::from_str(&saved).unwrap();
    assert_eq!(restarted.settings, a.ui.settings);
}

#[test]
fn the_button_cycles_auto_light_dark_without_losing_theme_choices() {
    let mut a = app();
    let ctx = egui::Context::default();
    a.services.system_theme = Some(Box::new(|_| Some(egui::Theme::Dark)));
    a.run("view.theme", json!({"theme": "paper"})).unwrap();
    a.run("view.theme", json!({"theme": "midnight"})).unwrap();
    assert_eq!(a.ui.settings.appearance_mode, AppearanceMode::Dark, "a dark theme fixes the mode to Dark");
    for (mode, shown) in
        [(AppearanceMode::Auto, ThemeKind::Midnight), (AppearanceMode::Light, ThemeKind::Paper), (AppearanceMode::Dark, ThemeKind::Midnight)]
    {
        let r = a.run("view.appearance", json!({})).unwrap();
        assert_eq!(a.ui.settings.appearance_mode, mode);
        assert_eq!(r["darkTheme"], "midnight");
        assert_eq!(r["lightTheme"], "paper");
        frame(&mut a, &ctx);
        assert_eq!(crate::theme::current(&ctx), Some(shown));
        assert_eq!(crate::theme::Tokens::get(&ctx).chrome, crate::theme::Tokens::for_kind(shown).chrome);
        let icon = match mode {
            AppearanceMode::Auto => crate::icons::Icon::Monitor,
            AppearanceMode::Light => crate::icons::Icon::Sun,
            AppearanceMode::Dark => crate::icons::Icon::Moon,
        };
        assert_eq!(crate::panels::topbar::appearance_icon(mode).0, icon);
    }
    // a light theme from the menu fixes Light
    a.run("view.theme", json!({"theme": "silver"})).unwrap();
    assert_eq!(
        (a.ui.settings.appearance_mode, a.ui.settings.light_theme, a.ui.settings.dark_theme),
        (AppearanceMode::Light, LightTheme::Silver, DarkTheme::Midnight)
    );
    // bad parameters are errors, never a panic, and change nothing
    for p in [json!({}), json!({"theme": 4}), json!({"theme": "neon"})] {
        assert!(a.run("view.theme", p.clone()).is_err(), "{p}");
    }
    for p in [json!({"mode": "sepia"}), json!({"mode": 1}), json!({"mode": []})] {
        assert!(a.run("view.appearance", p.clone()).is_err(), "{p}");
    }
    assert_eq!(a.ui.settings.appearance_mode, AppearanceMode::Light);
    a.run("view.appearance", json!({"mode": "auto"})).unwrap();
    assert_eq!(a.ui.settings.appearance_mode, AppearanceMode::Auto);
}

#[test]
fn auto_follows_the_system_and_falls_back_to_dark() {
    use std::sync::{Arc, Mutex};
    let system = Arc::new(Mutex::new(Some(egui::Theme::Light)));
    let mut a = app();
    let reported = Arc::clone(&system);
    a.services.system_theme = Some(Box::new(move |_| *reported.lock().unwrap()));
    a.run("view.appearance", json!({"mode": "auto"})).unwrap();
    let ctx = egui::Context::default();
    frame(&mut a, &ctx);
    assert_eq!(crate::theme::current(&ctx), Some(ThemeKind::Silver));
    assert_eq!(ctx.theme(), egui::Theme::Light);
    *system.lock().unwrap() = Some(egui::Theme::Dark);
    frame(&mut a, &ctx);
    assert_eq!(crate::theme::current(&ctx), Some(ThemeKind::Charcoal));
    assert_eq!(ctx.theme(), egui::Theme::Dark);
    // no answer from the host or from egui: dark
    *system.lock().unwrap() = None;
    frame(&mut a, &ctx);
    assert_eq!(crate::theme::current(&ctx), Some(ThemeKind::Charcoal));
    // without a host service, egui's report is used
    let mut plain = app();
    plain.ui.settings.appearance_mode = AppearanceMode::Auto;
    let ctx = egui::Context::default();
    let input = egui::RawInput { system_theme: Some(egui::Theme::Light), ..Default::default() };
    let mut output = ctx.run_ui(input, |ui| {
        assert_eq!(plain.system_theme(ui.ctx()), Some(egui::Theme::Light));
        plain.logic(ui.ctx());
    });
    output.textures_delta.clear();
    assert_eq!(crate::theme::current(&ctx), Some(ThemeKind::Silver));
}

/// The Studio design (the server admin page's look) has a light and a dark theme: each joins its
/// own family in the View menu, the commands, `ui.set` and the saved settings, and brings its
/// shape (panel cards, capsule buttons, rounder windows) where the classic themes keep theirs.
#[test]
fn studio_comes_in_both_appearances() {
    let mut a = app();
    let r = a.run("view.theme", json!({"theme": "studioLight"})).unwrap();
    assert_eq!((a.ui.settings.appearance_mode, a.ui.settings.light_theme), (AppearanceMode::Light, LightTheme::StudioLight));
    assert_eq!(r["lightTheme"], "studioLight");
    let r = a.run("view.theme", json!({"theme": "studioDark"})).unwrap();
    assert_eq!((a.ui.settings.appearance_mode, a.ui.settings.dark_theme), (AppearanceMode::Dark, DarkTheme::StudioDark));
    assert_eq!((r["darkTheme"].as_str(), r["lightTheme"].as_str()), (Some("studioDark"), Some("studioLight")));
    assert_eq!(a.ui.settings.theme(None), ThemeKind::StudioDark);
    assert_eq!(a.ui.settings.theme(Some(egui::Theme::Light)), ThemeKind::StudioDark, "Dark mode ignores the system");
    a.run("view.appearance", json!({"mode": "auto"})).unwrap();
    assert_eq!(a.ui.settings.theme(Some(egui::Theme::Light)), ThemeKind::StudioLight);
    // each belongs to its own family only
    let ctx = egui::Context::default();
    for bad in [json!({"darkTheme": "studioLight"}), json!({"lightTheme": "studioDark"}), json!({"darkTheme": "studio"})] {
        let (req, _) = crate::control::ControlRequest::new("ui.set", json!({ "settings": bad }));
        let crate::control::Outcome::Done(reply) = crate::control::handle(&mut a, &ctx, &req) else { panic!("expected a reply") };
        assert_eq!(reply["ok"], false, "{bad} accepted");
    }
    let restarted: UiState = serde_json::from_str(&serde_json::to_string(&a.ui).unwrap()).unwrap();
    assert_eq!((restarted.settings.dark_theme, restarted.settings.light_theme), (DarkTheme::StudioDark, LightTheme::StudioLight));
    // the menu lists them with their families, checked
    let text = serde_json::to_string(&crate::menubar::menu_bar(&a)).unwrap();
    assert!(text.contains(r#""params":{"theme":"studioDark"}"#) && text.contains(r#""params":{"theme":"studioLight"}"#));
    assert!(text.contains(r#""Studio Dark""#) && text.contains(r#""Studio Light""#));
    // shape: Studio's own, the classic themes' unchanged
    for kind in ThemeKind::ALL {
        let t = crate::theme::Tokens::for_kind(kind);
        assert_eq!(kind.is_studio(), t.cards(), "{kind:?}");
        if kind.is_studio() {
            assert!(t.button_radius >= 12 && t.window_radius > 6 && t.filled_sliders && t.dock_gap > 4.0, "{kind:?}");
        } else {
            assert_eq!((t.control_radius, t.button_radius, t.window_radius, t.dock_gap, t.backdrop), (4, 4, 6, 4.0, t.chrome), "{kind:?}");
            assert!(!t.filled_sliders);
        }
    }
    // applied: egui's own widgets and windows take the Studio radii
    crate::theme::apply_kind(&ctx, ThemeKind::StudioLight);
    let style = ctx.global_style();
    assert_eq!(style.visuals.window_corner_radius, egui::CornerRadius::same(14));
    assert_eq!(style.visuals.widgets.inactive.corner_radius, egui::CornerRadius::same(6));
    crate::theme::apply_kind(&ctx, ThemeKind::Charcoal);
    assert_eq!(ctx.global_style().visuals.widgets.inactive.corner_radius, egui::CornerRadius::same(4));
}

#[test]
fn view_menu_lists_modes_and_themes_with_checks() {
    let mut a = app();
    a.run("view.theme", json!({"theme": "paper"})).unwrap();
    let menus = crate::menubar::menu_bar(&a);
    let text = serde_json::to_string(&menus).unwrap();
    for label in [
        "Appearance",
        "Sync with System",
        "Light Mode",
        "Dark Mode",
        "Next Appearance Mode",
        "Charcoal",
        "Midnight",
        "Studio Dark",
        "Silver",
        "Paper",
        "Studio Light",
    ] {
        assert!(text.contains(&format!("\"{label}\"")), "{label} missing from the menus");
    }
    assert!(text.contains(r#""params":{"theme":"paper"}"#));
}

#[test]
fn the_top_bar_button_and_settings_cards_drive_the_mode() {
    let mut h = Headless::new(app(), [1400.0, 900.0], 1.0);
    h.settle(Duration::from_secs(60));
    assert_eq!(h.request("ui.clickWidget", json!({"id": "icon:appearance"}), T)["ok"], true);
    h.step();
    assert_eq!(h.app.ui.settings.appearance_mode, AppearanceMode::Auto);
    assert_eq!(h.request("ui.clickWidget", json!({"id": "icon:appearance"}), T)["ok"], true);
    h.step();
    h.step();
    assert_eq!(h.app.ui.settings.appearance_mode, AppearanceMode::Light);
    assert_eq!(crate::theme::current(&h.view.ctx), Some(ThemeKind::Silver));
    // Settings ▸ Interface: the mode buttons and the theme radios
    h.app.ui.dialog = Some(crate::state::Dialog::Settings { tab: "interface".into() });
    h.step();
    h.step();
    assert_eq!(h.request("ui.clickWidget", json!({"id": "radio:theme-paper"}), T)["ok"], true);
    h.step();
    h.step();
    assert_eq!(h.app.ui.settings.light_theme, LightTheme::Paper);
    assert_eq!(crate::theme::current(&h.view.ctx), Some(ThemeKind::Paper));
    assert_eq!(h.request("ui.clickWidget", json!({"id": "button:settingsAppearance-1"}), T)["ok"], true);
    h.step();
    h.step();
    assert_eq!(h.app.ui.settings.appearance_mode, AppearanceMode::Dark);
    assert_eq!(crate::theme::current(&h.view.ctx), Some(ThemeKind::Charcoal));
    // the Studio theme of the family in use
    assert_eq!(h.request("ui.clickWidget", json!({"id": "radio:theme-studioDark"}), T)["ok"], true);
    h.step();
    h.step();
    assert_eq!(h.app.ui.settings.dark_theme, DarkTheme::StudioDark);
    assert_eq!(crate::theme::current(&h.view.ctx), Some(ThemeKind::StudioDark));
}

#[test]
fn interface_settings_fit_small_windows_and_stack_theme_choices() {
    // Desktop windows have a 900×560 minimum (a narrower window gets the compact layout, whose
    // Settings is a page of its own: see `phone`).
    for size in [[900.0, 560.0]] {
        let mut h = Headless::new(app(), size, 1.0);
        h.app.ui.dialog = Some(crate::state::Dialog::Settings { tab: "interface".into() });
        for _ in 0..4 {
            h.step();
        }
        let window = h.app.widgets.iter().find(|(id, _)| id == "dialog:window").unwrap().1;
        assert!(window.width() <= size[0], "{size:?}: {window:?}");
        assert!(window.height() <= size[1], "{size:?}: {window:?}");
        let silver = h.app.widgets.iter().find(|(id, _)| id == "radio:theme-silver").unwrap().1;
        let mode = h.app.widgets.iter().find(|(id, _)| id == "button:settingsAppearance-0").unwrap().1;
        assert!(silver.top() > mode.bottom() + 80.0, "theme choices must sit below their preview");
        assert!(silver.right() <= window.right(), "theme choices stay inside the modal");
    }
}

#[test]
fn favorites_validate_roundtrip_and_stars_toggle_without_selecting() {
    for bad in [json!(["paper", "paper"]), json!(["unknown"]), json!([3]), json!("paper")] {
        assert!(serde_json::from_value::<AppSettings>(json!({"favoriteThemes": bad})).is_err());
    }
    let mut h = Headless::new(app(), [1400.0, 900.0], 1.0);
    h.app.ui.dialog = Some(crate::state::Dialog::Settings { tab: "interface".into() });
    for _ in 0..4 {
        h.step();
    }
    assert_eq!(h.request("ui.hoverWidget", json!({"id": "radio:theme-paper"}), T)["ok"], true);
    h.step();
    assert_eq!(h.request("ui.clickWidget", json!({"id": "button:themeFavorite-paper"}), T)["ok"], true);
    h.step();
    assert_eq!(h.app.ui.settings.favorite_themes, vec!["paper"]);
    if let Some(path) = std::env::var_os("FAVORITES_SCREENSHOT_DIR") {
        h.request("ui.scroll", json!({"dy": -90.0}), T);
        for _ in 0..4 {
            h.step();
        }
        let pixels = h.paint();
        let image =
            lightcraft_raster::Rgba8 { width: pixels.size[0], height: pixels.size[1], data: pixels.pixels.iter().map(|c| c.to_array()).collect() };
        let png = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&image), &lightcraft_codecs::EncodeMeta::default()).unwrap();
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(std::path::Path::new(&path).join("lightcraft-favorites.png"), png).unwrap();
    }
    assert_eq!(h.app.ui.settings.light_theme, LightTheme::Silver, "starring does not select");
    let saved = serde_json::to_value(&h.app.ui.settings).unwrap();
    assert_eq!(serde_json::from_value::<AppSettings>(saved).unwrap(), h.app.ui.settings);
    assert_eq!(h.request("ui.clickWidget", json!({"id": "button:themeFavorite-paper"}), T)["ok"], true);
    h.step();
    assert!(h.app.ui.settings.favorite_themes.is_empty());
    h.app.ui.dialog = None;
    h.app.ui.settings.favorite_themes = vec!["midnight".into(), "paper".into()];
    h.step();
    assert_eq!(h.request("ui.clickWidget", json!({"id": "icon:appearance", "button": "right"}), T)["ok"], true);
    h.step();
    h.step();
    let light = h.app.widgets.iter().find(|(id, _)| id == "menu:themeFavorite-paper").unwrap().1;
    let dark = h.app.widgets.iter().find(|(id, _)| id == "menu:themeFavorite-midnight").unwrap().1;
    assert!(light.top() < dark.top(), "light favorites precede dark favorites");
    assert_eq!(h.request("ui.clickWidget", json!({"id": "menu:themeFavorite-paper"}), T)["ok"], true);
    h.step();
    h.step();
    assert_eq!(h.app.ui.settings.appearance_mode, AppearanceMode::Light);
    assert_eq!(h.app.ui.settings.light_theme, LightTheme::Paper);
    assert_eq!(h.app.ui.settings.dark_theme, DarkTheme::Charcoal);
}

/// The phone layout's light / dark / system appearance (iOS).
mod phone {
    use std::time::Duration;

    use egui::Color32;
    use lightcraft_engine::Session;
    use serde_json::json;

    use crate::headless::Headless;
    use crate::state::{Appearance, AppearanceMode, RightPanel, ViewMode};
    use crate::theme::{ThemeKind, Tokens};
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
    /// app always was); Light and Dark ignore it; the desktop layout follows Appearance Mode instead.
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
        // the desktop layout ignores the setting: it follows Appearance Mode (Dark by default)
        run(&mut h, "app.appearance.light");
        let r = h.request("ui.resize", json!({"width": 1200, "height": 800}), T);
        assert_eq!(r["ok"], true, "{r}");
        h.settle(SETTLE);
        assert!(!h.app.compact && h.app.dark && Tokens::is_dark(&h.view.ctx), "the desktop layout stays dark");
        assert_eq!(Tokens::get(&h.view.ctx).canvas, Tokens::default().canvas);
        assert_eq!(crate::theme::current(&h.view.ctx), Some(ThemeKind::Charcoal));
        // and its Light Mode is light; back on the phone, the phone's own setting again
        h.app.ui.settings.appearance_mode = AppearanceMode::Light;
        h.settle(SETTLE);
        assert!(!h.app.dark && !Tokens::is_dark(&h.view.ctx));
        assert_eq!(crate::theme::current(&h.view.ctx), Some(ThemeKind::Silver));
        run(&mut h, "app.appearance.dark");
        let r = h.request("ui.resize", json!({"width": 390, "height": 844}), T);
        assert_eq!(r["ok"], true, "{r}");
        h.settle(SETTLE);
        assert!(h.app.compact && h.app.dark && Tokens::is_dark(&h.view.ctx));
        assert_eq!(Tokens::get(&h.view.ctx).canvas, Color32::BLACK);
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

    /// Review mode's stars and flag sit on light pills in the light appearance (its dark glyphs were
    /// lost on the dark ones) and on dark pills in the dark one.
    #[test]
    fn the_review_pills_follow_the_appearance() {
        let mut h = phone();
        let r = h.request("engine.execute", json!({"command": "view.reviewMode", "params": {"on": true}}), T);
        assert_eq!(r["ok"], true, "{r}");
        for (command, light) in [("app.appearance.light", true), ("app.appearance.dark", false)] {
            run(&mut h, command);
            for _ in 0..30 {
                h.step();
            }
            let pill = h.app.widgets.iter().find(|(w, _)| w == "review:stars").map(|(_, r)| *r).expect("the readout is up");
            let img = h.paint();
            // inside the pill's rounded left end, clear of the first star
            let (x, y) = ((pill.left() + 8.0) as usize, pill.center().y as usize);
            let px = img.pixels[y * img.size[0] + x];
            assert!(if light { px.r() > 200 } else { px.r() < 60 }, "{command}: {px:?}");
        }
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
}
