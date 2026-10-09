//! Map: the visible photos where they were taken. Photos that would overlap group into one marker
//! (a thumbnail with a count); the map pans, zooms toward the cursor or the pinch, and finds
//! places by name from the offline gazetteer.
//!
//! The background is, from the bottom: ocean and coastlines built into the app (always there, so
//! the map is never blank and works offline), then map tiles from the server in Settings → Map as
//! they arrive (a coarser tile stands in while a sharp one loads). The host fetches the tiles off
//! the UI thread ([`crate::Services::tile_exec`]); without one the built-in map is all there is.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use egui::epaint::{Mesh, Vertex};
use egui::{Align2, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, StrokeKind, TextureHandle, pos2, vec2};
use lightcraft_catalog::PhotoId;
use lightcraft_engine::tiles::{TileRequest, TileResult};
use lightcraft_geo::cluster::{Cluster, Point};
use lightcraft_geo::geodesy::{World, project, unproject};
use lightcraft_geo::tiles::{MAX_ZOOM, TileId, Viewport};
use serde_json::json;

use crate::LightcraftApp;
use crate::icons::{Icon, paint};
use crate::theme::Tokens;
use crate::widgets::register;

/// Runs one tile request off the UI thread and sends the answer back (then repaints).
pub type TileExec = Box<dyn Fn(TileRequest, Sender<TileResult>, egui::Context)>;

/// The native transport: a worker thread per request ([`lightcraft_engine::tiles::load`]); the
/// view keeps only a few in flight.
#[cfg(not(target_arch = "wasm32"))]
pub fn native_exec() -> TileExec {
    Box::new(|req, tx, ctx| {
        let id = req.id;
        let failed = tx.clone();
        let spawned = std::thread::Builder::new().name("lc-tile".into()).spawn(move || {
            let _ = tx.send(lightcraft_engine::tiles::load(&req));
            ctx.request_repaint();
        });
        if let Err(e) = spawned {
            let _ = failed.send(TileResult { id, tile: Err(format!("can't start a tile thread: {e}")) });
        }
    })
}

/// Tiles requested and not answered yet, at most (tile servers ask for few connections).
const MAX_IN_FLIGHT: usize = 4;
/// Tile textures kept (256 × 256 × 4 bytes each).
const MAX_TEXTURES: usize = 160;
/// How long a failed tile waits before it is asked for again (seconds).
const RETRY_SECS: f64 = 20.0;
/// Edge of a photo marker (points).
const MARKER: f32 = 46.0;
/// Photos closer than this on screen share a marker (points).
const CLUSTER_PX: f64 = 64.0;
const HEADER_H: f32 = 44.0;
const OCEAN: Color32 = Color32::from_rgb(170, 211, 223);
const LAND: Color32 = Color32::from_rgb(242, 239, 233);

enum Slot {
    Pending,
    Ready { tex: TextureHandle, used: u64 },
    Failed { at: f64 },
}

/// The photos of the visible list that have a position, per list generation.
struct Located {
    generation: u64,
    points: Arc<Vec<Point>>,
    /// Photos of the list without a position.
    without: usize,
}

struct Markers {
    key: (u64, i32),
    list: Arc<Vec<Cluster>>,
}

/// What the map did, for tests and `ui.inspect`.
#[derive(Clone, Debug, Default)]
pub struct MapStats {
    pub tiles_drawn: usize,
    pub stand_ins: usize,
    pub markers_drawn: usize,
    pub tiles_requested: usize,
    pub tiles_loaded: usize,
    pub tiles_failed: usize,
}

pub struct MapState {
    view: Option<Viewport>,
    /// Where an animation (Fit, a place found) is heading: centre and zoom.
    target: Option<(World, f64)>,
    tiles: HashMap<TileId, Slot>,
    clock: u64,
    in_flight: usize,
    chan: (Sender<TileResult>, Receiver<TileResult>),
    located: Option<Located>,
    markers: Option<Markers>,
    /// The (source, filter) the view was last fitted to: a different one refits.
    fitted_for: Option<u64>,
    pub search: String,
    /// Consecutive failures since the last tile that arrived.
    failures: u32,
    pub stats: MapStats,
    /// Tiles drawn from the server last frame, to tell the user whose map they are looking at.
    online_tiles: bool,
}

impl Default for MapState {
    fn default() -> Self {
        MapState {
            view: None,
            target: None,
            tiles: HashMap::new(),
            clock: 0,
            in_flight: 0,
            chan: channel(),
            located: None,
            markers: None,
            fitted_for: None,
            search: String::new(),
            failures: 0,
            stats: MapStats::default(),
            online_tiles: false,
        }
    }
}

impl MapState {
    /// The viewport's centre and zoom (for the saved UI state and `ui.inspect`).
    pub fn camera(&self) -> Option<(f64, f64, f64)> {
        self.view.map(|v| {
            let (lat, lon) = unproject(v.center);
            (lat, lon, v.zoom)
        })
    }

    /// The visible photos that have a position, as of the last frame.
    pub fn located_points(&self) -> &[Point] {
        self.located.as_ref().map_or(&[], |l| l.points.as_slice())
    }

