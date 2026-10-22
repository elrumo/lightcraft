//! The conversion against the one it replaced: a `powf` per channel of every pixel, a luma sample at a time.

use super::*;

/// `to_working` as it was before the curve tables and the row sums, for a block of `factor` × `factor` luma samples and no
/// rotation: the reference the fast one is held to.
pub(crate) fn reference(f: &Frame, factor: usize) -> Rgb32f {
    let (ow, oh) = ((f.width / factor).max(1), (f.height / factor).max(1));
    let (kr, kb) = coefficients(f.color.matrix, f.height);
    let kg = 1.0 - kr - kb;
    let (cr_r, cb_b) = (2.0 * (1.0 - kr), 2.0 * (1.0 - kb));
    let (cr_g, cb_g) = (cr_r * kr / kg, cb_b * kb / kg);
    let m = primaries(f.color.primaries).to_space(&WORKING).to_f32();
    let scale = (1u32 << (f.bit_depth - 8)) as f32;
    let (y_off, y_rng, c_mid, c_rng) = if f.color.full_range {
        (0.0, ((1u32 << f.bit_depth) - 1) as f32, 128.0 * scale, ((1u32 << f.bit_depth) - 1) as f32)
    } else {
        (16.0 * scale, 219.0 * scale, 128.0 * scale, 224.0 * scale)
    };
    let tf = f.color.transfer;
    let mut out = Rgb32f::new(ow, oh);
    for oy in 0..oh {
        for ox in 0..ow {
            let (x0, y0) = (ox * factor, oy * factor);
            let (x1, y1) = ((x0 + factor).min(f.width), (y0 + factor).min(f.height));
            let (mut sum, mut n) = (0u64, 0u64);
            for sy in y0..y1 {
                for sx in x0..x1 {
                    sum += u64::from(f.y.at(sy * f.y_stride + sx));
                    n += 1;
                }
            }
            let luma = if n == 0 { 0.0 } else { sum as f32 / n as f32 };
            let (cx, cy) = (((x0 + x1) / 2) / 2, ((y0 + y1) / 2) / 2);
            let ci = cy * f.uv_stride + cx;
            let (cb, cr) = (f.u.at(ci) as f32, f.v.at(ci) as f32);
            let yn = ((luma - y_off) / y_rng).clamp(-0.07, 1.09);
            let (pb, pr) = ((cb - c_mid) / c_rng, (cr - c_mid) / c_rng);
            let r = (yn + cr_r * pr).clamp(0.0, 1.0);
            let g = (yn - cb_g * pb - cr_g * pr).clamp(0.0, 1.0);
            let b = (yn + cb_b * pb).clamp(0.0, 1.0);
            let lin = [linear(tf, r), linear(tf, g), linear(tf, b)];
            out.data[oy * ow + ox] = [
                m[0][0] * lin[0] + m[0][1] * lin[1] + m[0][2] * lin[2],
                m[1][0] * lin[0] + m[1][1] * lin[1] + m[1][2] * lin[2],
                m[2][0] * lin[0] + m[2][1] * lin[1] + m[2][2] * lin[2],
            ];
        }
    }
    out
}

/// The largest difference between two pictures of the same size.
pub(crate) fn max_diff(a: &Rgb32f, b: &Rgb32f) -> f32 {
    assert_eq!((a.width, a.height), (b.width, b.height));
    a.data.iter().zip(&b.data).flat_map(|(p, q)| p.iter().zip(q).map(|(x, y)| (x - y).abs())).fold(0.0, f32::max)
}

/// A `w` × `h` picture of pseudo-random samples (a stride wider than the picture, so a wrong row step shows).
fn noise(w: usize, h: usize, bit_depth: u8, color: Color, seed: u32) -> Frame {
    let mut state = seed;
    let mut next = move || {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        state >> 8
    };
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let (ys, cs) = (w + 5, cw + 3);
    let top = 1u32 << bit_depth;
    let mut plane = |len: usize| -> Plane {
        if bit_depth == 8 {
            Plane::U8((0..len).map(|_| (next() % top) as u8).collect())
        } else {
            Plane::U16((0..len).map(|_| (next() % top) as u16).collect())
        }
    };
    Frame {
        width: w,
        height: h,
        chroma_width: cw,
        chroma_height: ch,
        bit_depth,
        y: plane(ys * h),
        u: plane(cs * ch),
        v: plane(cs * ch),
        y_stride: ys,
        uv_stride: cs,
        pts: 0,
        color,
    }
}

#[test]
fn the_fast_conversion_is_the_old_one() {
    let c = |full_range, primaries, transfer, matrix| Color { full_range, primaries, transfer, matrix };
    // (picture colour, bit depth, the largest difference allowed): SDR curves to the last digits of an f32 picture, PQ and
    // HLG to half a thousandth (their tables are steeper)
    let cases = [
        (c(false, 1, 1, 1), 8, 2e-6),
        (c(true, 1, 13, 5), 8, 2e-6),
        (c(false, 12, 13, 1), 8, 2e-6),
        (c(false, 9, 14, 9), 10, 2e-6),
        (c(false, 1, 8, 1), 8, 2e-6),
        (c(false, 9, 16, 9), 10, 5e-4),
        (c(false, 9, 18, 9), 10, 5e-4),
        (Color::UNSPECIFIED, 8, 2e-6),
    ];
    for (n, (color, depth, tolerance)) in cases.into_iter().enumerate() {
        for (w, h, factor) in [(64, 48, 1), (67, 43, 1), (67, 43, 3), (128, 72, 4)] {
            let f = noise(w, h, depth, color, 7 + n as u32);
            let (new, old) = (to_working_by(&f, factor, 0).unwrap(), reference(&f, factor));
            let d = max_diff(&new, &old);
            assert!(d <= tolerance, "case {n} {w}x{h} by {factor}: differs by {d}");
        }
    }
}

#[test]
fn the_curve_tables_follow_the_curves() {
    for (code, tolerance) in [(1, 1e-6), (13, 1e-6), (8, 1e-6), (16, 5e-4), (18, 5e-4)] {
        let table = curve_table(code);
        assert_eq!(table.len(), CURVE_STEPS + 1);
        let worst = (0..=9973).map(|i| i as f32 / 9973.0).map(|v| (lookup(table, v) - linear(code, v)).abs()).fold(0.0, f32::max);
        assert!(worst <= tolerance, "curve {code} is off by {worst}");
    }
    // outside 0..=1 is clamped, not read past the table
    let table = curve_table(1);
    assert_eq!(lookup(table, -3.0), lookup(table, 0.0));
    assert_eq!(lookup(table, 9.0), lookup(table, 1.0));
}

#[test]
fn a_picture_is_reduced_to_within_twice_the_edge_asked() {
    for (w, h, edge) in [(1920, 1080, 512), (3840, 64, 512), (128, 96, 50), (128, 96, 16), (100, 60, 500), (4, 4, 1)] {
        let f = noise(w, h, 8, Color::UNSPECIFIED, 3);
        let within = to_working_within(&f, edge, 0).unwrap();
        let long = within.width.max(within.height);
        assert!(long <= 2 * edge || long == w, "{w}x{h} for {edge}: {}x{}", within.width, within.height);
        assert!(long >= edge || long == w, "{w}x{h} for {edge}: {}x{}", within.width, within.height);
        // `to_working` keeps what it did: at least twice the edge when it can
        let sharp = to_working(&f, edge, 0).unwrap();
        assert!(sharp.width >= within.width && (sharp.width.max(sharp.height) >= 2 * edge || sharp.width == w), "{w}x{h} for {edge}");
    }
}
