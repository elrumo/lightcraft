//! People: a card per person — named on faces (read from XMP, or written by naming a person here)
//! or, when the user has turned finding people on, found in the photos and not named yet. A card
//! shows a close-up of their largest face, the name and how many photos they are in; a click shows
//! that person's photos in the grid. An unnamed person has a field to type their name in.
//!
//! Only the rows on screen ask for a face render (the engine caches them, memory and disk).
//! Finding people is the user's choice, with the models and licences shown first: nothing is
//! downloaded or looked at until they say so (the bar at the top of this view).

use egui::{Align2, Color32, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use lightcraft_catalog::Person;
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::theme::Tokens;
use crate::widgets::register;

/// A card's face picture (points); the name and count sit below it.
const CARD: f32 = 150.0;
const LABEL_H: f32 = 60.0;
const GAP: f32 = 16.0;
const PAD: f32 = 20.0;
const HEADER_H: f32 = 44.0;
/// Found people listed at most.
const FOUND_MAX: usize = 400;

/// An engine error without its "invalid parameters for `cmd`:" lead-in.
fn plain(e: &str) -> String {
    e.split_once("`: ").map_or(e, |(_, rest)| rest).to_string()
}

/// The people found in the photos that nobody has named (or named, with faces to confirm), as the
/// engine lists them; recomputed when the faces, the catalog or the user's naming change.
fn found(app: &mut LightcraftApp) -> std::sync::Arc<Vec<Value>> {
    let v = &app.session.vision;
    let key = (v.faces_found(), v.faces_scanned(), app.session.catalog.revision, v.faces, v.people_rev());
    if let Some((k, list)) = &app.ui.people_found
        && *k == key
    {
        return list.clone();
    }
    let list = if key.3 || key.0 > 0 || v.server_faces() {
        app.session.people_list(false, FOUND_MAX, false).ok().and_then(|v| v["people"].as_array().cloned()).unwrap_or_default()
    } else {
        Vec::new()
    };
    let list = std::sync::Arc::new(list);
    app.ui.people_found = Some((key, list.clone()));
    list
}

/// What the top of the view has to say about finding people.
fn bar(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let v = &app.session.vision;
    if !v.faces_available() {
        return;
    }
    let t = Tokens::get(ui.ctx());
    let downloading = v.faces_download_status();
    let installed = v.faces_installed();
    let running = v
        .job()
        .filter(|j| !j.finished.load(std::sync::atomic::Ordering::Relaxed))
        .map(|j| (j.done.load(std::sync::atomic::Ordering::Relaxed), j.total, j.phase()));
    let (enabled, found_n, scanned) = (v.faces, v.faces_found(), v.faces_scanned());
    let photos = app.session.vision_photo_count();
    egui::Frame::new()
        .fill(t.chrome)
        .stroke(Stroke::new(1.0, t.field_border))
        .corner_radius(6.0)
        .inner_margin(12.0)
        .outer_margin(egui::Margin::symmetric(PAD as i8, 0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 6.0;
            if let Some(e) = app.ui.people_error.clone() {
                ui.label(egui::RichText::new(&e).color(Color32::from_rgb(230, 90, 80)));
                let r = ui.button(crate::i18n::tr("OK"));
                register(ui.ctx(), "button:peopleDismiss", r.rect);
                if r.clicked() {
                    app.ui.people_error = None;
                }
            } else if downloading["running"].as_bool() == Some(true) {
                let (done, total) = (downloading["done"].as_u64().unwrap_or(0), downloading["total"].as_u64().unwrap_or(0));
                let frac = if total > 0 { done as f64 / total as f64 } else { 0.0 };
                let r = ui.add(egui::ProgressBar::new(frac as f32).text(format!("{:.1} / {:.1} MB", done as f64 / 1e6, total as f64 / 1e6)));
                register(ui.ctx(), "progress:peopleDownload", r.rect);
                let r = ui.button(crate::i18n::tr("Cancel Download"));
                register(ui.ctx(), "button:peopleCancel", r.rect);
                if r.clicked() {
                    app.session.vision.cancel_faces_download();
                }
            } else if app.ui.faces_offer && !installed {
                offer(app, ui, &t);
            } else if !enabled {
                ui.label(crate::i18n::tr("Find the people in your photos, and name them once for all their photos."));
                ui.label(
                    egui::RichText::new(crate::i18n::tr(
                        "It runs on this computer, only when you ask, and you can forget everything it found at any time.",
                    ))
                    .color(t.text_dim),
                );
                let r = ui.button(crate::i18n::tr("Find People…"));
                register(ui.ctx(), "button:peopleFind", r.rect);
                if r.clicked() {
                    if installed {
                        turn_on(app);
                    } else {
                        app.ui.faces_offer = true;
                    }
                }
            } else if let Some((done, total, phase)) = running {
                let frac = if total > 0 { done as f32 / total as f32 } else { 0.0 };
                let text = if phase == "finding" {
                    crate::i18n::tr_format!("Looking for faces: {done} of {total}", done = done, total = total)
                } else {
                    crate::i18n::tr_format!("Getting your photos ready to search: {done} of {total}", done = done, total = total)
                };
                let r = ui.add(egui::ProgressBar::new(frac).text(text));
                register(ui.ctx(), "progress:peopleScan", r.rect);
                let r = ui.button(crate::i18n::tr("Stop"));
                register(ui.ctx(), "button:peopleStop", r.rect);
                if r.clicked() {
                    let _ = app.run("vision.indexCancel", json!({}));
                }
            } else {
                if app.session.vision.share_with_server && app.session.vision.server_faces() {
                    ui.label(
                        egui::RichText::new(crate::i18n::tr("Because you send your search data to your server, the faces found here go there too."))
                            .color(t.text_dim),
                    );
                }
                ui.horizontal_wrapped(|ui| {
                    ui.label(crate::i18n::tr_format!("{faces} faces found in {photos} photos.", faces = found_n, photos = scanned));
                    if scanned < photos {
                        let r = ui.button(crate::i18n::tr("Look in the Rest"));
                        register(ui.ctx(), "button:peopleScanRest", r.rect);
                        if r.clicked()
                            && let Err(e) = app.run("vision.index", json!({}))
                        {
                            app.ui.people_error = Some(plain(&e));
                        }
                    }
                    let r = ui.button(crate::i18n::tr("Turn Off"));
                    register(ui.ctx(), "button:peopleOff", r.rect);
                    if r.clicked() {
                        let _ = app.run("vision.setFaces", json!({"on": false}));
                    }
                    let r = ui.button(crate::i18n::tr("Forget All Faces"));
                    register(ui.ctx(), "button:peopleForget", r.rect);
                    if r.clicked() {
                        let _ = app.run("vision.setFaces", json!({"on": false}));
                        if let Err(e) = app.run("people.deleteData", json!({})) {
                            app.ui.people_error = Some(plain(&e));
                        }
                        app.ui.people_found = None;
                    }
                });
            }
        });
    ui.add_space(8.0);
}

/// Asking before the face models are downloaded: what they are, their size and licences, and what
/// happens to the faces.
fn offer(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    use lightcraft_engine::vision::{FACE_BYTES, FACE_LICENSES};
    ui.label(crate::i18n::tr(
        "Finding people uses two small AI models: one finds the faces in your photos and one tells them apart. They aren't part of LightCraft, and everything else works without them.",
    ));
    let dir = app.session.vision.faces_dir().map(|d| d.display().to_string()).unwrap_or_default();
    ui.label(format!("{} {:.0} MB, {} {dir}", crate::i18n::tr("A one-time download of about"), FACE_BYTES as f64 / 1e6, crate::i18n::tr("saved in")));
    for (name, url) in FACE_LICENSES {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(format!("{} {name}", crate::i18n::tr("Licence:"))).color(t.text_label));
            let r = ui.link(crate::i18n::tr("Read the licence")).on_hover_text(url);
            register(ui.ctx(), format!("link:peopleLicense:{url}"), r.rect);
            if r.clicked() {
                let _ = crate::links::open(app, url);
            }
        });
    }
    ui.label(
        egui::RichText::new(crate::i18n::tr(
            "These models were trained on collections of public photos of people; their makers publish them for general use. What they find in your photos stays on this computer unless you choose to share it with your own server. You can forget it all at any time. Naming a person writes their name on the photos they are in.",
        ))
        .color(t.text_dim),
    );
    ui.horizontal(|ui| {
        let r = ui.button(crate::i18n::tr("Download and Turn On"));
        register(ui.ctx(), "button:peopleDownload", r.rect);
        if r.clicked() {
            match app.run("vision.faces.download", json!({"acknowledged": true})) {
                Ok(_) => {
                    app.ui.faces_offer = false;
                    app.ui.faces_downloading = true;
                    let _ = app.run("vision.setFaces", json!({"on": true}));
                }
                Err(e) => app.ui.people_error = Some(plain(&e)),
            }
        }
        let r = ui.button(crate::i18n::tr("Not Now"));
        register(ui.ctx(), "button:peopleLater", r.rect);
        if r.clicked() {
            app.ui.faces_offer = false;
        }
    });
}

