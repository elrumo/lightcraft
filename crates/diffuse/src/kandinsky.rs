//! **Kandinsky 2.2 Inpaint** (Shakhmatov, Razzhigaev, Nikolich, Arkhipkin, Pavlov, Kuznetsov and
//! Dimitrov, Sber AI / AIRI, 2023; Apache-2.0), following the diffusers implementation
//! (`UNet2DConditionModel` with image conditioning, `VQModel` with spatial norms, the
//! `KandinskyV22InpaintPipeline`; Apache-2.0), re-written in Rust on candle.
//!
//! Kandinsky's decoder is conditioned on a CLIP *image* embedding, not on text. LightCraft gives
//! it the embedding of the photo itself (OpenCLIP ViT-bigG/14, from Kandinsky's prior), so the new
//! area is drawn as "more of this picture", and the embedding of an empty picture as the negative
//! for guidance, as Kandinsky's prior does. No prompt is asked of the user. The latents come from
//! MoVQ (a VQ-GAN whose decoder is normalised by the latents themselves), and after every step the
//! photo's own latents, noised to the step's level, replace the network's over the photo.
//!
//! The UNet is 1.2 billion parameters, the image encoder 1.8 billion: on a computer only, about
//! 9 GB to download. The image encoder runs a layer at a time ([`crate::clip`]).

use std::path::Path;

use candle_core::{D, DType, Module, Tensor};
use candle_nn as nn;
use lightcraft_erase::expand::{Monitor, frame_for};
use lightcraft_erase::{Frame, Painter};
use lightcraft_raster::Rgb32f;
use lightcraft_raster::resample::{Filter, resize};

use crate::models::{IMAGE_ENCODER, KANDINSKY, UNET_FP32};
use crate::noise::gaussian;
use crate::schedule::{Betas, Dpm};
use crate::sd::pinned;
use crate::weights::Weights;
use crate::{Error, Result, Target, clip};

const GROUPS: usize = 32;

fn conv(i: usize, o: usize, k: usize, pad: usize, vb: nn::VarBuilder) -> Result<nn::Conv2d> {
    Ok(nn::conv2d(i, o, k, nn::Conv2dConfig { padding: pad, ..Default::default() }, vb)?)
}

fn silu(x: &Tensor) -> Result<Tensor> {
    Ok(nn::ops::silu(x)?)
}

/// Attention over `q` (`(b, n, c)`) and `k`, `v` (`(b, m, c)`) with `heads` heads, the softmax
/// in 32-bit.
fn attend(q: &Tensor, k: &Tensor, v: &Tensor, heads: usize) -> Result<Tensor> {
    let (b, n, c) = q.dims3()?;
    let m = k.dim(1)?;
    let hd = c / heads.max(1);
    let split = |t: &Tensor, len: usize| -> Result<Tensor> { Ok(t.reshape((b, len, heads, hd))?.transpose(1, 2)?.contiguous()?) };
    let (q, k, v) = (split(q, n)?, split(k, m)?, split(v, m)?);
    let w = (q.matmul(&k.t()?)? * (hd as f64).powf(-0.5))?;
    let w = nn::ops::softmax_last_dim(&w.to_dtype(DType::F32)?)?.to_dtype(v.dtype())?;
    Ok(w.matmul(&v)?.transpose(1, 2)?.reshape((b, n, c))?)
}

/// `(b, c, h, w)` to `(b, h·w, c)` and back. (Contiguous: candle 0.9.2's `Linear` answers
/// wrongly for a transposed three-dimensional input; found against diffusers, 2026-10-14.)
fn tokens(x: &Tensor) -> Result<Tensor> {
    Ok(x.flatten_from(2)?.transpose(1, 2)?.contiguous()?)
}

fn untokens(x: &Tensor, (b, c, h, w): (usize, usize, usize, usize)) -> Result<Tensor> {
    Ok(x.transpose(1, 2)?.reshape((b, c, h, w))?)
}

// ---------------------------------------------------------------------------------------------
// the UNet

/// A resnet block with "scale-shift" time conditioning, optionally sampling up or down first.
struct ResBlock {
    norm1: nn::GroupNorm,
    conv1: nn::Conv2d,
    temb: nn::Linear,
    norm2: nn::GroupNorm,
    conv2: nn::Conv2d,
    shortcut: Option<nn::Conv2d>,
    resample: Resample,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Resample {
    None,
    Up,
    Down,
}

impl ResBlock {
    fn new(vb: nn::VarBuilder, i: usize, o: usize, temb: usize, resample: Resample) -> Result<ResBlock> {
        Ok(ResBlock {
            norm1: nn::group_norm(GROUPS, i, 1e-5, vb.pp("norm1"))?,
            conv1: conv(i, o, 3, 1, vb.pp("conv1"))?,
            temb: nn::linear(temb, 2 * o, vb.pp("time_emb_proj"))?,
            norm2: nn::group_norm(GROUPS, o, 1e-5, vb.pp("norm2"))?,
            conv2: conv(o, o, 3, 1, vb.pp("conv2"))?,
            shortcut: if i != o { Some(conv(i, o, 1, 0, vb.pp("conv_shortcut"))?) } else { None },
            resample,
        })
    }

