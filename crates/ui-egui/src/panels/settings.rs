//! The Settings dialog (⌘,): General, Import, Performance, Interface, Sync, AI Models.
//!
//! Changes apply immediately (no OK/Cancel). Where they are stored:
//! - **app settings** ([`crate::state::AppSettings`]: startup view, delete confirmation, GPU,
//!   preview size, filmstrip/grid badges, last library) and Auto Advance live in the UI state,
//!   saved by the host in its config folder (`ui.json`);
//! - **library settings** (import defaults, XMP sidecars, thumbnail cache size) go through the
//!   `library.preferences` / `library.xmpPreferences` commands into the library's `prefs.json`,
//!   so they travel with the library.
//!
//! On a phone (the compact layout) it looks and behaves like iOS's Settings app: a list of the
//! sections (`tab` "") that pushes each one as a page of its own, rows on rounded cards under small
//! grey headings with notes below them, switches, and choices in pull-down menus. What the iOS app
//! can't do isn't offered there (another library folder, an external editor, watched or
//! smart-preview folders, XMP sidecars no other app sees, Local folders); nor is the filmstrip,
//! which the compact layout doesn't have.

use egui::{Align, Color32, Layout, Margin, Rect, RichText, Sense, Stroke, pos2, vec2};
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::icons::Icon;
use crate::panels::mobile;
use crate::state::{GridBadges, PREVIEW_EDGES, StartupView};
use crate::theme::Tokens;
use crate::widgets::register;

/// (id, label) of the tabs, in order.
pub const TABS: &[(&str, &str)] = &[
    ("general", "General"),
    ("import", "Import"),
    ("performance", "Performance"),
    ("interface", "Interface"),
    ("sync", "Sync"),
    ("models", "AI Models"),
];

/// Thumbnail cache sizes offered (MB).
const CACHE_SIZES: [u32; 5] = [512, 1024, 2048, 4096, 8192];

const LABEL_W: f32 = 150.0;

/// The iOS app's library lives in its container: no folder to choose or watch, no external editor,
/// no other app to read XMP sidecars, no Local folders. Those settings aren't offered there.
const IOS: bool = cfg!(target_os = "ios");

/// The phone's list: each tab's icon and the colour of its tile (iOS's system colours).
const TAB_TILES: [(Icon, Color32); 6] = [
    (Icon::Gear, Color32::from_rgb(0x8e, 0x8e, 0x93)),
    (Icon::Photos, Color32::from_rgb(0x30, 0xd1, 0x58)),
    (Icon::Wand, Color32::from_rgb(0xff, 0x9f, 0x0a)),
    (Icon::Eye, Color32::from_rgb(0x5e, 0x5c, 0xe6)),
    (Icon::Cloud, Color32::from_rgb(0x0a, 0x84, 0xff)),
    (Icon::Subject, Color32::from_rgb(0xbf, 0x5a, 0xf2)),
];

/// A row's height on the phone (Apple's 44 pt).
const ROW: f32 = crate::TOUCH_ROW_H;

/// The phone page's title, and its back button (`None` on the list).
pub fn phone_title(tab: &str) -> (&'static str, Option<&'static str>) {
    match TABS.iter().find(|(id, _)| *id == tab) {
        Some((_, label)) => (*label, Some("Settings")),
        None => ("Settings", None),
    }
}

/// The dialog body for `tab` (the tab bar switches `tab`).
pub fn body(app: &mut LightcraftApp, ui: &mut egui::Ui, tab: &mut String) {
    if crate::is_compact(ui.ctx()) {
        phone_body(app, ui, tab);
        return;
    }
    let t = Tokens::get(ui.ctx());
    ui.set_min_width(crate::panels::modal_width(ui.ctx(), 560.0));
    ui.set_min_height(330.0);
    // (wrapping: on a phone the six tabs don't fit in one row)
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for (id, label) in TABS {
            if crate::widgets::text_button(ui, &format!("settingsTab-{id}"), label, tab == id).clicked() {
                *tab = id.to_string();
            }
        }
    });
    ui.separator();
    ui.add_space(2.0);
    match tab.as_str() {
        "import" => import_tab(app, ui, &t),
        "performance" => performance_tab(app, ui, &t),
        "interface" => interface_tab(app, ui, &t),
        "sync" => {
            // (the storage numbers make this tab long)
            let h = (ui.ctx().content_rect().height() - 240.0).max(300.0);
            egui::ScrollArea::vertical().max_height(h).auto_shrink([false, true]).show(ui, |ui| sync_tab(app, ui, &t));
        }
        "models" => {
            // (five models make this tab long)
            let h = (ui.ctx().content_rect().height() - 240.0).max(300.0);
            egui::ScrollArea::vertical().max_height(h).auto_shrink([false, true]).show(ui, |ui| crate::panels::ai_models::tab(app, ui, &t));
        }
        _ => general_tab(app, ui, &t),
    }
}

/// The phone's Settings: the list of sections, or the one `tab` names, sliding in from the side
/// it comes from (a section from the right, the list back from the left).
fn phone_body(app: &mut LightcraftApp, ui: &mut egui::Ui, tab: &mut String) {
    let t = Tokens::get(ui.ctx());
    let k = mobile::enter(ui.ctx(), egui::Id::new("settings-page-in"), tab.as_str(), 0.25);
    if k < 1.0 {
        // (a page starts at its top)
        ui.scroll_to_cursor(Some(Align::TOP));
    }
    let dx = (1.0 - k) * 48.0 * if tab.is_empty() { -1.0 } else { 1.0 };
    let r = ui.available_rect_before_wrap().translate(vec2(dx, 0.0));
    ui.scope_builder(egui::UiBuilder::new().max_rect(r), |ui| {
        ui.multiply_opacity(k);
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.data_mut(|d| d.remove::<Card>(card_id()));
        match tab.as_str() {
            "general" => general_tab(app, ui, &t),
            "import" => import_tab(app, ui, &t),
            "performance" => performance_tab(app, ui, &t),
            "interface" => interface_tab(app, ui, &t),
            "sync" => sync_tab(app, ui, &t),
            "models" => crate::panels::ai_models::tab(app, ui, &t),
            _ => list(app, ui, &t, tab),
        }
        card_end(ui, &t);
        ui.add_space(12.0);
    });
}

/// The list, as iOS's Settings app opens: the library (tap: its sync) on a card of its own, then a
/// row per section, its icon on a coloured tile.
fn list(app: &LightcraftApp, ui: &mut egui::Ui, t: &Tokens, tab: &mut String) {
    let n = app.session.catalog.len();
    let synced = app.session.sync_state().filter(|s| s.signed_in()).map(|s| s.config.server.clone());
    let (name, sub) = match (&app.session.library, &synced) {
        (Some(l), Some(server)) => (
            l.dir.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default(),
            crate::i18n::tr_format!("{n} photos · synced with {server}", n = n, server = server),
        ),
        (Some(l), None) => (
            l.dir.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default(),
            crate::i18n::tr_format!("{n} photos on this device", n = n),
        ),
        (None, _) => (crate::i18n::tr("Demo Library").to_string(), crate::i18n::tr_format!("{n} photos, not saved", n = n)),
    };
    ui.add_space(6.0);
    if nav_row(ui, t, "settingsLibrary", Icon::Photos, t.accent, &name, Some(sub.as_str()), "") {
        *tab = "sync".into();
    }
    card_end(ui, t);
    ui.add_space(28.0);
    for ((id, label), (icon, tile)) in TABS.iter().zip(TAB_TILES) {
        let value = match *id {
            "sync" if synced.is_some() => crate::i18n::tr("On"),
            "sync" => crate::i18n::tr("Off"),
            "general" => app.ui.language.name(),
            _ => "",
        };
        if nav_row(ui, t, &format!("settingsTab-{id}"), icon, tile, crate::i18n::tr(label), None, value) {
            *tab = id.to_string();
        }
    }
}

