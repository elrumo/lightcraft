//! The faces found in a library's photos, one record per face in an append-only file (the same
//! kind of store as the search vectors). A photo with no faces gets a marker record, so it is
//! known to have been looked at; a photo's records are written together and carry the number of
//! faces it has, so a crash can never leave half a photo that looks complete. Opening it reads
//! which photos were looked at and how many faces there are: the faces themselves are read the
//! first time they are wanted ([`FaceIndex::faces`]), and kept as f16, as stored.

use std::cell::OnceCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;

use super::{DIM, ENGINE, FaceFound, MAX_FACES};
use crate::index::f16_to_f32;
use crate::store::{Error, HEADER_LEN, KEY_LEN, Key, RecordFile, Spec};

/// Bytes of a record: key, index, flags, count, reserved, rect, score, vector.
pub const REC_LEN: usize = KEY_LEN + 4 + 16 + 4 + DIM * 2;
const FLAG_MARKER: u8 = 1;

/// A face in the index.
#[derive(Clone, Debug, PartialEq)]
pub struct Face {
    pub key: Key,
    /// Which face of its photo this is.
    pub index: u8,
    /// `[x0, y0, x1, y1]`, normalized to the upright photo.
    pub rect: [f32; 4],
    pub score: f32,
    /// [`DIM`] numbers of unit length, as f16 bit patterns (as stored: half the memory of f32).
    pub embedding_f16: [u16; DIM],
}

impl Face {
    /// The face's [`DIM`] numbers of unit length.
    pub fn embedding(&self) -> Vec<f32> {
        self.embedding_f16.iter().map(|&b| f16_to_f32(b)).collect()
    }
}

/// What [`FaceIndex::import`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Import {
    /// Photos added.
    pub added: usize,
    /// Photos already present, or not allowed.
    pub skipped: usize,
}

#[derive(Debug)]
pub struct FaceIndex {
    /// `None`: in memory only.
    file: Option<RecordFile>,
    /// Read from the file the first time they are wanted.
    faces: OnceCell<Vec<Face>>,
    /// Faces in the index (known before they are read).
    count: usize,
    /// Photos that were looked at (their faces are in `faces`, or there are none).
    scanned: HashSet<Key>,
    /// Bumped by every change (callers cache what they derive from the faces).
    revision: u64,
}

fn spec() -> Spec<'static> {
    Spec { model: ENGINE, dim: DIM, rec_len: REC_LEN }
}

fn put(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(bytes);
}

/// The record of face `index` (of `count`) of the photo `key`; `None` makes the "no faces" marker.
fn record(key: &Key, index: u8, count: u8, face: Option<&Face>) -> Vec<u8> {
    let mut out = Vec::with_capacity(REC_LEN);
    put(&mut out, &key.0);
    put(&mut out, &[index, if face.is_none() { FLAG_MARKER } else { 0 }, count, 0]);
    let (rect, score) = face.map_or(([0.0; 4], 0.0), |f| (f.rect, f.score));
    for v in rect {
        put(&mut out, &v.to_le_bytes());
    }
    put(&mut out, &score.to_le_bytes());
    for i in 0..DIM {
        let bits = face.and_then(|f| f.embedding_f16.get(i)).copied().unwrap_or(0);
        put(&mut out, &bits.to_le_bytes());
    }
    out
}

fn f32_at(rec: &[u8], at: usize) -> Option<f32> {
    rec.get(at..at + 4).and_then(|b| <[u8; 4]>::try_from(b).ok()).map(f32::from_le_bytes)
}

