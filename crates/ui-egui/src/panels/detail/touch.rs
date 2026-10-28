//! The photo on a phone (the compact layout): a drag across a fitted photo carries it to the next
//! or the previous one, a drag down puts it away, up and down in review mode rate (left half) and
//! flag (right half) it, a finger held on it shows the Before, and a tap shows or hides the bars.
//! The loupe (`detail.rs`) runs these before it draws the photo and reads what they made of it.

use egui::{Color32, Pos2, Rect, Vec2, pos2, vec2};
use lightcraft_catalog::PhotoId;
use serde_json::json;

use crate::LightcraftApp;
use crate::haptics::{self, Haptic};
use crate::theme::Tokens;

/// How far (points) an up / down swipe must travel for each star, and for each step of the flag
/// (reject, none, pick); half of it already counts, so the readout answers the first movement.
const STAR_STEP_PT: f32 = 44.0;

const FLAG_STEP_PT: f32 = 56.0;

/// The gap (points) between photos in the phone loupe's carousel.
const SLIDE_GAP: f32 = 16.0;

/// A sideways drag settles on the next photo when it carried the photo this fraction of its width
/// across, or was this fast (points per second) and went a few points.
const SLIDE_FRACTION: f32 = 0.25;

const SLIDE_FLING: f32 = 600.0;

/// How quickly a let-go photo settles: the distance left shrinks by e^-10 each second.
const SLIDE_SETTLE: f32 = 10.0;

/// A tap shows or hides the bars once no second finger has come down this many seconds after it
/// lifted (a double tap's second comes sooner, and zooms).
pub(super) const TAP_GAP_S: f64 = 0.2;

/// How long the ring that answers a tap on the phone loupe lasts (seconds).
pub(super) const TAP_RING_S: f64 = 0.35;

/// A finger held still this long on the photo shows the Before until it lifts (a slow tap is well
/// under it; iOS's own long press takes half a second).
pub(super) const HOLD_S: f64 = 0.4;

/// How long the review readout's pop lasts (seconds).
const POP_S: f64 = 0.45;

/// A one-finger drag on the phone loupe, kept from frame to frame.
#[derive(Clone, Copy, Default)]
pub(super) struct Swipe {
    /// Where the photo is, as points from rest, and where it was when the finger came down.
    off: f32,
    base: f32,
    /// The finger: how far it has moved, how fast it was going sideways, where it came down, and
    /// whether the drag is sideways (the carousel) or up / down (rating), known once it has moved.
    d: Vec2,
    v: f32,
    from: Pos2,
    sideways: Option<bool>,
    /// The last frame this ran, and the last rating / flag change (when, was it the flag?).
    t: f64,
    pop: Option<(f64, bool)>,
    /// A rating / flag swipe: the (stars, flag) the photo had when it began, and what the finger
    /// has made of them so far, which the readout shows (and which is kept for the frame the finger
    /// lifts, when the photo's own are not yet updated). The flag is 0 reject, 1 none, 2 pick.
    start: (u8, u8),
    shown: Option<(u8, u8)>,
    /// The photo this ran for last, to slide in a change made by anything but the finger.
    id: u64,
    /// A pull down (back to the grid): the photo's offset from rest, and whether the finger has it.
    pull: Vec2,
    pulling: bool,
}

/// What a one-finger drag on the phone loupe made of the photo this frame.
#[derive(Clone, Copy, Default)]
pub(super) struct Moved {
    /// Sideways from rest (the carousel), in points.
    pub(super) slide: f32,
    /// Pulled down (and sideways with it) from rest, in points.
    pub(super) pull: Vec2,
    /// Let go far or fast enough down: back to the grid.
    pub(super) dismiss: bool,
}

/// A pull down this far (points), or a flick down this fast (points per second), goes back to the grid.
const PULL_DISMISS: f32 = 110.0;

const PULL_FLING: f32 = 700.0;

/// How much smaller the photo is pulled all the way down the screen.
pub(super) const PULL_SHRINK: f32 = 0.45;

/// How far the photo is pulled down this frame, as a part of the screen (0..1): the grid shows
/// through behind it and the bars fade as it goes (`compact::content`, `compact::top_bar`).
pub fn pulled(ctx: &egui::Context) -> f32 {
    ctx.data(|d| d.get_temp::<f32>(egui::Id::unique("loupe-pull"))).unwrap_or(0.0)
}

/// The phone loupe's last tap: when and where it lifted, whether the bars are still to change for
/// it, and whether they have.
#[derive(Clone, Copy, Default)]
pub(super) struct LoupeTap {
    pub(super) at: f64,
    pub(super) pos: Pos2,
    pub(super) pending: bool,
    pub(super) toggled: bool,
}

