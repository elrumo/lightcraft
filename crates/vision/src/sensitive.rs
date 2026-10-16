//! How likely each photo is to be sensitive (nudity and the like), as a [`Classifier`] scored it: one
//! record per photo in an append-only file (the same kind of store as the search vectors and the faces).
//! Derived data like them: keyed by content, never in the catalog, deleted and made again at will.

use std::collections::HashMap;
use std::path::Path;

use lightcraft_raster::Rgba8;

use crate::store::{Error, KEY_LEN, Key, RecordFile, Spec};

/// The model that tells sensitive photos apart, whatever runs it.
pub trait Classifier: Send + Sync {
    /// Which model and version (the store's model id): scores of another one aren't mixed in.
    fn engine(&self) -> &str;
    /// How likely the photo (display-referred, any size) is to be sensitive, from 0 to 1.
    fn score(&self, img: &Rgba8) -> Result<f32, Error>;
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
        Ok(SensitiveIndex { file: Some(file), scores, revision: 0 })
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
