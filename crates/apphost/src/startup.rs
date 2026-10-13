//! Starting a mobile app on the engine: where the library lives, opening it (or falling back to
//! the demo library), the memory budget, and the staging folders the system pickers and the share
//! sheet use. Shared by the iOS apps (egui and SwiftUI), so both start the same way.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use lightcraft_engine::Session;

/// Where the library lives: `library` (`$LIGHTCRAFT_LIBRARY`), else `LightCraft Library` in the
/// Documents folder under `home` (on iOS `$HOME` is the app's container: kept across launches and
/// updates, backed up).
pub fn library_dir(library: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    lightcraft_engine::library::default_dir_in(library, home, true)
}

/// The library at `dir` (a new one starts with the demo photos), or, when it can't be opened, the
/// in-memory demo library (nothing saved, no sync) with the reason in the log. The second value
/// says why the fallback was taken.
pub fn open_session(dir: Option<&Path>) -> (Session, Option<String>) {
    let why = match dir {
        Some(dir) => {
            let mut s = Session::new().with_fs().with_system_clock();
            match s.open_library(dir, true) {
                Ok(r) => {
                    let replayed = r.replayed;
                    log::info!("library {}: {} photos, {replayed} log records replayed", dir.display(), s.catalog.len());
                    return (s, None);
                }
                Err(e) => format!("can't open the library {}: {e}", dir.display()),
            }
        }
        None => "no home folder for the library".to_string(),
    };
    log::error!("{why}; using an in-memory demo library (not saved, can't sync)");
    (Session::with_demo().with_fs().with_system_clock(), Some(why))
}

/// AI Super Resolution on a phone: the model (a few MB, downloaded when the user agrees) goes in
/// `Library/Caches`, which iOS never backs up and may empty when storage runs low (the app offers
/// the download again), and the work is sized to half of the memory the system lets the app use
/// (the caches have their own third).
pub fn configure_enhancer(enhancer: &mut lightcraft_engine::enhance::Enhancer, home: Option<&OsStr>, available: Option<usize>) {
    enhancer.dir = home.map(|h| PathBuf::from(h).join("Library/Caches/LightCraft/models"));
    // (unknown: assume little)
    enhancer.limit_memory(available.map_or(512 << 20, |a| a / 2));
}

/// Size the engine's caches to what the system lets this app use (`available`, from
/// `lightcraft_sysmem::available_memory`; `None` keeps the default), before anything is cached, and
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

/// Empty what a previous run left in the staging folders: picks the review didn't take
/// (cancelled, duplicates) and exports that were shared or dismissed.
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
        let tmp = std::env::temp_dir().join(format!("lc-apphost-staging-{}", std::process::id()));
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

    #[test]
    fn a_saved_library_can_sign_in_and_the_fallback_says_why_not() {
        let root = std::env::temp_dir().join(format!("lc-apphost-library-{}", std::process::id()));
        let home = root.join("home");
        let dir = library_dir(None, Some(home.into_os_string())).unwrap();
        let (mut s, _) = open_session(Some(&dir));
        assert_ne!(s.catalog.len(), 0, "a new library starts with the demo photos");
        assert!(dir.is_dir());
        s.sync_sign_in("photos.example.com", "ann", "a password", "iPhone").unwrap();
        drop(s);
        // reopened: the same library, still signed in to the same server
        let (s, _) = open_session(Some(&dir));
        assert_eq!(s.sync_state().map(|st| st.status()["server"].clone()), Some("https://photos.example.com".into()));
        drop(s);
        // a library that can't be opened (a file in the way) falls back to the demo, which can't sync
        let blocked = root.join("blocked");
        std::fs::write(&blocked, b"not a folder").unwrap();
        let (mut s, why) = open_session(Some(&blocked));
        assert!(why.is_some_and(|w| w.contains("can't open the library")));
        assert_ne!(s.catalog.len(), 0);
        assert!(s.sync_sign_in("https://photos.example.com", "ann", "a password", "iPhone").is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Super Resolution on the phone: the model goes where iOS never backs it up, and the work is
    /// sized to the memory the app has.
    #[test]
    fn the_phone_keeps_models_in_caches_and_sizes_the_work_to_its_memory() {
        use lightcraft_engine::enhance::Enhancer;
        let home = std::ffi::OsString::from("/var/mobile/Containers/Data/Application/ABC");
        let mut e = Enhancer::default();
        configure_enhancer(&mut e, Some(&home), Some(3 << 30));
        assert_eq!(e.dir.as_deref(), Some(Path::new("/var/mobile/Containers/Data/Application/ABC/Library/Caches/LightCraft/models")));
        // half of 3 GB: small tiles, and room for a 9 MP photo's enlargement (36 MP out), not a 12 MP one's
        assert_eq!(e.tile, Some(192));
        let max = e.max_output_pixels.unwrap();
        assert!((34_000_000..40_000_000).contains(&max), "{max}");
        // an old phone with little to spare, and one that can't say: still a usable, smaller limit
        configure_enhancer(&mut e, Some(&home), Some(400 << 20));
        assert_eq!(e.max_output_pixels, Some(4_000_000));
        configure_enhancer(&mut e, Some(&home), None);
        assert!(e.max_output_pixels.is_some_and(|m| m >= 4_000_000 && m < max));
        // no home folder: nowhere to put it (the dialog says so), never a guess
        configure_enhancer(&mut e, None, Some(3 << 30));
        assert_eq!(e.dir, None);
    }
}
