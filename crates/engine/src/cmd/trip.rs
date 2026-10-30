//! Trips (`trip.*`) and the album references they keep (`album.ref`, `album.resolve`, `album.trips`). The trips and
//! their notes are the shared documents `trips` and `trip-notes` ([`crate::trips`]), not the catalog: none of these is
//! an undo step but `trip.toAlbum`, which makes an album. Like `track.*` they stay out of the command journal (declared
//! as queries): a note is a person's own words.

use std::collections::{HashMap, HashSet};

use lightcraft_catalog::{Album, AlbumId, Op, PhotoId};
use serde_json::{Map, Value, json};

use super::{CommandSpec, always, bad, cmd, str_param};
use crate::trips::{self as store, Doc, LIST_MAX, Located, MODES, NAME_MAX, TEXT_MAX, capture_ms, id_ok, readable, refs_of, str_of};
use crate::{EngineError, Result, Session};

fn id_param<'a>(p: &'a Value, key: &str, c: &str) -> Result<&'a str> {
    str_param(p, key).ok_or_else(|| bad(c, format!("missing `{key}`")))
}

fn album_param(p: &Value, key: &str, c: &str) -> Result<AlbumId> {
    p.get(key).and_then(Value::as_u64).map(AlbumId).ok_or_else(|| bad(c, format!("`{key}` is an album id")))
}

/// A trip's name: trimmed, 1..200 characters.
fn name_param(p: &Value, c: &str) -> Result<Option<String>> {
    match p.get("name") {
        None => Ok(None),
        Some(Value::String(s)) if s.trim().is_empty() => Err(bad(c, "a trip's name can't be empty")),
        Some(Value::String(s)) if s.trim().chars().count() > NAME_MAX => Err(bad(c, format!("`name` is at most {NAME_MAX} characters"))),
        Some(Value::String(s)) => Ok(Some(s.trim().to_string())),
        Some(_) => Err(bad(c, "`name` is text")),
    }
}

fn ms_param(p: &Value, key: &str, c: &str) -> Result<Option<i64>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_i64().map(Some).ok_or_else(|| bad(c, format!("`{key}` is milliseconds since 1970 (a whole number)"))),
    }
}

/// The references to the albums `p[key]` lists (album ids), each once.
fn albums_param(s: &mut Session, p: &Value, key: &str, c: &str) -> Result<Vec<Value>> {
    let ids = match p.get(key) {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Array(a)) if a.len() <= LIST_MAX => a,
        Some(_) => return Err(bad(c, format!("`{key}` is a list of at most {LIST_MAX} album ids"))),
    };
    let mut refs: Vec<Value> = Vec::new();
    for v in ids {
        let id = v.as_u64().map(AlbumId).ok_or_else(|| bad(c, format!("`{key}` is a list of album ids")))?;
        let r = json!(s.album_ref(id)?);
        if !refs.contains(&r) {
            refs.push(r);
        }
    }
    Ok(refs)
}

/// `range`: `{fromMs, toMs}`, from not after to; `null` clears it.
fn range_param(v: &Value, c: &str) -> Result<Option<Value>> {
    if v.is_null() {
        return Ok(None);
    }
    match (v.get("fromMs").and_then(Value::as_i64), v.get("toMs").and_then(Value::as_i64)) {
        (Some(f), Some(t)) if f <= t => Ok(Some(json!({"fromMs": f, "toMs": t}))),
        _ => Err(bad(c, "`range` is {fromMs, toMs}, milliseconds since 1970, fromMs not after toMs")),
    }
}

/// `tracks`: `"range"` (the recorded sessions that overlap the trip; also what `null` means) or a list of sessions.
fn tracks_param(v: &Value, c: &str) -> Result<Value> {
    match v {
        Value::Null => Ok(json!("range")),
        Value::String(r) if r == "range" => Ok(json!("range")),
        Value::Array(a) if a.len() <= LIST_MAX && a.iter().all(|t| t.as_str().is_some_and(|t| id_ok(t, 's'))) => {
            let mut seen = HashSet::new();
            Ok(Value::Array(a.iter().filter(|t| seen.insert(t.as_str())).cloned().collect()))
        }
        _ => Err(bad(c, format!("`tracks` is \"range\" or a list of at most {LIST_MAX} recorded sessions (`s…`)"))),
    }
}

