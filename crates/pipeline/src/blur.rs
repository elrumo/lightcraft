//! Blur (Effects panel, card #499): the photo, or what a mask selects, blurred after the fact. Five styles
//! ([`BlurStyle`]): a plain Gaussian, a lens's disc (bokeh: scene-linear highlights spread into bright discs), and a
//! camera moving while the shutter is open: along a line (Motion), turning round a centre (Spin) or zooming towards it
//! (Zoom). Sizes are fractions of the long edge, so a preview and an export look alike.
//!
//! It runs on the scene-linear image after Lens Blur ([`crate::after_denoise`]), on the CPU, for both renderers; the
//! Apple renderer of the native frontend draws the same steps (`BlurStage`):
//! - Gaussian: [`blur::gaussian`] of sigma [`GAUSS_MAX`] · amount (Core Image's Gaussian of that sigma);
//! - Lens: the disc blur of [`crate::lens_blur`] of radius [`lens_blur::MAX_RADIUS`] · amount (Core Image's disc blur);
//! - Motion: a Gaussian along the line of sigma [`MOTION_MAX`] · amount ([`crate::glow`]'s line blur, Core Image's
//!   motion blur, which is a Gaussian along the line whose sigma is its radius);
//! - Spin and Zoom: the mean of copies of the picture turned round (zoomed towards) the centre, evenly spread over
//!   [`SPIN_MAX`] radians (a scale of `e^-ZOOM_MAX`) · amount. The copies are made in two passes, a few fine ones over one
//!   step of the coarse ones and then the coarse ones, so a wide blur costs `coarse + fine` taps a pixel and still has
//!   about one tap per pixel the picture moves ([`taps`]).
//!
//! A mask's Blur ([`LocalAdjustments::blur`](lightcraft_develop::LocalAdjustments)) blurs the whole picture the same way
//! and mixes it in by the mask's strength.

use lightcraft_develop::{BlurStyle, DevelopSettings, LocalAdjustments, Mask};
use lightcraft_raster::{Rgb32f, blur};

use crate::geometry::Frame;
use crate::masks::{self, Mattes};

/// Gaussian sigma at Blur 100, as a fraction of the long edge.
pub const GAUSS_MAX: f64 = 0.02;
/// Motion's sigma along its line at Blur 100, as a fraction of the long edge.
pub const MOTION_MAX: f64 = 0.03;
/// Spin's whole turn at Blur 100, radians (30°).
pub const SPIN_MAX: f64 = std::f64::consts::PI / 6.0;
/// Zoom at Blur 100: the copies run from the picture to it scaled by `e^-ZOOM_MAX` towards the centre.
pub const ZOOM_MAX: f64 = 0.35;
/// The most copies one pass of Spin or Zoom makes (with two passes, up to `MAX_TAPS²` steps along the path).
pub const MAX_TAPS: usize = 32;

/// One blur, resolved for a render's size (the photo's or a mask's).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Kind {
    pub style: BlurStyle,
    /// 0..1.
    pub amount: f32,
    /// Motion's direction (radians, 0: horizontal, positive turns up as the sliders show it).
    pub angle: f32,
    /// Spin's and Zoom's centre, 0..1 of the whole output frame.
    pub center: [f32; 2],
}

impl Kind {
    fn new(style: BlurStyle, amount: f64, angle: f64, cx: f64, cy: f64) -> Option<Kind> {
        if !(amount.is_finite() && amount > 0.0) {
            return None;
        }
        let pct = |v: f64| if v.is_finite() { (v / 100.0).clamp(0.0, 1.0) as f32 } else { 0.5 };
        let angle = if angle.is_finite() { angle.clamp(-90.0, 90.0).to_radians() as f32 } else { 0.0 };
        Some(Kind { style, amount: (amount / 100.0).min(1.0) as f32, angle, center: [pct(cx), pct(cy)] })
    }

    fn local(a: &LocalAdjustments) -> Option<Kind> {
        Kind::new(a.blur_style, a.blur, a.blur_angle, a.blur_center_x, a.blur_center_y)
    }

