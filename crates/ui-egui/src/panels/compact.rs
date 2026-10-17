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
use crate::glass;
use crate::haptics::{self, Haptic};
use crate::icons::Icon;
use crate::state::{RightPanel, ViewMode};
use crate::theme::Tokens;

/// Height of the top bar: Apple's minimum touch target is 44 pt.
const BAR_H: f32 = 44.0;
/// Height of the tool bar and of the select action bar: a glass capsule 52 pt tall (iOS 26's tab bar)
/// with a little room over and under it ([`FLOAT_GAP`]).
const TAB_H: f32 = 60.0;
/// The room under a bar's capsule, at the bottom of the safe area (the bar's colour carries on
/// through it and the home indicator's strip below).
const FLOAT_GAP: f32 = 4.0;
/// Height of the Edit tool's group row (icon over label, on a pill with room above and below).
const GROUP_H: f32 = 60.0;
/// A group's width in that row.
const GROUP_W: f32 = 64.0;
/// From this width (iPad) the tool sheet is a panel on the right and My Photos a column on the left,
/// instead of a bottom sheet and a page of their own.
pub const WIDE_PT: f32 = 600.0;
/// The side panels' width on a wide compact window.
const SIDE_W: f32 = 340.0;
/// A wide compact window shorter than this is a phone held sideways: the tools are a rail down its
/// left edge and the Edit groups sit at the top of the panel on the right, so that the photo keeps
/// the height.
const LANDSCAPE_BELOW_PT: f32 = 520.0;
/// The width of that rail of tools.
const RAIL_W: f32 = 60.0;
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
const GROUPS: [(&str, Icon, &str); 8] = [
    ("profile", Icon::ProfileGrid, "Profile"),
    ("light", Icon::Sun, "Light"),
    ("color", Icon::Drop, "Color"),
    ("effects", Icon::Vignette, "Effects"),
    ("detail", Icon::Detail, "Detail"),
    ("optics", Icon::Lens, "Optics"),
    ("lensBlur", Icon::Aperture, "Lens Blur"),
    ("calibration", Icon::Target, "Calibration"),
];

pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    // (black behind the screen when a page pushes it back: `mobile::stack_behind`)
    let backdrop = ui.painter().add(egui::Shape::Noop);
    let t = Tokens::get(&ctx);
    let detail = app.ui.view == ViewMode::Detail;
    let wide = ctx.content_rect().width() >= WIDE_PT;
    let landscape = wide && ctx.content_rect().height() < LANDSCAPE_BELOW_PT;
    if app.ui.select_mode && !matches!(app.ui.view, ViewMode::PhotoGrid | ViewMode::SquareGrid) {
        app.ui.select_mode = false;
    }
    // choosing photos for an album ends with choosing (Add or Cancel): back to the album
    if let Some(album) = app.ui.adding_to.filter(|_| !app.ui.select_mode) {
        app.ui.adding_to = None;
        let _ = app.run("library.source", json!({"kind": "album", "id": album}));
    }
    // the albums list: a column on a tablet, a page sliding over the grid on a phone
    let albums_page = app.ui.left_panel && !wide;
    // review mode (bars away, swipes rate) belongs to a photo with no tool in hand
    let tool = !app.ui.tool.is_empty() || matches!(app.ui.right, RightPanel::Crop | RightPanel::Masking | RightPanel::Remove | RightPanel::RedEye);
    app.ui.review &= detail && !tool;
    let bars = !app.ui.review;
    // Crop, Remove and Masking have a screen of their own in place of the bars and the sheet
    let (crop, remove, masking) = (super::crop_mobile::open(app), super::remove_mobile::open(app), super::mask_mobile::open(app));
    track_screen(app, crop || remove || masking);
    if crop {
        super::crop_mobile::show(app, ui, &t, landscape);
    } else if remove {
        super::remove_mobile::show(app, ui, &t, landscape);
    } else if masking {
        super::mask_mobile::show(app, ui, &t, landscape);
    } else {
        top_bar(app, ui, &t, bars);
        // the bars below are called every frame, shown or not, so that they slide in and out
        action_bar(app, ui, &t, app.ui.select_mode);
        let edit = !app.ui.presets && matches!(app.ui.right, RightPanel::Edit);
        tool_bar(app, ui, &t, landscape, wide, detail && bars);
        // (held sideways, the groups are at the top of the side panel: `sheet`)
        group_bar(app, ui, &t, wide, detail && bars && edit && !landscape);
        sheet(app, ui, &t, wide, landscape, detail && bars && (app.ui.right != RightPanel::None || app.ui.presets));
    }
    let bg = if detail { t.canvas } else { t.grid_bg };
    if wide {
        // (sliding in and out at the left edge)
        let mut open = app.ui.left_panel;
        egui::Panel::left("compact_sources")
            .resizable(false)
            .drag_to_open(false)
            .exact_size(SIDE_W)
            .frame(egui::Frame::NONE.fill(t.chrome).stroke(egui::Stroke::new(1.0, t.divider)))
            .show_collapsible(ui, &mut open, |ui| super::left::body(app, ui));
    }
    content(app, ui, bg);
    if masking {
        super::mask_mobile::floating(app, &ctx);
    }
    albums(app, &ctx, albums_page);
    super::filter_sheet::show(app, &ctx);
    super::people_mobile::naming_page(app, &ctx);
    if !albums_page && !app.ui.select_mode {
        add_button(app, &ctx, &t);
    }
    span_bar(app, &ctx, &t, !albums_page && !app.ui.select_mode);
    menus(app, &ctx);
    overlays(app, &ctx);
    mobile::all_commands_page(app, &ctx);
    mobile::edit_menu(app, &ctx);
    mobile::stack_behind(&ctx, ui.layer_id(), backdrop);
}

/// Called every frame with whether a tool's screen (Crop, Remove, Masking) is open: remembers where
/// the undo stack stood when it opened (and starts Crop on its Aspect tab).
fn track_screen(app: &mut LightcraftApp, open: bool) {
    match (open, app.ui.screen_undo_base) {
        (true, None) => {
            app.ui.screen_undo_base = Some(app.session.undo.len());
            app.ui.crop_geometry = false;
        }
        (false, Some(_)) => app.ui.screen_undo_base = None,
        _ => {}
    }
}

/// A tool screen's ✕: undo every step taken since it opened, then back to Edit.
pub(super) fn cancel_screen(app: &mut LightcraftApp) {
    let _ = app.session.end_interaction();
    let base = app.ui.screen_undo_base.unwrap_or(app.session.undo.len());
    while app.session.undo.len() > base && app.run("edit.undo", json!({})).is_ok() {}
    app.ui.tool.clear();
    app.gesture = None;
    app.ui.right = RightPanel::Edit;
}

/// The albums list (My Photos) as a page with a ✕ at its top right, as Lightroom's mobile app has
/// it; choosing an album, the ✕ or a pull down closes it. Its rows come in one after another
/// (`left::entrance`), counted from when it opened.
fn albums(app: &mut LightcraftApp, ctx: &egui::Context, open: bool) {
    let src = app.session.source;
    let was = egui::Id::new("compact-albums-open");
    if open && !ctx.data(|d| d.get_temp::<bool>(was)).unwrap_or(false) {
        let now = ctx.input(|i| i.time);
        ctx.data_mut(|d| d.insert_temp(egui::Id::new(super::left::OPENED_AT), now));
    }
    ctx.data_mut(|d| d.insert_temp(was, open));
    let bar = mobile::PageBar {
        title: "Albums",
        left: mobile::Item::None,
        right: mobile::Item::Text("Close", true),
        left_id: "sheetOk",
        right_id: "sheetCancel",
    };
    let bar = mobile::page_with(ctx, "albums", bar, open, |ui| {
        // (edge to edge, as Lightroom's list is: not inside the page's margins)
        let r = ui.max_rect().expand2(vec2(16.0, 0.0));
        let mut list = ui.new_child(egui::UiBuilder::new().max_rect(r));
        list.set_clip_rect(ui.clip_rect().expand2(vec2(16.0, 0.0)));
        super::left::body(app, &mut list);
    });
    if bar.cancel || bar.ok || (open && app.session.source != src) {
        app.ui.left_panel = false;
    }
}

