//! A lighter copy of a clip on disk: every picture decoded, scaled to a smaller size and encoded again as H.264 at a target
//! bitrate, with the clip's sound copied as it is. What a sync server does for a library's videos when no Mac does it
//! for it (docs/video.md, *Lighter copies*); the pictures come back in display orientation, so the copy carries no turn.

use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use lightcraft_raster::Rgba8;
use lightcraft_raster::resample::{Filter, resize};

use crate::{Clip, EncodeOptions, Error, Mp4Encoder, Tuning};

/// What the copy is made as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    /// The size of every picture (even, in display orientation: the clip's size after its turn).
    pub width: usize,
    pub height: usize,
    /// The bitrate to aim at (kbit/s).
    pub kbps: u32,
    /// Threads the encoder may use (1: on the calling thread; 0: every core).
    pub threads: usize,
}

/// What a finished copy is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lightened {
    pub frames: u64,
    pub width: usize,
    pub height: usize,
    pub bytes: u64,
    /// The clip's sound was copied into it.
    pub audio: bool,
}

/// Why a copy was not made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LighterError {
    /// `cancel` was set.
    Cancelled,
    /// The clip could not be read or decoded.
    Source(Error),
    /// The copy could not be written or encoded.
    Encoder(Error),
}

impl std::fmt::Display for LighterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LighterError::Cancelled => f.write_str("cancelled"),
            LighterError::Source(e) | LighterError::Encoder(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for LighterError {}

/// Write `src` as a lighter copy at `dst` (made even if it is there; the caller writes a `.part` file and renames it).
/// `progress(done, total)` is called after each picture; the copy stops with [`LighterError::Cancelled`] soon after `cancel`
/// is set. HDR clips are refused: the pictures would be squeezed into SDR and the copy would not look like the clip.
pub fn lighter_copy(src: &Path, dst: &Path, plan: Plan, cancel: &AtomicBool, progress: &dyn Fn(u64, u64)) -> Result<Lightened, LighterError> {
    let clip = Clip::open_path(src).map_err(LighterError::Source)?;
    let info = clip.info().clone();
    if info.hdr {
        return Err(LighterError::Source(Error::Unsupported("an HDR clip (the copy would be squeezed into SDR)".into())));
    }
    if plan.width == 0 || plan.height == 0 || !plan.width.is_multiple_of(2) || !plan.height.is_multiple_of(2) {
        return Err(LighterError::Encoder(Error::Unsupported(format!("a {}x{} picture can't be encoded (even sizes)", plan.width, plan.height))));
    }
    let sound = clip.audio_track();
    let fps = if info.fps > 0.0 {
        info.fps
    } else if info.duration > 0.0 {
        info.frames as f64 / info.duration
    } else {
        30.0
    };
    let file = std::fs::File::create(dst).map_err(|e| LighterError::Encoder(Error::Io(format!("{}: {e}", dst.display()))))?;
    let mut file = Some(std::io::BufWriter::new(file));
    let mut encoder: Option<Mp4Encoder<std::io::BufWriter<std::fs::File>>> = None;
    let (mut done, mut cancelled, mut broke) = (0u64, false, false);
    let total = info.frames as u64;
    let to_srgb = lightcraft_color::WORKING.to_space(&lightcraft_color::SRGB).to_f32();
    let long_edge = plan.width.max(plan.height);
    let r = clip.for_each_frame(0.0, f64::INFINITY, long_edge, |seconds, picture| {
        if cancel.load(Ordering::Relaxed) {
            cancelled = true;
            return Err(Error::Io("cancelled".into()));
        }
        let picture = resize(&picture, plan.width, plan.height, Filter::Bilinear);
        let frame = Rgba8::from_fn(picture.width, picture.height, |x, y| {
            let px = picture.get(x, y);
            let q = [0, 1, 2].map(|r| to_srgb[r][0] * px[0] + to_srgb[r][1] * px[1] + to_srgb[r][2] * px[2]);
            let enc = lightcraft_color::transfer::encode_srgb8;
            [enc(q[0]), enc(q[1]), enc(q[2]), 255]
        });
        if encoder.is_none() {
            let options = EncodeOptions { width: plan.width, height: plan.height, fps, crf: 23.0, bframes: 2 };
            let tuning = Tuning { kbps: Some(plan.kbps), threads: plan.threads };
            let out = file.take().ok_or_else(|| Error::Io("the file is gone".into()));
            match out.and_then(|out| Mp4Encoder::new_tuned(out, options, tuning, sound.as_ref())) {
                Ok(e) => encoder = Some(e),
                Err(e) => {
                    broke = true;
                    return Err(e);
                }
            }
        }
        if let Some(e) = &mut encoder
            && let Err(e) = e.push(&frame, seconds.max(0.0))
        {
            broke = true;
            return Err(e);
        }
        done += 1;
        progress(done, total.max(done));
        Ok(true)
    });
    if cancelled {
        return Err(LighterError::Cancelled);
    }
    if let Err(e) = r {
        return Err(if broke { LighterError::Encoder(e) } else { LighterError::Source(e) });
    }
    let mut encoder = encoder.ok_or_else(|| LighterError::Source(Error::Malformed("the clip has no pictures".into())))?;
    let copied = match &sound {
        Some(track) => encoder.copy_audio(&clip, track, 0.0, f64::INFINITY).map_err(LighterError::Encoder)?,
        None => 0,
    };
    let mut out = encoder.finish().map_err(LighterError::Encoder)?;
    out.flush().map_err(|e| LighterError::Encoder(Error::Io(e.to_string())))?;
    let file = out.into_inner().map_err(|e| LighterError::Encoder(Error::Io(e.to_string())))?;
    file.sync_all().map_err(|e| LighterError::Encoder(Error::Io(e.to_string())))?;
    let bytes = file.metadata().map(|m| m.len()).unwrap_or(0);
    Ok(Lightened { frames: done, width: plan.width, height: plan.height, bytes, audio: copied > 0 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdata;

    fn temp(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("lc-video-lighter-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A clip comes back smaller on screen, with the same pictures in the same time, its sound and an H.264 header any player reads.
    #[test]
    fn a_clip_is_written_again_smaller_with_its_sound() {
        let dir = temp("copy");
        let (src, dst) = (dir.join("in.mp4"), dir.join("out.mp4"));
        std::fs::write(&src, testdata::clip_with_sound(20, 10, 2)).unwrap();
        let plan = Plan { width: 64, height: 48, kbps: 300, threads: 1 };
        let seen = std::sync::Mutex::new(Vec::new());
        let r = lighter_copy(&src, &dst, plan, &AtomicBool::new(false), &|done, total| seen.lock().unwrap().push((done, total))).unwrap();
        assert_eq!((r.frames, r.width, r.height, r.audio), (20, 64, 48, true));
        assert_eq!(r.bytes, std::fs::metadata(&dst).unwrap().len());
        assert_eq!(seen.lock().unwrap().last(), Some(&(20, 20)));
        let info = crate::probe_path(&dst).unwrap();
        assert_eq!((info.width, info.height, info.codec, info.frames, info.has_audio), (64, 48, "h264", 20, true));
        assert!((info.duration - 2.0).abs() < 0.15, "{}", info.duration);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A cancel stops the copy; a clip that can't be read, or an odd size, is said so without a panic.
    #[test]
    fn a_cancelled_or_unreadable_copy_says_so() {
        let dir = temp("errors");
        let (src, dst) = (dir.join("in.mp4"), dir.join("out.mp4"));
        let clip = testdata::clip(20, 10, 0);
        std::fs::write(&src, &clip).unwrap();
        let plan = Plan { width: 64, height: 48, kbps: 300, threads: 1 };
        assert_eq!(lighter_copy(&src, &dst, plan, &AtomicBool::new(true), &|_, _| {}), Err(LighterError::Cancelled));
        let odd = Plan { width: 63, ..plan };
        assert!(matches!(lighter_copy(&src, &dst, odd, &AtomicBool::new(false), &|_, _| {}), Err(LighterError::Encoder(_))));
        std::fs::write(&src, &clip[..clip.len() / 3]).unwrap();
        assert!(matches!(lighter_copy(&src, &dst, plan, &AtomicBool::new(false), &|_, _| {}), Err(LighterError::Source(_))));
        assert!(matches!(lighter_copy(&dir.join("none.mp4"), &dst, plan, &AtomicBool::new(false), &|_, _| {}), Err(LighterError::Source(_))));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
