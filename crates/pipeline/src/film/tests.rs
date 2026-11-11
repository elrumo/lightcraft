use lightcraft_develop::{DevelopSettings, FilmFrame, FilmKind, FilmLayer, FrameTone};

use super::*;

const GREY: [f32; 3] = [0.18, 0.18, 0.18];

fn with(layers: &[(FilmKind, u32)], frame: FilmFrame) -> DevelopSettings {
    let mut s = DevelopSettings::default();
    s.film.frame = frame;
    for (i, &(kind, seed)) in layers.iter().enumerate() {
        s.film.layers.push(FilmLayer::new(i as u32 + 1, kind, seed));
    }
    s
}

fn draw(s: &DevelopSettings, salt: u32, w: usize, h: usize) -> Vec<[f32; 3]> {
    let mut px = vec![GREY; w * h];
    if let Some(f) = Setup::new(s, salt, OutputSpace::Srgb, true) {
        f.apply_linear(&mut px, w, h, [0, 0], [w, h]);
    }
    px
}

fn luma(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

fn mean_luma(px: &[[f32; 3]], w: usize, x0: usize, y0: usize, x1: usize, y1: usize) -> f32 {
    let mut sum = 0.0;
    for y in y0..y1 {
        for x in x0..x1 {
            sum += luma(px[y * w + x]);
        }
    }
    sum / ((x1 - x0) * (y1 - y0)) as f32
}

/// `px` (`w × h`) halved by 2 × 2 boxes.
fn halve(px: &[[f32; 3]], w: usize, h: usize) -> Vec<[f32; 3]> {
    let (hw, hh) = (w / 2, h / 2);
    let mut out = vec![[0.0; 3]; hw * hh];
    for y in 0..hh {
        for x in 0..hw {
            let mut c = [0.0; 3];
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let p = px[(2 * y + dy) * w + 2 * x + dx];
                for k in 0..3 {
                    c[k] += p[k] / 4.0;
                }
            }
            out[y * hw + x] = c;
        }
    }
    out
}

/// The numbers a native frontend's kernel is checked against (its parity harness pins the same ones).
#[test]
fn the_hash_and_windows_are_the_native_kernels() {
    assert_eq!(hash(1, 2, 3), 0x3a0e_a07b);
    assert_eq!(hash(-5, 7, 0xdead_beef), 0x59d4_26fa);
    assert_eq!(salt_of("42"), 0x87e3_8583);
    assert_eq!(effective_seed(7, salt_of("42"), true), 0x38f9_03b7);
    assert_eq!(effective_seed(7, 99, false), 7);
    let f = Setup::new(&with(&[], FilmFrame::Strip), 0, OutputSpace::Srgb, true).unwrap();
    assert_eq!(f.window(1500, 1000), Some(Window { x0: 236, y0: 157, x1: 1264, y1: 843 }));
    assert!(Setup::new(&with(&[], FilmFrame::Strip), 0, OutputSpace::Srgb, false).is_none(), "an uncropped view has no frame");
}

#[test]
fn nothing_to_draw_is_no_setup() {
    assert!(Setup::new(&DevelopSettings::default(), 0, OutputSpace::Srgb, true).is_none());
    let mut s = with(&[(FilmKind::Dust, 1)], FilmFrame::None);
    s.film.layers[0].visible = false;
    assert!(Setup::new(&s, 0, OutputSpace::Srgb, true).is_none(), "a hidden layer draws nothing");
    let mut s = with(&[(FilmKind::Dust, 1)], FilmFrame::Strip);
    s.set_section_enabled("film", false);
    assert!(Setup::new(&s, 0, OutputSpace::Srgb, true).is_none(), "the Film eye switches it all off");
}

