use super::dates::browse_all_photos;
use super::rows::{INDENT_STEP, out_of_sight, row, tree_open, tree_toggle};
use super::sections::header_h;
use super::*;

/// The user's library folders on the sync server, as folders (photos found there in place): a
/// click shows a folder's photos (and those below it), the triangle opens it. Under My Photos'
/// rows, before the sections, while the server has any.
pub(super) fn server_folders_section(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let folders = app.caches.server_folders(&app.session.catalog);
    if folders.is_empty() {
        return;
    }
    ui.add_space(SECTION_GAP);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), header_h(ui)), Sense::hover());
    heading(ui, pos2(r.left() + 18.0, r.center().y), "Server Folders", t.semibold(13.5), t.text_label);
    server_folder_rows(app, ui, &folders, "", 0.0);
}

/// The folders directly in `parent` (`""`: the library folders themselves).
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
        let has_children = all.range(format!("{path}/")..).next().is_some_and(|(k, _)| k.starts_with(&format!("{path}/")));
        if has_children {
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
            }
            if app.services.sync_exec.is_some() && app.session.sync_state().is_some() {
                if ui.button(crate::i18n::tr("Make Available Offline")).clicked() {
                    let ids = ids(&app.session.catalog);
                    let _ = app.run("photo.makeAvailableOffline", json!({"ids": ids, "on": true}));
                }
                if ui.button(crate::i18n::tr("Download Originals")).clicked() {
                    let ids = ids(&app.session.catalog);
                    let _ = app.run("sync.downloadOriginals", json!({"ids": ids}));
                }
            }
        });
        crate::motion::fold(ui, open_id, open, |ui| server_folder_rows(app, ui, all, path, indent + 16.0));
    }
}

/// "Keywords": the library's keyword tree with photo counts (`a|b|c` keywords nest). A click
/// filters the grid by the keyword (children included), the triangle opens a level, and the
/// context menu renames, merges or deletes the keyword across the library.
pub(super) fn keyword_rows(app: &mut LightcraftApp, ui: &mut egui::Ui, nodes: &[KeywordNode], indent: f32) {
    for n in nodes {
        // leaves (most keywords) have no open state to look up
        let open_id = (!n.children.is_empty()).then(|| egui::Id::unique(("kw-open", n.path.to_lowercase())));
        let mut open = open_id.is_some_and(|k| tree_open(ui, k, false));
        let id = format!("keyword:{}", n.path);
        if !out_of_sight(app, ui, &id, &n.name, Some(n.count), indent, false) {
            keyword_row(app, ui, n, &id, indent, open_id, &mut open);
        }
        if let Some(open_id) = open_id {
            crate::motion::fold(ui, open_id, open, |ui| keyword_rows(app, ui, &n.children, indent + INDENT_STEP));
        }
    }
}

/// One drawn row of [`keyword_rows`]; `open_id` and `open` are its triangle's when it has rows inside it.
fn keyword_row(app: &mut LightcraftApp, ui: &mut egui::Ui, n: &KeywordNode, id: &str, indent: f32, open_id: Option<egui::Id>, open: &mut bool) {
    let sel = app.session.filter.keyword.as_deref().is_some_and(|k| k.eq_ignore_ascii_case(&n.path));
    let resp = row(app, ui, id, Icon::Tag, &n.name, Some(n.count), sel, indent);
    if let Some(k) = open_id {
        tree_toggle(ui, &resp, indent, k, open, format!("keywordToggle:{}", n.path));
    }
    let resp = resp.on_hover_text(if n.children.is_empty() { n.path.clone() } else { format!("{} (includes the keywords below it)", n.path) });
    if resp.clicked() {
        let v = if sel { serde_json::Value::Null } else { json!(n.path) };
        browse_all_photos(app, !sel);
        let _ = app.run("library.filter", json!({"keyword": v}));
    }
    crate::widgets::context_menu(&resp, |ui| {
        let has_sel = app.session.active().is_some();
        if ui.add_enabled(has_sel, egui::Button::new(crate::i18n::tr("Add to Selected Photos"))).clicked() {
            let _ = app.run("photo.setMeta", json!({"addKeywords": [n.path]}));
        }
        if ui.add_enabled(has_sel, egui::Button::new(crate::i18n::tr("Remove from Selected Photos"))).clicked() {
            let _ = app.run("photo.setMeta", json!({"removeKeywords": [n.path]}));
        }
        ui.separator();
        if ui.button(crate::i18n::tr("Rename Keyword…")).clicked() {
            app.ui.dialog = Some(crate::state::Dialog::RenameKeyword { from: n.path.clone(), to: n.path.clone() });
        }
        if ui.button(crate::i18n::tr("Merge into…")).clicked() {
            app.ui.dialog = Some(crate::state::Dialog::MergeKeywords { from: vec![n.path.clone()], into: String::new() });
        }
        if ui.button(crate::i18n::tr("Delete Keyword")).clicked() {
            let _ = app.run("keyword.delete", json!({"keyword": n.path}));
        }
    });
}
