//! The Settings dialog (⌘,): General, Import, Performance, Display, Interface, Slideshow, Faces,
//! AI Denoise.
//!
//! Changes apply immediately (no OK/Cancel). Where they are stored:
//! - **app settings** ([`crate::state::AppSettings`]: startup view, delete confirmation, GPU,
//!   preview size, filmstrip/grid badges, last library, monitor profile, whether Import Lightroom
//!   Catalog… asks where a catalog goes) and Auto Advance live in
//!   the UI state,
//!   saved by the host in its config folder (`ui.json`);
//! - **library settings** (import defaults, XMP sidecars, thumbnail cache size, slideshow
//!   playback) go through the `library.preferences` / `library.xmpPreferences` /
//!   `slideshow.playback` commands into the library's `prefs.json`,
//!   so they travel with the library.

use egui::{Align, Color32, Layout, Margin, Rect, RichText, Sense, Stroke, pos2, vec2};
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::icons::Icon;
use crate::panels::mobile;
use crate::state::{Appearance, GridBadges, PREVIEW_LIMITS, StartupView};
use crate::sync_ui::SignOutHow;
use crate::theme::Tokens;
use crate::widgets::register;

mod appearance;
mod display;
mod general;
mod import;
mod libraries;
mod performance;
mod slideshow;
mod widgets;

pub use display::display_profile;
pub(super) use display::view_options;
use display::{display_tab, interface_tab};
use general::general_tab;
use import::import_tab;
pub(crate) use libraries::{can_switch_library, free_name};
pub use libraries::{forget_library, library_name, new_library, open_library, open_library_name, recent_libraries, remember_library};
use performance::performance_tab;
use slideshow::slideshow_tab;
pub(crate) use widgets::*;
#[allow(unused_imports)]
pub(crate) use appearance::*;

pub mod slide_layout;
pub mod slide_overlays;
pub mod slide_templates;
pub mod slide_titles;

/// (id, label) of the tabs, in order.
pub const TABS: &[(&str, &str)] = &[
    ("general", "General"),
    ("import", "Import"),
    ("performance", "Performance"),
    ("display", "Display"),
    ("interface", "Interface"),
    ("slideshow", "Slideshow"),
    ("faces", "Faces"),
    ("denoise", "AI Denoise"),
    ("sync", "Sync"),
    ("models", "AI Models"),
];

/// Thumbnail cache sizes offered (MB).
const CACHE_SIZES: [u32; 5] = [512, 1024, 2048, 4096, 8192];

const LABEL_W: f32 = 150.0;

