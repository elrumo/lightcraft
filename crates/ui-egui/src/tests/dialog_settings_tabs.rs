//! The choices in Settings say which one is chosen by more than a slightly different fill, a
//! choice reached with the keyboard shows that it has the focus, and a screen reader is told what a
//! choice or a tab is and whether it is the one that holds.
//!
//! Before, one widget drew every choice and tab: what was chosen stood 1.35 to 1.56:1 against what
//! was not, and Tab moved a focus nobody could see (the picture with and without it was the same,
//! pixel for pixel).
//!
//! (This fork's Settings sections are a sidebar, not a tab bar; [`crate::widgets::tab_button`] is
//! for tab bars elsewhere. The upstream tests of the tab bar's look are not carried over.)

use std::time::Duration;

use egui::Color32;
use serde_json::json;

use crate::headless::Headless;
use crate::theme::{ThemeKind, Tokens};
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);

fn demo() -> Headless {
    let mut h = Headless::new(LightcraftApp::new(lightcraft_engine::Session::with_demo(), Services::default()), [1200.0, 900.0], 1.0);
    // let the first renders finish (a render still running at exit can crash in GPU teardown)
    h.settle(T);
    h
}

fn run(h: &mut Headless, command: &str, params: serde_json::Value) {
    let r = h.request("engine.execute", json!({"command": command, "params": params}), T);
    assert_eq!(r["ok"], true, "{command}: {r}");
    h.settle(T);
    for _ in 0..4 {
        h.step();
    }
}

fn show(h: &mut Headless, tab: &str) {
    run(h, "app.settings", json!({"tab": tab}));
}

fn key(h: &mut Headless, key: &str) {
    assert_eq!(h.request("ui.key", json!({"key": key}), T)["ok"], true, "{key}");
    for _ in 0..3 {
        h.step();
    }
}

fn rect(h: &Headless, id: &str) -> egui::Rect {
    h.app.widgets.iter().find(|(w, _)| w == id).map(|(_, r)| *r).unwrap_or_else(|| panic!("no widget {id}"))
}

/// How many pixels of `rect` are `colour` (one pixel per point here).
fn count(image: &egui::ColorImage, rect: egui::Rect, colour: Color32) -> usize {
    let [w, h] = image.size;
    let (x0, x1) = (rect.left().round().max(0.0) as usize, (rect.right().round().max(0.0) as usize).min(w));
    let (y0, y1) = (rect.top().round().max(0.0) as usize, (rect.bottom().round().max(0.0) as usize).min(h));
    (y0..y1).flat_map(|y| (x0..x1).map(move |x| (x, y))).filter(|(x, y)| image.pixels[y * w + x] == colour).count()
}

/// How many pixels of the focus ring's colour run round `rect`: along its top and its two sides.
fn ring(image: &egui::ColorImage, rect: egui::Rect, colour: Color32) -> usize {
    let [top, _, left, right] = edges(rect, 2.0);
    [top, left, right].iter().map(|e| count(image, *e, colour)).sum()
}

/// The outermost ring of `rect`, `width` points thick, as its four edges.
fn edges(rect: egui::Rect, width: f32) -> [egui::Rect; 4] {
    [
        egui::Rect::from_min_max(rect.min, egui::pos2(rect.right(), rect.top() + width)),
        egui::Rect::from_min_max(egui::pos2(rect.left(), rect.bottom() - width), rect.max),
        egui::Rect::from_min_max(rect.min, egui::pos2(rect.left() + width, rect.bottom())),
        egui::Rect::from_min_max(egui::pos2(rect.right() - width, rect.top()), rect.max),
    ]
}

