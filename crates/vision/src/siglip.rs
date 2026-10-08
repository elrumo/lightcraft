//! SigLIP 2 B/16 (Google, Apache-2.0): image and text embeddings in one space, so a sentence can
//! be compared with a photo. The weights are the `google/siglip2-base-patch16-224` checkpoint,
//! which LightCraft doesn't ship: the user downloads it and [`SigLip::load`] reads the folder.
//!
//! SigLIP 2's fixed-resolution checkpoints are the original SigLIP architecture, so this runs on
//! candle's `siglip` module with the one config difference, the 256k-entry Gemma vocabulary.
//! Preprocessing follows the checkpoint's own configuration: the image is squashed to 224 × 224
//! with an antialiased bilinear filter and mapped to [-1, 1]; the text is lower-cased, ends in
//! `<eos>` (no `<bos>`), and is right-padded with `<pad>` to exactly 64 tokens, because the
//! text tower reads its result from the last position.
//!
//! Outputs are raw (not unit length); the [`crate::EmbeddingIndex`] normalises what it stores.

use std::path::Path;

use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::siglip;
use lightcraft_raster::resample::{self, Filter};
use lightcraft_raster::{Image, Rgb32f, Rgba8};
use tokenizers::Tokenizer;

use crate::Error;
use crate::weights::{Backend, Weights};

/// Names the index and the model folder (at most 16 bytes, see [`crate::store::MODEL_LEN`]).
pub const MODEL_ID: &str = "siglip2-b16-224";
/// Length of an embedding.
pub const DIM: usize = 768;
pub const WEIGHTS_FILE: &str = "model.safetensors";
pub const TOKENIZER_FILE: &str = "tokenizer.json";

/// Side of the square model input.
const SIDE: usize = 224;
/// Tokens the text tower reads (the checkpoint's position table).
const CONTEXT: usize = 64;
const PAD: u32 = 0;
const EOS: u32 = 1;
/// Longest query used, in characters (a hostile string can't make the tokenizer run long).
const MAX_QUERY_CHARS: usize = 512;
/// Largest image accepted, in pixels: callers pass thumbnails, not originals.
const MAX_PIXELS: usize = 64 << 20;

/// The best device here: Metal on macOS (when a GPU is available), else the CPU.
pub fn best_device() -> Device {
    #[cfg(target_os = "macos")]
    if let Ok(d) = Device::new_metal(0) {
        return d;
    }
    Device::Cpu
}

/// Whether `dir` holds the files [`SigLip::load`] reads.
pub fn is_model_dir(dir: &Path) -> bool {
    dir.join(WEIGHTS_FILE).is_file() && dir.join(TOKENIZER_FILE).is_file()
}

/// A loaded model.
pub struct SigLip {
    model: siglip::Model,
    tokenizer: Tokenizer,
    device: Device,
}

impl SigLip {
    /// Loads the model from `dir` onto the best device.
    pub fn load(dir: &Path) -> Result<SigLip, Error> {
        Self::load_on(dir, best_device())
    }

    pub fn load_on(dir: &Path, device: Device) -> Result<SigLip, Error> {
        if !is_model_dir(dir) {
            return Err(Error::Missing(dir.to_path_buf()));
        }
        let weights = Weights::open(&dir.join(WEIGHTS_FILE))?;
        let vb = VarBuilder::from_backend(Box::new(Backend(weights)), DType::F32, device.clone());
        let mut cfg = siglip::Config::base_patch16_224();
        cfg.text_config.vocab_size = 256_000;
        let model = siglip::Model::new(&cfg, vb)?;
        let tokenizer = Tokenizer::from_file(dir.join(TOKENIZER_FILE)).map_err(|e| Error::Model(format!("{TOKENIZER_FILE}: {e}")))?;
        let model = SigLip { model, tokenizer, device };
        // On Metal the first text pass compiles the tower's kernels (~12 s, measured); do it here,
        // where loading already runs on a worker, so the first search is as fast as the rest.
        model.encode_text("")?;
        Ok(model)
    }

    pub fn device(&self) -> &Device {
        &self.device
    }

    /// The tokens the text tower reads for `text`: lower-cased, `<eos>`-terminated, padded or
    /// cut to exactly [`CONTEXT`].
    fn token_ids(&self, text: &str) -> Result<Vec<u32>, Error> {
        let text: String = text.chars().take(MAX_QUERY_CHARS).collect::<String>().to_lowercase();
        let enc = self.tokenizer.encode(text, true).map_err(|e| Error::Model(format!("tokenizer: {e}")))?;
        // the tokenizer file pads to 64 itself; start from the real tokens either way
        let mut ids: Vec<u32> = enc.get_ids().iter().copied().take_while(|&i| i != PAD).collect();
        if ids.len() > CONTEXT {
            ids.truncate(CONTEXT - 1);
            ids.push(EOS);
        }
        ids.resize(CONTEXT, PAD);
        Ok(ids)
    }

