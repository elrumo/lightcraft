//! The choice after signing in to a sync server that already has a library, when this library has
//! photos too: two libraries that never met (their ids mean different photos), so nothing syncs until
//! the user says how to combine them (`sync.resolveConflict`, `docs/sync.md` → *When this library and
//! the server's both have photos*):
//!
//! - **Add this library to the server's** — this library's photos, edits and albums are uploaded into
//!   the server's library (photos both have are kept once) and the server's photos come here;
//! - **Use the server's library instead** — this library is replaced by the server's (a copy of its
//!   catalog is kept; the photo files on this computer are never touched);
//! - **Don't sync** — sign out; this library stays as it is.
//!
//! The body of `Dialog::SyncChoice`: a window on the desktop, a page on a phone. It can be put off
//! (Decide Later): syncing waits, the cloud button says why, and Settings ▸ Sync and the popover
//! offer the choice again.

use egui::{Color32, Margin, Sense, pos2, vec2};
use lightcraft_engine::sync::ConflictInfo;
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::sync_ui::count_label;
use crate::theme::Tokens;
use crate::widgets::register;

/// `1 photo`, `3,420 photos`.
fn photos(n: usize) -> String {
    format!("{} {}", count_label(n as u64), crate::i18n::tr(if n == 1 { "photo" } else { "photos" }))
}

fn albums(n: usize) -> String {
    format!("{} {}", count_label(n as u64), crate::i18n::tr(if n == 1 { "album" } else { "albums" }))
}

/// Draw the choice; true once one was made (the dialog closes). Waiting for no choice (it was made
/// elsewhere, or the sign-in ended) closes it too.
pub fn body(app: &mut LightcraftApp, ui: &mut egui::Ui) -> bool {
    // (a phone's page scrolls by itself; a window must fit the screen)
    if crate::is_compact(ui.ctx()) {
        return choices(app, ui);
    }
    let max_h = (ui.ctx().content_rect().height() - 190.0).max(260.0);
    ui.set_max_height(max_h);
    egui::ScrollArea::vertical().id_salt("syncChoiceScroll").max_height(max_h).auto_shrink([false, true]).show(ui, |ui| choices(app, ui)).inner
}

fn choices(app: &mut LightcraftApp, ui: &mut egui::Ui) -> bool {
    let t = Tokens::get(ui.ctx());
    let Some(info) = app.session.sync_conflict().cloned() else {
        ui.label(crate::i18n::tr("Nothing is waiting for a choice."));
        return true;
    };
    let mut chosen = None;
    ui.spacing_mut().item_spacing = vec2(8.0, 10.0);
    ui.label(
        egui::RichText::new(format!(
            "{} {} {} {}. {}",
            crate::i18n::tr("You signed in to"),
            info.server,
            crate::i18n::tr("as"),
            info.user,
            crate::i18n::tr(
                "The server already has a library and so does this computer, and they were never connected. Choose how to combine them; nothing syncs until you do."
            )
        ))
        .size(14.0)
        .color(t.text_label),
    );
    summary(ui, &t, &info);
    let new_here = info.here.photos.saturating_sub(info.shared);
    // (English only, like the other sentences with numbers in them)
    let mut add_text = if new_here > 0 {
        format!(
            "Uploads the {} the server doesn't have, with their edits, ratings, keywords and albums, and brings the server's {} here.",
            photos(new_here),
            photos(info.there.photos)
        )
    } else {
        format!(
            "Everything in this library is on the server already. This brings the server's {} here and adds what only this library knew: edits, ratings, keywords and albums.",
            photos(info.there.photos)
        )
    };
    if info.shared > 0 && new_here > 0 {
        add_text.push_str(&format!(
            " The {} {} on both stay as the server has them; anything only this library knew about them is added.",
            info.shared,
            if info.shared == 1 { "photo that is" } else { "photos that are" }
        ));
    }
    if option(ui, &t, "syncChoiceUpload", "Add this library to the server's", &add_text, "Add to Server's Library", Style::Primary) {
        chosen = Some("upload");
    }
    let replace_text = format!(
        "{} {}. {} {} {}",
        crate::i18n::tr("Replaces this library with the server's"),
        photos(info.there.photos),
        crate::i18n::tr("This library's"),
        photos(info.here.photos),
        crate::i18n::tr(
            "leave the library, but the files on this computer aren't touched, and a copy of this library's catalog is kept in its folder so nothing is lost."
        )
    );
    if option(ui, &t, "syncChoiceUseServer", "Use the server's library instead", &replace_text, "Use Server's Library", Style::Plain) {
        chosen = Some("useServer");
    }
    if option(
        ui,
        &t,
        "syncChoiceCancel",
        "Don't sync this library",
        crate::i18n::tr("Signs out. This library stays only on this computer, as it is."),
        "Sign Out",
        Style::Plain,
    ) {
        chosen = Some("cancel");
    }
    ui.label(
        egui::RichText::new(crate::i18n::tr(
            "To keep both libraries apart, choose Decide Later or Sign Out, then open a new, empty library (Settings > General > Open Library…) and sign in from there.",
        ))
        .size(12.5)
        .color(t.text_dim),
    );
    let Some(choice) = chosen else { return false };
    match app.run("sync.resolveConflict", json!({"choice": choice})) {
        Ok(r) => {
            let ctx = ui.ctx().clone();
            app.toast(&ctx, outcome(choice, &r["resolved"], &info));
            true
        }
        Err(e) => {
            let ctx = ui.ctx().clone();
            app.toast(&ctx, e);
            false
        }
    }
}

