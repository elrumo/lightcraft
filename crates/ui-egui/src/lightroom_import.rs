//! Background Lightroom catalog inspection and import: egui's side of
//! [`lightcraft_apphost::lightroom`] (the runner both apps share). This file starts tasks from
//! `library.inspectLightroom` / `library.importLightroom`, polls them every frame and words the
//! outcome in a toast.

use std::path::PathBuf;
use std::sync::Arc;

use lightcraft_apphost::lightroom::{Finished, Kind, Wake};
use serde_json::Value;

use crate::LightcraftApp;

/// The words of the activity row, in the user's language.
fn words(w: &'static str) -> String {
    crate::i18n::tr(w).to_string()
}

/// A repaint when a worker has something for the owner thread.
fn wake(ctx: &egui::Context) -> Wake {
    let ctx = ctx.clone();
    Arc::new(move || ctx.request_repaint())
}

/// Whether a Lightroom task currently owns the catalog transition.
pub fn is_running(app: &LightcraftApp) -> bool {
    app.lightroom.running()
}

/// Say how a task ended.
fn announce(app: &mut LightcraftApp, ctx: &egui::Context, done: Finished) {
    let (kind, value) = (done.kind, done.value);
    let message = if let Some(e) = value.get("error").and_then(Value::as_str) {
        let warning = value.get("indexWarning").and_then(Value::as_str).map_or(String::new(), |w| format!("; index warning: {w}"));
        if kind == Kind::Inspect {
            crate::i18n::tr_format!("Lightroom inspection failed: {e}{warning}", e = e, warning = warning)
        } else {
            crate::i18n::tr_format!("Lightroom import failed: {e}{warning}", e = e, warning = warning)
        }
    } else if value.get("cancelled").and_then(Value::as_bool).unwrap_or(false) {
        crate::i18n::tr(if kind == Kind::Inspect { "Lightroom inspection cancelled" } else { "Lightroom import cancelled" }).into()
    } else if kind == Kind::Inspect {
        let photos = value["photos"].as_u64().unwrap_or(0);
        let collections = value["collections"].as_u64().unwrap_or(0);
        let missing = value["missing"].as_array().map_or(0, Vec::len);
        crate::i18n::tr_format!(
            "Lightroom catalog inspected: {photos} photos, {collections} collections, {missing} missing",
            photos = photos,
            collections = collections,
            missing = missing
        )
    } else {
        let report = value.get("report").unwrap_or(&value);
        let imported = report["imported"].as_u64().unwrap_or(0);
        let failed = report["failed"].as_array().map_or(0, Vec::len);
        let warning = value.get("indexWarning").and_then(Value::as_str);
        if let Some(warning) = warning {
            crate::i18n::tr_format!(
                "Lightroom import complete: {imported} photos added; index warning: {warning}",
                imported = imported,
                warning = warning
            )
        } else if failed == 0 {
            crate::i18n::tr_format!("Lightroom import complete: {imported} photos added", imported = imported)
        } else {
            crate::i18n::tr_format!("Lightroom import complete: {imported} photos added, {failed} failed", imported = imported, failed = failed)
        }
    };
    app.toast_for(ctx, message, 6.0);
}

/// Run Lightroom UI commands before the generic engine dispatcher.
pub fn command(app: &mut LightcraftApp, id: &str, p: &Value, ctx: &egui::Context) -> Result<Value, String> {
    let path = p.get("path").and_then(Value::as_str).map(PathBuf::from);
    let wait = p.get("wait").and_then(Value::as_bool).unwrap_or(false);
    let status = p.get("status").and_then(Value::as_bool).unwrap_or(false) || path.is_none();
    if status {
        if p.get("cancel").and_then(Value::as_bool).unwrap_or(false) {
            app.lightroom.cancel();
        }
        if wait && app.lightroom.running() {
            return wait_for(app, ctx, std::time::Duration::from_secs(600));
        }
        return Ok(app.lightroom.status());
    }
    let Some(path) = path else { return Err("missing `path`".into()) };
    if app.import.is_some() || app.scan.is_some() {
        return Err("an import is running".into());
    }
    let result = if id == "library.inspectLightroom" {
        app.lightroom.start_inspect(&app.session, &path, wake(ctx), &words)
    } else {
        let update = p.get("updateExisting").and_then(Value::as_bool).unwrap_or(false);
        app.lightroom.start_import(&mut app.session, &path, update, wake(ctx), &words)
    }?;
    if wait {
        return wait_for(app, ctx, std::time::Duration::from_secs(600));
    }
    Ok(result)
}

/// Poll owner-thread work, commit a prepared import, and finish its archive index.
pub fn tick(app: &mut LightcraftApp, ctx: &egui::Context) {
    if !app.lightroom.running() {
        return;
    }
    if let Some(done) = app.lightroom.poll(&mut app.session, wake(ctx), &words) {
        announce(app, ctx, done);
    }
    ctx.request_repaint();
}

/// Wait for a task in control/headless mode while still applying owner-thread completion.
pub fn wait_for(app: &mut LightcraftApp, ctx: &egui::Context, timeout: std::time::Duration) -> Result<Value, String> {
    let start = web_time::Instant::now();
    while app.lightroom.running() {
        tick(app, ctx);
        if start.elapsed() >= timeout {
            return Err("timed out waiting for Lightroom task".into());
        }
        #[cfg(not(target_arch = "wasm32"))]
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    Ok(app.lightroom.last.clone().unwrap_or(Value::Null))
}
