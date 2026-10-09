//! Phone-sized building blocks for the compact layout (`compact.rs`): pages that cover the screen
//! and slide up from the bottom (every dialog, the albums list, the command list), and action
//! sheets at the bottom of the screen in place of the desktop's popup menus, and the text fields'
//! edit menu (Cut, Copy, Paste: the touch keyboard has no shortcuts). Layout and behaviour
//! follow Lightroom's mobile app; the drawing and the words are our own.

use egui::{Align2, Color32, CornerRadius, Id, Rect, Sense, Stroke, UiBuilder, pos2, vec2};
use serde_json::Value;

use crate::LightcraftApp;
use crate::icons::Icon;
use crate::theme::Tokens;

/// How long a sheet takes to slide in (s); it leaves a little faster.
const SLIDE_S: f32 = 0.28;
const LEAVE_S: f32 = 0.2;
/// Height of a page's bar and of an action row: a comfortable touch target (Apple's minimum is 44).
pub const ROW_H: f32 = 52.0;
/// Action sheets are no wider than this (iPad).
const ACTIONS_MAX_W: f32 = 480.0;

/// The slide progress of the sheet `id`, eased: 0 = off screen, 1 = in place. It follows `open`, so
/// a sheet slides out the way it slid in (the same curve backwards: it starts slowly and picks up).
/// Call it, or [`hidden`], every frame: a sheet first seen open appears at once, not sliding in.
fn slide(ctx: &egui::Context, id: Id, open: bool) -> f32 {
    ctx.animate_bool_with_time_and_easing(id.with("slide"), open, if open { SLIDE_S } else { LEAVE_S }, egui::emath::easing::cubic_out)
}

/// The sheet `id` isn't on screen this frame and has no exit to play (its next opening slides in
/// from the bottom).
pub fn hidden(ctx: &egui::Context, id: &str) {
    let _ = ctx.animate_bool_with_time(Id::new(("lc-sheet", id)).with("slide"), false, 0.0);
}

/// 0 → 1 over `secs` (eased out) each time `key` changes, for fading in what `key` names (the view
/// shown, the tool in the sheet); 1 while it stays. The first call only notes the key, so nothing
/// fades in at start-up. Multiply a ui's opacity by it.
pub fn enter(ctx: &egui::Context, id: Id, key: impl std::hash::Hash + std::fmt::Debug, secs: f32) -> f32 {
    let key = Id::new(key).value();
    let seen = ctx.data(|d| d.get_temp::<u64>(id));
    if seen != Some(key) {
        ctx.data_mut(|d| d.insert_temp(id, key));
        if seen.is_some() {
            let _ = ctx.animate_bool_with_time(id.with("enter"), false, 0.0);
        }
    }
    ctx.animate_bool_with_time_and_easing(id.with("enter"), true, secs, egui::emath::easing::cubic_out)
}

/// What was tapped in a page's bar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Bar {
    pub cancel: bool,
    pub ok: bool,
    /// The page is off screen (shut, or done sliding out): nothing was drawn.
    pub gone: bool,
}

/// A text button in a bar, `align`ed to `at` (`button:<id>`); true when tapped.
/// An empty `label` draws nothing; one starting with "‹" is a back button (a chevron, then the
/// rest of the label), as iOS's navigation bars have.
fn bar_button(ui: &mut egui::Ui, id: &str, label: &str, at: egui::Pos2, align: Align2, strong: bool, enabled: bool) -> bool {
    if label.is_empty() {
        return false;
    }
    let (back, label) = match label.strip_prefix('‹') {
        Some(rest) => (true, rest),
        None => (false, label),
    };
    let t = Tokens::get(ui.ctx());
    let font = if strong { t.semibold(16.0) } else { t.font(16.0) };
    let galley = ui.painter().layout_no_wrap(crate::i18n::tr(label).to_string(), font, t.accent);
    let chevron = if back { 16.0 } else { 0.0 };
    let size = vec2(galley.size().x + 24.0 + chevron, ROW_H);
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
    if back {
        let c = pos2(r.left() + 6.0 + chevron / 2.0, r.center().y);
        crate::icons::paint(ui.painter(), Rect::from_center_size(c, vec2(22.0, 22.0)), Icon::ChevronLeft, color);
        ui.painter().galley(pos2(r.left() + 6.0 + chevron + 4.0, r.center().y - galley.size().y / 2.0), galley, color);
    } else {
        ui.painter().galley(r.center() - galley.size() / 2.0, galley, color);
    }
    resp.clicked()
}

