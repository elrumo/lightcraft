//! `library.storage`: what the library's originals take, by kind, and the biggest photos
//! (the Free Up Space screen's list). Read only: it never re-sorts the person's view.

use std::collections::HashMap;

use lightcraft_catalog::{Catalog, Flag, MediaKind, Op, OriginKind, Photo, PhotoId, Source};
use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd, query::photo_summary};
use crate::{Result, Session};

/// The most photos one answer lists (the call can ask for fewer).
const MOST: usize = 500;
const DEFAULT: usize = 100;

fn kind_key(k: MediaKind) -> &'static str {
    match k {
        MediaKind::Image => "image",
        MediaKind::Raw => "raw",
        MediaKind::Video => "video",
    }
}

/// A photo whose bytes count: in the library, and not a virtual copy (it shares its original's file).
fn counts(p: &Photo) -> bool {
    p.in_library() && p.copy_of.is_none()
}

/// `{total: {count, bytes}, byKind: {image, raw, video: {count, bytes}}, largest: [summary + inPhotos]}`,
/// the `limit` biggest first after the first `offset` (ties by id, so the list doesn't shuffle between calls).
pub fn storage(cat: &Catalog, limit: usize, offset: usize, kind: Option<&str>) -> Value {
    let mut totals = [(0u64, 0u64); 3];
    let mut picked: Vec<&Photo> = Vec::new();
    for p in cat.photos().filter(|p| counts(p)) {
        let i = match p.kind {
            MediaKind::Image => 0,
            MediaKind::Raw => 1,
            MediaKind::Video => 2,
        };
        totals[i].0 = totals[i].0.saturating_add(1);
        totals[i].1 = totals[i].1.saturating_add(p.file_size);
        if kind.is_none_or(|k| k == kind_key(p.kind)) {
            picked.push(p);
        }
    }
    picked.sort_by(|a, b| b.file_size.cmp(&a.file_size).then(a.id.0.cmp(&b.id.0)));
    let picked: Vec<&Photo> = picked.into_iter().skip(offset).take(limit.min(MOST)).collect();
    let largest: Vec<Value> = picked
        .into_iter()
        .map(|p| {
            let mut v = photo_summary(p);
            // an original that stays in Apple Photos: this file is a copy, and replacing it is Apple Photos' business
            v["inPhotos"] = json!(p.origin.as_ref().is_some_and(|o| o.kind == OriginKind::Photos));
            // where the original is (a file on this device), for a frontend that replaces it with a smaller one
            v["path"] = match &p.source {
                Source::File { path } if !path.starts_with(lightcraft_catalog::sync::PATH_PREFIX) => json!(path),
                _ => Value::Null,
            };
            v
        })
        .collect();
    let entry = |i: usize| json!({"count": totals[i].0, "bytes": totals[i].1});
    json!({
        "total": {"count": totals.iter().map(|t| t.0).sum::<u64>(), "bytes": totals.iter().fold(0u64, |a, t| a.saturating_add(t.1))},
        "byKind": {"image": entry(0), "raw": entry(1), "video": entry(2)},
        "largest": largest,
    })
}

/// The most groups one answer lists.
const MOST_GROUPS: usize = 1000;
const DEFAULT_GROUPS: usize = 200;

/// The content hash of a photo that can be an exact copy of another: a library photo with a file of its own that was
/// not made on purpose (Photo > Duplicate gives a `:dupN` hash) and does not stay in Apple Photos.
fn candidate(p: &Photo) -> Option<&str> {
    if !counts(p) || !matches!(p.source, Source::File { .. }) {
        return None;
    }
    if p.origin.as_ref().is_some_and(|o| o.kind == OriginKind::Photos) {
        return None;
    }
    p.content_hash.as_deref().filter(|h| !h.is_empty() && !h.contains(':'))
}

/// Which of a group to keep: edited first, then the better rating, a pick, and the older photo.
fn keeper(group: &[&Photo]) -> PhotoId {
    group.iter().max_by_key(|p| (p.is_edited(), p.rating, p.flag == Flag::Pick, std::cmp::Reverse(p.id.0))).map_or(PhotoId(0), |p| p.id)
}

