//! The crop screen of the compact (phone) layout, modelled on Lightroom's mobile app. While the
//! Crop tool is open it takes the screen over from the tool bar and the tool sheet: a title (what
//! the photo is cropped to) with Undo above the photo; under it an angle dial and four round
//! buttons (level the horizon, lock the aspect, rotate, more); a panel with an Aspect tab
//! (Original, Ratios, Instagram and TikTok formats) and a Geometry tab (Upright and the manual
//! sliders); and a bar with ✕ and ✓. Edits apply live, so the photo already shows the result: ✓
//! keeps it, ✕ undoes every step taken since the screen opened. The drawing and the words are our
//! own. Held sideways, the title, the panel and the bar are one column on the right.

use egui::{Align2, Color32, Id, Rect, Sense, Stroke, StrokeKind, Ui, UiBuilder, pos2, vec2};
use lightcraft_develop::{DevelopSettings, Upright};
use serde_json::json;

use super::{compact, mobile};
use crate::LightcraftApp;
use crate::haptics::{self, Haptic};
use crate::icons::{self, Icon};
use crate::state::{RightPanel, ViewMode};
use crate::theme::Tokens;
use crate::widgets::register;

/// The title row over the photo, and the bar of ✕ and ✓ under everything.
const TITLE_H: f32 = 44.0;
const BAR_H: f32 = 50.0;
/// The Aspect / Geometry tabs, and the height of each tab's content.
const TABS_H: f32 = 40.0;
const ASPECT_H: f32 = 84.0;
const GEOMETRY_H: f32 = 236.0;
/// The angle dial, the round buttons under it, and their diameter.
const DIAL_H: f32 = 44.0;
const BUTTONS_H: f32 = 52.0;
const ROUND_D: f32 = 44.0;
/// The Aspect tab's tiles are this wide (four fit the column held sideways).
const TILE_W: f32 = 66.0;
/// The column held sideways, and the widest a tab's content gets (iPad).
const SIDE_W: f32 = 316.0;
const MAX_CONTENT_W: f32 = 600.0;
/// Points of dial per degree: a drag across it covers about 30°.
const PPD: f32 = 7.0;
/// The radius of the arc the dial's ticks follow.
const DIAL_R: f32 = 220.0;

/// An aspect ratio of a list: the `crop.aspect` name and the two sides, shorter first.
struct Ratio {
    key: &'static str,
    short: f64,
    long: f64,
}

const fn ratio(key: &'static str, short: f64, long: f64) -> Ratio {
    Ratio { key, short, long }
}

/// The Ratios list, as Lightroom's mobile app has it.
const RATIOS: [Ratio; 9] = [
    ratio("1x1", 1.0, 1.0),
    ratio("1x2", 1.0, 2.0),
    ratio("2x3", 2.0, 3.0),
    ratio("3x4", 3.0, 4.0),
    ratio("4x5", 4.0, 5.0),
    ratio("5x7", 5.0, 7.0),
    ratio("8.5x11", 8.5, 11.0),
    ratio("9x16", 9.0, 16.0),
    ratio("10x16", 10.0, 16.0),
];

/// The formats of the two social networks: (name shown, ratio).
const POSTS: [(&str, Ratio); 4] = [
    ("Square", ratio("1x1", 1.0, 1.0)),
    ("Portrait", ratio("4x5", 4.0, 5.0)),
    ("Story or Reel", ratio("9x16", 9.0, 16.0)),
    ("Landscape", ratio("100x191", 1.0, 1.91)),
];
const VIDEOS: [(&str, Ratio); 3] = [("Video", ratio("9x16", 9.0, 16.0)), ("Photo", ratio("3x4", 3.0, 4.0)), ("Square", ratio("1x1", 1.0, 1.0))];

/// Whether the compact layout shows the crop screen: a photo with the Crop tool open.
pub fn open(app: &LightcraftApp) -> bool {
    app.compact && app.ui.view == ViewMode::Detail && !app.ui.presets && app.ui.right == RightPanel::Crop && app.session.active().is_some()
}

/// Called every frame: remembers where the undo stack stood when the screen opened (and starts it
/// on the Aspect tab).
pub fn track(app: &mut LightcraftApp, open: bool) {
    match (open, app.ui.crop_undo_base) {
        (true, None) => {
            app.ui.crop_undo_base = Some(app.session.undo.len());
            app.ui.crop_geometry = false;
        }
        (false, Some(_)) => app.ui.crop_undo_base = None,
        _ => {}
    }
}

