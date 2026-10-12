//! People on a phone (`ViewMode::People` in the compact layout), drawn the way iOS's Photos shows its
//! People album: round faces three across with each person's name and photo count, then the people
//! the face finder grouped and nobody has named (tap one to name them all at once), then the faces
//! nobody has named one by one (tap one to name it). A person's page shows their face large, their
//! name (tap it to rename them everywhere), their photos' faces (tap one to open the photo) and the
//! faces that look like them, each with ✓ (it is them) and ✕ (it isn't). Every name is typed on one
//! page, [`naming_page`], which the photo editor opens too when a face on the photo is tapped, or a
//! face is marked by hand ("Tag a Face" in the photo's "…" menu). The engine does the looking and
//! the naming (`faces.*`, `people.*`, `photo.addRegion`); this draws and taps.

use egui::{Align2, Color32, Rect, Sense, Stroke, pos2, vec2};
use lightcraft_catalog::PhotoId;
use serde_json::{Value, json};

use super::mobile;
use crate::LightcraftApp;
use crate::icons::{Icon, paint};
use crate::theme::Tokens;
use crate::widgets::register;

/// Side margins and the gap between faces (points).
const PAD: f32 = 16.0;
const GAP: f32 = 14.0;
/// Under a face: its name and count.
const LABEL_H: f32 = 46.0;
/// A section's heading.
const HEAD_H: f32 = 44.0;

/// What a naming page names.
#[derive(Clone, Debug, PartialEq)]
pub enum NameTarget {
    /// A face region on a photo (unnamed, or named: renaming that one face).
    Face { photo: u64, index: usize },
    /// A person the face finder grouped (`people.name`), shown by one of their faces.
    Cluster { id: String, photo: u64, rect: lightcraft_geom::Rect },
    /// Everyone named `from`: renaming a person on every photo.
    Person { from: String },
    /// A face marked by hand (`photo.addRegion`), in the photo's upright frame.
    New { photo: u64, rect: lightcraft_geom::Rect },
}

/// The naming page while it is open: what it names and what has been typed.
#[derive(Clone, Debug, PartialEq)]
pub struct Naming {
    pub target: NameTarget,
    pub text: String,
    /// The field takes the keyboard on the next frame.
    pub fresh: bool,
}

impl Naming {
    pub fn new(target: NameTarget, text: impl Into<String>) -> Naming {
        Naming { target, text: text.into(), fresh: true }
    }
}

/// A face (`view`: the box to show it by) cut out round, `r` across.
fn disc(app: &mut LightcraftApp, ui: &egui::Ui, photo: PhotoId, view: lightcraft_geom::Rect, r: Rect) {
    let t = Tokens::get(ui.ctx());
    let p = ui.painter();
    let px = (r.width() * ui.ctx().pixels_per_point()).ceil() as usize;
    let round = (r.width() / 2.0).min(255.0) as u8;
    match app.session.face_job(photo, view, px).and_then(|job| app.renderer.variant(job).map(|tex| tex.tex.id())) {
        Some(tex) => {
            p.add(egui::epaint::RectShape::filled(r, round, Color32::WHITE).with_texture(tex, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))));
        }
        None => {
            p.rect_filled(r, round, if Tokens::is_dark(ui.ctx()) { Color32::from_gray(44) } else { t.inset });
            paint(p, Rect::from_center_size(r.center(), r.size() * 0.45), Icon::Person, t.text_dim);
        }
    }
}

/// `text` cut with "…" to `w` points.
fn fit(ui: &egui::Ui, text: &str, font: egui::FontId, w: f32) -> String {
    super::people::fit(ui.painter(), text, font, w)
}

/// "1 photo", "5 photos".
fn photo_count(n: u64) -> String {
    crate::i18n::tr_format!("{n} photo{}", if n == 1 { "" } else { "s" }, n = n)
}

/// A small grey heading over a section.
fn heading(ui: &egui::Ui, at: egui::Pos2, text: &str) {
    let t = Tokens::get(ui.ctx());
    ui.painter().text(at, Align2::LEFT_CENTER, crate::i18n::tr(text).to_uppercase(), t.font(13.0), t.text_dim);
}

