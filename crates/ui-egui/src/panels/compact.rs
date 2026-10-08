//! The compact (phone-sized) layout, modelled on Lightroom's mobile app and drawn in iOS's own
//! style (`theme::Tokens::ios`): no menu bar. The grid has the collection as its title (a tap opens
//! the albums list), Select and a "…" menu; a photo has back, undo, share and "…" above it and the
//! tools below it, with the Edit tool's groups (Light, Color…) in a row of their own and the active
//! tool's panel as a bottom sheet. Dialogs are pages that slide up (`mobile::page`) and menus are
//! pull-downs or action sheets (`mobile::actions`). Panels reuse the desktop bodies
//! (`right::body`) and commands, so every control stays a `develop` control spec / command; the
//! rest of the menus' commands are in "All Commands" (`mobile::all_commands_page`).

use egui::{Align2, Sense, Stroke, pos2, vec2};
use serde_json::json;

use super::mobile;
use crate::LightcraftApp;
use crate::icons::Icon;
use crate::state::{RightPanel, ViewMode};
use crate::theme::Tokens;
use crate::widgets::icon_button;

/// Height of the top bar: Apple's minimum touch target is 44 pt.
const BAR_H: f32 = 44.0;
/// Height of the tool bar and of the select action bar (iOS's tab bar is 49 pt).
const TAB_H: f32 = 50.0;
/// Height of the Edit tool's group row (icon over label).
const GROUP_H: f32 = 54.0;
/// From this width (iPad) the tool sheet is a panel on the right and My Photos a column on the left,
/// instead of a bottom sheet and a page of their own.
pub const WIDE_PT: f32 = 600.0;
/// The side panels' width on a wide compact window.
const SIDE_W: f32 = 340.0;
/// The grabber at the top of the bottom sheet: drag it to make the sheet taller or shorter.
const GRABBER_H: f32 = 24.0;
/// A slider row's height, which the sheet's stops are counted in.
const SLIDER_ROW_H: f32 = crate::widgets::TOUCH_SLIDER_ROW_H;

/// The tools under a photo: (id, icon, panel, name). Presets is a panel of its own (`ui.presets`).
const TOOLS: [(&str, Icon, RightPanel, &str); 6] = [
    ("presets", Icon::Presets, RightPanel::None, "Presets"),
    ("crop", Icon::Crop, RightPanel::Crop, "Crop & Rotate"),
    ("edit", Icon::Sliders, RightPanel::Edit, "Edit"),
    ("masking", Icon::Mask, RightPanel::Masking, "Masking"),
    ("remove", Icon::Eraser, RightPanel::Remove, "Remove"),
    ("info", Icon::Info, RightPanel::Info, "Info"),
];

/// The Edit tool's groups, one shown at a time: (id = the desktop section, icon, name).
const GROUPS: [(&str, Icon, &str); 7] = [
    ("profile", Icon::ProfileGrid, "Profile"),
    ("light", Icon::Sun, "Light"),
    ("color", Icon::Drop, "Color"),
    ("effects", Icon::Vignette, "Effects"),
    ("detail", Icon::Detail, "Detail"),
    ("optics", Icon::Lens, "Optics"),
    ("calibration", Icon::Target, "Calibration"),
];

pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let t = Tokens::get(&ctx);
    let detail = app.ui.view == ViewMode::Detail;
    let wide = ctx.content_rect().width() >= WIDE_PT;
    if app.ui.select_mode && !matches!(app.ui.view, ViewMode::PhotoGrid | ViewMode::SquareGrid) {
        app.ui.select_mode = false;
    }
    // the albums list: a column on a tablet, a page sliding over the grid on a phone
    let albums_page = app.ui.left_panel && !wide;
    top_bar(app, ui, &t);
    if app.ui.select_mode {
        action_bar(app, ui, &t);
    }
    if detail {
        tool_bar(app, ui, &t);
        if !app.ui.presets && matches!(app.ui.right, RightPanel::Edit) {
            group_bar(app, ui, &t);
        }
        if app.ui.right != RightPanel::None || app.ui.presets {
            sheet(app, ui, &t, wide);
        }
    }
    let bg = if detail { t.canvas } else { t.grid_bg };
    if app.ui.left_panel && wide {
        egui::Panel::left("compact_sources")
            .resizable(false)
            .exact_size(SIDE_W)
            .frame(egui::Frame::NONE.fill(t.chrome).stroke(egui::Stroke::new(1.0, t.divider)))
            .show(ui, |ui| super::left::body(app, ui));
    }
    content(app, ui, bg);
    if albums_page {
        albums(app, &ctx);
    } else {
        mobile::hidden(&ctx, "albums");
    }
    if !albums_page && !app.ui.select_mode {
        add_button(app, &ctx, &t);
    }
    menus(app, &ctx);
    overlays(app, &ctx);
    mobile::all_commands_page(app, &ctx);
}

/// The albums list (My Photos) as a page; choosing one closes it.
fn albums(app: &mut LightcraftApp, ctx: &egui::Context) {
    let src = app.session.source;
    let bar = mobile::page(ctx, "albums", "Albums", "Done", None, |ui| super::left::body(app, ui));
    if bar.cancel || app.session.source != src {
        app.ui.left_panel = false;
    }
}

/// Diameter of the add-photos button.
const ADD_D: f32 = 56.0;

