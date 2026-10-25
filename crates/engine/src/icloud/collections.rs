//! Albums, folders, smart albums and stacks in the iCloud record sync, one record each (`album`, `stack`).
//!
//! Ids are this library's own, so a record names its album or stack by a **uid** this device made the first time it
//! sent it ([`ICloud`]'s `uids`, kept in `icloud.json`), and its photos by their **content key** (the hash of the
//! original), the way an `edit` record names a photo. A device resolves the keys against the photos it has, whatever
//! way they came: a photo opened from the Photos library, one in the iCloud Drive folder, or one both computers
//! imported from the same file.
//!
//! - **Albums** carry the name, whether it is a folder, the folder it is in (by uid), its place among its siblings, a
//!   smart album's rules, the cover and the members. Members this device has no photo for yet are kept (`pending`) and
//!   go with the record, so another device's members are never lost here, and join the album when the photo comes.
//!   The Quick Collection is per library and stays here; smart albums whose rules name photos or an album (ids that
//!   mean nothing elsewhere) stay too.
//! - **Stacks** carry the members in order, top first, and whether it is collapsed. A stack takes the members this
//!   device has (two at least).
//! - **Merging** is by record, as for everything else here: changed on both sides, the newer change wins (so photos
//!   added to one album on two devices meanwhile are not both kept, as a server's merge would).
//! - **Applying** is not an undo step. A removal on another device removes the album or stack here unless it changed here.

use std::collections::{BTreeMap, HashMap, HashSet};

use lightcraft_catalog::{Album, AlbumId, Op, PhotoId, Stack, StackId};
use serde_json::{Value, json};

use super::{ICloud, record_name};
use crate::Session;

pub const ALBUM: &str = "album";
pub const STACK: &str = "stack";

/// Nesting of folders followed when records name each other as parents.
const DEPTH_MAX: usize = 32;
/// Members of an album or stack taken from a record, at most.
const MEMBERS_MAX: usize = 1_000_000;

pub(super) fn is_collection(kind: &str) -> bool {
    kind == ALBUM || kind == STACK
}

/// Whether `op` may change what an album or stack record says (or which of its photos this device has).
pub(super) fn touches(op: &Op, depth: usize) -> bool {
    match op {
        Op::AddAlbum { .. }
        | Op::RemoveAlbum { .. }
        | Op::RenameAlbum { .. }
        | Op::MoveAlbum { .. }
        | Op::SetAlbumOrder { .. }
        | Op::SetAlbumPhotos { .. }
        | Op::SetAlbumCover { .. }
        | Op::SetAlbumRules { .. }
        | Op::AddStack { .. }
        | Op::RemoveStack { .. }
        | Op::SetStack { .. }
        | Op::AddPhoto { .. }
        | Op::RemovePhoto { .. }
        | Op::SetContent { .. } => true,
        Op::Batch { ops } if depth < 32 => ops.iter().any(|o| touches(o, depth + 1)),
        _ => false,
    }
}

/// A new uid: random (the hasher's keys come from the OS).
fn new_uid() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::hash::RandomState::new().build_hasher();
    h.write_i64(super::now_ms());
    format!("{:016x}", h.finish())
}

fn key_of(s: &Session, id: PhotoId) -> Option<String> {
    s.catalog.photo(id).filter(|p| p.copy_of.is_none() && !p.local).and_then(|p| lightcraft_catalog::sync::content_key(p))
}

/// The photos this device has by content key (the first of each: copies are not the picture's).
fn index(s: &Session) -> HashMap<String, PhotoId> {
    let mut out = HashMap::new();
    for p in s.catalog.photos().filter(|p| p.copy_of.is_none() && !p.local) {
        if let Some(k) = lightcraft_catalog::sync::content_key(p) {
            out.entry(k).or_insert(p.id);
        }
    }
    out
}

impl ICloud {
    /// The uid of this library's album or stack `local` (`a12`, `s3`), made when first asked.
    fn uid(&mut self, local: String) -> String {
        if let Some(u) = self.saved.uids.get(&local) {
            return u.clone();
        }
        let u = new_uid();
        self.saved.uids.insert(local, u.clone());
        self.saved_dirty = true;
        u
    }

    fn local_of(&self, prefix: char, uid: &str) -> Option<u64> {
        self.saved.uids.iter().find(|(k, u)| k.starts_with(prefix) && *u == uid).and_then(|(k, _)| k.get(1..)?.parse().ok())
    }
}