/// Turn finding on (the models are in place) and look at the photos.
fn turn_on(app: &mut LightcraftApp) {
    let _ = app.run("vision.setFaces", json!({"on": true}));
    if let Err(e) = app.run("vision.index", json!({})) {
        app.ui.people_error = Some(plain(&e));
    }
}

/// Once per frame: when the face models finished downloading, say so and start looking.
pub fn frame(app: &mut LightcraftApp, ctx: &egui::Context) {
    let d = app.session.vision.faces_download_status();
    let running = d["running"].as_bool() == Some(true);
    if running {
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }
    if app.ui.faces_downloading && !running {
        app.ui.faces_downloading = false;
        match (d["error"].as_str(), d["finished"].as_bool() == Some(true)) {
            (Some(e), _) => {
                let _ = app.run("vision.setFaces", json!({"on": false}));
                if !e.contains("cancelled") {
                    app.ui.people_error = Some(format!("{} {e}", crate::i18n::tr("The download didn't work:")));
                }
            }
            (None, true) => {
                app.toast_for(ctx, crate::i18n::tr("The face models are installed: looking for faces in your photos."), 4.0);
                turn_on(app);
            }
            (None, false) => {}
        }
    }
    // a run that is looking for faces keeps the cards fresh
    if app.session.vision.job().is_some_and(|j| !j.finished.load(std::sync::atomic::Ordering::Relaxed)) {
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
    }
}

pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let named = app.caches.people(&app.session.catalog, &app.session.filter);
    let chips = lightcraft_engine::filter_chips(&app.session.filter, &app.session.catalog);
    // (people found in photos aren't narrowed by the filters; they are offered when there are none)
    let others = if chips.is_empty() { found(app) } else { std::sync::Arc::new(Vec::new()) };
    let total = named.len() + others.len();
    let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), HEADER_H), Sense::hover());
    ui.painter().text(pos2(head.left() + PAD, head.center().y), Align2::LEFT_CENTER, crate::i18n::tr("People"), t.semibold(15.0), t.text);
    ui.painter().text(pos2(head.right() - PAD, head.center().y), Align2::RIGHT_CENTER, total.to_string(), t.font(13.0), t.text_dim);
    // the filters narrowing the list (a date, a keyword…), removable here
    let chips = super::chips::with_understanding(app, chips);
    super::chips::show(app, ui, &chips);
    bar(app, ui);
    if total == 0 {
        let (title, body) = if chips.is_empty() {
            ("No people yet", "Names written to XMP by Lightroom and other apps show up here, and so do the people found in your photos.")
        } else {
            ("No named people in these photos", "Remove a filter above, or choose Clear all")
        };
        super::empty_message(ui, ui.available_rect_before_wrap(), title, body);
        return;
    }
    let ppp = ui.ctx().pixels_per_point();
    let active = app.session.filter.person.clone();
    egui::ScrollArea::vertical().auto_shrink(false).show_viewport(ui, |ui, viewport| {
        let width = ui.available_width();
        let cols = (((width - PAD * 2.0 + GAP) / (CARD + GAP)).floor() as usize).max(1);
        let row_h = CARD + LABEL_H + GAP;
        let rows = total.div_ceil(cols);
        let (area, _) = ui.allocate_exact_size(vec2(width, PAD * 2.0 + rows as f32 * row_h), Sense::hover());
        let first = ((viewport.top() - PAD) / row_h).floor().max(0.0) as usize;
        let last = (((viewport.bottom() - PAD) / row_h).ceil().max(0.0) as usize).min(rows);
        for row in first..last {
            for col in 0..cols {
                let i = row * cols + col;
                if i >= total {
                    break;
                }
                let min = area.min + vec2(PAD + col as f32 * (CARD + GAP), PAD + row as f32 * row_h);
                let r = Rect::from_min_size(min, vec2(CARD, CARD + LABEL_H));
                match named.get(i) {
                    Some(person) => {
                        let selected = active.as_deref().is_some_and(|a| a.eq_ignore_ascii_case(&person.name));
                        card(app, ui, person, r, ppp, selected);
                    }
                    None => {
                        if let Some(p) = others.get(i - named.len()) {
                            found_card(app, ui, p, r, ppp);
                        }
                    }
                }
            }
        }
    });
}

