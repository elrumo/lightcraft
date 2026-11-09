//! **Surface normals** for Relight: which way each part of a photo faces, guessed from the picture
//! alone by MoGe-2 (Wang et al., "MoGe-2: Accurate Monocular Geometry with Metric Scale and Sharp
//! Details", Microsoft Research 2025) in its smallest size with a normal head (ViT-S, 35M
//! parameters), on rten like Depth Anything ([`crate::Estimator`]). The depth's slope says the same
//! thing far more coarsely: a relative depth map has steps and no perspective, so a face lit from the
//! side shows terraces and a floor seen from above faces the camera like a wall. The network's
//! normals show the shape: Relight shades with them and still casts its shadows through the depth.
//!
//! The weights are not part of LightCraft: the user downloads them when they ask for a finer surface
//! ([`FILE`] says what to fetch). [`Surface::load`] takes the published file unchanged.
//!
//! The network sees the photo as a grid of about [`TOKENS`] 14-pixel patches (its own resize is
//! bilinear without antialiasing, so [`Surface::estimate`] makes the picture that size first) and
//! answers with camera-space normals (x right, y down, z away from the camera) and a mask of what has
//! no surface (sky). [`Surface::estimate`] turns them to face the camera (z toward it) and makes
//! the unmasked ones face it straight.

use lightcraft_raster::resample::{Filter, resize};
use lightcraft_raster::{Plane, Rgb32f};
use rten::{Model, ModelOptions, NodeId, ValueView};
use rten_tensor::AsView;

use crate::{Error, MAX_SIDE, Pinned, Result};

/// Stable id: its folder under `models/` and its id in commands.
pub const ID: &str = "moge-2-vits-normal";
pub const LABEL: &str = "MoGe-2 Surface (ViT-S)";
/// The ONNX export (one file: the graph and its weights).
pub const FILE: Pinned = Pinned { name: "model.onnx", size: 140_852_051, sha256: "24eacb5dc7a2c54c7bc98f7de085ffbed79ad006ea5b664c2c2cdc02ff3a52f0" };
/// The author's ONNX export at a fixed revision (public, no account). `<BASE_URL>/<file name>`.
pub const BASE_URL: &str = "https://huggingface.co/Ruicheng/moge-2-vits-normal-onnx/resolve/e50ffda41565591092adea54c6ac83d6212e1e23";
/// Licence verdict, read 2026-11-09: the Hugging Face card of Ruicheng/moge-2-vits-normal (the
/// weights the export is made from, by the same author) says MIT, as does the code's `LICENSE` at
/// github.com/microsoft/MoGe; its DINOv2 backbone is Apache-2.0.
pub const LICENCE: &str = "MIT";
pub const LICENCE_URL: &str = "https://huggingface.co/Ruicheng/moge-2-vits-normal";
pub const CREDIT: &str = "MoGe-2 by Ruicheng Wang et al. (Microsoft Research); ONNX export by Ruicheng Wang";
pub const HOME_URL: &str = "https://github.com/microsoft/MoGe";

/// How many of its 14-pixel patches the network sees the photo as (its authors suggest 1200 to 2500):
/// about 500 pixels on the long side of a 3:2 photo, more than the [`shrunk`] copy that is kept. Measured on
/// two photos against 2000 patches, after that shrink to 384 pixels: the normals turn by 1.5° to 2° (median),
/// which a relit photo doesn't show, and the estimate takes 1.5 s instead of 2.7 s on a 4-core CPU.
pub const TOKENS: usize = 1200;
/// The network's patch size.
const PATCH: usize = 14;
/// Below this, the mask says the network sees no surface there (sky, the infinitely far).
const MASK_EDGE: f32 = 0.5;
/// The network's normals show faint traces of its patches: a Gaussian of this many of its pixels takes them out.
const SMOOTH_SIGMA: f32 = 1.0;

/// Unit normals over the picture the network saw (x right, y down, z toward the camera), row-major.
#[derive(Clone, Debug, PartialEq)]
pub struct Normals {
    pub width: usize,
    pub height: usize,
    pub data: Vec<[f32; 3]>,
}

/// The network's input size for a `w × h` picture: about [`TOKENS`] patches of the picture's shape.
pub fn net_size(w: usize, h: usize) -> (usize, usize) {
    let aspect = w.max(1) as f64 / h.max(1) as f64;
    let rows = ((TOKENS as f64 / aspect).sqrt().round() as usize).max(1);
    let cols = ((TOKENS as f64 * aspect).sqrt().round() as usize).max(1);
    (cols * PATCH, rows * PATCH)
}

/// MoGe-2 ViT-S, loaded.
pub struct Surface {
    model: Box<Model>,
    image: NodeId,
    tokens: NodeId,
    normal: NodeId,
    mask: Option<NodeId>,
}

