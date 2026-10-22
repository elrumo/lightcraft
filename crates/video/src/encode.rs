//! Writing a clip: pictures encoded as H.264 (FilmCraft's pure-Rust encoder) into an MP4, with the audio of the clip they
//! came from copied alongside, sample for sample (it is not decoded, so it costs nothing and loses nothing).

use std::io::{Seek, Write};

use filmcraft_h264enc::{Encoder, EncoderConfig, Packet, PacketFormat, Profile, RateControl, YuvFrame};
use filmcraft_isobmff::{AvcConfig, Brand, ByteSource, Mp4Writer, SampleEntry, TrackConfig, TrackKind, WriteSample, WriterOptions};
use lightcraft_raster::Rgba8;

use crate::{Clip, Error, Result, guarded};

/// Ticks a second in the video track.
const TIMESCALE: u32 = 90_000;
/// The most pictures an encoder may be handed (a day of 60 pictures a second): a bound for a damaged clip's length.
const MAX_FRAMES: u64 = 5_184_000;

/// How a clip is written.
#[derive(Debug, Clone, Copy)]
pub struct EncodeOptions {
    /// The size of every picture pushed (even).
    pub width: usize,
    pub height: usize,
    /// Pictures a second, for the encoder's rate control (the pictures' own times set the clip's).
    pub fps: f64,
    /// Constant quality: lower is better and bigger (18 near lossless to the eye, 23 balanced, 28 small).
    pub crf: f32,
    /// B pictures between references (0 to 3); they make the clip smaller.
    pub bframes: u8,
}

/// How an encode is paced and what it aims at, beyond [`EncodeOptions`] (the default is what [`Mp4Encoder::new`] does).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tuning {
    /// Aim at this average bitrate (kbit/s, with room to burst to half as much again) instead of the constant quality.
    pub kbps: Option<u32>,
    /// Worker threads (0: every core). 1 encodes on the calling thread, which the caller can give a low priority.
    pub threads: usize,
}

/// The audio track of a clip, to be copied into the file written ([`Mp4Encoder::new`], [`Mp4Encoder::copy_audio`]).
#[derive(Debug, Clone)]
pub struct AudioTrack {
    entry: SampleEntry,
    timescale: u32,
    index: usize,
}

impl<S: ByteSource> Clip<S> {
    /// The clip's first audio track that has sound, if it has one.
    pub fn audio_track(&self) -> Option<AudioTrack> {
        let (index, t) = self.mp4().tracks.iter().enumerate().find(|(_, t)| t.kind == TrackKind::Audio && !t.samples.is_empty())?;
        let entry = t.entries.first()?.clone();
        Some(AudioTrack { entry, timescale: t.timescale, index })
    }
}

/// An MP4 being written: [`push`](Self::push) the pictures in the order they are shown, optionally
/// [`copy_audio`](Self::copy_audio), then [`finish`](Self::finish). Dropping it unfinished writes nothing complete.
pub struct Mp4Encoder<W: Write + Seek> {
    enc: Encoder,
    out: Mp4Writer<W>,
    /// The tracks are added with the first packet, which says how far the first picture is shown after it is decoded.
    avc: AvcConfig,
    audio_entry: Option<(SampleEntry, u32)>,
    video: Option<usize>,
    audio: Option<usize>,
    /// When the first picture pushed is shown (ticks).
    first_pts: Option<i64>,
    width: usize,
    height: usize,
    /// The packet waiting for the next one's time to know how long it lasts.
    pending: Option<Packet>,
    /// How long the last packet written lasted (the last picture lasts as long).
    last_duration: u32,
    frames: u64,
    /// The encoder has been flushed: no more pictures.
    drained: bool,
}

impl<W: Write + Seek> Mp4Encoder<W> {
    pub fn new(out: W, o: EncodeOptions, audio: Option<&AudioTrack>) -> Result<Self> {
        Self::new_tuned(out, o, Tuning::default(), audio)
    }

