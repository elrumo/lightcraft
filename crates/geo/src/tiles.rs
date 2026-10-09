//! Slippy-map tiles (256 px, Web Mercator, `z/x/y`) and the viewport that decides which are shown.

use crate::geodesy::{MAX_MERCATOR_LAT, World, project, unproject, valid, zoom_to_fit};

/// Deepest level the map zooms to (street level; every tile server serves at least this).
pub const MAX_ZOOM: u8 = 19;
/// Tile edge in pixels.
pub const TILE_PX: f64 = 256.0;
/// Largest view edge in pixels (a view is a window, not a poster); also bounds the tiles listed.
pub const MAX_VIEW_PX: f64 = 16384.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TileId {
    pub z: u8,
    pub x: u32,
    pub y: u32,
}

impl TileId {
    /// The tile's request URL: `{z}`, `{x}` and `{y}` in `template` replaced.
    pub fn url(self, template: &str) -> String {
        template.replace("{z}", &self.z.to_string()).replace("{x}", &self.x.to_string()).replace("{y}", &self.y.to_string())
    }

    /// Top-left corner on the world square, and the tile's edge there.
    pub fn world(self) -> (World, f64) {
        let n = f64::from(1u32 << self.z.min(MAX_ZOOM));
        (World { x: f64::from(self.x) / n, y: f64::from(self.y) / n }, 1.0 / n)
    }
}

/// Whether `template` is a usable tile URL: web address with `{z}`, `{x}` and `{y}`.
pub fn valid_template(template: &str) -> bool {
    let t = template.trim();
    (t.starts_with("https://") || t.starts_with("http://"))
        && ["{z}", "{x}", "{y}"].iter().all(|k| t.contains(k))
        && t.len() <= 512
        && !t.contains(char::is_whitespace)
}

/// A view edge in pixels, made sane (NaN counts as the least).
fn view_px(v: f64) -> f64 {
    if v.is_nan() { 1.0 } else { v.clamp(1.0, MAX_VIEW_PX) }
}

/// What part of the world is on screen: the world point at the centre of a `width` × `height`
/// pixel view, and the (fractional) zoom: at zoom `z` the world square is `256 · 2^z` pixels wide.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub center: World,
    pub zoom: f64,
    pub width: f64,
    pub height: f64,
}

impl Viewport {
    pub fn new(center: World, zoom: f64, width: f64, height: f64) -> Viewport {
        let mut v = Viewport { center, zoom, width: view_px(width), height: view_px(height) };
        v.clamp();
        v
    }

    /// The whole world, centred.
    pub fn world(width: f64, height: f64) -> Viewport {
        Viewport::new(World { x: 0.5, y: 0.5 }, 0.0, width, height)
    }

    /// The least zoom that still fills the view with map (no grey margins around the world).
    pub fn min_zoom(&self) -> f64 {
        if self.width.is_finite() && self.height.is_finite() { (self.width.max(self.height) / TILE_PX).log2().max(0.0) } else { 0.0 }
    }

    pub fn pixels_per_world(&self) -> f64 {
        TILE_PX * self.zoom.exp2()
    }

    /// Keep the zoom in range and the view over the world.
    pub fn clamp(&mut self) {
        if !self.zoom.is_finite() {
            self.zoom = 0.0;
        }
        // not `clamp`: that panics when the least zoom (a huge view) is above the deepest
        self.zoom = self.zoom.max(self.min_zoom()).min(f64::from(MAX_ZOOM));
        let s = self.pixels_per_world();
        let (hw, hh) = (self.width / 2.0 / s, self.height / 2.0 / s);
        let fit = |c: f64, half: f64| {
            if half >= 0.5 {
                0.5
            } else if c.is_finite() {
                c.clamp(half, 1.0 - half)
            } else {
                0.5
            }
        };
        self.center = World { x: fit(self.center.x, hw), y: fit(self.center.y, hh) };
    }

    /// Pixel position (from the view's top-left) of a world point.
    pub fn to_screen(&self, w: World) -> (f64, f64) {
        let s = self.pixels_per_world();
        (self.width / 2.0 + (w.x - self.center.x) * s, self.height / 2.0 + (w.y - self.center.y) * s)
    }

    pub fn to_world(&self, x: f64, y: f64) -> World {
        let s = self.pixels_per_world();
        World { x: self.center.x + (x - self.width / 2.0) / s, y: self.center.y + (y - self.height / 2.0) / s }
    }

    /// Pixel position of a position on the globe.
    pub fn locate(&self, lat: f64, lon: f64) -> (f64, f64) {
        self.to_screen(project(lat, lon))
    }

    /// Drag the map by `(dx, dy)` pixels.
    pub fn pan(&mut self, dx: f64, dy: f64) {
        let s = self.pixels_per_world();
        self.center = World { x: self.center.x - dx / s, y: self.center.y - dy / s };
        self.clamp();
    }

