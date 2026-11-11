use std::sync::Arc;

use lightcraft_develop::{DepthMap, DepthPalette, DevelopSettings, Relight};
use lightcraft_geom::Orientation;
use lightcraft_raster::{Plane, Rgb32f};

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

#[test]
fn the_gaussian_is_normalized_and_a_flat_depth_stays_flat() {
    let w = gaussian_weights(SMOOTH_SIGMA);
    let sum: f32 = w[0] + 2.0 * w[1..].iter().sum::<f32>();
    assert!((sum - 1.0).abs() < 1e-6, "{sum}");
    assert_eq!(w.len(), 6, "radius of three sigmas");
    let flat = Plane::filled(7, 5, 0.4);
    assert!(smoothed(&flat).data.iter().all(|v| (v - 0.4).abs() < 1e-6));
    // a step is softened, its ends kept
    let step = Plane::from_fn(16, 1, |x, _| if x < 8 { 0.0 } else { 1.0 });
    let s = smoothed(&step);
    assert!(s.data[0] < 1e-4 && s.data[15] > 1.0 - 1e-4 && s.data[7] > 0.2 && s.data[8] < 0.8, "{:?}", s.data);
}

#[test]
fn defaults_and_settings_without_depth_change_nothing() {
    let plain = shot(&DevelopSettings::default());
    // the default (off) is not written, and renders as nothing
    assert!(DevelopSettings::default().to_json().get("relight").is_none());
    assert!(DevelopSettings::default().to_json().get("depth_map").is_none());
    assert_eq!(shot(&with_depth()), plain);
    // on without any depth: nothing to light or draw
    let mut s = DevelopSettings::default();
    s.relight.enabled = true;
    s.depth_map.enabled = true;
    assert_eq!(shot(&s), plain);
    // on with Brightness 0 and all the ambient light: nothing
    let mut s = with_depth();
    s.relight.enabled = true;
    s.relight.brightness = 0.0;
    s.relight.ambient = 100.0;
    assert!(Setup::new(&s, crate::lensblur::mattes_for(&s, None).as_deref(), Orientation::Normal).is_none());
    assert_eq!(shot(&s), plain);
    // its eye off
    let mut s = with_depth();
    s.relight.enabled = true;
    s.set_section_enabled("relight", false);
    assert_eq!(shot(&s), plain);
}

#[test]
fn the_light_brightens_what_faces_it_and_its_place_follows_the_photo() {
    let plain = shot(&with_depth());
    let mut s = with_depth();
    s.relight.enabled = true;
    s.relight.shadows = 0.0;
    s.relight.x = 0.05;
    s.relight.y = 0.9;
    let left = shot(&s);
    s.relight.x = 0.95;
    let right = shot(&s);
    assert_ne!(left, plain);
    // the near ground on the light's side is brighter than on the other
    let (l_left, l_right) = (mean_luma(&left, 0, 12, 52, 64), mean_luma(&left, 84, 96, 52, 64));
    assert!(l_left > l_right + 5.0, "{l_left} vs {l_right}");
    let (r_left, r_right) = (mean_luma(&right, 0, 12, 52, 64), mean_luma(&right, 84, 96, 52, 64));
    assert!(r_right > r_left + 5.0, "{r_left} vs {r_right}");
    // turned a half turn, the light placed at the same point of the photo as shown lights the same corner of the output
    let mut t = s.clone();
    t.orientation = Orientation::Rotate180;
    t.lens_blur.depth = DepthMap::from_nearness(48, 32, &scene_depth(48, 32).into_iter().rev().collect::<Vec<_>>());
    let turned = shot(&t);
    let (u_left, u_right) = (mean_luma(&turned, 0, 12, 52, 64), mean_luma(&turned, 84, 96, 52, 64));
    assert!(u_right > u_left + 5.0, "{u_left} vs {u_right}");
}

