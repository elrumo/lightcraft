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
/// A pull-down menu's width.
const PULL_DOWN_W: f32 = 270.0;
/// A menu's corners (iOS 26's are round enough to look like a lozenge).
const MENU_R: f32 = 24.0;

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

/// What a glass button at the end of a page's bar shows.
#[derive(Clone, Copy)]
enum Face<'a> {
    Icon(Icon),
    Label(&'a str),
}

/// A glass button at one end of a page's bar (`button:<id>`), `align`ed to `at`, as iOS 26's sheets
/// have them: a circle with an icon, or a capsule with a label; `tint` makes it a prominent one.
/// `tip` is what a screen reader calls it. True when tapped.
fn glass_end(ui: &mut egui::Ui, id: &str, tip: &str, face: Face, tint: Option<Color32>, enabled: bool, at: egui::Pos2, align: Align2) -> bool {
    let t = Tokens::get(ui.ctx());
    let tint = tint.filter(|_| enabled);
    let ink = match (enabled, tint) {
        (false, _) => t.text_disabled,
        (true, Some(_)) => Color32::WHITE,
        (true, None) => t.text,
    };
    let galley = match face {
        Face::Label(label) => Some(ui.painter().layout_no_wrap(crate::i18n::tr(label).to_string(), t.semibold(15.0), ink)),
        Face::Icon(_) => None,
    };
    let w = galley.as_ref().map_or(crate::glass::BUTTON_D, |g| g.size().x + 30.0);
    // (12 pt in from the page's edge, as iOS sets them)
    let left = if align == Align2::LEFT_CENTER { at.x + 8.0 } else { at.x - 8.0 - w };
    let glass = Rect::from_min_size(pos2(left, at.y - crate::glass::BUTTON_D / 2.0), vec2(w, crate::glass::BUTTON_D));
    let hit = glass.expand2(vec2(3.0, (ROW_H - glass.height()) / 2.0));
    // (two pages can be up at once with the same buttons: the id is the page's own)
    let resp = ui.interact(hit, ui.id().with(("lc-bar", id)), if enabled { Sense::click() } else { Sense::hover() });
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, crate::i18n::tr(tip)));
    crate::widgets::register(ui.ctx(), format!("button:{id}"), hit);
    let k = crate::glass::press(ui.ctx(), &resp);
    let glass = glass.expand(2.0 * k);
    crate::glass::paint(ui.painter(), glass, glass.height() / 2.0, tint, k);
    match (face, galley) {
        (_, Some(g)) => ui.painter().galley(glass.center() - g.size() / 2.0, g, ink),
        (Face::Icon(icon), None) => crate::icons::paint(ui.painter(), Rect::from_center_size(glass.center(), vec2(20.0, 20.0)), icon, ink),
        (Face::Label(_), None) => {}
    }
    resp.clicked()
}

/// A text item of a bar as iOS 26 draws it: "Cancel" and "Close" a ✕, "Done" and "OK" a ✓ (the
/// page's action, `strong`, tinted), a back button ("‹…") a chevron, any other label a capsule.
fn bar_button(ui: &mut egui::Ui, id: &str, label: &str, at: egui::Pos2, align: Align2, strong: bool, enabled: bool) -> bool {
    if label.is_empty() {
        return false;
    }
    let accent = Tokens::get(ui.ctx()).accent;
    let tint = strong.then_some(accent);
    if let Some(rest) = label.strip_prefix('‹') {
        let tip = if rest.is_empty() { "Back" } else { rest };
        return glass_end(ui, id, tip, Face::Icon(Icon::ChevronLeft), None, enabled, at, align);
    }
    let face = match label {
        "Cancel" | "Close" => Face::Icon(Icon::Close),
        "Done" | "OK" => Face::Icon(Icon::Check),
        _ => Face::Label(label),
    };
    glass_end(ui, id, label, face, tint, enabled, at, align)
}

