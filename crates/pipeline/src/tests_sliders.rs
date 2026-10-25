//! The slider sweep (FB-SLIDER): every develop control spec must visibly change the render when it moves away
//! from its default, at both ends of its range and a quarter of the way (no dead zone), more the further it goes,
//! in the direction its name promises, and must give the untouched render back, bit for bit, when it is set to its
//! default. The same holds for a mask's local sliders. A control that legitimately can't change the synthetic
//! scene is in [`EXEMPT`] with its reason: a new control without one fails here, so no slider can silently stop
//! working again.

use lightcraft_develop::{
    DevelopSettings, EmbeddedLens, EmbeddedVignette, EmbeddedWarp, LocalAdjustments, Mask, MaskComponent, MaskOp, MaskShape, controls,
};
use lightcraft_geom::Point;
use lightcraft_raster::{Rgb32f, Rgba8};

use crate::{RenderRequest, SourceInfo, render};

const W: usize = 384;
const H: usize = 256;

/// Controls the synthetic scene can't show, and why.
const EXEMPT: &[(&str, &str)] = &[
    ("enhance.denoise", "mixes in the picture the AI denoise model makes (a model download); without that picture the render is the unedited one"),
    ("hdr.maxEv", "the headroom limit shapes only the HDR rendition (an F32Hdr render, tested in tests_hdr.rs); every SDR render ignores it"),
];

/// Known dead stretches of a slider (`id`, end, fraction of the way to it), each with its reason. A stretch fixed
/// is taken off this list; a new one is a failure.
const KNOWN_DEAD: &[(&str, &str, f64, &str)] = &[(
    "detail.sharpenRadius",
    "min",
    0.25,
    "the blur is a Gaussian made of whole-pixel boxes (`lightcraft_raster::blur`), so radii from 0.58 to 1.04 px are the same blur; \
     Radius 0.82–1.0 shows nothing (a blend of two neighbouring box blurs would make it continuous)",
)];

fn scene() -> Rgb32f {
    lightcraft_scenes::sweep_scene(W, H)
}

/// Grain cells are never finer than 0.6 px of the output, so Grain Size shows only on a picture wider than a
/// thumbnail: it is swept on a bigger one.
fn big_scene() -> Rgb32f {
    lightcraft_scenes::sweep_scene(1280, 854)
}

/// What a control needs besides settings: a lens profile for the Profile Corrections amounts.
fn info_for(id: &str) -> SourceInfo {
    if !id.starts_with("optics.profile") {
        return SourceInfo::default();
    }
    let centre = Point::new(0.5, 0.5);
    let lens = EmbeddedLens {
        warp: Some(EmbeddedWarp { planes: [[1.0, 0.08, 0.0, 0.0, 0.0, 0.0]; 3], center: centre, radius: 0.6 }),
        vignette: Some(EmbeddedVignette { k: [0.5, 0.2, 0.0, 0.0, 0.0], center: centre, radius: 0.6 }),
        ..Default::default()
    };
    SourceInfo { lens: Some(lens), ..Default::default() }
}

fn shot(src: &Rgb32f, info: &SourceInfo, s: &DevelopSettings) -> Rgba8 {
    render(src, info, s, &RenderRequest::fit(src.width, src.height)).image
}

/// How many 8-bit channel values differ.
fn changed(a: &Rgba8, b: &Rgba8) -> usize {
    assert_eq!((a.width, a.height), (b.width, b.height));
    a.data.iter().zip(&b.data).map(|(p, q)| (0..3).filter(|i| p[*i] != q[*i]).count()).sum()
}

/// Mean absolute difference over the colour channels, in 8-bit steps.
fn distance(a: &Rgba8, b: &Rgba8) -> f64 {
    let sum: u64 = a.data.iter().zip(&b.data).map(|(p, q)| (0..3).map(|i| u64::from(p[i].abs_diff(q[i]))).sum::<u64>()).sum();
    sum as f64 / (a.data.len().max(1) * 3) as f64
}

#[derive(Clone, Copy, Debug)]
enum Metric {
    /// Mean luma.
    Luma,
    /// Standard deviation of luma (contrast).
    Contrast,
    /// Mean of max(R,G,B) − min(R,G,B) (colourfulness).
    Chroma,
    /// Mean R − B (warmth).
    Warmth,
    /// Mean G − (R + B) / 2 (green over magenta).
    Green,
    /// Mean luma step between neighbouring pixels in the textured ground (fine detail and noise).
    Detail,
}

