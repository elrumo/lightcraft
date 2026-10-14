//! Generative Expand in the UI (`lightcraft_engine::expand`): the crop tool's **Expand** switch,
//! the choice of what paints the new area (Automatic, or one of the models this device runs: MI-GAN,
//! LaMa, Stable Diffusion 1.5, Kandinsky 2.2, SDXL, Classic; the same choice as Settings → AI
//! Models), each model's state and download offer, and **Generate**, which runs in the background
//! with a row in the activity stack (✕ there, or Cancel here, stops it). The desktop Crop panel
//! ([`panel`]) and the phone's crop screen (`panels::crop_mobile`) share it.
//!
//! The area of the crop past the photo that has nothing yet is transparent in the render, and the
//! loupe shows a checkerboard through it ([`checkerboard`]), as Lightroom does.

use egui::{Color32, Pos2, Rect, Shape, pos2};
use lightcraft_catalog::PhotoId;
use lightcraft_develop::DevelopSettings;
use lightcraft_engine::expand::{ExpandChoice, ExpandModel};
use serde_json::json;

use crate::LightcraftApp;
use crate::i18n::{tr, tr_format};
use crate::theme::Tokens;

/// What Generate paints with: the user's pick if this device runs it, else Automatic's (the best
/// model for this device, which may still need its download).
pub fn chosen(app: &LightcraftApp) -> ExpandModel {
    app.session.chosen_expand_model()
}

/// Choose what paints (`None`: Automatic), for the session and the saved settings.
pub fn choose(app: &mut LightcraftApp, m: Option<ExpandModel>) {
    match app.run("crop.expandModel", json!({"model": m.map_or("auto", |m| m.id())})) {
        Ok(_) => app.ui.expand_model = m.map(|m| m.id().to_string()).unwrap_or_default(),
        Err(e) => app.ui.status = e,
    }
}

/// The models this device runs, in the pickers' order.
pub fn offered(app: &LightcraftApp) -> Vec<ExpandModel> {
    ExpandModel::ALL.into_iter().filter(|m| app.session.expand_available(*m)).collect()
}

/// A model's name on a picker's button.
pub fn short_label(m: ExpandModel) -> &'static str {
    match m {
        ExpandModel::Sd15 => "SD 1.5",
        ExpandModel::Kandinsky => "Kandinsky",
        m => m.label(),
    }
}

/// The saved choice is applied to the session on the first frame (the settings load after the
/// app is built); from then on the session's (changed here, in Settings or by an agent) is saved.
pub fn sync_choice(app: &mut LightcraftApp) {
    if !app.expand_synced {
        app.expand_synced = true;
        let saved = ExpandModel::from_id(&app.ui.expand_model).filter(|m| app.session.expand_available(*m));
        let _ = app.session.set_expand_preference(saved);
        return;
    }
    let now = app.session.expand.preferred.map(|m| m.id().to_string()).unwrap_or_default();
    if now != app.ui.expand_model {
        app.ui.expand_model = now;
    }
}

/// The choice for `m` as the engine describes it.
fn choice(app: &LightcraftApp, m: ExpandModel) -> Option<ExpandChoice> {
    app.session.expand_choices().into_iter().find(|c| c.id == m.id())
}

/// The photo's oriented size.
fn dims(app: &LightcraftApp, id: PhotoId) -> (f64, f64) {
    let Some(p) = app.session.catalog.photo(id) else { return (3.0, 2.0) };
    let (w, h) = (p.width.max(1) as f64, p.height.max(1) as f64);
    if p.develop.orientation.swaps_axes() { (h, w) } else { (w, h) }
}

/// Whether the crop of `d` shows area past the photo that nothing fills yet.
pub fn empty_area(app: &LightcraftApp, id: PhotoId, d: &DevelopSettings) -> bool {
    if !d.crop.expand {
        return false;
    }
    let (w, h) = dims(app, id);
    let frame = lightcraft_pipeline::geometry::Frame::with_lens(w as usize, h as usize, d, true, app.session.embedded_lens(id).as_ref());
    lightcraft_pipeline::expand::missing(&frame, 48, 48).is_some()
}

/// Whether the crop of `d` reaches past the photo at all.
pub fn reaches_past(app: &LightcraftApp, id: PhotoId, d: &DevelopSettings) -> bool {
    let (w, h) = dims(app, id);
    d.crop.expand && d.crop.geometry.reaches_past(w, h)
}

/// Turn Expand on or off (`crop.expand`).
pub fn set_expand(app: &mut LightcraftApp, on: bool) {
    let _ = app.run("crop.expand", json!({"on": on}));
}

/// Start Generate with the chosen model, or say why it can't.
pub fn generate(app: &mut LightcraftApp, ctx: &egui::Context) {
    let model = chosen(app);
    if !app.session.expand_ready(model) {
        app.toast_for(ctx, tr("This model isn't downloaded yet: download it, or choose Classic."), 4.0);
        return;
    }
    if cfg!(target_arch = "wasm32") {
        // the browser has no worker for it: say so first, run it once that is on screen
        app.toast_for(ctx, tr("Generating the new area…"), 60.0);
        app.ui.expand_pending = Some((model.id().to_string(), false));
        ctx.request_repaint();
        return;
    }
    run(app, ctx, model.id());
}