/// The album the grid shows, when photos can be put in and taken out of it by hand (not a smart
/// album's, nor a folder's).
fn plain_album(app: &LightcraftApp) -> Option<u64> {
    match app.session.source {
        lightcraft_engine::LibrarySource::Album(id) => app.session.catalog.album(id).filter(|a| !a.is_smart() && !a.folder).map(|a| a.id.0),
        _ => None,
    }
}

/// The album's + ▸ From LightCraft: All Photos to choose from, with Add in place of the action bar
/// (`adding_bar`); choosing ends back in the album (`show`).
fn start_adding(app: &mut LightcraftApp, album: u64) {
    app.ui.adding_to = Some(album);
    let _ = app.run("library.source", json!({"kind": "all"}));
    let _ = app.run("view.selectMode", json!({"on": true}));
}

/// Delete tapped (the select bar's, or a photo's "…" ▸ Delete): in an album, a choice between taking
/// the photos out of it and deleting them (`delete_rows`); deleting asks first either way.
fn delete_tapped(app: &mut LightcraftApp, ctx: &egui::Context) {
    haptics::tap(ctx, Haptic::Warning);
    if plain_album(app).is_some() {
        mobile::open_actions(ctx, "deleteChoice");
    } else {
        confirm_delete(app);
    }
}

/// Ask before the photos go to Recently Deleted (the alert's Delete runs `photo.delete`).
fn confirm_delete(app: &mut LightcraftApp) {
    let count = app.session.selected().len();
    if count > 0 {
        app.ui.dialog = Some(crate::state::Dialog::ConfirmDelete { count });
    }
}

/// The choice Delete offers in an album: Remove from Album, or Delete (from the library).
fn delete_rows(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let ids: Vec<u64> = app.session.selected().iter().map(|p| p.0).collect();
    if let Some(album) = plain_album(app)
        && mobile::row(ui, "removeFromAlbum", Some(Icon::Album), crate::i18n::tr("Remove from Album"), !ids.is_empty())
    {
        match app.run("album.removePhotos", json!({"id": album, "ids": ids})) {
            // (they have left the grid: nothing chosen is out of sight)
            Ok(_) if app.ui.select_mode => {
                let _ = app.run("library.select", json!({"ids": []}));
            }
            Ok(_) => {}
            Err(e) => app.toast(ui.ctx(), e),
        }
    }
    if mobile::row_danger(ui, "deleteFromLibrary", Icon::Trash, crate::i18n::tr("Delete")) {
        confirm_delete(app);
    }
}

/// Diameter of the add-photos button.
const ADD_D: f32 = 56.0;

/// The phone's add-photos button: a round + of tinted glass over the grid's bottom right corner, offering the
/// host's pickers (Photos, Files, a folder) in a menu. Only in hosts that have them (iOS).
fn add_button(app: &mut LightcraftApp, ctx: &egui::Context, t: &Tokens) {
    if (app.services.host_pick.is_none() && plain_album(app).is_none()) || !matches!(app.ui.view, ViewMode::PhotoGrid | ViewMode::SquareGrid) {
        return;
    }
    let screen = ctx.content_rect();
    let at = pos2(screen.right() - 20.0 - ADD_D, screen.bottom() - 20.0 - ADD_D);
    egui::Area::new(egui::Id::new("compact_add")).order(egui::Order::Foreground).fixed_pos(at).show(ctx, |ui| {
        let (r, resp) = ui.allocate_exact_size(vec2(ADD_D, ADD_D), Sense::click());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, crate::i18n::tr("Add Photos")));
        crate::widgets::register(ui.ctx(), "button:addPhotos", r);
        let p = ui.painter();
        // prominent (accent-tinted) glass, swelling under the finger as iOS 26's does
        let k = glass::press(ui.ctx(), &resp);
        let d = ADD_D * (1.0 + 0.08 * k);
        glass::paint(p, egui::Rect::from_center_size(r.center(), vec2(d, d)), d / 2.0, Some(t.accent), k);
        let (c, arm) = (r.center(), ADD_D * 0.2);
        let stroke = Stroke::new(2.5, egui::Color32::WHITE);
        p.line_segment([c - vec2(arm, 0.0), c + vec2(arm, 0.0)], stroke);
        p.line_segment([c - vec2(0.0, arm), c + vec2(0.0, arm)], stroke);
        if resp.clicked() {
            mobile::open_menu(ui.ctx(), "add", r);
        }
    });
}

/// A segment of the Years / Months / All capsule.
const SPAN_W: f32 = 72.0;
const SPAN_H: f32 = 44.0;

/// Years, Months and All under the grid, as iOS's Photos has them: a glass capsule at the bottom
/// with a pill that springs to the span chosen (`view.gridSpan`; widgets `button:span-<id>`). It
/// slides away while photos are chosen or a page covers the grid.
fn span_bar(app: &mut LightcraftApp, ctx: &egui::Context, t: &Tokens, shown: bool) {
    let grid = matches!(app.ui.view, ViewMode::PhotoGrid | ViewMode::SquareGrid);
    let shown = shown && grid && !app.session.visible().is_empty();
    let k = ctx.animate_bool_with_time(egui::Id::new("span-bar-shown"), shown, 0.25);
    if k <= 0.0 {
        return;
    }
    let screen = ctx.content_rect();
    let w = SPAN_W * 3.0;
    // (level with the + button, in the middle unless that would run into it)
    let mut x = screen.center().x;
    if app.services.host_pick.is_some() {
        x = x.min(screen.right() - 20.0 - ADD_D - 10.0 - w / 2.0).max(screen.left() + 8.0 + w / 2.0);
    }
    let centre = pos2(x, screen.bottom() - 20.0 - ADD_D / 2.0 + (1.0 - k) * 90.0);
    let capsule = egui::Rect::from_center_size(centre, vec2(w, SPAN_H));
    egui::Area::new(egui::Id::new("compact_span")).order(egui::Order::Foreground).fixed_pos(capsule.min).show(ctx, |ui| {
        ui.multiply_opacity(k);
        let (r, _) = ui.allocate_exact_size(capsule.size(), Sense::hover());
        let p = ui.painter();
        // (the thicker glass of menus: the words must read over any photo)
        glass::paint(p, r, SPAN_H / 2.0, Some(glass::menu_fill(ui.ctx())), 0.0);
        let pill = p.add(egui::Shape::Noop);
        let mut chosen = None;
        for (i, span) in crate::state::GridSpan::ALL.into_iter().enumerate() {
            let seg = egui::Rect::from_min_size(r.min + vec2(i as f32 * SPAN_W, 0.0), vec2(SPAN_W, SPAN_H));
            let resp = ui.interact(seg, egui::Id::new(("span", span.id())), if shown { Sense::click() } else { Sense::hover() });
            let on = app.ui.grid_span == span;
            resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, crate::i18n::tr(span.label())));
            crate::widgets::register(ui.ctx(), format!("button:span-{}", span.id()), seg);
            if on {
                chosen = Some(seg);
            }
            let ink = if on { t.text } else { t.text_dim };
            ui.painter().text(seg.center(), Align2::CENTER_CENTER, crate::i18n::tr(span.label()), t.semibold(15.0), ink);
            if resp.clicked() && !on {
                haptics::tap(ui.ctx(), Haptic::Selection);
                let _ = app.run("view.gridSpan", json!({"span": span.id()}));
            }
        }
        ui.painter().set(pill, selection_pill(ui.ctx(), egui::Id::new("span-pill"), chosen, false, vec2(8.0, 8.0)));
    });
}

