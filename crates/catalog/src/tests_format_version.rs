//! Issue #102: catalog format versions. Older formats load and are upgraded; a newer format is
//! refused with a clear error and left untouched (never read in part, truncated or set aside).

use crate::journal::{LOG, SNAPSHOT, VERSION, encode_record};
use crate::*;

fn photo(c: &mut Catalog) -> Op {
    let id = c.alloc_photo_id();
    Op::AddPhoto { photo: Box::new(Photo::new(id, Source::File { path: format!("/p/{}.jpg", id.0) }, "p.jpg", "JPEG", 4, 3, "2026-01-01")) }
}

/// A catalog with two photos and the log line for a third op on top of it.
fn legacy_parts() -> (Catalog, String, Catalog) {
    let mut c = Catalog::new();
    for _ in 0..2 {
        let op = photo(&mut c);
        c.apply(op).unwrap();
    }
    let base = c.clone();
    let id = c.photos().next().unwrap().id;
    let op = Op::SetRating { id, rating: 4 };
    c.apply(op.clone()).unwrap();
    (base, format!("{}\n", encode_record(3, &op)), c)
}

fn snapshot_version(m: &MemStore) -> u64 {
    let v: serde_json::Value = serde_json::from_slice(&m.get(SNAPSHOT).unwrap()).unwrap();
    v["version"].as_u64().unwrap()
}

#[test]
fn v1_library_loads_and_is_upgraded() {
    let (base, log, full) = legacy_parts();
    let m = MemStore::new();
    m.set(SNAPSHOT, format!("{{\"format\":\"lightcraft-catalog\",\"version\":1,\"seq\":2,\"catalog\":{}}}\n", base.to_snapshot()).into_bytes());
    m.set(LOG, log.into_bytes());
    let (mut j, c, r) = Journal::open(Box::new(m.clone())).unwrap();
    assert_eq!(c.to_snapshot(), full.to_snapshot());
    assert_eq!((r.replayed, r.upgraded_from), (1, Some(1)));
    // rewritten in this format before anything is appended
    assert_eq!(snapshot_version(&m), u64::from(VERSION));
    assert!(m.get(LOG).unwrap().is_empty());
    assert_eq!(j.seq(), 3);
    j.append(&[Op::SetLabelName { label: ColorLabel::ALL[0], name: Some("x".into()) }]).unwrap();
    drop(j);
    let (_, _, r) = Journal::open(Box::new(m)).unwrap();
    assert_eq!((r.replayed, r.upgraded_from), (1, None));
}

/// A library saved before albums could be ordered by hand opens as it was and is rewritten in the current format.
#[test]
fn v2_library_loads_and_is_upgraded() {
    let (base, log, full) = legacy_parts();
    let m = MemStore::new();
    m.set(SNAPSHOT, format!("{{\"format\":\"lightcraft-catalog\",\"version\":2,\"seq\":2,\"catalog\":{}}}\n", base.to_snapshot()).into_bytes());
    m.set(LOG, log.into_bytes());
    let (_, c, r) = Journal::open(Box::new(m.clone())).unwrap();
    assert_eq!(c.to_snapshot(), full.to_snapshot());
    assert_eq!((r.replayed, r.upgraded_from), (1, Some(2)));
    assert_eq!(snapshot_version(&m), u64::from(VERSION));
}

/// A library saved before keywords could be listed on their own opens as it was and is rewritten
/// in the current format.
#[test]
fn v3_library_loads_and_is_upgraded() {
    let (base, log, full) = legacy_parts();
    let m = MemStore::new();
    m.set(SNAPSHOT, format!("{{\"format\":\"lightcraft-catalog\",\"version\":3,\"seq\":2,\"catalog\":{}}}\n", base.to_snapshot()).into_bytes());
    m.set(LOG, log.into_bytes());
    let (_, c, r) = Journal::open(Box::new(m.clone())).unwrap();
    assert_eq!(c.to_snapshot(), full.to_snapshot());
    assert_eq!((r.replayed, r.upgraded_from), (1, Some(3)));
    assert_eq!(snapshot_version(&m), u64::from(VERSION));
}

