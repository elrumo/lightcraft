//! **Generative Expand**: continues a photo past its edges with an inpainting network
//! ([`Painter`]: MI-GAN or LaMa), for the crop tool's Expand.
//!
//! The networks see 512 × 512 pixels and were trained to fill holes. [`outpaint`] gives them the
//! whole canvas at once (the photo and the new area around it), so what goes on past one edge
//! agrees with what goes on past the others: a horizon continues on both sides at the same height,
//! a sky stays one sky. A canvas that isn't square is squeezed by at most [`MAX_SQUEEZE`] and the
//! rest letterboxed (the bands are painted too and thrown away), so shapes keep their proportions.
//!
//! The answer has the network's 512 pixels of detail over the canvas: the structure. The caller
//! adds the photo's grain and fine texture on top (`lightcraft_pipeline::expand`, as AI Remove
//! does). Repainting the canvas in 512-pixel windows at full scale was tried and dropped
//! (2026-10-13): a window far from the photo sees too little of it, and the networks painted
//! streaks there that the whole-canvas pass doesn't.
//!
//! The photo's own pixels are never changed. Everything is deterministic: the same canvas gives the
//! same fill.

use lightcraft_raster::resample::{Filter, resize};
use lightcraft_raster::{Plane, Rgb32f};

use crate::{Error, Painter, Result, SIDE};

/// Largest canvas side [`outpaint`] takes: the stored fill's own limit is lower (1024), so this only
/// bounds hostile calls.
pub const MAX_CANVAS: usize = 4096;
/// The most a canvas is squeezed along its long side to fit the network's square (1.25: a 5 × 4
/// canvas fills it); a longer one is letterboxed beyond that.
pub const MAX_SQUEEZE: f32 = 1.25;
/// Network pixels kept free around the canvas in the square: the networks darken the outermost
/// rows of their answer, and this band (painted, then dropped) takes that instead of the canvas.
const MARGIN: usize = 12;

/// Progress and cancellation for [`outpaint`].
pub struct Monitor<'a> {
    /// Asked before the pass; `true` stops the work with [`Error::Cancelled`].
    pub cancelled: &'a (dyn Fn() -> bool + Sync),
    /// Told the fraction done (0..=1).
    pub progress: &'a (dyn Fn(f32) + Sync),
}

impl Monitor<'_> {
    /// No progress, never cancelled.
    pub const NONE: Monitor<'static> = Monitor { cancelled: &|| false, progress: &|_| {} };
}

/// Where a `w × h` canvas goes in the network's square: `(x, y, cw, ch)`, squeezed by at most
/// [`MAX_SQUEEZE`], centred, [`MARGIN`] from the square's edges.
fn placement(w: usize, h: usize) -> (usize, usize, usize, usize) {
    let room = SIDE - 2 * MARGIN;
    let a = w as f32 / h.max(1) as f32;
    let long = a.max(1.0 / a);
    // the short side's share of the room: as large as the squeeze allows
    let short = ((room as f32) * (MAX_SQUEEZE / long).min(1.0)).round().clamp(1.0, room as f32) as usize;
    let (cw, ch) = if a >= 1.0 { (room, short) } else { (short, room) };
    ((SIDE - cw) / 2, (SIDE - ch) / 2, cw, ch)
}

