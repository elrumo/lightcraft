use super::scan::{busy_synchronizing, running};
use super::*;

/// A running import (see the module docs). The file-system work — expanding folders, probing,
/// reading sidecars, copying or placing files — runs on a worker thread
/// ([`lightcraft_engine::import::ImportJob`]); each batch it readies is added to the catalog on the
/// UI thread between frames, so a slow drive never stalls the window.
#[derive(Debug, Default)]
pub struct ImportTask {
    /// Files and folders to import (handed to the worker when the task starts).
    queue: Vec<String>,
    pub total: usize,
    pub done: usize,
    params: Value,
    /// What the batches added so far (shared with the engine host).
    pub tally: lightcraft_apphost::import::Tally,
    undo0: usize,
    /// Reading a folder for the Local view: the photos stay out of the library, and nothing is
    /// selected or announced as added.
    pub(super) browse: bool,
    /// Cancel was pressed: no further files are started; batches already readied are added.
    pub cancelled: bool,
    /// Auto Import (the watched folder): the selection stays as it is, a short toast when done.
    auto: bool,
    /// Auto Import: the selection to keep.
    keep_selection: Option<lightcraft_engine::Selection>,
    /// The selection and source as the import last left them: a change between frames is the
    /// user choosing another photo or view, which the end of the import doesn't take away.
    left: Option<(lightcraft_engine::Selection, lightcraft_engine::LibrarySource)>,
    /// The user chose another photo or view while the import ran.
    moved_on: bool,
    run: Option<lightcraft_apphost::import::Run>,
    /// The import's row in the activity stack (its ✕ and `activity.cancel` set the run's cancel flag).
    guard: Option<lightcraft_engine::activity::TaskGuard>,
    /// Photos dropped on the window: the grid holds a tile for each until it is in the catalog
    /// ([`crate::file_drop`]).
    pending: Option<crate::file_drop::Pendings>,
    /// A staging folder of the app's own the files were copied into (off a camera): removed
    /// when the import ends (what is left there are duplicates and failures, still on the camera).
    pub cleanup: Option<std::path::PathBuf>,
}

impl ImportTask {
    /// An import of `paths` (files or folders) with `library.import` params (without `paths`).
    pub fn new(paths: Vec<String>, params: Value, undo0: usize, browse: bool) -> Self {
        ImportTask { total: paths.len(), queue: paths, params, undo0, browse, ..Default::default() }
    }

    /// An Auto Import (see [`ImportTask::auto`]).
    pub fn auto(mut self) -> Self {
        self.auto = true;
        self
    }

    /// `{done, total, imported, cancelled, pending}` for `ui.inspect`.
    pub fn status(&self) -> Value {
        let pending = self.pending.as_ref().map_or(0, |p| p.len());
        json!({"done": self.done, "total": self.total, "imported": self.tally.imported, "cancelled": self.cancelled, "pending": pending})
    }

    /// The dropped photos not in the catalog yet (none for an import that wasn't a drop).
    pub fn pending(&self) -> Option<&crate::file_drop::Pendings> {
        self.pending.as_ref()
    }

    /// The album the photos are added to.
    pub fn album(&self) -> Option<u64> {
        self.params["album"].as_u64()
    }
}

/// Tells the grid's tiles for dropped photos how the import's worker goes, and wakes the window
/// when a batch is ready.
struct DropWatch {
    pending: Option<crate::file_drop::Pendings>,
    ctx: egui::Context,
}

impl lightcraft_apphost::import::Watch for DropWatch {
    fn listed(&self, files: &[String]) {
        // (the files of dropped folders are known now)
        if let Some(p) = &self.pending {
            p.set(files);
            self.ctx.request_repaint();
        }
    }

    fn readying(&self, n: usize) {
        if let Some(p) = &self.pending {
            p.start(n);
        }
    }

    fn ready(&self) {
        self.ctx.request_repaint();
    }
}