/// Before it merged upstream's formats 3..7, this fork wrote `SetServerPath` (now format 10) as
/// format 3: such a library opens with its server paths and is rewritten in the current format.
#[test]
fn fork_v3_library_with_server_paths_loads_and_is_upgraded() {
    let (base, _, _) = legacy_parts();
    let id = base.photos().next().unwrap().id;
    let op = Op::SetServerPath { id, path: Some("Photos/IMG_1.jpg".into()) };
    let mut full = base.clone();
    full.apply(op.clone()).unwrap();
    let m = MemStore::new();
    m.set(SNAPSHOT, format!("{{\"format\":\"lightcraft-catalog\",\"version\":3,\"seq\":2,\"catalog\":{}}}\n", base.to_snapshot()).into_bytes());
    m.set(LOG, format!("{}\n", encode_record(3, &op)).into_bytes());
    let (_, c, r) = Journal::open(Box::new(m.clone())).unwrap();
    assert_eq!(c.to_snapshot(), full.to_snapshot());
    assert_eq!((r.replayed, r.upgraded_from), (1, Some(3)));
    assert_eq!(snapshot_version(&m), u64::from(VERSION));
}

/// A format 10 library (before the Hidden album) opens with every photo shown, and is upgraded.
#[test]
fn v10_library_loads_with_nothing_hidden_and_is_upgraded() {
    let (base, _, _) = legacy_parts();
    let id = base.photos().next().unwrap().id;
    let op = Op::SetServerPath { id, path: Some("Photos/IMG_1.jpg".into()) };
    let m = MemStore::new();
    m.set(SNAPSHOT, format!("{{\"format\":\"lightcraft-catalog\",\"version\":10,\"seq\":2,\"catalog\":{}}}\n", base.to_snapshot()).into_bytes());
    m.set(LOG, format!("{}\n", encode_record(3, &op)).into_bytes());
    let (_, c, r) = Journal::open(Box::new(m.clone())).unwrap();
    assert_eq!(r.upgraded_from, Some(10));
    assert!(c.photos().all(|p| !p.hidden));
    assert_eq!(snapshot_version(&m), u64::from(VERSION));
}

/// A format 13 library (before Keep Originals Where They Are) opens with every original in the library's care, and is
/// upgraded.
#[test]
fn v13_library_loads_with_no_origins_and_is_upgraded() {
    let (base, _, _) = legacy_parts();
    let m = MemStore::new();
    m.set(SNAPSHOT, format!("{{\"format\":\"lightcraft-catalog\",\"version\":13,\"seq\":2,\"catalog\":{}}}\n", base.to_snapshot()).into_bytes());
    let (_, c, r) = Journal::open(Box::new(m.clone())).unwrap();
    assert_eq!(r.upgraded_from, Some(13));
    assert!(c.photos().all(|p| p.origin.is_none()));
    assert_eq!(snapshot_version(&m), u64::from(VERSION));
}

/// Every older format number opens and is rewritten in this one: this fork's line (formats 10 to 14: server paths,
/// Hidden, video clips, compact snapshots, origins) and upstream's (formats 10 to 13: masks on the photo, lens
/// database profiles, shared grids, Lens Blur) use the same numbers for different fields, and a library of either
/// line loads, since every field either one added is `#[serde(default)]`.
#[test]
fn every_older_format_number_loads_and_is_upgraded() {
    for v in 1..VERSION {
        let (base, log, full) = legacy_parts();
        let m = MemStore::new();
        m.set(SNAPSHOT, format!("{{\"format\":\"lightcraft-catalog\",\"version\":{v},\"seq\":2,\"catalog\":{}}}\n", base.to_snapshot()).into_bytes());
        m.set(LOG, log.into_bytes());
        let (_, c, r) = Journal::open(Box::new(m.clone())).unwrap_or_else(|e| panic!("v{v}: {e}"));
        assert_eq!(c.to_snapshot(), full.to_snapshot(), "v{v}");
        assert_eq!((r.replayed, r.upgraded_from), (1, Some(v)), "v{v}");
        assert_eq!(snapshot_version(&m), u64::from(VERSION), "v{v}");
    }
}

