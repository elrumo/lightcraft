//! One path for a selection of photos: **recorded** where a stored track covers the time, **estimated** elsewhere
//! ([`route`]), with the stops, the photos placed along it and what it all adds up to. Pure maths on positions and
//! times (seconds since 1970, UTC); the engine reads the tracks and the photos.
//!
//! **Recorded pieces.** A track (one recording, its points in time order) is cut into pieces wherever two points are
//! more than [`route::Params::gap_s`] apart (no line across the days between), and where an earlier track already
//! covers the time: tracks are taken in the order given (by their start), the first one to cover a moment keeps it,
//! so two recordings of the same walk (a phone and a watch) never draw it twice. A break in the recording
//! ([`Point::new_segment`]) stays inside its piece as a new line: nothing is drawn across it, and its time is neither
//! moving nor stopped. A piece's distance, climb and stops come from every point; its lines are simplified to
//! [`Params::max_points`] by shape ([`simplify_shape`]), and all the recorded lines together hold at most
//! [`RECORDED_MAX`] points.
//!
//! **Estimated legs** fill only the time no recorded piece covers: [`route::estimate_runs`] through the photos taken
//! between two recorded pieces (or before the first, after the last), each run starting at the end of the piece
//! before it and ending at the start of the piece after it, so the dashed line meets the recorded one. A recorded
//! piece therefore replaces the legs over its time: a photo taken while a track was recording is on the track.
//!
//! **Stops:** for estimated parts, [`route`]'s stops that hold photos. For recorded parts, a place where the track
//! stays within the stop radius of where the stay began (its running centre) for [`Params::dwell_s`] at least
//! (10 minutes by default: a red light or a photo is not a stop, lunch is).
//!
//! **Photos** taken while recording are on that recording (`at`: the fraction of its time); a photo at an estimated
//! stop is on the leg arriving there or the one leaving (or the recording the stop joins), whichever is nearer in time;
//! a photo whose stop is on no piece (a route of a single stop) is on none.
//!
//! **Privacy trim** ([`Params::trim_m`]): each track loses its start and end within the radius of its first and last
//! position ([`track::trim`]); each estimated route likewise, at the ends that are photos (an end that meets a
//! recorded piece is not a home). The photos taken in what was cut go too. A track or route that never leaves the
//! radius vanishes. Only the answer changes, never what is stored.
//!
//! Everything is bounded: points that aren't real are left out, the answer's lines are capped as [`route`]'s are,
//! and the recorded pieces at [`PIECES_MAX`].

use std::collections::{BinaryHeap, HashMap};

use crate::geodesy::{distance_km, valid};
use crate::route::{self, Fix, Mode};
use crate::track::{self, Point};

/// A recorded stop lasts this long at least (seconds) by default.
pub const DWELL_S: f64 = 600.0;
/// A recorded piece's lines hold at most this many points by default…
pub const MAX_POINTS: usize = 4000;
/// …and this many when asked for more (and at least two).
pub const MAX_POINTS_LIMIT: usize = 20_000;
/// All the recorded lines of one answer hold at most this many points.
pub const RECORDED_MAX: usize = 60_000;
/// One answer holds at most this many recorded pieces.
pub const PIECES_MAX: usize = 5000;

/// What the path is worked out with; each value is kept to a sane range by [`build`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    /// The estimate's parameters; its stop radius and gap also cut and group the recorded pieces.
    pub route: route::Params,
    /// A recorded stop lasts this long at least (seconds).
    pub dwell_s: f64,
    /// The most points of one recorded piece's lines.
    pub max_points: usize,
    /// The privacy trim's radius in metres, `None`: off.
    pub trim_m: Option<f64>,
    /// Only what lies in this window (seconds; either end open).
    pub from: Option<f64>,
    pub to: Option<f64>,
}

impl Default for Params {
    fn default() -> Self {
        Params { route: route::Params::default(), dwell_s: DWELL_S, max_points: MAX_POINTS, trim_m: None, from: None, to: None }
    }
}

/// What kind of piece, and what only that kind has.
#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    /// Where a track was recorded: `track` is its index among the tracks given; `times` parallel `lines`.
    Recorded { track: usize, ascent_m: f64, descent_m: f64, times: Vec<Vec<f64>> },
    /// An estimated leg; `from` and `to` index [`Path::stops`] (`None`: that end meets a recorded piece).
    Estimated { from: Option<usize>, to: Option<usize>, speed_kmh: f64, mode: Mode, arc: bool },
}

/// A stretch of the path.
#[derive(Clone, Debug, PartialEq)]
pub struct Piece {
    pub start: f64,
    pub end: f64,
    pub distance_km: f64,
    /// `end - start`.
    pub duration_s: f64,
    /// The time not in a stop (nor, recorded, in a break of the recording).
    pub moving_s: f64,
    /// `(lat, lon)`; more than one where the recording broke off or the line crosses the antimeridian.
    pub lines: Vec<Vec<(f64, f64)>>,
    pub kind: Kind,
}

impl Piece {
    pub fn recorded(&self) -> bool {
        matches!(self.kind, Kind::Recorded { .. })
    }
}

/// A photo on the path.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    pub id: u64,
    pub t: f64,
    pub lat: f64,
    pub lon: f64,
    /// Index into [`Path::pieces`], `None` when there are none.
    pub piece: Option<usize>,
    /// `0..=1`: how far through that piece's time.
    pub at: f64,
}

/// What the privacy trim took.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Trimmed {
    pub radius_m: f64,
    /// Photos left out near where a track or route began…
    pub start: usize,
    /// …and near where one ended.
    pub end: usize,
    /// Tracks and routes that never left the radius: gone whole (their photos count in `start`).
    pub vanished: usize,
    /// Positions left out along the tracks (their points) and the estimated routes (their stops and line points).
    pub points: usize,
}

/// The answer of [`build`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Path {
    /// In time order, never overlapping in time.
    pub pieces: Vec<Piece>,
    /// In time order.
    pub stops: Vec<route::Stop>,
    /// In time order.
    pub photos: Vec<Placed>,
    pub trim: Option<Trimmed>,
    /// Photos left out: a time or a position that isn't real.
    pub skipped: usize,
}

/// What a path adds up to: each figure the sum of its pieces'.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stats {
    pub distance_km: f64,
    pub recorded_km: f64,
    pub estimated_km: f64,
    pub duration_s: f64,
    pub moving_s: f64,
    pub ascent_m: f64,
    pub descent_m: f64,
    pub photos: usize,
    pub stops: usize,
}

impl Path {
    pub fn stats(&self) -> Stats {
        let mut s = Stats { photos: self.photos.len(), stops: self.stops.len(), ..Stats::default() };
        for p in &self.pieces {
            s.distance_km += p.distance_km;
            s.duration_s += p.duration_s;
            s.moving_s += p.moving_s;
            match &p.kind {
                Kind::Recorded { ascent_m, descent_m, .. } => {
                    s.recorded_km += p.distance_km;
                    s.ascent_m += ascent_m;
                    s.descent_m += descent_m;
                }
                Kind::Estimated { .. } => s.estimated_km += p.distance_km,
            }
        }
        s
    }

    /// The first and last moment of the pieces, stops and photos; `None` when there is nothing.
    pub fn range(&self) -> Option<(f64, f64)> {
        let starts = self.pieces.iter().map(|p| p.start).chain(self.stops.iter().map(|s| s.arrive)).chain(self.photos.iter().map(|p| p.t));
        let ends = self.pieces.iter().map(|p| p.end).chain(self.stops.iter().map(|s| s.leave)).chain(self.photos.iter().map(|p| p.t));
        Some((starts.reduce(f64::min)?, ends.reduce(f64::max)?))
    }
}

/// Too many recorded pieces for one answer (their number).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TooManyPieces(pub usize);

fn secs(p: &Point) -> f64 {
    p.time_ms as f64 / 1000.0
}

/// A longitude difference the short way round.
fn wrap(d: f64) -> f64 {
    (d + 540.0).rem_euclid(360.0) - 180.0
}

// ---- simplification ------------------------------------------------------------------------------------------------

/// A point's weight in [`simplify_shape`]: the area of the triangle it makes with its neighbours (smaller goes first).
#[derive(PartialEq)]
struct Area(f64, usize);

impl Eq for Area {}

impl Ord for Area {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        // a max-heap: the smallest area must come out first
        o.0.total_cmp(&self.0).then(o.1.cmp(&self.1))
    }
}

