//! Paths from the stored tracks: `photo.autoTagTracks` (positions for photos by their capture time). It reads the
//! shared `tracks` document ([`crate::tracks`]) one session at a time.

use lightcraft_geo::track::Point;
use lightcraft_meta::{Match, TrackPoint, Tracklog};
use serde_json::{Value, json};

use super::manage::{Found, apply_tags, photos_to_tag};
use super::{CommandSpec, always, bad, cmd};
use crate::map::iso;
use crate::tracks as store;
use crate::{EngineError, Result, Session};

/// The most points of the stored tracks one call reads (about 4 months of a phone recording all day, every day).
const POINTS_READ_MAX: u64 = 4_000_000;

fn none_stored() -> EngineError {
    EngineError::Other("no such track: none stored".into())
}

fn no_such(session: &str) -> EngineError {
    EngineError::Other(format!("no such track: {}", session.chars().take(40).collect::<String>()))
}

fn too_many_points(n: u64) -> EngineError {
    EngineError::Other(format!("limit: the tracks to read hold {n} points, more than {POINTS_READ_MAX} at once; name fewer (`tracks`)"))
}

/// One session's points from its chunks (in index order). A chunk that is missing, or of a newer version, is a break in
/// the recording: nothing is drawn or interpolated across it.
fn session_points(pieces: &[&Value]) -> Vec<Point> {
    let mut out: Vec<Point> = Vec::new();
    let mut follows: Option<u64> = None;
    for v in pieces {
        let index = v.get("index").and_then(Value::as_u64);
        let mut points = store::points_of(std::slice::from_ref(v));
        if let Some(first) = points.first_mut()
            && !out.is_empty()
            && (index.is_none() || index != follows)
        {
            first.new_segment = true;
        }
        follows = if points.is_empty() { None } else { index.and_then(|i| i.checked_add(1)) };
        out.extend(points);
    }
    out
}

/// A session's first and last moment (ms) and its points, from its summary.
fn span_of(session: &str, pieces: &[&Value]) -> (i64, i64, u64) {
    let s = store::summary(session, pieces);
    (s["startMs"].as_i64().unwrap_or(i64::MAX), s["endMs"].as_i64().unwrap_or(i64::MIN), s["points"].as_u64().unwrap_or(0))
}

/// `tracks`: `None` for all, else the sessions named.
fn named_tracks(p: &Value, c: &str) -> Result<Option<Vec<String>>> {
    match p.get("tracks") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(a)) if !a.is_empty() => a
            .iter()
            .map(|v| v.as_str().map(str::to_string).ok_or_else(|| bad(c, "`tracks` is a list of sessions")))
            .collect::<Result<Vec<_>>>()
            .map(Some),
        Some(_) => Err(bad(c, "`tracks` is a list of sessions")),
    }
}

/// A session's points as a GPX-like track log: a break in the recording begins a new segment.
fn tracklog_of(points: &[Point]) -> Tracklog {
    let mut segment = 0u32;
    let mut out: Vec<TrackPoint> = points
        .iter()
        .enumerate()
        .map(|(i, q)| {
            if i > 0 && q.new_segment {
                segment = segment.saturating_add(1);
            }
            TrackPoint { time: q.time_ms as f64 / 1000.0, latitude: q.lat, longitude: q.lon, elevation: q.ele_m, segment }
        })
        .collect();
    // (stored in time order; a hostile set of chunks may not be)
    out.sort_by(|a, b| a.time.total_cmp(&b.time));
    Tracklog { points: out, tracks: 1, untimed: 0 }
}

