//! The Objective-C side (iOS and macOS): ImageCaptureCore's device browser, a delegate for it and
//! for the devices, and the block-based thumbnail and download requests.
//!
//! The framework's objects stay on the main thread, in [`HOST`]; its delegate callbacks arrive
//! there too. Thumbnail and download completions arrive on the framework's queues: they carry only
//! plain Rust values (bytes, paths, messages) back, into the crate's state or onto the main queue.
//! No borrow of [`HOST`] is held across a call into the framework, so a callback the framework
//! makes on the spot finds it free.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use block2::RcBlock;
use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, ProtocolObject};
use objc2::{MainThreadMarker, MainThreadOnly, Message, define_class, msg_send};
use objc2_foundation::{NSArray, NSData, NSDate, NSDictionary, NSError, NSNumber, NSObjectProtocol, NSString, NSTimeZone, NSURL};
use objc2_image_capture_core::{
    ICCameraDevice, ICCameraDeviceDelegate, ICCameraFile, ICCameraItem, ICDevice, ICDeviceBrowser, ICDeviceBrowserDelegate, ICDeviceDelegate,
    ICDeviceLocationTypeMask, ICDeviceTypeMask, ICDownloadsDirectoryURL, ICSaveAsFilename, ICTransportTypeMassStorage,
};

use crate::{Camera, Copied, Done, Item, Kind, Transfer};

/// A download in progress: the files still to copy (id, the name to save it under).
struct Job {
    queue: VecDeque<(String, String)>,
    dir: PathBuf,
    transfer: Arc<Transfer>,
    copied: Copied,
    done: Option<Done>,
}

struct Host {
    browser: Retained<ICDeviceBrowser>,
    /// The browser's and every device's delegate: kept alive here (they hold it weakly).
    _delegate: Retained<Delegate>,
    devices: HashMap<String, Retained<ICCameraDevice>>,
    /// The files of opened devices: device → item id → file.
    files: HashMap<String, HashMap<String, Retained<ICCameraFile>>>,
    /// Downloads in progress, one per device.
    jobs: HashMap<String, Job>,
}

thread_local! {
    static HOST: RefCell<Option<Host>> = const { RefCell::new(None) };
}

/// Run `f` on the host (main thread only; `None` before [`watch`], or if it is borrowed — which
/// the module never lets happen across a framework call).
fn with_host<R>(f: impl FnOnce(&mut Host) -> R) -> Option<R> {
    HOST.with(|h| h.try_borrow_mut().ok().and_then(|mut h| h.as_mut().map(f)))
}

