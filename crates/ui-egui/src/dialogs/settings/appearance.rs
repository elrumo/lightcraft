use super::widgets::{choices, heading};
use super::*;

/// Appearance Mode (`button:settingsAppearance-{0,1,2}`: Auto, Dark, Light) above the light and
/// dark theme cards (`radio:theme-{id}`), each with a preview of the chosen theme. The card in
/// use is outlined. Applied on the next frame ([`LightcraftApp::sync_theme`]).
pub(crate) fn appearance(ui: &mut egui::Ui, t: &Tokens, s: &mut crate::state::AppSettings, system: Option<egui::Theme>) {
    use crate::state::{AppearanceMode as M, DarkTheme, LightTheme};
    heading(ui, t, crate::i18n::tr("Appearance"));
    row(ui, t, crate::i18n::tr("Appearance Mode"), |ui| {
        choices(ui, "settingsAppearance", &[(M::Auto, "Sync with System"), (M::Dark, "Dark Mode"), (M::Light, "Light Mode")], &mut s.appearance_mode);
    });
    card_end(ui, t);
    ui.add_space(8.0);
    let light_active = s.theme(system) == s.light_theme.kind();
    // (in a Settings section, out to the edges of the cards above and below)
    let mut area = ui.available_rect_before_wrap();
    if let Some(col) = column(ui) {
        area.min.x = col.left - CARD_PAD;
        area.max.x = col.right + CARD_PAD;
    }
    let available = area.width();
    let mut cards = |ui: &mut egui::Ui, width: f32| {
        ui.spacing_mut().item_spacing.x = 12.0;
        let light: Vec<_> = LightTheme::ALL.iter().map(|l| (*l, l.kind())).collect();
        theme_card(ui, t, "Light Theme", light_active, &light, &mut s.light_theme, &mut s.favorite_themes, width);
        let dark: Vec<_> = DarkTheme::ALL.iter().map(|d| (*d, d.kind())).collect();
        theme_card(ui, t, "Dark Theme", !light_active, &dark, &mut s.dark_theme, &mut s.favorite_themes, width);
    };
    ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
        if available < 440.0 {
            ui.vertical(|ui| cards(ui, available));
        } else {
            ui.horizontal_top(|ui| cards(ui, ((available - 12.0) / 2.0).floor()));
        }
    });
    ui.add_space(2.0);
}

/// One family's card: its title (and "Active" when it is the one shown), a preview of the
/// selected theme and a radio button per theme.
pub(crate) fn theme_card<V: PartialEq + Copy>(
    ui: &mut egui::Ui,
    t: &Tokens,
    title: &str,
    active: bool,
    options: &[(V, crate::theme::ThemeKind)],
    value: &mut V,
    favorites: &mut Vec<String>,
    width: f32,
) {
    egui::Frame::new()
        .fill(t.inset)
        .stroke(egui::Stroke::new(if active { 1.5 } else { 1.0 }, if active { t.accent } else { t.field_border }))
        .corner_radius(f32::from(t.control_radius) + 4.0)
        .inner_margin(8)
        .show(ui, |ui| {
            ui.vertical(|ui| {
                let inner = (width - 18.0).clamp(120.0, 320.0);
                ui.set_width(inner);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(crate::i18n::tr(title)).font(t.semibold(12.0)).color(t.text));
                    if active {
                        ui.label(RichText::new(crate::i18n::tr("In Use")).size(11.0).color(t.accent));
                    }
                });
                let shown = options.iter().find(|(v, _)| v == value).or(options.first()).map(|(_, k)| *k).unwrap_or_default();
                theme_preview(ui, shown, inner);
                for (v, kind) in options {
                    let row = egui::Rect::from_min_size(ui.cursor().min, egui::vec2(ui.available_width(), 24.0));
                    let hovered = ui.ctx().pointer_hover_pos().is_some_and(|p| row.contains(p));
                    ui.horizontal(|ui| {
                        let r = ui.radio_value(value, *v, crate::i18n::tr(kind.label()));
                        register(ui, format!("radio:theme-{}", kind.id()), r.rect);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let favorite = favorites.iter().any(|id| id == kind.id());
                            let r = ui
                                .add_visible(favorite || hovered, egui::Button::new("").min_size(egui::vec2(24.0, 24.0)).frame(false))
                                .on_hover_text(crate::i18n::tr(if favorite { "Remove from favorites" } else { "Add to favorites" }));
                            r.widget_info(|| {
                                egui::WidgetInfo::labeled(
                                    egui::Role::Button,
                                    true,
                                    format!(
                                        "{}: {}",
                                        crate::i18n::tr(if favorite { "Remove from favorites" } else { "Add to favorites" }),
                                        crate::i18n::tr(kind.label())
                                    ),
                                )
                            });
                            register(ui, format!("button:themeFavorite-{}", kind.id()), r.rect);
                            if favorite || hovered {
                                crate::rating::paint_star(ui.painter(), r.rect.shrink(4.0), favorite, if favorite { t.accent } else { t.text_dim });
                            }
                            if r.clicked() {
                                if favorite {
                                    favorites.retain(|id| id != kind.id());
                                } else {
                                    favorites.push(kind.id().into());
                                }
                            }
                        });
                    });
                }
            });
        });
}

