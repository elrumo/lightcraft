//! **Stable Diffusion 1.5 Inpainting** and **SDXL Inpainting** (Rombach et al., "High-Resolution
//! Image Synthesis with Latent Diffusion Models", CVPR 2022; Podell et al., "SDXL", 2023).
//!
//! An inpainting UNet reads nine channels: the noisy latents, the hole at the latents' size, and
//! the latents of the picture with the hole made grey. Generate starts from pure noise (seeded)
//! and takes [`Recipe::steps`] DPM-Solver++ steps, then the VAE decodes the latents. No prompt comes from
//! the user, as with Lightroom's Generative Expand: the text the model is conditioned on is fixed
//! ([`Recipe`]), encoded once when the model loads, and the text encoders are dropped again.
//!
//! On the GPU the networks run in 16-bit, except SDXL's VAE, which overflows there and runs in
//! 32-bit (it is small). On the CPU everything is 32-bit.

use std::path::{Path, PathBuf};

use candle_core::{D, DType, Tensor};
use candle_transformers::models::stable_diffusion::vae::{AutoEncoderKL, AutoEncoderKLConfig};
use lightcraft_erase::expand::{Monitor, frame_for};
use lightcraft_erase::{Frame, Painter};
use lightcraft_raster::Rgb32f;

use crate::clip;
use crate::models::{self, MERGES, Spec, TEXT_FP16, UNET_FP16, VOCAB};
use crate::noise::gaussian;
use crate::schedule::{Betas, Dpm};
use crate::tokenizer::{LENGTH, Pad, Tokenizer};
use crate::unet::{Config, Unet};
use crate::weights::{Weights, verify};
use crate::{Error, Result, Target};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Sd15,
    Sdxl,
}

impl Kind {
    pub fn spec(self) -> &'static Spec {
        match self {
            Kind::Sd15 => &models::SD15,
            Kind::Sdxl => &models::SDXL,
        }
    }

    /// The side of the square the model was trained on (its canvases have about as many pixels).
    pub fn native(self) -> usize {
        match self {
            Kind::Sd15 => 512,
            Kind::Sdxl => 1024,
        }
    }

    fn vae_scale(self) -> f64 {
        match self {
            Kind::Sd15 => 0.18215,
            Kind::Sdxl => 0.13025,
        }
    }

    fn unet(self) -> Config {
        match self {
            Kind::Sd15 => Config::sd15_inpaint(),
            Kind::Sdxl => Config::sdxl_inpaint(),
        }
    }
}

/// How the model is asked to paint.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Recipe {
    pub steps: usize,
    /// Classifier-free guidance: how strongly the answer follows the text over the negative text
    /// (1: one network pass per step, no negative text).
    pub guidance: f64,
    pub prompt: &'static str,
    pub negative: &'static str,
    pub seed: u64,
}

impl Recipe {
    /// What Generative Expand uses for `kind`: 20 steps on a GPU, 15 on a CPU (where each takes
    /// many seconds).
    pub const fn for_kind(kind: Kind, gpu: bool) -> Recipe {
        let steps = if gpu { 20 } else { 15 };
        match kind {
            Kind::Sd15 | Kind::Sdxl => Recipe { steps, guidance: 4.0, prompt: PROMPT, negative: NEGATIVE, seed: SEED },
        }
    }
}

/// The text the new area is drawn after: nothing but "the rest of this photograph".
const PROMPT: &str = "";
/// What it must not become (an outpainted edge's usual failures).
const NEGATIVE: &str =
    "frame, border, picture frame, text, caption, watermark, signature, logo, collage, split screen, blurry, lowres, jpeg artifacts";
const SEED: u64 = 0x4c43_4558_5041_4e44;

/// Stable Diffusion's VAE (both models).
fn vae_config() -> AutoEncoderKLConfig {
    AutoEncoderKLConfig {
        block_out_channels: vec![128, 256, 512, 512],
        layers_per_block: 2,
        latent_channels: 4,
        norm_num_groups: 32,
        use_quant_conv: true,
        use_post_quant_conv: true,
    }
}