/// What was done, in a sentence for the toast.
fn outcome(choice: &str, resolved: &Value, info: &ConflictInfo) -> String {
    match choice {
        "upload" => {
            let added = resolved["joined"]["photosAdded"].as_u64().unwrap_or(0) as usize;
            format!("{} {} {}", crate::i18n::tr("Adding"), photos(added), crate::i18n::tr("to the server's library…"))
        }
        "useServer" => format!("{} {}", crate::i18n::tr("Now using the server's library:"), photos(info.there.photos)),
        _ => crate::i18n::tr("Signed out; this library stays on this computer.").to_string(),
    }
}

/// This library and the server's, side by side, and the photos both have.
fn summary(ui: &mut egui::Ui, t: &Tokens, info: &ConflictInfo) {
    let compact = crate::is_compact(ui.ctx());
    let fill = if compact { t.cell_selected } else { t.inset };
    let w = ui.available_width();
    let gap = 8.0;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        for (title, counts, edited) in [("This library", info.here, true), ("On the server", info.there, false)] {
            egui::Frame::NONE.fill(fill).corner_radius(10.0).inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
                // (the row is horizontal: each card stacks its own lines)
                ui.vertical(|ui| {
                    ui.set_width((w - gap) / 2.0 - 24.0);
                    ui.spacing_mut().item_spacing.y = 3.0;
                    ui.label(egui::RichText::new(crate::i18n::tr(title).to_uppercase()).size(11.0).color(t.text_dim));
                    ui.label(egui::RichText::new(photos(counts.photos)).font(t.semibold(16.0)).color(t.text));
                    ui.label(egui::RichText::new(albums(counts.albums)).size(13.0).color(t.text_dim));
                    if edited && counts.edited > 0 {
                        let n = count_label(counts.edited as u64);
                        ui.label(egui::RichText::new(format!("{n} {}", crate::i18n::tr("edited"))).size(13.0).color(t.text_dim));
                    }
                });
            });
        }
    });
    if info.shared > 0 {
        let line = if info.shared == 1 {
            "1 of this library's photos is on the server too (the same file).".to_string()
        } else {
            format!("{} of this library's photos are on the server too (the same files).", count_label(info.shared as u64))
        };
        ui.label(egui::RichText::new(line).size(13.0).color(t.text_dim));
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Style {
    /// The one most people want: filled with the accent colour.
    Primary,
    Plain,
}

/// One way out: its name, what it does, and the button that does it. True when pressed.
fn option(ui: &mut egui::Ui, t: &Tokens, id: &str, title: &str, text: &str, button: &str, style: Style) -> bool {
    let compact = crate::is_compact(ui.ctx());
    let fill = if compact { t.cell_selected } else { t.inset };
    let mut pressed = false;
    egui::Frame::NONE.fill(fill).corner_radius(10.0).inner_margin(Margin::symmetric(14, 12)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 6.0;
        ui.label(egui::RichText::new(crate::i18n::tr(title)).font(t.semibold(15.5)).color(t.text));
        ui.label(egui::RichText::new(text).size(13.5).color(t.text_dim));
        ui.add_space(4.0);
        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::click());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, button));
        register(ui.ctx(), format!("button:{id}"), r);
        let down = resp.is_pointer_button_down_on();
        let (bg, fg) = match style {
            Style::Primary => (if down { t.accent.gamma_multiply(0.7) } else { t.accent }, Color32::WHITE),
            Style::Plain => (if down { t.pressed } else { t.hover }, t.text),
        };
        ui.painter().rect_filled(r, 9.0, bg);
        let g = ui.painter().layout_no_wrap(crate::i18n::tr(button).to_string(), t.semibold(15.0), fg);
        ui.painter().galley(pos2(r.center().x - g.size().x / 2.0, r.center().y - g.size().y / 2.0), g, fg);
        pressed = resp.clicked();
    });
    pressed
}