fn main_thread() -> Result<MainThreadMarker, String> {
    MainThreadMarker::new().ok_or_else(|| "must be called on the main thread".to_string())
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; no Drop. The framework calls the delegate
    // methods on the main thread (its headers say so for every callback used here).
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "LightCraftCaptureDelegate"]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    // SAFETY: the methods' types are those of ICDeviceBrowserDelegate's required methods.
    unsafe impl ICDeviceBrowserDelegate for Delegate {
        #[unsafe(method(deviceBrowser:didAddDevice:moreComing:))]
        fn browser_added(&self, _browser: &ICDeviceBrowser, device: &ICDevice, _more: bool) {
            added(self, device);
        }

        #[unsafe(method(deviceBrowser:didRemoveDevice:moreGoing:))]
        fn browser_removed(&self, _browser: &ICDeviceBrowser, device: &ICDevice, _more: bool) {
            removed(device);
        }
    }

    // SAFETY: the methods' types are those of ICDeviceDelegate's required methods.
    unsafe impl ICDeviceDelegate for Delegate {
        #[unsafe(method(device:didCloseSessionWithError:))]
        fn closed(&self, _device: &ICDevice, _error: Option<&NSError>) {}

        #[unsafe(method(didRemoveDevice:))]
        fn device_removed(&self, device: &ICDevice) {
            removed(device);
        }

        #[unsafe(method(device:didOpenSessionWithError:))]
        fn opened(&self, device: &ICDevice, error: Option<&NSError>) {
            if let Some(e) = error {
                crate::failed(&device_id(device), format!("the camera or card couldn't be opened: {}", e.localizedDescription()));
            }
        }
    }

    // SAFETY: the methods' types are those of ICCameraDeviceDelegate's required methods (an
    // ICCameraDevice, NSArray<ICCameraItem *>, CGImage, NSDictionary, NSData, NSError).
    unsafe impl ICCameraDeviceDelegate for Delegate {
        #[unsafe(method(cameraDevice:didAddItems:))]
        fn items_added(&self, camera: &ICCameraDevice, items: &NSArray<ICCameraItem>) {
            let id = device_id(camera);
            let added = remember(&id, items);
            crate::items_added(&id, added);
        }

        #[unsafe(method(cameraDevice:didRemoveItems:))]
        fn items_removed(&self, camera: &ICCameraDevice, items: &NSArray<ICCameraItem>) {
            let id = device_id(camera);
            let gone: Vec<String> = items.iter().map(|i| item_id(&i)).collect();
            with_host(|h| {
                if let Some(files) = h.files.get_mut(&id) {
                    for g in &gone {
                        files.remove(g);
                    }
                }
            });
            crate::items_removed(&id, &gone);
        }

        #[unsafe(method(cameraDevice:didReceiveThumbnail:forItem:error:))]
        fn thumbnail(
            &self,
            _camera: &ICCameraDevice,
            _thumbnail: Option<&objc2_core_graphics::CGImage>,
            _item: &ICCameraItem,
            _error: Option<&NSError>,
        ) {
        }

        #[unsafe(method(cameraDevice:didReceiveMetadata:forItem:error:))]
        fn metadata(&self, _camera: &ICCameraDevice, _metadata: Option<&NSDictionary>, _item: &ICCameraItem, _error: Option<&NSError>) {}

        #[unsafe(method(cameraDevice:didRenameItems:))]
        fn renamed(&self, _camera: &ICCameraDevice, _items: &NSArray<ICCameraItem>) {}

        #[unsafe(method(cameraDeviceDidChangeCapability:))]
        fn capability(&self, _camera: &ICCameraDevice) {}

        #[unsafe(method(cameraDevice:didReceivePTPEvent:))]
        fn ptp_event(&self, _camera: &ICCameraDevice, _event: &NSData) {}

        #[unsafe(method(deviceDidBecomeReadyWithCompleteContentCatalog:))]
        fn ready(&self, camera: &ICCameraDevice) {
            catalog(camera);
        }

        #[unsafe(method(cameraDeviceDidRemoveAccessRestriction:))]
        fn unlocked(&self, _device: &ICDevice) {}

        #[unsafe(method(cameraDeviceDidEnableAccessRestriction:))]
        fn locked(&self, device: &ICDevice) {
            crate::failed(&device_id(device), "unlock the iPhone or iPad, then open it again".into());
        }
    }
);

/// A device's id: its UUID, else the object's address (stable while it is connected).
fn device_id(d: &ICDevice) -> String {
    // SAFETY: a property getter.
    let uuid = unsafe { d.UUIDString() }.map(|s| s.to_string()).filter(|s| !s.is_empty());
    uuid.unwrap_or_else(|| format!("{:p}", d))
}

/// An item's id: the object's address (the device keeps the same object for a file while it is
/// open, and the files are kept in [`Host::files`] meanwhile).
fn item_id(i: &ICCameraItem) -> String {
    format!("{:p}", i)
}

fn is_mass_storage(d: &ICDevice) -> bool {
    // SAFETY: a property getter, and reading an ImageCaptureCore string constant.
    let (transport, mass) = unsafe { (d.transportType(), ICTransportTypeMassStorage) };
    transport.is_some_and(|t| t.isEqualToString(mass))
}

fn kind_of(d: &ICDevice) -> Kind {
    if is_mass_storage(d) {
        return Kind::Card;
    }
    // SAFETY: a property getter.
    let product = unsafe { d.productKind() }.map(|p| p.to_string()).unwrap_or_default();
    if ["iPhone", "iPad", "iPod"].iter().any(|p| product.contains(p)) { Kind::Phone } else { Kind::Camera }
}

/// A device was found: cameras are listed (on macOS not a card, which mounts as a disk).
fn added(delegate: &Delegate, device: &ICDevice) {
    let Some(camera) = device.downcast_ref::<ICCameraDevice>().map(|c| c.retain()) else { return };
    let kind = kind_of(device);
    if cfg!(target_os = "macos") && kind == Kind::Card {
        return;
    }
    // SAFETY: the delegate conforms to ICDeviceDelegate (and ICCameraDeviceDelegate) and is kept
    // alive in the host for as long as the device is.
    unsafe { device.setDelegate(Some(ProtocolObject::from_ref(delegate))) };
    let id = device_id(device);
    // SAFETY: a property getter.
    let name = unsafe { device.name() }.map(|n| n.to_string()).filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "Camera".into());
    with_host(|h| h.devices.insert(id.clone(), camera));
    crate::camera_added(Camera { id, name, kind });
}

