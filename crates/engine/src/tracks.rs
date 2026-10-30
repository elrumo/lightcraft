//! Recorded tracks: where a person went, as the shared document `tracks` (the commands are `track.*`,
//! [`crate::cmd`]).
//!
//! A recording is a *session*. It is stored as one or more *chunks* ([`track::chunks`]: at most 20 000 points and
//! 24 hours each), one item of the document each:
//!
//! ```text
//! {id: "<session>-<index>", session, index, name?, startMs, endMs, tz?, source, device?, points, distanceKm,
//!  ascentM, descentM, bbox: {south, west, north, east}, fmt: 1, data: base64(track::pack(points))}
//! ```
//!
//! `session` is `s` and 16 hex digits worked out from the points themselves ([`session_of`]), so importing the same
//! file twice is one session. Once written, only an item's `name` changes, so two devices never edit one chunk's points
//! at the same time. Times are milliseconds since 1970 (UTC); `tz` is the minutes east of UTC where the session
//! began. A chunk's figures are its own (the one step between two chunks of a session counts in neither).
//!
//! The document lives in `tracks.json` in the library (not `prefs.json`: it can be megabytes) and syncs like the other
//! shared documents ([`crate::cloud_sync`]): with a server as a whole, through iCloud and a cloud folder one record per
//! chunk ([`crate::icloud`]). Saving it bumps the library's preferences generation, which is what both look at.
//!
//! What is read is hostile until checked (the file, and what a server, iCloud or a cloud folder sends): an item whose
//! id, types or sizes are wrong, or whose `data` doesn't unpack to the points it claims, is left out with a log line and
//! the others are kept ([`take`]). An item of a newer format (`fmt` above 1) is kept as it is, listed as unreadable and
//! never decoded; fields this build doesn't know are kept too. A new track is refused (`limit: …`) when the document
//! would pass [`BYTES_LIMIT`] or [`ITEMS_LIMIT`]: a server keeps 24 MiB and 2000 items, and the margin is for what two
//! devices add while apart.
//!
//! The tracking preferences (`tracking.settings`: trim the start and end of an exported track) are the item
//! `{id: "tracking", trim, trimRadiusM}` of the `prefs` document, kept in `prefs.json`.

use std::collections::{BTreeMap, HashMap, HashSet};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use lightcraft_geo::track::{self, Point};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::Digest as _;

use crate::{EngineError, Result, Session};

/// The shared document's name.
pub const DOC: &str = "tracks";
/// Where the library keeps it.
pub const FILE: &str = "tracks.json";
/// A new track is refused when the document would pass this many bytes (as JSON)…
pub const BYTES_LIMIT: usize = 16 << 20;
/// …or this many items.
pub const ITEMS_LIMIT: usize = 1500;
/// What is taken from elsewhere at most (a server's own limits: past them it keeps nothing for anyone).
const BYTES_TAKEN_MAX: usize = 24 << 20;
const ITEMS_TAKEN_MAX: usize = 2000;
/// One item, as JSON, at most: a chunk of 20 000 points takes well under 1 MiB, and an item must fit a CloudKit record.
const ITEM_MAX: usize = 4 << 20;
/// The format of the items this build writes and reads.
const FMT: u64 = 1;
/// A track's name or device at most, in characters.
pub const NAME_MAX: usize = 200;
/// A source this long at most (a newer build may know more than [`SOURCES`]).
const SOURCE_MAX: usize = 32;
/// Where a track came from.
pub const SOURCES: [&str; 4] = ["phone", "import", "watch", "other"];
/// A zone is at most this many minutes from UTC.
pub const TZ_MAX: i64 = 18 * 60;
/// A new session's points at most (the document's size refuses far fewer of most tracks; this bounds the work).
pub const SESSION_POINTS_MAX: usize = 2_000_000;
/// The `prefs` item that holds [`TrackingSettings`].
pub const TRACKING_ITEM: &str = "tracking";
/// The privacy trim's radius, in metres.
pub const TRIM_RADIUS_M: std::ops::RangeInclusive<f64> = 50.0..=5000.0;
/// Distances (km) and climbs (m) above these aren't real.
const KM_MAX: f64 = 1e7;
const CLIMB_MAX_M: f64 = 1e8;

/// The tracking preferences (`tracking.settings`), the same on all of a person's devices.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TrackingSettings {
    /// Cut the part of a track near where it began and ended from what is exported (never from what is stored).
    pub trim: bool,
    /// How near, in metres ([`TRIM_RADIUS_M`]).
    pub trim_radius_m: f64,
}

impl Default for TrackingSettings {
    fn default() -> Self {
        TrackingSettings { trim: false, trim_radius_m: 300.0 }
    }
}

