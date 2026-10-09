//! Settings ▸ AI Models: every optional model in one place. For each: what it is for, whether it
//! is on this device (and how much space it takes) or still to download (and from which host),
//! its licence and credit, whether it is turned on, and the buttons to download (after showing
//! the licence), cancel, delete (after asking) and turn it off.
//!
//! Nothing here downloads or deletes by itself: each action is a click, and a download or a
//! delete asks first in the card. The work is the engine's `models.*` commands
//! (`lightcraft_engine::models`), so an agent can do all of it too.

use egui::RichText;
use lightcraft_engine::models::ModelInfo;
use serde_json::json;

use crate::LightcraftApp;
use crate::state::ModelStep;
use crate::theme::Tokens;
use crate::widgets::register;

/// `1.2 GB`, `4.5 MB`, `230 KB`.
pub fn size(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1e9 {
        format!("{:.2} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.1} MB", b / 1e6)
    } else {
        format!("{:.0} KB", (b / 1e3).max(if bytes > 0 { 1.0 } else { 0.0 }))
    }
}

fn hint(ui: &mut egui::Ui, t: &Tokens, text: impl Into<String>) {
    ui.label(RichText::new(text.into()).size(11.0).color(t.text_dim));
}

/// The tab's content.
pub fn tab(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    // the text wraps within the dialog (on a phone, within the screen)
    ui.set_max_width(crate::panels::modal_width(ui.ctx(), 540.0));
    let models = app.session.models();
    ui.add_space(4.0);
    hint(
        ui,
        t,
        crate::i18n::tr(
            "Optional models that power AI features. They are never part of LightCraft, they download only when you choose, and they run on this device: your photos are not uploaded for them.",
        ),
    );
    ui.add_space(6.0);
    if models.is_empty() {
        hint(ui, t, crate::i18n::tr("This version of LightCraft has no AI models."));
        return;
    }
    for m in &models {
        card(app, ui, t, m);
        ui.add_space(6.0);
    }
    let total: u64 = models.iter().map(|m| m.disk_bytes).fold(0, u64::saturating_add);
    let r = ui.label(RichText::new(format!("{} {}", crate::i18n::tr("Models on this device:"), size(total))).color(t.text));
    register(ui.ctx(), "label:modelStorage", r.rect);
}

