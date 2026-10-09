//! The export flow. On the desktop it is one form in a dialog ([`form`]). A phone gets what
//! Lightroom's mobile app has for it, drawn and worded our own way: a share sheet (the photos to
//! send, a check under each, and where to send them), the export options (file type, size, quality
//! and watermark as pull-down fields), the rest of the options, and a card with the progress while
//! it runs ([`progress`]).

use egui::{Align2, Color32, CornerRadius, Id, Rect, RichText, Sense, Stroke, StrokeKind, UiBuilder, pos2, vec2};
use lightcraft_develop::{ControlSpec, Section, Track};
use lightcraft_engine::export::{
    Anchor as P, ExportFormat as F, ExportOptions, MetadataPolicy as M, Resize, ResizeMode as R, SharpenAmount as A, SharpenFor as S, Watermark,
};
use serde_json::json;

use super::dialogs::{choices, field, num, trailing_button_row};
use super::mobile::{self, Item, PageBar};
use crate::LightcraftApp;
use crate::icons::Icon;
use crate::state::{Dialog, ExportPage, ExportThen};
use crate::theme::Tokens;
use crate::widgets::register;

// ------------------------------------------------------------------------------------------ the form

/// What the export form edits: the fields of [`Dialog::Export`].
pub struct Form<'a> {
    pub opts: &'a mut ExportOptions,
    pub full_size: &'a mut bool,
    pub resize: &'a mut Resize,
    pub preset_name: &'a mut String,
    pub limit_kb: &'a mut u32,
    pub dir: &'a mut String,
}

/// Which rows of the form to show: all of them (the desktop's dialog), or the ones a phone's
/// options page doesn't have (it has the file type, a few sizes and the quality itself).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    All,
    More,
}

const fn spec(id: &'static str, label: &'static str, min: f64, max: f64, default: f64, step: f64) -> ControlSpec {
    ControlSpec { id, label, section: Section::Light, min, max, default, step, decimals: 0, track: Track::Plain }
}
const QUALITY: ControlSpec = spec("export.quality", "Quality", 1.0, 100.0, 90.0, 1.0);
const LIMIT_KB: ControlSpec = spec("export.limitKb", "Limit file size (KB, 0 = off)", 0.0, 20_000.0, 0.0, 10.0);
const SIZE_PX: ControlSpec = spec("export.sizePx", "Pixels", 16.0, 20_000.0, 2048.0, 16.0);
const SIZE_W: ControlSpec = spec("export.sizeW", "Width (px)", 16.0, 20_000.0, 2048.0, 16.0);
const SIZE_H: ControlSpec = spec("export.sizeH", "Height (px)", 16.0, 20_000.0, 2048.0, 16.0);
const SIZE_MP: ControlSpec = ControlSpec { decimals: 1, ..spec("export.sizeMp", "Megapixels", 0.1, 100.0, 12.0, 0.1) };
const SIZE_PCT: ControlSpec = spec("export.sizePercent", "Percent", 1.0, 400.0, 50.0, 1.0);
const START_NUMBER: ControlSpec = spec("export.startNumber", "Start number", 1.0, 9999.0, 1.0, 1.0);
const PPI: ControlSpec = spec("export.ppi", "Resolution (ppi)", 1.0, 1200.0, 240.0, 1.0);
const WM_IMAGE_WIDTH: ControlSpec = spec("export.watermarkImageWidth", "Width (% of photo)", 2.0, 100.0, 20.0, 1.0);
const WM_SIZE: ControlSpec = spec("export.watermarkSize", "Size (% of short edge)", 1.0, 15.0, 3.5, 0.5);
const WM_OPACITY: ControlSpec = spec("export.watermarkOpacity", "Opacity (%)", 5.0, 100.0, 70.0, 1.0);

