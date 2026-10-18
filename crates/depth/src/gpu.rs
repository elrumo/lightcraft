//! Depth Anything V2 Small on the GPU: candle on Metal, in half precision, from the original weights
//! (`depth_anything_v2_vits.safetensors`, the names of the authors' code). The same network as the ONNX export
//! rten runs on the CPU, written out here because candle's own version only takes a square picture (its
//! position embeddings and its decoder's sizes are fixed at 518 × 518) and a photo isn't square.
//!
//! The backbone is DINOv2 ViT-S/14 (12 blocks of 6 heads, layer scale), whose position embeddings are
//! resampled bicubically to the picture's grid of patches; four of its blocks (2, 5, 8, 11) feed the DPT decoder,
//! which fuses them from the coarsest up and answers one depth value per pixel.

use candle_core::{DType, Device, Module, Tensor};
use candle_nn::{Conv2d, Conv2dConfig, ConvTranspose2d, ConvTranspose2dConfig, LayerNorm, Linear, VarBuilder};
use lightcraft_vision::weights::{Backend, Weights};

use crate::{Error, PATCH, Result};

const DIM: usize = 384;
const HEADS: usize = 6;
const BLOCKS: usize = 12;
/// The blocks whose output the decoder reads.
const TAPS: [usize; 4] = [2, 5, 8, 11];
/// The grid of patches the position embeddings were trained on.
const POS_GRID: usize = 37;
const OUT_CHANNELS: [usize; 4] = [48, 96, 192, 384];
const FEATURES: usize = 64;

fn bad(e: impl std::fmt::Display) -> Error {
    Error::Model(e.to_string())
}

struct Block {
    norm1: LayerNorm,
    qkv: Linear,
    proj: Linear,
    gamma1: Tensor,
    norm2: LayerNorm,
    fc1: Linear,
    fc2: Linear,
    gamma2: Tensor,
}

impl Block {
    fn new(vb: VarBuilder) -> candle_core::Result<Block> {
        Ok(Block {
            norm1: candle_nn::layer_norm(DIM, 1e-6, vb.pp("norm1"))?,
            qkv: candle_nn::linear(DIM, 3 * DIM, vb.pp("attn.qkv"))?,
            proj: candle_nn::linear(DIM, DIM, vb.pp("attn.proj"))?,
            gamma1: vb.get(DIM, "ls1.gamma")?,
            norm2: candle_nn::layer_norm(DIM, 1e-6, vb.pp("norm2"))?,
            fc1: candle_nn::linear(DIM, 4 * DIM, vb.pp("mlp.fc1"))?,
            fc2: candle_nn::linear(4 * DIM, DIM, vb.pp("mlp.fc2"))?,
            gamma2: vb.get(DIM, "ls2.gamma")?,
        })
    }

    fn forward(&self, x: &Tensor) -> candle_core::Result<Tensor> {
        let (b, n, c) = x.dims3()?;
        let hd = c / HEADS;
        let qkv = self.qkv.forward(&self.norm1.forward(x)?)?.reshape((b, n, 3, HEADS, hd))?.permute((2, 0, 3, 1, 4))?;
        let (q, k, v) = (qkv.get(0)?.contiguous()?, qkv.get(1)?.contiguous()?, qkv.get(2)?.contiguous()?);
        let att = q.affine((hd as f64).powf(-0.5), 0.0)?.matmul(&k.transpose(2, 3)?.contiguous()?)?;
        let att = candle_nn::ops::softmax_last_dim(&att)?;
        let out = att.matmul(&v)?.transpose(1, 2)?.reshape((b, n, c))?;
        let x = (x + self.proj.forward(&out)?.broadcast_mul(&self.gamma1)?)?;
        let m = self.fc2.forward(&self.fc1.forward(&self.norm2.forward(&x)?)?.gelu_erf()?)?;
        x + m.broadcast_mul(&self.gamma2)?
    }
}

struct Backbone {
    patch: Conv2d,
    cls: Tensor,
    pos: Vec<f32>,
    blocks: Vec<Block>,
    norm: LayerNorm,
}

