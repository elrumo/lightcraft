//! Preset files: import `.lcpreset`, XMP, `.lrtemplate`, DNG-preset, Luminar looks and zip bundles; export `.lcpreset`.

use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd, str_param};
use crate::preset_import::{group_from_dir, read_presets};
use crate::presets::{LCPRESET_EXT, expand_preset_paths, imported_preset_id, to_lcpreset};
use crate::{Result, Session};

fn strings(p: &Value, key: &str) -> Option<Vec<String>> {
    p.get(key).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
}

/// What a preset import has done so far.
#[derive(Default)]
struct Imports {
    imported: Vec<Value>,
    failed: Vec<Value>,
    skipped: usize,
}

impl Imports {
    /// Add the presets read from `file` (or say why none could be).
    fn take(
        &mut self,
        s: &mut Session,
        file: &str,
        read: std::result::Result<Vec<crate::preset_import::Imported>, String>,
        group: Option<&str>,
        dry: bool,
    ) {
        let list = match read {
            Ok(list) => list,
            Err(e) => {
                self.failed.push(json!([file, e]));
                return;
            }
        };
        for mut it in list {
            if let Some(g) = group {
                it.preset.group = g.to_string();
            }
            let (name, grp) = (it.preset.name.clone(), it.preset.group.clone());
            let id = if dry { Some(imported_preset_id(&it.preset)) } else { s.add_presets(vec![it.preset]).pop() };
            match id {
                Some(id) => self.imported.push(json!({"id": id, "name": name, "group": grp, "file": file, "unmapped": it.unmapped})),
                None => self.skipped += 1,
            }
        }
    }
}

fn import(s: &mut Session, p: &Value) -> Result<Value> {
    let paths = strings(p, "paths").unwrap_or_default();
    let files = crate::inline_files::from_params(p);
    if paths.is_empty() && files.is_empty() {
        return Err(bad("preset.import", "no paths or files"));
    }
    let group = str_param(p, "group").map(str::trim).filter(|g| !g.is_empty()).map(str::to_string);
    let dry = p.get("dryRun").and_then(Value::as_bool).unwrap_or(false);
    let mut out = Imports::default();
    for top in &paths {
        let from_dir = std::path::Path::new(top).is_dir();
        for f in expand_preset_paths(std::slice::from_ref(top)) {
            // presets in a folder go to a group named after it (unless the file names its own)
            // the path from the imported folder (inclusive) to the file's folder
            let base = std::path::Path::new(top).parent().unwrap_or(std::path::Path::new(""));
            let rel = std::path::Path::new(&f).parent().and_then(|d| d.strip_prefix(base).ok()).map(|d| d.to_string_lossy().to_string());
            let dir_group = rel.filter(|_| from_dir).and_then(|d| group_from_dir(&d));
            let read = std::fs::read(&f).map_err(|e| e.to_string()).and_then(|b| read_presets(&f, &b, dir_group));
            out.take(s, &f, read, group.as_deref(), dry);
        }
    }
    // files given by content: a name with folders in it (`Pack/Look.xmp`) names the group as a folder would
    for f in files {
        match f {
            Ok((name, bytes)) => {
                let dir_group = name.rsplit_once(['/', '\\']).and_then(|(d, _)| group_from_dir(d));
                let read = read_presets(crate::inline_files::base_name(&name), &bytes, dir_group);
                out.take(s, &name, read, group.as_deref(), dry);
            }
            Err((name, e)) => out.failed.push(json!([name, e])),
        }
    }
    Ok(json!({"imported": out.imported, "skipped": out.skipped, "failed": out.failed}))
}

fn export(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_param(p, "path").filter(|x| !x.trim().is_empty()).ok_or_else(|| bad("preset.export", "missing path"))?;
    let mut path = std::path::PathBuf::from(path);
    if path.extension().is_none() {
        path.set_extension(LCPRESET_EXT);
    }
    let ids = strings(p, "ids");
    let group = str_param(p, "group");
    let chosen: Vec<_> = s
        .presets
        .iter()
        .filter(|x| match (&ids, group) {
            (Some(ids), _) => ids.contains(&x.id),
            (None, Some(g)) => x.group == g,
            (None, None) => !x.builtin,
        })
        .cloned()
        .collect();
    if chosen.is_empty() {
        return Err(bad("preset.export", "no presets to export (create one first, or pass ids/group)"));
    }
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| crate::EngineError::Other(format!("{}: {e}", dir.display())))?;
    }
    std::fs::write(&path, to_lcpreset(&chosen)).map_err(|e| crate::EngineError::Other(format!("{}: {e}", path.display())))?;
    Ok(json!({"path": path.display().to_string(), "count": chosen.len()}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "preset.import",
            "Import Presets",
            [],
            None,
            "{paths?: [file or folder], files?: [{name, data: base64}] (files given by content, e.g. from a browser), group?, dryRun?} — .lcpreset, XMP presets, .lrtemplate, photos carrying edits (DNG/JPEG/TIFF \"DNG presets\"), Luminar looks (.lmp, .mplumpack collections) and .zip bundles of these; folders give their name as group; each preset gets an id named by its content (the same preset imported on two synced devices stays one); crs: fields mapped per docs/xmp-interop.md → {imported: [{id,name,group,file,unmapped}], skipped, failed}",
            always,
            import
        ),
        cmd!(
            "preset.export",
            "Export Presets",
            [],
            None,
            "{path, ids?: [presetId], group?: name} (default: all user presets) → {path, count}; writes a .lcpreset JSON file",
            always,
            export
        ),
    ]
}