/// The phone's add-photos button: a round + over the grid's bottom right corner, offering the
/// host's pickers (Photos, Files, a folder) in a menu. Only in hosts that have them (iOS).
fn add_button(app: &mut LightcraftApp, ctx: &egui::Context, t: &Tokens) {
    if app.services.host_pick.is_none() || !matches!(app.ui.view, ViewMode::PhotoGrid | ViewMode::SquareGrid) {
        return;
    }
    let screen = ctx.content_rect();
    let at = pos2(screen.right() - 20.0 - ADD_D, screen.bottom() - 20.0 - ADD_D);
    egui::Area::new(egui::Id::new("compact_add")).order(egui::Order::Foreground).fixed_pos(at).show(ctx, |ui| {
        let (r, resp) = ui.allocate_exact_size(vec2(ADD_D, ADD_D), Sense::click());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, crate::i18n::tr("Add Photos")));
        crate::widgets::register(ui.ctx(), "button:addPhotos", r);
        let p = ui.painter();
        // pressed: a little smaller and darker, as iOS buttons respond to a touch
        let down = resp.is_pointer_button_down_on();
        let k = ui.ctx().animate_bool_with_time(resp.id.with("press"), down, 0.1);
        let fill = t.accent.gamma_multiply(1.0 - 0.2 * k);
        p.circle_filled(r.center() + vec2(0.0, 2.0), ADD_D / 2.0, egui::Color32::from_black_alpha(80));
        p.circle_filled(r.center(), ADD_D / 2.0 * (1.0 - 0.06 * k), fill);
        let (c, arm) = (r.center(), ADD_D * 0.2);
        let stroke = Stroke::new(2.5, egui::Color32::WHITE);
        p.line_segment([c - vec2(arm, 0.0), c + vec2(arm, 0.0)], stroke);
        p.line_segment([c - vec2(0.0, arm), c + vec2(0.0, arm)], stroke);
        if resp.clicked() {
            mobile::open_menu(ui.ctx(), "add", r);
        }
    });
}

/// Every menu and action sheet of the compact layout (each draws only while it's open).
fn menus(app: &mut LightcraftApp, ctx: &egui::Context) {
    mobile::actions(ctx, "add", None, |ui| {
        for (id, icon, label) in [
            ("file.importFromPhotos", Icon::Photos, "From Photos…"),
            ("file.importFromFiles", Icon::Folder, "From Files…"),
            ("file.importFolderFromFiles", Icon::Folder, "Folder from Files…"),
        ] {
            if mobile::row(ui, id, Some(icon), crate::i18n::tr(label), true)
                && let Err(e) = app.run(id, json!({}))
            {
                app.toast(ui.ctx(), e);
            }
        }
    });
    mobile::actions(ctx, "more", None, |ui| grid_more(app, ui));
    mobile::actions(ctx, "sort", Some("Sort"), |ui| sort_items(app, ui));
    mobile::actions(ctx, "photoMore", None, |ui| photo_more(app, ui));
    let ids: Vec<u64> = app.session.selection.ids.iter().map(|id| id.0).collect();
    mobile::actions(ctx, "selRate", Some("Rating"), |ui| rating_items(app, ui, &ids));
    mobile::actions(ctx, "selFlag", Some("Flag"), |ui| flag_items(app, ui, &ids));
    mobile::actions(ctx, "selLabel", Some("Color Label"), |ui| label_items(app, ui, &ids));
    mobile::actions(ctx, "selAlbum", Some("Add to Album"), |ui| album_items(app, ui, &ids));
    mobile::actions(ctx, "selMore", None, |ui| more_items(app, ui, &ids));
}

/// Runs a command from a menu row, saying why it failed.
fn run_cmd(app: &mut LightcraftApp, ctx: &egui::Context, id: &str, params: serde_json::Value) {
    if let Err(e) = crate::menubar::run_item(app, id, params) {
        app.toast(ctx, e);
    }
}

/// The grid's "…" menu: importing, albums, sorting, the grid's look, settings and help.
fn grid_more(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    if app.services.host_pick.is_none() && mobile::row(ui, "addPhotosDesktop", Some(Icon::Plus), crate::i18n::tr("Import Photos…"), true) {
        run_cmd(app, &ctx, "file.addPhotos", json!({}));
    }
    if mobile::row(ui, "newAlbum", Some(Icon::Album), crate::i18n::tr("New Album…"), true) {
        run_cmd(app, &ctx, "dialog.newAlbum", json!({}));
    }
    mobile::row_gap(ui);
    if mobile::row(ui, "sortMenu", Some(Icon::Sort), crate::i18n::tr("Sort"), true) {
        mobile::open_actions(&ctx, "sort");
    }
    mobile::row_gap(ui);
    if mobile::row_checked(ui, "filterBar", Some(Icon::Filter), crate::i18n::tr("Filter Bar"), true, Some(app.ui.filter_bar)) {
        run_cmd(app, &ctx, "view.filterBar", json!({}));
    }
    let square = app.ui.view == ViewMode::SquareGrid;
    if mobile::row_checked(ui, "squareGrid", Some(Icon::GridSquare), crate::i18n::tr("Square Thumbnails"), true, Some(square)) {
        run_cmd(app, &ctx, if square { "view.photoGrid" } else { "view.squareGrid" }, json!({}));
    }
    mobile::row_gap(ui);
    if mobile::row(ui, "settings", Some(Icon::Gear), crate::i18n::tr("Settings"), true) {
        run_cmd(app, &ctx, "app.settings", json!({}));
    }
    if mobile::row(ui, "help", Some(Icon::Help), crate::i18n::tr("Help"), true) {
        run_cmd(app, &ctx, "app.help", json!({}));
    }
    if mobile::row(ui, "about", Some(Icon::Info), crate::i18n::tr("About LightCraft"), true) {
        run_cmd(app, &ctx, "app.about", json!({}));
    }
    mobile::row_gap(ui);
    if mobile::row(ui, "allCommands", Some(Icon::Search), crate::i18n::tr("All Commands…"), true) {
        app.ui.all_commands = true;
    }
}

