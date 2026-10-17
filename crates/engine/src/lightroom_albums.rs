//! Lightroom albums and folders of albums into the open library, holding the photos the library
//! already has.
//!
//! The albums come from a Lightroom (cloud) catalog ([`crate::lightroom_cloud`]) or a Classic one
//! ([`crate::lightroom_catalog`]). In albums-only mode no photo is added or changed: each
//! Lightroom photo is matched to a library photo by its file's identity (path, byte size, name,
//! capture time, dimensions), never guessed: when two library photos fit equally it is reported
//! as ambiguous and left out. The Classic import builds its albums with the same [`album_ops`].
//!
//! Albums already imported are found again through the import index, else by name in the same
//! folder, so importing twice (or on a second device of a synced library) merges instead of
//! duplicating.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use lightcraft_catalog::{Album, AlbumId, Op, PhotoId};
use serde_json::{Value, json};

use crate::lightroom_catalog::{ImportIndex, path_key};

/// A catalog's albums, their photos and what identifies each photo's file.
#[derive(Clone, Debug, Default)]
pub struct AlbumSource {
    /// The catalog file.
    pub source: String,
    /// Albums and folders, in any order.
    pub albums: Vec<SourceAlbum>,
    /// Each album's photos (catalog keys), in the catalog's order.
    pub members: HashMap<String, Vec<String>>,
    /// The photos albums hold, by catalog key.
    pub photos: HashMap<String, PhotoIdentity>,
    pub warnings: Vec<String>,
}

/// An album or a folder of albums, as the catalog names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceAlbum {
    pub key: String,
    pub name: String,
    /// The folder it is in (its key).
    pub parent: Option<String>,
    pub folder: bool,
}

/// What a catalog knows of a photo's original file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PhotoIdentity {
    /// The original's file name.
    pub name: String,
    /// Its size in bytes.
    pub size: Option<u64>,
    /// Capture time to the second ([`second`]).
    pub captured: Option<String>,
    /// Pixel width and height, either way round.
    pub dims: Option<(u32, u32)>,
    /// Where the original is (Classic catalogs).
    pub path: Option<String>,
}

/// A capture time to the second as `YYYYMMDDHHMMSS`, from any ISO 8601 or EXIF spelling (its first
/// 14 digits): fractions and zone are left out, as one side often lacks them.
pub(crate) fn second(text: &str) -> Option<String> {
    let digits: String = text.chars().filter(char::is_ascii_digit).take(14).collect();
    (digits.len() == 14 && !digits.starts_with("0000")).then_some(digits)
}

/// A file name without its extension, lower-cased: the same photo keeps its name across copies,
/// but a re-encoded copy may change the extension.
fn stem(name: &str) -> String {
    Path::new(name).file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default()
}

/// What matching needs of a library photo, taken on the owner thread.
#[derive(Clone, Debug)]
pub(crate) struct LibraryPhoto {
    id: PhotoId,
    stem: String,
    size: u64,
    second: Option<String>,
    dims: (u32, u32),
    path: Option<String>,
    hash: Option<String>,
}

/// The library's photos as matching sees them (not deleted, not only browsed, not virtual copies).
pub(crate) fn library_photos(s: &crate::Session) -> Vec<LibraryPhoto> {
    s.catalog
        .photos()
        .filter(|p| p.in_library() && p.copy_of.is_none())
        .map(|p| LibraryPhoto {
            id: p.id,
            stem: stem(&p.file_name),
            size: p.file_size,
            second: p.captured.as_deref().and_then(second),
            dims: (p.width, p.height),
            path: match &p.source {
                lightcraft_catalog::Source::File { path } => Some(path_key(path)),
                _ => None,
            },
            hash: lightcraft_catalog::sync::content_key(p),
        })
        .collect()
}

/// How the photos albums hold were matched.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct MatchCounts {
    pub(crate) matched: usize,
    /// More than one library photo fits.
    pub(crate) ambiguous: usize,
    /// Not in the library.
    pub(crate) unmatched: usize,
}

enum Found {
    One(PhotoId),
    Ambiguous,
    None,
}

/// One photo out of `candidates`: the only one, or one of several copies of the same file.
fn pick(candidates: &[&LibraryPhoto]) -> Found {
    match candidates {
        [] => Found::None,
        [one] => Found::One(one.id),
        [first, rest @ ..] if first.hash.is_some() && rest.iter().all(|p| p.hash == first.hash) => {
            Found::One(candidates.iter().map(|p| p.id).min().unwrap_or(first.id))
        }
        _ => Found::Ambiguous,
    }
}

struct Matcher<'a> {
    by_path: HashMap<&'a str, PhotoId>,
    by_size: HashMap<u64, Vec<&'a LibraryPhoto>>,
    by_stem: HashMap<&'a str, Vec<&'a LibraryPhoto>>,
}

