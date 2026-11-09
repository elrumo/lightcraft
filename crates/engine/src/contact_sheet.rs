//! Paginated contact sheets in one of several looks ([`STYLES`]), rendered with the normal export
//! pipeline and packaged as a PDF ([`crate::pdf`]) or one JPEG per page. A preview draws the first
//! page alone at screen resolution; sizes are in points, so it is laid out exactly as the file.
use crate::export::{Anchor, ExportFormat, ExportOptions, Watermark, draw_watermark_in, encode_image};
use crate::{Session, media::RenderJob};
use lightcraft_pipeline::{OutputDepth, OutputSpace};
use lightcraft_raster::Rgba8;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The looks, `(id, English name)`: the `style` option.
pub const STYLES: &[(&str, &str)] = &[
    ("classic", "Classic"),
    ("minimal", "Minimal"),
    ("light", "Light"),
    ("dark", "Dark"),
    ("film", "35mm Film"),
    ("instant", "Instant"),
    ("gallery", "Gallery"),
];

/// The page sizes, `(id, English name, width and height in points)`: the `paper` option. Paper, then the sizes of social
/// media posts, exact in pixels at the file's 150 dpi (a 1080 px edge is 518.4 points).
pub const PAPERS: &[(&str, &str, f64, f64)] = &[
    ("a4", "A4", 595.28, 841.89),
    ("letter", "Letter", 612.0, 792.0),
    ("square", "Square 1:1", 518.4, 518.4),
    ("portrait", "Portrait 4:5", 518.4, 648.0),
    ("story", "Story 9:16", 518.4, 921.6),
    ("wide", "Wide 16:9", 921.6, 518.4),
];

/// The file's resolution, and a preview's.
const FILE_DPI: f64 = 150.0;
const PREVIEW_DPI: f64 = 100.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Options {
    pub paper: String,
    pub landscape: bool,
    pub columns: usize,
    pub rows: usize,
    pub captions: bool,
    /// One of [`STYLES`].
    pub style: String,
    /// `pdf`, or `jpg`: one JPEG per page.
    pub format: String,
    /// The first page alone, at screen resolution, as a JPEG (`format` aside).
    pub preview: bool,
    /// Film strips run top to bottom, the photos down each strip (film style; else left to right, along the rows).
    pub vertical: bool,
    /// A graphic (PNG with transparency, JPEG, …) drawn small in each page's bottom corner, a frontend's mark
    /// (empty: none), and `logo_text` beside it in the style's caption colour.
    pub logo: String,
    pub logo_text: String,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            paper: "a4".into(),
            landscape: false,
            columns: 3,
            rows: 4,
            captions: true,
            style: "classic".into(),
            format: "pdf".into(),
            preview: false,
            vertical: false,
            logo: String::new(),
            logo_text: String::new(),
        }
    }
}

impl Options {
    pub fn parse(p: &Value) -> Result<Self, String> {
        let mut settings = p.as_object().cloned().ok_or("params must be an object")?;
        settings.remove("path");
        settings.remove("ids");
        settings.remove("id");
        let o: Self = serde_json::from_value(Value::Object(settings)).map_err(|e| e.to_string())?;
        o.layout()?;
        Ok(o)
    }

    fn per_page(&self) -> usize {
        self.columns * self.rows
    }

    fn jpeg(&self) -> bool {
        self.preview || self.format == "jpg"
    }

