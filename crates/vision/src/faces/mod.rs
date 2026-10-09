//! Faces: where they are in a photo and who they look like. YuNet finds them (and five landmarks
//! each), SFace turns an aligned crop of each into a 128-number vector that is close to the
//! vector of the same person's other faces ([`model`], pure Rust on `rten`); faces are kept per
//! photo in a small derived file ([`index`]) and grouped into people by how close their vectors
//! are ([`cluster`]).
//!
//! Opt-in, local and deletable: nothing here runs until the user turns it on, vectors stay in the
//! library's `search/` folder (or the server's, when the user shares them), they are never part
//! of the catalog or any sidecar, and deleting the file forgets them. Naming a person is a
//! separate, deliberate step that writes ordinary face regions into the catalog.

pub mod cluster;
pub mod index;
#[cfg(all(feature = "onnx", not(target_arch = "wasm32")))]
pub mod model;

use lightcraft_raster::Rgba8;

use crate::Error;

/// Names the face index these models fill (a different pair would see differently).
pub const ENGINE: &str = "yunet-sface";
/// Length of a face's vector.
pub const DIM: usize = 128;
/// The detector's file in the model folder.
pub const DETECTOR_FILE: &str = "face_detection_yunet_2023mar.onnx";
/// The recogniser's file in the model folder.
pub const RECOGNISER_FILE: &str = "face_recognition_sface_2021dec.onnx";
/// Most faces kept from one photo.
pub const MAX_FACES: usize = 64;

/// A face found in a photo: where, how sure the detector was, and what it looks like.
#[derive(Clone, Debug, PartialEq)]
pub struct FaceFound {
    /// `[x0, y0, x1, y1]`, normalized to the photo as rendered (upright), y down.
    pub rect: [f32; 4],
    /// The detector's confidence, 0 to 1.
    pub score: f32,
    /// [`DIM`] numbers of unit length: faces of one person point the same way.
    pub embedding: Vec<f32>,
}

/// What the engine and the server need from a face finder, so neither depends on particular
/// models (and tests can use a tiny stand-in).
pub trait FaceEngine: Send + Sync {
    /// Names the face index (at most 16 ASCII bytes).
    fn engine(&self) -> &str;
    /// The faces in `img` (display-referred 8-bit RGBA, at least ~1280 px on the long side for
    /// faces in a group), best first. No faces is an empty list, not an error.
    fn faces(&self, img: &Rgba8) -> Result<Vec<FaceFound>, Error>;
}
