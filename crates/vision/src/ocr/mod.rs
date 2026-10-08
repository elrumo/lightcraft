//! Reading the text in a photo: PP-OCRv6 small (PaddleOCR, Apache-2.0) on `rten`, a pure-Rust
//! ONNX runtime. A detector finds text lines (a probability map, turned into boxes by [`dbnet`]),
//! each line is cut out and straightened ([`crop`]), and a recogniser reads it ([`ctc`]).
//! English, Simplified and Traditional Chinese, Japanese and 46 other Latin-script languages.
//!
//! The model files are not part of LightCraft: the user downloads them (see [`crate::models`]).
//! They are read from one folder: `det/inference.onnx`, `rec/inference.onnx` and
//! `rec/inference.yml` (the character dictionary).
//!
//! Known limits: text upside down is not read (there is no orientation classifier yet), nor is
//! curved text; very small text needs a render of the photo at 1280 px or more.

pub mod crop;
pub mod ctc;
pub mod dbnet;

use std::path::Path;

use lightcraft_raster::resample::{self, Filter};
use lightcraft_raster::{Image, Rgb32f, Rgba8};
use rten::{Model, NodeId, ValueView};

use crate::Error;
use dbnet::{Params, Rect};

/// Names the text index these models fill (a different model would read differently).
pub const ENGINE: &str = "ppocrv6-small";
pub const DET_FILE: &str = "det/inference.onnx";
pub const REC_FILE: &str = "rec/inference.onnx";
pub const DICT_FILE: &str = "rec/inference.yml";

/// The detector's input: the long side is brought into this range (a multiple of 32 each way).
const DET_MIN_LONG: usize = 640;
const DET_MAX_LONG: usize = 1280;
/// Lines read per photo at most, and bytes of text kept.
const MAX_LINES: usize = 300;
const MAX_TEXT: usize = 64 << 10;
/// Widest recogniser input.
const REC_MAX_W: usize = 3200;
/// Lines per recogniser pass (the batch is padded to its widest line).
const REC_BATCH: usize = 8;
/// A line read with less confidence than this is dropped (PaddleOCR's own default).
const DROP_SCORE: f32 = 0.5;
/// Largest photo accepted, in pixels.
const MAX_PIXELS: usize = 64 << 20;

/// One line of text found in a photo.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub text: String,
    /// How sure the recogniser was, 0 to 1.
    pub score: f32,
    /// The line's corners in the photo's pixels, top-left first in reading order.
    pub quad: [[f32; 2]; 4],
}

struct Net {
    model: Model,
    input: NodeId,
    output: NodeId,
}

impl Net {
    fn load(path: &Path, what: &str) -> Result<Net, Error> {
        let bad = |e: &dyn std::fmt::Display| Error::Model(format!("{what}: {}: {e}", path.display()));
        let model = Model::load_file(path).map_err(|e| bad(&e))?;
        let input = model.input_ids().first().copied().ok_or_else(|| bad(&"the model has no input"))?;
        let output = model.output_ids().first().copied().ok_or_else(|| bad(&"the model has no output"))?;
        Ok(Net { model, input, output })
    }

    /// Runs the model on `data` shaped `shape` and returns its first output as (shape, values).
    fn run<const N: usize, const M: usize>(&self, shape: [usize; N], data: &[f32]) -> Result<([usize; M], Vec<f32>), Error> {
        let input = ValueView::from_shape(shape, data).map_err(|e| Error::Model(format!("OCR input: {e}")))?;
        let [out] = self.model.run_n(vec![(self.input, input.into())], [self.output], None).map_err(|e| Error::Model(format!("OCR: {e}")))?;
        out.into_shape_vec::<f32, M>().map_err(|e| Error::Model(format!("OCR output: {e}")))
    }
}

/// The OCR models, loaded.
pub struct Ocr {
    det: Net,
    rec: Net,
    dict: Vec<String>,
}

/// Whether `dir` holds the files [`Ocr::load`] reads.
pub fn is_model_dir(dir: &Path) -> bool {
    [DET_FILE, REC_FILE, DICT_FILE].iter().all(|f| dir.join(f).is_file())
}

impl Ocr {
    pub fn load(dir: &Path) -> Result<Ocr, Error> {
        if !is_model_dir(dir) {
            return Err(Error::Missing(dir.to_path_buf()));
        }
        let det = Net::load(&dir.join(DET_FILE), "text detector")?;
        let rec = Net::load(&dir.join(REC_FILE), "text recogniser")?;
        let yml = std::fs::read_to_string(dir.join(DICT_FILE)).map_err(|e| Error::Model(format!("{DICT_FILE}: {e}")))?;
        let dict = ctc::parse_dict(&yml)?;
        let ocr = Ocr { det, rec, dict };
        ocr.warm_up()?;
        Ok(ocr)
    }