    fn layout(&self) -> Result<Layout, String> {
        if !(1..=8).contains(&self.columns) || !(1..=10).contains(&self.rows) {
            return Err("columns must be 1–8 and rows 1–10".into());
        }
        if !matches!(self.format.as_str(), "pdf" | "jpg") {
            return Err("format must be pdf or jpg".into());
        }
        let look =
            look(&self.style).ok_or_else(|| format!("style must be one of {}", STYLES.iter().map(|(id, _)| *id).collect::<Vec<_>>().join(", ")))?;
        let Some(&(_, _, mut w, mut h)) = PAPERS.iter().find(|(id, ..)| *id == self.paper) else {
            return Err(format!("paper must be one of {}", PAPERS.iter().map(|(id, ..)| *id).collect::<Vec<_>>().join(", ")));
        };
        if self.landscape {
            std::mem::swap(&mut w, &mut h);
        }
        // Fixed resolutions, bounded paper sizes and grid dimensions keep allocations predictable.
        let scale = if self.preview { PREVIEW_DPI } else { FILE_DPI } / 72.0;
        let (width, height) = ((w * scale).round() as usize, (h * scale).round() as usize);
        let margin = look.margin * scale;
        let folio = if look.folio { look.caption_pt * 3.0 * scale } else { 0.0 };
        let grid = Rect { x: margin, y: margin, w: width as f64 - 2.0 * margin, h: height as f64 - 2.0 * margin - folio };
        let (cell_w, cell_h) = (grid.w / self.columns as f64, grid.h / self.rows as f64);
        let vertical = self.vertical && look.film;
        Ok(Layout { points: (w, h), width, height, scale, look, grid, cell_w, cell_h, columns: self.columns, rows: self.rows, vertical })
    }
}

/// How a style draws a page. Lengths in points; a mat's widths are fractions of its cell's short edge.
struct Look {
    paper: [u8; 3],
    /// Around the grid, and around each photo inside its cell.
    margin: f64,
    gap: f64,
    caption_pt: f64,
    caption: [u8; 3],
    caption_left: bool,
    mat: Option<Mat>,
    /// Rows drawn as 35 mm film strips: sprocket holes, numbered frames.
    film: bool,
    /// The page number under the grid ("2 / 5").
    folio: bool,
}

/// A frame around each photo.
struct Mat {
    color: [u8; 3],
    side: f64,
    bottom: f64,
    /// A thin line around the mat (a picture frame).
    outline: Option<[u8; 3]>,
    shadow: bool,
    /// The file name is written on the mat's bottom edge (an instant print), not under it.
    caption_inside: bool,
}

fn look(style: &str) -> Option<Look> {
    let base = Look {
        paper: [255; 3],
        margin: 28.8,
        gap: 3.84,
        caption_pt: 6.7,
        caption: [32; 3],
        caption_left: false,
        mat: None,
        film: false,
        folio: false,
    };
    let mat = Mat { color: [255; 3], side: 0.0, bottom: 0.0, outline: None, shadow: false, caption_inside: false };
    Some(match style {
        "classic" => base,
        "minimal" => Look { margin: 54.0, gap: 7.0, caption_pt: 5.6, caption: [140; 3], caption_left: true, folio: true, ..base },
        "light" => Look {
            paper: [246, 243, 236],
            margin: 36.0,
            gap: 6.0,
            caption: [118, 112, 102],
            caption_left: true,
            mat: Some(Mat { side: 0.025, bottom: 0.025, ..mat }),
            ..base
        },
        "dark" => Look { paper: [20, 20, 22], margin: 36.0, gap: 5.0, caption: [168, 168, 172], folio: true, ..base },
        "film" => Look {
            paper: [12, 12, 12],
            margin: 30.0,
            gap: 4.0,
            caption_pt: 6.2,
            caption: [236, 160, 52],
            caption_left: true,
            film: true,
            folio: true,
            ..base
        },
        "instant" => Look {
            paper: [232, 229, 222],
            margin: 36.0,
            gap: 9.0,
            caption: [70, 68, 64],
            mat: Some(Mat { color: [253, 252, 249], side: 0.05, bottom: 0.17, shadow: true, caption_inside: true, ..mat }),
            ..base
        },
        "gallery" => Look {
            paper: [238, 236, 232],
            margin: 40.0,
            gap: 8.0,
            caption: [96, 94, 90],
            folio: true,
            mat: Some(Mat { side: 0.08, bottom: 0.08, outline: Some([34; 3]), shadow: true, ..mat }),
            ..base
        },
        _ => return None,
    })
}

