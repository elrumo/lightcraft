//! Optional AI models: what each one is ([`registry`]) and how it reaches the user's computer
//! ([`fetch`] downloads it from configurable mirrors, [`download`] runs that on a background
//! thread). Nothing here runs a model; the crates that do (`lightcraft-segment` for SAM 3) read
//! the files this crate puts in a model's folder.
//!
//! No model is part of LightCraft: each is downloaded only when the user asks, after the app
//! names its licence, and every file is verified (exact size and SHA-256 where pinned) before
//! it is used. Without a model, the features that need it say so and everything else works.
//! See docs/ai-models.md and docs/ai-masks.md.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

#[cfg(not(target_arch = "wasm32"))]
pub mod download;
#[cfg(not(target_arch = "wasm32"))]
pub mod fetch;
#[cfg(not(target_arch = "wasm32"))]
pub mod registry;

#[cfg(not(target_arch = "wasm32"))]
pub use registry::ModelSpec;
