//! Starting an app on the engine: where the library lives, opening it (or falling back to the demo
//! library), the memory budget, and the staging folders the system pickers and the share sheet use.
//! The functions without a `_for` / `desktop` in their name are a phone's rules, shared by the iOS
//! apps (egui and SwiftUI), so both start the same way; a [`Profile`] picks a Mac's instead.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use lightcraft_engine::Session;
use serde::{Deserialize, Serialize};

/// How a host starts the engine (`OpenConfig::profile`). `Mobile` is a phone's rules: the app's
/// container, the memory the system lets the app use, the demo library when the real one won't
/// open. `Desktop` is a Mac's (the SwiftUI Mac app, as `apps/lightcraft` starts): the library in
/// the user's Pictures folder, the machine's RAM sizes the caches, AI models stay in the settings
/// folder shared with egui and the CLI, and a library that won't open (open in another LightCraft,
/// unreadable) is an error for the user to act on, never a demo library in its place.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Profile {
    #[default]
    Mobile,
    Desktop,
}

/// Where the library lives: `library` (`$LIGHTCRAFT_LIBRARY`), else `LightCraft Library` in the
/// Documents folder under `home` (on iOS `$HOME` is the app's container: kept across launches and
/// updates, backed up).
pub fn library_dir(library: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    library_dir_for(Profile::Mobile, library, home)
}