    fn forward(&self, x: &Tensor, temb: &Tensor) -> Result<Tensor> {
        let mut input = x.clone();
        let mut h = silu(&self.norm1.forward(x)?)?;
        match self.resample {
            Resample::Up => {
                let (_, _, hh, ww) = h.dims4()?;
                h = h.upsample_nearest2d(hh * 2, ww * 2)?;
                input = input.upsample_nearest2d(hh * 2, ww * 2)?;
            }
            Resample::Down => {
                h = h.avg_pool2d(2)?;
                input = input.avg_pool2d(2)?;
            }
            Resample::None => {}
        }
        let h = self.conv1.forward(&h)?;
        let t = self.temb.forward(&silu(temb)?)?.unsqueeze(D::Minus1)?.unsqueeze(D::Minus1)?;
        let c = t.dim(1)? / 2;
        let (scale, shift) = (t.narrow(1, 0, c)?, t.narrow(1, c, c)?);
        let h = self.norm2.forward(&h)?.broadcast_mul(&(scale + 1.0)?)?.broadcast_add(&shift)?;
        let h = self.conv2.forward(&silu(&h)?)?;
        let input = match &self.shortcut {
            Some(s) => s.forward(&input)?,
            None => input,
        };
        Ok((input + h)?)
    }
}

/// Attention over the picture's own tokens and the image condition's (projected separately), as
/// diffusers' `AttnAddedKVProcessor`.
struct AddedKvAttention {
    norm: nn::GroupNorm,
    q: nn::Linear,
    k: nn::Linear,
    v: nn::Linear,
    add_k: nn::Linear,
    add_v: nn::Linear,
    out: nn::Linear,
    heads: usize,
}

impl AddedKvAttention {
    fn new(vb: nn::VarBuilder, c: usize, ctx: usize) -> Result<AddedKvAttention> {
        let lin = |n: &str, i: usize| nn::linear(i, c, vb.pp(n));
        Ok(AddedKvAttention {
            norm: nn::group_norm(GROUPS, c, 1e-5, vb.pp("group_norm"))?,
            q: lin("to_q", c)?,
            k: lin("to_k", c)?,
            v: lin("to_v", c)?,
            add_k: lin("add_k_proj", ctx)?,
            add_v: lin("add_v_proj", ctx)?,
            out: lin("to_out.0", c)?,
            heads: c / 64,
        })
    }

