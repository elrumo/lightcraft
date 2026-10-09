//! iOS 26's "Liquid Glass" for the compact layout's floating controls (bar buttons, the tab bar,
//! menus, the add button) and the springs iOS moves them with. egui can't blur what is behind a
//! shape, so the glass is drawn the way it reads over a plain backdrop: a translucent fill, a sheen
//! fading down from its top, a thin rim that catches the light along its top and bottom edges, and
//! a soft shadow. Pressed, it swells a little and lights up, as the system's glass does under a
//! finger. Our own drawing, from watching the system's controls; no Apple artwork.

use egui::epaint::{CornerRadiusF32, Mesh, PathShape, PathStroke, Shadow, Vertex};
use egui::{Color32, Id, Painter, Pos2, Rect, Sense, Shape, Stroke, pos2, vec2};

use crate::icons::Icon;
use crate::theme::Tokens;

/// A glass button's diameter (iOS 26's bar buttons), inside a 44 pt touch target.
pub const BUTTON_D: f32 = 38.0;

/// Paint a piece of glass over `r` with corners of `radius` (half the height: a capsule). `tint`
/// colours it (an accent-tinted, "prominent" glass); `glow` (0–1) lights it up, for a press.
pub fn paint(p: &Painter, r: Rect, radius: f32, tint: Option<Color32>, glow: f32) {
    p.add(shape(p.ctx(), r, radius, tint, glow));
}

/// The thicker glass of menus and popovers (pass it as the tint): the rows on it must read over
/// whatever is behind.
pub fn menu_fill(ctx: &egui::Context) -> Color32 {
    // (nearly opaque: without a blur, what shows through would be sharp and in the way)
    if Tokens::is_dark(ctx) { Color32::from_rgba_unmultiplied(40, 40, 43, 252) } else { Color32::from_rgba_unmultiplied(248, 248, 250, 252) }
}

/// [`paint`]'s shapes, for drawing under what is painted first (`Painter::set` on a placeholder).
pub fn shape(ctx: &egui::Context, r: Rect, radius: f32, tint: Option<Color32>, glow: f32) -> Shape {
    let dark = Tokens::is_dark(ctx);
    let radius = radius.min(r.height() / 2.0).min(r.width() / 2.0).max(0.0);
    let mut out: Vec<Shape> = Vec::with_capacity(6);
    // the shadow: soft, larger for larger glass (a menu floats higher than a button)
    let lift = (r.height().min(r.width()) / 60.0).clamp(0.4, 1.6);
    let shadow = Shadow {
        offset: [0, (3.0 * lift) as i8],
        blur: (14.0 * lift) as u8,
        spread: 0,
        color: Color32::from_black_alpha(if dark { 90 } else { 34 }),
    };
    out.push(shadow.as_shape(r, radius).into());
    let fill = match tint {
        Some(c) => c,
        None if dark => Color32::from_rgba_unmultiplied(58, 58, 62, 178),
        None => Color32::from_rgba_unmultiplied(250, 250, 252, 200),
    };
    out.push(Shape::rect_filled(r, radius, fill));
    if glow > 0.0 {
        out.push(Shape::rect_filled(r, radius, Color32::from_white_alpha((if dark { 34.0 } else { 70.0 } * glow.min(1.0)) as u8)));
    }
    let mut outline = Vec::new();
    egui::epaint::tessellator::path::rounded_rectangle(&mut outline, r, CornerRadiusF32::same(radius));
    // the sheen: brightest along the top, gone by the middle
    let top = if tint.is_some() {
        60.0
    } else if dark {
        26.0
    } else {
        120.0
    };
    out.push(gradient(&outline, r, |y| Color32::from_white_alpha((top * (1.0 - y / 0.6).max(0.0)) as u8)));
    if !dark && tint.is_none() {
        // (on a white page, a faint grey edge says where the glass ends)
        out.push(Shape::rect_stroke(r, radius, Stroke::new(0.5, Color32::from_black_alpha(22)), egui::StrokeKind::Outside));
    }
    // the rim: a hairline lit at the top and, less, at the bottom, dim down the sides
    let (hi, lo) = if dark || tint.is_some() { (120.0, 22.0) } else { (230.0, 40.0) };
    let center = r.center();
    let half = (r.height() / 2.0).max(1.0);
    let rim = PathStroke::new_uv(1.0, move |_, at: Pos2| {
        let v = ((at.y - center.y) / half).clamp(-1.0, 1.0);
        let a = if v < 0.0 { lo + (hi - lo) * v * v } else { lo + (hi * 0.45 - lo) * v * v };
        Color32::from_white_alpha(a as u8)
    });
    out.push(Shape::Path(PathShape { points: outline, closed: true, fill: Color32::TRANSPARENT, stroke: rim }));
    Shape::Vec(out)
}

