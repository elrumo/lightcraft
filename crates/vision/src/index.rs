//! The embedding index: one unit-length vector per photo (stored as f16, half the disk and RAM of
//! f32), searched by brute force: exact, with no index to corrupt or rebuild. Measured (release,
//! dev laptop): 100k photos × 768 dimensions search in ~90 ms and load in ~0.4 s.
// ponytail: brute force, one thread; chunk the scan across threads, or use an approximate index
// (HNSW), beyond ~1M photos.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::store::{Error, HEADER_LEN, KEY_LEN, Key, RecordFile, Spec};

/// Largest vector accepted (a hostile `dim` can't make a store or an upload allocate wildly).
pub const MAX_DIM: usize = 4096;

/// One search result.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub key: Key,
    /// Cosine similarity with the query.
    pub score: f32,
}

/// What [`EmbeddingIndex::import`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Import {
    pub added: usize,
    /// Records already present, or not allowed.
    pub skipped: usize,
}

#[derive(Debug)]
pub struct EmbeddingIndex {
    file: RecordFile,
    spec_model: String,
    dim: usize,
    keys: Vec<Key>,
    /// `keys.len() × dim` f16 bit patterns, row `i` belonging to `keys[i]`.
    rows: Vec<u16>,
    pos: HashMap<Key, usize>,
}

/// f16 → f32 without a lookup or a library: shift the 15 magnitude bits into f32 position and
/// rescale by 2^112 (the difference of the exponent biases). Exact for zero, subnormals and normal
/// numbers; callers have excluded infinities and NaN.
pub(crate) fn f16_to_f32(h: u16) -> f32 {
    let mag = f32::from_bits(u32::from(h & 0x7fff) << 13) * f32::from_bits(0x7780_0000);
    f32::from_bits(mag.to_bits() | (u32::from(h & 0x8000) << 16))
}

/// Little-endian f16 bit patterns of a record's vector bytes.
fn bits_of(raw: &[u8]) -> Vec<u16> {
    raw.as_chunks::<2>().0.iter().map(|&c| u16::from_le_bytes(c)).collect()
}

fn f16_is_finite(h: u16) -> bool {
    h & 0x7c00 != 0x7c00
}

/// `v` scaled to unit length, or `None` when it has a non-finite entry or no length.
fn normalized(v: &[f32]) -> Option<Vec<f32>> {
    let mut sum = 0f64;
    for &x in v {
        if !x.is_finite() {
            return None;
        }
        sum += f64::from(x) * f64::from(x);
    }
    let n = sum.sqrt();
    (n.is_finite() && n > 1e-12).then(|| v.iter().map(|&x| (f64::from(x) / n) as f32).collect())
}

fn encode(v: &[f32]) -> Option<Vec<u16>> {
    normalized(v).map(|u| u.iter().map(|&x| half::f16::from_f32(x).to_bits()).collect())
}

fn decode(bits: &[u16]) -> Vec<f32> {
    bits.iter().map(|&b| f16_to_f32(b)).collect()
}

/// `a · b`, eight partial sums at a time so the loop vectorises (the f32 sum is otherwise one long
/// dependency chain).
fn dot(row: &[u16], q: &[f32]) -> f32 {
    let mut acc = [0f32; 8];
    let ((rc, rr), (qc, qr)) = (row.as_chunks::<8>(), q.as_chunks::<8>());
    for (r, q) in rc.iter().zip(qc) {
        for ((a, &b), &x) in acc.iter_mut().zip(r).zip(q) {
            *a += f16_to_f32(b) * x;
        }
    }
    let tail: f32 = rr.iter().zip(qr).map(|(&b, &x)| f16_to_f32(b) * x).sum();
    acc.iter().sum::<f32>() + tail
}

