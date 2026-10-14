//! The phone's Filter page (the filter button over the grid), modelled on Lightroom's mobile app:
//! the rating (at least / exactly / at most, then the stars) and the flags at the top, each with how
//! many photos the grid shows, the colour labels, then a row for each kind of choice — Type, Camera,
//! People, Location, Keyword, Edited — that opens its list in the same page (‹ goes back). Every
//! control drives `library.filter`, so the grid behind follows as you choose; Clear resets the
//! filter (`library.clearFilter`). The words and the drawing are our own.

use std::sync::Arc;

use egui::{Align2, Rect, Sense, Stroke, pos2, vec2};
use lightcraft_catalog::{Catalog, ColorLabel, Flag, MediaKind, RatingOp};
use serde_json::{Value, json};

use super::mobile;
use super::settings::{card_end, card_row, line, tap_row};
use crate::LightcraftApp;
use crate::icons::{Icon, paint};
use crate::theme::Tokens;
use crate::widgets::register;

/// The lists the page's rows open: (id, icon, name).
pub const LISTS: [(&str, Icon, &str); 6] = [
    ("type", Icon::Photos, "Type"),
    ("camera", Icon::Camera, "Camera"),
    ("people", Icon::Person, "People"),
    ("location", Icon::Pin, "Location"),
    ("keyword", Icon::Tag, "Keyword"),
    ("edited", Icon::Pencil, "Edited"),
];

/// A list longer than this gets a field to narrow it, and shows at most [`LIST_MAX`] rows.
const SEARCH_FROM: usize = 12;
const LIST_MAX: usize = 300;
/// A row of the page.
const ROW: f32 = crate::TOUCH_ROW_H;

/// The values the lists offer, each with how many of the library's photos have it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Choices {
    pub cameras: Vec<(String, usize)>,
    pub people: Vec<(String, usize)>,
    pub places: Vec<(String, usize)>,
    pub keywords: Vec<(String, usize)>,
}

impl Choices {
    pub fn of(cat: &Catalog) -> Choices {
        Choices {
            cameras: cat.cameras(),
            people: cat.people().into_iter().map(|p| (p.name, p.count)).collect(),
            places: cat.places(),
            keywords: cat.keywords(),
        }
    }
}

/// Open the page at its list of choices (`view.filterSheet`).
pub fn open(app: &mut LightcraftApp) {
    app.ui.filter_sheet = Some(String::new());
}

fn set(app: &mut LightcraftApp, ctx: &egui::Context, patch: Value) {
    if let Err(e) = app.run("library.filter", patch) {
        app.toast(ctx, e);
    }
}

/// The page, while it is open (and while it slides away). Called every frame.
pub fn show(app: &mut LightcraftApp, ctx: &egui::Context) {
    // what the page shows: kept while it slides away
    let shown_id = egui::Id::new("filter-sheet-shown");
    let open = app.ui.filter_sheet.is_some();
    let at = match &app.ui.filter_sheet {
        Some(at) => {
            ctx.data_mut(|d| d.insert_temp(shown_id, at.clone()));
            at.clone()
        }
        None => ctx.data(|d| d.get_temp::<String>(shown_id)).unwrap_or_default(),
    };
    let title = LISTS.iter().find(|(id, _, _)| *id == at).map_or("Filter", |(_, _, name)| *name);
    let filtering = app.session.filter != lightcraft_catalog::Filter::default();
    let bar = mobile::PageBar {
        title,
        left: if at.is_empty() { mobile::Item::Text("Close", true) } else { mobile::Item::Text("‹", true) },
        right: if at.is_empty() && filtering { mobile::Item::Pill("Clear") } else { mobile::Item::None },
        left_id: "filterBack",
        right_id: "filterClear",
    };
    let t = Tokens::get(ctx);
    let out = mobile::page_with(ctx, "filter", bar, open, |ui| {
        // (the list fades in when the page turns to another)
        ui.multiply_opacity(mobile::enter(ui.ctx(), egui::Id::new("filter-sheet-in"), at.clone(), 0.18));
        if at.is_empty() {
            main(app, ui, &t);
        } else {
            list(app, ui, &t, &at);
        }
    });
    if out.cancel {
        app.ui.filter_sheet = if at.is_empty() { None } else { Some(String::new()) };
    }
    if out.ok {
        app.ui.search.clear();
        if let Err(e) = app.run("library.clearFilter", json!({})) {
            app.toast(ctx, e);
        }
    }
}

