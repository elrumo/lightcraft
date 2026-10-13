//! The crop model.
//!
//! The image (w × h pixels after orientation) is rotated by `angle` about its centre. The crop is an
//! axis-aligned rectangle in that rotated frame, stored *normalized* to the image size
//! (`x / w`, `y / h`), so it is independent of preview resolution. The crop must stay inside the
//! rotated image (Lightroom's "Constrain to image").
//!
//! With **Generative Expand** the crop may reach past the photo, up to [`EXPAND_MAX`] of its size
//! beyond each edge. That is the same rule on a bigger image: the photo grown by `EXPAND_MAX` on
//! every side, about the same centre ([`CropGeometry::grown`]). The `*_in` functions take an
//! `expand` flag and apply the plain rule on that image.

use serde::{Deserialize, Serialize};

use crate::{Affine, Point, Rect, Vec2};

/// How far Generative Expand lets a crop reach past the photo on each side, as a fraction of the
/// photo's width (left and right) and height (top and bottom): the canvas can be up to three times
/// the photo's size, enough for a landscape photo to become a 9 × 16 portrait.
pub const EXPAND_MAX: f64 = 1.0;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CropGeometry {
    /// Normalized crop rectangle in the rotated frame (0..1 when angle = 0).
    pub rect: Rect,
    /// Straighten angle in degrees, clockwise positive, −45..45.
    pub angle: f64,
}

impl Default for CropGeometry {
    fn default() -> Self {
        Self { rect: Rect::UNIT, angle: 0.0 }
    }
}

impl CropGeometry {
    pub fn is_identity(&self) -> bool {
        self.angle == 0.0 && self.rect == Rect::UNIT
    }

    /// Crop rectangle in pixels of the rotated frame for an image of `w × h`.
    pub fn rect_px(&self, w: f64, h: f64) -> Rect {
        self.rect.scale(w, h)
    }

    /// Transform from *output* (cropped) pixel coordinates to *source* (oriented image) pixel
    /// coordinates, for an output of `out_w × out_h` pixels rendered from a `w × h` source.
    pub fn output_to_source(&self, w: f64, h: f64, out_w: f64, out_h: f64) -> Affine {
        let r = self.rect_px(w, h);
        // output -> crop rect in rotated frame
        let to_rot = Affine::translate(Vec2::new(r.x0, r.y0)) * Affine::scale(r.width() / out_w, r.height() / out_h);
        // rotated frame -> source: inverse rotation about the image centre
        let c = Point::new(w / 2.0, h / 2.0);
        Affine::rotate_about(-self.angle.to_radians(), c) * to_rot
    }

    /// Whether every crop corner lies within the rotated image.
    pub fn is_within_image(&self, w: f64, h: f64) -> bool {
        let r = self.rect_px(w, h);
        let back = Affine::rotate_about(-self.angle.to_radians(), Point::new(w / 2.0, h / 2.0));
        r.corners().iter().all(|p| {
            let q = back.apply(*p);
            q.x >= -1e-6 * w && q.y >= -1e-6 * h && q.x <= w * (1.0 + 1e-6) && q.y <= h * (1.0 + 1e-6)
        })
    }

    /// Shrink the crop (about its centre, keeping aspect) and then shift it until it fits inside the
    /// rotated image.
    pub fn constrained(&self, w: f64, h: f64) -> CropGeometry {
        if self.is_within_image(w, h) {
            return *self;
        }
        let r = self.rect_px(w, h);
        // Try moving towards the image centre first.
        let c = Point::new(w / 2.0, h / 2.0);
        let mut best = *self;
        for i in 1..=32 {
            let t = i as f64 / 32.0;
            let nc = r.center().lerp(c, t);
            let cand = CropGeometry { rect: Rect::from_center(nc, r.width(), r.height()).scale(1.0 / w, 1.0 / h), angle: self.angle };
            if cand.is_within_image(w, h) {
                return cand;
            }
            best = cand;
        }
        // Then shrink around the centre.
        let s = max_inscribed_scale(w, h, self.angle, r.width(), r.height());
        let nr = Rect::from_center(c, r.width() * s, r.height() * s);
        let _ = best;
        CropGeometry { rect: nr.scale(1.0 / w, 1.0 / h), angle: self.angle }
    }