    /// The size of the blur in output pixels: Gaussian and Motion's sigma, Lens's radius; for Spin and Zoom the farthest
    /// any pixel of the `full` frame moves along its path.
    pub fn size(&self, px_per_long: f64, full: [usize; 2]) -> f32 {
        let a = f64::from(self.amount);
        let px = match self.style {
            BlurStyle::Gaussian => GAUSS_MAX * a * px_per_long,
            BlurStyle::Lens => crate::lens_blur::MAX_RADIUS * a * px_per_long,
            BlurStyle::Motion => MOTION_MAX * a * px_per_long,
            BlurStyle::Spin => SPIN_MAX * a * self.far(full),
            BlurStyle::Zoom => (1.0 - (-ZOOM_MAX * a).exp()) * self.far(full),
        };
        if px.is_finite() { px.max(0.0) as f32 } else { 0.0 }
    }

    /// The distance (pixels) from the centre to the farthest corner of the `full` frame.
    fn far(&self, [w, h]: [usize; 2]) -> f64 {
        let (cx, cy) = (f64::from(self.center[0]) * w as f64, f64::from(self.center[1]) * h as f64);
        let dx = cx.max(w as f64 - cx);
        let dy = cy.max(h as f64 - cy);
        dx.hypot(dy)
    }

    /// How far (output pixels) a pixel's blur reads.
    fn reach(&self, px_per_long: f64, full: [usize; 2]) -> usize {
        let s = self.size(px_per_long, full);
        let r = match self.style {
            BlurStyle::Gaussian | BlurStyle::Motion => 3.0 * s,
            _ => s,
        };
        r.ceil() as usize + 2
    }
}

/// The two passes of Spin and Zoom for a path `d` pixels long at its farthest: `(coarse, fine)` copies, about one copy a
/// pixel, at most [`MAX_TAPS`] each (both 1 for no movement).
pub fn taps(d: f32) -> (usize, usize) {
    if !(d.is_finite() && d > 1.0) {
        return (1, 1);
    }
    let coarse = (d.sqrt().ceil() as usize).clamp(1, MAX_TAPS);
    let fine = ((d / coarse as f32).ceil() as usize).clamp(1, MAX_TAPS);
    (coarse, fine)
}

/// What a render's Blur does: the photo's, then each blurring mask's.
#[derive(Clone, Debug)]
pub struct Setup {
    global: Option<Kind>,
    /// The masks that blur, with their blur (in mask order).
    masks: Vec<(Mask, Kind)>,
    px_per_long: f64,
    /// Stage-cache key of all of the above.
    pub key: u64,
}

impl Setup {
    /// The blur of `s` at `px_per_long` output pixels per long edge; `None` when nothing blurs (Blur at 0 and no mask
    /// with one, or the Effects eye off).
    pub fn new(s: &DevelopSettings, px_per_long: f64) -> Option<Setup> {
        if !(px_per_long.is_finite() && px_per_long > 0.0) || !s.section_enabled("effects") {
            return None;
        }
        let b = &s.blur;
        let global = Kind::new(b.style, b.amount, b.angle, b.center_x, b.center_y);
        let masks: Vec<(Mask, Kind)> =
            s.masks.iter().filter(|m| m.visible && !m.components.is_empty()).filter_map(|m| Kind::local(&m.adjust).map(|k| (m.clone(), k))).collect();
        if global.is_none() && masks.is_empty() {
            return None;
        }
        let key = crate::hash_of((format!("{global:?}"), format!("{masks:?}"), px_per_long.to_bits(), s.light.exposure.to_bits()));
        Some(Setup { global, masks, px_per_long, key })
    }

    /// Pixels the blur reads beyond a pixel (for windowed renders) in a `full` output frame.
    pub fn reach(&self, full: [usize; 2]) -> usize {
        self.global.iter().chain(self.masks.iter().map(|(_, k)| k)).map(|k| k.reach(self.px_per_long, full)).max().unwrap_or(0)
    }
}

