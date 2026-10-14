//! CLIP's transformers (Radford et al., 2021): the text encoders of Stable Diffusion and SDXL, and
//! the image encoder Kandinsky 2.2 is conditioned with.
//!
//! Each runs a layer at a time: a layer's weights are read from the file, used, and dropped
//! ([`crate::weights::Weights::var_builder_of`]), so even SDXL's 1.3 GB text encoder or
//! Kandinsky's 1.8-billion-parameter image encoder needs the memory of one layer, not of the
//! whole model. They run once per Generate (no prompt from the user: their answers depend only on
//! the model), so reading them again costs little.

use candle_core::{DType, IndexOp, Module, Tensor};
use candle_nn as nn;

use crate::weights::Weights;
use crate::{Error, Result, Target};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    /// `x · σ(1.702 x)` (OpenAI's CLIP).
    QuickGelu,
    /// The exact (erf) GELU (OpenCLIP).
    Gelu,
}

/// A CLIP transformer's shape.
#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub dim: usize,
    pub heads: usize,
    pub layers: usize,
    pub mlp: usize,
    pub act: Act,
}

/// CLIP ViT-L/14's text encoder (Stable Diffusion 1.5; SDXL's first).
pub const TEXT_L: Config = Config { dim: 768, heads: 12, layers: 12, mlp: 3072, act: Act::QuickGelu };
/// OpenCLIP ViT-bigG/14's text encoder (SDXL's second).
pub const TEXT_BIG_G: Config = Config { dim: 1280, heads: 20, layers: 32, mlp: 5120, act: Act::Gelu };
/// OpenCLIP ViT-bigG/14's image encoder (Kandinsky 2.2).
pub const VISION_BIG_G: Config = Config { dim: 1664, heads: 16, layers: 48, mlp: 8192, act: Act::Gelu };

const EPS: f64 = 1e-5;

/// One encoder layer: attention, then the MLP, each after a layer norm and added back.
fn layer(vb: nn::VarBuilder, c: &Config, x: &Tensor, mask: Option<&Tensor>) -> Result<Tensor> {
    let d = c.dim;
    let lin = |n: &str, i: usize, o: usize| nn::linear(i, o, vb.pp(n));
    let (q, k, v, o) =
        (lin("self_attn.q_proj", d, d)?, lin("self_attn.k_proj", d, d)?, lin("self_attn.v_proj", d, d)?, lin("self_attn.out_proj", d, d)?);
    let (ln1, ln2) = (nn::layer_norm(d, EPS, vb.pp("layer_norm1"))?, nn::layer_norm(d, EPS, vb.pp("layer_norm2"))?);
    let (fc1, fc2) = (lin("mlp.fc1", d, c.mlp)?, lin("mlp.fc2", c.mlp, d)?);
    let (b, t, _) = x.dims3()?;
    let hd = d / c.heads.max(1);
    let h = ln1.forward(x)?;
    let split = |y: Tensor| -> Result<Tensor> { Ok(y.reshape((b, t, c.heads, hd))?.transpose(1, 2)?.contiguous()?) };
    let (qs, ks, vs) = (split((q.forward(&h)? * (hd as f64).powf(-0.5))?)?, split(k.forward(&h)?)?, split(v.forward(&h)?)?);
    let mut w = qs.matmul(&ks.t()?)?;
    if let Some(m) = mask {
        w = w.broadcast_add(m)?;
    }
    let w = nn::ops::softmax_last_dim(&w.to_dtype(DType::F32)?)?.to_dtype(x.dtype())?;
    let a = w.matmul(&vs)?.transpose(1, 2)?.reshape((b, t, d))?;
    let x = (x + o.forward(&a)?)?;
    let h = fc1.forward(&ln2.forward(&x)?)?;
    let h = match c.act {
        Act::QuickGelu => (&h * nn::ops::sigmoid(&(&h * 1.702)?)?)?,
        Act::Gelu => h.gelu_erf()?,
    };
    Ok((x + fc2.forward(&h)?)?)
}

/// What a text encoder gives.
pub struct Text {
    /// The last layer's output after the final layer norm, `(1, 77, dim)`.
    pub last: Option<Tensor>,
    /// The output of the last layer but one, `(1, 77, dim)` (what SDXL reads).
    pub penultimate: Tensor,
}

