//! YuNet (finds faces) and SFace (tells them apart) on `rten`, a pure-Rust ONNX runtime. Both are
//! OpenCV Zoo models (YuNet: MIT, Shiqi Yu's group; SFace: Apache-2.0, BUPT); this is our own code
//! reading their published input and output conventions.
//!
//! The detector takes a fixed 640×640 BGR picture (values 0–255): the photo is shrunk to fit and
//! padded at the bottom and right. For each of three strides it answers, per cell, a class score,
//! an "objectness" score, a box (offsets from the cell, log size) and five landmarks (the eyes,
//! the nose and the corners of the mouth). Overlapping boxes are thinned by non-maximum
//! suppression. Each face is then turned (by the similarity transform that puts its landmarks on
//! SFace's template) into an upright 112×112 crop that SFace reads as a 128-number vector.

use std::path::Path;

use lightcraft_raster::resample::{self, Filter};
use lightcraft_raster::{Image, Rgb32f, Rgba8};
use rten::{Model, ModelOptions, NodeId, ValueView};
use rten_tensor::AsView;

use super::{DETECTOR_FILE, DIM, ENGINE, FaceEngine, FaceFound, MAX_FACES, RECOGNISER_FILE};
use crate::Error;

/// The detector's input side, in pixels.
const SIDE: usize = 640;
const STRIDES: [usize; 3] = [8, 16, 32];
/// Faces the detector is less sure of than this are not faces.
const SCORE_MIN: f32 = 0.85;
/// Two boxes overlapping more than this are one face.
const NMS_IOU: f32 = 0.3;
/// Faces narrower than this many pixels of the photo are too small to tell apart.
const MIN_FACE_PX: f32 = 32.0;
/// The recogniser's input side.
const CROP: usize = 112;
/// SFace's template: where the five landmarks (the eye at the image's left, the eye at its right,
/// the nose, the left and right corners of the mouth) sit in the 112×112 crop.
const TEMPLATE: [[f32; 2]; 5] = [[38.2946, 51.6963], [73.5318, 51.5014], [56.0252, 71.7366], [41.5493, 92.3655], [70.7299, 92.2041]];
/// Largest photo accepted, in pixels.
const MAX_PIXELS: usize = 64 << 20;

/// One face the detector found, in the photo's pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Detection {
    /// `[x0, y0, x1, y1]`.
    pub rect: [f32; 4],
    pub score: f32,
    /// The right eye (the one at the image's left), the left eye, the nose and the right and left
    /// corners of the mouth.
    pub landmarks: [[f32; 2]; 5],
}

struct Net {
    model: Model,
    input: NodeId,
    outputs: Vec<NodeId>,
}

impl Net {
    /// Loads `bytes` and finds the named outputs (the model's own, in order, when `names` is empty).
    fn load(bytes: Vec<u8>, what: &str, names: &[&str]) -> Result<Net, Error> {
        let bad = |e: &dyn std::fmt::Display| Error::Model(format!("{what}: {e}"));
        let model = ModelOptions::with_all_ops().load(bytes).map_err(|e| bad(&e))?;
        let input = model.input_ids().first().copied().ok_or_else(|| bad(&"the model has no input"))?;
        let outputs = if names.is_empty() {
            model.output_ids().iter().take(1).copied().collect()
        } else {
            names.iter().map(|n| model.node_id(n).map_err(|e| bad(&format!("no output {n}: {e}")))).collect::<Result<Vec<_>, _>>()?
        };
        if outputs.is_empty() {
            return Err(bad(&"the model has no output"));
        }
        Ok(Net { model, input, outputs })
    }

    /// Runs the model on `data` shaped `shape`; every output's values, flattened.
    fn run(&self, shape: [usize; 4], data: &[f32]) -> Result<Vec<Vec<f32>>, Error> {
        let input = ValueView::from_shape(shape, data).map_err(|e| Error::Model(format!("face input: {e}")))?;
        let out = self.model.run(vec![(self.input, input.into())], &self.outputs, None).map_err(|e| Error::Model(format!("faces: {e}")))?;
        out.iter()
            .map(|v| v.as_tensor_view::<f32>().map(|t| t.to_vec()).ok_or_else(|| Error::Model("a face model's answer isn't numbers".into())))
            .collect()
    }
}

