//! How likely each photo is to be sensitive (nudity and the like), as a [`Classifier`] scored it: one
//! record per photo in an append-only file (the same kind of store as the search vectors and the faces).
//! Derived data like them: keyed by content, never in the catalog, deleted and made again at will.

use std::collections::HashMap;
use std::path::Path;

use lightcraft_raster::Rgba8;

use crate::index::Import;
use crate::store::{Error, HEADER_LEN, KEY_LEN, Key, RecordFile, Spec};

/// The model that tells sensitive photos apart, whatever runs it.
pub trait Classifier: Send + Sync {
    /// Which model and version (the store's model id): scores of another one aren't mixed in.
    fn engine(&self) -> &str;
    /// How likely the photo (display-referred, any size) is to be sensitive, from 0 to 1.
    fn score(&self, img: &Rgba8) -> Result<f32, Error>;
    /// What it runs on: `cpu` or `gpu`.
    fn backend(&self) -> &'static str {
        "cpu"
    }
}

/// Bytes of a record: the key, then the score (f32, little-endian).
pub const REC_LEN: usize = KEY_LEN + 4;

fn spec(engine: &str) -> Spec<'_> {
    Spec { model: engine, dim: 1, rec_len: REC_LEN }
}

/// Every photo's score.
#[derive(Debug, Default)]
pub struct SensitiveIndex {
    /// `None`: in memory only.
    file: Option<RecordFile>,
    scores: HashMap<Key, f32>,
    /// Bumped by every change (callers cache what they derive from the scores).
    revision: u64,
    /// The model of the scores (empty: an index from [`SensitiveIndex::memory`], which has none to send).
    engine: String,
}

impl SensitiveIndex {
    /// In memory only (tests, a device that only reads the server's).
    pub fn memory() -> SensitiveIndex {
        SensitiveIndex::default()
    }

    /// Opens (or creates) the scores of model `engine` at `path`. A file of another model is an error.
    pub fn open(path: &Path, engine: &str) -> Result<SensitiveIndex, Error> {
        let mut scores = HashMap::new();
        let file = RecordFile::open(path, &spec(engine), |rec| {
            let (Some(key), Some(score)) = (Key::read(rec), rec.get(KEY_LEN..REC_LEN).and_then(|b| <[u8; 4]>::try_from(b).ok())) else { return };
            let score = f32::from_le_bytes(score);
            // (a later record of a photo replaces an earlier one)
            if score.is_finite() {
                scores.insert(key, score.clamp(0.0, 1.0));
            }
        })?;
        Ok(SensitiveIndex { file: Some(file), scores, revision: 0, engine: engine.to_string() })
    }

    /// In memory only, for scores of model `engine` (what a device packs to send a server).
    pub fn in_memory(engine: &str) -> SensitiveIndex {
        SensitiveIndex { engine: engine.to_string(), ..SensitiveIndex::default() }
    }

    pub fn len(&self) -> usize {
        self.scores.len()
    }
    pub fn is_empty(&self) -> bool {
        self.scores.is_empty()
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn contains(&self, key: &Key) -> bool {
        self.scores.contains_key(key)
    }
    pub fn score(&self, key: &Key) -> Option<f32> {
        self.scores.get(key).copied()
    }
    /// The photos scored.
    pub fn keys(&self) -> impl Iterator<Item = Key> + '_ {
        self.scores.keys().copied()
    }

    /// Keep `score` for `key` (written to the file first: a failed write keeps nothing).
    pub fn insert(&mut self, key: Key, score: f32) -> Result<(), Error> {
        if !score.is_finite() {
            return Err(Error::Invalid("score"));
        }
        let score = score.clamp(0.0, 1.0);
        if let Some(file) = self.file.as_mut() {
            let mut rec = Vec::with_capacity(REC_LEN);
            rec.extend_from_slice(&key.0);
            rec.extend_from_slice(&score.to_le_bytes());
            file.append(&rec)?;
        }
        self.scores.insert(key, score);
        self.revision += 1;
        Ok(())
    }

