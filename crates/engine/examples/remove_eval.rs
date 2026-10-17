//! Heal / Remove / AI Remove quality, measured against the photo they were painted on.
//!
//! `cargo run --release -p lightcraft-engine --example remove_eval -- [--out sheet.png] [--feather 0]
//!  [--positions 6] [--images N] [--edge 1200] [--tile 150] [--removal] [photo.jpg …]`
//!
//! For each picture (the procedural demo scenes, the CC0 photos in `corpus/cc0-remove/`, and any
//! files named on the command line) it paints dabs of two sizes at fixed places, with each mode,
//! applies them in linear light (`spots::apply`, the stage the renderer uses) and compares with the
//! untouched picture. The place under a dab is usually plain (sky, sand, wall), so what was there
//! is a fair answer for the low frequencies:
//!
//! - **seam**: how much more the colour steps across the dab's edge than it does in the photo (log
//!   ratio of a band just inside against one just outside, per channel, averaged round the edge).
//! - **tone**: how far the fill's blurred colour (σ = r/2) is from the photo's, inside the dab.
//! - **grain**: how far the fill's fine-detail strength is from the photo's right around it
//!   (|ln(std inside / std around)| of the picture minus its σ 1.5 blur).
//!
//! Medians and means over all cases; lower is better. The AI rows run when `LIGHTCRAFT_MIGAN` /
//! `LIGHTCRAFT_LAMA` name the model files (as the tests use them). Feather defaults to 0 here: a
//! feathered edge hides exactly the mismatch this measures. Writes a contact sheet (photo | modes)
//! of every few cases to `--out`.

use std::time::Instant;

use lightcraft_codecs::{DecodeOptions, EncodeImage, EncodeMeta, Samples, decode, encode_png};
use lightcraft_develop::{DevelopSettings, Spot, SpotMode};
use lightcraft_erase::lama::Lama;
use lightcraft_erase::{Eraser, Painter, fill_with};
use lightcraft_geom::Point;
use lightcraft_pipeline::{SourceInfo, frame_for, spots};
use lightcraft_raster::Rgb32f;
use lightcraft_raster::blur::gaussian;
use lightcraft_raster::resample::{Filter, resize};

/// Where the dabs go, as fractions of the picture, and how big they are (fractions of the long edge).
const PLACES: [(f64, f64); 6] = [(0.25, 0.3), (0.5, 0.3), (0.75, 0.3), (0.25, 0.7), (0.5, 0.7), (0.75, 0.7)];
const SIZES: [f64; 2] = [0.02, 0.05];
/// Every this many cases get a row in the contact sheet.
const SHEET_EVERY: usize = 9;
/// Side of a contact-sheet tile (`--tile`).
static TILE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(150);

fn tile() -> usize {
    TILE.load(std::sync::atomic::Ordering::Relaxed)
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

#[derive(Clone, Copy)]
enum Mode {
    Heal,
    Remove,
    Migan,
    Lama,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Mode::Heal => "heal",
            Mode::Remove => "remove",
            Mode::Migan => "ai migan",
            Mode::Lama => "ai lama",
        }
    }
}

#[derive(Clone, Copy, Default)]
struct Score {
    seam: f64,
    tone: f64,
    grain: f64,
    secs: f64,
}

struct Case {
    image: usize,
    pos: (f64, f64),
    size: f64,
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let out = flag("--out").unwrap_or_else(|| "remove_eval.png".into());
    let feather: f64 = flag("--feather").and_then(|v| v.parse().ok()).unwrap_or(0.0);
    let n_pos: usize = flag("--positions").and_then(|v| v.parse().ok()).unwrap_or(PLACES.len()).min(PLACES.len());
    let n_img: usize = flag("--images").and_then(|v| v.parse().ok()).unwrap_or(usize::MAX);
    if let Some(t) = flag("--tile").and_then(|v| v.parse::<usize>().ok()) {
        TILE.store(t.clamp(32, 600), std::sync::atomic::Ordering::Relaxed);
    }
    let edge: usize = flag("--edge").and_then(|v| v.parse().ok()).unwrap_or(1200);
    let files: Vec<String> = {
        let mut skip_next = false;
        args.iter()
            .filter(|a| {
                if skip_next {
                    skip_next = false;
                    return false;
                }
                if a.starts_with("--") {
                    skip_next = a.as_str() != "--removal";
                    return false;
                }
                true
            })
            .cloned()
            .collect()
    };