/// `{groups: [{hash, keep, bytes, reclaimable, photos}], groupCount, extraCopies, reclaimable}`: library photos whose
/// files have the same content hash, the groups that would free the most first. `bytes` is what each copy takes and
/// `reclaimable` counts every copy but the one to keep. Photos hashed by different algorithms (a library from before the
/// content hash changed) are not matched: a file's hash is whatever the library stored for it.
pub fn duplicates(cat: &Catalog, limit: usize) -> Value {
    let mut by_hash: HashMap<&str, Vec<&Photo>> = HashMap::new();
    for p in cat.photos() {
        if let Some(h) = candidate(p) {
            by_hash.entry(h).or_default().push(p);
        }
    }
    // (reclaimable bytes, copies, group)
    let mut groups: Vec<(u64, u64, Value)> = by_hash
        .into_iter()
        .filter(|(_, g)| g.len() > 1)
        .map(|(hash, mut g)| {
            g.sort_by_key(|p| p.id.0);
            let keep = keeper(&g);
            let each = g.iter().map(|p| p.file_size).max().unwrap_or(0);
            let reclaim = g.iter().filter(|p| p.id != keep).fold(0u64, |a, p| a.saturating_add(p.file_size));
            let photos: Vec<Value> = g.iter().map(|p| photo_summary(p)).collect();
            (reclaim, g.len() as u64 - 1, json!({"hash": hash, "keep": keep.0, "bytes": each, "reclaimable": reclaim, "photos": photos}))
        })
        .collect();
    groups.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.2["keep"].as_u64().cmp(&b.2["keep"].as_u64())));
    let group_count = groups.len();
    let extra: u64 = groups.iter().map(|g| g.1).sum();
    let reclaimable = groups.iter().fold(0u64, |a, g| a.saturating_add(g.0));
    groups.truncate(limit.min(MOST_GROUPS));
    json!({
        "groups": groups.into_iter().map(|g| g.2).collect::<Vec<_>>(),
        "groupCount": group_count,
        "extraCopies": extra,
        "reclaimable": reclaimable,
    })
}