    fn forward(&self, x: &Tensor, ctx: &Tensor) -> Result<Tensor> {
        let dims = x.dims4()?;
        let h = tokens(&self.norm.forward(x)?)?;
        let q = self.q.forward(&h)?;
        let k = Tensor::cat(&[&self.add_k.forward(ctx)?, &self.k.forward(&h)?], 1)?;
        let v = Tensor::cat(&[&self.add_v.forward(ctx)?, &self.v.forward(&h)?], 1)?;
        let a = self.out.forward(&attend(&q, &k, &v, self.heads)?)?;
        Ok((untokens(&a, dims)? + x)?)
    }
}

struct DownBlock {
    resnets: Vec<ResBlock>,
    attentions: Vec<AddedKvAttention>,
    down: Option<ResBlock>,
}

struct UpBlock {
    resnets: Vec<ResBlock>,
    attentions: Vec<AddedKvAttention>,
    up: Option<ResBlock>,
}

/// Kandinsky 2.2's decoder UNet (inpainting: nine channels in, eight out, the first four the
/// noise).
pub struct Unet {
    conv_in: nn::Conv2d,
    time_proj: candle_transformers::models::stable_diffusion::embeddings::Timesteps,
    time_embedding: candle_transformers::models::stable_diffusion::embeddings::TimestepEmbedding,
    add_proj: nn::Linear,
    add_norm: nn::LayerNorm,
    hid_proj: nn::Linear,
    hid_norm: nn::LayerNorm,
    down: Vec<DownBlock>,
    mid: (ResBlock, AddedKvAttention, ResBlock),
    up: Vec<UpBlock>,
    norm_out: nn::GroupNorm,
    conv_out: nn::Conv2d,
}

/// The channels of each level, the resnets per level and the condition's width.
const CHANNELS: [usize; 4] = [384, 768, 1152, 1536];
const LAYERS: usize = 3;
const CTX: usize = 768;
const IMAGE_TOKENS: usize = 32;
const EMBED: usize = 1280;

impl Unet {
    pub fn new(vb: nn::VarBuilder) -> Result<Unet> {
        use candle_transformers::models::stable_diffusion::embeddings::{TimestepEmbedding, Timesteps};
        let c0 = CHANNELS[0];
        let temb = c0 * 4;
        let n = CHANNELS.len();
        let mut down = Vec::with_capacity(n);
        let mut prev = c0;
        for (i, &c) in CHANNELS.iter().enumerate() {
            let vb = vb.pp("down_blocks").pp(i.to_string());
            let resnets = (0..LAYERS)
                .map(|j| ResBlock::new(vb.pp("resnets").pp(j.to_string()), if j == 0 { prev } else { c }, c, temb, Resample::None))
                .collect::<Result<Vec<_>>>()?;
            // the first level has no attention
            let attentions = if i == 0 {
                Vec::new()
            } else {
                (0..LAYERS).map(|j| AddedKvAttention::new(vb.pp("attentions").pp(j.to_string()), c, CTX)).collect::<Result<Vec<_>>>()?
            };
            let down_block = if i + 1 < n { Some(ResBlock::new(vb.pp("downsamplers.0"), c, c, temb, Resample::Down)?) } else { None };
            down.push(DownBlock { resnets, attentions, down: down_block });
            prev = c;
        }
        let last = CHANNELS[n - 1];
        let mvb = vb.pp("mid_block");
        let mid = (
            ResBlock::new(mvb.pp("resnets.0"), last, last, temb, Resample::None)?,
            AddedKvAttention::new(mvb.pp("attentions.0"), last, CTX)?,
            ResBlock::new(mvb.pp("resnets.1"), last, last, temb, Resample::None)?,
        );
        let rev: Vec<usize> = CHANNELS.iter().rev().copied().collect();
        let at = |i: usize| rev.get(i).copied().unwrap_or(c0);
        let mut up = Vec::with_capacity(n);
        for i in 0..n {
            let vb = vb.pp("up_blocks").pp(i.to_string());
            let (out, prev, inc) = (at(i), if i == 0 { at(0) } else { at(i - 1) }, at((i + 1).min(n - 1)));
            let layers = LAYERS + 1;
            let resnets = (0..layers)
                .map(|j| {
                    let skip = if j + 1 == layers { inc } else { out };
                    let input = if j == 0 { prev } else { out };
                    ResBlock::new(vb.pp("resnets").pp(j.to_string()), input + skip, out, temb, Resample::None)
                })
                .collect::<Result<Vec<_>>>()?;
            // the last level (the picture's own size) has no attention
            let attentions = if i + 1 == n {
                Vec::new()
            } else {
                (0..layers).map(|j| AddedKvAttention::new(vb.pp("attentions").pp(j.to_string()), out, CTX)).collect::<Result<Vec<_>>>()?
            };
            let up_block = if i + 1 < n { Some(ResBlock::new(vb.pp("upsamplers.0"), out, out, temb, Resample::Up)?) } else { None };
            up.push(UpBlock { resnets, attentions, up: up_block });
        }
        Ok(Unet {
            conv_in: conv(9, c0, 3, 1, vb.pp("conv_in"))?,
            time_proj: Timesteps::new(c0, true, 0.0),
            time_embedding: TimestepEmbedding::new(vb.pp("time_embedding"), c0, temb)?,
            add_proj: nn::linear(EMBED, temb, vb.pp("add_embedding.image_proj"))?,
            add_norm: nn::layer_norm(temb, 1e-5, vb.pp("add_embedding.image_norm"))?,
            hid_proj: nn::linear(EMBED, IMAGE_TOKENS * CTX, vb.pp("encoder_hid_proj.image_embeds"))?,
            hid_norm: nn::layer_norm(CTX, 1e-5, vb.pp("encoder_hid_proj.norm"))?,
            down,
            mid,
            up,
            norm_out: nn::group_norm(GROUPS, c0, 1e-5, vb.pp("conv_norm_out"))?,
            conv_out: conv(c0, 8, 3, 1, vb.pp("conv_out"))?,
        })
    }