/// The phone's rows sit on a rounded card: the first row after a heading or a note starts one,
/// whose background [`card_end`] draws behind them once the run is over.
#[derive(Clone, Copy)]
struct Card {
    shape: egui::layers::ShapeIdx,
    top: f32,
}

fn card_id() -> egui::Id {
    egui::Id::new("lc-settings-card")
}

/// Before a phone row: start a card, or draw the hairline between this row and the one above
/// (from `inset`, where the row's text starts, to the card's edge, as iOS does).
fn card_row(ui: &mut egui::Ui, t: &Tokens, inset: f32) {
    let y = ui.cursor().top();
    if ui.data(|d| d.get_temp::<Card>(card_id())).is_some() {
        let r = ui.max_rect();
        ui.painter().hline((r.left() + inset)..=r.right(), y, Stroke::new(0.5, t.divider));
    } else {
        let shape = ui.painter().add(egui::Shape::Noop);
        ui.data_mut(|d| d.insert_temp(card_id(), Card { shape, top: y }));
    }
}

/// Close the card the rows above are on (if any): its rounded background goes behind them.
fn card_end(ui: &mut egui::Ui, t: &Tokens) {
    let Some(card) = ui.data(|d| d.get_temp::<Card>(card_id())) else { return };
    ui.data_mut(|d| d.remove::<Card>(card_id()));
    let r = Rect::from_min_max(pos2(ui.max_rect().left(), card.top), pos2(ui.max_rect().right(), ui.cursor().top()));
    ui.painter().set(card.shape, egui::epaint::RectShape::filled(r, 10.0, t.inset));
}

/// A whole phone row that reacts to a tap (`button:{id}` unless `id` is empty), lit while pressed.
fn tap_row(ui: &mut egui::Ui, t: &Tokens, id: &str, label: &str, h: f32, inset: f32, enabled: bool) -> (Rect, egui::Response) {
    card_row(ui, t, inset);
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), h), if enabled { Sense::click() } else { Sense::hover() });
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    if !id.is_empty() {
        register(ui.ctx(), format!("button:{id}"), r);
    }
    if enabled && resp.is_pointer_button_down_on() {
        ui.painter().rect_filled(r, 0.0, t.hover);
    }
    (r, resp)
}

/// `text` as one line of at most `max_w` points (cut with "…").
fn line(ui: &egui::Ui, text: &str, size: f32, color: Color32, max_w: f32) -> std::sync::Arc<egui::Galley> {
    egui::WidgetText::from(RichText::new(text).size(size).color(color)).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        max_w.max(20.0),
        egui::TextStyle::Body,
    )
}

/// A row of the phone's list that opens a page: `icon` white on a `tile`, the label (and a smaller
/// line under it, on a taller row), a value and a chevron. True when tapped.
#[allow(clippy::too_many_arguments)]
fn nav_row(ui: &mut egui::Ui, t: &Tokens, id: &str, icon: Icon, tile: Color32, label: &str, sub: Option<&str>, value: &str) -> bool {
    let (side, h) = if sub.is_some() { (46.0, 72.0) } else { (29.0, ROW) };
    let text_x = 16.0 + side + 14.0;
    let (r, resp) = tap_row(ui, t, id, label, h, text_x, true);
    let p = ui.painter();
    let tile_r = Rect::from_min_size(pos2(r.left() + 16.0, r.center().y - side / 2.0), vec2(side, side));
    p.rect_filled(tile_r, side * 0.24, tile);
    crate::icons::paint(p, tile_r.shrink(side * 0.19), icon, Color32::WHITE);
    let chevron = Rect::from_center_size(pos2(r.right() - 22.0, r.center().y), vec2(14.0, 14.0));
    crate::icons::paint(p, chevron, Icon::ChevronRight, t.text_disabled);
    let value = line(ui, value, 17.0, t.text_dim, r.width() * 0.4);
    let value_x = chevron.left() - 6.0 - value.size().x;
    let label = line(ui, label, 17.0, t.text, value_x - 8.0 - (r.left() + text_x));
    let p = ui.painter();
    match sub {
        Some(sub) => {
            let sub = line(ui, sub, 13.0, t.text_dim, chevron.left() - 8.0 - (r.left() + text_x));
            let top = r.center().y - (label.size().y + 2.0 + sub.size().y) / 2.0;
            p.galley(pos2(r.left() + text_x, top + label.size().y + 2.0), sub, t.text_dim);
            p.galley(pos2(r.left() + text_x, top), label, t.text);
        }
        None => p.galley(pos2(r.left() + text_x, r.center().y - label.size().y / 2.0), label, t.text),
    }
    p.galley(pos2(value_x, r.center().y - value.size().y / 2.0), value, t.text_dim);
    resp.clicked()
}

/// A phone row showing a choice: `label`, then `value` and ⌃⌄ on the right (`button:{id}`). A tap
/// opens `items` (`option` / `group` rows) as a pull-down menu under the value, scrolling if long.
fn menu_row(ui: &mut egui::Ui, t: &Tokens, id: &str, label: &str, value: &str, items: impl FnOnce(&mut egui::Ui)) {
    let (r, resp) = tap_row(ui, t, id, label, ROW, 16.0, true);
    let c = pos2(r.right() - 22.0, r.center().y);
    for s in [-1.0, 1.0] {
        ui.painter()
            .add(egui::Shape::line(vec![c + vec2(-3.5, s * 1.8), c + vec2(0.0, s * 5.3), c + vec2(3.5, s * 1.8)], Stroke::new(1.6, t.text_dim)));
    }
    // the label keeps its words; the value gives way (as iOS cuts it)
    let label = line(ui, crate::i18n::tr(label), 17.0, t.text, r.width() * 0.62);
    let value = line(ui, value, 17.0, t.text_dim, c.x - 10.0 - (r.left() + 16.0 + label.size().x + 12.0));
    let value_x = c.x - 10.0 - value.size().x;
    ui.painter().galley(pos2(r.left() + 16.0, r.center().y - label.size().y / 2.0), label, t.text);
    ui.painter().galley(pos2(value_x, r.center().y - value.size().y / 2.0), value, t.text_dim);
    let ctx = ui.ctx().clone();
    if resp.clicked() {
        mobile::open_menu(&ctx, id, Rect::from_min_max(pos2(value_x, r.top()), pos2(r.right() - 8.0, r.bottom())));
    }
    let max_h = ctx.content_rect().height() * 0.6;
    mobile::actions(&ctx, id, None, |ui| {
        egui::ScrollArea::vertical().id_salt(id).max_height(max_h).show(ui, items);
    });
}

/// One choice of a [`select`] (`button:{id}-{i}` on the phone); true when chosen.
fn option(ui: &mut egui::Ui, id: &str, i: usize, selected: bool, label: &str) -> bool {
    if crate::is_compact(ui.ctx()) {
        mobile::row_checked(ui, &format!("{id}-{i}"), None, label, true, Some(selected))
    } else {
        ui.selectable_label(selected, label).clicked()
    }
}

