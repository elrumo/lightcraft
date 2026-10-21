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
        let (y, u, v) = (vec![luma(i); W * H], vec![128u8; W * H / 4], vec![128u8; W * H / 4]);
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
    for p in &packets {
        if w.write_sample(t, WriteSample { data: &p.data, duration: STEP as u32, composition_offset: (p.pts - p.dts) as i32, is_sync: p.keyframe })
            .is_err()
        {
            return Vec::new();
        }
    }
    w.finish().map(Cursor::into_inner).unwrap_or_default()
}