/// `overrides`: `[{atMs, mode}]` (`null`: none).
fn overrides_param(v: &Value, c: &str) -> Result<Value> {
    let Some(a) = v.as_array().filter(|a| a.len() <= LIST_MAX) else {
        return if v.is_null() { Ok(json!([])) } else { Err(bad(c, format!("`overrides` is a list of at most {LIST_MAX} {{atMs, mode}}"))) };
    };
    let mut out = Vec::new();
    for o in a {
        match (o.get("atMs").and_then(Value::as_i64), o.get("mode").and_then(Value::as_str)) {
            (Some(at), Some(mode)) if MODES.contains(&mode) => out.push(json!({"atMs": at, "mode": mode})),
            _ => return Err(bad(c, format!("an override is {{atMs, mode}}, mode one of {}", MODES.join(", ")))),
        }
    }
    Ok(Value::Array(out))
}

/// The content key of photo `id` (a trip's cover, a note's photo).
fn photo_key(s: &Session, id: u64, c: &str) -> Result<String> {
    let p = s.catalog.photo(PhotoId(id)).ok_or_else(|| EngineError::Other(format!("no such photo: {id}")))?;
    lightcraft_catalog::sync::content_key(p).ok_or_else(|| bad(c, format!("photo {id} has no content hash yet: try again once it is read")))
}

/// `cover`: a photo id; `null` clears it.
fn cover_param(s: &Session, v: &Value, c: &str) -> Result<Option<Value>> {
    match v {
        Value::Null => Ok(None),
        v => {
            let id = v.as_u64().ok_or_else(|| bad(c, "`cover` is a photo id"))?;
            Ok(Some(json!({"key": photo_key(s, id, c)?})))
        }
    }
}

/// The photo the content key `key` names here.
fn photo_of_key(index: &HashMap<String, PhotoId>, key: Option<&str>) -> Value {
    json!(key.and_then(|k| index.get(k)).map(|id| id.0))
}

/// `trip.get`'s answer for trip `t` (and what the commands that change one answer).
fn trip_answer(s: &Session, t: &Value) -> Value {
    let id = str_of(t, "id").unwrap_or_default();
    let linked: Vec<Value> = refs_of(t)
        .iter()
        .map(|r| {
            let album = s.resolve_album_ref(r).and_then(|a| s.catalog.album(a)).map(|a| json!({"id": a.id.0, "name": a.name}));
            json!({"ref": r, "album": album})
        })
        .collect();
    let cover = t.get("cover").and_then(|c| str_of(c, "key"));
    let index = if cover.is_some() { crate::icloud::collections::index(s) } else { HashMap::new() };
    let photos = if readable(t) { s.photos_of_trip(t, None).ids.len() } else { 0 };
    json!({
        "trip": t,
        "linked": linked,
        "coverPhoto": photo_of_key(&index, cover),
        "photos": photos,
        "notes": s.trips.items(Doc::Notes).iter().filter(|n| str_of(n, "trip") == Some(id)).count(),
        "unreadable": !readable(t),
    })
}

fn answer_for(s: &Session, id: &str) -> Result<Value> {
    Ok(trip_answer(s, s.trip_item(id)?))
}

/// Trip `id`, one this build can change.
fn changeable(s: &Session, id: &str) -> Result<()> {
    if readable(s.trip_item(id)?) {
        Ok(())
    } else {
        Err(EngineError::Other(format!("trip {id} was saved by a newer version: update this one to change it")))
    }
}