impl TrackingSettings {
    pub fn json(&self) -> Value {
        json!({"trim": self.trim, "trimRadiusM": self.trim_radius_m})
    }

    /// The `prefs` item that carries them.
    pub fn item(&self) -> Value {
        json!({"id": TRACKING_ITEM, "trim": self.trim, "trimRadiusM": self.trim_radius_m})
    }

    /// The settings `prefs` item `v` holds, when it holds real ones.
    pub fn from_item(v: &Value) -> Option<TrackingSettings> {
        let trim = v.get("trim")?.as_bool()?;
        let trim_radius_m = v.get("trimRadiusM")?.as_f64().filter(|r| TRIM_RADIUS_M.contains(r))?;
        Some(TrackingSettings { trim, trim_radius_m })
    }
}

/// The session's document as this device has it.
#[derive(Default)]
pub(crate) struct Tracks {
    /// Its items, checked ([`take`]).
    items: Vec<Value>,
    /// Its size as JSON.
    bytes: usize,
    /// The tracking preferences, once set (until then a device sends none: it would undo another's choice).
    pub(crate) settings: Option<TrackingSettings>,
}

impl Tracks {
    /// The document read from `tracks.json` (already parsed): what can be kept, and how many items were left out.
    pub(crate) fn load(items: &[Value], settings: Option<TrackingSettings>) -> (Tracks, usize) {
        let (items, dropped) = take(items, &[]);
        let bytes = serde_json::to_vec(&items).map_or(0, |b| b.len());
        (Tracks { items, bytes, settings }, dropped)
    }

    pub(crate) fn items(&self) -> &[Value] {
        &self.items
    }

    /// The document's size as JSON, and its items.
    pub(crate) fn size(&self) -> (usize, usize) {
        (self.bytes, self.items.len())
    }
}

/// `s` and 16 lowercase hex digits.
fn session_ok(s: &str) -> bool {
    s.len() == 17 && s.strip_prefix('s').is_some_and(|h| h.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
}

fn opt_str(o: &Map<String, Value>, k: &str, max: usize) -> bool {
    match o.get(k) {
        None | Some(Value::Null) => true,
        Some(Value::String(s)) => s.chars().count() <= max,
        Some(_) => false,
    }
}

fn number(o: &Map<String, Value>, k: &str, range: std::ops::RangeInclusive<f64>) -> bool {
    o.get(k).and_then(Value::as_f64).is_some_and(|v| range.contains(&v))
}

/// Can item `v` be kept? `Ok(true)`: it is one this build reads (its points unpack to what it claims); `Ok(false)`: an
/// item of a newer format, kept as it is; `Err`: why not.
fn check(v: &Value) -> std::result::Result<bool, &'static str> {
    let o = v.as_object().ok_or("not an object")?;
    let session = o.get("session").and_then(Value::as_str).filter(|s| session_ok(s)).ok_or("no session")?;
    let index = o.get("index").and_then(Value::as_u64).filter(|i| *i < ITEMS_TAKEN_MAX as u64).ok_or("no index")?;
    if o.get("id").and_then(Value::as_str) != Some(format!("{session}-{index}").as_str()) {
        return Err("not its session's id");
    }
    match o.get("fmt").and_then(Value::as_u64) {
        Some(FMT) => {}
        Some(f) if f > FMT => return Ok(false),
        _ => return Err("an unknown fmt"),
    }
    let tz_ok = match o.get("tz") {
        None | Some(Value::Null) => true,
        Some(t) => t.as_i64().is_some_and(|t| t.abs() <= TZ_MAX),
    };
    let source_ok = o.get("source").and_then(Value::as_str).is_some_and(|s| !s.is_empty() && s.chars().count() <= SOURCE_MAX);
    if !opt_str(o, "name", NAME_MAX) || !opt_str(o, "device", NAME_MAX) || !tz_ok || !source_ok {
        return Err("a name, device, zone or source that isn't one");
    }
    if !number(o, "distanceKm", 0.0..=KM_MAX) || !number(o, "ascentM", 0.0..=CLIMB_MAX_M) || !number(o, "descentM", 0.0..=CLIMB_MAX_M) {
        return Err("figures that aren't real");
    }
    let b = o.get("bbox").and_then(Value::as_object).ok_or("no bbox")?;
    let side = |k: &str, lim: f64| b.get(k).and_then(Value::as_f64).filter(|v| v.abs() <= lim);
    let (Some(south), Some(_), Some(north), Some(_)) = (side("south", 90.0), side("west", 180.0), side("north", 90.0), side("east", 180.0)) else {
        return Err("a bbox that isn't one");
    };
    if south > north {
        return Err("a bbox that isn't one");
    }
    let (Some(points), Some(start), Some(end)) =
        (o.get("points").and_then(Value::as_u64), o.get("startMs").and_then(Value::as_i64), o.get("endMs").and_then(Value::as_i64))
    else {
        return Err("no points, startMs or endMs");
    };
    let data = o.get("data").and_then(Value::as_str).ok_or("no data")?;
    let bytes = B64.decode(data).map_err(|_| "data that isn't base64")?;
    let unpacked = track::unpack(&bytes).map_err(|_| "data that doesn't unpack")?;
    let (Some(first), Some(last)) = (unpacked.first(), unpacked.last()) else { return Err("no points") };
    if unpacked.len() as u64 != points || first.time_ms != start || last.time_ms != end {
        return Err("data that doesn't hold the points it claims");
    }
    Ok(true)
}

