//! Widgets for a finger: an on / off switch in place of the desktop's checkbox, text boxes with a
//! 44 pt row, and iOS's segmented control. On the desktop layout each falls back to what egui
//! (or the desktop panels) draw, so a panel can use them without asking which layout it is in.

use egui::{Align2, Color32, Rect, Response, Sense, Stroke, Ui, pos2, vec2};

use super::{register, strip_key_hint};
use crate::theme::Tokens;

/// A phone's slider row: the label and value, and the track under them.
pub const TOUCH_SLIDER_ROW_H: f32 = 46.0;

/// The left and right padding of what a tool panel holds: 24 and 22 pt on the desktop, 16 pt on a
/// phone (iOS's own margin).
pub fn side_pad(ctx: &egui::Context) -> (f32, f32) {
    if crate::is_compact(ctx) { (16.0, 16.0) } else { (24.0, 22.0) }
}

/// A tool panel's margin: the usual sides ([`side_pad`]) and `top` / `bottom` as given.
pub fn margin(ctx: &egui::Context, top: i8, bottom: i8) -> egui::Margin {
    let (left, right) = side_pad(ctx);
    egui::Margin { left: left as i8, right: right as i8, top, bottom }
}

/// The colours of the touch controls: a switch's off track and on track, and the raised segment of
/// a segmented control. Ours (a grey and a green that read on both pages), not a platform's.
fn colors(dark: bool) -> (Color32, Color32, Color32) {
    if dark {
        (Color32::from_rgb(0x39, 0x39, 0x3d), Color32::from_rgb(0x30, 0xd1, 0x58), Color32::from_rgb(0x63, 0x63, 0x66))
    } else {
        (Color32::from_rgb(0xe5, 0xe5, 0xea), Color32::from_rgb(0x34, 0xc7, 0x59), Color32::WHITE)
    }
}

/// A checkbox on the desktop; on a phone a switch at the end of a full-width row, the label
/// first (green when on, its knob sliding across), as forms on a phone show an on / off choice.
pub fn check(ui: &mut Ui, on: &mut bool, label: impl Into<egui::WidgetText>) -> Response {
    if !crate::is_compact(ui.ctx()) {
        return ui.checkbox(on, label);
    }
    let t = Tokens::get(ui.ctx());
    let enabled = ui.is_enabled();
    let w = ui.available_width().max(80.0);
    let label: egui::WidgetText = label.into();
    let label = match strip_key_hint(label.text()) {
        plain if plain.len() != label.text().len() => plain.to_string().into(),
        _ => label,
    };
    let galley = label.into_galley(ui, Some(egui::TextWrapMode::Wrap), w - 70.0, egui::TextStyle::Body);
    let (r, mut resp) = ui.allocate_exact_size(vec2(w, (galley.size().y + 16.0).max(44.0)), Sense::click());
    if resp.clicked() && enabled {
        *on = !*on;
        resp.mark_changed();
    }
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, enabled, *on, galley.text()));
    // the knob springs across (a little past, and back)
    let k = crate::glass::spring(ui.ctx(), resp.id.with("knob"), if *on { 1.0 } else { 0.0 }, 0.35, 0.68);
    let held = crate::glass::press(ui.ctx(), &resp);
    let p = ui.painter();
    p.galley(pos2(r.left(), r.center().y - galley.size().y / 2.0), galley, if enabled { t.text } else { t.text_disabled });
    // 62 × 28 pt with a capsule knob; its off track is a translucent grey
    let sw = Rect::from_center_size(pos2(r.right() - 31.0, r.center().y), vec2(62.0, 28.0));
    let (off, green, _) = colors(ui.visuals().dark_mode);
    let tone = k.clamp(0.0, 1.0);
    let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * tone).round() as u8;
    let track = Color32::from_rgb(mix(off.r(), green.r()), mix(off.g(), green.g()), mix(off.b(), green.b()));
    p.rect_filled(sw, 14.0, if enabled { track } else { track.gamma_multiply(0.5) });
    // held, the knob swells past the track into a lens of glass
    let knob_size = vec2(38.0, 24.0) + vec2(10.0, 12.0) * held;
    let x = sw.left() + 2.0 + knob_size.x / 2.0 - 5.0 * held + (sw.width() - 4.0 - knob_size.x + 10.0 * held) * k;
    let knob = Rect::from_center_size(pos2(x, sw.center().y), knob_size);
    if held > 0.01 {
        crate::glass::paint(p, knob, knob_size.y / 2.0, None, 0.4 * held);
        p.rect_filled(knob, knob_size.y / 2.0, Color32::from_white_alpha((255.0 * (1.0 - held).max(0.0)) as u8));
    } else {
        p.add(egui::epaint::Shadow { offset: [0, 1], blur: 6, spread: 0, color: Color32::from_black_alpha(50) }.as_shape(knob, 12.0));
        p.rect_filled(knob, 12.0, Color32::WHITE);
    }
    resp
}