/// Change trip `id`'s fields with `f` (on its object) and save.
fn change_trip(s: &mut Session, id: &str, f: impl FnOnce(&mut Map<String, Value>)) -> Result<()> {
    s.change_doc(Doc::Trips, true, |trips| {
        if let Some(o) = trips.iter_mut().find(|t| str_of(t, "id") == Some(id)).and_then(Value::as_object_mut) {
            f(o);
        }
    })
}

fn create(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "trip.create";
    s.trips_follow_mode();
    let name = name_param(p, C)?.ok_or_else(|| bad(C, "missing `name`"))?;
    let mut t = Map::new();
    t.insert("name".into(), json!(name));
    t.insert("createdMs".into(), json!(store::now_ms()));
    t.insert("albums".into(), Value::Array(albums_param(s, p, "albums", C)?));
    if let Some(r) = p.get("range").map(|r| range_param(r, C)).transpose()?.flatten() {
        t.insert("range".into(), r);
    }
    let in_range = p.get("photosInRange").filter(|v| !v.is_null()).map(|v| v.as_bool().ok_or_else(|| bad(C, "`photosInRange` is true or false")));
    t.insert("photosInRange".into(), json!(in_range.transpose()?.unwrap_or(false)));
    t.insert("tracks".into(), tracks_param(p.get("tracks").unwrap_or(&Value::Null), C)?);
    t.insert("overrides".into(), json!([]));
    if let Some(c) = cover_param(s, p.get("cover").unwrap_or(&Value::Null), C)? {
        t.insert("cover".into(), c);
    }
    let id = s.add_trip(t)?;
    answer_for(s, &id)
}

fn from_album(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "trip.fromAlbum";
    s.trips_follow_mode();
    let album = album_param(p, "album", C)?;
    let r = s.album_ref(album)?;
    let a = s.catalog.album(album).ok_or_else(|| EngineError::Other(format!("no such album: {}", album.0)))?;
    let name = match name_param(p, C)? {
        Some(n) => n,
        None => a.name.trim().chars().take(NAME_MAX).collect::<String>(),
    };
    let times: Vec<i64> =
        a.photos.iter().filter_map(|id| s.catalog.photo(*id)).filter(|p| !p.deleted && !p.hidden).filter_map(|p| capture_ms(p)).collect();
    let mut t = Map::new();
    t.insert("name".into(), json!(if name.is_empty() { "Trip".to_string() } else { name }));
    t.insert("createdMs".into(), json!(store::now_ms()));
    t.insert("albums".into(), json!([r]));
    if let (Some(from), Some(to)) = (times.iter().min(), times.iter().max()) {
        t.insert("range".into(), json!({"fromMs": from, "toMs": to}));
    }
    t.insert("photosInRange".into(), json!(false));
    t.insert("tracks".into(), json!("range"));
    t.insert("overrides".into(), json!([]));
    let id = s.add_trip(t)?;
    answer_for(s, &id)
}

fn to_album(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "trip.toAlbum";
    s.trips_follow_mode();
    let id = id_param(p, "id", C)?;
    let t = s.trip_item(id)?;
    let fallback = str_of(t, "name").map(str::trim).filter(|n| !n.is_empty()).unwrap_or("Trip").to_string();
    let name = name_param(p, C)?.unwrap_or(fallback);
    let photos: Vec<PhotoId> = s.photos_of_trip(t, None).ids.into_iter().filter(|id| s.catalog.photo(*id).is_some_and(|p| !p.local)).collect();
    // (the trip's cover, when the album holds it; else its first photo)
    let key = t.get("cover").and_then(|c| str_of(c, "key"));
    let cover = photos
        .iter()
        .copied()
        .find(|id| key.is_some() && s.catalog.photo(*id).and_then(|p| lightcraft_catalog::sync::content_key(p)).as_deref() == key);
    let album = s.catalog.alloc_album_id();
    let n = photos.len();
    let a = Album { cover: cover.or_else(|| photos.first().copied()), photos, ..Album::new(album, name) };
    s.commit("New Album", Op::AddAlbum { album: a })?;
    Ok(json!({"album": album.0, "photos": n}))
}

