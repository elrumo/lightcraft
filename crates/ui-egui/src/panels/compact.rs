//! The compact (phone-sized) layout: one panel at a time. A slim top bar, the grid or loupe in the
//! middle, and in the loupe a bottom tab bar of tools with the active tool's panel as a bottom
//! sheet. It reuses the desktop panels' bodies (`right::body`) and commands, so every control
//! stays a `develop` control spec / command.

use egui::{Align2, vec2};
use serde_json::json;

use crate::LightcraftApp;
use crate::icons::Icon;
use crate::state::{RightPanel, ViewMode};
use crate::theme::Tokens;
use crate::widgets::icon_button;

/// Height of the top bar and of the tab bar: Apple's minimum touch target is 44 pt.
const BAR_H: f32 = 52.0;
/// The sheet opens at this fraction of the window height.
const SHEET_FRACTION: f32 = 0.4;

pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let t = Tokens::get(&ctx);
    let detail = app.ui.view == ViewMode::Detail;
    top_bar(app, ui, &t);
    if detail {
        tab_bar(app, ui, &t);
        if app.ui.right != RightPanel::None {
            sheet(app, ui, &t);
        }
    }
    let bg = if detail { t.canvas } else { t.grid_bg };
    egui::CentralPanel::default().frame(egui::Frame::NONE.fill(bg)).show(ui, |ui| match app.ui.view {
        ViewMode::Detail => super::detail::show(app, ui),
        ViewMode::Compare => super::compare::show_compare(app, ui),
        ViewMode::Survey => super::compare::show_survey(app, ui),
        ViewMode::Reference => super::compare::show_reference(app, ui),
        ViewMode::People => super::people::show(app, ui),
        ViewMode::PhotoGrid | ViewMode::SquareGrid => super::grid::show(app, ui),
    });
    super::second::show(app, &ctx);
    super::notices::show(app, &ctx);
    super::dialogs::show(app, &ctx);
    super::library_problem::show(app, &ctx);
    crate::import::progress(app, &ctx);
    crate::import::scan_progress(app, &ctx);
    crate::export_task::poll(app, &ctx);
    super::toast(app, &ctx);
}

fn top_bar(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    egui::Panel::top("compact_top")
        .exact_size(BAR_H)
        .frame(egui::Frame::NONE.fill(t.chrome).inner_margin(egui::Margin::symmetric(8, 0)).stroke(egui::Stroke::new(1.0, t.divider)))
        .show(ui, |ui| {
            let full = ui.max_rect();
            ui.horizontal_centered(|ui| {
                let grid = matches!(app.ui.view, ViewMode::PhotoGrid | ViewMode::SquareGrid);
                if icon_button(ui, "back", Icon::Back, vec2(44.0, 44.0), false, !grid, "Back").clicked() {
                    let _ = app.run("view.back", json!({}));
                }
            });
            let title = if app.ui.view == ViewMode::Detail { "Edit" } else { "Photos" };
            ui.painter().text(full.center(), Align2::CENTER_CENTER, crate::i18n::tr(title), t.semibold(17.0), t.text);
        });
}

fn tab_bar(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    // the bottom safe area is already outside this ui (see the host); the bar sits at its edge
    egui::Panel::bottom("compact_tabs").exact_size(BAR_H).frame(egui::Frame::NONE.fill(t.chrome).stroke(egui::Stroke::new(1.0, t.divider))).show(
        ui,
        |ui| {
            let has_photo = app.session.active().is_some();
            let tools = [
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
                    let on = app.ui.right == panel || (panel == RightPanel::Edit && app.ui.right == RightPanel::Profiles);
                    if icon_button(ui, id, icon, vec2(w, BAR_H - 4.0), on, has_photo, tip).clicked() {
                        // tapping the open tool closes its sheet
                        if app.ui.right == panel {
                            app.ui.right = RightPanel::None;
                        } else {
                            let _ = app.run(&format!("panel.{id}"), json!({}));
                        }
                    }
                }
            });
        },
    );
}

fn sheet(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let h = ui.max_rect().height();
    egui::Panel::bottom("compact_sheet")
        .resizable(true)
        .default_size(h * SHEET_FRACTION)
        .size_range(120.0..=h * 0.85)
        .frame(egui::Frame::NONE.fill(t.chrome).stroke(egui::Stroke::new(1.0, t.divider)))
        .show(ui, |ui| super::right::body(app, ui));
}
