use super::albums::MARK_W;
use super::*;

/// How wide the sidebar's content needs to be, from the widest row of the last frame: rows are
/// drawn at this width (or the panel's, if wider), and the sidebar scrolls sideways when it
/// exceeds the panel. One frame behind, which needs no second layout pass.
pub(crate) fn content_width(ctx: &egui::Context) -> f32 {
    ctx.data(|d| d.get_temp::<f32>(egui::Id::unique("left-content-width"))).unwrap_or(0.0)
}

/// The most a row asks for its name: longer ones are cut when there is no more room.
const MAX_NAME_NEED: f32 = 140.0;

/// Height of a sidebar row.
pub(super) const ROW_HEIGHT: f32 = 29.0;

/// Temp-data key of the row (its widget id after `source:`, lower-case) the sidebar scrolls to
/// this frame: set before the sections are drawn, cleared after.
pub(super) const SCROLL_TO_ROW: &str = "left-scroll-to-row";
/// Until when (`ctx.input().time`) [`SCROLL_TO_ROW`] is asked again each frame: the rows above the
/// one asked for unfold, so it moves for a moment after it is first drawn.
pub(super) const SCROLL_UNTIL: &str = "left-scroll-until";

/// Temp-data key of the Folders choice whose rows above were last opened ([`reveal_chosen`]):
/// removed, they open again.
pub(super) const LIBFOLDER_REVEALED: &str = "libfolder-revealed";

/// A row says how wide it needs to be.
pub(super) fn note_width(ui: &egui::Ui, w: f32) {
    ui.data_mut(|d| {
        let m = d.get_temp_mut_or_insert_with::<f32>(egui::Id::unique("left-content-width-next"), || 0.0);
        *m = m.max(w);
    });
}

/// One level of a sidebar tree when the panel has room: callers pass `indent` in these steps.
pub(crate) const INDENT_STEP: f32 = 16.0;

/// The narrowest a level is drawn: the chevron still steps visibly right of its parent's.
pub(crate) const MIN_INDENT_STEP: f32 = 8.0;

/// What a row keeps for its name when the step is chosen: the deepest row's name is not cut to
/// less than this while a narrower step would still keep it.
const MIN_NAME_ROOM: f32 = 80.0;

/// Room kept for a count at the end of the deepest row (three digits and their gap).
const COUNT_ROOM: f32 = 40.0;

/// The step a level is drawn with this frame: [`INDENT_STEP`] while the deepest open row fits the
/// panel, narrower when it would not, never below [`MIN_INDENT_STEP`]. From the last frame.
pub(crate) fn indent_step(ctx: &egui::Context) -> f32 {
    ctx.data(|d| d.get_temp::<f32>(egui::Id::unique("left-indent-step"))).unwrap_or(INDENT_STEP)
}

/// How far right a row at `indent` (in [`INDENT_STEP`]s) is drawn this frame.
pub(super) fn indent_x(ctx: &egui::Context, indent: f32) -> f32 {
    indent / INDENT_STEP * indent_step(ctx)
}

/// [`indent_x`] for a row being laid out, which also notes how deep the tree goes, for next
/// frame's step.
fn drawn_indent(ui: &egui::Ui, indent: f32) -> f32 {
    ui.data_mut(|d| {
        let m = d.get_temp_mut_or_insert_with::<f32>(egui::Id::unique("left-deepest-indent-next"), || 0.0);
        *m = m.max(indent);
    });
    indent_x(ui.ctx(), indent)
}