/// This device's albums and stacks as records, in `ic.ours` and `ic.coll`; photos that arrived since join the albums
/// that were waiting for them (`ic` is out of the session).
pub(super) fn refresh(s: &mut Session, ic: &mut ICloud) {
    let have = index(s);
    join_waiting(s, ic, &have);
    // records held for want of photos: tried again now that some may have come
    let held: Vec<(String, String, String, Option<String>)> = ic
        .held
        .iter()
        .filter(|(_, h)| is_collection(&h.kind))
        .map(|(n, h)| (n.clone(), h.kind.clone(), h.key.clone(), Some(h.data.clone())))
        .collect();
    if !held.is_empty() {
        let refs: Vec<&(String, String, String, Option<String>)> = held.iter().collect();
        apply(s, ic, &refs);
    }
    let mut records: Vec<(String, String, String)> = Vec::new(); // (kind, uid, data)
    let albums: Vec<Album> = s.catalog.albums().filter(|a| !a.quick).cloned().collect();
    for a in &albums {
        // (rules that name photos or an album mean nothing on another device)
        if a.smart.as_ref().is_some_and(|f| !f.only.is_empty() || f.album.is_some() || f.semantic.is_some()) {
            continue;
        }
        let uid = ic.uid(format!("a{}", a.id.0));
        let parent = a.parent.and_then(|p| albums.iter().find(|x| x.id == p)).map(|p| ic.uid(format!("a{}", p.id.0)));
        let mut keys: Vec<String> = a.photos.iter().filter_map(|id| key_of(s, *id)).collect();
        keys.extend(ic.saved.pending.get(&uid).cloned().unwrap_or_default());
        keys.sort();
        keys.dedup();
        let data = json!({
            "name": a.name,
            "folder": a.folder,
            "parent": parent,
            "order": a.order,
            "smart": a.smart,
            "cover": a.cover.and_then(|c| key_of(s, c)),
            "photos": keys,
        });
        records.push((ALBUM.into(), uid, data.to_string()));
    }
    let stacks: Vec<Stack> = s.catalog.stacks().cloned().collect();
    for st in &stacks {
        let keys: Vec<String> = st.photos.iter().filter_map(|id| key_of(s, *id)).collect();
        if keys.len() < 2 {
            continue;
        }
        let uid = ic.uid(format!("s{}", st.id.0));
        records.push((STACK.into(), uid, json!({"photos": keys, "collapsed": st.collapsed}).to_string()));
    }
    ic.ours.retain(|_, m| !is_collection(&m.kind));
    ic.coll.clear();
    for (kind, uid, data) in records {
        let name = record_name(&kind, &uid);
        ic.ours.insert(name.clone(), super::Mine { kind, key: uid, hash: super::hash_of(&data), parts: 0 });
        ic.coll.insert(name, data);
    }
}

/// Members an album was waiting for that this device has now join it.
fn join_waiting(s: &mut Session, ic: &mut ICloud, have: &HashMap<String, PhotoId>) {
    let waiting: Vec<(String, Vec<String>)> =
        ic.saved.pending.iter().filter(|(_, keys)| keys.iter().any(|k| have.contains_key(k))).map(|(u, k)| (u.clone(), k.clone())).collect();
    for (uid, keys) in waiting {
        let Some(id) = ic.local_of('a', &uid).map(AlbumId) else { continue };
        let Some(album) = s.catalog.album(id) else { continue };
        let mut photos = album.photos.clone();
        let (arrived, still): (Vec<String>, Vec<String>) = keys.into_iter().partition(|k| have.contains_key(k));
        photos.extend(arrived.iter().filter_map(|k| have.get(k).copied()).filter(|p| !album.photos.contains(p)));
        if let Err(e) = s.apply_system(Op::SetAlbumPhotos { id, photos }) {
            log::warn!("iCloud: album {}: {e}", id.0);
            continue;
        }
        if still.is_empty() {
            ic.saved.pending.remove(&uid);
        } else {
            ic.saved.pending.insert(uid, still);
        }
        ic.saved_dirty = true;
    }
}

