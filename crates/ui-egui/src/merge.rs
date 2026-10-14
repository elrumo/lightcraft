//! Photo Merge in the UI: the HDR / Panorama / HDR Panorama dialog (options + live preview).
//!
//! The merges themselves run in [`lightcraft_apphost::merge::Runner`] (shared with the SwiftUI
//! app): a preview for the open dialog, redrawn when its options change, and the full merge,
//! which keeps running after the dialog closes (progress in the activity stack) and adds its
//! result to the library.

use std::sync::Arc;

use lightcraft_apphost::merge::{Done, Runner};
use lightcraft_catalog::PhotoId;
use serde_json::{Value, json};

use crate::LightcraftApp;

/// The dialog's options (the parameters of the `merge.*` commands).
pub use lightcraft_apphost::merge::MergeOptions as MergeDialog;

/// The dialog's title for a merge command.
pub fn title(opts: &MergeDialog) -> &'static str {
    match opts.command.as_str() {
        "merge.panorama" => "Panorama Merge Preview",
        "merge.hdrPanorama" => "HDR Panorama Merge Preview",
        _ => "HDR Merge Preview",
    }
}

/// Merge state of the app.
#[derive(Default)]
pub struct MergeState {
    pub runner: Runner,
    /// The latest preview (texture, size, its options, stats) and the latest error.
    pub preview: Option<(egui::TextureHandle, [usize; 2], MergeDialog, Value)>,
    pub preview_pixels: Option<Arc<egui::ColorImage>>,
    pub error: Option<String>,
    /// The last finished merge (`{id, path, …}`).
    pub last_result: Option<Value>,
    /// Photos the dialog merges (the selection when it opened).
    pub ids: Vec<PhotoId>,
}

impl MergeState {
    /// A preview or a merge is running.
    pub fn busy(&self) -> bool {
        self.runner.busy()
    }
}

/// Open the merge dialog for the selection.
pub fn open(app: &mut LightcraftApp, command: &str) -> Result<Value, String> {
    let ids = app.session.targets(&json!({}));
    if ids.len() < 2 {
        return Err("select at least 2 photos to merge".into());
    }
    app.merge.ids = ids;
    app.merge.error = None;
    app.merge.preview = None;
    app.merge.preview_pixels = None;
    app.merge.runner.cancel_preview();
    app.merge.runner.failed = None;
    app.ui.dialog = Some(crate::state::Dialog::Merge { opts: MergeDialog::for_command(command) });
    Ok(json!({"photos": app.merge.ids.len()}))
}

/// Start the full-resolution merge (the dialog's Merge button / `ui.dialog.confirm`).
pub fn start_final(app: &mut LightcraftApp, opts: &MergeDialog) -> Result<Value, String> {
    let ids = app.merge.ids.clone();
    app.merge.runner.start(&app.session, opts, &ids)?;
    Ok(json!({"started": true, "photos": ids.len()}))
}

/// Merge the selection without the dialog, with the options last used for `command` (the
/// defaults the first time).
pub fn start_last(app: &mut LightcraftApp, command: &str) -> Result<Value, String> {
    let ids = app.session.targets(&json!({}));
    if ids.len() < 2 {
        return Err("select at least 2 photos to merge".into());
    }
    app.merge.ids = ids.clone();
    app.merge.runner.start_last(&app.session, command, &ids)?;
    Ok(json!({"started": true, "photos": ids.len()}))
}

