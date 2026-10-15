//! Model results stored with the settings, so rendering never needs the model: segmentations
//! with a mask ([`SegMask`]), AI Remove fills with a spot ([`SpotFill`]) and Generative Expand's
//! fill past the photo's edges ([`ExpandFill`]).

use serde::{Deserialize, Serialize};

/// Logits are stored in steps of 1/`SCALE`, clamped to ±127/`SCALE` (≈ ±15.9: a sigmoid of
/// 1 − 1.2e-7, i.e. fully in or out).
const SCALE: f32 = 8.0;
/// Largest grid a stored segmentation may claim (hostile input caps the allocation).
pub const MAX_SIDE: usize = 1024;

/// A segmentation computed by a model (SAM 3 in `lightcraft-segment`): `side × side` logits
/// over the uncropped, oriented image stretched to a square (row `y`, column `x` covers
/// normalized `((x + 0.5) / side, (y + 0.5) / side)`), quantized to `i8`, deflated and base64
/// encoded. Coverage at a point is the sigmoid of the bilinearly sampled logit, so edges stay
/// smooth at any output size.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SegMask {
    pub side: u32,
    pub data: String,
    /// The part of the image the grid covers, normalized `[x0, y0, x1, y1]` (`None` = all of
    /// it). A zoomed-in pass over one object covers just its surroundings, at a higher
    /// resolution than a pass over the whole photo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rect: Option<[f64; 4]>,
}

impl SegMask {
    /// From `side × side` logits over the whole image.
    pub fn from_logits(side: usize, logits: &[f32]) -> SegMask {
        let q: Vec<u8> = logits.iter().take(side * side).map(|l| ((l * SCALE).round().clamp(-127.0, 127.0) as i8) as u8).collect();
        let z = miniz_oxide::deflate::compress_to_vec(&q, 8);
        SegMask { side: side as u32, data: base64_encode(&z), rect: None }
    }

    /// From `side × side` logits over the part `rect` of the image.
    pub fn from_logits_in(side: usize, logits: &[f32], rect: [f64; 4]) -> SegMask {
        SegMask { rect: Some(rect), ..SegMask::from_logits(side, logits) }
    }

    /// From a picture of coverage (0 = out … 255 = in), `w × h` bytes over the whole image: what a
    /// system segmenter (Apple Vision on iPhone) answers. It is averaged down to `side × side`
    /// (stretched to a square like every grid here) and each value becomes the logit that samples
    /// back to that coverage. `None` when the data doesn't match `w × h`, or a size is absurd.
    pub fn from_coverage(side: usize, w: usize, h: usize, cov: &[u8]) -> Option<SegMask> {
        /// Largest picture accepted (hostile input caps the work).
        const MAX_PICTURE: usize = 8192;
        if side == 0 || side > MAX_SIDE || w == 0 || h == 0 || w > MAX_PICTURE || h > MAX_PICTURE || cov.len() != w * h {
            return None;
        }
        // the source pixels [a, b) that grid cell `t` of `side` covers on an axis of `n`
        let span = |t: usize, n: usize| {
            let a = t * n / side;
            (a, ((t + 1) * n).div_ceil(side).max(a + 1).min(n))
        };
        let logits: Vec<f32> = (0..side * side)
            .map(|i| {
                let ((x0, x1), (y0, y1)) = (span(i % side, w), span(i / side, h));
                let mut sum = 0u64;
                for y in y0..y1 {
                    sum += (x0..x1).map(|x| u64::from(cov.get(y * w + x).copied().unwrap_or(0))).sum::<u64>();
                }
                let n = ((x1 - x0) * (y1 - y0)) as f32;
                // (kept off 0 and 1 so the logit stays finite; ±8.5 is fully in or out)
                let c = (sum as f32 / (n * 255.0)).clamp(2e-4, 1.0 - 2e-4);
                (c / (1.0 - c)).ln()
            })
            .collect();
        Some(SegMask::from_logits(side, &logits))
    }