/// What a page's bar holds at either end.
#[derive(Clone, Copy, Debug)]
pub enum Item<'a> {
    None,
    /// A text button; `true`: it can be tapped. A leading "‹" makes it a back button.
    Text(&'a str, bool),
    /// The page's action, in bold.
    Strong(&'a str, bool),
    /// An icon button (what a screen reader calls it; `true`: tinted with the accent colour).
    Icon(Icon, &'a str, bool),
    /// A text in an outlined capsule: a choice that applies to the page (Select All).
    Pill(&'a str),
}

/// A page's bar: its title, what is at each end, and the widget ids of those two (`button:<id>`).
#[derive(Clone, Copy, Debug)]
pub struct PageBar<'a> {
    pub title: &'a str,
    pub left: Item<'a>,
    pub right: Item<'a>,
    pub left_id: &'a str,
    pub right_id: &'a str,
}

/// One end of a page's bar, `align`ed to `at`; true when tapped.
fn bar_item(ui: &mut egui::Ui, id: &str, item: Item, at: egui::Pos2, align: Align2) -> bool {
    let t = Tokens::get(ui.ctx());
    let left_of = |w: f32| if align == Align2::LEFT_CENTER { at.x } else { at.x - w };
    match item {
        Item::None => false,
        Item::Text(label, enabled) => bar_button(ui, id, label, at, align, false, enabled),
        Item::Strong(label, enabled) => bar_button(ui, id, label, at, align, true, enabled),
        Item::Icon(icon, tip, accent) => {
            let size = vec2(48.0, ROW_H);
            let r = Rect::from_min_size(pos2(left_of(size.x), at.y - size.y / 2.0), size);
            let resp = ui.interact(r, Id::new(("lc-bar", id)), Sense::click());
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, crate::i18n::tr(tip)));
            crate::widgets::register(ui.ctx(), format!("button:{id}"), r);
            let ink = if accent { t.accent } else { t.text };
            let color = if resp.is_pointer_button_down_on() { ink.gamma_multiply(0.6) } else { ink };
            crate::icons::paint(ui.painter(), Rect::from_center_size(r.center(), vec2(22.0, 22.0)), icon, color);
            resp.clicked()
        }
        Item::Pill(label) => {
            let label = crate::i18n::tr(label);
            let galley = ui.painter().layout_no_wrap(label.to_string(), t.font(15.0), t.text);
            let size = vec2(galley.size().x + 28.0, 34.0);
            let r = Rect::from_min_size(pos2(left_of(size.x + 12.0) + 6.0, at.y - size.y / 2.0), size);
            let hit = r.expand2(vec2(6.0, (ROW_H - size.y) / 2.0));
            let resp = ui.interact(hit, Id::new(("lc-bar", id)), Sense::click());
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
            crate::widgets::register(ui.ctx(), format!("button:{id}"), hit);
            let p = ui.painter();
            if resp.is_pointer_button_down_on() {
                p.rect_filled(r, size.y / 2.0, t.hover);
            }
            p.rect_stroke(r, size.y / 2.0, Stroke::new(1.0, t.text_disabled), egui::StrokeKind::Inside);
            p.galley(r.center() - galley.size() / 2.0, galley, t.text);
            resp.clicked()
        }
    }
}

/// A page covering the screen, sliding up from the bottom, as a phone shows dialogs and lists: a
/// bar with `cancel` on the left, `title` in the middle and the `ok` action (if any) on the right,
/// then `body`, which scrolls. It stays above the on-screen keyboard (the host counts the keyboard
/// in the safe area). Widget ids: `sheet:<id>`, `button:sheetCancel`, `button:sheetOk`.
///
/// When `open` goes false the page slides back down, still drawn from `body` (so the caller keeps
/// what it showed until `gone`) with a deaf bar; call it every frame, or [`hidden`].
pub fn page(ctx: &egui::Context, id: &str, title: &str, cancel: &str, ok: Option<(&str, bool)>, open: bool, body: impl FnOnce(&mut egui::Ui)) -> Bar {
    let bar = PageBar {
        title,
        left: if cancel.is_empty() { Item::None } else { Item::Text(cancel, true) },
        right: ok.map_or(Item::None, |(label, enabled)| Item::Strong(label, enabled)),
        left_id: "sheetCancel",
        right_id: "sheetOk",
    };
    page_with(ctx, id, bar, open, body)
}

