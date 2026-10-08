//! iOS's inset grouped lists for the phone's forms (Settings): small grey section headers, rows in
//! rounded cards with hairline separators between them, and grey footers under a card. Layout and
//! behaviour follow iOS's own forms; the drawing and the words are our own.
//!
//! A card's background can only be drawn once its rows are laid out, so [`row`] reserves a place
//! for it behind the rows (a no-op shape) and [`close`] fills it. Rows go into the open card
//! (a new one starts when none is open); a [`header`] or [`footer`] closes it; the page closes
//! the last one at its end.

use egui::{Align2, CornerRadius, Margin, Rect, RichText, Sense, Shape, Stroke, pos2, vec2};

use crate::icons::Icon;
use crate::theme::Tokens;

/// Rows are at least this tall (Apple's minimum touch target).
const ROW_H: f32 = 44.0;
/// The page's side margin, and where a row's separator starts.
const SIDE: f32 = 16.0;

/// The open card: where its background goes, where it starts, and how many rows it has; and the
/// pass it was opened in (a card left open by a pass that ended early must not be filled in a later
/// one, whose shape list its slot doesn't belong to).
#[derive(Clone, Copy)]
struct Open {
    slot: egui::layers::ShapeIdx,
    top: f32,
    rows: usize,
    pass: u64,
}

fn key(ui: &egui::Ui) -> egui::Id {
    ui.id().with("lc-form-card")
}

/// One row of the open card (a card is opened when there is none): `add` draws its contents
/// inside 16 pt margins, at least 44 pt tall, under a hairline that sets it off from the row above.
pub fn row<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let t = Tokens::get(ui.ctx());
    let pass = ui.ctx().cumulative_pass_nr();
    let mut open = match ui.data(|d| d.get_temp::<Open>(key(ui))).filter(|o| o.pass == pass) {
        Some(o) => o,
        None => Open { slot: ui.painter().add(Shape::Noop), top: ui.cursor().top(), rows: 0, pass },
    };
    if open.rows > 0 {
        let (left, right, y) = (ui.max_rect().left() + SIDE, ui.max_rect().right(), ui.cursor().top());
        ui.painter().line_segment([pos2(left, y), pos2(right, y)], Stroke::new(0.5, t.divider));
    }
    open.rows += 1;
    ui.data_mut(|d| d.insert_temp(key(ui), open));
    ui.spacing_mut().item_spacing.y = 0.0;
    egui::Frame::NONE
        .inner_margin(Margin::symmetric(SIDE as i8, 0))
        .show(ui, |ui| {
            ui.set_min_height(ROW_H);
            ui.spacing_mut().item_spacing = egui::vec2(8.0, 6.0);
            add(ui)
        })
        .inner
}

/// Close the open card, if any: its background goes behind its rows.
pub fn close(ui: &mut egui::Ui) {
    let pass = ui.ctx().cumulative_pass_nr();
    let id = key(ui);
    let open = ui.data(|d| d.get_temp::<Open>(id));
    ui.data_mut(|d| d.remove::<Open>(id));
    let Some(open) = open.filter(|o| o.pass == pass) else { return };
    if open.rows == 0 {
        return;
    }
    let t = Tokens::get(ui.ctx());
    let rect = egui::Rect::from_min_max(pos2(ui.max_rect().left(), open.top), pos2(ui.max_rect().right(), ui.cursor().top()));
    ui.painter().set(open.slot, Shape::rect_filled(rect, CornerRadius::same(10), t.cell_selected));
}

/// A section's small grey title above its card.
pub fn header(ui: &mut egui::Ui, text: &str) {
    close(ui);
    let t = Tokens::get(ui.ctx());
    ui.add_space(22.0);
    ui.horizontal(|ui| {
        ui.add_space(SIDE);
        ui.label(RichText::new(text.to_uppercase()).size(13.0).color(t.text_dim));
    });
    ui.add_space(6.0);
}

/// Small grey text under a card (what the rows above do).
pub fn footer(ui: &mut egui::Ui, text: &str) {
    close(ui);
    let t = Tokens::get(ui.ctx());
    ui.add_space(6.0);
    egui::Frame::NONE.inner_margin(Margin::symmetric(SIDE as i8, 0)).show(ui, |ui| {
        ui.label(RichText::new(text).size(13.0).color(t.text_dim));
    });
    ui.add_space(6.0);
}

/// One choice of a list (an iOS inline picker): its label, a check at the right when it is the
/// chosen one, a hairline above it. `width` is the list's (take it before the first one: a wrapping
/// row's available width shrinks as it fills). True when tapped. Widget id `button:<id>`.
pub fn pick(ui: &mut egui::Ui, id: &str, label: &str, on: bool, width: f32) -> bool {
    let t = Tokens::get(ui.ctx());
    let label = crate::i18n::tr(label);
    let (r, resp) = ui.allocate_exact_size(vec2(width, ROW_H), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::RadioButton, true, on, label));
    crate::widgets::register(ui.ctx(), format!("button:{id}"), r);
    let p = ui.painter();
    p.hline(r.x_range(), r.top(), Stroke::new(0.5, t.divider));
    p.text(r.left_center(), Align2::LEFT_CENTER, label, t.font(15.0), if resp.is_pointer_button_down_on() { t.text_dim } else { t.text });
    if on {
        crate::icons::paint(p, Rect::from_center_size(pos2(r.right() - 9.0, r.center().y), vec2(18.0, 18.0)), Icon::Check, t.accent);
    }
    resp.clicked()
}