/// The items of `items` that can be kept ([`check`]), in their order, and how many were left out (each with a log
/// line): at most [`ITEMS_TAKEN_MAX`] items, [`BYTES_TAKEN_MAX`] bytes, an id once. An item the same as in `known` (what
/// this device already has) isn't decoded again.
pub(crate) fn take(items: &[Value], known: &[Value]) -> (Vec<Value>, usize) {
    let known: HashMap<&str, &Value> = known.iter().filter_map(|v| Some((v.get("id")?.as_str()?, v))).collect();
    let mut seen: HashSet<&str> = HashSet::new();
    let (mut out, mut total, mut dropped) = (Vec::new(), 2usize, 0usize);
    for v in items {
        let size = serde_json::to_vec(v).map_or(usize::MAX, |b| b.len());
        let id = v.get("id").and_then(Value::as_str).unwrap_or_default();
        let why = if out.len() >= ITEMS_TAKEN_MAX {
            Err("one item too many")
        } else if size > ITEM_MAX {
            Err("too large")
        } else if total.saturating_add(size).saturating_add(1) > BYTES_TAKEN_MAX {
            Err("the document would be too large")
        } else if known.get(id).is_some_and(|k| *k == v) {
            Ok(true)
        } else {
            check(v)
        };
        match why {
            Ok(_) if seen.insert(id) => {
                total = total.saturating_add(size).saturating_add(1);
                out.push(v.clone());
            }
            Ok(_) => {
                dropped += 1;
                log::warn!("tracks: item {} left out: its id came before", id.chars().take(40).collect::<String>());
            }
            Err(why) => {
                dropped += 1;
                log::warn!("tracks: item {} left out: {why}", id.chars().take(40).collect::<String>());
            }
        }
    }
    (out, dropped)
}

/// Points of a new track that were left out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Dropped {
    /// A time or position that isn't real (or an entry that isn't a point).
    pub invalid: usize,
    /// Earlier than the point before.
    pub out_of_order: usize,
    /// At the same moment as the point before.
    pub duplicate: usize,
}

impl Dropped {
    pub fn json(&self) -> Value {
        json!({"invalid": self.invalid, "outOfOrder": self.out_of_order, "duplicate": self.duplicate})
    }
}

/// The points of a new track that can be kept, in the order they came (`None`: an entry that isn't a point): a point
/// whose time or position isn't real, that is earlier than the one kept before it or at the same moment is left out
/// (an elevation or accuracy that isn't real only leaves the point without it). A point left out that began a stretch
/// of recording passes that on to the next one kept, so a break is never drawn over.
pub fn tidy(raw: impl IntoIterator<Item = Option<Point>>) -> (Vec<Point>, Dropped) {
    let mut out: Vec<Point> = Vec::new();
    let mut d = Dropped::default();
    let mut broke = false;
    for p in raw {
        let Some(p) = p else {
            d.invalid += 1;
            continue;
        };
        let Some(p) = track::real(&p) else {
            d.invalid += 1;
            broke |= p.new_segment;
            continue;
        };
        match out.last() {
            Some(q) if p.time_ms < q.time_ms => d.out_of_order += 1,
            Some(q) if p.time_ms == q.time_ms => d.duplicate += 1,
            _ => {
                out.push(Point { new_segment: p.new_segment || broke, ..p });
                broke = false;
                continue;
            }
        }
        broke |= p.new_segment;
    }
    (out, d)
}

/// The session id of `points`: `s` and 16 hex digits of a hash of their count, their first and last moment and their
/// first and last position (to the millionth of a degree they are stored at), so the same recording is the same
/// session however often it is added.
pub fn session_of(points: &[Point]) -> Option<String> {
    let (f, l) = (points.first()?, points.last()?);
    let micro = |deg: f64| (deg * 1e6).round() as i64;
    let mut h = sha2::Sha256::new();
    for n in [points.len() as i64, f.time_ms, l.time_ms, micro(f.lat), micro(f.lon), micro(l.lat), micro(l.lon)] {
        h.update(n.to_le_bytes());
    }
    Some(h.finalize().iter().take(8).fold(String::from("s"), |s, b| s + &format!("{b:02x}")))
}

