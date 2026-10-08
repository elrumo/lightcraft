//! Light and dark: what the system is set to, and making the app's windows follow what the app
//! draws, so that UIKit's own chrome (the status bar, the keyboard, the share sheet, the pickers)
//! matches it. winit ignores the theme on iOS, so this is the app's only source for it.

use objc2::MainThreadMarker;
use objc2_ui_kit::{UIApplication, UIScreen, UITraitEnvironment, UIUserInterfaceStyle, UIWindowScene};

use crate::InterfaceStyle;

/// The system's own style. The main screen's traits are the system's: a window's
/// `overrideUserInterfaceStyle` doesn't reach them, so this stays right while the app forces a style.
pub fn system_style() -> Option<InterfaceStyle> {
    let mtm = MainThreadMarker::new()?;
    let traits = UIScreen::mainScreen(mtm).traitCollection();
    // SAFETY: a plain getter on a valid UITraitCollection, called on the main thread.
    let style = unsafe { traits.userInterfaceStyle() };
    match style {
        UIUserInterfaceStyle::Dark => Some(InterfaceStyle::Dark),
        UIUserInterfaceStyle::Light => Some(InterfaceStyle::Light),
        _ => None,
    }
}

/// Force the app's windows to `style` (`None`: back to the system's). New windows start with the
/// last style set, as the app's windows are all that show it.
pub fn set_style(style: Option<InterfaceStyle>) -> Result<(), String> {
    let mtm = super::main_thread()?;
    let value = match style {
        Some(InterfaceStyle::Light) => UIUserInterfaceStyle::Light,
        Some(InterfaceStyle::Dark) => UIUserInterfaceStyle::Dark,
        None => UIUserInterfaceStyle::Unspecified,
    };
    let app = UIApplication::sharedApplication(mtm);
    for scene in app.connectedScenes().allObjects().iter() {
        if let Ok(scene) = scene.downcast::<UIWindowScene>() {
            for window in scene.windows().iter() {
                window.setOverrideUserInterfaceStyle(value);
            }
        }
    }
    Ok(())
}