/// Sort order: the key (checked), then oldest / newest first.
fn sort_items(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let cur = serde_json::to_value(app.session.sort.key).ok();
    for (key, label) in [
        ("captureDate", "Capture Time"),
        ("importDate", "Date Added"),
        ("editDate", "Edit Time"),
        ("fileName", "File Name"),
        ("rating", "Rating"),
        ("fileSize", "File Size"),
    ] {
        if mobile::row_checked(
            ui,
            &format!("sort-{key}"),
            None,
            crate::i18n::tr(label),
            true,
            Some(cur.as_ref().and_then(|v| v.as_str()) == Some(key)),
        ) {
            run_cmd(app, &ctx, "library.sort", json!({"key": key}));
        }
    }
    mobile::row_gap(ui);
    let asc = app.session.sort.ascending;
    if mobile::row_checked(ui, "sort-ascending", None, crate::i18n::tr("Ascending"), true, Some(asc)) {
        run_cmd(app, &ctx, "library.sort", json!({"ascending": true}));
    }
    if mobile::row_checked(ui, "sort-descending", None, crate::i18n::tr("Descending"), true, Some(!asc)) {
        run_cmd(app, &ctx, "library.sort", json!({"ascending": false}));
    }
}

/// A photo's "…" menu: rating, flag and label, then copying edits, its other panels, deleting.
fn photo_more(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let Some(id) = app.session.active() else { return };
    egui::Frame::NONE.inner_margin(egui::Margin::symmetric(10, 4)).show(ui, |ui| culling_menu(app, ui, &[id.0]));
    mobile::row_gap(ui);
    if mobile::row(ui, "copySettings", None, crate::i18n::tr("Copy Edit Settings"), true) {
        run_cmd(app, &ctx, "develop.copy", json!({}));
        app.toast(&ctx, crate::i18n::tr("Edit settings copied"));
    }
    if mobile::row(ui, "pasteSettings", None, crate::i18n::tr("Paste Edit Settings"), app.session.clipboard.is_some()) {
        run_cmd(app, &ctx, "develop.paste", json!({}));
    }
    if mobile::row(ui, "resetEdits", None, crate::i18n::tr("Reset All Edits"), true) {
        run_cmd(app, &ctx, "develop.reset", json!({}));
    }
    mobile::row_gap(ui);
    for (cmd, icon, label) in
        [("panel.versions", Icon::Versions, "Versions"), ("panel.activity", Icon::Activity, "History"), ("panel.keywords", Icon::Tag, "Keywords")]
    {
        if mobile::row(ui, cmd, Some(icon), crate::i18n::tr(label), true) {
            app.ui.presets = false;
            run_cmd(app, &ctx, cmd, json!({}));
        }
    }
    mobile::row_gap(ui);
    let ba = app.ui.before_after != crate::state::BeforeAfter::Off;
    if mobile::row_checked(ui, "beforeAfter", Some(Icon::BeforeAfter), crate::i18n::tr("Before / After"), true, Some(ba)) {
        run_cmd(app, &ctx, "view.beforeAfter", json!({}));
    }
    mobile::row_gap(ui);
    if mobile::row(ui, "deletePhoto", Some(Icon::Trash), crate::i18n::tr("Delete Photo"), true) {
        run_cmd(app, &ctx, "photo.delete", json!({}));
    }
    if mobile::row(ui, "allCommands", Some(Icon::Search), crate::i18n::tr("All Commands…"), true) {
        app.ui.all_commands = true;
    }
}

fn content(app: &mut LightcraftApp, ui: &mut egui::Ui, bg: egui::Color32) {
    egui::CentralPanel::default().frame(egui::Frame::NONE.fill(bg)).show(ui, |ui| match app.ui.view {
        ViewMode::Detail => super::detail::show(app, ui),
        ViewMode::Compare => super::compare::show_compare(app, ui),
        ViewMode::Survey => super::compare::show_survey(app, ui),
        ViewMode::Reference => super::compare::show_reference(app, ui),
        ViewMode::People => super::people::show(app, ui),
        ViewMode::PhotoGrid | ViewMode::SquareGrid => super::grid::show(app, ui),
    });
}

fn overlays(app: &mut LightcraftApp, ctx: &egui::Context) {
    let ctx = ctx.clone();
    super::second::show(app, &ctx);
    super::notices::show(app, &ctx);
    super::dialogs::show(app, &ctx);
    super::library_problem::show(app, &ctx);
    crate::import::progress(app, &ctx);
    crate::import::scan_progress(app, &ctx);
    crate::export_task::poll(app, &ctx);
    super::toast(app, &ctx);
}

