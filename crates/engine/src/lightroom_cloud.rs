//! A Lightroom (cloud) catalog's albums, read for [`crate::lightroom_albums`].
//!
//! The Lightroom desktop app keeps a copy of the whole cloud catalog on the computer, even of
//! photos whose originals are only in the cloud: `Lightroom Library.lrlibrary/<catalog id>/Managed
//! Catalog.mcat`. It is SQLite (read by [`crate::lightroom_sqlite`], never written): table `docs`
//! lists the documents (`fullDocId`, `type`, `subtype`, `deleted`, `winningRevSequence`) and table
//! `revs` holds each revision's `content`, a MessagePack map. The documents read here:
//! - `album` / `collection`: an album (`name`, `parent.id`); `album` / `collection_set`: a folder;
//! - `album_asset`: a photo in an album (`album.id`, `asset.id`);
//! - `asset` / `image` or `video`: a photo (`importSource.fileName`, `fileSize`, `originalWidth`,
//!   `originalHeight`, `captureDate`, `xmp.exif.DateTimeOriginal`).
//!
//! Learnt from a user's own catalog file; nothing of Adobe's app was looked into.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use serde_json::Value;

use crate::lightroom_albums::{AlbumSource, PhotoIdentity, SourceAlbum, second};
use crate::lightroom_sqlite::{LiveTable, Value as SqlValue};

/// Whether `path` is a Lightroom (cloud) catalog (`*.mcat`) rather than a Classic `.lrcat`.
pub fn is_cloud(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("mcat"))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Album,
    Folder,
    Member,
    Photo,
}

fn columns(table: &LiveTable, wanted: &[&str]) -> Result<(usize, Vec<usize>), String> {
    let names = table.column_names.as_ref().ok_or_else(|| format!("cannot read the {} table's columns", table.name))?;
    let at = wanted
        .iter()
        .map(|w| names.iter().position(|n| n == w).ok_or_else(|| format!("not a Lightroom catalog: {} has no {w}", table.name)))
        .collect::<Result<_, _>>()?;
    Ok((names.len(), at))
}

fn text(values: &[SqlValue], at: usize) -> &str {
    match values.get(at) {
        Some(SqlValue::Text(s)) => s,
        _ => "",
    }
}

fn int(values: &[SqlValue], at: usize) -> i64 {
    match values.get(at) {
        Some(SqlValue::Integer(n)) => *n,
        _ => 0,
    }
}

fn str_at<'a>(doc: &'a Value, pointer: &str) -> &'a str {
    doc.pointer(pointer).and_then(Value::as_str).unwrap_or("")
}

