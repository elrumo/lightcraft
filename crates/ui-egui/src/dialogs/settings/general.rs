use super::widgets::{check, choices, heading, hint, path_label, row};
use super::*;

// ------------------------------------------------------------------------------------- General

pub(crate) fn general_tab(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let languages: Vec<_> = crate::i18n::Locale::for_picker().into_iter().map(|language| (language, language.name())).collect();
    pick(ui, t, "settingsLanguage", crate::i18n::tr("Language"), &languages, &mut app.ui.language);
    crate::i18n::set_language(app.ui.language);
    if !IOS {
        heading(ui, t, crate::i18n::tr("Library"));
        let location = match &app.session.library {
            Some(l) => l.dir.display().to_string(),
            None => crate::i18n::tr("In memory — nothing is saved").to_string(),
        };
        row(ui, t, crate::i18n::tr("Location"), |ui| {
            path_label(ui, "settingsLocation", &location, egui::TextStyle::Body.resolve(ui.style()), t.text_label);
        });
        row(ui, t, "", |ui| {
            let can = app.services.pick_folder.is_some();
            if action(ui, t, "settingsOpenLibrary", "Open Library…", can, false) {
                let _ = app.run("app.openLibrary", json!({}));
            }
            if action(ui, t, "settingsNewLibrary", "New Library…", can, false) {
                let _ = app.run("app.newLibrary", json!({}));
            }
            if !can {
                hint(ui, t, crate::i18n::tr("not available here"));
            }
        });
        // where libraries are folders to choose from (not in the browser, which keeps one)
        if app.services.pick_folder.is_some() {
            use crate::state::LaunchLibrary;
            pick(
                ui,
                t,
                "settingsAtLaunch",
                crate::i18n::tr("At launch"),
                &[(LaunchLibrary::Last, "Open the last library"), (LaunchLibrary::Ask, "Ask me")],
                &mut app.ui.settings.at_launch,
            );
            hint(
                ui,
                t,
                crate::i18n::tr("Ask me lists your recent libraries at launch. Holding Alt (Option on a Mac) as the window appears asks once."),
            );
        }
        // File ▸ Import Lightroom Catalog… (#2672): the window that asks where a catalog goes, or not
        {
            use crate::state::LightroomImport;
            pick(
                ui,
                t,
                "settingsLightroomImport",
                crate::i18n::tr("When importing a Lightroom catalog"),
                &[(LightroomImport::Ask, "Ask where to import it"), (LightroomImport::AlwaysMerge, "Always merge into the open library")],
                &mut app.ui.settings.lightroom_import,
            );
        }
    }
    heading(ui, t, crate::i18n::tr("Hidden"));
    row(ui, t, "", |ui| {
        let label = if app.session.hidden.has_password() { "Change Hidden Password…" } else { "Set Hidden Password…" };
        if action(ui, t, "settingsHiddenPassword", label, true, false) {
            crate::panels::dialogs::change_hidden_password(app);
        }
    });
    hint(
        ui,
        t,
        crate::i18n::tr("The password that opens the Hidden album on every device of this library (Face ID or Touch ID on an iPhone, iPad or Mac)."),
    );
    // (the photos the sensitive-content model flagged, Settings ▸ AI Models)
    let mut blur = app.session.blur_sensitive;
    if ui.checkbox(&mut blur, crate::i18n::tr("Blur photos that might be sensitive in the grid")).changed() {
        let _ = app.run("library.setBlurSensitive", json!({"on": blur}));
    }
    heading(ui, t, crate::i18n::tr("Startup"));
    pick(
        ui,
        t,
        "settingsStartup",
        crate::i18n::tr("Open in"),
        &[(StartupView::Last, "Last view"), (StartupView::Grid, "Photo Grid"), (StartupView::Detail, "Detail View")],
        &mut app.ui.settings.startup_view,
    );
    heading(ui, t, crate::i18n::tr("Culling"));
    check(ui, "settings.autoAdvance", &mut app.ui.auto_advance, "Auto Advance: move to the next photo after rating or flagging");
    check(ui, "settings.confirmDelete", &mut app.ui.settings.confirm_delete, "Confirm before moving photos to Recently Deleted");
    check(ui, "settings.askBeforeMove", &mut app.ui.settings.ask_before_move, "Confirm before moving photos to another folder on disk");
    if IOS {
        return;
    }
    heading(ui, t, crate::i18n::tr("Dialogs"));
    check(ui, "settings.closeDialogsOutside", &mut app.ui.settings.close_dialogs_outside, "Close a dialog by clicking outside it");
    heading(ui, t, crate::i18n::tr("External Editing"));
    row(ui, t, crate::i18n::tr("PhotoCraft"), |ui| {
        crate::text_field::TextField::singleline("field:photocraftPath", &mut app.ui.settings.photocraft_path)
            .hint(crate::i18n::tr("Found automatically"))
            .width(220.0)
            .show(ui);
    });
    edit_file_rows(ui, t, "photocraftFile", &mut app.ui.settings.photocraft_file);
    hint(
        ui,
        t,
        crate::i18n::tr(
            "Photo ▸ Edit In ▸ Edit in PhotoCraft (⇧⌘E, or right-click a photo) renders a copy with your edits, stacks it with the original and opens it in PhotoCraft. Save there and the copy updates here. As PSD, PhotoCraft saves straight back to it. Leave the location empty unless you use a portable copy.",
        ),
    );
    row(ui, t, crate::i18n::tr("Other editor"), |ui| {
        crate::text_field::TextField::singleline("field:externalEditor", &mut app.ui.settings.external_editor)
            .hint(crate::i18n::tr("System default"))
            .width(220.0)
            .show(ui);
    });
    edit_file_rows(ui, t, "externalFile", &mut app.ui.settings.external_file);
    hint(ui, t, crate::i18n::tr("Edit in Other Editor (⌥⌘E) opens the copy in this application (an app name on macOS, a program path elsewhere)."));
    hint(
        ui,
        t,
        crate::i18n::tr(
            "To hand this editor the original file itself instead (the raw, without a copy: DxO PhotoLab, for example), right-click a photo → Edit In → Open Original in Other Editor (RAW, no copy).",
        ),
    );
}