fn removed(device: &ICDevice) {
    let id = device_id(device);
    let job = with_host(|h| {
        h.devices.remove(&id);
        h.files.remove(&id);
        h.jobs.remove(&id)
    })
    .flatten();
    if let Some(mut job) = job {
        // what was still to copy won't be
        let left = job.queue.len();
        job.copied.failed.extend(job.queue.drain(..).map(|(_, name)| format!("{name}: the camera or card was disconnected")));
        job.transfer.done.fetch_add(left, Ordering::Relaxed);
        if let Some(done) = job.done.take() {
            done(job.copied);
        }
    }
    crate::camera_removed(&id);
}

/// Local ISO 8601 time of a date (as the library keeps capture times).
fn local(date: &NSDate) -> Option<String> {
    let unix = date.timeIntervalSince1970();
    let zone = NSTimeZone::localTimeZone();
    let offset = zone.secondsFromGMTForDate(date);
    crate::local_time(unix, offset as i64)
}

fn item_of(file: &ICCameraFile, id: String) -> Item {
    // SAFETY: property getters of a file the device handed over.
    let (name, size, exif, created, raw, width, height) =
        unsafe { (file.name(), file.fileSize(), file.exifCreationDate(), file.creationDate(), file.isRaw(), file.width(), file.height()) };
    let name = name.map(|n| n.to_string()).unwrap_or_default();
    Item {
        id,
        size: u64::try_from(size).unwrap_or(0),
        captured: exif.or(created).and_then(|d| local(&d)),
        raw: raw || crate::raw_name(&name),
        width: u32::try_from(width).unwrap_or(0),
        height: u32::try_from(height).unwrap_or(0),
        name,
    }
}

/// Keep the files among `items` (folders are skipped) and describe them.
fn remember(device: &str, items: &NSArray<ICCameraItem>) -> Vec<Item> {
    let mut out = Vec::new();
    let mut kept = Vec::new();
    for i in items.iter() {
        let Some(file) = i.downcast_ref::<ICCameraFile>() else { continue };
        let id = item_id(&i);
        out.push(item_of(file, id.clone()));
        kept.push((id, file.retain()));
    }
    with_host(|h| h.files.entry(device.to_string()).or_default().extend(kept));
    out
}

/// The device has catalogued everything: all its media files.
fn catalog(camera: &ICCameraDevice) {
    let id = device_id(camera);
    // SAFETY: a property getter (a flattened list of the device's media files).
    let files = unsafe { camera.mediaFiles() };
    let items = files.map(|f| remember(&id, &f)).unwrap_or_default();
    crate::catalogued(&id, items);
}

fn error_text(err: *mut NSError) -> String {
    // SAFETY: a completion handler's error is null or a valid NSError for the call.
    match unsafe { err.as_ref() } {
        Some(e) => e.localizedDescription().to_string(),
        None => "the device didn't say why".into(),
    }
}

pub fn watch() -> Result<(), String> {
    let mtm = main_thread()?;
    if HOST.with(|h| h.try_borrow().map_or(true, |h| h.is_some())) {
        return Ok(());
    }
    // SAFETY: plain `new` of the framework's browser.
    let browser = unsafe { ICDeviceBrowser::new() };
    let delegate = Delegate::alloc(mtm).set_ivars(());
    // SAFETY: NSObject's `init` on a freshly allocated object.
    let delegate: Retained<Delegate> = unsafe { msg_send![super(delegate), init] };
    // SAFETY: the delegate conforms to the protocol and is kept alive in the host; the mask is a
    // combination of the framework's own flags (cameras, attached to this device).
    unsafe {
        browser.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        browser.setBrowsedDeviceTypeMask(ICDeviceTypeMask(ICDeviceTypeMask::Camera.0 | ICDeviceLocationTypeMask::Local.0));
    }
    HOST.with(|h| {
        if let Ok(mut h) = h.try_borrow_mut() {
            *h = Some(Host { browser: browser.clone(), _delegate: delegate, devices: HashMap::new(), files: HashMap::new(), jobs: HashMap::new() });
        }
    });
    // (devices already connected are announced to the delegate as if just added)
    // SAFETY: starting the browser we own, on the main thread.
    unsafe { browser.start() };
    Ok(())
}

