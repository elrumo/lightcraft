//! Modal dialogs (new album, rename, create preset, choose settings to copy, export, about, shortcuts).

use lightcraft_develop::{ControlSpec, SettingsGroup};
use serde_json::json;

use crate::LightcraftApp;
use crate::state::Dialog;
use crate::theme::Tokens;

/// Rows the Rename dialog previews.
const RENAME_PREVIEW_ROWS: usize = 6;

/// The Rename dialog's preview: its first rows (`None` while being planned) and how many files
/// are renamed. Which names are taken is checked on disk, which can block on a slow drive, so the
/// rows are planned on a worker thread, and only when the template, start number, photos or
/// catalog change — never once per frame.
pub(crate) fn rename_preview(
    app: &mut LightcraftApp,
    ctx: &egui::Context,
    template: &str,
    start: usize,
) -> (Option<Vec<lightcraft_engine::rename::RenamePlan>>, usize) {
    use std::hash::{Hash, Hasher};
    type Rows = std::sync::Arc<std::sync::Mutex<Option<Vec<lightcraft_engine::rename::RenamePlan>>>>;
    let ids = app.session.targets(&json!({}));
    let photos = lightcraft_engine::rename::rename_photos(&app.session.catalog, &ids);
    let total = photos.len();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (template, start, app.session.catalog.revision, &ids).hash(&mut h);
    let key = h.finish();
    let id = egui::Id::new("rename-preview-rows");
    let read = |rows: &Rows| rows.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
    if let Some((k, rows)) = ctx.data(|d| d.get_temp::<(u64, Rows)>(id))
        && k == key
    {
        return (read(&rows), total);
    }
    let rows: Rows = Default::default();
    let first: Vec<_> = photos.into_iter().take(RENAME_PREVIEW_ROWS).collect();
    let (out, template, repaint) = (rows.clone(), template.to_string(), ctx.clone());
    let exists = app.session.media.availability.probe();
    let work = move || {
        let plans = lightcraft_engine::rename::plan_rename_photos(&first, &template, start, &|f| exists(f));
        *out.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(plans);
        repaint.request_repaint();
    };
    #[cfg(not(target_arch = "wasm32"))]
    if let Err(e) = std::thread::Builder::new().name("lc-rename-preview".into()).spawn(work) {
        log::warn!("rename preview: {e}");
    }
    #[cfg(target_arch = "wasm32")]
    work();
    ctx.data_mut(|d| d.insert_temp(id, (key, rows.clone())));
    (read(&rows), total)
}

/// Where the SAM 3 download stands, which decides the dialog's buttons: `(installed, running,
/// failed, nowhere)`, "nowhere" being a build with no download location (only the manual install).
fn sam_state(app: &LightcraftApp) -> (bool, bool, bool, bool) {
    let sam = &app.session.segmenter;
    let (installed, running, failed) = (sam.installed(), sam.download_status().running, sam.download_status().error.is_some());
    (installed, running, failed, sam_by_hand(sam))
}

/// The dialog's dismiss button (desktop) or its page's left bar button (phone).
fn cancel_label(app: &LightcraftApp, dlg: &Dialog, informational: bool) -> &'static str {
    let (installed, running, _, nowhere) = sam_state(app);
    match dlg {
        Dialog::SamModel { .. } if running || installed || nowhere => "Close",
        Dialog::SamModel { .. } => "Not Now",
        Dialog::SuperRes { .. } => crate::superres::cancel_label(app),
        // (the choices are in the body: this one puts it off)
        Dialog::SyncChoice => "Decide Later",
        Dialog::PresetImport { review } => crate::preset_import::cancel_label(review),
        _ if informational => "Done",
        _ => "Cancel",
    }
}

/// The dialog's action as its button (desktop) or its page's bar (phone) says it; empty when
/// there is none.
fn ok_label(app: &LightcraftApp, dlg: &Dialog, informational: bool) -> String {
    let compact = app.compact;
    let (installed, running, failed, nowhere) = sam_state(app);
    let label = match dlg {
        Dialog::PresetImport { review } => return crate::preset_import::ok_label(review, compact),
        Dialog::Import { opts } if compact => return crate::i18n::tr_format!("Add {n}", n = opts.selected_paths().len()),
        Dialog::Import { opts } => {
            let n = opts.selected_paths().len();
            let verb = if opts.copy && opts.move_files { "Move" } else { "Import" };
            return crate::i18n::tr_format!("{verb} {n} Photo{}", if n == 1 { "" } else { "s" }, verb = crate::i18n::tr(verb), n = n);
        }
        Dialog::ContactSheet { .. } => "Choose PDF…",
        Dialog::Merge { .. } => "Merge",
        Dialog::DenoiseModel { info, .. } if info["download"].is_string() => "Accept & Download",
        Dialog::DenoiseModel { .. } => "Install",
        Dialog::FaceModel { info, .. } if !informational && info["download"].is_string() => "Download",
        Dialog::FaceModel { .. } if !informational => "Install",
        Dialog::ConfirmDelete { .. } | Dialog::DeleteKeyword { .. } => "Delete",
        // (not "Merge": that is photo merging, HDR / panorama, in some languages)
        Dialog::MoveKeyword { .. } => "Merge Keywords",
        Dialog::KeywordSet { .. } => "Save",
        Dialog::KeywordTag { editing: None, .. } => "Create",
        Dialog::KeywordTag { .. } => "Save",
        Dialog::RemoveFolder { .. } => "Remove",
        Dialog::EditIn { .. } => "Edit",
        Dialog::ShareLink { .. } => "Create Link",
        Dialog::LightroomCatalog { .. } => "Import Lightroom Catalog…",
        Dialog::SynchronizeFolder { .. } => "Synchronize",
        Dialog::Export { .. } => "Export",
        Dialog::SamModel { then: Some(_), .. } if installed => "Continue",
        Dialog::SamModel { .. } if installed || running || nowhere => return String::new(),
        Dialog::SamModel { error, .. } if error.is_some() || failed => "Try Again",
        Dialog::SamModel { .. } => "Download",
        Dialog::SuperRes { error } => crate::superres::ok_label(app, error.as_ref()),
        Dialog::SyncChoice => return String::new(),
        Dialog::NewAlbum { .. } | Dialog::NewSmartAlbum { .. } | Dialog::SmartRules { id: None, .. } | Dialog::CreatePreset { .. } if compact => {
            "Create"
        }
        Dialog::Rename { .. } | Dialog::RenameAlbum { .. } | Dialog::RenameKeyword { .. } if compact => "Rename",
        _ if informational => "Close",
        _ if compact => "Done",
        _ => "OK",
    };
    crate::i18n::tr(label).to_string()
}

/// About dialog tabs: (widget id suffix, label). The credits come from `crate::credits`.
pub const ABOUT_TABS: &[(&str, &str)] = &[("about", "About"), ("contributors", "Contributors"), ("models", "Models")];

/// Help ▸ What's New (docs/whats-new.md).
pub use lightcraft_engine::about::WHATS_NEW;

/// The import review and Settings keep window ids of their own, so the size each is given
/// doesn't carry over to the other dialogs.
fn window_id(dialog: &Dialog) -> egui::Id {
    egui::Id::new(match dialog {
        Dialog::Import { .. } => "lightcraft-import-dialog",
        Dialog::Settings { .. } => "lightcraft-settings-dialog",
        _ => "lightcraft-dialog",
    })
}

/// Block the underlying panels from the first frame, before the window establishes its modal
/// layer. Close background menus on entry; popups subsequently opened inside the dialog stay usable.
pub(crate) fn prepare(app: &LightcraftApp, ui: &mut egui::Ui) {
    let Some(dialog) = &app.ui.dialog else { return };
    ui.disable();
    // (a phone's dialog is a page, not a modal window: the popups it opens are its own)
    if app.compact {
        return;
    }
    let layer = egui::LayerId::new(egui::Order::Middle, window_id(dialog));
    if ui.ctx().memory(|m| m.top_modal_layer()) != Some(layer) {
        egui::Popup::close_all(ui.ctx());
    }
}

