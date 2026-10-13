//! Photo Grid (justified rows) and Square Grid. Virtualized: only visible cells request thumbnails.

#[cfg(test)]
#[path = "grid_header_tests.rs"]
mod header_tests;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use egui::{Align2, Color32, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use lightcraft_catalog::{Catalog, ColorLabel, DateRun, Flag, GroupBy, PhotoId, SortKey, StackId};
use serde_json::json;

use crate::LightcraftApp;
use crate::haptics::{self, Haptic};
use crate::icons::{Icon, paint};
use crate::render::Slot;
use crate::state::ViewMode;
use crate::theme::Tokens;
use crate::widgets::register;

/// Thumbnail render size (pixels, long edge) for a cell of `pts` points.
pub(super) fn thumb_px(pts: f32, ppp: f32) -> usize {
    let px = (pts * ppp).ceil() as usize;
    if px <= 256 {
        256
    } else if px <= 384 {
        384
    } else {
        512
    }
}

/// Counters of the grid's work, cumulative over frames (benchmarks and tests: an unchanged frame
/// must not rebuild the view, its layout or its indexes, and visits only the cells near the screen).
#[derive(Clone, Debug, Default)]
pub struct GridStats {
    /// Frames the grid was drawn.
    pub frames: u64,
    /// Cells looked at (tested against the viewport or drawn).
    pub cells_visited: u64,
    /// Layouts computed.
    pub layout_builds: u64,
    /// Per-view data (date runs, row index) built.
    pub view_builds: u64,
    /// Stack indexes built.
    pub stack_index_builds: u64,
    /// Wall-clock time of the last [`show`].
    pub last_show: web_time::Duration,
}

pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t0 = web_time::Instant::now();
    show_inner(app, ui);
    app.caches.grid_stats.frames += 1;
    app.caches.grid_stats.last_show = t0.elapsed();
}

/// Photo → (stack, position in the stack).
type StackIndex = HashMap<PhotoId, (StackId, usize)>;

/// What the date runs depend on: the visible list's generation, the date the grid groups by
/// (`None`: no headers) and the grouping.
type RunsKey = (u64, Option<SortKey>, GroupBy);

/// What the grid derives from the visible photos, kept across frames. Everything is keyed by the
/// session's visible-list generation ([`lightcraft_engine::Session::visible_shared`]), which
/// changes whenever the list is recomputed (any catalog change: crops, orientation, dates, stacks;
/// or another source, filter or sort) — so an unchanged frame neither copies nor hashes the ids.
#[derive(Default)]
pub struct GridCache {
    runs: Option<(RunsKey, Arc<Vec<DateRun>>)>,
    /// Layout by (runs key, width, thumbnail size, square).
    layout: Option<((RunsKey, u32, u32, bool), Arc<GridLayout>)>,
    /// Stack index by generation.
    stacks: Option<(u64, Arc<StackIndex>)>,
    /// How many of the photos are browsed (Local) ones, by generation.
    local: Option<(u64, usize)>,
}

impl GridCache {
    fn runs(&mut self, stats: &mut GridStats, cat: &Catalog, ids: &[PhotoId], key: RunsKey) -> Arc<Vec<DateRun>> {
        match &self.runs {
            Some((k, r)) if *k == key => r.clone(),
            _ => {
                stats.view_builds += 1;
                let r = Arc::new(key.1.map(|sort| cat.date_runs(ids, sort, key.2)).unwrap_or_default());
                self.runs = Some((key, r.clone()));
                r
            }
        }
    }

    fn stacks(&mut self, stats: &mut GridStats, cat: &Catalog, generation: u64) -> Arc<StackIndex> {
        match &self.stacks {
            Some((g, s)) if *g == generation => s.clone(),
            _ => {
                stats.stack_index_builds += 1;
                let s = Arc::new(cat.stack_index());
                self.stacks = Some((generation, s.clone()));
                s
            }
        }
    }

    fn local(&mut self, cat: &Catalog, ids: &[PhotoId], generation: u64) -> usize {
        match self.local {
            Some((g, n)) if g == generation => n,
            _ => {
                let n = ids.iter().filter(|id| cat.photo(**id).is_some_and(|p| p.local)).count();
                self.local = Some((generation, n));
                n
            }
        }
    }
}

/// Width / height of each photo's cell: its cropped, oriented shape.
pub(crate) fn aspects(cat: &Catalog, ids: &[PhotoId]) -> Vec<f32> {
    ids.iter()
        .map(|id| {
            let p = cat.photo(*id);
            let (w, h) = p.map(|p| (p.width.max(1) as f32, p.height.max(1) as f32)).unwrap_or((3.0, 2.0));
            let swap = p.is_some_and(|p| p.develop.orientation.swaps_axes());
            let crop = p.map(|p| p.develop.crop.geometry.rect).unwrap_or(lightcraft_geom::Rect::UNIT);
            let (w, h) = if swap { (h, w) } else { (w, h) };
            (w * crop.width() as f32) / (h * crop.height() as f32).max(1e-3)
        })
        .collect()
}

/// The rows (of a layout's `rows`, top to bottom) that meet the y range `top..=bottom`, found by
/// binary search.
pub fn rows_between(rows: &[GridRow], top: f32, bottom: f32) -> &[GridRow] {
    let first = rows.partition_point(|r| r.bottom < top);
    let rest = rows.get(first..).unwrap_or(&[]);
    let n = rest.partition_point(|r| r.top <= bottom);
    rest.get(..n).unwrap_or(&[])
}