    /// The predicted noise (the answer's first four channels) for `xs` (`(b, 9, h, w)`) at
    /// `timestep`, conditioned on `image_embeds` (`(b, 1280)`).
    pub fn forward(&self, xs: &Tensor, timestep: f64, image_embeds: &Tensor) -> Result<Tensor> {
        let b = xs.dim(0)?;
        let dt = xs.dtype();
        let t = (Tensor::ones(b, dt, xs.device())? * timestep)?;
        let emb = self.time_embedding.forward(&self.time_proj.forward(&t)?)?;
        let ie = image_embeds.to_dtype(dt)?;
        let emb = (emb + self.add_norm.forward(&self.add_proj.forward(&ie)?)?)?;
        let ctx = self.hid_norm.forward(&self.hid_proj.forward(&ie)?.reshape((b, IMAGE_TOKENS, CTX))?)?;
        let mut x = self.conv_in.forward(xs)?;
        let mut res = vec![x.clone()];
        for d in &self.down {
            for (j, r) in d.resnets.iter().enumerate() {
                x = r.forward(&x, &emb)?;
                if let Some(a) = d.attentions.get(j) {
                    x = a.forward(&x, &ctx)?;
                }
                res.push(x.clone());
            }
            if let Some(s) = &d.down {
                x = s.forward(&x, &emb)?;
                res.push(x.clone());
            }
        }
        x = self.mid.0.forward(&x, &emb)?;
        x = self.mid.1.forward(&x, &ctx)?;
        x = self.mid.2.forward(&x, &emb)?;
        for u in &self.up {
            for (j, r) in u.resnets.iter().enumerate() {
                let skip = res.pop().ok_or_else(|| Error::Model("bad Kandinsky UNet shape".into()))?;
                x = r.forward(&Tensor::cat(&[&x, &skip], 1)?, &emb)?;
                if let Some(a) = u.attentions.get(j) {
                    x = a.forward(&x, &ctx)?;
                }
            }
            if let Some(s) = &u.up {
                x = s.forward(&x, &emb)?;
            }
        }
        let x = self.conv_out.forward(&silu(&self.norm_out.forward(&x)?)?)?;
        Ok(x.narrow(1, 0, 4)?)
    }
}

// ---------------------------------------------------------------------------------------------
// MoVQ

/// A group norm, or a spatial norm (the group norm scaled and shifted by the latents) in the
/// decoder.
enum Norm {
    Group(nn::GroupNorm),
    Spatial { norm: nn::GroupNorm, y: nn::Conv2d, b: nn::Conv2d },
}

impl Norm {
    fn new(vb: nn::VarBuilder, c: usize, spatial: bool) -> Result<Norm> {
        Ok(if spatial {
            Norm::Spatial {
                norm: nn::group_norm(GROUPS, c, 1e-6, vb.pp("norm_layer"))?,
                y: conv(4, c, 1, 0, vb.pp("conv_y"))?,
                b: conv(4, c, 1, 0, vb.pp("conv_b"))?,
            }
        } else {
            Norm::Group(nn::group_norm(GROUPS, c, 1e-6, vb)?)
        })
    }

    fn forward(&self, f: &Tensor, zq: Option<&Tensor>) -> Result<Tensor> {
        match (self, zq) {
            (Norm::Group(n), _) => Ok(n.forward(f)?),
            (Norm::Spatial { norm, y, b }, Some(zq)) => {
                let (_, _, h, w) = f.dims4()?;
                let zq = zq.upsample_nearest2d(h, w)?;
                Ok(norm.forward(f)?.mul(&y.forward(&zq)?)?.add(&b.forward(&zq)?)?)
            }
            (Norm::Spatial { .. }, None) => Err(Error::Model("MoVQ's decoder needs its latents".into())),
        }
    }
}

struct VqRes {
    norm1: Norm,
    conv1: nn::Conv2d,
    norm2: Norm,
    conv2: nn::Conv2d,
    shortcut: Option<nn::Conv2d>,
}

impl VqRes {
    fn new(vb: nn::VarBuilder, i: usize, o: usize, spatial: bool) -> Result<VqRes> {
        Ok(VqRes {
            norm1: Norm::new(vb.pp("norm1"), i, spatial)?,
            conv1: conv(i, o, 3, 1, vb.pp("conv1"))?,
            norm2: Norm::new(vb.pp("norm2"), o, spatial)?,
            conv2: conv(o, o, 3, 1, vb.pp("conv2"))?,
            shortcut: if i != o { Some(conv(i, o, 1, 0, vb.pp("conv_shortcut"))?) } else { None },
        })
    }

    fn forward(&self, x: &Tensor, zq: Option<&Tensor>) -> Result<Tensor> {
        let h = self.conv1.forward(&silu(&self.norm1.forward(x, zq)?)?)?;
        let h = self.conv2.forward(&silu(&self.norm2.forward(&h, zq)?)?)?;
        let x = match &self.shortcut {
            Some(s) => s.forward(x)?,
            None => x.clone(),
        };
        Ok((x + h)?)
    }
}

/// One-head self-attention over the picture (MoVQ's).
struct VqAttention {
    norm: Norm,
    q: nn::Linear,
    k: nn::Linear,
    v: nn::Linear,
    out: nn::Linear,
}

impl VqAttention {
    fn new(vb: nn::VarBuilder, c: usize, spatial: bool) -> Result<VqAttention> {
        let lin = |n: &str| nn::linear(c, c, vb.pp(n));
        Ok(VqAttention {
            norm: Norm::new(vb.pp(if spatial { "spatial_norm" } else { "group_norm" }), c, spatial)?,
            q: lin("to_q")?,
            k: lin("to_k")?,
            v: lin("to_v")?,
            out: lin("to_out.0")?,
        })
    }