    /// [`SegMask::from_coverage`] of just what is selected: the part of the picture holding coverage above
    /// half, and a margin of 4 % of its size, becomes the grid (its `rect`), so a small object gets a fine
    /// one. Also the selection's own box `[x0, y0, x1, y1]` and its centre, normalized over the picture.
    /// `None` when nothing is selected, or the picture is bad.
    pub fn from_coverage_cropped(side: usize, w: usize, h: usize, cov: &[u8]) -> Option<CroppedSelection> {
        if w == 0 || h == 0 || cov.len() != w.checked_mul(h)? {
            return None;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0usize, 0usize);
        let (mut sx, mut sy, mut n) = (0.0f64, 0.0f64, 0.0f64);
        for (i, c) in cov.iter().enumerate() {
            if *c > 127 {
                let (x, y) = (i % w, i / w);
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
                (sx, sy, n) = (sx + x as f64 + 0.5, sy + y as f64 + 0.5, n + 1.0);
            }
        }
        if n == 0.0 {
            return None;
        }
        let m = (((x1 - x0).max(y1 - y0) as f64) * 0.04).ceil().max(2.0) as usize;
        let (cx0, cy0, cx1, cy1) = (x0.saturating_sub(m), y0.saturating_sub(m), (x1 + m).min(w), (y1 + m).min(h));
        let (cw, ch) = (cx1 - cx0, cy1 - cy0);
        let mut sub = Vec::with_capacity(cw * ch);
        for y in cy0..cy1 {
            sub.extend_from_slice(cov.get(y * w + cx0..y * w + cx1)?);
        }
        let mut mask = SegMask::from_coverage(side, cw, ch, &sub)?;
        let (fw, fh) = (w as f64, h as f64);
        mask.rect = Some([cx0 as f64 / fw, cy0 as f64 / fh, cx1 as f64 / fw, cy1 as f64 / fh]);
        Some(CroppedSelection { mask, bbox: [x0 as f64 / fw, y0 as f64 / fh, x1 as f64 / fw, y1 as f64 / fh], center: (sx / n / fw, sy / n / fh) })
    }

    /// The covered part, normalized (the whole image without a `rect`); `None` when damaged.
    pub fn bounds(&self) -> Option<[f64; 4]> {
        let r = self.rect.unwrap_or([0.0, 0.0, 1.0, 1.0]);
        (r.iter().all(|v| v.is_finite()) && r[2] > r[0] && r[3] > r[1]).then_some(r)
    }

    /// The logits, or `None` when the data is damaged or claims an absurd size.
    pub fn logits(&self) -> Option<Vec<f32>> {
        let side = self.side as usize;
        if side == 0 || side > MAX_SIDE {
            return None;
        }
        let z = base64_decode(&self.data)?;
        let q = miniz_oxide::inflate::decompress_to_vec_with_limit(&z, side * side).ok()?;
        (q.len() == side * side).then(|| q.iter().map(|b| f32::from(*b as i8) / SCALE).collect())
    }
}

/// A selection cropped to its own box ([`SegMask::from_coverage_cropped`]).
#[derive(Clone, Debug, PartialEq)]
pub struct CroppedSelection {
    pub mask: SegMask,
    /// The selection's box, normalized `[x0, y0, x1, y1]`.
    pub bbox: [f64; 4],
    /// Its centre of mass, normalized.
    pub center: (f64, f64),
}

/// Most zoomed-in detail patches one AI mask component keeps (the app makes one; each costs a
/// decode per render, so a hostile document can't make rendering explode).
pub const MAX_DETAIL: usize = 4;

/// Deserialize a component's detail patches, keeping at most [`MAX_DETAIL`] (the rest are
/// skipped without being decoded).
pub(crate) fn de_detail<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<SegMask>, D::Error> {
    struct Capped;
    impl<'de> serde::de::Visitor<'de> for Capped {
        type Value = Vec<SegMask>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a list of segmentations")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<SegMask>, A::Error> {
            let mut out = Vec::new();
            while out.len() < MAX_DETAIL {
                match seq.next_element::<SegMask>()? {
                    Some(m) => out.push(m),
                    None => return Ok(out),
                }
            }
            while seq.next_element::<serde::de::IgnoredAny>()?.is_some() {}
            Ok(out)
        }
    }
    d.deserialize_seq(Capped)
}

