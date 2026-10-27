use super::*;

/// Where the system keeps monitor profiles, for the Choose Profile dialog to start in: the user's
/// own profiles first (Linux: colord / DisplayCAL; macOS: ColorSync), then the system's.
pub(crate) fn display_profile_dir() -> Option<std::path::PathBuf> {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).filter(|h| !h.is_empty()).map(std::path::PathBuf::from);
    let user = |rel: &str| home.as_ref().map(|h| h.join(rel));
    let candidates: Vec<Option<std::path::PathBuf>> = if cfg!(target_os = "macos") {
        vec![user("Library/ColorSync/Profiles"), Some("/Library/ColorSync/Profiles".into())]
    } else if cfg!(windows) {
        let root = std::env::var_os("SystemRoot").map(std::path::PathBuf::from).unwrap_or_else(|| "C:\\Windows".into());
        vec![Some(root.join("System32").join("spool").join("drivers").join("color"))]
    } else {
        let data = std::env::var_os("XDG_DATA_HOME").filter(|d| !d.is_empty()).map(std::path::PathBuf::from).or_else(|| user(".local/share"));
        vec![data.map(|d| d.join("icc")), user(".color/icc"), Some("/usr/share/color/icc".into())]
    };
    candidates.into_iter().flatten().find(|d| d.is_dir())
}

pub(crate) fn config_dir() -> Option<std::path::PathBuf> {
    lightcraft_engine::config::config_dir()
}

/// The window at the size it was last used at, zoomed if it was (`window`), or at the usual size
/// when nothing is remembered. A zoomed window is given the size it goes back to. Where it
/// opens is the system's to say.
pub(crate) fn as_remembered(builder: egui::ViewportBuilder, window: Option<lightcraft_ui_egui::window_place::WindowPlace>) -> egui::ViewportBuilder {
    let builder = builder.with_inner_size(window.map_or(lightcraft_ui_egui::window_place::DEFAULT_SIZE, |w| w.size));
    // (only ever asked to zoom: asked not to, macOS un-zooms a window whose size fills the screen
    // to a size of its own choosing)
    if window.is_some_and(|w| w.zoomed) { builder.with_maximized(true) } else { builder }
}