pub fn show(app: &mut LightcraftApp, ctx: &egui::Context) {
    // a phone's page slides away after the dialog closes, drawn from the dialog as it was
    if app.ui.dialog.is_some() {
        app.ui.dialog_leaving = None;
    }
    let leaving = app.ui.dialog.is_none() && app.compact;
    let Some(mut dlg) = app.ui.dialog.clone().or_else(|| app.ui.dialog_leaving.clone().filter(|_| leaving)) else {
        app.ui.dialog_leaving = None;
        super::mobile::hidden(ctx, "dialog");
        super::mobile::hidden(ctx, "dialogOptions");
        super::mobile::hidden(ctx, "dialogMore");
        super::alert::hidden(ctx, "dialog");
        return;
    };
    let at_start = dlg.clone();
    // Esc with a popup open in the dialog (a dropdown, a date picker's calendar) closes the popup
    // only: claimed before the dialog is drawn, since the popup is gone once it has handled the key
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) && egui::Popup::is_any_open(ctx) {
        crate::widgets::take_escape(ctx);
    }
    let t = Tokens::get(ctx);
    let screen = ctx.content_rect();
    // The backdrop is an area below the dialog window (a bare `Middle` layer painter would be
    // painted after every area — i.e. over the dialog too). A phone's page dims behind itself.
    if !app.compact {
        egui::Area::new(egui::Id::new("dialog-dim")).order(egui::Order::Middle).fixed_pos(screen.min).interactable(false).show(ctx, |ui| {
            ui.painter().rect_filled(screen, 0.0, egui::Color32::from_black_alpha(140));
        });
    }
    let mut close = false;
    let mut confirm = false;
    // Return presses OK in a dialog without a text field of its own (Lightroom's default button);
    // a widget that has the keyboard focus (a text field, a button or checkbox reached with Tab)
    // takes Return itself. Consumed before the dialog is drawn, so nothing else sees it too.
    if return_confirms(&dlg)
        && ctx.memory(|m| m.focused().is_none())
        && !egui::Popup::is_any_open(ctx)
        && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter))
    {
        confirm = true;
    }
    let title: String = match &dlg {
        Dialog::TextPrompt { title, .. } => title.as_str(),
        Dialog::NewAlbum { folder: true, .. } => "Create Folder",
        Dialog::NewAlbum { .. } => "Create Album",
        Dialog::RenameAlbum { .. } => "Rename Album",
        Dialog::ShareLink { .. } => "Share Link",
        Dialog::Rename { .. } => "Rename Photos",
        Dialog::Import { .. } => "Import Photos",
        Dialog::PresetImport { .. } => "Import Presets",
        Dialog::LabelNames { .. } => "Edit Color Label Names",
        Dialog::CaptureTime { .. } => "Edit Capture Time",
        Dialog::RenameKeyword { .. } => "Rename Keyword",
        Dialog::KeywordTag { editing: None, .. } => "Create Keyword Tag",
        Dialog::KeywordTag { .. } => "Edit Keyword Tag",
        Dialog::DeleteKeyword { .. } => "Delete Keyword",
        Dialog::MoveKeyword { .. } => "Merge Keywords",
        Dialog::KeywordSet { .. } => "Edit Keyword Set",
        Dialog::MergeKeywords { .. } => "Merge Keywords",
        Dialog::NewSmartAlbum { .. } => "Create Smart Album",
        Dialog::AllMetadata { .. } => "All Metadata",
        Dialog::SystemInfo { .. } => "System Info",
        Dialog::DenoiseModel { .. } => "AI Denoise Model",
        Dialog::FaceModel { info, .. } if info["download"].is_string() => "Download Face Model",
        Dialog::FaceModel { .. } => "Add Face Model",
        Dialog::WhatsNew => "What's New",
        Dialog::Cull { .. } => "Assisted Culling",
        Dialog::SmartRules { id: None, .. } => "New Smart Album",
        Dialog::SmartRules { .. } => "Edit Smart Album",
        Dialog::AutoStack { .. } => "Auto-Stack by Capture Time",
        Dialog::EditIn { photocraft: true, .. } => "Edit Photo with PhotoCraft",
        Dialog::EditIn { .. } => "Edit Photo in Other Editor",
        Dialog::CreatePreset { .. } => "Create Preset",
        Dialog::CopySettings { .. } => "Choose Edit Settings to Copy",
        Dialog::PasteSettings { .. } => "Paste Selected Settings",
        Dialog::Export { .. } => "Export",
        Dialog::ContactSheet { .. } => "Contact Sheet PDF",
        Dialog::Merge { opts } => crate::merge::title(opts),
        Dialog::Settings { .. } => "Settings",
        Dialog::ConfirmDelete { .. } => "Delete Photos",
        Dialog::LightroomCatalog { .. } => "Lightroom Classic Catalog",
        Dialog::RemoveFolder { disk: true, .. } => "Remove Disk from Library",
        Dialog::RemoveFolder { .. } => "Remove Folder from Library",
        Dialog::SynchronizeFolder { .. } => "Synchronize Folder",
        // (nothing to download from in this build: the dialog explains the manual install)
        Dialog::SamModel { .. } if sam_by_hand(&app.session.segmenter) => "Install the SAM 3 Model",
        Dialog::SamModel { .. } => "Download the SAM 3 Model?",
        Dialog::SuperRes { .. } => "Super Resolution",
        Dialog::SyncChoice => "Sync This Library",
        Dialog::About => "About LightCraft",
        Dialog::Shortcuts => "Keyboard Shortcuts",
    }
    .to_string();
    // a model file LightCraft cannot use has nothing to confirm, nor has a dialog that only shows
    // something (one Close, no Cancel / OK)
    let unusable_model = matches!(&dlg, Dialog::DenoiseModel { info, .. } | Dialog::FaceModel { info, .. } if info["kind"] == "unsupported");
    let informational = unusable_model
        || matches!(&dlg, Dialog::PresetImport { review } if crate::preset_import::informational(review))
        || matches!(
            dlg,
            Dialog::About | Dialog::Shortcuts | Dialog::Settings { .. } | Dialog::SystemInfo { .. } | Dialog::AllMetadata { .. } | Dialog::WhatsNew
        );
    // installing waits for the licence to be accepted (and Synchronize waits for the scan); smart-album
    // rules that can't mean anything wait to be fixed
    let can_confirm = informational
        || !matches!(
            &dlg,
            Dialog::DenoiseModel { accepted: false, .. } | Dialog::FaceModel { accepted: false, .. } | Dialog::SynchronizeFolder { counts: None, .. }
        ) && !matches!(&dlg, Dialog::SmartRules { id, rules, .. } if !rules.check_for(&app.session.catalog, id.map(lightcraft_catalog::AlbumId)).is_empty())
        // (a link's password, if there is one, is long enough)
        && !matches!(&dlg, Dialog::ShareLink { password, .. } if !password.is_empty() && password.chars().count() < lightcraft_engine::cloud_sync::SHARE_PASSWORD_MIN);
    // (taken before `body` borrows the dialog)
    let confirm_delete = if let Dialog::ConfirmDelete { count } = &dlg { Some(*count) } else { None };
    let ok = ok_label(app, &dlg, informational);
    let cancel = cancel_label(app, &dlg, informational);
    let compact = app.compact;
    // a phone's export is a flow of pages of its own (`panels::export`)
    let export_flow = compact && matches!(dlg, Dialog::Export { .. });
    // a phone's Settings, as iOS's: the list, or a section with a back button; Done on the right
    let phone_settings = match &dlg {
        Dialog::Settings { tab } if compact => Some(crate::panels::settings::phone_title(tab)),
        _ => None,
    };
    // The import review can be resized (its photo grid takes the room); it keeps a window id of its
    // own so the size it is given doesn't carry over to the other dialogs. Its content scrolls
    // rather than growing the window when the options below the grid get taller (e.g. Copy).
    let import = matches!(dlg, Dialog::Import { .. });
    // the desktop Settings scrolls each section on its own, beside its sidebar
    let settings_page = !compact && matches!(dlg, Dialog::Settings { .. });
    let window_id = window_id(&dlg);
    let default_width = match dlg {
        Dialog::Import { .. } => crate::import::DIALOG_SIZE[0],
        Dialog::SmartRules { .. } => 680.0,
        Dialog::PresetImport { .. } => 520.0,
        Dialog::AllMetadata { .. } => 620.0,
        Dialog::FaceModel { .. } => 460.0,
        Dialog::LightroomCatalog { .. } => 440.0,
        _ => 380.0,
    };
    // Every other dialog fits the window (#781): its options scroll when they don't, and the title
    // bar and the button row stay outside the scroll area. The room they take is measured on the
    // previous frame (a guess on the first one).
    let chrome_id = window_id.with("chrome");
    let chrome = ctx.data(|d| d.get_temp::<f32>(chrome_id)).unwrap_or(DIALOG_CHROME_GUESS);
    let body_max = (screen.height() - 2.0 * DIALOG_SCREEN_MARGIN - chrome).max(DIALOG_MIN_BODY);
    // (a choice made in the body of the sync dialog closes it)
    let mut choice_made = false;
    let mut body = |ui: &mut egui::Ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        match &mut dlg {
            Dialog::EditIn { mode, .. } => {
                for m in crate::state::EditInMode::ALL {
                    let (label, what) = m.label();
                    let r = ui.radio_value(mode, m, crate::i18n::tr(label));
                    crate::widgets::register(ui.ctx(), format!("radio:editIn-{}", m.param()), r.rect);
                    ui.indent(("editIn", m.param()), |ui| ui.label(egui::RichText::new(crate::i18n::tr(what)).color(t.text_dim)));
                }
            }
            Dialog::AutoStack { gap } => {
                ui.label(egui::RichText::new(crate::i18n::tr("Stack photos taken within this time of each other:")).color(t.text_label));
                ui.add(
                    egui::Slider::new(gap, 0.0..=86400.0)
                        .logarithmic(true)
                        .smallest_positive(1.0)
                        .custom_formatter(|v, _| crate::panels::dialogs::fmt_gap(v))
                        .custom_parser(|s| s.trim().trim_end_matches('s').parse().ok()),
                );
                let preview = app.session.execute("stack.auto", &json!({"gap": *gap, "preview": true})).unwrap_or_default();
                let scope = if app.session.selection.ids.len() > 1 { "the selected photos" } else { "the photos in view" };
                ui.label(
                    egui::RichText::new(crate::i18n::tr_format!(
                        "Creates {} stacks from {} of {scope}",
                        preview["stacks"],
                        preview["photos"],
                        scope = crate::i18n::tr(scope)
                    ))
                    .color(t.text_dim),
                );
            }
            Dialog::Cull { reject_below, pick_best } => {
                let n = app.session.selection.ids.len();
                let scope = if n > 1 {
                    crate::i18n::tr_format!("the {n} selected photos", n = n)
                } else {
                    crate::i18n::tr_format!("the {} photos in view", app.session.visible_cloned().len())
                };
                ui.label(
                    egui::RichText::new(crate::i18n::tr_format!(
                        "Scores {scope} for focus and exposure and finds similar shots taken within seconds of each other (bursts).",
                        scope = scope
                    ))
                    .color(t.text_label),
                );
                ui.add_space(6.0);
                let r = ui.add(egui::Slider::new(reject_below, 0.0..=80.0).text(crate::i18n::tr("Reject below focus")).step_by(1.0));
                crate::widgets::register(ui.ctx(), "field:cullReject", r.rect);
                ui.label(
                    egui::RichText::new(crate::i18n::tr(if *reject_below > 0.0 {
                        "Blurry photos below the score are flagged as rejects."
                    } else {
                        "0: nothing is rejected."
                    }))
                    .color(t.text_dim),
                );
                let r = crate::widgets::check(ui, pick_best, crate::i18n::tr("Pick the sharpest photo of each burst"));
                crate::widgets::register(ui.ctx(), "check:cullPick", r.rect);
                ui.label(
                    egui::RichText::new(crate::i18n::tr(
                        "Scores stay on the photos: filter or make smart albums with Focus and Best of Similar Shots.",
                    ))
                    .color(t.text_dim),
                );
            }
            Dialog::WhatsNew => {
                egui::ScrollArea::vertical().max_height(list_height(ui, 460.0)).auto_shrink([false, true]).show(ui, |ui| {
                    for line in WHATS_NEW.lines() {
                        let l = line.trim_end();
                        if let Some(h) = l.strip_prefix("### ") {
                            ui.add_space(6.0);
                            ui.label(egui::RichText::new(crate::i18n::tr(h)).font(t.semibold(12.5)).color(t.text));
                        } else if let Some(h) = l.strip_prefix("## ") {
                            ui.add_space(8.0);
                            ui.label(egui::RichText::new(crate::i18n::tr(h)).font(t.semibold(14.0)).color(t.text));
                        } else if l.starts_with("# ") || l.is_empty() {
                        } else if let Some(b) = l.strip_prefix("- ") {
                            ui.label(egui::RichText::new(format!("•  {}", crate::i18n::tr(b).replace('`', ""))).color(t.text_label));
                        } else {
                            ui.label(egui::RichText::new(crate::i18n::tr(l.trim()).replace('`', "")).color(t.text_label));
                        }
                    }
                });
            }
            Dialog::DenoiseModel { info, accepted } => crate::panels::denoise::model_dialog(app, ui, &t, info, accepted),
            Dialog::FaceModel { info, accepted, .. } => crate::panels::faces::model_dialog(app, ui, &t, info, accepted),
            Dialog::SystemInfo { rows } => {
                egui::Grid::new("sysinfo").num_columns(2).spacing([16.0, 4.0]).striped(true).show(ui, |ui| {
                    for (k, v) in rows.iter() {
                        ui.label(egui::RichText::new(crate::i18n::tr(k)).color(t.text_dim));
                        ui.add(egui::Label::new(egui::RichText::new(v).color(t.text_label)).wrap());
                        ui.end_row();
                    }
                });
                if ui.button(crate::i18n::tr("Copy to Clipboard")).clicked() {
                    let text: String = rows.iter().map(|(k, v)| format!("{k}: {v}\n")).collect();
                    ui.ctx().copy_text(text);
                }
            }
            Dialog::AllMetadata { title, rows, search } => {
                ui.label(egui::RichText::new(title.as_str()).color(t.text_label));
                let r = ui.add(crate::widgets::text_box(
                    ui,
                    egui::TextEdit::singleline(search).hint_text(crate::i18n::tr("Filter fields")).desired_width(f32::INFINITY),
                ));
                crate::widgets::register(ui.ctx(), "field:metadataSearch", r.rect);
                let q = search.trim().to_lowercase();
                let keep = |n: &str, v: &str| q.is_empty() || n.to_lowercase().contains(&q) || v.to_lowercase().contains(&q);
                let mut groups: Vec<(String, Vec<(String, String)>)> = Vec::new();
                for row in rows["exif"].as_array().into_iter().flatten() {
                    let (g, n, v) = (row["group"].as_str().unwrap_or(""), row["name"].as_str().unwrap_or(""), row["value"].as_str().unwrap_or(""));
                    if !keep(n, v) {
                        continue;
                    }
                    match groups.iter_mut().find(|(x, _)| x == g) {
                        Some((_, list)) => list.push((n.into(), v.into())),
                        None => groups.push((g.into(), vec![(n.into(), v.into())])),
                    }
                }
                let xmp: Vec<(String, String)> = rows["xmp"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|r| (r["name"].as_str().unwrap_or("").to_string(), r["value"].as_str().unwrap_or("").to_string()))
                    .filter(|(n, v)| keep(n, v))
                    .collect();
                if !xmp.is_empty() {
                    groups.push(("XMP".into(), xmp));
                }
                egui::ScrollArea::vertical().max_height(list_height(ui, 440.0)).auto_shrink([false, true]).show(ui, |ui| {
                    if groups.is_empty() {
                        ui.label(egui::RichText::new(crate::i18n::tr(rows["note"].as_str().unwrap_or("No metadata found"))).color(t.text_dim));
                    }
                    for (g, list) in &groups {
                        ui.add_space(6.0);
                        ui.label(egui::RichText::new(g).font(t.semibold(12.5)).color(t.text));
                        egui::Grid::new(format!("meta-{g}")).num_columns(2).spacing([16.0, 3.0]).striped(true).show(ui, |ui| {
                            for (n, v) in list {
                                ui.label(egui::RichText::new(n).color(t.text_dim));
                                ui.add(egui::Label::new(egui::RichText::new(v).color(t.text_label)).wrap());
                                ui.end_row();
                            }
                        });
                    }
                });
            }
            Dialog::SmartRules { id, name, rules, .. } => {
                let r = ui.add(crate::widgets::text_box(
                    ui,
                    egui::TextEdit::singleline(name).hint_text(crate::i18n::tr("Name")).desired_width(f32::INFINITY),
                ));
                crate::widgets::register(ui.ctx(), "field:smartName", r.rect);
                ui.add_space(6.0);
                // the folder an album made from a folder view carries is not in the editor, but it counts
                let folder = id
                    .and_then(|id| app.session.catalog.album(lightcraft_catalog::AlbumId(id)))
                    .and_then(|a| a.smart.as_deref().and_then(|f| f.library_folder.clone()));
                // problems, count and albums: cached until the catalog or the rules change (not per frame)
                let now = (app.session.clock)();
                let view = app.caches.rules_view(&app.session.catalog, rules, *id, folder, &now);
                let env = crate::panels::rules_editor::Env { problems: view.problems.clone(), today: now, albums: view.albums.clone() };
                egui::ScrollArea::vertical().max_height(list_height(ui, 360.0)).auto_shrink([false, true]).show(ui, |ui| {
                    crate::panels::rules_editor::edit(ui, rules, "rules", 0, &[], &env);
                });
                let n = view.count;
                ui.add_space(4.0);
                if view.problems.is_empty() {
                    ui.label(
                        egui::RichText::new(crate::i18n::tr_format!(
                            "{n} photo{} match · updates automatically as photos change",
                            if n == 1 { "" } else { "s" },
                            n = n
                        ))
                        .color(t.text_dim),
                    );
                } else {
                    ui.label(egui::RichText::new(crate::i18n::tr("Fix the marked rules to save this album.")).color(t.caution));
                }
            }
            Dialog::NewSmartAlbum { name, .. } => {
                if dialog_field(ui, "field:smartAlbumName", name, "Name") {
                    confirm = true;
                }
                let rules = app.session.view_rules();
                let n = app.session.catalog.query(&rules, &Default::default()).len();
                ui.label(
                    egui::RichText::new(crate::i18n::tr_format!("Matches: {}", crate::i18n::filter_label(&rules, &app.session.catalog)))
                        .color(t.text_label),
                );
                ui.label(
                    egui::RichText::new(crate::i18n::tr_format!(
                        "{n} photo{} now · updates automatically as photos change",
                        if n == 1 { "" } else { "s" },
                        n = n
                    ))
                    .color(t.text_dim),
                );
            }
            Dialog::CaptureTime { mode, time, days, hours, minutes, zone } => {
                let n = app.session.targets(&json!({})).len();
                field(ui, "Change", |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    for (i, (label, m)) in [("Set date & time", "set"), ("Shift by", "shift"), ("Time zone", "zone")].iter().enumerate() {
                        if crate::widgets::text_button(ui, &format!("captureMode-{i}"), label, mode == m).clicked() {
                            *mode = m.to_string();
                        }
                    }
                });
                let params = match mode.as_str() {
                    "set" => {
                        field(ui, "New time", |ui| {
                            let r = ui.add(crate::widgets::text_box(
                                ui,
                                egui::TextEdit::singleline(time).hint_text("2026-09-30 14:05:00").desired_width(f32::INFINITY),
                            ));
                            crate::widgets::register(ui.ctx(), "field:captureTime", r.rect);
                        });
                        json!({"time": time})
                    }
                    "shift" => {
                        field(ui, "Shift", |ui| {
                            ui.add(egui::DragValue::new(days).suffix(" d"));
                            ui.add(egui::DragValue::new(hours).suffix(" h"));
                            ui.add(egui::DragValue::new(minutes).suffix(" min"));
                        });
                        json!({"shift": *days as i64 * 86_400 + *hours as i64 * 3600 + *minutes as i64 * 60})
                    }
                    _ => {
                        field(ui, "Hours", |ui| ui.add(egui::DragValue::new(zone).range(-26.0..=26.0).speed(0.25).fixed_decimals(2)));
                        json!({"hours": zone})
                    }
                };
                // preview on the active photo (the others move by the same amount)
                let active = app.session.active().and_then(|id| app.session.catalog.photo(id)).map(|p| (p.file_name.clone(), p.date().to_string()));
                if let Some((name, cur)) = active {
                    let delta = match mode.as_str() {
                        "set" => lightcraft_catalog::dates::normalize_iso(time)
                            .and_then(|t| Some(lightcraft_catalog::dates::iso_seconds(&t)? - lightcraft_catalog::dates::iso_seconds(&cur)?)),
                        "shift" => params["shift"].as_i64(),
                        _ => Some((*zone as f64 * 3600.0).round() as i64),
                    };
                    let after = delta.and_then(|d| lightcraft_catalog::dates::shift_iso(&cur, d));
                    ui.label(
                        egui::RichText::new(match after {
                            Some(a) => format!("{name}: {} → {}", cur.replace('T', " "), a.replace('T', " ")),
                            None => "Enter a date as YYYY-MM-DD HH:MM:SS".into(),
                        })
                        .color(t.text_label),
                    );
                }
                if n > 1 {
                    ui.label(
                        egui::RichText::new(crate::i18n::tr_format!("All {n} selected photos move by the same amount.", n = n)).color(t.text_dim),
                    );
                }
            }
            Dialog::LabelNames { names, save_as } => {
                names.resize(5, String::new());
                // start from a set
                let sets = lightcraft_engine::cmd::manage::label_sets_json(&app.session);
                field(ui, "Set", |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    for set in sets["sets"].as_array().into_iter().flatten() {
                        let name = set["name"].as_str().unwrap_or_default();
                        if crate::widgets::text_button(ui, &format!("labelSet-{name}"), name, false).clicked() {
                            *names = set["names"].as_array().into_iter().flatten().map(|n| n.as_str().unwrap_or_default().to_string()).collect();
                        }
                    }
                });
                for (i, l) in lightcraft_catalog::ColorLabel::ALL.iter().enumerate() {
                    let colour = format!("{l:?}");
                    field(ui, &colour, |ui| {
                        let (r, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                        ui.painter().circle_filled(r.center(), 6.0, crate::panels::grid::label_color(*l));
                        let te = ui.add(crate::widgets::text_box(
                            ui,
                            egui::TextEdit::singleline(&mut names[i]).hint_text(colour.as_str()).desired_width(f32::INFINITY),
                        ));
                        crate::widgets::register(ui.ctx(), format!("field:labelName-{}", colour.to_lowercase()), te.rect);
                    });
                }
                field(ui, "Save as set", |ui| {
                    let te = ui.add(crate::widgets::text_box(
                        ui,
                        egui::TextEdit::singleline(save_as).hint_text(crate::i18n::tr("Optional name")).desired_width(f32::INFINITY),
                    ));
                    crate::widgets::register(ui.ctx(), "field:labelSetName", te.rect);
                });
                ui.label(
                    egui::RichText::new(crate::i18n::tr(
                        "Names appear in the label menu, the filter bar and the Info panel, and are written to XMP. Empty = the colour's name.",
                    ))
                    .color(t.text_dim),
                );
            }
            Dialog::Rename { template, start } => {
                let n = app.session.targets(&json!({})).len();
                ui.label(egui::RichText::new(crate::i18n::tr_format!("{n} photo{}", if n == 1 { "" } else { "s" }, n = n)).color(t.text_dim));
                let template_id = egui::Id::new("rename-template");
                let tags_open = field(ui, "Template", |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    let w = crate::import::tag_field_width(ui);
                    let r = ui
                        .add(crate::widgets::text_box(ui, egui::TextEdit::singleline(template).id(template_id).hint_text("{name}").desired_width(w)));
                    crate::widgets::register(ui.ctx(), "field:renameTemplate", r.rect);
                    crate::import::tag_toggle(ui, "renameTemplate")
                });
                if tags_open {
                    crate::import::tag_help(ui, "renameTemplate", template, template_id);
                }
                crate::import::unknown_tags_warning(ui, template);
                field(ui, "Presets", |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    for (i, (label, tpl)) in
                        [("Name", "{name}"), ("Date–Name", "{date}_{name}"), ("Title–Seq", "{title}-{seq:3}"), ("Custom–Seq", "Photo-{seq:3}")]
                            .iter()
                            .enumerate()
                    {
                        if crate::widgets::text_button(ui, &format!("renamePreset-{i}"), label, template == tpl).clicked() {
                            *template = tpl.to_string();
                        }
                    }
                });
                field(ui, "Start at", |ui| ui.add(egui::DragValue::new(start).range(0..=999_999)));
                ui.label(
                    egui::RichText::new(crate::i18n::tr(
                        "Tags: see Tags beside the template. Files are renamed on disk (with their XMP sidecars); existing names get -1, -2…",
                    ))
                    .color(t.text_dim),
                );
                let (preview, total) = rename_preview(app, ui.ctx(), template, *start as usize);
                match &preview {
                    Some(rows) => {
                        egui::Grid::new("rename-preview").num_columns(3).spacing([8.0, 2.0]).show(ui, |ui| {
                            for pl in rows {
                                ui.label(egui::RichText::new(&pl.from).color(t.text_dim));
                                ui.label(egui::RichText::new("→").color(t.text_dim));
                                ui.label(egui::RichText::new(&pl.to).color(t.text));
                                ui.end_row();
                            }
                        });
                    }
                    None => {
                        ui.label(egui::RichText::new(crate::i18n::tr("Checking names…")).color(t.text_dim));
                    }
                }
                let more = total.saturating_sub(RENAME_PREVIEW_ROWS);
                if more > 0 {
                    ui.label(egui::RichText::new(crate::i18n::tr_format!("… and {more} more", more = more)).color(t.text_dim));
                }
            }
            Dialog::RenameKeyword { from, to } => {
                let n =
                    app.session.catalog.photos().filter(|p| p.meta.keywords.iter().any(|k| lightcraft_catalog::keywords::is_under(k, from))).count();
                if dialog_field(ui, "field:keywordName", to, "New name") {
                    confirm = true;
                }
                ui.label(
                    egui::RichText::new(crate::i18n::tr_format!("Renames “{from}” on {n} photo{} (keywords below it too). Use | for levels, e.g. Travel|Italy. An existing name merges the two.",
                        if n == 1 { "" } else { "s" }, from = from, n = n))
                    .color(t.text_dim),
                );
            }
            Dialog::MergeKeywords { from, into } => {
                ui.label(
                    egui::RichText::new(crate::i18n::tr_format!(
                        "Replace {} with:",
                        from.iter().map(|f| format!("“{f}”")).collect::<Vec<_>>().join(", ")
                    ))
                    .color(t.text_label),
                );
                if dialog_field(ui, "field:keywordInto", into, "Keyword") {
                    confirm = true;
                }
                let options: Vec<String> = app
                    .session
                    .catalog
                    .keyword_suggestions(from, into, 12)
                    .into_iter()
                    .filter(|k| !from.iter().any(|f| lightcraft_catalog::keywords::is_under(k, f)))
                    .collect();
                ui.horizontal_wrapped(|ui| {
                    for k in options {
                        if ui.add(egui::Button::new(egui::RichText::new(&k).color(t.text_label)).corner_radius(10.0)).clicked() {
                            *into = k;
                        }
                    }
                });
            }
            Dialog::TextPrompt { value, hint, .. } => {
                if dialog_field(ui, "field:prompt", value, hint) {
                    confirm = true;
                }
            }
            Dialog::NewAlbum { name, .. } | Dialog::RenameAlbum { name, .. } => {
                if dialog_field(ui, "field:albumName", name, "Name") {
                    confirm = true;
                }
            }
            Dialog::ShareLink { name, days, originals, upload, password, .. } => {
                if share_link_body(ui, &t, name, days, originals, upload, password) {
                    confirm = true;
                }
            }
            Dialog::CreatePreset { name, group, groups } => {
                ui.add(crate::widgets::text_box(
                    ui,
                    egui::TextEdit::singleline(name).hint_text(crate::i18n::tr("Preset name")).desired_width(f32::INFINITY),
                ));
                ui.add(crate::widgets::text_box(
                    ui,
                    egui::TextEdit::singleline(group).hint_text(crate::i18n::tr("Group")).desired_width(f32::INFINITY),
                ));
                ui.label(egui::RichText::new(crate::i18n::tr("Settings to include")).color(t.text_dim));
                group_checklist(ui, "presetInclude", groups);
            }
            Dialog::CopySettings { groups } => group_checklist(ui, "copyGroup", groups),
            Dialog::PasteSettings { groups } => {
                let n = app.session.targets(&json!({})).len();
                ui.label(
                    egui::RichText::new(crate::i18n::tr_format!(
                        "Paste into {n} photo{} — only settings that were copied are pasted",
                        if n == 1 { "" } else { "s" },
                        n = n
                    ))
                    .color(t.text_dim),
                );
                group_checklist(ui, "pasteGroup", groups);
            }
            Dialog::ContactSheet { options } => {
                options.columns = options.columns.clamp(1, 8);
                options.rows = options.rows.clamp(1, 10);
                let n = app.session.targets(&json!({})).len();
                ui.label(format!("{n} selected photos · {} pages", n.div_ceil(options.columns * options.rows)));
                field(ui, "Paper", |ui| {
                    egui::ComboBox::from_id_salt("contactSheetPaper").selected_text(&options.paper).show_ui(ui, |ui| {
                        ui.selectable_value(&mut options.paper, "a4".into(), "A4");
                        ui.selectable_value(&mut options.paper, "letter".into(), "Letter");
                    });
                });
                ui.checkbox(&mut options.landscape, "Landscape");
                field(ui, "Columns", |ui| {
                    ui.add(egui::DragValue::new(&mut options.columns).range(1..=8));
                });
                field(ui, "Rows", |ui| {
                    ui.add(egui::DragValue::new(&mut options.rows).range(1..=10));
                });
                ui.checkbox(&mut options.captions, "Filename captions");
                ui.label("150 dpi · sRGB · current edits · fit without cropping");
                ui.label("Choose a PDF destination after confirming. Print the PDF from your viewer.");
            }
            Dialog::Export { opts, full_size, resize, preset_name, limit_kb, dir, .. } => {
                super::export::form(app, ui, super::export::Form { opts, full_size, resize, preset_name, limit_kb, dir }, super::export::Part::All);
            }
            Dialog::Merge { opts } => crate::merge::body(app, ui, opts),
            Dialog::PresetImport { review } => crate::preset_import::body(app, ui, review),
            Dialog::Import { opts } if app.compact => crate::import::mobile_body(app, ui, opts),
            Dialog::Import { opts } => crate::import::body(app, ui, opts),
            Dialog::Settings { tab } => crate::panels::settings::body(app, ui, tab, body_max),
            Dialog::SamModel { error, .. } => sam_model_body(app, ui, error.as_deref()),
            Dialog::SuperRes { error } => crate::superres::body(app, ui, error.as_deref()),
            Dialog::SyncChoice => choice_made |= super::sync_choice::body(app, ui),
            Dialog::KeywordTag {
                editing,
                name,
                parent,
                inside,
                synonyms,
                include_on_export,
                export_containing,
                export_synonyms,
                person,
                add_to_selected,
                ..
            } => {
                ui.label(egui::RichText::new(crate::i18n::tr("Keyword Name")).color(t.text_dim));
                // several fields: the name takes the focus when the dialog opens, not after
                let r = crate::text_field::TextField::singleline("field:keywordTagName", name)
                    .hint(crate::i18n::tr("Keyword Name"))
                    .width(f32::INFINITY)
                    .select_on_focus(true)
                    .show(ui);
                if just_opened(ui, "keyword-tag") {
                    r.response.request_focus();
                }
                if r.ending == Some(crate::text_field::Ending::Return) {
                    confirm = true;
                }
                ui.add_space(4.0);
                ui.label(egui::RichText::new(crate::i18n::tr("Synonyms")).color(t.text_dim));
                // Return in either field creates (or saves) the keyword
                let r = crate::text_field::TextField::singleline("field:keywordSynonyms", synonyms)
                    .hint(crate::i18n::tr("Separated by commas"))
                    .width(f32::INFINITY)
                    .show(ui);
                if r.ending == Some(crate::text_field::Ending::Return) {
                    confirm = true;
                }
                ui.add_space(6.0);
                if editing.is_none() {
                    if let Some(p) = parent {
                        let r = ui.checkbox(inside, crate::i18n::tr_format!("Put inside “{parent}”", parent = p.replace('|', " › ")));
                        crate::widgets::register(ui.ctx(), "check:keywordInside", r.rect);
                    }
                    let n = app.session.selection.ids.len();
                    let r = ui.add_enabled(n > 0, egui::Checkbox::new(add_to_selected, crate::i18n::tr("Add to Selected Photos")));
                    crate::widgets::register(ui.ctx(), "check:keywordAddToSelected", r.rect);
                    ui.add_space(4.0);
                }
                ui.label(egui::RichText::new(crate::i18n::tr("Keyword Tag Options")).color(t.text_dim));
                for (on, label, id) in [
                    (include_on_export, "Include on Export", "check:keywordIncludeOnExport"),
                    (export_containing, "Export Containing Keywords", "check:keywordExportContaining"),
                    (export_synonyms, "Export Synonyms", "check:keywordExportSynonyms"),
                    (person, "Person", "check:keywordPerson"),
                ] {
                    let r = ui.checkbox(on, crate::i18n::tr(label));
                    crate::widgets::register(ui.ctx(), id, r.rect);
                }
            }
            Dialog::KeywordSet { replaces, name, slots, as_new } => {
                ui.label(egui::RichText::new(crate::i18n::tr("Set name")).color(t.text_dim));
                let r = crate::text_field::TextField::singleline("field:keywordSetName", name)
                    .hint(crate::i18n::tr("Set name"))
                    .width(f32::INFINITY)
                    .select_on_focus(true)
                    .show(ui);
                if just_opened(ui, "keyword-set") {
                    r.response.request_focus();
                }
                let mut returned = r.ending == Some(crate::text_field::Ending::Return);
                ui.add_space(6.0);
                ui.label(egui::RichText::new(crate::i18n::tr("Keywords (⌥1–⌥9)")).color(t.text_dim));
                slots.resize(9, String::new());
                // equal columns of a set width (the window is still finding its own)
                let w = 130.0;
                egui::Grid::new("keyword-set-slots").num_columns(3).min_col_width(w).spacing([6.0, 4.0]).show(ui, |ui| {
                    for (i, slot) in slots.iter_mut().enumerate() {
                        let r = crate::text_field::TextField::singleline(&format!("field:keywordSetSlot:{}", i + 1), slot)
                            .hint(format!("⌥{}", i + 1))
                            .width(w)
                            .show(ui);
                        returned |= r.ending == Some(crate::text_field::Ending::Return);
                        if i % 3 == 2 {
                            ui.end_row();
                        }
                    }
                });
                if replaces.is_some() {
                    ui.add_space(6.0);
                    let r = ui.checkbox(as_new, crate::i18n::tr("Save as a new set"));
                    crate::widgets::register(ui.ctx(), "check:keywordSetAsNew", r.rect);
                }
                // Return in any field saves the set
                if returned {
                    confirm = true;
                }
            }
            Dialog::MoveKeyword { keyword, parent } => {
                let name = keyword.rsplit('|').next().unwrap_or(keyword);
                match parent {
                    Some(p) => ui.label(crate::i18n::tr_format!(
                        "“{parent}” has a “{name}” already. Merge the two?",
                        parent = p.replace('|', " › "),
                        name = name
                    )),
                    None => ui.label(crate::i18n::tr_format!("There is a “{name}” at the top level already. Merge the two?", name = name)),
                };
                ui.label(
                    egui::RichText::new(crate::i18n::tr("Their photos and the keywords below them come together under one keyword."))
                        .color(t.text_dim),
                );
            }
            Dialog::DeleteKeyword { keyword, count } => {
                let name = keyword.replace('|', " › ");
                ui.label(crate::i18n::tr_format!("Delete the keyword “{name}”?", name = name));
                ui.label(
                    egui::RichText::new(if *count == 0 {
                        crate::i18n::tr("No photo has it; the keywords below it go too.").to_string()
                    } else {
                        crate::i18n::tr_format!(
                            "It is taken off {count} photo{}, with the keywords below it. Undo brings it back.",
                            if *count == 1 { "" } else { "s" },
                            count = count
                        )
                    })
                    .color(t.text_dim),
                );
            }
            Dialog::ConfirmDelete { count } => {
                let what = if *count == 1 {
                    crate::i18n::tr("this photo").to_string()
                } else {
                    crate::i18n::tr_format!("these {count} photos", count = count)
                };
                ui.label(crate::i18n::tr_format!("Move {what} to Recently Deleted?", what = what));
                ui.label(egui::RichText::new(crate::i18n::tr("They can be restored from Recently Deleted until it is emptied.")).color(t.text_dim));
            }
            d @ Dialog::SynchronizeFolder { .. } => crate::sync::body(app, ui, d),
            Dialog::LightroomCatalog { folder, catalog } => {
                ui.label(crate::i18n::tr("This is a Lightroom catalog, not a LightCraft library. Import it into a LightCraft library instead?"));
                ui.label(egui::RichText::new(catalog.as_deref().unwrap_or(folder.as_str())).color(t.text_dim));
                ui.label(
                    egui::RichText::new(crate::i18n::tr(
                        "The import reads the catalog without changing it and adds its photos, edits, keywords and collections to the library open now.",
                    ))
                    .color(t.text_dim),
                );
            }
            Dialog::RemoveFolder { name, count, path, disk } => {
                ui.label(crate::i18n::tr_format!(
                    "Remove “{name}” and its {count} photo{} from the library?",
                    if *count == 1 { "" } else { "s" },
                    name = name,
                    count = count
                ));
                ui.label(egui::RichText::new(path.as_str()).color(t.text_dim));
                let scope =
                    if *disk { "All the photos imported from this disk are included." } else { "That includes the photos in the folders inside it." };
                ui.label(egui::RichText::new(crate::i18n::tr(scope)).color(t.text_dim));
                ui.label(
                    egui::RichText::new(crate::i18n::tr("They move to Recently Deleted and can be restored; no file on disk is touched."))
                        .color(t.text_dim),
                );
            }
            Dialog::About => {
                if !compact {
                    ui.set_min_width(680.0);
                }
                let tab_id = egui::Id::new("about_tab");
                let mut tab = ui.data_mut(|d| d.get_temp::<u8>(tab_id)).unwrap_or(0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    for (i, (id, label)) in ABOUT_TABS.iter().enumerate() {
                        if crate::widgets::text_button(ui, &format!("aboutTab-{id}"), label, usize::from(tab) == i).clicked() {
                            tab = u8::try_from(i).unwrap_or(0);
                        }
                    }
                });
                ui.data_mut(|d| d.insert_temp(tab_id, tab));
                ui.separator();
                match tab {
                    1 => crate::credits::contributors_ui(app, ui),
                    2 => crate::credits::models_ui(ui),
                    _ => {
                        ui.label(egui::RichText::new("LightCraft").font(t.semibold(20.0)).color(t.text));
                        ui.label(crate::i18n::tr_format!(
                            "Version {} — a clean-room, pure-Rust photo library and raw developer.",
                            env!("CARGO_PKG_VERSION")
                        ));
                        ui.label(crate::i18n::tr_format!(
                            "MIT OR Apache-2.0. Fonts: {} (OFL). Icons: original.",
                            crate::theme::font_credits(app.chinese_font.is_some())
                        ));
                        ui.add_space(10.0);
                        let discord = egui::Button::new(
                            egui::RichText::new(crate::i18n::tr("Join the ArtCraft Discord")).font(t.semibold(15.0)).color(egui::Color32::WHITE),
                        )
                        .fill(t.accent)
                        .min_size(egui::vec2(260.0, 34.0));
                        let r = ui.add(discord).on_hover_text(crate::links::DISCORD);
                        crate::widgets::register(ui.ctx(), "button:aboutDiscord", r.rect);
                        if r.clicked() {
                            let _ = crate::links::open(app, crate::links::DISCORD);
                        }
                        ui.add_space(6.0);
                        for (label, url) in [
                            ("LightCraft website", crate::links::APP_PAGE),
                            ("Source code on GitHub", crate::links::GITHUB),
                            ("ArtCraft — more creative apps", crate::links::WEBSITE),
                        ] {
                            let r = ui.link(crate::i18n::tr(label)).on_hover_text(url);
                            if r.clicked() {
                                let _ = crate::links::open(app, url);
                            }
                        }
                    }
                }
            }
            Dialog::Shortcuts => crate::panels::keymap::body(app, ui, &t),
        }
    };
    let (mut cancel_tapped, mut ok_tapped) = (false, false);
    if compact && let Some(count) = confirm_delete {
        if leaving {
            // (an alert has no page to slide away: it is gone with its answer)
            app.ui.dialog_leaving = None;
            super::alert::hidden(ctx, "dialog");
            return;
        }
        // a yes-or-no question on a phone is an alert, not a page
        let what =
            if count == 1 { crate::i18n::tr("this photo").to_string() } else { crate::i18n::tr_format!("these {count} photos", count = count) };
        let title = crate::i18n::tr_format!("Move {what} to Recently Deleted?", what = what);
        let alert = super::alert::Alert {
            id: "dialog",
            title: &title,
            message: "They can be restored from Recently Deleted until it is emptied.",
            cancel,
            ok: &ok,
            destructive: true,
        };
        (cancel_tapped, ok_tapped) = super::alert::show(ctx, &alert);
    } else if export_flow {
        // share sheet, options, more options
        let (outcome, gone) = super::export::phone(app, ctx, &mut dlg, !leaving);
        if leaving {
            if gone {
                app.ui.dialog_leaving = None;
            }
            return;
        }
        match outcome {
            super::export::Outcome::Stay => {}
            super::export::Outcome::Close => cancel_tapped = true,
            super::export::Outcome::Go(to) => {
                if let Dialog::Export { then, .. } = &mut dlg {
                    *then = to;
                }
                ok_tapped = true;
            }
        }
    } else if compact {
        // a phone: a page covering the screen, the action in its bar
        let mut action = (!informational && !ok.is_empty()).then_some((ok.as_str(), can_confirm));
        let (mut title, mut cancel) = (title.clone(), cancel.to_string());
        if let Some((page, back)) = phone_settings {
            title = page.to_string();
            cancel = back.map(|b| format!("‹{}", crate::i18n::tr(b))).unwrap_or_default();
            action = Some((crate::i18n::tr("Done"), true));
        }
        let bar = super::mobile::page(ctx, "dialog", &title, &cancel, action, !leaving, &mut body);
        (cancel_tapped, ok_tapped) = (bar.cancel, bar.ok);
        // (back to the list)
        if let Dialog::Settings { tab } = &mut dlg
            && cancel_tapped
            && !tab.is_empty()
        {
            tab.clear();
            cancel_tapped = false;
        }
        if leaving {
            if bar.gone {
                app.ui.dialog_leaving = None;
            }
            return;
        }
    } else {
        let frame = egui::Frame::window(&ctx.global_style()).inner_margin(egui::Margin::symmetric(16, 12));
        let mut body_h = None;
        let shown = egui::Window::new(crate::i18n::tr(&title))
            .id(window_id)
            .collapsible(false)
            .resizable(import)
            .vscroll(import)
            .resize(|r| if import { r.default_height(crate::import::DIALOG_SIZE[1]) } else { r })
            .frame(frame)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .default_width(default_width)
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 8.0;
                body_h = if settings_page {
                    body(ui);
                    Some(crate::panels::settings::desktop_height(body_max))
                } else {
                    dialog_body(ui, (!import).then_some(title.as_str()), body_max, &mut body)
                };
                ui.add_space(4.0);
                // (Settings' Close sits at its end, under the section)
                let layout =
                    if settings_page { egui::Layout::right_to_left(egui::Align::Center) } else { egui::Layout::left_to_right(egui::Align::Center) };
                ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), 26.0), layout, |ui| {
                    // Studio: capsules, the action in the accent colour (as on the admin page)
                    let capsule = |text: egui::WidgetText| {
                        let b = egui::Button::new(text);
                        if t.cards() { b.corner_radius(t.button_radius).min_size(egui::vec2(64.0, 26.0)) } else { b }
                    };
                    if !informational {
                        let r = ui.add(capsule(crate::i18n::tr(cancel).into()));
                        crate::widgets::register(ui.ctx(), "button:dialogCancel", r.rect);
                        cancel_tapped = r.clicked();
                    }
                    if !ok.is_empty() {
                        let button = if t.cards() && can_confirm {
                            capsule(egui::RichText::new(ok.as_str()).color(egui::Color32::WHITE).strong().into()).fill(t.accent)
                        } else {
                            capsule(ok.as_str().into())
                        };
                        let r = ui.add_enabled(can_confirm, button);
                        crate::widgets::register(ui.ctx(), "button:dialogOk", r.rect);
                        ok_tapped = r.clicked();
                    }
                });
                if import {
                    crate::import::note_dialog_bottom(ui);
                }
            });
        if let Some(w) = shown {
            ctx.memory_mut(|m| m.set_modal_layer(w.response.layer_id));
            if let Some(body_h) = body_h {
                let measured = (w.response.rect.height() - body_h).max(0.0);
                if (measured - chrome).abs() > 0.5 {
                    ctx.data_mut(|d| d.insert_temp(chrome_id, measured));
                    ctx.request_repaint();
                }
            }
            ctx.move_to_top(w.response.layer_id);
            crate::widgets::register(ctx, "dialog:window", w.response.rect);
        }
    }
    if choice_made {
        cancel_tapped = true;
    }
    if cancel_tapped || (ok_tapped && informational) {
        close = true;
    } else if ok_tapped {
        confirm = true;
    }
    // (while the shortcuts editor records a key, it takes Esc itself to cancel; an open popup in
    // the dialog, such as a date picker's calendar, takes it to close itself)
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) && !crate::widgets::escape_taken(ctx) {
        close = true;
    }
    if confirm {
        match confirm_dialog(app, &dlg) {
            // a phone's export stays open on an error: the options may be what to change
            Err(e) if compact && matches!(dlg, Dialog::Export { .. }) => app.toast_error(ctx, e),
            // the import review stays open on an error (e.g. an unusable folder template), and so
            // does Export (e.g. no folder chosen) so the choices aren't lost
            Err(e)
                if matches!(
                    dlg,
                    Dialog::Import { .. }
                        | Dialog::Export { .. }
                        | Dialog::ContactSheet { .. }
                        | Dialog::NewAlbum { .. }
                        | Dialog::NewSmartAlbum { .. }
                        | Dialog::SmartRules { .. }
                        | Dialog::KeywordTag { .. }
                        | Dialog::KeywordSet { .. }
                        | Dialog::SynchronizeFolder { .. }
                        | Dialog::ShareLink { .. }
                ) =>
            {
                app.toast(ctx, e)
            }
            // the SAM 3 dialog stays open to show the download (or why it can't start)
            Err(e) if matches!(dlg, Dialog::SamModel { .. } | Dialog::SuperRes { .. }) => match &mut dlg {
                Dialog::SamModel { error, .. } | Dialog::SuperRes { error } => *error = Some(e),
                _ => {}
            },
            Ok(r) if matches!(dlg, Dialog::NewAlbum { .. }) && r.get("skipped").and_then(serde_json::Value::as_u64).is_some_and(|n| n > 0) => {
                // albums hold library photos (issue #684): say which part of the selection stayed out
                crate::panels::grid::toast_skipped(app, ctx, &r);
                close = true;
            }
            Ok(_) if keeps_open(app, &dlg) => match &mut dlg {
                Dialog::SamModel { error, .. } | Dialog::SuperRes { error } => *error = None,
                _ => {}
            },
            _ => close = true,
        }
    }
    // a dialog opened from inside this one (a button in Settings that shows a licence) takes its place
    if app.ui.dialog.as_ref().is_some_and(|now| *now != at_start) {
        return;
    }
    if close {
        // (the keyboard goes with it)
        ctx.memory_mut(|m| {
            if let Some(f) = m.focused() {
                m.surrender_focus(f);
            }
        });
        app.ui.dialog_leaving = compact.then_some(dlg);
        app.ui.dialog = None;
    } else {
        app.ui.dialog = Some(dlg);
    }
}