impl<'a> Matcher<'a> {
    fn new(library: &'a [LibraryPhoto]) -> Self {
        let mut m = Matcher { by_path: HashMap::new(), by_size: HashMap::new(), by_stem: HashMap::new() };
        for p in library {
            if let Some(path) = &p.path {
                m.by_path.entry(path).or_insert(p.id);
            }
            if p.size > 0 {
                m.by_size.entry(p.size).or_default().push(p);
            }
            m.by_stem.entry(&p.stem).or_default().push(p);
        }
        m
    }

    /// 1. the same file; 2. the same byte size and name or capture second (the same bytes,
    /// maybe renamed); 3. the same name, capture second and dimensions (a re-encoded copy).
    fn find(&self, photo: &PhotoIdentity) -> Found {
        if let Some(id) = photo.path.as_deref().and_then(|p| self.by_path.get(path_key(p).as_str())) {
            return Found::One(*id);
        }
        let name = stem(&photo.name);
        let same_second = |p: &LibraryPhoto| photo.captured.is_some() && p.second == photo.captured;
        if let Some(size) = photo.size.filter(|n| *n > 0) {
            let candidates: Vec<_> =
                self.by_size.get(&size).into_iter().flatten().copied().filter(|p| (!name.is_empty() && p.stem == name) || same_second(p)).collect();
            if !candidates.is_empty() {
                return pick(&candidates);
            }
        }
        if name.is_empty() || photo.captured.is_none() {
            return Found::None;
        }
        let fits = |p: &LibraryPhoto| match photo.dims {
            Some((w, h)) if p.dims != (0, 0) => p.dims == (w, h) || p.dims == (h, w),
            _ => true,
        };
        let candidates: Vec<_> = self.by_stem.get(name.as_str()).into_iter().flatten().copied().filter(|p| same_second(p) && fits(p)).collect();
        pick(&candidates)
    }
}

/// The library photo of each photo `source`'s albums hold, by catalog key, with the counts.
pub(crate) fn match_photos(source: &AlbumSource, library: &[LibraryPhoto]) -> (HashMap<String, PhotoId>, MatchCounts) {
    let matcher = Matcher::new(library);
    let mut out = HashMap::new();
    let mut counts = MatchCounts::default();
    let mut seen = HashSet::new();
    for key in source.members.values().flatten() {
        if !seen.insert(key) {
            continue;
        }
        let found = source.photos.get(key).map_or(Found::None, |photo| matcher.find(photo));
        match found {
            Found::One(id) => {
                counts.matched += 1;
                out.insert(key.clone(), id);
            }
            Found::Ambiguous => counts.ambiguous += 1,
            Found::None => counts.unmatched += 1,
        }
    }
    (out, counts)
}

/// What [`album_ops`] makes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct AlbumCounts {
    /// New albums and new folders.
    pub(crate) albums: usize,
    pub(crate) folders: usize,
    /// Albums that were there already and got photos.
    pub(crate) updated: usize,
    /// Photos put in albums.
    pub(crate) added: usize,
}

