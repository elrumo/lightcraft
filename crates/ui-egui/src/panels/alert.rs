//! iOS's alert: a small card in the middle of a dimmed screen with a title, a message and a Cancel
//! and an action button side by side, for a question that wants a yes or a no (a whole page would
//! be too much). Layout and behaviour follow iOS's alerts; the drawing and the words are our own.

use egui::{Align2, Color32, CornerRadius, Id, Rect, Sense, Stroke, pos2, vec2};

use crate::theme::Tokens;

/// How long the alert takes to appear (s).
const FADE_S: f32 = 0.2;
/// The card's width (iOS's alerts are 270 pt).
const WIDTH: f32 = 270.0;
const BUTTON_H: f32 = 44.0;

pub struct Alert<'a> {
    pub id: &'a str,
    pub title: &'a str,
    pub message: &'a str,
    pub cancel: &'a str,
    pub ok: &'a str,
    /// The action destroys something: its button is red.
    pub destructive: bool,
}

/// Show the alert; returns what was tapped, `(cancel, ok)`. Widget ids: `alert:<id>`,
/// `button:alertCancel`, `button:alertOk`.
pub fn show(ctx: &egui::Context, a: &Alert) -> (bool, bool) {
    let t = Tokens::get(ctx);
    let id = Id::new(("lc-alert", a.id));
    let p = ctx.animate_bool_with_time_and_easing(id.with("in"), true, FADE_S, egui::emath::easing::cubic_out);
    let screen = ctx.viewport_rect();
    let content = ctx.content_rect();
    // dim what is behind, and keep taps off it
    egui::Area::new(id.with("dim")).order(egui::Order::Middle).fixed_pos(screen.min).show(ctx, |ui| {
        ui.painter().rect_filled(screen, 0.0, Color32::from_black_alpha((t.scrim as f32 * 0.8 * p) as u8));
        let _ = ui.allocate_rect(screen, Sense::click());
    });
    let width = WIDTH.min(content.width() - 48.0).max(200.0);
    let (mut cancel, mut ok) = (false, false);
    egui::Area::new(id)
        .order(egui::Order::Foreground)
        .constrain(false)
        .pivot(Align2::CENTER_CENTER)
        .fixed_pos(content.center() + vec2(0.0, (1.0 - p) * 10.0))
        .show(ctx, |ui| {
            ui.multiply_opacity(p);
            // the text first, to know how tall the card is
            // (each line is centred on the galley's anchor, which is placed at the card's middle)
            let centred = |text: &str, font: egui::FontId, color: Color32| {
                let mut job = egui::text::LayoutJob::simple(crate::i18n::tr(text).to_string(), font, color, width - 32.0);
                job.halign = egui::Align::Center;
                ui.painter().layout_job(job)
            };
            let title = centred(a.title, t.semibold(17.0), t.text);
            let message = centred(a.message, t.font(13.0), t.text_dim);
            let text_h = 20.0 + title.size().y + if a.message.is_empty() { 0.0 } else { 4.0 + message.size().y } + 20.0;
            let (card, _) = ui.allocate_exact_size(vec2(width, text_h + BUTTON_H + 0.5), Sense::hover());
            crate::widgets::register(ctx, format!("alert:{}", a.id), card);
            let painter = ui.painter();
            painter.add(
                egui::epaint::Shadow { offset: [0, 8], blur: 30, spread: 0, color: Color32::from_black_alpha(90) }
                    .as_shape(card, CornerRadius::same(14)),
            );
            painter.rect_filled(card, 14.0, t.cell_selected);
            painter.galley(pos2(card.center().x, card.top() + 20.0), title.clone(), t.text);
            if !a.message.is_empty() {
                painter.galley(pos2(card.center().x, card.top() + 20.0 + title.size().y + 4.0), message, t.text_dim);
            }
            let row_top = card.top() + text_h;
            painter.hline(card.x_range(), row_top, Stroke::new(0.5, t.divider));
            let half = width / 2.0;
            for (i, (label, strong, red)) in [(a.cancel, false, false), (a.ok, true, a.destructive)].into_iter().enumerate() {
                let r = Rect::from_min_size(pos2(card.left() + i as f32 * half, row_top + 0.5), vec2(half, BUTTON_H));
                let resp = ui.interact(r, id.with(("button", i)), Sense::click());
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, crate::i18n::tr(label)));
                crate::widgets::register(ctx, if i == 0 { "button:alertCancel" } else { "button:alertOk" }, r);
                if resp.is_pointer_button_down_on() {
                    ui.painter().rect_filled(
                        r,
                        CornerRadius { nw: 0, ne: 0, sw: if i == 0 { 14 } else { 0 }, se: if i == 1 { 14 } else { 0 } },
                        t.hover,
                    );
                }
                let color = if red { t.reject } else { t.accent };
                ui.painter().text(
                    r.center(),
                    Align2::CENTER_CENTER,
                    crate::i18n::tr(label),
                    if strong { t.semibold(17.0) } else { t.font(17.0) },
                    color,
                );
                if i == 1 {
                    ui.painter().vline(r.left(), r.y_range(), Stroke::new(0.5, t.divider));
                }
                if resp.clicked() {
                    if i == 0 {
                        cancel = true;
                    } else {
                        ok = true;
                    }
                }
            }
        });
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        cancel = true;
    }
    (cancel, ok)
}

/// The alert `id` isn't on screen this frame (its next showing fades in again).
pub fn hidden(ctx: &egui::Context, id: &str) {
    let _ = ctx.animate_bool_with_time(Id::new(("lc-alert", id)).with("in"), false, 0.0);
}
