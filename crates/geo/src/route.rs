//! Estimated routes: a smooth path through photos that already have a position, by the time they
//! were taken. Nothing is recorded, so nothing here is a measurement: it is the most plausible line
//! through what the photos say, and callers should show it as an estimate.
//!
//! 1. **Order** the photos by time and cut the sequence wherever two photos are further apart in
//!    time than [`Params::gap_s`]: a trip and the next one are different routes, never joined by a
//!    line across the weeks between them.
//! 2. **Stops:** photos taken one after another within [`Params::stop_radius_m`] of a stop's centre
//!    are one stop.
//! 3. **Legs** join consecutive stops. A leg has its distance, the time between the last photo of
//!    one stop and the first of the next, the speed that implies (a lower bound: the person may
//!    have lingered), a [`Mode`] guessed from it, and a line: a great-circle arc for a flight, a
//!    long leg or the polar regions, else a centripetal Catmull-Rom spline through the stops in Web
//!    Mercator (so a leg between two stops is straight and a longer trail bends smoothly through
//!    its stops). Lines are cut where they cross the antimeridian, so a client never draws one
//!    across the whole map.
//!
//! Everything is bounded: the lines of one answer hold at most about [`MAX_POINTS`] points whatever
//! the number of photos, and positions or times that aren't real are left out and counted.

use crate::geodesy::{World, distance_km, project, unproject, valid};

/// Photos within this distance (metres) of a stop's centre belong to that stop.
pub const STOP_RADIUS_M: f64 = 150.0;
/// A pause between two photos longer than this (36 hours) ends a route.
pub const GAP_S: f64 = 36.0 * 3600.0;
/// The most intervals one leg's line is cut into.
pub const DETAIL: usize = 48;
/// A leg at least this long (kilometres) is drawn as a great-circle arc.
pub const ARC_KM: f64 = 300.0;
/// The lines of one answer together hold about this many points at most (more only when there are
/// more legs than half of it).
pub const MAX_POINTS: usize = 60_000;
/// Latitude beyond which Web Mercator stretches too far: legs there are arcs.
const POLAR_LAT: f64 = 80.0;
/// The time a leg is taken to last at least when its speed is worked out (seconds), so photos
/// taken at the same moment in two places don't make an infinite speed.
const MIN_LEG_S: f64 = 60.0;

/// Slower than this (km/h) is walking.
pub const WALK_MAX_KMH: f64 = 6.0;
/// Slower than this (km/h) is cycling.
pub const BIKE_MAX_KMH: f64 = 20.0;
/// A leg this long (km) at least, and this fast (km/h) at least, is a flight.
pub const FLIGHT_MIN_KM: f64 = 200.0;
pub const FLIGHT_MIN_KMH: f64 = 150.0;

/// One photo's position at a moment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fix {
    /// The caller's id for the photo.
    pub id: u64,
    /// Seconds since 1970 (UTC, or an estimate of it: only the order and the gaps matter).
    pub time: f64,
    pub lat: f64,
    pub lon: f64,
}

/// What the estimate may be tuned with; each is limited to a sane range by [`estimate`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    pub stop_radius_m: f64,
    pub gap_s: f64,
    /// The most intervals one leg's line is cut into (2..=256).
    pub detail: usize,
    pub arc_km: f64,
}

impl Default for Params {
    fn default() -> Self {
        Params { stop_radius_m: STOP_RADIUS_M, gap_s: GAP_S, detail: DETAIL, arc_km: ARC_KM }
    }
}

impl Params {
    pub(crate) fn sane(self) -> Params {
        let fin = |v: f64, lo: f64, hi: f64, d: f64| if v.is_finite() { v.clamp(lo, hi) } else { d };
        Params {
            stop_radius_m: fin(self.stop_radius_m, 0.0, 100_000.0, STOP_RADIUS_M),
            gap_s: fin(self.gap_s, 1.0, 1e9, GAP_S),
            detail: self.detail.clamp(2, 256),
            arc_km: fin(self.arc_km, 0.0, 1e6, ARC_KM),
        }
    }
}

/// How a leg was probably travelled, from its speed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Walk,
    Bike,
    Road,
    Flight,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Walk => "walk",
            Mode::Bike => "bike",
            Mode::Road => "road",
            Mode::Flight => "flight",
        }
    }
}

