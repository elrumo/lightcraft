//! 128-bit hashing. Not cryptographic: it identifies files for duplicate detection and keys caches;
//! collisions are astronomically unlikely by accident.
//!
//! Two functions, both shown as 32 hex digits:
//! - [`Hasher128`] / [`hash_bytes`]: SipHash-1-3 with a fixed key. Every cache key made from a hash, and
//!   the content hash of every file hashed before HB-532 (a photo's identity, the server's blob names).
//! - [`content_hash`] / [`ContentHasher`]: XXH3-128 (seed 0, the reference function: `xxhsum -H2` prints the
//!   same digits), several times faster. The content hash of a file hashed from HB-532 on. A file is the
//!   one a stored content hash names when either function gives that hash ([`ContentHasher::is`]).

use std::fmt;
use std::hash::Hasher;

use siphasher::sip128::{Hasher128 as _, SipHasher13};
use twox_hash::XxHash3_128;

/// A 128-bit hash, shown as 32 hex digits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Hash128(pub u128);

impl fmt::Display for Hash128 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:032x}", self.0)
    }
}

impl Hash128 {
    pub fn parse(s: &str) -> Option<Hash128> {
        // `from_str_radix` also accepts a leading '+', so check the digits explicitly.
        (s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit())).then(|| u128::from_str_radix(s, 16).ok().map(Hash128)).flatten()
    }
}

/// Streaming hasher.
#[derive(Clone, Debug)]
pub struct Hasher128(SipHasher13);

impl Default for Hasher128 {
    fn default() -> Self {
        Hasher128(SipHasher13::new_with_keys(0x4c69_6768_7443_7261, 0x6674_2043_6f6e_7465))
    }
}

impl Hasher128 {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn update(&mut self, bytes: &[u8]) -> &mut Self {
        self.0.write(bytes);
        self
    }
    /// Length-prefixed string (so `("ab","c")` and `("a","bc")` differ).
    pub fn str(&mut self, s: &str) -> &mut Self {
        self.0.write(&(s.len() as u64).to_le_bytes());
        self.0.write(s.as_bytes());
        self
    }
    pub fn u64(&mut self, v: u64) -> &mut Self {
        self.0.write(&v.to_le_bytes());
        self
    }
    pub fn finish(&self) -> Hash128 {
        Hash128(self.0.finish128().as_u128())
    }
}

/// Hash of a whole byte string.
pub fn hash_bytes(bytes: &[u8]) -> Hash128 {
    Hasher128::new().update(bytes).finish()
}

/// The content hash a file's bytes get from HB-532 on (XXH3-128).
pub fn content_hash(bytes: &[u8]) -> Hash128 {
    Hash128(XxHash3_128::oneshot(bytes))
}

/// A file's content hash, streamed: the new one ([`content_hash`]) and, when asked, the old one
/// ([`hash_bytes`]) from the same pass, for files that may be stored under it.
#[derive(Clone)]
pub struct ContentHasher {
    new: XxHash3_128,
    old: Option<Hasher128>,
}

impl ContentHasher {
    /// Only the new hash.
    pub fn new() -> Self {
        ContentHasher { new: XxHash3_128::new(), old: None }
    }
    /// Both hashes: to check a file against a stored hash, which either function may have made.
    pub fn both() -> Self {
        ContentHasher { new: XxHash3_128::new(), old: Some(Hasher128::new()) }
    }
    pub fn update(&mut self, bytes: &[u8]) -> &mut Self {
        self.new.write(bytes);
        if let Some(old) = &mut self.old {
            old.update(bytes);
        }
        self
    }
    /// The new hash.
    pub fn finish(&self) -> Hash128 {
        Hash128(self.new.finish_128())
    }
    /// The old hash (`None`: not asked for).
    pub fn finish_old(&self) -> Option<Hash128> {
        self.old.as_ref().map(Hasher128::finish)
    }
    /// Are the bytes given so far the file `hash` names (32 hex digits, any case), by either function?
    /// (Without [`ContentHasher::both`], only the new one is checked.)
    pub fn is(&self, hash: &str) -> bool {
        let Some(want) = Hash128::parse(hash) else { return false };
        self.finish() == want || self.finish_old() == Some(want)
    }
}

impl Default for ContentHasher {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for ContentHasher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ContentHasher").field("old", &self.old.is_some()).finish_non_exhaustive()
    }
}

