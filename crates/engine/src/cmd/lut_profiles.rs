//! Imported profiles: 3D LUTs used as creative profiles (Amount 0–200 % like the built-in ones). `.cube` files are
//! read as they are, `.3dl` files and Hald CLUT images are converted to `.cube` when imported, so a profile is stored
//! and synced in one format. A library on disk keeps a copy of each file in its `Profiles/` folder; the
//! list lives in the library's prefs and the LUTs are registered with the pipeline when the
//! library opens.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd, str_param};
use crate::{Result, Session};

/// One imported LUT profile.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LutProfile {
    /// `lut:<slug>`.
    pub id: String,
    pub name: String,
    pub group: String,
    /// The library's copy of the `.cube` file (empty without a library on disk).
    pub file: String,
}

fn profiles_dir(s: &Session) -> Option<PathBuf> {
    s.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.join("Profiles"))
}

/// Register every imported profile's LUT with the pipeline (missing files are skipped).
pub fn register_all(s: &Session) {
    for p in &s.lut_profiles {
        if let Some(l) = std::fs::read_to_string(&p.file).ok().and_then(|t| lightcraft_pipeline::lut::Lut3d::parse_cube(&t).ok()) {
            lightcraft_pipeline::lut::register(&p.id, l);
        }
    }
}

/// The biggest file read as a LUT, in bytes (a 129-point `.cube` is about 60 MB).
const LUT_FILE_MAX: u64 = 128 << 20;
/// The biggest Hald CLUT image read, in pixels (level 12: 1728 a side, 144 points a side).
const HALD_MAX_PIXELS: u64 = 3_000_000;
/// A table finer than this is sampled down to it on import: a 144-point one would be 70 MB of text, and a 65-point one is
/// the biggest the GPU renders.
const HALD_MAX_SIZE: usize = 65;

/// Is `name` a file `profile.import` reads: a `.cube` 3D LUT, an Autodesk `.3dl`, or a Hald CLUT image (`.png`)?
pub(crate) fn is_lut_file(name: &str) -> bool {
    let l = name.to_ascii_lowercase();
    l.ends_with(".cube") || l.ends_with(".3dl") || l.ends_with(".png")
}

/// A LUT found inside a folder or an archive: a `.cube` or `.3dl`, and a `.png` only when it says it is a Hald image
/// (a pack's preview pictures are no tables).
fn is_lut_inside(name: &str) -> bool {
    let l = name.to_ascii_lowercase();
    l.ends_with(".cube") || l.ends_with(".3dl") || (l.ends_with(".png") && l.contains("hald"))
}

/// The `.cube` text of the LUT file `name` holding `data`: a `.cube` as it is (its problems are told when it is parsed), a
/// `.3dl` or a Hald CLUT image converted. A converted table is stored and synced as `.cube` like any other.
fn cube_text(name: &str, data: &[u8]) -> std::result::Result<String, String> {
    let l = name.to_ascii_lowercase();
    if l.ends_with(".3dl") {
        lightcraft_pipeline::lut::Lut3d::parse_3dl(&String::from_utf8_lossy(data)).map(|t| t.to_cube())
    } else if l.ends_with(".png") {
        let (w, h, pixels) = lightcraft_codecs::decode_png_codes(data, HALD_MAX_PIXELS).map_err(|e| format!("not a Hald CLUT image ({e})"))?;
        if w != h {
            return Err(format!("not a Hald CLUT image: it is {w}×{h}, a square one is needed"));
        }
        let table = lightcraft_pipeline::lut::Lut3d::from_hald(&pixels, w as usize)?;
        Ok(if table.size > HALD_MAX_SIZE { table.resampled(HALD_MAX_SIZE) } else { table }.to_cube())
    } else {
        Ok(String::from_utf8_lossy(data).into_owned())
    }
}

/// The LUT files in `paths` (folders searched, zip bundles opened) as `.cube` text, and the files that couldn't be read with
/// the reason (`[name, reason]`).
fn cube_files(paths: &[String]) -> (Vec<(String, String, Option<String>)>, Vec<Value>) {
    // (name, text, group)
    let mut out = Vec::new();
    let mut failed = Vec::new();
    let stem = |f: &Path| f.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let read = |f: &Path| -> std::result::Result<Vec<u8>, String> {
        let size = std::fs::metadata(f).map_err(|e| e.to_string())?.len();
        if size > LUT_FILE_MAX {
            return Err(format!("too large ({} MB)", size >> 20));
        }
        std::fs::read(f).map_err(|e| e.to_string())
    };
    for p in paths {
        let path = Path::new(p);
        if path.is_dir() {
            let lut = |f: &Path| f.file_name().is_some_and(|n| is_lut_inside(&n.to_string_lossy()));
            for ep in crate::walk::files_in(path, None, crate::walk::Limits::default(), lut).files {
                let group = ep.parent().and_then(|d| d.file_name()).map(|n| n.to_string_lossy().to_string());
                match read(&ep).and_then(|b| cube_text(&ep.to_string_lossy(), &b)) {
                    Ok(t) => out.push((stem(&ep), t, group)),
                    Err(e) => failed.push(json!([stem(&ep), e])),
                }
            }
        } else if path.extension().is_some_and(|x| x.eq_ignore_ascii_case("zip")) {
            let b = match std::fs::read(path) {
                Ok(b) => b,
                Err(e) => {
                    failed.push(json!([stem(path), e.to_string()]));
                    continue;
                }
            };
            // (a zip of presets has none of these, and one that can't be read is the presets' to report)
            let z = crate::preset_import::read_zip_where(&b, is_lut_inside).unwrap_or_default();
            failed.extend(z.failed.iter().map(|(inner, why)| json!([inner, why])));
            for (inner, data) in z.files {
                let ip = Path::new(&inner);
                let group = ip
                    .parent()
                    .and_then(|d| d.file_name())
                    .map(|n| n.to_string_lossy().to_string())
                    .or_else(|| path.file_stem().map(|n| n.to_string_lossy().to_string()));
                let name = ip.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                match cube_text(&inner, &data) {
                    Ok(t) => out.push((name, t, group)),
                    Err(e) => failed.push(json!([name, e])),
                }
            }
        } else {
            match read(path).and_then(|b| cube_text(p, &b)) {
                Ok(t) => out.push((stem(path), t, None)),
                Err(e) => failed.push(json!([stem(path), e])),
            }
        }
    }
    (out, failed)
}

