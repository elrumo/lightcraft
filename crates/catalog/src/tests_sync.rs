use std::sync::Arc;

use lightcraft_develop::DevelopSettings;
use serde_json::json;

use super::sync::*;
use super::*;

fn add_photo(c: &mut Catalog) -> Op {
    let id = c.alloc_photo_id();
    Op::AddPhoto { photo: Box::new(Photo::new(id, Source::Demo { scene: 1 }, "a.jpg", "JPEG", 60, 40, "2026-01-01T00:00:00")) }
}

fn develop(c: &Catalog, id: PhotoId, f: impl FnOnce(&mut DevelopSettings)) -> Op {
    let mut s = (*c.photo(id).unwrap().develop).clone();
    f(&mut s);
    Op::SetDevelop { id, settings: Arc::new(s), label: "Edit".into(), edited: None }
}

/// A device: its catalog, pending changes and how far it has pulled.
struct Replica {
    cat: Catalog,
    out: Outbox,
    cursor: u64,
    space: u32,
    dirty: bool,
}

impl Replica {
    fn join(s: &ServerCore, space: u32) -> Replica {
        let (cursor, snap) = s.snapshot();
        let mut cat = Catalog::from_snapshot(&snap).unwrap();
        cat.set_id_space(space);
        Replica { cat, out: Outbox::default(), cursor, space, dirty: false }
    }
    fn local(&mut self, op: Op) {
        if let Ok(inv) = self.cat.apply(op.clone()) {
            self.out.record(&op, &inv, &self.cat);
        }
    }
    fn pull(&mut self, s: &mut ServerCore) {
        match s.since(self.cursor, usize::MAX).unwrap() {
            Pull::Ops(ops) => {
                for (seq, op) in ops {
                    if let Some(op) = self.out.merge_remote(&op)
                        && self.cat.apply(op).is_err()
                    {
                        self.dirty = true;
                    }
                    self.cursor = seq;
                }
            }
            Pull::Gone => self.dirty = true,
        }
    }
    fn push(&mut self, s: &mut ServerCore, lose_ack: bool) {
        let (ops, pushed) = self.out.take_push(usize::MAX);
        if ops.is_empty() {
            return;
        }
        match s.push(self.cursor, &ops) {
            Ok(head) if !lose_ack => {
                self.out.acked(&pushed);
                self.cursor = head;
            }
            Ok(_) | Err(PushError::Behind { .. }) | Err(PushError::Storage(_)) => {}
            Err(PushError::Rejected { index, .. }) => {
                self.out.rejected(&pushed, index);
                self.dirty = true;
            }
        }
    }
    fn repair(&mut self, s: &ServerCore) {
        let (seq, snap) = s.snapshot();
        let mut c = Catalog::from_snapshot(&snap).unwrap();
        carry_local(&self.cat, &mut c);
        c.set_id_space(self.space);
        self.out.rebase(&mut c);
        self.cat = c;
        self.cursor = seq;
        self.dirty = false;
    }
    fn sync(&mut self, s: &mut ServerCore) {
        self.pull(s);
        if self.dirty {
            self.repair(s);
        }
        self.push(s, false);
    }
}

/// What every device must agree on (ids counters and device-local state aside).
fn shared(c: &Catalog) -> Catalog {
    let mut c = c.clone();
    for p in c.photos.values_mut() {
        let p = Arc::make_mut(p);
        p.history.clear();
        p.local_baseline = None;
    }
    c.next_photo = 0;
    c.next_album = 0;
    c.next_stack = 0;
    c.browsed.clear();
    c.revision = 0;
    c
}

fn server() -> ServerCore {
    ServerCore::open(Box::new(MemStore::new())).unwrap()
}

