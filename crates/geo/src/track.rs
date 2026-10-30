//! Recorded tracks: where a person was over time, as points with a moment, a position and
//! optionally an elevation and the accuracy of the fix. This module packs them for storing and
//! syncing, reads them back, works out what a track covers (distance, climb, time, box) and cuts a
//! long recording into pieces that stay small.
//!
//! The packed form is columnar (all the times, then all the latitudes…), each column as differences
//! in variable-length integers, then deflated: about 5 to 8 bytes a point for a walk sampled every
//! few seconds, against some 60 in JSON. Every read is bounded: a damaged or hostile payload is an
//! [`Error`], never a panic, and never more than [`MAX_POINTS`] points or [`MAX_INFLATED`] bytes.

use miniz_oxide::deflate::compress_to_vec;
use miniz_oxide::inflate::decompress_to_vec_with_limit;

use crate::geodesy::{distance_km, valid};

/// The most points in one packed track.
pub const MAX_POINTS: usize = 100_000;
/// A long recording is cut into pieces of at most this many points…
pub const CHUNK_POINTS: usize = 20_000;
/// …and at most this much time (24 hours, in milliseconds).
pub const CHUNK_SPAN_MS: i64 = 24 * 3600 * 1000;
/// The most a payload inflates to (a corrupt payload can't exhaust memory).
pub const MAX_INFLATED: usize = 16 << 20;
/// The format number a payload starts with.
const FORMAT: u8 = 1;
/// Times are milliseconds since 1970, up to the end of the year 9999.
const TIME_MAX_MS: i64 = 253_402_300_799_000;
/// Elevations outside this (metres above sea level) are not real.
const ELE_MIN_M: f64 = -500.0;
const ELE_MAX_M: f64 = 20_000.0;
/// The accuracy of a fix, at most (metres).
const ACC_MAX_M: f64 = 100_000.0;
/// Positions are kept to a millionth of a degree (about 11 cm), elevations and accuracies to a tenth of a metre.
const POS_UNIT: f64 = 1e6;
const DM: f64 = 10.0;
/// A climb or descent counts only after the elevation moved this far (metres): GPS elevation wobbles.
const CLIMB_STEP_M: f64 = 3.0;

const FLAG_SEGMENT: u8 = 1;
const FLAG_ELE: u8 = 2;
const FLAG_ACC: u8 = 4;

/// One fix of a track.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    /// Milliseconds since 1970-01-01T00:00:00Z.
    pub time_ms: i64,
    pub lat: f64,
    pub lon: f64,
    /// Metres above sea level.
    pub ele_m: Option<f64>,
    /// How far off the position may be (metres).
    pub acc_m: Option<f64>,
    /// This point begins a new stretch of recording (reception was lost, or the person paused): no line is drawn,
    /// and no distance counted, from the point before it.
    pub new_segment: bool,
}

/// Why a track can't be packed or read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// No points.
    Empty,
    /// More than [`MAX_POINTS`].
    TooMany,
    /// A time, position, elevation or accuracy that isn't real, or times that go backwards.
    BadPoint(&'static str),
    /// The payload is damaged, truncated or from another version.
    Damaged(&'static str),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Empty => f.write_str("track: no points"),
            Error::TooMany => write!(f, "track: more than {MAX_POINTS} points"),
            Error::BadPoint(w) => write!(f, "track: {w}"),
            Error::Damaged(w) => write!(f, "track data: {w}"),
        }
    }
}

impl std::error::Error for Error {}

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

fn unzigzag(v: u64) -> i64 {
    ((v >> 1) as i64) ^ -((v & 1) as i64)
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn u8(&mut self) -> Result<u8, Error> {
        let b = *self.buf.get(self.pos).ok_or(Error::Damaged("truncated"))?;
        self.pos += 1;
        Ok(b)
    }

    /// LEB128, at most 10 bytes (a `u64`).
    fn varint(&mut self) -> Result<u64, Error> {
        let mut v: u64 = 0;
        for i in 0..10u32 {
            let b = self.u8()?;
            let part = u64::from(b & 0x7f).checked_shl(i * 7).ok_or(Error::Damaged("number"))?;
            // the tenth byte may only carry the top bit
            if i == 9 && b > 1 {
                return Err(Error::Damaged("number too large"));
            }
            v |= part;
            if b & 0x80 == 0 {
                return Ok(v);
            }
        }
        Err(Error::Damaged("number too long"))
    }

    fn done(&self) -> bool {
        self.pos == self.buf.len()
    }
}

/// A position as millionths of a degree.
fn micro(deg: f64) -> i64 {
    (deg * POS_UNIT).round() as i64
}