    /// Point the map at a position (the next frame goes there: `animate` eases, else it jumps).
    pub fn look_at(&mut self, lat: f64, lon: f64, zoom: f64, animate: bool) {
        let c = project(lat, lon);
        match (&mut self.view, animate) {
            (Some(v), false) => {
                v.center = c;
                v.zoom = zoom;
                v.clamp();
                self.target = None;
            }
            (None, false) => {
                self.view = Some(Viewport::new(c, zoom, 800.0, 600.0));
            }
            (_, true) => self.target = Some((c, zoom)),
        }
    }

    /// Fit the map to a box of positions: eased over the next frames, or at once.
    pub fn fit(&mut self, min: (f64, f64), max: (f64, f64), animate: bool) {
        let mut v = self.view.unwrap_or_else(|| Viewport::world(800.0, 600.0));
        v.fit(min, max, 70.0, 15.0);
        if animate && self.view.is_some() {
            self.target = Some((v.center, v.zoom));
        } else {
            self.view = Some(v);
            self.target = None;
        }
    }

    /// Forget which filter the view was fitted for (it refits on the next frame).
    pub fn refit_next(&mut self) {
        self.fitted_for = None;
    }
}

/// "1 234" photos in a marker: 999, then 1.2k.
pub fn count_label(n: usize) -> String {
    match n {
        0..=999 => n.to_string(),
        1000..=9999 => format!("{:.1}k", n as f64 / 1000.0),
        _ => format!("{}k", n / 1000),
    }
}

/// The part of a tile (as a fraction of it) that the ancestor `levels` up covers: where `id`'s
/// pixels are inside the ancestor's texture.
fn ancestor_uv(id: TileId, levels: u8) -> (TileId, Rect) {
    let anc = TileId { z: id.z - levels, x: id.x >> levels, y: id.y >> levels };
    let n = (1u32 << levels) as f32;
    let (fx, fy) = ((id.x - (anc.x << levels)) as f32, (id.y - (anc.y << levels)) as f32);
    (anc, Rect::from_min_max(pos2(fx / n, fy / n), pos2((fx + 1.0) / n, (fy + 1.0) / n)))
}

fn tile_template(app: &LightcraftApp) -> String {
    let custom = app.ui.settings.map_tile_url.trim();
    if lightcraft_geo::tiles::valid_template(custom) { custom.to_string() } else { lightcraft_engine::tiles::DEFAULT_TILE_URL.to_string() }
}

fn using_default_server(app: &LightcraftApp) -> bool {
    tile_template(app) == lightcraft_engine::tiles::DEFAULT_TILE_URL
}

/// Photos at least roughly where `v` looks: those whose position is on screen.
fn on_screen(v: &Viewport, p: &Point) -> bool {
    let (x, y) = v.locate(p.lat, p.lon);
    x >= -40.0 && y >= -40.0 && x <= v.width + 40.0 && y <= v.height + 40.0
}

pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let now = ui.input(|i| i.time);
    poll_tiles(app, ui.ctx(), now);
    refresh_points(app);

    // the filters narrowing the list, removable here
    let chips = lightcraft_engine::filter_chips(&app.session.filter, &app.session.catalog);
    let chips = super::chips::with_understanding(app, chips);
    header(app, ui, &t);
    super::chips::show(app, ui, &chips);

    let rect = ui.available_rect_before_wrap();
    if rect.width() < 40.0 || rect.height() < 40.0 {
        return;
    }
    let resp = ui.allocate_rect(rect, Sense::click_and_drag());
    register(ui.ctx(), "map", rect);
    app.canvas_rect = Some(rect);

    // the viewport follows the panel's size
    let (w, h) = (f64::from(rect.width()), f64::from(rect.height()));
    let mut v = app.map.view.unwrap_or_else(|| initial_view(app, w, h));
    if (v.width - w).abs() > 0.5 || (v.height - h).abs() > 0.5 {
        let keep = (v.center, v.zoom);
        v = Viewport::new(keep.0, keep.1, w, h);
    }
    fit_if_filter_changed(app, &mut v);
    ease_to_target(app, &mut v, ui.ctx());
    handle_input(app, ui, &resp, rect, &mut v);
    app.map.view = Some(v);
    if let Some((lat, lon, zoom)) = app.map.camera() {
        app.ui.map_view = Some((lat, lon, zoom));
    }

    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, OCEAN);
    draw_land(&painter, rect, &v);
    draw_tiles(app, ui, &painter, rect, &v, now);
    draw_markers(app, ui, rect, &v);
    overlays(app, ui, rect, &v);
}

/// Where to look when there is no saved view: all the located photos (or the world).
fn initial_view(app: &mut LightcraftApp, w: f64, h: f64) -> Viewport {
    let mut v = Viewport::world(w, h);
    if let Some((lat, lon, zoom)) = app.ui.map_view.filter(|(la, lo, z)| lightcraft_geo::geodesy::valid(*la, *lo) && z.is_finite()) {
        v.look_at(lat, lon, Some(zoom));
        app.map.fitted_for = Some(filter_key(app));
        return v;
    }
    if let Some(b) = app.map.located.as_ref().and_then(|l| lightcraft_engine::map::bounds(&l.points)) {
        v.fit(b.0, b.1, 70.0, 15.0);
    }
    app.map.fitted_for = Some(filter_key(app));
    v
}

