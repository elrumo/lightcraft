//! No weights needed: a small SPAN with deterministic pseudo-random weights.

use std::collections::HashMap;

use candle_core::{Device, Tensor};
use lightcraft_raster::Rgb32f;

use crate::{Error, Options, Span};

use crate::testing::noise;

fn rnd(shape: &[usize], seed: u64, gain: f32) -> Tensor {
    crate::testing::rnd(shape, seed, gain).unwrap()
}

fn tensors(c: usize, scale: usize, seed: u64, gain: f32) -> HashMap<String, Tensor> {
    crate::testing::tensors(c, scale, seed, gain).unwrap()
}

fn tamed(c: usize, scale: usize, seed: u64) -> HashMap<String, Tensor> {
    crate::testing::tamed(c, scale, seed).unwrap()
}

fn conv3xc(t: &mut HashMap<String, Tensor>, prefix: &str, ci: usize, co: usize, seed: u64) {
    crate::testing::conv3xc(t, prefix, ci, co, seed).unwrap()
}

/// A small model whose output sits well inside 0…1 for any input.
fn model(c: usize, scale: usize, seed: u64) -> Span {
    Span::from_tensors(&tamed(c, scale, seed), &Device::Cpu).unwrap()
}

fn picture(w: usize, h: usize, seed: u64) -> Rgb32f {
    let n = noise(3 * w * h, seed);
    Rgb32f::from_fn(w, h, |x, y| {
        let i = (y * w + x) * 3;
        // smooth gradient plus grain, so tiles differ from each other
        let g = (x as f32 / w as f32 + y as f32 / h as f32) / 2.0;
        [g + 0.1 * n[i], 0.5 + 0.2 * n[i + 1], 1.0 - g + 0.1 * n[i + 2]].map(|v| v.clamp(0.0, 1.0))
    })
}

fn max_diff(a: &Rgb32f, b: &Rgb32f) -> f32 {
    assert_eq!((a.width, a.height), (b.width, b.height));
    a.data.iter().zip(&b.data).flat_map(|(p, q)| p.iter().zip(q).map(|(x, y)| (x - y).abs())).fold(0.0, f32::max)
}

#[test]
fn fusing_the_branches_equals_running_them() {
    let (ci, co) = (5, 7);
    let mut t = HashMap::new();
    conv3xc(&mut t, "c", ci, co, 3);
    let g = |n: &str| t.get(&format!("c.{n}")).unwrap().clone();
    let (w, b) = crate::span::fuse(
        &g("sk.weight"),
        &g("sk.bias"),
        &g("conv.0.weight"),
        &g("conv.0.bias"),
        &g("conv.1.weight"),
        &g("conv.1.bias"),
        &g("conv.2.weight"),
        &g("conv.2.bias"),
    )
    .unwrap();
    assert_eq!(w.dims(), [co, ci, 3, 3]);

    let x = Tensor::from_vec(noise(ci * 9 * 11, 5), (1, ci, 9, 11), &Device::Cpu).unwrap();
    let bias = |b: Tensor| {
        let n = b.elem_count();
        b.reshape((1, n, 1, 1)).unwrap()
    };
    // the training-time computation: zero-pad, 1×1, 3×3 (no padding), 1×1, plus the 1×1 skip
    let padded = x.pad_with_zeros(2, 1, 1).unwrap().pad_with_zeros(3, 1, 1).unwrap();
    let a = padded.conv2d(&g("conv.0.weight"), 0, 1, 1, 1).unwrap().broadcast_add(&bias(g("conv.0.bias"))).unwrap();
    let a = a.conv2d(&g("conv.1.weight"), 0, 1, 1, 1).unwrap().broadcast_add(&bias(g("conv.1.bias"))).unwrap();
    let a = a.conv2d(&g("conv.2.weight"), 0, 1, 1, 1).unwrap().broadcast_add(&bias(g("conv.2.bias"))).unwrap();
    let skip = x.conv2d(&g("sk.weight"), 0, 1, 1, 1).unwrap().broadcast_add(&bias(g("sk.bias"))).unwrap();
    let want = (a + skip).unwrap();

    let got = x.conv2d(&w, 1, 1, 1, 1).unwrap().broadcast_add(&bias(b)).unwrap();
    assert_eq!(got.dims(), want.dims());
    let d = (got - want).unwrap().abs().unwrap().flatten_all().unwrap().max(0).unwrap().to_scalar::<f32>().unwrap();
    assert!(d < 1e-4, "fused and unfused differ by {d}");
}

