//! UIKit's lifecycle notifications and background time.

use std::cell::RefCell;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use block2::RcBlock;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::{NSNotification, NSNotificationCenter, NSObjectProtocol, NSOperationQueue, NSString};
use objc2_ui_kit::{
    UIApplication, UIApplicationDidBecomeActiveNotification, UIApplicationDidEnterBackgroundNotification,
    UIApplicationDidReceiveMemoryWarningNotification, UIApplicationWillEnterForegroundNotification, UIApplicationWillResignActiveNotification,
    UIApplicationWillTerminateNotification,
};

use crate::Lifecycle;

thread_local! {
    /// The observer tokens: kept for the life of the app (the notification centre doesn't).
    static OBSERVERS: RefCell<Vec<Retained<ProtocolObject<dyn NSObjectProtocol>>>> = const { RefCell::new(Vec::new()) };
}

pub fn observe(f: Box<dyn Fn(Lifecycle)>) -> Result<(), String> {
    let _mtm = super::main_thread()?;
    let f: Rc<dyn Fn(Lifecycle)> = Rc::from(f);
    let center = NSNotificationCenter::defaultCenter();
    let queue = NSOperationQueue::mainQueue();
    // SAFETY: reading UIKit's notification-name constants (immutable NSString statics the
    // framework defines; UIKit is linked).
    let names = unsafe {
        [
            (UIApplicationWillResignActiveNotification, Lifecycle::WillResignActive),
            (UIApplicationDidEnterBackgroundNotification, Lifecycle::DidEnterBackground),
            (UIApplicationWillEnterForegroundNotification, Lifecycle::WillEnterForeground),
            (UIApplicationDidBecomeActiveNotification, Lifecycle::DidBecomeActive),
            (UIApplicationDidReceiveMemoryWarningNotification, Lifecycle::MemoryWarning),
            (UIApplicationWillTerminateNotification, Lifecycle::WillTerminate),
        ]
    };
    for (name, kind) in names {
        let f = f.clone();
        // runs on the main queue (below), so the non-Send `Rc` never leaves the main thread
        let block = RcBlock::new(move |_n: NonNull<NSNotification>| f(kind));
        // SAFETY: no object filter; the queue is the main queue, so the block runs on the main
        // thread; the block copies what it captures and lives as long as the observer.
        let token = unsafe { center.addObserverForName_object_queue_usingBlock(Some(name), None, Some(&queue), &block) };
        OBSERVERS.with(|o| {
            if let Ok(mut o) = o.try_borrow_mut() {
                o.push(token);
            }
        });
    }
    Ok(())
}

/// A UIKit background task (see [`crate::BackgroundTask`]).
pub struct Task {
    /// The task's identifier; 0 (`UIBackgroundTaskInvalid`) once ended.
    id: Arc<AtomicUsize>,
}

fn end(id: &AtomicUsize) {
    let t = id.swap(0, Ordering::SeqCst);
    if t != 0
        && let Some(mtm) = MainThreadMarker::new()
    {
        UIApplication::sharedApplication(mtm).endBackgroundTask(t);
    }
}

impl Task {
    pub fn begin(name: &str) -> Option<Task> {
        let mtm = MainThreadMarker::new()?;
        let id = Arc::new(AtomicUsize::new(0));
        let on_expiry = id.clone();
        // the allowance ran out: end the task now, or the system ends the app
        let handler = RcBlock::new(move || end(&on_expiry));
        let t = UIApplication::sharedApplication(mtm).beginBackgroundTaskWithName_expirationHandler(Some(&NSString::from_str(name)), Some(&handler));
        if t == 0 {
            return None;
        }
        id.store(t, Ordering::SeqCst);
        Some(Task { id })
    }
}

impl Drop for Task {
    fn drop(&mut self) {
        end(&self.id);
    }
}