/// A number that changes when the source or the filter does (not when a photo is rated).
fn filter_key(app: &LightcraftApp) -> u64 {
    crate::key_of(format!("{:?}|{:?}", app.session.source, app.session.filter))
}

/// A new search or filter shows its own photos: fit them.
fn fit_if_filter_changed(app: &mut LightcraftApp, v: &mut Viewport) {
    let key = filter_key(app);
    if app.map.fitted_for == Some(key) {
        return;
    }
    app.map.fitted_for = Some(key);
    if let Some(b) = app.map.located.as_ref().and_then(|l| lightcraft_engine::map::bounds(&l.points)) {
        let mut f = *v;
        f.fit(b.0, b.1, 70.0, 15.0);
        app.map.target = Some((f.center, f.zoom));
    }
}

fn ease_to_target(app: &mut LightcraftApp, v: &mut Viewport, ctx: &egui::Context) {
    let Some((c, z)) = app.map.target else { return };
    let k = 0.22;
    v.center = World { x: v.center.x + (c.x - v.center.x) * k, y: v.center.y + (c.y - v.center.y) * k };
    v.zoom += (z - v.zoom) * k;
    v.clamp();
    let close =
        (v.zoom - z).abs() < 0.01 && (v.center.x - c.x).abs() * v.pixels_per_world() < 0.5 && (v.center.y - c.y).abs() * v.pixels_per_world() < 0.5;
    if close {
        v.center = c;
        v.zoom = z;
        v.clamp();
        app.map.target = None;
    }
    ctx.request_repaint();
}

fn handle_input(app: &mut LightcraftApp, ui: &egui::Ui, resp: &egui::Response, rect: Rect, v: &mut Viewport) {
    let hover = resp.hover_pos().or_else(|| ui.input(|i| i.pointer.hover_pos())).filter(|p| rect.contains(*p));
    let local = |p: Pos2| ((p.x - rect.left()) as f64, (p.y - rect.top()) as f64);
    if resp.dragged() {
        let d = resp.drag_delta();
        if d != egui::Vec2::ZERO {
            app.map.target = None;
            v.pan(f64::from(d.x), f64::from(d.y));
        }
    }
    if resp.hovered() || resp.dragged() {
        let (scroll, pinch) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
        let dz = f64::from(scroll) * 0.004 + f64::from(pinch).max(1e-3).log2();
        if dz.abs() > 1e-6 {
            app.map.target = None;
            let (ax, ay) = hover.map_or((v.width / 2.0, v.height / 2.0), local);
            v.zoom_about(ax, ay, dz);
        }
    }
    if resp.double_clicked()
        && let Some(p) = resp.interact_pointer_pos()
    {
        let (ax, ay) = local(p);
        let before = v.to_world(ax, ay);
        let mut z = *v;
        z.zoom_about(ax, ay, 1.0);
        app.map.target = Some((z.center, z.zoom));
        let _ = before;
    }
    // keys: + and - zoom, 0 fits all the photos
    if resp.hovered() || resp.has_focus() {
        let (plus, minus) = ui.input(|i| (i.key_pressed(egui::Key::Plus) || i.key_pressed(egui::Key::Equals), i.key_pressed(egui::Key::Minus)));
        if plus {
            v.zoom_about(v.width / 2.0, v.height / 2.0, 1.0);
        }
        if minus {
            v.zoom_about(v.width / 2.0, v.height / 2.0, -1.0);
        }
    }
}

// ---- data ---------------------------------------------------------------------------------

fn refresh_points(app: &mut LightcraftApp) {
    let (generation, ids) = app.session.visible_shared();
    if app.map.located.as_ref().is_some_and(|l| l.generation == generation) {
        return;
    }
    let points = lightcraft_engine::map::points(&app.session.catalog, &ids);
    let without = ids.len() - points.len();
    app.map.located = Some(Located { generation, points: Arc::new(points), without });
    app.map.markers = None;
}

fn markers_for(app: &mut LightcraftApp, zoom: f64) -> Arc<Vec<Cluster>> {
    let Some(loc) = app.map.located.as_ref() else { return Arc::new(vec![]) };
    // clustering is computed for quarter zooms: panning and small zoom steps reuse it
    let key = (loc.generation, (zoom * 4.0).round() as i32);
    if let Some(m) = app.map.markers.as_ref().filter(|m| m.key == key) {
        return m.list.clone();
    }
    let list = Arc::new(lightcraft_engine::map::markers(&loc.points, f64::from(key.1) / 4.0, CLUSTER_PX));
    app.map.markers = Some(Markers { key, list: list.clone() });
    list
}

// ---- tiles --------------------------------------------------------------------------------

fn poll_tiles(app: &mut LightcraftApp, ctx: &egui::Context, now: f64) {
    while let Ok(r) = app.map.chan.1.try_recv() {
        app.map.in_flight = app.map.in_flight.saturating_sub(1);
        match r.tile {
            Ok(tile) => {
                app.map.failures = 0;
                app.map.stats.tiles_loaded += 1;
                let image = egui::ColorImage::from_rgba_unmultiplied([tile.width as usize, tile.height as usize], &tile.rgba);
                let tex = ctx.load_texture(format!("map-tile-{}-{}-{}", r.id.z, r.id.x, r.id.y), Arc::new(image), egui::TextureOptions::LINEAR);
                app.map.clock += 1;
                app.map.tiles.insert(r.id, Slot::Ready { tex, used: app.map.clock });
            }
            Err(e) => {
                log::debug!("map tile {:?}: {e}", r.id);
                app.map.failures += 1;
                app.map.stats.tiles_failed += 1;
                app.map.tiles.insert(r.id, Slot::Failed { at: now });
            }
        }
    }
}

