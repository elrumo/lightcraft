//! Trips and their notes: the shared documents `trips` and `trip-notes` (the commands are `trip.*`, `album.ref`,
//! `album.resolve` and `album.trips`, [`crate::cmd`]).
//!
//! ```text
//! trip  {id: "t…", name, createdMs, cover?: {key: content hash}, albums: [album reference], range?: {fromMs, toMs},
//!        photosInRange, tracks: "range" | [session], overrides: [{atMs, mode}]}
//! note  {id: "n…", trip, tMs, lat?, lon?, text, photo?: content hash, createdMs}
//! ```
//!
//! A trip's photos are the members of the albums it links and, with `photosInRange`, the photos with a position
//! captured inside its range ([`Session::trip_photos`]); it holds no photo of its own, so a change to an album reaches
//! the trip with nothing copied. Its path comes from its photos and recorded tracks (`map.path {trip}`). Notes are one
//! item each, so notes added on two devices all survive. Times are milliseconds since 1970 (UTC).
//!
//! **Album references.** Album ids are a library's own, so a trip names an album by a reference that means the same on
//! every device of the library ([`Session::album_ref`]): `a:<album id>` when the library syncs with a server (which
//! shares album ids) or not at all, `u:<uid>` when it syncs through iCloud or a cloud folder, where an album is the uid
//! the first device gave it ([`crate::icloud::collections`], kept in `icloud.json`). When the library changes how it
//! syncs, the references this device can resolve are rewritten for the new way ([`Session::trips_follow_mode`]); one
//! it can't resolve (an album deleted, or not arrived yet) is kept as it is, and resolves once its album is here.
//!
//! Each document lives in a file of its own in the library (`trips.json`, `trip-notes.json`), as the tracks do: a
//! damaged file or item never costs the other document, and writing one bumps the preferences generation the sync
//! looks at. They sync like the other shared documents ([`crate::cloud_sync`]): with a server merged item by item and
//! field by field (a list field, `albums`, `tracks` or `overrides`, is one value: two devices changing the same list at
//! once keep one side), through iCloud and a cloud folder one record per item (the newer change wins).
//!
//! What is read is hostile until checked: an item whose id, types or sizes are wrong is left out with a log line and
//! the others are kept ([`take`]); fields this build doesn't know are kept; an item with a newer `fmt` is kept as it is
//! and listed as unreadable. A new trip or note is refused (`limit: …`) past [`TRIPS_LIMIT`] trips, [`NOTES_LIMIT`]
//! notes or [`BYTES_LIMIT`] bytes a document; what comes from elsewhere is taken up to a server's own limits.

use std::collections::{HashMap, HashSet};

use lightcraft_catalog::{AlbumId, Photo, PhotoId};
use lightcraft_meta::DateTime;
use serde_json::{Map, Value, json};

use crate::{EngineError, Result, Session};

/// The shared documents' names.
pub const TRIPS: &str = "trips";
pub const NOTES: &str = "trip-notes";
/// Where the library keeps them.
pub const TRIPS_FILE: &str = "trips.json";
pub const NOTES_FILE: &str = "trip-notes.json";
/// A new trip or note is refused past these.
pub const TRIPS_LIMIT: usize = 500;
pub const NOTES_LIMIT: usize = 1500;
/// …or when its document would pass this many bytes (as JSON): a server keeps 24 MiB.
pub const BYTES_LIMIT: usize = 16 << 20;
/// A trip's name at most, in characters.
pub const NAME_MAX: usize = 200;
/// A note's text at most, in characters.
pub const TEXT_MAX: usize = 10_000;
/// The modes an estimated leg can be given.
pub const MODES: [&str; 4] = ["walk", "bike", "road", "flight"];
/// What is taken from elsewhere at most (a server's own limits).
const ITEMS_TAKEN_MAX: usize = 2000;
const BYTES_TAKEN_MAX: usize = 24 << 20;
/// An id, a session or a mode this long at most.
const ID_MAX: usize = 64;
const MODE_MAX: usize = 16;
/// An album reference, a content key this long at most.
const REF_MAX: usize = 80;
const KEY_MAX: usize = 128;
/// Albums, tracks and overrides of one trip at most.
pub const LIST_MAX: usize = 1000;
/// The format of the items this build reads (an item without `fmt` is of this one).
const FMT: u64 = 1;

/// How the library's album references are written: by album id, or by the uid iCloud and a cloud folder know an album by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefMode {
    Id,
    Uid,
}