/// Card #345: the depth range. Near 0 to Far 10 lights the near block only; the ground (depths 25 to 85) keeps the
/// photo's own light, Ambient included, as with Relight off. The whole range (the default) is not written.
#[test]
fn the_depth_range_limits_the_light_to_its_depths() {
    let plain = shot(&with_depth());
    let mut s = with_depth();
    s.relight.enabled = true;
    s.relight.x = 0.4;
    s.relight.y = 0.5;
    let whole = shot(&s);
    s.relight.far = 10.0;
    let near_only = shot(&s);
    // (the block: x 24..48, rows 16..48; the ground below it)
    let (b_plain, b_lit) = (mean_luma(&plain, 28, 44, 20, 44), mean_luma(&near_only, 28, 44, 20, 44));
    assert!((b_lit - mean_luma(&whole, 28, 44, 20, 44)).abs() < 2.0, "the block is lit as with the whole range");
    assert!((b_lit - b_plain).abs() > 5.0, "the block is lit: {b_lit} vs {b_plain}");
    let (g_plain, g_lit) = (mean_luma(&plain, 60, 96, 52, 64), mean_luma(&near_only, 60, 96, 52, 64));
    assert!((g_lit - g_plain).abs() < 1.0, "the ground keeps its own light: {g_lit} vs {g_plain}");
    assert!((mean_luma(&whole, 60, 96, 52, 64) - g_plain).abs() > 5.0, "lit by the whole range");
    assert!(
        DevelopSettings { relight: Relight { enabled: true, ..Relight::default() }, ..DevelopSettings::default() }.to_json()["relight"]
            .get("near")
            .is_none()
    );
    assert_eq!(s.to_json()["relight"]["far"], 10.0);
}

#[test]
fn the_block_casts_a_shadow_away_from_the_light() {
    let mut s = with_depth();
    s.relight.enabled = true;
    s.relight.x = 0.02;
    s.relight.y = 0.5;
    s.relight.distance = 55.0;
    s.relight.spread = 100.0;
    s.relight.softness = 0.0;
    s.relight.shadows = 0.0;
    let lit = shot(&s);
    s.relight.shadows = 100.0;
    let shaded = shot(&s);
    // just right of the block (x 48..60, rows 24..40): in its shadow
    let (a, b) = (mean_luma(&lit, 50, 60, 26, 38), mean_luma(&shaded, 50, 60, 26, 38));
    assert!(b < a - 5.0, "shadow {b} vs lit {a}");
    // the ground on the light's side of the block is in no shadow
    let (c, d) = (mean_luma(&lit, 4, 18, 26, 38), mean_luma(&shaded, 4, 18, 26, 38));
    assert!((c - d).abs() < 1.5, "{c} vs {d}");
}

#[test]
fn brightness_and_ambient_raise_the_light_and_colour_tints_it() {
    let mut s = with_depth();
    s.relight.enabled = true;
    let at = |f: &dyn Fn(&mut DevelopSettings)| {
        let mut t = s.clone();
        f(&mut t);
        let img = shot(&t);
        mean_luma(&img, 0, W, 0, H)
    };
    let base = at(&|_| {});
    assert!(at(&|t| t.relight.brightness = 100.0) > base + 2.0);
    assert!(at(&|t| t.relight.ambient = 100.0) > base + 2.0);
    assert!(at(&|t| t.relight.ambient = 0.0) < base - 2.0);
    let mut warm = s.clone();
    warm.relight.color = [1.0, 0.5, 0.2];
    let img = shot(&warm);
    let p = img.data[(H - 4) * W + 8];
    assert!(p[0] > p[2] + 10, "{p:?}");
}

#[test]
fn shine_adds_highlights_only_when_asked() {
    let mut s = with_depth();
    s.relight.enabled = true;
    let mattes = crate::lensblur::mattes_for(&s, None);
    let dull = Setup::new(&s, mattes.as_deref(), Orientation::Normal).unwrap();
    assert!(dull.planes(8).1.is_none());
    s.relight.shine = 100.0;
    let shiny = Setup::new(&s, mattes.as_deref(), Orientation::Normal).unwrap();
    let (_, h) = shiny.planes(8);
    assert!(h.unwrap().data.iter().any(|v| *v > 0.01));
    assert_ne!(dull.key, shiny.key);
}

