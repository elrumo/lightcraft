//! XMP sidecar commands: save/read metadata to/from files, sidecar preferences.

use lightcraft_catalog::Op;
use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd, has_selection, str_param};
use crate::sidecar::SidecarNaming;
use crate::{Result, Session};

fn save(s: &mut Session, p: &Value) -> Result<Value> {
    let mut written = Vec::new();
    let mut merged = Vec::new();
    let mut backups = Vec::new();
    let mut failed = Vec::new();
    let ids = s.targets(p)?;
    let owners = crate::sidecar::StemOwners::among(&s.catalog, &ids);
    for id in ids {
        match s.save_sidecar_with(id, &owners) {
            Ok(r) => {
                s.sidecar_times.note(&r.path);
                let path = r.path.display().to_string();
                if r.merged {
                    merged.push(path.clone());
                }
                if let Some(b) = r.backup {
                    backups.push(json!({"path": path, "backup": b.display().to_string()}));
                }
                written.push(path);
            }
            Err(e) => failed.push(json!({"id": id.0, "error": e.to_string()})),
        }
    }
    s.save_sidecar_times();
    Ok(json!({"written": written, "merged": merged, "backups": backups, "failed": failed}))
}

fn read(s: &mut Session, p: &Value) -> Result<Value> {
    let mut ops = Vec::new();
    let mut read = Vec::new();
    let mut failed = Vec::new();
    let mut warnings = Vec::new();
    for id in s.targets(p)? {
        match s.read_sidecar_op_with_warnings(id) {
            Ok(Some((op, from, mut photo_warnings))) => {
                if from.extension().is_some_and(|e| e.eq_ignore_ascii_case("xmp")) {
                    s.sidecar_times.note(&from);
                }
                ops.push(op);
                read.push(json!({"id": id.0, "from": from.display().to_string()}));
                warnings.append(&mut photo_warnings);
            }
            Ok(None) => failed.push(json!({"id": id.0, "error": "no XMP sidecar or embedded XMP"})),
            Err(e) => failed.push(json!({"id": id.0, "error": e.to_string()})),
        }
    }
    if !ops.is_empty() {
        s.commit(crate::sidecar::READ_LABEL, Op::Batch { ops })?;
        // Don't auto-write the sidecars just read: that would add our `lc:settings` to one that had
        // only another app's `crs:` settings, and later reads prefer `lc:settings`, so that app's
        // next edits would be ignored (issue #934). The next edit, or Save Metadata, writes them.
        s.skip_auto_write = true;
    }
    s.save_sidecar_times();
    let mut result = json!({"read": read, "failed": failed});
    if !warnings.is_empty() {
        result["warnings"] = serde_json::to_value(warnings).unwrap_or_default();
    }
    Ok(result)
}

/// The largest XMP packet the XMP file commands take (a sidecar of a photo with many brushed masks is a few MB).
const MAX_XMP: usize = 16 << 20;

/// The history label of [`apply`].
pub const APPLY_LABEL: &str = "Import XMP";

fn xmp_param<'a>(cmd: &str, p: &'a Value) -> Result<&'a str> {
    let xmp = str_param(p, "xmp").ok_or_else(|| bad(cmd, "`xmp` (the packet's text) is required"))?;
    if xmp.len() > MAX_XMP {
        return Err(bad(cmd, "the XMP packet is larger than 16 MB"));
    }
    Ok(xmp)
}

/// The photo an XMP command reads a packet for: `id`, else the first of `ids` or of the selection, else the active one.
fn first_target(s: &Session, p: &Value) -> Result<Option<lightcraft_catalog::PhotoId>> {
    Ok(s.targets(p)?.into_iter().next().or(s.active()))
}