/// A heading between a [`select`]'s choices.
fn group(ui: &mut egui::Ui, t: &Tokens, label: &str) {
    if crate::is_compact(ui.ctx()) {
        mobile::row_gap(ui);
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::hover());
        ui.painter().text(pos2(r.left() + 16.0, r.center().y), egui::Align2::LEFT_CENTER, label, t.font(13.0), t.text_dim);
    } else {
        ui.label(RichText::new(label).size(10.5).weak());
    }
}

/// A choice from a list (`option`s, `group`s): a combo box beside the label on the desktop
/// (`combo:{id}`), a pull-down menu on the phone.
fn select(ui: &mut egui::Ui, t: &Tokens, id: &str, label: &str, value: String, items: impl FnOnce(&mut egui::Ui)) {
    if crate::is_compact(ui.ctx()) {
        menu_row(ui, t, id, label, &value, items);
        return;
    }
    row(ui, t, label, |ui| {
        let r = egui::ComboBox::from_id_salt(id).width(240.0).selected_text(value).show_ui(ui, items);
        register(ui.ctx(), format!("combo:{id}"), r.response.rect);
    });
}

/// A labelled choice among a few `options`: mutually exclusive buttons on the desktop
/// (`button:{id}-{i}`), a pull-down menu on the phone. True when it changed.
fn pick<V: PartialEq + Copy>(ui: &mut egui::Ui, t: &Tokens, id: &str, label: &str, options: &[(V, &str)], value: &mut V) -> bool {
    if !crate::is_compact(ui.ctx()) {
        return row(ui, t, label, |ui| choices(ui, id, options, value));
    }
    let current = options.iter().find(|(v, _)| *v == *value).map(|(_, l)| crate::i18n::tr(l)).unwrap_or_default();
    let mut chosen = None;
    menu_row(ui, t, id, label, current, |ui| {
        for (i, (v, l)) in options.iter().enumerate() {
            if option(ui, id, i, *v == *value, crate::i18n::tr(l)) {
                chosen = Some(*v);
            }
        }
    });
    match chosen {
        Some(v) if v != *value => {
            *value = v;
            true
        }
        _ => false,
    }
}

/// A label and its value: beside each other on the desktop, the value on the right on the phone.
fn info(ui: &mut egui::Ui, t: &Tokens, label: &str, value: &str, color: Color32) -> Rect {
    if !crate::is_compact(ui.ctx()) {
        return row(ui, t, label, |ui| ui.label(RichText::new(value).color(color)).rect);
    }
    let (r, _) = tap_row(ui, t, "", label, ROW, 16.0, false);
    let label = line(ui, crate::i18n::tr(label), 17.0, t.text, r.width() * 0.62);
    let value = line(ui, value, 17.0, if color == t.text { t.text_dim } else { color }, r.width() - 44.0 - label.size().x);
    ui.painter().galley(pos2(r.left() + 16.0, r.center().y - label.size().y / 2.0), label, t.text);
    let vr = Rect::from_min_size(pos2(r.right() - 16.0 - value.size().x, r.center().y - value.size().y / 2.0), value.size());
    ui.painter().galley(vr.min, value, t.text_dim);
    vr
}

/// A button (`button:{id}`): beside the others on the desktop, a row of its own on the phone (blue,
/// or red when it `destroys` something). True when tapped.
fn action(ui: &mut egui::Ui, t: &Tokens, id: &str, label: &str, enabled: bool, destroys: bool) -> bool {
    if !crate::is_compact(ui.ctx()) {
        let r = ui.add_enabled(enabled, egui::Button::new(crate::i18n::tr(label)));
        register(ui.ctx(), format!("button:{id}"), r.rect);
        return r.clicked();
    }
    let label = crate::i18n::tr(label);
    let (r, resp) = tap_row(ui, t, id, label, ROW, 16.0, enabled);
    let color = match (enabled, destroys) {
        (false, _) => t.text_disabled,
        (true, true) => t.reject,
        (true, false) => t.accent,
    };
    let g = line(ui, label, 17.0, color, r.width() - 32.0);
    ui.painter().galley(pos2(r.left() + 16.0, r.center().y - g.size().y / 2.0), g, color);
    resp.clicked()
}

/// A one-line text field (`field:{id}`): beside its label on the desktop; on the phone the label
/// on the left and the text right-aligned and frameless in the rest of the row, as iOS forms have it.
#[allow(clippy::too_many_arguments)]
fn text_row(ui: &mut egui::Ui, t: &Tokens, id: &str, label: &str, value: &mut String, hint_text: &str, password: bool, width: f32) -> egui::Response {
    let compact = crate::is_compact(ui.ctx());
    let r = row(ui, t, label, |ui| {
        let edit = egui::TextEdit::singleline(value).hint_text(crate::i18n::tr(hint_text)).password(password);
        if compact {
            ui.add(
                edit.frame(egui::Frame::NONE)
                    .font(egui::FontId::proportional(17.0))
                    .horizontal_align(Align::RIGHT)
                    .vertical_align(Align::Center)
                    .min_size(vec2(0.0, ROW))
                    .desired_width(ui.available_width()),
            )
        } else {
            ui.add(edit.desired_width(width.min(ui.available_width())))
        }
    });
    register(ui.ctx(), format!("field:{id}"), r.rect);
    r
}

fn heading(ui: &mut egui::Ui, t: &Tokens, text: &str) {
    if crate::is_compact(ui.ctx()) {
        // iOS: small grey capitals over the card
        card_end(ui, t);
        ui.add_space(26.0);
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            ui.label(RichText::new(crate::i18n::tr(text).to_uppercase()).size(13.0).color(t.text_dim));
        });
        ui.add_space(7.0);
        return;
    }
    ui.add_space(4.0);
    ui.label(RichText::new(crate::i18n::tr(text)).font(t.semibold(12.5)).color(t.text));
}

/// A labelled row. On the phone: the label on the left and `add`'s widgets on the right, on the
/// card; without a label, `add`'s own rows (buttons, switches) as they are.
fn row<R>(ui: &mut egui::Ui, t: &Tokens, label: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    if crate::is_compact(ui.ctx()) {
        if label.is_empty() {
            return add(ui);
        }
        card_row(ui, t, 16.0);
        return egui::Frame::NONE
            .inner_margin(Margin::symmetric(16, 0))
            .show(ui, |ui| {
                ui.allocate_ui_with_layout(vec2(ui.available_width(), ROW), Layout::left_to_right(Align::Center), |ui| {
                    ui.set_min_height(ROW);
                    ui.label(RichText::new(crate::i18n::tr(label)).size(17.0).color(t.text));
                    ui.add_space(12.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), add).inner
                })
                .inner
            })
            .inner;
    }
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(egui::vec2(LABEL_W, 24.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.set_min_width(LABEL_W);
            ui.label(RichText::new(crate::i18n::tr(label)).color(t.text_label));
        });
        add(ui)
    })
    .inner
}

/// A note. On the phone it goes under the card above it, as iOS's section footers do.
fn hint(ui: &mut egui::Ui, t: &Tokens, text: &str) {
    if crate::is_compact(ui.ctx()) {
        card_end(ui, t);
        ui.add_space(7.0);
        egui::Frame::NONE.inner_margin(Margin::symmetric(16, 0)).show(ui, |ui| {
            ui.label(RichText::new(crate::i18n::tr(text)).size(13.0).color(t.text_dim));
        });
        ui.add_space(10.0);
        return;
    }
    ui.label(RichText::new(crate::i18n::tr(text)).size(11.0).color(t.text_dim));
}

