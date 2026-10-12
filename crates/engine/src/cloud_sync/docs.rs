//! Settings that follow a user to their other devices: export, metadata and filter presets, curve
//! presets, colour-label and keyword sets, LUT profiles and the import defaults that don't name a
//! place on one computer. Each is a small document on the server (`GET` / `PUT /api/docs/<name>`,
//! [`proto::Doc`]) — a JSON list — kept like the user presets ([`super::merge_presets`]): a device
//! sends its list when it changes, and merges the server's against the copy both last agreed on,
//! item by item, so a preset deleted on one device stays deleted and ones added on two devices are
//! all kept.
//!
//! What stays on each device on purpose: folders, the cache size, the external editor, the search
//! sharing choices and the window layout (they describe one computer or one person's privacy
//! choice there). An older server that doesn't know the documents says `404`: they are left alone
//! for the session and the library's own sync goes on.

use std::collections::{BTreeMap, HashSet};

use lightcraft_catalog::sync::proto;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::{Body, Control, Done, SyncState, decode, merge_by, why};
use crate::{Result, Session};

/// How the items of a document are told apart.
#[derive(Clone, Copy)]
enum Key {
    /// A preset or set: by its name, whatever the case.
    Name,
    /// By its `id`.
    Id,
}

/// The documents, in the order they are synced.
const SPECS: [(&str, Key); 8] = [
    ("export-presets", Key::Name),
    ("metadata-presets", Key::Name),
    ("filter-presets", Key::Name),
    ("curve-presets", Key::Name),
    ("label-sets", Key::Name),
    ("keyword-sets", Key::Name),
    ("lut-profiles", Key::Id),
    ("prefs", Key::Id),
];

/// Items taken from the server per document at most (untrusted input).
const ITEMS_MAX: usize = 2000;
/// A LUT's `.cube` text at most, in bytes.
const CUBE_MAX: usize = 4 << 20;
/// A preset's or set's name at most, in characters.
const NAME_MAX: usize = 200;
/// The id of the one item of the `prefs` document.
const IMPORT_ITEM: &str = "import";
/// The import defaults that name a place on one computer: not shared.
const LOCAL_IMPORT: [&str; 3] = ["autoFolder", "autoCopy", "autoAlbum"];

/// What the server and this device have of one document.
#[derive(Default)]
pub(super) struct DocState {
    /// The items both sides last agreed on.
    base: Vec<Value>,
    /// The server's version, as the last pull said.
    remote: u64,
    /// This device's items differ from `base`: send them.
    changed: bool,
    /// The server doesn't keep it (an older server): not asked again this session.
    unsupported: bool,
}

fn key_of(key: Key, v: &Value) -> Option<String> {
    match key {
        Key::Name => v.get("name").and_then(Value::as_str).map(|n| n.trim().to_lowercase()).filter(|n| !n.is_empty()),
        Key::Id => v.get("id").and_then(Value::as_str).map(str::to_string),
    }
}

fn key_for(name: &str) -> Option<Key> {
    SPECS.iter().find(|(n, _)| *n == name).map(|(_, k)| *k)
}

impl SyncState {
    /// Read the bases `sync.docs` holds (by name), and set each document's version from the settings.
    pub(super) fn load_docs_base(&mut self, bytes: Option<&[u8]>) {
        let bases: BTreeMap<String, Vec<Value>> = bytes.and_then(|b| serde_json::from_slice(b).ok()).unwrap_or_default();
        for (name, _) in SPECS {
            let d = self.docs.entry(name.to_string()).or_default();
            d.base = bases.get(name).cloned().unwrap_or_default();
            d.remote = self.config.docs_versions.get(name).copied().unwrap_or(0);
        }
    }

    /// The bases, for `sync.docs`.
    pub(super) fn docs_base(&self) -> BTreeMap<&str, &Vec<Value>> {
        self.docs.iter().map(|(n, d)| (n.as_str(), &d.base)).collect()
    }

