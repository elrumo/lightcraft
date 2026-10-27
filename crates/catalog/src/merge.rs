//! Merging a library that was never synced into the one a server already has (the second device of
//! someone who has been using LightCraft on both). Photos are matched by what they are — the
//! content hash of their file — never by id (each library numbered its own); everything else of
//! this library comes across under fresh ids in the signing-in device's id space, as the same
//! changes a person would have made on the server's library: the result is a catalog and the
//! [`Outbox`] that takes the server there, so a merge syncs like any edit and nothing needs a
//! second code path.
//!
//! What the merge does, so it can be told to someone before they agree to it:
//! - a photo on both sides stays the server's; what this library gave it that the server's
//!   doesn't have (rating, flag, label, settings, metadata, versions) is added, and where both
//!   changed the same value differently the server's stays (counted in [`MergeReport::conflicts`]);
//! - a photo only here is added, with its albums and stacks; virtual copies come along;
//! - an album with the same name in the same place on both sides is one album with the photos of
//!   both; the others are added; smart albums are added with their rules' photos and albums
//!   renumbered;
//! - a stack is added when none of its photos is in a stack on the server;
//! - nothing of the server's is removed or replaced wholesale; merging again changes nothing.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::rules::{Rule, RuleSet};
use crate::sync::{Outbox, content_key, merge3, portable};
use crate::{Album, AlbumId, Catalog, Filter, Op, Photo, PhotoId, Source, Stack};

/// What a merge found and did (or would do).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeReport {
    /// Photos of this library in the server's too.
    pub matched: usize,
    /// Photos only here, now added.
    pub added: usize,
    /// Of those, photos without a content hash: the record comes across, their file can't be uploaded.
    pub unreadable: usize,
    /// Values changed on both sides to different things (the server's stayed), at least.
    pub conflicts: usize,
    pub albums_matched: usize,
    pub albums_added: usize,
    pub stacks_added: usize,
}

/// The merged library and how to get the server there.
pub struct Merged {
    /// The server's catalog with this library's changes applied (no device-local state yet: see [`carry_local_mapped`]).
    pub catalog: Catalog,
    /// The changes, to push on top of the server's state.
    pub outbox: Outbox,
    /// This library's photo ids → the ids they have in the merged one.
    pub photos: HashMap<PhotoId, PhotoId>,
    pub albums: HashMap<AlbumId, AlbumId>,
    pub report: MergeReport,
}

/// Apply `op` to the merged catalog and note it for the server.
fn apply(m: &mut Catalog, out: &mut Outbox, op: Op) {
    if let Ok(inverse) = m.apply(op.clone()) {
        out.record(&op, &inverse, m);
    }
}

/// The value of one setting after merging: the server's, plus what this library changed from `base` (an
/// untouched photo's) that the server didn't.
fn pick<T: Serialize + DeserializeOwned + PartialEq + Clone>(base: &T, server: &T, local: &T, conflicts: &mut usize) -> T {
    if local == base || local == server {
        return server.clone();
    }
    if server == base {
        return local.clone();
    }
    let v = |x: &T| serde_json::to_value(x).unwrap_or(Value::Null);
    // both changed it, differently: the server's changes win where they meet
    let merged: T = serde_json::from_value(merge3(&v(base), &v(server), &v(local))).unwrap_or_else(|_| server.clone());
    if merged == *server {
        *conflicts += 1;
    }
    merged
}

