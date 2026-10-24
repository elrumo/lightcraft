//! People: a card per person named on faces (read from XMP): a close-up of their face and how many photos they are in
//! (a small number on the picture). A click opens that person's page (`person.rs`): only their cropped faces, and the
//! faces that look like them. Below the people, the faces nobody has named yet (`unnamed.rs`), to be named a group at a
//! time.
//!
//! When the user has turned finding people on (`vision`: the models and licences are shown first, nothing is downloaded
//! or looked at until they say so — the bar at the top of this view), the people it found and grouped and nobody has
//! named yet follow the named ones, each with a field to type their name in.
//!
//! The thumbnail-size slider in the bottom bar sizes the faces here too (with its own smaller and larger limits), and
//! every face is cut from the same kind of box (the detector's, once the scan has looked at it), so faces are shown
//! equally close. Only the rows on screen ask for a face render (the engine caches them, memory and disk).

use egui::{Align2, Color32, Rect, RichText, Sense, Stroke, StrokeKind, pos2, vec2};
use lightcraft_catalog::Person;
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::theme::Tokens;
use crate::widgets::register;

const GAP: f32 = 14.0;
pub(super) const PAD: f32 = 20.0;
const HEADER_H: f32 = 44.0;
/// The name under a card's picture.
const NAME_H: f32 = 26.0;
/// The unnamed-faces heading and its hints.
const SECTION_H: f32 = 70.0;
/// The found-people heading.
const FOUND_H: f32 = 44.0;
/// Under a found person's picture: the name field (or name and confirm button) and the count.
const FOUND_LABEL_H: f32 = 60.0;
/// Found people listed at most.
const FOUND_MAX: usize = 400;

/// `thumb` (the thumbnail slider, 90 to 480) as a position between `low` and `high`.
fn between(thumb: f32, low: f32, high: f32) -> f32 {
    let f = (thumb - 90.0) / (480.0 - 90.0);
    low + if f.is_finite() { f.clamp(0.0, 1.0) } else { 0.4 } * (high - low)
}

/// A person card's face picture (points) for the thumbnail slider's value.
pub fn card_edge(thumb: f32) -> f32 {
    between(thumb, 84.0, 260.0)
}

/// A face tile's picture (points), for the unnamed faces and a person's page.
pub fn tile_edge(thumb: f32) -> f32 {
    between(thumb, 56.0, 180.0)
}

/// How many `edge`-wide cells (with the gap between them) fit across `width`.
pub(super) fn columns(width: f32, edge: f32) -> usize {
    (((width - PAD * 2.0 + GAP) / (edge + GAP)).floor() as usize).max(1)
}

pub(super) const TILE_GAP: f32 = GAP;