/// The People view on a phone.
pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    if let Some(name) = app.ui.person_page.clone() {
        return person(app, ui, &name);
    }
    let t = Tokens::get(ui.ctx());
    let now = ui.input(|i| i.time);
    let people = app.caches.people(&app.session.catalog, &app.session.filter);
    let chips = lightcraft_engine::filter_chips(&app.session.filter, &app.session.catalog);
    let found = if chips.is_empty() { super::people::found(app) } else { std::sync::Arc::new(Vec::new()) };
    // the large title, as the grid has its collection's
    let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), 56.0), Sense::hover());
    let title = ui.painter().layout_no_wrap(crate::i18n::tr("People").to_string(), t.semibold(28.0), t.text);
    let tw = title.size().x;
    ui.painter().galley(pos2(head.left() + PAD, head.center().y - title.size().y / 2.0), title, t.text);
    ui.painter().text(
        pos2(head.left() + PAD + tw + 10.0, head.center().y + 3.0),
        Align2::LEFT_CENTER,
        people.len().to_string(),
        t.font(13.0),
        t.text_dim,
    );
    register(ui.ctx(), "people:title", head);
    // finding people: what it takes, and its progress (the desktop's own bar and banner)
    super::faces::setup_banner(app, ui);
    super::people::bar(app, ui);
    let chips = super::chips::with_understanding(app, chips);
    super::chips::show(app, ui, &chips);
    let unnamed = super::unnamed::faces(app, now);
    if people.is_empty() && found.is_empty() && unnamed.faces.is_empty() {
        super::empty_message(
            ui,
            ui.available_rect_before_wrap(),
            "No people yet",
            "Faces named by other apps show up here. Turn on finding people above, or open a photo and tap Tag a Face in its … menu.",
        );
        return;
    }
    let mut tap: Option<Tap> = None;
    egui::ScrollArea::vertical().id_salt("people-phone").auto_shrink(false).show_viewport(ui, |ui, viewport| {
        let w = ui.available_width();
        let d = ((w - 2.0 * PAD - 2.0 * GAP) / 3.0).floor().max(40.0);
        let small = ((w - 2.0 * PAD - 3.0 * GAP) / 4.0).floor().max(30.0);
        let row = d + LABEL_H + GAP;
        let srow = small + 26.0 + GAP;
        let rows = |n: usize, per: usize| n.div_ceil(per);
        // the sections, top to bottom: (heading's top, rows, row height, faces across)
        let mut y = 4.0;
        let named_top = y;
        y += if people.is_empty() { 0.0 } else { HEAD_H + rows(people.len(), 3) as f32 * row };
        let found_top = y;
        y += if found.is_empty() { 0.0 } else { HEAD_H + rows(found.len(), 3) as f32 * row };
        let unnamed_top = y;
        y += if unnamed.faces.is_empty() { 0.0 } else { HEAD_H + rows(unnamed.faces.len(), 4) as f32 * srow };
        let (area, _) = ui.allocate_exact_size(vec2(w, y + 24.0), Sense::hover());
        let o = area.min;
        let visible = |top: f32, n: usize, h: f32| {
            let first = ((viewport.top() - top) / h).floor().max(0.0) as usize;
            let last = (((viewport.bottom() - top) / h).ceil().max(0.0) as usize).min(n);
            first..last
        };
        let mut shown = 0;
        let cell = |k: usize, per: usize, size: f32, top: f32, h: f32| {
            let (r, c) = (k / per, k % per);
            Rect::from_min_size(o + vec2(PAD + c as f32 * (size + GAP), top + HEAD_H + r as f32 * h), vec2(size, size))
        };
        if !people.is_empty() {
            heading(ui, o + vec2(PAD, named_top + HEAD_H / 2.0 + 6.0), "People");
            for r in visible(named_top + HEAD_H, rows(people.len(), 3), row) {
                for k in r * 3..((r + 1) * 3).min(people.len()) {
                    let Some(person) = people.get(k) else { continue };
                    shown += 1;
                    let face = cell(k, 3, d, named_top, row);
                    let view = app.session.face_view(person.photo, person.face);
                    disc(app, ui, person.photo, view, face);
                    let name = fit(ui, &person.name, t.semibold(15.0), d + GAP);
                    ui.painter().text(pos2(face.center().x, face.bottom() + 14.0), Align2::CENTER_CENTER, name, t.semibold(15.0), t.text);
                    let n = photo_count(person.count as u64);
                    ui.painter().text(pos2(face.center().x, face.bottom() + 33.0), Align2::CENTER_CENTER, n, t.font(13.0), t.text_dim);
                    let hit = face.union(Rect::from_min_size(face.left_bottom(), vec2(d, LABEL_H)));
                    let resp = ui.interact(hit, egui::Id::new(("phone-person", &person.name)), Sense::click());
                    register(ui.ctx(), format!("person:{}", person.name), hit);
                    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &person.name));
                    if resp.clicked() {
                        tap = Some(Tap::Person(person.name.clone()));
                    }
                }
            }
        }
        if !found.is_empty() {
            heading(ui, o + vec2(PAD, found_top + HEAD_H / 2.0 + 6.0), "Found in Your Photos");
            for r in visible(found_top + HEAD_H, rows(found.len(), 3), row) {
                for k in r * 3..((r + 1) * 3).min(found.len()) {
                    let Some(p) = found.get(k) else { continue };
                    let Some(id) = p["id"].as_str() else { continue };
                    shown += 1;
                    let face = cell(k, 3, d, found_top, row);
                    let rect = rect_of(&p["cover"]["rect"]);
                    let photo = p["cover"]["photo"].as_u64();
                    if let (Some(photo), Some(rect)) = (photo, rect) {
                        disc(app, ui, PhotoId(photo), rect, face);
                    }
                    let n_photos = p["photos"].as_u64().unwrap_or(0);
                    let (line, ink) = match p["name"].as_str() {
                        Some(name) => (fit(ui, name, t.semibold(15.0), d + GAP), t.text),
                        None => (crate::i18n::tr("Add Name").to_string(), t.accent),
                    };
                    ui.painter().text(pos2(face.center().x, face.bottom() + 14.0), Align2::CENTER_CENTER, line, t.semibold(15.0), ink);
                    let sub = match (p["name"].as_str(), p["pending"].as_u64()) {
                        (Some(_), Some(n)) if n > 0 => crate::i18n::tr_format!("Confirm {pending} more", pending = n),
                        _ => photo_count(n_photos),
                    };
                    ui.painter().text(pos2(face.center().x, face.bottom() + 33.0), Align2::CENTER_CENTER, sub, t.font(13.0), t.text_dim);
                    let hit = face.union(Rect::from_min_size(face.left_bottom(), vec2(d, LABEL_H)));
                    let resp = ui.interact(hit, egui::Id::new(("phone-found", id)), Sense::click());
                    register(ui.ctx(), format!("person:found:{id}"), hit);
                    if resp.clicked() {
                        tap = Some(match (p["name"].as_str(), photo, rect) {
                            (Some(name), _, _) => Tap::Confirm(id.to_string(), name.to_string()),
                            (None, Some(photo), Some(rect)) => Tap::Name(Naming::new(NameTarget::Cluster { id: id.to_string(), photo, rect }, "")),
                            _ => Tap::None,
                        });
                    }
                }
            }
        }
        if !unnamed.faces.is_empty() {
            heading(ui, o + vec2(PAD, unnamed_top + HEAD_H / 2.0 + 6.0), "Unnamed Faces");
            for r in visible(unnamed_top + HEAD_H, rows(unnamed.faces.len(), 4), srow) {
                for k in r * 4..((r + 1) * 4).min(unnamed.faces.len()) {
                    let Some(f) = unnamed.faces.get(k) else { continue };
                    shown += 1;
                    let face = cell(k, 4, small, unnamed_top, srow);
                    disc(app, ui, PhotoId(f.photo), f.view, face);
                    if let Some(s) = &f.suggestion {
                        let s = fit(ui, &format!("{s}?"), t.font(13.0), small + GAP);
                        ui.painter().text(pos2(face.center().x, face.bottom() + 12.0), Align2::CENTER_CENTER, s, t.font(13.0), t.text_dim);
                    }
                    let resp = ui.interact(face, egui::Id::new(("phone-unnamed", f.photo, f.index)), Sense::click());
                    register(ui.ctx(), format!("unnamed-face:{}:{}", f.photo, f.index), face);
                    if resp.clicked() {
                        let start = f.suggestion.clone().unwrap_or_default();
                        tap = Some(Tap::Name(Naming::new(NameTarget::Face { photo: f.photo, index: f.index }, start)));
                    }
                }
            }
        }
        app.renderer.want_variants(shown);
    });
    act(app, ui.ctx(), tap);
}

