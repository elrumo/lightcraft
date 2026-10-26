//! Process V3's capture sharpening ([`local::Capture`]): its radius is in the original's pixels, so it
//! sharpens the finest detail without the wide halos the texture band drew before, fades in previews
//! (as the full-size render scaled down does), and Detail holds back what strong edges get.

use lightcraft_develop::{DevelopSettings, LocalAdjustments, Mask, Process};
use lightcraft_raster::{Rgb32f, Rgba8};

use crate::{RenderRequest, SourceInfo, local, render};

/// `p`'s settings with Lightroom's raw sharpening defaults (Amount 40, Radius 1, Detail 25).
fn on(p: Process) -> DevelopSettings {
    let mut s = DevelopSettings { process: p.version(), ..DevelopSettings::default() };
    s.detail.sharpen_amount = 40.0;
    s
}

/// A vertical edge between two greys at x = w/2.
fn edge(w: usize, dark: f32, bright: f32) -> Rgb32f {
    Rgb32f::from_fn(w, 8, |x, _| if x < w / 2 { [dark; 3] } else { [bright; 3] })
}

fn shot(src: &Rgb32f, s: &DevelopSettings, w: usize) -> Rgba8 {
    render(src, &SourceInfo::default(), s, &RenderRequest::fit(w, w)).image
}

/// Green along the middle row of `img` less that of `base`.
fn diff(img: &Rgba8, base: &Rgba8) -> Vec<i32> {
    let y = img.height / 2;
    (0..img.width).map(|x| img.data[y * img.width + x][1] as i32 - base.data[y * base.width + x][1] as i32).collect()
}

#[test]
fn halos_stay_at_the_edge_at_full_size() {
    // a 24 MP photo's long edge: the texture band V2 sharpened with is 0.18 % of it, about 10 px
    let w = 6000;
    let src = edge(w, 0.03, 0.18);
    let none = |p: Process| {
        let mut s = on(p);
        s.detail.sharpen_amount = 0.0;
        shot(&src, &s, w)
    };
    let (v2, v3) = (diff(&shot(&src, &on(Process::V2), w), &none(Process::V2)), diff(&shot(&src, &on(Process::V3), w), &none(Process::V3)));
    let at = |d: &[i32], off: usize| d[w / 2 + off];
    // both brighten the bright side right at the edge
    assert!(at(&v3, 0) > 0 && at(&v2, 0) > 0, "edge: V2 {} V3 {}", at(&v2, 0), at(&v3, 0));
    // V2's band still brightens 12 px away; capture sharpening is gone there
    assert!(at(&v2, 12) >= 2, "V2 at 12 px: {}", at(&v2, 12));
    assert!((6..40).all(|o| at(&v3, o) == 0), "V3 past 6 px: {:?}", &v3[w / 2..w / 2 + 40]);
}

#[test]
fn a_preview_shows_what_the_full_size_render_scaled_down_shows() {
    // the fitted view of a 6000 px photo: one output pixel is ten of the original's, and a 1 px radius
    // leaves almost nothing (V2 drew its halo at every size)
    let src = edge(6000, 0.03, 0.18);
    let flat = |p: Process| {
        let mut s = on(p);
        s.detail.sharpen_amount = 0.0;
        shot(&src, &s, 600)
    };
    let v3 = diff(&shot(&src, &on(Process::V3), 600), &flat(Process::V3));
    assert!(v3.iter().all(|d| d.abs() <= 1), "{:?}", &v3[290..310]);
    let v2 = diff(&shot(&src, &on(Process::V2), 600), &flat(Process::V2));
    assert!(v2.iter().any(|d| d.abs() >= 2), "{:?}", &v2[290..310]);
}

#[test]
fn detail_holds_back_strong_edges() {
    let w = 2000;
    let src = edge(w, 0.004, 0.6);
    let with = |detail: f64| {
        let mut s = on(Process::V3);
        s.detail.sharpen_amount = 100.0;
        s.detail.sharpen_detail = detail;
        shot(&src, &s, w)
    };
    let mut flat = on(Process::V3);
    flat.detail.sharpen_amount = 0.0;
    let flat = shot(&src, &flat, w);
    let overshoot = |img: &Rgba8| diff(img, &flat)[w / 2];
    let (low, high) = (overshoot(&with(0.0)), overshoot(&with(100.0)));
    assert!(high > 0 && low < high, "detail 0: {low}, detail 100: {high}");
}

#[test]
fn radius_detail_and_scale_give_the_parameters() {
    let v3 = on(Process::V3);
    assert_eq!(local::capture_sharpen(&on(Process::V2), 1.0), None, "V2 sharpens with the texture band");
    let full = local::capture_sharpen(&v3, 1.0).expect("V3 at full size");
    assert_eq!((full.sigma, full.w, full.detail), (1.0, 1.0, 0.25));
    // a radius in the original's pixels: half size halves it, and below the smallest blur it fades
    let mut wide = v3.clone();
    wide.detail.sharpen_radius = 3.0;
    assert_eq!(local::capture_sharpen(&wide, 0.5).map(|c| (c.sigma, c.w)), Some((1.5, 1.0)));
    let half = local::capture_sharpen(&v3, 0.5).expect("half size");
    assert!(half.w < 0.5 && half.w > 0.1, "{half:?}");
    assert_eq!(local::capture_sharpen(&v3, 0.02), None, "nothing left to show");
    // no sharpening, unless a mask asks for some
    let mut off = v3.clone();
    off.detail.sharpen_amount = 0.0;
    assert_eq!(local::capture_sharpen(&off, 1.0), None);
    off.masks = vec![Mask { adjust: LocalAdjustments { sharpness: 50.0, ..Default::default() }, ..Default::default() }];
    assert!(local::capture_sharpen(&off, 1.0).is_some());
    // hostile values stay in range
    let mut odd = v3;
    odd.detail.sharpen_radius = f64::NAN;
    odd.detail.sharpen_detail = 1e9;
    let c = local::capture_sharpen(&odd, f32::NAN).expect("still sharpens");
    assert!(c.sigma.is_finite() && c.w.is_finite() && c.detail == 1.0, "{c:?}");
}

#[test]
fn the_texture_band_is_left_to_texture_from_v3() {
    let sig = |p: Process| local::plane_sigmas(&on(p), 6000.0, crate::Quality::Full).texture;
    assert!(sig(Process::V2).is_some(), "V2 sharpens with it");
    assert_eq!(sig(Process::V3), None, "V3 sharpens with its own plane");
}
