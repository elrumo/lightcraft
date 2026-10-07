//! The Photos picker (PHPickerViewController) and the Files picker (UIDocumentPickerViewController).
//!
//! Both hand over copies: the Photos picker lends each file only for the duration of a callback
//! (on a background queue), the Files picker copies files into the app's tmp folder or, for a
//! folder, grants access to it while it is read. Everything is copied into a new subfolder of the
//! staging folder ([`crate::new_batch_dir`]) and delivered once.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use block2::RcBlock;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyObject, NSObject, ProtocolObject};
use objc2::{AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, extern_class, extern_methods, msg_send};
use objc2_foundation::{NSArray, NSError, NSItemProvider, NSObjectProtocol, NSString, NSURL};
use objc2_photos_ui::{
    PHPickerConfiguration, PHPickerConfigurationAssetRepresentationMode, PHPickerFilter, PHPickerResult, PHPickerViewControllerDelegate,
};
use objc2_ui_kit::{UIDocumentPickerDelegate, UIDocumentPickerViewController, UIResponder, UIViewController};
use objc2_uniform_type_identifiers::{UTType, UTTypeFolder, UTTypeImage, UTTypeRAWImage};

use crate::{Accept, Deliver, PickKind, Picked};

extern_class!(
    /// PhotosUI's picker on iOS (`objc2-photos-ui` binds only its macOS twin).
    #[unsafe(super(UIViewController, UIResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PHPickerViewController"]
    struct PhotoPicker;
);

impl PhotoPicker {
    extern_methods!(
        #[unsafe(method(initWithConfiguration:))]
        #[unsafe(method_family = init)]
        unsafe fn init_with_configuration(this: Allocated<Self>, configuration: &PHPickerConfiguration) -> Retained<Self>;

        #[unsafe(method(setDelegate:))]
        #[unsafe(method_family = none)]
        unsafe fn set_delegate(&self, delegate: Option<&ProtocolObject<dyn PHPickerViewControllerDelegate>>);
    );
}

/// One pick in progress.
struct Job {
    batch: PathBuf,
    accept: Accept,
    deliver: Deliver,
}

thread_local! {
    /// The delegate of the picker on screen (pickers hold their delegate weakly). Replaced by the
    /// next pick rather than dropped in its own callback, which would free it while it runs.
    static DELEGATE: RefCell<Option<Retained<AnyObject>>> = const { RefCell::new(None) };
}

fn keep(delegate: Retained<AnyObject>) {
    DELEGATE.with(|d| {
        if let Ok(mut d) = d.try_borrow_mut() {
            *d = Some(delegate);
        }
    });
}

fn take_job(slot: &RefCell<Option<Job>>) -> Option<Job> {
    slot.try_borrow_mut().ok().and_then(|mut j| j.take())
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; no Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = RefCell<Option<Job>>]
    struct PhotosDelegate;

    unsafe impl NSObjectProtocol for PhotosDelegate {}

    // SAFETY: the method's types are those of -[PHPickerViewControllerDelegate picker:didFinishPicking:]
    // (a PHPickerViewController and an NSArray<PHPickerResult *>); it is called on the main thread.
    unsafe impl PHPickerViewControllerDelegate for PhotosDelegate {
        #[unsafe(method(picker:didFinishPicking:))]
        fn picker_did_finish(&self, picker: &UIViewController, results: &NSArray<PHPickerResult>) {
            // the picker doesn't close itself
            picker.dismissViewControllerAnimated_completion(true, None);
            if let Some(job) = take_job(self.ivars()) {
                load_photos(job, results);
            }
        }
    }
);

define_class!(
    // SAFETY: NSObject has no subclassing requirements; no Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = RefCell<Option<(Job, bool)>>]
    struct FilesDelegate;

    unsafe impl NSObjectProtocol for FilesDelegate {}

    // SAFETY: the methods' types are those of UIDocumentPickerDelegate's (a document picker and an
    // NSArray<NSURL *>); they are called on the main thread.
    unsafe impl UIDocumentPickerDelegate for FilesDelegate {
        #[unsafe(method(documentPicker:didPickDocumentsAtURLs:))]
        fn did_pick(&self, _picker: &UIDocumentPickerViewController, urls: &NSArray<NSURL>) {
            let job = self.ivars().try_borrow_mut().ok().and_then(|mut j| j.take());
            if let Some((job, folder)) = job {
                if folder {
                    copy_picked_folder(job, urls);
                } else {
                    copy_picked_files(job, urls);
                }
            }
        }

        #[unsafe(method(documentPickerWasCancelled:))]
        fn cancelled(&self, _picker: &UIDocumentPickerViewController) {
            let job = self.ivars().try_borrow_mut().ok().and_then(|mut j| j.take());
            if let Some((job, _)) = job {
                (job.deliver)(Picked { cancelled: true, ..Default::default() });
            }
        }
    }
);

