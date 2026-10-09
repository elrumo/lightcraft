//! `enhance.*`: AI Super Resolution and the models it needs (see `crate::enhance`).

use lightcraft_catalog::PhotoId;
use serde_json::{Value, json};

use super::{CommandSpec, bad, cmd, has_active};
use crate::enhance::{Enhancer, SUPER_RES_MODEL};
use crate::{EngineError, Result, Session};

/// Enlarge the photo 2× and add the result next to it (synchronously: the apps run the job on a
/// worker thread themselves, with progress).
fn super_res(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "enhance.superRes";
    let id = match p.get("id").and_then(Value::as_u64) {
        Some(i) => PhotoId(i),
        None => s.active().ok_or_else(|| bad(C, "no active photo"))?,
    };
    let limit = match p.get("maxMegapixels") {
        None => None,
        Some(v) => Some(
            v.as_f64()
                .filter(|m| m.is_finite() && *m > 0.0 && *m <= 1000.0)
                .ok_or_else(|| bad(C, "`maxMegapixels` must be a number from 0 to 1000"))?,
        ),
    };
    let before = s.enhancer.max_output_pixels;
    if let Some(m) = limit {
        s.enhancer.max_output_pixels = Some((m * 1e6) as usize);
    }
    let job = s.super_res_job(id, super::str_param(p, "dir"));
    s.enhancer.max_output_pixels = before;
    let done = job.map_err(EngineError::Other)?.run(&|_, _| true).map_err(EngineError::Other)?;
    s.finish_super_res(done)
}

fn status(s: &mut Session, _: &Value) -> Result<Value> {
    let (fetching, download) = s.enhancer.download_status();
    Ok(json!({
        "available": Enhancer::AVAILABLE,
        "dir": s.enhancer.dir.as_ref().map(|d| d.display().to_string()),
        "ready": s.enhancer.super_res_ready().is_ok(),
        "models": s.enhancer.status(),
        "download": {"id": fetching, "status": download},
    }))
}

fn download(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "enhance.model.download";
    let id = super::str_param(p, "id").unwrap_or(SUPER_RES_MODEL);
    if !Enhancer::AVAILABLE {
        return Err(EngineError::Other("AI enhancement is not available in this build".into()));
    }
    if p.get("acknowledged").and_then(Value::as_bool) != Some(true) {
        let what = s.enhancer.status().into_iter().find(|m| m.id == id);
        return Err(bad(
            C,
            match what {
                Some(m) => format!(
                    "{} is a {:.1} MB download under {} ({}), {}: pass `acknowledged: true` once the user has agreed to download it",
                    m.label,
                    m.size_bytes as f64 / 1e6,
                    m.licence,
                    m.licence_url,
                    m.credit
                ),
                None => format!("no enhancement model `{id}`"),
            },
        ));
    }
    let started = s.enhancer.start_download(id).map_err(EngineError::Other)?;
    Ok(json!({"started": started, "installed": s.enhancer.installed(id), "downloading": s.enhancer.download_status().1.running}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "enhance.superRes",
            "Super Resolution",
            [],
            None,
            "{id?, maxMegapixels?, dir?} — enlarge the photo 2× with AI (Nomos Uni SPAN): render it with its edits in sRGB, write `<name>-SR.tif` (16-bit, never replacing a file) next to it, add it stacked on the original with neutral settings and select it. The model must be installed (enhance.model.download). Output is limited to `maxMegapixels` (default 100) → {id, path, original, width, height}",
            has_active,
            super_res
        ),
        cmd!(
            query "enhance.model.status",
            "Enhancement Model Status",
            [],
            None,
            "{} — whether this build has AI Super Resolution, the models (installed, size, licence, credit, folder) and the download's progress → {available, dir, ready, models: [{id, label, installed, sizeBytes, licence, licenceUrl, credit, homeUrl, dir, mirrors, downloading}], download: {id, status: {running, done, total, file, error, finished}}}",
            super::always,
            status
        ),
        cmd!(
            query "enhance.model.download",
            "Download Enhancement Model",
            [],
            None,
            "{id?: nomos-span-2x, acknowledged: true} — download the model (about 4.5 MB, CC-BY-4.0, credited to its author) in the background; only after the user agreed. Watch enhance.model.status; enhance.model.cancel stops it (it resumes later) → {started, installed, downloading}",
            super::always,
            download
        ),
        cmd!(
            query "enhance.model.cancel",
            "Cancel Enhancement Model Download",
            [],
            None,
            "{} — stop the download (what has arrived is kept, and a new download resumes from it) → {cancelled}",
            super::always,
            |s, _| Ok(json!({"cancelled": s.enhancer.cancel_download()}))
        ),
    ]
}