/// The albums and folders of the catalog at `path`, each album's photos, and (with `photos`) what
/// identifies those photos' files; `total` / `done` count the documents read.
pub fn read(path: &Path, photos: bool, cancel: &AtomicBool, total: &AtomicUsize, done: &AtomicUsize) -> Result<AlbumSource, String> {
    let (db, unchanged) = crate::lightroom_catalog::open_database(path)?;
    let tables = db.live_tables()?;
    let table = |name: &str| tables.iter().find(|t| t.name == name).ok_or_else(|| format!("not a Lightroom catalog: no {name} table"));
    let (docs, revs) = (table("docs")?, table("revs")?);
    let (doc_columns, at) = columns(docs, &["fullDocId", "winningRevSequence", "deleted", "type", "subtype"])?;
    let &[id_at, rev_at, deleted_at, type_at, subtype_at] = at.as_slice() else { return Err("docs columns".into()) };
    let (rev_columns, at) = columns(revs, &["content", "deleted"])?;
    let &[content_at, rev_deleted_at] = at.as_slice() else { return Err("revs columns".into()) };

    let mut wanted = Vec::new();
    let mut photo_revs = HashMap::new();
    for row in db.read_table(docs.rootpage, doc_columns)? {
        if int(&row.values, deleted_at) != 0 {
            continue;
        }
        let kind = match (text(&row.values, type_at), text(&row.values, subtype_at)) {
            ("album", "collection") => Kind::Album,
            ("album", "collection_set") => Kind::Folder,
            ("album_asset", _) => Kind::Member,
            ("asset", "image" | "video") => Kind::Photo,
            _ => continue,
        };
        let id = text(&row.values, id_at).to_string();
        let rev = int(&row.values, rev_at);
        if kind == Kind::Photo {
            photo_revs.insert(id, rev);
        } else {
            wanted.push((rev, id, kind));
        }
    }
    // the revisions in file order, so neighbouring rows are read together
    wanted.sort_unstable_by_key(|w| w.0);
    let content = |rev: i64| -> Result<Option<Value>, String> {
        let Some(row) = db.get_row(revs.rootpage, rev, rev_columns)? else { return Ok(None) };
        if int(&row.values, rev_deleted_at) != 0 {
            return Ok(None);
        }
        // (stored with TEXT type; MessagePack isn't UTF-8, so the reader hands it back as bytes)
        match row.values.get(content_at) {
            Some(SqlValue::Blob(bytes)) => crate::msgpack::decode(bytes).map(Some),
            Some(SqlValue::Text(text)) => crate::msgpack::decode(text.as_bytes()).map(Some),
            _ => Ok(None),
        }
    };
    let step = |n: usize| -> Result<(), String> {
        done.fetch_add(n, Ordering::Relaxed);
        if cancel.load(Ordering::Relaxed) { Err("Lightroom import cancelled".into()) } else { Ok(()) }
    };
    total.store(wanted.len(), Ordering::Relaxed);
    done.store(0, Ordering::Relaxed);

    let mut out = AlbumSource { source: path.to_string_lossy().into(), ..AlbumSource::default() };
    for (rev, id, kind) in wanted {
        step(1)?;
        let doc = match content(rev) {
            Ok(Some(doc)) => doc,
            Ok(None) => continue,
            Err(e) => {
                warn(&mut out, format!("document {id}: {e}"));
                continue;
            }
        };
        match kind {
            Kind::Album | Kind::Folder => {
                let parent = Some(str_at(&doc, "/parent/id")).filter(|p| !p.is_empty()).map(str::to_string);
                out.albums.push(SourceAlbum { key: id, name: str_at(&doc, "/name").to_string(), parent, folder: kind == Kind::Folder, smart: None });
            }
            Kind::Member => {
                let (album, photo) = (str_at(&doc, "/album/id"), str_at(&doc, "/asset/id"));
                if !album.is_empty() && photo_revs.contains_key(photo) {
                    out.members.entry(album.to_string()).or_default().push(photo.to_string());
                }
            }
            Kind::Photo => {}
        }
    }
    // a member of an album that isn't there (deleted, or in a shared space) is dropped
    let keys: HashSet<String> = out.albums.iter().map(|a| a.key.clone()).collect();
    out.members.retain(|album, _| keys.contains(album));
    // a folder in a folder that isn't there goes to the top
    for album in &mut out.albums {
        if album.parent.as_deref().is_some_and(|p| !keys.contains(p)) {
            album.parent = None;
        }
    }

    if photos {
        let mut needed: Vec<(i64, String)> = out
            .members
            .values()
            .flatten()
            .collect::<HashSet<_>>()
            .into_iter()
            .filter_map(|id| photo_revs.get(id).map(|rev| (*rev, id.clone())))
            .collect();
        needed.sort_unstable();
        total.fetch_add(needed.len(), Ordering::Relaxed);
        for (rev, id) in needed {
            step(1)?;
            match content(rev) {
                Ok(Some(doc)) => {
                    out.photos.insert(id, identity(&doc));
                }
                Ok(None) => {}
                Err(e) => warn(&mut out, format!("photo {id}: {e}")),
            }
        }
    }
    unchanged(&db)?;
    Ok(out)
}