fn luma(p: [u8; 4]) -> f64 {
    0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2])
}

fn measure(m: Metric, img: &Rgba8) -> f64 {
    let n = img.data.len() as f64;
    match m {
        Metric::Luma => img.data.iter().map(|p| luma(*p)).sum::<f64>() / n,
        Metric::Contrast => {
            let mean = img.data.iter().map(|p| luma(*p)).sum::<f64>() / n;
            (img.data.iter().map(|p| (luma(*p) - mean).powi(2)).sum::<f64>() / n).sqrt()
        }
        Metric::Chroma => {
            img.data
                .iter()
                .map(|p| f64::from(p[..3].iter().max().copied().unwrap_or(0)) - f64::from(p[..3].iter().min().copied().unwrap_or(0)))
                .sum::<f64>()
                / n
        }
        Metric::Warmth => img.data.iter().map(|p| f64::from(p[0]) - f64::from(p[2])).sum::<f64>() / n,
        Metric::Green => img.data.iter().map(|p| f64::from(p[1]) - (f64::from(p[0]) + f64::from(p[2])) / 2.0).sum::<f64>() / n,
        Metric::Detail => {
            // (the ground: the bottom quarter, left of the bright edge)
            let (y0, x1) = (img.height * 78 / 100, img.width * 70 / 100);
            let (mut sum, mut count) = (0.0, 0usize);
            for y in y0..img.height {
                for x in 1..x1 {
                    sum += (luma(img.data[y * img.width + x]) - luma(img.data[y * img.width + x - 1])).abs();
                    count += 1;
                }
            }
            sum / count.max(1) as f64
        }
    }
}

/// What a control's name promises: the metric rises as its value rises (`1`) or falls (`-1`).
const DIRECTION: &[(&str, Metric, i8)] = &[
    ("light.exposure", Metric::Luma, 1),
    ("light.contrast", Metric::Contrast, 1),
    ("light.highlights", Metric::Luma, 1),
    ("light.shadows", Metric::Luma, 1),
    ("light.whites", Metric::Luma, 1),
    ("light.blacks", Metric::Luma, 1),
    ("curve.highlights", Metric::Luma, 1),
    ("curve.lights", Metric::Luma, 1),
    ("curve.darks", Metric::Luma, 1),
    ("curve.shadows", Metric::Luma, 1),
    ("color.vibrance", Metric::Chroma, 1),
    ("color.saturation", Metric::Chroma, 1),
    ("wb.temp", Metric::Warmth, 1),
    ("wb.tint", Metric::Green, -1),
    ("effects.dehaze", Metric::Chroma, 1),
    ("effects.texture", Metric::Detail, 1),
    ("vignette.amount", Metric::Luma, 1),
    ("grain.amount", Metric::Detail, 1),
    ("detail.sharpenAmount", Metric::Detail, 1),
    ("detail.nrLuminance", Metric::Detail, -1),
    ("optics.vignetting", Metric::Luma, 1),
    ("grading.global.lum", Metric::Luma, 1),
    ("grading.shadows.lum", Metric::Luma, 1),
    ("grading.midtones.lum", Metric::Luma, 1),
    ("grading.highlights.lum", Metric::Luma, 1),
    ("grading.global.sat", Metric::Chroma, 1),
    ("mixer.red.sat", Metric::Chroma, 1),
    ("mixer.orange.sat", Metric::Chroma, 1),
    ("mixer.yellow.sat", Metric::Chroma, 1),
    ("mixer.green.sat", Metric::Chroma, 1),
    ("mixer.aqua.sat", Metric::Chroma, 1),
    ("mixer.blue.sat", Metric::Chroma, 1),
    ("mixer.purple.sat", Metric::Chroma, 1),
    ("mixer.magenta.sat", Metric::Chroma, 1),
    ("mixer.red.lum", Metric::Luma, 1),
    ("mixer.orange.lum", Metric::Luma, 1),
    ("mixer.yellow.lum", Metric::Luma, 1),
    ("mixer.green.lum", Metric::Luma, 1),
    ("mixer.aqua.lum", Metric::Luma, 1),
    ("mixer.blue.lum", Metric::Luma, 1),
    ("mixer.purple.lum", Metric::Luma, 1),
    ("mixer.magenta.lum", Metric::Luma, 1),
    ("bw.red", Metric::Luma, 1),
    ("bw.orange", Metric::Luma, 1),
    ("bw.yellow", Metric::Luma, 1),
    ("bw.green", Metric::Luma, 1),
    ("bw.aqua", Metric::Luma, 1),
    ("bw.blue", Metric::Luma, 1),
    ("bw.purple", Metric::Luma, 1),
    ("bw.magenta", Metric::Luma, 1),
];