/// `dir/part/name`, checked against its pin.
pub(crate) fn pinned(dir: &Path, spec: &Spec, part: &str, name: &str) -> Result<PathBuf> {
    let f = spec.file(part, name).ok_or_else(|| Error::Model(format!("{} has no {part}/{name}", spec.label)))?;
    let p = dir.join(part).join(name);
    let (Some(size), Some(sha)) = (f.size, f.sha256) else { return Err(Error::Model(format!("{part}/{name} isn't pinned"))) };
    verify(&p, size, sha).map_err(|e| match e {
        Error::Model(m) if m.ends_with("is missing") => Error::Model(format!("{} isn't downloaded ({part}/{name} is missing)", spec.label)),
        e => e,
    })?;
    Ok(p)
}

fn tokenizer(dir: &Path, spec: &Spec, part: &str, pad: Pad) -> Result<Tokenizer> {
    Tokenizer::load(&pinned(dir, spec, part, VOCAB)?, &pinned(dir, spec, part, MERGES)?, pad)
}

/// The text conditioning: `(context, pooled)`, `[negative, prompt]` with guidance, `[prompt]`
/// without.
fn encode(kind: Kind, dir: &Path, target: &Target, r: &Recipe) -> Result<(Tensor, Option<Tensor>)> {
    let spec = kind.spec();
    let guided = r.guidance > 1.0;
    match kind {
        Kind::Sd15 => {
            let tok = tokenizer(dir, spec, "tokenizer", Pad::Eos)?;
            let w = Weights::open(&pinned(dir, spec, "text_encoder", TEXT_FP16)?)?;
            let run = |text: &str| -> Result<Tensor> {
                clip::text(&w, &clip::TEXT_L, &tok.encode(text), target, true)?.last.ok_or_else(|| Error::Model("no text embedding".into()))
            };
            let prompt = run(r.prompt)?;
            Ok((if guided { Tensor::cat(&[&run(r.negative)?, &prompt], 0)? } else { prompt }, None))
        }
        Kind::Sdxl => {
            let (t1, t2) = (tokenizer(dir, spec, "tokenizer", Pad::Eos)?, tokenizer(dir, spec, "tokenizer_2", Pad::Bang)?);
            let w1 = Weights::open(&pinned(dir, spec, "text_encoder", TEXT_FP16)?)?;
            let w2 = Weights::open(&pinned(dir, spec, "text_encoder_2", TEXT_FP16)?)?;
            let run = |text: &str| -> Result<(Tensor, Tensor)> {
                let ids2 = t2.encode(text);
                let a = clip::text(&w1, &clip::TEXT_L, &t1.encode(text), target, false)?;
                let b = clip::text(&w2, &clip::TEXT_BIG_G, &ids2, target, true)?;
                let end = ids2.iter().position(|&i| i == t2.eos()).unwrap_or(LENGTH - 1);
                let last = b.last.ok_or_else(|| Error::Model("no text embedding".into()))?;
                let pooled = clip::pooled(&w2, &clip::TEXT_BIG_G, &last, end, target)?;
                Ok((Tensor::cat(&[&a.penultimate, &b.penultimate], D::Minus1)?, pooled))
            };
            let (c, p) = run(r.prompt)?;
            if !guided {
                return Ok((c, Some(p)));
            }
            // (an empty negative text is zeros, as SDXL was trained: force_zeros_for_empty_prompt)
            let (nc, np) = if r.negative.is_empty() { (c.zeros_like()?, p.zeros_like()?) } else { run(r.negative)? };
            Ok((Tensor::cat(&[&nc, &c], 0)?, Some(Tensor::cat(&[&np, &p], 0)?)))
        }
    }
}

/// Stable Diffusion 1.5 or SDXL inpainting, loaded.
pub struct StableDiffusion {
    kind: Kind,
    target: Target,
    unet: Unet,
    vae: AutoEncoderKL,
    vae_dtype: DType,
    context: Tensor,
    pooled: Option<Tensor>,
    recipe: Recipe,
}

