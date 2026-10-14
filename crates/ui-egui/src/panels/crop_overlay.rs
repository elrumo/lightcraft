//! Crop guide overlays: the geometry is [`lightcraft_geom::guides`], shared with every UI; this
//! maps the app's setting onto it.

use crate::state::CropOverlay;
use lightcraft_geom::guides::Guide;

/// Guide lines for `kind` on a `w × h` crop (see [`lightcraft_geom::guides::paths`]).
pub fn paths(kind: CropOverlay, orient: u8, w: f32, h: f32) -> Vec<Vec<(f32, f32)>> {
    let guide = match kind {
        CropOverlay::Thirds => Guide::Thirds,
        CropOverlay::Grid => Guide::Grid,
        CropOverlay::Golden => Guide::Golden,
        CropOverlay::Diagonal => Guide::Diagonal,
        CropOverlay::Triangle => Guide::Triangle,
        CropOverlay::Spiral => Guide::Spiral,
        CropOverlay::None => Guide::None,
    };
    lightcraft_geom::guides::paths(guide, orient, w, h)
}
