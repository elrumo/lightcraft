//! AI Super Resolution in the UI: the Photo ▸ Enhance… dialog and the background job.
//!
//! The dialog has three states: the model isn't installed (what it is, its size, licence and
//! credit; nothing downloads without the user pressing Download), it is downloading (progress),
//! and it is ready (the photo's size before and after, and Enlarge). Enlarge runs it in the
//! background ([`lightcraft_apphost::superres`]: progress and cancel on its activity row); when it
//! finishes the result is written, imported and selected.

use lightcraft_engine::enhance::{DEFAULT_MAX_OUTPUT_PIXELS, SUPER_RES_MODEL, SUPER_RES_SCALE};
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::state::Dialog;
use crate::theme::Tokens;

/// Super Resolution state of the app.
#[derive(Default)]
pub struct SuperResState {
    /// The enlargement under way (its progress is its activity row).
    pub task: Option<lightcraft_apphost::superres::Running>,
    /// The last finished enlargement (`{id, path, original, width, height}`).
    pub last_result: Option<Value>,
}

impl SuperResState {
    pub fn busy(&self) -> bool {
        self.task.is_some()
    }
}

/// Where the model stands, which decides the dialog's buttons: `(installed, downloading, failed)`.
fn state(app: &LightcraftApp) -> (bool, bool, bool) {
    let e = &app.session.enhancer;
    let (id, d) = e.download_status();
    let mine = id.as_deref() == Some(SUPER_RES_MODEL);
    (e.installed(SUPER_RES_MODEL), mine && d.running, mine && d.error.is_some())
}

/// Open the dialog for the active photo (Photo ▸ Enhance…).
pub fn open(app: &mut LightcraftApp) -> Result<Value, String> {
    if !lightcraft_engine::enhance::Enhancer::AVAILABLE {
        return Err(crate::i18n::tr("AI Super Resolution is not available in this build").to_string());
    }
    let id = app.session.active().ok_or_else(|| crate::i18n::tr("Select a photo to enlarge").to_string())?;
    if app.session.enhancer.disabled.contains(SUPER_RES_MODEL) {
        return Err(crate::i18n::tr("Super Resolution is turned off: turn its model on in Settings → AI Models.").to_string());
    }
    app.ui.dialog = Some(Dialog::SuperRes { error: None });
    Ok(json!({"photo": id.0}))
}

/// The dialog's dismiss button.
pub fn cancel_label(app: &LightcraftApp) -> &'static str {
    let (installed, running, _) = state(app);
    if running || installed { "Close" } else { "Not Now" }
}

/// The dialog's action; empty when there is none (while the download runs).
pub fn ok_label(app: &LightcraftApp, error: Option<&String>) -> &'static str {
    let (installed, running, failed) = state(app);
    if installed {
        "Enlarge"
    } else if running {
        ""
    } else if error.is_some() || failed {
        "Try Again"
    } else {
        "Download"
    }
}

/// Whether the dialog stays open after its action (the download starts, then shows progress).
pub fn keeps_open(app: &LightcraftApp) -> bool {
    !app.session.enhancer.installed(SUPER_RES_MODEL)
}

/// The dialog's action: download the model (the user has pressed Download), or enlarge.
pub fn confirm(app: &mut LightcraftApp) -> Result<Value, String> {
    if app.session.enhancer.installed(SUPER_RES_MODEL) {
        start(app)
    } else {
        app.run("enhance.model.download", json!({"id": SUPER_RES_MODEL, "acknowledged": true}))
    }
}

/// Start enlarging the active photo on a worker thread.
pub fn start(app: &mut LightcraftApp) -> Result<Value, String> {
    if app.superres.task.is_some() {
        return Err(crate::i18n::tr("Super Resolution is already running").to_string());
    }
    let id = app.session.active().ok_or_else(|| crate::i18n::tr("Select a photo to enlarge").to_string())?;
    app.superres.task = Some(lightcraft_apphost::superres::Running::start(&mut app.session, id, None)?);
    Ok(json!({"started": true, "photo": id.0}))
}

/// Stop the running enlargement. Whether one was running.
pub fn cancel(app: &mut LightcraftApp) -> bool {
    match &app.superres.task {
        Some(t) => {
            t.cancel();
            true
        }
        None => false,
    }
}

/// Per frame: add a finished enlargement to the library.
pub fn poll(app: &mut LightcraftApp, ctx: &egui::Context) {
    let Some(t) = &app.superres.task else { return };
    match t.poll(&mut app.session) {
        Some(r) => {
            app.superres.task = None;
            finish(app, ctx, r);
        }
        None => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
    }
}