#[test]
fn id_spaces_keep_devices_apart() {
    let mut a = Catalog::new();
    let legacy = a.alloc_photo_id();
    assert_eq!(legacy, PhotoId(1), "a library that never synced is unchanged");
    a.set_id_space(3);
    let p = a.alloc_photo_id();
    assert_eq!(p.0, (3 << ID_SPACE_SHIFT) | 1);
    assert_eq!(a.alloc_album_id().0, (3 << ID_SPACE_SHIFT) | 1);
    // a photo from another device's space doesn't move this device's counter
    let other = PhotoId((5 << ID_SPACE_SHIFT) | 9);
    a.apply(Op::AddPhoto { photo: Box::new(Photo::new(other, Source::Demo { scene: 1 }, "x", "JPEG", 1, 1, "t")) }).unwrap();
    assert_eq!(a.alloc_photo_id().0, (3 << ID_SPACE_SHIFT) | 2);
    // reopened (the counter is saved), and re-entering the space after the highest id in it
    let mut b = Catalog::from_snapshot(&a.to_snapshot()).unwrap();
    b.set_id_space(3);
    assert_eq!(b.alloc_photo_id().0, (3 << ID_SPACE_SHIFT) | 3);
    let mut c = Catalog::from_snapshot(&a.to_snapshot()).unwrap();
    c.set_id_space(5);
    assert_eq!(c.alloc_photo_id().0, (5 << ID_SPACE_SHIFT) | 10);
    // hostile ids don't overflow
    c.apply(Op::AddPhoto { photo: Box::new(Photo::new(PhotoId(u64::MAX), Source::Demo { scene: 1 }, "x", "JPEG", 1, 1, "t")) }).unwrap();
}

#[test]
fn merge3_keeps_both_sides() {
    let base = json!({"light": {"exposure": 0, "contrast": 0}, "photos": [1, 2, 3], "curve": [0, 1]});
    let ours = json!({"light": {"exposure": 1, "contrast": 0}, "photos": [1, 3, 4], "curve": [0, 2]});
    let theirs = json!({"light": {"exposure": 0, "contrast": 20}, "photos": [1, 2, 3, 5], "curve": [0, 3]});
    assert_eq!(
        merge3(&base, &ours, &theirs),
        json!({"light": {"exposure": 1, "contrast": 20}, "photos": [1, 3, 5, 4], "curve": [0, 2]}),
        "both sliders, both album changes; other arrays are one value (ours)"
    );
    // a field only we added, one only we removed
    assert_eq!(merge3(&json!({"a": 1}), &json!({"b": 2}), &json!({"a": 1, "c": 3})), json!({"b": 2, "c": 3}));
    assert_eq!(merge3(&json!(1), &json!(1), &json!(2)), json!(2), "unchanged here: theirs");
}

#[test]
fn outbox_records_shared_changes_only() {
    let mut c = Catalog::new();
    let add = add_photo(&mut c);
    let mut out = Outbox::default();
    let inv = c.apply(add.clone()).unwrap();
    out.record(&add, &inv, &c);
    let id = PhotoId(1);
    // a batch is recorded op by op; History stays here
    let batch = Op::Batch {
        ops: vec![
            Op::SetRating { id, rating: 3 },
            Op::PushHistory { id, step: HistoryStep { label: "x".into(), settings: Arc::default() } },
            Op::SetBrowsed { folder: "/x".into(), at: Some("t".into()) },
        ],
    };
    let inv = c.apply(batch.clone()).unwrap();
    out.record(&batch, &inv, &c);
    let (ops, _) = out.take_push(10);
    assert_eq!(ops.len(), 2);
    assert!(matches!(ops[1], Op::SetRating { rating: 3, .. }));
    // a second change to the same value replaces the pending one (and moves it last)
    let op = Op::SetRating { id, rating: 5 };
    let inv = c.apply(op.clone()).unwrap();
    out.record(&op, &inv, &c);
    assert_eq!(out.len(), 2);

    // a Local record isn't shared until it's added to the library, then it goes whole
    let mut local = Photo::new(c.alloc_photo_id(), Source::File { path: "/disk/b.jpg".into() }, "b.jpg", "JPEG", 1, 1, "t");
    local.local = true;
    local.content_hash = Some("00ff".into());
    let lid = local.id;
    let add = Op::AddPhoto { photo: Box::new(local) };
    let inv = c.apply(add.clone()).unwrap();
    out.record(&add, &inv, &c);
    let rate = Op::SetRating { id: lid, rating: 2 };
    let inv = c.apply(rate.clone()).unwrap();
    out.record(&rate, &inv, &c);
    assert_eq!(out.len(), 2);
    let promote = Op::SetLocal { id: lid, local: false };
    let inv = c.apply(promote.clone()).unwrap();
    out.record(&promote, &inv, &c);
    let (ops, _) = out.take_push(10);
    let Some(Op::AddPhoto { photo }) = ops.last() else { panic!("{ops:?}") };
    assert_eq!((photo.rating, photo.local), (2, false));
    assert_eq!(photo.source, Source::File { path: "web/00ff/b.jpg".into() }, "points at its content");
}

