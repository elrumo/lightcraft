//! The frontend runtime every LightCraft UI shares (L4, above the engine).
//!
//! The engine is the document and its commands; this crate is what a frontend runs around it so
//! that two UIs (egui on desktop, web and today's iOS app; SwiftUI in the native iOS app) behave the
//! same without writing anything twice:
//!
//! - [`render`]: the render queue: per-slot requests, de-duplication and priorities, stand-ins,
//!   stage caches for slider drags, draft sizing, and what each slot shows (in the frontend's own
//!   form, through [`render::Present`]).
//! - [`export`]: Export Now with the desktop app's parameters, for frontends without an export
//!   dialog of their own (the MCP server's headless backend, the SwiftUI app).
//! - [`startup`]: how a mobile app opens its library, sizes its memory budget and keeps its
//!   staging folders (both iOS apps).
//! - `host` (native only): the engine on a thread of its own, driven by JSON requests and
//!   answering with events and leased pictures, for a frontend that isn't Rust (the SwiftUI app,
//!   through the C ABI in `apps/lightcraft-ios-swift`).
//!
//! The rule (`CLAUDE.md`, *Non-negotiables*): when a second UI first needs logic that sits in
//! `ui-egui`, it moves here (threads, scheduling, background pumps, post-command policy), to the
//! engine (session behaviour, as a command) or to `geom` / `develop` / `color` (geometry and edit
//! math), and egui switches to it in the same change. No UI toolkit below L5: nothing here knows egui.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod export;
#[cfg(not(target_arch = "wasm32"))]
pub mod host;
pub mod render;
pub mod startup;
