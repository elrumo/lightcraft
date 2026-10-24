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
//!   `originalHeight`, `captureDate`, `xmp.exif.DateTimeOriginal`);
//! - `version` / `edit`: one edit of a photo (`asset.id`, `userUpdated`, `develop.xmpCameraRaw`: the
//!   XMP packet itself, or `{sha256}` naming the file `settings/<sha256>` beside the catalog).
//!
//! Learnt from a user's own catalog file; nothing of Adobe's app was looked into.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};

use serde_json::Value;

use crate::lightroom_albums::{AlbumSource, DevelopSource, PhotoIdentity, SourceAlbum, second};
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
    Edit,
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

/// What the reader is doing now, for [`crate::lightroom_job::LightroomProgress::stage`]: each is set
/// as the code path starts it.
pub mod stage {
    /// Reading the catalog's write-ahead log and table list.
    pub const OPENING: u8 = 1;
    /// Walking the table of documents (no count: it is one pass over the file).
    pub const LISTING: u8 = 2;
    /// Reading the albums, folders and which photos each holds (counted).
    pub const ALBUMS: u8 = 3;
    /// Reading the files' names, sizes and times of the photos albums hold (counted).
    pub const PHOTOS: u8 = 4;
    /// Matching those photos with the library's (set by the job).
    pub const MATCHING: u8 = 5;
}

/// The albums and folders of the catalog at `path`, each album's photos, and (with `photos`) what
/// identifies those photos' files; `total` / `done` count the documents read.
pub fn read(path: &Path, photos: bool, cancel: &AtomicBool, total: &AtomicUsize, done: &AtomicUsize) -> Result<AlbumSource, String> {
    read_staged(path, photos, cancel, total, done, &AtomicU8::new(0))
}