#[test]
fn tiles_give_the_same_pixels_as_one_pass() {
    let span = model(8, 2, 1);
    assert_eq!(span.scale(), 2);
    let img = picture(70, 53, 9);
    let whole = span.upscale(&img, &Options { tile: 2048, ..Default::default() }, &mut |_| true).unwrap();
    assert_eq!((whole.width, whole.height), (140, 106));
    let spread = whole.data.iter().flatten().fold((1f32, 0f32), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
    assert!(spread.1 - spread.0 > 0.05, "the output is flat: {spread:?}");
    for tile in [16, 23, 40] {
        let tiled = span.upscale(&img, &Options { tile, ..Default::default() }, &mut |_| true).unwrap();
        let d = max_diff(&whole, &tiled);
        assert!(d < 1e-3, "tile {tile}: tiled differs from one pass by {d}");
    }
}

#[test]
fn four_times_models_work_and_progress_is_reported() {
    let span = model(6, 4, 2);
    assert_eq!(span.scale(), 4);
    let mut seen = Vec::new();
    let out = span.upscale(&picture(40, 33, 4), &Options { tile: 16, ..Default::default() }, &mut |p| {
        seen.push(p);
        true
    });
    assert_eq!(out.map(|o| (o.width, o.height)).unwrap(), (160, 132));
    assert_eq!(seen.len(), 3 * 3 + 1);
    assert!(seen.windows(2).all(|w| w[0] <= w[1]) && seen.last() == Some(&1.0));
}

#[test]
fn bad_input_is_an_error_not_a_panic() {
    let span = model(8, 2, 3);
    let empty = Rgb32f { width: 0, height: 5, data: vec![] };
    assert!(matches!(span.upscale(&empty, &Options::default(), &mut |_| true), Err(Error::Image(_))));
    let lying = Rgb32f { width: 10, height: 10, data: vec![[0.0; 3]; 7] };
    assert!(matches!(span.upscale(&lying, &Options::default(), &mut |_| true), Err(Error::Image(_))));
    let big = picture(30, 30, 1);
    assert!(matches!(span.upscale(&big, &Options { max_output_pixels: 100, ..Default::default() }, &mut |_| true), Err(Error::Image(_))));
    // stopping part way
    let mut calls = 0;
    let r = span.upscale(&picture(64, 64, 1), &Options { tile: 16, ..Default::default() }, &mut |_| {
        calls += 1;
        calls < 3
    });
    assert!(matches!(r, Err(Error::Cancelled)));
    // NaN and infinity in a damaged source give finite pixels
    let mut dirty = picture(20, 20, 1);
    dirty.data[3] = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY];
    let out = span.upscale(&dirty, &Options::default(), &mut |_| true).unwrap();
    assert!(out.data.iter().flatten().all(|v| (0.0..=1.0).contains(v)));
}

