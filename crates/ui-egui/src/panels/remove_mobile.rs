//! The Remove screen of the compact (phone) layout, modelled on Lightroom's mobile app. While the
//! Remove tool is open it takes the screen over from the tool bar and the tool sheet: the photo
//! with its spots; under it a panel with the tool — Remove, Heal or Clone, one tile that opens a
//! menu of the three — beside its brush size (Heal and Clone also have feather and opacity, Remove
//! the Generative AI switch); and a
//! bar with ✕, the tool's name and ✓. Paint over a distraction to remove it; tap a spot's pin for
//! its options (its tool, another source, delete). Edits apply live: ✓ keeps them, ✕ undoes every
//! step taken since the screen opened. Undo and a "…" menu (Find Dust Spots, Visualize Spots,
//! Delete All Spots) sit above the photo. The drawing and the words are our own. Held sideways, the
//! title, the panel and the bar are one column on the right.

use egui::{Align2, Color32, Rect, Sense, Stroke, Ui, UiBuilder, pos2, vec2};
use lightcraft_develop::SpotMode;
use serde_json::json;

use super::{compact, mobile};
use crate::LightcraftApp;
use crate::haptics::{self, Haptic};
use crate::icons::{self, Icon};
use crate::state::{RightPanel, ViewMode};
use crate::theme::Tokens;
use crate::widgets::{TOUCH_SLIDER_ROW_H, register};

/// The title row over the photo, and the bar of ✕ and ✓ under everything.
const TITLE_H: f32 = 44.0;
const BAR_H: f32 = 50.0;
/// The tool's tile, and the panel's first row (the tile with its name under it, beside Size).
const TILE: f32 = 56.0;
const FIRST_ROW_H: f32 = 86.0;
/// The column held sideways.
const SIDE_W: f32 = 316.0;

/// The three tools: (`app.ui.tool`, the spots' mode, icon, name).
const TOOLS: [(&str, SpotMode, Icon, &str); 3] = [
    ("remove", SpotMode::Remove, Icon::Eraser, "Remove"),
    ("heal", SpotMode::Heal, Icon::Heal, "Heal"),
    ("clone", SpotMode::Clone, Icon::Stamp, "Clone"),
];

/// Whether the compact layout shows the Remove screen: a photo with the Remove tool open.
pub fn open(app: &LightcraftApp) -> bool {
    app.compact && app.ui.view == ViewMode::Detail && !app.ui.presets && app.ui.right == RightPanel::Remove && app.session.active().is_some()
}

/// The tool in hand (Remove unless Heal or Clone was chosen).
fn tool(app: &LightcraftApp) -> (&'static str, SpotMode, Icon, &'static str) {
    TOOLS.into_iter().find(|t| t.0 == app.ui.tool).unwrap_or(TOOLS[0])
}

/// The screen's panels: the title, the tool's panel and the bar. The photo takes what is left
/// (`compact::content`).
pub fn show(app: &mut LightcraftApp, ui: &mut Ui, t: &Tokens, landscape: bool) {
    // (the tool paints on the photo: Remove, Heal or Clone is always in hand here)
    if !TOOLS.iter().any(|x| x.0 == app.ui.tool) {
        app.ui.tool = "remove".into();
    }
    let frame = |fill: Color32| egui::Frame::NONE.fill(fill);
    if landscape {
        egui::Panel::right("remove_side").resizable(false).exact_size(SIDE_W).frame(frame(t.chrome)).show(ui, |ui| {
            let full = ui.max_rect();
            let part = |top: f32, bottom: f32| Rect::from_min_max(pos2(full.left(), top), pos2(full.right(), bottom));
            let mut title = ui.new_child(UiBuilder::new().max_rect(part(full.top(), full.top() + TITLE_H)));
            title_bar(app, &mut title, t);
            let mut body = ui.new_child(UiBuilder::new().max_rect(part(full.top() + TITLE_H, full.bottom() - BAR_H)));
            egui::ScrollArea::vertical().id_salt("remove-side").show(&mut body, |ui| panel(app, ui, t));
            let mut bar = ui.new_child(UiBuilder::new().max_rect(part(full.bottom() - BAR_H, full.bottom())));
            bottom_bar(app, &mut bar, t);
        });
    } else {
        egui::Panel::top("remove_top").show_separator_line(false).exact_size(TITLE_H).frame(frame(t.canvas)).show(ui, |ui| title_bar(app, ui, t));
        egui::Panel::bottom("remove_bar").show_separator_line(false).exact_size(BAR_H).frame(frame(t.chrome)).show(ui, |ui| {
            super::compact::fill_below(ui, ui.max_rect(), t.chrome);
            bottom_bar(app, ui, t);
        });
        egui::Panel::bottom("remove_panel").show_separator_line(false).exact_size(panel_height(app)).frame(frame(t.chrome)).show(ui, |ui| {
            panel(app, ui, t);
        });
    }
}

