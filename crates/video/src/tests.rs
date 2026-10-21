use std::io::Cursor;

use filmcraft_h264enc::{Encoder, EncoderConfig, PacketFormat, Profile, RateControl, YuvFrame};
use filmcraft_isobmff::{AvcConfig, Brand, Mp4Writer, SampleEntry, TrackConfig, WriteSample, WriterOptions};

use crate::info::parse_iso6709;
use crate::yuv::{Color, Frame, Plane, to_working};
use crate::{Clip, Error, is_video, probe};

use crate::testdata::{H, STEP, W, clip};

/// Mean of the red channel of a picture (linear Rec.2020).
fn mean_red(img: &lightcraft_raster::Rgb32f) -> f32 {
    img.data.iter().map(|p| p[0]).sum::<f32>() / img.data.len() as f32
}

#[test]
fn probe_reads_size_duration_and_codec() {
    let data = clip(20, 10, 0);
    assert!(is_video(&data));
    let info = probe(data).unwrap();
    assert_eq!((info.width, info.height, info.codec), (W as u32, H as u32, "h264"));
    assert!((info.duration - 2.0).abs() < 0.01, "{}", info.duration);
    assert!((info.fps - 10.0).abs() < 0.1, "{}", info.fps);
    assert_eq!((info.frames, info.rotation, info.has_audio, info.hdr), (20, 0, false, false));
    assert_eq!(info.unsupported, None);
}

#[test]
fn a_poster_is_a_key_frame_of_the_right_size_and_not_black() {
    let c = Clip::open(clip(20, 10, 0)).unwrap();
    let img = c.poster(512).unwrap();
    assert_eq!((img.width, img.height), (W, H));
    assert!(mean_red(&img) > 0.005, "{}", mean_red(&img));
}

#[test]
fn a_poster_skips_a_black_start() {
    // pictures 0-9 are black (luma 16), the clip then brightens: the poster must not be the first key frame
    let mut cfg = EncoderConfig::new(W as u32, H as u32, 10, 1);
    cfg.profile = Profile::Baseline;
    cfg.rate = RateControl::Qp(18);
    cfg.keyint = 5;
    cfg.scenecut = false;
    cfg.threads = 1;
    cfg.format = PacketFormat::LengthPrefixed;
    let mut enc = Encoder::new(cfg).unwrap();
    let avcc = AvcConfig::parse(&enc.avcc()).unwrap();
    let mut w = Mp4Writer::new(Cursor::new(Vec::new()), WriterOptions::new(Brand::Mp4)).unwrap();
    let t = w.add_track(TrackConfig::new(SampleEntry::avc(avcc, W as u16, H as u16), 10 * STEP as u32)).unwrap();
    let mut out = Vec::new();
    for i in 0..20 {
        let l = if i < 10 { 16 } else { 150 };
        let (y, u, v) = (vec![l; W * H], vec![128u8; W * H / 4], vec![128u8; W * H / 4]);
        out.extend(enc.encode(&YuvFrame { y: &y, u: &u, v: &v, y_stride: W, uv_stride: W / 2 }, i * STEP).unwrap());
    }
    out.extend(enc.flush());
    for p in &out {
        w.write_sample(t, WriteSample { data: &p.data, duration: STEP as u32, composition_offset: 0, is_sync: p.keyframe }).unwrap();
    }
    let c = Clip::open(w.finish().unwrap().into_inner()).unwrap();
    // 10 % in is picture 2 (black), 25 % picture 5 (black), 50 % picture 10 (bright)
    assert!(mean_red(&c.poster(512).unwrap()) > 0.1);
}

#[test]
fn frame_at_decodes_forward_from_the_key_frame() {
    let c = Clip::open(clip(20, 10, 0)).unwrap();
    let reds: Vec<f32> = [0.0, 0.3, 0.9, 1.0, 1.3, 1.9].iter().map(|t| mean_red(&c.frame_at(*t, 512).unwrap())).collect();
    // each is at a different picture (0, 3, 9, 10, 13, 19) and the clip brightens, so the order is the time order
    assert!(reds.windows(2).all(|w| w[0] < w[1]), "{reds:?}");
}

