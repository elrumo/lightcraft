//! Double exposure (card #452): other photos' finished pictures drawn over this one's, bottom to top
//! ([`lightcraft_develop::ExposureLayer`]). Each layer is its photo rendered with its own edits; it is placed over
//! the frame (covering it at a scale of 100, moved and turned about its middle) and mixed in on display-encoded
//! values, as an image editor's layer modes are. A native renderer draws the same: one formula per mode, below.

use lightcraft_develop::{ExposureLayer, LayerBlend};
use lightcraft_raster::Rgba8;

/// The biggest picture a layer is rendered at, as a multiple of the frame's long edge and of its pixels: a layer scaled
/// up past it is drawn from that and stretched (the part outside the frame is never seen), and a full-size export of a
/// portrait layer over a landscape frame does not render a picture several times the export's size.
const MAX_RENDER_SCALE: f64 = 2.0;
const MAX_RENDER_PIXELS: f64 = 3.0;

/// A layer's values brought into range (from a file or another device, any number can arrive).
fn sane(l: &ExposureLayer) -> (f64, f64, f64, f64, f64) {
    let f = |v: f64, lo: f64, hi: f64, d: f64| if v.is_finite() { v.clamp(lo, hi) } else { d };
    (
        f(l.opacity, 0.0, 100.0, 100.0) / 100.0,
        f(l.x, -100.0, 100.0, 0.0) / 100.0,
        f(l.y, -100.0, 100.0, 0.0) / 100.0,
        f(l.scale, 10.0, 400.0, 100.0) / 100.0,
        f(l.angle, -180.0, 180.0, 0.0),
    )
}

/// Whether `l` draws anything here: a photo layer, shown, with some opacity. A text layer is a native frontend's to
/// draw (card #446): this renderer leaves it out, as it leaves out a layer whose photo is gone.
pub fn draws(l: &ExposureLayer) -> bool {
    l.visible && l.text.is_none() && sane(l).0 > 0.0
}

/// The box to render a layer's photo into, of aspect `aspect` (width / height), so that it covers a `w × h` frame at
/// its scale (the render is fitted inside the box, keeping its aspect). Never more than [`MAX_RENDER_SCALE`] times the
/// frame's long edge nor [`MAX_RENDER_PIXELS`] times its pixels, and at least 1 × 1.
pub fn render_box(w: usize, h: usize, aspect: f64, l: &ExposureLayer) -> (usize, usize) {
    let aspect = if aspect.is_finite() && aspect > 0.0 { aspect.clamp(0.01, 100.0) } else { 1.0 };
    let (w, h) = (w.max(1) as f64, h.max(1) as f64);
    let scale = sane(l).3;
    // (covering: the layer's height is the frame's, or its width is, whichever is the larger)
    let (lw, lh) = if w / h > aspect { (w, w / aspect) } else { (h * aspect, h) };
    let limit = w.max(h) * MAX_RENDER_SCALE;
    let k = scale.min(limit / lw.max(lh)).min((MAX_RENDER_PIXELS * w * h / (lw * lh)).sqrt());
    ((lw * k).round().max(1.0) as usize, (lh * k).round().max(1.0) as usize)
}

/// Mode `m` of backdrop `b` and source `s`, both display-encoded in 0..1 (the W3C compositing formulas; Soft Light is
/// theirs too).
pub fn blend(m: LayerBlend, b: f32, s: f32) -> f32 {
    match m {
        LayerBlend::Normal => s,
        LayerBlend::Screen => 1.0 - (1.0 - b) * (1.0 - s),
        LayerBlend::Multiply => b * s,
        LayerBlend::Overlay => {
            if b <= 0.5 {
                2.0 * b * s
            } else {
                1.0 - 2.0 * (1.0 - b) * (1.0 - s)
            }
        }
        LayerBlend::SoftLight => {
            if s <= 0.5 {
                b - (1.0 - 2.0 * s) * b * (1.0 - b)
            } else {
                let d = if b <= 0.25 { ((16.0 * b - 12.0) * b + 4.0) * b } else { b.sqrt() };
                b + (2.0 * s - 1.0) * (d - b)
            }
        }
        LayerBlend::Lighten => b.max(s),
        LayerBlend::Darken => b.min(s),
        LayerBlend::Difference => (b - s).abs(),
    }
}

/// Where a layer's picture sits on the output: from an output pixel to a point of the picture.
struct Placement {
    cx: f64,
    cy: f64,
    cos: f64,
    sin: f64,
    /// Picture pixels per output pixel.
    inv: f64,
    pw: f64,
    ph: f64,
}

