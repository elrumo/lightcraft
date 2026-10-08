//! The system pasteboard (UIPasteboard): what text fields copy goes there, and Paste reads it.

use objc2_foundation::NSString;
use objc2_ui_kit::UIPasteboard;

/// The pasteboard's text, if it has any. iOS may ask the user to allow the paste first.
pub fn text() -> Option<String> {
    let pb = UIPasteboard::generalPasteboard();
    // SAFETY: a plain property read on the shared general pasteboard (thread-safe in UIKit); the
    // returned string is retained and copied out before it is dropped.
    let s = unsafe { pb.string() }?;
    Some(s.to_string())
}

/// Put `text` on the pasteboard.
pub fn set_text(text: &str) {
    let pb = UIPasteboard::generalPasteboard();
    let s = NSString::from_str(text);
    // SAFETY: a plain property write on the shared general pasteboard (thread-safe in UIKit); `s`
    // is a valid NSString the pasteboard copies.
    unsafe { pb.setString(Some(&s)) };
}
