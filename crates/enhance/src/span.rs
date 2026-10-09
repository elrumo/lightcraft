//! The SPAN network.
//!
//! Every convolution in SPAN is a `Conv3XC`: during training a 1×1 → 3×3 → 1×1 chain plus a 1×1
//! skip, which is one 3×3 convolution at inference. Distributed files hold the training branches
//! (`sk`, `conv.0..2`) and usually a stored fused copy (`eval_conv`) that can be stale, so, like
//! the reference loaders, we fuse the branches ourselves ([`fuse`]) and use the stored copy only
//! when a file has nothing else.

use std::collections::HashMap;
use std::path::Path;

use candle_core::{DType, Device, Tensor};
use candle_nn::ops::{pixel_shuffle, sigmoid, silu};

use crate::{Error, Result};

/// Largest weights file read (the Nomos models are ~4.5 MB).
const MAX_FILE: u64 = 64 << 20;
/// Number of SPAB blocks.
const BLOCKS: usize = 6;
/// Per-channel mean and range the Nomos / neosr models are trained with (files without a
/// `no_norm` tensor), as in the reference loaders.
const MEAN: [f32; 3] = [0.4488, 0.4371, 0.4040];
const IMG_RANGE: f32 = 255.0;

/// A convolution with bias; `pad` keeps the size for 3×3 (1) and 1×1 (0) kernels.
struct Conv {
    w: Tensor,
    /// `(1, out, 1, 1)`.
    b: Tensor,
    pad: usize,
}

impl Conv {
    fn new(w: Tensor, b: Tensor, pad: usize) -> Result<Self> {
        let out = b.elem_count();
        Ok(Conv { w, b: b.reshape((1, out, 1, 1))?, pad })
    }

    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        Ok(x.conv2d(&self.w, self.pad, 1, 1, 1)?.broadcast_add(&self.b)?)
    }
}

/// Three fused convolutions with the parameter-free attention of SPAN.
struct Spab {
    c1: Conv,
    c2: Conv,
    c3: Conv,
}

impl Spab {
    /// `(output, the first convolution's activated output)`.
    ///
    /// The reference applies its SiLU in place (`SiLU(inplace=True)`), so the `out1` it returns
    /// and SPAN concatenates from the last block is the activated tensor, not the raw one.
    fn forward(&self, x: &Tensor) -> Result<(Tensor, Tensor)> {
        let a1 = silu(&self.c1.forward(x)?)?;
        let o2 = self.c2.forward(&a1)?;
        let o3 = self.c3.forward(&silu(&o2)?)?;
        let att = (sigmoid(&o3)? - 0.5)?;
        Ok(((o3 + x)?.mul(&att)?, a1))
    }
}

/// A loaded SPAN model.
pub struct Span {
    conv_1: Conv,
    blocks: Vec<Spab>,
    conv_2: Conv,
    conv_cat: Conv,
    upsampler: Conv,
    scale: usize,
    /// `(1, 3, 1, 1)`, `None` for models trained without input normalisation.
    mean: Option<Tensor>,
    device: Device,
}

fn bad(msg: impl Into<String>) -> Error {
    Error::Model(msg.into())
}

/// Tensor `name` as f32, with exactly `dims`.
fn take(t: &HashMap<String, Tensor>, name: &str, dims: &[usize]) -> Result<Tensor> {
    let x = t.get(name).ok_or_else(|| bad(format!("{name} is missing: this is not a SPAN model")))?;
    if x.dims() != dims {
        return Err(bad(format!("{name} has shape {:?}, expected {dims:?}", x.dims())));
    }
    Ok(x.to_dtype(DType::F32)?)
}