/// Room kept free above and below a dialog (#781).
const DIALOG_SCREEN_MARGIN: f32 = 12.0;
/// A dialog's title bar, frame and button row before the first frame measures them.
const DIALOG_CHROME_GUESS: f32 = 100.0;
/// A dialog's options never get less room than this, however short the window.
const DIALOG_MIN_BODY: f32 = 80.0;

/// A list that scrolls on its own never gets shorter than this to make its dialog fit.
const DIALOG_MIN_LIST: f32 = 120.0;

fn list_shrink_id() -> egui::Id {
    egui::Id::new("dialog-list-shrink")
}

fn list_applied_id() -> egui::Id {
    egui::Id::new("dialog-list-applied")
}

/// The height for a list in a dialog that scrolls on its own (What's New, All Metadata, the
/// shortcuts, smart-album rules): `preferred`, less what the dialog needs to fit the window, so
/// on a short window the list gets shorter rather than scrolling inside a scrolling dialog.
pub fn list_height(ui: &egui::Ui, preferred: f32) -> f32 {
    let shrink = ui.data(|d| d.get_temp::<f32>(list_shrink_id())).unwrap_or(0.0);
    let height = (preferred - shrink).max(preferred.min(DIALOG_MIN_LIST));
    ui.data_mut(|d| *d.get_temp_mut_or_default::<f32>(list_applied_id()) += preferred - height);
    height
}

