//! Starting the app on the engine: where the library lives, opening it (or falling back to the demo
//! library), the memory budget, and the staging folders the system pickers and the share sheet use.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use lightcraft_engine::Session;

/// Where the library lives: `library` (`$LIGHTCRAFT_LIBRARY`), else `LightCraft Library` in the
/// Documents folder under `home` (on iOS `$HOME` is the app's container: kept across launches and
/// updates, and backed up).
pub fn library_dir(library: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    if let Some(p) = library.filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    home.filter(|h| !h.is_empty()).map(|h| PathBuf::from(h).join("Documents").join("LightCraft Library"))
}

/// The library at `dir` (a new one starts with the demo photos), or why it can't be opened.
fn open_dir(dir: &Path) -> Result<Session, String> {
    let mut s = Session::new().with_fs().with_system_clock();
    match s.open_library(dir, true) {
        Ok(r) => {
            let replayed = r.replayed;
            log::info!("library {}: {} photos, {replayed} log records replayed", dir.display(), s.catalog.len());
            Ok(s)
        }
        Err(e) => Err(format!("can't open the library {}: {e}", dir.display())),
    }
}

/// The library at `dir`, or, when it can't be opened, the in-memory demo library (nothing saved,
/// no sync) with the reason in the log. The second value says why the fallback was taken.
pub fn open_session(dir: Option<&Path>) -> (Session, Option<String>) {
    let why = match dir.map(open_dir) {
        Some(Ok(s)) => return (s, None),
        Some(Err(why)) => why,
        None => "no home folder for the library".to_string(),
    };
    log::error!("{why}; using an in-memory demo library (not saved, can't sync)");
    (Session::with_demo().with_fs().with_system_clock(), Some(why))
}

/// Face models on a phone (chosen and downloaded by the user, kept with the app's data so iOS backs
/// them up and never empties them): `Library/Application Support/LightCraft/face-models`. Without a
/// home folder there is nowhere to keep them (Settings ▸ Faces says so).
pub fn face_models_dir(home: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    home.map(|h| PathBuf::from(h).join("Library/Application Support/LightCraft/face-models"))
}

/// Size the engine's caches to what the system lets this app use (`available`, from
/// `lightcraft_sysmem::available_memory`; `None` assumes little), before anything is cached, and
/// hand freed allocator pages back to the system after big decodes. Returns the budget in bytes.
pub fn set_memory_budget(available: Option<usize>) -> usize {
    let budget = lightcraft_engine::memory::set_budget(lightcraft_engine::memory::budget_for_limit(available));
    log::info!("memory: {} MB available to the app, budget {} MB", available.map_or(0, |a| a >> 20), budget >> 20);
    lightcraft_engine::memory::set_release_hook(|| {
        let _ = lightcraft_sysmem::release_free_memory();
    });
    budget
}

/// The staging folders in the app's tmp folder: copies the pickers made (moved into the library by
/// the import review) and the last export (handed to the share sheet).
pub fn staging(tmp: &Path) -> (PathBuf, PathBuf) {
    (tmp.join("Import"), tmp.join("Exports"))
}

/// Empty what a previous run left in the staging folders: picks the review didn't take (cancelled,
/// duplicates) and exports that were shared or dismissed.
pub fn clean_staging(tmp: &Path) {
    let (import, export) = staging(tmp);
    for d in [import, export] {
        if let Err(e) = std::fs::remove_dir_all(&d)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            log::warn!("{}: {e}", d.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staging_is_cleaned_at_launch() {
        let tmp = std::env::temp_dir().join(format!("lc-ios-staging-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let (import, export) = staging(&tmp);
        std::fs::create_dir_all(import.join("pick-1")).unwrap();
        std::fs::create_dir_all(&export).unwrap();
        std::fs::write(export.join("old.jpg"), b"x").unwrap();
        clean_staging(&tmp);
        assert!(!import.exists() && !export.exists());
        clean_staging(&tmp); // nothing there: fine
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn the_library_is_kept_in_documents() {
        let dir = library_dir(None, Some("/var/mobile/Containers/Data/Application/X".into()));
        assert_eq!(dir, Some(PathBuf::from("/var/mobile/Containers/Data/Application/X/Documents/LightCraft Library")));
        assert_eq!(library_dir(Some("/tmp/lib".into()), Some("/home".into())), Some(PathBuf::from("/tmp/lib")));
        assert_eq!(library_dir(Some("".into()), None), None);
    }

    /// A library that can't be opened (here, one already open) starts the demo library in memory.
    #[test]
    fn a_library_open_elsewhere_falls_back_to_the_demo() {
        let root = std::env::temp_dir().join(format!("lc-ios-locked-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("lib");
        let (holder, why) = open_session(Some(&dir));
        assert!(why.is_none() && dir.is_dir());
        let (demo, why) = open_session(Some(&dir));
        assert!(why.is_some_and(|w| w.contains("already open")), "the second opening falls back");
        assert!(!demo.catalog.is_empty(), "to the demo library");
        drop(holder);
        let _ = std::fs::remove_dir_all(&root);
    }
}