pub fn pick(kind: PickKind, staging: &Path, accept: Accept, deliver: Deliver) -> Result<(), String> {
    let mtm = super::main_thread()?;
    let batch = crate::new_batch_dir(staging)?;
    let job = Job { batch, accept, deliver };
    match kind {
        PickKind::Photos => pick_photos(mtm, job),
        PickKind::Files => pick_documents(mtm, job, false),
        PickKind::Folder => pick_documents(mtm, job, true),
    }
}

fn pick_photos(mtm: MainThreadMarker, job: Job) -> Result<(), String> {
    // (PHPicker is iOS 14+; the app targets 16, but a missing class must be an error, not a panic)
    if objc2::runtime::AnyClass::get(c"PHPickerViewController").is_none() {
        return Err("the Photos picker isn't available on this system".into());
    }
    // SAFETY: plain `init` of a freshly allocated configuration, then setters of the object we
    // own; 0 = no selection limit; the images filter is a framework singleton.
    let config = unsafe { PHPickerConfiguration::init(PHPickerConfiguration::alloc()) };
    unsafe {
        config.setSelectionLimit(0);
        config.setFilter(Some(&PHPickerFilter::imagesFilter()));
        // the file as it is in the library (HEIC, ProRAW DNG), not a JPEG made for us
        config.setPreferredAssetRepresentationMode(PHPickerConfigurationAssetRepresentationMode::Current);
    }
    // SAFETY: -[PHPickerViewController initWithConfiguration:] takes a configuration and returns
    // an initialized picker (main thread: `alloc(mtm)`).
    let picker = unsafe { PhotoPicker::init_with_configuration(PhotoPicker::alloc(mtm), &config) };
    let delegate = PhotosDelegate::alloc(mtm).set_ivars(RefCell::new(Some(job)));
    // SAFETY: NSObject's `init` on a freshly allocated object.
    let delegate: Retained<PhotosDelegate> = unsafe { msg_send![super(delegate), init] };
    // SAFETY: the delegate conforms to the protocol and is kept alive (`keep`) while the picker shows.
    unsafe { picker.set_delegate(Some(ProtocolObject::from_ref(&*delegate))) };
    keep(Retained::into_super(Retained::into_super(delegate)));
    super::present(mtm, &picker)
}

fn pick_documents(mtm: MainThreadMarker, job: Job, folder: bool) -> Result<(), String> {
    // SAFETY: reading UniformTypeIdentifiers' type constants (immutable framework statics).
    let types = unsafe { if folder { NSArray::from_slice(&[UTTypeFolder]) } else { NSArray::from_slice(&[UTTypeImage, UTTypeRAWImage]) } };
    // files: copies made for us (no security scope to manage); a folder: opened in place, read
    // while its security scope is held
    let picker = UIDocumentPickerViewController::initForOpeningContentTypes_asCopy(UIDocumentPickerViewController::alloc(mtm), &types, !folder);
    picker.setAllowsMultipleSelection(!folder);
    let delegate = FilesDelegate::alloc(mtm).set_ivars(RefCell::new(Some((job, folder))));
    // SAFETY: NSObject's `init` on a freshly allocated object.
    let delegate: Retained<FilesDelegate> = unsafe { msg_send![super(delegate), init] };
    picker.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    keep(Retained::into_super(Retained::into_super(delegate)));
    super::present(mtm, &picker)
}

/// Deliveries collected from callbacks on other threads.
struct Collect {
    left: usize,
    picked: Picked,
    deliver: Option<Deliver>,
}

fn finish_one(state: &Mutex<Collect>, r: Result<PathBuf, String>) {
    let mut g = state.lock().unwrap_or_else(PoisonError::into_inner);
    match r {
        Ok(p) => g.picked.files.push(p),
        Err(e) => g.picked.failed.push(e),
    }
    g.left = g.left.saturating_sub(1);
    if g.left == 0
        && let Some(deliver) = g.deliver.take()
    {
        let picked = std::mem::take(&mut g.picked);
        drop(g);
        deliver(picked);
    }
}

/// The type to ask a photo's provider for: a raw (ProRAW DNG) if it has one, else its image as
/// stored (HEIC, JPEG…).
fn best_type(provider: &NSItemProvider) -> Option<Retained<NSString>> {
    let ids = provider.registeredTypeIdentifiers();
    let typed: Vec<(Retained<NSString>, Retained<UTType>)> = ids.iter().filter_map(|id| UTType::typeWithIdentifier(&id).map(|t| (id, t))).collect();
    // SAFETY: reading UniformTypeIdentifiers' type constants (immutable framework statics).
    let (raw, image) = unsafe { (UTTypeRAWImage, UTTypeImage) };
    let pick = |want: &UTType| typed.iter().find(|(_, t)| t.conformsToType(want)).map(|(id, _)| id.clone());
    pick(raw).or_else(|| pick(image))
}

fn error_text(err: *mut NSError) -> String {
    // SAFETY: the completion handler's error is null or a valid NSError for the call.
    match unsafe { err.as_ref() } {
        Some(e) => e.localizedDescription().to_string(),
        None => "the photo couldn't be read".into(),
    }
}

