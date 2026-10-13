//! Photos dragged in from outside the app (from a file manager): which album they go to, and how
//! the grid shows them until they are in the catalog.
//!
//! Where a drop goes:
//! - on an album in the sidebar, which lights up under the pointer: into that album;
//! - elsewhere in the sidebar: into the library only;
//! - anywhere else (the grid): into the album the grid shows, if it shows one.
//!
//! The sidebar can only tell which row the files are over where the host reports the pointer
//! ([`crate::Services::file_drag_pointer`]; winit sends none during such a drag). Without it every
//! drop goes to the album the grid shows, and no row lights up.
//!
//! Until a dropped photo is in the catalog the grid holds a tile for it ([`Pending`]), with a badge
//! saying where it is: waiting for its turn, or being read and added.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use egui::{Align2, Color32, Id, Pos2, Rect, Sense, Stroke, pos2, vec2};
use lightcraft_engine::LibrarySource;

use crate::LightcraftApp;
use crate::icons::{Icon, paint};
use crate::theme::Tokens;

/// Memory keys (egui temp data).
const POINTER: &str = "file-drop-pointer";
const OVER: &str = "file-drop-over";
const SIDEBAR: &str = "file-drop-sidebar";
const REVEAL: &str = "file-drop-reveal";

/// How long after a drop the grid scrolls to the tiles it shows for it (seconds).
const REVEAL_FOR: f64 = 2.0;

/// Once a frame, before a drop is handled: read where the pointer is while files hover over the
/// window (and in the frame they are dropped), and forget the album row they were over when they
/// leave without a drop.
pub fn track(app: &mut LightcraftApp, ctx: &egui::Context) {
    let (hovering, dropped) = ctx.input(|i| (!i.raw.hovered_files.is_empty(), !i.raw.dropped_files.is_empty()));
    let at = if hovering || dropped { app.services.file_drag_pointer.as_mut().and_then(|f| f()) } else { None };
    ctx.data_mut(|d| {
        d.insert_temp(Id::new(POINTER), at);
        if hovering {
            // (found again while the sidebar is drawn)
            d.insert_temp(Id::new(OVER), None::<u64>);
        } else if !dropped {
            d.remove_temp::<Option<u64>>(Id::new(OVER));
        }
    });
    if hovering {
        // the pointer moves without events
        ctx.request_repaint();
    }
}

/// The pointer while files are dragged in from outside the window, if the host tells.
pub fn pointer(ctx: &egui::Context) -> Option<Pos2> {
    ctx.data(|d| d.get_temp::<Option<Pos2>>(Id::new(POINTER))).flatten()
}

/// The sidebar was drawn at `rect`.
pub fn note_sidebar(ctx: &egui::Context, rect: Rect) {
    ctx.data_mut(|d| d.insert_temp(Id::new(SIDEBAR), rect));
}

/// The dragged files are over this album's row.
pub fn set_over(ctx: &egui::Context, album: u64) {
    ctx.data_mut(|d| d.insert_temp(Id::new(OVER), Some(album)));
}

/// The album row the dragged files were over when the last frame was drawn.
pub(crate) fn over(ctx: &egui::Context) -> Option<u64> {
    ctx.data(|d| d.get_temp::<Option<u64>>(Id::new(OVER))).flatten()
}

/// The album files dropped now go to (`None`: the library only).
pub fn destination(app: &LightcraftApp, ctx: &egui::Context) -> Option<u64> {
    let row = over(ctx);
    ctx.data_mut(|d| d.remove_temp::<Option<u64>>(Id::new(OVER)));
    if row.is_some() {
        return row;
    }
    let on_sidebar = pointer(ctx).zip(ctx.data(|d| d.get_temp::<Rect>(Id::new(SIDEBAR)))).is_some_and(|(at, sidebar)| sidebar.contains(at));
    if on_sidebar { None } else { shown_album(app) }
}