/// Every menu and action sheet of the compact layout (each draws only while it's open).
fn menus(app: &mut LightcraftApp, ctx: &egui::Context) {
    mobile::actions(ctx, "add", Some("Add Photos From"), |ui| {
        // in an album, the library's own photos too
        let album = plain_album(app);
        if let Some(album) = album
            && mobile::row(ui, "addFromLibrary", Some(Icon::Album), crate::i18n::tr("LightCraft"), true)
        {
            start_adding(app, album);
        }
        if app.services.host_pick.is_none() {
            return;
        }
        if album.is_some() {
            mobile::row_gap(ui);
        }
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
        // a camera or card reader plugged into the iPhone or iPad (ImageCaptureCore)
        if app.services.cameras.is_some() && mobile::row(ui, "connectedCamera", Some(Icon::Camera), crate::i18n::tr("Connected Camera/Card"), true) {
            connected_camera(app, ui.ctx());
        }
    });
    mobile::actions(ctx, "cameras", Some("Connected Camera/Card"), |ui| {
        for c in crate::camera_import::cameras(app) {
            if mobile::row(ui, &format!("camera:{}", c.id), Some(Icon::Camera), &c.name, true)
                && let Err(e) = app.run("file.addFromDevice", json!({"camera": c.id}))
            {
                app.toast(ui.ctx(), e);
            }
        }
    });
    mobile::actions(ctx, "newAlbum", Some("Create New"), |ui| super::left::new_album_rows(app, ui));
    mobile::actions(ctx, "deleteChoice", None, |ui| delete_rows(app, ui));
    super::crop_mobile::menus(app, ctx);
    super::remove_mobile::menus(app, ctx);
    super::mask_mobile::menus(app, ctx);
    mobile::actions(ctx, "more", None, |ui| grid_more(app, ui));
    mobile::actions(ctx, "sort", Some("Sort"), |ui| sort_items(app, ui));
    mobile::actions(ctx, "photoMore", None, |ui| photo_more(app, ui));
    mobile::actions(ctx, "undoMenu", None, |ui| undo_menu(app, ui));
    let ids: Vec<u64> = app.session.selection.ids.iter().map(|id| id.0).collect();
    mobile::actions(ctx, "selRate", Some("Rating"), |ui| rating_items(app, ui, &ids));
    mobile::actions(ctx, "selFlag", Some("Flag"), |ui| flag_items(app, ui, &ids));
    mobile::actions(ctx, "selLabel", Some("Color Label"), |ui| label_items(app, ui, &ids));
    mobile::actions(ctx, "selAlbum", Some("Add to Album"), |ui| album_items(app, ui, &ids));
    mobile::actions(ctx, "selMore", None, |ui| more_items(app, ui, &ids));
}

