//! Where the pointer is while files are dragged in from outside the window (macOS).
//!
//! winit tells the app which files hover over the window but stops sending the pointer's position
//! when such a drag starts, so the sidebar couldn't tell which album the files are over
//! (`Services::file_drag_pointer`). AppKit knows where the pointer is at any moment.

use objc2::MainThreadMarker;
use objc2_app_kit::NSApplication;

/// The pointer in points from the top left of the window's content (egui's coordinates), or `None`
/// when it isn't known (off the main thread, no window) or is outside the window.
pub fn pointer() -> Option<egui::Pos2> {
    let mtm = MainThreadMarker::new()?;
    let app = NSApplication::sharedApplication(mtm);
    // (the app isn't active while files are dragged in from another one: any window will do)
    let window = app.mainWindow().or_else(|| app.keyWindow()).or_else(|| app.windows().firstObject())?;
    let content = window.contentView()?.frame();
    // window coordinates grow upwards; the content's frame is in the same ones
    let at = window.mouseLocationOutsideOfEventStream();
    inside(at.x - content.origin.x, content.origin.y + content.size.height - at.y, content.size.width, content.size.height)
}

/// `(x, y)` as a position if it is within a `w` × `h` window.
fn inside(x: f64, y: f64, w: f64, h: f64) -> Option<egui::Pos2> {
    ((0.0..=w).contains(&x) && (0.0..=h).contains(&y)).then(|| egui::pos2(x as f32, y as f32))
}

#[cfg(test)]
mod tests {
    #[test]
    fn unknown_off_the_main_thread() {
        // AppKit is main-thread only; a test thread gets nothing rather than undefined behaviour
        assert_eq!(std::thread::spawn(super::pointer).join().unwrap(), None);
    }

    #[test]
    fn only_inside_the_window_counts() {
        assert_eq!(super::inside(10.0, 20.0, 100.0, 50.0), Some(egui::pos2(10.0, 20.0)));
        assert_eq!(super::inside(-1.0, 20.0, 100.0, 50.0), None);
        assert_eq!(super::inside(10.0, 51.0, 100.0, 50.0), None);
        assert_eq!(super::inside(f64::NAN, 1.0, 100.0, 50.0), None);
    }
}