/// The album the view shows, if photos can be added to it.
fn shown_album(app: &LightcraftApp) -> Option<u64> {
    let LibrarySource::Album(id) = app.session.source else { return None };
    app.session.catalog.album(id).filter(|a| !a.folder && !a.is_smart()).map(|a| a.id.0)
}

/// A drop just started: the grid scrolls to its tiles.
pub fn reveal(ctx: &egui::Context) {
    let now = ctx.input(|i| i.time);
    ctx.data_mut(|d| d.insert_temp(Id::new(REVEAL), now));
}

fn take_reveal(ctx: &egui::Context) -> bool {
    let now = ctx.input(|i| i.time);
    ctx.data_mut(|d| d.remove_temp::<f64>(Id::new(REVEAL))).is_some_and(|at| (0.0..REVEAL_FOR).contains(&(now - at)))
}

// ------------------------------------------------------------------------------------- tiles

/// Where a dropped photo is on its way into the catalog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Found, not started.
    Waiting,
    /// Being read and added.
    Adding,
}

/// A dropped photo that isn't in the catalog yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pending {
    pub name: String,
    pub state: State,
}

#[derive(Debug, Default)]
struct Inner {
    list: VecDeque<Pending>,
    /// How many from the front have been started (they are the oldest: photos leave from the front).
    started: usize,
}

/// The dropped photos not yet in the catalog, in the order they are added. Shared by the import's
/// worker, which finds the files and reads them, and the UI thread, which draws them and adds them
/// to the catalog.
#[derive(Clone, Debug, Default)]
pub struct Pendings(Arc<Mutex<Inner>>);

impl Pendings {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// These files are what is waiting (replacing any list from before).
    pub fn set<S: AsRef<str>>(&self, paths: impl IntoIterator<Item = S>) {
        let list = paths.into_iter().map(|p| Pending { name: file_name(p.as_ref()), state: State::Waiting }).collect();
        let mut inner = self.lock();
        inner.list = list;
        inner.started = 0;
    }

    /// The next `n` are being read.
    pub fn start(&self, n: usize) {
        let mut inner = self.lock();
        let from = inner.started;
        for p in inner.list.iter_mut().skip(from).take(n) {
            p.state = State::Adding;
        }
        inner.started = from.saturating_add(n).min(inner.list.len());
    }

    /// The first `n` are in the catalog now (or turned out not to belong there).
    pub fn added(&self, n: usize) {
        let mut inner = self.lock();
        let n = n.min(inner.list.len());
        inner.list.drain(..n);
        inner.started = inner.started.saturating_sub(n);
    }