/// A box in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Rect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

impl Rect {
    fn inset(self, d: f64) -> Self {
        Self { x: self.x + d, y: self.y + d, w: (self.w - 2.0 * d).max(0.0), h: (self.h - 2.0 * d).max(0.0) }
    }

    fn bottom(self) -> f64 {
        self.y + self.h
    }

    fn right(self) -> f64 {
        self.x + self.w
    }
}

struct Layout {
    points: (f64, f64),
    width: usize,
    height: usize,
    /// Pixels per point.
    scale: f64,
    look: Look,
    grid: Rect,
    cell_w: f64,
    cell_h: f64,
    columns: usize,
    rows: usize,
    /// Film strips down the columns: the photos go down each one.
    vertical: bool,
}

impl Layout {
    fn cell(&self, slot: usize) -> Rect {
        let (col, row) = if self.vertical { (slot / self.rows, slot % self.rows) } else { (slot % self.columns, slot / self.columns) };
        Rect { x: self.grid.x + col as f64 * self.cell_w, y: self.grid.y + row as f64 * self.cell_h, w: self.cell_w, h: self.cell_h }
    }

    /// The height a line of captions takes.
    fn caption_band(&self) -> f64 {
        self.look.caption_pt * self.scale * 2.4
    }

    /// A row's film strip (film style), or a column's when the strips are vertical.
    fn strip(&self, index: usize) -> Rect {
        let gap = self.look.gap * self.scale;
        if self.vertical {
            let x = self.grid.x + index as f64 * self.cell_w + gap;
            return Rect { x, y: self.grid.y, w: (self.cell_w - 2.0 * gap).max(1.0), h: self.grid.h };
        }
        let y = self.grid.y + index as f64 * self.cell_h + gap;
        Rect { x: self.grid.x, y, w: self.grid.w, h: (self.cell_h - 2.0 * gap - self.caption_band()).max(1.0) }
    }

    /// The strip that holds `slot`.
    fn strip_of(&self, slot: usize) -> Rect {
        self.strip(if self.vertical { slot / self.rows } else { slot / self.columns })
    }

    /// The sprocket band along each edge of a strip.
    fn sprockets(&self, strip: Rect) -> f64 {
        (if self.vertical { strip.w } else { strip.h }) * 0.15
    }

    /// Where the photo in `slot` is fitted, and the mat's side and bottom widths around it.
    fn photo_box(&self, slot: usize, captions: bool) -> (Rect, f64, f64) {
        let cell = self.cell(slot);
        let gap = self.look.gap * self.scale;
        if self.look.film {
            let strip = self.strip_of(slot);
            let band = self.sprockets(strip);
            let frame = if self.vertical {
                // (its number and name under it, on the strip)
                Rect { x: strip.x + band + gap, y: cell.y + gap, w: strip.w - 2.0 * band - 2.0 * gap, h: cell.h - 2.0 * gap - self.caption_band() }
            } else {
                Rect { x: cell.x + gap, y: strip.y + band + gap, w: cell.w - 2.0 * gap, h: strip.h - 2.0 * band - 2.0 * gap }
            };
            return (Rect { w: frame.w.max(1.0), h: frame.h.max(1.0), ..frame }, 0.0, 0.0);
        }
        let inner = cell.inset(gap);
        let short = inner.w.min(inner.h);
        let (side, bottom) = self.look.mat.as_ref().map_or((0.0, 0.0), |m| (m.side * short, m.bottom * short));
        let inside = self.look.mat.as_ref().is_some_and(|m| m.caption_inside);
        let band = if captions && !inside { self.caption_band() } else { 0.0 };
        let b = Rect { x: inner.x + side, y: inner.y + side, w: inner.w - 2.0 * side, h: inner.h - band - side - bottom };
        (Rect { w: b.w.max(1.0), h: b.h.max(1.0), ..b }, side, bottom)
    }