impl StableDiffusion {
    /// Load `kind` from its folder `dir` (`unet/`, `vae/`, …, as [`Kind::spec`] lists): the files
    /// are checked against their pins, the text is encoded, then the VAE and the UNet are read.
    /// `mon` hears the fraction loaded and is asked between the parts whether to stop.
    pub fn load(kind: Kind, dir: &Path, target: Target, recipe: Recipe, mon: &Monitor<'_>) -> Result<StableDiffusion> {
        let spec = kind.spec();
        let stop = || if (mon.cancelled)() { Err(Error::Cancelled) } else { Ok(()) };
        let unet_file = pinned(dir, spec, "unet", UNET_FP16)?;
        let vae_file = pinned(dir, spec, "vae", UNET_FP16)?;
        (mon.progress)(0.15);
        stop()?;
        let (context, pooled) = encode(kind, dir, &target, &recipe)?;
        (mon.progress)(0.3);
        stop()?;
        let vae_dtype = if kind == Kind::Sdxl { DType::F32 } else { target.dtype };
        let vae = AutoEncoderKL::new(Weights::open(&vae_file)?.var_builder(vae_dtype, &target.device)?, 3, 3, vae_config())?;
        (mon.progress)(0.4);
        stop()?;
        let mut cfg = kind.unet();
        if target.sliced_attention {
            cfg.sliced_attention_size = Some(2);
        }
        let unet = Unet::new(Weights::open(&unet_file)?.var_builder(target.dtype, &target.device)?, &cfg)?;
        (mon.progress)(1.0);
        Ok(StableDiffusion { kind, target, unet, vae, vae_dtype, context, pooled, recipe })
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// SDXL's added conditioning for a `w × h` canvas: the pooled text and the size ids
    /// (original size, crop corner, target size).
    fn added(&self, w: usize, h: usize) -> Result<Option<(Tensor, Tensor)>> {
        let Some(p) = &self.pooled else { return Ok(None) };
        let rows = p.dim(0)?;
        let (h, w) = (h as f32, w as f32);
        let ids: Vec<f32> = (0..rows).flat_map(|_| [h, w, 0.0, 0.0, h, w]).collect();
        Ok(Some((p.clone(), Tensor::from_vec(ids, (rows, 6), &self.target.device)?)))
    }

    /// `img` (`w × h`, multiples of 8, display-encoded 0..=1) with the pixels where `hole` is set
    /// painted.
    pub fn inpaint(&self, img: &Rgb32f, hole: &[bool], mon: &Monitor<'_>) -> Result<Rgb32f> {
        let (w, h) = (img.width, img.height);
        let n = w.checked_mul(h).unwrap_or(0);
        if n == 0
            || w % 8 != 0
            || h % 8 != 0
            || w > lightcraft_erase::expand::MAX_CANVAS
            || h > lightcraft_erase::expand::MAX_CANVAS
            || img.data.len() != n
            || hole.len() != n
        {
            return Err(Error::Image(format!("{} can't paint a {w} × {h} canvas", self.kind.spec().label)));
        }
        let dev = &self.target.device;
        let dt = self.unet.dtype();
        // the picture in -1..=1 with the hole grey, as the model was trained
        let mut px = vec![0f32; 3 * n];
        for (i, (p, &k)) in img.data.iter().zip(hole).enumerate() {
            for (c, v) in p.iter().enumerate() {
                if let Some(o) = px.get_mut(c * n + i) {
                    *o = if k { 0.0 } else { v.clamp(0.0, 1.0) * 2.0 - 1.0 };
                }
            }
        }
        let masked = Tensor::from_vec(px, (1, 3, h, w), dev)?.to_dtype(self.vae_dtype)?;
        let masked = (self.vae.encode(&masked)?.sample()? * self.kind.vae_scale())?.to_dtype(dt)?;
        // the hole at the latents' size: a latent is new when any of its 8 × 8 pixels is
        let (lw, lh) = (w / 8, h / 8);
        let m: Vec<f32> = (0..lw * lh)
            .map(|i| {
                let (x, y) = (i % lw * 8, i / lw * 8);
                let any = (0..8).any(|dy| (0..8).any(|dx| hole.get((y + dy) * w + x + dx).copied().unwrap_or(false)));
                if any { 1.0 } else { 0.0 }
            })
            .collect();
        let mask = Tensor::from_vec(m, (1, 1, lh, lw), dev)?.to_dtype(dt)?;
        let r = &self.recipe;
        let mut sched = Dpm::new(r.steps, 0.00085, 0.012, Betas::ScaledLinear);
        let mut latents = Tensor::from_vec(gaussian(4 * lw * lh, r.seed), (1, 4, lh, lw), dev)?.to_dtype(dt)?;
        let guided = r.guidance > 1.0;
        let added = self.added(w, h)?;
        let ts = sched.timesteps().to_vec();
        let steps = ts.len().max(1);
        for (i, &t) in ts.iter().enumerate() {
            if (mon.cancelled)() {
                return Err(Error::Cancelled);
            }
            let x = Tensor::cat(&[&latents, &mask, &masked], 1)?;
            let x = if guided { Tensor::cat(&[&x, &x], 0)? } else { x };
            let eps = self.unet.forward(&x, t as f64, &self.context, added.as_ref().map(|(p, ids)| (p, ids)))?;
            let eps = if guided {
                let (u, c) = (eps.narrow(0, 0, 1)?, eps.narrow(0, 1, 1)?);
                (&u + ((&c - &u)? * r.guidance)?)?
            } else {
                eps
            };
            latents = sched.step(i, &eps, &latents)?;
            (mon.progress)(0.95 * (i + 1) as f32 / steps as f32);
        }
        if (mon.cancelled)() {
            return Err(Error::Cancelled);
        }
        let out = self.vae.decode(&(latents.to_dtype(self.vae_dtype)? / self.kind.vae_scale())?)?;
        let v = ((out.to_dtype(DType::F32)?.clamp(-1.0, 1.0)? + 1.0)? * 0.5)?.flatten_all()?.to_vec1::<f32>()?;
        if v.len() != 3 * n {
            return Err(Error::Model(format!("the VAE answered {} numbers, not {}", v.len(), 3 * n)));
        }
        let painted = Rgb32f::from_fn(w, h, |x, y| {
            let i = y * w + x;
            [0, 1, 2].map(|c| v.get(c * n + i).copied().filter(|u| u.is_finite()).unwrap_or(0.5))
        });
        (mon.progress)(1.0);
        Ok(crate::seam::match_edges(img, &painted, hole))
    }
}

impl Painter for StableDiffusion {
    fn paint(&self, img: &Rgb32f, hole: &[bool]) -> lightcraft_erase::Result<Rgb32f> {
        self.inpaint(img, hole, &Monitor::NONE).map_err(|e| e.into_erase(self.kind.spec().label))
    }