/// A checkbox addressable as `check:{id}` (on the phone, a row with a switch); true when toggled.
fn check(ui: &mut egui::Ui, id: &str, value: &mut bool, label: &str) -> bool {
    let r = if crate::is_compact(ui.ctx()) {
        card_row(ui, &Tokens::get(ui.ctx()), 16.0);
        egui::Frame::NONE
            .inner_margin(Margin::symmetric(16, 0))
            .show(ui, |ui| crate::widgets::check(ui, value, RichText::new(crate::i18n::tr(label)).size(17.0)))
            .inner
    } else {
        crate::widgets::check(ui, value, crate::i18n::tr(label))
    };
    register(ui.ctx(), format!("check:{id}"), r.rect);
    r.changed()
}

/// Mutually exclusive buttons (`button:{id}-{index}`).
fn choices<V: PartialEq + Copy>(ui: &mut egui::Ui, id: &str, options: &[(V, &str)], value: &mut V) -> bool {
    let mut changed = false;
    ui.spacing_mut().item_spacing.x = 4.0;
    for (i, (v, l)) in options.iter().enumerate() {
        if crate::widgets::text_button(ui, &format!("{id}-{i}"), l, *value == *v).clicked() && *value != *v {
            *value = *v;
            changed = true;
        }
    }
    changed
}

// ------------------------------------------------------------------------------------- General

fn general_tab(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let languages: Vec<_> = crate::i18n::Locale::ALL.iter().map(|language| (*language, language.name())).collect();
    pick(ui, t, "settingsLanguage", crate::i18n::tr("Language"), &languages, &mut app.ui.language);
    crate::i18n::set_language(app.ui.language);
    if !IOS {
        heading(ui, t, crate::i18n::tr("Library"));
        let location = match &app.session.library {
            Some(l) => l.dir.display().to_string(),
            None => crate::i18n::tr("In memory — nothing is saved").to_string(),
        };
        row(ui, t, crate::i18n::tr("Location"), |ui| {
            ui.label(RichText::new(location).color(t.text));
        });
        row(ui, t, "", |ui| {
            let can = app.services.pick_folder.is_some();
            if action(ui, t, "settingsOpenLibrary", "Open Library…", can, false) {
                let _ = app.run("app.openLibrary", json!({}));
            }
            if !can {
                hint(ui, t, crate::i18n::tr("not available here"));
            }
        });
    }
    heading(ui, t, crate::i18n::tr("Startup"));
    pick(
        ui,
        t,
        "settingsStartup",
        crate::i18n::tr("Open in"),
        &[(StartupView::Last, "Last view"), (StartupView::Grid, "Photo Grid"), (StartupView::Detail, "Detail")],
        &mut app.ui.settings.startup_view,
    );
    heading(ui, t, crate::i18n::tr("Culling"));
    check(ui, "settings.autoAdvance", &mut app.ui.auto_advance, "Auto Advance: move to the next photo after rating or flagging");
    check(ui, "settings.confirmDelete", &mut app.ui.settings.confirm_delete, "Confirm before moving photos to Recently Deleted");
    if IOS {
        return;
    }
    heading(ui, t, crate::i18n::tr("External Editor"));
    row(ui, t, crate::i18n::tr("Application"), |ui| {
        let r = ui
            .add(egui::TextEdit::singleline(&mut app.ui.settings.external_editor).hint_text(crate::i18n::tr("System default")).desired_width(220.0));
        register(ui.ctx(), "field:externalEditor", r.rect);
    });
    hint(
        ui,
        t,
        crate::i18n::tr(
            "Photo ▸ Edit in External Editor (⇧⌘E) renders a 16-bit TIFF copy, stacks it with the original and opens it here (an app name on macOS, a program path elsewhere).",
        ),
    );
}

// -------------------------------------------------------------------------------------- Import

/// A labelled preset picker: `None` = `none_label`. Returns the new choice when it changed.
fn preset_combo(
    app: &LightcraftApp,
    ui: &mut egui::Ui,
    t: &Tokens,
    id: &str,
    label: &str,
    current: Option<&str>,
    none_label: &str,
) -> Option<Option<String>> {
    let name = |pid: &str| app.session.presets.iter().find(|p| p.id == pid).map(|p| p.name.clone()).unwrap_or_else(|| format!("{pid} (missing)"));
    let text = current.map(name).unwrap_or_else(|| none_label.to_string());
    let mut out = None;
    select(ui, t, id, label, text, |ui| {
        if option(ui, id, 0, current.is_none(), none_label) {
            out = Some(None);
        }
        let mut last = "";
        for (i, p) in app.session.presets.iter().enumerate() {
            if p.group != last {
                last = &p.group;
                group(ui, t, last);
            }
            if option(ui, id, i + 1, current == Some(p.id.as_str()), &p.name) {
                out = Some(Some(p.id.clone()));
            }
        }
    });
    out.filter(|v| v.as_deref() != current)
}

/// Cameras of the raws in the library plus those with a stored default, sorted.
fn cameras(app: &LightcraftApp) -> Vec<String> {
    let mut v: Vec<String> = app
        .session
        .catalog
        .photos()
        .filter(|p| p.kind == lightcraft_catalog::MediaKind::Raw && !p.meta.camera.is_empty())
        .map(|p| p.meta.camera.clone())
        .chain(app.session.import_defaults.cameras.iter().map(|c| c.camera.clone()))
        .collect();
    v.sort_by_key(|c| c.to_lowercase());
    v.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    v
}