impl PartialOrd for Area {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}

/// At most `max` (at least 2) of `points` (in time order) for drawing, chosen by shape (Visvalingam-Whyatt: the
/// point whose triangle with its neighbours is smallest goes first, so a stay of an hour gives up its points before a
/// bend does): both ends and both sides of every break between stretches are kept. When the breaks alone need more
/// than `max`, [`track::simplify`] decides. `O(n log n)` whatever the shape.
pub fn simplify_shape(points: &[Point], max: usize) -> Vec<Point> {
    let (n, max) = (points.len(), max.max(2));
    if n <= max {
        return points.to_vec();
    }
    let fixed: Vec<bool> =
        points.iter().enumerate().map(|(i, p)| i == 0 || i + 1 == n || p.new_segment || points.get(i + 1).is_some_and(|q| q.new_segment)).collect();
    if fixed.iter().filter(|f| **f).count() >= max {
        return track::simplify(points, max).0;
    }
    // a flat local picture: degrees of latitude and of longitude scaled to the latitude, the short way round
    let area = |a: usize, b: usize, c: usize| -> f64 {
        let (Some(pa), Some(pb), Some(pc)) = (points.get(a), points.get(b), points.get(c)) else { return f64::INFINITY };
        let k = pb.lat.to_radians().cos();
        let (ax, ay) = (wrap(pa.lon - pb.lon) * k, pa.lat - pb.lat);
        let (cx, cy) = (wrap(pc.lon - pb.lon) * k, pc.lat - pb.lat);
        (ax * cy - ay * cx).abs() / 2.0
    };
    let mut prev: Vec<usize> = (0..n).map(|i| i.saturating_sub(1)).collect();
    let mut next: Vec<usize> = (0..n).map(|i| (i + 1).min(n - 1)).collect();
    let mut current = vec![f64::INFINITY; n];
    let mut heap = BinaryHeap::with_capacity(n);
    for i in 1..n - 1 {
        if !fixed.get(i).copied().unwrap_or(true) {
            let a = area(i - 1, i, i + 1);
            if let Some(c) = current.get_mut(i) {
                *c = a;
            }
            heap.push(Area(a, i));
        }
    }
    let mut removed = vec![false; n];
    let mut left = n;
    while left > max {
        let Some(Area(a, i)) = heap.pop() else { break };
        // (an entry another took the place of since)
        if removed.get(i).copied().unwrap_or(true) || current.get(i).map(|c| c.to_bits()) != Some(a.to_bits()) {
            continue;
        }
        if let Some(r) = removed.get_mut(i) {
            *r = true;
        }
        left -= 1;
        let (pi, ni) = (prev.get(i).copied().unwrap_or(0), next.get(i).copied().unwrap_or(0));
        if let Some(x) = next.get_mut(pi) {
            *x = ni;
        }
        if let Some(x) = prev.get_mut(ni) {
            *x = pi;
        }
        for j in [pi, ni] {
            if fixed.get(j).copied().unwrap_or(true) {
                continue;
            }
            let (pj, nj) = (prev.get(j).copied().unwrap_or(0), next.get(j).copied().unwrap_or(0));
            // never smaller than the point just taken: what it stood for doesn't come back cheaper
            let w = area(pj, j, nj).max(a);
            if let Some(c) = current.get_mut(j) {
                *c = w;
            }
            heap.push(Area(w, j));
        }
    }
    points.iter().zip(&removed).filter(|(_, r)| !**r).map(|(p, _)| *p).collect()
}

// ---- recorded pieces -----------------------------------------------------------------------------------------------

/// A recorded piece being put together: its figures from every point, its lines from a few.
struct Rec {
    track: usize,
    start: f64,
    end: f64,
    distance_km: f64,
    ascent_m: f64,
    descent_m: f64,
    moving_s: f64,
    /// Simplified: the ends are the piece's own.
    points: Vec<Point>,
    stops: Vec<route::Stop>,
}

/// Consecutive points within `radius_km` of where they gather (their running centre) are one stay; a stay of
/// `dwell_s` at least is a stop. Answers the stops and, for each point, the stop it is in.
fn stays(points: &[Point], radius_km: f64, dwell_s: f64) -> (Vec<route::Stop>, Vec<Option<usize>>) {
    let mut stops = Vec::new();
    let mut of = vec![None; points.len()];
    let mut i = 0;
    while i < points.len() {
        let Some(first) = points.get(i) else { break };
        let (mut lat_sum, mut dlon_sum, mut n) = (first.lat, 0.0, 1.0);
        let mut j = i + 1;
        while let Some(q) = points.get(j) {
            let centre = (lat_sum / n, wrap(first.lon + dlon_sum / n));
            if distance_km(centre.0, centre.1, q.lat, q.lon) > radius_km {
                break;
            }
            lat_sum += q.lat;
            dlon_sum += wrap(q.lon - first.lon);
            n += 1.0;
            j += 1;
        }
        let last = points.get(j - 1).unwrap_or(first);
        if secs(last) - secs(first) >= dwell_s && j - i >= 2 {
            for o in of.get_mut(i..j).into_iter().flatten() {
                *o = Some(stops.len());
            }
            stops.push(route::Stop {
                lat: lat_sum / n,
                lon: wrap(first.lon + dlon_sum / n),
                arrive: secs(first),
                leave: secs(last),
                photos: Vec::new(),
            });
        }
        i = j;
    }
    (stops, of)
}

/// A piece of track `track` made of `run` (at least two points, in time order).
fn rec_of(track: usize, run: &[Point], p: &Params) -> Option<Rec> {
    let s = track::stats(run)?;
    let (stops, of) = stays(run, p.route.stop_radius_m / 1000.0, p.dwell_s);
    let moving_s = run
        .windows(2)
        .zip(of.windows(2))
        .filter(|(w, o)| !w.get(1).is_some_and(|q| q.new_segment) && !matches!(o, [Some(a), Some(b)] if a == b))
        .map(|(w, _)| match w {
            [a, b] => (secs(b) - secs(a)).max(0.0),
            _ => 0.0,
        })
        .sum();
    Some(Rec {
        track,
        start: s.start_ms as f64 / 1000.0,
        end: s.end_ms as f64 / 1000.0,
        distance_km: s.distance_km,
        ascent_m: s.ascent_m,
        descent_m: s.descent_m,
        moving_s,
        points: simplify_shape(run, p.max_points),
        stops,
    })
}

/// Whether `t` lies in one of `spans` (sorted, not overlapping).
fn inside(spans: &[(f64, f64)], t: f64) -> bool {
    let i = spans.partition_point(|s| s.1 < t);
    spans.get(i).is_some_and(|s| s.0 <= t)
}

/// Add `(a, b)` to `spans` (sorted, not overlapping), merged with those it touches.
fn add_span(spans: &mut Vec<(f64, f64)>, a: f64, b: f64) {
    let lo = spans.partition_point(|s| s.1 < a);
    let hi = spans.partition_point(|s| s.0 <= b).max(lo);
    let merged = spans.get(lo..hi).unwrap_or_default().iter().fold((a, b), |(x, y), s| (x.min(s.0), y.max(s.1)));
    spans.splice(lo..hi, [merged]);
}

/// Times whose photos the trim takes, and at which end.
struct Cut {
    from: f64,
    to: f64,
    /// `from` itself is cut (the start of a track; an end cut leaves its own first moment).
    from_in: bool,
    to_in: bool,
    end: bool,
}

impl Cut {
    fn holds(&self, t: f64) -> bool {
        (t > self.from || (self.from_in && t == self.from)) && (t < self.to || (self.to_in && t == self.to))
    }
}