/// Blur `img` (scene-linear, at output resolution through `frame`; its pixel (0, 0) sits at `origin` of the `full` output
/// frame) as `setup` says; `ev` and `mattes` are what the masks are evaluated with.
pub fn apply(img: &mut Rgb32f, setup: &Setup, frame: &Frame, origin: [usize; 2], full: [usize; 2], ev: f32, mattes: Option<&Mattes>) {
    let (w, h) = (img.width, img.height);
    if w == 0 || h == 0 || img.data.len() != w.saturating_mul(h) {
        return;
    }
    crate::timed("blur", || {
        if let Some(k) = &setup.global {
            *img = blurred(img, k, setup.px_per_long, origin, full);
        }
        for (m, k) in &setup.masks {
            let log_l = img.map(crate::local::log_lum);
            let alpha = masks::evaluate_one(m, frame, w, h, img, &log_l, ev, mattes);
            if alpha.data.len() != img.data.len() {
                continue;
            }
            let b = blurred(img, k, setup.px_per_long, origin, full);
            for ((p, q), a) in img.data.iter_mut().zip(&b.data).zip(&alpha.data) {
                let a = if a.is_finite() { a.clamp(0.0, 1.0) } else { 0.0 };
                *p = std::array::from_fn(|c| p[c] + (q[c] - p[c]) * a);
            }
        }
    });
}

/// `img` blurred by `k` (see [`apply`]).
pub fn blurred(img: &Rgb32f, k: &Kind, px_per_long: f64, origin: [usize; 2], full: [usize; 2]) -> Rgb32f {
    let size = k.size(px_per_long, full);
    match k.style {
        BlurStyle::Gaussian => blur::gaussian(img, size),
        BlurStyle::Lens => {
            let mut out = img.clone();
            if size >= 0.5 {
                let coc = vec![1.0f32; img.data.len()];
                crate::lens_blur::disc_blur(&mut out, &coc, size, (origin[0], origin[1]));
            }
            out
        }
        BlurStyle::Motion => {
            // (the sliders' angle turns up on the screen, the rows run down)
            let dir = [k.angle.cos(), -k.angle.sin()];
            crate::glow::line_blur(img, dir, size)
        }
        BlurStyle::Spin | BlurStyle::Zoom => path_blur(img, k, size, origin, full),
    }
}

/// Spin or Zoom (see the module's note): the fine pass over one coarse step, then the coarse pass.
fn path_blur(img: &Rgb32f, k: &Kind, d: f32, origin: [usize; 2], full: [usize; 2]) -> Rgb32f {
    let (coarse, fine) = taps(d);
    if coarse * fine <= 1 {
        return img.clone();
    }
    // the centre in this image's pixels
    let c = [k.center[0] * full[0] as f32 - origin[0] as f32, k.center[1] * full[1] as f32 - origin[1] as f32];
    // the whole path as one parameter: Spin's turn (radians), Zoom's log scale
    let span = match k.style {
        BlurStyle::Spin => SPIN_MAX as f32 * k.amount,
        _ => ZOOM_MAX as f32 * k.amount,
    };
    let step = span / coarse as f32;
    // fine copies centred on 0 over one coarse step; coarse ones over the whole path (Spin centred on the picture, Zoom
    // from it towards the centre)
    let fine_at: Vec<f32> = (0..fine).map(|j| ((j as f32 + 0.5) / fine as f32 - 0.5) * step).collect();
    let coarse_at: Vec<f32> = (0..coarse)
        .map(|i| match k.style {
            BlurStyle::Spin => ((i as f32 + 0.5) / coarse as f32 - 0.5) * span,
            _ => (i as f32 + 0.5) * step,
        })
        .collect();
    let a = copies(img, k.style, c, &fine_at);
    copies(&a, k.style, c, &coarse_at)
}

