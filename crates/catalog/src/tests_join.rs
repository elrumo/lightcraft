//! Joining a library with photos to a server library that has some (`join_libraries`).

use std::sync::Arc;

use super::join::*;
use super::sync::*;
use super::*;

fn hash(n: u8) -> String {
    format!("{n:02x}").repeat(16)
}

/// A photo with the file `name` (its content: `content` as a hash; none for a photo without one).
fn photo(id: u64, name: &str, content: Option<u8>) -> Photo {
    let mut p = Photo::new(PhotoId(id), Source::File { path: format!("/pics/{name}") }, name, "JPEG", 60, 40, "2026-01-01T00:00:00");
    p.content_hash = content.map(hash);
    p
}

fn add(c: &mut Catalog, p: Photo) -> PhotoId {
    let id = p.id;
    c.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    id
}

fn album(c: &mut Catalog, name: &str, parent: Option<AlbumId>, photos: &[PhotoId]) -> AlbumId {
    let id = c.alloc_album_id();
    let mut a = Album::new(id, name);
    a.parent = parent;
    a.photos = photos.to_vec();
    c.apply(Op::AddAlbum { album: a }).unwrap();
    id
}

fn folder(c: &mut Catalog, name: &str) -> AlbumId {
    let id = c.alloc_album_id();
    let mut a = Album::new(id, name);
    a.folder = true;
    c.apply(Op::AddAlbum { album: a }).unwrap();
    id
}

/// The server's library: ids from the first device that uploaded it (space 0), and a library of this
/// device's own, whose ids collide with them.
fn libraries() -> (Catalog, Catalog) {
    let mut server = Catalog::new();
    let s1 = add(&mut server, photo(1, "server-a.jpg", Some(1)));
    let _s2 = add(&mut server, photo(2, "server-b.jpg", Some(2)));
    album(&mut server, "Holidays", None, &[s1]);
    let mut local = Catalog::new();
    let l1 = add(&mut local, photo(1, "mine-a.jpg", Some(11)));
    let l2 = add(&mut local, photo(2, "mine-b.jpg", Some(12)));
    let l3 = add(&mut local, photo(3, "mine-c.jpg", None));
    album(&mut local, "Holidays", None, &[l1, l2]);
    album(&mut local, "Mine", None, &[l3]);
    (local, server)
}

fn names(c: &Catalog) -> Vec<String> {
    let mut v: Vec<String> = c.photos().map(|p| p.file_name.clone()).collect();
    v.sort();
    v
}

#[test]
fn this_librarys_photos_are_added_under_ids_of_this_devices_space() {
    let (local, mut server) = libraries();
    let mut outbox = Outbox::default();
    let joined = join_libraries(&local, &mut server, 5, &mut outbox);
    assert_eq!(joined, Joined { photos_added: 3, albums_added: 1, albums_merged: 1, ..Default::default() });
    assert_eq!(names(&server), ["mine-a.jpg", "mine-b.jpg", "mine-c.jpg", "server-a.jpg", "server-b.jpg"]);
    // the server's photos keep their ids; this library's are in this device's space, so they can't collide
    let added: Vec<u64> = server.photos().filter(|p| p.file_name.starts_with("mine")).map(|p| p.id.0).collect();
    assert!(added.iter().all(|id| id >> ID_SPACE_SHIFT == 5), "{added:?}");
    assert_eq!(server.photo(PhotoId(1)).unwrap().file_name, "server-a.jpg");
    // "Holidays" is one album, with the server's photo and this library's two
    let holidays: Vec<&Album> = server.albums().filter(|a| a.name == "Holidays").collect();
    assert_eq!(holidays.len(), 1);
    let members: Vec<String> = holidays[0].photos.iter().map(|id| server.photo(*id).unwrap().file_name.clone()).collect();
    assert_eq!(members, ["server-a.jpg", "mine-a.jpg", "mine-b.jpg"]);
    let mine = server.albums().find(|a| a.name == "Mine").unwrap();
    assert_eq!(server.photo(mine.photos[0]).unwrap().file_name, "mine-c.jpg");
    assert_eq!(mine.id.0 >> ID_SPACE_SHIFT, 5);
}

