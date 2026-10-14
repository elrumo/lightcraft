use super::*;
use crate::masks::Mattes;
use crate::{PixelWindow, RenderRequest, SourceInfo, render};
use lightcraft_develop::{LensBlur, Mask, MaskComponent, MaskOp};

/// A checkerboard whose left half is near (nearness 1) and right half far (0), with one bright
/// point far away; the depth comes with the source.
fn scene(w: usize, h: usize) -> (Rgb32f, SourceInfo) {
    let img = Rgb32f::from_fn(w, h, |x, y| {
        let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
        if ((fx - 0.75).powi(2) + (fy - 0.5).powi(2)).sqrt() < 0.004 {
            return [8.0; 3];
        }
        let cell = (w / 40).max(1);
        if ((x / cell) + (y / cell)) % 2 == 0 { [0.6, 0.5, 0.4] } else { [0.05, 0.06, 0.08] }
    });
    let mut m = Mattes::default();
    m.set_depth(Plane::from_fn(64, 48, |x, _| if x < 32 { 1.0 } else { 0.0 }));
    (img, SourceInfo { mattes: Some(Arc::new(m)), ..Default::default() })
}

fn blurred(focal_distance: f64) -> DevelopSettings {
    DevelopSettings { lens_blur: LensBlur { boost: 0.0, ..LensBlur::applied(focal_distance) }, ..Default::default() }
}

/// Mean absolute difference between horizontal neighbours over a region (normalized x, y ranges).
fn detail(img: &lightcraft_raster::Rgba8, x: [f32; 2], y: [f32; 2]) -> f32 {
    let (w, h) = (img.width, img.height);
    let (x0, x1) = ((x[0] * w as f32) as usize, (x[1] * w as f32) as usize);
    let (y0, y1) = ((y[0] * h as f32) as usize, (y[1] * h as f32) as usize);
    let mut sum = 0.0;
    let mut n = 0.0;
    for yy in y0..y1 {
        for xx in x0..x1 - 1 {
            let (a, b) = (img.data[yy * w + xx], img.data[yy * w + xx + 1]);
            sum += (a[1] as f32 - b[1] as f32).abs();
            n += 1.0;
        }
    }
    sum / n
}

#[test]
fn normalize_maps_percentiles_to_the_unit_range() {
    let mut p = Plane::from_fn(100, 1, |x, _| x as f32 * 3.0 + 7.0);
    p.data[0] = f32::NAN;
    p.data[99] = 1e9;
    normalize(&mut p);
    assert!(p.data.iter().all(|v| (0.0..=1.0).contains(v)));
    assert_eq!(p.data[0], 0.0);
    assert_eq!(p.data[99], 1.0);
    assert!(p.data[50] > 0.4 && p.data[50] < 0.6, "{}", p.data[50]);
    let mut flat = Plane::from_fn(4, 4, |_, _| 2.0);
    normalize(&mut flat);
    assert!(flat.data.iter().all(|v| v.is_finite()));
}

#[test]
fn the_focal_range_stays_sharp_and_the_rest_blurs_at_any_size() {
    for size in [400usize, 900] {
        let (src, info) = scene(size, size * 3 / 4);
        let req = RenderRequest::fit(size, size);
        let plain = render(&src, &info, &DevelopSettings::default(), &req).image;
        // focus on the near half (depth 0)
        let out = render(&src, &info, &blurred(0.0), &req).image;
        let near = (detail(&plain, [0.05, 0.45], [0.1, 0.9]), detail(&out, [0.05, 0.45], [0.1, 0.9]));
        let far = (detail(&plain, [0.55, 0.95], [0.1, 0.9]), detail(&out, [0.55, 0.95], [0.1, 0.9]));
        assert!(near.1 > near.0 * 0.95, "{size}: the near half kept its detail: {near:?}");
        assert!(far.1 < far.0 * 0.3, "{size}: the far half is blurred: {far:?}");
        // focus on the far half: the other way round
        let out = render(&src, &info, &blurred(100.0), &req).image;
        assert!(detail(&out, [0.55, 0.95], [0.1, 0.4]) > far.0 * 0.95);
        assert!(detail(&out, [0.05, 0.45], [0.1, 0.9]) < near.0 * 0.3);
    }
}

