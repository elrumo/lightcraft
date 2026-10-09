//! Neural enhancement on candle, in pure Rust: AI **super resolution** with SPAN (Swift
//! Parameter-free Attention Network, Wan et al., CVPR Workshops 2024) and the community models
//! trained with it (the Nomos Uni family).
//!
//! The weights are not part of LightCraft: the user downloads them when they ask for the feature
//! (`lightcraft-fetch` does the download, [`models`] says what to fetch); this crate reads the
//! `.safetensors` file.
//!
//! [`Span`] loads a model and [`Span::upscale`] enlarges an sRGB-encoded image in tiles, so a
//! 24 MP photo needs a few hundred MB, not gigabytes, and the result is identical to one pass
//! over the whole image (the network is fully convolutional; each tile carries a halo wider than
//! its receptive field). On macOS it runs on the GPU (Metal); elsewhere on the CPU.
//!
//! Modified work (Apache License 2.0, §4(b)): the network follows the SPAN reference
//! implementation (github.com/hongyuanyu/SPAN, `basicsr/archs/span_arch.py`, Apache-2.0,
//! Copyright 2024 Cheng Wan et al.) and its neosr / spandrel ports, re-written in Rust on candle
//! by the LightCraft contributors in 2026. See NOTICE.
#![cfg(not(target_arch = "wasm32"))]
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod models;
mod span;
#[doc(hidden)]
pub mod testing;
#[cfg(test)]
mod tests;
mod tile;

pub use span::Span;
pub use tile::Options;

/// What can go wrong loading or running a model. Never a panic: a damaged, truncated or foreign
/// file is an error the user can act on.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The weights file is missing, damaged, or not a model this crate runs.
    #[error("model: {0}")]
    Model(String),
    /// The image is empty, or its enlargement would not fit in memory.
    #[error("image: {0}")]
    Image(String),
    /// The caller's progress callback asked to stop.
    #[error("cancelled")]
    Cancelled,
}

pub type Result<T> = std::result::Result<T, Error>;

impl From<candle_core::Error> for Error {
    fn from(e: candle_core::Error) -> Self {
        Error::Model(e.to_string())
    }
}

/// The best device here: Metal on macOS and iOS when there is one, otherwise the CPU.
pub fn best_device() -> candle_core::Device {
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    if let Ok(d) = candle_core::Device::new_metal(0) {
        return d;
    }
    candle_core::Device::Cpu
}