    /// The size every photo is rendered to fit.
    fn render_size(&self, captions: bool) -> (usize, usize) {
        let (b, _, _) = self.photo_box(0, captions);
        ((b.w.floor() as usize).max(1), (b.h.floor() as usize).max(1))
    }

    /// The page behind the photos: paper, film strips for the rows (or columns) in use, the page number, the logo.
    fn background(&self, page: &mut Rgba8, index: usize, pages: usize, photos: usize, o: &Options) {
        fill(page, Rect { x: 0.0, y: 0.0, w: self.width as f64, h: self.height as f64 }, self.look.paper, 255, 0.0);
        if self.look.film {
            for i in 0..photos.div_ceil(if self.vertical { self.rows } else { self.columns }) {
                let strip = self.strip(i);
                fill(page, strip, [30, 27, 24], 255, 0.0);
                let band = self.sprockets(strip);
                // (measured along the strip, then across it)
                let (along, across, pitch) = (band * 0.55, band * 0.4, band * 0.95);
                let length = if self.vertical { strip.h } else { strip.w };
                let n = (length / pitch).floor().max(0.0) as usize;
                let start = (length - n as f64 * pitch) / 2.0 + (pitch - along) / 2.0;
                for edge in [(band - across) / 2.0, if self.vertical { strip.w } else { strip.h } - band + (band - across) / 2.0] {
                    for k in 0..n {
                        let at = start + k as f64 * pitch;
                        let hole = if self.vertical {
                            Rect { x: strip.x + edge, y: strip.y + at, w: across, h: along }
                        } else {
                            Rect { x: strip.x + at, y: strip.y + edge, w: along, h: across }
                        };
                        fill(page, hole, [214, 210, 200], 255, across * 0.22);
                    }
                }
            }
        }
        if self.look.folio {
            let band = Rect { x: self.grid.x, y: self.grid.bottom(), w: self.grid.w, h: self.look.caption_pt * 3.0 * self.scale };
            self.text(page, band, &format!("{} / {}", index + 1, pages), false);
        }
        self.logo(page, o.logo.trim(), o.logo_text.trim());
    }

    /// The logo at the right of the bottom margin: the graphic, then its text.
    fn logo(&self, page: &mut Rgba8, graphic: &str, text: &str) {
        if graphic.is_empty() && text.is_empty() {
            return;
        }
        let margin = self.look.margin * self.scale;
        let mark = (margin * 0.42).min(18.0 * self.scale).max(1.0);
        // (both centred in the bottom margin; the text's band leaves its letters their full size)
        let middle = self.height as f64 - margin / 2.0;
        let y = middle - mark / 2.0;
        let px = (self.look.caption_pt * 1.2 * self.scale) as f32;
        let text_w = crate::text::TextLayout::new(text, px, false, crate::fonts::CRAFT_FONTS).map_or(0.0, |l| f64::from(l.width).ceil() + 2.0);
        let mut right = self.grid.right();
        if text_w > 0.0 {
            let h = f64::from(px) / 0.45;
            let band = Rect { x: right - text_w, y: middle - h / 2.0, w: text_w, h };
            let wm = Watermark {
                text: text.into(),
                size: (f64::from(px) / band.w.min(band.h).max(1.0)) as f32,
                opacity: 1.0,
                anchor: Anchor::Right,
                inset_x: 0.0,
                inset_y: 0.0,
                color: self.look.caption,
                shadow: false,
                ..Default::default()
            };
            draw_watermark_in(
                page,
                &wm,
                band.x.round().max(0.0) as usize,
                band.y.round().max(0.0) as usize,
                band.w as usize,
                band.h.max(1.0) as usize,
            );
            right -= text_w + mark * 0.35;
        }
        if !graphic.is_empty() {
            let wm = Watermark {
                image: graphic.into(),
                image_width: 1.0,
                opacity: 1.0,
                anchor: Anchor::Center,
                inset_x: 0.0,
                inset_y: 0.0,
                ..Default::default()
            };
            draw_watermark_in(page, &wm, (right - mark).round().max(0.0) as usize, y.round().max(0.0) as usize, mark as usize, mark as usize);
        }
    }

