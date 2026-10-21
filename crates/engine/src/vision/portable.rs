//! What was found in a photo, as it travels between one person's devices through iCloud ([`crate::icloud::analysis`]):
//! its faces, the words in it, its description vector and its sensitive-content score, by the photo's content key.
//! A device keeps what another found in the same files its own indexes are (`search/`), so People, the search for
//! words in photos and the list of possibly sensitive photos use them as if it had found them itself: a device that
//! can't run the models (not downloaded, or too little memory) has them from one that can, and a second computer
//! needn't work them out again.
//!
//! What is taken only fills in: a photo this device has a part of keeps its own (the indexes never replace a photo's
//! entry), so two devices that both worked a photo out never undo each other. A description vector is kept only by a
//! device with the same search model (another can't search by it), a part from another engine only by a device that
//! has none of its own.

use std::collections::{HashMap, HashSet};

use base64::Engine as _;
use lightcraft_vision::faces::FaceFound;
use lightcraft_vision::store::Key;
use serde_json::{Value, json};

use super::sensitive::{ClassSpec, with_sensitive};
use super::{IndexSpec, TextSpec, with_faces, with_index, with_text};
use crate::Session;

/// The parts of a photo's analysis, as bits.
pub(crate) const FACES: u8 = 1;
pub(crate) const TEXT: u8 = 2;
pub(crate) const VECTOR: u8 = 4;
pub(crate) const SENSITIVE: u8 = 8;

/// The words of a photo kept at most, in bytes (the text index's own limit).
const TEXT_MAX: usize = 64 << 10;

fn b64(v: &[f32]) -> String {
    let bytes: Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// `dim` finite numbers from `text` (base64 of little-endian f32s), or nothing.
fn floats(text: &str, dim: usize) -> Option<Vec<f32>> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(text).ok()?;
    if bytes.len() != dim.checked_mul(4)? {
        return None;
    }
    let v: Vec<f32> = bytes.chunks_exact(4).map(|c| <[u8; 4]>::try_from(c).map(f32::from_le_bytes)).collect::<Result<_, _>>().ok()?;
    v.iter().all(|x| x.is_finite()).then_some(v)
}

/// Which parts an analysis record's data has.
pub(crate) fn parts_of(v: &Value) -> u8 {
    let mut parts = 0;
    for (key, bit) in [("faces", FACES), ("text", TEXT), ("vector", VECTOR), ("sensitive", SENSITIVE)] {
        if v.get(key).is_some_and(Value::is_object) {
            parts |= bit;
        }
    }
    parts
}

/// The engine ids of the `prefix-<engine>.bin` indexes in the library folder `dir`'s `search/`.
fn engines_on_disk(dir: Option<&std::path::Path>, prefix: &str) -> Vec<String> {
    let Some(dir) = dir else { return Vec::new() };
    let mut out: Vec<String> = std::fs::read_dir(dir.join("search"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let engine = name.strip_prefix(prefix)?.strip_prefix('-')?.strip_suffix(".bin")?.to_string();
            (!engine.is_empty() && engine.len() <= 64).then_some(engine)
        })
        .collect();
    out.sort();
    out
}

/// Whether an index at `path` can be read without making one: it is a file already.
fn on_disk(path: Option<&std::path::Path>) -> bool {
    path.is_some_and(std::path::Path::is_file)
}

