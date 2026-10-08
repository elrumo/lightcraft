//! Phone-sized building blocks for the compact layout (`compact.rs`): pages that cover the screen
//! and slide up from the bottom (every dialog, the albums list, the command list), and action
//! sheets at the bottom of the screen in place of the desktop's popup menus. Layout and behaviour
//! follow Lightroom's mobile app; the drawing and the words are our own.

use egui::{Align2, Color32, CornerRadius, Id, Rect, Sense, Stroke, UiBuilder, pos2, vec2};
use serde_json::Value;

use crate::LightcraftApp;
use crate::icons::Icon;
use crate::theme::Tokens;

/// How long a sheet takes to slide in (s).
const SLIDE_S: f32 = 0.28;
/// Height of a page's bar and of an action row: a comfortable touch target (Apple's minimum is 44).
pub const ROW_H: f32 = 52.0;
/// Action sheets are no wider than this (iPad).
const ACTIONS_MAX_W: f32 = 480.0;

/// The slide-in progress of the sheet `id`, eased: 0 = off screen, 1 = in place. Call [`hidden`]
/// on frames when it isn't shown, so that the next opening slides in again.
fn slide(ctx: &egui::Context, id: Id) -> f32 {
    ctx.animate_bool_with_time_and_easing(id.with("slide"), true, SLIDE_S, egui::emath::easing::cubic_out)
}

/// The sheet `id` isn't on screen this frame (its next opening slides in from the bottom).
pub fn hidden(ctx: &egui::Context, id: &str) {
    let _ = ctx.animate_bool_with_time(Id::new(("lc-sheet", id)).with("slide"), false, 0.0);
}

/// What was tapped in a page's bar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Bar {
    pub cancel: bool,
    pub ok: bool,
}

/// A text button in a bar, `align`ed to `at` (`button:<id>`); true when tapped.
fn bar_button(ui: &mut egui::Ui, id: &str, label: &str, at: egui::Pos2, align: Align2, strong: bool, enabled: bool) -> bool {
    let t = Tokens::get(ui.ctx());
    let font = if strong { t.semibold(16.0) } else { t.font(16.0) };
    let galley = ui.painter().layout_no_wrap(crate::i18n::tr(label).to_string(), font, t.accent);
    let size = vec2(galley.size().x + 24.0, ROW_H);
    let left = if align == Align2::LEFT_CENTER { at.x } else { at.x - size.x };
    let r = Rect::from_min_size(pos2(left, at.y - size.y / 2.0), size);
    let resp = ui.interact(r, Id::new(("lc-bar", id)), if enabled { Sense::click() } else { Sense::hover() });
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, crate::i18n::tr(label)));
    crate::widgets::register(ui.ctx(), format!("button:{id}"), r);
    let color = if !enabled {
        t.text_disabled
    } else if resp.is_pointer_button_down_on() {
        t.accent.gamma_multiply(0.6)
    } else {
        t.accent
    };
    ui.painter().galley(r.center() - galley.size() / 2.0, galley, color);
    resp.clicked()
}