/// Give the server's photo `sid` what `local` has and it hasn't.
fn merge_photo(m: &mut Catalog, out: &mut Outbox, sid: PhotoId, local: &Photo, report: &mut MergeReport) {
    let Some(server) = m.photo(sid).cloned() else { return };
    // an untouched photo: what "changed" is measured from
    let base = Photo::new(sid, Source::Demo { scene: 0 }, "", "", 0, 0, "");
    let c = &mut report.conflicts;
    let rating = pick(&base.rating, &server.rating, &local.rating, c);
    if rating != server.rating {
        apply(m, out, Op::SetRating { id: sid, rating });
    }
    let flag = pick(&base.flag, &server.flag, &local.flag, c);
    if flag != server.flag {
        apply(m, out, Op::SetFlag { id: sid, flag });
    }
    let label = pick(&base.label, &server.label, &local.label, c);
    if label != server.label {
        apply(m, out, Op::SetLabel { id: sid, label });
    }
    let develop = pick(&*base.develop, &*server.develop, &*local.develop, c);
    if develop != *server.develop {
        let edited = server.edited.clone().or_else(|| local.edited.clone());
        apply(m, out, Op::SetDevelop { id: sid, settings: Arc::new(develop), label: "Merged libraries".into(), edited });
    }
    let meta = pick(&base.meta, &server.meta, &local.meta, c);
    if meta != server.meta {
        apply(m, out, Op::SetMeta { id: sid, meta: Box::new(meta) });
    }
    // versions: the server's, then the ones only this library has
    let mut versions = server.versions.clone();
    for v in &local.versions {
        if !versions.iter().any(|x| x.name == v.name && x.settings == v.settings) {
            versions.push(v.clone());
        }
    }
    if versions != server.versions {
        apply(m, out, Op::SetVersions { id: sid, versions });
    }
    if server.captured.is_none() && local.captured.is_some() {
        apply(m, out, Op::SetCaptured { id: sid, captured: local.captured.clone() });
    }
}

/// `f` with the photos and albums it names renumbered (those not in the maps are dropped).
fn remap_filter(f: &Filter, photos: &HashMap<PhotoId, PhotoId>, albums: &HashMap<AlbumId, AlbumId>) -> Filter {
    fn rules(set: &RuleSet, albums: &HashMap<AlbumId, AlbumId>) -> RuleSet {
        let mapped = set
            .rules
            .iter()
            .map(|r| match r {
                Rule::Group { group } => Rule::Group { group: rules(group, albums) },
                Rule::Field { field, op, value } if field == "album" => {
                    let to = value.as_u64().and_then(|id| albums.get(&AlbumId(id)));
                    Rule::Field { field: field.clone(), op: op.clone(), value: to.map_or(Value::Null, |a| Value::from(a.0)) }
                }
                other => other.clone(),
            })
            .collect();
        RuleSet { mode: set.mode, rules: mapped }
    }
    let mut out = f.clone();
    out.only = f.only.iter().filter_map(|id| photos.get(id).copied()).collect();
    out.album = f.album.and_then(|a| albums.get(&a).copied());
    out.rule_set = f.rule_set.as_ref().map(|s| rules(s, albums));
    out
}

