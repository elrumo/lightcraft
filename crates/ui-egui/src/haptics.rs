//! Haptic feedback on touch hosts (iOS). The panels say what just happened ([`tap`]); once the
//! frame is laid out the app hands the frame's taps to the host ([`Services::haptic`](crate::Services::haptic)).
//! A host without one (desktop, web) drops them.

use egui::{Context, Id};

/// What a tap should feel like.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Haptic {
    /// A light tick: one step of a swipe, a slider's detent at its default, a photo chosen, a tool picked.
    Selection,
    /// A small thing landed: a rating kept, a photo settled, the bars shown or hidden, a reset.
    Light,
    /// A mode began: choosing photos by a long press.
    Medium,
    /// Something is going away: Delete.
    Warning,
}

/// The host's player ([`Services::haptic`](crate::Services::haptic)).
pub type PlayHaptic = Box<dyn Fn(Haptic)>;

/// Ask for `h` this frame (cheap anywhere; several of one kind in a frame are played once).
pub fn tap(ctx: &Context, h: Haptic) {
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Vec<Haptic>>(Id::new("lc-haptics")).push(h));
}

/// Hand the frame's taps to `play`, each kind once, in the order they were asked for. Always
/// empties the queue, so a host that plays nothing doesn't collect them.
pub(crate) fn flush(ctx: &Context, play: Option<&PlayHaptic>) {
    let taps = ctx.data_mut(|d| std::mem::take(d.get_temp_mut_or_default::<Vec<Haptic>>(Id::new("lc-haptics"))));
    let Some(play) = play else { return };
    let mut played = Vec::with_capacity(taps.len().min(4));
    for h in taps {
        if !played.contains(&h) {
            played.push(h);
            play(h);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[test]
    fn a_frame_plays_each_kind_once_in_order_and_empties_the_queue() {
        let ctx = Context::default();
        let played = Rc::new(RefCell::new(Vec::new()));
        let sink = played.clone();
        let play: PlayHaptic = Box::new(move |h| sink.borrow_mut().push(h));
        for h in [Haptic::Selection, Haptic::Light, Haptic::Selection, Haptic::Selection, Haptic::Warning] {
            tap(&ctx, h);
        }
        flush(&ctx, Some(&play));
        assert_eq!(*played.borrow(), [Haptic::Selection, Haptic::Light, Haptic::Warning]);
        flush(&ctx, Some(&play));
        assert_eq!(played.borrow().len(), 3, "nothing is played twice");
        // no player: the taps are dropped, not kept for a player that never comes
        tap(&ctx, Haptic::Medium);
        flush(&ctx, None);
        flush(&ctx, Some(&play));
        assert_eq!(played.borrow().len(), 3);
    }
}