    fn forward(&self, x: &Tensor, zq: Option<&Tensor>) -> Result<Tensor> {
        let dims = x.dims4()?;
        let h = tokens(&self.norm.forward(x, zq)?)?;
        let a = attend(&self.q.forward(&h)?, &self.k.forward(&h)?, &self.v.forward(&h)?, 1)?;
        Ok((untokens(&self.out.forward(&a)?, dims)? + x)?)
    }
}

/// MoVQ: the encoder (to latents, before quantization) and the decoder (from them).
pub struct Movq {
    enc_in: nn::Conv2d,
    enc_down: Vec<(Vec<VqRes>, Vec<VqAttention>, Option<nn::Conv2d>)>,
    enc_mid: (VqRes, VqAttention, VqRes),
    enc_norm: nn::GroupNorm,
    enc_out: nn::Conv2d,
    quant_conv: nn::Conv2d,
    post_quant_conv: nn::Conv2d,
    dec_in: nn::Conv2d,
    dec_mid: (VqRes, VqAttention, VqRes),
    dec_up: Vec<(Vec<VqRes>, Vec<VqAttention>, Option<nn::Conv2d>)>,
    dec_norm: Norm,
    dec_out: nn::Conv2d,
}

const VQ: [usize; 4] = [128, 256, 256, 512];

impl Movq {
    pub fn new(vb: nn::VarBuilder) -> Result<Movq> {
        let (e, d) = (vb.pp("encoder"), vb.pp("decoder"));
        let n = VQ.len();
        let last = VQ[n - 1];
        let mut enc_down = Vec::with_capacity(n);
        let mut prev = VQ[0];
        for (i, &c) in VQ.iter().enumerate() {
            let b = e.pp("down_blocks").pp(i.to_string());
            let res =
                (0..2).map(|j| VqRes::new(b.pp("resnets").pp(j.to_string()), if j == 0 { prev } else { c }, c, false)).collect::<Result<Vec<_>>>()?;
            let att = if i + 1 == n {
                (0..2).map(|j| VqAttention::new(b.pp("attentions").pp(j.to_string()), c, false)).collect::<Result<Vec<_>>>()?
            } else {
                Vec::new()
            };
            let down = if i + 1 < n {
                Some(nn::conv2d(c, c, 3, nn::Conv2dConfig { stride: 2, ..Default::default() }, b.pp("downsamplers.0.conv"))?)
            } else {
                None
            };
            enc_down.push((res, att, down));
            prev = c;
        }
        let mid = |vb: nn::VarBuilder, spatial: bool| -> Result<(VqRes, VqAttention, VqRes)> {
            Ok((
                VqRes::new(vb.pp("resnets.0"), last, last, spatial)?,
                VqAttention::new(vb.pp("attentions.0"), last, spatial)?,
                VqRes::new(vb.pp("resnets.1"), last, last, spatial)?,
            ))
        };
        let rev: Vec<usize> = VQ.iter().rev().copied().collect();
        let mut dec_up = Vec::with_capacity(n);
        let mut prev = last;
        for (i, &c) in rev.iter().enumerate() {
            let b = d.pp("up_blocks").pp(i.to_string());
            let res =
                (0..3).map(|j| VqRes::new(b.pp("resnets").pp(j.to_string()), if j == 0 { prev } else { c }, c, true)).collect::<Result<Vec<_>>>()?;
            let att = if i == 0 {
                (0..3).map(|j| VqAttention::new(b.pp("attentions").pp(j.to_string()), c, true)).collect::<Result<Vec<_>>>()?
            } else {
                Vec::new()
            };
            let up = if i + 1 < n { Some(conv(c, c, 3, 1, b.pp("upsamplers.0.conv"))?) } else { None };
            dec_up.push((res, att, up));
            prev = c;
        }
        Ok(Movq {
            enc_in: conv(3, VQ[0], 3, 1, e.pp("conv_in"))?,
            enc_down,
            enc_mid: mid(e.pp("mid_block"), false)?,
            enc_norm: nn::group_norm(GROUPS, last, 1e-6, e.pp("conv_norm_out"))?,
            enc_out: conv(last, 4, 3, 1, e.pp("conv_out"))?,
            quant_conv: conv(4, 4, 1, 0, vb.pp("quant_conv"))?,
            post_quant_conv: conv(4, 4, 1, 0, vb.pp("post_quant_conv"))?,
            dec_in: conv(4, last, 3, 1, d.pp("conv_in"))?,
            dec_mid: mid(d.pp("mid_block"), true)?,
            dec_up,
            dec_norm: Norm::new(d.pp("conv_norm_out"), VQ[0], true)?,
            dec_out: conv(VQ[0], 3, 3, 1, d.pp("conv_out"))?,
        })
    }

    /// The latents of `x` (`(1, 3, h, w)` in -1..=1), `(1, 4, h/8, w/8)`.
    pub fn encode(&self, x: &Tensor) -> Result<Tensor> {
        let mut x = self.enc_in.forward(x)?;
        for (res, att, down) in &self.enc_down {
            for (j, r) in res.iter().enumerate() {
                x = r.forward(&x, None)?;
                if let Some(a) = att.get(j) {
                    x = a.forward(&x, None)?;
                }
            }
            if let Some(dn) = down {
                // (diffusers pads right and bottom by one before a stride-2 convolution)
                x = dn.forward(&x.pad_with_zeros(D::Minus1, 0, 1)?.pad_with_zeros(D::Minus2, 0, 1)?)?;
            }
        }
        let (r0, a, r1) = &self.enc_mid;
        x = r1.forward(&a.forward(&r0.forward(&x, None)?, None)?, None)?;
        let x = self.enc_out.forward(&silu(&self.enc_norm.forward(&x)?)?)?;
        Ok(self.quant_conv.forward(&x)?)
    }

