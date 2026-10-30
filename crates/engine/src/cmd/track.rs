//! Recorded tracks (`track.*`): add a recording or a GPX file, list, read, rename and delete them. The tracks are the
//! shared document `tracks` ([`crate::tracks`]), not the catalog: none of these is an undo step. They stay out of the
//! command journal too (declared as queries): a track is where someone was, and its points can be megabytes.

use lightcraft_geo::track::{self, Point};
use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd, str_param};
use crate::tracks::{self as store, About, Dropped, NAME_MAX, SESSION_POINTS_MAX, SOURCES, TZ_MAX};
use crate::{EngineError, Result, Session};

/// A GPX file read from a path at most (a file of 5 million points is about half a gigabyte).
const GPX_FILE_MAX: u64 = 256 << 20;
/// `track.get`'s points at most, and by default.
const MAX_POINTS: std::ops::RangeInclusive<u64> = 2..=20_000;
const MAX_POINTS_DEFAULT: usize = 4000;

/// An optional text parameter: trimmed, at most `max` characters, empty = none.
fn text(p: &Value, key: &str, max: usize, c: &str) -> Result<Option<String>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if s.trim().chars().count() > max => Err(bad(c, format!("`{key}` is at most {max} characters"))),
        Some(Value::String(s)) => Ok(Some(s.trim().to_string()).filter(|s| !s.is_empty())),
        Some(_) => Err(bad(c, format!("`{key}` is text"))),
    }
}

/// `tz`: minutes east of UTC.
fn tz_param(p: &Value, c: &str) -> Result<Option<i64>> {
    match p.get("tz") {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_i64()
            .filter(|t| t.abs() <= TZ_MAX)
            .map(Some)
            .ok_or_else(|| bad(c, format!("`tz` is whole minutes east of UTC, -{TZ_MAX}..{TZ_MAX}"))),
    }
}

/// A time in milliseconds since 1970, when given.
fn ms_param(p: &Value, key: &str, c: &str) -> Result<Option<i64>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_i64().map(Some).ok_or_else(|| bad(c, format!("`{key}` is milliseconds since 1970 (a whole number)"))),
    }
}

/// One `[tMs, lat, lon, ele?, acc?, seg?]` of `track.create` (`None`: not a point).
fn point_of(v: &Value) -> Option<Point> {
    let a = v.as_array().filter(|a| (3..=6).contains(&a.len()))?;
    let num = |i: usize| a.get(i).and_then(Value::as_f64);
    let t = num(0).filter(|t| t.is_finite())?;
    Some(Point {
        // (a time past what an i64 holds saturates, and is then refused as out of range)
        time_ms: t.round() as i64,
        lat: num(1)?,
        lon: num(2)?,
        ele_m: num(3),
        acc_m: num(4),
        new_segment: a.get(5).is_some_and(|s| s.as_bool() == Some(true) || s.as_u64() == Some(1)),
    })
}

/// The answer of `track.create` and `track.import`.
fn created(s: &Session, session: &str, made: &[Value], existing: bool, dropped: Dropped) -> Value {
    let (bytes, items) = s.tracks.size();
    json!({
        "session": session,
        "created": made.iter().map(store::chunk_json).collect::<Vec<_>>(),
        "existing": existing,
        "dropped": dropped.json(),
        "docBytes": bytes,
        "docItems": items,
    })
}

/// Add `raw` (the points of a new track as they came) as a session.
fn add(s: &mut Session, c: &str, raw: impl IntoIterator<Item = Option<Point>>, about: &About) -> Result<Value> {
    let (points, dropped) = store::tidy(raw);
    if points.is_empty() {
        return Err(bad(
            c,
            format!(
                "no usable points (left out: {} not real, {} out of order, {} repeated)",
                dropped.invalid, dropped.out_of_order, dropped.duplicate
            ),
        ));
    }
    let (session, made, existing) = s.add_track(&points, about)?;
    Ok(created(s, &session, &made, existing, dropped))
}

fn too_many(n: usize) -> EngineError {
    EngineError::Other(format!("limit: a track holds at most {SESSION_POINTS_MAX} points, this one has {n}"))
}