fn show_inner(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    if matches!(app.ui.view, ViewMode::PhotoGrid | ViewMode::SquareGrid) {
        app.ui.grid_mode = app.ui.view;
    }
    let (generation, ids) = app.session.visible_shared();
    // header: source title + count
    let sel_n = app.session.selection.ids.len();
    let chips = lightcraft_engine::filter_chips(&app.session.filter, &app.session.catalog);
    let total = app.session.source_total();
    let counted = super::chips::count_text(ids.len(), total, !chips.is_empty());
    let cnt = if sel_n > 1 { crate::i18n::tr_format!("{sel_n} selected · {counted}", counted = counted, sel_n = sel_n) } else { counted };
    match app.session.browse.clone().filter(|_| app.session.source == lightcraft_engine::LibrarySource::Folder) {
        Some(b) => {
            let local = app.caches.grid.local(&app.session.catalog, &ids, generation);
            folder_header(app, ui, &b, &ids, local, &cnt);
        }
        None if app.compact => {
            let (hr, _) = ui.allocate_exact_size(vec2(ui.available_width(), 56.0), Sense::hover());
            ui.painter().rect_filled(hr, 0.0, t.canvas);
            super::compact::grid_header(app, ui, hr, &cnt);
        }
        None => {
            let (hr, _) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::hover());
            ui.painter().rect_filled(hr, 0.0, t.canvas);
            let full = crate::i18n::source_title(&app.session);
            let count_w = ui.painter().layout_no_wrap(cnt.clone(), t.font(12.5), t.text_dim).size().x;
            // the title gives way to the count: a long album or folder name is trimmed, in full on hover
            let room = (hr.width() - 20.0 - count_w - 36.0).max(0.0);
            let font = t.semibold(17.0);
            let title = crate::widgets::elide_head(&full, room, |s| ui.painter().layout_no_wrap(s.to_string(), font.clone(), t.text).size().x);
            let title_rect = ui.painter().text(pos2(hr.left() + 20.0, hr.center().y), Align2::LEFT_CENTER, &title, font, t.text);
            let count_rect = ui.painter().text(pos2(hr.right() - 20.0, hr.center().y), Align2::RIGHT_CENTER, cnt, t.font(12.5), t.text_dim);
            crate::widgets::register(ui.ctx(), "grid:title", title_rect);
            crate::widgets::register(ui.ctx(), "grid:count", count_rect);
            if title != full {
                ui.interact(title_rect, ui.id().with("grid-title"), Sense::hover()).on_hover_text(full);
            }
        }
    }
    let chips = super::chips::with_understanding(app, chips);
    super::chips::show(app, ui, &chips);
    if app.ui.filter_bar {
        super::filterbar::show(app, ui);
    }
    app.canvas_rect = Some(ui.max_rect());
    // photos dropped on the window: a tile for each until it is in the catalog (after the last row)
    let dropped = crate::file_drop::pending(app);
    if ids.is_empty()
        && let Some(pend) = &dropped
        && chips.is_empty()
        && app.session.filter == Default::default()
    {
        crate::file_drop::show_alone(app, ui, pend);
        return;
    }
    if ids.is_empty() {
        if !chips.is_empty() {
            let body = match total {
                Some(n) if n > 0 => {
                    crate::i18n::tr_format!("{n} photos in {} are hidden by the filters above", crate::i18n::source_title(&app.session), n = n)
                }
                _ => "Remove a filter above, or choose Clear all".to_string(),
            };
            super::empty_message(ui, ui.max_rect(), "No photos match the active filters", &body);
        } else if app.session.filter != Default::default() {
            let how = if app.compact {
                "A filter is hiding this view's photos: change it, or clear it with the filter button"
            } else {
                "A filter is hiding this view's photos: change it, or clear it (View → Clear Filters)"
            };
            super::empty_message(ui, ui.max_rect(), "No matching photos", how);
        } else if app.session.source == lightcraft_engine::LibrarySource::Folder {
            super::empty_message(ui, ui.max_rect(), "No photos in this folder", "Turn on Include subfolders, or pick another folder under Local");
        } else {
            // (a phone has no File menu, shortcuts or dropping: its + button, or the … menu)
            let how = if !app.compact {
                "Import photos with File → Import Photos… (Cmd+Shift+I), or drop them here"
            } else if app.services.host_pick.is_some() {
                "Tap + to add photos from Photos or Files"
            } else {
                "Import photos with … → Import Photos…"
            };
            super::empty_message(ui, ui.max_rect(), "No photos", how);
        }
        return;
    }
    let ppp = ui.ctx().pixels_per_point();
    let square = app.ui.view == ViewMode::SquareGrid;
    // a phone's Months and Years (`grid_dates`), faded in when the span changes
    let span = if app.compact { app.ui.grid_span } else { crate::state::GridSpan::All };
    if app.compact {
        ui.multiply_opacity(super::mobile::enter(ui.ctx(), egui::Id::new("grid-span-in"), span, 0.25));
    }
    if span != crate::state::GridSpan::All {
        super::grid_zoom::forget(ui.ctx());
        super::grid_dates::show(app, ui, &ids, generation, span);
        return;
    }
    // a phone: a pinch changes how many photos a row holds, the photos gliding to their new places
    let zoom = if app.compact {
        let area = ui.available_rect_before_wrap();
        super::grid_zoom::update(app, ui, area, generation)
    } else if let (Some(prev), Some(scroll)) = (app.caches.grid.layout.as_ref().map(|(_, l)| Arc::clone(l)), app.grid_scroll) {
        // a desktop: a trackpad pinch sizes the thumbnails
        super::grid_zoom::pinch_thumbs(app, ui, ui.available_rect_before_wrap(), &prev.cells, scroll)
    } else {
        super::grid_zoom::Zoom::none()
    };
    // phone squares (Lightroom's mobile grid): edge to edge, three across by default, 2 pt apart
    let tiles = square && app.compact;
    let cols = f32::from(app.ui.grid_columns.max(1));
    let avail_w = if tiles { ui.available_width() } else { ui.available_width() - 8.0 };
    // phone: about `grid_columns` photos across (and, through `resolve_group`, month headers rather
    // than one per day at three)
    let target = if tiles {
        ((avail_w - (cols - 1.0) * TILE_GAP) / cols).floor().max(8.0)
    } else if app.compact {
        (avail_w / cols).max(24.0)
    } else {
        app.ui.thumb_size
    };
    // ranked results (search by description) are in relevance order, not date order: no date headers
    let by = if app.session.filter.semantic.is_some() { GroupBy::None } else { resolve_group(app.session.sort.group, target) };
    // (a phone heads its months, not its days: a day's few photos would leave most rows half empty)
    let by = if app.compact && by == GroupBy::Day && app.session.sort.group == GroupBy::Auto { GroupBy::Month } else { by };
    let group_key = match app.session.source {
        lightcraft_engine::LibrarySource::RecentlyDeleted => None,
        lightcraft_engine::LibrarySource::RecentlyAdded => Some(SortKey::ImportDate),
        _ => Some(app.session.sort.key),
    };
    let runs_key = (generation, group_key, by);
    let stats = &mut app.caches.grid_stats;
    let runs = app.caches.grid.runs(stats, &app.session.catalog, &ids, runs_key);
    // the layout only changes with the photos (and their shapes), the grouping, the width and
    // the thumbnail size
    // (phone tiles have their own width and size, so they key apart from desktop squares)
    let lay_key = (runs_key, avail_w.to_bits(), target.to_bits(), square);
    let lay = match &app.caches.grid.layout {
        Some((k, l)) if *k == lay_key => l.clone(),
        _ => {
            stats.layout_builds += 1;
            let aspects = if square { vec![1.0; ids.len()] } else { aspects(&app.session.catalog, &ids) };
            let spans: Vec<(usize, usize)> = runs.iter().map(|r| (r.start, r.count)).collect();
            let l = Arc::new(layout(&aspects, &spans, avail_w, target, square, tiles));
            app.caches.grid.layout = Some((lay_key, l.clone()));
            l
        }
    };
    let stacks = app.caches.grid.stacks(stats, &app.session.catalog, generation);
    // (a phone leaves room under the last row for the Years / Months / All capsule)
    let tiles_h = dropped.as_ref().map_or(0.0, |p| crate::file_drop::height(p.len(), avail_w, crate::file_drop::side(app)));
    let total_h = lay.height + tiles_h + if app.compact { super::grid_dates::BOTTOM_ROOM } else { 12.0 };
    let active = app.session.selection.active;
    // where the grid has to go this frame: the photo under the fingers stays under them when the
    // columns change; a month or photo picked in Months
    let top = ui.available_rect_before_wrap().top();
    let mut forced: Option<f32> = None;
    if zoom.changed
        && let Some((i, u)) = zoom.anchor
        && let Some(c) = lay.cells.get(i)
    {
        forced = Some(super::grid_zoom::offset_for(*c, u, zoom.focal, top));
    }
    if let Some(jump) = app.ui.grid_jump.take() {
        forced = jump_offset(app, &ids, &lay, &runs, &jump).or(forced);
    }
    // bring the active photo into view when it changes (keyboard, click, command) or the grid
    // comes back on screen; otherwise the scroll position is the user's
    let follow = follow_active(ui.ctx(), egui::Id::new("grid-follow-active"), active);
    let scroll_to: Option<Rect> =
        if follow && forced.is_none() { active.and_then(|a| ids.iter().position(|x| *x == a)).and_then(|i| lay.cells.get(i).copied()) } else { None };
    let mut visible_ids = HashSet::new();
    let mut visited = 0u64;
    // a phone: a finger's drag scrolls the grid (its cells take only taps), except while a
    // sideways drag is choosing photos (`drag_choose`)
    let source = if !app.compact {
        egui::scroll_area::ScrollSource::default()
    } else if zoom.pinching || ui.data(|d| d.get_temp::<DragChoose>(egui::Id::new(DRAG_CHOOSE))).is_some() {
        egui::scroll_area::ScrollSource { drag: egui::scroll_area::DragScroll::Never, ..Default::default() }
    } else {
        egui::scroll_area::ScrollSource::ALL
    };
    // (a phone has its own handle to drag: `scrubber`)
    let bars =
        if app.compact { egui::scroll_area::ScrollBarVisibility::AlwaysHidden } else { egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded };
    let mut scroll =
        egui::ScrollArea::vertical().id_salt("grid-scroll").scroll_source(source).scroll_bar_visibility(bars).auto_shrink([false, false]);
    if let Some(o) = forced {
        scroll = scroll.vertical_scroll_offset(o);
    }
    scroll.show_viewport(ui, |ui, viewport| {
        let (area, _) = ui.allocate_exact_size(vec2(ui.available_width(), total_h), Sense::hover());
        let origin = area.min;
        app.grid_scroll = Some(viewport.top());
        if let Some(r) = scroll_to {
            ui.scroll_to_rect(r.translate(origin.to_vec2()), None);
        }
        // only the rows on screen and a screen above and below it (their thumbnails are prefetched);
        // more of them while a pinch draws the grid smaller
        let seen = zoom.visible(viewport, origin);
        let prefetch = seen.expand2(vec2(0.0, viewport.height()));
        let clip = ui.clip_rect();
        let mut drawn: Vec<(usize, Rect)> = Vec::new();
        // (a photo flying back from the loupe lands in its cell: `homing`)
        let flying = super::detail::homing(ui.ctx()).is_some();
        for row in rows_between(&lay.rows, prefetch.top(), prefetch.bottom()) {
            for i in row.start..row.end {
                let (Some(&id), Some(&rect)) = (ids.get(i), lay.cells.get(i)) else { continue };
                visited += 1;
                if !rect.intersects(prefetch) {
                    continue;
                }
                visible_ids.insert(id);
                let r = zoom.place(i, rect.translate(origin.to_vec2()));
                let onscreen = r.intersects(clip);
                cell(app, ui, id, r, square, onscreen, ppp);
                if let Some((sid, pos)) = stacks.get(&id) {
                    stack_badge(app, ui, id, *sid, *pos, r, square);
                }
                if (app.compact || flying) && onscreen {
                    drawn.push((i, r));
                }
            }
        }
        if let Some(pend) = &dropped {
            let block = Rect::from_min_size(origin + vec2(0.0, lay.height), vec2(avail_w, tiles_h));
            crate::file_drop::reveal_now(ui, block);
            crate::file_drop::paint_block(ui, pend, block, viewport.translate(origin.to_vec2()), crate::file_drop::side(app));
        }
        homing(app, ui, &ids, &drawn, square);
        if app.compact {
            // photos gliding off the screen to places further up or down
            let gone: Vec<usize> = zoom.gliding().filter(|i| !drawn.iter().any(|(d, _)| d == i)).collect();
            for i in gone {
                let (Some(&id), Some(&rect)) = (ids.get(i), lay.cells.get(i)) else { continue };
                let r = zoom.place(i, rect.translate(origin.to_vec2()));
                if r.intersects(clip) {
                    visible_ids.insert(id);
                    cell(app, ui, id, r, square, true, ppp);
                }
            }
            super::grid_zoom::remember(ui.ctx(), generation, app.ui.grid_columns, drawn);
            if super::detail::pulled(ui.ctx()) <= 0.0 {
                long_press(app, ui, &ids, &lay, origin);
            }
        }
        // back to the top (a tap on the top bar or the status bar), sliding there
        if std::mem::take(&mut app.ui.scroll_top) {
            ui.scroll_to_rect(Rect::from_min_size(origin, vec2(1.0, 1.0)), Some(egui::Align::TOP));
        }
        if app.compact && app.ui.select_mode {
            drag_choose(app, ui, &ids, &lay, origin);
        }
        // date headers (top to bottom); the current group's header sticks to the top while its
        // photos scroll by (not while a pinch moves the photos about)
        let headers: &[Rect] = if zoom.moving() { &[] } else { &lay.headers };
        let first = headers.partition_point(|h| h.bottom() < viewport.top());
        for (g, hr) in headers.iter().enumerate().skip(first) {
            if hr.top() > viewport.bottom() {
                break;
            }
            if let Some(run) = runs.get(g) {
                group_header(app, ui, run, &ids, hr.translate(origin.to_vec2()), false);
            }
        }
        let sticky = headers.partition_point(|h| h.top() <= viewport.top()).checked_sub(1);
        if let Some(g) = sticky
            && let Some(head) = headers.get(g)
            && head.top() < viewport.top()
            && let Some(run) = runs.get(g)
        {
            // pushed up by the next header as it arrives
            let next_top = headers.get(g + 1).map(|h| h.top()).unwrap_or(f32::INFINITY);
            let y = viewport.top().min(next_top - HEADER_H);
            let hr = Rect::from_min_size(pos2(0.0, y), vec2(area.width(), HEADER_H)).translate(origin.to_vec2());
            group_header(app, ui, run, &ids, hr, true);
        }
        if app.compact {
            scrubber(app, ui, viewport, total_h, &ids, &lay);
        }
    });
    app.caches.grid_stats.cells_visited += visited;
    app.renderer.evict_thumbs(&visible_ids, 600);
    let _ = (Color32::BLACK, StrokeKind::Inside, Stroke::NONE);
}

/// The photo let go on its way back from the loupe (`detail::homing`): it flies from where it was
/// to its place in the grid, growing into its square's crop, over the cell (which waits for it).
fn homing(app: &LightcraftApp, ui: &egui::Ui, ids: &[PhotoId], drawn: &[(usize, Rect)], square: bool) {
    let Some(h) = super::detail::homing(ui.ctx()) else { return };
    let Some(&(_, to)) = drawn.iter().find(|(i, _)| ids.get(*i).is_some_and(|id| id.0 == h.photo)) else { return };
    let id = PhotoId(h.photo);
    let tex = app.renderer.textures.get(&Slot::Main).filter(|t| t.photo == id).or_else(|| app.renderer.thumb(id));
    let Some(tex) = tex else { return };
    let now = ui.input(|i| i.time);
    let k = egui::emath::easing::cubic_out((((now - h.since) / super::detail::HOMING_S) as f32).clamp(0.0, 1.0));
    let [tw, th] = tex.size;
    // (a desktop's square cell has the photo inset under its name, fitted as `cell` draws it)
    let to = if square && !app.compact { Rect::from_min_max(to.min + vec2(10.0, 24.0), to.max - vec2(10.0, 10.0)) } else { to };
    let to = if square && app.compact { to } else { super::detail::fit_texture_rect(to, [tw, th]) };
    // (into a phone square the photo's middle, as the cell shows it)
    let full = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    let end = if square && app.compact { cover_uv(tw as f32, th as f32) } else { full };
    let uv = Rect::from_min_max(full.min.lerp(end.min, k), full.max.lerp(end.max, k));
    let r = Rect::from_min_max(h.from.min.lerp(to.min, k), h.from.max.lerp(to.max, k));
    let p = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("grid-homing"))).with_clip_rect(ui.clip_rect());
    p.image(tex.tex.id(), r, uv, Color32::WHITE);
    ui.ctx().request_repaint();
}

/// The photo lifted out of the grid (pulled down from the loupe, or flying back into place): its
/// cell leaves the space empty for it.
fn lifted(app: &LightcraftApp, ctx: &egui::Context) -> Option<PhotoId> {
    if let Some(h) = super::detail::homing(ctx) {
        return Some(PhotoId(h.photo));
    }
    (super::detail::pulled(ctx) > 0.0).then(|| app.session.active()).flatten()
}

