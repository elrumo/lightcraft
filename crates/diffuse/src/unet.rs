//! Stable Diffusion's UNet, for Stable Diffusion 1.5 and SDXL inpainting.
//!
//! Put together from candle-transformers' public Stable Diffusion blocks following its
//! `unet_2d.rs` (MIT OR Apache-2.0), with what candle's version leaves out: SDXL's added
//! conditioning (`addition_embed_type = "text_time"`), where the pooled text embedding and the
//! picture's size and crop are embedded and added to the timestep's embedding. Without it SDXL
//! still draws, but not the picture it was trained to draw.

use candle_core::{DType, Module, Tensor};
use candle_nn as nn;
use candle_transformers::models::stable_diffusion::embeddings::{TimestepEmbedding, Timesteps};
use candle_transformers::models::stable_diffusion::unet_2d::BlockConfig;
use candle_transformers::models::stable_diffusion::unet_2d_blocks::{
    CrossAttnDownBlock2D, CrossAttnDownBlock2DConfig, CrossAttnUpBlock2D, CrossAttnUpBlock2DConfig, DownBlock2D, DownBlock2DConfig,
    UNetMidBlock2DCrossAttn, UNetMidBlock2DCrossAttnConfig, UpBlock2D, UpBlock2DConfig,
};

use crate::{Error, Result};

/// The shape of a UNet.
#[derive(Clone, Debug)]
pub struct Config {
    pub blocks: Vec<BlockConfig>,
    pub layers_per_block: usize,
    pub cross_attention_dim: usize,
    pub use_linear_projection: bool,
    /// SDXL: the size of each embedded time id and of the added embedding's input.
    pub addition: Option<(usize, usize)>,
    pub in_channels: usize,
    pub out_channels: usize,
    /// Heads computed at a time in the attention layers (`None`: all at once).
    pub sliced_attention_size: Option<usize>,
}

fn bc(out_channels: usize, use_cross_attn: Option<usize>, attention_head_dim: usize) -> BlockConfig {
    BlockConfig { out_channels, use_cross_attn, attention_head_dim }
}

impl Config {
    /// Stable Diffusion 1.5 Inpainting.
    pub fn sd15_inpaint() -> Config {
        Config {
            blocks: vec![bc(320, Some(1), 8), bc(640, Some(1), 8), bc(1280, Some(1), 8), bc(1280, None, 8)],
            layers_per_block: 2,
            cross_attention_dim: 768,
            use_linear_projection: false,
            addition: None,
            in_channels: 9,
            out_channels: 4,
            sliced_attention_size: None,
        }
    }

    /// SDXL 1.0 Inpainting 0.1.
    pub fn sdxl_inpaint() -> Config {
        Config {
            blocks: vec![bc(320, None, 5), bc(640, Some(2), 10), bc(1280, Some(10), 20)],
            layers_per_block: 2,
            cross_attention_dim: 2048,
            use_linear_projection: true,
            addition: Some((256, 2816)),
            in_channels: 9,
            out_channels: 4,
            sliced_attention_size: None,
        }
    }
}

enum Down {
    Basic(DownBlock2D),
    Cross(CrossAttnDownBlock2D),
}

enum Up {
    Basic(UpBlock2D),
    Cross(CrossAttnUpBlock2D),
}

/// SDXL's added conditioning.
struct Addition {
    proj: Timesteps,
    embedding: TimestepEmbedding,
}

pub struct Unet {
    conv_in: nn::Conv2d,
    time_proj: Timesteps,
    time_embedding: TimestepEmbedding,
    addition: Option<Addition>,
    down: Vec<Down>,
    mid: UNetMidBlock2DCrossAttn,
    up: Vec<Up>,
    norm_out: nn::GroupNorm,
    conv_out: nn::Conv2d,
    /// Residuals each up block takes.
    up_resnets: usize,
}