/// Largest side of a stored [`SpotFill`] (hostile input caps the allocation; the app stores at
/// most the model's 512).
pub const MAX_FILL_SIDE: usize = 1024;
/// Encoding gamma of [`SpotFill`] pixels.
const FILL_GAMMA: f32 = 2.2;

/// Pixels a model painted for a Remove spot (AI Remove, `lightcraft-erase`), stored with the spot
/// so rendering never needs the model. `w × h` RGB over the part `rect` of the image (normalized
/// `[x0, y0, x1, y1]`, the coordinates of the spot's points): the scene-linear light of the
/// retouching stage, encoded `255 · (v / scale)^(1/2.2)`, row-major, each byte stored as its
/// difference from the one to its left (smooth pictures deflate better), deflated and base64
/// encoded. Around the stroke the pixels are the photo as it was when the fill was made, which
/// lets a render match the fill to the photo's current tones.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpotFill {
    pub w: u32,
    pub h: u32,
    pub rect: [f64; 4],
    /// Linear value of code 255.
    pub scale: f32,
    pub data: String,
}

impl SpotFill {
    /// From `w × h` linear pixels over `rect`; `None` for an empty or oversized picture.
    pub fn from_linear(w: usize, h: usize, rect: [f64; 4], px: &[[f32; 3]]) -> Option<SpotFill> {
        if w == 0 || h == 0 || w > MAX_FILL_SIDE || h > MAX_FILL_SIDE || px.len() != w * h {
            return None;
        }
        let scale = px.iter().flatten().copied().filter(|v| v.is_finite()).fold(0.0f32, f32::max).max(1e-6);
        let mut q = Vec::with_capacity(px.len() * 3);
        for row in px.chunks(w) {
            let mut prev = [0u8; 3];
            for p in row {
                for (c, v) in p.iter().enumerate() {
                    let code = (255.0 * (v / scale).clamp(0.0, 1.0).powf(1.0 / FILL_GAMMA)).round() as u8;
                    q.push(code.wrapping_sub(prev[c]));
                    prev[c] = code;
                }
            }
        }
        let data = base64_encode(&miniz_oxide::deflate::compress_to_vec(&q, 8));
        Some(SpotFill { w: w as u32, h: h as u32, rect, scale, data })
    }

    /// The covered part, normalized; `None` when damaged.
    pub fn bounds(&self) -> Option<[f64; 4]> {
        let r = self.rect;
        (r.iter().all(|v| v.is_finite()) && r[2] > r[0] && r[3] > r[1]).then_some(r)
    }

    /// The linear pixels, or `None` when the data is damaged or claims an absurd size.
    pub fn pixels(&self) -> Option<Vec<[f32; 3]>> {
        let (w, h) = (self.w as usize, self.h as usize);
        if w == 0 || h == 0 || w > MAX_FILL_SIDE || h > MAX_FILL_SIDE || !self.scale.is_finite() || self.scale <= 0.0 {
            return None;
        }
        let n = w * h * 3;
        let q = miniz_oxide::inflate::decompress_to_vec_with_limit(&base64_decode(&self.data)?, n).ok()?;
        if q.len() != n {
            return None;
        }
        let lut: Vec<f32> = (0..=255u8).map(|c| self.scale * (f32::from(c) / 255.0).powf(FILL_GAMMA)).collect();
        let mut out = Vec::with_capacity(w * h);
        for row in q.chunks(w * 3) {
            let mut prev = [0u8; 3];
            for d in row.chunks(3) {
                let mut p = [0.0f32; 3];
                for c in 0..3 {
                    prev[c] = prev[c].wrapping_add(d.get(c).copied().unwrap_or(0));
                    p[c] = lut.get(usize::from(prev[c])).copied().unwrap_or(0.0);
                }
                out.push(p);
            }
        }
        Some(out)
    }
}

/// What Generative Expand painted past the photo's edges (the crop's Expand), stored with the
/// settings so rendering never needs the model. `fill` covers the canvas the model saw:
/// `fill.rect` is in the normalized coordinates of the transformed image (the coordinates of
/// masks and spots), reaching past 0..1 on the sides it extends. Its pixels are the light of the
/// resampled photo before white balance (the stage where a render puts them, so every edit after
/// it applies to the new area as to the photo), and black over the photo's inside, which a
/// render never reads from the fill (only a few pixels along its edges are kept, for the
/// blend). See `lightcraft_pipeline::expand`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExpandFill {
    pub fill: SpotFill,
    /// The model that painted it: `migan`, `lama` or `classic` (texture synthesized from the
    /// photo, no model).
    pub model: String,
}