    /// The photos scored at least `min`, most likely first.
    pub fn flagged(&self, min: f32) -> Vec<(Key, f32)> {
        let mut out: Vec<(Key, f32)> = self.scores.iter().filter(|(_, s)| **s >= min).map(|(k, s)| (*k, *s)).collect();
        out.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.0.cmp(&b.0.0)));
        out
    }

    /// A file's worth of records (header first) for the photos `skip` doesn't name, at most `max` of
    /// them: what a device sends a server that lacks them. [`SensitiveIndex::import`] reads it back.
    pub fn export(&self, skip: impl Fn(&Key) -> bool, max: usize) -> Vec<u8> {
        let mut out = spec(&self.engine).header().to_vec();
        let mut keys: Vec<&Key> = self.scores.keys().filter(|k| !skip(k)).collect();
        keys.sort_by_key(|k| k.0);
        for key in keys.into_iter().take(max) {
            out.extend_from_slice(&key.0);
            out.extend_from_slice(&self.scores.get(key).copied().unwrap_or(0.0).to_le_bytes());
        }
        out
    }

    /// Adds the records of a [`SensitiveIndex::export`] for the photos `allow` accepts, skipping those
    /// already scored. All or nothing: another model's header, a truncated body or one score that is not a
    /// number from 0 to 1 rejects the whole thing.
    pub fn import(&mut self, bytes: &[u8], allow: impl Fn(&Key) -> bool) -> Result<Import, Error> {
        spec(&self.engine).check_header(bytes)?;
        let body = bytes.get(HEADER_LEN..).ok_or(Error::Format("short header"))?;
        if !body.len().is_multiple_of(REC_LEN) {
            return Err(Error::Format("truncated record"));
        }
        let mut fresh: Vec<(Key, f32)> = Vec::new();
        let mut out = Import::default();
        for rec in body.as_chunks::<REC_LEN>().0 {
            let (Some(key), Some(raw)) = (Key::read(rec), rec.get(KEY_LEN..REC_LEN).and_then(|b| <[u8; 4]>::try_from(b).ok())) else {
                return Err(Error::Format("truncated record"));
            };
            let score = f32::from_le_bytes(raw);
            if !(0.0..=1.0).contains(&score) {
                return Err(Error::Invalid("score"));
            }
            if self.scores.contains_key(&key) || !allow(&key) || fresh.iter().any(|(k, _)| *k == key) {
                out.skipped += 1;
            } else {
                fresh.push((key, score));
            }
        }
        for (key, score) in fresh {
            self.insert(key, score)?;
            out.added += 1;
        }
        Ok(out)
    }

    /// Forget every score.
    pub fn clear(&mut self) -> Result<(), Error> {
        if let Some(file) = self.file.as_mut() {
            file.truncate()?;
        }
        self.scores.clear();
        self.revision += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(n: u8) -> Key {
        Key([n; KEY_LEN])
    }

    #[test]
    fn scores_travel_between_devices_whole_or_not_at_all() {
        let mut sent = SensitiveIndex::in_memory("m");
        sent.insert(key(1), 0.9).unwrap();
        sent.insert(key(2), 0.1).unwrap();
        let bytes = sent.export(|_| false, 10);
        let mut got = SensitiveIndex::in_memory("m");
        got.insert(key(2), 0.5).unwrap();
        assert_eq!(got.import(&bytes, |_| true).unwrap(), Import { added: 1, skipped: 1 }, "a score already there stays");
        assert_eq!(got.score(&key(2)), Some(0.5));
        assert_eq!(SensitiveIndex::in_memory("m").import(&bytes, |k| k.0[0] == 1).unwrap().added, 1, "only photos allowed");
        assert!(SensitiveIndex::in_memory("other").import(&bytes, |_| true).is_err(), "another model");
        let mut cut = bytes.clone();
        cut.pop();
        assert!(SensitiveIndex::in_memory("m").import(&cut, |_| true).is_err(), "truncated");
        let mut bad = bytes;
        let at = bad.len() - 1;
        bad[at] = 0x7f; // a score of 3.4e38
        let mut none = SensitiveIndex::in_memory("m");
        assert!(none.import(&bad, |_| true).is_err() && none.is_empty(), "one bad score refuses the lot");
    }

    #[test]
    fn scores_are_kept_in_the_file_and_the_flagged_come_most_likely_first() {
        let dir = std::env::temp_dir().join(format!("lc-sensitive-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("sensitive-test.bin");
        let _ = std::fs::remove_file(&path);
        let mut ix = SensitiveIndex::open(&path, "test-model").unwrap();
        ix.insert(key(1), 0.2).unwrap();
        ix.insert(key(2), 0.95).unwrap();
        ix.insert(key(3), 0.8).unwrap();
        ix.insert(key(1), 0.9).unwrap();
        assert!(ix.insert(key(4), f32::NAN).is_err());
        drop(ix);

        let ix = SensitiveIndex::open(&path, "test-model").unwrap();
        assert_eq!(ix.len(), 3);
        let flagged: Vec<u8> = ix.flagged(0.85).iter().map(|(k, _)| k.0[0]).collect();
        assert_eq!(flagged, [2, 1], "the later score of a photo counts");
        // another model's file is refused, not mixed in
        assert!(matches!(SensitiveIndex::open(&path, "other-model"), Err(Error::Mismatch { .. })));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
