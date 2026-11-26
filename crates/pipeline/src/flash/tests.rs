use lightcraft_develop::{DepthMap, DevelopSettings, Flash};
use lightcraft_geom::Orientation;
use lightcraft_raster::Rgb32f;

use super::*;
use crate::{RenderRequest, SourceInfo, render};

const W: usize = 96;
const H: usize = 64;

/// A ground from far (top) to near (bottom) with a near block left of the middle.
fn scene_depth(w: usize, h: usize) -> Vec<f32> {
    (0..w * h)
        .map(|i| {
            let (x, y) = (i % w, i / w);
            let ground = 0.15 + 0.6 * y as f32 / (h - 1) as f32;
            let block = x >= w / 4 && x < w / 2 && y >= h / 4 && y < 3 * h / 4;
            if block { 0.95 } else { ground }
        })
        .collect()
}

fn grey(w: usize, h: usize) -> Rgb32f {
    Rgb32f::filled(w, h, [0.18; 3])
}

fn with_depth() -> DevelopSettings {
    let mut s = DevelopSettings::default();
    s.lens_blur.depth = DepthMap::from_nearness(48, 32, &scene_depth(48, 32));
    s
}

fn flashed() -> DevelopSettings {
    let mut s = with_depth();
    s.flash.enabled = true;
    s
}

fn shot(s: &DevelopSettings) -> lightcraft_raster::Rgba8 {
    render(&grey(W, H), &SourceInfo::default(), s, &RenderRequest::fit(W, H)).image
}

fn luma(p: [u8; 4]) -> f64 {
    0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2])
}

fn mean_luma(img: &lightcraft_raster::Rgba8, x0: usize, x1: usize, y0: usize, y1: usize) -> f64 {
    let mut sum = 0.0;
    for y in y0..y1 {
        for x in x0..x1 {
            sum += luma(img.data[y * img.width + x]);
        }
    }
    sum / ((x1 - x0) * (y1 - y0)) as f64
}

/// The block's pixels (the nearest part), the far ground at the top, and the ground in front of the block.
fn regions(img: &lightcraft_raster::Rgba8) -> (f64, f64, f64) {
    (
        mean_luma(img, W / 4 + 4, W / 2 - 4, H / 4 + 6, 3 * H / 4 - 6),
        mean_luma(img, W / 2, W - 4, 4, H / 8),
        mean_luma(img, 4, W - 4, 7 * H / 8, H - 2),
    )
}

#[test]
fn flash_off_changes_nothing_and_on_without_depth_does_nothing() {
    let plain = shot(&DevelopSettings::default());
    assert_eq!(shot(&with_depth()), plain, "depth alone is not a look");
    let mut no_depth = DevelopSettings::default();
    no_depth.flash.enabled = true;
    assert_eq!(shot(&no_depth), plain, "no depth, nothing to light by");
    let mut eye_off = flashed();
    eye_off.disabled_sections.push("flash".into());
    assert_eq!(shot(&eye_off), shot(&with_depth()), "the section's eye");
    assert_ne!(shot(&flashed()), plain);
}

#[test]
fn the_flash_lights_what_is_near_and_leaves_the_far_ground_dark() {
    let lit = shot(&flashed());
    let (block, far, front) = regions(&lit);
    assert!(block > far + 40.0, "the near block {block} against the far ground {far}");
    assert!(front > far + 20.0, "the ground in front {front} against the far ground {far}");
}

#[test]
fn ambient_and_power_move_the_far_ground_and_the_subject() {
    let at = |f: fn(&mut Flash)| {
        let mut s = flashed();
        f(&mut s.flash);
        regions(&shot(&s))
    };
    let (_, far_dim, _) = at(|f| f.ambient = 0.0);
    let (_, far_kept, _) = at(|f| f.ambient = 100.0);
    assert!(far_kept > far_dim + 30.0, "ambient 100 {far_kept} against 0 {far_dim}");
    let (block_low, ..) = at(|f| f.power = 20.0);
    let (block_high, ..) = at(|f| f.power = 100.0);
    assert!(block_high > block_low + 5.0, "power 100 {block_high} against 20 {block_low}");
}