fn request_tiles(app: &mut LightcraftApp, ctx: &egui::Context, want: Vec<TileId>) {
    let Some(exec) = app.services.tile_exec.as_ref() else { return };
    let template = tile_template(app);
    let dir = app.session.library.as_ref().filter(|l| l.on_disk).map(|l| l.tiles_dir());
    let online = app.ui.settings.map_online;
    for id in want {
        if app.map.in_flight >= MAX_IN_FLIGHT {
            break;
        }
        app.map.in_flight += 1;
        app.map.tiles.insert(id, Slot::Pending);
        app.map.stats.tiles_requested += 1;
        let req = TileRequest { id, url: id.url(&template), cache: dir.as_deref().map(|d| lightcraft_engine::tiles::cache_path(d, id)), online };
        exec(req, app.map.chan.0.clone(), ctx.clone());
    }
}

fn evict_tiles(app: &mut LightcraftApp, keep: &[TileId]) {
    let ready = app.map.tiles.values().filter(|s| matches!(s, Slot::Ready { .. })).count();
    if ready <= MAX_TEXTURES {
        return;
    }
    let mut old: Vec<(u64, TileId)> = app
        .map
        .tiles
        .iter()
        .filter_map(|(id, s)| if let Slot::Ready { used, .. } = s { (!keep.contains(id)).then_some((*used, *id)) } else { None })
        .collect();
    old.sort();
    for (_, id) in old.into_iter().take(ready - MAX_TEXTURES) {
        app.map.tiles.remove(&id);
    }
}

fn draw_tiles(app: &mut LightcraftApp, ui: &egui::Ui, painter: &egui::Painter, rect: Rect, v: &Viewport, now: f64) {
    let ctx = ui.ctx().clone();
    app.map.stats.tiles_drawn = 0;
    app.map.stats.stand_ins = 0;
    let level = v.tile_level();
    let visible = v.visible_tiles(level);
    let ppw = v.pixels_per_world();
    let mut want: Vec<TileId> = Vec::new();
    let mut drawn_online = false;
    let uv_all = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    for id in &visible {
        let (o, size) = id.world();
        let (x0, y0) = v.to_screen(o);
        let edge = (size * ppw) as f32;
        // a hair of overlap hides seams between tiles
        let r = Rect::from_min_size(pos2(rect.left() + x0 as f32 - 0.25, rect.top() + y0 as f32 - 0.25), vec2(edge + 0.5, edge + 0.5));
        let state = app.map.tiles.get(id);
        let mut shown = false;
        if let Some(Slot::Ready { tex, .. }) = state {
            painter.image(tex.id(), r, uv_all, Color32::WHITE);
            app.map.stats.tiles_drawn += 1;
            shown = true;
        }
        let retry = matches!(state, Some(Slot::Failed { at }) if now - at > RETRY_SECS);
        if state.is_none() || retry {
            want.push(*id);
        }
        if shown {
            drawn_online = true;
            continue;
        }
        // a coarser tile that is loaded stands in (blurry, but never a hole)
        for up in 1..=4u8.min(id.z) {
            let (anc, uv) = ancestor_uv(*id, up);
            if let Some(Slot::Ready { tex, .. }) = app.map.tiles.get(&anc) {
                painter.image(tex.id(), r, uv, Color32::WHITE);
                app.map.stats.stand_ins += 1;
                drawn_online = true;
                break;
            }
        }
    }
    app.map.online_tiles = drawn_online;
    // touch what is on screen so it is the last to go
    app.map.clock += 1;
    let clock = app.map.clock;
    for id in &visible {
        if let Some(Slot::Ready { used, .. }) = app.map.tiles.get_mut(id) {
            *used = clock;
        }
    }
    // the nearest to the centre first
    let c = v.center;
    want.sort_by(|a, b| {
        let d = |t: &TileId| {
            let (o, s) = t.world();
            (o.x + s / 2.0 - c.x).powi(2) + (o.y + s / 2.0 - c.y).powi(2)
        };
        d(a).total_cmp(&d(b))
    });
    // while the view is flying somewhere nothing is asked for: the in-between views are never looked at
    if app.map.target.is_none() {
        request_tiles(app, &ctx, want);
    }
    evict_tiles(app, &visible);
    if app.map.in_flight > 0 {
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }
}

/// The coastlines built into the app, under the tiles.
fn draw_land(painter: &egui::Painter, rect: Rect, v: &Viewport) {
    let land = lightcraft_geo::land::Land::global();
    let mut mesh = Mesh::default();
    let (o, ppw) = (rect.min, v.pixels_per_world());
    for &(lat, lon) in &land.vertices {
        let w = project(f64::from(lat), f64::from(lon));
        let (x, y) = (v.width / 2.0 + (w.x - v.center.x) * ppw, v.height / 2.0 + (w.y - v.center.y) * ppw);
        mesh.vertices.push(Vertex { pos: o + vec2(x as f32, y as f32), uv: egui::epaint::WHITE_UV, color: LAND });
    }
    mesh.indices = land.indices.iter().map(|i| u32::from(*i)).collect();
    painter.add(mesh);
}

