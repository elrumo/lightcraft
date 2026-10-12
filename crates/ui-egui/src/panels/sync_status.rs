//! The sync status popover: what the cloud button (the top bar's, or the phone grid's) opens, as
//! Lightroom's mobile app shows what its cloud is doing. In one panel:
//!
//! - **where syncing stands** (at the top) — up to date, syncing, paused, signed out, a problem, or
//!   the choice a sign-in is waiting for (this library and the server's both have photos), and the
//!   server it syncs with;
//! - **This device** — changes waiting to go, photos being uploaded and files being downloaded
//!   ("12 of 56", with the names of the ones moving now);
//! - **Server** — what the server is doing for this library: scanning its library folders,
//!   building previews, indexing photos for search (`GET /api/activity`);
//! - **Storage** — what the library takes on the server and how much room its disk has left;
//! - **Pause / Sync Now / Settings** (at the bottom).
//!
//! Each section is a rounded card on the popover's glass, as iOS groups a list: a row per piece of
//! work, with its icon, how far it is and a bar. The status and the buttons stay put while the
//! sections between them scroll (a server with a lot to do on a small phone).
//!
//! The panel is a pull-down of [`mobile`] (a tap outside closes it) kept current while it is open:
//! the engine asks the server again every couple of seconds only while somebody looks.

use std::time::Duration;

use egui::{Color32, Margin, Rect, Sense, Stroke, pos2, vec2};
use lightcraft_catalog::sync::proto;
use lightcraft_engine::cloud_sync::{ConflictInfo, Transfer, Transfers};
use serde_json::json;

use crate::LightcraftApp;
use crate::icons::{Icon, paint};
use crate::panels::mobile;
use crate::sync_ui::{bytes_label, count_label};
use crate::theme::Tokens;
use crate::widgets::register;

/// The popover's id (a pull-down panel of [`mobile`]).
pub const ID: &str = "syncStatus";
/// Its width on a desktop (a phone's is the screen's, less its margins).
const WIDTH: f32 = 380.0;
/// The space between the popover's edges and what is in it.
const PAD: f32 = 16.0;
/// A card's corner radius and inner padding.
const CARD_R: f32 = 16.0;
const CARD_PAD: f32 = 12.0;
/// A card row's icon (a tinted disc) and the gap between it and the text.
const ICON_D: f32 = 30.0;
const ICON_GAP: f32 = 12.0;
/// The height of the buttons at the bottom.
const BUTTON_H: f32 = 40.0;
/// The room the buttons take, with the space around them.
const FOOTER_H: f32 = 14.0 + BUTTON_H + 10.0;

/// Open the popover under `anchor`, the button that was pressed.
pub fn open(ctx: &egui::Context, anchor: Rect) {
    mobile::open_menu(ctx, ID, anchor);
}

pub fn is_open(ctx: &egui::Context) -> bool {
    mobile::actions_open(ctx, ID)
}

/// What the popover says, read from the app first so that drawing it can change the app.
struct View {
    /// `off` (never synced), `signedOut`, `conflict`, `paused`, `error`, `syncing` or `idle`.
    state: &'static str,
    host: String,
    error: Option<String>,
    photos: usize,
    /// While the server's library is read in pages: (photos read, photos it has).
    reading: Option<(usize, usize)>,
    transfers: Transfers,
    activity: Option<proto::Activity>,
    activity_error: Option<String>,
    usage: Option<proto::Usage>,
    conflict: Option<ConflictInfo>,
    /// Changes kept only in memory because saving fails: (how many, why).
    unsaved: Option<(usize, String)>,
}

impl View {
    fn read(app: &LightcraftApp) -> View {
        let st = app.session.sync_state();
        let host = st
            .map(|st| st.config.server.trim_start_matches("https://").trim_start_matches("http://").trim_end_matches('/').to_string())
            .unwrap_or_default();
        View {
            state: st.map_or("off", |st| st.state()),
            host,
            error: st.and_then(|st| st.error()).map(str::to_string),
            photos: app.session.catalog.photos().filter(|p| p.in_library()).count(),
            reading: st.and_then(|st| st.reading()),
            transfers: app.session.sync_transfers(),
            activity: st.and_then(|st| st.activity()).cloned(),
            activity_error: st.and_then(|st| st.activity_error()).map(str::to_string),
            usage: st.and_then(|st| st.usage()).cloned(),
            conflict: st.and_then(|st| st.conflict()).cloned(),
            unsaved: app.session.unsaved().map(|(n, e)| (n, e.to_string())),
        }
    }

    /// This library is the server's too: there are things to show about it.
    fn joined(&self) -> bool {
        matches!(self.state, "paused" | "error" | "syncing" | "idle")
    }
}

/// What a press in the popover asks for (done after it is drawn).
#[derive(Clone, Copy)]
enum Act {
    PauseToggle,
    SyncNow,
    Settings,
    Choose,
    SignIn,
}