#[test]
fn the_depth_range_keeps_the_photos_own_light_outside_it() {
    let plain = shot(&with_depth());
    let mut s = flashed();
    // the whole scene is nearer than depth 100: a range that ends at 0 reaches nothing (the block is at depth 5)
    s.flash.far = 0.0;
    s.flash.near = 0.0;
    let out = shot(&s);
    let (_, far_plain, _) = regions(&plain);
    let (_, far_out, _) = regions(&out);
    assert!((far_plain - far_out).abs() < 3.0, "outside the range the photo keeps its light: {far_plain} against {far_out}");
}

#[test]
fn lamps_keep_their_light_with_keep_lights_up() {
    let bright = |emit: f64| {
        let mut s = flashed();
        s.flash.emit = emit;
        s.flash.ambient = 0.0;
        let img = Rgb32f::filled(W, H, [0.95; 3]);
        let out = render(&img, &SourceInfo::default(), &s, &RenderRequest::fit(W, H)).image;
        mean_luma(&out, 4, W - 4, 4, H / 8)
    };
    assert!(bright(100.0) > bright(0.0) + 30.0, "{} against {}", bright(100.0), bright(0.0));
}

#[test]
fn the_noise_has_zero_mean_and_unit_variance() {
    let v: Vec<f32> = (0..64).flat_map(|y| (0..64).map(move |x| grain_noise(x, y))).collect();
    let mean = v.iter().sum::<f32>() / v.len() as f32;
    let var = v.iter().map(|z| (z - mean).powi(2)).sum::<f32>() / v.len() as f32;
    assert!(mean.abs() < 0.08, "{mean}");
    assert!((var - 1.0).abs() < 0.15, "{var}");
    assert_eq!(grain_noise(3, 9), grain_noise(3, 9), "the same cell, the same noise");
    // per channel: unit variance in each, and the channels are alike in luminance only partly
    let c: Vec<V3> = (0..64).flat_map(|y| (0..64).map(move |x| grain_noise3(x, y))).collect();
    for k in 0..3 {
        let m = c.iter().map(|z| z[k]).sum::<f32>() / c.len() as f32;
        let v = c.iter().map(|z| (z[k] - m).powi(2)).sum::<f32>() / c.len() as f32;
        assert!(m.abs() < 0.1 && (v - 0.81).abs() < 0.2, "channel {k}: mean {m} variance {v}");
    }
    let cov = c.iter().map(|z| z[0] * z[1]).sum::<f32>() / c.len() as f32;
    assert!(cov > 0.3 && cov < 0.75, "the channels share the luminance part: {cov}");
}

#[test]
fn a_far_field_with_relief_is_flattened_and_the_subject_is_found() {
    let (w, h) = (64usize, 48usize);
    // a far field with a ripple (a sky's relief), a near block in the middle
    let data: Vec<f32> = (0..w * h)
        .map(|i| {
            let (x, y) = (i % w, i / w);
            if (16..44).contains(&x) && (8..40).contains(&y) { 0.9 } else { 0.1 + 0.08 * ((x as f32 * 0.7).sin() * (y as f32 * 0.9).cos()) }
        })
        .collect();
    let depth = Plane { width: w, height: h, data };
    let p = Prepared::new(&depth);
    let spread = |q: &Plane| {
        let v: Vec<f32> = (0..w * 10).map(|i| q.data[i]).collect();
        let m = v.iter().sum::<f32>() / v.len() as f32;
        v.iter().map(|a| (a - m).powi(2)).sum::<f32>() / v.len() as f32
    };
    assert!(spread(&p.n) < spread(&p.d) * 0.3, "the far field's relief: {} against {}", spread(&p.n), spread(&p.d));
    assert!(p.subject > 0.5, "the subject is the near block: {}", p.subject);
    assert!(p.n.data.iter().all(|v| (0.0..=1.0).contains(v)));
}

#[test]
fn the_flash_direction_is_a_unit_vector_that_follows_the_orientation() {
    let s = flashed();
    let mattes = crate::lensblur::mattes_for(&s, None);
    let dir = |o: Orientation| Setup::new(&s, mattes.as_deref(), o).unwrap().direction;
    for o in [Orientation::Normal, Orientation::Rotate90, Orientation::Rotate180, Orientation::Rotate270] {
        let d = dir(o);
        assert!(((d[0] * d[0] + d[1] * d[1]) - 1.0).abs() < 1e-4, "{o:?}: {d:?}");
    }
    let (a, b) = (dir(Orientation::Normal), dir(Orientation::Rotate90));
    assert!((a[0] - b[0]).abs() + (a[1] - b[1]).abs() > 0.5, "a turned photo turns the flash: {a:?} {b:?}");
    let (c, d) = (dir(Orientation::Normal), dir(Orientation::Rotate180));
    assert!((c[0] + d[0]).abs() + (c[1] + d[1]).abs() < 1e-3, "half a turn reverses it: {c:?} {d:?}");
}

