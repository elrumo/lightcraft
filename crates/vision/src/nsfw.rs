//! Falconsai's NSFW image classifier (a ViT-B/16, Apache-2.0, `Falconsai/nsfw_image_detection`): our own
//! code reading its published conventions. It takes a 224×224 RGB picture scaled to −1…1 (mean and standard
//! deviation 0.5) and answers two logits, `normal` and `nsfw`; the score is the second one's softmax.
//!
//! It runs on the GPU (candle on Metal, in half precision) when the folder holds the original weights
//! ([`GPU_WEIGHTS_FILE`]) and the machine has one, else on `rten`, a pure-Rust ONNX runtime, from the int8
//! export ([`MODEL_FILE`]): the same scores to within a few hundredths either way.
//!
//! The **large** model ([`LARGE_DIR`]: AdamCodd's `vit-base-nsfw-detector`, Apache-2.0) is the same ViT-B/16
//! trained on more pictures and reading 384×384 instead of 224×224. Installed, it is what `Auto` runs, on the
//! CPU (int8); its scores are kept apart (its own [`LARGE_ENGINE`]), so a photo is checked again by it.

use std::path::Path;

use lightcraft_raster::resample::{self, Filter};
use lightcraft_raster::{Image, Rgb32f, Rgba8};
use rten::{Model, ModelOptions, NodeId, ValueView};
use rten_tensor::AsView;

use crate::sensitive::Classifier;
use crate::store::Error;

/// The model's id in the scores' store (also the folder's file and what the server reports).
pub const ENGINE: &str = "falconsai-nsfw";
/// The file [`Nsfw::load`] reads from the model's folder.
pub const MODEL_FILE: &str = "model_int8.onnx";
/// The original weights (safetensors, f32): what the GPU runs; the ONNX file is for machines without one.
pub const GPU_WEIGHTS_FILE: &str = "model.safetensors";
/// The input's side, in pixels.
const SIDE: usize = 224;
/// The large model's id in the scores' store.
pub const LARGE_ENGINE: &str = "adamcodd-nsfw-384";
/// The large model's folder inside the model's own (its file is [`MODEL_FILE`] too).
pub const LARGE_DIR: &str = "large";
/// The large model's input side.
const LARGE_SIDE: usize = 384;
/// Largest photo accepted, in pixels.
const MAX_PIXELS: usize = 64 << 20;

/// Which weights to run: the user's choice.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Prefer {
    /// The GPU's when there is a GPU and its weights are here, else the CPU's.
    #[default]
    Auto,
    /// The int8 export, on the CPU (an error when its file isn't here).
    Cpu,
    /// The original weights, on the GPU (an error when they aren't here or there is no GPU).
    Gpu,
}

impl Prefer {
    pub fn from_name(name: &str) -> Option<Prefer> {
        match name {
            "auto" => Some(Prefer::Auto),
            "cpu" => Some(Prefer::Cpu),
            "gpu" => Some(Prefer::Gpu),
            _ => None,
        }
    }
}

/// The classifier, loaded.
pub struct Nsfw {
    backend: Backend,
    /// The input's side, in pixels.
    side: usize,
    engine: &'static str,
}

enum Backend {
    Onnx {
        model: Model,
        input: NodeId,
        output: NodeId,
    },
    #[cfg(feature = "siglip")]
    Gpu(gpu::Vit),
}

/// Whether the large model's int8 file (the CPU's) is in `dir`.
pub fn large_cpu_installed(dir: &Path) -> bool {
    dir.join(LARGE_DIR).join(MODEL_FILE).is_file()
}

/// Whether the large model's original weights (the GPU's) are in `dir`.
pub fn large_gpu_installed(dir: &Path) -> bool {
    dir.join(LARGE_DIR).join(GPU_WEIGHTS_FILE).is_file()
}

/// Whether any of the large model's files is in `dir`.
pub fn large_installed(dir: &Path) -> bool {
    large_cpu_installed(dir) || large_gpu_installed(dir)
}

/// Whether `dir` holds a file [`Nsfw::load`] reads.
pub fn is_model_dir(dir: &Path) -> bool {
    dir.join(MODEL_FILE).is_file() || dir.join(GPU_WEIGHTS_FILE).is_file() || large_installed(dir)
}

/// The id [`Nsfw::load_with`] gives its scores for `dir` and `prefer`, known without loading it.
pub fn engine_for(dir: &Path, prefer: Prefer) -> &'static str {
    if prefer == Prefer::Auto && large_installed(dir) { LARGE_ENGINE } else { ENGINE }
}

