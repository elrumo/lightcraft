//! The starting noise: Gaussian numbers from a fixed seed (SplitMix64 and Box–Muller), so the same
//! canvas gives the same answer on every run and device.

/// `n` standard normal numbers from `seed`.
pub fn gaussian(n: usize, seed: u64) -> Vec<f32> {
    let mut s = seed ^ 0x9e37_79b9_7f4a_7c15;
    let mut next = || {
        s = s.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = s;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    };
    // (0, 1]: never 0, so the logarithm is finite
    let mut unit = || ((next() >> 11) as f64 + 1.0) / (1u64 << 53) as f64;
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        let (u, v) = (unit(), unit());
        let r = (-2.0 * u.ln()).sqrt();
        let t = std::f64::consts::TAU * v;
        out.push((r * t.cos()) as f32);
        if out.len() < n {
            out.push((r * t.sin()) as f32);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_standard() {
        let a = gaussian(100_001, 7);
        assert_eq!(a, gaussian(100_001, 7));
        assert_ne!(a[..10], gaussian(10, 8)[..]);
        let mean = a.iter().map(|&v| v as f64).sum::<f64>() / a.len() as f64;
        let var = a.iter().map(|&v| (v as f64 - mean).powi(2)).sum::<f64>() / a.len() as f64;
        assert!(mean.abs() < 0.01 && (var - 1.0).abs() < 0.02, "{mean} {var}");
        assert!(a.iter().all(|v| v.is_finite()));
        assert!(gaussian(0, 1).is_empty());
    }
}