/// A page covering the screen, sliding up from the bottom, as a phone shows dialogs and lists: a
/// bar with `cancel` on the left, `title` in the middle and the `ok` action (if any) on the right,
/// then `body`, which scrolls. It stays above the on-screen keyboard (the host counts the keyboard
/// in the safe area). Widget ids: `sheet:<id>`, `button:sheetCancel`, `button:sheetOk`.
pub fn page(ctx: &egui::Context, id: &str, title: &str, cancel: &str, ok: Option<(&str, bool)>, body: impl FnOnce(&mut egui::Ui)) -> Bar {
    let t = Tokens::get(ctx);
    let sid = Id::new(("lc-sheet", id));
    let p = slide(ctx, sid);
    let content = ctx.content_rect();
    let screen = ctx.viewport_rect();
    let drop = (1.0 - p) * (screen.bottom() - content.top());
    // dim what is behind while it slides in, and keep taps off it
    egui::Area::new(sid.with("dim")).order(egui::Order::Middle).fixed_pos(screen.min).show(ctx, |ui| {
        ui.painter().rect_filled(screen, 0.0, Color32::from_black_alpha((t.scrim as f32 * p) as u8));
        let _ = ui.allocate_rect(screen, Sense::click());
    });
    let mut bar = Bar::default();
    let top = content.top() + 8.0 + drop;
    egui::Area::new(sid).order(egui::Order::Foreground).constrain(false).fixed_pos(pos2(content.left(), top)).show(ctx, |ui| {
        let w = content.width();
        // the sheet reaches the bottom edge of the screen (behind the home indicator); its contents
        // stop at the safe area / keyboard
        let full = Rect::from_min_max(pos2(content.left(), top), pos2(content.right(), screen.bottom().max(top + ROW_H)));
        let _ = ui.allocate_rect(full, Sense::hover());
        crate::widgets::register(ctx, format!("sheet:{id}"), full);
        crate::widgets::register(ctx, "dialog:window", full);
        ui.painter().rect_filled(full, CornerRadius { nw: 12, ne: 12, sw: 0, se: 0 }, t.chrome);
        let bar_r = Rect::from_min_size(full.min, vec2(w, ROW_H));
        let galley = ui.painter().layout(crate::i18n::tr(title).to_string(), t.semibold(17.0), t.text, (w - 200.0).max(80.0));
        ui.painter().galley(bar_r.center() - galley.size() / 2.0, galley, t.text);
        bar.cancel = bar_button(ui, "sheetCancel", cancel, bar_r.left_center() + vec2(4.0, 0.0), Align2::LEFT_CENTER, false, true);
        if let Some((label, enabled)) = ok {
            bar.ok = bar_button(ui, "sheetOk", label, bar_r.right_center() - vec2(4.0, 0.0), Align2::RIGHT_CENTER, true, enabled);
        }
        ui.painter().hline(full.x_range(), bar_r.bottom(), Stroke::new(1.0, t.divider));
        let body_r =
            Rect::from_min_max(pos2(full.left(), bar_r.bottom() + 1.0), pos2(full.right(), (content.bottom() + drop).max(bar_r.bottom() + 1.0)));
        let mut child = ui.new_child(UiBuilder::new().max_rect(body_r).id_salt(sid.with("body")));
        child.set_clip_rect(body_r.intersect(ui.clip_rect()));
        egui::ScrollArea::vertical().id_salt(sid.with("scroll")).auto_shrink([false, false]).show(&mut child, |ui| {
            egui::Frame::NONE.inner_margin(egui::Margin { left: 16, right: 16, top: 12, bottom: 24 }).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 10.0;
                body(ui);
            });
        });
    });
    bar
}

fn open_id() -> Id {
    Id::new("lc-actions-open")
}

/// Open the action sheet `id` at the bottom of the screen (one menu or sheet at a time).
pub fn open_actions(ctx: &egui::Context, id: &str) {
    ctx.data_mut(|d| d.insert_temp(open_id(), (id.to_string(), None::<Rect>)));
}

/// Open `id` as a pull-down menu under (or over) `anchor`, the button that opened it, as iOS
/// shows the menus of "…" and + buttons.
pub fn open_menu(ctx: &egui::Context, id: &str, anchor: Rect) {
    ctx.data_mut(|d| d.insert_temp(open_id(), (id.to_string(), Some(anchor))));
}

/// Close whichever menu or action sheet is open.
pub fn close_actions(ctx: &egui::Context) {
    ctx.data_mut(|d| d.remove::<(String, Option<Rect>)>(open_id()));
}

fn opened(ctx: &egui::Context, id: &str) -> Option<Option<Rect>> {
    ctx.data(|d| d.get_temp::<(String, Option<Rect>)>(open_id())).filter(|(o, _)| o == id).map(|(_, a)| a)
}

/// The menu or action sheet `id` is open.
pub fn actions_open(ctx: &egui::Context, id: &str) -> bool {
    opened(ctx, id).is_some()
}