#[test]
fn the_depth_map_draws_the_depth_in_its_palette() {
    let mut s = with_depth();
    s.depth_map.enabled = true;
    let mattes = crate::lensblur::mattes_for(&s, None);
    let look = Look::new(&s, mattes.as_deref()).unwrap();
    // gray: near white, far black, the middle between
    assert_eq!(look.colour(1.0), [1.0; 3]);
    assert_eq!(look.colour(0.0), [0.0; 3]);
    assert!((look.colour(0.5)[0] - 0.5).abs() < 1e-6);
    // the range: past its ends, the ends' colours
    s.depth_map.near = 20.0;
    s.depth_map.far = 60.0;
    let ranged = Look::new(&s, mattes.as_deref()).unwrap();
    assert_eq!(ranged.colour(0.9), [1.0; 3]);
    assert_eq!(ranged.colour(0.3), [0.0; 3]);
    assert!((ranged.colour(0.6)[0] - 0.5).abs() < 1e-5);
    // inverted, and bent by the curve
    s.depth_map.invert = true;
    assert_eq!(Look::new(&s, mattes.as_deref()).unwrap().colour(0.9), [0.0; 3]);
    s.depth_map.invert = false;
    s.depth_map.curve = 100.0;
    assert!(Look::new(&s, mattes.as_deref()).unwrap().colour(0.6)[0] > 0.6);
    // custom colours
    let mut c = with_depth();
    c.depth_map.enabled = true;
    c.depth_map.palette = DepthPalette::Custom;
    c.depth_map.near_color = [1.0, 0.0, 0.0];
    c.depth_map.far_color = [0.0, 0.0, 1.0];
    let custom = Look::new(&c, mattes.as_deref()).unwrap();
    assert_eq!(custom.colour(1.0), [1.0, 0.0, 0.0]);
    assert_eq!(custom.colour(0.0), [0.0, 0.0, 1.0]);
    // contours: lines along equal depths where the depth slopes, as wide as asked, none between them nor on a flat
    c.depth_map.contours = 50.0;
    let lined = Look::new(&c, mattes.as_deref()).unwrap();
    assert_eq!(lined.bands, 16.0);
    let on_line = 10.0 / 16.0;
    let between = 10.5 / 16.0;
    // (a slope of a hundredth of the depth range per pixel: a line every 6.25 pixels)
    assert_eq!(lined.line(on_line, 0.01, 1.0), 1.0);
    assert_eq!(lined.line(between, 0.01, 1.0), 0.0);
    assert_eq!(lined.line(on_line, 0.0, 1.0), 0.0, "flat: no line");
    assert!(lined.line(on_line + 0.0015 / 16.0 * 100.0, 0.01, 1.0) > 0.0, "the line's edge");
    assert_eq!(custom.line(on_line, 0.01, 1.0), 0.0, "no contours asked");
    // a line thinner than a pixel is a pixel wide and fainter: the same ink
    assert_eq!(lined.line(on_line, 0.01, 0.25), 0.5);
    assert_eq!(lined.line(on_line, 0.01, 0.0), 0.0);
}

#[test]
fn a_depth_map_render_shows_near_light_and_far_dark_and_amount_mixes() {
    let mut s = with_depth();
    s.depth_map.enabled = true;
    let img = shot(&s);
    // the block (near) is lighter than the far ground at the top
    assert!(mean_luma(&img, 30, 44, 24, 40) > mean_luma(&img, 60, 90, 0, 6) + 100.0);
    s.depth_map.amount = 50.0;
    let half = shot(&s);
    let plain = shot(&with_depth());
    let (a, b, c) = (mean_luma(&plain, 30, 44, 24, 40), mean_luma(&half, 30, 44, 24, 40), mean_luma(&img, 30, 44, 24, 40));
    assert!(a < b && b < c, "{a} {b} {c}");
}