/// What a tap in the People view or a person's page asks for.
enum Tap {
    None,
    Person(String),
    Name(Naming),
    /// A grouped person already named, with faces waiting: confirm them all.
    Confirm(String, String),
    /// Open this photo (from this person's page).
    Open(u64),
    /// It is them / it isn't.
    Yes(u64, usize),
    No(u64, usize),
    YesAll,
}

fn rect_of(v: &Value) -> Option<lightcraft_geom::Rect> {
    Some(lightcraft_geom::Rect { x0: v["x0"].as_f64()?, y0: v["y0"].as_f64()?, x1: v["x1"].as_f64()?, y1: v["y1"].as_f64()? })
}

fn act(app: &mut LightcraftApp, ctx: &egui::Context, tap: Option<Tap>) {
    let Some(tap) = tap else { return };
    let result = match tap {
        Tap::None => Ok(Value::Null),
        Tap::Person(name) => app.run("view.person", json!({"name": name})),
        Tap::Name(n) => {
            app.ui.face_naming = Some(n);
            Ok(Value::Null)
        }
        Tap::Confirm(id, name) => app.run("people.name", json!({"cluster": id, "name": name})),
        Tap::Open(photo) => {
            let name = app.ui.person_page.clone().unwrap_or_default();
            let r = app.run("library.select", json!({"ids": [photo], "active": photo}));
            let _ = app.run("view.detail", json!({}));
            // Back comes here again
            app.ui.person_from = Some((name.clone(), photo));
            app.ui.last_person = Some(name);
            app.ui.person_page = None;
            r
        }
        Tap::Yes(photo, index) => {
            let name = app.ui.person_page.clone().unwrap_or_default();
            crate::haptics::tap(ctx, crate::haptics::Haptic::Selection);
            app.run("faces.setName", json!({"id": photo, "index": index, "name": name}))
        }
        Tap::No(photo, index) => {
            app.ui.dismissed_faces.insert((photo, index));
            Ok(Value::Null)
        }
        Tap::YesAll => Ok(Value::Null),
    };
    if let Err(e) = result {
        app.toast(ctx, e);
    }
}

