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
    Indexing { done: usize, total: usize },
    Partial { indexed: usize, total: usize },
    Searching,
    Hint,
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
    // switched on with the model in place: index what isn't yet, once
    let v = &app.session.vision;
    let idle = v.job().is_none_or(|j| j.finished.load(std::sync::atomic::Ordering::Relaxed));
    if app.ui.ai_search && !app.ui.ai_search_autostarted && v.installed() && idle && !downloading {
        app.ui.ai_search_autostarted = true;
        let (indexed, total) = (v.indexed(), total_photos(app));
        if indexed < total {
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
    let total = if installed { total_photos(app) } else { 0 };
    let which = if let Some(e) = app.ui.ai_search_error.clone() {
        Panel::Error(e)
    } else if download["running"].as_bool() == Some(true) {
        Panel::Downloading
    } else if !installed {
        Panel::Install
    } else if let Some((done, total)) = running_job {
        Panel::Indexing { done, total }
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
    egui::Area::new(egui::Id::new("describe-panel")).order(egui::Order::Foreground).fixed_pos(pos2(field.left(), field.bottom() + 6.0)).show(
        ctx,
        |ui| {
            egui::Frame::new().fill(t.chrome).stroke(Stroke::new(1.0, t.field_border)).corner_radius(6.0).inner_margin(12.0).show(ui, |ui| {
                ui.set_width((field.width() - 24.0).max(240.0));
                ui.spacing_mut().item_spacing.y = 6.0;
                body(app, ui, &which, &download, &t);
            });
        },
    );
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
                egui::RichText::new(crate::i18n::tr("Your photos are then indexed on this computer; nothing about them leaves it."))
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
        Panel::Indexing { done, total } => {
            let frac = if *total > 0 { *done as f32 / *total as f32 } else { 0.0 };
            let text = crate::i18n::tr_format!("Getting your photos ready to search: {done} of {total}", done = done, total = total);
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
            let r = ui.button(crate::i18n::tr("Prepare the Rest"));
            register(ui.ctx(), "button:describeIndex", r.rect);
            if r.clicked()
                && let Err(e) = app.run("vision.index", json!({}))
            {
                app.ui.ai_search_error = Some(plain(&e));
            }
        }
        Panel::Searching => {
            ui.label(egui::RichText::new(crate::i18n::tr("Searching…")).color(t.text_label));
        }
        Panel::Hint => {
            ui.label(
                egui::RichText::new(crate::i18n::tr("Describe what you're looking for, for example “a dog on a beach at sunset”, and press Return."))
                    .color(t.text_dim),
            );
        }
    }
}
