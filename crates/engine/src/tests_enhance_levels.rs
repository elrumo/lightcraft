//! FB-UPSCALE: how Super Resolution moves levels. A 96 × 64 picture with a bright sky, a textured
//! near-black shadow and a pure black block, in colour or in black and white (a colour file
//! switched to the B&W treatment), is enlarged and both are rendered; the enlargement is shrunk
//! back by 2 and compared with the original render: luma mean and low percentiles.
#![cfg(feature = "enhance")]

use std::path::{Path, PathBuf};

use lightcraft_catalog::PhotoId;
use lightcraft_develop::Treatment;
use serde_json::json;

use crate::enhance::SUPER_RES_MODEL;
use crate::tests_enhance::ready;

fn scene_png(path: &Path, (w, h): (usize, usize)) {
    let rgb: Vec<u8> = (0..w * h)
        .flat_map(|i| {
            let (x, y) = (i % w, i / w);
            let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
            let n = ((x * 7919 + y * 104729 + (x * y) % 13 * 31) % 97) as f32 / 97.0;
            let v = if fy < 0.35 {
                0.55 + 0.35 * fx
            } else if fx < 0.5 {
                0.02 + 0.03 * n
            } else if fy < 0.7 {
                0.0
            } else {
                0.15 + 0.5 * (fx * 6.0).sin().abs() + 0.05 * n
            };
            // a colour cast, as a real colour photo has (the B&W treatment removes it)
            [(v * 255.0) as u8, (v * 0.9 * 255.0 + 5.0).min(255.0) as u8, (v * 0.7 * 255.0) as u8]
        })
        .collect();
    let png = lightcraft_codecs::encode_png(
        &lightcraft_codecs::EncodeImage::new(w as u32, h as u32, 3, lightcraft_codecs::Samples::U8(&rgb)),
        &lightcraft_codecs::EncodeMeta::default(),
    )
    .unwrap();
    std::fs::write(path, png).unwrap();
}

#[derive(Debug)]
struct Levels {
    mean: f64,
    p01: f64,
    p1: f64,
    p5: f64,
    p99: f64,
    min: f64,
}

fn luma_levels(img: &lightcraft_raster::Rgba8, shrink: usize) -> Levels {
    let (w, h) = (img.width / shrink, img.height / shrink);
    let mut l: Vec<f64> = (0..w * h)
        .map(|i| {
            let (x, y) = (i % w, i / w);
            let mut acc = 0.0;
            for dy in 0..shrink {
                for dx in 0..shrink {
                    let p = img.get(x * shrink + dx, y * shrink + dy);
                    acc += 0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2]);
                }
            }
            acc / (shrink * shrink) as f64
        })
        .collect();
    l.sort_by(f64::total_cmp);
    let pc = |q: f64| l[((l.len() - 1) as f64 * q) as usize];
    Levels { mean: l.iter().sum::<f64>() / l.len() as f64, p01: pc(0.001), p1: pc(0.01), p5: pc(0.05), p99: pc(0.99), min: l[0] }
}

/// Levels (0..255) of the photo's render and of its enlargement's render, shrunk back.
fn before_and_after(tag: &str, bw: bool, model: Option<&Path>) -> (Levels, Levels) {
    before_and_after_of(tag, bw, model, None)
}

/// [`before_and_after`] for a photo file instead of the synthetic scene.
fn before_and_after_of(tag: &str, bw: bool, model: Option<&Path>, photo: Option<&Path>) -> (Levels, Levels) {
    before_and_after_crop(tag, bw, model, photo, None)
}

/// [`before_and_after_of`] on the middle `crop` (a fraction of each side) of the photo.
fn before_and_after_crop(tag: &str, bw: bool, model: Option<&Path>, photo: Option<&Path>, crop: Option<f64>) -> (Levels, Levels) {
    let (mut s, dir, _) = ready(tag, (8, 8));
    if let Some(m) = model {
        std::fs::copy(m, s.enhancer.model_file(SUPER_RES_MODEL).unwrap()).unwrap();
    }
    let path = match photo {
        Some(p) => dir.join(p.file_name().unwrap()),
        None => dir.join("SCENE.png"),
    };
    match photo {
        Some(p) => drop(std::fs::copy(p, &path).unwrap()),
        None => scene_png(&path, (96, 64)),
    }
    let r = s.execute("library.import", &json!({"paths": [path]})).unwrap();
    let id = PhotoId(r["imported"][0].as_u64().unwrap());
    let mut d = s.develop_of(id).unwrap().as_ref().clone();
    if bw {
        d.treatment = Treatment::Bw;
    }
    if let Some(f) = crop {
        d.crop.geometry.rect = lightcraft_geom::Rect { x0: 0.5 - f / 2.0, y0: 0.5 - f / 2.0, x1: 0.5 + f / 2.0, y1: 0.5 + f / 2.0 };
    }
    s.set_develop(id, d, "test").unwrap();
    let r = s.execute("enhance.superRes", &json!({"id": id.0})).unwrap();
    let new = PhotoId(r["id"].as_u64().unwrap());
    let (nw, nh) = {
        let p = s.catalog.photo(new).unwrap();
        (p.width as usize, p.height as usize)
    };
    // the photo as it shows, at half the enlargement's size (a raw's export is its active area, a few
    // pixels off its preview: compare levels, not pixels)
    let before = s.render_now(id, nw / 2, nh / 2).unwrap();
    let after = s.render_now(new, nw, nh).unwrap();
    assert_eq!((after.image.width, after.image.height), (nw, nh));
    let out = (luma_levels(&before.image, 1), luma_levels(&after.image, 2));
    let _ = std::fs::remove_dir_all(&dir);
    out
}