/// ✕: undo every step taken since the screen opened, then back to Edit.
pub fn cancel(app: &mut LightcraftApp) {
    let _ = app.session.end_interaction();
    let base = app.ui.crop_undo_base.unwrap_or(app.session.undo.len());
    while app.session.undo.len() > base && app.run("edit.undo", json!({})).is_ok() {}
    app.ui.tool.clear();
    app.gesture = None;
    app.ui.right = RightPanel::Edit;
}

/// The screen's panels: the title, the tools under the photo, the tabs' panel and the bar. The photo
/// takes what is left (`compact::content`).
pub fn show(app: &mut LightcraftApp, ui: &mut Ui, t: &Tokens, landscape: bool) {
    let geometry = app.ui.crop_geometry;
    let frame = |fill: Color32| egui::Frame::NONE.fill(fill);
    if landscape {
        egui::Panel::right("crop_side").resizable(false).exact_size(SIDE_W).frame(frame(t.chrome)).show(ui, |ui| {
            let full = ui.max_rect();
            let part = |top: f32, bottom: f32| Rect::from_min_max(pos2(full.left(), top), pos2(full.right(), bottom));
            let mut title = ui.new_child(UiBuilder::new().max_rect(part(full.top(), full.top() + TITLE_H)));
            title_bar(app, &mut title, t);
            let mut body = ui.new_child(UiBuilder::new().max_rect(part(full.top() + TITLE_H, full.bottom() - BAR_H)));
            tabs_panel(app, &mut body, t);
            let mut bar = ui.new_child(UiBuilder::new().max_rect(part(full.bottom() - BAR_H, full.bottom())));
            bottom_bar(app, &mut bar, t);
        });
        if !geometry {
            egui::Panel::bottom("crop_tools").show_separator_line(false).exact_size(BUTTONS_H).frame(frame(t.canvas)).show(ui, |ui| {
                tools(app, ui, t, true);
            });
        }
    } else {
        egui::Panel::top("crop_top").show_separator_line(false).exact_size(TITLE_H).frame(frame(t.canvas)).show(ui, |ui| title_bar(app, ui, t));
        egui::Panel::bottom("crop_bar").show_separator_line(false).exact_size(BAR_H).frame(frame(t.canvas)).show(ui, |ui| {
            bottom_bar(app, ui, t);
        });
        let panel_h = TABS_H + if geometry { GEOMETRY_H } else { ASPECT_H };
        egui::Panel::bottom("crop_panel").show_separator_line(false).exact_size(panel_h).frame(frame(t.chrome)).show(ui, |ui| {
            tabs_panel(app, ui, t);
        });
        if !geometry {
            egui::Panel::bottom("crop_tools").show_separator_line(false).exact_size(DIAL_H + BUTTONS_H).frame(frame(t.canvas)).show(ui, |ui| {
                tools(app, ui, t, false);
            });
        }
    }
    // (what is left of the screen is the photo's)
    if app.ui.tool == "straighten" {
        straighten_hint(app, ui.ctx(), ui.available_rect_before_wrap());
    }
}

// -------------------------------------------------------------------------------- the title

/// The photo's size in the orientation it is cropped in.
fn dims(app: &LightcraftApp) -> (f64, f64) {
    let Some(p) = app.session.active().and_then(|id| app.session.catalog.photo(id)) else { return (3.0, 2.0) };
    let (w, h) = (p.width.max(1) as f64, p.height.max(1) as f64);
    if p.develop.orientation.swaps_axes() { (h, w) } else { (w, h) }
}

/// Two sides as they are written: shorter first for a tall photo, longer first for a wide one.
fn sides(short: f64, long: f64, portrait: bool) -> (f64, f64) {
    if portrait { (short, long) } else { (long, short) }
}

fn number(x: f64) -> String {
    if (x - x.round()).abs() < 1e-9 { format!("{x:.0}") } else { format!("{x}") }
}

fn same(a: (u32, u32), short: f64, long: f64) -> bool {
    let r = a.0 as f64 / a.1.max(1) as f64;
    let q = short / long;
    (r - q).abs() < 0.005 * q || (r - 1.0 / q).abs() < 0.005 / q
}