/// `img` (display-encoded, 0..=1) with every pixel where `known` is false painted by `painter`.
/// `known` pixels come back unchanged.
pub fn outpaint(painter: &dyn Painter, img: &Rgb32f, known: &[bool], mon: &Monitor<'_>) -> Result<Rgb32f> {
    let (w, h) = (img.width, img.height);
    if w == 0 || h == 0 || w > MAX_CANVAS || h > MAX_CANVAS || img.data.len() != w * h || known.len() != w * h {
        return Err(Error::Image(format!("can't expand a {w} × {h} canvas")));
    }
    if known.iter().all(|&k| k) {
        return Ok(img.clone());
    }
    if !known.iter().any(|&k| k) {
        return Err(Error::Image("there is nothing of the photo to continue".into()));
    }
    if (mon.cancelled)() {
        return Err(Error::Cancelled);
    }
    // the canvas in the network's square: squeezed a little, letterboxed beyond that
    let (x0, y0, cw, ch) = placement(w, h);
    let small = resize(img, cw, ch, Filter::Mitchell);
    let cover = resize(&Plane { width: w, height: h, data: known.iter().map(|&k| if k { 0.0 } else { 1.0 }).collect() }, cw, ch, Filter::Bilinear);
    let inside = |x: usize, y: usize| x >= x0 && x < x0 + cw && y >= y0 && y < y0 + ch;
    let at = |x: usize, y: usize| (y - y0) * cw + (x - x0);
    let square = Rgb32f::from_fn(SIDE, SIDE, |x, y| if inside(x, y) { small.data.get(at(x, y)).copied().unwrap_or([0.0; 3]) } else { [0.0; 3] });
    // (a network pixel is new if any of the new area reaches it, so the edge isn't kept blurred)
    let hole: Vec<bool> =
        (0..SIDE * SIDE).map(|i| (i % SIDE, i / SIDE)).map(|(x, y)| !inside(x, y) || cover.data.get(at(x, y)).is_some_and(|&c| c > 0.01)).collect();
    let painted = painter.paint(&square, &hole)?;
    (mon.progress)(0.9);
    let content = painted.crop(x0, y0, cw, ch);
    let back = resize(&content, w, h, Filter::Mitchell);
    let mut out = img.clone();
    for ((o, c), &k) in out.data.iter_mut().zip(&back.data).zip(known) {
        if !k {
            *o = c.map(|v| v.clamp(0.0, 1.0));
        }
    }
    (mon.progress)(1.0);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Paints every hole pixel with the mean of the pixels it was given (a stand-in network), and
    /// keeps what it was given.
    struct Mean(Mutex<Vec<(Rgb32f, Vec<bool>)>>);

    impl Painter for Mean {
        fn paint(&self, img: &Rgb32f, hole: &[bool]) -> Result<Rgb32f> {
            crate::check_side(img, hole)?;
            self.0.lock().unwrap().push((img.clone(), hole.to_vec()));
            let kept: Vec<[f32; 3]> = img.data.iter().zip(hole).filter(|(_, h)| !**h).map(|(p, _)| *p).collect();
            let n = kept.len().max(1) as f32;
            let m: [f32; 3] = std::array::from_fn(|c| kept.iter().map(|p| p[c]).sum::<f32>() / n);
            Ok(Rgb32f::from_fn(SIDE, SIDE, |x, y| if hole[y * SIDE + x] { m } else { img.get(x, y) }))
        }
        fn label(&self) -> &'static str {
            "mean"
        }
    }

    fn canvas(w: usize, h: usize, photo: (usize, usize, usize, usize)) -> (Rgb32f, Vec<bool>) {
        let inside = |x: usize, y: usize| x >= photo.0 && x < photo.2 && y >= photo.1 && y < photo.3;
        let img = Rgb32f::from_fn(w, h, |x, y| if inside(x, y) { [0.6, 0.4, 0.2] } else { [0.0; 3] });
        let known = (0..w * h).map(|i| inside(i % w, i / w)).collect();
        (img, known)
    }

    #[test]
    fn the_photo_is_kept_and_the_new_area_painted_in_one_pass() {
        let (img, known) = canvas(900, 600, (100, 80, 800, 520));
        let m = Mean(Mutex::new(Vec::new()));
        let done = Mutex::new(Vec::new());
        let mon = Monitor { cancelled: &|| false, progress: &|f| done.lock().unwrap().push(f) };
        let out = outpaint(&m, &img, &known, &mon).unwrap();
        assert_eq!(m.0.lock().unwrap().len(), 1);
        assert!((done.into_inner().unwrap().last().copied().unwrap() - 1.0).abs() < 1e-6);
        for (i, (o, p)) in out.data.iter().zip(&img.data).enumerate() {
            if known[i] {
                assert_eq!(o, p, "photo pixel {i} changed");
            } else {
                assert!((o[0] - 0.6).abs() < 0.05 && (o[2] - 0.2).abs() < 0.05, "{i}: {o:?}");
            }
        }
    }

    #[test]
    fn a_long_canvas_is_letterboxed_not_squashed() {
        // 3 × 1: squeezed 1.25×, the rest of the square is a band the network paints and we drop
        assert_eq!(placement(900, 300), (12, 154, 488, 203));
        assert_eq!(placement(300, 900), (154, 12, 203, 488));
        assert_eq!(placement(500, 400), (12, 12, 488, 488), "5 × 4 fills the square but its margin");
        assert_eq!(placement(400, 400), (12, 12, 488, 488));
        let (img, known) = canvas(900, 300, (100, 0, 800, 300));
        let m = Mean(Mutex::new(Vec::new()));
        outpaint(&m, &img, &known, &Monitor::NONE).unwrap();
        let (square, hole) = m.0.lock().unwrap().pop().unwrap();
        // the bands above and below are holes, the canvas's photo isn't
        assert!(hole[10 * SIDE + 256] && hole[505 * SIDE + 256]);
        assert!(!hole[256 * SIDE + 256]);
        assert!((square.get(256, 256)[0] - 0.6).abs() < 0.01);
    }

    #[test]
    fn cancel_and_hostile_canvases_are_errors() {
        let (img, known) = canvas(600, 400, (100, 80, 500, 320));
        let m = Mean(Mutex::new(Vec::new()));
        let mon = Monitor { cancelled: &|| true, progress: &|_| {} };
        assert!(matches!(outpaint(&m, &img, &known, &mon), Err(Error::Cancelled)));
        assert!(outpaint(&m, &img, &known[1..], &Monitor::NONE).is_err());
        assert!(outpaint(&m, &Rgb32f::new(0, 0), &[], &Monitor::NONE).is_err());
        assert!(outpaint(&m, &Rgb32f::new(5000, 1), &[false; 5000], &Monitor::NONE).is_err());
        // a 1-pixel-high canvas still fits the square
        let thin = outpaint(&m, &Rgb32f::filled(3000, 1, [0.5; 3]), &(0..3000).map(|i| i < 1500).collect::<Vec<_>>(), &Monitor::NONE).unwrap();
        assert!(thin.data.iter().all(|p| p.iter().all(|v| v.is_finite())));
        // nothing of the photo to continue
        assert!(outpaint(&m, &Rgb32f::new(20, 20), &[false; 400], &Monitor::NONE).is_err());
        // nothing new: the canvas comes back as it is, without a pass
        let calls = m.0.lock().unwrap().len();
        let all = vec![true; 600 * 400];
        assert_eq!(outpaint(&m, &img, &all, &Monitor::NONE).unwrap().data, img.data);
        assert_eq!(m.0.lock().unwrap().len(), calls);
    }

    /// With the real networks (`$LIGHTCRAFT_MIGAN`, `$LIGHTCRAFT_LAMA`): a gradient photo widened by
    /// a quarter on each side is continued as the gradient: closely at the photo's edges, loosely
    /// far from them (LaMa drifts towards the photo's mean there).
    #[test]
    fn the_networks_continue_a_gradient_past_the_edges() {
        let migan = std::env::var_os("LIGHTCRAFT_MIGAN").and_then(|p| std::fs::read(p).ok()).map(|b| crate::Eraser::load(&b).unwrap());
        let lama = crate::lama::tests::model();
        let painters: Vec<Box<dyn Painter>> =
            [migan.map(|m| Box::new(m) as Box<dyn Painter>), lama.map(|m| Box::new(m) as Box<dyn Painter>)].into_iter().flatten().collect();
        let (w, h) = (800, 400);
        let grad = |y: usize| [0.15 + 0.7 * y as f32 / h as f32, 0.5, 0.8 - 0.5 * y as f32 / h as f32];
        let inside = |x: usize| (200..600).contains(&x);
        let img = Rgb32f::from_fn(w, h, |x, y| if inside(x) { grad(y) } else { [0.0; 3] });
        let known: Vec<bool> = (0..w * h).map(|i| inside(i % w)).collect();
        for p in painters {
            let out = outpaint(&*p, &img, &known, &Monitor::NONE).unwrap();
            for (x, y, tol) in [(190, 100, 0.08), (610, 300, 0.08), (150, 100, 0.12), (650, 300, 0.12), (20, 200, 0.2)] {
                let (got, want) = (out.get(x, y), grad(y));
                assert!((0..3).all(|c| (got[c] - want[c]).abs() < tol), "{} ({x}, {y}): {got:?} vs {want:?}", p.label());
            }
        }
    }
}