pub fn open(device: &str) -> Result<(), String> {
    main_thread()?;
    let (browser, camera) = with_host(|h| (h.browser.clone(), h.devices.get(device).cloned())).ok_or("cameras aren't being watched")?;
    let camera = camera.ok_or("that camera or card isn't connected any more")?;
    #[cfg(target_os = "ios")]
    {
        use objc2_image_capture_core::{ICAuthorizationStatusAuthorized, ICAuthorizationStatusNotDetermined};
        use std::ptr::NonNull;
        // SAFETY: a property getter, and the framework's string constants.
        let (status, yes, unknown) =
            unsafe { (browser.contentsAuthorizationStatus(), ICAuthorizationStatusAuthorized, ICAuthorizationStatusNotDetermined) };
        if status.isEqualToString(unknown) {
            // the system asks the user; the answer arrives on another queue
            let id = device.to_string();
            let block = RcBlock::new(move |status: NonNull<NSString>| {
                // SAFETY: the completion's status is a valid string for the call.
                let ok = unsafe { status.as_ref() }.isEqualToString(unsafe { ICAuthorizationStatusAuthorized });
                let id = id.clone();
                DispatchQueue::main().exec_async(move || {
                    if ok {
                        let camera = with_host(|h| h.devices.get(&id).cloned()).flatten();
                        match camera {
                            Some(c) => open_session(&c),
                            None => crate::failed(&id, "the camera or card was disconnected".into()),
                        }
                    } else {
                        crate::failed(&id, denied());
                    }
                });
            });
            // SAFETY: the block has the signature the method expects and is copied by it.
            unsafe { browser.requestContentsAuthorizationWithCompletion(&block) };
            return Ok(());
        }
        if !status.isEqualToString(yes) {
            crate::failed(device, denied());
            return Ok(());
        }
    }
    let _ = browser;
    open_session(&camera);
    Ok(())
}

#[cfg(target_os = "ios")]
fn denied() -> String {
    "LightCraft isn't allowed to read cameras and cards: allow it in Settings ▸ Privacy & Security".into()
}

fn open_session(camera: &ICCameraDevice) {
    // SAFETY: property getters and session requests on a device the browser handed over, on the
    // main thread; the device's delegate (set when it was added) hears the answers.
    unsafe {
        if camera.hasOpenSession() {
            // opened before (another review): what it has catalogued is there already
            if camera.contentCatalogPercentCompleted() >= 100 {
                catalog(camera);
            } else if let Some(files) = camera.mediaFiles() {
                let id = device_id(camera);
                let items = remember(&id, &files);
                crate::items_added(&id, items);
            }
            return;
        }
        camera.requestOpenSession();
    }
}

pub fn close(device: &str) {
    if main_thread().is_err() {
        return;
    }
    let camera = with_host(|h| {
        h.files.remove(device);
        // (a download still running keeps the session)
        if h.jobs.contains_key(device) { None } else { h.devices.get(device).cloned() }
    })
    .flatten();
    if let Some(c) = camera {
        // SAFETY: a session request on a device the browser handed over, on the main thread.
        unsafe {
            if c.hasOpenSession() {
                c.requestCloseSession();
            }
        }
    }
}

pub fn request_thumbnail(device: &str, item: &str) -> Result<(), String> {
    main_thread()?;
    let file = with_host(|h| h.files.get(device).and_then(|f| f.get(item)).cloned()).flatten().ok_or("not on the device any more")?;
    // (the key is ImageIO's kCGImageSourceThumbnailMaxPixelSize, whose value is its own name)
    let key = NSString::from_str("kCGImageSourceThumbnailMaxPixelSize");
    let edge = NSNumber::new_u32(crate::THUMBNAIL_EDGE);
    let value: &AnyObject = &edge;
    let options = NSDictionary::from_slices(&[&*key], &[value]);
    let (d, i) = (device.to_string(), item.to_string());
    let block = RcBlock::new(move |data: *mut NSData, err: *mut NSError| {
        // SAFETY: the completion's data is null or a valid NSData for the call.
        let r = match unsafe { data.as_ref() } {
            Some(data) if data.length() > 0 => Ok(data.to_vec()),
            _ => Err(error_text(err)),
        };
        crate::thumbnail_arrived(&d, &i, r);
    });
    // SAFETY: the options dictionary maps the documented option to an NSNumber; the block has the
    // signature the method expects and is copied by it.
    unsafe { file.requestThumbnailDataWithOptions_completion(Some(&options), &block) };
    Ok(())
}