/// A person's page on a phone.
fn person(app: &mut LightcraftApp, ui: &mut egui::Ui, name: &str) {
    let t = Tokens::get(ui.ctx());
    let now = ui.input(|i| i.time);
    let page = super::person::page_for(app, name, now);
    let shown_name = if page.name.is_empty() { name.to_string() } else { page.name.clone() };
    let best =
        app.caches.people(&app.session.catalog, &lightcraft_catalog::Filter::default()).iter().find(|p| p.name.eq_ignore_ascii_case(name)).cloned();
    let more: Vec<super::person::Face> = page.more.iter().filter(|f| !app.ui.dismissed_faces.contains(&(f.photo, f.index))).cloned().collect();
    let mut tap: Option<Tap> = None;
    let mut rename = false;
    let mut photos = false;
    egui::ScrollArea::vertical().id_salt(("person-phone", name)).auto_shrink(false).show_viewport(ui, |ui, viewport| {
        let w = ui.available_width();
        let tile = ((w - 2.0 * PAD - 3.0 * 6.0) / 4.0).floor().max(30.0);
        let trow = tile + 6.0;
        let top_h = 236.0;
        let conf_rows = page.confirmed.len().div_ceil(4);
        let more_top = top_h + HEAD_H + conf_rows as f32 * trow + 12.0;
        let more_rows = more.len().div_ceil(4);
        let total = more_top + HEAD_H + 26.0 + more_rows as f32 * trow + 24.0;
        let (area, _) = ui.allocate_exact_size(vec2(w, total), Sense::hover());
        let o = area.min;
        // their face, their name (tap: rename), how many photos, and their photos in the grid
        let face = Rect::from_center_size(o + vec2(w / 2.0, 70.0), vec2(116.0, 116.0));
        match &best {
            Some(p) => {
                let view = app.session.face_view(p.photo, p.face);
                disc(app, ui, p.photo, view, face);
            }
            None => {
                if let Some(f) = page.confirmed.first() {
                    disc(app, ui, PhotoId(f.photo), f.rect, face);
                }
            }
        }
        let g = ui.painter().layout_no_wrap(fit(ui, &shown_name, t.semibold(26.0), w - 2.0 * PAD), t.semibold(26.0), t.text);
        let name_r = Rect::from_center_size(o + vec2(w / 2.0, 150.0), g.size() + vec2(16.0, 8.0));
        let resp = ui.interact(name_r, egui::Id::new("person-phone-name"), Sense::click());
        register(ui.ctx(), "person:rename", name_r);
        ui.painter().galley(name_r.center() - g.size() / 2.0, g, t.text);
        if resp.clicked() {
            rename = true;
        }
        let n = best.as_ref().map_or(page.total, |b| b.count);
        let count = photo_count(n as u64);
        ui.painter().text(o + vec2(w / 2.0, 178.0), Align2::CENTER_CENTER, count, t.font(14.0), t.text_dim);
        let button = Rect::from_center_size(o + vec2(w / 2.0, 212.0), vec2(170.0, 36.0));
        let b = ui.interact(button, egui::Id::new("person-phone-photos"), Sense::click());
        register(ui.ctx(), "person:photos", button);
        let k = crate::glass::press(ui.ctx(), &b);
        crate::glass::paint(ui.painter(), button.expand(2.0 * k), 18.0, Some(t.accent), k);
        ui.painter().text(button.center(), Align2::CENTER_CENTER, crate::i18n::tr("Show Photos"), t.semibold(15.0), Color32::WHITE);
        if b.clicked() {
            photos = true;
        }
        // their faces, a photo each
        let tile_at = |k: usize, top: f32| {
            let (r, c) = (k / 4, k % 4);
            Rect::from_min_size(o + vec2(PAD + c as f32 * (tile + 6.0), top + HEAD_H + r as f32 * trow), vec2(tile, tile))
        };
        heading(ui, o + vec2(PAD, top_h + HEAD_H / 2.0 + 6.0), "Photos");
        let visible = |top: f32, rows: usize| {
            let first = ((viewport.top() - top) / trow).floor().max(0.0) as usize;
            let last = (((viewport.bottom() - top) / trow).ceil().max(0.0) as usize).min(rows);
            first * 4..last * 4
        };
        let mut shown = 0;
        for k in visible(top_h + HEAD_H, conf_rows) {
            let Some(f) = page.confirmed.get(k) else { break };
            shown += 1;
            let r = tile_at(k, top_h);
            square_face(app, ui, f, r);
            let resp = ui.interact(r, egui::Id::new(("person-phone-face", f.photo, f.index)), Sense::click());
            register(ui.ctx(), format!("person-face:{}:{}", f.photo, f.index), r);
            if resp.clicked() {
                tap = Some(Tap::Open(f.photo));
            }
        }
        // the faces that look like them: ✓ it is them, ✕ it isn't
        let head = format!("{} {}?", crate::i18n::tr("Is this"), shown_name);
        ui.painter().text(o + vec2(PAD, more_top + HEAD_H / 2.0 + 6.0), Align2::LEFT_CENTER, head, t.semibold(17.0), t.text);
        let note = if more.is_empty() {
            if app.caches.faces_active || page.ready {
                crate::i18n::tr("No other faces look like them yet: more appear as your photos are looked at.")
            } else {
                crate::i18n::tr("Turn on finding people (People, at the top) to find more of their photos.")
            }
        } else {
            crate::i18n::tr("Tap ✓ when it is them, ✕ when it isn't.")
        };
        // (it wraps: the screen is narrow)
        let g = ui.painter().layout(note.to_string(), t.font(13.0), t.text_dim, w - 2.0 * PAD);
        ui.painter().galley(o + vec2(PAD, more_top + HEAD_H - 4.0), g, t.text_dim);
        if more.len() > 1 {
            let all = Rect::from_min_size(o + vec2(w - PAD - 110.0, more_top + 8.0), vec2(110.0, 30.0));
            let b = ui.interact(all, egui::Id::new("person-phone-confirm-all"), Sense::click());
            register(ui.ctx(), "person:confirmAll", all);
            crate::glass::paint(ui.painter(), all, 15.0, None, crate::glass::press(ui.ctx(), &b));
            ui.painter().text(all.center(), Align2::CENTER_CENTER, crate::i18n::tr("Confirm All"), t.semibold(14.0), t.accent);
            if b.clicked() {
                tap = Some(Tap::YesAll);
            }
        }
        let grid_top = more_top + 26.0;
        for k in visible(grid_top + HEAD_H, more_rows) {
            let Some(f) = more.get(k) else { break };
            shown += 1;
            let r = tile_at(k, grid_top);
            square_face(app, ui, f, r);
            // ✓ and ✕ along the bottom, big enough for a finger
            let yes = Rect::from_min_max(pos2(r.center().x, r.bottom() - 30.0), r.right_bottom());
            let no = Rect::from_min_max(pos2(r.left(), r.bottom() - 30.0), pos2(r.center().x, r.bottom()));
            let p = ui.painter();
            p.add(egui::epaint::RectShape::filled(
                Rect::from_min_max(no.min, yes.max),
                egui::CornerRadius { nw: 0, ne: 0, sw: 10, se: 10 },
                Color32::from_black_alpha(150),
            ));
            paint(p, Rect::from_center_size(no.center(), vec2(14.0, 14.0)), Icon::Close, Color32::WHITE);
            paint(p, Rect::from_center_size(yes.center(), vec2(16.0, 16.0)), Icon::Check, Color32::WHITE);
            let n = ui.interact(no, egui::Id::new(("more-no", f.photo, f.index)), Sense::click());
            let y = ui.interact(yes, egui::Id::new(("more-yes", f.photo, f.index)), Sense::click());
            register(ui.ctx(), format!("more-no:{}:{}", f.photo, f.index), no);
            register(ui.ctx(), format!("more-yes:{}:{}", f.photo, f.index), yes);
            if y.clicked() {
                tap = Some(Tap::Yes(f.photo, f.index));
            } else if n.clicked() {
                tap = Some(Tap::No(f.photo, f.index));
            }
        }
        app.renderer.want_variants(shown);
    });
    if rename {
        app.ui.face_naming = Some(Naming::new(NameTarget::Person { from: shown_name.clone() }, shown_name.clone()));
    }
    if photos {
        let _ = app.run("library.filter", json!({"person": shown_name}));
        let _ = app.run("view.gridSpan", json!({"span": "all"}));
    }
    if matches!(tap, Some(Tap::YesAll)) {
        let faces: Vec<Value> = more.iter().map(|f| json!({"photo": f.photo, "index": f.index})).collect();
        if let Err(e) = app.run("faces.nameFaces", json!({"faces": faces, "name": shown_name})) {
            app.toast(ui.ctx(), e);
        }
        return;
    }
    act(app, ui.ctx(), tap);
}