/// What a record holds: its key, face number, how many faces its photo has, and the face (`None`:
/// the "no faces" marker). `None` for a record that must not be believed.
fn parse(rec: &[u8]) -> Option<(Key, u8, u8, Option<Face>)> {
    let key = Key::read(rec)?;
    let (index, flags, count) = (*rec.get(KEY_LEN)?, *rec.get(KEY_LEN + 1)?, *rec.get(KEY_LEN + 2)?);
    if flags & !FLAG_MARKER != 0 || usize::from(count) > MAX_FACES {
        return None;
    }
    if flags & FLAG_MARKER != 0 {
        return (count == 0).then_some((key, 0, 0, None));
    }
    if count == 0 || index >= count {
        return None;
    }
    let mut rect = [0f32; 4];
    for (i, r) in rect.iter_mut().enumerate() {
        *r = f32_at(rec, KEY_LEN + 4 + i * 4)?;
    }
    let score = f32_at(rec, KEY_LEN + 20)?;
    let body = rec.get(KEY_LEN + 24..)?.as_chunks::<2>().0;
    if body.len() != DIM {
        return None;
    }
    let mut embedding_f16 = [0u16; DIM];
    for (e, c) in embedding_f16.iter_mut().zip(body) {
        *e = u16::from_le_bytes(*c);
        if *e & 0x7c00 == 0x7c00 {
            return None;
        }
    }
    let sane = rect.iter().all(|v| v.is_finite() && (-0.01..=1.01).contains(v)) && rect[2] > rect[0] && rect[3] > rect[1] && score.is_finite();
    let length = embedding_f16.iter().map(|&b| f16_to_f32(b) * f16_to_f32(b)).sum::<f32>().sqrt();
    (sane && length > 0.5).then_some((key, index, count, Some(Face { key, index, rect, score, embedding_f16 })))
}

fn norm(v: &[f32]) -> f32 {
    v.iter().map(|x| x * x).sum::<f32>().sqrt()
}

/// `v` scaled to unit length, or `None` when it has a non-finite entry or no length.
fn unit(v: &[f32]) -> Option<Vec<f32>> {
    if v.len() != DIM || v.iter().any(|x| !x.is_finite()) {
        return None;
    }
    let n = norm(v);
    (n.is_finite() && n > 1e-6).then(|| v.iter().map(|x| x / n).collect())
}

/// Collects the records of one photo at a time, and lets through only photos whose records are all
/// there.
#[derive(Default)]
struct Groups {
    key: Option<Key>,
    count: u8,
    faces: Vec<Face>,
    marker: bool,
}

impl Groups {
    /// Adds a record (face `index` of `count`); returns the photo it completed or ended (as `(key,
    /// faces)`) when it was not of the photo being collected. A photo's records start with its
    /// first face (or its marker): a photo written again after records of it that were never whole
    /// starts over.
    fn push(&mut self, key: Key, index: u8, count: u8, face: Option<Face>, done: &mut Vec<(Key, Vec<Face>)>) {
        if self.key != Some(key) || index == 0 {
            self.finish(done);
            self.key = Some(key);
            self.count = count;
        }
        match face {
            Some(f) if self.count == count => self.faces.push(f),
            Some(_) => self.key = None,
            None => self.marker = true,
        }
    }

    fn finish(&mut self, done: &mut Vec<(Key, Vec<Face>)>) {
        if let Some(key) = self.key.take() {
            let whole = if self.marker { self.faces.is_empty() } else { self.faces.len() == usize::from(self.count) };
            let in_order = self.faces.iter().enumerate().all(|(i, f)| usize::from(f.index) == i);
            if whole && in_order {
                done.push((key, std::mem::take(&mut self.faces)));
            }
        }
        *self = Groups::default();
    }
}

/// A file's records read one at a time into whole photos, each the first time it is whole: the
/// photos, how many faces they have, and the faces when `keep`.
#[derive(Default)]
struct Reading {
    groups: Groups,
    done: Vec<(Key, Vec<Face>)>,
    scanned: HashSet<Key>,
    count: usize,
    keep: bool,
    faces: Vec<Face>,
}

impl Reading {
    fn record(&mut self, rec: &[u8]) {
        if let Some((key, index, count, face)) = parse(rec) {
            self.groups.push(key, index, count, face, &mut self.done);
        }
        self.take_done();
    }

    fn end(&mut self) {
        self.groups.finish(&mut self.done);
        self.take_done();
    }

    fn take_done(&mut self) {
        for (key, faces) in self.done.drain(..) {
            if self.scanned.insert(key) {
                self.count += faces.len();
                if self.keep {
                    self.faces.extend(faces);
                }
            }
        }
    }
}

