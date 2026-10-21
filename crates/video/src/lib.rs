//! `lightcraft-video`: video clips (MP4 / MOV) for the library.
//!
//! - [`probe`] reads a clip's header (duration, display size, frame rate, codec, creation date, GPS) from the
//!   `moov` box alone, through a [`ByteSource`], so a multi-gigabyte clip is never read whole.
//! - [`Clip`] decodes one picture: [`Clip::poster`] (a key frame near a tenth of the way in, decoded on its own,
//!   skipping black frames) and [`Clip::frame_at`] (seek to the key frame before a time, decode forward). Pictures
//!   come back as linear Rec.2020 [`Rgb32f`], the develop working space, in display orientation.
//!
//! FilmCraft's clean-room demuxer and decoders do the work ([`filmcraft_isobmff`], `filmcraft-h264`,
//! `filmcraft-hevc`); this crate adds the colour conversion, the seeking policy and the guard rails: every call runs
//! under `catch_unwind`, sizes read from the file are capped before anything is allocated, and a clip nobody can decode
//! (4:2:2, interlaced, a codec not built in) is an [`Error::Unsupported`] with a reason, never a panic.
//!
//! Not here yet: audio, playback, AV1 / VP9, the `clap` clean aperture, and exact HDR tone mapping (PQ and HLG
//! clips get a plain SDR mapping, enough for a poster).

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod decode;
#[cfg(feature = "encode")]
mod encode;
mod info;
/// Synthetic clips, for the tests of this crate and the ones above it (feature `testdata`).
#[cfg(any(test, feature = "testdata"))]
#[doc(hidden)]
pub mod testdata;
mod yuv;

pub use decode::Clip;
#[cfg(feature = "encode")]
pub use encode::{EncodeOptions, Mp4Encoder};
pub use filmcraft_isobmff::ByteSource;
#[cfg(not(target_arch = "wasm32"))]
pub use info::probe_path;
pub use info::{VideoInfo, is_video, probe};

/// Why a clip could not be read or decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Not an MP4 / MOV file, or damaged beyond reading.
    Malformed(String),
    /// A valid clip this build cannot decode (the reason says what: codec, chroma format, field coding…).
    Unsupported(String),
    /// The file could not be read.
    Io(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Malformed(m) => write!(f, "damaged video: {m}"),
            Error::Unsupported(m) => write!(f, "unsupported video: {m}"),
            Error::Io(m) => write!(f, "video: {m}"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

impl From<filmcraft_isobmff::Error> for Error {
    fn from(e: filmcraft_isobmff::Error) -> Self {
        match e {
            filmcraft_isobmff::Error::Io(e) => Error::Io(e.to_string()),
            filmcraft_isobmff::Error::Unsupported(m) => Error::Unsupported(m),
            e => Error::Malformed(e.to_string()),
        }
    }
}

/// Run `f`, turning a panic inside a third-party decoder into [`Error::Malformed`].
pub(crate) fn guarded<T>(what: &str, f: impl FnOnce() -> Result<T>) -> Result<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
        .unwrap_or_else(|_| Err(Error::Malformed(format!("the video decoder failed while {what}"))))
}

#[cfg(test)]
mod tests;