/// What a new session says about itself besides its points.
#[derive(Clone, Debug, Default)]
pub struct About {
    pub name: Option<String>,
    pub source: String,
    pub device: Option<String>,
    /// Minutes east of UTC where it began.
    pub tz: Option<i64>,
}

/// The items of session `session` made of `points` (tidied, at least one): one per chunk.
fn chunk_items(session: &str, points: &[Point], about: &About) -> Result<Vec<Value>> {
    let failed = |e: String| EngineError::Other(format!("track: {e}"));
    track::chunks(points, track::CHUNK_POINTS, track::CHUNK_SPAN_MS)
        .iter()
        .enumerate()
        .map(|(index, c)| {
            let data = track::pack(c).map_err(|e| failed(e.to_string()))?;
            let s = track::stats(c).ok_or_else(|| failed("no points".into()))?;
            let [south, west, north, east] = s.bbox;
            let mut o = Map::new();
            o.insert("id".into(), json!(format!("{session}-{index}")));
            o.insert("session".into(), json!(session));
            o.insert("index".into(), json!(index));
            if let Some(n) = &about.name {
                o.insert("name".into(), json!(n));
            }
            o.insert("startMs".into(), json!(s.start_ms));
            o.insert("endMs".into(), json!(s.end_ms));
            if let Some(tz) = about.tz {
                o.insert("tz".into(), json!(tz));
            }
            o.insert("source".into(), json!(about.source));
            if let Some(d) = &about.device {
                o.insert("device".into(), json!(d));
            }
            o.insert("points".into(), json!(s.points));
            o.insert("distanceKm".into(), json!(s.distance_km));
            o.insert("ascentM".into(), json!(s.ascent_m));
            o.insert("descentM".into(), json!(s.descent_m));
            o.insert("bbox".into(), json!({"south": south, "west": west, "north": north, "east": east}));
            o.insert("fmt".into(), json!(FMT));
            o.insert("data".into(), json!(B64.encode(data)));
            Ok(Value::Object(o))
        })
        .collect()
}

/// A chunk as the commands show it: the item without its points, and its size in the document.
pub(crate) fn chunk_json(v: &Value) -> Value {
    let bytes = serde_json::to_vec(v).map_or(0, |b| b.len());
    let mut c = v.clone();
    if let Some(o) = c.as_object_mut() {
        o.remove("data");
        o.insert("bytes".into(), json!(bytes));
    }
    c
}

fn readable(v: &Value) -> bool {
    v.get("fmt").and_then(Value::as_u64) == Some(FMT)
}

fn session_id(v: &Value) -> Option<&str> {
    v.get("session")?.as_str()
}

fn index_of(v: &Value) -> u64 {
    v.get("index").and_then(Value::as_u64).unwrap_or(u64::MAX)
}

fn bbox_of(v: &Value) -> Option<[f64; 4]> {
    let b = v.get("bbox")?;
    let side = |k: &str| b.get(k).and_then(Value::as_f64);
    Some([side("south")?, side("west")?, side("north")?, side("east")?])
}

/// The smallest box that holds boxes `a` and `b` (`[south, west, north, east]`; west is greater than east across the
/// antimeridian): its west edge is one of theirs.
fn union(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    let width = |x: &[f64; 4]| if x[3] - x[1] >= 360.0 { 360.0 } else { (x[3] - x[1]).rem_euclid(360.0) };
    let cover = |from: f64| [&a, &b].iter().map(|x| (x[1] - from).rem_euclid(360.0) + width(x)).fold(0.0, f64::max);
    let (ca, cb) = (cover(a[1]), cover(b[1]));
    let (west, w) = if ca <= cb { (a[1], ca) } else { (b[1], cb) };
    let (west, east) = if w >= 360.0 { (-180.0, 180.0) } else { (west, (west + w + 540.0).rem_euclid(360.0) - 180.0) };
    [a[0].min(b[0]), west, a[2].max(b[2]), east]
}

