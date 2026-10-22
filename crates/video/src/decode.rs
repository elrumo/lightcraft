//! Opening a clip and decoding single pictures from it.

use filmcraft_isobmff::{ByteSource, CodecConfig, ColorInfo, Mp4File, Track};
use lightcraft_raster::Rgb32f;

use crate::info::{VideoInfo, info_of, video_track};
use crate::yuv::{self, Color, Frame};
use crate::{Error, Result, guarded, timed};

/// The largest compressed picture read from a file.
const MAX_SAMPLE: u32 = 256 << 20;
/// The most pictures decoded to reach one: key frames further apart than this are not worth a seek.
const MAX_FEED: usize = 2000;
/// A poster frame darker than this (mean luma, black 0 to white 1) is a fade-in or a black leader; try another.
const DARK: f32 = 0.04;

/// An open clip: its header, and the source its pictures are read from on demand.
pub struct Clip<S> {
    src: S,
    mp4: Mp4File,
    track: usize,
    info: VideoInfo,
    color: Color,
}

impl<S: ByteSource> Clip<S> {
    /// Read the header from `src`; pictures are fetched only when asked for.
    pub fn open(src: S) -> Result<Clip<S>> {
        let mp4 = guarded("reading the header", || Ok(filmcraft_isobmff::open(&src)?))?;
        let info = info_of(&mp4)?;
        let (track, t) = video_track(&mp4).ok_or_else(|| Error::Malformed("no video track".into()))?;
        let color = match t.video().and_then(|v| v.color.as_ref()) {
            Some(ColorInfo::Nclx { primaries, transfer, matrix, full_range }) => {
                Color { full_range: *full_range, primaries: *primaries, transfer: *transfer, matrix: *matrix }
            }
            Some(ColorInfo::Nclc { primaries, transfer, matrix }) => {
                Color { full_range: false, primaries: *primaries, transfer: *transfer, matrix: *matrix }
            }
            _ => Color::UNSPECIFIED,
        };
        Ok(Clip { src, mp4, track, info, color })
    }

    pub fn info(&self) -> &VideoInfo {
        &self.info
    }

    pub(crate) fn mp4(&self) -> &Mp4File {
        &self.mp4
    }

    pub(crate) fn source(&self) -> &S {
        &self.src
    }

    fn t(&self) -> Result<&Track> {
        self.mp4.tracks.get(self.track).ok_or_else(|| Error::Malformed("no video track".into()))
    }

    /// A good picture to stand for the clip: a key frame about a tenth of the way in (skipping black ones), decoded on
    /// its own, so a poster costs one picture however long the clip is. At most about twice `max_edge` on the long side.
    pub fn poster(&self, max_edge: usize) -> Result<Rgb32f> {
        self.supported()?;
        guarded("making a poster", || {
            let t = self.t()?;
            let ticks = t.duration;
            let mut tried: Vec<usize> = Vec::new();
            let mut best: Option<(f32, Frame)> = None;
            for fraction in [0.1, 0.25, 0.5, 0.0] {
                let at = (ticks as f64 * fraction) as i64;
                let Some(i) = t.sample_at_pts(at) else { continue };
                let key = t.sync_sample_before(i);
                if tried.contains(&key) {
                    continue;
                }
                tried.push(key);
                let frame = timed("poster: decode", || self.decode(key, key, None))?;
                let frame = Frame { color: frame.color.or(self.color), ..frame };
                let luma = timed("poster: mean luma", || frame.mean_luma());
                let bright = luma >= DARK;
                if best.as_ref().is_none_or(|(l, _)| luma > *l) {
                    best = Some((luma, frame));
                }
                if bright {
                    break;
                }
            }
            let (_, frame) = best.ok_or_else(|| Error::Malformed("the clip has no pictures".into()))?;
            timed("poster: to_working", || yuv::to_working_within(&frame, max_edge, self.info.rotation))
        })
    }

    /// The picture shown `seconds` into the clip (clamped to its length): the key frame before it is decoded, then every
    /// picture up to it. At most about twice `max_edge` on the long side.
    pub fn frame_at(&self, seconds: f64, max_edge: usize) -> Result<Rgb32f> {
        self.supported()?;
        guarded("decoding a picture", || {
            let t = self.t()?;
            let secs = if seconds.is_finite() { seconds.clamp(0.0, self.info.duration.max(0.0)) } else { 0.0 };
            let ticks = (secs * f64::from(t.timescale)).round() as i64;
            let i = t.sample_at_presentation_time(ticks).ok_or_else(|| Error::Malformed("the clip has no pictures".into()))?;
            let want = t.samples.get(i).map(|s| s.pts).ok_or_else(|| Error::Malformed("the clip has no pictures".into()))?;
            let frame = self.decode(t.sync_sample_before(i), i, Some(want))?;
            yuv::to_working(&Frame { color: frame.color.or(self.color), ..frame }, max_edge, self.info.rotation)
        })
    }