/// The add button's Connected Camera/Card: the import of the one connected, a choice between
/// several, or how to connect one.
fn connected_camera(app: &mut LightcraftApp, ctx: &egui::Context) {
    let cameras = crate::camera_import::cameras(app);
    match cameras.as_slice() {
        [] => app.toast(ctx, crate::i18n::tr("No camera or card connected: plug in a camera or a card reader (USB-C or Lightning), then try again")),
        [one] => {
            if let Err(e) = app.run("file.addFromDevice", json!({"camera": one.id})) {
                app.toast(ctx, e);
            }
        }
        _ => mobile::open_actions(ctx, "cameras"),
    }
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
    // hidden photos with the rest (asks for the password while Hidden is locked)
    let showing = app.session.hidden.showing();
    if app.session.source != lightcraft_engine::LibrarySource::Hidden
        && mobile::row_checked(ui, "showHidden", Some(Icon::EyeOff), crate::i18n::tr("Show Hidden"), true, Some(showing))
    {
        let r = if showing { app.run("library.showHidden", json!({"on": false})) } else { crate::panels::dialogs::open_hidden(app, true) };
        if let Err(e) = r {
            app.toast(&ctx, e);
        }
    }
    let square = app.ui.view == ViewMode::SquareGrid;
    if mobile::row_checked(ui, "squareGrid", Some(Icon::GridSquare), crate::i18n::tr("Square Thumbnails"), true, Some(square)) {
        run_cmd(app, &ctx, if square { "view.photoGrid" } else { "view.squareGrid" }, json!({}));
    }
    if mobile::row(ui, "map", Some(Icon::Pin), crate::i18n::tr("Map"), true) {
        run_cmd(app, &ctx, "view.map", json!({}));
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

/// The photo's undo menu, as Lightroom's mobile app has it: undo and redo side by side (the menu
/// stays open, for stepping back and forth), then Reset (every edit, one undo step), Versions and
/// History. Widgets `button:undoStep`, `button:redoStep`, `button:undoReset`, `button:undoVersions`,
/// `button:undoHistory`.
fn undo_menu(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let t = Tokens::get(&ctx);
    let w = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(w, 52.0), Sense::hover());
    let steps = [
        ("undoStep", Icon::Undo, "Undo", "edit.undo", lightcraft_engine::cmd::can_undo(&app.session).is_ok()),
        ("redoStep", Icon::Redo, "Redo", "edit.redo", lightcraft_engine::cmd::can_redo(&app.session).is_ok()),
    ];
    for (i, (id, icon, tip, cmd, enabled)) in steps.into_iter().enumerate() {
        let cell = egui::Rect::from_min_size(row.min + vec2(i as f32 * w / 2.0, 0.0), vec2(w / 2.0, row.height()));
        let resp = ui.interact(cell, egui::Id::new(("undo-menu", id)), if enabled { Sense::click() } else { Sense::hover() });
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, crate::i18n::tr(tip)));
        crate::widgets::register(&ctx, format!("button:{id}"), cell);
        if enabled && resp.is_pointer_button_down_on() {
            let spot = cell.shrink2(vec2(10.0, 4.0));
            ui.painter().rect_filled(
                spot,
                12.0,
                if Tokens::is_dark(&ctx) { egui::Color32::from_white_alpha(28) } else { egui::Color32::from_black_alpha(16) },
            );
        }
        crate::icons::paint(
            ui.painter(),
            egui::Rect::from_center_size(cell.center(), vec2(24.0, 24.0)),
            icon,
            if enabled { t.text } else { t.text_disabled },
        );
        // (the menu stays: a step back can be followed by another, or taken again)
        if resp.clicked() {
            run_cmd(app, &ctx, cmd, json!({}));
            haptics::tap(&ctx, Haptic::Light);
        }
    }
    mobile::row_gap(ui);
    let has_photo = app.session.active().is_some();
    if mobile::row(ui, "undoReset", Some(Icon::Reset), crate::i18n::tr("Reset"), has_photo) {
        run_cmd(app, &ctx, "develop.reset", json!({}));
        haptics::tap(&ctx, Haptic::Light);
    }
    for (id, cmd, icon, label) in
        [("undoVersions", "panel.versions", Icon::Versions, "Versions"), ("undoHistory", "panel.activity", Icon::Activity, "History")]
    {
        if mobile::row(ui, id, Some(icon), crate::i18n::tr(label), has_photo) {
            app.ui.presets = false;
            run_cmd(app, &ctx, cmd, json!({}));
        }
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
    if lightcraft_engine::enhance::Enhancer::AVAILABLE
        && mobile::row(ui, "superRes", None, crate::i18n::tr("Super Resolution…"), !app.superres.busy())
    {
        run_cmd(app, &ctx, "dialog.superRes", json!({}));
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
    // the people in it: their faces framed and named on the photo, and a face to mark by hand
    if mobile::row_checked(ui, "faceBoxes", Some(Icon::FaceBox), crate::i18n::tr("Faces"), true, Some(app.ui.face_boxes)) {
        run_cmd(app, &ctx, "view.faceBoxes", json!({}));
    }
    if mobile::row(ui, "tagFace", Some(Icon::Person), crate::i18n::tr("Tag a Face"), true) {
        run_cmd(app, &ctx, "view.tagFace", json!({"on": true}));
    }
    mobile::row_gap(ui);
    let ba = app.ui.before_after != crate::state::BeforeAfter::Off;
    if mobile::row_checked(ui, "beforeAfter", Some(Icon::BeforeAfter), crate::i18n::tr("Before / After"), true, Some(ba)) {
        run_cmd(app, &ctx, "view.beforeAfter", json!({}));
    }
    mobile::row_gap(ui);
    let (hide, label) = if crate::menubar::selection_hidden(app) { ("photo.unhide", "Unhide Photo") } else { ("photo.hide", "Hide Photo") };
    if mobile::row(ui, "hidePhoto", Some(Icon::EyeOff), crate::i18n::tr(label), true) {
        run_cmd(app, &ctx, hide, json!({}));
    }
    if mobile::row(ui, "deletePhoto", Some(Icon::Trash), crate::i18n::tr("Delete Photo"), true) {
        delete_tapped(app, &ctx);
    }
    if mobile::row(ui, "allCommands", Some(Icon::Search), crate::i18n::tr("All Commands…"), true) {
        app.ui.all_commands = true;
    }
}

fn content(app: &mut LightcraftApp, ui: &mut egui::Ui, bg: egui::Color32) {
    // a photo opened grows out of its thumbnail, one put away flies back into it
    super::detail::view_changed(app, ui.ctx());
    // the grid and the photo fade into each other (between the two grids, the tiles stay put)
    let kind = match app.ui.view {
        ViewMode::PhotoGrid | ViewMode::SquareGrid => 0,
        ViewMode::Detail => 1,
        _ => 2,
    };
    let mut k = mobile::enter(ui.ctx(), egui::Id::new("compact-content-in"), kind, 0.22);
    // a photo pulled down shows the grid behind it, and one let go flies back into it: the grid was
    // showing already, so it doesn't fade in
    if super::detail::homing(ui.ctx()).is_some() {
        k = 1.0;
    }
    let pull = if app.ui.view == ViewMode::Detail { super::detail::pulled(ui.ctx()) } else { 0.0 };
    // a photo opened grows over the grid, which stays behind it until the photo's background has
    // come in (it doesn't fade in: it was there, under the finger)
    let grown = if app.ui.view == ViewMode::Detail { super::detail::growing(ui.ctx()).map(|(_, g)| g) } else { None };
    if grown.is_some() {
        k = 1.0;
    }
    let behind_grid = pull > 0.0 || grown.is_some();
    let fill = if behind_grid { egui::Color32::TRANSPARENT } else { bg };
    egui::CentralPanel::default().frame(egui::Frame::NONE.fill(fill)).show(ui, |ui| {
        ui.multiply_opacity(k);
        // the grid runs on under the home indicator, as iOS's do (its last row scrolls clear of it)
        let grid = matches!(app.ui.view, ViewMode::PhotoGrid | ViewMode::SquareGrid);
        let mut to_bottom = if grid { to_screen_bottom(ui, fill) } else { None };
        let ui = match to_bottom.as_mut() {
            Some(below) => below,
            None => ui,
        };
        if behind_grid {
            let behind = ui.max_rect();
            let mut g = ui.new_child(egui::UiBuilder::new().max_rect(behind).id_salt("pulled-grid"));
            let view = app.ui.view;
            app.ui.view = app.ui.grid_mode;
            super::grid::show(app, &mut g);
            app.ui.view = view;
            // the photo's own background thins out as it comes down, and comes in as it grows
            let cover = match grown {
                Some(g) => egui::emath::easing::cubic_out(g),
                None => (1.0 - 1.8 * pull).clamp(0.0, 1.0),
            };
            ui.painter().rect_filled(behind, 0.0, bg.gamma_multiply(cover));
        }
        match app.ui.view {
            ViewMode::Detail => super::detail::show(app, ui),
            ViewMode::Compare => super::compare::show_compare(app, ui),
            ViewMode::Survey => super::compare::show_survey(app, ui),
            ViewMode::Reference => super::compare::show_reference(app, ui),
            ViewMode::People => super::people::show(app, ui),
            ViewMode::Map => super::map::show(app, ui),
            ViewMode::PhotoGrid | ViewMode::SquareGrid => super::grid::show(app, ui),
        }
    });
}

/// A child of the content's `ui` reaching down to the bottom of the screen, behind the home
/// indicator (outside the safe area the host lays the app out in), `fill` painted there. None when
/// a bar sits under the content, whose colour carries on there instead (`fill_below`).
fn to_screen_bottom(ui: &mut egui::Ui, fill: egui::Color32) -> Option<egui::Ui> {
    let (screen, safe, r) = (ui.ctx().viewport_rect(), ui.ctx().content_rect(), ui.max_rect());
    if r.bottom() + 0.5 < safe.bottom() || screen.bottom() <= r.bottom() + 0.5 {
        return None;
    }
    let full = egui::Rect::from_min_max(r.min, pos2(r.right(), screen.bottom()));
    let mut below = ui.new_child(egui::UiBuilder::new().max_rect(full).layout(*ui.layout()));
    below.set_clip_rect(full);
    below.painter().rect_filled(egui::Rect::from_min_max(r.left_bottom(), full.max), 0.0, fill);
    Some(below)
}

fn overlays(app: &mut LightcraftApp, ctx: &egui::Context) {
    let ctx = ctx.clone();
    super::second::show(app, &ctx);
    super::notices::show(app, &ctx);
    super::sync_status::show(app, &ctx);
    super::dialogs::show(app, &ctx);
    super::library_problem::show(app, &ctx);
    crate::import::progress(app, &ctx);
    crate::import::scan_progress(app, &ctx);
    crate::export_task::poll(app, &ctx);
    // an album carried in the albums list: its name over the finger, and the end of a drag let go anywhere
    super::left::album_drag_feedback(app, &ctx);
    super::toast(app, &ctx);
}

/// An icon button in a bar (`icon:<id>`): a round glass button, as iOS 26's bars have.
pub(super) fn bar_icon(ui: &mut egui::Ui, id: &str, icon: Icon, tip: &str, enabled: bool) -> egui::Response {
    glass::circle_button(ui, &format!("icon:{id}"), icon, tip, enabled, None)
}

fn top_bar(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens, mut shown: bool) {
    egui::Panel::top("compact_top")
        .show_separator_line(false)
        .exact_size(BAR_H)
        .frame(egui::Frame::NONE.fill(t.canvas).inner_margin(egui::Margin::symmetric(6, 0)))
        .show_collapsible(ui, &mut shown, |ui| {
            let full = ui.max_rect();
            // over the grid, a tap on the bar between its buttons slides the grid back to its top, as a
            // tap on the status bar does in iOS's lists (the buttons, added after, take their own taps)
            if matches!(app.ui.view, ViewMode::PhotoGrid | ViewMode::SquareGrid) && !app.ui.select_mode {
                let bar = ui.interact(full, egui::Id::new("compact-top-tap"), Sense::click());
                crate::widgets::register(ui.ctx(), "bar:top", full);
                if bar.clicked() {
                    let _ = app.run("view.scrollToTop", json!({}));
                }
            }
            // (the bar's contents fade in when it turns into another: grid, photo, choosing)
            ui.multiply_opacity(mobile::enter(
                ui.ctx(),
                egui::Id::new("compact-top-in"),
                (app.ui.select_mode, app.ui.view == ViewMode::Detail),
                0.18,
            ));
            // (a photo pulled down takes its bars with it)
            ui.multiply_opacity(1.0 - super::detail::pulled(ui.ctx()));
            if app.ui.select_mode {
                select_bar(app, ui, t, full);
            } else if app.ui.view == ViewMode::Detail {
                photo_bar(app, ui, t);
            } else if app.ui.view == ViewMode::Map {
                map_bar(app, ui);
            } else if app.ui.view == ViewMode::People {
                people_bar(app, ui, t);
            } else {
                grid_bar(app, ui, t);
            }
        });
}

/// Over the grid: Select and the "…" menu (the collection is the grid's own title row).
fn grid_bar(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    // (centred in the bar, as the photo's are: any lower and the bar cuts their bottoms off)
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let more = glass::circle_button(ui, "icon:more", Icon::Dots, "More", true, None);
        if more.clicked() {
            mobile::open_menu(ui.ctx(), "more", more.rect);
        }
        ui.add_space(4.0);
        let has_photos = !app.session.visible().is_empty();
        if has_photos && glass::capsule_button(ui, "button:select", "Select", None).clicked() {
            let _ = app.run("view.selectMode", json!({"on": true}));
        }
        // the cloud: where syncing stands, and its popover (Lightroom's has it beside the "…" too)
        if app.services.sync_exec.is_some() {
            ui.add_space(4.0);
            let cloud = cloud_button(app, ui, t);
            if cloud.clicked() {
                crate::panels::sync_status::open(ui.ctx(), cloud.rect);
            }
        }
    });
}