/// A dialog's options: in a vertical scroll area at most `max_height` tall when `scroll` names
/// the dialog (it shrinks to its content, so a dialog that fits looks as it would without it;
/// each dialog keeps a scroll position of its own), drawn directly otherwise. Returns the height
/// they took when scrolled.
fn dialog_body(ui: &mut egui::Ui, scroll: Option<&str>, max_height: f32, body: impl FnOnce(&mut egui::Ui)) -> Option<f32> {
    let Some(name) = scroll else {
        ui.data_mut(|d| d.remove_temp::<f32>(list_shrink_id()));
        body(ui);
        return None;
    };
    ui.data_mut(|d| d.insert_temp(list_applied_id(), 0.0_f32));
    let top = ui.cursor().top();
    // (an auto-sized window offers its content last frame's height; the minimum lets a dialog
    // that grows take its full height at once, and the area still shrinks to fit its content)
    let out = egui::ScrollArea::vertical()
        .id_salt(("dialog-body", name))
        .max_height(max_height)
        .min_scrolled_height(max_height)
        .auto_shrink([true, true])
        .show(ui, body);
    // what the lists that scroll on their own gave up this frame, and what they should next frame
    let applied = ui.data(|d| d.get_temp::<f32>(list_applied_id())).unwrap_or(0.0);
    let was = ui.data(|d| d.get_temp::<f32>(list_shrink_id())).unwrap_or(0.0);
    let shrink = (out.content_size.y + applied - max_height).max(0.0);
    if (shrink - was).abs() > 0.5 {
        ui.data_mut(|d| d.insert_temp(list_shrink_id(), shrink));
        ui.ctx().request_repaint();
    }
    // (the room the scroll area took: its `inner_rect` can be taller than what it allocated)
    Some(ui.min_rect().bottom() - top)
}