    /// [`new`](Self::new) with a bitrate to aim at and a number of threads ([`Tuning`]).
    pub fn new_tuned(out: W, o: EncodeOptions, t: Tuning, audio: Option<&AudioTrack>) -> Result<Self> {
        guarded("starting the encoder", || {
            if o.width == 0 || o.height == 0 || !o.width.is_multiple_of(2) || !o.height.is_multiple_of(2) || o.width > 16384 || o.height > 16384 {
                return Err(Error::Unsupported(format!("a {}x{} picture can't be encoded (an even size up to 16384)", o.width, o.height)));
            }
            if !o.fps.is_finite() || o.fps <= 0.0 || o.fps > 1000.0 {
                return Err(Error::Unsupported("the frame rate is not usable".into()));
            }
            let mut cfg = EncoderConfig::new(o.width as u32, o.height as u32, (o.fps * 1000.0).round().max(1.0) as u32, 1000);
            cfg.profile = if o.bframes > 0 { Profile::High } else { Profile::Main };
            cfg.rate = match t.kbps {
                Some(k) => RateControl::Vbr { target_kbps: k.clamp(50, 400_000), max_kbps: k.clamp(50, 400_000).saturating_mul(3) / 2 },
                None => RateControl::Crf(o.crf.clamp(10.0, 40.0)),
            };
            cfg.threads = t.threads;
            cfg.bframes = o.bframes.min(3);
            cfg.keyint = ((o.fps * 2.0).round() as u32).clamp(1, 250);
            cfg.scenecut = false;
            cfg.format = PacketFormat::LengthPrefixed;
            let enc = Encoder::new(cfg).map_err(|e| Error::Unsupported(e.to_string()))?;
            let avcc = AvcConfig::parse(&enc.avcc()).map_err(|e| Error::Malformed(e.to_string()))?;
            let out = Mp4Writer::new(out, WriterOptions::new(Brand::Mp4))?;
            Ok(Mp4Encoder {
                enc,
                out,
                avc: avcc,
                audio_entry: audio.map(|a| (a.entry.clone(), a.timescale)),
                video: None,
                audio: None,
                first_pts: None,
                width: o.width,
                height: o.height,
                pending: None,
                last_duration: TIMESCALE / 30,
                frames: 0,
                drained: false,
            })
        })
    }

    /// Encode `picture` (8-bit sRGB, the size given to [`new`](Self::new)) shown `seconds` into the clip.
    pub fn push(&mut self, picture: &Rgba8, seconds: f64) -> Result<()> {
        guarded("encoding a picture", || {
            if self.drained {
                return Err(Error::Malformed("a picture pushed after the sound was copied".into()));
            }
            if picture.width != self.width || picture.height != self.height {
                return Err(Error::Malformed(format!("a {}x{} picture in a {}x{} clip", picture.width, picture.height, self.width, self.height)));
            }
            if !seconds.is_finite() || seconds < 0.0 {
                return Err(Error::Malformed("a picture's time is not usable".into()));
            }
            self.frames += 1;
            if self.frames > MAX_FRAMES {
                return Err(Error::Unsupported("the clip has too many pictures".into()));
            }
            let (y, u, v) = yuv420(picture);
            let frame = YuvFrame { y: &y, u: &u, v: &v, y_stride: self.width, uv_stride: self.width / 2 };
            let pts = (seconds * f64::from(TIMESCALE)).round() as i64;
            self.first_pts.get_or_insert(pts);
            let packets = self.enc.encode(&frame, pts).map_err(|e| Error::Malformed(e.to_string()))?;
            for p in packets {
                self.write(p)?;
            }
            Ok(())
        })
    }

    /// Write the packet held back, now that `next` says when the following one starts (`None`: it is the last).
    fn write(&mut self, next: Packet) -> Result<()> {
        if let Some(p) = self.pending.replace(next) {
            let following = self.pending.as_ref().map_or(p.dts, |n| n.dts);
            self.write_one(&p, u32::try_from((following - p.dts).max(1)).unwrap_or(self.last_duration))?;
        }
        Ok(())
    }

    fn write_one(&mut self, p: &Packet, duration: u32) -> Result<()> {
        let video = match self.video {
            Some(v) => v,
            None => {
                // the first sample starts the file's clock at 0, however early the encoder decodes it: an edit puts the first
                // picture shown at 0
                let mut cfg = TrackConfig::new(SampleEntry::avc(self.avc.clone(), self.width as u16, self.height as u16), TIMESCALE);
                cfg.media_start = Some((self.first_pts.unwrap_or(p.pts) - p.dts).max(0));
                let v = self.out.add_track(cfg)?;
                if let Some((entry, scale)) = self.audio_entry.take() {
                    self.audio = Some(self.out.add_track(TrackConfig::new(entry, scale))?);
                }
                self.video = Some(v);
                v
            }
        };
        self.last_duration = duration;
        let offset = i32::try_from(p.pts - p.dts).map_err(|_| Error::Malformed("a picture is shown too long after it is decoded".into()))?;
        self.out.write_sample(video, WriteSample { data: &p.data, duration, composition_offset: offset, is_sync: p.keyframe })?;
        Ok(())
    }