/// Where each photo taken at `times` (UTC seconds) was, from the sessions (`spans`: first and last moment in ms, and
/// points; `read(k)` gives session k's points): the place [`Tracklog::locate`] finds within `max_gap`, from the session
/// it came from. An interpolated place beats a nearest one, a nearer point a farther one; otherwise the first session
/// keeps it. A session is read only when a photo lies within its reach, and once; at most [`POINTS_READ_MAX`] points
/// are read.
fn locate_all(
    times: &[f64],
    spans: &[(i64, i64, u64)],
    max_gap: f64,
    mut read: impl FnMut(usize) -> Vec<Point>,
) -> Result<Vec<Option<(lightcraft_meta::Gps, Match, usize)>>> {
    let mut order: Vec<usize> = (0..times.len()).collect();
    let time = |i: usize| times.get(i).copied().unwrap_or(f64::NAN);
    order.sort_by(|a, b| time(*a).total_cmp(&time(*b)));
    // (the place, how, the session, how far in time from the point it came from)
    let mut best: Vec<Option<(lightcraft_meta::Gps, Match, usize, f64)>> = vec![None; times.len()];
    let mut total = 0u64;
    for (k, &(a, b, n)) in spans.iter().enumerate() {
        let lo = order.partition_point(|&i| time(i) < a as f64 / 1000.0 - max_gap);
        let hi = order.partition_point(|&i| time(i) <= b as f64 / 1000.0 + max_gap);
        let Some(near) = order.get(lo..hi).filter(|o| !o.is_empty()) else { continue };
        total = total.saturating_add(n);
        if total > POINTS_READ_MAX {
            return Err(too_many_points(total));
        }
        let log = tracklog_of(&read(k));
        for &i in near {
            let t = time(i);
            let Some((g, m)) = log.locate(t, max_gap) else { continue };
            let off = match m {
                Match::Interpolated => 0.0,
                Match::Nearest => {
                    let j = log.points.partition_point(|q| q.time < t);
                    [j.checked_sub(1), Some(j)]
                        .into_iter()
                        .flatten()
                        .filter_map(|j| log.points.get(j))
                        .map(|q| (q.time - t).abs())
                        .fold(f64::INFINITY, f64::min)
                }
            };
            if let Some(slot) = best.get_mut(i)
                && slot.as_ref().is_none_or(|(_, had, _, was)| (*had == Match::Nearest && m == Match::Interpolated) || (*had == m && off < *was))
            {
                *slot = Some((g, m, k, off));
            }
        }
    }
    Ok(best.into_iter().map(|b| b.map(|(g, m, k, _)| (g, m, k))).collect())
}

