use super::*;

// ------------------------------------------------------------------------------- Open Library…

/// Switching libraries is refused while an import (or the folder scan before one) is running: its
/// remaining batches would be committed into the new library (issue #1509).
pub(crate) fn can_switch_library(app: &LightcraftApp) -> Result<(), String> {
    if crate::lightroom_import::is_running(app) {
        return Err("wait for Lightroom catalog import to finish before switching libraries".into());
    }
    if app.import.is_some() || app.scan.is_some() {
        return Err("wait for the import to finish before switching libraries".into());
    }
    Ok(())
}

/// `app.openLibrary {path?}`: close the current library and open (or create) the one at `path`,
/// or a folder chosen in a dialog. Remembered as the library to open at launch.
///
/// A Lightroom Classic catalog (its folder, an `.lrdata` / `.lrcat-data` folder or the `.lrcat`
/// itself, see [`lightcraft_engine::lightroom_folder`]) is not opened: the current library stays
/// and a dialog offers Import Lightroom Catalog… instead; returns `{opened: false,
/// lightroomCatalog: <.lrcat path or null>, path}`.
pub fn open_library(app: &mut LightcraftApp, p: &Value) -> Result<Value, String> {
    can_switch_library(app)?;
    let path = match p.get("path").and_then(Value::as_str) {
        Some(x) => x.to_string(),
        None => {
            let req = crate::pick::PickRequest::folder(crate::i18n::tr("Open Library"));
            match crate::pick::ask(app, "app.openLibrary", p, "path", req, |s| s.pick_folder.as_mut().and_then(|f| f()).map(|x| vec![x])) {
                crate::pick::Picked::Now(v) => match v.into_iter().next() {
                    Some(x) => x,
                    None => return Ok(Value::Null),
                },
                crate::pick::Picked::Later => return Ok(Value::Null),
                crate::pick::Picked::Unavailable => return Err("no folder dialog on this platform".into()),
            }
        }
    };
    // A Lightroom catalog folder (or part of one) is not a library: before closing the library
    // open now, offer to import it instead (issue #891). The engine refuses it too.
    if let Some(place) = lightcraft_engine::lightroom_folder::lightroom_catalog_at(std::path::Path::new(&path)) {
        let catalog = place.catalog.map(|c| c.to_string_lossy().into_owned());
        let err = lightcraft_engine::EngineError::LightroomCatalog { path: path.clone().into(), catalog: catalog.clone().map(Into::into) };
        // the library problem window (no library could be opened) shows the error instead
        if app.library_problem.is_some() {
            return Err(err.to_string());
        }
        app.ui.dialog = Some(crate::state::Dialog::LightroomCatalog { folder: path.clone(), catalog: catalog.clone() });
        return Ok(json!({"opened": false, "lightroomCatalog": catalog, "path": path}));
    }
    // a remembered library that isn't there (moved, or on a disk or network share that isn't connected) is said
    // so, never made anew in its place, and stays in Open Recent Library for when its share is back; a new one is
    // made only in a folder that is there
    let remembered = app.ui.settings.recent_libraries.contains(&path);
    if let Some(why) = lightcraft_apphost::libraries::missing(std::path::Path::new(&path), remembered) {
        return Err(why);
    }
    // a drive's or a share's top folder opens the one library in it (several: said so); a folder with none becomes
    // a library, unless it was one (a share's empty mount point)
    let dir = std::path::Path::new(&path);
    let path = if dir.is_dir() { lightcraft_apphost::libraries::find(dir, remembered)?.to_string_lossy().into_owned() } else { path };
    switch_library(app, &path)
}

/// Close the current library and open the one at `path` ([`lightcraft_apphost::libraries::switch`]:
/// checked before anything is closed, the previous library opened again if it still fails), then
/// let go of what the window showed of the old one and keep both in the recent list.
pub(crate) fn switch_library(app: &mut LightcraftApp, path: &str) -> Result<Value, String> {
    let generation = app.session.library_generation();
    let switched = lightcraft_apphost::libraries::switch(&mut app.session, std::path::Path::new(path));
    // (refused before anything was closed: the window keeps what it shows)
    if app.session.library_generation() != generation {
        app.lightroom.last = None;
        app.lightroom_result = None;
        if matches!(app.ui.dialog.as_ref(), Some(crate::state::Dialog::LightroomResult { .. })) {
            app.ui.dialog = None;
        }
        app.renderer.forget_all();
    }
    let switched = switched.map_err(|e| e.to_string())?;
    // Photo IDs are local to a catalog: pins cannot follow them into another library (a refused
    // or failed switch reopens the same catalog, so its pins stay).
    if app.session.library_generation() != generation {
        app.ui.reference = None;
        app.ui.compare = None;
        app.ui.brief_info = crate::state::BriefInfo::default();
        if app.ui.view == crate::state::ViewMode::Reference {
            app.ui.view = crate::state::ViewMode::PhotoGrid;
        }
    }
    let path = switched.dir.to_string_lossy().into_owned();
    app.ui.settings.library_path = path.clone();
    let recent = &mut app.ui.settings.recent_libraries;
    if let Some(prev) = switched.previous.map(|p| p.to_string_lossy().into_owned()).filter(|p| p != &path && !recent.contains(p)) {
        lightcraft_apphost::libraries::remember(recent, &prev);
    }
    lightcraft_apphost::libraries::remember(recent, &path);
    Ok(json!({"path": path, "name": lightcraft_apphost::libraries::name(&switched.dir), "photos": app.session.catalog.len()}))
}

