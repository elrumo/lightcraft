//! The Settings dialog (⌘,): General, Import, Performance, Display, Interface, Slideshow, Faces, AI Denoise,
//! Sync, AI Models.
//!
//! On the desktop the sections are a sidebar on the left of the dialog (each with its icon on a
//! coloured tile) beside the open one, which scrolls on its own under its title and a line saying
//! what it is for. Its settings are grouped on rounded cards under small headings: a row per
//! setting, its name on the left and its control on the right, on / off settings as switches, and
//! notes in grey under the row they explain (or under a heading, for the whole group).
//!
//! Changes apply immediately (no OK/Cancel). Where they are stored:
//! - **app settings** ([`crate::state::AppSettings`]: startup view, delete confirmation, GPU,
//!   preview size, filmstrip/grid badges, last library, monitor profile, whether Import Lightroom
//!   Catalog… asks where a catalog goes) and Auto Advance live in
//!   the UI state, saved by the host in its config folder (`ui.json`);
//! - **library settings** (import defaults, XMP sidecars, thumbnail cache size, slideshow
//!   playback) go through the `library.preferences` / `library.xmpPreferences` /
//!   `slideshow.playback` commands into the library's `prefs.json`,
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
use crate::state::{Appearance, GridBadges, PREVIEW_LIMITS, StartupView};
use crate::sync_ui::SignOutHow;
use crate::theme::Tokens;
use crate::widgets::register;

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

/// `path` shortened to fit `max` (as `width` measures it) by dropping leading folders, after
/// either kind of separator, so that its end stays (`…/Pictures/Library`,
/// `…\Pictures\Library`): the end says which folder it is. A last folder too long by itself
/// loses its end ([`crate::widgets::elide_head`]).
fn elide_path(path: &str, max: f32, width: impl Fn(&str) -> f32) -> String {
    if width(path) <= max {
        return path.to_string();
    }
    // (a separator at the very end is not where a name starts: `…/` alone would say nothing)
    let end = path.trim_end_matches(['/', '\\']).len();
    let mut rest = path;
    while let Some(i) = rest.find(['/', '\\']).filter(|i| path.len() - rest.len() + i + 1 < end) {
        let separator = if rest.get(i..).is_some_and(|r| r.starts_with('\\')) { '\\' } else { '/' };
        rest = rest.get(i + 1..).unwrap_or("");
        let shorter = format!("…{separator}{rest}");
        if width(&shorter) <= max {
            return shorter;
        }
    }
    let name = rest.trim_end_matches(['/', '\\']);
    // (nothing but separators: never an empty label)
    if name.is_empty() { "…".to_string() } else { crate::widgets::elide_head(name, max, width) }
}

/// A folder or file path on one line in the room left in its row (`text:{id}`): shortened from
/// the front when it is too long ([`elide_path`]), with all of it in a tooltip then.
fn path_label(ui: &mut egui::Ui, id: &str, path: &str, font: egui::FontId, color: egui::Color32) {
    path_label_beside(ui, id, path, font, color, &[]);
}

/// [`path_label`] with room left after it for `beside`, the texts of the controls that follow it in
/// the row (their buttons are as wide as the text and the padding round it): a long path gives way
/// to them, so they stay in the row.
fn path_label_beside(ui: &mut egui::Ui, id: &str, path: &str, font: egui::FontId, color: egui::Color32, beside: &[&str]) {
    let pad = ui.spacing().button_padding.x * 2.0 + ui.spacing().item_spacing.x;
    let reserve: f32 = beside
        .iter()
        .map(|text| ui.painter().layout_no_wrap(text.to_string(), egui::TextStyle::Button.resolve(ui.style()), color).size().x + pad)
        .sum();
    let room = (ui.available_width() - reserve).max(24.0);
    let shown = elide_path(path, room, |s| ui.painter().layout_no_wrap(s.to_string(), font.clone(), color).size().x);
    // (`truncate` is the last resort, for a room narrower than "…")
    let r = ui.add(egui::Label::new(RichText::new(&shown).font(font).color(color)).truncate().show_tooltip_when_elided(false));
    register(ui.ctx(), format!("text:{id}"), r.rect);
    if shown != path {
        r.on_hover_text(path);
    }
}

/// A desktop choice with more options than this is a combo box rather than a row of buttons.
const PICK_BUTTONS: usize = 6;

/// The desktop Settings: the sidebar's width, a row's height, how far a card reaches past the rows
/// on it (their text lines up with the headings above the cards).
const SIDEBAR_W: f32 = 188.0;
const DESK_ROW_H: f32 = 34.0;
const CARD_PAD: f32 = 12.0;

/// The desktop sidebar's groups of sections, a line between them: the library and how LightCraft
/// looks and works, then the AI features, then sync.
const SIDEBAR: [&[&str]; 3] =
    [&["general", "interface", "slideshow", "display", "import", "performance"], &["faces", "denoise", "models"], &["sync"]];

/// The iOS app's library lives in its container: no folder to choose or watch, no external editor,
/// no other app to read XMP sidecars, no Local folders. Those settings aren't offered there.
const IOS: bool = cfg!(target_os = "ios");

/// The phone's list: each tab's icon and the colour of its tile (iOS's system colours).
const TAB_TILES: [(Icon, Color32); 10] = [
    (Icon::Gear, Color32::from_rgb(0x8e, 0x8e, 0x93)),
    (Icon::Photos, Color32::from_rgb(0x30, 0xd1, 0x58)),
    (Icon::Wand, Color32::from_rgb(0xff, 0x9f, 0x0a)),
    (Icon::Monitor, Color32::from_rgb(0x64, 0xd2, 0xff)),
    (Icon::Eye, Color32::from_rgb(0x5e, 0x5c, 0xe6)),
    (Icon::Play, Color32::from_rgb(0xff, 0x45, 0x3a)),
    (Icon::FaceBox, Color32::from_rgb(0xff, 0x37, 0x5f)),
    (Icon::Detail, Color32::from_rgb(0xac, 0x8e, 0x68)),
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

/// The desktop Settings' height, given the room the dialog has for it (`max`): as much as fits, up
/// to a comfortable height (in a short window the sidebar scrolls too).
pub fn desktop_height(max: f32) -> f32 {
    if max.is_nan() { 620.0 } else { max.clamp(1.0, 620.0) }
}

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
                    "faces" => super::faces::settings_tab(app, ui, &t),
                    "denoise" => super::denoise::settings_tab(app, ui, &t),
                    "sync" => sync_tab(app, ui, &t),
                    "models" => crate::panels::ai_models::tab(app, ui, &t),
                    _ => general_tab(app, ui, &t),
                }
                card_end(ui, &t);
                ui.data_mut(|d| d.remove::<Column>(column_id()));
                // (what the page took: wider than the room for it would mean a control is cut off)
                register(ui.ctx(), "settings:page", ui.min_rect());
            });
        });
        register(ui.ctx(), "settings:pageRoom", scrolled.inner_rect);
    });
}

/// The share of the page's room the pinned slide preview of the Slideshow section takes.
const PINNED_PREVIEW_SHARE: f32 = 0.4;

/// What the section's title and blurb, the preview's heading, the hint under the slide, its Photo /
/// Intro / Ending choice and the rule under it take around the slide.
const PINNED_CHROME: f32 = 146.0;

/// The least room in which the page below the pinned preview still scrolls.
const PAGE_MIN_HEIGHT: f32 = 120.0;

/// On Slideshow, the section's title and the slide preview stay above the page, not in it:
/// hovering a template in the Template Browser previews it there, so it has to stay in view however
/// far down the templates are scrolled to. It takes [`PINNED_PREVIEW_SHARE`] of the `height` of the
/// room (at least the least tall slide); where that leaves less than [`PAGE_MIN_HEIGHT`] for the
/// page, it scrolls with the page instead. Whether it was pinned.
fn pin_slide_preview(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens, tab: &str, height: f32) -> bool {
    if tab != "slideshow" {
        return false;
    }
    let slide_h = (height * PINNED_PREVIEW_SHARE - PINNED_CHROME).max(super::slide_layout::PREVIEW_MIN_H);
    if height - (slide_h + PINNED_CHROME) < PAGE_MIN_HEIGHT {
        return false;
    }
    egui::Frame::NONE.inner_margin(Margin { left: 20 + CARD_PAD as i8, right: 10 + CARD_PAD as i8, top: 0, bottom: 0 }).show(ui, |ui| {
        ui.set_width(ui.available_width());
        page_header(ui, t, tab);
        super::slide_layout::preview(app, ui, t, slide_h);
    });
    ui.add_space(2.0);
    ui.separator();
    true
}

/// The desktop sidebar: a row per section, its icon on a coloured tile (as on the phone's list), the
/// open one highlighted, a line between the groups ([`SIDEBAR`]). A click opens the section
/// (`button:settingsTab-{id}`); Sync says whether it is on.
fn sidebar(app: &LightcraftApp, ui: &mut egui::Ui, t: &Tokens, tab: &mut String) {
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
            register(ui.ctx(), format!("button:settingsTab-{id}"), r);
            resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, on, label));
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
fn sync_value(app: &LightcraftApp) -> &'static str {
    if app.session.sync_conflict().is_some() {
        crate::i18n::tr("Choose…")
    } else if app.session.sync_state().is_some_and(|s| s.signed_in()) || app.session.icloud_folder().is_some() {
        crate::i18n::tr("On")
    } else {
        crate::i18n::tr("Off")
    }
}

/// A desktop section's title, and a line saying what it is for.
fn page_header(ui: &mut egui::Ui, t: &Tokens, tab: &str) {
    let title = TABS.iter().find(|(id, _)| *id == tab).map_or("General", |(_, label)| *label);
    ui.add_space(2.0);
    ui.label(RichText::new(crate::i18n::tr(title)).font(t.semibold(20.0)).color(t.text));
    ui.label(RichText::new(crate::i18n::tr(blurb(tab))).size(12.5).color(t.text_dim));
    ui.add_space(10.0);
}

/// What a section is for, under its title on the desktop.
fn blurb(tab: &str) -> &'static str {
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
            "display" => display_tab(app, ui, &t),
            "interface" => interface_tab(app, ui, &t),
            "slideshow" => slideshow_tab(app, ui, &t, true),
            "faces" => super::faces::settings_tab(app, ui, &t),
            "denoise" => super::denoise::settings_tab(app, ui, &t),
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
struct Card {
    shape: egui::layers::ShapeIdx,
    top: f32,
    /// Where the last row started (a row asked twice, by a row and the switch on it, is one).
    last: f32,
}

fn card_id() -> egui::Id {
    egui::Id::new("lc-settings-card")
}

/// Where the open desktop section's rows go, left and right (cards reach [`CARD_PAD`] past them),
/// and where its settings start (under its title). Set only while a desktop section is drawn: the
/// helpers below lay rows out on cards then, as they always did otherwise.
#[derive(Clone, Copy)]
struct Column {
    left: f32,
    right: f32,
    top: f32,
}

fn column_id() -> egui::Id {
    egui::Id::new("lc-settings-column")
}

fn column(ui: &egui::Ui) -> Option<Column> {
    if crate::is_compact(ui.ctx()) {
        return None;
    }
    ui.data(|d| d.get_temp::<Column>(column_id()))
}

/// Before a row: start a card, or draw the line between this row and the one above (on the phone a
/// hairline from `inset`, where the row's text starts, to the card's edge, as iOS does; on the
/// desktop a gap in the dialog's colour across the card).
pub(super) fn card_row(ui: &mut egui::Ui, t: &Tokens, inset: f32) {
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
pub(super) fn card_end(ui: &mut egui::Ui, t: &Tokens) {
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
pub(super) fn tap_row(ui: &mut egui::Ui, t: &Tokens, id: &str, label: &str, h: f32, inset: f32, enabled: bool) -> (Rect, egui::Response) {
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
pub(super) fn line(ui: &egui::Ui, text: &str, size: f32, color: Color32, max_w: f32) -> std::sync::Arc<egui::Galley> {
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
        let r = crate::widgets::combo_box(id).width(220.0).selected_text(value).show_ui(ui, items);
        register(ui.ctx(), format!("combo:{id}"), r.response.rect);
    });
}

/// A labelled choice among a few `options`: mutually exclusive buttons on the desktop
/// (`button:{id}-{i}`; a combo box `combo:{id}` when there are more than [`PICK_BUTTONS`]), a
/// pull-down menu on the phone. True when it changed.
fn pick<V: PartialEq + Copy>(ui: &mut egui::Ui, t: &Tokens, id: &str, label: &str, options: &[(V, &str)], value: &mut V) -> bool {
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
fn info(ui: &mut egui::Ui, t: &Tokens, label: &str, value: &str, color: Color32) -> Rect {
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
            ui.add(crate::widgets::text_box(ui, edit.desired_width(width.min(ui.available_width()))))
        }
    });
    register(ui.ctx(), format!("field:{id}"), r.rect);
    r
}