const GROUPS: usize = 32;
const EPS: f64 = 1e-5;

impl Unet {
    pub fn new(vs: nn::VarBuilder, c: &Config) -> Result<Unet> {
        let n = c.blocks.len();
        let (Some(first), Some(last)) = (c.blocks.first(), c.blocks.last()) else {
            return Err(Error::Model("a UNet needs blocks".into()));
        };
        let at = |i: usize| c.blocks.get(i).copied().ok_or_else(|| Error::Model("bad UNet shape".into()));
        let (b0, bl) = (first.out_channels, last.out_channels);
        let temb = b0 * 4;
        let conv = nn::Conv2dConfig { padding: 1, ..Default::default() };
        let conv_in = nn::conv2d(c.in_channels, b0, 3, conv, vs.pp("conv_in"))?;
        let time_proj = Timesteps::new(b0, true, 0.0);
        let time_embedding = TimestepEmbedding::new(vs.pp("time_embedding"), b0, temb)?;
        let addition = match c.addition {
            Some((dim, input)) => {
                Some(Addition { proj: Timesteps::new(dim, true, 0.0), embedding: TimestepEmbedding::new(vs.pp("add_embedding"), input, temb)? })
            }
            None => None,
        };
        let mut down = Vec::with_capacity(n);
        for i in 0..n {
            let b = at(i)?;
            let inc = if i > 0 { at(i - 1)?.out_channels } else { b0 };
            let db = DownBlock2DConfig {
                num_layers: c.layers_per_block,
                resnet_eps: EPS,
                resnet_groups: GROUPS,
                add_downsample: i + 1 < n,
                downsample_padding: 1,
                ..Default::default()
            };
            let vb = vs.pp("down_blocks").pp(i.to_string());
            down.push(match b.use_cross_attn {
                Some(layers) => Down::Cross(CrossAttnDownBlock2D::new(
                    vb,
                    inc,
                    b.out_channels,
                    Some(temb),
                    false,
                    CrossAttnDownBlock2DConfig {
                        downblock: db,
                        attn_num_head_channels: b.attention_head_dim,
                        cross_attention_dim: c.cross_attention_dim,
                        sliced_attention_size: c.sliced_attention_size,
                        use_linear_projection: c.use_linear_projection,
                        transformer_layers_per_block: layers,
                    },
                )?),
                None => Down::Basic(DownBlock2D::new(vb, inc, b.out_channels, Some(temb), db)?),
            });
        }
        let mid = UNetMidBlock2DCrossAttn::new(
            vs.pp("mid_block"),
            bl,
            Some(temb),
            false,
            UNetMidBlock2DCrossAttnConfig {
                resnet_eps: EPS,
                output_scale_factor: 1.0,
                cross_attn_dim: c.cross_attention_dim,
                attn_num_head_channels: last.attention_head_dim,
                resnet_groups: Some(GROUPS),
                use_linear_projection: c.use_linear_projection,
                transformer_layers_per_block: last.use_cross_attn.unwrap_or(1),
                sliced_attention_size: c.sliced_attention_size,
                ..Default::default()
            },
        )?;
        let mut up = Vec::with_capacity(n);
        for i in 0..n {
            let b = at(n - 1 - i)?;
            let prev = if i > 0 { at(n - i)?.out_channels } else { bl };
            let inc = at(if i + 1 == n { 0 } else { n - i - 2 })?.out_channels;
            let ub = UpBlock2DConfig {
                num_layers: c.layers_per_block + 1,
                resnet_eps: EPS,
                resnet_groups: GROUPS,
                add_upsample: i + 1 < n,
                ..Default::default()
            };
            let vb = vs.pp("up_blocks").pp(i.to_string());
            up.push(match b.use_cross_attn {
                Some(layers) => Up::Cross(CrossAttnUpBlock2D::new(
                    vb,
                    inc,
                    prev,
                    b.out_channels,
                    Some(temb),
                    false,
                    CrossAttnUpBlock2DConfig {
                        upblock: ub,
                        attn_num_head_channels: b.attention_head_dim,
                        cross_attention_dim: c.cross_attention_dim,
                        sliced_attention_size: c.sliced_attention_size,
                        use_linear_projection: c.use_linear_projection,
                        transformer_layers_per_block: layers,
                    },
                )?),
                None => Up::Basic(UpBlock2D::new(vb, inc, prev, b.out_channels, Some(temb), ub)?),
            });
        }
        let norm_out = nn::group_norm(GROUPS, b0, EPS, vs.pp("conv_norm_out"))?;
        let conv_out = nn::conv2d(b0, c.out_channels, 3, conv, vs.pp("conv_out"))?;
        Ok(Unet { conv_in, time_proj, time_embedding, addition, down, mid, up, norm_out, conv_out, up_resnets: c.layers_per_block + 1 })
    }