/// What a page's bar holds at either end.
#[derive(Clone, Copy, Debug)]
pub enum Item<'a> {
    None,
    /// A text button; `true`: it can be tapped. A leading "‹" makes it a back button.
    Text(&'a str, bool),
    /// The page's action, in bold.
    Strong(&'a str, bool),
    /// An icon button (what a screen reader calls it; `true`: a prominent one, of accent-tinted glass).
    Icon(Icon, &'a str, bool),
    /// A text in a glass capsule: a choice that applies to the page (Select All).
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
    let accent = Tokens::get(ui.ctx()).accent;
    match item {
        Item::None => false,
        Item::Text(label, enabled) => bar_button(ui, id, label, at, align, false, enabled),
        Item::Strong(label, enabled) => bar_button(ui, id, label, at, align, true, enabled),
        Item::Icon(icon, tip, prominent) => glass_end(ui, id, tip, Face::Icon(icon), prominent.then_some(accent), true, at, align),
        Item::Pill(label) => glass_end(ui, id, label, Face::Label(label), None, true, at, align),
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
        ctx.data_mut(|d| d.remove::<f32>(sid.with("pull").with("from")));
        return Bar { gone: true, ..Bar::default() };
    }
    let content = ctx.content_rect();
    let screen = ctx.viewport_rect();
    let travel = screen.bottom() - content.top();
    // pulled down by its bar, or by its content when that is scrolled to its top, it follows the
    // finger and closes when let go far enough down (as `Bar::cancel`)
    let at_rest = Rect::from_min_max(pos2(content.left(), content.top() + 8.0), screen.max);
    let (scroll, at_top) = ctx.data(|d| (d.get_temp::<[Id; 2]>(sid.with("scroll-ids")), d.get_temp::<bool>(sid.with("at-top")).unwrap_or(true)));
    let pulled = pull(ctx, sid, at_rest, scroll.as_ref().map_or(&[][..], |s| &s[..]), |o| at_top || o.y < at_rest.top() + ROW_H);
    let pulled_shut = pulled.1 && open;
    let drop = pulled_drop(ctx, sid, p, travel, pulled);
    // how far up it is: the dim and the screen behind follow a pull too
    let up = (1.0 - drop / travel.max(1.0)).clamp(0.0, 1.0);
    // (the screen behind steps back as it comes up: `stack_behind`)
    ctx.data_mut(|d| {
        let most = d.get_temp::<f32>(presented_id()).unwrap_or(0.0);
        d.insert_temp(presented_id(), most.max(up));
    });
    // dim what is behind while it slides in, and keep taps off it
    egui::Area::new(sid.with("dim")).order(egui::Order::Middle).fixed_pos(screen.min).show(ctx, |ui| {
        ui.painter().rect_filled(screen, 0.0, Color32::from_black_alpha((t.scrim as f32 * up) as u8));
        let _ = ui.allocate_rect(screen, Sense::click());
    });
    let mut bar = Bar { cancel: pulled_shut, ..Bar::default() };
    let top = content.top() + 8.0 + drop;
    // (opaque as it slides: not egui's fade-in of a new area, through which what is behind showed)
    egui::Area::new(sid).order(egui::Order::Foreground).constrain(false).fade_in(false).fixed_pos(pos2(content.left(), top)).show(ctx, |ui| {
        let w = content.width();
        // the sheet reaches the bottom edge of the screen (behind the home indicator); its contents
        // stop at the safe area / keyboard
        let full = Rect::from_min_max(pos2(content.left(), top), pos2(content.right(), screen.bottom().max(top + ROW_H)));
        let _ = ui.allocate_rect(full, Sense::hover());
        // (its colour goes on under the home indicator: the area's own clip stops at the safe area)
        ui.set_clip_rect(full);
        crate::widgets::register(ctx, format!("sheet:{id}"), full);
        crate::widgets::register(ctx, "dialog:window", full);
        ui.painter().rect_filled(full, CornerRadius { nw: PAGE_R, ne: PAGE_R, sw: 0, se: 0 }, t.chrome);
        let bar_r = Rect::from_min_size(full.min, vec2(w, ROW_H));
        let galley = ui.painter().layout(crate::i18n::tr(bar_spec.title).to_string(), t.semibold(17.0), t.text, (w - 200.0).max(80.0));
        ui.painter().galley(bar_r.center() - galley.size() / 2.0, galley, t.text);
        bar.cancel |= bar_item(ui, bar_spec.left_id, bar_spec.left, bar_r.left_center() + vec2(4.0, 0.0), Align2::LEFT_CENTER);
        bar.ok = bar_item(ui, bar_spec.right_id, bar_spec.right, bar_r.right_center() - vec2(4.0, 0.0), Align2::RIGHT_CENTER);
        if !open {
            // (sliding out: its bar's buttons are deaf)
            bar = Bar::default();
        }
        ui.painter().hline(full.x_range(), bar_r.bottom(), Stroke::new(0.5, t.divider));
        let body_r =
            Rect::from_min_max(pos2(full.left(), bar_r.bottom() + 1.0), pos2(full.right(), (content.bottom() + drop).max(bar_r.bottom() + 1.0)));
        let mut child = ui.new_child(UiBuilder::new().max_rect(body_r).id_salt(sid.with("body")));
        child.set_clip_rect(body_r.intersect(ui.clip_rect()));
        let out = egui::ScrollArea::vertical().id_salt(sid.with("scroll")).auto_shrink([false, false]).show(&mut child, |ui| {
            egui::Frame::NONE.inner_margin(egui::Margin { left: 16, right: 16, top: 12, bottom: 24 }).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 10.0;
                body(ui);
            });
        });
        // (a list of its own inside, like the albums', says where it is scrolled: `scrolled`)
        let (top, inner) = ctx.data(|d| d.get_temp::<(bool, Id)>(sid.with("inner"))).unwrap_or((true, out.id));
        ctx.data_mut(|d| {
            d.insert_temp(sid.with("scroll-ids"), [out.id, inner]);
            d.insert_temp(sid.with("at-top"), out.state.offset.y <= 0.5 && top);
            d.remove::<(bool, Id)>(sid.with("inner"));
        });
    });
    bar
}

