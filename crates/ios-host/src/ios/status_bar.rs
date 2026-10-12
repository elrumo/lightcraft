//! A tap on the status bar. iOS answers it by scrolling the scroll view under it back to the top,
//! but LightCraft draws its own scrolling (egui), so UIKit has no scroll view of the app's to ask.
//! This gives it one: a transparent UIScrollView the size of the window, behind winit's view (so it
//! never gets a touch), set to scroll to the top and always a point away from it. Its delegate turns
//! the system's question into a tap the app takes on its next frame ([`crate::take_status_bar_taps`])
//! and says no, so the decoy itself never moves and the next tap is asked about too.

use std::cell::RefCell;
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::{NSObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_core_foundation::{CGPoint, CGSize};
use objc2_foundation::NSObjectProtocol;
use objc2_ui_kit::{UIColor, UIScrollView, UIScrollViewDelegate, UIViewAutoresizing};

define_class!(
    // SAFETY: NSObject has no subclassing requirements; no Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = Rc<dyn Fn()>]
    struct TopDelegate;

    unsafe impl NSObjectProtocol for TopDelegate {}

    // SAFETY: the method's types are those of -[UIScrollViewDelegate scrollViewShouldScrollToTop:]
    // (a UIScrollView, returning BOOL); UIKit calls it on the main thread.
    unsafe impl UIScrollViewDelegate for TopDelegate {
        #[unsafe(method(scrollViewShouldScrollToTop:))]
        fn should_scroll_to_top(&self, _scroll_view: &UIScrollView) -> bool {
            crate::status_bar_tapped();
            (self.ivars())();
            false
        }
    }
);

impl TopDelegate {
    fn new(mtm: MainThreadMarker, f: Rc<dyn Fn()>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(f);
        // SAFETY: NSObject's designated initialiser, on an allocated instance with its ivars set.
        unsafe { msg_send![super(this), init] }
    }
}

thread_local! {
    /// The decoy and its delegate, kept for the life of the app (a scroll view holds its delegate
    /// weakly). Installed once.
    static DECOY: RefCell<Option<(Retained<UIScrollView>, Retained<TopDelegate>)>> = const { RefCell::new(None) };
}

pub fn observe(f: Box<dyn Fn()>) -> Result<(), String> {
    let mtm = super::main_thread()?;
    if DECOY.with(|d| d.try_borrow().map(|d| d.is_some()).unwrap_or(true)) {
        return Ok(());
    }
    let window = super::app_window(mtm).ok_or("no window yet")?;
    let bounds = window.bounds();
    let view = UIScrollView::initWithFrame(UIScrollView::alloc(mtm), bounds);
    view.setAutoresizingMask(UIViewAutoresizing::FlexibleWidth | UIViewAutoresizing::FlexibleHeight);
    view.setBackgroundColor(Some(&UIColor::clearColor()));
    view.setShowsVerticalScrollIndicator(false);
    view.setShowsHorizontalScrollIndicator(false);
    view.setScrollsToTop(true);
    // a point of room to scroll up through, and already scrolled down by it
    view.setContentSize(CGSize { width: bounds.size.width, height: bounds.size.height + 1.0 });
    view.setContentOffset(CGPoint { x: 0.0, y: 1.0 });
    let delegate = TopDelegate::new(mtm, Rc::from(f));
    // SAFETY: the delegate is a live object implementing UIScrollViewDelegate, kept alive in `DECOY`
    // for as long as the scroll view (the property is weak).
    unsafe { view.setDelegate(Some(ProtocolObject::from_ref(&*delegate))) };
    // behind winit's view, which covers the window: it is on screen for UIKit, out of reach of touches
    window.insertSubview_atIndex(&view, 0);
    DECOY.with(|d| {
        if let Ok(mut d) = d.try_borrow_mut() {
            *d = Some((view, delegate));
        }
    });
    Ok(())
}