pub(super) fn heading(ui: &mut egui::Ui, t: &Tokens, text: &str) {
    if crate::is_compact(ui.ctx()) {
        // iOS: small grey capitals over the card
        card_end(ui, t);
        ui.add_space(26.0);
        // (a heading with a long server address in it wraps like a note)
        egui::Frame::NONE.inner_margin(Margin::symmetric(16, 0)).show(ui, |ui| {
            ui.label(RichText::new(crate::i18n::tr(text).to_uppercase()).size(13.0).color(t.text_dim));
        });
        ui.add_space(7.0);
        return;
    }
    if let Some(col) = column(ui) {
        card_end(ui, t);
        // (a heading under the section's title needs no gap of its own)
        if ui.cursor().top() > col.top + 1.0 {
            ui.add_space(14.0);
        }
        ui.label(RichText::new(crate::i18n::tr(text)).font(t.semibold(13.5)).color(t.text));
        ui.add_space(6.0);
        return;
    }
    ui.add_space(4.0);
    ui.label(RichText::new(crate::i18n::tr(text)).font(t.semibold(12.5)).color(t.text));
}

/// A labelled row. On the phone: the label on the left and `add`'s widgets on the right, on the
/// card; without a label, `add`'s own rows (buttons, switches) as they are.
pub(super) fn row<R>(ui: &mut egui::Ui, t: &Tokens, label: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
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
    if column(ui).is_some() {
        // a Settings section's row, on its card: the name on the left, the controls at the end
        card_row(ui, t, 0.0);
        let w = ui.available_width();
        return ui
            .allocate_ui_with_layout(vec2(w, DESK_ROW_H), Layout::left_to_right(Align::Center), |ui| {
                ui.set_min_height(DESK_ROW_H);
                if !label.is_empty() {
                    ui.add(egui::Label::new(RichText::new(crate::i18n::tr(label)).color(t.text)).wrap_mode(egui::TextWrapMode::Extend));
                    ui.add_space(12.0);
                }
                trailing(ui, add)
            })
            .inner;
    }
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(egui::vec2(LABEL_W, 24.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.set_min_width(LABEL_W);
            // (a label longer than its column wraps in it; it does not push the controls along)
            ui.add(egui::Label::new(RichText::new(crate::i18n::tr(label)).color(t.text_label)).wrap());
        });
        // The controls wrap in the room beside the label, each further line starting under the
        // first control, so a row with many choices grows down and never widens the window.
        let wrapped = egui::Layout::left_to_right(egui::Align::Center).with_main_wrap(true);
        ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), 24.0), wrapped, |ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            add(ui)
        })
        .inner
    })
    .inner
}

/// `add`'s widgets at the end of the row, in the order they are added. Their width is measured and
/// kept for the next pass, with the room the row had then; a pass whose widths changed (the first
/// one, a row that grew) is discarded, so a row is never seen out of place. A row given more room
/// than when it was measured is measured afresh: otherwise a path elided to fit a narrow first
/// frame would stay short in a wide window.
fn trailing<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    // (the row's own id: rows without a name, or with the same one, are told apart)
    let id = ui.unique_id().with("trailing");
    let room = ui.available_width();
    let known = ui.data(|d| d.get_temp::<(f32, f32)>(id)).filter(|(was, _)| (was - room).abs() <= 0.5).map(|(_, used)| used);
    let width = known.unwrap_or(room).min(room);
    ui.add_space((room - width).max(0.0));
    let start = ui.cursor().left();
    let out = add(ui);
    let used = (ui.min_rect().right() - start).max(0.0);
    if known.is_none_or(|w| (w - used).abs() > 0.5) {
        ui.data_mut(|d| d.insert_temp(id, (room, used)));
        ui.ctx().request_discard("a Settings row's width");
    }
    out
}

/// A note. On the phone it goes under the card above it, as iOS's section footers do.
pub(super) fn hint(ui: &mut egui::Ui, t: &Tokens, text: &str) {
    if crate::is_compact(ui.ctx()) {
        card_end(ui, t);
        ui.add_space(7.0);
        egui::Frame::NONE.inner_margin(Margin::symmetric(16, 0)).show(ui, |ui| {
            ui.label(RichText::new(crate::i18n::tr(text)).size(13.0).color(t.text_dim));
        });
        ui.add_space(10.0);
        return;
    }
    if column(ui).is_some() {
        // under the row it explains, on its card; under a heading, for the whole group
        let on_card = ui.data(|d| d.get_temp::<Card>(card_id())).is_some();
        ui.label(RichText::new(crate::i18n::tr(text)).size(if on_card { 11.5 } else { 12.0 }).color(t.text_dim));
        ui.add_space(4.0);
        return;
    }
    ui.label(RichText::new(crate::i18n::tr(text)).size(11.0).color(t.text_dim));
}

/// [`hint`] on one line, cut with "…" and the whole text on hover: for text that can be long and
/// changes between frames (a driver's message), so nothing below it moves under the pointer.
fn hint_line(ui: &mut egui::Ui, t: &Tokens, text: &str) {
    let compact = crate::is_compact(ui.ctx());
    if compact {
        card_end(ui, t);
        ui.add_space(7.0);
    }
    let label = egui::Label::new(RichText::new(crate::i18n::tr(text)).size(if compact { 13.0 } else { 11.5 }).color(t.text_dim)).truncate();
    ui.add(label).on_hover_text(text);
    ui.add_space(if compact { 10.0 } else { 4.0 });
}

/// A checkbox addressable as `check:{id}` (a row with a switch on the phone, and in a desktop
/// Settings section); true when toggled.
pub(super) fn check(ui: &mut egui::Ui, id: &str, value: &mut bool, label: &str) -> bool {
    if column(ui).is_some() {
        let r = switch_row(ui, value, crate::i18n::tr(label));
        register(ui.ctx(), format!("check:{id}"), r.rect);
        return r.changed();
    }
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

/// A desktop Settings row that is an on / off choice: its name (wrapping when long) on the left, a
/// switch on the right in the accent colour when on. The whole row takes the click.
fn switch_row(ui: &mut egui::Ui, on: &mut bool, label: &str) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    card_row(ui, &t, 0.0);
    let enabled = ui.is_enabled();
    let w = ui.available_width();
    let text = egui::WidgetText::from(RichText::new(label).color(if enabled { t.text } else { t.text_disabled }));
    let galley = text.into_galley(ui, Some(egui::TextWrapMode::Wrap), (w - 56.0).max(40.0), egui::TextStyle::Body);
    let (r, mut resp) = ui.allocate_exact_size(vec2(w, (galley.size().y + 14.0).max(DESK_ROW_H)), Sense::click());
    if resp.clicked() && enabled {
        *on = !*on;
        resp.mark_changed();
    }
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, enabled, *on, galley.text()));
    let k = ui.ctx().animate_bool_with_time(resp.id.with("switch"), *on, 0.12);
    let p = ui.painter();
    p.galley(pos2(r.left(), r.center().y - galley.size().y / 2.0), galley, t.text);
    let sw = Rect::from_min_size(pos2(r.right() - 34.0, r.center().y - 10.0), vec2(34.0, 20.0));
    let track = crate::widgets::lerp(t.track, t.accent, k);
    p.rect_filled(sw, 10.0, if enabled { track } else { track.gamma_multiply(0.5) });
    if resp.hovered() && enabled {
        p.rect_filled(sw, 10.0, Color32::from_white_alpha(18));
    }
    p.circle_filled(pos2(sw.left() + 10.0 + (sw.width() - 20.0) * k, sw.center().y), 8.0, Color32::WHITE);
    resp
}

/// Mutually exclusive buttons (`button:{id}-{index}`).
pub(super) fn choices<V: PartialEq + Copy>(ui: &mut egui::Ui, id: &str, options: &[(V, &str)], value: &mut V) -> bool {
    let mut changed = false;
    ui.spacing_mut().item_spacing.x = 4.0;
    for (i, (v, l)) in options.iter().enumerate() {
        if crate::widgets::choice_button(ui, &format!("{id}-{i}"), l, *value == *v).clicked() && *value != *v {
            *value = *v;
            changed = true;
        }
    }
    changed
}

// ------------------------------------------------------------------------------------- General

fn general_tab(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let languages: Vec<_> = crate::i18n::Locale::for_picker().into_iter().map(|language| (language, language.name())).collect();
    pick(ui, t, "settingsLanguage", crate::i18n::tr("Language"), &languages, &mut app.ui.language);
    crate::i18n::set_language(app.ui.language);
    if !IOS {
        heading(ui, t, crate::i18n::tr("Library"));
        let location = match &app.session.library {
            Some(l) => l.dir.display().to_string(),
            None => crate::i18n::tr("In memory — nothing is saved").to_string(),
        };
        row(ui, t, crate::i18n::tr("Location"), |ui| {
            path_label(ui, "settingsLocation", &location, egui::TextStyle::Body.resolve(ui.style()), t.text_label);
        });
        row(ui, t, "", |ui| {
            let can = app.services.pick_folder.is_some();
            if action(ui, t, "settingsOpenLibrary", "Open Library…", can, false) {
                let _ = app.run("app.openLibrary", json!({}));
            }
            if action(ui, t, "settingsNewLibrary", "New Library…", can, false) {
                let _ = app.run("app.newLibrary", json!({}));
            }
            if !can {
                hint(ui, t, crate::i18n::tr("not available here"));
            }
        });
        // where libraries are folders to choose from (not in the browser, which keeps one)
        if app.services.pick_folder.is_some() {
            use crate::state::LaunchLibrary;
            pick(
                ui,
                t,
                "settingsAtLaunch",
                crate::i18n::tr("At launch"),
                &[(LaunchLibrary::Last, "Open the last library"), (LaunchLibrary::Ask, "Ask me")],
                &mut app.ui.settings.at_launch,
            );
            hint(
                ui,
                t,
                crate::i18n::tr("Ask me lists your recent libraries at launch. Holding Alt (Option on a Mac) as the window appears asks once."),
            );
        }
        // File ▸ Import Lightroom Catalog… (#2672): the window that asks where a catalog goes, or not
        {
            use crate::state::LightroomImport;
            pick(
                ui,
                t,
                "settingsLightroomImport",
                crate::i18n::tr("When importing a Lightroom catalog"),
                &[(LightroomImport::Ask, "Ask where to import it"), (LightroomImport::AlwaysMerge, "Always merge into the open library")],
                &mut app.ui.settings.lightroom_import,
            );
        }
    }
    heading(ui, t, crate::i18n::tr("Hidden"));
    row(ui, t, "", |ui| {
        let label = if app.session.hidden.has_password() { "Change Hidden Password…" } else { "Set Hidden Password…" };
        if action(ui, t, "settingsHiddenPassword", label, true, false) {
            crate::panels::dialogs::change_hidden_password(app);
        }
    });
    hint(
        ui,
        t,
        crate::i18n::tr("The password that opens the Hidden album on every device of this library (Face ID or Touch ID on an iPhone, iPad or Mac)."),
    );
    // (the photos the sensitive-content model flagged, Settings ▸ AI Models)
    let mut blur = app.session.blur_sensitive;
    if ui.checkbox(&mut blur, crate::i18n::tr("Blur photos that might be sensitive in the grid")).changed() {
        let _ = app.run("library.setBlurSensitive", json!({"on": blur}));
    }
    heading(ui, t, crate::i18n::tr("Startup"));
    pick(
        ui,
        t,
        "settingsStartup",
        crate::i18n::tr("Open in"),
        &[(StartupView::Last, "Last view"), (StartupView::Grid, "Photo Grid"), (StartupView::Detail, "Detail View")],
        &mut app.ui.settings.startup_view,
    );
    heading(ui, t, crate::i18n::tr("Culling"));
    check(ui, "settings.autoAdvance", &mut app.ui.auto_advance, "Auto Advance: move to the next photo after rating or flagging");
    check(ui, "settings.confirmDelete", &mut app.ui.settings.confirm_delete, "Confirm before moving photos to Recently Deleted");
    check(ui, "settings.askBeforeMove", &mut app.ui.settings.ask_before_move, "Confirm before moving photos to another folder on disk");
    if IOS {
        return;
    }
    heading(ui, t, crate::i18n::tr("Dialogs"));
    check(ui, "settings.closeDialogsOutside", &mut app.ui.settings.close_dialogs_outside, "Close a dialog by clicking outside it");
    heading(ui, t, crate::i18n::tr("External Editing"));
    row(ui, t, crate::i18n::tr("PhotoCraft"), |ui| {
        crate::text_field::TextField::singleline("field:photocraftPath", &mut app.ui.settings.photocraft_path)
            .hint(crate::i18n::tr("Found automatically"))
            .width(220.0)
            .show(ui);
    });
    edit_file_rows(ui, t, "photocraftFile", &mut app.ui.settings.photocraft_file);
    hint(
        ui,
        t,
        crate::i18n::tr(
            "Photo ▸ Edit In ▸ Edit in PhotoCraft (⇧⌘E, or right-click a photo) renders a copy with your edits, stacks it with the original and opens it in PhotoCraft. Save there and the copy updates here. As PSD, PhotoCraft saves straight back to it. Leave the location empty unless you use a portable copy.",
        ),
    );
    row(ui, t, crate::i18n::tr("Other editor"), |ui| {
        crate::text_field::TextField::singleline("field:externalEditor", &mut app.ui.settings.external_editor)
            .hint(crate::i18n::tr("System default"))
            .width(220.0)
            .show(ui);
    });
    edit_file_rows(ui, t, "externalFile", &mut app.ui.settings.external_file);
    hint(ui, t, crate::i18n::tr("Edit in Other Editor (⌥⌘E) opens the copy in this application (an app name on macOS, a program path elsewhere)."));
    hint(
        ui,
        t,
        crate::i18n::tr(
            "To hand this editor the original file itself instead (the raw, without a copy: DxO PhotoLab, for example), right-click a photo → Edit In → Open Original in Other Editor (RAW, no copy).",
        ),
    );
}