/// A page's body that scrolls a list of its own (the albums) says where that list is, so that a
/// pull down on it closes the page only from its top. Call while drawing the page `id`.
pub fn scrolled<R>(ctx: &egui::Context, id: &str, list: &egui::scroll_area::ScrollAreaOutput<R>) {
    let at = (list.state.offset.y <= 0.5, list.id);
    ctx.data_mut(|d| d.insert_temp(Id::new(("lc-sheet", id)).with("inner"), at));
}

/// A page's top corners, rounder than they were before iOS 26.
const PAGE_R: u8 = 24;

/// How far (points) a finger moves down before a sheet follows it.
const PULL_SLOP: f32 = 10.0;
/// A sheet let go this far down, or flicked down this fast (points per second), closes.
const PULL_CLOSE: f32 = 120.0;
const PULL_FLICK: f32 = 700.0;

/// A pull of a sheet in progress: where the finger came down and whether the sheet follows it.
#[derive(Clone, Copy, Debug)]
struct Pull {
    origin: egui::Pos2,
    /// The finger came down where it may pull (the bar, or content scrolled to its top).
    grabbed: bool,
    following: bool,
}

/// Something inside a sheet has the finger this frame (an album being carried): the sheet must not
/// follow it down. Call every frame while it lasts.
pub fn hold(ctx: &egui::Context) {
    let frame = ctx.cumulative_frame_nr();
    ctx.data_mut(|d| d.insert_temp(Id::new("lc-sheet-hold"), frame));
}

fn held(ctx: &egui::Context) -> bool {
    let at = ctx.data(|d| d.get_temp::<u64>(Id::new("lc-sheet-hold")));
    at.is_some_and(|f| f + 1 >= ctx.cumulative_frame_nr())
}

