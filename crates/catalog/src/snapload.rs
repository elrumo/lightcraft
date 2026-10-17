//! Reading a big `catalog.snap` on several threads.
//!
//! A snapshot is one JSON object whose first member is the photos map: `{"photos":{"<id>":{"id":…},…},"albums":…}`,
//! a few KB per photo, hundreds of MB for a big library, and serde reads it on one thread. This cuts
//! the photos map at entry boundaries, reads the pieces on worker threads and the rest (albums,
//! stacks, counters: small) in one go. It is an optimisation of [`serde_json::from_slice`] and gives
//! the same catalog. It answers `None`, and the caller reads the snapshot the plain way, for a small
//! snapshot, a layout it does not recognise, or a piece that does not parse (so every error still
//! comes from the plain read).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::journal::SnapFile;
use crate::model::{Photo, PhotoId};

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

/// The snapshot read on several threads, or `None` (read it the plain way).
pub(crate) fn read(bytes: &[u8]) -> Option<SnapFile> {
    // (the web build has no threads)
    if cfg!(target_arch = "wasm32") {
        return None;
    }
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(16);
    read_with(bytes, Settings { min_bytes: MIN_BYTES, piece_bytes: PIECE_BYTES, threads })
}

#[derive(Clone, Copy)]
pub(crate) struct Settings {
    pub min_bytes: usize,
    pub piece_bytes: usize,
    pub threads: usize,
}

pub(crate) fn read_with(bytes: &[u8], s: Settings) -> Option<SnapFile> {
    if bytes.len() < s.min_bytes || s.threads < 2 {
        return None;
    }
    let head = bytes.get(..bytes.len().min(256))?;
    let first = find(head, PHOTOS)? + PHOTOS.len();
    // the `}` that closes the photos map (the albums follow it; found from the end, where the albums are)
    let end = rfind(bytes, AFTER)?;
    let span = end.checked_sub(first).filter(|n| *n > 0)?;
    let pieces = (span / s.piece_bytes.max(1)).max(s.threads);
    // where each piece starts: the opening quote of an entry's key, found past an even cut
    let mut starts = vec![first];
    for k in 1..pieces {
        let cut = first.checked_add(span / pieces * k)?;
        // (none: the cut is inside the last entry)
        let Some(open) = next_entry(bytes, first, cut, end.min(cut.saturating_add(LOOK))) else { continue };
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
                        mine.push((i, entries(bytes.get(from..to)?)?));
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
    let mut rest = Vec::with_capacity(first + 1 + bytes.len().saturating_sub(end));
    rest.extend_from_slice(bytes.get(..first)?);
    rest.push(b'}');
    rest.extend_from_slice(bytes.get(end + 1..)?);
    let mut snap: SnapFile = serde_json::from_slice(&rest).ok()?;
    snap.catalog.photos = photos;
    Some(snap)
}

/// The opening quote of the first entry's key that starts in `bytes[from..to]` (`first` is where the
/// photos map starts): `,"<id>":{"id":<id>,`. Other `":{"id":` in an entry (a nested `"profile":{"id":…`) are
/// passed over.
fn next_entry(bytes: &[u8], first: usize, from: usize, to: usize) -> Option<usize> {
    let mut at = from;
    loop {
        let key_end = at + find(bytes.get(at..to)?, ENTRY)?;
        at = key_end + 1;
        let digits = bytes.get(first..key_end)?.iter().rev().take_while(|b| b.is_ascii_digit()).count();
        let Some(open) = key_end.checked_sub(digits + 1).filter(|_| digits > 0) else { continue };
        let id = bytes.get(open + 1..key_end)?;
        let value = bytes.get(key_end + ENTRY.len()..)?;
        if bytes.get(open) == Some(&b'"') && bytes.get(open.checked_sub(1)?) == Some(&b',') && value.starts_with(id) && value.get(id.len()) == Some(&b',') {
            return Some(open);
        }
    }
}

/// The entries `"<id>":{…},"<id>":{…}` of one piece.
fn entries(piece: &[u8]) -> Option<Vec<(PhotoId, Arc<Photo>)>> {
    let mut object = Vec::with_capacity(piece.len() + 2);
    object.push(b'{');
    object.extend_from_slice(piece);
    object.push(b'}');
    let map: BTreeMap<PhotoId, Arc<Photo>> = serde_json::from_slice(&object).ok()?;
    Some(map.into_iter().collect())
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// The last place `needle` starts at, looking from the end.
fn rfind(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).rposition(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;

    fn catalog(n: u64) -> Catalog {
        let mut c = Catalog::new();
        let ops = (0..n)
            .map(|i| {
                let mut p = Photo::new(PhotoId(i + 1), Source::Demo { scene: 0 }, &format!("IMG_{i}.jpg"), "JPEG", 600, 400, "2026-01-01T00:00:00");
                p.rating = (i % 6) as u8;
                // text that looks like the markers, inside strings (quotes escaped, as JSON has them)
                p.meta.caption = format!("a\":{{\"id\":{i}, b\"}},\"albums\":");
                p.develop = Arc::new(Default::default());
                Op::AddPhoto { photo: Box::new(p) }
            })
            .chain([Op::AddAlbum { album: Album::new(AlbumId(1), "Trip") }])
            .collect();
        c.apply(Op::Batch { ops }).unwrap();
        c
    }

    fn snapshot(c: &Catalog) -> Vec<u8> {
        format!("{{\"format\":\"lightcraft-catalog\",\"version\":11,\"seq\":7,\"catalog\":{}}}\n", c.to_snapshot()).into_bytes()
    }

    const SMALL: Settings = Settings { min_bytes: 0, piece_bytes: 4096, threads: 4 };

    #[test]
    fn reads_the_same_catalog_as_the_plain_read() {
        for n in [1, 2, 3, 50, 700] {
            let bytes = snapshot(&catalog(n));
            let plain: SnapFile = serde_json::from_slice(&bytes).unwrap();
            let fast = read_with(&bytes, SMALL).unwrap_or_else(|| panic!("{n} photos"));
            assert_eq!(fast.seq, 7);
            assert_eq!(fast.catalog, plain.catalog, "{n} photos");
            assert_eq!(fast.catalog.len(), n as usize);
        }
    }

    #[test]
    fn small_or_odd_snapshots_are_left_to_the_plain_read() {
        let bytes = snapshot(&catalog(300));
        assert!(read_with(&bytes, Settings { min_bytes: bytes.len() + 1, ..SMALL }).is_none(), "below the size");
        assert!(read_with(&bytes, Settings { threads: 1, ..SMALL }).is_none(), "one thread");
        assert!(read_with(b"", SMALL).is_none() && read_with(b"{", SMALL).is_none() && read_with(b"not json at all", SMALL).is_none());
        // damage inside the photos: the pieces do not parse, so the plain read gets to say why
        let mut bad = bytes.clone();
        let at = find(&bad[bad.len() / 2..], b"\"rating\":").map(|i| i + bad.len() / 2).unwrap();
        bad[at + 9] = b'#';
        assert!(read_with(&bad, SMALL).is_none());
        // truncated, and the albums gone
        assert!(read_with(bytes.get(..bytes.len() / 2).unwrap_or_default(), SMALL).is_none());
    }
}