/// A face as a rounded square tile.
fn square_face(app: &mut LightcraftApp, ui: &egui::Ui, f: &super::person::Face, r: Rect) {
    let t = Tokens::get(ui.ctx());
    let px = (r.width() * ui.ctx().pixels_per_point()).ceil() as usize;
    match app.session.face_job(PhotoId(f.photo), f.rect, px).and_then(|job| app.renderer.variant(job).map(|tex| tex.tex.id())) {
        Some(tex) => {
            ui.painter()
                .add(egui::epaint::RectShape::filled(r, 10, Color32::WHITE).with_texture(tex, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))));
        }
        None => {
            ui.painter().rect_filled(r, 10.0, if Tokens::is_dark(ui.ctx()) { Color32::from_gray(44) } else { t.inset });
        }
    }
}

/// The faces named `from` across the library (case-insensitive): what renaming a person renames.
pub fn faces_named(cat: &lightcraft_catalog::Catalog, from: &str) -> Vec<(u64, usize)> {
    let from = from.trim();
    let mut out = Vec::new();
    for p in cat.photos().filter(|p| p.in_library()) {
        for (i, r) in p.meta.regions.iter().enumerate() {
            if r.kind == lightcraft_meta::RegionKind::Face && r.name.as_deref().is_some_and(|n| n.trim().eq_ignore_ascii_case(from)) {
                out.push((p.id.0, i));
            }
        }
    }
    out
}