#[test]
fn a_leak_lights_the_edge_it_comes_from_in_warm_light() {
    let (w, h) = (300, 200);
    let s = with(&[(FilmKind::Leak, 3)], FilmFrame::None);
    // (from the top: angle 90)
    let px = draw(&s, 0, w, h);
    let top = mean_luma(&px, w, 0, 0, w, 20);
    let bottom = mean_luma(&px, w, 0, h - 20, w, h);
    assert!(top > luma(GREY) + 0.05, "top {top}");
    assert!(top > bottom + 0.05, "top {top} bottom {bottom}");
    // warm: red over blue where it lights
    let brightest = px.iter().copied().max_by(|a, b| luma(*a).total_cmp(&luma(*b))).unwrap();
    assert!(brightest[0] > brightest[2], "{brightest:?}");
}

/// A leak's offset moves it along its edge, and the offset that undoes its seed's place puts it in the middle of the
/// edge (what a frontend sets when the leak is dragged on the photo: the angle toward the finger, this offset).
#[test]
fn a_leak_moves_along_its_edge_with_its_offset() {
    let (w, h) = (300, 200);
    let mut s = with(&[(FilmKind::Leak, 3)], FilmFrame::None);
    s.film.layers[0].texture = 0.0;
    // the brightest column of the top rows (the leak comes from the top: angle 90, where `s` runs to the right)
    let column = |s: &DevelopSettings| {
        let px = draw(s, 0, w, h);
        (0..w).max_by(|&a, &b| mean_luma(&px, w, a, 0, a + 1, 20).total_cmp(&mean_luma(&px, w, b, 0, b + 1, 20))).unwrap() as i32
    };
    s.film.layers[0].offset = -60.0;
    let left = column(&s);
    s.film.layers[0].offset = 60.0;
    let right = column(&s);
    // (60 of 100 is 0.3 of the edge each way)
    assert!((right - left - 180).abs() <= 2, "left {left} right {right}");
    // (the seed as drawn: mixed with the photo's salt while the marks vary per photo)
    s.film.layers[0].offset = f64::from(-(rand_n(0, 0, effective_seed(3, 0, true), 1) - 0.5) * 1.8 * 100.0);
    // (the number a native frontend's own arithmetic is checked against)
    assert!((s.film.layers[0].offset - 23.9288).abs() < 1e-3, "{}", s.film.layers[0].offset);
    assert!((column(&s) - w as i32 / 2).abs() <= 1, "centred {}", column(&s));
}

#[test]
fn a_streak_crosses_the_frame_and_a_burn_blows_out_its_edge() {
    let (w, h) = (300, 200);
    let px = draw(&with(&[(FilmKind::Streak, 4)], FilmFrame::None), 0, w, h);
    // a horizontal band through the middle
    let middle = mean_luma(&px, w, 0, h / 2 - 3, w, h / 2 + 3);
    let top = mean_luma(&px, w, 0, 0, w, 10);
    assert!(middle > top + 0.05, "middle {middle} top {top}");
    let px = draw(&with(&[(FilmKind::Burn, 5)], FilmFrame::None), 0, w, h);
    // from the bottom (angle −90): nearly white there, untouched at the top
    let bottom = mean_luma(&px, w, 0, h - 3, w, h);
    assert!(bottom > 0.8, "bottom {bottom}");
    assert!((mean_luma(&px, w, 0, 0, w, 10) - luma(GREY)).abs() < 0.01);
}