/// `photo.inspectXmp`: every field of a packet and what importing it would do.
fn inspect(s: &mut Session, p: &Value) -> Result<Value> {
    use crate::crs::{Field, FieldStatus, Setting};
    let xmp = xmp_param("photo.inspectXmp", p)?;
    let d = lightcraft_meta::parse_xmp(xmp).map_err(|e| bad("photo.inspectXmp", format!("not an XMP packet: {e}")))?;
    let id = first_target(s, p)?;
    let photo = id.and_then(|id| s.catalog.photo(id));
    let info = |field: &str, value: Option<String>, section: &'static str, reason: &'static str| Field {
        field: field.into(),
        value,
        count: None,
        section,
        status: FieldStatus::Info,
        to: Vec::new(),
        reason: Some(reason),
        detail: None,
        default: false,
    };
    let settings = d
        .lc_settings
        .as_deref()
        .and_then(|j| serde_json::from_str::<Value>(j).ok())
        .filter(|v| lightcraft_develop::DevelopSettings::from_json(v).is_ok());
    let (kind, mut fields) = if let Some(settings) = settings {
        // LightCraft's own settings: everything in them applies; one row per setting that isn't at its default
        let defaults = serde_json::to_value(lightcraft_develop::DevelopSettings::default()).unwrap_or_default();
        let delta = crate::json_delta(&defaults, &settings).unwrap_or_default();
        let mut rows = Vec::new();
        for (head, v) in delta.as_object().into_iter().flatten() {
            let leaves: Vec<(String, &Value)> = match v.as_object() {
                Some(m) if crate::crs::setting_label(head).is_empty() => m.iter().map(|(k, v)| (format!("{head}.{k}"), v)).collect(),
                _ => vec![(head.clone(), v)],
            };
            for (path, v) in leaves {
                let (value, count) = match v {
                    Value::Array(a) => (None, Some(a.len())),
                    Value::Object(_) => (None, None),
                    Value::String(t) => (Some(t.clone()), None),
                    other => (Some(other.to_string()), None),
                };
                rows.push(Field {
                    field: path.clone(),
                    value,
                    count,
                    section: setting_section(&path),
                    status: FieldStatus::Applied,
                    to: vec![Setting::new(&path)],
                    reason: None,
                    detail: None,
                    default: false,
                });
            }
        }
        // another app's fields beside them are not read: LightCraft's own settings win
        let mut foreign: Vec<&str> = d.properties.keys().filter_map(|k| k.strip_prefix("crs:")).map(|k| k.split('/').next().unwrap_or(k)).collect();
        foreign.dedup();
        rows.extend(foreign.into_iter().map(|k| info(k, None, crate::crs::section_of(k), "overridden")));
        ("lightcraft", rows)
    } else {
        let target = photo.map_or(crate::crs::Target::Any, |p| crate::crs::Target::for_photo(p));
        let (partial, mut rows) = crate::crs::fields(&d.properties, Some(&d.values), target, crate::crs_masks::DEFAULT_ASPECT);
        if !crate::crs::has_adjustments(&d.properties) {
            // `AlreadyApplied`: the pixels of the file it came with hold the edit already, and nothing is imported
            for r in rows.iter_mut().filter(|r| r.status != FieldStatus::Info) {
                (r.status, r.reason, r.to) = (FieldStatus::Info, Some("alreadyApplied"), Vec::new());
            }
            ("none", rows)
        } else {
            // what reading it for this photo leaves out besides: a crop on a file whose orientation can't be read, and a
            // saved Upright solved again
            if let Some(id) = id {
                let sc = s.edit_from_xmp(id, xmp)?;
                let crop_held =
                    partial.get("crop").is_some() && matches!(&sc.develop, Some(crate::sidecar::DevelopPatch::Partial(v)) if v.get("crop").is_none());
                let upright_solved = sc.warnings.iter().any(|w| w.code == crate::import::ImportWarningCode::UprightGeometry);
                for r in &mut rows {
                    if crop_held && r.to.iter().any(|t| t.path == "crop.geometry") {
                        (r.status, r.reason, r.to) = (FieldStatus::NotApplied, Some("cropFrame"), Vec::new());
                    }
                    if upright_solved && r.field == "PerspectiveUpright" {
                        r.reason = Some("uprightSolved");
                    }
                }
            }
            ("lightroom", rows)
        }
    };
    // the metadata the packet holds: never imported with an edit
    let m = &d.metadata;
    let nz = |v: &Option<String>| v.clone().filter(|t| !t.trim().is_empty());
    let meta: [(&str, Option<String>); 9] = [
        ("Rating", m.rating.map(|r| r.to_string())),
        ("Label", nz(&m.label)),
        ("Title", nz(&m.title)),
        ("Caption", nz(&m.caption)),
        ("Keywords", (!m.keywords.is_empty()).then(|| m.keywords.join(", "))),
        ("Creator", nz(&m.artist)),
        ("Copyright", nz(&m.copyright)),
        ("Location", [&m.sublocation, &m.city, &m.state, &m.country].into_iter().filter_map(nz).reduce(|a, b| format!("{a}, {b}"))),
        ("Capture Time", m.capture_time.map(|t| t.to_iso())),
    ];
    fields.extend(meta.into_iter().filter_map(|(k, v)| v.map(|v| info(k, Some(v), "Metadata", "metadata"))));
    let count = |st: FieldStatus| fields.iter().filter(|f| f.status == st).count();
    let counts = json!({"applied": count(FieldStatus::Applied), "notApplied": count(FieldStatus::NotApplied), "info": count(FieldStatus::Info)});
    Ok(json!({"kind": kind, "photo": id.map(|id| id.0), "fields": fields, "counts": counts}))
}