fn update(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "trip.update";
    s.trips_follow_mode();
    let id = id_param(p, "id", C)?.to_string();
    changeable(s, &id)?;
    if p.get("name").is_some_and(Value::is_null) {
        return Err(bad(C, "a trip's name can't be cleared"));
    }
    // (everything checked before anything changes)
    let mut set: Vec<(&str, Option<Value>)> = Vec::new();
    if let Some(n) = name_param(p, C)? {
        set.push(("name", Some(json!(n))));
    }
    if p.get("albums").is_some() {
        set.push(("albums", Some(Value::Array(albums_param(s, p, "albums", C)?))));
    }
    if let Some(r) = p.get("range") {
        set.push(("range", range_param(r, C)?));
    }
    if let Some(v) = p.get("photosInRange") {
        let on = if v.is_null() { false } else { v.as_bool().ok_or_else(|| bad(C, "`photosInRange` is true or false"))? };
        set.push(("photosInRange", Some(json!(on))));
    }
    if let Some(v) = p.get("tracks") {
        set.push(("tracks", Some(tracks_param(v, C)?)));
    }
    if let Some(v) = p.get("overrides") {
        set.push(("overrides", Some(overrides_param(v, C)?)));
    }
    if let Some(v) = p.get("cover") {
        set.push(("cover", cover_param(s, v, C)?));
    }
    let same = s.trip_item(&id).is_ok_and(|t| set.iter().all(|(k, v)| t.get(*k) == v.as_ref()));
    if !same {
        change_trip(s, &id, |o| {
            for (k, v) in set {
                match v {
                    Some(v) => drop(o.insert(k.into(), v)),
                    None => drop(o.remove(k)),
                }
            }
        })?;
    }
    answer_for(s, &id)
}

fn link(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "trip.link";
    s.trips_follow_mode();
    let id = id_param(p, "id", C)?.to_string();
    changeable(s, &id)?;
    let album = album_param(p, "album", C)?;
    let r = s.album_ref(album)?;
    let t = s.trip_item(&id)?;
    if refs_of(t).len() >= LIST_MAX {
        return Err(EngineError::Other(format!("limit: a trip links at most {LIST_MAX} albums")));
    }
    if !refs_of(t).iter().any(|x| *x == r || s.resolve_album_ref(x) == Some(album)) {
        change_trip(s, &id, |o| {
            let mut refs: Vec<Value> = o.get("albums").and_then(Value::as_array).cloned().unwrap_or_default();
            refs.push(json!(r));
            o.insert("albums".into(), Value::Array(refs));
        })?;
    }
    answer_for(s, &id)
}

fn unlink(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "trip.unlink";
    s.trips_follow_mode();
    let id = id_param(p, "id", C)?.to_string();
    changeable(s, &id)?;
    // (by album id, or by the reference itself: one whose album is gone can only be named so)
    let gone: Vec<String> = match (p.get("album").filter(|v| !v.is_null()), str_param(p, "ref")) {
        (Some(_), None) => {
            let album = album_param(p, "album", C)?;
            refs_of(s.trip_item(&id)?)
                .into_iter()
                .filter(|r| s.resolve_album_ref(r) == Some(album) || *r == format!("a:{}", album.0))
                .map(str::to_string)
                .collect()
        }
        (None, Some(r)) => vec![r.to_string()],
        _ => return Err(bad(C, "give `album` (an album id) or `ref` (a reference trip.get lists)")),
    };
    if refs_of(s.trip_item(&id)?).iter().any(|r| gone.iter().any(|g| g == r)) {
        change_trip(s, &id, |o| {
            if let Some(a) = o.get_mut("albums").and_then(Value::as_array_mut) {
                a.retain(|r| !r.as_str().is_some_and(|r| gone.iter().any(|g| g == r)));
            }
        })?;
    }
    answer_for(s, &id)
}

