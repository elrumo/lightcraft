//! Where the camera focused, in the photo as it is shown.
//!
//! A photo's focus point ([`lightcraft_meta::FocusPoint`]) is stored on the upright (EXIF-oriented) photo. What a frontend
//! draws is the photo after the user's Rotate and flips and their crop, so the box is mapped through both here, once, for
//! every frontend: egui's loupe, Compare and Survey, and a native one asking `view.focus`.

use lightcraft_catalog::Photo;
use lightcraft_develop::EmbeddedLens;
use lightcraft_geom::{Point, Rect};
use lightcraft_pipeline::geometry::Frame;

/// The photo as shown, in pixels: its size after the crop and the user's rotation, at least 1 × 1.
fn output_px(frame: &Frame) -> [usize; 2] {
    let (w, h) = frame.native_size();
    let px = |v: f64| if v.is_finite() { v.round().clamp(1.0, 1e9) as usize } else { 1 };
    [px(w), px(h)]
}

/// The camera's focus point of `photo` in the displayed frame (its crop, rotation and flips), as the normalized box over the
/// drawn image (y down): `None` when the photo has none or its centre is cropped away. The box may reach past an edge (a
/// frontend clips it when it draws). `lens` is the source's embedded lens correction, which shapes the frame.
pub fn focus_box(photo: &Photo, lens: Option<&EmbeddedLens>) -> Option<Rect> {
    let f = photo.meta.focus.as_ref()?;
    // (`f64::min` and `max` skip a NaN, so a garbage box must be refused before it is mapped)
    if ![f.rect.x0, f.rect.y0, f.rect.x1, f.rect.y1].iter().all(|v| v.is_finite()) {
        return None;
    }
    let d = &photo.develop;
    let frame = Frame::with_lens(photo.width.max(1) as usize, photo.height.max(1) as usize, d, true, lens);
    let [w, h] = output_px(&frame);
    let to_out = frame.out_to_norm(w, h).inverse()?;
    // stored on the upright photo: the user's Rotate / Flip on top, then the crop
    let r = d.orientation.map_norm_rect(f.rect);
    let corners = [Point::new(r.x0, r.y0), Point::new(r.x1, r.y0), Point::new(r.x0, r.y1), Point::new(r.x1, r.y1)].map(|c| {
        let q = to_out.apply(c);
        (q.x / w as f64, q.y / h as f64)
    });
    let xs = corners.map(|c| c.0);
    let ys = corners.map(|c| c.1);
    let out = Rect {
        x0: xs.iter().copied().fold(f64::INFINITY, f64::min),
        y0: ys.iter().copied().fold(f64::INFINITY, f64::min),
        x1: xs.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        y1: ys.iter().copied().fold(f64::NEG_INFINITY, f64::max),
    };
    if ![out.x0, out.y0, out.x1, out.y1].iter().all(|v| v.is_finite()) {
        return None;
    }
    // its centre must be in the frame; the box may reach past the edge (it is clipped when drawn)
    let (cx, cy) = ((out.x0 + out.x1) / 2.0, (out.y0 + out.y1) / 2.0);
    ((0.0..=1.0).contains(&cx) && (0.0..=1.0).contains(&cy)).then_some(out)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use lightcraft_catalog::{PhotoId, Source};
    use lightcraft_meta::FocusPoint;

    use super::*;

    fn photo(w: u32, h: u32, focus: Option<Rect>) -> Photo {
        let mut p = Photo::new(PhotoId(1), Source::Demo { scene: 1 }, "a.jpg", "JPEG", w, h, "2026-10-13T00:00:00");
        p.meta.focus = focus.map(|rect| FocusPoint { rect, mode: Some("AF-C".into()), area: None, tracking: None });
        p
    }

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Rect {
        Rect { x0, y0, x1, y1 }
    }

    #[test]
    fn an_uncropped_unturned_photo_shows_the_box_where_it_is_stored() {
        let b = focus_box(&photo(6000, 4000, Some(rect(0.70, 0.45, 0.80, 0.55))), None).expect("box");
        assert!((b.x0 - 0.70).abs() < 1e-9 && (b.y0 - 0.45).abs() < 1e-9 && (b.x1 - 0.80).abs() < 1e-9 && (b.y1 - 0.55).abs() < 1e-9, "{b:?}");
    }

    #[test]
    fn no_focus_point_gives_no_box() {
        assert!(focus_box(&photo(6000, 4000, None), None).is_none());
    }

    #[test]
    fn the_box_follows_the_crop_and_a_point_cropped_away_gives_none() {
        let mut p = photo(6000, 4000, Some(rect(0.70, 0.45, 0.80, 0.55)));
        // keep the right half: x 0.5..1 becomes 0..1
        let mut d = (*p.develop).clone();
        d.crop.geometry.rect = Rect { x0: 0.5, y0: 0.0, x1: 1.0, y1: 1.0 };
        p.develop = Arc::new(d.clone());
        let b = focus_box(&p, None).expect("box");
        assert!((b.x0 - 0.40).abs() < 1e-6 && (b.x1 - 0.60).abs() < 1e-6, "{b:?}");
        // keep the left half: the point is gone
        d.crop.geometry.rect = Rect { x0: 0.0, y0: 0.0, x1: 0.5, y1: 1.0 };
        p.develop = Arc::new(d);
        assert!(focus_box(&p, None).is_none());
    }

    /// The demo library's first photo, given `focus` as its focus point and chosen.
    fn session_with_focus(focus: Rect) -> (crate::Session, PhotoId) {
        let mut s = crate::Session::with_demo();
        let id = *s.visible_cloned().first().expect("a demo photo");
        let mut meta = s.catalog.photo(id).expect("photo").meta.clone();
        meta.focus = Some(FocusPoint { rect: focus, mode: Some("AF-C".into()), area: Some("Wide".into()), tracking: None });
        s.catalog.apply(lightcraft_catalog::Op::SetMeta { id, meta: Box::new(meta) }).expect("set meta");
        s.execute("library.select", &serde_json::json!({"ids": [id.0]})).expect("select");
        (s, id)
    }

    #[test]
    fn view_focus_answers_in_the_shown_frame_and_follows_a_rotation() {
        let (mut s, id) = session_with_focus(rect(0.20, 0.30, 0.26, 0.36));
        let near = |v: &serde_json::Value, key: &str, want: [f64; 2]| {
            let got = [v[key][0].as_f64().expect("number"), v[key][1].as_f64().expect("number")];
            assert!((got[0] - want[0]).abs() < 1e-6 && (got[1] - want[1]).abs() < 1e-6, "{key}: {got:?}, want {want:?}");
        };
        let v = s.execute("view.focus", &serde_json::json!({"id": id.0})).expect("view.focus");
        near(&v, "center", [0.23, 0.33]);
        assert_eq!(v["rect"].as_array().map(Vec::len), Some(4), "{v}");
        assert_eq!((v["mode"].as_str(), v["area"].as_str(), v.get("tracking")), (Some("AF-C"), Some("Wide"), None), "{v}");
        // without `id`: the active photo
        assert_eq!(s.execute("view.focus", &serde_json::json!({})).expect("active"), v);
        // Rotate Right turns the photo 90° clockwise: the upright point (x, y) is shown at (1 − y, x)
        s.execute("photo.rotateRight", &serde_json::json!({})).expect("rotate");
        let v = s.execute("view.focus", &serde_json::json!({"id": id.0})).expect("rotated");
        near(&v, "center", [0.67, 0.23]);
    }

    #[test]
    fn view_focus_is_null_without_a_point_and_an_error_for_a_missing_photo() {
        let mut s = crate::Session::with_demo();
        let id = *s.visible_cloned().first().expect("a demo photo");
        assert_eq!(s.execute("view.focus", &serde_json::json!({"id": id.0})).expect("no point"), serde_json::Value::Null);
        assert!(s.execute("view.focus", &serde_json::json!({"id": 987_654_321u64})).is_err());
        assert!(s.execute("view.focus", &serde_json::json!({"id": "x"})).is_err());
    }

    #[test]
    fn a_zero_size_photo_or_a_garbage_box_never_panics() {
        let _ = focus_box(&photo(0, 0, Some(rect(0.1, 0.1, 0.2, 0.2))), None);
        for bad in [rect(f64::NAN, 0.0, 1.0, 1.0), rect(0.0, f64::INFINITY, 1.0, 1.0), rect(5.0, 5.0, 6.0, 6.0)] {
            assert!(focus_box(&photo(6000, 4000, Some(bad)), None).is_none(), "{bad:?}");
        }
    }
}