#[test]
fn frame_at_follows_presentation_order_with_b_pictures() {
    let c = Clip::open(clip(20, 10, 2)).unwrap();
    let reds: Vec<f32> = (0..20).step_by(3).map(|i| mean_red(&c.frame_at(i as f64 / 10.0, 512).unwrap())).collect();
    assert!(reds.windows(2).all(|w| w[0] < w[1]), "{reds:?}");
}

#[test]
fn a_time_outside_the_clip_is_clamped() {
    let c = Clip::open(clip(20, 10, 0)).unwrap();
    let last = mean_red(&c.frame_at(1.9, 512).unwrap());
    assert!((mean_red(&c.frame_at(1e9, 512).unwrap()) - last).abs() < 1e-4);
    assert!((mean_red(&c.frame_at(f64::NAN, 512).unwrap()) - mean_red(&c.frame_at(0.0, 512).unwrap())).abs() < 1e-4);
    assert!(mean_red(&c.frame_at(-5.0, 512).unwrap()) < last);
}

#[test]
fn the_picture_is_reduced_for_a_small_request() {
    let c = Clip::open(clip(10, 10, 0)).unwrap();
    // 128 px long side, asked for at most about 2 × 16 px: reduced by 4
    let img = c.frame_at(0.0, 16).unwrap();
    assert_eq!((img.width, img.height), (W / 4, H / 4));
}

#[test]
fn damaged_files_are_errors_not_panics() {
    let data = clip(20, 10, 2);
    // truncated at every eighth of the file, and with a run of garbage written over the middle
    for cut in (1..8).map(|k| data.len() * k / 8) {
        let part = data[..cut].to_vec();
        if let Ok(c) = Clip::open(part) {
            let _ = c.poster(256);
            let _ = c.frame_at(1.0, 256);
        }
    }
    let mut bad = data.clone();
    let mid = bad.len() / 2;
    for b in &mut bad[mid..mid + 200] {
        *b = 0xA5;
    }
    if let Ok(c) = Clip::open(bad) {
        let _ = c.poster(256);
        let _ = c.frame_at(1.0, 256);
    }
    assert!(matches!(probe(vec![0u8; 100]), Err(Error::Malformed(_) | Error::Io(_))));
    assert!(probe(Vec::new()).is_err());
    assert!(!is_video(b"\xFF\xD8\xFF\xE0\0\x10JFIF\0\x01\x01"));
}

#[test]
fn only_movie_files_are_videos() {
    assert!(is_video(b"\0\0\0\x20ftypisom\0\0\x02\0"));
    assert!(is_video(b"\0\0\0\x14ftypqt  \0\0\0\0"));
    assert!(is_video(b"\0\0\0\x08wide\0\0\0\x08mdat"));
    // HEIC / AVIF stills and audio files are not
    assert!(!is_video(b"\0\0\0\x18ftypheic\0\0\0\0"));
    assert!(!is_video(b"\0\0\0\x18ftypavif\0\0\0\0"));
    assert!(!is_video(b"\0\0\0\x18ftypM4A \0\0\0\0"));
    assert!(!is_video(b"\0\0"));
}

#[test]
fn iso6709_positions() {
    assert_eq!(parse_iso6709("+48.8584+002.2945+010.000/"), Some((48.8584, 2.2945)));
    assert_eq!(parse_iso6709("-33.8688+151.2093/"), Some((-33.8688, 151.2093)));
    assert_eq!(parse_iso6709("+95.0+002.0/"), None);
    assert_eq!(parse_iso6709("+48.8584"), None);
    assert_eq!(parse_iso6709(""), None);
}

fn frame(w: usize, h: usize, y: u8, c: u8, color: Color) -> Frame {
    Frame {
        width: w,
        height: h,
        chroma_width: w.div_ceil(2),
        chroma_height: h.div_ceil(2),
        bit_depth: 8,
        y: Plane::U8(vec![y; w * h]),
        u: Plane::U8(vec![c; w.div_ceil(2) * h.div_ceil(2)]),
        v: Plane::U8(vec![c; w.div_ceil(2) * h.div_ceil(2)]),
        y_stride: w,
        uv_stride: w.div_ceil(2),
        pts: 0,
        color,
    }
}