/// The export settings: the desktop dialog's body, and a phone's "More Options" page.
pub fn form(app: &mut LightcraftApp, ui: &mut egui::Ui, f: Form<'_>, part: Part) {
    let Form { opts, full_size, resize, preset_name, limit_kb, dir } = f;
    let t = Tokens::get(ui.ctx());
    let phone = crate::is_compact(ui.ctx());
    if part == Part::All {
        let n = app.session.selection.ids.len().max(1);
        ui.label(RichText::new(crate::i18n::tr_format!("{n} photo{}", if n == 1 { "" } else { "s" }, n = n)).color(t.text_dim));
    }
    // Preset: load a built-in or saved set of options into the dialog
    let mut chosen = None;
    let presets = app.session.all_export_presets();
    if phone {
        pulldown(ui, "exportPreset", "Preset", crate::i18n::tr("Choose…"), |ui| {
            for (i, (p, b)) in presets.iter().enumerate() {
                if mobile::row(ui, &format!("exportPreset-{i}"), None, crate::i18n::builtin_label(&p.name, *b), true) {
                    chosen = Some(p.name.clone());
                }
            }
        });
    } else {
        field(ui, "Preset", |ui| {
            let c = egui::ComboBox::from_id_salt("exportPreset").width(220.0).selected_text(crate::i18n::tr("Choose…")).show_ui(ui, |ui| {
                let mut builtin = true;
                for (p, b) in &presets {
                    if builtin && !*b {
                        ui.separator();
                    }
                    builtin = *b;
                    if ui.selectable_label(false, crate::i18n::builtin_label(&p.name, *b)).clicked() {
                        chosen = Some(p.name.clone());
                    }
                }
            });
            register(ui.ctx(), "combo:exportPreset", c.response.rect);
        });
    }
    if let Some(name) = chosen
        && let Ok(params) = app.session.export_params(&json!({"preset": name}))
    {
        let o = ExportOptions::from_json(&params);
        *full_size = o.resize.is_none();
        *resize = o.resize.unwrap_or_default();
        *limit_kb = o.limit_kb.unwrap_or(0);
        *opts = o;
    }
    ui.add_space(4.0);
    let before = opts.format;
    if part == Part::All {
        choices(
            ui,
            "Format",
            "exportFormat",
            &[
                (F::Jpeg, "JPEG"),
                (F::Png, "PNG"),
                (F::Tiff, "TIFF"),
                (F::Webp, "WebP"),
                (F::Avif, "AVIF"),
                (F::Dng, "DNG"),
                (F::Original, "Original files"),
            ],
            &mut opts.format,
        );
    }
    if !opts.format.is_rendered() {
        let note = if opts.format == F::Dng {
            "Raw photos as DNG, with the edits embedded. Size, color and output options don't apply."
        } else {
            "The original files, unchanged, each with an XMP sidecar holding its edits."
        };
        ui.label(RichText::new(note).color(t.text_dim));
    }
    if opts.format != before {
        // each format starts at its own default depth (TIFF 16-bit, others 8-bit)
        opts.bit_depth = None;
    }
    let rendered = opts.format.is_rendered();
    let depths = ExportOptions::bit_depths(opts.format);
    if rendered && depths.len() > 1 {
        let mut bd = opts.bit_depth.filter(|b| depths.iter().any(|d| d.0 == *b)).unwrap_or(depths[0].0);
        choices(ui, "Bit depth", "exportBitDepth", depths, &mut bd);
        opts.bit_depth = Some(bd);
    }
    if !rendered {
    } else if opts.format == F::Avif {
        ui.label(RichText::new(crate::i18n::tr("Color space: sRGB (AVIF)")).color(t.text_dim));
    } else {
        use lightcraft_engine::export::OutputSpace as C;
        choices(
            ui,
            "Color space",
            "exportColorSpace",
            &[(C::Srgb, "sRGB"), (C::DisplayP3, "P3"), (C::AdobeRgb, "Adobe RGB"), (C::ProPhoto, "ProPhoto"), (C::Rec2020, "Rec.2020")],
            &mut opts.color_space,
        );
    }
    if part == Part::All && matches!(opts.format, F::Jpeg | F::Avif) {
        let mut q = opts.quality as f64;
        if num(ui, &QUALITY, &mut q) {
            opts.quality = q as u8;
        }
    }
    if opts.format == F::Jpeg {
        let mut k = *limit_kb as f64;
        if num(ui, &LIMIT_KB, &mut k) {
            *limit_kb = k as u32;
        }
    }
    if rendered {
        export_size(ui, full_size, resize, &mut opts.ppi);
        choices(
            ui,
            "Sharpen",
            "exportSharpen",
            &[(S::None, "None"), (S::Screen, "Screen"), (S::Matte, "Matte"), (S::Glossy, "Glossy")],
            &mut opts.sharpen,
        );
        if opts.sharpen != S::None {
            choices(ui, "Amount", "exportSharpenAmount", &[(A::Low, "Low"), (A::Standard, "Standard"), (A::High, "High")], &mut opts.sharpen_amount);
        }
        choices(
            ui,
            "Metadata",
            "exportMetadata",
            &[(M::All, "All"), (M::AllExceptCamera, "No camera"), (M::Copyright, "Copyright"), (M::None, "None")],
            &mut opts.metadata,
        );
        if !matches!(opts.metadata, M::None | M::Copyright) {
            crate::widgets::check(ui, &mut opts.remove_location, crate::i18n::tr("Remove location info"));
        }
        let mut wm_on = opts.watermark.is_some();
        if crate::widgets::check(ui, &mut wm_on, crate::i18n::tr("Watermark")).changed() {
            opts.watermark = wm_on.then(|| Watermark { text: "© ".into(), ..Default::default() });
        }
        if let Some(wm) = &mut opts.watermark {
            watermark_form(app, ui, wm);
        }
    }
    ui.add_space(4.0);
    if opts.format == F::Tiff {
        use lightcraft_engine::export::TiffCompression as Z;
        choices(ui, "Compression", "exportTiffCompression", &[(Z::None, "None"), (Z::Lzw, "LZW"), (Z::Deflate, "ZIP")], &mut opts.tiff_compression);
    }
    if opts.format == F::Dng {
        use lightcraft_engine::export::DngCompression as Z;
        choices(
            ui,
            "Compression",
            "exportDngCompression",
            &[(Z::Lossless, "Lossless"), (Z::Deflate, "ZIP"), (Z::Uncompressed, "None")],
            &mut opts.dng_compression,
        );
    }
    let naming_id = egui::Id::new("export-naming");
    let tags_open = field(ui, "File name", |ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let w = (ui.available_width() - 50.0).max(80.0);
        ui.add(crate::widgets::touch_field(
            ui,
            egui::TextEdit::singleline(&mut opts.naming).id(naming_id).hint_text("{name}-{seq}  ·  {date}  ·  {title}  ·  {folder}").desired_width(w),
        ));
        crate::import::tag_toggle(ui, "exportNaming")
    });
    if tags_open {
        crate::import::tag_help(ui, "exportNaming", &mut opts.naming, naming_id);
    }
    crate::import::unknown_tags_warning(ui, &opts.naming);
    if opts.naming.contains("{seq") {
        let mut v = opts.start_number as f64;
        if num(ui, &START_NUMBER, &mut v) {
            opts.start_number = v as u32;
        }
    }
    if app.services.share_exports.is_some() {
        // iOS: no folder to choose; the share sheet saves or sends the photos
        let r = ui.label(
            RichText::new(crate::i18n::tr(
                "When the export is done, the share sheet opens: save the photos to Photos or Files, or send them to another app.",
            ))
            .color(t.text_dim)
            .small(),
        );
        register(ui.ctx(), "label:exportShareHelp", r.rect);
    } else {
        field(ui, "Folder", |ui| {
            trailing_button_row(ui, |ui| {
                if app.services.pick_folder.is_some()
                    && crate::widgets::text_button(ui, "exportChooseFolder", crate::i18n::tr("Choose…"), false).clicked()
                    && let Some(pick) = app.services.pick_folder.as_mut()
                    && let Some(d) = pick()
                {
                    *dir = d;
                }
                ui.add(crate::widgets::touch_field(ui, egui::TextEdit::singleline(dir).desired_width(ui.available_width())));
            });
        });
        field(ui, "Subfolder", |ui| {
            ui.add(crate::widgets::touch_field(
                ui,
                egui::TextEdit::singleline(&mut opts.subfolder).hint_text(crate::i18n::tr("none")).desired_width(f32::INFINITY),
            ))
        });
    }
    use lightcraft_engine::export::Conflict as K;
    choices(ui, "If file exists", "exportConflict", &[(K::Unique, "Add number"), (K::Overwrite, "Overwrite"), (K::Skip, "Skip")], &mut opts.conflict);
    ui.add_space(4.0);
    field(ui, "Save preset", |ui| {
        trailing_button_row(ui, |ui| {
            let named = !preset_name.trim().is_empty();
            if crate::widgets::text_button(ui, "exportSavePreset", crate::i18n::tr("Save"), false).clicked() && named {
                let params = dialog_params(opts, *full_size, resize, *limit_kb);
                match app.run("export.savePreset", json!({"name": preset_name.trim(), "params": params})) {
                    Ok(_) => {
                        app.toast(ui.ctx(), crate::i18n::tr_format!("Saved export preset “{}”", preset_name.trim()));
                        preset_name.clear();
                    }
                    Err(e) => app.toast(ui.ctx(), e),
                }
            }
            ui.add(crate::widgets::touch_field(
                ui,
                egui::TextEdit::singleline(preset_name).hint_text(crate::i18n::tr("Preset name")).desired_width(ui.available_width()),
            ));
        });
    });
}

