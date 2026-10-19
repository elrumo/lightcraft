//! Photo ▸ Edit In, the host half, for native frontends (egui keeps the same list in `ui.json`). The
//! engine makes the copy and brings back what an editor saved (`photo.editExternal`,
//! `photo.checkExternalEdits`); this keeps the files handed over, watched for two weeks, in a file of
//! the library's folder so the watch outlives a restart, and checks them when the app is active again.

use std::path::{Path, PathBuf};

use lightcraft_engine::catalog::Source;
use lightcraft_engine::cmd::{TrackedEdit, can_edit_original};
use lightcraft_engine::{EngineError, Session};
use serde_json::{Value, json};

/// The watch list, in the library's folder.
const FILE: &str = "external-edits.json";
/// Most originals one Open Original hands the editor (each starts it once).
const MAX_ORIGINALS: usize = 50;

/// The files handed to an editor and still watched.
#[derive(Default)]
pub struct Watch {
    files: Vec<TrackedEdit>,
}

/// Where the open library keeps the list (`None`: a demo or in-memory library, which keeps none).
fn path(s: &Session) -> Option<PathBuf> {
    s.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.join(FILE))
}

impl Watch {
    /// The list of the library `s` has open (empty when there is none, or it can't be read).
    pub fn load(s: &Session) -> Watch {
        let files = path(s).and_then(|p| std::fs::read(p).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Watch { files }
    }

    fn save(&self, s: &Session) {
        let Some(p) = path(s) else { return };
        let bytes = serde_json::to_vec_pretty(&self.files).unwrap_or_default();
        if let Err(e) = lightcraft_engine::export::write_file_durable(&p.to_string_lossy(), &bytes) {
            log::warn!("external edits: {e}");
        }
    }

    /// Photo ▸ Edit In: `photo.editExternal` with `p`, and the files it made or handed over are watched.
    /// → its answer `{mode, copies: [{path, id, original}], …}`; the host opens each copy in the editor.
    /// Without a `mode`, when every photo is an image file whose own file an editor can open (not a raw),
    /// nothing is made and `{ask: true}` is the answer: the frontend asks how to hand it over (Edit a Copy
    /// with LightCraft Adjustments, Edit a Copy, Edit Original) and calls again with that `mode`.
    pub fn start(&mut self, s: &mut Session, p: &Value) -> Result<Value, EngineError> {
        if p.get("mode").is_none() {
            let ids = s.targets(p)?;
            if !ids.is_empty() && ids.iter().all(|id| s.catalog.photo(*id).is_some_and(|ph| can_edit_original(ph))) {
                return Ok(json!({"ask": true}));
            }
        }
        let r = s.execute("photo.editExternal", p)?;
        if let Ok(track) = serde_json::from_value::<Vec<TrackedEdit>>(r["track"].clone()) {
            TrackedEdit::merge(&mut self.files, track);
            self.save(s);
        }
        Ok(r)
    }

    /// Bring back what the editor saved to the watched files (`photo.checkExternalEdits`): changed files
    /// are read again, a Save As beside one is added to its stack. → the engine's answer and what to
    /// tell the user (none when nothing came back).
    pub fn check(&mut self, s: &mut Session) -> Result<(Value, Option<String>), EngineError> {
        if self.files.is_empty() {
            return Ok((json!({"reloaded": [], "added": [], "files": []}), None));
        }
        let r = s.execute("photo.checkExternalEdits", &json!({"files": self.files}))?;
        if let Ok(list) = serde_json::from_value(r["files"].clone()) {
            self.files = list;
            self.save(s);
        }
        let added = r["added"].as_array().map_or(0, Vec::len);
        let name = |path: &str| Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let text = if added == 1 {
            Some(format!("Added {}, saved in the external editor, to its stack", name(r["added"][0]["path"].as_str().unwrap_or_default())))
        } else if added > 1 {
            Some(format!("Added {added} files saved in the external editor to their stacks"))
        } else if r["reloaded"].as_array().is_some_and(|a| !a.is_empty()) {
            Some("Updated the edits saved in the external editor".to_string())
        } else {
            None
        };
        Ok((r, text))
    }

    /// A phone cannot be told when an editor saves: what it sends back arrives as a file shared into the
    /// app. A file named as a watched one (same name without its extension and without the ` 2` or
    /// ` (1)` a share adds) is moved beside it under its name, where [`Watch::check`] takes it as the
    /// editor's save (same extension) or Save As (another one: added on top of the stack).
    /// → {claimed: [paths taken], reloaded, added}; the files not claimed are left where they are.
    // ponytail: matching is by name, so an editor that renames the file is not claimed and imports as a
    // photo of its own. Upgrade: ask which photo it belongs to in the import review.
    pub fn claim(&mut self, s: &mut Session, paths: &[String]) -> Result<(Value, Option<String>), EngineError> {
        let mut claimed = Vec::new();
        for from in paths {
            let from_path = Path::new(from);
            let key = stem_key(from);
            let Some(ext) = from_path.extension() else { continue };
            let Some(into) = self.files.iter().find(|t| stem_key(&t.path) == key).map(|t| Path::new(&t.path)) else { continue };
            let (Some(dir), Some(stem)) = (into.parent(), into.file_stem()) else { continue };
            // (not `with_extension`: a dot in the name would be taken for the extension)
            let to = dir.join(format!("{}.{}", stem.to_string_lossy(), ext.to_string_lossy()));
            match move_file(from_path, &to) {
                Ok(()) => claimed.push(from.clone()),
                Err(e) => log::warn!("external edits: {from}: {e}"),
            }
        }
        let (mut r, text) = self.check(s)?;
        r["claimed"] = json!(claimed);
        Ok((r, text))
    }
}

/// A file's name lower case, without its extension and without a trailing ` 2` or ` (1)`.
fn stem_key(name: &str) -> String {
    let stem = Path::new(name).file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    let number = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    match stem.rsplit_once(' ') {
        Some((head, tail)) if !head.is_empty() && number(tail.trim_matches(['(', ')'])) => head.to_string(),
        _ => stem,
    }
}

/// `from` as `to` (replacing it), across volumes too.
fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::rename(from, to).or_else(|_| std::fs::copy(from, to).and_then(|_| std::fs::remove_file(from)))
}