/// The simplest whole-number ratio close to `r` ("2 × 3" for 0.667), if there is one.
fn simple_ratio(r: f64) -> Option<(u32, u32)> {
    (1..=20u32).find_map(|q| {
        let p = (r * q as f64).round();
        (p >= 1.0 && (p / q as f64 - r).abs() < 0.004 * r).then_some((p as u32, q))
    })
}

/// The shape of the crop: the locked aspect, else the frame as it is (an untouched one is the
/// photo's own shape, which is "Original").
fn shape(d: &DevelopSettings, w: f64, h: f64) -> (u32, u32) {
    d.crop.aspect.unwrap_or_else(|| {
        let r = d.crop.geometry.rect_px(w, h);
        (r.width().round().max(1.0) as u32, r.height().round().max(1.0) as u32)
    })
}

/// What the photo is cropped to, for the title: "Original (2 × 3)", "4 × 5", or "Custom".
fn title(app: &LightcraftApp) -> String {
    let Some(d) = app.session.active().and_then(|id| app.session.develop_of(id)) else { return String::new() };
    let (w, h) = dims(app);
    let a = shape(&d, w, h);
    let portrait = h > w;
    if same(a, w.min(h), w.max(h)) {
        let tail = simple_ratio(w / h).map(|(p, q)| format!(" ({p} × {q})")).unwrap_or_default();
        return format!("{}{tail}", crate::i18n::tr("Original"));
    }
    let known = RATIOS.iter().chain(POSTS.iter().map(|(_, r)| r)).chain(VIDEOS.iter().map(|(_, r)| r)).find(|r| same(a, r.short, r.long));
    match known {
        Some(r) => {
            let (x, y) = sides(r.short, r.long, portrait);
            format!("{} × {}", number(x), number(y))
        }
        None => crate::i18n::tr("Custom").to_string(),
    }
}

/// Over the photo: what it is cropped to in the middle, Undo and Help on the right.
fn title_bar(app: &mut LightcraftApp, ui: &mut Ui, t: &Tokens) {
    let r = ui.max_rect();
    ui.painter().text(r.center(), Align2::CENTER_CENTER, title(app), t.font(17.0), t.text);
    let mut right = ui.new_child(UiBuilder::new().max_rect(r.shrink2(vec2(6.0, 0.0))).layout(egui::Layout::right_to_left(egui::Align::Center)));
    right.spacing_mut().item_spacing.x = 0.0;
    if compact::bar_icon(&mut right, "cropHelp", Icon::Help, "Help", true).clicked() {
        app.toast_for(ui.ctx(), crate::i18n::tr("Drag the corners or edges to crop, and the dial to straighten"), 4.0);
    }
    let can_undo = lightcraft_engine::cmd::can_undo(&app.session).is_ok();
    if compact::bar_icon(&mut right, "undo", Icon::Undo, "Undo", can_undo).clicked() {
        let _ = app.run("edit.undo", json!({}));
        haptics::tap(ui.ctx(), Haptic::Light);
    }
}

// -------------------------------------------------------------------------------- the bar