/// A watermark's controls: text or a graphic, where it goes, how big and how strong.
fn watermark_form(app: &mut LightcraftApp, ui: &mut egui::Ui, wm: &mut Watermark) {
    // text, or a graphic (a logo with transparency)
    let mut graphic = !wm.image.is_empty() || ui.data(|d| d.get_temp::<bool>(egui::Id::new("wm-graphic"))).unwrap_or(false);
    let before = graphic;
    choices(ui, "Style", "exportWmStyle", &[(false, "Text"), (true, "Graphic")], &mut graphic);
    if graphic != before {
        ui.data_mut(|d| d.insert_temp(egui::Id::new("wm-graphic"), graphic));
        if !graphic {
            wm.image.clear();
        }
    }
    if graphic {
        field(ui, "Graphic", |ui| {
            trailing_button_row(ui, |ui| {
                if app.services.pick_files.is_some()
                    && crate::widgets::text_button(ui, "exportWmChoose", crate::i18n::tr("Choose…"), false).clicked()
                    && let Some(f) = app.services.pick_files.as_mut().and_then(|pick| pick().into_iter().next())
                {
                    wm.image = f;
                }
                ui.add(crate::widgets::touch_field(
                    ui,
                    egui::TextEdit::singleline(&mut wm.image).hint_text("logo.png").desired_width(ui.available_width()),
                ));
            });
        });
        let mut width = wm.image_width as f64 * 100.0;
        if num(ui, &WM_IMAGE_WIDTH, &mut width) {
            wm.image_width = (width / 100.0) as f32;
        }
    } else {
        choices(
            ui,
            app.ui.language.tr("Text direction"),
            "exportWmOrientation",
            &[(false, app.ui.language.tr("Horizontal text")), (true, app.ui.language.tr("Vertical text"))],
            &mut wm.vertical,
        );
        field(ui, "Text", |ui| {
            ui.add(crate::widgets::touch_field(
                ui,
                egui::TextEdit::multiline(&mut wm.text).desired_rows(2).hint_text("© Your Name").desired_width(f32::INFINITY),
            ))
        });
    }
    choices(
        ui,
        "Position",
        "exportWmAnchor",
        &[(P::TopLeft, "↖"), (P::TopRight, "↗"), (P::Center, "•"), (P::BottomLeft, "↙"), (P::BottomRight, "↘")],
        &mut wm.anchor,
    );
    let mut size = wm.size as f64 * 100.0;
    if !graphic && num(ui, &WM_SIZE, &mut size) {
        wm.size = (size / 100.0) as f32;
    }
    let mut op = wm.opacity as f64 * 100.0;
    if num(ui, &WM_OPACITY, &mut op) {
        wm.opacity = (op / 100.0) as f32;
    }
    if !graphic {
        crate::widgets::check(ui, &mut wm.shadow, crate::i18n::tr("Shadow"));
    }
}

/// Image Sizing: full size, or a resize mode and its value(s), don't enlarge, ppi.
fn export_size(ui: &mut egui::Ui, full: &mut bool, r: &mut Resize, ppi: &mut u16) {
    const MODES: [(R, &str); 7] = [
        (R::LongEdge, "Long Edge"),
        (R::ShortEdge, "Short Edge"),
        (R::Width, "Width"),
        (R::Height, "Height"),
        (R::Dimensions, "Width × Height"),
        (R::Megapixels, "Megapixels"),
        (R::Percent, "Percentage"),
    ];
    let r_full = crate::widgets::check(ui, full, crate::i18n::tr("Full size"));
    register(ui.ctx(), "check:exportFullSize", r_full.rect);
    if !*full {
        let cur = MODES.iter().find(|m| m.0 == r.mode).map_or("Long Edge", |m| m.1);
        let before = r.mode;
        if crate::is_compact(ui.ctx()) {
            pulldown(ui, "exportResizeMode", "Resize to", crate::i18n::tr(cur), |ui| {
                for (i, (m, l)) in MODES.iter().enumerate() {
                    if mobile::row_checked(ui, &format!("exportResizeMode-{i}"), None, crate::i18n::tr(l), true, Some(r.mode == *m)) {
                        r.mode = *m;
                    }
                }
            });
        } else {
            field(ui, "Resize to", |ui| {
                let c = egui::ComboBox::from_id_salt("exportResizeMode").width(150.0).selected_text(crate::i18n::tr(cur)).show_ui(ui, |ui| {
                    for (m, l) in MODES {
                        ui.selectable_value(&mut r.mode, m, crate::i18n::tr(l));
                    }
                });
                register(ui.ctx(), "combo:exportResizeMode", c.response.rect);
            });
        }
        if r.mode != before {
            // a sensible value for the new unit
            r.value = match r.mode {
                R::Megapixels => 12.0,
                R::Percent => 50.0,
                _ if matches!(before, R::Megapixels | R::Percent) => 2048.0,
                _ => r.value,
            };
            if r.mode == R::Dimensions && r.height == 0 {
                r.height = r.value as u32;
            }
        }
        let mut v = r.value as f64;
        let spec = match r.mode {
            R::Megapixels => &SIZE_MP,
            R::Percent => &SIZE_PCT,
            R::Dimensions => &SIZE_W,
            _ => &SIZE_PX,
        };
        if num(ui, spec, &mut v) {
            r.value = v as f32;
        }
        if r.mode == R::Dimensions {
            let mut h = r.height as f64;
            if num(ui, &SIZE_H, &mut h) {
                r.height = h as u32;
            }
        }
        let c = crate::widgets::check(ui, &mut r.dont_enlarge, crate::i18n::tr("Don't enlarge"));
        register(ui.ctx(), "check:exportDontEnlarge", c.rect);
    }
    let mut p = *ppi as f64;
    if num(ui, &PPI, &mut p) {
        *ppi = p as u16;
    }
}