fn import_tab(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let d = app.session.import_defaults.clone();
    heading(ui, t, crate::i18n::tr("Raw defaults"));
    hint(ui, t, crate::i18n::tr("Settings new raw photos start from. Changing them doesn't touch photos already in the library."));
    if let Some(v) = preset_combo(app, ui, t, "settingsRawPreset", crate::i18n::tr("Raw photos"), d.raw_preset.as_deref(), "LightCraft Default") {
        let _ = app.run("library.preferences", json!({"import": {"rawPreset": v}}));
    }
    let mut per = d.per_camera;
    row(ui, t, "", |ui| {
        if check(ui, "settings.perCamera", &mut per, "Use camera-specific defaults") {
            let _ = app.run("library.preferences", json!({"import": {"perCamera": per}}));
        }
    });
    if per {
        let cams = cameras(app);
        if cams.is_empty() {
            hint(ui, t, crate::i18n::tr("No raw photos yet: cameras appear here once their photos are in the library."));
        }
        let mut cameras = |ui: &mut egui::Ui| {
            for (i, cam) in cams.iter().enumerate() {
                let entry = d.cameras.iter().find(|c| c.camera.eq_ignore_ascii_case(cam));
                // "Raw default" = no entry; otherwise the entry's preset (None = LightCraft Default)
                const RAW_DEFAULT: &str = "\u{1}raw";
                let current = match entry {
                    None => Some(RAW_DEFAULT),
                    Some(e) => e.preset.as_deref(),
                };
                let mut pick = None;
                let label = match current {
                    Some(RAW_DEFAULT) => "Same as raw default".to_string(),
                    None => "LightCraft Default".to_string(),
                    Some(pid) => app.session.presets.iter().find(|p| p.id == pid).map(|p| p.name.clone()).unwrap_or_else(|| pid.to_string()),
                };
                let id = format!("settingsCamera-{i}");
                select(ui, t, &id, cam, label, |ui| {
                    if option(ui, &id, 0, current == Some(RAW_DEFAULT), crate::i18n::tr("Same as raw default")) {
                        pick = Some(json!({"camera": cam, "remove": true}));
                    }
                    if option(ui, &id, 1, current.is_none(), crate::i18n::tr("LightCraft Default")) {
                        pick = Some(json!({"camera": cam, "preset": null}));
                    }
                    for (j, p) in app.session.presets.iter().enumerate() {
                        if option(ui, &id, j + 2, current == Some(p.id.as_str()), &p.name) {
                            pick = Some(json!({"camera": cam, "preset": p.id}));
                        }
                    }
                });
                if let Some(c) = pick {
                    let _ = app.run("library.preferences", json!({"camera": c}));
                }
            }
        };
        if crate::is_compact(ui.ctx()) {
            // (the page scrolls)
            cameras(ui);
        } else {
            egui::ScrollArea::vertical().max_height(150.0).id_salt("settingsCameras").show(ui, cameras);
        }
    }
    heading(ui, t, crate::i18n::tr("Other images (JPEG, PNG, TIFF, HEIC…)"));
    if let Some(v) = preset_combo(app, ui, t, "settingsOtherPreset", crate::i18n::tr("Non-raw photos"), d.other_preset.as_deref(), "None") {
        let _ = app.run("library.preferences", json!({"import": {"otherPreset": v}}));
    }
    heading(ui, t, crate::i18n::tr("Metadata"));
    hint(ui, t, crate::i18n::tr("Added to photos you import that don't already have it."));
    for (key, label, hint_text, value) in
        [("copyright", "Copyright", "© 2026 Your Name", d.copyright.clone()), ("creator", "Creator", "Your Name", d.creator.clone())]
    {
        let id = egui::Id::new(("settingsMeta", key));
        let mut text: String = ui.data(|m| m.get_temp(id)).unwrap_or(value.clone());
        let r = text_row(ui, t, &format!("settings.{key}"), label, &mut text, hint_text, false, 240.0);
        if r.lost_focus() && text.trim() != value {
            let _ = app.run("library.preferences", json!({"import": {key: text.trim()}}));
        }
        if r.has_focus() {
            ui.data_mut(|m| m.insert_temp(id, text));
        } else {
            ui.data_mut(|m| m.remove::<String>(id));
        }
    }
    if !app.session.metadata_presets.is_empty() {
        let cur = d.metadata_preset.clone();
        let mut pick = None;
        let id = "settingsMetaPreset";
        select(ui, t, id, crate::i18n::tr("Metadata preset"), cur.clone().unwrap_or_else(|| "None".into()), |ui| {
            if option(ui, id, 0, cur.is_none(), crate::i18n::tr("None")) {
                pick = Some(String::new());
            }
            for (i, m) in app.session.metadata_presets.iter().enumerate() {
                if option(ui, id, i + 1, cur.as_deref() == Some(m.name.as_str()), &m.name) {
                    pick = Some(m.name.clone());
                }
            }
        });
        if let Some(n) = pick {
            let _ = app.run("library.preferences", json!({"import": {"metadataPreset": n}}));
        }
    }
    if !cfg!(target_arch = "wasm32") && !IOS {
        heading(ui, t, crate::i18n::tr("Ignored folders and files"));
        hint(
            ui,
            t,
            crate::i18n::tr(
                "Names that importing and browsing skip, one per line. * matches any text and ? one character: *.fcpbundle skips Final Cut bundles and everything in them.",
            ),
        );
        let id = egui::Id::new("settingsIgnore");
        let value = d.ignore.join("\n");
        let mut text: String = ui.data(|m| m.get_temp(id)).unwrap_or(value.clone());
        let r = ui.add(egui::TextEdit::multiline(&mut text).desired_rows(4).desired_width(f32::INFINITY).hint_text("*.fcpbundle"));
        register(ui.ctx(), "field:settings.ignore", r.rect);
        if r.lost_focus()
            && text != value
            && let Err(e) = app.run("library.preferences", json!({"import": {"ignore": text}}))
        {
            app.toast(ui.ctx(), e);
        }
        if r.has_focus() {
            ui.data_mut(|m| m.insert_temp(id, text));
        } else {
            ui.data_mut(|m| m.remove::<String>(id));
        }
    }
    if !IOS {
        heading(ui, t, crate::i18n::tr("XMP sidecars"));
        let mut xmp = app.session.xmp;
        if check(ui, "settings.autoWriteXmp", &mut xmp.auto_write, "Automatically write changes into XMP sidecars") {
            let _ = app.run("library.xmpPreferences", json!({"autoWrite": xmp.auto_write}));
        }
        use lightcraft_engine::sidecar::SidecarNaming as N;
        let mut n = xmp.naming;
        if pick(ui, t, "settingsXmpNaming", crate::i18n::tr("Sidecar names"), &[(N::Stem, "IMG_1.xmp"), (N::Full, "IMG_1.CR3.xmp")], &mut n) {
            let naming = if n == N::Full { "full" } else { "stem" };
            let _ = app.run("library.xmpPreferences", json!({"naming": naming}));
        }
    }
    if !cfg!(target_arch = "wasm32") && !IOS {
        heading(ui, t, crate::i18n::tr("Auto Import"));
        hint(ui, t, crate::i18n::tr("Photos that arrive in this folder (tethering, a scanner, a sync app) are added as soon as they're complete."));
        row(ui, t, crate::i18n::tr("Watched folder"), |ui| {
            ui.label(RichText::new(d.auto_folder.clone().unwrap_or_else(|| "Off".into())).color(t.text));
            let can = app.services.pick_folder.is_some();
            let r = ui.add_enabled(can, egui::Button::new(crate::i18n::tr("Choose…")));
            register(ui.ctx(), "button:settingsAutoFolder", r.rect);
            if r.clicked()
                && let Some(f) = app.services.pick_folder.as_mut().and_then(|f| f())
                && let Err(e) = app.run("library.autoImport", json!({"folder": f}))
            {
                app.toast(ui.ctx(), e);
            }
            if d.auto_folder.is_some() && ui.button(crate::i18n::tr("Turn Off")).clicked() {
                let _ = app.run("library.autoImport", json!({"folder": null}));
            }
        });
        if d.auto_folder.is_some() {
            row(ui, t, "", |ui| {
                let mut copy = d.auto_copy;
                if crate::widgets::check(ui, &mut copy, crate::i18n::tr("Copy into the library (else use the files where they are)")).changed() {
                    let _ = app.run("library.autoImport", json!({"copy": copy}));
                }
            });
            row(ui, t, crate::i18n::tr("Album"), |ui| {
                let id = egui::Id::new("auto-album");
                let mut name = ui.data(|m| m.get_temp::<String>(id)).unwrap_or_else(|| d.auto_album.clone().unwrap_or_default());
                let r = ui.add(egui::TextEdit::singleline(&mut name).hint_text(crate::i18n::tr("None")).desired_width(180.0));
                if r.lost_focus() {
                    let _ = app.run("library.autoImport", json!({"album": name.trim()}));
                }
                ui.data_mut(|m| m.insert_temp(id, name));
            });
        }
    }
    if app.session.library.is_none() {
        hint(ui, t, crate::i18n::tr("In-memory session: these settings last until LightCraft quits."));
    }
}

