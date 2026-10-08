//! Search by description: the "Describe" switch on the search field and the panel under it.
//!
//! With the switch on, the search field takes a description ("a dog on a beach at sunset") and
//! Return runs `library.search` instead of matching text on every keystroke. The panel below the
//! field says what is missing — the model (with its size and licence, and a download that needs
//! the user's click), the photos' index (built in the background) — and what is happening.
//! Nothing is downloaded or indexed until the user turns the switch on and agrees.

use egui::{Align2, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use serde_json::json;

use crate::LightcraftApp;
use crate::theme::Tokens;
use crate::widgets::register;

const PILL_W: f32 = 78.0;

/// Room the switch takes at the search field's right end (none where search by description doesn't
/// exist: the web build).
pub fn pill_width(app: &LightcraftApp) -> f32 {
    if app.session.vision.available() { PILL_W + 8.0 } else { 0.0 }
}

/// An engine error without its "invalid parameters for `cmd`:" lead-in.
fn plain(e: &str) -> String {
    e.split_once("`: ").map_or(e, |(_, rest)| rest).to_string()
}

/// Photos the index can hold (distinct by content), cached per catalog revision.
fn total_photos(app: &mut LightcraftApp) -> usize {
    let rev = app.session.catalog.revision;
    if app.ui.ai_search_total.0 != rev || app.ui.ai_search_total.1 == 0 {
        app.ui.ai_search_total = (rev, app.session.vision_photo_count());
    }
    app.ui.ai_search_total.1
}

/// The switch, drawn inside the search field's right end.
pub fn pill(app: &mut LightcraftApp, ui: &mut egui::Ui, field: Rect) {
    if !app.session.vision.available() {
        return;
    }
    let t = Tokens::get(ui.ctx());
    let r = Rect::from_center_size(pos2(field.right() - 6.0 - PILL_W / 2.0, field.center().y), vec2(PILL_W, 20.0));
    let resp = ui.interact(r, egui::Id::new("describe-pill"), Sense::click());
    let on = app.ui.ai_search;
    let label = crate::i18n::tr("Describe");
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, label));
    register(ui.ctx(), "button:describeSearch", r);
    let (fill, line, ink) = match (on, resp.hovered()) {
        (true, _) => (t.accent, t.accent, egui::Color32::WHITE),
        (false, true) => (t.hover, t.field_border, t.text),
        (false, false) => (egui::Color32::TRANSPARENT, t.field_border, t.text_dim),
    };
    ui.painter().rect(r, 10.0, fill, Stroke::new(1.0, line), StrokeKind::Inside);
    ui.painter().text(r.center(), Align2::CENTER_CENTER, label, t.font(11.5), ink);
    if resp.on_hover_text(crate::i18n::tr("Search by describing the photo, in your own words")).clicked() {
        toggle(app);
    }
}

fn toggle(app: &mut LightcraftApp) {
    app.ui.ai_search = !app.ui.ai_search;
    app.ui.ai_search_error = None;
    app.ui.ai_search_autostarted = false;
    // the field starts afresh: its old text meant something else in the other mode
    app.ui.search.clear();
    app.ui.focus_search = true;
    let mut patch = json!({"text": ""});
    if app.session.filter.semantic.is_some() {
        patch = json!({"text": "", "semantic": null, "only": []});
    }
    let _ = app.run("library.filter", patch);
}

/// Return pressed in the field with the switch on.
pub fn submit(app: &mut LightcraftApp) {
    let q = app.ui.search.trim().to_string();
    if q.is_empty() {
        return clear(app);
    }
    app.ui.ai_search_error = None;
    if let Err(e) = app.run("library.search", json!({"q": q})) {
        app.ui.ai_search_error = Some(plain(&e));
    }
}

/// Leave the search results (the field was emptied).
pub fn clear(app: &mut LightcraftApp) {
    if app.session.filter.semantic.is_some() {
        let _ = app.run("library.filter", json!({"semantic": null, "only": []}));
    }
}

/// What the panel under the field has to say.
enum Panel {
    Error(String),
    Install,
    Downloading,
    Indexing {
        done: usize,
        total: usize,
        /// `describing`, `reading` (the text in photos) or `finding` (faces).
        phase: &'static str,
    },
    /// Asking whether to download the models that read the text in photos.
    TextInstall,
    TextDownloading,
    Partial {
        indexed: usize,
        total: usize,
    },
    /// The sync server can search, but its model isn't installed (this device has none either).
    ServerNoModel,
    /// The sync server is getting the photos ready (this device searches through it).
    ServerIndexing {
        indexed: usize,
        total: usize,
    },
    Searching,
    Hint,
}

