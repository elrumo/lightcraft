//! Big originals held in half floats ([`Rgb16f`]): their renders are the float source's within one 8-bit level, and a
//! window of one (a zoomed view, an export's band) reads only its part of the picture, widened exactly.

use lightcraft_develop::{DevelopSettings, controls};
use lightcraft_geom::Orientation;
use lightcraft_raster::{PixelView, Rgb16f, Rgb32f, Rgba8};

use crate::{PixelWindow, RenderRequest, SourceInfo, render};

/// A photo-like scene as a raw develops one (highlights past 1, deep shadows) and as an 8-bit file gives one.
fn sources() -> [(&'static str, Rgb32f); 2] {
    let scene = lightcraft_scenes::demo_library()[0].render(600, 400);
    let raw = scene.map(|p| p.map(|v| v * 2.5));
    let file = scene.map(|p| {
        p.map(|v| lightcraft_color::transfer::srgb_to_linear((lightcraft_color::transfer::linear_to_srgb(v.clamp(0.0, 1.0)) * 255.0).round() / 255.0))
    });
    [("raw", raw), ("8-bit file", file)]
}

/// The edits that push the picture's values hardest: exposure, contrast and tone, local contrast, dehaze, sharpening.
fn edits() -> Vec<(&'static str, DevelopSettings)> {
    let set = |pairs: &[(&str, f64)]| {
        let mut s = DevelopSettings::default();
        for (id, v) in pairs {
            assert!(controls::set(&mut s, id, *v), "{id}");
        }
        s
    };
    vec![
        ("default", DevelopSettings::default()),
        ("+3 EV, shadows", set(&[("light.exposure", 3.0), ("light.shadows", 100.0), ("light.blacks", 40.0)])),
        ("-2 EV, highlights", set(&[("light.exposure", -2.0), ("light.highlights", -100.0), ("light.whites", -60.0), ("light.contrast", 80.0)])),
        ("local contrast", set(&[("effects.clarity", 80.0), ("effects.texture", 80.0), ("effects.dehaze", 60.0), ("color.saturation", 60.0)])),
        ("sharpened", set(&[("detail.sharpenAmount", 150.0), ("detail.sharpenRadius", 1.0), ("detail.nrLuminance", 40.0)])),
    ]
}

/// Largest per-channel difference and how many channel values differ.
fn diff(a: &Rgba8, b: &Rgba8) -> (u8, usize) {
    assert_eq!((a.width, a.height), (b.width, b.height));
    a.data
        .iter()
        .zip(&b.data)
        .flat_map(|(p, q)| (0..3).map(move |k| p[k].abs_diff(q[k])))
        .fold((0, 0), |(m, n), d| (m.max(d), n + usize::from(d > 0)))
}

// Given a picture held in half floats, then its 8-bit renders (whole at its own size, fitted, and a window at 1:1) are
// the float picture's within one level, whatever the edit
#[test]
fn compact_sources_render_like_floats() {
    let info = SourceInfo::default();
    for (what, src) in sources() {
        let half = Rgb16f::from_f32(&src);
        for (name, s) in edits() {
            let window = PixelWindow { x: 170, y: 90, w: 260, h: 180 };
            for (size, req) in [
                ("1:1", RenderRequest::fit(600, 600)),
                ("fit", RenderRequest::fit(256, 256)),
                ("window", RenderRequest { window: Some(window), ..RenderRequest::fit(600, 600) }),
            ] {
                let (a, b) = (render(&src, &info, &s, &req).image, render(&half, &info, &s, &req).image);
                let (max, n) = diff(&a, &b);
                assert!(max <= 1, "{what}, {name}, {size}: {max} levels apart ({n} values differ)");
            }
        }
    }
}

// Given a picture held in half floats, then a window of it (upright or turned, at 1:1 or smaller) is the very render
// of the picture widened whole: the part it reads is widened exactly, and nothing else is read
#[test]
fn a_window_of_a_compact_source_is_the_widened_pictures_window() {
    let (_, src) = sources().into_iter().next().unwrap_or_else(|| ("", Rgb32f::new(1, 1)));
    let half = Rgb16f::from_f32(&src);
    let wide = half.widen();
    let info = SourceInfo::default();
    for orientation in [Orientation::Normal, Orientation::Rotate90] {
        let mut s = DevelopSettings { orientation, ..Default::default() };
        assert!(controls::set(&mut s, "effects.clarity", 40.0) && controls::set(&mut s, "light.shadows", 50.0));
        for (fit, win) in [(600, PixelWindow { x: 100, y: 120, w: 200, h: 150 }), (450, PixelWindow { x: 0, y: 30, w: 450, h: 64 })] {
            let req = RenderRequest { window: Some(win), ..RenderRequest::fit(fit, fit) };
            let (a, b) = (render(&wide, &info, &s, &req).image, render(PixelView::from(&half), &info, &s, &req).image);
            assert!(a.data == b.data, "{orientation:?}, {fit}: {:?}", diff(&a, &b));
        }
    }
}
