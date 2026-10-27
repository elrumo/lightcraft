use super::selection::strs;
use super::*;

/// The auto-import folder's visible files with their sizes (the file-system half of
/// `library.autoImportScan`; the app lists on a worker thread and passes `listing`).
pub fn list_auto_import_folder(folder: &str) -> std::result::Result<Vec<(String, u64)>, String> {
    AutoImportWalk::new(folder, false, Vec::new()).list().map(|l| l.files)
}

/// Folder levels an auto import with subfolders lists (the watched folder's own entries are
/// level 1).
pub const AUTO_IMPORT_MAX_DEPTH: usize = 16;
/// Directory entries an auto import with subfolders looks at per scan, at most.
pub const AUTO_IMPORT_MAX_ENTRIES: usize = 200_000;

/// How the watched folder is listed (`library.autoImportScan`): built from the session on the UI
/// thread ([`AutoImportWalk::of`]), listed on a worker thread ([`AutoImportWalk::list`]).
#[derive(Clone, Debug)]
pub struct AutoImportWalk {
    pub folder: String,
    /// Also list subfolders (photos only, by extension), up to [`AUTO_IMPORT_MAX_DEPTH`] levels and
    /// [`AUTO_IMPORT_MAX_ENTRIES`] entries. Hidden folders and folder links are never entered.
    pub subfolders: bool,
    /// Folders never entered: the library's own (where auto import's copies go).
    pub skip: Vec<std::path::PathBuf>,
    /// Bounds of a listing with subfolders.
    pub max_depth: usize,
    pub max_entries: usize,
}

/// What [`AutoImportWalk::list`] found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AutoImportListing {
    /// (path, size) of each file.
    pub files: Vec<(String, u64)>,
    /// What was left out and why (an unreadable subfolder, a bound reached).
    pub warnings: Vec<String>,
}

impl AutoImportWalk {
    /// The session's auto-import listing, if a watched folder is set.
    pub fn of(s: &Session) -> Option<Self> {
        let d = &s.import_defaults;
        let folder = d.auto_folder.clone()?;
        // auto import copies into the library's Originals, so skipping the library also skips
        // the destination when it sits inside the watched folder
        let skip = s.library.as_ref().filter(|l| l.on_disk).map(|l| vec![l.dir.clone()]).unwrap_or_default();
        Some(AutoImportWalk::new(&folder, d.auto_subfolders, skip))
    }

    /// A listing of `folder` with the default bounds.
    pub fn new(folder: &str, subfolders: bool, skip: Vec<std::path::PathBuf>) -> Self {
        AutoImportWalk { folder: folder.to_string(), subfolders, skip, max_depth: AUTO_IMPORT_MAX_DEPTH, max_entries: AUTO_IMPORT_MAX_ENTRIES }
    }

