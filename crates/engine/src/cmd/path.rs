//! Paths from the stored tracks: `photo.autoTagTracks` (positions for photos by their capture time) and `map.path`
//! (one path for a selection, recorded where a track covers the time and estimated elsewhere,
//! [`lightcraft_geo::path`]). Both read the shared `tracks` document ([`crate::tracks`]) one session at a time.

use lightcraft_catalog::{AlbumId, PhotoId};
use lightcraft_geo::path::{self as geo_path, Kind, Path};
use lightcraft_geo::route::Mode;
use lightcraft_geo::track::Point;
use lightcraft_meta::{Match, TrackPoint, Tracklog};
use serde_json::{Value, json};

use super::manage::{Found, apply_tags, photos_to_tag};
use super::{CommandSpec, always, bad, cmd, f64_or};
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
    EngineError::Other(format!(
        "limit: the tracks to read hold {n} points, more than {POINTS_READ_MAX} at once; name fewer (`tracks`) or a shorter time (`fromMs`/`toMs`)"
    ))
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

/// `tracks`: `None` for all (or `"auto"` where the command takes it), else the sessions named.
fn named_tracks(p: &Value, c: &str, auto: bool) -> Result<Option<Vec<String>>> {
    match p.get("tracks") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(a)) if auto && a == "auto" => Ok(None),
        Some(Value::Array(a)) if !a.is_empty() => a
            .iter()
            .map(|v| v.as_str().map(str::to_string).ok_or_else(|| bad(c, "`tracks` is a list of sessions")))
            .collect::<Result<Vec<_>>>()
            .map(Some),
        Some(_) => Err(bad(c, if auto { "`tracks` is \"auto\" or a list of sessions" } else { "`tracks` is a list of sessions" })),
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
    let named = named_tracks(p, C, false)?;
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

/// What a path is drawn for: photos, which tracks (`None`: those that overlap), a window (ms) and the modes a trip
/// gives some of its legs (`(seconds the leg leaves, mode)`).
struct Selection {
    photos: Vec<PhotoId>,
    tracks: Option<Vec<String>>,
    from_ms: Option<i64>,
    to_ms: Option<i64>,
    overrides: Vec<(f64, Mode)>,
}

impl Selection {
    fn of(photos: Vec<PhotoId>) -> Selection {
        Selection { photos, tracks: None, from_ms: None, to_ms: None, overrides: Vec::new() }
    }
}

/// A trip's photos, tracks, range and overrides. A mode the trip names that isn't one of ours is dropped.
fn resolve_trip(s: &Session, id: &str) -> Result<Selection> {
    let mode = |m: &str| match m {
        "walk" => Some(Mode::Walk),
        "bike" => Some(Mode::Bike),
        "road" => Some(Mode::Road),
        "flight" => Some(Mode::Flight),
        _ => None,
    };
    let range = s.trip_range(id)?;
    Ok(Selection {
        photos: s.trip_photos(id)?,
        tracks: Some(s.trip_tracks(id)?),
        from_ms: range.map(|r| r.0),
        to_ms: range.map(|r| r.1),
        overrides: s.trip_overrides(id)?.into_iter().filter_map(|(at, m)| Some((at as f64 / 1000.0, mode(&m)?))).collect(),
    })
}

/// A time in milliseconds since 1970, when given.
fn ms_param(p: &Value, key: &str, c: &str) -> Result<Option<i64>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_i64().map(Some).ok_or_else(|| bad(c, format!("`{key}` is milliseconds since 1970 (a whole number)"))),
    }
}

/// `privacyTrim`: the radius in metres, `None` when off. Not given: the tracking settings'.
fn trim_param(s: &Session, p: &Value, c: &str) -> Result<Option<f64>> {
    let settings = s.tracks.settings.unwrap_or_default();
    let on = |r: f64| (r > 0.0).then_some(r);
    match p.get("privacyTrim") {
        None | Some(Value::Null) => Ok(settings.trim.then_some(settings.trim_radius_m).and_then(on)),
        Some(Value::Bool(b)) => Ok(b.then_some(settings.trim_radius_m).and_then(on)),
        Some(v) => v
            .as_f64()
            .filter(|r| (0.0..=20_000_000.0).contains(r))
            .map(on)
            .ok_or_else(|| bad(c, "`privacyTrim` is true, false or metres (0..20000000; 0 is off)")),
    }
}

/// The leg leaving within a minute of an override takes its mode.
fn apply_overrides(path: &mut Path, overrides: &[(f64, Mode)]) {
    for piece in &mut path.pieces {
        if let Kind::Estimated { mode, .. } = &mut piece.kind
            && let Some((_, m)) = overrides.iter().find(|(at, _)| (piece.start - at).abs() <= 60.0)
        {
            *mode = *m;
        }
    }
}

fn line_json(lines: &[Vec<(f64, f64)>]) -> Value {
    Value::Array(lines.iter().map(|l| Value::Array(l.iter().map(|(la, lo)| json!([la, lo])).collect())).collect())
}