/// Run text encoder `w` (shape `c`) on 77 token `ids`. With `last = false` the last layer and the
/// final norm are skipped.
pub fn text(w: &Weights, c: &Config, ids: &[u32], target: &Target, last: bool) -> Result<Text> {
    let (dev, dt) = (&target.device, target.dtype);
    let t = ids.len();
    let emb = w.var_builder_of("text_model.embeddings.", dt, dev)?;
    let tok = nn::embedding(49408, c.dim, emb.pp("token_embedding"))?;
    let pos = emb.get((77, c.dim), "position_embedding.weight")?;
    let ids_t = Tensor::new(ids, dev)?.unsqueeze(0)?;
    let mut x = tok.forward(&ids_t)?.broadcast_add(&pos.i(..t)?.unsqueeze(0)?)?;
    drop(emb);
    // causal: a token sees itself and those before it
    let m: Vec<f32> = (0..t).flat_map(|i| (0..t).map(move |j| if j > i { f32::NEG_INFINITY } else { 0.0 })).collect();
    let mask = Tensor::from_vec(m, (1, 1, t, t), dev)?.to_dtype(dt)?;
    let layers = if last { c.layers } else { c.layers.saturating_sub(1) };
    let mut penultimate = None;
    for i in 0..layers {
        let vb = w.var_builder_of(&format!("text_model.encoder.layers.{i}."), dt, dev)?;
        x = layer(vb, c, &x, Some(&mask))?;
        if i + 2 == c.layers {
            penultimate = Some(x.clone());
        }
    }
    let penultimate = penultimate.ok_or_else(|| Error::Model("the text encoder is too short".into()))?;
    let last = if last {
        let ln = nn::layer_norm(c.dim, EPS, w.var_builder_of("text_model.final_layer_norm.", dt, dev)?)?;
        Some(ln.forward(&x)?)
    } else {
        None
    };
    Ok(Text { last, penultimate })
}

/// SDXL's pooled text embedding: the last layer's output at the end token, through the text
/// projection (`(1, dim)`).
pub fn pooled(w: &Weights, c: &Config, last: &Tensor, end: usize, target: &Target) -> Result<Tensor> {
    let proj = w.var_builder_of("text_projection.", target.dtype, &target.device)?.get((c.dim, c.dim), "weight")?;
    Ok(last.i((.., end, ..))?.matmul(&proj.t()?)?)
}

/// CLIP's image size and normalisation.
pub const IMAGE: usize = 224;
pub const MEAN: [f32; 3] = [0.481_454_66, 0.457_827_5, 0.408_210_73];
pub const STD: [f32; 3] = [0.268_629_54, 0.261_302_58, 0.275_777_11];

/// The image embedding (`(1, projection)`) of `pixels` (`(1, 3, 224, 224)`, normalised), by
/// image encoder `w`; `cancelled` is asked between layers.
pub fn image(w: &Weights, c: &Config, pixels: &Tensor, target: &Target, cancelled: &dyn Fn() -> bool) -> Result<Tensor> {
    let (dev, dt) = (&target.device, target.dtype);
    let emb = w.var_builder_of("vision_model.embeddings.", dt, dev)?;
    let patch = emb.get((c.dim, 3, 14, 14), "patch_embedding.weight")?;
    let class = emb.get(c.dim, "class_embedding")?;
    let n = (IMAGE / 14) * (IMAGE / 14) + 1;
    let pos = emb.get((n, c.dim), "position_embedding.weight")?;
    drop(emb);
    let p = pixels.to_dtype(dt)?.conv2d(&patch, 0, 14, 1, 1)?; // (1, dim, 16, 16)
    let p = p.flatten_from(2)?.transpose(1, 2)?; // (1, 256, dim)
    let x = Tensor::cat(&[&class.reshape((1, 1, c.dim))?, &p], 1)?.broadcast_add(&pos.unsqueeze(0)?)?;
    let pre = nn::layer_norm(c.dim, EPS, w.var_builder_of("vision_model.pre_layrnorm.", dt, dev)?)?;
    let mut x = pre.forward(&x)?;
    for i in 0..c.layers {
        if cancelled() {
            return Err(Error::Cancelled);
        }
        let vb = w.var_builder_of(&format!("vision_model.encoder.layers.{i}."), dt, dev)?;
        x = layer(vb, c, &x, None)?;
    }
    let post = nn::layer_norm(c.dim, EPS, w.var_builder_of("vision_model.post_layernorm.", dt, dev)?)?;
    let pooled = post.forward(&x.i((.., 0, ..))?)?;
    let proj = w.var_builder_of("visual_projection.", dt, dev)?;
    let shape = w.shape("visual_projection.weight").ok_or_else(|| Error::Model("the image encoder has no projection".into()))?;
    let out = shape.first().copied().unwrap_or(0);
    let proj = proj.get((out, c.dim), "weight")?;
    Ok(pooled.matmul(&proj.t()?)?)
}
