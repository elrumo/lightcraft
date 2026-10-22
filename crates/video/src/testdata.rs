//! Synthetic clips for tests (feature `testdata`): H.264 from FilmCraft's own encoder, muxed as MP4 by its muxer.

use std::io::Cursor;

use filmcraft_h264enc::{Encoder, EncoderConfig, PacketFormat, Profile, RateControl, YuvFrame};
use filmcraft_isobmff::{AvcConfig, Brand, Mp4Writer, SampleEntry, TrackConfig, WriteSample, WriterOptions};

pub const W: usize = 128;
pub const H: usize = 96;
/// Media ticks per picture (the track timescale is ten times as many: 10 pictures a second).
pub const STEP: i64 = 1000;

/// Luma of picture `i` of [`clip`]: a flat grey that brightens by 6 code values each picture.
pub fn grey(i: usize) -> u8 {
    40 + 6 * i as u8
}

/// A `frames`-picture H.264 clip, 128 × 96, 10 pictures a second, every picture a flat grey (see [`grey`]), a key
/// frame every `keyint` pictures, `bframes` B pictures between references. Muxed as MP4.
pub fn clip(frames: usize, keyint: u32, bframes: u8) -> Vec<u8> {
    clip_with(frames, keyint, bframes, grey)
}

/// [`clip`] with picture `i` a flat grey of luma `luma(i)`.
pub fn clip_with(frames: usize, keyint: u32, bframes: u8, luma: impl Fn(usize) -> u8) -> Vec<u8> {
    build(frames, keyint, bframes, flat(luma), false)
}

/// [`clip`] with pictures that have something in them: a diagonal luma ramp that moves, and chroma that changes across and
/// down the picture (a flat grey cannot tell a wrong chroma sample, a mirrored or a turned picture from a right one).
pub fn clip_textured(frames: usize, keyint: u32, bframes: u8) -> Vec<u8> {
    let picture = |i: usize| {
        let y = (0..W * H).map(|p| 40 + ((p % W) * 2 + (p / W) * 3 + i * 5) as u8 % 160).collect();
        let u = (0..W * H / 4).map(|p| 80 + ((p % (W / 2)) * 3) as u8 % 90).collect();
        let v = (0..W * H / 4).map(|p| 70 + ((p / (W / 2)) * 4) as u8 % 100).collect();
        (y, u, v)
    };
    build(frames, keyint, bframes, picture, false)
}

/// Every picture a flat grey of luma `luma(i)`.
fn flat(luma: impl Fn(usize) -> u8) -> impl Fn(usize) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    move |i| (vec![luma(i); W * H], vec![128u8; W * H / 4], vec![128u8; W * H / 4])
}

/// [`clip`] with a sound track beside it: 48 kHz AAC "samples" of 1024 frames that are not sound (they are copied, never
/// decoded), one byte each, `n` where `n` counts from 0.
pub fn clip_with_sound(frames: usize, keyint: u32, bframes: u8) -> Vec<u8> {
    build(frames, keyint, bframes, flat(grey), true)
}

fn build(frames: usize, keyint: u32, bframes: u8, picture: impl Fn(usize) -> (Vec<u8>, Vec<u8>, Vec<u8>), sound: bool) -> Vec<u8> {
    let mut cfg = EncoderConfig::new(W as u32, H as u32, 10, 1);
    cfg.profile = if bframes > 0 { Profile::High } else { Profile::Baseline };
    cfg.rate = RateControl::Qp(18);
    cfg.keyint = keyint;
    cfg.bframes = bframes;
    cfg.scenecut = false;
    cfg.threads = 1;
    cfg.format = PacketFormat::LengthPrefixed;
    let Ok(mut enc) = Encoder::new(cfg) else { return Vec::new() };
    let Ok(avcc) = AvcConfig::parse(&enc.avcc()) else { return Vec::new() };
    let mut packets = Vec::new();
    for i in 0..frames {
        let (y, u, v) = picture(i);
        let f = YuvFrame { y: &y, u: &u, v: &v, y_stride: W, uv_stride: W / 2 };
        if let Ok(p) = enc.encode(&f, i as i64 * STEP) {
            packets.extend(p);
        }
    }
    packets.extend(enc.flush());
    let first_pts = packets.iter().map(|p| p.pts).min().unwrap_or(0);
    let first_dts = packets.first().map_or(0, |p| p.dts);
    let Ok(mut w) = Mp4Writer::new(Cursor::new(Vec::new()), WriterOptions::new(Brand::Mp4)) else { return Vec::new() };
    let mut track = TrackConfig::new(SampleEntry::avc(avcc, W as u16, H as u16), 10 * STEP as u32);
    track.media_start = Some(first_pts - first_dts);
    let Ok(t) = w.add_track(track) else { return Vec::new() };
    let a = if sound { w.add_track(TrackConfig::new(SampleEntry::aac(vec![0x11, 0x90], 2, 48_000), 48_000)).ok() } else { None };
    for p in &packets {
        if w.write_sample(t, WriteSample { data: &p.data, duration: STEP as u32, composition_offset: (p.pts - p.dts) as i32, is_sync: p.keyframe })
            .is_err()
        {
            return Vec::new();
        }
    }
    if let Some(a) = a {
        // as long as the pictures: frames / 10 seconds
        for n in 0..(frames * 48_000 / 10 / 1024 + 1) {
            if w.write_sample(a, WriteSample { data: &[n as u8], duration: 1024, composition_offset: 0, is_sync: true }).is_err() {
                return Vec::new();
            }
        }
    }
    w.finish().map(Cursor::into_inner).unwrap_or_default()
}