/// The face models, loaded.
pub struct Faces {
    det: Net,
    rec: Net,
}

/// Whether `dir` holds the files [`Faces::load`] reads.
pub fn is_model_dir(dir: &Path) -> bool {
    [DETECTOR_FILE, RECOGNISER_FILE].iter().all(|f| dir.join(f).is_file())
}

impl Faces {
    pub fn load(dir: &Path) -> Result<Faces, Error> {
        if !is_model_dir(dir) {
            return Err(Error::Missing(dir.to_path_buf()));
        }
        let read = |f: &str| std::fs::read(dir.join(f)).map_err(|e| Error::Model(format!("{f}: {e}")));
        let names = ["cls_8", "cls_16", "cls_32", "obj_8", "obj_16", "obj_32", "bbox_8", "bbox_16", "bbox_32", "kps_8", "kps_16", "kps_32"];
        let det = Net::load(read(DETECTOR_FILE)?, "face detector", &names)?;
        // (SFace's export needs a few fixes to run; the file on disk is untouched, see `onnxfix`)
        let rec = Net::load(crate::onnxfix::inference_only(&read(RECOGNISER_FILE)?)?, "face recogniser", &[])?;
        let faces = Faces { det, rec };
        faces.warm_up()?;
        Ok(faces)
    }

    /// Runs each network once on blank input, so the first real photo doesn't pay for preparing
    /// the weights and starting the worker threads.
    fn warm_up(&self) -> Result<(), Error> {
        self.det.run([1, 3, SIDE, SIDE], &vec![0f32; 3 * SIDE * SIDE])?;
        self.rec.run([1, 3, CROP, CROP], &vec![0f32; 3 * CROP * CROP])?;
        Ok(())
    }

    /// The faces the detector finds in a photo (display-referred, any size), best first, in the
    /// photo's pixels.
    pub fn detect(&self, img: &Rgba8) -> Result<Vec<Detection>, Error> {
        let n = img.width.checked_mul(img.height).filter(|&n| n > 0 && n <= MAX_PIXELS && n == img.data.len());
        if n.is_none() {
            return Err(Error::Invalid("image size"));
        }
        let (w, h) = (img.width as f32, img.height as f32);
        let scale = SIDE as f32 / w.max(h);
        let (tw, th) = (((w * scale).round() as usize).clamp(1, SIDE), ((h * scale).round() as usize).clamp(1, SIDE));
        let rgb: Rgb32f = Image {
            width: img.width,
            height: img.height,
            data: img.data.iter().map(|&[r, g, b, _]| [f32::from(r) / 255.0, f32::from(g) / 255.0, f32::from(b) / 255.0]).collect(),
        };
        let small = resample::resize(&rgb, tw, th, Filter::Bilinear);
        // BGR, 0–255, the picture in the top left of a black square
        let plane = SIDE * SIDE;
        let mut input = vec![0f32; 3 * plane];
        for y in 0..th {
            for x in 0..tw {
                let Some(px) = small.data.get(y * tw + x) else { continue };
                for (k, src) in [2usize, 1, 0].into_iter().enumerate() {
                    if let (Some(o), Some(v)) = (input.get_mut(k * plane + y * SIDE + x), px.get(src)) {
                        *o = v * 255.0;
                    }
                }
            }
        }
        let outs = self.det.run([1, 3, SIDE, SIDE], &input)?;
        let mut found = decode(&outs)?;
        found = nms(found);
        for d in &mut found {
            d.rect = [d.rect[0] / scale, d.rect[1] / scale, d.rect[2] / scale, d.rect[3] / scale];
            d.rect = [d.rect[0].clamp(0.0, w), d.rect[1].clamp(0.0, h), d.rect[2].clamp(0.0, w), d.rect[3].clamp(0.0, h)];
            for l in &mut d.landmarks {
                *l = [l[0] / scale, l[1] / scale];
            }
        }
        found.retain(|d| d.rect[2] - d.rect[0] >= MIN_FACE_PX && d.rect[3] - d.rect[1] >= MIN_FACE_PX);
        found.truncate(MAX_FACES);
        Ok(found)
    }

