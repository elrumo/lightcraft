use super::rows::{INDENT_STEP, row, tree_open, tree_toggle};
use super::*;

/// The user's library folders on the sync server, as folders (photos found there in place): a click shows a
/// folder's photos (and those below it), the triangle opens it. Under My Photos' rows, before the sections, while the
/// server has any.
pub(super) fn server_folders_section(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let folders = app.caches.server_folders(&app.session.catalog);
    if folders.is_empty() {
        return;
    }
    ui.add_space(SECTION_GAP);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::hover());
    ui.painter().text(pos2(r.left() + 18.0, r.center().y), Align2::LEFT_CENTER, crate::i18n::tr("Server Folders"), t.semibold(13.5), t.text_label);
    server_folder_rows(app, ui, &folders, "", 0.0);
}

/// The folders directly in `parent` (`""`: the library folders themselves), with their photo counts.
fn server_folder_children<'a>(all: &'a std::collections::BTreeMap<String, usize>, parent: &str) -> Vec<(&'a str, usize)> {
    let prefix = if parent.is_empty() { String::new() } else { format!("{parent}/") };
    all.range(prefix.clone()..)
        .take_while(|(k, _)| k.starts_with(&prefix))
        .filter(|(k, _)| k.get(prefix.len()..).is_some_and(|rest| !rest.is_empty() && !rest.contains('/')))
        .map(|(k, n)| (k.as_str(), *n))
        .collect()
}

fn server_folder_rows(app: &mut LightcraftApp, ui: &mut egui::Ui, all: &std::collections::BTreeMap<String, usize>, parent: &str, indent: f32) {
    for (path, count) in server_folder_children(all, parent) {
        let name = path.rsplit('/').next().unwrap_or(path);
        let open_id = egui::Id::unique(("server-folder-open", path.to_string()));
        let mut open = tree_open(ui, open_id, false);
        let sel = app.session.filter.server_folder.as_deref() == Some(path);
        let resp = row(app, ui, &format!("serverFolder:{path}"), Icon::Folder, name, Some(count), sel, indent);
        let children = format!("{path}/");
        if all.range(children.clone()..).next().is_some_and(|(k, _)| k.starts_with(&children)) {
            tree_toggle(ui, &resp, indent, open_id, &mut open, format!("serverFolderToggle:{path}"));
        }
        let resp = resp.on_hover_text(format!("{path} on the sync server (with the folders in it)"));
        if resp.clicked() {
            // the whole library, narrowed to the folder (like By Date)
            if !sel && !matches!(app.session.source, LibrarySource::All) {
                let _ = app.run("library.source", json!({"kind": "all"}));
            }
            let v = if sel { serde_json::Value::Null } else { json!(path) };
            let _ = app.run("library.filter", json!({"serverFolder": v}));
        }
        resp.context_menu(|ui| {
            let ids = |c: &lightcraft_catalog::Catalog| -> Vec<u64> {
                c.photos()
                    .filter(|p| !p.deleted && p.server_path.as_deref().is_some_and(|sp| lightcraft_catalog::query::in_server_folder(sp, path)))
                    .map(|p| p.id.0)
                    .collect()
            };
            if ui.button(crate::i18n::tr("Create Album from Folder")).clicked() {
                let ids = ids(&app.session.catalog);
                if let Ok(r) = app.run("album.create", json!({"name": name})) {
                    let _ = app.run("album.addPhotos", json!({"id": r["id"], "ids": ids}));
                }
                ui.close();
            }
            if app.services.sync_exec.is_some() && app.session.sync_state().is_some() {
                if ui.button(crate::i18n::tr("Make Available Offline")).clicked() {
                    let ids = ids(&app.session.catalog);
                    let _ = app.run("photo.makeAvailableOffline", json!({"ids": ids, "on": true}));
                    ui.close();
                }
                if ui.button(crate::i18n::tr("Download Originals")).clicked() {
                    let ids = ids(&app.session.catalog);
                    let _ = app.run("sync.downloadOriginals", json!({"ids": ids}));
                    ui.close();
                }
            }
        });
        if open {
            server_folder_rows(app, ui, all, path, indent + INDENT_STEP);
        }
    }
}