/// The scroll offset that shows `jump` (a photo, or the first of a month or year) near the top.
fn jump_offset(app: &LightcraftApp, ids: &[PhotoId], lay: &GridLayout, runs: &[DateRun], jump: &crate::state::GridJump) -> Option<f32> {
    match jump {
        crate::state::GridJump::Photo(id) => {
            let i = ids.iter().position(|x| x.0 == *id)?;
            // (a little below the top, with its date header above it)
            lay.cells.get(i).map(|c| (c.top() - HEADER_H - 8.0).max(0.0))
        }
        crate::state::GridJump::Date(d) => {
            if let Some(g) = runs.iter().position(|r| r.key.starts_with(d.as_str()))
                && let Some(h) = lay.headers.get(g)
            {
                return Some(h.top().max(0.0));
            }
            let i = ids
                .iter()
                .position(|id| app.session.catalog.photo(*id).and_then(|p| p.captured.as_deref()).is_some_and(|c| c.starts_with(d.as_str())))?;
            lay.cells.get(i).map(|c| (c.top() - HEADER_H - 8.0).max(0.0))
        }
    }
}

/// Memory key of a [`DragChoose`] in progress.
const DRAG_CHOOSE: &str = "grid-drag-choose";

/// A sideways drag over the phone's grid while choosing photos.
#[derive(Clone, Debug)]
struct DragChoose {
    /// Where the drag began (index into the grid's photos).
    start: usize,
    /// Choosing (the first photo wasn't chosen) or unchoosing.
    adding: bool,
    /// What was chosen before the drag.
    base: Vec<u64>,
    /// The photo under the finger last frame.
    last: Option<usize>,
}

/// Choosing photos by touch, as iOS Photos does: a drag that sets off sideways from a photo
/// chooses the run of photos from it to the one under the finger (or unchooses them when the
/// first was chosen); an up or down drag scrolls instead.
fn drag_choose(app: &mut LightcraftApp, ui: &egui::Ui, ids: &[PhotoId], lay: &GridLayout, origin: egui::Pos2) {
    let key = egui::Id::new(DRAG_CHOOSE);
    let (down, press, pos) = ui.input(|i| (i.pointer.primary_down(), i.pointer.press_origin(), i.pointer.interact_pos()));
    let at = |p: egui::Pos2| cell_at(lay, origin, p);
    let mut state: Option<DragChoose> = ui.data(|d| d.get_temp(key));
    if !down {
        if state.is_some() {
            ui.data_mut(|d| d.remove::<DragChoose>(key));
        }
        return;
    }
    if state.is_none()
        && let (Some(o), Some(p)) = (press, pos)
    {
        let d = p - o;
        if d.x.abs() >= 12.0
            && d.x.abs() >= 2.0 * d.y.abs()
            && let Some(start) = at(o)
            && let Some(first) = ids.get(start)
        {
            let adding = !app.session.selection.contains(*first);
            state = Some(DragChoose { start, adding, base: app.session.selection.ids.iter().map(|i| i.0).collect(), last: None });
        }
    }
    let Some(mut s) = state else { return };
    if let Some(cur) = pos.and_then(at)
        && s.last != Some(cur)
    {
        s.last = Some(cur);
        haptics::tap(ui.ctx(), Haptic::Selection);
        let run: HashSet<u64> = ids.get(s.start.min(cur)..=s.start.max(cur)).unwrap_or(&[]).iter().map(|i| i.0).collect();
        let mut chosen: Vec<u64> = s.base.iter().copied().filter(|i| s.adding || !run.contains(i)).collect();
        if s.adding {
            chosen.extend(ids.iter().map(|i| i.0).filter(|i| run.contains(i) && !s.base.contains(i)));
        }
        let _ = app.run("library.select", json!({"ids": chosen}));
    }
    ui.data_mut(|d| d.insert_temp(key, s));
}

/// The photo (index into the grid's photos) whose cell holds screen point `p`; `origin` is where the
/// grid's content starts on screen.
fn cell_at(lay: &GridLayout, origin: egui::Pos2, p: egui::Pos2) -> Option<usize> {
    let local = p - origin.to_vec2();
    rows_between(&lay.rows, local.y, local.y).iter().flat_map(|r| r.start..r.end).find(|i| lay.cells.get(*i).is_some_and(|c| c.contains(local)))
}

/// Memory key: a long press was answered during the press in progress (its release is not a tap).
const LONG_PRESS: &str = "grid-long-press";
/// How long a finger rests on a photo before choosing starts: iOS's long press.
pub const LONG_PRESS_SECS: f64 = 0.5;
/// How far (points) a finger may wander and still be holding still.
const LONG_PRESS_SLOP: f32 = 10.0;

/// Whether the press in progress (or the one just released) was a long press: its tap is not one.
fn long_pressed(ui: &egui::Ui) -> bool {
    ui.data(|d| d.get_temp::<bool>(egui::Id::new(LONG_PRESS))).unwrap_or(false)
}

/// A long press on a phone's photo, answered while the finger is still down, as iOS Photos does:
/// it starts choosing photos with that one (while choosing, it chooses or unchooses it), and a drag
/// that follows, in any direction, chooses the run to the photo under the finger (`drag_choose`).
/// egui's own long touch waits 0.8 s, needs a touch screen, and fires only if a frame happens to
/// run then; this one asks for that frame, and a mouse held down works the same.
fn long_press(app: &mut LightcraftApp, ui: &egui::Ui, ids: &[PhotoId], lay: &GridLayout, origin: egui::Pos2) {
    let key = egui::Id::new(LONG_PRESS);
    let (pressed, down, press, start, pos, now, touches) = ui.input(|i| {
        (
            i.pointer.any_pressed(),
            i.pointer.primary_down(),
            i.pointer.press_origin(),
            i.pointer.press_start_time(),
            i.pointer.interact_pos(),
            i.time,
            i.multi_touch().is_some(),
        )
    });
    if pressed {
        ui.data_mut(|d| d.remove::<bool>(key));
    }
    if !down || touches || long_pressed(ui) {
        return;
    }
    let (Some(o), Some(start), Some(p)) = (press, start, pos) else { return };
    // (a finger held on the scroll handle is scrubbing, not pressing the photo under it)
    if (p - o).length() > LONG_PRESS_SLOP || scrubber_hit(ui).is_some_and(|r| r.contains(o)) {
        return;
    }
    let Some(i) = cell_at(lay, origin, o) else { return };
    let Some(&id) = ids.get(i) else { return };
    let held = now - start;
    if held < LONG_PRESS_SECS {
        // nothing else may come in while the finger rests: wake up when the time is up
        ui.ctx().request_repaint_after(std::time::Duration::try_from_secs_f64(LONG_PRESS_SECS - held).unwrap_or_default());
        return;
    }
    ui.data_mut(|d| d.insert_temp(key, true));
    haptics::tap(ui.ctx(), Haptic::Medium);
    let base: Vec<u64> = if app.ui.select_mode { app.session.selection.ids.iter().map(|p| p.0).collect() } else { Vec::new() };
    let adding = !(app.ui.select_mode && app.session.selection.contains(id));
    if app.ui.select_mode {
        let _ = app.run("library.select", json!({"ids": [id.0], "mode": "toggle"}));
    } else {
        let _ = app.run("view.selectMode", json!({"on": true, "id": id.0}));
    }
    ui.data_mut(|d| d.insert_temp(egui::Id::new(DRAG_CHOOSE), DragChoose { start: i, adding, base, last: Some(i) }));
}

/// The scroll handle's size (points): a capsule at the grid's right edge, as Lightroom's mobile app
/// and iOS's long lists have it.
const SCRUB_W: f32 = 6.0;
const SCRUB_H: f32 = 48.0;
/// How long the handle stays after the grid stops moving (s).
const SCRUB_LINGER: f64 = 1.4;

/// The scroll handle between frames: the offset last seen, when the grid last moved, and where on
/// the handle the finger holds it (while dragged).
#[derive(Clone, Copy, Debug)]
struct Scrub {
    offset: f32,
    moved_at: f64,
    grab: Option<f32>,
}

impl Default for Scrub {
    fn default() -> Self {
        // (a grid that hasn't moved since it was first drawn shows no handle)
        Scrub { offset: 0.0, moved_at: f64::NEG_INFINITY, grab: None }
    }
}

fn scrub_id() -> egui::Id {
    egui::Id::new("grid-scrubber")
}

/// Where the scroll handle can be taken hold of (last frame), while it shows.
fn scrubber_hit(ui: &egui::Ui) -> Option<Rect> {
    ui.data(|d| d.get_temp::<Option<Rect>>(scrub_id().with("hit"))).flatten()
}

/// The phone grid's scroll handle (widget `grid:scrubber`): it shows while the grid moves and for a
/// moment after, and dragging it runs through the whole library at once, the month of the photos
/// at the top of the screen in a bubble beside it, as iOS's Photos and Lightroom's mobile app do.
/// It drives egui's own scroll state (`scroll_with_delta_animation`, which also stops a fling), so
/// a tap on the top bar or the status bar still slides back to the top. It only takes a finger on
/// the handle itself, never on the photos at the grid's edge.
fn scrubber(app: &LightcraftApp, ui: &mut egui::Ui, viewport: Rect, content_h: f32, ids: &[PhotoId], lay: &GridLayout) {
    let t = Tokens::get(ui.ctx());
    let key = scrub_id();
    let clip = ui.clip_rect();
    let max = content_h - viewport.height();
    let mut st: Scrub = ui.data(|d| d.get_temp(key)).unwrap_or_default();
    let now = ui.input(|i| i.time);
    // nothing to run through: a screen or two of photos scroll well enough by hand
    if max < viewport.height() || !max.is_finite() {
        ui.data_mut(|d| {
            d.insert_temp(key, Scrub::default());
            d.insert_temp(key.with("hit"), None::<Rect>);
        });
        return;
    }
    if (viewport.top() - st.offset).abs() > 0.5 {
        (st.offset, st.moved_at) = (viewport.top(), now);
    }
    let travel = (clip.top() + 8.0)..=(clip.bottom() - 8.0 - SCRUB_H).max(clip.top() + 8.0);
    let span = (travel.end() - travel.start()).max(1.0);
    let y = travel.start() + (viewport.top() / max).clamp(0.0, 1.0) * span;
    let lingering = now - st.moved_at < SCRUB_LINGER;
    let shown = ui.ctx().animate_bool_with_time(key.with("shown"), lingering || st.grab.is_some(), 0.2);
    if lingering {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
    }
    let hit = Rect::from_min_max(pos2(clip.right() - 44.0, y - 12.0), pos2(clip.right(), y + SCRUB_H + 12.0));
    let live = shown > 0.3 || st.grab.is_some();
    if live {
        let resp = ui.interact(hit, key.with("drag"), Sense::drag());
        register(ui.ctx(), "grid:scrubber", hit);
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ScrollBar, true, crate::i18n::tr("Scroll")));
        let pointer = ui.input(|i| i.pointer.interact_pos());
        if resp.drag_started()
            && let Some(p) = pointer
        {
            st.grab = Some(p.y - y);
            haptics::tap(ui.ctx(), Haptic::Selection);
        }
        if resp.dragged()
            && let (Some(p), Some(g)) = (pointer, st.grab)
        {
            let top = (p.y - g).clamp(*travel.start(), *travel.end());
            let want = (top - travel.start()) / span * max;
            // (an instant scroll by the difference: egui keeps the offset, and a fling still under
            // way is stopped)
            ui.scroll_with_delta_animation(vec2(0.0, viewport.top() - want), egui::style::ScrollAnimation::none());
            st.moved_at = now;
        }
        if resp.drag_stopped() || !ui.input(|i| i.pointer.primary_down()) {
            st.grab = None;
        }
    } else {
        st.grab = None;
    }
    ui.data_mut(|d| {
        d.insert_temp(key, st);
        d.insert_temp(key.with("hit"), live.then_some(hit));
    });
    if shown <= 0.0 {
        return;
    }
    // the handle: a capsule that widens while held
    let held = ui.ctx().animate_bool_with_time(key.with("held"), st.grab.is_some(), 0.15);
    let w = SCRUB_W + 3.0 * held;
    let handle = Rect::from_min_size(pos2(clip.right() - 5.0 - w, y), vec2(w, SCRUB_H));
    let p = ui.painter();
    let ink = if Tokens::is_dark(ui.ctx()) { Color32::WHITE } else { Color32::from_gray(30) };
    p.rect_filled(handle.expand(1.0), w / 2.0 + 1.0, Color32::from_black_alpha((60.0 * shown) as u8));
    p.rect_filled(handle, w / 2.0, ink.gamma_multiply(0.85 * shown));
    // while held: the month at the top of the screen beside it
    if held > 0.0
        && let Some(row) = rows_between(&lay.rows, viewport.top(), viewport.top() + 1.0).first()
        && let Some(captured) = ids.get(row.start).and_then(|id| app.session.catalog.photo(*id)).and_then(|p| p.captured.clone())
        && let Some(month) = captured.get(..7)
    {
        let label = crate::i18n::date_group_label(month, false);
        let g = p.layout_no_wrap(label, t.semibold(15.0), t.text);
        let bubble = Rect::from_min_size(pos2(handle.left() - 14.0 - g.size().x - 28.0, handle.center().y - 18.0), vec2(g.size().x + 28.0, 36.0));
        let mut p = p.clone().with_layer_id(egui::LayerId::new(egui::Order::Foreground, key.with("bubble")));
        p.set_opacity(held);
        crate::glass::paint(&p, bubble, 18.0, None, 0.0);
        p.galley(bubble.center() - g.size() / 2.0, g, t.text);
    }
}

