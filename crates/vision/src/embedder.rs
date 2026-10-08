//! What the engine and the server need from an image-text model, so neither depends on a
//! particular one (and their tests can use a tiny stand-in instead of a 1.5 GB checkpoint).

use lightcraft_raster::Rgba8;

use crate::Error;

/// A model that puts photos and sentences in one space, so a sentence can be compared with a photo.
/// Vectors need not be unit length; the [`crate::EmbeddingIndex`] normalises what it stores.
pub trait Embedder: Send + Sync {
    /// Names the model's index files (at most 16 ASCII bytes).
    fn model_id(&self) -> &str;
    /// Length of every vector.
    fn dim(&self) -> usize;
    /// The vector of a sentence.
    fn encode_text(&self, text: &str) -> Result<Vec<f32>, Error>;
    /// The vectors of several photos (display-referred 8-bit RGBA, any size), in order.
    fn encode_images(&self, imgs: &[&Rgba8]) -> Result<Vec<Vec<f32>>, Error>;
}

#[cfg(not(target_arch = "wasm32"))]
impl Embedder for crate::siglip::SigLip {
    fn model_id(&self) -> &str {
        crate::siglip::MODEL_ID
    }
    fn dim(&self) -> usize {
        crate::siglip::DIM
    }
    fn encode_text(&self, text: &str) -> Result<Vec<f32>, Error> {
        crate::siglip::SigLip::encode_text(self, text)
    }
    fn encode_images(&self, imgs: &[&Rgba8]) -> Result<Vec<Vec<f32>>, Error> {
        crate::siglip::SigLip::encode_images(self, imgs)
    }
}