/// The mode a leg of `distance_km` at `speed_kmh` suggests. The speed is a lower bound (the time
/// between photos includes any stay), so this leans towards the slower modes.
pub fn mode_of(distance_km: f64, speed_kmh: f64) -> Mode {
    if distance_km >= FLIGHT_MIN_KM && speed_kmh >= FLIGHT_MIN_KMH {
        Mode::Flight
    } else if speed_kmh < WALK_MAX_KMH {
        Mode::Walk
    } else if speed_kmh < BIKE_MAX_KMH {
        Mode::Bike
    } else {
        Mode::Road
    }
}

/// A place where photos were taken one after another.
#[derive(Clone, Debug, PartialEq)]
pub struct Stop {
    pub lat: f64,
    pub lon: f64,
    /// Time of the first photo here.
    pub arrive: f64,
    /// Time of the last photo here.
    pub leave: f64,
    /// The photos taken here, in time order.
    pub photos: Vec<u64>,
}

/// The way from one stop to the next.
#[derive(Clone, Debug, PartialEq)]
pub struct Leg {
    /// Indices into the route's stops.
    pub from: usize,
    pub to: usize,
    pub distance_km: f64,
    /// From the last photo at `from` to the first at `to`, seconds (never negative).
    pub duration_s: f64,
    pub speed_kmh: f64,
    pub mode: Mode,
    /// A great-circle arc rather than a spline.
    pub arc: bool,
    /// The line as `(lat, lon)` points, in more than one piece only where it crosses the
    /// antimeridian (a piece ends at longitude ±180 and the next begins at ∓180).
    pub lines: Vec<Vec<(f64, f64)>>,
}

/// A run of photos with no long pause in it.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Route {
    pub stops: Vec<Stop>,
    /// One fewer than the stops.
    pub legs: Vec<Leg>,
}

impl Route {
    pub fn distance_km(&self) -> f64 {
        self.legs.iter().map(|l| l.distance_km).sum()
    }

    /// The time of the first photo.
    pub fn start(&self) -> f64 {
        self.stops.first().map_or(0.0, |s| s.arrive)
    }

    /// The time of the last photo.
    pub fn end(&self) -> f64 {
        self.stops.last().map_or(0.0, |s| s.leave)
    }
}

/// The answer of [`estimate`].
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Routes {
    /// In time order.
    pub routes: Vec<Route>,
    /// Photos left out: a time or a position that isn't real.
    pub skipped: usize,
}

/// A longitude difference taken the short way round, in degrees, `-180..180`.
fn wrap_deg(d: f64) -> f64 {
    (d + 540.0).rem_euclid(360.0) - 180.0
}

/// A difference on the world square taken the short way round, `-0.5..=0.5`.
fn wrap_unit(d: f64) -> f64 {
    d - d.round()
}

/// A stop being built: photos so far, averaged.
struct Building {
    lat_sum: f64,
    /// Sum of the photos' longitudes relative to the first photo's (each the short way round).
    dlon_sum: f64,
    lon0: f64,
    n: f64,
    arrive: f64,
    leave: f64,
    photos: Vec<u64>,
}

impl Building {
    fn new(f: &Fix) -> Building {
        Building { lat_sum: f.lat, dlon_sum: 0.0, lon0: f.lon, n: 1.0, arrive: f.time, leave: f.time, photos: vec![f.id] }
    }

    fn centre(&self) -> (f64, f64) {
        let lon = self.lon0 + self.dlon_sum / self.n;
        // a lone photo's stop is exactly where the photo is
        (self.lat_sum / self.n, if (-180.0..=180.0).contains(&lon) { lon } else { wrap_deg(lon) })
    }

    fn add(&mut self, f: &Fix) {
        self.lat_sum += f.lat;
        self.dlon_sum += wrap_deg(f.lon - self.lon0);
        self.n += 1.0;
        self.leave = f.time;
        self.photos.push(f.id);
    }

    fn finish(self) -> Stop {
        let (lat, lon) = self.centre();
        Stop { lat, lon, arrive: self.arrive, leave: self.leave, photos: self.photos }
    }
}