#[test]
fn limited_range_black_and_white_map_to_zero_and_one() {
    let color = Color { full_range: false, primaries: 1, transfer: 1, matrix: 1 };
    let black = to_working(&frame(8, 8, 16, 128, color), 64, 0).unwrap();
    let white = to_working(&frame(8, 8, 235, 128, color), 64, 0).unwrap();
    assert!(black.data.iter().all(|p| p.iter().all(|c| c.abs() < 1e-4)));
    // white is (1, 1, 1) in the working space too (white-preserving primaries conversion)
    assert!(white.data.iter().all(|p| p.iter().all(|c| (c - 1.0).abs() < 1e-3)), "{:?}", white.data[0]);
}

#[test]
fn rotation_turns_the_picture() {
    let color = Color { full_range: true, primaries: 1, transfer: 1, matrix: 1 };
    let img = to_working(&frame(8, 4, 100, 128, color), 64, 1).unwrap();
    assert_eq!((img.width, img.height), (4, 8));
}

#[test]
fn a_picture_with_short_planes_is_refused() {
    let mut f = frame(8, 8, 100, 128, Color::UNSPECIFIED);
    f.y = Plane::U8(vec![0; 10]);
    assert!(to_working(&f, 64, 0).is_err());
    let mut g = frame(8, 8, 100, 128, Color::UNSPECIFIED);
    g.width = usize::MAX;
    assert!(to_working(&g, 64, 0).is_err());
    let mut h = frame(8, 8, 100, 128, Color::UNSPECIFIED);
    h.bit_depth = 31;
    assert!(to_working(&h, 64, 0).is_err());
}

#[cfg(feature = "encode")]
mod encode {
    use std::io::Cursor;

    use super::*;
    use crate::testdata::clip_with_sound;
    use crate::{EncodeOptions, Mp4Encoder};

    /// 8-bit sRGB of a decoded picture (the working space is linear Rec.2020; a grey is the same grey in any of them).
    fn srgb(img: &lightcraft_raster::Rgb32f) -> lightcraft_raster::Rgba8 {
        let m = lightcraft_color::WORKING.to_space(&lightcraft_color::SRGB).to_f32();
        let mut out = lightcraft_raster::Rgba8::new(img.width, img.height);
        for (o, px) in out.data.iter_mut().zip(&img.data) {
            let q = [0, 1, 2].map(|r| m[r][0] * px[0] + m[r][1] * px[1] + m[r][2] * px[2]);
            let e = lightcraft_color::transfer::encode_srgb8;
            *o = [e(q[0]), e(q[1]), e(q[2]), 255];
        }
        out
    }

    fn opts() -> EncodeOptions {
        EncodeOptions { width: W, height: H, fps: 10.0, crf: 20.0, bframes: 2 }
    }

    #[test]
    fn pictures_in_a_range_come_in_the_order_they_are_shown() {
        let c = Clip::open(clip(20, 10, 2)).unwrap();
        let mut seen = Vec::new();
        c.for_each_frame(0.35, 1.25, 64, |t, img| {
            seen.push((t, mean_red(&img)));
            Ok(true)
        })
        .unwrap();
        // pictures 4 to 12 (0.4 s to 1.2 s)
        assert_eq!(seen.len(), 9, "{seen:?}");
        assert!(seen.windows(2).all(|w| w[0].0 < w[1].0 && w[0].1 < w[1].1), "{seen:?}");
        assert!((seen[0].0 - 0.4).abs() < 0.01 && (seen[8].0 - 1.2).abs() < 0.01, "{seen:?}");
        // the caller can stop it
        let mut n = 0;
        c.for_each_frame(0.0, 2.0, 64, |_, _| {
            n += 1;
            Ok(n < 3)
        })
        .unwrap();
        assert_eq!(n, 3);
        // an error from the caller comes back
        assert!(c.for_each_frame(0.0, 2.0, 64, |_, _| Err(Error::Unsupported("no".into()))).is_err());
    }