    /// The photo in `slot` (the sheet's `number`th) with its mat and caption.
    fn photo(&self, page: &mut Rgba8, slot: usize, number: usize, image: &Rgba8, name: &str, captions: bool) -> Result<(), String> {
        let (b, side, bottom) = self.photo_box(slot, captions);
        // The renderer fits within the requested rectangle and applies crop/orientation.
        if image.width as f64 > b.w.floor().max(1.0) || image.height as f64 > b.h.floor().max(1.0) {
            return Err("render exceeded contact sheet cell".into());
        }
        let x = (b.x + (b.w - image.width as f64) / 2.0).round().max(0.0);
        let y = (b.y + (b.h - image.height as f64) / 2.0).round().max(0.0);
        let shown = Rect { x, y, w: image.width as f64, h: image.height as f64 };
        let mut outer = shown;
        if let Some(m) = &self.look.mat {
            let mat = Rect { x: x - side, y: y - side, w: shown.w + 2.0 * side, h: shown.h + side + bottom };
            let line = m.outline.map_or(0.0, |_| (1.2 * self.scale).max(1.0));
            outer = Rect { x: mat.x - line, y: mat.y - line, w: mat.w + 2.0 * line, h: mat.h + 2.0 * line };
            if m.shadow {
                let d = (0.012 * self.cell_w.min(self.cell_h)).max(1.0);
                fill(page, Rect { x: outer.x + d * 0.4, y: outer.y + d, ..outer }, [0; 3], 40, 0.0);
            }
            if let Some(c) = m.outline {
                fill(page, outer, c, 255, 0.0);
            }
            fill(page, mat, m.color, 255, 0.0);
        }
        blit(page, image, x as usize, y as usize)?;
        let gap = self.look.gap * self.scale;
        let inner = self.cell(slot).inset(gap);
        if self.look.film {
            let text = if captions { format!("{}  {name}", number + 1) } else { (number + 1).to_string() };
            let under = if self.vertical { b.bottom() } else { self.strip_of(slot).bottom() };
            let band = Rect { x: b.x, y: under, w: b.w, h: self.caption_band() };
            self.text(page, band, &text, true);
        } else if captions {
            let band = match &self.look.mat {
                Some(m) if m.caption_inside => Rect { x: outer.x, y: shown.bottom(), w: outer.w, h: bottom },
                _ if self.look.caption_left => Rect { x: outer.x, y: outer.bottom(), w: inner.right() - outer.x, h: self.caption_band() },
                _ => Rect { x: inner.x, y: outer.bottom(), w: inner.w, h: self.caption_band() },
            };
            self.text(page, band, name, self.look.caption_left);
        }
        Ok(())
    }

    /// One line of caption type in `band`, shortened to fit.
    fn text(&self, page: &mut Rgba8, band: Rect, text: &str, left: bool) {
        let px = self.look.caption_pt * self.scale;
        let (x, y) = (band.x.round().max(0.0) as usize, band.y.round().max(0.0) as usize);
        let (w, h) = (band.w.floor().max(1.0) as usize, band.h.floor().max(1.0) as usize);
        let wm = Watermark {
            text: caption_text(text, w, px),
            size: (px / w.min(h) as f64) as f32,
            opacity: 1.0,
            anchor: if left { Anchor::Left } else { Anchor::Center },
            inset_x: 0.0,
            inset_y: 0.0,
            color: self.look.caption,
            shadow: false,
            ..Default::default()
        };
        draw_watermark_in(page, &wm, x, y, w, h);
    }
}

pub struct Prepared {
    options: Options,
    items: Vec<(String, RenderJob)>,
    /// Every photo of the sheet (a preview renders only the first page's).
    photos: usize,
}