/// Draw the popover when it is open (every frame: it slides out when closed). Call once a frame
/// after the panels.
pub fn show(app: &mut LightcraftApp, ctx: &egui::Context) {
    if std::mem::take(&mut app.ui.sync_popover) {
        // (asked for by a command: under the top right corner, where the cloud button is)
        let screen = ctx.content_rect();
        open(ctx, Rect::from_min_size(pos2(screen.right() - 52.0, screen.top() + if app.compact { 44.0 } else { 36.0 }), vec2(40.0, 2.0)));
    }
    if is_open(ctx) {
        // keep what it shows current while somebody looks
        app.session.sync_want_activity();
        app.session.sync_want_usage();
        ctx.request_repaint_after(Duration::from_millis(1000));
    }
    let max_h = ctx.content_rect().height() * 0.78;
    let mut act = None;
    mobile::actions_wide(ctx, ID, None, WIDTH, |ui| {
        ui.set_max_height(max_h);
        act = content(app, ui, max_h);
    });
    match act {
        None => {}
        Some(Act::PauseToggle) => {
            let paused = app.session.sync_state().is_some_and(|st| st.config.paused);
            report(app, ctx, "sync.pause", json!({"on": !paused}));
        }
        Some(Act::SyncNow) => report(app, ctx, "sync.now", json!({})),
        Some(Act::Settings | Act::SignIn) => {
            mobile::close_actions(ctx);
            report(app, ctx, "app.settings", json!({"tab": "sync"}));
        }
        Some(Act::Choose) => {
            mobile::close_actions(ctx);
            report(app, ctx, "dialog.syncChoice", json!({}));
        }
    }
}

fn report(app: &mut LightcraftApp, ctx: &egui::Context, id: &str, params: serde_json::Value) {
    if let Err(e) = app.run(id, params) {
        app.toast(ctx, e);
    }
}

/// The panel's contents, at most `max_h` tall; the press, if any.
fn content(app: &LightcraftApp, ui: &mut egui::Ui, max_h: f32) -> Option<Act> {
    let t = Tokens::get(ui.ctx());
    let v = View::read(app);
    let mut act = None;
    ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
    let s = status(&t, &v);
    let top = ui.cursor().top();
    padded(ui, 12.0, 0.0, |ui| header(ui, &t, &s));
    // (an area lays its content out in the size it had last frame, and the contents change size
    // as answers come in: the scroll area is given its own room, or it could never grow)
    let footer_h = if v.joined() { FOOTER_H } else { 14.0 };
    let room = (max_h - (ui.cursor().top() - top) - footer_h).max(120.0);
    egui::ScrollArea::vertical().id_salt("syncStatusScroll").max_height(room).auto_shrink([false, true]).show(ui, |ui| {
        padded(ui, 0.0, 0.0, |ui| body(ui, &t, &v, &s, &mut act));
    });
    if v.joined() {
        padded(ui, 14.0, 10.0, |ui| footer(ui, &t, &v, &mut act));
    } else {
        ui.add_space(14.0);
    }
    act
}

/// What is between the status and the buttons (and scrolls): the status's sentence and its button,
/// warnings, then the sections.
fn body(ui: &mut egui::Ui, t: &Tokens, v: &View, s: &Status, act: &mut Option<Act>) {
    if let Some((note, color)) = s.note.as_ref().filter(|(n, _)| !n.is_empty()) {
        ui.add_space(10.0);
        text(ui, 13.5, *color, note.as_str());
    }
    if let Some((label, what)) = s.primary {
        ui.add_space(14.0);
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::hover());
        if capsule(ui, t, r, "syncStatusPrimary", label, Face::Label(None), true) {
            *act = Some(what);
        }
    }
    if let Some((n, why)) = &v.unsaved {
        let text = format!(
            "{n} change{} saved in memory but not written to disk: {why}. LightCraft retries automatically; quitting now would lose {}.",
            if *n == 1 { "" } else { "s" },
            if *n == 1 { "it" } else { "them" }
        );
        ui.add_space(14.0);
        banner(ui, t, "unsaved", t.reject, "Changes aren't saved", &text);
    }
    if let Some(d) = v.usage.as_ref().and_then(|u| u.disk).filter(low) {
        let text = format!(
            "Only {} of its {} disk is left. New photos may not upload until some space is freed on the server.",
            bytes_label(d.free),
            bytes_label(d.total)
        );
        ui.add_space(14.0);
        banner(ui, t, "serverDisk", t.caution, "The server is almost out of space", &text);
    }
    if v.joined() {
        section(ui, t, "This device");
        card(ui, "syncDevice", |ui| rows(ui, t, &device(t, v)));
        section(ui, t, "Server");
        card(ui, "syncServer", |ui| rows(ui, t, &server(t, v)));
        if let Some(u) = &v.usage {
            section(ui, t, "Storage");
            card(ui, "syncStorage", |ui| storage(ui, t, u));
        }
    }
}

