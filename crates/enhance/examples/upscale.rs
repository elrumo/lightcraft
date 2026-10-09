//! Try a SPAN model on a picture.
//!
//! `cargo run --release -p lightcraft-enhance --example upscale -- <model.safetensors> <in.jpg|png|tif> <out.png> [--check] [--crop N]`
//!
//! Without `--check`: enlarge the picture and write it. With `--check`: shrink it by the model's
//! scale, enlarge that again with the model and with bicubic interpolation, report the PSNR of
//! each against the original, and write `<out>` as a side-by-side crop (original | bicubic |
//! model) of the busiest 480 × 320 area, for looking at.

use std::time::Instant;

use lightcraft_codecs::{DecodeOptions, EncodeImage, EncodeMeta, Samples, decode, encode_png};
use lightcraft_enhance::{Options, Span, best_device};
use lightcraft_raster::Rgb32f;

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [model, input, output, rest @ ..] = args.as_slice() else {
        return Err("usage: upscale <model.safetensors> <in> <out.png> [--check]".into());
    };
    let check = rest.iter().any(|a| a == "--check");

    let t = Instant::now();
    let device = best_device();
    let span = Span::load(std::path::Path::new(model), &device).map_err(|e| e.to_string())?;
    eprintln!("model: {}× on {:?}, loaded in {:.2?}", span.scale(), device, t.elapsed());

    let bytes = std::fs::read(input).map_err(|e| format!("{input}: {e}"))?;
    let decoded = decode(&bytes, DecodeOptions::default()).map_err(|e| e.to_string())?;
    let srgb = decoded.to_srgb8();
    let img = Rgb32f::from_fn(srgb.width, srgb.height, |x, y| {
        let p = srgb.get(x, y);
        [f32::from(p[0]) / 255.0, f32::from(p[1]) / 255.0, f32::from(p[2]) / 255.0]
    });
    let img = match rest.iter().position(|a| a == "--crop").and_then(|i| rest.get(i + 1)).and_then(|n| n.parse::<usize>().ok()) {
        Some(n) if n < img.width && n < img.height => {
            let (x0, y0) = ((img.width - n) / 2, (img.height - n) / 2);
            Rgb32f::from_fn(n, n, |x, y| img.get(x0 + x, y0 + y))
        }
        _ => img,
    };
    eprintln!("input: {} × {}", img.width, img.height);

    let opts = Options::default();
    let mut last = -1.0f32;
    let mut progress = |p: f32| {
        if p - last >= 0.1 || p >= 1.0 {
            eprint!("\r  {:3.0} %", p * 100.0);
            last = p;
        }
        true
    };

    if !check {
        let t = Instant::now();
        let out = span.upscale(&img, &opts, &mut progress).map_err(|e| e.to_string())?;
        eprintln!("\nenlarged to {} × {} in {:.2?}", out.width, out.height, t.elapsed());
        return write_png(output, &out);
    }

    // shrink by the scale (box filter), crop to a multiple
    let s = span.scale();
    let (w, h) = (img.width / s * s, img.height / s * s);
    let truth = Rgb32f::from_fn(w, h, |x, y| img.get(x, y));
    let small = shrink(&truth, s);
    let t = Instant::now();
    let sr = span.upscale(&small, &opts, &mut progress).map_err(|e| e.to_string())?;
    eprintln!("\nmodel: {} × {} → {} × {} in {:.2?}", small.width, small.height, sr.width, sr.height, t.elapsed());
    let bicubic = bicubic_up(&small, s);
    // an enlargement should shrink back to the picture it came from (no detail needed to judge)
    let (back, back_bi) = (shrink(&sr, s), shrink(&bicubic, s));
    println!("shrunk back to the input: model {:.2} dB, bicubic {:.2} dB", psnr(&back, &small), psnr(&back_bi, &small));
    let (p_sr, p_bi) = (psnr(&truth, &sr), psnr(&truth, &bicubic));
    println!("PSNR vs original: model {p_sr:.2} dB, bicubic {p_bi:.2} dB (model {:+.2} dB)", p_sr - p_bi);

    // the busiest area: most local contrast in the original
    let (cw, ch) = (480.min(w), 320.min(h));
    let mut best = (0usize, 0usize, -1.0f32);
    for y in (0..h - ch + 1).step_by(80) {
        for x in (0..w - cw + 1).step_by(80) {
            let mut e = 0.0f32;
            for yy in (y..y + ch).step_by(4) {
                for xx in (x..x + cw).step_by(4) {
                    let (a, b) = (truth.get(xx, yy)[1], truth.get((xx + 1).min(w - 1), yy)[1]);
                    e += (a - b).abs();
                }
            }
            if e > best.2 {
                best = (x, y, e);
            }
        }
    }
    let (bx, by, _) = best;
    let strip = Rgb32f::from_fn(cw * 3, ch, |x, y| {
        let (panel, xx) = (x / cw, x % cw);
        match panel {
            0 => truth.get(bx + xx, by + y),
            1 => bicubic.get(bx + xx, by + y),
            _ => sr.get(bx + xx, by + y),
        }
    });
    write_png(output, &strip)
}

fn write_png(path: &str, img: &Rgb32f) -> Result<(), String> {
    let bytes: Vec<u8> = img.data.iter().flat_map(|p| p.map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)).collect();
    let png = encode_png(&EncodeImage::new(img.width as u32, img.height as u32, 3, Samples::U8(&bytes)), &EncodeMeta::default())
        .map_err(|e| e.to_string())?;
    std::fs::write(path, png).map_err(|e| format!("{path}: {e}"))?;
    eprintln!("wrote {path}");
    Ok(())
}

/// PSNR in 8-bit units over all channels.
fn psnr(a: &Rgb32f, b: &Rgb32f) -> f64 {
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as f64;
    let (mut se, mut n) = (0f64, 0f64);
    for (p, r) in a.data.iter().zip(&b.data) {
        for c in 0..3 {
            let d = q(p[c]) - q(r[c]);
            se += d * d;
            n += 1.0;
        }
    }
    10.0 * (255.0 * 255.0 / (se / n).max(1e-12)).log10()
}

/// Catmull-Rom enlargement by an integer factor (edges extended).
fn bicubic_up(img: &Rgb32f, s: usize) -> Rgb32f {
    let w = |t: f32| -> [f32; 4] {
        let (t2, t3) = (t * t, t * t * t);
        [(-0.5 * t3 + t2 - 0.5 * t), (1.5 * t3 - 2.5 * t2 + 1.0), (-1.5 * t3 + 2.0 * t2 + 0.5 * t), (0.5 * t3 - 0.5 * t2)]
    };
    Rgb32f::from_fn(img.width * s, img.height * s, |x, y| {
        let (fx, fy) = ((x as f32 + 0.5) / s as f32 - 0.5, (y as f32 + 0.5) / s as f32 - 0.5);
        let (ix, iy) = (fx.floor(), fy.floor());
        let (wx, wy) = (w(fx - ix), w(fy - iy));
        let mut acc = [0f32; 3];
        for (j, wj) in wy.iter().enumerate() {
            for (i, wi) in wx.iter().enumerate() {
                let p = img.get_clamped(ix as isize + i as isize - 1, iy as isize + j as isize - 1);
                for c in 0..3 {
                    acc[c] += wi * wj * p[c];
                }
            }
        }
        acc
    })
}

/// Box-filter reduction by an integer factor.
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