    /// The pull said these documents are at these versions on the server.
    pub(super) fn note_docs(&mut self, versions: &BTreeMap<String, u64>) {
        for (name, v) in versions {
            if key_for(name).is_some() {
                let d = self.docs.entry(name.clone()).or_default();
                d.remote = d.remote.max(*v);
            }
        }
    }
}

/// Items from `list` that parse as `T`, with a usable, unique name (at most [`ITEMS_MAX`]).
fn parse<T: DeserializeOwned>(items: &[Value], name_of: impl Fn(&T) -> String) -> Vec<T> {
    let mut seen = HashSet::new();
    items
        .iter()
        .take(ITEMS_MAX)
        .filter_map(|v| serde_json::from_value::<T>(v.clone()).ok())
        .filter(|t| {
            let n = name_of(t).trim().to_lowercase();
            !n.is_empty() && n.chars().count() <= NAME_MAX && seen.insert(n)
        })
        .collect()
}

fn list<T: serde::Serialize>(v: &[T]) -> Vec<Value> {
    v.iter().filter_map(|x| serde_json::to_value(x).ok()).collect()
}

impl Session {
    /// This device's items of document `name`, as synced.
    pub(super) fn doc_items(&self, name: &str) -> Vec<Value> {
        match name {
            "export-presets" => list(&self.export_presets),
            "metadata-presets" => list(&self.metadata_presets),
            "filter-presets" => list(&self.filter_presets),
            "curve-presets" => list(&self.curve_presets),
            "label-sets" => list(&self.label_sets),
            "keyword-sets" => list(&self.keyword_sets),
            // (a profile whose file can't be read here isn't offered: to the others it looks removed)
            "lut-profiles" => self
                .lut_profiles
                .iter()
                .filter_map(|p| {
                    let cube = std::fs::read_to_string(&p.file).ok().filter(|c| c.len() <= CUBE_MAX)?;
                    Some(json!({"id": p.id, "name": p.name, "group": p.group, "cube": cube}))
                })
                .collect(),
            "prefs" => {
                let mut v = serde_json::to_value(&self.import_defaults).unwrap_or(Value::Null);
                if let Some(o) = v.as_object_mut() {
                    LOCAL_IMPORT.iter().for_each(|k| drop(o.remove(*k)));
                    o.insert("id".into(), json!(IMPORT_ITEM));
                }
                vec![v]
            }
            _ => Vec::new(),
        }
    }

    /// Make `items` this device's document `name`: what doesn't parse, repeats a name, or is too large is
    /// left out. Saves the preferences.
    pub(super) fn set_doc_items(&mut self, name: &str, items: &[Value]) -> Result<()> {
        match name {
            "export-presets" => self.export_presets = parse(items, |p: &crate::export::ExportPreset| p.name.clone()),
            "metadata-presets" => self.metadata_presets = parse(items, |p: &crate::cmd::metadata::MetadataPreset| p.name.clone()),
            "filter-presets" => self.filter_presets = parse(items, |p: &crate::cmd::filters::FilterPreset| p.name.clone()),
            "curve-presets" => {
                self.curve_presets = parse(items, |p: &crate::cmd::curves::CurvePreset| p.name.clone());
                self.curve_presets.retain(|p| !p.builtin);
            }
            "label-sets" => self.label_sets = parse(items, |p: &crate::cmd::manage::LabelSet| p.name.clone()),
            "keyword-sets" => {
                self.keyword_sets = parse(items, |p: &crate::cmd::keywords::KeywordSet| p.name.clone());
                self.keyword_sets.iter_mut().for_each(|k| k.keywords.truncate(9));
            }
            "lut-profiles" => self.set_lut_profiles(items),
            "prefs" => self.set_import_defaults(items),
            _ => return Ok(()),
        }
        self.save_prefs()
    }