    // pictures: the CC0 photos (if the corpus is here), what was named, then the demo scenes
    let mut images: Vec<(String, Rgb32f)> = Vec::new();
    let corpus = std::env::var("LIGHTCRAFT_CORPUS").unwrap_or_else(|_| "corpus".into());
    let mut paths: Vec<String> = std::fs::read_dir(format!("{corpus}/cc0-remove"))
        .map(|d| d.flatten().map(|e| e.path().to_string_lossy().into_owned()).filter(|p| p.ends_with(".jpg")).collect())
        .unwrap_or_default();
    paths.sort();
    paths.extend(files);
    for p in paths {
        let bytes = std::fs::read(&p).map_err(|e| format!("{p}: {e}"))?;
        let opts = DecodeOptions { max_size: Some((edge as u32, edge as u32)), ..DecodeOptions::default() };
        let d = decode(&bytes, opts).map_err(|e| format!("{p}: {e}"))?;
        images.push((p, d.image));
    }
    // the scenes last: they are smooth and noiseless, so `--images N` keeps the real photos
    images.extend(lightcraft_scenes::demo_library().iter().map(|s| (s.name.clone(), s.render_fit(900))));
    images.truncate(n_img);

    let mut modes = vec![Mode::Heal, Mode::Remove];
    let load = |var: &str| -> Option<Vec<u8>> { std::env::var(var).ok().and_then(|p| std::fs::read(p).ok()) };
    let migan = load("LIGHTCRAFT_MIGAN").and_then(|b| Eraser::load(&b).ok());
    let lama = load("LIGHTCRAFT_LAMA").and_then(|b| Lama::load(&b).ok());
    if migan.is_some() {
        modes.push(Mode::Migan);
    } else {
        eprintln!("(no LIGHTCRAFT_MIGAN model file: the MI-GAN row is skipped)");
    }
    if lama.is_some() {
        modes.push(Mode::Lama);
    } else {
        eprintln!("(no LIGHTCRAFT_LAMA model file: the LaMa row is skipped)");
    }

    let mut cases = Vec::new();
    for image in 0..images.len() {
        for &size in &SIZES {
            for &pos in PLACES.iter().take(n_pos) {
                cases.push(Case { image, pos, size });
            }
        }
    }

    let mut scores: Vec<Vec<Score>> = vec![Vec::new(); modes.len()];
    let mut sheet_rows: Vec<Vec<Rgb32f>> = Vec::new();
    for (ci, case) in cases.iter().enumerate() {
        let Some((_, src)) = images.get(case.image) else { continue };
        let r = case.size * src.width.max(src.height) as f64;
        let (cx, cy) = (case.pos.0 * src.width as f64, case.pos.1 * src.height as f64);
        let mut row = vec![crop(src, cx, cy, 2.2 * r)];
        for (mi, mode) in modes.iter().enumerate() {
            let t = Instant::now();
            let painter: Option<&dyn Painter> = match mode {
                Mode::Migan => migan.as_ref().map(|m| m as &dyn Painter),
                Mode::Lama => lama.as_ref().map(|m| m as &dyn Painter),
                _ => None,
            };
            let Some(done) = paint(src, &[case.pos], case.size, *mode, painter, feather) else {
                eprintln!("case {ci} {}: no result", mode.name());
                continue;
            };
            let mut s = measure(src, &done, cx, cy, r);
            s.secs = t.elapsed().as_secs_f64();
            if let Some(v) = scores.get_mut(mi) {
                v.push(s);
            }
            if ci % SHEET_EVERY == 0 {
                row.push(crop(&done, cx, cy, 2.2 * r));
            }
        }
        if ci % SHEET_EVERY == 0 {
            sheet_rows.push(row);
        }
    }

    println!("{} cases, {} pictures, feather {feather}", cases.len(), images.len());
    println!(
        "{:<10} {:>6} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>7}",
        "mode", "n", "seam med", "seam avg", "tone med", "tone avg", "grain med", "grain avg", "s/case"
    );
    for (mode, v) in modes.iter().zip(&scores) {
        let col = |f: fn(&Score) -> f64| -> (f64, f64) {
            let mut x: Vec<f64> = v.iter().map(f).filter(|y| y.is_finite()).collect();
            x.sort_by(f64::total_cmp);
            let avg = x.iter().sum::<f64>() / x.len().max(1) as f64;
            (x.get(x.len() / 2).copied().unwrap_or(f64::NAN), avg)
        };
        let (s, t, g, c) = (col(|s| s.seam), col(|s| s.tone), col(|s| s.grain), col(|s| s.secs));
        println!("{:<10} {:>6} {:>8.4} {:>8.4} {:>8.4} {:>8.4} {:>8.4} {:>8.4} {:>7.2}", mode.name(), v.len(), s.0, s.1, t.0, t.1, g.0, g.1, c.1);
    }
    write_sheet(&out, &sheet_rows)?;
    if args.iter().any(|a| a == "--removal") {
        removal(&corpus, &modes, migan.as_ref(), lama.as_ref(), feather, &out.replace(".png", "-removal.png"))?;
    }
    Ok(())
}

