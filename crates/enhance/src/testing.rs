//! Synthetic SPAN models with deterministic pseudo-random weights, for tests (here and in the
//! crates that run the whole feature). Not for production use.

use std::collections::HashMap;

use candle_core::{Device, Result, Tensor};

use crate::Span;

/// Deterministic values in [-1, 1) (xorshift).
pub fn noise(n: usize, seed: u64) -> Vec<f32> {
    let mut s = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..n)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            ((s >> 40) as f32 / (1u64 << 23) as f32) - 1.0
        })
        .collect()
}

pub fn rnd(shape: &[usize], seed: u64, gain: f32) -> Result<Tensor> {
    let n: usize = shape.iter().product();
    let fan_in: usize = shape.iter().skip(1).product::<usize>().max(1);
    let k = gain * (3.0 / fan_in as f32).sqrt();
    Tensor::from_vec(noise(n, seed).into_iter().map(|v| v * k).collect::<Vec<_>>(), shape, &Device::Cpu)
}

/// All the tensors of a `Conv3XC` ci → co under `prefix` (training branches and a fused copy
/// that is deliberately wrong, to prove the loader doesn't use it).
pub fn conv3xc(t: &mut HashMap<String, Tensor>, prefix: &str, ci: usize, co: usize, seed: u64) -> Result<()> {
    let mut put = |name: &str, shape: &[usize], gain: f32| -> Result<()> {
        let s = seed.wrapping_mul(131).wrapping_add(t.len() as u64);
        t.insert(format!("{prefix}.{name}"), rnd(shape, s, gain)?);
        Ok(())
    };
    put("sk.weight", &[co, ci, 1, 1], 0.5)?;
    put("sk.bias", &[co], 0.1)?;
    put("conv.0.weight", &[2 * ci, ci, 1, 1], 0.7)?;
    put("conv.0.bias", &[2 * ci], 0.1)?;
    put("conv.1.weight", &[2 * co, 2 * ci, 3, 3], 0.7)?;
    put("conv.1.bias", &[2 * co], 0.1)?;
    put("conv.2.weight", &[co, 2 * co, 1, 1], 0.7)?;
    put("conv.2.bias", &[co], 0.1)?;
    put("eval_conv.weight", &[co, ci, 3, 3], 9.0)?;
    put("eval_conv.bias", &[co], 9.0)
}

/// A complete model file's tensors: `c` features, `scale`×, the upsampler scaled by `out_gain`.
pub fn tensors(c: usize, scale: usize, seed: u64, out_gain: f32) -> Result<HashMap<String, Tensor>> {
    let mut t = HashMap::new();
    conv3xc(&mut t, "conv_1", 3, c, seed)?;
    for b in 1..=6 {
        for k in 1..=3 {
            conv3xc(&mut t, &format!("block_{b}.c{k}_r"), c, c, seed + b * 10 + k)?;
        }
    }
    conv3xc(&mut t, "conv_2", c, c, seed + 99)?;
    t.insert("conv_cat.weight".into(), rnd(&[c, 4 * c, 1, 1], seed + 100, 0.8)?);
    t.insert("conv_cat.bias".into(), rnd(&[c], seed + 101, 0.1)?);
    t.insert("upsampler.0.weight".into(), rnd(&[3 * scale * scale, c, 3, 3], seed + 102, out_gain)?);
    t.insert("upsampler.0.bias".into(), Tensor::from_vec(vec![0.5f32; 3 * scale * scale], 3 * scale * scale, &Device::Cpu)?);
    Ok(t)
}

/// The tensors of a model whose output sits well inside 0…1 for any input (so clamping can't
/// hide differences): the upsampler's gain is lowered until a probe image stays in range.
pub fn tamed(c: usize, scale: usize, seed: u64) -> Result<HashMap<String, Tensor>> {
    let mut gain = 1.0f32;
    for _ in 0..40 {
        let t = tensors(c, scale, seed, gain)?;
        let span = Span::from_tensors(&t, &Device::Cpu).map_err(|e| candle_core::Error::Msg(e.to_string()))?;
        let probe = noise(3 * 24 * 24, 7).into_iter().map(|v| v * 0.5 + 0.5).collect::<Vec<_>>();
        let x = Tensor::from_vec(probe, (1, 3, 24, 24), &Device::Cpu)?;
        let out = span.forward(&x).map_err(|e| candle_core::Error::Msg(e.to_string()))?.flatten_all()?.to_vec1::<f32>()?;
        let spread = out.iter().fold(0f32, |m, v| m.max((v - 0.5).abs()));
        if spread > 0.002 && spread < 0.3 {
            return Ok(t);
        }
        gain *= if spread > 0.0 { 0.5 * 0.3 / spread } else { 2.0 };
    }
    tensors(c, scale, seed, 0.01)
}

/// Write a small working model file (not a good one: random weights) to `path`, for tests of
/// code that runs the whole feature without the real download.
pub fn write_synthetic_model(path: &std::path::Path, features: usize, scale: usize) -> std::result::Result<(), String> {
    let t = tamed(features, scale, 1).map_err(|e| e.to_string())?;
    candle_core::safetensors::save(&t, path).map_err(|e| e.to_string())
}
