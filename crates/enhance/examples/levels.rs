//! Measure how the enlargement moves levels (black point, percentiles, mean) on a synthetic
//! picture, in colour and in black and white (R = G = B).
//!
//! `cargo run --release -p lightcraft-enhance --example levels -- <model.safetensors>`

use lightcraft_enhance::{Options, Span, best_device};
use lightcraft_raster::Rgb32f;

fn scene(w: usize, h: usize, gray: bool) -> Rgb32f {
    Rgb32f::from_fn(w, h, |x, y| {
        let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
        // deterministic texture
        let n = ((x * 7919 + y * 104729 + (x * y) % 13 * 31) % 97) as f32 / 97.0;
        let base = if fy < 0.35 {
            0.55 + 0.35 * fx // sky
        } else if fx < 0.5 {
            0.02 + 0.03 * n // deep shadow, near black, textured
        } else if fy < 0.7 {
            0.0 // pure black block
        } else {
            0.15 + 0.5 * (fx * 6.0).sin().abs() + 0.05 * n // mixed
        };
        let b = base.clamp(0.0, 1.0);
        if gray { [b, b, b] } else { [b, (b * 0.9 + 0.02).min(1.0), (b * 0.7).min(1.0)] }
    })
}

fn luma(p: [f32; 3]) -> f32 {
    0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]
}

fn shrink(img: &Rgb32f, s: usize) -> Rgb32f {
    Rgb32f::from_fn(img.width / s, img.height / s, |x, y| {
        let mut acc = [0f32; 3];
        for dy in 0..s {
            for dx in 0..s {
                let p = img.get(x * s + dx, y * s + dy);
                for c in 0..3 {
                    acc[c] += p[c];
                }
            }
        }
        acc.map(|v| v / (s * s) as f32)
    })
}

fn stats(name: &str, img: &Rgb32f) {
    let mut l: Vec<f32> = img.data.iter().map(|p| luma(*p)).collect();
    l.sort_by(f32::total_cmp);
    let pc = |q: f64| l[((l.len() - 1) as f64 * q) as usize] * 255.0;
    let mean: f64 = l.iter().map(|v| f64::from(*v)).sum::<f64>() / l.len() as f64 * 255.0;
    let chroma: f64 = img.data.iter().map(|p| f64::from((p[0] - p[1]).abs() + (p[2] - p[1]).abs())).sum::<f64>() / img.data.len() as f64 * 255.0;
    println!(
        "{name:28} min {:6.2}  p1 {:6.2}  p5 {:6.2}  p25 {:6.2}  p50 {:6.2}  p99 {:6.2}  mean {:6.2}  |R-G|+|B-G| {:5.2}",
        pc(0.0),
        pc(0.01),
        pc(0.05),
        pc(0.25),
        pc(0.5),
        pc(0.99),
        mean,
        chroma
    );
}

fn main() {
    let model = std::env::args().nth(1).expect("model path");
    let span = Span::load(std::path::Path::new(&model), &best_device()).expect("load");
    for gray in [true, false] {
        let img = scene(192, 192, gray);
        let up = span.upscale(&img, &Options::default(), &mut |_| true).expect("upscale");
        let back = shrink(&up, 2);
        println!("--- {}", if gray { "black and white" } else { "colour" });
        stats("input", &img);
        stats("upscaled (shrunk back)", &back);
        stats("upscaled full", &up);
    }
}
