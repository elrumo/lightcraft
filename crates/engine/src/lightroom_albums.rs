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
use std::sync::atomic::{AtomicBool, Ordering};

use lightcraft_catalog::{Album, AlbumId, Filter, Op, PhotoId, RuleSet};
use serde_json::{Value, json};

use crate::lightroom_catalog::{ImportIndex, path_key};
use crate::lightroom_companions::{EditFile, Server};
use crate::sidecar::{SidecarNaming, StoredFrame};

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
    /// The latest develop settings (Camera Raw XMP) of those photos, by catalog key (cloud catalogs).
    pub edits: HashMap<String, DevelopSource>,
    pub warnings: Vec<String>,
}

/// Where a cloud catalog keeps a photo's develop settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DevelopSource {
    /// The XMP packet, in the catalog itself.
    Inline(String),
    /// The sha256 of the packet's file, `settings/<sha256>` beside the catalog.
    Hash(String),
}

/// An album or a folder of albums, as the catalog names it.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceAlbum {
    pub key: String,
    pub name: String,
    /// The folder it is in (its key).
    pub parent: Option<String>,
    pub folder: bool,
    /// A Classic smart collection: its rules, or why they can't be carried over (the album then
    /// holds the photos the collection has now).
    pub smart: Option<Result<RuleSet, String>>,
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
    /// The file's EXIF orientation (1 to 8), when the catalog says.
    pub orientation: Option<u16>,
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
    /// Whether the photo has edits of its own already (Lightroom's never replace them).
    edited: bool,
    /// The original, when it is a file on this disk, and how its sidecar is named.
    file: Option<(String, SidecarNaming)>,
}

