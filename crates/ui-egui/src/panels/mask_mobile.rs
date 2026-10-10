//! The Masking screen of the compact (phone) layout, modelled on Lightroom's mobile app. While the
//! Masking tool is open it takes the screen over from the tool bar and the tool sheet: the photo,
//! with the masks in a column over its right edge — a + that opens Create New Mask (a list of the
//! kinds), a tile per mask, the selected mask's parts under it and a ± that adds to it or takes
//! away from it — and, while the brush is in hand, the brush's tools in a column over its left edge
//! (paint, erase, size, feather, flow, auto mask). Under the photo, the selected mask's adjustments
//! in groups (Mask, Light, Color, Effects, Detail, Optics), the chosen group's sliders in a sheet
//! above them, and a bar with ✕, "Masking" and ✓. Tapping a mask's pin on the photo, or its tile,
//! opens its options. Edits apply live: ✓ keeps them, ✕ undoes every step taken since the screen
//! opened. The drawing and the words are our own. Held sideways, the title, the groups, the sheet
//! and the bar are one column on the right.

use egui::{Align2, Color32, Id, Rect, Sense, Stroke, StrokeKind, Ui, UiBuilder, pos2, vec2};
use lightcraft_develop::{Mask, MaskOp, MaskShape};
use serde_json::json;

use super::masking::{self, LOCAL};
use super::{compact, mobile};
use crate::LightcraftApp;
use crate::glass;
use crate::haptics::{self, Haptic};
use crate::icons::{self, Icon};
use crate::state::{RightPanel, ViewMode};
use crate::theme::Tokens;
use crate::widgets::{TOUCH_SLIDER_ROW_H, register};

/// The title row over the photo, the bar of ✕ and ✓ under everything, and the groups' row.
const TITLE_H: f32 = 44.0;
const BAR_H: f32 = 50.0;
const GROUP_H: f32 = 60.0;
/// The column held sideways.
const SIDE_W: f32 = 316.0;
/// The floating columns over the photo: their width, a mask's tile and a part's tile.
const COL_W: f32 = 56.0;
const MASK_TILE: f32 = 44.0;
const PART_TILE: f32 = 36.0;

/// The adjustment groups: (id, icon, name, the `LocalAdjustments` sliders in it). "Mask" is the
/// mask's own settings (amount, invert, its parts' ranges, refine edges, the overlay).
const GROUPS: [(&str, Icon, &str, &[&str]); 6] = [
    ("mask", Icon::Mask, "Mask", &[]),
    ("light", Icon::Sun, "Light", &["exposure", "contrast", "highlights", "shadows", "whites", "blacks"]),
    ("color", Icon::Drop, "Color", &["temp", "tint", "hue", "saturation"]),
    ("effects", Icon::Vignette, "Effects", &["texture", "clarity", "dehaze"]),
    ("detail", Icon::Detail, "Detail", &["sharpness", "noise", "moire"]),
    ("optics", Icon::Lens, "Optics", &["defringe"]),
];

/// Create New Mask, as Lightroom's mobile app lists it: (kind, icon, name); `None` = a gap.
const NEW_KINDS: [Option<(&str, Icon, &str)>; 12] = [
    Some(("subject", Icon::Subject, "Select Subject")),
    Some(("sky", Icon::Sky, "Select Sky")),
    Some(("background", Icon::Subject, "Select Background")),
    Some(("object", Icon::Target, "Select Object")),
    Some(("prompt", Icon::Pencil, "Describe…")),
    None,
    Some(("brush", Icon::Brush, "Brush")),
    Some(("linear", Icon::Linear, "Linear Gradient")),
    Some(("radial", Icon::Radial, "Radial Gradient")),
    None,
    Some(("colorRange", Icon::Picker, "Color Range")),
    Some(("luminanceRange", Icon::Sliders, "Luminance Range")),
];

/// Whether the compact layout shows the Masking screen: a photo with the Masking tool open.
pub fn open(app: &LightcraftApp) -> bool {
    app.compact && app.ui.view == ViewMode::Detail && !app.ui.presets && app.ui.right == RightPanel::Masking && app.session.active().is_some()
}