    /// The picture (`(1, 3, 8h, 8w)` in about -1..=1) of latents `z`, unquantized.
    pub fn decode(&self, z: &Tensor) -> Result<Tensor> {
        let zq = Some(z);
        let mut x = self.dec_in.forward(&self.post_quant_conv.forward(z)?)?;
        let (r0, a, r1) = &self.dec_mid;
        x = r1.forward(&a.forward(&r0.forward(&x, zq)?, zq)?, zq)?;
        for (res, att, up) in &self.dec_up {
            for (j, r) in res.iter().enumerate() {
                x = r.forward(&x, zq)?;
                if let Some(a) = att.get(j) {
                    x = a.forward(&x, zq)?;
                }
            }
            if let Some(u) = up {
                let (_, _, h, w) = x.dims4()?;
                x = u.forward(&x.upsample_nearest2d(h * 2, w * 2)?)?;
            }
        }
        Ok(self.dec_out.forward(&silu(&self.dec_norm.forward(&x, zq)?)?)?)
    }
}

// ---------------------------------------------------------------------------------------------
// the pipeline

/// How Kandinsky is asked to paint.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Recipe {
    pub steps: usize,
    pub guidance: f64,
    pub seed: u64,
}

impl Recipe {
    /// What Generative Expand uses: 20 steps on a GPU, 15 on a CPU.
    pub const fn new(gpu: bool) -> Recipe {
        Recipe { steps: if gpu { 20 } else { 15 }, guidance: 4.0, seed: 0x4b41_4e44_494e_534b }
    }
}

/// Kandinsky 2.2 Inpaint, loaded. The image encoder isn't held: it is read a layer at a time when
/// a picture is embedded.
pub struct Kandinsky {
    target: Target,
    unet: Unet,
    movq: Movq,
    movq_dtype: DType,
    encoder: std::path::PathBuf,
    /// The embedding of an empty picture: the negative for guidance.
    zero: Tensor,
    recipe: Recipe,
}

/// `img`'s pixels as CLIP takes them: the shorter side to 224 (cubic), the middle 224 × 224,
/// normalised; `(1, 3, 224, 224)`.
fn clip_pixels(img: &Rgb32f, target: &Target) -> Result<Tensor> {
    let (w, h) = (img.width.max(1), img.height.max(1));
    let s = clip::IMAGE as f64 / w.min(h) as f64;
    let (rw, rh) = (((w as f64 * s).round() as usize).max(clip::IMAGE), ((h as f64 * s).round() as usize).max(clip::IMAGE));
    let r = resize(img, rw, rh, Filter::Mitchell);
    let c = r.crop((rw - clip::IMAGE) / 2, (rh - clip::IMAGE) / 2, clip::IMAGE, clip::IMAGE);
    let n = clip::IMAGE * clip::IMAGE;
    let mut px = vec![0f32; 3 * n];
    for (i, p) in c.data.iter().enumerate() {
        for ch in 0..3 {
            if let (Some(o), Some(v), Some(m), Some(sd)) = (px.get_mut(ch * n + i), p.get(ch), clip::MEAN.get(ch), clip::STD.get(ch)) {
                *o = (v.clamp(0.0, 1.0) - m) / sd;
            }
        }
    }
    Ok(Tensor::from_vec(px, (1, 3, clip::IMAGE, clip::IMAGE), &target.device)?)
}

impl Kandinsky {
    /// Load from the model folder `dir` (`unet/`, `movq/`, `image_encoder/`), checking each file
    /// against its pin.
    pub fn load(dir: &Path, target: Target, recipe: Recipe, mon: &Monitor<'_>) -> Result<Kandinsky> {
        let stop = || if (mon.cancelled)() { Err(Error::Cancelled) } else { Ok(()) };
        let unet_file = pinned(dir, &KANDINSKY, "unet", UNET_FP32)?;
        let movq_file = pinned(dir, &KANDINSKY, "movq", UNET_FP32)?;
        let encoder = pinned(dir, &KANDINSKY, "image_encoder", IMAGE_ENCODER)?;
        (mon.progress)(0.2);
        stop()?;
        // (an empty picture is zeros after CLIP's normalisation)
        let zeros = Tensor::zeros((1, 3, clip::IMAGE, clip::IMAGE), DType::F32, &target.device)?;
        let zero = clip::image(&Weights::open(&encoder)?, &clip::VISION_BIG_G, &zeros, &target, mon.cancelled)?;
        (mon.progress)(0.4);
        stop()?;
        // (MoVQ is small: 32-bit everywhere)
        let movq_dtype = DType::F32;
        let movq = Movq::new(Weights::open(&movq_file)?.var_builder(movq_dtype, &target.device)?)?;
        (mon.progress)(0.5);
        stop()?;
        let unet = Unet::new(Weights::open(&unet_file)?.var_builder(target.dtype, &target.device)?)?;
        (mon.progress)(1.0);
        Ok(Kandinsky { target, unet, movq, movq_dtype, encoder, zero, recipe })
    }