/// [`read`], saying in `progress_stage` what it is doing ([`stage`]).
pub fn read_staged(
    path: &Path,
    photos: bool,
    cancel: &AtomicBool,
    total: &AtomicUsize,
    done: &AtomicUsize,
    progress_stage: &AtomicU8,
) -> Result<AlbumSource, String> {
    progress_stage.store(stage::OPENING, Ordering::Relaxed);
    let (db, unchanged) = crate::lightroom_catalog::open_database(path)?;
    let tables = db.live_tables()?;
    let table = |name: &str| tables.iter().find(|t| t.name == name).ok_or_else(|| format!("not a Lightroom catalog: no {name} table"));
    let (docs, revs) = (table("docs")?, table("revs")?);
    let (doc_columns, at) = columns(docs, &["fullDocId", "winningRevSequence", "deleted", "type", "subtype"])?;
    let &[id_at, rev_at, deleted_at, type_at, subtype_at] = at.as_slice() else { return Err("docs columns".into()) };
    let (rev_columns, at) = columns(revs, &["content", "deleted"])?;
    let &[content_at, rev_deleted_at] = at.as_slice() else { return Err("revs columns".into()) };

    progress_stage.store(stage::LISTING, Ordering::Relaxed);
    let mut wanted = Vec::new();
    let mut photo_revs = HashMap::new();
    let mut edit_revs = Vec::new();
    for row in db.read_table(docs.rootpage, doc_columns)? {
        if int(&row.values, deleted_at) != 0 {
            continue;
        }
        let kind = match (text(&row.values, type_at), text(&row.values, subtype_at)) {
            ("album", "collection") => Kind::Album,
            ("album", "collection_set") => Kind::Folder,
            ("album_asset", _) => Kind::Member,
            ("asset", "image" | "video") => Kind::Photo,
            ("version", "edit") => Kind::Edit,
            _ => continue,
        };
        let id = text(&row.values, id_at).to_string();
        let rev = int(&row.values, rev_at);
        if kind == Kind::Photo {
            photo_revs.insert(id, rev);
        } else if kind == Kind::Edit {
            edit_revs.push(rev);
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
    progress_stage.store(stage::ALBUMS, Ordering::Relaxed);
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
            Kind::Photo | Kind::Edit => {}
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
        progress_stage.store(stage::PHOTOS, Ordering::Relaxed);
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
    if photos {
        // each of those photos' latest edit (a photo is edited again and again: every edit is a document)
        edit_revs.sort_unstable();
        total.fetch_add(edit_revs.len(), Ordering::Relaxed);
        let mut latest: HashMap<String, (String, i64, DevelopSource)> = HashMap::new();
        for rev in edit_revs {
            step(1)?;
            let Ok(Some(doc)) = content(rev) else { continue };
            let asset = str_at(&doc, "/asset/id");
            if !out.photos.contains_key(asset) {
                continue;
            }
            let develop = match doc.pointer("/develop/xmpCameraRaw") {
                Some(Value::String(packet)) if !packet.is_empty() => DevelopSource::Inline(packet.clone()),
                Some(Value::Object(o)) => match o.get("sha256").and_then(Value::as_str) {
                    Some(hash) => DevelopSource::Hash(hash.to_string()),
                    None => continue,
                },
                _ => continue,
            };
            let updated = Some(str_at(&doc, "/userUpdated")).filter(|u| !u.is_empty()).unwrap_or_else(|| str_at(&doc, "/develop/userUpdated"));
            if latest.get(asset).is_none_or(|(u, r, _)| (updated, rev) > (u.as_str(), *r)) {
                latest.insert(asset.to_string(), (updated.to_string(), rev, develop));
            }
        }
        out.edits = latest.into_iter().map(|(asset, (_, _, develop))| (asset, develop)).collect();
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
        orientation: orientation(str_at(doc, "/xmp/tiff/Orientation")),
    }
}

/// The EXIF orientation (1 to 8) of the words Lightroom keeps it in: where the stored image's first row
/// and first column are (`right top`: its top is on the right, turned a quarter clockwise).
fn orientation(words: &str) -> Option<u16> {
    Some(match words.trim().to_lowercase().as_str() {
        "top left" => 1,
        "top right" => 2,
        "bottom right" => 3,
        "bottom left" => 4,
        "left top" => 5,
        "right top" => 6,
        "right bottom" => 7,
        "left bottom" => 8,
        _ => return None,
    })
}

/// A report of the catalog at `path` without reading its photos: `{kind, albums, folders, photos,
/// warnings}`.
pub fn inspect(path: &Path, cancel: &AtomicBool, total: &AtomicUsize, done: &AtomicUsize) -> Result<Value, String> {
    inspect_staged(path, cancel, total, done, &AtomicU8::new(0))
}

/// [`inspect`], saying in `progress_stage` what it is doing ([`stage`]).
pub fn inspect_staged(path: &Path, cancel: &AtomicBool, total: &AtomicUsize, done: &AtomicUsize, progress_stage: &AtomicU8) -> Result<Value, String> {
    let source = read_staged(path, false, cancel, total, done, progress_stage)?;
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

    /// A Camera Raw settings packet, as Lightroom keeps a photo's edit.
    pub(crate) fn settings_packet(exposure: &str) -> String {
        format!(
            "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
             <rdf:Description rdf:about=\"\" xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" crs:Version=\"17.1\" \
             crs:HasSettings=\"True\" crs:Exposure2012=\"{exposure}\" crs:HasCrop=\"True\" crs:CropLeft=\"0.1\" crs:CropTop=\"0.1\" \
             crs:CropRight=\"0.9\" crs:CropBottom=\"0.9\" crs:CropAngle=\"0\"/></rdf:RDF></x:xmpmeta>"
        )
    }

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
                "fileSize": 2_000_000, "originalWidth": 4032, "originalHeight": 3024}, "xmp": {"exif": {"DateTimeOriginal": "2024-06-25T22:25:00.670+01:00"}, "tiff": {"Orientation": "right top"}}}),
            ),
            ("p2", "asset", "video", 0, json!({"captureDate": "2024-06-26T10:00:00", "importSource": {"fileName": "IMG_0002.MOV", "fileSize": 9}})),
            ("preset", "asset", "preset", 0, json!({"name": "Warm"})),
            // two edits of p1: the later one's settings are a file beside the catalog
            (
                "e1",
                "version",
                "edit",
                0,
                json!({"asset": {"id": "p1"}, "userUpdated": "2024-07-01T00:00:00.000Z", "develop": {"xmpCameraRaw": settings_packet("3.00")}}),
            ),
            (
                "e2",
                "version",
                "edit",
                0,
                json!({"asset": {"id": "p1"}, "userUpdated": "2025-01-01T00:00:00.000Z", "develop": {"xmpCameraRaw": {"sha256": "abc123"}}}),
            ),
            ("e3", "version", "edit", 0, json!({"asset": {"id": "p2"}, "develop": {"xmpCameraRaw": {}}})),
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
    fn the_reader_says_where_it_is() {
        let dir = temp_dir("stage");
        let (cancel, total, done, at) = (Default::default(), Default::default(), Default::default(), AtomicU8::new(0));
        inspect_staged(&mcat(&dir), &cancel, &total, &done, &at).unwrap();
        assert_eq!(at.load(Ordering::Relaxed), stage::ALBUMS, "an inspection never reads the photos");
        read_staged(&mcat(&dir), true, &cancel, &total, &done, &at).unwrap();
        assert_eq!(at.load(Ordering::Relaxed), stage::PHOTOS);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn exif_orientation_is_read_from_lightrooms_words() {
        assert_eq!([orientation("top left"), orientation("Right Top"), orientation("left bottom"), orientation("")], [Some(1), Some(6), Some(8), None]);
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
                path: None,
                orientation: Some(6),
            }
        );
        assert_eq!(source.photos.get("p2").and_then(|p| p.captured.clone()), Some("20240626100000".into()));
        assert_eq!(source.edits.get("p1"), Some(&DevelopSource::Hash("abc123".into())), "the latest edit of a photo, by its time");
        assert_eq!(source.edits.get("p2"), None, "an edit without settings is left out");
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
        let missing = report["missing"].as_array().unwrap();
        assert_eq!((report["missingTotal"].as_u64(), missing.len()), (Some(1), 1), "the photo the library lacks is listed");
        assert!(missing[0]["name"].as_str().is_some_and(|n| !n.is_empty()) && !missing[0]["albums"].as_array().unwrap().is_empty());
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

    /// The catalog's develop settings reach photos without edits: applied as the edit and copied
    /// beside a local original as its sidecar (never over one that is there); a photo with edits
    /// of its own keeps them.
    #[test]
    fn develop_settings_become_the_edit_and_a_sidecar() {
        use lightcraft_catalog::{Op, Photo, Source};
        let dir = temp_dir("develop");
        let path = mcat(&dir);
        let packet = settings_packet("1.00");
        std::fs::create_dir_all(dir.join("settings")).unwrap();
        std::fs::write(dir.join("settings/abc123"), &packet).unwrap();
        // the photo is in a folder of its own; the library's edit file (Photoshop's) is beside the catalog
        std::fs::create_dir_all(dir.join("photos")).unwrap();
        let original = dir.join("photos/IMG_0001.HEIC");
        std::fs::write(&original, b"x").unwrap();
        std::fs::create_dir_all(dir.join("originals/2024")).unwrap();
        std::fs::write(dir.join("originals/2024/IMG_0001-Edit.psb"), b"layers").unwrap();
        std::fs::write(dir.join("originals/2024/IMG_7777-Edit.tif"), b"nobody's").unwrap();
        let mut s = crate::Session::new();
        let id = s.catalog.alloc_photo_id();
        let mut photo =
            Photo::new(id, Source::File { path: original.to_string_lossy().into() }, "IMG_0001.HEIC", "HEIC", 4032, 3024, "2026-01-01T00:00:00");
        photo.file_size = 2_000_000;
        photo.captured = Some("2024-06-25T22:25:00".into());
        s.commit("add", Op::AddPhoto { photo: Box::new(photo) }).unwrap();

        let report = crate::lightroom_catalog::import(&mut s, &path, false, false).unwrap();
        assert_eq!((report["edits"].as_u64(), report["sidecars"].as_u64()), (Some(1), Some(1)), "{report}");
        assert_eq!(std::fs::read_to_string(dir.join("photos/IMG_0001.xmp")).unwrap(), packet, "the file as Lightroom kept it");
        assert_eq!(std::fs::read(dir.join("photos/IMG_0001-Edit.psb")).unwrap(), b"layers", "the edit file beside its photo");
        assert_eq!((report["editFiles"].as_u64(), report["editFilesUnmatched"].as_u64()), (Some(1), Some(1)), "{report}");
        assert_eq!(report["editsCropHeld"].as_u64(), Some(0), "the catalog's orientation puts the crop in its frame: {report}");
        assert!(s.catalog.photo(id).unwrap().edited.is_some());

        let again = crate::lightroom_catalog::import(&mut s, &path, false, false).unwrap();
        assert_eq!((again["edits"].as_u64(), again["editsKept"].as_u64(), again["sidecars"].as_u64()), (Some(0), Some(1), Some(0)));
        assert_eq!((again["editFiles"].as_u64(), again["editFilesKept"].as_u64()), (Some(0), Some(1)), "an edit file is never put twice");
        let _ = std::fs::remove_dir_all(dir);
    }
}