/// The mean must stay, and with a real model (`real`) so must the low percentiles. (A synthetic model's
/// random weights add detail as large as a shadow itself, which no anchor keeps.)
fn assert_kept(what: &str, (a, b): &(Levels, Levels), real: bool) {
    eprintln!("{what}: before {a:?}\n{what}: after  {b:?}");
    assert!((a.mean - b.mean).abs() < 1.5, "{what}: mean {} → {}", a.mean, b.mean);
    if !real {
        return;
    }
    assert!((a.p1 - b.p1).abs() < 1.5, "{what}: 1st percentile {} → {}", a.p1, b.p1);
    assert!((a.p5 - b.p5).abs() < 1.5, "{what}: 5th percentile {} → {}", a.p5, b.p5);
    assert!((a.p01 - b.p01).abs() < 2.0, "{what}: 0.1th percentile {} → {}", a.p01, b.p01);
    assert!((a.p99 - b.p99).abs() < 2.0, "{what}: 99th percentile {} → {}", a.p99, b.p99);
    assert!((a.min - b.min).abs() < 3.0, "{what}: black point {} → {}", a.min, b.min);
}

#[test]
fn super_resolution_keeps_the_levels_in_colour_and_in_black_and_white() {
    assert_kept("colour", &before_and_after("lv-col", false, None), false);
    assert_kept("b&w", &before_and_after("lv-bw", true, None), false);
}

fn real_model() -> Option<PathBuf> {
    let p = std::env::var_os("LIGHTCRAFT_SPAN_MODEL").map(PathBuf::from).or_else(|| {
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join("Library/Application Support/LightCraft/models/nomos-span-2x/2xNomosUni_span_multijpg.safetensors"))
    })?;
    p.is_file().then_some(p)
}

/// The same with the real model, when it is in the user's models folder.
#[test]
fn real_super_resolution_keeps_the_levels_in_colour_and_in_black_and_white() {
    let Some(model) = real_model() else {
        lightcraft_testkit::skip!(Model, "the Nomos SPAN model is not in the models folder (or LIGHTCRAFT_SPAN_MODEL)");
        return;
    };
    assert_kept("real colour", &before_and_after("lvr-col", false, Some(&model)), true);
    assert_kept("real b&w", &before_and_after("lvr-bw", true, Some(&model)), true);
}

/// A real black-and-white photograph (CC0, `corpus/cc0-remove/two-people-beach.jpg`).
#[test]
fn real_photo_keeps_its_black_point() {
    let corpus = std::env::var_os("LIGHTCRAFT_CORPUS").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("corpus"));
    let photo = corpus.join("cc0-remove/two-people-beach.jpg");
    let (Some(model), true) = (real_model(), photo.is_file()) else {
        lightcraft_testkit::skip!(Model, "the Nomos SPAN model or corpus/cc0-remove/two-people-beach.jpg is missing");
        return;
    };
    assert_kept("real photo", &before_and_after_of("lvr-photo", false, Some(&model), Some(&photo)), true);
}

/// A real raw (corpus), its middle part, in colour and in black and white.
#[test]
fn real_raw_keeps_its_black_point() {
    let corpus = std::env::var_os("LIGHTCRAFT_CORPUS").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("corpus"));
    let photo = corpus.join("raw/cr3-canon-r6-raw.cr3");
    let (Some(model), true) = (real_model(), photo.is_file()) else {
        lightcraft_testkit::skip!(Model, "the Nomos SPAN model or corpus/raw/cr3-canon-r6-raw.cr3 is missing");
        return;
    };
    assert_kept("raw colour", &before_and_after_crop("lvr-raw-c", false, Some(&model), Some(&photo), Some(0.2)), true);
    assert_kept("raw b&w", &before_and_after_crop("lvr-raw-bw", true, Some(&model), Some(&photo), Some(0.2)), true);
}

/// An iPhone ProRAW (corpus), its middle part, in colour and in black and white.
#[test]
fn real_proraw_keeps_its_black_point() {
    let corpus = std::env::var_os("LIGHTCRAFT_CORPUS").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("corpus"));
    let photo = corpus.join("raw/dng-apple-iphone17pro-proraw-jxl.dng");
    let (Some(model), true) = (real_model(), photo.is_file()) else {
        lightcraft_testkit::skip!(Model, "the Nomos SPAN model or corpus/raw/dng-apple-iphone17pro-proraw-jxl.dng is missing");
        return;
    };
    assert_kept("proraw colour", &before_and_after_crop("lvr-dng-c", false, Some(&model), Some(&photo), Some(0.2)), true);
    assert_kept("proraw b&w", &before_and_after_crop("lvr-dng-bw", true, Some(&model), Some(&photo), Some(0.2)), true);
}
