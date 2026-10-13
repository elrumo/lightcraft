//! Transitions both layouts share: how long things take to move, and a body that folds open and
//! shut under its header (an Edit section, a flyout, a sidebar section) rather than popping. Side
//! panels slide in `docking::show`, the filmstrip in `detail::show`; a photo opened from the grid
//! grows out of its thumbnail and flies back into it (`detail::view_changed`).

use egui::{Id, Rect, Ui, UiBuilder, vec2};

/// A section folding open or shut (s).
pub const FOLD_S: f32 = 0.18;
/// A side panel or the filmstrip sliding in or out (s).
pub const SLIDE_S: f32 = 0.2;
/// A view or a tool's controls fading in for another (s).
pub const FADE_S: f32 = 0.18;

/// How open `id` is (0 shut … 1 open), eased, following `open` over `secs`. The first call goes
/// straight to `open`: nothing moves at start-up.
pub fn openness(ctx: &egui::Context, id: Id, open: bool, secs: f32) -> f32 {
    ctx.animate_bool_with_time_and_easing(id, open, secs, egui::emath::easing::cubic_out)
}

/// The turn (radians) of a chevron that points right while `id` is shut and down while it is open,
/// in step with [`fold`]ing the same `id`.
pub fn chevron_turn(ctx: &egui::Context, id: Id, open: bool) -> f32 {
    openness(ctx, id.with("fold"), open, FOLD_S) * std::f32::consts::FRAC_PI_2
}

/// Draw `body` while `open` (in a vertical ui): opening, it grows from nothing to its height, and
/// shutting it shrinks away, what is below moving up and down with it. It is drawn in a child ui
/// of its own (salted with `id`) whether it moves or not, so its widgets keep their ids. Returns
/// what `body` returned, if it was drawn.
pub fn fold<R>(ui: &mut Ui, id: Id, open: bool, body: impl FnOnce(&mut Ui) -> R) -> Option<R> {
    let k = openness(ui.ctx(), id.with("fold"), open, FOLD_S);
    if k <= 0.0 {
        return None;
    }
    let height_id = id.with("fold-h");
    let at = ui.available_rect_before_wrap();
    let mut child = ui.new_child(UiBuilder::new().id_salt(id).max_rect(at).layout(*ui.layout()));
    // (its height is what it took the frame before; opening from nothing it starts at nothing)
    let shown = (k < 1.0).then(|| (ui.data(|d| d.get_temp::<f32>(height_id)).unwrap_or(0.0) * k).round());
    if let Some(h) = shown {
        child.shrink_clip_rect(Rect::from_min_size(at.min, vec2(at.width(), h)));
    }
    let out = body(&mut child);
    let used = child.min_rect();
    ui.data_mut(|d| d.insert_temp(height_id, used.height()));
    ui.advance_cursor_after_rect(Rect::from_min_size(used.min, vec2(used.width(), shown.unwrap_or(used.height()))));
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run `frames` frames 1/60 s apart, folding a 200 pt body open or shut at `toggle`; returns the
    /// height the body took in each frame (where the cursor ended up below it).
    fn heights(frames: usize, toggle: usize, start_open: bool) -> Vec<f32> {
        let ctx = egui::Context::default();
        let mut open = start_open;
        let mut out = Vec::new();
        for f in 0..frames {
            if f == toggle {
                open = !open;
            }
            let input = egui::RawInput { time: Some(f as f64 / 60.0), predicted_dt: 1.0 / 60.0, ..Default::default() };
            let mut full = ctx.run_ui(input, |ui| {
                ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
                let top = ui.cursor().top();
                fold(ui, Id::new("body"), open, |ui| ui.allocate_space(vec2(100.0, 200.0)));
                out.push(ui.cursor().top() - top);
            });
            full.textures_delta.clear();
        }
        out
    }

    #[test]
    fn a_body_grows_open_and_shrinks_shut_rather_than_popping() {
        // open from the start: whole at once (nothing animates at start-up)
        let h = heights(3, usize::MAX, true);
        assert_eq!(h, vec![200.0; 3]);
        // shut at frame 2: in between for a while, then gone
        let h = heights(40, 2, true);
        assert!(h[3] > 0.0 && h[3] < 200.0, "mid-fold: {:?}", &h[..8]);
        assert_eq!(*h.last().unwrap(), 0.0);
        // opened again: from nothing, through the middle, to the whole of it
        let h = heights(40, 2, false);
        assert_eq!(h[1], 0.0);
        assert!(h[5] > 0.0 && h[5] < 200.0, "mid-fold: {:?}", &h[..10]);
        assert_eq!(*h.last().unwrap(), 200.0);
        assert!(h.windows(2).skip(2).all(|w| w[1] >= w[0]), "only grows: {h:?}");
    }
}
