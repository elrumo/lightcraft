//! The libraries a frontend switches between: egui's File ▸ Open Library…, New Library… and Open
//! Recent Library, and a native frontend's list of libraries (`host.libraries`, `host.library.*`).
//! What is shared: switching (the library asked for is checked before the open one is closed, and
//! the open one comes back when the other still fails), a new library's folder, the recent list
//! (the app keeps it with its own settings and passes it in), and a library described without
//! opening it: its name, whether it is there, when it last changed and the server it syncs with.

use std::path::{Path, PathBuf};

use lightcraft_engine::{EngineError, Session};
use serde_json::{Value, json};

/// How many libraries the recent list keeps.
pub const RECENT: usize = 8;

/// The most library folders [`found_in`] lists from one folder (input from the disk: bounded).
const FOUND_MAX: usize = 200;

/// Characters a folder name can't hold on Windows (and `/`, which no system allows).
const RESERVED_NAME_CHARS: [char; 9] = ['<', '>', ':', '"', '/', '\\', '|', '?', '*'];

/// Put `path` first in the recent list, without duplicates, capped at [`RECENT`].
pub fn remember(recent: &mut Vec<String>, path: &str) {
    recent.retain(|r| r != path);
    recent.insert(0, path.to_string());
    recent.truncate(RECENT);
}

/// A library's name as people see it: its folder's name.
pub fn name(dir: &Path) -> String {
    dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| dir.display().to_string())
}

/// `s` as a new library's folder name (trimmed), or why it can't be one.
pub fn valid_name(s: &str) -> Result<&str, String> {
    let n = s.trim();
    // Windows drops trailing dots and spaces from a folder name: refuse them rather than
    // creating a folder whose name differs from the one asked for (spaces around the name
    // are trimmed first)
    let bad = n.is_empty() || n == "." || n == ".." || n.ends_with(['.', ' ']) || n.contains(RESERVED_NAME_CHARS) || n.chars().any(char::is_control);
    if bad {
        return Err(format!("{s:?} can't be a library name (a folder name without < > : \" / \\ | ? * and not ending in a dot or space)"));
    }
    Ok(n)
}

/// `base` inside `location`, or `base 2`, `base 3`… if that is taken.
pub fn free_name(location: &Path, base: &str) -> PathBuf {
    let first = location.join(base);
    if !first.exists() {
        return first;
    }
    (2..1000u32).map(|i| location.join(format!("{base} {i}"))).find(|p| !p.exists()).unwrap_or(first)
}

/// Make a new library's folder: `name` inside `location`; without a name, a new folder named after
/// the default library inside a `picked` location (one chosen in a dialog), else `location` itself.
/// The folder must not hold a library already, nor anything else. Returns the folder, made.
pub fn new_dir(location: &Path, name: Option<&str>, picked: bool) -> Result<PathBuf, String> {
    let dir = match name {
        Some(n) => location.join(valid_name(n)?),
        None if picked => free_name(location, lightcraft_engine::library::DEFAULT_NAME),
        None => location.to_path_buf(),
    };
    if lightcraft_engine::library::is_library(&dir) {
        return Err(format!("{}: there is a library here already: open it, or choose another name", dir.display()));
    }
    if dir.is_file() {
        return Err(format!("{}: is a file; a library is a folder", dir.display()));
    }
    if std::fs::read_dir(&dir).is_ok_and(|mut d| d.next().is_some()) {
        return Err(format!("{}: the folder isn't empty; choose an empty folder or a new name", dir.display()));
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: can't create the library folder: {e}", dir.display()))?;
    Ok(dir)
}

/// What [`switch`] did.
#[derive(Debug)]
pub struct Switched {
    /// The library open now.
    pub dir: PathBuf,
    /// The library open before (on disk), to keep in the recent list.
    pub previous: Option<PathBuf>,
}

/// Close the open library and open the one at `path` (a library is made there when the folder
/// holds none). It is checked first ([`Session::check_library`]: not a Lightroom catalog, a
/// readable folder, not open in another program), so a refusal leaves the open library as it was;
/// if opening it still fails, the previous library is opened again (both errors are said if that
/// fails too). The error keeps the engine's kind (`LibraryInUse`, `LightroomCatalog`…), its
/// message the path.
pub fn switch(session: &mut Session, path: &Path) -> Result<Switched, EngineError> {
    let dir = session.check_library(path).map_err(|e| at(path, e))?;
    // the library being left goes in the recent list too (also when it was opened at launch)
    let previous = session.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.clone());
    session.close_library()?;
    if let Err(e) = session.open_library(&dir, false) {
        let err = at(&dir, e);
        let Some(prev) = previous else { return Err(err) };
        return match session.open_library(&prev, false) {
            Ok(_) => Err(err),
            Err(e) => Err(EngineError::Other(format!("{err}; reopening the previous library {} failed too: {e}", prev.display()))),
        };
    }
    Ok(Switched { dir, previous })
}