    /// What a detected face looks like: 128 numbers of unit length.
    pub fn embed(&self, img: &Rgba8, d: &Detection) -> Result<Vec<f32>, Error> {
        let crop = align(img, &d.landmarks).ok_or(Error::Invalid("face landmarks"))?;
        let mut out = self.rec.run([1, 3, CROP, CROP], &crop)?;
        let v = out.pop().filter(|v| v.len() == DIM).ok_or_else(|| Error::Model("the recogniser's answer has the wrong size".into()))?;
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if !n.is_finite() || n < 1e-6 || v.iter().any(|x| !x.is_finite()) {
            return Err(Error::Model("the recogniser's answer isn't a face".into()));
        }
        Ok(v.iter().map(|x| x / n).collect())
    }
}

impl FaceEngine for Faces {
    fn engine(&self) -> &str {
        ENGINE
    }

    fn faces(&self, img: &Rgba8) -> Result<Vec<FaceFound>, Error> {
        let (w, h) = (img.width as f32, img.height as f32);
        let mut out = Vec::new();
        for d in self.detect(img)? {
            let embedding = self.embed(img, &d)?;
            out.push(FaceFound { rect: [d.rect[0] / w, d.rect[1] / h, d.rect[2] / w, d.rect[3] / h], score: d.score, embedding });
        }
        Ok(out)
    }
}

/// The detector's twelve answers (class, objectness, box and landmarks for strides 8, 16 and 32)
/// as detections in the 640×640 square.
fn decode(outs: &[Vec<f32>]) -> Result<Vec<Detection>, Error> {
    let bad = || Error::Model("the face detector's answer has the wrong size".into());
    let mut found = Vec::new();
    for (k, stride) in STRIDES.into_iter().enumerate() {
        let cols = SIDE / stride;
        let cells = cols * cols;
        let (cls, obj, bbox, kps) =
            (outs.get(k).ok_or_else(bad)?, outs.get(3 + k).ok_or_else(bad)?, outs.get(6 + k).ok_or_else(bad)?, outs.get(9 + k).ok_or_else(bad)?);
        if cls.len() != cells || obj.len() != cells || bbox.len() != cells * 4 || kps.len() != cells * 10 {
            return Err(bad());
        }
        let s = stride as f32;
        for idx in 0..cells {
            let (Some(c), Some(o)) = (cls.get(idx), obj.get(idx)) else { continue };
            let score = (c.clamp(0.0, 1.0) * o.clamp(0.0, 1.0)).sqrt();
            if score.is_nan() || score < SCORE_MIN {
                continue;
            }
            let (row, col) = ((idx / cols) as f32, (idx % cols) as f32);
            let b = bbox.get(idx * 4..idx * 4 + 4).ok_or_else(bad)?;
            let k10 = kps.get(idx * 10..idx * 10 + 10).ok_or_else(bad)?;
            let (cx, cy) = ((col + b[0]) * s, (row + b[1]) * s);
            let (bw, bh) = (b[2].exp() * s, b[3].exp() * s);
            let mut landmarks = [[0f32; 2]; 5];
            for (n, l) in landmarks.iter_mut().enumerate() {
                *l = [(k10.get(2 * n).copied().unwrap_or(0.0) + col) * s, (k10.get(2 * n + 1).copied().unwrap_or(0.0) + row) * s];
            }
            let rect = [cx - bw / 2.0, cy - bh / 2.0, cx + bw / 2.0, cy + bh / 2.0];
            if rect.iter().chain(landmarks.iter().flatten()).all(|v| v.is_finite()) {
                found.push(Detection { rect, score, landmarks });
            }
        }
    }
    Ok(found)
}

fn iou(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    let (w, h) = ((a[2].min(b[2]) - a[0].max(b[0])).max(0.0), (a[3].min(b[3]) - a[1].max(b[1])).max(0.0));
    let inter = w * h;
    let union = (a[2] - a[0]) * (a[3] - a[1]) + (b[2] - b[0]) * (b[3] - b[1]) - inter;
    if union > 0.0 { inter / union } else { 0.0 }
}