/// The page's first list: rating, flags and labels, then a row per kind of choice.
fn main(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let ctx = ui.ctx().clone();
    let f = app.session.filter.clone();
    let shown = app.session.visible().len().to_string();
    ui.spacing_mut().item_spacing.y = 0.0;
    // rating: the comparison, then the stars (a tap on the chosen one clears it)
    card_row(ui, t, 16.0);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::hover());
    register(ui.ctx(), "filter:ratingRow", r);
    let op = Rect::from_min_size(pos2(r.left() + 8.0, r.top()), vec2(44.0, ROW));
    let op_resp = ui.interact(op, egui::Id::new("filter-rating-op"), Sense::click());
    register(ui.ctx(), "filter:ratingOp", op);
    let (glyph, next, said) = match f.rating_op {
        RatingOp::AtLeast => ("≥", "exactly", "At least"),
        RatingOp::Exactly => ("=", "atMost", "Exactly"),
        RatingOp::AtMost => ("≤", "atLeast", "At most"),
    };
    op_resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, crate::i18n::tr(said)));
    let ink = if op_resp.is_pointer_button_down_on() { t.text_dim } else { t.text };
    ui.painter().text(op.center(), Align2::CENTER_CENTER, glyph, t.font(24.0), ink);
    if op_resp.clicked() {
        set(app, &ctx, json!({"ratingOp": next}));
    }
    for i in 1..=5u8 {
        let cell = Rect::from_min_size(pos2(op.right() + 4.0 + (i - 1) as f32 * 40.0, r.top()), vec2(40.0, ROW));
        let resp = ui.interact(cell, egui::Id::new(("filter-star", i)), Sense::click());
        register(ui.ctx(), format!("filter:star{i}"), cell);
        resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, f.rating == i, format!("{i} ★")));
        let on = i <= f.rating;
        let k = ui.ctx().animate_bool_with_time(egui::Id::new(("filter-star-on", i)), on, 0.15);
        let size = 24.0 * (1.0 + 0.15 * (k * std::f32::consts::PI).sin());
        let star = Rect::from_center_size(cell.center(), vec2(size, size));
        if on {
            paint(ui.painter(), star, Icon::StarFilled, t.accent);
        } else {
            paint(ui.painter(), star, Icon::Star, t.text_dim);
        }
        if resp.clicked() {
            set(app, &ctx, json!({"rating": if f.rating == i { 0 } else { i }}));
        }
    }
    count(ui, t, r, &shown);
    // flags, one at a time (a tap on the chosen one clears it)
    card_row(ui, t, 16.0);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::hover());
    register(ui.ctx(), "filter:flagRow", r);
    for (i, (id, icon, flag, said)) in [
        ("pick", Icon::FlagChecked, Flag::Pick, "Picked"),
        ("none", Icon::FlagPick, Flag::None, "Unflagged"),
        ("reject", Icon::FlagReject, Flag::Reject, "Rejected"),
    ]
    .into_iter()
    .enumerate()
    {
        let cell = Rect::from_min_size(pos2(r.left() + 8.0 + i as f32 * 56.0, r.top()), vec2(52.0, ROW));
        let resp = ui.interact(cell, egui::Id::new(("filter-flag", id)), Sense::click());
        register(ui.ctx(), format!("filter:{id}"), cell);
        let on = f.flag == Some(flag);
        resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, crate::i18n::tr(said)));
        let color = match flag {
            _ if !on => t.text_dim,
            Flag::Pick => t.pick,
            Flag::Reject => t.reject,
            Flag::None => t.accent,
        };
        if on {
            ui.painter().rect_filled(cell.shrink2(vec2(4.0, 8.0)), 10.0, t.tool_active);
        }
        paint(ui.painter(), Rect::from_center_size(cell.center(), vec2(26.0, 26.0)), icon, color);
        if resp.clicked() {
            set(app, &ctx, json!({"flag": if on { Value::Null } else { json!(id) }}));
        }
    }
    count(ui, t, r, &shown);
    // colour labels: any of those chosen
    card_row(ui, t, 16.0);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::hover());
    register(ui.ctx(), "filter:labelRow", r);
    let mut chosen: Vec<ColorLabel> = f.labels.clone();
    chosen.extend(f.label.filter(|l| !f.labels.contains(l)));
    for (i, l) in ColorLabel::ALL.into_iter().enumerate() {
        let cell = Rect::from_min_size(pos2(r.left() + 8.0 + i as f32 * 44.0, r.top()), vec2(44.0, ROW));
        let name = format!("{l:?}").to_lowercase();
        let resp = ui.interact(cell, egui::Id::new(("filter-label", i)), Sense::click());
        register(ui.ctx(), format!("filter:label-{name}"), cell);
        let on = chosen.contains(&l);
        let spoken = crate::i18n::color_label(&app.session.catalog, l);
        resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, &spoken));
        let c = cell.center();
        ui.painter().circle_filled(c, 9.0, crate::theme::label_color(l));
        if on {
            ui.painter().circle_stroke(c, 13.0, Stroke::new(2.0, t.text));
        }
        if resp.clicked() {
            let mut next = chosen.clone();
            if on {
                next.retain(|x| *x != l);
            } else {
                next.push(l);
            }
            next.sort();
            let names: Vec<String> = next.iter().map(|x| format!("{x:?}").to_lowercase()).collect();
            set(app, &ctx, json!({"label": Value::Null, "labels": names}));
        }
    }
    card_end(ui, t);
    ui.add_space(28.0);
    // a row for each kind of choice, with what is chosen
    for (id, icon, name) in LISTS {
        let value = match id {
            "type" => kind_name(&f).to_string(),
            "camera" => f.camera.clone().unwrap_or_default(),
            "people" => f.person.clone().unwrap_or_default(),
            "location" => f.place.clone().unwrap_or_default(),
            "keyword" => f.keyword.clone().unwrap_or_default(),
            _ => match f.edited {
                Some(true) => crate::i18n::tr("Edited").to_string(),
                Some(false) => crate::i18n::tr("Unedited").to_string(),
                None => String::new(),
            },
        };
        let value = if value.is_empty() { crate::i18n::tr("Any").to_string() } else { value };
        if nav_row(ui, t, &format!("filterList-{id}"), icon, crate::i18n::tr(name), &value) {
            app.ui.filter_sheet = Some(id.to_string());
        }
    }
    card_end(ui, t);
}