/// Are `bytes` the file `hash` names, by either function? (The old hash is only computed when the new one
/// isn't it.)
pub fn content_is(bytes: &[u8], hash: &str) -> bool {
    Hash128::parse(hash).is_some_and(|want| content_hash(bytes) == want || hash_bytes(bytes) == want)
}

/// The content hashes a library has, with the sizes of their files: what a file hashed now is checked against, so that a
/// file the library already has keeps the hash it has there (HB-532: photos hashed before keep the old hash forever).
///
/// A file gets the hash new files get in this library (the new one; the old one while [`KnownHashes::prefer_old`], for a
/// library that syncs with a server knowing only the old). When a photo has the same file size, the file is hashed the
/// other way too and takes whichever hash a photo has. Two copies of one file always have one size, so a file the
/// library has is always found, and only a size match costs the second hash.
#[derive(Clone, Debug, Default)]
pub struct KnownHashes {
    /// (file size, content hash), sorted (shared: a library may hold millions).
    known: std::sync::Arc<[(u64, u128)]>,
    prefer_old: bool,
}

impl KnownHashes {
    /// From each photo's file size and content hash (32 hex digits; a suffix from `:` on is not part of it; anything else
    /// is skipped). `prefer_old`: new files get the old hash.
    pub fn new<'a>(photos: impl IntoIterator<Item = (u64, &'a str)>, prefer_old: bool) -> Self {
        let mut known: Vec<(u64, u128)> = photos.into_iter().filter_map(|(size, h)| Some((size, Hash128::parse(h.split(':').next()?)?.0))).collect();
        known.sort_unstable();
        known.dedup();
        KnownHashes { known: known.into(), prefer_old }
    }

    /// Do new files get the old hash?
    pub fn prefer_old(&self) -> bool {
        self.prefer_old
    }

    /// The same library, where new files get the old hash.
    pub fn preferring_old(&self) -> KnownHashes {
        KnownHashes { known: self.known.clone(), prefer_old: true }
    }

    fn of_size(&self, size: u64) -> &[(u64, u128)] {
        let from = self.known.partition_point(|&(s, _)| s < size);
        let to = self.known.partition_point(|&(s, _)| s <= size);
        self.known.get(from..to).unwrap_or(&[])
    }

    /// A hasher for a file of `size` bytes: both hashes when a photo has that size (or new files get the old one),
    /// else the new one only. Its result is [`KnownHashes::finish`].
    pub fn hasher(&self, size: u64) -> ContentHasher {
        if self.prefer_old || !self.of_size(size).is_empty() { ContentHasher::both() } else { ContentHasher::new() }
    }

    /// The content hash of a file of `size` bytes, hashed by [`KnownHashes::hasher`], in this library.
    pub fn finish(&self, h: &ContentHasher, size: u64) -> Hash128 {
        let new = h.finish();
        let Some(old) = h.finish_old() else { return new };
        let (first, second) = if self.prefer_old { (old, new) } else { (new, old) };
        let has = |x: Hash128| self.of_size(size).iter().any(|&(_, k)| k == x.0);
        if !has(first) && has(second) { second } else { first }
    }

    /// The content hash `bytes` get in this library.
    pub fn hash(&self, bytes: &[u8]) -> Hash128 {
        let size = bytes.len() as u64;
        let mut h = self.hasher(size);
        h.update(bytes);
        self.finish(&h, size)
    }
}