fn list(s: &mut Session, _: &Value) -> Result<Value> {
    s.trips_follow_mode();
    let located = Located::of(s);
    let trips = s.trips.items(Doc::Trips);
    let index = if trips.iter().any(|t| t.get("cover").is_some()) { crate::icloud::collections::index(s) } else { HashMap::new() };
    let mut notes: HashMap<&str, usize> = HashMap::new();
    for n in s.trips.items(Doc::Notes) {
        if let Some(t) = str_of(n, "trip") {
            *notes.entry(t).or_default() += 1;
        }
    }
    let mut rows: Vec<(i64, Value)> = Vec::new();
    for t in trips {
        let id = str_of(t, "id").unwrap_or_default();
        let mut o = Map::new();
        o.insert("id".into(), json!(id));
        o.insert("name".into(), t.get("name").cloned().unwrap_or(Value::Null));
        o.insert("createdMs".into(), t.get("createdMs").cloned().unwrap_or(Value::Null));
        o.insert("notes".into(), json!(notes.get(id).copied().unwrap_or(0)));
        let created = t.get("createdMs").and_then(Value::as_i64).unwrap_or(0);
        if !readable(t) {
            o.insert("unreadable".into(), json!(true));
            rows.push((created, Value::Object(o)));
            continue;
        }
        let photos = s.photos_of_trip(t, Some(&located)).ids;
        o.insert("cover".into(), photo_of_key(&index, t.get("cover").and_then(|c| str_of(c, "key"))));
        o.insert("photos".into(), json!(photos.len()));
        o.insert("albums".into(), json!(refs_of(t).len()));
        if let Some(r) = t.get("range").filter(|r| !r.is_null()) {
            o.insert("range".into(), r.clone());
        }
        let span = s.trip_span(t, &photos);
        if let Some((start, end)) = span {
            o.insert("startMs".into(), json!(start));
            o.insert("endMs".into(), json!(end));
        }
        o.insert("unreadable".into(), json!(false));
        rows.push((span.map_or(created, |(start, _)| start), Value::Object(o)));
    }
    // (the latest first)
    rows.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
    Ok(json!({"trips": rows.into_iter().map(|(_, v)| v).collect::<Vec<_>>()}))
}

fn get(s: &mut Session, p: &Value) -> Result<Value> {
    s.trips_follow_mode();
    answer_for(s, id_param(p, "id", "trip.get")?)
}

fn photos(s: &mut Session, p: &Value) -> Result<Value> {
    s.trips_follow_mode();
    let t = s.trip_item(id_param(p, "id", "trip.photos")?)?;
    let found = s.photos_of_trip(t, None);
    Ok(json!({"ids": found.ids.iter().map(|id| id.0).collect::<Vec<_>>(), "fromAlbums": found.from_albums, "fromRange": found.from_range}))
}

fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    let id = id_param(p, "id", "trip.delete")?.to_string();
    s.trip_item(&id)?;
    let of_trip = |n: &Value| str_of(n, "trip") == Some(id.as_str());
    let notes = s.trips.items(Doc::Notes).iter().filter(|n| of_trip(n)).count();
    s.change_doc(Doc::Trips, false, |trips| trips.retain(|t| str_of(t, "id") != Some(id.as_str())))?;
    if notes > 0 {
        s.change_doc(Doc::Notes, false, |all| all.retain(|n| !of_trip(n)))?;
    }
    Ok(json!({"removed": true, "notes": notes}))
}

// ---- notes

/// `lat`/`lon`, given together: `None` neither given, `Some(None)` both `null`, `Some(Some(..))` a position.
fn place_param(p: &Value, c: &str) -> Result<Option<Option<(f64, f64)>>> {
    match (p.get("lat"), p.get("lon")) {
        (None, None) => Ok(None),
        (Some(Value::Null), Some(Value::Null)) => Ok(Some(None)),
        (Some(a), Some(b)) => match (a.as_f64().filter(|d| d.abs() <= 90.0), b.as_f64().filter(|d| d.abs() <= 180.0)) {
            (Some(lat), Some(lon)) => Ok(Some(Some((lat, lon)))),
            _ => Err(bad(c, "`lat` is -90..90 and `lon` -180..180 degrees")),
        },
        _ => Err(bad(c, "give `lat` and `lon` together")),
    }
}