/// The content in a column inset from the panel's sides.
fn padded(ui: &mut egui::Ui, top: f32, bottom: f32, add: impl FnOnce(&mut egui::Ui)) {
    let m = Margin { left: PAD as i8, right: PAD as i8, top: top as i8, bottom: bottom as i8 };
    egui::Frame::NONE.inner_margin(m).show(ui, |ui| {
        ui.set_width(ui.available_width());
        add(ui);
    });
}

/// Wrapped text.
fn text(ui: &mut egui::Ui, size: f32, color: Color32, s: impl Into<String>) {
    ui.label(egui::RichText::new(s.into()).size(size).color(color));
}

/// `s` on one line of at most `max_w` points (cut with "…").
fn one_line(ui: &egui::Ui, s: &str, font: egui::FontId, color: Color32, max_w: f32) -> std::sync::Arc<egui::Galley> {
    egui::WidgetText::from(egui::RichText::new(s).font(font).color(color)).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        max_w.max(20.0),
        egui::TextStyle::Body,
    )
}

/// The fill of a card on the popover's glass: a grey that darkens the light glass and lightens the
/// dark one, as iOS fills grouped rows in a popover.
fn card_fill(ctx: &egui::Context) -> Color32 {
    Color32::from_rgba_unmultiplied(118, 118, 128, if Tokens::is_dark(ctx) { 46 } else { 26 })
}

/// A wash of `tint` for what stands behind a coloured glyph or a warning.
fn wash(ctx: &egui::Context, tint: Color32) -> Color32 {
    tint.gamma_multiply(if Tokens::is_dark(ctx) { 0.24 } else { 0.14 })
}

/// The empty part of a bar on a card (the theme's track is for the panels, and gets lost on a dark
/// card).
fn track(ctx: &egui::Context, t: &Tokens) -> Color32 {
    t.text_dim.gamma_multiply(if Tokens::is_dark(ctx) { 0.38 } else { 0.28 })
}

/// Black or white, whichever reads on `fill`.
fn ink_on(fill: Color32) -> Color32 {
    let luma = fill.r() as u32 * 299 + fill.g() as u32 * 587 + fill.b() as u32 * 114;
    if luma > 170_000 { Color32::BLACK } else { Color32::WHITE }
}

/// The name of a group of rows, over its card.
fn section(ui: &mut egui::Ui, t: &Tokens, title: &str) {
    ui.add_space(18.0);
    let g = one_line(ui, crate::i18n::tr(title), t.semibold(13.0), t.text_dim, ui.available_width() - CARD_PAD);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), g.size().y), Sense::hover());
    ui.painter().galley(pos2(r.left() + CARD_PAD, r.top()), g, t.text_dim);
    ui.add_space(6.0);
}

/// A rounded card (`section:{id}`) behind what `add` puts in it.
fn card(ui: &mut egui::Ui, id: &str, add: impl FnOnce(&mut egui::Ui)) {
    let under = ui.painter().add(egui::Shape::Noop);
    let r = egui::Frame::NONE
        .inner_margin(Margin::symmetric(CARD_PAD as i8, 0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        })
        .response
        .rect;
    ui.painter().set(under, egui::epaint::RectShape::filled(r, CARD_R, card_fill(ui.ctx())));
    register(ui.ctx(), format!("section:{id}"), r);
}

/// What a row's disc shows.
#[derive(Clone, Copy)]
enum Glyph {
    Icon(Icon),
    /// A `!`: something went wrong.
    Alert,
}

/// `glyph` in `tint` on a disc washed with it, filling `r`.
fn disc(ui: &egui::Ui, t: &Tokens, r: Rect, glyph: Glyph, tint: Color32) {
    let p = ui.painter();
    p.circle_filled(r.center(), r.width() / 2.0, wash(ui.ctx(), tint));
    match glyph {
        Glyph::Icon(icon) => paint(p, Rect::from_center_size(r.center(), r.size() * 0.56), icon, tint),
        Glyph::Alert => {
            p.text(r.center(), egui::Align2::CENTER_CENTER, "!", t.semibold(r.width() * 0.52), tint);
        }
    }
}

/// One line of a card: a piece of work, or how things stand.
struct Row {
    glyph: Glyph,
    tint: Color32,
    title: String,
    /// How far it is ("12 of 56"), on the right.
    value: String,
    /// Its bar, when it can tell how far it is.
    fraction: Option<f32>,
    /// Smaller lines under it.
    detail: Vec<String>,
}

impl Row {
    fn new(glyph: Glyph, tint: Color32, title: impl Into<String>) -> Row {
        Row { glyph, tint, title: title.into(), value: String::new(), fraction: None, detail: Vec::new() }
    }

    fn value(mut self, value: impl Into<String>) -> Row {
        self.value = value.into();
        self
    }

    fn fraction(mut self, f: f32) -> Row {
        self.fraction = Some(f);
        self
    }