    /// List the watched folder (an error only when the folder itself can't be read).
    pub fn list(&self) -> std::result::Result<AutoImportListing, String> {
        use std::path::{Path, PathBuf};
        let folder = &self.folder;
        let rd = std::fs::read_dir(folder).map_err(|e| format!("{folder}: {e}"))?;
        let mut out = AutoImportListing::default();
        if !self.subfolders {
            out.files = rd
                .flatten()
                .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
                .filter_map(|e| {
                    let m = std::fs::metadata(e.path()).ok()?;
                    m.is_file().then(|| (e.path().to_string_lossy().to_string(), m.len()))
                })
                .collect();
            return Ok(out);
        }
        // skipped folders compared by their canonical paths: a folder's canonical path is the
        // canonical root joined with its path below the root (folder links aren't followed)
        let canon = |p: &Path| std::fs::canonicalize(p).ok();
        let skip: Vec<PathBuf> = self.skip.iter().filter_map(|p| canon(p)).collect();
        let root = canon(Path::new(folder)).unwrap_or_else(|| PathBuf::from(folder));
        if skip.contains(&root) {
            return Ok(out);
        }
        let mut entries = 0usize;
        // stopped at the entry bound / left folders below the depth bound unlisted
        let (mut full, mut deep) = (false, false);
        let mut bad_names = 0usize;
        // (folder, its canonical path, its level: the watched folder's entries are level 1)
        let mut stack: Vec<(PathBuf, PathBuf, usize, Option<std::fs::ReadDir>)> = vec![(PathBuf::from(folder), root, 1, Some(rd))];
        while let Some((dir, canon_dir, level, rd)) = stack.pop() {
            let rd = match rd {
                Some(rd) => rd,
                None => match std::fs::read_dir(&dir) {
                    Ok(rd) => rd,
                    // gone since it was listed (moved or deleted while scanning): nothing to say
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(e) => {
                        out.warnings.push(format!("Auto Import skipped {}: {e}", dir.display()));
                        continue;
                    }
                },
            };
            let mut below = Vec::new();
            for e in rd.flatten() {
                if entries >= self.max_entries {
                    full = true;
                    break;
                }
                entries = entries.saturating_add(1);
                let name = e.file_name();
                let Some(name) = name.to_str() else {
                    bad_names = bad_names.saturating_add(1);
                    continue;
                };
                if name.starts_with('.') {
                    continue;
                }
                let Ok(ft) = e.file_type() else { continue };
                let path = e.path();
                if ft.is_dir() {
                    let c = canon_dir.join(name);
                    if skip.contains(&c) {
                        continue;
                    }
                    if level >= self.max_depth {
                        deep = true;
                        continue;
                    }
                    below.push((path, c, level.saturating_add(1), None));
                    continue;
                }
                // a link to a file is a file; a link to a folder isn't followed (no loops)
                if level > 1 && !crate::import::is_supported(&path) {
                    continue;
                }
                let Ok(m) = std::fs::metadata(&path) else { continue };
                if m.is_file() {
                    out.files.push((path.to_string_lossy().to_string(), m.len()));
                }
            }
            if full {
                break;
            }
            below.sort_by(|a, b| b.0.cmp(&a.0));
            stack.extend(below);
        }
        if bad_names > 0 {
            out.warnings.push(format!("Auto Import skipped {bad_names} file or folder names that aren't valid Unicode in {folder}"));
        }
        if full {
            out.warnings.push(format!(
                "Auto Import stopped looking in {folder} after {} files and folders: photos beyond them aren't imported",
                self.max_entries
            ));
        }
        if deep {
            out.warnings.push(format!("Auto Import looks {} folder levels deep in {folder}: photos further down aren't imported", self.max_depth));
        }
        Ok(out)
    }
}

/// `library.import`'s params, checked (no file-system calls).
pub struct ImportRequest {
    pub paths: Vec<String>,
    pub opts: crate::import::ImportOptions,
    pub album: Option<u64>,
    pub album_name: Option<String>,
}