/// An icon button in a bar (`icon:<id>`): white glyphs on black, dimmed while pressed.
fn bar_icon(ui: &mut egui::Ui, id: &str, icon: Icon, tip: &str, enabled: bool) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    let (r, resp) = ui.allocate_exact_size(vec2(44.0, 44.0), if enabled { Sense::click() } else { Sense::hover() });
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, crate::i18n::tr(tip)));
    crate::widgets::register(ui.ctx(), format!("icon:{id}"), r);
    let color = if !enabled {
        t.text_disabled
    } else if resp.is_pointer_button_down_on() {
        t.text_dim
    } else {
        t.text
    };
    crate::icons::paint(ui.painter(), egui::Rect::from_center_size(r.center(), vec2(24.0, 24.0)), icon, color);
    resp
}

fn top_bar(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    egui::Panel::top("compact_top")
        .show_separator_line(false)
        .exact_size(BAR_H)
        .frame(egui::Frame::NONE.fill(t.canvas).inner_margin(egui::Margin::symmetric(6, 0)))
        .show(ui, |ui| {
            let full = ui.max_rect();
            if app.ui.select_mode {
                select_bar(app, ui, t, full);
            } else if app.ui.view == ViewMode::Detail {
                photo_bar(app, ui);
            } else {
                grid_bar(app, ui, t);
            }
        });
}

/// Over the grid: Select and the "…" menu (the collection is the grid's own title row).
fn grid_bar(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let more = bar_icon(ui, "more", Icon::Dots, "More", true);
        if more.clicked() {
            mobile::open_menu(ui.ctx(), "more", more.rect);
        }
        let has_photos = !app.session.visible().is_empty();
        if has_photos && bar_text(ui, "select", "Select", t).clicked() {
            let _ = app.run("view.selectMode", json!({"on": true}));
        }
    });
}

/// Over a photo: back on the left; undo, share and "…" on the right (a long press on undo opens
/// the history).
fn photo_bar(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    ui.horizontal_centered(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        if bar_icon(ui, "back", Icon::ChevronLeft, "Back", true).clicked() {
            let _ = app.run("view.back", json!({}));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let has_photo = app.session.active().is_some();
            let more = bar_icon(ui, "photoMore", Icon::Dots, "More", has_photo);
            if more.clicked() {
                mobile::open_menu(ui.ctx(), "photoMore", more.rect);
            }
            if bar_icon(ui, "share", Icon::Share, "Export", has_photo).clicked() {
                let _ = app.run("dialog.export", json!({}));
            }
            let can_undo = lightcraft_engine::cmd::can_undo(&app.session).is_ok();
            let undo = bar_icon(ui, "undo", Icon::Undo, "Undo", can_undo);
            if undo.clicked() {
                let _ = app.run("edit.undo", json!({}));
            }
            if undo.long_touched() || undo.secondary_clicked() {
                app.ui.presets = false;
                let _ = app.run("panel.activity", json!({}));
            }
        });
    });
}

/// The grid's title row on a phone: the collection with a ▾ (a tap opens the albums list), its
/// photo count, and filter and sort buttons, as Lightroom's mobile app has it.
pub fn grid_header(app: &mut LightcraftApp, ui: &mut egui::Ui, hr: egui::Rect, count: &str) {
    let t = Tokens::get(ui.ctx());
    let title = crate::i18n::source_label(app.session.source, &app.session.catalog);
    let galley = ui.painter().layout_no_wrap(title, t.semibold(22.0), t.text);
    let title_r = egui::Rect::from_min_size(pos2(hr.left() + 16.0, hr.center().y - 20.0), vec2(galley.size().x + 30.0, 40.0));
    let resp = ui.interact(title_r, egui::Id::new("compact-collection"), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, crate::i18n::tr("Albums")));
    crate::widgets::register(ui.ctx(), "button:collections", title_r);
    let color = if resp.is_pointer_button_down_on() { t.text_dim } else { t.text };
    ui.painter().galley(pos2(title_r.left(), hr.center().y - galley.size().y / 2.0), galley.clone(), color);
    crate::icons::paint(
        ui.painter(),
        egui::Rect::from_center_size(pos2(title_r.left() + galley.size().x + 16.0, hr.center().y + 1.0), vec2(16.0, 16.0)),
        Icon::ChevronDown,
        color,
    );
    if resp.clicked() {
        let _ = app.run("view.leftPanel", json!({}));
    }
    let right = egui::Rect::from_min_max(pos2(hr.right() - 104.0, hr.top()), pos2(hr.right() - 8.0, hr.bottom()));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(right).layout(egui::Layout::right_to_left(egui::Align::Center)));
    child.spacing_mut().item_spacing.x = 0.0;
    let sort = bar_icon(&mut child, "sort", Icon::Sort, "Sort", true);
    if sort.clicked() {
        mobile::open_menu(ui.ctx(), "sort", sort.rect);
    }
    if bar_icon(&mut child, "filter", Icon::Filter, "Filter Bar", true).clicked() {
        let _ = app.run("view.filterBar", json!({}));
    }
    let after = title_r.right() + 8.0;
    if after + 60.0 < right.left() {
        ui.painter().text(pos2(after, hr.center().y + 2.0), Align2::LEFT_CENTER, count, t.font(13.0), t.text_dim);
    }
}

