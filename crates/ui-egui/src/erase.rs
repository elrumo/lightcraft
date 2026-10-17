//! AI Remove in the UI (`lightcraft_engine::erase`): the Remove tool's **Generative AI** switch with the
//! model's state and download offer, its **High quality** switch (LaMa, where this device runs it), and the fill:
//! on a worker thread with a row in the activity stack (the stroke shows at once, filled from around it, and the
//! AI fill replaces it when done), or in the browser, run a couple of frames after a "Removing with AI…" notice
//! is on screen, so the wait (a few seconds, on the page's thread) is never a silent freeze.
//!
//! The desktop and phone apps download the model with the engine's downloader
//! (`models.download`); the browser build has its own fetch ([`ModelFetch`]), which keeps the file
//! in the site's storage and hands its bytes to the session.

use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};

use serde_json::{Value, json};

use lightcraft_engine::erase::RemoveQuality;

use crate::LightcraftApp;
use crate::i18n::{tr, tr_format};
use crate::theme::Tokens;

/// The browser build's model fetch: from the site's storage, or with `download` from the model's
/// own repository (then kept). Sends the file's bytes, or [`NOT_STORED`] when it isn't stored
/// and `download` is false, then repaints.
pub type ModelFetch = Box<dyn Fn(bool, Sender<Result<Vec<u8>, String>>, egui::Context)>;

/// The fetch's answer when the model isn't in the site's storage (and wasn't to be downloaded).
pub const NOT_STORED: &str = "not stored";

/// What the Remove tool's AI switch is waiting for.
#[derive(Default)]
pub struct EraseState {
    /// A command waiting for its notice to be drawn: id, params, and whether a frame has passed.
    pending: Option<(String, Value, bool)>,
    /// The browser build's fetch in flight.
    rx: Option<Receiver<Result<Vec<u8>, String>>>,
    /// The browser build looked in its storage and the model isn't there.
    not_stored: bool,
    /// The last fetch's error.
    error: Option<String>,
}

/// Run command `id` (`spot.add` with `ai`, `spot.erase`) once a "Removing with AI…" notice has
/// been drawn.
pub fn queue(app: &mut LightcraftApp, ctx: &egui::Context, id: &str, params: Value) {
    // on a worker the command only starts the fill (its row is in the activity stack): nothing to wait behind a notice
    if params.get("wait").and_then(Value::as_bool) == Some(false) {
        if let Err(e) = app.run(id, params) {
            app.toast_error(ctx, e);
        }
        return;
    }
    queue_with(app, ctx, tr("Removing with AI…"), id, params);
}

/// Run a slow model command `id` once `notice` (already translated) has been drawn (also Lens
/// Blur's depth estimate).
pub fn queue_with(app: &mut LightcraftApp, ctx: &egui::Context, notice: &str, id: &str, params: Value) {
    app.toast_for(ctx, notice, 60.0);
    app.erase.pending = Some((id.into(), params, false));
    ctx.request_repaint();
}

/// Whether a fill is waiting to run.
pub fn busy(app: &LightcraftApp) -> bool {
    app.erase.pending.is_some()
}

/// Every frame: store the fills painted on workers, take the browser build's fetch answer, and run a queued fill.
pub fn poll(app: &mut LightcraftApp, ctx: &egui::Context) {
    for e in app.session.poll_erase() {
        app.toast_error(ctx, tr_format!("AI Remove failed: {}", e));
    }
    if app.session.erase_running() > 0 {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
    if let Some(rx) = &app.erase.rx {
        let got = match rx.try_recv() {
            Ok(r) => Some(r),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(tr("The download stopped.").into())),
        };
        if let Some(r) = got {
            app.erase.rx = None;
            match r.and_then(|bytes| app.session.install_eraser(&bytes)) {
                Ok(()) => (app.erase.not_stored, app.erase.error) = (false, None),
                Err(e) if e == NOT_STORED => app.erase.not_stored = true,
                Err(e) => app.erase.error = Some(e),
            }
            ctx.request_repaint();
        }
    }
    match app.erase.pending.take() {
        Some((id, params, false)) => {
            app.erase.pending = Some((id, params, true));
            ctx.request_repaint();
        }
        Some((id, params, true)) => {
            let r = app.run(&id, params);
            app.ui.toast = None;
            if let Err(e) = r {
                app.toast_error(ctx, e);
            }
        }
        None => {}
    }
}

/// The browser build: look for the model in the site's storage (or download it), unless a fetch
/// runs already.
fn fetch(app: &mut LightcraftApp, ctx: &egui::Context, download: bool) {
    let Some(f) = app.services.erase_model.as_ref() else { return };
    if app.erase.rx.is_some() {
        return;
    }
    let (tx, rx) = channel();
    app.erase.rx = Some(rx);
    app.erase.error = None;
    f(download, tx, ctx.clone());
}