/// Apply a dialog's action (also used by `ui.dialog.confirm`).
/// `90` → `1 min 30 s`.
pub fn fmt_gap(v: f64) -> String {
    let v = v.round() as u64;
    match v {
        0..60 => format!("{v} s"),
        60..3600 if v.is_multiple_of(60) => format!("{} min", v / 60),
        60..3600 => format!("{} min {} s", v / 60, v % 60),
        3600..86400 if v.is_multiple_of(3600) => format!("{} h", v / 3600),
        3600..86400 => format!("{} h {} min", v / 3600, v % 3600 / 60),
        _ => crate::i18n::tr("1 day").into(),
    }
}

/// Whether a dialog stays open after its action succeeded (the SAM 3 dialog while the model
/// downloads).
pub fn keeps_open(app: &LightcraftApp, dlg: &Dialog) -> bool {
    (matches!(dlg, Dialog::SamModel { .. }) && !app.session.segmenter.installed())
        || (matches!(dlg, Dialog::SuperRes { .. }) && crate::superres::keeps_open(app))
}

/// No SAM 3 model, no download running and nowhere to download it from: installing it by hand is
/// all the dialog can offer.
fn sam_by_hand(sam: &lightcraft_engine::segment::Segmenter) -> bool {
    !sam.installed() && !sam.download_status().running && sam.mirrors().is_empty()
}