/// The dialog's choices as `app.export` params (without the folder).
pub fn dialog_params(opts: &ExportOptions, full_size: bool, resize: &Resize, limit_kb: u32) -> serde_json::Value {
    let o = ExportOptions { resize: (!full_size).then_some(*resize), limit_kb: (limit_kb > 0).then_some(limit_kb), ..opts.clone() };
    o.to_json()
}

// ------------------------------------------------------------------------------------ phone widgets

/// A labelled field that opens a pull-down menu (`button:{id}`): the label over a rounded box with the
/// value and a chevron, as Lightroom's mobile app draws its options. `items` are the menu's rows
/// ([`mobile::row_checked`]); a tap on one closes the menu.
pub fn pulldown(ui: &mut egui::Ui, id: &str, label: &str, value: &str, items: impl FnOnce(&mut egui::Ui)) {
    let t = Tokens::get(ui.ctx());
    ui.add_space(6.0);
    ui.label(RichText::new(crate::i18n::tr(label)).size(13.0).color(t.text_dim));
    ui.add_space(2.0);
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 46.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, value));
    register(ui.ctx(), format!("button:{id}"), r);
    let p = ui.painter();
    p.rect_filled(r, 9.0, if resp.is_pointer_button_down_on() { t.hover } else { t.inset });
    p.rect_stroke(r, 9.0, Stroke::new(1.0, t.divider), StrokeKind::Inside);
    let galley = p.layout(value.to_string(), t.font(17.0), t.text, (r.width() - 56.0).max(40.0));
    p.galley(pos2(r.left() + 14.0, r.center().y - galley.size().y / 2.0), galley, t.text);
    crate::icons::paint(p, Rect::from_center_size(pos2(r.right() - 22.0, r.center().y), vec2(16.0, 16.0)), Icon::ChevronDown, t.text_dim);
    let ctx = ui.ctx().clone();
    if resp.clicked() {
        mobile::open_menu(&ctx, id, r);
    }
    let max_h = ctx.content_rect().height() * 0.6;
    mobile::actions(&ctx, id, None, |ui| {
        egui::ScrollArea::vertical().id_salt(id).max_height(max_h).show(ui, items);
    });
}

/// A full-width list row (`button:{id}`): an icon, the label, a smaller line under it, and at the
/// end a chevron or a button of its own (`trailing`: icon, widget id `icon:{id}`, what it's called).
/// Returns (the row was tapped, the trailing button was).
fn list_row(ui: &mut egui::Ui, id: &str, icon: Option<Icon>, label: &str, sub: Option<&str>, enabled: bool, trailing: Trailing) -> (bool, bool) {
    let t = Tokens::get(ui.ctx());
    let h = if sub.is_some() { 64.0 } else { 56.0 };
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
    // (the trailing button is a target of its own, 52 pt wide)
    let end = match trailing {
        Trailing::Chevron => 0.0,
        Trailing::Button(..) => 52.0,
    };
    let body = Rect::from_min_max(r.min, pos2(r.right() - end, r.bottom()));
    let resp = ui.interact(body, Id::new(("lc-share-row", id)), if enabled { Sense::click() } else { Sense::hover() });
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    register(ui.ctx(), format!("button:{id}"), body);
    let p = ui.painter();
    if enabled && resp.is_pointer_button_down_on() {
        p.rect_filled(r.expand2(vec2(16.0, 0.0)), 0.0, t.hover);
    }
    let (ink, dim) = if enabled { (t.text, t.text_dim) } else { (t.text_disabled, t.text_disabled) };
    let mut x = r.left();
    if let Some(icon) = icon {
        crate::icons::paint(p, Rect::from_center_size(pos2(x + 14.0, r.center().y), vec2(26.0, 26.0)), icon, ink);
        x += 44.0;
    }
    let w = (r.right() - end - x - 8.0).max(40.0);
    let name = p.layout(crate::i18n::tr(label).to_string(), t.font(17.0), ink, w);
    match sub {
        Some(sub) => {
            let sub = p.layout(crate::i18n::tr(sub).to_string(), t.font(13.0), dim, w);
            let top = r.center().y - (name.size().y + 2.0 + sub.size().y) / 2.0;
            p.galley(pos2(x, top + name.size().y + 2.0), sub, dim);
            p.galley(pos2(x, top), name, ink);
        }
        None => {
            p.galley(pos2(x, r.center().y - name.size().y / 2.0), name, ink);
        }
    }
    let mut tapped = false;
    match trailing {
        Trailing::Chevron => {
            crate::icons::paint(
                p,
                Rect::from_center_size(pos2(r.right() - 10.0, r.center().y), vec2(14.0, 14.0)),
                Icon::ChevronRight,
                t.text_disabled,
            );
        }
        Trailing::Button(icon, tip) => {
            let b = Rect::from_min_max(pos2(r.right() - end, r.top()), r.right_bottom());
            let resp = ui.interact(b, Id::new(("lc-share-row-end", id)), if enabled { Sense::click() } else { Sense::hover() });
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, crate::i18n::tr(tip)));
            register(ui.ctx(), format!("icon:{id}"), b);
            let c = if !enabled {
                t.text_disabled
            } else if resp.is_pointer_button_down_on() {
                t.text_dim
            } else {
                t.icon
            };
            crate::icons::paint(ui.painter(), Rect::from_center_size(pos2(b.right() - 20.0, b.center().y), vec2(24.0, 24.0)), icon, c);
            tapped = resp.clicked();
        }
    }
    (resp.clicked(), tapped)
}

