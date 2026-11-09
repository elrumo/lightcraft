//! **Depth estimation** for Lens Blur and the Depth Range mask: how far each part of a photo is
//! from the camera, guessed from the picture alone by Depth Anything 3 (Lin et al., "Depth
//! Anything 3: Recovering the Visual Space from Any Views", ByteDance Seed 2025), in the size the
//! user chose ([`Size`]: Small or Base, both Apache-2.0). It runs on rten, a pure-Rust ONNX
//! runtime, so the same code serves the desktop app, the phone apps and the browser.
//!
//! The weights are not part of LightCraft: the user downloads them when they first ask for Lens
//! Blur on a photo without depth data of their own ([`Size::graph`], [`Size::data`] and
//! [`Size::base_url`] say what to fetch). [`Estimator::load`] takes the published files unchanged.
//!
//! The network sees the photo with its short side at [`SHORT_SIDE`] pixels (both sides multiples
//! of its 14-pixel patches), answers with depth (larger is farther), and [`Estimator::estimate`]
//! returns *relative inverse depth*: larger is nearer, on a scale of its own for each photo (the
//! caller normalizes it). The older Depth Anything V2 Small stays as the GPU weights ([`gpu`]).
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

#[cfg(all(feature = "gpu", not(target_arch = "wasm32")))]
pub mod gpu;
pub mod surface;

use lightcraft_raster::resample::{Filter, resize};
use lightcraft_raster::{Plane, Rgb32f};
use rten::{Model, ModelOptions, NodeId, ValueView};
use rten_tensor::AsView;
use sha2::{Digest, Sha256};

/// Short side of the picture Depth Anything 3 sees (its default processing resolution).
pub const SHORT_SIDE: usize = 504;
/// The network's patch size: both sides of its input are multiples of it.
pub const PATCH: usize = 14;
/// Longest side the network is given (a panorama keeps its short side smaller instead).
pub const MAX_LONG: usize = 1008;
/// Short and longest side of the V2 network on the GPU (its training size).
#[cfg(any(test, all(feature = "gpu", not(target_arch = "wasm32"))))]
const V2_SHORT_SIDE: usize = 518;
#[cfg(any(test, all(feature = "gpu", not(target_arch = "wasm32"))))]
const V2_MAX_LONG: usize = 1036;
/// Largest picture [`Estimator::estimate`] takes (per side).
pub const MAX_SIDE: usize = 16384;

/// Licence verdict, read 2026-11-07: the Hugging Face cards of depth-anything/DA3-SMALL, DA3-BASE
/// and of onnx-community's exports of them say Apache-2.0, as does the code's `LICENSE` at
/// github.com/ByteDance-Seed/depth-anything-3 for these two (its Large and Giant weights are
/// CC-BY-NC-4.0 and must not be used; the 1.1 Large says Apache-2.0 on its card but is 1.4 GB, past
/// what the ONNX reader takes, so it isn't offered).
pub const LICENCE: &str = "Apache-2.0";
pub const LICENCE_URL: &str = "https://huggingface.co/depth-anything/DA3-SMALL";
pub const CREDIT: &str = "Depth Anything 3 by Haotong Lin, Sili Chen, Jun Hao Liew, Donny Y. Chen, Zhenyu Li, Guang Shi, Jiashi Feng and Bingyi Kang (ByteDance Seed); ONNX export by onnx-community";
pub const HOME_URL: &str = "https://github.com/ByteDance-Seed/depth-anything-3";

/// One pinned file: name, size and SHA-256.
#[derive(Clone, Copy, Debug)]
pub struct Pinned {
    pub name: &'static str,
    pub size: u64,
    pub sha256: &'static str,
}

impl Pinned {
    /// Whether `bytes` is exactly this file (size and SHA-256).
    pub fn matches(&self, bytes: &[u8]) -> bool {
        bytes.len() as u64 == self.size && hex(&Sha256::digest(bytes)) == self.sha256
    }
}

/// Which Depth Anything 3 the user chose. Small is the default: the quickest, 105 MB. Base sees
/// finer edges, 412 MB and about three times slower.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Size {
    #[default]
    Small,
    Base,
}

impl Size {
    pub const ALL: [Size; 2] = [Size::Small, Size::Base];

