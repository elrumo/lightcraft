use super::*;

#[test]
fn net_size_keeps_the_short_side_and_the_patch_grid() {
    assert_eq!(net_size(4032, 3024), (686, 518));
    assert_eq!(net_size(3024, 4032), (518, 686));
    assert_eq!(net_size(518, 518), (518, 518));
    // a panorama keeps its long side bounded
    let (w, h) = net_size(12000, 2000);
    assert!(w <= MAX_LONG + PATCH && h < SHORT_SIDE && w % PATCH == 0 && h % PATCH == 0, "{w} × {h}");
    // degenerate sizes still give a valid input
    assert_eq!(net_size(0, 0), (518, 518));
    assert_eq!(net_size(1, 100000).0 % PATCH, 0);
}

#[test]
fn verify_wants_the_exact_file() {
    assert!(!verify(b""));
    assert!(!verify(&[0u8; 64]));
    assert_eq!(hex(&Sha256::digest(b"abc")), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
}

#[test]
fn a_damaged_file_is_an_error() {
    assert!(Estimator::load(b"").is_err());
    assert!(Estimator::load(&[0x08, 0xff, 0xff, 0xff]).is_err());
}

/// With `LIGHTCRAFT_DEPTH_MODEL` (the pinned file): a near object on a far wall comes out nearer.
/// `LIGHTCRAFT_DEPTH_MODEL_FP32` (the fp32 `model.onnx`) also compares the two; with
/// `LIGHTCRAFT_DEPTH_IMAGE` (a JPEG) and `LIGHTCRAFT_DEPTH_OUT` (a folder) the map is written as a PGM.
#[test]
fn the_model_puts_a_near_object_in_front() {
    let Ok(path) = std::env::var("LIGHTCRAFT_DEPTH_MODEL") else { return };
    let bytes = std::fs::read(&path).unwrap();
    assert!(verify(&bytes), "{path} isn't the pinned file");
    let t = web_time();
    let est = Estimator::load(&bytes).unwrap();
    eprintln!("load {:.0} ms", t.elapsed().as_secs_f64() * 1e3);
    let img = match std::env::var("LIGHTCRAFT_DEPTH_IMAGE") {
        Ok(p) => {
            let d = lightcraft_codecs::decode(&std::fs::read(p).unwrap(), lightcraft_codecs::DecodeOptions::fit(1600, 1600)).unwrap();
            let lin = d.to_working();
            Rgb32f::from_fn(lin.width, lin.height, |x, y| lin.data[y * lin.width + x].map(|v| v.max(0.0).powf(1.0 / 2.2)))
        }
        // a bright ball in front of a receding checkered floor
        Err(_) => Rgb32f::from_fn(800, 600, |x, y| {
            let (fx, fy) = (x as f32 / 800.0, y as f32 / 600.0);
            if (fx - 0.5).powi(2) + (fy - 0.55).powi(2) < 0.02 {
                return [0.9, 0.3, 0.2];
            }
            let check = ((x / 40 + y / 40) % 2) as f32;
            [0.3 + 0.3 * check * fy, 0.35, 0.4]
        }),
    };
    let t = web_time();
    let d = est.estimate(&img).unwrap();
    eprintln!("estimate {} × {} -> {} × {}: {:.0} ms", img.width, img.height, d.width, d.height, t.elapsed().as_secs_f64() * 1e3);
    if std::env::var("LIGHTCRAFT_DEPTH_IMAGE").is_err() {
        let at = |fx: f32, fy: f32| d.data[(fy * d.height as f32) as usize * d.width + (fx * d.width as f32) as usize];
        assert!(at(0.5, 0.55) > at(0.5, 0.1), "ball {} vs top {}", at(0.5, 0.55), at(0.5, 0.1));
    }
    if let Ok(dir) = std::env::var("LIGHTCRAFT_DEPTH_OUT") {
        write_pgm(&d, &format!("{dir}/depth.pgm"));
    }
    if let Ok(fp32) = std::env::var("LIGHTCRAFT_DEPTH_MODEL_FP32") {
        let full = Estimator::load(&std::fs::read(fp32).unwrap()).unwrap();
        let t = web_time();
        let f = full.estimate(&img).unwrap();
        eprintln!("fp32 estimate: {:.0} ms", t.elapsed().as_secs_f64() * 1e3);
        let (lo, hi) = f.data.iter().fold((f32::MAX, f32::MIN), |(a, b), &v| (a.min(v), b.max(v)));
        let mut diffs: Vec<f32> = f.data.iter().zip(&d.data).map(|(a, b)| (a - b).abs() / (hi - lo).max(1e-6)).collect();
        diffs.sort_by(f32::total_cmp);
        eprintln!("int8 vs fp32, share of the range: median {:.4}, p99 {:.4}, max {:.4}", diffs[diffs.len() / 2], diffs[diffs.len() * 99 / 100], diffs[diffs.len() - 1]);
        if let Ok(dir) = std::env::var("LIGHTCRAFT_DEPTH_OUT") {
            write_pgm(&f, &format!("{dir}/depth_fp32.pgm"));
        }
    }
}

fn web_time() -> std::time::Instant {
    std::time::Instant::now()
}

fn write_pgm(p: &Plane, path: &str) {
    let (lo, hi) = p.data.iter().fold((f32::MAX, f32::MIN), |(a, b), &v| (a.min(v), b.max(v)));
    let mut out = format!("P5\n{} {}\n255\n", p.width, p.height).into_bytes();
    out.extend(p.data.iter().map(|v| ((v - lo) / (hi - lo).max(1e-6) * 255.0) as u8));
    std::fs::write(path, out).unwrap();
}
