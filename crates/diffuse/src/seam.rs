//! Meeting the photo at its edge.
//!
//! A diffusion model repaints the whole canvas, the photo too, and its colours drift a little from
//! the photo's (a sky a shade bluer, a shadow a shade lighter). Kept as is, the new area meets the
//! photo along a visible line. So the difference between the photo and the model's own rendering
//! of it is taken over the photo (blurred, so texture doesn't count, only colour and light), and
//! carried into the new area by membrane (harmonic) interpolation, as Poisson image editing does
//! (Pérez, Gangnet & Blake, 2003): at the edge the new area takes the photo's colour, and the
//! correction fades smoothly into what the model drew.

use lightcraft_raster::blur::gaussian;
use lightcraft_raster::{Plane, Rgb32f};

/// `painted` (the model's answer over the whole canvas) with its new area (`hole`) corrected to
/// meet `input` (the canvas the model was given) at the photo's edge.
pub fn match_edges(input: &Rgb32f, painted: &Rgb32f, hole: &[bool]) -> Rgb32f {
    let (w, h) = (input.width, input.height);
    let n = w.saturating_mul(h);
    if n == 0 || painted.width != w || painted.height != h || hole.len() != n || input.data.len() != n || painted.data.len() != n {
        return painted.clone();
    }
    if !hole.iter().any(|&b| b) || hole.iter().all(|&b| b) {
        return painted.clone();
    }
    // the difference over the photo, blurred within it (normalised: the new area adds nothing)
    let sigma = (w.max(h) as f32 / 128.0).max(2.0);
    let diff = Rgb32f::from_fn(w, h, |x, y| {
        let i = y * w + x;
        match (hole.get(i), input.data.get(i), painted.data.get(i)) {
            (Some(false), Some(a), Some(b)) => [a[0] - b[0], a[1] - b[1], a[2] - b[2]],
            _ => [0.0; 3],
        }
    });
    let weight = Plane { width: w, height: h, data: hole.iter().map(|&b| if b { 0.0 } else { 1.0 }).collect() };
    let (bd, bw) = (gaussian(&diff, sigma), gaussian(&weight, sigma));
    let mut field = vec![0f32; 3 * n];
    for (i, ((d, wt), &new)) in bd.data.iter().zip(&bw.data).zip(hole).enumerate() {
        if !new && *wt > 1e-4 {
            for c in 0..3 {
                if let Some(f) = field.get_mut(i * 3 + c) {
                    *f = d[c] / wt;
                }
            }
        }
    }
    let field = lightcraft_inpaint::membrane_fill(w, h, 3, &field, hole);
    let mut out = painted.clone();
    for (i, (p, &new)) in out.data.iter_mut().zip(hole).enumerate() {
        if new {
            for (c, v) in p.iter_mut().enumerate() {
                *v = (*v + field.get(i * 3 + c).copied().unwrap_or(0.0)).clamp(0.0, 1.0);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_colour_shift_is_taken_out_at_the_edge() {
        // the photo is the left half; the model drew everything 0.1 too blue
        let (w, h) = (64, 32);
        let hole: Vec<bool> = (0..w * h).map(|i| i % w >= 32).collect();
        let input = Rgb32f::from_fn(w, h, |x, _| if x < 32 { [0.5, 0.4, 0.3] } else { [0.0; 3] });
        let painted = Rgb32f::from_fn(w, h, |x, _| [0.5, 0.4, 0.4 + 0.001 * x as f32]);
        let out = match_edges(&input, &painted, &hole);
        // at the edge the new area meets the photo; its own variation is kept
        let edge = out.get(32, 16);
        assert!((edge[2] - 0.3).abs() < 0.02, "{edge:?}");
        let far = out.get(63, 16);
        assert!(far[2] > edge[2], "{far:?} {edge:?}");
        // the photo's own pixels aren't touched here (the caller keeps the photo anyway)
        assert_eq!(out.get(0, 0), painted.get(0, 0));
        // mismatched inputs come back as they were
        assert_eq!(match_edges(&input, &painted, &hole[..10]).data, painted.data);
    }
}