/// The answer of `map.path`.
fn path_json(path: &Path, sessions: &[String], found: &crate::map::RouteFixes) -> Value {
    let pieces: Vec<Value> = path
        .pieces
        .iter()
        .map(|p| match &p.kind {
            Kind::Recorded { track, ascent_m, descent_m, times } => json!({
                "source": "recorded", "track": sessions.get(*track), "start": p.start, "end": p.end,
                "distanceKm": p.distance_km, "durationS": p.duration_s, "ascentM": ascent_m, "descentM": descent_m,
                "lines": line_json(&p.lines), "times": times,
            }),
            Kind::Estimated { from, to, speed_kmh, mode, arc } => json!({
                "source": "estimated", "from": from, "to": to, "start": p.start, "end": p.end,
                "distanceKm": p.distance_km, "durationS": p.duration_s, "speedKmh": speed_kmh, "mode": mode.as_str(), "arc": arc,
                "lines": line_json(&p.lines),
            }),
        })
        .collect();
    let stops: Vec<Value> = path
        .stops
        .iter()
        .map(|s| json!({"lat": s.lat, "lon": s.lon, "arrive": s.arrive, "leave": s.leave, "count": s.photos.len(), "photos": s.photos}))
        .collect();
    let photos: Vec<Value> =
        path.photos.iter().map(|p| json!({"id": p.id, "t": p.t, "lat": p.lat, "lon": p.lon, "piece": p.piece, "at": p.at})).collect();
    let st = path.stats();
    json!({
        "estimated": path.pieces.iter().any(|p| !p.recorded()),
        "range": path.range().map(|(a, b)| json!({"start": a, "end": b, "startIso": iso(a), "endIso": iso(b)})),
        "pieces": pieces,
        "stops": stops,
        "photos": photos,
        "stats": {
            "distanceKm": st.distance_km, "recordedKm": st.recorded_km, "estimatedKm": st.estimated_km,
            "durationS": st.duration_s, "movingS": st.moving_s, "ascentM": st.ascent_m, "descentM": st.descent_m,
            "photos": st.photos, "stops": st.stops,
        },
        "trim": path.trim.map(|t| json!({
            "applied": t.points > 0,
            "radiusM": t.radius_m,
            "removed": {"start": t.start, "end": t.end, "routes": t.vanished, "points": t.points},
        })),
        "skipped": {"noGps": found.no_gps, "noTime": found.no_time, "unknown": found.unknown, "invalid": path.skipped},
        "zoneEstimated": found.zone_estimated,
    })
}