/// Largest side of a stored [`DepthMap`] (hostile input caps the allocation; the app stores at
/// most the depth model's ~1000 pixels).
pub const MAX_DEPTH_SIDE: usize = 2048;

/// How far each part of the photo is, as estimated by a model (Lens Blur's depth,
/// `lightcraft-depth`) for a photo without depth data of its own, stored with the settings so
/// rendering never needs the model. `w × h` *nearness* values over the uncropped, EXIF-oriented
/// photo (row `y`, column `x` covers normalized `((x + 0.5) / w, (y + 0.5) / h)`): 1 is the
/// nearest part of the photo, 0 the farthest, linear in inverse depth (which is what lens blur
/// grows with). 8-bit, each byte stored as its difference from the one to its left, deflated and
/// base64 encoded.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DepthMap {
    pub w: u32,
    pub h: u32,
    pub data: String,
}

impl DepthMap {
    /// From `w × h` nearness values (clamped to 0..1); `None` for an empty or oversized map.
    pub fn from_nearness(w: usize, h: usize, v: &[f32]) -> Option<DepthMap> {
        if w == 0 || h == 0 || w > MAX_DEPTH_SIDE || h > MAX_DEPTH_SIDE || v.len() != w * h {
            return None;
        }
        let mut q = Vec::with_capacity(v.len());
        for row in v.chunks(w) {
            let mut prev = 0u8;
            for n in row {
                let code = if n.is_finite() { (n.clamp(0.0, 1.0) * 255.0).round() as u8 } else { 0 };
                q.push(code.wrapping_sub(prev));
                prev = code;
            }
        }
        Some(DepthMap { w: w as u32, h: h as u32, data: base64_encode(&miniz_oxide::deflate::compress_to_vec(&q, 8)) })
    }