/// ✕ and ✓ with the tool's name between them.
fn bottom_bar(app: &mut LightcraftApp, ui: &mut Ui, t: &Tokens) {
    let r = ui.max_rect();
    ui.painter().text(r.center(), Align2::CENTER_CENTER, crate::i18n::tr("Crop & Rotate"), t.font(17.0), t.text);
    let mut left = ui.new_child(UiBuilder::new().max_rect(r.shrink2(vec2(6.0, 0.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
    if compact::bar_icon(&mut left, "cropCancel", Icon::Close, "Cancel", true).clicked() {
        cancel(app);
    }
    let mut right = ui.new_child(UiBuilder::new().max_rect(r.shrink2(vec2(6.0, 0.0))).layout(egui::Layout::right_to_left(egui::Align::Center)));
    if compact::bar_icon(&mut right, "cropDone", Icon::Check, "Done", true).clicked() {
        let _ = app.run("tool.done", json!({}));
    }
}

// -------------------------------------------------------------------------------- tools

/// The dial and the round buttons under the photo; held sideways (`inline`) they share one row.
fn tools(app: &mut LightcraftApp, ui: &mut Ui, t: &Tokens, inline: bool) {
    let r = ui.max_rect();
    let (dial_r, row) = if inline {
        (r, r)
    } else {
        (Rect::from_min_size(r.min, vec2(r.width(), DIAL_H)), Rect::from_min_max(pos2(r.left(), r.top() + DIAL_H), r.max))
    };
    let (margin, gap) = (14.0, 10.0);
    let y = row.center().y;
    let at = |x: f32| Rect::from_center_size(pos2(x, y), vec2(ROUND_D, ROUND_D));
    let step = ROUND_D + gap;
    let (x0, x1) = (row.left() + margin + ROUND_D / 2.0, row.right() - margin - ROUND_D / 2.0);
    let locked = app.session.active().and_then(|id| app.session.develop_of(id)).is_some_and(|d| d.crop.aspect.is_some());
    if round_button(ui, t, "cropAuto", Icon::CropAuto, "Auto", at(x0), false).clicked() {
        match app.run("crop.autoStraighten", json!({})) {
            Ok(v) if v["changed"] == false => app.toast(ui.ctx(), crate::i18n::tr("No horizon or vertical lines found")),
            Ok(_) => haptics::tap(ui.ctx(), Haptic::Light),
            Err(e) => app.toast(ui.ctx(), e),
        }
    }
    let lock = round_button(ui, t, "cropLock", if locked { Icon::Lock } else { Icon::LockOpen }, "Lock Aspect Ratio", at(x0 + step), locked);
    if lock.clicked() {
        let _ = app.run("crop.aspect", json!({"aspect": "toggle"}));
        haptics::tap(ui.ctx(), Haptic::Selection);
    }
    if round_button(ui, t, "cropRotate", Icon::Rotate, "Rotate", at(x1 - step), false).clicked() {
        let _ = app.run("photo.rotateLeft", json!({}));
    }
    let more = round_button(ui, t, "cropMore", Icon::Dots, "More", at(x1), false);
    if more.clicked() {
        mobile::open_menu(ui.ctx(), "cropMore", more.rect);
    }
    // (the dial takes what is between the buttons)
    let between = if inline {
        Rect::from_min_max(pos2(x0 + step + ROUND_D / 2.0 + gap, r.top()), pos2(x1 - step - ROUND_D / 2.0 - gap, r.bottom()))
    } else {
        dial_r
    };
    dial(app, ui, t, between);
}

/// A round button of the tools row (`icon:<id>`): dark, with its glyph white when `lit`.
fn round_button(ui: &mut Ui, t: &Tokens, id: &str, icon: Icon, tip: &str, at: Rect, lit: bool) -> egui::Response {
    let resp = ui.interact(at, Id::new(("crop-round", id)), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, crate::i18n::tr(tip)));
    register(ui.ctx(), format!("icon:{id}"), at);
    let k = ui.ctx().animate_bool_with_time(resp.id.with("press"), resp.is_pointer_button_down_on(), 0.1);
    let c = at.center();
    ui.painter().circle(c, ROUND_D / 2.0 * (1.0 - 0.06 * k), crate::widgets::lerp(t.chrome, t.pressed, k), Stroke::new(1.0, t.button_border));
    icons::paint(ui.painter(), Rect::from_center_size(c, vec2(22.0, 22.0)), icon, if lit { t.text } else { t.text_label });
    resp
}

/// The angle dial: a ruler along an arc that a sideways drag slides under the middle mark. The
/// angle it reads is the crop's (`crop.straighten`); a drag is one undo step.
fn dial(app: &mut LightcraftApp, ui: &mut Ui, t: &Tokens, r: Rect) {
    let Some(id) = app.session.active() else { return };
    let angle_now = |app: &LightcraftApp| app.session.develop_of(id).map_or(0.0, |d| d.crop.geometry.angle);
    let area = Rect::from_center_size(r.center(), vec2(r.width().min(260.0), r.height()));
    let resp = ui.interact(area.expand2(vec2(24.0, 0.0)), Id::new("crop-dial"), Sense::drag());
    let before = angle_now(app);
    resp.widget_info(|| egui::WidgetInfo::slider(true, before, crate::i18n::tr("Straighten")));
    register(ui.ctx(), "dial:angle", area);
    let start = Id::new("crop-dial-start");
    if resp.drag_started() {
        let _ = app.run("develop.beginInteraction", json!({"label": "Straighten"}));
        ui.ctx().data_mut(|d| d.insert_temp(start, before));
    }
    if resp.dragged()
        && let Some(from) = ui.ctx().data(|d| d.get_temp::<f64>(start))
        && let Some((a, b)) = ui.input(|i| i.pointer.press_origin()).zip(resp.interact_pointer_pos())
    {
        // the ruler follows the finger: dragging right reads lower
        let now = (from - (b.x - a.x) as f64 / PPD as f64).clamp(-45.0, 45.0);
        let now = (now * 20.0).round() / 20.0;
        if (now - before).abs() > 1e-9 {
            if now.floor() != before.floor() {
                haptics::tap(ui.ctx(), Haptic::Selection);
            }
            let _ = app.run("crop.straighten", json!({"angle": now}));
        }
    }
    if resp.drag_stopped() {
        let _ = app.run("develop.endInteraction", json!({}));
    }
    let angle = angle_now(app) as f32;
    let p = ui.painter();
    let c = pos2(area.center().x, area.bottom() - 4.0);
    let half = area.width() / 2.0;
    if half < 1.0 {
        return;
    }
    // (a point of the arc, and the way in from it, for the ruler's value `k`)
    let at = |k: f32| {
        let x = (k - angle) * PPD;
        let sag = DIAL_R - (DIAL_R * DIAL_R - x * x).max(0.0).sqrt();
        (x, pos2(c.x + x, c.y - sag), vec2(-x, sag - DIAL_R) / DIAL_R)
    };
    for k in ((angle - half / PPD).floor() as i32)..=((angle + half / PPD).ceil() as i32) {
        let (x, pt, inward) = at(k as f32);
        if x.abs() > half {
            continue;
        }
        let fade = 1.0 - (x.abs() / half).powi(2);
        let color = t.text.gamma_multiply(fade);
        if k % 5 == 0 {
            p.line_segment([pt, pt + inward * 9.0], Stroke::new(1.5, color));
        } else {
            p.circle_filled(pt, 1.1, color.gamma_multiply(0.8));
        }
    }
    // the mark the ruler is read at
    p.line_segment([c, c + vec2(0.0, -14.0)], Stroke::new(2.0, t.text));
    let a = f64::from(angle);
    let text = if (a - a.round()).abs() < 0.05 { format!("{:.0}°", a.round() + 0.0) } else { format!("{a:.1}°") };
    p.text(pos2(c.x, c.y - 26.0), Align2::CENTER_CENTER, text, t.font(13.0), t.text);
}

// -------------------------------------------------------------------------------- the tabs

/// The two tabs and the chosen one's content.
fn tabs_panel(app: &mut LightcraftApp, ui: &mut Ui, t: &Tokens) {
    let r = ui.max_rect();
    let labels = [("aspect", "Aspect"), ("geometry", "Geometry")];
    let gap = 48.0;
    let layout =
        |label: &str, on: bool| ui.painter().layout_no_wrap(crate::i18n::tr(label).to_string(), t.font(17.0), if on { t.text } else { t.text_dim });
    let widths: Vec<f32> = labels.iter().enumerate().map(|(i, (_, l))| layout(l, app.ui.crop_geometry == (i == 1)).size().x).collect();
    let mut x = r.center().x - (widths.iter().sum::<f32>() + gap) / 2.0;
    for (i, ((key, label), w)) in labels.iter().zip(widths).enumerate() {
        let on = app.ui.crop_geometry == (i == 1);
        let cell = Rect::from_min_size(pos2(x - 12.0, r.top()), vec2(w + 24.0, TABS_H));
        let resp = ui.interact(cell, Id::new(("crop-tab", *key)), Sense::click());
        resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, crate::i18n::tr(label)));
        register(ui.ctx(), format!("button:cropTab-{key}"), cell);
        let galley = layout(label, on);
        ui.painter().galley(pos2(x, cell.center().y - galley.size().y / 2.0), galley, t.text);
        if on {
            ui.painter().rect_filled(Rect::from_min_size(pos2(x, cell.bottom() - 4.0), vec2(w, 2.0)), 1.0, t.text);
        }
        if resp.clicked() && !on {
            app.ui.crop_geometry = i == 1;
            app.ui.tool.clear();
            haptics::tap(ui.ctx(), Haptic::Selection);
        }
        x += w + gap;
    }
    let body = Rect::from_min_max(pos2(r.left(), r.top() + TABS_H), r.max);
    let column = Rect::from_center_size(body.center(), vec2(body.width().min(MAX_CONTENT_W), body.height()));
    let mut content = ui.new_child(UiBuilder::new().max_rect(column));
    if app.ui.crop_geometry { geometry_tab(app, &mut content, t) } else { aspect_tab(app, &mut content, t) }
}

/// A tile of the Aspect tab (`button:<id>`): its icon over its name, on a grey tile when `on`.
fn tile(ui: &mut Ui, t: &Tokens, id: &str, icon: Icon, label: &str, on: bool) -> egui::Response {
    let label = crate::i18n::tr(label);
    let (r, resp) = ui.allocate_exact_size(vec2(TILE_W, 72.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, label));
    register(ui.ctx(), format!("button:{id}"), r);
    let k = ui.ctx().animate_bool_with_time(resp.id.with("on"), on, 0.15);
    if k > 0.0 {
        ui.painter().rect_filled(r, 10.0, t.tool_active.gamma_multiply(k));
    }
    let color = if on || resp.is_pointer_button_down_on() { t.text } else { t.text_label };
    icons::paint(ui.painter(), Rect::from_center_size(r.center() - vec2(0.0, 11.0), vec2(26.0, 26.0)), icon, color);
    ui.painter().text(r.center() + vec2(0.0, 20.0), Align2::CENTER_CENTER, label, t.font(12.0), color);
    resp
}

/// Original, then Ratios, Instagram and TikTok, which open a menu of their formats.
fn aspect_tab(app: &mut LightcraftApp, ui: &mut Ui, t: &Tokens) {
    let Some(d) = app.session.active().and_then(|id| app.session.develop_of(id)) else { return };
    let (w, h) = dims(app);
    let aspect = shape(&d, w, h);
    let is = |r: &Ratio| same(aspect, r.short, r.long);
    let original = same(aspect, w.min(h), w.max(h));
    let ratios = !original && RATIOS.iter().any(is);
    let post = !original && !ratios && POSTS.iter().any(|(_, r)| is(r));
    let video = !original && !ratios && !post && VIDEOS.iter().any(|(_, r)| is(r));
    ui.add_space(6.0);
    egui::ScrollArea::horizontal().id_salt("crop-aspect-row").scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(
        ui,
        |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.add_space(10.0);
                if tile(ui, t, "cropOriginal", Icon::Photos, "Original", original).clicked() {
                    let _ = app.run("crop.aspect", json!({"aspect": "original"}));
                }
                let (sep, _) = ui.allocate_exact_size(vec2(9.0, 40.0), Sense::hover());
                ui.painter().vline(sep.center().x, sep.y_range(), Stroke::new(1.0, t.text_disabled));
                for (id, icon, label, on) in [
                    ("cropRatios", Icon::Ratios, "Ratios", ratios),
                    ("cropInstagram", Icon::PostFrame, "Instagram", post),
                    ("cropTikTok", Icon::VideoFrame, "TikTok", video),
                ] {
                    let resp = tile(ui, t, id, icon, label, on);
                    if resp.clicked() {
                        mobile::open_menu(ui.ctx(), id, resp.rect);
                    }
                }
                ui.add_space(10.0);
            });
        },
    );
}