/// A small LightCraft window painted with `kind`'s tokens: the top bar with its search field, the
/// photo with the filmstrip under it, the Edit panel (histogram and sliders), the tool strip and
/// the bottom bar. A Studio theme's preview shows its panels as cards on the backdrop.
pub(crate) fn theme_preview(ui: &mut egui::Ui, kind: crate::theme::ThemeKind, width: f32) {
    use egui::{Color32, Rect, Stroke, StrokeKind, pos2, vec2};
    let p = Tokens::for_kind(kind);
    let (rect, _) = ui.allocate_exact_size(vec2(width, (width * 0.56).round().min(130.0)), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    let bar = |y: f32, x: f32, w: f32, c: Color32| painter.rect_filled(Rect::from_min_size(pos2(x, y), vec2(w, 2.0)), 1.0, c);
    // (cards: the gap between them and their rounding, at this scale)
    let cards = p.cards();
    let (gap, round) = if cards { (2.0, 2.5) } else { (0.0, 0.0) };
    painter.rect_filled(rect, 3.0, if cards { p.backdrop } else { p.canvas });
    // top bar: sidebar and back icons, the centred search field, icons on the right
    let top = Rect::from_min_size(rect.min, vec2(rect.width(), 13.0));
    painter.rect_filled(top, 0.0, p.backdrop);
    for x in [6.0, 14.0, 20.0] {
        painter.rect_filled(Rect::from_min_size(top.min + vec2(x, 4.5), vec2(4.0, 4.0)), 1.0, p.icon);
    }
    let search = Rect::from_center_size(top.center(), vec2(rect.width() * 0.34, 8.0));
    painter.rect(search, if cards { 4.0 } else { 2.0 }, p.field, Stroke::new(1.0, p.field_border), StrokeKind::Inside);
    bar(search.center().y - 1.0, search.center().x - 9.0, 18.0, p.text_dim);
    for i in 0..4 {
        painter.rect_filled(Rect::from_min_size(pos2(top.right() - 10.0 - i as f32 * 9.0, top.top() + 4.5), vec2(4.0, 4.0)), 2.0, p.icon);
    }
    let body = Rect::from_min_max(pos2(rect.left() + gap, top.bottom()), pos2(rect.right() - gap, rect.bottom() - gap));
    // tool strip on the right edge, the first tool (Edit) active; the bottom bar runs under it
    // unless the photo is a card of its own, which holds the bottom bar
    let strip_bottom = if cards { body.bottom() } else { body.bottom() - 12.0 };
    let strip = Rect::from_min_max(pos2(body.right() - 12.0, body.top()), pos2(body.right(), strip_bottom));
    painter.rect_filled(strip, round, p.chrome);
    if !cards {
        painter.line_segment([strip.left_top(), strip.left_bottom()], Stroke::new(1.0, p.divider));
    }
    for i in 0..5 {
        let r = Rect::from_min_size(pos2(strip.left() + 3.0, strip.top() + 4.0 + i as f32 * 9.0), vec2(6.0, 6.0));
        if i == 0 {
            painter.rect_filled(r.expand(1.5), 1.5, p.tool_active);
        }
        painter.rect_stroke(r, 1.0, Stroke::new(1.0, if i == 0 { p.text } else { p.icon }), StrokeKind::Inside);
    }
    // Edit panel: histogram, then labelled sliders
    let panel_right = strip.left() - gap;
    let panel = Rect::from_min_max(pos2(panel_right - (rect.width() * 0.27).round(), body.top()), pos2(panel_right, strip_bottom));
    painter.rect_filled(panel, round, p.chrome);
    if !cards {
        painter.line_segment([panel.left_top(), panel.left_bottom()], Stroke::new(1.0, p.divider));
    }
    let hist = Rect::from_min_size(panel.min + vec2(4.0, 4.0), vec2(panel.width() - 8.0, 14.0));
    painter.rect_filled(hist, 1.0, p.inset);
    let curve = [0.2, 0.55, 0.9, 0.7, 0.5, 0.62, 0.4, 0.3, 0.18, 0.1];
    let mut pts = vec![hist.left_bottom()];
    for (i, h) in curve.iter().enumerate() {
        pts.push(pos2(hist.left() + hist.width() * i as f32 / (curve.len() - 1) as f32, hist.bottom() - hist.height() * h));
    }
    pts.push(hist.right_bottom());
    // the area under the curve, a column per stretch between two points
    for pair in pts.windows(2) {
        if let [a, b] = pair {
            let r = Rect::from_min_max(pos2(a.x, a.y.max(b.y)), pos2(b.x, hist.bottom()));
            painter.rect_filled(r, 0.0, p.track.gamma_multiply(0.5));
        }
    }
    painter.add(egui::Shape::line(pts, Stroke::new(1.0, p.text_dim)));
    let sliders = [0.55, 0.62, 0.35, 0.7, 0.6, 0.45];
    let mut y = hist.bottom() + 7.0;
    bar(y, panel.left() + 5.0, 14.0, p.text);
    y += 6.0;
    for v in sliders {
        if y + 7.0 > panel.bottom() {
            break;
        }
        bar(y, panel.left() + 5.0, 12.0, p.text_label);
        bar(y, panel.right() - 10.0, 5.0, p.text_dim);
        let track_y = y + 5.0;
        let (from, to) = (panel.left() + 5.0, panel.right() - 5.0);
        let x = from + (to - from) * v;
        if p.filled_sliders {
            // a rounded track, filled from its middle (the default) to the handle
            painter.line_segment([pos2(from, track_y), pos2(to, track_y)], Stroke::new(1.5, p.track));
            let mid = (from + to) / 2.0;
            painter.line_segment([pos2(mid.min(x), track_y), pos2(mid.max(x), track_y)], Stroke::new(1.5, p.thumb));
            painter.circle_filled(pos2(x, track_y), 2.2, p.thumb);
        } else {
            painter.line_segment([pos2(from, track_y), pos2(to, track_y)], Stroke::new(1.0, p.track));
            painter.circle_filled(pos2(x, track_y), 1.8, p.thumb);
        }
        y += 10.0;
    }
    // the photo card (its own colours: a photo looks the same in every theme) above the filmstrip,
    // above the bottom bar: view buttons, rating stars, a button
    let photo_card = Rect::from_min_max(pos2(body.left(), body.top()), pos2(panel.left() - gap, body.bottom()));
    let bottom = if cards {
        Rect::from_min_max(pos2(photo_card.left(), photo_card.bottom() - 12.0), photo_card.max)
    } else {
        Rect::from_min_max(pos2(rect.left(), rect.bottom() - 12.0), rect.max)
    };
    let film = Rect::from_min_max(pos2(photo_card.left(), bottom.top() - 18.0), pos2(photo_card.right(), bottom.top()));
    let stage = Rect::from_min_max(photo_card.min, pos2(photo_card.right(), film.top()));
    if cards {
        painter.rect_filled(photo_card, round, p.canvas);
    }
    painter.rect_filled(bottom, egui::CornerRadius { nw: 0, ne: 0, sw: round as u8, se: round as u8 }, p.chrome);
    for x in [6.0, 13.0, 20.0] {
        painter.rect_filled(Rect::from_min_size(bottom.min + vec2(x, 4.0), vec2(4.0, 4.0)), 1.0, p.icon);
    }
    let stars_x = bottom.center().x - 30.0;
    for i in 0..5 {
        painter.circle_filled(pos2(stars_x + i as f32 * 5.0, bottom.center().y), 1.4, p.star);
    }
    let copy = Rect::from_min_size(pos2(stars_x + 28.0, bottom.top() + 2.5), vec2(30.0, 7.0));
    painter.rect(copy, if cards { 3.5 } else { 3.0 }, p.button, Stroke::new(1.0, p.button_border), StrokeKind::Inside);
    let photo = Rect::from_center_size(stage.center(), vec2(stage.width() * 0.62, stage.height() - 8.0));
    painter.rect_filled(photo, 0.0, Color32::from_rgb(0x6f, 0x8f, 0xb4));
    let horizon = photo.top() + photo.height() * 0.62;
    painter.add(egui::Shape::convex_polygon(
        vec![
            pos2(photo.left(), horizon),
            pos2(photo.left() + photo.width() * 0.3, photo.top() + photo.height() * 0.3),
            pos2(photo.left() + photo.width() * 0.55, horizon),
        ],
        Color32::from_rgb(0x4a, 0x55, 0x66),
        Stroke::NONE,
    ));
    painter.add(egui::Shape::convex_polygon(
        vec![
            pos2(photo.left() + photo.width() * 0.35, horizon),
            pos2(photo.left() + photo.width() * 0.68, photo.top() + photo.height() * 0.2),
            pos2(photo.right(), horizon),
        ],
        Color32::from_rgb(0x5a, 0x63, 0x72),
        Stroke::NONE,
    ));
    painter.rect_filled(Rect::from_min_max(pos2(photo.left(), horizon), photo.max), 0.0, Color32::from_rgb(0x3d, 0x5a, 0x3a));
    painter.line_segment([film.left_top(), film.right_top()], Stroke::new(1.0, p.divider));
    for i in 0..5 {
        let cell = Rect::from_min_size(pos2(film.left() + 4.0 + i as f32 * 17.0, film.top() + 3.0), vec2(15.0, 12.0));
        if cell.right() > film.right() - 2.0 {
            break;
        }
        let selected = i == 1;
        painter.rect_filled(cell, 0.0, if selected { p.cell_selected } else { p.cell });
        let thumb = cell.shrink(2.5);
        painter.rect_filled(thumb, 0.0, Color32::from_rgb(0x6f + 8 * i as u8, 0x80, 0x8f));
        if selected {
            painter.rect_stroke(thumb, 0.0, Stroke::new(1.0, p.pick), StrokeKind::Outside);
        }
    }
}

// ---------------------------------------------------------------------------------------- Sync

/// Run a sync command, showing what went wrong.
pub(crate) fn sync_cmd(app: &mut LightcraftApp, ui: &egui::Ui, id: &str, p: Value) {
    if let Err(e) = app.run(id, p) {
        app.toast(ui.ctx(), e);
    }
}

pub(crate) fn sync_tab(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    // (the desktop section's title says it)
    if crate::is_compact(ui.ctx()) {
        heading(ui, t, crate::i18n::tr("Sync"));
    }
    hint(
        ui,
        t,
        crate::i18n::tr(
            "Share this library — photos, edits, albums and presets — with your other devices through your own LightCraft server (see docs/sync.md). Without one, everything stays on this computer.",
        ),
    );
    if app.services.sync_exec.is_none() {
        hint(ui, t, crate::i18n::tr("Sync isn't available here yet."));
        return;
    }
    if app.session.library.is_none() {
        hint(ui, t, crate::i18n::tr("Sync needs a saved library: open or create one in General first."));
        return;
    }
    let status = app.session.sync_state().map(|st| (st.status(), st.signed_in(), st.config.clone()));
    match status {
        Some((s, true, c)) => {
            if crate::is_compact(ui.ctx()) {
                heading(ui, t, crate::i18n::tr("Account"));
            }
            info(ui, t, crate::i18n::tr("Server"), &c.server, t.text);
            info(ui, t, crate::i18n::tr("User"), &c.user, t.text);
            let (tip, problem, _) = crate::sync_ui::cloud_status(app);
            let r = info(ui, t, crate::i18n::tr("Status"), &tip, if problem { t.caution } else { t.text });
            register(ui, "label:syncStatus", r);
            // signed in to a server that has a library already, and this one has photos: nothing
            // syncs until the user chooses (a dialog asked once; this is where to come back to it)
            let waiting = app.session.sync_conflict().is_some();
            // signing out waits for what is left to be sent: it can be cancelled, or done without waiting
            let leaving = c.leaving;
            // (a question left open that no longer applies, because the state moved on, goes)
            app.cloud_sync.sign_out =
                app.cloud_sync.sign_out.filter(|h| matches!((h, leaving), (SignOutHow::Wait, false) | (SignOutHow::Anyway, true)));
            if let Some(how) = app.cloud_sync.sign_out {
                sign_out_question(app, ui, t, how);
                return;
            }
            row(ui, t, "", |ui| {
                if leaving {
                    if action(ui, t, "syncSignOutCancel", "Cancel Sign Out", true, false) {
                        sync_cmd(app, ui, "sync.cancelSignOut", json!({}));
                    }
                } else if waiting {
                    if action(ui, t, "syncChoose", "Choose What to Do…", true, false) {
                        sync_cmd(app, ui, "dialog.syncChoice", json!({}));
                    }
                } else {
                    if action(ui, t, "syncNow", "Sync Now", true, false) {
                        sync_cmd(app, ui, "sync.now", json!({}));
                    }
                    if action(ui, t, "syncPause", if c.paused { "Resume Syncing" } else { "Pause Syncing" }, true, false) {
                        sync_cmd(app, ui, "sync.pause", json!({"on": !c.paused}));
                    }
                }
                if crate::is_compact(ui.ctx()) {
                    // (iOS: signing out on a card of its own)
                    card_end(ui, t);
                    ui.add_space(28.0);
                }
                if leaving {
                    if action(ui, t, "syncSignOutAnyway", "Sign Out Anyway…", true, true) {
                        app.cloud_sync.sign_out = Some(SignOutHow::Anyway);
                    }
                } else if action(ui, t, "syncSignOut", "Sign Out", true, true) {
                    if c.library.is_empty() {
                        // (this library never joined the server's: nothing of it is there, so it signs out and stays as it is)
                        sync_cmd(app, ui, "sync.signOut", json!({}));
                    } else {
                        app.cloud_sync.sign_out = Some(SignOutHow::Wait);
                    }
                }
            });
            if leaving {
                hint(
                    ui,
                    t,
                    crate::i18n::tr(
                        "Signing out waits until everything this device has made has reached the server, then removes the library from this device. Cancel stays signed in. Sign Out Anyway doesn't wait: what hasn't been sent is lost.",
                    ),
                );
                return;
            }
            if waiting {
                hint(
                    ui,
                    t,
                    crate::i18n::tr(
                        "This library and the server's both have photos. Add this library's photos to the server's, or use the server's library instead; nothing syncs until you choose.",
                    ),
                );
                return;
            }
            storage_section(app, ui, t, &c.server);
            heading(ui, t, crate::i18n::tr("On This Device"));
            let mut keep = c.store_originals;
            if check(ui, "sync.storeOriginals", &mut keep, "Store the originals of all photos on this device") {
                sync_cmd(app, ui, "sync.storeOriginalsLocally", json!({"on": keep}));
            }
            hint(
                ui,
                t,
                crate::i18n::tr(
                    "Otherwise photos from other devices come down as previews: small ones for the grid, an editable smart preview when you open a photo or make an album available offline (right-click it), and the original when you ask (Photo > Download Originals).",
                ),
            );
            let mut all = c.all_previews;
            let label = match app.session.sync_previews_left() {
                Some((n, bytes)) if n > 0 && !all => {
                    use crate::sync_ui::{bytes_label, count_label};
                    format!("Download all small previews ({} photos, about {})", count_label(n as u64), bytes_label(bytes))
                }
                _ => crate::i18n::tr("Download all small previews").to_string(),
            };
            if check(ui, "sync.allPreviews", &mut all, &label) {
                sync_cmd(app, ui, "sync.allPreviews", json!({"on": all}));
            }
            hint(
                ui,
                t,
                crate::i18n::tr(
                    "Otherwise the grid's small previews come down as you scroll to them. All of them: the whole library can be browsed offline.",
                ),
            );
            // the downloaded originals' budget (MB; 0: none); with every original kept it has no effect
            let mut budget = c.originals_budget_mb.unwrap_or(0);
            let sizes = [
                (0u64, "No limit"),
                (5_000, "5 GB"),
                (10_000, "10 GB"),
                (25_000, "25 GB"),
                (50_000, "50 GB"),
                (100_000, "100 GB"),
                (250_000, "250 GB"),
            ];
            if !c.store_originals && pick(ui, t, "syncOriginalsBudget", crate::i18n::tr("Originals limit"), &sizes, &mut budget) {
                sync_cmd(app, ui, "sync.originalsBudget", json!({"mb": (budget > 0).then_some(budget)}));
            }
            hint(
                ui,
                t,
                crate::i18n::tr(
                    "Originals downloaded from the server are deleted, the ones used longest ago first, when they take more than this. Only ones the server has; photos you are working on or made available offline keep theirs.",
                ),
            );
            let mut by_server = c.server_builds_previews();
            if check(ui, "sync.serverPreviews", &mut by_server, "Let the server build the previews of photos added here") {
                sync_cmd(app, ui, "sync.serverPreviews", json!({"on": by_server}));
            }
            hint(
                ui,
                t,
                crate::i18n::tr(
                    "The server decodes each original it is sent, so this device doesn't: that saves battery on a phone and keeps a browser tab responsive. Off: the previews are built here, before the photo goes up.",
                ),
            );
            let offline = s["offlineAlbums"].as_array().map_or(0, Vec::len);
            if offline > 0 {
                hint(ui, t, &format!("{offline} album(s) available offline"));
            }
            shared_links(app, ui, t);
        }
        other => {
            let known = other.map(|(_, _, c)| c);
            if let Some(c) = &known {
                if app.cloud_sync.form.server.is_empty() {
                    app.cloud_sync.form.server = c.server.clone();
                    app.cloud_sync.form.user = c.user.clone();
                }
                if let Some(e) = app.session.sync_state().and_then(|st| st.error()) {
                    info(ui, t, crate::i18n::tr("Status"), e, t.caution);
                }
            }
            if crate::is_compact(ui.ctx()) {
                heading(ui, t, crate::i18n::tr("Account"));
            }
            let mut form = std::mem::take(&mut app.cloud_sync.form);
            text_row(ui, t, "syncServer", crate::i18n::tr("Server"), &mut form.server, "https://photos.example.com", false, 260.0);
            text_row(
                ui,
                t,
                "syncUser",
                crate::i18n::tr("User"),
                &mut form.user,
                if crate::is_compact(ui.ctx()) { "Required" } else { "" },
                false,
                260.0,
            );
            text_row(
                ui,
                t,
                "syncPassword",
                crate::i18n::tr("Password"),
                &mut form.password,
                if crate::is_compact(ui.ctx()) { "Required" } else { "" },
                true,
                260.0,
            );
            if crate::is_compact(ui.ctx()) {
                card_end(ui, t);
                ui.add_space(28.0);
            }
            check(ui, "sync.merge", &mut form.merge, "Combine with the photos already on the server");
            row(ui, t, "", |ui| {
                let ready = !form.server.trim().is_empty() && !form.user.trim().is_empty() && !form.password.is_empty();
                if action(ui, t, "syncSignIn", "Sign In", ready, false) {
                    let p = json!({"server": form.server.trim(), "user": form.user.trim(), "password": form.password, "merge": form.merge});
                    form.password.clear();
                    sync_cmd(app, ui, "sync.signIn", p);
                }
            });
            app.cloud_sync.form = form;
            hint(
                ui,
                t,
                crate::i18n::tr(
                    "A library signed in to an empty server uploads its photos there. Signing in from a new, empty library gets the server's. If both have photos, you choose: add this library to the server's, or use the server's instead. Ticking Combine merges them at once: photos are matched by what they are, and nothing is removed on either side.",
                ),
            );
            // joined to an account's library, and signed out by the server: sign in again as that user, or remove it
            let bound = known.as_ref().is_some_and(|c| !c.library.is_empty());
            app.cloud_sync.sign_out = app.cloud_sync.sign_out.filter(|h| bound && *h == SignOutHow::Remove);
            if let Some(how) = app.cloud_sync.sign_out {
                sign_out_question(app, ui, t, how);
            } else if bound {
                if crate::is_compact(ui.ctx()) {
                    ui.add_space(28.0);
                }
                row(ui, t, "", |ui| {
                    if action(ui, t, "syncRemoveLibrary", "Remove Library from This Device…", true, true) {
                        app.cloud_sync.sign_out = Some(SignOutHow::Remove);
                    }
                });
                hint(
                    ui,
                    t,
                    crate::i18n::tr(
                        "This library belongs to the account it was signed in as, and the server signed this device out. Sign in as the same user to go on, or remove the library from this device to sign in as someone else: what hasn't been sent is lost.",
                    ),
                );
            }
            // (last: a page that is long already keeps the buttons above in reach)
            cloud_folder_section(app, ui, t);
        }
    }
}

/// Syncing through a folder of iCloud Drive, Dropbox or Google Drive instead of a server (`docs/sync.md`).
pub(crate) fn cloud_folder_section(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    heading(ui, t, crate::i18n::tr("Without a Server"));
    hint(
        ui,
        t,
        crate::i18n::tr(
            "Keep this library's presets, settings and edits in step with your other computers through a folder your own iCloud Drive, Dropbox or Google Drive already syncs. LightCraft writes only to a folder named LightCraft Sync inside it.",
        ),
    );
    if let Some(chosen) = app.session.icloud_folder_info() {
        let path = chosen["path"].as_str().unwrap_or_default().to_string();
        info(ui, t, crate::i18n::tr("Folder"), &path, t.text);
        let (text, problem) = cloud_folder_status(&chosen["last"]);
        let r = info(ui, t, crate::i18n::tr("Status"), &text, if problem { t.caution } else { t.text });
        register(ui, "label:cloudFolderStatus", r);
        row(ui, t, "", |ui| {
            if action(ui, t, "cloudFolderNow", "Sync Now", true, false) {
                sync_cmd(app, ui, "icloud.folderNow", json!({}));
            }
            if action(ui, t, "cloudFolderStop", "Stop Syncing", true, true) {
                sync_cmd(app, ui, "icloud.useFolder", json!({"path": null}));
            }
        });
        return;
    }
    let folders = app.cloud_sync.folders.get_or_insert_with(lightcraft_engine::icloud::folder::detected_cloud_folders).clone();
    let can_pick = app.services.pick_folder.is_some() || app.services.picker.is_some();
    if folders.is_empty() && !can_pick {
        hint(ui, t, crate::i18n::tr("No iCloud Drive, Dropbox or Google Drive folder was found on this computer."));
        return;
    }
    row(ui, t, "", |ui| {
        for (provider, path) in &folders {
            let id = format!("cloudFolder:{provider}");
            if action(ui, t, &id, &crate::i18n::tr_format!("Use {provider}", provider = provider), true, false) {
                sync_cmd(app, ui, "icloud.useFolder", json!({"path": path}));
            }
        }
        if can_pick && action(ui, t, "cloudFolderChoose", "Choose Folder…", true, false) {
            sync_cmd(app, ui, "icloud.chooseFolder", json!({}));
        }
    });
}

/// How the last pass through the cloud folder went, in words.
pub(crate) fn cloud_folder_status(last: &Value) -> (String, bool) {
    let Some(at) = last["at"].as_i64() else { return (crate::i18n::tr("Starting…").to_string(), false) };
    if let Some(e) = last["error"].as_str() {
        return (crate::i18n::tr_format!("Can't sync: {error}", error = e), true);
    }
    let waiting = last["report"]["waiting"].as_u64().unwrap_or(0);
    let secs = (web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64) - at).max(0) / 1000;
    let text = match secs {
        0..=59 => crate::i18n::tr("Synced just now").to_string(),
        60..=3599 => crate::i18n::tr_format!("Synced {n} min ago", n = secs / 60),
        _ => crate::i18n::tr_format!("Synced {n} h ago", n = secs / 3600),
    };
    if waiting > 0 {
        return (crate::i18n::tr_format!("{status}; waiting for {n} file(s) from the provider", status = text, n = waiting), false);
    }
    (text, false)
}

