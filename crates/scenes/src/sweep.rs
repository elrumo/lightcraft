//! The slider sweep's test image: one picture every develop control has something to bite on.

use lightcraft_raster::Rgb32f;

/// A synthetic scene (linear Rec.2020, `w` × `h`, at least 32 × 32) built so that every develop slider has
/// something to change: in the top half a hue/saturation sweep with a lightness ramp (colour mixer, grading,
/// vibrance, saturation, calibration), below it a neutral grey ramp reaching 4.0 with a hard-edged chart (tone,
/// sharpening, clarity), then a dim textured ground with deterministic noise (noise reduction, grain, sharpening
/// masking), and on the right a bright, hazy edge with a purple and a green fringe (highlights, defringe,
/// dehaze). Deterministic: the same size always gives the same pixels.
pub fn sweep_scene(w: usize, h: usize) -> Rgb32f {
    let (w, h) = (w.max(32), h.max(32));
    Rgb32f::from_fn(w, h, |x, y| {
        let (fx, fy) = (x as f32 / (w - 1) as f32, y as f32 / (h - 1) as f32);
        let noise = |k: u32| {
            let mut n = (x as u32).wrapping_mul(73856093) ^ (y as u32).wrapping_mul(19349663) ^ k.wrapping_mul(83492791);
            n ^= n >> 13;
            n = n.wrapping_mul(0x5bd1e995);
            n ^= n >> 15;
            (n & 0xffff) as f32 / 65535.0 - 0.5
        };
        let px = if fy < 0.5 {
            // hue sweep, saturation rising down, lightness ramp across
            let hue = fx * 6.0;
            let sat = 0.25 + 0.7 * (fy / 0.5);
            let l = 0.1 + 0.5 * fx.powf(0.8);
            let c = |o: f32| ((hue + o).rem_euclid(6.0) - 3.0).abs().clamp(1.0, 2.0) - 1.0;
            [c(0.0), c(4.0), c(2.0)].map(|v| l * (1.0 - sat + sat * v) * 1.6)
        } else if fy < 0.75 {
            // grey ramp to 4.0, with a hard-edged chart every 1/10 of the width
            let v = 0.004 * 1000.0f32.powf(fx);
            [if ((fx * 10.0) as usize).is_multiple_of(2) { v } else { v * 0.6 }; 3]
        } else if fx > 0.75 {
            // a bright edge with a purple and a green fringe
            let d = (fx - 0.75) * w as f32;
            if d < 3.0 {
                [2.2, 0.6, 2.6]
            } else if d < 6.0 {
                [0.9, 2.4, 1.0]
            } else {
                [3.0; 3]
            }
        } else {
            // dim textured ground with noise
            let g = 0.12 + 0.05 * ((x as f32 * 0.9).sin() * (y as f32 * 0.7).cos());
            [g + 0.03 * noise(1), g * 0.9 + 0.03 * noise(2), g * 0.8 + 0.03 * noise(3)]
        };
        px.map(|v| v.max(0.0))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sweep_scene_is_deterministic_finite_and_wide_in_range() {
        let a = sweep_scene(96, 64);
        assert_eq!(a, sweep_scene(96, 64));
        assert!(a.data.iter().all(|p| p.iter().all(|v| v.is_finite() && *v >= 0.0)));
        let max = a.data.iter().map(|p| p[1]).fold(0.0f32, f32::max);
        assert!(max >= 3.0, "highlights above SDR white: {max}");
        assert_eq!((sweep_scene(0, 0).width, sweep_scene(0, 0).height), (32, 32), "a degenerate size is raised, not a panic");
    }
}
