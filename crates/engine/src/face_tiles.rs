//! Face tiles cut from a raw's embedded preview (issue #1440).
//!
//! A People card's face picture ([`crate::Session::face_job`]) is a small square around the face.
//! Developing the raw for it reads and demosaics most of a 24–50 MB file for a ~170 px picture; the
//! camera's own JPEG inside the file (read through the planned preview path,
//! [`crate::files::preview_window`], which reads the header and the preview and not the sensor
//! data) usually holds that square at the tile's size already. A [`PreviewCut`] says which preview
//! to read and how large it must be; [`crate::media::RenderJob::run`] tries it before the job's own
//! source and falls back to that source when the preview can't serve.
//!
//! Only a raw that still looks the way it was imported gets one: that is the photo whose grid
//! thumbnail is the same embedded JPEG ([`crate::Session::embedded_thumb_is_final`]). An edited raw
//! needs its edits rendered, which the camera's display-referred JPEG can't take the way the raw
//! does, so it keeps the full develop render.

use lightcraft_geom::Rect;
use lightcraft_raster::Rgba8;
use lightcraft_raster::resample::{Filter, resize};

use crate::media::PreviewLoader;

/// How much smaller than asked an embedded preview may be and still serve a face tile: at least
/// three quarters of the pixels the tile asks for across the face. The square is then enlarged by
/// at most 4⁄3 on its way to the tile, which at a 56–180 pt tile is not visible (the camera's JPEG
/// is sharpened and denoised, so its pixels hold more detail than a binned raw's), and it is the
/// same bound the raw path already accepts for an X-Trans sensor binned 3× for a preview
/// ([`crate::files::bin_factor`]). Below it the tile would look soft: the job's own source renders it.
pub const SOFT_LIMIT: f64 = 0.75;

/// Largest relative difference between the preview's and the photo's aspect ratios: a preview of
/// another shape (letterboxed, 16:9 from a 3:2 sensor, a 4:3 sensor's 3:2 JPEG) does not show the
/// same frame, and the face box would land beside the face. Real previews of the same frame differ
/// by rounding only (Sony A7S III: 1616 × 1080 against 4240 × 2832, 0.06 %).
const ASPECT_TOLERANCE: f64 = 0.02;

/// A face tile to cut from the photo's embedded preview.
#[derive(Clone)]
pub struct PreviewCut {
    /// The raw file.
    pub path: String,
    /// Reads its largest embedded preview, display-ready and upright, no larger than the edge given.
    pub loader: PreviewLoader,
    /// The preview's long edge that gives the tile its own size across the face.
    pub want: usize,
    /// The smallest long edge accepted ([`SOFT_LIMIT`] of `want`).
    pub min: usize,
    /// The square around the face, normalized on the upright photo.
    pub rect: Rect,
    /// The photo's width / height, upright.
    pub aspect: f64,
    /// The tile's edge, pixels.
    pub edge: usize,
}

impl PreviewCut {
    /// The tile, or `None` when the preview can't give it: none could be read (no preview, an
    /// offline or damaged file), it is smaller than [`Self::min`], or it shows another frame.
    pub fn cut(&self) -> Option<Rgba8> {
        let preview = (self.loader)(&self.path, self.want)?;
        self.cut_from(&preview)
    }

    /// [`Self::cut`] from a preview already read.
    pub fn cut_from(&self, img: &Rgba8) -> Option<Rgba8> {
        let (w, h) = (img.width, img.height);
        if w == 0 || h == 0 || w.max(h) < self.min || img.data.len() != w.checked_mul(h)? {
            return None;
        }
        let aspect = w as f64 / h as f64;
        if !(self.aspect.is_finite() && self.aspect > 0.0) || ((aspect - self.aspect) / self.aspect).abs() > ASPECT_TOLERANCE {
            return None;
        }
        let r = self.rect;
        if ![r.x0, r.y0, r.x1, r.y1].iter().all(|v| v.is_finite()) {
            return None;
        }
        // pixel bounds inside the preview (the clamps keep every value in 0..=w / 0..=h)
        let px = |v: f64, n: usize| (v.clamp(0.0, 1.0) * n as f64).round() as usize;
        let (x0, x1, y0, y1) = (px(r.x0, w), px(r.x1, w), px(r.y0, h), px(r.y1, h));
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        let edge = self.edge.clamp(1, crate::media::SourceLevel::Thumb.max_edge());
        let square = img.crop(x0, y0, x1 - x0, y1 - y0).to_linear();
        Some(resize(&square, edge, edge, Filter::Mitchell).to_srgb8())
    }
}