#[test]
fn an_unknown_palette_reads_as_gray_and_settings_round_trip() {
    let v = serde_json::json!({"depth_map": {"enabled": true, "palette": "aurora", "near": 10.0}});
    let s = DevelopSettings::from_json(&v).unwrap();
    assert_eq!(s.depth_map.palette, DepthPalette::Gray);
    assert_eq!(s.depth_map.near, 10.0);
    let mut t = DevelopSettings::default();
    t.depth_map.enabled = true;
    t.depth_map.palette = DepthPalette::Sunset;
    t.relight.enabled = true;
    t.relight.color = [0.9, 0.7, 0.5];
    let j = t.to_json();
    assert_eq!(j["depth_map"]["palette"], serde_json::json!("sunset"));
    assert_eq!(DevelopSettings::from_json(&j).unwrap(), t);
    assert_ne!(t.hash64(), DevelopSettings::default().hash64());
}

#[test]
fn a_windowed_render_matches_the_whole_one() {
    let mut s = with_depth();
    s.relight.enabled = true;
    s.relight.shine = 30.0;
    s.depth_map.enabled = true;
    s.depth_map.amount = 40.0;
    s.depth_map.palette = DepthPalette::Thermal;
    let src = grey(W, H);
    let whole = render(&src, &SourceInfo::default(), &s, &RenderRequest::fit(W, H)).image;
    let win = crate::PixelWindow { x: 20, y: 10, w: 40, h: 30 };
    let req = RenderRequest { window: Some(win), ..RenderRequest::fit(W, H) };
    let part = render(&src, &SourceInfo::default(), &s, &req).image;
    // (the guided filter that snaps the light to the photo's edges sees only the window, as Lens Blur's does: within
    // three of its radii (here 4 px: two depth pixels) of the window's edges the two differ a little)
    let margin = 12;
    let (mut worst, mut inside) = (0u8, 0u8);
    for y in 0..win.h {
        for x in 0..win.w {
            let (a, b) = (part.data[y * win.w + x], whole.data[(y + win.y) * W + x + win.x]);
            let d = (0..3).map(|k| a[k].abs_diff(b[k])).max().unwrap_or(0);
            worst = worst.max(d);
            if (margin..win.w - margin).contains(&x) && (margin..win.h - margin).contains(&y) {
                inside = inside.max(d);
            }
        }
    }
    assert!(inside <= 1 && worst <= 24, "inside {inside}, worst {worst}");
}

#[test]
fn nan_settings_light_by_the_defaults() {
    let mut s = with_depth();
    s.relight.enabled = true;
    s.relight.x = f64::NAN;
    s.relight.brightness = f64::INFINITY;
    s.depth_map.enabled = true;
    s.depth_map.curve = f64::NAN;
    s.depth_map.near_color = [f64::NAN, 2.0, -1.0];
    s.depth_map.palette = DepthPalette::Custom;
    let img = shot(&s);
    assert_eq!(img.width, W);
    let depth = Arc::new(Plane::filled(1, 1, f32::NAN));
    assert!(smoothed(&depth).data[0].is_nan() || smoothed(&depth).data[0].is_finite());
}

/// A flat depth with the surface model's normals: the left half faces left, the right half right.
fn with_surface() -> DevelopSettings {
    let mut s = DevelopSettings::default();
    s.lens_blur.depth = DepthMap::from_nearness(48, 32, &[0.5; 48 * 32]);
    let normals: Vec<[f32; 3]> = (0..24 * 16).map(|i| if i % 24 < 12 { [-0.7, 0.0, 0.7] } else { [0.7, 0.0, 0.7] }).collect();
    s.relight.normals = lightcraft_develop::NormalMap::from_normals(24, 16, &normals);
    s.relight.enabled = true;
    s.relight.ambient = 0.0;
    s.relight.shadows = 0.0;
    s.relight.spread = 100.0;
    s.relight.y = 0.5;
    s
}