    /// The import defaults the other device shares, over this device's (its watched folder stays).
    fn set_import_defaults(&mut self, items: &[Value]) {
        let Some(theirs) = items.iter().find(|v| v.get("id").and_then(Value::as_str) == Some(IMPORT_ITEM)).and_then(Value::as_object) else { return };
        let mut mine = serde_json::to_value(&self.import_defaults).unwrap_or(Value::Null);
        let Some(m) = mine.as_object_mut() else { return };
        for (k, v) in theirs {
            if k != "id" && !LOCAL_IMPORT.contains(&k.as_str()) {
                m.insert(k.clone(), v.clone());
            }
        }
        if let Ok(d) = serde_json::from_value(mine) {
            self.import_defaults = d;
        }
    }

    /// Install the LUT profiles in `items` (each carries its `.cube` text): a new one is written to
    /// this library's `Profiles` folder and registered, one that is no longer listed goes.
    fn set_lut_profiles(&mut self, items: &[Value]) {
        use crate::cmd::lut_profiles::LutProfile;
        let dir = self.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.join("Profiles"));
        let mut out: Vec<LutProfile> = Vec::new();
        let mut seen = HashSet::new();
        for v in items.iter().take(ITEMS_MAX) {
            let (Some(id), Some(cube)) = (v.get("id").and_then(Value::as_str), v.get("cube").and_then(Value::as_str)) else { continue };
            // (the id names a file: nothing that could leave the folder)
            let Some(slug) = id
                .strip_prefix("lut:")
                .filter(|s| !s.is_empty() && s.len() <= 120 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
            else {
                continue;
            };
            if cube.len() > CUBE_MAX || !seen.insert(id.to_string()) {
                continue;
            }
            let name = v.get("name").and_then(Value::as_str).filter(|n| !n.is_empty() && n.chars().count() <= NAME_MAX).unwrap_or(slug).to_string();
            let group =
                v.get("group").and_then(Value::as_str).filter(|n| !n.is_empty() && n.chars().count() <= NAME_MAX).unwrap_or("Imported").to_string();
            // the same LUT is already here: keep its file
            if let Some(old) = self.lut_profiles.iter().find(|p| p.id == id)
                && std::fs::read_to_string(&old.file).is_ok_and(|t| t == cube)
            {
                out.push(LutProfile { id: id.to_string(), name, group, file: old.file.clone() });
                continue;
            }
            let Ok(lut) = lightcraft_pipeline::lut::Lut3d::parse_cube(cube) else { continue };
            let file = match &dir {
                Some(d) => d.join(format!("{slug}.cube")),
                // no library on disk: the in-memory session ends with the app
                None => std::env::temp_dir().join(format!("lightcraft-{slug}.cube")),
            };
            if let Some(parent) = file.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if std::fs::write(&file, cube).is_err() {
                continue;
            }
            lightcraft_pipeline::lut::register(id, lut);
            out.push(LutProfile { id: id.to_string(), name, group, file: file.to_string_lossy().to_string() });
        }
        for old in &self.lut_profiles {
            if !out.iter().any(|p| p.id == old.id) {
                lightcraft_pipeline::lut::unregister(&old.id);
                if dir.as_ref().is_some_and(|d| std::path::Path::new(&old.file).starts_with(d)) {
                    let _ = std::fs::remove_file(&old.file);
                }
            }
        }
        self.lut_profiles = out;
    }

    /// Did this device's documents change since they were last synced? (Compared only after prefs.json was
    /// written, which happens when they change.)
    fn docs_changed(&self, st: &mut SyncState) {
        let generation = self.library.as_ref().map(|l| l.prefs_gen());
        if st.docs_seen == generation {
            return;
        }
        st.docs_seen = generation;
        for (name, _) in SPECS {
            let items = self.doc_items(name);
            let d = st.docs.entry(name.to_string()).or_default();
            d.changed = items != d.base;
        }
    }