fn create(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "track.create";
    let source = match str_param(p, "source") {
        None => "phone",
        Some(x) if SOURCES.contains(&x) => x,
        Some(_) => return Err(bad(C, format!("`source` is one of {}", SOURCES.join(", ")))),
    };
    let about = About { name: text(p, "name", NAME_MAX, C)?, source: source.into(), device: text(p, "device", NAME_MAX, C)?, tz: tz_param(p, C)? };
    match (p.get("points").filter(|v| !v.is_null()), p.get("data").filter(|v| !v.is_null())) {
        (Some(Value::Array(a)), None) => {
            if a.len() > SESSION_POINTS_MAX {
                return Err(too_many(a.len()));
            }
            add(s, C, a.iter().map(point_of), &about)
        }
        (None, Some(Value::String(d))) => {
            use base64::Engine as _;
            // (a packed track is at most 100 000 points: well under this)
            if d.len() > 8 << 20 {
                return Err(bad(C, "`data` is larger than a packed track can be"));
            }
            let bytes = base64::engine::general_purpose::STANDARD.decode(d).map_err(|_| bad(C, "`data` is not base64"))?;
            let points = track::unpack(&bytes).map_err(|e| bad(C, e.to_string()))?;
            add(s, C, points.into_iter().map(Some), &about)
        }
        (Some(_), Some(_)) => Err(bad(C, "give `points` or `data`, not both")),
        (Some(_), None) => Err(bad(C, "`points` is a list of [tMs, lat, lon, ele?, acc?, seg?]")),
        (None, Some(_)) => Err(bad(C, "`data` is base64 text")),
        (None, None) => Err(bad(C, "missing `points` or `data`")),
    }
}

fn import(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "track.import";
    let (text_param, path) = (str_param(p, "gpx"), str_param(p, "path"));
    let gpx = match (text_param, path) {
        (Some(g), None) => g.to_string(),
        (None, Some(path)) => {
            let len = std::fs::metadata(path).map_err(|e| bad(C, format!("can't read {path}: {e}")))?.len();
            if len > GPX_FILE_MAX {
                return Err(bad(C, format!("{path} is larger than {} MiB", GPX_FILE_MAX >> 20)));
            }
            std::fs::read_to_string(path).map_err(|e| bad(C, format!("can't read {path}: {e}")))?
        }
        (Some(_), Some(_)) => return Err(bad(C, "give `gpx` or `path`, not both")),
        (None, None) => return Err(bad(C, "missing `path` (a .gpx file) or `gpx` (its text)")),
    };
    let log = lightcraft_meta::parse_gpx(&gpx).map_err(|e| bad(C, e.to_string()))?;
    drop(gpx);
    if log.points.len() > SESSION_POINTS_MAX {
        return Err(too_many(log.points.len()));
    }
    // (a file's name names its track, when nothing else does)
    let stem = path.and_then(|p| std::path::Path::new(p).file_stem()).map(|n| n.to_string_lossy().chars().take(NAME_MAX).collect::<String>());
    let about = About { name: text(p, "name", NAME_MAX, C)?.or(stem), source: "import".into(), device: None, tz: tz_param(p, C)? };
    let mut segment = log.points.first().map(|p| p.segment);
    let raw = log.points.iter().map(|tp| {
        let ms = tp.time * 1000.0;
        let new_segment = segment != Some(tp.segment);
        segment = Some(tp.segment);
        Some(Point {
            time_ms: if ms.is_finite() { ms.round() as i64 } else { -1 },
            lat: tp.latitude,
            lon: tp.longitude,
            ele_m: tp.elevation,
            acc_m: None,
            new_segment,
        })
    });
    let mut answer = add(s, C, raw, &about)?;
    if let Some(o) = answer.as_object_mut() {
        o.insert("tracks".into(), json!(log.tracks));
        o.insert("untimed".into(), json!(log.untimed));
    }
    Ok(answer)
}