// --------------------------------------------------------------------------------- Performance

fn performance_tab(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    use lightcraft_engine::gpu;
    heading(ui, t, crate::i18n::tr("Rendering"));
    check(ui, "settings.gpu", &mut app.ui.settings.gpu, "Use the GPU for rendering");
    let status = if !gpu::available() {
        match gpu::unavailable_reason() {
            Some(why) => crate::i18n::tr_format!("Rendering on the CPU: {why}", why = why),
            None => "No usable GPU found: rendering on the CPU".to_string(),
        }
    } else {
        format!("GPU: {}", gpu::adapter_name().unwrap_or_else(|| "starting…".into()))
    };
    hint(ui, t, &status);
    let opts: Vec<(u32, String)> = PREVIEW_EDGES.iter().map(|e| (*e, format!("{e} px"))).collect();
    let opts: Vec<(u32, &str)> = opts.iter().map(|(e, l)| (*e, l.as_str())).collect();
    pick(ui, t, "settingsPreview", crate::i18n::tr("Preview size"), &opts, &mut app.ui.settings.preview_edge);
    hint(ui, t, crate::i18n::tr("Largest long edge the Detail view renders at; larger is sharper on big displays but slower."));
    let auto = crate::i18n::tr_format!("Automatic ({} MB)", lightcraft_engine::memory::default_budget() >> 20);
    let opts = [(0u32, auto.as_str()), (512, "512 MB"), (1024, "1 GB"), (2048, "2 GB"), (4096, "4 GB")];
    pick(ui, t, "settingsMemory", crate::i18n::tr("Memory for caches"), &opts, &mut app.ui.settings.memory_mb);
    heading(ui, t, crate::i18n::tr("Thumbnail cache"));
    let cur = (app.session.cache_bytes() >> 20) as u32;
    let opts = [(512u32, "512 MB"), (1024, "1 GB"), (2048, "2 GB"), (4096, "4 GB"), (8192, "8 GB")];
    let mut v = if CACHE_SIZES.contains(&cur) { cur } else { 2048 };
    if pick(ui, t, "settingsCache", crate::i18n::tr("Size limit"), &opts, &mut v) {
        let _ = app.run("library.preferences", json!({"cacheMb": v}));
    }
    let used = app.session.media.rendered.disk().map(|d| d.size());
    let used = used.map(|b| format!("{:.1} MB", b as f64 / 1048576.0)).unwrap_or_else(|| "memory only".into());
    let clear = if crate::is_compact(ui.ctx()) {
        info(ui, t, crate::i18n::tr("In use"), &used, t.text);
        action(ui, t, "settingsClearCache", "Clear Cache", true, false)
    } else {
        row(ui, t, crate::i18n::tr("In use"), |ui| {
            ui.label(RichText::new(used).color(t.text));
            action(ui, t, "settingsClearCache", "Clear Cache", true, false)
        })
    };
    if clear {
        let _ = app.run("library.clearPreviews", json!({}));
    }
    // (no Local folders on iOS: an app sees only its own container)
    if IOS {
        return;
    }
    heading(ui, t, crate::i18n::tr("Local folders"));
    let opts = [(0u32, "Never"), (7, "After 7 days"), (30, "After 30 days"), (90, "After 90 days"), (365, "After a year")];
    let mut v = app.session.forget_local_days;
    if pick(ui, t, "settingsForgetLocal", crate::i18n::tr("Forget unchanged photos"), &opts, &mut v) {
        let _ = app.run("library.preferences", json!({"forgetLocalDays": v}));
    }
    hint(
        ui,
        t,
        crate::i18n::tr(
            "Photos seen in Local but never added or changed leave the catalog when their folder hasn't been browsed for this long (checked when the library opens). Files stay on disk; browsing the folder shows them again.",
        ),
    );
    #[cfg(not(target_arch = "wasm32"))]
    smart_previews(app, ui, t);
}

/// Where this library keeps its smart previews (the offline-editing proxies, which can be large):
/// the effective folder, what is in it, and choosing another one.
#[cfg(not(target_arch = "wasm32"))]
fn smart_previews(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    // listing a big folder every frame would be slow: refresh every 2 s and after a change
    let cache = egui::Id::new("smart-location");
    let now = ui.input(|i| i.time);
    let loc: Value = match ui.data(|d| d.get_temp::<(f64, Value)>(cache)) {
        Some((at, v)) if now - at < 2.0 => v,
        _ => {
            let v = app.run("library.smartPreviewsLocation", json!({})).unwrap_or(Value::Null);
            ui.data_mut(|d| d.insert_temp(cache, (now, v.clone())));
            v
        }
    };
    if loc.is_null() {
        return;
    }
    heading(ui, t, crate::i18n::tr("Smart previews"));
    let path = loc["path"].as_str().unwrap_or_default().to_string();
    let available = loc["available"].as_bool().unwrap_or(true);
    let custom = loc["custom"].as_bool().unwrap_or(false);
    row(ui, t, crate::i18n::tr("Folder"), |ui| {
        let text = format!("{path}{}", if custom { "" } else { "  (library default)" });
        ui.add(egui::Label::new(RichText::new(text.clone()).color(if available { t.text } else { t.reject })).truncate()).on_hover_text(text);
    });
    let (count, bytes) = (loc["count"].as_u64().unwrap_or(0), loc["bytes"].as_u64().unwrap_or(0));
    if available {
        hint(
            ui,
            t,
            &crate::i18n::tr_format!(
                "{count} smart preview{} · {:.1} MB",
                if count == 1 { "" } else { "s" },
                bytes as f64 / 1048576.0,
                count = count
            ),
        );
    } else {
        hint(
            ui,
            t,
            crate::i18n::tr(
                "This folder is not available (drive disconnected?). Smart previews are not built or used until it is back or you choose another folder.",
            ),
        );
    }
    // what happens to the previews already built when the folder changes
    let mode_id = egui::Id::new("smart-existing");
    let mut mode: u8 = ui.data(|d| d.get_temp(mode_id)).unwrap_or(0);
    if count > 0 {
        row(ui, t, crate::i18n::tr("Existing previews"), |ui| {
            if choices(ui, "settingsSmartExisting", &[(0u8, "Move them"), (1, "Leave them"), (2, "Delete them")], &mut mode) {
                ui.data_mut(|d| d.insert_temp(mode_id, mode));
            }
        });
    }
    let existing = ["move", "leave", "discard"][mode.min(2) as usize];
    let mut result = None;
    ui.horizontal(|ui| {
        ui.add_space(LABEL_W);
        if app.services.pick_folder.is_some() {
            let r = ui.button(crate::i18n::tr("Choose Folder…"));
            register(ui.ctx(), "button:settingsSmartChoose", r.rect);
            if r.clicked()
                && let Some(dir) = app.services.pick_folder.as_mut().and_then(|f| f())
            {
                result = Some(app.run("library.smartPreviewsLocation", json!({"path": dir, "existing": existing})));
            }
        }
        let r = ui.add_enabled(custom, egui::Button::new(crate::i18n::tr("Use Library Folder")));
        register(ui.ctx(), "button:settingsSmartReset", r.rect);
        if r.clicked() {
            result = Some(app.run("library.smartPreviewsLocation", json!({"reset": true, "existing": existing})));
        }
    });
    if let Some(r) = result {
        ui.data_mut(|d| d.remove::<(f64, Value)>(cache));
        match r {
            Ok(v) => {
                let failed = v["failed"].as_array().map_or(0, Vec::len);
                let msg = match (existing, v["handled"].as_u64().unwrap_or(0)) {
                    (_, 0) => "Smart previews folder changed".to_string(),
                    ("move", n) => format!("Smart previews folder changed; moved {n}"),
                    ("discard", n) => format!("Smart previews folder changed; deleted {n}"),
                    _ => "Smart previews folder changed".to_string(),
                };
                app.toast(ui.ctx(), if failed > 0 { crate::i18n::tr_format!("{msg} ({failed} failed)", failed = failed, msg = msg) } else { msg });
            }
            Err(e) => app.toast(ui.ctx(), e),
        }
    }
    hint(ui, t, crate::i18n::tr("Keep it on a drive with room: smart previews are about 1 MB per photo. The thumbnail cache stays in the library."));
}