/// The pieces of one track: trimmed, kept to the window, and cut where it pauses too long or where `covered`
/// (what tracks before it took) already holds the time.
fn pieces_of(track: usize, points: Vec<Point>, p: &Params, covered: &[(f64, f64)], cuts: &mut Vec<Cut>, trim: &mut Trimmed) -> Vec<Rec> {
    let mut pts: Vec<Point> = points.iter().filter_map(track::real).collect();
    pts.sort_by_key(|q| q.time_ms);
    if let (Some(r), Some(first), Some(last)) = (p.trim_m, pts.first().copied(), pts.last().copied()) {
        let (kept, a, b) = track::trim(&pts, r);
        trim.points += a + b;
        match (kept.first(), kept.last()) {
            (Some(kf), Some(kl)) => {
                if a > 0 {
                    cuts.push(Cut { from: secs(&first), to: secs(kf), from_in: true, to_in: false, end: false });
                }
                if b > 0 {
                    cuts.push(Cut { from: secs(kl), to: secs(&last), from_in: false, to_in: true, end: true });
                }
            }
            _ => {
                cuts.push(Cut { from: secs(&first), to: secs(&last), from_in: true, to_in: true, end: false });
                trim.vanished += 1;
            }
        }
        pts = kept;
    }
    let keep = |q: &Point| p.from.is_none_or(|f| secs(q) >= f) && p.to.is_none_or(|t| secs(q) <= t) && !inside(covered, secs(q));
    let mut out = Vec::new();
    let mut run: Vec<Point> = Vec::new();
    let mut flush = |run: &mut Vec<Point>| {
        if run.len() >= 2
            && let Some(r) = rec_of(track, run, p)
        {
            out.push(r);
        }
        run.clear();
    };
    for q in &pts {
        if !keep(q) {
            flush(&mut run);
            continue;
        }
        if run.last().is_some_and(|l| secs(q) - secs(l) > p.route.gap_s) {
            flush(&mut run);
        }
        // the first point of a piece begins its first line whatever it says
        run.push(Point { new_segment: q.new_segment && !run.is_empty(), ..*q });
    }
    flush(&mut run);
    out
}

// ---- estimated legs ------------------------------------------------------------------------------------------------

/// An estimated leg kept for the answer: its route, its stops (indices into that route's stops), its time.
struct Leg {
    route: usize,
    from: usize,
    to: usize,
    start: f64,
    end: f64,
    leg: route::Leg,
}

/// The length of `lines` (km).
fn line_km(lines: &[Vec<(f64, f64)>]) -> f64 {
    lines.iter().flat_map(|l| l.windows(2)).map(|w| if let [a, b] = w { distance_km(a.0, a.1, b.0, b.1) } else { 0.0 }).sum()
}

/// Route `r`'s legs and stops as kept after the privacy trim (`radius_m`) of its start (when `trim_start`) and its end
/// (when `trim_end`): along its positions in order (its first stop, then each leg's line) what comes before the first
/// one farther than the radius from the first position goes, and likewise from the end, as [`track::trim`] does.
/// A leg cut into keeps its part outside, its distance and time in proportion. Answers the legs and, per stop, whether
/// it stays.
fn trim_route(index: usize, r: &route::Route, radius_m: Option<f64>, trim_start: bool, trim_end: bool) -> (Vec<Leg>, Vec<bool>, usize) {
    let mut seq: Vec<(f64, f64)> = r.stops.first().map(|s| (s.lat, s.lon)).into_iter().collect();
    let mut ranges: Vec<(usize, usize)> = Vec::with_capacity(r.legs.len());
    for leg in &r.legs {
        let s = seq.len();
        seq.extend(leg.lines.iter().flatten());
        ranges.push((s, seq.len().saturating_sub(1)));
    }
    let n = seq.len();
    let (mut ka, mut kb) = (0, n);
    if let (Some(radius), Some(&first), Some(&last)) = (radius_m, seq.first(), seq.last()) {
        let far = |o: (f64, f64), q: &(f64, f64)| distance_km(o.0, o.1, q.0, q.1) * 1000.0 > radius;
        if trim_start {
            ka = seq.iter().position(|q| far(first, q)).unwrap_or(n);
        }
        if trim_end {
            kb = n - seq.iter().rev().position(|q| far(last, q)).unwrap_or(n);
        }
    }
    let kept = |i: usize| ka <= i && i < kb;
    let cut = if ka >= kb { n } else { ka + (n - kb) };
    // a stop is where its legs meet (and the first stop is the first position)
    let stops_kept: Vec<bool> = (0..r.stops.len())
        .map(|i| {
            let before = i.checked_sub(1).and_then(|k| ranges.get(k)).map(|r| r.1);
            let after = ranges.get(i).map(|r| r.0);
            (i == 0 && kept(0)) || before.is_some_and(kept) || after.is_some_and(kept)
        })
        .collect();
    let mut legs = Vec::new();
    for (leg, &(s, e)) in r.legs.iter().zip(&ranges) {
        let (from, to) = (s.max(ka), e.min(kb.saturating_sub(1)));
        let (Some(a), Some(b)) = (r.stops.get(leg.from), r.stops.get(leg.to)) else { continue };
        if to <= from {
            continue;
        }
        let span = e.saturating_sub(s).max(1) as f64;
        let time = |i: usize| a.leave + (b.arrive - a.leave) * i.saturating_sub(s) as f64 / span;
        let mut out = leg.clone();
        if from > s || to < e {
            let full = line_km(&leg.lines);
            let mut idx = s;
            out.lines = leg
                .lines
                .iter()
                .map(|line| {
                    let part: Vec<(f64, f64)> = line.iter().enumerate().filter(|(j, _)| (from..=to).contains(&(idx + j))).map(|(_, q)| *q).collect();
                    idx += line.len();
                    part
                })
                .filter(|l| l.len() >= 2)
                .collect();
            if full > 0.0 {
                out.distance_km = leg.distance_km * line_km(&out.lines) / full;
            }
        }
        out.duration_s = (time(to) - time(from)).max(0.0);
        legs.push(Leg { route: index, from: leg.from, to: leg.to, start: time(from), end: time(to), leg: out });
    }
    (legs, stops_kept, cut)
}

// ---- the path ------------------------------------------------------------------------------------------------------

impl Params {
    fn sane(&self) -> Params {
        let fin = |v: Option<f64>| v.filter(|v| v.is_finite());
        Params {
            route: self.route.sane(),
            dwell_s: if self.dwell_s.is_finite() { self.dwell_s.clamp(0.0, 1e9) } else { DWELL_S },
            max_points: self.max_points.clamp(2, MAX_POINTS_LIMIT),
            trim_m: fin(self.trim_m).map(|r| r.max(0.0)),
            from: fin(self.from),
            to: fin(self.to),
        }
    }
}

/// How far through piece `q`'s time `t` is: 0 before it, 1 after it.
fn fraction(q: &Piece, t: f64) -> f64 {
    let f = (t - q.start) / (q.end - q.start);
    if f.is_finite() {
        f.clamp(0.0, 1.0)
    } else if t > q.start {
        1.0
    } else {
        0.0
    }
}

/// Of the pieces `candidates` names, the one nearest `t` in time (one whose time holds it first; a tie goes to the later).
fn nearest(pieces: &[Piece], candidates: impl IntoIterator<Item = usize>, t: f64) -> Option<usize> {
    let off = |q: &Piece| if t < q.start { q.start - t } else { (t - q.end).max(0.0) };
    candidates
        .into_iter()
        .filter_map(|k| pieces.get(k).map(|q| (k, off(q), q.start)))
        .min_by(|a, b| a.1.total_cmp(&b.1).then(b.2.total_cmp(&a.2)))
        .map(|(k, _, _)| k)
}