/// A finger held on the photo to see the Before, kept from frame to frame.
#[derive(Clone, Copy, Default)]
pub(super) struct BeforeHold {
    /// The press this follows (when it came down), and whether it can no longer become a hold.
    pub(super) press: Option<f64>,
    pub(super) void: bool,
    /// The Before is showing.
    pub(super) on: bool,
    /// The finger lifted from a hold this frame: its release is no tap.
    pub(super) lifted: bool,
}

/// The stars and flag (0 reject, 1 none, 2 pick) of a photo, as a swipe changes them.
fn stars_and_flag(photo: &lightcraft_catalog::Photo) -> (u8, u8) {
    use lightcraft_catalog::Flag;
    (
        photo.rating,
        match photo.flag {
            Flag::Reject => 0,
            Flag::None => 1,
            Flag::Pick => 2,
        },
    )
}

/// What `dy` points of drag (up is negative) make of `start` on one side: the stars (left) or the
/// flag (right), a step for each stretch of the finger.
fn swiped(start: (u8, u8), flag_side: bool, dy: f32) -> (u8, u8) {
    let up = -dy;
    if flag_side {
        (start.0, (f32::from(start.1) + (up / FLAG_STEP_PT).round()).clamp(0.0, 2.0) as u8)
    } else {
        ((f32::from(start.0) + (up / STAR_STEP_PT).round()).clamp(0.0, 5.0) as u8, start.1)
    }
}

/// Is there a photo before / after `id` in view order?
fn has_neighbours(app: &mut LightcraftApp, id: PhotoId) -> (bool, bool) {
    let ids = app.session.visible();
    ids.iter().position(|p| *p == id).map_or((false, false), |i| (i > 0, i + 1 < ids.len()))
}