/// A negative inverts the picture (white to black, a dark grey to a light one), its orange mask keeps the red and takes
/// the blue, half its amount is halfway, and the marks over it are not inverted with the photo.
#[test]
fn a_negative_inverts_the_picture_under_it() {
    let (w, h) = (40, 30);
    let mut s = with(&[(FilmKind::Negative, 1)], FilmFrame::None);
    s.film.layers[0].texture = 0.0;
    let grey = draw(&s, 0, w, h)[0];
    let expected = (1.0 - 0.18f32.powf(1.0 / 2.2)).powf(2.2);
    assert!(grey.iter().all(|v| (v - expected).abs() < 1e-5) && expected > 0.18, "{grey:?} {expected}");
    let mut white = vec![[1.0f32, 1.0, 1.0]; w * h];
    Setup::new(&s, 0, OutputSpace::Srgb, true).unwrap().apply_linear(&mut white, w, h, [0, 0], [w, h]);
    assert!(white[0].iter().all(|v| v.abs() < 1e-6), "{:?}", white[0]);
    // (hostile pixels: no panic, and a NaN reads as black, so it comes out white)
    let mut odd = vec![[f32::NAN, f32::INFINITY, -5.0]; w * h];
    Setup::new(&s, 0, OutputSpace::Srgb, true).unwrap().apply_linear(&mut odd, w, h, [0, 0], [w, h]);
    assert_eq!(odd[0], [1.0, 0.0, 1.0]);
    s.film.layers[0].amount = 50.0;
    assert!((draw(&s, 0, w, h)[0][0] - (0.18 + expected) / 2.0).abs() < 1e-5);
    s.film.layers[0].amount = 100.0;
    s.film.layers[0].texture = 100.0;
    let masked = draw(&s, 0, w, h)[0];
    assert!((masked[0] - expected).abs() < 1e-5 && (masked[2] - expected * 0.26).abs() < 1e-5 && masked[1] < masked[0], "{masked:?}");
    // light dust over the negative stays light; under it, it is inverted to dark
    s.film.layers[0].texture = 0.0;
    let mut dust = FilmLayer::new(2, FilmKind::Dust, 9);
    dust.amount = 100.0;
    dust.size = 80.0;
    s.film.layers.push(dust.clone());
    let over = draw(&s, 0, 600, 400);
    assert!(over.iter().any(|c| luma(*c) > expected + 0.2), "light dust over the negative");
    s.film.layers = vec![dust, s.film.layers[0].clone()];
    let under = draw(&s, 0, 600, 400);
    assert!(under.iter().any(|c| luma(*c) < 0.05) && under.iter().all(|c| luma(*c) <= expected + 1e-4), "dust under the negative is dark");
}

#[test]
fn dust_and_scratches_follow_their_tone() {
    let (w, h) = (600, 400);
    for kind in [FilmKind::Dust, FilmKind::Scratches] {
        let mut s = with(&[(kind, 9)], FilmFrame::None);
        s.film.layers[0].amount = 100.0;
        s.film.layers[0].size = 60.0;
        s.film.layers[0].tone = 100.0;
        let px = draw(&s, 0, w, h);
        assert!(px.iter().all(|c| luma(*c) >= luma(GREY) - 1e-4), "{kind:?} light only lightens");
        assert!(px.iter().any(|c| luma(*c) > luma(GREY) + 0.1), "{kind:?} draws something");
        s.film.layers[0].tone = -100.0;
        let px = draw(&s, 0, w, h);
        assert!(px.iter().all(|c| luma(*c) <= luma(GREY) + 1e-4), "{kind:?} dark only darkens");
        assert!(px.iter().any(|c| luma(*c) < luma(GREY) - 0.08), "{kind:?} draws something dark");
    }
}

#[test]
fn hairs_come_with_fibres() {
    let (w, h) = (600, 400);
    let mut s = with(&[(FilmKind::Dust, 2)], FilmFrame::None);
    s.film.layers[0].amount = 0.01;
    s.film.layers[0].fibres = 0.0;
    let plain = draw(&s, 0, w, h);
    s.film.layers[0].amount = 100.0;
    s.film.layers[0].fibres = 100.0;
    let hairy = draw(&s, 0, w, h);
    let changed = |px: &[[f32; 3]]| px.iter().filter(|c| (luma(**c) - luma(GREY)).abs() > 0.02).count();
    assert!(changed(&hairy) > changed(&plain) + 200);
}

