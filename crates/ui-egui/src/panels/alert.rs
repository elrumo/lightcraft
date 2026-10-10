//! iOS's alert: a small card of glass in the middle of a dimmed screen with a title, a message and a
//! Cancel and an action capsule side by side, for a question that wants a yes or a no (a whole page
//! would be too much). Layout and behaviour follow iOS 26's alerts; the drawing and the words are
//! our own.

use egui::{Align2, Color32, Id, Rect, Sense, pos2, vec2};

use crate::theme::Tokens;

/// How long the alert takes to appear (s).
const FADE_S: f32 = 0.2;
/// The card's width, corners, margins and buttons (iOS 26's alerts: wider and rounder than before,
/// with capsule buttons).
const WIDTH: f32 = 300.0;
const RADIUS: f32 = 32.0;
const PAD: f32 = 20.0;
const GAP: f32 = 10.0;
const BUTTON_H: f32 = 46.0;

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
    let shown = egui::Area::new(id)
        .order(egui::Order::Foreground)
        .constrain(false)
        .fade_in(false)
        .pivot(Align2::CENTER_CENTER)
        .fixed_pos(content.center())
        .show(ctx, |ui| {
            ui.multiply_opacity(p);
            // the text first, to know how tall the card is
            let text = |text: &str, font: egui::FontId, color: Color32| {
                ui.painter().layout(crate::i18n::tr(text).to_string(), font, color, width - 2.0 * PAD)
            };
            let title = text(a.title, t.semibold(17.0), t.text);
            let message = text(a.message, t.font(15.0), t.text_label);
            let text_h = PAD + 4.0 + title.size().y + if a.message.is_empty() { 0.0 } else { 6.0 + message.size().y } + 20.0;
            let (card, _) = ui.allocate_exact_size(vec2(width, text_h + BUTTON_H + PAD), Sense::hover());
            crate::widgets::register(ctx, format!("alert:{}", a.id), card);
            let painter = ui.painter();
            crate::glass::paint(painter, card, RADIUS, Some(crate::glass::menu_fill(ctx)), 0.0);
            painter.galley(pos2(card.left() + PAD, card.top() + PAD + 4.0), title.clone(), t.text);
            if !a.message.is_empty() {
                painter.galley(pos2(card.left() + PAD, card.top() + PAD + 4.0 + title.size().y + 6.0), message, t.text_label);
            }
            // the buttons: two capsules side by side, the action's tinted (red when it destroys)
            let row_top = card.top() + text_h;
            let half = (width - 2.0 * PAD - GAP) / 2.0;
            for (i, (label, strong, red)) in [(a.cancel, false, false), (a.ok, true, a.destructive)].into_iter().enumerate() {
                let r = Rect::from_min_size(pos2(card.left() + PAD + i as f32 * (half + GAP), row_top), vec2(half, BUTTON_H));
                let resp = ui.interact(r, id.with(("button", i)), Sense::click());
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, crate::i18n::tr(label)));
                crate::widgets::register(ctx, if i == 0 { "button:alertCancel" } else { "button:alertOk" }, r);
                let k = crate::glass::press(ctx, &resp);
                let pill = r.expand(2.0 * k);
                let (fill, ink) = match (strong, red) {
                    (true, true) => (t.reject, Color32::WHITE),
                    (true, false) => (t.accent, Color32::WHITE),
                    // (a grey that shows on the card in either appearance)
                    _ => (if Tokens::is_dark(ctx) { t.hover } else { t.inset }, t.text),
                };
                ui.painter().rect_filled(pill, pill.height() / 2.0, fill);
                if k > 0.0 {
                    ui.painter().rect_filled(pill, pill.height() / 2.0, Color32::from_white_alpha((40.0 * k.min(1.0)) as u8));
                }
                ui.painter().text(
                    r.center(),
                    Align2::CENTER_CENTER,
                    crate::i18n::tr(label),
                    if strong { t.semibold(17.0) } else { t.font(17.0) },
                    ink,
                );
                if resp.clicked() {
                    if i == 0 {
                        cancel = true;
                    } else {
                        ok = true;
                    }
                }
            }
        });
    // it settles in from a touch larger, as iOS's alerts do
    let s = 1.0 + 0.1 * (1.0 - p);
    let c = content.center();
    ctx.transform_layer_shapes(shown.response.layer_id, egui::emath::TSTransform::new(c.to_vec2() * (1.0 - s), s));
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        cancel = true;
    }
    (cancel, ok)
}

/// The alert `id` isn't on screen this frame (its next showing fades in again).
pub fn hidden(ctx: &egui::Context, id: &str) {
    let _ = ctx.animate_bool_with_time(Id::new(("lc-alert", id)).with("in"), false, 0.0);
}