/// A document that can't be read is skipped with a warning.
// ponytail: one line per bad document, capped at 50; a count of the rest if real catalogs need it
fn warn(out: &mut AlbumSource, why: String) {
    if out.warnings.len() < 50 {
        out.warnings.push(why);
    }
}

/// What a photo document says of its original file.
fn identity(doc: &Value) -> PhotoIdentity {
    let number = |pointer: &str| doc.pointer(pointer).and_then(Value::as_u64).filter(|n| *n > 0);
    let dimension = |pointer: &str| number(pointer).and_then(|n| u32::try_from(n).ok());
    PhotoIdentity {
        name: str_at(doc, "/importSource/fileName").to_string(),
        size: number("/importSource/fileSize"),
        // the camera's wall-clock time first, as LightCraft keeps it
        captured: second(str_at(doc, "/xmp/exif/DateTimeOriginal")).or_else(|| second(str_at(doc, "/captureDate"))),
        dims: dimension("/importSource/originalWidth").zip(dimension("/importSource/originalHeight")),
        path: None,
    }
}

/// A report of the catalog at `path` without reading its photos: `{kind, albums, folders, photos,
/// warnings}`.
pub fn inspect(path: &Path, cancel: &AtomicBool, total: &AtomicUsize, done: &AtomicUsize) -> Result<Value, String> {
    let source = read(path, false, cancel, total, done)?;
    let photos: HashSet<_> = source.members.values().flatten().collect();
    Ok(serde_json::json!({
        "kind": "cloud",
        "albums": source.albums.iter().filter(|a| !a.folder).count(),
        "folders": source.albums.iter().filter(|a| a.folder).count(),
        "photos": photos.len(),
        "warnings": source.warnings,
    }))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::lightroom_sqlite::Value as Sql;
    use serde_json::json;

    /// A Lightroom (cloud) catalog in `dir`: folder "Trips" holding album "Japan" (photos `p1`, a
    /// photo `p9` that isn't in the catalog), album "Empty", album "Video" (`p2`), a deleted album, a
    /// member of a shared-space album and a preset, each document one revision.
    pub(crate) fn mcat(dir: &Path) -> std::path::PathBuf {
        let docs = [
            ("fold", "album", "collection_set", 0, json!({"type": "album", "subtype": "collection_set", "name": "Trips"})),
            ("japan", "album", "collection", 0, json!({"type": "album", "subtype": "collection", "name": "Japan", "parent": {"id": "fold"}})),
            ("empty", "album", "collection", 0, json!({"type": "album", "subtype": "collection", "name": "Empty"})),
            ("video", "album", "collection", 0, json!({"type": "album", "name": "Video"})),
            ("gone", "album", "collection", 1, json!({"type": "album", "name": "Gone"})),
            ("m1", "album_asset", "", 0, json!({"album": {"id": "japan"}, "asset": {"id": "p1"}})),
            ("m2", "album_asset", "", 0, json!({"album": {"id": "japan"}, "asset": {"id": "p9"}})),
            ("m3", "album_asset", "", 0, json!({"album": {"id": "video"}, "asset": {"id": "p2"}})),
            ("m4", "album_asset", "", 0, json!({"album": {"id": "shared"}, "asset": {"id": "p1"}})),
            (
                "p1",
                "asset",
                "image",
                0,
                json!({"captureDate": "2024-06-25T21:25:00Z", "importSource": {"fileName": "IMG_0001.HEIC",
                "fileSize": 2_000_000, "originalWidth": 4032, "originalHeight": 3024}, "xmp": {"exif": {"DateTimeOriginal": "2024-06-25T22:25:00.670+01:00"}}}),
            ),
            ("p2", "asset", "video", 0, json!({"captureDate": "2024-06-26T10:00:00", "importSource": {"fileName": "IMG_0002.MOV", "fileSize": 9}})),
            ("preset", "asset", "preset", 0, json!({"name": "Warm"})),
        ];
        let doc_rows = docs
            .iter()
            .enumerate()
            .map(|(i, (id, kind, subtype, deleted, _))| {
                vec![
                    Sql::Null,
                    Sql::Text((*id).into()),
                    Sql::Integer(i as i64 + 1),
                    Sql::Integer(*deleted),
                    Sql::Text((*kind).into()),
                    Sql::Text((*subtype).into()),
                ]
            })
            .collect();
        let rev_rows = docs
            .iter()
            .enumerate()
            .map(|(i, (.., doc))| vec![Sql::Null, Sql::Integer(i as i64 + 1), Sql::Integer(0), Sql::Blob(crate::msgpack::encode(doc))])
            .collect();
        let bytes = crate::lightroom_catalog::tests::sqlite_file(&[
            (
                "docs",
                "localDocId INTEGER PRIMARY KEY, fullDocId TEXT UNIQUE NOT NULL, winningRevSequence INTEGER, deleted BOOLEAN, type TEXT, subtype TEXT",
                doc_rows,
            ),
            ("revs", "sequence INTEGER PRIMARY KEY AUTOINCREMENT, localDocId INTEGER NOT NULL, deleted BOOLEAN, content BLOB", rev_rows),
        ]);
        let path = dir.join("Managed Catalog.mcat");
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("lc-engine-mcat-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn read_all(path: &Path) -> AlbumSource {
        let (cancel, total, done) = Default::default();
        read(path, true, &cancel, &total, &done).unwrap()
    }

    #[test]
    fn albums_folders_members_and_photo_identities_are_read() {
        let dir = temp_dir("read");
        let source = read_all(&mcat(&dir));
        let album = |key: &str| source.albums.iter().find(|a| a.key == key).cloned();
        assert_eq!(album("fold"), Some(SourceAlbum { key: "fold".into(), name: "Trips".into(), parent: None, folder: true, smart: None }));
        assert_eq!(album("japan").and_then(|a| a.parent), Some("fold".into()));
        assert!(album("gone").is_none(), "deleted documents are left out");
        assert_eq!(source.albums.len(), 4);
        assert_eq!(source.members.get("japan"), Some(&vec!["p1".to_string()]), "a photo not in the catalog is dropped");
        assert_eq!(source.members.get("video"), Some(&vec!["p2".to_string()]));
        assert!(!source.members.contains_key("shared"), "albums not in the catalog are left out");
        let p1 = source.photos.get("p1").unwrap();
        assert_eq!(
            p1,
            &PhotoIdentity {
                name: "IMG_0001.HEIC".into(),
                size: Some(2_000_000),
                captured: Some("20240625222500".into()),
                dims: Some((4032, 3024)),
                path: None
            }
        );
        assert_eq!(source.photos.get("p2").and_then(|p| p.captured.clone()), Some("20240626100000".into()));
        assert_eq!(source.photos.len(), 2);
        let (cancel, total, done) = Default::default();
        let report = inspect(&dir.join("Managed Catalog.mcat"), &cancel, &total, &done).unwrap();
        assert_eq!(report, json!({"kind": "cloud", "albums": 3, "folders": 1, "photos": 2, "warnings": []}));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Albums only: the folder and albums are made in one undo step holding the photos the library
    /// has; a photo it lacks is counted, not added. Importing again adds nothing.
    #[test]
    fn albums_only_import_adds_the_photos_the_library_has() {
        use lightcraft_catalog::{Op, Photo, Source};
        let dir = temp_dir("import");
        let path = mcat(&dir);
        let mut s = crate::Session::new();
        let id = s.catalog.alloc_photo_id();
        let mut photo =
            Photo::new(id, Source::File { path: "/phone/IMG_0001.HEIC".into() }, "IMG_0001.HEIC", "HEIC", 4032, 3024, "2026-01-01T00:00:00");
        photo.file_size = 2_000_000;
        photo.captured = Some("2024-06-25T22:25:00".into());
        s.commit("add", Op::AddPhoto { photo: Box::new(photo) }).unwrap();
        let photos = s.catalog.len();

        let report = crate::lightroom_catalog::import(&mut s, &path, false, false).unwrap();
        assert_eq!((report["albums"].as_u64(), report["folders"].as_u64(), report["added"].as_u64()), (Some(3), Some(1), Some(1)));
        assert_eq!((report["matched"].as_u64(), report["unmatched"].as_u64(), report["ambiguous"].as_u64()), (Some(1), Some(1), Some(0)));
        assert_eq!(s.catalog.len(), photos, "no photo is added");
        let japan = s.catalog.albums().find(|a| a.name == "Japan").unwrap().clone();
        let trips = s.catalog.albums().find(|a| a.name == "Trips").unwrap();
        assert!(trips.folder);
        assert_eq!(japan.parent, Some(trips.id));
        assert_eq!(japan.photos, vec![id]);

        let again = crate::lightroom_catalog::import(&mut s, &path, false, false).unwrap();
        assert_eq!((again["albums"].as_u64(), again["folders"].as_u64(), again["added"].as_u64()), (Some(0), Some(0), Some(0)));
        assert_eq!(s.catalog.albums().count(), 4, "the albums are found again by name");

        s.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(s.catalog.albums().count(), 0, "one undo step");
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod scratch_debug {
    use crate::lightroom_sqlite::Value as Sql;
    use serde_json::Value;
    use std::collections::BTreeSet;
    use std::sync::atomic::AtomicUsize;

    fn keys(v: &Value, prefix: &str, depth: usize, out: &mut BTreeSet<String>) {
        if depth > 5 {
            return;
        }
        match v {
            Value::Object(m) => {
                for (k, v) in m {
                    let p = format!("{prefix}/{k}");
                    out.insert(p.clone());
                    keys(v, &p, depth + 1, out);
                }
            }
            Value::Array(a) => {
                for v in a.iter().take(2) {
                    keys(v, &format!("{prefix}[]"), depth + 1, out);
                }
            }
            _ => {}
        }
    }

    #[test]
    #[ignore]
    fn real_catalog() {
        let path = std::path::PathBuf::from(std::env::var("LC_MCAT").unwrap());
        let t = std::time::Instant::now();
        let report = super::inspect(&path, &Default::default(), &AtomicUsize::new(0), &AtomicUsize::new(0));
        eprintln!("inspect {:?}: {:?}", t.elapsed(), report.map(|r| r.to_string().chars().take(400).collect::<String>()));
        let (db, _) = crate::lightroom_catalog::open_database(&path).unwrap();
        let tables = db.live_tables().unwrap();
        let docs = tables.iter().find(|t| t.name == "docs").unwrap();
        let revs = tables.iter().find(|t| t.name == "revs").unwrap();
        let mut seen = std::collections::HashMap::<String, usize>::new();
        for row in db.read_table(docs.rootpage, 10).unwrap() {
            let (Some(Sql::Text(ty)), Some(Sql::Text(sub))) = (row.values.get(8), row.values.get(9)) else { continue };
            if !["asset", "version", "album_asset"].contains(&ty.as_str()) {
                continue;
            }
            let n = seen.entry(format!("{ty}/{sub}")).or_default();
            *n += 1;
            if *n > 1 {
                continue;
            }
            let Some(Sql::Integer(rev)) = row.values.get(2) else { continue };
            let Some(r) = db.get_row(revs.rootpage, *rev, 7).unwrap() else { continue };
            let bytes = match r.values.get(3) { Some(Sql::Blob(b)) => b.clone(), Some(Sql::Text(t)) => t.as_bytes().to_vec(), _ => continue };
            let doc = crate::msgpack::decode(&bytes).unwrap();
            let mut ks = BTreeSet::new();
            keys(&doc, "", 0, &mut ks);
            eprintln!("--- {ty}/{sub}: {} keys\n{}", ks.len(), ks.into_iter().collect::<Vec<_>>().join("\n"));
        }
    }
}