/// Should a scrolling photo strip (`key`: the grid, the filmstrip) bring the active photo into
/// view on this pass? Yes when the active photo changed since the strip was last drawn, or when
/// the strip was not drawn on the previous pass (view switch, panel shown again). Otherwise the
/// scroll position is the user's: time passing, finished renders, edits to the photo or a released
/// scroll bar never move it (issue #11).
pub(crate) fn follow_active(ctx: &egui::Context, key: egui::Id, active: Option<PhotoId>) -> bool {
    let pass = ctx.cumulative_pass_nr();
    let last: Option<(Option<PhotoId>, u64)> = ctx.data(|d| d.get_temp(key));
    ctx.data_mut(|d| d.insert_temp(key, (active, pass)));
    match last {
        Some((was, drawn)) => was != active || drawn + 1 < pass,
        None => true,
    }
}

/// Height of a date header row (points).
pub const HEADER_H: f32 = 40.0;

/// Space between the phone's square tiles (points).
pub const TILE_GAP: f32 = 2.0;

/// Cell rectangles (one per photo, in order), date-header rectangles (one per group, top to
/// bottom), the rows of cells (top to bottom, for finding the visible ones by binary search) and
/// the total height of the grid content.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GridLayout {
    pub cells: Vec<Rect>,
    pub headers: Vec<Rect>,
    pub rows: Vec<GridRow>,
    pub height: f32,
}

/// One row of cells: `cells[start..end]`, between `top` and `bottom`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridRow {
    pub top: f32,
    pub bottom: f32,
    pub start: usize,
    pub end: usize,
}

/// The grouping actually shown: `Auto` is days with large thumbnails, months with medium ones and
/// years when zoomed far out.
pub fn resolve_group(by: GroupBy, thumb_size: f32) -> GroupBy {
    match by {
        GroupBy::Auto if thumb_size >= 150.0 => GroupBy::Day,
        GroupBy::Auto if thumb_size >= 110.0 => GroupBy::Month,
        GroupBy::Auto => GroupBy::Year,
        other => other,
    }
}

/// Lay out the grid: `aspects` (width / height per photo; ignored for the square grid) in
/// `groups` (`(start, count)` runs; empty = one group without a header), `avail_w` wide, rows
/// about `target` high (square cells about `target` wide). Every group starts on a new row
/// below its header. Justified rows fill the width exactly except a group's last row, which keeps
/// the target height.
pub fn layout(aspects: &[f32], groups: &[(usize, usize)], avail_w: f32, target: f32, square: bool, tiles: bool) -> GridLayout {
    let n = aspects.len();
    let whole = [(0, n)];
    let (groups, headed) = if groups.is_empty() { (&whole[..], false) } else { (groups, true) };
    let gap = if tiles {
        TILE_GAP
    } else if square {
        1.0
    } else {
        6.0
    };
    // phone tiles run to the screen's edges
    let left = if tiles { 0.0 } else { 4.0 };
    let mut out = GridLayout { cells: vec![Rect::NOTHING; n], headers: Vec::with_capacity(groups.len()), rows: Vec::new(), height: 0.0 };
    let mut y = 4.0f32;
    for &(start, count) in groups {
        let end = (start + count).min(n);
        if headed {
            out.headers.push(Rect::from_min_size(pos2(0.0, y), vec2(avail_w + 8.0, HEADER_H)));
            y += HEADER_H;
        }
        if start >= end {
            continue;
        }
        if square {
            let cols = ((avail_w + gap) / (target + gap)).floor().max(1.0);
            let cw = (avail_w - gap * (cols - 1.0)) / cols;
            for i in start..end {
                let k = (i - start) as f32;
                let (c, r) = (k % cols, (k / cols).floor());
                out.cells[i] = Rect::from_min_size(pos2(left + c * (cw + gap), y + r * (cw + gap)), vec2(cw, cw));
            }
            let per_row = (cols as usize).max(1);
            for (r, row_start) in (start..end).step_by(per_row).enumerate() {
                let top = y + r as f32 * (cw + gap);
                out.rows.push(GridRow { top, bottom: top + cw, start: row_start, end: (row_start + per_row).min(end) });
            }
            let rows = ((end - start) as f32 / cols).ceil();
            y += rows * (cw + gap);
        } else {
            // justified rows: accumulate aspect ratios until the row is full
            let mut i = start;
            while i < end {
                let mut sum = 0.0;
                let mut j = i;
                while j < end {
                    sum += aspects[j].max(0.05);
                    let h = (avail_w - gap * (j - i) as f32) / sum;
                    j += 1;
                    if h <= target {
                        break;
                    }
                }
                let row_h = (avail_w - gap * (j - i - 1) as f32) / sum;
                let h = if j < end || row_h <= target { row_h } else { target };
                let mut x = 4.0;
                for k in i..j {
                    let w = aspects[k].max(0.05) * h;
                    out.cells[k] = Rect::from_min_size(pos2(x, y), vec2(w, h));
                    x += w + gap;
                }
                out.rows.push(GridRow { top: y, bottom: y + h, start: i, end: j });
                y += h + gap;
                i = j;
            }
        }
        if headed {
            y += 8.0;
        }
    }
    out.height = y;
    out
}

/// A date header: "Wednesday, 30 September 2026 · 12 photos". Clicking it selects the group
/// (Cmd/Shift: adds to the selection).
fn group_header(app: &mut LightcraftApp, ui: &mut egui::Ui, run: &DateRun, ids: &[PhotoId], r: Rect, pinned: bool) {
    let t = Tokens::get(ui.ctx());
    let resp = ui.interact(r, egui::Id::new(("group-header", run.start, pinned)), Sense::click());
    if !pinned {
        register(ui.ctx(), format!("group:{}", if run.key.is_empty() { "unknown" } else { &run.key }), r);
    }
    let p = ui.painter();
    if pinned {
        p.rect_filled(r, 0.0, t.canvas);
        p.hline(r.x_range(), r.bottom(), Stroke::new(1.0, t.divider));
    }
    let g = p.layout_no_wrap(crate::i18n::date_group_label(&run.key, false), t.semibold(15.0), if resp.hovered() { t.text } else { t.text_label });
    let x = r.left() + 8.0;
    let gw = g.size().x;
    p.galley(pos2(x, r.center().y - g.size().y / 2.0), g, t.text);
    let n = run.count;
    p.text(
        pos2(x + gw + 10.0, r.center().y),
        Align2::LEFT_CENTER,
        crate::i18n::tr_format!("· {n} photo{}", if n == 1 { "" } else { "s" }, n = n),
        t.font(12.5),
        t.text_dim,
    );
    if resp.clicked() {
        let group: Vec<u64> = ids[run.start..(run.start + run.count).min(ids.len())].iter().map(|p| p.0).collect();
        let m = ui.input(|i| i.modifiers);
        let mode = if m.command || m.shift { "add" } else { "replace" };
        let _ = app.run("library.select", json!({"ids": group, "mode": mode}));
    }
}

/// Request a grid/filmstrip thumbnail at `priority`. An unedited raw without a thumbnail texture
/// gets its embedded camera preview, and the render of the raw behind it follows in the background
/// — except where that render would only redraw what the embedded JPEG already shows (see
/// [`lightcraft_engine::media::Session::embedded_thumb_is_final`]), which for a folder of raws on a
/// network share is the difference between reading every file whole and reading a preview each.
pub fn request_thumb(app: &mut LightcraftApp, id: PhotoId, size: usize, priority: u32) {
    // a slider drag changes the photo every frame: its thumbnail keeps what it shows (a render per
    // tick would take a worker from the loupe) and catches up once the drag ends
    if app.session.interaction.as_ref().is_some_and(|i| i.photo == id) && app.renderer.textures.contains_key(&Slot::Thumb(id)) {
        return;
    }
    let bucket = lightcraft_engine::media::thumb_bucket(size);
    let shown = priority > PREFETCH_THUMB_PRIORITY;
    // A synced photo whose preview hasn't come down yet has no picture, and the render that failed for want of it
    // is remembered (`thumb_current`): without this the grid would ask for it once and the download would be
    // forgotten a few frames later, leaving the cell empty for good while it stays on screen.
    if app.renderer.thumb(id).is_none() {
        app.session.note_thumb_ask(id, shown);
    }
    if let Some(photo) = app.session.catalog.photo(id)
        && app.renderer.thumb_current(photo, bucket, priority)
    {
        return;
    }
    // The camera's embedded preview is this photo's thumbnail (unedited raw): once its quick render
    // has arrived there is nothing left to ask for. Without this the grid would rebuild the render
    // job — and its cache key — on every frame, which is what the embedded-thumbnail record settles.
    if let Some(photo) = app.session.catalog.photo(id)
        && (app.renderer.thumb_quick_is_final(photo, bucket, priority) || app.renderer.thumb_quick_pending(photo, bucket))
    {
        return;
    }
    let Some(job) = app.session.thumb_job_for(id, size, shown) else { return };
    #[cfg(test)]
    {
        app.renderer.thumb_jobs_built += 1;
    }
    let quick = if app.renderer.textures.contains_key(&Slot::Thumb(id)) { None } else { app.session.quick_thumb_job(&job) };
    // A cached render is a stand-in for whatever comes next; an embedded camera preview of an
    // unedited raw is the picture itself, so no sensor decode is scheduled behind it.
    let embedded_is_final =
        quick.is_some() && app.session.catalog.photo(id).is_some_and(|p| skips_the_real_render(app.session.embedded_thumb_is_final(p), true));
    if let Some(photo) = app.session.catalog.photo(id) {
        if embedded_is_final {
            app.renderer.remember_embedded_thumb(photo, bucket, job.key);
        } else {
            app.renderer.remember_thumb(photo, bucket, job.key, quick.is_some());
        }
    }
    match quick {
        Some(q) => {
            app.renderer.request_quick(Slot::ThumbQuick(id), q, priority + 1);
            if !embedded_is_final && !app.renderer.is_pending(Slot::ThumbQuick(id)) {
                app.renderer.request(Slot::Thumb(id), job, BACKGROUND_THUMB_PRIORITY);
            }
        }
        None => app.renderer.request(Slot::Thumb(id), job, priority),
    }
}