/// WCAG contrast of two opaque colours.
fn contrast(a: Color32, b: Color32) -> f32 {
    let lum = |c: Color32| {
        let lin = |v: u8| {
            let v = f32::from(v) / 255.0;
            if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
    };
    let (a, b) = (lum(a), lum(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

#[test]
fn a_chosen_option_has_an_outline_and_the_others_do_not() {
    for kind in ThemeKind::ALL {
        let t = Tokens::for_kind(kind);
        // (the outline is counted along the button's bounding box: a pill-shaped button, the glass
        // themes', leaves that box at its corners)
        if t.button_radius > 12 {
            continue;
        }
        let mut h = demo();
        run(&mut h, "view.theme", json!({"theme": kind.id()}));
        // Given Performance, where Preview size is Automatic (the first choice)
        show(&mut h, "performance");
        assert_eq!(h.app.ui.settings.preview_limit, 0);
        let image = h.paint();
        let outline = |id: &str| edges(rect(&h, id), 1.0).iter().map(|e| count(&image, *e, t.text_label)).sum::<usize>();
        // Then the chosen one is outlined in the label colour, and one that is not chosen is not
        let chosen = rect(&h, "button:settingsPreview-0");
        assert!(
            outline("button:settingsPreview-0") as f32 > chosen.width() + chosen.height(),
            "{kind:?}: the chosen size is outlined ({})",
            outline("button:settingsPreview-0")
        );
        assert!(outline("button:settingsPreview-1") < 8, "{kind:?}: another size is not ({})", outline("button:settingsPreview-1"));
        // And an outline is not the focus ring: nothing in the ring's colour runs round the chosen one
        assert!(ring(&image, chosen, t.text) < 8, "{kind:?}: chosen is not focused ({})", ring(&image, chosen, t.text));
        // And the outline stands out from the button it is on, from one that is not chosen and from the window
        for (what, other) in [("its fill", t.pressed), ("a button", t.button), ("the window", t.chrome)] {
            assert!(contrast(t.text_label, other) >= 3.0, "{kind:?}: the outline against {what} is {}", contrast(t.text_label, other));
        }
    }
}

#[test]
fn a_choice_reached_with_the_keyboard_shows_the_focus() {
    let mut h = demo();
    let t = Tokens::get(&h.view.ctx);
    show(&mut h, "performance");
    let ring = |h: &Headless, id: &str| ring(&h.paint(), rect(h, id), t.text);
    assert!(ring(&h, "button:settingsPreview-1") < 8);
    // When Tab is pressed until the second preview size has the focus (the sidebar's rows and the GPU switch come first)
    let reached = (0..40).any(|_| {
        key(&mut h, "Tab");
        ring(&h, "button:settingsPreview-1") > 60
    });
    // Then it shows a ring, and is not chosen by that
    assert!(reached, "a choice shows the focus");
    assert_eq!(h.app.ui.settings.preview_limit, 0);
    // And Space chooses it
    key(&mut h, "Space");
    assert_eq!(h.app.ui.settings.preview_limit, crate::state::PREVIEW_LIMITS[1]);
}

/// What a screen reader is told: a choice is a radio button that says whether it is the one
/// chosen, a tab a button that says whether it is the one open.
#[test]
fn choices_and_tabs_say_what_they_are_and_which_one_holds() {
    let ctx = egui::Context::default();
    crate::theme::install_fonts(&ctx);
    ctx.enable_accesskit();
    let mut found = Vec::new();
    for _ in 0..3 {
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let _ = crate::widgets::choice_button(ui, "size-0", "Automatic", true);
            let _ = crate::widgets::choice_button(ui, "size-1", "1600 px", false);
            let _ = crate::widgets::tab_button(ui, "tab-general", "General", true);
            let _ = crate::widgets::tab_button(ui, "tab-import", "Import", false);
        });
        out.textures_delta.clear();
        if let Some(update) = out.platform_output.accesskit_update.take() {
            found = update
                .nodes
                .iter()
                .map(|(_, n)| (format!("{:?}", n.role()), n.label().unwrap_or_default().to_string(), format!("{:?}", n.toggled())))
                .collect();
        }
    }
    let has =
        |role: &str, label: &str, on: bool| found.iter().any(|(r, l, t)| r == role && l == label && t.contains(if on { "True" } else { "False" }));
    assert!(has("RadioButton", "Automatic", true), "{found:?}");
    assert!(has("RadioButton", "1600 px", false), "{found:?}");
    assert!(has("Button", "General", true), "{found:?}");
    assert!(has("Button", "Import", false), "{found:?}");
}
