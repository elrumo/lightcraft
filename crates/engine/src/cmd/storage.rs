//! `library.storage`: what the library's originals take, by kind, and the biggest photos
//! (the Free Up Space screen's list). Read only: it never re-sorts the person's view.

use lightcraft_catalog::{Catalog, MediaKind, OriginKind, Photo};
use serde_json::{Value, json};

use super::{CommandSpec, always, cmd, query::photo_summary};
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
/// the `limit` biggest first (ties by id, so the list doesn't shuffle between calls).
pub fn storage(cat: &Catalog, limit: usize, kind: Option<&str>) -> Value {
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
    picked.truncate(limit.min(MOST));
    let largest: Vec<Value> = picked
        .into_iter()
        .map(|p| {
            let mut v = photo_summary(p);
            // an original that stays in Apple Photos: this file is a copy, and replacing it is Apple Photos' business
            v["inPhotos"] = json!(p.origin.as_ref().is_some_and(|o| o.kind == OriginKind::Photos));
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

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(query "library.storage", "Library Storage", [], None,
    "{limit?: 100 (at most 500), kind?: image|raw|video} → {total, byKind: {image, raw, video: {count, bytes}}, largest: [photo summary + inPhotos]} — what the library's originals take and its biggest photos (virtual copies and Recently Deleted are not counted)",
    always, |s: &mut Session, p: &Value| -> Result<Value> {
        let limit = p.get("limit").and_then(Value::as_u64).map_or(DEFAULT, |n| usize::try_from(n).unwrap_or(MOST));
        Ok(storage(&s.catalog, limit, super::str_param(p, "kind")))
    })]
}

#[cfg(test)]
mod tests {
    use lightcraft_catalog::{Op, PhotoId, Source};

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
        let r = storage(&s.catalog, 3, None);
        assert_eq!(r["total"], json!({"count": 4, "bytes": 455}));
        assert_eq!(r["byKind"]["raw"], json!({"count": 2, "bytes": 150}));
        let ids: Vec<u64> = r["largest"].as_array().unwrap().iter().map(|v| v["id"].as_u64().unwrap()).collect();
        assert_eq!(ids, [3, 1, 4], "biggest first, a tie by id, cut at the limit");
        let r = storage(&s.catalog, 10, Some("raw"));
        assert_eq!(r["largest"].as_array().unwrap().len(), 2, "one kind's list");
        assert_eq!(r["total"]["count"], json!(4), "the totals still cover every kind");
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
        let r = storage(&s.catalog, usize::MAX, None);
        assert_eq!(r["total"], json!({"count": 1, "bytes": 10}));
        let r = s.execute("library.storage", &json!({"limit": u64::MAX})).unwrap();
        assert_eq!(r["largest"].as_array().unwrap().len(), 1);
    }
}
