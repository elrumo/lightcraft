//! The Objective-C side (iOS only). Everything here runs on the main thread unless it says
//! otherwise; `objc2`'s `MainThreadMarker` makes that a compile-time requirement for UIKit.

pub mod imageio;
pub mod lifecycle;
pub mod pickers;
pub mod share;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_ui_kit::{UIApplication, UIViewController, UIWindowScene};

pub(crate) fn main_thread() -> Result<MainThreadMarker, String> {
    MainThreadMarker::new().ok_or_else(|| "must be called on the main thread".to_string())
}

/// The view controller to present a sheet over: the app window's root view controller (winit's),
/// then whatever it already presents.
pub(crate) fn top_controller(mtm: MainThreadMarker) -> Result<Retained<UIViewController>, String> {
    let app = UIApplication::sharedApplication(mtm);
    let mut windows = Vec::new();
    for scene in app.connectedScenes().allObjects().iter() {
        if let Ok(scene) = scene.downcast::<UIWindowScene>() {
            windows.extend(scene.windows().iter());
        }
    }
    let window = windows.iter().find(|w| w.isKeyWindow()).or(windows.first()).ok_or("no window to show the sheet over")?;
    let mut top = window.rootViewController().ok_or("the window has no view controller")?;
    // a sheet already up (bounded: presentation chains are short)
    for _ in 0..16 {
        match top.presentedViewController() {
            Some(p) if !p.isBeingDismissed() => top = p,
            _ => break,
        }
    }
    Ok(top)
}

/// Present `vc` over the app.
pub(crate) fn present(mtm: MainThreadMarker, vc: &UIViewController) -> Result<(), String> {
    let top = top_controller(mtm)?;
    top.presentViewController_animated_completion(vc, true, None);
    Ok(())
}