impl Surface {
    /// From the bytes of [`FILE`], read through `lightcraft_vision::onnxfix` first, as Depth Anything is.
    pub fn load(bytes: &[u8]) -> Result<Surface> {
        let bad = |e: &dyn std::fmt::Display| Error::Model(e.to_string());
        let fixed = lightcraft_vision::onnxfix::inference_only(bytes).map_err(|e| bad(&e))?;
        let model = ModelOptions::with_all_ops().load(fixed).map_err(|e| bad(&e))?;
        let named = |ids: &[NodeId], name: &str| {
            ids.iter().copied().find(|id| model.node_info(*id).and_then(|i| i.name().map(|n| n == name)).unwrap_or(false))
        };
        let (inputs, outputs) = (model.input_ids().to_vec(), model.output_ids().to_vec());
        let image = named(&inputs, "image").ok_or_else(|| bad(&"the model has no `image` input"))?;
        let tokens = named(&inputs, "num_tokens").ok_or_else(|| bad(&"the model has no `num_tokens` input"))?;
        let normal = named(&outputs, "normal").ok_or_else(|| bad(&"the model has no `normal` output"))?;
        let mask = named(&outputs, "mask");
        Ok(Surface { model: Box::new(model), image, tokens, normal, mask })
    }

    /// The normals of `img` (display-encoded, sRGB-like 0..=1, at most [`MAX_SIDE`] per side) at the
    /// network's resolution ([`net_size`]).
    pub fn estimate(&self, img: &Rgb32f) -> Result<Normals> {
        let (w, h) = (img.width, img.height);
        if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE || img.data.len() != w * h {
            return Err(Error::Image(format!("can't estimate the surface of a {w} × {h} image")));
        }
        let (nw, nh) = net_size(w, h);
        let small = resize(img, nw, nh, Filter::Mitchell);
        let n = nw * nh;
        let mut pixels = vec![0f32; 3 * n];
        for (i, p) in small.data.iter().enumerate() {
            for c in 0..3 {
                if let Some(o) = pixels.get_mut(c * n + i) {
                    // (the network normalizes the picture itself)
                    *o = p[c].clamp(0.0, 1.0);
                }
            }
        }
        let bad = |e: &dyn std::fmt::Display| Error::Model(e.to_string());
        let image = ValueView::from_shape([1, 3, nh, nw], &pixels[..]).map_err(|e| bad(&e))?;
        let count = [TOKENS as i32];
        let tokens = ValueView::from_shape([], &count[..]).map_err(|e| bad(&e))?;
        let mut wanted = vec![self.normal];
        wanted.extend(self.mask);
        let out = self.model.run(vec![(self.image, image.into()), (self.tokens, tokens.into())], &wanted, None).map_err(|e| bad(&e))?;
        let normal = out.first().and_then(|v| v.as_tensor_view::<f32>()).ok_or_else(|| bad(&"its normals aren't numbers"))?.to_vec();
        if normal.len() != 3 * n {
            return Err(bad(&format!("its normals have {} numbers, not {}", normal.len(), 3 * n)));
        }
        let mask = out.get(1).and_then(|v| v.as_tensor_view::<f32>()).map(|t| t.to_vec()).filter(|m| m.len() == n);
        Ok(from_camera(nw, nh, &normal, mask.as_deref()))
    }
}

/// The network's answer (camera space: x right, y down, z away from the camera, `[h][w][3]`) as
/// [`Normals`] facing the camera, smoothed a little; where `mask` says there is no surface, facing it straight.
pub fn from_camera(w: usize, h: usize, raw: &[f32], mask: Option<&[f32]>) -> Normals {
    let mut planes = [Plane::new(w, h), Plane::new(w, h), Plane::new(w, h)];
    for i in 0..w * h {
        let v = [raw.get(3 * i).copied().unwrap_or(0.0), raw.get(3 * i + 1).copied().unwrap_or(0.0), -raw.get(3 * i + 2).copied().unwrap_or(-1.0)];
        let sky = mask.and_then(|m| m.get(i)).is_some_and(|m| m.is_nan() || *m < MASK_EDGE);
        let v = if sky || !v.iter().all(|c| c.is_finite()) { [0.0, 0.0, 1.0] } else { v };
        for (p, c) in planes.iter_mut().zip(v) {
            if let Some(o) = p.data.get_mut(i) {
                *o = c;
            }
        }
    }
    let planes = planes.map(|p| lightcraft_raster::blur::gaussian(&p, SMOOTH_SIGMA));
    let data = (0..w * h)
        .map(|i| {
            let v = [planes[0].data[i], planes[1].data[i], planes[2].data[i].max(0.0)];
            let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            if l.is_finite() && l > 1e-6 { v.map(|c| c / l) } else { [0.0, 0.0, 1.0] }
        })
        .collect();
    Normals { width: w, height: h, data }
}