#[test]
fn the_blur_is_the_same_relative_to_the_photo_at_every_size() {
    // the bright point's disc covers the same share of the frame in a small and a large render
    let share = |size: usize| {
        let (src, info) = scene(size, size * 3 / 4);
        let out = render(&src, &info, &blurred(0.0), &RenderRequest::fit(size, size)).image;
        let (w, h) = (out.width, out.height);
        let row: Vec<f32> = (w / 2 + w / 10..w - w / 20).map(|x| out.data[(h / 2) * w + x][1] as f32).collect();
        // the disc: brighter than halfway from the blurred surroundings to its peak
        let peak = row.iter().copied().fold(0.0, f32::max);
        let base = row[0].min(row[row.len() - 1]);
        let lit = row.iter().filter(|&&v| v > (peak + base) / 2.0).count();
        lit as f32 / w as f32
    };
    let (a, b) = (share(500), share(1000));
    assert!(a > 0.02, "the point became a disc: {a}");
    assert!((a - b).abs() < 0.01, "{a} vs {b}");
}

#[test]
fn every_bokeh_spreads_about_the_same_light() {
    let (src, info) = scene(400, 300);
    let req = RenderRequest::fit(400, 400);
    let light = |bokeh: Bokeh| {
        let mut s = blurred(0.0);
        s.lens_blur.bokeh = bokeh;
        s.light.exposure = -3.0;
        let out = render(&src, &info, &s, &req).image;
        let (w, h) = (out.width, out.height);
        let mut sum = 0.0;
        for y in h / 4..3 * h / 4 {
            for x in w / 2 + w / 8..w - w / 16 {
                sum += out.data[y * w + x][1] as f32;
            }
        }
        sum
    };
    let circle = light(Bokeh::Circle);
    for b in [Bokeh::Bubble, Bokeh::Blade5, Bokeh::Ring, Bokeh::CatEye] {
        let l = light(b);
        assert!((l / circle - 1.0).abs() < 0.12, "{b:?}: {l} vs {circle}");
    }
}

#[test]
fn boost_brightens_out_of_focus_highlights() {
    let (src, info) = scene(400, 300);
    let req = RenderRequest::fit(400, 400);
    let mut s = blurred(0.0);
    s.light.exposure = -3.0;
    let plain = render(&src, &info, &s, &req).image;
    s.lens_blur.boost = 100.0;
    let boosted = render(&src, &info, &s, &req).image;
    let at = |i: &lightcraft_raster::Rgba8| i.data[(i.height / 2) * i.width + i.width * 3 / 4][1];
    assert!(at(&boosted) > at(&plain), "{} vs {}", at(&boosted), at(&plain));
}

#[test]
fn without_depth_or_amount_nothing_changes() {
    let (src, mut info) = scene(300, 200);
    let req = RenderRequest::fit(300, 300);
    let plain = render(&src, &info, &DevelopSettings::default(), &req).image;
    let s = blurred(0.0);
    let mut off = s.clone();
    off.lens_blur.amount = 0.0;
    assert_eq!(render(&src, &info, &off, &req).image, plain, "amount 0");
    info.mattes = None;
    assert_eq!(render(&src, &info, &s, &req).image, plain, "no depth");
}

#[test]
fn a_stored_estimate_stands_in_for_the_photos_depth() {
    let (src, info) = scene(300, 200);
    let req = RenderRequest::fit(300, 300);
    let from_source = render(&src, &info, &blurred(0.0), &req).image;
    let mut s = blurred(0.0);
    let v: Vec<f32> = (0..64 * 48).map(|i| if i % 64 < 32 { 1.0 } else { 0.0 }).collect();
    s.lens_blur.depth = lightcraft_develop::DepthMap::from_nearness(64, 48, &v);
    let bare = SourceInfo::default();
    assert_eq!(render(&src, &bare, &s, &req).image, from_source);
}

#[test]
fn hostile_settings_and_depth_never_panic() {
    let (src, info) = scene(120, 90);
    let req = RenderRequest::fit(120, 120);
    for v in [f64::NAN, f64::INFINITY, -1e9, 1e9] {
        let mut s = blurred(0.0);
        (s.lens_blur.amount, s.lens_blur.focal_distance, s.lens_blur.focal_range, s.lens_blur.boost) = (v, v, v, v);
        let _ = render(&src, &info, &s, &req);
    }
    let mut s = blurred(0.0);
    s.lens_blur.depth = Some(lightcraft_develop::DepthMap { w: 3, h: 3, data: "!!".into() });
    let _ = render(&src, &SourceInfo::default(), &s, &req);
    let mut m = Mattes::default();
    m.set_depth(Plane { width: 2, height: 1, data: vec![f32::NAN, 1.0] });
    assert!(m.depth().is_none(), "a depth with NaN is refused");
    m.set_depth(Plane { width: 1, height: 1, data: vec![0.5] });
    let _ = render(&src, &SourceInfo { mattes: Some(Arc::new(m)), ..Default::default() }, &blurred(0.0), &req);
}