/// The menu `id`, when it's open (`open_menu` / `open_actions`): rows (`row`, `row_checked`) in a
/// rounded panel over a dimmed screen — a pull-down under its button, or an action sheet at the
/// bottom with Cancel under it. A tap outside, on Cancel or on any row closes it.
pub fn actions(ctx: &egui::Context, id: &str, title: Option<&str>, add: impl FnOnce(&mut egui::Ui)) {
    let sid = Id::new(("lc-sheet", id));
    let Some(anchor) = opened(ctx, id) else {
        let _ = ctx.animate_bool_with_time(sid.with("slide"), false, 0.0);
        return;
    };
    let t = Tokens::get(ctx);
    let p = slide(ctx, sid);
    let content = ctx.content_rect();
    let screen = ctx.viewport_rect();
    let mut close = false;
    egui::Area::new(sid.with("dim")).order(egui::Order::Middle).fixed_pos(screen.min).show(ctx, |ui| {
        let alpha = t.scrim as f32 * if anchor.is_some() { 0.4 } else { 0.8 };
        ui.painter().rect_filled(screen, 0.0, Color32::from_black_alpha((alpha * p) as u8));
        if ui.allocate_rect(screen, Sense::click()).clicked() {
            close = true;
        }
    });
    let h_id = sid.with("height");
    let h = ctx.data(|d| d.get_temp::<f32>(h_id)).unwrap_or(320.0);
    let (w, pos) = match anchor {
        Some(a) => {
            // a pull-down: under its button, flush with the nearer screen edge; above it when
            // there's no room below
            let w = 290.0_f32.min(content.width() - 16.0);
            let x = if a.center().x > content.center().x { a.right() - w } else { a.left() };
            let x = x.clamp(content.left() + 8.0, (content.right() - w - 8.0).max(content.left() + 8.0));
            let below = a.bottom() + 6.0;
            let y = if below + h <= content.bottom() - 8.0 { below } else { (a.top() - 6.0 - h).max(content.top() + 8.0) };
            (w, pos2(x, y))
        }
        None => {
            let w = (content.width() - 16.0).min(ACTIONS_MAX_W);
            (w, pos2(content.center().x - w / 2.0, content.bottom() - 8.0 - h + (1.0 - p) * (h + 40.0)))
        }
    };
    let shown = egui::Area::new(sid).order(egui::Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
        if anchor.is_some() {
            ui.multiply_opacity(p);
        }
        ui.set_width(w);
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        let frame = egui::Frame::NONE.fill(t.cell_selected).corner_radius(13.0).shadow(egui::epaint::Shadow {
            offset: [0, 8],
            blur: 30,
            spread: 0,
            color: Color32::from_black_alpha(120),
        });
        frame.show(ui, |ui| {
            ui.set_width(w);
            if let Some(title) = title {
                let galley = ui.painter().layout(crate::i18n::tr(title).to_string(), t.font(13.0), t.text_dim, w - 32.0);
                let (r, _) = ui.allocate_exact_size(vec2(w, galley.size().y + 24.0), Sense::hover());
                ui.painter().galley(pos2(r.left() + 16.0, r.center().y - galley.size().y / 2.0), galley, t.text_dim);
                ui.painter().hline(r.x_range(), r.bottom() - 0.5, Stroke::new(1.0, t.divider));
            }
            add(ui);
        });
        if anchor.is_none() {
            ui.add_space(8.0);
            frame.show(ui, |ui| {
                let (r, resp) = ui.allocate_exact_size(vec2(w, ROW_H + 4.0), Sense::click());
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, crate::i18n::tr("Cancel")));
                crate::widgets::register(ctx, "button:actionsCancel", r);
                let color = if resp.is_pointer_button_down_on() { t.accent.gamma_multiply(0.6) } else { t.accent };
                ui.painter().text(r.center(), Align2::CENTER_CENTER, crate::i18n::tr("Cancel"), t.semibold(17.0), color);
                if resp.clicked() {
                    close = true;
                }
            });
        }
    });
    crate::widgets::register(ctx, format!("actions:{id}"), shown.response.rect);
    ctx.data_mut(|d| d.insert_temp(h_id, shown.response.rect.height()));
    if close || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        close_actions(ctx);
    }
}

/// A row of a menu or list page (`button:<id>`), as iOS draws menu items: the label on the left
/// (a check mark before it when `checked` is `Some(true)`), the icon on the right. True when
/// tapped; a tap closes the menu.
pub fn row_checked(ui: &mut egui::Ui, id: &str, icon: Option<Icon>, label: &str, enabled: bool, checked: Option<bool>) -> bool {
    let t = Tokens::get(ui.ctx());
    let w = ui.available_width();
    let (r, resp) = ui.allocate_exact_size(vec2(w, 46.0), if enabled { Sense::click() } else { Sense::hover() });
    resp.widget_info(|| match checked {
        Some(c) => egui::WidgetInfo::selected(egui::WidgetType::Checkbox, enabled, c, label),
        None => egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label),
    });
    crate::widgets::register(ui.ctx(), format!("button:{id}"), r);
    let p = ui.painter();
    if resp.is_pointer_button_down_on() {
        p.rect_filled(r, 0.0, t.hover);
    }
    let color = if enabled { t.text } else { t.text_disabled };
    let mut x = r.left() + 16.0;
    if checked.is_some() {
        if checked == Some(true) {
            crate::icons::paint(p, Rect::from_center_size(pos2(x + 7.0, r.center().y), vec2(15.0, 15.0)), Icon::Check, color);
        }
        x += 26.0;
    }
    let right = if icon.is_some() { r.right() - 48.0 } else { r.right() - 16.0 };
    let galley = p.layout(label.to_string(), t.font(17.0), color, (right - x).max(40.0));
    p.galley(pos2(x, r.center().y - galley.size().y / 2.0), galley, color);
    if let Some(icon) = icon {
        crate::icons::paint(p, Rect::from_center_size(pos2(r.right() - 28.0, r.center().y), vec2(20.0, 20.0)), icon, color);
    }
    p.hline(r.x_range(), r.bottom() - 0.5, Stroke::new(0.5, t.divider));
    let clicked = resp.clicked();
    if clicked {
        close_actions(ui.ctx());
    }
    clicked
}

