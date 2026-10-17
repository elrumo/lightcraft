//! Reading a big `catalog.snap` on several threads.
//!
//! A snapshot is one JSON object whose first member is the photos map: `{"photos":{"<id>":{"id":…},…},"albums":…}`,
//! a few KB per photo, hundreds of MB for a big library, and serde reads it on one thread. This cuts
//! the photos map at entry boundaries, reads the pieces on worker threads and the rest (albums,
//! stacks, counters: small) in one go. It is an optimisation of [`serde_json::from_slice`] and gives
//! the same catalog. It answers `None`, and the caller reads the snapshot the plain way, for a small
//! snapshot, a layout it does not recognise, or a piece that does not parse (so every error still
//! comes from the plain read).
//!
//! Most of a photo's JSON is its develop settings (3 KB of defaults), and most photos of a library have
//! the same ones. Within a piece, the second and later photos with the same settings text are rewritten
//! before serde sees them as `[n]`, a reference to the `n`th settings of the piece, which the first one
//! (`[n,{…}]`) introduced and [`develop`] remembers: the text is compared, not parsed, and the photos
//! share one allocation. Anything that does not fit (a reference to nothing, the markers in a place they
//! should not be) is a parse error of the piece, so the plain read decides.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use lightcraft_develop::DevelopSettings;
use memchr::memmem;
use serde::de::{self, Error as _, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};

use crate::journal::SnapFile;
use crate::model::{Photo, PhotoId};
use crate::store::RangeReader;

/// A snapshot shorter than this is read on one thread (starting threads would cost more than they save).
const MIN_BYTES: usize = 16 << 20;
/// About how much of the photos map one piece holds: workers take pieces as they finish, so they end
/// together, and each holds one piece's copy at a time.
const PIECE_BYTES: usize = 4 << 20;
/// Where in the snapshot the photos map starts, and what follows it.
const PHOTOS: &[u8] = b"\"catalog\":{\"photos\":{";
const AFTER: &[u8] = b"},\"albums\":";
/// The end of an entry's key and the start of its value (a photo's first member is its `id`).
const ENTRY: &[u8] = b"\":{\"id\":";
/// How far past a cut to look for the next entry (an entry is a few KB).
const LOOK: usize = 1 << 20;

/// The snapshot read on several threads, straight from the file by byte ranges (so it is never held whole in
/// memory), or `None` (read it the plain way).
pub(crate) fn read(size: u64, read_at: &RangeReader) -> Option<SnapFile> {
    // (the web build has no threads)
    if cfg!(target_arch = "wasm32") {
        return None;
    }
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(16);
    read_with(size, read_at, Settings { min_bytes: MIN_BYTES, piece_bytes: PIECE_BYTES, threads })
}

#[derive(Clone, Copy)]
pub(crate) struct Settings {
    pub min_bytes: usize,
    pub piece_bytes: usize,
    pub threads: usize,
}

/// How far into the file's end the albums and the other small members are looked for.
const TAIL: usize = 16 << 20;