/// The widest step (16, 14, 12, 10 or 8 pt) at which a row `deepest` deep (in [`INDENT_STEP`]s)
/// keeps [`MIN_NAME_ROOM`] for its name and its count in a panel `panel_w` wide. In steps, so a
/// row does not slide sideways at every point the panel is dragged; when even the narrowest step
/// does not fit, the sidebar scrolls sideways as before.
pub(crate) fn step_for(panel_w: f32, deepest: f32) -> f32 {
    let levels = deepest / INDENT_STEP;
    if levels < 1.0 {
        return INDENT_STEP;
    }
    let room = panel_w - 42.0 - MIN_NAME_ROOM - 18.0 - COUNT_ROOM;
    [16.0, 14.0, 12.0, 10.0].into_iter().find(|step| levels * step <= room).unwrap_or(MIN_INDENT_STEP)
}

pub(super) fn row(
    app: &mut LightcraftApp,
    ui: &mut egui::Ui,
    id: &str,
    icon: Icon,
    label: &str,
    count: Option<usize>,
    selected: bool,
    indent: f32,
) -> egui::Response {
    row_named(app, ui, id, icon, label, None, count, selected, indent)
}

/// [`row`] whose spoken name is `spoken` when the painted `label` is a shortened form of it.
fn row_named(
    app: &mut LightcraftApp,
    ui: &mut egui::Ui,
    id: &str,
    icon: Icon,
    label: &str,
    spoken: Option<&str>,
    count: Option<usize>,
    selected: bool,
    indent: f32,
) -> egui::Response {
    row_sensed(app, ui, id, icon, label, spoken, count, selected, indent, Sense::click(), None)
}

/// [`row_named`] with the given `sense` (rows that can be dragged add `drag`) and a colour
/// label's dot (`mark`) before the count.
#[allow(clippy::too_many_arguments)]
pub(super) fn row_sensed(
    app: &mut LightcraftApp,
    ui: &mut egui::Ui,
    id: &str,
    icon: Icon,
    label: &str,
    spoken: Option<&str>,
    count: Option<usize>,
    selected: bool,
    indent: f32,
    sense: Sense,
    mark: Option<egui::Color32>,
) -> egui::Response {
    let label =
        if matches!(id, "all" | "recentlyAdded" | "picks" | "people" | "missing" | "recentlyDeleted") { crate::i18n::tr(label) } else { label };
    let t = Tokens::get(ui.ctx());
    let phone = crate::is_compact(ui.ctx());
    let indent = drawn_indent(ui, indent);
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_height(ui)), sense);
    if scroll_asked(ui, id) {
        ui.scroll_to_rect_animation(r, None, egui::style::ScrollAnimation::none());
    }
    register(ui, format!("source:{id}"), r);
    let mark_w = if mark.is_some() { MARK_W } else { 0.0 };
    let said = spoken.unwrap_or(label);
    let name = match count {
        Some(n) if id.starts_with("folder:") => crate::i18n::tr_format!("{label}, {n} items", label = said, n = n),
        Some(n) => crate::i18n::tr_format!("{label}, {n} photos", label = said, n = n),
        None => said.to_string(),
    };
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, selected, &name));
    if phone {
        return phone_row(app, ui, resp, id, icon, label, count, selected, indent, mark);
    }
    // what is visible of the row: when the sidebar is scrolled sideways its bars and counts end at
    // the panel's edge, not at the end of the (wider) content
    let visible_right: f32 = ui.data(|d| d.get_temp(egui::Id::unique("left-visible-right"))).unwrap_or(f32::MAX);
    let edge = r.right().min(visible_right);
    let inner = Rect::from_min_max(r.min + vec2(8.0, 0.0), pos2((r.right() - 8.0).min(edge - 8.0).max(r.left() + 8.0), r.bottom()));
    if selected {
        ui.painter().rect_filled(inner, 4.0, t.canvas);
        register(ui, format!("highlight:{id}"), inner);
    } else if resp.hovered() {
        ui.painter().rect_filled(inner, 4.0, t.hover.gamma_multiply(0.6));
    }
    let (icon_size, label_x, font_size, count_size) = (16.0, 42.0, 13.5, 12.5);
    paint(
        ui.painter(),
        Rect::from_center_size(pos2(r.left() + 18.0 + indent + icon_size / 2.0, r.center().y), vec2(icon_size, icon_size)),
        icon,
        if selected { t.text } else { t.icon },
    );
    let font = t.font(font_size);
    let color = if selected { t.text } else { t.text_label };
    // a count of nothing is there, but fainter: the rows that hold photos stand out
    let count_color = |n: usize| if n == 0 { t.text_dim.gamma_multiply(0.55) } else { t.text_dim };
    let count_galley = count.filter(|_| app.ui.show_counts).map(|n| ui.painter().layout_no_wrap(n.to_string(), t.font(count_size), count_color(n)));
    let label_left = r.left() + label_x + indent;
    let count_left = count_galley.as_ref().map_or(edge - 18.0, |g| edge - 18.0 - g.size().x);
    // a label's dot sits just before the count
    if let Some(c) = mark {
        let dot = Rect::from_center_size(pos2(count_left - MARK_W / 2.0, r.center().y), vec2(8.0, 8.0));
        ui.painter().circle_filled(dot.center(), 4.0, c);
        register(ui, format!("labelMark:{id}"), dot);
    }
    // the name gives way to the count: cut with an ellipsis, in full on hover
    let room = count_left - mark_w - 8.0 - label_left;
    let measure = |s: &str| ui.painter().layout_no_wrap(s.to_string(), font.clone(), color).size().x;
    let full_w = measure(label);
    let shown = if full_w <= room { label.to_string() } else { crate::widgets::elide_head(label, room.max(0.0), measure) };
    let label_rect = ui.painter().text(pos2(label_left, r.center().y), Align2::LEFT_CENTER, &shown, font.clone(), color);
    register(ui, format!("label:{id}"), label_rect);
    let resp = if shown != label { resp.on_hover_text(label) } else { resp };
    note_width(ui, row_need(indent, full_w, mark_w, count_galley.as_ref().map(|g| g.size().x)));
    if let Some(galley) = count_galley {
        let rect = Rect::from_min_size(pos2(edge - 18.0 - galley.size().x, r.center().y - galley.size().y / 2.0), galley.size());
        register(ui, format!("count:{id}"), rect);
        ui.painter().galley(rect.min, galley, t.text_dim);
    }
    resp
}

