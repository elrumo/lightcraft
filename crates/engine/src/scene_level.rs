//! How bright a raw's scene-linear picture is, as a handful of luminance quantiles.
//!
//! The base tone curve a raw is shown through ([`lightcraft_pipeline::SourceInfo::camera_tone`]) is fitted against this
//! engine's own scene-linear light, whose scale (sensor white level, white balance multipliers, baseline exposure, the
//! look's gain) is the decoder's. A frontend that develops the raw with another decoder (Apple's `CIRAWFilter`) gets light
//! on a scale of its own, per camera and per photo, and the curve then acts on the wrong operating point. The quantiles
//! let it bring its light to this scale (the same scene, so every quantile differs by the same factor) without knowing how
//! either decoder gets there.
//!
//! The colour grid ([`colour_grid`]) does the same for colour: the mean scene-linear colour of a coarse grid of cells. In
//! scene-linear light both decoders are linear in the camera's RGB, so the mean of a cell maps by the same 3 × 3 matrix as each
//! of its pixels, and a frontend fits that matrix from its own grid of the same picture to bring its decode's colour (its own
//! camera matrix and white) to this engine's.

use lightcraft_color::LUMA_2020;
use lightcraft_raster::Rgb32f;

/// The probabilities of the quantiles: the deciles. The tails are left out: the darkest few percent are sensor noise and the
/// brightest depend on how each decoder rebuilds clipped highlights.
pub const PROBABILITIES: [f32; 9] = [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9];

/// Samples taken from a picture for the quantiles (a regular grid across it).
const SAMPLES: usize = 40_000;

/// Cells of the colour grid along the picture's long edge and its short edge, whichever way the picture is turned: a frontend's
/// grid of the same picture in another orientation then differs from this one only by reversing either axis.
pub const GRID_LONG: usize = 12;
pub const GRID_SHORT: usize = 8;

/// The luminance (Rec. 2020 weights on linear Rec. 2020, so Y) of `img` at [`PROBABILITIES`], from a regular grid of about
/// [`SAMPLES`] pixels. `None` for a picture too small to say (under 64 usable samples) or with no finite pixel.
pub fn luminance_quantiles(img: &Rgb32f) -> Option<[f32; 9]> {
    let (w, h) = (img.width, img.height);
    let pixels = w.checked_mul(h).filter(|n| *n > 0 && *n == img.data.len())?;
    // (a grid with the same step across and down; at least one pixel apart)
    let step = (pixels as f64 / SAMPLES as f64).sqrt().ceil().max(1.0) as usize;
    let mut ys: Vec<f32> = Vec::with_capacity((w / step + 1) * (h / step + 1));
    for y in (0..h).step_by(step) {
        for x in (0..w).step_by(step) {
            let Some(p) = img.data.get(y * w + x) else { continue };
            let luma = LUMA_2020[0] * p[0] + LUMA_2020[1] * p[1] + LUMA_2020[2] * p[2];
            if luma.is_finite() {
                ys.push(luma.max(0.0));
            }
        }
    }
    if ys.len() < 64 {
        return None;
    }
    ys.sort_by(f32::total_cmp);
    let last = (ys.len() - 1) as f32;
    let mut out = [0.0; 9];
    for (slot, p) in out.iter_mut().zip(PROBABILITIES) {
        *slot = ys.get((last * p).round() as usize).copied().unwrap_or(0.0);
    }
    Some(out)
}

