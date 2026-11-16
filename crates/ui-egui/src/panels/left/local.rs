use super::folders::folder_tree;
use super::rows::Row;
use super::{Icon, LibrarySource, LightcraftApp, SidebarSection, json, register};

/// Local's folders as they stand this frame, and the folder being browsed among them.
pub(super) struct LocalNow {
    pub(super) places: LocalPlaces,
    current: Option<String>,
    /// The places removed from Local, as (name, path), in the order they were hidden.
    hidden: Vec<(String, String)>,
}

/// Folders on this computer to browse without adding (Lightroom's Local): Pictures, Desktop,
/// Downloads, the home folder, the folder being browsed, and the folders kept with Browse
/// Folder…. Worked out every frame the sidebar shows, drawn or not: the row listed for this
/// session only (`local_browse_root`) follows the browsing. `None` where there is no Local (web).
pub(super) fn local_now(app: &mut LightcraftApp, ui: &egui::Ui) -> Option<LocalNow> {
    // (an iOS app sees only its own container: photos come in through the Photos and Files pickers)
    if !SidebarSection::Local.available() || cfg!(target_os = "ios") {
        return None;
    }
    let home = home_folder();
    let mut builtin = app.services.local_places.clone().unwrap_or_default();
    if app.services.local_places.is_none() {
        for (name, p) in default_places(&home) {
            // (checked off the UI thread: a home folder can be on a network share)
            if fs_cached(ui, "is-dir", &p, 5.0, |p| std::path::Path::new(p).is_dir()) == Some(true) {
                builtin.push((crate::i18n::tr(name).to_string(), p));
            }
        }
    }
    let browsing = app.session.browse.clone().filter(|_| app.session.source == LibrarySource::Folder);
    let current = browsing.as_ref().map(|b| b.path.clone());
    // a hidden built-in place keeps its name (Home, not the user's folder name)
    let name_of = |path: &str| builtin.iter().find(|(_, p)| same_folder(p, path)).map_or_else(|| folder_label(path), |(n, _)| n.clone());
    let hidden = app.ui.hidden_locations.iter().map(|h| (name_of(h), h.clone())).collect();
    let unlisted = current.as_deref().and_then(|path| fs_cached(ui, "unlisted-ancestors", path, 2.0, unlisted_ancestors));
    let mut places = local_places_with_unlisted(
        builtin,
        &app.ui.local_roots,
        current.as_deref(),
        app.ui.local_browse_root.as_deref(),
        &app.ui.hidden_locations,
        unlisted.as_deref().unwrap_or(&[]),
    );
    if current.is_some() && unlisted.is_none() {
        places.owner = None;
    }
    if current.is_some() && unlisted.is_some() {
        app.ui.local_browse_root = places.browse_root.clone();
    }
    Some(LocalNow { places, current, hidden })
}