/// Whether a photo's grid thumbnail is already the picture it should show, so the render of the raw
/// behind it is skipped. `embedded_is_final` is the engine's answer
/// ([`lightcraft_engine::media::Session::embedded_thumb_is_final`]: the photo is an unedited raw
/// whose camera JPEG the viewer is showing) and `from_embedded` says the stand-in actually came from
/// the embedded preview rather than a cached render of the file.
///
/// Skipping matters where the file is expensive to read: decoding a 56 MB Fuji RAF to draw a 512 px
/// thumbnail — which the 4 416 × 2 944 embedded JPEG already covers — cost 45 s a photo over a
/// 1.2 MB/s share, and the queue it built delayed the render the user was waiting for.
pub(crate) fn skips_the_real_render(embedded_is_final: bool, from_embedded: bool) -> bool {
    embedded_is_final && from_embedded
}

/// Rendered thumbnails replacing embedded previews: after everything on screen.
pub const BACKGROUND_THUMB_PRIORITY: u32 = 3;
/// A thumbnail in the grid's reach above and below the screen (everything above this is on screen or wanted now).
pub const PREFETCH_THUMB_PRIORITY: u32 = 5;

fn cell(app: &mut LightcraftApp, ui: &mut egui::Ui, id: PhotoId, r: Rect, square: bool, onscreen: bool, ppp: f32) {
    let t = Tokens::get(ui.ctx());
    let Some(photo) = app.session.catalog.photo(id).cloned() else { return };
    // (the photo is out of its place: in the loupe being pulled down, or flying back)
    if lifted(app, ui.ctx()) == Some(id) {
        register(ui.ctx(), format!("thumb:{}", id.0), r);
        return;
    }
    // (a phone's cells take only taps and long presses: a drag scrolls the grid)
    let resp = ui.interact(r, egui::Id::new(("cell", id.0)), if app.compact { Sense::click() } else { Sense::click_and_drag() });
    register(ui.ctx(), format!("thumb:{}", id.0), r);
    let state = app.session.selection.state_of(id);
    let selected = state != lightcraft_engine::SelectionState::NotSelected;
    // screen readers: the file, then rating / flag / label
    let mut spoken = photo.file_name.clone();
    if photo.rating > 0 {
        spoken.push_str(&crate::i18n::tr_format!(", {} star{}", photo.rating, if photo.rating == 1 { "" } else { "s" }));
    }
    match photo.flag {
        lightcraft_catalog::Flag::Pick => spoken.push_str(crate::i18n::tr(", picked")),
        lightcraft_catalog::Flag::Reject => spoken.push_str(crate::i18n::tr(", rejected")),
        lightcraft_catalog::Flag::None => {}
    }
    if let Some(l) = photo.label {
        spoken.push_str(&crate::i18n::tr_format!(", {} label", crate::i18n::color_label(&app.session.catalog, l)));
    }
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, &spoken));
    let active = state == lightcraft_engine::SelectionState::Active;
    let tiles = square && app.compact;
    let p = ui.painter();
    let img_rect = if tiles {
        r
    } else if square {
        let base = if selected { t.cell_selected } else { t.cell };
        p.rect_filled(r, 0.0, crate::theme::label_background(base, photo.label, selected));
        Rect::from_min_max(r.min + vec2(10.0, 24.0), r.max - vec2(10.0, 10.0))
    } else {
        r
    };
    // thumbnail
    let size = thumb_px(img_rect.width().max(img_rect.height()), ppp);
    request_thumb(app, id, size, if onscreen { 10 } else { PREFETCH_THUMB_PRIORITY });
    // (choosing by touch draws its own frame: `check_badge`)
    let choosing = app.compact && app.ui.select_mode;
    // (the rings are the page's text colour: white on the dark pages, black on the light ones)
    let dark = crate::theme::Tokens::is_dark(ui.ctx());
    let (ring, ring_sel) = if dark { (Color32::WHITE, Color32::from_gray(170)) } else { (t.text, t.text_dim) };
    if let Some(tex) = app.renderer.thumb(id) {
        let [tw, th] = tex.size;
        // Preserve the decoded preview's ratio when it differs from the catalog dimensions
        // (phone tiles: the photo fills its square, the long side cropped).
        let fit = if tiles { img_rect } else { super::detail::fit_texture_rect(img_rect, [tw, th]) };
        let uv = if tiles { cover_uv(tw as f32, th as f32) } else { Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)) };
        // a thumbnail that was not there yet fades in (one that was, shows at once: it has no stored 0)
        let fade = if app.compact { ui.ctx().animate_bool_with_time(egui::Id::new(("thumb-in", id.0)), true, 0.2) } else { 1.0 };
        p.image(tex.tex.id(), fit, uv, Color32::WHITE.gamma_multiply(fade));
        if active {
            super::detail::note_tile(ui.ctx(), id, fit, uv);
        }
        if active && !choosing && tiles {
            p.rect_stroke(fit, 0.0, Stroke::new(2.0, ring), StrokeKind::Inside);
        } else if active && !choosing {
            p.rect_stroke(fit.expand(if square { 2.0 } else { 0.0 }), 0.0, Stroke::new(2.0, ring), StrokeKind::Outside);
        } else if selected && !choosing && tiles {
            p.rect_stroke(fit, 0.0, Stroke::new(2.0, ring_sel), StrokeKind::Inside);
        } else if selected && !choosing {
            p.rect_stroke(fit, 0.0, Stroke::new(2.0, ring_sel), StrokeKind::Outside);
        }
    } else {
        let ph = img_rect.shrink(if square && !tiles { 20.0 } else { 0.0 });
        p.rect_filled(ph, 0.0, if crate::theme::Tokens::is_dark(ui.ctx()) { Color32::from_gray(38) } else { t.inset });
        if app.compact {
            let _ = ui.ctx().animate_bool_with_time(egui::Id::new(("thumb-in", id.0)), false, 0.0);
        }
        // not drawn yet (or unreadable): the selection is still visible
        let side = if tiles { StrokeKind::Inside } else { StrokeKind::Outside };
        if active && !choosing {
            p.rect_stroke(ph, 0.0, Stroke::new(2.0, ring), side);
        } else if selected && !choosing {
            p.rect_stroke(ph, 0.0, Stroke::new(2.0, ring_sel), side);
        }
        if app.renderer.failure(Slot::Thumb(id)).is_some() {
            // unreadable / missing file
            p.text(ph.center(), Align2::CENTER_CENTER, "!", t.semibold(18.0), t.text_dim);
        }
    }
    // labels and badges
    if square && !tiles && app.ui.show_filenames {
        let m = &photo.meta;
        let name = match app.ui.grid_info.as_str() {
            "exposure" => {
                let parts: Vec<String> = [
                    // whole seconds read "8 s"; fractions as written ("1/250")
                    (!m.shutter.is_empty())
                        .then(|| if m.shutter.contains('/') || m.shutter.ends_with('s') { m.shutter.clone() } else { format!("{} s", m.shutter) }),
                    m.aperture.map(|a| format!("f/{a:.1}").replace(".0", "")),
                    m.iso.map(|i| format!("ISO {i}")),
                    m.focal_mm.map(|f| format!("{f:.0} mm")),
                ]
                .into_iter()
                .flatten()
                .collect();
                if parts.is_empty() { "—".to_string() } else { parts.join(" · ") }
            }
            "date" => photo.captured.as_deref().map(crate::i18n::display_time).unwrap_or_else(|| crate::i18n::tr("No date").into()),
            _ => photo.file_name.rsplit_once('.').map(|(n, _)| n.to_string()).unwrap_or(photo.file_name.clone()),
        };
        p.text(pos2(r.left() + 8.0, r.top() + 12.0), Align2::LEFT_CENTER, name, t.font(10.5), t.text_dim);
        let fmt = photo.format.clone();
        let g = p.layout_no_wrap(fmt, t.semibold(9.0), t.text_label);
        let br = Rect::from_min_size(pos2(r.right() - g.size().x - 14.0, r.top() + 5.0), g.size() + vec2(8.0, 3.0));
        p.rect_filled(br, 2.0, if Tokens::is_dark(ui.ctx()) { Color32::from_gray(26) } else { t.inset });
        p.galley(br.min + vec2(4.0, 1.5), g, t.text_label);
    }
    let show_badges = match app.ui.settings.grid_badges {
        crate::state::GridBadges::Auto => resp.hovered() || selected || photo.rating > 0 || photo.flag != Flag::None || photo.label.is_some(),
        crate::state::GridBadges::Always => true,
        crate::state::GridBadges::Never => false,
    };
    let badge_bar = Rect::from_min_max(pos2(img_rect.left(), img_rect.bottom() - 24.0), img_rect.right_bottom());
    if !square && photo.label.is_some() {
        // Justified photos fill the whole cell, so the label shows as a translucent tinted footer
        // over the photo's bottom edge (opaque, it would hide 24 px of every labelled photo).
        p.rect_filled(badge_bar, 0.0, crate::theme::label_background(t.cell, photo.label, selected).gamma_multiply(0.7));
    }
    if show_badges {
        let bar = badge_bar;
        if (resp.hovered() || selected) && (square || photo.label.is_none()) {
            p.rect_filled(bar, 0.0, Color32::from_black_alpha(120));
        }
        let mut x = bar.left() + 6.0;
        for i in 0..photo.rating {
            paint(p, Rect::from_min_size(pos2(x + i as f32 * 13.0, bar.center().y - 6.0), vec2(12.0, 12.0)), Icon::StarFilled, t.photo_star);
        }
        x += photo.rating as f32 * 13.0 + 4.0;
        match photo.flag {
            Flag::Pick => paint(p, Rect::from_min_size(pos2(x, bar.center().y - 7.0), vec2(14.0, 14.0)), Icon::FlagPick, t.photo_pick),
            Flag::Reject => paint(p, Rect::from_min_size(pos2(x, bar.center().y - 7.0), vec2(14.0, 14.0)), Icon::FlagReject, t.reject),
            Flag::None => {}
        }
        let mut right = bar.right();
        if photo.is_edited() {
            paint(p, Rect::from_min_size(pos2(right - 20.0, bar.center().y - 7.0), vec2(14.0, 14.0)), Icon::Sliders, t.photo_star);
            right -= 20.0;
        }
        if let Some(l) = photo.label {
            p.circle(pos2(right - 12.0, bar.center().y), 5.0, label_color(l), Stroke::new(1.0, Color32::from_black_alpha(120)));
        }
    }
    if let Some(name) = &photo.copy_name {
        // virtual copy: a folded-corner tag at the top right
        let g = p.layout_no_wrap(name.clone(), t.semibold(10.5), Color32::WHITE);
        let tr = if square { img_rect.right_top() + vec2(-6.0, 6.0) } else { img_rect.right_top() + vec2(-5.0, 5.0) };
        let br = Rect::from_min_max(pos2(tr.x - g.size().x - 26.0, tr.y), pos2(tr.x, tr.y + 20.0));
        p.rect_filled(br, 10.0, Color32::from_black_alpha(165));
        let c = pos2(br.left() + 13.0, br.center().y);
        let s = 5.0;
        p.add(egui::Shape::convex_polygon(
            vec![c + vec2(-s, -s), c + vec2(s * 0.3, -s), c + vec2(s, -s * 0.3), c + vec2(s, s), c + vec2(-s, s)],
            Color32::TRANSPARENT,
            Stroke::new(1.2, Color32::WHITE),
        ));
        p.line_segment([c + vec2(s * 0.3, -s), c + vec2(s * 0.3, -s * 0.3)], Stroke::new(1.2, Color32::WHITE));
        p.line_segment([c + vec2(s * 0.3, -s * 0.3), c + vec2(s, -s * 0.3)], Stroke::new(1.2, Color32::WHITE));
        p.galley(pos2(br.left() + 22.0, br.center().y - g.size().y / 2.0), g, Color32::WHITE);
    }
    if let Some(why) = &photo.preview_only {
        // a raw shown from its embedded JPEG: a small amber "Preview" pill at the image's bottom
        // left, above the badge bar (always shown: it changes what editing does)
        let g = p.layout_no_wrap(crate::i18n::tr("Preview").into(), t.semibold(9.5), Color32::WHITE);
        let at = pos2(img_rect.left() + 6.0, img_rect.bottom() - 30.0 - 16.0);
        let br = Rect::from_min_size(at, vec2(g.size().x + 24.0, 16.0));
        p.rect_filled(br, 8.0, Color32::from_black_alpha(170));
        paint(p, Rect::from_center_size(pos2(br.left() + 9.0, br.center().y), vec2(12.0, 12.0)), Icon::Info, t.caution);
        p.galley(pos2(br.left() + 17.0, br.center().y - g.size().y / 2.0), g, Color32::WHITE);
        register(ui.ctx(), format!("badge:previewOnly:{}", id.0), br);
        ui.interact(br, egui::Id::new(("preview-only-badge", id.0)), Sense::hover())
            .on_hover_text(crate::i18n::tr_format!("Preview only — {}", crate::widgets::preview_only_explanation(why)));
    }
    if photo.flag == Flag::Reject {
        p.rect_filled(img_rect, 0.0, Color32::from_black_alpha(110));
    }
    // interaction
    if let Some(k) = app.ui.keyword_painter.clone() {
        // painting: a click toggles the keyword on this photo
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        }
        if resp.clicked() {
            let has = photo.meta.keywords.iter().any(|x| x.eq_ignore_ascii_case(&k));
            let key = if has { "removeKeywords" } else { "addKeywords" };
            let _ = app.run("photo.setMeta", json!({"ids": [id.0], key: [k]}));
        }
        return;
    }
    if app.compact {
        // touch: a tap opens the photo (no double-tap, no modifiers); choosing photos, it adds or
        // removes it; a long press starts choosing (there is no right click: the action bar and
        // Menu have what the context menu would)
        // (a long press is answered while the finger is down, `long_press`: its release is no tap)
        let tapped = resp.clicked() && !long_pressed(ui);
        if app.ui.select_mode {
            check_badge(ui, img_rect, selected, &t, egui::Id::new(("check", id.0)));
            if tapped {
                let _ = app.run("library.select", json!({"ids": [id.0], "mode": "toggle"}));
                haptics::tap(ui.ctx(), Haptic::Selection);
            }
        } else if tapped {
            let _ = app.run("library.select", json!({"ids": [id.0]}));
            let _ = app.run("view.detail", json!({}));
        } else if resp.clicked_by(egui::PointerButton::Secondary) {
            // a right click (a mouse on a narrow window) starts choosing too
            let _ = app.run("view.selectMode", json!({"on": true, "id": id.0}));
            haptics::tap(ui.ctx(), Haptic::Medium);
        }
        return;
    }
    if resp.clicked() {
        let m = ui.input(|i| i.modifiers);
        let mode = if m.shift {
            "range"
        } else if m.command {
            "toggle"
        } else {
            "replace"
        };
        let _ = app.run("library.select", json!({"ids": [id.0], "mode": mode}));
    }
    if resp.double_clicked() {
        let _ = app.run("library.select", json!({"ids": [id.0]}));
        let _ = app.run("view.detail", json!({}));
    }
    // drag the selection (this photo joins it, or replaces it when it wasn't selected)
    if resp.drag_started() {
        if !app.session.selection.contains(id) {
            let _ = app.run("library.select", json!({"ids": [id.0]}));
        }
        app.ui.dragging_photos = Some(app.session.selection.ids.iter().map(|p| p.0).collect());
    }
    resp.context_menu(|ui| context_menu(app, ui, id));
}