/// A sheet pulled down by the finger, as iOS's are: a press inside `sheet` where `grab` says it may
/// pull, that then moves down more than sideways, drags the sheet down with it (unless a widget
/// other than the `scroll` areas took the drag: a slider, a text field). Returns how far it is pulled this
/// frame (springing back once let go) and whether it was just let go far or fast enough to close.
fn pull(ctx: &egui::Context, id: Id, sheet: Rect, scroll: &[Id], grab: impl Fn(egui::Pos2) -> bool) -> (f32, bool) {
    let key = id.with("pull");
    // (only where nothing is over it: a menu or another sheet on top takes its own pulls)
    let on_top = |o: egui::Pos2| ctx.layer_id_at(o) == Some(egui::LayerId::new(egui::Order::Foreground, id));
    let (pressed, down, origin, pos, vy) =
        ctx.input(|i| (i.pointer.any_pressed(), i.pointer.primary_down(), i.pointer.press_origin(), i.pointer.latest_pos(), i.pointer.velocity().y));
    let mut st = ctx.data(|d| d.get_temp::<Pull>(key));
    if pressed {
        st = origin.filter(|o| sheet.contains(*o) && on_top(*o)).map(|o| Pull { origin: o, grabbed: grab(o), following: false });
    }
    let mut closed = false;
    let mut by = None;
    if let (Some(s), Some(p)) = (st.as_mut(), pos) {
        let d = p - s.origin;
        let free = ctx.dragged_id().is_none_or(|w| scroll.iter().any(|s| w == s.with("area")));
        if down && !s.following && s.grabbed && d.y > PULL_SLOP && d.y > d.x.abs() && free && !held(ctx) {
            s.following = true;
        }
        if s.following {
            if down {
                by = Some(d.y.max(0.0));
            } else {
                closed = d.y > PULL_CLOSE.min(sheet.height() * 0.4) || vy > PULL_FLICK;
            }
        }
    }
    if !down {
        st = None;
    }
    ctx.data_mut(|d| {
        d.remove::<Pull>(key);
        if let Some(s) = st {
            d.insert_temp(key, s);
        }
    });
    // (followed exactly while held; let go, it springs back up)
    let shown = ctx.animate_value_with_time(key.with("at"), by.unwrap_or(0.0), if by.is_some() { 0.0 } else { 0.25 });
    (shown, closed)
}

/// How far below its place to draw a sheet that slides `slide` points down when shut (`p`: its
/// slide progress) and is pulled by the finger ([`pull`]): let go to close, it carries on down from
/// where the finger left it instead of springing back first.
fn pulled_drop(ctx: &egui::Context, id: Id, p: f32, slide: f32, pulled: (f32, bool)) -> f32 {
    let from = id.with("pull").with("from");
    let (by, closed) = pulled;
    if closed {
        ctx.data_mut(|d| d.insert_temp(from, by));
    }
    match ctx.data(|d| d.get_temp::<f32>(from)) {
        // (sliding away, or left open by its caller: then it springs back)
        Some(f) if p < 1.0 || closed => f + (1.0 - p) * (slide - f).max(0.0),
        Some(_) => {
            ctx.data_mut(|d| d.remove::<f32>(from));
            by
        }
        None => by + (1.0 - p) * slide,
    }
}

/// Where the pages say how far up they are this frame (the most of any), for [`stack_behind`].
fn presented_id() -> Id {
    Id::new("lc-presented")
}