/// Photo ▸ Edit In ▸ Open Original in Other Editor: the files of the originals themselves (a raw stays
/// a raw: nothing is rendered, copied or added to the library) for the host to hand to the editor.
/// Demo photos and missing files are skipped and listed; when no photo has a file to open it fails
/// saying why. `p`: `{ids?}`. → {files: [{id, path}], skipped: [{id, reason}]}
pub fn originals(s: &Session, p: &Value) -> Result<Value, String> {
    let mut ids = s.targets(p).map_err(|e| e.to_string())?;
    ids.dedup();
    if ids.is_empty() {
        return Err("no photo selected".into());
    }
    if ids.len() > MAX_ORIGINALS {
        return Err(format!("{} photos selected: open at most {MAX_ORIGINALS} originals at a time", ids.len()));
    }
    let mut files = Vec::new();
    let mut skipped = Vec::new();
    for id in ids {
        // the path may come from an imported catalog: only a photo file LightCraft reads is
        // handed to the opener, never a program (#2135)
        match s.catalog.photo(id).map(|ph| (&ph.file_name, &ph.source)) {
            Some((name, Source::File { path })) => match lightcraft_engine::open_path::openable(path, name) {
                Ok(absolute) => files.push(json!({"id": id.0, "path": absolute})),
                Err(reason) => skipped.push(json!({"id": id.0, "reason": reason})),
            },
            Some(_) => skipped.push(json!({"id": id.0, "reason": "a demo photo has no original file"})),
            None => skipped.push(json!({"id": id.0, "reason": "no such photo"})),
        }
    }
    if files.is_empty() {
        let why = skipped.first().and_then(|s| s["reason"].as_str()).unwrap_or("no original file");
        return Err(if skipped.len() == 1 {
            format!("nothing to open: {why}")
        } else {
            format!("nothing to open: none of these photos has its original file ({why})")
        });
    }
    Ok(json!({"files": files, "skipped": skipped}))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Open Original hands the editor a photo file, never what a catalog's path names otherwise (#2135).
    #[cfg(unix)]
    #[test]
    fn originals_refuse_a_path_that_is_not_a_photo_file() {
        let dir = std::env::temp_dir().join(format!("lc-originals-gate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let pixels = vec![128u8; 16 * 16 * 3];
        let img = lightcraft_codecs::encode::EncodeImage::new(16, 16, 3, lightcraft_codecs::encode::Samples::U8(&pixels));
        let png = lightcraft_codecs::encode::encode_png(&img, &Default::default()).unwrap();
        let file = dir.join("IMG_1.png").to_string_lossy().to_string();
        std::fs::write(&file, png).unwrap();
        let mut s = Session::new();
        s.execute("library.import", &json!({"paths": [file.clone()]})).unwrap();
        let id = s.catalog.photos().next().unwrap().id.0;
        let ok = originals(&s, &json!({"ids": [id]})).unwrap();
        assert_eq!(ok["files"][0]["path"], file.as_str());
        // the same name, now a link to a program
        std::fs::remove_file(&file).unwrap();
        std::os::unix::fs::symlink("/bin/sh", &file).unwrap();
        let e = originals(&s, &json!({"ids": [id]})).unwrap_err();
        assert!(e.contains("points at"), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn names_match_without_extension_or_a_numbered_suffix() {
        assert_eq!(stem_key("/a/IMG_0001-Edit.tif"), "img_0001-edit");
        assert_eq!(stem_key("IMG_0001-Edit 2.psd"), "img_0001-edit");
        assert_eq!(stem_key("IMG_0001-Edit (1).png"), "img_0001-edit");
        assert_eq!(stem_key("2024.jpg"), "2024");
        assert_ne!(stem_key("IMG_0002.tif"), stem_key("IMG_0001.tif"));
    }
}