/// A library of upstream's formats 10 to 13 opens with its masks on the photo, its lens database profile and its
/// Lens Blur fields, and is rewritten in this format.
#[test]
fn upstream_v10_to_v13_library_keeps_its_fields_and_is_upgraded() {
    let mut c = Catalog::new();
    let op = photo(&mut c);
    let Op::AddPhoto { photo: mut p } = op else { panic!("photo() adds a photo") };
    let id = p.id;
    let mut s = lightcraft_develop::DevelopSettings::default();
    s.masks.push(lightcraft_develop::Mask { id: 1, on_photo: true, ..Default::default() });
    s.lens_blur.transition = 0.25;
    s.lens_blur.focus_mask = Some(1);
    p.develop = std::sync::Arc::new(s);
    p.embedded_lens = Some(Box::new(lightcraft_develop::EmbeddedLens { source: lightcraft_develop::LensSource::Database, ..Default::default() }));
    c.apply(Op::AddPhoto { photo: p }).unwrap();
    let snap = c.to_snapshot();
    for v in 10..=13 {
        let m = MemStore::new();
        m.set(SNAPSHOT, format!("{{\"format\":\"lightcraft-catalog\",\"version\":{v},\"seq\":1,\"catalog\":{snap}}}\n").into_bytes());
        let (_, loaded, r) = Journal::open(Box::new(m.clone())).unwrap_or_else(|e| panic!("v{v}: {e}"));
        let p = loaded.photo(id).unwrap();
        assert!(p.develop.masks.first().is_some_and(|m| m.on_photo), "v{v}: the mask stays on the photo");
        assert_eq!((p.develop.lens_blur.transition, p.develop.lens_blur.focus_mask), (0.25, Some(1)), "v{v}");
        assert_eq!(p.embedded_lens.as_ref().map(|l| l.source), Some(lightcraft_develop::LensSource::Database), "v{v}");
        assert_eq!(r.upgraded_from, Some(v));
        assert_eq!(snapshot_version(&m), u64::from(VERSION));
        assert_eq!(loaded.to_snapshot(), c.to_snapshot(), "v{v}");
    }
}

/// A library of this fork's formats 13 and 14 (compact snapshots: the settings as what differs from the header's
/// `defaults`) opens as it was and is rewritten in this format.
#[test]
fn fork_v13_and_v14_compact_library_loads_and_is_upgraded() {
    let m = MemStore::new();
    let (mut j, mut c, _) = Journal::open(Box::new(m.clone())).unwrap();
    let op = photo(&mut c);
    c.apply(op.clone()).unwrap();
    let id = c.photos().next().unwrap().id;
    let rate = Op::SetRating { id, rating: 5 };
    c.apply(rate.clone()).unwrap();
    j.append(&[op, rate]).unwrap();
    j.snapshot(&c).unwrap();
    let written = String::from_utf8(m.get(SNAPSHOT).unwrap()).unwrap();
    assert!(written.contains("\"defaults\":"), "this build writes compact snapshots");
    let current = format!("\"version\":{VERSION},");
    for v in [13, 14] {
        let older = MemStore::new();
        older.set(SNAPSHOT, written.replacen(&current, &format!("\"version\":{v},"), 1).into_bytes());
        let (_, loaded, r) = Journal::open(Box::new(older.clone())).unwrap_or_else(|e| panic!("v{v}: {e}"));
        assert_eq!(loaded.to_snapshot(), c.to_snapshot(), "v{v}");
        assert_eq!(r.upgraded_from, Some(v));
        assert_eq!(snapshot_version(&older), u64::from(VERSION));
    }
}

