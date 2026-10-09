//! Joining this device's library to a server library that already has photos (`docs/sync.md` →
//! *When this library and the server's both have photos*).
//!
//! Two libraries that never synced with each other hand out the same ids to different photos, so
//! the photos, albums and stacks of one are *added* to the other under new ids
//! ([`join_libraries`]), as ordinary ops queued in the [`Outbox`]: pushed on top of the server's
//! library like any change, merged with whatever other devices do meanwhile, undone by nothing
//! and lost by nobody.
//!
//! - **A photo the server has too** (the same file content) is kept once, as the server has it.
//!   What this library knew about it and the server's copy lacks — a rating, a flag, a colour label,
//!   edits, keywords and other metadata, versions — is filled in; where both have a value the
//!   server's wins (other devices already show it).
//! - **Albums with the same name in the same place** (both top level, or in folders that were
//!   joined) are one album: this library's photos are added to the server's.
//! - **Stacks** carry over when all their photos did and none is stacked on the server already.
//! - **Local records** (folders browsed but not added to the library) and the times they were
//!   browsed stay on this device, under new ids.
//!
//! No I/O here: the engine fetches the server's library and pushes the queued changes.

use std::collections::HashMap;
use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;

use crate::sync::{Outbox, content_key, merge3};
use crate::{Album, AlbumId, Catalog, Filter, Flag, Meta, Op, Photo, PhotoId, Rule, RuleSet, Source, Stack, Version};

/// Deepest folder nesting followed (a folder inside a folder inside…).
const MAX_DEPTH: usize = 32;

/// What joining two libraries did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Joined {
    /// Photos added to the server's library (virtual copies included).
    pub photos_added: usize,
    /// Photos the server's library had already (same file content): kept once.
    pub photos_shared: usize,
    pub albums_added: usize,
    /// Albums that were already there under the same name: this library's photos were added to them.
    pub albums_merged: usize,
    pub stacks_added: usize,
}

/// A photo that belongs in a shared library: not a Local record (a folder browsed, not added) and
/// not one of the procedural demo photos a new library starts with.
pub fn is_shareable(p: &Photo) -> bool {
    !p.local && !matches!(p.source, Source::Demo { .. })
}

/// What a photo is, for telling whether two libraries have the same one: its content hash,
/// suffix included (a converted copy of a raw isn't the raw).
fn identity(p: &Photo) -> Option<String> {
    content_key(p)?;
    p.content_hash.as_deref().map(str::to_ascii_lowercase)
}

/// What a library holds, for the choice offered at sign-in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Counts {
    /// Photos, virtual copies included, without the demo photos and Local records.
    pub photos: usize,
    /// Albums (folders aren't counted).
    pub albums: usize,
    /// Photos with edits.
    pub edited: usize,
}

/// What `c` holds that [`join_libraries`] would carry over.
pub fn counts(c: &Catalog) -> Counts {
    // (what the grid shows: photos in Recently Deleted aren't counted; they still go along)
    let mine = || c.photos().filter(|p| is_shareable(p) && !p.deleted);
    Counts { photos: mine().count(), albums: c.albums().filter(|a| !a.folder).count(), edited: mine().filter(|p| p.is_edited()).count() }
}

/// How many of `local`'s photos `other` has too (the same file content): joining keeps them once.
pub fn shared_photos(local: &Catalog, other: &Catalog) -> usize {
    let theirs: std::collections::HashSet<String> =
        other.photos().filter(|p| !p.local && !p.deleted && p.copy_of.is_none()).filter_map(|p| identity(p)).collect();
    local
        .photos()
        .filter(|p| is_shareable(p) && !p.deleted && p.copy_of.is_none())
        .filter(|p| identity(p).is_some_and(|k| theirs.contains(&k)))
        .count()
}

/// Add `local`'s photos, albums, stacks and label names to `server` (the server's library as of its
/// newest op) under ids of this device's id space `space`. `server` becomes the joined library as
/// this device sees it (its own photos keep the files they have here); every change to push is
/// queued in `outbox`.
pub fn join_libraries(local: &Catalog, server: &mut Catalog, space: u32, outbox: &mut Outbox) -> Joined {
    server.set_id_space(space);
    let mut joined = Joined::default();
    let photos = join_photos(local, server, outbox, &mut joined);
    join_albums(local, server, outbox, &photos, &mut joined);
    join_stacks(local, server, outbox, &photos, &mut joined);
    let names: Vec<(crate::ColorLabel, String)> =
        local.label_names.iter().filter(|(l, _)| server.custom_label_name(**l).is_none()).map(|(l, n)| (*l, n.clone())).collect();
    for (label, name) in names {
        apply(server, outbox, Op::SetLabelName { label, name: Some(name) });
    }
    // device-local state: folders browsed and the records they made
    for p in local.photos().filter(|p| p.local) {
        let id = server.alloc_photo_id();
        let mut record = (**p).clone();
        record.id = id;
        record.copy_of = None;
        server.photos.insert(id, Arc::new(record));
    }
    server.browsed = local.browsed.clone();
    joined
}