/// The file Edit In hands an editor: format, colour space and bit depth (`button:{id}Format-…`).
fn edit_file_rows(ui: &mut egui::Ui, t: &Tokens, id: &str, file: &mut crate::state::EditFile) {
    use crate::state::{EditFormat, EditSpace};
    row(ui, t, crate::i18n::tr("Format"), |ui| {
        choices(ui, &format!("{id}Format"), &[(EditFormat::Psd, "PSD"), (EditFormat::Tiff, "TIFF")], &mut file.format);
        ui.separator();
        choices(ui, &format!("{id}Depth"), &[(16u8, "16-bit"), (8, "8-bit")], &mut file.bit_depth);
    });
    row(ui, t, crate::i18n::tr("Color space"), |ui| {
        choices(
            ui,
            &format!("{id}Space"),
            &[(EditSpace::AdobeRgb, "Adobe RGB"), (EditSpace::ProPhoto, "ProPhoto"), (EditSpace::DisplayP3, "P3"), (EditSpace::Srgb, "sRGB")],
            &mut file.color_space,
        );
    });
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
    let name = |pid: &str| {
        app.session
            .presets
            .iter()
            .find(|p| p.id == pid)
            .map(|p| crate::i18n::builtin_label(&p.name, p.builtin).to_string())
            .unwrap_or_else(|| crate::i18n::tr_format!("{pid} (missing)", pid = pid))
    };
    let text = current.map(name).unwrap_or_else(|| crate::i18n::tr(none_label).to_string());
    let mut out = None;
    select(ui, t, id, label, text, |ui| {
        if option(ui, id, 0, current.is_none(), crate::i18n::tr(none_label)) {
            out = Some(None);
        }
        let mut last = "";
        for (i, p) in app.session.presets.iter().enumerate() {
            if p.group != last {
                last = &p.group;
                group(ui, t, crate::i18n::builtin_label(last, p.builtin));
            }
            if option(ui, id, i + 1, current == Some(p.id.as_str()), crate::i18n::builtin_label(&p.name, p.builtin)) {
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
    if !cfg!(target_arch = "wasm32") && !IOS {
        heading(ui, t, crate::i18n::tr("Dropped Files"));
        row(ui, t, "", |ui| {
            check(ui, "settings.importDropped", &mut app.ui.settings.import_dropped_directly, "Import dropped files without review");
        });
    }
    heading(ui, t, crate::i18n::tr("Cameras and cards"));
    row(ui, t, "", |ui| {
        check(ui, "settings.importOnConnect", &mut app.ui.settings.import_on_connect, "Show the import when a camera or memory card is connected");
    });
    if !cfg!(target_arch = "wasm32") && !IOS {
        // (the library's own: it travels with the library, where the switch above is this app's)
        let mut on_card = d.show_on_card;
        row(ui, t, "", |ui| {
            if check(ui, "settings.importOnCard", &mut on_card, "Show the import review when a camera or card is connected") {
                let _ = app.run("library.preferences", json!({"import": {"showOnCard": on_card}}));
            }
        });
    }
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
                    Some(RAW_DEFAULT) => crate::i18n::tr("Same as raw default").to_string(),
                    None => crate::i18n::tr("LightCraft Default").to_string(),
                    Some(pid) => app
                        .session
                        .presets
                        .iter()
                        .find(|p| p.id == pid)
                        .map(|p| crate::i18n::builtin_label(&p.name, p.builtin).to_string())
                        .unwrap_or_else(|| pid.to_string()),
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
                        if option(ui, &id, j + 2, current == Some(p.id.as_str()), crate::i18n::builtin_label(&p.name, p.builtin)) {
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
        select(ui, t, id, crate::i18n::tr("Metadata preset"), cur.clone().unwrap_or_else(|| crate::i18n::tr("None").into()), |ui| {
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
        let r = ui.add(crate::widgets::text_box(
            ui,
            egui::TextEdit::multiline(&mut text).desired_rows(4).desired_width(f32::INFINITY).hint_text("*.fcpbundle"),
        ));
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
            match &d.auto_folder {
                Some(folder) => {
                    let beside = [crate::i18n::tr("Choose…"), crate::i18n::tr("Turn Off")];
                    path_label_beside(ui, "settingsAutoFolder", folder, egui::TextStyle::Body.resolve(ui.style()), t.text, &beside)
                }
                None => drop(ui.label(RichText::new(crate::i18n::tr("Off")).color(t.text))),
            }
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
            let mut sub = d.auto_subfolders;
            if check(ui, "settings.autoImportSubfolders", &mut sub, "Include subfolders") {
                let _ = app.run("library.autoImport", json!({"subfolders": sub}));
            }
            hint(
                ui,
                t,
                crate::i18n::tr(
                    "Photos in folders inside the watched folder are added too (hidden folders and the library's own folder are skipped).",
                ),
            );
            row(ui, t, "", |ui| {
                let mut copy = d.auto_copy;
                if crate::widgets::check(ui, &mut copy, crate::i18n::tr("Copy into the library (else use the files where they are)")).changed() {
                    let _ = app.run("library.autoImport", json!({"copy": copy}));
                }
            });
            row(ui, t, crate::i18n::tr("Album"), |ui| {
                let id = egui::Id::new("auto-album");
                let mut name = ui.data(|m| m.get_temp::<String>(id)).unwrap_or_else(|| d.auto_album.clone().unwrap_or_default());
                let r = ui
                    .add(crate::widgets::text_box(ui, egui::TextEdit::singleline(&mut name).hint_text(crate::i18n::tr("None")).desired_width(200.0)));
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
    hint_line(ui, t, &status);
    let opts: Vec<(u32, String)> =
        PREVIEW_LIMITS.iter().map(|e| (*e, if *e == 0 { crate::i18n::tr("Automatic").to_string() } else { format!("{e} px") })).collect();
    let opts: Vec<(u32, &str)> = opts.iter().map(|(e, l)| (*e, l.as_str())).collect();
    pick(ui, t, "settingsPreview", crate::i18n::tr("Preview size"), &opts, &mut app.ui.settings.preview_limit);
    hint(
        ui,
        t,
        crate::i18n::tr(
            "Automatic renders the Detail view at the size it is shown, up to the photo's own pixels. Choose a size to cap it: smaller is faster.",
        ),
    );
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
    let used = used.map(|b| format!("{:.1} MB", b as f64 / 1048576.0)).unwrap_or_else(|| crate::i18n::tr("memory only").to_string());
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
    #[cfg(not(target_arch = "wasm32"))]
    thumb_cache_folder(app, ui, t);
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

/// Where this library keeps its thumbnail cache (issue #903): the effective folder, and choosing
/// another one, like the smart previews folder below.
#[cfg(not(target_arch = "wasm32"))]
fn thumb_cache_folder(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    // (the iOS app's cache lives in its container)
    if IOS {
        return;
    }
    // listing a big cache every frame would be slow: refresh every 2 s and after a change
    let cache = egui::Id::new("thumbs-location");
    let now = ui.input(|i| i.time);
    let loc: Value = match ui.data(|d| d.get_temp::<(f64, Value)>(cache)) {
        Some((at, v)) if now - at < 2.0 => v,
        _ => {
            let v = app.run("library.thumbnailCacheLocation", json!({})).unwrap_or(Value::Null);
            ui.data_mut(|d| d.insert_temp(cache, (now, v.clone())));
            v
        }
    };
    if loc.is_null() {
        return;
    }
    let path = loc["path"].as_str().unwrap_or_default().to_string();
    let available = loc["available"].as_bool().unwrap_or(true);
    let custom = loc["custom"].as_bool().unwrap_or(false);
    row(ui, t, crate::i18n::tr("Folder"), |ui| {
        let beside: &[&str] = if custom { &[] } else { &["(library default)"] };
        path_label_beside(
            ui,
            "settingsThumbsFolder",
            &path,
            egui::TextStyle::Body.resolve(ui.style()),
            if available { t.text } else { t.reject },
            beside,
        );
        if !custom {
            ui.label(RichText::new("(library default)").color(t.text));
        }
    });
    if !available {
        hint(
            ui,
            t,
            crate::i18n::tr(
                "This folder is not available (drive disconnected?). Thumbnails are kept in memory only until it is back or you choose another folder.",
            ),
        );
    }
    // what happens to the thumbnails already cached when the folder changes
    let mode_id = egui::Id::new("thumbs-existing");
    let mut mode: u8 = ui.data(|d| d.get_temp(mode_id)).unwrap_or(0);
    if loc["count"].as_u64().unwrap_or(0) > 0 {
        row(ui, t, crate::i18n::tr("Existing thumbnails"), |ui| {
            if choices(ui, "settingsThumbsExisting", &[(0u8, "Move them"), (1, "Leave them"), (2, "Delete them")], &mut mode) {
                ui.data_mut(|d| d.insert_temp(mode_id, mode));
            }
        });
    }
    let existing = ["move", "leave", "discard"][mode.min(2) as usize];
    let mut result = None;
    row(ui, t, "", |ui| {
        if app.services.pick_folder.is_some() {
            let r = ui.button(crate::i18n::tr("Choose Folder…"));
            register(ui.ctx(), "button:settingsThumbsChoose", r.rect);
            if r.clicked()
                && let Some(dir) = app.services.pick_folder.as_mut().and_then(|f| f())
            {
                result = Some(app.run("library.thumbnailCacheLocation", json!({"path": dir, "existing": existing})));
            }
        }
        let r = ui.add_enabled(custom, egui::Button::new(crate::i18n::tr("Use Library Folder")));
        register(ui.ctx(), "button:settingsThumbsReset", r.rect);
        if r.clicked() {
            result = Some(app.run("library.thumbnailCacheLocation", json!({"reset": true, "existing": existing})));
        }
    });
    if let Some(r) = result {
        ui.data_mut(|d| d.remove::<(f64, Value)>(cache));
        match r {
            Ok(v) => {
                let failed = v["failed"].as_array().map_or(0, Vec::len);
                let msg = match (existing, v["handled"].as_u64().unwrap_or(0)) {
                    ("move", n) if n > 0 => format!("Thumbnail cache folder changed; moved {n}"),
                    ("discard", n) if n > 0 => format!("Thumbnail cache folder changed; deleted {n}"),
                    _ => "Thumbnail cache folder changed".to_string(),
                };
                app.toast(ui.ctx(), if failed > 0 { crate::i18n::tr_format!("{msg} ({failed} failed)", failed = failed, msg = msg) } else { msg });
            }
            Err(e) => app.toast(ui.ctx(), e),
        }
    }
    hint(ui, t, crate::i18n::tr("A fast local drive keeps the grid quick when the library itself is on a network or slow drive."));
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
        let beside: &[&str] = if custom { &[] } else { &["(library default)"] };
        path_label_beside(
            ui,
            "settingsSmartFolder",
            &path,
            egui::TextStyle::Body.resolve(ui.style()),
            if available { t.text } else { t.reject },
            beside,
        );
        if !custom {
            ui.label(RichText::new("(library default)").color(t.text));
        }
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
    row(ui, t, "", |ui| {
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
    hint(ui, t, crate::i18n::tr("Keep it on a drive with room: smart previews are about 1 MB per photo."));
}

// ---------------------------------------------------------------------------------- Interface

/// The monitor profile (`app.displayProfile`): previews are shown through the display's ICC
/// profile, so wide-gamut and calibrated displays show colours as they are.
fn display_tab(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    heading(ui, t, crate::i18n::tr("Monitor profile"));
    let current = app.renderer.display().cloned();
    let screen = current.as_ref().is_some_and(|d| d.path == crate::SCREEN_PROFILE);
    row(ui, t, crate::i18n::tr("Profile"), |ui| {
        let name = match &current {
            Some(d) => d.profile.description.clone(),
            None => crate::i18n::tr("None (the display is treated as sRGB)").to_string(),
        };
        ui.label(RichText::new(name).color(t.text));
        if screen {
            hint(ui, t, crate::i18n::tr("this screen's profile"));
        }
    });
    row(ui, t, "", |ui| {
        let can = app.services.pick_display_profile.is_some();
        let r = ui.add_enabled(can, egui::Button::new(crate::i18n::tr("Choose Profile…")));
        register(ui.ctx(), "button:settingsDisplayProfile", r.rect);
        if r.clicked()
            && let Some(path) = app.services.pick_display_profile.as_mut().and_then(|pick| pick().into_iter().next())
        {
            app.ui.settings.display_profile = path;
        }
        // with no profile chosen, previews follow the screen's own profile where the host knows it
        let none = if app.services.screen_profile.is_some() { crate::i18n::tr("Use Screen Profile") } else { crate::i18n::tr("Use sRGB") };
        let r = ui.add_enabled(!app.ui.settings.display_profile.is_empty(), egui::Button::new(none));
        register(ui.ctx(), "button:settingsDisplayProfileNone", r.rect);
        if r.clicked() {
            app.ui.settings.display_profile.clear();
        }
        if !can {
            hint(ui, t, crate::i18n::tr("not available here"));
        }
    });
    if let Some(e) = &app.display_error {
        ui.label(RichText::new(e).size(11.0).color(t.reject));
    }
    if let Some(d) = &current {
        let info = d.describe();
        if !screen {
            row(ui, t, crate::i18n::tr("File"), |ui| {
                path_label(ui, "settingsDisplayFile", &d.path, egui::FontId::proportional(11.5), t.text_dim);
            });
        }
        row(ui, t, crate::i18n::tr("Type"), |ui| {
            let kind = match d.profile.kind {
                lightcraft_engine::display::DisplayKind::MatrixTrc => crate::i18n::tr("Matrix/TRC"),
                lightcraft_engine::display::DisplayKind::Lut => crate::i18n::tr("LUT-based"),
            };
            ui.label(RichText::new(kind).color(t.text));
        });
        row(ui, t, crate::i18n::tr("Primaries (x, y)"), |ui| {
            let p = &info["primaries"];
            let xy = |(k, l): (&str, &str)| format!("{l} {:.3}, {:.3}", p[k][0].as_f64().unwrap_or(0.0), p[k][1].as_f64().unwrap_or(0.0));
            let text = [("red", "R"), ("green", "G"), ("blue", "B")].map(xy).join("   ");
            ui.label(RichText::new(text).size(11.0).color(t.text_dim));
        });
    }
    hint(
        ui,
        t,
        crate::i18n::tr(
            "Photos are shown through your monitor's ICC profile, so colours are right on wide-gamut and calibrated displays, and the Detail view uses the display's whole gamut. The histogram, the preview caches and exports are not affected.",
        ),
    );
    #[cfg(target_os = "linux")]
    hint(ui, t, crate::i18n::tr("Profiles assigned in your desktop's colour settings are usually in ~/.local/share/icc."));
}

fn interface_tab(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    // (the phone and tablet layouts are drawn light, dark or as the system is; the desktop's has
    // its themes)
    if app.compact {
        heading(ui, t, crate::i18n::tr("Appearance"));
        let items: Vec<(Appearance, &str)> = Appearance::ALL.iter().map(|a| (*a, a.label())).collect();
        pick(ui, t, "settingsAppearance", crate::i18n::tr("Appearance"), &items, &mut app.ui.appearance);
        hint(ui, t, crate::i18n::tr("System follows the device's Display & Brightness setting."));
    } else {
        let system = app.system_theme(ui.ctx());
        appearance(ui, t, &mut app.ui.settings, system);
    }
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
    heading(ui, t, crate::i18n::tr("Keywords"));
    row(ui, t, crate::i18n::tr("Keywords of the selection"), |ui| {
        use crate::state::KeywordChips as K;
        choices(ui, "settingsKeywordChips", &[(K::Paths, "Whole names"), (K::Grouped, "Grouped by parent")], &mut app.ui.settings.keyword_chips);
    });
    hint(
        ui,
        t,
        crate::i18n::tr("Grouped: keywords inside the same keyword share a line naming it, and long lists and the suggestions are shortened."),
    );
    heading(ui, t, crate::i18n::tr("Detail View"));
    check(ui, "settings.navigator", &mut app.ui.navigator, "Show the Navigator while zoomed in");
    use crate::state::InfoOverlay as I;
    pick(
        ui,
        t,
        "settingsInfo",
        crate::i18n::tr("Info overlay"),
        &[(I::Off, "Off"), (I::Basic, "Info 1"), (I::Exposure, "Info 2")],
        &mut app.ui.info_overlay,
    );
    hint(ui, t, crate::i18n::tr("View → View Options chooses the lines Info 1 and Info 2 show."));
    heading(ui, t, crate::i18n::tr("Map"));
    check(ui, "settings.mapOnline", &mut app.ui.settings.map_online, "Load map tiles from the internet");
    hint(
        ui,
        t,
        crate::i18n::tr(
            "Off: the Map shows the built-in world map and tiles saved earlier. Place names and the photos' positions never leave your computer; a tile request tells the tile server which area you look at.",
        ),
    );
    text_row(
        ui,
        t,
        "mapTileUrl",
        crate::i18n::tr("Tile server"),
        &mut app.ui.settings.map_tile_url,
        "https://tile.openstreetmap.org/{z}/{x}/{y}.png",
        false,
        260.0,
    );
    let custom = app.ui.settings.map_tile_url.trim();
    if !custom.is_empty() && !lightcraft_geo::tiles::valid_template(custom) {
        hint(ui, t, crate::i18n::tr("Not a tile address (it needs https://, {z}, {x} and {y}): the standard tiles are used."));
    } else if custom.is_empty() {
        hint(
            ui,
            t,
            crate::i18n::tr(
                "Empty: OpenStreetMap's standard tiles. They are meant for light use; for a busy shared deployment, use your own or a commercial tile server.",
            ),
        );
    }
}

/// View ▸ View Options (⌘J), Lightroom Classic's Loupe View tab: whether the info overlay shows and
/// which, and the three lines each of Info 1 and Info 2 shows (`view.viewOptions`). Changes apply
/// as they are made.
pub(super) fn view_options(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    use crate::state::{InfoOverlay as I, LoupeInfo, LoupeInfoField as F};
    heading(ui, t, crate::i18n::tr("Loupe View"));
    let mut show = app.ui.info_overlay != I::Off;
    if check(ui, "viewOptions.showInfo", &mut show, "Show Info Overlay") {
        app.ui.info_overlay = if show { I::Basic } else { I::Off };
    }
    ui.add_enabled_ui(show, |ui| {
        ui.horizontal(|ui| {
            choices(ui, "viewOptionsInfo", &[(I::Basic, "Info 1"), (I::Exposure, "Info 2")], &mut app.ui.info_overlay);
        });
    });
    for (n, title, defaults) in [(1, "Loupe Info 1", LoupeInfo::INFO1), (2, "Loupe Info 2", LoupeInfo::INFO2)] {
        heading(ui, t, crate::i18n::tr(title));
        let lines = if n == 1 { &mut app.ui.settings.loupe_info.info1 } else { &mut app.ui.settings.loupe_info.info2 };
        for (i, line) in lines.iter_mut().enumerate() {
            let id = format!("viewOptionsInfo{n}Line{}", i + 1);
            let r = crate::widgets::combo_box(&id).width(220.0).selected_text(crate::i18n::tr(line.label())).show_ui(ui, |ui| {
                for f in F::ALL {
                    let key = serde_json::to_value(f).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
                    let r = ui.selectable_label(line == f, crate::i18n::tr(f.label()));
                    register(ui.ctx(), format!("button:{id}-{key}"), r.rect);
                    if r.clicked() {
                        *line = *f;
                    }
                }
            });
            register(ui.ctx(), format!("combo:{id}"), r.response.rect);
        }
        let r = ui.add_enabled(*lines != defaults, egui::Button::new(crate::i18n::tr("Use Defaults")));
        register(ui.ctx(), format!("button:viewOptionsDefaults{n}"), r.rect);
        if r.clicked() {
            *lines = defaults;
        }
    }
}

// ----------------------------------------------------------------------------------- Slideshow

/// A playback duration slider (`slider:{id}`). While it is dragged the value changes in memory
/// (the slideshow reads it); letting go saves it through `slideshow.playback`.
fn duration_slider(app: &mut LightcraftApp, ui: &mut egui::Ui, id: &str, key: &str, value: f64, range: std::ops::RangeInclusive<f64>) {
    let mut v = value;
    let r = ui.add(egui::Slider::new(&mut v, range).logarithmic(key == "slideDuration").suffix(" s").max_decimals(1));
    register(ui.ctx(), format!("slider:{id}"), r.rect);
    if r.changed()
        && let Ok(next) = app.session.slideshow.merged(&json!({ key: v }))
    {
        app.session.slideshow = next;
    }
    if r.drag_stopped() || (r.changed() && !r.dragged()) {
        let _ = app.run("slideshow.playback", json!({ key: v }));
    }
}

/// `with_preview` the composed slide is left out: [`pin_slide_preview`] has pinned it above the page.
fn slideshow_tab(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens, with_preview: bool) {
    // one column (the sidebar or the phone's page takes the side): the composed slide first, then the
    // settings
    if with_preview {
        super::slide_layout::preview(app, ui, t, f32::INFINITY);
        ui.add_space(6.0);
    }
    super::slide_templates::section(app, ui, t);
    ui.add_space(6.0);
    playback_section(app, ui, t);
    ui.add_space(6.0);
    super::slide_layout::sections(app, ui, t);
    ui.add_space(6.0);
    super::slide_overlays::sections(app, ui, t);
    ui.add_space(6.0);
    super::slide_titles::sections(app, ui, t);
    ui.add_space(6.0);
    export_section(app, ui, t);
}

/// Export: PDF, JPEG sequence or video of the slideshow (View ▸ Export Slideshow…).
fn export_section(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    heading(ui, t, crate::i18n::tr("Export"));
    let running = app.session.slideshow_export.as_ref().is_some_and(|r| r.running());
    row(ui, t, "", |ui| {
        for (id, label, format) in [
            ("slideshowExportPdf", "Export PDF…", "pdf"),
            ("slideshowExportJpeg", "Export JPEG…", "jpeg"),
            ("slideshowExportVideo", "Export Video…", "video"),
        ] {
            if action(ui, t, id, label, !running, false) {
                let _ = app.run("dialog.slideshowExport", json!({"format": format}));
            }
        }
    });
    hint(ui, t, crate::i18n::tr("The photos in view as slides, with the settings above. Progress shows in the activity stack."));
}

fn playback_section(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    use lightcraft_engine::slideshow::{FADE_MAX, SLIDE_MAX, SLIDE_MIN};
    heading(ui, t, crate::i18n::tr("Playback"));
    let s = app.session.slideshow.clone();
    row(ui, t, crate::i18n::tr("Slide duration"), |ui| {
        duration_slider(app, ui, "slideshowSlide", "slideDuration", s.slide_duration, SLIDE_MIN..=SLIDE_MAX)
    });
    row(ui, t, crate::i18n::tr("Fade duration"), |ui| {
        duration_slider(app, ui, "slideshowFade", "fadeDuration", s.fade_duration, 0.0..=FADE_MAX.min(s.slide_duration))
    });
    hint(ui, t, crate::i18n::tr("A slide stays up for its duration, its fade in included. A fade of 0 cuts."));
    #[derive(Clone, Copy, PartialEq)]
    enum Fade {
        Cross,
        Black,
        White,
        Color,
    }
    let mut f = match s.fade_color {
        None => Fade::Cross,
        Some([0, 0, 0]) => Fade::Black,
        Some([255, 255, 255]) => Fade::White,
        Some(_) => Fade::Color,
    };
    let opts = [(Fade::Cross, "Crossfade"), (Fade::Black, "Through black"), (Fade::White, "Through white"), (Fade::Color, "Through a colour")];
    if pick(ui, t, "settingsSlideshowFade", crate::i18n::tr("Fade"), &opts, &mut f) {
        let c = match f {
            Fade::Cross => Value::Null,
            Fade::Black => json!("#000000"),
            Fade::White => json!("#ffffff"),
            Fade::Color => json!([64, 64, 64]),
        };
        let _ = app.run("slideshow.playback", json!({"fadeColor": c}));
    }
    if f == Fade::Color
        && let Some(mut c) = s.fade_color
    {
        row(ui, t, crate::i18n::tr("Fade color"), |ui| {
            let r = egui::color_picker::color_edit_button_srgb(ui, &mut c);
            register(ui.ctx(), "button:slideshowFadeColor", r.rect);
            if r.changed() {
                let _ = app.run("slideshow.playback", json!({"fadeColor": c}));
            }
        });
    }
    let mut v = s.random_order;
    if check(ui, "settings.slideshowRandom", &mut v, "Random order") {
        let _ = app.run("slideshow.playback", json!({"randomOrder": v}));
    }
    let mut v = s.repeat;
    if check(ui, "settings.slideshowRepeat", &mut v, "Repeat (off: the slideshow ends after the last photo)") {
        let _ = app.run("slideshow.playback", json!({"repeat": v}));
    }
    let mut v = s.prepare_previews;
    if check(ui, "settings.slideshowPrepare", &mut v, "Prepare previews in advance") {
        let _ = app.run("slideshow.playback", json!({"preparePreviews": v}));
    }
    hint(
        ui,
        t,
        crate::i18n::tr(
            "Renders the next slides while the current one is up, so fades never wait. Off: each slide renders when it is due (less memory and work).",
        ),
    );
    hint(
        ui,
        t,
        crate::i18n::tr("Slideshow in the View menu plays the photos in view from the selected one. These settings are saved with the library."),
    );
}

/// Appearance Mode (`button:settingsAppearance-{0,1,2}`: Auto, Dark, Light) above the light and
/// dark theme cards (`radio:theme-{id}`), each with a preview of the chosen theme. The card in
/// use is outlined. Applied on the next frame ([`LightcraftApp::sync_theme`]).
fn appearance(ui: &mut egui::Ui, t: &Tokens, s: &mut crate::state::AppSettings, system: Option<egui::Theme>) {
    use crate::state::{AppearanceMode as M, DarkTheme, LightTheme};
    heading(ui, t, crate::i18n::tr("Appearance"));
    row(ui, t, crate::i18n::tr("Appearance Mode"), |ui| {
        choices(ui, "settingsAppearance", &[(M::Auto, "Sync with System"), (M::Dark, "Dark Mode"), (M::Light, "Light Mode")], &mut s.appearance_mode);
    });
    card_end(ui, t);
    ui.add_space(8.0);
    let light_active = s.theme(system) == s.light_theme.kind();
    // (in a Settings section, out to the edges of the cards above and below)
    let mut area = ui.available_rect_before_wrap();
    if let Some(col) = column(ui) {
        area.min.x = col.left - CARD_PAD;
        area.max.x = col.right + CARD_PAD;
    }
    let available = area.width();
    let mut cards = |ui: &mut egui::Ui, width: f32| {
        ui.spacing_mut().item_spacing.x = 12.0;
        let light: Vec<_> = LightTheme::ALL.iter().map(|l| (*l, l.kind())).collect();
        theme_card(ui, t, "Light Theme", light_active, &light, &mut s.light_theme, &mut s.favorite_themes, width);
        let dark: Vec<_> = DarkTheme::ALL.iter().map(|d| (*d, d.kind())).collect();
        theme_card(ui, t, "Dark Theme", !light_active, &dark, &mut s.dark_theme, &mut s.favorite_themes, width);
    };
    ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
        if available < 440.0 {
            ui.vertical(|ui| cards(ui, available));
        } else {
            ui.horizontal_top(|ui| cards(ui, ((available - 12.0) / 2.0).floor()));
        }
    });
    ui.add_space(2.0);
}

/// One family's card: its title (and "Active" when it is the one shown), a preview of the
/// selected theme and a radio button per theme.
fn theme_card<V: PartialEq + Copy>(
    ui: &mut egui::Ui,
    t: &Tokens,
    title: &str,
    active: bool,
    options: &[(V, crate::theme::ThemeKind)],
    value: &mut V,
    favorites: &mut Vec<String>,
    width: f32,
) {
    egui::Frame::new()
        .fill(t.inset)
        .stroke(egui::Stroke::new(if active { 1.5 } else { 1.0 }, if active { t.accent } else { t.field_border }))
        .corner_radius(f32::from(t.control_radius) + 4.0)
        .inner_margin(8)
        .show(ui, |ui| {
            ui.vertical(|ui| {
                let inner = (width - 18.0).clamp(120.0, 320.0);
                ui.set_width(inner);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(crate::i18n::tr(title)).font(t.semibold(12.0)).color(t.text));
                    if active {
                        ui.label(RichText::new(crate::i18n::tr("In Use")).size(11.0).color(t.accent));
                    }
                });
                let shown = options.iter().find(|(v, _)| v == value).or(options.first()).map(|(_, k)| *k).unwrap_or_default();
                theme_preview(ui, shown, inner);
                for (v, kind) in options {
                    let row = egui::Rect::from_min_size(ui.cursor().min, egui::vec2(ui.available_width(), 24.0));
                    let hovered = ui.ctx().pointer_hover_pos().is_some_and(|p| row.contains(p));
                    ui.horizontal(|ui| {
                        let r = ui.radio_value(value, *v, crate::i18n::tr(kind.label()));
                        register(ui.ctx(), format!("radio:theme-{}", kind.id()), r.rect);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let favorite = favorites.iter().any(|id| id == kind.id());
                            let r = ui
                                .add_visible(favorite || hovered, egui::Button::new("").min_size(egui::vec2(24.0, 24.0)).frame(false))
                                .on_hover_text(crate::i18n::tr(if favorite { "Remove from favorites" } else { "Add to favorites" }));
                            r.widget_info(|| {
                                egui::WidgetInfo::labeled(
                                    egui::WidgetType::Button,
                                    true,
                                    format!(
                                        "{}: {}",
                                        crate::i18n::tr(if favorite { "Remove from favorites" } else { "Add to favorites" }),
                                        crate::i18n::tr(kind.label())
                                    ),
                                )
                            });
                            register(ui.ctx(), format!("button:themeFavorite-{}", kind.id()), r.rect);
                            if favorite || hovered {
                                crate::rating::paint_star(ui.painter(), r.rect.shrink(4.0), favorite, if favorite { t.accent } else { t.text_dim });
                            }
                            if r.clicked() {
                                if favorite {
                                    favorites.retain(|id| id != kind.id());
                                } else {
                                    favorites.push(kind.id().into());
                                }
                            }
                        });
                    });
                }
            });
        });
}

/// A small LightCraft window painted with `kind`'s tokens: the top bar with its search field, the
/// photo with the filmstrip under it, the Edit panel (histogram and sliders), the tool strip and
/// the bottom bar. A Studio theme's preview shows its panels as cards on the backdrop.
fn theme_preview(ui: &mut egui::Ui, kind: crate::theme::ThemeKind, width: f32) {
    use egui::{Color32, Rect, Stroke, StrokeKind, pos2, vec2};
    let p = Tokens::for_kind(kind);
    let (rect, _) = ui.allocate_exact_size(vec2(width, (width * 0.56).round().min(130.0)), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    let bar = |y: f32, x: f32, w: f32, c: Color32| painter.rect_filled(Rect::from_min_size(pos2(x, y), vec2(w, 2.0)), 1.0, c);
    // (cards: the gap between them and their rounding, at this scale)
    let cards = p.cards();
    let (gap, round) = if cards { (2.0, 2.5) } else { (0.0, 0.0) };
    painter.rect_filled(rect, 3.0, if cards { p.backdrop } else { p.canvas });
    // top bar: sidebar and back icons, the centred search field, icons on the right
    let top = Rect::from_min_size(rect.min, vec2(rect.width(), 13.0));
    painter.rect_filled(top, 0.0, p.backdrop);
    for x in [6.0, 14.0, 20.0] {
        painter.rect_filled(Rect::from_min_size(top.min + vec2(x, 4.5), vec2(4.0, 4.0)), 1.0, p.icon);
    }
    let search = Rect::from_center_size(top.center(), vec2(rect.width() * 0.34, 8.0));
    painter.rect(search, if cards { 4.0 } else { 2.0 }, p.field, Stroke::new(1.0, p.field_border), StrokeKind::Inside);
    bar(search.center().y - 1.0, search.center().x - 9.0, 18.0, p.text_dim);
    for i in 0..4 {
        painter.rect_filled(Rect::from_min_size(pos2(top.right() - 10.0 - i as f32 * 9.0, top.top() + 4.5), vec2(4.0, 4.0)), 2.0, p.icon);
    }
    let body = Rect::from_min_max(pos2(rect.left() + gap, top.bottom()), pos2(rect.right() - gap, rect.bottom() - gap));
    // tool strip on the right edge, the first tool (Edit) active; the bottom bar runs under it
    // unless the photo is a card of its own, which holds the bottom bar
    let strip_bottom = if cards { body.bottom() } else { body.bottom() - 12.0 };
    let strip = Rect::from_min_max(pos2(body.right() - 12.0, body.top()), pos2(body.right(), strip_bottom));
    painter.rect_filled(strip, round, p.chrome);
    if !cards {
        painter.line_segment([strip.left_top(), strip.left_bottom()], Stroke::new(1.0, p.divider));
    }
    for i in 0..5 {
        let r = Rect::from_min_size(pos2(strip.left() + 3.0, strip.top() + 4.0 + i as f32 * 9.0), vec2(6.0, 6.0));
        if i == 0 {
            painter.rect_filled(r.expand(1.5), 1.5, p.tool_active);
        }
        painter.rect_stroke(r, 1.0, Stroke::new(1.0, if i == 0 { p.text } else { p.icon }), StrokeKind::Inside);
    }
    // Edit panel: histogram, then labelled sliders
    let panel_right = strip.left() - gap;
    let panel = Rect::from_min_max(pos2(panel_right - (rect.width() * 0.27).round(), body.top()), pos2(panel_right, strip_bottom));
    painter.rect_filled(panel, round, p.chrome);
    if !cards {
        painter.line_segment([panel.left_top(), panel.left_bottom()], Stroke::new(1.0, p.divider));
    }
    let hist = Rect::from_min_size(panel.min + vec2(4.0, 4.0), vec2(panel.width() - 8.0, 14.0));
    painter.rect_filled(hist, 1.0, p.inset);
    let curve = [0.2, 0.55, 0.9, 0.7, 0.5, 0.62, 0.4, 0.3, 0.18, 0.1];
    let mut pts = vec![hist.left_bottom()];
    for (i, h) in curve.iter().enumerate() {
        pts.push(pos2(hist.left() + hist.width() * i as f32 / (curve.len() - 1) as f32, hist.bottom() - hist.height() * h));
    }
    pts.push(hist.right_bottom());
    // the area under the curve, a column per stretch between two points
    for pair in pts.windows(2) {
        if let [a, b] = pair {
            let r = Rect::from_min_max(pos2(a.x, a.y.max(b.y)), pos2(b.x, hist.bottom()));
            painter.rect_filled(r, 0.0, p.track.gamma_multiply(0.5));
        }
    }
    painter.add(egui::Shape::line(pts, Stroke::new(1.0, p.text_dim)));
    let sliders = [0.55, 0.62, 0.35, 0.7, 0.6, 0.45];
    let mut y = hist.bottom() + 7.0;
    bar(y, panel.left() + 5.0, 14.0, p.text);
    y += 6.0;
    for v in sliders {
        if y + 7.0 > panel.bottom() {
            break;
        }
        bar(y, panel.left() + 5.0, 12.0, p.text_label);
        bar(y, panel.right() - 10.0, 5.0, p.text_dim);
        let track_y = y + 5.0;
        let (from, to) = (panel.left() + 5.0, panel.right() - 5.0);
        let x = from + (to - from) * v;
        if p.filled_sliders {
            // a rounded track, filled from its middle (the default) to the handle
            painter.line_segment([pos2(from, track_y), pos2(to, track_y)], Stroke::new(1.5, p.track));
            let mid = (from + to) / 2.0;
            painter.line_segment([pos2(mid.min(x), track_y), pos2(mid.max(x), track_y)], Stroke::new(1.5, p.thumb));
            painter.circle_filled(pos2(x, track_y), 2.2, p.thumb);
        } else {
            painter.line_segment([pos2(from, track_y), pos2(to, track_y)], Stroke::new(1.0, p.track));
            painter.circle_filled(pos2(x, track_y), 1.8, p.thumb);
        }
        y += 10.0;
    }
    // the photo card (its own colours: a photo looks the same in every theme) above the filmstrip,
    // above the bottom bar: view buttons, rating stars, a button
    let photo_card = Rect::from_min_max(pos2(body.left(), body.top()), pos2(panel.left() - gap, body.bottom()));
    let bottom = if cards {
        Rect::from_min_max(pos2(photo_card.left(), photo_card.bottom() - 12.0), photo_card.max)
    } else {
        Rect::from_min_max(pos2(rect.left(), rect.bottom() - 12.0), rect.max)
    };
    let film = Rect::from_min_max(pos2(photo_card.left(), bottom.top() - 18.0), pos2(photo_card.right(), bottom.top()));
    let stage = Rect::from_min_max(photo_card.min, pos2(photo_card.right(), film.top()));
    if cards {
        painter.rect_filled(photo_card, round, p.canvas);
    }
    painter.rect_filled(bottom, egui::CornerRadius { nw: 0, ne: 0, sw: round as u8, se: round as u8 }, p.chrome);
    for x in [6.0, 13.0, 20.0] {
        painter.rect_filled(Rect::from_min_size(bottom.min + vec2(x, 4.0), vec2(4.0, 4.0)), 1.0, p.icon);
    }
    let stars_x = bottom.center().x - 30.0;
    for i in 0..5 {
        painter.circle_filled(pos2(stars_x + i as f32 * 5.0, bottom.center().y), 1.4, p.star);
    }
    let copy = Rect::from_min_size(pos2(stars_x + 28.0, bottom.top() + 2.5), vec2(30.0, 7.0));
    painter.rect(copy, if cards { 3.5 } else { 3.0 }, p.button, Stroke::new(1.0, p.button_border), StrokeKind::Inside);
    let photo = Rect::from_center_size(stage.center(), vec2(stage.width() * 0.62, stage.height() - 8.0));
    painter.rect_filled(photo, 0.0, Color32::from_rgb(0x6f, 0x8f, 0xb4));
    let horizon = photo.top() + photo.height() * 0.62;
    painter.add(egui::Shape::convex_polygon(
        vec![
            pos2(photo.left(), horizon),
            pos2(photo.left() + photo.width() * 0.3, photo.top() + photo.height() * 0.3),
            pos2(photo.left() + photo.width() * 0.55, horizon),
        ],
        Color32::from_rgb(0x4a, 0x55, 0x66),
        Stroke::NONE,
    ));
    painter.add(egui::Shape::convex_polygon(
        vec![
            pos2(photo.left() + photo.width() * 0.35, horizon),
            pos2(photo.left() + photo.width() * 0.68, photo.top() + photo.height() * 0.2),
            pos2(photo.right(), horizon),
        ],
        Color32::from_rgb(0x5a, 0x63, 0x72),
        Stroke::NONE,
    ));
    painter.rect_filled(Rect::from_min_max(pos2(photo.left(), horizon), photo.max), 0.0, Color32::from_rgb(0x3d, 0x5a, 0x3a));
    painter.line_segment([film.left_top(), film.right_top()], Stroke::new(1.0, p.divider));
    for i in 0..5 {
        let cell = Rect::from_min_size(pos2(film.left() + 4.0 + i as f32 * 17.0, film.top() + 3.0), vec2(15.0, 12.0));
        if cell.right() > film.right() - 2.0 {
            break;
        }
        let selected = i == 1;
        painter.rect_filled(cell, 0.0, if selected { p.cell_selected } else { p.cell });
        let thumb = cell.shrink(2.5);
        painter.rect_filled(thumb, 0.0, Color32::from_rgb(0x6f + 8 * i as u8, 0x80, 0x8f));
        if selected {
            painter.rect_stroke(thumb, 0.0, Stroke::new(1.0, p.pick), StrokeKind::Outside);
        }
    }
}

// ---------------------------------------------------------------------------------------- Sync

/// Run a sync command, showing what went wrong.
fn sync_cmd(app: &mut LightcraftApp, ui: &egui::Ui, id: &str, p: Value) {
    if let Err(e) = app.run(id, p) {
        app.toast(ui.ctx(), e);
    }
}

fn sync_tab(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    // (the desktop section's title says it)
    if crate::is_compact(ui.ctx()) {
        heading(ui, t, crate::i18n::tr("Sync"));
    }
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
            // signed in to a server that has a library already, and this one has photos: nothing
            // syncs until the user chooses (a dialog asked once; this is where to come back to it)
            let waiting = app.session.sync_conflict().is_some();
            // signing out waits for what is left to be sent: it can be cancelled, or done without waiting
            let leaving = c.leaving;
            // (a question left open that no longer applies, because the state moved on, goes)
            app.cloud_sync.sign_out =
                app.cloud_sync.sign_out.filter(|h| matches!((h, leaving), (SignOutHow::Wait, false) | (SignOutHow::Anyway, true)));
            if let Some(how) = app.cloud_sync.sign_out {
                sign_out_question(app, ui, t, how);
                return;
            }
            row(ui, t, "", |ui| {
                if leaving {
                    if action(ui, t, "syncSignOutCancel", "Cancel Sign Out", true, false) {
                        sync_cmd(app, ui, "sync.cancelSignOut", json!({}));
                    }
                } else if waiting {
                    if action(ui, t, "syncChoose", "Choose What to Do…", true, false) {
                        sync_cmd(app, ui, "dialog.syncChoice", json!({}));
                    }
                } else {
                    if action(ui, t, "syncNow", "Sync Now", true, false) {
                        sync_cmd(app, ui, "sync.now", json!({}));
                    }
                    if action(ui, t, "syncPause", if c.paused { "Resume Syncing" } else { "Pause Syncing" }, true, false) {
                        sync_cmd(app, ui, "sync.pause", json!({"on": !c.paused}));
                    }
                }
                if crate::is_compact(ui.ctx()) {
                    // (iOS: signing out on a card of its own)
                    card_end(ui, t);
                    ui.add_space(28.0);
                }
                if leaving {
                    if action(ui, t, "syncSignOutAnyway", "Sign Out Anyway…", true, true) {
                        app.cloud_sync.sign_out = Some(SignOutHow::Anyway);
                    }
                } else if action(ui, t, "syncSignOut", "Sign Out", true, true) {
                    if c.library.is_empty() {
                        // (this library never joined the server's: nothing of it is there, so it signs out and stays as it is)
                        sync_cmd(app, ui, "sync.signOut", json!({}));
                    } else {
                        app.cloud_sync.sign_out = Some(SignOutHow::Wait);
                    }
                }
            });
            if leaving {
                hint(
                    ui,
                    t,
                    crate::i18n::tr(
                        "Signing out waits until everything this device has made has reached the server, then removes the library from this device. Cancel stays signed in. Sign Out Anyway doesn't wait: what hasn't been sent is lost.",
                    ),
                );
                return;
            }
            if waiting {
                hint(
                    ui,
                    t,
                    crate::i18n::tr(
                        "This library and the server's both have photos. Add this library's photos to the server's, or use the server's library instead; nothing syncs until you choose.",
                    ),
                );
                return;
            }
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
            let mut all = c.all_previews;
            let label = match app.session.sync_previews_left() {
                Some((n, bytes)) if n > 0 && !all => {
                    use crate::sync_ui::{bytes_label, count_label};
                    format!("Download all small previews ({} photos, about {})", count_label(n as u64), bytes_label(bytes))
                }
                _ => crate::i18n::tr("Download all small previews").to_string(),
            };
            if check(ui, "sync.allPreviews", &mut all, &label) {
                sync_cmd(app, ui, "sync.allPreviews", json!({"on": all}));
            }
            hint(
                ui,
                t,
                crate::i18n::tr(
                    "Otherwise the grid's small previews come down as you scroll to them. All of them: the whole library can be browsed offline.",
                ),
            );
            // the downloaded originals' budget (MB; 0: none); with every original kept it has no effect
            let mut budget = c.originals_budget_mb.unwrap_or(0);
            let sizes = [
                (0u64, "No limit"),
                (5_000, "5 GB"),
                (10_000, "10 GB"),
                (25_000, "25 GB"),
                (50_000, "50 GB"),
                (100_000, "100 GB"),
                (250_000, "250 GB"),
            ];
            if !c.store_originals && pick(ui, t, "syncOriginalsBudget", crate::i18n::tr("Originals limit"), &sizes, &mut budget) {
                sync_cmd(app, ui, "sync.originalsBudget", json!({"mb": (budget > 0).then_some(budget)}));
            }
            hint(
                ui,
                t,
                crate::i18n::tr(
                    "Originals downloaded from the server are deleted, the ones used longest ago first, when they take more than this. Only ones the server has; photos you are working on or made available offline keep theirs.",
                ),
            );
            let mut by_server = c.server_builds_previews();
            if check(ui, "sync.serverPreviews", &mut by_server, "Let the server build the previews of photos added here") {
                sync_cmd(app, ui, "sync.serverPreviews", json!({"on": by_server}));
            }
            hint(
                ui,
                t,
                crate::i18n::tr(
                    "The server decodes each original it is sent, so this device doesn't: that saves battery on a phone and keeps a browser tab responsive. Off: the previews are built here, before the photo goes up.",
                ),
            );
            let offline = s["offlineAlbums"].as_array().map_or(0, Vec::len);
            if offline > 0 {
                hint(ui, t, &format!("{offline} album(s) available offline"));
            }
            shared_links(app, ui, t);
        }
        other => {
            let known = other.map(|(_, _, c)| c);
            if let Some(c) = &known {
                if app.cloud_sync.form.server.is_empty() {
                    app.cloud_sync.form.server = c.server.clone();
                    app.cloud_sync.form.user = c.user.clone();
                }
                if let Some(e) = app.session.sync_state().and_then(|st| st.error()) {
                    info(ui, t, crate::i18n::tr("Status"), e, t.caution);
                }
            }
            if crate::is_compact(ui.ctx()) {
                heading(ui, t, crate::i18n::tr("Account"));
            }
            let mut form = std::mem::take(&mut app.cloud_sync.form);
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
            check(ui, "sync.merge", &mut form.merge, "Combine with the photos already on the server");
            row(ui, t, "", |ui| {
                let ready = !form.server.trim().is_empty() && !form.user.trim().is_empty() && !form.password.is_empty();
                if action(ui, t, "syncSignIn", "Sign In", ready, false) {
                    let p = json!({"server": form.server.trim(), "user": form.user.trim(), "password": form.password, "merge": form.merge});
                    form.password.clear();
                    sync_cmd(app, ui, "sync.signIn", p);
                }
            });
            app.cloud_sync.form = form;
            hint(
                ui,
                t,
                crate::i18n::tr(
                    "A library signed in to an empty server uploads its photos there. Signing in from a new, empty library gets the server's. If both have photos, you choose: add this library to the server's, or use the server's instead. Ticking Combine merges them at once: photos are matched by what they are, and nothing is removed on either side.",
                ),
            );
            // joined to an account's library, and signed out by the server: sign in again as that user, or remove it
            let bound = known.as_ref().is_some_and(|c| !c.library.is_empty());
            app.cloud_sync.sign_out = app.cloud_sync.sign_out.filter(|h| bound && *h == SignOutHow::Remove);
            if let Some(how) = app.cloud_sync.sign_out {
                sign_out_question(app, ui, t, how);
            } else if bound {
                if crate::is_compact(ui.ctx()) {
                    ui.add_space(28.0);
                }
                row(ui, t, "", |ui| {
                    if action(ui, t, "syncRemoveLibrary", "Remove Library from This Device…", true, true) {
                        app.cloud_sync.sign_out = Some(SignOutHow::Remove);
                    }
                });
                hint(
                    ui,
                    t,
                    crate::i18n::tr(
                        "This library belongs to the account it was signed in as, and the server signed this device out. Sign in as the same user to go on, or remove the library from this device to sign in as someone else: what hasn't been sent is lost.",
                    ),
                );
            }
            // (last: a page that is long already keeps the buttons above in reach)
            cloud_folder_section(app, ui, t);
        }
    }
}

/// Syncing through a folder of iCloud Drive, Dropbox or Google Drive instead of a server (`docs/sync.md`).
fn cloud_folder_section(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    heading(ui, t, crate::i18n::tr("Without a Server"));
    hint(
        ui,
        t,
        crate::i18n::tr(
            "Keep this library's presets, settings and edits in step with your other computers through a folder your own iCloud Drive, Dropbox or Google Drive already syncs. LightCraft writes only to a folder named LightCraft Sync inside it.",
        ),
    );
    if let Some(chosen) = app.session.icloud_folder_info() {
        let path = chosen["path"].as_str().unwrap_or_default().to_string();
        info(ui, t, crate::i18n::tr("Folder"), &path, t.text);
        let (text, problem) = cloud_folder_status(&chosen["last"]);
        let r = info(ui, t, crate::i18n::tr("Status"), &text, if problem { t.caution } else { t.text });
        register(ui.ctx(), "label:cloudFolderStatus", r);
        row(ui, t, "", |ui| {
            if action(ui, t, "cloudFolderNow", "Sync Now", true, false) {
                sync_cmd(app, ui, "icloud.folderNow", json!({}));
            }
            if action(ui, t, "cloudFolderStop", "Stop Syncing", true, true) {
                sync_cmd(app, ui, "icloud.useFolder", json!({"path": null}));
            }
        });
        return;
    }
    let folders = app.cloud_sync.folders.get_or_insert_with(lightcraft_engine::icloud::folder::detected_cloud_folders).clone();
    let can_pick = app.services.pick_folder.is_some() || app.services.picker.is_some();
    if folders.is_empty() && !can_pick {
        hint(ui, t, crate::i18n::tr("No iCloud Drive, Dropbox or Google Drive folder was found on this computer."));
        return;
    }
    row(ui, t, "", |ui| {
        for (provider, path) in &folders {
            let id = format!("cloudFolder:{provider}");
            if action(ui, t, &id, &crate::i18n::tr_format!("Use {provider}", provider = provider), true, false) {
                sync_cmd(app, ui, "icloud.useFolder", json!({"path": path}));
            }
        }
        if can_pick && action(ui, t, "cloudFolderChoose", "Choose Folder…", true, false) {
            sync_cmd(app, ui, "icloud.chooseFolder", json!({}));
        }
    });
}

/// How the last pass through the cloud folder went, in words.
fn cloud_folder_status(last: &Value) -> (String, bool) {
    let Some(at) = last["at"].as_i64() else { return (crate::i18n::tr("Starting…").to_string(), false) };
    if let Some(e) = last["error"].as_str() {
        return (crate::i18n::tr_format!("Can't sync: {error}", error = e), true);
    }
    let waiting = last["report"]["waiting"].as_u64().unwrap_or(0);
    let secs = (web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64) - at).max(0) / 1000;
    let text = match secs {
        0..=59 => crate::i18n::tr("Synced just now").to_string(),
        60..=3599 => crate::i18n::tr_format!("Synced {n} min ago", n = secs / 60),
        _ => crate::i18n::tr_format!("Synced {n} h ago", n = secs / 3600),
    };
    if waiting > 0 {
        return (crate::i18n::tr_format!("{status}; waiting for {n} file(s) from the provider", status = text, n = waiting), false);
    }
    (text, false)
}

/// What a sign-out button asked, asked back: what the answer does, then the answer. `sync.signOut` takes
/// `confirm` for the graceful one (it waits for what is left to be sent) and `force` for the others.
fn sign_out_question(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens, how: SignOutHow) {
    let (title, first, rest, confirm) = match how {
        SignOutHow::Wait => (
            "Sign Out?",
            "This removes this account's library from this device.",
            "It stays on the server, and signing in again brings it back. Anything that hasn't reached the server yet is sent first.",
            "Sign Out",
        ),
        SignOutHow::Anyway => (
            "Sign Out Without Waiting?",
            "Some changes or photos haven't reached the server.",
            "Signing out now removes this library from this device, and what hasn't been sent is lost.",
            "Sign Out Anyway",
        ),
        SignOutHow::Remove => (
            "Remove Library from This Device?",
            "The server signed this device out, so nothing can be sent.",
            "Removing the library from this device loses what hasn't been sent. Signing in again brings the rest back from the server.",
            "Remove Library",
        ),
    };
    heading(ui, t, title);
    if how == SignOutHow::Remove {
        let account = app.session.sync_state().map_or_else(String::new, |st| {
            let host = st.config.server.trim_start_matches("https://").trim_start_matches("http://").trim_end_matches('/');
            format!("{} · {host}", st.config.user)
        });
        info(ui, t, crate::i18n::tr("Library"), &account, t.text);
    }
    let mut text = format!("{} {}", crate::i18n::tr(first), crate::i18n::tr(rest));
    // (a library that isn't a folder, a browser's, has no copy to keep)
    if how != SignOutHow::Wait && app.session.library.as_ref().is_some_and(|l| l.on_disk) {
        text.push(' ');
        text.push_str(crate::i18n::tr("A copy of the catalog is first saved in the library folder."));
    }
    hint(ui, t, &text);
    let (mut go, mut back) = (false, false);
    row(ui, t, "", |ui| {
        go = action(ui, t, "syncSignOutConfirm", confirm, true, true);
        back = action(ui, t, "syncSignOutBack", "Cancel", true, false);
    });
    if go {
        app.cloud_sync.sign_out = None;
        let p = if how == SignOutHow::Wait { json!({"confirm": true}) } else { json!({"force": true}) };
        sync_cmd(app, ui, "sync.signOut", p);
    } else if back {
        app.cloud_sync.sign_out = None;
    }
}

/// The album links this user has made: copy one, or take it back.
fn shared_links(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    heading(ui, t, crate::i18n::tr("Shared Links"));
    app.session.sync_refresh_shares(false);
    let (links, error) = app.session.sync_shares();
    let error = error.map(str::to_string);
    if links.is_empty() {
        hint(
            ui,
            t,
            crate::i18n::tr(
                "Right-click an album and choose Share Link… to make a link anyone can open in a browser, with no account. It shows the album's photos as you have edited them, can ask for a password or let people add photos, and you can take it back here.",
            ),
        );
    }
    let (mut copy, mut revoke) = (None, None);
    for (link, url) in &links {
        let ends = match link.expires {
            Some(e) => {
                format!("ends {}", lightcraft_catalog::dates::civil(i64::try_from(e).unwrap_or(i64::MAX)).chars().take(10).collect::<String>())
            }
            None => "no end".to_string(),
        };
        let mut what = vec![if link.originals { "originals too" } else { "pictures only" }];
        if link.password {
            what.push("password");
        }
        if link.upload {
            what.push("people add photos");
        }
        row(ui, t, &link.name, |ui| {
            ui.label(RichText::new(format!("{}, {ends}", what.join(", "))).color(t.text_dim));
            if action(ui, t, &format!("shareCopy{}", link.id), "Copy Link", true, false) {
                copy = Some(url.clone());
            }
            if action(ui, t, &format!("shareRevoke{}", link.id), "Revoke", true, true) {
                revoke = Some(link.id.clone());
            }
        });
    }
    if let Some(e) = error {
        hint(ui, t, &e);
    }
    if let Some(url) = copy {
        ui.ctx().copy_text(url);
        app.toast(ui.ctx(), "Link copied");
    }
    if let Some(id) = revoke {
        sync_cmd(app, ui, "album.unshare", json!({"id": id}));
    }
}

/// A disk's bar: what the library (`whose`) takes of it, what else is on it, what is free.
fn disk_bar(ui: &mut egui::Ui, t: &Tokens, whose: &str, library: u64, disk: lightcraft_catalog::sync::proto::Disk) {
    use crate::sync_ui::bytes_label;
    let used = disk.total.saturating_sub(disk.free);
    let legend = format!(
        "{}: {}\n{}: {}\n{}: {}",
        whose,
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
            if u.folders.files > 0 {
                info(ui, t, crate::i18n::tr("Library folders"), &files(u.folders), t.text);
            }
            // (everything the rows above take: the photos read in place from library folders are the library's too)
            info(ui, t, crate::i18n::tr("Total"), &bytes_label(u.total()), t.text);
            // (an admin's limit on what the server keeps for this user: library folders don't count)
            let full = u.quota.is_some_and(|q| u.stored() >= q.saturating_mul(9) / 10);
            if let Some(q) = u.quota {
                let text = format!("{} of {} used", bytes_label(u.stored()), bytes_label(q));
                info(ui, t, crate::i18n::tr("Storage limit"), &text, if full { t.caution } else { t.text });
            }
            if let Some(d) = u.disk {
                let text = format!("{} free of {}", bytes_label(d.free), bytes_label(d.total));
                info(ui, t, crate::i18n::tr("Server disk"), &text, t.text);
                disk_bar(ui, t, crate::i18n::tr("This library"), u.on_disk(), d);
            }
            if u.folders.files > 0 {
                hint(
                    ui,
                    t,
                    crate::i18n::tr(
                        "Library folders are read where they are on the server and never copied, so LightCraft keeps only their previews. The total counts their photos too: they take that room on the server.",
                    ),
                );
            }
            if full {
                hint(
                    ui,
                    t,
                    crate::i18n::tr(
                        "Nearly at the storage limit the server's admin set for you. Over it, new photos wait to upload until there is room; nothing is deleted.",
                    ),
                );
            }
        }
        None if error.is_none() => hint(ui, t, crate::i18n::tr("Asking the server…")),
        None => {}
    }
    if let Some(e) = &error {
        hint(ui, t, &format!("{}: {e}", crate::i18n::tr("The server can't say how much it stores")));
    }

    let Some(l) = app.session.local_dirs().and(app.cloud_sync.storage.local.clone()) else { return };
    let (photos, remote) = app.cloud_sync.storage.counts;
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
        disk_bar(ui, t, crate::i18n::tr("LightCraft"), l.total(), d);
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

/// Switching libraries is refused while an import (or the folder scan before one) is running: its
/// remaining batches would be committed into the new library (issue #1509).
pub(crate) fn can_switch_library(app: &LightcraftApp) -> Result<(), String> {
    if crate::lightroom_import::is_running(app) {
        return Err("wait for Lightroom catalog import to finish before switching libraries".into());
    }
    if app.import.is_some() || app.scan.is_some() {
        return Err("wait for the import to finish before switching libraries".into());
    }
    Ok(())
}

/// `app.openLibrary {path?}`: close the current library and open (or create) the one at `path`,
/// or a folder chosen in a dialog. Remembered as the library to open at launch.
///
/// A Lightroom Classic catalog (its folder, an `.lrdata` / `.lrcat-data` folder or the `.lrcat`
/// itself, see [`lightcraft_engine::lightroom_folder`]) is not opened: the current library stays
/// and a dialog offers Import Lightroom Catalog… instead; returns `{opened: false,
/// lightroomCatalog: <.lrcat path or null>, path}`.
pub fn open_library(app: &mut LightcraftApp, p: &Value) -> Result<Value, String> {
    can_switch_library(app)?;
    let path = match p.get("path").and_then(Value::as_str) {
        Some(x) => x.to_string(),
        None => {
            let req = crate::pick::PickRequest::folder(crate::i18n::tr("Open Library"));
            match crate::pick::ask(app, "app.openLibrary", p, "path", req, |s| s.pick_folder.as_mut().and_then(|f| f()).map(|x| vec![x])) {
                crate::pick::Picked::Now(v) => match v.into_iter().next() {
                    Some(x) => x,
                    None => return Ok(Value::Null),
                },
                crate::pick::Picked::Later => return Ok(Value::Null),
                crate::pick::Picked::Unavailable => return Err("no folder dialog on this platform".into()),
            }
        }
    };
    // A Lightroom catalog folder (or part of one) is not a library: before closing the library
    // open now, offer to import it instead (issue #891). The engine refuses it too.
    if let Some(place) = lightcraft_engine::lightroom_folder::lightroom_catalog_at(std::path::Path::new(&path)) {
        let catalog = place.catalog.map(|c| c.to_string_lossy().into_owned());
        let err = lightcraft_engine::EngineError::LightroomCatalog { path: path.clone().into(), catalog: catalog.clone().map(Into::into) };
        // the library problem window (no library could be opened) shows the error instead
        if app.library_problem.is_some() {
            return Err(err.to_string());
        }
        app.ui.dialog = Some(crate::state::Dialog::LightroomCatalog { folder: path.clone(), catalog: catalog.clone() });
        return Ok(json!({"opened": false, "lightroomCatalog": catalog, "path": path}));
    }
    // a remembered library that isn't there (moved, or on a disk or network share that isn't connected) is said
    // so, never made anew in its place, and stays in Open Recent Library for when its share is back; a new one is
    // made only in a folder that is there
    let remembered = app.ui.settings.recent_libraries.contains(&path);
    if let Some(why) = lightcraft_apphost::libraries::missing(std::path::Path::new(&path), remembered) {
        return Err(why);
    }
    // a drive's or a share's top folder opens the one library in it (several: said so); a folder with none becomes
    // a library, unless it was one (a share's empty mount point)
    let dir = std::path::Path::new(&path);
    let path = if dir.is_dir() { lightcraft_apphost::libraries::find(dir, remembered)?.to_string_lossy().into_owned() } else { path };
    switch_library(app, &path)
}

/// Close the current library and open the one at `path` ([`lightcraft_apphost::libraries::switch`]:
/// checked before anything is closed, the previous library opened again if it still fails), then
/// let go of what the window showed of the old one and keep both in the recent list.
fn switch_library(app: &mut LightcraftApp, path: &str) -> Result<Value, String> {
    let generation = app.session.library_generation();
    let switched = lightcraft_apphost::libraries::switch(&mut app.session, std::path::Path::new(path));
    // (refused before anything was closed: the window keeps what it shows)
    if app.session.library_generation() != generation {
        app.lightroom.last = None;
        app.lightroom_result = None;
        if matches!(app.ui.dialog.as_ref(), Some(crate::state::Dialog::LightroomResult { .. })) {
            app.ui.dialog = None;
        }
        app.renderer.forget_all();
    }
    let switched = switched.map_err(|e| e.to_string())?;
    // Photo IDs are local to a catalog: pins cannot follow them into another library (a refused
    // or failed switch reopens the same catalog, so its pins stay).
    if app.session.library_generation() != generation {
        app.ui.reference = None;
        app.ui.compare = None;
        if app.ui.view == crate::state::ViewMode::Reference {
            app.ui.view = crate::state::ViewMode::PhotoGrid;
        }
    }
    let path = switched.dir.to_string_lossy().into_owned();
    app.ui.settings.library_path = path.clone();
    let recent = &mut app.ui.settings.recent_libraries;
    if let Some(prev) = switched.previous.map(|p| p.to_string_lossy().into_owned()).filter(|p| p != &path && !recent.contains(p)) {
        lightcraft_apphost::libraries::remember(recent, &prev);
    }
    lightcraft_apphost::libraries::remember(recent, &path);
    Ok(json!({"path": path, "name": lightcraft_apphost::libraries::name(&switched.dir), "photos": app.session.catalog.len()}))
}

/// The open library's name (`None`: in memory, nothing is saved).
pub fn open_library_name(app: &LightcraftApp) -> Option<String> {
    lightcraft_apphost::libraries::open_dir(&app.session).map(lightcraft_apphost::libraries::name)
}

/// `app.newLibrary {path?, name?}`: create a library and switch to it. With `name`, the library
/// is the folder `name` inside `path` (or inside a location chosen in a dialog); without it,
/// `path` itself, or a new folder named after the default library. The folder must not hold a
/// library already, nor anything else.
pub fn new_library(app: &mut LightcraftApp, p: &Value) -> Result<Value, String> {
    can_switch_library(app)?;
    let name = match p.get("name") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(lightcraft_apphost::libraries::valid_name(s).map_err(|e| format!("app.newLibrary: {e}"))?),
        Some(_) => return Err("app.newLibrary: name must be a string".into()),
    };
    // from the menu: ask for the name first (default prefilled), then for the location
    if name.is_none() && p.get("path").is_none() && app.services.pick_folder.is_some() {
        crate::panels::dialogs::prompt(
            app,
            "New Library",
            "Library name",
            lightcraft_engine::library::DEFAULT_NAME,
            "app.newLibrary",
            json!({}),
            "name",
        );
        return Ok(Value::Null);
    }
    let (location, picked) = match p.get("path").and_then(Value::as_str) {
        Some(x) => (std::path::PathBuf::from(x), false),
        None => {
            let req = crate::pick::PickRequest::folder(crate::i18n::tr("New Library: Choose Where to Put It"));
            match crate::pick::ask(app, "app.newLibrary", p, "path", req, |s| s.pick_folder.as_mut().and_then(|f| f()).map(|x| vec![x])) {
                crate::pick::Picked::Now(v) => match v.into_iter().next() {
                    Some(x) => (std::path::PathBuf::from(x), true),
                    None => return Ok(Value::Null),
                },
                crate::pick::Picked::Later => return Ok(Value::Null),
                crate::pick::Picked::Unavailable => return Err("no folder dialog on this platform".into()),
            }
        }
    };
    let dir = lightcraft_apphost::libraries::new_dir(&location, name, picked)?;
    switch_library(app, &dir.to_string_lossy())
}

/// `app.recentLibraries {remove?: path, clear?: bool}`: the recently opened libraries, newest
/// first, as `[{path, name, exists, checking, open, lastOpened}]`. `exists` is what the last
/// background check found (`false`: moved, on a disconnected drive, not a folder or no longer a
/// library; `null`: not checked yet, or the check couldn't tell: a folder that can't be listed, an
/// unreadable catalog, a lookup or check that failed; `checking` says whether a check is running):
/// the command never looks at a folder itself, as one on an offline network share can take
/// seconds to answer. `lastOpened`: ISO 8601 time, `null` when not known. `remove` forgets that
/// one entry (an error if the list doesn't hold it) and `clear` forgets them all; neither touches
/// anything on disk (issue #1290).
pub fn recent_libraries(app: &mut LightcraftApp, p: &Value) -> Result<Value, String> {
    match p.get("remove") {
        None | Some(Value::Null) => {}
        Some(Value::String(path)) => {
            if !forget_library(&mut app.ui.settings, path) {
                return Err(format!("app.recentLibraries: {path:?} is not in the recent libraries"));
            }
            app.recent_checks.forget(path);
        }
        Some(_) => return Err("app.recentLibraries: remove must be a library path (a string)".into()),
    }
    if p.get("clear").and_then(Value::as_bool) == Some(true) {
        for path in std::mem::take(&mut app.ui.settings.recent_libraries) {
            app.recent_checks.forget(&path);
        }
        app.ui.settings.recent_opened.clear();
    }
    let open = lightcraft_apphost::libraries::open_dir(&app.session);
    let (settings, checks) = (&app.ui.settings, &app.recent_checks);
    let list: Vec<Value> = settings
        .recent_libraries
        .iter()
        .map(|r| {
            let d = std::path::Path::new(r);
            json!({
                "path": r,
                "name": library_name(d),
                "exists": checks.exists(r),
                "checking": checks.is_checking(r),
                "open": open.is_some_and(|o| lightcraft_apphost::libraries::same(o, d)),
                "lastOpened": settings.recent_opened.get(r),
            })
        })
        .collect();
    Ok(Value::Array(list))
}

/// Forget `path` in File ▸ Open Recent Library (and when it was opened); the library itself is
/// left where it is. False if the list didn't hold it.
pub fn forget_library(settings: &mut crate::state::AppSettings, path: &str) -> bool {
    let before = settings.recent_libraries.len();
    settings.recent_libraries.retain(|r| r != path);
    settings.recent_opened.remove(path);
    settings.recent_libraries.len() != before
}

/// A library's name as people see it: its folder's name.
pub fn library_name(dir: &std::path::Path) -> String {
    lightcraft_apphost::libraries::name(dir)
}

/// Put `path` first in the recent list ([`lightcraft_apphost::libraries::remember`]).
pub fn remember_library(recent: &mut Vec<String>, path: &str) {
    lightcraft_apphost::libraries::remember(recent, path);
}

/// `base` inside `location`, or `base 2`, `base 3`… if that is taken.
pub(crate) fn free_name(location: &std::path::Path, base: &str) -> std::path::PathBuf {
    lightcraft_apphost::libraries::free_name(location, base)
}

// ---------------------------------------------------------------------------- Display profile

/// `app.displayProfile {path?: string | null}`: show previews through this monitor ICC profile
/// (`""` or `null`: none, the profile of the screen showing the window where the host knows it
/// (macOS), else the display is treated as sRGB) and keep it as the setting; no `path`: report
/// only. A file that can't be used is an error and changes nothing. Returns the profile in use:
/// `{path, description, kind (matrix|lut), primaries {red, green, blue: [x, y]}}`, with
/// `path: null, screen: true` for the screen's profile.
pub fn display_profile(app: &mut LightcraftApp, p: &Value) -> Result<Value, String> {
    let path = match p.get("path") {
        None => None,
        Some(Value::Null) => Some(String::new()),
        Some(Value::String(s)) => Some(s.trim().to_string()),
        Some(_) => return Err("app.displayProfile: path must be a string or null".into()),
    };
    if let Some(path) = path {
        let d = lightcraft_engine::display::Display::load_opt(&path).map_err(|e| format!("app.displayProfile: {e}"))?;
        app.renderer.set_display(d);
        app.ui.settings.display_profile = path.clone();
        app.display_applied = Some(path);
        app.display_error = None;
        app.screen_profile = None;
        app.screen_profile_read = None;
        if app.ui.settings.display_profile.is_empty() {
            app.apply_screen_profile();
        }
    }
    Ok(match app.renderer.display() {
        Some(d) if d.path == crate::SCREEN_PROFILE => {
            let mut v = d.describe();
            v["path"] = Value::Null;
            v["screen"] = json!(true);
            v
        }
        Some(d) => d.describe(),
        None => json!({"path": null, "description": "sRGB (no display profile)"}),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn app_with_import() -> LightcraftApp {
        let mut app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), crate::Services::default());
        app.run("photo.setReference", json!({})).unwrap();
        app.import = Some(crate::import::ImportTask::new(vec!["a.jpg".into()], json!({"mode": "add"}), 0, false));
        app
    }

    #[test]
    fn open_library_is_refused_while_an_import_runs() {
        let mut app = app_with_import();
        let pin = app.ui.reference;
        let err = open_library(&mut app, &json!({"path": "other"})).unwrap_err();
        assert!(err.contains("import to finish"), "{err}");
        assert!(app.import.is_some());
        assert_eq!(app.ui.reference, pin, "a refused Open Library keeps the reference pin");
    }

    #[test]
    fn new_library_is_refused_while_an_import_runs() {
        let mut app = app_with_import();
        let pin = app.ui.reference;
        let err = new_library(&mut app, &json!({"path": "other"})).unwrap_err();
        assert!(err.contains("import to finish"), "{err}");
        assert!(app.import.is_some());
        assert_eq!(app.ui.reference, pin, "a refused New Library keeps the reference pin");
    }

    /// The menu items that switch libraries are greyed out meanwhile, like during a Lightroom import.
    #[test]
    fn library_switching_menu_items_are_disabled_while_an_import_runs() {
        let mut app = LightcraftApp::new(lightcraft_engine::Session::new(), crate::Services::default());
        app.services.pick_folder = Some(Box::new(|| None));
        assert!(crate::menus::ui_enabled(&app, "app.openLibrary"));
        assert!(crate::menus::ui_enabled(&app, "app.newLibrary"));
        app.import = Some(crate::import::ImportTask::new(vec!["a.jpg".into()], json!({"mode": "add"}), 0, false));
        assert!(!crate::menus::ui_enabled(&app, "app.openLibrary"));
        assert!(!crate::menus::ui_enabled(&app, "app.newLibrary"));
    }

    /// The folder scan before an import's review counts too: it ends in an import into the open library.
    #[test]
    fn open_library_is_refused_while_an_import_scan_runs() {
        let dir = std::env::temp_dir().join(format!("lc-ui-switchscan-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let mut app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), crate::Services::default());
        app.run("photo.setReference", json!({})).unwrap();
        let pin = app.ui.reference;
        crate::import::open(&mut app, vec![dir.to_string_lossy().into_owned()]).unwrap();
        assert!(app.scan.is_some());
        let err = open_library(&mut app, &json!({"path": "other"})).unwrap_err();
        assert!(err.contains("import to finish"), "{err}");
        assert_eq!(app.ui.reference, pin, "a refused Open Library during a scan keeps the reference pin");
        let err = new_library(&mut app, &json!({"path": "other"})).unwrap_err();
        assert!(err.contains("import to finish"), "{err}");
        assert_eq!(app.ui.reference, pin, "a refused New Library during a scan keeps the reference pin");
        assert!(app.scan.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod path_tests {
    use super::elide_path;

    /// Width = characters, so a limit of 20 is "20 characters".
    fn fit(path: &str, max: usize) -> String {
        elide_path(path, max as f32, |s| s.chars().count() as f32)
    }

    #[test]
    fn a_path_that_fits_is_left_alone() {
        assert_eq!(fit("/Users/me/Pictures", 18), "/Users/me/Pictures");
        assert_eq!(fit("", 5), "");
    }

    #[test]
    fn leading_folders_go_first_and_the_end_stays() {
        assert_eq!(fit("/Users/me/Pictures/Library", 20), "…/Pictures/Library");
        assert_eq!(fit("/Users/me/Pictures/Library", 10), "…/Library");
    }

    /// Windows paths: `Path::display` gives backslashes there.
    #[test]
    fn a_path_with_backslashes_keeps_its_end_too() {
        assert_eq!(fit(r"C:\Users\me\Pictures\Library", 20), r"…\Pictures\Library");
        assert_eq!(fit(r"C:\Users\me/Pictures\Library", 10), r"…\Library");
    }

    #[test]
    fn a_last_folder_too_long_by_itself_loses_its_end() {
        assert_eq!(fit("/Users/me/a-very-long-folder-name", 8), "a-very-…");
        assert_eq!(fit("no-separator-at-all", 8), "no-sepa…");
        assert_eq!(fit("/Users/Фотографии-из-отпуска", 8), "Фотогра…");
    }

    #[test]
    fn a_separator_at_the_end_is_not_where_the_name_starts() {
        assert_eq!(fit("/Users/me/Pictures/Library/", 11), "…/Library/");
        assert_eq!(fit("/Users/me/a-very-long-folder-name/", 8), "a-very-…");
        assert_eq!(fit(r"C:\Users\me\Library\", 11), r"…\Library\");
        assert_eq!(fit("///", 2), "…");
        assert_eq!(fit(r"\\server\share\folder", 15), r"…\share\folder");
    }
}