/// The grid bar's cloud: a glass circle like the "…", its cloud tinted by how syncing stands (blue
/// while it works, with an arc turning round it; amber with a red `!` on a problem), and the
/// number of things waiting under it is the popover's to say.
fn cloud_button(app: &LightcraftApp, ui: &mut egui::Ui, t: &Tokens) -> egui::Response {
    const H: f32 = glass::BUTTON_D;
    let (tip, problem, busy) = crate::sync_ui::cloud_status(app);
    let unsaved = app.session.unsaved().is_some();
    let (r, resp) = ui.allocate_exact_size(vec2(44.0, 44.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tip.clone()));
    crate::widgets::register(ui.ctx(), "icon:cloud", r);
    let k = glass::press(ui.ctx(), &resp);
    let circle = egui::Rect::from_center_size(r.center(), vec2(H, H) * (1.0 + 0.1 * k));
    let tint = if problem || unsaved {
        t.caution
    } else if busy {
        t.accent
    } else {
        t.text
    };
    let p = ui.painter();
    glass::paint(p, circle, H / 2.0, None, k);
    crate::icons::paint(p, egui::Rect::from_center_size(circle.center(), vec2(20.0, 20.0)), Icon::Cloud, tint);
    if busy {
        // an arc turning round the cloud while it works
        let t0 = ui.ctx().input(|i| i.time) as f32;
        let a0 = t0 * 3.2;
        let pts: Vec<egui::Pos2> = (0..=14)
            .map(|i| {
                let a = a0 + i as f32 * 0.09;
                circle.center() + vec2(a.cos(), a.sin()) * (H / 2.0 - 1.5)
            })
            .collect();
        p.add(egui::Shape::line(pts, Stroke::new(1.8, t.accent)));
        ui.ctx().request_repaint();
    }
    if problem || unsaved {
        let c = circle.right_top() + vec2(-2.0, 4.0);
        p.circle_filled(c, 6.5, t.reject);
        p.text(c, Align2::CENTER_CENTER, "!", t.semibold(9.5), egui::Color32::WHITE);
    }
    resp
}

/// Over the map: back to the grid on the left.
fn map_bar(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    ui.horizontal_centered(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.add_space(2.0);
        if bar_icon(ui, "back", Icon::ChevronLeft, "Back", true).clicked() {
            let _ = app.run("view.photoGrid", json!({}));
        }
    });
}

/// Over People: back (from a person's page to everyone, from everyone to the grid) and the
/// person's name in the middle.
fn people_bar(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let full = ui.max_rect();
    if let Some(name) = app.ui.person_page.clone() {
        let shown = super::people::fit(ui.painter(), &name, t.semibold(17.0), full.width() - 120.0);
        ui.painter().text(full.center(), Align2::CENTER_CENTER, shown, t.semibold(17.0), t.text);
    }
    ui.horizontal_centered(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.add_space(2.0);
        if bar_icon(ui, "back", Icon::ChevronLeft, "Back", true).clicked() {
            if app.ui.person_page.is_some() {
                let _ = app.run("view.people", json!({}));
            } else {
                let grid = if app.ui.grid_mode == ViewMode::SquareGrid { "view.squareGrid" } else { "view.photoGrid" };
                let _ = app.run(grid, json!({}));
            }
        }
    });
}