    /// The image embedding of `img` (`(1, 1280)`).
    pub fn embed(&self, img: &Rgb32f, cancelled: &dyn Fn() -> bool) -> Result<Tensor> {
        clip::image(&Weights::open(&self.encoder)?, &clip::VISION_BIG_G, &clip_pixels(img, &self.target)?, &self.target, cancelled)
    }

    /// `img` (`w × h`, multiples of 64, display-encoded 0..=1) with the pixels where `hole` is
    /// set painted.
    pub fn inpaint(&self, img: &Rgb32f, hole: &[bool], mon: &Monitor<'_>) -> Result<Rgb32f> {
        let (w, h) = (img.width, img.height);
        let n = w.checked_mul(h).unwrap_or(0);
        if n == 0
            || w % 64 != 0
            || h % 64 != 0
            || w > lightcraft_erase::expand::MAX_CANVAS
            || h > lightcraft_erase::expand::MAX_CANVAS
            || img.data.len() != n
            || hole.len() != n
        {
            return Err(Error::Image(format!("{} can't paint a {w} × {h} canvas", KANDINSKY.label)));
        }
        let dev = &self.target.device;
        let dt = self.unet.conv_in.weight().dtype();
        // the condition: the photo (the picture's known part) and an empty picture
        let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
        for (i, _) in hole.iter().enumerate().filter(|(_, k)| !**k) {
            let (x, y) = (i % w, i / w);
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
        }
        if x1 <= x0 || y1 <= y0 {
            return Err(Error::Image("there is nothing of the photo to continue".into()));
        }
        let embeds = Tensor::cat(&[&self.zero, &self.embed(&img.crop(x0, y0, x1 - x0, y1 - y0), mon.cancelled)?], 0)?.to_dtype(dt)?;
        (mon.progress)(0.1);
        // the picture's latents, and which of them are the photo's (a latent is the photo's only
        // when all of its pixels are; then, as Kandinsky's pipeline does, a latent beside the new
        // area is new too)
        let mut px = vec![0f32; 3 * n];
        for (i, (p, &k)) in img.data.iter().zip(hole).enumerate() {
            for (c, v) in p.iter().enumerate() {
                if let Some(o) = px.get_mut(c * n + i) {
                    *o = if k { -1.0 } else { v.clamp(0.0, 1.0) * 2.0 - 1.0 };
                }
            }
        }
        let latents_img = self.movq.encode(&Tensor::from_vec(px, (1, 3, h, w), dev)?.to_dtype(self.movq_dtype)?)?.to_dtype(dt)?;
        let (lw, lh) = (w / 8, h / 8);
        let keep: Vec<bool> = (0..lw * lh)
            .map(|i| {
                let (x, y) = (i % lw * 8, i / lw * 8);
                !(0..8).any(|dy| (0..8).any(|dx| hole.get((y + dy) * w + x + dx).copied().unwrap_or(true)))
            })
            .collect();
        let mut grown = keep.clone();
        for (i, _) in keep.iter().enumerate().filter(|(_, k)| !**k) {
            let (x, y) = ((i % lw) as isize, (i / lw) as isize);
            for (dx, dy) in [(0, -1), (-1, 0), (-1, -1), (0, 1), (1, 0), (1, 1)] {
                let (nx, ny) = (x + dx, y + dy);
                if nx >= 0
                    && ny >= 0
                    && (nx as usize) < lw
                    && (ny as usize) < lh
                    && let Some(g) = grown.get_mut(ny as usize * lw + nx as usize)
                {
                    *g = false;
                }
            }
        }
        let mask = Tensor::from_vec(grown.iter().map(|&k| if k { 1.0f32 } else { 0.0 }).collect::<Vec<_>>(), (1, 1, lh, lw), dev)?.to_dtype(dt)?;
        let inv = (1.0 - &mask)?;
        let masked = latents_img.broadcast_mul(&mask)?;
        let r = &self.recipe;
        let mut sched = Dpm::new(r.steps, 0.00085, 0.012, Betas::Linear);
        let noise = Tensor::from_vec(gaussian(4 * lw * lh, r.seed), (1, 4, lh, lw), dev)?.to_dtype(dt)?;
        let mut latents = noise.clone();
        let ts = sched.timesteps().to_vec();
        let steps = ts.len().max(1);
        for (i, &t) in ts.iter().enumerate() {
            if (mon.cancelled)() {
                return Err(Error::Cancelled);
            }
            let x = Tensor::cat(&[&latents, &masked, &mask], 1)?;
            let x = Tensor::cat(&[&x, &x], 0)?;
            let eps = self.unet.forward(&x, t as f64, &embeds)?;
            let (u, c) = (eps.narrow(0, 0, 1)?, eps.narrow(0, 1, 1)?);
            let eps = (&u + ((&c - &u)? * r.guidance)?)?;
            latents = sched.step(i, &eps, &latents)?;
            // the photo's latents, at the next step's noise
            let proper = match ts.get(i + 1) {
                Some(&next) => sched.add_noise(&latents_img, &noise, next)?,
                None => latents_img.clone(),
            };
            latents = (proper.broadcast_mul(&mask)? + latents.broadcast_mul(&inv)?)?;
            (mon.progress)(0.1 + 0.85 * (i + 1) as f32 / steps as f32);
        }
        if (mon.cancelled)() {
            return Err(Error::Cancelled);
        }
        let out = self.movq.decode(&latents.to_dtype(self.movq_dtype)?)?;
        let v = ((out.to_dtype(DType::F32)?.clamp(-1.0, 1.0)? + 1.0)? * 0.5)?.flatten_all()?.to_vec1::<f32>()?;
        if v.len() != 3 * n {
            return Err(Error::Model(format!("MoVQ answered {} numbers, not {}", v.len(), 3 * n)));
        }
        let painted = Rgb32f::from_fn(w, h, |x, y| {
            let i = y * w + x;
            [0, 1, 2].map(|c| v.get(c * n + i).copied().filter(|u| u.is_finite()).unwrap_or(0.5))
        });
        (mon.progress)(1.0);
        Ok(crate::seam::match_edges(img, &painted, hole))
    }
}