    /// This crop turned to `angle_deg` on a `w × h` image: kept as it is when it still fits inside
    /// the rotated image, else the largest crop of its aspect that fits, centred (Lightroom's
    /// "Constrain to image"). Every way of setting the angle goes through here (the rotate handle,
    /// the Straighten slider and Angle field, `crop.straighten`, `develop.set` on `crop.angle`), so
    /// they all give the same crop (issue #742).
    pub fn with_angle(&self, angle_deg: f64, w: f64, h: f64) -> CropGeometry {
        let cand = CropGeometry { rect: self.rect, angle: angle_deg };
        if cand.is_within_image(w, h) {
            return cand;
        }
        let r = self.rect_px(w, h);
        // a degenerate rect has no aspect to keep: fall back to the image's own
        let aspect = Some(r.width() / r.height()).filter(|a| a.is_finite() && *a > 0.0);
        crop_fit_angle(w, h, angle_deg, aspect)
    }

    /// This crop normalized to the photo grown by `e` of its size on every side (the same pixels:
    /// `(v + e) / (1 + 2e)`; the centre and the angle stay).
    pub fn grown(&self, e: f64) -> CropGeometry {
        let k = 1.0 + 2.0 * e;
        let f = |v: f64| (v + e) / k;
        let r = self.rect;
        CropGeometry { rect: Rect { x0: f(r.x0), y0: f(r.y0), x1: f(r.x1), y1: f(r.y1) }, angle: self.angle }
    }

    /// Inverse of [`CropGeometry::grown`].
    pub fn shrunk(&self, e: f64) -> CropGeometry {
        let k = 1.0 + 2.0 * e;
        let f = |v: f64| v * k - e;
        let r = self.rect;
        CropGeometry { rect: Rect { x0: f(r.x0), y0: f(r.y0), x1: f(r.x1), y1: f(r.y1) }, angle: self.angle }
    }

    /// [`CropGeometry::is_within_image`], or with `expand` within the photo grown by [`EXPAND_MAX`].
    pub fn is_within(&self, w: f64, h: f64, expand: bool) -> bool {
        if !expand {
            return self.is_within_image(w, h);
        }
        let k = 1.0 + 2.0 * EXPAND_MAX;
        self.grown(EXPAND_MAX).is_within_image(w * k, h * k)
    }

    /// [`CropGeometry::constrained`] to the photo, or with `expand` to the photo grown by
    /// [`EXPAND_MAX`].
    pub fn constrained_in(&self, w: f64, h: f64, expand: bool) -> CropGeometry {
        if !expand {
            return self.constrained(w, h);
        }
        // (a crop that fits stays exactly as it is: the round trip through the grown frame isn't exact)
        if self.is_within(w, h, true) {
            return *self;
        }
        let k = 1.0 + 2.0 * EXPAND_MAX;
        self.grown(EXPAND_MAX).constrained(w * k, h * k).shrunk(EXPAND_MAX)
    }

    /// [`CropGeometry::with_angle`], or with `expand`: the crop turned as it is (the corners it
    /// uncovers are for the fill), only kept within the grown photo.
    pub fn with_angle_in(&self, angle_deg: f64, w: f64, h: f64, expand: bool) -> CropGeometry {
        if !expand {
            return self.with_angle(angle_deg, w, h);
        }
        CropGeometry { rect: self.rect, angle: angle_deg }.constrained_in(w, h, true)
    }

    /// Whether some of the crop lies outside the photo (there is area for Generative Expand to
    /// fill).
    pub fn reaches_past(&self, w: f64, h: f64) -> bool {
        !self.is_within_image(w, h)
    }
}

/// The smallest crop of `aspect` (width / height in pixels; the photo's own if `None`) at
/// `angle_deg` that holds the whole `w × h` photo, centred, kept within the photo grown by
/// [`EXPAND_MAX`]: what choosing an aspect ratio gives with Generative Expand on (a landscape photo
/// asked for 1 × 1 gains a strip above and below).
pub fn crop_cover_angle(w: f64, h: f64, angle_deg: f64, aspect: Option<f64>) -> CropGeometry {
    let a = aspect.filter(|a| a.is_finite() && *a > 0.0).unwrap_or(w / h);
    // the photo's corners in the straightened frame
    let c = Point::new(w / 2.0, h / 2.0);
    let turn = Affine::rotate_about(angle_deg.to_radians(), c);
    let pts = Rect::new(0.0, 0.0, w, h).corners().map(|p| turn.apply(p));
    let (mut hw, mut hh) = (0.0f64, 0.0f64);
    for p in pts {
        hw = hw.max((p.x - c.x).abs());
        hh = hh.max((p.y - c.y).abs());
    }
    // grow the short side to the aspect
    if hw / hh.max(1e-9) < a {
        hw = hh * a;
    } else {
        hh = hw / a;
    }
    let r = Rect::from_center(c, 2.0 * hw, 2.0 * hh);
    CropGeometry { rect: r.scale(1.0 / w, 1.0 / h), angle: angle_deg }.constrained_in(w, h, true)
}

