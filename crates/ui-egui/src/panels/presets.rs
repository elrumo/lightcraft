//! The Presets column (opens to the left of the Edit panel): grouped presets with hover preview
//! (the loupe shows the photo with the preset while the pointer rests on it; nothing is
//! committed), optional live thumbnails, amount slider, create/favourite, import/export of preset
//! files.

use egui::{Align2, Color32, Rect, Sense, pos2, vec2};
use lightcraft_catalog::PhotoId;
use lightcraft_develop::{ControlSpec, DevelopSettings, Preset, Section, Track};
use serde_json::json;

use crate::icons::{Icon, paint};
use crate::theme::Tokens;
use crate::widgets::{divider, icon_button, register, slider};
use crate::{HoverPreview, LightcraftApp};

/// Long edge of preset thumbnails (px).
const THUMB_EDGE: usize = 128;

/// The presets by group, in the order the user put the groups in ([`lightcraft_engine::Session::preset_groups`]).
fn grouped(app: &LightcraftApp) -> Vec<(String, Vec<Preset>)> {
    let s = &app.session;
    s.preset_groups().into_iter().map(|g| (g.clone(), s.presets.iter().filter(|p| p.group == g).cloned().collect())).collect()
}

pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    egui::Frame::NONE.fill(t.chrome).stroke(egui::Stroke::new(1.0, t.divider)).show(ui, |ui| body(app, ui));
}

/// The presets list: the column left of the tool strip on desktop, a tool sheet in the compact layout.
pub fn body(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    if !app.compact {
        return list(app, ui);
    }
    // (the sheet is a panel of a fixed height: what doesn't fit scrolls, and the sheet learns how tall it needs to be)
    let out = egui::ScrollArea::vertical().id_salt("presets-sheet").auto_shrink([false, false]).show(ui, |ui| list(app, ui));
    super::compact::note_sheet_content(app, ui.ctx(), out.content_size.y);
}