/// Give `target` the name `name` (one undo step).
pub fn apply(app: &mut LightcraftApp, target: &NameTarget, name: &str) -> Result<Value, String> {
    match target {
        NameTarget::Face { photo, index } => app.run("faces.setName", json!({"id": photo, "index": index, "name": name})),
        NameTarget::Cluster { id, .. } => app.run("people.name", json!({"cluster": id, "name": name})),
        NameTarget::Person { from } => {
            let faces: Vec<Value> =
                faces_named(&app.session.catalog, from).into_iter().map(|(photo, index)| json!({"photo": photo, "index": index})).collect();
            if faces.is_empty() {
                return Err(crate::i18n::tr("Nobody has that name any more").to_string());
            }
            let r = app.run("faces.nameFaces", json!({"faces": faces, "name": name}))?;
            // the page follows the person to their new name
            if app.ui.person_page.as_deref().is_some_and(|p| p.eq_ignore_ascii_case(from)) {
                app.ui.person_page = Some(name.to_string());
            }
            Ok(r)
        }
        NameTarget::New { photo, rect } => {
            app.run("photo.addRegion", json!({"id": photo, "rect": {"x0": rect.x0, "y0": rect.y0, "x1": rect.x1, "y1": rect.y1}, "name": name}))
        }
    }
}

