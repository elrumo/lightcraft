//! Photo understanding for the library: natural-language search (image and text embeddings), the
//! text in photos, faces, and how likely a photo is to be sensitive ([`sensitive`]).
//! This crate holds the parts that don't depend on a model runtime, so they build
//! everywhere (web and iOS included): the on-disk record store ([`store`]) and the embedding index
//! with its cosine search ([`index`]).
//!
//! Derived data only. Embeddings are a pure function of a photo's pixels and the model, keyed by
//! the photo's content hash ([`Key`]), so any device or server can compute them and the results
//! merge by set union. They are never part of the catalog (which is synced whole to every device)
//! and can be deleted and rebuilt at any time, like the thumbnail cache.
//!
//! No UI dependencies (L3).
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod colors;
pub mod embedder;
pub mod faces;
#[cfg(any(test, feature = "testing"))]
pub mod fake;
pub mod index;
#[cfg(all(feature = "siglip", not(target_arch = "wasm32")))]
pub mod models;
#[cfg(all(feature = "onnx", not(target_arch = "wasm32")))]
pub mod nsfw;
#[cfg(all(feature = "onnx", not(target_arch = "wasm32")))]
pub mod ocr;
// protobuf bytes only (no runtime), so it builds everywhere: the web eraser uses it too
pub mod onnxfix;
pub mod reader;
pub mod sensitive;
#[cfg(all(feature = "siglip", not(target_arch = "wasm32")))]
pub mod siglip;
pub mod store;
pub mod textindex;
// (safetensors read on demand, with no memory map: the other candle models of the workspace read theirs through it)
#[cfg(all(feature = "siglip", not(target_arch = "wasm32")))]
pub mod weights;

pub use colors::ColorIndex;
pub use embedder::Embedder;
pub use faces::FaceEngine;
pub use index::{EmbeddingIndex, Hit, Import};
pub use reader::TextReader;
pub use sensitive::{Classifier, SensitiveIndex};
pub use store::{Error, Key};
pub use textindex::TextIndex;

/// The thread pool the ONNX models (text, faces) run on, and its size: `None` is rten's own, sized for the whole
/// machine; a host that shares the machine sets a size ([`set_model_threads`]).
#[cfg(all(feature = "onnx", not(target_arch = "wasm32")))]
static MODEL_POOL: std::sync::RwLock<Option<(usize, std::sync::Arc<rten::ThreadPool>)>> = std::sync::RwLock::new(None);

/// Run the ONNX models on at most `threads` threads from now on (runs in progress finish on the old pool).
#[cfg(all(feature = "onnx", not(target_arch = "wasm32")))]
pub fn set_model_threads(threads: usize) {
    let threads = threads.max(1);
    let pool = std::sync::Arc::new(rten::ThreadPool::with_num_threads(threads));
    *MODEL_POOL.write().unwrap_or_else(std::sync::PoisonError::into_inner) = Some((threads, pool));
}

thread_local! {
    /// The threads the models this thread runs may take as background work ([`in_background`]), if it is doing some.
    static BACKGROUND: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

/// Run `f` as background work: the ONNX models it runs on this thread use a pool of their own, at most `threads`
/// threads at a priority below the default ([`lightcraft_sysmem::utility_thread_priority`]), and not the pool the
/// runs a person is waiting for use. For work that shares a small machine with its owner (a phone indexing its
/// library while the app is in use): it leaves processors free and gives way. A size set with [`set_model_threads`]
/// still holds. Nested calls take the inner one's `threads`; a build without the ONNX models only runs `f`.
pub fn in_background<R>(threads: usize, f: impl FnOnce() -> R) -> R {
    /// Puts the thread's state back, also when `f` unwinds.
    struct Was(Option<usize>);
    impl Drop for Was {
        fn drop(&mut self) {
            BACKGROUND.set(self.0);
        }
    }
    let _was = Was(BACKGROUND.replace(Some(threads.max(1))));
    f()
}

/// The pool background model runs of `threads` threads use: kept until another size is asked for, its threads
/// lowered once.
#[cfg(all(feature = "onnx", not(target_arch = "wasm32")))]
fn background_pool(threads: usize) -> std::sync::Arc<rten::ThreadPool> {
    static POOL: std::sync::Mutex<Option<(usize, std::sync::Arc<rten::ThreadPool>)>> = std::sync::Mutex::new(None);
    /// Is this one of rten's own threads? (A pool rten could not build runs its work on the caller's thread, and a
    /// `broadcast` from there would reach rayon's global pool: those threads must keep their priority.)
    fn on_model_thread() -> bool {
        std::thread::current().name().is_some_and(|n| n.starts_with("rten-"))
    }
    let mut kept = POOL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((n, pool)) = kept.as_ref()
        && *n == threads
    {
        return pool.clone();
    }
    let pool = std::sync::Arc::new(rten::ThreadPool::with_num_threads(threads));
    pool.run(|| {
        if on_model_thread() {
            rayon::broadcast(|_| {
                if on_model_thread() {
                    let _ = lightcraft_sysmem::utility_thread_priority();
                }
            });
        }
    });
    *kept = Some((threads, pool.clone()));
    pool
}

/// The options a model run takes: the background pool inside [`in_background`], else the pool
/// [`set_model_threads`] made, if any.
#[cfg(all(feature = "onnx", not(target_arch = "wasm32")))]
pub(crate) fn run_options() -> Option<rten::RunOptions> {
    let sized = MODEL_POOL.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
    let pool = match BACKGROUND.get() {
        Some(threads) => background_pool(sized.as_ref().map_or(threads, |(most, _)| threads.min(*most))),
        None => sized?.1,
    };
    Some(rten::RunOptions::default().with_thread_pool(Some(pool)))
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_faces;
#[cfg(test)]
mod tests_pools;
#[cfg(test)]
mod tests_text;