/// The tools under a photo, icons only; the open one on a blue rounded square. Tapping the open
/// tool closes its sheet.
fn tool_bar(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    // the bottom safe area is already outside this ui (see the host); the bar sits at its edge
    egui::Panel::bottom("compact_tabs").show_separator_line(false).exact_size(TAB_H).frame(egui::Frame::NONE.fill(t.canvas)).show(ui, |ui| {
        let has_photo = app.session.active().is_some();
        let full = ui.max_rect();
        ui.painter().hline(full.x_range(), full.top(), Stroke::new(0.5, t.divider));
        let w = full.width() / TOOLS.len() as f32;
        for (i, (id, icon, panel, tip)) in TOOLS.into_iter().enumerate() {
            let presets = id == "presets";
            let on = if presets {
                app.ui.presets
            } else {
                !app.ui.presets && (app.ui.right == panel || (panel == RightPanel::Edit && app.ui.right == RightPanel::Profiles))
            };
            let cell = egui::Rect::from_min_size(pos2(full.left() + i as f32 * w, full.top()), vec2(w, full.height()));
            let resp = ui.interact(cell, egui::Id::new(("compact-tool", id)), if has_photo { Sense::click() } else { Sense::hover() });
            resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, has_photo, on, crate::i18n::tr(tip)));
            crate::widgets::register(ui.ctx(), format!("icon:{id}"), cell);
            let k = ui.ctx().animate_bool_with_time(resp.id.with("on"), on, 0.15);
            let tile = egui::Rect::from_center_size(cell.center(), vec2(48.0, 38.0));
            if k > 0.0 {
                ui.painter().rect_filled(tile, 10.0, t.accent.gamma_multiply(k));
            }
            let color = if !has_photo {
                t.text_disabled
            } else if on {
                egui::Color32::WHITE
            } else if resp.is_pointer_button_down_on() {
                t.text_dim
            } else {
                t.icon
            };
            crate::icons::paint(ui.painter(), egui::Rect::from_center_size(cell.center(), vec2(24.0, 24.0)), icon, color);
            if resp.clicked() {
                // one sheet at a time; tapping the open tool closes it
                if presets {
                    let _ = app.run("panel.presets", json!({}));
                } else if on {
                    app.ui.right = RightPanel::None;
                } else {
                    app.ui.presets = false;
                    let _ = app.run(&format!("panel.{id}"), json!({}));
                }
            }
        }
    });
}

/// The Edit tool's groups in a row that scrolls sideways (icon over name), Auto first: a phone
/// shows one group's sliders at a time, as Lightroom's mobile app does.
fn group_bar(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    egui::Panel::bottom("compact_groups").show_separator_line(false).exact_size(GROUP_H).frame(egui::Frame::NONE.fill(t.canvas)).show(ui, |ui| {
        egui::ScrollArea::horizontal().id_salt("compact-groups").scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(
            ui,
            |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    ui.add_space(6.0);
                    if group_cell(ui, "auto", Icon::Wand, "Auto", false).clicked() {
                        let _ = app.run("develop.auto", json!({}));
                        app.toast(ui.ctx(), "Auto settings applied");
                    }
                    let (sep, _) = ui.allocate_exact_size(vec2(13.0, 34.0), Sense::hover());
                    ui.painter().vline(sep.center().x, sep.y_range(), Stroke::new(1.0, t.divider));
                    for (id, icon, label) in GROUPS {
                        if group_cell(ui, &format!("group-{id}"), icon, label, app.ui.edit_group == id).clicked() {
                            app.ui.edit_group = id.to_string();
                            if app.ui.right == RightPanel::Profiles {
                                app.ui.right = RightPanel::Edit;
                            }
                        }
                    }
                    ui.add_space(6.0);
                });
            },
        );
    });
}

/// One group in the group row (`button:<id>`): its icon over its name, on a grey tile when chosen.
fn group_cell(ui: &mut egui::Ui, id: &str, icon: Icon, label: &str, on: bool) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    let label = crate::i18n::tr(label);
    let (r, resp) = ui.allocate_exact_size(vec2(68.0, GROUP_H - 6.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, label));
    crate::widgets::register(ui.ctx(), format!("button:{id}"), r);
    let k = ui.ctx().animate_bool_with_time(resp.id.with("on"), on, 0.15);
    if k > 0.0 {
        ui.painter().rect_filled(r, 10.0, t.tool_active.gamma_multiply(k));
    }
    let color = if on || resp.is_pointer_button_down_on() { t.text } else { t.text_dim };
    crate::icons::paint(ui.painter(), egui::Rect::from_center_size(r.center() - vec2(0.0, 8.0), vec2(20.0, 20.0)), icon, color);
    ui.painter().text(r.center() + vec2(0.0, 14.0), Align2::CENTER_CENTER, label, t.font(11.5), color);
    resp
}