/// Over a photo: back on the left; undo, then share, save and "…" sharing one glass capsule on the
/// right, as iOS 26 groups a bar's buttons. Undo opens its menu (`undo_menu`), as Lightroom's mobile
/// app has it; a long press on it opens the history.
fn photo_bar(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    ui.horizontal_centered(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.add_space(2.0);
        if bar_icon(ui, "back", Icon::ChevronLeft, "Back", true).clicked() {
            let _ = app.run("view.back", json!({}));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(2.0);
            let has_photo = app.session.active().is_some();
            // (right to left: "…", save, share)
            let save = app.services.save_to_photos.is_some();
            let mut items: Vec<glass::GroupItem> = vec![("icon:photoMore", Icon::Dots, "More", has_photo)];
            if save {
                items.push(("icon:save", Icon::Download, "Save to Photos", has_photo));
            }
            items.push(("icon:share", Icon::Share, "Share", has_photo));
            let tapped = glass::group(ui, &items);
            if let Some(more) = tapped.first()
                && more.clicked()
            {
                mobile::open_menu(ui.ctx(), "photoMore", more.rect);
            }
            if save
                && tapped.get(1).is_some_and(|r| r.clicked())
                && let Err(e) = app.run("app.saveToPhotos", json!({}))
            {
                app.toast_error(ui.ctx(), e);
            }
            if tapped.last().is_some_and(|r| r.clicked()) {
                let _ = app.run("dialog.export", json!({}));
            }
            ui.add_space(6.0);
            // (a corner mark says it opens a menu: undo, redo, reset, versions)
            let undo = bar_icon(ui, "undo", Icon::Undo, "Undo, Redo and Reset", app.session.active().is_some());
            let mark = undo.rect.center() + vec2(9.0, 9.0);
            let ink = if app.session.active().is_some() { t.text } else { t.text_disabled };
            ui.painter().add(egui::Shape::convex_polygon(
                vec![mark + vec2(3.5, -3.5), mark + vec2(3.5, 3.5), mark + vec2(-3.5, 3.5)],
                ink,
                Stroke::NONE,
            ));
            if undo.clicked() {
                mobile::open_menu(ui.ctx(), "undoMenu", undo.rect);
                haptics::tap(ui.ctx(), Haptic::Light);
            }
            if undo.long_touched() || undo.clicked_by(egui::PointerButton::Secondary) {
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
    // where the buttons are (the sort and filter capsule, 92 pt wide, 12 pt in from the edge)
    let right = egui::Rect::from_min_max(pos2(hr.right() - 104.0, hr.top()), pos2(hr.right() - 12.0, hr.bottom()));
    // (a large title, as iOS's Photos heads its library; a long name is cut short with … before the ▾ and
    // the buttons, not run under them: 16 pt in from the edge, 30 pt for the ▾ and its padding, an 8 pt gap)
    let room = (right.left() - 8.0 - (hr.left() + 16.0) - 30.0).max(24.0);
    let galley = crate::widgets::one_line(ui.painter(), &title, t.semibold(28.0), t.text, room);
    let title_r = egui::Rect::from_min_size(pos2(hr.left() + 16.0, hr.center().y - 20.0), vec2(galley.size().x + 30.0, 40.0));
    let mut resp = ui.interact(title_r, egui::Id::new("compact-collection"), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, crate::i18n::tr("Albums")));
    if galley.elided {
        // (the whole name, where there is a pointer to rest on it)
        resp = resp.on_hover_text(title);
    }
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
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(right).layout(egui::Layout::right_to_left(egui::Align::Center)));
    child.spacing_mut().item_spacing.x = 0.0;
    let tapped = glass::group(&mut child, &[("icon:sort", Icon::Sort, "Sort", true), ("icon:filter", Icon::Filter, "Filter", true)]);
    if let Some(sort) = tapped.first()
        && sort.clicked()
    {
        mobile::open_menu(ui.ctx(), "sort", sort.rect);
    }
    if let Some(filter) = tapped.get(1) {
        // (a dot on the button while a filter hides photos)
        if app.session.filter != lightcraft_catalog::Filter::default() {
            ui.painter().circle_filled(filter.rect.right_top() + vec2(-9.0, 9.0), 4.0, t.accent);
        }
        if filter.clicked() {
            let _ = app.run("view.filterSheet", json!({}));
        }
    }
    // the count, where there's room for it before the buttons
    let after = title_r.right() + 4.0;
    let count = ui.painter().layout_no_wrap(count.to_string(), t.font(13.0), t.text_dim);
    if after + count.size().x + 8.0 < child.min_rect().left() {
        ui.painter().galley(pos2(after, hr.center().y + 3.0 - count.size().y / 2.0), count, t.text_dim);
    }
}

/// The tools under a photo as an iOS 26 tab bar: a floating glass capsule, the open tool's icon and
/// name tinted, on a lighter pill that springs across to the tool tapped. Tapping the open tool
/// closes its sheet. A phone held sideways has them as a rail down the left edge instead.
fn tool_bar(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens, rail: bool, wide: bool, mut shown: bool) {
    // the bottom safe area is already outside this ui (see the host); the bar sits at its edge
    let bar = if rail { egui::Panel::left("compact_rail").exact_size(RAIL_W) } else { egui::Panel::bottom("compact_tabs").exact_size(TAB_H) };
    // (what is above the bar, so that the capsule floats on it: a phone's sheet when a tool is open)
    let under = if !wide && (app.ui.right != RightPanel::None || app.ui.presets) { t.chrome } else { t.canvas };
    let bar = bar.show_separator_line(false).frame(egui::Frame::NONE.fill(if rail { t.canvas } else { under }));
    bar.show_collapsible(ui, &mut shown, |ui| {
        ui.multiply_opacity(1.0 - super::detail::pulled(ui.ctx()));
        let has_photo = app.session.active().is_some();
        let full = ui.max_rect();
        // (no wider than a phone's: on a tablet the tools sit together in the middle)
        let capsule = if rail {
            full.shrink2(vec2(4.0, 0.0))
        } else {
            fill_below(ui, full, under);
            floating_capsule(full)
        };
        // the tools share the capsule; a rail's don't stretch (52 pt apart, in the middle)
        let n = TOOLS.len() as f32;
        let (cell_w, cell_h) =
            if rail { (capsule.width(), (capsule.height() / n).min(52.0)) } else { ((capsule.width() - 8.0) / n, capsule.height()) };
        let capsule = if rail { egui::Rect::from_center_size(capsule.center(), vec2(capsule.width(), cell_h * n + 8.0)) } else { capsule };
        let start = capsule.min + vec2(4.0, 4.0);
        let glass_idx = ui.painter().add(egui::Shape::Noop);
        let pill_idx = ui.painter().add(egui::Shape::Noop);
        let mut chosen = None;
        for (i, (id, icon, panel, tip)) in TOOLS.into_iter().enumerate() {
            let presets = id == "presets";
            let on = if presets {
                app.ui.presets
            } else {
                !app.ui.presets && (app.ui.right == panel || (panel == RightPanel::Edit && app.ui.right == RightPanel::Profiles))
            };
            let at = if rail { start + vec2(-4.0, i as f32 * cell_h) } else { start + vec2(i as f32 * cell_w, -4.0) };
            let cell = egui::Rect::from_min_size(at, vec2(cell_w, cell_h));
            if on {
                chosen = Some(cell);
            }
            let resp = ui.interact(cell, egui::Id::new(("compact-tool", id)), if has_photo { Sense::click() } else { Sense::hover() });
            resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, has_photo, on, crate::i18n::tr(tip)));
            crate::widgets::register(ui.ctx(), format!("icon:{id}"), cell);
            // iOS's tab bar: the open tool's icon and name are tinted, the others the label colour
            let k = ui.ctx().animate_bool_with_time(resp.id.with("on"), on, 0.15);
            let color = if !has_photo {
                t.text_disabled
            } else if resp.is_pointer_button_down_on() {
                t.accent.gamma_multiply(0.6)
            } else {
                crate::widgets::lerp(t.text, t.accent, k)
            };
            let name = crate::i18n::tr(if id == "crop" { "Crop" } else { tip });
            let label = !rail;
            let icon_c = if label { cell.center() - vec2(0.0, 7.0) } else { cell.center() };
            crate::icons::paint(ui.painter(), egui::Rect::from_center_size(icon_c, vec2(22.0, 22.0)), icon, color);
            if label {
                ui.painter().text(cell.center() + vec2(0.0, 13.0), egui::Align2::CENTER_CENTER, name, t.semibold(10.0), color);
            }
            if resp.clicked() {
                haptics::tap(ui.ctx(), Haptic::Selection);
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
        ui.painter().set(glass_idx, glass::shape(ui.ctx(), capsule, if rail { 26.0 } else { capsule.height() / 2.0 }, None, 0.0));
        let inset = if rail { vec2(8.0, 2.0) } else { vec2(2.0, 8.0) };
        ui.painter().set(pill_idx, selection_pill(ui.ctx(), egui::Id::new("compact-tool-pill"), chosen, rail, inset));
    });
}

/// Where a bottom bar's floating capsule goes in the bar's `full` rect: 14 pt in from the sides (no
/// wider than a phone's: on a tablet it sits in the middle), 4 pt down, [`FLOAT_GAP`] above the bottom.
fn floating_capsule(full: egui::Rect) -> egui::Rect {
    let w = (full.width() - 28.0).clamp(0.0, 480.0);
    let top = full.top() + 4.0;
    let bottom = (full.bottom() - FLOAT_GAP).max(top);
    egui::Rect::from_x_y_ranges((full.center().x - w / 2.0)..=(full.center().x + w / 2.0), top..=bottom)
}

/// Paint `color` from the bottom of a bar (`full`) down to the screen's edge: the home indicator's
/// strip (outside the safe area, so nothing else draws there) carries on the bar's colour.
pub(super) fn fill_below(ui: &egui::Ui, full: egui::Rect, color: egui::Color32) {
    let screen = ui.ctx().viewport_rect();
    if screen.bottom() > full.bottom() {
        let mut p = ui.painter().clone();
        p.set_clip_rect(screen);
        p.rect_filled(egui::Rect::from_min_max(pos2(full.left(), full.bottom()), pos2(full.right(), screen.bottom())), 0.0, color);
    }
}

/// The lighter pill behind a bar's chosen item (`at` less `inset`; none: it fades away), springing
/// from the item chosen before to this one along the bar (down a rail).
fn selection_pill(ctx: &egui::Context, id: egui::Id, at: Option<egui::Rect>, vertical: bool, inset: egui::Vec2) -> egui::Shape {
    let shown = glass::spring(ctx, id.with("alpha"), if at.is_some() { 1.0 } else { 0.0 }, 0.25, 1.0);
    let last = id.with("rect");
    let at = match at {
        Some(r) => {
            ctx.data_mut(|d| d.insert_temp(last, r));
            r
        }
        None => match ctx.data(|d| d.get_temp::<egui::Rect>(last)) {
            Some(r) => r,
            None => return egui::Shape::Noop,
        },
    };
    let along = if vertical { at.center().y } else { at.center().x };
    // (one that appears starts where it is, rather than sliding in from an old place)
    if shown < 0.05 {
        ctx.data_mut(|d| d.insert_temp(id.with("pos"), (along, 0.0_f32)));
    }
    let pos = glass::spring(ctx, id.with("pos"), along, 0.42, 0.72);
    let c = if vertical { pos2(at.center().x, pos) } else { pos2(pos, at.center().y) };
    let r = egui::Rect::from_center_size(c, (at.size() - inset).max(vec2(1.0, 1.0)));
    let ink = if Tokens::is_dark(ctx) { egui::Color32::from_white_alpha(30) } else { egui::Color32::from_black_alpha(14) };
    egui::Shape::rect_filled(r, r.height().min(r.width()) / 2.0, ink.gamma_multiply(shown.clamp(0.0, 1.0)))
}

/// The Edit tool's groups in a row that scrolls sideways (icon over name), Auto first: a phone
/// shows one group's sliders at a time, as Lightroom's mobile app does.
fn group_bar(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens, wide: bool, mut shown: bool) {
    // (on a phone, on the sheet's colour: the sheet, its groups and the tool bar under them read as one)
    let fill = if wide { t.canvas } else { t.chrome };
    let bar = egui::Panel::bottom("compact_groups").show_separator_line(false).exact_size(GROUP_H).frame(egui::Frame::NONE.fill(fill));
    bar.show_collapsible(ui, &mut shown, |ui| group_strip(app, ui, t));
}

/// The row of groups itself (`group_bar` is its panel; a phone held sideways has it at the top of
/// the panel on the right).
fn group_strip(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    egui::ScrollArea::horizontal().id_salt("compact-groups").scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(
        ui,
        |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.add_space(8.0);
                // the chosen group's pill goes under the cells, and springs from one to the next
                let (pill, origin) = (ui.painter().add(egui::Shape::Noop), ui.cursor().min.to_vec2());
                let mut chosen = None;
                if group_cell(ui, "auto", Icon::Wand, "Auto", false).clicked() {
                    let _ = app.run("develop.auto", json!({}));
                    app.toast(ui.ctx(), "Auto settings applied");
                }
                let (sep, _) = ui.allocate_exact_size(vec2(9.0, 30.0), Sense::hover());
                ui.painter().vline(sep.center().x, sep.y_range(), Stroke::new(1.0, t.divider));
                for (id, icon, label) in GROUPS {
                    let cell = group_cell(ui, &format!("group-{id}"), icon, label, app.ui.edit_group == id);
                    if app.ui.edit_group == id {
                        chosen = Some(cell.rect.translate(-origin));
                    }
                    if cell.clicked() {
                        app.ui.edit_group = id.to_string();
                        if app.ui.right == RightPanel::Profiles {
                            app.ui.right = RightPanel::Edit;
                        }
                    }
                }
                ui.add_space(8.0);
                // (as wide as a group, 6 pt clear of the row's top and bottom)
                let mut shape = selection_pill(ui.ctx(), egui::Id::new("compact-group-pill"), chosen, false, vec2(0.0, 8.0));
                shape.translate(origin);
                ui.painter().set(pill, shape);
            });
        },
    );
}