/// [`row_checked`] without a check mark.
pub fn row(ui: &mut egui::Ui, id: &str, icon: Option<Icon>, label: &str, enabled: bool) -> bool {
    row_checked(ui, id, icon, label, enabled, None)
}

/// A thicker gap between groups of menu rows, as iOS separates menu sections.
pub fn row_gap(ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 8.0), Sense::hover());
    ui.painter().rect_filled(r, 0.0, t.chrome);
}

/// Every menu command, flattened: (where it lives, label, id, params, enabled, checked).
fn all_commands(app: &LightcraftApp) -> Vec<(String, String, String, Value, bool, Option<bool>)> {
    fn walk(path: &str, nodes: &[crate::menubar::MenuNode], out: &mut Vec<(String, String, String, Value, bool, Option<bool>)>, depth: usize) {
        for n in nodes {
            match n {
                crate::menubar::MenuNode::Item { id, params, label, enabled, checked, .. } => {
                    let shown = crate::menubar::display_item_label(id, params, label).to_string();
                    out.push((path.to_string(), shown, id.clone(), params.clone(), *enabled, *checked));
                }
                crate::menubar::MenuNode::Submenu { label, children } if depth < 6 => {
                    walk(&format!("{path} › {}", crate::i18n::tr(label)), children, out, depth + 1);
                }
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    for (title, items) in crate::menubar::menu_bar(app) {
        walk(crate::i18n::tr(&title), &items, &mut out, 0);
    }
    out
}

/// "All Commands": every command the menus have, as a searchable list (the phone layout has no
/// menu bar). Open with `app.ui.all_commands = true`.
pub fn all_commands_page(app: &mut LightcraftApp, ctx: &egui::Context) {
    if !app.ui.all_commands {
        hidden(ctx, "allCommands");
        return;
    }
    let query_id = Id::new("lc-all-commands-query");
    let mut query = ctx.data(|d| d.get_temp::<String>(query_id)).unwrap_or_default();
    let commands = all_commands(app);
    let mut run: Option<(String, Value)> = None;
    let bar = page(ctx, "allCommands", "All Commands", "Close", None, |ui| {
        let r = ui.add(egui::TextEdit::singleline(&mut query).hint_text(crate::i18n::tr("Search")).desired_width(f32::INFINITY));
        crate::widgets::register(ui.ctx(), "field:allCommandsSearch", r.rect);
        let q = query.to_lowercase();
        let t = Tokens::get(ui.ctx());
        let mut section = String::new();
        ui.spacing_mut().item_spacing.y = 0.0;
        for (path, label, id, params, enabled, checked) in &commands {
            if !q.is_empty() && !label.to_lowercase().contains(&q) && !path.to_lowercase().contains(&q) {
                continue;
            }
            if *path != section {
                section = path.clone();
                ui.add_space(10.0);
                ui.label(egui::RichText::new(path).color(t.text_dim).size(12.0));
            }
            let key = crate::menubar::MenuNode::key(id, params);
            if row_checked(ui, &format!("cmd:{key}"), None, label, *enabled, *checked) {
                run = Some((id.clone(), params.clone()));
            }
        }
    });
    ctx.data_mut(|d| d.insert_temp(query_id, query));
    if let Some((id, params)) = run {
        app.ui.all_commands = false;
        if let Err(e) = crate::menubar::run_item(app, &id, params) {
            app.toast(ctx, e);
        }
    } else if bar.cancel {
        app.ui.all_commands = false;
    }
}