fn import(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "profile.import";
    let paths: Vec<String> =
        p.get("paths").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
    if paths.is_empty() {
        return Err(bad(C, "no paths"));
    }
    // a dry run reads and checks the files and says what it would add, writing nothing
    let dry = p.get("dryRun").and_then(Value::as_bool).unwrap_or(false);
    let dir = profiles_dir(s);
    if let Some(d) = dir.as_ref().filter(|_| !dry) {
        std::fs::create_dir_all(d).map_err(|e| bad(C, format!("{}: {e}", d.display())))?;
    }
    let (files, mut failed) = cube_files(&paths);
    let mut imported = Vec::new();
    for (name, text, group) in files {
        let lut = match lightcraft_pipeline::lut::Lut3d::parse_cube(&text) {
            Ok(l) => l,
            Err(e) => {
                failed.push(json!([name, e]));
                continue;
            }
        };
        let display = lut.title.clone().filter(|t| !t.trim().is_empty()).unwrap_or_else(|| name.clone());
        let group = super::super::preset_import::group_from_dir(&group.unwrap_or_default()).unwrap_or_else(|| "Imported".into());
        let slug = crate::presets::slug(&format!("{group}-{display}"));
        let mut id = format!("lut:{slug}");
        let mut n = 2;
        while s.lut_profiles.iter().any(|x| x.id == id) {
            id = format!("lut:{slug}-{n}");
            n += 1;
        }
        if dry {
            imported.push(json!({"id": id, "name": display, "group": group}));
            continue;
        }
        let file = match &dir {
            Some(d) => {
                let f = d.join(format!("{}.cube", id.trim_start_matches("lut:")));
                std::fs::write(&f, &text).map_err(|e| bad(C, format!("{}: {e}", f.display())))?;
                f.to_string_lossy().to_string()
            }
            // no library on disk: the registered LUT is all this session needs (a copy would only
            // be read back when a library opens, and that takes its own list from its prefs)
            None => String::new(),
        };
        lightcraft_pipeline::lut::register(&id, lut);
        s.lut_profiles.push(LutProfile { id: id.clone(), name: display.clone(), group: group.clone(), file });
        imported.push(json!({"id": id, "name": display, "group": group}));
    }
    if !dry {
        s.save_prefs()?;
        // presets that name these profiles use them now
        crate::profile_refs::refresh(s);
    }
    Ok(json!({"imported": imported, "failed": failed}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "profile.import",
            "Import Profiles",
            [],
            None,
            "{paths: [LUT file (.cube, .3dl, or a Hald CLUT .png), folder or .zip], dryRun? (check the files and say what would be imported, writing nothing)} — 3D LUTs become creative profiles (Amount 0–200 %; rendered on the CPU) in the profile browser's groups (folder / zip name, else Imported) → {imported: [{id, name, group}], failed}",
            always,
            import
        ),
        cmd!(
            query "profile.lutFile",
            "Profile LUT File",
            [],
            None,
            "{id: lut:…} → {file: path | null} the `.cube` file behind an imported profile or an app pack's profile: what a frontend that applies LUT profiles itself reads (the file is read-only to it). `null` for an id nothing is registered under, or a table with no file. Changes nothing",
            always,
            |s, p| {
                let id = str_param(p, "id").ok_or_else(|| bad("profile.lutFile", "missing id"))?;
                let file = s.lut_profiles.iter().chain(s.pack_profiles()).find(|x| x.id == id).map(|x| x.file.as_str()).filter(|f| !f.is_empty());
                Ok(json!({"file": file}))
            }
        ),
        cmd!("profile.deleteImported", "Delete Imported Profile", [], None, "{id: lut:…}", always, |s, p| {
            let id = str_param(p, "id").ok_or_else(|| bad("profile.deleteImported", "missing id"))?.to_string();
            let Some(i) = s.lut_profiles.iter().position(|x| x.id == id) else {
                return Err(bad("profile.deleteImported", format!("no imported profile `{id}`")));
            };
            let gone = s.lut_profiles.remove(i);
            if profiles_dir(s).is_some_and(|d| Path::new(&gone.file).starts_with(d)) {
                let _ = std::fs::remove_file(&gone.file);
            }
            lightcraft_pipeline::lut::unregister(&id);
            s.save_prefs()?;
            crate::profile_refs::refresh(s);
            Ok(json!({"deleted": id}))
        }),
    ]
}