/// The panel's height: the tool and Size, then a slider row for each of the others it has.
fn panel_height(app: &LightcraftApp) -> f32 {
    // Remove has the Generative AI switch (and the model's download offer under it while it's missing)
    let tool_rows = if tool(app).1 == SpotMode::Remove { 1.0 + crate::erase::extra_rows(app) } else { 2.0 };
    let rows = tool_rows + if app.ui.visualize_spots { 1.0 } else { 0.0 };
    FIRST_ROW_H + rows * TOUCH_SLIDER_ROW_H + 8.0
}

// -------------------------------------------------------------------------------- the title

/// Over the photo: which spot is selected in the middle; Undo and "…" on the right.
fn title_bar(app: &mut LightcraftApp, ui: &mut Ui, t: &Tokens) {
    let r = ui.max_rect();
    if let Some(d) = app.session.active().and_then(|id| app.session.develop_of(id))
        && let Some((i, sp)) = app.session.active_spot.and_then(|i| d.spots.get(i).map(|s| (i, s)))
    {
        let mode = crate::i18n::tr(super::right::spot_mode_name(sp.mode));
        let text = crate::i18n::tr_format!("{mode} spot {} of {}", i + 1, d.spots.len(), mode = mode);
        ui.painter().text(r.center(), Align2::CENTER_CENTER, text, t.font(15.0), t.text_label);
    }
    let mut right = ui.new_child(UiBuilder::new().max_rect(r.shrink2(vec2(6.0, 0.0))).layout(egui::Layout::right_to_left(egui::Align::Center)));
    right.spacing_mut().item_spacing.x = 0.0;
    let more = compact::bar_icon(&mut right, "removeMore", Icon::Dots, "More", true);
    if more.clicked() {
        mobile::open_menu(ui.ctx(), "removeMore", more.rect);
    }
    let can_undo = lightcraft_engine::cmd::can_undo(&app.session).is_ok();
    if compact::bar_icon(&mut right, "undo", Icon::Undo, "Undo", can_undo).clicked() {
        let _ = app.run("edit.undo", json!({}));
        haptics::tap(ui.ctx(), Haptic::Light);
    }
}

// -------------------------------------------------------------------------------- the bar