impl Painter for Kandinsky {
    fn paint(&self, img: &Rgb32f, hole: &[bool]) -> lightcraft_erase::Result<Rgb32f> {
        self.inpaint(img, hole, &Monitor::NONE).map_err(|e| e.into_erase(KANDINSKY.label))
    }

    fn label(&self) -> &'static str {
        KANDINSKY.label
    }

    fn frame(&self, w: usize, h: usize) -> Frame {
        frame_for(w, h, 768, 64, 2.0)
    }

    fn paint_watched(&self, img: &Rgb32f, hole: &[bool], mon: &Monitor<'_>) -> lightcraft_erase::Result<Rgb32f> {
        self.inpaint(img, hole, mon).map_err(|e| e.into_erase(KANDINSKY.label))
    }

    fn detailed(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use candle_core::Device;
    use std::collections::HashMap;

    /// The model folder and reference outputs from diffusers (`$LIGHTCRAFT_KANDINSKY`,
    /// `$LIGHTCRAFT_KANDINSKY_REF`): never part of the repository.
    fn reference() -> Option<(std::path::PathBuf, HashMap<String, Tensor>)> {
        let dir = std::path::PathBuf::from(std::env::var_os("LIGHTCRAFT_KANDINSKY")?);
        Some((dir, candle_core::safetensors::load(std::env::var_os("LIGHTCRAFT_KANDINSKY_REF")?, &Device::Cpu).unwrap()))
    }

    fn max_diff(a: &Tensor, b: &Tensor) -> f32 {
        (a.to_dtype(DType::F32).unwrap() - b.to_dtype(DType::F32).unwrap())
            .unwrap()
            .abs()
            .unwrap()
            .flatten_all()
            .unwrap()
            .max(0)
            .unwrap()
            .to_scalar::<f32>()
            .unwrap()
    }

    #[test]
    fn matches_diffusers() {
        let Some((dir, r)) = reference() else { return };
        let target = Target::cpu();
        let ie = Weights::open(&pinned(&dir, &KANDINSKY, "image_encoder", IMAGE_ENCODER).unwrap()).unwrap();
        let emb = clip::image(&ie, &clip::VISION_BIG_G, &r["ie_pix"], &target, &|| false).unwrap();
        let d = max_diff(&emb, &r["ie_emb"]);
        eprintln!("image encoder: {d}");
        assert!(d < 2e-3, "image encoder: {d}");
        let zeros = Tensor::zeros((1, 3, 224, 224), DType::F32, &Device::Cpu).unwrap();
        let d = max_diff(&clip::image(&ie, &clip::VISION_BIG_G, &zeros, &target, &|| false).unwrap(), &r["ie_zero"]);
        assert!(d < 2e-3, "zero image: {d}");
        let unet =
            Unet::new(Weights::open(&pinned(&dir, &KANDINSKY, "unet", UNET_FP32).unwrap()).unwrap().var_builder(DType::F32, &Device::Cpu).unwrap())
                .unwrap();
        let d = max_diff(&unet.forward(&r["unet_x"], 500.0, &r["unet_emb"]).unwrap(), &r["unet_eps"]);
        eprintln!("unet: {d}");
        assert!(d < 2e-3, "unet: {d}");
        drop(unet);
        let movq =
            Movq::new(Weights::open(&pinned(&dir, &KANDINSKY, "movq", UNET_FP32).unwrap()).unwrap().var_builder(DType::F32, &Device::Cpu).unwrap())
                .unwrap();
        let d = max_diff(&movq.encode(&r["vq_img"]).unwrap(), &r["vq_enc"]);
        eprintln!("movq encode: {d}");
        assert!(d < 2e-3, "movq encode: {d}");
        let d = max_diff(&movq.decode(&r["vq_lat"]).unwrap(), &r["vq_dec"]);
        eprintln!("movq decode: {d}");
        assert!(d < 2e-3, "movq decode: {d}");
    }
}