    pub fn len(&self) -> usize {
        self.lock().list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The entries `from..to` (as far as there are).
    pub fn slice(&self, from: usize, to: usize) -> Vec<Pending> {
        self.lock().list.iter().skip(from).take(to.saturating_sub(from)).cloned().collect()
    }
}

fn file_name(path: &str) -> String {
    std::path::Path::new(path).file_name().map_or_else(|| path.to_string(), |n| n.to_string_lossy().into_owned())
}

/// The tiles the view shows: those of the import running, when its photos will be among this
/// view's (the library, or the album they are dropped into).
pub fn pending(app: &LightcraftApp) -> Option<Pendings> {
    let task = app.import.as_ref()?;
    let list = task.pending()?;
    let here = match app.session.source {
        LibrarySource::All | LibrarySource::RecentlyAdded => true,
        LibrarySource::Album(a) => task.album() == Some(a.0),
        _ => false,
    };
    (here && !list.is_empty()).then(|| list.clone())
}

const GAP: f32 = 4.0;
const CAPTION_H: f32 = 28.0;

/// About how wide a tile is: the grid's thumbnails (they are stretched to fill the row, see [`fit`]).
pub fn side(app: &LightcraftApp) -> f32 {
    app.ui.thumb_size.max(72.0)
}

fn columns(width: f32, side: f32) -> usize {
    (((width + GAP) / (side + GAP)).floor() as usize).max(1)
}

/// How many tiles a row of a page `width` wide holds, and how wide each is: as many as would be
/// about `side` wide, stretched so that the row is filled from edge to edge.
fn fit(width: f32, side: f32) -> (usize, f32) {
    let cols = columns(width, side);
    let tile = (width - GAP * (cols + 1) as f32) / cols as f32;
    (cols, if tile.is_finite() { tile.max(1.0) } else { side })
}

/// The height of the block of `n` tiles in a page `width` wide.
pub fn height(n: usize, width: f32, side: f32) -> f32 {
    if n == 0 {
        return 0.0;
    }
    let (cols, tile) = fit(width, side);
    CAPTION_H + n.div_ceil(cols) as f32 * (tile + GAP) + GAP
}

/// Scroll to the block if a drop has just started.
pub fn reveal_now(ui: &egui::Ui, block: Rect) {
    if take_reveal(ui.ctx()) {
        ui.scroll_to_rect(block, Some(egui::Align::TOP));
    }
}

/// A view with no photos of its own yet but dropped ones on their way: just their tiles.
pub fn show_alone(app: &LightcraftApp, ui: &mut egui::Ui, pend: &Pendings) {
    let side = side(app);
    egui::ScrollArea::vertical().id_salt("pending-scroll").auto_shrink([false, false]).show_viewport(ui, |ui, viewport| {
        let width = ui.available_width() - 8.0;
        let h = height(pend.len(), width, side);
        let (area, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
        let block = Rect::from_min_size(area.min, vec2(width, h));
        paint_block(ui, pend, block, viewport.translate(area.min.to_vec2()), side);
    });
}

/// The block at `block`: its caption, and the tiles within `view` (the part of the page on screen).
pub fn paint_block(ui: &egui::Ui, pend: &Pendings, block: Rect, view: Rect, side: f32) {
    let t = Tokens::get(ui.ctx());
    let n = pend.len();
    let p = ui.painter().clone();
    let caption = format!("{} ({n})", crate::i18n::tr("Adding photos"));
    p.text(pos2(block.left() + 10.0, block.top() + CAPTION_H / 2.0), Align2::LEFT_CENTER, caption, t.font(12.0), t.text_dim);
    let (cols, tile_side) = fit(block.width(), side);
    let (top, row_h) = (block.top() + CAPTION_H, tile_side + GAP);
    let first_row = ((view.top() - top) / row_h).floor().max(0.0) as usize;
    let end_row = ((view.bottom() - top) / row_h).ceil().max(0.0) as usize;
    let (from, to) = (first_row.saturating_mul(cols), end_row.saturating_mul(cols).min(n));
    if from >= to {
        return;
    }
    for (k, tile) in pend.slice(from, to).iter().enumerate() {
        let i = from + k;
        let at = pos2(block.left() + GAP + (i % cols) as f32 * (tile_side + GAP), top + (i / cols) as f32 * row_h);
        paint_tile(ui, &p, &t, Rect::from_min_size(at, vec2(tile_side, tile_side)), i, tile);
    }
}

fn paint_tile(ui: &egui::Ui, p: &egui::Painter, t: &Tokens, r: Rect, i: usize, tile: &Pending) {
    p.rect_filled(r, 0.0, if Tokens::is_dark(ui.ctx()) { Color32::from_gray(38) } else { t.inset });
    badge(ui, p, t, pos2(r.center().x, r.center().y - 8.0), tile.state);
    let stem = tile.name.rsplit_once('.').map_or(tile.name.as_str(), |(stem, _)| stem);
    p.with_clip_rect(r.shrink2(vec2(6.0, 0.0))).text(pos2(r.center().x, r.bottom() - 14.0), Align2::CENTER_CENTER, stem, t.font(10.5), t.text_dim);
    crate::widgets::register(ui.ctx(), format!("pending:{i}"), r);
    let what = match tile.state {
        State::Waiting => crate::i18n::tr("Waiting to be added"),
        State::Adding => crate::i18n::tr("Adding to the library"),
    };
    ui.interact(r, Id::new(("pending", i)), Sense::hover()).on_hover_text(format!("{} · {what}", tile.name));
}

/// The status mark, as the cloud's while it syncs: an arrow into the library, dim while it waits
/// and with an arc turning round it while the photo is being read and added.
fn badge(ui: &egui::Ui, p: &egui::Painter, t: &Tokens, c: Pos2, state: State) {
    const R: f32 = 15.0;
    p.circle_filled(c, R, Color32::from_black_alpha(140));
    let tint = if state == State::Adding { Color32::WHITE } else { Color32::WHITE.gamma_multiply(0.55) };
    paint(p, Rect::from_center_size(c, vec2(16.0, 16.0)), Icon::Download, tint);
    if state == State::Adding {
        let a0 = ui.input(|i| i.time) as f32 * 3.2;
        let arc: Vec<Pos2> = (0..=14)
            .map(|k| {
                let a = a0 + k as f32 * 0.14;
                c + vec2(a.cos(), a.sin()) * (R - 1.0)
            })
            .collect();
        p.add(egui::Shape::line(arc, Stroke::new(2.0, t.accent)));
        ui.ctx().request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(p: &Pendings) -> Vec<String> {
        p.slice(0, usize::MAX).into_iter().map(|t| format!("{}:{:?}", t.name, t.state)).collect()
    }

    #[test]
    fn photos_are_marked_as_they_start_and_leave_from_the_front() {
        let p = Pendings::default();
        p.set(["/a/1.jpg", "/a/2.jpg", "/b/3.jpg", "/b/4.jpg"]);
        assert_eq!(names(&p), ["1.jpg:Waiting", "2.jpg:Waiting", "3.jpg:Waiting", "4.jpg:Waiting"]);
        p.start(2);
        assert_eq!(names(&p), ["1.jpg:Adding", "2.jpg:Adding", "3.jpg:Waiting", "4.jpg:Waiting"]);
        // the first batch is in the catalog; the worker starts the next while it is
        p.added(2);
        p.start(1);
        assert_eq!(names(&p), ["3.jpg:Adding", "4.jpg:Waiting"]);
        p.added(1);
        assert_eq!(names(&p), ["4.jpg:Waiting"]);
    }

    #[test]
    fn counts_past_the_end_are_harmless() {
        let p = Pendings::default();
        p.added(5);
        p.start(5);
        assert!(p.is_empty());
        p.set(["/a/1.jpg"]);
        p.start(9);
        p.added(9);
        p.start(1);
        assert!(p.is_empty());
        assert!(p.slice(3, 1).is_empty());
    }

    #[test]
    fn a_folder_listing_replaces_the_files_known_at_the_drop() {
        let p = Pendings::default();
        p.set(["/trip"]);
        p.set(["/trip/a.jpg", "/trip/b.jpg"]);
        assert_eq!(p.len(), 2);
    }

    #[test]
    fn rows_of_tiles_fill_the_width() {
        assert_eq!(height(0, 800.0, 100.0), 0.0);
        // 7 tiles of about 100 + 4 fit in 800, stretched to 800 = 4 + 7 × (4 + tile)
        let (cols, tile) = fit(800.0, 100.0);
        assert_eq!(cols, 7);
        assert!(tile > 100.0 && (GAP + cols as f32 * (tile + GAP) - 800.0).abs() < 0.01, "{tile}");
        // 7 tiles are a row, 8 take two
        assert_eq!(height(7, 800.0, 100.0), CAPTION_H + tile + GAP + GAP);
        assert_eq!(height(8, 800.0, 100.0), CAPTION_H + 2.0 * (tile + GAP) + GAP);
        // fewer photos than a row holds: the same tiles, the row not stretched wider
        assert_eq!(height(1, 800.0, 100.0), height(7, 800.0, 100.0));
        // a page narrower than a tile still holds one per row
        assert_eq!(fit(10.0, 100.0).0, 1);
        assert_eq!(fit(f32::NAN, 100.0), (1, 100.0));
    }
}