/// The selected mask of the active photo.
fn selected(app: &LightcraftApp) -> Option<Mask> {
    let mid = app.session.active_mask?;
    let d = app.session.active().and_then(|id| app.session.develop_of(id))?;
    d.masks.iter().find(|m| m.id == mid).cloned()
}

/// The masks of the active photo.
fn masks(app: &LightcraftApp) -> Vec<Mask> {
    app.session.active().and_then(|id| app.session.develop_of(id)).map(|d| d.masks.clone()).unwrap_or_default()
}

/// The screen's panels: the title, and under the photo the bar, the groups and the chosen group's
/// sheet. The photo takes what is left (`compact::content`); the columns over it are [`floating`].
pub fn show(app: &mut LightcraftApp, ui: &mut Ui, t: &Tokens, landscape: bool) {
    let mask = selected(app);
    if mask.is_none() && app.ui.describe.is_none() {
        app.ui.mask_group.clear();
    }
    let frame = |fill: Color32| egui::Frame::NONE.fill(fill);
    if landscape {
        egui::Panel::right("mask_side").resizable(false).exact_size(SIDE_W).frame(frame(t.chrome)).show(ui, |ui| {
            let full = ui.max_rect();
            let part = |top: f32, bottom: f32| Rect::from_min_max(pos2(full.left(), top), pos2(full.right(), bottom));
            let mut title = ui.new_child(UiBuilder::new().max_rect(part(full.top(), full.top() + TITLE_H)));
            title_bar(app, &mut title, t);
            let mut groups = ui.new_child(UiBuilder::new().max_rect(part(full.top() + TITLE_H, full.top() + TITLE_H + GROUP_H)));
            group_row(app, &mut groups, t, mask.as_ref());
            let mut body = ui.new_child(UiBuilder::new().max_rect(part(full.top() + TITLE_H + GROUP_H, full.bottom() - BAR_H)));
            egui::ScrollArea::vertical().id_salt("mask-side").show(&mut body, |ui| sheet_body(app, ui, mask.as_ref()));
            let mut bar = ui.new_child(UiBuilder::new().max_rect(part(full.bottom() - BAR_H, full.bottom())));
            bottom_bar(app, &mut bar, t);
        });
        return;
    }
    egui::Panel::top("mask_top").show_separator_line(false).exact_size(TITLE_H).frame(frame(t.canvas)).show(ui, |ui| title_bar(app, ui, t));
    egui::Panel::bottom("mask_bar").show_separator_line(false).exact_size(BAR_H).frame(frame(t.chrome)).show(ui, |ui| bottom_bar(app, ui, t));
    egui::Panel::bottom("mask_groups").show_separator_line(false).exact_size(GROUP_H).frame(frame(t.chrome)).show(ui, |ui| {
        group_row(app, ui, t, mask.as_ref());
    });
    let open = !app.ui.mask_group.is_empty() || app.ui.describe.is_some();
    if open {
        // as tall as what it holds, up to two fifths of the room (then it scrolls)
        let avail = ui.available_rect_before_wrap().height();
        let fit_id = Id::new(("mask-sheet-fit", app.ui.mask_group.clone()));
        let fit = ui.ctx().data(|d| d.get_temp::<f32>(fit_id)).unwrap_or(3.0 * TOUCH_SLIDER_ROW_H);
        let h = (fit + 12.0).min(avail * 0.4).max(TOUCH_SLIDER_ROW_H.min(avail));
        let frame = frame(t.chrome).corner_radius(egui::CornerRadius { nw: 18, ne: 18, sw: 0, se: 0 });
        egui::Panel::bottom("mask_sheet").show_separator_line(false).exact_size(h).frame(frame).show(ui, |ui| {
            ui.add_space(12.0);
            let out =
                egui::ScrollArea::vertical().id_salt(("mask-sheet", app.ui.mask_group.clone())).show(ui, |ui| sheet_body(app, ui, mask.as_ref()));
            ui.ctx().data_mut(|d| d.insert_temp(fit_id, out.content_size.y));
        });
    }
}

// -------------------------------------------------------------------------------- the title