impl Placement {
    fn new(full_w: usize, full_h: usize, pic: &Rgba8, l: &ExposureLayer) -> Option<Placement> {
        let (_, x, y, scale, angle) = sane(l);
        let (fw, fh) = (full_w.max(1) as f64, full_h.max(1) as f64);
        let (pw, ph) = (pic.width as f64, pic.height as f64);
        if pic.width == 0 || pic.height == 0 {
            return None;
        }
        // the picture's own size decides how it covers the frame: a render a pixel off its box changes nothing
        let cover = (fw / pw).max(fh / ph) * scale;
        let (sin, cos) = angle.to_radians().sin_cos();
        Some(Placement { cx: fw / 2.0 + x * fw, cy: fh / 2.0 + y * fh, cos, sin, inv: 1.0 / cover, pw, ph })
    }

    /// The picture point (in its pixels) under output pixel centre (`px`, `py`).
    fn at(&self, px: f64, py: f64) -> (f64, f64) {
        let (dx, dy) = (px - self.cx, py - self.cy);
        // (turned back: the layer is turned clockwise on the output)
        let (qx, qy) = (self.cos * dx + self.sin * dy, -self.sin * dx + self.cos * dy);
        (qx * self.inv + self.pw / 2.0, qy * self.inv + self.ph / 2.0)
    }
}

/// The picture's colour (0..1, display-encoded) and how much of it covers (`u`, `v`): bilinear, with the edge
/// softened over one output pixel (`cover`: output pixels per picture pixel) and the picture's own alpha (an expanded
/// area left transparent).
fn sample(pic: &Rgba8, u: f64, v: f64, cover: f64) -> Option<([f32; 3], f32)> {
    let (w, h) = (pic.width as f64, pic.height as f64);
    let edge = |t: f64, n: f64| (t.min(n - t) * cover + 0.5).clamp(0.0, 1.0);
    let cover = edge(u, w) * edge(v, h);
    if cover <= 0.0 {
        return None;
    }
    let (x, y) = ((u - 0.5).clamp(0.0, w - 1.0), (v - 0.5).clamp(0.0, h - 1.0));
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(pic.width - 1), (y0 + 1).min(pic.height - 1));
    let (fx, fy) = ((x - x0 as f64) as f32, (y - y0 as f64) as f32);
    let px = |x: usize, y: usize| pic.data.get(y * pic.width + x).copied().unwrap_or([0; 4]);
    let (a, b, c, d) = (px(x0, y0), px(x1, y0), px(x0, y1), px(x1, y1));
    let mut rgb = [0.0f32; 4];
    for (k, out) in rgb.iter_mut().enumerate() {
        let top = f32::from(a[k]) * (1.0 - fx) + f32::from(b[k]) * fx;
        let bottom = f32::from(c[k]) * (1.0 - fx) + f32::from(d[k]) * fx;
        *out = (top * (1.0 - fy) + bottom * fy) / 255.0;
    }
    Some(([rgb[0], rgb[1], rgb[2]], rgb[3] * cover as f32))
}