/// The menus of the Aspect tab and of the "…" button; called every frame, each draws only while
/// open.
pub fn menus(app: &mut LightcraftApp, ctx: &egui::Context) {
    let (w, h) = dims(app);
    let portrait = h > w;
    let current = app.session.active().and_then(|id| app.session.develop_of(id)).map(|d| shape(&d, w, h));
    let rows = |app: &mut LightcraftApp, ui: &mut Ui, list: &[(Option<&str>, &Ratio)]| {
        for (name, r) in list {
            let (x, y) = sides(r.short, r.long, portrait);
            let numbers = format!("{} × {}", number(x), number(y));
            let label = match name {
                Some(n) => format!("{} · {numbers}", crate::i18n::tr(n)),
                None => numbers,
            };
            let on = current.is_some_and(|a| same(a, r.short, r.long));
            if ratio_row(ui, &format!("cropAspect-{}", r.key), &label, x, y, on) {
                let _ = app.run("crop.aspect", json!({"aspect": r.key}));
            }
        }
    };
    mobile::actions(ctx, "cropRatios", None, |ui| rows(app, ui, &RATIOS.iter().map(|r| (None, r)).collect::<Vec<(Option<&str>, &Ratio)>>()));
    mobile::actions(ctx, "cropInstagram", None, |ui| {
        rows(app, ui, &POSTS.iter().map(|(n, r)| (Some(*n), r)).collect::<Vec<(Option<&str>, &Ratio)>>())
    });
    mobile::actions(ctx, "cropTikTok", None, |ui| rows(app, ui, &VIDEOS.iter().map(|(n, r)| (Some(*n), r)).collect::<Vec<(Option<&str>, &Ratio)>>()));
    mobile::actions(ctx, "cropMore", None, |ui| {
        if mobile::row(ui, "cropStraighten", Some(Icon::Straighten), crate::i18n::tr("Straighten"), true) {
            app.ui.tool = "straighten".into();
        }
        if mobile::row(ui, "cropFlipV", Some(Icon::FlipV), crate::i18n::tr("Flip Vertical"), true) {
            let _ = app.run("photo.flipVertical", json!({}));
        }
        if mobile::row(ui, "cropFlipH", Some(Icon::Flip), crate::i18n::tr("Flip Horizontal"), true) {
            let _ = app.run("photo.flipHorizontal", json!({}));
        }
        mobile::row_gap(ui);
        if mobile::row(ui, "cropReset", Some(Icon::Undo), crate::i18n::tr("Reset Crop"), true) {
            let _ = app.run("crop.reset", json!({}));
        }
    });
}