// ---- header and overlays --------------------------------------------------------------------

fn header(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), HEADER_H), Sense::hover());
    ui.painter().rect_filled(bar, 0.0, t.grid_bg);
    let p = ui.painter();
    p.text(pos2(bar.left() + 20.0, bar.center().y), Align2::LEFT_CENTER, crate::i18n::tr("Map"), t.semibold(15.0), t.text);
    let (located, without) = app.map.located.as_ref().map_or((0, 0), |l| (l.points.len(), l.without));
    // on a phone there is room for the count only
    let narrow = bar.width() < 640.0;
    let summary = if narrow {
        located.to_string()
    } else if without == 0 {
        format!("{}: {located}", crate::i18n::tr("On the map"))
    } else {
        format!("{}: {located} · {}: {without}", crate::i18n::tr("On the map"), crate::i18n::tr("No location"))
    };
    let title_w = p.layout_no_wrap(crate::i18n::tr("Map").to_string(), t.semibold(15.0), t.text).size().x;
    p.text(pos2(bar.left() + 32.0 + title_w, bar.center().y), Align2::LEFT_CENTER, summary, t.font(12.5), t.text_dim);

    // place search, right-aligned: type a place, Enter goes there
    let w = 260.0f32.min(bar.width() * 0.45);
    let sr = Rect::from_min_size(pos2(bar.right() - w - 16.0, bar.center().y - 14.0), vec2(w, 28.0));
    ui.painter().rect(sr, 14.0, t.field, Stroke::new(1.0, t.field_border), StrokeKind::Inside);
    let mut child =
        ui.new_child(egui::UiBuilder::new().max_rect(sr.shrink2(vec2(34.0, 2.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
    let id = egui::Id::new("map-search");
    let edit = child.add(
        egui::TextEdit::singleline(&mut app.map.search)
            .id(id)
            .frame(egui::Frame::NONE)
            .hint_text(crate::i18n::tr("Go to a place"))
            .desired_width(sr.width() - 44.0)
            .font(t.font(13.0))
            .text_color(t.text),
    );
    register(ui.ctx(), "map:search", sr);
    paint(ui.painter(), Rect::from_min_size(pos2(sr.left() + 10.0, sr.center().y - 7.0), vec2(14.0, 14.0)), Icon::Search, t.text_dim);
    let hits = if app.map.search.trim().chars().count() >= 2 { place_hits(&app.map.search) } else { vec![] };
    if edit.has_focus() && !hits.is_empty() {
        let mut go = None;
        egui::Area::new(egui::Id::new("map-search-hits")).order(egui::Order::Foreground).fixed_pos(pos2(sr.left(), sr.bottom() + 4.0)).show(
            ui.ctx(),
            |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_min_width(sr.width());
                    for h in &hits {
                        if ui.selectable_label(false, &h.label).clicked() {
                            go = Some(h.clone());
                        }
                    }
                });
            },
        );
        if go.is_none() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            go = hits.first().cloned();
        }
        if let Some(h) = go {
            app.map.fit(h.min, h.max, true);
            app.map.search.clear();
            edit.surrender_focus();
        }
    }
}

#[derive(Clone)]
struct PlaceHit {
    label: String,
    min: (f64, f64),
    max: (f64, f64),
}

fn place_hits(text: &str) -> Vec<PlaceHit> {
    let g = lightcraft_geo::Gazetteer::global();
    let mut ids = g.lookup_text(text);
    ids.sort_by_key(|i| std::cmp::Reverse(g.importance(*i)));
    ids.into_iter()
        .take(6)
        .filter_map(|id| {
            let (min, max) = g.view_of(id)?;
            Some(PlaceHit { label: g.label(id)?, min, max })
        })
        .collect()
}