/// The dialog body for `tab`: on the desktop the sidebar ([`sidebar`], which switches `tab`) beside
/// the open section, which scrolls on its own within [`desktop_height`]`(max_height)`.
pub fn body(app: &mut LightcraftApp, ui: &mut egui::Ui, tab: &mut String, max_height: f32) {
    if crate::is_compact(ui.ctx()) {
        phone_body(app, ui, tab);
        return;
    }
    let t = Tokens::get(ui.ctx());
    let width = (ui.ctx().content_rect().width() - 80.0).clamp(560.0, 860.0);
    let (outer, _) = ui.allocate_exact_size(vec2(width, desktop_height(max_height)), Sense::hover());
    let side = Rect::from_min_size(outer.min, vec2(SIDEBAR_W, outer.height()));
    ui.scope_builder(egui::UiBuilder::new().max_rect(side), |ui| {
        egui::ScrollArea::vertical().id_salt("settings-sidebar").auto_shrink([false, false]).show(ui, |ui| sidebar(app, ui, &t, tab));
    });
    let line_x = (side.right() + 12.0).round() + 0.5;
    ui.painter().vline(line_x, outer.y_range(), Stroke::new(1.0, t.divider));
    let page = Rect::from_min_max(pos2(line_x + 1.0, outer.top()), outer.max);
    ui.scope_builder(egui::UiBuilder::new().max_rect(page), |ui| {
        let pinned = pin_slide_preview(app, ui, &t, tab.as_str(), page.height());
        // (each section starts at its top)
        let scrolled = egui::ScrollArea::vertical().id_salt(("settings-page", tab.clone())).auto_shrink([false, false]).show(ui, |ui| {
            egui::Frame::NONE.inner_margin(Margin { left: 20 + CARD_PAD as i8, right: 10 + CARD_PAD as i8, top: 0, bottom: 12 }).show(ui, |ui| {
                ui.set_width(ui.available_width());
                if !pinned {
                    page_header(ui, &t, tab.as_str());
                }
                let column = Column { left: ui.max_rect().left(), right: ui.max_rect().right(), top: ui.cursor().top() };
                ui.data_mut(|d| {
                    d.remove::<Card>(card_id());
                    d.insert_temp(column_id(), column);
                });
                ui.spacing_mut().item_spacing.y = 4.0;
                match tab.as_str() {
                    "import" => import_tab(app, ui, &t),
                    "performance" => performance_tab(app, ui, &t),
                    "display" => display_tab(app, ui, &t),
                    "interface" => interface_tab(app, ui, &t),
                    "slideshow" => slideshow_tab(app, ui, &t, !pinned),
                    "faces" => crate::panels::faces::settings_tab(app, ui, &t),
                    "denoise" => crate::panels::denoise::settings_tab(app, ui, &t),
                    "sync" => sync_tab(app, ui, &t),
                    "models" => crate::panels::ai_models::tab(app, ui, &t),
                    _ => general_tab(app, ui, &t),
                }
                card_end(ui, &t);
                ui.data_mut(|d| d.remove::<Column>(column_id()));
                // (what the page took: wider than the room for it would mean a control is cut off)
                register(ui, "settings:page", ui.min_rect());
            });
        });
        register(ui, "settings:pageRoom", scrolled.inner_rect);
    });
}

/// The least room in which the page below the pinned preview still scrolls.
pub(crate) const PAGE_MIN_HEIGHT: f32 = 120.0;

/// On Slideshow, the section's title and the slide preview stay above the page, not in it:
/// hovering a template in the Template Browser previews it there, so it has to stay in view however
/// far down the templates are scrolled to. It takes [`PINNED_PREVIEW_SHARE`] of the `height` of the
/// room (at least the least tall slide); where that leaves less than [`PAGE_MIN_HEIGHT`] for the
/// page, it scrolls with the page instead. Whether it was pinned.
pub(crate) fn pin_slide_preview(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens, tab: &str, height: f32) -> bool {
    if tab != "slideshow" {
        return false;
    }
    let slide_h = (height * PINNED_PREVIEW_SHARE - PINNED_CHROME).max(crate::panels::slide_layout::PREVIEW_MIN_H);
    if height - (slide_h + PINNED_CHROME) < PAGE_MIN_HEIGHT {
        return false;
    }
    egui::Frame::NONE.inner_margin(Margin { left: 20 + CARD_PAD as i8, right: 10 + CARD_PAD as i8, top: 0, bottom: 0 }).show(ui, |ui| {
        ui.set_width(ui.available_width());
        page_header(ui, t, tab);
        crate::panels::slide_layout::preview(app, ui, t, slide_h);
    });
    ui.add_space(2.0);
    ui.separator();
    true
}