/// Over the photo: what the AI masks are doing in the middle (while they work); Undo, the overlay's
/// eye and "…" on the right.
fn title_bar(app: &mut LightcraftApp, ui: &mut Ui, t: &Tokens) {
    let r = ui.max_rect();
    let seg = &app.session.segmenter;
    let status = if seg.analyzing() {
        Some("Analyzing the photo…")
    } else if seg.busy() {
        Some("Selecting…")
    } else if seg.detail_busy() || app.ui.detail_due.is_some() {
        Some("Refining the mask…")
    } else {
        None
    };
    if let Some(status) = status {
        ui.painter().text(r.center(), Align2::CENTER_CENTER, crate::i18n::tr(status), t.font(14.0), t.text_dim);
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
    }
    let mut right = ui.new_child(UiBuilder::new().max_rect(r.shrink2(vec2(6.0, 0.0))).layout(egui::Layout::right_to_left(egui::Align::Center)));
    right.spacing_mut().item_spacing.x = 0.0;
    let more = compact::bar_icon(&mut right, "maskMore", Icon::Dots, "More", true);
    if more.clicked() {
        mobile::open_menu(ui.ctx(), "maskMore", more.rect);
    }
    let on = app.ui.mask_overlay;
    let eye = compact::bar_icon(&mut right, "maskOverlay", if on { Icon::Eye } else { Icon::EyeOff }, "Show Overlay", true);
    if eye.clicked() {
        app.ui.mask_overlay = !on;
        haptics::tap(ui.ctx(), Haptic::Selection);
    }
    let can_undo = lightcraft_engine::cmd::can_undo(&app.session).is_ok();
    if compact::bar_icon(&mut right, "undo", Icon::Undo, "Undo", can_undo).clicked() {
        let _ = app.run("edit.undo", json!({}));
        haptics::tap(ui.ctx(), Haptic::Light);
    }
}

// -------------------------------------------------------------------------------- the bar