/// One group in the group row (`button:<id>`): its icon over its name (the chosen one's pill is the
/// row's to draw).
fn group_cell(ui: &mut egui::Ui, id: &str, icon: Icon, label: &str, on: bool) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    let label = crate::i18n::tr(label);
    let (r, resp) = ui.allocate_exact_size(vec2(GROUP_W, GROUP_H - 4.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, label));
    crate::widgets::register(ui.ctx(), format!("button:{id}"), r);
    let color = if on || resp.is_pointer_button_down_on() { t.text } else { t.text_dim };
    // (icon and name close together, so that the pill has room round them)
    crate::icons::paint(ui.painter(), egui::Rect::from_center_size(r.center() - vec2(0.0, 8.0), vec2(19.0, 19.0)), icon, color);
    ui.painter().text(r.center() + vec2(0.0, 11.5), Align2::CENTER_CENTER, label, t.font(11.0), color);
    if resp.clicked() {
        haptics::tap(ui.ctx(), Haptic::Selection);
    }
    resp
}

/// The tool sheet: a panel on the right on a tablet, a bottom sheet on a phone, sliding in and out
/// with `shown`. The sheet has a grabber: drag it to one of three heights (small, medium, large)
/// or down to close the tool, tap it to step through them. A sheet whose content is shorter than
/// its height shrinks to fit (the Profile group is one row), so the photo gets the room.
fn sheet(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens, wide: bool, landscape: bool, mut shown: bool) {
    // the room the bars left, for the photo and the sheet
    let avail = ui.available_rect_before_wrap().height();
    let ctx = ui.ctx().clone();
    // sliding away, the sheet still shows the tool it had (the app has moved on)
    let (last, mine) = (egui::Id::new("compact-sheet-last"), (app.ui.right, app.ui.presets));
    let now = (mine.0 != RightPanel::None || mine.1).then_some(mine);
    let tool = now.or_else(|| ctx.data(|d| d.get_temp::<(RightPanel, bool)>(last))).unwrap_or(mine);
    ctx.data_mut(|d| d.insert_temp(last, tool));
    (app.ui.right, app.ui.presets) = tool;
    let frame = egui::Frame::NONE.fill(t.chrome);
    let was = shown;
    let body = |app: &mut LightcraftApp, ui: &mut egui::Ui| {
        // the tool's controls fade in on a change of tool or group
        let key = format!("{tool:?}{}", app.ui.edit_group);
        ui.multiply_opacity(mobile::enter(ui.ctx(), egui::Id::new("compact-sheet-in"), key, 0.18));
        if app.ui.presets { super::presets::body(app, ui) } else { super::right::body(app, ui) }
    };
    if wide {
        let width = if landscape { 316.0 } else { SIDE_W };
        egui::Panel::right("compact_side")
            .resizable(true)
            .drag_to_open(false)
            .default_size(width)
            .size_range(280.0..=480.0)
            .frame(frame)
            .show_collapsible(ui, &mut shown, |ui| {
                if landscape && !app.ui.presets && matches!(app.ui.right, RightPanel::Edit) {
                    ui.allocate_ui(vec2(ui.available_width(), GROUP_H), |ui| group_strip(app, ui, t));
                    ui.painter().hline(ui.max_rect().x_range(), ui.cursor().top(), Stroke::new(0.5, t.divider));
                } else {
                    ui.add_space(8.0);
                }
                body(app, ui)
            });
        // (dragged shut: the tool is put away rather than coming back the next frame)
        if was && !shown {
            app.ui.right = RightPanel::None;
            app.ui.presets = false;
        } else if now.is_none() {
            (app.ui.right, app.ui.presets) = mine;
        }
        return;
    }
    let stops = sheet_stops(avail);
    let detent = (app.ui.sheet_detent as usize).min(stops.len() - 1);
    // what the finger has made it while dragging the grabber, else the stop it was left at; never
    // taller than its content (measured by `right::body` a frame before)
    let drag_id = egui::Id::new("compact-sheet-drag");
    let dragging = ctx.data(|d| d.get_temp::<f32>(drag_id));
    let fit = ctx.data(|d| d.get_temp::<f32>(fit_id(app))).map(|c| c + GRABBER_H);
    // (content that only just overflows a stop is shown whole: a row cut in half helps nobody)
    let limit = |h: f32| fit.map_or(h, |f| if f <= h + SLIDER_ROW_H { f } else { h }).max(GRABBER_H + SLIDER_ROW_H);
    let h = match dragging {
        Some(h) => limit(h),
        None => ease_to(&ctx, egui::Id::new("compact-sheet-h"), limit(stops[detent])),
    };
    let frame = frame.corner_radius(egui::CornerRadius { nw: 22, ne: 22, sw: 0, se: 0 });
    // (what shows through the rounded corners is the photo's canvas, not what the window was cleared to)
    let behind = ui.painter().add(egui::Shape::Noop);
    let out = egui::Panel::bottom("compact_sheet").show_separator_line(false).resizable(false).exact_size(h).frame(frame).show_collapsible(
        ui,
        &mut shown,
        |ui| {
            let (g, resp) = ui.allocate_exact_size(vec2(ui.available_width(), GRABBER_H), Sense::click_and_drag());
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, crate::i18n::tr("Resize Panel")));
            crate::widgets::register(ui.ctx(), "sheet:grabber", g);
            let pill = if resp.is_pointer_button_down_on() { t.text_dim } else { t.track };
            ui.painter().rect_filled(egui::Rect::from_center_size(g.center() + vec2(0.0, 1.0), vec2(36.0, 5.0)), 2.5, pill);
            if resp.dragged() {
                let now = dragging.unwrap_or(h) - ui.input(|i| i.pointer.delta().y);
                // (not `clamp`: that panics when a tiny window leaves `avail` negative)
                let now = now.max(0.0).min(stops[2].max(avail * 0.85));
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
        },
    );
    if let Some(out) = out {
        ui.painter().set(behind, egui::Shape::rect_filled(out.response.rect, 0.0, t.canvas));
    }
    if now.is_none() {
        (app.ui.right, app.ui.presets) = mine;
    }
}