/// The page every name is typed on (widget `sheet:faceName`, field `field:faceNamePhone`): the face,
/// the name, the names it could be (the people it looks like first, then those that start with what
/// is typed), and for a face on a photo, Remove Face. Call every frame.
pub fn naming_page(app: &mut LightcraftApp, ctx: &egui::Context) {
    let open = app.ui.face_naming.is_some();
    let keep = egui::Id::new("face-naming-shown");
    let Some(mut n) = app.ui.face_naming.clone().or_else(|| ctx.data(|d| d.get_temp::<Naming>(keep))) else {
        mobile::hidden(ctx, "faceName");
        return;
    };
    if open {
        ctx.data_mut(|d| d.insert_temp(keep, n.clone()));
    }
    let t = Tokens::get(ctx);
    let title = match n.target {
        NameTarget::Person { .. } => "Rename",
        _ => "Who Is This?",
    };
    // the face to show: (photo, box)
    let face: Option<(u64, lightcraft_geom::Rect)> = match &n.target {
        NameTarget::Face { photo, index } => {
            app.session.catalog.photo(PhotoId(*photo)).and_then(|p| p.meta.regions.get(*index).map(|r| (*photo, r.rect)))
        }
        NameTarget::Cluster { photo, rect, .. } | NameTarget::New { photo, rect } => Some((*photo, *rect)),
        NameTarget::Person { from } => app
            .caches
            .people(&app.session.catalog, &lightcraft_catalog::Filter::default())
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(from))
            .map(|p| (p.photo.0, app.session.face_view(p.photo, p.face))),
    };
    // what it could be: the people it looks like (a face on a photo), then names starting with the text
    let mut offered: Vec<String> = Vec::new();
    if let NameTarget::Face { photo, index } = &n.target
        && let Some(h) = super::faces::hints_for(app, *photo)
        && let Some(hint) = h.by_index.get(index)
    {
        offered.extend(hint.candidates.iter().take(3).map(|(name, _)| name.clone()));
    }
    let typed = n.text.trim().to_lowercase();
    for name in app.caches.person_names(&app.session.catalog).iter() {
        if offered.len() >= 8 {
            break;
        }
        if (typed.is_empty() || name.to_lowercase().starts_with(&typed)) && !offered.iter().any(|o| o.eq_ignore_ascii_case(name)) {
            offered.push(name.clone());
        }
    }
    let can = !n.text.trim().is_empty();
    let mut chosen: Option<String> = None;
    let mut remove = false;
    let bar = mobile::page(ctx, "faceName", title, "Cancel", Some(("Done", can)), open, |ui| {
        if let Some((photo, rect)) = face {
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 128.0), Sense::hover());
            disc(app, ui, PhotoId(photo), rect, Rect::from_center_size(r.center(), vec2(112.0, 112.0)));
        }
        let field = crate::text_field::TextField::singleline("field:faceNamePhone", &mut n.text)
            .hint(crate::i18n::tr("Name"))
            .font(t.font(17.0))
            .width(ui.available_width())
            .margin(egui::Margin::symmetric(12, 10))
            .show(ui);
        if n.fresh {
            ui.memory_mut(|m| m.request_focus(crate::text_field::id("field:faceNamePhone")));
            n.fresh = false;
        }
        if field.ending == Some(crate::text_field::Ending::Return) && !n.text.trim().is_empty() {
            chosen = Some(n.text.trim().to_string());
        }
        for (i, name) in offered.iter().enumerate() {
            if mobile::row(ui, &format!("faceNamePick-{i}"), Some(Icon::Person), name, true) {
                chosen = Some(name.clone());
            }
        }
        if let NameTarget::Face { .. } = n.target {
            mobile::row_gap(ui);
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::click());
            register(ui.ctx(), "button:removeFace", r);
            ui.painter().text(
                pos2(r.left() + 16.0, r.center().y),
                Align2::LEFT_CENTER,
                crate::i18n::tr("Not a Face — Remove"),
                t.font(17.0),
                t.reject,
            );
            if resp.clicked() {
                remove = true;
            }
        }
    });
    if open && app.ui.face_naming.is_some() {
        app.ui.face_naming = Some(n.clone());
    }
    if bar.cancel {
        app.ui.face_naming = None;
        return;
    }
    if remove && let NameTarget::Face { photo, index } = n.target {
        app.ui.face_naming = None;
        if let Err(e) = app.run("photo.removeRegion", json!({"id": photo, "index": index})) {
            app.toast(ctx, e);
        }
        return;
    }
    let name = chosen.or_else(|| (bar.ok && can).then(|| n.text.trim().to_string()));
    if let Some(name) = name {
        app.ui.face_naming = None;
        match apply(app, &n.target, &name) {
            Ok(_) => crate::haptics::tap(ctx, crate::haptics::Haptic::Light),
            Err(e) => app.toast(ctx, e),
        }
    }
}

