//! Preset files: import `.lcpreset`, XMP, `.lrtemplate`, DNG-preset, Luminar looks and zip bundles; export `.lcpreset`.

use serde_json::{Value, json};

use super::mask_effect_files as mef;
use super::{CommandSpec, always, bad, cmd, str_param};
use crate::preset_import::{group_from_dir, read_presets_report};
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
    /// Local adjustment (brush) presets found in the files: they become mask effects.
    effects: Vec<mef::Found>,
    effect_failed: Vec<(String, String)>,
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
            // the profile it names (issue #1001): one the user imported, or ours for a basic one
            let mut profile = Value::Null;
            if let Some((r, _)) = crate::profile_refs::apply(&mut it.preset.settings, &s.lut_profiles) {
                let look = it.preset.settings["profile"]["look"].clone();
                it.unmapped.retain(|u| u != "Look");
                it.unmapped.extend(crate::profile_refs::report(&look, &r));
                profile = json!({"name": look["name"], "id": it.preset.settings["profile"].get("id")});
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
                    let mut entry = json!({"id": id, "name": name, "group": grp, "file": file, "unmapped": it.unmapped, "profile": profile});
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
            let rel = rel.filter(|_| from_dir);
            let bytes = match mef::read_capped(&f) {
                Ok(b) => b,
                Err(e) => {
                    out.failed.push(json!([f, e]));
                    continue;
                }
            };
            // local adjustment (brush) presets: Lightroom's .lrtemplate, Camera Raw's local .xmp, our .lceffect,
            // also inside zips, become mask effects
            let lower = f.to_ascii_lowercase();
            let maybe_local = [".lrtemplate", ".xmp", ".zip", ".lceffect"].iter().any(|x| lower.ends_with(x));
            let (before_found, before_failed) = (out.effects.len(), out.effect_failed.len());
            let local =
                maybe_local && mef::found_in(&f, &bytes, rel.as_deref().and_then(mef::group_for_dir), &mut out.effects, &mut out.effect_failed);
            // a zip may hold develop presets as well
            if local && !lower.ends_with(".zip") {
                continue;
            }
            let dir_group = rel.and_then(|d| group_from_dir(&d));
            let mut read = read_presets_report(&f, &bytes, dir_group);
            // the local presets of a bundle were read as effects above: not develop presets that failed
            let handled: Vec<&str> = (out.effects.iter().skip(before_found).map(|e| e.file.as_str()))
                .chain(out.effect_failed.iter().skip(before_failed).map(|(file, _)| file.as_str()))
                .collect();
            read.failed.retain(|(file, _)| !handled.contains(&file.as_str()));
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
    out.failed.extend(std::mem::take(&mut out.effect_failed).into_iter().map(|(f, e)| json!([f, e])));
    let (effects, dividers) = mef::add_found(s, std::mem::take(&mut out.effects), group.as_deref(), dry)?;
    Ok(json!({"imported": out.imported, "skipped": out.skipped, "failed": out.failed, "effects": effects, "effectDividers": dividers}))
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
    let target = path.to_string_lossy();
    s.check_write_target(&target).map_err(|e| bad("preset.export", e))?;
    crate::export::write_file_durable(&target, to_lcpreset(&chosen).as_bytes()).map_err(crate::EngineError::Other)?;
    Ok(json!({"path": path.display().to_string(), "count": chosen.len()}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "preset.import",
            "Import Presets",
            [],
            None,
            "{paths?: [file or folder], files?: [{name, data: base64}] (files given by content, e.g. from a browser), group?, dryRun? (report what a real run would add and skip, adding nothing), includeSettings? (with each imported preset's settings, for a preview)} — .lcpreset, XMP presets, .lrtemplate, photos carrying edits (DNG/JPEG/TIFF \"DNG presets\"), Luminar looks (.lmp, .mplumpack collections) and .zip bundles of these; folders give their name as group; each preset gets an id named by its content (the same preset imported on two synced devices stays one); crs: fields mapped per docs/xmp-interop.md; local adjustment presets (Lightroom .lrtemplate, Camera Raw local .xmp, .lceffect) found in `paths` become mask effects (`mask.effects`; divider presets become their groups); a profile the preset names applies when it was imported with profile.import (before or after) → {imported: [{id,name,group,file,unmapped,profile: {name, id}|null}], skipped, failed, effects: [{name,group,file,unmapped}], effectDividers: [{name,group,file}]}",
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