#[test]
fn what_was_joined_goes_to_the_server_as_ordinary_changes() {
    let (local, server_catalog) = libraries();
    // a real server holding the library, and this device's copy of it
    let mut core = ServerCore::open(Box::new(MemStore::default())).unwrap();
    let ops: Vec<Op> = server_catalog
        .photos()
        .map(|p| Op::AddPhoto { photo: Box::new((**p).clone()) })
        .chain(server_catalog.albums().map(|a| Op::AddAlbum { album: a.clone() }))
        .collect();
    core.push(0, &ops).unwrap();
    let (seq, snap) = core.snapshot();
    let mut mine = Catalog::from_snapshot(&snap).unwrap();
    let mut outbox = Outbox::default();
    join_libraries(&local, &mut mine, 7, &mut outbox);
    assert!(!outbox.is_empty());

    let (ops, pushed) = outbox.take_push(usize::MAX);
    let head = core.push(seq, &ops).expect("the server takes what the join queued, in that order");
    outbox.acked(&pushed);
    assert!(outbox.is_empty());
    assert_eq!(core.head(), head);
    // the server now has everything this device sees (the files this device has aside)
    assert_eq!(names(core.catalog()), names(&mine));
    let ids = |c: &Catalog| c.photos().map(|p| p.id).collect::<Vec<_>>();
    assert_eq!(ids(core.catalog()), ids(&mine));
    let albums = |c: &Catalog| c.albums().map(|a| (a.id, a.name.clone(), a.photos.clone())).collect::<Vec<_>>();
    assert_eq!(albums(core.catalog()), albums(&mine));
    // pushed as `web/<hash>/<name>`, not as a path on this computer
    let on_server = core.catalog().photos().find(|p| p.file_name == "mine-a.jpg").unwrap();
    assert_eq!(on_server.source, Source::File { path: format!("web/{}/mine-a.jpg", hash(11)) });
    // this device keeps pointing at its own file
    let here = mine.photos().find(|p| p.file_name == "mine-a.jpg").unwrap();
    assert_eq!(here.source, Source::File { path: "/pics/mine-a.jpg".into() });
}

#[test]
fn a_photo_both_libraries_have_is_kept_once_and_filled_in() {
    let mut server = Catalog::new();
    let mut s = photo(1, "IMG_1.jpg", Some(9));
    s.rating = 3;
    s.meta.keywords = vec!["city".into()];
    s.meta.title = "Server title".into();
    add(&mut server, s);
    add(&mut server, photo(2, "IMG_2.jpg", Some(10)));
    let mut local = Catalog::new();
    let mut l = photo(1, "IMG_1-copy.jpg", Some(9));
    l.rating = 5;
    l.meta.keywords = vec!["night".into()];
    l.meta.title = "My title".into();
    l.meta.caption = "My caption".into();
    add(&mut local, l);
    let mut l2 = photo(2, "IMG_2-copy.jpg", Some(10));
    l2.rating = 4;
    l2.flag = Flag::Pick;
    l2.label = Some(ColorLabel::Red);
    let mut d = (*l2.develop).clone();
    d.light.exposure = 1.0;
    l2.develop = Arc::new(d);
    add(&mut local, l2);

    let mut outbox = Outbox::default();
    let joined = join_libraries(&local, &mut server, 3, &mut outbox);
    assert_eq!(joined, Joined { photos_shared: 2, ..Default::default() });
    assert_eq!(server.len(), 2, "no photo twice");
    // both have a rating, a title: the server's stay; the caption it lacked comes from here; keywords are both
    let one = server.photo(PhotoId(1)).unwrap();
    assert_eq!((one.rating, one.meta.title.as_str(), one.meta.caption.as_str()), (3, "Server title", "My caption"));
    let mut keywords = one.meta.keywords.clone();
    keywords.sort();
    assert_eq!(keywords, ["city", "night"]);
    // the server's copy had nothing: what this library knew is filled in
    let two = server.photo(PhotoId(2)).unwrap();
    assert_eq!((two.rating, two.flag, two.label), (4, Flag::Pick, Some(ColorLabel::Red)));
    assert!((two.develop.light.exposure - 1.0).abs() < 1e-9);
    // the photo shows from this device's file here
    assert_eq!(one.file_name, "IMG_1-copy.jpg");
    assert_eq!(one.source, Source::File { path: "/pics/IMG_1-copy.jpg".into() });
    // …and only the changes were queued: no photo was added
    let (ops, _) = outbox.take_push(usize::MAX);
    assert!(ops.iter().all(|op| !matches!(op, Op::AddPhoto { .. })), "{ops:?}");
    assert!(ops.iter().any(|op| matches!(op, Op::SetRating { id: PhotoId(2), rating: 4 })), "{ops:?}");
}

#[test]
fn two_copies_of_a_file_here_are_two_photos_even_when_the_server_has_it_once() {
    let mut server = Catalog::new();
    add(&mut server, photo(1, "a.jpg", Some(4)));
    let mut local = Catalog::new();
    add(&mut local, photo(1, "x.jpg", Some(4)));
    add(&mut local, photo(2, "y.jpg", Some(4)));
    let mut outbox = Outbox::default();
    let joined = join_libraries(&local, &mut server, 2, &mut outbox);
    assert_eq!((joined.photos_shared, joined.photos_added), (1, 1));
    assert_eq!(server.len(), 2);
}