impl Backbone {
    fn new(vb: VarBuilder, pos: Vec<f32>) -> candle_core::Result<Backbone> {
        let cfg = Conv2dConfig { stride: PATCH, ..Default::default() };
        Ok(Backbone {
            patch: candle_nn::conv2d(3, DIM, PATCH, cfg, vb.pp("patch_embed.proj"))?,
            cls: vb.get((1, 1, DIM), "cls_token")?,
            pos,
            blocks: (0..BLOCKS).map(|i| Block::new(vb.pp(format!("blocks.{i}")))).collect::<candle_core::Result<_>>()?,
            norm: candle_nn::layer_norm(DIM, 1e-6, vb.pp("norm"))?,
        })
    }

    /// The patch tokens (`1 × gh·gw × 384`) after each of the [`TAPS`] blocks, normalized.
    fn forward(&self, image: &Tensor, gh: usize, gw: usize) -> candle_core::Result<Vec<Tensor>> {
        let dev = image.device();
        let dtype = image.dtype();
        let patches = self.patch.forward(image)?.flatten_from(2)?.transpose(1, 2)?;
        // (the class token's embedding, then the patches' resampled to this picture's grid)
        let cls_pos = Tensor::from_slice(self.pos.get(..DIM).unwrap_or(&[]), (1, 1, DIM), dev)?.to_dtype(dtype)?;
        let grid = resample_pos(self.pos.get(DIM..).unwrap_or(&[]), gh, gw);
        let grid = Tensor::from_vec(grid, (1, gh * gw, DIM), dev)?.to_dtype(dtype)?;
        let mut x = Tensor::cat(&[&(&self.cls + &cls_pos)?, &(&patches + &grid)?], 1)?;
        let mut out = Vec::with_capacity(TAPS.len());
        for (i, block) in self.blocks.iter().enumerate() {
            x = block.forward(&x)?;
            if TAPS.contains(&i) {
                out.push(self.norm.forward(&x)?.narrow(1, 1, gh * gw)?);
            }
            if i >= TAPS[TAPS.len() - 1] {
                break;
            }
        }
        Ok(out)
    }
}

/// The weight of a bicubic tap at distance `t` (PyTorch's, with its −0.75).
fn cubic(t: f32) -> f32 {
    const A: f32 = -0.75;
    let t = t.abs();
    if t <= 1.0 {
        ((A + 2.0) * t - (A + 3.0)) * t * t + 1.0
    } else if t < 2.0 {
        ((A * t - 5.0 * A) * t + 8.0 * A) * t - 4.0 * A
    } else {
        0.0
    }
}

/// For each of `out` positions along a line of `inp` samples: four sample indexes and their weights. As DINOv2
/// resamples its position grid: the scale is `inp / (out + 0.1)`, not `inp / out` (its "interpolate offset"), and the
/// edge sample is repeated beyond the ends.
fn taps(inp: usize, out: usize) -> Vec<([usize; 4], [f32; 4])> {
    let scale = inp as f32 / (out as f32 + 0.1);
    (0..out)
        .map(|o| {
            let src = (o as f32 + 0.5) * scale - 0.5;
            let base = src.floor();
            let t = src - base;
            let at = |d: i32| (base as i32 + d).clamp(0, inp as i32 - 1) as usize;
            ([at(-1), at(0), at(1), at(2)], [cubic(t + 1.0), cubic(t), cubic(1.0 - t), cubic(2.0 - t)])
        })
        .collect()
}

/// The `POS_GRID × POS_GRID × DIM` position embeddings (row major) resampled bicubically to `gh × gw`.
fn resample_pos(pos: &[f32], gh: usize, gw: usize) -> Vec<f32> {
    let at = |y: usize, x: usize, c: usize| pos.get((y * POS_GRID + x) * DIM + c).copied().unwrap_or(0.0);
    let (ty, tx) = (taps(POS_GRID, gh), taps(POS_GRID, gw));
    let mut out = vec![0f32; gh * gw * DIM];
    for (y, (iy, wy)) in ty.iter().enumerate() {
        for (x, (ix, wx)) in tx.iter().enumerate() {
            for c in 0..DIM {
                let mut v = 0.0;
                for (j, &sy) in iy.iter().enumerate() {
                    for (i, &sx) in ix.iter().enumerate() {
                        v += wy[j] * wx[i] * at(sy, sx, c);
                    }
                }
                if let Some(o) = out.get_mut((y * gw + x) * DIM + c) {
                    *o = v;
                }
            }
        }
    }
    out
}

struct ResUnit {
    conv1: Conv2d,
    conv2: Conv2d,
}

