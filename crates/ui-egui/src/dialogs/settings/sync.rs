//! Settings ▸ Sync: signing in to a LightCraft server (`docs/sync.md`), how syncing stands, what it keeps on this
//! computer, what this library takes on the server and the album links made from it.

use egui::{Color32, Rect, Sense, pos2, vec2};

use super::widgets::{check, choices, heading, hint, row};
use super::*;
use crate::sync_ui::SignOutHow;

/// A label and its value, beside each other. A value that doesn't fit wraps, never cut off.
fn info(ui: &mut egui::Ui, t: &Tokens, label: &str, value: &str, color: Color32) -> Rect {
    row(ui, t, label, |ui| ui.add(egui::Label::new(RichText::new(value).color(color)).wrap()).rect)
}

/// A button (`button:{id}`). True when pressed.
fn action(ui: &mut egui::Ui, _t: &Tokens, id: &str, label: &str, enabled: bool, _destroys: bool) -> bool {
    let r = ui.add_enabled(enabled, egui::Button::new(crate::i18n::tr(label)));
    register(ui, format!("button:{id}"), r.rect);
    r.clicked()
}

/// A labelled choice among a few `options`: mutually exclusive buttons (`button:{id}-{i}`). True when it changed.
fn pick<V: PartialEq + Copy>(ui: &mut egui::Ui, t: &Tokens, id: &str, label: &str, options: &[(V, &str)], value: &mut V) -> bool {
    row(ui, t, label, |ui| choices(ui, id, options, value))
}

/// A one-line text field (`field:{id}`) beside its label.
#[allow(clippy::too_many_arguments)]
fn text_row(ui: &mut egui::Ui, t: &Tokens, id: &str, label: &str, value: &mut String, hint_text: &str, password: bool, width: f32) {
    row(ui, t, label, |ui| {
        crate::text_field::TextField::singleline(&format!("field:{id}"), value)
            .label(crate::i18n::tr(label))
            .hint(crate::i18n::tr(hint_text))
            .password(password)
            .width(width.min(ui.available_width()))
            .show(ui);
    });
}

// ---------------------------------------------------------------------------------------- Sync

/// Run a sync command, showing what went wrong.
fn sync_cmd(app: &mut LightcraftApp, ui: &egui::Ui, id: &str, p: Value) {
    if let Err(e) = app.run(id, p) {
        app.toast(ui.ctx(), e);
    }
}

pub(super) fn sync_tab(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
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
                    "The server decodes each original it is sent, so this device doesn't: that keeps a browser tab responsive. Off: the previews are built here, before the photo goes up.",
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
            let mut form = std::mem::take(&mut app.cloud_sync.form);
            text_row(ui, t, "syncServer", crate::i18n::tr("Server"), &mut form.server, "https://photos.example.com", false, 260.0);
            text_row(ui, t, "syncUser", crate::i18n::tr("User"), &mut form.user, "", false, 260.0);
            text_row(ui, t, "syncPassword", crate::i18n::tr("Password"), &mut form.password, "", true, 260.0);
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
        }
    }
}

/// The question Sign Out asks first: wait for what is left to be sent, sign out without waiting, or
/// (when the server signed this device out) remove the library from the device.
fn sign_out_question(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens, how: SignOutHow) {
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
fn shared_links(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
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
fn disk_bar(ui: &mut egui::Ui, t: &Tokens, whose: &str, library: u64, disk: lightcraft_catalog::sync::proto::Disk) {
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
    row(ui, t, "", |ui| {
        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width().min(300.0), 8.0), Sense::hover());
        paint(ui, r);
        resp.on_hover_text(legend);
    });
}

/// Settings ▸ Sync ▸ Storage: what this library takes on the server (as the server last said) and
/// on this computer (measured by a worker thread, see [`crate::sync_ui::storage_poll`]).
fn storage_section(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens, server: &str) {
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
        if let Some(a) = age {
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