pub struct Document {
    /// The PDF, or one JPEG per page (a preview: its first page).
    pub files: Vec<Vec<u8>>,
    /// The sheet's pages and photos (a preview's whole sheet, not its one page).
    pub pages: usize,
    pub photos: usize,
}

impl Document {
    pub fn bytes(&self) -> usize {
        self.files.iter().map(Vec::len).sum()
    }
}

pub fn prepare(session: &mut Session, p: &Value) -> Result<Prepared, String> {
    let options = Options::parse(p)?;
    if let Some(ids) = p.get("ids") {
        let a = ids.as_array().ok_or("ids must be an array of photo ids")?;
        if a.iter().any(|v| v.as_u64().is_none()) {
            return Err("ids must contain only photo ids".into());
        }
    }
    if p.get("id").is_some_and(|v| v.as_u64().is_none()) {
        return Err("id must be a photo id".into());
    }
    let mut ids = session.targets(p).map_err(|e| e.to_string())?;
    if ids.is_empty() || ids.len() > 1000 {
        return Err("select between 1 and 1000 photos".into());
    }
    let l = options.layout()?;
    let photos = ids.len();
    if photos.div_ceil(options.per_page()) > 100 {
        return Err("contact sheets are limited to 100 pages; increase the grid size or select fewer photos".into());
    }
    if options.preview {
        ids.truncate(options.per_page());
    }
    let (w, h) = l.render_size(options.captions);
    let mut items = Vec::with_capacity(ids.len());
    for id in ids {
        let name = session.catalog.photo(id).ok_or("no such photo")?.file_name.clone();
        let job = session.export_job(id, w, h, OutputSpace::Srgb, OutputDepth::U8)?;
        items.push((name, job));
    }
    Ok(Prepared { options, items, photos })
}

impl Prepared {
    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The sheet's pages (a preview draws only the first).
    pub fn pages(&self) -> usize {
        self.photos.div_ceil(self.options.per_page())
    }

    /// Where [`Document::files`] go: `path` for a PDF or the first JPEG, then `<name> 2.jpg`, `<name> 3.jpg`, …
    /// beside it.
    pub fn paths(&self, path: &str) -> Vec<String> {
        let files = if self.options.preview || !self.options.jpeg() { 1 } else { self.pages() };
        let p = std::path::Path::new(path);
        let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        (0..files)
            .map(|i| if i == 0 { path.to_string() } else { p.with_file_name(format!("{stem} {}{ext}", i + 1)).to_string_lossy().into_owned() })
            .collect()
    }

    /// Cancellation produces no partial document; callers write only after this succeeds.
    pub fn run(self, progress: &mut dyn FnMut(usize, &str) -> bool) -> Result<Document, String> {
        let l = self.options.layout()?;
        let (per_page, pages, jpeg) = (self.options.per_page(), self.pages(), self.options.jpeg());
        let count = self.items.len();
        let mut pdf = if jpeg { None } else { Some(crate::pdf::Pdf::new(Vec::new())?) };
        let mut files = Vec::new();
        let mut size = 0usize;
        let mut page = Rgba8::filled(l.width, l.height, [255; 4]);
        for (i, (name, job)) in self.items.into_iter().enumerate() {
            if !progress(i, &name) {
                return Err("Contact sheet cancelled".into());
            }
            let image = job.run().rendered?.image;
            let slot = i % per_page;
            if slot == 0 {
                l.background(&mut page, i / per_page, pages, (count - i).min(per_page), &self.options);
            }
            l.photo(&mut page, slot, i, &image, &name, self.options.captions)?;
            if slot + 1 == per_page || i + 1 == count {
                let jpg = encode_image(&page, &ExportOptions { format: ExportFormat::Jpeg, quality: 92, ..Default::default() })?;
                size = size.saturating_add(jpg.len());
                if size > 256 * 1024 * 1024 {
                    return Err("contact sheet exceeds 256 MiB; select fewer photos".into());
                }
                match pdf.as_mut() {
                    Some(pdf) => pdf.image_page(&jpg, l.width, l.height, l.points, "")?,
                    None => files.push(jpg),
                }
            }
        }
        if !progress(count, if jpeg { "JPEG" } else { "PDF" }) {
            return Err("Contact sheet cancelled".into());
        }
        if let Some(pdf) = pdf {
            files.push(pdf.finish("")?);
        }
        Ok(Document { files, pages, photos: self.photos })
    }
}