/// The tool sheet: a panel on the right on a tablet, a bottom sheet on a phone. The sheet has a
/// grabber: drag it to one of three heights (small, medium, large) or down to close the tool, tap
/// it to step through them. A sheet whose content is shorter than its height shrinks to fit (the
/// Profile group is one row), so the photo gets the room.
fn sheet(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens, wide: bool) {
    let avail = ui.max_rect().height();
    let frame = egui::Frame::NONE.fill(t.chrome);
    let body = |app: &mut LightcraftApp, ui: &mut egui::Ui| {
        if app.ui.presets { super::presets::body(app, ui) } else { super::right::body(app, ui) }
    };
    if wide {
        egui::Panel::right("compact_side").resizable(true).default_size(SIDE_W).size_range(280.0..=480.0).frame(frame).show(ui, |ui| body(app, ui));
        return;
    }
    let ctx = ui.ctx().clone();
    let stops = sheet_stops(avail);
    let detent = (app.ui.sheet_detent as usize).min(stops.len() - 1);
    // what the finger has made it while dragging the grabber, else the stop it was left at; never
    // taller than its content (measured by `right::body` a frame before)
    let drag_id = egui::Id::new("compact-sheet-drag");
    let dragging = ctx.data(|d| d.get_temp::<f32>(drag_id));
    let fit = ctx.data(|d| d.get_temp::<f32>(fit_id(app))).map(|c| c + GRABBER_H);
    let limit = |h: f32| fit.map_or(h, |f| h.min(f)).max(GRABBER_H + SLIDER_ROW_H);
    let h = match dragging {
        Some(h) => limit(h),
        None => ease_to(&ctx, egui::Id::new("compact-sheet-h"), limit(stops[detent])),
    };
    let frame = frame.corner_radius(egui::CornerRadius { nw: 12, ne: 12, sw: 0, se: 0 });
    egui::Panel::bottom("compact_sheet").show_separator_line(false).resizable(false).exact_size(h).frame(frame).show(ui, |ui| {
        let (g, resp) = ui.allocate_exact_size(vec2(ui.available_width(), GRABBER_H), Sense::click_and_drag());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, crate::i18n::tr("Resize Panel")));
        crate::widgets::register(ui.ctx(), "sheet:grabber", g);
        let pill = if resp.is_pointer_button_down_on() { t.text_dim } else { t.track };
        ui.painter().rect_filled(egui::Rect::from_center_size(g.center() + vec2(0.0, 1.0), vec2(36.0, 5.0)), 2.5, pill);
        if resp.dragged() {
            let now = dragging.unwrap_or(h) - ui.input(|i| i.pointer.delta().y);
            let now = now.clamp(0.0, stops[2].max(avail * 0.85));
            ctx.data_mut(|d| d.insert_temp(drag_id, now));
            ctx.data_mut(|d| d.insert_temp(egui::Id::new("compact-sheet-h"), limit(now)));
        }
        if resp.drag_stopped() {
            let dropped = dragging.unwrap_or(h);
            ctx.data_mut(|d| d.remove::<f32>(drag_id));
            if dropped < stops[0] - 60.0 {
                // dragged right down: the tool is put away
                app.ui.right = RightPanel::None;
                app.ui.presets = false;
            } else {
                let near = (0..stops.len()).min_by(|a, b| (limit(stops[*a]) - dropped).abs().total_cmp(&(limit(stops[*b]) - dropped).abs()));
                app.ui.sheet_detent = near.unwrap_or(detent) as u8;
            }
        } else if resp.clicked() {
            app.ui.sheet_detent = ((detent + 1) % stops.len()) as u8;
        }
        body(app, ui);
    });
}

/// The sheet's heights: about two and a half slider rows, four and a half (a part of the next row
/// shows that the rest scrolls), and most of the screen.
fn sheet_stops(avail: f32) -> [f32; 3] {
    let most = (avail * 0.68).max(GRABBER_H + 6.0 * SLIDER_ROW_H);
    let cap = avail * 0.8;
    [(GRABBER_H + 2.5 * SLIDER_ROW_H).min(cap), (GRABBER_H + 4.5 * SLIDER_ROW_H).min(cap), most.min(cap)]
}

/// Where the height of what the sheet shows now (the tool, and the Edit group) is kept.
fn fit_id(app: &LightcraftApp) -> egui::Id {
    egui::Id::new(("compact-sheet-fit", format!("{:?}", app.ui.right), app.ui.presets, app.ui.edit_group.clone()))
}

/// The tool panel says how tall its content is (`right::body`), so that the sheet needn't be taller.
pub fn note_sheet_content(app: &LightcraftApp, ctx: &egui::Context, height: f32) {
    ctx.data_mut(|d| d.insert_temp(fit_id(app), height));
}

/// `target`, approached from wherever `id` was last, easing out (about a sixth of a second).
fn ease_to(ctx: &egui::Context, id: egui::Id, target: f32) -> f32 {
    let now = ctx.data(|d| d.get_temp::<f32>(id)).unwrap_or(target);
    let dt = ctx.input(|i| i.stable_dt).min(0.1);
    let next = if (now - target).abs() < 0.5 { target } else { now + (target - now) * (1.0 - (-dt * 18.0).exp()) };
    ctx.data_mut(|d| d.insert_temp(id, next));
    if next != target {
        ctx.request_repaint();
    }
    next
}

