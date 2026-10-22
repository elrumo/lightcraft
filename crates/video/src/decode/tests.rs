//! The poster and the pictures of a clip against how they were made before a poster stopped feeding the decoder past its
//! key frame and the conversion got its curve tables: the same decoder output, converted the long way.

use std::sync::atomic::{AtomicU64, Ordering};

use super::*;
use crate::testdata::{self, H, W};
use crate::yuv::tests::{max_diff, reference};

/// `data` (an MP4) with its video track turned `quarter` quarter turns clockwise for display, by writing the `tkhd` matrix.
fn turned(mut data: Vec<u8>, quarter: u8) -> Vec<u8> {
    let at = data.windows(4).position(|w| w == b"tkhd").expect("a track header");
    let matrix = at + 4 + if data[at + 4] == 1 { 52 } else { 40 };
    let (a, b, c, d): (i32, i32, i32, i32) = match quarter % 4 {
        0 => (1, 0, 0, 1),
        1 => (0, 1, -1, 0),
        2 => (-1, 0, 0, -1),
        _ => (0, -1, 1, 0),
    };
    for (slot, v) in [(0, a), (1, b), (3, c), (4, d)] {
        data[matrix + 4 * slot..matrix + 4 * slot + 4].copy_from_slice(&(v * 0x10000).to_be_bytes());
    }
    data
}

/// The first picture the decoder gives when the key frame `key` and what follows are fed until one comes out (what a poster
/// did), else the first one a flush releases.
fn fed_until_one_comes_out(c: &Clip<Vec<u8>>, key: usize) -> Frame {
    let t = c.t().unwrap();
    let mut dec = Decoder::new(t.codec().unwrap()).unwrap();
    for i in key..t.samples.len() {
        if i > key && t.samples[i].is_sync {
            break;
        }
        let data = c.mp4.read_sample(&c.src, c.track, i).unwrap();
        if let Some(f) = dec.feed(&data, t.samples[i].pts).unwrap().into_iter().next() {
            return f;
        }
    }
    dec.flush().into_iter().next().expect("a picture")
}

/// Every picture of the clip in the order they are shown: all samples fed in order, then a flush.
fn every_picture(c: &Clip<Vec<u8>>) -> Vec<Frame> {
    let t = c.t().unwrap();
    let mut dec = Decoder::new(t.codec().unwrap()).unwrap();
    let mut out = Vec::new();
    for (i, s) in t.samples.iter().enumerate() {
        out.extend(dec.feed(&c.mp4.read_sample(&c.src, c.track, i).unwrap(), s.pts).unwrap());
    }
    out.extend(dec.flush());
    out.sort_by_key(|f| f.pts);
    out
}

#[test]
fn a_poster_is_the_picture_it_was_with_and_without_b_pictures_and_turned() {
    for bframes in [0, 2] {
        for quarter in [0, 1, 2, 3] {
            let c = Clip::open(turned(testdata::clip_textured(20, 10, bframes), quarter)).unwrap();
            assert_eq!(c.info().rotation, quarter, "the test's own matrix");
            let t = c.t().unwrap();
            let key = t.sync_sample_before(t.sample_at_pts(t.duration as i64 / 10).unwrap());
            let f = fed_until_one_comes_out(&c, key);
            let f = Frame { color: f.color.or(c.color), ..f };
            for edge in [16, 40, 50, 64, 500] {
                // the smallest whole reduction that brings the long side to at most twice the edge
                let factor = W.div_ceil(2 * edge).max(1);
                let mut want = reference(&f, factor);
                for _ in 0..quarter {
                    want = want.rotate_cw();
                }
                let got = c.poster(edge).unwrap();
                assert_eq!((got.width, got.height), (want.width, want.height), "{bframes} B, turned {quarter}, edge {edge}");
                let d = max_diff(&got, &want);
                assert!(d <= 2e-6, "{bframes} B, turned {quarter}, edge {edge}: differs by {d}");
                let long = got.width.max(got.height);
                assert!(long <= (2 * edge).max(W) && long >= edge.min(W), "{}x{} for {edge}", got.width, got.height);
            }
        }
    }
}

#[test]
fn every_picture_of_a_clip_with_b_pictures_is_found() {
    for bframes in [0, 2] {
        let c = Clip::open(testdata::clip_textured(20, 10, bframes)).unwrap();
        let all = every_picture(&c);
        assert_eq!(all.len(), 20);
        for (k, f) in all.into_iter().enumerate() {
            let want = yuv::to_working(&Frame { color: f.color.or(c.color), ..f }, 512, 0).unwrap();
            let got = c.frame_at(k as f64 / 10.0, 512).unwrap();
            assert_eq!((got.width, got.height), (W, H));
            assert!(got.data == want.data, "{bframes} B, picture {k}");
        }
    }
}

/// A clip in memory that counts the bytes read from it.
struct Counting {
    data: Vec<u8>,
    read: AtomicU64,
}

impl ByteSource for Counting {
    fn len(&self) -> u64 {
        self.data.len() as u64
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> std::io::Result<()> {
        self.read.fetch_add(buf.len() as u64, Ordering::Relaxed);
        self.data.read_at(offset, buf)
    }
}

#[test]
fn a_poster_reads_only_its_key_frame() {
    for bframes in [0, 2] {
        let c = Clip::open(Counting { data: testdata::clip_textured(20, 10, bframes), read: AtomicU64::new(0) }).unwrap();
        let t = c.t().unwrap();
        let key = t.sync_sample_before(t.sample_at_pts(t.duration as i64 / 10).unwrap());
        let size = u64::from(t.samples[key].size);
        let before = c.source().read.load(Ordering::Relaxed);
        c.poster(512).unwrap();
        assert_eq!(c.source().read.load(Ordering::Relaxed) - before, size, "{bframes} B: the pictures after the key frame were read too");
    }
}