    /// Every picture shown from `start` to `end` seconds, in the order they are shown: `f(seconds, picture)` gets each (linear
    /// Rec.2020, at most about twice `max_edge` on the long side) and answers whether to go on. The key frame before `start` is
    /// decoded once and the clip is read forward from it, so a long stretch costs one pass, not a seek a picture.
    pub fn for_each_frame(&self, start: f64, end: f64, max_edge: usize, mut f: impl FnMut(f64, Rgb32f) -> Result<bool>) -> Result<()> {
        self.supported()?;
        guarded("decoding pictures", || {
            let t = self.t()?;
            let scale = f64::from(t.timescale);
            let first_ticks = (start.max(0.0) * scale).round() as i64;
            let first = t.sample_at_presentation_time(first_ticks).ok_or_else(|| Error::Malformed("the clip has no pictures".into()))?;
            let entry = t.codec().ok_or_else(|| Error::Malformed("the video track has no codec record".into()))?;
            let mut dec = Decoder::new(entry)?;
            // (a picture's time is half a frame of slack from the start asked for: 1/1000 s)
            let from = start - 0.001;
            let mut emit = |frames: Vec<Frame>| -> Result<bool> {
                for frame in frames {
                    let seconds = (frame.pts.saturating_add(t.edit_offset)) as f64 / scale;
                    if seconds < from {
                        continue;
                    }
                    if seconds >= end {
                        return Ok(false);
                    }
                    let frame = Frame { color: frame.color.or(self.color), ..frame };
                    if !f(seconds, yuv::to_working(&frame, max_edge, self.info.rotation)?)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            };
            for i in t.sync_sample_before(first)..t.samples.len() {
                let s = t.samples.get(i).ok_or_else(|| Error::Malformed("a sample is missing".into()))?;
                if s.size > MAX_SAMPLE {
                    return Err(Error::Malformed("a picture claims to be larger than 256 MiB".into()));
                }
                let data = self.mp4.read_sample(&self.src, self.track, i)?;
                if !emit(dec.feed(&data, s.pts)?)? {
                    return Ok(());
                }
            }
            emit(dec.flush())?;
            Ok(())
        })
    }

    fn supported(&self) -> Result<()> {
        match &self.info.unsupported {
            Some(why) => Err(Error::Unsupported(why.clone())),
            None => Ok(()),
        }
    }

    /// Decode samples `first..=last` in decode order and return the picture
    /// whose pts is `want` (the closest, when no picture has exactly it), or the first picture out when `want` is `None`.
    fn decode(&self, first: usize, last: usize, want: Option<i64>) -> Result<Frame> {
        let t = self.t()?;
        if last.saturating_sub(first) > MAX_FEED {
            return Err(Error::Unsupported("the key frames of this clip are too far apart to seek".into()));
        }
        let entry = t.codec().ok_or_else(|| Error::Malformed("the video track has no codec record".into()))?;
        let mut dec = Decoder::new(entry)?;
        let mut best: Option<(u64, Frame)> = None;
        let take = |frames: Vec<Frame>, best: &mut Option<(u64, Frame)>| -> bool {
            for f in frames {
                let diff = want.map_or(0, |w| f.pts.abs_diff(w));
                if best.as_ref().is_none_or(|(d, _)| diff < *d) {
                    *best = Some((diff, f));
                }
            }
            best.as_ref().is_some_and(|(d, _)| *d == 0)
        };
        for i in first..=last {
            let Some(s) = t.samples.get(i) else { break };
            if s.size > MAX_SAMPLE {
                return Err(Error::Malformed("a picture claims to be larger than 256 MiB".into()));
            }
            let data = timed("decode: read", || self.mp4.read_sample(&self.src, self.track, i))?;
            if take(timed("decode: feed", || dec.feed(&data, s.pts))?, &mut best) {
                break;
            }
        }
        // (the picture wanted is among those fed; the decoder may hold it for the ones that follow in show order, which
        // `flush` releases at once, where feeding on would decode pictures nobody asked for)
        if best.as_ref().is_none_or(|(d, _)| want.is_some() && *d != 0) {
            take(timed("decode: flush", || dec.flush()), &mut best);
        }
        best.map(|(_, f)| f).ok_or_else(|| Error::Malformed("the decoder produced no picture".into()))
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Clip<std::fs::File> {
    /// Open a clip on disk (reads only the box headers and `moov`).
    pub fn open_path(path: &std::path::Path) -> Result<Clip<std::fs::File>> {
        let f = std::fs::File::open(path).map_err(|e| Error::Io(format!("{}: {e}", path.display())))?;
        Clip::open(f)
    }
}

/// The decoder of a track's codec.
enum Decoder {
    #[cfg(feature = "h264")]
    H264(Box<filmcraft_h264::Decoder>),
    #[cfg(feature = "hevc")]
    Hevc(Box<filmcraft_hevc::Decoder>),
}

impl Decoder {
    fn new(codec: &CodecConfig) -> Result<Decoder> {
        match codec {
            #[cfg(feature = "h264")]
            CodecConfig::Avc(c) => {
                let mut d = filmcraft_h264::Decoder::with_threads(1);
                d.configure_avcc(&c.to_bytes()).map_err(|e| Error::Malformed(e.to_string()))?;
                Ok(Decoder::H264(Box::new(d)))
            }
            #[cfg(feature = "hevc")]
            CodecConfig::Hevc(c) => {
                let mut d = filmcraft_hevc::Decoder::with_threads(1);
                d.configure_hvcc(&c.to_bytes()).map_err(|e| Error::Malformed(e.to_string()))?;
                Ok(Decoder::Hevc(Box::new(d)))
            }
            _ => Err(Error::Unsupported("this video codec is not supported".into())),
        }
    }

    fn feed(&mut self, data: &[u8], pts: i64) -> Result<Vec<Frame>> {
        match self {
            #[cfg(feature = "h264")]
            Decoder::H264(d) => Ok(d.decode(data, pts).map_err(|e| Error::Malformed(e.to_string()))?.into_iter().map(from_h264).collect()),
            #[cfg(feature = "hevc")]
            Decoder::Hevc(d) => Ok(d.decode(data, pts).map_err(|e| Error::Malformed(e.to_string()))?.into_iter().map(from_hevc).collect()),
        }
    }

    fn flush(&mut self) -> Vec<Frame> {
        match self {
            #[cfg(feature = "h264")]
            Decoder::H264(d) => d.flush().into_iter().map(from_h264).collect(),
            #[cfg(feature = "hevc")]
            Decoder::Hevc(d) => d.flush().into_iter().map(from_hevc).collect(),
        }
    }
}

#[cfg(feature = "h264")]
fn from_h264(p: filmcraft_h264::Picture) -> Frame {
    Frame {
        width: p.width as usize,
        height: p.height as usize,
        chroma_width: p.chroma_width as usize,
        chroma_height: p.chroma_height as usize,
        bit_depth: 8,
        y: yuv::Plane::U8(p.y),
        u: yuv::Plane::U8(p.u),
        v: yuv::Plane::U8(p.v),
        y_stride: p.y_stride,
        uv_stride: p.uv_stride,
        pts: p.pts,
        color: Color {
            full_range: p.color.full_range,
            primaries: u16::from(p.color.primaries),
            transfer: u16::from(p.color.transfer),
            matrix: u16::from(p.color.matrix),
        },
    }
}

#[cfg(feature = "hevc")]
fn from_hevc(p: filmcraft_hevc::Picture) -> Frame {
    let plane = |p: filmcraft_hevc::Plane| match p {
        filmcraft_hevc::Plane::U8(v) => yuv::Plane::U8(v),
        filmcraft_hevc::Plane::U16(v) => yuv::Plane::U16(v),
    };
    Frame {
        width: p.width as usize,
        height: p.height as usize,
        chroma_width: p.chroma_width as usize,
        chroma_height: p.chroma_height as usize,
        bit_depth: u8::try_from(p.bit_depth).unwrap_or(0),
        y: plane(p.y),
        u: plane(p.u),
        v: plane(p.v),
        y_stride: p.y_stride,
        uv_stride: p.uv_stride,
        pts: p.pts,
        color: Color {
            full_range: p.color.full_range,
            primaries: u16::from(p.color.primaries),
            transfer: u16::from(p.color.transfer),
            matrix: u16::from(p.color.matrix),
        },
    }
}

#[cfg(test)]
mod tests;