    /// Runs each network once on blank input, so the first real photo doesn't pay for preparing
    /// the weights and starting the worker threads (measured: ~5 s of a ~0.6 s read).
    fn warm_up(&self) -> Result<(), Error> {
        let t = std::time::Instant::now();
        let (h, w) = (320, 320);
        self.det.run::<4, 4>([1, 3, h, w], &vec![0f32; 3 * h * w])?;
        let (rh, rw) = (crop::LINE_H, 640);
        self.rec.run::<4, 3>([REC_BATCH, 3, rh, rw], &vec![0f32; REC_BATCH * 3 * rh * rw])?;
        if std::env::var_os("LIGHTCRAFT_PROFILE").is_some() {
            eprintln!("ocr warm-up {:?}", t.elapsed());
        }
        Ok(())
    }

    /// The characters the recogniser knows (without the blank and the space).
    pub fn dictionary_len(&self) -> usize {
        self.dict.len()
    }

    /// Reads the text in a photo (display-referred; at least ~1280 px on the long side for small
    /// print), in reading order. No text is an empty list, not an error.
    pub fn read(&self, img: &Rgba8) -> Result<Vec<Line>, Error> {
        let n = img.width.checked_mul(img.height).filter(|&n| n > 0 && n <= MAX_PIXELS && n == img.data.len());
        if n.is_none() {
            return Err(Error::Invalid("image size"));
        }
        let profile = std::env::var_os("LIGHTCRAFT_PROFILE").is_some();
        let t = std::time::Instant::now();
        let rects = self.detect(img)?;
        let found = rects.len();
        let detected = t.elapsed();
        let lines = self.recognise(img, rects)?;
        if profile {
            eprintln!("ocr: detection {detected:?} ({found} boxes), recognition {:?}", t.elapsed().saturating_sub(detected));
        }
        Ok(lines)
    }

    /// Text boxes in the photo's pixels.
    fn detect(&self, img: &Rgba8) -> Result<Vec<Rect>, Error> {
        let profile = std::env::var_os("LIGHTCRAFT_PROFILE").is_some();
        let t0 = std::time::Instant::now();
        let (tw, th) = det_size(img.width, img.height);
        let rgb: Rgb32f = Image {
            width: img.width,
            height: img.height,
            data: img.data.iter().map(|&[r, g, b, _]| [f32::from(r) / 255.0, f32::from(g) / 255.0, f32::from(b) / 255.0]).collect(),
        };
        let small = resample::resize(&rgb, tw, th, Filter::Bilinear);
        // BGR, ImageNet mean and deviation as the model was trained (applied in that order)
        let (mean, std) = ([0.485f32, 0.456, 0.406], [0.229f32, 0.224, 0.225]);
        let plane = tw * th;
        let mut input = vec![0f32; 3 * plane];
        for (i, px) in small.data.iter().enumerate() {
            for (k, &src) in [2usize, 1, 0].iter().enumerate() {
                if let (Some(o), Some(v), Some(m), Some(s)) = (input.get_mut(k * plane + i), px.get(src), mean.get(k), std.get(k)) {
                    *o = (v - m) / s;
                }
            }
        }
        let prepared = t0.elapsed();
        let (shape, map): ([usize; 4], Vec<f32>) = self.det.run([1, 3, th, tw], &input)?;
        let ran = t0.elapsed().saturating_sub(prepared);
        let (mh, mw) = (shape[2], shape[3]);
        let mut rects = dbnet::boxes(&map, mw, mh, &Params::default());
        if profile {
            eprintln!("ocr detect {tw}x{th}: prepare {prepared:?}, model {ran:?}, boxes {:?}", t0.elapsed().saturating_sub(prepared + ran));
        }
        // map pixels → the photo's
        let (sx, sy) = (img.width as f32 / mw.max(1) as f32, img.height as f32 / mh.max(1) as f32);
        for r in &mut rects {
            let v = r.v();
            let (ux, uy) = (r.u[0] * sx, r.u[1] * sy);
            let (vx, vy) = (v[0] * sx, v[1] * sy);
            let (lu, lv) = (ux.hypot(uy).max(f32::EPSILON), vx.hypot(vy).max(f32::EPSILON));
            r.c = [r.c[0] * sx, r.c[1] * sy];
            r.u = [ux / lu, uy / lu];
            r.half = [r.half[0] * lu, r.half[1] * lv];
        }
        sort_reading_order(&mut rects);
        rects.truncate(MAX_LINES);
        Ok(rects)
    }