/// The sheet's heights: about two and a half slider rows, four and a half (a part of the next row
/// shows that the rest scrolls), and three fifths of the room (the photo keeps the rest).
fn sheet_stops(avail: f32) -> [f32; 3] {
    let most = (avail * 0.62).max(GRABBER_H + 6.0 * SLIDER_ROW_H);
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

/// The top bar while choosing photos: Cancel, how many are chosen, Select All / Deselect All.
fn select_bar(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens, full: egui::Rect) {
    let n = app.session.selection.ids.len();
    let adding = app.ui.adding_to.and_then(|a| app.session.catalog.album(lightcraft_catalog::AlbumId(a))).map(|a| a.name.clone());
    let title = match adding {
        Some(album) => crate::i18n::tr_format!("Add to {album}", album = album),
        None if n == 0 => crate::i18n::tr("Select Photos").to_string(),
        None => crate::i18n::tr_format!("{n} Selected", n = n),
    };
    ui.painter().text(full.center(), Align2::CENTER_CENTER, title, t.semibold(17.0), t.text);
    ui.horizontal_centered(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.add_space(6.0);
        if glass::capsule_button(ui, "button:selectCancel", "Cancel", None).clicked() {
            let _ = app.run("view.selectMode", json!({"on": false}));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(6.0);
            let all = n > 0 && n >= app.session.visible().len();
            if all {
                if glass::capsule_button(ui, "button:selectNone", "Deselect All", None).clicked() {
                    let _ = app.run("library.select", json!({"ids": []}));
                }
            } else if glass::capsule_button(ui, "button:selectAll", "Select All", None).clicked() {
                let _ = app.run("library.selectAll", json!({}));
            }
        });
    });
}

/// What the chosen photos can have done to them, at the bottom: rating, flag, label, album,
/// export, more (copy / paste settings) and delete; each opens its own menu.
fn action_bar(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens, mut shown: bool) {
    egui::Panel::bottom("compact_actions").show_separator_line(false).exact_size(TAB_H).frame(egui::Frame::NONE.fill(t.grid_bg)).show_collapsible(
        ui,
        &mut shown,
        |ui| {
            if let Some(album) = app.ui.adding_to {
                adding_bar(app, ui, t, album);
                return;
            }
            // (sliding away after Cancel, it must not delete anything)
            let any = app.ui.select_mode && !app.session.selection.ids.is_empty();
            let actions = [
                ("selRate", Icon::Star, "Rating"),
                ("selFlag", Icon::FlagPick, "Flag"),
                ("selLabel", Icon::Circle, "Color Label"),
                ("selAlbum", Icon::Album, "Add to Album"),
                ("selExport", Icon::Share, "Export"),
                ("selMore", Icon::Dots, "More"),
                ("selDelete", Icon::Trash, "Delete"),
            ];
            // a floating glass capsule, as the tool bar
            fill_below(ui, ui.max_rect(), t.grid_bg);
            let capsule = floating_capsule(ui.max_rect());
            glass::paint(ui.painter(), capsule, capsule.height() / 2.0, None, 0.0);
            let w = (capsule.width() - 8.0) / actions.len() as f32;
            let inner = capsule.shrink2(vec2(4.0, 0.0));
            ui.scope_builder(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::left_to_right(egui::Align::Center)), |ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                for (id, icon, tip) in actions {
                    let r = action_cell(ui, id, icon, w, any, tip);
                    if !r.clicked() {
                        continue;
                    }
                    match id {
                        "selExport" => {
                            let _ = app.run("dialog.export", json!({}));
                        }
                        "selDelete" => delete_tapped(app, ui.ctx()),
                        _ => mobile::open_menu(ui.ctx(), id, r.rect),
                    }
                }
            });
        },
    );
}

/// While photos are chosen for an album (`adding_to`): how many, and Add (`button:addToAlbum`),
/// which puts them in it and goes back to the album.
fn adding_bar(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens, album: u64) {
    fill_below(ui, ui.max_rect(), t.grid_bg);
    let capsule = floating_capsule(ui.max_rect());
    glass::paint(ui.painter(), capsule, capsule.height() / 2.0, None, 0.0);
    let ids = app.session.selection.ids.clone();
    let count = crate::i18n::tr_format!("{n} Selected", n = ids.len());
    ui.painter().text(capsule.left_center() + vec2(22.0, 0.0), Align2::LEFT_CENTER, count, t.font(15.0), t.text_dim);
    let inner = capsule.shrink2(vec2(6.0, 0.0));
    ui.scope_builder(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::right_to_left(egui::Align::Center)), |ui| {
        let tint = (!ids.is_empty()).then_some(t.accent);
        if glass::capsule_button(ui, "button:addToAlbum", "Add", tint).clicked() && !ids.is_empty() {
            match app.run("album.addPhotos", json!({"id": album, "ids": ids.iter().map(|p| p.0).collect::<Vec<u64>>()})) {
                Ok(r) => super::grid::toast_skipped(app, ui.ctx(), &r),
                Err(e) => app.toast(ui.ctx(), e),
            }
            let _ = app.run("view.selectMode", json!({"on": false}));
        }
    });
}

/// One action of the select bar (`icon:<id>`): its icon, lit while pressed.
fn action_cell(ui: &mut egui::Ui, id: &str, icon: Icon, w: f32, enabled: bool, tip: &str) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    let (r, resp) = ui.allocate_exact_size(vec2(w, ui.available_height()), if enabled { Sense::click() } else { Sense::hover() });
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, crate::i18n::tr(tip)));
    crate::widgets::register(ui.ctx(), format!("icon:{id}"), r);
    let k = glass::press(ui.ctx(), &resp);
    if k > 0.0 {
        let spot = egui::Rect::from_center_size(r.center(), vec2(w - 2.0, r.height() - 8.0));
        let ink = if Tokens::is_dark(ui.ctx()) { egui::Color32::from_white_alpha(30) } else { egui::Color32::from_black_alpha(14) };
        ui.painter().rect_filled(spot, spot.height() / 2.0, ink.gamma_multiply(k.min(1.0)));
    }
    let color = if enabled { t.text } else { t.text_disabled };
    crate::icons::paint(ui.painter(), egui::Rect::from_center_size(r.center(), vec2(22.0, 22.0)), icon, color);
    resp
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

/// A finger-sized cell in an icon row (`button:<id>`), on a round highlight when `on` or pressed.
fn row_cell(ui: &mut egui::Ui, id: &str, tip: &str, on: bool) -> (egui::Rect, egui::Response) {
    let (r, resp) = ui.allocate_exact_size(vec2(44.0, 44.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, crate::i18n::tr(tip)));
    crate::widgets::register(ui.ctx(), format!("button:{id}"), r);
    let t = Tokens::get(ui.ctx());
    // (a round highlight, as iOS marks the chosen one of a row of symbols)
    if on || resp.is_pointer_button_down_on() {
        ui.painter().circle_filled(r.center(), 19.0, t.tool_active);
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
                let ring = if on { Stroke::new(2.5, t.text) } else { Stroke::NONE };
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
    let hidden = common(app, ids, |p| p.hidden) == Some(true);
    let (hide, label) = if hidden { ("photo.unhide", "Unhide") } else { ("photo.hide", "Hide") };
    if menu_row(ui, "selHide", crate::i18n::tr(label), false) {
        run_on(app, ui, hide, json!({}), ids);
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