/// What a session is, from its items (in index order): its name, zone, source and device are its first chunk's, its
/// figures the sums of its chunks', `unreadable` when one of them is of a newer format.
pub(crate) fn summary(session: &str, pieces: &[&Value]) -> Value {
    let first = pieces.first().copied().unwrap_or(&Value::Null);
    let num = |v: &Value, k: &str| v.get(k).and_then(Value::as_f64).unwrap_or(0.0);
    let start = pieces.iter().filter_map(|v| v.get("startMs").and_then(Value::as_i64)).min();
    let end = pieces.iter().filter_map(|v| v.get("endMs").and_then(Value::as_i64)).max();
    let points: u64 = pieces.iter().filter_map(|v| v.get("points").and_then(Value::as_u64)).fold(0, u64::saturating_add);
    let bbox = pieces.iter().filter_map(|v| bbox_of(v)).reduce(union).map(|[s, w, n, e]| json!({"south": s, "west": w, "north": n, "east": e}));
    let mut o = Map::new();
    o.insert("session".into(), json!(session));
    for k in ["name", "tz", "device"] {
        if let Some(v) = first.get(k).filter(|v| !v.is_null()) {
            o.insert(k.into(), v.clone());
        }
    }
    o.insert("startMs".into(), json!(start));
    o.insert("endMs".into(), json!(end));
    o.insert("source".into(), first.get("source").cloned().unwrap_or(Value::Null));
    o.insert("points".into(), json!(points));
    for k in ["distanceKm", "ascentM", "descentM"] {
        o.insert(k.into(), json!(pieces.iter().map(|v| num(v, k)).sum::<f64>()));
    }
    o.insert("bbox".into(), bbox.unwrap_or(Value::Null));
    o.insert("chunks".into(), json!(pieces.len()));
    o.insert("unreadable".into(), json!(pieces.iter().any(|v| !readable(v))));
    Value::Object(o)
}

/// The items of the document by session, each in index order.
pub(crate) fn sessions(items: &[Value]) -> BTreeMap<&str, Vec<&Value>> {
    let mut by: BTreeMap<&str, Vec<&Value>> = BTreeMap::new();
    for v in items {
        if let Some(s) = session_id(v) {
            by.entry(s).or_default().push(v);
        }
    }
    by.values_mut().for_each(|p| p.sort_by_key(|v| index_of(v)));
    by
}

/// The points of `pieces` (one session's items in index order), those of a newer format left out.
pub(crate) fn points_of(pieces: &[&Value]) -> Vec<Point> {
    let mut out = Vec::new();
    for v in pieces.iter().filter(|v| readable(v)) {
        let decoded = v.get("data").and_then(Value::as_str).and_then(|d| B64.decode(d).ok()).map(|b| track::unpack(&b));
        match decoded {
            Some(Ok(p)) => out.extend(p),
            // (checked when it was taken: only a fault on this device gets here)
            _ => log::error!("tracks: {} can't be read", v.get("id").and_then(Value::as_str).unwrap_or_default()),
        }
    }
    out
}

impl Session {
    /// Change the tracks document with `f` and save it; when it can't be saved, it stays as it was.
    pub(crate) fn change_tracks<R>(&mut self, f: impl FnOnce(&mut Vec<Value>) -> R) -> Result<R> {
        let before = self.tracks.items.clone();
        let r = f(&mut self.tracks.items);
        let json = serde_json::to_vec(&self.tracks.items).map_err(|e| EngineError::Other(format!("{FILE}: {e}")));
        let saved = match (json, self.library.as_mut()) {
            (Ok(json), Some(lib)) => lib.write_shared(FILE, &json).map(|()| json.len()),
            (Ok(json), None) => Ok(json.len()),
            (Err(e), _) => Err(e),
        };
        match saved {
            Ok(bytes) => {
                self.tracks.bytes = bytes;
                Ok(r)
            }
            Err(e) => {
                self.tracks.items = before;
                Err(e)
            }
        }
    }

    /// Add the session made of `points` (tidied, at least one): its id, the items made (none when it was there already,
    /// then also `true`). Refused with `limit: …` when the document would pass its limits.
    pub(crate) fn add_track(&mut self, points: &[Point], about: &About) -> Result<(String, Vec<Value>, bool)> {
        let session = session_of(points).ok_or_else(|| EngineError::Other("track: no points".into()))?;
        if self.tracks.items.iter().any(|v| session_id(v) == Some(session.as_str())) {
            return Ok((session, Vec::new(), true));
        }
        let new = chunk_items(&session, points, about)?;
        let (bytes, items) = self.tracks.size();
        let adds: usize = new.iter().map(|v| serde_json::to_vec(v).map_or(usize::MAX, |b| b.len()).saturating_add(1)).fold(0, usize::saturating_add);
        if items.saturating_add(new.len()) > ITEMS_LIMIT {
            return Err(EngineError::Other(format!(
                "limit: the tracks would be {} pieces, more than {ITEMS_LIMIT} (this one has {}); delete or export older tracks first",
                items.saturating_add(new.len()),
                new.len()
            )));
        }
        if bytes.max(2).saturating_add(adds) > BYTES_LIMIT {
            return Err(EngineError::Other(format!(
                "limit: the tracks would take {:.1} MiB, more than {} MiB (this one is {:.1} MiB); delete or export older tracks first",
                bytes.saturating_add(adds) as f64 / 1048576.0,
                BYTES_LIMIT >> 20,
                adds as f64 / 1048576.0
            )));
        }
        let made = new.clone();
        self.change_tracks(|items| items.extend(new))?;
        Ok((session, made, false))
    }