/// Feed all `r` gives into `h`, a megabyte at a time (an interrupted read is tried again).
pub fn read_into(mut r: impl std::io::Read, h: &mut ContentHasher) -> std::io::Result<()> {
    let mut buf = vec![0u8; 1 << 20];
    loop {
        match r.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(n) => {
                h.update(buf.get(..n).unwrap_or(&[]));
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
}

/// `h` fed with the file at `path`: a regular file, read no further than its size when opened (a FIFO or a
/// device would never end).
pub fn read_file_into(path: &std::path::Path, h: &mut ContentHasher) -> std::io::Result<()> {
    use std::io::Read as _;
    let f = std::fs::File::open(path)?;
    let meta = f.metadata()?;
    if !meta.is_file() {
        return Err(std::io::Error::other(format!("{} is not a file", path.display())));
    }
    read_into(f.take(meta.len()), h)
}

/// Is the file at `path` the one `hash` names, by either function? It is read once for the new hash and, only
/// when that isn't it, once more for the old one (a file hashed before HB-532).
pub fn file_is(path: &std::path::Path, hash: &str) -> std::io::Result<bool> {
    let mut h = ContentHasher::new();
    read_file_into(path, &mut h)?;
    if h.is(hash) {
        return Ok(true);
    }
    let mut old = ContentHasher::both();
    read_file_into(path, &mut old)?;
    Ok(old.is(hash))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rejects_signed_and_non_hex() {
        assert_eq!(Hash128::parse(&format!("+{}", "1".repeat(31))), None);
        assert_eq!(Hash128::parse(&format!("-{}", "1".repeat(31))), None);
        let h = "0123456789abcdefABCDEF0123456789";
        assert_eq!(Hash128::parse(h).map(|x| x.to_string()), Some(h.to_lowercase()));
    }

    /// `xxh128sum` 0.8.3 (the reference implementation) of the same bytes: empty, short, one block and a bit, many blocks.
    #[test]
    fn content_hash_is_the_reference_xxh3_128() {
        let block: Vec<u8> = (0..1280u32).map(|i| (i & 255) as u8).collect();
        let big: Vec<u8> = (0..5 * 1_048_576 + 13u32).map(|i| (i.wrapping_mul(7).wrapping_add(3) & 255) as u8).collect();
        for (bytes, want) in [
            (&b""[..], "99aa06d3014798d86001c324468d497f"),
            (b"LightCraft", "24a5f15ffaa13d2d2103da1b2d96922d"),
            (&block, "d92ab3a1cb0542a74844b009e164352e"),
            (&big, "29b95bcd260ef0d21942774b1212065c"),
        ] {
            assert_eq!(content_hash(bytes).to_string(), want, "{} bytes", bytes.len());
            // streamed in pieces that cross every boundary, both hashes in one pass
            let mut h = ContentHasher::both();
            for piece in bytes.chunks(1000) {
                h.update(piece);
            }
            assert_eq!(h.finish().to_string(), want);
            assert_eq!(h.finish_old(), Some(hash_bytes(bytes)));
            assert_eq!(ContentHasher::new().update(bytes).finish_old(), None);
        }
    }

    /// `xxh128sum` of `i % 251` for `i` in `0..n`: every length class (the Swift app's port checks the same table).
    #[test]
    fn every_length_class_is_the_reference_hash() {
        for (n, want) in [
            (1, "a6cd5e9392000f6ac44bdff4074eecdb"),
            (3, "e3b55f57945a17cf5f4299fc161c9cbb"),
            (4, "eb70bf5fc779e9e6a6111d53e80a3db5"),
            (8, "e1e4432a62217fe4cfd50c61c8bb98c1"),
            (9, "16c769d83e4aebce907931979dca3746"),
            (16, "72950631827607e2842812cc870dcae2"),
            (17, "685bc458b37d057fc06e233df7729217"),
            (128, "14792fc3af88dc6c05321a0b64d67b41"),
            (129, "dd5e74ac6b45f54ebc30b63382b09a3b"),
            (240, "65b5be86da5540e7c92b68e16f83bbb6"),
            (241, "1da1cb61bcb8a2a102e8cd95421c6d02"),
            (256, "96c36c85d00e5bc544f5d90dacde463a"),
            (257, "8c650dc0594ae28188fc3f7934a6c9be"),
            (1024, "d0ac1f7b93bf57b9e5d78bafa45b2aa5"),
            (1088, "295519bc52981c5c1a848d807034c403"),
            (4097, "0f77b4bc73e3337db69d29f17d48293f"),
        ] {
            let bytes: Vec<u8> = (0..n).map(|i| (i % 251) as u8).collect();
            assert_eq!(content_hash(&bytes).to_string(), want, "{n} bytes");
        }
    }

    #[test]
    fn a_file_is_the_one_either_hash_names() {
        let (old, new) = (hash_bytes(b"pixels").to_string(), content_hash(b"pixels").to_string());
        assert_ne!(old, new);
        for name in [&old, &new, &new.to_uppercase()] {
            assert!(content_is(b"pixels", name), "{name}");
            assert!(ContentHasher::both().update(b"pixels").is(name), "{name}");
        }
        // the new-only stream knows only the new hash
        assert!(ContentHasher::new().update(b"pixels").is(&new));
        assert!(!ContentHasher::new().update(b"pixels").is(&old));
        // other bytes, a hash with a suffix or not a hash: no
        for name in [hash_bytes(b"other").to_string(), content_hash(b"other").to_string(), format!("{new}:dup3"), String::new(), "zz".repeat(16)] {
            assert!(!content_is(b"pixels", &name), "{name}");
            assert!(!ContentHasher::both().update(b"pixels").is(&name), "{name}");
        }
    }

    /// A reader that gives its bytes a few at a time, and an interruption in between.
    struct Trickle<'a>(&'a [u8], bool);

    impl std::io::Read for Trickle<'_> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.1 = !self.1;
            if self.1 {
                return Err(std::io::ErrorKind::Interrupted.into());
            }
            let n = self.0.len().min(buf.len()).min(7);
            buf[..n].copy_from_slice(&self.0[..n]);
            self.0 = &self.0[n..];
            Ok(n)
        }
    }

    #[test]
    fn a_hash_read_in_pieces_is_the_hash_of_the_whole() {
        // longer than one piece, so the read loop runs more than once
        let bytes: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
        for part in [&bytes[..], &bytes[..1000], &[]] {
            let mut h = ContentHasher::both();
            let r = if part.len() == 1000 { read_into(Trickle(part, false), &mut h) } else { read_into(part, &mut h) };
            r.unwrap();
            assert_eq!((h.finish(), h.finish_old()), (content_hash(part), Some(hash_bytes(part))), "{} bytes", part.len());
        }
    }

    #[test]
    fn a_file_keeps_the_hash_its_library_has() {
        let (file, other) = (b"raw file".as_slice(), b"raw fil!".as_slice());
        let (old, new) = (hash_bytes(file), content_hash(file));
        let size = file.len() as u64;
        let lib = |photos: &[(u64, String)], prefer_old| KnownHashes::new(photos.iter().map(|(s, h)| (*s, h.as_str())), prefer_old);
        // a new file: the new hash, and only that is computed
        assert_eq!(lib(&[], false).hash(file), new);
        assert_eq!(lib(&[(size + 1, old.to_string())], false).hasher(size).finish_old(), None);
        // the library had it before HB-532: it keeps the old hash (a virtual copy's suffix is not part of the hash)
        assert_eq!(lib(&[(size, old.to_string())], false).hash(file), old);
        assert_eq!(lib(&[(size, format!("{old}:dup7"))], false).hash(file), old);
        // another photo of the same size: the new hash
        assert_eq!(lib(&[(size, hash_bytes(other).to_string())], false).hash(file), new);
        // the file under its new hash: that
        assert_eq!(lib(&[(size, new.to_string()), (size, hash_bytes(other).to_string())], false).hash(file), new);
        // signed in to a server that knows only the old hash: new files get the old one, a file the library has keeps its own
        assert_eq!(lib(&[], true).hash(file), old);
        assert_eq!(lib(&[], false).preferring_old().hash(file), old);
        assert_eq!(lib(&[(size, new.to_string())], false).preferring_old().hash(file), new);
        assert_eq!(lib(&[(size, new.to_string())], true).hash(file), new);
        assert_eq!(lib(&[(size, "not a hash".into())], true).hash(file), old);
        // streamed in pieces: the same answer
        let k = lib(&[(size, old.to_string())], false);
        let mut h = k.hasher(size);
        h.update(&file[..3]).update(&file[3..]);
        assert_eq!(k.finish(&h, size), old);
    }

    #[test]
    fn a_file_on_disk_is_checked_by_either_hash() {
        let dir = std::env::temp_dir().join(format!("lc-file-is-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.bin");
        let bytes: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&f, &bytes).unwrap();
        assert!(file_is(&f, &content_hash(&bytes).to_string()).unwrap());
        assert!(file_is(&f, &hash_bytes(&bytes).to_string()).unwrap());
        assert!(!file_is(&f, &content_hash(b"other").to_string()).unwrap());
        assert!(file_is(&dir.join("nope"), &content_hash(&bytes).to_string()).is_err());
        assert!(file_is(&dir, &content_hash(&bytes).to_string()).is_err(), "a folder is not a file");
        #[cfg(unix)]
        assert!(file_is(std::path::Path::new("/dev/zero"), &content_hash(b"").to_string()).is_err(), "a device never ends");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