    fn detail(mut self, line: impl Into<String>) -> Row {
        let line = line.into();
        if !line.is_empty() {
            self.detail.push(line);
        }
        self
    }
}

/// `rows` one under the other, a hairline between them from where their text starts.
fn rows(ui: &mut egui::Ui, t: &Tokens, rows: &[Row]) {
    for (i, row) in rows.iter().enumerate() {
        if i > 0 {
            let r = ui.max_rect();
            ui.painter().hline((r.left() + ICON_D + ICON_GAP)..=(r.right() + CARD_PAD), ui.cursor().top(), Stroke::new(0.5, t.divider));
        }
        draw_row(ui, t, row);
    }
}

/// A row: its disc, then its title and value, its bar and its detail, all centred on the disc.
fn draw_row(ui: &mut egui::Ui, t: &Tokens, row: &Row) {
    const V: f32 = 11.0;
    const BAR: f32 = 4.0;
    let w = ui.available_width();
    let text_x = ICON_D + ICON_GAP;
    let text_w = (w - text_x).max(60.0);
    let p = ui.painter();
    let value_g = p.layout_no_wrap(row.value.clone(), t.font(13.5), t.text_dim);
    let title_w = (text_w - if row.value.is_empty() { 0.0 } else { value_g.size().x + 10.0 }).max(60.0);
    let title_g = p.layout(row.title.clone(), t.font(15.0), t.text, title_w);
    let top_h = title_g.size().y.max(value_g.size().y);
    let detail_g: Vec<_> = row.detail.iter().map(|d| p.layout(d.clone(), t.font(12.5), t.text_dim, text_w)).collect();
    let bar_h = if row.fraction.is_some() { 8.0 + BAR } else { 0.0 };
    let detail_h: f32 = detail_g.iter().map(|g| 3.0 + g.size().y).sum();
    let content_h = top_h + bar_h + detail_h;
    let (r, _) = ui.allocate_exact_size(vec2(w, content_h.max(ICON_D) + 2.0 * V), Sense::hover());
    disc(ui, t, Rect::from_center_size(pos2(r.left() + ICON_D / 2.0, r.center().y), vec2(ICON_D, ICON_D)), row.glyph, row.tint);
    let p = ui.painter();
    let x = r.left() + text_x;
    let mut y = r.center().y - content_h / 2.0;
    p.galley(pos2(x, y + (top_h - title_g.size().y) / 2.0), title_g, t.text);
    if !row.value.is_empty() {
        p.galley(pos2(r.right() - value_g.size().x, y + (top_h - value_g.size().y) / 2.0), value_g, t.text_dim);
    }
    y += top_h;
    if let Some(f) = row.fraction {
        let track = Rect::from_min_size(pos2(x, y + 8.0), vec2(r.right() - x, BAR));
        p.rect_filled(track, BAR / 2.0, self::track(ui.ctx(), t));
        let f = f.clamp(0.0, 1.0);
        if f > 0.0 {
            let mut fill = track;
            fill.set_width((track.width() * f).max(BAR));
            p.rect_filled(fill, BAR / 2.0, row.tint);
        }
        y += bar_h;
    }
    for g in detail_g {
        y += 3.0;
        let gh = g.size().y;
        p.galley(pos2(x, y), g, t.text_dim);
        y += gh;
    }
}

/// A problem above the sections (`banner:{id}`): a `!` in `tint`, what it is and why, on a card
/// washed with it.
fn banner(ui: &mut egui::Ui, t: &Tokens, id: &str, tint: Color32, title: &str, why: &str) {
    const D: f32 = 22.0;
    let under = ui.painter().add(egui::Shape::Noop);
    let r = egui::Frame::NONE
        .inner_margin(Margin::same(CARD_PAD as i8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let w = ui.available_width();
            ui.horizontal_top(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(D, D), Sense::hover());
                ui.painter().circle_filled(r.center(), D / 2.0, tint);
                ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, "!", t.semibold(13.0), ink_on(tint));
                ui.add_space(10.0);
                ui.vertical(|ui| {
                    ui.set_width(w - D - 10.0);
                    ui.label(egui::RichText::new(crate::i18n::tr(title)).font(t.semibold(14.5)).color(t.text));
                    ui.add_space(2.0);
                    text(ui, 13.0, t.text_dim, why);
                });
            });
        })
        .response
        .rect;
    ui.painter().set(under, egui::epaint::RectShape::filled(r, CARD_R, wash(ui.ctx(), tint)));
    register(ui.ctx(), format!("banner:{id}"), r);
}

/// The server's disk is nearly full: less than 1 GB or 2 % left.
fn low(d: &proto::Disk) -> bool {
    d.total > 0 && (d.free < 1_000_000_000 || d.free.saturating_mul(50) < d.total)
}

/// What a [`capsule`] shows.
#[derive(Clone, Copy)]
enum Face {
    /// Its label, after the icon when both fit.
    Label(Option<Icon>),
    /// Only the icon (the label is what a screen reader says).
    Icon(Icon),
}