/// What a sign-out button asked, asked back: what the answer does, then the answer. `sync.signOut` takes
/// `confirm` for the graceful one (it waits for what is left to be sent) and `force` for the others.
pub(crate) fn sign_out_question(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens, how: SignOutHow) {
    let (title, first, rest, confirm) = match how {
        SignOutHow::Wait => (
            "Sign Out?",
            "This removes this account's library from this device.",
            "It stays on the server, and signing in again brings it back. Anything that hasn't reached the server yet is sent first.",
            "Sign Out",
        ),
        SignOutHow::Anyway => (
            "Sign Out Without Waiting?",
            "Some changes or photos haven't reached the server.",
            "Signing out now removes this library from this device, and what hasn't been sent is lost.",
            "Sign Out Anyway",
        ),
        SignOutHow::Remove => (
            "Remove Library from This Device?",
            "The server signed this device out, so nothing can be sent.",
            "Removing the library from this device loses what hasn't been sent. Signing in again brings the rest back from the server.",
            "Remove Library",
        ),
    };
    heading(ui, t, title);
    if how == SignOutHow::Remove {
        let account = app.session.sync_state().map_or_else(String::new, |st| {
            let host = st.config.server.trim_start_matches("https://").trim_start_matches("http://").trim_end_matches('/');
            format!("{} · {host}", st.config.user)
        });
        info(ui, t, crate::i18n::tr("Library"), &account, t.text);
    }
    let mut text = format!("{} {}", crate::i18n::tr(first), crate::i18n::tr(rest));
    // (a library that isn't a folder, a browser's, has no copy to keep)
    if how != SignOutHow::Wait && app.session.library.as_ref().is_some_and(|l| l.on_disk) {
        text.push(' ');
        text.push_str(crate::i18n::tr("A copy of the catalog is first saved in the library folder."));
    }
    hint(ui, t, &text);
    let (mut go, mut back) = (false, false);
    row(ui, t, "", |ui| {
        go = action(ui, t, "syncSignOutConfirm", confirm, true, true);
        back = action(ui, t, "syncSignOutBack", "Cancel", true, false);
    });
    if go {
        app.cloud_sync.sign_out = None;
        let p = if how == SignOutHow::Wait { json!({"confirm": true}) } else { json!({"force": true}) };
        sync_cmd(app, ui, "sync.signOut", p);
    } else if back {
        app.cloud_sync.sign_out = None;
    }
}