impl Session {
    fn library_dir(&self) -> Option<&std::path::Path> {
        self.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.as_path())
    }

    /// The text index this device uses: its reader's, else one another device filled (this build reads none).
    pub(super) fn portable_text_spec(&self) -> Option<TextSpec> {
        let engine = match self.vision.reader_provider() {
            Ok((_, engine)) => engine,
            Err(_) if !self.vision.text_available() => engines_on_disk(self.library_dir(), "text").into_iter().next()?,
            Err(_) => return None,
        };
        Some(TextSpec { path: self.vision_text_path(&engine), engine })
    }

    /// The sensitive-content scores this device uses: its classifier's, else ones another device filled.
    pub(super) fn portable_sensitive_spec(&self) -> Option<ClassSpec> {
        let engine = match self.vision.classifier_provider() {
            Ok((_, engine)) => engine,
            Err(_) if !self.vision.sensitive_available() => engines_on_disk(self.library_dir(), "sensitive").into_iter().next()?,
            Err(_) => return None,
        };
        Some(ClassSpec { path: self.vision_sensitive_path(&engine), engine })
    }

    /// The description vectors this device searches by: only with the search model here.
    fn portable_vector_spec(&self) -> Option<IndexSpec> {
        let (_, model, dim) = self.vision.provider().ok()?;
        Some(IndexSpec { path: self.vision_index_path(&model), model, dim })
    }

    /// Which parts this device has of each photo of `keys` (content keys): none is left out.
    pub(crate) fn analysis_parts(&self, keys: &[String]) -> HashMap<String, u8> {
        let shared = &self.vision.shared;
        let ks: Vec<(String, Key)> = keys.iter().map(|k| (k.clone(), Key::of(k))).collect();
        let mut out: HashMap<String, u8> = ks.iter().map(|(k, _)| (k.clone(), 0)).collect();
        let mut mark = |bit: u8, has: &dyn Fn(&Key) -> bool| {
            for (k, key) in &ks {
                if has(key)
                    && let Some(p) = out.get_mut(k)
                {
                    *p |= bit;
                }
            }
        };
        let faces = self.faces_spec();
        if on_disk(faces.path.as_deref()) {
            let _ = with_faces(shared, &faces, |ix| mark(FACES, &|k| ix.has(k)));
        }
        if let Some(spec) = self.portable_text_spec().filter(|s| on_disk(s.path.as_deref())) {
            let _ = with_text(shared, &spec, |ix| mark(TEXT, &|k| ix.contains(k)));
        }
        if let Some(spec) = self.portable_vector_spec().filter(|s| on_disk(s.path.as_deref())) {
            let _ = with_index(shared, &spec, |ix| mark(VECTOR, &|k| ix.contains(k)));
        }
        if let Some(spec) = self.portable_sensitive_spec().filter(|s| on_disk(s.path.as_deref())) {
            let _ = with_sensitive(shared, &spec, |ix| mark(SENSITIVE, &|k| ix.contains(k)));
        }
        out
    }

    /// What this device found in each photo of `keys` it has something of, as it travels: `{faces: {engine, found:
    /// [{rect, score, e}]}, text: {engine, text}, vector: {model, e}, sensitive: {engine, score}}` (`e`: base64 of
    /// little-endian f32s), the parts it has.
    pub(crate) fn analysis_data(&self, keys: &[String]) -> HashMap<String, Value> {
        let shared = &self.vision.shared;
        let ks: Vec<(String, Key)> = keys.iter().map(|k| (k.clone(), Key::of(k))).collect();
        let mut out: HashMap<String, serde_json::Map<String, Value>> = HashMap::new();
        let faces = self.faces_spec();
        if on_disk(faces.path.as_deref()) {
            let _ = with_faces(shared, &faces, |ix| {
                // (only the faces of these photos are written out)
                let wanted: HashSet<Key> = ks.iter().map(|(_, key)| *key).collect();
                let mut found: HashMap<Key, Vec<Value>> = HashMap::new();
                for f in ix.faces().iter().filter(|f| wanted.contains(&f.key)) {
                    found.entry(f.key).or_default().push(json!({"rect": f.rect, "score": f.score, "e": b64(&f.embedding)}));
                }
                for (k, key) in &ks {
                    if ix.has(key) {
                        let list = found.remove(key).unwrap_or_default();
                        out.entry(k.clone()).or_default().insert("faces".into(), json!({"engine": faces.engine, "found": list}));
                    }
                }
            });
        }
        if let Some(spec) = self.portable_text_spec().filter(|s| on_disk(s.path.as_deref())) {
            let _ = with_text(shared, &spec, |ix| {
                for (k, key) in &ks {
                    if let Some(t) = ix.text(key) {
                        out.entry(k.clone()).or_default().insert("text".into(), json!({"engine": spec.engine, "text": t}));
                    }
                }
            });
        }
        if let Some(spec) = self.portable_vector_spec().filter(|s| on_disk(s.path.as_deref())) {
            let _ = with_index(shared, &spec, |ix| {
                for (k, key) in &ks {
                    if let Some(v) = ix.vector(key) {
                        out.entry(k.clone()).or_default().insert("vector".into(), json!({"model": spec.model, "e": b64(&v)}));
                    }
                }
            });
        }
        if let Some(spec) = self.portable_sensitive_spec().filter(|s| on_disk(s.path.as_deref())) {
            let _ = with_sensitive(shared, &spec, |ix| {
                for (k, key) in &ks {
                    if let Some(score) = ix.score(key) {
                        out.entry(k.clone()).or_default().insert("sensitive".into(), json!({"engine": spec.engine, "score": score}));
                    }
                }
            });
        }
        out.into_iter().map(|(k, m)| (k, Value::Object(m))).collect()
    }

    /// Keep what another device found in photo `key` (`data` as [`Session::analysis_data`] makes it): each part this
    /// device has nothing of, into the index it uses for it. Answers the parts added; what doesn't read right is left out.
    pub(crate) fn analysis_take(&mut self, key: &str, data: &Value) -> u8 {
        let shared = self.vision.shared.clone();
        let k = Key::of(key);
        let mut added = 0;
        let engine_ok =
            |e: &str| !e.is_empty() && e.len() <= 64 && e.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.');
        // faces: into the face index this device uses (another engine's faces don't group with its own)
        if let Some(f) = data.get("faces").filter(|f| f["engine"].as_str() == Some(self.faces_spec().engine.as_str()))
            && let Some(list) = f["found"].as_array()
        {
            let found: Option<Vec<FaceFound>> = list
                .iter()
                .take(lightcraft_vision::faces::MAX_FACES)
                .map(|v| {
                    let r = v["rect"].as_array()?;
                    let mut rect = [0f32; 4];
                    for (slot, x) in rect.iter_mut().zip(r.iter()) {
                        *slot = x.as_f64()? as f32;
                    }
                    let score = v["score"].as_f64()? as f32;
                    let embedding = floats(v["e"].as_str()?, lightcraft_vision::faces::DIM)?;
                    (r.len() == 4).then_some(FaceFound { rect, score, embedding })
                })
                .collect();
            if let Some(found) = found
                && with_faces(&shared, &self.faces_spec(), |ix| ix.insert_photo(k, &found)).is_ok_and(|r| r.unwrap_or(false))
            {
                added |= FACES;
            }
        }
        // words: into the reader's index, or the one another device filled when this build reads none
        if let Some(t) = data.get("text")
            && let (Some(engine), Some(text)) = (t["engine"].as_str().filter(|e| engine_ok(e)), t["text"].as_str().filter(|t| t.len() <= TEXT_MAX))
        {
            let mine = self.portable_text_spec();
            let spec = match mine {
                Some(s) if s.engine == engine => Some(s),
                // (a device that reads text with another engine searches its own index only)
                Some(_) => None,
                None if !self.vision.text_available() => Some(TextSpec { path: self.vision_text_path(engine), engine: engine.to_string() }),
                None => None,
            };
            if let Some(spec) = spec
                && with_text(&shared, &spec, |ix| ix.insert(k, text)).is_ok_and(|r| r.unwrap_or(false))
            {
                added |= TEXT;
            }
        }
        // the description vector: only with the same search model here (another can't search by it)
        if let Some(v) = data.get("vector")
            && let Some(spec) = self.portable_vector_spec().filter(|s| v["model"].as_str() == Some(s.model.as_str()))
            && let Some(e) = v["e"].as_str().and_then(|e| floats(e, spec.dim))
            && with_index(&shared, &spec, |ix| if ix.contains(&k) { Ok(false) } else { ix.insert(k, &e) }).is_ok_and(|r| r.unwrap_or(false))
        {
            added |= VECTOR;
        }
        // the sensitive-content score: into the classifier's scores, or the ones another device filled
        if let Some(v) = data.get("sensitive")
            && let (Some(engine), Some(score)) = (
                v["engine"].as_str().filter(|e| engine_ok(e)),
                v["score"].as_f64().map(|x| x as f32).filter(|x| x.is_finite() && (0.0..=1.0).contains(x)),
            )
        {
            let spec = match self.portable_sensitive_spec() {
                Some(s) if s.engine == engine => Some(s),
                Some(_) => None,
                None if !self.vision.sensitive_available() => {
                    Some(ClassSpec { path: self.vision_sensitive_path(engine), engine: engine.to_string() })
                }
                None => None,
            };
            if let Some(spec) = spec
                && with_sensitive(&shared, &spec, |ix| if ix.contains(&k) { Ok(false) } else { ix.insert(k, score).map(|()| true) })
                    .is_ok_and(|r| r.unwrap_or(false))
            {
                added |= SENSITIVE;
            }
        }
        // (People is worked out again by itself: the face index's revision moved)
        added
    }

    /// How many photos each index has (faces, words, vectors, scores): a change means there may be more to send.
    pub(crate) fn analysis_sizes(&self) -> [usize; 4] {
        let sh = &self.vision.shared;
        use std::sync::atomic::Ordering::Relaxed;
        [sh.face_photos.load(Relaxed), sh.text_len.load(Relaxed), sh.indexed.load(Relaxed), sh.sensitive_len.load(Relaxed)]
    }
}
