//! Background Lightroom catalog inspection and import: egui's side of
//! [`lightcraft_apphost::lightroom`] (the runner both apps share). This file starts tasks from
//! `library.inspectLightroom` / `library.importLightroom`, polls them every frame and words the
//! outcome in a toast (an import's end also stays readable in a dialog).

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
    let (kind, mut value) = (done.kind, done.value);
    if let Some(o) = value.as_object_mut()
        && let Some(library) = app.session.library.as_ref().filter(|l| l.on_disk)
    {
        o.entry("library").or_insert_with(|| serde_json::json!(library.dir.to_string_lossy()));
    }
    let message = if let Some(e) = value.get("error").and_then(Value::as_str) {
        let warning = value.get("indexWarning").and_then(Value::as_str).map_or(String::new(), |w| format!("; index warning: {w}"));
        if kind == Kind::Inspect {
            crate::i18n::tr_format!("Lightroom inspection failed: {e}{warning}", e = e, warning = warning)
        } else {
            crate::i18n::tr_format!("Lightroom import failed: {e}{warning}", e = e, warning = warning)
        }
    } else if value.get("cancelled").and_then(Value::as_bool).unwrap_or(false) {
        crate::i18n::tr(if kind == Kind::Inspect { "Lightroom inspection cancelled" } else { "Lightroom import cancelled" }).into()
    } else if kind == Kind::Inspect && value["kind"] == "cloud" {
        crate::i18n::tr_format!(
            "Lightroom catalog inspected: {albums} albums, {folders} folders, {photos} photos in albums",
            albums = value["albums"].as_u64().unwrap_or(0),
            folders = value["folders"].as_u64().unwrap_or(0),
            photos = value["photos"].as_u64().unwrap_or(0)
        )
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
    } else if value.get("report").unwrap_or(&value)["albumsOnly"] == true {
        let report = value.get("report").unwrap_or(&value);
        crate::i18n::tr_format!(
            "Lightroom albums imported: {albums} albums and {folders} folders made, {added} photos put in albums",
            albums = report["albums"].as_u64().unwrap_or(0),
            folders = report["folders"].as_u64().unwrap_or(0),
            added = report["added"].as_u64().unwrap_or(0)
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
    if kind == Kind::Import && value.get("cancelled") != Some(&Value::Bool(true)) {
        app.lightroom_result = Some(result_dialog(&value, &message));
        show_result(app);
    }
    // (the runner kept the result for the status response; this adds the destination library)
    app.lightroom.last = Some(value);
    app.toast_for(ctx, message, 6.0);
}

/// Open the import's result dialog once no other dialog is open.
fn show_result(app: &mut LightcraftApp) {
    if app.ui.dialog.is_none() {
        app.ui.dialog = app.lightroom_result.take();
    }
}

/// The dialog that keeps an import's outcome readable after its toast is gone: counts, failures,
/// missing originals, the skipped images (one line, in the UI language) and at most eight warnings.
fn result_dialog(value: &Value, summary: &str) -> crate::state::Dialog {
    let report = value.get("report").unwrap_or(value);
    let mut details = Vec::new();
    if let Some(photos) = report["photos"].as_u64() {
        details.push(crate::i18n::tr_format!(
            "{photos} catalog records processed; {collections} collections.",
            photos = photos,
            collections = report["collections"].as_u64().unwrap_or(0)
        ));
    }
    if report["albumsOnly"] == true {
        let count = |k: &str| report[k].as_u64().unwrap_or(0);
        details.push(crate::i18n::tr_format!(
            "{matched} of the albums' photos are in this library; {unmatched} are not, and were left out.",
            matched = count("matched"),
            unmatched = count("unmatched")
        ));
        if count("ambiguous") > 0 {
            details
                .push(crate::i18n::tr_format!("{ambiguous} photos fit more than one photo here, and were left out.", ambiguous = count("ambiguous")));
        }
        if count("albumsUpdated") > 0 {
            details.push(crate::i18n::tr_format!("{updated} albums that were already here got photos.", updated = count("albumsUpdated")));
        }
    }
    if let Some(failures) = report["failed"].as_array() {
        details
            .extend(failures.iter().take(4).map(|failure| format!("{}: {}", failure[0].as_str().unwrap_or(""), failure[1].as_str().unwrap_or(""))));
    }
    let missing = report["missing"].as_array().map_or(0, Vec::len);
    if missing > 0 {
        details.push(crate::i18n::tr_format!(
            "{missing} originals are missing. Their catalog records are available; use Find Missing Photos to relink them.",
            missing = missing
        ));
    }
    // images whose catalog records are incomplete come as data (`skipped`), said here in the UI
    // language; their ids are in the report for the CLI and agents
    let skipped = report["skipped"].as_array().map_or(0, Vec::len);
    if skipped > 0 {
        details.push(crate::i18n::tr_format!(
            "{skipped} catalog images were not imported because their records in the catalog are incomplete.",
            skipped = skipped
        ));
    }
    // images whose catalog records are incomplete come as data (`skipped`), said here in the UI
    // language; their ids are in the report for the CLI and agents
    let skipped = report["skipped"].as_array().map_or(0, Vec::len);
    if skipped > 0 {
        details.push(crate::i18n::tr_format!(
            "{skipped} catalog images were not imported because their records in the catalog are incomplete.",
            skipped = skipped
        ));
    }
    if let Some(warning) = value.get("indexWarning").and_then(Value::as_str) {
        details.push(warning.to_string());
    }
    if let Some(warnings) = report["warnings"].as_array() {
        // Results can contain one warning per image; keep the dialog bounded.
        details.extend(warnings.iter().take(8).filter_map(Value::as_str).map(str::to_string));
        if warnings.len() > 8 {
            details.push(crate::i18n::tr_format!("{count} more warnings.", count = warnings.len() - 8));
        }
    }
    let first = value.get("error").is_none().then(|| report["mapping"].as_object()?.values().filter_map(Value::as_u64).min()).flatten();
    crate::state::Dialog::LightroomResult {
        summary: summary.to_string(),
        library: report["library"].as_str().or_else(|| value["library"].as_str()).map(str::to_string),
        details,
        first,
    }
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
        let albums_only = p.get("albumsOnly").and_then(Value::as_bool).unwrap_or(false);
        app.lightroom.start_import(&mut app.session, &path, update, albums_only, wake(ctx), &words)
    }?;
    if wait {
        return wait_for(app, ctx, std::time::Duration::from_secs(600));
    }
    Ok(result)
}

