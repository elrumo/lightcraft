//! The sync status popover: what the cloud button (the top bar's, or the phone grid's) opens, as
//! Lightroom's mobile app shows what its cloud is doing. In one panel:
//!
//! - **where syncing stands** — up to date, syncing, paused, signed out, a problem, or the choice a
//!   sign-in is waiting for (this library and the server's both have photos);
//! - **On this device** — changes waiting to go, photos being uploaded and files being downloaded
//!   ("12 of 56", with the names of the ones moving now);
//! - **On the server** — what the server is doing for this library: scanning its library folders,
//!   building previews, indexing photos for search (`GET /api/activity`);
//! - **Storage** — what the library takes on the server and how much room its disk has left;
//! - **Pause / Sync Now / Settings**.
//!
//! The panel is a pull-down of [`mobile`] (a tap outside closes it) kept current while it is open:
//! the engine asks the server again every couple of seconds only while somebody looks.

use std::time::Duration;

use egui::{Color32, Margin, Rect, Sense, Stroke, pos2, vec2};
use lightcraft_catalog::sync::proto;
use lightcraft_engine::sync::{ConflictInfo, Transfer, Transfers};
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
/// A tappable row's height.
const ROW: f32 = 48.0;

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
        // (an area lays its content out in the size it had last frame, and the contents change size
        // as answers come in: the scroll area is given its own room, or it could never grow)
        ui.set_max_height(max_h);
        egui::ScrollArea::vertical().id_salt("syncStatusScroll").max_height(max_h).auto_shrink([false, true]).show(ui, |ui| {
            act = content(app, ui);
        });
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

/// The panel's contents; the press, if any.
fn content(app: &mut LightcraftApp, ui: &mut egui::Ui) -> Option<Act> {
    let t = Tokens::get(ui.ctx());
    let v = View::read(app);
    let mut act = None;
    ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
    padded(ui, 14.0, |ui| header(ui, &t, &v, &mut act));
    if let Some((n, why)) = &v.unsaved {
        rule(ui, &t);
        let text = format!(
            "{n} change{} saved in memory but not written to disk: {why}. LightCraft retries automatically; quitting now would lose {}.",
            if *n == 1 { "" } else { "s" },
            if *n == 1 { "it" } else { "them" }
        );
        padded(ui, 12.0, |ui| banner(ui, &t, "unsaved", "Changes aren't saved", &text));
    }
    if let Some(d) = v.usage.as_ref().and_then(|u| u.disk).filter(low) {
        rule(ui, &t);
        let text = format!(
            "Only {} of its {} disk is left. New photos may not upload until some space is freed on the server.",
            bytes_label(d.free),
            bytes_label(d.total)
        );
        padded(ui, 12.0, |ui| banner(ui, &t, "serverDisk", "The server is almost out of space", &text));
    }
    if v.joined() {
        rule(ui, &t);
        padded(ui, 12.0, |ui| device(ui, &t, &v));
        rule(ui, &t);
        padded(ui, 12.0, |ui| server(ui, &t, &v));

        if let Some(u) = &v.usage {
            rule(ui, &t);
            padded(ui, 12.0, |ui| storage(ui, &t, &v, u));
        }
        rule(ui, &t);
        footer(ui, &t, &v, &mut act);
    }
    act
}

/// The content in a column inset 16 pt from the panel's sides.
fn padded(ui: &mut egui::Ui, vertical: f32, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::NONE.inner_margin(Margin { left: 16, right: 16, top: vertical as i8, bottom: vertical as i8 }).show(ui, |ui| {
        ui.set_width(ui.available_width());
        add(ui);
    });
}

fn rule(ui: &mut egui::Ui, t: &Tokens) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(r.x_range(), r.center().y, Stroke::new(1.0, t.divider));
}

