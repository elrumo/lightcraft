//! A brush stroke on the photo as one even shape: the trail of a round brush along the stroke's
//! samples, joined (a finger moving fast leaves no gaps), filled or outlined, never darker where it
//! overlaps itself — unlike a translucent dab drawn per sample. It is drawn from a distance field
//! over the stroke's surroundings (up to two texels per point), kept from frame to frame so that a
//! stroke being painted only draws in its new samples, and shown as a texture.

use std::sync::{Arc, Mutex, PoisonError};

use egui::{Color32, ColorImage, Id, Pos2, Rect, TextureHandle, TextureOptions, pos2, vec2};

/// How a trail is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Style {
    /// The brush's whole width.
    Fill,
    /// Its edge only, this many points wide.
    Outline(f32),
}

/// At most this many texels per trail (a big canvas on a high-density screen gets fewer per point).
const MAX_TEXELS: f32 = 2_000_000.0;
/// Room left round a trail's samples when its field is made, so that a growing stroke rarely has
/// to start a bigger one (points).
const ROOM: f32 = 64.0;

struct Field {
    /// The screen area the field covers (points).
    area: Rect,
    w: usize,
    h: usize,
    /// Distance (points) from each texel's centre to the trail drawn in so far.
    dist: Vec<f32>,
    radius: f32,
    style: Style,
    scale: f32,
    /// The trail's first sample (another one is another trail).
    first: Pos2,
    /// Samples drawn in.
    done: usize,
    tex: Option<TextureHandle>,
    /// Texels changed since the last upload: x0, y0, x1, y1 (exclusive).
    dirty: Option<[usize; 4]>,
}

impl Field {
    fn new(area: Rect, scale: f32, radius: f32, style: Style, first: Pos2, tex: Option<TextureHandle>) -> Field {
        let w = ((area.width() * scale).ceil() as usize).max(1);
        let h = ((area.height() * scale).ceil() as usize).max(1);
        Field { area, w, h, dist: vec![f32::INFINITY; w * h], radius, style, scale, first, done: 0, tex, dirty: Some([0, 0, w, h]) }
    }

    /// Texels per point across and down.
    fn per_point(&self) -> (f32, f32) {
        (self.w as f32 / self.area.width().max(1e-3), self.h as f32 / self.area.height().max(1e-3))
    }

    /// Draw in the segment `a`–`b` (a dab where they're the same point).
    fn segment(&mut self, a: Pos2, b: Pos2) {
        let reach = self.radius + 2.0;
        let (sx, sy) = self.per_point();
        let lo = a.min(b) - vec2(reach, reach) - self.area.min.to_vec2();
        let hi = a.max(b) + vec2(reach, reach) - self.area.min.to_vec2();
        let x0 = ((lo.x * sx).floor().max(0.0) as usize).min(self.w);
        let y0 = ((lo.y * sy).floor().max(0.0) as usize).min(self.h);
        let x1 = ((hi.x * sx).ceil().max(0.0) as usize).min(self.w);
        let y1 = ((hi.y * sy).ceil().max(0.0) as usize).min(self.h);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let ab = b - a;
        let len2 = ab.length_sq();
        for y in y0..y1 {
            let qy = self.area.min.y + (y as f32 + 0.5) / sy;
            for x in x0..x1 {
                let q = pos2(self.area.min.x + (x as f32 + 0.5) / sx, qy);
                let t = if len2 > 0.0 { ((q - a).dot(ab) / len2).clamp(0.0, 1.0) } else { 0.0 };
                let d = (q - (a + ab * t)).length();
                if let Some(v) = self.dist.get_mut(y * self.w + x)
                    && d < *v
                {
                    *v = d;
                }
            }
        }
        self.dirty = Some(match self.dirty {
            Some([a0, b0, a1, b1]) => [a0.min(x0), b0.min(y0), a1.max(x1), b1.max(y1)],
            None => [x0, y0, x1, y1],
        });
    }

    /// How much of a texel at distance `d` from the trail is covered (anti-aliased over a texel).
    fn cover(&self, d: f32) -> f32 {
        let inside = match self.style {
            Style::Fill => self.radius - d,
            Style::Outline(width) => width / 2.0 - (d - self.radius).abs(),
        };
        (inside * self.scale + 0.5).clamp(0.0, 1.0)
    }

    /// Send what changed to the texture.
    fn upload(&mut self, ctx: &egui::Context) {
        let Some([x0, y0, x1, y1]) = self.dirty.take() else { return };
        let mut px = Vec::with_capacity((x1 - x0) * (y1 - y0));
        for y in y0..y1 {
            for x in x0..x1 {
                let d = self.dist.get(y * self.w + x).copied().unwrap_or(f32::INFINITY);
                px.push(Color32::from_white_alpha((self.cover(d) * 255.0).round() as u8));
            }
        }
        let img = ColorImage::new([x1 - x0, y1 - y0], px);
        let whole = [x0, y0, x1, y1] == [0, 0, self.w, self.h];
        match &mut self.tex {
            Some(t) if t.size() == [self.w, self.h] => {
                if whole {
                    t.set(img, TextureOptions::LINEAR);
                } else {
                    t.set_partial([x0, y0], img, TextureOptions::LINEAR);
                }
            }
            // (a partial update needs the whole texture first: a new field always starts whole)
            _ if whole => self.tex = Some(ctx.load_texture("lc-trail", img, TextureOptions::LINEAR)),
            _ => {}
        }
    }
}

