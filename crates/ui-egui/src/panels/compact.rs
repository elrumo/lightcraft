//! The compact (phone-sized) layout: one panel at a time. A slim top bar, the grid or loupe in the
//! middle, and in the loupe a bottom tab bar of tools with the active tool's panel as a bottom
//! sheet. It reuses the desktop panels' bodies (`right::body`) and commands, so every control
//! stays a `develop` control spec / command.

use egui::{Align2, Sense, Stroke, pos2, vec2};
use serde_json::json;

use crate::LightcraftApp;
use crate::icons::Icon;
use crate::state::{RightPanel, ViewMode};
use crate::theme::Tokens;
use crate::widgets::icon_button;

/// Height of the top bar and of the tab bar: Apple's minimum touch target is 44 pt.
const BAR_H: f32 = 52.0;
/// From this width (iPad) the tool sheet is a panel on the right and My Photos a column on the left,
/// instead of a bottom sheet and a page of their own.
pub const WIDE_PT: f32 = 600.0;
/// The side panels' width on a wide compact window.
const SIDE_W: f32 = 340.0;
/// The sheet opens at this fraction of the window height.
const SHEET_FRACTION: f32 = 0.4;

pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let t = Tokens::get(&ctx);
    let detail = app.ui.view == ViewMode::Detail;
    let wide = ctx.content_rect().width() >= WIDE_PT;
    if app.ui.select_mode && !matches!(app.ui.view, ViewMode::PhotoGrid | ViewMode::SquareGrid) {
        app.ui.select_mode = false;
    }
    top_bar(app, ui, &t, wide);
    if app.ui.select_mode {
        action_bar(app, ui, &t);
    }
    if detail {
        tab_bar(app, ui, &t);
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
        content(app, ui, bg);
    } else if app.ui.left_panel {
        // "My Photos" as a page of its own; choosing a source closes it
        let src = app.session.source;
        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(t.chrome)).show(ui, |ui| super::left::body(app, ui));
        if app.session.source != src {
            app.ui.left_panel = false;
        }
    } else {
        content(app, ui, bg);
    }
    if !(app.ui.left_panel && !wide) && !app.ui.select_mode {
        add_button(app, &ctx, &t);
    }
    overlays(app, &ctx);
}

/// Diameter of the add-photos button.
const ADD_D: f32 = 56.0;

/// The phone's add-photos button: a round + over the grid's bottom right corner, offering the
/// host's pickers (Photos, Files, a folder). Only in hosts that have them (iOS).
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
        let fill = if resp.is_pointer_button_down_on() { t.accent.gamma_multiply(0.8) } else { t.accent };
        p.circle_filled(r.center(), ADD_D / 2.0, fill);
        let (c, arm) = (r.center(), ADD_D * 0.2);
        let stroke = Stroke::new(2.5, egui::Color32::WHITE);
        p.line_segment([c - vec2(arm, 0.0), c + vec2(arm, 0.0)], stroke);
        p.line_segment([c - vec2(0.0, arm), c + vec2(0.0, arm)], stroke);
        egui::Popup::menu(&resp).show(|ui| {
            for (id, label) in [
                ("file.importFromPhotos", "From Photos…"),
                ("file.importFromFiles", "From Files…"),
                ("file.importFolderFromFiles", "Folder from Files…"),
            ] {
                let b = ui.add(egui::Button::new(crate::i18n::tr(label)).min_size(vec2(220.0, 44.0)));
                crate::widgets::register(ui.ctx(), format!("button:{id}"), b.rect);
                if b.clicked() {
                    if let Err(e) = app.run(id, json!({})) {
                        app.toast(ui.ctx(), e);
                    }
                    ui.close();
                }
            }
        });
    });
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

fn top_bar(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens, wide: bool) {
    egui::Panel::top("compact_top")
        .exact_size(BAR_H)
        .frame(egui::Frame::NONE.fill(t.chrome).inner_margin(egui::Margin::symmetric(8, 0)).stroke(egui::Stroke::new(1.0, t.divider)))
        .show(ui, |ui| {
            let full = ui.max_rect();
            let grid = matches!(app.ui.view, ViewMode::PhotoGrid | ViewMode::SquareGrid);
            if app.ui.select_mode {
                select_bar(app, ui, t, full);
                return;
            }
            let title = if app.ui.left_panel && !wide {
                "My Photos"
            } else if app.ui.view == ViewMode::Detail {
                "Edit"
            } else {
                "Photos"
            };
            ui.painter().text(full.center(), Align2::CENTER_CENTER, crate::i18n::tr(title), t.semibold(17.0), t.text);
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                if icon_button(ui, "sidebar", Icon::Sidebar, vec2(44.0, 44.0), app.ui.left_panel, true, "My Photos").clicked() {
                    let _ = app.run("view.leftPanel", json!({}));
                }
                if icon_button(ui, "back", Icon::Back, vec2(44.0, 44.0), false, !grid, "Back").clicked() {
                    let _ = app.run("view.back", json!({}));
                }
                // every command stays reachable: the whole menu bar behind one button
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    crate::menubar::show_in_window(app, ui, 0.0);
                    let has_photos = !app.session.visible().is_empty();
                    if grid && !(app.ui.left_panel && !wide) && has_photos && bar_text(ui, "select", "Select", t).clicked() {
                        let _ = app.run("view.selectMode", json!({"on": true}));
                    }
                    // the loupe: rate, flag and label the photo without a right click
                    if app.ui.view == ViewMode::Detail
                        && let Some(id) = app.session.active()
                    {
                        let r = icon_button(ui, "rateFlag", Icon::Star, vec2(44.0, 44.0), false, true, "Rating, Flag and Label");
                        egui::Popup::menu(&r).show(|ui| culling_menu(app, ui, &[id.0]));
                    }
                });
            });
        });
}