/// Photos taken one after another within `radius_km` of where the stop is so far are one stop.
fn stops_of(run: &[Fix], radius_km: f64) -> Vec<Stop> {
    let mut out: Vec<Stop> = Vec::new();
    let mut cur: Option<Building> = None;
    for f in run {
        let joins = cur.as_ref().is_some_and(|b| {
            let (la, lo) = b.centre();
            distance_km(la, lo, f.lat, f.lon) <= radius_km
        });
        if joins {
            if let Some(b) = cur.as_mut() {
                b.add(f);
            }
        } else {
            if let Some(b) = cur.take() {
                out.push(b.finish());
            }
            cur = Some(Building::new(f));
        }
    }
    if let Some(b) = cur {
        out.push(b.finish());
    }
    out
}

/// The legs between consecutive stops, with their lines still to be drawn.
fn legs_of(stops: &[Stop], p: &Params) -> Vec<Leg> {
    stops
        .windows(2)
        .enumerate()
        .filter_map(|(i, pair)| {
            let [a, b] = pair else { return None };
            let distance = distance_km(a.lat, a.lon, b.lat, b.lon);
            let duration = (b.arrive - a.leave).max(0.0);
            let speed = distance / (duration.max(MIN_LEG_S) / 3600.0);
            let mode = mode_of(distance, speed);
            let arc = mode == Mode::Flight || distance >= p.arc_km || a.lat.abs() > POLAR_LAT || b.lat.abs() > POLAR_LAT;
            Some(Leg { from: i, to: i + 1, distance_km: distance, duration_s: duration, speed_kmh: speed, mode, arc, lines: Vec::new() })
        })
        .collect()
}

/// A great-circle arc from `a` to `b` in `n` intervals (`n + 1` points, the ends exact).
fn arc(a: (f64, f64), b: (f64, f64), n: usize) -> Vec<(f64, f64)> {
    let unit = |(lat, lon): (f64, f64)| {
        let (la, lo) = (lat.to_radians(), lon.to_radians());
        [la.cos() * lo.cos(), la.cos() * lo.sin(), la.sin()]
    };
    let (va, vb) = (unit(a), unit(b));
    let dot = (va[0] * vb[0] + va[1] * vb[1] + va[2] * vb[2]).clamp(-1.0, 1.0);
    let omega = dot.acos();
    let s = omega.sin();
    // the same point, or opposite ones (no single great circle): a straight line says as much as can be said
    if s.abs() < 1e-9 {
        return straight(a, b, n);
    }
    let mut out: Vec<(f64, f64)> = (0..=n)
        .map(|k| {
            let t = k as f64 / n as f64;
            let (k1, k2) = (((1.0 - t) * omega).sin() / s, (t * omega).sin() / s);
            let v = [k1 * va[0] + k2 * vb[0], k1 * va[1] + k2 * vb[1], k1 * va[2] + k2 * vb[2]];
            (v[2].clamp(-1.0, 1.0).asin().to_degrees(), v[1].atan2(v[0]).to_degrees())
        })
        .collect();
    pin_ends(&mut out, a, b);
    out
}

/// A straight line in latitude and longitude (the short way round), `n` intervals.
fn straight(a: (f64, f64), b: (f64, f64), n: usize) -> Vec<(f64, f64)> {
    let dlon = wrap_deg(b.1 - a.1);
    let mut out: Vec<(f64, f64)> = (0..=n)
        .map(|k| {
            let t = k as f64 / n as f64;
            (a.0 + (b.0 - a.0) * t, wrap_deg(a.1 + dlon * t))
        })
        .collect();
    pin_ends(&mut out, a, b);
    out
}

/// The ends of a line are the stops themselves, not what the arithmetic made of them.
fn pin_ends(line: &mut [(f64, f64)], a: (f64, f64), b: (f64, f64)) {
    if let Some(first) = line.first_mut() {
        *first = a;
    }
    if let Some(last) = line.last_mut() {
        *last = b;
    }
}

fn mix(p: World, q: World, w: f64) -> World {
    World { x: p.x + (q.x - p.x) * w, y: p.y + (q.y - p.y) * w }
}