/// `n` at `w × h` (at most as large: never enlarged), each component averaged over the pixels it
/// covers and the vectors made one long again: what is stored with the settings.
pub fn shrunk(n: &Normals, w: usize, h: usize) -> Normals {
    let (w, h) = (w.clamp(1, n.width.max(1)), h.clamp(1, n.height.max(1)));
    if (w, h) == (n.width, n.height) {
        return n.clone();
    }
    let plane = |k: usize| Plane { width: n.width, height: n.height, data: n.data.iter().map(|v| v[k]).collect() };
    let p = [0, 1, 2].map(|k| resize(&plane(k), w, h, Filter::Box));
    let data = (0..w * h)
        .map(|i| {
            let v = [p[0].data[i], p[1].data[i], p[2].data[i].max(0.0)];
            let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            if l.is_finite() && l > 1e-6 { v.map(|c| c / l) } else { [0.0, 0.0, 1.0] }
        })
        .collect();
    Normals { width: w, height: h, data }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_network_sees_about_its_tokens_in_the_photos_shape() {
        let (w, h) = net_size(3000, 2000);
        assert_eq!((w % PATCH, h % PATCH), (0, 0));
        let tokens = (w / PATCH) * (h / PATCH);
        assert!((tokens as f64 - TOKENS as f64).abs() < 0.05 * TOKENS as f64, "{tokens}");
        assert!((w as f64 / h as f64 - 1.5).abs() < 0.05, "{w} × {h}");
        assert_eq!(net_size(0, 0).0 % PATCH, 0);
    }

    #[test]
    fn camera_space_turns_to_face_the_camera_and_the_sky_faces_it_straight() {
        // (camera space: z away from the camera, so a surface facing it has z −1)
        let (w, h) = (6, 4);
        let mut raw = Vec::new();
        for i in 0..w * h {
            raw.extend_from_slice(if i % w < 3 { &[-0.6, 0.0, -0.8] } else { &[0.0, -0.6, -0.8] });
        }
        let mut mask = vec![1.0f32; w * h];
        mask[w * h - 1] = 0.1;
        let n = from_camera(w, h, &raw, Some(&mask));
        assert_eq!((n.width, n.height, n.data.len()), (w, h, w * h));
        // the left half faces left, the right half up the photo (y runs down), both toward the camera
        let left = n.data[w];
        assert!(left[0] < -0.4 && left[2] > 0.6, "{left:?}");
        let right = n.data[w + 4];
        assert!(right[1] < -0.3 && right[2] > 0.6, "{right:?}");
        assert!(n.data.iter().all(|v| ((v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt() - 1.0).abs() < 1e-4 && v[2] >= 0.0));
        // nonsense faces the camera
        let odd = from_camera(1, 1, &[f32::NAN, 0.0, 0.0], None);
        assert_eq!(odd.data, vec![[0.0, 0.0, 1.0]]);
    }

    #[test]
    fn shrinking_keeps_unit_normals_and_never_enlarges() {
        let n = Normals { width: 4, height: 2, data: vec![[0.6, 0.0, 0.8]; 8] };
        let s = shrunk(&n, 2, 1);
        assert_eq!((s.width, s.height), (2, 1));
        assert!(s.data.iter().all(|v| (v[0] - 0.6).abs() < 1e-4 && (v[2] - 0.8).abs() < 1e-4));
        assert_eq!(shrunk(&n, 10, 10), n);
    }

    /// With `LIGHTCRAFT_SURFACE_MODEL` (the path of [`FILE`]): a lit ball in front of a wall faces left on its left and
    /// right on its right.
    #[test]
    fn the_model_sees_a_ball_round() {
        let Ok(path) = std::env::var("LIGHTCRAFT_SURFACE_MODEL") else { return };
        let bytes = std::fs::read(path).unwrap();
        assert!(FILE.matches(&bytes), "not the pinned file");
        let s = Surface::load(&bytes).unwrap();
        let (w, h) = (640usize, 480usize);
        let img = Rgb32f::from_fn(w, h, |x, y| {
            let (dx, dy) = ((x as f32 - 320.0) / 150.0, (y as f32 - 240.0) / 150.0);
            let r = dx * dx + dy * dy;
            if r < 1.0 {
                let n = [dx, dy, (1.0 - r).sqrt()];
                let l = (-0.5 * n[0] - 0.5 * n[1] + 0.7 * n[2]).max(0.0);
                [0.15 + 0.8 * l, 0.12 + 0.7 * l, 0.1 + 0.6 * l]
            } else {
                [0.45, 0.45, 0.42]
            }
        });
        let t = std::time::Instant::now();
        let n = s.estimate(&img).unwrap();
        eprintln!("surface {} × {}: {:.0} ms", n.width, n.height, t.elapsed().as_secs_f64() * 1e3);
        let at = |fx: f32, fy: f32| n.data[(fy * n.height as f32) as usize * n.width + (fx * n.width as f32) as usize];
        let (left, right, top) = (at(0.5 - 0.18, 0.5), at(0.5 + 0.18, 0.5), at(0.5, 0.5 - 0.24));
        assert!(left[0] < -0.2 && right[0] > 0.2 && top[1] < -0.2, "{left:?} {right:?} {top:?}");
    }
}