/// `text` shortened with "…" until it is no wider than `max_w` in `font` (measured, not guessed).
pub(super) fn fit(painter: &egui::Painter, text: &str, font: egui::FontId, max_w: f32) -> String {
    let width = |s: &str| painter.layout_no_wrap(s.to_string(), font.clone(), Color32::WHITE).size().x;
    if width(text) <= max_w {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    (1..chars.len()).rev().map(|n| chars[..n].iter().collect::<String>() + "…").find(|s| width(s) <= max_w).unwrap_or_else(|| "…".to_string())
}

/// An engine error without its "invalid parameters for `cmd`:" lead-in.
fn plain(e: &str) -> String {
    e.split_once("`: ").map_or(e, |(_, rest)| rest).to_string()
}

/// The people found in the photos that nobody has named (or named, with faces to confirm), as the
/// engine lists them; recomputed when the faces, the catalog or the user's naming change.
pub(super) fn found(app: &mut LightcraftApp) -> std::sync::Arc<Vec<Value>> {
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
pub(super) fn bar(app: &mut LightcraftApp, ui: &mut egui::Ui) {
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
    // a phone has its own, drawn as iOS draws its People album
    if app.compact {
        return super::people_mobile::show(app, ui);
    }
    if let Some(name) = app.ui.person_page.clone() {
        return super::person::show(app, ui, &name);
    }
    let t = Tokens::get(ui.ctx());
    let now = ui.input(|i| i.time);
    let thumb = app.ui.thumb_size;
    let people = app.caches.people(&app.session.catalog, &app.session.filter);
    let chips = lightcraft_engine::filter_chips(&app.session.filter, &app.session.catalog);
    // (people found in photos aren't narrowed by the filters; they are offered when there are none)
    let others = if chips.is_empty() { found(app) } else { std::sync::Arc::new(Vec::new()) };
    let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), HEADER_H), Sense::hover());
    let title =
        ui.painter().text(pos2(head.left() + PAD, head.center().y), Align2::LEFT_CENTER, crate::i18n::tr("Named People"), t.semibold(15.0), t.text);
    // the person page last left (a face opened from it, or its back button): one click returns to it
    if let Some(last) = app.ui.last_person.clone().filter(|n| people.iter().any(|p| &p.name == n)) {
        let label = fit(ui.painter(), &last, t.font(13.0), 224.0);
        let width = ui.painter().layout_no_wrap(label.clone(), t.font(13.0), Color32::WHITE).size().x + 44.0;
        let r = Rect::from_min_size(pos2(title.right() + 14.0, head.center().y - 12.0), vec2(width, 24.0));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r));
        let b = crate::widgets::arrow_button(&mut child, RichText::new(label).font(t.font(13.0)), crate::icons::Icon::ChevronRight, false);
        register(ui.ctx(), "people:last", b.rect);
        if b.on_hover_text(crate::i18n::tr("Back to this person")).clicked() {
            let _ = app.run("view.person", json!({"name": last}));
        }
    }
    ui.painter().text(pos2(head.right() - PAD, head.center().y), Align2::RIGHT_CENTER, people.len().to_string(), t.font(13.0), t.text_dim);
    // until face recognition is set up: what it takes, and a button that does the next step
    super::faces::setup_banner(app, ui);
    // finding and grouping people with the `vision` models (their own opt-in)
    bar(app, ui);
    // the filters narrowing the list (a date, a keyword…), removable here
    let chips = super::chips::with_understanding(app, chips);
    super::chips::show(app, ui, &chips);
    // what is selected below, and the box to name it with
    super::unnamed::naming_bar(app, ui);
    let unnamed = super::unnamed::faces(app, now);
    if people.is_empty() && others.is_empty() && unnamed.faces.is_empty() {
        let (title, body) = if chips.is_empty() {
            ("No people yet", "Face names written to XMP by Lightroom and other apps show up here, and faces found in your photos can be named.")
        } else {
            ("No named people in these photos", "Remove a filter above, or choose Clear all")
        };
        super::empty_message(ui, ui.available_rect_before_wrap(), title, body);
        return;
    }
    let ppp = ui.ctx().pixels_per_point();
    let active = app.session.filter.person.clone();
    let (edge, tile) = (card_edge(thumb), tile_edge(thumb));
    egui::ScrollArea::vertical().auto_shrink(false).show_viewport(ui, |ui, viewport| {
        let width = ui.available_width();
        let cols = columns(width, edge);
        let row_h = edge + NAME_H + GAP;
        let rows = people.len().div_ceil(cols);
        let named_h = if people.is_empty() { 40.0 } else { PAD + rows as f32 * row_h };
        let (found_row, found_rows) = (edge + FOUND_LABEL_H + GAP, others.len().div_ceil(cols));
        let found_top = named_h + 6.0;
        let found_h = if others.is_empty() { 0.0 } else { FOUND_H + found_rows as f32 * found_row };
        let (tile_cols, tile_row) = (columns(width, tile), tile + GAP);
        let tile_rows = unnamed.people.len().div_ceil(tile_cols);
        let section_top = found_top + found_h;
        let (area, _) = ui.allocate_exact_size(vec2(width, section_top + SECTION_H + tile_rows as f32 * tile_row + PAD), Sense::hover());
        let visible = |top: f32, rows: usize, row_h: f32| {
            let first = ((viewport.top() - top) / row_h).floor().max(0.0) as usize;
            let last = (((viewport.bottom() - top) / row_h).ceil().max(0.0) as usize).min(rows);
            first..last
        };
        if people.is_empty() {
            ui.painter().text(
                pos2(area.left() + PAD, area.top() + 24.0),
                Align2::LEFT_CENTER,
                "No named people yet. Name the faces below to start.",
                t.font(13.0),
                t.text_dim,
            );
        }
        let mut shown = 0;
        for row in visible(PAD, rows, row_h) {
            for col in 0..cols {
                let Some(person) = people.get(row * cols + col) else { break };
                shown += 1;
                let min = area.min + vec2(PAD + col as f32 * (edge + GAP), PAD + row as f32 * row_h);
                let selected = active.as_deref().is_some_and(|a| a.eq_ignore_ascii_case(&person.name));
                card(app, ui, person, Rect::from_min_size(min, vec2(edge, edge + NAME_H)), edge, ppp, selected);
            }
        }
        // the people `vision` found and nobody has named (or named, with faces to confirm)
        if !others.is_empty() {
            let top = area.top() + found_top;
            ui.painter().line_segment([pos2(area.left() + PAD, top), pos2(area.right() - PAD, top)], Stroke::new(1.0, t.button_border));
            ui.painter().text(
                pos2(area.left() + PAD, top + FOUND_H / 2.0),
                Align2::LEFT_CENTER,
                crate::i18n::tr("Found in Your Photos"),
                t.semibold(13.0),
                t.text_label,
            );
            let grid_top = found_top + FOUND_H;
            for row in visible(grid_top, found_rows, found_row) {
                for col in 0..cols {
                    let Some(p) = others.get(row * cols + col) else { break };
                    shown += 1;
                    let min = area.min + vec2(PAD + col as f32 * (edge + GAP), grid_top + row as f32 * found_row);
                    found_card(app, ui, p, Rect::from_min_size(min, vec2(edge, edge + FOUND_LABEL_H)), edge, ppp);
                }
            }
        }
        // the faces nobody has named, a picture per person
        let top = area.top() + section_top;
        ui.painter().line_segment([pos2(area.left() + PAD, top), pos2(area.right() - PAD, top)], Stroke::new(1.0, t.button_border));
        super::unnamed::section_header(app, ui, &unnamed, pos2(area.left() + PAD, top + 14.0), area.right() - PAD);
        let grid_top = section_top + SECTION_H;
        for row in visible(grid_top, tile_rows, tile_row) {
            for col in 0..tile_cols {
                let i = row * tile_cols + col;
                if i >= unnamed.people.len() {
                    break;
                }
                shown += 1;
                let min = area.min + vec2(PAD + col as f32 * (tile + GAP), grid_top + row as f32 * tile_row);
                super::unnamed::tile(app, ui, &unnamed, i, Rect::from_min_size(min, vec2(tile, tile)), ppp);
            }
        }
        // the picture cache keeps all of them (a fixed budget would evict and re-request the same few every frame)
        app.renderer.want_variants(shown);
    });
}