/// The two documents as this device has them.
#[derive(Default)]
pub(crate) struct Trips {
    trips: Vec<Value>,
    notes: Vec<Value>,
    /// Their sizes as JSON.
    trips_bytes: usize,
    notes_bytes: usize,
    /// How the album references in `trips` are written (`None` until a library is open: taken as it is then).
    pub(crate) mode: Option<RefMode>,
}

/// Which of the two documents.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Doc {
    Trips,
    Notes,
}

impl Doc {
    fn file(self) -> &'static str {
        match self {
            Doc::Trips => TRIPS_FILE,
            Doc::Notes => NOTES_FILE,
        }
    }
}

impl Trips {
    /// The documents read from their files, as [`take`] kept them.
    pub(crate) fn load(trips: Vec<Value>, notes: Vec<Value>) -> Trips {
        let size = |v: &Vec<Value>| serde_json::to_vec(v).map_or(0, |b| b.len());
        Trips { trips_bytes: size(&trips), notes_bytes: size(&notes), trips, notes, mode: None }
    }

    pub(crate) fn items(&self, doc: Doc) -> &[Value] {
        match doc {
            Doc::Trips => &self.trips,
            Doc::Notes => &self.notes,
        }
    }

    fn items_mut(&mut self, doc: Doc) -> &mut Vec<Value> {
        match doc {
            Doc::Trips => &mut self.trips,
            Doc::Notes => &mut self.notes,
        }
    }

    fn bytes_mut(&mut self, doc: Doc) -> &mut usize {
        match doc {
            Doc::Trips => &mut self.trips_bytes,
            Doc::Notes => &mut self.notes_bytes,
        }
    }

    /// Trip `id` (as stored).
    pub(crate) fn trip(&self, id: &str) -> Option<&Value> {
        self.trips.iter().find(|t| str_of(t, "id") == Some(id))
    }
}

pub(crate) fn str_of<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str)
}

/// `prefix` and 1..63 letters, digits, `-` or `_`: nothing that could name a path or split a list.
pub(crate) fn id_ok(s: &str, prefix: char) -> bool {
    (2..=ID_MAX).contains(&s.len()) && s.starts_with(prefix) && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn short(v: Option<&Value>, max: usize) -> bool {
    v.and_then(Value::as_str).is_some_and(|s| s.chars().count() <= max)
}

fn absent(v: Option<&Value>) -> bool {
    v.is_none_or(Value::is_null)
}

fn list_of(v: Option<&Value>, each: impl Fn(&Value) -> bool) -> bool {
    match v {
        None | Some(Value::Null) => true,
        Some(Value::Array(a)) => a.len() <= LIST_MAX && a.iter().all(each),
        Some(_) => false,
    }
}

/// The item's format: `Ok(true)` this build's, `Ok(false)` a newer one's (kept, not read), `Err` neither.
fn readable_fmt(o: &Map<String, Value>) -> std::result::Result<bool, &'static str> {
    match o.get("fmt") {
        None | Some(Value::Null) => Ok(true),
        Some(f) => match f.as_u64() {
            Some(FMT) => Ok(true),
            Some(n) if n > FMT => Ok(false),
            _ => Err("an unknown fmt"),
        },
    }
}

/// Can trip item `v` be kept? `Ok(true)`: one this build reads; `Ok(false)`: of a newer format, kept as it is; `Err`: why not.
fn check_trip(v: &Value) -> std::result::Result<bool, &'static str> {
    let o = v.as_object().ok_or("not an object")?;
    o.get("id").and_then(Value::as_str).filter(|i| id_ok(i, 't')).ok_or("no id")?;
    if !readable_fmt(o)? {
        return Ok(false);
    }
    if !short(o.get("name"), NAME_MAX) || o.get("createdMs").and_then(Value::as_i64).is_none() {
        return Err("no name or createdMs");
    }
    let cover_ok = absent(o.get("cover")) || o.get("cover").is_some_and(|c| c.is_object() && short(c.get("key"), KEY_MAX));
    let range_ok = absent(o.get("range"))
        || o.get("range").is_some_and(
            |r| matches!((r.get("fromMs").and_then(Value::as_i64), r.get("toMs").and_then(Value::as_i64)), (Some(f), Some(t)) if f <= t),
        );
    let tracks_ok = o.get("tracks").and_then(Value::as_str) == Some("range") || list_of(o.get("tracks"), |s| short(Some(s), ID_MAX));
    let overrides_ok = list_of(o.get("overrides"), |x| x.get("atMs").and_then(Value::as_i64).is_some() && short(x.get("mode"), MODE_MAX));
    let flag_ok = absent(o.get("photosInRange")) || o.get("photosInRange").is_some_and(Value::is_boolean);
    if !cover_ok || !range_ok || !tracks_ok || !overrides_ok || !flag_ok || !list_of(o.get("albums"), |r| short(Some(r), REF_MAX)) {
        return Err("a cover, range, list or flag that isn't one");
    }
    Ok(true)
}