/// Start importing the dialog's checked files (the dialog's OK / `ui.dialog.confirm`); on a camera,
/// copying them off it first ([`crate::camera_import::start`]). Refused while another import runs:
/// replacing `app.import` would stop its worker.
pub fn start(app: &mut LightcraftApp, d: &ImportDialog) -> Result<Value, String> {
    if d.camera.is_some() {
        return crate::camera_import::start(app, d);
    }
    busy_synchronizing(app)?;
    if !d.in_library(app) {
        return Err("another library was opened since the photos were scanned: nothing was imported".into());
    }
    // a browse scan that finishes starts its own import, replacing this one
    if app.import.is_some() || app.scan.as_ref().is_some_and(|t| t.browse) {
        return Err(crate::i18n::tr("An import is running").to_string());
    }
    let queue = d.selected_paths();
    if queue.is_empty() {
        return Err("no photos selected".into());
    }
    let undo0 = app.session.undo.len();
    let params = import_params(app, d)?;
    let total = queue.len();
    app.import = Some(ImportTask::new(queue, params, undo0, false));
    app.renderer.forget_imports();
    Ok(json!({"importing": total}))
}

/// The `library.import` params (without `paths`) the review's choices make, creating the new
/// album it names.
pub(crate) fn import_params(app: &mut LightcraftApp, d: &ImportDialog) -> Result<Value, String> {
    let mut album = d.album;
    if album.is_none() && !d.new_album.trim().is_empty() {
        let r = app.session.execute("album.create", &json!({"name": d.new_album.trim()})).map_err(|e| e.to_string())?;
        album = r["id"].as_u64();
    }
    let mode = match (d.copy, d.move_files) {
        _ if d.staged => "move",
        (false, _) => "add",
        (true, false) => "copy",
        (true, true) => "move",
    };
    let mut params = json!({"mode": mode, "keywords": d.keywords()});
    if !d.on_deleted.is_empty() {
        params["onDeleted"] = json!(d.on_deleted);
    }
    if let Some(a) = album {
        params["album"] = json!(a);
    }
    if !d.preset.is_empty() {
        params["preset"] = json!(d.preset);
    }
    if !d.metadata_preset.is_empty() {
        params["metadataPreset"] = json!(d.metadata_preset);
    }
    if d.copy || d.staged {
        if !d.destination.trim().is_empty() {
            params["destination"] = json!(d.destination.trim());
        }
        if let Some(o) = d.organize_param()? {
            params["organize"] = json!(o);
        }
        if !d.rename.trim().is_empty() {
            params["rename"] = json!(d.rename.trim());
            params["renameStart"] = json!(1);
        }
        if d.dng && !d.move_files {
            params["dng"] = json!(true);
        }
    }
    Ok(params)
}

/// Start importing `paths` (files or folders) in the background.
pub fn start_paths(app: &mut LightcraftApp, paths: Vec<String>) -> Result<Value, String> {
    start_with(app, paths, None, false)
}

/// Start importing `paths` dropped on the window, into `album` when one is given. The grid shows a
/// tile for each photo from the moment of the drop until it is in the catalog.
pub fn start_dropped(app: &mut LightcraftApp, paths: Vec<String>, album: Option<u64>) -> Result<Value, String> {
    start_with(app, paths, album, true)
}

fn start_with(app: &mut LightcraftApp, paths: Vec<String>, album: Option<u64>, tiles: bool) -> Result<Value, String> {
    busy_synchronizing(app)?;
    if running(app) {
        return Err(crate::i18n::tr("An import is running").to_string());
    }
    let undo0 = app.session.undo.len();
    let total = paths.len();
    let mut params = json!({"mode": "add"});
    if let Some(a) = album {
        params["album"] = json!(a);
    }
    let mut task = ImportTask::new(paths, params, undo0, false);
    if tiles {
        // the files named are known at once; a dropped folder's are listed by the worker once it has read it
        let pending = crate::file_drop::Pendings::default();
        pending.set(task.queue.iter().filter(|p| !std::path::Path::new(p).is_dir()));
        task.pending = Some(pending);
    }
    app.import = Some(task);
    Ok(json!({"importing": total}))
}