/// Merge `local` into `server`, which this device then continues from as `space` (the id space the server gave it).
pub fn merge(local: &Catalog, server: Catalog, space: u32) -> Merged {
    let mut m = server;
    m.set_id_space(space);
    let mut out = Outbox::default();
    let mut report = MergeReport::default();
    let mut photos: HashMap<PhotoId, PhotoId> = HashMap::new();

    // the server's photos by what they are (a repeated file: in id order; virtual copies are never matched)
    let mut by_hash: HashMap<String, VecDeque<PhotoId>> = HashMap::new();
    for p in m.photos().filter(|p| !p.local && p.copy_of.is_none()) {
        if let Some(k) = content_key(p) {
            by_hash.entry(k).or_default().push_back(p.id);
        }
    }
    // this library's photos, originals before their copies
    let mut mine: Vec<&Arc<Photo>> = local.photos().filter(|p| !p.local && !matches!(p.source, Source::Demo { .. })).collect();
    mine.sort_by_key(|p| (p.copy_of.is_some(), p.id));
    for p in mine {
        let found = if p.copy_of.is_none() { content_key(p).and_then(|k| by_hash.get_mut(&k)?.pop_front()) } else { None };
        if let Some(sid) = found {
            photos.insert(p.id, sid);
            report.matched += 1;
            merge_photo(&mut m, &mut out, sid, p, &mut report);
            continue;
        }
        let id = m.alloc_photo_id();
        let mut np = portable(p);
        np.id = id;
        // (a virtual copy's hash names the copy: the id in it is this copy's new one)
        if let Some((base, _)) = np.content_hash.as_deref().and_then(|h| h.split_once(":dup")) {
            np.content_hash = Some(format!("{base}:dup{}", id.0));
        }
        np.copy_of = p.copy_of.and_then(|o| photos.get(&o).copied());
        if content_key(p).is_none() {
            report.unreadable += 1;
        }
        apply(&mut m, &mut out, Op::AddPhoto { photo: Box::new(np) });
        photos.insert(p.id, id);
        report.added += 1;
    }

    // albums: folders and plain albums by depth (parents first), smart albums last (their rules name albums)
    let depth = |a: &Album| {
        let mut d = 0;
        let mut cur = a.parent;
        while let Some(id) = cur.filter(|_| d < 32) {
            d += 1;
            cur = local.album(id).and_then(|a| a.parent);
        }
        d
    };
    let mut albums_here: Vec<&Album> = local.albums().collect();
    albums_here.sort_by_key(|a| (a.is_smart(), depth(a), a.id));
    let mut albums: HashMap<AlbumId, AlbumId> = HashMap::new();
    for a in albums_here {
        let parent = a.parent.and_then(|p| albums.get(&p).copied());
        let same = |x: &&Album| {
            x.quick == a.quick && x.folder == a.folder && x.is_smart() == a.is_smart() && x.parent == parent && (a.quick || x.name == a.name)
        };
        let found = m.albums().find(same).map(|x| x.id);
        if let Some(sid) = found {
            albums.insert(a.id, sid);
            report.albums_matched += 1;
            if !a.folder && !a.is_smart() {
                let mut members = m.album(sid).map(|x| x.photos.clone()).unwrap_or_default();
                let before = members.len();
                for id in a.photos.iter().filter_map(|p| photos.get(p)) {
                    if !members.contains(id) {
                        members.push(*id);
                    }
                }
                if members.len() != before {
                    apply(&mut m, &mut out, Op::SetAlbumPhotos { id: sid, photos: members });
                }
            }
            continue;
        }
        let id = m.alloc_album_id();
        let mut na = a.clone();
        na.id = id;
        na.parent = parent;
        na.photos = a.photos.iter().filter_map(|p| photos.get(p).copied()).collect();
        na.cover = a.cover.and_then(|c| photos.get(&c).copied());
        na.smart = a.smart.as_ref().map(|f| Box::new(remap_filter(f, &photos, &albums)));
        apply(&mut m, &mut out, Op::AddAlbum { album: na });
        albums.insert(a.id, id);
        report.albums_added += 1;
    }

    // stacks whose photos aren't in a stack on the server yet
    for st in local.stacks.values() {
        let members: Vec<PhotoId> = st.photos.iter().filter_map(|p| photos.get(p).copied()).collect();
        if members.len() < 2 || members.iter().any(|id| m.stacks.values().any(|s| s.photos.contains(id))) {
            continue;
        }
        let id = m.alloc_stack_id();
        apply(&mut m, &mut out, Op::AddStack { stack: Stack { id, photos: members, collapsed: st.collapsed } });
        report.stacks_added += 1;
    }
    // label names the server doesn't have
    for (label, name) in &local.label_names {
        if !m.label_names.contains_key(label) {
            apply(&mut m, &mut out, Op::SetLabelName { label: *label, name: Some(name.clone()) });
        }
    }
    Merged { catalog: m, outbox: out, photos, albums, report }
}

/// Bring this device's local state from `from` (the library before the merge) into the merged `to`: where
/// each photo's file is on this disk, its name, History, and the Local records and browsed folders — the
/// records under new ids, since the ids they had belong to this library's numbering. Returns the new ids of the
/// Local records.
pub fn carry_local_mapped(from: &Catalog, to: &mut Catalog, photos: &HashMap<PhotoId, PhotoId>) -> HashMap<PhotoId, PhotoId> {
    for (old, new) in photos {
        if let (Some(old), Some(p)) = (from.photo(*old), to.photos.get_mut(new)) {
            let p = Arc::make_mut(p);
            p.source = old.source.clone();
            p.file_name = old.file_name.clone();
            p.history = old.history.clone();
            p.local_baseline = old.local_baseline;
        }
    }
    let mut locals = HashMap::new();
    for old in from.photos().filter(|p| p.local) {
        let id = to.alloc_photo_id();
        let mut p = (**old).clone();
        p.id = id;
        to.photos.insert(id, Arc::new(p));
        locals.insert(old.id, id);
    }
    to.browsed = from.browsed.clone();
    locals
}