/// Put `path` first in the recent list ([`lightcraft_apphost::libraries::remember`]).
pub fn remember_library(recent: &mut Vec<String>, path: &str) {
    lightcraft_apphost::libraries::remember(recent, path);
}

/// A library's name as people see it: its folder's name.
pub fn library_name(dir: &std::path::Path) -> String {
    lightcraft_apphost::libraries::name(dir)
}

/// The open library's name (`None`: in memory, nothing is saved).
pub fn open_library_name(app: &LightcraftApp) -> Option<String> {
    lightcraft_apphost::libraries::open_dir(&app.session).map(lightcraft_apphost::libraries::name)
}

/// `app.newLibrary {path?, name?}`: create a library and switch to it. With `name`, the library
/// is the folder `name` inside `path` (or inside a location chosen in a dialog); without it,
/// `path` itself, or a new folder named after the default library. The folder must not hold a
/// library already, nor anything else.
pub fn new_library(app: &mut LightcraftApp, p: &Value) -> Result<Value, String> {
    can_switch_library(app)?;
    let name = match p.get("name") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(lightcraft_apphost::libraries::valid_name(s).map_err(|e| format!("app.newLibrary: {e}"))?),
        Some(_) => return Err("app.newLibrary: name must be a string".into()),
    };
    // from the menu: ask for the name first (default prefilled), then for the location
    if name.is_none() && p.get("path").is_none() && app.services.pick_folder.is_some() {
        crate::panels::dialogs::prompt(
            app,
            "New Library",
            "Library name",
            lightcraft_engine::library::DEFAULT_NAME,
            "app.newLibrary",
            json!({}),
            "name",
        );
        return Ok(Value::Null);
    }
    let (location, picked) = match p.get("path").and_then(Value::as_str) {
        Some(x) => (std::path::PathBuf::from(x), false),
        None => {
            let req = crate::pick::PickRequest::folder(crate::i18n::tr("New Library: Choose Where to Put It"));
            match crate::pick::ask(app, "app.newLibrary", p, "path", req, |s| s.pick_folder.as_mut().and_then(|f| f()).map(|x| vec![x])) {
                crate::pick::Picked::Now(v) => match v.into_iter().next() {
                    Some(x) => (std::path::PathBuf::from(x), true),
                    None => return Ok(Value::Null),
                },
                crate::pick::Picked::Later => return Ok(Value::Null),
                crate::pick::Picked::Unavailable => return Err("no folder dialog on this platform".into()),
            }
        }
    };
    let dir = lightcraft_apphost::libraries::new_dir(&location, name, picked)?;
    switch_library(app, &dir.to_string_lossy())
}

/// Characters a folder name can't hold on Windows (and `/`, which no system allows).
const RESERVED_NAME_CHARS: [char; 9] = ['<', '>', ':', '"', '/', '\\', '|', '?', '*'];

fn valid_library_name(s: &str) -> Result<&str, String> {
    let n = s.trim();
    // Windows drops trailing dots and spaces from a folder name: refuse them rather than
    // creating a folder whose name differs from the one asked for (spaces around the name
    // are trimmed first)
    let bad = n.is_empty() || n == "." || n == ".." || n.ends_with(['.', ' ']) || n.contains(RESERVED_NAME_CHARS) || n.chars().any(char::is_control);
    if bad {
        return Err(format!(
            "app.newLibrary: {s:?} can't be a library name (a folder name without < > : \" / \\ | ? * and not ending in a dot or space)"
        ));
    }
    Ok(n)
}

/// `base` inside `location`, or `base 2`, `base 3`… if that is taken.
pub(crate) fn free_name(location: &std::path::Path, base: &str) -> std::path::PathBuf {
    lightcraft_apphost::libraries::free_name(location, base)
}