/// The file's name: the photo's own (`IMG_1234`) with the extension of what was loaded.
fn photo_name(suggested: Option<String>, loaded: &Path) -> String {
    let ext = loaded.extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_default();
    let stem = suggested.filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "Photo".into());
    if ext.is_empty() || stem.to_lowercase().ends_with(&format!(".{}", ext.to_lowercase())) { stem } else { format!("{stem}.{ext}") }
}

fn load_photos(job: Job, results: &NSArray<PHPickerResult>) {
    let n = results.count();
    if n == 0 {
        (job.deliver)(Picked { cancelled: true, ..Default::default() });
        return;
    }
    let state = Arc::new(Mutex::new(Collect { left: n, picked: Picked::default(), deliver: Some(job.deliver) }));
    for result in results.iter() {
        // SAFETY: a PHPickerResult's item provider (a property getter).
        let provider = unsafe { result.itemProvider() };
        let Some(type_id) = best_type(&provider) else {
            finish_one(&state, Err("not a photo".into()));
            continue;
        };
        let suggested = provider.suggestedName().map(|s| s.to_string());
        let (batch, st) = (job.batch.clone(), state.clone());
        // called once, on a background queue; the file exists only until it returns, so it is
        // copied here (what it captures is Send: paths, strings and the shared state)
        let handler = RcBlock::new(move |url: *mut NSURL, err: *mut NSError| {
            // SAFETY: the handler's URL is null or a valid file URL for the duration of the call.
            let r = match unsafe { url.as_ref() }.and_then(|u| u.path()) {
                Some(path) => {
                    let path = PathBuf::from(path.to_string());
                    crate::copy_into(&batch, &path, &photo_name(suggested.clone(), &path))
                }
                None => Err(error_text(err)),
            };
            finish_one(&st, r);
        });
        // SAFETY: `type_id` is one of the provider's registered types; the handler has the
        // signature the method expects and is copied by it.
        let _progress = unsafe { provider.loadFileRepresentationForTypeIdentifier_completionHandler(&type_id, &handler) };
    }
}

fn url_paths(urls: &NSArray<NSURL>) -> Vec<PathBuf> {
    urls.iter().filter_map(|u| u.path()).map(|p| PathBuf::from(p.to_string())).collect()
}

/// Off the main thread when possible (a big file must not freeze the app).
fn in_background(work: impl FnOnce() + Send + 'static) {
    let cell = Arc::new(Mutex::new(Some(work)));
    let c = cell.clone();
    let spawned = std::thread::Builder::new().name("lc-pick-copy".into()).spawn(move || {
        if let Some(w) = c.lock().unwrap_or_else(PoisonError::into_inner).take() {
            w();
        }
    });
    if spawned.is_err()
        && let Some(w) = cell.lock().unwrap_or_else(PoisonError::into_inner).take()
    {
        w();
    }
}

fn copy_picked_files(job: Job, urls: &NSArray<NSURL>) {
    // the picker's own copies (in tmp/…-Inbox): moved into the batch, copied if that fails
    let paths = url_paths(urls);
    in_background(move || {
        let mut picked = Picked::default();
        for p in paths {
            let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let to = crate::unique_path(&job.batch, &crate::safe_file_name(&name, "Photo"));
            let r = std::fs::rename(&p, &to).map(|()| to).or_else(|_| crate::copy_into(&job.batch, &p, &name));
            match r {
                Ok(t) => picked.files.push(t),
                Err(e) => picked.failed.push(e),
            }
        }
        (job.deliver)(picked);
    });
}

/// An NSURL handed to the copying thread.
struct ScopedUrl(Retained<NSURL>);
// SAFETY: NSURL is immutable and thread-safe (Apple's Thread Safety Summary lists it among the
// Foundation classes that can be used from any thread); only `stopAccessingSecurityScopedResource`
// is called on it there.
unsafe impl Send for ScopedUrl {}

fn copy_picked_folder(job: Job, urls: &NSArray<NSURL>) {
    let Some(url) = urls.iter().next() else {
        (job.deliver)(Picked { cancelled: true, ..Default::default() });
        return;
    };
    let Some(path) = url.path().map(|p| PathBuf::from(p.to_string())) else {
        (job.deliver)(Picked { failed: vec!["the folder has no path".into()], ..Default::default() });
        return;
    };
    // SAFETY: a URL the document picker returned; balanced by `stopAccessing…` below.
    let scoped = unsafe { url.startAccessingSecurityScopedResource() };
    let url = ScopedUrl(url);
    in_background(move || {
        let picked = crate::copy_folder(&path, &job.batch, job.accept);
        if scoped {
            // SAFETY: balances the successful `startAccessingSecurityScopedResource`.
            unsafe { url.0.stopAccessingSecurityScopedResource() };
        }
        drop(url);
        (job.deliver)(picked);
    });
}