/// A format 11 library (the Hidden album, this fork) opens with its hidden photos still hidden.
#[test]
fn fork_v11_library_keeps_hidden_photos() {
    let (mut base, _, _) = legacy_parts();
    let id = base.photos().next().unwrap().id;
    base.apply(Op::SetHidden { id, hidden: true }).unwrap();
    let m = MemStore::new();
    m.set(SNAPSHOT, format!("{{\"format\":\"lightcraft-catalog\",\"version\":11,\"seq\":3,\"catalog\":{}}}\n", base.to_snapshot()).into_bytes());
    let (_, c, r) = Journal::open(Box::new(m.clone())).unwrap();
    assert_eq!(r.upgraded_from, Some(11));
    assert!(c.photo(id).is_some_and(|p| p.hidden));
    assert_eq!(snapshot_version(&m), u64::from(VERSION));
}

/// A photo's origin is kept through the snapshot and the log, and travels in `AddPhoto`.
#[test]
fn origin_round_trips() {
    let mut c = Catalog::new();
    let Op::AddPhoto { mut photo } = photo(&mut c) else { panic!("photo() adds a photo") };
    photo.origin = Some(Origin { kind: OriginKind::Photos, cloud: Some("ABC:001:XYZ".into()), device: 7 });
    let op = Op::AddPhoto { photo: photo.clone() };
    let back: Op = serde_json::from_str(&serde_json::to_string(&op).unwrap()).unwrap();
    assert_eq!(back, op);
    let mut d = Catalog::new();
    d.apply(op).unwrap();
    let snap = Catalog::from_snapshot(&d.to_snapshot()).unwrap();
    assert_eq!(snap.photo(photo.id).and_then(|p| p.origin.clone()), photo.origin);
}

/// Before upstream's own format 9 (the preview-only stamp), this fork wrote `SetServerPath` as format 9:
/// such a library opens with its server paths and is rewritten as format 10.
#[test]
fn fork_v9_library_with_server_paths_loads_and_is_upgraded() {
    let (base, _, _) = legacy_parts();
    let id = base.photos().next().unwrap().id;
    let op = Op::SetServerPath { id, path: Some("Photos/IMG_1.jpg".into()) };
    let mut full = base.clone();
    full.apply(op.clone()).unwrap();
    let m = MemStore::new();
    m.set(SNAPSHOT, format!("{{\"format\":\"lightcraft-catalog\",\"version\":9,\"seq\":2,\"catalog\":{}}}\n", base.to_snapshot()).into_bytes());
    m.set(LOG, format!("{}\n", encode_record(3, &op)).into_bytes());
    let (_, c, r) = Journal::open(Box::new(m.clone())).unwrap();
    assert_eq!(c.to_snapshot(), full.to_snapshot());
    assert_eq!((r.replayed, r.upgraded_from), (1, Some(9)));
    assert_eq!(snapshot_version(&m), u64::from(VERSION));
}

/// A v6 library (before preview-only photos recorded which version decided it, #662) opens with
/// an empty stamp on its preview-only photos, so they are rechecked once, and is rewritten in the
/// current format.
#[test]
fn v6_preview_only_photo_loads_unstamped_and_is_upgraded() {
    let mut c = Catalog::new();
    let id = c.alloc_photo_id();
    let mut p = Photo::new(id, Source::File { path: "/p/a.cr3".into() }, "a.cr3", "CR3", 160, 120, "2026-01-01");
    p.kind = MediaKind::Raw;
    p.preview_only = Some("Cr3 files are not decoded yet".into());
    c.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    let snap = c.to_snapshot();
    assert!(!snap.contains("preview_checked"), "an empty stamp isn't written");
    let m = MemStore::new();
    m.set(SNAPSHOT, format!("{{\"format\":\"lightcraft-catalog\",\"version\":6,\"seq\":1,\"catalog\":{snap}}}\n").into_bytes());
    let (_, loaded, r) = Journal::open(Box::new(m.clone())).unwrap();
    let p = loaded.photo(id).unwrap();
    assert_eq!(p.preview_only.as_deref(), Some("Cr3 files are not decoded yet"));
    assert_eq!(p.preview_checked, "");
    assert_eq!(r.upgraded_from, Some(6));
    assert_eq!(snapshot_version(&m), u64::from(VERSION));
}

