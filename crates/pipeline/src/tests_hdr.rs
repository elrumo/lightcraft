//! HDR editing: the HDR render ([`OutputDepth::F32Hdr`]) and the SDR rendition every other render uses.

use lightcraft_develop::DevelopSettings;
use lightcraft_raster::Rgb32f;

use crate::{DeepSamples, OutputDepth, RenderRequest, SourceInfo, render};

fn sunset() -> Rgb32f {
    let lib = lightcraft_scenes::demo_library();
    let s = lib.iter().find(|s| s.kind == lightcraft_scenes::Kind::OceanSunset).map(|s| s.render(240, 160));
    s.unwrap_or_else(|| lib[0].render(240, 160))
}

fn raw() -> SourceInfo {
    SourceInfo { raw: true, ..Default::default() }
}

fn hdr_on(max_ev: f64) -> DevelopSettings {
    let mut s = DevelopSettings::default();
    s.hdr.enabled = true;
    s.hdr.max_ev = max_ev;
    s
}

fn deep(src: &Rgb32f, s: &DevelopSettings, depth: OutputDepth) -> Vec<f32> {
    let req = RenderRequest { depth, ..RenderRequest::fit(240, 240) };
    match render(src, &raw(), s, &req).deep.map(|d| d.samples) {
        Some(DeepSamples::F32(v)) => v,
        other => panic!("expected float samples, got {other:?}"),
    }
}

#[test]
fn hdr_render_keeps_highlights_up_to_the_peak() {
    let src = sunset();
    let v = deep(&src, &hdr_on(3.0), OutputDepth::F32Hdr);
    assert!(v.iter().all(|x| x.is_finite() && (0.0..=8.0 + 1e-4).contains(x)));
    let max = v.iter().copied().fold(0.0f32, f32::max);
    assert!(max > 1.5, "the sun should rise above SDR white, max {max}");
    // a smaller headroom limit caps lower
    let v1 = deep(&src, &hdr_on(1.0), OutputDepth::F32Hdr);
    assert!(v1.iter().all(|x| *x <= 2.0 + 1e-4));
}

#[test]
fn hdr_midtones_match_the_sdr_render() {
    let src = sunset();
    let hdr = deep(&src, &hdr_on(3.0), OutputDepth::F32Hdr);
    let sdr = deep(&src, &DevelopSettings::default(), OutputDepth::F32Linear);
    // pixels whose brightest channel is a midtone (bright saturated colours legitimately differ:
    // SDR desaturates them at white, HDR keeps their colour)
    let mut n = 0;
    for (h, s) in hdr.as_chunks::<3>().0.iter().zip(sdr.as_chunks::<3>().0) {
        if s.iter().all(|v| *v < 0.15) {
            for (a, b) in h.iter().zip(s) {
                assert!((a - b).abs() < 2e-3, "{h:?} vs {s:?}");
            }
            n += 1;
        }
    }
    assert!(n > 100, "the scene should have midtones and shadows ({n})");
}

#[test]
fn hdr_keeps_the_camera_tone_curve_in_the_midtones() {
    // A raw with a fitted camera look (brighter midtones than the generic curve): its HDR render
    // used to drop the look and come out more than a stop darker than the SDR render.
    let knots = std::array::from_fn(|i| {
        let x = 0.004 * 1.18f32.powi(i as i32);
        [x, 1.0 - (-2.0 * x).exp()]
    });
    let info = SourceInfo { camera_tone: crate::tone::CameraTone::new(knots), ..raw() };
    let src = sunset();
    let render_at = |s: &DevelopSettings, depth| {
        let req = RenderRequest { depth, ..RenderRequest::fit(240, 240) };
        match render(&src, &info, s, &req).deep.map(|d| d.samples) {
            Some(DeepSamples::F32(v)) => v,
            other => panic!("expected float samples, got {other:?}"),
        }
    };
    let hdr = render_at(&hdr_on(3.0), OutputDepth::F32Hdr);
    let sdr = render_at(&DevelopSettings::default(), OutputDepth::F32Linear);
    let mut n = 0;
    for (h, s) in hdr.as_chunks::<3>().0.iter().zip(sdr.as_chunks::<3>().0) {
        if s.iter().all(|v| *v < 0.4) {
            for (a, b) in h.iter().zip(s) {
                assert!((a - b).abs() < 2e-3, "{h:?} vs {s:?}");
            }
            n += 1;
        }
    }
    assert!(n > 100, "the scene should have midtones and shadows ({n})");
    assert!(hdr.iter().any(|v| *v > 1.0), "highlights go past SDR white");
}

#[test]
fn f32hdr_without_hdr_is_the_sdr_float_render() {
    let src = sunset();
    let s = DevelopSettings::default();
    assert_eq!(deep(&src, &s, OutputDepth::F32Hdr), deep(&src, &s, OutputDepth::F32Linear));
}