impl FaceIndex {
    /// Opens (or creates) the index at `path`. A file written for other models is
    /// [`Error::Mismatch`]: the caller deletes it and starts again. Records that can't be
    /// believed, and photos whose faces are not all there, are ignored.
    pub fn open(path: &Path) -> Result<FaceIndex, Error> {
        let mut r = Reading::default();
        let file = RecordFile::open(path, &spec(), |rec| r.record(rec))?;
        r.end();
        Ok(FaceIndex { file: Some(file), faces: OnceCell::new(), count: r.count, scanned: r.scanned, revision: 0 })
    }

    /// An index that lives in memory only.
    pub fn in_memory() -> FaceIndex {
        FaceIndex { file: None, faces: OnceCell::from(Vec::new()), count: 0, scanned: HashSet::new(), revision: 0 }
    }

    /// Faces in the index.
    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Photos that were looked at (with or without faces).
    pub fn photos(&self) -> usize {
        self.scanned.len()
    }

    /// Changes so far: equal revisions mean equal faces.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn scanned(&self, key: &Key) -> bool {
        self.scanned.contains(key)
    }

    pub fn keys(&self) -> impl Iterator<Item = &Key> {
        self.scanned.iter()
    }

    /// The faces, read from the file the first time they are wanted; `None` when it can't be read
    /// now (a library on a drive that went away), which is not "no faces": the next call tries again.
    pub fn read_faces(&self) -> Option<&[Face]> {
        if let Some(faces) = self.faces.get() {
            return Some(faces);
        }
        let mut r = Reading { keep: true, ..Reading::default() };
        self.file.as_ref()?.each(|rec| r.record(rec)).ok()?;
        r.end();
        Some(self.faces.get_or_init(|| r.faces))
    }

    /// [`FaceIndex::read_faces`], none when they can't be read.
    pub fn faces(&self) -> &[Face] {
        self.read_faces().unwrap_or_default()
    }

    /// Whether photo `key` was looked at (faces found in it or not).
    pub fn has(&self, key: &Key) -> bool {
        self.scanned.contains(key)
    }

    /// Keeps the faces found in a photo (none: the photo is marked as looked at). `Ok(false)` when
    /// the photo already is. Faces that aren't usable (not finite, no length) are left out.
    pub fn insert_photo(&mut self, key: Key, found: &[FaceFound]) -> Result<bool, Error> {
        if self.scanned.contains(&key) {
            return Ok(false);
        }
        let faces: Vec<Face> = found
            .iter()
            .take(MAX_FACES)
            .filter_map(|f| {
                let e = unit(&f.embedding)?;
                let ok = f.rect.iter().all(|v| v.is_finite()) && f.score.is_finite();
                let r = f.rect.map(|v| v.clamp(0.0, 1.0));
                let mut embedding_f16 = [0u16; DIM];
                for (b, x) in embedding_f16.iter_mut().zip(&e) {
                    *b = half::f16::from_f32(*x).to_bits();
                }
                (ok && r[2] > r[0] && r[3] > r[1]).then_some((r, f.score, embedding_f16))
            })
            .enumerate()
            .filter_map(|(i, (rect, score, embedding_f16))| Some(Face { key, index: u8::try_from(i).ok()?, rect, score, embedding_f16 }))
            .collect();
        let count = u8::try_from(faces.len()).unwrap_or(0);
        let mut bytes = Vec::with_capacity(REC_LEN * faces.len().max(1));
        if faces.is_empty() {
            put(&mut bytes, &record(&key, 0, 0, None));
        }
        for f in &faces {
            put(&mut bytes, &record(&key, f.index, count, Some(f)));
        }
        if let Some(file) = &mut self.file {
            file.append(&bytes)?;
        }
        self.scanned.insert(key);
        self.count += faces.len();
        if let Some(all) = self.faces.get_mut() {
            all.extend(faces);
        }
        self.revision += 1;
        Ok(true)
    }

    /// A file's worth of records (header first) for the photos `skip` doesn't name, at most `max`
    /// photos: what a device sends a server that lacks them. [`FaceIndex::import`] reads it back.
    /// (No records when the faces can't be read: never a photo said to have none.)
    pub fn export(&self, skip: impl Fn(&Key) -> bool, max: usize) -> Vec<u8> {
        let mut out = spec().header().to_vec();
        let Some(faces) = self.read_faces() else { return out };
        let mut sent = 0;
        let mut by_photo: HashMap<Key, Vec<&Face>> = HashMap::new();
        for f in faces {
            by_photo.entry(f.key).or_default().push(f);
        }
        let mut keys: Vec<&Key> = self.scanned.iter().filter(|k| !skip(k)).collect();
        keys.sort();
        for key in keys {
            if sent >= max {
                break;
            }
            sent += 1;
            match by_photo.get(key) {
                Some(faces) => {
                    let count = u8::try_from(faces.len()).unwrap_or(0);
                    for f in faces {
                        put(&mut out, &record(key, f.index, count, Some(f)));
                    }
                }
                None => put(&mut out, &record(key, 0, 0, None)),
            }
        }
        out
    }

    /// The photos of the keys in an [`FaceIndex::export`], without the header or the records.
    pub fn exported_keys(bytes: &[u8]) -> Vec<Key> {
        let mut seen = HashSet::new();
        bytes.get(HEADER_LEN..).unwrap_or_default().as_chunks::<REC_LEN>().0.iter().filter_map(|r| Key::read(r)).filter(|k| seen.insert(*k)).collect()
    }

    /// Adds the photos of an [`FaceIndex::export`] that `allow` accepts, skipping those already
    /// here. All or nothing: other models, a cut-off, damaged or incomplete photo rejects the lot.
    pub fn import(&mut self, bytes: &[u8], allow: impl Fn(&Key) -> bool) -> Result<Import, Error> {
        spec().check_header(bytes)?;
        let body = bytes.get(HEADER_LEN..).unwrap_or_default();
        if !body.len().is_multiple_of(REC_LEN) {
            return Err(Error::Format("cut-off record"));
        }
        let mut groups = Groups::default();
        let mut done: Vec<(Key, Vec<Face>)> = Vec::new();
        let mut records = 0;
        for rec in body.as_chunks::<REC_LEN>().0 {
            let (key, index, count, face) = parse(rec).ok_or(Error::Invalid("a face record"))?;
            groups.push(key, index, count, face, &mut done);
            records += 1;
        }
        groups.finish(&mut done);
        // every record belongs to a whole photo, and a photo appears once
        let mut seen = HashSet::new();
        if done.iter().map(|(_, f)| f.len().max(1)).sum::<usize>() != records || !done.iter().all(|(k, _)| seen.insert(*k)) {
            return Err(Error::Invalid("faces of a photo that are not whole"));
        }
        let mut out = Import::default();
        let mut bytes = Vec::new();
        let mut fresh: Vec<(Key, Vec<Face>)> = Vec::new();
        for (key, faces) in done {
            if self.scanned.contains(&key) || !allow(&key) {
                out.skipped += 1;
                continue;
            }
            let count = u8::try_from(faces.len()).unwrap_or(0);
            if faces.is_empty() {
                put(&mut bytes, &record(&key, 0, 0, None));
            }
            for f in &faces {
                put(&mut bytes, &record(&key, f.index, count, Some(f)));
            }
            fresh.push((key, faces));
        }
        if !bytes.is_empty()
            && let Some(file) = &mut self.file
        {
            file.append(&bytes)?;
        }
        for (key, faces) in fresh {
            self.scanned.insert(key);
            self.count += faces.len();
            if let Some(all) = self.faces.get_mut() {
                all.extend(faces);
            }
            out.added += 1;
        }
        if out.added > 0 {
            self.revision += 1;
        }
        Ok(out)
    }

    /// Forgets every face (and empties the file).
    pub fn clear(&mut self) -> Result<(), Error> {
        if let Some(file) = &mut self.file {
            file.truncate()?;
        }
        self.faces = OnceCell::from(Vec::new());
        self.count = 0;
        self.scanned.clear();
        self.revision += 1;
        Ok(())
    }
}