    /// The nearness values (0..1, row-major), or `None` when the data is damaged or claims an
    /// absurd size.
    pub fn nearness(&self) -> Option<Vec<f32>> {
        let (w, h) = (self.w as usize, self.h as usize);
        if w == 0 || h == 0 || w > MAX_DEPTH_SIDE || h > MAX_DEPTH_SIDE {
            return None;
        }
        let q = miniz_oxide::inflate::decompress_to_vec_with_limit(&base64_decode(&self.data)?, w * h).ok()?;
        if q.len() != w * h {
            return None;
        }
        let mut out = Vec::with_capacity(w * h);
        for row in q.chunks(w) {
            let mut prev = 0u8;
            for d in row {
                prev = prev.wrapping_add(*d);
                out.push(f32::from(prev) / 255.0);
            }
        }
        Some(out)
    }
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk.first().copied().unwrap_or(0), chunk.get(1).copied().unwrap_or(0), chunk.get(2).copied().unwrap_or(0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(B64[((n >> (18 - 6 * i)) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes().filter(|c| !c.is_ascii_whitespace()) {
        if c == b'=' {
            break;
        }
        let v = B64.iter().position(|b| *b == c)? as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips() {
        for n in 0..10 {
            let v: Vec<u8> = (0..n).map(|i| (i * 37 + 11) as u8).collect();
            assert_eq!(base64_decode(&base64_encode(&v)).unwrap(), v);
        }
        assert_eq!(base64_encode(b"Man"), "TWFu");
        assert_eq!(base64_encode(b"Ma"), "TWE=");
        assert!(base64_decode("TW!u").is_none());
    }

    #[test]
    fn logits_round_trip_quantized() {
        let side = 16;
        let l: Vec<f32> = (0..side * side).map(|i| (i as f32 - 128.0) / 4.0).collect();
        let m = SegMask::from_logits(side, &l);
        let back = m.logits().unwrap();
        for (a, b) in l.iter().zip(&back) {
            assert!((a.clamp(-127.0 / SCALE, 127.0 / SCALE) - b).abs() <= 0.5 / SCALE + 1e-6);
        }
    }

    #[test]
    fn a_hostile_document_keeps_at_most_max_detail_patches() {
        let patch = serde_json::to_value(SegMask::from_logits(4, &[1.0; 16])).unwrap();
        let shape = serde_json::json!({"kind": "object", "hint": [{"x": 0.5, "y": 0.5}], "detail": vec![patch; 10_000]});
        let s: crate::MaskShape = serde_json::from_value(shape).unwrap();
        let crate::MaskShape::Object { detail, .. } = s else { panic!("an Object") };
        assert_eq!(detail.len(), MAX_DETAIL);
        // and a whole develop document with them still loads
        let mut d = crate::DevelopSettings::default();
        d.masks.push(crate::Mask {
            id: 1,
            components: vec![crate::MaskComponent {
                name: None,
                op: crate::MaskOp::Add,
                invert: false,
                shape: crate::MaskShape::Prompt { text: "x".into(), seg: None, detail: vec![SegMask::from_logits(4, &[1.0; 16]); 50], edge: 0.0 },
            }],
            ..Default::default()
        });
        let back: crate::DevelopSettings = serde_json::from_str(&serde_json::to_string(&d).unwrap()).unwrap();
        let crate::MaskShape::Prompt { detail, .. } = &back.masks[0].components[0].shape else { panic!("a Prompt") };
        assert_eq!(detail.len(), MAX_DETAIL);
    }

    #[test]
    fn a_fill_round_trips_within_its_quantization() {
        let (w, h) = (23, 9);
        let px: Vec<[f32; 3]> = (0..w * h).map(|i| [(i % w) as f32 * 0.05, 0.3, (i / w) as f32 * 0.2 + 0.01]).collect();
        let f = SpotFill::from_linear(w, h, [0.1, 0.2, 0.3, 0.4], &px).unwrap();
        let back = f.pixels().unwrap();
        assert_eq!(back.len(), px.len());
        for (a, b) in px.iter().zip(&back) {
            for c in 0..3 {
                // one 8-bit step of a gamma-2.2 code, relative to the value (plus a floor near black)
                assert!((a[c] - b[c]).abs() <= a[c] * 0.02 + f.scale * 2e-4, "{a:?} vs {b:?}");
            }
        }
        assert_eq!(f.bounds(), Some([0.1, 0.2, 0.3, 0.4]));
    }

    #[test]
    fn a_damaged_fill_is_none() {
        let f = SpotFill::from_linear(4, 4, [0.0, 0.0, 1.0, 1.0], &[[0.5; 3]; 16]).unwrap();
        assert!(SpotFill { w: 5, ..f.clone() }.pixels().is_none(), "size mismatch");
        assert!(SpotFill { w: 100_000, ..f.clone() }.pixels().is_none(), "absurd size");
        assert!(SpotFill { scale: f32::NAN, ..f.clone() }.pixels().is_none());
        assert!(SpotFill { data: "!!".into(), ..f.clone() }.pixels().is_none());
        assert!(SpotFill { rect: [0.5, 0.0, 0.2, 1.0], ..f.clone() }.bounds().is_none());
        assert!(SpotFill::from_linear(0, 4, [0.0; 4], &[]).is_none());
        assert!(SpotFill::from_linear(2, 2, [0.0; 4], &[[0.0; 3]; 3]).is_none());
    }

    #[test]
    fn a_depth_map_round_trips_and_refuses_damage() {
        let (w, h) = (31, 7);
        let v: Vec<f32> = (0..w * h).map(|i| ((i % w) as f32 / w as f32 + (i / w) as f32 * 0.03).min(1.0)).collect();
        let d = DepthMap::from_nearness(w, h, &v).unwrap();
        let back = d.nearness().unwrap();
        assert!(v.iter().zip(&back).all(|(a, b)| (a - b).abs() <= 0.5 / 255.0 + 1e-6));
        // out-of-range and NaN values are kept inside 0..1
        let odd = DepthMap::from_nearness(3, 1, &[-1.0, f32::NAN, 7.0]).unwrap().nearness().unwrap();
        assert_eq!(odd, vec![0.0, 0.0, 1.0]);
        assert!(DepthMap { w: 30, ..d.clone() }.nearness().is_none(), "size mismatch");
        assert!(DepthMap { w: 1 << 20, ..d.clone() }.nearness().is_none(), "absurd size");
        assert!(DepthMap { data: "!!".into(), ..d.clone() }.nearness().is_none());
        assert!(DepthMap::from_nearness(0, 3, &[]).is_none());
        assert!(DepthMap::from_nearness(2, 2, &[0.0; 3]).is_none());
    }

    #[test]
    fn damaged_data_is_none() {
        let mut m = SegMask::from_logits(8, &[1.0; 64]);
        m.side = 9;
        assert!(m.logits().is_none(), "size mismatch");
        m.side = 100_000;
        assert!(m.logits().is_none(), "absurd size");
        assert!(SegMask { side: 8, data: "not base64 !".into(), rect: None }.logits().is_none());
        assert!(SegMask { rect: Some([0.5, 0.0, 0.2, 1.0]), ..SegMask::from_logits(2, &[0.0; 4]) }.bounds().is_none());
    }

    #[test]
    fn a_coverage_picture_becomes_a_mask_with_the_same_edge() {
        // 100 × 50 with the left 40 columns in; 50 cells across, so cell 19 (columns 38–39) is
        // the last one in and cell 20 (40–41) the first one out
        let (w, h) = (100, 50);
        let cov: Vec<u8> = (0..w * h).map(|i| if i % w < 40 { 255 } else { 0 }).collect();
        let l = SegMask::from_coverage(50, w, h, &cov).unwrap().logits().unwrap();
        let at = |x: usize, y: usize| l[y * 50 + x];
        assert!(at(0, 0) > 5.0 && at(19, 25) > 5.0, "{} {}", at(0, 0), at(19, 25));
        assert!(at(20, 25) < -5.0 && at(49, 49) < -5.0, "{} {}", at(20, 25), at(49, 49));
    }

    #[test]
    fn a_half_covered_cell_is_undecided() {
        let m = SegMask::from_coverage(1, 2, 1, &[255, 0]).unwrap();
        assert!(m.logits().unwrap()[0].abs() < 0.2);
    }

    #[test]
    fn a_selection_is_cropped_to_its_own_box() {
        // 200 × 100 with a 20 × 10 block in at columns 120–139, rows 60–69
        let (w, h) = (200, 100);
        let cov: Vec<u8> = (0..w * h).map(|i| if (120..140).contains(&(i % w)) && (60..70).contains(&(i / w)) { 255 } else { 0 }).collect();
        let c = SegMask::from_coverage_cropped(64, w, h, &cov).unwrap();
        assert_eq!(c.bbox, [0.6, 0.6, 0.7, 0.7]);
        assert!((c.center.0 - 0.65).abs() < 1e-9 && (c.center.1 - 0.65).abs() < 1e-9, "{:?}", c.center);
        // the grid covers the box and a 4 % margin (0.8 columns of 20: rounded up to 2 px)
        let r = c.mask.rect.unwrap();
        assert_eq!(r, [118.0 / 200.0, 58.0 / 100.0, 142.0 / 200.0, 72.0 / 100.0]);
        let l = c.mask.logits().unwrap();
        assert!(l[32 * 64 + 32] > 5.0, "the middle is in");
        assert!(l[0] < -5.0 && l[63 * 64 + 63] < -5.0, "the margin is out");
        // nothing selected, a bad picture
        assert!(SegMask::from_coverage_cropped(64, w, h, &vec![0; w * h]).is_none());
        assert!(SegMask::from_coverage_cropped(64, w, h, &[255; 5]).is_none());
        assert!(SegMask::from_coverage_cropped(64, 0, 0, &[]).is_none());
    }

    #[test]
    fn a_bad_coverage_picture_is_none() {
        assert!(SegMask::from_coverage(8, 4, 4, &[0; 15]).is_none(), "size mismatch");
        assert!(SegMask::from_coverage(0, 4, 4, &[0; 16]).is_none());
        assert!(SegMask::from_coverage(MAX_SIDE + 1, 4, 4, &[0; 16]).is_none());
        assert!(SegMask::from_coverage(8, 0, 4, &[]).is_none());
        assert!(SegMask::from_coverage(8, 100_000, 1, &[]).is_none(), "absurd size");
    }
}