/// Whether `points` can be packed: times in range and not going backwards, real positions, elevations and accuracies.
fn check(points: &[Point]) -> Result<(), Error> {
    if points.is_empty() {
        return Err(Error::Empty);
    }
    if points.len() > MAX_POINTS {
        return Err(Error::TooMany);
    }
    let mut last = 0i64;
    for p in points {
        if !(0..=TIME_MAX_MS).contains(&p.time_ms) {
            return Err(Error::BadPoint("a time is outside the years 1970 to 9999"));
        }
        if p.time_ms < last {
            return Err(Error::BadPoint("times go backwards"));
        }
        last = p.time_ms;
        if !valid(p.lat, p.lon) {
            return Err(Error::BadPoint("a position is not on the globe"));
        }
        if p.ele_m.is_some_and(|e| !e.is_finite() || !(ELE_MIN_M..=ELE_MAX_M).contains(&e)) {
            return Err(Error::BadPoint("an elevation is not real"));
        }
        if p.acc_m.is_some_and(|a| !a.is_finite() || !(0.0..=ACC_MAX_M).contains(&a)) {
            return Err(Error::BadPoint("an accuracy is not real"));
        }
    }
    Ok(())
}

/// `p` as it can be packed: `None` when its time or position isn't real (out of range), else the point with an
/// elevation or accuracy that isn't real left out.
pub fn real(p: &Point) -> Option<Point> {
    ((0..=TIME_MAX_MS).contains(&p.time_ms) && valid(p.lat, p.lon)).then(|| Point {
        ele_m: p.ele_m.filter(|e| e.is_finite() && (ELE_MIN_M..=ELE_MAX_M).contains(e)),
        acc_m: p.acc_m.filter(|a| a.is_finite() && (0.0..=ACC_MAX_M).contains(a)),
        ..*p
    })
}

/// Put points in time order (equal times keep their order), leaving out those that aren't real: a time or position
/// out of range, or an elevation or accuracy that isn't real is dropped from the point, not the point. The count of
/// points left out comes second. At most [`MAX_POINTS`] are kept (the first ones).
pub fn clean(points: &[Point]) -> (Vec<Point>, usize) {
    let mut kept: Vec<Point> = points.iter().filter_map(real).collect();
    kept.sort_by_key(|p| p.time_ms);
    kept.truncate(MAX_POINTS);
    let left_out = points.len() - kept.len();
    (kept, left_out)
}

/// Pack `points` (in time order, see [`clean`]).
pub fn pack(points: &[Point]) -> Result<Vec<u8>, Error> {
    check(points)?;
    let mut raw = vec![FORMAT];
    put_varint(&mut raw, points.len() as u64);
    // times: the first as it is, then the gaps (never negative: checked)
    let mut prev = 0i64;
    for p in points {
        put_varint(&mut raw, (p.time_ms - prev) as u64);
        prev = p.time_ms;
    }
    let axes: [fn(&Point) -> f64; 2] = [|p| p.lat, |p| p.lon];
    for pick in axes {
        let mut prev = 0i64;
        for p in points {
            let v = micro(pick(p));
            put_varint(&mut raw, zigzag(v - prev));
            prev = v;
        }
    }
    for p in points {
        let flags = u8::from(p.new_segment) * FLAG_SEGMENT + u8::from(p.ele_m.is_some()) * FLAG_ELE + u8::from(p.acc_m.is_some()) * FLAG_ACC;
        raw.push(flags);
    }
    let mut prev = 0i64;
    for e in points.iter().filter_map(|p| p.ele_m) {
        let v = (e * DM).round() as i64;
        put_varint(&mut raw, zigzag(v - prev));
        prev = v;
    }
    for a in points.iter().filter_map(|p| p.acc_m) {
        put_varint(&mut raw, (a * DM).round() as u64);
    }
    Ok(compress_to_vec(&raw, 6))
}