/// Per frame: keep the preview in sync with the dialog's options, collect finished jobs.
pub fn poll(app: &mut LightcraftApp, ctx: &egui::Context) {
    // preview for the open dialog
    match &app.ui.dialog {
        Some(crate::state::Dialog::Merge { opts }) => {
            let opts = opts.clone();
            let shown = app.merge.preview.as_ref().map(|p| &p.2) == Some(&opts);
            if !shown && app.merge.runner.wants_preview(&opts) {
                let ids = app.merge.ids.clone();
                match app.merge.runner.preview(&app.session, &opts, &ids) {
                    Ok(()) => app.merge.error = None,
                    Err(e) => app.merge.error = Some(e),
                }
            }
        }
        _ => app.merge.runner.cancel_preview(),
    }
    let done = app.merge.runner.poll(&mut app.session, &|stage| crate::i18n::tr(stage).to_string());
    for d in done {
        match d {
            Done::Preview { options, result: Ok((img, info)) } => {
                if let Some(mut img) = img {
                    // shown through the monitor profile like every other preview
                    if let Some(d) = app.renderer.display()
                        && let Err(e) = d.profile.from_srgb(&mut img)
                    {
                        log::warn!("display profile: {e}");
                    }
                    let color = Arc::new(egui::ColorImage::from_rgba_unmultiplied([img.width, img.height], &img.as_bytes()));
                    let tex = ctx.load_texture("merge-preview", color.clone(), egui::TextureOptions::LINEAR);
                    app.merge.preview_pixels = Some(color);
                    app.merge.preview = Some((tex, [img.width, img.height], options, info));
                }
            }
            Done::Preview { result: Err(e), .. } => app.merge.error = Some(e),
            Done::Merged { options, result, cancelled } => {
                let what = crate::i18n::tr(lightcraft_apphost::merge::what(&options.command));
                match result {
                    Ok(v) => {
                        app.merge.last_result = Some(v);
                        app.toast(ctx, crate::i18n::tr_format!("{what} merge added", what = what));
                    }
                    Err(_) if cancelled => app.toast(ctx, crate::i18n::tr_format!("{what} merge cancelled", what = what)),
                    Err(e) => {
                        app.ui.status = e.clone();
                        app.toast(ctx, crate::i18n::tr_format!("{what} merge failed: {e}", e = e, what = what));
                    }
                }
            }
        }
    }
    if app.merge.runner.busy() {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
}

/// The dialog body (options on the left, preview on the right).
pub fn body(app: &mut LightcraftApp, ui: &mut egui::Ui, opts: &mut MergeDialog) {
    let t = crate::theme::Tokens::get(ui.ctx());
    ui.horizontal_top(|ui| {
        // preview
        ui.vertical(|ui| {
            let box_size = egui::vec2(520.0, 360.0);
            let (rect, _) = ui.allocate_exact_size(box_size, egui::Sense::hover());
            ui.painter().rect_filled(rect, 4.0, t.canvas);
            crate::widgets::register(ui.ctx(), "merge.preview", rect);
            if let Some((tex, size, _, _)) = &app.merge.preview {
                let s = (box_size.x / size[0] as f32).min(box_size.y / size[1] as f32);
                let r = egui::Rect::from_center_size(rect.center(), egui::vec2(size[0] as f32 * s, size[1] as f32 * s));
                ui.painter().image(tex.id(), r, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
            }
            if let Some(task) = &app.merge.runner.preview {
                let (f, stage) = task.progress.lock().map(|g| g.clone()).unwrap_or_default();
                ui.painter().text(
                    rect.center_bottom() - egui::vec2(0.0, 14.0),
                    egui::Align2::CENTER_CENTER,
                    crate::i18n::tr_format!("{stage}… {:.0}%", f * 100.0, stage = stage),
                    t.font(12.0),
                    t.text,
                );
            }
            if let Some(e) = &app.merge.error {
                ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, e, t.font(13.0), t.text);
            }
            let info = app.merge.preview.as_ref().map(|p| p.3.clone());
            if let Some(info) = info {
                let txt = if opts.is_pano() {
                    crate::i18n::tr_format!(
                        "{} photos · {} · {:.0}° × {:.0}°",
                        info["used"].as_array().map_or(0, Vec::len),
                        info["projection"].as_str().unwrap_or(""),
                        info["fov"][0].as_f64().unwrap_or(0.0),
                        info["fov"][1].as_f64().unwrap_or(0.0)
                    )
                } else {
                    let ev: Vec<String> =
                        info["ev"].as_array().map(|a| a.iter().filter_map(Value::as_f64).map(|v| format!("{v:+.1}")).collect()).unwrap_or_default();
                    crate::i18n::tr_format!("{} photos · exposures {} EV", ev.len(), ev.join(" / "))
                };
                ui.label(egui::RichText::new(txt).color(t.text_dim));
            }
        });
        ui.add_space(12.0);
        // options
        ui.vertical(|ui| {
            ui.set_width(220.0);
            if opts.is_pano() {
                ui.label(egui::RichText::new(crate::i18n::tr("Projection")).color(t.text_label));
                for (v, l) in [("auto", "Auto Select"), ("spherical", "Spherical"), ("cylindrical", "Cylindrical"), ("perspective", "Perspective")] {
                    if ui.radio(opts.projection == v, crate::i18n::tr(l)).clicked() {
                        opts.projection = v.to_string();
                    }
                }
                ui.add_space(4.0);
                let mut bw = opts.boundary_warp;
                if num(ui, &BOUNDARY_WARP, &mut bw) {
                    opts.boundary_warp = bw.round();
                }
                crate::widgets::check(ui, &mut opts.fill_edges, crate::i18n::tr("Fill Edges"));
                crate::widgets::check(ui, &mut opts.auto_crop, crate::i18n::tr("Auto Crop"));
                ui.add_space(4.0);
            }
            if opts.is_hdr() {
                crate::widgets::check(ui, &mut opts.align, crate::i18n::tr("Auto Align"));
                ui.label(egui::RichText::new(crate::i18n::tr("Deghost Amount")).color(t.text_label));
                ui.horizontal(|ui| {
                    for (i, (v, l)) in [("none", "None"), ("low", "Low"), ("medium", "Med"), ("high", "High")].iter().enumerate() {
                        if crate::widgets::text_button(ui, &format!("mergeDeghost-{i}"), l, opts.deghost == *v).clicked() {
                            opts.deghost = v.to_string();
                        }
                    }
                });
                if opts.command == "merge.hdr" {
                    ui.add_enabled_ui(opts.deghost != "none", |ui| {
                        crate::widgets::check(ui, &mut opts.show_overlay, crate::i18n::tr("Show Deghost Overlay"))
                    });
                }
                if opts.command == "merge.hdrPanorama" {
                    let mut b = opts.bracket as f64;
                    if num(ui, &BRACKET, &mut b) {
                        opts.bracket = b.round() as u32;
                    }
                }
                ui.add_space(4.0);
            }
            crate::widgets::check(ui, &mut opts.auto_settings, crate::i18n::tr("Auto Settings"));
            crate::widgets::check(ui, &mut opts.stack, crate::i18n::tr("Create Stack"));
            ui.add_space(8.0);
            ui.label(egui::RichText::new(crate::i18n::tr_format!("{} photos", app.merge.ids.len())).color(t.text_dim));
        });
    });
}

const BOUNDARY_WARP: lightcraft_develop::ControlSpec = lightcraft_develop::ControlSpec {
    id: "merge.boundaryWarp",
    label: "Boundary Warp",
    section: lightcraft_develop::Section::Light,
    min: 0.0,
    max: 100.0,
    default: 0.0,
    step: 1.0,
    decimals: 0,
    track: lightcraft_develop::Track::Plain,
};
const BRACKET: lightcraft_develop::ControlSpec = lightcraft_develop::ControlSpec {
    id: "merge.bracket",
    label: "Photos per bracket (0 = auto)",
    section: lightcraft_develop::Section::Light,
    min: 0.0,
    max: 9.0,
    default: 0.0,
    step: 1.0,
    decimals: 0,
    track: lightcraft_develop::Track::Plain,
};

fn num(ui: &mut egui::Ui, spec: &lightcraft_develop::ControlSpec, v: &mut f64) -> bool {
    match crate::widgets::slider(ui, spec, *v, true, None).value {
        Some(n) => {
            *v = n;
            true
        }
        None => false,
    }
}
