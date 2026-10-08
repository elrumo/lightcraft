//! JPEG XL pixel data of DNG 1.7 (`Compression` 52546), read with `jxl-oxide` (pure Rust, MIT / Apache-2.0).
//!
//! Every strip or tile is a standalone JPEG XL image: a bare codestream (Adobe DNG Converter, Panasonic) or one
//! wrapped in an ISO BMFF container (Apple ProRAW); the decoder reads both. A chunk carries `SamplesPerPixel`
//! channels (one for a CFA tile, three for LinearRaw). The colour encoding in the stream is not interpreted: the
//! decoder renders in the stream's own encoding, so the samples come back as the writer gave them, scaled to 0…1.
//!
//! Layout rules come from the public DNG 1.7 specification and from how other open decoders describe the files;
//! they are checked here against streams written by libjxl (`cjxl`), not yet against a camera file. Lossy streams
//! (Adobe's "lossy" setting) rely on an `OpcodeList2` `MapPolynomial`, which [`crate::opcodes`] applies.

use crate::{RawError, Result};
use jxl_oxide::{AllocTracker, FrameBuffer, JxlImage};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, PoisonError};

/// How many tiles may be decoded at once, and how many are.
static LIMIT: AtomicUsize = AtomicUsize::new(usize::MAX);
static RUNNING: (Mutex<usize>, Condvar) = (Mutex::new(0), Condvar::new());

/// Cap the JPEG XL tiles decoded at the same time. Each takes tens of MB of working memory whatever the image's
/// size (about 23 MB for a 1008 × 1008 RGB tile), so on a device with a memory limit the cap — not the image — sets
/// the decode's peak. Default: no cap beyond the thread count.
pub fn set_max_parallel_tiles(n: usize) {
    LIMIT.store(n.max(1), Ordering::Relaxed);
    RUNNING.1.notify_all();
}

/// A decoding slot, free again on drop.
struct Slot;

impl Slot {
    fn take() -> Slot {
        let mut running = RUNNING.0.lock().unwrap_or_else(PoisonError::into_inner);
        while *running >= LIMIT.load(Ordering::Relaxed) {
            running = RUNNING.1.wait(running).unwrap_or_else(PoisonError::into_inner);
        }
        *running += 1;
        Slot
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        *RUNNING.0.lock().unwrap_or_else(PoisonError::into_inner) -= 1;
        RUNNING.1.notify_one();
    }
}

fn err(e: impl std::fmt::Display) -> RawError {
    RawError::Corrupt(format!("JPEG XL tile: {e}"))
}

/// What a tile's header says, read without rendering it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Header {
    pub width: usize,
    pub height: usize,
    /// Colour channels plus extra channels.
    pub channels: usize,
    /// Declared bits per sample of the colour channels.
    pub bits: u32,
}

/// A decoded tile: interleaved samples scaled to 0…1 (floating-point streams: as stored).
pub(crate) struct Tile {
    pub header: Header,
    samples: FrameBuffer,
}

fn open(src: &[u8], samples: usize) -> Result<JxlImage> {
    // The tile's own size bounds what its header may ask for: every buffer the decoder makes goes through this.
    let tracker = AllocTracker::with_limit(samples.saturating_mul(32).saturating_add(64 << 20));
    let img = JxlImage::builder().alloc_tracker(tracker).read(std::io::Cursor::new(src)).map_err(err)?;
    if img.num_loaded_keyframes() == 0 {
        return Err(err("truncated"));
    }
    Ok(img)
}

fn header_of(img: &JxlImage) -> Header {
    let colour = if img.pixel_format().is_grayscale() { 1 } else { 3 };
    Header {
        width: img.width() as usize,
        height: img.height() as usize,
        channels: colour + img.image_header().metadata.ec_info.len(),
        bits: img.image_header().metadata.bit_depth.bits_per_sample(),
    }
}

/// The header of a tile; `samples` is the number of samples its chunk holds (it bounds the memory used).
pub(crate) fn header(src: &[u8], samples: usize) -> Result<Header> {
    open(src, samples).map(|img| header_of(&img))
}

/// Decode a tile of at most `cw × ch` pixels with exactly `cpp` channels. A tile cut short by the image edge
/// may be smaller than its chunk; [`Tile::placed`] pads it.
pub(crate) fn decode(src: &[u8], cw: usize, ch: usize, cpp: usize) -> Result<Tile> {
    let _slot = Slot::take();
    let img = open(src, cw.saturating_mul(ch).saturating_mul(cpp))?;
    let header = header_of(&img);
    if header.channels != cpp {
        return Err(err(format!("{} channels, expected {cpp}", header.channels)));
    }
    if header.width > cw || header.height > ch {
        return Err(err(format!("{}×{} pixels in a {cw}×{ch} chunk", header.width, header.height)));
    }
    let fb = img.render_frame(0).map_err(err)?.image_all_channels();
    drop(img);
    if fb.channels() != cpp {
        return Err(err("channel count changed while rendering"));
    }
    let header = Header { width: fb.width(), height: fb.height(), ..header };
    Ok(Tile { header, samples: fb })
}

impl Tile {
    /// The samples converted by `f` into a `cw × ch × cpp` buffer, zero-padded where the tile is smaller.
    pub fn placed<T: Copy + Default>(&self, cw: usize, ch: usize, cpp: usize, f: impl Fn(f32) -> T) -> Vec<T> {
        let mut out = vec![T::default(); cw.saturating_mul(ch).saturating_mul(cpp)];
        let row = (self.header.width * cpp).max(1);
        for (dst, src) in out.chunks_mut((cw * cpp).max(1)).zip(self.samples.buf().chunks_exact(row)) {
            for (d, &s) in dst.iter_mut().zip(src) {
                *d = f(s);
            }
        }
        out
    }

    /// The factor that turns the tile's 0…1 samples into the DNG's integer samples.
    ///
    /// A stream of `b` bits holds raw values `0..=2^b-1`, so that is the factor — unless the DNG's `WhiteLevel`
    /// lies beyond it (Apple declares 10 bits but a white level of 65535): then the writer meant the full 16-bit
    /// range, which is also what libjxl's own 16-bit output does.
    pub fn int_scale(&self, white: Option<f32>) -> f32 {
        let max = ((1u64 << self.header.bits.clamp(1, 16)) - 1) as f32;
        match white {
            Some(w) if w > max => 65535.0,
            _ => max,
        }
    }
}