/// A centripetal Catmull-Rom spline from stop `a` to stop `b` in Web Mercator, bending towards the
/// stops before and after them (without one, the line leaves along the chord), `n` intervals.
fn curve(prev: Option<&Stop>, a: &Stop, b: &Stop, next: Option<&Stop>, n: usize) -> Vec<(f64, f64)> {
    let (pa, pb0) = (project(a.lat, a.lon), project(b.lat, b.lon));
    // the copy of each neighbour nearest to the one it is next to, so a trail across the antimeridian stays in one piece
    let near = |anchor: f64, w: World| World { x: anchor + wrap_unit(w.x - anchor), y: w.y };
    let pb = near(pa.x, pb0);
    let p0 = prev.map_or(World { x: 2.0 * pa.x - pb.x, y: 2.0 * pa.y - pb.y }, |s| near(pa.x, project(s.lat, s.lon)));
    let p3 = next.map_or(World { x: 2.0 * pb.x - pa.x, y: 2.0 * pb.y - pa.y }, |s| near(pb.x, project(s.lat, s.lon)));
    let dist = |p: World, q: World| ((q.x - p.x).powi(2) + (q.y - p.y).powi(2)).sqrt();
    let t1 = dist(p0, pa).sqrt();
    let t2 = t1 + dist(pa, pb).sqrt();
    let t3 = t2 + dist(pb, p3).sqrt();
    // coinciding points would divide by zero: say it with a straight line
    if [t1, t2 - t1, t3 - t2, t2, t3 - t1].iter().any(|d| !d.is_finite() || *d < 1e-9) {
        return straight((a.lat, a.lon), (b.lat, b.lon), n);
    }
    let mut out: Vec<(f64, f64)> = (0..=n)
        .map(|k| {
            let t = t1 + (t2 - t1) * k as f64 / n as f64;
            let a1 = mix(p0, pa, t / t1);
            let a2 = mix(pa, pb, (t - t1) / (t2 - t1));
            let a3 = mix(pb, p3, (t - t2) / (t3 - t2));
            let b1 = mix(a1, a2, t / t2);
            let b2 = mix(a2, a3, (t - t1) / (t3 - t1));
            unproject(mix(b1, b2, (t - t1) / (t2 - t1)))
        })
        .collect();
    pin_ends(&mut out, (a.lat, a.lon), (b.lat, b.lon));
    out
}

/// Cut a line (longitudes `-180..=180`) where it jumps across the antimeridian, so no piece spans the
/// map; the pieces meet at the crossing latitude. Pieces of a single point are dropped.
fn split_antimeridian(points: Vec<(f64, f64)>) -> Vec<Vec<(f64, f64)>> {
    split_timed(points.into_iter().map(|(la, lo)| (la, lo, 0.0)).collect())
        .into_iter()
        .map(|piece| piece.into_iter().map(|(la, lo, _)| (la, lo)).collect())
        .collect()
}

/// [`split_antimeridian`] for `(lat, lon, time)` points: a crossing's time lies between its neighbours' like its latitude.
pub(crate) fn split_timed(points: Vec<(f64, f64, f64)>) -> Vec<Vec<(f64, f64, f64)>> {
    let mut out: Vec<Vec<(f64, f64, f64)>> = Vec::new();
    let mut cur: Vec<(f64, f64, f64)> = Vec::new();
    for p in points {
        if let Some(&(la, lo, at)) = cur.last()
            && (p.1 - lo).abs() > 180.0
        {
            // the other copy of this point, on the far side of the crossing
            let (edge_from, edge_to, far) = if lo > 0.0 { (180.0, -180.0, p.1 + 360.0) } else { (-180.0, 180.0, p.1 - 360.0) };
            let t = ((edge_from - lo) / (far - lo)).clamp(0.0, 1.0);
            let (lat, time) = (la + t * (p.0 - la), at + t * (p.2 - at));
            cur.push((lat, edge_from, time));
            out.push(std::mem::take(&mut cur));
            cur.push((lat, edge_to, time));
        }
        cur.push(p);
    }
    out.push(cur);
    out.retain(|piece| piece.len() >= 2);
    out
}

/// Estimate the routes through `fixes` (any order). Fixes whose time or position isn't real are left
/// out and counted in [`Routes::skipped`].
pub fn estimate(fixes: &[Fix], params: &Params) -> Routes {
    estimate_runs(&[fixes], params)
}