/// The album links this user has made: copy one, or take it back.
pub(crate) fn shared_links(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    heading(ui, t, crate::i18n::tr("Shared Links"));
    app.session.sync_refresh_shares(false);
    let (links, error) = app.session.sync_shares();
    let error = error.map(str::to_string);
    if links.is_empty() {
        hint(
            ui,
            t,
            crate::i18n::tr(
                "Right-click an album and choose Share Link… to make a link anyone can open in a browser, with no account. It shows the album's photos as you have edited them, can ask for a password or let people add photos, and you can take it back here.",
            ),
        );
    }
    let (mut copy, mut revoke) = (None, None);
    for (link, url) in &links {
        let ends = match link.expires {
            Some(e) => {
                format!("ends {}", lightcraft_catalog::dates::civil(i64::try_from(e).unwrap_or(i64::MAX)).chars().take(10).collect::<String>())
            }
            None => "no end".to_string(),
        };
        let mut what = vec![if link.originals { "originals too" } else { "pictures only" }];
        if link.password {
            what.push("password");
        }
        if link.upload {
            what.push("people add photos");
        }
        row(ui, t, &link.name, |ui| {
            ui.label(RichText::new(format!("{}, {ends}", what.join(", "))).color(t.text_dim));
            if action(ui, t, &format!("shareCopy{}", link.id), "Copy Link", true, false) {
                copy = Some(url.clone());
            }
            if action(ui, t, &format!("shareRevoke{}", link.id), "Revoke", true, true) {
                revoke = Some(link.id.clone());
            }
        });
    }
    if let Some(e) = error {
        hint(ui, t, &e);
    }
    if let Some(url) = copy {
        ui.ctx().copy_text(url);
        app.toast(ui.ctx(), "Link copied");
    }
    if let Some(id) = revoke {
        sync_cmd(app, ui, "album.unshare", json!({"id": id}));
    }
}