/// [`page`] with a bar of its own: icons or a capsule at its ends, and their widget ids. A tap on
/// the left one is `Bar::cancel`, on the right one `Bar::ok`. Pages stack: one shown after another
/// is drawn over it.
pub fn page_with(ctx: &egui::Context, id: &str, bar_spec: PageBar, open: bool, body: impl FnOnce(&mut egui::Ui)) -> Bar {
    let t = Tokens::get(ctx);
    let sid = Id::new(("lc-sheet", id));
    let p = slide(ctx, sid, open);
    if p <= 0.0 {
        return Bar { gone: true, ..Bar::default() };
    }
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
        let galley = ui.painter().layout(crate::i18n::tr(bar_spec.title).to_string(), t.semibold(17.0), t.text, (w - 200.0).max(80.0));
        ui.painter().galley(bar_r.center() - galley.size() / 2.0, galley, t.text);
        bar.cancel = bar_item(ui, bar_spec.left_id, bar_spec.left, bar_r.left_center() + vec2(4.0, 0.0), Align2::LEFT_CENTER);
        bar.ok = bar_item(ui, bar_spec.right_id, bar_spec.right, bar_r.right_center() - vec2(4.0, 0.0), Align2::RIGHT_CENTER);
        if !open {
            // (sliding out: its bar's buttons are deaf)
            bar = Bar::default();
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
/// bottom with Cancel under it. A tap outside, on Cancel or on any row closes it: it slides (or
/// fades) away, drawn as it was, under a layer that takes the touches meanwhile.
pub fn actions(ctx: &egui::Context, id: &str, title: Option<&str>, add: impl FnOnce(&mut egui::Ui)) {
    let sid = Id::new(("lc-sheet", id));
    let live = opened(ctx, id);
    let p = slide(ctx, sid, live.is_some());
    let last = sid.with("anchor");
    let closing = live.is_none();
    let anchor = match live {
        Some(a) => {
            ctx.data_mut(|d| d.insert_temp(last, a));
            a
        }
        // (leaving: where it was)
        None if p > 0.0 => ctx.data(|d| d.get_temp::<Option<Rect>>(last)).flatten(),
        None => return,
    };
    let t = Tokens::get(ctx);
    let content = ctx.content_rect();
    let screen = ctx.viewport_rect();
    let mut close = false;
    if closing {
        egui::Area::new(sid.with("block")).order(egui::Order::Tooltip).fixed_pos(screen.min).show(ctx, |ui| {
            let _ = ui.allocate_rect(screen, Sense::click_and_drag());
        });
    }
    // (in the foreground layer: over a page the menu was opened from, which it guards from taps;
    // shown after the page, it goes on top of it)
    egui::Area::new(sid.with("dim")).order(egui::Order::Foreground).fixed_pos(screen.min).show(ctx, |ui| {
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
            // (it drifts a few points towards its button as it fades in and out)
            (w, pos2(x, y + (1.0 - p) * if y < a.top() { 6.0 } else { -6.0 }))
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
    if !closing && (close || ctx.input(|i| i.key_pressed(egui::Key::Escape))) {
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
    let query_id = Id::new("lc-all-commands-query");
    let mut query = ctx.data(|d| d.get_temp::<String>(query_id)).unwrap_or_default();
    let mut run: Option<(String, Value)> = None;
    let open = app.ui.all_commands;
    let bar = page(ctx, "allCommands", "All Commands", "Close", None, open, |ui| {
        let commands = all_commands(app);
        let r = ui.add(crate::widgets::touch_field(
            ui,
            egui::TextEdit::singleline(&mut query).hint_text(crate::i18n::tr("Search")).desired_width(f32::INFINITY),
        ));
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
    if run.is_some() || bar.cancel {
        // (the keyboard goes with the page)
        ctx.memory_mut(|m| {
            if let Some(f) = m.focused() {
                m.surrender_focus(f);
            }
        });
        app.ui.all_commands = false;
    }
    if let Some((id, params)) = run
        && let Err(e) = crate::menubar::run_item(app, &id, params)
    {
        app.toast(ctx, e);
    }
}

/// A text field's edit menu, as iOS shows it over the text: Cut, Copy and Paste with a selection,
/// Select All and Paste without. A tap on a field that already has the keyboard, a double tap
/// (which selects a word) or a long press opens it; typing, a choice or a tap elsewhere closes it.
/// Touch screens only (a mouse has the keyboard's shortcuts). Its choices reach the field as the
/// events those shortcuts make (`app.synthetic`, next frame); Paste reads the host's clipboard
/// (`Services::clipboard_text`). While it is open a tap elsewhere doesn't take the keyboard away.
/// Call once a frame, after everything else is drawn. Buttons: `button:edit-<action>`.
pub fn edit_menu(app: &mut LightcraftApp, ctx: &egui::Context) {
    let id = Id::new("lc-edit-menu");
    let focused = ctx.memory(|m| m.focused()).filter(|f| egui::TextEdit::load_state(ctx, *f).is_some());
    // which field had the keyboard when the finger came down: a first tap only focuses
    let before = ctx.data(|d| d.get_temp::<Option<Id>>(id.with("before"))).flatten();
    if ctx.input(|i| i.pointer.any_pressed()) {
        ctx.data_mut(|d| d.insert_temp(id.with("press"), before));
    }
    ctx.data_mut(|d| d.insert_temp(id.with("before"), focused));
    let ime = ctx.output(|o| o.ime);
    let (Some(field), Some(ime), true) = (focused, ime, ctx.input(|i| i.has_touch_screen())) else {
        ctx.data_mut(|d| d.remove::<Id>(id));
        ctx.options_mut(|o| o.input_options.surrender_focus_on = egui::SurrenderFocusOn::Clicks);
        return;
    };
    let mut open = ctx.data(|d| d.get_temp::<Id>(id)) == Some(field);
    let tapped = match ctx.read_response(field) {
        Some(r) if r.double_clicked() || r.long_touched() => {
            open = true;
            true
        }
        Some(r) if r.clicked() => {
            let had_keyboard = ctx.data(|d| d.get_temp::<Option<Id>>(id.with("press"))).flatten() == Some(field);
            open = had_keyboard && !open;
            true
        }
        _ => false,
    };
    // typing closes it (not the ⌘A its own Select All sends)
    let typed = |e: &egui::Event| match e {
        egui::Event::Text(_) => true,
        egui::Event::Key { pressed: true, modifiers, .. } => !modifiers.command,
        _ => false,
    };
    if ctx.input(|i| i.events.iter().any(typed)) {
        open = false;
    }
    let mut hit = false;
    if open {
        let selected = egui::TextEdit::load_state(ctx, field).and_then(|s| s.cursor.char_range()).is_some_and(|r| !r.is_empty());
        let secret = ime.purpose == egui::IMEPurpose::Password;
        let mut items: Vec<(&'static str, &str)> = Vec::new();
        if selected && !secret {
            items.extend([("cut", "Cut"), ("copy", "Copy")]);
        }
        if !selected {
            items.push(("selectAll", "Select All"));
        }
        if app.services.clipboard_text.is_some() {
            items.push(("paste", "Paste"));
        }
        let (chosen, rect) = edit_menu_bar(ctx, id, &items, ime.cursor_rect, ime.rect);
        hit = ctx.input(|i| i.pointer.interact_pos()).is_some_and(|p| rect.contains(p));
        let command = egui::Modifiers::COMMAND;
        let key = |pressed| egui::Event::Key { key: egui::Key::A, physical_key: None, pressed, repeat: false, modifiers: command };
        match chosen {
            Some("cut") => app.synthetic.push(egui::Event::Cut),
            Some("copy") => app.synthetic.push(egui::Event::Copy),
            Some("paste") => {
                if let Some(text) = app.services.clipboard_text.as_mut().and_then(|f| f()) {
                    app.synthetic.push(egui::Event::Paste(text));
                }
            }
            Some(_) => app.synthetic.extend([key(true), key(false)]),
            None => {}
        }
        // (after Select All it offers Cut and Copy, as iOS does)
        open = chosen.is_none() || chosen == Some("selectAll");
    }
    if ctx.input(|i| i.pointer.any_click()) && !tapped && !hit {
        open = false;
    }
    ctx.data_mut(|d| {
        if open {
            d.insert_temp(id, field);
        } else {
            d.remove::<Id>(id);
        }
    });
    let keep = if open { egui::SurrenderFocusOn::Never } else { egui::SurrenderFocusOn::Clicks };
    ctx.options_mut(|o| o.input_options.surrender_focus_on = keep);
}

/// The edit menu's bar: `items` (id, label) side by side on a rounded dark platter with a small
/// arrow at the cursor, above the field (below it when there's no room). The tapped item and the
/// bar's rect.
fn edit_menu_bar(ctx: &egui::Context, id: Id, items: &[(&'static str, &str)], cursor: Rect, field: Rect) -> (Option<&'static str>, Rect) {
    const H: f32 = 40.0;
    const ARROW: f32 = 7.0;
    let t = Tokens::get(ctx);
    let content = ctx.content_rect();
    let font = t.font(15.0);
    let galleys: Vec<_> =
        items.iter().map(|(_, l)| ctx.fonts_mut(|f| f.layout_no_wrap(crate::i18n::tr(l).to_string(), font.clone(), t.text))).collect();
    let w: f32 = galleys.iter().map(|g| g.size().x + 28.0).sum();
    let above = field.top() - ARROW - H >= content.top() + 4.0;
    let y = if above { field.top() - ARROW - H } else { field.bottom() + ARROW };
    let x = (cursor.center().x - w / 2.0).clamp(content.left() + 8.0, (content.right() - 8.0 - w).max(content.left() + 8.0));
    let bar = Rect::from_min_size(pos2(x, y), vec2(w, H));
    let mut chosen = None;
    egui::Area::new(id.with("bar")).order(egui::Order::Tooltip).constrain(false).fixed_pos(bar.min).show(ctx, |ui| {
        let p = ui.painter().clone();
        p.add(egui::epaint::Shadow { offset: [0, 6], blur: 24, spread: 0, color: Color32::from_black_alpha(110) }.as_shape(bar, 9.0));
        p.rect_filled(bar, 9.0, t.hover);
        // the arrow points at the cursor
        let ax = cursor.center().x.clamp(bar.left() + 14.0, bar.right() - 14.0);
        let (base, tip) = if above { (bar.bottom(), bar.bottom() + ARROW) } else { (bar.top(), bar.top() - ARROW) };
        p.add(egui::Shape::convex_polygon(vec![pos2(ax - ARROW, base), pos2(ax + ARROW, base), pos2(ax, tip)], t.hover, Stroke::NONE));
        let mut left = bar.left();
        for (i, ((key, label), galley)) in items.iter().zip(galleys).enumerate() {
            let r = Rect::from_min_size(pos2(left, bar.top()), vec2(galley.size().x + 28.0, H));
            let resp = ui.interact(r, id.with(*key), Sense::click());
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, crate::i18n::tr(label)));
            crate::widgets::register(ctx, format!("button:edit-{key}"), r);
            if resp.is_pointer_button_down_on() {
                let round = |first, last| CornerRadius { nw: first, sw: first, ne: last, se: last };
                p.rect_filled(r, round(if i == 0 { 9 } else { 0 }, if i + 1 == items.len() { 9 } else { 0 }), t.pressed);
            }
            if i > 0 {
                p.vline(r.left(), r.shrink2(vec2(0.0, 10.0)).y_range(), Stroke::new(1.0, t.text_disabled));
            }
            p.galley(r.center() - galley.size() / 2.0, galley, t.text);
            if resp.clicked() {
                chosen = Some(*key);
            }
            left = r.right();
        }
    });
    (chosen, bar)
}