/// The convex `outline` (around `r`) filled with `color(y)`, `y` going 0 → 1 from its top to its bottom.
fn gradient(outline: &[Pos2], r: Rect, color: impl Fn(f32) -> Color32) -> Shape {
    let mut mesh = Mesh::default();
    let at = |p: Pos2| Vertex { pos: p, uv: egui::epaint::WHITE_UV, color: color(((p.y - r.top()) / r.height().max(1.0)).clamp(0.0, 1.0)) };
    mesh.vertices.push(at(r.center()));
    for p in outline {
        mesh.vertices.push(at(*p));
    }
    let n = outline.len() as u32;
    for i in 0..n {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % n.max(1));
    }
    Shape::mesh(mesh)
}

/// `target`, followed as by a spring (iOS's: it settles in about `response` seconds; a `damping`
/// below 1 overshoots a little and bounces back, as iOS's lively springs do). Its state (value,
/// velocity) lives under `id`; the first call starts at `target`.
pub fn spring(ctx: &egui::Context, id: Id, target: f32, response: f32, damping: f32) -> f32 {
    let (mut x, mut v) = ctx.data(|d| d.get_temp::<(f32, f32)>(id)).unwrap_or((target, 0.0));
    if !x.is_finite() || !v.is_finite() {
        (x, v) = (target, 0.0);
    }
    let dt = ctx.input(|i| i.stable_dt).clamp(0.0, 0.05);
    let w = std::f32::consts::TAU / response.max(0.05);
    // small steps keep the stiff springs stable whatever the frame rate
    let steps = (dt / (1.0 / 240.0)).ceil().clamp(1.0, 16.0);
    let h = dt / steps;
    for _ in 0..steps as u32 {
        let a = -w * w * (x - target) - 2.0 * damping * w * v;
        v += a * h;
        x += v * h;
    }
    if (x - target).abs() < 0.01 && v.abs() < 0.1 {
        (x, v) = (target, 0.0);
    } else {
        ctx.request_repaint();
    }
    ctx.data_mut(|d| d.insert_temp(id, (x, v)));
    x
}

/// iOS's bouncy spring as an easing curve over 0 → 1: it overshoots by a few percent and settles
/// by `t = 1` (for `animate_bool`-driven entrances, e.g. a menu growing out of its button).
pub fn bounce(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let (zeta, w) = (0.62_f32, 13.0_f32);
    let wd = w * (1.0 - zeta * zeta).sqrt();
    let x = |t: f32| 1.0 - (-zeta * w * t).exp() * ((wd * t).cos() + zeta * w / wd * (wd * t).sin());
    // (pinned to end exactly at 1)
    x(t) + t * (1.0 - x(1.0))
}

/// How far a press on `resp` has swollen its glass (0–1, springing back after the finger lifts).
pub fn press(ctx: &egui::Context, resp: &egui::Response) -> f32 {
    let down = resp.is_pointer_button_down_on() && resp.sense.senses_click();
    spring(ctx, resp.id.with("glass-press"), if down { 1.0 } else { 0.0 }, 0.3, 0.6).max(0.0)
}

/// A round glass button with `icon` (`key` is its automation id), in a 44 pt touch target, as iOS
/// 26's bars have them: back, close, "…". `tint` makes it a prominent (coloured) one.
pub fn circle_button(ui: &mut egui::Ui, key: &str, icon: Icon, tip: &str, enabled: bool, tint: Option<Color32>) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(vec2(44.0, 44.0), if enabled { Sense::click() } else { Sense::hover() });
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, crate::i18n::tr(tip)));
    crate::widgets::register(ui.ctx(), key, r);
    paint_circle(ui, r.center(), &resp, icon, enabled, tint);
    resp
}