/// Draw layer `l` (its photo's finished picture `pic`, sRGB-encoded) over `base`, the part of a `full_w × full_h`
/// output whose top left is at (`ox`, `oy`) (the whole output: 0, 0 and its own size).
pub fn composite_rgba8(base: &mut Rgba8, (ox, oy): (usize, usize), (full_w, full_h): (usize, usize), pic: &Rgba8, l: &ExposureLayer) {
    let opacity = sane(l).0 as f32;
    let Some(place) = Placement::new(full_w, full_h, pic, l).filter(|_| draws(l)) else { return };
    let width = base.width;
    for (i, out) in base.data.iter_mut().enumerate() {
        let (px, py) = ((ox + i % width.max(1)) as f64 + 0.5, (oy + i / width.max(1)) as f64 + 0.5);
        let (u, v) = place.at(px, py);
        let Some((s, a)) = sample(pic, u, v, 1.0 / place.inv) else { continue };
        let alpha = a * opacity;
        for k in 0..3 {
            let b = f32::from(out[k]) / 255.0;
            let m = blend(l.blend, b, s[k]);
            out[k] = ((b + (m - b) * alpha) * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
}

/// [`composite_rgba8`] for a whole output's 16-bit display-encoded RGB samples, so a 16-bit export has the layers its
/// 8-bit reduction has.
pub fn composite_u16(samples: &mut [u16], width: usize, (full_w, full_h): (usize, usize), pic: &Rgba8, l: &ExposureLayer) {
    let opacity = sane(l).0 as f32;
    let Some(place) = Placement::new(full_w, full_h, pic, l).filter(|_| draws(l)) else { return };
    for (i, out) in samples.as_chunks_mut::<3>().0.iter_mut().enumerate() {
        let (px, py) = ((i % width.max(1)) as f64 + 0.5, (i / width.max(1)) as f64 + 0.5);
        let (u, v) = place.at(px, py);
        let Some((s, a)) = sample(pic, u, v, 1.0 / place.inv) else { continue };
        let alpha = a * opacity;
        for k in 0..3 {
            let b = f32::from(out[k]) / 65535.0;
            let m = blend(l.blend, b, s[k]);
            out[k] = ((b + (m - b) * alpha) * 65535.0).round().clamp(0.0, 65535.0) as u16;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: usize, h: usize, c: [u8; 4]) -> Rgba8 {
        Rgba8 { width: w, height: h, data: vec![c; w * h] }
    }

    #[test]
    fn modes_follow_their_formulas() {
        let (b, s) = (0.25f32, 0.5f32);
        assert_eq!(blend(LayerBlend::Normal, b, s), 0.5);
        assert!((blend(LayerBlend::Screen, b, s) - 0.625).abs() < 1e-6);
        assert!((blend(LayerBlend::Multiply, b, s) - 0.125).abs() < 1e-6);
        assert!((blend(LayerBlend::Overlay, b, s) - 0.25).abs() < 1e-6);
        assert!((blend(LayerBlend::SoftLight, b, s) - 0.25).abs() < 1e-6);
        assert_eq!(blend(LayerBlend::Lighten, b, s), 0.5);
        assert_eq!(blend(LayerBlend::Darken, b, s), 0.25);
        assert!((blend(LayerBlend::Difference, b, s) - 0.25).abs() < 1e-6);
        // every mode keeps 0..1 in 0..1
        for m in LayerBlend::ALL {
            for b in [0.0f32, 0.1, 0.5, 0.9, 1.0] {
                for s in [0.0f32, 0.3, 0.5, 0.7, 1.0] {
                    let v = blend(m, b, s);
                    assert!((0.0..=1.0).contains(&v), "{m:?} {b} {s} -> {v}");
                }
            }
        }
    }

    #[test]
    fn a_screened_layer_lightens_the_frame_it_covers() {
        let mut base = solid(8, 6, [64, 64, 64, 255]);
        let pic = solid(4, 3, [128, 128, 128, 255]);
        composite_rgba8(&mut base, (0, 0), (8, 6), &pic, &ExposureLayer::new(7));
        // 1 - (1 - 0.251)(1 - 0.502) = 0.627
        assert!(base.data.iter().all(|p| (p[0] as i32 - 160).abs() <= 1 && p[3] == 255), "{:?}", base.data[0]);
    }

    #[test]
    fn opacity_hidden_and_placement() {
        let pic = solid(4, 4, [255, 255, 255, 255]);
        let mut half = solid(4, 4, [0, 0, 0, 255]);
        composite_rgba8(&mut half, (0, 0), (4, 4), &pic, &ExposureLayer { blend: LayerBlend::Normal, opacity: 50.0, ..ExposureLayer::new(1) });
        assert!(half.data.iter().all(|p| p[0] == 128));
        let mut hidden = solid(4, 4, [0, 0, 0, 255]);
        composite_rgba8(&mut hidden, (0, 0), (4, 4), &pic, &ExposureLayer { visible: false, ..ExposureLayer::new(1) });
        assert!(hidden.data.iter().all(|p| p[0] == 0));
        // moved half a frame right at half size: the left half of the frame keeps the base
        let mut moved = solid(8, 8, [0, 0, 0, 255]);
        composite_rgba8(
            &mut moved,
            (0, 0),
            (8, 8),
            &pic,
            &ExposureLayer { blend: LayerBlend::Normal, x: 25.0, scale: 50.0, ..ExposureLayer::new(1) },
        );
        let at = |x: usize, y: usize| moved.data[y * 8 + x][0];
        assert_eq!(at(1, 4), 0);
        assert_eq!(at(6, 4), 255);
        assert_eq!(at(6, 0), 0);
    }

    #[test]
    fn hostile_numbers_and_empty_pictures_do_not_panic() {
        let mut base = solid(5, 5, [10, 20, 30, 255]);
        let weird = ExposureLayer { opacity: f64::NAN, x: f64::INFINITY, y: -1e300, scale: 0.0, angle: f64::NAN, ..ExposureLayer::new(1) };
        composite_rgba8(&mut base, (0, 0), (5, 5), &solid(3, 2, [200; 4]), &weird);
        composite_rgba8(&mut base, (0, 0), (5, 5), &Rgba8 { width: 0, height: 0, data: vec![] }, &ExposureLayer::new(1));
        let mut deep = vec![1000u16; 5 * 5 * 3];
        composite_u16(&mut deep, 5, (5, 5), &solid(3, 2, [200; 4]), &weird);
        assert_eq!(render_box(0, 0, f64::NAN, &weird), (1, 1));
    }

    #[test]
    fn the_render_box_covers_the_frame() {
        let l = ExposureLayer::new(1);
        // a portrait photo over a landscape frame: as wide as the frame
        assert_eq!(render_box(600, 400, 2.0 / 3.0, &l), (600, 900));
        assert_eq!(render_box(600, 400, 3.0, &l), (1200, 400));
        // scaled up past the limits: capped at three times the frame's pixels
        let big = ExposureLayer { scale: 400.0, ..l };
        assert_eq!(render_box(600, 400, 1.5, &big), (1039, 693));
    }
}