/// The mean colour (linear Rec. 2020) of each cell of a [`GRID_LONG`] × [`GRID_SHORT`] grid laid along `img`'s long and short
/// edges, short index major (`cells[v * GRID_LONG + u]`, `u` along the long edge from the left or the top), from about
/// [`SAMPLES`] pixels. A cell with no finite sample is `None`. `None` for a picture too small for one sample per cell.
pub fn colour_grid(img: &Rgb32f) -> Option<Vec<Option<[f32; 3]>>> {
    let (w, h) = (img.width, img.height);
    let pixels = w.checked_mul(h).filter(|n| *n > 0 && *n == img.data.len())?;
    let (long, short) = (w.max(h), w.min(h));
    if long < GRID_LONG || short < GRID_SHORT {
        return None;
    }
    let landscape = w >= h;
    let step = (pixels as f64 / SAMPLES as f64).sqrt().ceil().max(1.0) as usize;
    let mut sums = vec![([0.0f64; 3], 0usize); GRID_LONG * GRID_SHORT];
    for y in (0..h).step_by(step) {
        for x in (0..w).step_by(step) {
            let Some(p) = img.data.get(y * w + x) else { continue };
            if !p.iter().all(|c| c.is_finite()) {
                continue;
            }
            let (a, b) = if landscape { (x, y) } else { (y, x) };
            let (u, v) = ((a * GRID_LONG / long).min(GRID_LONG - 1), (b * GRID_SHORT / short).min(GRID_SHORT - 1));
            if let Some((sum, n)) = sums.get_mut(v * GRID_LONG + u) {
                for (s, c) in sum.iter_mut().zip(p) {
                    *s += f64::from(*c);
                }
                *n += 1;
            }
        }
    }
    Some(sums.into_iter().map(|(sum, n)| (n > 0).then(|| sum.map(|s| (s / n as f64) as f32))).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture(w: usize, h: usize, f: impl Fn(usize, usize) -> [f32; 3]) -> Rgb32f {
        Rgb32f { width: w, height: h, data: (0..w * h).map(|i| f(i % w, i / w)).collect() }
    }

    /// A ramp of luminance across the picture: the quantiles are the ramp's values at the probabilities.
    #[test]
    fn a_ramp_has_its_values_at_the_probabilities() {
        let img = picture(400, 300, |x, _| {
            let v = x as f32 / 399.0;
            [v, v, v]
        });
        let q = luminance_quantiles(&img).unwrap();
        for (q, p) in q.iter().zip(PROBABILITIES) {
            assert!((q - p).abs() < 0.01, "{q} at {p}");
        }
    }

    /// The same scene lit twice as bright has every quantile doubled: the property a frontend relies on.
    #[test]
    fn a_brighter_scene_scales_every_quantile() {
        let at = |gain: f32| {
            luminance_quantiles(&picture(333, 217, |x, y| {
                let v = 0.02 + ((x * 7 + y * 13) % 211) as f32 / 400.0;
                [v * gain, v * gain * 0.8, v * gain * 0.6]
            }))
            .unwrap()
        };
        let (a, b) = (at(1.0), at(2.0));
        for (a, b) in a.iter().zip(&b) {
            assert!((b / a - 2.0).abs() < 1e-4, "{a} then {b}");
        }
    }

    /// Each cell holds the mean of its own part of the picture, laid along the long edge whichever way the picture stands, so a
    /// picture and its quarter turn give the same grid up to a reversal of an axis.
    #[test]
    fn the_colour_grid_follows_the_long_edge() {
        // red rises along x, blue along y
        let land = picture(240, 160, |x, y| [x as f32 / 240.0, 0.5, y as f32 / 160.0]);
        let g = colour_grid(&land).unwrap();
        assert_eq!(g.len(), GRID_LONG * GRID_SHORT);
        let cell = |g: &[Option<[f32; 3]>], u: usize, v: usize| g[v * GRID_LONG + u].unwrap();
        // (cell u spans x 20u..20u+20: its mean red is about (20u + 9.5) / 240 with the sampling step of 1)
        assert!((cell(&g, 3, 0)[0] - 69.5 / 240.0).abs() < 0.01);
        assert!((cell(&g, 0, 5)[2] - 109.5 / 160.0).abs() < 0.01);
        // the same scene turned a quarter (portrait): red now rises down the picture, which is the long edge
        let port = picture(160, 240, |x, y| [y as f32 / 240.0, 0.5, (159 - x) as f32 / 160.0]);
        let p = colour_grid(&port).unwrap();
        for v in 0..GRID_SHORT {
            for u in 0..GRID_LONG {
                let (a, b) = (cell(&g, u, v), cell(&p, u, GRID_SHORT - 1 - v));
                assert!(a.iter().zip(&b).all(|(a, b)| (a - b).abs() < 0.01), "{u},{v}: {a:?} {b:?}");
            }
        }
    }

    /// A picture too small for the grid, or of no number, gives none or empty cells, never a wrong colour.
    #[test]
    fn the_colour_grid_of_nothing_usable() {
        assert!(colour_grid(&picture(0, 0, |_, _| [0.0; 3])).is_none());
        assert!(colour_grid(&picture(7, 30, |_, _| [0.5; 3])).is_none());
        assert!(colour_grid(&picture(64, 64, |_, _| [f32::NAN; 3])).is_some_and(|g| g.iter().all(Option::is_none)));
        let mut bad = picture(64, 64, |_, _| [0.5; 3]);
        bad.data.truncate(100);
        assert!(colour_grid(&bad).is_none());
    }

    /// A picture too small, empty, or not a number gives no answer rather than a wrong one.
    #[test]
    fn nothing_usable_gives_none() {
        assert!(luminance_quantiles(&picture(0, 0, |_, _| [0.0; 3])).is_none());
        assert!(luminance_quantiles(&picture(5, 5, |_, _| [0.5; 3])).is_none());
        assert!(luminance_quantiles(&picture(64, 64, |_, _| [f32::NAN; 3])).is_none());
        // (a picture whose size and data disagree)
        let mut bad = picture(64, 64, |_, _| [0.5; 3]);
        bad.data.truncate(100);
        assert!(luminance_quantiles(&bad).is_none());
        // (infinities are not light: they are left out, the rest answers)
        let some = picture(64, 64, |x, _| if x == 0 { [f32::INFINITY; 3] } else { [0.5; 3] });
        assert!(luminance_quantiles(&some).is_some_and(|q| q.iter().all(|v| (v - 0.5).abs() < 1e-6)));
    }
}
