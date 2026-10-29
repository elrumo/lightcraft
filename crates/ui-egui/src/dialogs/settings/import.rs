use super::general::{cameras, preset_combo};
use super::widgets::{check, heading, hint, row};
use super::*;

pub(crate) fn import_tab(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let d = app.session.import_defaults.clone();
    if !cfg!(target_arch = "wasm32") && !IOS {
        heading(ui, t, crate::i18n::tr("Dropped Files"));
        row(ui, t, "", |ui| {
            check(ui, "settings.importDropped", &mut app.ui.settings.import_dropped_directly, "Import dropped files without review");
        });
    }
    heading(ui, t, crate::i18n::tr("Cameras and cards"));
    row(ui, t, "", |ui| {
        check(ui, "settings.importOnConnect", &mut app.ui.settings.import_on_connect, "Show the import when a camera or memory card is connected");
    });
    if !cfg!(target_arch = "wasm32") && !IOS {
        // (the library's own: it travels with the library, where the switch above is this app's)
        let mut on_card = d.show_on_card;
        row(ui, t, "", |ui| {
            if check(ui, "settings.importOnCard", &mut on_card, "Show the import review when a camera or card is connected") {
                let _ = app.run("library.preferences", json!({"import": {"showOnCard": on_card}}));
            }
        });
    }
    heading(ui, t, crate::i18n::tr("Raw defaults"));
    hint(ui, t, crate::i18n::tr("Settings new raw photos start from. Changing them doesn't touch photos already in the library."));
    if let Some(v) = preset_combo(app, ui, t, "settingsRawPreset", crate::i18n::tr("Raw photos"), d.raw_preset.as_deref(), "LightCraft Default") {
        let _ = app.run("library.preferences", json!({"import": {"rawPreset": v}}));
    }
    // under the raw preset (a preset that sets Detail wins); part of the photo's import look, so it isn't an edit
    row(ui, t, "Color Noise Reduction", |ui| {
        let mut v = d.color_noise_reduction();
        let r = ui.add(egui::Slider::new(&mut v, 0.0..=100.0).step_by(1.0).max_decimals(0));
        register(ui, "slider:settingsImportColorNoise".to_string(), r.rect);
        // (saved when let go, not on every step of a drag)
        if r.drag_stopped() || (r.changed() && !r.dragged()) {
            let _ = app.run("library.preferences", json!({"import": {"colorNoiseReduction": v}}));
        }
    });
    let mut per = d.per_camera;
    row(ui, t, "", |ui| {
        if check(ui, "settings.perCamera", &mut per, "Use camera-specific defaults") {
            let _ = app.run("library.preferences", json!({"import": {"perCamera": per}}));
        }
    });
    if per {
        let cams = cameras(app);
        if cams.is_empty() {
            hint(ui, t, crate::i18n::tr("No raw photos yet: cameras appear here once their photos are in the library."));
        }
        let mut cameras = |ui: &mut egui::Ui| {
            for (i, cam) in cams.iter().enumerate() {
                let entry = d.cameras.iter().find(|c| c.camera.eq_ignore_ascii_case(cam));
                // "Raw default" = no entry; otherwise the entry's preset (None = LightCraft Default)
                const RAW_DEFAULT: &str = "\u{1}raw";
                let current = match entry {
                    None => Some(RAW_DEFAULT),
                    Some(e) => e.preset.as_deref(),
                };
                let mut pick = None;
                let label = match current {
                    Some(RAW_DEFAULT) => crate::i18n::tr("Same as raw default").to_string(),
                    None => crate::i18n::tr("LightCraft Default").to_string(),
                    Some(pid) => app
                        .session
                        .presets
                        .iter()
                        .find(|p| p.id == pid)
                        .map(|p| crate::i18n::builtin_label(&p.name, p.builtin).to_string())
                        .unwrap_or_else(|| pid.to_string()),
                };
                let id = format!("settingsCamera-{i}");
                select(ui, t, &id, cam, label, |ui| {
                    if option(ui, &id, 0, current == Some(RAW_DEFAULT), crate::i18n::tr("Same as raw default")) {
                        pick = Some(json!({"camera": cam, "remove": true}));
                    }
                    if option(ui, &id, 1, current.is_none(), crate::i18n::tr("LightCraft Default")) {
                        pick = Some(json!({"camera": cam, "preset": null}));
                    }
                    for (j, p) in app.session.presets.iter().enumerate() {
                        if option(ui, &id, j + 2, current == Some(p.id.as_str()), crate::i18n::builtin_label(&p.name, p.builtin)) {
                            pick = Some(json!({"camera": cam, "preset": p.id}));
                        }
                    }
                });
                if let Some(c) = pick {
                    let _ = app.run("library.preferences", json!({"camera": c}));
                }
            }
        };
        if crate::is_compact(ui.ctx()) {
            // (the page scrolls)
            cameras(ui);
        } else {
            egui::ScrollArea::vertical().max_height(150.0).id_salt("settingsCameras").show(ui, cameras);
        }
    }
    heading(ui, t, crate::i18n::tr("Other images (JPEG, PNG, TIFF, HEIC…)"));
    if let Some(v) = preset_combo(app, ui, t, "settingsOtherPreset", crate::i18n::tr("Non-raw photos"), d.other_preset.as_deref(), "None") {
        let _ = app.run("library.preferences", json!({"import": {"otherPreset": v}}));
    }
    heading(ui, t, crate::i18n::tr("Metadata"));
    hint(ui, t, crate::i18n::tr("Added to photos you import that don't already have it."));
    for (key, label, hint_text, value) in
        [("copyright", "Copyright", "© 2026 Your Name", d.copyright.clone()), ("creator", "Creator", "Your Name", d.creator.clone())]
    {
        let id = egui::Id::unique(("settingsMeta", key));
        let mut text: String = ui.data(|m| m.get_temp(id)).unwrap_or(value.clone());
        let r = text_row(ui, t, &format!("settings.{key}"), label, &mut text, hint_text, false, 240.0);
        if r.lost_focus() && text.trim() != value {
            let _ = app.run("library.preferences", json!({"import": {key: text.trim()}}));
        }
        if r.has_focus() {
            ui.data_mut(|m| m.insert_temp(id, text));
        } else {
            ui.data_mut(|m| m.remove::<String>(id));
        }
    }
    if !app.session.metadata_presets.is_empty() {
        let cur = d.metadata_preset.clone();
        let mut pick = None;
        let id = "settingsMetaPreset";
        select(ui, t, id, crate::i18n::tr("Metadata preset"), cur.clone().unwrap_or_else(|| crate::i18n::tr("None").into()), |ui| {
            if option(ui, id, 0, cur.is_none(), crate::i18n::tr("None")) {
                pick = Some(String::new());
            }
            for (i, m) in app.session.metadata_presets.iter().enumerate() {
                if option(ui, id, i + 1, cur.as_deref() == Some(m.name.as_str()), &m.name) {
                    pick = Some(m.name.clone());
                }
            }
        });
        if let Some(n) = pick {
            let _ = app.run("library.preferences", json!({"import": {"metadataPreset": n}}));
        }
    }
    if !cfg!(target_arch = "wasm32") && !IOS {
        heading(ui, t, crate::i18n::tr("Ignored folders and files"));
        hint(
            ui,
            t,
            crate::i18n::tr(
                "Names that importing and browsing skip, one per line. * matches any text and ? one character: *.fcpbundle skips Final Cut bundles and everything in them.",
            ),
        );
        let id = egui::Id::unique("settingsIgnore");
        let value = d.ignore.join("\n");
        let mut text: String = ui.data(|m| m.get_temp(id)).unwrap_or(value.clone());
        let r = ui.add(crate::widgets::text_box(
            ui,
            egui::TextEdit::multiline(&mut text).desired_rows(4).desired_width(f32::INFINITY).hint_text("*.fcpbundle"),
        ));
        register(ui, "field:settings.ignore", r.rect);
        if r.lost_focus()
            && text != value
            && let Err(e) = app.run("library.preferences", json!({"import": {"ignore": text}}))
        {
            app.toast(ui.ctx(), e);
        }
        if r.has_focus() {
            ui.data_mut(|m| m.insert_temp(id, text));
        } else {
            ui.data_mut(|m| m.remove::<String>(id));
        }
    }
    if !IOS {
        heading(ui, t, crate::i18n::tr("XMP sidecars"));
        let mut xmp = app.session.xmp;
        if check(ui, "settings.autoWriteXmp", &mut xmp.auto_write, "Automatically write changes into XMP sidecars") {
            let _ = app.run("library.xmpPreferences", json!({"autoWrite": xmp.auto_write}));
        }
        use lightcraft_engine::sidecar::SidecarNaming as N;
        let mut n = xmp.naming;
        if pick(ui, t, "settingsXmpNaming", crate::i18n::tr("Sidecar names"), &[(N::Stem, "IMG_1.xmp"), (N::Full, "IMG_1.CR3.xmp")], &mut n) {
            let naming = if n == N::Full { "full" } else { "stem" };
            let _ = app.run("library.xmpPreferences", json!({"naming": naming}));
        }
    }
    if !cfg!(target_arch = "wasm32") && !IOS {
        heading(ui, t, crate::i18n::tr("Auto Import"));
        hint(ui, t, crate::i18n::tr("Photos that arrive in this folder (tethering, a scanner, a sync app) are added as soon as they're complete."));
        row(ui, t, crate::i18n::tr("Watched folder"), |ui| {
            match &d.auto_folder {
                Some(folder) => {
                    let beside = [crate::i18n::tr("Choose…"), crate::i18n::tr("Turn Off")];
                    path_label_beside(ui, "settingsAutoFolder", folder, egui::TextStyle::Body.resolve(ui.style()), t.text, &beside)
                }
                None => drop(ui.label(RichText::new(crate::i18n::tr("Off")).color(t.text))),
            }
            let can = app.services.pick_folder.is_some();
            let r = ui.add_enabled(can, egui::Button::new(crate::i18n::tr("Choose…")));
            register(ui, "button:settingsAutoFolder", r.rect);
            if r.clicked()
                && let Some(f) = app.services.pick_folder.as_mut().and_then(|f| f())
                && let Err(e) = app.run("library.autoImport", json!({"folder": f}))
            {
                app.toast(ui.ctx(), e);
            }
            if d.auto_folder.is_some() && ui.button(crate::i18n::tr("Turn Off")).clicked() {
                let _ = app.run("library.autoImport", json!({"folder": null}));
            }
        });
        if d.auto_folder.is_some() {
            let mut sub = d.auto_subfolders;
            if check(ui, "settings.autoImportSubfolders", &mut sub, "Include subfolders") {
                let _ = app.run("library.autoImport", json!({"subfolders": sub}));
            }
            hint(
                ui,
                t,
                crate::i18n::tr(
                    "Photos in folders inside the watched folder are added too (hidden folders and the library's own folder are skipped).",
                ),
            );
            row(ui, t, "", |ui| {
                let mut copy = d.auto_copy;
                if crate::widgets::check(ui, &mut copy, crate::i18n::tr("Copy into the library (else use the files where they are)")).changed() {
                    let _ = app.run("library.autoImport", json!({"copy": copy}));
                }
            });
            row(ui, t, crate::i18n::tr("Album"), |ui| {
                let id = egui::Id::unique("auto-album");
                let mut name = ui.data(|m| m.get_temp::<String>(id)).unwrap_or_else(|| d.auto_album.clone().unwrap_or_default());
                let r = ui
                    .add(crate::widgets::text_box(ui, egui::TextEdit::singleline(&mut name).hint_text(crate::i18n::tr("None")).desired_width(200.0)));
                if r.lost_focus() {
                    let _ = app.run("library.autoImport", json!({"album": name.trim()}));
                }
                ui.data_mut(|m| m.insert_temp(id, name));
            });
        }
    }
    if app.session.library.is_none() {
        hint(ui, t, crate::i18n::tr("In-memory session: these settings last until LightCraft quits."));
    }
}