#[test]
fn outbox_acks_only_what_did_not_change() {
    let mut c = Catalog::new();
    let add = add_photo(&mut c);
    c.apply(add).unwrap();
    let id = PhotoId(1);
    let mut out = Outbox::default();
    let op = Op::SetRating { id, rating: 1 };
    let inv = c.apply(op.clone()).unwrap();
    out.record(&op, &inv, &c);
    let (_, pushed) = out.take_push(10);
    // changed again while the push was on its way
    let op = Op::SetRating { id, rating: 2 };
    let inv = c.apply(op.clone()).unwrap();
    out.record(&op, &inv, &c);
    out.acked(&pushed);
    assert_eq!(out.len(), 1);
    // our op coming back (its ack was lost) settles it
    assert_eq!(out.merge_remote(&Op::SetRating { id, rating: 2 }), None);
    assert!(out.is_empty());
}

#[test]
fn server_core_orders_validates_and_persists() {
    let store = MemStore::new();
    let mut s = ServerCore::open(Box::new(store.clone())).unwrap();
    let mut c = Catalog::new();
    c.set_id_space(1);
    let add = add_photo(&mut c);
    let Op::AddPhoto { photo } = &add else { panic!("{add:?}") };
    let id = photo.id;
    assert_eq!(s.push(0, std::slice::from_ref(&add)), Ok(1));
    assert_eq!(s.push(0, &[Op::SetRating { id, rating: 1 }]), Err(PushError::Behind { head: 1 }));
    // all or nothing
    let r = s.push(1, &[Op::SetRating { id, rating: 4 }, Op::SetRating { id: PhotoId(77), rating: 1 }]);
    assert!(matches!(r, Err(PushError::Rejected { index: 1, .. })), "{r:?}");
    assert_eq!(s.catalog().photo(id).unwrap().rating, 0);
    // device-local ops and batches aren't accepted
    assert!(matches!(s.push(1, &[Op::SetBrowsed { folder: "/x".into(), at: None }]), Err(PushError::Rejected { index: 0, .. })));
    assert!(matches!(s.push(1, &[Op::Batch { ops: vec![] }]), Err(PushError::Rejected { .. })));
    assert_eq!(s.push(1, &[Op::SetRating { id, rating: 4 }]), Ok(2));
    assert_eq!(s.since(0, 10).unwrap(), Pull::Ops(vec![(1, add), (2, Op::SetRating { id, rating: 4 })]));
    assert_eq!(s.since(1, 10).unwrap(), Pull::Ops(vec![(2, Op::SetRating { id, rating: 4 })]));
    assert_eq!(s.since(2, 10).unwrap(), Pull::Ops(vec![]));
    drop(s);
    let s = ServerCore::open(Box::new(store)).unwrap();
    assert_eq!((s.head(), s.catalog().photo(id).unwrap().rating), (2, 4));
}

