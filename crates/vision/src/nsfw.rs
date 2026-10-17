//! Falconsai's NSFW image classifier (a ViT-B/16, Apache-2.0, `Falconsai/nsfw_image_detection`) on
//! `rten`, a pure-Rust ONNX runtime: our own code reading its published conventions. It takes a 224×224
//! RGB picture scaled to −1…1 (mean and standard deviation 0.5) and answers two logits, `normal` and
//! `nsfw`; the score is the second one's softmax.

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
/// The input's side, in pixels.
const SIDE: usize = 224;
/// Largest photo accepted, in pixels.
const MAX_PIXELS: usize = 64 << 20;

/// The classifier, loaded.
pub struct Nsfw {
    model: Model,
    input: NodeId,
    output: NodeId,
}

/// Whether `dir` holds the file [`Nsfw::load`] reads.
pub fn is_model_dir(dir: &Path) -> bool {
    dir.join(MODEL_FILE).is_file()
}

impl Nsfw {
    pub fn load(dir: &Path) -> Result<Nsfw, Error> {
        if !is_model_dir(dir) {
            return Err(Error::Missing(dir.to_path_buf()));
        }
        let bad = |e: &dyn std::fmt::Display| Error::Model(format!("sensitive-content model: {e}"));
        let bytes = std::fs::read(dir.join(MODEL_FILE)).map_err(|e| bad(&e))?;
        let model = ModelOptions::with_all_ops().load(bytes).map_err(|e| bad(&e))?;
        let input = model.input_ids().first().copied().ok_or_else(|| bad(&"the model has no input"))?;
        let output = model.output_ids().first().copied().ok_or_else(|| bad(&"the model has no output"))?;
        let n = Nsfw { model, input, output };
        // (the first real photo doesn't pay for preparing the weights)
        n.run(&vec![0f32; 3 * SIDE * SIDE])?;
        Ok(n)
    }

    /// The two logits for one NCHW picture.
    fn run(&self, data: &[f32]) -> Result<Vec<f32>, Error> {
        let input = ValueView::from_shape([1, 3, SIDE, SIDE], data).map_err(|e| Error::Model(format!("sensitive-content input: {e}")))?;
        let out = self
            .model
            .run(vec![(self.input, input.into())], &[self.output], crate::run_options())
            .map_err(|e| Error::Model(format!("sensitive-content model: {e}")))?;
        let logits = out.first().and_then(|v| v.as_tensor_view::<f32>()).map(|t| t.to_vec());
        logits.filter(|l| l.len() == 2).ok_or_else(|| Error::Model("the sensitive-content model's answer isn't two numbers".into()))
    }
}

/// The picture the model reads: the photo squashed to 224×224 (as the model's processor does), RGB in −1…1, NCHW.
pub(crate) fn input(img: &Rgba8) -> Result<Vec<f32>, Error> {
    if img.width.checked_mul(img.height).is_none_or(|n| n == 0 || n > MAX_PIXELS || n != img.data.len()) {
        return Err(Error::Invalid("photo size"));
    }
    let rgb: Rgb32f = Image {
        width: img.width,
        height: img.height,
        data: img.data.iter().map(|&[r, g, b, _]| [f32::from(r) / 255.0, f32::from(g) / 255.0, f32::from(b) / 255.0]).collect(),
    };
    let small = resample::resize(&rgb, SIDE, SIDE, Filter::Bilinear);
    let plane = SIDE * SIDE;
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
        ENGINE
    }
    fn score(&self, img: &Rgba8) -> Result<f32, Error> {
        Ok(nsfw_probability(&self.run(&input(img)?)?))
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
    fn the_input_is_224_square_in_minus_one_to_one() {
        let img = Rgba8::from_fn(40, 30, |_, _| [255, 0, 128, 255]);
        let v = input(&img).unwrap();
        assert_eq!(v.len(), 3 * SIDE * SIDE);
        let plane = SIDE * SIDE;
        assert!((v[0] - 1.0).abs() < 1e-4 && (v[plane] + 1.0).abs() < 1e-4, "{} {}", v[0], v[plane]);
        assert!(input(&Rgba8::from_fn(0, 0, |_, _| [0; 4])).is_err());
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
}