/// The part of a `w` × `h` texture that fills a square (the middle of its long side).
pub fn cover_uv(w: f32, h: f32) -> Rect {
    let (w, h) = (w.max(1.0), h.max(1.0));
    if w > h {
        let k = h / w;
        Rect::from_min_max(pos2((1.0 - k) / 2.0, 0.0), pos2((1.0 + k) / 2.0, 1.0))
    } else {
        let k = w / h;
        Rect::from_min_max(pos2(0.0, (1.0 - k) / 2.0), pos2(1.0, (1.0 + k) / 2.0))
    }
}

/// Choosing photos by touch: a circle at the cell's top right, filled with a check when chosen.
fn check_badge(ui: &egui::Ui, img: Rect, selected: bool, t: &Tokens, id: egui::Id) {
    let p = ui.painter();
    let c = pos2(img.right() - 15.0, img.top() + 15.0);
    // the circle fills and the tick pops in (and back out) as the photo is chosen
    let k = ui.ctx().animate_bool_with_time(id, selected, 0.18);
    p.circle(c, 10.0, Color32::from_black_alpha(70).lerp_to_gamma(t.accent, k), Stroke::new(1.5, Color32::WHITE));
    if k > 0.0 {
        p.rect_stroke(img, 0.0, Stroke::new(3.0 * k, t.accent.gamma_multiply(k)), StrokeKind::Inside);
        let s = k * (1.0 + 0.25 * (k * std::f32::consts::PI).sin());
        let tick = [c + vec2(-4.5, 0.0) * s, c + vec2(-1.5, 3.5) * s, c + vec2(4.5, -3.5) * s];
        let stroke = Stroke::new(2.0, Color32::WHITE.gamma_multiply(k));
        p.line_segment([tick[0], tick[1]], stroke);
        p.line_segment([tick[1], tick[2]], stroke);
    }
}

/// The header of a Local folder view: the path as a breadcrumb (each part opens that folder),
/// Include subfolders, Add to My Photos.
fn folder_header(app: &mut LightcraftApp, ui: &mut egui::Ui, b: &lightcraft_engine::Browse, ids: &[PhotoId], local_n: usize, cnt: &str) -> Rect {
    let t = Tokens::get(ui.ctx());
    // The path/count and actions need independent rows. The frame reserves the actual height,
    // including any wrapped actions, so the grid below never paints under the header.
    let header = egui::Frame::NONE.fill(t.canvas).inner_margin(egui::Margin::symmetric(16, 6)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let count_w =
                ui.painter().layout_no_wrap(cnt.to_owned(), t.font(12.5), t.text_dim).size().x.ceil().min((ui.available_width() * 0.45).floor());
            let path_w = (ui.available_width() - count_w - 8.0).max(0.0).floor();
            ui.allocate_ui_with_layout(vec2(path_w, 24.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                folder_breadcrumbs(app, ui, &b.path);
            });
            let count = ui.add_sized(vec2(count_w, 24.0), egui::Label::new(egui::RichText::new(cnt).size(12.5).color(t.text_dim)).truncate());
            register(ui.ctx(), "grid:count", count.rect);
            count.on_hover_text(cnt);
        });
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
            let mut sub = b.subfolders;
            let c = crate::widgets::check(ui, &mut sub, crate::i18n::tr("Include subfolders"));
            register(ui.ctx(), "check:includeSubfolders", c.rect);
            if c.changed() {
                let _ = app.run("library.browse", json!({"path": b.path, "subfolders": sub}));
            }
            // Counted once per view (cached); the ids are only gathered on a click.
            if local_n > 0 {
                let label = crate::i18n::tr_format!("Add {local_n} to My Photos", local_n = local_n);
                let button =
                    ui.add(egui::Button::new(egui::RichText::new(label).font(t.semibold(11.5)).color(t.text)).wrap().min_size(vec2(37.0, 24.0)));
                register(ui.ctx(), "button:addToLibrary", button.rect);
                if button.clicked() {
                    let local: Vec<u64> = ids.iter().filter(|id| app.session.catalog.photo(**id).is_some_and(|p| p.local)).map(|id| id.0).collect();
                    let n = local.len();
                    match app.run("photo.addToLibrary", json!({"ids": local})) {
                        Ok(_) => app.toast(ui.ctx(), crate::i18n::tr_format!("Added {n} photo{} to My Photos", if n == 1 { "" } else { "s" }, n = n)),
                        Err(e) => app.toast(ui.ctx(), e),
                    }
                }
            }
        });
    });
    register(ui.ctx(), "grid:folderHeader", header.response.rect);
    header.response.rect
}

/// The folders a breadcrumb shows for `path`: each name with the path that opens it, spelled as
/// in `path` (its separators, a leading `/` or a UNC `\\server\share`; issue #538). A bare drive
/// (`C:`) gets its separator back: on its own it means the current folder on that drive.
pub(crate) fn crumbs(path: &str) -> Vec<(&str, String)> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, c) in path.char_indices().chain(std::iter::once((path.len(), '/'))) {
        if c != '/' && c != '\\' {
            continue;
        }
        if let Some(part) = path.get(start..i).filter(|p| !p.is_empty()) {
            let mut to = path.get(..i).unwrap_or(path).to_string();
            if out.is_empty() && part.ends_with(':') && to == part {
                to.push(if path.contains('\\') { '\\' } else { '/' });
            }
            out.push((part, to));
        }
        start = i + c.len_utf8();
    }
    out
}

fn folder_breadcrumbs(app: &mut LightcraftApp, ui: &mut egui::Ui, path: &str) {
    let t = Tokens::get(ui.ctx());
    ui.spacing_mut().item_spacing.x = 4.0;
    let crumbs = crumbs(path);
    let parts: Vec<&str> = crumbs.iter().map(|(name, _)| *name).collect();
    // With all sidebars open even four ellipses and their separators can exceed the row.
    // Prefer the current folder, showing more ancestors as space permits.
    let visible = ((ui.available_width() / 80.0) as usize).clamp(1, 4);
    let skip = parts.len().saturating_sub(visible);
    if skip > 0 {
        ui.label(egui::RichText::new("…  ›").color(t.text_dim)).on_hover_text(path);
    }
    let separator_w = ui.painter().layout_no_wrap("›".into(), t.font(13.0), t.text_dim).size().x + 8.0;
    for (i, part) in parts.iter().enumerate().skip(skip) {
        let remaining = parts.len() - i;
        let last = remaining == 1;
        // Share the bounded row among the remaining crumbs, leaving room for every separator.
        let width = ((ui.available_width() - separator_w * (remaining - 1) as f32) / remaining as f32).max(0.0).floor();
        let text = egui::RichText::new(*part).size(13.0).color(if last { t.text } else { t.text_label });
        let r = ui.add_sized(vec2(width, 24.0), egui::Label::new(if last { text.strong() } else { text }).truncate().sense(Sense::click()));
        register(ui.ctx(), format!("crumb:{i}"), r.rect);
        if !last {
            if r.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if r.clicked()
                && let Some((_, to)) = crumbs.get(i)
            {
                let _ = app.run("library.browse", json!({"path": to}));
            }
            ui.label(egui::RichText::new("›").size(13.0).color(t.text_dim));
        }
        r.on_hover_text(path);
    }
}