/// A disk's bar: what the library (`whose`) takes of it, what else is on it, what is free.
pub(crate) fn disk_bar(ui: &mut egui::Ui, t: &Tokens, whose: &str, library: u64, disk: lightcraft_catalog::sync::proto::Disk) {
    use crate::sync_ui::bytes_label;
    let used = disk.total.saturating_sub(disk.free);
    let legend = format!(
        "{}: {}\n{}: {}\n{}: {}",
        whose,
        bytes_label(library),
        crate::i18n::tr("Everything else"),
        bytes_label(used.saturating_sub(library)),
        crate::i18n::tr("Free"),
        bytes_label(disk.free)
    );
    let paint = |ui: &mut egui::Ui, r: Rect| {
        let total = disk.total.max(1) as f32;
        let used = disk.total.saturating_sub(disk.free);
        let ours = library.min(used);
        let at = |bytes: u64| r.left() + r.width() * (bytes as f32 / total).clamp(0.0, 1.0);
        let p = ui.painter();
        p.rect_filled(r, 4.0, t.track);
        p.rect_filled(Rect::from_min_max(r.min, pos2(at(used), r.bottom())), 4.0, t.text_disabled);
        // (at least a sliver, so a small library still shows)
        let ours_w = (at(ours) - r.left()).max(if ours > 0 { 2.0 } else { 0.0 });
        p.rect_filled(Rect::from_min_size(r.min, vec2(ours_w, r.height())), 4.0, t.accent);
    };
    if crate::is_compact(ui.ctx()) {
        let (r, _) = tap_row(ui, t, "", "", 24.0, 16.0, false);
        paint(ui, Rect::from_center_size(r.center(), vec2(r.width() - 32.0, 8.0)));
        return;
    }
    row(ui, t, "", |ui| {
        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width().min(300.0), 8.0), Sense::hover());
        paint(ui, r);
        resp.on_hover_text(legend);
    });
}

