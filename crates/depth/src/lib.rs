//! **Depth estimation** for Lens Blur and the Depth Range mask: how far each part of a photo is
//! from the camera, guessed from the picture alone by Depth Anything V2 Small (Yang, Kang, Huang,
//! Zhao, Xu, Feng & Zhao, "Depth Anything V2", NeurIPS 2024; code and Small weights Apache-2.0).
//! It runs on rten, a pure-Rust ONNX runtime, so the same code serves the desktop app, the phone
//! apps and the browser.
//!
//! The weights are not part of LightCraft: the user downloads them when they first ask for Lens
//! Blur on a photo without depth data of its own ([`FILE`], [`FILE_SIZE`], [`FILE_SHA256`] and
//! [`BASE_URL`] say what to fetch). [`Estimator::load`] takes the published file unchanged.
//!
//! The network sees the photo with its short side at [`SHORT_SIDE`] pixels (both sides multiples
//! of its 14-pixel patches) and answers with *relative inverse depth*: larger is nearer, on a
//! scale of its own for each photo (the caller normalizes it).
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use lightcraft_raster::resample::{Filter, resize};
use lightcraft_raster::{Plane, Rgb32f};
use rten::{Model, ModelOptions, NodeId, ValueView};
use rten_tensor::AsView;
use sha2::{Digest, Sha256};

/// Short side of the picture the network sees (its training size).
pub const SHORT_SIDE: usize = 518;
/// The network's patch size: both sides of its input are multiples of it.
pub const PATCH: usize = 14;
/// Longest side the network is given (a panorama keeps its short side smaller instead).
pub const MAX_LONG: usize = 1036;
/// Largest picture [`Estimator::estimate`] takes (per side).
pub const MAX_SIDE: usize = 16384;

/// Stable id of the model: its folder under `models/` and its id in commands.
pub const ID: &str = "depth-anything-v2-small";
/// The model's name in messages.
pub const LABEL: &str = "Depth Anything V2 Small";
/// The int8 (dynamically quantized) export: a quarter of the fp32 file's size, the same depth to
/// within a few percent of its range on the photos it was checked on (see the tests).
pub const FILE: &str = "model_int8.onnx";
pub const FILE_SIZE: u64 = 27_258_801;
pub const FILE_SHA256: &str = "01aa7a23de3f4a0ee1a2bb9997e6918104c85a9f95dea46d27b9b3fb0c6b9001";
/// The ONNX export's Hugging Face repository at a fixed revision (public, no account; it sends
/// CORS headers, so the browser build can fetch it too). `<BASE_URL>/<FILE>`.
pub const BASE_URL: &str = "https://huggingface.co/onnx-community/depth-anything-v2-small/resolve/4472b7362082ad9968fee890ca0f1e5aca36b93d/onnx";
/// Licence verdict, read 2026-10-14: the Hugging Face cards of onnx-community/depth-anything-v2-small
/// and of the original depth-anything/Depth-Anything-V2-Small both say Apache-2.0, as does the
/// code's `LICENSE` at github.com/DepthAnything/Depth-Anything-V2 for the Small model (its Base,
/// Large and Giant weights are CC-BY-NC-4.0 and must not be used).
pub const LICENCE: &str = "Apache-2.0";
pub const LICENCE_URL: &str = "https://huggingface.co/depth-anything/Depth-Anything-V2-Small";
pub const CREDIT: &str = "Depth Anything V2 Small by Lihe Yang, Bingyi Kang, Zilong Huang, Zhen Zhao, Xiaogang Xu, Jiashi Feng and Hengshuang Zhao; ONNX export by onnx-community";
pub const HOME_URL: &str = "https://github.com/DepthAnything/Depth-Anything-V2";

/// ImageNet statistics the network was trained with.
const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];

/// What can go wrong loading or running the model. Never a panic: a damaged, truncated or foreign
/// file, or an empty image, is an error the user can act on.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The file is missing, damaged, or not the model this crate runs.
    #[error("depth model: {0}")]
    Model(String),
    /// The image is empty or too large.
    #[error("depth estimation: {0}")]
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

/// The network's input size for a `w × h` picture: the short side at [`SHORT_SIDE`] (smaller for
/// a long panorama, so the long side stays within [`MAX_LONG`]), both sides multiples of [`PATCH`].
pub fn net_size(w: usize, h: usize) -> (usize, usize) {
    let (w, h) = (w.max(1) as f64, h.max(1) as f64);
    let k = (SHORT_SIDE as f64 / w.min(h)).min(MAX_LONG as f64 / w.max(h));
    let side = |v: f64| (((v * k) / PATCH as f64).round() as usize).max(1) * PATCH;
    (side(w), side(h))
}

/// Depth Anything V2 Small, loaded.
pub struct Estimator {
    model: Model,
    input: NodeId,
    output: NodeId,
}

impl Estimator {
    /// Loads the model from the bytes of [`FILE`]. They are read through
    /// `lightcraft_vision::onnxfix` first, which checks the protobuf's structure (rten's own
    /// reader can spin on some damaged files).
    pub fn load(bytes: &[u8]) -> Result<Estimator> {
        let bad = |e: &dyn std::fmt::Display| Error::Model(e.to_string());
        let fixed = lightcraft_vision::onnxfix::inference_only(bytes).map_err(|e| bad(&e))?;
        let model = ModelOptions::with_all_ops().load(fixed).map_err(|e| bad(&e))?;
        let input = model.input_ids().first().copied().ok_or_else(|| bad(&"the model has no input"))?;
        let output = model.output_ids().first().copied().ok_or_else(|| bad(&"the model has no output"))?;
        Ok(Estimator { model, input, output })
    }

    /// The relative inverse depth of `img` (display-encoded, sRGB-like 0..=1, at most
    /// [`MAX_SIDE`] per side) at the network's resolution ([`net_size`]): larger is nearer, on a
    /// scale of the network's own.
    pub fn estimate(&self, img: &Rgb32f) -> Result<Plane> {
        let (w, h) = (img.width, img.height);
        if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE || img.data.len() != w * h {
            return Err(Error::Image(format!("can't estimate the depth of a {w} × {h} image")));
        }
        let (nw, nh) = net_size(w, h);
        let small = resize(img, nw, nh, Filter::Mitchell);
        let n = nw * nh;
        let mut pixels = vec![0f32; 3 * n];
        for (i, p) in small.data.iter().enumerate() {
            for c in 0..3 {
                if let Some(o) = pixels.get_mut(c * n + i) {
                    *o = (p[c].clamp(0.0, 1.0) - MEAN[c]) / STD[c];
                }
            }
        }
        let bad = |e: &dyn std::fmt::Display| Error::Model(e.to_string());
        let input = ValueView::from_shape([1, 3, nh, nw], &pixels[..]).map_err(|e| bad(&e))?;
        let out = self.model.run(vec![(self.input, input.into())], &[self.output], None).map_err(|e| bad(&e))?;
        let t = out.first().and_then(|v| v.as_tensor_view::<f32>()).ok_or_else(|| bad(&"its answer isn't numbers"))?;
        let v = t.to_vec();
        if v.len() != n {
            return Err(bad(&format!("its answer has {} numbers, not {n}", v.len())));
        }
        Ok(Plane { width: nw, height: nh, data: v.into_iter().map(|d| if d.is_finite() { d } else { 0.0 }).collect() })
    }
}

#[cfg(test)]
mod tests;