    fn recognise(&self, img: &Rgba8, rects: Vec<Rect>) -> Result<Vec<Line>, Error> {
        // cut every line out, then read them in batches of similar width
        let mut strips: Vec<(usize, usize, Vec<f32>)> =
            rects.iter().enumerate().filter_map(|(i, r)| crop::strip(img, r, REC_MAX_W).map(|(w, d)| (i, w, d))).collect();
        let mut texts: Vec<Option<(String, f32)>> = vec![None; rects.len()];
        strips.sort_by_key(|s| s.1);
        for batch in strips.chunks(REC_BATCH) {
            let width = batch.iter().map(|s| s.1).max().unwrap_or(1);
            let h = crop::LINE_H;
            let mut input = vec![0f32; batch.len() * 3 * h * width];
            for (b, (_, w, data)) in batch.iter().enumerate() {
                for k in 0..3 {
                    for y in 0..h {
                        let (src, dst) = ((k * h + y) * w, ((b * 3 + k) * h + y) * width);
                        if let (Some(from), Some(to)) = (data.get(src..src + w), input.get_mut(dst..dst + w)) {
                            to.copy_from_slice(from);
                        }
                    }
                }
            }
            let (shape, probs): ([usize; 3], Vec<f32>) = self.rec.run([batch.len(), 3, h, width], &input)?;
            let (steps, classes) = (shape[1], shape[2]);
            for (b, (i, _, _)) in batch.iter().enumerate() {
                let row = probs.get(b * steps * classes..(b + 1) * steps * classes).unwrap_or_default();
                if let Some(slot) = texts.get_mut(*i) {
                    *slot = Some(ctc::decode(row, steps, classes, &self.dict));
                }
            }
        }
        let mut out = Vec::new();
        let mut bytes = 0;
        for (r, t) in rects.iter().zip(texts) {
            let Some((text, score)) = t else { continue };
            let text = text.trim().to_string();
            if text.is_empty() || score < DROP_SCORE {
                continue;
            }
            bytes += text.len();
            if bytes > MAX_TEXT {
                break;
            }
            out.push(Line { text, score, quad: r.corners() });
        }
        Ok(out)
    }
}

/// The detector's input size for a `w × h` photo: the long side within [`DET_MIN_LONG`,
/// `DET_MAX_LONG`], each side a multiple of 32.
fn det_size(w: usize, h: usize) -> (usize, usize) {
    let long = w.max(h).max(1) as f32;
    let target = long.clamp(DET_MIN_LONG as f32, DET_MAX_LONG as f32);
    let scale = target / long;
    let snap = |v: usize| (((v as f32 * scale) / 32.0).round() as usize).max(1) * 32;
    (snap(w), snap(h))
}

/// Top to bottom, and left to right within a row (boxes whose centres are within half a line of
/// each other share a row).
fn sort_reading_order(rects: &mut [Rect]) {
    rects.sort_by(|a, b| a.c[1].total_cmp(&b.c[1]));
    let mut start = 0;
    while start < rects.len() {
        let Some(first) = rects.get(start) else { break };
        let tolerance = first.half[1].max(1.0);
        let mut end = start + 1;
        while rects.get(end).is_some_and(|r| (r.c[1] - first.c[1]).abs() <= tolerance) {
            end += 1;
        }
        if let Some(row) = rects.get_mut(start..end) {
            row.sort_by(|a, b| a.c[0].total_cmp(&b.c[0]));
        }
        start = end;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_detector_input_is_a_multiple_of_32_in_a_sane_range() {
        for (w, h) in [(4000, 3000), (640, 480), (100, 100), (1, 1), (6000, 100), (1280, 960)] {
            let (tw, th) = det_size(w, h);
            assert!(tw % 32 == 0 && th % 32 == 0 && tw > 0 && th > 0, "{w}x{h} -> {tw}x{th}");
            assert!(tw.max(th) <= DET_MAX_LONG + 32 && tw.max(th) >= DET_MIN_LONG - 32 || w.max(h) < 32, "{w}x{h} -> {tw}x{th}");
        }
        assert_eq!(det_size(4000, 3000), (1280, 960));
    }

    #[test]
    fn lines_are_read_top_to_bottom_and_left_to_right() {
        let r = |x: f32, y: f32| Rect { c: [x, y], u: [1.0, 0.0], half: [20.0, 8.0] };
        let mut v = vec![r(300.0, 102.0), r(50.0, 40.0), r(100.0, 98.0), r(10.0, 200.0)];
        sort_reading_order(&mut v);
        let xs: Vec<f32> = v.iter().map(|r| r.c[0]).collect();
        assert_eq!(xs, [50.0, 100.0, 300.0, 10.0]);
    }

    #[test]
    fn a_missing_model_folder_is_reported() {
        let dir = std::env::temp_dir().join(format!("lc-ocr-missing-{}", std::process::id()));
        assert!(!is_model_dir(&dir));
        assert!(matches!(Ocr::load(&dir), Err(Error::Missing(_))));
    }
}