/// The screen behind a page steps back while the page is up, as an iPhone shows a sheet: it
/// shrinks a little towards the top, its corners round, and black shows around it. Call once a
/// frame after everything is drawn, with the layer the screen is drawn in and a placeholder added
/// before it (for the black). Phones only: a tablet's pages don't cover the whole screen's width.
pub fn stack_behind(ctx: &egui::Context, layer: egui::LayerId, backdrop: egui::layers::ShapeIdx) {
    let p = ctx.data_mut(|d| d.remove_temp::<f32>(presented_id())).unwrap_or(0.0).clamp(0.0, 1.0);
    let (screen, content) = (ctx.viewport_rect(), ctx.content_rect());
    if p <= 0.0 || screen.width() >= super::compact::WIDE_PT {
        return;
    }
    let s = 1.0 - 0.07 * p;
    // its top comes to rest a little above the page's
    let top = screen.top() + ((content.top() - 4.0).max(screen.top()) - screen.top()) * p;
    let shift = vec2(screen.center().x * (1.0 - s), top - s * screen.top());
    ctx.transform_layer_shapes(layer, egui::emath::TSTransform::new(shift, s));
    let card = Rect::from_min_size(pos2(screen.left() * s, screen.top() * s) + shift, screen.size() * s);
    // (over the whole screen: a layer's painter stops at the safe area, and the black goes round the
    // status bar too)
    let mut painter = ctx.layer_painter(layer);
    painter.set_clip_rect(screen);
    // black round it, and under its content the bars' colour, which is what shows of it above the
    // page (nothing is drawn in its status bar's strip)
    let r = 14.0 * p;
    let fill = Tokens::get(ctx).canvas;
    painter.set(backdrop, egui::Shape::Vec(vec![egui::Shape::rect_filled(screen, 0.0, Color32::BLACK), egui::Shape::rect_filled(card, r, fill)]));
    // (its rounded corners: black over what is outside them)
    painter.rect_stroke(card, r, Stroke::new(r + 2.0, Color32::BLACK), egui::StrokeKind::Outside);
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

/// Some menu or action sheet is open (it has the keyboard: Esc closes it, and no shortcut fires
/// behind it).
pub fn any_actions_open(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<(String, Option<Rect>)>(open_id()).is_some())
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
    actions_wide(ctx, id, title, PULL_DOWN_W, add);
}