#[test]
fn the_surface_models_normals_shade_where_the_depth_is_flat() {
    let mut s = with_surface();
    let setup = Setup::new(&s, crate::lensblur::mattes_for(&s, None).as_deref(), Orientation::Normal).unwrap();
    assert!(setup.has_normals());
    // a light on the left lights what faces left, one on the right what faces right
    s.relight.x = -0.3;
    let left = shot(&s);
    s.relight.x = 1.3;
    let right = shot(&s);
    let (l_on_left, l_on_right) = (mean_luma(&left, 8, 40, 16, 48), mean_luma(&left, 56, 88, 16, 48));
    let (r_on_left, r_on_right) = (mean_luma(&right, 8, 40, 16, 48), mean_luma(&right, 56, 88, 16, 48));
    assert!(l_on_left > l_on_right + 20.0 && r_on_right > r_on_left + 20.0, "{l_on_left} {l_on_right} {r_on_left} {r_on_right}");
    // Surface 0: flat again, the two halves alike (the light in the middle)
    s.relight.surface = 0.0;
    s.relight.x = 0.5;
    let flat = shot(&s);
    let (a, b) = (mean_luma(&flat, 8, 40, 16, 48), mean_luma(&flat, 56, 88, 16, 48));
    assert!((a - b).abs() < 6.0, "{a} {b}");
    // without them, the flat depth lights both halves alike too
    let mut plain = with_surface();
    plain.relight.normals = None;
    plain.relight.x = 0.5;
    let p = shot(&plain);
    assert!((mean_luma(&p, 8, 40, 16, 48) - mean_luma(&p, 56, 88, 16, 48)).abs() < 12.0);
    // damaged normals are no normals
    let mut bad = with_surface();
    bad.relight.normals = Some(lightcraft_develop::NormalMap { w: 24, h: 16, data: "!!".into() });
    let setup = Setup::new(&bad, crate::lensblur::mattes_for(&bad, None).as_deref(), Orientation::Normal).unwrap();
    assert!(!setup.has_normals());
}

#[test]
fn the_light_stops_at_the_photos_own_edges() {
    const EDGE: usize = 43;
    // a near wall on the left of the depth (48 wide: two output pixels each) under a light, the ground beside it in its
    // shadow; the light fades out over the wall's rim, a few pixels short of the photo's own edge, a dark left half and
    // a bright right half meeting at x = EDGE of 96: snapped, it holds up to that edge and stops there
    let (w, h) = (96usize, 64usize);
    let src = Rgb32f::from_fn(w, h, |x, _| if x < EDGE { [0.04; 3] } else { [0.5; 3] });
    let depth: Vec<f32> = (0..48 * 32).map(|i| if i % 48 < 24 { 0.95 } else { 0.1 }).collect();
    let mut s = DevelopSettings::default();
    s.lens_blur.depth = DepthMap::from_nearness(48, 32, &depth);
    s.relight.enabled = true;
    s.relight.x = 0.25;
    s.relight.y = 0.5;
    s.relight.distance = 55.0;
    s.relight.shadows = 100.0;
    s.relight.softness = 0.0;
    // (the surface model's normals, all facing the camera: no rim where the depth's slope would turn the wall's edge away)
    s.relight.normals = lightcraft_develop::NormalMap::from_normals(48, 32, &[[0.0, 0.0, 1.0]; 48 * 32]);
    let setup = Setup::new(&s, crate::lensblur::mattes_for(&s, None).as_deref(), Orientation::Normal).unwrap();
    let (light, _) = setup.planes(SHADOW_STEPS);
    let frame = crate::plan(&src, &SourceInfo::default(), &s, &RenderRequest::fit(w, h)).frame;
    let plain = source_to_out(&light, &frame, w, h);
    let snapped = crate::lensblur::nearness_out(&light, &frame, w, h, &crate::lensblur::log_luma(&src));
    // the plain light changes most a few pixels short of the edge; the snapped one where the photo's is
    let row = h / 2;
    let step = |p: &Plane, x: usize| (p.data[row * w + x + 1] - p.data[row * w + x]).abs();
    let biggest = |p: &Plane| (40..60).max_by(|a, b| step(p, *a).total_cmp(&step(p, *b))).unwrap_or(0);
    assert_eq!(biggest(&snapped), EDGE - 1, "snapped at {}", biggest(&snapped));
    assert!(snapped.data[row * w + EDGE - 1] > 0.1 && snapped.data[row * w + EDGE] < 0.02);
    assert!(biggest(&plain) < biggest(&snapped), "plain at {}, snapped at {}", biggest(&plain), biggest(&snapped));
}