/// What a one-finger drag does on the phone loupe, run before the photo is drawn so it follows the
/// finger: sideways across a fitted photo carries it, with its neighbours alongside, and letting go
/// settles on the next / previous photo (far or fast enough) or back; up / down, in review mode,
/// rates (left half) or flags (right half). Returns the photo's offset from rest, in points.
pub(super) fn swipe(app: &mut LightcraftApp, ui: &egui::Ui, resp: &egui::Response, canvas: Rect, id: PhotoId, live: bool) -> Moved {
    let key = egui::Id::unique("loupe-swipe");
    let mut s: Swipe = ui.data(|d| d.get_temp(key)).unwrap_or_default();
    let now = ui.input(|i| i.time);
    if now - s.t > 0.25 {
        (s.off, s.id) = (0.0, id.0); // (it was off screen a while: it starts at rest)
        (s.pull, s.pulling) = (Vec2::ZERO, false);
    }
    let mut dismiss = false;
    s.t = now;
    let w = canvas.width() + SLIDE_GAP;
    if !resp.dragged() && !resp.drag_stopped() {
        s.shown = None;
    }
    // another photo than last time, and not by the finger (a button, an arrow key): it slides in
    // from the side it lies on
    if s.id != id.0 {
        let ids = app.session.visible();
        let at = |n: u64| ids.iter().position(|p| p.0 == n);
        if let (Some(a), Some(b)) = (at(s.id), at(id.0))
            && a.abs_diff(b) == 1
        {
            s.off = if b > a { w } else { -w };
        }
        s.id = id.0;
    }
    if resp.drag_started() {
        // a photo still settling can be caught
        (s.base, s.d, s.sideways) = (s.off, Vec2::ZERO, None);
        s.from = ui.input(|i| i.pointer.press_origin()).unwrap_or(canvas.center());
    }
    // (the photo drawn this frame: the one before a change of photo, which comes next frame)
    let mut drawn = None;
    if live && resp.dragged() {
        s.d = resp.total_drag_delta().unwrap_or(s.d);
        s.v = ui.input(|i| i.pointer.velocity().x);
        if s.sideways.is_none() && s.d.length() > 8.0 {
            s.sideways = Some(s.d.x.abs() > s.d.y.abs());
            // down, with no rating to give: the photo is pulled back towards the grid
            s.pulling = s.sideways == Some(false) && !app.ui.review && s.d.y > 0.0;
        }
        if s.sideways == Some(true) {
            let (prev, next) = has_neighbours(app, id);
            let want = s.base + s.d.x;
            // nothing beyond the first and last photo: it gives way only grudgingly
            s.off = if (want > 0.0 && !prev) || (want < 0.0 && !next) { want * 0.3 } else { want };
        } else if s.pulling {
            // the photo follows the finger down (and sideways with it); back up past where it began
            // it gives way only grudgingly
            s.pull = vec2(s.d.x, if s.d.y >= 0.0 { s.d.y } else { s.d.y * 0.3 });
        } else if s.sideways == Some(false) && app.ui.review {
            // up / down: the stars (left half) or the flag (right half) follow the finger
            if s.shown.is_none() {
                s.start = app.session.catalog.photo(id).map(|p| stars_and_flag(p)).unwrap_or_default();
            }
            let flag_side = s.from.x > canvas.center().x;
            let now_shown = swiped(s.start, flag_side, s.d.y);
            if s.shown != Some(now_shown) && s.shown.is_some() {
                s.pop = Some((now, flag_side));
            }
            // a tick for each star or flag step the finger reaches (or gives up)
            if s.shown.unwrap_or(s.start) != now_shown {
                haptics::tap(ui.ctx(), Haptic::Selection);
            }
            s.shown = Some(now_shown);
        }
    } else {
        if resp.drag_stopped() && s.pulling {
            let vy = ui.input(|i| i.pointer.velocity().y);
            dismiss = s.pull.y > PULL_DISMISS || (vy > PULL_FLING && s.pull.y > 24.0);
            if dismiss {
                haptics::tap(ui.ctx(), Haptic::Light);
            }
            (s.pulling, s.sideways) = (false, None);
        }
        if resp.drag_stopped() {
            match s.sideways.filter(|_| live) {
                Some(true) => {
                    let (prev, next) = has_neighbours(app, id);
                    let dir = s.off.signum(); // -1: towards the next photo
                    let far = s.off.abs() > w * SLIDE_FRACTION || (s.v * dir > SLIDE_FLING && s.off.abs() > 24.0);
                    if far && if dir < 0.0 { next } else { prev } {
                        drawn = Some(s.off);
                        let _ = app.run(if dir < 0.0 { "library.next" } else { "library.previous" }, json!({}));
                        haptics::tap(ui.ctx(), Haptic::Light);
                        s.off -= dir * w; // (the new photo is where its neighbour was)
                        s.id = app.session.active().map_or(s.id, |p| p.0);
                    }
                }
                Some(false) if app.ui.review => {
                    // (the readout has been showing it all along; now it is kept)
                    if let Some(end) = s.shown.filter(|end| *end != s.start) {
                        let flag_side = s.from.x > canvas.center().x;
                        rate_to(app, id, flag_side, end);
                        haptics::tap(ui.ctx(), Haptic::Light);
                        s.pop = Some((now, flag_side));
                        ui.ctx().request_repaint();
                    }
                }
                _ => {}
            }
            s.sideways = None;
        }
        if drawn.is_none() && s.off != 0.0 {
            s.off *= (-SLIDE_SETTLE * ui.input(|i| i.stable_dt)).exp();
            if s.off.abs() < 0.5 {
                s.off = 0.0;
            }
        }
        // a pull let go short of the grid springs back (one that went back is drawn where it was)
        if !s.pulling && s.pull != Vec2::ZERO && !dismiss {
            s.pull *= (-SLIDE_SETTLE * 1.4 * ui.input(|i| i.stable_dt)).exp();
            if s.pull.length() < 0.5 {
                s.pull = Vec2::ZERO;
            }
        }
    }
    if s.off != 0.0 || s.pull != Vec2::ZERO {
        ui.ctx().request_repaint();
    }
    let pull = s.pull;
    if dismiss {
        s.pull = Vec2::ZERO;
    }
    ui.data_mut(|d| d.insert_temp(key, s));
    Moved { slide: drawn.unwrap_or(s.off), pull, dismiss }
}

