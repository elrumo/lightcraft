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
#[cfg(all(feature = "siglip", not(target_arch = "wasm32")))]
mod weights;

pub use embedder::Embedder;
pub use faces::FaceEngine;
pub use index::{EmbeddingIndex, Hit, Import};
pub use reader::TextReader;
pub use sensitive::{Classifier, SensitiveIndex};
pub use store::{Error, Key};
pub use textindex::TextIndex;

/// The thread pool the ONNX models (text, faces) run on: `None` is rten's own, sized for the whole
/// machine; a host that shares the machine sets a size ([`set_model_threads`]).
#[cfg(all(feature = "onnx", not(target_arch = "wasm32")))]
static MODEL_POOL: std::sync::RwLock<Option<std::sync::Arc<rten::ThreadPool>>> = std::sync::RwLock::new(None);

/// Run the ONNX models on at most `threads` threads from now on (runs in progress finish on the old pool).
#[cfg(all(feature = "onnx", not(target_arch = "wasm32")))]
pub fn set_model_threads(threads: usize) {
    let pool = std::sync::Arc::new(rten::ThreadPool::with_num_threads(threads.max(1)));
    *MODEL_POOL.write().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(pool);
}

/// The options a model run takes: the pool [`set_model_threads`] made, if any.
#[cfg(all(feature = "onnx", not(target_arch = "wasm32")))]
pub(crate) fn run_options() -> Option<rten::RunOptions> {
    let pool = MODEL_POOL.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone()?;
    Some(rten::RunOptions::default().with_thread_pool(Some(pool)))
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_faces;
#[cfg(test)]
mod tests_text;