#[test]
fn a_zoomed_window_matches_the_whole_render() {
    let (src, info) = scene(600, 450);
    let s = blurred(0.0);
    let whole = render(&src, &info, &s, &RenderRequest::fit(600, 600)).image;
    let win = PixelWindow { x: 250, y: 100, w: 200, h: 150 };
    let part = render(&src, &info, &s, &RenderRequest { window: Some(win), ..RenderRequest::fit(600, 600) }).image;
    assert_eq!((part.width, part.height), (win.w, win.h));
    let mut worst = (0i32, 0, 0);
    for y in 0..win.h {
        for x in 0..win.w {
            let (a, b) = (part.data[y * win.w + x], whole.data[(y + win.y) * whole.width + x + win.x]);
            let d = (0..3).map(|c| (a[c] as i32 - b[c] as i32).abs()).max().unwrap_or(0);
            if d > worst.0 {
                worst = (d, x, y);
            }
        }
    }
    if let Ok(dir) = std::env::var("LIGHTCRAFT_TEST_OUT") {
        let save = |i: &lightcraft_raster::Rgba8, n: &str| {
            let raw: Vec<u8> = i.data.iter().flatten().copied().collect();
            image::save_buffer(format!("{dir}/{n}.png"), &raw, i.width as u32, i.height as u32, image::ColorType::Rgba8).unwrap();
        };
        save(&whole, "whole");
        save(&part, "part");
    }
    assert!(worst.0 <= 24, "worst difference {worst:?}");
}

#[test]
fn a_depth_range_mask_selects_by_depth() {
    let (src, info) = scene(200, 150);
    let mut s = DevelopSettings::default();
    s.masks.push(Mask {
        id: 1,
        components: vec![MaskComponent {
            name: None,
            op: MaskOp::Add,
            invert: false,
            shape: MaskShape::DepthRange { lo: 0.5, hi: 1.0, feather: 0.05 },
        }],
        ..Default::default()
    });
    s.masks[0].adjust.exposure = -2.0;
    let plain = render(&src, &info, &DevelopSettings::default(), &RenderRequest::fit(200, 200)).image;
    let out = render(&src, &info, &s, &RenderRequest::fit(200, 200)).image;
    let sum = |i: &lightcraft_raster::Rgba8, x0: usize, x1: usize| -> u64 {
        (0..i.height).flat_map(|y| (x0..x1).map(move |x| (y, x))).map(|(y, x)| i.data[y * i.width + x][1] as u64).sum()
    };
    // the far (right) half darkened, the near (left) half left alone
    assert!(sum(&out, 110, 190) < sum(&plain, 110, 190) * 3 / 4);
    assert!(sum(&out, 10, 90) * 100 >= sum(&plain, 10, 90) * 97);
}

#[test]
fn refine_strokes_sharpen_and_blur_where_painted() {
    let (src, info) = scene(400, 300);
    let req = RenderRequest::fit(400, 400);
    let plain = render(&src, &info, &DevelopSettings::default(), &req).image;
    let stroke = |x: f64| lightcraft_develop::BrushStroke {
        points: vec![lightcraft_geom::Point::new(x, 0.3), lightcraft_geom::Point::new(x, 0.7)],
        size: 0.12,
        feather: 0.0,
        ..Default::default()
    };
    // focus on the near half; paint the far half sharp and the near half blurred
    let mut s = blurred(0.0);
    s.lens_blur.focus = vec![stroke(0.75)];
    s.lens_blur.blur = vec![stroke(0.25)];
    let out = render(&src, &info, &s, &req).image;
    let (far_plain, far_out) = (detail(&plain, [0.7, 0.8], [0.4, 0.6]), detail(&out, [0.7, 0.8], [0.4, 0.6]));
    let (near_plain, near_out) = (detail(&plain, [0.2, 0.3], [0.4, 0.6]), detail(&out, [0.2, 0.3], [0.4, 0.6]));
    assert!(far_out > far_plain * 0.9, "the Focus stroke kept the far half sharp: {far_out} vs {far_plain}");
    assert!(near_out < near_plain * 0.4, "the Blur stroke blurred the near half: {near_out} vs {near_plain}");
    // and outside the strokes nothing changed from the unpainted blur
    let unpainted = render(&src, &info, &blurred(0.0), &req).image;
    assert!((detail(&out, [0.02, 0.08], [0.05, 0.15]) - detail(&unpainted, [0.02, 0.08], [0.05, 0.15])).abs() < 1.0);
}