/// `e` with the library's path in front of its message, of the same kind.
fn at(path: &Path, e: EngineError) -> EngineError {
    match e {
        EngineError::LibraryInUse(m) => EngineError::LibraryInUse(format!("{}: {m}", path.display())),
        // (its message names the folder)
        e @ EngineError::LightroomCatalog { .. } => e,
        e => EngineError::Other(format!("{}: {e}", path.display())),
    }
}

/// The library `session` has open, when it is a folder on disk.
pub fn open_dir(session: &Session) -> Option<&Path> {
    session.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.as_path())
}

/// Whether two paths name the same folder, however they are spelled.
pub fn same(a: &Path, b: &Path) -> bool {
    crate::local::same_folder(&a.to_string_lossy(), &b.to_string_lossy())
}

/// The recent list as egui's Open Recent Library and `app.recentLibraries` give it, newest first:
/// `[{path, name, exists, open}]` (`exists: false`: moved, or on a disconnected drive).
pub fn recent_list(recent: &[String], open: Option<&Path>) -> Vec<Value> {
    recent
        .iter()
        .map(|r| {
            let d = Path::new(r);
            json!({"path": r, "name": name(d), "exists": d.is_dir(), "open": open.is_some_and(|o| same(o, d))})
        })
        .collect()
}

/// A library described without opening it: `{name, exists, library (its catalog is there), open,
/// modified (when its catalog last changed, Unix seconds), sync}`. `sync` is `null` for a library
/// that never synced, else `{server, user, signedIn, joined (it is a copy of the account's
/// library)}`, from its `sync.json` (the open library's from `session`, which is newer). Reads a
/// few small files: the folder isn't listed.
pub fn describe(dir: &Path, session: &Session) -> Value {
    let open = open_dir(session).is_some_and(|o| same(o, dir));
    let sync = if open {
        session.sync_state().map(|st| sync_json(&st.config, st.signed_in()))
    } else {
        sync_config(dir).map(|c| {
            let signed_in = !c.token.is_empty();
            sync_json(&c, signed_in)
        })
    };
    json!({
        "name": name(dir),
        "exists": dir.is_dir(),
        "library": lightcraft_engine::library::is_library(dir),
        "open": open,
        "modified": modified(dir),
        "sync": sync,
    })
}

fn sync_json(c: &lightcraft_engine::cloud_sync::SyncConfig, signed_in: bool) -> Value {
    json!({"server": c.server, "user": c.user, "signedIn": signed_in, "joined": !c.library.is_empty()})
}

/// A library's sync settings as its folder holds them (`None`: it never synced, or the file is
/// unreadable).
pub fn sync_config(dir: &Path) -> Option<lightcraft_engine::cloud_sync::SyncConfig> {
    let bytes = std::fs::read(dir.join(lightcraft_engine::cloud_sync::CONFIG)).ok()?;
    let c: lightcraft_engine::cloud_sync::SyncConfig = serde_json::from_slice(&bytes).ok()?;
    // (a library signed out and emptied keeps `{}`: one that never synced)
    (c != lightcraft_engine::cloud_sync::SyncConfig::default()).then_some(c)
}

/// The changes a library's device hasn't sent to its server yet (0: none, or it never synced).
pub fn unsent(dir: &Path) -> usize {
    std::fs::read(dir.join(lightcraft_engine::cloud_sync::OUTBOX))
        .ok()
        .and_then(|b| serde_json::from_slice::<lightcraft_catalog::sync::Outbox>(&b).ok())
        .map_or(0, |o| o.len())
}

