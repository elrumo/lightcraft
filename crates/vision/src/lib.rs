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

pub mod index;
pub mod store;

pub use index::{EmbeddingIndex, Hit, Import};
pub use store::{Error, Key};

#[cfg(test)]
mod tests;