    /// The embedding of a sentence.
    pub fn encode_text(&self, text: &str) -> Result<Vec<f32>, Error> {
        let ids = self.token_ids(text)?;
        let input = Tensor::new(ids.as_slice(), &self.device)?.unsqueeze(0)?;
        let out = self.model.get_text_features(&input)?;
        let v = out.flatten_all()?.to_vec1::<f32>()?;
        if v.len() != DIM {
            return Err(Error::Model(format!("text embedding of {} values, expected {DIM}", v.len())));
        }
        Ok(v)
    }

    /// The embedding of a photo (any size; a thumbnail of a few hundred pixels is plenty).
    pub fn encode_image(&self, img: &Rgba8) -> Result<Vec<f32>, Error> {
        self.encode_images(&[img])?.pop().ok_or_else(|| Error::Model("no image embedding".into()))
    }

    /// The embeddings of several photos in one pass (faster than one by one on a GPU).
    pub fn encode_images(&self, imgs: &[&Rgba8]) -> Result<Vec<Vec<f32>>, Error> {
        if imgs.is_empty() {
            return Ok(Vec::new());
        }
        let mut all = Vec::with_capacity(imgs.len() * 3 * SIDE * SIDE);
        for img in imgs {
            all.extend_from_slice(&preprocess(img)?);
        }
        let x = Tensor::from_vec(all, (imgs.len(), 3, SIDE, SIDE), &self.device)?;
        let rows = self.model.get_image_features(&x)?.to_vec2::<f32>()?;
        if rows.len() != imgs.len() || rows.iter().any(|r| r.len() != DIM) {
            return Err(Error::Model("image embeddings of the wrong shape".into()));
        }
        Ok(rows)
    }
}

/// The model's input for a photo: planar RGB, 224 × 224, values in [-1, 1].
pub fn preprocess(img: &Rgba8) -> Result<Vec<f32>, Error> {
    let (w, h) = (img.width, img.height);
    let pixels = w.checked_mul(h).filter(|&n| n > 0 && n <= MAX_PIXELS && n == img.data.len());
    if pixels.is_none() {
        return Err(Error::Invalid("image size"));
    }
    let rgb: Rgb32f = Image {
        width: w,
        height: h,
        data: img.data.iter().map(|&[r, g, b, _]| [f32::from(r) / 255.0, f32::from(g) / 255.0, f32::from(b) / 255.0]).collect(),
    };
    let small = resample::resize(&rgb, SIDE, SIDE, Filter::Bilinear);
    let plane = SIDE * SIDE;
    let mut out = vec![0f32; 3 * plane];
    for (i, px) in small.data.iter().enumerate() {
        for (c, v) in px.iter().enumerate() {
            if let Some(o) = out.get_mut(c * plane + i) {
                *o = v * 2.0 - 1.0;
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: usize, h: usize, px: [u8; 4]) -> Rgba8 {
        Image { width: w, height: h, data: vec![px; w * h] }
    }

    #[test]
    fn a_photo_becomes_planar_values_between_minus_one_and_one() {
        let out = preprocess(&solid(100, 60, [255, 0, 128, 255])).unwrap();
        assert_eq!(out.len(), 3 * SIDE * SIDE);
        let plane = SIDE * SIDE;
        assert!(out[..plane].iter().all(|&v| (v - 1.0).abs() < 1e-5));
        assert!(out[plane..2 * plane].iter().all(|&v| (v + 1.0).abs() < 1e-5));
        assert!(out[2 * plane..].iter().all(|&v| (v - (128.0 / 255.0 * 2.0 - 1.0)).abs() < 1e-4));
    }

    #[test]
    fn a_tiny_or_huge_photo_still_resizes() {
        assert_eq!(preprocess(&solid(1, 1, [9, 9, 9, 255])).unwrap().len(), 3 * SIDE * SIDE);
        assert_eq!(preprocess(&solid(3000, 7, [9, 9, 9, 255])).unwrap().len(), 3 * SIDE * SIDE);
    }

    #[test]
    fn nonsense_images_are_refused() {
        assert!(preprocess(&Image { width: 0, height: 5, data: vec![] }).is_err());
        assert!(preprocess(&Image { width: 5, height: 0, data: vec![] }).is_err());
        // the pixel count doesn't match the buffer
        assert!(preprocess(&Image { width: 4, height: 4, data: vec![[0, 0, 0, 0]; 15] }).is_err());
        // a size whose area overflows or exceeds the cap
        assert!(preprocess(&Image { width: usize::MAX, height: 2, data: vec![] }).is_err());
        assert!(preprocess(&Image { width: 1 << 20, height: 1 << 20, data: vec![] }).is_err());
    }

    #[test]
    fn a_missing_model_is_reported_not_panicked_on() {
        let dir = std::env::temp_dir().join(format!("lc-vision-nomodel-{}", std::process::id()));
        assert!(!is_model_dir(&dir));
        assert!(matches!(SigLip::load_on(&dir, Device::Cpu), Err(Error::Missing(_))));
    }
}