    fn label(&self) -> &'static str {
        self.kind.spec().label
    }

    fn frame(&self, w: usize, h: usize) -> Frame {
        frame_for(w, h, self.kind.native(), 64, 2.0)
    }

    fn paint_watched(&self, img: &Rgb32f, hole: &[bool], mon: &Monitor<'_>) -> lightcraft_erase::Result<Rgb32f> {
        self.inpaint(img, hole, mon).map_err(|e| e.into_erase(self.kind.spec().label))
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

    /// The model folder and the reference outputs (`docs/generative-expand.md` → How it was checked),
    /// when given (`$LIGHTCRAFT_SD15`, `$LIGHTCRAFT_SD15_REF`): never part of the repository.
    fn reference() -> Option<(PathBuf, HashMap<String, Tensor>)> {
        let dir = PathBuf::from(std::env::var_os("LIGHTCRAFT_SD15")?);
        let r = candle_core::safetensors::load(std::env::var_os("LIGHTCRAFT_SD15_REF")?, &Device::Cpu).unwrap();
        Some((dir, r))
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

    fn ids(t: &Tensor) -> Vec<u32> {
        t.flatten_all().unwrap().to_vec1::<i64>().unwrap().into_iter().map(|i| i as u32).collect()
    }

    #[test]
    fn matches_diffusers() {
        let Some((dir, r)) = reference() else { return };
        let spec = Kind::Sd15.spec();
        let target = Target::cpu();
        // tokens
        let tok = tokenizer(&dir, spec, "tokenizer", Pad::Eos).unwrap();
        assert_eq!(tok.encode(""), ids(&r["ids_empty"]));
        assert_eq!(tok.encode(NEGATIVE), ids(&r["ids_neg"]));
        assert_eq!(tok.encode("a photo of a cat, 12 dogs!"), ids(&r["ids_cat"]));
        // text
        let w = Weights::open(&pinned(&dir, spec, "text_encoder", TEXT_FP16).unwrap()).unwrap();
        let neg = clip::text(&w, &clip::TEXT_L, &tok.encode(NEGATIVE), &target, true).unwrap().last.unwrap();
        let d = max_diff(&neg, &r["emb_neg"]);
        assert!(d < 1e-3, "text: {d}");
        // one UNet step
        let unet = Unet::new(
            Weights::open(&pinned(&dir, spec, "unet", UNET_FP16).unwrap()).unwrap().var_builder(DType::F32, &Device::Cpu).unwrap(),
            &Config::sd15_inpaint(),
        )
        .unwrap();
        let eps = unet.forward(&r["unet_x"], 500.0, &neg, None).unwrap();
        let d = max_diff(&eps, &r["unet_eps"]);
        assert!(d < 2e-3, "unet: {d}");
        drop(unet);
        // the VAE
        let vae = AutoEncoderKL::new(
            Weights::open(&pinned(&dir, spec, "vae", UNET_FP16).unwrap()).unwrap().var_builder(DType::F32, &Device::Cpu).unwrap(),
            3,
            3,
            vae_config(),
        )
        .unwrap();
        let d = max_diff(&vae.decode(&r["vae_lat"]).unwrap(), &r["vae_dec"]);
        assert!(d < 2e-3, "vae decode: {d}");
        let d = max_diff(&vae.encode(&r["vae_img"]).unwrap().sample().unwrap(), &r["vae_enc_mean"]);
        assert!(d < 0.05, "vae encode: {d}");
        eprintln!("SD 1.5 matches diffusers");
    }

    /// SDXL's two text encoders (the penultimate layers, the pooled embedding) and one UNet step
    /// with its added conditioning, against diffusers (`$LIGHTCRAFT_SDXL`, `$LIGHTCRAFT_SDXL_REF`).
    #[test]
    fn sdxl_matches_diffusers() {
        let (Some(dir), Some(rf)) = (std::env::var_os("LIGHTCRAFT_SDXL"), std::env::var_os("LIGHTCRAFT_SDXL_REF")) else { return };
        let (dir, r) = (PathBuf::from(dir), candle_core::safetensors::load(rf, &Device::Cpu).unwrap());
        let spec = Kind::Sdxl.spec();
        let target = Target::cpu();
        let (t1, t2) = (tokenizer(&dir, spec, "tokenizer", Pad::Eos).unwrap(), tokenizer(&dir, spec, "tokenizer_2", Pad::Bang).unwrap());
        let (i1, i2) = (t1.encode(NEGATIVE), t2.encode(NEGATIVE));
        assert_eq!(i1, ids(&r["ids1"]));
        assert_eq!(i2, ids(&r["ids2"]));
        let w1 = Weights::open(&pinned(&dir, spec, "text_encoder", TEXT_FP16).unwrap()).unwrap();
        let d = max_diff(&clip::text(&w1, &clip::TEXT_L, &i1, &target, false).unwrap().penultimate, &r["h1"]);
        eprintln!("text 1: {d}");
        assert!(d < 1e-3, "text 1: {d}");
        let w2 = Weights::open(&pinned(&dir, spec, "text_encoder_2", TEXT_FP16).unwrap()).unwrap();
        let b = clip::text(&w2, &clip::TEXT_BIG_G, &i2, &target, true).unwrap();
        let d = max_diff(&b.penultimate, &r["h2"]);
        eprintln!("text 2: {d}");
        assert!(d < 2e-3, "text 2: {d}");
        let end = i2.iter().position(|&i| i == t2.eos()).unwrap();
        let d = max_diff(&clip::pooled(&w2, &clip::TEXT_BIG_G, &b.last.unwrap(), end, &target).unwrap(), &r["pooled"]);
        eprintln!("pooled: {d}");
        assert!(d < 2e-3, "pooled: {d}");
        let unet = Unet::new(
            Weights::open(&pinned(&dir, spec, "unet", UNET_FP16).unwrap()).unwrap().var_builder(DType::F32, &Device::Cpu).unwrap(),
            &Config::sdxl_inpaint(),
        )
        .unwrap();
        let eps = unet.forward(&r["unet_x"], 500.0, &r["unet_ctx"], Some((&r["unet_pooled"], &r["unet_ids"]))).unwrap();
        let d = max_diff(&eps, &r["unet_eps"]);
        eprintln!("unet: {d}");
        assert!(d < 3e-3, "unet: {d}");
    }
}