/// Take album and stack records from the cloud (`data` `None`: removed there). Answers how many changed something.
pub(super) fn apply(s: &mut Session, ic: &mut ICloud, take: &[&(String, String, String, Option<String>)]) -> usize {
    let have = index(s);
    let mut applied = 0;
    let mut albums: BTreeMap<String, Value> = BTreeMap::new();
    for (name, kind, uid, data) in take.iter().map(|t| &**t).filter(|t| t.1 == ALBUM) {
        match data.as_deref().map(serde_json::from_str::<Value>) {
            Some(Ok(v)) if v.is_object() => {
                albums.insert(uid.clone(), v);
            }
            // (one this version can't read is held: it stays the cloud's, never taken for a removal here)
            Some(_) => hold(ic, name, kind, uid, data.as_deref()),
            None => applied += remove_album(s, ic, uid),
        }
    }
    // folders before what is in them
    let mut done: HashSet<String> = HashSet::new();
    let uids: Vec<String> = albums.keys().cloned().collect();
    for uid in uids {
        applied += apply_album(s, ic, &albums, &uid, &have, &mut done, 0);
        // (an album whose record this device couldn't place is held too)
        let name = record_name(ALBUM, &uid);
        let placed = ic.local_of('a', &uid).is_some_and(|id| s.catalog.album(AlbumId(id)).is_some());
        match (placed, take.iter().find(|t| t.0 == name)) {
            (true, _) => release(ic, &name),
            (false, Some((_, kind, key, data))) => hold(ic, &name, kind, key, data.as_deref()),
            (false, None) => {}
        }
    }
    for (name, kind, uid, data) in take.iter().map(|t| &**t).filter(|t| t.1 == STACK) {
        match data.as_deref().map(serde_json::from_str::<Value>) {
            Some(Ok(v)) => {
                applied += apply_stack(s, ic, uid, &v, &have);
                if ic.local_of('s', uid).is_some_and(|id| s.catalog.stack(StackId(id)).is_some()) {
                    release(ic, name);
                } else {
                    // (fewer than two of its photos are here yet)
                    hold(ic, name, kind, uid, data.as_deref());
                }
            }
            Some(Err(_)) => hold(ic, name, kind, uid, data.as_deref()),
            None => {
                applied += remove_stack(s, ic, uid);
                release(ic, name);
            }
        }
    }
    applied
}

/// Keep a record this device couldn't place as the cloud's own (it counts as this device's, so it isn't removed there).
fn hold(ic: &mut ICloud, name: &str, kind: &str, key: &str, data: Option<&str>) {
    let Some(data) = data else { return };
    let h = super::Held { kind: kind.to_string(), key: key.to_string(), data: data.to_string(), hash: super::hash_of(data) };
    if ic.held.insert(name.to_string(), h).is_none() {
        ic.held_dirty = true;
    }
}

fn release(ic: &mut ICloud, name: &str) {
    if ic.held.remove(name).is_some() {
        ic.held_dirty = true;
    }
}

fn keys_of(v: &Value, field: &str) -> Vec<String> {
    v.get(field)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).take(MEMBERS_MAX).map(str::to_string).collect())
        .unwrap_or_default()
}

fn apply_album(
    s: &mut Session,
    ic: &mut ICloud,
    all: &BTreeMap<String, Value>,
    uid: &str,
    have: &HashMap<String, PhotoId>,
    done: &mut HashSet<String>,
    depth: usize,
) -> usize {
    let Some(v) = all.get(uid) else { return 0 };
    if depth > DEPTH_MAX || !done.insert(uid.to_string()) {
        return 0;
    }
    let mut applied = 0;
    // (the folder it is in comes first; one this device doesn't have leaves it at the top)
    let parent_uid = v.get("parent").and_then(Value::as_str).map(str::to_string);
    if let Some(p) = &parent_uid {
        applied += apply_album(s, ic, all, p, have, done, depth + 1);
    }
    let parent = parent_uid.as_deref().and_then(|p| ic.local_of('a', p)).map(AlbumId).filter(|id| s.catalog.album(*id).is_some_and(|a| a.folder));
    let name = v.get("name").and_then(Value::as_str).map(str::trim).filter(|n| !n.is_empty()).unwrap_or("Untitled").to_string();
    let order = v.get("order").and_then(Value::as_u64).and_then(|o| u32::try_from(o).ok());
    let smart = v.get("smart").filter(|f| !f.is_null()).and_then(|f| serde_json::from_value::<lightcraft_catalog::Filter>(f.clone()).ok());
    let folder = v.get("folder").and_then(Value::as_bool).unwrap_or(false);
    let (mut members, mut waiting) = (Vec::new(), Vec::new());
    for k in keys_of(v, "photos") {
        match have.get(&k) {
            Some(id) => members.push(*id),
            None => waiting.push(k),
        }
    }
    let cover = v.get("cover").and_then(Value::as_str).and_then(|k| have.get(k).copied());
    match ic.local_of('a', uid).map(AlbumId).and_then(|id| s.catalog.album(id).cloned()) {
        None => {
            let id = s.catalog.alloc_album_id();
            let mut a = Album::new(id, name);
            (a.parent, a.folder, a.order, a.smart, a.cover) = (parent, folder, order, smart.map(Box::new), cover);
            if !folder && a.smart.is_none() {
                a.photos = members;
            }
            match s.apply_system(Op::AddAlbum { album: a }) {
                Ok(()) => {
                    ic.saved.uids.insert(format!("a{}", id.0), uid.to_string());
                    applied += 1;
                }
                Err(e) => {
                    log::warn!("iCloud: album: {e}");
                    return applied;
                }
            }
        }
        Some(a) => {
            // (members this device has no key for stay: the record can't say them)
            let mut photos = members;
            photos.extend(a.photos.iter().copied().filter(|p| key_of(s, *p).is_none()));
            let mut ops = Vec::new();
            if a.name != name {
                ops.push(Op::RenameAlbum { id: a.id, name });
            }
            if a.parent != parent {
                ops.push(Op::MoveAlbum { id: a.id, parent });
            }
            if a.order != order {
                ops.push(Op::SetAlbumOrder { id: a.id, order });
            }
            if let Some(rules) = smart.filter(|f| a.smart.as_deref() != Some(f)) {
                ops.push(Op::SetAlbumRules { id: a.id, rules: Box::new(rules) });
            }
            let same: HashSet<PhotoId> = a.photos.iter().copied().collect();
            if !a.folder && !a.is_smart() && (photos.len() != a.photos.len() || photos.iter().any(|p| !same.contains(p))) {
                ops.push(Op::SetAlbumPhotos { id: a.id, photos });
            }
            if a.cover != cover && !a.is_smart() {
                ops.push(Op::SetAlbumCover { id: a.id, cover });
            }
            for op in ops {
                match s.apply_system(op) {
                    Ok(()) => applied += 1,
                    Err(e) => log::warn!("iCloud: album {}: {e}", a.id.0),
                }
            }
        }
    }
    if waiting.is_empty() {
        ic.saved.pending.remove(uid);
    } else {
        ic.saved.pending.insert(uid.to_string(), waiting);
    }
    ic.saved_dirty = true;
    applied
}

