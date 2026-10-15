//! **Generative Expand's diffusion models**, on candle, in pure Rust: the larger, slower models
//! that draw new detail past a photo's edges, where MI-GAN and LaMa (`lightcraft-erase`) continue
//! what is there at 512 pixels.
//!
//! - **Stable Diffusion 1.5 Inpainting** ([`models::SD15`], CreativeML OpenRAIL-M): 512 pixels,
//!   about 2.1 GB, the one a phone can run (a recent iPhone or iPad with enough memory).
//! - **Stable Diffusion XL Inpainting** ([`models::SDXL`], CreativeML Open RAIL++-M): 1024 pixels,
//!   about 6.9 GB; a computer with a GPU and 16 GB or more.
//! - **Kandinsky 2.2 Inpaint** ([`models::KANDINSKY`], Apache-2.0): 768 pixels, about 9 GB;
//!   conditioned on the photo itself (its CLIP image embedding), no prompt.
//!
//! Each is a [`lightcraft_erase::Painter`]: [`lightcraft_erase::expand::outpaint`] gives it the
//! canvas in the photo's proportions at its native size ([`Painter::frame`]), and it runs its
//! denoising steps with progress and cancellation between them ([`Painter::paint_watched`]). The
//! weights are never part of LightCraft: the user downloads them after reading the licence
//! ([`models`] says what to fetch and pins every file by size and SHA-256), and this crate reads
//! the `.safetensors` files a tensor at a time ([`weights`]), so loading never holds a file and
//! its converted copy at once. On macOS and iOS the models run on the GPU (Metal, 16-bit); on
//! other computers on the CPU (32-bit).
//!
//! The same canvas gives the same answer: the starting noise comes from a fixed seed.
//!
//! Modified work (Apache License 2.0, §4(b)): the UNet ([`unet`]) is put together from
//! candle-transformers' Stable Diffusion blocks (github.com/huggingface/candle, MIT OR
//! Apache-2.0) following its `unet_2d.rs`, with SDXL's added time and text conditioning; the
//! Kandinsky 2.2 networks ([`kandinsky`]) follow the diffusers implementation
//! (github.com/huggingface/diffusers, Apache-2.0, Copyright The HuggingFace Team): both re-written
//! in Rust by the LightCraft contributors in 2026. See NOTICE.
#![cfg(not(target_arch = "wasm32"))]
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod clip;
pub mod kandinsky;
pub mod models;
pub(crate) mod noise;
pub(crate) mod schedule;
pub mod sd;
pub mod seam;
pub mod tokenizer;
pub mod unet;
pub mod weights;

pub use lightcraft_erase::Painter;

/// What can go wrong loading or running a model. Never a panic: a damaged, truncated or foreign
/// file, or a canvas of the wrong shape, is an error the user can act on.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A file is missing, damaged, or not part of the model this crate runs.
    #[error("{0}")]
    Model(String),
    /// The canvas or the area to paint is empty, too large, or doesn't match.
    #[error("{0}")]
    Image(String),
    /// The user stopped the work.
    #[error("cancelled")]
    Cancelled,
}

pub type Result<T> = std::result::Result<T, Error>;

impl From<candle_core::Error> for Error {
    fn from(e: candle_core::Error) -> Self {
        Error::Model(e.to_string())
    }
}

impl Error {
    /// As a [`lightcraft_erase::Error`], naming the model.
    pub fn into_erase(self, label: &str) -> lightcraft_erase::Error {
        match self {
            Error::Cancelled => lightcraft_erase::Error::Cancelled,
            Error::Model(e) => lightcraft_erase::Error::Other(format!("{label}: {e}")),
            Error::Image(e) => lightcraft_erase::Error::Image(e),
        }
    }
}

/// Where the models run: the GPU (Metal) on macOS and iOS when there is one, else the CPU.
#[derive(Clone, Debug)]
pub struct Target {
    pub device: candle_core::Device,
    /// What the weights and activations are held in: 16-bit on the GPU, 32-bit on the CPU (whose
    /// 16-bit arithmetic is slow).
    pub dtype: candle_core::DType,
    /// Attention computed a few heads at a time, so a phone's peak memory stays low (a little
    /// slower).
    pub sliced_attention: bool,
}

impl Target {
    /// The best place here: Metal on macOS and iOS when there is one, else the CPU (found once
    /// per run).
    pub fn best() -> Target {
        static BEST: std::sync::OnceLock<Target> = std::sync::OnceLock::new();
        BEST.get_or_init(|| {
            #[cfg(any(target_os = "macos", target_os = "ios"))]
            if let Ok(device) = candle_core::Device::new_metal(0) {
                return Target { device, dtype: candle_core::DType::F16, sliced_attention: cfg!(target_os = "ios") };
            }
            Target::cpu()
        })
        .clone()
    }

    /// The CPU, in 32-bit.
    pub fn cpu() -> Target {
        Target { device: candle_core::Device::Cpu, dtype: candle_core::DType::F32, sliced_attention: false }
    }

    /// Whether this runs on a GPU.
    pub fn gpu(&self) -> bool {
        !self.device.is_cpu()
    }
}

/// Whether a GPU (Metal) is there for these models.
pub fn has_gpu() -> bool {
    Target::best().gpu()
}