impl EmbeddingIndex {
    fn spec(model: &str, dim: usize) -> Result<Spec<'_>, Error> {
        if dim == 0 || dim > MAX_DIM {
            return Err(Error::Invalid("vector size"));
        }
        Ok(Spec { model, dim, rec_len: KEY_LEN + dim * 2 })
    }

    /// Opens (or creates) the index at `path` for `model`'s `dim`-sized vectors. A file written
    /// for another model or size is [`Error::Mismatch`]: the caller deletes it and starts again.
    /// Records that don't decode to a finite, non-zero vector are ignored.
    pub fn open(path: &Path, model: &str, dim: usize) -> Result<EmbeddingIndex, Error> {
        let spec = Self::spec(model, dim)?;
        let (mut keys, mut rows, mut pos) = (Vec::new(), Vec::new(), HashMap::new());
        let file = RecordFile::open(path, &spec, |rec| {
            let (Some(key), Some(body)) = (Key::read(rec), rec.get(KEY_LEN..)) else { return };
            let bits = bits_of(body);
            if bits.len() != dim || !bits.iter().all(|&b| f16_is_finite(b)) || bits.iter().all(|&b| b & 0x7fff == 0) || pos.contains_key(&key) {
                return;
            }
            pos.insert(key, keys.len());
            keys.push(key);
            rows.extend_from_slice(&bits);
        })?;
        Ok(EmbeddingIndex { file, spec_model: model.to_string(), dim, keys, rows, pos })
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    pub fn dim(&self) -> usize {
        self.dim
    }

    pub fn contains(&self, key: &Key) -> bool {
        self.pos.contains_key(key)
    }

    pub fn keys(&self) -> impl Iterator<Item = &Key> {
        self.keys.iter()
    }

    fn record(key: &Key, bits: &[u16]) -> Vec<u8> {
        let mut rec = Vec::with_capacity(KEY_LEN + bits.len() * 2);
        rec.extend_from_slice(&key.0);
        for b in bits {
            rec.extend_from_slice(&b.to_le_bytes());
        }
        rec
    }

    /// Adds a photo's vector (scaled to unit length). `Ok(false)` when the photo already has one.
    pub fn insert(&mut self, key: Key, vec: &[f32]) -> Result<bool, Error> {
        if self.pos.contains_key(&key) {
            return Ok(false);
        }
        if vec.len() != self.dim {
            return Err(Error::Invalid("vector size"));
        }
        let bits = encode(vec).ok_or(Error::Invalid("vector is not finite or has no length"))?;
        self.file.append(&Self::record(&key, &bits))?;
        self.pos.insert(key, self.keys.len());
        self.keys.push(key);
        self.rows.extend_from_slice(&bits);
        Ok(true)
    }

    /// The `k` photos whose vectors are closest to `query`, best first (ties by insertion order).
    pub fn search(&self, query: &[f32], k: usize) -> Result<Vec<Hit>, Error> {
        if query.len() != self.dim {
            return Err(Error::Invalid("vector size"));
        }
        let q = normalized(query).ok_or(Error::Invalid("query is not finite or has no length"))?;
        let mut scored: Vec<(f32, usize)> = self.rows.chunks_exact(self.dim).enumerate().map(|(i, row)| (dot(row, &q), i)).collect();
        let order = |a: &(f32, usize), b: &(f32, usize)| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1));
        if k < scored.len() {
            if k == 0 {
                return Ok(Vec::new());
            }
            scored.select_nth_unstable_by(k - 1, order);
            scored.truncate(k);
        }
        scored.sort_unstable_by(order);
        Ok(scored.into_iter().filter_map(|(score, i)| self.keys.get(i).map(|&key| Hit { key, score })).collect())
    }

    /// A vector as stored (unit length, f16 precision), for comparing photos.
    pub fn vector(&self, key: &Key) -> Option<Vec<f32>> {
        let i = *self.pos.get(key)?;
        self.rows.get(i * self.dim..(i + 1) * self.dim).map(decode)
    }

    /// A file's worth of records (header first) for the photos `skip` doesn't name, at most
    /// `max` of them: what a device sends a server that lacks them. [`EmbeddingIndex::import`]
    /// reads it back.
    pub fn export(&self, skip: impl Fn(&Key) -> bool, max: usize) -> Vec<u8> {
        let mut out = Vec::new();
        if let Ok(spec) = Self::spec(&self.spec_model, self.dim) {
            out.extend_from_slice(&spec.header());
        }
        let mut n = 0;
        for (key, row) in self.keys.iter().zip(self.rows.chunks_exact(self.dim)) {
            if n >= max {
                break;
            }
            if !skip(key) {
                out.extend_from_slice(&Self::record(key, row));
                n += 1;
            }
        }
        out
    }

    /// Adds the records of an [`EmbeddingIndex::export`] (or any upload shaped like one) for the
    /// photos `allow` accepts, skipping those already here. All or nothing: a wrong model, a
    /// truncated body or one bad vector rejects the whole thing. Vectors are rescaled to unit
    /// length, so a sender can't skew rankings with large ones.
    pub fn import(&mut self, bytes: &[u8], allow: impl Fn(&Key) -> bool) -> Result<Import, Error> {
        let spec = Self::spec(&self.spec_model, self.dim)?;
        spec.check_header(bytes)?;
        let body = bytes.get(HEADER_LEN..).ok_or(Error::Format("short header"))?;
        if !body.len().is_multiple_of(spec.rec_len) {
            return Err(Error::Format("truncated record"));
        }
        let mut out = Import::default();
        let mut seen: HashSet<Key> = HashSet::new();
        let mut fresh: Vec<(Key, Vec<u16>)> = Vec::new();
        for rec in body.chunks_exact(spec.rec_len) {
            let (Some(key), Some(raw)) = (Key::read(rec), rec.get(KEY_LEN..)) else { return Err(Error::Format("truncated record")) };
            let bits = bits_of(raw);
            if !bits.iter().all(|&b| f16_is_finite(b)) {
                return Err(Error::Invalid("vector is not finite"));
            }
            let bits = encode(&decode(&bits)).ok_or(Error::Invalid("vector has no length"))?;
            if self.pos.contains_key(&key) || !allow(&key) || !seen.insert(key) {
                out.skipped += 1;
            } else {
                fresh.push((key, bits));
            }
        }
        let mut recs = Vec::with_capacity(fresh.len() * spec.rec_len);
        for (key, bits) in &fresh {
            recs.extend_from_slice(&Self::record(key, bits));
        }
        if !recs.is_empty() {
            self.file.append(&recs)?;
        }
        for (key, bits) in fresh {
            self.pos.insert(key, self.keys.len());
            self.keys.push(key);
            self.rows.extend_from_slice(&bits);
            out.added += 1;
        }
        Ok(out)
    }
}