/// The number of photos shown, at the right end of a row.
fn count(ui: &egui::Ui, t: &Tokens, r: Rect, n: &str) {
    ui.painter().text(pos2(r.right() - 16.0, r.center().y), Align2::RIGHT_CENTER, n, t.font(17.0), t.text_dim);
}

/// A row that opens a list: its icon, its name, what is chosen and a chevron. True when tapped.
fn nav_row(ui: &mut egui::Ui, t: &Tokens, id: &str, icon: Icon, label: &str, value: &str) -> bool {
    let (r, resp) = tap_row(ui, t, id, label, ROW, 56.0, true);
    paint(ui.painter(), Rect::from_center_size(pos2(r.left() + 28.0, r.center().y), vec2(24.0, 24.0)), icon, t.text);
    let chevron = Rect::from_center_size(pos2(r.right() - 20.0, r.center().y), vec2(14.0, 14.0));
    paint(ui.painter(), chevron, Icon::ChevronRight, t.text_disabled);
    let value = line(ui, value, 17.0, t.text_dim, r.width() * 0.45);
    let value_x = chevron.left() - 6.0 - value.size().x;
    let label = line(ui, label, 17.0, t.text, value_x - 8.0 - (r.left() + 56.0));
    ui.painter().galley(pos2(r.left() + 56.0, r.center().y - label.size().y / 2.0), label, t.text);
    ui.painter().galley(pos2(value_x, r.center().y - value.size().y / 2.0), value, t.text_dim);
    resp.clicked()
}

/// What the Type list has chosen.
fn kind_name(f: &lightcraft_catalog::Filter) -> &'static str {
    match (f.merged.as_deref(), f.kind) {
        (Some("hdr"), _) => "HDR",
        (Some("panorama"), _) => "Panoramas",
        (Some("hdrPanorama"), _) => "HDR Panoramas",
        (Some(_), _) => "Merged",
        (None, Some(MediaKind::Image)) => "Photos",
        (None, Some(MediaKind::Raw)) => "Raw",
        (None, Some(MediaKind::Video)) => "Videos",
        (None, None) => "",
    }
}

/// A row of a list (`button:filterPick-<list>-<i>`): a check mark when chosen, the name, how many
/// photos have it. True when tapped.
fn pick_row(ui: &mut egui::Ui, t: &Tokens, id: &str, label: &str, n: Option<usize>, chosen: bool) -> bool {
    let (r, resp) = tap_row(ui, t, id, label, ROW, 16.0, true);
    if chosen {
        paint(ui.painter(), Rect::from_center_size(pos2(r.right() - 22.0, r.center().y), vec2(16.0, 16.0)), Icon::Check, t.accent);
    }
    let right = r.right() - 44.0;
    let count = n.map(|n| line(ui, &n.to_string(), 17.0, t.text_dim, 80.0));
    let room = right - count.as_ref().map_or(0.0, |g| g.size().x + 12.0) - (r.left() + 16.0);
    let name = line(ui, label, 17.0, t.text, room);
    ui.painter().galley(pos2(r.left() + 16.0, r.center().y - name.size().y / 2.0), name, t.text);
    if let Some(g) = count {
        ui.painter().galley(pos2(right - g.size().x, r.center().y - g.size().y / 2.0), g, t.text_dim);
    }
    resp.clicked()
}