/// The model installation guide.
const SAM_HELP: &str = "https://github.com/storytold/lightcraft/blob/main/docs/ai-masks.md#getting-the-model";

/// Show the SAM 3 model folder in the file manager (created first, so there is something to show).
fn show_model_folder(app: &mut LightcraftApp, dir: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let reveal = app.services.reveal.as_mut().ok_or("not available here")?;
    reveal(&dir.to_string_lossy())
}

/// The SAM 3 dialog: what the model is, its size and licence, and the download's progress.
fn sam_model_body(app: &mut LightcraftApp, ui: &mut egui::Ui, error: Option<&str>) {
    use lightcraft_engine::segment::{LICENSE_NAME, LICENSE_URL, MODEL_BYTES};
    let t = Tokens::get(ui.ctx());
    let seg = &app.session.segmenter;
    let d = seg.download_status();
    if seg.installed() {
        ui.label(crate::i18n::tr("The SAM 3 model is installed: Object and Describe masks are ready."));
        return;
    }
    let gb = |b: u64| b as f64 / 1e9;
    ui.label(crate::i18n::tr(
        "Object and Describe masks use SAM 3, Meta's segmentation model. It isn't part of LightCraft, and everything else works without it.",
    ));
    let dir = seg.dir.as_ref().map(|d| d.display().to_string()).unwrap_or_default();
    ui.label(format!("{} {:.1} GB, {} {dir}", crate::i18n::tr("A one-time download of about"), gb(MODEL_BYTES), crate::i18n::tr("saved in")));
    ui.label(
        egui::RichText::new(format!(
            "{} {LICENSE_NAME} — {}",
            crate::i18n::tr("Licence:"),
            crate::i18n::tr("Meta's terms, not LightCraft's. Downloading it means accepting them.")
        ))
        .color(t.text_label),
    );
    let r = ui.link(crate::i18n::tr("Read the SAM License")).on_hover_text(LICENSE_URL);
    crate::widgets::register(ui.ctx(), "link:samLicense", r.rect);
    if r.clicked() {
        let _ = crate::links::open(app, LICENSE_URL);
    }
    let seg = &app.session.segmenter;
    if d.running {
        ui.add_space(4.0);
        let frac = if d.total > 0 { d.done as f64 / d.total as f64 } else { 0.0 };
        let text = format!("{:.2} / {:.2} GB · {}", gb(d.done), gb(d.total), d.file);
        let r = ui.add(egui::ProgressBar::new(frac as f32).text(text));
        crate::widgets::register(ui.ctx(), "progress:samDownload", r.rect);
        let r = ui.button(crate::i18n::tr("Cancel Download"));
        crate::widgets::register(ui.ctx(), "button:samCancel", r.rect);
        if r.clicked() {
            app.session.segmenter.cancel_download();
        }
        ui.label(egui::RichText::new(crate::i18n::tr("You can close this: the download continues, and resumes if interrupted.")).color(t.text_dim));
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
        return;
    }
    if seg.mirrors().is_empty() {
        // nothing to download from: say how to install it by hand, with the folder and the guide a click away
        let dir = seg.dir.clone();
        ui.label(
            egui::RichText::new(crate::i18n::tr(
                "This build can't download the model yet. To install it by hand, put model.safetensors, vocab.json and merges.txt from Meta's facebook/sam3 in the folder above: Object and Describe work as soon as they are there.",
            ))
            .color(t.text_dim),
        );
        ui.horizontal_wrapped(|ui| {
            if let Some(dir) = dir.filter(|_| app.services.reveal.is_some()) {
                let r = ui.button(crate::i18n::tr(crate::menus::reveal_label()));
                crate::widgets::register(ui.ctx(), "button:samFolder", r.rect);
                if r.clicked()
                    && let Err(e) = show_model_folder(app, &dir)
                {
                    app.toast(ui.ctx(), e);
                }
            }
            let r = ui.link(crate::i18n::tr("How to install the model")).on_hover_text(SAM_HELP);
            crate::widgets::register(ui.ctx(), "link:samHelp", r.rect);
            if r.clicked()
                && let Err(e) = crate::links::open(app, SAM_HELP)
            {
                app.toast(ui.ctx(), e);
            }
        });
    }
    if let Some(e) = error.map(str::to_string).or(d.error) {
        ui.label(egui::RichText::new(format!("{} {e}", crate::i18n::tr("The download didn't work:"))).color(egui::Color32::from_rgb(230, 90, 80)));
    }
}