/// A face tile the sync server cuts from the previews it keeps (`GET /api/faces/tile`), for a synced photo none of
/// whose previews is on this device yet: a few kilobytes for the face, where cutting it here needs the photo's whole
/// smart preview (about 1 MB) downloaded first, once for every face (issue HB-159). [`crate::media::RenderJob::run`]
/// tries it after [`PreviewCut`] and before the job's own source, which stays the way a tile comes when the server can't
/// give it (offline, an older server, finding people off for the user, previews not built yet).
#[derive(Clone)]
pub struct ServerTile {
    /// The request: the server's address, the photo's content hash, the square around the face, the tile's edge.
    pub url: String,
    /// This device's token.
    pub token: String,
    /// The tile's edge, pixels.
    pub edge: usize,
}

/// Largest side a tile from the server may have, and the most its sides may differ by.
const TILE_MAX_SIDE: usize = 512;
const TILE_SQUARE: f64 = 1.05;

impl ServerTile {
    /// The tile, or `None` when the server can't give it now (see the type): the job's own source renders it then.
    pub fn fetch(&self) -> Option<Rgba8> {
        let bytes = crate::cloud_sync::fetch_face_tile(&self.url, &self.token)?;
        self.sized(lightcraft_preview::decode_jpeg(&bytes)?)
    }

    /// `img` as the tile: a square of a sensible size, at the tile's edge.
    fn sized(&self, img: Rgba8) -> Option<Rgba8> {
        let (w, h) = (img.width, img.height);
        if w < 8 || h < 8 || w.max(h) > TILE_MAX_SIDE || img.data.len() != w.checked_mul(h)? {
            return None;
        }
        if w.max(h) as f64 / w.min(h) as f64 > TILE_SQUARE {
            return None;
        }
        let edge = self.edge.clamp(1, crate::media::SourceLevel::Thumb.max_edge());
        if (w, h) == (edge, edge) {
            return Some(img);
        }
        Some(resize(&img.to_linear(), edge, edge, Filter::Mitchell).to_srgb8())
    }
}

/// The long edge of a photo's picture that leaves `edge` pixels across `square` (the square around a face, normalized on the
/// upright photo, `[x0, y0, x1, y1]`): what [`tile_from_preview`] renders the preview at, and so which preview serves it (a mini preview holds a
/// big face at the size of a tile, a small one needs the smart preview). `None` for a square that is empty or not finite.
#[cfg(not(target_arch = "wasm32"))]
pub fn tile_source_edge(photo: &lightcraft_catalog::Photo, square: [f64; 4], edge: usize) -> Option<usize> {
    let [x0, y0, x1, y1] = square;
    let (fw, fh) = (x1 - x0, y1 - y0);
    if !square.iter().all(|v| v.is_finite()) || fw <= 0.0 || fh <= 0.0 {
        return None;
    }
    let edge = edge.clamp(16, crate::media::SourceLevel::Thumb.max_edge());
    let along_long = if photo.width >= photo.height { fw } else { fh };
    let needed = (edge as f64 / along_long.clamp(1e-6, 1.0)).ceil();
    Some(needed.clamp(16.0, crate::media::SourceLevel::Preview.max_edge() as f64) as usize)
}