/// Advance the import in progress (called every frame): start its worker, then add the batches it
/// has readied to the catalog.
pub fn tick(app: &mut LightcraftApp, ctx: &egui::Context) {
    let Some(mut task) = app.import.take() else { return };
    if task.run.is_none() {
        let watch: std::sync::Arc<dyn lightcraft_apphost::import::Watch> =
            std::sync::Arc::new(DropWatch { pending: task.pending.clone(), ctx: ctx.clone() });
        let started = lightcraft_apphost::import::Run::begin(&mut app.session, std::mem::take(&mut task.queue), &task.params, Some(watch));
        if task.auto {
            task.keep_selection = Some(app.session.selection.clone());
        }
        match started {
            Ok(run) => {
                let label = if task.browse { "Reading folder" } else { "Importing" };
                task.guard = Some(app.session.activity.start("import", label, lightcraft_engine::activity::Cancel::Flag(run.cancel.clone())));
                task.run = Some(run);
            }
            Err(e) => {
                log::warn!("import: {e}");
                app.toast(ctx, crate::i18n::tr_format!("Import failed: {e}", e = e));
                return;
            }
        }
    }
    ctx.request_repaint_after(std::time::Duration::from_millis(100));
    let Some(run) = task.run.as_mut() else { return };
    let batches = run.next_batches();
    let finished = batches.is_none();
    if task.left.as_ref().is_some_and(|(sel, source)| *sel != app.session.selection || *source != app.session.source) {
        task.moved_on = true;
    }
    for prepared in batches.into_iter().flatten() {
        commit_batch(app, &mut task, prepared);
        // arriving photos don't take over the selection
        if let Some(sel) = &task.keep_selection {
            app.session.selection = sel.clone();
        }
        // a Local folder still on screen: its first photo becomes active once there is one (#1447)
        if task.browse && app.session.source == lightcraft_engine::LibrarySource::Folder {
            app.session.keep_active_in_view();
        }
    }
    task.left = Some((app.session.selection.clone(), app.session.source));
    if let Some(run) = &task.run {
        task.total = run.total.load(Ordering::Relaxed).max(task.total);
        task.done = run.done.load(Ordering::Relaxed);
    }
    if let Some(guard) = &task.guard {
        guard.progress(task.done as u64, task.total as u64);
        // ✕ in the stack (or `activity.cancel`): no further files are started
        if guard.is_cancelled() {
            task.cancelled = true;
        }
    }
    if !finished {
        app.import = Some(task);
        return;
    }
    finish(app, ctx, task);
}

/// Add one readied batch to the catalog (an undo step merged into the import's at the end).
fn commit_batch(app: &mut LightcraftApp, task: &mut ImportTask, prepared: lightcraft_engine::import::Prepared) {
    let Some(run) = task.run.as_mut() else { return };
    let n = prepared.len();
    let r = lightcraft_apphost::import::commit(&mut app.session, run, prepared);
    // the batch's photos are in the catalog (or not coming): their tiles give way in the same frame
    if let Some(p) = &task.pending {
        p.added(n);
    }
    match r {
        Ok(v) => task.tally.add(&v),
        Err(e) => {
            log::warn!("import: {e}");
            task.tally.failed += n;
        }
    }
}

fn xmp_warning_text(warning: &ImportWarning) -> String {
    let path = &warning.path;
    match warning.code {
        ImportWarningCode::CameraProfile => crate::i18n::tr_format!(
            "XMP profile \"{profile}\" was not applied for {path}; colors may differ from Lightroom.",
            profile = warning.profile.as_deref().unwrap_or_default(),
            path = path,
        ),
        ImportWarningCode::UprightGeometry => match warning.mode.unwrap_or(ImportUprightMode::Unknown) {
            ImportUprightMode::Off => crate::i18n::tr_format!(
                "Lightroom geometry for {path} was not preserved; saved transform is ignored because Upright is Off.",
                path = path,
            ),
            ImportUprightMode::Guided => crate::i18n::tr_format!(
                "Lightroom geometry for {path} was not preserved; Guided Upright saved transform is unsupported.",
                path = path,
            ),
            ImportUprightMode::Auto => crate::i18n::tr_format!(
                "Lightroom geometry was not preserved for {path}; {mode} will be recalculated.",
                path = path,
                mode = crate::i18n::tr("Auto")
            ),
            ImportUprightMode::Level => crate::i18n::tr_format!(
                "Lightroom geometry was not preserved for {path}; {mode} will be recalculated.",
                path = path,
                mode = crate::i18n::tr("Level")
            ),
            ImportUprightMode::Vertical => crate::i18n::tr_format!(
                "Lightroom geometry was not preserved for {path}; {mode} will be recalculated.",
                path = path,
                mode = crate::i18n::tr("Vertical")
            ),
            ImportUprightMode::Full => crate::i18n::tr_format!(
                "Lightroom geometry was not preserved for {path}; {mode} will be recalculated.",
                path = path,
                mode = crate::i18n::tr("Full")
            ),
            ImportUprightMode::Unknown => {
                crate::i18n::tr_format!("Lightroom geometry for {path} was not preserved; saved Upright transform is unsupported.", path = path,)
            }
        },
        ImportWarningCode::UnmappedXmp => {
            crate::i18n::tr_format!("XMP adjustments were not imported for {path}: {fields}.", path = path, fields = warning.fields.join(", "),)
        }
    }
}