/// Press and hold: a finger held still on the photo for [`HOLD_S`] shows the Before, as holding `\`
/// does, until it lifts (as Lightroom's mobile app shows the original). A finger that moves first is
/// a swipe or a pan and a second one a pinch; touch only, since a mouse held down starts a pan. Run
/// before the photo is drawn; `allowed`: no tool takes the press.
pub(super) fn before_hold(ui: &egui::Ui, resp: &egui::Response, allowed: bool) -> BeforeHold {
    let key = egui::Id::unique("loupe-hold");
    let mut h: BeforeHold = ui.data(|d| d.get_temp(key)).unwrap_or_default();
    let (touch, start, moved, fingers, now) = ui.input(|i| {
        let moved = i.pointer.total_drag_delta().map_or(0.0, |d| d.length());
        (i.any_touches(), i.pointer.press_start_time(), moved, i.multi_touch().is_some(), i.time)
    });
    let slop = ui.ctx().options(|o| o.input_options.max_click_dist);
    match start.filter(|_| touch) {
        // no finger down: the After is back
        None => h = BeforeHold { lifted: h.on, ..Default::default() },
        Some(t0) => {
            if h.press != Some(t0) {
                h = BeforeHold { press: Some(t0), ..Default::default() };
            }
            h.lifted = false;
            if !h.on && !h.void {
                let held = now - t0;
                if moved > slop || fingers || !allowed {
                    h.void = true;
                } else if held < HOLD_S {
                    ui.ctx().request_repaint_after(std::time::Duration::try_from_secs_f64(HOLD_S - held).unwrap_or_default());
                } else if resp.is_pointer_button_down_on() || resp.long_touched() {
                    // (it came down on the photo, not on a button over it)
                    h.on = true;
                    haptics::tap(ui.ctx(), Haptic::Medium);
                } else {
                    h.void = true;
                }
            }
        }
    }
    ui.data_mut(|d| d.insert_temp(key, h));
    h
}

/// Review mode's readout over the photo: its stars at the bottom left and its flag at the bottom
/// right. It fades in as the bars go. A swipe shows its stars and flag as the finger makes them
/// (`Swipe::shown`): each star fills with a little pop as it is reached, the flag changes into the
/// next one, the pill pops at every step and the side under the finger swells.
pub(super) fn review_hud(ui: &egui::Ui, p: &egui::Painter, canvas: Rect, photo: &lightcraft_catalog::Photo, on: bool) {
    use crate::icons::{Icon, paint};
    let ctx = ui.ctx();
    let fade = ctx.animate_bool_with_time(egui::Id::unique("review-hud"), on, 0.25);
    if fade <= 0.0 {
        return;
    }
    let t = Tokens::get(ctx);
    let (s, now) = (ui.data(|d| d.get_temp::<Swipe>(egui::Id::unique("loupe-swipe"))).unwrap_or_default(), ui.input(|i| i.time));
    let (rating, flag) = s.shown.unwrap_or_else(|| stars_and_flag(photo));
    // how much bigger than its size at rest a side is: a pop after a change, a swell under the finger
    let size = |flag: bool| {
        let u = s.pop.filter(|(_, f)| *f == flag).map_or(1.0, |(at, _)| (now - at) / POP_S);
        let pop = if u < 1.0 { (u as f32 * std::f32::consts::PI).sin() } else { 0.0 };
        let touched = s.sideways == Some(false) && (s.from.x > canvas.center().x) == flag;
        let swell = ctx.animate_value_with_time(egui::Id::unique(("review-swell", flag)), if touched { 0.12 } else { 0.0 }, 0.12);
        if u < 1.0 || swell > 0.0 {
            ctx.request_repaint();
        }
        1.0 + 0.35 * pop + swell
    };
    let (h, y) = (44.0, canvas.bottom() - 16.0 - 22.0);
    // (the glyphs' colours follow the appearance: dark pills under light ones, light under dark)
    let fill =
        if crate::glass::is_dark(ctx) { Color32::from_black_alpha((150.0 * fade) as u8) } else { Color32::from_white_alpha((210.0 * fade) as u8) };
    let pill = |c: Pos2, w: f32, k: f32| p.rect_filled(Rect::from_center_size(c, vec2(w, h) * k), h / 2.0 * k, fill);
    // the stars
    let (star, gap) = (26.0, 6.0);
    let w = 5.0 * star + 4.0 * gap + 28.0;
    let k = size(false);
    // (a pill grows from its screen edge, not out of it)
    let c = pos2(canvas.left() + 16.0 + w * k / 2.0, y);
    pill(c, w, k);
    crate::widgets::register_geometry(ctx, "review:stars", Rect::from_center_size(c, vec2(w, h) * k));
    for i in 0..5u8 {
        // 0 → 1 as the star is reached; it swells on the way (and shrinks back as it is given up)
        let f = ctx.animate_bool_with_time(egui::Id::unique(("review-star", i)), rating > i, 0.14);
        let bump = 1.0 + 1.6 * f * (1.0 - f);
        let at = c + vec2((f32::from(i) - 2.0) * (star + gap), 0.0) * k;
        let r = Rect::from_center_size(at, vec2(star, star) * k * bump);
        paint(p, r, Icon::Star, t.text_dim.gamma_multiply(fade * (1.0 - f)));
        paint(p, r, Icon::StarFilled, t.star.gamma_multiply(fade * f));
    }
    // the flag: the three states cross-fade
    let k = size(true);
    let c = pos2(canvas.right() - 16.0 - h * k / 2.0, y);
    pill(c, h, k);
    crate::widgets::register_geometry(ctx, "review:flag", Rect::from_center_size(c, vec2(h, h) * k));
    for (ix, icon, color) in [(0u8, Icon::FlagReject, t.reject), (1, Icon::FlagPick, t.text_dim), (2, Icon::FlagPick, t.pick)] {
        let f = ctx.animate_bool_with_time(egui::Id::unique(("review-flag", ix)), flag == ix, 0.14);
        if f > 0.0 {
            paint(p, Rect::from_center_size(c, vec2(star, star) * k * (0.8 + 0.2 * f)), icon, color.gamma_multiply(fade * f));
        }
    }
}