#[test]
fn damaged_or_foreign_models_are_errors() {
    let dev = Device::Cpu;
    // missing tensors
    let mut t = tensors(8, 2, 1, 0.1);
    t.remove("block_3.c2_r.conv.1.weight");
    assert!(matches!(Span::from_tensors(&t, &dev), Err(Error::Model(_))));
    // the wrong shape for one tensor
    let mut t = tensors(8, 2, 1, 0.1);
    t.insert("conv_cat.weight".into(), rnd(&[8, 7, 1, 1], 1, 1.0));
    assert!(matches!(Span::from_tensors(&t, &dev), Err(Error::Model(_))));
    // an upsampler that is not 3 × s² planes
    let mut t = tensors(8, 2, 1, 0.1);
    t.insert("upsampler.0.weight".into(), rnd(&[10, 8, 3, 3], 1, 1.0));
    assert!(matches!(Span::from_tensors(&t, &dev), Err(Error::Model(_))));
    // single-channel and huge feature counts
    let mut t = tensors(8, 2, 1, 0.1);
    t.insert("conv_1.sk.weight".into(), rnd(&[8, 1, 1, 1], 1, 1.0));
    assert!(Span::from_tensors(&t, &dev).is_err());
    assert!(Span::from_tensors(&HashMap::new(), &dev).is_err());
    // files
    let dir = std::env::temp_dir().join(format!("lc-enhance-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for (name, bytes) in
        [("empty", Vec::new()), ("junk", noise(4096, 3).iter().map(|v| (v * 100.0) as u8).collect()), ("header_lies", [0xff; 64].to_vec())]
    {
        let p = dir.join(name);
        std::fs::write(&p, bytes).unwrap();
        assert!(matches!(Span::load(&p, &dev), Err(Error::Model(_))), "{name}");
    }
    assert!(matches!(Span::load(&dir.join("nope"), &dev), Err(Error::Model(_))));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_model_file_round_trips_and_f16_weights_load() {
    let dir = std::env::temp_dir().join(format!("lc-enhance-rt-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // keep only the fused tensors, in half precision, as an exported model would
    let full = tensors(8, 2, 5, 0.05);
    let fused: HashMap<String, Tensor> = full
        .iter()
        .filter(|(k, _)| !k.contains(".sk.") && !k.contains(".conv."))
        .map(|(k, v)| (k.clone(), v.to_dtype(candle_core::DType::F16).unwrap()))
        .collect();
    let path = dir.join("m.safetensors");
    candle_core::safetensors::save(&fused, &path).unwrap();
    let span = Span::load(&path, &Device::Cpu).unwrap();
    assert_eq!(span.scale(), 2);
    let out = span.upscale(&picture(20, 20, 2), &Options::default(), &mut |_| true).unwrap();
    assert_eq!((out.width, out.height), (40, 40));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_box_blur_and_bicubic_helpers_behave() {
    // a flat plane stays flat, however big the radius; a lone pixel keeps its total
    let flat = vec![0.3f32; 9 * 7];
    assert!(crate::tile::box_blur(&flat, (9, 7), 20).iter().all(|v| (v - 0.3).abs() < 1e-6));
    let mut spike = vec![0f32; 11 * 11];
    spike[5 * 11 + 5] = 1.0;
    let blurred = crate::tile::box_blur(&spike, (11, 11), 2);
    assert!((blurred.iter().sum::<f32>() - 1.0).abs() < 1e-5);
    assert!((blurred[5 * 11 + 5] - 1.0 / 25.0).abs() < 1e-6);
    // bicubic keeps flat planes flat and has the right size
    let up = crate::tile::bicubic(&flat, (9, 7), 3);
    assert_eq!(up.len(), 27 * 21);
    assert!(up.iter().all(|v| (v - 0.3).abs() < 1e-5));
}

#[test]
fn the_enlargement_keeps_the_tones_of_its_input() {
    let span = model(8, 2, 6);
    let img = picture(60, 44, 3);
    let out = span.upscale(&img, &Options::default(), &mut |_| true).unwrap();
    // the same network without the anchor, for comparison
    let planes: Vec<f32> = (0..3).flat_map(|c| img.data.iter().map(move |p| p[c])).collect();
    let input = Tensor::from_vec(planes, (1, 3, 44, 60), &Device::Cpu).unwrap();
    let raw = span.forward(&input).unwrap().flatten_all().unwrap().to_vec1::<f32>().unwrap();

    let plane = |im: &Rgb32f, c: usize| im.data.iter().map(|p| p[c]).collect::<Vec<_>>();
    let (mut with, mut without) = (0f32, 0f32);
    for c in 0..3 {
        let up = crate::tile::box_blur(&crate::tile::bicubic(&plane(&img, c), (60, 44), 2), (120, 88), 8);
        let anchored = crate::tile::box_blur(&plane(&out, c), (120, 88), 8);
        let unanchored =
            crate::tile::box_blur(&raw[c * 120 * 88..(c + 1) * 120 * 88].iter().map(|v| v.clamp(0.0, 1.0)).collect::<Vec<_>>(), (120, 88), 8);
        // away from the borders, where all three blurs see real pixels
        for y in 20..68 {
            for x in 20..100 {
                with = with.max((anchored[y * 120 + x] - up[y * 120 + x]).abs());
                without = without.max((unanchored[y * 120 + x] - up[y * 120 + x]).abs());
            }
        }
    }
    assert!(without > 0.1, "the synthetic model should stray from its input's tones ({without})");
    assert!(with < without / 4.0, "broad tones moved by {with} with the anchor, {without} without");
}