fn xmp_warnings_text(warnings: &[ImportWarning]) -> String {
    crate::i18n::tr_format!(
        "Import warnings ({n}):\n{warnings}",
        n = warnings.len(),
        warnings = warnings.iter().map(xmp_warning_text).collect::<Vec<_>>().join("\n"),
    )
}

/// The import is done (or cancelled): one undo step, show and select the first photo (unless the
/// user chose another meanwhile), say what happened.
fn finish(app: &mut LightcraftApp, ctx: &egui::Context, task: ImportTask) {
    if let Some(dir) = &task.cleanup
        && let Err(e) = std::fs::remove_dir_all(dir)
    {
        log::warn!("{}: {e}", dir.display());
    }
    let steps = app.session.undo.len().saturating_sub(task.undo0);
    let plural = |n: usize| if n == 1 { "" } else { "s" };
    let label = if task.tally.imported == 0 && task.tally.restored > 0 {
        crate::i18n::tr_format!("Restore {} Photo{}", task.tally.restored, plural(task.tally.restored))
    } else if task.tally.restored > 0 {
        crate::i18n::tr_format!("Add {} Photo{}, restore {}", task.tally.imported, plural(task.tally.imported), task.tally.restored)
    } else {
        crate::i18n::tr_format!("Add {} Photo{}", task.tally.imported, if task.tally.imported == 1 { "" } else { "s" })
    };
    app.session.merge_undo(steps, &label);
    if task.auto {
        if !task.tally.warnings.is_empty() {
            app.toast_for(ctx, xmp_warnings_text(&task.tally.warnings), 8.0);
        } else if task.tally.imported > 0 {
            app.toast(
                ctx,
                crate::i18n::tr_format!("Auto Import: added {} photo{}", task.tally.imported, if task.tally.imported == 1 { "" } else { "s" }),
            );
        }
        return;
    }
    if task.browse {
        if !task.tally.warnings.is_empty() {
            app.toast_for(ctx, xmp_warnings_text(&task.tally.warnings), 8.0);
        } else if task.tally.failed > 0 {
            app.toast(ctx, crate::i18n::tr_format!("{} photo{} not readable", task.tally.failed, if task.tally.failed == 1 { "" } else { "s" }));
        }
        return;
    }
    if let Some(f) = task.tally.first.filter(|_| !task.moved_on) {
        // a view the new photos aren't in (an album, Picks, a Local folder) would leave the active
        // photo off screen: show All Photos, as Lightroom shows the import's own collection (#1446)
        if !app.session.visible().contains(&lightcraft_catalog::PhotoId(f)) {
            let _ = app.run("library.source", json!({"kind": "all"}));
        }
        let _ = app.run("library.select", json!({"ids": [f]}));
    }
    let plural = |n: usize| if n == 1 { "" } else { "s" };
    // nothing new, only files the library already has: show where they are (Recently Deleted is
    // easy to miss, and the side panel that lists it starts collapsed)
    if task.tally.imported == 0
        && task.tally.restored == 0
        && task.tally.failed == 0
        && task.tally.warnings.is_empty()
        && !task.cancelled
        && show_existing(app, ctx, &task.tally.existing)
    {
        return;
    }
    let mut msg = if task.cancelled {
        crate::i18n::tr_format!("Import cancelled · {} photo{} added", task.tally.imported, plural(task.tally.imported))
    } else if task.tally.imported == 0 && task.tally.restored > 0 {
        crate::i18n::tr_format!("Restored {} photo{} from Recently Deleted", task.tally.restored, plural(task.tally.restored))
    } else {
        crate::i18n::tr_format!("Imported {} photo{}", task.tally.imported, plural(task.tally.imported))
    };
    if task.tally.restored > 0 && (task.tally.imported > 0 || task.cancelled) {
        msg.push_str(&crate::i18n::tr_format!(" · {} restored from Recently Deleted", task.tally.restored));
    }
    // (dropped on an album you are not looking at: this is what says where the photos went)
    if let Some(name) = task.album().and_then(|a| app.session.catalog.album(lightcraft_catalog::AlbumId(a))).map(|a| a.name.clone()) {
        msg.push_str(&format!(" · {} “{name}”", crate::i18n::tr("in album")));
    }
    if task.params["mode"] == "move" {
        msg.push_str(&crate::i18n::tr_format!(" · {} moved", task.tally.moved));
        if task.tally.kept > 0 {
            msg.push_str(&crate::i18n::tr_format!(" · {} original{} left at the source", task.tally.kept, plural(task.tally.kept)));
        }
    }
    if task.tally.duplicates > 0 {
        msg.push_str(&crate::i18n::tr_format!(" · {} duplicate{} skipped", task.tally.duplicates, if task.tally.duplicates == 1 { "" } else { "s" }));
        let deleted = deleted_of(app, &task.tally.existing).len();
        if deleted > 0 {
            msg.push_str(&crate::i18n::tr_format!(" ({} in Recently Deleted)", deleted));
        }
    }
    if task.tally.failed > 0 {
        msg.push_str(&crate::i18n::tr_format!(" · {} not readable", task.tally.failed));
    }
    if task.tally.warnings.is_empty() {
        app.toast(ctx, msg);
    } else {
        msg.push_str("\n\n");
        msg.push_str(&xmp_warnings_text(&task.tally.warnings));
        app.toast_for(ctx, msg, 8.0);
    }
}

