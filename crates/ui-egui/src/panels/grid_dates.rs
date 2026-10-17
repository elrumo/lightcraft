//! The phone grid's Months and Years, beside All Photos (`view.gridSpan`), as iOS's Photos offers
//! them for getting about a large library fast: Months shows each month as a heading over a few of
//! its photos (the best one large, the rest spread through the month, each with its day), Years a
//! card a year with its best photo. A tap on a month's photo opens All Photos at that photo, on a
//! month's heading at the month's first photo, on a year at its months. The switch between them is
//! a capsule at the bottom of the grid (`compact::span_bar`).

use std::sync::Arc;

use egui::{Align2, Color32, Rect, Sense, pos2, vec2};
use lightcraft_catalog::PhotoId;
use serde_json::json;

use crate::LightcraftApp;
use crate::state::{GridJump, GridSpan};
use crate::theme::Tokens;
use crate::widgets::register;

/// Space at the sides, between tiles, and under a month or year (points).
const PAD: f32 = 12.0;
const GAP: f32 = 4.0;
const AFTER: f32 = 22.0;
/// A month's heading.
const HEAD_H: f32 = 52.0;
/// A tile's corners.
const ROUND: u8 = 14;
/// Room under the last block for the Years / Months / All capsule.
const BOTTOM_ROOM: f32 = 84.0;

/// Room under the end of a phone's grid in `ui`: for the capsule, and for the home indicator when the
/// grid runs on under it (`compact::content`).
pub fn bottom_room(ui: &egui::Ui) -> f32 {
    BOTTOM_ROOM + (ui.max_rect().bottom() - ui.ctx().content_rect().bottom()).max(0.0)
}

/// One month or year: its key (`2025-08`, `2025`; empty for photos with no date), how many photos
/// it has, and the ones it shows (indices into the grid's photos), the best first.
#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub key: String,
    pub count: usize,
    pub shown: Vec<usize>,
}

/// The photos in `ids` grouped by month (`key_len` 7) or year (4) ([`lightcraft_catalog::Catalog::date_blocks`],
/// shared with the SwiftUI app's `library.blocks`).
pub fn blocks(app: &LightcraftApp, ids: &[PhotoId], key_len: usize, show: usize) -> Vec<Block> {
    app.session
        .catalog
        .date_blocks(ids, app.session.visible_stacked(), key_len, show)
        .into_iter()
        .map(|b| Block { key: b.key, count: b.count, shown: b.shown })
        .collect()
}

/// The blocks for this view and span, kept while neither changes.
fn cached(app: &LightcraftApp, ctx: &egui::Context, ids: &[PhotoId], generation: u64, span: GridSpan) -> Arc<Vec<Block>> {
    let key = egui::Id::new("grid-date-blocks");
    if let Some((g, s, b)) = ctx.data(|d| d.get_temp::<(u64, GridSpan, Arc<Vec<Block>>)>(key))
        && g == generation
        && s == span
    {
        return b;
    }
    let b = Arc::new(match span {
        GridSpan::Years => blocks(app, ids, 4, 1),
        _ => blocks(app, ids, 7, 6),
    });
    ctx.data_mut(|d| d.insert_temp(key, (generation, span, b.clone())));
    b
}

/// How tall a block is at width `w` (the tiles' width, inside the side padding).
fn block_h(span: GridSpan, b: &Block, w: f32) -> f32 {
    let small = (w - 2.0 * GAP) / 3.0;
    match span {
        GridSpan::Years => (w * 0.66).round() + AFTER,
        _ => {
            let tiles = match b.shown.len() {
                0 => 0.0,
                1 => (w * 0.6).round(),
                2 => (w - GAP) / 2.0,
                3..=5 => 2.0 * small + GAP,
                _ => 3.0 * small + 2.0 * GAP,
            };
            HEAD_H + tiles + AFTER
        }
    }
}

/// The tiles of a month block (`flip`: the large one on the right), relative to `at`, `w` wide.
fn month_tiles(n: usize, w: f32, at: egui::Pos2, flip: bool) -> Vec<Rect> {
    let small = (w - 2.0 * GAP) / 3.0;
    let big = 2.0 * small + GAP;
    let r = |x: f32, y: f32, ww: f32, hh: f32| Rect::from_min_size(at + vec2(x, y), vec2(ww, hh));
    match n {
        0 => vec![],
        1 => vec![r(0.0, 0.0, w, (w * 0.6).round())],
        2 => {
            let half = (w - GAP) / 2.0;
            vec![r(0.0, 0.0, half, half), r(half + GAP, 0.0, half, half)]
        }
        _ => {
            let (bx, sx) = if flip { (small + GAP, 0.0) } else { (0.0, big + GAP) };
            let mut v = vec![r(bx, 0.0, big, big), r(sx, 0.0, small, small), r(sx, small + GAP, small, small)];
            if n >= 6 {
                for k in 0..3 {
                    v.push(r(k as f32 * (small + GAP), big + GAP, small, small));
                }
            }
            v
        }
    }
}