/// Can note item `v` be kept? As [`check_trip`]. (A note whose trip isn't here is kept: it may not have arrived yet.)
fn check_note(v: &Value) -> std::result::Result<bool, &'static str> {
    let o = v.as_object().ok_or("not an object")?;
    o.get("id").and_then(Value::as_str).filter(|i| id_ok(i, 'n')).ok_or("no id")?;
    if !readable_fmt(o)? {
        return Ok(false);
    }
    if !o.get("trip").and_then(Value::as_str).is_some_and(|t| id_ok(t, 't')) {
        return Err("no trip");
    }
    if o.get("tMs").and_then(Value::as_i64).is_none() || o.get("createdMs").and_then(Value::as_i64).is_none() {
        return Err("no tMs or createdMs");
    }
    if !short(o.get("text"), TEXT_MAX) || !(absent(o.get("photo")) || short(o.get("photo"), KEY_MAX)) {
        return Err("a text or photo that isn't one");
    }
    let deg = |k: &str, lim: f64| o.get(k).and_then(Value::as_f64).filter(|d| d.abs() <= lim);
    match (absent(o.get("lat")), absent(o.get("lon"))) {
        (true, true) => {}
        (false, false) if deg("lat", 90.0).is_some() && deg("lon", 180.0).is_some() => {}
        _ => return Err("a position that isn't one"),
    }
    Ok(true)
}

/// The items of `items` that can be kept (checked as items of `doc`), in their order, and how many were left out
/// (each with a log line): at most [`ITEMS_TAKEN_MAX`] items and [`BYTES_TAKEN_MAX`] bytes, an id once.
pub(crate) fn take(doc: Doc, items: &[Value]) -> (Vec<Value>, usize) {
    let check = match doc {
        Doc::Trips => check_trip,
        Doc::Notes => check_note,
    };
    let mut seen: HashSet<&str> = HashSet::new();
    let (mut out, mut total, mut dropped) = (Vec::new(), 2usize, 0usize);
    for v in items {
        let size = serde_json::to_vec(v).map_or(usize::MAX, |b| b.len());
        let id = str_of(v, "id").unwrap_or_default();
        let why = if out.len() >= ITEMS_TAKEN_MAX {
            Err("one item too many")
        } else if total.saturating_add(size).saturating_add(1) > BYTES_TAKEN_MAX {
            Err("the document would be too large")
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
                log::warn!("{}: item {} left out: its id came before", doc.file(), id.chars().take(40).collect::<String>());
            }
            Err(why) => {
                dropped += 1;
                log::warn!("{}: item {} left out: {why}", doc.file(), id.chars().take(40).collect::<String>());
            }
        }
    }
    (out, dropped)
}

/// Whether item `v` is one this build reads (not of a newer format).
pub(crate) fn readable(v: &Value) -> bool {
    v.as_object().is_some_and(|o| readable_fmt(o) == Ok(true))
}

/// A trip's album references.
pub(crate) fn refs_of(t: &Value) -> Vec<&str> {
    t.get("albums").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default()
}

/// A trip's range (inclusive, ms).
pub(crate) fn range_of(t: &Value) -> Option<(i64, i64)> {
    let r = t.get("range")?;
    Some((r.get("fromMs")?.as_i64()?, r.get("toMs")?.as_i64()?))
}

/// When photo `p` was taken, in ms since 1970 (UTC). The capture time is the camera's clock, read as `map.route` reads
/// it ([`crate::map::route_fixes`]): with its recorded zone it is exact; without one, a photo with a position takes the
/// zone of its longitude (to the hour); one without a position is read as UTC. `None`: no time, or only a date.
pub fn capture_ms(p: &Photo) -> Option<i64> {
    let dt = p.captured.as_deref().filter(|c| c.contains('T')).and_then(DateTime::parse_iso)?;
    let mut ms = dt.unix_seconds().saturating_mul(1000).saturating_add(i64::from(dt.millis));
    if dt.offset_minutes.is_none()
        && let Some((_, lon)) = p.meta.gps
    {
        ms = ms.saturating_sub((lon / 15.0).round().clamp(-12.0, 14.0) as i64 * 3_600_000);
    }
    Some(ms)
}

/// A photo a trip may show: here, not in Recently Deleted, not hidden.
fn shown(p: &Photo) -> bool {
    !p.deleted && !p.hidden
}