/// Catalog operations making `albums` (folders before what they hold) with `members` as their
/// photos. An album imported before (`index`, keyed by the catalog's path and the album's key), or
/// else one with the same name and kind in the same folder, keeps its name, place and photos and
/// only gains the photos it lacks.
pub(crate) fn album_ops(
    s: &mut crate::Session,
    source: &str,
    albums: &[SourceAlbum],
    members: &HashMap<String, Vec<PhotoId>>,
    index: &mut ImportIndex,
) -> (Vec<Op>, AlbumCounts) {
    let parents: HashMap<&str, Option<&str>> = albums.iter().map(|a| (a.key.as_str(), a.parent.as_deref())).collect();
    let depth = |album: &SourceAlbum| {
        let mut depth = 0;
        let mut at = album.parent.as_deref();
        while let Some(key) = at
            && depth < 64
        {
            depth += 1;
            at = parents.get(key).copied().flatten();
        }
        depth
    };
    let mut ordered: Vec<_> = albums.iter().collect();
    ordered.sort_by_key(|a| depth(a));
    let mut ids: HashMap<&str, AlbumId> = HashMap::new();
    let mut planned: HashSet<AlbumId> = HashSet::new();
    // what may hold albums: a damaged catalog's album "in" an album goes to the top instead
    let mut folders: HashSet<AlbumId> = HashSet::new();
    let mut ops = Vec::new();
    let mut counts = AlbumCounts::default();
    for album in ordered {
        let parent = album.parent.as_deref().and_then(|p| ids.get(p)).copied().filter(|p| folders.contains(p));
        let index_key = format!("{}:{}", path_key(source), album.key);
        let known = index.collections.get(&index_key).copied().map(AlbumId).filter(|id| s.catalog.album(*id).is_some() && !planned.contains(id));
        let same_name = || {
            s.catalog
                .albums()
                .find(|a| {
                    a.name == album.name && a.parent == parent && a.folder == album.folder && !a.is_smart() && !a.quick && !planned.contains(&a.id)
                })
                .map(|a| a.id)
        };
        let existing = known.or_else(same_name);
        let id = existing.unwrap_or_else(|| s.catalog.alloc_album_id());
        planned.insert(id);
        if s.catalog.album(id).map_or(album.folder, |a| a.folder) {
            folders.insert(id);
        }
        index.collections.insert(index_key, id.0);
        ids.insert(&album.key, id);
        let mut seen = HashSet::new();
        let photos: Vec<PhotoId> =
            if album.folder { Vec::new() } else { members.get(&album.key).into_iter().flatten().copied().filter(|p| seen.insert(*p)).collect() };
        match s.catalog.album(id) {
            Some(old) if !old.folder && !old.is_smart() => {
                let mut merged = old.photos.clone();
                let had: HashSet<_> = merged.iter().copied().collect();
                merged.extend(photos.into_iter().filter(|p| !had.contains(p)));
                if merged.len() > old.photos.len() {
                    counts.updated += 1;
                    counts.added += merged.len() - old.photos.len();
                    ops.push(Op::SetAlbumPhotos { id, photos: merged });
                }
            }
            Some(_) => {}
            None => {
                let mut new = Album::new(id, album.name.clone());
                new.parent = parent;
                new.folder = album.folder;
                counts.added += photos.len();
                new.photos = photos;
                if album.folder {
                    counts.folders += 1;
                } else {
                    counts.albums += 1;
                }
                ops.push(Op::AddAlbum { album: new });
            }
        }
    }
    (ops, counts)
}

/// Albums-only work prepared off the owner thread: the albums with the library photos that matched.
#[derive(Clone, Debug)]
pub struct AlbumPlan {
    pub(crate) source: String,
    pub(crate) albums: Vec<SourceAlbum>,
    pub(crate) members: HashMap<String, Vec<PhotoId>>,
    pub(crate) counts: MatchCounts,
    pub(crate) warnings: Vec<String>,
}

impl AlbumPlan {
    /// Match the photos of `source`'s albums against `library`.
    pub(crate) fn new(source: AlbumSource, library: &[LibraryPhoto]) -> Self {
        let (matches, counts) = match_photos(&source, library);
        let members = source
            .members
            .iter()
            .map(|(album, photos)| (album.clone(), photos.iter().filter_map(|key| matches.get(key).copied()).collect()))
            .collect();
        AlbumPlan { source: source.source, albums: source.albums, members, counts, warnings: source.warnings }
    }