// ---------------------------------------------------------------------------------- Interface

fn interface_tab(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    // (the compact layout has no filmstrip)
    if !crate::is_compact(ui.ctx()) {
        heading(ui, t, crate::i18n::tr("Filmstrip"));
        check(ui, "settings.filmNames", &mut app.ui.settings.film_names, "Show file names");
        check(ui, "settings.filmBadges", &mut app.ui.settings.film_badges, "Show ratings, flags and edit badges");
    }
    heading(ui, t, crate::i18n::tr("Grid"));
    pick(
        ui,
        t,
        "settingsGridBadges",
        crate::i18n::tr("Ratings & flags"),
        &[(GridBadges::Auto, "When rated or hovered"), (GridBadges::Always, "Always"), (GridBadges::Never, "Never")],
        &mut app.ui.settings.grid_badges,
    );
    check(ui, "settings.showFilenames", &mut app.ui.show_filenames, "Square Grid: show file names and formats");
    heading(ui, t, crate::i18n::tr("Detail"));
    check(ui, "settings.navigator", &mut app.ui.navigator, "Show the Navigator while zoomed in");
    use crate::state::InfoOverlay as I;
    pick(
        ui,
        t,
        "settingsInfo",
        crate::i18n::tr("Info overlay"),
        &[(I::Off, "Off"), (I::Basic, "File & date"), (I::Exposure, "Exposure")],
        &mut app.ui.info_overlay,
    );
}

// ---------------------------------------------------------------------------------------- Sync

/// Run a sync command, showing what went wrong.
fn sync_cmd(app: &mut LightcraftApp, ui: &egui::Ui, id: &str, p: Value) {
    if let Err(e) = app.run(id, p) {
        app.toast(ui.ctx(), e);
    }
}

fn sync_tab(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    heading(ui, t, crate::i18n::tr("Sync"));
    hint(
        ui,
        t,
        crate::i18n::tr(
            "Share this library — photos, edits, albums and presets — with your other devices through your own LightCraft server (see docs/sync.md). Without one, everything stays on this computer.",
        ),
    );
    if app.services.sync_exec.is_none() {
        hint(ui, t, crate::i18n::tr("Sync isn't available here yet."));
        return;
    }
    if app.session.library.is_none() {
        hint(ui, t, crate::i18n::tr("Sync needs a saved library: open or create one in General first."));
        return;
    }
    let status = app.session.sync_state().map(|st| (st.status(), st.signed_in(), st.config.clone()));
    match status {
        Some((s, true, c)) => {
            if crate::is_compact(ui.ctx()) {
                heading(ui, t, crate::i18n::tr("Account"));
            }
            info(ui, t, crate::i18n::tr("Server"), &c.server, t.text);
            info(ui, t, crate::i18n::tr("User"), &c.user, t.text);
            let (tip, problem, _) = crate::sync_ui::cloud_status(app);
            let r = info(ui, t, crate::i18n::tr("Status"), &tip, if problem { t.caution } else { t.text });
            register(ui.ctx(), "label:syncStatus", r);
            row(ui, t, "", |ui| {
                if action(ui, t, "syncNow", "Sync Now", true, false) {
                    sync_cmd(app, ui, "sync.now", json!({}));
                }
                if action(ui, t, "syncPause", if c.paused { "Resume Syncing" } else { "Pause Syncing" }, true, false) {
                    sync_cmd(app, ui, "sync.pause", json!({"on": !c.paused}));
                }
                if crate::is_compact(ui.ctx()) {
                    // (iOS: signing out on a card of its own)
                    card_end(ui, t);
                    ui.add_space(28.0);
                }
                if action(ui, t, "syncSignOut", "Sign Out", true, true) {
                    sync_cmd(app, ui, "sync.signOut", json!({}));
                }
            });
            storage_section(app, ui, t, &c.server);
            heading(ui, t, crate::i18n::tr("On This Device"));
            let mut keep = c.store_originals;
            if check(ui, "sync.storeOriginals", &mut keep, "Store the originals of all photos on this device") {
                sync_cmd(app, ui, "sync.storeOriginalsLocally", json!({"on": keep}));
            }
            hint(
                ui,
                t,
                crate::i18n::tr(
                    "Otherwise photos from other devices come down as previews: small ones for the grid, an editable smart preview when you open a photo or make an album available offline (right-click it), and the original when you ask (Photo > Download Originals).",
                ),
            );
            let offline = s["offlineAlbums"].as_array().map_or(0, Vec::len);
            if offline > 0 {
                hint(ui, t, &format!("{offline} album(s) available offline"));
            }
        }
        other => {
            let known = other.map(|(_, _, c)| c);
            if let Some(c) = &known {
                if app.sync.form.server.is_empty() {
                    app.sync.form.server = c.server.clone();
                    app.sync.form.user = c.user.clone();
                }
                if let Some(e) = app.session.sync_state().and_then(|st| st.error()) {
                    info(ui, t, crate::i18n::tr("Status"), e, t.caution);
                }
            }
            if crate::is_compact(ui.ctx()) {
                heading(ui, t, crate::i18n::tr("Account"));
            }
            let mut form = std::mem::take(&mut app.sync.form);
            text_row(ui, t, "syncServer", crate::i18n::tr("Server"), &mut form.server, "https://photos.example.com", false, 260.0);
            text_row(
                ui,
                t,
                "syncUser",
                crate::i18n::tr("User"),
                &mut form.user,
                if crate::is_compact(ui.ctx()) { "Required" } else { "" },
                false,
                260.0,
            );
            text_row(
                ui,
                t,
                "syncPassword",
                crate::i18n::tr("Password"),
                &mut form.password,
                if crate::is_compact(ui.ctx()) { "Required" } else { "" },
                true,
                260.0,
            );
            if crate::is_compact(ui.ctx()) {
                card_end(ui, t);
                ui.add_space(28.0);
            }
            row(ui, t, "", |ui| {
                let ready = !form.server.trim().is_empty() && !form.user.trim().is_empty() && !form.password.is_empty();
                if action(ui, t, "syncSignIn", "Sign In", ready, false) {
                    let p = json!({"server": form.server.trim(), "user": form.user.trim(), "password": form.password});
                    form.password.clear();
                    sync_cmd(app, ui, "sync.signIn", p);
                }
            });
            app.sync.form = form;
            hint(
                ui,
                t,
                crate::i18n::tr(
                    "The first library signed in uploads its photos to the empty server. To get them on another computer, sign in from a new, empty library there.",
                ),
            );
        }
    }
}