pub(crate) fn read_with(size: u64, read_at: &RangeReader, s: Settings) -> Option<SnapFile> {
    let size = usize::try_from(size).ok()?;
    if size < s.min_bytes || s.threads < 2 {
        return None;
    }
    let head = read_at(0, size.min(256)).ok()?;
    let first = find(&head, PHOTOS)? + PHOTOS.len();
    // the `}` that closes the photos map (the albums follow it; found from the end, where the albums are)
    let tail_from = size.saturating_sub(TAIL).max(first);
    let tail = read_at(tail_from as u64, size - tail_from).ok()?;
    let end = tail_from + rfind(&tail, AFTER)?;
    let span = end.checked_sub(first).filter(|n| *n > 0)?;
    let pieces = (span / s.piece_bytes.max(1)).max(s.threads);
    // where each piece starts: the opening quote of an entry's key, found past an even cut
    let mut starts = vec![first];
    for k in 1..pieces {
        let cut = first.checked_add(span / pieces * k)?;
        // (a few bytes before the cut: the digits of a key that it falls in)
        let from = cut.saturating_sub(32).max(first);
        let window = read_at(from as u64, end.min(cut.saturating_add(LOOK)).checked_sub(from)?).ok()?;
        // (none: the cut is inside the last entry)
        let Some(open) = next_entry(&window, cut - from).map(|i| from + i) else { continue };
        if open > *starts.last()? {
            starts.push(open);
        }
    }
    // piece i is [starts[i], starts[i + 1] - 1): without the comma between entries
    let ranges: Vec<(usize, usize)> = starts.iter().enumerate().map(|(i, s)| (*s, starts.get(i + 1).map_or(end, |n| n - 1))).collect();

    let next = AtomicUsize::new(0);
    let done: Vec<Vec<(usize, Vec<(PhotoId, Arc<Photo>)>)>> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..s.threads.min(ranges.len()))
            .map(|_| {
                scope.spawn(|| {
                    let mut mine = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(&(from, to)) = ranges.get(i) else { return Some(mine) };
                        let piece = read_at(from as u64, to.checked_sub(from)?).ok()?;
                        mine.push((i, entries(&piece)?));
                    }
                })
            })
            .collect();
        // (a worker that failed or panicked: the plain read decides)
        workers.into_iter().map(|w| w.join().ok().flatten()).collect::<Option<Vec<_>>>()
    })?;
    let mut parts: Vec<_> = done.into_iter().flatten().collect();
    parts.sort_unstable_by_key(|(i, _)| *i);
    let photos: BTreeMap<PhotoId, Arc<Photo>> = parts.into_iter().flat_map(|(_, part)| part).collect();

    // everything but the photos, with an empty photos map in their place
    let mut rest = Vec::with_capacity(first + 1 + tail.len());
    rest.extend_from_slice(head.get(..first)?);
    rest.push(b'}');
    rest.extend_from_slice(tail.get(end - tail_from + 1..)?);
    let mut snap: SnapFile = serde_json::from_slice(&rest).ok()?;
    snap.catalog.photos = photos;
    Some(snap)
}

/// The opening quote of the first entry's key that starts at or after `from` in `window` (which starts
/// inside the photos map, some digits before `from` at most): `,"<id>":{"id":<id>,`. Other `":{"id":` in
/// an entry (a nested `"profile":{"id":…`) are passed over.
fn next_entry(window: &[u8], from: usize) -> Option<usize> {
    let mut at = from;
    loop {
        let key_end = at + find(window.get(at..)?, ENTRY)?;
        at = key_end + 1;
        let digits = window.get(..key_end)?.iter().rev().take_while(|b| b.is_ascii_digit()).count();
        let Some(open) = key_end.checked_sub(digits + 1).filter(|_| digits > 0) else { continue };
        let id = window.get(open + 1..key_end)?;
        let value = window.get(key_end + ENTRY.len()..)?;
        if window.get(open) == Some(&b'"')
            && window.get(open.checked_sub(1)?) == Some(&b',')
            && value.starts_with(id)
            && value.get(id.len()) == Some(&b',')
        {
            return Some(open);
        }
    }
}

/// The entries `"<id>":{…},"<id>":{…}` of one piece, if each photo has the id it is filed under.
fn entries(piece: &[u8]) -> Option<Vec<(PhotoId, Arc<Photo>)>> {
    let _scope = Scope::enter();
    let map: BTreeMap<PhotoId, Arc<Photo>> = serde_json::from_slice(&with_references(piece)).ok()?;
    // (every photo carries the id it is filed under, which `Catalog::repair` then need not check: a file where one does not
    // is read the plain way, and repaired there)
    map.iter().all(|(id, p)| p.id == *id).then(|| map.into_iter().collect())
}

/// What comes before a photo's develop settings, and after them (the next member, in the order photos are written).
const DEVELOP: &[u8] = b"\"develop\":";
const EDITED: &[u8] = b",\"edited\":";