/// ✕ and ✓ with "Masking" between them.
fn bottom_bar(app: &mut LightcraftApp, ui: &mut Ui, t: &Tokens) {
    let r = ui.max_rect();
    ui.painter().text(r.center(), Align2::CENTER_CENTER, crate::i18n::tr("Masking"), t.font(17.0), t.text);
    let mut left = ui.new_child(UiBuilder::new().max_rect(r.shrink2(vec2(6.0, 0.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
    if compact::bar_icon(&mut left, "maskCancel", Icon::Close, "Cancel", true).clicked() {
        app.ui.describe = None;
        compact::cancel_screen(app);
    }
    let mut right = ui.new_child(UiBuilder::new().max_rect(r.shrink2(vec2(6.0, 0.0))).layout(egui::Layout::right_to_left(egui::Align::Center)));
    if glass::circle_button(&mut right, "icon:maskDone", Icon::Check, "Done", true, Some(t.accent)).clicked() {
        app.ui.describe = None;
        let _ = app.run("tool.done", json!({}));
    }
}

// -------------------------------------------------------------------------------- the groups

/// The adjustment groups in a row (icon over name, `button:maskGroup-<id>`), dimmed until a mask is
/// selected; tapping one opens its sheet, tapping it again closes it.
fn group_row(app: &mut LightcraftApp, ui: &mut Ui, t: &Tokens, mask: Option<&Mask>) {
    let r = ui.max_rect();
    let w = (r.width() / GROUPS.len() as f32).min(72.0);
    let x0 = r.center().x - w * GROUPS.len() as f32 / 2.0;
    let enabled = mask.is_some();
    for (i, (id, icon, label, _)) in GROUPS.into_iter().enumerate() {
        let cell = Rect::from_min_size(pos2(x0 + i as f32 * w, r.top()), vec2(w, r.height()));
        let on = enabled && app.ui.mask_group == id;
        let resp = ui.interact(cell, Id::new(("mask-group", id)), if enabled { Sense::click() } else { Sense::hover() });
        resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, enabled, on, crate::i18n::tr(label)));
        register(ui.ctx(), format!("button:maskGroup-{id}"), cell);
        let k = ui.ctx().animate_bool_with_time(resp.id.with("on"), on, 0.15);
        if k > 0.0 {
            let pill = cell.shrink2(vec2(3.0, 7.0));
            let ink = if Tokens::is_dark(ui.ctx()) { Color32::from_white_alpha(30) } else { Color32::from_black_alpha(14) };
            ui.painter().rect_filled(pill, 14.0, ink.gamma_multiply(k));
        }
        let color = if !enabled {
            t.text_disabled
        } else if on || resp.is_pointer_button_down_on() {
            t.text
        } else {
            t.text_dim
        };
        icons::paint(ui.painter(), Rect::from_center_size(cell.center() - vec2(0.0, 8.0), vec2(20.0, 20.0)), icon, color);
        ui.painter().text(cell.center() + vec2(0.0, 13.0), Align2::CENTER_CENTER, crate::i18n::tr(label), t.font(11.0), color);
        if resp.clicked() {
            haptics::tap(ui.ctx(), Haptic::Selection);
            app.ui.mask_group = if on { String::new() } else { id.to_string() };
        }
    }
}

/// The sheet's content: the Describe field while it is open, else the chosen group's sliders for
/// the selected mask.
fn sheet_body(app: &mut LightcraftApp, ui: &mut Ui, mask: Option<&Mask>) {
    ui.spacing_mut().item_spacing.y = 0.0;
    ui.spacing_mut().interact_size.y = 36.0;
    if app.ui.describe.is_some() {
        egui::Frame::NONE.inner_margin(crate::widgets::margin(ui.ctx(), 0, 8)).show(ui, |ui| {
            masking::describe_field(app, ui, true);
            masking::describe_field(app, ui, false);
        });
        return;
    }
    let Some(m) = mask else { return };
    match app.ui.mask_group.as_str() {
        "mask" => {
            masking::amount_slider(app, ui, m);
            egui::Frame::NONE.inner_margin(crate::widgets::margin(ui.ctx(), 2, 2)).show(ui, |ui| {
                let mut inverted = m.invert;
                if crate::widgets::check(ui, &mut inverted, crate::i18n::tr("Invert")).changed() {
                    let _ = app.run("mask.invert", json!({"id": m.id}));
                }
            });
            for (i, c) in m.components.iter().enumerate() {
                egui::Frame::NONE.inner_margin(crate::widgets::margin(ui.ctx(), 0, 0)).show(ui, |ui| masking::range_controls(app, ui, i, &c.shape));
            }
            masking::refine_slider(app, ui, m);
            egui::Frame::NONE.inner_margin(crate::widgets::margin(ui.ctx(), 6, 8)).show(ui, |ui| masking::overlay_options(app, ui));
        }
        group => {
            let ids = GROUPS.iter().find(|g| g.0 == group).map_or(&[][..], |g| g.3);
            for s in LOCAL.iter().filter(|s| ids.contains(&s.id)) {
                let v = masking::local_get(&m.adjust, s.id);
                let out = crate::widgets::slider(ui, s, v, true, None);
                super::edit::apply_slider_out(app, s, out, |app, v| app.run("mask.adjust", json!({"values": {s.id: v}})));
            }
        }
    }
}

// -------------------------------------------------------------------------------- over the photo

/// The columns over the photo (after it is drawn, so they're over it): the masks on the right, the
/// brush's tools on the left while it is in hand.
pub fn floating(app: &mut LightcraftApp, ctx: &egui::Context) {
    let Some(canvas) = app.canvas_rect else { return };
    if canvas.height() < 120.0 {
        return;
    }
    mask_column(app, ctx, canvas);
    if app.ui.tool == "brush" {
        brush_column(app, ctx, canvas);
    }
}

/// A column of glass over the photo at `at` (its top left), holding `add`.
// ponytail: no scrolling (a ScrollArea in an auto-sized Area keeps its first height); a photo with
// more masks than fit runs the column off the photo's bottom
fn column(ctx: &egui::Context, id: &str, at: egui::Pos2, add: impl FnOnce(&mut Ui)) {
    egui::Area::new(Id::new(id)).order(egui::Order::Middle).fixed_pos(at).show(ctx, |ui| {
        let under = ui.painter().add(egui::Shape::Noop);
        let inner = egui::Frame::NONE.inner_margin(egui::Margin::same(6)).show(ui, |ui| {
            ui.set_width(COL_W - 12.0);
            ui.spacing_mut().item_spacing = vec2(0.0, 8.0);
            add(ui)
        });
        let r = inner.response.rect;
        ui.painter().set(under, glass::shape(ui.ctx(), r, 16.0, Some(glass::menu_fill(ui.ctx())), 0.0));
    });
}

/// The masks over the photo's right edge: + (Create New Mask), a tile per mask — tap one to select
/// it, the selected one for its options — and under the selected one its parts and a ±.
fn mask_column(app: &mut LightcraftApp, ctx: &egui::Context, canvas: Rect) {
    let t = Tokens::get(ctx);
    let all = masks(app);
    let active = app.session.active_mask;
    let at = pos2(canvas.right() - COL_W - 8.0, canvas.top() + 8.0);
    column(ctx, "mask-column", at, |ui| {
        let (r, resp) = ui.allocate_exact_size(vec2(MASK_TILE, MASK_TILE), Sense::click());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, crate::i18n::tr("Create New Mask")));
        register(ctx, "button:maskNew", r);
        let k = glass::press(ctx, &resp);
        ui.painter().circle_filled(r.center(), MASK_TILE / 2.0 - 2.0 + 2.0 * k, t.accent);
        icons::paint(ui.painter(), Rect::from_center_size(r.center(), vec2(22.0, 22.0)), Icon::Plus, Color32::WHITE);
        if resp.clicked() {
            haptics::tap(ctx, Haptic::Selection);
            mobile::open_actions(ctx, "maskNew");
        }
        for m in &all {
            let sel = active == Some(m.id);
            let icon = m.components.first().map_or(Icon::Mask, |c| masking::kind_label(&c.shape).1);
            let resp = tile(ui, &t, &format!("mask:{}", m.id), &m.name, icon, MASK_TILE, sel, m.visible, None);
            if resp.clicked() {
                haptics::tap(ctx, Haptic::Selection);
                if sel {
                    mobile::open_menu(ctx, "maskPin", resp.rect);
                } else {
                    let _ = app.run("mask.select", json!({"id": m.id}));
                    // the brush stays in hand for a mask that is painted
                    let painted = m.components.iter().any(|c| matches!(c.shape, MaskShape::Brush { .. }));
                    if !painted || app.ui.tool != "brush" {
                        app.ui.tool.clear();
                    }
                }
            }
            if !sel {
                continue;
            }
            // its parts, then ± (add to it or take away from it)
            for (k, c) in m.components.iter().enumerate() {
                let (kind, icon) = masking::kind_label(&c.shape);
                let badge = match c.op {
                    MaskOp::Add => None,
                    MaskOp::Subtract => Some("−"),
                    MaskOp::Intersect => Some("∩"),
                };
                let name = c.name.clone().unwrap_or_else(|| crate::i18n::tr(kind).to_string());
                let resp = tile(ui, &t, &format!("component:{k}"), &name, icon, PART_TILE, false, true, badge);
                if resp.clicked() {
                    haptics::tap(ctx, Haptic::Selection);
                    ctx.data_mut(|d| d.insert_temp(Id::new("mask-comp-menu"), k));
                    // a painted part puts the brush in hand
                    if matches!(c.shape, MaskShape::Brush { .. }) && app.ui.tool != "brush" {
                        app.ui.tool = "brush".into();
                        app.ui.brush_erase = c.op == MaskOp::Subtract;
                    } else {
                        mobile::open_menu(ctx, "maskComp", resp.rect);
                    }
                }
            }
            let (r, resp) = ui.allocate_exact_size(vec2(MASK_TILE, PART_TILE), Sense::click());
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, crate::i18n::tr("Add or Subtract")));
            register(ctx, "button:maskCombine", r);
            let k = glass::press(ctx, &resp);
            let c = r.center();
            ui.painter().circle_filled(c, PART_TILE / 2.0 - 2.0 + 2.0 * k, t.pressed);
            icons::paint(ui.painter(), Rect::from_center_size(c, vec2(22.0, 22.0)), Icon::PlusMinus, t.text);
            if resp.clicked() {
                haptics::tap(ctx, Haptic::Selection);
                ctx.data_mut(|d| d.insert_temp(Id::new("mask-combine-at"), r));
                mobile::open_menu(ctx, "maskCombine", r);
            }
        }
    });
}