/// A text button sized for a finger in a bar (`button:<id>`).
fn bar_text(ui: &mut egui::Ui, id: &str, label: &str, t: &Tokens) -> egui::Response {
    let label = crate::i18n::tr(label);
    let galley = ui.painter().layout_no_wrap(label.to_string(), t.font(17.0), t.accent);
    let (r, resp) = ui.allocate_exact_size(vec2(galley.size().x + 20.0, 44.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    crate::widgets::register(ui.ctx(), format!("button:{id}"), r);
    let color = if resp.is_pointer_button_down_on() { t.accent.gamma_multiply(0.6) } else { t.accent };
    ui.painter().galley(r.center() - galley.size() / 2.0, galley, color);
    resp
}

/// The top bar while choosing photos: Cancel, how many are chosen, Select All / Deselect All.
fn select_bar(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens, full: egui::Rect) {
    let n = app.session.selection.ids.len();
    let title = if n == 0 { crate::i18n::tr("Select Photos").to_string() } else { crate::i18n::tr_format!("{n} Selected", n = n) };
    ui.painter().text(full.center(), Align2::CENTER_CENTER, title, t.semibold(17.0), t.text);
    ui.horizontal_centered(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        if bar_text(ui, "selectCancel", "Cancel", t).clicked() {
            let _ = app.run("view.selectMode", json!({"on": false}));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let all = n > 0 && n >= app.session.visible().len();
            if all {
                if bar_text(ui, "selectNone", "Deselect All", t).clicked() {
                    let _ = app.run("library.select", json!({"ids": []}));
                }
            } else if bar_text(ui, "selectAll", "Select All", t).clicked() {
                let _ = app.run("library.selectAll", json!({}));
            }
        });
    });
}

/// What the chosen photos can have done to them, at the bottom: rating, flag, label, album,
/// export, more (copy / paste settings) and delete; each opens its own menu.
fn action_bar(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    egui::Panel::bottom("compact_actions")
        .show_separator_line(false)
        .exact_size(TAB_H)
        .frame(egui::Frame::NONE.fill(t.canvas).stroke(Stroke::new(0.5, t.divider)))
        .show(ui, |ui| {
            let any = !app.session.selection.ids.is_empty();
            let actions = [
                ("selRate", Icon::Star, "Rating"),
                ("selFlag", Icon::FlagPick, "Flag"),
                ("selLabel", Icon::Circle, "Color Label"),
                ("selAlbum", Icon::Album, "Add to Album"),
                ("selExport", Icon::Share, "Export"),
                ("selMore", Icon::Dots, "More"),
                ("selDelete", Icon::Trash, "Delete"),
            ];
            let w = ui.max_rect().width() / actions.len() as f32;
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                for (id, icon, tip) in actions {
                    let r = icon_button(ui, id, icon, vec2(w, TAB_H - 4.0), false, any, tip);
                    if !r.clicked() {
                        continue;
                    }
                    match id {
                        "selExport" => {
                            let _ = app.run("dialog.export", json!({}));
                        }
                        "selDelete" => {
                            // (asks first when Settings say so; the photos go to Recently Deleted)
                            if let Err(e) = crate::menubar::run_item(app, "photo.delete", json!({})) {
                                app.toast(ui.ctx(), e);
                            }
                        }
                        _ => mobile::open_menu(ui.ctx(), id, r.rect),
                    }
                }
            });
        });
}

/// A finger-sized menu row (`button:<id>`); true when tapped.
fn menu_row(ui: &mut egui::Ui, id: &str, label: &str, on: bool) -> bool {
    let b = ui.add(egui::Button::selectable(on, label.to_string()).min_size(vec2(220.0, 40.0)));
    crate::widgets::register(ui.ctx(), format!("button:{id}"), b.rect);
    b.clicked()
}

fn run_on(app: &mut LightcraftApp, ui: &mut egui::Ui, id: &str, mut params: serde_json::Value, ids: &[u64]) {
    params["ids"] = json!(ids);
    if let Err(e) = app.run(id, params) {
        app.toast(ui.ctx(), e);
    }
    mobile::close_actions(ui.ctx());
}

/// The one photo's state, or none when several differ.
fn common<T: PartialEq + Copy>(app: &LightcraftApp, ids: &[u64], f: impl Fn(&lightcraft_catalog::Photo) -> T) -> Option<T> {
    let mut v = ids.iter().filter_map(|id| app.session.catalog.photo(lightcraft_catalog::PhotoId(*id))).map(|p| f(p));
    let first = v.next()?;
    v.all(|x| x == first).then_some(first)
}

/// A finger-sized cell in an icon row (`button:<id>`), highlighted when `on`.
fn row_cell(ui: &mut egui::Ui, id: &str, tip: &str, on: bool) -> (egui::Rect, egui::Response) {
    let (r, resp) = ui.allocate_exact_size(vec2(44.0, 44.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, crate::i18n::tr(tip)));
    crate::widgets::register(ui.ctx(), format!("button:{id}"), r);
    let t = Tokens::get(ui.ctx());
    if on {
        ui.painter().rect_filled(r.shrink(3.0), 6.0, t.tool_active);
    }
    (r, resp)
}

/// No rating, then one to five stars: a tap sets that many.
fn rating_items(app: &mut LightcraftApp, ui: &mut egui::Ui, ids: &[u64]) {
    let t = Tokens::get(ui.ctx());
    let cur = common(app, ids, |p| p.rating);
    let tapped = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            let mut tapped = None;
            for r in 0..=5u8 {
                let (rect, resp) = row_cell(ui, &format!("rate{r}"), if r == 0 { "No Rating" } else { "Stars" }, r == 0 && cur == Some(0));
                let icon_r = egui::Rect::from_center_size(rect.center(), vec2(22.0, 22.0));
                if r == 0 {
                    crate::icons::paint(ui.painter(), icon_r.shrink(3.0), Icon::Close, t.text_dim);
                } else {
                    let filled = cur.is_some_and(|c| c >= r);
                    crate::icons::paint(
                        ui.painter(),
                        icon_r,
                        if filled { Icon::StarFilled } else { Icon::Star },
                        if filled { t.star } else { t.text_dim },
                    );
                }
                if resp.clicked() {
                    tapped = Some(r);
                }
            }
            tapped
        })
        .inner;
    if let Some(r) = tapped {
        run_on(app, ui, "photo.rate", json!({"rating": r}), ids);
    }
}