/// Read what [`pack`] made. Positions and times come back exactly as stored (positions to a millionth of a degree).
pub fn unpack(bytes: &[u8]) -> Result<Vec<Point>, Error> {
    let raw = decompress_to_vec_with_limit(bytes, MAX_INFLATED).map_err(|_| Error::Damaged("can't decompress"))?;
    let mut r = Reader { buf: &raw, pos: 0 };
    if r.u8()? != FORMAT {
        return Err(Error::Damaged("another format"));
    }
    let n = usize::try_from(r.varint()?).map_err(|_| Error::Damaged("count"))?;
    if n == 0 || n > MAX_POINTS {
        return Err(Error::Damaged("implausible count"));
    }
    // every point takes at least its time, its two positions and its flags
    if n.checked_mul(4).is_none_or(|b| b > raw.len()) {
        return Err(Error::Damaged("implausible count"));
    }
    let mut times = Vec::with_capacity(n);
    let mut t = 0i64;
    for _ in 0..n {
        let gap = i64::try_from(r.varint()?).map_err(|_| Error::Damaged("time"))?;
        t = t.checked_add(gap).filter(|t| *t <= TIME_MAX_MS).ok_or(Error::Damaged("time"))?;
        times.push(t);
    }
    let mut axis = |limit: i64| -> Result<Vec<f64>, Error> {
        let mut out = Vec::with_capacity(n);
        let mut v = 0i64;
        for _ in 0..n {
            v = v.checked_add(unzigzag(r.varint()?)).filter(|v| v.abs() <= limit).ok_or(Error::Damaged("position"))?;
            out.push(v as f64 / POS_UNIT);
        }
        Ok(out)
    };
    let lats = axis(90_000_000)?;
    let lons = axis(180_000_000)?;
    let mut flags = Vec::with_capacity(n);
    for _ in 0..n {
        let f = r.u8()?;
        if f & !(FLAG_SEGMENT | FLAG_ELE | FLAG_ACC) != 0 {
            return Err(Error::Damaged("flags"));
        }
        flags.push(f);
    }
    let mut ele_at = 0i64;
    let mut points = Vec::with_capacity(n);
    for i in 0..n {
        let f = flags.get(i).copied().unwrap_or(0);
        let ele_m = if f & FLAG_ELE != 0 {
            ele_at = ele_at.checked_add(unzigzag(r.varint()?)).ok_or(Error::Damaged("elevation"))?;
            let e = ele_at as f64 / DM;
            if !(ELE_MIN_M..=ELE_MAX_M).contains(&e) {
                return Err(Error::Damaged("elevation"));
            }
            Some(e)
        } else {
            None
        };
        points.push(Point {
            time_ms: times.get(i).copied().unwrap_or(0),
            lat: lats.get(i).copied().unwrap_or(0.0),
            lon: lons.get(i).copied().unwrap_or(0.0),
            ele_m,
            acc_m: None,
            new_segment: f & FLAG_SEGMENT != 0,
        });
    }
    for (p, f) in points.iter_mut().zip(&flags) {
        if f & FLAG_ACC != 0 {
            let a = r.varint()? as f64 / DM;
            if !(0.0..=ACC_MAX_M).contains(&a) {
                return Err(Error::Damaged("accuracy"));
            }
            p.acc_m = Some(a);
        }
    }
    if !r.done() {
        return Err(Error::Damaged("trailing bytes"));
    }
    if points.iter().any(|p| !valid(p.lat, p.lon)) {
        return Err(Error::Damaged("position"));
    }
    Ok(points)
}

/// What a track covers.
#[derive(Clone, Debug, PartialEq)]
pub struct Stats {
    pub points: usize,
    /// Stretches of recording (one more than the number of `new_segment` points after the first).
    pub segments: usize,
    pub distance_km: f64,
    /// Metres climbed and descended (elevation moves under 3 m are not counted).
    pub ascent_m: f64,
    pub descent_m: f64,
    pub start_ms: i64,
    pub end_ms: i64,
    /// `[south, west, north, east]`; west is greater than east when the track crosses the antimeridian.
    pub bbox: [f64; 4],
}

/// Metres climbed and descended along `elevations`: a climb counts once the elevation turns more than
/// [`CLIMB_STEP_M`] back from its peak, a descent likewise from its valley, so the wobble of a GPS elevation adds nothing.
fn climb(elevations: impl Iterator<Item = f64>) -> (f64, f64) {
    enum Heading {
        /// Not yet moved by a step: the range so far.
        Unknown { lo: f64, hi: f64 },
        /// Climbing from `base`, the highest so far `ext`.
        Up { base: f64, ext: f64 },
        /// Descending from `base`, the lowest so far `ext`.
        Down { base: f64, ext: f64 },
    }
    let (mut ascent, mut descent) = (0.0f64, 0.0f64);
    let mut state: Option<Heading> = None;
    for e in elevations {
        state = Some(match state {
            None => Heading::Unknown { lo: e, hi: e },
            Some(Heading::Unknown { lo, hi }) => {
                let (lo, hi) = (lo.min(e), hi.max(e));
                if e - lo >= CLIMB_STEP_M {
                    Heading::Up { base: lo, ext: e }
                } else if hi - e >= CLIMB_STEP_M {
                    Heading::Down { base: hi, ext: e }
                } else {
                    Heading::Unknown { lo, hi }
                }
            }
            Some(Heading::Up { base, ext }) => {
                let ext = ext.max(e);
                if ext - e >= CLIMB_STEP_M {
                    ascent += ext - base;
                    Heading::Down { base: ext, ext: e }
                } else {
                    Heading::Up { base, ext }
                }
            }
            Some(Heading::Down { base, ext }) => {
                let ext = ext.min(e);
                if e - ext >= CLIMB_STEP_M {
                    descent += base - ext;
                    Heading::Up { base: ext, ext: e }
                } else {
                    Heading::Down { base, ext }
                }
            }
        });
    }
    match state {
        Some(Heading::Up { base, ext }) => ascent += ext - base,
        Some(Heading::Down { base, ext }) => descent += base - ext,
        _ => {}
    }
    (ascent, descent)
}