/// A row of a ratio menu (`button:<id>`): the shape of the crop, its name, a check when it's the
/// crop's. True when tapped.
fn ratio_row(ui: &mut Ui, id: &str, label: &str, w: f64, h: f64, on: bool) -> bool {
    let t = Tokens::get(ui.ctx());
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 46.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, label));
    register(ui.ctx(), format!("button:{id}"), r);
    let p = ui.painter();
    if resp.is_pointer_button_down_on() {
        p.rect_filled(r, 0.0, t.hover);
    }
    // the shape: its longer side 22 pt, in the orientation the crop will have
    let k = 22.0 / w.max(h).max(1e-3) as f32;
    let shape = Rect::from_center_size(pos2(r.left() + 32.0, r.center().y), vec2(w as f32 * k, h as f32 * k));
    p.rect_stroke(shape, 3.0, Stroke::new(1.5, t.text), StrokeKind::Inside);
    p.text(pos2(r.left() + 60.0, r.center().y), Align2::LEFT_CENTER, label, t.font(17.0), t.text);
    if on {
        icons::paint(p, Rect::from_center_size(pos2(r.right() - 26.0, r.center().y), vec2(16.0, 16.0)), Icon::Check, t.text);
    }
    p.hline(r.x_range(), r.bottom() - 0.5, Stroke::new(0.5, t.divider));
    let clicked = resp.clicked();
    if clicked {
        mobile::close_actions(ui.ctx());
    }
    clicked
}