/// ✕ and ✓ with the tool's name between them, and an ⓘ that says how to use it.
fn bottom_bar(app: &mut LightcraftApp, ui: &mut Ui, t: &Tokens) {
    let r = ui.max_rect();
    let name = crate::i18n::tr(tool(app).3);
    let galley = ui.painter().layout_no_wrap(name.to_string(), t.font(17.0), t.text);
    let gap = 26.0;
    let x0 = r.center().x - (galley.size().x + gap) / 2.0;
    ui.painter().galley(pos2(x0, r.center().y - galley.size().y / 2.0), galley.clone(), t.text);
    let info = Rect::from_center_size(pos2(x0 + galley.size().x + gap - 6.0, r.center().y), vec2(36.0, 36.0));
    let resp = ui.interact(info, egui::Id::new("remove-help"), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, crate::i18n::tr("Help")));
    register(ui.ctx(), "icon:removeHelp", info);
    icons::paint(ui.painter(), Rect::from_center_size(info.center(), vec2(20.0, 20.0)), Icon::Info, t.text_label);
    if resp.clicked() {
        let help = match tool(app).1 {
            // (Content-Aware has no tile on the phone yet: it is never the tool in hand here)
            SpotMode::Remove | SpotMode::Fill => "Paint over a distraction to remove it. Tap a pin for its options.",
            SpotMode::Heal => "Paint over a blemish: it is healed from a nearby area, which you can drag. Tap a pin for its options.",
            SpotMode::Clone => "Paint over an area to copy another over it; drag the source to choose it. Tap a pin for its options.",
        };
        app.toast_for(ui.ctx(), crate::i18n::tr(help), 5.0);
    }
    let mut left = ui.new_child(UiBuilder::new().max_rect(r.shrink2(vec2(6.0, 0.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
    if compact::bar_icon(&mut left, "removeCancel", Icon::Close, "Cancel", true).clicked() {
        compact::cancel_screen(app);
    }
    let mut right = ui.new_child(UiBuilder::new().max_rect(r.shrink2(vec2(6.0, 0.0))).layout(egui::Layout::right_to_left(egui::Align::Center)));
    if crate::glass::circle_button(&mut right, "icon:removeDone", Icon::Check, "Done", true, Some(t.accent)).clicked() {
        let _ = app.run("tool.done", json!({}));
    }
}

// -------------------------------------------------------------------------------- the panel

/// The tool's tile beside its Size, then Feather and Opacity for Heal and Clone (and the Visualize
/// Spots threshold while that is on). With a spot selected, they are that spot's.
fn panel(app: &mut LightcraftApp, ui: &mut Ui, t: &Tokens) {
    let Some(d) = app.session.active().and_then(|id| app.session.develop_of(id)) else { return };
    ui.spacing_mut().item_spacing.y = 0.0;
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), FIRST_ROW_H), Sense::hover());
    let tile_r = Rect::from_min_size(pos2(row.left() + 16.0, row.top() + 8.0), vec2(TILE, TILE));
    tool_tile(app, ui, t, tile_r);
    // Size takes the rest of the row, level with the tile
    let size_r = Rect::from_min_max(pos2(tile_r.right() + 2.0, tile_r.top() + 4.0), pos2(row.right(), row.bottom()));
    let mut size = ui.new_child(UiBuilder::new().max_rect(size_r).layout(egui::Layout::top_down(egui::Align::Min)));
    super::right::spot_sliders(app, &mut size, &d, &["size"]);
    if tool(app).1 != SpotMode::Remove {
        super::right::spot_sliders(app, ui, &d, &["feather", "opacity"]);
    } else {
        ui.add_space(6.0);
        egui::Frame::NONE.inner_margin(egui::Margin::symmetric(16, 0)).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            crate::erase::switch(app, ui);
        });
    }
    if app.ui.visualize_spots {
        let spec = lightcraft_develop::ControlSpec {
            id: "ui.spotsThreshold",
            label: "Threshold",
            section: lightcraft_develop::Section::Detail,
            min: 0.0,
            max: 100.0,
            default: 50.0,
            step: 1.0,
            decimals: 0,
            track: lightcraft_develop::Track::Plain,
        };
        if let Some(v) = crate::widgets::slider(ui, &spec, app.ui.spots_threshold as f64, true, None).value {
            app.ui.spots_threshold = v as f32;
        }
    }
}

/// The tool's tile (`button:removeTool`): its icon white on the accent colour, a corner mark that
/// says it opens a menu (Remove, Heal, Clone), and its name under it.
fn tool_tile(app: &mut LightcraftApp, ui: &mut Ui, t: &Tokens, r: Rect) {
    let (_, _, icon, name) = tool(app);
    let resp = ui.interact(r, egui::Id::new("remove-tool-tile"), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, crate::i18n::tr(name)));
    register(ui.ctx(), "button:removeTool", r);
    let k = crate::glass::press(ui.ctx(), &resp);
    let tile = r.shrink(2.0 * k);
    let p = ui.painter();
    p.rect_filled(tile, 10.0, t.accent);
    icons::paint(p, Rect::from_center_size(tile.center(), vec2(28.0, 28.0)), icon, Color32::WHITE);
    let c = tile.right_bottom() - vec2(6.0, 6.0);
    p.add(egui::Shape::convex_polygon(vec![c, c - vec2(7.0, 0.0), c - vec2(0.0, 7.0)], Color32::WHITE, Stroke::NONE));
    p.text(pos2(r.center().x, r.bottom() + 12.0), Align2::CENTER_CENTER, crate::i18n::tr(name), t.font(12.0), t.text_dim);
    if resp.clicked() {
        haptics::tap(ui.ctx(), Haptic::Selection);
        mobile::open_menu(ui.ctx(), "removeTool", r);
    }
}