#[test]
fn concurrent_edits_merge() {
    let mut s = server();
    let mut a = Replica::join(&s, 1);
    let add = add_photo(&mut a.cat);
    a.local(add);
    let id = a.cat.photos().next().unwrap().id;
    let album = a.cat.alloc_album_id();
    a.local(Op::AddAlbum { album: Album::new(album, "Trip") });
    a.sync(&mut s);
    let mut b = Replica::join(&s, 2);
    let p2 = add_photo(&mut b.cat);
    b.local(p2.clone());
    let id2 = b.cat.photos().map(|p| p.id).find(|p| *p != id).unwrap();
    // exposure on one device, contrast on the other; each adds a photo to the album
    a.local(develop(&a.cat, id, |s| s.light.exposure = 1.0));
    b.local(develop(&b.cat, id, |s| s.light.contrast = 20.0));
    a.local(Op::SetAlbumPhotos { id: album, photos: vec![id] });
    b.local(Op::SetAlbumPhotos { id: album, photos: vec![id2] });
    a.sync(&mut s);
    b.sync(&mut s);
    a.sync(&mut s);
    for r in [&a, &b] {
        let d = &r.cat.photo(id).unwrap().develop;
        assert_eq!((d.light.exposure, d.light.contrast), (1.0, 20.0));
        let mut photos = r.cat.album(album).unwrap().photos.clone();
        photos.sort();
        assert_eq!(photos, vec![id, id2]);
        assert_eq!(shared(&r.cat), shared(s.catalog()));
    }
    // a removal beats an edit
    a.local(Op::SetRating { id: id2, rating: 5 });
    b.local(b.cat.delete_permanently_ops(id2));
    b.sync(&mut s);
    a.sync(&mut s);
    b.sync(&mut s);
    assert!(a.cat.photo(id2).is_none() && s.catalog().photo(id2).is_none());
    assert_eq!(shared(&a.cat), shared(&b.cat));
}

#[test]
fn hostile_snapshot_stack_does_not_panic() {
    let s = Stack { id: StackId(1), photos: vec![], collapsed: false };
    assert_eq!(s.top(), PhotoId(0));
}

/// What a step of the convergence test does.
#[derive(Clone, Debug)]
enum Step {
    Rate(usize, u8),
    Flag(usize, u8),
    Develop(usize, u8, i8),
    AlbumAdd(usize, usize),
    AlbumRemove(usize, usize),
    Keyword(usize, u8),
    AddPhoto,
    Delete(usize),
    SoftDelete(usize, bool),
    AddAlbum,
    RemoveAlbum(usize),
    RenameAlbum(usize, u8),
    Sync,
    PushLosingAck,
    Pull,
}

fn step() -> impl proptest::strategy::Strategy<Value = Step> {
    use proptest::prelude::*;
    let i = || 0usize..8;
    prop_oneof![
        (i(), 0u8..6).prop_map(|(p, r)| Step::Rate(p, r)),
        (i(), 0u8..3).prop_map(|(p, f)| Step::Flag(p, f)),
        (i(), 0u8..3, -3i8..4).prop_map(|(p, f, v)| Step::Develop(p, f, v)),
        (i(), i()).prop_map(|(a, p)| Step::AlbumAdd(a, p)),
        (i(), i()).prop_map(|(a, p)| Step::AlbumRemove(a, p)),
        (i(), 0u8..4).prop_map(|(p, k)| Step::Keyword(p, k)),
        Just(Step::AddPhoto),
        i().prop_map(Step::Delete),
        (i(), any::<bool>()).prop_map(|(p, d)| Step::SoftDelete(p, d)),
        Just(Step::AddAlbum),
        i().prop_map(Step::RemoveAlbum),
        (i(), 0u8..3).prop_map(|(a, n)| Step::RenameAlbum(a, n)),
        Just(Step::Sync),
        Just(Step::Sync),
        Just(Step::PushLosingAck),
        Just(Step::Pull),
    ]
}