/// The panel a setting of LightCraft's own settings belongs to, by its path.
fn setting_section(path: &str) -> &'static str {
    match path.split('.').next().unwrap_or(path) {
        "light" | "hdr" => "Light",
        "wb" => "White Balance",
        "curve" => "Tone Curve",
        "color" | "treatment" => "Color",
        "mixer" => "Color Mixer",
        "bw_mix" => "B&W Mixer",
        "point_colors" => "Point Color",
        "grading" => "Color Grading",
        "effects" => "Effects",
        "vignette" => "Vignette",
        "grain" => "Grain",
        "glow" => "Glow",
        "film" => "Film",
        "detail" | "enhance" => "Detail",
        "optics" => "Optics",
        "profile" => "Profile",
        "calibration" => "Calibration",
        "geometry" => "Geometry",
        "crop" => "Crop",
        "masks" => "Masks",
        "spots" | "red_eye" => "Remove",
        "lens_blur" => "Lens Blur",
        _ => "Other",
    }
}

/// `photo.applyXmp`: a packet's edit on the target photos, one undo step.
fn apply(s: &mut Session, p: &Value) -> Result<Value> {
    let xmp = xmp_param("photo.applyXmp", p)?;
    let ids = s.targets(p)?;
    if ids.is_empty() {
        return Err(bad("photo.applyXmp", "no photo to apply the XMP edit to"));
    }
    let now = (s.clock)();
    let (mut ops, mut applied, mut unchanged, mut warnings) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for id in ids {
        let sc = s.edit_from_xmp(id, xmp)?;
        if sc.develop.is_none() {
            return Err(bad("photo.applyXmp", "the XMP packet holds no edit"));
        }
        let photo = s.catalog.photo(id).ok_or(lightcraft_catalog::CatalogError::NoPhoto(id))?;
        let changes = crate::sidecar::sidecar_ops(photo, &sc, &now, APPLY_LABEL);
        for mut w in sc.warnings {
            w.path = photo.file_name.clone();
            warnings.push(w);
        }
        if changes.is_empty() {
            unchanged.push(id.0);
        } else {
            applied.push(id.0);
            ops.push(crate::sidecar::with_history(changes));
        }
    }
    if !ops.is_empty() {
        s.commit(APPLY_LABEL, Op::Batch { ops })?;
    }
    let mut result = json!({"applied": applied, "unchanged": unchanged});
    if !warnings.is_empty() {
        result["warnings"] = serde_json::to_value(warnings).unwrap_or_default();
    }
    Ok(result)
}

/// `photo.exportXmp`: the photo's edit and metadata as an XMP packet, written to `path` when one is given.
fn export(s: &mut Session, p: &Value) -> Result<Value> {
    let id = first_target(s, p)?.ok_or_else(|| bad("photo.exportXmp", "no photo to export the edit of"))?;
    let photo = s.catalog.photo(id).ok_or(lightcraft_catalog::CatalogError::NoPhoto(id))?;
    let xmp = crate::sidecar::sidecar_packet(photo, &s.catalog);
    let stem = std::path::Path::new(&photo.file_name).file_stem().map_or_else(|| "Photo".to_string(), |n| n.to_string_lossy().to_string());
    let name = format!("{stem}.xmp");
    let mut result = json!({"xmp": xmp, "name": name});
    if let Some(path) = str_param(p, "path") {
        lightcraft_catalog::safe_file::write_atomic(std::path::Path::new(path), xmp.as_bytes())
            .map_err(|e| crate::EngineError::Other(format!("{path}: {e}")))?;
        result["path"] = json!(path);
    }
    Ok(result)
}