fn tab_bar(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    // the bottom safe area is already outside this ui (see the host); the bar sits at its edge
    egui::Panel::bottom("compact_tabs").exact_size(BAR_H).frame(egui::Frame::NONE.fill(t.chrome).stroke(egui::Stroke::new(1.0, t.divider))).show(
        ui,
        |ui| {
            let has_photo = app.session.active().is_some();
            let tools = [
                ("presets", Icon::Presets, RightPanel::None, "Presets"),
                ("edit", Icon::Sliders, RightPanel::Edit, "Edit"),
                ("crop", Icon::Crop, RightPanel::Crop, "Crop & Rotate"),
                ("remove", Icon::Eraser, RightPanel::Remove, "Remove"),
                ("masking", Icon::Mask, RightPanel::Masking, "Masking"),
                ("info", Icon::Info, RightPanel::Info, "Info"),
            ];
            let w = ui.max_rect().width() / tools.len() as f32;
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                for (id, icon, panel, tip) in tools {
                    let presets = id == "presets";
                    let on = if presets {
                        app.ui.presets
                    } else {
                        !app.ui.presets && (app.ui.right == panel || (panel == RightPanel::Edit && app.ui.right == RightPanel::Profiles))
                    };
                    if icon_button(ui, id, icon, vec2(w, BAR_H - 4.0), on, has_photo, tip).clicked() {
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
        },
    );
}

fn sheet(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens, wide: bool) {
    let h = ui.max_rect().height();
    let frame = egui::Frame::NONE.fill(t.chrome).stroke(egui::Stroke::new(1.0, t.divider));
    let body = |app: &mut LightcraftApp, ui: &mut egui::Ui| {
        if app.ui.presets { super::presets::body(app, ui) } else { super::right::body(app, ui) }
    };
    if wide {
        egui::Panel::right("compact_side").resizable(true).default_size(SIDE_W).size_range(280.0..=480.0).frame(frame).show(ui, |ui| body(app, ui));
    } else {
        egui::Panel::bottom("compact_sheet")
            .resizable(true)
            .default_size(h * SHEET_FRACTION)
            .size_range(120.0..=h * 0.85)
            .frame(frame)
            .show(ui, |ui| body(app, ui));
    }
}

/// A text button sized for a finger in a bar (`button:<id>`).
fn bar_text(ui: &mut egui::Ui, id: &str, label: &str, t: &Tokens) -> egui::Response {
    let label = crate::i18n::tr(label);
    let galley = ui.painter().layout_no_wrap(label.to_string(), t.semibold(15.0), t.accent);
    let (r, resp) = ui.allocate_exact_size(vec2(galley.size().x + 20.0, 44.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    crate::widgets::register(ui.ctx(), format!("button:{id}"), r);
    let color = if resp.is_pointer_button_down_on() { t.accent.gamma_multiply(0.7) } else { t.accent };
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
/// export, more (copy / paste settings) and delete.
fn action_bar(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    egui::Panel::bottom("compact_actions").exact_size(BAR_H).frame(egui::Frame::NONE.fill(t.chrome).stroke(Stroke::new(1.0, t.divider))).show(
        ui,
        |ui| {
            let ids: Vec<u64> = app.session.selection.ids.iter().map(|id| id.0).collect();
            let any = !ids.is_empty();
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
                    let r = icon_button(ui, id, icon, vec2(w, BAR_H - 4.0), false, any, tip);
                    match id {
                        "selRate" => {
                            egui::Popup::menu(&r).show(|ui| rating_items(app, ui, &ids));
                        }
                        "selFlag" => {
                            egui::Popup::menu(&r).show(|ui| flag_items(app, ui, &ids));
                        }
                        "selLabel" => {
                            egui::Popup::menu(&r).show(|ui| label_items(app, ui, &ids));
                        }
                        "selAlbum" => {
                            egui::Popup::menu(&r).show(|ui| album_items(app, ui, &ids));
                        }
                        "selMore" => {
                            egui::Popup::menu(&r).show(|ui| more_items(app, ui, &ids));
                        }
                        "selExport" if r.clicked() => {
                            let _ = app.run("dialog.export", json!({}));
                        }
                        "selDelete" if r.clicked() => {
                            // (asks first when Settings say so; the photos go to Recently Deleted)
                            if let Err(e) = crate::menubar::run_item(app, "photo.delete", json!({})) {
                                app.toast(ui.ctx(), e);
                            }
                        }
                        _ => {}
                    }
                }
            });
        },
    );
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
    ui.close();
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
        ui.close();
    }
    if app.session.clipboard.is_some() && menu_row(ui, "pasteSettings", crate::i18n::tr("Paste Edit Settings"), false) {
        run_on(app, ui, "develop.paste", json!({}), ids);
    }
    if app.session.clipboard.is_some() && menu_row(ui, "pasteSelected", crate::i18n::tr("Paste Selected Settings…"), false) {
        let _ = app.run("dialog.pasteSettings", json!({}));
        ui.close();
    }
    if menu_row(ui, "selectEdit", crate::i18n::tr("Edit"), false)
        && let Some(first) = ids.first()
    {
        let _ = app.run("library.select", json!({"ids": [first]}));
        let _ = app.run("view.selectMode", json!({"on": false}));
        let _ = app.run("view.detail", json!({}));
        ui.close();
    }
}