/// Pick, reject, no flag.
fn flag_items(app: &mut LightcraftApp, ui: &mut egui::Ui, ids: &[u64]) {
    use lightcraft_catalog::Flag;
    let t = Tokens::get(ui.ctx());
    let cur = common(app, ids, |p| p.flag);
    let tapped = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            let mut tapped = None;
            for (flag, key, tip, icon, color) in [
                (Flag::Pick, "pick", "Pick", Icon::FlagPick, t.pick),
                (Flag::Reject, "reject", "Reject", Icon::FlagReject, t.reject),
                (Flag::None, "none", "Unflagged", Icon::Close, t.text_dim),
            ] {
                let (rect, resp) = row_cell(ui, &format!("flag-{key}"), tip, cur == Some(flag));
                let size = if flag == Flag::None { 14.0 } else { 22.0 };
                crate::icons::paint(ui.painter(), egui::Rect::from_center_size(rect.center(), vec2(size, size)), icon, color);
                if resp.clicked() {
                    tapped = Some(key);
                }
            }
            tapped
        })
        .inner;
    if let Some(key) = tapped {
        run_on(app, ui, "photo.flag", json!({"flag": key}), ids);
    }
}

/// The five colour labels and no label.
fn label_items(app: &mut LightcraftApp, ui: &mut egui::Ui, ids: &[u64]) {
    let t = Tokens::get(ui.ctx());
    let cur = common(app, ids, |p| p.label);
    let tapped = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            let mut tapped = None;
            for l in lightcraft_catalog::ColorLabel::ALL {
                let key = format!("{l:?}").to_lowercase();
                let name = app.session.catalog.label_name(l).to_string();
                let (rect, resp) = row_cell(ui, &format!("label-{key}"), &name, false);
                let on = cur == Some(Some(l));
                let ring = if on { Stroke::new(2.5, egui::Color32::WHITE) } else { Stroke::NONE };
                ui.painter().circle(rect.center(), 10.0, super::grid::label_color(l), ring);
                if resp.on_hover_text(name).clicked() {
                    tapped = Some(key);
                }
            }
            let (rect, resp) = row_cell(ui, "label-none", "No Label", cur == Some(None));
            let c = rect.center();
            ui.painter().circle_stroke(c, 9.0, Stroke::new(1.5, t.text_dim));
            ui.painter().line_segment([c + vec2(-6.0, 6.0), c + vec2(6.0, -6.0)], Stroke::new(1.5, t.text_dim));
            if resp.clicked() {
                tapped = Some("none".into());
            }
            tapped
        })
        .inner;
    if let Some(key) = tapped {
        run_on(app, ui, "photo.label", json!({"label": key}), ids);
    }
}

/// The rating, flag and label of photos in one menu (the loupe's star button).
fn culling_menu(app: &mut LightcraftApp, ui: &mut egui::Ui, ids: &[u64]) {
    rating_items(app, ui, ids);
    ui.separator();
    flag_items(app, ui, ids);
    ui.separator();
    label_items(app, ui, ids);
}

fn album_items(app: &mut LightcraftApp, ui: &mut egui::Ui, ids: &[u64]) {
    let mut albums: Vec<(u64, String)> =
        app.session.catalog.albums().filter(|a| !a.folder && !a.is_smart()).map(|a| (a.id.0, a.name.clone())).collect();
    albums.sort_by_key(|(_, n)| n.to_lowercase());
    if albums.is_empty() {
        ui.label(crate::i18n::tr("No albums yet (My Photos ▸ Albums ▸ +)"));
    }
    egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
        for (id, name) in albums {
            if menu_row(ui, &format!("album-{id}"), &name, false) {
                run_on(app, ui, "album.addPhotos", json!({"id": id}), ids);
            }
        }
    });
}

fn more_items(app: &mut LightcraftApp, ui: &mut egui::Ui, ids: &[u64]) {
    // copying needs one photo to copy from
    if let [one] = ids
        && menu_row(ui, "copySettings", crate::i18n::tr("Copy Edit Settings"), false)
    {
        let _ = app.run("library.select", json!({"ids": [one]}));
        if let Err(e) = app.run("develop.copy", json!({})) {
            app.toast(ui.ctx(), e);
        } else {
            app.toast(ui.ctx(), crate::i18n::tr("Edit settings copied"));
        }
        mobile::close_actions(ui.ctx());
    }
    if app.session.clipboard.is_some() && menu_row(ui, "pasteSettings", crate::i18n::tr("Paste Edit Settings"), false) {
        run_on(app, ui, "develop.paste", json!({}), ids);
    }
    if app.session.clipboard.is_some() && menu_row(ui, "pasteSelected", crate::i18n::tr("Paste Selected Settings…"), false) {
        let _ = app.run("dialog.pasteSettings", json!({}));
        mobile::close_actions(ui.ctx());
    }
    if menu_row(ui, "selectEdit", crate::i18n::tr("Edit"), false)
        && let Some(first) = ids.first()
    {
        let _ = app.run("library.select", json!({"ids": [first]}));
        let _ = app.run("view.selectMode", json!({"on": false}));
        let _ = app.run("view.detail", json!({}));
        mobile::close_actions(ui.ctx());
    }
}