/// What `points` (in time order) cover; `None` for no points.
pub fn stats(points: &[Point]) -> Option<Stats> {
    let first = points.first()?;
    let last = points.last()?;
    let (mut south, mut north) = (first.lat, first.lat);
    // longitudes relative to the first point's, the short way round, so a track over the antimeridian has a small span
    let rel = |lon: f64| (lon - first.lon + 540.0).rem_euclid(360.0) - 180.0;
    let (mut west, mut east) = (0.0f64, 0.0f64);
    let (mut distance, mut segments) = (0.0f64, 1usize);
    let mut prev: Option<&Point> = None;
    for p in points {
        south = south.min(p.lat);
        north = north.max(p.lat);
        let r = rel(p.lon);
        west = west.min(r);
        east = east.max(r);
        if let Some(q) = prev {
            if p.new_segment {
                segments += 1;
            } else {
                distance += distance_km(q.lat, q.lon, p.lat, p.lon);
            }
        }
        prev = Some(p);
    }
    let (ascent, descent) = climb(points.iter().filter_map(|p| p.ele_m));
    let wrap = |lon: f64| (lon + 540.0).rem_euclid(360.0) - 180.0;
    Some(Stats {
        points: points.len(),
        segments,
        distance_km: distance,
        ascent_m: ascent,
        descent_m: descent,
        start_ms: first.time_ms,
        end_ms: last.time_ms,
        bbox: [south, wrap(first.lon + west), north, wrap(first.lon + east)],
    })
}

