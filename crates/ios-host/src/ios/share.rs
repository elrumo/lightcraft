//! The share sheet (UIActivityViewController): Save Image (to Photos), Save to Files, AirDrop,
//! Mail, Messages and other apps.

use std::path::PathBuf;

use objc2::MainThreadOnly;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{NSArray, NSString, NSURL};
use objc2_ui_kit::UIActivityViewController;

pub fn share(paths: &[PathBuf]) -> Result<(), String> {
    let mtm = super::main_thread()?;
    if paths.is_empty() {
        return Err("nothing to share".into());
    }
    let items: Vec<Retained<AnyObject>> =
        paths.iter().map(|p| Retained::into_super(Retained::into_super(NSURL::fileURLWithPath(&NSString::from_str(&p.to_string_lossy()))))).collect();
    let items = NSArray::from_retained_slice(&items);
    // SAFETY: the items are file URLs (NSURL objects, which the activity view controller accepts
    // as activity items) and there are no custom activities.
    let vc = unsafe { UIActivityViewController::initWithActivityItems_applicationActivities(UIActivityViewController::alloc(mtm), &items, None) };
    let top = super::top_controller(mtm)?;
    // an iPad shows the sheet as a popover, which UIKit refuses to show without an anchor
    if let Some(pop) = vc.popoverPresentationController()
        && let Some(view) = top.view()
    {
        let b = view.bounds();
        pop.setSourceView(Some(&view));
        pop.setSourceRect(CGRect::new(CGPoint::new(b.origin.x + b.size.width / 2.0, b.origin.y + b.size.height / 2.0), CGSize::new(1.0, 1.0)));
    }
    top.presentViewController_animated_completion(&vc, true, None);
    Ok(())
}