/// The ViT on the GPU.
#[cfg(feature = "siglip")]
mod gpu {
    use candle_core::{DType, Device, Tensor};
    use candle_nn::VarBuilder;
    use candle_transformers::models::vit;

    use super::GPU_WEIGHTS_FILE;
    use crate::store::Error;
    use crate::weights::{Backend, Weights};

    pub struct Vit {
        model: vit::Model,
        device: Device,
        /// The input's side, in pixels.
        side: usize,
    }

    impl Vit {
        /// Loads the weights in `dir` (a ViT-B/16 reading `side` pixels) onto the GPU; `None` when there are none or the machine has no GPU.
        pub fn load(dir: &std::path::Path, side: usize) -> Result<Option<Vit>, Error> {
            let file = dir.join(GPU_WEIGHTS_FILE);
            let device = crate::siglip::best_device();
            if !file.is_file() || device.is_cpu() {
                return Ok(None);
            }
            // (read tensor by tensor, and converted to half precision on the way: the file is 343 MB)
            let vb = VarBuilder::from_backend(Box::new(Backend(Weights::open(&file)?)), DType::F16, device.clone());
            let config = vit::Config { image_size: side, ..vit::Config::vit_base_patch16_224() };
            let model = vit::Model::new(&config, 2, vb)?;
            let vit = Vit { model, device, side };
            // (the first picture doesn't pay for compiling the kernels)
            vit.run(&vec![0f32; 3 * side * side])?;
            Ok(Some(vit))
        }

        pub fn run(&self, data: &[f32]) -> Result<Vec<f32>, Error> {
            let input = Tensor::from_slice(data, (1, 3, self.side, self.side), &self.device)?.to_dtype(DType::F16)?;
            Ok(self.model.forward(&input)?.to_dtype(DType::F32)?.flatten_all()?.to_vec1::<f32>()?)
        }
    }
}

impl Nsfw {
    pub fn load(dir: &Path) -> Result<Nsfw, Error> {
        Self::load_with(dir, Prefer::Auto)
    }

    /// Loads the weights `prefer` names (see [`Prefer`]).
    pub fn load_with(dir: &Path, prefer: Prefer) -> Result<Nsfw, Error> {
        if !is_model_dir(dir) {
            return Err(Error::Missing(dir.to_path_buf()));
        }
        if prefer == Prefer::Auto && large_installed(dir) {
            let large = dir.join(LARGE_DIR);
            #[cfg(feature = "siglip")]
            if let Some(vit) = gpu::Vit::load(&large, LARGE_SIDE)? {
                return Ok(Nsfw { backend: Backend::Gpu(vit), side: LARGE_SIDE, engine: LARGE_ENGINE });
            }
            return Self::load_onnx(&large, LARGE_SIDE, LARGE_ENGINE);
        }
        if prefer != Prefer::Cpu {
            #[cfg(feature = "siglip")]
            if let Some(vit) = gpu::Vit::load(dir, SIDE)? {
                return Ok(Nsfw { backend: Backend::Gpu(vit), side: SIDE, engine: ENGINE });
            }
            if prefer == Prefer::Gpu {
                return Err(Error::Model(if dir.join(GPU_WEIGHTS_FILE).is_file() {
                    "sensitive-content model: this machine has no GPU to run the GPU weights on".into()
                } else {
                    "sensitive-content model: the GPU weights are not downloaded".into()
                }));
            }
        }
        Self::load_onnx(dir, SIDE, ENGINE)
    }

    /// The int8 export in `dir` on `rten`, whatever the machine has.
    fn load_onnx(dir: &Path, side: usize, engine: &'static str) -> Result<Nsfw, Error> {
        if !dir.join(MODEL_FILE).is_file() {
            return Err(Error::Model(
                "sensitive-content model: the CPU model is not downloaded (the folder has only the GPU weights, and this machine has no GPU)".into(),
            ));
        }
        let bad = |e: &dyn std::fmt::Display| Error::Model(format!("sensitive-content model: {e}"));
        let bytes = std::fs::read(dir.join(MODEL_FILE)).map_err(|e| bad(&e))?;
        let model = ModelOptions::with_all_ops().load(bytes).map_err(|e| bad(&e))?;
        let input = model.input_ids().first().copied().ok_or_else(|| bad(&"the model has no input"))?;
        let output = model.output_ids().first().copied().ok_or_else(|| bad(&"the model has no output"))?;
        let n = Nsfw { backend: Backend::Onnx { model, input, output }, side, engine };
        // (the first real photo doesn't pay for preparing the weights)
        n.run(&vec![0f32; 3 * side * side])?;
        Ok(n)
    }