#[derive(Clone, Copy)]
enum Trailing {
    Chevron,
    Button(Icon, &'static str),
}

/// A round button with its name under it (`button:{id}`): where the photos can go.
fn circle(ui: &mut egui::Ui, id: &str, w: f32, icon: Icon, tint: Color32, label: &str, enabled: bool) -> bool {
    const D: f32 = 56.0;
    let t = Tokens::get(ui.ctx());
    let (cell, resp) = ui.allocate_exact_size(vec2(w, D + 30.0), if enabled { Sense::click() } else { Sense::hover() });
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, crate::i18n::tr(label)));
    register(ui.ctx(), format!("button:{id}"), cell);
    let k = ui.ctx().animate_bool_with_time(resp.id.with("press"), enabled && resp.is_pointer_button_down_on(), 0.1);
    let c = pos2(cell.center().x, cell.top() + D / 2.0);
    let p = ui.painter();
    p.circle_filled(c, D / 2.0 * (1.0 - 0.06 * k), if k > 0.0 { t.pressed } else { t.cell_selected });
    crate::icons::paint(p, Rect::from_center_size(c, vec2(26.0, 26.0)), icon, if enabled { tint } else { t.text_disabled });
    p.text(
        pos2(cell.center().x, cell.top() + D + 16.0),
        Align2::CENTER_CENTER,
        crate::i18n::tr(label),
        t.font(13.0),
        if enabled { t.text_label } else { t.text_disabled },
    );
    enabled && resp.clicked()
}

/// A thin line across the page (the sheet's sections are separated by them).
fn rule(ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(r.expand2(vec2(16.0, 0.0)).x_range(), r.center().y, Stroke::new(0.5, t.divider));
}

// ------------------------------------------------------------------------------------- the filmstrip

/// Space between two photos of the strip, and before the first.
const FILM_GAP: f32 = 12.0;
/// Side of the check box under a photo.
const BOX: f32 = 24.0;
/// Room above the strip's photos (their ring is drawn outside them) and under its check boxes.
const FILM_TOP: f32 = 8.0;
const FILM_BOTTOM: f32 = 10.0;
/// Where a photo of the strip is: its left edge, width and height (the strip is `h` high, the photo
/// is centred in it).
type Cell = (f32, f32, f32);
/// What the strip's cached layout was made for: the photo list (its generation), the strip's height
/// and the widest a photo may be.
type FilmKey = (u64, u32, u32);

/// Where each photo of the strip is: as high as the strip, as wide as its shape makes it, but no wider
/// than `cap` (a landscape photo would fill the screen), cached while the list is the same.
fn film_layout(
    app: &mut LightcraftApp,
    ctx: &egui::Context,
    generation: u64,
    ids: &[lightcraft_catalog::PhotoId],
    h: f32,
    cap: f32,
) -> std::sync::Arc<Vec<Cell>> {
    let id = Id::new("lc-share-film-layout");
    let key: FilmKey = (generation, h as u32, cap as u32);
    if let Some((k, v)) = ctx.data(|d| d.get_temp::<(FilmKey, std::sync::Arc<Vec<Cell>>)>(id))
        && k == key
    {
        return v;
    }
    let aspects = super::grid::aspects(&app.session.catalog, ids);
    let mut x = FILM_GAP;
    let cells: Vec<Cell> = aspects
        .iter()
        .map(|a| {
            let a = if a.is_finite() { a.clamp(0.4, 2.5) } else { 1.5 };
            let w = (a * h).min(cap).round();
            let at = x;
            x += w + FILM_GAP;
            (at, w, (w / a).round())
        })
        .collect();
    let v = std::sync::Arc::new(cells);
    ctx.data_mut(|d| d.insert_temp(id, (key, v.clone())));
    v
}

/// The strip of photos with a check box under each (the chosen ones have a ring), edge to edge in
/// the page's body, scrolling sideways. A tap on a photo or its box chooses or unchooses it.
fn film(app: &mut LightcraftApp, ui: &mut egui::Ui, chosen: &mut Vec<u64>, centered: &mut bool, h: f32) {
    let t = Tokens::get(ui.ctx());
    let ctx = ui.ctx().clone();
    let total_h = FILM_TOP + h + 16.0 + BOX + FILM_BOTTOM;
    // (the page's body has 16 pt of margin each side; the strip ignores it)
    let (slot, _) = ui.allocate_exact_size(vec2(ui.available_width(), total_h), Sense::hover());
    let strip = slot.expand2(vec2(16.0, 0.0));
    let (generation, visible) = app.session.visible_shared();
    // photos the list doesn't have (a filter changed since): in front, so that they can be unchosen
    let mut list: Vec<lightcraft_catalog::PhotoId> = if chosen.len() <= 16 {
        chosen.iter().map(|i| lightcraft_catalog::PhotoId(*i)).filter(|i| !visible.contains(i)).collect()
    } else {
        Vec::new()
    };
    let extra = list.len();
    let owned: Vec<lightcraft_catalog::PhotoId>;
    let (generation, ids): (u64, &[lightcraft_catalog::PhotoId]) = if extra == 0 {
        (generation, &visible[..])
    } else {
        list.extend(visible.iter().copied());
        owned = list;
        (generation ^ 0x9e37_79b9_7f4a_7c15 ^ extra as u64, &owned[..])
    };
    let cells = film_layout(app, &ctx, generation, ids, h, strip.width() * 0.62);
    let content_w = cells.last().map_or(0.0, |(x, w, _)| x + w + FILM_GAP).max(strip.width());
    let mut child = ui.new_child(UiBuilder::new().max_rect(strip).id_salt("share-film"));
    child.set_clip_rect(strip.intersect(ui.clip_rect()));
    let mut scroll = egui::ScrollArea::horizontal()
        .id_salt("share-film-scroll")
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
        .auto_shrink([false, false]);
    if !*centered {
        // the first chosen photo in the middle
        if let Some((x, w, _)) = ids.iter().position(|i| chosen.contains(&i.0)).and_then(|i| cells.get(i)) {
            scroll = scroll.horizontal_scroll_offset((x + w / 2.0 - strip.width() / 2.0).clamp(0.0, (content_w - strip.width()).max(0.0)));
        }
        *centered = true;
    }
    let mut toggled = None;
    scroll.show_viewport(&mut child, |ui, view| {
        let (content, _) = ui.allocate_exact_size(vec2(content_w, total_h), Sense::hover());
        let first = cells.partition_point(|(x, w, _)| x + w < view.left() - 1.0);
        for (i, (x, w, ph)) in cells.iter().enumerate().skip(first) {
            if *x > view.right() + 1.0 {
                break;
            }
            let Some(&photo) = ids.get(i) else { break };
            let thumb = Rect::from_min_size(content.min + vec2(*x, FILM_TOP + (h - ph) / 2.0), vec2(*w, *ph));
            let on = chosen.contains(&photo.0);
            let name = app.session.catalog.photo(photo).map(|p| p.file_name.clone()).unwrap_or_default();
            let resp = ui.interact(thumb, Id::new(("share-thumb", photo.0)), Sense::click());
            resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, on, &name));
            register(ui.ctx(), format!("thumb:share-{}", photo.0), thumb);
            if app.renderer.thumb(photo).is_none() {
                super::grid::request_thumb(app, photo, 512, 8);
            }
            let p = ui.painter();
            match app.renderer.thumb(photo) {
                Some(tex) => {
                    let fade = ui.ctx().animate_bool_with_time(Id::new(("share-thumb-in", photo.0)), true, 0.2);
                    p.image(tex.tex.id(), thumb, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE.gamma_multiply(fade));
                }
                None => {
                    p.rect_filled(thumb, 4.0, t.inset);
                    let _ = ui.ctx().animate_bool_with_time(Id::new(("share-thumb-in", photo.0)), false, 0.0);
                }
            }
            if on {
                p.rect_stroke(thumb.expand(3.0), 6.0, Stroke::new(3.0, t.accent), StrokeKind::Middle);
            }
            // the box: a tap on it counts as a tap on the photo
            let b = Rect::from_center_size(pos2(thumb.center().x, content.top() + FILM_TOP + h + 16.0 + BOX / 2.0), vec2(BOX, BOX));
            let hit = ui.interact(b.expand(10.0), Id::new(("share-box", photo.0)), Sense::click());
            hit.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, on, &name));
            register(ui.ctx(), format!("check:shareItem-{}", photo.0), b.expand(10.0));
            let k = ui.ctx().animate_bool_with_time(Id::new(("share-box-on", photo.0)), on, 0.12);
            let p = ui.painter();
            if k > 0.0 {
                p.rect_filled(b, 6.0, t.accent.gamma_multiply(k));
                crate::icons::paint(p, b.shrink(4.0), Icon::Check, Color32::WHITE.gamma_multiply(k));
            }
            if k < 1.0 {
                p.rect_stroke(b, 6.0, Stroke::new(1.5, t.text_dim.gamma_multiply(1.0 - k)), StrokeKind::Inside);
            }
            if resp.clicked() || hit.clicked() {
                toggled = Some(photo.0);
            }
        }
    });
    if let Some(id) = toggled {
        match chosen.iter().position(|c| *c == id) {
            Some(i) => {
                chosen.remove(i);
            }
            None => chosen.push(id),
        }
        crate::haptics::tap(&ctx, crate::haptics::Haptic::Selection);
    }
}