fn prefs(s: &mut Session, p: &Value) -> Result<Value> {
    if let Some(a) = p.get("autoWrite").and_then(Value::as_bool) {
        s.xmp.auto_write = a;
    }
    if let Some(n) = str_param(p, "naming") {
        s.xmp.naming = match n {
            "stem" => SidecarNaming::Stem,
            "full" => SidecarNaming::Full,
            other => return Err(bad("library.xmpPreferences", format!("unknown naming `{other}` (stem|full)"))),
        };
    }
    if p.as_object().is_some_and(|o| !o.is_empty()) {
        s.save_prefs()?;
    }
    Ok(serde_json::to_value(s.xmp).unwrap_or_default())
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "photo.saveMetadataToFile",
            "Save Metadata to File",
            ["Photo"],
            Some("Cmd+S"),
            "{ids?} — writes each photo's XMP sidecar (metadata + develop settings) next to the original, merged into an existing one (other apps' data kept; an unreadable one is backed up first) → {written: [path], merged: [path], backups: [{path, backup}], failed}",
            has_selection,
            save
        ),
        cmd!(
            "photo.readMetadataFromFile",
            "Read Metadata from File",
            ["Photo"],
            None,
            "{ids?} — reads each photo's XMP sidecar, or embedded XMP in raw/DNG and JPEG files when no sidecar exists: metadata and develop settings, one undo step; never writes the sidecar, even with auto-write on; JPEG uses the first complete standard XMP APP1 before SOS/EOI, within 16 MiB of headers and 4096 marker/fill steps; an unreadable sidecar fails without embedded fallback → {read, failed, warnings?: [{path, code, profile?, mode?, fields?}]}",
            has_selection,
            read
        ),
        cmd!(
            "photo.inspectXmp",
            "Inspect XMP Edit",
            [],
            None,
            "{xmp: the packet's text, id?} — what importing an XMP file's edit into the photo (`id`, else the first selected, else the active one) would do, without changing anything: one row per field. Another app's `crs:` fields are carried over, left out (why) or only information; LightCraft's own settings apply whole (other apps' fields beside them are not read); metadata is listed, never imported → {kind: lightroom|lightcraft|none, photo?, fields: [{field, value?, count?, section, status: applied|notApplied|info, to?: [{path, label}], reason?: profile|mask|spot|whiteBalance|unsupported|cropFrame|uprightSolved|bookkeeping|panelSwitch|analysis|auto|defaultProfile|unused|overridden|alreadyApplied|metadata, detail?, default}], counts: {applied, notApplied, info}}",
            always,
            inspect
        ),
        cmd!(
            "photo.applyXmp",
            "Import XMP Edit",
            [],
            None,
            "{xmp: the packet's text, ids?} — puts an XMP file's edit (another app's `crs:` settings or LightCraft's own) on each photo, as Read Metadata from File reads a sidecar but without its metadata; one undo step → {applied: [id], unchanged: [id], warnings?}",
            has_selection,
            apply
        ),
        cmd!(
            "photo.exportXmp",
            "Export XMP Edit",
            [],
            None,
            "{id?, path?} — the photo's edit and metadata as an XMP packet (LightCraft's own settings, which only LightCraft reads, as in its sidecars), written to `path` when given → {xmp, name: <file stem>.xmp, path?}",
            has_selection,
            export
        ),
        cmd!(
            "library.xmpPreferences",
            "XMP Sidecar Preferences",
            [],
            None,
            "{autoWrite?: bool, naming?: stem (IMG_1.xmp, default) | full (IMG_1.CR3.xmp)} → current preferences (saved with the library)",
            always,
            prefs
        ),
        cmd!(
            "library.retrySidecars",
            "Write Waiting XMP Sidecars",
            [],
            None,
            "{} — writes again the automatic XMP sidecars that failed (folder offline, read-only or full) whose originals are reachable now → {written, waiting}",
            always,
            |s, _| {
                let written = s.retry_unsaved_sidecars();
                Ok(json!({"written": written, "waiting": s.unsaved_sidecars()}))
            }
        ),
        cmd!("library.toggleAutoWriteXmp", "Automatically Write Changes into XMP", ["File"], None, "{} → {autoWrite}", always, |s, _| {
            let on = !s.xmp.auto_write;
            prefs(s, &json!({"autoWrite": on}))
        }),
    ]
}
