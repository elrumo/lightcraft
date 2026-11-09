use super::*;
use crate::export::draw_watermark;
use serde_json::json;

fn decode(jpg: &[u8]) -> Rgba8 {
    lightcraft_codecs::decode(jpg, Default::default()).unwrap().to_srgb8()
}

fn first_page(bytes: &[u8]) -> Rgba8 {
    let start = bytes.windows(2).position(|v| v == [0xff, 0xd8]).unwrap();
    let end = bytes[start..].windows(2).position(|v| v == [0xff, 0xd9]).unwrap() + start + 2;
    decode(&bytes[start..end])
}

fn sheet(s: &mut Session, p: Value) -> Document {
    prepare(s, &p).unwrap().run(&mut |_, _| true).unwrap()
}

#[test]
fn validates_settings_and_selection() {
    let mut s = Session::with_demo();
    for p in [
        json!({"columns": 0}),
        json!({"rows": 11}),
        json!({"paper": "poster"}),
        json!({"landscape": "yes"}),
        json!({"typo": 2}),
        json!({"style": "poster"}),
        json!({"format": "png"}),
        json!({"preview": "yes"}),
        json!({"ids": ["bad"]}),
        json!({"ids": []}),
    ] {
        assert!(prepare(&mut s, &p).is_err(), "{p}");
    }
    let a = Options::default().layout().unwrap();
    let b = Options { landscape: true, ..Default::default() }.layout().unwrap();
    assert_eq!((a.width, a.height), (b.height, b.width));
    let id = s.catalog.photos().next().unwrap().id.0;
    assert!(prepare(&mut s, &json!({"ids": [id, u64::MAX]})).is_err());
    assert!(prepare(&mut s, &json!({"ids": vec![id; 1001]})).is_err());
    assert!(prepare(&mut s, &json!({"ids": vec![id; 101], "columns": 1, "rows": 1})).is_err());
}

#[test]
fn transparent_pixels_composite_over_white() {
    let mut dst = Rgba8::filled(3, 1, [255; 4]);
    let src = Rgba8 { width: 3, height: 1, data: vec![[0, 0, 0, 0], [255, 0, 0, 128], [10, 20, 30, 255]] };
    blit(&mut dst, &src, 0, 0).unwrap();
    assert_eq!(dst.data, vec![[255; 4], [255, 127, 127, 255], [10, 20, 30, 255]]);
    assert!(blit(&mut dst, &src, usize::MAX, 0).is_err());
}

#[test]
fn fills_clip_to_the_page_and_round_their_corners() {
    let mut page = Rgba8::filled(10, 10, [255; 4]);
    fill(&mut page, Rect { x: -5.0, y: -5.0, w: 1e9, h: 7.0 }, [0; 3], 255, 0.0);
    assert_eq!(page.get(9, 1), [0, 0, 0, 255]);
    assert_eq!(page.get(0, 2), [255; 4]);
    let mut page = Rgba8::filled(10, 10, [255; 4]);
    fill(&mut page, Rect { x: 0.0, y: 0.0, w: 10.0, h: 10.0 }, [0; 3], 255, 4.0);
    assert_eq!(page.get(0, 0), [255; 4], "a rounded corner leaves the paper");
    assert_eq!(page.get(5, 5), [0, 0, 0, 255]);
    fill(&mut page, Rect { x: f64::NAN, y: 0.0, w: f64::INFINITY, h: 3.0 }, [9; 3], 128, 0.0);
}

#[test]
fn unicode_captions_are_rasterized() {
    assert_eq!(caption_text("very-long-éééé-filename.jpg", 140, 14.0), "very-….jpg");
    assert_eq!(caption_text("café\n.jpg", 300, 14.0), "café .jpg");
    let wm = |text: &str| Watermark {
        text: text.into(),
        size: 0.5,
        opacity: 1.0,
        anchor: Anchor::Center,
        inset_x: 0.0,
        inset_y: 0.0,
        color: [32; 3],
        shadow: false,
        ..Default::default()
    };
    let mut img = Rgba8::filled(360, 28, [255; 4]);
    draw_watermark(&mut img, &wm("Été – Straße café.jpg"));
    assert!(img.data.iter().filter(|p| p[0] < 128).count() > 100);
    let mut ascii = Rgba8::filled(360, 28, [255; 4]);
    draw_watermark(&mut ascii, &wm("Ete - Strasse cafe.jpg"));
    assert_ne!(img, ascii);
}