/// A new id: `prefix` and 16 hex digits, none of `taken`'s.
fn new_id(prefix: char, taken: &[Value]) -> String {
    loop {
        let id = format!("{prefix}{}", crate::icloud::collections::new_uid());
        if !taken.iter().any(|v| str_of(v, "id") == Some(id.as_str())) {
            return id;
        }
    }
}

/// Milliseconds since 1970 now.
pub(crate) fn now_ms() -> i64 {
    web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX)).unwrap_or(0)
}

/// The located photos of a library by capture time, for the photos inside trips' ranges.
pub(crate) struct Located(Vec<(i64, PhotoId)>);

impl Located {
    /// The photos with a position and a capture time that a trip may show (Local records and virtual copies left out:
    /// they are this device's own, or the same picture again).
    pub(crate) fn of(s: &Session) -> Located {
        let mut v: Vec<(i64, PhotoId)> = s
            .catalog
            .photos()
            .filter(|p| shown(p) && !p.local && p.copy_of.is_none() && p.meta.gps.is_some())
            .filter_map(|p| Some((capture_ms(p)?, p.id)))
            .collect();
        v.sort_unstable();
        Located(v)
    }

    fn inside(&self, (from, to): (i64, i64)) -> impl Iterator<Item = PhotoId> + '_ {
        let start = self.0.partition_point(|(t, _)| *t < from);
        self.0.get(start..).unwrap_or_default().iter().take_while(move |(t, _)| *t <= to).map(|(_, id)| *id)
    }
}

/// A trip's photos, and where they came from.
pub(crate) struct TripPhotos {
    /// Each once, by capture time (photos without one last, by id).
    pub ids: Vec<PhotoId>,
    /// Members of the linked albums.
    pub from_albums: usize,
    /// Photos inside the range that no linked album holds.
    pub from_range: usize,
}

impl Session {
    /// How this library's album references are written now: by uid while it syncs through iCloud or a cloud folder
    /// (and not through a server, which then keeps the albums), else by album id.
    pub(crate) fn ref_mode(&self) -> RefMode {
        if self.icloud.is_some() && !self.server_linked() { RefMode::Uid } else { RefMode::Id }
    }

    /// The reference to album `id` a trip keeps (assigning the album the uid iCloud will know it by, when it has none
    /// yet). Folders, smart albums and the Quick Collection are refused, with why.
    pub(crate) fn album_ref(&mut self, id: AlbumId) -> Result<String> {
        self.album_ref_as(id, self.ref_mode())
    }

    /// [`Session::album_ref`] written as `mode` says.
    fn album_ref_as(&mut self, id: AlbumId, mode: RefMode) -> Result<String> {
        let a = self.catalog.album(id).ok_or_else(|| EngineError::Other(format!("no such album: {}", id.0)))?;
        let why = if a.folder {
            Some("it is a folder: link the albums in it")
        } else if a.smart.is_some() {
            Some("it is a smart album, whose photos change with its rules: link a manual album")
        } else if a.quick {
            Some("it is the Quick Collection, which is this device's own: link a manual album")
        } else {
            None
        };
        if let Some(why) = why {
            return Err(EngineError::Other(format!("album {} can't be linked to a trip: {why}", id.0)));
        }
        match mode {
            RefMode::Id => Ok(format!("a:{}", id.0)),
            RefMode::Uid => {
                let ic = self.icloud.as_mut().ok_or_else(|| EngineError::Other("iCloud sync went off".into()))?;
                Ok(format!("u:{}", ic.album_uid(id)))
            }
        }
    }

    /// The album reference `r` names on this device, when it is here. An `a:` reference is read only while the library
    /// writes references by id (another device's ids mean nothing here); a `u:` one whenever this library knows its
    /// albums' uids.
    pub(crate) fn resolve_album_ref(&self, r: &str) -> Option<AlbumId> {
        let id = match r.split_once(':')? {
            ("a", n) if self.ref_mode() == RefMode::Id => n.parse::<u64>().ok().map(AlbumId)?,
            ("u", uid) => self.icloud.as_ref()?.album_of_uid(uid)?,
            _ => return None,
        };
        self.catalog.album(id).filter(|a| !a.folder && a.smart.is_none()).map(|a| a.id)
    }

    /// The album references in `trips` follow the way the library syncs now ([`Session::trips_follow`]).
    pub fn trips_follow_mode(&mut self) {
        self.trips_follow(self.ref_mode());
    }