/// A button (`button:{id}`) drawn as a capsule filling `rect`: the accent's when `prominent`, else
/// a card's grey. True when pressed.
fn capsule(ui: &mut egui::Ui, t: &Tokens, rect: Rect, id: &str, label: &str, face: Face, prominent: bool) -> bool {
    let label = crate::i18n::tr(label);
    let resp = ui.interact(rect, egui::Id::new(("sync-status", id)), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    register(ui.ctx(), format!("button:{id}"), rect);
    let dark = Tokens::is_dark(ui.ctx());
    let (fill, ink) = if prominent { (t.accent, Color32::WHITE) } else { (card_fill(ui.ctx()), t.text) };
    let p = ui.painter();
    p.rect_filled(rect, rect.height() / 2.0, fill);
    // (lit under the pointer, more while pressed)
    let lit = if resp.is_pointer_button_down_on() {
        36
    } else if resp.hovered() {
        16
    } else {
        0
    };
    if lit > 0 {
        let over = if dark || prominent { Color32::from_white_alpha(lit) } else { Color32::from_black_alpha(lit) };
        p.rect_filled(rect, rect.height() / 2.0, over);
    }
    match face {
        Face::Icon(icon) => paint(p, Rect::from_center_size(rect.center(), vec2(20.0, 20.0)), icon, if prominent { ink } else { t.text_dim }),
        Face::Label(icon) => {
            const ICON: f32 = 18.0;
            const GAP: f32 = 6.0;
            let room = rect.width() - 24.0;
            let g = one_line(ui, label, t.semibold(15.0), ink, room);
            // (the icon gives way to a label that needs its room)
            let icon = icon.filter(|_| g.size().x + ICON + GAP <= room);
            let lead = if icon.is_some() { ICON + GAP } else { 0.0 };
            let x = rect.center().x - (g.size().x + lead) / 2.0;
            let p = ui.painter();
            if let Some(icon) = icon {
                paint(p, Rect::from_min_size(pos2(x, rect.center().y - ICON / 2.0), vec2(ICON, ICON)), icon, ink);
            }
            p.galley(pos2(x + lead, rect.center().y - g.size().y / 2.0), g, ink);
        }
    }
    resp.clicked()
}

// -------------------------------------------------------------------------------------- header

/// How syncing stands, as the header says it.
struct Status {
    glyph: Icon,
    tint: Color32,
    /// A red `!` on the disc: something needs the user.
    problem: bool,
    title: String,
    /// The server (and for an up-to-date library, its size).
    sub: String,
    /// A sentence under them, and its colour.
    note: Option<(String, Color32)>,
    /// The one thing to do about it: (label, press).
    primary: Option<(&'static str, Act)>,
}

fn status(t: &Tokens, v: &View) -> Status {
    let host = v.host.clone();
    let tr = |s: &str| crate::i18n::tr(s).to_string();
    let plain = |glyph, tint, title: String, sub: String| Status { glyph, tint, problem: false, title, sub, note: None, primary: None };
    match v.state {
        "off" => Status {
            note: Some((tr("Sync it with your own LightCraft server to use its photos and edits on your other devices."), t.text_dim)),
            primary: Some(("Set Up Sync…", Act::Settings)),
            ..plain(Icon::Cloud, t.text_dim, tr("This library is only on this device"), String::new())
        },
        "signedOut" => Status {
            problem: true,
            note: Some((
                v.error.clone().unwrap_or_else(|| tr("Sign in again to keep syncing. Changes made meanwhile wait on this device; nothing is lost.")),
                t.text_dim,
            )),
            primary: Some(("Sign In…", Act::SignIn)),
            ..plain(Icon::Cloud, t.caution, tr("Signed out"), host)
        },
        "conflict" => Status {
            problem: true,
            note: Some((conflict_text(v.conflict.as_ref()), t.text_dim)),
            primary: Some(("Choose What to Do…", Act::Choose)),
            ..plain(Icon::Cloud, t.caution, tr("The server has a library already"), host)
        },
        "paused" => Status {
            note: Some((tr("Changes wait on this device until you resume."), t.text_dim)),
            ..plain(Icon::Pause, t.text_dim, tr("Syncing is paused"), host)
        },
        "error" => Status {
            problem: true,
            note: Some((
                format!("{} {}", v.error.clone().unwrap_or_default(), tr("LightCraft keeps trying; changes wait on this device.")).trim().to_string(),
                t.text,
            )),
            ..plain(Icon::Cloud, t.caution, tr("Can't sync"), host)
        },
        "syncing" => plain(Icon::Refresh, t.accent, tr("Syncing"), host),
        _ => {
            let count = photos(v.photos as u64);
            let sub = if host.is_empty() { count } else { format!("{host} · {count}") };
            plain(Icon::Check, t.switch_on, tr("Up to date"), sub)
        }
    }
}

/// The status: a disc, what is going on and with which server (its sentence and button are the
/// top of [`body`], which scrolls: an error can be long).
fn header(ui: &mut egui::Ui, t: &Tokens, s: &Status) {
    const D: f32 = 44.0;
    let w = ui.available_width();
    let text_w = (w - D - 12.0).max(60.0);
    let title_g = ui.painter().layout(s.title.clone(), t.semibold(17.0), t.text, text_w);
    let sub_g = (!s.sub.is_empty()).then(|| ui.painter().layout(s.sub.clone(), t.font(13.5), t.text_dim, text_w));
    let text_h = title_g.size().y + sub_g.as_ref().map_or(0.0, |g| 1.0 + g.size().y);
    let (r, _) = ui.allocate_exact_size(vec2(w, text_h.max(D)), Sense::hover());
    let tile = Rect::from_min_size(pos2(r.left(), r.center().y - D / 2.0), vec2(D, D));
    disc(ui, t, tile, Glyph::Icon(s.glyph), s.tint);
    let p = ui.painter();
    if s.problem {
        let c = tile.right_top() + vec2(-5.0, 5.0);
        p.circle_filled(c, 8.0, t.reject);
        p.text(c, egui::Align2::CENTER_CENTER, "!", t.semibold(11.0), Color32::WHITE);
    }
    let x = tile.right() + 12.0;
    let mut y = r.center().y - text_h / 2.0;
    let title_h = title_g.size().y;
    p.galley(pos2(x, y), title_g, t.text);
    y += title_h + 1.0;
    if let Some(g) = sub_g {
        p.galley(pos2(x, y), g, t.text_dim);
    }
}

/// The state line of a conflict: the two libraries' sizes.
fn conflict_text(c: Option<&ConflictInfo>) -> String {
    match c {
        Some(c) => format!(
            "{} {}, {} {}. {}",
            crate::i18n::tr("This library has"),
            photos(c.here.photos as u64),
            crate::i18n::tr("the server's has"),
            photos(c.there.photos as u64),
            crate::i18n::tr("Nothing syncs until you choose how to combine them.")
        ),
        None => crate::i18n::tr("Nothing syncs until you choose how to combine the two libraries.").to_string(),
    }
}

/// "1 photo", "12 photos".
fn photos(n: u64) -> String {
    format!("{} {}", count_label(n), crate::i18n::tr(if n == 1 { "photo" } else { "photos" }))
}

/// "12 of 56".
fn of(done: u64, total: u64) -> String {
    format!("{} {} {}", count_label(done), crate::i18n::tr("of"), count_label(total))
}

// ---------------------------------------------------------------------------------- This device

fn device(t: &Tokens, v: &View) -> Vec<Row> {
    let tr = &v.transfers;
    let mut out = Vec::new();
    if let Some((read, total)) = v.reading {
        out.push(
            Row::new(Glyph::Icon(Icon::Download), t.accent, crate::i18n::tr("Loading the library"))
                .value(of(read as u64, total as u64))
                .fraction(fraction(read as u64, total as u64)),
        );
    }
    if tr.pending > 0 {
        out.push(Row::new(Glyph::Icon(Icon::Refresh), t.accent, crate::i18n::tr("Sending changes")).value(count_label(tr.pending as u64)));
    }
    if tr.uploads > 0 {
        let total = tr.uploads_done + tr.uploads;
        let mut row = Row::new(Glyph::Icon(Icon::Upload), t.accent, crate::i18n::tr("Uploading photos"))
            .value(of(tr.uploads_done as u64, total as u64))
            .fraction(fraction(tr.uploads_done as u64, total as u64));
        row.detail = names(&tr.running, true);
        out.push(row);
    }
    if tr.downloads > 0 {
        let total = tr.downloads_done + tr.downloads;
        let what = match tr.running.iter().filter(|r| !r.upload).map(|r| r.what).collect::<std::collections::BTreeSet<_>>().iter().next() {
            Some(&"original") => "Downloading originals",
            Some(&"smart preview") => "Downloading previews to edit",
            _ => "Downloading previews",
        };
        let mut row = Row::new(Glyph::Icon(Icon::Download), t.accent, crate::i18n::tr(what))
            .value(of(tr.downloads_done as u64, total as u64))
            .fraction(fraction(tr.downloads_done as u64, total as u64));
        row.detail = names(&tr.running, false);
        out.push(row);
    }
    if let Some(e) = &tr.error {
        let n = count_label(tr.failing.max(1) as u64);
        let what = crate::i18n::tr(if tr.failing == 1 { "file couldn't be sent or received" } else { "files couldn't be sent or received" });
        out.push(Row::new(Glyph::Alert, t.caution, format!("{n} {what}")).detail(format!("{e}. LightCraft tries again every minute.")));
    }
    if out.is_empty() {
        out.push(Row::new(Glyph::Icon(Icon::Check), t.text_dim, crate::i18n::tr("Nothing to send or receive")));
    }
    out
}

/// "IMG_0231.CR3 · original" for the files moving now (at most three, and how many more).
fn names(running: &[Transfer], upload: bool) -> Vec<String> {
    let mine: Vec<&Transfer> = running.iter().filter(|r| r.upload == upload).collect();
    let mut out: Vec<String> = mine
        .iter()
        .take(3)
        .map(|r| if r.name.is_empty() { crate::i18n::tr(r.what).to_string() } else { format!("{} · {}", r.name, crate::i18n::tr(r.what)) })
        .collect();
    if mine.len() > 3 {
        out.push(format!("+ {} {}", mine.len() - 3, crate::i18n::tr("more")));
    }
    out
}

// --------------------------------------------------------------------------------------- Server

fn server(t: &Tokens, v: &View) -> Vec<Row> {
    let Some(a) = &v.activity else {
        return vec![match &v.activity_error {
            Some(e) => Row::new(Glyph::Alert, t.caution, crate::i18n::tr("Can't tell what the server is doing")).detail(e.clone()),
            None => Row::new(Glyph::Icon(Icon::Server), t.text_dim, crate::i18n::tr("Asking the server…")),
        }];
    };
    let mut out = Vec::new();
    if let Some(s) = a.scan.as_ref().filter(|s| s.scanning) {
        let row = Row::new(Glyph::Icon(Icon::Folder), t.accent, crate::i18n::tr("Scanning library folders"));
        out.push(match s.phase.as_str() {
            "reading" => {
                let mut detail = vec![crate::i18n::tr("Reading new photos").to_string()];
                detail.extend(s.eta_secs.map(eta));
                row.value(of(s.done, s.todo)).fraction(fraction(s.done, s.todo)).detail(detail.join(" · "))
            }
            "listing" => row.value(format!("{} {}", count_label(s.files), crate::i18n::tr("found"))).detail(crate::i18n::tr("Looking for photos")),
            _ => row.detail(crate::i18n::tr("Finishing up")),
        });
    }
    for (icon, title, p) in [
        (Icon::Photos, "Building previews", Some(a.previews)),
        (Icon::Search, "Indexing photos for search", a.search),
        (Icon::Info, "Reading the text in photos", a.text),
        (Icon::Subject, "Finding people in photos", a.faces),
    ] {
        if let Some(p) = p.filter(|p| p.active()) {
            out.push(
                Row::new(Glyph::Icon(icon), t.accent, crate::i18n::tr(title))
                    .value(of(p.done, p.total))
                    .fraction(fraction(p.done, p.total))
                    .detail(p.eta_secs.map(eta).unwrap_or_default()),
            );
        }
    }
    if out.is_empty() {
        let last = a
            .scan
            .as_ref()
            .and_then(|s| s.last_scan)
            .map(|at| format!("{} {}", crate::i18n::tr("Library folders were scanned"), ago(at)))
            .unwrap_or_default();
        out.push(Row::new(Glyph::Icon(Icon::Check), t.text_dim, crate::i18n::tr("Nothing to do right now")).detail(last));
    }
    if let Some(s) = a.scan.as_ref().filter(|s| s.problems > 0) {
        let what = crate::i18n::tr(if s.problems == 1 { "file or folder couldn't be read" } else { "files or folders couldn't be read" });
        out.push(
            Row::new(Glyph::Alert, t.caution, format!("{} {what}", count_label(s.problems)))
                .detail(crate::i18n::tr("They're listed on the server's admin page.")),
        );
    }
    out
}

fn fraction(done: u64, total: u64) -> f32 {
    if total == 0 { 0.0 } else { (done as f64 / total as f64) as f32 }
}

/// "about 5 min left".
fn eta(secs: u64) -> String {
    match secs {
        0..45 => crate::i18n::tr("less than a minute left").to_string(),
        45..5400 => format!("{} {} {}", crate::i18n::tr("about"), secs.div_ceil(60), crate::i18n::tr("min left")),
        _ => format!("{} {} h {} min {}", crate::i18n::tr("about"), secs / 3600, secs % 3600 / 60, crate::i18n::tr("left")),
    }
}

/// "5 min ago" for a unix time.
fn ago(unix_secs: u64) -> String {
    let now = (crate::now_ms() / 1000.0) as u64;
    match now.saturating_sub(unix_secs) {
        0..60 => crate::i18n::tr("just now").to_string(),
        s @ 60..3600 => format!("{} min {}", s / 60, crate::i18n::tr("ago")),
        s @ 3600..86400 => format!("{} h {}", s / 3600, crate::i18n::tr("ago")),
        s => format!("{} {} {}", s / 86400, crate::i18n::tr("days"), crate::i18n::tr("ago")),
    }
}

// -------------------------------------------------------------------------------------- Storage

/// What this library takes (the accent's dot and share of the bar), the rest of the server's disk
/// in use (grey) and what is free; then the photos read in place from library folders.
fn storage(ui: &mut egui::Ui, t: &Tokens, u: &proto::Usage) {
    const BAR: f32 = 6.0;
    ui.add_space(12.0);
    let w = ui.available_width();
    let value_g = ui.painter().layout_no_wrap(bytes_label(u.stored()), t.semibold(15.0), t.text);
    let label_g = one_line(ui, crate::i18n::tr("This library"), t.font(15.0), t.text, w - 14.0 - value_g.size().x - 10.0);
    let (r, _) = ui.allocate_exact_size(vec2(w, label_g.size().y.max(value_g.size().y)), Sense::hover());
    let p = ui.painter();
    p.circle_filled(pos2(r.left() + 4.0, r.center().y), 4.0, t.accent);
    p.galley(pos2(r.left() + 14.0, r.center().y - label_g.size().y / 2.0), label_g, t.text);
    p.galley(pos2(r.right() - value_g.size().x, r.center().y - value_g.size().y / 2.0), value_g, t.text);
    if let Some(d) = u.disk {
        ui.add_space(10.0);
        let (r, _) = ui.allocate_exact_size(vec2(w, BAR), Sense::hover());
        let total = d.total.max(1) as f32;
        let used = d.total.saturating_sub(d.free);
        let at = |bytes: u64| r.left() + r.width() * (bytes as f32 / total).clamp(0.0, 1.0);
        let p = ui.painter();
        p.rect_filled(r, BAR / 2.0, track(ui.ctx(), t));
        p.rect_filled(Rect::from_min_max(r.min, pos2(at(used), r.bottom())), BAR / 2.0, t.text_dim.gamma_multiply(0.75));
        let ours = u.stored().min(used);
        if ours > 0 {
            p.rect_filled(Rect::from_min_size(r.min, vec2((at(ours) - r.left()).max(BAR), BAR)), BAR / 2.0, t.accent);
        }
        ui.add_space(8.0);
        text(ui, 12.5, t.text_dim, format!("{} {} {}", bytes_label(d.free), crate::i18n::tr("free of"), bytes_label(d.total)));
    }
    if u.folders.files > 0 {
        ui.add_space(3.0);
        text(ui, 12.5, t.text_dim, format!("{} {}", count_label(u.folders.files), crate::i18n::tr("photos from library folders, not copied")));
    }
    ui.add_space(12.0);
}

// --------------------------------------------------------------------------------------- Footer

/// Pause (or Resume) and Sync Now side by side, then the gear (Settings ▸ Sync).
fn footer(ui: &mut egui::Ui, t: &Tokens, v: &View, act: &mut Option<Act>) {
    const GAP: f32 = 8.0;
    let paused = v.state == "paused";
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), BUTTON_H), Sense::hover());
    let gear = Rect::from_min_size(pos2(r.right() - BUTTON_H, r.top()), vec2(BUTTON_H, BUTTON_H));
    let half = ((gear.left() - GAP - r.left() - GAP) / 2.0).max(40.0);
    let pause = Rect::from_min_size(r.min, vec2(half, BUTTON_H));
    let now = Rect::from_min_size(pos2(pause.right() + GAP, r.top()), vec2(half, BUTTON_H));
    let (label, icon) = if paused { ("Resume", Icon::Play) } else { ("Pause", Icon::Pause) };
    if capsule(ui, t, pause, "syncPause", label, Face::Label(Some(icon)), false) {
        *act = Some(Act::PauseToggle);
    }
    if capsule(ui, t, now, "syncNow", "Sync Now", Face::Label(Some(Icon::Refresh)), true) {
        *act = Some(Act::SyncNow);
    }
    if capsule(ui, t, gear, "syncSettings", "Sync Settings", Face::Icon(Icon::Gear), false) {
        *act = Some(Act::Settings);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ink_reads_on_its_fill() {
        assert_eq!(ink_on(Color32::from_rgb(0xff, 0xd6, 0x0a)), Color32::BLACK, "dark mode's yellow");
        assert_eq!(ink_on(Color32::from_rgb(0xff, 0x95, 0x00)), Color32::WHITE, "light mode's orange");
        assert_eq!(ink_on(Color32::from_rgb(0xe3, 0xa8, 0x3c)), Color32::BLACK, "the desktop's amber");
        assert_eq!(ink_on(Color32::from_rgb(0xff, 0x3b, 0x30)), Color32::WHITE);
    }

    #[test]
    fn rows_say_how_far() {
        let r = Row::new(Glyph::Alert, Color32::RED, "x").value("1 of 2").fraction(0.5).detail("").detail("why");
        assert_eq!((r.value.as_str(), r.fraction, r.detail.len()), ("1 of 2", Some(0.5), 1), "an empty detail adds no line");
        assert!(fraction(1, 0) <= 0.0, "nothing to do is no progress, not a division by zero");
        assert_eq!(of(26, 182), "26 of 182");
        assert_eq!((photos(1).as_str(), photos(1200).as_str()), ("1 photo", "1,200 photos"));
    }
}