fn overlays(app: &mut LightcraftApp, ui: &mut egui::Ui, rect: Rect, v: &Viewport) {
    let t = Tokens::get(ui.ctx());
    let ctx = ui.ctx().clone();
    let pill = |ui: &egui::Ui, r: Rect| {
        ui.painter().rect_filled(r, 4.0, Color32::from_white_alpha(215));
    };
    // attribution: the licence of the map being shown
    let (text, url) = if app.map.online_tiles && using_default_server(app) {
        (lightcraft_engine::tiles::DEFAULT_ATTRIBUTION, Some(lightcraft_engine::tiles::ATTRIBUTION_URL))
    } else if app.map.online_tiles {
        ("Map tiles from the server in Settings", None)
    } else {
        ("Built-in map: Natural Earth", None)
    };
    let font = t.font(11.0);
    let g = ui.painter().layout_no_wrap(text.to_string(), font, Color32::from_gray(40));
    let r = Rect::from_min_size(pos2(rect.right() - g.size().x - 16.0, rect.bottom() - 22.0), vec2(g.size().x + 10.0, 18.0));
    pill(ui, r);
    ui.painter().galley(r.min + vec2(5.0, 2.0), g, Color32::from_gray(40));
    register(&ctx, "map:attribution", r);
    if let Some(url) = url {
        let resp = ui.interact(r, egui::Id::new("map-attribution"), Sense::click()).on_hover_text(url);
        if resp.clicked()
            && let Some(open) = app.services.open_url.as_mut()
        {
            let _ = open(url);
        }
    }
    // a hint when the server can't be reached
    let offline = !app.ui.settings.map_online;
    let unreachable = app.map.failures >= 3 && app.services.tile_exec.is_some();
    if (offline || unreachable) && app.services.tile_exec.is_some() {
        let msg = if offline {
            "Offline — showing the built-in map and saved tiles"
        } else {
            "Can't reach the map server — showing the built-in map and saved tiles"
        };
        let g = ui.painter().layout_no_wrap(crate::i18n::tr(msg).to_string(), t.font(12.0), t.text);
        let r = Rect::from_min_size(pos2(rect.left() + 12.0, rect.top() + 12.0), vec2(g.size().x + 16.0, 24.0));
        ui.painter().rect(r, 12.0, t.chrome.gamma_multiply(0.92), Stroke::new(1.0, t.button_border), StrokeKind::Inside);
        ui.painter().galley(r.min + vec2(8.0, 4.0), g, t.text);
    }
    // zoom buttons and "fit all"
    let col = Rect::from_min_size(pos2(rect.right() - 44.0, rect.top() + 12.0), vec2(32.0, 100.0));
    ui.painter().rect(col, 6.0, t.chrome.gamma_multiply(0.92), Stroke::new(1.0, t.button_border), StrokeKind::Inside);
    let mut next = *v;
    let mut changed = false;
    for (i, (label, id)) in [("+", "map:zoomIn"), ("−", "map:zoomOut"), ("", "map:fit")].into_iter().enumerate() {
        let b = Rect::from_min_size(col.min + vec2(0.0, i as f32 * 33.0), vec2(32.0, 33.0));
        let resp = ui.interact(b, egui::Id::new(id), Sense::click());
        register(ui.ctx(), id, b);
        let c = if resp.hovered() { t.text } else { t.text_label };
        if id == "map:fit" {
            fit_glyph(ui.painter(), b.center(), c);
        } else {
            ui.painter().text(b.center(), Align2::CENTER_CENTER, label, t.semibold(16.0), c);
        }
        let tip = match id {
            "map:zoomIn" => "Zoom in",
            "map:zoomOut" => "Zoom out",
            _ => "Show all photos",
        };
        if resp.on_hover_text(crate::i18n::tr(tip)).clicked() {
            match id {
                "map:zoomIn" => next.zoom_about(next.width / 2.0, next.height / 2.0, 1.0),
                "map:zoomOut" => next.zoom_about(next.width / 2.0, next.height / 2.0, -1.0),
                _ => {
                    if let Some(b) = app.map.located.as_ref().and_then(|l| lightcraft_engine::map::bounds(&l.points)) {
                        next.fit(b.0, b.1, 70.0, 15.0);
                    }
                }
            }
            changed = true;
        }
    }
    if changed {
        app.map.target = Some((next.center, next.zoom));
    }
    // empty map
    if app.map.located.as_ref().is_some_and(|l| l.points.is_empty()) {
        let located_anywhere = app.session.catalog.photos().any(|p| p.meta.gps.is_some());
        let (title, body) = if app.session.filter != Default::default() && located_anywhere {
            ("No photos with a location match", "Remove a filter above, or choose Clear all")
        } else {
            ("No photos with a location yet", "Photos taken with GPS on show here. Photo ▸ Auto-Tag from Tracklog… adds locations from a GPX file.")
        };
        let c = Rect::from_center_size(rect.center(), vec2(420.0, 70.0));
        ui.painter().rect_filled(c, 8.0, t.chrome.gamma_multiply(0.92));
        super::empty_message(ui, c, title, body);
    }
}

/// "Show everything": four corner brackets drawn round a point (no font has the right arrows).
fn fit_glyph(p: &egui::Painter, c: Pos2, color: Color32) {
    let stroke = Stroke::new(1.6, color);
    for (sx, sy) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
        let corner = c + vec2(sx * 6.0, sy * 6.0);
        p.line_segment([corner, corner - vec2(sx * 4.0, 0.0)], stroke);
        p.line_segment([corner, corner - vec2(0.0, sy * 4.0)], stroke);
    }
}

// ---- markers ------------------------------------------------------------------------------