// --------------------------------------------------------------------------------------- the pages

/// What the phone's export flow asks of the dialog after a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Stay,
    Close,
    /// Export the chosen photos, then send them on.
    Go(ExportThen),
}

/// The host has somewhere to send exports other than a folder (iOS: the share sheet, Photos): the
/// phone's flow then starts at its share sheet; otherwise at the options.
pub fn has_sheet(app: &LightcraftApp) -> bool {
    app.services.share_exports.is_some() || app.services.save_to_photos.is_some()
}

/// `ids` in the order of the grid (so that `{seq}` in a file name follows what the user sees), then
/// the ones the grid doesn't show.
pub fn in_grid_order(app: &mut LightcraftApp, ids: &[u64]) -> Vec<u64> {
    let want: std::collections::HashSet<u64> = ids.iter().copied().collect();
    let mut out: Vec<u64> = app.session.visible().iter().map(|p| p.0).filter(|id| want.contains(id)).collect();
    let shown: std::collections::HashSet<u64> = out.iter().copied().collect();
    out.extend(ids.iter().copied().filter(|id| !shown.contains(id)));
    out
}

/// The photos the export dialog starts with: the selection, else the open photo.
pub fn initial_ids(app: &mut LightcraftApp) -> Vec<u64> {
    let want: Vec<u64> = app.session.targets(&json!({})).iter().map(|p| p.0).collect();
    in_grid_order(app, &want)
}

/// How the pull-down offers sizes: the long edge, and its name.
const SIZES: [(u32, &str); 3] = [(1080, "Small"), (2048, "Medium"), (4096, "Large")];
/// The qualities the pull-down offers.
const QUALITIES: [u8; 8] = [100, 95, 90, 85, 80, 70, 60, 50];

/// What the size field says: a named size, "Full size", or "Custom" for anything the pull-down
/// doesn't offer (set under More Options).
fn size_label(full: bool, r: &Resize) -> String {
    if full {
        return crate::i18n::tr("Full size").to_string();
    }
    let name = |px: u32, name: &str| format!("{} ({px} px)", crate::i18n::tr(name));
    match SIZES.iter().find(|(px, _)| r.mode == R::LongEdge && r.value == *px as f32) {
        Some((px, n)) => name(*px, n),
        None if r.mode == R::LongEdge => name(r.value as u32, "Custom"),
        None => crate::i18n::tr("Custom").to_string(),
    }
}