#[test]
fn the_shadow_falls_away_from_the_flash() {
    // a depth map big enough for a shadow of several pixels (the shadow is as long as the block is near, in rays that rise a
    // whole depth range per `reach_len` long edges)
    let mut s = DevelopSettings::default();
    s.lens_blur.depth = DepthMap::from_nearness(192, 128, &scene_depth(192, 128));
    s.flash.enabled = true;
    // the flash to the right of the lens: the block's shadow falls to its left
    s.flash.angle = 0.0;
    s.flash.offset = 100.0;
    let mattes = crate::lensblur::mattes_for(&s, None);
    let setup = Setup::new(&s, mattes.as_deref(), Orientation::Normal).unwrap();
    let [_, lit, ..] = setup.planes(SHADOW_STEPS);
    let lit = lit.unwrap();
    let (w, h) = (lit.width, lit.height);
    let (left, right) = (lit.get_clamped((w / 4) as isize - 4, (h / 2) as isize), lit.get_clamped((w / 2) as isize + 4, (h / 2) as isize));
    assert!(left < 0.7, "left of the block, away from the flash: {left}");
    assert!(right > 0.95, "right of the block, toward the flash: {right}");
    s.flash.shadow = 0.0;
    let setup = Setup::new(&s, mattes.as_deref(), Orientation::Normal).unwrap();
    assert!(setup.planes(SHADOW_STEPS)[1].as_ref().unwrap().data.iter().all(|v| *v == 1.0), "no shadow, nothing marched");
}

#[test]
fn every_pixel_comes_out_finite_and_in_range_for_hostile_numbers() {
    let setup = {
        let s = flashed();
        let mattes = crate::lensblur::mattes_for(&s, None);
        Setup::new(&s, mattes.as_deref(), Orientation::Normal).unwrap()
    };
    for c in [[0.0; 3], [1e6; 3], [-1.0, 0.5, 2.0], [f32::MAX, 0.0, 0.0]] {
        for n in [0.0, 1.0, -3.0, 7.0] {
            let out = pixel(&setup, c, n, 0.5, 0.0, 1.0, 2.0, [3.0; 3], -2.0, -3.0);
            assert!(out.iter().all(|v| v.is_finite() && *v >= 0.0 && *v <= 1.0 + 1e-4), "{c:?} {n}: {out:?}");
        }
    }
}

/// A flat wall `wall` deep with a near block standing in front of it (the middle third across, the middle half down), `w` x `h`.
fn wall_and_block(wall: f32, w: usize, h: usize) -> Vec<f32> {
    (0..w * h).map(|i| if (w / 3..2 * w / 3).contains(&(i % w)) && (h / 4..3 * h / 4).contains(&(i / w)) { 0.95 } else { wall }).collect()
}

#[test]
fn a_shadow_is_not_seen_on_a_wall_far_behind_the_subject_but_is_on_a_near_one() {
    // the left of the block, where a flash to the right of the lens throws its shadow
    let side = |s: &DevelopSettings| mean_luma(&shot(s), 27, 31, 22, 42);
    for (wall, seen) in [(0.08f32, false), (0.6, true)] {
        let mut a = DevelopSettings::default();
        a.lens_blur.depth = DepthMap::from_nearness(192, 128, &wall_and_block(wall, 192, 128));
        a.flash = Flash { enabled: true, shadow: 100.0, offset: 100.0, angle: 0.0, fill: 0.0, grain: 0.0, ..Flash::default() };
        let mut b = a.clone();
        b.flash.shadow = 0.0;
        let drop = side(&b) - side(&a);
        if seen {
            assert!(drop > 2.0, "a wall {wall} deep shows the shadow: {drop}");
        } else {
            assert!(drop.abs() < 1.0, "a wall {wall} deep is too far for one: {drop}");
        }
    }
}