    /// The library now writes album references as `now`: the ones in `trips` written the other way that this device can
    /// resolve are rewritten (an `a:` one gets its album's uid, made if it has none; a `u:` one its album's id); one it
    /// can't is kept as it is. Saved when anything changed.
    pub(crate) fn trips_follow(&mut self, now: RefMode) {
        let Some(was) = self.trips.mode.replace(now) else { return };
        if was == now || self.trips.trips.is_empty() {
            return;
        }
        let mut map: HashMap<String, String> = HashMap::new();
        for t in &self.trips.trips {
            for r in refs_of(t) {
                map.entry(r.to_string()).or_insert_with(|| r.to_string());
            }
        }
        for (old, new) in map.iter_mut() {
            let id = match (now, old.split_once(':')) {
                // (resolved as the library wrote them until now)
                (RefMode::Uid, Some(("a", n))) => n.parse::<u64>().ok().map(AlbumId),
                (RefMode::Id, Some(("u", _))) => self.resolve_album_ref(old),
                _ => None,
            };
            if let Some(r) = id.and_then(|id| self.album_ref_as(id, now).ok()) {
                *new = r;
            }
        }
        if map.iter().all(|(a, b)| a == b) {
            return;
        }
        let res = self.change_doc(Doc::Trips, false, |trips| {
            for t in trips.iter_mut().filter(|t| readable(t)) {
                let refs: Vec<Value> = refs_of(t).iter().map(|r| json!(map.get(*r).map_or(*r, String::as_str))).collect();
                if let Some(o) = t.as_object_mut()
                    && o.contains_key("albums")
                {
                    o.insert("albums".into(), Value::Array(refs));
                }
            }
        });
        if let Err(e) = res {
            log::error!("trips: the album references couldn't be rewritten: {e}");
            self.trips.mode = Some(was);
        }
    }

    /// Change document `doc` with `f` and save it; when it can't be saved, or `limited` and it grew past
    /// [`BYTES_LIMIT`], it stays as it was.
    pub(crate) fn change_doc<R>(&mut self, doc: Doc, limited: bool, f: impl FnOnce(&mut Vec<Value>) -> R) -> Result<R> {
        let before = self.trips.items(doc).to_vec();
        let was = *self.trips.bytes_mut(doc);
        let r = f(self.trips.items_mut(doc));
        let json = serde_json::to_vec(self.trips.items(doc)).map_err(|e| EngineError::Other(format!("{}: {e}", doc.file())));
        let saved = match json {
            Ok(json) if limited && json.len() > BYTES_LIMIT && json.len() > was => Err(EngineError::Other(format!(
                "limit: the {} would take {:.1} MiB, more than {} MiB; delete some first",
                if doc == Doc::Trips { "trips" } else { "trip notes" },
                json.len() as f64 / 1048576.0,
                BYTES_LIMIT >> 20
            ))),
            Ok(json) => match self.library.as_mut() {
                Some(lib) => lib.write_shared(doc.file(), &json).map(|()| json.len()),
                None => Ok(json.len()),
            },
            Err(e) => Err(e),
        };
        match saved {
            Ok(bytes) => {
                *self.trips.bytes_mut(doc) = bytes;
                Ok(r)
            }
            Err(e) => {
                *self.trips.items_mut(doc) = before;
                Err(e)
            }
        }
    }

    /// Make `items` (from a server, iCloud or a cloud folder) this device's document `doc`: what can't be kept is left
    /// out ([`take`]). The notes of a trip that left the trips go with it, as they went where it was deleted.
    pub(crate) fn set_trip_doc(&mut self, doc: Doc, items: &[Value]) -> Result<()> {
        let (items, dropped) = take(doc, items);
        if dropped > 0 {
            log::warn!("{}: {dropped} item(s) from elsewhere left out", doc.file());
        }
        if items == self.trips.items(doc) {
            return Ok(());
        }
        let gone: HashSet<String> = match doc {
            Doc::Trips => {
                let now: HashSet<&str> = items.iter().filter_map(|t| str_of(t, "id")).collect();
                self.trips.trips.iter().filter_map(|t| str_of(t, "id")).filter(|id| !now.contains(id)).map(str::to_string).collect()
            }
            Doc::Notes => HashSet::new(),
        };
        self.change_doc(doc, false, |mine| *mine = items)?;
        if !gone.is_empty() && self.trips.notes.iter().any(|n| str_of(n, "trip").is_some_and(|t| gone.contains(t))) {
            self.change_doc(Doc::Notes, false, |notes| notes.retain(|n| !str_of(n, "trip").is_some_and(|t| gone.contains(t))))?;
        }
        Ok(())
    }

    /// Trip `id`, or `no such trip: …`.
    pub(crate) fn trip_item(&self, id: &str) -> Result<&Value> {
        self.trips.trip(id).ok_or_else(|| EngineError::Other(format!("no such trip: {}", id.chars().take(ID_MAX).collect::<String>())))
    }