    /// The predicted noise for `xs` (`(batch, in_channels, h, w)`) at `timestep`, given the text
    /// encoder's `context` (`(batch, 77, cross_attention_dim)`) and, for SDXL, the pooled text
    /// embedding and the time ids (`(batch, 1280)`, `(batch, 6)`).
    pub fn forward(&self, xs: &Tensor, timestep: f64, context: &Tensor, added: Option<(&Tensor, &Tensor)>) -> Result<Tensor> {
        let (batch, _, h, w) = xs.dims4()?;
        let n = self.down.len();
        let factor = 1usize << n.saturating_sub(1);
        let odd = h % factor != 0 || w % factor != 0;
        // 1. time
        let t = (Tensor::ones(batch, xs.dtype(), xs.device())? * timestep)?;
        let mut emb = self.time_embedding.forward(&self.time_proj.forward(&t)?)?;
        match (&self.addition, added) {
            (Some(a), Some((text, ids))) => {
                let (b, k) = ids.dims2()?;
                let ids = a.proj.forward(&ids.flatten_all()?)?.reshape((b, ()))?;
                let _ = k;
                let input = Tensor::cat(&[text, &ids.to_dtype(text.dtype())?], 1)?;
                emb = (emb + a.embedding.forward(&input.to_dtype(xs.dtype())?)?)?;
            }
            (Some(_), None) => return Err(Error::Model("SDXL needs its added conditioning".into())),
            _ => {}
        }
        // 2. in
        let xs = self.conv_in.forward(xs)?;
        // 3. down
        let mut res = vec![xs.clone()];
        let mut xs = xs;
        for d in &self.down {
            let (out, r) = match d {
                Down::Basic(b) => b.forward(&xs, Some(&emb))?,
                Down::Cross(b) => b.forward(&xs, Some(&emb), Some(context))?,
            };
            res.extend(r);
            xs = out;
        }
        // 4. mid
        let mut xs = self.mid.forward(&xs, Some(&emb), Some(context))?;
        // 5. up
        for (i, u) in self.up.iter().enumerate() {
            let at = res.len().checked_sub(self.up_resnets).ok_or_else(|| Error::Model("bad UNet shape".into()))?;
            let r = res.split_off(at);
            let size = match res.last() {
                Some(l) if odd && i + 1 < n => {
                    let (_, _, h, w) = l.dims4()?;
                    Some((h, w))
                }
                _ => None,
            };
            xs = match u {
                Up::Basic(b) => b.forward(&xs, &r, Some(&emb), size)?,
                Up::Cross(b) => b.forward(&xs, &r, Some(&emb), size, Some(context))?,
            };
        }
        // 6. out
        let xs = nn::ops::silu(&self.norm_out.forward(&xs)?)?;
        Ok(self.conv_out.forward(&xs)?)
    }

    /// The dtype the network computes in (that of its first convolution).
    pub fn dtype(&self) -> DType {
        self.conv_in.weight().dtype()
    }
}