    /// Stable id: its folder under `models/` and its id in commands (`small` and `base` name the size).
    pub const fn id(self) -> &'static str {
        match self {
            Size::Small => "depth-anything-3-small",
            Size::Base => "depth-anything-3-base",
        }
    }

    /// The size's name in settings and commands: `small` or `base`.
    pub const fn name(self) -> &'static str {
        match self {
            Size::Small => "small",
            Size::Base => "base",
        }
    }

    pub fn from_name(name: &str) -> Option<Size> {
        Size::ALL.into_iter().find(|s| s.name() == name)
    }

    /// The model's name in messages.
    pub const fn label(self) -> &'static str {
        match self {
            Size::Small => "Depth Anything 3 Small",
            Size::Base => "Depth Anything 3 Base",
        }
    }

    /// The ONNX graph (the small file: the network's structure).
    pub const fn graph(self) -> Pinned {
        match self {
            Size::Small => Pinned { name: GRAPH_FILE, size: 640_691, sha256: "396008798244a074297fd88e450433b1357fc687f534939375c804ded86e7b2a" },
            Size::Base => Pinned { name: GRAPH_FILE, size: 646_912, sha256: "d9e7775d8b0340ce9416cfdb4356651ef72abab75eb1485f0aeefa352d78255c" },
        }
    }

    /// The weights (the graph refers to them by [`DATA_FILE`]).
    pub const fn data(self) -> Pinned {
        match self {
            Size::Small => Pinned { name: DATA_FILE, size: 104_702_464, sha256: "802bb24741e67f5bb2b369fc64d40afe11439cc895d676d658d65cfb75c9860f" },
            Size::Base => Pinned { name: DATA_FILE, size: 412_068_352, sha256: "09de4dcff3fefe48b04fedbf146151f3339e6115416a72bf9b36dee3aafd03bd" },
        }
    }

    /// The ONNX export's Hugging Face repository at a fixed revision (public, no account; it sends
    /// CORS headers, so the browser build can fetch it too). `<base_url>/<file name>`.
    pub const fn base_url(self) -> &'static str {
        match self {
            Size::Small => "https://huggingface.co/onnx-community/depth-anything-v3-small/resolve/0b6a7f3bf5595f9950b91389e0da3a0de130324c/onnx",
            Size::Base => "https://huggingface.co/onnx-community/depth-anything-v3-base/resolve/c56b234c94b459a116d5dc782edf4f2db5889276/onnx",
        }
    }

    /// Whether `graph` and `data` are exactly the pinned files (size and SHA-256).
    pub fn verify(self, graph: &[u8], data: &[u8]) -> bool {
        self.graph().matches(graph) && self.data().matches(data)
    }
}

/// The graph's and the weights' file names (the same for every size).
pub const GRAPH_FILE: &str = "model.onnx";
pub const DATA_FILE: &str = "model.onnx_data";

/// The same network on the GPU (candle on Metal), from the authors' original weights: a model of its own, in a folder of its
/// own. Where it comes from: a community conversion of the Apache-2.0 `depth_anything_v2_vits.pth` to safetensors.
pub const GPU_ID: &str = "depth-anything-v2-small-gpu";
pub const GPU_LABEL: &str = "Depth Anything V2 Small (GPU)";
pub const GPU_FILE: &str = "depth_anything_v2_vits.safetensors";
pub const GPU_FILE_SIZE: u64 = 99_165_428;
pub const GPU_FILE_SHA256: &str = "07635472183d063a80e6b2d78645ab944a9210fd973b1c88e4b7e3ba23981f75";
/// V2 Small's licence (Apache-2.0; its Base, Large and Giant are CC-BY-NC-4.0 and must not be used), read 2026-10-14.
pub const GPU_LICENCE_URL: &str = "https://huggingface.co/depth-anything/Depth-Anything-V2-Small";
pub const GPU_CREDIT: &str =
    "Depth Anything V2 Small by Lihe Yang, Bingyi Kang, Zilong Huang, Zhen Zhao, Xiaogang Xu, Jiashi Feng and Hengshuang Zhao";
pub const GPU_HOME_URL: &str = "https://github.com/DepthAnything/Depth-Anything-V2";
pub const GPU_BASE_URL: &str = "https://huggingface.co/jeroenvlek/depth-anything-v2-safetensors/resolve/a8cd5eb93485537f612b31b78864b41f659b245d";

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