impl Panel {
    /// Which panel this is, for the widget id (`panel:describe:<name>`).
    fn name(&self) -> &'static str {
        match self {
            Panel::Error(_) => "error",
            Panel::Install => "install",
            Panel::Downloading => "downloading",
            Panel::Indexing { .. } => "indexing",
            Panel::TextInstall => "textInstall",
            Panel::TextDownloading => "textDownloading",
            Panel::Partial { .. } => "partial",
            Panel::ServerNoModel => "serverNoModel",
            Panel::ServerIndexing { .. } => "serverIndexing",
            Panel::Searching => "searching",
            Panel::Hint => "hint",
        }
    }
}

/// Once per frame: apply finished work (a search, a download, a finished index), announce what
/// the user should hear, and keep repainting while something runs.
pub fn frame(app: &mut LightcraftApp, ctx: &egui::Context) {
    let polled = app.session.vision_poll();
    if polled.changed {
        ctx.request_repaint();
    }
    if let Some(m) = polled.messages.into_iter().last() {
        app.toast_for(ctx, m, 4.0);
    }
    let v = &app.session.vision;
    let download = v.download_status();
    let downloading = download["running"].as_bool() == Some(true);
    if v.busy() || v.searching() {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
    if downloading {
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }
    // a download that ended: say so, and get the photos indexed
    if app.ui.ai_search_downloading && !downloading {
        app.ui.ai_search_downloading = false;
        match (download["error"].as_str(), download["finished"].as_bool() == Some(true)) {
            (Some(e), _) if e.contains("cancelled") => {}
            (Some(e), _) => app.ui.ai_search_error = Some(format!("{} {e}", crate::i18n::tr("The download didn't work:"))),
            (None, true) => app.toast_for(ctx, crate::i18n::tr("The search model is installed: describe a photo to find it."), 4.0),
            (None, false) => {}
        }
    }
    // the text-reading models arrived: say so, and start reading
    let text_download = app.session.vision.text_download_status();
    let text_downloading = text_download["running"].as_bool() == Some(true);
    if text_downloading {
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }
    if app.ui.ai_text_downloading && !text_downloading {
        app.ui.ai_text_downloading = false;
        match (text_download["error"].as_str(), text_download["finished"].as_bool() == Some(true)) {
            (Some(e), _) if e.contains("cancelled") => {
                let _ = app.run("vision.setText", json!({"on": false}));
            }
            (Some(e), _) => {
                let _ = app.run("vision.setText", json!({"on": false}));
                app.ui.ai_search_error = Some(format!("{} {e}", crate::i18n::tr("The download didn't work:")));
            }
            (None, true) => {
                app.toast_for(ctx, crate::i18n::tr("Text search is ready: your photos are being read."), 4.0);
                let _ = app.run("vision.index", json!({}));
            }
            (None, false) => {}
        }
    }
    // switched on with the model in place: index what isn't yet, once
    let v = &app.session.vision;
    let idle = v.job().is_none_or(|j| j.finished.load(std::sync::atomic::Ordering::Relaxed));
    if app.ui.ai_search && !app.ui.ai_search_autostarted && v.installed() && idle && !downloading {
        app.ui.ai_search_autostarted = true;
        let (indexed, total) = (v.indexed(), total_photos(app));
        let unread = app.session.vision.text_ready() && app.session.vision.text_indexed() < total;
        if indexed < total || unread {
            let _ = app.run("vision.index", json!({}));
        }
    }
}

/// The panel under the search field.
pub fn panel(app: &mut LightcraftApp, ctx: &egui::Context, field: Rect) {
    if !app.ui.ai_search || !app.session.vision.available() {
        return;
    }
    let v = &app.session.vision;
    let download = v.download_status();
    let running_job = v
        .job()
        .filter(|j| !j.finished.load(std::sync::atomic::Ordering::Relaxed))
        .map(|j| (j.done.load(std::sync::atomic::Ordering::Relaxed), j.total));
    let (installed, searching, indexed) = (v.installed(), v.searching(), v.indexed());
    let (local, server_ready) = (v.local_available(), v.server_ready());
    // what the sync server has done, when this device searches through it
    let server = v.server_status().map(|s| (s["indexed"].as_u64().unwrap_or(0) as usize, s["total"].as_u64().unwrap_or(0) as usize));
    let total = if installed { total_photos(app) } else { 0 };
    let which = if let Some(e) = app.ui.ai_search_error.clone() {
        Panel::Error(e)
    } else if download["running"].as_bool() == Some(true) {
        Panel::Downloading
    } else if app.session.vision.text_download_status()["running"].as_bool() == Some(true) {
        Panel::TextDownloading
    } else if app.ui.ai_text_offer && !app.session.vision.text_installed() {
        Panel::TextInstall
    } else if !installed && !server_ready {
        if local { Panel::Install } else { Panel::ServerNoModel }
    } else if !installed {
        // no model on this device: the server's
        match server {
            _ if searching => Panel::Searching,
            Some((indexed, total)) if indexed < total => Panel::ServerIndexing { indexed, total },
            _ if app.ui.search.trim().is_empty() => Panel::Hint,
            _ => return,
        }
    } else if let Some((done, total)) = running_job {
        let phase = app.session.vision.job().map_or("describing", |j| j.phase());
        Panel::Indexing { done, total, phase }
    } else if searching {
        Panel::Searching
    } else if indexed < total {
        Panel::Partial { indexed, total }
    } else if app.ui.search.trim().is_empty() {
        Panel::Hint
    } else {
        return;
    };
    let t = Tokens::get(ctx);
    let shown = egui::Area::new(egui::Id::new("describe-panel"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos2(field.left(), field.bottom() + 6.0))
        .show(ctx, |ui| {
            egui::Frame::new().fill(t.chrome).stroke(Stroke::new(1.0, t.field_border)).corner_radius(6.0).inner_margin(12.0).show(ui, |ui| {
                ui.set_width((field.width() - 24.0).max(240.0));
                ui.spacing_mut().item_spacing.y = 6.0;
                body(app, ui, &which, &download, &t);
            });
        });
    register(ctx, format!("panel:describe:{}", which.name()), shown.response.rect);
}

/// For a device that has the model, next to a server that can search: whether to send it the
/// search data, so the server and the user's other devices needn't compute it again.
fn share_row(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let v = &app.session.vision;
    if !v.installed() || !v.server_status().is_some_and(|s| s["available"].as_bool() == Some(true)) {
        return;
    }
    let mut on = v.share_with_server;
    let r = crate::widgets::check(ui, &mut on, crate::i18n::tr("Send the search data to my server, so my other devices can search too"));
    register(ui.ctx(), "check:describeShare", r.rect);
    if r.changed() {
        let _ = app.run("vision.setShare", json!({"on": on}));
    }
}

/// For a device that has the model: whether search also reads the words printed in photos. Ticking
/// it asks first when the models (a small download of their own) aren't there yet.
fn text_row(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let v = &app.session.vision;
    if !v.local_available() || !v.text_available() {
        return;
    }
    let mut on = v.text || app.ui.ai_text_offer;
    let r = crate::widgets::check(ui, &mut on, crate::i18n::tr("Also find words in photos (signs, menus, documents)"));
    register(ui.ctx(), "check:describeText", r.rect);
    if !r.changed() {
        return;
    }
    if !on {
        app.ui.ai_text_offer = false;
        let _ = app.run("vision.setText", json!({"on": false}));
    } else if app.session.vision.text_installed() {
        let _ = app.run("vision.setText", json!({"on": true}));
        let _ = app.run("vision.index", json!({}));
    } else {
        app.ui.ai_text_offer = true;
    }
}

fn body(app: &mut LightcraftApp, ui: &mut egui::Ui, which: &Panel, download: &serde_json::Value, t: &Tokens) {
    use lightcraft_engine::vision::{LICENSE_NAME, LICENSE_URL, MODEL_BYTES};
    let gb = |b: u64| b as f64 / 1e9;
    match which {
        Panel::Error(e) => {
            ui.label(egui::RichText::new(e).color(egui::Color32::from_rgb(230, 90, 80)));
            let r = ui.button(crate::i18n::tr("OK"));
            register(ui.ctx(), "button:describeDismiss", r.rect);
            if r.clicked() {
                app.ui.ai_search_error = None;
            }
        }
        Panel::Install => {
            ui.label(crate::i18n::tr(
                "Search by description uses an AI model that understands photos and words. It isn't part of LightCraft, and everything else works without it.",
            ));
            let dir = app.session.vision.dir.as_ref().map(|d| d.display().to_string()).unwrap_or_default();
            ui.label(format!("{} {:.1} GB, {} {dir}", crate::i18n::tr("A one-time download of about"), gb(MODEL_BYTES), crate::i18n::tr("saved in")));
            ui.label(
                egui::RichText::new(format!(
                    "{} {LICENSE_NAME} — {}",
                    crate::i18n::tr("Licence:"),
                    crate::i18n::tr("Google's terms, not LightCraft's. Downloading it means accepting them.")
                ))
                .color(t.text_label),
            );
            ui.label(
                egui::RichText::new(crate::i18n::tr(
                    "Your photos are then indexed on this computer. Nothing is sent anywhere unless you choose to share it with your own server.",
                ))
                .color(t.text_dim),
            );
            let r = ui.link(crate::i18n::tr("Read the licence")).on_hover_text(LICENSE_URL);
            register(ui.ctx(), "link:describeLicense", r.rect);
            if r.clicked() {
                let _ = crate::links::open(app, LICENSE_URL);
            }
            if app.session.vision.mirrors().is_empty() {
                ui.label(egui::RichText::new(crate::i18n::tr("This build has no download location for the model yet.")).color(t.text_dim));
            } else {
                let r = ui.button(crate::i18n::tr("Download and Turn On"));
                register(ui.ctx(), "button:describeDownload", r.rect);
                if r.clicked() {
                    match app.run("vision.model.download", json!({"acknowledged": true})) {
                        Ok(_) => app.ui.ai_search_downloading = true,
                        Err(e) => app.ui.ai_search_error = Some(plain(&e)),
                    }
                }
            }
        }
        Panel::Downloading => {
            let (done, total) = (download["done"].as_u64().unwrap_or(0), download["total"].as_u64().unwrap_or(0));
            let frac = if total > 0 { done as f64 / total as f64 } else { 0.0 };
            let file = download["file"].as_str().unwrap_or("");
            let r = ui.add(egui::ProgressBar::new(frac as f32).text(format!("{:.2} / {:.2} GB · {file}", gb(done), gb(total))));
            register(ui.ctx(), "progress:describeDownload", r.rect);
            let r = ui.button(crate::i18n::tr("Cancel Download"));
            register(ui.ctx(), "button:describeCancel", r.rect);
            if r.clicked() {
                app.session.vision.cancel_download();
            }
            ui.label(egui::RichText::new(crate::i18n::tr("The download continues if you close this, and resumes if interrupted.")).color(t.text_dim));
        }
        Panel::Indexing { done, total, phase } => {
            let frac = if *total > 0 { *done as f32 / *total as f32 } else { 0.0 };
            let text = match *phase {
                "reading" => crate::i18n::tr_format!("Reading the text in your photos: {done} of {total}", done = done, total = total),
                "finding" => crate::i18n::tr_format!("Looking for faces: {done} of {total}", done = done, total = total),
                _ => crate::i18n::tr_format!("Getting your photos ready to search: {done} of {total}", done = done, total = total),
            };
            let r = ui.add(egui::ProgressBar::new(frac).text(text));
            register(ui.ctx(), "progress:describeIndex", r.rect);
            let r = ui.button(crate::i18n::tr("Stop"));
            register(ui.ctx(), "button:describeStop", r.rect);
            if r.clicked() {
                let _ = app.run("vision.indexCancel", json!({}));
            }
            ui.label(egui::RichText::new(crate::i18n::tr("You can search the photos that are ready now.")).color(t.text_dim));
        }
        Panel::Partial { indexed, total } => {
            ui.label(crate::i18n::tr_format!("{indexed} of {total} photos can be searched.", indexed = indexed, total = total));
            text_row(app, ui);
            share_row(app, ui);
            let r = ui.button(crate::i18n::tr("Prepare the Rest"));
            register(ui.ctx(), "button:describeIndex", r.rect);
            if r.clicked()
                && let Err(e) = app.run("vision.index", json!({}))
            {
                app.ui.ai_search_error = Some(plain(&e));
            }
        }
        Panel::ServerNoModel => {
            ui.label(crate::i18n::tr("Your server can search by description, but its search model isn't installed yet."));
            ui.label(
                egui::RichText::new(crate::i18n::tr("Whoever runs the server installs it with: lightcraft-server model download --accept-licences"))
                    .color(t.text_dim),
            );
        }
        Panel::ServerIndexing { indexed, total } => {
            ui.label(crate::i18n::tr_format!("Your server has {indexed} of {total} photos ready to search.", indexed = indexed, total = total));
            ui.label(egui::RichText::new(crate::i18n::tr("It keeps going in the background; search the ones that are ready now.")).color(t.text_dim));
        }
        Panel::TextInstall => {
            use lightcraft_engine::vision::{TEXT_BYTES, TEXT_LICENSE_NAME, TEXT_LICENSE_URL};
            ui.label(crate::i18n::tr(
                "Finding words in photos uses two small AI models that read text, in English, Chinese, Japanese and many other languages. They aren't part of LightCraft, and everything else works without them.",
            ));
            let dir = app.session.vision.text_dir().map(|d| d.display().to_string()).unwrap_or_default();
            ui.label(format!(
                "{} {:.0} MB, {} {dir}",
                crate::i18n::tr("A one-time download of about"),
                TEXT_BYTES as f64 / 1e6,
                crate::i18n::tr("saved in")
            ));
            ui.label(
                egui::RichText::new(format!(
                    "{} {TEXT_LICENSE_NAME} — {}",
                    crate::i18n::tr("Licence:"),
                    crate::i18n::tr("Baidu's terms, not LightCraft's. Downloading them means accepting them.")
                ))
                .color(t.text_label),
            );
            ui.label(
                egui::RichText::new(crate::i18n::tr(
                    "Your photos are then read on this computer, a few at a time in the background; it takes a while. Nothing is sent anywhere unless you choose to share it with your own server.",
                ))
                .color(t.text_dim),
            );
            let r = ui.link(crate::i18n::tr("Read the licence")).on_hover_text(TEXT_LICENSE_URL);
            register(ui.ctx(), "link:describeTextLicense", r.rect);
            if r.clicked() {
                let _ = crate::links::open(app, TEXT_LICENSE_URL);
            }
            ui.horizontal(|ui| {
                let r = ui.button(crate::i18n::tr("Download and Turn On"));
                register(ui.ctx(), "button:describeTextDownload", r.rect);
                if r.clicked() {
                    match app.run("vision.text.download", json!({"acknowledged": true})) {
                        Ok(_) => {
                            app.ui.ai_text_offer = false;
                            app.ui.ai_text_downloading = true;
                            let _ = app.run("vision.setText", json!({"on": true}));
                        }
                        Err(e) => app.ui.ai_search_error = Some(plain(&e)),
                    }
                }
                let r = ui.button(crate::i18n::tr("Not Now"));
                register(ui.ctx(), "button:describeTextLater", r.rect);
                if r.clicked() {
                    app.ui.ai_text_offer = false;
                }
            });
        }
        Panel::TextDownloading => {
            let d = app.session.vision.text_download_status();
            let (done, total) = (d["done"].as_u64().unwrap_or(0), d["total"].as_u64().unwrap_or(0));
            let frac = if total > 0 { done as f64 / total as f64 } else { 0.0 };
            let r = ui.add(egui::ProgressBar::new(frac as f32).text(format!("{:.1} / {:.1} MB", done as f64 / 1e6, total as f64 / 1e6)));
            register(ui.ctx(), "progress:describeTextDownload", r.rect);
            let r = ui.button(crate::i18n::tr("Cancel Download"));
            register(ui.ctx(), "button:describeTextCancel", r.rect);
            if r.clicked() {
                app.session.vision.cancel_text_download();
            }
        }
        Panel::Searching => {
            ui.label(egui::RichText::new(crate::i18n::tr("Searching…")).color(t.text_label));
        }
        Panel::Hint => {
            text_row(app, ui);
            share_row(app, ui);
            ui.label(
                egui::RichText::new(crate::i18n::tr("Describe what you're looking for, for example “a dog on a beach at sunset”, and press Return."))
                    .color(t.text_dim),
            );
        }
    }
}