#[test]
fn the_same_marks_at_any_size() {
    // a render at twice the size, halved, looks like the small one: the marks are placed and sized by the long edge
    let s = with(&[(FilmKind::Leak, 1), (FilmKind::Streak, 2), (FilmKind::Burn, 3), (FilmKind::Dust, 4), (FilmKind::Scratches, 5)], FilmFrame::None);
    let small = draw(&s, 0, 400, 300);
    let big = halve(&draw(&s, 0, 800, 600), 800, 600);
    let diff: f32 = small.iter().zip(&big).map(|(a, b)| (luma(*a) - luma(*b)).abs()).sum::<f32>() / small.len() as f32;
    assert!(diff < 0.01, "mean difference {diff}");
}

#[test]
fn a_window_is_cut_from_the_whole() {
    let s = with(&[(FilmKind::Leak, 1), (FilmKind::Dust, 4), (FilmKind::Scratches, 5)], FilmFrame::None);
    let (w, h) = (300, 200);
    let whole = draw(&s, 7, w, h);
    let (x0, y0, ww, wh) = (110, 40, 70, 50);
    let mut win = vec![GREY; ww * wh];
    Setup::new(&s, 7, OutputSpace::Srgb, true).unwrap().apply_linear(&mut win, ww, wh, [x0, y0], [w, h]);
    for y in 0..wh {
        for x in 0..ww {
            assert_eq!(win[y * ww + x], whole[(y0 + y) * w + x0 + x], "at {x},{y}");
        }
    }
}

#[test]
fn seeds_shuffle_and_photos_vary() {
    let (w, h) = (300, 200);
    let s = with(&[(FilmKind::Dust, 1)], FilmFrame::None);
    let a = draw(&s, 10, w, h);
    assert_eq!(a, draw(&s, 10, w, h), "the same photo draws the same marks");
    assert_ne!(a, draw(&s, 11, w, h), "another photo its own");
    assert_ne!(a, draw(&with(&[(FilmKind::Dust, 2)], FilmFrame::None), 10, w, h), "a shuffled seed others");
    let mut same = s.clone();
    same.film.vary = false;
    assert_eq!(draw(&same, 10, w, h), draw(&same, 11, w, h), "Same on Every Photo");
    assert_eq!(salt_of("42"), salt_of("42"));
    assert_ne!(salt_of("42"), salt_of("43"));
}

#[test]
fn a_strip_holds_the_photo_in_its_window_between_the_holes() {
    let (w, h) = (1500, 1000);
    let s = with(&[], FilmFrame::Strip);
    let f = Setup::new(&s, 0, OutputSpace::Srgb, true).unwrap();
    let win = f.window(w, h).unwrap();
    // 36 × 24 mm in a 35 mm high strip: the window is two thirds of the height, 3:2
    let (ww, wh) = (win.x1 - win.x0, win.y1 - win.y0);
    assert!((wh as f32 / h as f32 - 24.0 / 35.0).abs() < 0.01, "{win:?}");
    assert!((ww as f32 / wh as f32 - 1.5).abs() < 0.01, "{win:?}");
    // a picture with a bright square in the middle: it stays in the middle, smaller
    let mut px = vec![GREY; w * h];
    for y in 400..600 {
        for x in 650..850 {
            px[y * w + x] = [1.0; 3];
        }
    }
    f.apply_linear(&mut px, w, h, [0, 0], [w, h]);
    assert_eq!(px.len(), w * h, "the output keeps its size");
    assert!(luma(px[500 * w + 750]) > 0.9);
    // the slide base is near black, its holes backlit, one every 4.75 mm along the film
    let k = h as f32 / 35.0;
    let hole_y = (h as f32 / 2.0 - (35.0 / 2.0 - PERF_MARGIN - PERF_ACROSS / 2.0) * k) as usize;
    let row: Vec<f32> = (0..w).map(|x| luma(px[hole_y * w + x])).collect();
    let lit: Vec<usize> = (1..w).filter(|&x| row[x] > 0.5 && row[x - 1] <= 0.5).collect();
    assert!(lit.len() >= 8, "holes {lit:?}");
    let pitch = (lit[lit.len() - 1] - lit[0]) as f32 / (lit.len() - 1) as f32;
    assert!((pitch - PERF_PITCH * k).abs() < 1.5, "pitch {pitch} px against {}", PERF_PITCH * k);
    // a negative's base is amber
    let mut neg = with(&[], FilmFrame::Strip);
    neg.film.frame_tone = FrameTone::Light;
    let px = draw(&neg, 0, w, h);
    let base = px[3 * w + w / 2 + 1];
    assert!(base[0] > base[2] * 3.0, "{base:?}");
}