fn finish(app: &mut LightcraftApp, ctx: &egui::Context, r: Result<Value, String>) {
    match r {
        Ok(v) => {
            app.superres.last_result = Some(v);
            app.toast(ctx, crate::i18n::tr("Super Resolution added"));
        }
        Err(e) if e.contains("cancelled") => app.toast(ctx, crate::i18n::tr("Super Resolution cancelled")),
        Err(e) => {
            app.ui.status = e.clone();
            app.toast_error(ctx, crate::i18n::tr_format!("Super Resolution failed: {e}", e = e));
        }
    }
}

/// The dialog's body.
pub fn body(app: &mut LightcraftApp, ui: &mut egui::Ui, error: Option<&str>) {
    let t = Tokens::get(ui.ctx());
    let (installed, running, _) = state(app);
    let mb = |b: u64| b as f64 / 1e6;
    let model = app.session.enhancer.status().into_iter().find(|m| m.id == SUPER_RES_MODEL);
    ui.set_max_width(460.0);

    if installed {
        let photo = app.session.active().and_then(|id| app.session.catalog.photo(id));
        match photo {
            Some(p) => {
                let (w, h) = lightcraft_engine::export::output_size(p, &Default::default());
                let (ow, oh) = (w * SUPER_RES_SCALE, h * SUPER_RES_SCALE);
                let limit = app.session.enhancer.max_output_pixels.unwrap_or(DEFAULT_MAX_OUTPUT_PIXELS);
                ui.label(egui::RichText::new(p.file_name.clone()).strong());
                let mp = (ow * oh) as f64 / 1e6;
                let r = ui.label(format!("{w} × {h}  →  {ow} × {oh}  ({})", if mp < 1.0 { "< 1 MP".to_string() } else { format!("{mp:.0} MP") }));
                crate::widgets::register(ui.ctx(), "label:superResSize", r.rect);
                if ow.saturating_mul(oh) > limit {
                    ui.label(
                        egui::RichText::new(format!(
                            "{} {:.0} {}",
                            crate::i18n::tr("This is too large for Super Resolution here. Crop the photo first: the limit is"),
                            limit as f64 / 1e6,
                            crate::i18n::tr("megapixels.")
                        ))
                        .color(egui::Color32::from_rgb(230, 90, 80)),
                    );
                }
            }
            None => {
                ui.label(crate::i18n::tr("Select a photo to enlarge"));
            }
        }
        ui.add_space(4.0);
        ui.label(crate::i18n::tr(
            "The photo is enlarged to twice its width and height with an AI model, with your edits applied, and added next to the original (a 16-bit sRGB TIFF), stacked with it. The original isn't changed. This takes a while on large photos.",
        ));
        if let Some(m) = &model {
            ui.label(egui::RichText::new(format!("{} · {}", m.credit, m.licence)).color(t.text_dim));
        }
        return;
    }

    ui.label(crate::i18n::tr(
        "Super Resolution enlarges a photo to twice its width and height with an AI model. The model isn't part of LightCraft, and everything else works without it.",
    ));
    if let Some(m) = &model {
        let dir = m.dir.clone().unwrap_or_default();
        ui.label(format!("{} {:.1} MB, {} {dir}", crate::i18n::tr("A one-time download of about"), mb(m.size_bytes), crate::i18n::tr("saved in")));
        ui.label(egui::RichText::new(format!("{} {}", m.credit, crate::i18n::tr("— licence:"))).color(t.text_label));
        ui.horizontal(|ui| {
            let r = ui.link(m.licence.as_str()).on_hover_text(m.licence_url.as_str());
            crate::widgets::register(ui.ctx(), "link:superResLicence", r.rect);
            if r.clicked() {
                let _ = crate::links::open(app, &m.licence_url);
            }
            let r = ui.link(crate::i18n::tr("About the model")).on_hover_text(m.home_url.as_str());
            crate::widgets::register(ui.ctx(), "link:superResModel", r.rect);
            if r.clicked() {
                let _ = crate::links::open(app, &m.home_url);
            }
        });
    }
    let (fetching, d) = app.session.enhancer.download_status();
    let mine = fetching.as_deref() == Some(SUPER_RES_MODEL);
    if running {
        ui.add_space(4.0);
        let frac = if d.total > 0 { d.done as f64 / d.total as f64 } else { 0.0 };
        let r = ui.add(egui::ProgressBar::new(frac as f32).text(format!("{:.2} / {:.2} MB", mb(d.done), mb(d.total))));
        crate::widgets::register(ui.ctx(), "progress:superResDownload", r.rect);
        let r = ui.button(crate::i18n::tr("Cancel Download"));
        crate::widgets::register(ui.ctx(), "button:superResCancel", r.rect);
        if r.clicked() {
            app.session.enhancer.cancel_download();
        }
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
        return;
    }
    if let Some(e) = error.map(str::to_string).or_else(|| mine.then_some(d.error).flatten()) {
        ui.label(egui::RichText::new(format!("{} {e}", crate::i18n::tr("The download didn't work:"))).color(egui::Color32::from_rgb(230, 90, 80)));
    }
}