fn list(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "track.list";
    let (from, to) = (ms_param(p, "fromMs", C)?, ms_param(p, "toMs", C)?);
    let with_chunks = p.get("chunks").and_then(Value::as_bool).unwrap_or(false);
    let overlaps = |v: &Value| {
        let (start, end) = (v["startMs"].as_i64().unwrap_or(i64::MIN), v["endMs"].as_i64().unwrap_or(i64::MAX));
        from.is_none_or(|f| end >= f) && to.is_none_or(|t| start <= t)
    };
    let mut sessions = Vec::new();
    let mut chunks = Vec::new();
    for (id, pieces) in store::sessions(s.tracks.items()) {
        let summary = store::summary(id, &pieces);
        if !overlaps(&summary) {
            continue;
        }
        if with_chunks {
            chunks.extend(pieces.iter().map(|v| store::chunk_json(v)));
        }
        sessions.push(summary);
    }
    sessions.sort_by_key(|v| v["startMs"].as_i64().unwrap_or(i64::MAX));
    let (bytes, items) = s.tracks.size();
    let mut answer = json!({
        "sessions": sessions,
        "docBytes": bytes,
        "docItems": items,
        "limits": {"bytes": store::BYTES_LIMIT, "items": store::ITEMS_LIMIT},
    });
    if with_chunks && let Some(o) = answer.as_object_mut() {
        o.insert("chunks".into(), Value::Array(chunks));
    }
    Ok(answer)
}

fn no_such(session: &str) -> EngineError {
    EngineError::Other(format!("no such track: {}", session.chars().take(40).collect::<String>()))
}

fn get(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "track.get";
    let (from, to) = (ms_param(p, "fromMs", C)?, ms_param(p, "toMs", C)?);
    let max = match p.get("maxPoints") {
        None | Some(Value::Null) => MAX_POINTS_DEFAULT,
        Some(v) => v.as_u64().filter(|m| MAX_POINTS.contains(m)).map(|m| m as usize).ok_or_else(|| bad(C, "`maxPoints` is 2..20000"))?,
    };
    let by = store::sessions(s.tracks.items());
    let (session, pieces) = match (str_param(p, "session"), str_param(p, "id")) {
        (Some(id), None) => (id, by.get(id).cloned().ok_or_else(|| no_such(id))?),
        (None, Some(chunk)) => {
            let v = s.tracks.items().iter().find(|v| v["id"].as_str() == Some(chunk)).ok_or_else(|| no_such(chunk))?;
            (v["session"].as_str().unwrap_or_default(), vec![v])
        }
        (Some(_), Some(_)) => return Err(bad(C, "give `session` or `id`, not both")),
        (None, None) => return Err(bad(C, "missing `session` (or `id`, one chunk)")),
    };
    let all = by.get(session).map(Vec::as_slice).unwrap_or_default();
    let mut answer = store::summary(session, all);
    let Some(o) = answer.as_object_mut() else { return Ok(answer) };
    o.insert("chunks".into(), Value::Array(pieces.iter().map(|v| store::chunk_json(v)).collect()));
    if p.get("points").and_then(Value::as_bool) == Some(true) {
        let mut points = store::points_of(&pieces);
        points.retain(|q| from.is_none_or(|f| q.time_ms >= f) && to.is_none_or(|t| q.time_ms <= t));
        let (few, simplified) = track::simplify(&points, max);
        let rows: Vec<Value> = few.iter().map(|q| json!([q.time_ms, q.lat, q.lon, q.ele_m, q.acc_m, u8::from(q.new_segment)])).collect();
        o.insert("points".into(), Value::Array(rows));
        o.insert("simplified".into(), json!(simplified));
    }
    Ok(answer)
}

fn session_param<'a>(p: &'a Value, c: &str) -> Result<&'a str> {
    str_param(p, "session").ok_or_else(|| bad(c, "missing `session`"))
}

fn rename(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "track.rename";
    let session = session_param(p, C)?.to_string();
    if p.get("name").is_none_or(Value::is_null) {
        return Err(bad(C, "missing `name` (empty clears it)"));
    }
    let name = text(p, "name", NAME_MAX, C)?;
    let mine = |v: &Value| v["session"].as_str() == Some(session.as_str());
    if !s.tracks.items().iter().any(mine) {
        return Err(no_such(&session));
    }
    let same = s.tracks.items().iter().filter(|v| mine(v)).all(|v| v.get("name").and_then(Value::as_str) == name.as_deref());
    if !same {
        s.change_tracks(|items| {
            for o in items.iter_mut().filter(|v| mine(v)).filter_map(Value::as_object_mut) {
                match &name {
                    Some(n) => drop(o.insert("name".into(), json!(n))),
                    None => drop(o.remove("name")),
                }
            }
        })?;
    }
    Ok(json!({"session": session, "name": name}))
}

fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    let session = session_param(p, "track.delete")?.to_string();
    let mine = |v: &Value| v["session"].as_str() == Some(session.as_str());
    let n = s.tracks.items().iter().filter(|v| mine(v)).count();
    if n == 0 {
        return Err(no_such(&session));
    }
    s.change_tracks(|items| items.retain(|v| !mine(v)))?;
    Ok(json!({"removed": n}))
}

fn tracking_settings(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "tracking.settings";
    let mut t = s.tracks.settings.unwrap_or_default();
    let (trim, radius) = (p.get("trim").filter(|v| !v.is_null()), p.get("trimRadiusM").filter(|v| !v.is_null()));
    if let Some(v) = trim {
        t.trim = v.as_bool().ok_or_else(|| bad(C, "`trim` is true or false"))?;
    }
    if let Some(v) = radius {
        t.trim_radius_m = v.as_f64().filter(|r| store::TRIM_RADIUS_M.contains(r)).ok_or_else(|| bad(C, "`trimRadiusM` is 50..5000 metres"))?;
    }
    if (trim.is_some() || radius.is_some()) && s.tracks.settings != Some(t) {
        let before = s.tracks.settings.replace(t);
        if let Err(e) = s.save_prefs() {
            s.tracks.settings = before;
            return Err(e);
        }
    }
    Ok(t.json())
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "track.create", "Add Recorded Track", [], None,
            "{points?: [[tMs, lat, lon, ele?, acc?, seg?: 1 begins a new stretch]] | data?: base64 of a packed track, name?, source?: phone|import|watch|other (phone), device?, tz?: minutes east of UTC at the start} → {session, created: [chunk], existing, dropped: {invalid, outOfOrder, duplicate}, docBytes, docItems}. A recording as one session of the shared `tracks` document, cut into chunks of at most 20000 points and 24 hours (a chunk is its item without `data`, plus `bytes`, its size in the document). Points that aren't real, go back in time or repeat a moment are left out and counted; the same points again are the same session (`existing`, nothing added). Refused with `limit: …` past 16 MiB or 1500 chunks. Times are milliseconds since 1970 (UTC). Not an undo step",
            always, create),
        cmd!(query "track.import", "Import GPX Track", [], None,
            "{gpx?: GPX text | path?: a .gpx file, name? (the file's name), tz?} → as track.create, plus {tracks: <trk> in the file, untimed: points without a time (left out)}. Every track and segment of the file is one session; a new segment begins a new stretch (`seg: 1`). Importing the same file again adds nothing",
            always, import),
        cmd!(query "track.list", "Recorded Tracks", [], None,
            "{fromMs?, toMs?: only sessions that overlap, chunks?: bool} → {sessions: [{session, name?, startMs, endMs, tz?, source, device?, points, distanceKm, ascentM, descentM, bbox: {south, west, north, east}, chunks: n, unreadable}], chunks?: [chunk], docBytes, docItems, limits: {bytes, items}}. The stored sessions by start; `unreadable`: a chunk is from a newer version (kept, not read). A session's figures are the sums of its chunks'",
            always, list),
        cmd!(query "track.get", "Recorded Track", [], None,
            "{session | id: one chunk, points?: bool, maxPoints?: 2..20000 (4000), fromMs?, toMs?} → the session as track.list has it, with chunks: [chunk] and, with `points`, points: [[tMs, lat, lon, ele|null, acc|null, seg: 0|1]] (at most `maxPoints`: both ends and both sides of every break kept, `simplified` says whether any was left out)",
            always, get),
        cmd!(query "track.rename", "Rename Recorded Track", [], None,
            "{session, name} → {session, name}. An empty name clears it. The only change a stored track takes",
            always, rename),
        cmd!(query "track.delete", "Delete Recorded Track", [], None,
            "{session} → {removed: chunks}. Removes the session from this device and, through the sync, from the others. Not an undo step: ask first",
            always, delete),
        cmd!(
            "tracking.settings",
            "Tracking Settings",
            [],
            None,
            "{trim?: bool, trimRadiusM?: 50..5000 metres} → {trim, trimRadiusM}. The privacy trim of exported tracks (off and 300 m until set): what lies within the radius of where a track began and where it ended is left out of an export, never out of what is stored. No params: read. Shared by the person's devices (the `tracking` item of the `prefs` document)",
            always,
            tracking_settings
        ),
    ]
}
