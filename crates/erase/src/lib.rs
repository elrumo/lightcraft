//! **AI Remove**: fills the area under a Remove stroke with what the photo most likely shows
//! behind it, using MI-GAN (Sargsyan, Navasardyan, Xu & Shi, "MI-GAN: A Simple Baseline for Image
//! Inpainting on Mobile Devices", ICCV 2023; code and weights MIT, Picsart AI Research). It runs on
//! rten, a pure-Rust ONNX runtime, so the same code serves the desktop app, the phone app and the
//! browser.
//!
//! The weights are not part of LightCraft: the user downloads them when they ask for the feature
//! ([`FILE`], [`FILE_SIZE`], [`FILE_SHA256`] and [`BASE_URL`] say what to fetch). [`Eraser::load`]
//! takes the author's file unchanged. Its `uint8` casts become `Identity` in memory
//! (`lightcraft_vision::onnxfix`), and the bytes on disk are what the SHA-256 checks.
//!
//! The network works on 512 × 512 pixels. The caller crops the area around the hole (the hole and
//! about as much again around it), and [`Eraser::fill`] scales the crop to the network's size and
//! back. The fill therefore has at most 512 × 512 pixels of detail over the crop.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use lightcraft_raster::resample::{Filter, resize};
use lightcraft_raster::{Plane, Rgb32f};
use rten::{Model, ModelOptions, NodeId, ValueView};
use rten_tensor::AsView;
use sha2::{Digest, Sha256};

/// The network's input and output size (square).
pub const SIDE: usize = 512;
/// Largest image [`Eraser::fill`] takes (per side): a crop around a stroke, never a whole photo.
pub const MAX_SIDE: usize = 8192;

/// Stable id of the model: its folder under `models/` and its id in commands.
pub const ID: &str = "migan";
/// The model's name in messages.
pub const LABEL: &str = "MI-GAN";
/// The model file as one of MI-GAN's authors published it (the 512 × 512 Places2 model with its
/// resize and blend steps).
pub const FILE: &str = "migan.onnx";
pub const FILE_SIZE: u64 = 29_546_882;
pub const FILE_SHA256: &str = "593eba0b7e04730f1b61c0a3cbca68d97d8d6a7ff5c6a44a7b9d7fcd880fc5ae";
/// Where the file comes from: the author's Hugging Face repository at a fixed revision (public,
/// no account; it sends CORS headers, so the browser build can fetch it too). `<BASE_URL>/<FILE>`.
pub const BASE_URL: &str = "https://huggingface.co/andraniksargsyan/migan/resolve/406830d0fa60666da0071c342ad2fbc8f30c5c64";
/// Licence verdict, read 2026-10-12: the code's `LICENSE` and the weights' `LICENSE-WEIGHTS` at
/// github.com/Picsart-AI-Research/MI-GAN are both MIT, as is the Hugging Face repository's card.
/// (The Places2 training images have terms of their own; see docs/ai-models.md.)
pub const LICENCE: &str = "MIT";
pub const LICENCE_URL: &str = "https://github.com/Picsart-AI-Research/MI-GAN/blob/main/LICENSE-WEIGHTS";
pub const CREDIT: &str = "MI-GAN by Andranik Sargsyan, Shant Navasardyan, Xingqian Xu and Humphrey Shi (Picsart AI Research)";
pub const HOME_URL: &str = "https://github.com/Picsart-AI-Research/MI-GAN";