/// The phone's export flow as pages: the share sheet, over it the options, over those the rest of
/// them. `open` goes false when the dialog is closed, and they slide away drawn as they were. Call
/// it every frame while the dialog is shown or leaving. The second value: every page has gone.
pub fn phone(app: &mut LightcraftApp, ctx: &egui::Context, dlg: &mut Dialog, open: bool) -> (Outcome, bool) {
    let Dialog::Export { opts, full_size, resize, preset_name, limit_kb, dir, ids, page, then, centered } = dlg else { return (Outcome::Stay, true) };
    let sheet = has_sheet(app);
    if !sheet && *page == ExportPage::Share {
        *page = ExportPage::Options;
    }
    let mut out = Outcome::Stay;
    let mut next = *page;

    // the share sheet
    let (_, visible) = app.session.visible_shared();
    let all = !visible.is_empty() && ids.len() >= visible.len();
    let mut gone = true;
    if sheet {
        let pill = if all { "Deselect All" } else { "Select All" };
        let bar = mobile::page_with(
            ctx,
            "dialog",
            PageBar {
                title: "Share",
                left: Item::Icon(Icon::Close, "Close", false),
                right: Item::Pill(pill),
                left_id: "shareClose",
                right_id: "shareSelectAll",
            },
            open,
            |ui| match share_body(app, ui, ids, centered) {
                Some(Act::Send(to)) => out = Outcome::Go(to),
                Some(Act::Options(to)) => {
                    *then = to;
                    next = ExportPage::Options;
                }
                None => {}
            },
        );
        gone &= bar.gone;
        if bar.cancel {
            out = Outcome::Close;
        }
        if bar.ok {
            if all {
                ids.clear();
            } else {
                *ids = visible.iter().map(|p| p.0).collect();
            }
        }
    } else {
        mobile::hidden(ctx, "dialogOptions");
    }

    // the options (the first page when the host has no share sheet)
    let n = ids.len();
    let bar = mobile::page_with(
        ctx,
        if sheet { "dialogOptions" } else { "dialog" },
        PageBar {
            title: "Export Options",
            left: Item::Icon(Icon::Close, "Close", false),
            right: Item::Icon(Icon::Check, "Export", true),
            left_id: "optionsClose",
            right_id: "optionsDone",
        },
        open && *page != ExportPage::Share,
        |ui| {
            let f = Form { opts, full_size, resize, preset_name, limit_kb, dir };
            if options_body(ui, f, n) {
                next = ExportPage::More;
            }
        },
    );
    gone &= bar.gone;
    if bar.cancel {
        if sheet {
            next = ExportPage::Share;
        } else {
            out = Outcome::Close;
        }
    }
    if bar.ok && n > 0 {
        out = Outcome::Go(*then);
    }

    // the rest of the options
    let bar = mobile::page_with(
        ctx,
        "dialogMore",
        PageBar { title: "More Options", left: Item::Text("‹Options", true), right: Item::None, left_id: "moreBack", right_id: "moreNone" },
        open && *page == ExportPage::More,
        |ui| {
            let f = Form { opts, full_size, resize, preset_name, limit_kb, dir };
            form(app, ui, f, Part::More);
        },
    );
    gone &= bar.gone;
    if bar.cancel {
        next = ExportPage::Options;
    }
    *page = next;
    (out, gone)
}

/// What a tap in the share sheet asks for.
enum Act {
    /// Export and send the photos on, with the options as they are.
    Send(ExportThen),
    /// Open the options; their check mark then sends the photos on.
    Options(ExportThen),
}

/// The share sheet's contents: the photos to send, and where.
fn share_body(app: &mut LightcraftApp, ui: &mut egui::Ui, chosen: &mut Vec<u64>, centered: &mut bool) -> Option<Act> {
    let t = Tokens::get(ui.ctx());
    ui.spacing_mut().item_spacing.y = 0.0;
    let room = ui.ctx().content_rect().height();
    let h = (room * 0.27).clamp(130.0, 240.0);
    ui.add_space(6.0);
    film(app, ui, chosen, centered, h);
    let n = chosen.len();
    ui.vertical_centered(|ui| {
        let text = if n == 0 {
            crate::i18n::tr("No photo chosen").to_string()
        } else {
            crate::i18n::tr_format!("{n} photo{}", if n == 1 { "" } else { "s" }, n = n)
        };
        let r = ui.label(RichText::new(text).size(13.0).color(t.text_dim));
        register(ui.ctx(), "label:shareCount", r.rect);
    });
    ui.add_space(10.0);
    rule(ui);
    ui.add_space(16.0);
    let any = n > 0;
    let mut act = None;
    let shares = app.services.share_exports.is_some();
    let cells = if shares { 2 } else { 1 };
    let w = (ui.available_width() / cells as f32).min(170.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.add_space(((ui.available_width() - w * cells as f32) / 2.0).max(0.0));
        if shares && circle(ui, "shareShare", w, Icon::Share, t.accent, "Share", any) {
            act = Some(Act::Send(ExportThen::Share));
        }
        if circle(ui, "shareExportAs", w, Icon::Dots, t.text, "Export As…", any) {
            act = Some(Act::Options(ExportThen::Share));
        }
    });
    ui.add_space(12.0);
    rule(ui);
    if app.services.save_to_photos.is_some() {
        let (tapped, gear) =
            list_row(ui, "shareSave", Some(Icon::Download), "Save to Photos", None, any, Trailing::Button(Icon::Gear, "Export Options"));
        if tapped {
            act = Some(Act::Send(ExportThen::Save));
        } else if gear {
            act = Some(Act::Options(ExportThen::Save));
        }
    }
    act
}

/// The options page: file type, size and quality as pull-down fields, the watermark, and a row to
/// the rest. True when that row was tapped.
fn options_body(ui: &mut egui::Ui, f: Form<'_>, n: usize) -> bool {
    let t = Tokens::get(ui.ctx());
    let Form { opts, full_size, resize, .. } = f;
    ui.spacing_mut().item_spacing.y = 0.0;
    let r = ui.label(RichText::new(crate::i18n::tr_format!("{n} photo{}", if n == 1 { "" } else { "s" }, n = n)).size(13.0).color(t.text_dim));
    register(ui.ctx(), "label:exportCount", r.rect);
    ui.add_space(6.0);
    // file type
    const TYPES: [(F, &str); 7] =
        [(F::Jpeg, "JPEG"), (F::Png, "PNG"), (F::Tiff, "TIFF"), (F::Webp, "WebP"), (F::Avif, "AVIF"), (F::Dng, "DNG"), (F::Original, "Original")];
    let now = TYPES.iter().find(|(x, _)| *x == opts.format).map_or("JPEG", |(_, l)| *l);
    let mut picked = None;
    pulldown(ui, "exportFileType", "File Type", crate::i18n::tr(now), |ui| {
        for (i, (x, l)) in TYPES.iter().enumerate() {
            if mobile::row_checked(ui, &format!("exportFileType-{i}"), None, crate::i18n::tr(l), true, Some(*x == opts.format)) {
                picked = Some(*x);
            }
        }
    });
    if let Some(x) = picked.filter(|x| *x != opts.format) {
        opts.format = x;
        // each format starts at its own default depth (TIFF 16-bit, others 8-bit)
        opts.bit_depth = None;
    }
    if !opts.format.is_rendered() {
        let note = if opts.format == F::Dng {
            "Raw photos as DNG, with the edits embedded. Size, color and output options don't apply."
        } else {
            "The original files, unchanged, each with an XMP sidecar holding its edits."
        };
        ui.add_space(8.0);
        ui.label(RichText::new(crate::i18n::tr(note)).size(13.0).color(t.text_dim));
    } else {
        // size
        let mut size = None;
        pulldown(ui, "exportSize", "Size", &size_label(*full_size, resize), |ui| {
            for (i, (px, name)) in SIZES.iter().enumerate() {
                let on = !*full_size && resize.mode == R::LongEdge && resize.value == *px as f32;
                if mobile::row_checked(ui, &format!("exportSize-{i}"), None, &format!("{} ({px} px)", crate::i18n::tr(name)), true, Some(on)) {
                    size = Some(Some(*px));
                }
            }
            if mobile::row_checked(ui, "exportSize-full", None, crate::i18n::tr("Full size"), true, Some(*full_size)) {
                size = Some(None);
            }
        });
        match size {
            Some(Some(px)) => {
                *full_size = false;
                resize.mode = R::LongEdge;
                resize.value = px as f32;
            }
            Some(None) => *full_size = true,
            None => {}
        }
        // quality
        if matches!(opts.format, F::Jpeg | F::Avif) {
            let mut quality = None;
            pulldown(ui, "exportQuality", "Image Quality", &format!("{}%", opts.quality), |ui| {
                for (i, q) in QUALITIES.iter().enumerate() {
                    if mobile::row_checked(ui, &format!("exportQuality-{i}"), None, &format!("{q}%"), true, Some(opts.quality == *q)) {
                        quality = Some(*q);
                    }
                }
            });
            if let Some(q) = quality {
                opts.quality = q;
            }
        }
        ui.add_space(14.0);
        rule(ui);
        // watermark
        let mut on = opts.watermark.is_some();
        let r = crate::widgets::check(ui, &mut on, RichText::new(crate::i18n::tr("Include Watermark")).size(17.0));
        register(ui.ctx(), "check:exportWatermark", r.rect);
        if r.changed() {
            opts.watermark = on.then(|| Watermark { text: "© ".into(), ..Default::default() });
        }
        if let Some(wm) = opts.watermark.as_mut().filter(|w| w.image.is_empty()) {
            let r = ui
                .add(crate::widgets::touch_field(ui, egui::TextEdit::singleline(&mut wm.text).hint_text("© Your Name").desired_width(f32::INFINITY)));
            register(ui.ctx(), "field:exportWatermarkText", r.rect);
            ui.add_space(10.0);
        }
    }
    rule(ui);
    let (more, _) = list_row(ui, "exportMore", None, "More Options", None, true, Trailing::Chevron);
    rule(ui);
    more
}