/// Largest scale `s` such that a centred `cw·s × ch·s` rectangle fits inside a `w × h` image rotated
/// by `angle_deg` about its centre.
pub fn max_inscribed_scale(w: f64, h: f64, angle_deg: f64, cw: f64, ch: f64) -> f64 {
    let a = angle_deg.to_radians().abs();
    let (s, c) = a.sin_cos();
    // Half extents of the crop must satisfy, in the image frame (rotated back):
    // |x| c + |y| s <= w/2 and |x| s + |y| c <= h/2 for the crop corner (x, y) = (cw/2, ch/2)·scale.
    let (hx, hy) = (cw / 2.0, ch / 2.0);
    let s1 = (w / 2.0) / (hx * c + hy * s);
    let s2 = (h / 2.0) / (hx * s + hy * c);
    s1.min(s2)
}

/// The biggest crop of the image's own aspect (or `aspect` if given) that fits at `angle_deg`.
pub fn crop_fit_angle(w: f64, h: f64, angle_deg: f64, aspect: Option<f64>) -> CropGeometry {
    let a = aspect.unwrap_or(w / h);
    let base = Rect::new(0.0, 0.0, w, h).fit_aspect(a);
    let s = max_inscribed_scale(w, h, angle_deg, base.width(), base.height()).min(1.0);
    let r = Rect::from_center(Point::new(w / 2.0, h / 2.0), base.width() * s, base.height() * s);
    CropGeometry { rect: r.scale(1.0 / w, 1.0 / h), angle: angle_deg }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grown_and_shrunk_are_inverse_and_keep_the_centre() {
        let c = CropGeometry { rect: Rect::new(-0.2, 0.1, 0.9, 1.3), angle: 7.0 };
        let g = c.grown(EXPAND_MAX);
        let back = g.shrunk(EXPAND_MAX);
        for (a, b) in [(c.rect.x0, back.rect.x0), (c.rect.y0, back.rect.y0), (c.rect.x1, back.rect.x1), (c.rect.y1, back.rect.y1)] {
            assert!((a - b).abs() < 1e-12);
        }
        assert!((CropGeometry::default().grown(0.5).rect.center().x - 0.5).abs() < 1e-12);
        assert_eq!(g.angle, 7.0);
    }

    #[test]
    fn expand_widens_the_bounds() {
        let past = CropGeometry { rect: Rect::new(-0.3, 0.0, 1.0, 1.0), angle: 0.0 };
        assert!(!past.is_within(600.0, 400.0, false) && past.reaches_past(600.0, 400.0));
        assert!(past.is_within(600.0, 400.0, true));
        let too_far = CropGeometry { rect: Rect::new(-1.5, 0.0, 1.0, 1.0), angle: 0.0 };
        assert!(!too_far.is_within(600.0, 400.0, true));
        let k = too_far.constrained_in(600.0, 400.0, true);
        assert!(k.is_within(600.0, 400.0, true) && k.rect.x0 >= -EXPAND_MAX - 1e-6, "{k:?}");
        // without expand the same crop is pulled back inside the photo
        assert!(past.constrained_in(600.0, 400.0, false).is_within_image(600.0, 400.0));
        // turning keeps the crop (its corners are for the fill), unlike the plain rule
        let full = CropGeometry::default();
        let turned = full.with_angle_in(10.0, 600.0, 400.0, true);
        assert_eq!(turned.rect, full.rect);
        assert!(turned.reaches_past(600.0, 400.0));
        assert!(full.with_angle_in(10.0, 600.0, 400.0, false).is_within_image(600.0, 400.0));
    }

    #[test]
    fn a_cover_crop_holds_the_photo_at_its_aspect() {
        // a 3 × 2 photo asked for 1 × 1: a strip above and below
        let c = crop_cover_angle(600.0, 400.0, 0.0, Some(1.0));
        let r = c.rect_px(600.0, 400.0);
        assert!((r.width() - 600.0).abs() < 1e-6 && (r.height() - 600.0).abs() < 1e-6, "{r:?}");
        assert!((r.y0 + 100.0).abs() < 1e-6);
        // 9 × 16 from 3 × 2 stays within the grown photo
        let tall = crop_cover_angle(600.0, 400.0, 0.0, Some(9.0 / 16.0));
        assert!(tall.is_within(600.0, 400.0, true));
        let tr = tall.rect_px(600.0, 400.0);
        assert!((tr.width() / tr.height() - 9.0 / 16.0).abs() < 1e-6);
        // turned, it still covers every corner of the photo
        let t = crop_cover_angle(600.0, 400.0, 12.0, None);
        let back = Affine::rotate_about(12.0f64.to_radians(), Point::new(300.0, 200.0));
        let r = t.rect_px(600.0, 400.0);
        for p in Rect::new(0.0, 0.0, 600.0, 400.0).corners() {
            let q = back.apply(p);
            assert!(q.x >= r.x0 - 1e-6 && q.x <= r.x1 + 1e-6 && q.y >= r.y0 - 1e-6 && q.y <= r.y1 + 1e-6, "{q:?} outside {r:?}");
        }
        // hostile aspect: the photo's own
        let own = crop_cover_angle(600.0, 400.0, 0.0, Some(f64::NAN));
        assert!((own.rect.width() - 1.0).abs() < 1e-9 && (own.rect.height() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn identity_maps_pixels() {
        let c = CropGeometry::default();
        let t = c.output_to_source(400.0, 300.0, 400.0, 300.0);
        let p = t.apply(Point::new(10.0, 20.0));
        assert!((p.x - 10.0).abs() < 1e-9 && (p.y - 20.0).abs() < 1e-9);
    }

    #[test]
    fn rotated_fit_is_inside() {
        for ang in [-45.0, -10.0, 0.0, 3.5, 30.0, 45.0] {
            let c = crop_fit_angle(600.0, 400.0, ang, None);
            assert!(c.is_within_image(600.0, 400.0), "{ang}");
            // and maximal: 1% bigger does not fit (except angle 0 where it equals the image)
            let big = CropGeometry { rect: Rect::from_center(c.rect.center(), c.rect.width() * 1.01, c.rect.height() * 1.01), angle: ang };
            assert!(!big.is_within_image(600.0, 400.0), "{ang}");
        }
    }

    #[test]
    fn constrain_pulls_crop_inside() {
        let c = CropGeometry { rect: Rect::new(0.5, 0.5, 1.4, 1.2), angle: 12.0 };
        let k = c.constrained(300.0, 200.0);
        assert!(k.is_within_image(300.0, 200.0));
    }

    #[test]
    fn with_angle_keeps_a_crop_that_still_fits() {
        // a small centred crop fits at a few degrees: the angle changes, the rect does not
        let c = CropGeometry { rect: Rect::new(0.3, 0.3, 0.7, 0.7), angle: 0.0 };
        for ang in [-3.0, 0.5, 2.0, 5.0] {
            let t = c.with_angle(ang, 600.0, 400.0);
            assert_eq!(t.rect, c.rect, "{ang}°");
            assert_eq!(t.angle, ang);
        }
    }

    #[test]
    fn with_angle_shrinks_a_crop_that_no_longer_fits_keeping_its_aspect() {
        for (rect, ang) in [(Rect::UNIT, 15.0), (Rect::new(0.0, 0.1, 0.8, 0.9), -20.0), (Rect::new(0.2, 0.0, 0.8, 1.0), 45.0)] {
            let c = CropGeometry { rect, angle: 0.0 };
            let t = c.with_angle(ang, 600.0, 400.0);
            assert!(t.is_within_image(600.0, 400.0), "{rect:?} at {ang}°");
            let (a, b) = (c.rect_px(600.0, 400.0), t.rect_px(600.0, 400.0));
            assert!((a.aspect() - b.aspect()).abs() < 1e-9, "aspect kept: {} vs {}", a.aspect(), b.aspect());
            assert_eq!(t.angle, ang);
        }
    }

    #[test]
    fn with_angle_survives_a_degenerate_crop() {
        // zero height: no aspect to keep (width / height = ∞)
        let c = CropGeometry { rect: Rect::new(0.0, 0.5, 1.0, 0.5), angle: 0.0 };
        let t = c.with_angle(30.0, 600.0, 400.0);
        assert!(t.is_within_image(600.0, 400.0));
        assert!([t.rect.x0, t.rect.y0, t.rect.x1, t.rect.y1].iter().all(|v| v.is_finite()));
    }

    #[test]
    fn half_crop_maps_to_centre_quadrant() {
        let c = CropGeometry { rect: Rect::new(0.25, 0.25, 0.75, 0.75), angle: 0.0 };
        let t = c.output_to_source(100.0, 100.0, 50.0, 50.0);
        let p = t.apply(Point::new(0.0, 0.0));
        assert!((p.x - 25.0).abs() < 1e-9 && (p.y - 25.0).abs() < 1e-9);
    }
}
