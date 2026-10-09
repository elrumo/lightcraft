//! The real PP-OCRv6 models on text rendered here. Skipped (passes) unless the model folder is
//! given: `LIGHTCRAFT_OCR_DIR=/path/with/det-and-rec cargo test -p lightcraft-vision --features onnx --release --test ocr_reference -- --nocapture`.
//! Chinese and Japanese text needs a font with those glyphs: `LIGHTCRAFT_TEST_CJK_FONT=/path/to/font.ttc`
//! (a system font is fine: it only draws test images and is never kept).

use std::time::Instant;

use ab_glyph::{Font, FontRef, PxScale, ScaleFont, point};
use lightcraft_raster::{Image, Rgba8};
use lightcraft_vision::ocr::Ocr;

fn ocr() -> Option<Ocr> {
    let dir = std::env::var_os("LIGHTCRAFT_OCR_DIR")?;
    let t = Instant::now();
    let m = Ocr::load(std::path::Path::new(&dir)).expect("the model folder loads");
    println!("loaded in {:?}, {} characters", t.elapsed(), m.dictionary_len());
    Some(m)
}

/// Black text on white: `(text, pixel size, x, baseline y)`.
fn render(font: &FontRef<'_>, w: usize, h: usize, lines: &[(&str, f32, f32, f32)]) -> Rgba8 {
    let mut data = vec![[255u8, 255, 255, 255]; w * h];
    for &(text, px, x0, y) in lines {
        let scale = PxScale::from(px);
        let scaled = font.as_scaled(scale);
        let mut x = x0;
        for c in text.chars() {
            let id = font.glyph_id(c);
            let glyph = id.with_scale_and_position(scale, point(x, y));
            x += scaled.h_advance(id);
            if let Some(o) = font.outline_glyph(glyph) {
                let b = o.px_bounds();
                o.draw(|gx, gy, cov| {
                    let (px, py) = (b.min.x as i64 + i64::from(gx), b.min.y as i64 + i64::from(gy));
                    if px >= 0 && py >= 0 && (px as usize) < w && (py as usize) < h {
                        let p = &mut data[py as usize * w + px as usize];
                        let v = (255.0 * (1.0 - cov)).min(f32::from(p[0])) as u8;
                        *p = [v, v, v, 255];
                    }
                });
            }
        }
    }
    Image { width: w, height: h, data }
}

fn inter() -> FontRef<'static> {
    let bytes: &'static [u8] = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
    FontRef::try_from_slice(bytes).unwrap()
}

fn text_of(lines: &[lightcraft_vision::ocr::Line]) -> String {
    lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n")
}

#[test]
fn latin_text_is_read_with_its_numbers() {
    let Some(ocr) = ocr() else {
        eprintln!("LIGHTCRAFT_OCR_DIR not set: skipping");
        return;
    };
    let img = render(
        &inter(),
        1280,
        720,
        &[
            ("LightCraft Search 2026", 72.0, 60.0, 130.0),
            ("Invoice No. 48213   Total: $1,234.50", 52.0, 60.0, 300.0),
            ("Hello World, open 9am to 5pm", 44.0, 60.0, 460.0),
        ],
    );
    // (the first read prepares the weights: time several)
    for i in 0..3 {
        let t = Instant::now();
        let _ = ocr.read(&img).unwrap();
        println!("read #{i} took {:?}", t.elapsed());
    }
    let t = Instant::now();
    let lines = ocr.read(&img).unwrap();
    println!("read in {:?}:\n{}", t.elapsed(), lines.iter().map(|l| format!("  {:.2}  {}", l.score, l.text)).collect::<Vec<_>>().join("\n"));
    let all = text_of(&lines).to_lowercase();
    for want in ["lightcraft", "search", "2026", "invoice", "48213", "1,234.50", "hello world", "9am"] {
        assert!(all.contains(want), "missing {want:?} in {all:?}");
    }
    // top to bottom
    assert!(lines.len() >= 3 && lines.first().unwrap().text.to_lowercase().contains("lightcraft"), "{lines:?}");
    // the box of a line surrounds where it was drawn
    let q = lines[0].quad;
    assert!(q[0][0] < 80.0 && q[1][0] > 600.0 && q[0][1] < 100.0 && q[3][1] > 100.0, "{q:?}");
}

#[test]
fn a_photo_without_text_has_none() {
    let Some(ocr) = ocr() else { return };
    let blank = Image { width: 800, height: 600, data: vec![[200u8, 180, 90, 255]; 800 * 600] };
    assert!(ocr.read(&blank).unwrap().is_empty());
    // noise and a gradient don't make up words
    let mut s = 12345u32;
    let noise = Image {
        width: 640,
        height: 480,
        data: (0..640 * 480)
            .map(|_| {
                s = s.wrapping_mul(1664525).wrapping_add(1013904223);
                let v = (s >> 24) as u8;
                [v, v, v, 255]
            })
            .collect(),
    };
    let found = ocr.read(&noise).unwrap();
    assert!(found.iter().all(|l| l.text.chars().count() <= 6), "{found:?}");
    // hostile sizes are refused, not run
    assert!(ocr.read(&Image { width: 0, height: 0, data: vec![] }).is_err());
    assert!(ocr.read(&Image { width: 10, height: 10, data: vec![[0, 0, 0, 255]; 99] }).is_err());
}

#[test]
fn tiny_images_and_small_text_do_not_panic() {
    let Some(ocr) = ocr() else { return };
    for (w, h) in [(1, 1), (7, 3), (31, 31), (64, 8), (3000, 40)] {
        let img = Image { width: w, height: h, data: vec![[255u8, 255, 255, 255]; w * h] };
        let _ = ocr.read(&img).unwrap();
    }
}

#[test]
fn chinese_and_japanese_text_is_read() {
    let Some(ocr) = ocr() else { return };
    let Some(path) = std::env::var_os("LIGHTCRAFT_TEST_CJK_FONT") else {
        eprintln!("LIGHTCRAFT_TEST_CJK_FONT not set: skipping");
        return;
    };
    let bytes = std::fs::read(path).unwrap();
    let font = (0..8)
        .find_map(|i| FontRef::try_from_slice_and_index(&bytes, i).ok().filter(|f| f.glyph_id('搜').0 != 0 && f.glyph_id('あ').0 != 0).map(|_| i));
    let Some(index) = font else {
        eprintln!("that font has no Chinese and kana glyphs: skipping");
        return;
    };
    let font = FontRef::try_from_slice_and_index(&bytes, index).unwrap();
    let img = render(
        &font,
        1280,
        720,
        &[("搜索照片和文字", 72.0, 60.0, 140.0), ("東京タワーの写真を検索", 60.0, 60.0, 320.0), ("欢迎来到上海 2026", 56.0, 60.0, 500.0)],
    );
    let lines = ocr.read(&img).unwrap();
    println!("{}", lines.iter().map(|l| format!("  {:.2}  {}", l.score, l.text)).collect::<Vec<_>>().join("\n"));
    let all = text_of(&lines);
    for want in ["搜索", "照片", "東京", "タワー", "上海", "2026"] {
        assert!(all.contains(want), "missing {want:?} in {all:?}");
    }
}