#[test]
fn versionless_log_only_library_is_upgraded() {
    let mut c = Catalog::new();
    let op = photo(&mut c);
    c.apply(op.clone()).unwrap();
    let m = MemStore::new();
    m.set(LOG, format!("{}\n", encode_record(1, &op)).into_bytes());
    let (_, loaded, r) = Journal::open(Box::new(m.clone())).unwrap();
    assert_eq!(loaded.to_snapshot(), c.to_snapshot());
    assert_eq!(r.upgraded_from, Some(0));
    assert_eq!(snapshot_version(&m), u64::from(VERSION));
}

/// A new library gets a snapshot in this format at once, so an older build never sees a bare log
/// it would read in part.
#[test]
fn new_library_is_written_in_this_format() {
    let m = MemStore::new();
    let (_, _, r) = Journal::open(Box::new(m.clone())).unwrap();
    assert!(r.created);
    assert_eq!(r.upgraded_from, None);
    assert_eq!(snapshot_version(&m), u64::from(VERSION));
}

fn assert_refused_untouched(m: &MemStore) {
    let before = m.files.lock().unwrap().clone();
    let e = Journal::open(Box::new(m.clone())).err().expect("refused");
    assert!(matches!(e, CatalogError::Newer(_)), "{e:?}");
    let msg = e.to_string();
    assert!(msg.contains("newer version of LightCraft") && msg.contains("left unchanged"), "{msg}");
    assert_eq!(*m.files.lock().unwrap(), before, "nothing modified, nothing renamed");
}

#[test]
fn newer_snapshot_is_refused_untouched() {
    let (base, log, _) = legacy_parts();
    let m = MemStore::new();
    m.set(
        SNAPSHOT,
        format!("{{\"format\":\"lightcraft-catalog\",\"version\":{},\"seq\":2,\"catalog\":{}}}\n", VERSION + 1, base.to_snapshot()).into_bytes(),
    );
    m.set(LOG, log.into_bytes());
    assert_refused_untouched(&m);
    // even when its catalog doesn't parse as this version's
    let m = MemStore::new();
    m.set(
        SNAPSHOT,
        format!("{{\"format\":\"lightcraft-catalog\",\"version\":{},\"seq\":2,\"catalog\":{{\"photos\":7}}}}\n", VERSION + 1).into_bytes(),
    );
    assert_refused_untouched(&m);
    let e = Journal::open(Box::new(m)).err().unwrap().to_string();
    assert!(e.contains(&format!("v{}", VERSION + 1)), "{e}");
}