/// Apply `op` to `server` and queue it. An op that doesn't apply is dropped (nothing changed).
fn apply(server: &mut Catalog, outbox: &mut Outbox, op: Op) -> bool {
    match server.apply(op.clone()) {
        Ok(inverse) => {
            outbox.record(&op, &inverse, server);
            true
        }
        Err(e) => {
            log::warn!("sync: joining libraries: {e}");
            false
        }
    }
}

/// Photos: each of this library's is added, or matched with the server's copy. Returns where each
/// of this library's photos went.
fn join_photos(local: &Catalog, server: &mut Catalog, outbox: &mut Outbox, joined: &mut Joined) -> HashMap<PhotoId, PhotoId> {
    // the server's photos by content, each claimed by at most one of ours (two copies of a file
    // in this library are two photos, as they are anywhere)
    let mut theirs: HashMap<String, Vec<PhotoId>> = HashMap::new();
    // (one the server's owner put in Recently Deleted isn't a match: the photo stays in this library)
    for p in server.photos().filter(|p| !p.local && !p.deleted && p.copy_of.is_none()) {
        if let Some(k) = identity(p) {
            theirs.entry(k).or_default().push(p.id);
        }
    }
    let mut mine: Vec<&Arc<Photo>> = local.photos().filter(|p| is_shareable(p)).collect();
    // virtual copies after the photos they were copied from
    mine.sort_by_key(|p| (p.copy_of.is_some(), p.id));
    let mut placed: HashMap<PhotoId, PhotoId> = HashMap::new();
    for p in mine {
        let shared = match (p.copy_of, identity(p)) {
            (None, Some(k)) => theirs.get_mut(&k).filter(|v| !v.is_empty()).map(|v| v.remove(0)),
            _ => None,
        };
        if let Some(sid) = shared {
            placed.insert(p.id, sid);
            fill_in(server, outbox, sid, p);
            joined.photos_shared += 1;
            continue;
        }
        let id = server.alloc_photo_id();
        let mut added = (**p).clone();
        added.id = id;
        // (a copy whose original wasn't carried over stands alone: it has the file's content)
        added.copy_of = p.copy_of.and_then(|m| placed.get(&m).copied());
        if apply(server, outbox, Op::AddPhoto { photo: Box::new(added) }) {
            placed.insert(p.id, id);
            joined.photos_added += 1;
        }
    }
    placed
}

/// What the server's copy of a photo lacks and this library knew: queued as changes. The photo
/// also points at the file this device has (device-local, like [`crate::sync::carry_local`]).
fn fill_in(server: &mut Catalog, outbox: &mut Outbox, sid: PhotoId, l: &Photo) {
    let Some(s) = server.photo(sid).cloned() else { return };
    let mut ops = Vec::new();
    if s.rating == 0 && l.rating > 0 {
        ops.push(Op::SetRating { id: sid, rating: l.rating });
    }
    if s.flag == Flag::None && l.flag != Flag::None {
        ops.push(Op::SetFlag { id: sid, flag: l.flag });
    }
    if s.label.is_none() && l.label.is_some() {
        ops.push(Op::SetLabel { id: sid, label: l.label });
    }
    if !s.is_edited() && l.is_edited() {
        ops.push(Op::SetDevelop { id: sid, settings: l.develop.clone(), label: "Joined library".into(), edited: l.edited.clone() });
    }
    let meta = fill_meta(&s.meta, &l.meta);
    if meta != s.meta {
        ops.push(Op::SetMeta { id: sid, meta: Box::new(meta) });
    }
    let extra: Vec<Version> = l.versions.iter().filter(|v| !s.versions.iter().any(|x| x.name == v.name)).cloned().collect();
    if !extra.is_empty() {
        let mut all = s.versions.clone();
        all.extend(extra);
        ops.push(Op::SetVersions { id: sid, versions: all });
    }
    if s.captured.is_none() && l.captured.is_some() {
        ops.push(Op::SetCaptured { id: sid, captured: l.captured.clone() });
    }
    for op in ops {
        apply(server, outbox, op);
    }
    // this device has the file: the photo shows from it here (never sent: a path on this disk)
    if let Some(p) = server.photos.get_mut(&sid) {
        let p = Arc::make_mut(p);
        p.source = l.source.clone();
        p.file_name = l.file_name.clone();
        p.history = l.history.clone();
        p.local_baseline = l.local_baseline;
    }
}

/// `server`'s metadata with what only `local` has filled in: a field the server has set stays,
/// keywords are the union of both.
fn fill_meta(server: &Meta, local: &Meta) -> Meta {
    let json = |m: &Meta| serde_json::to_value(m).unwrap_or(Value::Null);
    // what the server's copy changed from a blank one, applied over this library's
    let merged = merge3(&json(&Meta::default()), &json(server), &json(local));
    serde_json::from_value(merged).unwrap_or_else(|_| server.clone())
}