fn draw_markers(app: &mut LightcraftApp, ui: &mut egui::Ui, rect: Rect, v: &Viewport) {
    let t = Tokens::get(ui.ctx());
    let markers = markers_for(app, v.zoom);
    let Some(points) = app.map.located.as_ref().map(|l| l.points.clone()) else { return };
    let ppp = ui.ctx().pixels_per_point();
    let mut hovered: Option<(Rect, usize)> = None;
    let mut drawn = 0;
    // the selected photos, so their markers show it
    let selected: std::collections::HashSet<u64> = app.session.selection.ids.iter().map(|i| i.0).collect();
    let active = app.session.selection.active.map(|i| i.0);
    for (ci, c) in markers.iter().enumerate() {
        let (x, y) = v.to_screen(c.at);
        let centre = pos2(rect.left() + x as f32, rect.top() + y as f32);
        let r = Rect::from_center_size(centre, vec2(MARKER, MARKER));
        if !rect.expand(MARKER).contains(centre) {
            continue;
        }
        let front = points.get(c.front as usize).map(|p| PhotoId(p.id));
        let Some(front) = front else { continue };
        drawn += 1;
        let n = c.count();
        let any_selected = c.members.iter().any(|i| points.get(*i as usize).is_some_and(|p| selected.contains(&p.id)));
        let resp = ui.interact(r, egui::Id::new(("map-marker", front.0, n)), Sense::click());
        register(ui.ctx(), format!("marker:{}", front.0), r);
        if resp.hovered() {
            hovered = Some((r, ci));
        }
        let p = ui.painter();
        // a stack: frames behind the front one
        if n > 1 {
            for off in [6.0f32, 3.0] {
                let b = r.translate(vec2(off, -off));
                p.rect(b, 5.0, Color32::from_gray(235), Stroke::new(1.0, Color32::from_gray(150)), StrokeKind::Inside);
            }
        }
        p.rect_filled(r.translate(vec2(0.0, 2.0)).expand(1.0), 6.0, Color32::from_black_alpha(70));
        super::grid::request_thumb(app, front, super::grid::thumb_px(MARKER, ppp), 8);
        let p = ui.painter();
        p.rect_filled(r, 5.0, t.cell);
        if let Some(tex) = app.renderer.thumb(front) {
            let [tw, th] = tex.size;
            p.image(tex.tex.id(), r.shrink(1.5), super::grid::cover_uv(tw as f32, th as f32), Color32::WHITE);
        } else {
            paint(p, r.shrink(12.0), Icon::Pin, t.text_dim);
        }
        let ring = if any_selected && active.is_some_and(|a| c.members.iter().any(|i| points.get(*i as usize).is_some_and(|p| p.id == a))) {
            Stroke::new(3.0, t.accent)
        } else if any_selected {
            Stroke::new(2.0, t.accent)
        } else {
            Stroke::new(2.0, Color32::WHITE)
        };
        p.rect_stroke(r, 5.0, ring, StrokeKind::Inside);
        if n > 1 {
            let label = count_label(n);
            let g = p.layout_no_wrap(label, t.semibold(11.0), Color32::WHITE);
            let w = (g.size().x + 10.0).max(20.0);
            let b = Rect::from_center_size(r.right_top() + vec2(-2.0, 2.0), vec2(w, 18.0));
            p.rect(b, CornerRadius::same(9), t.accent, Stroke::new(1.5, Color32::WHITE), StrokeKind::Inside);
            p.galley(pos2(b.center().x - g.size().x / 2.0, b.center().y - g.size().y / 2.0), g, Color32::WHITE);
        }
        marker_actions(app, ui, &resp, v, c, &points, front);
    }
    app.map.stats.markers_drawn = drawn;
    if let Some((r, ci)) = hovered
        && let Some(c) = markers.get(ci)
    {
        let n = c.count();
        let (lat, lon) = unproject(c.at);
        let place = lightcraft_geo::Gazetteer::global().name_of(lat, lon).map(|p| p.display()).filter(|s| !s.is_empty());
        let mut tip = super::chips::count_text(n, None, false);
        if let Some(pl) = place {
            tip.push_str(&format!(" · {pl}"));
        }
        let g = ui.painter().layout_no_wrap(tip, t.font(12.0), t.text);
        let b = Rect::from_min_size(pos2(r.center().x - g.size().x / 2.0 - 8.0, r.top() - 30.0), vec2(g.size().x + 16.0, 22.0));
        ui.painter().rect(b, 4.0, t.chrome, Stroke::new(1.0, t.button_border), StrokeKind::Inside);
        ui.painter().galley(b.min + vec2(8.0, 3.0), g, t.text);
    }
}

/// What clicking a marker does: a photo is selected (double-click: opened); a group zooms in to
/// show what is in it — or, when it can't split any further, selects it. The context menu does
/// the rest.
fn marker_actions(app: &mut LightcraftApp, ui: &egui::Ui, resp: &egui::Response, v: &Viewport, c: &Cluster, points: &[Point], front: PhotoId) {
    let ids: Vec<u64> = c.members.iter().filter_map(|i| points.get(*i as usize).map(|p| p.id)).collect();
    let n = ids.len();
    let mode = if ui.input(|i| i.modifiers.command || i.modifiers.shift) { "add" } else { "replace" };
    if resp.double_clicked() && n == 1 {
        let _ = app.run("library.select", json!({"ids": ids, "mode": "replace"}));
        let _ = app.run("view.detail", json!({}));
    } else if resp.clicked() {
        if n == 1 || v.zoom >= f64::from(MAX_ZOOM) - 2.5 {
            let _ = app.run("library.select", json!({"ids": ids, "active": front.0, "mode": mode}));
        } else {
            let (a, b) = (unproject(c.bounds.0), unproject(c.bounds.1));
            let mut z = *v;
            z.fit((a.0.min(b.0), a.1.min(b.1)), (a.0.max(b.0), a.1.max(b.1)), 110.0, f64::from(MAX_ZOOM) - 2.0);
            app.map.target = Some((z.center, z.zoom.max(v.zoom + 1.0).min(f64::from(MAX_ZOOM))));
        }
    }
    resp.context_menu(|ui| {
        if n > 1 && ui.button(crate::i18n::tr("Zoom to these photos")).clicked() {
            let (a, b) = (unproject(c.bounds.0), unproject(c.bounds.1));
            let mut z = *v;
            z.fit((a.0.min(b.0), a.1.min(b.1)), (a.0.max(b.0), a.1.max(b.1)), 110.0, f64::from(MAX_ZOOM) - 2.0);
            app.map.target = Some((z.center, z.zoom));
            ui.close();
        }
        let label = crate::i18n::tr(if n == 1 { "Select this photo" } else { "Select these photos" });
        if ui.button(label).clicked() {
            let _ = app.run("library.select", json!({"ids": ids, "active": front.0, "mode": "replace"}));
            ui.close();
        }
        let label = crate::i18n::tr(if n == 1 { "Show this photo in Grid" } else { "Show these photos in Grid" });
        if ui.button(label).clicked() {
            show_in_grid(app, &ids);
            ui.close();
        }
    });
}

