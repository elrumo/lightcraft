//! The Edit panel's **Lens Blur** section (`lensBlur.*` commands, `lightcraft_engine::depth`):
//! Apply (estimating the depth with the model first when the photo has none of its own, behind an
//! "Estimating depth…" notice, or offering the model's download), Blur Amount, the focal range on
//! a near-to-far bar (drag its ends or its middle, or pick a point on the photo, or the subject),
//! Boost, the bokeh shape and Visualize Depth. The phone shows the same body as its own group.

use egui::{Color32, CornerRadius, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use lightcraft_catalog::PhotoId;
use lightcraft_develop::{Bokeh, DevelopSettings};
use serde_json::json;

use crate::LightcraftApp;
use crate::i18n::{tr, tr_format};
use crate::icons::Icon;
use crate::theme::Tokens;
use crate::widgets::register;

/// The browser build's fetch of the depth model (`Services::depth_model`): from the site's storage,
/// or downloaded when the user asks.
#[derive(Default)]
pub struct DepthFetch {
    rx: Option<std::sync::mpsc::Receiver<Result<Vec<u8>, String>>>,
    /// The site's storage was looked in and the model isn't there.
    not_stored: bool,
    error: Option<String>,
}

/// Every frame: take the browser build's fetch answer and hand the model to the session.
pub fn poll(app: &mut LightcraftApp, ctx: &egui::Context) {
    let Some(rx) = &app.depth_fetch.rx else { return };
    let got = match rx.try_recv() {
        Ok(r) => r,
        Err(std::sync::mpsc::TryRecvError::Empty) => return,
        Err(std::sync::mpsc::TryRecvError::Disconnected) => Err(tr("The download stopped.").to_string()),
    };
    app.depth_fetch.rx = None;
    match got.and_then(|bytes| app.session.install_depth_model(&bytes)) {
        Ok(()) => (app.depth_fetch.not_stored, app.depth_fetch.error) = (false, None),
        Err(e) if e == crate::erase::NOT_STORED => app.depth_fetch.not_stored = true,
        Err(e) => app.depth_fetch.error = Some(e),
    }
    ctx.request_repaint();
}

/// The browser build: look in the site's storage (or download), unless a fetch runs.
fn fetch(app: &mut LightcraftApp, ctx: &egui::Context, download: bool) {
    let Some(f) = app.services.depth_model.as_ref() else { return };
    if app.depth_fetch.rx.is_some() {
        return;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    app.depth_fetch.rx = Some(rx);
    app.depth_fetch.error = None;
    f(download, tx, ctx.clone());
}

/// The loupe tool that focuses on the point clicked.
pub const FOCUS_TOOL: &str = "lensFocus";
/// The loupe tool that paints the Refine brush (`app.ui.lens_brush`: focus, blur or erase).
pub const BRUSH_TOOL: &str = "lensBrush";

/// The Refine brushes: (mode, label).
const BRUSHES: [(&str, &str); 3] = [("focus", "Focus"), ("blur", "Blur"), ("erase", "Erase")];

/// The live stroke's colour for each Refine brush.
pub fn brush_color(mode: &str) -> [u8; 3] {
    match mode {
        "focus" => [80, 210, 120],
        "blur" => [90, 140, 255],
        _ => [235, 80, 60],
    }
}

/// Whether photo `id` has depth to blur by now: a stored estimate, or its file's own (as far as
/// its loaded source says).
fn has_depth(app: &LightcraftApp, id: PhotoId, d: &DevelopSettings) -> bool {
    d.lens_blur.depth.is_some() || app.session.source_info(id).mattes.as_deref().and_then(lightcraft_engine::pipeline::masks::Mattes::depth).is_some()
}

/// Turn Lens Blur on: at once when there is depth, else after the "Estimating depth…" notice (the
/// model runs on this thread), else the model's download is offered.
pub fn apply(app: &mut LightcraftApp, ctx: &egui::Context, id: PhotoId, d: &DevelopSettings) {
    if has_depth(app, id, d) || app.session.photo_has_depth(id) {
        if let Err(e) = app.run("lensBlur.apply", json!({})) {
            app.toast_error(ctx, e);
        }
    } else if app.session.depth_model_ready() {
        crate::erase::queue_with(app, ctx, tr("Estimating depth…"), "lensBlur.apply", json!({}));
    } else {
        app.ui.lens_blur_offer = true;
    }
}

fn note(ui: &mut egui::Ui, text: impl Into<String>) {
    let dim = Tokens::get(ui.ctx()).text_dim;
    ui.add(egui::Label::new(egui::RichText::new(text.into()).color(dim).size(11.5)).wrap());
}

/// The depth model's state and download offer (size, host's licence and credit first), when a
/// photo without depth asked for it.
fn model_offer(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    use lightcraft_engine::depth;
    let ctx = ui.ctx().clone();
    if app.session.enhancer.disabled.contains(depth::ID) {
        note(ui, tr("Depth estimation is turned off in Settings → AI Models."));
        return;
    }
    let offer = tr_format!(
        "This photo has no depth of its own: Lens Blur estimates it with Depth Anything V2 Small, a {:.0} MB download ({} licence). {}.",
        depth::FILE_SIZE as f64 / 1e6,
        depth::LICENCE,
        depth::CREDIT
    );
    if app.services.depth_model.is_some() {
        // the browser build: is it in the site's storage? (looked for once the offer shows)
        if app.depth_fetch.rx.is_none() && !app.depth_fetch.not_stored && app.depth_fetch.error.is_none() {
            fetch(app, &ctx, false);
        }
        if app.depth_fetch.rx.is_some() {
            note(ui, tr("Getting the depth model…"));
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
            return;
        }
        note(ui, offer);
        if let Some(e) = &app.depth_fetch.error {
            note(ui, e.clone());
        }
        if crate::widgets::text_button(ui, "lensBlurDownload", tr("Download"), false).clicked() {
            fetch(app, &ctx, true);
        }
        return;
    }
    if !lightcraft_engine::enhance::Enhancer::AVAILABLE {
        note(ui, tr("This photo has no depth of its own, and depth estimation isn't available in this build."));
        return;
    }
    let st = app.session.enhancer.model_download_status(depth::ID);
    if st.running {
        let pct = if st.total > 0 { st.done as f64 / st.total as f64 * 100.0 } else { 0.0 };
        note(ui, tr_format!("Downloading the depth model… {:.0} %", pct));
        if crate::widgets::text_button(ui, "lensBlurCancel", tr("Cancel Download"), false).clicked() {
            let _ = app.run("models.cancel", json!({"id": depth::ID}));
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
        return;
    }
    note(ui, offer);
    if let Some(e) = &st.error {
        note(ui, e.clone());
    }
    if crate::widgets::text_button(ui, "lensBlurDownload", tr("Download"), false).clicked()
        && let Err(e) = app.run("models.download", json!({"id": depth::ID, "acknowledged": true}))
    {
        app.toast_error(&ctx, e);
    }
}

/// The section's body.
pub fn panel(app: &mut LightcraftApp, ui: &mut egui::Ui, d: &DevelopSettings, id: PhotoId) {
    let lb = &d.lens_blur;
    let ctx = ui.ctx().clone();
    if !lb.enabled {
        egui::Frame::NONE.inner_margin(crate::widgets::margin(&ctx, 4, 4)).show(ui, |ui| {
            note(ui, tr("Blur what is nearer or farther than the subject, as a wide aperture would, using the photo's depth."));
            ui.add_space(4.0);
            if crate::widgets::text_button(ui, "lensBlurApply", tr("Apply"), false).clicked() {
                apply(app, &ctx, id, d);
            }
            // the download offer once a photo without depth asked, until the model is here
            if app.ui.lens_blur_offer && !app.session.depth_model_ready() {
                ui.add_space(4.0);
                model_offer(app, ui);
            } else if app.ui.lens_blur_offer && !crate::erase::busy(app) {
                app.ui.lens_blur_offer = false;
                apply(app, &ctx, id, d);
            }
        });
        ui.add_space(8.0);
        return;
    }
    egui::Frame::NONE.inner_margin(crate::widgets::margin(&ctx, 4, 4)).show(ui, |ui| {
        let mut on = true;
        let r = crate::widgets::check(ui, &mut on, tr("Apply"));
        register(&ctx, "check:lensBlur", r.rect);
        if r.changed() {
            let _ = app.run("lensBlur.apply", json!({"on": false}));
        }
        let source = if lb.depth.is_some() {
            tr("Depth estimated by AI (Depth Anything V2 Small).")
        } else if has_depth(app, id, d) {
            tr("Depth from the photo.")
        } else {
            tr("No depth yet: turn Lens Blur off and on to estimate it.")
        };
        note(ui, source);
    });
    crate::panels::edit::control(app, ui, d, "lensBlur.amount", true);
    crate::panels::edit::sub_title(ui, tr("Focal Range"));
    focal_bar(app, ui, d);
    ui.horizontal(|ui| {
        ui.add_space(18.0);
        let picking = app.ui.tool == FOCUS_TOOL;
        if crate::widgets::icon_button(ui, FOCUS_TOOL, Icon::Target, vec2(28.0, 28.0), picking, true, tr("Focus on a point: click the photo"))
            .clicked()
        {
            if picking {
                app.ui.tool.clear();
            } else {
                app.ui.tool = FOCUS_TOOL.into();
            }
        }
        if crate::widgets::text_button(ui, "lensBlurSubject", tr("Subject"), false).on_hover_text(tr("Focus on the photo's subject")).clicked()
            && let Err(e) = app.run("lensBlur.focus", json!({"subject": true}))
        {
            app.toast_error(&ctx, e);
        }
    });
    let mut vis = app.ui.depth_visualize;
    egui::Frame::NONE.inner_margin(crate::widgets::margin(&ctx, 2, 4)).show(ui, |ui| {
        let r = crate::widgets::check(ui, &mut vis, tr("Visualize Depth"));
        register(&ctx, "check:visualizeDepth", r.rect);
        if r.changed() {
            app.ui.depth_visualize = vis;
        }
    });
    refine(app, ui, d);
    crate::panels::edit::sub_title(ui, tr("Bokeh"));
    let labels: Vec<String> = Bokeh::ALL.iter().map(|b| tr(b.label()).to_string()).collect();
    let items: Vec<(&str, &str)> = labels.iter().zip(Bokeh::ALL).map(|(l, b)| (l.as_str(), b.name())).collect();
    let active = Bokeh::ALL.iter().position(|b| *b == lb.bokeh);
    let mut chosen = None;
    egui::Frame::NONE.inner_margin(crate::widgets::margin(&ctx, 2, 4)).show(ui, |ui| {
        chosen = crate::widgets::segmented(ui, "lensBlurBokeh", &items, active, 3);
    });
    if let Some(b) = chosen.and_then(|i| Bokeh::ALL.get(i)) {
        let _ = app.run("lensBlur.bokeh", json!({"shape": b.name()}));
    }
    crate::panels::edit::control(app, ui, d, "lensBlur.boost", true);
    ui.add_space(8.0);
}

/// Refine: Focus / Blur / Erase brushes painted on the photo (choosing one turns its tool on, choosing
/// it again turns it off), the brush's settings and Clear.
fn refine(app: &mut LightcraftApp, ui: &mut egui::Ui, d: &DevelopSettings) {
    let ctx = ui.ctx().clone();
    crate::panels::edit::sub_title(ui, tr("Refine"));
    let labels: Vec<String> = BRUSHES.iter().map(|(_, l)| tr(l).to_string()).collect();
    let items: Vec<(&str, &str)> = labels.iter().zip(BRUSHES).map(|(l, (k, _))| (l.as_str(), k)).collect();
    let painting = app.ui.tool == BRUSH_TOOL;
    let active = BRUSHES.iter().position(|(k, _)| painting && *k == app.ui.lens_brush);
    let mut chosen = None;
    egui::Frame::NONE.inner_margin(crate::widgets::margin(&ctx, 2, 4)).show(ui, |ui| {
        chosen = crate::widgets::segmented(ui, "lensBlurBrush", &items, active, 3);
    });
    if let Some((k, _)) = chosen.and_then(|i| BRUSHES.get(i)) {
        if painting && app.ui.lens_brush == *k {
            app.ui.tool.clear();
        } else {
            app.ui.lens_brush = (*k).to_string();
            app.ui.tool = BRUSH_TOOL.into();
        }
    }
    if app.ui.tool == BRUSH_TOOL {
        egui::Frame::NONE.inner_margin(crate::widgets::margin(&ctx, 2, 4)).show(ui, |ui| {
            note(ui, tr("Paint on the photo: Focus keeps what you paint sharp, Blur blurs it, Erase takes either back."));
            let mut auto = app.ui.brush_auto_mask;
            if crate::widgets::check(ui, &mut auto, tr("Auto Mask")).on_hover_text(tr("Paint only areas like the one under the brush")).changed() {
                app.ui.brush_auto_mask = auto;
            }
        });
        crate::panels::masking::brush_sliders(app, ui);
    }
    let lb = &d.lens_blur;
    if !lb.focus.is_empty() || !lb.blur.is_empty() {
        ui.horizontal(|ui| {
            ui.add_space(18.0);
            if crate::widgets::text_button(ui, "lensBlurClearBrush", tr("Clear Brushing"), false).clicked() {
                let _ = app.run("lensBlur.clearBrush", json!({}));
            }
        });
    }
}

/// What a drag on the focal bar holds.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Grip {
    Near,
    Far,
    /// The whole range, grabbed `offset` (depth) right of its middle.
    Range {
        offset: f64,
    },
}

/// The focal range on a near-to-far bar (the colours Visualize Depth paints): its two ends drag
/// apart or together, its middle moves it, a click elsewhere centres it there. One undo step per
/// drag (`develop.beginInteraction` … `develop.endInteraction`).
fn focal_bar(app: &mut LightcraftApp, ui: &mut egui::Ui, d: &DevelopSettings) {
    use lightcraft_engine::pipeline::visualize::{DEPTH_FAR, DEPTH_NEAR};
    let t = Tokens::get(ui.ctx());
    let [near, far] = d.lens_blur.focus_bounds();
    // (inset like the sliders' tracks)
    let (outer, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click_and_drag());
    let bar =
        Rect::from_min_max(pos2(outer.left() + 24.0, outer.top() + 7.0), pos2((outer.right() - 22.0).max(outer.left() + 64.0), outer.top() + 23.0));
    register(ui.ctx(), "lensBlurRange", bar);
    let x_of = |v: f64| bar.left() + bar.width() * (v / 100.0) as f32;
    let v_of = |x: f32| f64::from(((x - bar.left()) / bar.width()).clamp(0.0, 1.0)) * 100.0;
    let p = ui.painter();
    // the gradient, near (left) to far (right)
    let mut mesh = egui::epaint::Mesh::default();
    let steps = 24;
    for i in 0..=steps {
        let f = i as f32 / steps as f32;
        let c: [u8; 3] = std::array::from_fn(|k| (f32::from(DEPTH_NEAR[k]) + (f32::from(DEPTH_FAR[k]) - f32::from(DEPTH_NEAR[k])) * f).round() as u8);
        let col = Color32::from_rgb(c[0], c[1], c[2]);
        let x = bar.left() + bar.width() * f;
        mesh.colored_vertex(pos2(x, bar.top()), col);
        mesh.colored_vertex(pos2(x, bar.bottom()), col);
        if i > 0 {
            let b = (2 * i) as u32;
            mesh.add_triangle(b - 2, b - 1, b);
            mesh.add_triangle(b - 1, b + 1, b);
        }
    }
    p.add(mesh);
    // dimmed outside the sharp range, outlined inside it
    let shade = Color32::from_black_alpha(150);
    p.rect_filled(Rect::from_x_y_ranges(bar.left()..=x_of(near), bar.y_range()), CornerRadius::ZERO, shade);
    p.rect_filled(Rect::from_x_y_ranges(x_of(far)..=bar.right(), bar.y_range()), CornerRadius::ZERO, shade);
    let range = Rect::from_x_y_ranges(x_of(near)..=x_of(far).max(x_of(near) + 2.0), bar.y_range()).expand(2.0);
    p.rect_stroke(range, CornerRadius::same(3), Stroke::new(2.0, Color32::WHITE), StrokeKind::Middle);
    for (v, name) in [(near, "near"), (far, "far")] {
        let h = Rect::from_center_size(pos2(x_of(v), bar.center().y), vec2(8.0, 22.0));
        p.rect(h, CornerRadius::same(2), Color32::WHITE, Stroke::new(1.0, t.button_border), StrokeKind::Inside);
        register(ui.ctx(), format!("lensBlurRange:{name}"), h.expand(4.0));
    }
    let gid = egui::Id::new("lensBlurGrip");
    let set = |app: &mut LightcraftApp, lo: f64, hi: f64| {
        let (lo, hi) = (lo.clamp(0.0, 100.0), hi.clamp(0.0, 100.0));
        let _ = app.run("develop.set", json!({"values": {"lensBlur.focalDistance": (lo + hi) / 2.0, "lensBlur.focalRange": (hi - lo).max(0.0)}}));
    };
    // (the grip is chosen where the press was: a drag is recognised only some points later)
    if resp.drag_started()
        && let Some(q) = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos())
    {
        let reach = 10.0;
        let grip = if (q.x - x_of(near)).abs() <= reach && (q.x - x_of(near)).abs() < (q.x - x_of(far)).abs() {
            Grip::Near
        } else if (q.x - x_of(far)).abs() <= reach {
            Grip::Far
        } else {
            Grip::Range { offset: v_of(q.x) - (near + far) / 2.0 }
        };
        ui.ctx().data_mut(|m| m.insert_temp(gid, grip));
        let _ = app.run("develop.beginInteraction", json!({"label": "Focal Range"}));
    }
    if resp.dragged()
        && let (Some(q), Some(grip)) = (resp.interact_pointer_pos(), ui.ctx().data(|m| m.get_temp::<Grip>(gid)))
    {
        let v = v_of(q.x);
        match grip {
            Grip::Near => set(app, v.min(far), far),
            Grip::Far => set(app, near, v.max(near)),
            Grip::Range { offset } => {
                let half = (far - near) / 2.0;
                let c = (v - offset).clamp(half, 100.0 - half);
                set(app, c - half, c + half);
            }
        }
    }
    if resp.drag_stopped() {
        ui.ctx().data_mut(|m| m.remove::<Grip>(gid));
        let _ = app.run("develop.endInteraction", json!({}));
    }
    if resp.clicked()
        && let Some(q) = resp.interact_pointer_pos()
    {
        let half = (far - near) / 2.0;
        let c = v_of(q.x).clamp(half, 100.0 - half);
        set(app, c - half, c + half);
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    resp.on_hover_text(tr_format!("In focus: {:.0}–{:.0} (0 nearest, 100 farthest)", near, far));
}