/// The browser build: download MI-GAN into the site's storage (Generative Expand's offer).
pub(crate) fn fetch_model(app: &mut LightcraftApp, ctx: &egui::Context) {
    fetch(app, ctx, true);
}

/// The browser build: look for MI-GAN in the site's storage once, unless that was done already or
/// a fetch runs.
pub(crate) fn look_in_storage(app: &mut LightcraftApp, ctx: &egui::Context) {
    if app.services.erase_model.is_some() && app.erase.rx.is_none() && !app.erase.not_stored && app.erase.error.is_none() {
        fetch(app, ctx, false);
    }
}

/// How new strokes are painted: High when its switch is on and LaMa is ready, else Fast.
pub fn quality(app: &LightcraftApp) -> RemoveQuality {
    if app.ui.remove_hq && app.session.remove_ready(RemoveQuality::High) { RemoveQuality::High } else { RemoveQuality::Fast }
}

/// The command parameters that make a Remove command paint with AI at `quality`: on a worker where the platform has
/// threads (the stroke is there at once, the fill follows), inline in the browser (behind its notice).
pub fn ai_params(quality: RemoveQuality) -> Value {
    json!({"ai": true, "quality": quality.id(), "wait": cfg!(target_arch = "wasm32")})
}

/// How a stroke should be filled by AI, or `None` for from around it: the Generative AI switch is on and the model is
/// ready. With the switch on and no model, a toast says what to do and the stroke is filled from around it.
pub fn wanted(app: &mut LightcraftApp, ctx: &egui::Context) -> Option<RemoveQuality> {
    if !app.ui.remove_ai {
        return None;
    }
    let q = quality(app);
    if app.session.remove_ready(q) {
        return Some(q);
    }
    app.toast_for(ctx, tr("AI Remove needs its model: download it in the Remove panel. This stroke was filled from around it."), 4.0);
    None
}

/// The Generative AI switch (Remove only; saved with the app settings, like Lightroom's) and, when it's on, the model's state: downloading, the offer to
/// download it (size, licence and credit first), or nothing when it's ready.
pub fn switch(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let mut on = app.ui.remove_ai;
    let r = crate::widgets::check(ui, &mut on, tr("Generative AI"))
        .on_hover_text(tr("Fill with what is most likely behind the stroke (the MI-GAN model) instead of texture from around it"));
    crate::widgets::register(ui.ctx(), "check:removeAi", r.rect);
    if r.changed() {
        app.ui.remove_ai = on;
    }
    if !app.ui.remove_ai {
        return;
    }
    fast_model(app, ui);
    high_quality(app, ui);
}