/// A hue that goes all the way round: its top end is its default again.
fn wraps(c: &controls::ControlSpec, end: f64) -> bool {
    c.id.ends_with(".hue") && c.id.starts_with("grading.") && end == c.max
}

fn base_for(c: &controls::ControlSpec) -> DevelopSettings {
    let mut s = DevelopSettings::default();
    controls::prepare_to_show(&mut s, c.id);
    controls::set(&mut s, c.id, c.default);
    s
}

#[test]
fn every_control_changes_the_picture_at_both_ends_and_resets_exactly() {
    let (small, big) = (scene(), big_scene());
    let untouched_small = shot(&small, &SourceInfo::default(), &DevelopSettings::default());
    let untouched_big = shot(&big, &SourceInfo::default(), &DevelopSettings::default());
    let mut failures: Vec<String> = Vec::new();
    for c in controls::CONTROLS {
        let (src, untouched) = if c.id == "grain.size" { (&big, &untouched_big) } else { (&small, &untouched_small) };
        // (c) setting the default on an untouched edit gives the untouched render back, bit for bit
        let mut s = DevelopSettings::default();
        controls::set(&mut s, c.id, c.default);
        if s != DevelopSettings::default() || shot(src, &SourceInfo::default(), &s) != *untouched {
            failures.push(format!("{}: its default does not give the untouched render back", c.id));
        }
        if EXEMPT.iter().any(|(id, _)| *id == c.id) {
            continue;
        }
        // (a, b) both ends, and a quarter of the way: a change, more the further it goes
        let info = info_for(c.id);
        let base = base_for(c);
        let base_img = shot(src, &info, &base);
        let at = |v: f64| {
            let mut s = base.clone();
            controls::set(&mut s, c.id, v);
            shot(src, &info, &s)
        };
        let mut ends = Vec::new();
        for (name, end) in [("min", c.min), ("max", c.max)] {
            if end == c.default || wraps(c, end) {
                continue;
            }
            let mut last = 0.0;
            for f in [0.25, 0.5, 1.0] {
                let img = at(c.default + (end - c.default) * f);
                let (n, d) = (changed(&base_img, &img), distance(&base_img, &img));
                let known = KNOWN_DEAD.iter().any(|(id, e, fr, _)| *id == c.id && *e == name && *fr == f);
                if n == 0 && !known {
                    failures.push(format!("{} {name} x{f}: no change at all", c.id));
                }
                if n != 0 && known {
                    failures.push(format!("{} {name} x{f}: listed in KNOWN_DEAD but it changes the picture now", c.id));
                }
                if f == 1.0 && d + 0.02 < last * 0.9 {
                    failures.push(format!("{} {name}: further moves it less ({last:.3} at half, {d:.3} at the end)", c.id));
                }
                last = d;
            }
            ends.push((end, at(end)));
        }
        // the direction its name promises
        if let Some((_, metric, sign)) = DIRECTION.iter().find(|(id, ..)| *id == c.id) {
            let b = measure(*metric, &base_img);
            for (end, img) in &ends {
                let delta = (measure(*metric, img) - b) * if *end > c.default { 1.0 } else { -1.0 } * f64::from(*sign);
                if delta <= 0.0 {
                    failures.push(format!("{}: moving to {end} gave {delta:+.4} of {metric:?}, the wrong way", c.id));
                }
            }
        }
    }
    for (id, _) in EXEMPT {
        assert!(controls::find(id).is_some(), "exempt control `{id}` no longer exists");
    }
    assert!(failures.is_empty(), "{} problems:\n{}", failures.len(), failures.join("\n"));
}

// --- local (mask) sliders ----------------------------------------------------------------------------------------

/// A mask over the whole picture (a linear gradient that starts past its right edge) with `adjust`.
fn masked(adjust: LocalAdjustments) -> DevelopSettings {
    let shape = MaskShape::Linear { start: Point::new(1.01, 0.5), end: Point::new(1.02, 0.5) };
    let mut s = DevelopSettings::default();
    s.masks.push(Mask { id: 1, components: vec![MaskComponent { name: None, op: MaskOp::Add, invert: false, shape }], adjust, ..Default::default() });
    s
}