fn card(app: &mut LightcraftApp, ui: &mut egui::Ui, person: &Person, r: Rect, edge: f32, ppp: f32, selected: bool) {
    let t = Tokens::get(ui.ctx());
    let face = Rect::from_min_size(r.min, vec2(edge, edge));
    let resp = ui.interact(r, egui::Id::new(("person-card", &person.name)), Sense::click());
    crate::widgets::announce(&resp, person.name.clone());
    register(ui.ctx(), format!("person:{}", person.name), r);
    let view = app.session.face_view(person.photo, person.face);
    let p = ui.painter();
    p.rect_filled(face, 3.0, t.canvas);
    if let Some(job) = app.session.face_job(person.photo, view, (edge * ppp).ceil() as usize)
        && let Some(tex) = app.renderer.variant(job)
    {
        p.image(tex.tex.id(), face, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    }
    // how many photos they are in, on the picture
    let count = if person.count > 999 { "999+".to_string() } else { person.count.to_string() };
    let g = p.layout_no_wrap(count, t.font(11.5), Color32::WHITE);
    let pill = Rect::from_min_size(pos2(face.right() - g.size().x - 14.0, face.bottom() - g.size().y - 11.0), g.size() + vec2(10.0, 6.0));
    p.rect_filled(pill, 4.0, Color32::from_black_alpha(175));
    p.galley(pill.min + vec2(5.0, 3.0), g, Color32::WHITE);
    if selected {
        p.rect_stroke(face, 3.0, Stroke::new(2.0, Color32::WHITE), StrokeKind::Outside);
    } else if resp.hovered() {
        p.rect_stroke(face, 3.0, Stroke::new(1.0, t.text_dim), StrokeKind::Outside);
    }
    // a long name must not run past the card
    let name = fit(p, &person.name, t.semibold(13.0), edge);
    p.text(pos2(r.left() + 2.0, face.bottom() + 14.0), Align2::LEFT_CENTER, name, t.semibold(13.0), t.text);
    let photos = crate::i18n::tr_format!("{n} photo{}", if person.count == 1 { "" } else { "s" }, n = person.count);
    p.text(pos2(r.left() + 2.0, face.bottom() + 32.0), Align2::LEFT_CENTER, photos, t.font(12.0), t.text_dim);
    let resp = resp.on_hover_text(crate::i18n::tr_format!("{} — show their photos", person.name));
    let full = person.name.clone();
    crate::widgets::context_menu(&resp, |ui| {
        let r = ui.button(crate::i18n::tr("Rename Person…"));
        register(ui.ctx(), "personMenu:rename", r.rect);
        if r.clicked() {
            open_rename(app, &full);
        }
    });
    if resp.clicked() {
        let _ = app.run("view.person", json!({"name": person.name}));
    }
}

/// The Rename Person dialog, prefilled with the name the card shows.
pub(crate) fn open_rename(app: &mut LightcraftApp, name: &str) {
    app.ui.dialog = Some(crate::state::Dialog::RenamePerson { from: name.to_string(), to: name.to_string(), merge: false });
}

/// The close-up of a found person's face (`edge` points across).
fn face_picture(
    app: &mut LightcraftApp,
    ui: &egui::Ui,
    photo: lightcraft_catalog::PhotoId,
    face: lightcraft_geom::Rect,
    r: Rect,
    edge: f32,
    ppp: f32,
) {
    let t = Tokens::get(ui.ctx());
    let p = ui.painter();
    p.rect_filled(r, 3.0, t.canvas);
    if let Some(job) = app.session.face_job(photo, face, (edge * ppp).ceil() as usize)
        && let Some(tex) = app.renderer.variant(job)
    {
        p.image(tex.tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    }
}

/// A person found in the photos: not named yet (a field to name them), or named with faces that
/// haven't been written to their photos (a button to confirm).
fn found_card(app: &mut LightcraftApp, ui: &mut egui::Ui, p: &Value, r: Rect, edge: f32, ppp: f32) {
    let t = Tokens::get(ui.ctx());
    let Some(id) = p["id"].as_str() else { return };
    let face = Rect::from_min_size(r.min, vec2(edge, edge));
    let resp = ui.interact(face, egui::Id::new(("found-card", id)), Sense::click());
    register(ui.ctx(), format!("person:found:{id}"), r);
    let (photo, rect) = (p["cover"]["photo"].as_u64().map(lightcraft_catalog::PhotoId), &p["cover"]["rect"]);
    let num = |k: &str| rect[k].as_f64().unwrap_or(0.0);
    if let Some(photo) = photo {
        face_picture(app, ui, photo, lightcraft_geom::Rect { x0: num("x0"), y0: num("y0"), x1: num("x1"), y1: num("y1") }, face, edge, ppp);
    }
    let painter = ui.painter();
    if resp.hovered() {
        painter.rect_stroke(face, 3.0, Stroke::new(1.0, t.text_dim), StrokeKind::Outside);
    }
    let photos = p["photos"].as_u64().unwrap_or(0);
    let count = crate::i18n::tr_format!("{n} photo{}", if photos == 1 { "" } else { "s" }, n = photos);
    match p["name"].as_str() {
        Some(name) => {
            // named, with faces found since: confirm them
            let pending = p["pending"].as_u64().unwrap_or(0);
            let shown = fit(painter, name, t.semibold(13.0), edge);
            painter.text(pos2(r.left() + 2.0, face.bottom() + 14.0), Align2::LEFT_CENTER, shown, t.semibold(13.0), t.text);
            let b = Rect::from_min_size(pos2(r.left(), face.bottom() + 28.0), vec2(edge, 26.0));
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
            let b = Rect::from_min_size(pos2(r.left(), face.bottom() + 6.0), vec2(edge, 26.0));
            let mut text = app.ui.people_names.get(id).cloned().unwrap_or_default();
            let res =
                ui.put(b, egui::TextEdit::singleline(&mut text).hint_text(crate::i18n::tr("Who is this?")).margin(egui::Margin::symmetric(8, 4)));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn faces_follow_the_thumbnail_slider_within_their_own_limits() {
        assert_eq!((card_edge(90.0), card_edge(480.0)), (84.0, 260.0));
        assert_eq!((tile_edge(90.0), tile_edge(480.0)), (56.0, 180.0));
        // between, growing with the slider; beyond its ends, held at the limits; nonsense, a middle size
        assert!(card_edge(220.0) > card_edge(150.0) && tile_edge(300.0) > tile_edge(220.0));
        assert_eq!((card_edge(-5.0), card_edge(10_000.0)), (84.0, 260.0));
        assert!(card_edge(f32::NAN) > 84.0 && card_edge(f32::NAN) < 260.0);
        // there is always at least one column, whatever the width
        assert_eq!((columns(0.0, 260.0), columns(-50.0, 260.0), columns(f32::NAN, 100.0)), (1, 1, 1));
        assert!(columns(1400.0, 84.0) > columns(1400.0, 260.0));
    }
}
