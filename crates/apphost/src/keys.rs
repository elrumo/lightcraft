//! Key bindings that no command declares for itself: secondary keys for commands that already have
//! one (Space zooms like Z, Shift+X rejects and goes on to the next photo), the digits that rate and
//! label, and the pairs of commands that share a key and are told apart by the tool that is open.
//!
//! egui's shortcut handler and keymap dialog read this table, and a native Mac frontend asks for it
//! (`host.keyBindings`) to build its own key map, so the apps' keys agree and nothing is written
//! twice. A shortcut is spelled as in [`crate::ui_commands`]: `Cmd+Shift+K`, `Shift+X`, `Right`,
//! `Space`, `6`.

/// (shortcut, command id, params as JSON text).
pub type FixedKey = (&'static str, &'static str, &'static str);

/// Secondary key bindings for commands that already exist, and the rating and label digits. They
/// complement the primary shortcut a command declares (Lightroom's keys that our primary keymap
/// assigns elsewhere, see docs/ui-parity.md, Shortcuts). `Shift` with a digit or a flag key goes on to
/// the next photo (`advance`). Shown in Help, Keyboard Shortcuts.
pub const FIXED_KEYS: &[FixedKey] = &[
    ("Cmd+D", "library.selectNone", "{}"),
    ("Shift+E", "dialog.export", "{}"),
    ("Cmd+E", "app.exportPrevious", "{}"),
    ("Space", "view.zoomToggle", "{}"),
    // Lightroom Classic: F6 toggles the filmstrip, F7 the left library panel.
    // Keep existing primary shortcuts and honour custom keymap assignments.
    ("F6", "view.filmstrip", "{}"),
    ("F7", "view.leftPanel", "{}"),
    ("Shift+M", "version.create", "{}"),
    // grid thumbnail size: Classic's key is `=`; `+` (keypad, or Shift+= reported as Plus) agrees
    ("Plus", "view.thumbSizeUp", "{}"),
    ("Shift+Plus", "view.thumbSizeUp", "{}"),
    ("Shift+X", "photo.flag", r#"{"flag": "reject", "advance": true}"#),
    ("Shift+Z", "photo.flag", r#"{"flag": "pick", "advance": true}"#),
    ("Shift+U", "photo.flag", r#"{"flag": "none", "advance": true}"#),
    // rating 0 to 5, and with Shift the same and on to the next photo
    ("0", "photo.rate", r#"{"rating": 0}"#),
    ("1", "photo.rate", r#"{"rating": 1}"#),
    ("2", "photo.rate", r#"{"rating": 2}"#),
    ("3", "photo.rate", r#"{"rating": 3}"#),
    ("4", "photo.rate", r#"{"rating": 4}"#),
    ("5", "photo.rate", r#"{"rating": 5}"#),
    ("Shift+0", "photo.rate", r#"{"rating": 0, "advance": true}"#),
    ("Shift+1", "photo.rate", r#"{"rating": 1, "advance": true}"#),
    ("Shift+2", "photo.rate", r#"{"rating": 2, "advance": true}"#),
    ("Shift+3", "photo.rate", r#"{"rating": 3, "advance": true}"#),
    ("Shift+4", "photo.rate", r#"{"rating": 4, "advance": true}"#),
    ("Shift+5", "photo.rate", r#"{"rating": 5, "advance": true}"#),
    // colour labels 6 to 9 (red, yellow, green, blue)
    ("6", "photo.label", r#"{"label": "red"}"#),
    ("7", "photo.label", r#"{"label": "yellow"}"#),
    ("8", "photo.label", r#"{"label": "green"}"#),
    ("9", "photo.label", r#"{"label": "blue"}"#),
    ("Shift+6", "photo.label", r#"{"label": "red", "advance": true}"#),
    ("Shift+7", "photo.label", r#"{"label": "yellow", "advance": true}"#),
    ("Shift+8", "photo.label", r#"{"label": "green", "advance": true}"#),
    ("Shift+9", "photo.label", r#"{"label": "blue", "advance": true}"#),
    // Shift+[ / Shift+] arrive as { / } on most layouts
    ("Shift+{", "brush.featherLess", "{}"),
    ("Shift+}", "brush.featherMore", "{}"),
    // keyword set: Alt+1 to Alt+9 toggle the current set's keywords on the selection
    ("Alt+1", "keyword.toggleFromSet", r#"{"index": 1}"#),
    ("Alt+2", "keyword.toggleFromSet", r#"{"index": 2}"#),
    ("Alt+3", "keyword.toggleFromSet", r#"{"index": 3}"#),
    ("Alt+4", "keyword.toggleFromSet", r#"{"index": 4}"#),
    ("Alt+5", "keyword.toggleFromSet", r#"{"index": 5}"#),
    ("Alt+6", "keyword.toggleFromSet", r#"{"index": 6}"#),
    ("Alt+7", "keyword.toggleFromSet", r#"{"index": 7}"#),
    ("Alt+8", "keyword.toggleFromSet", r#"{"index": 8}"#),
    ("Alt+9", "keyword.toggleFromSet", r#"{"index": 9}"#),
];

/// Engine commands that intentionally share a key and are told apart by context (the Crop tool's X
/// swaps the crop's aspect, anywhere else it rejects the photo).
pub const CONTEXTUAL: &[(&str, &str)] = &[("photo.reject", "crop.rotateAspect")];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_fixed_key_names_a_command_and_has_params_that_parse() {
        let ui: Vec<&str> = crate::ui_commands::UI_COMMANDS.iter().map(|c| c.0).collect();
        for (shortcut, id, params) in FIXED_KEYS {
            assert!(!shortcut.is_empty(), "{id}: empty shortcut");
            assert!(ui.contains(id) || lightcraft_engine::find_command(id).is_some(), "{shortcut} names an unknown command {id}");
            let p: serde_json::Value = serde_json::from_str(params).unwrap_or_else(|e| panic!("{shortcut}: params {params}: {e}"));
            assert!(p.is_object(), "{shortcut}: params must be an object");
        }
    }

    #[test]
    fn no_key_is_listed_twice() {
        for (i, (a, ..)) in FIXED_KEYS.iter().enumerate() {
            assert!(!FIXED_KEYS[i + 1..].iter().any(|(b, ..)| a == b), "{a} is listed twice");
        }
    }

    #[test]
    fn contextual_pairs_name_real_commands() {
        for (a, b) in CONTEXTUAL {
            assert!(lightcraft_engine::find_command(a).is_some(), "{a}");
            assert!(lightcraft_engine::find_command(b).is_some(), "{b}");
        }
    }
}