fn run(app: &mut LightcraftApp, ctx: &egui::Context, model: &str) {
    match app.run("crop.generate", json!({"model": model, "wait": false})) {
        Ok(_) => crate::haptics::tap(ctx, crate::haptics::Haptic::Light),
        Err(e) => app.toast_error(ctx, e),
    }
}

/// Every frame: start a Generate waiting for its notice, store a finished one (one undo step) and
/// say how it went.
pub fn poll(app: &mut LightcraftApp, ctx: &egui::Context) {
    match app.ui.expand_pending.take() {
        Some((m, false)) => {
            app.ui.expand_pending = Some((m, true));
            ctx.request_repaint();
        }
        Some((m, true)) => {
            app.ui.toast = None;
            run(app, ctx, &m);
        }
        None => {}
    }
    if app.session.expand_running().is_some() {
        ctx.request_repaint_after(std::time::Duration::from_millis(150));
    }
    let Some(r) = app.session.poll_expand() else { return };
    match r {
        Ok(m) => app.toast(ctx, tr_format!("New area painted with {}", m.label())),
        Err(e) if e.starts_with("cancelled") => app.toast(ctx, tr("Generate cancelled")),
        Err(e) => app.toast_error(ctx, format!("{}: {e}", tr_format!("{} failed", tr("Generate")))),
    }
    ctx.request_repaint();
}

/// The running Generate's progress (0..=1), from its activity row.
pub fn progress(app: &LightcraftApp) -> Option<f32> {
    app.session.expand_running()?;
    let row = app.session.activity.list().into_iter().find(|t| t.kind == "expand")?;
    Some(if row.total > 0 { row.done as f32 / row.total as f32 } else { 0.0 })
}

/// The download offer for `m` (size, licence, credit, and what to know first), or `None` when
/// there is nothing to offer.
pub fn offer(app: &LightcraftApp, m: ExpandModel) -> Option<String> {
    let c = choice(app, m)?;
    if c.ready || m == ExpandModel::Classic || !c.available {
        return None;
    }
    let size = crate::panels::ai_models::size(c.download_bytes);
    let mut text = tr_format!("Uses the {} model, a {} download ({} licence). {}.", c.label, size, c.licence, c.credit);
    if c.restricted {
        text = format!("{text} {}", tr("Its licence lists uses that aren't allowed: read it before downloading (Settings → AI Models ▸ Licence)."));
    }
    if c.diffusion && !app.session.device.gpu {
        text = format!("{text} {}", tr("Without a GPU it takes minutes per Generate."));
    }
    Some(text)
}

/// The state of model `m`'s download: `Some(percent)` while it runs.
pub fn downloading(app: &LightcraftApp, m: ExpandModel) -> Option<f64> {
    let (fetching, d) = app.session.enhancer.download_status();
    (fetching.as_deref() == Some(m.id()) && d.running).then(|| if d.total > 0 { d.done as f64 / d.total as f64 * 100.0 } else { 0.0 })
}

/// Download model `m` (the user pressed Download after reading the offer).
pub fn download(app: &mut LightcraftApp, ctx: &egui::Context, m: ExpandModel) {
    if m == ExpandModel::MiGan && app.services.erase_model.is_some() {
        // the browser build fetches MI-GAN itself, into the site's storage
        crate::erase::fetch_model(app, ctx);
        return;
    }
    if !lightcraft_engine::enhance::Enhancer::AVAILABLE {
        app.toast(ctx, tr("Models can't be downloaded in this build: put the file in the models folder (Settings → AI Models)."));
        return;
    }
    if let Err(e) = app.run("models.download", json!({"id": m.id(), "acknowledged": true})) {
        app.toast_error(ctx, e);
    }
}

/// A short line about model `m` for the pickers: its note, and its size when it isn't here.
pub fn describe(app: &LightcraftApp, m: ExpandModel) -> String {
    let Some(c) = choice(app, m) else { return String::new() };
    let note = tr(c.note).to_string();
    if c.ready || m == ExpandModel::Classic {
        note
    } else if c.disabled {
        format!("{note} · {}", tr("Turned off in Settings → AI Models"))
    } else {
        tr_format!("{note} · {size} download", size = crate::panels::ai_models::size(c.download_bytes), note = note)
    }
}

/// What Automatic picks here, for the pickers ("Automatic: SDXL suits this device").
pub fn automatic_line(app: &LightcraftApp) -> String {
    tr_format!("Automatic: {} suits this device", app.session.recommended_expand_model().label())
}