    /// Make the albums in one undo step and report what was done.
    pub(crate) fn apply(mut self, s: &mut crate::Session, index: &mut ImportIndex) -> crate::Result<Value> {
        // photos deleted while the catalog was read are left out
        for photos in self.members.values_mut() {
            photos.retain(|id| s.catalog.photo(*id).is_some_and(|p| p.in_library()));
        }
        let (ops, made) = album_ops(s, &self.source, &self.albums, &self.members, index);
        if !ops.is_empty() {
            s.commit("Import Lightroom Albums", Op::Batch { ops })?;
        }
        Ok(json!({
            "albumsOnly": true,
            "source": self.source,
            "library": s.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.to_string_lossy()),
            "collections": self.albums.len(),
            "albums": made.albums,
            "folders": made.folders,
            "albumsUpdated": made.updated,
            "added": made.added,
            "matched": self.counts.matched,
            "ambiguous": self.counts.ambiguous,
            "unmatched": self.counts.unmatched,
            "warnings": self.warnings,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn photo(id: u64, name: &str, size: u64, captured: &str, dims: (u32, u32)) -> LibraryPhoto {
        LibraryPhoto { id: PhotoId(id), stem: stem(name), size, second: second(captured), dims, path: None, hash: Some(format!("{id:032x}")) }
    }

    fn identity(name: &str, size: Option<u64>, captured: &str, dims: Option<(u32, u32)>) -> PhotoIdentity {
        PhotoIdentity { name: name.into(), size, captured: second(captured), dims, path: None }
    }

    fn source(photos: Vec<(&str, PhotoIdentity)>) -> AlbumSource {
        AlbumSource {
            members: HashMap::from([("a".to_string(), photos.iter().map(|(k, _)| k.to_string()).collect())]),
            photos: photos.into_iter().map(|(k, p)| (k.to_string(), p)).collect(),
            ..AlbumSource::default()
        }
    }

    #[test]
    fn capture_times_compare_to_the_second() {
        assert_eq!(second("2024-06-25T22:25:00.670+01:00").as_deref(), Some("20240625222500"));
        assert_eq!(second("2024:06:25 22:25:00").as_deref(), Some("20240625222500"));
        assert_eq!(second("0000-00-00T00:00:00"), None);
        assert_eq!(second("2024-06-25"), None);
    }

    /// Size + name, size + capture (renamed), name + capture + turned dimensions (re-encoded);
    /// two different files that fit equally are ambiguous, never guessed.
    #[test]
    fn photos_match_by_file_identity_or_not_at_all() {
        let library = vec![
            photo(1, "IMG_0001.HEIC", 2_000_000, "2024-06-25T22:25:00", (4032, 3024)),
            photo(2, "renamed.jpg", 3_000_000, "2024-06-26T10:00:00", (4000, 3000)),
            photo(3, "IMG_0003.jpeg", 900_000, "2024-06-27T08:00:01+02:00", (3024, 4032)),
            photo(4, "DSC_0001.NEF", 5_000_000, "2024-01-01T00:00:00", (6000, 4000)),
            photo(5, "DSC_0001.NEF", 5_100_000, "2024-01-01T00:00:00", (6000, 4000)),
        ];
        let src = source(vec![
            ("p1", identity("IMG_0001.HEIC", Some(2_000_000), "2024-06-25T22:25:00", None)),
            ("p2", identity("IMG_0002.JPG", Some(3_000_000), "2024-06-26T10:00:00.120Z", None)),
            ("p3", identity("IMG_0003.HEIC", Some(1_500_000), "2024-06-27T08:00:01", Some((4032, 3024)))),
            ("p4", identity("DSC_0001.NEF", None, "2024-01-01T00:00:00", Some((6000, 4000)))),
            ("p5", identity("IMG_9999.HEIC", Some(42), "2023-01-01T00:00:00", None)),
            ("p6", identity("IMG_0003.HEIC", Some(1_500_000), "2024-06-27T08:00:01", Some((100, 100)))),
        ]);
        let (found, counts) = match_photos(&src, &library);
        assert_eq!(found.get("p1"), Some(&PhotoId(1)));
        assert_eq!(found.get("p2"), Some(&PhotoId(2)));
        assert_eq!(found.get("p3"), Some(&PhotoId(3)));
        assert_eq!(found.get("p4"), None);
        assert_eq!(counts, MatchCounts { matched: 3, ambiguous: 1, unmatched: 2 });
    }

    /// Folders come before what they hold; an album "in" an album (a damaged catalog) goes to the
    /// top instead of failing the import; a second run finds the albums by name.
    #[test]
    fn album_tree_is_made_parents_first_and_found_again_by_name() {
        let mut s = crate::Session::new();
        let album = |key: &str, parent: Option<&str>, folder: bool| SourceAlbum {
            key: key.into(),
            name: key.to_uppercase(),
            parent: parent.map(str::to_string),
            folder,
        };
        let albums =
            vec![album("inner", Some("mid"), false), album("mid", Some("top"), true), album("top", None, true), album("odd", Some("inner"), false)];
        let mut index = ImportIndex::default();
        let (ops, counts) = album_ops(&mut s, "x.mcat", &albums, &HashMap::new(), &mut index);
        assert_eq!((counts.albums, counts.folders), (2, 2));
        s.commit("albums", Op::Batch { ops }).unwrap();
        let named = |name: &str| s.catalog.albums().find(|a| a.name == name).cloned().unwrap();
        assert_eq!(named("INNER").parent, Some(named("MID").id));
        assert_eq!(named("MID").parent, Some(named("TOP").id));
        assert_eq!(named("ODD").parent, None);
        let (again, counts) = album_ops(&mut s, "x.mcat", &albums, &HashMap::new(), &mut ImportIndex::default());
        assert!(again.is_empty());
        assert_eq!(counts, AlbumCounts::default());
    }

    /// Two copies of one file in the library are one photo, not an ambiguity.
    #[test]
    fn copies_of_one_file_match_the_first() {
        let mut a = photo(7, "IMG_1.HEIC", 10, "2024-06-25T22:25:00", (1, 1));
        let mut b = photo(4, "IMG_1.HEIC", 10, "2024-06-25T22:25:00", (1, 1));
        a.hash = Some("same".into());
        b.hash = Some("same".into());
        let src = source(vec![("p", identity("IMG_1.HEIC", Some(10), "", None))]);
        let (found, _) = match_photos(&src, &[a, b]);
        assert_eq!(found.get("p"), Some(&PhotoId(4)));
    }
}