fn text_param(p: &Value, c: &str) -> Result<Option<String>> {
    match p.get("text") {
        None => Ok(None),
        Some(Value::String(t)) if t.chars().count() > TEXT_MAX => Err(bad(c, format!("`text` is at most {TEXT_MAX} characters"))),
        Some(Value::String(t)) => Ok(Some(t.clone())),
        Some(_) => Err(bad(c, "`text` is text")),
    }
}

fn no_note(id: &str) -> EngineError {
    EngineError::Other(format!("no such note: {}", id.chars().take(64).collect::<String>()))
}

/// Note `n` as the commands show it: as stored, with `photoId`, the photo its `photo` names here (or `null`).
fn note_json(n: &Value, index: &HashMap<String, PhotoId>) -> Value {
    let mut n = n.clone();
    if let Some(o) = n.as_object_mut()
        && o.get("photo").is_some_and(Value::is_string)
    {
        let id = photo_of_key(index, o.get("photo").and_then(Value::as_str));
        o.insert("photoId".into(), id);
    }
    n
}

fn note_answer(s: &Session, id: &str) -> Result<Value> {
    let n = s.trips.items(Doc::Notes).iter().find(|n| str_of(n, "id") == Some(id)).ok_or_else(|| no_note(id))?;
    let index = if n.get("photo").is_some_and(Value::is_string) { crate::icloud::collections::index(s) } else { HashMap::new() };
    Ok(json!({"note": note_json(n, &index)}))
}

fn note_add(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "trip.note.add";
    let trip = id_param(p, "trip", C)?.to_string();
    s.trip_item(&trip)?;
    let text = text_param(p, C)?.ok_or_else(|| bad(C, "missing `text`"))?;
    let place = place_param(p, C)?.flatten();
    let photo = match p.get("photo").filter(|v| !v.is_null()) {
        Some(v) => Some(v.as_u64().ok_or_else(|| bad(C, "`photo` is a photo id"))?),
        None => None,
    };
    let key = photo.map(|id| photo_key(s, id, C)).transpose()?;
    // (when: as given, else when its photo was taken, else now)
    let t = match ms_param(p, "tMs", C)? {
        Some(t) => t,
        None => photo.and_then(|id| s.catalog.photo(PhotoId(id))).and_then(|p| capture_ms(p)).unwrap_or_else(store::now_ms),
    };
    let mut n = Map::new();
    n.insert("trip".into(), json!(trip));
    n.insert("tMs".into(), json!(t));
    if let Some((lat, lon)) = place {
        n.insert("lat".into(), json!(lat));
        n.insert("lon".into(), json!(lon));
    }
    n.insert("text".into(), json!(text));
    if let Some(k) = key {
        n.insert("photo".into(), json!(k));
    }
    n.insert("createdMs".into(), json!(store::now_ms()));
    let id = s.add_note(n)?;
    note_answer(s, &id)
}