#[test]
fn sdr_renders_of_an_hdr_edit_use_the_sdr_rendition() {
    let src = sunset();
    let req = RenderRequest::fit(240, 240);
    // neutral offsets: identical to the edit with HDR off
    let plain = render(&src, &raw(), &DevelopSettings::default(), &req);
    let on = render(&src, &raw(), &hdr_on(3.0), &req);
    assert_eq!(plain.image.data, on.image.data);
    // offsets apply to every SDR render
    let mut s = hdr_on(3.0);
    s.hdr.sdr_brightness = 50.0;
    s.hdr.sdr_highlights = -40.0;
    let brighter = render(&src, &raw(), &s, &req);
    let expect = render(&src, &raw(), &s.sdr_rendition(), &req);
    assert_eq!(brighter.image.data, expect.image.data);
    assert_ne!(brighter.image.data, plain.image.data);
    let r = s.sdr_rendition();
    assert!(!r.hdr.enabled && (r.light.exposure - 0.5).abs() < 1e-9 && r.light.highlights == -40.0);
}

#[test]
fn visualize_hdr_paints_only_what_rises_above_white() {
    use crate::Overlay;
    use crate::visualize::HDR_BANDS;
    let src = sunset();
    let req = RenderRequest { overlay: Overlay::HdrRange, ..RenderRequest::fit(240, 240) };
    let banded = |p: &[u8; 4]| HDR_BANDS.iter().any(|b| (0..3).all(|k| (p[k] as i32 - b[k] as i32).abs() < 70) && p[0] > p[2] + 40);
    let on = render(&src, &raw(), &hdr_on(3.0), &req);
    let n = on.image.data.iter().filter(|p| banded(p)).count();
    assert!(n > 0 && n < on.image.data.len() / 3, "{n} banded pixels");
    // the rest is grey
    assert!(on.image.data.iter().filter(|p| !banded(p)).all(|p| p[0] == p[1] && p[1] == p[2]));
    // HDR off: the overlay draws nothing
    let off = render(&src, &raw(), &DevelopSettings::default(), &req);
    let plain = render(&src, &raw(), &DevelopSettings::default(), &RenderRequest::fit(240, 240));
    assert_eq!(off.image.data, plain.image.data);
}

#[test]
fn hdr_histogram_shows_the_range_above_sdr_white_on_a_fixed_scale() {
    let src = sunset();
    let req = RenderRequest::fit(240, 240);
    // SDR edits, and requests the image's own histogram describes, have none
    assert!(crate::hdr_histogram(&src, &raw(), &DevelopSettings::default(), &req).is_none());
    let mut s = hdr_on(3.0);
    s.light.exposure = 1.0;
    for other in [
        RenderRequest { depth: OutputDepth::U16, ..req },
        RenderRequest { window: Some(crate::PixelWindow { x: 0, y: 0, w: 8, h: 8 }), ..req },
        RenderRequest { proof: Some(crate::Proof::default()), ..req },
    ] {
        assert!(crate::hdr_histogram(&src, &raw(), &s, &other).is_none());
    }
    let h = crate::hdr_histogram(&src, &raw(), &s, &req).expect("an HDR edit's histogram");
    let bins = h.hdr.expect("HDR bins");
    assert_eq!(bins.stops, lightcraft_develop::Hdr::MAX_EV_LIMIT as f32, "the whole HDR range");
    assert!(h.above_sdr() > 0.0, "highlights above SDR white: {}", h.above_sdr());
    assert!(h.total > 0);
    // the scale doesn't follow the headroom limit (a damaged one included); the content does
    let highest = |h: &lightcraft_raster::Histogram| h.luma.iter().rposition(|&n| n > 0).unwrap_or(0);
    let top3 = highest(&h);
    s.hdr.max_ev = 1.5;
    let h15 = crate::hdr_histogram(&src, &raw(), &s, &req).unwrap();
    assert_eq!(h15.hdr.unwrap().stops, bins.stops);
    assert!(highest(&h15) < top3, "a lower limit pulls the highlights down: {} vs {top3}", highest(&h15));
    s.hdr.max_ev = f64::NAN;
    assert_eq!(crate::hdr_histogram(&src, &raw(), &s, &req).unwrap().hdr.unwrap().stops, bins.stops);
    // the SDR rendition's own histogram has nothing above white
    assert!(render(&src, &raw(), &hdr_on(3.0), &req).histogram.hdr.is_none());
}

/// Luminance with Rec. 2020 weights (renders into Rec. 2020).
fn luma(p: &[f32; 3]) -> f32 {
    0.2627 * p[0] + 0.678 * p[1] + 0.0593 * p[2]
}

/// Luminance with sRGB weights (renders into sRGB, the default).
fn luma709(p: &[f32; 3]) -> f32 {
    0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]
}