#[test]
fn a_converted_copy_is_not_the_original() {
    let mut server = Catalog::new();
    add(&mut server, photo(1, "a.cr3", Some(4)));
    let mut local = Catalog::new();
    let mut p = photo(1, "a.dng", Some(4));
    p.content_hash = Some(format!("{}:dng", hash(4)));
    add(&mut local, p);
    let mut outbox = Outbox::default();
    let joined = join_libraries(&local, &mut server, 2, &mut outbox);
    assert_eq!((joined.photos_shared, joined.photos_added), (0, 1), "the suffix keeps them apart");
}

#[test]
fn virtual_copies_stacks_folders_and_smart_albums_follow_their_photos() {
    let mut server = Catalog::new();
    add(&mut server, photo(1, "s.jpg", Some(1)));
    let mut local = Catalog::new();
    let a = add(&mut local, photo(1, "a.jpg", Some(21)));
    let b = add(&mut local, photo(2, "b.jpg", Some(22)));
    let mut copy = photo(3, "a.jpg", Some(21));
    copy.copy_of = Some(a);
    copy.copy_name = Some("Copy 1".into());
    let c = add(&mut local, copy);
    local.apply(Op::AddStack { stack: Stack { id: StackId(1), photos: vec![a, b], collapsed: true } }).unwrap();
    let trips = folder(&mut local, "Trips");
    let italy = album(&mut local, "Italy", Some(trips), &[a, c]);
    // a smart album that looks at the album Italy and at one photo
    let smart_id = local.alloc_album_id();
    let mut smart = Album::new(smart_id, "Italy, rated");
    smart.smart = Some(Box::new(Filter {
        only: vec![b],
        album: Some(italy),
        rule_set: Some(RuleSet {
            mode: Match::All,
            rules: vec![Rule::Field { field: "album".into(), op: "is".into(), value: serde_json::json!(italy.0) }],
        }),
        ..Default::default()
    }));
    local.apply(Op::AddAlbum { album: smart }).unwrap();

    let mut outbox = Outbox::default();
    let joined = join_libraries(&local, &mut server, 4, &mut outbox);
    assert_eq!((joined.photos_added, joined.stacks_added, joined.albums_added), (3, 1, 3));
    let id_of = |c: &Catalog, name: &str, copy: bool| c.photos().find(|p| p.file_name == name && p.copy_of.is_some() == copy).unwrap().id;
    let (na, nb) = (id_of(&server, "a.jpg", false), id_of(&server, "b.jpg", false));
    let nc = id_of(&server, "a.jpg", true);
    assert_eq!(server.photo(nc).unwrap().copy_of, Some(na), "the copy points at the new id of its photo");
    let stack = server.stacks().next().unwrap();
    assert_eq!((stack.photos.clone(), stack.collapsed), (vec![na, nb], true));
    let new_trips = server.albums().find(|x| x.name == "Trips").unwrap();
    let new_italy = server.albums().find(|x| x.name == "Italy").unwrap();
    assert!(new_trips.folder);
    assert_eq!(new_italy.parent, Some(new_trips.id));
    assert_eq!(new_italy.photos, [na, nc]);
    let smart = server.albums().find(|x| x.name == "Italy, rated").unwrap().smart.clone().unwrap();
    assert_eq!(smart.only, [nb]);
    assert_eq!(smart.album, Some(new_italy.id));
    let Some(Rule::Field { value, .. }) = smart.rule_set.as_ref().and_then(|r| r.rules.first()) else { panic!("the rule is gone") };
    assert_eq!(value.as_u64(), Some(new_italy.id.0));
}

#[test]
fn photos_already_stacked_on_the_server_stay_in_their_stack() {
    let mut server = Catalog::new();
    let s1 = add(&mut server, photo(1, "s1.jpg", Some(1)));
    let s2 = add(&mut server, photo(2, "s2.jpg", Some(2)));
    server.apply(Op::AddStack { stack: Stack { id: StackId(1), photos: vec![s1, s2], collapsed: false } }).unwrap();
    let mut local = Catalog::new();
    let a = add(&mut local, photo(1, "a.jpg", Some(1)));
    let b = add(&mut local, photo(2, "b.jpg", Some(50)));
    local.apply(Op::AddStack { stack: Stack { id: StackId(1), photos: vec![a, b], collapsed: false } }).unwrap();
    let mut outbox = Outbox::default();
    let joined = join_libraries(&local, &mut server, 2, &mut outbox);
    assert_eq!((joined.photos_shared, joined.photos_added, joined.stacks_added), (1, 1, 0));
    assert_eq!(server.stacks().count(), 1);
}