// ---------------------------------------------------------------------------------------- progress

/// A phone's export progress: a card in the middle of the dimmed screen, as iOS shows a task it is
/// waiting for. `done` photos are finished of `total`, `current` is the file in hand. True when
/// Cancel was tapped.
pub fn progress(ctx: &egui::Context, done: usize, total: usize, current: &str, stopping: bool) -> bool {
    const FADE_S: f32 = 0.2;
    let t = Tokens::get(ctx);
    let id = Id::new("lc-export-progress");
    let p = ctx.animate_bool_with_time_and_easing(id.with("in"), true, FADE_S, egui::emath::easing::cubic_out);
    let screen = ctx.viewport_rect();
    let content = ctx.content_rect();
    egui::Area::new(id.with("dim")).order(egui::Order::Foreground).fixed_pos(screen.min).show(ctx, |ui| {
        ui.painter().rect_filled(screen, 0.0, Color32::from_black_alpha((t.scrim as f32 * 0.8 * p) as u8));
        let _ = ui.allocate_rect(screen, Sense::click_and_drag());
    });
    let width = 340.0_f32.min(content.width() - 40.0).max(220.0);
    let mut cancel = false;
    egui::Area::new(id)
        .order(egui::Order::Tooltip)
        .constrain(false)
        .pivot(Align2::CENTER_CENTER)
        .fixed_pos(content.center() + vec2(0.0, (1.0 - p) * 10.0))
        .show(ctx, |ui| {
            ui.multiply_opacity(p);
            let (card, _) = ui.allocate_exact_size(vec2(width, 218.0), Sense::hover());
            register(ctx, "dialog:exportProgress", card);
            let painter = ui.painter();
            painter.add(
                egui::epaint::Shadow { offset: [0, 8], blur: 30, spread: 0, color: Color32::from_black_alpha(90) }
                    .as_shape(card, CornerRadius::same(14)),
            );
            painter.rect_filled(card, 14.0, t.cell_selected);
            let left = card.left() + 20.0;
            let inner = card.width() - 40.0;
            painter.text(pos2(left, card.top() + 28.0), Align2::LEFT_CENTER, crate::i18n::tr("Exporting"), t.semibold(17.0), t.text);
            painter.hline(card.x_range(), card.top() + 54.0, Stroke::new(0.5, t.divider));
            let name = painter.layout(current.to_string(), t.font(15.0), t.text, inner);
            painter.galley(pos2(left, card.top() + 66.0), name, t.text);
            painter.text(
                pos2(left, card.top() + 106.0),
                Align2::LEFT_CENTER,
                crate::i18n::tr_format!("Exporting {} of {total}", (done + 1).min(total), total = total),
                t.font(15.0),
                t.text_label,
            );
            let bar = Rect::from_min_size(pos2(left, card.top() + 128.0), vec2(inner, 6.0));
            register(ctx, "progress:export", bar);
            painter.rect_filled(bar, 3.0, t.inset);
            let frac = (done as f32 / total.max(1) as f32).clamp(0.0, 1.0);
            painter.rect_filled(Rect::from_min_size(bar.min, vec2(inner * frac, 6.0)), 3.0, t.accent);
            // Cancel: an outlined capsule at the right
            let label = crate::i18n::tr(if stopping { "Stopping…" } else { "Cancel" });
            let galley = painter.layout_no_wrap(label.to_string(), t.semibold(15.0), t.text);
            let pill = Rect::from_min_size(
                pos2(card.right() - 20.0 - (galley.size().x + 40.0), card.bottom() - 20.0 - 40.0),
                vec2(galley.size().x + 40.0, 40.0),
            );
            let resp = ui.interact(pill, id.with("cancel"), if stopping { Sense::hover() } else { Sense::click() });
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, !stopping, label));
            register(ctx, "button:exportCancel", pill);
            let painter = ui.painter();
            if resp.is_pointer_button_down_on() {
                painter.rect_filled(pill, 20.0, t.hover);
            }
            painter.rect_stroke(pill, 20.0, Stroke::new(1.5, if stopping { t.text_disabled } else { t.text }), StrokeKind::Inside);
            painter.galley(pill.center() - galley.size() / 2.0, galley, if stopping { t.text_disabled } else { t.text });
            cancel = resp.clicked();
        });
    cancel
}