/// The desktop Crop panel's Expand section.
pub fn panel(app: &mut LightcraftApp, ui: &mut egui::Ui, id: PhotoId, d: &DevelopSettings) {
    let t = Tokens::get(ui.ctx());
    let dim = t.text_dim;
    let note = |ui: &mut egui::Ui, text: String| {
        ui.add(egui::Label::new(egui::RichText::new(text).color(dim).size(11.5)).wrap());
    };
    let mut on = d.crop.expand;
    let r = crate::widgets::check(ui, &mut on, crate::i18n::tr_ctx("crop", "Expand"))
        .on_hover_text(tr("Let the crop reach past the photo's edges and paint the new area on this computer"));
    crate::widgets::register(ui.ctx(), "check:cropExpand", r.rect);
    if r.changed() {
        set_expand(app, on);
    }
    if !d.crop.expand {
        return;
    }
    let ctx = ui.ctx().clone();
    // what paints: Automatic, or one of the models this device runs
    let models = offered(app);
    let automatic = app.session.expand.preferred.is_none();
    let mut items: Vec<(&str, &str)> = vec![(tr("Auto"), "auto")];
    items.extend(models.iter().map(|m| (short_label(*m), m.id())));
    ui.label(tr("Fill With"));
    let at = if automatic { Some(0) } else { models.iter().position(|m| *m == chosen(app)).map(|i| i + 1) };
    if let Some(i) = crate::widgets::segmented(ui, "expandModel", &items, at, 4) {
        choose(app, if i == 0 { None } else { models.get(i - 1).copied() });
    }
    let current = chosen(app);
    if app.session.expand.preferred.is_none() {
        note(ui, automatic_line(app));
    }
    note(ui, format!("{} · {}", current.label(), describe(app, current)));
    let running = app.session.expand_running();
    if current == ExpandModel::MiGan && !app.session.expand_ready(current) {
        // the browser build: is it in the site's storage already?
        crate::erase::look_in_storage(app, &ctx);
    }
    if let Some(pct) = downloading(app, current) {
        note(ui, tr_format!("{stage}… {:.0}%", pct, stage = tr("Downloading")));
        if crate::widgets::text_button(ui, "expandCancelDownload", tr("Cancel Download"), false).clicked() {
            let _ = app.run("models.cancel", json!({"id": current.id()}));
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
    } else if let Some(text) = offer(app, current) {
        note(ui, text);
        if crate::widgets::text_button(ui, "expandDownload", tr("Download"), false).clicked() {
            download(app, &ctx, current);
        }
    }
    ui.add_space(4.0);
    let past = reaches_past(app, id, d) || empty_area(app, id, d);
    ui.horizontal(|ui| {
        if let Some(f) = progress(app).filter(|_| running.is_some()) {
            ui.label(tr_format!("{stage}… {:.0}%", f * 100.0, stage = tr("Generating")));
            if crate::widgets::text_button(ui, "expandCancel", tr("Cancel"), false).clicked() {
                app.session.cancel_expand();
            }
        } else {
            let ready = app.session.expand_ready(current);
            let r = crate::widgets::text_button(ui, "cropGenerate", tr("Generate"), past && ready);
            if r.clicked() {
                if past {
                    generate(app, &ctx);
                } else {
                    app.toast(&ctx, tr("Drag the crop past the photo's edges, or choose an aspect ratio, first."));
                }
            }
        }
    });
    if empty_area(app, id, d) {
        note(ui, tr("The checkerboard shows area with nothing in it yet: Generate fills it.").into());
    } else if let Some(f) = &d.expand_fill {
        let by = ExpandModel::from_id(&f.model).map_or(f.model.as_str(), |m| m.label());
        note(ui, tr_format!("The new area was painted by {by} on this device.", by = by));
    } else {
        note(ui, tr("Drag the crop past the photo's edges, or choose an aspect ratio, then Generate.").into());
    }
}

/// A checkerboard over `r` (the loupe's view of the photo): what shows through where the crop
/// reaches past the photo and nothing is painted there yet.
pub fn checkerboard(p: &egui::Painter, r: Rect) {
    if !r.is_finite() || r.width() <= 0.0 || r.height() <= 0.0 {
        return;
    }
    const CELL: f32 = 10.0;
    let (light, dark) = (Color32::from_gray(150), Color32::from_gray(112));
    p.rect_filled(r, 0.0, light);
    let mut mesh = egui::epaint::Mesh::default();
    let (nx, ny) = ((r.width() / CELL).ceil() as usize, (r.height() / CELL).ceil() as usize);
    // (a huge zoomed view has too many cells to draw: its light fill is enough)
    if nx.saturating_mul(ny) > 40_000 {
        return;
    }
    for j in 0..ny {
        for i in (j % 2..nx).step_by(2) {
            let a = pos2(r.left() + i as f32 * CELL, r.top() + j as f32 * CELL);
            let b = Pos2::new((a.x + CELL).min(r.right()), (a.y + CELL).min(r.bottom()));
            mesh.add_colored_rect(Rect::from_min_max(a, b), dark);
        }
    }
    p.add(Shape::mesh(mesh));
}