/// The part of a `tw` × `th` texture that fills a rect of aspect `aspect` (width / height).
fn cover(tw: f32, th: f32, aspect: f32) -> Rect {
    let (tw, th, a) = (tw.max(1.0), th.max(1.0), if aspect.is_finite() && aspect > 0.0 { aspect } else { 1.0 });
    let ta = tw / th;
    if ta > a {
        let k = a / ta;
        Rect::from_min_max(pos2((1.0 - k) / 2.0, 0.0), pos2((1.0 + k) / 2.0, 1.0))
    } else {
        let k = ta / a;
        Rect::from_min_max(pos2(0.0, (1.0 - k) / 2.0), pos2(1.0, (1.0 + k) / 2.0))
    }
}

/// A photo in a rounded tile (widget `dateTile:<id>`); true when tapped.
fn tile(app: &mut LightcraftApp, ui: &mut egui::Ui, id: PhotoId, r: Rect, onscreen: bool) -> bool {
    let t = Tokens::get(ui.ctx());
    let resp = ui.interact(r, egui::Id::new(("date-tile", id.0, r.min.x as i32)), Sense::click());
    register(ui.ctx(), format!("dateTile:{}", id.0), r);
    let ppp = ui.ctx().pixels_per_point();
    super::grid::request_thumb(
        app,
        id,
        super::grid::thumb_px(r.width().max(r.height()), ppp),
        if onscreen { 10 } else { super::grid::PREFETCH_THUMB_PRIORITY },
    );
    let blur = super::grid::obscured(app, id);
    let p = ui.painter();
    match app.renderer.thumb(id) {
        Some(tex) => {
            let [tw, th] = tex.size;
            let uv = cover(tw as f32, th as f32, r.width() / r.height().max(1.0));
            let k = ui.ctx().animate_bool_with_time(egui::Id::new(("thumb-in", id.0)), true, 0.2);
            if blur {
                super::grid::paint_obscured(p, tex.tex.id(), r, uv, k);
            } else {
                p.add(egui::epaint::RectShape::filled(r, ROUND, Color32::WHITE.gamma_multiply(k)).with_texture(tex.tex.id(), uv));
            }
        }
        None => {
            p.rect_filled(r, ROUND, if Tokens::is_dark(ui.ctx()) { Color32::from_gray(38) } else { t.inset });
        }
    }
    if resp.is_pointer_button_down_on() {
        p.rect_filled(r, ROUND, Color32::from_black_alpha(60));
    }
    resp.clicked()
}

/// The day of the month on a tile, as iOS's Photos marks them.
fn day_label(ui: &egui::Ui, captured: Option<&str>, r: Rect) {
    let Some(day) = captured.and_then(|c| c.get(8..10)).and_then(|d| d.parse::<u32>().ok()) else { return };
    let t = Tokens::get(ui.ctx());
    let at = r.min + vec2(12.0, 10.0);
    let p = ui.painter();
    p.text(at + vec2(0.0, 1.0), Align2::LEFT_TOP, day.to_string(), t.semibold(20.0), Color32::from_black_alpha(140));
    p.text(at, Align2::LEFT_TOP, day.to_string(), t.semibold(20.0), Color32::WHITE);
}

/// What a block's heading says: "August 2025", "2025", or "No Date".
fn title(key: &str) -> String {
    if key.is_empty() { crate::i18n::tr("No Date").to_string() } else { crate::i18n::date_group_label(key, false) }
}