    /// Zoom by `dz` levels keeping the world point under pixel `(ax, ay)` where it is (wheel and
    /// pinch zoom toward the cursor).
    pub fn zoom_about(&mut self, ax: f64, ay: f64, dz: f64) {
        let before = self.to_world(ax, ay);
        self.zoom += dz;
        self.clamp();
        let s = self.pixels_per_world();
        self.center = World { x: before.x - (ax - self.width / 2.0) / s, y: before.y - (ay - self.height / 2.0) / s };
        self.clamp();
    }

    /// Centre on a position, optionally zooming.
    pub fn look_at(&mut self, lat: f64, lon: f64, zoom: Option<f64>) {
        if valid(lat, lon) {
            self.center = project(lat, lon);
            if let Some(z) = zoom {
                self.zoom = z;
            }
            self.clamp();
        }
    }

    /// Show the box `min`..`max` (`(lat, lon)` corners) with `margin` pixels around it. A single
    /// point (or a tiny box) is shown at `at_most` zoom.
    pub fn fit(&mut self, min: (f64, f64), max: (f64, f64), margin: f64, at_most: f64) {
        if !(valid(min.0, min.1) && valid(max.0, max.1)) {
            return;
        }
        let z = zoom_to_fit(min, max, (self.width - 2.0 * margin).max(1.0), (self.height - 2.0 * margin).max(1.0)).min(at_most);
        let (a, b) = (project(min.0, min.1), project(max.0, max.1));
        self.center = World { x: (a.x + b.x) / 2.0, y: (a.y + b.y) / 2.0 };
        self.zoom = z;
        self.clamp();
    }

    /// The tile level that draws at about its natural size: the nearest whole zoom, never beyond
    /// [`MAX_ZOOM`].
    pub fn tile_level(&self) -> u8 {
        self.zoom.round().clamp(0.0, f64::from(MAX_ZOOM)) as u8
    }

    /// The tiles of `level` that touch the view (a few dozen at most).
    pub fn visible_tiles(&self, level: u8) -> Vec<TileId> {
        let level = level.min(MAX_ZOOM);
        let n = 1u32 << level;
        let nf = f64::from(n);
        let (tl, br) = (self.to_world(0.0, 0.0), self.to_world(self.width, self.height));
        let range = |a: f64, b: f64| {
            let lo = (a * nf).floor().clamp(0.0, nf - 1.0) as u32;
            let hi = (b * nf).floor().clamp(0.0, nf - 1.0) as u32;
            lo..=hi
        };
        let (xs, ys) = (range(tl.x, br.x), range(tl.y, br.y));
        // a hand-built `Viewport` can be any size: never list a poster's worth of tiles
        if (u64::from(*xs.end() - *xs.start()) + 1) * (u64::from(*ys.end() - *ys.start()) + 1) > 4096 {
            return vec![];
        }
        let mut v = Vec::new();
        for y in ys {
            for x in xs.clone() {
                v.push(TileId { z: level, x, y });
            }
        }
        v
    }