/// The header is read from the bytes before `"catalog":`; a file laid out otherwise is still read whole, and the
/// refusals are the same either way.
#[test]
fn snapshot_header_is_read_from_the_prefix_or_the_whole_file() {
    let (base, _, _) = legacy_parts();
    let cat = base.to_snapshot();
    let head = format!("\"format\":\"lightcraft-catalog\",\"version\":{VERSION},\"seq\":2");
    // catalog first, header after: the prefix holds no header, the whole file does
    let m = MemStore::new();
    m.set(SNAPSHOT, format!("{{\"catalog\":{cat},{head}}}\n").into_bytes());
    let (_, c, r) = Journal::open(Box::new(m)).unwrap();
    assert_eq!((c.to_snapshot(), r.snapshot_seq), (cat.clone(), 2));
    // spaces around the header's keys
    let m = MemStore::new();
    m.set(SNAPSHOT, format!("{{ {head} , \"catalog\" : {cat} }}\n").into_bytes());
    assert_eq!(Journal::open(Box::new(m)).unwrap().1.to_snapshot(), cat);
    // another format, refused from the prefix alone
    let m = MemStore::new();
    m.set(SNAPSHOT, format!("{{\"format\":\"other\",\"version\":1,\"seq\":0,\"catalog\":{cat}}}\n").into_bytes());
    let e = Journal::open(Box::new(m)).err().unwrap();
    assert!(matches!(e, CatalogError::Corrupt(_)) && e.to_string().contains("not a LightCraft catalog"), "{e}");
    // a `"catalog":` inside the format string cuts the prefix short: read whole, refused the same
    let m = MemStore::new();
    m.set(SNAPSHOT, format!("{{\"format\":\"a\\\"catalog\\\":b\",\"version\":1,\"seq\":0,\"catalog\":{cat}}}\n").into_bytes());
    let e = Journal::open(Box::new(m)).err().unwrap();
    assert!(matches!(e, CatalogError::Corrupt(_)) && e.to_string().contains("not a LightCraft catalog"), "{e}");
    // no header at all, or no JSON: corrupt
    for bytes in [b"{\"catalog\":{}}\n".to_vec(), b"\"catalog\":".to_vec(), Vec::new(), b"{\"format\":\"lightcraft-catalog\",".to_vec()] {
        let m = MemStore::new();
        m.set(SNAPSHOT, bytes);
        let e = Journal::open(Box::new(m)).err().unwrap();
        assert!(matches!(e, CatalogError::Corrupt(_)), "{e:?}");
    }
}

/// A log record with a valid CRC whose op this version doesn't know: refused, not taken for a
/// torn tail (truncated) or damage (renamed) — wherever it is in the log.
#[test]
fn unknown_op_in_the_log_is_refused_untouched() {
    let (base, log, _) = legacy_parts();
    let future = |seq: u64| {
        let body = r#"{"op":"fromTheFuture","id":1,"what":"something new"}"#;
        format!("{{\"seq\":{seq},\"crc\":{},\"op\":{body}}}\n", crc32fast::hash(body.as_bytes()))
    };
    let snap = format!("{{\"format\":\"lightcraft-catalog\",\"version\":{VERSION},\"seq\":2,\"catalog\":{}}}\n", base.to_snapshot());
    let id = base.photos().next().unwrap().id;
    let after = format!("{}\n", encode_record(5, &Op::SetRating { id, rating: 1 }));
    for log in [format!("{log}{}", future(4)), format!("{log}{}{after}", future(4)), format!("{}{log}", future(3))] {
        let m = MemStore::new();
        m.set(SNAPSHOT, snap.clone().into_bytes());
        m.set(LOG, log.into_bytes());
        assert_refused_untouched(&m);
    }
    // a known op with a field of an unknown shape is "newer" too
    let body = r#"{"op":"setRating","id":1,"rating":"five stars"}"#;
    let m = MemStore::new();
    m.set(SNAPSHOT, snap.into_bytes());
    m.set(LOG, format!("{log}{{\"seq\":4,\"crc\":{},\"op\":{body}}}\n", crc32fast::hash(body.as_bytes())).into_bytes());
    assert_refused_untouched(&m);
}

/// A CRC mismatch is still damage / a torn tail (not "newer").
#[test]
fn bad_crc_is_still_a_torn_tail() {
    let (base, log, _) = legacy_parts();
    let m = MemStore::new();
    m.set(
        SNAPSHOT,
        format!("{{\"format\":\"lightcraft-catalog\",\"version\":{VERSION},\"seq\":2,\"catalog\":{}}}\n", base.to_snapshot()).into_bytes(),
    );
    let body = r#"{"op":"fromTheFuture"}"#;
    m.set(LOG, format!("{log}{{\"seq\":4,\"crc\":1,\"op\":{body}}}\n").into_bytes());
    let (_, _, r) = Journal::open(Box::new(m)).unwrap();
    assert!(r.torn_bytes > 0 && r.damaged.is_none(), "{r:?}");
}