fn map_path(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "map.path";
    let given = |k: &str| p.get(k).filter(|v| !v.is_null());
    let mut sel = match (given("ids"), given("album"), given("trip")) {
        (ids, None, None) => Selection::of(match ids {
            None => s.visible_cloned(),
            Some(Value::Array(a)) => a.iter().map(|v| v.as_u64().map(PhotoId).ok_or_else(|| bad(C, "ids are photo ids"))).collect::<Result<_>>()?,
            Some(_) => return Err(bad(C, "ids is a list of photo ids")),
        }),
        (None, Some(a), None) => {
            let id = a.as_u64().map(AlbumId).ok_or_else(|| bad(C, "`album` is an album id"))?;
            s.catalog.album(id).ok_or_else(|| EngineError::Other(format!("no such album: {}", id.0)))?;
            Selection::of(s.catalog.album_photos(id))
        }
        (None, None, Some(t)) => resolve_trip(s, t.as_str().ok_or_else(|| bad(C, "`trip` is a trip id"))?)?,
        _ => return Err(bad(C, "give one of `ids`, `album` or `trip`")),
    };
    // what the call says wins over what a trip says
    if let Some(t) = named_tracks(p, C, true)? {
        sel.tracks = Some(t);
    } else if given("tracks").is_some() {
        sel.tracks = None;
    }
    sel.from_ms = ms_param(p, "fromMs", C)?.or(sel.from_ms);
    sel.to_ms = ms_param(p, "toMs", C)?.or(sel.to_ms);
    let max_points = match given("maxPoints") {
        None => geo_path::MAX_POINTS,
        Some(v) => v
            .as_u64()
            .filter(|m| (2..=geo_path::MAX_POINTS_LIMIT as u64).contains(m))
            .map(|m| m as usize)
            .ok_or_else(|| bad(C, "`maxPoints` is 2..20000"))?,
    };
    let stop_minutes = f64_or(p, "stopMinutes", geo_path::DWELL_S / 60.0);
    if !stop_minutes.is_finite() || !(0.0..=10_000.0).contains(&stop_minutes) {
        return Err(bad(C, "`stopMinutes` is 0..10000"));
    }
    let params = geo_path::Params {
        route: super::map::route_params(p, C)?,
        dwell_s: stop_minutes * 60.0,
        max_points,
        trim_m: trim_param(s, p, C)?,
        from: sel.from_ms.map(|t| t as f64 / 1000.0),
        to: sel.to_ms.map(|t| t as f64 / 1000.0),
    };
    let found = crate::map::route_fixes(&s.catalog, &sel.photos);
    // the tracks: those named, else those that overlap the window, its open sides the photos' time
    let by = store::sessions(s.tracks.items());
    let in_window = |t: f64| params.from.is_none_or(|a| t >= a) && params.to.is_none_or(|b| t <= b);
    let times: Vec<f64> = found.fixes.iter().map(|f| f.time).filter(|t| in_window(*t)).collect();
    let (first, last) = times.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), t| (a.min(*t), b.max(*t)));
    let span = match (params.from, params.to, times.is_empty()) {
        (None, None, true) => None,
        (from, to, empty) => {
            Some((from.unwrap_or(if empty { f64::NEG_INFINITY } else { first }), to.unwrap_or(if empty { f64::INFINITY } else { last })))
        }
    };
    let mut chosen: Vec<(&str, &Vec<&Value>, (i64, i64, u64))> = match &sel.tracks {
        Some(list) => list
            .iter()
            .map(|id| by.get_key_value(id.as_str()).map(|(k, v)| (*k, v, span_of(k, v))).ok_or_else(|| no_such(id)))
            .collect::<Result<_>>()?,
        None => by
            .iter()
            .map(|(k, v)| (*k, v, span_of(k, v)))
            .filter(|(_, _, (a, b, _))| span.is_some_and(|(from, to)| *b as f64 / 1000.0 >= from && *a as f64 / 1000.0 <= to))
            .collect(),
    };
    chosen.sort_by_key(|c| (c.2.0, c.0));
    let read: u64 = chosen.iter().map(|c| c.2.2).fold(0, u64::saturating_add);
    if read > POINTS_READ_MAX {
        return Err(too_many_points(read));
    }
    let names: Vec<String> = chosen.iter().map(|(k, _, _)| k.to_string()).collect();
    let mut path = geo_path::build(&found.fixes, chosen.iter().map(|(_, v, _)| session_points(v)), &params).map_err(|e| {
        EngineError::Other(format!(
            "limit: the path would have {} recorded pieces, more than {}; name fewer tracks or a shorter time",
            e.0,
            geo_path::PIECES_MAX
        ))
    })?;
    apply_overrides(&mut path, &sel.overrides);
    Ok(path_json(&path, &names, &found))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
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
        },
        cmd!(query "map.path", "Path of Photos and Tracks", [], None,
            "{ids? (default the visible photos) | album?: id | trip?: id, fromMs?, toMs?, tracks?: \"auto\" (default: the recorded sessions that overlap the window, or the photos' time) | [session], privacyTrim?: bool | metres (default the tracking settings; 0 is off), stopRadius? (150 m), gapHours? (36), arcKm? (300), detail? (48), stopMinutes?: a recorded stop lasts this long at least (10), maxPoints?: per recorded piece, 2..20000 (4000)} → {estimated, range: {start, end, startIso, endIso} | null, pieces: [{source: \"recorded\", track: session, start, end, distanceKm, durationS, ascentM, descentM, lines: [[[lat, lon], …]], times: [[seconds, …]]} | {source: \"estimated\", from: stop | null, to: stop | null, start, end, distanceKm, durationS, speedKmh, mode: walk|bike|road|flight, arc, lines}], stops: [{lat, lon, arrive, leave, count, photos}], photos: [{id, t, lat, lon, piece: index | null (a route of one stop), at: 0..1}], stats: {distanceKm, recordedKm, estimatedKm, durationS, movingS, ascentM, descentM, photos, stops}, trim: {applied, radiusM, removed: {start, end, routes}} | null, skipped: {noGps, noTime, unknown, invalid}, zoneEstimated}: one path for a selection: recorded where a stored track covers the time, map.route's estimated legs elsewhere (a recording replaces them over its time; an estimated leg's `from`/`to` is null where it meets a recording). Pieces are in time order and never overlap; a long pause (gapHours) ends a piece; a break in a recording is a new line; lines cross the antimeridian as two. The privacy trim cuts what lies within the radius of where each track and each estimated route began and ended (and the photos taken there) from this answer only. Times are seconds since 1970 (UTC)",
            always, map_path),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightcraft_geo::path::Piece;

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

    #[test]
    fn a_trips_mode_override_names_the_leg_leaving_then() {
        let leg = |start: f64| Piece {
            start,
            end: start + 100.0,
            distance_km: 1.0,
            duration_s: 100.0,
            moving_s: 100.0,
            lines: Vec::new(),
            kind: Kind::Estimated { from: None, to: None, speed_kmh: 36.0, mode: Mode::Road, arc: false },
        };
        let mut path = Path { pieces: vec![leg(0.0), leg(1000.0), leg(2000.0)], ..Path::default() };
        apply_overrides(&mut path, &[(1030.0, Mode::Bike), (2061.0, Mode::Walk)]);
        let modes: Vec<Mode> = path.pieces.iter().filter_map(|p| if let Kind::Estimated { mode, .. } = p.kind { Some(mode) } else { None }).collect();
        assert_eq!(modes, vec![Mode::Road, Mode::Bike, Mode::Road], "within a minute only");
    }
}