/// Rating by swiping on a phone, as Lightroom's mobile app does when reviewing: up or down on the
/// photo's left half adds or takes stars, a stretch of finger for each; on its right half, up picks
/// and down rejects (towards the other flag from one of them first clears it). The review readout
/// follows the finger (`swiped`); this stores what it showed when the finger lifts.
pub(super) fn rate_to(app: &mut LightcraftApp, id: PhotoId, flag_side: bool, (rating, flag): (u8, u8)) {
    if flag_side {
        let key = match flag {
            0 => "reject",
            2 => "pick",
            _ => "none",
        };
        let _ = app.run("photo.flag", json!({"ids": [id.0], "flag": key}));
    } else {
        let _ = app.run("photo.rate", json!({"ids": [id.0], "rating": rating}));
    }
}

/// A single tap shows or hides the bars (review mode) as soon as it can't be a double tap's first,
/// and is answered at once: a light haptic, and a ring where the finger lifted. `clicked` /
/// `double_clicked`: the loupe's taps this frame (not a lift from a hold on the Before).
pub(super) fn tap(app: &mut LightcraftApp, ui: &egui::Ui, resp: &egui::Response, canvas: Rect, clicked: bool, double_clicked: bool) {
    let (key, now) = (egui::Id::unique("loupe-tap"), ui.input(|i| i.time));
    let mut tap = ui.data(|m| m.get_temp(key)).unwrap_or(LoupeTap { at: f64::NEG_INFINITY, ..Default::default() });
    if double_clicked {
        // (a double tap slower than the wait has already changed the bars: they go back)
        if tap.toggled {
            let _ = app.run("view.reviewMode", json!({}));
        }
        (tap.pending, tap.toggled) = (false, false);
    } else if clicked {
        tap = LoupeTap { at: now, pos: resp.interact_pointer_pos().unwrap_or(canvas.center()), pending: true, toggled: false };
        haptics::tap(ui.ctx(), Haptic::Light);
    } else if resp.drag_started() {
        tap.pending = false; // (a tap then a swipe: the swipe)
    }
    // (a finger down on the photo again may be a double tap's second: wait for it to lift)
    if tap.pending && !resp.is_pointer_button_down_on() {
        if now - tap.at >= TAP_GAP_S {
            let _ = app.run("view.reviewMode", json!({}));
            (tap.pending, tap.toggled) = (false, true);
        } else {
            ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(tap.at + TAP_GAP_S - now));
        }
    }
    let u = ((now - tap.at) / TAP_RING_S) as f32;
    if (0.0..1.0).contains(&u) {
        // it swells (easing out) and fades; a dark halo keeps it seen on a light background
        let (r, a) = (18.0 + 14.0 * (1.0 - (1.0 - u).powi(3)), 1.0 - u);
        let p = ui.painter_at(canvas);
        p.circle(tap.pos, r, Color32::from_white_alpha((50.0 * a) as u8), egui::Stroke::new(3.5, Color32::from_black_alpha((40.0 * a) as u8)));
        p.circle_stroke(tap.pos, r, egui::Stroke::new(2.0, Color32::from_white_alpha((220.0 * a) as u8)));
        ui.ctx().request_repaint();
    }
    ui.data_mut(|m| m.insert_temp(key, tap));
}

/// Is a finger on a touch screen behind this frame's pointer input (rather than a mouse)? Also in
/// the frame a finger lifts: its touch has ended then, but the end comes with the pointer's release.
pub(super) fn touch_input(ui: &egui::Ui) -> bool {
    ui.input(|i| i.any_touches() || i.events.iter().any(|e| matches!(e, egui::Event::Touch { .. })))
}

/// How far (points) from a handle, a pin or a corner a finger still grabs it (the mark is 44 pt wide with margin).
pub(super) const TOUCH_HANDLE_REACH: f32 = 28.0;