fn with_local(id: &str, v: f64) -> LocalAdjustments {
    let mut a = LocalAdjustments::default();
    let mut json = serde_json::to_value(a).unwrap_or_default();
    json[id] = serde_json::json!(v);
    a = serde_json::from_value(json).unwrap_or_default();
    a
}

#[test]
fn every_local_slider_changes_the_picture_and_zero_changes_nothing() {
    let src = scene();
    let info = SourceInfo::default();
    let untouched = shot(&src, &info, &DevelopSettings::default());
    // a mask with nothing adjusted is no edit at all
    assert_eq!(shot(&src, &info, &masked(LocalAdjustments::default())), untouched, "an empty mask changes the picture");
    let mut failures: Vec<String> = Vec::new();
    for c in controls::LOCAL {
        assert_eq!(
            serde_json::to_value(with_local(c.id, 1.0)).unwrap_or_default()[c.id],
            serde_json::json!(1.0),
            "`{}` is a LocalAdjustments field",
            c.id
        );
        for (name, end) in [("min", c.min), ("max", c.max)] {
            if end == c.default {
                continue;
            }
            let mut last = 0.0;
            for f in [0.25, 0.5, 1.0] {
                let img = shot(&src, &info, &masked(with_local(c.id, end * f)));
                let (n, d) = (changed(&untouched, &img), distance(&untouched, &img));
                if n == 0 {
                    failures.push(format!("local {} {name} x{f}: no change at all", c.id));
                }
                if f == 1.0 && d + 0.02 < last * 0.9 {
                    failures.push(format!("local {} {name}: further moves it less ({last:.3} at half, {d:.3} at the end)", c.id));
                }
                last = d;
            }
        }
    }
    // direction
    let mut dir = |id: &str, m: Metric, sign: f64| {
        let end = if id == "exposure" { 4.0 } else { 100.0 };
        let (lo, hi) = (measure(m, &shot(&src, &info, &masked(with_local(id, -end)))), measure(m, &shot(&src, &info, &masked(with_local(id, end)))));
        if (hi - lo) * sign <= 0.0 {
            failures.push(format!("local {id}: {lo:.3} at min, {hi:.3} at max of {m:?}, the wrong way"));
        }
    };
    dir("exposure", Metric::Luma, 1.0);
    dir("highlights", Metric::Luma, 1.0);
    dir("shadows", Metric::Luma, 1.0);
    dir("whites", Metric::Luma, 1.0);
    dir("blacks", Metric::Luma, 1.0);
    dir("contrast", Metric::Contrast, 1.0);
    dir("saturation", Metric::Chroma, 1.0);
    dir("temp", Metric::Warmth, 1.0);
    dir("tint", Metric::Green, -1.0);
    dir("sharpness", Metric::Detail, 1.0);
    dir("noise", Metric::Detail, -1.0);
    // Amount scales the adjustments: 0 none of it, 200 more than 100
    let adj = with_local("exposure", 1.0);
    let at = |amount: f64| shot(&src, &info, &masked(LocalAdjustments { amount, ..adj }));
    assert_eq!(at(0.0), untouched, "Amount 0 still adjusts");
    let (a100, a200, a50) = (distance(&untouched, &at(100.0)), distance(&untouched, &at(200.0)), distance(&untouched, &at(50.0)));
    if !(a50 < a100 && a100 < a200) {
        failures.push(format!("local amount: 50 → {a50:.3}, 100 → {a100:.3}, 200 → {a200:.3} should grow"));
    }
    assert!(failures.is_empty(), "{} problems:\n{}", failures.len(), failures.join("\n"));
}