pub fn download(device: &str, items: &[String], dir: &Path, transfer: Arc<Transfer>, done: Done) -> Result<(), String> {
    main_thread()?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let names = with_host(|h| {
        let files = h.files.get(device);
        items
            .iter()
            .map(|i| {
                let file = files.and_then(|f| f.get(i));
                // SAFETY: a property getter of a file the device handed over.
                file.and_then(|f| unsafe { f.name() }).map(|n| n.to_string()).unwrap_or_default()
            })
            .collect::<Vec<_>>()
    })
    .ok_or("cameras aren't being watched")?;
    let saved = crate::save_names(&names);
    let job =
        Job { queue: items.iter().cloned().zip(saved).collect(), dir: dir.to_path_buf(), transfer, copied: Copied::default(), done: Some(done) };
    let started = with_host(|h| {
        if h.jobs.contains_key(device) {
            return Err("photos are already being copied from that camera or card".to_string());
        }
        h.jobs.insert(device.to_string(), job);
        Ok(())
    })
    .ok_or("cameras aren't being watched")?;
    started?;
    step(device);
    Ok(())
}

/// What to do next in a download.
enum Next {
    File { file: Retained<ICCameraFile>, name: String, dir: PathBuf },
    Finish(Done, Copied),
    Nothing,
}

/// Start copying the next file of `device`'s download, or end it (main thread).
fn step(device: &str) {
    let next = with_host(|h| {
        let Some(job) = h.jobs.get_mut(device) else { return Next::Nothing };
        loop {
            if job.transfer.cancel.load(Ordering::Relaxed) {
                job.copied.cancelled = true;
                break;
            }
            let Some((item, name)) = job.queue.pop_front() else { break };
            match h.files.get(device).and_then(|f| f.get(&item)) {
                Some(file) => return Next::File { file: file.clone(), name, dir: job.dir.clone() },
                None => {
                    job.copied.failed.push(format!("{name}: not on the device any more"));
                    job.transfer.done.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        match h.jobs.remove(device) {
            Some(mut job) => match job.done.take() {
                Some(done) => Next::Finish(done, std::mem::take(&mut job.copied)),
                None => Next::Nothing,
            },
            None => Next::Nothing,
        }
    })
    .unwrap_or(Next::Nothing);
    match next {
        Next::File { file, name, dir } => {
            let url = NSURL::fileURLWithPath_isDirectory(&NSString::from_str(&dir.to_string_lossy()), true);
            let save_as = NSString::from_str(&name);
            // SAFETY: reading ImageCaptureCore's option keys (immutable framework statics).
            let keys: [&NSString; 2] = unsafe { [ICDownloadsDirectoryURL, ICSaveAsFilename] };
            let values: [&AnyObject; 2] = [&url, &save_as];
            let options = NSDictionary::from_slices(&keys, &values);
            let (id, folder) = (device.to_string(), dir);
            let block = RcBlock::new(move |filename: *mut NSString, err: *mut NSError| {
                // SAFETY: the completion's file name is null or a valid NSString for the call.
                let r = match unsafe { filename.as_ref() } {
                    Some(f) if err.is_null() => {
                        let f = f.to_string();
                        Ok(if Path::new(&f).is_absolute() { PathBuf::from(f) } else { folder.join(f) })
                    }
                    _ => Err(format!("{name}: {}", error_text(err))),
                };
                let id = id.clone();
                DispatchQueue::main().exec_async(move || copied_one(&id, r));
            });
            // SAFETY: the options map the documented download keys to an NSURL (a folder) and an
            // NSString (a file name); the block has the signature the method expects and is copied.
            let _progress = unsafe { file.requestDownloadWithOptions_completion(Some(&options), &block) };
        }
        Next::Finish(done, copied) => done(copied),
        Next::Nothing => {}
    }
}

/// One file of a download finished (main thread).
fn copied_one(device: &str, r: Result<PathBuf, String>) {
    let running = with_host(|h| match h.jobs.get_mut(device) {
        Some(job) => {
            match r {
                Ok(p) => job.copied.files.push(p),
                Err(e) => job.copied.failed.push(e),
            }
            job.transfer.done.fetch_add(1, Ordering::Relaxed);
            true
        }
        // (the device went away and the download was ended)
        None => false,
    });
    if running == Some(true) {
        step(device);
    }
}