/// Height of a row on a phone: an icon tile beside two lines, as Lightroom's mobile app lists albums.
const PHONE_ROW_H: f32 = 64.0;
/// The side of a phone row's icon tile.
const TILE: f32 = 44.0;
/// Room at a phone row's end for its "…" (album and folder rows) or its disclosure chevron.
pub(super) const PHONE_TRAIL: f32 = 44.0;

/// A row on a phone: its icon on a rounded tile (an album folder's a bare, larger folder), the name
/// with the count under it (a folder's: how many albums it holds), a rounded highlight across the
/// row when chosen and under the finger, and a "…" at the end of album and folder rows (the caller
/// answers it: [`more_button`]). Rows slide up into place as the list opens ([`entrance`]).
#[allow(clippy::too_many_arguments)]
fn phone_row(
    app: &LightcraftApp,
    ui: &mut egui::Ui,
    resp: egui::Response,
    id: &str,
    icon: Icon,
    label: &str,
    count: Option<usize>,
    selected: bool,
    indent: f32,
    mark: Option<egui::Color32>,
) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    let (dy, alpha) = entrance(ui, resp.rect);
    let r = resp.rect.translate(vec2(0.0, dy));
    let edge = r.right().min(visible_right(ui));
    let mut p = ui.painter().clone();
    p.multiply_opacity(alpha);
    let inner = Rect::from_min_max(pos2(r.left() + 8.0, r.top() + 2.0), pos2((edge - 8.0).max(r.left() + 8.0), r.bottom() - 2.0));
    if selected {
        p.rect_filled(inner, 12.0, t.hover);
        register(ui, format!("highlight:{id}"), inner);
    } else if resp.is_pointer_button_down_on() {
        p.rect_filled(inner, 12.0, t.hover.gamma_multiply(0.6));
    }
    let folder = id.starts_with("folder:");
    let tile = Rect::from_min_size(pos2(r.left() + 16.0 + indent, r.center().y - TILE / 2.0), vec2(TILE, TILE));
    if folder {
        paint(&p, tile.shrink(1.0), Icon::Folder, t.text_dim);
    } else {
        p.rect_filled(tile, 6.0, if selected { t.field.gamma_multiply(0.7) } else { t.field });
        paint(&p, Rect::from_center_size(tile.center(), vec2(22.0, 22.0)), icon, t.text);
    }
    let x = tile.right() + 14.0;
    let more = folder || id.starts_with("album:");
    // (room is kept at the end for a "…" or a disclosure chevron)
    let mut right = edge - 16.0 - PHONE_TRAIL;
    if more {
        paint(&p, Rect::from_center_size(more_rect(r, edge).center(), vec2(20.0, 20.0)), Icon::Dots, t.text);
    }
    if let Some(c) = mark {
        p.circle_filled(pos2(right - 4.0, r.center().y), 4.0, c);
        register(ui, format!("labelMark:{id}"), Rect::from_center_size(pos2(right - 4.0, r.center().y), vec2(8.0, 8.0)));
        right -= MARK_W;
    }
    let room = (right - x).max(0.0);
    let font = t.font(17.0);
    let measure = |s: &str| p.layout_no_wrap(s.to_string(), font.clone(), t.text).size().x;
    let shown = if measure(label) <= room { label.to_string() } else { crate::widgets::elide_head(label, room, measure) };
    let sub = count.filter(|_| app.ui.show_counts).map(|n| {
        let n = crate::i18n::grouped(n);
        if folder { crate::i18n::tr_format!("{n} items", n = n) } else { n }
    });
    let title_y = if sub.is_some() { r.center().y - 10.0 } else { r.center().y };
    let label_rect = p.text(pos2(x, title_y), Align2::LEFT_CENTER, &shown, font.clone(), t.text);
    register(ui, format!("label:{id}"), label_rect);
    if let Some(sub) = sub {
        let rect = p.text(pos2(x, r.center().y + 12.0), Align2::LEFT_CENTER, sub, t.font(15.0), t.text_dim);
        register(ui, format!("count:{id}"), rect);
    }
    if shown != label { resp.on_hover_text(label) } else { resp }
}