/// A small heading over a section.
fn heading(ui: &mut egui::Ui, t: &Tokens, text: &str) {
    ui.label(egui::RichText::new(crate::i18n::tr(text).to_uppercase()).size(11.5).color(t.text_dim));
    ui.add_space(6.0);
}

/// Wrapped text.
fn text(ui: &mut egui::Ui, size: f32, color: Color32, s: impl Into<String>) {
    ui.label(egui::RichText::new(s.into()).size(size).color(color));
}

/// A problem: a red `!`, what it is and why.
fn banner(ui: &mut egui::Ui, t: &Tokens, id: &str, title: &str, why: &str) {
    let w = ui.available_width();
    let top = ui.cursor().top();
    ui.horizontal_top(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::hover());
        ui.painter().circle_filled(r.center(), 11.0, t.reject);
        ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, "!", t.semibold(13.0), Color32::WHITE);
        ui.add_space(10.0);
        ui.vertical(|ui| {
            ui.set_width(w - 38.0);
            ui.label(egui::RichText::new(crate::i18n::tr(title)).font(t.semibold(14.5)).color(t.text));
            text(ui, 13.0, t.text_dim, why);
        });
    });
    register(ui.ctx(), format!("banner:{id}"), Rect::from_min_max(pos2(ui.min_rect().left(), top), pos2(ui.min_rect().right(), ui.cursor().top())));
}

/// The server's disk is nearly full: less than 1 GB or 2 % left.
fn low(d: &proto::Disk) -> bool {
    d.total > 0 && (d.free < 1_000_000_000 || d.free.saturating_mul(50) < d.total)
}

/// A button as a row of its own (a whole-width tap target): `icon`, `label`, true when pressed.
fn button(ui: &mut egui::Ui, t: &Tokens, id: &str, icon: Option<Icon>, label: &str, color: Color32) -> bool {
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    register(ui.ctx(), format!("button:{id}"), r);
    let fill = if resp.is_pointer_button_down_on() { t.pressed } else { t.hover };
    ui.painter().rect_filled(r, 9.0, fill);
    let g = ui.painter().layout_no_wrap(crate::i18n::tr(label).to_string(), t.semibold(15.0), color);
    let icon_w = if icon.is_some() { 24.0 } else { 0.0 };
    let x = r.center().x - (g.size().x + icon_w) / 2.0;
    if let Some(icon) = icon {
        paint(ui.painter(), Rect::from_center_size(pos2(x + 9.0, r.center().y), vec2(18.0, 18.0)), icon, color);
    }
    ui.painter().galley(pos2(x + icon_w, r.center().y - g.size().y / 2.0), g, color);
    resp.clicked()
}

// -------------------------------------------------------------------------------------- header