/// A mask's own controls (`controls::shape_controls`: a luminance range's bounds and smoothness, a colour range's
/// refine, depth range's near, far and smoothness) and its Refine Edges move what the mask selects.
#[test]
fn every_mask_shape_control_changes_what_is_selected() {
    use lightcraft_develop::MaskShape;
    let src = scene();
    let info = SourceInfo::default();
    let with = |shape: MaskShape, refine: f64| {
        let mut s = DevelopSettings::default();
        let adjust = with_local("exposure", 1.5);
        s.lens_blur.depth = lightcraft_develop::DepthMap::from_nearness(16, 8, &(0..128).map(|i| (i % 16) as f32 / 15.0).collect::<Vec<_>>());
        s.masks.push(Mask {
            id: 1,
            components: vec![MaskComponent { name: None, op: MaskOp::Add, invert: false, shape }],
            adjust,
            refine,
            ..Default::default()
        });
        shot(&src, &info, &s)
    };
    let shapes = [
        MaskShape::LuminanceRange { lo: 0.3, hi: 0.7, lo_feather: 0.1, hi_feather: 0.1 },
        MaskShape::DepthRange { lo: 0.3, hi: 0.7, feather: 0.1 },
        MaskShape::ColorRange { samples: vec![[0.6, 0.1, 0.05]], refine: 50.0 },
    ];
    let mut failures: Vec<String> = Vec::new();
    for shape in shapes {
        let base = with(shape.clone(), 0.0);
        let untouched = shot(&src, &info, &DevelopSettings::default());
        if changed(&base, &untouched) == 0 {
            failures.push(format!("{shape:?}: the mask selects nothing in the scene"));
            continue;
        }
        let specs = controls::shape_controls(&shape);
        assert!(!specs.is_empty(), "{shape:?} has controls");
        for (spec, value) in specs {
            for end in [spec.min, spec.max] {
                let Some(moved) = controls::set_shape_control(&shape, spec.id, end) else {
                    failures.push(format!("{shape:?}: `{}` can't be set", spec.id));
                    continue;
                };
                if end != value && changed(&base, &with(moved, 0.0)) == 0 {
                    failures.push(format!("{shape:?}: `{}` = {end} (from {value}) selects the same", spec.id));
                }
            }
        }
    }
    // Refine Edges: the edges of a feathered radial mask snap to the picture's
    let radial = MaskShape::Radial { center: Point::new(0.5, 0.6), rx: 0.35, ry: 0.3, angle: 0.0, feather: 60.0, invert: false };
    if changed(&with(radial.clone(), 0.0), &with(radial, 100.0)) == 0 {
        failures.push("Refine Edges 100 selects the same as 0".into());
    }
    assert!(failures.is_empty(), "{} problems:\n{}", failures.len(), failures.join("\n"));
}

/// A slider drag renders through the stage cache (`render_cached`), each tick reusing the stages whose inputs did not
/// change. A setting the cache keys forget would leave a slider dead while it is dragged and right when the picture is
/// made fresh: every control, moved one after the other through one cache, gives the picture a fresh render gives,
/// as a full render and as a draft.
#[test]
fn a_cached_render_gives_the_picture_a_fresh_one_does_for_every_control() {
    use crate::{Quality, StageCache, render_cached};
    use std::sync::Arc;
    let (small, big) = (Arc::new(scene()), Arc::new(big_scene()));
    let mut failures: Vec<String> = Vec::new();
    for c in controls::CONTROLS {
        if EXEMPT.iter().any(|(id, _)| *id == c.id) {
            continue;
        }
        let src = if c.id == "grain.size" { &big } else { &small };
        let info = info_for(c.id);
        let base = base_for(c);
        let at = |v: f64| {
            let mut s = base.clone();
            controls::set(&mut s, c.id, v);
            s
        };
        for quality in [Quality::Full, Quality::Draft] {
            // (a draft after a noise reduction change keeps the wide blurs of the one before, by design: `Planes::for_other_nr`)
            if quality == Quality::Draft && c.id.starts_with("detail.nr") {
                continue;
            }
            let req = RenderRequest { quality, ..RenderRequest::fit(src.width, src.height) };
            let cache = StageCache::default();
            let mut first: Option<Rgba8> = None;
            // base, then each end, then back to the base: what a drag does
            for (name, s) in [("base", base.clone()), ("min", at(c.min)), ("max", at(c.max)), ("base again", base.clone())] {
                let cached = render_cached(src, &info, &s, &req, &cache).image;
                let fresh = render(src, &info, &s, &req).image;
                if cached != fresh {
                    failures.push(format!("{} ({quality:?}, {name}): the cached render differs from a fresh one", c.id));
                }
                match &first {
                    None => first = Some(cached),
                    Some(f) if name == "base again" && *f != cached => {
                        failures.push(format!("{} ({quality:?}): back at the base the picture is not the first one", c.id));
                    }
                    _ => {}
                }
            }
        }
    }
    assert!(failures.is_empty(), "{} problems:\n{}", failures.len(), failures.join("\n"));
}

