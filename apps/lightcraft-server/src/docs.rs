//! Small documents a user's devices share beside the library: their export, metadata and filter
//! presets, curve presets, label and keyword sets, LUT profiles and a few preferences
//! (`GET` / `PUT /api/docs/<name>`). The server doesn't read them: a document is a versioned JSON
//! array the devices merge themselves (like the presets document, `/api/presets`), so a change
//! made on two devices at once is merged by the second one to write, not lost. A `PUT` names the
//! version it changed; a stale one gets `412` with the current document.
//!
//! `<user>/docs/<name>.json`, replaced atomically. Only the names in [`NAMES`] exist.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use lightcraft_catalog::sync::proto::Doc;

/// The documents a device may keep here. (Preferences that name a place on one computer — folders,
/// caches, the external editor — are not among them: they stay per device.)
pub const NAMES: [&str; 8] =
    ["export-presets", "metadata-presets", "filter-presets", "curve-presets", "label-sets", "keyword-sets", "lut-profiles", "prefs"];

/// The largest document (the LUT profiles carry their `.cube` files).
pub const MAX: u64 = 24 << 20;

/// One user's documents.
pub struct Docs {
    dir: PathBuf,
    docs: BTreeMap<String, Doc>,
}

fn empty() -> Doc {
    Doc { version: 0, items: serde_json::Value::Array(Vec::new()) }
}

impl Docs {
    /// Read the documents in `<user dir>/docs/`. A missing one is empty; a damaged one is an error (the
    /// user's library then doesn't open, like a damaged `presets.json`).
    pub fn load(user_dir: &Path) -> Result<Docs, String> {
        let dir = user_dir.join("docs");
        let mut docs = BTreeMap::new();
        for name in NAMES {
            let path = dir.join(format!("{name}.json"));
            match std::fs::read(&path) {
                Ok(bytes) => {
                    let doc: Doc = serde_json::from_slice(&bytes).map_err(|e| format!("{} is damaged: {e}", path.display()))?;
                    if doc.items.is_array() {
                        docs.insert(name.to_string(), doc);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("{}: {e}", path.display())),
            }
        }
        Ok(Docs { dir, docs })
    }

    /// Every document's version (the ones that were never written aren't listed): what a pull tells devices.
    pub fn versions(&self) -> BTreeMap<String, u64> {
        self.docs.iter().map(|(n, d)| (n.clone(), d.version)).collect()
    }

    /// Document `name`, empty (version 0) if it was never written; `None`: there is no such document.
    pub fn get(&self, name: &str) -> Option<Doc> {
        NAMES.contains(&name).then(|| self.docs.get(name).cloned().unwrap_or_else(empty))
    }

    /// Replace document `name` with `doc`, which changed version `doc.version`. `Ok(Ok(version))`: done,
    /// this is the new version; `Ok(Err(current))`: another device wrote first (get, merge, put again);
    /// `Err`: refused (no such document, not an array) or not written (`storage: …`).
    pub fn put(&mut self, name: &str, doc: Doc) -> Result<Result<u64, Doc>, String> {
        let Some(current) = self.get(name) else { return Err(format!("no document `{name}`")) };
        if !doc.items.is_array() {
            return Err("`items` must be an array".into());
        }
        if doc.version != current.version {
            return Ok(Err(current));
        }
        let next = Doc { version: current.version + 1, items: doc.items };
        let body = serde_json::to_vec(&next).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("storage: {}: {e}", self.dir.display()))?;
        let path = self.dir.join(format!("{name}.json"));
        lightcraft_catalog::safe_file::write_atomic(&path, &body).map_err(|e| format!("storage: {}: {e}", path.display()))?;
        let version = next.version;
        self.docs.insert(name.to_string(), next);
        Ok(Ok(version))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lc-docs-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn doc(version: u64, items: serde_json::Value) -> Doc {
        Doc { version, items }
    }

    #[test]
    fn documents_are_versioned_and_stale_writes_are_refused() {
        let dir = temp("versions");
        let mut d = Docs::load(&dir).unwrap();
        assert_eq!(d.get("export-presets"), Some(doc(0, json!([]))));
        assert!(d.versions().is_empty());
        assert_eq!(d.put("export-presets", doc(0, json!([{"name": "Web"}]))), Ok(Ok(1)));
        // a device that hasn't seen it gets the current document back
        assert_eq!(d.put("export-presets", doc(0, json!([{"name": "Print"}]))), Ok(Err(doc(1, json!([{"name": "Web"}])))));
        assert_eq!(d.put("export-presets", doc(1, json!([]))), Ok(Ok(2)));
        assert_eq!(d.versions().get("export-presets"), Some(&2));
        // it survives a restart
        let again = Docs::load(&dir).unwrap();
        assert_eq!(again.get("export-presets"), Some(doc(2, json!([]))));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_known_documents_holding_arrays_are_taken() {
        let dir = temp("names");
        let mut d = Docs::load(&dir).unwrap();
        assert!(d.get("../users").is_none() && d.get("presets").is_none());
        assert!(d.put("../users", doc(0, json!([]))).is_err());
        assert!(d.put("lut-profiles", doc(0, json!({"not": "an array"}))).is_err());
        assert!(!dir.join("docs").exists(), "nothing was written");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_damaged_document_is_an_error_not_a_reset() {
        let dir = temp("damaged");
        std::fs::create_dir_all(dir.join("docs")).unwrap();
        std::fs::write(dir.join("docs/prefs.json"), b"{not json").unwrap();
        assert!(Docs::load(&dir).err().is_some_and(|e| e.contains("prefs.json")));
        // a document of the wrong shape is ignored
        std::fs::write(dir.join("docs/prefs.json"), br#"{"version": 3, "items": {"x": 1}}"#).unwrap();
        assert_eq!(Docs::load(&dir).unwrap().get("prefs"), Some(doc(0, json!([]))));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