#[test]
fn pagination_xref_and_cancellation() {
    let mut s = Session::with_demo();
    let ids: Vec<_> = s.catalog.photos().take(3).map(|p| p.id.0).collect();
    let p = json!({"ids": ids, "columns": 1, "rows": 2, "captions": false});
    let mut seen = Vec::new();
    let doc = prepare(&mut s, &p)
        .unwrap()
        .run(&mut |i, _| {
            seen.push(i);
            true
        })
        .unwrap();
    assert_eq!((doc.pages, doc.photos, doc.files.len()), (2, 3, 1));
    assert_eq!(seen, vec![0, 1, 2, 3]);
    let bytes = &doc.files[0];
    let text = String::from_utf8_lossy(bytes);
    assert!(text.contains("/Count 2 /Kids [3 0 R 6 0 R]"));
    let xref = text.rsplit("startxref\n").next().unwrap().lines().next().unwrap().parse::<usize>().unwrap();
    assert!(bytes[xref..].starts_with(b"xref\n0 9\n"));
    let xref_text = std::str::from_utf8(&bytes[xref..]).unwrap();
    for (index, line) in xref_text.lines().skip(3).take(8).enumerate() {
        let offset = line.split_whitespace().next().unwrap().parse::<usize>().unwrap();
        assert!(bytes[offset..].starts_with(format!("{} 0 obj\n", index + 1).as_bytes()));
    }
    assert!(prepare(&mut s, &p).unwrap().run(&mut |_, _| false).is_err());
}

#[test]
fn sheet_uses_current_edits_and_captions() {
    let mut s = Session::with_demo();
    let p = json!({"columns": 1, "rows": 2, "captions": false});
    let plain = first_page(&sheet(&mut s, p.clone()).files[0]);
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 2.0})).unwrap();
    let bright = first_page(&sheet(&mut s, p).files[0]);
    let sum = |img: &Rgba8| img.data.iter().map(|p| u64::from(p[0]) + u64::from(p[1]) + u64::from(p[2])).sum::<u64>();
    assert!(sum(&bright) > sum(&plain), "sheet ignored develop settings");
    assert_eq!((plain.width, plain.height), (1240, 1754));
}

/// A caption is drawn under its photo when asked for; a film frame always shows its number.
#[test]
fn captions_are_drawn_when_asked_for() {
    for style in ["classic", "minimal", "instant", "film"] {
        let ink = |captions: bool| {
            let o = Options { style: style.into(), columns: 1, rows: 2, captions, ..Default::default() };
            let l = o.layout().unwrap();
            let (w, h) = l.render_size(captions);
            let mut page = Rgba8::filled(l.width, l.height, [255; 4]);
            l.background(&mut page, 0, 1, 1, &o);
            let before = page.clone();
            l.photo(&mut page, 0, 6, &Rgba8::filled(w, h, [40, 200, 90, 255]), "IMG_0007.jpg", captions).unwrap();
            let caption = look(style).unwrap().caption;
            let near = |p: [u8; 4]| (0..3).all(|c| p[c].abs_diff(caption[c]) < 40);
            page.data.iter().zip(&before.data).filter(|(p, b)| near(**p) && !near(**b)).count()
        };
        let (on, off) = (ink(true), ink(false));
        assert!(on > 80, "{style}: {on} caption pixels");
        if style == "film" {
            assert!(off > 3 && off < on, "{style}: the frame number alone ({off}) and with the name ({on})");
        } else {
            assert_eq!(off, 0, "{style}: no caption asked for");
        }
    }
}

/// Every style draws its own page: its paper, and (film) sprocket holes along each strip.
#[test]
fn every_style_draws_its_own_page() {
    let mut s = Session::with_demo();
    let mut pages = Vec::new();
    for (style, _) in STYLES {
        let doc = sheet(&mut s, json!({"style": style, "preview": true, "columns": 3, "rows": 3}));
        assert_eq!(doc.files.len(), 1, "{style}");
        let page = decode(&doc.files[0]);
        assert_eq!((page.width, page.height), (827, 1169), "{style}: A4 at a preview's 100 dpi");
        let paper = look(style).unwrap().paper;
        let corner = page.get(3, 3);
        assert!((0..3).all(|c| corner[c].abs_diff(paper[c]) <= 6), "{style}: corner {corner:?}, paper {paper:?}");
        pages.push((*style, page));
    }
    for (i, (a, pa)) in pages.iter().enumerate() {
        for (b, pb) in &pages[i + 1..] {
            assert_ne!(pa, pb, "{a} and {b} look the same");
        }
    }
    let film = &pages.iter().find(|(s, _)| *s == "film").unwrap().1;
    let l = Options { style: "film".into(), preview: true, columns: 3, rows: 3, ..Default::default() }.layout().unwrap();
    let strip = l.strip(0);
    let y = (strip.y + l.sprockets(strip) / 2.0) as usize;
    let light: Vec<bool> = (0..film.width).map(|x| film.get(x, y)[0] > 150).collect();
    let holes = light.windows(2).filter(|w| !w[0] && w[1]).count();
    let expected = (strip.w / (l.sprockets(strip) * 0.95)) as usize;
    assert!(expected > 10 && holes >= expected - 1 && holes <= expected, "{holes} sprocket holes along the first strip, {expected} drawn");
    let dark = |img: &Rgba8| img.data.iter().map(|p| u64::from(p[0])).sum::<u64>() / img.data.len() as u64;
    let light_page = &pages.iter().find(|(s, _)| *s == "light").unwrap().1;
    let dark_page = &pages.iter().find(|(s, _)| *s == "dark").unwrap().1;
    assert!(dark(dark_page) + 100 < dark(light_page), "the dark page is not darker than the light one");
}