/// One kind of choice: Any, then each value; a tap chooses it and goes back to the page's first list.
fn list(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens, at: &str) {
    let ctx = ui.ctx().clone();
    let f = app.session.filter.clone();
    ui.spacing_mut().item_spacing.y = 0.0;
    // (label, the filter's patch, photos, chosen)
    let mut rows: Vec<(String, Value, Option<usize>, bool)> =
        match at {
            "type" => [
                ("Any", json!({"kind": null, "merged": null})),
                ("Photos", json!({"kind": "image", "merged": null})),
                ("Raw", json!({"kind": "raw", "merged": null})),
                ("Videos", json!({"kind": "video", "merged": null})),
                ("HDR", json!({"kind": null, "merged": "hdr"})),
                ("Panoramas", json!({"kind": null, "merged": "panorama"})),
                ("HDR Panoramas", json!({"kind": null, "merged": "hdrPanorama"})),
            ]
            .into_iter()
            .map(|(label, patch)| {
                let chosen = kind_name(&f) == label || (label == "Any" && kind_name(&f).is_empty());
                (crate::i18n::tr(label).to_string(), patch, None, chosen)
            })
            .collect(),
            "edited" => [("Any", Value::Null, None), ("Edited", json!(true), Some(true)), ("Unedited", json!(false), Some(false))]
                .into_iter()
                .map(|(label, v, e)| (crate::i18n::tr(label).to_string(), json!({"edited": v}), None, f.edited == e))
                .collect(),
            _ => {
                let choices = app.caches.filter_choices(&app.session.catalog);
                let (key, current, values) = match at {
                    "camera" => ("camera", f.camera.clone(), &choices.cameras),
                    "people" => ("person", f.person.clone(), &choices.people),
                    "location" => ("place", f.place.clone(), &choices.places),
                    _ => ("keyword", f.keyword.clone(), &choices.keywords),
                };
                let mut rows = vec![(crate::i18n::tr("Any").to_string(), json!({key: null}), None, current.is_none())];
                // a long list can be narrowed by typing
                let query = if values.len() > SEARCH_FROM { search_field(ui, t, at) } else { String::new() };
                let query = query.trim().to_lowercase();
                rows.extend(
                    values.iter().filter(|(name, _)| query.is_empty() || name.to_lowercase().contains(&query)).take(LIST_MAX).map(|(name, n)| {
                        (name.clone(), json!({key: name}), Some(*n), current.as_deref().is_some_and(|c| c.eq_ignore_ascii_case(name)))
                    }),
                );
                if values.is_empty() {
                    let none = match at {
                        "camera" => "No camera is named in these photos",
                        "people" => "Nobody is named yet: name faces in People",
                        "location" => "No place is named in these photos",
                        _ => "No keywords yet",
                    };
                    rows.push((crate::i18n::tr(none).to_string(), Value::Null, None, false));
                }
                rows
            }
        };
    for (i, (label, patch, n, chosen)) in rows.drain(..).enumerate() {
        if patch.is_null() {
            // (a note, not a choice)
            card_end(ui, t);
            ui.add_space(12.0);
            ui.label(egui::RichText::new(label).size(15.0).color(t.text_dim));
            continue;
        }
        if pick_row(ui, t, &format!("filterPick-{at}-{i}"), &label, n, chosen) {
            set(app, &ctx, patch);
            app.ui.filter_sheet = Some(String::new());
        }
    }
    card_end(ui, t);
}

/// The field that narrows a long list (what was typed is kept for each list).
fn search_field(ui: &mut egui::Ui, t: &Tokens, at: &str) -> String {
    let id = egui::Id::new(("filter-sheet-query", at));
    let mut q: String = ui.data(|d| d.get_temp(id)).unwrap_or_default();
    crate::text_field::TextField::singleline("field:filterSearch", &mut q)
        .hint(crate::i18n::tr("Search"))
        .font(t.font(17.0))
        .width(ui.available_width())
        .margin(egui::Margin::symmetric(12, 10))
        .show(ui);
    ui.add_space(12.0);
    ui.data_mut(|d| d.insert_temp(id, q.clone()));
    q
}

/// The values for the lists, by catalog revision.
pub(crate) fn choices_for(cache: &mut Option<(u64, Arc<Choices>)>, cat: &Catalog) -> Arc<Choices> {
    match cache {
        Some((rev, c)) if *rev == cat.revision => c.clone(),
        _ => {
            let c = Arc::new(Choices::of(cat));
            *cache = Some((cat.revision, c.clone()));
            c
        }
    }
}
