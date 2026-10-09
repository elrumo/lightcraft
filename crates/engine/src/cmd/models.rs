//! `models.*`: the optional AI models as one list, for Settings ▸ AI Models and for agents.

use serde_json::{Value, json};

use super::{CommandSpec, bad, cmd};
use crate::{EngineError, Result, Session};

fn id_of<'a>(p: &'a Value, cmd: &str) -> Result<&'a str> {
    super::str_param(p, "id").ok_or_else(|| bad(cmd, "missing `id` (see models.list)"))
}

fn list(s: &mut Session, _: &Value) -> Result<Value> {
    let models = s.models();
    let disk: u64 = models.iter().map(|m| m.disk_bytes).fold(0, u64::saturating_add);
    Ok(json!({"models": models, "diskBytes": disk}))
}

fn download(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "models.download";
    let id = id_of(p, C)?;
    if p.get("acknowledged").and_then(Value::as_bool) != Some(true) {
        let what = s.models().into_iter().find(|m| m.id == id);
        return Err(bad(
            C,
            match what {
                Some(m) => format!(
                    "{} is a {:.1} MB download under {} ({}), {}: pass `acknowledged: true` once the user has agreed to download it",
                    m.label,
                    m.download_bytes as f64 / 1e6,
                    m.licence,
                    m.licence_url,
                    m.credit
                ),
                None => format!("no model `{id}` (see models.list)"),
            },
        ));
    }
    let started = s.start_model_download(id).map_err(EngineError::Other)?;
    let m = s.models().into_iter().find(|m| m.id == id);
    Ok(
        json!({"started": started, "installed": m.as_ref().is_some_and(|m| m.installed), "downloading": m.and_then(|m| m.download).is_some_and(|d| d.running)}),
    )
}

fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "models.delete";
    let id = id_of(p, C)?;
    if p.get("confirm").and_then(Value::as_bool) != Some(true) {
        let size = s.models().into_iter().find(|m| m.id == id).map_or(0, |m| m.disk_bytes);
        return Err(bad(
            C,
            format!(
                "this deletes the model's files from this device ({:.1} MB; it can be downloaded again): pass `confirm: true` once the user has agreed",
                size as f64 / 1e6
            ),
        ));
    }
    let d = s.delete_model(id).map_err(EngineError::Other)?;
    Ok(json!({"deleted": d}))
}

fn set_enabled(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "models.setEnabled";
    let id = id_of(p, C)?;
    let enabled = p.get("enabled").and_then(Value::as_bool).ok_or_else(|| bad(C, "missing boolean `enabled`"))?;
    s.set_model_enabled(id, enabled).map_err(EngineError::Other)?;
    Ok(json!({"id": id, "enabled": enabled, "disabled": s.models_disabled()}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "models.list",
            "AI Models",
            [],
            None,
            "{} — the optional AI models: what each is for, whether it is available in this build, installed on this device, turned on, its download size, the space it takes, its folder, where it downloads from (hosts), licence, credit, and its download's progress → {models: [{id, label, purpose, available, installed, enabled, downloadBytes, diskBytes, dir, sources, licence, licenceUrl, credit, homeUrl, download?: {running, done, total, file, error, finished}}], diskBytes}",
            super::always,
            list
        ),
        cmd!(
            query "models.download",
            "Download AI Model",
            [],
            None,
            "{id, acknowledged: true} — download a model in the background, only after the user agreed to its size and licence (it is refused without `acknowledged`). Watch models.list; models.cancel stops it (it resumes later) → {started, installed, downloading}",
            super::always,
            download
        ),
        cmd!(
            query "models.cancel",
            "Cancel AI Model Download",
            [],
            None,
            "{id} — stop that model's download (what has arrived is kept) → {cancelled}",
            super::always,
            |s, p| {
                let id = id_of(p, "models.cancel")?;
                Ok(json!({"cancelled": s.cancel_model_download(id)}))
            }
        ),
        cmd!(
            query "models.delete",
            "Delete AI Model",
            [],
            None,
            "{id, confirm: true} — delete the model's files (only those, and partial downloads) from this device to free the space; it can be downloaded again. Refused while it downloads → {deleted: {bytes, files}}",
            super::always,
            delete
        ),
        cmd!(
            query "models.setEnabled",
            "Turn AI Model On or Off",
            [],
            None,
            "{id, enabled} — turn a model off (its feature says so and does nothing; the files stay) or back on → {id, enabled, disabled: [ids turned off]}",
            super::always,
            set_enabled
        ),
    ]
}