/// Whether `bytes` is exactly the pinned GPU weights file (size and SHA-256).
pub fn verify_gpu(bytes: &[u8]) -> bool {
    bytes.len() as u64 == GPU_FILE_SIZE && hex(&Sha256::digest(bytes)) == GPU_FILE_SHA256
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// The network's input size for a `w × h` picture: the short side at [`SHORT_SIDE`] (smaller for
/// a long panorama, so the long side stays within [`MAX_LONG`]), both sides multiples of [`PATCH`].
pub fn net_size(w: usize, h: usize) -> (usize, usize) {
    net_size_at(SHORT_SIDE, MAX_LONG, w, h)
}

fn net_size_at(short: usize, long: usize, w: usize, h: usize) -> (usize, usize) {
    let (w, h) = (w.max(1) as f64, h.max(1) as f64);
    let k = (short as f64 / w.min(h)).min(long as f64 / w.max(h));
    let side = |v: f64| (((v * k) / PATCH as f64).round() as usize).max(1) * PATCH;
    (side(w), side(h))
}

/// Depth Anything 3 (or V2 Small on the GPU), loaded.
pub struct Estimator {
    backend: Backend,
}

enum Backend {
    Onnx {
        model: Box<Model>,
        input: NodeId,
        output: NodeId,
    },
    #[cfg(all(feature = "gpu", not(target_arch = "wasm32")))]
    Gpu(Box<gpu::Gpu>),
}

impl Estimator {
    /// Loads Depth Anything 3 from the bytes of [`Size::graph`] and [`Size::data`]. The graph is read
    /// through `lightcraft_vision::onnxfix` first, which checks the protobuf's structure (rten's
    /// own reader can spin on some damaged files).
    pub fn load(graph: &[u8], data: Vec<u8>) -> Result<Estimator> {
        let bad = |e: &dyn std::fmt::Display| Error::Model(e.to_string());
        let fixed = lightcraft_vision::onnxfix::inference_only(graph).map_err(|e| bad(&e))?;
        let mut options = ModelOptions::with_all_ops();
        options.external_data(DATA_FILE, data);
        let model = options.load(fixed).map_err(|e| bad(&e))?;
        let input = model.input_ids().first().copied().ok_or_else(|| bad(&"the model has no input"))?;
        // (`predicted_depth` is its first answer; the others, confidence and camera, aren't asked for)
        let output = model.output_ids().first().copied().ok_or_else(|| bad(&"the model has no output"))?;
        Ok(Estimator { backend: Backend::Onnx { model: Box::new(model), input, output } })
    }

    /// The model on the GPU, from the original weights (`gpu::WEIGHTS_FILE`); `None` when the machine has no GPU.
    #[cfg(all(feature = "gpu", not(target_arch = "wasm32")))]
    pub fn load_gpu(file: &std::path::Path) -> Result<Option<Estimator>> {
        Ok(gpu::Gpu::load(file)?.map(|g| Estimator { backend: Backend::Gpu(Box::new(g)) }))
    }

    /// What it runs on: `cpu` or `gpu`.
    pub fn backend(&self) -> &'static str {
        match self.backend {
            Backend::Onnx { .. } => "cpu",
            #[cfg(all(feature = "gpu", not(target_arch = "wasm32")))]
            Backend::Gpu(_) => "gpu",
        }
    }

    /// The relative inverse depth of `img` (display-encoded, sRGB-like 0..=1, at most
    /// [`MAX_SIDE`] per side) at the network's resolution ([`net_size`]): larger is nearer, on a
    /// scale of the network's own.
    pub fn estimate(&self, img: &Rgb32f) -> Result<Plane> {
        let (w, h) = (img.width, img.height);
        if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE || img.data.len() != w * h {
            return Err(Error::Image(format!("can't estimate the depth of a {w} × {h} image")));
        }
        let (nw, nh) = match self.backend {
            Backend::Onnx { .. } => net_size(w, h),
            #[cfg(all(feature = "gpu", not(target_arch = "wasm32")))]
            Backend::Gpu(_) => net_size_at(V2_SHORT_SIDE, V2_MAX_LONG, w, h),
        };
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
        let v: Vec<f32> = match &self.backend {
            #[cfg(all(feature = "gpu", not(target_arch = "wasm32")))]
            Backend::Gpu(g) => g.run(&pixels, nw, nh)?,
            Backend::Onnx { model, input, output } => {
                // (batch, pictures, channels, height, width: one picture)
                let value = ValueView::from_shape([1, 1, 3, nh, nw], &pixels[..]).map_err(|e| bad(&e))?;
                let out = model.run(vec![(*input, value.into())], &[*output], None).map_err(|e| bad(&e))?;
                let t = out.first().and_then(|v| v.as_tensor_view::<f32>()).ok_or_else(|| bad(&"its answer isn't numbers"))?;
                // depth, larger is farther: nearness is its inverse
                t.to_vec().into_iter().map(|d| if d.is_finite() && d > 1e-6 { 1.0 / d } else { 0.0 }).collect()
            }
        };
        if v.len() != n {
            return Err(bad(&format!("its answer has {} numbers, not {n}", v.len())));
        }
        Ok(Plane { width: nw, height: nh, data: v.into_iter().map(|d| if d.is_finite() { d } else { 0.0 }).collect() })
    }
}

#[cfg(test)]
mod tests;