/// A disk's bar: what this library takes of it, what else is on it, what is free.
fn disk_bar(ui: &mut egui::Ui, t: &Tokens, library: u64, disk: lightcraft_catalog::sync::proto::Disk) {
    use crate::sync_ui::bytes_label;
    let used = disk.total.saturating_sub(disk.free);
    let legend = format!(
        "{}: {}\n{}: {}\n{}: {}",
        crate::i18n::tr("LightCraft"),
        bytes_label(library),
        crate::i18n::tr("Everything else"),
        bytes_label(used.saturating_sub(library)),
        crate::i18n::tr("Free"),
        bytes_label(disk.free)
    );
    let paint = |ui: &mut egui::Ui, r: Rect| {
        let total = disk.total.max(1) as f32;
        let used = disk.total.saturating_sub(disk.free);
        let ours = library.min(used);
        let at = |bytes: u64| r.left() + r.width() * (bytes as f32 / total).clamp(0.0, 1.0);
        let p = ui.painter();
        p.rect_filled(r, 4.0, t.track);
        p.rect_filled(Rect::from_min_max(r.min, pos2(at(used), r.bottom())), 4.0, t.text_disabled);
        // (at least a sliver, so a small library still shows)
        let ours_w = (at(ours) - r.left()).max(if ours > 0 { 2.0 } else { 0.0 });
        p.rect_filled(Rect::from_min_size(r.min, vec2(ours_w, r.height())), 4.0, t.accent);
    };
    if crate::is_compact(ui.ctx()) {
        let (r, _) = tap_row(ui, t, "", "", 24.0, 16.0, false);
        paint(ui, Rect::from_center_size(r.center(), vec2(r.width() - 32.0, 8.0)));
        return;
    }
    row(ui, t, "", |ui| {
        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width().min(300.0), 8.0), Sense::hover());
        paint(ui, r);
        resp.on_hover_text(legend);
    });
}

/// Settings ▸ Sync ▸ Storage: what this library takes on the server (as the server last said) and
/// on this computer (measured by a worker thread, see [`crate::sync_ui::storage_poll`]).
fn storage_section(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens, server: &str) {
    use crate::sync_ui::{bytes_label, count_label};
    let files = |f: lightcraft_catalog::sync::proto::Files| {
        let n = count_label(f.files);
        format!("{} · {n} {}", bytes_label(f.bytes), if f.files == 1 { "file" } else { "files" })
    };
    heading(ui, t, crate::i18n::tr("Storage"));
    let mut refresh = false;
    let age = app.session.sync_state().and_then(|st| st.usage_age());
    row(ui, t, "", |ui| {
        if action(ui, t, "syncUsageRefresh", "Refresh", true, false) {
            refresh = true;
        }
        if !crate::is_compact(ui.ctx())
            && let Some(a) = age
        {
            let text = if a.as_secs() < 3 { crate::i18n::tr("Updated just now").to_string() } else { format!("Updated {} s ago", a.as_secs()) };
            ui.label(RichText::new(text).size(11.0).color(t.text_dim));
        }
    });
    crate::sync_ui::storage_poll(app, ui.ctx(), refresh);

    let host = server.trim_start_matches("https://").trim_start_matches("http://").trim_end_matches('/');
    heading(ui, t, &format!("{} {host}", crate::i18n::tr("On the server")));
    let (usage, error) = match app.session.sync_state() {
        Some(st) => (st.usage().cloned(), st.usage_error().map(str::to_string)),
        None => (None, None),
    };
    match &usage {
        Some(u) => {
            info(ui, t, crate::i18n::tr("Originals"), &files(u.original), t.text);
            info(ui, t, crate::i18n::tr("Smart previews"), &files(u.smart), t.text);
            info(ui, t, crate::i18n::tr("Small previews"), &files(u.mini), t.text);
            info(ui, t, crate::i18n::tr("Total"), &bytes_label(u.stored()), t.text);
            if u.folders.files > 0 {
                info(ui, t, crate::i18n::tr("Library folders"), &files(u.folders), t.text);
            }
            if let Some(d) = u.disk {
                let text = format!("{} free of {}", bytes_label(d.free), bytes_label(d.total));
                info(ui, t, crate::i18n::tr("Server disk"), &text, t.text);
                disk_bar(ui, t, u.stored(), d);
            }
            if u.folders.files > 0 {
                hint(
                    ui,
                    t,
                    crate::i18n::tr("Library folders are read where they are on the server and never copied, so they aren't part of the total."),
                );
            }
        }
        None if error.is_none() => hint(ui, t, crate::i18n::tr("Asking the server…")),
        None => {}
    }
    if let Some(e) = &error {
        hint(ui, t, &format!("{}: {e}", crate::i18n::tr("The server can't say how much it stores")));
    }

    let Some(l) = app.session.local_dirs().and(app.sync.storage.local.clone()) else { return };
    let (photos, remote) = app.sync.storage.counts;
    heading(ui, t, crate::i18n::tr("On this computer"));
    let text = if remote == 0 {
        format!("{} · all with their original here", count_label(photos as u64))
    } else {
        format!("{} · {} with only previews here", count_label(photos as u64), count_label(remote as u64))
    };
    info(ui, t, crate::i18n::tr("Photos"), &text, t.text);
    info(ui, t, crate::i18n::tr("Originals"), &files(l.downloaded), t.text);
    info(ui, t, crate::i18n::tr("Smart previews"), &files(l.smart), t.text);
    info(ui, t, crate::i18n::tr("Small previews"), &files(l.mini), t.text);
    info(ui, t, crate::i18n::tr("Thumbnails"), &files(l.thumbnails), t.text);
    if l.imported.files > 0 {
        info(ui, t, crate::i18n::tr("Imported copies"), &files(l.imported), t.text);
    }
    info(ui, t, crate::i18n::tr("Catalog and edits"), &bytes_label(l.library), t.text);
    info(ui, t, crate::i18n::tr("Total"), &bytes_label(l.total()), t.text);
    if let Some(d) = l.disk {
        let text = format!("{} free of {}", bytes_label(d.free), bytes_label(d.total));
        info(ui, t, crate::i18n::tr("This disk"), &text, t.text);
        disk_bar(ui, t, l.total(), d);
    }
    hint(
        ui,
        t,
        crate::i18n::tr(
            "Originals are the full-size files downloaded from the server. Photos you imported from folders on this computer stay where they are and aren't counted. Thumbnails can be deleted at any time; they are drawn again.",
        ),
    );
}

// ------------------------------------------------------------------------------- Open Library…

/// `app.openLibrary {path?}`: close the current library and open (or create) the one at `path`,
/// or a folder chosen in a dialog. Remembered as the library to open at launch.
pub fn open_library(app: &mut LightcraftApp, p: &Value) -> Result<Value, String> {
    let path = match p.get("path").and_then(Value::as_str) {
        Some(x) => x.to_string(),
        None => match app.services.pick_folder.as_mut() {
            Some(pick) => match pick() {
                Some(x) => x,
                None => return Ok(Value::Null),
            },
            None => return Err("no folder dialog on this platform".into()),
        },
    };
    app.session.close_library().map_err(|e| e.to_string())?;
    app.session.open_library(&path, false).map_err(|e| e.to_string())?;
    app.renderer.forget_all();
    app.ui.compare = None;
    app.ui.settings.library_path = path.clone();
    Ok(json!({"path": path, "photos": app.session.catalog.len()}))
}