/// `app.recentLibraries {remove?: path, clear?: bool}`: the recently opened libraries, newest
/// first, as `[{path, name, exists, checking, open, lastOpened}]`. `exists` is what the last
/// background check found (`false`: moved, on a disconnected drive, not a folder or no longer a
/// library; `null`: not checked yet, or the check couldn't tell: a folder that can't be listed, an
/// unreadable catalog, a lookup or check that failed; `checking` says whether a check is running):
/// the command never looks at a folder itself, as one on an offline network share can take
/// seconds to answer. `lastOpened`: ISO 8601 time, `null` when not known. `remove` forgets that
/// one entry (an error if the list doesn't hold it) and `clear` forgets them all; neither touches
/// anything on disk (issue #1290).
pub fn recent_libraries(app: &mut LightcraftApp, p: &Value) -> Result<Value, String> {
    match p.get("remove") {
        None | Some(Value::Null) => {}
        Some(Value::String(path)) => {
            if !forget_library(&mut app.ui.settings, path) {
                return Err(format!("app.recentLibraries: {path:?} is not in the recent libraries"));
            }
            app.recent_checks.forget(path);
        }
        Some(_) => return Err("app.recentLibraries: remove must be a library path (a string)".into()),
    }
    if p.get("clear").and_then(Value::as_bool) == Some(true) {
        for path in std::mem::take(&mut app.ui.settings.recent_libraries) {
            app.recent_checks.forget(&path);
        }
        app.ui.settings.recent_opened.clear();
    }
    let open = lightcraft_apphost::libraries::open_dir(&app.session);
    let (settings, checks) = (&app.ui.settings, &app.recent_checks);
    let list: Vec<Value> = settings
        .recent_libraries
        .iter()
        .map(|r| {
            let d = std::path::Path::new(r);
            json!({
                "path": r,
                "name": library_name(d),
                "exists": checks.exists(r),
                "checking": checks.is_checking(r),
                "open": open.is_some_and(|o| lightcraft_apphost::libraries::same(o, d)),
                "lastOpened": settings.recent_opened.get(r),
            })
        })
        .collect();
    Ok(Value::Array(list))
}

/// Forget `path` in File ▸ Open Recent Library (and when it was opened); the library itself is
/// left where it is. False if the list didn't hold it.
pub fn forget_library(settings: &mut crate::state::AppSettings, path: &str) -> bool {
    let before = settings.recent_libraries.len();
    settings.recent_libraries.retain(|r| r != path);
    settings.recent_opened.remove(path);
    settings.recent_libraries.len() != before
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn app_with_import() -> LightcraftApp {
        let mut app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), crate::Services::default());
        app.run("photo.setReference", json!({})).unwrap();
        app.import = Some(crate::import::ImportTask::new(vec!["a.jpg".into()], json!({"mode": "add"}), 0, false));
        app
    }

    #[test]
    fn open_library_is_refused_while_an_import_runs() {
        let mut app = app_with_import();
        let pin = app.ui.reference;
        let err = open_library(&mut app, &json!({"path": "other"})).unwrap_err();
        assert!(err.contains("import to finish"), "{err}");
        assert!(app.import.is_some());
        assert_eq!(app.ui.reference, pin, "a refused Open Library keeps the reference pin");
    }

    #[test]
    fn new_library_is_refused_while_an_import_runs() {
        let mut app = app_with_import();
        let pin = app.ui.reference;
        let err = new_library(&mut app, &json!({"path": "other"})).unwrap_err();
        assert!(err.contains("import to finish"), "{err}");
        assert!(app.import.is_some());
        assert_eq!(app.ui.reference, pin, "a refused New Library keeps the reference pin");
    }

    /// The menu items that switch libraries are greyed out meanwhile, like during a Lightroom import.
    #[test]
    fn library_switching_menu_items_are_disabled_while_an_import_runs() {
        let mut app = LightcraftApp::new(lightcraft_engine::Session::new(), crate::Services::default());
        app.services.pick_folder = Some(Box::new(|| None));
        assert!(crate::menus::ui_enabled(&app, "app.openLibrary"));
        assert!(crate::menus::ui_enabled(&app, "app.newLibrary"));
        app.import = Some(crate::import::ImportTask::new(vec!["a.jpg".into()], json!({"mode": "add"}), 0, false));
        assert!(!crate::menus::ui_enabled(&app, "app.openLibrary"));
        assert!(!crate::menus::ui_enabled(&app, "app.newLibrary"));
    }

    /// The folder scan before an import's review counts too: it ends in an import into the open library.
    #[test]
    fn open_library_is_refused_while_an_import_scan_runs() {
        let dir = std::env::temp_dir().join(format!("lc-ui-switchscan-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let mut app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), crate::Services::default());
        app.run("photo.setReference", json!({})).unwrap();
        let pin = app.ui.reference;
        crate::import::open(&mut app, vec![dir.to_string_lossy().into_owned()]).unwrap();
        assert!(app.scan.is_some());
        let err = open_library(&mut app, &json!({"path": "other"})).unwrap_err();
        assert!(err.contains("import to finish"), "{err}");
        assert_eq!(app.ui.reference, pin, "a refused Open Library during a scan keeps the reference pin");
        let err = new_library(&mut app, &json!({"path": "other"})).unwrap_err();
        assert!(err.contains("import to finish"), "{err}");
        assert_eq!(app.ui.reference, pin, "a refused New Library during a scan keeps the reference pin");
        assert!(app.scan.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}