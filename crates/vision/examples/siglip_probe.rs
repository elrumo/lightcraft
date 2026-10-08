//! Ranks captions against photos with SigLIP 2, to see the model work on real images.
//!
//! cargo run --release -p lightcraft-vision --example siglip_probe -- MODEL_DIR "a dog" "a cat" -- a.jpg b.jpg
//!
//! `MODEL_DIR` holds `model.safetensors` and `tokenizer.json`. Prints each photo's cosine
//! similarity with every caption (the best one marked) and the time each step took.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::time::Instant;

use lightcraft_raster::{Image, Rgba8};
use lightcraft_vision::siglip::SigLip;

fn load_photo(path: &str) -> Rgba8 {
    let img = image::open(path).unwrap_or_else(|e| panic!("{path}: {e}")).to_rgba8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    Image { width: w, height: h, data: img.pixels().map(|p| p.0).collect() }
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    dot / (a.iter().map(|x| x * x).sum::<f32>().sqrt() * b.iter().map(|x| x * x).sum::<f32>().sqrt())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((dir, rest)) = args.split_first() else { panic!("usage: siglip_probe MODEL_DIR CAPTION... -- IMAGE...") };
    let split = rest.iter().position(|a| a == "--").unwrap_or(rest.len());
    let (captions, photos) = (&rest[..split], rest.get(split + 1..).unwrap_or(&[]));

    let t = Instant::now();
    let model = SigLip::load(Path::new(dir)).unwrap();
    println!("loaded on {:?} in {:?}", model.device(), t.elapsed());

    let texts: Vec<Vec<f32>> = captions
        .iter()
        .map(|c| {
            let t = Instant::now();
            let v = model.encode_text(c).unwrap();
            println!("caption in {:?}: {c}", t.elapsed());
            v
        })
        .collect();
    // the same again, now that the kernels are compiled
    if let Some(c) = captions.first() {
        let t = Instant::now();
        model.encode_text(c).unwrap();
        println!("first caption again in {:?}", t.elapsed());
    }

    for (i, path) in photos.iter().enumerate() {
        let img = load_photo(path);
        let t = Instant::now();
        let v = model.encode_image(&img).unwrap();
        let took = t.elapsed();
        let scores: Vec<f32> = texts.iter().map(|t| cosine(&v, t)).collect();
        let best = scores.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).map(|(i, _)| i);
        println!("\n{path}  ({}x{}, embedded in {took:?}{})", img.width, img.height, if i == 0 { ", includes warm-up" } else { "" });
        for (j, (c, s)) in captions.iter().zip(&scores).enumerate() {
            println!("  {} {s:+.4}  {c}", if Some(j) == best { "*" } else { " " });
        }
    }
}