/// The file Edit In hands an editor: format, colour space and bit depth (`button:{id}Format-…`).
pub(crate) fn edit_file_rows(ui: &mut egui::Ui, t: &Tokens, id: &str, file: &mut crate::state::EditFile) {
    use crate::state::{EditFormat, EditSpace};
    row(ui, t, crate::i18n::tr("Format"), |ui| {
        choices(ui, &format!("{id}Format"), &[(EditFormat::Psd, "PSD"), (EditFormat::Tiff, "TIFF")], &mut file.format);
        ui.separator();
        choices(ui, &format!("{id}Depth"), &[(16u8, "16-bit"), (8, "8-bit")], &mut file.bit_depth);
    });
    row(ui, t, crate::i18n::tr("Color space"), |ui| {
        choices(
            ui,
            &format!("{id}Space"),
            &[(EditSpace::AdobeRgb, "Adobe RGB"), (EditSpace::ProPhoto, "ProPhoto"), (EditSpace::DisplayP3, "P3"), (EditSpace::Srgb, "sRGB")],
            &mut file.color_space,
        );
    });
}

// -------------------------------------------------------------------------------------- Import

/// A labelled preset picker: `None` = `none_label`. Returns the new choice when it changed.
pub(crate) fn preset_combo(
    app: &LightcraftApp,
    ui: &mut egui::Ui,
    t: &Tokens,
    id: &str,
    label: &str,
    current: Option<&str>,
    none_label: &str,
) -> Option<Option<String>> {
    let name = |pid: &str| {
        app.session
            .presets
            .iter()
            .find(|p| p.id == pid)
            .map(|p| crate::i18n::builtin_label(&p.name, p.builtin).to_string())
            .unwrap_or_else(|| crate::i18n::tr_format!("{pid} (missing)", pid = pid))
    };
    let text = current.map(name).unwrap_or_else(|| crate::i18n::tr(none_label).to_string());
    let mut out = None;
    select(ui, t, id, label, text, |ui| {
        if option(ui, id, 0, current.is_none(), crate::i18n::tr(none_label)) {
            out = Some(None);
        }
        if let Some(pid) = crate::panels::presets::picker_rows(ui, id, &app.session.presets, current) {
            out = Some(Some(pid));
        }
    });
    out.filter(|v| v.as_deref() != current)
}

/// Cameras of the raws in the library plus those with a stored default, sorted.
pub(super) fn cameras(app: &LightcraftApp) -> Vec<String> {
    let mut v: Vec<String> = app
        .session
        .catalog
        .photos()
        .filter(|p| p.kind == lightcraft_catalog::MediaKind::Raw && !p.meta.camera.is_empty())
        .map(|p| p.meta.camera.clone())
        .chain(app.session.import_defaults.cameras.iter().map(|c| c.camera.clone()))
        .collect();
    v.sort_by_key(|c| c.to_lowercase());
    v.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    v
}