/// Write `doc`'s files to `paths` ([`Prepared::paths`]) with `write`: the command's answer
/// `{path, paths, pages, photos, bytes}`.
pub fn write(doc: &Document, paths: &[String], write: &mut dyn FnMut(&str, &[u8]) -> Result<(), String>) -> Result<Value, String> {
    if paths.len() != doc.files.len() {
        return Err("contact sheet pages and files differ".into());
    }
    for (path, bytes) in paths.iter().zip(&doc.files) {
        write(path, bytes)?;
    }
    Ok(json!({"path": paths.first(), "paths": paths, "pages": doc.pages, "photos": doc.photos, "bytes": doc.bytes()}))
}

/// `text` without control characters, shortened in the middle to about one `em`-wide cell per
/// character in `width` pixels.
fn caption_text(text: &str, width: usize, em: f64) -> String {
    use unicode_segmentation::UnicodeSegmentation;
    let clean: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let cells: Vec<_> = clean.graphemes(true).collect();
    // A conservative one-em budget also accommodates the optional CJK faces.
    let limit = ((width as f64 / em.max(1.0)) as usize).max(3);
    if cells.len() <= limit {
        return clean;
    }
    let left = limit / 2;
    let right = limit - left - 1;
    format!("{}…{}", cells.iter().take(left).copied().collect::<String>(), cells.iter().skip(cells.len() - right).copied().collect::<String>())
}

/// Blend `color` over `r` (clipped to the page) at `alpha`, its corners rounded by `radius` pixels.
fn fill(page: &mut Rgba8, r: Rect, color: [u8; 3], alpha: u8, radius: f64) {
    let clamp = |v: f64, max: usize| (v.round().max(0.0) as usize).min(max);
    let (x0, x1) = (clamp(r.x, page.width), clamp(r.right(), page.width));
    let (y0, y1) = (clamp(r.y, page.height), clamp(r.bottom(), page.height));
    let a = u32::from(alpha);
    for y in y0..y1 {
        for x in x0..x1 {
            if radius > 0.0 {
                let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                let dx = (r.x + radius - px).max(px - (r.right() - radius)).max(0.0);
                let dy = (r.y + radius - py).max(py - (r.bottom() - radius)).max(0.0);
                if dx * dx + dy * dy > radius * radius {
                    continue;
                }
            }
            let Some(p) = page.data.get_mut(y * page.width + x) else { continue };
            for c in 0..3 {
                p[c] = ((u32::from(color[c]) * a + u32::from(p[c]) * (255 - a) + 127) / 255) as u8;
            }
            p[3] = 255;
        }
    }
}

fn blit(dst: &mut Rgba8, src: &Rgba8, x: usize, y: usize) -> Result<(), String> {
    if x.checked_add(src.width).is_none_or(|w| w > dst.width) || y.checked_add(src.height).is_none_or(|h| h > dst.height) {
        return Err("contact sheet image outside page".into());
    }
    for row in 0..src.height {
        let start = (y + row) * dst.width + x;
        let a = dst.data.get_mut(start..start + src.width).ok_or("invalid page buffer")?;
        let b = src.data.get(row * src.width..(row + 1) * src.width).ok_or("invalid render buffer")?;
        for (dst, src) in a.iter_mut().zip(b) {
            let alpha = u32::from(src[3]);
            for channel in 0..3 {
                dst[channel] = ((u32::from(src[channel]) * alpha + u32::from(dst[channel]) * (255 - alpha) + 127) / 255) as u8;
            }
            dst[3] = 255;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
