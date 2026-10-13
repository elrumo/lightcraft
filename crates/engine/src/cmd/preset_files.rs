//! Preset files: import `.lcpreset`, XMP, `.lrtemplate`, DNG-preset, Luminar looks and zip bundles; export `.lcpreset`.

use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd, str_param};
use crate::preset_import::{Read, group_from_dir, read_presets_report};
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
    /// A dry run adds nothing, so what it has counted is kept to tell repeats (the same preset as
    /// `.xmp` and as `.lrtemplate` in one pack) from new presets, as a real run would.
    counted: Vec<lightcraft_develop::Preset>,
}

impl Imports {
    /// Add the presets read from `file`, and say why the others couldn't be read. A dry run adds
    /// nothing; `settings` puts each preset's settings in its entry (for a preview).
    fn take(&mut self, s: &mut Session, file: &str, read: crate::preset_import::Read, group: Option<&str>, dry: bool, settings: bool) {
        self.failed.extend(read.failed.into_iter().map(|(f, why)| json!([f, why])));
        for mut it in read.presets {
            if let Some(g) = group {
                it.preset.group = g.to_string();
            }
            let (name, grp) = (it.preset.name.clone(), it.preset.group.clone());
            let shown = settings.then(|| it.preset.settings.clone());
            let id = if dry {
                let repeat = s.has_identical_preset(&it.preset)
                    || self.counted.iter().any(|q| q.name == it.preset.name && q.group == it.preset.group && q.settings == it.preset.settings);
                let id = imported_preset_id(&it.preset);
                if !repeat {
                    self.counted.push(it.preset);
                }
                (!repeat).then_some(id)
            } else {
                s.add_presets(vec![it.preset]).pop()
            };
            match id {
                Some(id) => {
                    let mut entry = json!({"id": id, "name": name, "group": grp, "file": file, "unmapped": it.unmapped});
                    if let (Some(settings), Some(o)) = (shown, entry.as_object_mut()) {
                        o.insert("settings".into(), settings);
                    }
                    self.imported.push(entry);
                }
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
    let settings = p.get("includeSettings").and_then(Value::as_bool).unwrap_or(false);
    let mut out = Imports::default();
    for top in &paths {
        let from_dir = std::path::Path::new(top).is_dir();
        for f in expand_preset_paths(std::slice::from_ref(top)) {
            // presets in a folder go to a group named after it (unless the file names its own)
            // the path from the imported folder (inclusive) to the file's folder
            let base = std::path::Path::new(top).parent().unwrap_or(std::path::Path::new(""));
            let rel = std::path::Path::new(&f).parent().and_then(|d| d.strip_prefix(base).ok()).map(|d| d.to_string_lossy().to_string());
            let dir_group = rel.filter(|_| from_dir).and_then(|d| group_from_dir(&d));
            let read = match std::fs::read(&f) {
                Ok(b) => read_presets_report(&f, &b, dir_group),
                Err(e) => Read { failed: vec![(f.clone(), e.to_string())], ..Default::default() },
            };
            out.take(s, &f, read, group.as_deref(), dry, settings);
        }
    }
    // files given by content: a name with folders in it (`Pack/Look.xmp`) names the group as a folder would
    for f in files {
        match f {
            Ok((name, bytes)) => {
                let dir_group = name.rsplit_once(['/', '\\']).and_then(|(d, _)| group_from_dir(d));
                let base = crate::inline_files::base_name(&name);
                let mut read = read_presets_report(base, &bytes, dir_group);
                // (failures name the file as it was given, folders and all)
                for (file, _) in &mut read.failed {
                    if let Some(rest) = file.strip_prefix(base) {
                        *file = format!("{name}{rest}");
                    }
                }
                out.take(s, &name, read, group.as_deref(), dry, settings);
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
            "{paths?: [file or folder], files?: [{name, data: base64}] (files given by content, e.g. from a browser), group?, dryRun? (report what a real run would add and skip, adding nothing), includeSettings? (with each imported preset's settings, for a preview)} — .lcpreset, XMP presets, .lrtemplate, photos carrying edits (DNG/JPEG/TIFF \"DNG presets\"), Luminar looks (.lmp, .mplumpack collections) and .zip bundles of these; folders give their name as group; each preset gets an id named by its content (the same preset imported on two synced devices stays one); crs: fields mapped per docs/xmp-interop.md → {imported: [{id,name,group,file,unmapped}], skipped, failed}",
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