/// Runs a command that creates an album and asks the Albums tree to open the folders down to it.
fn created_in(app: &mut LightcraftApp, command: &str, params: serde_json::Value) -> Result<serde_json::Value, String> {
    let r = app.run(command, params)?;
    app.ui.reveal_album = r.get("id").and_then(serde_json::Value::as_u64);
    Ok(r)
}

/// The albums an Album rule in smart album `editing` picks from, in the sidebar's order: the album
/// itself and any smart album that tests it (testing it back would loop) are blocked, with why.
pub(crate) fn album_entries(cat: &lightcraft_catalog::Catalog, editing: Option<u64>) -> Vec<crate::album_picker::AlbumEntry> {
    let editing = editing.map(lightcraft_catalog::AlbumId);
    crate::album_picker::entries_from(cat, |a| match editing {
        Some(e) if a.id == e => Some(crate::i18n::tr("This is the album you're editing").to_string()),
        Some(e) if cat.album_reaches(a.id, e) => Some(crate::i18n::tr("It tests this album, so testing it back would loop").to_string()),
        _ => None,
    })
}

pub fn confirm_dialog(app: &mut LightcraftApp, dlg: &Dialog) -> Result<serde_json::Value, String> {
    match dlg {
        Dialog::SuperRes { .. } => crate::superres::confirm(app),
        // (its buttons act in the body)
        Dialog::SyncChoice => Ok(serde_json::Value::Null),
        Dialog::SamModel { then, .. } => {
            if app.session.segmenter.installed() {
                // installed: start what the user was doing
                if let Some((kind, op)) = then {
                    crate::panels::masking::begin_ai(app, kind, op)?;
                }
                return Ok(serde_json::Value::Null);
            }
            let r = app.run("segment.model.download", json!({"acknowledged": true}));
            if r.is_ok() {
                app.ui.sam_downloading = true;
            }
            r
        }
        Dialog::NewAlbum { name, folder, parent } => {
            created_in(app, "album.create", json!({"name": name, "folder": folder, "addSelected": !folder, "parent": parent}))
        }
        Dialog::RenameAlbum { id, name } => app.run("album.rename", json!({"id": id, "name": name})),
        Dialog::ShareLink { id, days, originals, upload, password, .. } => {
            let mut p = json!({"id": id, "expiresDays": days, "originals": originals, "upload": upload});
            if !password.is_empty() {
                p["password"] = json!(password);
            }
            // (the link arrives with the next sync, and goes on the clipboard: `sync_ui::poll`)
            app.run("album.share", p)
        }
        Dialog::TextPrompt { value, command, params, key, .. } => {
            let mut p = params.clone();
            p[key.as_str()] = json!(value.trim());
            app.run(command, p)
        }
        Dialog::CaptureTime { mode, time, days, hours, minutes, zone } => app.run(
            "photo.setCaptureTime",
            match mode.as_str() {
                "set" => json!({"time": time}),
                "shift" => json!({"shift": *days as i64 * 86_400 + *hours as i64 * 3600 + *minutes as i64 * 60}),
                _ => json!({"hours": zone}),
            },
        ),
        Dialog::LabelNames { names, save_as } => {
            let mut m = serde_json::Map::new();
            for (l, n) in lightcraft_catalog::ColorLabel::ALL.iter().zip(names) {
                m.insert(format!("{l:?}").to_lowercase(), if n.trim().is_empty() { serde_json::Value::Null } else { json!(n.trim()) });
            }
            let r = app.run("label.setNames", json!({"names": m}));
            if r.is_ok() && !save_as.trim().is_empty() {
                return app.run("label.saveSet", json!({"name": save_as.trim()}));
            }
            r
        }
        Dialog::Rename { template, start } => app.run("photo.rename", json!({"template": template, "start": start})),
        Dialog::RenameKeyword { from, to } => app.run("keyword.rename", json!({"from": from, "to": to})),
        Dialog::KeywordTag {
            editing,
            name,
            parent,
            inside,
            synonyms,
            include_on_export,
            export_containing,
            export_synonyms,
            person,
            add_to_selected,
            reveal,
        } => {
            let mut params = json!({
                "synonyms": synonyms.split(',').map(str::trim).filter(|s| !s.is_empty()).collect::<Vec<_>>(),
                "includeOnExport": include_on_export,
                "exportContaining": export_containing,
                "exportSynonyms": export_synonyms,
                "person": person,
            });
            match editing {
                Some(keyword) => {
                    params["keyword"] = json!(keyword);
                    params["name"] = json!(name);
                    let r = app.run("keyword.edit", params);
                    if r.is_ok() {
                        let to = match keyword.rsplit_once('|') {
                            Some((parent, _)) => format!("{parent}|{}", name.trim()),
                            None => name.trim().to_string(),
                        };
                        crate::panels::keyword_list::follow(app, keyword, &to);
                    }
                    r
                }
                None => {
                    params["name"] = json!(name);
                    params["parent"] = if *inside { json!(parent) } else { serde_json::Value::Null };
                    params["addToSelected"] = json!(add_to_selected);
                    let r = app.run("keyword.create", params);
                    if *reveal && let Ok(made) = &r {
                        app.ui.reveal_keyword = made["keyword"].as_str().map(str::to_string);
                    }
                    r
                }
            }
        }
        Dialog::DeleteKeyword { keyword, .. } => app.run("keyword.delete", json!({"keyword": keyword})),
        Dialog::KeywordSet { replaces, name, slots, as_new } => {
            use lightcraft_catalog::keywords::same;
            let name = name.trim();
            if name.is_empty() {
                return Err(crate::i18n::tr("A keyword set needs a name").to_string());
            }
            if same(name, lightcraft_engine::cmd::keywords::RECENT) {
                return Err(crate::i18n::tr("Recent Keywords is built in: choose another name").to_string());
            }
            // the set it renames: none when saving as new, or when it was deleted meanwhile (then it is
            // saved anew rather than refused)
            let sets = &app.session.keyword_sets;
            let renames = replaces.as_deref().filter(|_| !as_new).filter(|old| sets.iter().any(|x| same(&x.name, old)));
            if sets.iter().any(|x| same(&x.name, name) && !renames.is_some_and(|old| same(&x.name, old))) {
                return Err(crate::i18n::tr_format!("There is a keyword set “{name}” already", name = name));
            }
            let mut params = json!({"name": name, "keywords": slots});
            match renames {
                Some(old) => params["replace"] = json!(old),
                None => params["new"] = json!(true),
            }
            app.run("keyword.saveSet", params)
        }
        Dialog::MoveKeyword { keyword, parent } => {
            let r = app.run("keyword.move", json!({"keyword": keyword, "parent": parent, "merge": true}));
            if r.is_ok() {
                let leaf = keyword.rsplit('|').next().unwrap_or(keyword);
                let to = parent.as_deref().map_or_else(|| leaf.to_string(), |p| format!("{p}|{leaf}"));
                crate::panels::keyword_list::follow(app, keyword, &to);
            }
            r
        }
        Dialog::MergeKeywords { from, into } => app.run("keyword.merge", json!({"from": from, "into": into})),
        Dialog::AutoStack { gap } => app.run("stack.auto", json!({"gap": gap})),
        Dialog::EditIn { photocraft, mode } => {
            app.ui.settings.edit_in_mode = *mode;
            let command = if *photocraft { "photo.editInPhotoCraft" } else { "photo.editInExternal" };
            app.run(command, json!({"mode": mode.param()}))
        }
        Dialog::DenoiseModel { info, accepted } => crate::panels::denoise::install(app, info, *accepted),
        Dialog::FaceModel { path, info, accepted } => crate::panels::faces::install(app, path, info, *accepted),
        Dialog::AllMetadata { .. } | Dialog::SystemInfo { .. } | Dialog::WhatsNew => Ok(serde_json::Value::Null),
        Dialog::Cull { reject_below, pick_best } => {
            let mut p = json!({"pickBest": pick_best});
            if *reject_below > 0.0 {
                p["rejectBelow"] = json!(reject_below);
            }
            app.run("photo.analyze", p)
        }
        Dialog::SmartRules { id, name, rules, parent } => {
            // refused before anything changes (not even the name)
            if let Some(problem) = rules.check_for(&app.session.catalog, id.map(lightcraft_catalog::AlbumId)).first() {
                return Err(problem.to_string());
            }
            let name = if name.trim().is_empty() { "Smart Album".to_string() } else { name.trim().to_string() };
            match id {
                // the name and the rules together: one undo step, both or neither
                Some(id) => app.run("album.setRules", json!({"id": id, "name": name, "replace": true, "rules": {"ruleSet": rules}})),
                None => created_in(app, "album.createSmart", json!({"name": name, "rules": {"ruleSet": rules}, "parent": parent})),
            }
        }
        Dialog::NewSmartAlbum { name, parent } => {
            created_in(app, "album.createSmart", json!({"name": if name.trim().is_empty() { "Smart Album" } else { name }, "parent": parent}))
        }
        Dialog::CreatePreset { name, group, groups } => app.run(
            "preset.create",
            json!({
                "name": if name.trim().is_empty() { "My Preset" } else { name },
                "group": if group.trim().is_empty() { "User Presets" } else { group },
                "groups": groups,
            }),
        ),
        Dialog::ContactSheet { options } => app.run("app.contactSheet", json!(options)),
        Dialog::CopySettings { groups } => app.run("develop.copy", json!({"groups": groups})),
        Dialog::PasteSettings { groups } => app.run("develop.paste", json!({"groups": groups})),
        Dialog::Export { opts, full_size, resize, limit_kb, dir, ids, then, .. } => {
            let mut p = super::export::dialog_params(opts, *full_size, resize, *limit_kb);
            p["dir"] = json!(dir);
            p["background"] = json!(true);
            if !ids.is_empty() {
                p["ids"] = json!(super::export::in_grid_order(app, ids));
            }
            if *then == crate::state::ExportThen::Save {
                p["sendTo"] = json!("photos");
            }
            app.run("app.export", p)
        }
        Dialog::Merge { opts } => crate::merge::start_final(app, opts),
        Dialog::Import { opts } => crate::import::start(app, opts),
        // (Import replaces the dialog with one that says how it went; Show Presets closes it)
        Dialog::PresetImport { review } => crate::preset_import::confirm(app, review),
        Dialog::ConfirmDelete { .. } => app.run("photo.delete", json!({})),
        // the catalog found: no file dialog; none: File ▸ Import Lightroom Catalog… asks for it
        Dialog::LightroomCatalog { catalog, .. } => {
            app.run("file.importLightroom", catalog.as_ref().map_or_else(|| json!({}), |c| json!({"path": c})))
        }
        Dialog::RemoveFolder { path, disk, .. } => app.run("library.removeFolder", json!({"path": path, "disk": disk})),
        d @ Dialog::SynchronizeFolder { .. } => crate::sync::confirm(app, d),
        Dialog::About | Dialog::Shortcuts | Dialog::Settings { .. } => Ok(serde_json::Value::Null),
    }
}

