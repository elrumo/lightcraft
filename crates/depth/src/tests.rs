use super::*;

#[test]
fn net_size_keeps_the_short_side_and_the_patch_grid() {
    assert_eq!(net_size(4032, 3024), (672, 504));
    assert_eq!(net_size(3024, 4032), (504, 672));
    assert_eq!(net_size(504, 504), (504, 504));
    // a panorama keeps its long side bounded
    let (w, h) = net_size(12000, 2000);
    assert!(w <= MAX_LONG + PATCH && h < SHORT_SIDE && w % PATCH == 0 && h % PATCH == 0, "{w} × {h}");
    // degenerate sizes still give a valid input
    assert_eq!(net_size(0, 0), (504, 504));
    assert_eq!(net_size(1, 100000).0 % PATCH, 0);
}

#[test]
fn verify_wants_the_exact_files() {
    for size in Size::ALL {
        assert!(!size.verify(b"", b""));
        assert!(!size.verify(&[0u8; 64], &[0u8; 64]));
        assert_eq!(Size::from_name(size.name()), Some(size));
    }
    assert_eq!(Size::default(), Size::Small, "Small is the default");
    assert_eq!(Size::from_name("giant"), None);
    assert_eq!(hex(&Sha256::digest(b"abc")), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
}

#[test]
fn a_damaged_file_is_an_error() {
    assert!(Estimator::load(b"", Vec::new()).is_err());
    assert!(Estimator::load(&[0x08, 0xff, 0xff, 0xff], Vec::new()).is_err());
}

/// The size whose pinned files are in `dir` (`model.onnx` and `model.onnx_data`), loaded.
fn load_dir(dir: &str) -> (Size, Estimator) {
    let graph = std::fs::read(format!("{dir}/{GRAPH_FILE}")).unwrap();
    let data = std::fs::read(format!("{dir}/{DATA_FILE}")).unwrap();
    let size = Size::ALL.into_iter().find(|s| s.verify(&graph, &data)).unwrap_or_else(|| panic!("{dir} isn't a pinned size"));
    (size, Estimator::load(&graph, data).unwrap())
}

/// With `LIGHTCRAFT_DEPTH_MODEL` (a folder with a size's pinned files): a near object on a far wall comes out
/// nearer. With `LIGHTCRAFT_DEPTH_IMAGE` (a JPEG) and `LIGHTCRAFT_DEPTH_OUT` (a folder) the map is written as a PGM.
#[test]
fn the_model_puts_a_near_object_in_front() {
    let Ok(dir) = std::env::var("LIGHTCRAFT_DEPTH_MODEL") else { return };
    let t = web_time();
    let (size, est) = load_dir(&dir);
    eprintln!("{} load {:.0} ms", size.label(), t.elapsed().as_secs_f64() * 1e3);
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
    if let Ok(out) = std::env::var("LIGHTCRAFT_DEPTH_OUT") {
        write_pgm(&d, &format!("{out}/depth-{}.pgm", size.name()));
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

/// With `LIGHTCRAFT_DEPTH_GPU` (the V2 weights) and `LIGHTCRAFT_DEPTH_V2_ONNX` (V2 Small's fp32 export, `model.onnx`): the GPU
/// gives the same depth as that export (correlation above 0.99) and is faster. (V2 only: DA3 has no GPU port.)
#[cfg(feature = "gpu")]
#[test]
fn the_gpu_and_the_cpu_see_the_same_depth() {
    let (Ok(_), Ok(weights)) = (std::env::var("LIGHTCRAFT_DEPTH_V2_ONNX"), std::env::var("LIGHTCRAFT_DEPTH_GPU")) else { return };
    let Some(gpu) = Estimator::load_gpu(std::path::Path::new(&weights)).unwrap() else { return };
    let pictures: Vec<Rgb32f> = match std::env::var("LIGHTCRAFT_DEPTH_IMAGE") {
        Ok(p) => vec![{
            let d = lightcraft_codecs::decode(&std::fs::read(p).unwrap(), lightcraft_codecs::DecodeOptions::fit(1600, 1600)).unwrap();
            let lin = d.to_working();
            Rgb32f::from_fn(lin.width, lin.height, |x, y| lin.data[y * lin.width + x].map(|v| v.max(0.0).powf(1.0 / 2.2)))
        }],
        Err(_) => vec![Rgb32f::from_fn(800, 600, |x, y| {
            let (fx, fy) = (x as f32 / 800.0, y as f32 / 600.0);
            if (fx - 0.5).powi(2) + (fy - 0.55).powi(2) < 0.02 {
                return [0.9, 0.3, 0.2];
            }
            let check = ((x / 40 + y / 40) % 2) as f32;
            [0.3 + 0.3 * check * fy, 0.35, 0.4]
        })],
    };
    let correlation = |a: &Plane, b: &Plane| {
        let n = a.data.len() as f64;
        let (ma, mb) = (a.data.iter().map(|&v| f64::from(v)).sum::<f64>() / n, b.data.iter().map(|&v| f64::from(v)).sum::<f64>() / n);
        let (mut sab, mut saa, mut sbb) = (0.0, 0.0, 0.0);
        for (&x, &y) in a.data.iter().zip(&b.data) {
            let (x, y) = (f64::from(x) - ma, f64::from(y) - mb);
            (sab, saa, sbb) = (sab + x * y, saa + x * x, sbb + y * y);
        }
        sab / (saa.sqrt() * sbb.sqrt())
    };
    // (the V2 fp32 graph is a plain input→depth network: run it through rten directly as the truth)
    let truth = {
        let bytes = std::fs::read(std::env::var("LIGHTCRAFT_DEPTH_V2_ONNX").unwrap()).unwrap();
        let fixed = lightcraft_vision::onnxfix::inference_only(&bytes).unwrap();
        let m = rten::ModelOptions::with_all_ops().load(fixed).unwrap();
        let (i, o) = (m.input_ids()[0], m.output_ids()[0]);
        move |img: &Rgb32f| -> Plane {
            let (w, h) = net_size_at(V2_SHORT_SIDE, V2_MAX_LONG, img.width, img.height);
            let small = lightcraft_raster::resample::resize(img, w, h, lightcraft_raster::resample::Filter::Mitchell);
            let mut px = vec![0f32; 3 * w * h];
            for (k, p) in small.data.iter().enumerate() {
                for c in 0..3 {
                    px[c * w * h + k] = (p[c].clamp(0.0, 1.0) - MEAN[c]) / STD[c];
                }
            }
            let v = rten::ValueView::from_shape([1, 3, h, w], &px[..]).unwrap();
            let out = m.run(vec![(i, v.into())], &[o], None).unwrap();
            let t = out[0].as_tensor_view::<f32>().unwrap();
            Plane { width: w, height: h, data: rten_tensor::AsView::to_vec(&t) }
        }
    };
    for img in pictures {
        // (twice: the first one with a new size compiles the GPU's kernels)
        gpu.estimate(&img).unwrap();
        let t = web_time();
        let a = truth(&img);
        let ct = t.elapsed();
        let t = web_time();
        let b = gpu.estimate(&img).unwrap();
        let gt = t.elapsed();
        assert_eq!((a.width, a.height), (b.width, b.height));
        let r = correlation(&a, &b);
        eprintln!("{} × {}: CPU {ct:?}, GPU {gt:?}, correlation {r:.4}", img.width, img.height);
        assert!(r > 0.99, "the GPU's depth differs from the full-precision model's: correlation {r}");
    }
}