/// The tile the server serves: the square `square` around a face (`[x0, y0, x1, y1]`, normalized on the upright photo), cut from the neutral
/// rendering of `photo`'s preview file, `edge` pixels across, rendered at [`tile_source_edge`].
#[cfg(not(target_arch = "wasm32"))]
pub fn tile_from_preview(photo: &lightcraft_catalog::Photo, preview: &std::path::Path, square: [f64; 4], edge: usize) -> Result<Rgba8, String> {
    let needed = tile_source_edge(photo, square, edge).ok_or("the square around the face is empty")?;
    let edge = edge.clamp(16, crate::media::SourceLevel::Thumb.max_edge());
    let img = crate::vision::neutral_from_preview(photo, preview, needed)?;
    let cut = PreviewCut {
        path: String::new(),
        loader: std::sync::Arc::new(|_, _| None),
        want: needed,
        min: 1,
        rect: Rect { x0: square[0], y0: square[1], x1: square[2], y1: square[3] },
        // (the picture is the photo's own frame: nothing to compare)
        aspect: img.width as f64 / img.height.max(1) as f64,
        edge,
    };
    cut.cut_from(&img).ok_or_else(|| "the square around the face is not inside the photo".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn cut(rect: Rect, aspect: f64, min: usize) -> PreviewCut {
        PreviewCut { path: String::new(), loader: Arc::new(|_, _| None), want: 300, min, rect, aspect, edge: 32 }
    }

    fn picture(w: usize, h: usize) -> Rgba8 {
        let mut img = Rgba8::new(w, h);
        for (i, p) in img.data.iter_mut().enumerate() {
            *p = [(i % w * 255 / w) as u8, (i / w * 255 / h) as u8, 90, 255];
        }
        img
    }

    /// The square comes out the tile's size, from the place the box names.
    #[test]
    fn cuts_the_square_at_the_tiles_size() {
        let img = picture(300, 200);
        let out = cut(Rect { x0: 0.5, y0: 0.5, x1: 0.6, y1: 0.65 }, 1.5, 200).cut_from(&img).unwrap();
        assert_eq!((out.width, out.height), (32, 32));
        // red follows x: the box starts at half the width
        let red = out.data[16 * 32 + 16][0];
        assert!((125..=160).contains(&red), "{red}");
    }

    fn server_tile(edge: usize) -> ServerTile {
        ServerTile { url: String::new(), token: String::new(), edge }
    }

    /// What the server sends is checked before it is a tile: a square of a sensible size, at the tile's edge.
    #[test]
    fn a_tile_from_the_server_is_checked_and_sized() {
        let t = server_tile(32);
        assert_eq!(t.sized(picture(32, 32)).map(|i| (i.width, i.height)), Some((32, 32)), "as asked");
        assert_eq!(t.sized(picture(64, 64)).map(|i| (i.width, i.height)), Some((32, 32)), "another size is resized");
        assert_eq!(t.sized(picture(16, 16)).map(|i| (i.width, i.height)), Some((32, 32)));
        assert!(t.sized(picture(64, 32)).is_none(), "not a square");
        assert!(t.sized(picture(4, 4)).is_none(), "too small to be a face");
        assert!(t.sized(picture(1024, 1024)).is_none(), "not a tile");
        assert!(t.sized(Rgba8::new(0, 0)).is_none(), "empty");
        let short = Rgba8 { width: 32, height: 32, data: vec![[0; 4]; 10] };
        assert!(t.sized(short).is_none(), "a buffer shorter than its size");
        assert!(server_tile(32).fetch().is_none(), "no server to ask");
    }

    /// A server that can't be reached is left alone for a while, and so is the request: the tile is the photo's preview's then.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_tile_the_server_could_not_give_is_not_asked_for_again_for_a_while() {
        let asked = ServerTile { url: "http://127.0.0.1:1/api/faces/tile?hash=00&edge=32".into(), token: "t".into(), edge: 32 };
        assert!(!crate::cloud_sync::face_tile_unavailable(&asked.url));
        assert!(asked.fetch().is_none(), "nothing listens there");
        assert!(crate::cloud_sync::face_tile_unavailable(&asked.url), "this request");
        assert!(crate::cloud_sync::face_tile_unavailable("http://127.0.0.1:1/api/faces/tile?hash=11&edge=32"), "the server, which can't be reached");
        assert!(!crate::cloud_sync::face_tile_unavailable("http://127.0.0.2:1/api/faces/tile?hash=00&edge=32"), "another server");
    }

    /// A preview too small, of another shape, empty, or a box that is no box: no tile, no panic.
    #[test]
    fn declines_what_it_cannot_cut() {
        let img = picture(300, 200);
        let ok = Rect { x0: 0.4, y0: 0.4, x1: 0.5, y1: 0.55 };
        assert!(cut(ok, 1.5, 301).cut_from(&img).is_none(), "below the soft limit");
        assert!(cut(ok, 16.0 / 9.0, 200).cut_from(&img).is_none(), "another frame");
        assert!(cut(ok, f64::NAN, 200).cut_from(&img).is_none());
        assert!(cut(ok, 0.0, 200).cut_from(&img).is_none());
        assert!(cut(ok, 1.5, 0).cut_from(&Rgba8::new(0, 0)).is_none(), "empty preview");
        let short = Rgba8 { width: 300, height: 200, data: vec![[0; 4]; 10] };
        assert!(cut(ok, 1.5, 0).cut_from(&short).is_none(), "a buffer shorter than its size");
        for bad in [
            Rect { x0: f64::NAN, y0: 0.4, x1: 0.5, y1: 0.5 },
            Rect { x0: 0.5, y0: 0.5, x1: 0.5, y1: 0.5 },
            Rect { x0: 0.6, y0: 0.6, x1: 0.4, y1: 0.4 },
            Rect { x0: 2.0, y0: 2.0, x1: 3.0, y1: 3.0 },
        ] {
            assert!(cut(bad, 1.5, 0).cut_from(&img).is_none(), "{bad:?}");
        }
        // a box past the edges is clamped to the picture
        let edge = cut(Rect { x0: -1.0, y0: -1.0, x1: 0.1, y1: 0.15 }, 1.5, 0).cut_from(&img).unwrap();
        assert_eq!((edge.width, edge.height), (32, 32));
        // and a loader with nothing to give leaves the job to its source
        assert!(cut(ok, 1.5, 0).cut().is_none());
    }
}