    /// The next request about the shared documents, if any: get one the server has newer, send one that
    /// changed here. (Method, path and body; the control it is.)
    pub(super) fn doc_request(&mut self, st: &mut SyncState) -> Option<(Control, &'static str, String, Body)> {
        self.docs_changed(st);
        for (name, _) in SPECS {
            let version = st.config.docs_versions.get(name).copied().unwrap_or(0);
            let d = st.docs.entry(name.to_string()).or_default();
            if d.unsupported {
                continue;
            }
            let path = format!("/api/docs/{name}");
            if d.remote > version {
                return Some((Control::GetDoc(name.to_string()), "GET", path, Body::Empty));
            }
            if d.changed {
                let items = self.doc_items(name);
                let body = serde_json::to_string(&proto::Doc { version, items: Value::Array(items.clone()) }).ok()?;
                return Some((Control::PutDoc(name.to_string(), items), "PUT", path, Body::Json(body)));
            }
        }
        None
    }

    /// The server's document `theirs` merged into this device's.
    fn take_doc(&mut self, st: &mut SyncState, name: &str, theirs: proto::Doc) {
        let Some(key) = key_for(name) else { return };
        let remote: Vec<Value> = theirs.items.as_array().map(|a| a.iter().take(ITEMS_MAX).cloned().collect()).unwrap_or_default();
        let ours = self.doc_items(name);
        let base = st.docs.get(name).map(|d| d.base.clone()).unwrap_or_default();
        let merged = merge_by(&base, &ours, &remote, &|v| key_of(key, v));
        if merged != ours
            && let Err(e) = self.set_doc_items(name, &merged)
        {
            log::warn!("sync: {name}: {e}");
        }
        let d = st.docs.entry(name.to_string()).or_default();
        d.base = remote;
        d.remote = d.remote.max(theirs.version);
        st.config.docs_versions.insert(name.to_string(), theirs.version);
        st.docs_base_dirty = true;
        st.docs_seen = None;
    }

    /// The answer to `GET /api/docs/<name>`.
    pub(super) fn doc_got(&mut self, st: &mut SyncState, name: &str, d: &Done) {
        match d.status {
            404 | 405 => self.doc_unsupported(st, name),
            _ if d.ok() => match decode::<proto::Doc>(d) {
                Ok(doc) => {
                    self.take_doc(st, name, doc);
                    st.succeeded();
                }
                Err(e) => st.fail(e),
            },
            _ => st.fail(why(d)),
        }
    }

    /// The answer to `PUT /api/docs/<name>` that sent `sent`.
    pub(super) fn doc_put(&mut self, st: &mut SyncState, name: &str, sent: Vec<Value>, d: &Done) {
        match d.status {
            404 | 405 => self.doc_unsupported(st, name),
            // another device wrote first: the answer is theirs, merge and send again
            412 => match decode::<proto::Doc>(d) {
                Ok(doc) => self.take_doc(st, name, doc),
                Err(e) => st.fail(e),
            },
            _ if d.ok() => match serde_json::from_str::<Value>(&d.body).ok().and_then(|v| v["version"].as_u64()) {
                Some(v) => {
                    let doc = st.docs.entry(name.to_string()).or_default();
                    doc.base = sent;
                    doc.remote = doc.remote.max(v);
                    st.config.docs_versions.insert(name.to_string(), v);
                    st.docs_base_dirty = true;
                    st.docs_seen = None;
                    st.succeeded();
                }
                None => st.fail("unexpected answer from the server".into()),
            },
            _ => st.fail(why(d)),
        }
    }

    /// A server that doesn't keep this document (older than this app): leave it, and the rest of the sync, alone.
    fn doc_unsupported(&mut self, st: &mut SyncState, name: &str) {
        log::info!("sync: this server doesn't keep `{name}`: update it to share it between devices");
        st.docs.entry(name.to_string()).or_default().unsupported = true;
        st.succeeded();
    }
}