/// The faces on a photo in the phone's editor (with face boxes on, `view.faceBoxes`): a soft frame
/// round each with its name (or the person it looks like, with "?") under it; a tap names it. In
/// Tag a Face mode (`view.tagFace`) a tap on the photo marks a face there and names it.
pub fn loupe_faces(
    app: &mut LightcraftApp,
    ui: &egui::Ui,
    map_screen: &dyn Fn(f64, f64) -> egui::Pos2,
    map_norm: &dyn Fn(egui::Pos2) -> (f64, f64),
    img: Rect,
    photo: &lightcraft_catalog::Photo,
    orient: lightcraft_geom::Orientation,
) {
    let t = Tokens::get(ui.ctx());
    let hints = super::faces::hints_for(app, photo.id.0);
    let p = ui.painter();
    for (index, r) in photo.meta.regions.iter().enumerate().filter(|(_, r)| r.kind == lightcraft_meta::RegionKind::Face) {
        let norm = orient.map_norm_rect(r.rect);
        let rect = Rect::from_two_pos(map_screen(norm.x0, norm.y0), map_screen(norm.x1, norm.y1));
        p.rect_stroke(rect.expand(1.0), 8.0, Stroke::new(1.0, Color32::from_black_alpha(110)), egui::StrokeKind::Outside);
        p.rect_stroke(rect, 8.0, Stroke::new(1.5, Color32::from_white_alpha(220)), egui::StrokeKind::Outside);
        let guess = hints.as_ref().and_then(|h| h.by_index.get(&index)).and_then(|h| h.suggestion.as_ref()).map(|(n, _)| n.clone());
        let (label, ink) = match (&r.name, &guess) {
            (Some(n), _) => (n.clone(), Color32::WHITE),
            (None, Some(g)) => (format!("{g}?"), Color32::from_gray(210)),
            (None, None) => (crate::i18n::tr("Name?").to_string(), Color32::from_gray(190)),
        };
        let g = p.layout_no_wrap(label, t.semibold(13.0), ink);
        let pill = Rect::from_center_size(pos2(rect.center().x, rect.bottom() + 8.0 + g.size().y / 2.0 + 4.0), g.size() + vec2(16.0, 8.0));
        p.rect_filled(pill, pill.height() / 2.0, Color32::from_black_alpha(160));
        p.galley(pill.center() - g.size() / 2.0, g, ink);
        let hit = rect.union(pill).expand(6.0);
        let resp = ui.interact(hit, egui::Id::new(("phone-face", photo.id.0, index)), Sense::click());
        register(ui.ctx(), format!("face:{index}"), rect);
        if resp.clicked() {
            let start = r.name.clone().or(guess).unwrap_or_default();
            app.ui.face_naming = Some(Naming::new(NameTarget::Face { photo: photo.id.0, index }, start));
            crate::haptics::tap(ui.ctx(), crate::haptics::Haptic::Light);
        }
    }
    if app.ui.tag_face {
        // a hint over the photo; a tap on it marks a face there
        let hint = crate::i18n::tr("Tap a face to mark it");
        let g = p.layout_no_wrap(hint.to_string(), t.semibold(14.0), Color32::WHITE);
        let pill = Rect::from_center_size(pos2(img.center().x, img.top().max(ui.clip_rect().top()) + 26.0), g.size() + vec2(28.0, 14.0));
        p.rect_filled(pill, pill.height() / 2.0, Color32::from_black_alpha(170));
        p.galley(pill.center() - g.size() / 2.0, g, Color32::WHITE);
        register(ui.ctx(), "hint:tagFace", pill);
        let resp = ui.interact(img, egui::Id::new("tag-face"), Sense::click());
        if resp.clicked()
            && let Some(at) = resp.interact_pointer_pos()
        {
            let half = 0.09 * img.width().min(img.height());
            let (a, b) = (map_norm(at - vec2(half, half)), map_norm(at + vec2(half, half)));
            let shown = lightcraft_geom::Rect {
                x0: a.0.min(b.0).clamp(0.0, 1.0),
                y0: a.1.min(b.1).clamp(0.0, 1.0),
                x1: a.0.max(b.0).clamp(0.0, 1.0),
                y1: a.1.max(b.1).clamp(0.0, 1.0),
            };
            let rect = orient.inverse().map_norm_rect(shown);
            app.ui.tag_face = false;
            app.ui.face_naming = Some(Naming::new(NameTarget::New { photo: photo.id.0, rect }, ""));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renaming_finds_every_face_of_that_name_in_any_case() {
        let mut s = lightcraft_engine::Session::with_demo();
        let id = s.catalog.photos().next().unwrap().id;
        for name in ["Ana", "ana ", "Bo"] {
            s.execute("photo.addRegion", &json!({"id": id.0, "rect": {"x0": 0.1, "y0": 0.1, "x1": 0.3, "y1": 0.3}, "name": name})).unwrap();
        }
        let found = faces_named(&s.catalog, "ANA");
        assert_eq!(found.len(), 2);
        assert!(faces_named(&s.catalog, "Cy").is_empty());
    }
}