/// One 3×3 convolution equal to a `Conv3XC` of `ci` → `co` channels, from the file's tensors
/// under `prefix`: fused from its branches, or the stored fused copy when there are no branches.
fn conv3xc(t: &HashMap<String, Tensor>, prefix: &str, ci: usize, co: usize) -> Result<Conv> {
    if t.contains_key(&format!("{prefix}.conv.0.weight")) {
        let m = t
            .get(&format!("{prefix}.conv.0.weight"))
            .and_then(|w| w.dims().first().copied())
            .filter(|m| (1..=4096).contains(m))
            .ok_or_else(|| bad(format!("{prefix}.conv.0.weight has an unusable shape")))?;
        let m2 = t
            .get(&format!("{prefix}.conv.1.weight"))
            .and_then(|w| w.dims().first().copied())
            .filter(|m| (1..=4096).contains(m))
            .ok_or_else(|| bad(format!("{prefix}.conv.1.weight is missing or unusable")))?;
        let (w, b) = fuse(
            &take(t, &format!("{prefix}.sk.weight"), &[co, ci, 1, 1])?,
            &take(t, &format!("{prefix}.sk.bias"), &[co])?,
            &take(t, &format!("{prefix}.conv.0.weight"), &[m, ci, 1, 1])?,
            &take(t, &format!("{prefix}.conv.0.bias"), &[m])?,
            &take(t, &format!("{prefix}.conv.1.weight"), &[m2, m, 3, 3])?,
            &take(t, &format!("{prefix}.conv.1.bias"), &[m2])?,
            &take(t, &format!("{prefix}.conv.2.weight"), &[co, m2, 1, 1])?,
            &take(t, &format!("{prefix}.conv.2.bias"), &[co])?,
        )?;
        return Conv::new(w, b, 1);
    }
    Conv::new(take(t, &format!("{prefix}.eval_conv.weight"), &[co, ci, 3, 3])?, take(t, &format!("{prefix}.eval_conv.bias"), &[co])?, 1)
}

/// Fuse a `Conv3XC`'s branches into one 3×3 kernel and bias:
/// `out = conv2(conv1(conv0(pad(x)))) + sk(x)` with 1×1 `conv0` (ci→m), 3×3 `conv1` (m→m2, no
/// padding) and 1×1 `conv2` (m2→co), the zero padding applied before `conv0`.
///
/// Exact, borders included: a padded pixel goes through `conv0`'s bias, which is what the bias
/// term below accounts for.
#[allow(clippy::too_many_arguments)]
pub(crate) fn fuse(
    sk_w: &Tensor,
    sk_b: &Tensor,
    w0: &Tensor,
    b0: &Tensor,
    w1: &Tensor,
    b1: &Tensor,
    w2: &Tensor,
    b2: &Tensor,
) -> Result<(Tensor, Tensor)> {
    let (co, ci) = (sk_w.dim(0)?, sk_w.dim(1)?);
    let (m, m2) = (w0.dim(0)?, w1.dim(0)?);
    let w0m = w0.reshape((m, ci))?;
    let w2m = w2.reshape((co, m2))?;
    // taps first: (9, m2, m) · (m, ci) → (9, m2, ci), then · (co, m2) → (9, co, ci)
    let w1t = w1.reshape((m2, m, 9))?.permute((2, 0, 1))?.contiguous()?;
    let w01 = w1t.broadcast_matmul(&w0m)?;
    let w012 = w2m.broadcast_matmul(&w01)?;
    // (9, co, ci) → (co, ci, 9), plus the skip on the centre tap
    let mut centre = [0f32; 9];
    centre[4] = 1.0;
    let one_hot = Tensor::from_slice(&centre, (1, 1, 9), w0.device())?;
    let w = (w012.permute((1, 2, 0))? + sk_w.reshape((co, ci, 1))?.broadcast_mul(&one_hot)?)?;
    let w = w.contiguous()?.reshape((co, ci, 3, 3))?;
    // bias: conv1's taps all see conv0's bias; then conv2; then the skip's
    let tap_sum = w1.sum(3)?.sum(2)?; // (m2, m)
    let b01 = (tap_sum.matmul(&b0.unsqueeze(1)?)?.squeeze(1)? + b1)?;
    let b = ((w2m.matmul(&b01.unsqueeze(1)?)?.squeeze(1)? + b2)? + sk_b)?;
    Ok((w, b))
}

impl Span {
    /// Load the model in `path` (a `.safetensors` file) onto `device`.
    pub fn load(path: &Path, device: &Device) -> Result<Self> {
        let len = std::fs::metadata(path).map_err(|e| bad(format!("{}: {e}", path.display())))?.len();
        if len == 0 || len > MAX_FILE {
            return Err(bad(format!("{}: {len} bytes is not a SPAN model", path.display())));
        }
        let tensors = candle_core::safetensors::load(path, &Device::Cpu).map_err(|e| bad(format!("{}: {e}", path.display())))?;
        Self::from_tensors(&tensors, device)
    }