/// A hash of `text` to find equal settings by (equal texts are compared in full, so only speed depends on it).
fn hash(text: &[u8]) -> u64 {
    const K: u64 = 0x517c_c1b7_2722_0a95;
    let (words, rest) = text.as_chunks::<8>();
    let tail = rest.iter().fold(0u64, |t, b| (t << 8) | u64::from(*b));
    words.iter().map(|w| u64::from_le_bytes(*w)).chain([tail]).fold(0u64, |h, w| (h.rotate_left(5) ^ w).wrapping_mul(K))
}

/// `piece` as the JSON object of its entries, with each develop settings text (the first of its kind stays, as
/// `[n,{…}]`) replaced, where it comes again, by `[n]`.
fn with_references(piece: &[u8]) -> Vec<u8> {
    let (start, end) = (memmem::Finder::new(DEVELOP), memmem::Finder::new(EDITED));
    let mut object = Vec::with_capacity(piece.len() / 2 + 2);
    object.push(b'{');
    // the texts met so far: by hash, where each is in `piece` (and its number)
    let mut seen: HashMap<u64, Vec<(usize, usize, usize)>> = HashMap::new();
    let mut distinct = 0;
    let mut at = 0;
    while let Some(i) = start.find(piece.get(at..).unwrap_or_default()) {
        let from = at + i + DEVELOP.len();
        let Some(len) = end.find(piece.get(from..).unwrap_or_default()) else { break };
        let to = from + len;
        let text = piece.get(from..to).unwrap_or_default();
        object.extend_from_slice(piece.get(at..from).unwrap_or_default());
        let same = seen.entry(hash(text)).or_default();
        match same.iter().find(|(f, t, _)| piece.get(*f..*t) == Some(text)) {
            Some((_, _, n)) => object.extend_from_slice(format!("[{n}]").as_bytes()),
            None => {
                same.push((from, to, distinct));
                object.extend_from_slice(format!("[{distinct},").as_bytes());
                object.extend_from_slice(text);
                object.push(b']');
                distinct += 1;
            }
        }
        at = to;
    }
    object.extend_from_slice(piece.get(at..).unwrap_or_default());
    object.push(b'}');
    object
}

thread_local! {
    /// The settings a piece introduced as `[n,{…}]`, for the `[n]` that follow; `None` outside the reading of a piece
    /// (there, develop settings written as a list are refused).
    static SHARED: RefCell<Option<HashMap<u64, Arc<DevelopSettings>>>> = const { RefCell::new(None) };
}

/// The reading of one piece on this thread, as far as [`develop`] is concerned.
struct Scope;

impl Scope {
    fn enter() -> Scope {
        SHARED.with(|t| *t.borrow_mut() = Some(HashMap::new()));
        Scope
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        SHARED.with(|t| *t.borrow_mut() = None);
    }
}

/// `#[serde(deserialize_with)]` for [`Photo::develop`]: the settings object, or what [`with_references`] made of it.
pub(crate) fn develop<'de, D: Deserializer<'de>>(d: D) -> Result<Arc<DevelopSettings>, D::Error> {
    struct Settings;
    impl<'de> Visitor<'de> for Settings {
        type Value = Arc<DevelopSettings>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("develop settings")
        }
        fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
            DevelopSettings::deserialize(de::value::MapAccessDeserializer::new(map)).map(Arc::new)
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let n: u64 = seq.next_element()?.ok_or_else(|| A::Error::custom("an empty reference to develop settings"))?;
            match seq.next_element::<DevelopSettings>()? {
                Some(settings) => {
                    let settings = Arc::new(settings);
                    SHARED
                        .with(|t| t.borrow_mut().as_mut().map(|t| t.insert(n, settings.clone())))
                        .ok_or_else(|| A::Error::custom("develop settings written as a list"))?;
                    Ok(settings)
                }
                None => SHARED
                    .with(|t| t.borrow().as_ref().and_then(|t| t.get(&n).cloned()))
                    .ok_or_else(|| A::Error::custom("a reference to develop settings that were not given")),
            }
        }
    }
    d.deserialize_any(Settings)
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    memmem::find(hay, needle)
}