/// The most certain boxes, dropping any that overlaps a better one too much.
fn nms(mut found: Vec<Detection>) -> Vec<Detection> {
    found.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut kept: Vec<Detection> = Vec::new();
    for d in found {
        if kept.iter().all(|k| iou(&k.rect, &d.rect) <= NMS_IOU) {
            kept.push(d);
        }
    }
    kept
}

/// The face turned upright: a 112×112 crop (planar RGB, 0–255) made by the similarity transform
/// (turn, scale, shift; no skew) that fits the five landmarks to SFace's template best.
fn align(img: &Rgba8, landmarks: &[[f32; 2]; 5]) -> Option<Vec<f32>> {
    let n = 5.0f32;
    let mean = |pts: &[[f32; 2]; 5]| pts.iter().fold([0f32; 2], |m, p| [m[0] + p[0] / n, m[1] + p[1] / n]);
    let (mp, mq) = (mean(landmarks), mean(&TEMPLATE));
    let (mut sxx, mut sa, mut sb) = (0f32, 0f32, 0f32);
    for (p, q) in landmarks.iter().zip(&TEMPLATE) {
        let (px, py, qx, qy) = (p[0] - mp[0], p[1] - mp[1], q[0] - mq[0], q[1] - mq[1]);
        sxx += px * px + py * py;
        sa += px * qx + py * qy;
        sb += px * qy - py * qx;
    }
    if !(sxx.is_finite() && sxx > 1e-3) {
        return None;
    }
    // template = [a -b; b a] (landmark − their mean) + template mean
    let (a, b) = (sa / sxx, sb / sxx);
    let det = a * a + b * b;
    if !(det.is_finite() && det > 1e-9) {
        return None;
    }
    let (w, h) = (img.width as i64, img.height as i64);
    let at = |x: i64, y: i64, c: usize| -> f32 {
        if x < 0 || y < 0 || x >= w || y >= h {
            return 0.0;
        }
        img.data.get((y * w + x) as usize).and_then(|p| p.get(c)).map_or(0.0, |&v| f32::from(v))
    };
    let plane = CROP * CROP;
    let mut out = vec![0f32; 3 * plane];
    for v in 0..CROP {
        for u in 0..CROP {
            // where this crop pixel comes from in the photo
            let (dx, dy) = (u as f32 - mq[0], v as f32 - mq[1]);
            let (sx, sy) = (mp[0] + (a * dx + b * dy) / det, mp[1] + (-b * dx + a * dy) / det);
            let (x0, y0) = (sx.floor(), sy.floor());
            let (fx, fy) = (sx - x0, sy - y0);
            let (x0, y0) = (x0 as i64, y0 as i64);
            for c in 0..3 {
                let top = at(x0, y0, c) * (1.0 - fx) + at(x0 + 1, y0, c) * fx;
                let bottom = at(x0, y0 + 1, c) * (1.0 - fx) + at(x0 + 1, y0 + 1, c) * fx;
                if let Some(o) = out.get_mut(c * plane + v * CROP + u) {
                    *o = top * (1.0 - fy) + bottom * fy;
                }
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_landmarks_of_a_face_already_in_place_make_the_crop_the_photo() {
        // a photo whose landmarks are the template: the crop is the photo
        let mut data = vec![[0u8, 0, 0, 255]; CROP * CROP];
        for (i, p) in data.iter_mut().enumerate() {
            *p = [(i % CROP) as u8, (i / CROP) as u8, 7, 255];
        }
        let img = Image { width: CROP, height: CROP, data };
        let crop = align(&img, &TEMPLATE).unwrap();
        let plane = CROP * CROP;
        for (x, y) in [(10usize, 10usize), (50, 60), (100, 90)] {
            assert!((crop[y * CROP + x] - x as f32).abs() < 0.01, "red at {x},{y}");
            assert!((crop[plane + y * CROP + x] - y as f32).abs() < 0.01, "green at {x},{y}");
            assert!((crop[2 * plane + y * CROP + x] - 7.0).abs() < 0.01);
        }
    }

    #[test]
    fn a_turned_and_scaled_face_is_put_upright() {
        // the template turned 30° and doubled, moved to (300, 200): aligning must undo that, so the
        // crop's centre pixel comes from the photo at the landmarks' mean
        let (s, c) = (30f32.to_radians().sin_cos().0 * 2.0, 30f32.to_radians().sin_cos().1 * 2.0);
        let mt = [TEMPLATE.iter().map(|p| p[0]).sum::<f32>() / 5.0, TEMPLATE.iter().map(|p| p[1]).sum::<f32>() / 5.0];
        let lm = TEMPLATE.map(|p| {
            let (x, y) = (p[0] - mt[0], p[1] - mt[1]);
            [300.0 + c * x - s * y, 200.0 + s * x + c * y]
        });
        let mut data = vec![[10u8, 10, 10, 255]; 600 * 400];
        // a bright dot where the landmarks' mean is
        for dy in -3i32..=3 {
            for dx in -3i32..=3 {
                data[((200 + dy) as usize) * 600 + (300 + dx) as usize] = [250, 250, 250, 255];
            }
        }
        let img = Image { width: 600, height: 400, data };
        let crop = align(&img, &lm).unwrap();
        let (cx, cy) = (mt[0].round() as usize, mt[1].round() as usize);
        assert!(crop[cy * CROP + cx] > 200.0, "the dot lands on the template's centre: {}", crop[cy * CROP + cx]);
        assert!(crop[5 * CROP + 5] < 30.0);
    }

    #[test]
    fn degenerate_landmarks_make_no_crop() {
        let img = Image { width: 64, height: 64, data: vec![[0u8, 0, 0, 255]; 64 * 64] };
        assert!(align(&img, &[[5.0, 5.0]; 5]).is_none(), "all landmarks on one point");
        assert!(align(&img, &[[f32::NAN, 1.0]; 5]).is_none());
        // far outside the photo is fine: black, not a panic
        assert!(align(&img, &TEMPLATE.map(|p| [p[0] + 1e6, p[1]])).is_some());
    }

    #[test]
    fn overlapping_boxes_keep_the_surest() {
        let d = |x: f32, score: f32| Detection { rect: [x, 0.0, x + 100.0, 100.0], score, landmarks: [[0.0; 2]; 5] };
        let kept = nms(vec![d(0.0, 0.9), d(10.0, 0.95), d(300.0, 0.88), d(305.0, 0.87)]);
        assert_eq!(kept.iter().map(|k| k.score).collect::<Vec<_>>(), [0.95, 0.88]);
        assert_eq!(iou(&[0.0, 0.0, 10.0, 10.0], &[20.0, 20.0, 30.0, 30.0]), 0.0);
        assert!((iou(&[0.0, 0.0, 10.0, 10.0], &[0.0, 0.0, 10.0, 10.0]) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn the_detectors_answers_are_decoded_and_checked() {
        // one confident cell at stride 32, row 2 column 3, a 64×96 box, nothing anywhere else
        let sizes = |cells: usize| [cells, cells, cells * 4, cells * 10];
        let mut outs: Vec<Vec<f32>> = Vec::new();
        for kind in 0..4 {
            for stride in STRIDES {
                let cells = (SIDE / stride) * (SIDE / stride);
                outs.push(vec![0.0; sizes(cells)[kind]]);
            }
        }
        let cell = 2 * 20 + 3;
        outs[2][cell] = 1.0; // class, stride 32
        outs[5][cell] = 1.0; // objectness
        outs[8][cell * 4..cell * 4 + 4].copy_from_slice(&[0.5, 0.5, 2f32.ln(), 3f32.ln()]);
        let found = decode(&outs).unwrap();
        assert_eq!(found.len(), 1);
        let d = &found[0];
        let (cx, cy) = (3.5 * 32.0, 2.5 * 32.0);
        assert_eq!(d.rect, [cx - 32.0, cy - 48.0, cx + 32.0, cy + 48.0]);
        assert!((d.score - 1.0).abs() < 1e-6);
        // an answer of the wrong size is an error, never an out-of-range read
        outs[0].pop();
        assert!(decode(&outs).is_err());
        assert!(decode(&[]).is_err());
    }
}