fn list(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    ui.spacing_mut().item_spacing.y = 0.0;
    let compact = app.compact;
    if compact {
        ui.spacing_mut().interact_size.y = 36.0;
    }
    let (hr, _) = ui.allocate_exact_size(vec2(ui.available_width(), if compact { 36.0 } else { 46.0 }), Sense::hover());
    let pad = crate::widgets::side_pad(ui.ctx()).0;
    ui.painter().text(pos2(hr.left() + pad, hr.center().y + 2.0), Align2::LEFT_CENTER, crate::i18n::tr("Presets"), t.semibold(15.0), t.text);
    let buttons = if compact {
        Rect::from_min_max(pos2(hr.right() - 92.0, hr.top()), pos2(hr.right() - 8.0, hr.bottom()))
    } else {
        Rect::from_min_max(pos2(hr.right() - 80.0, hr.top()), hr.right_bottom())
    };
    let mut hdr = ui.new_child(egui::UiBuilder::new().max_rect(buttons).layout(egui::Layout::right_to_left(egui::Align::Center)));
    if icon_button(&mut hdr, "presetCreate", Icon::Plus, vec2(28.0, 28.0), false, app.session.active().is_some(), "Create Preset…").clicked() {
        app.ui.dialog = Some(crate::state::Dialog::create_preset());
    }
    let more = icon_button(&mut hdr, "presetMore", Icon::More, vec2(28.0, 28.0), false, true, "More preset options");
    egui::Popup::menu(&more).show(|ui| {
        let r = crate::widgets::check(ui, &mut app.ui.preset_thumbs, crate::i18n::tr("Show Thumbnails"));
        register(ui.ctx(), "presetMenu:thumbnails", r.rect);
        ui.separator();
        // (a failure is said, never dropped: a button that does nothing looks broken)
        let r = ui.button(crate::i18n::tr("Import Presets…"));
        register(ui.ctx(), "presetMenu:import", r.rect);
        if r.clicked()
            && let Err(e) = app.run("file.importPresets", json!({}))
        {
            app.toast_error(ui.ctx(), e);
        }
        let any_user = app.session.presets.iter().any(|p| !p.builtin);
        if ui.add_enabled(any_user, egui::Button::new(crate::i18n::tr("Export User Presets…"))).clicked()
            && let Err(e) = app.run("file.exportPresets", json!({}))
        {
            app.toast_error(ui.ctx(), e);
        }
    });
    divider(ui);
    // amount slider (applies to the last applied preset)
    let amt_id = egui::Id::new("preset-amount");
    let last: Option<(String, f64)> = ui.data(|d| d.get_temp(amt_id));
    if let Some((pid, amount)) = last.clone() {
        let spec = ControlSpec {
            id: "presetAmount",
            label: "Amount",
            section: Section::Profile,
            min: 0.0,
            max: 200.0,
            default: 100.0,
            step: 1.0,
            decimals: 0,
            track: Track::Plain,
        };
        let out = slider(ui, &spec, amount, true, None);
        if let Some(v) = out.value {
            // re-apply from the pre-preset state: undo last preset step then apply with the new amount
            let _ = app.run("edit.undo", json!({}));
            let _ = app.run("preset.apply", json!({"id": pid, "amount": v}));
            ui.data_mut(|d| d.insert_temp(amt_id, (pid, v)));
        }
        divider(ui);
    }
    let active = app.session.active();
    let current = active.and_then(|id| app.session.develop_of(id)).map(|d| (*d).clone());
    if compact {
        strip(app, ui, active, current.as_ref(), last.as_ref().map(|(pid, _)| pid.as_str()));
        return;
    }
    let thumbs = app.ui.preset_thumbs && active.is_some();
    let row_h = if thumbs { 54.0 } else { 26.0 };
    let scroll = egui::ScrollArea::vertical().id_salt("presets-scroll").auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        // a group header dragged up or down puts the group elsewhere (`preset.moveGroup`)
        let drag_id = egui::Id::new("preset-group-drag");
        let dragging: Option<String> = ui.data(|d| d.get_temp(drag_id));
        let groups = grouped(app);
        let mut headers: Vec<(String, Rect)> = Vec::with_capacity(groups.len());
        for (g, items) in groups {
            // A group holding stock presets is a stock group (shown in the UI language), even
            // when user presets share its name; a group of user presets keeps its name.
            let builtin = items.iter().any(|p| p.builtin);
            let open_id = egui::Id::new(("preset-group", &g));
            let open: bool = ui.data(|d| d.get_temp(open_id)).unwrap_or(true);
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click_and_drag());
            register(ui.ctx(), format!("presetGroup:{g}"), r);
            headers.push((g.clone(), r));
            if resp.drag_started() {
                ui.data_mut(|d| d.insert_temp(drag_id, g.clone()));
            }
            if dragging.as_ref() == Some(&g) {
                ui.painter().rect_filled(r, 0.0, t.hover.gamma_multiply(0.7));
            } else if resp.hovered() && dragging.is_none() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
            }
            paint(
                ui.painter(),
                Rect::from_center_size(pos2(r.left() + 22.0, r.center().y), vec2(12.0, 12.0)),
                if open { Icon::ChevronDown } else { Icon::ChevronRight },
                t.text_label,
            );
            ui.painter().text(
                pos2(r.left() + 36.0, r.center().y),
                Align2::LEFT_CENTER,
                crate::i18n::builtin_label(&g, builtin),
                t.semibold(13.0),
                t.text_label,
            );
            if resp.clicked() {
                ui.data_mut(|d| d.insert_temp(open_id, !open));
            }
            resp.context_menu(|ui| {
                if ui.button(crate::i18n::tr("Export Group…")).clicked() {
                    let _ = app.run("file.exportPresets", json!({"group": g}));
                }
            });
            if !open {
                continue;
            }
            for pr in items {
                let (pid, name, fav) = (pr.id.clone(), crate::i18n::builtin_label(&pr.name, pr.builtin).to_string(), pr.favorite);
                let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click());
                register(ui.ctx(), format!("preset:{pid}"), r);
                let is_last = last.as_ref().is_some_and(|(l, _)| *l == pid);
                if is_last {
                    ui.painter().rect_filled(r, 0.0, t.tool_active);
                } else if resp.hovered() {
                    ui.painter().rect_filled(r, 0.0, t.hover.gamma_multiply(0.7));
                }
                let look = if thumbs || resp.hovered() { current.as_ref().map(|d| pr.apply(d, 1.0)) } else { None };
                let mut text_x = r.left() + 40.0;
                if thumbs && let (Some(id), Some(s)) = (active, look.as_ref()) {
                    let tr = Rect::from_min_size(pos2(r.left() + 36.0, r.top() + 5.0), vec2(66.0, row_h - 10.0));
                    ui.painter().rect_filled(tr, 2.0, t.canvas);
                    if let Some(job) = app.session.variant_job(id, s, THUMB_EDGE)
                        && let Some(tex) = app.renderer.variant(job)
                    {
                        ui.painter().image(tex.tex.id(), tr, cover_uv(tr, tex.size), Color32::WHITE);
                    }
                    text_x = tr.right() + 10.0;
                }
                ui.painter().text(pos2(text_x, r.center().y), Align2::LEFT_CENTER, &name, t.font(13.0), t.text_label);
                if fav {
                    paint(ui.painter(), Rect::from_center_size(pos2(r.right() - 20.0, r.center().y), vec2(12.0, 12.0)), Icon::StarFilled, t.star);
                }
                // resting on a preset previews it in the loupe (no history entry)
                if resp.hovered()
                    && let Some(s) = look
                {
                    let label = crate::i18n::tr_format!("Preset: {name}", name = name);
                    app.hover_preview = Some(HoverPreview { label: label.clone(), settings: s });
                    if let Some(id) = active {
                        ui.data_mut(|d| d.insert_temp(egui::Id::new("last-preset-hover"), (id, pr.clone(), label)));
                    }
                }
                if resp.clicked() {
                    let _ = app.run("preset.apply", json!({"id": pid, "amount": 100}));
                    ui.data_mut(|d| d.insert_temp(amt_id, (pid.clone(), 100.0)));
                    ui.data_mut(|d| d.remove::<(lightcraft_catalog::PhotoId, Preset, String)>(egui::Id::new("last-preset-hover")));
                    app.toast(ui.ctx(), crate::i18n::tr_format!("Preset: {name}", name = name));
                }
                resp.context_menu(|ui| preset_menu(app, ui, &pr, &name));
            }
        }
        if let Some(g) = dragging {
            drop_group(app, ui, drag_id, &g, &headers);
        }
        ui.add_space(30.0);
    });
    let last_hover_id = egui::Id::new("last-preset-hover");
    let pointer_in_scroll = ui.ctx().input(|i| i.pointer.hover_pos()).is_some_and(|pos| scroll.inner_rect.contains(pos));
    if pointer_in_scroll {
        if app.hover_preview.is_none()
            && let (Some(id), Some(curr)) = (active, current.as_ref())
            && let Some((saved_id, saved_preset, saved_label)) =
                ui.data(|d| d.get_temp::<(lightcraft_catalog::PhotoId, Preset, String)>(last_hover_id))
            && saved_id == id
        {
            app.hover_preview = Some(HoverPreview { label: saved_label, settings: saved_preset.apply(curr, 1.0) });
        }
    } else {
        ui.data_mut(|d| d.remove::<(lightcraft_catalog::PhotoId, Preset, String)>(last_hover_id));
    }
}

