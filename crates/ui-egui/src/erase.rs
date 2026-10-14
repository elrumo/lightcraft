//! AI Remove in the UI (`lightcraft_engine::erase`): the Remove tool's **Generative AI** switch with the
//! model's state and download offer, and the slow fill run a couple of frames after a
//! "Removing with AI…" notice is on screen, so the wait (about half a second on a computer, a
//! few seconds in a browser, where it runs on the page's thread) is never a silent freeze.
//!
//! The desktop and phone apps download the model with the engine's downloader
//! (`models.download`); the browser build has its own fetch ([`ModelFetch`]), which keeps the file
//! in the site's storage and hands its bytes to the session.

use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};

use serde_json::{Value, json};

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
    app.toast_for(ctx, tr("Removing with AI…"), 60.0);
    app.erase.pending = Some((id.into(), params, false));
    ctx.request_repaint();
}

/// Whether a fill is waiting to run.
pub fn busy(app: &LightcraftApp) -> bool {
    app.erase.pending.is_some()
}

/// Every frame: take the browser build's fetch answer, and run a queued fill.
pub fn poll(app: &mut LightcraftApp, ctx: &egui::Context) {
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

/// Whether a stroke should be filled by AI: the Generative AI switch is on and the model is ready. With the
/// switch on and no model, a toast says what to do and the stroke is filled from around it.
pub fn wanted(app: &mut LightcraftApp, ctx: &egui::Context) -> bool {
    if !app.ui.remove_ai {
        return false;
    }
    if app.session.eraser_ready() {
        return true;
    }
    app.toast_for(ctx, tr("AI Remove needs its model: download it in the Remove panel. This stroke was filled from around it."), 4.0);
    false
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

/// How many text rows [`switch`] takes below the switch itself (the phone panel sizes to it).
pub fn extra_rows(app: &LightcraftApp) -> f32 {
    if !app.ui.remove_ai || app.session.eraser_ready() || app.session.enhancer.disabled.contains(lightcraft_engine::erase::ID) { 0.0 } else { 2.5 }
}