#[test]
fn demo_photos_and_local_records_are_not_shared() {
    let mut server = Catalog::new();
    add(&mut server, photo(1, "s.jpg", Some(1)));
    let mut local = Catalog::new();
    add(&mut local, Photo::new(PhotoId(1), Source::Demo { scene: 3 }, "demo.jpg", "JPEG", 60, 40, "2026-01-01T00:00:00"));
    add(&mut local, photo(2, "mine.jpg", Some(2)));
    let mut browsed = photo(3, "browsed.jpg", Some(3));
    browsed.local = true;
    add(&mut local, browsed);
    local.apply(Op::SetBrowsed { folder: "/pics".into(), at: Some("2026-02-02T00:00:00".into()) }).unwrap();

    assert_eq!(counts(&local), Counts { photos: 1, albums: 0, edited: 0 });
    assert_eq!(shared_photos(&local, &server), 0);
    let mut outbox = Outbox::default();
    let joined = join_libraries(&local, &mut server, 2, &mut outbox);
    assert_eq!(joined.photos_added, 1);
    // pushed: the one photo; kept here: the record of the folder browsed, under an id of this space
    let (ops, _) = outbox.take_push(usize::MAX);
    assert_eq!(ops.len(), 1, "{ops:?}");
    assert!(server.photos().any(|p| p.local && p.file_name == "browsed.jpg" && p.id.0 >> ID_SPACE_SHIFT == 2));
    assert!(server.photos().all(|p| p.file_name != "demo.jpg"));
    assert!(server.to_snapshot().contains("browsed.jpg"));
}

#[test]
fn counting_what_both_libraries_have() {
    let (local, server) = libraries();
    assert_eq!(counts(&local), Counts { photos: 3, albums: 2, edited: 0 });
    assert_eq!(shared_photos(&local, &server), 0);
    let mut both = server.clone();
    add(&mut both, photo(9, "x.jpg", Some(11)));
    assert_eq!(shared_photos(&local, &both), 1);
}

#[test]
fn label_names_this_library_set_come_along_unless_the_server_has_its_own() {
    let mut server = Catalog::new();
    add(&mut server, photo(1, "s.jpg", Some(1)));
    server.apply(Op::SetLabelName { label: ColorLabel::Red, name: Some("Server red".into()) }).unwrap();
    let mut local = Catalog::new();
    add(&mut local, photo(1, "a.jpg", Some(2)));
    local.apply(Op::SetLabelName { label: ColorLabel::Red, name: Some("My red".into()) }).unwrap();
    local.apply(Op::SetLabelName { label: ColorLabel::Blue, name: Some("My blue".into()) }).unwrap();
    let mut outbox = Outbox::default();
    join_libraries(&local, &mut server, 2, &mut outbox);
    assert_eq!(server.custom_label_name(ColorLabel::Red), Some("Server red"));
    assert_eq!(server.custom_label_name(ColorLabel::Blue), Some("My blue"));
}

#[test]
fn a_backup_is_a_catalog_snapshot_that_opens_again() {
    let (local, _) = libraries();
    let mut store = MemStore::default();
    let bytes = journal::write_backup(&mut store, "backup.snap", &local).unwrap();
    let written = store.read("backup.snap").unwrap().unwrap();
    assert_eq!(bytes as usize, written.len());
    // as a library's own catalog.snap, it loads back to the same catalog
    let mut again = MemStore::default();
    again.write_atomic(journal::SNAPSHOT, &written).unwrap();
    let (_, loaded, _) = Journal::open(Box::new(again)).unwrap();
    assert_eq!(loaded.to_snapshot(), local.to_snapshot());
}

#[test]
fn a_photo_in_the_servers_recently_deleted_is_not_the_same_photo() {
    let mut server = Catalog::new();
    let gone = add(&mut server, photo(1, "gone.jpg", Some(7)));
    server.apply(Op::SetDeleted { id: gone, deleted: true }).unwrap();
    let mut local = Catalog::new();
    add(&mut local, photo(1, "mine.jpg", Some(7)));
    assert_eq!(shared_photos(&local, &server), 0);
    let mut outbox = Outbox::default();
    let joined = join_libraries(&local, &mut server, 2, &mut outbox);
    assert_eq!((joined.photos_shared, joined.photos_added), (0, 1), "the photo stays in this library, live");
    assert!(server.photos().any(|p| p.file_name == "mine.jpg" && !p.deleted));
}
