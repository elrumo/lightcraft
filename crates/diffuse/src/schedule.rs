//! The denoising schedules, with the training noise levels of each model:
//!
//! [`Dpm`]: DPM-Solver++ (Lu et al., 2022), the second-order multistep solver on the data
//! prediction, "linspace" timestep spacing and a last step to zero noise, as diffusers'
//! `DPMSolverMultistepScheduler` (`dpmsolver++`, order 2, midpoint). It reaches in 15–20 steps
//! about what DDIM (Song et al., 2020) needs 25–30 for, which on a CPU is minutes saved.

use candle_core::Tensor;

use crate::Result;

/// How the training noise grew.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Betas {
    /// Stable Diffusion: β linear in its square root.
    ScaledLinear,
    /// Kandinsky 2.2: β linear.
    Linear,
}

const TRAIN: usize = 1000;

/// The training noise levels ᾱ of a model trained with β from `start` to `end`.
fn alphas_cumprod(start: f64, end: f64, betas: Betas) -> Vec<f64> {
    let n = TRAIN as f64 - 1.0;
    let beta = |i: usize| match betas {
        Betas::ScaledLinear => {
            let (a, b) = (start.sqrt(), end.sqrt());
            (a + (b - a) * i as f64 / n).powi(2)
        }
        Betas::Linear => start + (end - start) * i as f64 / n,
    };
    let mut acc = 1.0;
    (0..TRAIN)
        .map(|i| {
            acc *= 1.0 - beta(i);
            acc
        })
        .collect()
}

/// DPM-Solver++ (2M): `steps` steps (1..=1000).
#[derive(Clone, Debug)]
pub struct Dpm {
    alphas_cumprod: Vec<f64>,
    timesteps: Vec<usize>,
    /// The last step's data prediction and its λ, for the second order.
    last: Option<(Tensor, f64)>,
}

impl Dpm {
    pub fn new(steps: usize, start: f64, end: f64, betas: Betas) -> Dpm {
        let steps = steps.clamp(1, TRAIN);
        let last = (TRAIN - 1) as f64;
        // linspace(0, 999, steps + 1), rounded, noisiest first, without the 0
        let timesteps = (0..=steps).rev().map(|i| (last * i as f64 / steps as f64).round() as usize).take(steps).collect();
        Dpm { alphas_cumprod: alphas_cumprod(start, end, betas), timesteps, last: None }
    }

    pub fn timesteps(&self) -> &[usize] {
        &self.timesteps
    }

    /// `(α, σ, λ)` at timestep `t`: √ᾱ, √(1 − ᾱ), log(α / σ).
    fn at(&self, t: usize) -> (f64, f64, f64) {
        let a = self.alphas_cumprod.get(t).copied().unwrap_or(1.0).clamp(1e-12, 1.0 - 1e-12);
        let (alpha, sigma) = (a.sqrt(), (1.0 - a).sqrt());
        (alpha, sigma, alpha.ln() - sigma.ln())
    }

    /// `x0` noised to timestep `t` with `noise`.
    pub fn add_noise(&self, x0: &Tensor, noise: &Tensor, t: usize) -> Result<Tensor> {
        let (alpha, sigma, _) = self.at(t);
        Ok(((x0 * alpha)? + (noise * sigma)?)?)
    }

    /// Step `i` (from `timesteps()[i]`): the next sample, given the model's predicted noise `eps`
    /// for `x`. The last step goes to zero noise (the clean sample).
    pub fn step(&mut self, i: usize, eps: &Tensor, x: &Tensor) -> Result<Tensor> {
        let t = self.timesteps.get(i).copied().unwrap_or(0);
        let (alpha_t, sigma_t, lambda_t) = self.at(t);
        let x0 = ((x - (eps * sigma_t)?)? / alpha_t)?;
        let next = self.timesteps.get(i + 1).copied();
        let out = match next {
            // to zero noise: first order there (diffusers' `final_sigmas_type = "zero"`), which is
            // the data prediction itself
            None => x0.clone(),
            Some(s) => {
                let (alpha_s, sigma_s, lambda_s) = self.at(s);
                let h = lambda_s - lambda_t;
                let k = (-h).exp() - 1.0;
                let first = ((x * (sigma_s / sigma_t))? - (&x0 * (alpha_s * k))?)?;
                match &self.last {
                    Some((prev, lambda_prev)) if i > 0 => {
                        let r = (lambda_t - lambda_prev) / h;
                        let d1 = ((&x0 - prev)? / r)?;
                        (first - (d1 * (0.5 * alpha_s * k))?)?
                    }
                    _ => first,
                }
            }
        };
        self.last = Some((x0, lambda_t));
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use candle_core::Device;

    #[test]
    fn noise_levels_and_timesteps() {
        // diffusers' alphas_cumprod for Stable Diffusion
        let a = alphas_cumprod(0.00085, 0.012, Betas::ScaledLinear);
        assert!((a[0] - 0.99915).abs() < 1e-5);
        assert!((a[999] - 0.0046).abs() < 2e-4, "{}", a[999]);
        assert_eq!(Dpm::new(0, 0.00085, 0.012, Betas::Linear).timesteps(), &[999]);
        assert_eq!(Dpm::new(5000, 0.00085, 0.012, Betas::Linear).timesteps().len(), 1000);
        assert_eq!(*Dpm::new(20, 0.00085, 0.012, Betas::Linear).timesteps().last().unwrap(), 50);
    }

    #[test]
    fn a_perfect_noise_guess_gives_back_the_clean_sample() {
        let mut s = Dpm::new(10, 0.00085, 0.012, Betas::Linear);
        let dev = Device::Cpu;
        let x0 = Tensor::new(&[0.5f32, -0.25], &dev).unwrap();
        let eps = Tensor::new(&[1.0f32, -1.0], &dev).unwrap();
        let mut x = s.add_noise(&x0, &eps, s.timesteps()[0]).unwrap();
        for i in 0..s.timesteps().len() {
            x = s.step(i, &eps, &x).unwrap();
        }
        let got = x.to_vec1::<f32>().unwrap();
        assert!(got.iter().zip([0.5f32, -0.25]).all(|(g, w)| (g - w).abs() < 1e-4), "{got:?}");
    }

    /// The same trajectories as diffusers' `DPMSolverMultistepScheduler` (dpmsolver++, order 2,
    /// linspace, final sigma zero) with a made-up noise model, `eps = x / 2 + t / 100 000`.
    #[test]
    fn dpm_solver_matches_diffusers() {
        for (betas, n, first, want) in [
            (Betas::ScaledLinear, 15, [999, 932, 866, 799], [2.678639163, -1.388212318, 0.645213301]),
            (Betas::ScaledLinear, 8, [999, 874, 749, 624], [2.714141617, -1.408089239, 0.653026504]),
            (Betas::Linear, 20, [999, 949, 899, 849], [3.490265989, -1.811303943, 0.839481139]),
        ] {
            let mut s = Dpm::new(n, 0.00085, 0.012, betas);
            assert_eq!(&s.timesteps()[..4], &first);
            let mut x = Tensor::new(&[1.0f64, -0.5, 0.25], &Device::Cpu).unwrap();
            for i in 0..s.timesteps().len() {
                let t = s.timesteps()[i] as f64;
                let eps = ((&x * 0.5).unwrap() + 0.01 * t / 1000.0).unwrap();
                x = s.step(i, &eps, &x).unwrap();
            }
            let got = x.to_vec1::<f64>().unwrap();
            assert!(got.iter().zip(&want).all(|(g, w)| (g - w).abs() < 1e-6), "{betas:?} {n}: {got:?} vs {want:?}");
        }
    }
}