/// A preview is the file's first page at a lower resolution, not a different layout.
#[test]
fn a_preview_is_the_first_page_of_the_file() {
    let mut s = Session::with_demo();
    let ids: Vec<_> = s.catalog.photos().take(5).map(|p| p.id.0).collect();
    for style in ["gallery", "film"] {
        let p = json!({"ids": ids, "style": style, "columns": 2, "rows": 2, "format": "jpg"});
        let file = sheet(&mut s, p.clone());
        let mut seen = Vec::new();
        let mut q = p.clone();
        q["preview"] = json!(true);
        let preview = prepare(&mut s, &q)
            .unwrap()
            .run(&mut |i, _| {
                seen.push(i);
                true
            })
            .unwrap();
        assert_eq!(seen, vec![0, 1, 2, 3, 4], "{style}: a preview renders its page's photos alone");
        assert_eq!((preview.pages, preview.photos, preview.files.len()), (2, 5, 1), "{style}: the sheet's pages and photos");
        let (big, small) = (decode(&file.files[0]), decode(&preview.files[0]));
        // the two pages averaged over the same coarse grid
        let blocks = |img: &Rgba8| {
            let (bw, bh) = (24, 34);
            let mut sum = vec![[0u64; 4]; bw * bh];
            for y in 0..img.height {
                for x in 0..img.width {
                    let b = &mut sum[y * bh / img.height * bw + x * bw / img.width];
                    let p = img.get(x, y);
                    (0..3).for_each(|c| b[c] += u64::from(p[c]));
                    b[3] += 1;
                }
            }
            sum.into_iter().flat_map(|b| (0..3).map(move |c| b[c] as f64 / b[3].max(1) as f64)).collect::<Vec<_>>()
        };
        let (a, b) = (blocks(&big), blocks(&small));
        let diff = a.iter().zip(&b).map(|(a, b)| (a - b).abs()).sum::<f64>() / a.len() as f64;
        assert!(diff < 6.0, "{style}: preview and file differ by {diff:.1} levels on average");
    }
}