/// While group `dragged` is dragged: a line where it would go, between the `headers` (top to
/// bottom), and the move on release.
fn drop_group(app: &mut LightcraftApp, ui: &mut egui::Ui, drag_id: egui::Id, dragged: &str, headers: &[(String, Rect)]) {
    let t = Tokens::get(ui.ctx());
    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    let (pos, down) = ui.input(|i| (i.pointer.latest_pos(), i.pointer.primary_down()));
    let from = headers.iter().position(|(g, _)| g == dragged);
    // the gap before the first header below the pointer (or after the last group)
    let slot = pos.map(|p| headers.iter().position(|(_, r)| p.y < r.center().y).unwrap_or(headers.len()));
    let to = match (from, slot) {
        (Some(f), Some(s)) if s != f && s != f + 1 => Some(if s > f { s - 1 } else { s }),
        _ => None,
    };
    if let (Some(to), Some(s)) = (to, slot) {
        let y = headers.get(s).map_or(ui.cursor().top(), |(_, r)| r.top()) + 1.0;
        let (x0, x1) = (ui.max_rect().left() + 18.0, ui.max_rect().right() - 8.0);
        ui.painter().circle_stroke(pos2(x0, y), 3.0, egui::Stroke::new(1.5, t.accent));
        ui.painter().line_segment([pos2(x0 + 3.0, y), pos2(x1.max(x0 + 3.0), y)], egui::Stroke::new(2.0, t.accent));
        if !down && let Err(e) = app.run("preset.moveGroup", json!({"group": dragged, "to": to})) {
            app.toast_error(ui.ctx(), e);
        }
    }
    if !down {
        ui.data_mut(|d| d.remove_temp::<String>(drag_id));
    }
}

