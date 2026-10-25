//! `cargo run -p lightcraft-engine --example dng_sharp`
//!
//! How much of a fine grating survives each way a DNG can hold the same picture (Bayer mosaic, linear RGB, lossy
//! JPEG tiles), measured on the loaded source and on the default render.
use lightcraft_develop::DevelopSettings;
use lightcraft_pipeline::{RenderRequest, render};
use lightcraft_raw::*;

const PERIODS: [usize; 6] = [3, 4, 6, 8, 12, 16];
const BAND: usize = 96;
const H: usize = 96;

fn scene(x: usize, band: usize) -> f64 {
    0.25 + 0.15 * (std::f64::consts::TAU * x as f64 / PERIODS[band] as f64).sin()
}

fn base(w: usize, h: usize, cpp: usize, data: Vec<u16>, cfa: Option<Cfa>) -> RawImage {
    RawImage {
        format: RawFormat::Dng,
        width: w,
        height: h,
        cpp,
        data: RawData::U16(data),
        cfa,
        bits: 16,
        black: BlackLevel::uniform(0.0),
        white: vec![65535.0],
        active_area: Rect::new(0, 0, w, h),
        crop: Rect::new(0, 0, w, h),
        orientation: Orientation::Normal,
        color: ColorData {
            illuminant: [21, 0],
            color_matrix: [Some(Mat3([[0.9, 0.2, -0.15], [-0.3, 1.25, 0.08], [0.02, -0.12, 0.85]])), None],
            as_shot_neutral: Some([1.0, 1.0, 1.0]),
            ..Default::default()
        },
        wb_multipliers: None,
        linearized: false,
        pixel_aspect: 1.0,
        opcodes: OpcodeLists::default(),
        metadata: Default::default(),
        lens_metadata: Default::default(),
    }
}

fn level(x: usize) -> u16 {
    let band = (x / BAND).min(PERIODS.len() - 1);
    (scene(x, band) * 65535.0).round() as u16
}

/// Amplitude of the grating of band `band` in `row(x)` (centre two thirds of the band), as a share of the scene's 0.15.
fn modulation(band: usize, row: impl Fn(usize) -> f64) -> f64 {
    let (x0, x1) = (band * BAND + BAND / 6, band * BAND + BAND * 5 / 6);
    let p = PERIODS[band] as f64;
    let n = (x1 - x0) as f64;
    let (mut s, mut c, mut m) = (0.0, 0.0, 0.0);
    for x in x0..x1 {
        let a = std::f64::consts::TAU * x as f64 / p;
        m += row(x);
        s += row(x) * a.sin();
        c += row(x) * a.cos();
    }
    let _ = m;
    2.0 * (s * s + c * c).sqrt() / n
}

fn report(name: &str, w: usize, row: impl Fn(usize) -> f64) {
    let v: Vec<String> = (0..PERIODS.len()).map(|b| format!("{:.3}", modulation(b, &row))).collect();
    println!("{name:<28} {}  (w {w})", v.join("  "));
}

fn main() {
    let w = BAND * PERIODS.len();
    let mosaic: Vec<u16> = (0..w * H).map(|i| level(i % w)).collect();
    let linear: Vec<u16> = (0..w * H).flat_map(|i| [level(i % w); 3]).collect();
    println!("{:<28} {}", "periods (px)", PERIODS.map(|p| format!("{p:>5}")).join("  "));
    let cases: Vec<(&str, RawImage, DngCompression)> = vec![
        ("cfa uncompressed", base(w, H, 1, mosaic.clone(), Cfa::bayer("RGGB")), DngCompression::Uncompressed),
        ("linear uncompressed", base(w, H, 3, linear.clone(), None), DngCompression::Uncompressed),
        ("linear lj92", base(w, H, 3, linear, None), DngCompression::Lj92 { tile: 64 }),
        ("cfa -> lossy jpeg q95", base(w, H, 1, mosaic, Cfa::bayer("RGGB")), DngCompression::LossyJpeg { tile: 96, quality: 95, long_edge: None }),
    ];
    println!("-- loaded source (linear Rec.2020, green) --");
    let mut loaded = Vec::new();
    for (name, raw, comp) in &cases {
        let bytes = write_dng(raw, &DngWriteOptions { compression: *comp, ..Default::default() }).unwrap();
        let (img, info) = lightcraft_engine::files::load_bytes(&bytes, 100_000).unwrap();
        // 1.0 of the scene's own scale: the linear level, so compare with the first row's ratio only
        report(name, img.width, |x| img.get(x, H / 2)[1] as f64);
        loaded.push((name, img, info));
    }
    println!("-- default render (sRGB 8-bit green; ratios only comparable between rows) --");
    for (name, img, info) in &loaded {
        let s = DevelopSettings::for_raw(6500.0, 0.0);
        let out = render(img, info, &s, &RenderRequest::fit(img.width, img.height)).image;
        report(name, out.width, |x| out.data[(out.height / 2) * out.width + x][1] as f64 / 255.0);
    }
}
