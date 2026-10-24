//! The Presets column (opens to the left of the Edit panel): grouped presets with hover preview
//! (the loupe shows the photo with the preset while the pointer rests on it; nothing is
//! committed), optional live thumbnails, amount slider, create/favourite, import/export of preset
//! files.

use std::sync::Arc;

use egui::{Align2, Color32, Rect, Sense, pos2, vec2};
use lightcraft_catalog::PhotoId;
use lightcraft_develop::{ControlSpec, DevelopSettings, Preset, Section, Track};
use lightcraft_engine::presets::PresetList;
use serde_json::json;

use crate::icons::{Icon, paint};
use crate::theme::Tokens;
use crate::widgets::{divider, icon_button, register, slider};
use crate::{HoverPreview, LightcraftApp};

/// Long edge of preset thumbnails (px).
const THUMB_EDGE: usize = 128;

/// The presets by group, in the order the user put the groups in ([`lightcraft_engine::Session::preset_groups`]),
/// without the hidden ones unless the panel shows them.
fn grouped_presets(app: &LightcraftApp) -> Vec<(String, Vec<Preset>)> {
    let (s, show) = (&app.session, app.ui.show_hidden_presets);
    s.preset_groups()
        .into_iter()
        .map(|g| (g.clone(), s.presets.iter().filter(|p| p.group == g && (show || !s.hidden_presets.hides(p))).cloned().collect::<Vec<_>>()))
        .filter(|(_, items)| !items.is_empty())
        .collect()
}

/// The preset amount slider may replace only the undo step it created.
#[derive(Clone)]
struct AppliedPreset {
    id: String,
    amount: f64,
    selected: Vec<lightcraft_catalog::PhotoId>,
    active: Option<lightcraft_catalog::PhotoId>,
    commits: u64,
}

impl AppliedPreset {
    fn new(session: &lightcraft_engine::Session, id: String, amount: f64) -> Self {
        Self { id, amount, selected: session.selection.ids.clone(), active: session.active(), commits: session.commits() }
    }

    fn is_current(&self, session: &lightcraft_engine::Session) -> bool {
        self.selected == session.selection.ids && self.active == session.active() && !self.is_stale(session)
    }

    /// A later edit, undo or redo (on any photo) owns the undo step: the slider may never replace it.
    fn is_stale(&self, session: &lightcraft_engine::Session) -> bool {
        !(self.commits == session.commits()
            && session.interaction.is_none()
            && session.redo.is_empty()
            && session
                .undo
                .last()
                .is_some_and(|entry| session.presets.iter().any(|preset| preset.id == self.id && entry.label == format!("Preset: {}", preset.name))))
    }
}

/// The preset each photo was given, with its settings right after: the panel marks it while the
/// photo still has exactly those settings, so it is still marked on coming back to the photo
/// (issue #2392) and drops once the photo is edited further.
#[derive(Clone, Default)]
struct AppliedByPhoto(std::collections::HashMap<lightcraft_catalog::PhotoId, (String, Arc<lightcraft_develop::DevelopSettings>)>);

impl AppliedByPhoto {
    const ID: &'static str = "preset-applied-by-photo";

    /// Remember `preset` on the photos `preset.apply` just changed (the selection, else the active
    /// photo; an active photo outside the selection wasn't changed and isn't marked).
    fn remember(ctx: &egui::Context, session: &lightcraft_engine::Session, preset: &str) {
        let id = egui::Id::new(Self::ID);
        let mut map: Self = ctx.data(|d| d.get_temp(id)).unwrap_or_default();
        for photo in session.selected() {
            if let Some(d) = session.develop_of(photo) {
                map.0.insert(photo, (preset.to_string(), d));
            }
        }
        ctx.data_mut(|d| d.insert_temp(id, map));
    }

    /// The preset to mark for `photo`: the one it was given, while its settings are unchanged since.
    fn of(ctx: &egui::Context, session: &lightcraft_engine::Session, photo: Option<lightcraft_catalog::PhotoId>) -> Option<String> {
        let photo = photo?;
        let map: Self = ctx.data(|d| d.get_temp(egui::Id::new(Self::ID)))?;
        let (preset, then) = map.0.get(&photo)?;
        let now = session.develop_of(photo)?;
        (Arc::ptr_eq(then, &now) || **then == *now).then(|| preset.clone())
    }
}