/// Where a phone row's "…" is: at its end, finger-sized.
fn more_rect(row: Rect, edge: f32) -> Rect {
    Rect::from_center_size(pos2(edge - 16.0 - PHONE_TRAIL / 2.0, row.center().y), vec2(PHONE_TRAIL, PHONE_TRAIL))
}

/// The "…" at the end of a phone's album or folder row (`more:<row id>`): a tap opens the row's menu
/// there, the one a right click opens on a desktop ([`folder_menu`]).
pub(super) fn more_button(ui: &mut egui::Ui, row: &egui::Response, widget: &str) {
    let r = more_rect(row.rect, row.rect.right().min(visible_right(ui)));
    let b = ui.interact(r, row.id.with("more"), Sense::click());
    b.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, crate::i18n::tr("More")));
    register(ui, format!("more:{widget}"), r);
    if b.clicked() {
        open_row_menu(ui.ctx(), row, r.center());
    }
}

/// Open the menu of `row` ([`row_popup`]) at `at`, as its right click would at the pointer.
pub(super) fn open_row_menu(ctx: &egui::Context, row: &egui::Response, at: egui::Pos2) {
    ctx.data_mut(|d| d.insert_temp(row.id.with("menu-at"), at));
    egui::Popup::open_id(ctx, egui::Popup::default_response_id(row));
}

