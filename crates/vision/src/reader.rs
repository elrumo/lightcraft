//! What the engine and the server need from a text reader (OCR), so neither depends on a
//! particular one and their tests can use a tiny stand-in instead of the real models.

use lightcraft_raster::Rgba8;

use crate::Error;

/// Reads the text printed in a photo.
pub trait TextReader: Send + Sync {
    /// Names the engine and its version, which names the text index (at most 16 ASCII bytes): text
    /// read by another engine is not mixed with this one's.
    fn engine(&self) -> &str;
    /// The text in `img` (display-referred 8-bit RGBA, at least ~1280 px on the long side for small
    /// print), one line of text per line, in reading order. No text is `""`, not an error.
    fn text(&self, img: &Rgba8) -> Result<String, Error>;
}

#[cfg(all(feature = "onnx", not(target_arch = "wasm32")))]
impl TextReader for crate::ocr::Ocr {
    fn engine(&self) -> &str {
        crate::ocr::ENGINE
    }

    fn text(&self, img: &Rgba8) -> Result<String, Error> {
        let lines = crate::ocr::Ocr::read(self, img)?;
        Ok(lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n"))
    }
}
