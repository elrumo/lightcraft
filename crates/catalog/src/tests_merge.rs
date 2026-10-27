use std::sync::Arc;

use lightcraft_develop::DevelopSettings;

use super::merge::{carry_local_mapped, merge};
use super::sync::Outbox;
use super::*;

const H1: &str = "11111111111111111111111111111111";
const H2: &str = "22222222222222222222222222222222";
const H3: &str = "33333333333333333333333333333333";

/// A library of its own: ids from 1, as every library that never synced numbers its photos.
fn photo(c: &mut Catalog, hash: &str, name: &str) -> PhotoId {
    let id = c.alloc_photo_id();
    let mut p = Photo::new(id, Source::File { path: format!("/disk/{name}") }, name, "JPEG", 60, 40, "2026-01-01T00:00:00");
    p.content_hash = Some(hash.into());
    c.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    id
}

fn album(c: &mut Catalog, name: &str, photos: &[PhotoId]) -> AlbumId {
    let id = c.alloc_album_id();
    let mut a = Album::new(id, name);
    a.photos = photos.to_vec();
    c.apply(Op::AddAlbum { album: a }).unwrap();
    id
}

fn exposure(c: &Catalog, id: PhotoId) -> f64 {
    c.photo(id).unwrap().develop.light.exposure
}

fn with_develop(c: &mut Catalog, id: PhotoId, f: impl FnOnce(&mut DevelopSettings)) {
    let mut s = (*c.photo(id).unwrap().develop).clone();
    f(&mut s);
    c.apply(Op::SetDevelop { id, settings: Arc::new(s), label: "Edit".into(), edited: None }).unwrap();
}

/// The server's library and another, never synced, that shares a photo with it — and the same ids for different photos.
fn two_libraries() -> (Catalog, Catalog, [PhotoId; 2], [PhotoId; 2]) {
    let mut server = Catalog::new();
    server.set_id_space(0);
    let (sa, sb) = (photo(&mut server, H1, "a.jpg"), photo(&mut server, H2, "b.jpg"));
    album(&mut server, "Trip", &[sa, sb]);
    let mut local = Catalog::new();
    let (la, lc) = (photo(&mut local, H1, "a-local.jpg"), photo(&mut local, H3, "c.jpg"));
    // (the ids collide on purpose: 1 and 2 mean other photos on the server)
    assert_eq!((la, lc), (sa, sb));
    album(&mut local, "Trip", &[la]);
    album(&mut local, "Mine", &[lc]);
    (server, local, [sa, sb], [la, lc])
}

/// What replaying the merge's changes on the server's state gives is the merged library.
fn replay(server: &Catalog, out: &mut Outbox) -> Catalog {
    let mut c = server.clone();
    c.set_id_space(3);
    let (ops, _) = out.take_push(usize::MAX);
    for op in ops {
        c.apply(op).unwrap();
    }
    c
}

#[test]
fn photos_are_matched_by_content_and_the_rest_comes_across_under_new_ids() {
    let (server, mut local, [sa, _sb], [la, lc]) = two_libraries();
    local.apply(Op::SetRating { id: la, rating: 4 }).unwrap();
    with_develop(&mut local, la, |s| s.light.exposure = 1.0);
    let mut server = server;
    with_develop(&mut server, sa, |s| s.light.contrast = 20.0);
    let mut merged = merge(&local, server.clone(), 3);
    let r = merged.report.clone();
    assert_eq!((r.matched, r.added, r.albums_matched, r.albums_added), (1, 1, 1, 1), "{r:?}");
    // the photo on both sides: the server's, with this library's rating and exposure added to its contrast
    let m = &merged.catalog;
    let a = m.photo(sa).unwrap();
    assert_eq!((a.rating, a.develop.light.exposure, a.develop.light.contrast), (4, 1.0, 20.0));
    assert_eq!(merged.photos[&la], sa);
    // the photo only here has a new id in this device's space, and points at its content
    let c = merged.photos[&lc];
    assert_eq!(c.0 >> ID_SPACE_SHIFT, 3);
    assert!(matches!(&m.photo(c).unwrap().source, Source::File { path } if path.starts_with("web/")), "{:?}", m.photo(c).unwrap().source);
    // albums: Trip is one album with both sides' photos, Mine is added and holds the new id
    let trips: Vec<_> = m.albums().filter(|a| a.name == "Trip").collect();
    assert_eq!(trips.len(), 1);
    assert!(trips[0].photos.contains(&sa) && trips[0].photos.len() == 2, "{:?}", trips[0].photos);
    let mine = m.albums().find(|a| a.name == "Mine").unwrap();
    assert_eq!((mine.photos.clone(), mine.id.0 >> ID_SPACE_SHIFT), (vec![c], 3));
    // pushing the changes on the server's state ends where the merge did
    let replayed = replay(&server, &mut merged.outbox);
    assert_eq!(exposure(&replayed, sa), 1.0);
    assert_eq!(replayed.len(), m.len());
    assert_eq!(replayed.albums().count(), m.albums().count());
    assert_eq!(replayed.photo(c).unwrap().content_hash.as_deref(), Some(H3));
}

#[test]
fn where_both_changed_the_same_thing_the_server_stays() {
    let (mut server, mut local, [sa, _], [la, _]) = two_libraries();
    server.apply(Op::SetRating { id: sa, rating: 2 }).unwrap();
    local.apply(Op::SetRating { id: la, rating: 5 }).unwrap();
    with_develop(&mut server, sa, |s| s.light.exposure = -1.0);
    with_develop(&mut local, la, |s| s.light.exposure = 2.0);
    let merged = merge(&local, server, 3);
    let a = merged.catalog.photo(sa).unwrap();
    assert_eq!((a.rating, a.develop.light.exposure), (2, -1.0));
    assert!(merged.report.conflicts >= 2, "{:?}", merged.report);
}