fn note_update(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "trip.note.update";
    let id = id_param(p, "id", C)?.to_string();
    let n = s.trips.items(Doc::Notes).iter().find(|n| str_of(n, "id") == Some(id.as_str())).ok_or_else(|| no_note(&id))?;
    if !readable(n) {
        return Err(EngineError::Other(format!("note {id} was saved by a newer version: update this one to change it")));
    }
    let (text, t, place) = (text_param(p, C)?, ms_param(p, "tMs", C)?, place_param(p, C)?);
    let mut set: Vec<(&str, Option<Value>)> = Vec::new();
    set.extend(text.map(|x| ("text", Some(json!(x)))));
    set.extend(t.map(|x| ("tMs", Some(json!(x)))));
    if let Some(place) = place {
        set.push(("lat", place.map(|(lat, _)| json!(lat))));
        set.push(("lon", place.map(|(_, lon)| json!(lon))));
    }
    if !set.iter().all(|(k, v)| n.get(*k) == v.as_ref()) {
        s.change_doc(Doc::Notes, true, |notes| {
            if let Some(o) = notes.iter_mut().find(|n| str_of(n, "id") == Some(id.as_str())).and_then(Value::as_object_mut) {
                for (k, v) in set {
                    match v {
                        Some(v) => drop(o.insert(k.into(), v)),
                        None => drop(o.remove(k)),
                    }
                }
            }
        })?;
    }
    note_answer(s, &id)
}

fn note_delete(s: &mut Session, p: &Value) -> Result<Value> {
    let id = id_param(p, "id", "trip.note.delete")?.to_string();
    if !s.trips.items(Doc::Notes).iter().any(|n| str_of(n, "id") == Some(id.as_str())) {
        return Err(no_note(&id));
    }
    s.change_doc(Doc::Notes, false, |notes| notes.retain(|n| str_of(n, "id") != Some(id.as_str())))?;
    Ok(json!({"removed": true}))
}

fn notes(s: &mut Session, p: &Value) -> Result<Value> {
    let trip = id_param(p, "trip", "trip.notes")?;
    s.trip_item(trip)?;
    let mut mine: Vec<&Value> = s.trips.items(Doc::Notes).iter().filter(|n| str_of(n, "trip") == Some(trip)).collect();
    mine.sort_by_key(|n| (n.get("tMs").and_then(Value::as_i64), n.get("createdMs").and_then(Value::as_i64), str_of(n, "id")));
    let index = if mine.iter().any(|n| n.get("photo").is_some_and(Value::is_string)) { crate::icloud::collections::index(s) } else { HashMap::new() };
    Ok(json!({"notes": mine.iter().map(|n| note_json(n, &index)).collect::<Vec<_>>()}))
}

// ---- album references

fn album_ref(s: &mut Session, p: &Value) -> Result<Value> {
    s.trips_follow_mode();
    let id = album_param(p, "id", "album.ref")?;
    Ok(json!({"ref": s.album_ref(id)?}))
}

fn album_resolve(s: &mut Session, p: &Value) -> Result<Value> {
    s.trips_follow_mode();
    let r = id_param(p, "ref", "album.resolve")?;
    Ok(match s.resolve_album_ref(r).and_then(|id| s.catalog.album(id)) {
        Some(a) => json!({"id": a.id.0, "name": a.name}),
        None => json!({"id": null}),
    })
}