    /// The photos of trip item `t` ([`Session::trip_photos`]); `located` when the caller has it already.
    pub(crate) fn photos_of_trip(&self, t: &Value, located: Option<&Located>) -> TripPhotos {
        let mut seen: HashSet<PhotoId> = HashSet::new();
        let mut ids: Vec<PhotoId> = Vec::new();
        for album in refs_of(t).iter().filter_map(|r| self.resolve_album_ref(r)) {
            for id in self.catalog.album(album).map(|a| a.photos.as_slice()).unwrap_or_default() {
                if self.catalog.photo(*id).is_some_and(|p| shown(p)) && seen.insert(*id) {
                    ids.push(*id);
                }
            }
        }
        let from_albums = ids.len();
        if t.get("photosInRange").and_then(Value::as_bool) == Some(true)
            && let Some(range) = range_of(t)
        {
            let own;
            let located = match located {
                Some(l) => l,
                None => {
                    own = Located::of(self);
                    &own
                }
            };
            ids.extend(located.inside(range).filter(|id| seen.insert(*id)).collect::<Vec<_>>());
        }
        let from_range = ids.len().saturating_sub(from_albums);
        let mut timed: Vec<(Option<i64>, PhotoId)> = ids.iter().map(|id| (self.catalog.photo(*id).and_then(|p| capture_ms(p)), *id)).collect();
        // (by time, the photos without one after the others)
        timed.sort_unstable_by_key(|(t, id)| (t.is_none(), *t, *id));
        TripPhotos { ids: timed.into_iter().map(|(_, id)| id).collect(), from_albums, from_range }
    }

    /// The photos of trip `id`: the members of the albums it links (those this device has) and, when the trip says so,
    /// the photos with a position captured inside its range ([`capture_ms`]), each once, by capture time (photos
    /// without one last). Photos in Recently Deleted or hidden are left out. For `map.path {trip}`.
    pub fn trip_photos(&self, id: &str) -> Result<Vec<PhotoId>> {
        Ok(self.photos_of_trip(self.trip_item(id)?, None).ids)
    }

    /// Trip `id`'s range (inclusive, ms since 1970 UTC), when it has one.
    pub fn trip_range(&self, id: &str) -> Result<Option<(i64, i64)>> {
        Ok(range_of(self.trip_item(id)?))
    }

    /// The span of trip `id`: its range, else the first and last capture time of its photos.
    pub(crate) fn trip_span(&self, t: &Value, photos: &[PhotoId]) -> Option<(i64, i64)> {
        range_of(t).or_else(|| {
            let times: Vec<i64> = photos.iter().filter_map(|id| self.catalog.photo(*id).and_then(|p| capture_ms(p))).collect();
            Some((*times.iter().min()?, *times.iter().max()?))
        })
    }

    /// The recorded sessions of trip `id` this device has: those it lists, or with `tracks: "range"` every session of
    /// the `tracks` document that overlaps the trip's span (its range, else its photos' capture times; none without
    /// either), in the order the document has them.
    pub fn trip_tracks(&self, id: &str) -> Result<Vec<String>> {
        let t = self.trip_item(id)?;
        let mut out: Vec<String> = Vec::new();
        let chunks = self.tracks.items().iter().filter_map(|c| Some((str_of(c, "session")?, c.get("startMs")?.as_i64()?, c.get("endMs")?.as_i64()?)));
        match t.get("tracks").and_then(Value::as_array) {
            Some(listed) => {
                let listed: HashSet<&str> = listed.iter().filter_map(Value::as_str).collect();
                for (session, _, _) in chunks {
                    if listed.contains(session) && !out.iter().any(|o| o == session) {
                        out.push(session.to_string());
                    }
                }
            }
            None => {
                let Some((from, to)) = self.trip_span(t, &self.photos_of_trip(t, None).ids) else { return Ok(out) };
                for (session, start, end) in chunks {
                    if end >= from && start <= to && !out.iter().any(|o| o == session) {
                        out.push(session.to_string());
                    }
                }
            }
        }
        Ok(out)
    }