/// MI-GAN's state under the Generative AI switch: turned off, downloading, the offer to download it, or nothing when it's
/// ready.
fn fast_model(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let dim = Tokens::get(ui.ctx()).text_dim;
    let note = |ui: &mut egui::Ui, text: String| {
        ui.add(egui::Label::new(egui::RichText::new(text).color(dim).size(11.5)).wrap());
    };
    if app.session.enhancer.disabled.contains(lightcraft_engine::erase::ID) {
        note(ui, tr("AI Remove is turned off in Settings → AI Models.").into());
        return;
    }
    if app.session.eraser_ready() {
        return;
    }
    let mb = lightcraft_engine::erase::FILE_SIZE as f64 / 1e6;
    let offer = tr_format!(
        "Uses the MI-GAN model, a {:.0} MB download ({} licence). {}.",
        mb,
        lightcraft_engine::erase::LICENCE,
        lightcraft_engine::erase::CREDIT
    );
    let ctx = ui.ctx().clone();
    if app.services.erase_model.is_some() {
        // the browser build: is it in the site's storage? (asked once the switch is on)
        if app.erase.rx.is_none() && !app.erase.not_stored && app.erase.error.is_none() {
            fetch(app, &ctx, false);
        }
        if app.erase.rx.is_some() {
            note(ui, tr("Getting the MI-GAN model…").into());
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
            return;
        }
        note(ui, offer);
        if let Some(e) = &app.erase.error {
            note(ui, e.clone());
        }
        if crate::widgets::text_button(ui, "removeAiDownload", tr("Download"), false).clicked() {
            fetch(app, &ctx, true);
        }
        return;
    }
    if !lightcraft_engine::enhance::Enhancer::AVAILABLE {
        note(ui, tr("AI Remove isn't available in this build.").into());
        return;
    }
    let (fetching, d) = app.session.enhancer.download_status();
    let mine = fetching.as_deref() == Some(lightcraft_engine::erase::ID);
    if mine && d.running {
        let pct = if d.total > 0 { d.done as f64 / d.total as f64 * 100.0 } else { 0.0 };
        note(ui, tr_format!("Downloading MI-GAN… {:.0} %", pct));
        if crate::widgets::text_button(ui, "removeAiCancel", tr("Cancel Download"), false).clicked() {
            let _ = app.run("models.cancel", json!({"id": lightcraft_engine::erase::ID}));
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
        return;
    }
    note(ui, offer);
    if mine && let Some(e) = &d.error {
        note(ui, e.clone());
    }
    if crate::widgets::text_button(ui, "removeAiDownload", tr("Download"), false).clicked()
        && let Err(e) = app.run("models.download", json!({"id": lightcraft_engine::erase::ID, "acknowledged": true}))
    {
        app.toast_error(&ctx, e);
    }
}

/// The High quality switch (LaMa), where this device runs it, and when it's on the model's state: downloading, or the offer to
/// download it (size, licence and credit first). Strokes are painted with it once it is ready.
fn high_quality(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    if !app.session.expand_available(lightcraft_engine::expand::ExpandModel::Lama) {
        return;
    }
    let mut on = app.ui.remove_hq;
    let r = crate::widgets::check(ui, &mut on, tr("High quality"))
        .on_hover_text(tr("Paint with LaMa, a larger model: slower, and better on textures and large areas"));
    crate::widgets::register(ui.ctx(), "check:removeAiHq", r.rect);
    if r.changed() {
        app.ui.remove_hq = on;
    }
    if !app.ui.remove_hq {
        return;
    }
    let dim = Tokens::get(ui.ctx()).text_dim;
    let note = |ui: &mut egui::Ui, text: String| {
        ui.add(egui::Label::new(egui::RichText::new(text).color(dim).size(11.5)).wrap());
    };
    if app.session.enhancer.disabled.contains(lightcraft_engine::erase::LAMA_ID) {
        note(ui, tr("LaMa is turned off in Settings → AI Models.").into());
        return;
    }
    if app.session.remove_ready(RemoveQuality::High) {
        return;
    }
    if !lightcraft_engine::enhance::Enhancer::AVAILABLE {
        note(ui, tr("LaMa isn't available in this build.").into());
        return;
    }
    let ctx = ui.ctx().clone();
    let (fetching, d) = app.session.enhancer.download_status();
    let mine = fetching.as_deref() == Some(lightcraft_engine::erase::LAMA_ID);
    if mine && d.running {
        let pct = if d.total > 0 { d.done as f64 / d.total as f64 * 100.0 } else { 0.0 };
        note(ui, tr_format!("Downloading LaMa… {:.0} %", pct));
        if crate::widgets::text_button(ui, "removeAiHqCancel", tr("Cancel Download"), false).clicked() {
            let _ = app.run("models.cancel", json!({"id": lightcraft_engine::erase::LAMA_ID}));
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
        return;
    }
    note(
        ui,
        tr_format!(
            "Uses the LaMa model, a {:.0} MB download ({} licence). {}. Until it is downloaded, strokes use the fast model.",
            lightcraft_engine::erase::LAMA_FILE_SIZE as f64 / 1e6,
            lightcraft_engine::erase::LAMA_LICENCE,
            lightcraft_engine::erase::LAMA_CREDIT
        ),
    );
    if mine && let Some(e) = &d.error {
        note(ui, e.clone());
    }
    if crate::widgets::text_button(ui, "removeAiHqDownload", tr("Download"), false).clicked()
        && let Err(e) = app.run("models.download", json!({"id": lightcraft_engine::erase::LAMA_ID, "acknowledged": true}))
    {
        app.toast_error(&ctx, e);
    }
}

/// How many text rows [`switch`] takes below the switch itself (the phone panel sizes to it).
pub fn extra_rows(app: &LightcraftApp) -> f32 {
    if !app.ui.remove_ai {
        return 0.0;
    }
    let fast = if app.session.eraser_ready() || app.session.enhancer.disabled.contains(lightcraft_engine::erase::ID) { 0.0 } else { 2.5 };
    let lama = app.session.expand_available(lightcraft_engine::expand::ExpandModel::Lama);
    let high = match (lama, app.ui.remove_hq) {
        (false, _) => 0.0,
        (true, false) => 1.0,
        (true, true)
            if app.session.remove_ready(RemoveQuality::High) || app.session.enhancer.disabled.contains(lightcraft_engine::erase::LAMA_ID) =>
        {
            1.0
        }
        (true, true) => 4.0,
    };
    fast + high
}