/// [`library_dir`] for `profile`: a desktop keeps it in the Pictures folder, like the egui app.
pub fn library_dir_for(profile: Profile, library: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    lightcraft_engine::library::default_dir_in(library, home, profile == Profile::Mobile)
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

/// The library at `dir` (a new one starts with the demo photos), or, when it can't be opened, the
/// in-memory demo library (nothing saved, no sync) with the reason in the log. The second value
/// says why the fallback was taken.
pub fn open_session(dir: Option<&Path>) -> (Session, Option<String>) {
    let why = match dir.map(open_dir) {
        Some(Ok(s)) => return (s, None),
        Some(Err(why)) => why,
        None => "no home folder for the library".to_string(),
    };
    log::error!("{why}; using an in-memory demo library (not saved, can't sync)");
    (Session::with_demo().with_fs().with_system_clock(), Some(why))
}

/// [`open_session`] for a desktop host: a library that won't open is the user's to fix (it is open
/// in another LightCraft, its folder can't be read), so the reason is an error, not a demo.
pub fn open_session_or_err(dir: Option<&Path>) -> Result<Session, String> {
    let opened = dir.ok_or_else(|| "there is no home folder to keep the library in: choose a folder for it".to_string()).and_then(open_dir);
    if let Err(why) = &opened {
        log::error!("{why}");
    }
    opened
}

/// What a desktop host sets on the session, as `apps/lightcraft` does: the AI models and the face
/// models in the settings folder (shared with egui, the CLI and MCP), and Add to Apple Photos on a
/// worker, since it waits for the user to answer macOS's Automation prompt.
pub fn configure_desktop(session: &mut Session) {
    session.enhancer.dir = lightcraft_engine::config::config_dir().map(|d| d.join("models"));
    session.face_models_dir = lightcraft_engine::config::default_face_models_dir();
    session.apple_photos_background = true;
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

/// Face models on a phone (chosen and downloaded by the user, kept with the app's data so iOS
/// backs them up and never empties them): `Library/Application Support/LightCraft/face-models`.
/// Without a home folder there is nowhere to keep them (Settings ▸ Faces says so).
pub fn face_models_dir(home: Option<&OsStr>) -> Option<PathBuf> {
    home.map(|h| PathBuf::from(h).join("Library/Application Support/LightCraft/face-models"))
}

/// Size the engine's caches to what the system lets this app use (`available`, from
/// `lightcraft_sysmem::available_memory`; `None` keeps the default), before anything is cached, and
/// hand freed allocator pages back to the system after big decodes. Returns the budget in bytes.
pub fn set_memory_budget(available: Option<usize>) -> usize {
    set_memory_budget_for(Profile::Mobile, available)
}

/// The cache budget for `profile`: a phone's is a third of what the system lets the app use
/// ([`lightcraft_engine::memory::budget_for_limit`]); a desktop's, unless the host names a limit,
/// is a quarter of the machine's RAM, 256 MiB to 1.5 GiB ([`lightcraft_engine::memory::default_budget`]).
pub fn budget_for(profile: Profile, available: Option<usize>) -> usize {
    match (profile, available) {
        (Profile::Desktop, None) => lightcraft_engine::memory::default_budget(),
        _ => lightcraft_engine::memory::budget_for_limit(available),
    }
}

/// [`set_memory_budget`] for `profile` ([`budget_for`]).
pub fn set_memory_budget_for(profile: Profile, available: Option<usize>) -> usize {
    let budget = lightcraft_engine::memory::set_budget(budget_for(profile, available));
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

    /// A Mac keeps the library in Pictures, as the egui app does; a phone, in the app's Documents.
    #[test]
    fn a_desktop_keeps_its_library_in_pictures() {
        let home = Some(OsString::from("/Users/ann"));
        assert_eq!(library_dir_for(Profile::Desktop, None, home.clone()), Some(PathBuf::from("/Users/ann/Pictures/LightCraft Library")));
        assert_eq!(library_dir_for(Profile::Mobile, None, home.clone()), library_dir(None, home.clone()));
        assert_eq!(library_dir_for(Profile::Desktop, Some("/Volumes/Photos/lib".into()), home), Some(PathBuf::from("/Volumes/Photos/lib")));
        // (the phone's rules are what the egui iOS app and a host without a profile get)
        assert_eq!(Profile::default(), Profile::Mobile);
        assert_eq!(serde_json::from_str::<Profile>("\"desktop\"").ok(), Some(Profile::Desktop));
    }

    /// A Mac's caches follow its RAM, not the 768 MiB a phone guesses when the system won't say.
    #[test]
    fn a_desktops_caches_follow_its_ram_unless_the_host_names_a_limit() {
        use lightcraft_engine::memory;
        assert_eq!(budget_for(Profile::Mobile, None), 768 << 20);
        assert_eq!(budget_for(Profile::Desktop, None), memory::default_budget());
        assert_eq!(budget_for(Profile::Desktop, Some(3 << 30)), memory::budget_for_limit(Some(3 << 30)));
    }

    /// The egui app holds an OS lock on `~/Pictures/LightCraft Library`: a Mac host that is second
    /// must say so (the user closes the other one or picks another library), where a phone's
    /// fallback to the demo library would let them import into a library that isn't theirs.
    #[test]
    fn a_library_open_elsewhere_is_an_error_on_a_desktop_and_the_demo_on_a_phone() {
        let root = std::env::temp_dir().join(format!("lc-apphost-locked-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("lib");
        let (holder, why) = open_session(Some(&dir));
        assert!(why.is_none() && dir.is_dir());
        let (_demo, why) = open_session(Some(&dir));
        assert!(why.is_some_and(|w| w.contains("already open")), "the phone falls back to the demo library");
        let err = open_session_or_err(Some(&dir)).err().unwrap_or_default();
        assert!(err.contains("already open") && err.contains("can't open the library"), "{err}");
        assert!(open_session_or_err(None).is_err());
        drop(holder);
        assert!(open_session_or_err(Some(&dir)).is_ok(), "closed elsewhere: it opens");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The profile reaches the engine thread: a desktop host opens the library it was given and,
    /// when another LightCraft has it, refuses with the reason; a phone's host (the default) falls
    /// back to the in-memory demo library and warns.
    #[test]
    fn the_host_starts_by_profile() {
        use crate::host::{Host, OpenConfig};
        let root = std::env::temp_dir().join(format!("lc-apphost-profile-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (tmp, lib) = (root.join("tmp"), root.join("lib"));
        std::fs::create_dir_all(&tmp).unwrap();
        let config = |profile| OpenConfig {
            profile,
            library: Some(lib.to_string_lossy().into()),
            tmp: Some(tmp.to_string_lossy().into()),
            render_threads: Some(1),
            ..Default::default()
        };
        let (desktop, opened) = Host::open(config(Profile::Desktop)).unwrap();
        assert_eq!(opened["demo"], false, "{opened}");
        assert!(opened["library"].as_str().is_some_and(|p| p.ends_with("lib")), "{opened}");
        let err = Host::open(config(Profile::Desktop)).err().unwrap_or_default();
        assert!(err.contains("already open"), "{err}");
        let (mobile, opened) = Host::open(config(Profile::Mobile)).unwrap();
        assert_eq!(opened["demo"], true, "{opened}");
        assert!(opened["warnings"][0].as_str().is_some_and(|w| w.contains("already open")), "{opened}");
        mobile.close(std::time::Duration::from_secs(5));
        desktop.close(std::time::Duration::from_secs(5));
        let _ = std::fs::remove_dir_all(&root);
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
        // face models stay with the app's data, never in the caches
        assert_eq!(
            face_models_dir(Some(&home)).as_deref(),
            Some(Path::new("/var/mobile/Containers/Data/Application/ABC/Library/Application Support/LightCraft/face-models"))
        );
        assert_eq!(face_models_dir(None), None);
    }
}