/// What can go wrong loading or running the model. Never a panic: a damaged, truncated or foreign
/// file, or an image of the wrong shape, is an error the user can act on.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The file is missing, damaged, or not the model this crate runs.
    #[error("AI Remove model: {0}")]
    Model(String),
    /// The image or hole is empty, too large, or doesn't match.
    #[error("AI Remove: {0}")]
    Image(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Whether `bytes` is exactly the pinned model file (size and SHA-256).
pub fn verify(bytes: &[u8]) -> bool {
    bytes.len() as u64 == FILE_SIZE && hex(&Sha256::digest(bytes)) == FILE_SHA256
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// MI-GAN, loaded.
pub struct Eraser {
    model: Model,
    image: NodeId,
    mask: NodeId,
    output: NodeId,
}

impl Eraser {
    /// Loads the model from the bytes of [`FILE`].
    pub fn load(bytes: &[u8]) -> Result<Eraser> {
        let bad = |e: &dyn std::fmt::Display| Error::Model(e.to_string());
        let fixed = lightcraft_vision::onnxfix::inference_only(bytes).map_err(|e| bad(&e))?;
        let model = ModelOptions::with_all_ops().load(fixed).map_err(|e| bad(&e))?;
        let node = |n: &str| model.node_id(n).map_err(|e| bad(&format!("no {n}: {e}")));
        let (image, mask) = (node("image")?, node("mask")?);
        let output = model.output_ids().first().copied().ok_or_else(|| bad(&"the model has no output"))?;
        Ok(Eraser { model, image, mask, output })
    }

    /// `img` with the pixels where `hole` is set (one flag per pixel, row-major) filled. `img` is
    /// display-encoded (sRGB-like, 0..=1) and at most [`MAX_SIDE`] per side; the pixels outside
    /// the hole are returned unchanged.
    pub fn fill(&self, img: &Rgb32f, hole: &[bool]) -> Result<Rgb32f> {
        let (w, h) = (img.width, img.height);
        if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE || img.data.len() != w * h {
            return Err(Error::Image(format!("can't fill a {w} × {h} image")));
        }
        if hole.len() != w * h {
            return Err(Error::Image("the area to fill doesn't match the image".into()));
        }
        if !hole.iter().any(|&b| b) {
            return Ok(img.clone());
        }
        let small = resize(img, SIDE, SIDE, Filter::Mitchell);
        // a network pixel is in the hole if any of the hole reaches it (scaled down, a thin
        // stroke must not vanish; scaled up, the edge must not shrink)
        let cover =
            resize(&Plane { width: w, height: h, data: hole.iter().map(|&b| if b { 1.0 } else { 0.0 }).collect() }, SIDE, SIDE, Filter::Bilinear);
        let n = SIDE * SIDE;
        let mut pixels = vec![0f32; 3 * n];
        for (i, p) in small.data.iter().enumerate() {
            for (c, v) in p.iter().enumerate() {
                if let Some(o) = pixels.get_mut(c * n + i) {
                    *o = (v.clamp(0.0, 1.0) * 255.0).round();
                }
            }
        }
        // 255 = keep, 0 = fill
        let mask: Vec<f32> = cover.data.iter().map(|&c| if c > 0.01 { 0.0 } else { 255.0 }).collect();
        let bad = |e: &dyn std::fmt::Display| Error::Model(e.to_string());
        let image = ValueView::from_shape([1, 3, SIDE, SIDE], &pixels[..]).map_err(|e| bad(&e))?;
        let mask = ValueView::from_shape([1, 1, SIDE, SIDE], &mask[..]).map_err(|e| bad(&e))?;
        let out = self.model.run(vec![(self.image, image.into()), (self.mask, mask.into())], &[self.output], None).map_err(|e| bad(&e))?;
        let t = out.first().and_then(|v| v.as_tensor_view::<f32>()).ok_or_else(|| bad(&"its answer isn't numbers"))?;
        let v = t.to_vec();
        if v.len() != 3 * n {
            return Err(bad(&format!("its answer has {} numbers, not {}", v.len(), 3 * n)));
        }
        let filled = Rgb32f::from_fn(SIDE, SIDE, |x, y| {
            let i = y * SIDE + x;
            [0, 1, 2].map(|c| v.get(c * n + i).map_or(0.0, |u| u / 255.0))
        });
        let back = resize(&filled, w, h, Filter::Mitchell);
        let mut res = img.clone();
        for ((o, b), &k) in res.data.iter_mut().zip(&back.data).zip(hole) {
            if k {
                *o = b.map(|c| c.clamp(0.0, 1.0));
            }
        }
        Ok(res)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_wants_the_exact_file() {
        assert!(!verify(b""));
        assert!(!verify(&[0u8; 64]));
        assert_eq!(hex(&Sha256::digest(b"abc")), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn damaged_models_are_errors_not_panics() {
        assert!(Eraser::load(b"").is_err());
        assert!(Eraser::load(&[0xff; 40]).is_err());
        assert!(Eraser::load(b"\x0a\x03abc").is_err());
    }

    /// The model, when a copy is at `$LIGHTCRAFT_MIGAN` (it is never part of the repository).
    fn model() -> Option<Eraser> {
        let path = std::env::var_os("LIGHTCRAFT_MIGAN")?;
        let bytes = std::fs::read(path).ok()?;
        assert!(verify(&bytes), "LIGHTCRAFT_MIGAN is not the pinned file");
        Some(Eraser::load(&bytes).unwrap())
    }

    #[test]
    fn fill_checks_its_arguments() {
        let Some(e) = model() else { return };
        let img = Rgb32f::filled(40, 30, [0.5; 3]);
        assert!(e.fill(&img, &[true; 10]).is_err());
        assert!(e.fill(&Rgb32f::new(0, 0), &[]).is_err());
        // nothing to fill: the image comes back as it is
        assert_eq!(e.fill(&img, &vec![false; 1200]).unwrap().data, img.data);
    }

    /// A dark square on a gradient is replaced by the gradient, and nothing outside the hole moves.
    #[test]
    fn fill_continues_a_gradient() {
        let Some(e) = model() else { return };
        let (w, h) = (300, 200);
        let inside = |x: usize, y: usize| (120..180).contains(&x) && (70..130).contains(&y);
        let grad = |x: usize| [0.2 + 0.6 * x as f32 / w as f32, 0.4, 0.6 - 0.3 * x as f32 / w as f32];
        let img = Rgb32f::from_fn(w, h, |x, y| if inside(x, y) { [0.02; 3] } else { grad(x) });
        let hole: Vec<bool> = (0..w * h).map(|i| inside(i % w, i / w)).collect();
        let out = e.fill(&img, &hole).unwrap();
        for (x, y) in [(150, 100), (125, 75), (175, 125)] {
            let (got, want) = (out.get(x, y), grad(x));
            assert!((0..3).all(|c| (got[c] - want[c]).abs() < 0.06), "({x}, {y}): {got:?} vs {want:?}");
        }
        assert_eq!(out.get(10, 10), img.get(10, 10));
    }
}
