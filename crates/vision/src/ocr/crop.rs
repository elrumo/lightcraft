//! Cutting a text line out of the photo for the recogniser: the box's pixels, straightened to a
//! strip 48 high (the recogniser's height) and as wide as the line's shape asks, as the
//! network's input (BGR, normalised to [-1, 1]).

use lightcraft_raster::Rgba8;

use super::dbnet::Rect;

/// Height of the recogniser's input.
pub const LINE_H: usize = 48;

/// Bilinear sample of `img` at `(x, y)` (pixel centres at +0.5; the edge is extended), as BGR.
fn sample(img: &Rgba8, x: f32, y: f32) -> [f32; 3] {
    let (w, h) = (img.width as i64, img.height as i64);
    let (fx, fy) = (x - 0.5, y - 0.5);
    let (x0, y0) = (fx.floor(), fy.floor());
    let (tx, ty) = (fx - x0, fy - y0);
    let at = |xi: i64, yi: i64| -> [f32; 3] {
        let (xi, yi) = (xi.clamp(0, w - 1) as usize, yi.clamp(0, h - 1) as usize);
        let p = img.data.get(yi * img.width + xi).copied().unwrap_or([0, 0, 0, 255]);
        [f32::from(p[2]), f32::from(p[1]), f32::from(p[0])]
    };
    let (xi, yi) = (x0 as i64, y0 as i64);
    let (a, b, c, d) = (at(xi, yi), at(xi + 1, yi), at(xi, yi + 1), at(xi + 1, yi + 1));
    let mut out = [0f32; 3];
    for k in 0..3 {
        let top = a[k] + (b[k] - a[k]) * tx;
        let bottom = c[k] + (d[k] - c[k]) * tx;
        out[k] = top + (bottom - top) * ty;
    }
    out
}

/// The line inside `r` as a `3 × LINE_H × width` plane-by-plane array of BGR values in [-1, 1],
/// with its width (the line's aspect at `LINE_H` high, at most `max_w`, at least 1).
pub fn strip(img: &Rgba8, r: &Rect, max_w: usize) -> Option<(usize, Vec<f32>)> {
    let (len, depth) = (2.0 * r.half[0], 2.0 * r.half[1]);
    if !(len.is_finite() && depth.is_finite()) || len < 1.0 || depth < 1.0 || img.width == 0 || img.height == 0 {
        return None;
    }
    let width = ((len * LINE_H as f32 / depth).ceil() as usize).clamp(1, max_w.max(1));
    let (su, sv) = (len / width as f32, depth / LINE_H as f32);
    let v = r.v();
    let mut out = vec![0f32; 3 * LINE_H * width];
    for y in 0..LINE_H {
        let dv = (y as f32 + 0.5) * sv - r.half[1];
        for x in 0..width {
            let du = (x as f32 + 0.5) * su - r.half[0];
            let p = sample(img, r.c[0] + du * r.u[0] + dv * v[0], r.c[1] + du * r.u[1] + dv * v[1]);
            for (k, value) in p.iter().enumerate() {
                if let Some(o) = out.get_mut(k * LINE_H * width + y * width + x) {
                    *o = value / 255.0 * 2.0 - 1.0;
                }
            }
        }
    }
    Some((width, out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightcraft_raster::Image;

    fn gradient(w: usize, h: usize) -> Rgba8 {
        Image { width: w, height: h, data: (0..w * h).map(|i| [(i % w * 255 / w.max(1)) as u8, 0, 0, 255]).collect() }
    }

    #[test]
    fn a_level_box_is_cut_out_in_reading_order_as_bgr() {
        let img = gradient(200, 100);
        let r = Rect { c: [100.0, 50.0], u: [1.0, 0.0], half: [96.0, 24.0] };
        let (w, data) = strip(&img, &r, 3200).unwrap();
        assert_eq!(w, 4 * LINE_H, "192 × 48 is 4 : 1");
        assert_eq!(data.len(), 3 * LINE_H * w);
        // the image's red grows to the right; red is the strip's last plane (BGR)
        let red = |x: usize| data[2 * LINE_H * w + 10 * w + x];
        assert!(red(0) < red(w / 2) && red(w / 2) < red(w - 1), "{} {} {}", red(0), red(w / 2), red(w - 1));
        // blue and green are zero in the photo: -1
        assert!(data[..LINE_H * w].iter().all(|&v| (v + 1.0).abs() < 1e-5));
    }

    #[test]
    fn a_box_off_the_edge_is_extended_not_a_panic() {
        let img = gradient(50, 50);
        let r = Rect { c: [-30.0, 70.0], u: [1.0, 0.0], half: [40.0, 10.0] };
        let (w, d) = strip(&img, &r, 3200).unwrap();
        assert!(w > 0 && d.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn a_long_line_is_squeezed_to_the_widest_input() {
        let img = gradient(4000, 100);
        let r = Rect { c: [2000.0, 50.0], u: [1.0, 0.0], half: [1900.0, 10.0] };
        assert_eq!(strip(&img, &r, 3200).unwrap().0, 3200);
    }

    #[test]
    fn degenerate_boxes_and_images_are_refused() {
        let img = gradient(50, 50);
        for half in [[0.2, 5.0], [5.0, 0.2], [f32::NAN, 5.0], [f32::INFINITY, 5.0]] {
            assert!(strip(&img, &Rect { c: [25.0, 25.0], u: [1.0, 0.0], half }, 3200).is_none(), "{half:?}");
        }
        let empty = Image { width: 0, height: 0, data: vec![] };
        assert!(strip(&empty, &Rect { c: [0.0, 0.0], u: [1.0, 0.0], half: [5.0, 5.0] }, 3200).is_none());
    }
}