/// The desktop sidebar: a row per section, its icon on a coloured tile (as on the phone's list), the
/// open one highlighted, a line between the groups ([`SIDEBAR`]). A click opens the section
/// (`button:settingsTab-{id}`); Sync says whether it is on.
pub(crate) fn sidebar(app: &LightcraftApp, ui: &mut egui::Ui, t: &Tokens, tab: &mut String) {
    let open = if TABS.iter().any(|(id, _)| id == tab) { tab.clone() } else { "general".to_string() };
    ui.spacing_mut().item_spacing.y = 2.0;
    for (g, group) in SIDEBAR.iter().enumerate() {
        if g > 0 {
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 13.0), Sense::hover());
            ui.painter().hline((r.left() + 8.0)..=(r.right() - 8.0), r.center().y, Stroke::new(1.0, t.divider));
        }
        for &id in group.iter().filter(|id| !(IOS && **id == "display")) {
            let Some(i) = TABS.iter().position(|(tid, _)| *tid == id) else { continue };
            let (Some((_, label)), Some((icon, tile))) = (TABS.get(i), TAB_TILES.get(i)) else { continue };
            let label = crate::i18n::tr(label);
            let on = open == id;
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
            register(ui, format!("button:settingsTab-{id}"), r);
            resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, on, label));
            let radius = f32::from(t.control_radius) + 2.0;
            if on {
                ui.painter().rect_filled(r, radius, t.tool_active);
            } else if resp.hovered() {
                ui.painter().rect_filled(r, radius, t.hover.gamma_multiply(0.6));
            }
            let tile_r = Rect::from_min_size(pos2(r.left() + 8.0, r.center().y - 10.0), vec2(20.0, 20.0));
            ui.painter().rect_filled(tile_r, 5.0, *tile);
            crate::icons::paint(ui.painter(), tile_r.shrink(3.5), *icon, Color32::WHITE);
            let value = if id == "sync" { sync_value(app) } else { "" };
            let value = line(ui, value, 11.5, t.text_dim, 70.0);
            let text_x = tile_r.right() + 10.0;
            let max_w = r.right() - 8.0 - value.size().x - 6.0 - text_x;
            let font = if on { t.semibold(13.0) } else { t.font(13.0) };
            let name = egui::WidgetText::from(RichText::new(label).font(font)).into_galley(
                ui,
                Some(egui::TextWrapMode::Truncate),
                max_w.max(20.0),
                egui::TextStyle::Body,
            );
            ui.painter().galley(pos2(text_x, r.center().y - name.size().y / 2.0), name, if on { t.text } else { t.text_label });
            ui.painter().galley(pos2(r.right() - 8.0 - value.size().x, r.center().y - value.size().y / 2.0), value, t.text_dim);
            if resp.clicked() {
                *tab = id.to_string();
            }
        }
    }
}

/// Whether this library syncs (the Sync section's value in the sidebar and on the phone's list).
pub(crate) fn sync_value(app: &LightcraftApp) -> &'static str {
    if app.session.sync_conflict().is_some() {
        crate::i18n::tr("Choose…")
    } else if app.session.sync_state().is_some_and(|s| s.signed_in()) || app.session.icloud_folder().is_some() {
        crate::i18n::tr("On")
    } else {
        crate::i18n::tr("Off")
    }
}

/// A desktop section's title, and a line saying what it is for.
pub(crate) fn page_header(ui: &mut egui::Ui, t: &Tokens, tab: &str) {
    let title = TABS.iter().find(|(id, _)| *id == tab).map_or("General", |(_, label)| *label);
    ui.add_space(2.0);
    ui.label(RichText::new(crate::i18n::tr(title)).font(t.semibold(20.0)).color(t.text));
    ui.label(RichText::new(crate::i18n::tr(blurb(tab))).size(12.5).color(t.text_dim));
    ui.add_space(10.0);
}

/// What a section is for, under its title on the desktop.
pub(crate) fn blurb(tab: &str) -> &'static str {
    match tab {
        "import" => "How new photos start out: their develop settings, metadata and sidecars, and a folder to watch.",
        "performance" => "Rendering, previews and the caches that keep browsing quick.",
        "display" => "Show colours accurately on your monitor.",
        "interface" => "How LightCraft looks, and what the grid, filmstrip and photo view show.",
        "slideshow" => "Playback, layout, overlays and titles for slideshows, and their export.",
        "faces" => "Find the people in your photos and suggest who they are. It all happens on this computer.",
        "denoise" => "Clean the noise out of raw photos with an AI model that runs on this computer.",
        "models" => "Download, turn on and remove the optional AI models.",
        "sync" => "Keep this library in step with your other devices through your own LightCraft server.",
        _ => "Language, your library, how LightCraft starts, and the editors it hands photos to.",
    }
}