fn card(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens, m: &ModelInfo) {
    let running = m.download.as_ref().is_some_and(|d| d.running);
    let step = app.ui.model_confirm.as_ref().filter(|(id, _)| *id == m.id).map(|(_, s)| *s);
    egui::Frame::group(ui.style()).inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        // title and status
        ui.horizontal(|ui| {
            ui.label(RichText::new(&m.label).font(t.semibold(13.0)).color(t.text));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (text, color) = if !m.available {
                    (crate::i18n::tr("Not available on this device"), t.text_dim)
                } else if running {
                    (crate::i18n::tr("Downloading…"), t.text)
                } else if m.installed {
                    (crate::i18n::tr("On this device"), t.accent)
                } else {
                    (crate::i18n::tr("Not downloaded"), t.text_dim)
                };
                let r = ui.label(RichText::new(text).color(color));
                register(ui.ctx(), format!("label:modelStatus-{}", m.id), r.rect);
            });
        });
        hint(ui, t, m.purpose.as_str());
        if !m.available {
            return;
        }
        // where it is, how big
        if m.installed {
            let r = ui.label(RichText::new(size(m.disk_bytes)).color(t.text_label));
            register(ui.ctx(), format!("label:modelStorage-{}", m.id), r.rect);
            hint(ui, t, format!("{} {}", crate::i18n::tr("Stored in"), m.dir.as_deref().unwrap_or_default()));
            if app.services.reveal.is_some()
                && let Some(dir) = &m.dir
            {
                let r = ui.small_button(crate::i18n::tr("Show in Folder"));
                register(ui.ctx(), format!("button:modelReveal-{}", m.id), r.rect);
                if r.clicked()
                    && let Some(reveal) = app.services.reveal.as_mut()
                {
                    let _ = reveal(dir);
                }
            }
        } else {
            let mut line = format!("{} {}", size(m.download_bytes), crate::i18n::tr("download"));
            if m.disk_bytes > 0 && !running {
                line = format!("{line} · {} {}", size(m.disk_bytes), crate::i18n::tr("already downloaded (it resumes)"));
            }
            let r = ui.label(RichText::new(line).color(t.text_label));
            register(ui.ctx(), format!("label:modelStorage-{}", m.id), r.rect);
        }
        let from = if m.sources.is_empty() {
            crate::i18n::tr("No download location is set up for this model yet (see docs/ai-masks.md).").to_string()
        } else {
            format!("{} {}", crate::i18n::tr("Downloads from"), m.sources.join(", "))
        };
        let r = ui.label(RichText::new(from).color(t.text_label));
        register(ui.ctx(), format!("label:modelSource-{}", m.id), r.rect);
        // licence and credit
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(format!("{} · {}", m.licence, m.credit)).size(11.0).color(t.text_dim));
            let r = ui.link(crate::i18n::tr("Licence")).on_hover_text(m.licence_url.as_str());
            register(ui.ctx(), format!("link:modelLicence-{}", m.id), r.rect);
            if r.clicked() {
                let _ = crate::links::open(app, &m.licence_url);
            }
            let r = ui.link(crate::i18n::tr("About")).on_hover_text(m.home_url.as_str());
            register(ui.ctx(), format!("link:modelAbout-{}", m.id), r.rect);
            if r.clicked() {
                let _ = crate::links::open(app, &m.home_url);
            }
        });
        ui.add_space(2.0);

        // on / off (the search, text and people models are switched on where they are used)
        if m.can_disable {
            let mut on = m.enabled;
            let r = crate::widgets::check(ui, &mut on, crate::i18n::tr("Use this model"));
            register(ui.ctx(), format!("check:modelEnabled-{}", m.id), r.rect);
            if r.changed()
                && let Err(e) = app.run("models.setEnabled", json!({"id": m.id, "enabled": on}))
            {
                app.ui.status = e;
            }
            if !m.enabled {
                hint(ui, t, crate::i18n::tr("Turned off: the feature that uses it won't run until you turn it back on."));
            }
        }

        // download in progress, or the buttons
        if let Some(d) = m.download.as_ref().filter(|d| d.running) {
            let frac = if d.total > 0 { d.done as f64 / d.total as f64 } else { 0.0 };
            let r = ui.add(egui::ProgressBar::new(frac as f32).text(format!("{} / {}", size(d.done), size(d.total))));
            register(ui.ctx(), format!("progress:model-{}", m.id), r.rect);
            let r = ui.button(crate::i18n::tr("Cancel Download"));
            register(ui.ctx(), format!("button:modelCancel-{}", m.id), r.rect);
            if r.clicked() {
                app.run("models.cancel", json!({"id": m.id})).ok();
            }
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
            return;
        }
        if let Some(e) = m.download.as_ref().and_then(|d| d.error.as_ref()) {
            ui.label(RichText::new(format!("{} {e}", crate::i18n::tr("The download didn't work:"))).color(t.caution));
        }
        match step {
            Some(ModelStep::Download) => {
                ui.label(
                    RichText::new(format!(
                        "{} {} {} {}?",
                        crate::i18n::tr("Download"),
                        size(m.download_bytes),
                        crate::i18n::tr("from"),
                        m.sources.join(", ")
                    ))
                    .color(t.text),
                );
                hint(ui, t, format!("{} {} ({}).", crate::i18n::tr("Downloading means accepting its licence:"), m.licence, m.credit));
                ui.horizontal_wrapped(|ui| {
                    let r = ui.button(crate::i18n::tr("Download"));
                    register(ui.ctx(), format!("button:modelDownloadConfirm-{}", m.id), r.rect);
                    if r.clicked() {
                        app.ui.model_confirm = None;
                        match app.run("models.download", json!({"id": m.id, "acknowledged": true})) {
                            Ok(_) => app.ui.sam_downloading |= m.id == "sam3",
                            Err(e) => app.ui.status = e,
                        }
                    }
                    let r = ui.button(crate::i18n::tr("Cancel"));
                    register(ui.ctx(), format!("button:modelStepCancel-{}", m.id), r.rect);
                    if r.clicked() {
                        app.ui.model_confirm = None;
                    }
                });
            }
            Some(ModelStep::Delete) => {
                ui.label(
                    RichText::new(format!(
                        "{} {} · {}",
                        crate::i18n::tr("Delete it from this device? This frees"),
                        size(m.disk_bytes),
                        crate::i18n::tr("You can download it again.")
                    ))
                    .color(t.text),
                );
                ui.horizontal_wrapped(|ui| {
                    let r = ui.button(crate::i18n::tr("Delete"));
                    register(ui.ctx(), format!("button:modelDeleteConfirm-{}", m.id), r.rect);
                    if r.clicked() {
                        app.ui.model_confirm = None;
                        if let Err(e) = app.run("models.delete", json!({"id": m.id, "confirm": true})) {
                            app.ui.status = e;
                        }
                    }
                    let r = ui.button(crate::i18n::tr("Cancel"));
                    register(ui.ctx(), format!("button:modelStepCancel-{}", m.id), r.rect);
                    if r.clicked() {
                        app.ui.model_confirm = None;
                    }
                });
            }
            None => {
                ui.horizontal_wrapped(|ui| {
                    if m.installed {
                        let r = ui.button(crate::i18n::tr("Delete…"));
                        register(ui.ctx(), format!("button:modelDelete-{}", m.id), r.rect);
                        if r.clicked() {
                            app.ui.model_confirm = Some((m.id.clone(), ModelStep::Delete));
                        }
                    } else {
                        let can = !m.sources.is_empty();
                        let r = ui.add_enabled(
                            can,
                            egui::Button::new(crate::i18n::tr(if m.download.as_ref().is_some_and(|d| d.error.is_some()) {
                                "Try Again"
                            } else {
                                "Download…"
                            })),
                        );
                        register(ui.ctx(), format!("button:modelDownload-{}", m.id), r.rect);
                        if r.clicked() {
                            app.ui.model_confirm = Some((m.id.clone(), ModelStep::Download));
                        }
                    }
                });
            }
        }
    });
}