/// The library's photos as matching sees them (not deleted, not only browsed, not virtual copies).
pub(crate) fn library_photos(s: &crate::Session) -> Vec<LibraryPhoto> {
    let photos: Vec<_> = s.catalog.photos().filter(|p| p.in_library() && p.copy_of.is_none()).collect();
    let ids: Vec<PhotoId> = photos.iter().map(|p| p.id).collect();
    let namings = s.sidecar_namings(&ids);
    photos
        .into_iter()
        .zip(namings)
        .map(|(p, naming)| {
            let file = match &p.source {
                lightcraft_catalog::Source::File { path } if !path.starts_with(lightcraft_catalog::sync::PATH_PREFIX) => Some((path.clone(), naming)),
                _ => None,
            };
            LibraryPhoto {
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
                edited: p.edited.is_some(),
                file,
            }
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

    /// Matches, in order: the same file; the same byte size and name or capture second (the same
    /// bytes, maybe renamed); the same name, capture second and dimensions (a re-encoded copy).
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
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct AlbumCounts {
    /// New albums and new folders.
    pub(crate) albums: usize,
    pub(crate) folders: usize,
    /// Albums that were there already and got photos.
    pub(crate) updated: usize,
    /// Photos put in albums.
    pub(crate) added: usize,
    /// Smart collections that are smart albums now (new ones and kept ones).
    pub(crate) smart: usize,
    /// Of those, albums from an earlier import that stay as they are, rules included.
    pub(crate) smart_kept: usize,
    /// Smart collections that are static albums: rules that can't be carried over, or an album
    /// an earlier import made static.
    pub(crate) smart_as_static: usize,
    /// What the user should know about smart collections.
    pub(crate) warnings: Vec<String>,
}

/// Catalog operations making `albums` (folders before what they hold) with `members` as their
/// photos. An album imported before (`index`, keyed by the catalog's path and the album's key), or
/// else one with the same name and kind in the same folder, keeps its name, place and photos and
/// only gains the photos it lacks; a smart album also keeps its rules (the user may have edited
/// them). A smart collection becomes a smart album, or a static one when its rules can't be read.
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
        let smart_rules = album.smart.as_ref().and_then(|rules| rules.as_ref().ok());
        let same_name = || {
            s.catalog
                .albums()
                .find(|a| {
                    a.name == album.name
                        && a.parent == parent
                        && a.folder == album.folder
                        && a.is_smart() == smart_rules.is_some()
                        && !a.quick
                        && !planned.contains(&a.id)
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
        let smart_note = |why: &Result<RuleSet, String>| match why {
            Ok(_) => {
                format!("{}: an earlier import created a static album; it stays static (delete it and reimport to get a smart album)", album.name)
            }
            Err(why) => format!("{}: {why}; imported current membership (rules kept in archive)", album.name),
        };
        match s.catalog.album(id) {
            Some(old) if !old.folder && !old.is_smart() => {
                if let Some(rules) = &album.smart {
                    counts.smart_as_static += 1;
                    counts.warnings.push(smart_note(rules));
                }
                let mut merged = old.photos.clone();
                let had: HashSet<_> = merged.iter().copied().collect();
                merged.extend(photos.into_iter().filter(|p| !had.contains(p)));
                if merged.len() > old.photos.len() {
                    counts.updated += 1;
                    counts.added += merged.len() - old.photos.len();
                    ops.push(Op::SetAlbumPhotos { id, photos: merged });
                }
            }
            Some(old) if !old.folder => {
                if album.smart.is_some() {
                    counts.smart += 1;
                    counts.smart_kept += 1;
                }
            }
            Some(_) => {}
            None => {
                let mut new = Album::new(id, album.name.clone());
                new.parent = parent;
                new.folder = album.folder;
                let photos = match &album.smart {
                    Some(Ok(rules)) => {
                        new.smart = Some(Box::new(Filter { rule_set: Some(rules.clone()), ..Default::default() }));
                        counts.smart += 1;
                        Vec::new()
                    }
                    Some(err) => {
                        counts.smart_as_static += 1;
                        counts.warnings.push(smart_note(err));
                        photos
                    }
                    None => photos,
                };
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

/// What goes with the photos besides the albums: where the catalog keeps develop settings, the edit files
/// found beside it, and the server (when the library syncs with one).
#[derive(Clone, Copy, Default)]
pub(crate) struct Extras<'a> {
    pub(crate) settings_dir: Option<&'a Path>,
    pub(crate) edit_files: &'a [EditFile],
    pub(crate) server: Option<&'a Server>,
}

/// Albums-only work prepared off the owner thread: the albums with the library photos that matched.
#[derive(Clone, Debug)]
pub struct AlbumPlan {
    pub(crate) source: String,
    pub(crate) albums: Vec<SourceAlbum>,
    pub(crate) members: HashMap<String, Vec<PhotoId>>,
    pub(crate) counts: MatchCounts,
    pub(crate) warnings: Vec<String>,
    /// The photos albums hold that the library lacks (or has twice over), by name.
    pub(crate) missing: Vec<Value>,
    /// Lightroom's develop settings for matched photos that have no edits here: the XMP packets.
    pub(crate) develop: Vec<DevelopEdit>,
    pub(crate) edits: EditCounts,
}

/// A photo's Lightroom develop settings, with the stored image's frame their crop is in (when the catalog
/// says how the file is oriented).
#[derive(Clone, Debug)]
pub(crate) struct DevelopEdit {
    pub(crate) id: PhotoId,
    pub(crate) packet: String,
    pub(crate) frame: Option<StoredFrame>,
}

/// What came of the develop settings and edit files of the matched photos.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct EditCounts {
    /// Applied as the photo's edit.
    pub(crate) applied: usize,
    /// Left alone: the photo has edits here already.
    pub(crate) kept: usize,
    /// Put beside the photo as its `.xmp` sidecar (a copy on this disk, or sent to the server).
    pub(crate) sidecars: usize,
    /// The catalog names settings that aren't in the library's `settings` folder (or can't be read).
    pub(crate) unavailable: usize,
    /// Applied without the crop: the catalog doesn't say how the file is oriented.
    pub(crate) crop_held: usize,
    /// Edit files (`IMG_1-Edit.tif`) put beside their photo or sent to the server.
    pub(crate) files: usize,
    /// Edit files already beside their photo.
    pub(crate) files_kept: usize,
    /// Edit files with no photo to go beside (none, several, or nowhere to put it).
    pub(crate) files_unmatched: usize,
    /// Edit files that couldn't be put there.
    pub(crate) files_failed: usize,
}

/// The largest settings packet read (they are a few KB).
const MAX_PACKET: u64 = 4 << 20;

/// The XMP packet `source` names: inline, or the file `settings/<sha256>` in `settings_dir`.
fn packet_of(source: &DevelopSource, settings_dir: Option<&Path>) -> Option<String> {
    match source {
        DevelopSource::Inline(packet) => Some(packet.clone()),
        DevelopSource::Hash(hash) if hash.bytes().all(|b| b.is_ascii_alphanumeric()) => {
            let path = settings_dir?.join(hash);
            std::fs::metadata(&path).ok().filter(|m| m.len() <= MAX_PACKET)?;
            std::fs::read_to_string(path).ok()
        }
        DevelopSource::Hash(_) => None,
    }
}

/// Where a photo's companion files go: beside its original when that is a file on this disk, else to the
/// server that holds it (by content hash).
enum Place<'a> {
    Beside(&'a str),
    Server(&'a str),
}

fn place_of<'a>(photo: &'a LibraryPhoto, server: Option<&Server>) -> Option<Place<'a>> {
    match (&photo.file, &photo.hash) {
        (Some((original, _)), _) if Path::new(original).is_file() => Some(Place::Beside(original)),
        (_, Some(hash)) if server.is_some() => Some(Place::Server(hash)),
        _ => None,
    }
}

/// Copy `src` into the folder of `original` as `name`; `Ok(false)`: a file of that name is there already.
fn copy_beside(src: &Path, original: &str, name: &str) -> Result<bool, String> {
    let dir = Path::new(original).parent().ok_or("the original has no folder")?;
    let (target, temp) = (dir.join(name), dir.join(format!(".{name}.lightcraft-part")));
    if target.exists() {
        return Ok(false);
    }
    let err = |e: std::io::Error| format!("{}: {e}", target.display());
    std::fs::copy(src, &temp).map_err(err)?;
    if target.exists() {
        let _ = std::fs::remove_file(&temp);
        return Ok(false);
    }
    std::fs::rename(&temp, &target).map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        err(e)
    })?;
    Ok(true)
}

/// Lightroom's develop settings for the matched photos that have no edits here, read off the owner
/// thread, and put (a copy, never replacing a sidecar) beside the originals: on this disk, or sent to the
/// server for photos it holds.
fn develop_of(
    source: &AlbumSource,
    matches: &HashMap<String, PhotoId>,
    library: &[LibraryPhoto],
    settings_dir: Option<&Path>,
    server: Option<&Server>,
    cancel: &AtomicBool,
) -> (Vec<DevelopEdit>, EditCounts) {
    let by_id: HashMap<PhotoId, &LibraryPhoto> = library.iter().map(|p| (p.id, p)).collect();
    let mut counts = EditCounts::default();
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut keys: Vec<_> = source.edits.keys().collect();
    keys.sort_unstable();
    for key in keys {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let (Some(id), Some(edit)) = (matches.get(key), source.edits.get(key)) else { continue };
        let Some(photo) = by_id.get(id).filter(|_| seen.insert(*id)) else { continue };
        if photo.edited {
            counts.kept += 1;
            continue;
        }
        let Some(packet) = packet_of(edit, settings_dir) else {
            counts.unavailable += 1;
            continue;
        };
        match place_of(photo, server) {
            Some(Place::Beside(original)) => {
                let naming = photo.file.as_ref().map(|(_, n)| *n).unwrap_or_default();
                if crate::sidecar::find_sidecar(original, naming).is_none()
                    && lightcraft_catalog::safe_file::write_atomic(&crate::sidecar::sidecar_path(original, naming), packet.as_bytes()).is_ok()
                {
                    counts.sidecars += 1;
                }
            }
            Some(Place::Server(hash)) => {
                // (the server puts it beside the photo, or keeps it for when the photo is filed into a folder)
                let temp = std::env::temp_dir().join(format!("lightcraft-lr-{}-{}.xmp", std::process::id(), id.0));
                if std::fs::write(&temp, packet.as_bytes()).is_ok() {
                    let sent = server.map(|s| s.put_companion(hash, &format!("{}.xmp", photo.stem), &temp));
                    counts.sidecars += usize::from(matches!(sent, Some(Ok(()))));
                    let _ = std::fs::remove_file(&temp);
                }
            }
            None => {}
        }
        // the crop is in the stored image's frame: the file's orientation (the catalog's words) and its shown size
        let frame = source
            .photos
            .get(key)
            .and_then(|p| p.orientation)
            .filter(|_| photo.dims.0 > 0 && photo.dims.1 > 0)
            .and_then(|o| StoredFrame::of_shown(lightcraft_geom::Orientation::from_exif(o), f64::from(photo.dims.0), f64::from(photo.dims.1)));
        out.push(DevelopEdit { id: *id, packet, frame });
    }
    (out, counts)
}

/// Put each edit file beside the library photo it came from (the one photo with its name), or send it to the
/// server that holds that photo; never over a file that is there.
fn place_edit_files(files: &[EditFile], library: &[LibraryPhoto], server: Option<&Server>, cancel: &AtomicBool, counts: &mut EditCounts) {
    let mut by_stem: HashMap<&str, Vec<&LibraryPhoto>> = HashMap::new();
    for p in library.iter().filter(|p| crate::lightroom_companions::original_stem(&format!("{}.tif", p.stem)).is_none()) {
        by_stem.entry(&p.stem).or_default().push(p);
    }
    for file in files {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let candidates = by_stem.get(file.original_stem.as_str()).map(Vec::as_slice).unwrap_or_default();
        let Found::One(id) = pick(candidates) else {
            counts.files_unmatched += 1;
            continue;
        };
        let Some(photo) = candidates.iter().find(|p| p.id == id) else { continue };
        match place_of(photo, server) {
            Some(Place::Beside(original)) => match copy_beside(&file.path, original, &file.name) {
                Ok(true) => counts.files += 1,
                Ok(false) => counts.files_kept += 1,
                Err(e) => {
                    log::warn!("Lightroom edit file {}: {e}", file.name);
                    counts.files_failed += 1;
                }
            },
            Some(Place::Server(hash)) => match server.map(|s| s.put_companion(hash, &file.name, &file.path)) {
                Some(Ok(())) => counts.files += 1,
                Some(Err(e)) => {
                    log::warn!("Lightroom edit file {}: {e}", file.name);
                    counts.files_failed += 1;
                }
                None => counts.files_unmatched += 1,
            },
            None => counts.files_unmatched += 1,
        }
    }
}

/// The most photos the result lists by name.
const MAX_LISTED: usize = 20_000;

/// The photos of `source`'s albums that `matches` lacks: name, size, capture time, the albums
/// holding them (first three), and whether several library photos fit (`ambiguous`).
fn missing_photos(source: &AlbumSource, matches: &HashMap<String, PhotoId>, library: &[LibraryPhoto]) -> Vec<Value> {
    let names: HashMap<&str, &str> = source.albums.iter().map(|a| (a.key.as_str(), a.name.as_str())).collect();
    let mut albums: HashMap<&str, Vec<&str>> = HashMap::new();
    for (album, keys) in &source.members {
        for key in keys {
            if !matches.contains_key(key) {
                albums.entry(key).or_default().push(names.get(album.as_str()).copied().unwrap_or(""));
            }
        }
    }
    let matcher = Matcher::new(library);
    let mut out: Vec<(String, Value)> = albums
        .into_iter()
        .map(|(key, mut in_albums)| {
            in_albums.sort_unstable();
            let photo = source.photos.get(key);
            let name = photo.map_or("", |p| p.name.as_str()).to_string();
            let ambiguous = photo.is_some_and(|p| matches!(matcher.find(p), Found::Ambiguous));
            let value = json!({
                "name": name,
                "size": photo.and_then(|p| p.size),
                "captured": photo.and_then(|p| p.captured.clone()),
                "albums": in_albums.iter().take(3).collect::<Vec<_>>(),
                "ambiguous": ambiguous,
            });
            (name.to_lowercase(), value)
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out.into_iter().map(|(_, v)| v).collect()
}

impl AlbumPlan {
    /// Match the photos of `source`'s albums against `library`; `settings_dir` is where the catalog's
    /// develop settings files are (cloud catalogs).
    pub(crate) fn new(source: AlbumSource, library: &[LibraryPhoto], extras: Extras<'_>, cancel: &AtomicBool) -> Self {
        let (matches, counts) = match_photos(&source, library);
        let (develop, mut edits) = develop_of(&source, &matches, library, extras.settings_dir, extras.server, cancel);
        place_edit_files(extras.edit_files, library, extras.server, cancel, &mut edits);
        let members = source
            .members
            .iter()
            .map(|(album, photos)| (album.clone(), photos.iter().filter_map(|key| matches.get(key).copied()).collect()))
            .collect();
        let missing = missing_photos(&source, &matches, library);
        AlbumPlan { source: source.source, albums: source.albums, members, counts, warnings: source.warnings, missing, develop, edits }
    }

    /// Make the albums in one undo step and report what was done.
    pub(crate) fn apply(mut self, s: &mut crate::Session, index: &mut ImportIndex) -> crate::Result<Value> {
        // photos deleted while the catalog was read are left out
        for photos in self.members.values_mut() {
            photos.retain(|id| s.catalog.photo(*id).is_some_and(|p| p.in_library()));
        }
        let (mut ops, made) = album_ops(s, &self.source, &self.albums, &self.members, index);
        self.warnings.extend(made.warnings.iter().cloned());
        // the settings only go to photos still without edits, and only what is left of the plan
        let mut edits = self.edits;
        for DevelopEdit { id, packet, frame } in std::mem::take(&mut self.develop) {
            let Some(photo) = s.catalog.photo(id).filter(|p| p.in_library() && p.edited.is_none()) else { continue };
            let Ok(mut sc) = crate::sidecar::parse_sidecar(&packet, crate::crs::Target::for_photo(photo)) else {
                edits.unavailable += 1;
                continue;
            };
            if sc.needs_frame() {
                // (without the catalog's orientation the crop is left out: the sidecar beside the photo keeps it)
                sc.set_frame(frame);
                edits.crop_held += usize::from(sc.hold_unframed_crop().is_some());
            }
            if let Ok(Op::Batch { ops: changes }) = s.sidecar_op(id, sc)
                && !changes.is_empty()
            {
                edits.applied += 1;
                ops.extend(changes);
            }
        }
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
            "missing": self.missing.iter().take(MAX_LISTED).collect::<Vec<_>>(),
            "missingTotal": self.missing.len(),
            "edits": edits.applied,
            "editsKept": edits.kept,
            "editsUnavailable": edits.unavailable,
            "editsCropHeld": edits.crop_held,
            "sidecars": edits.sidecars,
            "editFiles": edits.files,
            "editFilesKept": edits.files_kept,
            "editFilesUnmatched": edits.files_unmatched,
            "editFilesFailed": edits.files_failed,
            "warnings": self.warnings,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn photo(id: u64, name: &str, size: u64, captured: &str, dims: (u32, u32)) -> LibraryPhoto {
        LibraryPhoto {
            id: PhotoId(id),
            stem: stem(name),
            size,
            second: second(captured),
            dims,
            path: None,
            hash: Some(format!("{id:032x}")),
            edited: false,
            file: None,
        }
    }

    fn identity(name: &str, size: Option<u64>, captured: &str, dims: Option<(u32, u32)>) -> PhotoIdentity {
        PhotoIdentity { name: name.into(), size, captured: second(captured), dims, path: None, orientation: None }
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
            smart: None,
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

    /// A smart collection is a smart album, found again by name among the smart albums (so it keeps
    /// its rules and is not made twice); rules that can't be read make a regular album instead.
    #[test]
    fn smart_collections_become_smart_albums_found_again_by_name() {
        let mut s = crate::Session::new();
        let rules =
            crate::lightroom_smart::smart_rules("s = { { criteria = \"rating\", operation = \">=\", value = 4 }, combine = \"intersect\" }").unwrap();
        let album = |smart| SourceAlbum { key: "k".into(), name: "Best".into(), parent: None, folder: false, smart: Some(smart) };
        let smart = [album(Ok(rules))];
        let (ops, counts) = album_ops(&mut s, "x.lrcat", &smart, &HashMap::new(), &mut ImportIndex::default());
        assert_eq!((counts.albums, counts.smart, counts.smart_kept, counts.smart_as_static), (1, 1, 0, 0));
        s.commit("albums", Op::Batch { ops }).unwrap();
        assert!(s.catalog.albums().all(|a| a.is_smart()));
        let (again, counts) = album_ops(&mut s, "x.lrcat", &smart, &HashMap::new(), &mut ImportIndex::default());
        assert!(again.is_empty());
        assert_eq!((counts.albums, counts.smart, counts.smart_kept), (0, 1, 1));
        let (ops, counts) = album_ops(&mut s, "y.lrcat", &[album(Err("collection rules".into()))], &HashMap::new(), &mut ImportIndex::default());
        assert_eq!((counts.albums, counts.smart, counts.smart_as_static), (1, 0, 1));
        assert!(counts.warnings.iter().any(|w| w.starts_with("Best: collection rules")), "{:?}", counts.warnings);
        s.commit("albums", Op::Batch { ops }).unwrap();
        assert_eq!(s.catalog.albums().filter(|a| a.is_smart()).count(), 1);
        assert_eq!(s.catalog.albums().count(), 2);
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