impl Replica {
    fn nth_photo(&self, i: usize) -> Option<PhotoId> {
        let ids: Vec<PhotoId> = self.cat.photos.keys().copied().collect();
        (!ids.is_empty()).then(|| ids[i % ids.len()])
    }
    fn nth_album(&self, i: usize) -> Option<AlbumId> {
        let ids: Vec<AlbumId> = self.cat.albums.keys().copied().collect();
        (!ids.is_empty()).then(|| ids[i % ids.len()])
    }
    fn run(&mut self, step: &Step, s: &mut ServerCore) {
        match *step {
            Step::Rate(p, r) => {
                if let Some(id) = self.nth_photo(p) {
                    self.local(Op::SetRating { id, rating: r });
                }
            }
            Step::Flag(p, f) => {
                if let Some(id) = self.nth_photo(p) {
                    self.local(Op::SetFlag { id, flag: [Flag::None, Flag::Pick, Flag::Reject][f as usize] });
                }
            }
            Step::Develop(p, f, v) => {
                if let Some(id) = self.nth_photo(p) {
                    let op = develop(&self.cat, id, |s| match f {
                        0 => s.light.exposure = f64::from(v),
                        1 => s.light.contrast = f64::from(v) * 10.0,
                        _ => s.light.highlights = f64::from(v) * 10.0,
                    });
                    self.local(op);
                }
            }
            Step::AlbumAdd(a, p) | Step::AlbumRemove(a, p) => {
                if let (Some(album), Some(id)) = (self.nth_album(a), self.nth_photo(p)) {
                    let mut photos = self.cat.album(album).unwrap().photos.clone();
                    photos.retain(|x| *x != id);
                    if matches!(step, Step::AlbumAdd(..)) {
                        photos.push(id);
                    }
                    self.local(Op::SetAlbumPhotos { id: album, photos });
                }
            }
            Step::Keyword(p, k) => {
                if let Some(id) = self.nth_photo(p) {
                    let mut meta = self.cat.photo(id).unwrap().meta.clone();
                    let kw = format!("k{k}");
                    if !meta.keywords.contains(&kw) {
                        meta.keywords.push(kw);
                        self.local(Op::SetMeta { id, meta: Box::new(meta) });
                    }
                }
            }
            Step::AddPhoto => {
                let op = add_photo(&mut self.cat);
                self.local(op);
            }
            Step::Delete(p) => {
                if let Some(id) = self.nth_photo(p) {
                    let op = self.cat.delete_permanently_ops(id);
                    self.local(op);
                }
            }
            Step::SoftDelete(p, d) => {
                if let Some(id) = self.nth_photo(p) {
                    self.local(Op::SetDeleted { id, deleted: d });
                }
            }
            Step::AddAlbum => {
                let id = self.cat.alloc_album_id();
                self.local(Op::AddAlbum { album: Album::new(id, "new") });
            }
            Step::RemoveAlbum(a) => {
                if let Some(id) = self.nth_album(a) {
                    self.local(Op::RemoveAlbum { id });
                }
            }
            Step::RenameAlbum(a, n) => {
                if let Some(id) = self.nth_album(a) {
                    self.local(Op::RenameAlbum { id, name: format!("n{n}") });
                }
            }
            Step::Sync => self.sync(s),
            Step::PushLosingAck => self.push(s, true),
            Step::Pull => {
                self.pull(s);
                if self.dirty {
                    self.repair(s);
                }
            }
        }
    }
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig { cases: 400, ..Default::default() })]

    /// Three devices doing anything in any interleaving end up with the server's library once
    /// they've all synced.
    #[test]
    fn devices_converge(steps in proptest::collection::vec((0usize..3, step()), 1..60)) {
        let mut s = server();
        let mut seed = Replica::join(&s, 1);
        for _ in 0..3 {
            let op = add_photo(&mut seed.cat);
            seed.local(op);
        }
        let album = seed.cat.alloc_album_id();
        seed.local(Op::AddAlbum { album: Album::new(album, "A") });
        seed.sync(&mut s);
        let mut devices: Vec<Replica> = (2..5).map(|space| Replica::join(&s, space)).collect();
        for (d, st) in &steps {
            devices[*d].run(st, &mut s);
        }
        for _ in 0..4 {
            for d in &mut devices {
                d.sync(&mut s);
            }
        }
        for d in &mut devices {
            d.pull(&mut s);
            proptest::prop_assert!(!d.dirty);
            proptest::prop_assert!(d.out.is_empty(), "pending: {:?}", d.out);
            proptest::prop_assert_eq!(shared(&d.cat), shared(s.catalog()));
        }
    }
}