/// [`estimate`] through each of `runs` apart: a route never joins two runs (each is still cut at long pauses), and
/// the runs share one budget of points. The routes come run by run.
pub fn estimate_runs(runs: &[&[Fix]], params: &Params) -> Routes {
    let p = params.sane();
    let radius_km = p.stop_radius_m / 1000.0;
    let (mut routes, mut skipped) = (Vec::new(), 0usize);
    for fixes in runs {
        let mut v: Vec<Fix> = fixes.iter().copied().filter(|f| f.time.is_finite() && valid(f.lat, f.lon)).collect();
        skipped += fixes.len() - v.len();
        v.sort_by(|a, b| a.time.total_cmp(&b.time).then(a.id.cmp(&b.id)));
        routes.extend(v.chunk_by(|a, b| b.time - a.time <= p.gap_s).map(|run| {
            let stops = stops_of(run, radius_km);
            let legs = legs_of(&stops, &p);
            Route { stops, legs }
        }));
    }
    // the budget of points: more legs than it allows get coarser lines
    let legs_total: usize = routes.iter().map(|r| r.legs.len()).sum();
    let cap = MAX_POINTS.checked_div(legs_total).map_or(p.detail, |each| each.saturating_sub(1).max(1).min(p.detail));
    for route in &mut routes {
        let Route { stops, legs } = route;
        for leg in legs.iter_mut() {
            let (Some(a), Some(b)) = (stops.get(leg.from), stops.get(leg.to)) else { continue };
            let want = if leg.arc { ((leg.distance_km / 25.0).ceil() as usize).max(8) } else { ((leg.distance_km * 2.0).ceil() as usize).max(4) };
            let n = want.min(cap).max(1);
            let line = if leg.arc {
                arc((a.lat, a.lon), (b.lat, b.lon), n)
            } else {
                let prev = leg.from.checked_sub(1).and_then(|i| stops.get(i));
                curve(prev, a, b, stops.get(leg.to + 1), n)
            };
            leg.lines = split_antimeridian(line);
        }
    }
    Routes { routes, skipped }
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: f64 = 3600.0;

    fn fix(id: u64, hours: f64, lat: f64, lon: f64) -> Fix {
        Fix { id, time: 1_700_000_000.0 + hours * H, lat, lon }
    }

    fn all_points(r: &Routes) -> impl Iterator<Item = (f64, f64)> + '_ {
        r.routes.iter().flat_map(|r| r.legs.iter()).flat_map(|l| l.lines.iter()).flatten().copied()
    }

    #[test]
    fn two_photos_make_one_leg_whose_mode_follows_its_speed() {
        // 5 km apart: an hour is a walk, ten minutes is a drive
        let walk = estimate(&[fix(1, 0.0, 40.0, -3.0), fix(2, 1.0, 40.045, -3.0)], &Params::default());
        let leg = &walk.routes[0].legs[0];
        assert_eq!((walk.routes.len(), walk.routes[0].stops.len(), walk.routes[0].legs.len()), (1, 2, 1));
        assert!((leg.distance_km - 5.0).abs() < 0.1, "{}", leg.distance_km);
        assert_eq!(leg.mode, Mode::Walk);
        assert!(!leg.arc);
        let drive = estimate(&[fix(1, 0.0, 40.0, -3.0), fix(2, 10.0 / 60.0, 40.045, -3.0)], &Params::default());
        assert_eq!(drive.routes[0].legs[0].mode, Mode::Road);
        assert_eq!(mode_of(10.0, 12.0), Mode::Bike);
    }

    #[test]
    fn photos_at_one_place_are_one_stop_in_time_order() {
        // given out of order, three photos within 50 m, then one far away
        let r =
            estimate(&[fix(3, 0.2, 40.0003, -3.0), fix(9, 2.0, 41.0, -3.0), fix(1, 0.0, 40.0, -3.0), fix(2, 0.1, 40.0, -3.0002)], &Params::default());
        let route = &r.routes[0];
        assert_eq!(route.stops.len(), 2);
        assert_eq!(route.stops[0].photos, vec![1, 2, 3]);
        assert_eq!(route.stops[1].photos, vec![9]);
        assert_eq!((route.stops[0].arrive, route.stops[0].leave), (fix(1, 0.0, 0.0, 0.0).time, fix(3, 0.2, 0.0, 0.0).time));
        assert!((route.stops[0].lat - 40.0).abs() < 0.001, "the stop sits among its photos");
        assert_eq!(route.legs.len(), 1);
    }

    #[test]
    fn a_long_pause_ends_the_route() {
        let fixes = [fix(1, 0.0, 40.0, -3.0), fix(2, 5.0, 40.1, -3.0), fix(3, 5.0 + 40.0, 48.0, 2.0), fix(4, 5.0 + 41.0, 48.1, 2.0)];
        let r = estimate(&fixes, &Params::default());
        assert_eq!(r.routes.len(), 2, "40 hours is more than the 36 allowed");
        assert!(r.routes.iter().all(|r| r.legs.len() == 1), "no leg crosses the pause");
        let joined = estimate(&fixes, &Params { gap_s: 48.0 * H, ..Params::default() });
        assert_eq!((joined.routes.len(), joined.routes[0].legs.len()), (1, 3));
    }

    #[test]
    fn a_long_fast_leg_is_a_flight_on_a_great_circle() {
        // Madrid to New York in eight hours: the great circle bows north of both
        let r = estimate(&[fix(1, 0.0, 40.47, -3.56), fix(2, 8.0, 40.64, -73.78)], &Params::default());
        let leg = &r.routes[0].legs[0];
        assert_eq!(leg.mode, Mode::Flight);
        assert!(leg.arc);
        assert!((leg.distance_km - 5760.0).abs() < 60.0, "{}", leg.distance_km);
        let line = &leg.lines[0];
        assert!(line.len() > 10);
        assert_eq!(line.first(), Some(&(40.47, -3.56)));
        assert_eq!(line.last(), Some(&(40.64, -73.78)));
        let top = line.iter().map(|p| p.0).fold(f64::MIN, f64::max);
        assert!(top > 44.0, "the arc bows towards the pole: {top}");
        // every point is on the same great circle: no longer than the arc plus a little
        let length: f64 = line.windows(2).map(|w| distance_km(w[0].0, w[0].1, w[1].0, w[1].1)).sum();
        assert!((length - leg.distance_km).abs() < 5.0, "{length} vs {}", leg.distance_km);
    }

    #[test]
    fn a_trail_bends_smoothly_through_its_stops() {
        // a right-angle turn: the line goes through the middle stop and cuts the corner no further than the stops
        let r = estimate(&[fix(1, 0.0, 40.00, -3.00), fix(2, 1.0, 40.00, -2.90), fix(3, 2.0, 40.09, -2.90)], &Params::default());
        let route = &r.routes[0];
        assert_eq!(route.legs.len(), 2);
        let corner = (route.stops[1].lat, route.stops[1].lon);
        assert_eq!(route.legs[0].lines[0].last(), Some(&corner));
        assert_eq!(route.legs[1].lines[0].first(), Some(&corner));
        // the first leg leaves the first stop on the chord but turns towards the next stop at the end:
        // it is not a straight line, so it leaves the equator-parallel chord
        let off: f64 = route.legs[0].lines[0].iter().map(|p| (p.0 - 40.0).abs()).fold(0.0, f64::max);
        assert!(off > 0.001, "bends: {off}");
        // a leg between only two stops is straight
        let two = estimate(&[fix(1, 0.0, 40.0, -3.0), fix(2, 1.0, 40.0, -2.9)], &Params::default());
        assert!(two.routes[0].legs[0].lines[0].iter().all(|p| (p.0 - 40.0).abs() < 1e-9));
    }

    #[test]
    fn a_line_across_the_antimeridian_comes_in_two_pieces() {
        for (a, b) in [((0.0, 179.0), (0.0, -179.0)), ((0.0, -179.0), (0.0, 179.0))] {
            let r = estimate(&[fix(1, 0.0, a.0, a.1), fix(2, 1.0, b.0, b.1)], &Params::default());
            let leg = &r.routes[0].legs[0];
            assert_eq!(leg.lines.len(), 2, "{a:?} to {b:?}");
            let (first, second) = (&leg.lines[0], &leg.lines[1]);
            assert_eq!(first.first(), Some(&a));
            assert_eq!(second.last(), Some(&b));
            let (end, start) = (first.last().map(|p| p.1), second.first().map(|p| p.1));
            assert_eq!((end.map(f64::abs), start.map(f64::abs)), (Some(180.0), Some(180.0)));
            assert_ne!(end, start, "the pieces meet on opposite edges");
            assert!(all_points(&r).all(|p| valid(p.0, p.1)));
            assert!(leg.distance_km < 250.0, "the short way round: {}", leg.distance_km);
        }
        // a flight across it too
        let flight = estimate(&[fix(1, 0.0, 35.0, 139.0), fix(2, 9.0, 37.0, -122.0)], &Params::default());
        assert_eq!(flight.routes[0].legs[0].lines.len(), 2);
        assert!(all_points(&flight).all(|p| valid(p.0, p.1)));
    }

    #[test]
    fn what_is_not_real_is_left_out_and_counted() {
        let bad = [
            fix(1, 0.0, 40.0, -3.0),
            Fix { id: 2, time: f64::NAN, lat: 40.0, lon: -3.0 },
            Fix { id: 3, time: 1.0, lat: f64::NAN, lon: -3.0 },
            fix(4, 1.0, 91.0, 0.0),
            fix(5, 1.0, 0.0, 181.0),
            Fix { id: 6, time: f64::INFINITY, lat: 0.0, lon: 0.0 },
            fix(7, 2.0, 40.5, -3.0),
        ];
        let r = estimate(&bad, &Params::default());
        assert_eq!(r.skipped, 5);
        assert_eq!(r.routes[0].stops.iter().map(|s| s.photos.len()).sum::<usize>(), 2);
        assert_eq!(estimate(&[], &Params::default()), Routes::default());
        let one = estimate(&[fix(1, 0.0, 10.0, 10.0)], &Params::default());
        assert_eq!((one.routes.len(), one.routes[0].stops.len(), one.routes[0].legs.len()), (1, 1, 0));
    }

    #[test]
    fn the_input_order_does_not_matter() {
        let mut fixes: Vec<Fix> = (0..30).map(|i| fix(i, i as f64 * 0.4, 40.0 + (i as f64 * 0.37).sin() * 0.2, -3.0 + i as f64 * 0.01)).collect();
        let a = estimate(&fixes, &Params::default());
        fixes.reverse();
        assert_eq!(a, estimate(&fixes, &Params::default()));
    }

    #[test]
    fn the_same_moment_in_two_places_is_not_an_infinite_speed() {
        let r = estimate(&[fix(1, 0.0, 40.0, -3.0), fix(2, 0.0, 41.0, -3.0)], &Params::default());
        let leg = &r.routes[0].legs[0];
        assert!(leg.speed_kmh.is_finite() && leg.duration_s == 0.0, "{leg:?}");
    }

    #[test]
    fn photos_at_the_same_moment_keep_a_fixed_order() {
        let r = estimate(&[fix(2, 0.0, 40.0, -3.0), fix(1, 0.0, 40.0, -3.0), fix(3, 0.0, 40.0, -3.0)], &Params::default());
        assert_eq!(r.routes[0].stops[0].photos, vec![1, 2, 3]);
    }

    #[test]
    fn a_stop_across_the_antimeridian_is_on_it_not_on_the_other_side_of_the_world() {
        let r = estimate(&[fix(1, 0.0, 0.0, 179.9), fix(2, 0.1, 0.0, -179.9)], &Params { stop_radius_m: 50_000.0, ..Params::default() });
        let stop = &r.routes[0].stops[0];
        assert_eq!(stop.photos.len(), 2);
        assert!(stop.lon.abs() > 179.9, "{}", stop.lon);
        assert_eq!((wrap_deg(190.0), wrap_deg(-190.0), wrap_deg(0.0)), (-170.0, 170.0, 0.0));
        assert!((wrap_unit(0.9) + 0.1).abs() < 1e-12 && (wrap_unit(-0.7) - 0.3).abs() < 1e-12);
    }

    #[test]
    fn a_pole_and_coinciding_stops_are_drawn() {
        // beyond Mercator's reach: an arc; the same place twice with a zero stop radius: one stop
        let polar = estimate(&[fix(1, 0.0, 89.0, 0.0), fix(2, 1.0, 88.0, 90.0)], &Params::default());
        assert!(polar.routes[0].legs[0].arc && all_points(&polar).all(|p| valid(p.0, p.1)));
        let same = estimate(&[fix(1, 0.0, 10.0, 10.0), fix(2, 1.0, 10.0, 10.0)], &Params { stop_radius_m: 0.0, ..Params::default() });
        assert_eq!(same.routes[0].stops.len(), 1);
        // antipodes have no single great circle: a line is still given
        let anti = estimate(&[fix(1, 0.0, 0.0, 0.0), fix(2, 20.0, 0.0, 180.0)], &Params::default());
        assert!(!anti.routes[0].legs[0].lines.is_empty());
    }

    #[test]
    fn many_legs_share_the_point_budget() {
        // 3000 stops 10 km apart: lines get coarser instead of the answer growing without bound
        let fixes: Vec<Fix> = (0..3000).map(|i| fix(i, i as f64 * 0.2, 40.0 + (i as f64 % 50.0) * 0.09, -3.0 + (i / 50) as f64 * 0.12)).collect();
        let r = estimate(&fixes, &Params::default());
        let legs: usize = r.routes.iter().map(|r| r.legs.len()).sum();
        let points = all_points(&r).count();
        assert!(legs > 2900, "{legs}");
        assert!(points <= MAX_POINTS && points > MAX_POINTS * 8 / 10, "the budget is used, not exceeded: {points}");
        let few = estimate(&fixes[..3], &Params::default());
        assert!(all_points(&few).count() > 2 * 12, "a few legs keep their detail");
    }

    #[test]
    fn runs_are_never_joined_and_share_one_budget() {
        let a = [fix(1, 0.0, 40.0, -3.0), fix(2, 1.0, 40.1, -3.0)];
        let b = [fix(3, 2.0, 40.2, -3.0), fix(4, 3.0, 40.3, -3.0)];
        let apart = estimate_runs(&[&a, &b], &Params::default());
        assert_eq!(apart.routes.iter().map(|r| r.legs.len()).collect::<Vec<_>>(), vec![1, 1], "no leg from one run to the next");
        assert_eq!(estimate(&[a, b].concat(), &Params::default()).routes[0].legs.len(), 3);
        // two runs of 2000 legs each: the lines of both together stay within the budget
        let run = |from: u64| -> Vec<Fix> {
            (from..from + 2001).map(|i| fix(i, i as f64, 40.0 + (i % 50) as f64 * 0.09, -3.0 + (i / 50) as f64 * 0.1)).collect()
        };
        let (x, y) = (run(0), run(10_000));
        let r = estimate_runs(&[&x, &y], &Params::default());
        let points = all_points(&r).count();
        assert!(points <= MAX_POINTS && points > MAX_POINTS * 8 / 10, "{points}");
    }

    #[test]
    fn parameters_are_kept_in_range() {
        let wild = Params { stop_radius_m: f64::NAN, gap_s: -5.0, detail: 0, arc_km: f64::INFINITY };
        let r = estimate(&[fix(1, 0.0, 40.0, -3.0), fix(2, 1.0, 40.5, -3.0)], &wild);
        assert!(!r.routes.is_empty() && all_points(&r).all(|p| valid(p.0, p.1)));
        let huge = Params { detail: usize::MAX, ..Params::default() };
        assert!(all_points(&estimate(&[fix(1, 0.0, 40.0, -3.0), fix(2, 1.0, 40.5, -3.0)], &huge)).count() <= 257);
    }

    proptest::proptest! {
        #[test]
        fn any_photos_give_a_consistent_finite_answer(raw in proptest::collection::vec((0u64..500, -1e6f64..1e6, -100.0f64..100.0, -200.0f64..200.0), 0..80), gap in 1.0f64..1e6, radius in 0.0f64..5000.0) {
            let fixes: Vec<Fix> = raw.iter().map(|&(id, time, lat, lon)| Fix { id, time, lat, lon }).collect();
            let r = estimate(&fixes, &Params { gap_s: gap, stop_radius_m: radius, ..Params::default() });
            let kept: usize = r.routes.iter().flat_map(|r| r.stops.iter()).map(|s| s.photos.len()).sum();
            proptest::prop_assert_eq!(kept + r.skipped, fixes.len());
            for route in &r.routes {
                proptest::prop_assert!(!route.stops.is_empty());
                proptest::prop_assert_eq!(route.legs.len() + 1, route.stops.len());
                proptest::prop_assert!(route.stops.windows(2).all(|w| w[0].leave <= w[1].arrive));
                for s in &route.stops {
                    proptest::prop_assert!(s.arrive <= s.leave && valid(s.lat, s.lon));
                }
                for (i, leg) in route.legs.iter().enumerate() {
                    proptest::prop_assert_eq!((leg.from, leg.to), (i, i + 1));
                    proptest::prop_assert!(leg.distance_km.is_finite() && leg.duration_s >= 0.0 && leg.speed_kmh.is_finite());
                    proptest::prop_assert!(!leg.lines.is_empty());
                    for piece in &leg.lines {
                        proptest::prop_assert!(piece.len() >= 2 && piece.iter().all(|p| valid(p.0, p.1)));
                    }
                }
            }
        }
    }
}