    /// Build the model from named tensors (any float type).
    pub(crate) fn from_tensors(t: &HashMap<String, Tensor>, device: &Device) -> Result<Self> {
        // sizes come from the file, so check them before using them
        let sk = t
            .get("conv_1.sk.weight")
            .or_else(|| t.get("conv_1.eval_conv.weight"))
            .ok_or_else(|| bad("conv_1 is missing: this is not a SPAN model"))?;
        let (c, c_in) = match sk.dims() {
            [c, i, _, _] if (1..=512).contains(c) && *i == 3 => (*c, *i),
            d => return Err(bad(format!("conv_1 has shape {d:?}: only 3-channel SPAN models of up to 512 features are supported"))),
        };
        let up = t.get("upsampler.0.weight").ok_or_else(|| bad("upsampler.0.weight is missing: this is not a SPAN model"))?;
        let out_planes = match up.dims() {
            [o, i, 3, 3] if *i == c => *o,
            d => return Err(bad(format!("upsampler.0.weight has shape {d:?}"))),
        };
        let scale = (1..=8usize)
            .find(|s| s * s * c_in == out_planes)
            .ok_or_else(|| bad(format!("upsampler outputs {out_planes} planes: not a 3-channel ×1…8 model")))?;

        let blocks = (1..=BLOCKS)
            .map(|i| {
                Ok(Spab {
                    c1: conv3xc(t, &format!("block_{i}.c1_r"), c, c)?,
                    c2: conv3xc(t, &format!("block_{i}.c2_r"), c, c)?,
                    c3: conv3xc(t, &format!("block_{i}.c3_r"), c, c)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let mean = if t.contains_key("no_norm") { None } else { Some(Tensor::from_slice(&MEAN, (1, 3, 1, 1), device)?) };
        let model = Span {
            conv_1: conv3xc(t, "conv_1", c_in, c)?,
            blocks,
            conv_2: conv3xc(t, "conv_2", c, c)?,
            conv_cat: Conv::new(take(t, "conv_cat.weight", &[c, 4 * c, 1, 1])?, take(t, "conv_cat.bias", &[c])?, 0)?,
            upsampler: Conv::new(take(t, "upsampler.0.weight", &[out_planes, c, 3, 3])?, take(t, "upsampler.0.bias", &[out_planes])?, 1)?,
            scale,
            mean,
            device: device.clone(),
        };
        model.moved_to(device)
    }

    fn moved_to(mut self, device: &Device) -> Result<Self> {
        let mv = |c: &mut Conv| -> Result<()> {
            c.w = c.w.to_device(device)?;
            c.b = c.b.to_device(device)?;
            Ok(())
        };
        mv(&mut self.conv_1)?;
        mv(&mut self.conv_2)?;
        mv(&mut self.conv_cat)?;
        mv(&mut self.upsampler)?;
        for b in &mut self.blocks {
            mv(&mut b.c1)?;
            mv(&mut b.c2)?;
            mv(&mut b.c3)?;
        }
        Ok(self)
    }

    /// The enlargement factor (2 or 4 for the models people use).
    pub fn scale(&self) -> usize {
        self.scale
    }

    pub(crate) fn device(&self) -> &Device {
        &self.device
    }

    /// `(1, 3, h, w)` in [0, 1], sRGB-encoded → `(1, 3, h·scale, w·scale)`, not clamped.
    pub(crate) fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let x = match &self.mean {
            Some(mean) => (x.broadcast_sub(mean)? * f64::from(IMG_RANGE))?,
            None => x.clone(),
        };
        let f0 = self.conv_1.forward(&x)?;
        let mut cur = f0.clone();
        let (mut first, mut last_o1) = (None, None);
        for (i, blk) in self.blocks.iter().enumerate() {
            let (out, o1) = blk.forward(&cur)?;
            if i == 0 {
                first = Some(out.clone());
            }
            if i + 1 == self.blocks.len() {
                last_o1 = Some(o1);
            }
            cur = out;
        }
        let (Some(first), Some(last_o1)) = (first, last_o1) else { return Err(bad("the model has no blocks")) };
        let b6 = self.conv_2.forward(&cur)?;
        let y = self.conv_cat.forward(&Tensor::cat(&[&f0, &b6, &first, &last_o1], 1)?)?;
        Ok(pixel_shuffle(&self.upsampler.forward(&y)?, self.scale)?)
    }
}