/// Open a one-field dialog that runs `command` with `params` + `{key: typed value}`.
pub fn prompt(app: &mut LightcraftApp, title: &str, hint: &str, value: &str, command: &str, params: serde_json::Value, key: &str) {
    app.ui.dialog =
        Some(Dialog::TextPrompt { title: title.into(), hint: hint.into(), value: value.into(), command: command.into(), params, key: key.into() });
}

// ------------------------------------------------------------------------------------------ dialog widgets

/// Two columns of settings-group checkboxes (Copy Settings, Create Preset) plus All / None.
fn group_checklist(ui: &mut egui::Ui, tag: &str, groups: &mut Vec<String>) {
    let key_of = |g: &SettingsGroup| serde_json::to_value(g).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
    ui.columns(2, |cols| {
        for (i, g) in SettingsGroup::ALL.iter().enumerate() {
            let key = key_of(g);
            let mut on = groups.contains(&key);
            let r = cols[i % 2].checkbox(&mut on, crate::i18n::tr(g.label()));
            crate::widgets::register(&r.ctx, format!("{tag}:{key}"), r.rect);
            if r.changed() {
                if on {
                    groups.push(key);
                } else {
                    groups.retain(|x| *x != key);
                }
            }
        }
    });
    ui.horizontal(|ui| {
        let all = ui.small_button(crate::i18n::tr("All"));
        crate::widgets::register(ui.ctx(), format!("{tag}:all"), all.rect);
        if all.clicked() {
            *groups = SettingsGroup::ALL.iter().map(key_of).collect();
        }
        let none = ui.small_button(crate::i18n::tr("None"));
        crate::widgets::register(ui.ctx(), format!("{tag}:none"), none.rect);
        if none.clicked() {
            groups.clear();
        }
    });
}

/// Width of the label column in dialogs.
const LABEL_W: f32 = 78.0;

/// A themed slider row editing `v`; true when it changed.
pub(super) fn num(ui: &mut egui::Ui, spec: &ControlSpec, v: &mut f64) -> bool {
    match crate::widgets::slider(ui, spec, *v, true, None).value {
        Some(n) => {
            *v = n;
            true
        }
        None => false,
    }
}

/// A labelled row (fixed label column).
pub(super) fn field<R>(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let t = Tokens::get(ui.ctx());
    if crate::is_compact(ui.ctx()) {
        // a phone (iOS forms): the label above its controls, which wrap to the page's width
        ui.add_space(4.0);
        ui.label(egui::RichText::new(crate::i18n::tr(label)).color(t.text_dim).size(13.0));
        return ui.horizontal_wrapped(add).inner;
    }
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(egui::vec2(LABEL_W, 24.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.set_min_width(LABEL_W);
            ui.label(egui::RichText::new(crate::i18n::tr(label)).color(t.text_label));
        });
        add(ui)
    })
    .inner
}

/// The rest of a [`field`] row, laid out right to left: `add` places its trailing button(s) first,
/// then a text field taking exactly the width that's left. (Subtracting a guessed button width
/// instead made the auto-sized dialog grow every frame when the real button was wider, #8.)
pub(super) fn trailing_button_row(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    let h = ui.spacing().interact_size.y.max(24.0);
    ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), h), egui::Layout::right_to_left(egui::Align::Center), add);
}

/// A labelled row of mutually exclusive choice buttons (ids `button:{id}-{index}`).
pub(super) fn choices<V: PartialEq + Copy>(ui: &mut egui::Ui, label: &str, id: &str, options: &[(V, &str)], value: &mut V) {
    if crate::is_compact(ui.ctx()) {
        // a phone: iOS's segmented control under the label
        let t = Tokens::get(ui.ctx());
        ui.add_space(4.0);
        ui.label(egui::RichText::new(crate::i18n::tr(label)).color(t.text_dim).size(13.0));
        let labels: Vec<&str> = options.iter().map(|(_, l)| *l).collect();
        let active = options.iter().position(|(v, _)| *v == *value);
        if let Some((v, _)) = crate::widgets::ios_segmented(ui, id, &labels, active).and_then(|i| options.get(i)) {
            *value = *v;
        }
        return;
    }
    field(ui, label, |ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for (i, (v, l)) in options.iter().enumerate() {
            if crate::widgets::text_button(ui, &format!("{id}-{i}"), l, *value == *v).clicked() {
                *value = *v;
            }
        }
    });
}

/// Share Link…: when the link ends, what visitors may do, and an optional password. True when Return in the
/// password field confirms.
fn share_link_body(
    ui: &mut egui::Ui,
    t: &Tokens,
    name: &str,
    days: &mut u32,
    originals: &mut bool,
    upload: &mut bool,
    password: &mut String,
) -> bool {
    ui.label(egui::RichText::new(name).strong().color(t.text));
    ui.label(
        egui::RichText::new(crate::i18n::tr(
            "Anyone with the link can see this album in a browser, without an account: its photos by day, as you have edited them, one by one or as a slideshow.",
        ))
        .color(t.text_dim),
    );
    let ends: [(u32, &str); 5] = [(0, "Never"), (1, "1 Day"), (7, "1 Week"), (30, "1 Month"), (365, "1 Year")];
    choices(ui, "Link Ends", "shareEnds", &ends, days);
    let r = crate::widgets::check(ui, originals, crate::i18n::tr("People can download the originals"));
    crate::widgets::register(ui.ctx(), "check:shareOriginals", r.rect);
    let r = crate::widgets::check(ui, upload, crate::i18n::tr("People can add photos to this album"));
    crate::widgets::register(ui.ctx(), "check:shareUpload", r.rect);
    if *upload {
        ui.label(egui::RichText::new(crate::i18n::tr("Photos people add become yours: in this album, on all your devices.")).color(t.text_dim));
    }
    ui.label(egui::RichText::new(crate::i18n::tr("Password (optional)")).color(t.text_label));
    let mut field = crate::text_field::TextField::singleline("field:sharePassword", password)
        .password(true)
        .hint(crate::i18n::tr("No password"))
        .width(f32::INFINITY);
    if crate::is_compact(ui.ctx()) {
        field = field.font(egui::FontId::proportional(17.0)).margin(egui::Margin::symmetric(12, 10));
    }
    let r = field.show(ui);
    // (the server's shortest, `SHARE_PASSWORD_MIN`)
    let short = !password.is_empty() && password.chars().count() < lightcraft_engine::cloud_sync::SHARE_PASSWORD_MIN;
    let note = if short { "A password has at least 4 characters." } else { "Visitors type it before they see anything." };
    ui.label(egui::RichText::new(crate::i18n::tr(note)).color(if short { t.caution } else { t.text_dim }));
    r.ending == Some(crate::text_field::Ending::Return) && !short
}

/// Dialogs that Return confirms with nothing focused (those with text fields confirm from the
/// field: `dialog_field`).
fn return_confirms(dlg: &Dialog) -> bool {
    matches!(dlg, Dialog::CopySettings { .. } | Dialog::PasteSettings { .. } | Dialog::PresetImport { .. } | Dialog::EditIn { .. })
}

/// A dialog's one text field: it keeps the focus (except while its own menu has it) and opens
/// with its text selected, so typing replaces it. True when Return confirms the dialog; Esc
/// cancels the dialog (`show`). Taking the focus back with a bare `TextEdit` every frame hid the
/// focus Return gives up, and Return did nothing.
fn dialog_field(ui: &mut egui::Ui, widget: &str, text: &mut String, hint: &str) -> bool {
    let mut field = crate::text_field::TextField::singleline(widget, text).hint(crate::i18n::tr(hint)).width(f32::INFINITY).select_on_focus(true);
    if crate::is_compact(ui.ctx()) {
        // a phone's: as iOS draws a text field, big enough for a finger (`widgets::text_box`)
        field = field.font(egui::FontId::proportional(17.0)).margin(egui::Margin::symmetric(12, 10));
    }
    let r = field.show(ui);
    if !r.editing {
        r.response.request_focus();
    }
    r.ending == Some(crate::text_field::Ending::Return)
}

/// The dialog part `key` is drawn this frame but wasn't the frame before: the dialog has just
/// opened (to give its first field the focus once).
fn just_opened(ui: &egui::Ui, key: &str) -> bool {
    let id = egui::Id::new(("dialog-shown", key));
    let pass = ui.ctx().cumulative_pass_nr();
    let last: Option<u64> = ui.data_mut(|d| d.get_temp(id));
    ui.data_mut(|d| d.insert_temp(id, pass));
    last.and_then(|l| l.checked_add(1)) != Some(pass)
}