/// The phone's Settings: the list of sections, or the one `tab` names, sliding in from the side
/// it comes from (a section from the right, the list back from the left).
pub(crate) fn phone_body(app: &mut LightcraftApp, ui: &mut egui::Ui, tab: &mut String) {
    let t = Tokens::get(ui.ctx());
    let k = mobile::enter(ui.ctx(), egui::Id::unique("settings-page-in"), tab.as_str(), 0.25);
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
            "display" => display_tab(app, ui, &t),
            "interface" => interface_tab(app, ui, &t),
            "slideshow" => slideshow_tab(app, ui, &t, true),
            "faces" => crate::panels::faces::settings_tab(app, ui, &t),
            "denoise" => crate::panels::denoise::settings_tab(app, ui, &t),
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
pub(crate) fn list(app: &LightcraftApp, ui: &mut egui::Ui, t: &Tokens, tab: &mut String) {
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
    for ((id, label), (icon, tile)) in TABS.iter().zip(TAB_TILES).filter(|((id, _), _)| !(IOS && *id == "display")) {
        let value = match *id {
            "sync" => sync_value(app),
            "general" => app.ui.language.name(),
            _ => "",
        };
        if nav_row(ui, t, &format!("settingsTab-{id}"), icon, tile, crate::i18n::tr(label), None, value) {
            *tab = id.to_string();
        }
    }
}

/// The rows sit on a rounded card: the first row after a heading or (on the phone) a note starts
/// one, whose background [`card_end`] draws behind them once the run is over.
#[derive(Clone, Copy)]
pub(crate) struct Card {
    shape: egui::layers::ShapeIdx,
    top: f32,
    /// Where the last row started (a row asked twice, by a row and the switch on it, is one).
    last: f32,
}

pub(crate) fn card_id() -> egui::Id {
    egui::Id::unique("lc-settings-card")
}

/// Where the open desktop section's rows go, left and right (cards reach [`CARD_PAD`] past them),
/// and where its settings start (under its title). Set only while a desktop section is drawn: the
/// helpers below lay rows out on cards then, as they always did otherwise.
#[derive(Clone, Copy)]
pub(crate) struct Column {
    left: f32,
    right: f32,
    top: f32,
}

pub(crate) fn column_id() -> egui::Id {
    egui::Id::unique("lc-settings-column")
}

pub(crate) fn column(ui: &egui::Ui) -> Option<Column> {
    if crate::is_compact(ui.ctx()) {
        return None;
    }
    ui.data(|d| d.get_temp::<Column>(column_id()))
}

/// Before a row: start a card, or draw the line between this row and the one above (on the phone a
/// hairline from `inset`, where the row's text starts, to the card's edge, as iOS does; on the
/// desktop a gap in the dialog's colour across the card).
pub(crate) fn card_row(ui: &mut egui::Ui, t: &Tokens, inset: f32) {
    let desktop = column(ui);
    // (a desktop row outside a Settings section: no card)
    if desktop.is_none() && !crate::is_compact(ui.ctx()) {
        return;
    }
    let y = ui.cursor().top();
    match ui.data(|d| d.get_temp::<Card>(card_id())) {
        Some(card) if (y - card.last).abs() < 0.5 => {}
        Some(card) => {
            match desktop {
                Some(col) => {
                    ui.painter().hline(col.left..=(col.right + CARD_PAD), y - 2.0, Stroke::new(1.0, t.chrome));
                }
                None => {
                    let r = ui.max_rect();
                    ui.painter().hline((r.left() + inset)..=r.right(), y, Stroke::new(0.5, t.divider));
                }
            }
            ui.data_mut(|d| d.insert_temp(card_id(), Card { last: y, ..card }));
        }
        None => {
            let shape = ui.painter().add(egui::Shape::Noop);
            ui.data_mut(|d| d.insert_temp(card_id(), Card { shape, top: y, last: y }));
        }
    }
}

/// Close the card the rows above are on (if any): its rounded background goes behind them.
pub(crate) fn card_end(ui: &mut egui::Ui, t: &Tokens) {
    let Some(card) = ui.data(|d| d.get_temp::<Card>(card_id())) else { return };
    ui.data_mut(|d| d.remove::<Card>(card_id()));
    if let Some(col) = column(ui) {
        let r = Rect::from_min_max(pos2(col.left - CARD_PAD, card.top - 4.0), pos2(col.right + CARD_PAD, ui.cursor().top()));
        ui.painter().set(card.shape, egui::epaint::RectShape::filled(r, f32::from(t.control_radius) + 4.0, t.inset));
        return;
    }
    let r = Rect::from_min_max(pos2(ui.max_rect().left(), card.top), pos2(ui.max_rect().right(), ui.cursor().top()));
    // (iOS's grouped cards, as round as iOS 26 draws them: the sheet colour in the dark
    // appearance, white on the light page)
    ui.painter().set(card.shape, egui::epaint::RectShape::filled(r, 20.0, t.cell_selected));
}

/// A whole phone row that reacts to a tap (`button:{id}` unless `id` is empty), lit while pressed.
pub(crate) fn tap_row(ui: &mut egui::Ui, t: &Tokens, id: &str, label: &str, h: f32, inset: f32, enabled: bool) -> (Rect, egui::Response) {
    card_row(ui, t, inset);
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), h), if enabled { Sense::click() } else { Sense::hover() });
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, label));
    if !id.is_empty() {
        register(ui, format!("button:{id}"), r);
    }
    if enabled && resp.is_pointer_button_down_on() {
        ui.painter().rect_filled(r, 0.0, t.hover);
    }
    (r, resp)
}