/// Geotag photos from the stored tracks by capture time: `photo.autoTagTracklog` with the tracks as the log.
fn auto_tag_tracks(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "photo.autoTagTracks";
    let named = named_tracks(p, C)?;
    let t = photos_to_tag(s, p, C)?;
    let by = store::sessions(s.tracks.items());
    if by.is_empty() {
        return Err(none_stored());
    }
    let mut chosen: Vec<(&str, &Vec<&Value>)> = match &named {
        None => by.iter().map(|(k, v)| (*k, v)).collect(),
        Some(list) => list.iter().map(|id| by.get_key_value(id.as_str()).map(|(k, v)| (*k, v)).ok_or_else(|| no_such(id))).collect::<Result<_>>()?,
    };
    // by their start, so the earliest recording is the first to offer a place
    chosen.sort_by_cached_key(|(k, v)| (span_of(k, v).0, *k));
    let spans: Vec<(i64, i64, u64)> = chosen.iter().map(|(k, v)| span_of(k, v)).collect();
    let times: Vec<f64> = t.photos.iter().map(|x| x.1).collect();
    let best = locate_all(&times, &spans, t.max_gap, |k| chosen.get(k).map(|(_, v)| session_points(v)).unwrap_or_default())?;
    let points = spans.iter().map(|x| x.2).fold(0u64, u64::saturating_add);
    let start = spans.iter().map(|x| x.0 as f64 / 1000.0).fold(f64::INFINITY, f64::min);
    let end = spans.iter().map(|x| x.1 as f64 / 1000.0).fold(f64::NEG_INFINITY, f64::max);
    let names: Vec<String> = chosen.iter().map(|(k, _)| k.to_string()).collect();
    let tracks = names.len();
    let found: Vec<Found<'_>> = best.into_iter().map(|b| b.map(|(g, m, k)| (g, m, names.get(k).map(String::as_str)))).collect();
    let mut o = apply_tags(s, p, "Auto-Tag from Tracks", &t, &found)?;
    let finite = |x: f64| if x.is_finite() { Some(x) } else { None };
    o.insert("tracks".into(), json!(tracks));
    o.insert("points".into(), json!(points));
    o.insert("start".into(), json!(finite(start)));
    o.insert("end".into(), json!(finite(end)));
    o.insert("startIso".into(), json!(finite(start).map(iso)));
    o.insert("endIso".into(), json!(finite(end).map(iso)));
    Ok(Value::Object(o))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec {
        explicit_targets: true,
        ..cmd!(
            "photo.autoTagTracks",
            "Auto-Tag Photos from Recorded Tracks",
            [],
            None,
            "{ids?, tracks?: [session] (default all stored), offset?: camera clock's UTC offset for photos whose capture time has no zone (`+02:00`, or hours; default UTC), maxGap?: seconds (600), replace?: bool (also photos that already have GPS), dryRun?: bool} → {tagged, interpolated, nearest, skipped: {noTime, outside, hasGps}, assumedUtc, tracks, points, start, end, startIso, endIso, photos: [{id, gps: [lat, lon], match: interpolated|nearest, track: session}]}: positions from the stored tracks by capture time, as photo.autoTagTracklog takes them from a GPX file: interpolated between two points of one stretch of recording at most `maxGap` apart, else the nearest point within `maxGap`; never across a break (or a chunk that is missing). Where two tracks cover a photo, an interpolated place wins. `start`/`end` are seconds since 1970 (UTC). One undo step. No stored track: `no such track: none stored`",
            always,
            auto_tag_tracks
        )
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(time_ms: i64, lat: f64, lon: f64) -> Point {
        Point { time_ms, lat, lon, ele_m: None, acc_m: None, new_segment: false }
    }

    /// `n` points a second apart from `t0` (ms), heading north.
    fn line(n: usize, t0: i64) -> Vec<Point> {
        (0..n).map(|i| pt(t0 + i as i64 * 1000, 40.0 + i as f64 * 1e-5, -3.0)).collect()
    }

    #[test]
    fn a_missing_or_unreadable_chunk_is_a_break() {
        let mut s = Session::new();
        let about = store::About { source: "phone".into(), ..store::About::default() };
        let (session, made, _) = s.add_track(&line(45_000, 1_700_000_000_000), &about).unwrap();
        assert_eq!(made.len(), 3);
        let whole = |s: &Session| {
            let by = store::sessions(s.tracks.items());
            session_points(&by[session.as_str()])
        };
        let all = whole(&s);
        assert_eq!(all.len(), 45_000);
        assert!(all.iter().all(|q| !q.new_segment), "consecutive chunks are one stretch");
        // the middle chunk gone: the third begins a new stretch, and nothing interpolates across the hole
        s.change_tracks(|items| items.retain(|v| v["index"] != 1)).unwrap();
        let holed = whole(&s);
        assert_eq!(holed.iter().filter(|q| q.new_segment).map(|q| q.time_ms).collect::<Vec<_>>(), vec![1_700_000_000_000 + 40_000 * 1000]);
        let log = tracklog_of(&holed);
        let (_, m) = log.locate(1_700_000_000.0 + 30_000.0, 1e9).unwrap();
        assert_eq!(m, Match::Nearest);
        // a chunk of a newer version in the middle: the same
        let (session2, _, _) = s.add_track(&line(45_000, 1_800_000_000_000), &about).unwrap();
        s.change_tracks(|items| {
            for v in items.iter_mut().filter(|v| v["session"] == session2.as_str() && v["index"] == 1) {
                v["fmt"] = json!(2);
            }
        })
        .unwrap();
        let by = store::sessions(s.tracks.items());
        assert_eq!(session_points(&by[session2.as_str()]).iter().filter(|q| q.new_segment).count(), 1);
    }

    #[test]
    fn locate_all_prefers_interpolated_and_reads_a_session_once() {
        // session 0 has a fix only near the photo (nearest); session 1 brackets it (interpolated)
        let s0 = vec![pt(1_000_500, 10.0, 10.0)];
        let s1 = vec![pt(990_000, 20.0, 20.0), pt(1_010_000, 20.0, 20.002)];
        let spans = [(1_000_500, 1_000_500, 1), (990_000, 1_010_000, 2), (5_000_000_000, 5_000_000_000, 1)];
        let mut reads = vec![0; 3];
        let tracks = [s0, s1, vec![pt(5_000_000_000, 0.0, 0.0)]];
        let best = locate_all(&[1000.0, 1001.0, f64::NAN], &spans, 600.0, |k| {
            reads[k] += 1;
            tracks[k].clone()
        })
        .unwrap();
        assert_eq!(reads, vec![1, 1, 0], "each read once; the far one never");
        let (g, m, k) = best[0].unwrap();
        assert_eq!((m, k), (Match::Interpolated, 1));
        assert!((g.longitude - 20.001).abs() < 1e-9);
        assert!(best[2].is_none(), "a time that isn't one matches nothing");
        // too much to read: refused
        let e = locate_all(&[0.0], &[(0, 0, POINTS_READ_MAX + 1)], 600.0, |_| Vec::new()).unwrap_err().to_string();
        assert!(e.starts_with("limit: "), "{e}");
    }

    #[test]
    fn a_hundred_thousand_photos_and_a_year_of_tracks() {
        // a session every day for a year (8 hours, a point every 30 s), a photo every 5 minutes
        let day = 86_400_000i64;
        let spans: Vec<(i64, i64, u64)> = (0..365).map(|d| (d * day, d * day + 8 * 3_600_000, 961)).collect();
        let times: Vec<f64> = (0..100_000).map(|i| i as f64 * 300.0).collect();
        let started = std::time::Instant::now();
        let mut reads = 0;
        let best = locate_all(&times, &spans, 600.0, |k| {
            reads += 1;
            (0..961).map(|i| pt(k as i64 * day + i * 30_000, 40.0, -3.0 + i as f64 * 1e-4)).collect()
        })
        .unwrap();
        assert!(started.elapsed().as_secs() < 60);
        assert_eq!(reads, 348, "the days the photos reach");
        let tagged = best.iter().filter(|b| b.is_some()).count();
        assert!(tagged > 30_000 && tagged < 40_000, "about a third of each day: {tagged}");
    }
}