    /// The two logits for one NCHW picture.
    fn run(&self, data: &[f32]) -> Result<Vec<f32>, Error> {
        let logits = match &self.backend {
            #[cfg(feature = "siglip")]
            Backend::Gpu(vit) => vit.run(data)?,
            Backend::Onnx { model, input, output } => {
                let value =
                    ValueView::from_shape([1, 3, self.side, self.side], data).map_err(|e| Error::Model(format!("sensitive-content input: {e}")))?;
                let out = model
                    .run(vec![(*input, value.into())], &[*output], crate::run_options())
                    .map_err(|e| Error::Model(format!("sensitive-content model: {e}")))?;
                out.first().and_then(|v| v.as_tensor_view::<f32>()).map(|t| t.to_vec()).unwrap_or_default()
            }
        };
        if logits.len() == 2 { Ok(logits) } else { Err(Error::Model("the sensitive-content model's answer isn't two numbers".into())) }
    }
}

/// The picture the model reads: the photo squashed to `side`×`side` (as the model's processor does), RGB in −1…1, NCHW.
pub(crate) fn input(img: &Rgba8, side: usize) -> Result<Vec<f32>, Error> {
    if img.width.checked_mul(img.height).is_none_or(|n| n == 0 || n > MAX_PIXELS || n != img.data.len()) {
        return Err(Error::Invalid("photo size"));
    }
    let rgb: Rgb32f = Image {
        width: img.width,
        height: img.height,
        data: img.data.iter().map(|&[r, g, b, _]| [f32::from(r) / 255.0, f32::from(g) / 255.0, f32::from(b) / 255.0]).collect(),
    };
    let small = resample::resize(&rgb, side, side, Filter::Bilinear);
    let plane = side * side;
    let mut out = vec![0f32; 3 * plane];
    for (i, px) in small.data.iter().enumerate().take(plane) {
        for (c, v) in px.iter().enumerate() {
            if let Some(o) = out.get_mut(c * plane + i) {
                *o = (v - 0.5) / 0.5;
            }
        }
    }
    Ok(out)
}

/// The softmax of the second logit.
fn nsfw_probability(logits: &[f32]) -> f32 {
    let [normal, nsfw] = [logits.first().copied().unwrap_or(0.0), logits.get(1).copied().unwrap_or(0.0)];
    let m = normal.max(nsfw);
    let (a, b) = ((normal - m).exp(), (nsfw - m).exp());
    if a + b > 0.0 { b / (a + b) } else { 0.0 }
}