/// Strokes over real people in the CC0 photos: (file, stroke in normalized coordinates, size in long edges).
type Task = (&'static str, &'static [(f64, f64)], f64);
const REMOVALS: [Task; 4] = [
    ("people-standing-beach.jpg", &[(0.491, 0.614), (0.491, 0.678)], 0.02),
    ("people-standing-beach.jpg", &[(0.442, 0.590), (0.442, 0.640)], 0.018),
    ("people-standing-beach.jpg", &[(0.613, 0.597), (0.639, 0.603)], 0.018),
    ("two-people-beach.jpg", &[(0.523, 0.771), (0.542, 0.846)], 0.038),
];

/// The strokes of [`REMOVALS`] with each mode: a row a stroke, the photo then each mode, for looking at.
fn removal(corpus: &str, modes: &[Mode], migan: Option<&Eraser>, lama: Option<&Lama>, feather: f64, out: &str) -> Result<(), String> {
    let mut rows = Vec::new();
    for (file, pts, size) in REMOVALS {
        let path = format!("{corpus}/cc0-remove/{file}");
        let Ok(bytes) = std::fs::read(&path) else {
            eprintln!("(no {path}: skipped)");
            continue;
        };
        let d = decode(&bytes, DecodeOptions::default()).map_err(|e| format!("{path}: {e}"))?;
        let src = d.image;
        let long = src.width.max(src.height) as f64;
        let (cx, cy) = (
            pts.iter().map(|p| p.0).sum::<f64>() / pts.len() as f64 * src.width as f64,
            pts.iter().map(|p| p.1).sum::<f64>() / pts.len() as f64 * src.height as f64,
        );
        let side = 9.0 * size * long;
        let mut row = vec![crop(&src, cx, cy, side)];
        for mode in modes {
            let painter: Option<&dyn Painter> = match mode {
                Mode::Migan => migan.map(|m| m as &dyn Painter),
                Mode::Lama => lama.map(|m| m as &dyn Painter),
                _ => None,
            };
            match paint(&src, pts, size, *mode, painter, feather) {
                Some(done) => row.push(crop(&done, cx, cy, side)),
                None => row.push(Rgb32f::filled(tile(), tile(), [0.0; 3])),
            }
        }
        rows.push(row);
    }
    write_sheet(out, &rows)
}

/// `src` with one dab painted in `mode`, in linear light; `None` when the model can't paint it.
fn paint(src: &Rgb32f, pts: &[(f64, f64)], size: f64, mode: Mode, painter: Option<&dyn Painter>, feather: f64) -> Option<Rgb32f> {
    let info = SourceInfo::default();
    let spot_mode = match mode {
        Mode::Heal => SpotMode::Heal,
        _ => SpotMode::Remove,
    };
    let mut s = DevelopSettings::default();
    s.spots.push(Spot { mode: spot_mode, points: pts.iter().map(|p| Point::new(p.0, p.1)).collect(), size, feather, ..Default::default() });
    if let Some(p) = painter {
        let input = spots::erase_input(src, &info, &s, 0)?;
        let filled = fill_with(p, &input.image, &input.hole).ok()?;
        s.spots.first_mut()?.fill = Some(input.finish(&filled)?);
    }
    let frame = frame_for(src, &info, &s, true);
    let mut img = src.clone();
    let ppl = frame.px_per_long(img.width);
    spots::apply(&mut img, &s.spots, &frame, ppl);
    Some(img)
}

const EPS: f32 = 1e-3;

fn ln_ratio(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|c| (a[c].max(EPS) / b[c].max(EPS)).ln())
}

/// A square window of `img` of side `side` around (`cx`, `cy`), clamped to the picture.
fn window(img: &Rgb32f, cx: f64, cy: f64, side: f64) -> (Rgb32f, usize, usize) {
    let side = (side.round() as usize).clamp(8, img.width.min(img.height));
    let x0 = ((cx - side as f64 / 2.0).round().max(0.0) as usize).min(img.width - side);
    let y0 = ((cy - side as f64 / 2.0).round().max(0.0) as usize).min(img.height - side);
    (Rgb32f::from_fn(side, side, |x, y| img.get(x0 + x, y0 + y)), x0, y0)
}