/// The Months or Years grid in `ui` (the area under the grid's title row).
pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui, ids: &[PhotoId], generation: u64, span: GridSpan) {
    let t = Tokens::get(ui.ctx());
    let ctx = ui.ctx().clone();
    let blocks = cached(app, &ctx, ids, generation, span);
    let w = (ui.available_width() - 2.0 * PAD).max(60.0);
    let mut tops = Vec::with_capacity(blocks.len());
    let mut y = 8.0;
    for b in blocks.iter() {
        tops.push(y);
        y += block_h(span, b, w);
    }
    let total = y + bottom_room(ui);
    // a jump: to a year's first month
    let jump = match app.ui.grid_jump.take() {
        Some(GridJump::Date(d)) => blocks.iter().position(|b| b.key.starts_with(&d)).and_then(|i| tops.get(i).copied()),
        _ => None,
    };
    let mut area = egui::ScrollArea::vertical()
        .id_salt(("grid-dates", span))
        .scroll_source(egui::scroll_area::ScrollSource::ALL)
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
        .auto_shrink([false, false]);
    if let Some(o) = jump {
        area = area.vertical_scroll_offset((o - 4.0).max(0.0));
    }
    let mut go: Option<serde_json::Value> = None;
    area.show_viewport(ui, |ui, viewport| {
        let (all, _) = ui.allocate_exact_size(vec2(ui.available_width(), total), Sense::hover());
        let origin = all.min;
        if std::mem::take(&mut app.ui.scroll_top) {
            ui.scroll_to_rect(Rect::from_min_size(origin, vec2(1.0, 1.0)), Some(egui::Align::TOP));
        }
        let prefetch = viewport.expand2(vec2(0.0, viewport.height()));
        let first = tops.partition_point(|top| *top < prefetch.top()).saturating_sub(1);
        for (bi, b) in blocks.iter().enumerate().skip(first) {
            let Some(&top) = tops.get(bi) else { break };
            if top > prefetch.bottom() {
                break;
            }
            let at = origin + vec2(PAD, top);
            let onscreen = top < viewport.bottom() && top + block_h(span, b, w) > viewport.top();
            match span {
                GridSpan::Years => {
                    let Some(&i) = b.shown.first() else { continue };
                    let Some(&id) = ids.get(i) else { continue };
                    let r = Rect::from_min_size(at, vec2(w, (w * 0.66).round()));
                    if tile(app, ui, id, r, onscreen) {
                        go = Some(json!({"span": "months", "date": b.key}));
                    }
                    register(ui.ctx(), format!("year:{}", if b.key.is_empty() { "unknown" } else { &b.key }), r);
                    // the year over a shade at the card's top
                    let shade = Rect::from_min_size(r.min, vec2(r.width(), 76.0));
                    ui.painter().add(egui::epaint::RectShape::filled(
                        shade,
                        egui::CornerRadius { nw: ROUND, ne: ROUND, sw: 0, se: 0 },
                        Color32::from_black_alpha(70),
                    ));
                    ui.painter().text(r.min + vec2(16.0, 12.0), Align2::LEFT_TOP, title(&b.key), t.semibold(30.0), Color32::WHITE);
                    let n = crate::i18n::tr_format!("{n} photos", n = b.count);
                    ui.painter().text(r.min + vec2(17.0, 50.0), Align2::LEFT_TOP, n, t.font(14.0), Color32::from_white_alpha(220));
                }
                _ => {
                    let head = Rect::from_min_size(at, vec2(w, HEAD_H));
                    let resp = ui.interact(head, egui::Id::new(("month-head", &b.key)), Sense::click());
                    register(ui.ctx(), format!("month:{}", if b.key.is_empty() { "unknown" } else { &b.key }), head);
                    let ink = if resp.is_pointer_button_down_on() { t.text_dim } else { t.text };
                    let g = ui.painter().layout_no_wrap(title(&b.key), t.semibold(24.0), ink);
                    let gw = g.size().x;
                    ui.painter().galley(pos2(head.left(), head.center().y - g.size().y / 2.0), g, ink);
                    let n = crate::i18n::tr_format!("{n} photos", n = b.count);
                    ui.painter().text(pos2(head.left() + gw + 10.0, head.center().y + 2.0), Align2::LEFT_CENTER, n, t.font(14.0), t.text_dim);
                    if resp.clicked() {
                        go = Some(json!({"span": "all", "date": b.key}));
                    }
                    let tiles = month_tiles(b.shown.len().min(6), w, at + vec2(0.0, HEAD_H), bi % 2 == 1);
                    for (k, r) in tiles.into_iter().enumerate() {
                        let Some(&id) = b.shown.get(k).and_then(|i| ids.get(*i)) else { continue };
                        if tile(app, ui, id, r, onscreen) {
                            go = Some(json!({"span": "all", "photo": id.0}));
                        }
                        let captured = app.session.catalog.photo(id).and_then(|p| p.captured.clone());
                        day_label(ui, captured.as_deref(), r);
                    }
                }
            }
        }
        app.grid_scroll = Some(viewport.top());
    });
    if let Some(params) = go {
        if let Err(e) = app.run("view.gridSpan", params) {
            app.toast(&ctx, e);
        }
        crate::haptics::tap(&ctx, crate::haptics::Haptic::Light);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn month_tiles_fill_the_width_and_their_height() {
        let w = 366.0;
        for n in 1..=6 {
            let tiles = month_tiles(n, w, pos2(0.0, 0.0), n % 2 == 0);
            let shown = match n {
                1 | 2 => n,
                3..=5 => 3,
                _ => 6,
            };
            assert_eq!(tiles.len(), shown, "{n}");
            let b = Block { key: String::new(), count: n, shown: (0..n).collect() };
            let bottom = tiles.iter().map(|r| r.bottom()).fold(0.0, f32::max);
            assert!((bottom + HEAD_H + AFTER - block_h(GridSpan::Months, &b, w)).abs() < 0.5, "{n}");
            assert!(tiles.iter().all(|r| r.left() >= -0.01 && r.right() <= w + 0.01), "{n}");
        }
    }

    #[test]
    fn cover_keeps_the_middle_of_the_long_side() {
        let near = |a: Rect, b: Rect| (a.min - b.min).length() + (a.max - b.max).length() < 1e-5;
        assert!(near(cover(300.0, 200.0, 1.0), Rect::from_min_max(pos2(1.0 / 6.0, 0.0), pos2(5.0 / 6.0, 1.0))));
        assert!(near(cover(200.0, 200.0, 2.0), Rect::from_min_max(pos2(0.0, 0.25), pos2(1.0, 0.75))));
        assert!(near(cover(0.0, 0.0, f32::NAN), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))));
    }
}