/// Guard for the version rule (see `journal` → *Format versions*): adding an [`Op`] variant breaks
/// this match. When it does, bump `journal::VERSION`, add a row to the format table, and add the
/// variant here under the new version. A version that only adds serialized fields raises
/// `NEWEST_FIELDS` instead.
#[test]
fn op_variants_are_versioned() {
    fn since(op: &Op) -> u32 {
        match op {
            Op::AddPhoto { .. }
            | Op::RemovePhoto { .. }
            | Op::SetRating { .. }
            | Op::SetFlag { .. }
            | Op::SetLabel { .. }
            | Op::SetDevelop { .. }
            | Op::SetMeta { .. }
            | Op::SetDeleted { .. }
            | Op::SetLocal { .. }
            | Op::SetVersions { .. }
            | Op::SetHistory { .. }
            | Op::PushHistory { .. }
            | Op::AddAlbum { .. }
            | Op::RemoveAlbum { .. }
            | Op::RenameAlbum { .. }
            | Op::MoveAlbum { .. }
            | Op::SetAlbumPhotos { .. }
            | Op::SetAlbumCover { .. }
            | Op::SetAlbumRules { .. }
            | Op::AddStack { .. }
            | Op::RemoveStack { .. }
            | Op::SetStack { .. }
            | Op::SetCaptured { .. }
            | Op::SetAnalysis { .. }
            | Op::SetFile { .. }
            | Op::Relink { .. }
            | Op::SetContent { .. }
            | Op::SetLabelName { .. }
            | Op::Batch { .. } => 1,
            Op::SetBrowsed { .. } => 2,
            Op::SetAlbumOrder { .. } => 3,
            Op::SetEmbeddedLens { .. } => 4,
            Op::SetKeyword { .. } => 5,
            Op::SetFolderRecord { .. } => 6,
            Op::SetAsShotWb { .. } => 7,
            Op::SetHistoryCursor { .. } => 8,
            Op::SetServerPath { .. } => 10,
            Op::SetHidden { .. } => 11,
        }
    }
    /// v15 merged upstream's formats 10 to 13 into this fork, fields only: `Mask.on_photo`, `EmbeddedLens.source`,
    /// `Catalog.grids` / `SegMask.grid` and upstream's Lens Blur fields. Before it, v12 carried `DevelopSettings.video`,
    /// a clip's trim and poster frame, v13 wrote the snapshot's settings as what differs from its `defaults`
    /// (`crate::compact`), v14 carried `Photo.origin`, v16 `ExposureLayer.text` / `behind` and v17 `ExposureLayer.shape` /
    /// `drawing`, v18 `LayerText`'s type options, v19 `ExposureLayer.style` and v20 `ExposureLayer.subject` and `name`: none has an op of its own
    /// (v9 was upstream's `Photo.preview_checked`).
    const NEWEST_FIELDS: u32 = 20;
    let newest = since(&Op::SetHidden { id: PhotoId(1), hidden: true });
    assert_eq!(newest.max(NEWEST_FIELDS), VERSION, "the newest op's or fields' version must be the current format version");
    /// The newest version whose ops or fields go over the sync protocol (v20's layer subjects and names, v19's layer styles, v18's type options, v17's shape and drawing layers, v16's text and behind-the-subject layers; v13
    /// is only how `catalog.snap` is written, and so is v15's shared grid table).
    const NEWEST_SYNCED_FIELDS: u32 = 20;
    assert_eq!(newest.max(NEWEST_SYNCED_FIELDS), crate::journal::SYNC_VERSION, "a new op or synced field raises SYNC_VERSION");
}