#[test]
fn filed_and_instant_frames_border_the_photo() {
    let (w, h) = (600, 400);
    let mut s = with(&[], FilmFrame::Filed);
    let px = draw(&s, 0, w, h);
    assert!(luma(px[w + 1]) < 0.01, "the filed border is black");
    assert!((luma(px[(h / 2) * w + w / 2]) - luma(GREY)).abs() < 1e-3, "the photo inside");
    s.film.frame = FilmFrame::Instant;
    s.film.frame_tone = FrameTone::Light;
    let f = Setup::new(&s, 0, OutputSpace::Srgb, true).unwrap();
    let win = f.window(w, h).unwrap();
    assert!(h - win.y1 > 3 * win.y0, "the bottom is the widest: {win:?}");
    let px = draw(&s, 0, w, h);
    assert!(luma(px[(h - 5) * w + w / 2]) > 0.6, "white paper");
}

#[test]
fn hostile_numbers_and_tiny_pictures_draw_without_failing() {
    let mut s =
        with(&[(FilmKind::Leak, 1), (FilmKind::Streak, 2), (FilmKind::Burn, 3), (FilmKind::Dust, 4), (FilmKind::Scratches, 5)], FilmFrame::Strip);
    for l in &mut s.film.layers {
        l.size = f64::NAN;
        l.angle = f64::INFINITY;
        l.color = -1e30;
        l.texture = f64::NEG_INFINITY;
        l.fibres = 1e9;
        l.amount = 1e9;
    }
    s.film.border = f64::NAN;
    for frame in FilmFrame::ALL {
        s.film.frame = frame;
        for (w, h) in [(1, 1), (1, 7), (9, 1), (3, 2)] {
            let px = draw(&s, u32::MAX, w, h);
            assert!(px.iter().flatten().all(|v| v.is_finite()), "{frame:?} {w}×{h}");
        }
        let f = Setup::new(&s, 0, OutputSpace::Srgb, true).unwrap();
        // a short buffer is left alone
        let mut short = vec![GREY; 3];
        f.apply_linear(&mut short, 4, 4, [0, 0], [4, 4]);
        f.apply_linear(&mut [], 0, 0, [0, 0], [0, 0]);
    }
}

#[test]
fn an_8_bit_render_and_a_deep_one_draw_alike() {
    let s = with(&[(FilmKind::Leak, 1), (FilmKind::Dust, 2)], FilmFrame::Strip);
    let f = Setup::new(&s, 0, OutputSpace::Srgb, true).unwrap();
    let (w, h) = (90, 60);
    let mut img = Rgba8::new(w, h);
    img.data.fill([118, 118, 118, 255]);
    f.apply_rgba8(&mut img, [0, 0], [w, h], OutputSpace::Srgb.trc());
    let v = OutputSpace::Srgb.trc().decode(118.0 / 255.0);
    let mut deep = DeepImage { width: w, height: h, space: OutputSpace::Srgb, samples: DeepSamples::F32(vec![v; w * h * 3]) };
    f.apply_deep(&mut deep, [0, 0], [w, h]);
    let DeepSamples::F32(d) = &deep.samples else { panic!() };
    for (i, p) in img.data.iter().enumerate() {
        let e = (OutputSpace::Srgb.trc().encode(d[i * 3]) * 255.0 + 0.5) as i32;
        assert!((e - p[0] as i32).abs() <= 1, "pixel {i}: {e} against {}", p[0]);
        assert_eq!(p[3], 255);
    }
}