/// `view.map {lat?, lon?, zoom?, place?, fit?}`: show the Map, optionally looking at a position
/// (zoom 12 by default), at a place by name ("madrid"), or fitted to the photos (`fit`).
pub fn open(app: &mut LightcraftApp, p: &serde_json::Value) -> Result<serde_json::Value, String> {
    let num = |k: &str| p.get(k).and_then(serde_json::Value::as_f64);
    app.ui.view = crate::state::ViewMode::Map;
    // whatever it is told to look at, the map doesn't then jump to the photos
    app.map.fitted_for = Some(filter_key(app));
    if let (Some(lat), Some(lon)) = (num("lat"), num("lon")) {
        if !lightcraft_geo::geodesy::valid(lat, lon) {
            return Err(format!("{lat}, {lon} is not a position on Earth"));
        }
        app.map.look_at(lat, lon, num("zoom").unwrap_or(12.0), false);
    } else if let Some(place) = p.get("place").and_then(serde_json::Value::as_str) {
        let hit = place_hits(place).into_iter().next().ok_or_else(|| format!("no place called `{place}`"))?;
        app.map.fit(hit.min, hit.max, false);
    } else if p.get("fit").and_then(serde_json::Value::as_bool).unwrap_or(false) {
        app.map.refit_next();
    }
    let photos = app.session.visible_cloned();
    let located = lightcraft_engine::map::points(&app.session.catalog, &photos).len();
    Ok(json!({"photos": photos.len(), "located": located, "camera": app.map.camera().map(|(la, lo, z)| json!({"lat": la, "lon": lo, "zoom": z}))}))
}

/// Narrow the library to exactly these photos and go to the grid.
pub fn show_in_grid(app: &mut LightcraftApp, ids: &[u64]) {
    let _ = app.run("library.filter", json!({"only": ids}));
    let _ = app.run("view.photoGrid", json!({}));
}

/// The photos whose markers are inside `v`.
pub fn photos_in_view(points: &[Point], v: &Viewport) -> Vec<u64> {
    points.iter().filter(|p| on_screen(v, p)).map(|p| p.id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_fit_a_badge() {
        assert_eq!(count_label(1), "1");
        assert_eq!(count_label(999), "999");
        assert_eq!(count_label(1000), "1.0k");
        assert_eq!(count_label(1234), "1.2k");
        assert_eq!(count_label(48_000), "48k");
    }

    #[test]
    fn a_coarser_tile_stands_in_for_a_missing_one() {
        let id = TileId { z: 10, x: 503, y: 387 };
        let (anc, uv) = ancestor_uv(id, 1);
        assert_eq!(anc, TileId { z: 9, x: 251, y: 193 });
        assert_eq!((uv.min.x, uv.min.y, uv.max.x, uv.max.y), (0.5, 0.5, 1.0, 1.0));
        let (anc, uv) = ancestor_uv(id, 3);
        assert_eq!(anc, TileId { z: 7, x: 62, y: 48 });
        assert!((uv.width() - 0.125).abs() < 1e-6 && (uv.min.x - 7.0 / 8.0).abs() < 1e-6 && (uv.min.y - 3.0 / 8.0).abs() < 1e-6, "{uv:?}");
        // the parent of the zoom-0 tile never asked for: z - levels can't go below 0
        let (anc, _) = ancestor_uv(TileId { z: 1, x: 1, y: 1 }, 1);
        assert_eq!(anc, TileId { z: 0, x: 0, y: 0 });
    }

    #[test]
    fn photos_in_view_are_those_on_screen() {
        let v = Viewport::new(project(40.4, -3.7), 10.0, 800.0, 600.0);
        let pts = [Point { id: 1, lat: 40.4, lon: -3.7, weight: 0 }, Point { id: 2, lat: 48.8, lon: 2.3, weight: 0 }];
        assert_eq!(photos_in_view(&pts, &v), vec![1]);
    }

    #[test]
    fn place_search_finds_by_name() {
        let h = place_hits("madr");
        assert!(h.is_empty() || h.iter().all(|x| x.label.to_lowercase().contains("madr")));
        let h = place_hits("Madrid");
        assert!(h[0].label.starts_with("Madrid, Spain"), "{:?}", h.iter().map(|x| &x.label).collect::<Vec<_>>());
        assert!(h[0].min.0 < 40.4 && h[0].max.0 > 40.4);
        assert!(place_hits("zzzzzz").is_empty());
    }
}
