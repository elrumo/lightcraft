//! Saving files to the photo library (PhotoKit). "Add only" access: the app may put photos in the
//! library but never read it, which is all an export needs and the least iOS asks the user for
//! (`NSPhotoLibraryAddUsageDescription` is the sentence its prompt shows, the first time).

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use block2::{DynBlock, RcBlock};
use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2_foundation::{NSError, NSString, NSURL};
use objc2_photos::{PHAccessLevel, PHAssetCreationRequest, PHAssetResourceType, PHAuthorizationStatus, PHPhotoLibrary};

use crate::SaveDone;

/// Says how the save went, once, whichever of PhotoKit's callbacks gets there first.
type Finish = Arc<dyn Fn(Result<usize, String>) + Send + Sync>;

const DENIED: &str = "LightCraft may not add photos to your library: allow it in Settings ▸ LightCraft ▸ Photos";

pub fn save(paths: &[PathBuf], done: SaveDone) {
    let done = Arc::new(Mutex::new(Some(done)));
    let finish: Finish = Arc::new(move |result| {
        let taken = done.lock().unwrap_or_else(PoisonError::into_inner).take();
        if let Some(done) = taken {
            done(result);
        }
    });
    if paths.is_empty() {
        finish(Err("nothing to save".into()));
        return;
    }
    let urls: Vec<Retained<NSURL>> = paths.iter().map(|p| NSURL::fileURLWithPath(&NSString::from_str(&p.to_string_lossy()))).collect();
    let asked = finish.clone();
    let handler = RcBlock::new(move |status: PHAuthorizationStatus| {
        if status == PHAuthorizationStatus::Authorized || status == PHAuthorizationStatus::Limited {
            add(&urls, asked.clone());
        } else {
            asked(Err(DENIED.into()));
        }
    });
    // SAFETY: a class method taking an access level and a block of the type the framework declares.
    // PhotoKit copies the block and calls it once, on a background queue; it holds only immutable
    // NSURLs (thread-safe) and a `Finish`, which is `Send + Sync`.
    unsafe { PHPhotoLibrary::requestAuthorizationForAccessLevel_handler(PHAccessLevel::AddOnly, &handler) };
}

/// Put every file in the library as a photo of its own, in one change: all of them go in or none.
fn add(urls: &[Retained<NSURL>], finish: Finish) {
    let count = urls.len();
    let urls = urls.to_vec();
    let changes = RcBlock::new(move || {
        for url in &urls {
            // SAFETY: inside the change block of `performChanges`, the only place creation requests are
            // allowed; `url` is a file URL (an exported file: one that has gone missing makes the
            // whole change fail, which the completion handler reports).
            unsafe {
                PHAssetCreationRequest::creationRequestForAsset().addResourceWithType_fileURL_options(PHAssetResourceType::Photo, url, None);
            }
        }
    });
    let completion = RcBlock::new(move |ok: Bool, error: *mut NSError| {
        if ok.as_bool() {
            finish(Ok(count));
            return;
        }
        // SAFETY: PhotoKit passes nil, or an NSError it keeps alive for the call.
        let why =
            unsafe { error.as_ref() }.map(|e| e.localizedDescription().to_string()).unwrap_or_else(|| "the photo library refused them".to_string());
        finish(Err(why));
    });
    // SAFETY: `performChanges` copies both blocks and runs them on its own queue. The change block is
    // passed as the framework's `dispatch_block_t`, a pointer to the same block object, which stays
    // alive (in `changes`) for the call.
    unsafe {
        let library = PHPhotoLibrary::sharedPhotoLibrary();
        library.performChanges_completionHandler(&*changes as *const DynBlock<dyn Fn()> as *mut DynBlock<dyn Fn()>, Some(&completion));
    }
}