    /// The modes trip `id` gives estimated legs: `(atMs, mode)` (a leg leaving at `atMs`, ±60 s, takes `mode`), by time.
    pub fn trip_overrides(&self, id: &str) -> Result<Vec<(i64, String)>> {
        let t = self.trip_item(id)?;
        let mut out: Vec<(i64, String)> = t
            .get("overrides")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|o| Some((o.get("atMs")?.as_i64()?, o.get("mode")?.as_str()?.to_string()))).collect())
            .unwrap_or_default();
        out.sort();
        Ok(out)
    }

    /// New trip `t` (without its id: one is made), refused past [`TRIPS_LIMIT`]. Its id.
    pub(crate) fn add_trip(&mut self, mut t: Map<String, Value>) -> Result<String> {
        if self.trips.trips.len() >= TRIPS_LIMIT {
            return Err(EngineError::Other(format!("limit: at most {TRIPS_LIMIT} trips; delete some first")));
        }
        let id = new_id('t', &self.trips.trips);
        t.insert("id".into(), json!(id));
        self.change_doc(Doc::Trips, true, |trips| trips.push(Value::Object(t)))?;
        Ok(id)
    }

    /// New note `n` (without its id), refused past [`NOTES_LIMIT`]. Its id.
    pub(crate) fn add_note(&mut self, mut n: Map<String, Value>) -> Result<String> {
        if self.trips.notes.len() >= NOTES_LIMIT {
            return Err(EngineError::Other(format!("limit: at most {NOTES_LIMIT} trip notes; delete some first")));
        }
        let id = new_id('n', &self.trips.notes);
        n.insert("id".into(), json!(id));
        self.change_doc(Doc::Notes, true, |notes| notes.push(Value::Object(n)))?;
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trip() -> Value {
        json!({"id": "t0123456789abcdef", "name": "Alps", "createdMs": 1, "albums": ["a:3", "u:00ff"], "range": {"fromMs": 5, "toMs": 9},
               "photosInRange": true, "tracks": "range", "overrides": [{"atMs": 7, "mode": "flight"}], "cover": {"key": "ab"}})
    }

    fn note() -> Value {
        json!({"id": "n0123456789abcdef", "trip": "t0123456789abcdef", "tMs": 3, "text": "Lunch", "createdMs": 2, "lat": 46.0, "lon": 7.0})
    }

    fn with(v: &Value, k: &str, x: Value) -> Value {
        let mut v = v.clone();
        v[k] = x;
        v
    }

    fn without(v: &Value, k: &str) -> Value {
        let mut v = v.clone();
        v.as_object_mut().unwrap().remove(k);
        v
    }

    #[test]
    fn trips_from_elsewhere_are_checked_item_by_item() {
        let good = trip();
        assert_eq!(check_trip(&good), Ok(true));
        // (the optional fields can be absent or null; fields this build doesn't know are kept)
        let lean = json!({"id": "tA_b-1", "name": "", "createdMs": -5});
        assert_eq!(check_trip(&lean), Ok(true));
        assert_eq!(check_trip(&with(&good, "cover", Value::Null)), Ok(true));
        assert_eq!(check_trip(&with(&good, "tracks", json!(["s0123456789abcdef"]))), Ok(true));
        assert_eq!(check_trip(&with(&good, "future", json!({"x": 1}))), Ok(true));
        let bad = [
            with(&good, "id", json!("x0123")),
            with(&good, "id", json!("t")),
            with(&good, "id", json!("t../../etc")),
            with(&good, "id", json!("t a")),
            with(&good, "id", json!("t:1")),
            with(&good, "id", json!(format!("t{}", "a".repeat(ID_MAX)))),
            with(&good, "id", json!(7)),
            with(&good, "name", json!(42)),
            with(&good, "name", json!("x".repeat(NAME_MAX + 1))),
            without(&good, "name"),
            without(&good, "createdMs"),
            with(&good, "createdMs", json!("1")),
            with(&good, "cover", json!("ab")),
            with(&good, "cover", json!({"key": 5})),
            with(&good, "albums", json!("a:3")),
            with(&good, "albums", json!([3])),
            with(&good, "albums", json!(["a".repeat(REF_MAX + 1)])),
            with(&good, "albums", json!(vec!["a:1"; LIST_MAX + 1])),
            with(&good, "range", json!({"fromMs": 9, "toMs": 5})),
            with(&good, "range", json!({"fromMs": 9})),
            with(&good, "range", json!([1, 2])),
            with(&good, "photosInRange", json!("yes")),
            with(&good, "tracks", json!("all")),
            with(&good, "tracks", json!([1])),
            with(&good, "overrides", json!([{"atMs": "7", "mode": "walk"}])),
            with(&good, "overrides", json!([{"atMs": 7, "mode": "x".repeat(MODE_MAX + 1)}])),
            with(&good, "fmt", json!(0)),
            with(&good, "fmt", json!("2")),
            json!("a trip"),
            json!(null),
        ];
        for v in &bad {
            assert!(check_trip(v).is_err(), "{}", v.to_string().chars().take(200).collect::<String>());
        }
        // a newer format is kept unread, whatever else it holds
        let newer = json!({"id": "t9", "fmt": 2, "name": {"by": "locale"}});
        assert_eq!(check_trip(&newer), Ok(false));
        assert!(!readable(&newer) && readable(&good));
    }

    #[test]
    fn notes_from_elsewhere_are_checked_item_by_item() {
        let good = note();
        assert_eq!(check_note(&good), Ok(true));
        assert_eq!(check_note(&without(&without(&good, "lat"), "lon")), Ok(true), "a note without a place");
        assert_eq!(check_note(&with(&good, "trip", json!("tNotHere"))), Ok(true), "its trip may not be here yet");
        let bad = [
            with(&good, "id", json!("t0123")),
            with(&good, "id", json!("n/1")),
            with(&good, "trip", json!("n0123")),
            without(&good, "trip"),
            without(&good, "tMs"),
            with(&good, "tMs", json!(1.5)),
            without(&good, "createdMs"),
            without(&good, "text"),
            with(&good, "text", json!("x".repeat(TEXT_MAX + 1))),
            with(&good, "text", json!(["a"])),
            with(&good, "photo", json!(1)),
            with(&good, "lat", json!(91.0)),
            with(&good, "lon", json!("7")),
            without(&good, "lon"),
            with(&good, "lat", Value::Null),
        ];
        for v in &bad {
            assert!(check_note(v).is_err(), "{}", v.to_string().chars().take(200).collect::<String>());
        }
        // a text of 10 000 characters of four bytes each is still one
        assert_eq!(check_note(&with(&good, "text", json!("🏔".repeat(TEXT_MAX)))), Ok(true));
    }

    #[test]
    fn what_is_taken_stays_within_what_a_server_keeps() {
        let good = trip();
        let many: Vec<Value> = (0..10_000).map(|i| with(&good, "id", json!(format!("t{i}")))).collect();
        let (kept, dropped) = take(Doc::Trips, &many);
        assert_eq!((kept.len(), dropped), (ITEMS_TAKEN_MAX, 10_000 - ITEMS_TAKEN_MAX));
        // a repeated id: the first counts
        let (kept, dropped) = take(Doc::Trips, &[good.clone(), with(&good, "name", json!("Again")), json!(1)]);
        assert_eq!((kept, dropped), (vec![good.clone()], 2));
        // big notes: no more than the server's bytes
        let big = with(&note(), "text", json!("é".repeat(TEXT_MAX)));
        let notes: Vec<Value> = (0..1500).map(|i| with(&big, "id", json!(format!("n{i}")))).collect();
        let (kept, dropped) = take(Doc::Notes, &notes);
        assert!(dropped > 0 && serde_json::to_vec(&kept).unwrap().len() <= BYTES_TAKEN_MAX, "{dropped}");
    }

    #[test]
    fn a_capture_time_reads_as_the_route_reads_it() {
        use lightcraft_catalog::Source;
        let photo = |captured: &str, gps: Option<(f64, f64)>| {
            let mut p = Photo::new(PhotoId(1), Source::Demo { scene: 0 }, "a.jpg", "JPEG", 10, 10, "2026-01-01");
            p.captured = Some(captured.to_string());
            p.meta.gps = gps;
            p
        };
        let utc = 1_700_000_000_000i64;
        let wall = "2023-11-14T22:13:20";
        assert_eq!(capture_ms(&photo("2023-11-14T22:13:20.250Z", None)), Some(utc + 250));
        assert_eq!(capture_ms(&photo("2023-11-14T23:13:20+01:00", Some((0.0, 100.0)))), Some(utc), "its own zone wins");
        assert_eq!(capture_ms(&photo(wall, None)), Some(utc), "no zone, no place: UTC");
        // no zone, a place 45° east: three hours ahead of UTC
        let east = photo("2023-11-15T01:13:20", Some((10.0, 45.0)));
        assert_eq!(capture_ms(&east), Some(utc));
        let cat = {
            let mut c = lightcraft_catalog::Catalog::new();
            c.apply(lightcraft_catalog::Op::AddPhoto { photo: Box::new(east.clone()) }).unwrap();
            c
        };
        let fix = crate::map::route_fixes(&cat, &[PhotoId(1)]).fixes.remove(0);
        assert_eq!((fix.time * 1000.0).round() as i64, utc, "the same moment the route places it at");
        assert_eq!(capture_ms(&photo("2023-11-14", None)), None, "a date alone");
        assert_eq!(capture_ms(&photo("", None)), None);
    }
}