#[test]
fn a_step_too_small_to_stand_clear_casts_no_shadow() {
    // a bump 0.08 above the ground (a nose on a cheek) all over the middle
    let (w, h) = (192usize, 128usize);
    let data: Vec<f32> =
        (0..w * h).map(|i| if (w / 3..2 * w / 3).contains(&(i % w)) && (h / 4..3 * h / 4).contains(&(i / w)) { 0.58 } else { 0.5 }).collect();
    let mut s = DevelopSettings::default();
    s.lens_blur.depth = DepthMap::from_nearness(w, h, &data);
    s.flash = Flash { enabled: true, shadow: 100.0, offset: 100.0, angle: 0.0, ..Flash::default() };
    let mattes = crate::lensblur::mattes_for(&s, None);
    let setup = Setup::new(&s, mattes.as_deref(), Orientation::Normal).unwrap();
    let lit = setup.planes(SHADOW_STEPS)[1].clone().unwrap();
    assert!(lit.data.iter().all(|v| *v > 0.99), "the least light {}", lit.data.iter().copied().fold(1.0f32, f32::min));
}

#[test]
fn fill_shadows_lifts_the_shade_on_what_the_flash_lights_and_leaves_the_rest() {
    // a near block (what the flash lights) with a dark patch on it, over a far ground
    let (dw, dh) = (96usize, 64usize);
    let data: Vec<f32> = (0..dw * dh).map(|i| if (16..80).contains(&(i % dw)) && (8..56).contains(&(i / dw)) { 0.95 } else { 0.1 }).collect();
    let mut img = Rgb32f::filled(W, H, [0.2; 3]);
    for y in 24..40 {
        for x in 40..56 {
            img.data[y * W + x] = [0.03; 3];
        }
    }
    let render_with = |fill: f64| {
        let mut s = DevelopSettings::default();
        s.lens_blur.depth = DepthMap::from_nearness(dw, dh, &data);
        s.flash = Flash { enabled: true, fill, grain: 0.0, gloss: 0.0, ..Flash::default() };
        render(&img, &SourceInfo::default(), &s, &RenderRequest::fit(W, H)).image
    };
    let (flat, lifted) = (render_with(0.0), render_with(100.0));
    let patch = |i: &lightcraft_raster::Rgba8| mean_luma(i, 44, 52, 28, 36);
    let lit = |i: &lightcraft_raster::Rgba8| mean_luma(i, 20, 34, 14, 22);
    assert!(patch(&lifted) > patch(&flat) + 8.0, "the shade is lifted: {} against {}", patch(&lifted), patch(&flat));
    assert!((lit(&lifted) - lit(&flat)).abs() < 3.0, "the lit part stays: {} against {}", lit(&lifted), lit(&flat));
}

#[test]
fn at_power_zero_a_glint_and_the_fill_are_gone_with_the_flash() {
    // a near, flat block (what a flash lights and makes glints on) with a dark patch and a few bright dots on it: Gloss and Fill Shadows are the flash's
    // own light, so with no Power the picture is the same whatever they are set to
    let (dw, dh) = (96usize, 64usize);
    let data: Vec<f32> = (0..dw * dh).map(|i| if (16..80).contains(&(i % dw)) && (8..56).contains(&(i / dw)) { 0.95 } else { 0.1 }).collect();
    let mut img = Rgb32f::filled(W, H, [0.2; 3]);
    for y in 24..40 {
        for x in 40..56 {
            img.data[y * W + x] = [0.03; 3];
        }
    }
    for (x, y) in [(24, 20), (30, 44), (60, 18), (66, 46), (26, 32)] {
        img.data[y * W + x] = [0.9; 3];
    }
    let render_with = |power: f64, gloss: f64, fill: f64| {
        let mut s = DevelopSettings::default();
        s.lens_blur.depth = DepthMap::from_nearness(dw, dh, &data);
        s.flash = Flash { enabled: true, power, gloss, fill, grain: 0.0, ..Flash::default() };
        render(&img, &SourceInfo::default(), &s, &RenderRequest::fit(W, H)).image
    };
    let (bare, dressed) = (render_with(0.0, 0.0, 0.0), render_with(0.0, 100.0, 100.0));
    assert_eq!(bare.data, dressed.data, "no Power, no glint and no fill");
    // and with Power they are there
    let (a, b) = (render_with(50.0, 0.0, 0.0), render_with(50.0, 100.0, 100.0));
    assert_ne!(a.data, b.data, "with Power they show");
}