impl ResUnit {
    fn new(vb: VarBuilder) -> candle_core::Result<ResUnit> {
        let cfg = Conv2dConfig { padding: 1, ..Default::default() };
        Ok(ResUnit {
            conv1: candle_nn::conv2d(FEATURES, FEATURES, 3, cfg, vb.pp("conv1"))?,
            conv2: candle_nn::conv2d(FEATURES, FEATURES, 3, cfg, vb.pp("conv2"))?,
        })
    }

    fn forward(&self, x: &Tensor) -> candle_core::Result<Tensor> {
        let y = self.conv1.forward(&x.relu()?)?;
        let y = self.conv2.forward(&y.relu()?)?;
        y + x
    }
}

struct Fusion {
    unit1: ResUnit,
    unit2: ResUnit,
    out: Conv2d,
}

impl Fusion {
    fn new(vb: VarBuilder) -> candle_core::Result<Fusion> {
        Ok(Fusion {
            unit1: ResUnit::new(vb.pp("resConfUnit1"))?,
            unit2: ResUnit::new(vb.pp("resConfUnit2"))?,
            out: candle_nn::conv2d(FEATURES, FEATURES, 1, Default::default(), vb.pp("out_conv"))?,
        })
    }

    /// `x` (plus `skip`, once through the first unit), through the second, scaled to `size` (or doubled), through the 1×1.
    fn forward(&self, x: &Tensor, skip: Option<&Tensor>, size: Option<(usize, usize)>) -> candle_core::Result<Tensor> {
        let x = match skip {
            Some(s) => (x + self.unit1.forward(s)?)?,
            None => x.clone(),
        };
        let x = self.unit2.forward(&x)?;
        let (h, w) = size.unwrap_or_else(|| x.dims4().map_or((0, 0), |d| (d.2 * 2, d.3 * 2)));
        self.out.forward(&x.upsample_bilinear2d(h, w, true)?)
    }
}

struct Head {
    project: Vec<Conv2d>,
    up4: ConvTranspose2d,
    up2: ConvTranspose2d,
    down: Conv2d,
    rn: Vec<Conv2d>,
    fusion: Vec<Fusion>,
    out1: Conv2d,
    out2a: Conv2d,
    out2b: Conv2d,
}

impl Head {
    fn new(vb: VarBuilder) -> candle_core::Result<Head> {
        let one = Conv2dConfig::default();
        let s = vb.pp("scratch");
        let down_cfg = Conv2dConfig { padding: 1, stride: 2, ..Default::default() };
        let t = |stride| ConvTranspose2dConfig { stride, ..Default::default() };
        let three = Conv2dConfig { padding: 1, ..Default::default() };
        Ok(Head {
            project: (0..4)
                .map(|i| candle_nn::conv2d(DIM, OUT_CHANNELS[i], 1, one, vb.pp(format!("projects.{i}"))))
                .collect::<candle_core::Result<_>>()?,
            up4: candle_nn::conv_transpose2d(OUT_CHANNELS[0], OUT_CHANNELS[0], 4, t(4), vb.pp("resize_layers.0"))?,
            up2: candle_nn::conv_transpose2d(OUT_CHANNELS[1], OUT_CHANNELS[1], 2, t(2), vb.pp("resize_layers.1"))?,
            down: candle_nn::conv2d(OUT_CHANNELS[3], OUT_CHANNELS[3], 3, down_cfg, vb.pp("resize_layers.3"))?,
            rn: (0..4)
                .map(|i| candle_nn::conv2d_no_bias(OUT_CHANNELS[i], FEATURES, 3, three, s.pp(format!("layer{}_rn", i + 1))))
                .collect::<candle_core::Result<_>>()?,
            fusion: (1..=4).map(|i| Fusion::new(s.pp(format!("refinenet{i}")))).collect::<candle_core::Result<_>>()?,
            out1: candle_nn::conv2d(FEATURES, FEATURES / 2, 3, three, s.pp("output_conv1"))?,
            out2a: candle_nn::conv2d(FEATURES / 2, 32, 3, three, s.pp("output_conv2.0"))?,
            out2b: candle_nn::conv2d(32, 1, 1, one, s.pp("output_conv2.2"))?,
        })
    }

