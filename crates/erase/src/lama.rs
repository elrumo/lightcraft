//! **LaMa** (Suvorov et al., "Resolution-robust Large Mask Inpainting with Fourier Convolutions",
//! WACV 2022; code and weights Apache-2.0, Samsung AI Center): the larger of the two networks
//! Generative Expand can use. Its fast Fourier convolutions see the whole picture at once, so it
//! continues long structures (horizons, shorelines, rows of trees) across a wide new strip better
//! than MI-GAN, at about ten times the time per pass and 208 MB of weights: the desktop apps offer
//! it, phones and the browser don't.
//!
//! The file is the fixed-size (512 × 512) fp32 ONNX export of big-lama published by Carve
//! (`lama_fp32.onnx`), downloaded by the user when they ask, pinned by size and SHA-256. Its FFTs are
//! written as MatMul / Einsum / Cos / Sin, which rten runs as they are.

use lightcraft_raster::Rgb32f;
use rten::{Model, ModelOptions, NodeId, ValueView};
use rten_tensor::AsView;
use sha2::{Digest, Sha256};

use crate::{Error, Painter, Result, SIDE, check_side};

/// Stable id of the model: its folder under `models/` and its id in commands.
pub const ID: &str = "lama";
/// The model's name in messages.
pub const LABEL: &str = "LaMa";
/// The fixed-size fp32 export (the model card recommends it over the dynamo export).
pub const FILE: &str = "lama_fp32.onnx";
pub const FILE_SIZE: u64 = 208_044_816;
pub const FILE_SHA256: &str = "1faef5301d78db7dda502fe59966957ec4b79dd64e16f03ed96913c7a4eb68d6";
/// The export's Hugging Face repository at a fixed revision (public, no account). `<BASE_URL>/<FILE>`.
pub const BASE_URL: &str = "https://huggingface.co/Carve/LaMa-ONNX/resolve/c3c0c9e468934d62e79c329e35d82dd09ff8c444";
/// Licence verdict, read 2026-10-13: the Hugging Face card of Carve/LaMa-ONNX says Apache-2.0, and
/// the original code and weights (github.com/advimman/lama, `LICENSE`) are Apache-2.0. (The
/// Places2 training images have terms of their own; see docs/ai-models.md.)
pub const LICENCE: &str = "Apache-2.0";
pub const LICENCE_URL: &str = "https://github.com/advimman/lama/blob/main/LICENSE";
pub const CREDIT: &str = "LaMa by Roman Suvorov, Elizaveta Logacheva, Anton Mashikhin, Anastasia Remizova, Arsenii Ashukha, Aleksei Silvestrov, Naejin Kong, Harshith Goka, Kiwoong Park and Victor Lempitsky (Samsung AI Center); ONNX export by Carve";
pub const HOME_URL: &str = "https://github.com/advimman/lama";

/// Whether `bytes` is exactly the pinned model file (size and SHA-256).
pub fn verify(bytes: &[u8]) -> bool {
    bytes.len() as u64 == FILE_SIZE && crate::hex(&Sha256::digest(bytes)) == FILE_SHA256
}

/// LaMa, loaded.
pub struct Lama {
    model: Model,
    image: NodeId,
    mask: NodeId,
    output: NodeId,
}

impl Lama {
    /// Loads the model from the bytes of [`FILE`]. They are read through
    /// `lightcraft_vision::onnxfix` first, which checks the protobuf's structure (rten's own
    /// reader can spin on some damaged files) and leaves the graph runnable as it is.
    pub fn load(bytes: &[u8]) -> Result<Lama> {
        let bad = |e: &dyn std::fmt::Display| Error::Model(format!("LaMa: {e}"));
        let fixed = lightcraft_vision::onnxfix::inference_only(bytes).map_err(|e| bad(&e))?;
        let model = ModelOptions::with_all_ops().load(fixed).map_err(|e| bad(&e))?;
        let node = |n: &str| model.node_id(n).map_err(|e| bad(&format!("no {n}: {e}")));
        let (image, mask) = (node("image")?, node("mask")?);
        let output = model.output_ids().first().copied().ok_or_else(|| bad(&"the model has no output"))?;
        Ok(Lama { model, image, mask, output })
    }
}