/// Cut `points` (in time order) into pieces of at most `max_points` points and `max_span_ms` of time, at point
/// boundaries; the pieces are in order and together hold every point. A boundary doesn't mark a new stretch of
/// recording: the first point of the next piece keeps its own flag.
pub fn chunks(points: &[Point], max_points: usize, max_span_ms: i64) -> Vec<Vec<Point>> {
    let max_points = max_points.clamp(1, MAX_POINTS);
    let mut out: Vec<Vec<Point>> = Vec::new();
    let mut cur: Vec<Point> = Vec::new();
    for p in points {
        let full = cur.len() >= max_points || cur.first().is_some_and(|f| p.time_ms.saturating_sub(f.time_ms) >= max_span_ms.max(1));
        if full {
            out.push(std::mem::take(&mut cur));
        }
        cur.push(*p);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// At most `max` (at least 2) of `points` (in time order), for drawing: both ends and the points on both sides of every
/// break between stretches of recording are kept, the rest are picked evenly through the track. A kept point begins a
/// stretch when one began since the point kept before it, so a break is never drawn over. The second value says
/// whether any point was left out.
// ponytail: evenly by index, not by shape (Douglas-Peucker): a long stop keeps as many points as a long walk
pub fn simplify(points: &[Point], max: usize) -> (Vec<Point>, bool) {
    let (n, max) = (points.len(), max.max(2));
    if n <= max {
        return (points.to_vec(), false);
    }
    let mut must: Vec<usize> = vec![0, n - 1];
    for (i, p) in points.iter().enumerate().skip(1) {
        if p.new_segment {
            must.extend([i - 1, i]);
        }
    }
    must.sort_unstable();
    must.dedup();
    let mut keep = vec![false; n];
    // more breaks than room: evenly over everything (the breaks still carry over to the next kept point)
    let (spread, fixed) = if must.len() < max { (max - must.len(), must.as_slice()) } else { (max, &[][..]) };
    for &i in fixed {
        if let Some(k) = keep.get_mut(i) {
            *k = true;
        }
    }
    for k in 0..spread {
        let i = if spread < 2 { 0 } else { k * (n - 1) / (spread - 1) };
        if let Some(k) = keep.get_mut(i) {
            *k = true;
        }
    }
    let mut out = Vec::with_capacity(max);
    let mut broke = false;
    for (p, kept) in points.iter().zip(&keep) {
        broke |= p.new_segment;
        if *kept {
            out.push(Point { new_segment: broke, ..*p });
            broke = false;
        }
    }
    (out, true)
}

/// Leave out the start and the end of a track that lie within `radius_m` metres (on the globe) of its first and its
/// last position, where a recording often begins and ends (a home): the points before the first one farther than that
/// from the first position, and likewise from the end. Answers what is left and how many points went at each end.
pub fn trim(points: &[Point], radius_m: f64) -> (Vec<Point>, usize, usize) {
    let (Some(first), Some(last)) = (points.first(), points.last()) else { return (Vec::new(), 0, 0) };
    let n = points.len();
    let far = |a: &Point, p: &Point| distance_km(a.lat, a.lon, p.lat, p.lon) * 1000.0 > radius_m;
    let start = points.iter().position(|p| far(first, p)).unwrap_or(n);
    let end = points.iter().rev().position(|p| far(last, p)).unwrap_or(n);
    match points.get(start..n.saturating_sub(end)) {
        Some(kept) if !kept.is_empty() => (kept.to_vec(), start, end),
        _ => (Vec::new(), start.min(n), n - start.min(n)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(time_ms: i64, lat: f64, lon: f64) -> Point {
        Point { time_ms, lat, lon, ele_m: None, acc_m: None, new_segment: false }
    }

    /// A walk: a point every 5 seconds, about 7 m apart, with elevations and accuracies.
    fn walk(n: usize) -> Vec<Point> {
        (0..n)
            .map(|i| Point {
                time_ms: 1_700_000_000_000 + i as i64 * 5000,
                lat: 40.4168 + i as f64 * 0.00004,
                lon: -3.7038 + (i as f64 * 0.3).sin() * 0.0002,
                ele_m: Some(650.0 + (i as f64 * 0.05).sin() * 12.0),
                acc_m: Some(4.0 + (i % 7) as f64),
                new_segment: i % 500 == 499,
            })
            .collect()
    }

    fn same(a: &[Point], b: &[Point]) {
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(b) {
            assert_eq!((x.time_ms, x.new_segment), (y.time_ms, y.new_segment));
            assert!((x.lat - y.lat).abs() <= 5.1e-7 && (x.lon - y.lon).abs() <= 5.1e-7, "{x:?} {y:?}");
            assert_eq!(x.ele_m.is_some(), y.ele_m.is_some());
            assert!(x.ele_m.zip(y.ele_m).is_none_or(|(a, b)| (a - b).abs() <= 0.051), "{x:?} {y:?}");
            assert_eq!(x.acc_m.is_some(), y.acc_m.is_some());
            assert!(x.acc_m.zip(y.acc_m).is_none_or(|(a, b)| (a - b).abs() <= 0.051), "{x:?} {y:?}");
        }
    }

    #[test]
    fn a_track_comes_back_as_it_went_in_and_is_small() {
        let points = walk(17_000);
        let packed = pack(&points).unwrap();
        same(&points, &unpack(&packed).unwrap());
        // a 17 000-point day takes about 100 to 170 KB once in base64, so at most ~125 KB packed
        assert!(packed.len() < 17_000 * 8, "{} bytes", packed.len());
        let bare: Vec<Point> = points.iter().map(|p| Point { ele_m: None, acc_m: None, new_segment: false, ..*p }).collect();
        assert!(pack(&bare).unwrap().len() < packed.len(), "no elevations or accuracies: smaller");
        same(&bare, &unpack(&pack(&bare).unwrap()).unwrap());
    }

    #[test]
    fn extremes_round_trip() {
        let points = vec![
            Point { time_ms: 0, lat: -90.0, lon: -180.0, ele_m: Some(-500.0), acc_m: Some(0.0), new_segment: true },
            Point { time_ms: 0, lat: 90.0, lon: 180.0, ele_m: Some(20_000.0), acc_m: Some(100_000.0), new_segment: false },
            pt(TIME_MAX_MS, 0.0, 0.0),
        ];
        same(&points, &unpack(&pack(&points).unwrap()).unwrap());
        same(&points[..1], &unpack(&pack(&points[..1]).unwrap()).unwrap());
    }

    #[test]
    fn what_is_not_real_is_refused() {
        let ok = pt(1000, 10.0, 10.0);
        assert_eq!(pack(&[]), Err(Error::Empty));
        assert_eq!(pack(&vec![ok; MAX_POINTS + 1]), Err(Error::TooMany));
        for bad in [
            pt(-1, 0.0, 0.0),
            pt(TIME_MAX_MS + 1, 0.0, 0.0),
            pt(0, 91.0, 0.0),
            pt(0, f64::NAN, 0.0),
            pt(0, 0.0, 181.0),
            Point { ele_m: Some(f64::NAN), ..ok },
            Point { ele_m: Some(30_000.0), ..ok },
            Point { acc_m: Some(-1.0), ..ok },
            Point { acc_m: Some(f64::INFINITY), ..ok },
        ] {
            assert!(matches!(pack(&[bad]), Err(Error::BadPoint(_))), "{bad:?}");
        }
        assert!(matches!(pack(&[pt(2000, 1.0, 1.0), pt(1000, 1.0, 1.0)]), Err(Error::BadPoint(_))), "times go backwards");
    }

    #[test]
    fn clean_puts_points_in_order_and_drops_what_is_not_real() {
        let input = vec![
            pt(3000, 1.0, 1.0),
            pt(1000, 2.0, 2.0),
            pt(2000, 99.0, 0.0),
            Point { ele_m: Some(f64::NAN), acc_m: Some(-3.0), ..pt(2500, 3.0, 3.0) },
            pt(-5, 0.0, 0.0),
        ];
        let (kept, left_out) = clean(&input);
        assert_eq!(left_out, 2);
        assert_eq!(kept.iter().map(|p| p.time_ms).collect::<Vec<_>>(), vec![1000, 2500, 3000]);
        assert_eq!((kept[1].ele_m, kept[1].acc_m), (None, None), "a bad elevation or accuracy is dropped from its point");
        assert!(pack(&kept).is_ok());
        let many: Vec<Point> = (0..MAX_POINTS as i64 + 10).map(|i| pt(i, 1.0, 1.0)).collect();
        assert_eq!(clean(&many).0.len(), MAX_POINTS);
        assert_eq!(clean(&many).1, 10);
    }

    #[test]
    fn a_damaged_payload_is_an_error() {
        let packed = pack(&walk(50)).unwrap();
        assert!(unpack(&[]).is_err());
        assert!(unpack(b"not deflate at all").is_err());
        assert!(unpack(&packed[..packed.len() / 2]).is_err(), "truncated");
        // a payload that inflates to a huge count with little data behind it
        let mut raw = vec![FORMAT];
        put_varint(&mut raw, MAX_POINTS as u64);
        raw.extend([0u8; 10]);
        assert_eq!(unpack(&compress_to_vec(&raw, 6)), Err(Error::Damaged("implausible count")));
        // a count over the limit, an unknown format, trailing bytes, a position off the globe, a bad flag
        let mut raw = vec![FORMAT];
        put_varint(&mut raw, MAX_POINTS as u64 + 1);
        raw.extend(vec![0u8; MAX_POINTS * 5]);
        assert!(unpack(&compress_to_vec(&raw, 6)).is_err());
        let good = decompress_to_vec_with_limit(&packed, MAX_INFLATED).unwrap();
        let mut other = good.clone();
        other[0] = 9;
        assert_eq!(unpack(&compress_to_vec(&other, 6)), Err(Error::Damaged("another format")));
        let mut longer = good.clone();
        longer.push(0);
        assert_eq!(unpack(&compress_to_vec(&longer, 6)), Err(Error::Damaged("trailing bytes")));
        // one point at latitude 91°, longitude 0 (no elevation, accuracy or segment)
        let mut raw = vec![FORMAT];
        put_varint(&mut raw, 1);
        put_varint(&mut raw, 5);
        put_varint(&mut raw, zigzag(91_000_000));
        put_varint(&mut raw, zigzag(0));
        raw.push(0);
        assert_eq!(unpack(&compress_to_vec(&raw, 6)), Err(Error::Damaged("position")));
        // the same point on the globe, with a flag that doesn't exist
        let one = |flags: u8| {
            let mut raw = vec![FORMAT];
            put_varint(&mut raw, 1);
            put_varint(&mut raw, 5);
            put_varint(&mut raw, zigzag(10_000_000));
            put_varint(&mut raw, zigzag(20_000_000));
            raw.push(flags);
            compress_to_vec(&raw, 6)
        };
        assert_eq!(unpack(&one(0)).map(|p| p.len()), Ok(1));
        assert_eq!(unpack(&one(8)), Err(Error::Damaged("flags")));
        // an overlong number
        let mut raw = vec![FORMAT];
        raw.extend([0xff; 12]);
        assert!(unpack(&compress_to_vec(&raw, 6)).is_err());
        // a deflate bomb: far more than the limit
        let bomb = compress_to_vec(&vec![0u8; MAX_INFLATED + 1], 9);
        assert_eq!(unpack(&bomb), Err(Error::Damaged("can't decompress")));
    }

    #[test]
    fn stats_say_how_far_how_long_and_where() {
        // 1000 points due north, 11.1 m apart, 5 s each, a climb of 20 m then a descent of 20 m
        let points: Vec<Point> = (0..1000)
            .map(|i| Point {
                ele_m: Some(if i < 500 { i as f64 * 0.04 } else { 20.0 - (i - 500) as f64 * 0.04 }),
                ..pt(1_000_000 + i * 5000, 10.0 + i as f64 * 0.0001, 20.0)
            })
            .collect();
        let s = stats(&points).unwrap();
        assert_eq!((s.points, s.segments, s.start_ms, s.end_ms), (1000, 1, 1_000_000, 1_000_000 + 999 * 5000));
        assert!((s.distance_km - 11.1).abs() < 0.2, "{}", s.distance_km);
        assert!((s.ascent_m - 20.0).abs() < 0.01 && (s.descent_m - 19.96).abs() < 0.01, "{} {}", s.ascent_m, s.descent_m);
        // up, down, up again: each climb counted once, the dip in between counted as a descent
        let hills: Vec<f64> = vec![0.0, 5.0, 10.0, 6.0, 2.0, 7.0, 12.0];
        assert_eq!(climb(hills.iter().copied()), (10.0 + 10.0, 8.0));
        assert_eq!(climb(std::iter::empty()), (0.0, 0.0));
        assert_eq!(climb([5.0].into_iter()), (0.0, 0.0));
        assert_eq!(climb([10.0, 7.0].into_iter()), (0.0, 3.0), "a first move down");
        assert_eq!(s.bbox, [10.0, 20.0, 10.0 + 999.0 * 0.0001, 20.0]);
        // a wobble of a metre is not a climb
        let wobble: Vec<Point> = (0..100).map(|i| Point { ele_m: Some(100.0 + (i % 2) as f64), ..pt(i * 1000, 0.0, i as f64 * 0.001) }).collect();
        assert_eq!(stats(&wobble).unwrap().ascent_m, 0.0);
        assert!(stats(&[]).is_none());
    }

    #[test]
    fn a_new_segment_counts_no_distance_and_a_track_over_the_antimeridian_has_a_small_box() {
        let points = vec![pt(0, 0.0, 179.0), pt(1000, 0.0, 179.5), Point { new_segment: true, ..pt(2000, 0.0, -179.5) }, pt(3000, 0.0, -179.0)];
        let s = stats(&points).unwrap();
        assert_eq!(s.segments, 2);
        // 0.5° twice, not the jump between
        assert!((s.distance_km - 2.0 * 0.5 * 111.19).abs() < 1.0, "{}", s.distance_km);
        assert_eq!((s.bbox[1], s.bbox[3]), (179.0, -179.0), "west is east of east: it crosses");
    }

    #[test]
    fn a_long_recording_is_cut_where_it_must_be() {
        let points: Vec<Point> = (0..25).map(|i| pt(i * 1000, 0.0, 0.0)).collect();
        let by_count = chunks(&points, 10, CHUNK_SPAN_MS);
        assert_eq!(by_count.iter().map(Vec::len).collect::<Vec<_>>(), vec![10, 10, 5]);
        let by_time = chunks(&points, 100, 8000);
        assert_eq!(by_time.iter().map(Vec::len).collect::<Vec<_>>(), vec![8, 8, 8, 1]);
        assert_eq!(by_time.concat(), points, "every point, in order");
        assert!(chunks(&[], 10, 10).is_empty());
        assert_eq!(chunks(&points, 0, 0).len(), 25, "limits of nothing mean one point each, not a hang");
        let day = chunks(&walk(30_000), CHUNK_POINTS, CHUNK_SPAN_MS);
        assert_eq!(day.iter().map(Vec::len).collect::<Vec<_>>(), vec![17_280, 12_720], "a day is 17 280 points at 5 s");
    }

    #[test]
    fn simplify_keeps_the_ends_and_both_sides_of_every_break() {
        let mut points: Vec<Point> = (0..1000).map(|i| pt(i * 1000, 0.0, i as f64 * 0.001)).collect();
        points[400].new_segment = true;
        points[401].new_segment = true;
        let (few, simplified) = simplify(&points, 50);
        assert!(simplified && few.len() <= 50 && few.len() >= 45, "{}", few.len());
        assert_eq!((few[0].time_ms, few.last().unwrap().time_ms), (0, 999_000), "both ends");
        for t in [399_000, 400_000, 401_000] {
            assert!(few.iter().any(|p| p.time_ms == t), "{t} kept");
        }
        assert_eq!(few.iter().filter(|p| p.new_segment).map(|p| p.time_ms).collect::<Vec<_>>(), vec![400_000, 401_000]);
        assert!(few.windows(2).all(|w| w[0].time_ms < w[1].time_ms));
        assert_eq!(simplify(&points[..10], 50), (points[..10].to_vec(), false), "small enough: as it is");
        // little room: the point before a break is kept for the break's sake, not by chance
        let mut short: Vec<Point> = (0..100).map(|i| pt(i * 1000, 0.0, 0.0)).collect();
        short[50].new_segment = true;
        assert_eq!(simplify(&short, 5).0.iter().map(|p| p.time_ms).collect::<Vec<_>>(), vec![0, 49_000, 50_000, 99_000]);
        assert_eq!(simplify(&points, 0).0.len(), 2, "at least the two ends");
        // more breaks than room: a break left out still begins the next kept point's stretch
        let broken: Vec<Point> = (0..100).map(|i| Point { new_segment: i % 2 == 1, ..pt(i * 1000, 0.0, 0.0) }).collect();
        let (few, _) = simplify(&broken, 10);
        assert_eq!(few.len(), 10);
        assert!(few.iter().skip(1).all(|p| p.new_segment), "{few:?}");
    }

    #[test]
    fn trim_cuts_what_lies_near_the_first_and_last_positions() {
        // out from 0,0 due east 1 km at 100 m a step, then on to 2 km, then back to 1.5 km and ending there
        let mut points: Vec<Point> = (0..=20).map(|i| pt(i * 1000, 0.0, i as f64 * 0.1 / 111.195)).collect();
        points.extend((1..=5).map(|i| pt((20 + i) * 1000, 0.0, (2.0 - i as f64 * 0.1) / 111.195)));
        let (kept, start, end) = trim(&points, 250.0);
        assert_eq!(start, 3, "0, 100 and 200 m from the start go");
        assert_eq!(end, 3, "the last point and the two within 250 m of it go");
        assert_eq!(kept.len(), points.len() - 6);
        assert_eq!(kept[0].time_ms, 3000);
        // a radius that takes everything: nothing left, every point counted once
        let (kept, start, end) = trim(&points, 50_000.0);
        assert!(kept.is_empty());
        assert_eq!(start + end, points.len());
        assert_eq!(trim(&points, 0.0).0, points[1..points.len() - 1].to_vec(), "only the end points themselves");
        assert_eq!(trim(&[], 100.0), (vec![], 0, 0));
    }

    #[test]
    fn real_drops_what_cannot_be_packed() {
        assert!(real(&pt(-1, 0.0, 0.0)).is_none());
        assert!(real(&pt(0, 0.0, 200.0)).is_none());
        let p = real(&Point { ele_m: Some(f64::NAN), acc_m: Some(5.0), ..pt(0, 1.0, 2.0) }).unwrap();
        assert_eq!((p.ele_m, p.acc_m), (None, Some(5.0)));
    }

    proptest::proptest! {
        #[test]
        fn any_valid_track_round_trips(raw in proptest::collection::vec((0i64..100_000, -90.0f64..=90.0, -180.0f64..=180.0, proptest::option::of(-400.0f64..9000.0), proptest::option::of(0.0f64..500.0), proptest::bool::ANY), 1..300)) {
            let mut t = 1_000_000_000_000i64;
            let points: Vec<Point> = raw.iter().map(|&(gap, lat, lon, ele_m, acc_m, new_segment)| { t += gap; Point { time_ms: t, lat, lon, ele_m, acc_m, new_segment } }).collect();
            let back = unpack(&pack(&points).unwrap()).unwrap();
            proptest::prop_assert_eq!(back.len(), points.len());
            for (a, b) in points.iter().zip(&back) {
                proptest::prop_assert_eq!((a.time_ms, a.new_segment), (b.time_ms, b.new_segment));
                proptest::prop_assert!((a.lat - b.lat).abs() <= 5.1e-7 && (a.lon - b.lon).abs() <= 5.1e-7);
                proptest::prop_assert_eq!((a.ele_m.is_some(), a.acc_m.is_some()), (b.ele_m.is_some(), b.acc_m.is_some()));
            }
            let s = stats(&back).unwrap();
            proptest::prop_assert!(s.distance_km.is_finite() && s.ascent_m >= 0.0 && s.descent_m >= 0.0 && s.start_ms <= s.end_ms);
            proptest::prop_assert!(s.bbox.iter().all(|v| v.is_finite()) && s.bbox[0] <= s.bbox[2]);
        }

        #[test]
        fn arbitrary_bytes_never_panic_and_never_give_a_bad_point(bytes in proptest::collection::vec(proptest::num::u8::ANY, 0..400), deflate in proptest::bool::ANY) {
            let data = if deflate { compress_to_vec(&bytes, 6) } else { bytes };
            if let Ok(points) = unpack(&data) {
                proptest::prop_assert!(!points.is_empty() && points.len() <= MAX_POINTS);
                proptest::prop_assert!(points.iter().all(|p| valid(p.lat, p.lon) && p.time_ms >= 0));
                proptest::prop_assert!(points.windows(2).all(|w| w[0].time_ms <= w[1].time_ms));
            }
        }

        #[test]
        fn a_flipped_byte_is_an_error_or_a_real_track(flip in 0usize..2000, bit in 0u8..8) {
            let mut raw = decompress_to_vec_with_limit(&pack(&walk(100)).unwrap(), MAX_INFLATED).unwrap();
            let at = flip % raw.len();
            raw[at] ^= 1 << bit;
            if let Ok(points) = unpack(&compress_to_vec(&raw, 6)) {
                proptest::prop_assert!(points.iter().all(|p| valid(p.lat, p.lon)) && points.windows(2).all(|w| w[0].time_ms <= w[1].time_ms));
            }
        }
    }
}