/// The close-up of a face on a card.
fn face_picture(app: &mut LightcraftApp, ui: &egui::Ui, photo: lightcraft_catalog::PhotoId, face: lightcraft_geom::Rect, r: Rect, ppp: f32) {
    let t = Tokens::get(ui.ctx());
    let p = ui.painter();
    p.rect_filled(r, 3.0, t.canvas);
    if let Some(job) = app.session.face_job(photo, face, (CARD * ppp).ceil() as usize)
        && let Some(tex) = app.renderer.variant(job)
    {
        p.image(tex.tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    }
}

fn card(app: &mut LightcraftApp, ui: &mut egui::Ui, person: &Person, r: Rect, ppp: f32, selected: bool) {
    let t = Tokens::get(ui.ctx());
    let face = Rect::from_min_size(r.min, vec2(CARD, CARD));
    let resp = ui.interact(r, egui::Id::new(("person-card", &person.name)), Sense::click());
    register(ui.ctx(), format!("person:{}", person.name), r);
    face_picture(app, ui, person.photo, person.face, face, ppp);
    let p = ui.painter();
    if selected {
        p.rect_stroke(face, 3.0, Stroke::new(2.0, Color32::WHITE), StrokeKind::Outside);
    } else if resp.hovered() {
        p.rect_stroke(face, 3.0, Stroke::new(1.0, t.text_dim), StrokeKind::Outside);
    }
    // a long name must not run past the card
    let name =
        if person.name.chars().count() > 19 { format!("{}…", person.name.chars().take(18).collect::<String>()) } else { person.name.clone() };
    p.text(pos2(r.left() + 2.0, face.bottom() + 14.0), Align2::LEFT_CENTER, name, t.semibold(13.0), t.text);
    let photos = if person.count == 1 { "1 photo".to_string() } else { format!("{} photos", person.count) };
    p.text(pos2(r.left() + 2.0, face.bottom() + 32.0), Align2::LEFT_CENTER, photos, t.font(12.0), t.text_dim);
    if resp.on_hover_text(format!("{} — show their photos", person.name)).clicked() {
        let _ = app.run("library.filter", json!({"person": person.name}));
        let _ = app.run("view.photoGrid", json!({}));
    }
}

/// A person found in the photos: not named yet (a field to name them), or named with faces that
/// haven't been written to their photos (a button to confirm).
fn found_card(app: &mut LightcraftApp, ui: &mut egui::Ui, p: &Value, r: Rect, ppp: f32) {
    let t = Tokens::get(ui.ctx());
    let Some(id) = p["id"].as_str() else { return };
    let face = Rect::from_min_size(r.min, vec2(CARD, CARD));
    let resp = ui.interact(face, egui::Id::new(("found-card", id)), Sense::click());
    register(ui.ctx(), format!("person:found:{id}"), r);
    let (photo, rect) = (p["cover"]["photo"].as_u64().map(lightcraft_catalog::PhotoId), &p["cover"]["rect"]);
    let num = |k: &str| rect[k].as_f64().unwrap_or(0.0);
    if let Some(photo) = photo {
        face_picture(app, ui, photo, lightcraft_geom::Rect { x0: num("x0"), y0: num("y0"), x1: num("x1"), y1: num("y1") }, face, ppp);
    }
    let painter = ui.painter();
    if resp.hovered() {
        painter.rect_stroke(face, 3.0, Stroke::new(1.0, t.text_dim), StrokeKind::Outside);
    }
    let photos = p["photos"].as_u64().unwrap_or(0);
    let count = if photos == 1 { "1 photo".to_string() } else { format!("{photos} photos") };
    match p["name"].as_str() {
        Some(name) => {
            // named, with faces found since: confirm them
            let pending = p["pending"].as_u64().unwrap_or(0);
            let shown = if name.chars().count() > 19 { format!("{}…", name.chars().take(18).collect::<String>()) } else { name.to_string() };
            painter.text(pos2(r.left() + 2.0, face.bottom() + 14.0), Align2::LEFT_CENTER, shown, t.semibold(13.0), t.text);
            let b = Rect::from_min_size(pos2(r.left(), face.bottom() + 28.0), vec2(CARD, 26.0));
            let text = crate::i18n::tr_format!("Confirm {pending} more", pending = pending);
            let res = ui.put(b, egui::Button::new(text));
            register(ui.ctx(), format!("button:peopleConfirm:{id}"), res.rect);
            if res.clicked()
                && let Err(e) = app.run("people.name", json!({"cluster": id, "name": name}))
            {
                app.ui.people_error = Some(plain(&e));
            }
        }
        None => {
            painter.text(pos2(r.left() + 2.0, face.bottom() + 46.0), Align2::LEFT_CENTER, count, t.font(12.0), t.text_dim);
            let b = Rect::from_min_size(pos2(r.left(), face.bottom() + 6.0), vec2(CARD, 26.0));
            let mut text = app.ui.people_names.get(id).cloned().unwrap_or_default();
            let res = ui.put(b, egui::TextEdit::singleline(&mut text).hint_text(crate::i18n::tr("Who is this?")));
            register(ui.ctx(), format!("field:personName:{id}"), res.rect);
            app.ui.people_names.insert(id.to_string(), text.clone());
            if res.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !text.trim().is_empty() {
                match app.run("people.name", json!({"cluster": id, "name": text.trim()})) {
                    Ok(_) => {
                        app.ui.people_names.remove(id);
                    }
                    Err(e) => app.ui.people_error = Some(plain(&e)),
                }
            }
        }
    }
    if resp.on_hover_text(crate::i18n::tr("Show their photos")).clicked() {
        let _ = app.run("people.show", json!({"cluster": id}));
        let _ = app.run("view.photoGrid", json!({}));
    }
}