/// A row's context menu: at the pointer for a right click (or a long press), else where
/// [`open_row_menu`] opened it (a phone's "…").
pub(super) fn row_popup(row: &egui::Response) -> egui::Popup<'_> {
    let key = row.id.with("menu-at");
    if row.secondary_clicked() {
        row.ctx.data_mut(|d| d.remove::<egui::Pos2>(key));
    }
    let popup = egui::Popup::context_menu(row).close_behavior(crate::widgets::context_menu_close(row, egui::PopupCloseBehavior::CloseOnClick));
    match row.ctx.data(|d| d.get_temp::<egui::Pos2>(key)) {
        Some(at) => popup.anchor(at),
        None => popup,
    }
}

/// Memory key: when the albums list last opened (a phone's page), for [`entrance`].
pub(crate) const OPENED_AT: &str = "left-opened-at";

/// A phone's rows coming in as the albums list opens: each slides up a little and fades in, a beat
/// after the one above it, as iOS lists cascade. How far down to draw what is at `rect` now, and
/// its opacity.
pub(super) fn entrance(ui: &egui::Ui, rect: Rect) -> (f32, f32) {
    const SECS: f64 = 0.38;
    let Some(opened) = ui.data(|d| d.get_temp::<f64>(egui::Id::unique(OPENED_AT))) else { return (0.0, 1.0) };
    let delay = (f64::from((rect.top() - ui.clip_rect().top()).max(0.0) / PHONE_ROW_H) * 0.03).min(0.4);
    let k = ((ui.input(|i| i.time) - opened - delay) / SECS).clamp(0.0, 1.0) as f32;
    if k < 1.0 {
        ui.ctx().request_repaint();
    }
    let e = egui::emath::easing::cubic_out(k);
    ((1.0 - e) * 22.0, e)
}

/// Whether the row `id` about to be drawn is out of sight: then it only takes its room, is listed
/// for the control channel and asks for its width (so the content does not widen and narrow as
/// rows scroll by), and the caller skips it. A library can have thousands of keywords or folders,
/// and drawing them all every frame slows the whole app down (issue #1011). A row the sidebar is
/// asked to scroll to is drawn. `label`, `count` and `mark` as for [`row_sensed`].
pub(super) fn out_of_sight(app: &LightcraftApp, ui: &mut egui::Ui, id: &str, label: &str, count: Option<usize>, indent: f32, mark: bool) -> bool {
    let r = Rect::from_min_size(ui.cursor().min, vec2(ui.available_width(), row_height(ui)));
    if ui.is_rect_visible(r) || scroll_asked(ui, id) {
        return false;
    }
    ui.allocate_space(r.size());
    register(ui, format!("source:{id}"), r);
    let indent = drawn_indent(ui, indent);
    let t = Tokens::get(ui.ctx());
    let painter = ui.painter();
    let name_w = painter.layout_no_wrap(label.to_string(), t.font(13.5), t.text_label).size().x;
    let count_w = count.filter(|_| app.ui.show_counts).map(|n| painter.layout_no_wrap(n.to_string(), t.font(12.5), t.text_dim).size().x);
    note_width(ui, row_need(indent, name_w, if mark { MARK_W } else { 0.0 }, count_w));
    true
}

/// A row's height: taller on a phone ([`PHONE_ROW_H`]).
fn row_height(ui: &egui::Ui) -> f32 {
    if crate::is_compact(ui.ctx()) { PHONE_ROW_H } else { ROW_HEIGHT }
}

/// Ask the sidebar to scroll to the row `id`, and keep asking while the folds above it open.
pub(super) fn ask_scroll(ui: &egui::Ui, id: String) {
    let until = ui.input(|i| i.time) + 0.6;
    ui.data_mut(|d| {
        d.insert_temp(egui::Id::unique(SCROLL_TO_ROW), id.to_lowercase());
        d.insert_temp(egui::Id::unique(SCROLL_UNTIL), until);
    });
}