/// A tile of the masks' column (`<key>`): `icon` on a rounded square `side` wide, ringed in the
/// accent colour when `sel`, dimmed when hidden, with a `badge` (− or ∩) in its corner.
#[allow(clippy::too_many_arguments)]
fn tile(ui: &mut Ui, t: &Tokens, key: &str, name: &str, icon: Icon, side: f32, sel: bool, visible: bool, badge: Option<&str>) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(vec2(MASK_TILE, side), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, sel, name));
    register(ui.ctx(), key, r);
    let k = glass::press(ui.ctx(), &resp);
    let sq = Rect::from_center_size(r.center(), vec2(side, side) * (1.0 - 0.06 * k));
    let p = ui.painter();
    p.rect_filled(sq, 9.0, if sel { t.tool_active } else { t.chrome });
    p.rect_stroke(sq, 9.0, if sel { Stroke::new(2.0, t.accent) } else { Stroke::new(1.0, t.button_border) }, StrokeKind::Inside);
    let ink = if !visible {
        t.text_disabled
    } else if sel {
        t.text
    } else {
        t.text_label
    };
    icons::paint(p, Rect::from_center_size(sq.center(), vec2(side * 0.48, side * 0.48)), icon, ink);
    if let Some(b) = badge {
        let c = sq.right_top() + vec2(-6.0, 6.0);
        p.circle_filled(c, 7.0, t.text);
        p.text(c, Align2::CENTER_CENTER, b, t.semibold(11.0), t.chrome);
    }
    resp
}