/// The Presets panel: the Favorites section of starred presets on top (issue #1999), then the
/// groups of built-in and user presets, each row applying its preset on click.
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
    let more = icon_button(&mut hdr, "presetMore", Icon::More, vec2(28.0, 28.0), false, true, crate::i18n::tr("More preset options"));
    egui::Popup::menu(&more).show(|ui| {
        let r = crate::widgets::check(ui, &mut app.ui.preset_thumbs, crate::i18n::tr("Show Thumbnails"));
        register(ui.ctx(), "presetMenu:thumbnails", r.rect);
        let r = crate::widgets::check(ui, &mut app.ui.show_hidden_presets, crate::i18n::tr("Show Hidden Presets"));
        register(ui.ctx(), "presetMenu:showHidden", r.rect);
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
    let last: Option<AppliedPreset> = ui.data(|d| d.get_temp(amt_id));
    if let Some(last) = last.clone().filter(|last| last.is_current(&app.session)) {
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
        let out = slider(ui, &spec, last.amount, true, None);
        if let Some(v) = out.value
            && app.run("edit.undo", json!({})).is_ok()
        {
            // Only replace the most recent preset; never undo intervening edits.
            if app.run("preset.apply", json!({"id": &last.id, "amount": v})).is_ok() {
                AppliedByPhoto::remember(ui.ctx(), &app.session, &last.id);
                ui.data_mut(|d| d.insert_temp(amt_id, AppliedPreset::new(&app.session, last.id, v)));
            } else {
                let _ = app.run("edit.redo", json!({}));
                ui.data_mut(|d| d.remove::<AppliedPreset>(amt_id));
            }
        }
        divider(ui);
    } else if last.is_some_and(|last| last.is_stale(&app.session)) {
        // kept across a switch to another photo and back (the slider returns with it); gone once
        // a later edit, undo or redo owns the step
        ui.data_mut(|d| d.remove::<AppliedPreset>(amt_id));
    }
    let active = app.session.active();
    let marked = AppliedByPhoto::of(ui.ctx(), &app.session, active);
    let current = active.and_then(|id| app.session.develop_of(id)).map(|d| (*d).clone());
    if compact {
        strip(app, ui, active, current.as_ref(), marked.as_deref());
        return;
    }
    let thumbs = app.ui.preset_thumbs && active.is_some();
    let row_h = if thumbs { 54.0 } else { 26.0 };
    let groups = grouped(ui.ctx(), &app.session, app.ui.show_hidden_presets);
    let scroll = egui::ScrollArea::vertical().id_salt("presets-scroll").auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        // a group header dragged up or down puts the group elsewhere (`preset.moveGroup`)
        let drag_id = egui::Id::new("preset-group-drag");
        let dragging: Option<String> = ui.data(|d| d.get_temp(drag_id));
        let mut headers: Vec<(String, Rect)> = Vec::with_capacity(groups.len());
        // Only the rows in view are laid out, drawn and given thumbnails: a Lightroom collection
        // imported whole is 10 000+ presets, and building every row each frame took seconds.
        let visible = ui.clip_rect().expand(row_h);
        for grp in groups.iter() {
            let (g, builtin, hidden, favorites) = (grp.name.clone(), grp.builtin, grp.hidden, grp.favorites);
            // the Favorites section opens and closes apart from a user group named "Favorites"
            let open_id = egui::Id::new(("preset-group", favorites, &g));
            let open: bool = ui.data(|d| d.get_temp(open_id)).unwrap_or(true);
            // (the Favorites section is not a group: it can't be dragged to another place)
            let (r, resp) =
                ui.allocate_exact_size(vec2(ui.available_width(), 32.0), if favorites { Sense::click() } else { Sense::click_and_drag() });
            crate::widgets::announce(&resp, g.to_string());
            // outside the `presetGroup:` ids, which a user group of any name can take
            register(ui.ctx(), if favorites { "presetFavorites".to_string() } else { format!("presetGroup:{g}") }, r);
            if !favorites {
                headers.push((g.clone(), r));
            }
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
                Rect::from_center_size(pos2(r.left() + 22.0, r.center().y), vec2(16.0, 16.0)),
                if open { Icon::ChevronDown } else { Icon::ChevronRight },
                t.text_label,
            );
            ui.painter().text(
                pos2(r.left() + 36.0, r.center().y),
                Align2::LEFT_CENTER,
                crate::i18n::builtin_label(&g, builtin),
                t.semibold(13.0),
                if hidden { t.text_dim } else { t.text_label },
            );
            if resp.clicked() {
                ui.data_mut(|d| d.insert_temp(open_id, !open));
            }
            if !favorites {
                crate::widgets::context_menu(&resp, |ui| group_menu(app, ui, &g, builtin, hidden));
            }
            if !open {
                continue;
            }
            let (first, end) = rows_in_view(ui.cursor().top(), row_h, grp.items.len(), visible);
            if first > 0 {
                ui.allocate_space(vec2(ui.available_width(), first as f32 * row_h));
            }
            for &i in grp.items.get(first..end).unwrap_or_default() {
                let Some(pr) = app.session.presets.get(i).cloned() else { continue };
                let (pid, name, fav) = (pr.id.clone(), crate::i18n::builtin_label(&pr.name, pr.builtin).to_string(), pr.favorite);
                let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click());
                crate::widgets::announce(&resp, name.clone());
                // a starred preset is listed twice: in Favorites and in its group
                register(ui.ctx(), if favorites { format!("presetFavorite:{pid}") } else { format!("preset:{pid}") }, r);
                let is_last = marked.as_deref() == Some(pid.as_str());
                if is_last {
                    register(ui.ctx(), "presetApplied", r);
                    ui.painter().rect_filled(r, 0.0, t.tool_active);
                } else if resp.hovered() {
                    ui.painter().rect_filled(r, 0.0, t.hover.gamma_multiply(0.7));
                }
                // as Apply stores it (an Upright correction settled with the photo)
                let look = if thumbs || resp.hovered() {
                    active.and_then(|id| app.session.preset_look(id, &pr, 1.0)).or_else(|| current.as_ref().map(|d| pr.apply(d, 1.0)))
                } else {
                    None
                };
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
                let dim = hidden || app.session.hidden_presets.presets.contains(&pid);
                ui.painter().text(pos2(text_x, r.center().y), Align2::LEFT_CENTER, &name, t.font(13.0), if dim { t.text_dim } else { t.text_label });
                if fav {
                    crate::rating::paint_star(
                        ui.painter(),
                        Rect::from_center_size(pos2(r.right() - 20.0, r.center().y), vec2(16.0, 16.0)),
                        true,
                        t.star,
                    );
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
                if resp.clicked() && app.run("preset.apply", json!({"id": pid, "amount": 100})).is_ok() {
                    AppliedByPhoto::remember(ui.ctx(), &app.session, &pid);
                    ui.data_mut(|d| d.insert_temp(amt_id, AppliedPreset::new(&app.session, pid.clone(), 100.0)));
                    ui.data_mut(|d| d.remove::<(lightcraft_catalog::PhotoId, Preset, String)>(egui::Id::new("last-preset-hover")));
                    app.toast(ui.ctx(), crate::i18n::tr_format!("Preset: {name}", name = name));
                }
                crate::widgets::context_menu(&resp, |ui| preset_menu(app, ui, &pr, &name));
            }
            if end < grp.items.len() {
                ui.allocate_space(vec2(ui.available_width(), (grp.items.len() - end) as f32 * row_h));
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
            let settings = app.session.preset_look(id, &saved_preset, 1.0).unwrap_or_else(|| saved_preset.apply(curr, 1.0));
            app.hover_preview = Some(HoverPreview { label: saved_label, settings });
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
    let mut groups = grouped_presets(app);
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
                let hidden = app.session.hidden_presets.groups.contains(g);
                crate::widgets::context_menu(&chip, |ui| group_menu(app, ui, g, builtin, hidden));
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
                    && let Some(job) = {
                        // as Apply stores it (an Upright correction settled with the photo)
                        let look = app.session.preset_look(id, &pr, 1.0).unwrap_or_else(|| pr.apply(cur, 1.0));
                        app.session.variant_job(id, &look, THUMB_EDGE)
                    }
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
                let color = if on && !app.session.hidden_presets.hides(&pr) { t.text } else { t.text_dim };
                let mut job = egui::text::LayoutJob::simple(name.clone(), t.font(12.0), color, TILE);
                job.wrap.max_rows = 1;
                job.wrap.overflow_character = Some('…');
                let label = ui.painter().layout_job(job);
                ui.painter().galley(pos2(r.center().x - label.size().x.min(TILE) / 2.0, img.bottom() + 6.0), label, color);
                if resp.is_pointer_button_down_on() {
                    ui.painter().rect_filled(img, round, Color32::from_black_alpha(70));
                }
                if resp.clicked() {
                    if app.run("preset.apply", json!({"id": pr.id, "amount": 100})).is_ok() {
                        AppliedByPhoto::remember(ui.ctx(), &app.session, &pr.id);
                    }
                    ui.data_mut(|d| d.insert_temp(egui::Id::new("preset-amount"), (pr.id.clone(), 100.0)));
                    app.toast(ui.ctx(), crate::i18n::tr_format!("Preset: {name}", name = name));
                }
                crate::widgets::context_menu(&resp, |ui| preset_menu(app, ui, &pr, &name));
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
    let hidden = app.session.hidden_presets.presets.contains(&pid);
    if ui.button(crate::i18n::tr(if hidden { "Show Preset" } else { "Hide Preset" })).clicked() {
        hide(app, ui.ctx(), "preset.hide", json!({"id": pid, "hidden": !hidden}), name);
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
    crate::widgets::menu_button(ui, crate::i18n::tr("Move to Group"), |ui| {
        let groups = grouped(ui.ctx(), &app.session, true);
        for g in groups.iter().filter(|x| x.user && x.name != group).map(|x| &x.name) {
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

/// Hide a preset or group, or show it again (`preset.hide`, `preset.hideGroup`); one hidden from a panel that
/// doesn't show hidden presets says where it went.
fn hide(app: &mut LightcraftApp, ctx: &egui::Context, command: &str, params: serde_json::Value, name: &str) {
    match app.run(command, params) {
        Ok(r) if r["hidden"] == true && !app.ui.show_hidden_presets => {
            app.toast(ctx, crate::i18n::tr_format!("Hid “{name}”. Show Hidden Presets in the panel's menu brings it back.", name = name))
        }
        Ok(_) => {}
        Err(e) => app.toast_error(ctx, e),
    }
}

/// A group's menu (a right click on its header, a long press on its chip): hide it or show it again,
/// export it; a group of the user's own presets can also be renamed and deleted.
fn group_menu(app: &mut LightcraftApp, ui: &mut egui::Ui, g: &str, builtin: bool, hidden: bool) {
    let label = crate::i18n::builtin_label(g, builtin).to_string();
    if ui.button(crate::i18n::tr(if hidden { "Show Group" } else { "Hide Group" })).clicked() {
        hide(app, ui.ctx(), "preset.hideGroup", json!({"group": g, "hidden": !hidden}), &label);
    }
    if !builtin && ui.button(crate::i18n::tr("Rename Group…")).clicked() {
        crate::panels::dialogs::prompt(app, "Rename Preset Group", "Group name", g, "preset.renameGroup", json!({"group": g}), "name");
    }
    ui.separator();
    if ui.button(crate::i18n::tr("Export Group…")).clicked() {
        let _ = app.run("file.exportPresets", json!({"group": g}));
    }
    if !builtin && ui.button(crate::i18n::tr("Delete Group…")).clicked() {
        let count = app.session.presets.iter().filter(|p| p.group == g && !p.builtin).count();
        app.ui.dialog = Some(crate::state::Dialog::DeletePresetGroup { group: g.to_string(), count });
    }
}

/// A preset group as the panel lists it: indices into the session's presets.
struct Group {
    name: String,
    /// Holds a stock preset: a stock group (shown in the UI language), even when user presets
    /// share its name; a group of user presets keeps its name.
    builtin: bool,
    /// Holds a user preset (a Move to Group target).
    user: bool,
    /// The user hid it (`preset.hideGroup`).
    hidden: bool,
    /// The Favorites section on top (issue #1999): the starred presets, each also in its own group.
    favorites: bool,
    items: Vec<usize>,
}

/// The presets by group (groups in the user's order, [`lightcraft_engine::Session::preset_groups`];
/// presets in list order), without the hidden ones unless `show_hidden`; rebuilt only when the list (its
/// generation), the group order or what is hidden changed.
fn grouped(ctx: &egui::Context, session: &lightcraft_engine::Session, show_hidden: bool) -> Arc<Vec<Group>> {
    type Cached = (u64, Vec<String>, lightcraft_engine::presets::Hidden, Arc<Vec<Group>>);
    let id = egui::Id::new(("presets-grouped", show_hidden));
    let presets = &session.presets;
    let hide = &session.hidden_presets;
    if let Some((generation, order, hidden, groups)) = ctx.data(|d| d.get_temp::<Cached>(id))
        && generation == presets.generation()
        && order == session.preset_group_order
        && hidden == *hide
    {
        return groups;
    }
    let mut by_name: std::collections::HashMap<&str, Group> = std::collections::HashMap::new();
    for (i, p) in presets.iter().enumerate() {
        let g = by_name.entry(&p.group).or_insert_with(|| Group {
            name: p.group.clone(),
            builtin: false,
            user: false,
            hidden: hide.groups.contains(&p.group),
            favorites: false,
            items: Vec::new(),
        });
        g.builtin |= p.builtin;
        g.user |= !p.builtin;
        if show_hidden || !hide.hides(p) {
            g.items.push(i);
        }
    }
    let order = session.preset_groups();
    // starred presets first, in their own section, as Profiles does
    let starred: Vec<usize> = presets.iter().enumerate().filter(|(_, p)| p.favorite && (show_hidden || !hide.hides(p))).map(|(i, _)| i).collect();
    let favorites =
        (!starred.is_empty()).then(|| Group { name: "Favorites".into(), builtin: true, user: false, hidden: false, favorites: true, items: starred });
    let groups = Arc::new(
        favorites.into_iter().chain(order.iter().filter_map(|g| by_name.remove(g.as_str())).filter(|g| !g.items.is_empty())).collect::<Vec<_>>(),
    );
    ctx.data_mut(|d| d.insert_temp(id, (presets.generation(), session.preset_group_order.clone(), hide.clone(), groups.clone())));
    groups
}

/// The presets as the rows of a picker (a combo box's list), in list order with a header row
/// wherever the group changes. Only the rows in view are built (10 000+ presets made an open
/// picker take seconds a frame). Returns the id of the preset clicked.
pub fn picker_rows(ui: &mut egui::Ui, salt: &str, presets: &PresetList, selected: Option<&str>) -> Option<String> {
    let id = egui::Id::new("preset-picker-rows");
    let rows = match ui.ctx().data(|d| d.get_temp::<(u64, Arc<Vec<PickRow>>)>(id)) {
        Some((generation, rows)) if generation == presets.generation() => rows,
        _ => {
            let mut rows = Vec::with_capacity(presets.len() + 16);
            let mut group: Option<&str> = None;
            for (i, p) in presets.iter().enumerate() {
                if group != Some(p.group.as_str()) {
                    group = Some(&p.group);
                    rows.push(PickRow::Header(crate::i18n::builtin_label(&p.group, p.builtin).to_string()));
                }
                rows.push(PickRow::Preset(i));
            }
            let rows = Arc::new(rows);
            ui.ctx().data_mut(|d| d.insert_temp(id, (presets.generation(), rows.clone())));
            rows
        }
    };
    let row_h = ui.spacing().interact_size.y;
    let mut out = None;
    egui::ScrollArea::vertical().id_salt(("preset-picker", salt)).max_height(300.0).auto_shrink([false, true]).show_rows(
        ui,
        row_h,
        rows.len(),
        |ui, range| {
            for row in rows.get(range).unwrap_or_default() {
                let size = vec2(ui.available_width(), row_h);
                match row {
                    PickRow::Header(g) => {
                        ui.add_sized(size, egui::Label::new(egui::RichText::new(g).size(10.5).weak()));
                    }
                    PickRow::Preset(i) => {
                        let Some(p) = presets.get(*i) else { continue };
                        let label = crate::i18n::builtin_label(&p.name, p.builtin);
                        if ui.add_sized(size, egui::Button::selectable(selected == Some(p.id.as_str()), label)).clicked() {
                            out = Some(p.id.clone());
                        }
                    }
                }
            }
        },
    );
    out
}

#[derive(Clone)]
enum PickRow {
    /// A group's name (in the UI language for stock groups).
    Header(String),
    /// An index into the session's presets.
    Preset(usize),
}

/// The rows `first..end` of a block of `n` rows `row_h` high starting at `top` that meet
/// `visible`.
fn rows_in_view(top: f32, row_h: f32, n: usize, visible: Rect) -> (usize, usize) {
    if row_h <= 0.0 || !top.is_finite() {
        return (0, n);
    }
    // `as usize` saturates (negative and NaN give 0)
    let first = (((visible.top() - top) / row_h).floor() as usize).min(n);
    let end = (((visible.bottom() - top) / row_h).ceil() as usize).clamp(first, n);
    (first, end)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amount_slider_does_not_undo_a_later_edit() {
        let mut session = lightcraft_engine::Session::with_demo();
        let pid = "lc.golden-hour";
        session.execute("preset.apply", &json!({"id": pid})).unwrap();
        let state = AppliedPreset::new(&session, pid.into(), 100.0);
        assert!(state.is_current(&session));

        session.execute("develop.set", &json!({"values": {"light.exposure": 0.75}})).unwrap();
        assert!(!state.is_current(&session), "a newer edit owns the undo step");
    }

    #[test]
    fn amount_slider_does_not_undo_on_another_photo() {
        let mut session = lightcraft_engine::Session::with_demo();
        let pid = "lc.golden-hour";
        session.execute("preset.apply", &json!({"id": pid})).unwrap();
        let state = AppliedPreset::new(&session, pid.into(), 100.0);
        let active = session.active();
        let other = session.visible().iter().copied().find(|id| Some(*id) != active).unwrap();
        session.execute("library.select", &json!({"ids": [other.0]})).unwrap();
        assert!(!state.is_current(&session), "the preset belongs to the earlier selection");
    }
}