/// Whether the sidebar is asked to scroll to the row `id` this frame ([`SCROLL_TO_ROW`]).
fn scroll_asked(ui: &egui::Ui, id: &str) -> bool {
    ui.data(|d| d.get_temp::<String>(egui::Id::unique(SCROLL_TO_ROW))).is_some_and(|want| want == id.to_lowercase())
}

/// How wide a row needs to be, from its indent, name, colour dot and count widths. It asks for
/// room for its name up to a share of a panel, so one very long name does not make everything
/// scroll: depth does that.
fn row_need(indent: f32, name_w: f32, mark_w: f32, count_w: Option<f32>) -> f32 {
    42.0 + indent + name_w.min(MAX_NAME_NEED) + 18.0 + mark_w + count_w.map_or(0.0, |w| w + 16.0)
}

/// The disclosure chevron at the left of a tree row (`indent` is the row's): right-pointing
/// when closed, down when open, registered as widget `widget` for the control channel. Returns
/// its response: the caller flips the open state on `clicked()`.
fn disclosure_chevron(ui: &mut egui::Ui, row: &egui::Response, indent: f32, turn: f32, id: egui::Id, widget: String) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    if crate::is_compact(ui.ctx()) {
        // a phone's: a chevron at the row's end (where the count was), finger-sized, as iOS's outline lists have it
        let edge = row.rect.right().min(visible_right(ui));
        let hit = Rect::from_center_size(pos2(edge - 16.0 - PHONE_TRAIL / 2.0, row.rect.center().y), vec2(PHONE_TRAIL, PHONE_TRAIL));
        let tr = ui.interact(hit, id, Sense::click());
        register(ui, widget, hit);
        let (dy, alpha) = entrance(ui, row.rect);
        let mut p = ui.painter().clone();
        p.multiply_opacity(alpha);
        crate::icons::paint_turned(&p, Rect::from_center_size(hit.center() + vec2(0.0, dy), vec2(16.0, 16.0)), Icon::ChevronRight, t.text_dim, turn);
        return tr;
    }
    let c = pos2(row.rect.left() + 10.0 + indent_x(ui.ctx(), indent), row.rect.center().y);
    let tri = Rect::from_center_size(c, vec2(16.0, 16.0));
    let tr = ui.interact(tri, id, Sense::click());
    register(ui, widget, tri);
    let col = if tr.hovered() { t.text } else { t.text_dim };
    crate::icons::paint_turned(ui.painter(), Rect::from_center_size(c, vec2(16.0, 16.0)), Icon::ChevronRight, col, turn);
    tr
}

/// Whether the tree row `key` is open: what the user last chose this session, else `default`.
pub(super) fn tree_open(ui: &egui::Ui, key: egui::Id, default: bool) -> bool {
    ui.data(|d| d.get_temp(key)).unwrap_or(default)
}

/// Open or close the tree row `key` from now on.
pub(super) fn set_tree_open(ui: &egui::Ui, key: egui::Id, open: bool) {
    ui.data_mut(|d| d.insert_temp(key, open));
}

/// The chevron of a tree row that has rows inside it (Albums' folders, Local, By Date, Folders,
/// Keywords): a click flips `open` and remembers it under `key`. Returns the chevron's response
/// (it sits on the row and takes its clicks, so a row's context menu is attached to it too).
pub(super) fn tree_toggle(ui: &mut egui::Ui, row: &egui::Response, indent: f32, key: egui::Id, open: &mut bool, widget: String) -> egui::Response {
    let turn = crate::motion::chevron_turn(ui.ctx(), key, *open);
    let tri = disclosure_chevron(ui, row, indent, turn, key.with("triangle"), widget);
    if tri.clicked() {
        *open = !*open;
        set_tree_open(ui, key, *open);
    }
    let label = if *open { crate::i18n::tr("Collapse") } else { crate::i18n::tr("Expand") };
    tri.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    tri
}