/// Settings ▸ Sync ▸ Storage: what this library takes on the server (as the server last said) and
/// on this computer (measured by a worker thread, see [`crate::sync_ui::storage_poll`]).
pub(crate) fn storage_section(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens, server: &str) {
    use crate::sync_ui::{bytes_label, count_label};
    let files = |f: lightcraft_catalog::sync::proto::Files| {
        let n = count_label(f.files);
        format!("{} · {n} {}", bytes_label(f.bytes), if f.files == 1 { "file" } else { "files" })
    };
    heading(ui, t, crate::i18n::tr("Storage"));
    let mut refresh = false;
    let age = app.session.sync_state().and_then(|st| st.usage_age());
    row(ui, t, "", |ui| {
        if action(ui, t, "syncUsageRefresh", "Refresh", true, false) {
            refresh = true;
        }
        if !crate::is_compact(ui.ctx())
            && let Some(a) = age
        {
            let text = if a.as_secs() < 3 { crate::i18n::tr("Updated just now").to_string() } else { format!("Updated {} s ago", a.as_secs()) };
            ui.label(RichText::new(text).size(11.0).color(t.text_dim));
        }
    });
    crate::sync_ui::storage_poll(app, ui.ctx(), refresh);

    let host = server.trim_start_matches("https://").trim_start_matches("http://").trim_end_matches('/');
    heading(ui, t, &format!("{} {host}", crate::i18n::tr("On the server")));
    let (usage, error) = match app.session.sync_state() {
        Some(st) => (st.usage().cloned(), st.usage_error().map(str::to_string)),
        None => (None, None),
    };
    match &usage {
        Some(u) => {
            info(ui, t, crate::i18n::tr("Originals"), &files(u.original), t.text);
            info(ui, t, crate::i18n::tr("Smart previews"), &files(u.smart), t.text);
            info(ui, t, crate::i18n::tr("Small previews"), &files(u.mini), t.text);
            if u.folders.files > 0 {
                info(ui, t, crate::i18n::tr("Library folders"), &files(u.folders), t.text);
            }
            // (everything the rows above take: the photos read in place from library folders are the library's too)
            info(ui, t, crate::i18n::tr("Total"), &bytes_label(u.total()), t.text);
            // (an admin's limit on what the server keeps for this user: library folders don't count)
            let full = u.quota.is_some_and(|q| u.stored() >= q.saturating_mul(9) / 10);
            if let Some(q) = u.quota {
                let text = format!("{} of {} used", bytes_label(u.stored()), bytes_label(q));
                info(ui, t, crate::i18n::tr("Storage limit"), &text, if full { t.caution } else { t.text });
            }
            if let Some(d) = u.disk {
                let text = format!("{} free of {}", bytes_label(d.free), bytes_label(d.total));
                info(ui, t, crate::i18n::tr("Server disk"), &text, t.text);
                disk_bar(ui, t, crate::i18n::tr("This library"), u.on_disk(), d);
            }
            if u.folders.files > 0 {
                hint(
                    ui,
                    t,
                    crate::i18n::tr(
                        "Library folders are read where they are on the server and never copied, so LightCraft keeps only their previews. The total counts their photos too: they take that room on the server.",
                    ),
                );
            }
            if full {
                hint(
                    ui,
                    t,
                    crate::i18n::tr(
                        "Nearly at the storage limit the server's admin set for you. Over it, new photos wait to upload until there is room; nothing is deleted.",
                    ),
                );
            }
        }
        None if error.is_none() => hint(ui, t, crate::i18n::tr("Asking the server…")),
        None => {}
    }
    if let Some(e) = &error {
        hint(ui, t, &format!("{}: {e}", crate::i18n::tr("The server can't say how much it stores")));
    }

    let Some(l) = app.session.local_dirs().and(app.cloud_sync.storage.local.clone()) else { return };
    let (photos, remote) = app.cloud_sync.storage.counts;
    heading(ui, t, crate::i18n::tr("On this computer"));
    let text = if remote == 0 {
        format!("{} · all with their original here", count_label(photos as u64))
    } else {
        format!("{} · {} with only previews here", count_label(photos as u64), count_label(remote as u64))
    };
    info(ui, t, crate::i18n::tr("Photos"), &text, t.text);
    info(ui, t, crate::i18n::tr("Originals"), &files(l.downloaded), t.text);
    info(ui, t, crate::i18n::tr("Smart previews"), &files(l.smart), t.text);
    info(ui, t, crate::i18n::tr("Small previews"), &files(l.mini), t.text);
    info(ui, t, crate::i18n::tr("Thumbnails"), &files(l.thumbnails), t.text);
    if l.imported.files > 0 {
        info(ui, t, crate::i18n::tr("Imported copies"), &files(l.imported), t.text);
    }
    info(ui, t, crate::i18n::tr("Catalog and edits"), &bytes_label(l.library), t.text);
    info(ui, t, crate::i18n::tr("Total"), &bytes_label(l.total()), t.text);
    if let Some(d) = l.disk {
        let text = format!("{} free of {}", bytes_label(d.free), bytes_label(d.total));
        info(ui, t, crate::i18n::tr("This disk"), &text, t.text);
        disk_bar(ui, t, crate::i18n::tr("LightCraft"), l.total(), d);
    }
    hint(
        ui,
        t,
        crate::i18n::tr(
            "Originals are the full-size files downloaded from the server. Photos you imported from folders on this computer stay where they are and aren't counted. Thumbnails can be deleted at any time; they are drawn again.",
        ),
    );
}
