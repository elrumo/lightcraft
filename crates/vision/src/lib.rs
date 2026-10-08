//! Photo understanding for the library: natural-language search (image and text embeddings) and,
//! later, faces. This crate holds the parts that don't depend on a model runtime, so they build
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
pub mod ocr;
#[cfg(all(feature = "onnx", not(target_arch = "wasm32")))]
pub mod onnxfix;
pub mod reader;
#[cfg(all(feature = "siglip", not(target_arch = "wasm32")))]
pub mod siglip;
pub mod store;
pub mod textindex;
#[cfg(all(feature = "siglip", not(target_arch = "wasm32")))]
mod weights;

pub use embedder::Embedder;
pub use index::{EmbeddingIndex, Hit, Import};
pub use reader::TextReader;
pub use store::{Error, Key};
pub use textindex::TextIndex;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_faces;
#[cfg(test)]
mod tests_text;