fn remove_album(s: &mut Session, ic: &mut ICloud, uid: &str) -> usize {
    ic.saved.pending.remove(uid);
    let Some(id) = ic.local_of('a', uid).map(AlbumId).filter(|id| s.catalog.album(*id).is_some()) else { return 0 };
    // (what was in a folder goes up a level, as removing the folder here does)
    let kids: Vec<AlbumId> = s.catalog.albums().filter(|a| a.parent == Some(id)).map(|a| a.id).collect();
    let up = s.catalog.album(id).and_then(|a| a.parent);
    for k in kids {
        if let Err(e) = s.apply_system(Op::MoveAlbum { id: k, parent: up }) {
            log::warn!("iCloud: album {}: {e}", k.0);
        }
    }
    ic.saved.uids.remove(&format!("a{}", id.0));
    ic.saved_dirty = true;
    match s.apply_system(Op::RemoveAlbum { id }) {
        Ok(()) => 1,
        Err(e) => {
            log::warn!("iCloud: album {}: {e}", id.0);
            0
        }
    }
}

fn apply_stack(s: &mut Session, ic: &mut ICloud, uid: &str, v: &Value, have: &HashMap<String, PhotoId>) -> usize {
    let photos: Vec<PhotoId> = keys_of(v, "photos").iter().filter_map(|k| have.get(k).copied()).collect();
    let collapsed = v.get("collapsed").and_then(Value::as_bool).unwrap_or(false);
    let existing = ic.local_of('s', uid).map(StackId).and_then(|id| s.catalog.stack(id).cloned());
    if photos.len() < 2 {
        return 0;
    }
    let op = match existing {
        Some(st) if st.photos == photos && st.collapsed == collapsed => return 0,
        Some(st) => Op::SetStack { id: st.id, photos, collapsed },
        None => {
            // (a photo is in one stack: these are taken out of the others first)
            let others = s.catalog.remove_from_stacks_ops(&photos);
            for op in others {
                if let Err(e) = s.apply_system(op) {
                    log::warn!("iCloud: stack: {e}");
                    return 0;
                }
            }
            let id = s.catalog.alloc_stack_id();
            ic.saved.uids.insert(format!("s{}", id.0), uid.to_string());
            ic.saved_dirty = true;
            Op::AddStack { stack: Stack { id, photos, collapsed } }
        }
    };
    match s.apply_system(op) {
        Ok(()) => 1,
        Err(e) => {
            log::warn!("iCloud: stack: {e}");
            0
        }
    }
}

fn remove_stack(s: &mut Session, ic: &mut ICloud, uid: &str) -> usize {
    let Some(id) = ic.local_of('s', uid).map(StackId).filter(|id| s.catalog.stack(*id).is_some()) else { return 0 };
    ic.saved.uids.remove(&format!("s{}", id.0));
    ic.saved_dirty = true;
    match s.apply_system(Op::RemoveStack { id }) {
        Ok(()) => 1,
        Err(e) => {
            log::warn!("iCloud: stack {}: {e}", id.0);
            0
        }
    }
}