/// The brush's tools over the photo's left edge (`button:brush-<id>`): paint and erase, size,
/// feather and flow (each opens its slider), and Auto Mask.
fn brush_column(app: &mut LightcraftApp, ctx: &egui::Context, canvas: Rect) {
    let t = Tokens::get(ctx);
    let n = 6.0;
    let h = n * MASK_TILE + (n - 1.0) * 8.0 + 12.0;
    let at = pos2(canvas.left() + 8.0, (canvas.center().y - h / 2.0).max(canvas.top() + 8.0));
    column(ctx, "brush-column", at, |ui| {
        let size = format!("{:.0}", (app.ui.brush_size * 400.0).round());
        let items: [(&str, Option<Icon>, &str, bool); 6] = [
            ("paint", Some(Icon::Brush), "Brush", !app.ui.brush_erase),
            ("erase", Some(Icon::Eraser), "Erase", app.ui.brush_erase),
            ("size", None, "Size", false),
            ("feather", Some(Icon::Feather), "Feather", false),
            ("flow", Some(Icon::Flow), "Flow", false),
            ("auto", Some(Icon::Wand), "Auto Mask", app.ui.brush_auto_mask),
        ];
        for (id, icon, label, on) in items {
            let (r, resp) = ui.allocate_exact_size(vec2(MASK_TILE, MASK_TILE), Sense::click());
            resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, crate::i18n::tr(label)));
            register(ctx, format!("button:brush-{id}"), r);
            let k = glass::press(ctx, &resp);
            let sq = r.shrink(1.0 + 2.0 * k);
            let p = ui.painter();
            if on {
                p.rect_filled(sq, 10.0, t.accent);
            } else if id == "size" {
                p.rect_stroke(sq, 10.0, Stroke::new(1.0, t.button_border), StrokeKind::Inside);
            }
            let ink = if on { Color32::WHITE } else { t.text };
            match icon {
                Some(icon) => icons::paint(p, Rect::from_center_size(sq.center(), vec2(22.0, 22.0)), icon, ink),
                None => {
                    p.text(sq.center(), Align2::CENTER_CENTER, &size, t.semibold(15.0), ink);
                }
            }
            if resp.clicked() {
                haptics::tap(ctx, Haptic::Selection);
                match id {
                    "paint" => app.ui.brush_erase = false,
                    "erase" => app.ui.brush_erase = true,
                    "auto" => app.ui.brush_auto_mask = !app.ui.brush_auto_mask,
                    _ => mobile::open_menu(ctx, &format!("brush-{id}"), r),
                }
            }
        }
    });
}

// -------------------------------------------------------------------------------- menus