impl Classifier for Nsfw {
    fn engine(&self) -> &str {
        self.engine
    }
    fn backend(&self) -> &'static str {
        match self.backend {
            Backend::Onnx { .. } => "cpu",
            #[cfg(feature = "siglip")]
            Backend::Gpu(_) => "gpu",
        }
    }
    fn score(&self, img: &Rgba8) -> Result<f32, Error> {
        Ok(nsfw_probability(&self.run(&input(img, self.side)?)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_score_is_the_second_logits_softmax() {
        assert!((nsfw_probability(&[0.0, 0.0]) - 0.5).abs() < 1e-6);
        assert!(nsfw_probability(&[5.0, -5.0]) < 0.01);
        assert!(nsfw_probability(&[-5.0, 5.0]) > 0.99);
        assert!(nsfw_probability(&[1e30, -1e30]).is_finite());
    }

    #[test]
    fn a_chosen_backend_that_cannot_run_is_an_error_not_a_quiet_fallback() {
        assert_eq!(
            (Prefer::from_name("auto"), Prefer::from_name("cpu"), Prefer::from_name("gpu"), Prefer::from_name("x")),
            (Some(Prefer::Auto), Some(Prefer::Cpu), Some(Prefer::Gpu), None)
        );
        let dir = std::env::temp_dir().join(format!("lc-nsfw-prefer-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(MODEL_FILE), b"not a model").unwrap();
        // only the CPU file is here: asking for the GPU says so (Automatic would go on to the CPU)
        let e = Nsfw::load_with(&dir, Prefer::Gpu).err().map(|e| e.to_string()).unwrap_or_default();
        assert!(e.contains("GPU weights are not downloaded"), "{e}");
        assert!(Nsfw::load_with(&dir, Prefer::Auto).is_err(), "the file isn't a model");
        // an empty folder is missing, whatever is chosen
        let empty = dir.join("none");
        assert!(matches!(Nsfw::load_with(&empty, Prefer::Cpu), Err(Error::Missing(_))));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_large_model_replaces_the_compact_one_on_auto_only() {
        let dir = std::env::temp_dir().join(format!("lc-nsfw-large-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(LARGE_DIR)).unwrap();
        assert!(!is_model_dir(&dir) && !large_installed(&dir));
        assert_eq!(engine_for(&dir, Prefer::Auto), ENGINE);
        std::fs::write(dir.join(LARGE_DIR).join(MODEL_FILE), b"not a model").unwrap();
        assert!(is_model_dir(&dir) && large_installed(&dir));
        assert_eq!((engine_for(&dir, Prefer::Auto), engine_for(&dir, Prefer::Cpu), engine_for(&dir, Prefer::Gpu)), (LARGE_ENGINE, ENGINE, ENGINE));
        // Auto tries the large file (which isn't a model here); the CPU choice wants the compact one, which isn't here
        assert!(Nsfw::load_with(&dir, Prefer::Auto).err().is_some_and(|e| e.to_string().contains("sensitive-content model")));
        assert!(Nsfw::load_with(&dir, Prefer::Cpu).err().is_some_and(|e| e.to_string().contains("CPU model is not downloaded")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_input_is_224_square_in_minus_one_to_one() {
        let img = Rgba8::from_fn(40, 30, |_, _| [255, 0, 128, 255]);
        let v = input(&img, SIDE).unwrap();
        assert_eq!(v.len(), 3 * SIDE * SIDE);
        let plane = SIDE * SIDE;
        assert!((v[0] - 1.0).abs() < 1e-4 && (v[plane] + 1.0).abs() < 1e-4, "{} {}", v[0], v[plane]);
        assert!(input(&Rgba8::from_fn(0, 0, |_, _| [0; 4]), SIDE).is_err());
        assert_eq!(input(&img, LARGE_SIDE).unwrap().len(), 3 * LARGE_SIDE * LARGE_SIDE);
    }

    /// The two ways to run the real model agree: `LIGHTCRAFT_NSFW` = a folder with both files. The GPU in half
    /// precision and the int8 export differ by a few hundredths at most; also prints how fast each goes.
    #[cfg(feature = "siglip")]
    #[test]
    fn the_gpu_and_the_int8_model_give_the_same_scores() {
        let Some(dir) = std::env::var_os("LIGHTCRAFT_NSFW").map(std::path::PathBuf::from) else {
            lightcraft_testkit::skip!(Model, "LIGHTCRAFT_NSFW not set (the folder with Falconsai's model_int8.onnx and model.safetensors)");
            return;
        };
        if crate::siglip::best_device().is_cpu() || !dir.join(GPU_WEIGHTS_FILE).is_file() {
            lightcraft_testkit::skip!(Gpu, "no GPU, or no model.safetensors in the folder");
            return;
        }
        let gpu = Nsfw::load(&dir).unwrap();
        assert!(matches!(gpu.backend, Backend::Gpu(_)), "the GPU was not used");
        let cpu = Nsfw::load_onnx(&dir, SIDE, ENGINE).unwrap();
        // (varied pictures: gradients, stripes, noise and skin tones)
        let pics: Vec<Rgba8> = (0..24usize)
            .map(|n| {
                let (w, h) = (160 + n * 4, 120 + n * 3);
                Rgba8::from_fn(w, h, move |x, y| {
                    let r = (x.wrapping_mul(2654435761) ^ y.wrapping_mul(40503) ^ n.wrapping_mul(97)) as u8;
                    match n % 4 {
                        0 => [(x * 255 / w) as u8, (y * 255 / h) as u8, 200, 255],
                        1 => [if (x / 8 + y / 8) % 2 == 0 { 230 } else { 20 }, 120, 60, 255],
                        2 => [r, r.rotate_left(3), r.rotate_left(5), 255],
                        _ => [224 - (y % 30) as u8, 172, 105 + (x % 20) as u8, 255],
                    }
                })
            })
            .collect();
        let time = |n: &Nsfw| {
            let t = std::time::Instant::now();
            let scores: Vec<f32> = pics.iter().map(|p| n.score(p).unwrap()).collect();
            (scores, t.elapsed())
        };
        let ((g, gt), (c, ct)) = (time(&gpu), time(&cpu));
        eprintln!("sensitive-content model on {} pictures: GPU {gt:?}, CPU (int8) {ct:?}", pics.len());
        for (i, (a, b)) in g.iter().zip(&c).enumerate() {
            assert!((a - b).abs() < 0.05, "picture {i}: GPU {a}, int8 {b}");
        }
    }

    /// The real model (`LIGHTCRAFT_NSFW` = the folder holding `model_int8.onnx`): it loads on rten and an
    /// everyday picture isn't flagged. (How well it finds sensitive photos isn't tested here.)
    #[test]
    fn the_real_model_loads_and_passes_an_everyday_picture() {
        let Some(dir) = std::env::var_os("LIGHTCRAFT_NSFW").map(std::path::PathBuf::from) else {
            lightcraft_testkit::skip!(Model, "LIGHTCRAFT_NSFW not set (the folder with Falconsai's model_int8.onnx)");
            return;
        };
        let n = Nsfw::load(&dir).unwrap();
        let sky = Rgba8::from_fn(320, 240, |x, y| [(80 + x / 8) as u8, (140 + y / 6) as u8, 220, 255]);
        let s = n.score(&sky).unwrap();
        assert!(s < 0.5, "a blue gradient scored {s}");
    }

    /// The real large model (`LIGHTCRAFT_NSFW` = a folder holding `large/model_int8.onnx`): `Auto` picks it, it runs
    /// on rten at 384 px and an everyday picture isn't flagged; prints how long a photo takes.
    #[test]
    fn the_real_large_model_loads_and_passes_an_everyday_picture() {
        let Some(dir) = std::env::var_os("LIGHTCRAFT_NSFW").map(std::path::PathBuf::from).filter(|d| large_installed(d)) else {
            lightcraft_testkit::skip!(Model, "LIGHTCRAFT_NSFW not set to a folder with large/model_int8.onnx");
            return;
        };
        let n = Nsfw::load(&dir).unwrap();
        assert_eq!((n.engine(), n.backend()), (LARGE_ENGINE, "cpu"));
        let sky = Rgba8::from_fn(320, 240, |x, y| [(80 + x / 8) as u8, (140 + y / 6) as u8, 220, 255]);
        let t = std::time::Instant::now();
        let s = n.score(&sky).unwrap();
        eprintln!("large sensitive-content model: {:?} per photo", t.elapsed());
        assert!(s < 0.5, "a blue gradient scored {s}");
    }

    /// The large model's two ways to run agree: `LIGHTCRAFT_NSFW` = a folder whose `large/` holds both files.
    #[cfg(feature = "siglip")]
    #[test]
    fn the_large_gpu_and_int8_models_give_the_same_scores() {
        let Some(dir) = std::env::var_os("LIGHTCRAFT_NSFW").map(std::path::PathBuf::from).filter(|d| large_cpu_installed(d) && large_gpu_installed(d)) else {
            lightcraft_testkit::skip!(Model, "LIGHTCRAFT_NSFW not set to a folder with large/model_int8.onnx and large/model.safetensors");
            return;
        };
        if crate::siglip::best_device().is_cpu() {
            lightcraft_testkit::skip!(Gpu, "no GPU");
            return;
        }
        let gpu = Nsfw::load(&dir).unwrap();
        assert_eq!((gpu.engine(), gpu.backend()), (LARGE_ENGINE, "gpu"));
        let cpu = Nsfw::load_onnx(&dir.join(LARGE_DIR), LARGE_SIDE, LARGE_ENGINE).unwrap();
        let pics: Vec<Rgba8> = (0..8usize)
            .map(|n| {
                let (w, h) = (160 + n * 4, 120 + n * 3);
                Rgba8::from_fn(w, h, move |x, y| match n % 4 {
                    0 => [(x * 255 / w) as u8, (y * 255 / h) as u8, 200, 255],
                    1 => [if (x / 8 + y / 8) % 2 == 0 { 230 } else { 20 }, 120, 60, 255],
                    2 => [(x * 7 + n) as u8, (y * 13) as u8, (x ^ y) as u8, 255],
                    _ => [224 - (y % 30) as u8, 172, 105 + (x % 20) as u8, 255],
                })
            })
            .collect();
        let time = |n: &Nsfw| {
            let t = std::time::Instant::now();
            let scores: Vec<f32> = pics.iter().map(|p| n.score(p).unwrap()).collect();
            (scores, t.elapsed())
        };
        let ((g, gt), (c, ct)) = (time(&gpu), time(&cpu));
        eprintln!("large sensitive-content model on {} pictures: GPU {gt:?}, CPU (int8) {ct:?}", pics.len());
        for (i, (a, b)) in g.iter().zip(&c).enumerate() {
            assert!((a - b).abs() < 0.05, "picture {i}: GPU {a}, int8 {b}");
        }
    }
}