/// The edge of a preset tile on a phone.
const TILE: f32 = 84.0;

/// A phone's presets, as Lightroom's mobile app shows them: the groups as chips, and the chosen
/// group's presets as a row of thumbnails of this photo that scrolls sideways. All of it fits a
/// short sheet, where a list of rows wanted most of the screen. Only the tiles on screen are rendered.
fn strip(app: &mut LightcraftApp, ui: &mut egui::Ui, active: Option<PhotoId>, current: Option<&DevelopSettings>, last: Option<&str>) {
    let t = Tokens::get(ui.ctx());
    let mut groups = grouped(app);
    let key = egui::Id::new("preset-strip-group");
    // an import just added presets: show the group they went to, and its chip too
    let show_chosen = app.pending_strip_group.is_some();
    if let Some(g) = app.pending_strip_group.take() {
        ui.data_mut(|d| d.insert_temp(key, g));
    }
    let in_last = |items: &Vec<Preset>| last.is_some_and(|l| items.iter().any(|p| p.id == l));
    let chosen: Option<String> = ui
        .data(|d| d.get_temp::<String>(key))
        .filter(|g| groups.iter().any(|(k, _)| k == g))
        .or_else(|| groups.iter().find(|(_, items)| in_last(items)).map(|(g, _)| g.clone()))
        .or_else(|| groups.first().map(|(g, _)| g.clone()));
    let Some(chosen) = chosen else { return };
    let pad = crate::widgets::side_pad(ui.ctx()).0;
    let hidden = egui::scroll_area::ScrollBarVisibility::AlwaysHidden;
    let mut picked = chosen.clone();
    ui.add_space(6.0);
    egui::ScrollArea::horizontal().id_salt("preset-groups").scroll_bar_visibility(hidden).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.spacing_mut().interact_size.y = 36.0;
            ui.add_space(pad);
            for (g, items) in &groups {
                let builtin = items.iter().any(|p| p.builtin);
                let chip = crate::widgets::text_button(ui, &format!("presetGroup:{g}"), crate::i18n::builtin_label(g, builtin), *g == chosen);
                if show_chosen && *g == chosen {
                    chip.scroll_to_me(Some(egui::Align::Center));
                }
                if chip.clicked() {
                    picked = g.clone();
                }
            }
            ui.add_space(pad);
        });
    });
    ui.data_mut(|d| d.insert_temp(key, picked.clone()));
    ui.add_space(10.0);
    let items = groups.iter().position(|(g, _)| *g == picked).map(|i| groups.swap_remove(i).1).unwrap_or_default();
    egui::ScrollArea::horizontal().id_salt(("preset-tiles", &picked)).scroll_bar_visibility(hidden).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            ui.add_space(pad);
            for pr in items {
                let name = crate::i18n::builtin_label(&pr.name, pr.builtin).to_string();
                let (r, resp) = ui.allocate_exact_size(vec2(TILE, TILE + 24.0), Sense::click());
                register(ui.ctx(), format!("preset:{}", pr.id), r);
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &name));
                let img = Rect::from_min_size(r.min, vec2(TILE, TILE));
                let mut uv_texture = None;
                if ui.is_rect_visible(r)
                    && let (Some(id), Some(cur)) = (active, current)
                    && let Some(job) = app.session.variant_job(id, &pr.apply(cur, 1.0), THUMB_EDGE)
                    && let Some(tex) = app.renderer.variant(job)
                {
                    uv_texture = Some((tex.tex.id(), cover_uv(img, tex.size)));
                }
                let round = 10;
                match uv_texture {
                    Some((tex, uv)) => {
                        ui.painter().add(egui::epaint::RectShape::filled(img, round, Color32::WHITE).with_texture(tex, uv));
                    }
                    None => {
                        ui.painter().rect_filled(img, round, t.inset);
                    }
                }
                let on = last == Some(pr.id.as_str());
                if on {
                    ui.painter().rect_stroke(img.shrink(1.0), round, egui::Stroke::new(2.5, t.accent), egui::StrokeKind::Inside);
                }
                if pr.favorite {
                    paint(ui.painter(), Rect::from_center_size(img.right_top() + vec2(-12.0, 12.0), vec2(13.0, 13.0)), Icon::StarFilled, t.star);
                }
                let color = if on { t.text } else { t.text_dim };
                let mut job = egui::text::LayoutJob::simple(name.clone(), t.font(12.0), color, TILE);
                job.wrap.max_rows = 1;
                job.wrap.overflow_character = Some('…');
                let label = ui.painter().layout_job(job);
                ui.painter().galley(pos2(r.center().x - label.size().x.min(TILE) / 2.0, img.bottom() + 6.0), label, color);
                if resp.is_pointer_button_down_on() {
                    ui.painter().rect_filled(img, round, Color32::from_black_alpha(70));
                }
                if resp.clicked() {
                    let _ = app.run("preset.apply", json!({"id": pr.id, "amount": 100}));
                    ui.data_mut(|d| d.insert_temp(egui::Id::new("preset-amount"), (pr.id.clone(), 100.0)));
                    app.toast(ui.ctx(), crate::i18n::tr_format!("Preset: {name}", name = name));
                }
                resp.context_menu(|ui| preset_menu(app, ui, &pr, &name));
            }
            ui.add_space(pad);
        });
    });
    ui.add_space(8.0);
}

