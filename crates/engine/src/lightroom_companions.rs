//! Files a Lightroom (cloud) library keeps beside its catalog that belong with a photo: the results of
//! Edit in Photoshop (`IMG_0001-Edit.tif`, `IMG_0001-Edit.psb`), in the catalog's `originals` folder and
//! in the library's `TemporaryEdits` folder.
//!
//! [`crate::lightroom_albums`] matches each to the library photo of the same name and puts it beside
//! that photo (a file on this disk) or sends it to the server that holds the photo.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// The server this library syncs with, to send it files that belong beside its photos.
#[derive(Clone, Debug)]
pub struct Server {
    pub url: String,
    pub token: String,
}

/// `name` as a URL query value.
fn encode(name: &str) -> String {
    name.bytes().fold(String::new(), |mut out, b| {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
        out
    })
}

impl Server {
    /// Send the file at `file` to the server, to be kept beside the photo with content hash `hash` under `name`
    /// (`PUT /api/companions/<hash>?name=…`; see the server's `companion` module).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn put_companion(&self, hash: &str, name: &str, file: &Path) -> Result<(), String> {
        use crate::cloud_sync::{Body, Task, run};
        let url = format!("{}/api/companions/{hash}?name={}", self.url.trim_end_matches('/'), encode(name));
        let task = Task::Http { id: 0, method: "PUT", url, token: self.token.clone(), body: Body::File(file.to_string_lossy().into_owned()), save_to: None };
        let done = run(&task);
        match done.status {
            200..=299 => Ok(()),
            0 => Err(done.body),
            404 => Err("this server can't take files beside photos (it is older than this app)".into()),
            s => Err(format!("the server answered {s}")),
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub fn put_companion(&self, _hash: &str, _name: &str, _file: &Path) -> Result<(), String> {
        Err("browser builds don't send files to the server".into())
    }
}

/// What a Photoshop round trip can leave.
const EXTENSIONS: [&str; 7] = ["tif", "tiff", "psb", "psd", "jpg", "jpeg", "png"];
/// The most files looked at, and how deep.
const MAX_FILES: usize = 200_000;
const MAX_DEPTH: usize = 8;

/// An edit file found beside a catalog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditFile {
    pub path: PathBuf,
    /// Its file name, as it is put beside the photo.
    pub name: String,
    /// The lower-case name, without extension, of the photo it came from.
    pub original_stem: String,
}

/// The photo an edit file's name says it came from: `2V2A9200-Edit.psb` and `2V2A9200-Edit-2.tif` are
/// edits of `2v2a9200`; any other name is not an edit file.
pub fn original_stem(name: &str) -> Option<String> {
    let path = Path::new(name);
    let ext = path.extension()?.to_string_lossy().to_lowercase();
    if !EXTENSIONS.contains(&ext.as_str()) {
        return None;
    }
    let stem = path.file_stem()?.to_string_lossy().to_lowercase();
    let at = stem.rfind("-edit")?;
    let (original, rest) = (stem.get(..at)?, stem.get(at + "-edit".len()..)?);
    let counted = rest.strip_prefix('-').is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    (!original.is_empty() && (rest.is_empty() || counted)).then(|| original.to_string())
}

/// The edit files of the library the catalog at `catalog` belongs to: under `originals` beside it and in
/// the `TemporaryEdits` folder next to the catalog's own folder. Sorted by path.
pub fn find_edit_files(catalog: &Path, cancel: &AtomicBool) -> Vec<EditFile> {
    let Some(dir) = catalog.parent() else { return Vec::new() };
    let mut roots: Vec<PathBuf> = Vec::new();
    let mut seen = HashSet::new();
    let candidates = [dir.join("originals"), dir.join("Originals"), dir.parent().map(|p| p.join("TemporaryEdits")).unwrap_or_default()];
    for root in candidates {
        // (a case-insensitive disk names one folder both ways)
        if let Ok(real) = std::fs::canonicalize(&root)
            && real.is_dir()
            && seen.insert(real.clone())
        {
            roots.push(real);
        }
    }
    let mut out = Vec::new();
    let mut looked = 0usize;
    let mut stack: Vec<(PathBuf, usize)> = roots.into_iter().map(|r| (r, 0)).collect();
    while let Some((folder, depth)) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&folder) else { continue };
        for entry in entries.flatten() {
            looked += 1;
            if looked > MAX_FILES {
                break;
            }
            let Ok(kind) = entry.file_type() else { continue };
            let path = entry.path();
            if kind.is_dir() {
                if depth < MAX_DEPTH {
                    stack.push((path, depth + 1));
                }
            } else if kind.is_file()
                && let Some(name) = entry.file_name().to_str().map(str::to_string)
                && let Some(original_stem) = original_stem(&name)
            {
                out.push(EditFile { path, name, original_stem });
            }
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_encoded_for_a_url() {
        assert_eq!(encode("IMG 1-Edit.psb"), "IMG%201-Edit.psb");
        assert_eq!(encode("日.tif"), "%E6%97%A5.tif");
    }

    #[test]
    fn edit_files_are_named_after_their_photo() {
        assert_eq!(original_stem("2V2A9200-Edit.psb").as_deref(), Some("2v2a9200"));
        assert_eq!(original_stem("IMG_1-Edit-2.TIF").as_deref(), Some("img_1"));
        assert_eq!(original_stem("a-b-edit.jpg").as_deref(), Some("a-b"));
        for no in ["IMG_1.psb", "IMG_1-Edit.cr3", "-Edit.tif", "IMG_1-Edited.tif", "IMG_1-Edit-x.tif", "Edits.db", "IMG_1-Edit"] {
            assert_eq!(original_stem(no), None, "{no}");
        }
    }

    #[test]
    fn edit_files_are_found_in_originals_and_temporary_edits() {
        let base = std::env::temp_dir().join(format!("lc-edit-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let catalog_dir = base.join("catalog");
        std::fs::create_dir_all(catalog_dir.join("originals/2023/2023-10-17")).unwrap();
        std::fs::create_dir_all(base.join("TemporaryEdits")).unwrap();
        std::fs::write(catalog_dir.join("originals/2023/2023-10-17/A-Edit.psb"), b"1").unwrap();
        std::fs::write(catalog_dir.join("originals/2023/2023-10-17/A.CR3"), b"2").unwrap();
        std::fs::write(base.join("TemporaryEdits/B-Edit.tif"), b"3").unwrap();
        std::fs::write(base.join("TemporaryEdits/Edits.db"), b"4").unwrap();
        let found = find_edit_files(&catalog_dir.join("Managed Catalog.mcat"), &AtomicBool::new(false));
        let names: Vec<_> = found.iter().map(|f| (f.name.as_str(), f.original_stem.as_str())).collect();
        // (by path: `TemporaryEdits` sorts before `catalog`)
        assert_eq!(names, [("B-Edit.tif", "b"), ("A-Edit.psb", "a")]);
        let _ = std::fs::remove_dir_all(base);
    }
}