/// The photos among `ids` that are in Recently Deleted (each once).
fn deleted_of(app: &LightcraftApp, ids: &[u64]) -> Vec<u64> {
    let mut v: Vec<u64> =
        ids.iter().copied().filter(|&id| app.session.catalog.photo(lightcraft_catalog::PhotoId(id)).is_some_and(|p| p.deleted)).collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// Select the library photos an import skipped as duplicates of, in Recently Deleted (side panel
/// opened, with how to get them back) or in All Photos, and say so. `false` when there are none.
fn show_existing(app: &mut LightcraftApp, ctx: &egui::Context, existing: &[u64]) -> bool {
    let deleted = deleted_of(app, existing);
    let (kind, mut ids) = if deleted.is_empty() { ("all", existing.to_vec()) } else { ("recentlyDeleted", deleted) };
    ids.sort_unstable();
    ids.dedup();
    ids.retain(|&id| app.session.catalog.photo(lightcraft_catalog::PhotoId(id)).is_some());
    if ids.is_empty() || app.run("library.source", json!({"kind": kind})).is_err() {
        return false;
    }
    let _ = app.run("library.select", json!({"ids": ids}));
    app.ui.left_panel = true;
    let n = ids.len();
    let plural = if n == 1 { "" } else { "s" };
    let msg = if kind == "all" {
        crate::i18n::tr_format!("Already in the library: {} photo{} selected in All Photos", n, plural)
    } else {
        crate::i18n::tr_format!(
            "Already in the library, in Recently Deleted: {} photo{} selected · right-click ▸ Restore, or Delete Permanently to import afresh",
            n,
            plural
        )
    };
    app.toast_for(ctx, msg, 6.0);
    true
}

/// The phone layout's progress window while an import runs, with Cancel (files already copied or
/// added stay; nothing new is started). The desktop shows the import's row in the activity stack.
pub fn progress(app: &mut LightcraftApp, ctx: &egui::Context) {
    let Some(task) = &app.import else { return };
    let t = Tokens::get(ctx);
    let frac = task.done as f32 / task.total.max(1) as f32;
    let text = if task.cancelled {
        crate::i18n::tr("Stopping…").to_string()
    } else if task.browse {
        crate::i18n::tr_format!("Reading photos… {} of {}", task.done, task.total)
    } else {
        crate::i18n::tr_format!("Adding photos… {} of {}", task.done, task.total)
    };
    let mut cancel = false;
    egui::Window::new(crate::i18n::tr("Importing"))
        .title_bar(false)
        .resizable(false)
        .anchor(Align2::CENTER_BOTTOM, [0.0, -80.0])
        .fixed_size([340.0, 80.0])
        .show(ctx, |ui| {
            ui.label(egui::RichText::new(text).color(t.text));
            ui.add(egui::ProgressBar::new(frac).desired_width(320.0));
            let r = ui.add_enabled(!task.cancelled, egui::Button::new(crate::i18n::tr("Cancel")));
            crate::widgets::register_geometry(ctx, "button:importCancel", r.rect);
            cancel = r.clicked();
        });
    if cancel && let Some(task) = app.import.as_mut() {
        task.cancelled = true;
        if let Some(run) = &task.run {
            run.cancel.store(true, Ordering::Relaxed);
        }
    }
}