/// [`circle_button`]'s drawing, centred on `c`.
pub fn paint_circle(ui: &egui::Ui, c: Pos2, resp: &egui::Response, icon: Icon, enabled: bool, tint: Option<Color32>) {
    let t = Tokens::get(ui.ctx());
    let k = press(ui.ctx(), resp);
    let d = BUTTON_D * (1.0 + 0.1 * k);
    paint(ui.painter(), Rect::from_center_size(c, vec2(d, d)), d / 2.0, tint, k);
    let ink = match (enabled, tint) {
        (false, _) => t.text_disabled,
        (true, Some(_)) => Color32::WHITE,
        (true, None) => t.text,
    };
    crate::icons::paint(ui.painter(), Rect::from_center_size(c, vec2(20.0, 20.0)), icon, ink);
}

/// A glass capsule with `label` in it (`key` is its automation id): Select, Cancel, a bar's text
/// action. `tint` makes it a prominent one (white text on the colour).
pub fn capsule_button(ui: &mut egui::Ui, key: &str, label: &str, tint: Option<Color32>) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    let label = crate::i18n::tr(label);
    let ink = if tint.is_some() { Color32::WHITE } else { t.text };
    let galley = ui.painter().layout_no_wrap(label.to_string(), t.semibold(15.0), ink);
    let w = galley.size().x + 30.0;
    let (r, resp) = ui.allocate_exact_size(vec2(w.max(44.0), 44.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    crate::widgets::register(ui.ctx(), key, r);
    let k = press(ui.ctx(), &resp);
    let pill = Rect::from_center_size(r.center(), vec2(w, BUTTON_D) * (1.0 + 0.08 * k));
    paint(ui.painter(), pill, pill.height() / 2.0, tint, k);
    ui.painter().galley(pill.center() - galley.size() / 2.0, galley, ink);
    resp
}

/// One item of a [`group`]: (automation id, icon, what a screen reader calls it, enabled).
pub type GroupItem<'a> = (&'a str, Icon, &'a str, bool);

/// Icon buttons sharing one glass capsule, as iOS 26 groups a bar's trailing items (undo, share,
/// "…"). Laid out in the ui's direction; the responses come in `items`' order.
pub fn group(ui: &mut egui::Ui, items: &[GroupItem]) -> Vec<egui::Response> {
    const W: f32 = 42.0;
    let t = Tokens::get(ui.ctx());
    let n = items.len() as f32;
    let (r, _) = ui.allocate_exact_size(vec2(W * n + 8.0, 44.0), Sense::hover());
    let pill = Rect::from_center_size(r.center(), vec2(r.width(), BUTTON_D));
    let under = ui.painter().add(Shape::Noop);
    let rtl = ui.layout().main_dir() == egui::Direction::RightToLeft;
    let mut glow: f32 = 0.0;
    let out: Vec<egui::Response> = items
        .iter()
        .enumerate()
        .map(|(i, (key, icon, tip, enabled))| {
            let slot = if rtl { n - 1.0 - i as f32 } else { i as f32 };
            let cell = Rect::from_min_size(pos2(r.left() + 4.0 + slot * W, r.top()), vec2(W, 44.0));
            let resp = ui.interact(cell, Id::new(("lc-glass-group", *key)), if *enabled { Sense::click() } else { Sense::hover() });
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, *enabled, crate::i18n::tr(tip)));
            crate::widgets::register(ui.ctx(), *key, cell);
            let k = press(ui.ctx(), &resp);
            glow = glow.max(k);
            let ink = if *enabled { t.text } else { t.text_disabled };
            if k > 0.0 {
                // (the pressed one lights up inside the capsule)
                let dark = Tokens::is_dark(ui.ctx());
                let spot = Rect::from_center_size(cell.center(), vec2(W - 2.0, BUTTON_D - 6.0));
                ui.painter().rect_filled(spot, spot.height() / 2.0, Color32::from_white_alpha((if dark { 30.0 } else { 90.0 } * k) as u8));
            }
            crate::icons::paint(ui.painter(), Rect::from_center_size(cell.center(), vec2(21.0, 21.0)), *icon, ink);
            resp
        })
        .collect();
    // (the glass goes under the icons, which were drawn first)
    let swell = pill.expand2(vec2(3.0, 2.0) * glow);
    ui.painter().set(under, shape(ui.ctx(), swell, swell.height() / 2.0, None, glow * 0.4));
    out
}