/// For every tone control, the HDR render and the SDR render of the same edit agree wherever the SDR
/// render is a midtone or darker, and the HDR render is never darker than the SDR one (colour above
/// white legitimately differs: SDR desaturates towards white, HDR keeps it).
#[test]
fn hdr_follows_the_sdr_render_for_every_tone_edit() {
    type Edit = (&'static str, fn(&mut DevelopSettings));
    let edits: [Edit; 12] = [
        ("exposure +1", |s| s.light.exposure = 1.0),
        ("exposure -1", |s| s.light.exposure = -1.0),
        ("contrast +50", |s| s.light.contrast = 50.0),
        ("highlights -80", |s| s.light.highlights = -80.0),
        ("shadows +80", |s| s.light.shadows = 80.0),
        ("whites +60", |s| s.light.whites = 60.0),
        ("whites -60", |s| s.light.whites = -60.0),
        ("blacks -60", |s| s.light.blacks = -60.0),
        ("saturation +60", |s| s.color.saturation = 60.0),
        ("clarity +50", |s| s.effects.clarity = 50.0),
        ("vignette -50", |s| s.vignette.amount = -50.0),
        ("vignette +50", |s| s.vignette.amount = 50.0),
    ];
    let src = sunset();
    for (name, edit) in edits {
        for max_ev in [1.0, 3.0, 5.0] {
            let mut sdr_edit = DevelopSettings::default();
            edit(&mut sdr_edit);
            let mut hdr_edit = sdr_edit.clone();
            hdr_edit.hdr.enabled = true;
            hdr_edit.hdr.max_ev = max_ev;
            let (hdr, sdr) = (deep(&src, &hdr_edit, OutputDepth::F32Hdr), deep(&src, &sdr_edit, OutputDepth::F32Linear));
            let (mut n, mut above) = (0, 0);
            for (h, s) in hdr.as_chunks::<3>().0.iter().zip(sdr.as_chunks::<3>().0) {
                assert!(luma709(h) >= luma709(s) - 1e-3, "{name} at {max_ev} EV: HDR {h:?} darker than SDR {s:?}");
                if luma709(s) < 0.3 {
                    assert!((luma709(h) - luma709(s)).abs() < 2e-3, "{name} at {max_ev} EV: {h:?} vs {s:?}");
                    n += 1;
                }
                above += usize::from(h.iter().any(|v| *v > 1.0));
            }
            assert!(n > 100, "{name}: the scene has midtones and shadows ({n})");
            assert!(above > 0 || name == "exposure -1", "{name} at {max_ev} EV: nothing above SDR white");
        }
    }
}

/// A rendered (display-referred) source, a JPEG: an HDR edit renders it as the SDR edit up to white, and
/// what an edit pushes past white (exposure, whites) survives instead of clipping.
#[test]
fn hdr_edit_of_a_display_referred_source_follows_the_sdr_render() {
    let mut src = sunset();
    src.data.iter_mut().for_each(|p| *p = p.map(|v| v.clamp(0.0, 1.0)));
    let jpeg = SourceInfo::default();
    // (Rec. 2020 output: the clamped source is inside that gamut, so no gamut mapping is in the way)
    let req = |depth| RenderRequest { depth, space: crate::OutputSpace::Rec2020, ..RenderRequest::fit(240, 240) };
    let get = |s: &DevelopSettings, d| match render(&src, &jpeg, s, &req(d)).deep.map(|d| d.samples) {
        Some(DeepSamples::F32(v)) => v,
        other => panic!("expected float samples, got {other:?}"),
    };
    for (name, exposure) in [("neutral", 0.0), ("+0.5 EV", 0.5), ("-0.5 EV", -0.5)] {
        let mut sdr_edit = DevelopSettings::default();
        sdr_edit.light.exposure = exposure;
        let mut hdr_edit = sdr_edit.clone();
        hdr_edit.hdr.enabled = true;
        hdr_edit.hdr.max_ev = 3.0;
        let (hdr, sdr) = (get(&hdr_edit, OutputDepth::F32Hdr), get(&sdr_edit, OutputDepth::F32Linear));
        let mut above = 0;
        for (h, s) in hdr.as_chunks::<3>().0.iter().zip(sdr.as_chunks::<3>().0) {
            assert!(luma(h) >= luma(s) - 1e-3, "{name}: HDR {h:?} darker than SDR {s:?}");
            if luma(s) < 0.5 {
                assert!((luma(h) - luma(s)).abs() < 2e-3, "{name}: {h:?} vs {s:?}");
            }
            above += usize::from(h.iter().any(|v| *v > 1.001));
        }
        assert_eq!(above > 0, exposure > 0.0, "{name}: only an edit that pushes past white has anything above it ({above})");
    }
}