/// When a library's catalog last changed (its log or its snapshot), in Unix seconds.
fn modified(dir: &Path) -> Option<u64> {
    [lightcraft_catalog::journal::LOG, lightcraft_catalog::journal::SNAPSHOT]
        .iter()
        .filter_map(|f| std::fs::metadata(dir.join(f)).and_then(|m| m.modified()).ok())
        .filter_map(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .max()
}

/// The libraries directly inside `location` (where an app keeps the ones it makes), by name; not
/// the hidden folders. At most [`FOUND_MAX`].
pub fn found_in(location: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(location) else { return Vec::new() };
    let mut found: Vec<PathBuf> = rd
        .flatten()
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .filter(|p| lightcraft_engine::library::is_library(p))
        .take(FOUND_MAX)
        .collect();
    found.sort_by_key(|p| name(p).to_lowercase());
    found
}

/// Bytes the files under `dir` take, and whether the count stopped at `max_files` (a library is
/// mostly its originals and previews: the walk is bounded). Links are not followed.
pub fn size_on_disk(dir: &Path, max_files: usize) -> (u64, bool) {
    let (mut bytes, mut files) = (0u64, 0usize);
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let Ok(t) = e.file_type() else { continue };
            if t.is_dir() {
                todo.push(e.path());
            } else if t.is_file() {
                files += 1;
                if files > max_files {
                    return (bytes, true);
                }
                bytes = bytes.saturating_add(e.metadata().map_or(0, |m| m.len()));
            }
        }
    }
    (bytes, false)
}

/// A path as an app keeps it: `~/…` inside `home`, whose absolute path an iOS app's container
/// changes with each update; any other path as it is.
pub fn portable(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|h| path.strip_prefix(h).ok()) {
        Some(rest) if !rest.as_os_str().is_empty() => format!("~/{}", rest.to_string_lossy()),
        _ => path.to_string_lossy().into_owned(),
    }
}