#[test]
fn the_decoded_surface_and_the_smoothed_depth_are_kept_between_renders() {
    let s = with_surface();
    // the same normals decode once
    let (a, b) = (Normals::of(&s).unwrap(), Normals::of(&s).unwrap());
    assert!(Arc::ptr_eq(&a, &b));
    // the same depth is smoothed once, and gives the same light as a fresh setup
    let mattes = crate::lensblur::mattes_for(&s, None);
    let setup = Setup::new(&s, mattes.as_deref(), Orientation::Normal).unwrap();
    assert!(Arc::ptr_eq(&setup.smoothed(), &setup.smoothed()));
    let first = setup.planes(SHADOW_STEPS).0;
    let again = Setup::new(&s, crate::lensblur::mattes_for(&s, None).as_deref(), Orientation::Normal).unwrap().planes(SHADOW_STEPS).0;
    assert_eq!(first.data, again.data);
    // another depth is smoothed afresh, not taken for the kept one
    let mut other = s.clone();
    other.lens_blur.depth = DepthMap::from_nearness(48, 32, &scene_depth(48, 32));
    let o = Setup::new(&other, crate::lensblur::mattes_for(&other, None).as_deref(), Orientation::Normal).unwrap();
    assert!(!Arc::ptr_eq(&o.smoothed(), &setup.smoothed()));
    assert_ne!(o.smoothed().0.data, setup.smoothed().0.data);
}

/// Card #345: a wall seen at an angle, stored as 8-bit depth (a staircase of flat steps one code apart), lit from the
/// side. Its light is smooth (no bands where the steps are) and the wall does not shadow itself.
#[test]
fn an_8_bit_wall_is_lit_without_bands_and_casts_no_shadow_on_itself() {
    let (w, h) = (1024usize, 768usize);
    let wall: Vec<f32> = (0..w * h)
        .map(|i| {
            let (x, y) = (i % w, i / w);
            ((0.30 + 0.05 * x as f32 / w as f32 + 0.02 * y as f32 / h as f32) * 255.0).round() / 255.0
        })
        .collect();
    let mut m = crate::masks::Mattes::default();
    m.set_depth(Plane { width: w, height: h, data: wall });
    let light = |softness: f64, shadows: f64| {
        let mut s = DevelopSettings::default();
        s.relight.enabled = true;
        (s.relight.x, s.relight.y, s.relight.spread, s.relight.softness, s.relight.shadows) = (0.1, 0.6, 66.0, softness, shadows);
        Setup::new(&s, Some(&m), Orientation::Normal).unwrap().planes(SHADOW_STEPS).0
    };
    for softness in [0.0, 30.0, 100.0] {
        let lit = light(softness, 60.0);
        let unshadowed = light(softness, 0.0);
        let (mut ripple, mut n) = (0.0f64, 0);
        for y in h / 8..7 * h / 8 {
            for x in w / 8..7 * w / 8 {
                let i = y * w + x;
                let v = lit.data[i];
                // (a smooth light is straight over 16 pixels; a step's ripple is not)
                let bend = v - 0.5 * (lit.data[i - 8] + lit.data[i + 8]);
                ripple += f64::from(bend / v.max(1e-3)).powi(2);
                n += 1;
                assert!(
                    (v - unshadowed.data[i]).abs() <= 0.02 * unshadowed.data[i],
                    "softness {softness}: ({x}, {y}) shadowed: {v} vs {}",
                    unshadowed.data[i]
                );
            }
        }
        let ripple = (ripple / f64::from(n)).sqrt();
        assert!(ripple < 0.004, "softness {softness}: ripple {ripple}");
    }
}