#[test]
fn merging_again_changes_nothing() {
    let (server, mut local, _, [la, _]) = two_libraries();
    local.apply(Op::SetRating { id: la, rating: 4 }).unwrap();
    let first = merge(&local, server, 3);
    let again = merge(&local, first.catalog.clone(), 3);
    let r = again.report;
    assert_eq!((r.added, r.albums_added, r.stacks_added), (0, 0, 0), "{r:?}");
    assert_eq!(r.matched, 2);
    let mut out = again.outbox;
    assert!(out.is_empty(), "{} change(s)", out.take_push(usize::MAX).0.len());
    assert_eq!(again.catalog.len(), first.catalog.len());
}

#[test]
fn virtual_copies_stacks_smart_albums_and_label_names_come_across() {
    let (server, mut local, _, [la, lc]) = two_libraries();
    // a virtual copy of the matched photo, a stack, a smart album naming a photo and an album, a label name
    let copy = local.alloc_photo_id();
    let mut cp = Photo::new(copy, Source::File { path: "/disk/a-local.jpg".into() }, "a-local.jpg", "JPEG", 60, 40, "2026-01-01T00:00:00");
    cp.content_hash = Some(format!("{H1}:dup{}", copy.0));
    cp.copy_of = Some(la);
    cp.copy_name = Some("Copy 1".into());
    local.apply(Op::AddPhoto { photo: Box::new(cp) }).unwrap();
    let stack = local.alloc_stack_id();
    local.apply(Op::AddStack { stack: Stack { id: stack, photos: vec![lc, copy], collapsed: true } }).unwrap();
    let mine = local.albums().find(|a| a.name == "Mine").unwrap().id;
    let smart = local.alloc_album_id();
    let mut a = Album::new(smart, "Picks of mine");
    a.smart = Some(Box::new(Filter { only: vec![lc], album: Some(mine), ..Default::default() }));
    local.apply(Op::AddAlbum { album: a }).unwrap();
    local.apply(Op::SetLabelName { label: ColorLabel::Red, name: Some("Keep".into()) }).unwrap();

    let merged = merge(&local, server, 3);
    let m = &merged.catalog;
    // the copy is a photo of its own, of the server's matched original, with its own name for its file
    let c = m.photo(merged.photos[&copy]).unwrap();
    assert_eq!((c.copy_of, c.copy_name.as_deref()), (Some(merged.photos[&la]), Some("Copy 1")));
    assert_eq!(c.content_hash.as_deref(), Some(format!("{H1}:dup{}", c.id.0).as_str()));
    // the stack holds the renumbered photos
    assert_eq!(m.stacks.len(), 1);
    assert_eq!(m.stacks.values().next().unwrap().photos, vec![merged.photos[&lc], c.id]);
    // the smart album's rules name the new ids
    let s = m.albums().find(|a| a.name == "Picks of mine").unwrap();
    let f = s.smart.as_ref().unwrap();
    assert_eq!((f.only.clone(), f.album), (vec![merged.photos[&lc]], Some(merged.albums[&mine])));
    assert_eq!(m.label_names.get(&ColorLabel::Red).map(String::as_str), Some("Keep"));
    assert_eq!(merged.report.stacks_added, 1);
}

#[test]
fn hostile_libraries_do_not_panic() {
    // empty on either side, photos without hashes, copies of photos that aren't there, albums inside themselves
    let empty = Catalog::new();
    let r = merge(&empty, empty.clone(), 1).report;
    assert_eq!((r.matched, r.added), (0, 0));
    let mut local = Catalog::new();
    let id = local.alloc_photo_id();
    let mut p = Photo::new(id, Source::File { path: "/x/y.jpg".into() }, "y.jpg", "JPEG", 1, 1, "");
    p.copy_of = Some(PhotoId(999));
    p.content_hash = Some("zzz:dup5".into());
    local.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    let a = local.alloc_album_id();
    let mut al = Album::new(a, "loop");
    al.parent = Some(a);
    al.cover = Some(PhotoId(12345));
    let _ = local.apply(Op::AddAlbum { album: al });
    let merged = merge(&local, Catalog::new(), 1);
    assert_eq!(merged.report.added, 1);
    assert_eq!(merged.report.unreadable, 1);
    assert_eq!(merged.catalog.photo(merged.photos[&id]).unwrap().copy_of, None, "its original isn't in either library");
}

#[test]
fn what_is_local_to_this_device_is_carried_under_the_new_ids() {
    let (server, mut local, _, [la, lc]) = two_libraries();
    // a Local record (a photo seen while browsing a folder) and a browsed folder
    let rec = local.alloc_photo_id();
    let mut p = Photo::new(rec, Source::File { path: "/browse/r.jpg".into() }, "r.jpg", "JPEG", 1, 1, "");
    p.local = true;
    local.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    local.apply(Op::SetBrowsed { folder: "/browse".into(), at: Some("2026-01-02T00:00:00".into()) }).unwrap();
    let mut merged = merge(&local, server, 3);
    let locals = carry_local_mapped(&local, &mut merged.catalog, &merged.photos);
    let m = &merged.catalog;
    // each photo is where its file is on this disk
    assert!(matches!(&m.photo(merged.photos[&lc]).unwrap().source, Source::File { path } if path == "/disk/c.jpg"));
    assert!(matches!(&m.photo(merged.photos[&la]).unwrap().source, Source::File { path } if path == "/disk/a-local.jpg"));
    // the Local record has a new id too, and stays Local
    let r = m.photo(locals[&rec]).unwrap();
    assert!(r.local && r.id.0 >> ID_SPACE_SHIFT == 3);
    assert!(m.browsed_folders().contains_key("/browse"));
}