    /// The latitude / longitude box on screen: `((south, west), (north, east))`.
    pub fn bounds(&self) -> ((f64, f64), (f64, f64)) {
        let (nw, se) = (unproject(self.to_world(0.0, 0.0)), unproject(self.to_world(self.width, self.height)));
        ((se.0.max(-MAX_MERCATOR_LAT), nw.1), (nw.0.min(MAX_MERCATOR_LAT), se.1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        let t = TileId { z: 5, x: 16, y: 10 };
        assert_eq!(t.url("https://tile.example.org/{z}/{x}/{y}.png"), "https://tile.example.org/5/16/10.png");
        assert_eq!(t.url("https://t.example/{x}/{y}/{z}?z={z}"), "https://t.example/16/10/5?z=5");
        let (o, size) = t.world();
        assert_eq!((o.x, o.y, size), (0.5, 10.0 / 32.0, 1.0 / 32.0));
    }

    #[test]
    fn template_validation() {
        assert!(valid_template("https://tile.openstreetmap.org/{z}/{x}/{y}.png"));
        assert!(valid_template("http://localhost:8080/{z}/{x}/{y}.png"));
        assert!(!valid_template("https://tile.openstreetmap.org/{z}/{x}.png"));
        assert!(!valid_template("ftp://x/{z}/{x}/{y}"));
        assert!(!valid_template("file:///etc/{z}/{x}/{y}"));
        assert!(!valid_template("https://x y/{z}/{x}/{y}"));
        assert!(!valid_template(""));
    }

    #[test]
    fn the_world_fits_without_margins() {
        let v = Viewport::world(1024.0, 768.0);
        assert!((v.zoom - 2.0).abs() < 1e-9, "{}", v.zoom);
        let mut v = Viewport::new(World { x: 0.3, y: 0.3 }, 0.0, 512.0, 512.0);
        v.zoom_about(10.0, 10.0, -5.0);
        assert!(v.zoom >= 1.0 - 1e-9, "512 px needs zoom 1 to be all map: {}", v.zoom);
    }

    #[test]
    fn screen_and_world_agree() {
        let v = Viewport::new(project(40.4, -3.7), 10.0, 800.0, 600.0);
        let (x, y) = v.locate(40.4, -3.7);
        assert!((x - 400.0).abs() < 1e-6 && (y - 300.0).abs() < 1e-6);
        let w = v.to_world(123.0, 456.0);
        let (sx, sy) = v.to_screen(w);
        assert!((sx - 123.0).abs() < 1e-6 && (sy - 456.0).abs() < 1e-6);
    }

    #[test]
    fn zooming_keeps_the_point_under_the_cursor() {
        let mut v = Viewport::new(project(40.4, -3.7), 8.0, 800.0, 600.0);
        let w = v.to_world(200.0, 150.0);
        v.zoom_about(200.0, 150.0, 2.0);
        let (x, y) = v.to_screen(w);
        assert!((x - 200.0).abs() < 1e-6 && (y - 150.0).abs() < 1e-6, "{x},{y}");
        assert!((v.zoom - 10.0).abs() < 1e-9);
    }

    #[test]
    fn panning_moves_with_the_finger() {
        let mut v = Viewport::new(project(40.4, -3.7), 8.0, 800.0, 600.0);
        let w = v.to_world(400.0, 300.0);
        v.pan(100.0, -50.0);
        let (x, y) = v.to_screen(w);
        assert!((x - 500.0).abs() < 1e-6 && (y - 250.0).abs() < 1e-6, "{x},{y}");
        // can't drag the world away from the view
        v.pan(1e9, 1e9);
        assert!(v.center.x.is_finite() && v.center.y.is_finite());
        let (nw, _) = (v.to_world(0.0, 0.0), 0);
        assert!(nw.x >= -1e-9 && nw.y >= -1e-9);
    }

    #[test]
    fn visible_tiles_cover_the_view() {
        let v = Viewport::new(project(40.4, -3.7), 10.0, 800.0, 600.0);
        let tiles = v.visible_tiles(v.tile_level());
        assert!(!tiles.is_empty() && tiles.len() <= 20, "{}", tiles.len());
        assert!(tiles.iter().all(|t| t.z == 10 && t.x < 1024 && t.y < 1024));
        // Madrid at z10 is x=501 y=387
        assert!(tiles.contains(&TileId { z: 10, x: 501, y: 387 }), "{tiles:?}");
        // zoom 0: one tile
        assert_eq!(Viewport::world(256.0, 256.0).visible_tiles(0), vec![TileId { z: 0, x: 0, y: 0 }]);
    }

    #[test]
    fn fitting_a_box() {
        let mut v = Viewport::world(1000.0, 800.0);
        v.fit((36.0, -9.5), (43.8, 3.3), 40.0, 16.0); // Spain
        let ((s, w), (n, e)) = v.bounds();
        assert!(s <= 36.0 && n >= 43.8 && w <= -9.5 && e >= 3.3, "{s} {w} {n} {e}");
        assert!(v.zoom > 4.0 && v.zoom < 7.0, "{}", v.zoom);
        let mut p = Viewport::world(1000.0, 800.0);
        p.fit((40.4, -3.7), (40.4, -3.7), 40.0, 15.0);
        assert!((p.zoom - 15.0).abs() < 1e-9, "a single point stops at the maximum: {}", p.zoom);
        let before = p;
        p.fit((f64::NAN, 0.0), (1.0, 1.0), 0.0, 5.0);
        assert_eq!(p, before, "garbage leaves the view alone");
    }

    #[test]
    fn hostile_viewports() {
        for (w, h, z) in [(0.0, 0.0, 0.0), (f64::NAN, 5.0, 1.0), (1e9, 1e9, 99.0), (-5.0, -5.0, -3.0)] {
            let mut v = Viewport::new(World { x: f64::NAN, y: f64::INFINITY }, z, w, h);
            let hand_built = Viewport { center: World { x: 0.5, y: 0.5 }, zoom: 19.0, width: 1e12, height: 1e12 };
            assert!(hand_built.visible_tiles(19).len() <= 4096);
            v.pan(f64::NAN, 1.0);
            v.zoom_about(f64::NAN, f64::NAN, f64::INFINITY);
            let _ = v.visible_tiles(255);
            let _ = v.bounds();
            assert!(v.center.x.is_finite() && v.center.y.is_finite() && v.zoom.is_finite(), "{v:?}");
        }
    }
}