/// Parse and check `library.import`'s params (the app's import task runs the import itself, on a
/// worker thread, with the same options).
pub fn import_params(s: &Session, p: &Value) -> Result<ImportRequest> {
    let paths = strs(p, "paths");
    let mode = match str_param(p, "mode").unwrap_or("add") {
        "add" => crate::import::ImportMode::Add,
        "copy" => crate::import::ImportMode::Copy,
        "move" => crate::import::ImportMode::Move,
        other => return Err(bad("library.import", format!("unknown mode `{other}` (add|copy|move)"))),
    };
    let preset = match str_param(p, "preset").filter(|x| !x.is_empty()) {
        Some(id) => Some(s.presets.iter().find(|x| x.id == id).cloned().ok_or_else(|| bad("library.import", format!("unknown preset `{id}`")))?),
        None => None,
    };
    let album = p.get("album").and_then(Value::as_u64);
    if let Some(a) = album
        && s.catalog.album(AlbumId(a)).is_none_or(|al| al.folder || al.is_smart())
    {
        return Err(bad("library.import", "album must be a regular album"));
    }
    let organize = match str_param(p, "organize") {
        Some(o) => crate::import::Organize::parse(o).ok_or_else(|| {
            bad("library.import", format!("unknown organize `{o}` (date|month|flat, or a folder template like {{date:%Y}}/{{date:%Y%m%d}})"))
        })?,
        None => Default::default(),
    };
    if let crate::import::Organize::Template(t) = &organize
        && let Some(e) = crate::rename::folder_template_error(t)
    {
        return Err(bad("library.import", e));
    }
    let metadata_preset = str_param(p, "metadataPreset").map(str::trim).filter(|n| !n.is_empty()).map(str::to_string);
    if let Some(n) = &metadata_preset
        && !s.metadata_presets.iter().any(|m| m.name.eq_ignore_ascii_case(n))
    {
        return Err(bad("library.import", format!("unknown metadata preset `{n}`")));
    }
    let on_deleted = match str_param(p, "onDeleted") {
        Some(v) => {
            crate::import::OnDeleted::parse(v).ok_or_else(|| bad("library.import", format!("unknown onDeleted `{v}` (restore|skip|fresh)")))?
        }
        None => Default::default(),
    };
    let opts = crate::import::ImportOptions {
        on_deleted,
        mode,
        preset,
        keywords: strs(p, "keywords"),
        destination: str_param(p, "destination").map(str::to_string),
        organize,
        rename: str_param(p, "rename").map(str::to_string),
        rename_start: p.get("renameStart").and_then(Value::as_u64).unwrap_or(1) as usize,
        metadata_preset,
        convert_dng: bool_or(p, "dng", false),
        local: bool_or(p, "local", false),
        motions: motions_param(p)?,
    };
    let album_name = str_param(p, "albumName").map(str::trim).filter(|n| !n.is_empty()).map(str::to_string);
    Ok(ImportRequest { paths, opts, album, album_name })
}

/// `library.import`'s `motions`: `{still path: clip path}`, Live Photos' clips (at most one per still given).
fn motions_param(p: &Value) -> Result<std::collections::BTreeMap<String, String>> {
    const MAX: usize = 100_000;
    let Some(m) = p.get("motions").filter(|v| !v.is_null()) else { return Ok(Default::default()) };
    let m = m.as_object().ok_or_else(|| bad("library.import", "`motions` must be {still path: clip path}"))?;
    if m.len() > MAX {
        return Err(bad("library.import", "too many `motions`"));
    }
    m.iter()
        .map(|(still, clip)| {
            let clip = clip
                .as_str()
                .filter(|c| !c.is_empty())
                .ok_or_else(|| bad("library.import", format!("`motions`: the clip of {still} must be a path")))?;
            Ok((still.clone(), clip.to_string()))
        })
        .collect()
}

/// After a batch of an import was committed: the album (a new one named `album_name` when there
/// is no `album` and something was imported) gets the photos. Returns the report as JSON (with
/// `album`).
pub fn import_batch_done(s: &mut Session, report: crate::import::ImportReport, mut album: Option<u64>, album_name: Option<&str>) -> Result<Value> {
    let mut report = serde_json::to_value(report).unwrap_or_default();
    // photos an import restored from Recently Deleted take part like new ones (album, selection)
    let imported: Vec<u64> =
        ["imported", "restored"].iter().flat_map(|k| report[*k].as_array().into_iter().flatten().filter_map(Value::as_u64)).collect();
    if album.is_none()
        && let Some(name) = album_name
        && !imported.is_empty()
    {
        let r = s.execute("album.create", &json!({"name": name}))?;
        album = r["id"].as_u64();
    }
    if let Some(a) = album
        && !imported.is_empty()
    {
        // (album.addPhotos wants a selection: the new photos are about to be it)
        s.selection = Selection::single(PhotoId(imported[0]));
        s.execute("album.addPhotos", &json!({"id": a, "ids": imported}))?;
        report["album"] = json!(a);
    }
    Ok(report)
}