/// The mean of `img` sampled at each pixel moved by every parameter in `at` (Spin: turned by it round `c`; Zoom: scaled
/// by `e^-t` towards `c`).
fn copies(img: &Rgb32f, style: BlurStyle, c: [f32; 2], at: &[f32]) -> Rgb32f {
    let w = img.width;
    let n = at.len().max(1) as f32;
    let moves: Vec<[f32; 4]> = at
        .iter()
        .map(|&t| match style {
            // [a b; -b a] · (p - c): a turn by t
            BlurStyle::Spin => [t.cos(), t.sin(), 0.0, 0.0],
            _ => [(-t).exp(), 0.0, 0.0, 0.0],
        })
        .collect();
    let mut out = img.clone();
    crate::for_rows(&mut out.data, w, |y, row| {
        let py = y as f32 + 0.5 - c[1];
        for (x, px) in row.iter_mut().enumerate() {
            let pxx = x as f32 + 0.5 - c[0];
            let mut acc = [0f32; 3];
            for m in &moves {
                let sx = c[0] + m[0] * pxx - m[1] * py;
                let sy = c[1] + m[1] * pxx + m[0] * py;
                let v = img.sample_bilinear(sx, sy);
                acc.iter_mut().zip(v).for_each(|(a, v)| *a += v);
            }
            *px = acc.map(|v| v / n);
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightcraft_develop::{MaskComponent, MaskOp, MaskShape};

    /// A bright dot in the middle of a dark `n × n` square, with a bright vertical bar at a quarter.
    fn scene(n: usize) -> Rgb32f {
        let mut img = Rgb32f::new(n, n);
        for y in 0..n {
            for x in 0..n {
                let v = if (x as i64 - n as i64 / 2).abs() <= 1 && (y as i64 - n as i64 / 2).abs() <= 1 || x == n / 4 { 1.0 } else { 0.05 };
                img.data[y * n + x] = [v; 3];
            }
        }
        img
    }

    fn settings(style: BlurStyle, amount: f64) -> DevelopSettings {
        let mut s = DevelopSettings::default();
        s.blur.style = style;
        s.blur.amount = amount;
        s
    }

    fn mean(img: &Rgb32f) -> f64 {
        img.data.iter().map(|p| f64::from(p[1])).sum::<f64>() / img.data.len() as f64
    }

    /// `img` (square) blurred by `s`, rendered at `scale` output pixels per pixel of `img`'s long edge.
    fn run_at(s: &DevelopSettings, img: &Rgb32f, scale: f64) -> Rgb32f {
        let n = img.width;
        let setup = Setup::new(s, scale * n as f64).expect("a blur");
        let mut out = img.clone();
        let frame = Frame::new(n, n, s, true);
        apply(&mut out, &setup, &frame, [0, 0], [n, n], 0.0, None);
        out
    }

    fn run(s: &DevelopSettings, img: &Rgb32f) -> Rgb32f {
        run_at(s, img, 1.0)
    }

    #[test]
    fn nothing_blurs_at_zero_or_with_the_effects_eye_off() {
        assert!(Setup::new(&DevelopSettings::default(), 100.0).is_none());
        let mut s = settings(BlurStyle::Lens, 50.0);
        s.disabled_sections.push("effects".into());
        assert!(Setup::new(&s, 100.0).is_none());
        // (not a number, or nothing to draw on: no blur and no panic)
        assert!(Setup::new(&settings(BlurStyle::Spin, f64::NAN), 100.0).is_none());
        assert!(Setup::new(&settings(BlurStyle::Spin, 50.0), f64::INFINITY).is_none());
        let setup = Setup::new(&settings(BlurStyle::Zoom, 100.0), 100.0).expect("a blur");
        let mut empty = Rgb32f::new(0, 0);
        apply(&mut empty, &setup, &Frame::new(1, 1, &DevelopSettings::default(), true), [0, 0], [0, 0], 0.0, None);
    }

    /// Every style spreads the bar and keeps the light (a blur moves light, it doesn't add or take any).
    #[test]
    fn every_style_spreads_the_bar_and_keeps_the_light() {
        let n = 96;
        let img = scene(n);
        // (the bar, a quarter across, where Spin and Zoom move it too: the dot sits on their centre)
        let bar = |i: &Rgb32f| i.data[(n / 2 + 30) * n + n / 4][1];
        for style in BlurStyle::ALL {
            let out = run_at(&settings(style, 60.0), &img, 4.0);
            assert!(bar(&out) < 0.6, "{style:?}: the bar was not spread ({})", bar(&out));
            let (a, b) = (mean(&img), mean(&out));
            assert!((a - b).abs() / a < 0.08, "{style:?}: light {a} became {b}");
            assert!(out.data.iter().all(|p| p.iter().all(|v| v.is_finite())), "{style:?}");
        }
    }

    /// Spin moves a dot round the centre, at its distance from it; Zoom along its radius.
    #[test]
    fn spin_turns_and_zoom_runs_towards_the_centre() {
        let n = 128;
        let mut img = Rgb32f::new(n, n);
        // one bright pixel right of the middle
        img.data[(n / 2) * n + n / 2 + 40] = [1.0; 3];
        let at = |i: &Rgb32f, dx: i64, dy: i64| i.data[((n as i64 / 2 + dy) * n as i64 + n as i64 / 2 + dx) as usize][0];
        let spun = run(&settings(BlurStyle::Spin, 100.0), &img);
        // (30° of turn at 40 px: the arc's ends ±10 px up and down; 15 px towards the middle is off the arc)
        assert!(at(&spun, 39, 8) > 0.0 && at(&spun, 39, -8) > 0.0, "spin spreads the dot along its arc");
        assert_eq!(at(&spun, 25, 0), 0.0, "spin keeps the dot at its distance");
        let zoomed = run(&settings(BlurStyle::Zoom, 100.0), &img);
        // (each pixel reads the picture from itself towards the centre, so the dot streaks outwards along its radius)
        assert!(at(&zoomed, 46, 0) > 0.0, "zoom smears the dot along its radius");
        assert_eq!(at(&zoomed, 34, 0), 0.0, "and not inwards");
        assert_eq!(at(&zoomed, 40, 10), 0.0, "zoom doesn't turn it");
    }

    /// Motion at 90° runs up and down, at 0° across.
    #[test]
    fn motion_follows_its_angle() {
        let n = 96;
        let mut img = Rgb32f::new(n, n);
        img.data[(n / 2) * n + n / 2] = [1.0; 3];
        let at = |i: &Rgb32f, dx: i64, dy: i64| i.data[((n as i64 / 2 + dy) * n as i64 + n as i64 / 2 + dx) as usize][0];
        let mut s = settings(BlurStyle::Motion, 100.0);
        let across = run(&s, &img);
        assert!(at(&across, 3, 0) > 10.0 * at(&across, 0, 3), "0°: across");
        s.blur.angle = 90.0;
        let up = run(&s, &img);
        assert!(at(&up, 0, 3) > 10.0 * at(&up, 3, 0), "90°: up and down");
    }

    /// A preview and an export blur alike: the same photo at twice the size, shrunk, is about the same picture.
    #[test]
    fn sizes_follow_the_long_edge() {
        for style in BlurStyle::ALL {
            let k = Kind::new(style, 50.0, 0.0, 50.0, 50.0).expect("a blur");
            let (a, b) = (k.size(500.0, [500, 400]), k.size(1000.0, [1000, 800]));
            assert!((b / a - 2.0).abs() < 1e-3, "{style:?}: {a} at 500 px, {b} at 1000");
        }
        assert_eq!(taps(0.0), (1, 1));
        assert_eq!(taps(f32::NAN), (1, 1));
        let (c, f) = taps(400.0);
        assert!(c * f >= 400 && c <= MAX_TAPS && f <= MAX_TAPS, "{c} x {f}");
        let (c, f) = taps(1e9);
        assert!(c == MAX_TAPS && f == MAX_TAPS);
    }

    /// A mask's blur stays inside it: a radial mask round the left half blurs the bar there and leaves the dot sharp.
    #[test]
    fn a_mask_blurs_only_what_it_selects() {
        let n = 96;
        let img = scene(n);
        let mut s = DevelopSettings::default();
        let mut m = Mask::default();
        m.components.push(MaskComponent {
            name: None,
            op: MaskOp::Add,
            invert: false,
            shape: MaskShape::Radial { center: lightcraft_geom::Point::new(0.25, 0.5), rx: 0.12, ry: 0.4, angle: 0.0, feather: 10.0, invert: false },
        });
        m.adjust.blur = 100.0;
        m.adjust.blur_style = BlurStyle::Gaussian;
        s.masks.push(m);
        let out = run(&s, &img);
        let bar = |i: &Rgb32f| i.data[(n / 2) * n + n / 4][1];
        let dot = |i: &Rgb32f| i.data[(n / 2) * n + n / 2][1];
        assert!(bar(&out) < 0.8, "the bar inside the mask is blurred ({})", bar(&out));
        assert!((dot(&out) - 1.0).abs() < 1e-4, "the dot outside it stays sharp ({})", dot(&out));
    }
}