fn album_trips(s: &mut Session, p: &Value) -> Result<Value> {
    s.trips_follow_mode();
    let album = album_param(p, "id", "album.trips")?;
    let trips: Vec<Value> = s
        .trips
        .items(Doc::Trips)
        .iter()
        .filter(|t| refs_of(t).iter().any(|r| s.resolve_album_ref(r) == Some(album)))
        .map(|t| json!({"id": t.get("id"), "name": t.get("name")}))
        .collect();
    Ok(json!({"trips": trips}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "trip.create", "New Trip", [], None,
            "{name, albums?: [album id], range?: {fromMs, toMs}, photosInRange?: bool (false), tracks?: \"range\" | [session] (\"range\"), cover?: photo id} → as trip.get. A trip of the shared `trips` document: its photos are the members of the albums it links and, with photosInRange, the located photos captured inside its range. Refused with `limit: …` past 500 trips. Times are milliseconds since 1970 (UTC). Not an undo step",
            always, create),
        cmd!(query "trip.fromAlbum", "Trip from Album", [], None,
            "{album: album id, name? (the album's)} → as trip.get: a trip that links the album, its range the span of the album's photos' capture times, photosInRange false",
            always, from_album),
        cmd!(
            "trip.toAlbum",
            "Album from Trip",
            [],
            None,
            "{id, name? (the trip's)} → {album: album id, photos: n}: a new album holding the trip's photos (Local photos left out). One undo step; the trip doesn't link it",
            always,
            to_album
        ),
        cmd!(query "trip.update", "Edit Trip", [], None,
            "{id, name?, albums?: [album id], range?: {fromMs, toMs}, photosInRange?, tracks?: \"range\" | [session], overrides?: [{atMs, mode: walk|bike|road|flight}], cover?: photo id} → as trip.get. Only what is given changes; `null` clears (albums and overrides to none, tracks to \"range\"; a name can't be cleared)",
            always, update),
        cmd!(query "trip.link", "Link Album to Trip", [], None,
            "{id, album: album id} → as trip.get. Folders, smart albums and the Quick Collection are refused",
            always, link),
        cmd!(query "trip.unlink", "Unlink Album from Trip", [], None,
            "{id, album: album id | ref: an album reference trip.get lists (for one whose album is gone)} → as trip.get",
            always, unlink),
        cmd!(query "trip.list", "Trips", [], None,
            "{} → {trips: [{id, name, createdMs, cover: photo id | null, photos: n, albums: n, notes: n, range?, startMs?, endMs?, unreadable}]}, the latest first. startMs/endMs: the range, else the span of the photos' capture times; unreadable: saved by a newer version (kept, not read)",
            always, list),
        cmd!(query "trip.get", "Trip", [], None,
            "{id} → {trip: as stored, linked: [{ref, album: {id, name} | null (deleted, or not here yet)}], coverPhoto: photo id | null, photos: n, notes: n, unreadable}",
            always, get),
        cmd!(query "trip.photos", "Trip Photos", [], None,
            "{id} → {ids: [photo id], fromAlbums: n, fromRange: n}: the members of the linked albums and, with photosInRange, the located photos captured inside the range (fromRange: those no linked album holds), each once, by capture time (photos without one last). Recently Deleted and hidden photos are left out",
            always, photos),
        cmd!(query "trip.delete", "Delete Trip", [], None,
            "{id} → {removed: true, notes: n}: the trip and its notes, on this device and, through the sync, the others. Its albums and photos stay. Not an undo step: ask first",
            always, delete),
        cmd!(query "trip.note.add", "Add Trip Note", [], None,
            "{trip, text (at most 10000 characters), tMs? (the photo's capture time, else now), lat?, lon? (together), photo?: photo id} → {note: {id, trip, tMs, lat?, lon?, text, photo?: content hash, photoId?, createdMs}}. Refused with `limit: …` past 1500 notes",
            always, note_add),
        cmd!(query "trip.note.update", "Edit Trip Note", [], None,
            "{id, text?, tMs?, lat?, lon? (together; both null clear the place)} → {note}",
            always, note_update),
        cmd!(query "trip.note.delete", "Delete Trip Note", [], None, "{id} → {removed: true}", always, note_delete),
        cmd!(query "trip.notes", "Trip Notes", [], None,
            "{trip} → {notes: [note]} by time; a note's photoId is the photo its photo (a content hash) names here, or null",
            always, notes),
        cmd!(query "album.ref", "Album Reference", [], None,
            "{id} → {ref}: a reference to a manual album that names it on every device of the library: \"a:<album id>\" when the library syncs with a server or not at all, \"u:<uid>\" through iCloud or a cloud folder (the uid it travels under, made now when it has none). Folders, smart albums and the Quick Collection are refused, with why",
            always, album_ref),
        cmd!(query "album.resolve", "Resolve Album Reference", [], None,
            "{ref} → {id, name} | {id: null} (deleted, or not on this device yet)",
            always, album_resolve),
        cmd!(query "album.trips", "Album Trips", [], None, "{id} → {trips: [{id, name}]}: the trips that link the album", always, album_trips),
    ]
}