fn measure(orig: &Rgb32f, edited: &Rgb32f, cx: f64, cy: f64, r: f64) -> Score {
    // seam: the extra step across the edge, round it
    let ring = |img: &Rgb32f, rad: f64, a: f64| -> [f32; 3] {
        // three samples along the tangent keep a grain speck from deciding
        let mut sum = [0f32; 3];
        for d in [-1.5f64, 0.0, 1.5] {
            let (ux, uy) = (a.cos(), a.sin());
            let (x, y) = (cx + ux * rad - uy * d, cy + uy * rad + ux * d);
            let p = img.sample_bilinear(x as f32, y as f32);
            for c in 0..3 {
                sum[c] += p[c] / 3.0;
            }
        }
        sum
    };
    let n = 24;
    let mut seam = 0f64;
    for k in 0..n {
        let a = k as f64 / n as f64 * std::f64::consts::TAU;
        let e = ln_ratio(ring(edited, r - 2.0, a), ring(edited, r + 2.0, a));
        let o = ln_ratio(ring(orig, r - 2.0, a), ring(orig, r + 2.0, a));
        seam += (0..3).map(|c| f64::from((e[c] - o[c]).abs())).sum::<f64>() / 3.0;
    }
    seam /= f64::from(n);

    // tone and grain on a window of 6 radii
    let (we, ox, oy) = window(edited, cx, cy, 6.0 * r);
    let (wo, _, _) = window(orig, cx, cy, 6.0 * r);
    let (lx, ly) = (cx - ox as f64, cy - oy as f64);
    let dist = |x: usize, y: usize| (x as f64 + 0.5 - lx).hypot(y as f64 + 0.5 - ly);
    let (be, bo) = (gaussian(&we, (r / 2.0) as f32), gaussian(&wo, (r / 2.0) as f32));
    let (mut tone, mut tn) = (0f64, 0f64);
    let (fe, fo) = (gaussian(&we, 1.5), gaussian(&wo, 1.5));
    let (mut s_in, mut n_in, mut s_ring, mut n_ring) = (0f64, 0f64, 0f64, 0f64);
    for y in 0..we.height {
        for x in 0..we.width {
            let d = dist(x, y);
            if d < 0.9 * r {
                let l = ln_ratio(be.get(x, y), bo.get(x, y));
                tone += (0..3).map(|c| f64::from(l[c].abs())).sum::<f64>() / 3.0;
                tn += 1.0;
            }
            // fine detail of the green channel
            let hp = f64::from(we.get(x, y)[1] - fe.get(x, y)[1]);
            if d < 0.9 * r {
                s_in += hp * hp;
                n_in += 1.0;
            } else if d > r + 4.0 && d < 2.0 * r {
                let ho = f64::from(wo.get(x, y)[1] - fo.get(x, y)[1]);
                s_ring += ho * ho;
                n_ring += 1.0;
            }
        }
    }
    let std_in = (s_in / n_in.max(1.0)).sqrt().max(1e-6);
    let std_ring = (s_ring / n_ring.max(1.0)).sqrt().max(1e-6);
    Score { seam, tone: tone / tn.max(1.0), grain: (std_in / std_ring).ln().abs(), secs: 0.0 }
}

/// The square of side `side` around (`cx`, `cy`), shown at `TILE` pixels, display encoded.
fn crop(img: &Rgb32f, cx: f64, cy: f64, side: f64) -> Rgb32f {
    let (w, _, _) = window(img, cx, cy, side);
    let t = resize(&w, tile(), tile(), Filter::Bilinear);
    t.map(|p| p.map(|v| v.clamp(0.0, 1.0).powf(1.0 / 2.2)))
}

fn write_sheet(path: &str, rows: &[Vec<Rgb32f>]) -> Result<(), String> {
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    if cols == 0 {
        return Ok(());
    }
    let t = tile();
    let (w, h) = (cols * t, rows.len() * t);
    let mut bytes = vec![0u8; w * h * 3];
    for (ry, row) in rows.iter().enumerate() {
        for (cxi, tile) in row.iter().enumerate() {
            for y in 0..t {
                for x in 0..t {
                    let p = tile.get(x, y);
                    let o = ((ry * t + y) * w + cxi * t + x) * 3;
                    for c in 0..3 {
                        if let Some(b) = bytes.get_mut(o + c) {
                            *b = (p[c].clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                        }
                    }
                }
            }
        }
    }
    let png = encode_png(&EncodeImage::new(w as u32, h as u32, 3, Samples::U8(&bytes)), &EncodeMeta::default()).map_err(|e| e.to_string())?;
    std::fs::write(path, png).map_err(|e| format!("{path}: {e}"))?;
    eprintln!("wrote {path} (columns: photo | modes in the order above)");
    Ok(())
}