/// A preset's menu (a right click, or a long press on a phone): favourite it; a user's own preset can
/// also be updated from the photo, renamed, moved to another group and deleted.
fn preset_menu(app: &mut LightcraftApp, ui: &mut egui::Ui, pr: &Preset, name: &str) {
    let (pid, fav, builtin, group) = (pr.id.clone(), pr.favorite, pr.builtin, pr.group.clone());
    if ui.button(crate::i18n::tr(if fav { "Remove from Favorites" } else { "Add to Favorites" })).clicked() {
        let _ = app.run("preset.favorite", json!({"id": pid}));
    }
    if builtin {
        return;
    }
    ui.separator();
    if ui.add_enabled(app.session.active().is_some(), egui::Button::new(crate::i18n::tr("Update with Current Settings"))).clicked() {
        match app.run("preset.update", json!({"id": pid})) {
            Ok(_) => app.toast(ui.ctx(), crate::i18n::tr_format!("Updated “{name}”", name = name)),
            Err(e) => app.toast(ui.ctx(), e),
        }
    }
    if ui.button(crate::i18n::tr("Rename…")).clicked() {
        crate::panels::dialogs::prompt(app, "Rename Preset", "Preset name", name, "preset.rename", json!({"id": pid}), "name");
    }
    ui.menu_button(crate::i18n::tr("Move to Group"), |ui| {
        let mut groups: Vec<String> = app.session.presets.iter().filter(|p| !p.builtin).map(|p| p.group.clone()).collect();
        groups.sort();
        groups.dedup();
        for g in groups.iter().filter(|g| **g != group) {
            if ui.button(g).clicked() {
                let _ = app.run("preset.move", json!({"id": pid, "group": g}));
            }
        }
        if ui.button(crate::i18n::tr("New Group…")).clicked() {
            crate::panels::dialogs::prompt(app, "Move Preset to New Group", "Group name", "", "preset.move", json!({"id": pid}), "group");
        }
    });
    ui.separator();
    if ui.button(crate::i18n::tr("Delete Preset")).clicked() {
        let _ = app.run("preset.delete", json!({"id": pid}));
    }
}

/// The part of a `size` image that covers `r` (centre crop), as texture coordinates.
pub fn cover_uv(r: Rect, size: [usize; 2]) -> Rect {
    let a = size[0] as f32 / size[1].max(1) as f32;
    let ra = r.width() / r.height().max(1.0);
    if a > ra {
        let f = ra / a;
        Rect::from_min_max(pos2((1.0 - f) / 2.0, 0.0), pos2((1.0 + f) / 2.0, 1.0))
    } else {
        let f = a / ra;
        Rect::from_min_max(pos2(0.0, (1.0 - f) / 2.0), pos2(1.0, (1.0 + f) / 2.0))
    }
}