/// The brush's sliders as pop-overs: (menu, `ui.*` control id, name, range).
const BRUSH_SLIDERS: [(&str, &str, &str, f64, f64); 3] = [
    ("brush-size", "ui.brushSize", "Size", 1.0, 100.0),
    ("brush-feather", "ui.brushFeather", "Feather", 0.0, 100.0),
    ("brush-flow", "ui.brushFlow", "Flow", 1.0, 100.0),
];

/// The screen's menus (each draws only while open): Create New Mask, a mask's options (its pin or
/// its tile), a part's options, ± and its kinds, the brush's sliders and "…".
pub fn menus(app: &mut LightcraftApp, ctx: &egui::Context) {
    let max_h = (ctx.content_rect().height() - 150.0).max(120.0);
    mobile::actions(ctx, "maskNew", Some("Create New Mask"), |ui| {
        egui::ScrollArea::vertical().id_salt("mask-new-list").max_height(max_h).show(ui, |ui| {
            for item in NEW_KINDS {
                let Some((kind, icon, label)) = item else {
                    mobile::row_gap(ui);
                    continue;
                };
                if mobile::row(ui, &format!("maskNew-{kind}"), Some(icon), crate::i18n::tr(label), true) {
                    if kind == "brush" {
                        // a new mask: the first stroke makes it
                        let _ = app.run("mask.select", json!({"id": null}));
                        app.ui.brush_erase = false;
                    }
                    masking::create(app, ctx, kind);
                    app.ui.mask_group.clear();
                }
            }
        });
    });
    let mask = selected(app);
    mobile::actions(ctx, "maskPin", mask.as_ref().map(|m| m.name.as_str()), |ui| {
        let Some(m) = &mask else { return };
        let (icon, label) = if m.visible { (Icon::EyeOff, "Hide Mask") } else { (Icon::Eye, "Show Mask") };
        if mobile::row(ui, "maskPin-visible", Some(icon), crate::i18n::tr(label), true) {
            let _ = app.run("mask.visible", json!({"id": m.id}));
        }
        if mobile::row_checked(ui, "maskPin-invert", Some(Icon::Invert), crate::i18n::tr("Invert"), true, Some(m.invert)) {
            let _ = app.run("mask.invert", json!({"id": m.id}));
        }
        if mobile::row(ui, "maskPin-duplicate", Some(Icon::Plus), crate::i18n::tr("Duplicate Mask"), true) {
            let _ = app.run("mask.duplicate", json!({"id": m.id}));
        }
        if mobile::row(ui, "maskPin-adjust", Some(Icon::Sliders), crate::i18n::tr("Adjust"), true) && app.ui.mask_group.is_empty() {
            app.ui.mask_group = "light".into();
        }
        mobile::row_gap(ui);
        if mobile::row(ui, "maskPin-delete", Some(Icon::Trash), crate::i18n::tr("Delete Mask"), true) {
            haptics::tap(ctx, Haptic::Warning);
            let _ = app.run("mask.delete", json!({"id": m.id}));
            app.ui.tool.clear();
        }
    });
    let k = ctx.data(|d| d.get_temp::<usize>(Id::new("mask-comp-menu"))).unwrap_or(0);
    let comp = mask.as_ref().and_then(|m| m.components.get(k).map(|c| (m.id, m.components.len(), c.clone())));
    let comp_name = comp.as_ref().map(|(_, _, c)| c.name.clone().unwrap_or_else(|| crate::i18n::tr(masking::kind_label(&c.shape).0).to_string()));
    mobile::actions(ctx, "maskComp", comp_name.as_deref(), |ui| {
        let Some((mid, n, c)) = &comp else { return };
        let run = |app: &mut LightcraftApp, p: serde_json::Value| {
            let mut p = p;
            p["id"] = json!(mid);
            p["component"] = json!(k);
            let _ = app.run("mask.component", p);
        };
        if mobile::row_checked(ui, "comp-invert", Some(Icon::Invert), crate::i18n::tr("Invert"), true, Some(c.invert)) {
            run(app, json!({"action": "invert"}));
        }
        if mobile::row(ui, "comp-duplicate", Some(Icon::Plus), crate::i18n::tr("Duplicate"), true) {
            run(app, json!({"action": "duplicate"}));
        }
        if k > 0 {
            mobile::row_gap(ui);
            for (op, label, on) in [
                ("add", "Add", c.op == MaskOp::Add),
                ("subtract", "Subtract", c.op == MaskOp::Subtract),
                ("intersect", "Intersect", c.op == MaskOp::Intersect),
            ] {
                if mobile::row_checked(ui, &format!("comp-op-{op}"), None, crate::i18n::tr(label), true, Some(on)) && !on {
                    run(app, json!({"action": "op", "op": op}));
                }
            }
        }
        mobile::row_gap(ui);
        let del = if *n == 1 { "Delete (and the Mask)" } else { "Delete" };
        if mobile::row(ui, "comp-delete", Some(Icon::Trash), crate::i18n::tr(del), true) {
            haptics::tap(ctx, Haptic::Warning);
            run(app, json!({"action": "delete"}));
            app.ui.tool.clear();
        }
    });
    // ± then the kinds, from the same place
    let at = ctx.data(|d| d.get_temp::<Rect>(Id::new("mask-combine-at"))).unwrap_or(Rect::NOTHING);
    mobile::actions(ctx, "maskCombine", None, |ui| {
        for (op, menu, icon, label) in [
            ("add", "maskAdd", Icon::Plus, "Add to Mask"),
            ("subtract", "maskSubtract", Icon::Minus, "Subtract from Mask"),
            ("intersect", "maskIntersect", Icon::Invert, "Intersect with Mask"),
        ] {
            if mobile::row(ui, &format!("maskCombine-{op}"), Some(icon), crate::i18n::tr(label), true) {
                mobile::open_menu(ctx, menu, at);
            }
        }
    });
    for (op, menu, title) in
        [("add", "maskAdd", "Add to Mask"), ("subtract", "maskSubtract", "Subtract from Mask"), ("intersect", "maskIntersect", "Intersect with Mask")]
    {
        mobile::actions(ctx, menu, Some(title), |ui| {
            for (kind, label) in masking::COMPONENT_KINDS {
                // painting only adds or erases
                if kind == "brush" && op == "intersect" {
                    continue;
                }
                if mobile::row(ui, &format!("{menu}-{kind}"), None, crate::i18n::tr(label), true) {
                    masking::add_component(app, ctx, op, kind);
                }
            }
        });
    }
    for (menu, id, label, min, max) in BRUSH_SLIDERS {
        mobile::actions_wide(ctx, menu, None, 300.0, |ui| {
            let v = match id {
                "ui.brushSize" => (app.ui.brush_size * 400.0) as f64,
                "ui.brushFeather" => app.ui.brush_feather as f64,
                _ => app.ui.brush_flow as f64,
            };
            let spec = lightcraft_develop::ControlSpec {
                id,
                label,
                section: lightcraft_develop::Section::Light,
                min,
                max,
                default: min,
                step: 1.0,
                decimals: 0,
                track: lightcraft_develop::Track::Plain,
            };
            if let Some(v) = crate::widgets::slider(ui, &spec, v, true, None).value {
                match id {
                    "ui.brushSize" => app.ui.brush_size = (v / 400.0) as f32,
                    "ui.brushFeather" => app.ui.brush_feather = v as f32,
                    _ => app.ui.brush_flow = v as f32,
                }
            }
        });
    }
    mobile::actions(ctx, "maskMore", None, |ui| {
        let pins = app.ui.mask_pins;
        if mobile::row_checked(ui, "maskShowPins", Some(Icon::Pin), crate::i18n::tr("Show Pins"), true, Some(pins)) {
            let _ = app.run("view.maskPins", json!({"show": !pins}));
        }
        if mobile::row(ui, "maskOverlayOptions", Some(Icon::Eye), crate::i18n::tr("Overlay Options"), mask.is_some()) {
            app.ui.mask_group = "mask".into();
        }
        mobile::row_gap(ui);
        let any = !masks(app).is_empty();
        if mobile::row(ui, "maskDeleteAll", Some(Icon::Trash), crate::i18n::tr("Delete All Masks"), any) {
            for m in masks(app).iter().rev() {
                let _ = app.run("mask.delete", json!({"id": m.id}));
            }
            app.ui.tool.clear();
        }
    });
}