    /// Make `items` (from a server, iCloud or a cloud folder) this device's tracks document: what can't be kept is left
    /// out ([`take`]).
    pub(crate) fn set_track_items(&mut self, items: &[Value]) -> Result<()> {
        let (items, dropped) = take(items, &self.tracks.items);
        if dropped > 0 {
            log::warn!("tracks: {dropped} item(s) from elsewhere left out");
        }
        if items == self.tracks.items {
            return Ok(());
        }
        self.change_tracks(|mine| *mine = items)
    }

    /// The `prefs` item of the tracking preferences, once they were set.
    pub(crate) fn tracking_item(&self) -> Option<Value> {
        self.tracks.settings.map(|t| t.item())
    }

    /// The tracking preferences the other devices share: what `items` (the `prefs` document) says, none when it has
    /// none (back to the defaults), this device's own when its item isn't one.
    pub(crate) fn take_tracking_item(&mut self, items: &[Value]) {
        match items.iter().find(|v| v.get("id").and_then(Value::as_str) == Some(TRACKING_ITEM)) {
            None => self.tracks.settings = None,
            Some(v) => match TrackingSettings::from_item(v) {
                Some(t) => self.tracks.settings = Some(t),
                None => log::warn!("prefs: a `{TRACKING_ITEM}` item that isn't one is left out"),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(time_ms: i64, lat: f64, lon: f64) -> Point {
        Point { time_ms, lat, lon, ele_m: None, acc_m: None, new_segment: false }
    }

    #[test]
    fn a_union_of_boxes_takes_the_short_way_round() {
        assert_eq!(union([0.0, 10.0, 1.0, 30.0], [2.0, 20.0, 3.0, 40.0]), [0.0, 10.0, 3.0, 40.0]);
        assert_eq!(union([0.0, 0.0, 1.0, 50.0], [0.0, 10.0, 1.0, 20.0]), [0.0, 0.0, 1.0, 50.0], "one inside the other");
        // one each side of the antimeridian: across it, not round the world
        assert_eq!(union([0.0, 170.0, 1.0, 175.0], [0.0, -175.0, 1.0, -170.0]), [0.0, 170.0, 1.0, -170.0]);
        assert_eq!(union([0.0, -175.0, 1.0, -170.0], [0.0, 170.0, 1.0, 175.0]), [0.0, 170.0, 1.0, -170.0], "either order");
        assert_eq!(union([0.0, -180.0, 1.0, 180.0], [5.0, 3.0, 6.0, 4.0]), [0.0, -180.0, 6.0, 180.0], "the whole world stays whole");
    }

    #[test]
    fn tidy_counts_what_it_leaves_out_and_keeps_breaks() {
        let raw = vec![
            Some(pt(1000, 1.0, 1.0)),
            None,
            Some(pt(500, 1.0, 1.0)),
            Some(pt(1000, 2.0, 2.0)),
            Some(Point { new_segment: true, ..pt(1500, 99.0, 0.0) }),
            Some(pt(2000, 1.0, 1.0)),
        ];
        let (kept, d) = tidy(raw);
        assert_eq!(d, Dropped { invalid: 2, out_of_order: 1, duplicate: 1 });
        assert_eq!(kept.iter().map(|p| (p.time_ms, p.new_segment)).collect::<Vec<_>>(), vec![(1000, false), (2000, true)]);
    }

    #[test]
    fn the_session_id_is_the_points_own() {
        let a: Vec<Point> = (0..10).map(|i| pt(i * 1000, 40.0 + i as f64 * 1e-4, -3.0)).collect();
        let id = session_of(&a).unwrap();
        assert!(session_ok(&id), "{id}");
        // what packing keeps (a millionth of a degree) gives the same id
        let packed = track::unpack(&track::pack(&a).unwrap()).unwrap();
        assert_eq!(session_of(&packed).unwrap(), id);
        // any of the moments, the count or the ends changes it
        let mut later = a.clone();
        later[9].time_ms += 1;
        let mut moved = a.clone();
        moved[0].lon += 1e-5;
        let mut ended = a.clone();
        ended[9].lat += 1e-5;
        let mut more = a.clone();
        more.insert(5, pt(4500, 40.0, -3.0));
        let others = [session_of(&later), session_of(&moved), session_of(&ended), session_of(&more), session_of(&a[..9])];
        assert!(others.iter().all(|o| o.as_deref() != Some(id.as_str())), "{others:?}");
        assert!(session_of(&[]).is_none());
    }

    fn walk(n: usize, from_ms: i64) -> Vec<Point> {
        (0..n).map(|i| pt(from_ms + i as i64 * 5000, 40.0 + i as f64 * 1e-4, -3.0 + (i as f64 * 0.3).sin() * 1e-4)).collect()
    }

    fn about() -> About {
        About { source: "phone".into(), ..About::default() }
    }

    /// One readable item, of a session of `n` points.
    fn item(n: usize) -> Value {
        let points = walk(n, 1_700_000_000_000);
        chunk_items(&session_of(&points).unwrap(), &points, &about()).unwrap().remove(0)
    }

    fn with(v: &Value, k: &str, x: Value) -> Value {
        let mut v = v.clone();
        v[k] = x;
        v
    }

    #[test]
    fn what_comes_from_elsewhere_is_checked_item_by_item() {
        use miniz_oxide::deflate::compress_to_vec;
        let good = item(50);
        assert_eq!(check(&good), Ok(true));
        let data = good["data"].as_str().unwrap().to_string();
        let bomb = B64.encode(compress_to_vec(&vec![0u8; track::MAX_INFLATED + 1], 9));
        let other = item(40);
        let bad = [
            with(&good, "data", json!(&data[..data.len() / 2])),
            with(&good, "data", json!("!!! not base64 !!!")),
            with(&good, "data", json!(bomb)),
            with(&good, "data", other["data"].clone()),
            with(&good, "points", json!(51)),
            with(&good, "startMs", json!(0)),
            with(&good, "endMs", json!(good["endMs"].as_i64().unwrap() + 1)),
            with(&good, "name", json!(42)),
            with(&good, "name", json!("x".repeat(NAME_MAX + 1))),
            with(&good, "device", json!(["a"])),
            with(&good, "tz", json!(TZ_MAX + 1)),
            with(&good, "source", json!("")),
            with(&good, "distanceKm", json!("NaN")),
            with(&good, "distanceKm", json!(-1.0)),
            with(&good, "ascentM", json!(1e300)),
            with(&good, "bbox", json!({"south": 91.0, "west": 0.0, "north": 92.0, "east": 1.0})),
            with(&good, "bbox", json!({"south": 10.0, "west": 0.0, "north": 5.0, "east": 1.0})),
            with(&good, "bbox", json!([1, 2, 3, 4])),
            with(&good, "id", json!("../../etc/passwd")),
            with(&good, "id", json!(format!("{}-1", good["session"].as_str().unwrap()))),
            with(&good, "session", json!("S0123456789ABCDEF")),
            with(&good, "index", json!(-1)),
            with(&good, "fmt", json!(0)),
            with(&good, "fmt", json!("1")),
            json!("an item"),
            json!(null),
        ];
        for v in &bad {
            assert!(check(v).is_err(), "{}", v.to_string().chars().take(300).collect::<String>());
        }
        // a newer format is kept, never decoded; fields this build doesn't know are kept
        let newer = json!({"id": "s00000000000000ff-3", "session": "s00000000000000ff", "index": 3, "fmt": 2, "data": "a format to come"});
        assert_eq!(check(&newer), Ok(false));
        let extra = with(&good, "speedProfile", json!([1, 2, 3]));
        assert_eq!(check(&extra), Ok(true));
        let mut items: Vec<Value> = bad.to_vec();
        items.extend([good.clone(), newer.clone(), extra.clone(), good.clone()]);
        let (kept, dropped) = take(&items, &[]);
        assert_eq!(kept, vec![good.clone(), newer], "the first of an id; the extra-field copy repeats it");
        assert_eq!(dropped, bad.len() + 2);
        // a huge id is too large an item, whatever it says
        let (kept, dropped) = take(&[with(&good, "id", json!("x".repeat(10 << 20)))], &[]);
        assert!(kept.is_empty() && dropped == 1);
    }

    #[test]
    fn what_is_taken_stays_within_what_a_server_keeps() {
        let good = item(5);
        let many: Vec<Value> = (0..10_000u64)
            .map(|i| {
                let s = format!("s{i:016x}");
                let mut v = with(&good, "session", json!(s));
                v["id"] = json!(format!("{s}-0"));
                v
            })
            .collect();
        let (kept, dropped) = take(&many, &[]);
        assert_eq!((kept.len(), dropped), (ITEMS_TAKEN_MAX, 10_000 - ITEMS_TAKEN_MAX));
        // big items (positions all over the globe pack badly): no more than the server's bytes
        let mut x = 0x9e37_79b9_7f4a_7c15u64;
        let mut rand = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 11) as f64 / (1u64 << 53) as f64
        };
        let scattered: Vec<Point> = (0..20_000i64).map(|i| pt(i * 1000, rand() * 178.0 - 89.0, rand() * 358.0 - 179.0)).collect();
        let big = chunk_items(&session_of(&scattered).unwrap(), &scattered, &about()).unwrap().remove(0);
        let size = serde_json::to_vec(&big).unwrap().len();
        assert!(size > 200_000 && size < ITEM_MAX, "{size}");
        let copies: Vec<Value> = (0..(BYTES_TAKEN_MAX / size + 5) as u64)
            .map(|i| {
                let s = format!("s{i:016x}");
                let mut v = with(&big, "session", json!(s));
                v["id"] = json!(format!("{s}-0"));
                v
            })
            .collect();
        let (kept, dropped) = take(&copies, &[]);
        assert!(dropped >= 5, "{dropped}");
        assert!(serde_json::to_vec(&kept).unwrap().len() <= BYTES_TAKEN_MAX);
    }

    #[test]
    fn what_this_device_has_already_is_not_decoded_again() {
        // (an item that would fail its check but is the same as this device's own goes through as it is)
        let good = item(5);
        let liar = with(&good, "points", json!(6));
        assert_eq!(take(std::slice::from_ref(&liar), std::slice::from_ref(&liar)).0, vec![liar.clone()]);
        assert!(take(std::slice::from_ref(&liar), &[good]).0.is_empty(), "changed: checked again");
    }

    #[test]
    fn a_new_track_is_refused_past_the_limits() {
        let mut s = Session::new();
        let one = walk(10, 1_000_000_000_000);
        let (session, made, existing) = s.add_track(&one, &about()).unwrap();
        assert_eq!((made.len(), existing), (1, false));
        assert_eq!(s.add_track(&one, &about()).unwrap(), (session, vec![], true), "the same points again: nothing added");
        let (bytes, items) = s.tracks.size();
        assert_eq!((bytes, items), (serde_json::to_vec(&s.tracks.items).unwrap().len(), 1));
        // items: up to the limit, not past it
        let filler = s.tracks.items[0].clone();
        s.tracks.items = vec![filler; ITEMS_LIMIT - 1];
        let next = walk(10, 1_100_000_000_000);
        assert!(s.add_track(&next, &about()).is_ok(), "the 1500th");
        let e = s.add_track(&walk(10, 1_200_000_000_000), &about()).unwrap_err().to_string();
        assert!(e.starts_with("limit: ") && e.contains("1500"), "{e}");
        assert_eq!(s.tracks.items.len(), ITEMS_LIMIT, "nothing added");
        // bytes: up to the limit, not past it
        let mut s = Session::new();
        let new = chunk_items("s0000000000000000", &next, &about()).unwrap();
        let adds = serde_json::to_vec(&new[0]).unwrap().len() + 1;
        s.tracks.bytes = BYTES_LIMIT - adds;
        assert!(s.add_track(&next, &about()).is_ok(), "exactly at the limit");
        s.tracks.items.clear();
        s.tracks.bytes = BYTES_LIMIT - adds + 1;
        let e = s.add_track(&next, &about()).unwrap_err().to_string();
        assert!(e.starts_with("limit: ") && e.contains("16 MiB"), "{e}");
        assert!(s.tracks.items.is_empty());
    }

    #[test]
    fn a_day_and_more_is_cut_into_chunks_that_sum_to_the_session() {
        let mut s = Session::new();
        // 30 hours at a point every 5 s: 21 600 points, past both a chunk's count and its day
        let points = walk(21_600, 1_700_000_000_000);
        let (session, made, _) = s.add_track(&points, &About { tz: Some(120), name: Some("Long".into()), ..about() }).unwrap();
        assert_eq!(made.len(), 2);
        assert_eq!(made.iter().map(|v| v["points"].as_u64().unwrap()).collect::<Vec<_>>(), vec![17_280, 4320], "cut at 24 h");
        assert_eq!((made[0]["id"].as_str().unwrap(), made[1]["index"].as_u64()), (format!("{session}-0").as_str(), Some(1)));
        assert!(made.iter().all(|v| v["tz"] == 120 && v["name"] == "Long" && v["fmt"] == 1));
        let by = sessions(&s.tracks.items);
        let sum = summary(&session, &by[session.as_str()]);
        assert_eq!((sum["points"].as_u64(), sum["chunks"].as_u64()), (Some(21_600), Some(2)));
        assert_eq!((sum["startMs"].as_i64(), sum["endMs"].as_i64()), (Some(points[0].time_ms), Some(points[21_599].time_ms)));
        assert_eq!(points_of(&by[session.as_str()]), track::unpack(&track::pack(&points).unwrap()).unwrap(), "every point back, in order");
    }
}