/// The cloud, how syncing stands, and what to do about it.
fn header(ui: &mut egui::Ui, t: &Tokens, v: &View, act: &mut Option<Act>) {
    let host = &v.host;
    let (title, body, color): (String, String, Color32) = match v.state {
        "off" => (
            crate::i18n::tr("This library is only on this device").to_string(),
            crate::i18n::tr("Sync it with your own LightCraft server to use its photos and edits on your other devices.").to_string(),
            t.text,
        ),
        "signedOut" => (
            format!("{} {host}", crate::i18n::tr("Signed out of")),
            v.error.clone().unwrap_or_else(|| {
                crate::i18n::tr("Sign in again to keep syncing. Changes made meanwhile wait on this device; nothing is lost.").to_string()
            }),
            t.caution,
        ),
        "conflict" => (format!("{host} {}", crate::i18n::tr("has a library already")), conflict_text(v.conflict.as_ref()), t.caution),
        "paused" => (
            crate::i18n::tr("Syncing is paused").to_string(),
            format!("{} {host}.", crate::i18n::tr("Changes wait on this device until you resume. Server:")),
            t.text,
        ),
        "error" => (
            format!("{} {host}", crate::i18n::tr("Can't sync with")),
            format!("{} {}", v.error.clone().unwrap_or_default(), crate::i18n::tr("LightCraft keeps trying; changes wait on this device.")),
            t.caution,
        ),
        "syncing" => (format!("{} {host}", crate::i18n::tr("Syncing with")), summary(v), t.accent),
        _ => (
            format!("{} {host}", crate::i18n::tr("Up to date with")),
            format!("{} {}", count_label(v.photos as u64), crate::i18n::tr("photos")),
            t.text,
        ),
    };
    let problem = matches!(v.state, "error" | "signedOut" | "conflict");
    let w = ui.available_width();
    ui.horizontal_top(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::hover());
        paint(ui.painter(), r.shrink(3.0), Icon::Cloud, if problem { t.caution } else { color });
        if problem {
            let c = r.right_top() + vec2(-3.0, 6.0);
            ui.painter().circle_filled(c, 7.0, t.reject);
            ui.painter().text(c, egui::Align2::CENTER_CENTER, "!", t.semibold(10.0), Color32::WHITE);
        }
        ui.add_space(10.0);
        ui.vertical(|ui| {
            ui.set_width(w - 32.0 - 10.0);
            ui.label(egui::RichText::new(title).font(t.semibold(16.0)).color(t.text));
            ui.add_space(2.0);
            // (the cloud and its badge say it is a problem; an error's own words stand out)
            text(ui, 13.5, if v.state == "error" { t.caution } else { t.text_dim }, body);
        });
    });
    let (label, what) = match v.state {
        "off" => ("Set Up Sync…", Some(Act::Settings)),
        "signedOut" => ("Sign In…", Some(Act::SignIn)),
        "conflict" => ("Choose What to Do…", Some(Act::Choose)),
        _ => ("", None),
    };
    if let Some(what) = what {
        ui.add_space(12.0);
        if button(ui, t, "syncStatusPrimary", None, label, t.accent) {
            *act = Some(what);
        }
    }
}

/// The state line of a conflict: the two libraries' sizes.
fn conflict_text(c: Option<&ConflictInfo>) -> String {
    match c {
        Some(c) => format!(
            "{} {} {}, {} {} {}. {}",
            crate::i18n::tr("This library has"),
            count_label(c.here.photos as u64),
            crate::i18n::tr("photos"),
            crate::i18n::tr("the server's has"),
            count_label(c.there.photos as u64),
            crate::i18n::tr("photos"),
            crate::i18n::tr("Nothing syncs until you choose how to combine them.")
        ),
        None => crate::i18n::tr("Nothing syncs until you choose how to combine the two libraries.").to_string(),
    }
}

/// "3 changes to send, 12 of 56 uploaded, 30 files to download" for the header while syncing.
fn summary(v: &View) -> String {
    let t = &v.transfers;
    let mut parts = Vec::new();
    if t.pending > 0 {
        parts.push(format!(
            "{} {}",
            count_label(t.pending as u64),
            crate::i18n::tr(if t.pending == 1 { "change to send" } else { "changes to send" })
        ));
    }
    if t.uploads > 0 {
        parts.push(format!("{} {}", count_label(t.uploads as u64), crate::i18n::tr(if t.uploads == 1 { "upload left" } else { "uploads left" })));
    }
    if t.downloads > 0 {
        parts.push(format!(
            "{} {}",
            count_label(t.downloads as u64),
            crate::i18n::tr(if t.downloads == 1 { "download left" } else { "downloads left" })
        ));
    }
    if parts.is_empty() { crate::i18n::tr("Checking with the server…").to_string() } else { parts.join(" · ") }
}

// ------------------------------------------------------------------------------- On this device