/// While photos are dragged: a badge at the pointer; the drag ends when the button is up
/// (drop targets act on the release frame, before this runs).
pub fn drag_feedback(app: &mut LightcraftApp, ctx: &egui::Context) {
    let Some(ids) = &app.ui.dragging_photos else { return };
    let (released, down, pos) = ctx.input(|i| (i.pointer.any_released(), i.pointer.any_down(), i.pointer.latest_pos()));
    if released || !down {
        app.ui.dragging_photos = None;
        return;
    }
    let Some(pos) = pos else { return };
    let t = Tokens::get(ctx);
    let n = ids.len();
    ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
    egui::Area::new(egui::Id::new("drag-photos")).order(egui::Order::Tooltip).interactable(false).fixed_pos(pos + vec2(14.0, 10.0)).show(ctx, |ui| {
        egui::Frame::NONE.fill(t.accent).corner_radius(10.0).inner_margin(egui::Margin::symmetric(9, 3)).show(ui, |ui| {
            ui.label(
                egui::RichText::new(crate::i18n::tr_format!("{n} photo{}", if n == 1 { "" } else { "s" }, n = n))
                    .color(Color32::WHITE)
                    .font(t.semibold(12.0)),
            );
        });
    });
}

pub use super::filterbar::label_color;

/// Albums hold library photos (issue #684): true when `ids` are photos and all of them are
/// browsed (Local) ones, which album views never show.
pub fn local_only(app: &LightcraftApp, ids: &[PhotoId]) -> bool {
    !ids.is_empty() && ids.iter().all(|id| app.session.catalog.photo(*id).is_some_and(|p| p.local))
}

/// Why an album action is off for Local photos.
pub fn albums_hint() -> String {
    crate::i18n::tr("Albums hold library photos: Add to My Photos first").to_string()
}

/// Tell the user when an album command skipped Local photos of the selection (`skipped` in its
/// result).
pub fn toast_skipped(app: &mut LightcraftApp, ctx: &egui::Context, r: &serde_json::Value) {
    if let Some(n) = r.get("skipped").and_then(serde_json::Value::as_u64).filter(|n| *n > 0) {
        app.toast(
            ctx,
            crate::i18n::tr_format!(
                "{n} Local photo{} skipped: albums hold library photos (Add to My Photos first)",
                if n == 1 { "" } else { "s" },
                n = n
            ),
        );
    }
}

/// "Set Color Label" items for the active photo, shared by context menus.
pub fn label_menu(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let current = app.session.active().and_then(|id| app.session.catalog.photo(id)).and_then(|p| p.label);
    if let Some(l) = label_items(app, ui, current, "photoLabel") {
        let _ = app.run("photo.label", json!({"label": label_param(l)}));
        ui.close();
    }
}

/// The colour label items of a menu (coloured dot + the label's name, then None and Edit Label
/// Names…), `current` checked; each colour is widget `<widget>:<colour>` (`<widget>:none`) for
/// the control channel. Returns the choice (`Some(None)`: None).
pub fn label_items(app: &mut LightcraftApp, ui: &mut egui::Ui, current: Option<ColorLabel>, widget: &str) -> Option<Option<ColorLabel>> {
    let mut chosen = None;
    for l in ColorLabel::ALL {
        let name = crate::i18n::color_label(&app.session.catalog, l);
        let resp = ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
            ui.painter().circle_filled(r.center(), 5.0, label_color(l));
            ui.selectable_label(current == Some(l), name)
        });
        register(ui.ctx(), format!("{widget}:{}", label_param(Some(l))), resp.inner.rect);
        if resp.inner.clicked() {
            chosen = Some(Some(l));
        }
    }
    let none = ui.selectable_label(current.is_none(), crate::i18n::tr("None"));
    register(ui.ctx(), format!("{widget}:none"), none.rect);
    if none.clicked() {
        chosen = Some(None);
    }
    ui.separator();
    if ui.button(crate::i18n::tr("Edit Label Names…")).clicked() {
        let _ = app.run("dialog.labelNames", json!({}));
    }
    chosen
}

/// A label as the `label` parameter of `photo.label` / `folder.label` names it.
pub fn label_param(l: Option<ColorLabel>) -> String {
    l.map_or_else(|| "none".to_string(), |l| format!("{l:?}").to_lowercase())
}

/// Stack badge at the cell's top-left: the photo count on a collapsed stack's top, `i/n` on the
/// members of an expanded stack. Clicking it expands/collapses the stack.
pub fn stack_badge(app: &mut LightcraftApp, ui: &mut egui::Ui, id: PhotoId, sid: lightcraft_catalog::StackId, pos: usize, r: Rect, square: bool) {
    let t = Tokens::get(ui.ctx());
    let Some(st) = app.session.catalog.stack(sid) else { return };
    let n = st.photos.len();
    let collapsed = st.collapsed;
    let text = if collapsed { n.to_string() } else { format!("{}/{n}", pos + 1) };
    let p = ui.painter();
    let g = p.layout_no_wrap(text, t.semibold(11.0), Color32::WHITE);
    let origin = if square { r.min + vec2(10.0, 26.0) } else { r.min + vec2(5.0, 5.0) };
    let br = Rect::from_min_size(origin, vec2(g.size().x + 30.0, 20.0));
    let resp = ui.interact(br, egui::Id::new(("stack-badge", id.0)), Sense::click()).on_hover_text(if collapsed {
        "Stack — click to expand (S)"
    } else {
        "Stack — click to collapse (S)"
    });
    register(ui.ctx(), format!("stack:{}", id.0), br);
    let fill = if resp.hovered() { Color32::from_black_alpha(220) } else { Color32::from_black_alpha(165) };
    p.rect_filled(br, 10.0, fill);
    if pos == 0 && !collapsed {
        p.rect_stroke(br, 10.0, Stroke::new(1.0, t.accent), StrokeKind::Inside);
    }
    paint(p, Rect::from_min_size(br.min + vec2(6.0, 3.0), vec2(14.0, 14.0)), Icon::Stack, Color32::WHITE);
    p.galley(pos2(br.min.x + 24.0, br.center().y - g.size().y / 2.0), g, Color32::WHITE);
    if resp.clicked() {
        let _ = app.run("stack.toggle", json!({"ids": [id.0]}));
    }
}