/// A square button of the Geometry tab (`button:<id>`): its icon in a bordered box, its name under it.
fn square_tile(ui: &mut Ui, t: &Tokens, id: &str, icon: Icon, label: &str, on: bool) -> egui::Response {
    let label = crate::i18n::tr(label);
    let (r, resp) = ui.allocate_exact_size(vec2(54.0, 76.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, label));
    register(ui.ctx(), format!("button:{id}"), r);
    let b = Rect::from_min_size(r.min, vec2(54.0, 54.0));
    let down = resp.is_pointer_button_down_on();
    ui.painter().rect(
        b,
        8.0,
        if on {
            t.tool_active
        } else if down {
            t.pressed
        } else {
            t.canvas
        },
        Stroke::new(1.0, if on { t.text_dim } else { t.button_border }),
        StrokeKind::Inside,
    );
    icons::paint(ui.painter(), Rect::from_center_size(b.center(), vec2(24.0, 24.0)), icon, if on { t.text } else { t.text_label });
    ui.painter().text(pos2(r.center().x, b.bottom() + 12.0), Align2::CENTER_CENTER, label, t.font(12.0), if on { t.text } else { t.text_dim });
    resp
}

/// Upright's five modes, then the lens and perspective sliders; scrolls.
fn geometry_tab(app: &mut LightcraftApp, ui: &mut Ui, t: &Tokens) {
    let Some(id) = app.session.active() else { return };
    let d: DevelopSettings = (*app.session.develop_of(id).unwrap_or_default()).clone();
    egui::ScrollArea::vertical().id_salt("crop-geometry").auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.spacing_mut().interact_size.y = 36.0;
        ui.add_space(8.0);
        let modes = [
            ("Auto", "auto", Upright::Auto, Icon::UprightAuto),
            ("Level", "level", Upright::Level, Icon::UprightLevel),
            ("Vertical", "vertical", Upright::Vertical, Icon::UprightVertical),
            ("Full", "full", Upright::Full, Icon::UprightFull),
            ("Guided", "guided", Upright::Guided, Icon::UprightGuided),
        ];
        ui.horizontal(|ui| {
            let spare = (ui.available_width() - 32.0 - 54.0 * modes.len() as f32) / (modes.len() - 1) as f32;
            ui.spacing_mut().item_spacing.x = spare.clamp(6.0, 16.0);
            ui.add_space(16.0);
            for (label, key, mode, icon) in modes {
                let on = d.geometry.upright == mode;
                if square_tile(ui, t, &format!("upright-{key}"), icon, label, on).clicked() {
                    // (tapping the one that is on turns Upright off)
                    let next = if on { "off" } else { key };
                    let _ = app.run("geometry.upright", json!({"mode": next}));
                    app.ui.tool = if mode == Upright::Guided && !on { "guidedUpright".into() } else { String::new() };
                }
            }
        });
        ui.add_space(6.0);
        if d.geometry.upright == Upright::Guided {
            let pad = crate::widgets::side_pad(ui.ctx()).0;
            ui.horizontal(|ui| {
                ui.add_space(pad);
                ui.label(
                    egui::RichText::new(crate::i18n::tr_format!(
                        "{} of 4 guides — drag along lines that should be vertical or horizontal.",
                        d.geometry.guides.len()
                    ))
                    .size(11.0)
                    .color(t.text_dim),
                );
            });
            if !d.geometry.guides.is_empty() && crate::widgets::text_button(ui, "uprightClear", crate::i18n::tr("Clear Guides"), false).clicked() {
                let _ = app.run("geometry.guides", json!({"guides": []}));
            }
        }
        super::edit::control(app, ui, &d, "optics.distortion", true);
        for spec in lightcraft_develop::controls::in_section(lightcraft_develop::Section::Geometry).filter(|c| c.id != "crop.angle") {
            super::edit::control(app, ui, &d, spec.id, true);
        }
        ui.add_space(4.0);
        let mut constrain = d.geometry.constrain_crop;
        if crate::widgets::check(ui, &mut constrain, crate::i18n::tr("Constrain Crop")).changed() {
            let _ = app.run("develop.merge", json!({"settings": {"geometry": {"constrain_crop": constrain}}, "label": "Constrain Crop"}));
        }
        ui.add_space(12.0);
    });
}