fn device(ui: &mut egui::Ui, t: &Tokens, v: &View) {
    heading(ui, t, "On this device");
    let tr = &v.transfers;
    let mut shown = false;
    if tr.pending > 0 {
        shown = true;
        task(ui, t, Icon::Refresh, "Sending changes", &count_label(tr.pending as u64), None, &[], t.accent);
    }
    if tr.uploads > 0 {
        shown = true;
        let total = tr.uploads_done + tr.uploads;
        let running = names(&tr.running, true);
        task(
            ui,
            t,
            Icon::Upload,
            "Uploading photos",
            &format!("{} {} {}", count_label(tr.uploads_done as u64), crate::i18n::tr("of"), count_label(total as u64)),
            Some(tr.uploads_done as f32 / total.max(1) as f32),
            &running,
            t.accent,
        );
    }
    if tr.downloads > 0 {
        shown = true;
        let total = tr.downloads_done + tr.downloads;
        let running = names(&tr.running, false);
        let what = match tr.running.iter().filter(|r| !r.upload).map(|r| r.what).collect::<std::collections::BTreeSet<_>>().iter().next() {
            Some(&"original") => "Downloading originals",
            Some(&"smart preview") => "Downloading previews to edit",
            _ => "Downloading previews",
        };
        task(
            ui,
            t,
            Icon::Download,
            what,
            &format!("{} {} {}", count_label(tr.downloads_done as u64), crate::i18n::tr("of"), count_label(total as u64)),
            Some(tr.downloads_done as f32 / total.max(1) as f32),
            &running,
            t.accent,
        );
    }
    if let Some(e) = &tr.error {
        shown = true;
        ui.add_space(4.0);
        let n = count_label(tr.failing.max(1) as u64);
        let what = if tr.failing == 1 { "file couldn't" } else { "files couldn't" };
        text(ui, 12.5, t.caution, format!("{n} {what} be sent or received: {e}. LightCraft tries again every minute."));
    }
    if !shown {
        text(ui, 14.0, t.text_dim, crate::i18n::tr("Everything on this device is up to date."));
    }
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

/// One piece of work: an icon, what it is, how far (on the right), a bar when it has a fraction,
/// and lines of detail under it.
#[allow(clippy::too_many_arguments)]
fn task(ui: &mut egui::Ui, t: &Tokens, icon: Icon, title: &str, value: &str, fraction: Option<f32>, detail: &[String], tint: Color32) {
    const ICON_W: f32 = 28.0;
    let w = ui.available_width();
    let value_g = ui.painter().layout_no_wrap(value.to_string(), t.font(13.5), t.text_dim);
    let title_g = ui.painter().layout(crate::i18n::tr(title).to_string(), t.font(15.0), t.text, (w - ICON_W - value_g.size().x - 12.0).max(60.0));
    let top_h = title_g.size().y.max(value_g.size().y).max(20.0);
    let detail_g: Vec<_> = detail.iter().map(|d| ui.painter().layout(d.clone(), t.font(12.5), t.text_dim, w - ICON_W)).collect();
    let detail_h: f32 = detail_g.iter().map(|g| g.size().y + 2.0).sum();
    let bar_h = if fraction.is_some() { 12.0 } else { 0.0 };
    let h = top_h + bar_h + if detail_g.is_empty() { 0.0 } else { detail_h + 2.0 } + 8.0;
    let (r, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let p = ui.painter();
    paint(p, Rect::from_center_size(pos2(r.left() + 9.0, r.top() + top_h / 2.0), vec2(18.0, 18.0)), icon, tint);
    p.galley(pos2(r.left() + ICON_W, r.top() + (top_h - title_g.size().y) / 2.0), title_g, t.text);
    p.galley(pos2(r.right() - value_g.size().x, r.top() + (top_h - value_g.size().y) / 2.0), value_g, t.text_dim);
    let mut y = r.top() + top_h;
    if let Some(f) = fraction {
        let track = Rect::from_min_size(pos2(r.left() + ICON_W, y + 5.0), vec2(w - ICON_W, 4.0));
        p.rect_filled(track, 2.0, t.track);
        let mut fill = track;
        fill.set_width((track.width() * f.clamp(0.0, 1.0)).max(if f > 0.0 { 3.0 } else { 0.0 }));
        p.rect_filled(fill, 2.0, tint);
        y += bar_h;
    }
    y += 2.0;
    for g in detail_g {
        let gh = g.size().y;
        p.galley(pos2(r.left() + ICON_W, y), g, t.text_dim);
        y += gh + 2.0;
    }
}

// ------------------------------------------------------------------------------- On the server

fn server(ui: &mut egui::Ui, t: &Tokens, v: &View) {
    heading(ui, t, "On the server");
    let Some(a) = &v.activity else {
        match &v.activity_error {
            Some(e) => text(ui, 13.5, t.text_dim, format!("{} {e}", crate::i18n::tr("Can't tell what the server is doing:"))),
            None => text(ui, 14.0, t.text_dim, crate::i18n::tr("Asking the server…")),
        }
        return;
    };
    let mut shown = false;
    if let Some(s) = a.scan.as_ref().filter(|s| s.scanning) {
        shown = true;
        let title = "Scanning library folders";
        match s.phase.as_str() {
            "reading" => {
                let mut detail = vec![crate::i18n::tr("Reading new photos").to_string()];
                detail.extend(s.eta_secs.map(eta));
                task(
                    ui,
                    t,
                    Icon::Folder,
                    title,
                    &format!("{} {} {}", count_label(s.done), crate::i18n::tr("of"), count_label(s.todo)),
                    Some(fraction(s.done, s.todo)),
                    &[detail.join(" · ")],
                    t.accent,
                );
            }
            "listing" => task(
                ui,
                t,
                Icon::Folder,
                title,
                &format!("{} {}", count_label(s.files), crate::i18n::tr("found")),
                None,
                &[crate::i18n::tr("Looking for photos").to_string()],
                t.accent,
            ),
            _ => task(ui, t, Icon::Folder, title, "", None, &[crate::i18n::tr("Finishing up").to_string()], t.accent),
        }
    }
    if a.previews.active() {
        shown = true;
        progress(ui, t, Icon::Photos, "Building previews", &a.previews);
    }
    for (icon, title, p) in [
        (Icon::Search, "Indexing photos for search", a.search),
        (Icon::Info, "Reading the text in photos", a.text),
        (Icon::Subject, "Finding people in photos", a.faces),
    ] {
        if let Some(p) = p.filter(|p| p.active()) {
            shown = true;
            progress(ui, t, icon, title, &p);
        }
    }
    if !shown {
        let last = a
            .scan
            .as_ref()
            .and_then(|s| s.last_scan)
            .map(|at| format!(" {} {}.", crate::i18n::tr("Library folders were scanned"), ago(at)))
            .unwrap_or_default();
        text(ui, 14.0, t.text_dim, format!("{}{last}", crate::i18n::tr("The server isn't doing anything for this library right now.")));
    }
    if let Some(s) = a.scan.as_ref().filter(|s| s.problems > 0) {
        ui.add_space(4.0);
        text(
            ui,
            12.5,
            t.caution,
            format!(
                "{} {}",
                count_label(s.problems),
                crate::i18n::tr("files or folders in the library folders couldn't be read (see the admin page).")
            ),
        );
    }
}

fn progress(ui: &mut egui::Ui, t: &Tokens, icon: Icon, title: &str, p: &proto::Progress) {
    let detail: Vec<String> = p.eta_secs.map(eta).into_iter().collect();
    task(
        ui,
        t,
        icon,
        title,
        &format!("{} {} {}", count_label(p.done), crate::i18n::tr("of"), count_label(p.total)),
        Some(fraction(p.done, p.total)),
        &detail,
        t.accent,
    );
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

// ------------------------------------------------------------------------------------ Storage

fn storage(ui: &mut egui::Ui, t: &Tokens, v: &View, u: &proto::Usage) {
    heading(ui, t, "Storage");
    let line = format!(
        "{} {} {}",
        bytes_label(u.stored()),
        crate::i18n::tr("used on"),
        if v.host.is_empty() { crate::i18n::tr("the server").to_string() } else { v.host.clone() }
    );
    text(ui, 15.0, t.text, line);
    if let Some(d) = u.disk {
        ui.add_space(6.0);
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 6.0), Sense::hover());
        let total = d.total.max(1) as f32;
        let used = d.total.saturating_sub(d.free);
        let at = |bytes: u64| r.left() + r.width() * (bytes as f32 / total).clamp(0.0, 1.0);
        let p = ui.painter();
        p.rect_filled(r, 3.0, t.track);
        p.rect_filled(Rect::from_min_max(r.min, pos2(at(used), r.bottom())), 3.0, t.text_disabled);
        let ours = u.stored().min(used);
        p.rect_filled(Rect::from_min_size(r.min, vec2((at(ours) - r.left()).max(if ours > 0 { 3.0 } else { 0.0 }), r.height())), 3.0, t.accent);
        ui.add_space(6.0);
        text(ui, 12.5, t.text_dim, format!("{} {} {}", bytes_label(d.free), crate::i18n::tr("free of"), bytes_label(d.total)));
    }
    if u.folders.files > 0 {
        ui.add_space(4.0);
        text(
            ui,
            12.5,
            t.text_dim,
            format!("{} {}", count_label(u.folders.files), crate::i18n::tr("photos are read from library folders on the server (not copied).")),
        );
    }
}

// ------------------------------------------------------------------------------------- Footer

/// Pause (or resume), Sync Now and Settings, side by side.
fn footer(ui: &mut egui::Ui, t: &Tokens, v: &View, act: &mut Option<Act>) {
    let paused = v.state == "paused";
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::hover());
    let cell = |x0: f32, x1: f32| Rect::from_min_max(pos2(r.left() + x0, r.top()), pos2(r.left() + x1, r.bottom()));
    let w = r.width();
    let gear = cell(w - 52.0, w);
    let now = cell(w - 52.0 - 112.0, w - 52.0);
    let pause = cell(0.0, w - 52.0 - 112.0);
    let press = |rect: Rect, id: &str, label: &str, icon: Icon, left: bool, color: Color32, ui: &mut egui::Ui| -> bool {
        let resp = ui.interact(rect, egui::Id::new(("sync-status", id)), Sense::click());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
        register(ui.ctx(), format!("button:{id}"), rect);
        let c = if resp.is_pointer_button_down_on() { color.gamma_multiply(0.6) } else { color };
        let p = ui.painter();
        let icon_r = |x: f32| Rect::from_center_size(pos2(x, rect.center().y), vec2(20.0, 20.0));
        if label.is_empty() {
            paint(p, icon_r(rect.center().x), icon, c);
        } else {
            let g = p.layout_no_wrap(crate::i18n::tr(label).to_string(), t.font(15.0), c);
            let x0 = if left { rect.left() + 16.0 } else { rect.center().x - (g.size().x + 26.0) / 2.0 };
            paint(p, icon_r(x0 + 10.0), icon, c);
            p.galley(pos2(x0 + 26.0, rect.center().y - g.size().y / 2.0), g, c);
        }
        resp.clicked()
    };
    if press(
        pause,
        "syncPause",
        if paused { "Resume syncing" } else { "Pause syncing" },
        if paused { Icon::Play } else { Icon::Pause },
        true,
        t.text,
        ui,
    ) {
        *act = Some(Act::PauseToggle);
    }
    if press(now, "syncNow", "Sync Now", Icon::Refresh, false, t.accent, ui) {
        *act = Some(Act::SyncNow);
    }
    if press(gear, "syncSettings", "", Icon::Gear, false, t.text_dim, ui) {
        *act = Some(Act::Settings);
    }
}