/// `library.mergeDuplicates`: the copies `ids` go to Recently Deleted and the photo `id` takes their place: it joins the
/// albums they were in and keeps the best rating among them. One undo step. Every copy must have the same stored content
/// hash as the kept photo (a call that names an unrelated photo is refused and nothing changes). The files stay where
/// they are, as with any deletion from the library.
fn merge(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "library.mergeDuplicates";
    let keep = super::photo_id(p, C)?.ok_or_else(|| bad(C, "missing `id`, the photo to keep"))?;
    let mut copies = super::photo_ids(p, C)?.ok_or_else(|| bad(C, "missing `ids`, the copies to remove"))?;
    copies.sort_by_key(|c| c.0);
    copies.dedup();
    if copies.is_empty() {
        return Err(bad(C, "`ids` is empty"));
    }
    if copies.contains(&keep) {
        return Err(bad(C, "the photo to keep is also in `ids`"));
    }
    let kept = s.catalog.photo(keep).ok_or_else(|| bad(C, "no such photo to keep"))?;
    let hash = candidate(kept).ok_or_else(|| bad(C, "the photo to keep has no content hash of its own"))?.to_string();
    let kept_rating = kept.rating;
    let mut rating = kept_rating;
    for c in &copies {
        let copy = s.catalog.photo(*c).ok_or_else(|| bad(C, format!("no photo {}", c.0)))?;
        if candidate(copy) != Some(hash.as_str()) {
            return Err(bad(C, format!("photo {} is not an exact copy of photo {}", c.0, keep.0)));
        }
        rating = rating.max(copy.rating);
    }
    let mut ops: Vec<Op> = Vec::new();
    if rating != kept_rating {
        ops.push(Op::SetRating { id: keep, rating });
    }
    // the kept photo joins the regular albums its copies were in
    let mut joined = 0usize;
    let mut albums: Vec<_> = copies.iter().flat_map(|c| s.catalog.albums_of(*c)).collect();
    albums.sort_by_key(|a| a.0);
    albums.dedup();
    for id in albums {
        let Some(al) = s.catalog.album(id) else { continue };
        if al.is_smart() || al.folder || al.photos.contains(&keep) {
            continue;
        }
        let mut photos = al.photos.clone();
        photos.push(keep);
        let cover = al.cover.or(photos.first().copied());
        ops.push(Op::SetAlbumPhotos { id, photos });
        ops.push(Op::SetAlbumCover { id, cover });
        joined += 1;
    }
    ops.extend(copies.iter().map(|c| Op::SetDeleted { id: *c, deleted: true }));
    ops.extend(s.catalog.remove_from_stacks_ops(&copies));
    s.commit("Merge Duplicates", Op::Batch { ops })?;
    Ok(json!({"removed": copies.len(), "albumsJoined": joined, "rating": rating}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "library.storage", "Library Storage", [], None,
        "{limit?: 100 (at most 500), offset?: 0, kind?: image|raw|video} → {total, byKind: {image, raw, video: {count, bytes}}, largest: [photo summary + inPhotos + path]} — what the library's originals take and its biggest photos (virtual copies and Recently Deleted are not counted)",
        always, |s: &mut Session, p: &Value| -> Result<Value> {
            let limit = p.get("limit").and_then(Value::as_u64).map_or(DEFAULT, |n| usize::try_from(n).unwrap_or(MOST));
            let offset = p.get("offset").and_then(Value::as_u64).map_or(0, |n| usize::try_from(n).unwrap_or(usize::MAX));
            Ok(storage(&s.catalog, limit, offset, super::str_param(p, "kind")))
        }),
        cmd!(query "library.findDuplicates", "Find Duplicates", [], None,
        "{limit?: 200 (at most 1000 groups)} → {groups: [{hash, keep, bytes, reclaimable, photos: [photo summary]}], groupCount, extraCopies, reclaimable} — library photos whose files have the same content hash (copies made with Duplicate, virtual copies, Recently Deleted and photos kept in Apple Photos are left out); `keep` is the photo to keep: edited first, then rating, pick, older",
        always, |s: &mut Session, p: &Value| -> Result<Value> {
            let limit = p.get("limit").and_then(Value::as_u64).map_or(DEFAULT_GROUPS, |n| usize::try_from(n).unwrap_or(MOST_GROUPS));
            Ok(duplicates(&s.catalog, limit))
        }),
        cmd!(
            "library.mergeDuplicates",
            "Merge Duplicates",
            [],
            None,
            "{id: photoToKeep, ids: [exact copies]} → {removed, albumsJoined, rating} — moves the copies to Recently Deleted (the files stay), adds the kept photo to their albums and gives it their best rating; every copy must have the kept photo's content hash",
            always,
            merge
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn add(s: &mut Session, id: u64, format: &str, kind: MediaKind, size: u64) {
        let mut p = Photo::new(PhotoId(id), Source::File { path: format!("/lc-storage/{id}") }, &format!("{id}.x"), format, 4, 3, "2026-11-26");
        p.kind = kind;
        p.file_size = size;
        s.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    }

    #[test]
    fn totals_by_kind_and_the_biggest_first() {
        let mut s = Session::new();
        add(&mut s, 1, "DNG", MediaKind::Raw, 75);
        add(&mut s, 2, "JPEG", MediaKind::Image, 5);
        add(&mut s, 3, "MOV", MediaKind::Video, 300);
        add(&mut s, 4, "DNG", MediaKind::Raw, 75);
        let r = storage(&s.catalog, 3, 0, None);
        assert_eq!(r["total"], json!({"count": 4, "bytes": 455}));
        assert_eq!(r["byKind"]["raw"], json!({"count": 2, "bytes": 150}));
        let ids: Vec<u64> = r["largest"].as_array().unwrap().iter().map(|v| v["id"].as_u64().unwrap()).collect();
        assert_eq!(ids, [3, 1, 4], "biggest first, a tie by id, cut at the limit");
        let r = storage(&s.catalog, 10, 0, Some("raw"));
        assert_eq!(r["largest"].as_array().unwrap().len(), 2, "one kind's list");
        assert_eq!(r["total"]["count"], json!(4), "the totals still cover every kind");
        assert_eq!(storage(&s.catalog, 1, 0, None)["largest"][0]["path"], json!("/lc-storage/3"), "the file a photo is at");
        let page = storage(&s.catalog, 2, 1, None);
        let ids: Vec<u64> = page["largest"].as_array().unwrap().iter().map(|v| v["id"].as_u64().unwrap()).collect();
        assert_eq!(ids, [1, 4], "the second page starts after the first biggest");
        assert_eq!(storage(&s.catalog, 10, 99, None)["largest"].as_array().unwrap().len(), 0, "past the end is empty");
    }

    #[test]
    fn copies_and_deleted_photos_do_not_count_and_a_huge_limit_is_cut() {
        let mut s = Session::new();
        add(&mut s, 1, "DNG", MediaKind::Raw, 10);
        add(&mut s, 2, "DNG", MediaKind::Raw, 10);
        let mut copy = (**s.catalog.photo(PhotoId(1)).unwrap()).clone();
        copy.id = PhotoId(3);
        copy.copy_of = Some(PhotoId(1));
        s.catalog.apply(Op::AddPhoto { photo: Box::new(copy) }).unwrap();
        s.catalog.apply(Op::SetDeleted { id: PhotoId(2), deleted: true }).unwrap();
        let r = storage(&s.catalog, usize::MAX, 0, None);
        assert_eq!(r["total"], json!({"count": 1, "bytes": 10}));
        let r = s.execute("library.storage", &json!({"limit": u64::MAX})).unwrap();
        assert_eq!(r["largest"].as_array().unwrap().len(), 1);
    }

    fn hashed(s: &mut Session, id: u64, hash: &str, rating: u8) {
        let mut p = Photo::new(PhotoId(id), Source::File { path: format!("/lc-storage/{id}") }, &format!("{id}.x"), "JPEG", 4, 3, "2026-11-26");
        p.file_size = 10 * id;
        p.content_hash = Some(hash.to_string());
        p.rating = rating;
        s.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    }

    #[test]
    fn exact_copies_are_grouped_and_the_better_rated_one_is_kept() {
        let mut s = Session::new();
        hashed(&mut s, 1, "aa", 0);
        hashed(&mut s, 2, "aa", 4);
        hashed(&mut s, 3, "aa", 4);
        hashed(&mut s, 4, "bb", 0);
        hashed(&mut s, 5, "bb", 0);
        hashed(&mut s, 6, "cc", 0);
        let r = duplicates(&s.catalog, 10);
        assert_eq!(r["groupCount"], json!(2));
        assert_eq!(r["extraCopies"], json!(3));
        let first = &r["groups"][0];
        assert_eq!(first["hash"], json!("bb"), "the group that frees the most comes first (50 against 40)");
        assert_eq!(first["keep"], json!(4), "nothing tells them apart, so the older photo");
        assert_eq!(first["reclaimable"], json!(50));
        let second = &r["groups"][1];
        assert_eq!((second["hash"].clone(), second["keep"].clone()), (json!("aa"), json!(2)), "the best rating, then the older photo");
        assert_eq!(second["reclaimable"], json!(10 + 30), "every copy but the kept one");
        assert_eq!(r["reclaimable"], json!(40 + 50), "both groups");
        assert_eq!(duplicates(&s.catalog, 1)["groups"].as_array().unwrap().len(), 1, "cut at the limit, the totals still count all");
    }

    #[test]
    fn a_copy_made_on_purpose_and_a_deleted_photo_are_not_duplicates() {
        let mut s = Session::new();
        hashed(&mut s, 1, "aa", 0);
        hashed(&mut s, 2, "aa:dup1", 0);
        hashed(&mut s, 3, "aa", 0);
        s.catalog.apply(Op::SetDeleted { id: PhotoId(3), deleted: true }).unwrap();
        assert_eq!(duplicates(&s.catalog, 10)["groupCount"], json!(0));
    }

    #[test]
    fn merging_deletes_the_copies_adds_the_kept_photo_to_their_albums_and_keeps_the_best_rating() {
        let mut s = Session::new();
        hashed(&mut s, 1, "aa", 0);
        hashed(&mut s, 2, "aa", 5);
        let album = s.execute("album.create", &json!({"name": "Trip"})).unwrap()["id"].as_u64().unwrap();
        s.execute("album.addPhotos", &json!({"id": album, "ids": [2]})).unwrap();
        let r = s.execute("library.mergeDuplicates", &json!({"id": 1, "ids": [2]})).unwrap();
        assert_eq!(r, json!({"removed": 1, "albumsJoined": 1, "rating": 5}));
        assert!(s.catalog.photo(PhotoId(2)).unwrap().deleted, "the copy is in Recently Deleted");
        assert_eq!(s.catalog.photo(PhotoId(1)).unwrap().rating, 5);
        assert!(s.catalog.album(lightcraft_catalog::AlbumId(album)).unwrap().photos.contains(&PhotoId(1)));
        // one undo step brings everything back
        s.execute("edit.undo", &json!({})).unwrap();
        assert!(!s.catalog.photo(PhotoId(2)).unwrap().deleted);
        assert_eq!(s.catalog.photo(PhotoId(1)).unwrap().rating, 0);
    }

    #[test]
    fn merging_refuses_a_photo_that_is_not_an_exact_copy() {
        let mut s = Session::new();
        hashed(&mut s, 1, "aa", 0);
        hashed(&mut s, 2, "bb", 0);
        for params in [
            json!({"id": 1, "ids": [2]}),
            json!({"id": 1, "ids": [1]}),
            json!({"id": 1, "ids": []}),
            json!({"id": 1}),
            json!({"ids": [2]}),
            json!({"id": 9, "ids": [2]}),
        ] {
            assert!(s.execute("library.mergeDuplicates", &params).is_err(), "{params}");
        }
        assert!(!s.catalog.photo(PhotoId(2)).unwrap().deleted, "nothing changed");
    }
}
