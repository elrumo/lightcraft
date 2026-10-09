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

/// Keeps a dialog's text field focused, asking only when it isn't: every request interrupts IME
/// composition, which on iOS hides the on-screen keyboard and shows it again (a request each
/// frame made it flicker in a loop).
fn keep_focus(r: &egui::Response) {
    if !r.has_focus() && !r.lost_focus() {
        r.request_focus();
        // focus moves on the next frame; nothing else may ask for one
        r.ctx.request_repaint();
    }
}

/// Where the SAM 3 download stands, which decides the dialog's buttons: `(installed, running,
/// failed, nowhere)`, "nowhere" being a build with no download location (only the manual install).
fn sam_state(app: &LightcraftApp) -> (bool, bool, bool, bool) {
    let sam = &app.session.segmenter;
    let (installed, running, failed) = (sam.installed(), sam.download_status().running, sam.download_status().error.is_some());
    (installed, running, failed, !installed && !running && sam.mirrors().is_empty())
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
        Dialog::Import { opts } if compact => return crate::i18n::tr_format!("Add {n}", n = opts.selected_paths().len()),
        Dialog::Import { opts } => {
            let n = opts.selected_paths().len();
            let verb = if opts.copy && opts.move_files { "Move" } else { "Import" };
            return crate::i18n::tr_format!("{verb} {n} Photo{}", if n == 1 { "" } else { "s" }, verb = crate::i18n::tr(verb), n = n);
        }
        Dialog::Merge { .. } => "Merge",
        Dialog::ConfirmDelete { .. } => "Delete",
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

/// Help ▸ What's New (docs/whats-new.md).
pub const WHATS_NEW: &str = include_str!("../../../../docs/whats-new.md");

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
    let title: String = match &dlg {
        Dialog::TextPrompt { title, .. } => title.as_str(),
        Dialog::NewAlbum { folder: true, .. } => "Create Folder",
        Dialog::NewAlbum { .. } => "Create Album",
        Dialog::RenameAlbum { .. } => "Rename Album",
        Dialog::Rename { .. } => "Rename Photos",
        Dialog::Import { .. } => "Import Photos",
        Dialog::LabelNames { .. } => "Edit Color Label Names",
        Dialog::CaptureTime { .. } => "Edit Capture Time",
        Dialog::RenameKeyword { .. } => "Rename Keyword",
        Dialog::MergeKeywords { .. } => "Merge Keywords",
        Dialog::NewSmartAlbum { .. } => "Create Smart Album",
        Dialog::AllMetadata { .. } => "All Metadata",
        Dialog::SystemInfo { .. } => "System Info",
        Dialog::WhatsNew => "What's New",
        Dialog::Cull { .. } => "Assisted Culling",
        Dialog::SmartRules { id: None, .. } => "New Smart Album",
        Dialog::SmartRules { .. } => "Edit Smart Album",
        Dialog::AutoStack { .. } => "Auto-Stack by Capture Time",
        Dialog::CreatePreset { .. } => "Create Preset",
        Dialog::CopySettings { .. } => "Choose Edit Settings to Copy",
        Dialog::PasteSettings { .. } => "Paste Selected Settings",
        Dialog::Export { .. } => "Export",
        Dialog::Merge { opts } => opts.title(),
        Dialog::Settings { .. } => "Settings",
        Dialog::ConfirmDelete { .. } => "Delete Photos",
        Dialog::SamModel { .. } => "Download the SAM 3 Model?",
        Dialog::SuperRes { .. } => "Super Resolution",
        Dialog::SyncChoice => "Sync This Library",
        Dialog::About => "About LightCraft",
        Dialog::Shortcuts => "Keyboard Shortcuts",
    }
    .to_string();
    let informational = matches!(dlg, Dialog::About | Dialog::Shortcuts | Dialog::Settings { .. });
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
    let default_width = match dlg {
        Dialog::Import { .. } => 760.0_f32,
        Dialog::SmartRules { .. } => 680.0,
        Dialog::AllMetadata { .. } => 620.0,
        _ => 380.0,
    };
    // (a choice made in the body of the sync dialog closes it)
    let mut choice_made = false;
    let mut body = |ui: &mut egui::Ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        match &mut dlg {
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
                    egui::RichText::new(format!("Creates {} stacks from {} of {scope}", preview["stacks"], preview["photos"])).color(t.text_dim),
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
                    egui::RichText::new(format!(
                        "Scores {scope} for focus and exposure and finds similar shots taken within seconds of each other (bursts)."
                    ))
                    .color(t.text_label),
                );
                ui.add_space(6.0);
                let r = ui.add(egui::Slider::new(reject_below, 0.0..=80.0).text("Reject below focus").step_by(1.0));
                crate::widgets::register(ui.ctx(), "field:cullReject", r.rect);
                ui.label(
                    egui::RichText::new(if *reject_below > 0.0 {
                        "Blurry photos below the score are flagged as rejects."
                    } else {
                        "0: nothing is rejected."
                    })
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
                egui::ScrollArea::vertical().max_height(460.0).auto_shrink([false, true]).show(ui, |ui| {
                    for line in WHATS_NEW.lines() {
                        let l = line.trim_end();
                        if let Some(h) = l.strip_prefix("### ") {
                            ui.add_space(6.0);
                            ui.label(egui::RichText::new(h).font(t.semibold(12.5)).color(t.text));
                        } else if let Some(h) = l.strip_prefix("## ") {
                            ui.add_space(8.0);
                            ui.label(egui::RichText::new(h).font(t.semibold(14.0)).color(t.text));
                        } else if l.starts_with("# ") || l.is_empty() {
                        } else if let Some(b) = l.strip_prefix("- ") {
                            ui.label(egui::RichText::new(format!("•  {}", b.replace('`', ""))).color(t.text_label));
                        } else {
                            ui.label(egui::RichText::new(l.trim().replace('`', "")).color(t.text_label));
                        }
                    }
                });
            }
            Dialog::SystemInfo { rows } => {
                egui::Grid::new("sysinfo").num_columns(2).spacing([16.0, 4.0]).striped(true).show(ui, |ui| {
                    for (k, v) in rows.iter() {
                        ui.label(egui::RichText::new(k).color(t.text_dim));
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
                let r = ui.add(crate::widgets::touch_field(
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
                egui::ScrollArea::vertical().max_height(440.0).auto_shrink([false, true]).show(ui, |ui| {
                    if groups.is_empty() {
                        ui.label(egui::RichText::new(rows["note"].as_str().unwrap_or("No metadata found")).color(t.text_dim));
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
            Dialog::SmartRules { name, rules, .. } => {
                let r = ui.add(crate::widgets::touch_field(
                    ui,
                    egui::TextEdit::singleline(name).hint_text(crate::i18n::tr("Name")).desired_width(f32::INFINITY),
                ));
                crate::widgets::register(ui.ctx(), "field:smartName", r.rect);
                ui.add_space(6.0);
                egui::ScrollArea::vertical().max_height(360.0).auto_shrink([false, true]).show(ui, |ui| {
                    crate::panels::rules_editor::edit(ui, rules, "rules", 0);
                });
                let problems = rules.problems();
                let f = lightcraft_catalog::Filter { rule_set: Some(rules.clone()), ..Default::default() };
                let n = if problems.is_empty() { app.session.catalog.query(&f, &Default::default()).len() } else { 0 };
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(match problems.first() {
                        Some(p) => p.clone(),
                        None => crate::i18n::tr_format!(
                            "{n} photo{} match · updates automatically as photos change",
                            if n == 1 { "" } else { "s" },
                            n = n
                        ),
                    })
                    .color(t.text_dim),
                );
            }
            Dialog::NewSmartAlbum { name } => {
                let r = ui.add(crate::widgets::touch_field(
                    ui,
                    egui::TextEdit::singleline(name).hint_text(crate::i18n::tr("Name")).desired_width(f32::INFINITY),
                ));
                keep_focus(&r);
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    confirm = true;
                }
                let rules = app.session.view_rules();
                let n = app.session.catalog.query(&rules, &Default::default()).len();
                ui.label(egui::RichText::new(crate::i18n::tr_format!("Matches: {}", rules.describe())).color(t.text_label));
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
                            let r = ui.add(crate::widgets::touch_field(
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
                        let te = ui.add(crate::widgets::touch_field(
                            ui,
                            egui::TextEdit::singleline(&mut names[i]).hint_text(colour.as_str()).desired_width(f32::INFINITY),
                        ));
                        crate::widgets::register(ui.ctx(), format!("field:labelName-{}", colour.to_lowercase()), te.rect);
                    });
                }
                field(ui, "Save as set", |ui| {
                    let te = ui.add(crate::widgets::touch_field(
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
                    let w = (ui.available_width() - 50.0).max(80.0);
                    let r = ui.add(crate::widgets::touch_field(
                        ui,
                        egui::TextEdit::singleline(template).id(template_id).hint_text("{name}").desired_width(w),
                    ));
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
                        ui.label(egui::RichText::new("Checking names…").color(t.text_dim));
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
                let r = ui.add(crate::widgets::touch_field(
                    ui,
                    egui::TextEdit::singleline(to).hint_text(crate::i18n::tr("New name")).desired_width(f32::INFINITY),
                ));
                crate::widgets::register(ui.ctx(), "field:keywordName", r.rect);
                keep_focus(&r);
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    confirm = true;
                }
                ui.label(
                        egui::RichText::new(format!(
                            "Renames “{from}” on {n} photo{} (keywords below it too). Use | for levels, e.g. Travel|Italy. An existing name merges the two.",
                            if n == 1 { "" } else { "s" }
                        ))
                        .color(t.text_dim),
                    );
            }
            Dialog::MergeKeywords { from, into } => {
                ui.label(
                    egui::RichText::new(format!("Replace {} with:", from.iter().map(|f| format!("“{f}”")).collect::<Vec<_>>().join(", ")))
                        .color(t.text_label),
                );
                let r = ui.add(crate::widgets::touch_field(
                    ui,
                    egui::TextEdit::singleline(into).hint_text(crate::i18n::tr("Keyword")).desired_width(f32::INFINITY),
                ));
                crate::widgets::register(ui.ctx(), "field:keywordInto", r.rect);
                keep_focus(&r);
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
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
                let r =
                    ui.add(crate::widgets::touch_field(ui, egui::TextEdit::singleline(value).hint_text(hint.as_str()).desired_width(f32::INFINITY)));
                keep_focus(&r);
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    confirm = true;
                }
            }
            Dialog::NewAlbum { name, .. } | Dialog::RenameAlbum { name, .. } => {
                let r = ui.add(crate::widgets::touch_field(
                    ui,
                    egui::TextEdit::singleline(name).hint_text(crate::i18n::tr("Name")).desired_width(f32::INFINITY),
                ));
                keep_focus(&r);
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    confirm = true;
                }
            }
            Dialog::CreatePreset { name, group, groups } => {
                ui.add(crate::widgets::touch_field(
                    ui,
                    egui::TextEdit::singleline(name).hint_text(crate::i18n::tr("Preset name")).desired_width(f32::INFINITY),
                ));
                ui.add(crate::widgets::touch_field(
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
            Dialog::Export { opts, full_size, resize, preset_name, limit_kb, dir, .. } => {
                super::export::form(app, ui, super::export::Form { opts, full_size, resize, preset_name, limit_kb, dir }, super::export::Part::All);
            }
            Dialog::Merge { opts } => crate::merge::body(app, ui, opts),
            Dialog::Import { opts } if app.compact => crate::import::mobile_body(app, ui, opts),
            Dialog::Import { opts } => crate::import::body(app, ui, opts),
            Dialog::Settings { tab } => crate::panels::settings::body(app, ui, tab),
            Dialog::SamModel { error, .. } => sam_model_body(app, ui, error.as_deref()),
            Dialog::SuperRes { error } => crate::superres::body(app, ui, error.as_deref()),
            Dialog::SyncChoice => choice_made |= super::sync_choice::body(app, ui),
            Dialog::ConfirmDelete { count } => {
                let what = if *count == 1 {
                    crate::i18n::tr("this photo").to_string()
                } else {
                    crate::i18n::tr_format!("these {count} photos", count = count)
                };
                ui.label(crate::i18n::tr_format!("Move {what} to Recently Deleted?", what = what));
                ui.label(egui::RichText::new(crate::i18n::tr("They can be restored from Recently Deleted until it is emptied.")).color(t.text_dim));
            }
            Dialog::About => {
                ui.label(egui::RichText::new("LightCraft").font(t.semibold(20.0)).color(t.text));
                ui.label(crate::i18n::tr_format!("Version {} — a clean-room, pure-Rust photo library and raw developer.", env!("CARGO_PKG_VERSION")));
                ui.label(format!("MIT OR Apache-2.0. Fonts: {} (OFL). Icons: original.", crate::theme::font_credits()));
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
            Dialog::Shortcuts => {
                egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                    egui::Grid::new("shortcuts").striped(true).show(ui, |ui| {
                        for (id, label, sc, _) in crate::menus::ui_commands() {
                            if let Some(sc) = sc {
                                ui.label(crate::i18n::tr(label));
                                ui.label(*sc);
                                ui.label(egui::RichText::new(*id).color(t.text_dim));
                                ui.end_row();
                            }
                        }
                        for c in lightcraft_engine::command_specs() {
                            if let Some(sc) = c.shortcut {
                                ui.label(crate::i18n::tr(c.label));
                                ui.label(sc);
                                ui.label(egui::RichText::new(c.id).color(t.text_dim));
                                ui.end_row();
                            }
                        }
                        for (sc, id, _) in crate::shortcuts::ALIASES {
                            let label = crate::menus::ui_commands()
                                .find(|c| c.0 == *id)
                                .map(|c| c.1)
                                .or_else(|| lightcraft_engine::find_command(id).map(|c| c.label))
                                .unwrap_or(id);
                            ui.label(crate::i18n::tr(label));
                            ui.label(*sc);
                            ui.label(egui::RichText::new(*id).color(t.text_dim));
                            ui.end_row();
                        }
                    });
                });
            }
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
        let mut action = (!informational && !ok.is_empty()).then_some((ok.as_str(), true));
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
        let shown = egui::Window::new(crate::i18n::tr(&title))
            .id(egui::Id::new("lightcraft-dialog"))
            .collapsible(false)
            .resizable(false)
            .frame(frame)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .default_width(default_width)
            .show(ctx, |ui| {
                body(ui);
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if !informational {
                        let r = ui.button(crate::i18n::tr(cancel));
                        crate::widgets::register(ui.ctx(), "button:dialogCancel", r.rect);
                        cancel_tapped = r.clicked();
                    }
                    if !ok.is_empty() {
                        let r = ui.button(ok.as_str());
                        crate::widgets::register(ui.ctx(), "button:dialogOk", r.rect);
                        ok_tapped = r.clicked();
                    }
                });
            });
        if let Some(w) = shown {
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
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        close = true;
    }
    if confirm {
        match confirm_dialog(app, &dlg) {
            // the import review stays open on an error (e.g. an unusable folder template)
            Err(e) if matches!(dlg, Dialog::Import { .. }) => app.toast(ctx, e),
            // a phone's export stays open on an error: the options may be what to change
            Err(e) if compact && matches!(dlg, Dialog::Export { .. }) => app.toast_error(ctx, e),
            // the SAM 3 dialog stays open to show the download (or why it can't start)
            Err(e) if matches!(dlg, Dialog::SamModel { .. } | Dialog::SuperRes { .. }) => match &mut dlg {
                Dialog::SamModel { error, .. } | Dialog::SuperRes { error } => *error = Some(e),
                _ => {}
            },
            Ok(_) if keeps_open(app, &dlg) => match &mut dlg {
                Dialog::SamModel { error, .. } | Dialog::SuperRes { error } => *error = None,
                _ => {}
            },
            _ => close = true,
        }
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
        _ => "1 day".into(),
    }
}

/// Whether a dialog stays open after its action succeeded (the SAM 3 dialog while the model
/// downloads).
pub fn keeps_open(app: &LightcraftApp, dlg: &Dialog) -> bool {
    (matches!(dlg, Dialog::SamModel { .. }) && !app.session.segmenter.installed())
        || (matches!(dlg, Dialog::SuperRes { .. }) && crate::superres::keeps_open(app))
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
        ui.label(
            egui::RichText::new(crate::i18n::tr(
                "This build has no download location for the model yet: put the files in the folder above yourself (see docs/ai-masks.md).",
            ))
            .color(t.text_dim),
        );
    }
    if let Some(e) = error.map(str::to_string).or(d.error) {
        ui.label(egui::RichText::new(format!("{} {e}", crate::i18n::tr("The download didn't work:"))).color(egui::Color32::from_rgb(230, 90, 80)));
    }
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
        Dialog::NewAlbum { name, folder } => app.run("album.create", json!({"name": name, "folder": folder, "addSelected": !folder})),
        Dialog::RenameAlbum { id, name } => app.run("album.rename", json!({"id": id, "name": name})),
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
        Dialog::MergeKeywords { from, into } => app.run("keyword.merge", json!({"from": from, "into": into})),
        Dialog::AutoStack { gap } => app.run("stack.auto", json!({"gap": gap})),
        Dialog::AllMetadata { .. } | Dialog::SystemInfo { .. } | Dialog::WhatsNew => Ok(serde_json::Value::Null),
        Dialog::Cull { reject_below, pick_best } => {
            let mut p = json!({"pickBest": pick_best});
            if *reject_below > 0.0 {
                p["rejectBelow"] = json!(reject_below);
            }
            app.run("photo.analyze", p)
        }
        Dialog::SmartRules { id, name, rules } => {
            let name = if name.trim().is_empty() { "Smart Album".to_string() } else { name.trim().to_string() };
            match id {
                Some(id) => {
                    if app.session.catalog.album(lightcraft_catalog::AlbumId(*id)).is_some_and(|a| a.name != name) {
                        app.run("album.rename", json!({"id": id, "name": name}))?;
                    }
                    app.run("album.setRules", json!({"id": id, "replace": true, "rules": {"ruleSet": rules}}))
                }
                None => app.run("album.createSmart", json!({"name": name, "rules": {"ruleSet": rules}})),
            }
        }
        Dialog::NewSmartAlbum { name } => app.run("album.createSmart", json!({"name": if name.trim().is_empty() { "Smart Album" } else { name }})),
        Dialog::CreatePreset { name, group, groups } => app.run(
            "preset.create",
            json!({
                "name": if name.trim().is_empty() { "My Preset" } else { name },
                "group": if group.trim().is_empty() { "User Presets" } else { group },
                "groups": groups,
            }),
        ),
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
        Dialog::ConfirmDelete { .. } => app.run("photo.delete", json!({})),
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
            let r = cols[i % 2].checkbox(&mut on, g.label());
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
        let all = ui.small_button("All");
        crate::widgets::register(ui.ctx(), format!("{tag}:all"), all.rect);
        if all.clicked() {
            *groups = SettingsGroup::ALL.iter().map(key_of).collect();
        }
        let none = ui.small_button("None");
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