/// [`actions`] with a pull-down `menu_w` wide (at most the screen's width less its margins): for a
/// panel of its own rather than a list of rows, like the sync status.
pub fn actions_wide(ctx: &egui::Context, id: &str, title: Option<&str>, menu_w: f32, add: impl FnOnce(&mut egui::Ui)) {
    let mut add = Some(add);
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
        None => {
            ctx.data_mut(|d| d.remove::<f32>(sid.with("pull").with("from")));
            return;
        }
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
    let dim = egui::Area::new(sid.with("dim")).order(egui::Order::Foreground).fixed_pos(screen.min).show(ctx, |ui| {
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
            let w = menu_w.min(content.width() - 16.0);
            let x = if a.center().x > content.center().x { a.right() - w } else { a.left() };
            let x = x.clamp(content.left() + 8.0, (content.right() - w - 8.0).max(content.left() + 8.0));
            let below = a.bottom() + 6.0;
            let y = if below + h <= content.bottom() - 8.0 { below } else { (a.top() - 6.0 - h).max(content.top() + 8.0) };
            // (it drifts a few points towards its button as it fades in and out)
            (w, pos2(x, y + (1.0 - p) * if y < a.top() { 6.0 } else { -6.0 }))
        }
        None => {
            // an action sheet pulled down follows the finger, and closes when let go far enough
            let w = (content.width() - 16.0).min(ACTIONS_MAX_W);
            let at_rest = Rect::from_min_size(pos2(content.center().x - w / 2.0, content.bottom() - 8.0 - h), vec2(w, h));
            let pulled = if closing { (0.0, false) } else { pull(ctx, sid, at_rest, &[], |_| true) };
            close |= pulled.1;
            (w, at_rest.min + vec2(0.0, pulled_drop(ctx, sid, p, h + 40.0, pulled)))
        }
    };
    // a pull-down grows out of its button with a little bounce, and shrinks back into it
    let grow = ctx.animate_bool_with_time(sid.with("grow"), !closing, if closing { LEAVE_S } else { 0.42 });
    let shown = egui::Area::new(sid).order(egui::Order::Foreground).fade_in(false).fixed_pos(pos).show(ctx, |ui| {
        if anchor.is_some() {
            ui.multiply_opacity(p);
        }
        ui.set_width(w);
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        // glass, thicker than a button's so that the rows read over anything
        let glass = |ui: &mut egui::Ui, add: &mut dyn FnMut(&mut egui::Ui)| {
            let under = ui.painter().add(egui::Shape::Noop);
            let r = egui::Frame::NONE.inner_margin(egui::Margin::symmetric(0, 6)).show(ui, |ui| add(ui)).response.rect;
            ui.painter().set(under, crate::glass::shape(ui.ctx(), r, MENU_R, Some(crate::glass::menu_fill(ui.ctx())), 0.0));
        };
        glass(ui, &mut |ui| {
            ui.set_width(w);
            if let Some(title) = title {
                let galley = ui.painter().layout(crate::i18n::tr(title).to_string(), t.font(13.0), t.text_dim, w - 32.0);
                let (r, _) = ui.allocate_exact_size(vec2(w, galley.size().y + 24.0), Sense::hover());
                ui.painter().galley(pos2(r.left() + 16.0, r.center().y - galley.size().y / 2.0), galley, t.text_dim);
                ui.painter().hline(r.x_range(), r.bottom() - 0.5, Stroke::new(1.0, t.divider));
            }
            if let Some(add) = add.take() {
                add(ui);
            }
        });
        if anchor.is_none() {
            ui.add_space(8.0);
            glass(ui, &mut |ui| {
                let (r, resp) = ui.allocate_exact_size(vec2(w, ROW_H - 8.0), Sense::click());
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
    if let Some(a) = anchor {
        let s = if closing { 0.7 + 0.3 * grow } else { 0.3 + 0.7 * crate::glass::bounce(grow) };
        let pivot = a.center();
        ctx.transform_layer_shapes(shown.response.layer_id, egui::emath::TSTransform::new(pivot.to_vec2() * (1.0 - s), s));
    }
    // (the menu sits directly on its dim, whichever was pressed last: egui brings an area that is
    // pressed to the front and keeps that order when both show again, so after a tap outside closed
    // it the menu would open under its own dim, greyed, every tap on it closing it)
    ctx.set_sublayer(dim.response.layer_id, shown.response.layer_id);
    crate::widgets::register(ctx, format!("actions:{id}"), shown.response.rect);
    ctx.data_mut(|d| d.insert_temp(h_id, shown.response.rect.height()));
    if !closing && (close || ctx.input(|i| i.key_pressed(egui::Key::Escape))) {
        close_actions(ctx);
    }
}

/// A row of a menu or list page (`button:<id>`), as iOS 26 draws menu items: a check mark (when
/// `checked` is `Some(true)`) or else the icon at the leading edge, then the label; a rounded
/// highlight under the finger. True when tapped; a tap closes the menu.
pub fn row_checked(ui: &mut egui::Ui, id: &str, icon: Option<Icon>, label: &str, enabled: bool, checked: Option<bool>) -> bool {
    row_inked(ui, id, icon, label, enabled, checked, None)
}

/// A [`row`] for something that can't be taken back (Delete), in red as iOS draws those.
pub fn row_danger(ui: &mut egui::Ui, id: &str, icon: Icon, label: &str) -> bool {
    let red = Tokens::get(ui.ctx()).reject;
    row_inked(ui, id, Some(icon), label, true, None, Some(red))
}

fn row_inked(ui: &mut egui::Ui, id: &str, icon: Option<Icon>, label: &str, enabled: bool, checked: Option<bool>, ink: Option<Color32>) -> bool {
    let t = Tokens::get(ui.ctx());
    let w = ui.available_width();
    let (r, resp) = ui.allocate_exact_size(vec2(w, 44.0), if enabled { Sense::click() } else { Sense::hover() });
    resp.widget_info(|| match checked {
        Some(c) => egui::WidgetInfo::selected(egui::WidgetType::Checkbox, enabled, c, label),
        None => egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label),
    });
    crate::widgets::register(ui.ctx(), format!("button:{id}"), r);
    let p = ui.painter();
    if resp.is_pointer_button_down_on() {
        let spot = r.shrink2(vec2(6.0, 1.0));
        p.rect_filled(spot, 12.0, if Tokens::is_dark(ui.ctx()) { Color32::from_white_alpha(28) } else { Color32::from_black_alpha(16) });
    }
    let color = if enabled { ink.unwrap_or(t.text) } else { t.text_disabled };
    let lead = pos2(r.left() + 30.0, r.center().y);
    if checked == Some(true) {
        crate::icons::paint(p, Rect::from_center_size(lead, vec2(16.0, 16.0)), Icon::Check, color);
    } else if let Some(icon) = icon {
        crate::icons::paint(p, Rect::from_center_size(lead, vec2(20.0, 20.0)), icon, color);
    }
    let x = r.left() + 52.0;
    let galley = p.layout(label.to_string(), t.font(17.0), color, (r.right() - 16.0 - x).max(40.0));
    p.galley(pos2(x, r.center().y - galley.size().y / 2.0), galley, color);
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

/// A hairline between groups of menu rows, as iOS 26 separates menu sections.
pub fn row_gap(ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 13.0), Sense::hover());
    ui.painter().hline((r.left() + 16.0)..=(r.right() - 16.0), r.center().y, Stroke::new(1.0, t.divider.gamma_multiply(0.8)));
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
        let r = ui.add(crate::widgets::text_box(
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
/// events those shortcuts make (`app.synthetic`, next frame; Cut and Copy in another pass of the tap's own
/// frame, as a browser lets a page write the clipboard only then); Paste reads the host's clipboard
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
    // The host raises the keyboard when egui reports a field's IME rect, and iOS Safari does so only for a focus
    // made inside the tap. egui reports it only from the pass after the field takes focus and only once the field
    // is on screen (a page slides in from below), so for a moment it doesn't: report the field's own rect then.
    if let Some(field) = focused
        && ctx.input(|i| i.has_touch_screen())
        && ctx.output(|o| o.ime).is_none()
        && let Some(r) = ctx.read_response(field)
    {
        let rect = ctx.layer_transform_to_global(r.layer_id).unwrap_or_default() * r.rect;
        ctx.output_mut(|o| {
            o.ime = Some(egui::output::IMEOutput { purpose: egui::IMEPurpose::Normal, rect, cursor_rect: rect, should_interrupt_composition: false });
        });
    }
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
            Some(which @ ("cut" | "copy")) => {
                // (a browser lets a page write the clipboard only inside the tap: another pass of this frame)
                let event = if which == "cut" { egui::Event::Cut } else { egui::Event::Copy };
                ctx.data_mut(|d| d.insert_temp(id.with("event"), event));
                ctx.request_discard("edit menu: cut or copy");
            }
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

/// Deliver the Cut or Copy the edit menu chose to its field, in the extra pass it asked for (or the next
/// frame's first, when egui declines another pass). Call first thing in a pass.
pub fn edit_menu_event(ctx: &egui::Context) {
    let key = Id::new("lc-edit-menu").with("event");
    if let Some(event) = ctx.data(|d| d.get_temp::<egui::Event>(key)) {
        ctx.data_mut(|d| d.remove::<egui::Event>(key));
        ctx.input_mut(|i| i.events.push(event));
    }
}

/// The edit menu's bar: `items` (id, label) side by side in a glass capsule over the cursor, above
/// the field (below it when there's no room). The tapped item and the bar's rect.
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
        crate::glass::paint(&p, bar, H / 2.0, Some(crate::glass::menu_fill(ctx)), 0.0);
        let mut left = bar.left();
        for ((key, label), galley) in items.iter().zip(galleys) {
            let r = Rect::from_min_size(pos2(left, bar.top()), vec2(galley.size().x + 28.0, H));
            let resp = ui.interact(r, id.with(*key), Sense::click());
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, crate::i18n::tr(label)));
            crate::widgets::register(ctx, format!("button:edit-{key}"), r);
            if resp.is_pointer_button_down_on() {
                p.rect_filled(r.shrink(3.0), (H - 6.0) / 2.0, t.pressed);
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