#[test]
fn jpeg_sheets_write_one_file_per_page() {
    let mut s = Session::with_demo();
    let dir = std::env::temp_dir().join(format!("lc-contact-jpeg-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let ids: Vec<_> = s.catalog.photos().take(3).map(|p| p.id.0).collect();
    let path = dir.join("Sheet.jpg").to_string_lossy().into_owned();
    let p = json!({"path": path, "ids": ids, "columns": 1, "rows": 2, "format": "jpg", "style": "dark"});
    let r = s.execute("export.contactSheet", &p).unwrap();
    let second = dir.join("Sheet 2.jpg").to_string_lossy().into_owned();
    assert_eq!(r["paths"], json!([path, second]));
    assert_eq!((r["path"].as_str(), r["pages"].as_u64(), r["photos"].as_u64()), (Some(path.as_str()), Some(2), Some(3)));
    let mut total = 0;
    for f in [&path, &second] {
        let bytes = std::fs::read(f).unwrap();
        assert!(bytes.starts_with(&[0xff, 0xd8]), "{f} is not a JPEG");
        let page = decode(&bytes);
        assert_eq!((page.width, page.height), (1240, 1754), "{f}: A4 at 150 dpi");
        total += bytes.len();
    }
    assert_eq!(r["bytes"].as_u64(), Some(total as u64));
    // a PDF is one file whatever its pages
    let pdf = dir.join("Sheet.pdf").to_string_lossy().into_owned();
    let r = s.execute("export.contactSheet", &json!({"path": pdf, "ids": ids, "columns": 1, "rows": 2})).unwrap();
    assert_eq!(r["paths"], json!([pdf]));
    assert!(std::fs::read(&pdf).unwrap().starts_with(b"%PDF"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The social media sizes are exact in pixels: 1080 × 1080, 1080 × 1350, 1080 × 1920 and 1920 × 1080.
#[test]
fn social_sizes_are_exact_in_pixels() {
    let mut s = Session::with_demo();
    let id = s.catalog.photos().next().unwrap().id.0;
    for (paper, size) in [("square", (1080, 1080)), ("portrait", (1080, 1350)), ("story", (1080, 1920)), ("wide", (1920, 1080))] {
        let doc = sheet(&mut s, json!({"ids": [id], "paper": paper, "format": "jpg", "style": "dark", "columns": 2, "rows": 3}));
        let page = decode(&doc.files[0]);
        assert_eq!((page.width, page.height), size, "{paper}");
        let preview = decode(&sheet(&mut s, json!({"ids": [id], "paper": paper, "preview": true})).files[0]);
        assert_eq!((preview.width * 3, preview.height * 3), (size.0 * 2, size.1 * 2), "{paper}: the preview at 100 dpi");
    }
}

/// Vertical film strips run down the columns, the photos down each strip, with sprocket holes along both edges.
#[test]
fn vertical_film_strips_run_top_to_bottom() {
    let o = Options { style: "film".into(), vertical: true, columns: 3, rows: 4, ..Default::default() };
    let l = o.layout().unwrap();
    let (first, second) = (l.cell(0), l.cell(1));
    assert_eq!(first.x, second.x, "the second photo is under the first");
    assert!(second.y > first.y);
    assert!(l.cell(4).x > first.x, "the fifth starts the next strip");
    let across = Options { vertical: false, ..o.clone() }.layout().unwrap();
    assert!(across.cell(1).x > across.cell(0).x, "left to right without it");
    let strip = l.strip(0);
    assert!(strip.h > strip.w, "a strip is a column");
    let mut page = Rgba8::filled(l.width, l.height, [255; 4]);
    l.background(&mut page, 0, 1, 12, &o);
    let x = (strip.x + l.sprockets(strip) / 2.0) as usize;
    let light: Vec<bool> = (0..page.height).map(|y| page.get(x, y)[0] > 150).collect();
    let holes = light.windows(2).filter(|w| !w[0] && w[1]).count();
    assert!(holes > 30, "{holes} holes down the strip's edge");
    // (the photo boxes stay inside their strip, between the sprocket bands)
    let (b, ..) = l.photo_box(5, true);
    let s = l.strip_of(5);
    assert!(b.x >= s.x + l.sprockets(s) && b.right() <= s.right() - l.sprockets(s), "{b:?} in {s:?}");
}

/// A logo (a graphic and its text) goes at the right of the bottom margin when one is given.
#[test]
fn the_logo_is_drawn_in_the_bottom_corner_when_given() {
    let dir = std::env::temp_dir().join(format!("lc-contact-logo-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("mark.png");
    let mark = Rgba8::filled(64, 64, [230, 120, 20, 255]);
    std::fs::write(&png, encode_image(&mark, &ExportOptions { format: ExportFormat::Png, ..Default::default() }).unwrap()).unwrap();
    let corner = |logo: &str, text: &str| {
        let o = Options { logo: logo.into(), logo_text: text.into(), ..Default::default() };
        let l = o.layout().unwrap();
        let mut page = Rgba8::filled(l.width, l.height, [255; 4]);
        l.background(&mut page, 0, 1, 1, &o);
        let margin = (l.look.margin * l.scale) as usize;
        let rows = l.height - margin..l.height;
        let orange =
            rows.clone().flat_map(|y| (l.width / 2..l.width).map(move |x| (x, y))).filter(|&(x, y)| page.get(x, y) == [230, 120, 20, 255]).count();
        let ink = rows.flat_map(|y| (l.width / 2..l.width).map(move |x| (x, y))).filter(|&(x, y)| page.get(x, y)[1] < 100).count();
        (orange, ink)
    };
    let (orange, ink) = corner(&png.to_string_lossy(), "Studio");
    assert!(orange > 100, "{orange} pixels of the graphic");
    assert!(ink > 40, "{ink} pixels of its text beside it");
    assert_eq!(corner("", ""), (0, 0), "no logo asked for");
    let _ = std::fs::remove_dir_all(&dir);
}