/// Folders and albums.
fn join_albums(local: &Catalog, server: &mut Catalog, outbox: &mut Outbox, photos: &HashMap<PhotoId, PhotoId>, joined: &mut Joined) {
    let depth = |a: &Album| {
        let (mut d, mut cur) = (0, a.parent);
        while let Some(id) = cur.filter(|_| d < MAX_DEPTH) {
            d += 1;
            cur = local.album(id).and_then(|p| p.parent);
        }
        d
    };
    // folders before what they hold, and smart albums last (their rules may name any other album)
    let mut mine: Vec<&Album> = local.albums().collect();
    mine.sort_by_key(|a| (a.smart.is_some(), depth(a), a.id));
    // the server's albums as they were: each claimed by at most one of ours
    let mut open: Vec<AlbumId> = server.albums().map(|a| a.id).collect();
    let mut placed: HashMap<AlbumId, AlbumId> = HashMap::new();
    for a in mine {
        let parent = a.parent.and_then(|p| placed.get(&p).copied());
        let smart = a.smart.as_ref().map(|f| Box::new(remap_filter(f, photos, &placed)));
        let target = if a.quick {
            server.quick_collection().filter(|id| open.contains(id))
        } else {
            open.iter().copied().find(|id| {
                server.album(*id).is_some_and(|s| !s.quick && s.parent == parent && s.folder == a.folder && s.name == a.name && s.smart == smart)
            })
        };
        let members: Vec<PhotoId> = if a.folder || smart.is_some() {
            Vec::new()
        } else {
            let mut seen = std::collections::HashSet::new();
            a.photos.iter().filter_map(|p| photos.get(p).copied()).filter(|p| seen.insert(*p)).collect()
        };
        match target {
            Some(id) => {
                open.retain(|x| *x != id);
                placed.insert(a.id, id);
                let have = server.album(id).map(|s| s.photos.clone()).unwrap_or_default();
                let new: Vec<PhotoId> = members.into_iter().filter(|p| !have.contains(p)).collect();
                if !new.is_empty() {
                    let mut all = have;
                    all.extend(new);
                    apply(server, outbox, Op::SetAlbumPhotos { id, photos: all });
                }
                joined.albums_merged += 1;
            }
            None => {
                let id = server.alloc_album_id();
                let album = Album {
                    id,
                    name: a.name.clone(),
                    parent,
                    folder: a.folder,
                    photos: members,
                    cover: a.cover.and_then(|c| photos.get(&c).copied()),
                    smart,
                    quick: a.quick,
                };
                if apply(server, outbox, Op::AddAlbum { album }) {
                    placed.insert(a.id, id);
                    joined.albums_added += 1;
                }
            }
        }
    }
}

/// A smart album's rules, pointing at the photos and albums of the joined library.
fn remap_filter(f: &Filter, photos: &HashMap<PhotoId, PhotoId>, albums: &HashMap<AlbumId, AlbumId>) -> Filter {
    let mut f = f.clone();
    if !f.only.is_empty() {
        f.only = f.only.iter().filter_map(|p| photos.get(p).copied()).collect();
        // (no photo of the list came along: still "no photo", not "every photo")
        if f.only.is_empty() {
            f.only.push(PhotoId(0));
        }
    }
    // (an album that didn't come along is none: no album has id 0)
    f.album = f.album.map(|a| albums.get(&a).copied().unwrap_or(AlbumId(0)));
    if let Some(rules) = f.rule_set.as_mut() {
        remap_rules(rules, albums, 0);
    }
    f
}

fn remap_rules(set: &mut RuleSet, albums: &HashMap<AlbumId, AlbumId>, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    for r in &mut set.rules {
        match r {
            Rule::Group { group } => remap_rules(group, albums, depth + 1),
            Rule::Field { field, value, .. } if field == "album" => {
                let to = value.as_u64().map(AlbumId).and_then(|a| albums.get(&a).copied()).map_or(0, |a| a.0);
                *value = Value::from(to);
            }
            Rule::Field { .. } => {}
        }
    }
}

fn join_stacks(local: &Catalog, server: &mut Catalog, outbox: &mut Outbox, photos: &HashMap<PhotoId, PhotoId>, joined: &mut Joined) {
    for st in local.stacks() {
        let members: Vec<PhotoId> = st.photos.iter().filter_map(|p| photos.get(p).copied()).collect();
        if members.len() != st.photos.len() {
            continue;
        }
        let id = server.alloc_stack_id();
        // (refused when a photo is stacked already: the server's stack stays)
        if apply(server, outbox, Op::AddStack { stack: Stack { id, photos: members, collapsed: st.collapsed } }) {
            joined.stacks_added += 1;
        }
    }
}