/// `text` as one line of at most `max_w` points (cut with "…").
pub(crate) fn line(ui: &egui::Ui, text: &str, size: f32, color: Color32, max_w: f32) -> std::sync::Arc<egui::Galley> {
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
pub(crate) fn nav_row(ui: &mut egui::Ui, t: &Tokens, id: &str, icon: Icon, tile: Color32, label: &str, sub: Option<&str>, value: &str) -> bool {
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
pub(crate) fn menu_row(ui: &mut egui::Ui, t: &Tokens, id: &str, label: &str, value: &str, items: impl FnOnce(&mut egui::Ui)) {
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
pub(crate) fn option(ui: &mut egui::Ui, id: &str, i: usize, selected: bool, label: &str) -> bool {
    if crate::is_compact(ui.ctx()) {
        mobile::row_checked(ui, &format!("{id}-{i}"), None, label, true, Some(selected))
    } else {
        ui.selectable_label(selected, label).clicked()
    }
}

/// A choice from a list (`option`s, `group`s): a combo box beside the label on the desktop
/// (`combo:{id}`), a pull-down menu on the phone.
pub(crate) fn select(ui: &mut egui::Ui, t: &Tokens, id: &str, label: &str, value: String, items: impl FnOnce(&mut egui::Ui)) {
    if crate::is_compact(ui.ctx()) {
        menu_row(ui, t, id, label, &value, items);
        return;
    }
    row(ui, t, label, |ui| {
        let r = crate::widgets::combo_box(id).width(220.0).selected_text(value).show_ui(ui, items);
        crate::widgets::name_combo(&r.response, label);
        register(ui, format!("combo:{id}"), r.response.rect);
    });
}

/// A labelled choice among a few `options`: mutually exclusive buttons on the desktop
/// (`button:{id}-{i}`; a combo box `combo:{id}` when there are more than [`PICK_BUTTONS`]), a
/// pull-down menu on the phone. True when it changed.
pub(crate) fn pick<V: PartialEq + Copy>(ui: &mut egui::Ui, t: &Tokens, id: &str, label: &str, options: &[(V, &str)], value: &mut V) -> bool {
    if !crate::is_compact(ui.ctx()) && options.len() <= PICK_BUTTONS {
        return row(ui, t, label, |ui| choices(ui, id, options, value));
    }
    let current = options.iter().find(|(v, _)| *v == *value).map(|(_, l)| crate::i18n::tr(l)).unwrap_or_default();
    let mut chosen = None;
    let items = |ui: &mut egui::Ui| {
        for (i, (v, l)) in options.iter().enumerate() {
            if option(ui, id, i, *v == *value, crate::i18n::tr(l)) {
                chosen = Some(*v);
            }
        }
    };
    if crate::is_compact(ui.ctx()) {
        menu_row(ui, t, id, label, current, items);
    } else {
        select(ui, t, id, label, current.to_string(), items);
    }
    match chosen {
        Some(v) if v != *value => {
            *value = v;
            true
        }
        _ => false,
    }
}

/// A label and its value: beside each other on the desktop, the value on the right on the phone.
/// Nothing is ever cut off: a value that doesn't fit in the rest of the row wraps (on the phone it
/// moves to lines of its own under the label, as iOS's long values do).
pub(crate) fn info(ui: &mut egui::Ui, t: &Tokens, label: &str, value: &str, color: Color32) -> Rect {
    if !crate::is_compact(ui.ctx()) {
        return row(ui, t, label, |ui| ui.add(egui::Label::new(RichText::new(value).color(color)).wrap()).rect);
    }
    let value_color = if color == t.text { t.text_dim } else { color };
    let label_text = crate::i18n::tr(label);
    let width = ui.available_width();
    let label_galley = ui.painter().layout_no_wrap(label_text.to_string(), t.font(17.0), t.text);
    let one_line = ui.painter().layout_no_wrap(value.to_string(), t.font(17.0), value_color);
    // beside the label when it all fits on one line (with room for the margins and a gap)
    if label_galley.size().x + 12.0 + one_line.size().x + 32.0 <= width {
        let (r, _) = tap_row(ui, t, "", label_text, ROW, 16.0, false);
        ui.painter().galley(pos2(r.left() + 16.0, r.center().y - label_galley.size().y / 2.0), label_galley, t.text);
        let vr = Rect::from_min_size(pos2(r.right() - 16.0 - one_line.size().x, r.center().y - one_line.size().y / 2.0), one_line.size());
        ui.painter().galley(vr.min, one_line, value_color);
        return vr;
    }
    // otherwise the label, then the value wrapped over the whole width under it
    const PAD: f32 = 10.0;
    let wrapped = ui.painter().layout(value.to_string(), t.font(15.0), value_color, (width - 32.0).max(40.0));
    let h = (PAD + label_galley.size().y + 4.0 + wrapped.size().y + PAD).max(ROW);
    let (r, _) = tap_row(ui, t, "", label_text, h, 16.0, false);
    ui.painter().galley(pos2(r.left() + 16.0, r.top() + PAD), label_galley.clone(), t.text);
    let vr = Rect::from_min_size(pos2(r.left() + 16.0, r.top() + PAD + label_galley.size().y + 4.0), wrapped.size());
    ui.painter().galley(vr.min, wrapped, value_color);
    vr
}

/// A button (`button:{id}`): beside the others on the desktop, a row of its own on the phone (blue,
/// or red when it `destroys` something). True when tapped.
pub(crate) fn action(ui: &mut egui::Ui, t: &Tokens, id: &str, label: &str, enabled: bool, destroys: bool) -> bool {
    if !crate::is_compact(ui.ctx()) {
        let r = ui.add_enabled(enabled, egui::Button::new(crate::i18n::tr(label)));
        register(ui, format!("button:{id}"), r.rect);
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
pub(crate) fn text_row(ui: &mut egui::Ui, t: &Tokens, id: &str, label: &str, value: &mut String, hint_text: &str, password: bool, width: f32) -> egui::Response {
    let compact = crate::is_compact(ui.ctx());
    let r = row(ui, t, label, |ui| {
        let edit = egui::TextEdit::singleline(value).hint_text(crate::i18n::tr(hint_text)).password(password);
        if compact {
            ui.add(
                edit.frame(egui::Frame::NONE)
                    .font(egui::FontId::proportional(17.0))
                    .align(egui::Align2::RIGHT_CENTER)
                    .min_size(vec2(0.0, ROW))
                    .desired_width(ui.available_width()),
            )
        } else {
            ui.add(crate::widgets::text_box(ui, edit.desired_width(width.min(ui.available_width()))))
        }
    });
    register(ui, format!("field:{id}"), r.rect);
    r
}

/// The share of the page's room the pinned slide preview of the Slideshow section takes.
pub(crate) const PINNED_PREVIEW_SHARE: f32 = 0.4;

/// What the section's title and blurb, the preview's heading, the hint under the slide, its Photo /
/// Intro / Ending choice and the rule under it take around the slide.
pub(crate) const PINNED_CHROME: f32 = 146.0;