// -------------------------------------------------------------------------------- menus

/// The screen's menus: the tool's, the "…" and a spot's (tapping its pin, `detail::remove_overlay`);
/// called every frame, each draws only while open.
pub fn menus(app: &mut LightcraftApp, ctx: &egui::Context) {
    mobile::actions(ctx, "removeTool", None, |ui| {
        for (key, _, icon, label) in TOOLS {
            if mobile::row_checked(ui, &format!("removeMode-{key}"), Some(icon), crate::i18n::tr(label), true, Some(app.ui.tool == key)) {
                app.ui.tool = key.into();
            }
        }
    });
    mobile::actions(ctx, "removeMore", None, |ui| {
        if mobile::row(ui, "findDust", Some(Icon::Wand), crate::i18n::tr("Find Dust Spots"), true) {
            match app.run("spot.findDust", json!({})) {
                Ok(r) => {
                    let n = r["added"].as_u64().unwrap_or(0);
                    let msg = if n == 0 {
                        crate::i18n::tr("No dust spots found").to_string()
                    } else {
                        crate::i18n::tr_format!("Healed {n} dust spot{}", if n == 1 { "" } else { "s" }, n = n)
                    };
                    app.toast(ctx, msg);
                }
                Err(e) => app.toast(ctx, e),
            }
        }
        let on = app.ui.visualize_spots;
        if mobile::row_checked(ui, "visualizeSpots", Some(Icon::Eye), crate::i18n::tr("Visualize Spots"), true, Some(on)) {
            app.ui.visualize_spots = !on;
        }
        mobile::row_gap(ui);
        let any = app.session.active().and_then(|id| app.session.develop_of(id)).is_some_and(|d| !d.spots.is_empty());
        if mobile::row(ui, "removeClear", Some(Icon::Trash), crate::i18n::tr("Delete All Spots"), any) {
            let n = app.session.active().and_then(|id| app.session.develop_of(id)).map_or(0, |d| d.spots.len());
            for i in (0..n).rev() {
                let _ = app.run("spot.delete", json!({"index": i}));
            }
        }
    });
    let spot = app.session.active().and_then(|id| app.session.develop_of(id)).and_then(|d| {
        let i = app.session.active_spot?;
        d.spots.get(i).map(|s| (i, s.mode, d.spots.len()))
    });
    let title = spot
        .map(|(i, mode, n)| crate::i18n::tr_format!("{mode} spot {} of {}", i + 1, n, mode = crate::i18n::tr(super::right::spot_mode_name(mode))));
    mobile::actions(ctx, "spotPin", title.as_deref(), |ui| {
        let Some((i, mode, _)) = spot else { return };
        for (key, m, icon, label) in TOOLS {
            if mobile::row_checked(ui, &format!("spotMode-{key}"), Some(icon), crate::i18n::tr(label), true, Some(mode == m)) && mode != m {
                let _ = app.run("spot.update", json!({"index": i, "mode": key}));
            }
        }
        mobile::row_gap(ui);
        if mode != SpotMode::Remove
            && mobile::row(ui, "spotRefresh", Some(Icon::Refresh), crate::i18n::tr("New Source"), true)
            && let Err(e) = app.run("spot.refreshSource", json!({"index": i}))
        {
            app.toast(ctx, e);
        }
        if mobile::row(ui, "spotDelete", Some(Icon::Trash), crate::i18n::tr("Delete Spot"), true) {
            haptics::tap(ctx, Haptic::Warning);
            let _ = app.run("spot.delete", json!({"index": i}));
        }
    });
}
