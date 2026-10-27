//! Modal dialogs (new album, rename, create preset, choose settings to copy, export, about, shortcuts).

use lightcraft_develop::{ControlSpec, SettingsGroup};
use serde_json::json;

use crate::LightcraftApp;
use crate::state::{Dialog, HiddenPrompt};
use crate::theme::Tokens;

mod confirm;
mod cull;
mod export;
mod frame;
mod rename;
mod sam_model;

pub(crate) use confirm::album_entries;
pub use confirm::confirm_dialog;
pub use cull::{MAX_BURST_GAP, clamp_cull_burst, cull_params, fmt_gap, parse_gap};
use export::group_checklist;
pub use frame::{ConfirmStyle, confirm_style, list_height, prompt};
use frame::{
    DIALOG_CHROME_GUESS, DIALOG_MIN_BODY, DIALOG_SCREEN_MARGIN, choices, clicked_backdrop, dialog_body, dialog_field, field, just_opened, only_shows,
    return_confirms,
};
pub(crate) use rename::rename_preview;
use rename::{RENAME_PREVIEW_ROWS, person_rename_plan};
pub(crate) use sam_model::show_model_folder;
use sam_model::{sam_by_hand, sam_model_body};
use slideshow_export::slideshow_export_body;

pub mod credits;
pub mod faces;
pub mod import;
pub mod keymap;
pub mod lightroom_destination;
pub mod merge;
pub mod print_view;
pub mod rules_editor;
pub mod settings;
pub mod slideshow_export;
pub mod updates_ui;

/// About dialog tabs: (widget id suffix, label). The credits come from `crate::credits`.
pub const ABOUT_TABS: &[(&str, &str)] = &[("about", "About"), ("contributors", "Contributors"), ("models", "Models")];

/// Help ▸ What's New (docs/whats-new.md).
pub use lightcraft_engine::about::WHATS_NEW;


/// The import review and Settings keep window ids of their own, so the size each is given
/// doesn't carry over to the other dialogs.
fn window_id(dialog: &Dialog) -> egui::Id {
    egui::Id::unique(match dialog {
        Dialog::Import { .. } => "lightcraft-import-dialog",
        Dialog::Print => "lightcraft-print-dialog",
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
        Dialog::LightroomResult { .. } => "Close",
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
            // (the host's temporary copies, a camera's included, are moved: the originals stay)
            let verb = if opts.copy && opts.move_files && !opts.staged { "Move" } else { "Import" };
            return crate::i18n::tr_format!("{verb} {n} Photo{}", if n == 1 { "" } else { "s" }, verb = crate::i18n::tr(verb), n = n);
        }
        Dialog::LightroomResult { first: Some(_), .. } => "Show in Library",
        Dialog::ContactSheet { .. } => "Choose PDF…",
        Dialog::SlideshowExport { .. } => "Export…",
        Dialog::Print => "Print to PDF…",
        Dialog::Merge { .. } => "Merge",
        Dialog::DenoiseModel { info, .. } if info["download"].is_string() => "Accept & Download",
        Dialog::DenoiseModel { .. } => "Install",
        Dialog::FaceModel { info, .. } if !informational && info["download"].is_string() => "Download",
        Dialog::FaceModel { .. } if !informational => "Install",
        Dialog::ConfirmDelete { .. } | Dialog::DeleteKeyword { .. } | Dialog::DeleteEffects { .. } | Dialog::DeletePresetGroup { .. } => "Delete",
        Dialog::SaveEffect { .. } => "Create",
        Dialog::MovePhotos { .. } => "Move",
        // (not "Merge": that is photo merging, HDR / panorama, in some languages)
        Dialog::MoveKeyword { .. } => "Merge Keywords",
        Dialog::KeywordSet { .. } => "Save",
        Dialog::KeywordTag { editing: None, .. } => "Create",
        Dialog::KeywordTag { .. } => "Save",
        Dialog::RemoveFolder { .. } => "Remove",
        Dialog::EditIn { .. } => "Edit",
        Dialog::ShareLink { .. } => "Create Link",
        Dialog::HiddenPassword { mode: HiddenPrompt::Unlock, .. } => "Unlock",
        Dialog::HiddenPassword { mode: HiddenPrompt::Create, .. } => "Set Password",
        Dialog::HiddenPassword { mode: HiddenPrompt::Change, .. } => "Save",
        Dialog::LightroomCatalog { .. } => "Import Lightroom Catalog…",
        d @ Dialog::LightroomDestination { .. } => crate::lightroom_destination::ok_label(d),
        // the scan need not be waited for
        Dialog::SynchronizeFolder { counts: None, .. } => "Continue in Background",
        Dialog::SynchronizeFolder { .. } if informational => "Close",
        Dialog::SynchronizeFolder { .. } => "Synchronize",
        Dialog::ReportBug { .. } => "Create Report",
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
        Dialog::RenamePerson { from, to, .. } if person_rename_plan(&app.session.catalog, from, to).needs_merge => "Merge",
        Dialog::Rename { .. } | Dialog::RenameAlbum { .. } | Dialog::RenameKeyword { .. } | Dialog::RenamePerson { .. } if compact => "Rename",
        _ if informational => "Close",
        _ if compact => "Done",
        _ => "OK",
    };
    crate::i18n::tr(label).to_string()
}