/// The user's home folder, the engine's own reading of it (so the menu and the rename/move guard
/// agree); empty when the host has none.
fn home_folder() -> String {
    lightcraft_engine::library::home_folder().map(|h| h.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Local's built-in places under `home` as (English name, path), in order. The names are keys for
/// `tr`, never folder names: the folders on disk keep their own (Pictures, not "Bilder").
fn default_places(home: &str) -> Vec<(&'static str, String)> {
    if home.is_empty() {
        return Vec::new();
    }
    lightcraft_apphost::local::BUILTIN.into_iter().map(|(name, sub)| (name, lightcraft_apphost::local::builtin_path(home, sub))).collect()
}

/// Whether `path` is one of Local's built-in places (the home folder, Pictures, Desktop,
/// Downloads, or a place the host listed itself). They are the computer's own folders, shown under
/// a translated label, so Local offers no Rename or Move for them.
pub(super) fn is_builtin_place(app: &LightcraftApp, path: &str) -> bool {
    builtin_place_in(app.services.local_places.as_deref(), &home_folder(), path)
}

/// [`is_builtin_place`] for the places the host listed (`injected`, which replace the defaults) or,
/// without them, the ones under `home`.
fn builtin_place_in(injected: Option<&[(String, String)]>, home: &str, path: &str) -> bool {
    match injected {
        Some(places) => places.iter().any(|(_, p)| same_folder(p, path)),
        None => default_places(home).iter().any(|(_, p)| same_folder(p, path)),
    }
}

/// Local's rows: each place as a folder tree, Browse Folder…, and the way back for hidden places.
pub(super) fn local_rows(app: &mut LightcraftApp, ui: &mut egui::Ui, now: &LocalNow) {
    let (local, current) = (&now.places, now.current.as_deref());
    for (i, (name, path)) in local.places.iter().enumerate() {
        let transient = local.browse_root.as_deref() == Some(path.as_str());
        let reveal = local.owner == Some(i);
        folder_tree(app, ui, path, name, path, 0.0, current, reveal, transient);
    }
    if app.services.pick_folder.is_some() && Row::new("local:browse", Icon::Plus, crate::i18n::tr("Browse Folder…")).show(app, ui).clicked() {
        let picked = app.services.pick_folder.as_mut().and_then(|f| f());
        if let Some(path) = picked {
            match app.run("library.browse", json!({"path": path})) {
                // the picked folder stays in Local (and comes back if it was hidden)
                Ok(r) => {
                    let dir = r["path"].as_str().unwrap_or(&path).to_string();
                    let _ = app.run("local.addRoot", json!({"path": dir}));
                }
                Err(e) => app.toast(ui.ctx(), e),
            }
        }
    }
    // the way back for hidden places: one comes back from the row itself; several are listed, so
    // each comes back on its own, or all at once (issue #2114: the row used to bring every one back)
    let hidden = now.hidden.len();
    if hidden > 0 {
        let label = crate::i18n::tr_format!("Show {hidden} hidden location{}", if hidden == 1 { "" } else { "s" }, hidden = hidden);
        let resp = Row::new("local:restoreHidden", Icon::Folder, &label)
            .show(app, ui)
            .on_hover_text(crate::i18n::tr("Put the locations you removed from Local back (no files change)"));
        if let [(_, only)] = now.hidden.as_slice() {
            if resp.clicked() {
                let _ = app.run("local.restoreHidden", json!({"path": only}));
            }
        } else {
            egui::Popup::menu(&resp).show(|ui| hidden_menu(app, ui, &now.hidden));
        }
    }
}

/// The list behind "Show N hidden locations": each place by name (its path on hover), then
/// Restore All.
fn hidden_menu(app: &mut LightcraftApp, ui: &mut egui::Ui, hidden: &[(String, String)]) {
    for (name, path) in hidden {
        let r = ui.button(name).on_hover_text(path);
        register(ui, format!("local:restoreHidden:{path}"), r.rect);
        if r.clicked() {
            let _ = app.run("local.restoreHidden", json!({"path": path}));
            ui.close();
        }
    }
    ui.separator();
    let r = ui.button(crate::i18n::tr("Restore All"));
    register(ui, "local:restoreHidden:all", r.rect);
    if r.clicked() {
        let _ = app.run("local.restoreHidden", json!({}));
        ui.close();
    }
}

pub(crate) use lightcraft_apphost::local::{LocalPlaces, folder_label, same_folder};
use lightcraft_apphost::local::{local_places_with_unlisted, unlisted_ancestors};

/// The subfolders of `path` (not hidden ones), sorted; listed on a worker thread at most every
/// 2 s per folder (empty until the first listing).
pub(super) fn subfolders(ui: &egui::Ui, path: &str) -> std::sync::Arc<Vec<(String, String)>> {
    fs_cached(ui, "subfolders", path, 2.0, list_subfolders).unwrap_or_default()
}

/// [`lightcraft_apphost::local::list_subfolders`], shared: the cache hands out a copy every frame.
fn list_subfolders(path: &str) -> std::sync::Arc<Vec<(String, String)>> {
    lightcraft_apphost::local::list_subfolders(path).into()
}

/// How many file-system answers for `ctx` are being worked out right now: [`fs_cached`]'s and the
/// Missing Photos count's. Rows appear (and the sidebar below them moves) when they land, so the
/// headless driver counts them as pending work and waits for them before acting on widget
/// positions.
pub(crate) fn fs_cached_running(ctx: &egui::Context) -> usize {
    fs_running_counter(ctx).load(std::sync::atomic::Ordering::Acquire)
}

/// The counter behind [`fs_cached_running`]: a worker adds one before it starts and takes it off
/// after its answer is stored.
pub(super) fn fs_running_counter(ctx: &egui::Context) -> std::sync::Arc<std::sync::atomic::AtomicUsize> {
    ctx.data_mut(|d| d.get_temp_mut_or_default::<std::sync::Arc<std::sync::atomic::AtomicUsize>>(egui::Id::unique("fs-cached-running")).clone())
}

/// A file-system answer for `path` (`f(path)`), kept per `kind` and path and refreshed on a worker
/// thread at most every `every` seconds: a folder on a sleeping NAS, a dropped share or a
/// spinning-up drive never blocks a frame. `None` until the first answer arrives.
pub(crate) fn fs_cached<T: Clone + Send + 'static>(ui: &egui::Ui, kind: &'static str, path: &str, every: f64, f: fn(&str) -> T) -> Option<T> {
    struct Entry<T> {
        value: Option<T>,
        at: Option<f64>,
        running: bool,
    }
    type Cell<T> = std::sync::Arc<std::sync::Mutex<Entry<T>>>;
    let id = egui::Id::unique(("fs-cached", kind, path.to_string()));
    let now = ui.input(|i| i.time);
    let cell: Cell<T> = match ui.data(|d| d.get_temp::<Cell<T>>(id)) {
        Some(c) => c,
        None => {
            let c: Cell<T> = std::sync::Arc::new(std::sync::Mutex::new(Entry { value: None, at: None, running: false }));
            ui.data_mut(|d| d.insert_temp(id, c.clone()));
            c
        }
    };
    let lock = |c: &Cell<T>| c.lock().unwrap_or_else(std::sync::PoisonError::into_inner).value.clone();
    let start = {
        let mut e = cell.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let due = !e.running && e.at.is_none_or(|at| now - at >= every || now < at);
        if due {
            e.running = true;
            e.at = Some(now);
        }
        due
    };
    if start {
        use std::sync::atomic::Ordering;
        let running = fs_running_counter(ui.ctx());
        running.fetch_add(1, Ordering::AcqRel);
        let (out, path, repaint, done) = (cell.clone(), path.to_string(), ui.ctx().clone(), running.clone());
        let work = move || {
            let v = f(&path);
            let mut e = out.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            e.value = Some(v);
            e.running = false;
            drop(e);
            // after the answer is stored: a frame that sees the count drop also sees the answer
            done.fetch_sub(1, Ordering::AcqRel);
            repaint.request_repaint();
        };
        #[cfg(not(target_arch = "wasm32"))]
        if std::thread::Builder::new().name("lc-fs-list".into()).spawn(work).is_err() {
            cell.lock().unwrap_or_else(std::sync::PoisonError::into_inner).running = false;
            running.fetch_sub(1, Ordering::AcqRel);
        }
        #[cfg(target_arch = "wasm32")]
        work();
    }
    lock(&cell)
}

#[cfg(test)]
mod tests {
    use crate::widgets::elide_head;

    #[test]
    fn the_standard_places_under_a_home_are_built_in_and_nothing_else_is() {
        let home = "/home/example";
        for path in [home, "/home/example/", "/home/example/Pictures", "/home/example/Pictures/", "/home/example/Desktop", "/home/example/Downloads"]
        {
            assert!(super::builtin_place_in(None, home, path), "{path}");
        }
        for path in ["/home/example/Pictures/Trip", "/home/example/Documents", "/home/other", "/home", "Pictures"] {
            assert!(!super::builtin_place_in(None, home, path), "{path}");
        }
        // no home: no standard places, and no relative ones
        assert!(!super::builtin_place_in(None, "", "Pictures") && !super::builtin_place_in(None, "", ""));
        // places the host listed replace the standard ones
        let injected = vec![("Sandbox".to_string(), "/lightcraft-test/home".to_string())];
        assert!(super::builtin_place_in(Some(&injected), home, "/lightcraft-test/home"));
        assert!(!super::builtin_place_in(Some(&injected), home, "/home/example/Pictures"));
    }

    #[test]
    fn injected_places_replace_the_machine_home() {
        let places = vec![("Sandbox".to_string(), "/lightcraft-test/home".to_string())];
        let services = crate::Services { local_places: Some(places.clone()), ..Default::default() };
        let mut app = crate::LightcraftApp::new(lightcraft_engine::Session::new(), services);
        let mut view = crate::headless::HeadlessView::new();
        let raw = crate::headless::HeadlessView::raw_input(egui::vec2(300.0, 200.0), 1.0, 0.0, vec![]);
        view.run(raw, |ui| {
            assert_eq!(super::local_now(&mut app, ui).unwrap().places.places, places);
        });
    }

    #[test]
    fn local_folder_rows_outside_the_viewport_skip_interactive_widgets() {
        let mut app = crate::LightcraftApp::new(lightcraft_engine::Session::new(), crate::Services::default());
        let mut view = crate::headless::HeadlessView::new();
        for frame in 0..3 {
            let raw = crate::headless::HeadlessView::raw_input(egui::vec2(300.0, 160.0), 1.0, frame as f64 / 60.0, vec![]);
            view.run(raw, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for index in 0..500 {
                        let path = format!("/lightcraft-test/folder-{index:04}");
                        super::folder_tree(&mut app, ui, &path, &format!("Folder {index}"), &path, 0.0, None, false, false);
                    }
                });
            });
            let widgets = crate::widgets::take_registry(&view.ctx);
            if frame == 2 {
                let rows = widgets.iter().filter(|(id, _)| id.starts_with("source:local:")).count();
                let toggles = widgets.iter().filter(|(id, _)| id.starts_with("folderToggle:")).count();
                assert_eq!(rows, 500, "offscreen rows remain available for scrolling");
                assert!((1..=10).contains(&toggles), "only visible rows build interactive widgets: {toggles}");
            }
        }
    }

    #[test]
    fn browsing_below_an_unlisted_folder_has_a_session_root() {
        let home = "/home/example";
        let hidden = format!("{home}/.hidden");
        let current = format!("{hidden}/Photos");
        let builtin = || vec![("Home".into(), home.into())];
        let unlisted = vec![hidden];
        let first = super::local_places_with_unlisted(builtin(), &[], Some(&current), None, &[], &unlisted);
        assert_eq!(first.browse_root.as_deref(), Some(current.as_str()));
        assert_eq!(first.owner, Some(1));
        let child = format!("{current}/Trip");
        let next = super::local_places_with_unlisted(builtin(), &[], Some(&child), first.browse_root.as_deref(), &[], &unlisted);
        assert_eq!(next.browse_root, first.browse_root);
        assert_eq!(next.owner, Some(1));
        let kept = super::local_places_with_unlisted(builtin(), std::slice::from_ref(&current), Some(&child), None, &[], &unlisted);
        assert_eq!(kept.browse_root, None);
        assert_eq!(kept.owner, Some(1));
        let deeper = format!("{current}/.private");
        let inside = format!("{deeper}/Trip");
        let unlisted = vec![unlisted[0].clone(), deeper];
        let moved = super::local_places_with_unlisted(builtin(), &[], Some(&inside), first.browse_root.as_deref(), &[], &unlisted);
        assert_eq!(moved.browse_root.as_deref(), Some(inside.as_str()));
        assert_eq!(moved.owner, Some(1));
    }

    #[test]
    fn subfolders_leave_out_hidden_folders() {
        let dir = std::env::temp_dir().join(format!("lc-ui-hidden-subfolders-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for name in ["Shown", ".dot", "Hidden"] {
            std::fs::create_dir_all(dir.join(name)).unwrap();
        }
        #[cfg(windows)]
        assert!(std::process::Command::new("attrib").arg("+h").arg(dir.join("Hidden")).status().unwrap().success());
        #[cfg(target_os = "macos")]
        assert!(std::process::Command::new("chflags").arg("hidden").arg(dir.join("Hidden")).status().unwrap().success());
        let expected = if cfg!(any(windows, target_os = "macos")) { vec!["Shown"] } else { vec!["Hidden", "Shown"] };
        let listed = super::list_subfolders(&dir.to_string_lossy());
        assert_eq!(listed.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), expected);
        let unlisted = super::unlisted_ancestors(&dir.join(".dot/Photos").to_string_lossy());
        assert!(unlisted.contains(&dir.join(".dot").to_string_lossy().to_string()));
        if cfg!(any(windows, target_os = "macos")) {
            let unlisted = super::unlisted_ancestors(&dir.join("Hidden/Photos").to_string_lossy());
            assert!(unlisted.contains(&dir.join("Hidden").to_string_lossy().to_string()));
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Width = characters, so a limit of 12 is "12 characters".
    fn fit(text: &str, max: usize) -> String {
        elide_head(text, max as f32, |s| s.chars().count() as f32)
    }

    #[test]
    fn a_label_that_fits_is_left_alone() {
        assert_eq!(fit("/Users/me/Pictures", 18), "/Users/me/Pictures");
    }

    #[test]
    fn a_long_path_loses_its_leading_folders_not_its_end() {
        assert_eq!(fit("/Users/me/Pictures/Lightroom", 20), "…/Pictures/Lightroom");
        assert_eq!(fit("/Users/me/Pictures/Lightroom", 12), "…/Lightroom");
    }

    #[test]
    fn a_single_name_too_long_is_cut_at_the_end() {
        assert_eq!(fit("2024-summer-holiday", 8), "2024-su…");
        assert_eq!(fit("2024-summer-holiday", 1), "…");
        assert_eq!(fit("2024-summer-holiday", 0), "…");
    }

    #[test]
    fn multibyte_names_are_cut_on_character_boundaries() {
        assert_eq!(fit("/写真/夏休み旅行の記録", 6), "夏休み旅行…");
    }

    #[test]
    fn a_local_folder_lines_up_with_the_rows_above_and_its_triangle_stays_left_of_the_icon() {
        use crate::Services;
        use crate::headless::HeadlessView;
        let mut app = crate::LightcraftApp::new(lightcraft_engine::Session::new(), Services::default());
        let mut view = HeadlessView::new();
        let path = "/lightcraft-test/Pictures";
        let mut widgets = vec![];
        for frame in 0..3 {
            let raw = HeadlessView::raw_input(egui::vec2(300.0, 200.0), 1.0, frame as f64 / 60.0, vec![]);
            view.run(raw, |ui| {
                super::Row::new("all", super::Icon::Photos, "All Photos").show(&mut app, ui);
                super::folder_tree(&mut app, ui, path, "Pictures", path, 0.0, None, false, false);
            });
            widgets = crate::widgets::take_registry(&view.ctx);
        }
        let rect = |id: &str| widgets.iter().find(|(w, _)| w == id).unwrap_or_else(|| panic!("{id} not registered")).1;
        let (all, local) = (rect("label:all"), rect(&format!("label:local:{path}")));
        assert_eq!(local.left(), all.left(), "a top-level Local folder's name starts where All Photos' does");
        let triangle = rect(&format!("folderToggle:{path}"));
        // the icon is 16 points wide and ends 8 points before the name
        assert!(triangle.right() <= local.left() - 24.0, "triangle {triangle:?} overlaps the icon before {local:?}");
    }
}