/// Draw the trail of a brush of `radius` (points) through `pts` (screen positions) in `color`, in
/// `clip`. `id` keeps the trail from frame to frame: one that grows (a stroke being painted) only
/// draws in its new samples. No samples: nothing is drawn and the trail is forgotten.
#[allow(clippy::too_many_arguments)]
pub fn paint(ctx: &egui::Context, painter: &egui::Painter, id: Id, clip: Rect, pts: &[Pos2], radius: f32, style: Style, color: Color32) {
    let cell = ctx.data_mut(|d| d.get_temp_mut_or_insert_with(id, || Arc::new(Mutex::new(None::<Field>))).clone());
    let mut slot = cell.lock().unwrap_or_else(PoisonError::into_inner);
    let (Some(&first), true) = (pts.first(), radius.is_finite() && radius > 0.0 && pts.iter().all(|p| p.x.is_finite() && p.y.is_finite())) else {
        *slot = None;
        return;
    };
    let reach = radius + 2.0;
    let bounds = Rect::from_points(pts).expand(reach).intersect(clip);
    if !bounds.is_positive() {
        return;
    }
    let fits = |f: &Field| f.radius == radius && f.style == style && f.first == first && f.done <= pts.len() && f.area.contains_rect(bounds);
    if !slot.as_ref().is_some_and(fits) {
        let area = bounds.expand(ROOM).intersect(clip);
        let scale = ctx.pixels_per_point().clamp(1.0, 2.0).min((MAX_TEXELS / area.area().max(1.0)).sqrt()).max(0.05);
        let tex = slot.take().and_then(|f| f.tex);
        *slot = Some(Field::new(area, scale, radius, style, first, tex));
    }
    let Some(f) = slot.as_mut() else { return };
    // each new sample joined to the one before it (the first is a dab)
    for i in f.done..pts.len() {
        let (Some(&b), a) = (pts.get(i), i.checked_sub(1).and_then(|j| pts.get(j)).copied()) else { break };
        f.segment(a.unwrap_or(b), b);
    }
    f.done = pts.len();
    f.upload(ctx);
    if let Some(tex) = &f.tex {
        painter.image(tex.id(), f.area, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(pts: &[Pos2], radius: f32) -> Field {
        let mut f = Field::new(Rect::from_min_size(Pos2::ZERO, vec2(200.0, 100.0)), 1.0, radius, Style::Fill, pts[0], None);
        f.segment(pts[0], pts[0]);
        for w in pts.windows(2) {
            f.segment(w[0], w[1]);
        }
        f
    }

    fn cover_at(f: &Field, x: usize, y: usize) -> f32 {
        f.cover(f.dist[y * f.w + x])
    }

    /// Two samples far apart (a finger moving fast) make one trail between them, as even in the
    /// middle as at its ends.
    #[test]
    fn samples_far_apart_make_one_even_trail() {
        let f = field(&[pos2(20.0, 50.0), pos2(180.0, 50.0)], 10.0);
        for x in [20, 60, 100, 140, 179] {
            assert_eq!(cover_at(&f, x, 50), 1.0, "covered at x = {x}");
        }
        assert_eq!(cover_at(&f, 100, 50 + 12), 0.0, "no wider than the brush");
        assert!(cover_at(&f, 100, 59) > 0.0 && cover_at(&f, 100, 59) <= 1.0);
    }

    /// Where a stroke crosses itself, it is covered once, not twice.
    #[test]
    fn a_trail_crossing_itself_is_not_darker() {
        let f = field(&[pos2(20.0, 20.0), pos2(180.0, 80.0), pos2(180.0, 20.0), pos2(20.0, 80.0)], 8.0);
        let crossing = cover_at(&f, 100, 50);
        assert_eq!(crossing, 1.0);
        assert!(f.dist.iter().all(|d| f.cover(*d) <= 1.0));
    }

    /// An outline is the edge of the whole trail: none of it inside the trail.
    #[test]
    fn an_outline_is_the_edge_of_the_whole_trail() {
        let mut f = field(&[pos2(20.0, 50.0), pos2(180.0, 50.0)], 10.0);
        f.style = Style::Outline(1.5);
        assert_eq!(cover_at(&f, 100, 50), 0.0, "nothing drawn inside");
        assert!(cover_at(&f, 100, 40) > 0.5 || cover_at(&f, 100, 39) > 0.5, "the edge is drawn");
    }

    /// Hostile samples (NaN, no radius) draw nothing and don't panic.
    #[test]
    fn bad_samples_draw_nothing() {
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let ctx = ui.ctx();
            let painter = ctx.layer_painter(egui::LayerId::background());
            let clip = Rect::from_min_size(Pos2::ZERO, vec2(100.0, 100.0));
            for (pts, r) in [(vec![pos2(f32::NAN, 1.0)], 5.0), (vec![pos2(1.0, 1.0)], 0.0), (vec![], 5.0), (vec![pos2(-500.0, -500.0)], 5.0)] {
                paint(ctx, &painter, Id::new("t"), clip, &pts, r, Style::Fill, Color32::RED);
            }
        });
        out.textures_delta.clear();
    }
}