/// Draw the open dialog. `blocked` is true when a quit prompt or settings notice was on screen at
/// the start of this frame: the dialog is still drawn (its state is preserved), but none of its raw
/// keyboard/backdrop handling may act. The caller freezes `blocked` for the whole frame so the
/// click or key that dismisses the prompt cannot also reach the dialog drawn after it. The
/// blocking library-problem/launch-list window gets the same raw-input treatment while it is
/// actually on screen over this dialog; ordinary dialogs without that window keep their normal
/// Escape and outside-click behaviour.
pub fn show(app: &mut LightcraftApp, ctx: &egui::Context, blocked: bool) {
    // a phone's page slides away after the dialog closes, drawn from the dialog as it was
    if app.ui.dialog.is_some() {
        app.ui.dialog_leaving = None;
    }
    let leaving = app.ui.dialog.is_none() && app.compact;
    let Some(mut dlg) = app.ui.dialog.clone().or_else(|| app.ui.dialog_leaving.clone().filter(|_| leaving)) else {
        app.ui.dialog_leaving = None;
        crate::panels::mobile::hidden(ctx, "dialog");
        crate::panels::mobile::hidden(ctx, "dialogOptions");
        crate::panels::mobile::hidden(ctx, "dialogMore");
        crate::panels::alert::hidden(ctx, "dialog");
        return;
    };
    let at_start = dlg.clone();
    let library_blocks = crate::panels::library_problem::blocks_dialog_input(app);
    let raw_blocked = blocked || library_blocks;
    // Esc with a popup open in the dialog (a dropdown, a date picker's calendar) closes the popup
    // only: claimed before the dialog is drawn, since the popup is gone once it has handled the key
    if !raw_blocked && ctx.input(|i| i.key_pressed(egui::Key::Escape)) && egui::Popup::is_any_open(ctx) {
        crate::widgets::take_escape(ctx);
    }
    let t = Tokens::get(ctx);
    let screen = ctx.content_rect();
    // The backdrop is an area below the dialog window (a bare `Middle` layer painter would be
    // painted after every area — i.e. over the dialog too). A phone's page dims behind itself.
    if !app.compact {
        egui::Area::new(egui::Id::unique("dialog-dim")).order(egui::Order::Middle).fixed_pos(screen.min).interactable(false).show(ctx, |ui| {
            ui.painter().rect_filled(screen, 0.0, egui::Color32::from_black_alpha(140));
        });
    }
    let mut close = false;
    let mut confirm = false;
    // Return presses OK in a dialog without a text field of its own (Lightroom's default button);
    // a widget that has the keyboard focus (a text field, a button or checkbox reached with Tab)
    // takes Return itself. Consumed before the dialog is drawn, so nothing else sees it too.
    if !raw_blocked
        && return_confirms(&dlg)
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
        Dialog::HiddenPassword { mode: HiddenPrompt::Unlock, .. } => "Hidden",
        Dialog::HiddenPassword { mode: HiddenPrompt::Create, .. } => "Set a Password for Hidden",
        Dialog::HiddenPassword { mode: HiddenPrompt::Change, .. } => "Change Hidden Password",
        Dialog::Rename { .. } => "Rename Photos",
        Dialog::Import { .. } => "Import Photos",
        Dialog::PresetImport { .. } => "Import Presets",
        Dialog::LightroomResult { .. } => "Lightroom Catalog Import",
        Dialog::LabelNames { .. } => "Edit Color Label Names",
        Dialog::CaptureTime { .. } => "Edit Capture Time",
        Dialog::RenameKeyword { .. } => "Rename Keyword",
        Dialog::RenamePerson { .. } => "Rename Person",
        Dialog::KeywordTag { editing: None, .. } => "Create Keyword Tag",
        Dialog::KeywordTag { .. } => "Edit Keyword Tag",
        Dialog::DeleteKeyword { .. } => "Delete Keyword",
        Dialog::MoveKeyword { .. } => "Merge Keywords",
        Dialog::KeywordSet { .. } => "Edit Keyword Set",
        Dialog::MergeKeywords { .. } => "Merge Keywords",
        Dialog::NewSmartAlbum { .. } => "Create Smart Album",
        Dialog::AllMetadata { .. } => "All Metadata",
        Dialog::SystemInfo { .. } => "System Info",
        Dialog::ReportBug { .. } => "Report a Bug",
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
        Dialog::SlideshowExport { .. } => "Export Slideshow",
        Dialog::Print => "Print",
        Dialog::Merge { opts } => crate::merge::title(opts),
        Dialog::Settings { .. } => "Settings",
        Dialog::ConfirmDelete { .. } => "Delete Photos",
        Dialog::MovePhotos { .. } => "Move Photos",
        Dialog::LightroomCatalog { .. } => "Lightroom Classic Catalog",
        Dialog::LightroomDestination { .. } => "Import Lightroom Catalog",
        Dialog::SaveEffect { .. } => "New Effect Preset",
        Dialog::ManageEffects { .. } => "Manage Effect Presets",
        Dialog::DeleteEffects { names } if names.len() == 1 => "Delete Effect Preset",
        Dialog::DeleteEffects { .. } => "Delete Effect Presets",
        Dialog::DeletePresetGroup { .. } => "Delete Preset Group",
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
        Dialog::ViewOptions => "View Options",
    }
    .to_string();
    // a model file LightCraft cannot use has nothing to confirm, nor has a dialog that only shows
    // something (one Close, no Cancel / OK)
    let unusable_model = matches!(&dlg, Dialog::DenoiseModel { info, .. } | Dialog::FaceModel { info, .. } if info["kind"] == "unsupported");
    let informational = unusable_model
        || matches!(&dlg, Dialog::PresetImport { review } if crate::preset_import::informational(review))
        || matches!(dlg, Dialog::LightroomResult { first: None, .. })
        || only_shows(&dlg);
    // installing waits for the licence to be accepted (and Synchronize a moment after its scan ended); smart-album
    // rules that can't mean anything wait to be fixed
    // (a person's rename waits for a name that can be applied, and for the say-so to combine two people)
    let rename_blocked = match &dlg {
        Dialog::RenamePerson { from, to, merge } => {
            let plan = person_rename_plan(&app.session.catalog, from, to);
            plan.block.is_some() || (plan.needs_merge && !*merge)
        }
        _ => false,
    };
    let can_confirm = !rename_blocked
        && (informational
            || !matches!(&dlg, Dialog::DenoiseModel { accepted: false, .. } | Dialog::FaceModel { accepted: false, .. })
            // (Synchronize, just after its button stopped being Continue in Background)
            && !(matches!(&dlg, Dialog::SynchronizeFolder { counts: Some(_), .. }) && !crate::sync::settled(app, ctx))
            && !matches!(&dlg, Dialog::SmartRules { id, rules, .. } if !rules.check_for(&app.session.catalog, id.map(lightcraft_catalog::AlbumId)).is_empty())
            // (a link's password, if there is one, is long enough)
            && !matches!(&dlg, Dialog::ShareLink { password, .. } if !password.is_empty() && password.chars().count() < lightcraft_engine::cloud_sync::SHARE_PASSWORD_MIN)
            && !matches!(&dlg, Dialog::HiddenPassword { mode, password, confirm, .. } if !hidden_password_ok(*mode, password, confirm)));
    // (taken before `body` borrows the dialog)
    let confirm_delete = if let Dialog::ConfirmDelete { count } = &dlg { Some(*count) } else { None };
    let ok = ok_label(app, &dlg, informational);
    let cancel = cancel_label(app, &dlg, informational);
    // whether the confirm takes the accent's fill: it makes or brings in something, it doesn't delete
    // or remove (never the eye-catching default) and the dialog isn't only showing something
    let primary = !informational && can_confirm && confirm_style(&dlg) == ConfirmStyle::Primary;
    // (each kind of dialog its own buttons, told apart by this)
    let kind = std::mem::discriminant(&dlg);
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
    // The Print dialog sizes itself to the window and scrolls its settings column on its own.
    let print = matches!(dlg, Dialog::Print);
    let window_id = window_id(&dlg);
    let default_width = match dlg {
        Dialog::Import { .. } => crate::import::DIALOG_SIZE[0],
        Dialog::SmartRules { .. } => 680.0,
        Dialog::PresetImport { .. } => 520.0,
        Dialog::AllMetadata { .. } => 620.0,
        Dialog::Print => 1100.0,
        Dialog::FaceModel { .. } => 460.0,
        Dialog::ReportBug { .. } => 560.0,
        Dialog::LightroomCatalog { .. } => 440.0,
        Dialog::LightroomDestination { .. } => 480.0,
        Dialog::ManageEffects { .. } => 560.0,
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
                    crate::widgets::register(ui, format!("radio:editIn-{}", m.param()), r.rect);
                    ui.indent(("editIn", m.param()), |ui| ui.label(egui::RichText::new(crate::i18n::tr(what)).color(t.text_dim)));
                }
            }
            Dialog::AutoStack { gap } => {
                ui.label(egui::RichText::new(crate::i18n::tr("Stack photos taken within this time of each other:")).color(t.text_label));
                let r = ui.add(
                    egui::Slider::new(gap, 0.0..=86400.0)
                        .logarithmic(true)
                        .smallest_positive(1.0)
                        .custom_formatter(|v, _| crate::panels::dialogs::fmt_gap(v))
                        .custom_parser(crate::panels::dialogs::parse_gap),
                );
                crate::widgets::named(r, crate::i18n::tr("Stack photos taken within this time of each other:"));
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
            Dialog::Cull { reject_below, pick_best, burst_gap, similarity } => {
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
                crate::widgets::register(ui, "field:cullReject", r.rect);
                ui.label(
                    egui::RichText::new(crate::i18n::tr(if *reject_below > 0.0 {
                        "Blurry photos below the score are flagged as rejects."
                    } else {
                        "0: nothing is rejected."
                    }))
                    .color(t.text_dim),
                );
                let r = crate::widgets::check(ui, pick_best, crate::i18n::tr("Pick the sharpest photo of each burst"));
                crate::widgets::register(ui, "check:cullPick", r.rect);
                ui.add_space(6.0);
                // the sliders drag over the usual range; their value fields take whatever
                // `photo.analyze` accepts (a gap of 0 s to a day, a similarity of 0 to 1)
                let r = ui.add(
                    egui::Slider::new(burst_gap, 1.0..=600.0)
                        .logarithmic(true)
                        .smallest_positive(1.0)
                        .clamping(egui::SliderClamping::Never)
                        .text(crate::i18n::tr("Burst: shots at most this far apart"))
                        .custom_formatter(|v, _| crate::panels::dialogs::fmt_gap(v))
                        .custom_parser(crate::panels::dialogs::parse_gap),
                );
                crate::widgets::register(ui, "field:cullGap", r.rect);
                let r = ui.add(
                    egui::Slider::new(similarity, 0.70..=0.99)
                        .clamping(egui::SliderClamping::Never)
                        .text(crate::i18n::tr("Burst: how alike (loose … strict)"))
                        // a typed 0.995 is kept (whole hundredths would round it to 1.00)
                        .step_by(0.001)
                        .min_decimals(2)
                        .max_decimals(3),
                );
                crate::widgets::register(ui, "field:cullSimilar", r.rect);
                clamp_cull_burst(burst_gap, similarity);
                ui.label(
                    egui::RichText::new(crate::i18n::tr("Lower it when a panning or moving burst splits into several groups.")).color(t.text_dim),
                );
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
            Dialog::LightroomResult { summary, library, details, .. } => {
                ui.label(summary.as_str());
                ui.add_space(8.0);
                if let Some(library) = library {
                    ui.label(crate::i18n::tr("Destination library"));
                    ui.add(egui::Label::new(library.as_str()).wrap().selectable(true));
                    ui.label(crate::i18n::tr("Lightroom catalogs import into the open library; no separate catalog folder is created."));
                } else {
                    ui.label(crate::i18n::tr("These photos are only in memory. Open a library on disk and import the catalog again to save them."));
                }
                for detail in details.iter() {
                    ui.add(egui::Label::new(detail).wrap().selectable(true));
                }
            }
            Dialog::ReportBug { opts, rows, logs } => crate::bug_report::dialog_body(ui, &t, opts, rows, logs),
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
                let r = crate::widgets::named(
                    ui.add(crate::widgets::text_box(
                        ui,
                        egui::TextEdit::singleline(search).hint_text(crate::i18n::tr("Filter fields")).desired_width(f32::INFINITY),
                    )),
                    crate::i18n::tr("Filter fields"),
                );
                crate::widgets::register(ui, "field:metadataSearch", r.rect);
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
                let r = crate::widgets::named(
                    ui.add(crate::widgets::text_box(
                        ui,
                        egui::TextEdit::singleline(name).hint_text(crate::i18n::tr("Name")).desired_width(f32::INFINITY),
                    )),
                    crate::i18n::tr("Name"),
                );
                crate::widgets::register(ui, "field:smartName", r.rect);
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
                // counted when the library or the rules change (or, for "in the last…" rules, the minute), not every frame
                let now = (app.session.clock)();
                let minute = if rules.depends_on_now() { now.get(..16).unwrap_or(&now) } else { "" };
                let n = app.caches.dialog_count(&app.session.catalog, ("smartAlbum", format!("{rules:?}"), minute), || {
                    lightcraft_catalog::rules::set_now(Some(now.clone()));
                    app.session.catalog.query(&rules, &Default::default()).len()
                });
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
                let n = app.session.selected().len();
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
                            crate::widgets::register(ui, "field:captureTime", r.rect);
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
                    // the photo's own time is the problem (a Lightroom catalog's capture time
                    // is copied verbatim), not what was typed
                    let unreadable = lightcraft_catalog::dates::shift_iso(&cur, 0).is_none();
                    let after = match (mode.as_str(), unreadable && n == 1) {
                        // alone, it simply gets the time typed (photo.setCaptureTime repairs it)
                        ("set", true) => lightcraft_catalog::dates::normalize_iso(time),
                        _ => delta.and_then(|d| lightcraft_catalog::dates::shift_iso(&cur, d)),
                    };
                    ui.label(
                        egui::RichText::new(match after {
                            Some(a) => format!("{name}: {} → {}", cur.replace('T', " "), a.replace('T', " ")),
                            None if unreadable && !(mode == "set" && n == 1) => {
                                crate::i18n::tr("The active photo's capture time can't be read").to_string()
                            }
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
                        let te = crate::widgets::named(
                            ui.add(crate::widgets::text_box(
                                ui,
                                egui::TextEdit::singleline(&mut names[i]).hint_text(colour.as_str()).desired_width(f32::INFINITY),
                            )),
                            crate::i18n::tr(&colour),
                        );
                        crate::widgets::register(ui, format!("field:labelName-{}", colour.to_lowercase()), te.rect);
                    });
                }
                field(ui, "Save as set", |ui| {
                    let te = crate::widgets::named(
                        ui.add(crate::widgets::text_box(
                            ui,
                            egui::TextEdit::singleline(save_as).hint_text(crate::i18n::tr("Optional name")).desired_width(f32::INFINITY),
                        )),
                        crate::i18n::tr("Save as set"),
                    );
                    crate::widgets::register(ui, "field:labelSetName", te.rect);
                });
                ui.label(
                    egui::RichText::new(crate::i18n::tr(
                        "Names appear in the label menu, the filter bar and the Info panel, and are written to XMP. Empty = the colour's name.",
                    ))
                    .color(t.text_dim),
                );
            }
            Dialog::Rename { template, start } => {
                let n = app.session.selected().len();
                ui.label(egui::RichText::new(crate::i18n::tr_format!("{n} photo{}", if n == 1 { "" } else { "s" }, n = n)).color(t.text_dim));
                let template_id = egui::Id::unique("rename-template");
                let tags_open = field(ui, "Template", |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    let w = crate::import::tag_field_width(ui);
                    let r = crate::widgets::named(
                        ui.add(crate::widgets::text_box(
                            ui,
                            egui::TextEdit::singleline(template).id(template_id).hint_text("{name}").desired_width(w),
                        )),
                        crate::i18n::tr("Template"),
                    );
                    crate::widgets::register(ui, "field:renameTemplate", r.rect);
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
                                let (arrow, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
                                crate::icons::paint(ui.painter(), arrow, crate::icons::Icon::ChevronRight, t.text_dim);
                                ui.label(egui::RichText::new(&pl.to).color(t.text));
                                ui.end_row();
                            }
                        });
                    }
                    None => {
                        ui.horizontal(|ui| {
                            crate::progress::spinner(ui, 12.0);
                            ui.label(egui::RichText::new(crate::i18n::tr("Checking names…")).color(t.text_dim));
                        });
                    }
                }
                let more = total.saturating_sub(RENAME_PREVIEW_ROWS);
                if more > 0 {
                    ui.label(egui::RichText::new(crate::i18n::tr_format!("… and {more} more", more = more)).color(t.text_dim));
                }
            }
            Dialog::RenamePerson { from, to, merge } => {
                let submitted = dialog_field(ui, "field:personName", to, "Name");
                let plan = person_rename_plan(&app.session.catalog, from, to);
                if !plan.needs_merge {
                    *merge = false;
                }
                if submitted && plan.block.is_none() && (!plan.needs_merge || *merge) {
                    confirm = true;
                }
                if let Some(why) = plan.block {
                    ui.label(egui::RichText::new(crate::i18n::tr(why)).color(t.caution));
                } else {
                    ui.label(
                        egui::RichText::new(crate::i18n::tr_format!(
                            "Renames {n} face{} across the library. The files are not changed.",
                            if plan.faces == 1 { "" } else { "s" },
                            n = plan.faces
                        ))
                        .color(t.text_dim),
                    );
                }
                if plan.needs_merge {
                    ui.label(
                        egui::RichText::new(crate::i18n::tr_format!(
                            "“{to}” is already a person. Combining them makes one person; Undo separates them again.",
                            to = to.trim()
                        ))
                        .color(t.caution),
                    );
                    let r = ui.checkbox(merge, crate::i18n::tr("Combine with the existing person"));
                    crate::widgets::register(ui, "check:personMerge", r.rect);
                }
            }
            Dialog::RenameKeyword { from, to } => {
                let n = app.caches.dialog_count(&app.session.catalog, ("renameKeyword", from.as_str()), || {
                    app.session.catalog.photos().filter(|p| p.meta.keywords.iter().any(|k| lightcraft_catalog::keywords::is_under(k, from))).count()
                });
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
                    .caches
                    .suggestions(&app.session.catalog, from, into, 12)
                    .iter()
                    .filter(|k| !from.iter().any(|f| lightcraft_catalog::keywords::is_under(k, f)))
                    .cloned()
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
            Dialog::NewAlbum { name, folder, include_selected, .. } => {
                if dialog_field(ui, "field:albumName", name, "Name") {
                    confirm = true;
                }
                // as in Lightroom, the option is only there when something is selected
                if !*folder && !app.session.selected().is_empty() {
                    ui.add_space(6.0);
                    let r = ui.checkbox(include_selected, crate::i18n::tr("Include selected photos"));
                    crate::widgets::register(ui, "check:albumIncludeSelected", r.rect);
                }
            }
            Dialog::RenameAlbum { name, .. } => {
                if dialog_field(ui, "field:albumName", name, "Name") {
                    confirm = true;
                }
            }
            Dialog::ShareLink { name, days, originals, upload, password, .. } => {
                if share_link_body(ui, &t, name, days, originals, upload, password) {
                    confirm = true;
                }
            }
            Dialog::HiddenPassword { mode, current, password, confirm: again, .. } => {
                let locked = !app.session.hidden.unlocked();
                if hidden_password_body(ui, &t, *mode, locked, current, password, again) {
                    confirm = true;
                }
            }
            Dialog::CreatePreset { name, group, groups } => {
                crate::widgets::named(
                    ui.add(crate::widgets::text_box(
                        ui,
                        egui::TextEdit::singleline(name).hint_text(crate::i18n::tr("Preset name")).desired_width(f32::INFINITY),
                    )),
                    crate::i18n::tr("Preset name"),
                );
                crate::widgets::named(
                    ui.add(crate::widgets::text_box(
                        ui,
                        egui::TextEdit::singleline(group).hint_text(crate::i18n::tr("Group")).desired_width(f32::INFINITY),
                    )),
                    crate::i18n::tr("Group"),
                );
                ui.label(egui::RichText::new(crate::i18n::tr("Settings to include")).color(t.text_dim));
                group_checklist(ui, "presetInclude", groups);
            }
            Dialog::CopySettings { groups } => group_checklist(ui, "copyGroup", groups),
            Dialog::PasteSettings { groups } => {
                let n = app.session.selected().len();
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
            Dialog::SlideshowExport { settings } => slideshow_export_body(app, ui, settings),
            Dialog::ContactSheet { options } => {
                options.columns = options.columns.clamp(1, 8);
                options.rows = options.rows.clamp(1, 10);
                let n = app.session.selected().len();
                ui.label(crate::i18n::tr_format!("{n} selected photos · {pages} pages", n = n, pages = n.div_ceil(options.columns * options.rows)));
                field(ui, "Paper", |ui| {
                    let c = crate::widgets::combo_box("contactSheetPaper").selected_text(&options.paper).show_ui(ui, |ui| {
                        ui.selectable_value(&mut options.paper, "a4".into(), "A4");
                        ui.selectable_value(&mut options.paper, "letter".into(), crate::i18n::tr("Letter"));
                    });
                    crate::widgets::name_combo(&c.response, crate::i18n::tr("Paper"));
                });
                ui.checkbox(&mut options.landscape, crate::i18n::tr("Landscape"));
                field(ui, "Columns", |ui| {
                    ui.add(egui::DragValue::new(&mut options.columns).range(1..=8));
                });
                field(ui, "Rows", |ui| {
                    ui.add(egui::DragValue::new(&mut options.rows).range(1..=10));
                });
                ui.checkbox(&mut options.captions, crate::i18n::tr("Filename captions"));
                ui.label(crate::i18n::tr("150 dpi · sRGB · current edits · fit without cropping"));
                ui.label(crate::i18n::tr("Choose a PDF destination after confirming. Print the PDF from your viewer."));
            }
            Dialog::Export { opts, full_size, resize, preset_name, limit_kb, dir, .. } => {
                crate::panels::export::form(app, ui, crate::panels::export::Form { opts, full_size, resize, preset_name, limit_kb, dir }, crate::panels::export::Part::All);
            }
            Dialog::Merge { opts } => crate::merge::body(app, ui, opts),
            Dialog::PresetImport { review } => crate::preset_import::body(app, ui, review),
            Dialog::Import { opts } if app.compact => crate::import::mobile_body(app, ui, opts),
            Dialog::Import { opts } => crate::import::body(app, ui, opts),
            Dialog::Settings { tab } => crate::panels::settings::body(app, ui, tab, body_max),
            Dialog::SamModel { error, .. } => sam_model_body(app, ui, error.as_deref()),
            Dialog::SuperRes { error } => crate::superres::body(app, ui, error.as_deref()),
            Dialog::SyncChoice => choice_made |= crate::panels::sync_choice::body(app, ui),
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
                if ui.is_enabled() && just_opened(ui, "keyword-tag") {
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
                        crate::widgets::register(ui, "check:keywordInside", r.rect);
                    }
                    let n = app.session.selection.ids.len();
                    let r = ui.add_enabled(n > 0, egui::Checkbox::new(add_to_selected, crate::i18n::tr("Add to Selected Photos")));
                    crate::widgets::register(ui, "check:keywordAddToSelected", r.rect);
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
                    crate::widgets::register(ui, id, r.rect);
                }
            }
            Dialog::KeywordSet { replaces, name, slots, as_new } => {
                ui.label(egui::RichText::new(crate::i18n::tr("Set name")).color(t.text_dim));
                let r = crate::text_field::TextField::singleline("field:keywordSetName", name)
                    .hint(crate::i18n::tr("Set name"))
                    .width(f32::INFINITY)
                    .select_on_focus(true)
                    .show(ui);
                if ui.is_enabled() && just_opened(ui, "keyword-set") {
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
                    crate::widgets::register(ui, "check:keywordSetAsNew", r.rect);
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
            Dialog::MovePhotos { ids, name, dont_ask, .. } => {
                let n = ids.len();
                ui.label(crate::i18n::tr_format!("Move {n} photo{} to “{}”?", if n == 1 { "" } else { "s" }, name, n = n));
                ui.label(
                    egui::RichText::new(crate::i18n::tr("Their files and XMP sidecars move on disk. Edit → Undo moves them back.")).color(t.text_dim),
                );
                ui.add_space(4.0);
                let c = ui.checkbox(dont_ask, crate::i18n::tr("Don't ask again"));
                crate::widgets::register(ui, "check:moveDontAsk", c.rect);
                c.on_hover_text(crate::i18n::tr("Settings → Confirm before moving photos to another folder on disk turns it back on"));
            }
            Dialog::SaveEffect { name, group, fields, amount, brush } => {
                if crate::panels::mask_effects::save_body(app, ui, name, group, fields, amount, brush) {
                    confirm = true;
                }
            }
            Dialog::ManageEffects { search, selected, rename, confirm_delete, new_group } => {
                crate::panels::mask_effects::manage_body(app, ui, search, selected, rename, confirm_delete, new_group)
            }
            Dialog::DeleteEffects { names } => {
                let what = match names.as_slice() {
                    [one] => crate::i18n::tr_format!("Delete the effect preset “{name}”?", name = one),
                    _ => crate::i18n::tr_format!("Delete these {n} effect presets?", n = names.len()),
                };
                ui.label(what);
                ui.label(egui::RichText::new(crate::i18n::tr("Masks keep their sliders. Export presets first to keep a copy.")).color(t.text_dim));
            }
            Dialog::DeletePresetGroup { group, count } => {
                ui.label(crate::i18n::tr_format!("Delete the group “{name}” and its {n} presets?", name = group, n = count));
                ui.label(egui::RichText::new(crate::i18n::tr("Photos keep their edits. Export the group first to keep a copy.")).color(t.text_dim));
            }
            d @ Dialog::SynchronizeFolder { .. } => crate::sync::body(app, ui, d),
            Dialog::LightroomCatalog { folder, catalog } => {
                ui.label(crate::i18n::tr("This is a Lightroom catalog, not a LightCraft library. Import it into a LightCraft library instead?"));
                ui.label(egui::RichText::new(catalog.as_deref().unwrap_or(folder.as_str())).color(t.text_dim));
                // (the import then asks where the catalog goes, unless Settings says to merge, #2672)
                let asks =
                    app.ui.settings.lightroom_import == crate::state::LightroomImport::Ask && app.session.library.as_ref().is_some_and(|l| l.on_disk);
                let what = if asks {
                    "The import reads the catalog without changing it. Next, choose whether its photos, edits, keywords and collections join the library open now or go into a new library."
                } else {
                    "The import reads the catalog without changing it and adds its photos, edits, keywords and collections to the library open now."
                };
                ui.label(egui::RichText::new(crate::i18n::tr(what)).color(t.text_dim));
            }
            d @ Dialog::LightroomDestination { .. } => crate::lightroom_destination::body(app, ui, d),
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
                let tab_id = egui::Id::unique("about_tab");
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
                        let r = crate::button::Button::new("Join the ArtCraft Discord")
                            .primary()
                            .large()
                            .min_width(260.0)
                            .widget("button:aboutDiscord")
                            .tooltip(crate::links::DISCORD)
                            .show(ui);
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
            Dialog::ViewOptions => crate::panels::settings::view_options(app, ui, &t),
            Dialog::Print => crate::print_view::body(app, ui, &t),
        }
    };
    let (mut cancel_tapped, mut ok_tapped) = (false, false);
    if compact && let Some(count) = confirm_delete {
        if leaving {
            // (an alert has no page to slide away: it is gone with its answer)
            app.ui.dialog_leaving = None;
            crate::panels::alert::hidden(ctx, "dialog");
            return;
        }
        // a yes-or-no question on a phone is an alert, not a page
        let what =
            if count == 1 { crate::i18n::tr("this photo").to_string() } else { crate::i18n::tr_format!("these {count} photos", count = count) };
        let title = crate::i18n::tr_format!("Move {what} to Recently Deleted?", what = what);
        let alert = crate::panels::alert::Alert {
            id: "dialog",
            title: &title,
            message: "They can be restored from Recently Deleted until it is emptied.",
            cancel,
            ok: &ok,
            destructive: true,
        };
        (cancel_tapped, ok_tapped) = crate::panels::alert::show(ctx, &alert);
    } else if export_flow {
        // share sheet, options, more options
        let (outcome, gone) = crate::panels::export::phone(app, ctx, &mut dlg, !leaving);
        if leaving {
            if gone {
                app.ui.dialog_leaving = None;
            }
            return;
        }
        match outcome {
            crate::panels::export::Outcome::Stay => {}
            crate::panels::export::Outcome::Close => cancel_tapped = true,
            crate::panels::export::Outcome::Go(to) => {
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
        let bar = crate::panels::mobile::page(ctx, "dialog", &title, &cancel, action, !leaving, &mut body);
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
        // before the window: drawing it closes a dropdown the click lands outside of
        let popup_was_open = egui::Popup::is_any_open(ctx);
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
                // Disabled, not skipped: the dialog keeps its layout and state, but a raw event in the
                // first prompt frame cannot toggle a checkbox or press a button underneath it. The
                // blocking library window gets the same treatment: it owns input while it is on screen,
                // so the dialog behind it must not take focus or accept typed text either.
                if raw_blocked {
                    ui.disable();
                }
                ui.spacing_mut().item_spacing.y = 8.0;
                body_h = if settings_page {
                    body(ui);
                    Some(crate::panels::settings::desktop_height(body_max))
                } else {
                    dialog_body(ui, (!import && !print).then_some(title.as_str()), body_max, &mut body)
                };
                ui.add_space(4.0);
                if t.cards() {
                    // Studio: capsules, the action in the accent colour (as on the admin page); Settings'
                    // Close sits at its end, under the section
                    let layout = if settings_page {
                        egui::Layout::right_to_left(egui::Align::Center)
                    } else {
                        egui::Layout::left_to_right(egui::Align::Center)
                    };
                    ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), 26.0), layout, |ui| {
                        let capsule =
                            |text: egui::WidgetText| egui::Button::new(text).corner_radius(t.button_radius).min_size(egui::vec2(64.0, 26.0));
                        if !informational {
                            let r = ui.add(capsule(crate::i18n::tr(cancel).into()));
                            crate::widgets::register(ui, "button:dialogCancel", r.rect);
                            cancel_tapped = r.clicked();
                        }
                        if !ok.is_empty() {
                            let button = if primary {
                                capsule(egui::RichText::new(ok.as_str()).color(egui::Color32::WHITE).strong().into()).fill(t.accent)
                            } else {
                                capsule(ok.as_str().into())
                            };
                            let r = ui.add_enabled(can_confirm, button);
                            crate::widgets::register(ui, "button:dialogOk", r.rect);
                            ok_tapped = r.clicked();
                        }
                    });
                } else {
                    // the confirm on the right with Cancel to its left (macOS and GNOME's order), filled with
                    // the accent when it is the dialog's primary action. The row spans the width the window
                    // gives its content this frame (its default width, or its content's if wider), so it
                    // reaches the edge without widening the window.
                    let row_w = ui.available_width().max(ui.min_rect().width()).max(1.0);
                    let row_h = ui.spacing().interact_size.y;
                    ui.allocate_ui_with_layout(egui::vec2(row_w, row_h), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        use crate::button::{Button, Kind};
                        // (each kind of dialog its own buttons: the keyboard's place doesn't carry from a
                        // dialog to the one it opens)
                        let cancel_button = Button::new(cancel).widget("button:dialogCancel").id_salt(kind).min_width(72.0);
                        let ok_button = Button::new(ok.as_str())
                            .kind(if primary { Kind::Primary } else { Kind::Secondary })
                            .enabled(can_confirm)
                            .widget("button:dialogOk")
                            .id_salt(kind)
                            .min_width(72.0);
                        // (Cancel first, then the confirm: the keyboard goes through them as the eye does)
                        let gap = ui.spacing().item_spacing.x;
                        let mut buttons_w = 0.0;
                        if !informational {
                            buttons_w += cancel_button.desired_size(ui).x + gap;
                        }
                        if !ok.is_empty() {
                            buttons_w += ok_button.desired_size(ui).x;
                        }
                        ui.add_space((ui.available_width() - buttons_w).max(0.0));
                        if !informational {
                            cancel_tapped = cancel_button.show(ui).clicked();
                        }
                        if !ok.is_empty() {
                            ok_tapped = ok_button.show(ui).clicked();
                        }
                    });
                }
                if import {
                    crate::import::note_dialog_bottom(ui);
                }
            });
        if let Some(w) = &shown {
            // (the import review keeps its hand-picked photos however the click lands)
            if !raw_blocked && app.ui.settings.close_dialogs_outside && !import && clicked_backdrop(ctx, w.response.rect, popup_was_open) {
                close = true;
            } else if raw_blocked {
                // A press made under the prompt or library window must not be remembered as a
                // backdrop press for a release that arrives after that covering window goes away.
                ctx.data_mut(|d| d.remove_temp::<bool>(egui::Id::unique("dialog-backdrop-press")));
            }
        }
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
            crate::widgets::register_response("dialog:window", w.response.rect, &w.response);
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
    if !raw_blocked && ctx.input(|i| i.key_pressed(egui::Key::Escape)) && !crate::widgets::escape_taken(ctx) {
        close = true;
    }
    if !raw_blocked && confirm {
        match confirm_dialog(app, &dlg) {
            // a phone's export stays open on an error: the options may be what to change
            Err(e) if compact && matches!(dlg, Dialog::Export { .. }) => app.toast_error(ctx, e),
            // the import review stays open on an error (e.g. an unusable folder template), and so
            // does Export (e.g. no folder chosen) so the choices aren't lost, and Edit Capture Time
            // (a time that isn't a date, or a shift it refuses)
            Err(e)
                if matches!(
                    dlg,
                    Dialog::Import { .. }
                        | Dialog::Export { .. }
                        | Dialog::ContactSheet { .. }
                        | Dialog::SlideshowExport { .. }
                        | Dialog::Print
                        | Dialog::NewAlbum { .. }
                        | Dialog::NewSmartAlbum { .. }
                        | Dialog::SmartRules { .. }
                        | Dialog::KeywordTag { .. }
                        | Dialog::KeywordSet { .. }
                        | Dialog::SynchronizeFolder { .. }
                        | Dialog::ReportBug { .. }
                        | Dialog::ShareLink { .. }
                        | Dialog::HiddenPassword { .. }
                        | Dialog::SaveEffect { .. }
                        | Dialog::CaptureTime { .. }
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
            // a move says how it went, or why nothing moved
            Ok(r) if matches!(dlg, Dialog::MovePhotos { .. }) => {
                if let Dialog::MovePhotos { name, .. } = &dlg {
                    let text = crate::panels::left::moved_text(r["moved"].as_u64().unwrap_or(0), name);
                    app.toast(ctx, text);
                }
                close = true;
            }
            Err(e) if matches!(dlg, Dialog::MovePhotos { .. }) => {
                app.toast(ctx, e);
                close = true;
            }
            // photos whose capture time doesn't read are left as they were: say so
            Ok(r) if matches!(dlg, Dialog::CaptureTime { .. }) && r["unreadable"].as_u64().is_some_and(|n| n > 0) => {
                app.toast(ctx, crate::i18n::tr("Photos whose capture time can't be read were left as they were"));
                close = true;
            }
            // where a Lightroom catalog goes: stays open to be corrected (a folder that isn't
            // empty), unless the library it names has gone
            Err(e) if matches!(dlg, Dialog::LightroomDestination { .. }) => {
                close = !crate::lightroom_destination::keeps_open_on_error(app, &dlg);
                app.toast(ctx, e);
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
    if !close && !blocked {
        let frame = ctx.cumulative_frame_nr();
        ctx.data_mut(|d| d.insert_temp(egui::Id::unique(BACKDROP_SHOWN), frame));
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


/// The last frame a dialog was drawn and stayed open, for [`clicked_backdrop`].
const BACKDROP_SHOWN: &str = "dialog-backdrop-shown";

/// Whether a dialog stays open after its action succeeded (the SAM 3 dialog while the model
/// downloads).
pub fn keeps_open(app: &LightcraftApp, dlg: &Dialog) -> bool {
    (matches!(dlg, Dialog::SamModel { .. }) && !app.session.segmenter.installed())
        || (matches!(dlg, Dialog::SuperRes { .. }) && crate::superres::keeps_open(app))
}

/// Open a one-field dialog that runs `command` with `params` + `{key: typed value}`.
/// Show the Hidden album (`show`: turn on Show Hidden instead), asking for the password first while Hidden is
/// locked (or to choose one, when the library has none yet).
pub fn open_hidden(app: &mut LightcraftApp, show: bool) -> Result<serde_json::Value, String> {
    if app.session.hidden.unlocked() {
        return show_unlocked(app, show);
    }
    let mode = if app.session.hidden.has_password() { HiddenPrompt::Unlock } else { HiddenPrompt::Create };
    app.ui.dialog = Some(Dialog::HiddenPassword { mode, show, current: String::new(), password: String::new(), confirm: String::new() });
    Ok(json!({}))
}

/// Hidden is unlocked: show the Hidden album, or turn on Show Hidden.
fn show_unlocked(app: &mut LightcraftApp, show: bool) -> Result<serde_json::Value, String> {
    if show {
        return app.run("library.showHidden", json!({"on": true}));
    }
    let r = app.run("library.source", json!({"kind": "hidden"}));
    if r.is_ok() {
        crate::panels::left::leave_people(app);
    }
    r
}

/// Change the Hidden password (Settings): asks for the current one while Hidden is locked.
pub fn change_hidden_password(app: &mut LightcraftApp) {
    let mode = if app.session.hidden.has_password() { HiddenPrompt::Change } else { HiddenPrompt::Create };
    app.ui.dialog = Some(Dialog::HiddenPassword { mode, show: false, current: String::new(), password: String::new(), confirm: String::new() });
}

/// What the Hidden password dialog can confirm: a password to unlock with, or a new one typed the same twice.
fn hidden_password_ok(mode: HiddenPrompt, password: &str, confirm: &str) -> bool {
    match mode {
        HiddenPrompt::Unlock => !password.is_empty(),
        HiddenPrompt::Create | HiddenPrompt::Change => password.chars().count() >= lightcraft_engine::hidden::MIN_PASSWORD && password == confirm,
    }
}

/// The Hidden password dialog's fields. True when Return confirms.
fn hidden_password_body(
    ui: &mut egui::Ui,
    t: &Tokens,
    mode: HiddenPrompt,
    locked: bool,
    current: &mut String,
    password: &mut String,
    confirm: &mut String,
) -> bool {
    let first = just_opened(ui, "hiddenPassword");
    let field = |ui: &mut egui::Ui, id: &str, text: &mut String, hint: &str, focus: bool| {
        let mut f = crate::text_field::TextField::singleline(id, text).password(true).hint(crate::i18n::tr(hint)).width(f32::INFINITY);
        if crate::is_compact(ui.ctx()) {
            f = f.font(egui::FontId::proportional(17.0)).margin(egui::Margin::symmetric(12, 10));
        }
        let r = f.show(ui);
        if focus {
            r.response.request_focus();
        }
        r.ending == Some(crate::text_field::Ending::Return)
    };
    let note = match mode {
        HiddenPrompt::Unlock => "Enter the password to see your hidden photos.",
        HiddenPrompt::Create => {
            "Choose a password for Hidden. It opens Hidden on all your devices; on an iPhone, iPad or Mac, Face ID or Touch ID does."
        }
        HiddenPrompt::Change => "The new password opens Hidden on all your devices.",
    };
    ui.label(egui::RichText::new(crate::i18n::tr(note)).color(t.text_dim));
    let mut enter = false;
    if mode == HiddenPrompt::Change && locked {
        enter |= field(ui, "field:hiddenCurrent", current, "Current password", first);
    }
    let focus_new = first && !(mode == HiddenPrompt::Change && locked);
    let hint = if mode == HiddenPrompt::Unlock { "Password" } else { "New password" };
    enter |= field(ui, "field:hiddenPassword", password, hint, focus_new);
    if mode != HiddenPrompt::Unlock {
        enter |= field(ui, "field:hiddenConfirm", confirm, "Type it again", false);
        let short = !password.is_empty() && password.chars().count() < lightcraft_engine::hidden::MIN_PASSWORD;
        let differ = !confirm.is_empty() && confirm != password;
        let (note, warn) = match (short, differ) {
            (true, _) => ("A password has at least 4 characters.", true),
            (_, true) => ("The two passwords are different.", true),
            _ => ("Hidden keeps photos out of sight; it doesn't encrypt them.", false),
        };
        ui.label(egui::RichText::new(crate::i18n::tr(note)).color(if warn { t.caution } else { t.text_dim }));
    }
    enter && hidden_password_ok(mode, password, confirm)
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
    crate::widgets::register(ui, "check:shareOriginals", r.rect);
    let r = crate::widgets::check(ui, upload, crate::i18n::tr("People can add photos to this album"));
    crate::widgets::register(ui, "check:shareUpload", r.rect);
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