/// Poll owner-thread work, commit a prepared import, and finish its archive index.
pub fn tick(app: &mut LightcraftApp, ctx: &egui::Context) {
    show_result(app);
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn demo() -> crate::headless::Headless {
        let app = crate::LightcraftApp::new(lightcraft_engine::Session::with_demo(), crate::Services { png: None, ..Default::default() });
        let mut h = crate::headless::Headless::new(app, [1200.0, 800.0], 1.0);
        // no render left running at exit (see tests_activity::demo)
        h.settle(std::time::Duration::from_secs(20));
        h
    }

    /// An import that ended with `value`, as the runner hands it over.
    fn finished(h: &mut crate::headless::Headless, value: Value) {
        announce(&mut h.app, &h.view.ctx, Finished { kind: Kind::Import, value });
    }

    #[test]
    fn lightroom_result_stays_readable_and_shows_photos_hidden_by_a_filter() {
        let mut h = demo();
        let id = h.app.session.catalog.photos().next().unwrap().id.0;
        h.app.session.source = lightcraft_engine::LibrarySource::Picks;
        h.app.session.filter.text = "no-photo-has-this-name".into();
        h.app.session.filter.only = vec![lightcraft_catalog::PhotoId(u64::MAX)];
        h.app.session.filter.labels = vec![lightcraft_catalog::ColorLabel::Red];
        h.app.session.filter.rule_set = Some(lightcraft_catalog::RuleSet {
            mode: lightcraft_catalog::rules::Match::All,
            rules: vec![lightcraft_catalog::rules::Rule::Field { field: "rating".into(), op: "is".into(), value: json!(5) }],
        });
        let value = json!({"photos": 1, "imported": 1, "collections": 0, "missing": ["D:/Photos/a.NEF"], "mapping": {"1": id}, "library": "C:/Pictures/LightCraft Library"});
        finished(&mut h, value);
        assert!(h.app.ui.toast.as_ref().unwrap().0.contains("1 photos added"));
        h.step();
        h.step();
        let text = h.painted_text().join(" ");
        assert!(text.contains("Destination library"), "{text}");
        assert!(text.contains("C:/Pictures/LightCraft Library"), "{text}");
        assert!(text.contains("1 originals are missing"), "{text}");
        assert!(text.contains("Show in Library"), "{text}");
        // The activity and toast can go away; the modal result remains until dismissed.
        h.app.ui.toast = None;
        h.step();
        assert!(h.app.ui.dialog.is_some());
        let dialog = h.app.ui.dialog.clone().unwrap();
        crate::panels::dialogs::confirm_dialog(&mut h.app, &dialog).unwrap();
        assert_eq!(h.app.session.source, lightcraft_engine::LibrarySource::All);
        assert_eq!(h.app.session.filter, lightcraft_catalog::Filter::default());
        assert_eq!(h.app.session.selection.active.map(|p| p.0), Some(id));
        assert_eq!(h.app.ui.view, crate::state::ViewMode::PhotoGrid);
        assert!(!h.app.session.visible().is_empty());
    }

    #[test]
    fn lightroom_failure_stays_readable_without_offering_a_success_action() {
        let mut h = demo();
        finished(&mut h, json!({"error": "catalog or WAL exceeds 1 GiB import limit"}));
        h.step();
        h.step();
        assert!(h.painted_text().join(" ").contains("exceeds 1 GiB import limit"));
        assert!(matches!(h.app.ui.dialog, Some(crate::state::Dialog::LightroomResult { first: None, .. })));
        assert!(!h.app.widgets.iter().any(|(id, _)| id == "button:dialogCancel"));
    }

    #[test]
    fn lightroom_result_waits_for_an_existing_dialog() {
        let mut h = demo();
        h.app.ui.dialog = Some(crate::state::Dialog::About);
        finished(&mut h, json!({"error": "catalog could not be read"}));
        assert!(matches!(h.app.ui.dialog, Some(crate::state::Dialog::About)));
        assert!(h.app.lightroom_result.is_some());
        h.app.ui.dialog = None;
        tick(&mut h.app, &h.view.ctx);
        assert!(matches!(h.app.ui.dialog, Some(crate::state::Dialog::LightroomResult { .. })));
        assert!(h.app.lightroom_result.is_none());
    }

    #[test]
    fn skipped_images_are_one_line_in_the_ui_language() {
        let value = json!({"report": {"photos": 3, "collections": 0, "warnings": ["w1", "w2"], "skipped": [
            {"image": 7, "reason": "missingFile"},
            {"image": 8, "reason": "missingRoot"},
            {"image": 9, "reason": "masterSkipped"},
        ]}});
        let details = |locale| {
            crate::i18n::set_language(locale);
            let dialog = result_dialog(&value, "done");
            crate::i18n::set_language(crate::i18n::Locale::En);
            let crate::state::Dialog::LightroomResult { details, .. } = dialog else { panic!("not a Lightroom result") };
            details
        };
        let en = details(crate::i18n::Locale::En);
        assert_eq!(en.len(), 4, "{en:?}");
        assert_eq!(en[1], "3 catalog images were not imported because their records in the catalog are incomplete.");
        assert_eq!(&en[2..], ["w1", "w2"]);
        let es = details(crate::i18n::Locale::Es);
        assert_eq!(es[1], "No se importaron 3 imágenes del catálogo porque sus registros en el catálogo están incompletos.");
    }

    #[test]
    fn skipped_images_do_not_take_the_warning_slots() {
        let warnings: Vec<_> = (0..10).map(|i| format!("w{i}")).collect();
        let skipped: Vec<_> = (0..500).map(|i| json!({"image": i, "reason": "missingFile"})).collect();
        let value = json!({"report": {"warnings": warnings, "skipped": skipped}});
        let crate::state::Dialog::LightroomResult { details, .. } = result_dialog(&value, "done") else { panic!("not a Lightroom result") };
        // one line for the skipped images, then the usual eight warnings and the rest counted
        assert_eq!(details.len(), 10, "{details:?}");
        assert!(details[0].starts_with("500 catalog images were not imported"));
        assert_eq!(details[9], "2 more warnings.");
    }
}