    /// Copy the sound of `clip` between `start` and `end` seconds (its samples are copied as they are, the first one at
    /// the start of the sound: a few hundredths of a second off the picture at most). Call after the last [`push`](Self::push).
    pub fn copy_audio<S: ByteSource>(&mut self, clip: &Clip<S>, track: &AudioTrack, start: f64, end: f64) -> Result<u64> {
        // (the tracks are added with the first packet: the encoder is flushed so that it has come)
        self.drain()?;
        let Some(out) = self.audio else { return Ok(0) };
        guarded("copying the sound", || {
            let t = clip.mp4().tracks.get(track.index).ok_or_else(|| Error::Malformed("the clip has no sound track".into()))?;
            let scale = f64::from(t.timescale.max(1));
            let mut copied = 0;
            for (i, s) in t.samples.iter().enumerate() {
                let seconds = (s.pts.saturating_add(t.edit_offset)) as f64 / scale;
                if seconds < start - 0.001 {
                    continue;
                }
                if seconds >= end {
                    break;
                }
                if s.size > (16 << 20) {
                    return Err(Error::Malformed("a sound sample claims to be larger than 16 MiB".into()));
                }
                let data = clip.mp4().read_sample(clip.source(), track.index, i)?;
                self.out.write_sample(out, WriteSample { data: &data, duration: s.duration, composition_offset: 0, is_sync: true })?;
                copied += 1;
            }
            Ok(copied)
        })
    }

    /// Flush the encoder: every picture pushed is written (the last one waits for its duration).
    fn drain(&mut self) -> Result<()> {
        if self.drained {
            return Ok(());
        }
        self.drained = true;
        guarded("finishing the pictures", || {
            for p in self.enc.flush() {
                self.write(p)?;
            }
            // the last picture lasts as long as the one before it
            if let Some(p) = self.pending.take() {
                let d = self.last_duration;
                self.write_one(&p, d)?;
            }
            Ok(())
        })
    }

    /// Flush the encoder and close the file; the writer comes back.
    pub fn finish(mut self) -> Result<W> {
        self.drain()?;
        guarded("finishing the clip", || {
            if self.frames == 0 {
                return Err(Error::Malformed("no pictures were pushed".into()));
            }
            Ok(self.out.finish()?)
        })
    }
}

/// 8-bit sRGB → planar BT.709 limited-range 4:2:0 (chroma averaged over 2×2).
fn yuv420(img: &Rgba8) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let (w, h) = (img.width, img.height);
    let mut y = vec![0u8; w * h];
    let (cw, ch) = (w / 2, h / 2);
    let (mut u, mut v) = (vec![128u8; cw * ch], vec![128u8; cw * ch]);
    let luma = |p: &[u8; 4]| 0.2126 * f32::from(p[0]) + 0.7152 * f32::from(p[1]) + 0.0722 * f32::from(p[2]);
    for (o, p) in y.iter_mut().zip(&img.data) {
        *o = (16.0 + 219.0 / 255.0 * luma(p)).round().clamp(16.0, 235.0) as u8;
    }
    for cy in 0..ch {
        for cx in 0..cw {
            let (mut sb, mut sr) = (0.0f32, 0.0f32);
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let p = &img.data.get((cy * 2 + dy) * w + cx * 2 + dx).copied().unwrap_or([0, 0, 0, 255]);
                let l = luma(p);
                sb += f32::from(p[2]) - l;
                sr += f32::from(p[0]) - l;
            }
            let i = cy * cw + cx;
            u[i] = (128.0 + 224.0 / 255.0 * (sb / 4.0) / (2.0 * (1.0 - 0.0722))).round().clamp(16.0, 240.0) as u8;
            v[i] = (128.0 + 224.0 / 255.0 * (sr / 4.0) / (2.0 * (1.0 - 0.2126))).round().clamp(16.0, 240.0) as u8;
        }
    }
    (y, u, v)
}