    /// The depth (`1 × 1 × gh·14 × gw·14`) from the four blocks' patch tokens.
    fn forward(&self, tokens: &[Tensor], gh: usize, gw: usize) -> candle_core::Result<Tensor> {
        let mut layers = Vec::with_capacity(4);
        for (i, t) in tokens.iter().enumerate() {
            let (b, _, c) = t.dims3()?;
            let x = t.transpose(1, 2)?.reshape((b, c, gh, gw))?;
            let x = self.project.get(i).ok_or_else(|| candle_core::Error::Msg("four blocks' tokens".into()))?.forward(&x)?;
            layers.push(match i {
                0 => self.up4.forward(&x)?,
                1 => self.up2.forward(&x)?,
                2 => x,
                _ => self.down.forward(&x)?,
            });
        }
        let rn: Vec<Tensor> = layers.iter().zip(&self.rn).map(|(l, c)| c.forward(l)).collect::<candle_core::Result<_>>()?;
        let size = |t: &Tensor| t.dims4().map(|d| (d.2, d.3));
        let p4 = self.fusion[3].forward(&rn[3], None, Some(size(&rn[2])?))?;
        let p3 = self.fusion[2].forward(&p4, Some(&rn[2]), Some(size(&rn[1])?))?;
        let p2 = self.fusion[1].forward(&p3, Some(&rn[1]), Some(size(&rn[0])?))?;
        let p1 = self.fusion[0].forward(&p2, Some(&rn[0]), None)?;
        let out = self.out1.forward(&p1)?.upsample_bilinear2d(gh * PATCH, gw * PATCH, true)?;
        let out = self.out2a.forward(&out)?.relu()?;
        self.out2b.forward(&out)?.relu()
    }
}

/// The network, on the GPU.
pub struct Gpu {
    backbone: Backbone,
    head: Head,
    device: Device,
}

impl Gpu {
    /// Loads `file` onto the best device; `None` when the machine has no GPU.
    pub fn load(file: &std::path::Path) -> Result<Option<Gpu>> {
        let device = lightcraft_vision::siglip::best_device();
        if device.is_cpu() {
            return Ok(None);
        }
        let weights = Weights::open(file).map_err(bad)?;
        // (the position embeddings are resampled on the CPU, in full precision)
        let pos = weights.read("pretrained.pos_embed").map_err(bad)?.flatten_all().and_then(|t| t.to_vec1::<f32>()).map_err(bad)?;
        let vb = VarBuilder::from_backend(Box::new(Backend(weights)), DType::F16, device.clone());
        let backbone = Backbone::new(vb.pp("pretrained"), pos).map_err(bad)?;
        let head = Head::new(vb.pp("depth_head")).map_err(bad)?;
        let gpu = Gpu { backbone, head, device };
        // (the first photo doesn't pay for compiling the kernels)
        gpu.run(&vec![0f32; 3 * 2 * PATCH * 2 * PATCH], 2 * PATCH, 2 * PATCH)?;
        Ok(Some(gpu))
    }

    /// The depth of a `w × h` picture (`pixels`: planar RGB, normalized; `w` and `h` multiples of the patch size).
    pub fn run(&self, pixels: &[f32], w: usize, h: usize) -> Result<Vec<f32>> {
        let (gh, gw) = (h / PATCH, w / PATCH);
        let image = Tensor::from_slice(pixels, (1, 3, h, w), &self.device).map_err(bad)?.to_dtype(DType::F16).map_err(bad)?;
        let go = || -> candle_core::Result<Vec<f32>> {
            let tokens = self.backbone.forward(&image, gh, gw)?;
            self.head.forward(&tokens, gh, gw)?.to_dtype(DType::F32)?.flatten_all()?.to_vec1::<f32>()
        };
        go().map_err(bad)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_position_grid_keeps_its_values_where_the_size_is_unchanged() {
        // a grid resampled to its own size, with the 0.1 offset, is nearly itself
        let pos: Vec<f32> = (0..POS_GRID * POS_GRID * DIM).map(|i| ((i / DIM) % POS_GRID) as f32).collect();
        let r = resample_pos(&pos, POS_GRID, POS_GRID);
        let drift = r.iter().zip(&pos).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
        assert!(drift < 0.2, "{drift}");
    }

    #[test]
    fn taps_stay_inside_the_grid_and_sum_to_one() {
        for (inp, out) in [(37, 37), (37, 49), (37, 19), (1, 5)] {
            for (idx, w) in taps(inp, out) {
                assert!(idx.iter().all(|&i| i < inp));
                assert!((w.iter().sum::<f32>() - 1.0).abs() < 1e-4, "{w:?}");
            }
        }
    }
}