impl Painter for Lama {
    fn paint(&self, img: &Rgb32f, hole: &[bool]) -> Result<Rgb32f> {
        check_side(img, hole)?;
        let n = SIDE * SIDE;
        // planar RGB 0..=1, mask 1 = fill
        let mut pixels = vec![0f32; 3 * n];
        for (i, p) in img.data.iter().enumerate() {
            for (c, v) in p.iter().enumerate() {
                if let Some(o) = pixels.get_mut(c * n + i) {
                    *o = v.clamp(0.0, 1.0);
                }
            }
        }
        let mask: Vec<f32> = hole.iter().map(|&b| if b { 1.0 } else { 0.0 }).collect();
        let bad = |e: &dyn std::fmt::Display| Error::Model(format!("LaMa: {e}"));
        let image = ValueView::from_shape([1, 3, SIDE, SIDE], &pixels[..]).map_err(|e| bad(&e))?;
        let mask = ValueView::from_shape([1, 1, SIDE, SIDE], &mask[..]).map_err(|e| bad(&e))?;
        let out = self.model.run(vec![(self.image, image.into()), (self.mask, mask.into())], &[self.output], None).map_err(|e| bad(&e))?;
        let t = out.first().and_then(|v| v.as_tensor_view::<f32>()).ok_or_else(|| bad(&"its answer isn't numbers"))?;
        let v = t.to_vec();
        if v.len() != 3 * n {
            return Err(bad(&format!("its answer has {} numbers, not {}", v.len(), 3 * n)));
        }
        // the export answers 0..=255
        Ok(Rgb32f::from_fn(SIDE, SIDE, |x, y| {
            let i = y * SIDE + x;
            [0, 1, 2].map(|c| v.get(c * n + i).map_or(0.0, |u| if u.is_finite() { (u / 255.0).clamp(0.0, 1.0) } else { 0.0 }))
        }))
    }

    fn label(&self) -> &'static str {
        LABEL
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn verify_wants_the_exact_file() {
        assert!(!verify(b""));
        assert!(!verify(&[0u8; 64]));
    }

    #[test]
    fn damaged_models_are_errors_not_panics() {
        assert!(Lama::load(b"").is_err());
        assert!(Lama::load(&[0xff; 40]).is_err());
        assert!(Lama::load(b"\x0a\x03abc").is_err());
    }

    /// The model, when a copy is at `$LIGHTCRAFT_LAMA` (it is never part of the repository).
    pub(crate) fn model() -> Option<Lama> {
        let path = std::env::var_os("LIGHTCRAFT_LAMA")?;
        let bytes = std::fs::read(path).ok()?;
        assert!(verify(&bytes), "LIGHTCRAFT_LAMA is not the pinned file");
        Some(Lama::load(&bytes).unwrap())
    }

    /// The right part of a horizontal gradient is painted back as its continuation: the same
    /// colour at the edge, the same direction further in. (LaMa flattens a smooth gradient away
    /// from the edge rather than extrapolating it; on this machine its answers match onnxruntime's
    /// to 2e-6.)
    #[test]
    fn paint_continues_a_gradient() {
        let Some(m) = model() else { return };
        let grad = |x: usize| [0.2 + 0.6 * x as f32 / SIDE as f32, 0.4, 0.6 - 0.3 * x as f32 / SIDE as f32];
        let img = Rgb32f::from_fn(SIDE, SIDE, |x, _| if x < 400 { grad(x) } else { [0.0; 3] });
        let hole: Vec<bool> = (0..SIDE * SIDE).map(|i| i % SIDE >= 400).collect();
        let out = m.paint(&img, &hole).unwrap();
        let (got, want) = (out.get(406, 256), grad(406));
        assert!((0..3).all(|c| (got[c] - want[c]).abs() < 0.08), "at the edge: {got:?} vs {want:?}");
        let far = out.get(480, 256);
        assert!(far[0] > grad(200)[0] && far[2] < grad(200)[2], "further in: {far:?}");
        assert!(m.paint(&Rgb32f::new(10, 10), &[false; 100]).is_err());
    }
}