/// A text box on the desktop as it is; on a phone a 44 pt row with larger type and the text
/// centred in it.
pub fn text_box<'t>(ui: &Ui, edit: egui::TextEdit<'t>) -> egui::TextEdit<'t> {
    if !crate::is_compact(ui.ctx()) {
        return edit;
    }
    edit.font(egui::FontId::proportional(17.0))
        .margin(egui::Margin::symmetric(12, 10))
        .min_size(vec2(0.0, crate::TOUCH_ROW_H))
        .align(egui::Align2::LEFT_CENTER)
}

/// A segmented control (phone dialogs): segments of equal width on a capsule grey track, the
/// chosen one raised on a lighter capsule that slides to the one tapped; more than four wrap to
/// rows of four. Segment `i` is `button:{id}-{i}`. Returns the tapped index.
pub fn ios_segmented(ui: &mut Ui, id: &str, labels: &[&str], active: Option<usize>) -> Option<usize> {
    segments(ui, id, labels.iter().enumerate().map(|(i, l)| (*l, i.to_string())).collect(), active, 4)
}

/// [`ios_segmented`] with `per_row` segments to a row and `button:{id}-{key}` for each segment's
/// `(label, key)`.
pub(super) fn segments(ui: &mut Ui, id: &str, items: Vec<(&str, String)>, active: Option<usize>, per_row: usize) -> Option<usize> {
    let t = Tokens::get(ui.ctx());
    let w = ui.available_width();
    let mut tapped = None;
    // as few rows as the labels need, filled evenly (five are 3 + 2, not 4 + 1)
    let widest =
        items.iter().map(|(l, _)| ui.painter().layout_no_wrap(crate::i18n::tr(l).to_string(), t.semibold(13.0), t.text).size().x).fold(0.0, f32::max);
    let fit = ((w / (widest + 16.0)).floor() as usize).clamp(1, per_row.max(1));
    let rows = items.len().div_ceil(fit).max(1);
    let per_row = items.len().div_ceil(rows).max(1);
    let dark = ui.visuals().dark_mode;
    let (_, _, pill_fill) = colors(dark);
    for (row_i, row) in items.chunks(per_row).enumerate() {
        // a capsule track, the chosen segment on a raised capsule that slides to the one tapped
        let (track, _) = ui.allocate_exact_size(vec2(w, 36.0), Sense::hover());
        ui.painter().rect_filled(track, 18.0, t.inset);
        let seg_w = track.width() / row.len() as f32;
        let pill_idx = ui.painter().add(egui::Shape::Noop);
        let first = row_i * per_row.max(1);
        let chosen = active.filter(|a| (first..first + row.len()).contains(a)).map(|a| (a - first) as f32);
        if let Some(j) = chosen {
            let spring_id = ui.scope_id().with((id, "pill", row_i));
            let x = crate::glass::spring(ui.ctx(), spring_id, j, 0.4, 0.78);
            let pill = Rect::from_min_size(pos2(track.left() + x * seg_w, track.top()), vec2(seg_w, track.height())).shrink(2.5);
            let shadow = egui::epaint::Shadow { offset: [0, 2], blur: 6, spread: 0, color: Color32::from_black_alpha(if dark { 60 } else { 26 }) };
            ui.painter().set(
                pill_idx,
                egui::Shape::Vec(vec![
                    shadow.as_shape(pill, pill.height() / 2.0).into(),
                    egui::Shape::rect_filled(pill, pill.height() / 2.0, pill_fill),
                ]),
            );
        }
        for (j, (label, key)) in row.iter().enumerate() {
            let i = first + j;
            let r = Rect::from_min_size(pos2(track.left() + j as f32 * seg_w, track.top()), vec2(seg_w, track.height()));
            let resp = ui.interact(r, ui.scope_id().with((id, i)), Sense::click());
            let on = active == Some(i);
            resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, true, on, *label));
            register(ui, format!("button:{id}-{key}"), r);
            let p = ui.painter();
            if !on && j > 0 && active != Some(i - 1) {
                p.vline(r.left(), r.shrink2(vec2(0.0, 11.0)).y_range(), Stroke::new(1.0, t.hover));
            }
            let font = if on { t.semibold(13.0) } else { t.font(13.0) };
            p.text(r.center(), Align2::CENTER_CENTER, crate::i18n::tr(label), font, t.text);
            if resp.clicked() {
                tapped = Some(i);
            }
        }
    }
    tapped
}