/// A path an app kept ([`portable`]): `~/…` is inside `home` (without a home it stays as it is).
pub fn expand(path: &str, home: Option<&Path>) -> PathBuf {
    match (path.strip_prefix("~/"), home) {
        (Some(rest), Some(h)) => h.join(rest),
        _ if path == "~" => home.map_or_else(|| PathBuf::from(path), Path::to_path_buf),
        _ => PathBuf::from(path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lc-apphost-libs-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn session_in(dir: &Path) -> Session {
        let mut s = Session::new().with_fs();
        s.open_library(dir, false).unwrap();
        s
    }

    #[test]
    fn the_recent_list_is_newest_first_without_duplicates_and_capped() {
        let mut recent = Vec::new();
        for i in 0..20 {
            remember(&mut recent, &format!("/x/{i}"));
        }
        remember(&mut recent, "/x/15");
        assert_eq!(recent.len(), RECENT);
        assert_eq!(recent[0], "/x/15");
        assert_eq!(recent[1], "/x/19");
        assert_eq!(recent.iter().filter(|r| *r == "/x/15").count(), 1);
    }

    #[test]
    fn library_names_are_folder_names_a_system_keeps() {
        assert_eq!(valid_name("  Client A "), Ok("Client A"));
        for bad in ["", " ", ".", "..", "a/b", "a\\b", "x:", "name.", "a\tb", "what?"] {
            assert!(valid_name(bad).is_err_and(|e| e.contains("library name")), "{bad:?}");
        }
    }

    /// A new library's folder is made empty; a library, a file or anything else already there is refused.
    #[test]
    fn a_new_librarys_folder_must_be_new_or_empty() {
        let root = temp("new");
        let a = new_dir(&root, Some("Client A"), false).unwrap();
        assert_eq!(a, root.join("Client A"));
        assert!(a.is_dir());
        // a library made there: refused
        drop(session_in(&a));
        assert!(new_dir(&root, Some("Client A"), false).is_err_and(|e| e.contains("library here already")));
        std::fs::write(root.join("file"), b"x").unwrap();
        assert!(new_dir(&root, Some("file"), false).is_err_and(|e| e.contains("is a file")));
        std::fs::create_dir_all(root.join("full")).unwrap();
        std::fs::write(root.join("full/photo.jpg"), b"x").unwrap();
        assert!(new_dir(&root, Some("full"), false).is_err_and(|e| e.contains("isn't empty")));
        assert!(new_dir(&root, Some("../escape"), false).is_err());
        // a chosen place without a name: a new folder named after the default library, then "… 2"
        let first = new_dir(&root, None, true).unwrap();
        assert_eq!(first, root.join(lightcraft_engine::library::DEFAULT_NAME));
        let second = new_dir(&root, None, true).unwrap();
        assert_eq!(second, root.join(format!("{} 2", lightcraft_engine::library::DEFAULT_NAME)));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A library another program has open is refused before the open one is closed; one that opens
    /// becomes the open one, and the one before is said.
    #[test]
    fn switching_checks_first_and_keeps_the_open_library_on_a_refusal() {
        let root = temp("switch");
        let (a, b) = (root.join("a"), root.join("b"));
        drop(session_in(&b));
        let mut s = session_in(&a);
        let generation = s.library_generation();
        // "another program" holds b's lock
        let held = lightcraft_catalog::LibraryLock::acquire(&b, "someone else").unwrap();
        let e = switch(&mut s, &b).unwrap_err();
        assert!(matches!(e, EngineError::LibraryInUse(_)), "{e}");
        assert!(e.to_string().contains("already open") && e.to_string().contains(&*b.to_string_lossy()), "{e}");
        assert!(open_dir(&s).is_some_and(|d| same(d, &a)), "the open library stays");
        assert_eq!(s.library_generation(), generation, "nothing was reopened");
        drop(held);
        let done = switch(&mut s, &b).unwrap();
        assert!(same(&done.dir, &b) && done.previous.as_deref().is_some_and(|p| same(p, &a)));
        assert!(open_dir(&s).is_some_and(|d| same(d, &b)));
        assert_ne!(s.library_generation(), generation, "frontends forget the old library's photos");
        // a file is no library
        std::fs::write(root.join("file"), b"x").unwrap();
        assert!(switch(&mut s, &root.join("file")).is_err());
        assert!(open_dir(&s).is_some_and(|d| same(d, &b)));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The list a native frontend shows: each library's name, whether it is there and open, and the
    /// server it syncs with, read from its folder without opening it (never the token).
    #[test]
    fn a_library_is_described_without_opening_it() {
        let root = temp("describe");
        let (a, b) = (root.join("a"), root.join("b"));
        drop(session_in(&b));
        std::fs::write(
            b.join(lightcraft_engine::cloud_sync::CONFIG),
            br#"{"server": "https://photos.example.com", "user": "ann", "token": "secret", "library": "lib-1"}"#,
        )
        .unwrap();
        let s = session_in(&a);
        let d = describe(&b, &s);
        assert_eq!(d["name"], "b");
        assert_eq!((d["exists"].as_bool(), d["library"].as_bool(), d["open"].as_bool()), (Some(true), Some(true), Some(false)));
        assert!(d["modified"].as_u64().is_some_and(|t| t > 1_600_000_000), "{d}");
        assert_eq!(d["sync"], json!({"server": "https://photos.example.com", "user": "ann", "signedIn": true, "joined": true}));
        assert!(!d.to_string().contains("secret"));
        let d = describe(&a, &s);
        assert_eq!((d["open"].as_bool(), d["sync"].is_null()), (Some(true), true), "{d}");
        let gone = describe(&root.join("gone"), &s);
        assert_eq!((gone["exists"].as_bool(), gone["library"].as_bool(), gone["modified"].is_null()), (Some(false), Some(false), true));
        // signed out and emptied (`{}`): never synced
        std::fs::write(b.join(lightcraft_engine::cloud_sync::CONFIG), b"{}").unwrap();
        assert!(describe(&b, &s)["sync"].is_null());
        assert_eq!(unsent(&b), 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_libraries_in_a_folder_are_listed_by_name() {
        let root = temp("found");
        for n in ["Zoo", "alpha", ".hidden"] {
            drop(session_in(&root.join(n)));
        }
        std::fs::create_dir_all(root.join("Photos")).unwrap();
        std::fs::write(root.join("notes.txt"), b"x").unwrap();
        let names: Vec<String> = found_in(&root).iter().map(|p| name(p)).collect();
        assert_eq!(names, ["alpha", "Zoo"]);
        assert!(found_in(&root.join("missing")).is_empty());
        let (bytes, capped) = size_on_disk(&root, 10_000);
        assert!(bytes > 0 && !capped);
        assert!(size_on_disk(&root, 1).1, "a bounded walk says it stopped");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// An iOS app's container moves with each update: paths inside it are kept as `~/…`.
    #[test]
    fn paths_inside_the_home_folder_are_kept_relative_to_it() {
        let home = Path::new("/var/mobile/Containers/Data/Application/A");
        let lib = home.join("Documents/Client A");
        assert_eq!(portable(&lib, Some(home)), "~/Documents/Client A");
        assert_eq!(
            expand("~/Documents/Client A", Some(Path::new("/var/mobile/Containers/Data/Application/B"))),
            Path::new("/var/mobile/Containers/Data/Application/B/Documents/Client A")
        );
        assert_eq!(portable(Path::new("/Volumes/Photos/Lib"), Some(home)), "/Volumes/Photos/Lib");
        assert_eq!(portable(home, Some(home)), home.to_string_lossy());
        assert_eq!(portable(&lib, None), lib.to_string_lossy());
        assert_eq!(expand("/abs/lib", Some(home)), Path::new("/abs/lib"));
        assert_eq!(expand("~/x", None), Path::new("~/x"));
        assert_eq!(expand("~", Some(home)), home);
    }
}