/// A Point Color sample's eight sliders (`pointColor.<i>.*`, the indexed controls) change the picture around the colour
/// sampled. (Red Eye's two need a pupil to find and are tested with the red eye tool.)
#[test]
fn every_point_color_slider_changes_the_picture() {
    let src = scene();
    let info = SourceInfo::default();
    // a colour of the scene: a pixel in the hue sweep
    let [l, a, b] = lightcraft_color::perceptual::oklab_from_2020(src.data[src.width / 2 + src.width * (src.height * 3 / 10)]);
    let sample = lightcraft_develop::PointColor {
        lum: f64::from(l),
        chroma: f64::from(a.hypot(b)),
        hue: f64::from(b.atan2(a).to_degrees().rem_euclid(360.0)),
        ..Default::default()
    };
    let base = |shift_first: bool, off_lightness: bool| {
        let mut s = DevelopSettings::default();
        // (a lightness range only matters for colours at other lightnesses than the sample's)
        s.point_colors.push(lightcraft_develop::PointColor { lum: sample.lum + if off_lightness { 0.12 } else { 0.0 }, ..sample });
        if shift_first {
            // (the ranges and the variance only show once the colour is moved)
            s.point_colors[0].sat_shift = 100.0;
        }
        s
    };
    let untouched = shot(&src, &info, &DevelopSettings::default());
    assert_eq!(shot(&src, &info, &base(false, false)), untouched, "a sample with nothing shifted changes the picture");
    let mut failures: Vec<String> = Vec::new();
    for spec in controls::INDEXED.iter().filter(|c| c.id.starts_with("pointColor.")) {
        let field = spec.id.trim_start_matches("pointColor.");
        let id = format!("pointColor.0.{field}");
        let is_shift = field.ends_with("Shift");
        let start = base(!is_shift, field == "lumRange");
        let before = shot(&src, &info, &start);
        for (name, end) in [("min", spec.min), ("max", spec.max)] {
            if end == spec.default {
                continue;
            }
            for f in [0.25, 1.0] {
                let mut s = start.clone();
                controls::set(&mut s, &id, spec.default + (end - spec.default) * f);
                if changed(&before, &shot(&src, &info, &s)) == 0 {
                    failures.push(format!("{id} {name} x{f}: no change at all"));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{} problems:\n{}", failures.len(), failures.join("\n"));
}

// --- preview and export look alike --------------------------------------------------------------------------------

/// Two-by-two box average: the picture at half the size.
fn halved(img: &Rgba8) -> Rgba8 {
    let (w, h) = (img.width / 2, img.height / 2);
    Rgba8::from_fn(w, h, |x, y| {
        let mut out = [0u8; 4];
        for (c, o) in out.iter_mut().enumerate() {
            let sum: u32 =
                [(0, 0), (1, 0), (0, 1), (1, 1)].iter().map(|(dx, dy)| u32::from(img.data[(2 * y + dy) * img.width + 2 * x + dx][c])).sum();
            *o = ((sum + 2) / 4) as u8;
        }
        out
    })
}

/// A slider's effect is the same at a preview's size and an export's (settings are in normalized units): the render at
/// the full size, halved, is the render at half the size, as much as it is with no edit at all. The controls that work
/// on single pixels by design (grain, noise reduction, sharpening, texture, defringe) are left to their own tests.
#[test]
fn controls_look_the_same_at_two_sizes() {
    let src = lightcraft_scenes::sweep_scene(2 * W, 2 * H);
    let info = SourceInfo::default();
    let pair = |s: &DevelopSettings| {
        let small = render(&src, &info, s, &RenderRequest::fit(W, H)).image;
        let big = halved(&render(&src, &info, s, &RenderRequest::fit(2 * W, 2 * H)).image);
        distance(&small, &big)
    };
    let floor = pair(&DevelopSettings::default());
    let mut report = Vec::new();
    for c in controls::CONTROLS {
        let pixel_level = ["grain.", "detail.", "optics.defringe", "optics.ca", "effects.texture", "enhance.", "hdr.", "lensBlur."]
            .iter()
            .any(|p| c.id.starts_with(p));
        if pixel_level {
            continue;
        }
        let mut s = DevelopSettings::default();
        controls::prepare_to_show(&mut s, c.id);
        for end in [c.min, c.max] {
            if end == c.default {
                continue;
            }
            let mut e = s.clone();
            controls::set(&mut e, c.id, end);
            let d = pair(&e);
            report.push((c.id, end, d));
        }
    }
    // (at most a few levels of 8-bit grey apart: Dehaze, whose airlight is found from a window of the picture, is the
    // furthest, at about 2)
    let far: Vec<String> =
        report.iter().filter(|(_, _, d)| *d > floor + 2.5).map(|(id, end, d)| format!("{id} = {end}: {d:.3} (none: {floor:.3})")).collect();
    assert!(far.is_empty(), "these look different at two sizes:\n{}", far.join("\n"));
}