#[test]
fn seeding_uploads_a_whole_library() {
    let mut c = Catalog::new();
    let ids: Vec<PhotoId> = (0..3)
        .map(|_| {
            let op = add_photo(&mut c);
            c.apply(op).unwrap();
            c.photos().last().unwrap().id
        })
        .collect();
    // the album's id comes first: seeding must still send its folder before it
    let album = c.alloc_album_id();
    let folder = c.alloc_album_id();
    let mut f = Album::new(folder, "Trips");
    f.folder = true;
    c.apply(Op::AddAlbum { album: f }).unwrap();
    c.apply(Op::AddAlbum { album: Album { parent: Some(folder), photos: vec![ids[0], ids[1]], ..Album::new(album, "Rome") } }).unwrap();
    c.apply(Op::AddStack { stack: Stack { id: StackId(1), photos: vec![ids[1], ids[2]], collapsed: true } }).unwrap();
    c.apply(Op::SetLabelName { label: ColorLabel::Red, name: Some("Print".into()) }).unwrap();
    let mut out = Outbox::default();
    out.seed(&c);
    let mut s = server();
    let (ops, _) = out.take_push(usize::MAX);
    assert_eq!(s.push(0, &ops), Ok(ops.len() as u64));
    assert_eq!(shared(s.catalog()), shared(&c));
}

#[test]
fn a_lost_ack_does_not_undo_a_later_change() {
    let mut s = server();
    let mut a = Replica::join(&s, 1);
    let add = add_photo(&mut a.cat);
    a.local(add);
    a.sync(&mut s);
    let id = a.cat.photos().next().unwrap().id;
    a.local(develop(&a.cat, id, |s| s.light.exposure = 1.0));
    a.push(&mut s, true);
    // set back while the acknowledgement was lost: the server's copy of our first change
    // coming back must not win over the newer one
    a.local(develop(&a.cat, id, |s| s.light.exposure = 0.0));
    a.sync(&mut s);
    a.sync(&mut s);
    assert_eq!(a.cat.photo(id).unwrap().develop.light.exposure, 0.0);
    assert_eq!(s.catalog().photo(id).unwrap().develop.light.exposure, 0.0);
    assert!(a.out.is_empty());
}

#[test]
fn rebase_merges_with_the_server_value() {
    let mut s = server();
    let mut a = Replica::join(&s, 1);
    let add = add_photo(&mut a.cat);
    a.local(add);
    a.sync(&mut s);
    let id = a.cat.photos().next().unwrap().id;
    let mut b = Replica::join(&s, 2);
    b.local(develop(&b.cat, id, |s| s.light.contrast = 30.0));
    b.sync(&mut s);
    // A edits exposure, then reloads the server's state (a repair) before pulling B's change
    a.local(develop(&a.cat, id, |s| s.light.exposure = 1.0));
    a.repair(&s);
    let d = &a.cat.photo(id).unwrap().develop;
    assert_eq!((d.light.exposure, d.light.contrast), (1.0, 30.0), "both edits survive the reload");
    a.sync(&mut s);
    b.sync(&mut s);
    assert_eq!(shared(&a.cat), shared(&b.cat));
    assert_eq!(shared(&a.cat), shared(s.catalog()));
}