/// The path through `fixes` (the photos with a position and a time) and `tracks` (each one recording's points, in the
/// order of their start: the first to cover a moment keeps it). A track is taken when its turn comes, so only one is
/// held whole at a time.
pub fn build(fixes: &[Fix], tracks: impl IntoIterator<Item = Vec<Point>>, params: &Params) -> Result<Path, TooManyPieces> {
    let p = params.sane();
    let mut trim = Trimmed { radius_m: p.trim_m.unwrap_or(0.0), ..Trimmed::default() };
    let mut cuts: Vec<Cut> = Vec::new();
    // recorded pieces, one track at a time
    let mut covered: Vec<(f64, f64)> = Vec::new();
    let mut recs: Vec<Rec> = Vec::new();
    for (i, points) in tracks.into_iter().enumerate() {
        let made = pieces_of(i, points, &p, &covered, &mut cuts, &mut trim);
        for r in &made {
            add_span(&mut covered, r.start, r.end);
        }
        recs.extend(made);
        if recs.len() > PIECES_MAX {
            return Err(TooManyPieces(recs.len()));
        }
    }
    recs.sort_by(|a, b| a.start.total_cmp(&b.start));
    let total: usize = recs.iter().map(|r| r.points.len()).sum();
    if total > RECORDED_MAX {
        let each = (RECORDED_MAX / recs.len().max(1)).max(2);
        for r in &mut recs {
            r.points = simplify_shape(&r.points, each);
        }
    }
    // the photos: real, in the window, not in what the trim cut off a track
    let mut fx: Vec<Fix> = fixes.iter().copied().filter(|f| f.time.is_finite() && valid(f.lat, f.lon)).collect();
    let skipped = fixes.len() - fx.len();
    fx.retain(|f| p.from.is_none_or(|a| f.time >= a) && p.to.is_none_or(|b| f.time <= b));
    fx.retain(|f| match cuts.iter().find(|c| c.holds(f.time)) {
        Some(c) if c.end => {
            trim.end += 1;
            false
        }
        Some(_) => {
            trim.start += 1;
            false
        }
        None => true,
    });
    fx.sort_by(|a, b| a.time.total_cmp(&b.time).then(a.id.cmp(&b.id)));
    let covering = |t: f64| {
        let i = recs.partition_point(|r| r.end < t);
        recs.get(i).is_some_and(|r| r.start <= t)
    };
    // the estimated runs, between the recorded pieces: a photo is named by its place in `fx` (below `n`), and the
    // ends of recorded piece k by `n + 2k` (its start) and `n + 2k + 1` (its end)
    let n = fx.len() as u64;
    let is_photo = |id: u64| id < n;
    let anchor = |k: usize, end: bool| -> Option<Fix> {
        let r = recs.get(k)?;
        let q = if end { r.points.last()? } else { r.points.first()? };
        Some(Fix { id: n + 2 * k as u64 + u64::from(end), time: if end { r.end } else { r.start }, lat: q.lat, lon: q.lon })
    };
    let mut runs: Vec<Vec<Fix>> = Vec::with_capacity(recs.len() + 1);
    let mut cur: Vec<Fix> = Vec::new();
    let mut uncovered = fx.iter().enumerate().filter(|(_, f)| !covering(f.time)).map(|(i, f)| Fix { id: i as u64, ..*f }).peekable();
    for (k, r) in recs.iter().enumerate() {
        while let Some(f) = uncovered.next_if(|f| f.time < r.start) {
            cur.push(f);
        }
        cur.extend(anchor(k, false));
        runs.push(std::mem::take(&mut cur));
        cur.extend(anchor(k, true));
    }
    cur.extend(uncovered);
    runs.push(cur);
    let refs: Vec<&[Fix]> = runs.iter().map(Vec::as_slice).collect();
    let routes = route::estimate_runs(&refs, &p.route);
    // trim each route at its ends that are photos; keep its legs and the stops that hold photos
    let real_id = |i: u64| fx.get(i as usize).map(|f| f.id);
    let mut legs: Vec<Leg> = Vec::new();
    let mut stops: Vec<(Option<(usize, usize)>, route::Stop)> = Vec::new();
    let mut gone = vec![false; fx.len()];
    // each photo's stop, and the recorded pieces a stop with photos joins (its piece ends)
    let mut home: Vec<Option<(usize, usize)>> = vec![None; fx.len()];
    let mut joins: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
    for (ri, r) in routes.routes.iter().enumerate() {
        let photo_first = r.stops.first().and_then(|s| s.photos.first()).is_some_and(|id| is_photo(*id));
        let photo_last = r.stops.last().and_then(|s| s.photos.last()).is_some_and(|id| is_photo(*id));
        let (kept_legs, stops_kept, cut) = trim_route(ri, r, p.trim_m, photo_first, photo_last);
        trim.points += cut;
        let first_kept = stops_kept.iter().position(|k| *k);
        let has_photos = r.stops.iter().any(|s| s.photos.iter().any(|id| is_photo(*id)));
        if first_kept.is_none() && has_photos {
            trim.vanished += 1;
        }
        for (si, (s, kept)) in r.stops.iter().zip(&stops_kept).enumerate() {
            let photos: Vec<u64> = s.photos.iter().copied().filter(|id| is_photo(*id)).collect();
            if !kept {
                for id in &photos {
                    if let Some(g) = gone.get_mut(*id as usize) {
                        *g = true;
                    }
                }
                if first_kept.is_none_or(|f| si < f) {
                    trim.start += photos.len();
                } else {
                    trim.end += photos.len();
                }
            } else if !photos.is_empty() {
                for id in &photos {
                    if let Some(h) = home.get_mut(*id as usize) {
                        *h = Some((ri, si));
                    }
                }
                let ends: Vec<usize> = s.photos.iter().filter(|id| !is_photo(**id)).map(|a| ((a - n) / 2) as usize).collect();
                joins.insert((ri, si), ends);
                let photos = photos.into_iter().filter_map(real_id).collect();
                stops.push((Some((ri, si)), route::Stop { photos, ..s.clone() }));
            }
        }
        legs.extend(kept_legs);
    }
    // the stops in time order: estimated (with photos) and recorded (photos below)
    for r in &recs {
        stops.extend(r.stops.iter().map(|s| (None, s.clone())));
    }
    stops.sort_by(|a, b| a.1.arrive.total_cmp(&b.1.arrive).then(a.1.leave.total_cmp(&b.1.leave)));
    let index: HashMap<(usize, usize), usize> = stops.iter().enumerate().filter_map(|(i, (k, _))| k.map(|k| (k, i))).collect();
    // the pieces in time order, each with what it came from: a recorded piece, or a leg between two stops of a route
    let mut pieces: Vec<(Piece, Result<usize, (usize, usize, usize)>)> = Vec::with_capacity(recs.len() + legs.len());
    for (k, r) in recs.iter().enumerate() {
        let (mut lines, mut times) = (Vec::new(), Vec::new());
        for stretch in r.points.chunk_by(|_, b| !b.new_segment) {
            for line in route::split_timed(stretch.iter().map(|q| (q.lat, q.lon, secs(q))).collect()) {
                lines.push(line.iter().map(|q| (q.0, q.1)).collect());
                times.push(line.iter().map(|q| q.2).collect());
            }
        }
        let piece = Piece {
            start: r.start,
            end: r.end,
            distance_km: r.distance_km,
            duration_s: r.end - r.start,
            moving_s: r.moving_s,
            lines,
            kind: Kind::Recorded { track: r.track, ascent_m: r.ascent_m, descent_m: r.descent_m, times },
        };
        pieces.push((piece, Ok(k)));
    }
    for l in legs {
        let from = Err((l.route, l.from, l.to));
        let piece = Piece {
            start: l.start,
            end: l.end,
            distance_km: l.leg.distance_km,
            duration_s: l.leg.duration_s,
            moving_s: l.leg.duration_s,
            kind: Kind::Estimated {
                from: index.get(&(l.route, l.from)).copied(),
                to: index.get(&(l.route, l.to)).copied(),
                speed_kmh: l.leg.speed_kmh,
                mode: l.leg.mode,
                arc: l.leg.arc,
            },
            lines: l.leg.lines,
        };
        pieces.push((piece, from));
    }
    pieces.sort_by(|a, b| a.0.start.total_cmp(&b.0.start).then(a.0.end.total_cmp(&b.0.end)));
    let (pieces, came): (Vec<Piece>, Vec<_>) = pieces.into_iter().unzip();
    let mut rec_at: HashMap<usize, usize> = HashMap::new();
    let mut at_stop: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
    for (i, c) in came.iter().enumerate() {
        match *c {
            Ok(k) => {
                rec_at.insert(k, i);
            }
            Err((ri, from, to)) => {
                at_stop.entry((ri, from)).or_default().push(i);
                at_stop.entry((ri, to)).or_default().push(i);
            }
        }
    }
    // the photos on the path: taken while recording, on that recording; at a stop, on the leg arriving there or the
    // one leaving, or the recording the stop joins, whichever is nearest in time; a stop on none (a route of one
    // stop) leaves its photos on none. One taken during a recorded stop is also that stop's.
    let mut placed: Vec<Placed> = Vec::with_capacity(fx.len());
    for (i, f) in fx.iter().enumerate() {
        if gone.get(i).copied().unwrap_or(false) {
            continue;
        }
        let k = recs.partition_point(|r| r.end < f.time);
        let piece = match home.get(i).copied().flatten() {
            None => recs.get(k).filter(|r| r.start <= f.time).and_then(|_| rec_at.get(&k).copied()),
            Some(stop) => {
                let legs = at_stop.get(&stop).into_iter().flatten().copied();
                let recorded = joins.get(&stop).into_iter().flatten().filter_map(|k| rec_at.get(k).copied());
                nearest(&pieces, legs.chain(recorded), f.time)
            }
        };
        let at = piece.and_then(|k| pieces.get(k)).map_or(0.0, |q| fraction(q, f.time));
        placed.push(Placed { id: f.id, t: f.time, lat: f.lat, lon: f.lon, piece, at });
        if covering(f.time) {
            let j = stops.partition_point(|(_, s)| s.leave < f.time);
            if let Some((_, s)) = stops.get_mut(j..).into_iter().flatten().take_while(|(_, s)| s.arrive <= f.time).find(|(k, _)| k.is_none()) {
                s.photos.push(f.id);
            }
        }
    }
    Ok(Path { pieces, stops: stops.into_iter().map(|(_, s)| s).collect(), photos: placed, trim: p.trim_m.map(|_| trim), skipped })
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: f64 = 1_700_000_000.0;
    const H: f64 = 3600.0;
    /// Kilometres in a degree of latitude.
    const KM: f64 = 111.195;

    fn fix(id: u64, hours: f64, lat: f64, lon: f64) -> Fix {
        Fix { id, time: T0 + hours * H, lat, lon }
    }

    fn pt(secs: f64, lat: f64, lon: f64) -> Point {
        Point { time_ms: (secs * 1000.0).round() as i64, lat, lon, ele_m: None, acc_m: None, new_segment: false }
    }

    /// A track from `from_h` to `to_h` hours, a point a minute, due east from `(lat, lon)` at `kmh`.
    fn east(from_h: f64, to_h: f64, lat: f64, lon: f64, kmh: f64) -> Vec<Point> {
        let n = ((to_h - from_h) * 60.0).round() as usize;
        let per_deg = KM * lat.to_radians().cos();
        (0..=n).map(|i| pt(T0 + from_h * H + i as f64 * 60.0, lat, wrap(lon + kmh * i as f64 / 60.0 / per_deg))).collect()
    }

    fn km(a: (f64, f64), b: (f64, f64)) -> f64 {
        distance_km(a.0, a.1, b.0, b.1)
    }

    fn first_point(p: &Piece) -> (f64, f64) {
        p.lines.first().and_then(|l| l.first()).copied().unwrap()
    }

    fn last_point(p: &Piece) -> (f64, f64) {
        p.lines.last().and_then(|l| l.last()).copied().unwrap()
    }

    /// The invariants every path keeps.
    fn check(path: &Path, gap_s: f64) {
        for w in path.pieces.windows(2) {
            assert!(w[0].end <= w[1].start, "pieces in time order, never overlapping: {:?} {:?}", (w[0].start, w[0].end), (w[1].start, w[1].end));
        }
        for p in &path.pieces {
            assert!(p.start <= p.end && (p.duration_s - (p.end - p.start)).abs() < 1e-6 && p.moving_s <= p.duration_s + 1e-6, "{p:?}");
            assert!(p.distance_km.is_finite() && p.distance_km >= 0.0);
            for l in &p.lines {
                assert!(l.len() >= 2 && l.iter().all(|q| valid(q.0, q.1)));
                assert!(l.windows(2).all(|w| (w[0].1 - w[1].1).abs() <= 180.0), "no line across the antimeridian");
            }
            match &p.kind {
                Kind::Recorded { times, .. } => {
                    assert_eq!(times.iter().map(Vec::len).collect::<Vec<_>>(), p.lines.iter().map(Vec::len).collect::<Vec<_>>());
                    for t in times {
                        assert!(t.windows(2).all(|w| w[0] <= w[1] && w[1] - w[0] <= gap_s), "no recorded line across a long pause");
                        assert!(t.iter().all(|t| (p.start..=p.end).contains(t)));
                    }
                }
                Kind::Estimated { from, to, .. } => {
                    assert!(p.duration_s <= gap_s, "no estimated leg across a long pause");
                    assert!(from.is_none_or(|i| i < path.stops.len()) && to.is_none_or(|i| i < path.stops.len()));
                }
            }
        }
        let mut last = (0usize, 0.0f64);
        for ph in &path.photos {
            let Some(k) = ph.piece else { continue };
            assert!(k < path.pieces.len() && (0.0..=1.0).contains(&ph.at));
            assert!((k, ph.at) >= last, "photos follow the path in time: {:?} after {last:?}", (k, ph.at));
            last = (k, ph.at);
        }
        let s = path.stats();
        assert!((s.recorded_km + s.estimated_km - s.distance_km).abs() < 1e-9);
        assert!(s.moving_s <= s.duration_s + 1e-6);
    }

    fn lone(points: Vec<Point>) -> Vec<Vec<Point>> {
        vec![points]
    }

    #[test]
    fn a_recording_replaces_the_estimate_over_its_time() {
        // recorded 1 h to 3 h, 5 km/h east from (40, -3); photos before, on, and after it
        let track = east(1.0, 3.0, 40.0, -3.0, 5.0);
        let (start, end) = ((track[0].lat, track[0].lon), (track[120].lat, track[120].lon));
        let fixes = [fix(1, 0.0, 40.0, -3.1), fix(2, 1.5, 40.01, -2.97), fix(3, 2.5, 40.0, -2.96), fix(4, 4.0, 40.05, -2.8)];
        let path = build(&fixes, lone(track), &Params::default()).unwrap();
        check(&path, route::GAP_S);
        let kinds: Vec<bool> = path.pieces.iter().map(Piece::recorded).collect();
        assert_eq!(kinds, vec![false, true, false], "estimated, recorded, estimated: nothing estimated over the recorded time");
        let (before, rec, after) = (&path.pieces[0], &path.pieces[1], &path.pieces[2]);
        assert_eq!(
            (before.start, before.end, rec.start, rec.end, after.start, after.end),
            (T0, T0 + H, T0 + H, T0 + 3.0 * H, T0 + 3.0 * H, T0 + 4.0 * H)
        );
        // the dashed lines meet the recorded one
        assert_eq!((last_point(before), first_point(rec), last_point(rec), first_point(after)), (start, start, end, end));
        assert!((rec.distance_km - 10.0).abs() < 0.05, "{}", rec.distance_km);
        // the photos taken while recording are on the recording, by time
        let on: Vec<(u64, Option<usize>, f64)> = path.photos.iter().map(|p| (p.id, p.piece, p.at)).collect();
        assert_eq!(on, vec![(1, Some(0), 0.0), (2, Some(1), 0.25), (3, Some(1), 0.75), (4, Some(2), 1.0)]);
        // the stops are the photos' (a piece's end is no stop), and the legs name them
        assert_eq!(path.stops.iter().map(|s| s.photos.clone()).collect::<Vec<_>>(), vec![vec![1], vec![4]]);
        assert!(matches!(before.kind, Kind::Estimated { from: Some(0), to: None, .. }));
        assert!(matches!(after.kind, Kind::Estimated { from: None, to: Some(1), .. }));
        let s = path.stats();
        assert_eq!((s.photos, s.stops), (4, 2));
        assert!((s.recorded_km - rec.distance_km).abs() < 1e-12 && s.estimated_km > 8.0);
        assert!((s.duration_s - 4.0 * H).abs() < 1e-6);
    }

    #[test]
    fn a_photo_at_a_stop_is_on_the_leg_nearest_in_time() {
        // three photos at one stop (1 h, 1 h 10, 1 h 50) between two legs: the middle one is nearer the arrival
        let fixes = [
            fix(1, 0.0, 40.0, -3.0),
            fix(2, 1.0, 40.05, -3.0),
            fix(3, 7.0 / 6.0, 40.05, -3.0),
            fix(4, 11.0 / 6.0, 40.05, -3.0),
            fix(5, 3.0, 40.1, -3.0),
        ];
        let path = build(&fixes, Vec::<Vec<Point>>::new(), &Params::default()).unwrap();
        check(&path, route::GAP_S);
        let on: Vec<(u64, Option<usize>, f64)> = path.photos.iter().map(|p| (p.id, p.piece, p.at)).collect();
        assert_eq!(on, vec![(1, Some(0), 0.0), (2, Some(0), 1.0), (3, Some(0), 1.0), (4, Some(1), 0.0), (5, Some(1), 1.0)]);
        assert_eq!(path.stops[1].photos, vec![2, 3, 4]);
    }

    #[test]
    fn a_photo_just_after_a_recording_is_at_its_end_and_a_lone_one_on_none() {
        // the recording ends at 2 h; a photo 5 minutes later beside its end shares a stop with it (no leg)
        let track = east(1.0, 2.0, 40.0, -3.0, 5.0);
        let end = *track.last().unwrap();
        let fixes = [Fix { id: 1, time: T0 + 2.0 * H + 300.0, lat: end.lat, lon: end.lon + 0.0005 }, fix(2, 100.0, 50.0, 0.0)];
        let path = build(&fixes, lone(track), &Params::default()).unwrap();
        check(&path, route::GAP_S);
        assert_eq!(path.pieces.len(), 1);
        assert_eq!(path.photos.iter().map(|p| (p.id, p.piece, p.at)).collect::<Vec<_>>(), vec![(1, Some(0), 1.0), (2, None, 0.0)]);
    }

    #[test]
    fn a_track_inside_a_leg_and_a_leg_half_covered() {
        // photos at 0 h and 10 h, a recording 4 h to 6 h with none taken during it: the leg is cut around it
        let fixes = [fix(1, 0.0, 40.0, -3.5), fix(2, 10.0, 40.0, -2.0)];
        let path = build(&fixes, lone(east(4.0, 6.0, 40.0, -3.0, 5.0)), &Params::default()).unwrap();
        check(&path, route::GAP_S);
        let spans: Vec<(f64, f64, bool)> = path.pieces.iter().map(|p| ((p.start - T0) / H, (p.end - T0) / H, p.recorded())).collect();
        assert_eq!(spans, vec![(0.0, 4.0, false), (4.0, 6.0, true), (6.0, 10.0, false)]);
        // the second photo inside the recording: only the time before it stays estimated
        let fixes = [fix(1, 0.0, 40.0, -3.5), fix(2, 2.0, 40.0, -2.9)];
        let path = build(&fixes, lone(east(1.0, 3.0, 40.0, -3.0, 5.0)), &Params::default()).unwrap();
        check(&path, route::GAP_S);
        let spans: Vec<(f64, f64, bool)> = path.pieces.iter().map(|p| ((p.start - T0) / H, (p.end - T0) / H, p.recorded())).collect();
        assert_eq!(spans, vec![(0.0, 1.0, false), (1.0, 3.0, true)]);
        assert_eq!(path.photos[1].piece, Some(1));
    }

    #[test]
    fn long_pauses_and_breaks_in_a_recording() {
        // a pause of 39 hours inside one track: two pieces, and no line between them
        let mut track = east(0.0, 1.0, 40.0, -3.0, 5.0);
        track.extend(east(40.0, 41.0, 41.0, 2.0, 5.0));
        let path = build(&[], lone(track.clone()), &Params::default()).unwrap();
        check(&path, route::GAP_S);
        assert_eq!(path.pieces.iter().map(Piece::recorded).collect::<Vec<_>>(), vec![true, true]);
        // with pauses up to 48 hours allowed it is one piece (a recording is a recording)
        let longer = Params { route: route::Params { gap_s: 48.0 * H, ..route::Params::default() }, ..Params::default() };
        assert_eq!(build(&[], lone(track), &longer).unwrap().pieces.len(), 1);
        // a break in the recording: one piece, two lines, the break's half hour neither moving nor counted as distance
        let mut track = east(0.0, 1.0, 40.0, -3.0, 5.0);
        let mut more = east(1.5, 2.0, 40.1, -3.0, 5.0);
        more[0].new_segment = true;
        track.extend(more);
        let path = build(&[], lone(track), &Params::default()).unwrap();
        check(&path, route::GAP_S);
        let p = &path.pieces[0];
        assert_eq!((path.pieces.len(), p.lines.len()), (1, 2));
        assert!((p.moving_s - 1.5 * H).abs() < 1e-6 && (p.duration_s - 2.0 * H).abs() < 1e-6, "{} {}", p.moving_s, p.duration_s);
        assert!((p.distance_km - 7.5).abs() < 0.05, "the jump over the break is not distance: {}", p.distance_km);
    }

    #[test]
    fn tracks_on_different_days_and_the_photos_between() {
        let day = 24.0;
        let tracks = vec![east(9.0, 17.0, 40.0, -3.0, 4.0), east(2.0 * day + 9.0, 2.0 * day + 17.0, 41.0, 2.0, 4.0)];
        let fixes = [fix(1, 12.0, 40.0, -2.9), fix(2, day + 12.0, 40.5, 0.0), fix(3, 2.0 * day + 12.0, 41.0, 2.1), fix(4, 9.0 * day, 48.0, 2.0)];
        let path = build(&fixes, tracks, &Params::default()).unwrap();
        check(&path, route::GAP_S);
        let kinds: Vec<bool> = path.pieces.iter().map(Piece::recorded).collect();
        assert_eq!(kinds, vec![true, false, false, true], "day 1 recorded, two legs through day 2's photo, day 3 recorded; day 9 alone");
        assert_eq!(
            path.photos.iter().map(|p| p.piece).collect::<Vec<_>>(),
            vec![Some(0), Some(2), Some(3), None],
            "a week later, alone: on no piece"
        );
        assert_eq!(path.photos[1].at, 0.0, "between the leg arriving and the one leaving at the same moment: the start of the next");
        assert_eq!(path.stops.iter().map(|s| s.photos.clone()).collect::<Vec<_>>(), vec![vec![2], vec![4]]);
    }

    #[test]
    fn two_recordings_of_the_same_time_are_drawn_once() {
        // the first given keeps what it covers; the other keeps only what comes after
        let tracks = vec![east(0.0, 2.0, 40.0, -3.0, 5.0), east(1.0, 3.0, 40.0001, -2.95, 5.0), east(0.5, 1.5, 50.0, 0.0, 5.0)];
        let path = build(&[fix(1, 1.0, 40.0, -2.95)], tracks, &Params::default()).unwrap();
        check(&path, route::GAP_S);
        let tracks_of: Vec<usize> =
            path.pieces.iter().filter_map(|p| if let Kind::Recorded { track, .. } = p.kind { Some(track) } else { None }).collect();
        assert_eq!(tracks_of, vec![0, 1], "the third lies inside the first: gone");
        let second = path.pieces.iter().filter(|p| p.recorded()).nth(1).unwrap();
        assert!(second.start > T0 + 2.0 * H, "{}", second.start - T0);
        assert_eq!(path.photos[0].piece, Some(0));
    }

    #[test]
    fn a_stay_on_a_recording_is_a_stop() {
        // walk 30 minutes, stay 20 minutes within a few metres, walk 30 minutes; a photo during the stay
        let mut track = east(0.0, 0.5, 40.0, -3.0, 5.0);
        let here = *track.last().unwrap();
        track.extend((1..=20).map(|i| pt(secs(&here) + i as f64 * 60.0, here.lat + (i % 3) as f64 * 1e-5, here.lon)));
        let then = *track.last().unwrap();
        track.extend(
            east(0.0, 0.5, then.lat, then.lon, 5.0)
                .into_iter()
                .skip(1)
                .map(|q| Point { time_ms: q.time_ms + then.time_ms - (T0 * 1000.0) as i64, ..q }),
        );
        let tight = Params { route: route::Params { stop_radius_m: 50.0, ..route::Params::default() }, ..Params::default() };
        let path = build(&[fix(7, 40.0 / 60.0, 40.0, -2.97)], lone(track.clone()), &tight).unwrap();
        check(&path, route::GAP_S);
        assert_eq!(path.stops.len(), 1);
        let s = &path.stops[0];
        assert_eq!((s.arrive, s.leave, s.photos.clone()), (T0 + 0.5 * H, T0 + 50.0 * 60.0, vec![7]));
        let p = &path.pieces[0];
        assert!((p.moving_s - 60.0 * 60.0).abs() < 1e-6, "{}", p.moving_s);
        // a stay shorter than asked for is no stop
        let picky = Params { dwell_s: 30.0 * 60.0, ..tight };
        let path = build(&[], lone(track), &picky).unwrap();
        assert!(path.stops.is_empty() && (path.pieces[0].moving_s - path.pieces[0].duration_s).abs() < 1e-6);
    }

    #[test]
    fn a_recording_across_the_antimeridian_comes_in_two_lines() {
        let path = build(&[], lone(east(0.0, 1.0, 0.0, 179.9, 30.0)), &Params::default()).unwrap();
        check(&path, route::GAP_S);
        let p = &path.pieces[0];
        let Kind::Recorded { times, .. } = &p.kind else { panic!() };
        assert_eq!(p.lines.len(), 2);
        assert_eq!((p.lines[0].last().unwrap().1, p.lines[1].first().unwrap().1), (180.0, -180.0));
        assert_eq!(times[0].last(), times[1].first(), "the crossing is one moment");
        assert!((p.distance_km - 30.0).abs() < 0.1, "the short way round: {}", p.distance_km);
    }

    /// A square loop of 2 km sides from `home`, a point every 50 m and 36 s, starting at 0 h.
    fn square(home: (f64, f64)) -> Vec<Point> {
        let k = 1.0 / (KM * home.0.to_radians().cos());
        let corners = [(0.0, 0.0), (0.0, 2.0), (2.0, 2.0), (2.0, 0.0), (0.0, 0.0)];
        let mut out = Vec::new();
        for w in corners.windows(2) {
            for i in 0..40 {
                let f = i as f64 / 40.0;
                let (n, e) = (w[0].0 + (w[1].0 - w[0].0) * f, w[0].1 + (w[1].1 - w[0].1) * f);
                out.push(pt(T0 + out.len() as f64 * 36.0, home.0 + n / KM, home.1 + e * k));
            }
        }
        out.push(pt(T0 + out.len() as f64 * 36.0, home.0, home.1));
        out
    }

    #[test]
    fn the_privacy_trim_cuts_both_ends_of_a_loop_from_home() {
        let home = (40.0, -3.0);
        let track = square(home);
        let end_s = secs(track.last().unwrap());
        let fixes = [
            fix(1, 0.0, home.0, home.1),
            Fix { id: 2, time: T0 + 60.0 * 36.0, lat: 40.018, lon: -2.98 },
            Fix { id: 3, time: end_s, lat: home.0, lon: home.1 },
        ];
        let trimmed = Params { trim_m: Some(300.0), ..Params::default() };
        let path = build(&fixes, lone(track.clone()), &trimmed).unwrap();
        check(&path, route::GAP_S);
        assert_eq!(path.pieces.len(), 1);
        let p = &path.pieces[0];
        assert!(km(home, first_point(p)) > 0.3 && km(home, last_point(p)) > 0.3, "{:?} {:?}", first_point(p), last_point(p));
        assert!((p.distance_km - (8.0 - 0.7)).abs() < 0.11, "the middle is kept: {}", p.distance_km);
        assert_eq!(path.photos.iter().map(|p| p.id).collect::<Vec<_>>(), vec![2], "the photos at home went with the ends");
        assert_eq!(path.trim, Some(Trimmed { radius_m: 300.0, start: 1, end: 1, vanished: 0, points: 12 }));
        // off: nothing cut
        let path = build(&fixes, lone(track.clone()), &Params::default()).unwrap();
        assert_eq!((path.photos.len(), path.trim), (3, None));
        assert!(km(home, first_point(&path.pieces[0])) < 1e-9);
        // radius 0: the end points themselves only
        let zero = build(&[], lone(track.clone()), &Params { trim_m: Some(0.0), ..Params::default() }).unwrap();
        assert_eq!(zero.pieces[0].start, T0 + 36.0);
        // a radius larger than the loop: it vanishes, and says so
        let huge = build(&fixes, lone(track), &Params { trim_m: Some(1e8), ..Params::default() }).unwrap();
        assert!(huge.pieces.is_empty() && huge.photos.is_empty() && huge.stops.is_empty());
        assert_eq!(huge.trim, Some(Trimmed { radius_m: 1e8, start: 3, end: 0, vanished: 1, points: 161 }));
        assert_eq!(huge.range(), None);
    }

    #[test]
    fn a_route_shorter_than_twice_the_radius_vanishes() {
        let short = east(0.0, 0.05, 40.0, -3.0, 5.0);
        assert!(km((short[0].lat, short[0].lon), (short[3].lat, short[3].lon)) < 0.6);
        let path = build(&[fix(1, 0.01, 40.0, -3.0)], lone(short), &Params { trim_m: Some(300.0), ..Params::default() }).unwrap();
        assert!(path.pieces.is_empty() && path.photos.is_empty());
        assert_eq!(path.trim.map(|t| (t.vanished, t.start)), Some((1, 1)));
        // estimated only: two photos 400 m apart
        let path = build(
            &[fix(1, 0.0, 40.0, -3.0), fix(2, 1.0, 40.0036, -3.0)],
            Vec::<Vec<Point>>::new(),
            &Params { trim_m: Some(300.0), ..Params::default() },
        )
        .unwrap();
        assert!(path.pieces.is_empty() && path.photos.is_empty() && path.stops.is_empty());
        assert_eq!(path.trim.map(|t| (t.vanished, t.start, t.end)), Some((1, 2, 0)));
    }

    #[test]
    fn an_estimated_route_is_trimmed_at_its_photo_ends_only() {
        let home = (40.0, -3.0);
        let trimmed = Params { trim_m: Some(300.0), ..Params::default() };
        // photos only: home, 5 km, 10 km east, home again
        let fixes = [fix(1, 0.0, home.0, home.1), fix(2, 1.0, 40.0, -2.94), fix(3, 2.0, 40.0, -2.88), fix(4, 3.0, home.0, home.1)];
        let path = build(&fixes, Vec::<Vec<Point>>::new(), &trimmed).unwrap();
        check(&path, route::GAP_S);
        assert_eq!(path.photos.iter().map(|p| p.id).collect::<Vec<_>>(), vec![2, 3]);
        assert_eq!(path.stops.len(), 2);
        let (a, z) = (path.pieces.first().unwrap(), path.pieces.last().unwrap());
        assert!(km(home, first_point(a)) > 0.3 && km(home, last_point(z)) > 0.3);
        assert!(matches!(a.kind, Kind::Estimated { from: None, to: Some(0), .. }), "{:?}", a.kind);
        assert!(a.start > T0 && a.distance_km < 5.0 && a.duration_s < H, "a cut leg keeps its share: {} {}", a.distance_km, a.duration_s);
        assert_eq!(path.trim.map(|t| (t.start, t.end, t.vanished)), Some((1, 1, 0)));
        // a photo at home, then a recording that begins 5 km away: the leg is cut at home and still meets the recording
        let track = east(1.0, 2.0, 40.0, -2.94, 5.0);
        let path = build(&[fix(1, 0.0, home.0, home.1)], lone(track.clone()), &trimmed).unwrap();
        check(&path, route::GAP_S);
        assert_eq!(path.pieces.iter().map(Piece::recorded).collect::<Vec<_>>(), vec![false, true]);
        assert!(km(home, first_point(&path.pieces[0])) > 0.3);
        // (the recording's own start is trimmed too: where it now begins is where the leg ends)
        assert_eq!(last_point(&path.pieces[0]), first_point(&path.pieces[1]));
        assert!(path.photos.is_empty());
    }

    #[test]
    fn simplify_shape_keeps_a_corner_that_evenly_picked_points_miss() {
        // 10 001 points east, then north from a corner at an odd index
        let mut points: Vec<Point> = (0..=5003).map(|i| pt(i as f64, 0.0, i as f64 * 1e-4)).collect();
        points.extend((1..=5000).map(|i| pt(5003.0 + i as f64, i as f64 * 1e-4, 0.5003)));
        let few = simplify_shape(&points, 50);
        assert!(few.len() <= 50);
        assert!(few.iter().any(|q| q.time_ms == 5_003_000), "the corner is kept");
        assert!(!track::simplify(&points, 50).0.iter().any(|q| q.time_ms == 5_003_000), "evenly picked misses it");
        assert_eq!((few[0], *few.last().unwrap()), (points[0], *points.last().unwrap()));
        // breaks: both sides kept, flags as they were
        let mut broken = points.clone();
        broken[777].new_segment = true;
        let few = simplify_shape(&broken, 50);
        assert!(few.iter().any(|q| q.time_ms == 776_000) && few.iter().any(|q| q.time_ms == 777_000 && q.new_segment));
        assert_eq!(few.iter().filter(|q| q.new_segment).count(), 1);
        assert!(few.windows(2).all(|w| w[0].time_ms < w[1].time_ms));
        // more breaks than room: the evenly picked ones, never more than asked
        let all_broken: Vec<Point> = (0..1000).map(|i| Point { new_segment: i % 2 == 1, ..pt(i as f64, 0.0, i as f64 * 1e-3) }).collect();
        assert_eq!(simplify_shape(&all_broken, 10).len(), 10);
        assert_eq!(simplify_shape(&points[..5], 50), points[..5].to_vec());
        assert_eq!(simplify_shape(&points, 0).len(), 2);
    }

    #[test]
    fn simplify_shape_takes_the_worst_case_in_its_stride() {
        // 100 000 points of a random walk (the most a chunk holds): bounded time, at most what was asked
        let mut x = 0x2545_f491_4f6c_dd1du64;
        let mut rand = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 11) as f64 / (1u64 << 53) as f64 - 0.5
        };
        let (mut lat, mut lon) = (40.0, -3.0);
        let points: Vec<Point> = (0..100_000)
            .map(|i| {
                lat += rand() * 1e-4;
                lon += rand() * 1e-4;
                Point { new_segment: i % 9973 == 0 && i > 0, ..pt(i as f64, lat, lon) }
            })
            .collect();
        let started = std::time::Instant::now();
        let few = simplify_shape(&points, 4000);
        assert!(few.len() <= 4000 && few.len() >= 3990, "{}", few.len());
        assert!(started.elapsed().as_secs() < 30);
        // a stationary cloud (every triangle tiny, many equal) too
        let still: Vec<Point> = (0..100_000).map(|i| pt(i as f64, 40.0 + (i % 3) as f64 * 1e-7, -3.0)).collect();
        assert_eq!(simplify_shape(&still, 100).len(), 100);
    }

    #[test]
    fn what_is_not_real_is_left_out() {
        let fixes = [
            fix(1, 0.0, 40.0, -3.0),
            Fix { id: 2, time: f64::NAN, lat: 40.0, lon: -3.0 },
            Fix { id: 3, time: f64::INFINITY, lat: 40.0, lon: -3.0 },
            fix(4, 1.0, f64::NAN, 0.0),
            fix(5, 1.0, 91.0, 0.0),
            fix(6, 2.0, 40.1, -3.0),
        ];
        let mut track = east(0.5, 1.5, 40.0, -3.0, 5.0);
        track.push(pt(f64::NAN, 40.0, -3.0));
        track.push(Point { lat: f64::INFINITY, ..pt(T0 + 1.2 * H, 0.0, 0.0) });
        track.reverse();
        let wild = Params {
            route: route::Params { stop_radius_m: f64::NAN, gap_s: -1.0, detail: 0, arc_km: f64::INFINITY },
            dwell_s: f64::NAN,
            max_points: 0,
            trim_m: Some(f64::NAN),
            from: Some(f64::NEG_INFINITY),
            to: None,
        };
        for params in [Params::default(), wild] {
            let path = build(&fixes, vec![track.clone(), Vec::new(), vec![pt(T0, 1.0, 1.0)]], &params).unwrap();
            assert_eq!(path.skipped, 4);
            assert_eq!(path.photos.len(), 2);
            assert!(path.pieces.iter().flat_map(|p| p.lines.iter().flatten()).all(|q| valid(q.0, q.1)));
        }
        assert_eq!(build(&[], Vec::<Vec<Point>>::new(), &Params::default()).unwrap(), Path::default());
    }

    #[test]
    fn a_window_keeps_what_lies_in_it() {
        let fixes = [fix(1, 0.0, 40.0, -3.5), fix(2, 1.5, 40.0, -2.95), fix(3, 5.0, 40.0, -2.0)];
        let w = Params { from: Some(T0 + H), to: Some(T0 + 2.0 * H), ..Params::default() };
        let path = build(&fixes, lone(east(0.0, 3.0, 40.0, -3.0, 5.0)), &w).unwrap();
        check(&path, route::GAP_S);
        assert_eq!(path.photos.iter().map(|p| p.id).collect::<Vec<_>>(), vec![2]);
        assert_eq!((path.pieces.len(), path.pieces[0].start, path.pieces[0].end), (1, T0 + H, T0 + 2.0 * H));
        assert_eq!(path.range(), Some((T0 + H, T0 + 2.0 * H)));
    }

    #[test]
    fn a_hundred_thousand_photos_and_many_pieces_stay_bounded() {
        // a photo every 5 minutes for a year near Madrid, and a recording on every tenth day
        let fixes: Vec<Fix> =
            (0..100_000u64).map(|i| fix(i, i as f64 / 12.0, 40.0 + (i % 97) as f64 * 1e-3, -3.0 + (i % 89) as f64 * 1e-3)).collect();
        let tracks: Vec<Vec<Point>> = (0..36).map(|d| east(d as f64 * 240.0 + 9.0, d as f64 * 240.0 + 17.0, 40.05, -3.0, 0.5)).collect();
        let started = std::time::Instant::now();
        let path = build(&fixes, tracks, &Params::default()).unwrap();
        assert!(started.elapsed().as_secs() < 60);
        check(&path, route::GAP_S);
        assert_eq!(path.photos.len(), 100_000);
        let recorded: usize = path.pieces.iter().filter(|p| p.recorded()).flat_map(|p| p.lines.iter()).map(Vec::len).sum();
        let estimated: usize = path.pieces.iter().filter(|p| !p.recorded()).flat_map(|p| p.lines.iter()).map(Vec::len).sum();
        assert!(recorded <= RECORDED_MAX + 36 * 4 && estimated <= route::MAX_POINTS * 2, "{recorded} {estimated}");
        // too many recorded pieces: refused, not an answer of every one
        let many: Vec<Vec<Point>> =
            (0..PIECES_MAX + 1).map(|i| vec![pt(T0 + i as f64 * 1e6, 1.0, 1.0), pt(T0 + i as f64 * 1e6 + 1.0, 1.0, 1.001)]).collect();
        assert_eq!(build(&[], many, &Params::default()), Err(TooManyPieces(PIECES_MAX + 1)));
    }

    #[test]
    fn many_recorded_pieces_share_the_point_budget() {
        let tracks: Vec<Vec<Point>> = (0..40).map(|d| east(d as f64 * 48.0, d as f64 * 48.0 + 30.0, 40.0, -3.0, 3.0)).collect();
        let path = build(&[], tracks, &Params { max_points: 20_000, ..Params::default() }).unwrap();
        let recorded: usize = path.pieces.iter().filter(|p| p.recorded()).flat_map(|p| p.lines.iter()).map(Vec::len).sum();
        assert_eq!(path.pieces.iter().filter(|p| p.recorded()).count(), 40);
        assert!(recorded <= RECORDED_MAX && recorded > RECORDED_MAX * 9 / 10, "{recorded}");
    }

    proptest::proptest! {
        #[test]
        fn any_photos_and_tracks_give_a_consistent_path(
            raw in proptest::collection::vec((0u64..10_000, 0.0f64..200.0, -0.3f64..0.3, -0.3f64..0.3), 0..60),
            tracks in proptest::collection::vec((0.0f64..200.0, 0.1f64..20.0, 0.5f64..40.0, proptest::bool::ANY), 0..5),
            gap_h in 1.0f64..48.0,
            trim in proptest::option::of(0.0f64..3000.0),
        ) {
            let fixes: Vec<Fix> = raw.iter().map(|&(id, h, la, lo)| fix(id, h, 40.0 + la, -3.0 + lo)).collect();
            let tracks: Vec<Vec<Point>> = tracks
                .iter()
                .map(|&(from, len, kmh, broken)| {
                    let mut t = east(from, from + len, 40.0 + from / 1000.0, -3.2, kmh);
                    if broken && t.len() > 4 {
                        let mid = t.len() / 2;
                        t[mid].new_segment = true;
                    }
                    t
                })
                .collect();
            let params = Params { route: route::Params { gap_s: gap_h * H, ..route::Params::default() }, trim_m: trim, ..Params::default() };
            let path = build(&fixes, tracks, &params).unwrap();
            check(&path, gap_h * H);
            let t = path.trim.unwrap_or_default();
            proptest::prop_assert_eq!(path.photos.len() + t.start + t.end + path.skipped, fixes.len(), "every photo placed or counted");
            proptest::prop_assert!(path.photos.windows(2).all(|w| w[0].t <= w[1].t));
            let in_stops: usize = path.stops.iter().map(|s| s.photos.len()).sum();
            proptest::prop_assert!(in_stops <= path.photos.len());
        }
    }
}
