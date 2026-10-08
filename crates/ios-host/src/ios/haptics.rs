//! Haptic feedback: UIKit's feedback generators.

use std::cell::OnceCell;

use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_ui_kit::{
    UIFeedbackGenerator, UIImpactFeedbackGenerator, UIImpactFeedbackStyle, UINotificationFeedbackGenerator, UINotificationFeedbackType,
    UISelectionFeedbackGenerator,
};

use crate::Haptic;

/// One generator per kind, kept for the life of the app and `prepare`d after each tap so the next
/// one has no wake-up delay (Apple's advice for feedback that follows a finger).
struct Generators {
    selection: Retained<UISelectionFeedbackGenerator>,
    light: Retained<UIImpactFeedbackGenerator>,
    medium: Retained<UIImpactFeedbackGenerator>,
    notification: Retained<UINotificationFeedbackGenerator>,
}

thread_local! {
    static GENERATORS: OnceCell<Generators> = const { OnceCell::new() };
}

// (`initWithStyle:` is deprecated for `feedbackGeneratorWithStyle:forView:`, which needs iOS 17.5; the app runs on 16)
#[allow(deprecated)]
fn impact(mtm: MainThreadMarker, style: UIImpactFeedbackStyle) -> Retained<UIImpactFeedbackGenerator> {
    UIImpactFeedbackGenerator::initWithStyle(UIImpactFeedbackGenerator::alloc(mtm), style)
}

/// Play `h` (does nothing off the main thread: UIKit's generators are main-thread only).
pub fn play(h: Haptic) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    GENERATORS.with(|cell| {
        let g = cell.get_or_init(|| Generators {
            selection: UISelectionFeedbackGenerator::new(mtm),
            light: impact(mtm, UIImpactFeedbackStyle::Light),
            medium: impact(mtm, UIImpactFeedbackStyle::Medium),
            notification: UINotificationFeedbackGenerator::new(mtm),
        });
        let ready: &UIFeedbackGenerator = match h {
            Haptic::Selection => {
                g.selection.selectionChanged();
                &g.selection
            }
            Haptic::Light => {
                g.light.impactOccurred();
                &g.light
            }
            Haptic::Medium => {
                g.medium.impactOccurred();
                &g.medium
            }
            Haptic::Warning => {
                g.notification.notificationOccurred(UINotificationFeedbackType::Warning);
                &g.notification
            }
        };
        ready.prepare();
    });
}