    #[test]
    fn a_clip_written_again_has_the_same_pictures_and_length() {
        let c = Clip::open(clip(20, 10, 2)).unwrap();
        let mut enc = Mp4Encoder::new(Cursor::new(Vec::new()), opts(), None).unwrap();
        c.for_each_frame(0.0, 2.0, 512, |t, img| enc.push(&srgb(&img), t).map(|()| true)).unwrap();
        let bytes = enc.finish().unwrap().into_inner();
        let info = probe(bytes.clone()).unwrap();
        assert_eq!((info.width, info.height, info.codec, info.frames), (W as u32, H as u32, "h264", 20), "{info:?}");
        assert!((info.duration - 2.0).abs() < 0.05, "{}", info.duration);
        // and the pictures are the same ones, in the same order
        let (a, b) = (Clip::open(bytes).unwrap(), Clip::open(clip(20, 10, 2)).unwrap());
        for t in [0.0, 0.5, 1.0, 1.5, 1.9] {
            let (x, y) = (mean_red(&a.frame_at(t, 64).unwrap()), mean_red(&b.frame_at(t, 64).unwrap()));
            assert!((x - y).abs() < 0.02, "{t}: {x} vs {y}");
        }
    }

    #[test]
    fn a_trimmed_clip_keeps_its_part_and_its_sound() {
        let c = Clip::open(clip_with_sound(20, 10, 2)).unwrap();
        assert!(c.info().has_audio);
        let sound = c.audio_track().expect("a sound track");
        let mut enc = Mp4Encoder::new(Cursor::new(Vec::new()), opts(), Some(&sound)).unwrap();
        c.for_each_frame(0.5, 1.5, 512, |t, img| enc.push(&srgb(&img), t - 0.5).map(|()| true)).unwrap();
        let copied = enc.copy_audio(&c, &sound, 0.5, 1.5).unwrap();
        // a second of 48 kHz in 1024-frame samples
        assert!((45..=48).contains(&copied), "{copied}");
        let bytes = enc.finish().unwrap().into_inner();
        let out = Clip::open(bytes).unwrap();
        let i = out.info();
        assert_eq!(i.frames, 10, "{i:?}");
        assert!((i.duration - 1.0).abs() < 0.06, "{}", i.duration);
        assert!(i.has_audio);
        // it starts at the 5th picture of the original
        let first = mean_red(&out.frame_at(0.0, 64).unwrap());
        let original = mean_red(&c.frame_at(0.5, 64).unwrap());
        assert!((first - original).abs() < 0.02, "{first} vs {original}");
    }

    #[test]
    fn the_encoder_refuses_what_it_cannot_write() {
        for bad in [EncodeOptions { width: 127, ..opts() }, EncodeOptions { height: 0, ..opts() }, EncodeOptions { fps: 0.0, ..opts() }, EncodeOptions { fps: f64::NAN, ..opts() }] {
            assert!(Mp4Encoder::new(Cursor::new(Vec::new()), bad, None).is_err(), "{bad:?}");
        }
        let mut enc = Mp4Encoder::new(Cursor::new(Vec::new()), opts(), None).unwrap();
        // a picture of another size, a time that is not one
        assert!(enc.push(&lightcraft_raster::Rgba8::new(64, 48), 0.0).is_err());
        assert!(enc.push(&lightcraft_raster::Rgba8::new(W, H), f64::NAN).is_err());
        assert!(enc.push(&lightcraft_raster::Rgba8::new(W, H), -1.0).is_err());
        // nothing pushed: nothing to finish
        assert!(enc.finish().is_err());
    }

    #[test]
    fn one_picture_is_a_clip() {
        let mut enc = Mp4Encoder::new(Cursor::new(Vec::new()), EncodeOptions { bframes: 0, ..opts() }, None).unwrap();
        enc.push(&lightcraft_raster::Rgba8::from_fn(W, H, |_, _| [200, 100, 50, 255]), 0.0).unwrap();
        let info = probe(enc.finish().unwrap().into_inner()).unwrap();
        assert_eq!(info.frames, 1);
    }
}