/// While the Straighten line is being drawn: a pill over the photo saying how, which cancels it.
fn straighten_hint(app: &mut LightcraftApp, ctx: &egui::Context, canvas: Rect) {
    let t = Tokens::get(ctx);
    egui::Area::new(Id::new("crop-straighten-hint"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos2(canvas.center().x, canvas.top() + 12.0))
        .pivot(Align2::CENTER_TOP)
        .show(ctx, |ui| {
            let galley = ui.painter().layout_no_wrap(crate::i18n::tr("Drag along the horizon").to_string(), t.font(14.0), Color32::WHITE);
            let (r, resp) = ui.allocate_exact_size(galley.size() + vec2(56.0, 18.0), Sense::click());
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, crate::i18n::tr("Cancel")));
            register(ctx, "button:straightenCancel", r);
            ui.painter().rect_filled(r, r.height() / 2.0, Color32::from_black_alpha(190));
            ui.painter().galley(pos2(r.left() + 16.0, r.center().y - galley.size().y / 2.0), galley, Color32::WHITE);
            icons::paint(ui.painter(), Rect::from_center_size(pos2(r.right() - 20.0, r.center().y), vec2(14.0, 14.0)), Icon::Close, Color32::WHITE);
            if resp.clicked() {
                app.ui.tool.clear();
                app.gesture = None;
            }
        });
}