pub fn context_menu(app: &mut LightcraftApp, ui: &mut egui::Ui, id: PhotoId) {
    if !app.session.selection.contains(id) {
        let _ = app.run("library.select", json!({"ids": [id.0]}));
    }
    if ui.button(crate::i18n::tr("Open in Detail")).clicked() {
        let _ = app.run("view.detail", json!({}));
    }
    // Photo ▸ Edit In: a copy with this photo's edits, opened in PhotoCraft or another editor
    // (desktop only: the web can't start programs)
    ui.add_enabled_ui(crate::menus::ui_enabled(app, "photo.editInPhotoCraft"), |ui| {
        ui.menu_button(crate::i18n::tr("Edit In"), |ui| {
            if ui.button(crate::i18n::tr("Edit in PhotoCraft")).clicked() {
                let _ = app.run("photo.editInPhotoCraft", json!({}));
            }
            if ui.button(crate::i18n::tr("Edit in Other Editor")).clicked() {
                let _ = app.run("photo.editInExternal", json!({}));
            }
        });
    });
    if ui.button(crate::i18n::tr("Find Similar Photos")).clicked() {
        match app.run("library.findSimilar", json!({"id": id.0})) {
            Ok(r) => {
                let n = r["photos"].as_array().map_or(0, Vec::len);
                app.ui.view = crate::state::ViewMode::PhotoGrid;
                app.toast(
                    ui.ctx(),
                    crate::i18n::tr_format!("{n} similar photo{} · View ▸ Clear Filters to see all", if n == 1 { "" } else { "s" }, n = n),
                );
            }
            Err(e) => app.toast(ui.ctx(), e),
        }
    }
    if ui.button(crate::i18n::tr("Set as Reference Photo")).clicked() {
        let _ = app.run("photo.setReference", json!({"id": id.0}));
    }
    ui.separator();
    ui.menu_button(crate::i18n::tr("Set Rating"), |ui| {
        for r in 0..=5 {
            if ui.button(if r == 0 { crate::i18n::tr("No Stars").to_string() } else { "★".repeat(r) }).clicked() {
                let _ = app.run("photo.rate", json!({"rating": r}));
            }
        }
    });
    ui.menu_button(crate::i18n::tr("Set Flag"), |ui| {
        for (l, f) in [("Pick", "pick"), ("Reject", "reject"), ("Unflagged", "none")] {
            if ui.button(l).clicked() {
                let _ = app.run("photo.flag", json!({"flag": f}));
            }
        }
    });
    ui.menu_button(crate::i18n::tr("Set Color Label"), |ui| label_menu(app, ui));
    if local_only(app, &app.session.targets(&serde_json::Value::Null)) {
        // albums hold library photos (issue #684): nothing here to add until they join My Photos
        let add = ui.add_enabled(false, egui::Button::new(crate::i18n::tr("Add to Album"))).on_disabled_hover_text(albums_hint());
        register(ui.ctx(), "button:contextAddToAlbum", add.rect);
    } else {
        let add = ui.menu_button(crate::i18n::tr("Add to Album"), |ui| {
            let albums: Vec<_> = app.session.catalog.albums().filter(|a| !a.folder && !a.is_smart()).map(|a| (a.id.0, a.name.clone())).collect();
            let any = !albums.is_empty();
            for (aid, name) in albums {
                let b = ui.button(name);
                register(ui.ctx(), format!("button:contextAlbum:{aid}"), b.rect);
                if b.clicked() {
                    match app.run("album.addPhotos", json!({"id": aid})) {
                        Ok(r) => toast_skipped(app, ui.ctx(), &r),
                        Err(e) => app.toast(ui.ctx(), e),
                    }
                }
            }
            if any {
                ui.separator();
            }
            // as in the Photo menu: a new album holding the selection, also when there is none yet
            // to pick (issue #676: the submenu came up empty)
            let new = ui.button(crate::i18n::tr("New Album…"));
            register(ui.ctx(), "button:contextNewAlbum", new.rect);
            if new.clicked() {
                let _ = app.run("dialog.newAlbum", json!({}));
            }
        });
        register(ui.ctx(), "button:contextAddToAlbum", add.response.rect);
    }
    // in a (non-smart) album: make this photo its cover (taking photos out of it is by Delete Photo)
    if let Some(aid) = crate::menus::removable_album(app)
        && ui.button(crate::i18n::tr("Set as Album Cover")).clicked()
    {
        let _ = app.run("album.setCover", json!({"id": aid.0, "photo": id.0}));
    }
    if ui.button(crate::i18n::tr("Rename…")).clicked() {
        let _ = app.run("dialog.rename", json!({}));
    }
    let can_pick = app.services.pick_folder.is_some() || app.services.picker.is_some();
    if can_pick && ui.button(crate::i18n::tr("Move to Folder…")).clicked() {
        // the folder dialog goes through the shared picker (off the UI thread where the host has
        // one: then photo.moveToFolder runs when it closes)
        let req = crate::pick::PickRequest::folder(crate::i18n::tr("Move to Folder…"));
        let picked =
            crate::pick::ask(app, "photo.moveToFolder", &json!({}), "folder", req, |s| s.pick_folder.as_mut().and_then(|f| f()).map(|x| vec![x]));
        if let crate::pick::Picked::Now(v) = picked
            && let Some(into) = v.into_iter().next()
        {
            match app.run("photo.moveToFolder", json!({"folder": into})) {
                Ok(r) => app.toast(ui.ctx(), crate::i18n::tr_format!("Moved {} photo(s)", r["moved"])),
                Err(e) => app.toast(ui.ctx(), e),
            }
        }
    }
    if ui.button(crate::i18n::tr("Reload from Disk")).clicked() {
        let _ = app.run("photo.reload", json!({}));
    }
    if ui.button(crate::i18n::tr("Create Virtual Copy")).clicked() {
        let _ = app.run("photo.virtualCopy", json!({}));
    }
    if ui.button(crate::i18n::tr("Create Version")).clicked() {
        let _ = app.run("version.create", json!({}));
    }
    ui.menu_button(crate::i18n::tr("Stack"), |ui| {
        let stacked = app.session.catalog.stack_of(id).is_some();
        let several = app.session.selection.ids.len() > 1;
        for (label, cmd, on) in [
            ("Group into Stack", "stack.group", several),
            ("Ungroup Stack", "stack.ungroup", stacked),
            ("Remove from Stack", "stack.remove", stacked),
            ("Set as Top of Stack", "stack.setTop", stacked),
            ("Move Up in Stack", "stack.moveUp", stacked),
            ("Move Down in Stack", "stack.moveDown", stacked),
            ("Split Stack", "stack.split", stacked),
            ("Expand/Collapse Stack", "stack.toggle", stacked),
        ] {
            if ui.add_enabled(on, egui::Button::new(label)).clicked() {
                let _ = app.run(cmd, json!({}));
            }
        }
        ui.separator();
        if ui.button(crate::i18n::tr("Auto-Stack by Capture Time…")).clicked() {
            let _ = app.run("dialog.autoStack", json!({}));
        }
    });
    ui.separator();
    if ui.button(crate::i18n::tr("Copy Edit Settings")).clicked() {
        let _ = app.run("develop.copy", json!({}));
    }
    if ui.add_enabled(app.session.clipboard.is_some(), egui::Button::new(crate::i18n::tr("Paste Edit Settings"))).clicked() {
        let _ = app.run("develop.paste", json!({}));
    }
    if ui.add_enabled(app.session.clipboard.is_some(), egui::Button::new(crate::i18n::tr("Paste Selected Settings…"))).clicked() {
        let _ = app.run("dialog.pasteSettings", json!({}));
    }
    if ui.button(crate::i18n::tr("Reset Edits")).clicked() {
        let _ = app.run("develop.reset", json!({}));
    }
    ui.menu_button(crate::i18n::tr("Photo Merge"), |ui| {
        let n = app.session.targets(&json!({})).len();
        for (id, label) in [("dialog.mergeHdr", "HDR…"), ("dialog.mergePanorama", "Panorama…"), ("dialog.mergeHdrPanorama", "HDR Panorama…")] {
            if ui.add_enabled(n >= 2, egui::Button::new(label)).clicked() {
                let _ = app.run(id, json!({}));
            }
        }
    });
    ui.separator();
    if ui.button(crate::i18n::tr("Rotate Left")).clicked() {
        let _ = app.run("photo.rotateLeft", json!({}));
    }
    if ui.button(crate::i18n::tr("Rotate Right")).clicked() {
        let _ = app.run("photo.rotateRight", json!({}));
    }
    ui.separator();
    // the original moved or its drive is gone: point the photo at the file again
    // (a cached answer, checked off the UI thread)
    // (a synced photo whose original is on the server isn't missing)
    let missing = matches!(&app.session.catalog.photo(id).map(|p| p.source.clone()), Some(lightcraft_catalog::Source::File { path })
        if !path.starts_with(lightcraft_engine::catalog::sync::PATH_PREFIX) && app.session.media.availability.is_offline(path));
    if missing && ui.button(crate::i18n::tr("Locate Missing File…")).clicked() {
        let _ = app.run("photo.locate", json!({}));
    }
    if ui.add_enabled(crate::menus::ui_enabled(app, "app.showInFinder"), egui::Button::new(crate::i18n::tr(crate::menus::reveal_label()))).clicked() {
        let _ = app.run("app.showInFinder", json!({}));
    }
    if ui.button(crate::i18n::tr("Export…")).clicked() {
        let _ = app.run("dialog.export", json!({}));
    }
    ui.menu_button(crate::i18n::tr("Export with Preset"), |ui| {
        for (p, _) in app.session.all_export_presets() {
            if ui.button(&p.name).clicked() {
                match app.run("app.export", json!({"preset": p.name, "background": true})) {
                    Ok(r) if r.get("background").is_some() => {}
                    Ok(r) => {
                        let n = r["files"].as_array().map_or(0, Vec::len);
                        app.toast(ui.ctx(), crate::i18n::tr_format!("Exported {n} photo{}", if n == 1 { "" } else { "s" }, n = n));
                    }
                    Err(e) => app.toast(ui.ctx(), e),
                }
            }
        }
    });
    ui.separator();
    if crate::menubar::selection_deleted(app) {
        if ui.button(crate::i18n::tr("Restore")).clicked() {
            let _ = app.run("photo.restore", json!({}));
        }
        if ui.button(crate::i18n::tr("Delete Permanently")).clicked() {
            let _ = app.run("photo.deletePermanently", json!({}));
        }
    } else {
        // in a (non-smart) album: out of the album (⌫), or out of the library (⌘⌫, after asking)
        if let Some(aid) = crate::menus::removable_album(app) {
            let remove = ui.button(crate::i18n::tr("Remove from Album"));
            register(ui.ctx(), "button:contextRemoveFromAlbum", remove.rect);
            if remove.clicked() {
                let _ = app.run("album.removePhotos", json!({"id": aid.0}));
            }
        }
        let delete = ui.button(crate::i18n::tr("Delete Photo"));
        register(ui.ctx(), "button:contextDeletePhoto", delete.rect);
        if delete.clicked() && !crate::menus::confirm_delete(app) {
            let _ = app.run("photo.delete", json!({}));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ungrouped_justified_rows_fill_the_width() {
        let aspects = [1.5f32, 1.5, 0.66, 1.0, 1.5, 1.5, 1.5];
        let l = layout(&aspects, &[], 600.0, 150.0, false, false);
        assert!(l.headers.is_empty());
        assert_eq!(l.cells.len(), aspects.len());
        // full rows end at the right edge; rows are at most the target height
        let first_row: Vec<&Rect> = l.cells.iter().filter(|c| (c.top() - l.cells[0].top()).abs() < 0.01).collect();
        assert!(first_row.len() >= 2);
        assert!((first_row.last().unwrap().right() - 604.0).abs() < 0.5, "{first_row:?}");
        assert!(first_row.iter().all(|c| c.height() <= 150.0 + 0.01));
        let last = l.cells.last().unwrap();
        assert!(last.right() <= 604.0 + 0.5);
        assert!((l.height - (last.bottom() + 6.0)).abs() < 0.01);
    }

    #[test]
    fn groups_start_new_rows_below_their_headers() {
        let aspects = [1.5f32; 7];
        let l = layout(&aspects, &[(0, 2), (2, 5)], 1000.0, 200.0, false, false);
        assert_eq!(l.headers.len(), 2);
        assert_eq!(l.headers[0].top(), 4.0);
        assert_eq!(l.headers[0].height(), HEADER_H);
        // group 1: two photos in a short row below its header (not stretched)
        assert_eq!(l.cells[0].top(), l.headers[0].bottom());
        assert!((l.cells[0].height() - 200.0).abs() < 0.01);
        assert_eq!(l.cells[1].top(), l.cells[0].top());
        // group 2 starts on a new row below its own header, after group 1's photos
        assert!(l.headers[1].top() >= l.cells[1].bottom());
        assert_eq!(l.cells[2].top(), l.headers[1].bottom());
        assert!((l.cells[2].left() - 4.0).abs() < 0.01, "a group's first photo starts the row");
        for c in &l.cells[2..] {
            assert!(c.top() >= l.headers[1].bottom());
        }
        // every photo has a cell; nothing overlaps
        for (i, a) in l.cells.iter().enumerate() {
            assert!(a.width() > 0.0);
            for b in &l.cells[i + 1..] {
                assert!(!a.shrink(0.5).intersects(b.shrink(0.5)), "{a:?} {b:?}");
            }
            for h in &l.headers {
                assert!(!a.shrink(0.5).intersects(*h));
            }
        }
    }

    /// The row index covers every cell once, in order and top to bottom, so the visible cells
    /// are found by binary search.
    #[test]
    fn rows_index_every_cell_top_to_bottom() {
        let aspects: Vec<f32> = (0..500).map(|i| [1.5f32, 0.66, 1.0, 1.78][i % 4]).collect();
        let groups = [(0, 7), (7, 200), (207, 1), (208, 292)];
        for square in [false, true] {
            let l = layout(&aspects, &groups, 900.0, 140.0, square, false);
            let mut next = 0;
            for (k, row) in l.rows.iter().enumerate() {
                assert_eq!(row.start, next, "rows are contiguous");
                assert!(row.end > row.start);
                for c in &l.cells[row.start..row.end] {
                    assert!((c.top() - row.top).abs() < 0.01 && (c.bottom() - row.bottom).abs() < 0.01, "{c:?} {row:?}");
                }
                if let Some(prev) = k.checked_sub(1).map(|p| l.rows[p]) {
                    assert!(row.top > prev.bottom, "{prev:?} {row:?}");
                }
                next = row.end;
            }
            assert_eq!(next, aspects.len());
            // the binary search finds exactly the cells a linear scan does
            for (top, bottom) in [(0.0, 300.0), (1234.0, 2100.0), (l.height - 50.0, l.height + 900.0), (-500.0, -1.0)] {
                let band = Rect::from_min_max(pos2(0.0, top), pos2(2000.0, bottom));
                let want: Vec<usize> = (0..aspects.len()).filter(|i| l.cells[*i].intersects(band)).collect();
                let got: Vec<usize> = rows_between(&l.rows, top, bottom).iter().flat_map(|r| r.start..r.end).collect();
                assert_eq!(got, want, "square {square} {top}..{bottom}");
            }
        }
    }

    /// The phone's squares: three across, edge to edge, 2 pt apart; a photo fills its square from
    /// the middle of its long side.
    #[test]
    fn phone_tiles_run_edge_to_edge_three_across() {
        let w = 390.0;
        let l = layout(&[1.5; 7], &[], w, ((w - 2.0 * TILE_GAP) / 3.0).floor(), true, true);
        assert_eq!(l.cells[0].left(), 0.0);
        assert_eq!(l.cells[0].top(), l.cells[2].top());
        assert!((w - l.cells[2].right()).abs() < 3.0, "{:?}", l.cells[2]);
        assert_eq!(l.cells[3].left(), 0.0);
        assert!((l.cells[3].top() - l.cells[0].bottom() - TILE_GAP).abs() < 0.01);
        assert!((l.cells[1].left() - l.cells[0].right() - TILE_GAP).abs() < 0.01);
        let wide = cover_uv(300.0, 200.0);
        assert!((wide.width() - 2.0 / 3.0).abs() < 1e-6 && wide.height() == 1.0 && (wide.center().x - 0.5).abs() < 1e-6);
        let tall = cover_uv(200.0, 400.0);
        assert!(tall.width() == 1.0 && (tall.height() - 0.5).abs() < 1e-6 && (tall.min.y - 0.25).abs() < 1e-6);
    }

    #[test]
    fn square_grid_groups_and_auto_levels() {
        let l = layout(&[1.0; 5], &[(0, 3), (3, 2)], 400.0, 100.0, true, false);
        // 4 columns: group 1 fills 3 cells of a row, group 2 starts a new row
        assert_eq!(l.cells[0].top(), l.cells[2].top());
        assert_eq!(l.cells[3].left(), 4.0);
        assert!(l.cells[3].top() > l.cells[0].bottom() + HEADER_H - 1.0);
        assert!((l.cells[0].width() - l.cells[0].height()).abs() < 0.01);
        assert_eq!(resolve_group(GroupBy::Auto, 220.0), GroupBy::Day);
        assert_eq!(resolve_group(GroupBy::Auto, 120.0), GroupBy::Month);
        assert_eq!(resolve_group(GroupBy::Auto, 90.0), GroupBy::Year);
        assert_eq!(resolve_group(GroupBy::None, 220.0), GroupBy::None);
        assert_eq!(resolve_group(GroupBy::Year, 400.0), GroupBy::Year);
    }
}