/// The last place `needle` starts at, looking from the end.
fn rfind(hay: &[u8], needle: &[u8]) -> Option<usize> {
    memmem::rfind(hay, needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;
    use lightcraft_develop::DevelopSettings;

    fn catalog(n: u64) -> Catalog {
        let mut c = Catalog::new();
        let ops = (0..n)
            .map(|i| {
                let mut p = Photo::new(PhotoId(i + 1), Source::Demo { scene: 0 }, &format!("IMG_{i}.jpg"), "JPEG", 600, 400, "2026-01-01T00:00:00");
                p.rating = (i % 6) as u8;
                // text that looks like the markers, inside strings (quotes escaped, as JSON has them)
                p.meta.caption = format!("a\":{{\"id\":{i}, b\"}},\"albums\":\"develop\":{{}},\"edited\":");
                // a few distinct settings among many equal ones
                let mut d = DevelopSettings::default();
                d.light.exposure = (i % 4) as f64 * 0.25;
                p.develop = Arc::new(d);
                Op::AddPhoto { photo: Box::new(p) }
            })
            .chain([Op::AddAlbum { album: Album::new(AlbumId(1), "Trip") }])
            .collect();
        c.apply(Op::Batch { ops }).unwrap();
        c
    }

    /// A reader over bytes in memory.
    fn memory(bytes: &[u8]) -> (u64, RangeReader) {
        let size = bytes.len();
        let bytes = bytes.to_vec();
        let read: RangeReader = Arc::new(move |offset, len| {
            let from = usize::try_from(offset).unwrap_or(usize::MAX).min(bytes.len());
            let to = from.saturating_add(len).min(bytes.len());
            Ok(bytes.get(from..to).unwrap_or_default().to_vec())
        });
        (size as u64, read)
    }

    fn read_bytes(bytes: &[u8], s: Settings) -> Option<SnapFile> {
        let (size, read_at) = memory(bytes);
        read_with(size, &read_at, s)
    }

    fn snapshot(c: &Catalog) -> Vec<u8> {
        format!("{{\"format\":\"lightcraft-catalog\",\"version\":11,\"seq\":7,\"catalog\":{}}}\n", c.to_snapshot()).into_bytes()
    }

    #[test]
    fn equal_settings_come_out_shared_and_references_without_a_source_are_errors() {
        let bytes = snapshot(&catalog(120));
        let fast = read_bytes(&bytes, Settings { piece_bytes: 1 << 20, threads: 2, ..SMALL }).unwrap();
        let shared = fast.catalog.photos().filter(|p| Arc::ptr_eq(&p.develop, &fast.catalog.photo(PhotoId(1)).unwrap().develop)).count();
        assert!(shared > 1, "photos with equal settings share them");
        // outside a piece a reference means nothing; it is an error, never a panic or somebody else's settings
        let json = serde_json::to_string(&**fast.catalog.photo(PhotoId(1)).unwrap()).unwrap();
        let tampered = json.replacen("\"develop\":{", "\"develop\":[7,{", 1);
        assert!(serde_json::from_str::<Photo>(&tampered).is_err());
        let dangling = {
            let at = json.find("\"develop\":").unwrap() + 10;
            let end = json.find(",\"edited\":").unwrap();
            format!("{}[7]{}", &json[..at], &json[end..])
        };
        assert!(serde_json::from_str::<Photo>(&dangling).is_err());
        assert!(serde_json::from_str::<Photo>(&json).is_ok());
    }

    const SMALL: Settings = Settings { min_bytes: 0, piece_bytes: 4096, threads: 4 };

    #[test]
    fn reads_the_same_catalog_as_the_plain_read() {
        for n in [1, 2, 3, 50, 700] {
            let bytes = snapshot(&catalog(n));
            let plain: SnapFile = serde_json::from_slice(&bytes).unwrap();
            let fast = read_bytes(&bytes, SMALL).unwrap_or_else(|| panic!("{n} photos"));
            assert_eq!(fast.seq, 7);
            assert_eq!(fast.catalog, plain.catalog, "{n} photos");
            assert_eq!(fast.catalog.len(), n as usize);
        }
    }

    #[test]
    fn small_or_odd_snapshots_are_left_to_the_plain_read() {
        let bytes = snapshot(&catalog(300));
        assert!(read_bytes(&bytes, Settings { min_bytes: bytes.len() + 1, ..SMALL }).is_none(), "below the size");
        assert!(read_bytes(&bytes, Settings { threads: 1, ..SMALL }).is_none(), "one thread");
        assert!(read_bytes(b"", SMALL).is_none() && read_bytes(b"{", SMALL).is_none() && read_bytes(b"not json at all", SMALL).is_none());
        // damage inside the photos: the pieces do not parse, so the plain read gets to say why
        let mut bad = bytes.clone();
        let at = find(&bad[bad.len() / 2..], b"\"rating\":").map(|i| i + bad.len() / 2).unwrap();
        bad[at + 9] = b'#';
        assert!(read_bytes(&bad, SMALL).is_none());
        // a photo filed under another id than its own: the plain read repairs that, so the pieces leave it to it
        let text = String::from_utf8(bytes.clone()).unwrap();
        let misfiled = text.replacen("\"150\":{\"id\":150,", "\"150\":{\"id\":151,", 1);
        assert_ne!(misfiled, text);
        assert!(read_bytes(misfiled.as_bytes(), SMALL).is_none());
        // truncated, and the albums gone
        assert!(read_bytes(bytes.get(..bytes.len() / 2).unwrap_or_default(), SMALL).is_none());
    }
}

#[cfg(test)]
mod big_snapshot_test {
    use std::sync::Arc;

    use lightcraft_develop::DevelopSettings;

    use crate::journal::{Journal, SNAPSHOT};
    use crate::*;

    /// A library whose snapshot is past the size where it is read in pieces: opened from a folder, it comes out as
    /// the plain read gives it, and the pieces were what read it.
    #[test]
    fn a_big_snapshot_opens_in_pieces_and_equals_the_plain_read() {
        let dir = std::env::temp_dir().join(format!("lc-snapload-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut live = Catalog::new();
        let ops = (0..9000u64)
            .map(|i| {
                let mut p = Photo::new(PhotoId(i + 1), Source::Demo { scene: 0 }, &format!("IMG_{i}.jpg"), "JPEG", 600, 400, "2026-01-01T00:00:00");
                let mut d = DevelopSettings::default();
                d.light.exposure = (i % 13) as f64 * 0.1;
                p.develop = Arc::new(d);
                p.rating = (i % 6) as u8;
                Op::AddPhoto { photo: Box::new(p) }
            })
            .collect();
        live.apply(Op::Batch { ops }).unwrap();
        let (mut journal, ..) = Journal::open(Box::new(FsStore::open(&dir).unwrap())).unwrap();
        journal.snapshot(&live).unwrap();
        drop(journal);
        let size = std::fs::metadata(dir.join(SNAPSHOT)).unwrap().len();
        assert!(size > super::MIN_BYTES as u64, "the snapshot is {size} bytes: make the test library bigger");

        let store = FsStore::open(&dir).unwrap();
        let (read, version) = Journal::read_in_pieces(&store).expect("read in pieces");
        assert_eq!((read.seq, version), (0, crate::journal::VERSION));
        let plain: crate::journal::SnapFile = serde_json::from_slice(&std::fs::read(dir.join(SNAPSHOT)).unwrap()).unwrap();
        assert!(read.catalog == plain.catalog, "the pieces read another catalog than the plain read");
        let (_, opened, report) = Journal::open(Box::new(store)).unwrap();
        assert_eq!((opened.len(), report.created, report.replayed), (9000, false, 0));
        assert!(opened.to_snapshot() == live.to_snapshot(), "the opened library is not the saved one");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
