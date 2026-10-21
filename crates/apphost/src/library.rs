//! Opening another library, making a new one, and the libraries opened lately: what File ▸ Open
//! Library…, New Library… and Open Recent Library do in every frontend (egui's menus, a native
//! frontend's through the engine host's `host.library.*`).
//!
//! - A library that can't be opened (open in another program, unreadable, a file) leaves the one
//!   open now open: it is checked before anything is closed, and if opening it still fails the
//!   previous library is opened again.
//! - A library is never made anew where one was. One in the recent list that isn't there now
//!   (moved, renamed, or on a disk or network share that isn't connected) is an error that says so,
//!   and it stays in the list for when its disk is back. A new library's folder is made only in a
//!   folder that is there: a path whose parent is missing too is a disk that isn't connected, not
//!   a place for a library. A host opening a library the user chose at start does the same
//!   ([`missing`]).
//! - The recent list: newest first, without duplicates, at most [`RECENT`]; the library being left
//!   goes in it too.

use std::path::{Path, PathBuf};

use lightcraft_engine::Session;
use serde::Serialize;

/// The most libraries the recent list keeps.
pub const RECENT: usize = 8;

/// The library just opened.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Opened {
    /// Its folder.
    pub path: String,
    /// Its name ([`name`]).
    pub name: String,
    /// Photos in its catalog.
    pub photos: usize,
}

/// A library in the recent list, as a menu shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Recent {
    pub path: String,
    pub name: String,
    /// The folder it is in (the menu's "Trips — /Volumes/photos").
    pub location: String,
    /// Its folder is there (`false`: moved, or on a disk or share that isn't connected).
    pub exists: bool,
    /// It is the library open now.
    pub open: bool,
}

/// What the engine host answers once a library is open (`host.library.open` / `new`): `{path, name, photos,
/// warnings, relocated, recent: [path]}`.
pub fn opened_answer(
    o: &Opened,
    warnings: &[String],
    relocated: Option<&lightcraft_engine::library::Relocated>,
    recent: &[String],
) -> serde_json::Value {
    serde_json::json!({"path": o.path, "name": o.name, "photos": o.photos, "warnings": warnings, "relocated": relocated, "recent": recent})
}

/// A library's name as people see it: its folder's name.
pub fn name(dir: &Path) -> String {
    dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| dir.display().to_string())
}

/// The open library's name (`None`: in memory, nothing is saved).
pub fn open_name(session: &Session) -> Option<String> {
    session.library.as_ref().filter(|l| l.on_disk).map(|l| name(&l.dir))
}

/// Put `path` first in the recent list, without duplicates, at most [`RECENT`].
pub fn remember(recent: &mut Vec<String>, path: &str) {
    recent.retain(|r| r != path);
    recent.insert(0, path.to_string());
    recent.truncate(RECENT);
}

/// The recent list as a menu shows it, `open` the library open now.
pub fn recent(recent: &[String], open: Option<&Path>) -> Vec<Recent> {
    recent
        .iter()
        .map(|r| {
            let d = Path::new(r);
            Recent {
                path: r.clone(),
                name: name(d),
                location: d.parent().map(|p| p.display().to_string()).unwrap_or_default(),
                exists: d.is_dir(),
                open: open == Some(d),
            }
        })
        .collect()
}

/// Why a library the user chose (`remembered`: one opened before, not a new one) can't be opened at
/// `dir` without making something where nothing should be: it isn't there, or (for a new one) the
/// folder it would be made in isn't either. `None`: go ahead.
pub fn missing(dir: &Path, remembered: bool) -> Option<String> {
    if dir.exists() {
        return None;
    }
    if remembered {
        return Some(format!(
            "{}: the library isn't there: it was moved or renamed, or the disk or network share it is on isn't connected",
            dir.display()
        ));
    }
    let parent = dir.parent().filter(|p| !p.as_os_str().is_empty())?;
    (!parent.is_dir())
        .then(|| format!("{}: the folder it would be made in isn't there (a disk or network share that isn't connected?)", dir.display()))
}

/// The library a folder picked for Open Library… holds: the folder itself, or the one folder inside it
/// that is a library (the drive or server folder a library sits in was picked). Never a folder made a
/// library by opening it: one with no library is an error that says so. A Lightroom catalog is
/// returned as it is, for opening it to say what it is.
pub fn find(dir: &Path) -> Result<PathBuf, String> {
    use lightcraft_engine::library::is_library;
    if is_library(dir) || lightcraft_engine::lightroom_folder::lightroom_catalog_at(dir).is_some() {
        return Ok(dir.to_path_buf());
    }
    if let Some(why) = missing(dir, true) {
        return Err(why);
    }
    let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    // (a share's top folder can hold thousands of entries: only folders, and only so many of them, are looked in)
    let mut inside: Vec<PathBuf> =
        entries.flatten().take(10_000).map(|e| e.path()).filter(|p| !hidden(p) && p.is_dir() && is_library(p)).take(9).collect();
    inside.sort();
    match inside.as_slice() {
        [one] => Ok(one.clone()),
        [] => Err(format!(
            "{}: there is no library here: choose a library's folder (or the folder it is in), or make one with New Library…",
            dir.display()
        )),
        many => Err(format!(
            "{}: there are {} libraries here ({}): choose the one to open",
            dir.display(),
            many.len(),
            many.iter().map(|p| name(p)).collect::<Vec<_>>().join(", ")
        )),
    }
}

/// A file or folder whose name starts with a dot (a share's `.snapshot`, `.Trashes`).
fn hidden(p: &Path) -> bool {
    p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'))
}

/// Close the library open now and open the one at `path` (made there when the folder is new or
/// empty), remembering both in `recent`. A library from `recent` that isn't there is refused, and
/// kept in the list. On an error the library open before stays open.
pub fn open(session: &mut Session, path: &str, recent: &mut Vec<String>) -> Result<Opened, String> {
    let dir = Path::new(path);
    if let Some(why) = missing(dir, recent.iter().any(|r| r == path)) {
        return Err(why);
    }
    switch(session, dir, recent)
}

/// Make a library and open it: the folder `name` inside `location`; without a name, `location`
/// itself, or (`picked`: a place chosen in a dialog) a new folder in it named after the default
/// library. The folder must not hold a library already, nor anything else.
pub fn new(session: &mut Session, location: &Path, name: Option<&str>, picked: bool, recent: &mut Vec<String>) -> Result<Opened, String> {
    let name = name.map(valid_name).transpose()?;
    let dir = match name {
        Some(n) => location.join(n),
        // a chosen location gets a new folder in it, named after the default library
        None if picked => free_name(location, lightcraft_engine::library::DEFAULT_NAME),
        None => location.to_path_buf(),
    };
    if lightcraft_engine::library::is_library(&dir) {
        return Err(format!("{}: there is a library here already; open it with File ▸ Open Library…", dir.display()));
    }
    if dir.is_file() {
        return Err(format!("{}: is a file; a library is a folder", dir.display()));
    }
    if std::fs::read_dir(&dir).is_ok_and(|mut d| d.next().is_some()) {
        return Err(format!("{}: the folder isn't empty; choose an empty folder or a new name", dir.display()));
    }
    if let Some(why) = missing(&dir, false) {
        return Err(why);
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: can't create the library folder: {e}", dir.display()))?;
    switch(session, &dir, recent)
}

/// Close the current library and open the one at `dir`: checked first, the previous library opened
/// again if it still fails (both errors are reported if that fails too).
fn switch(session: &mut Session, dir: &Path, recent: &mut Vec<String>) -> Result<Opened, String> {
    let dir = session.check_library(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.to_string_lossy().into_owned();
    // the library being left goes in the list too (also when it was opened at launch)
    let previous = session.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.clone());
    session.close_library().map_err(|e| format!("the library open now couldn't be saved, so it stays open: {e}"))?;
    if let Err(e) = session.open_library(&dir, false) {
        let err = format!("{path}: {e}");
        let Some(prev) = previous else { return Err(err) };
        return match session.open_library(&prev, false) {
            Ok(_) => Err(err),
            Err(e) => Err(format!("{err}; reopening the previous library {} failed too: {e}", prev.display())),
        };
    }
    if let Some(prev) = previous.map(|d| d.to_string_lossy().into_owned()).filter(|p| p != &path && !recent.contains(p)) {
        remember(recent, &prev);
    }
    remember(recent, &path);
    Ok(Opened { name: name(&dir), path, photos: session.catalog.len() })
}

/// Characters a folder name can't hold on Windows (and `/`, which no system allows).
const RESERVED_NAME_CHARS: [char; 9] = ['<', '>', ':', '"', '/', '\\', '|', '?', '*'];

/// `s` as a library's folder name (spaces around it trimmed), or why it can't be one.
pub fn valid_name(s: &str) -> Result<&str, String> {
    let n = s.trim();
    // Windows drops trailing dots and spaces from a folder name: refuse them rather than
    // creating a folder whose name differs from the one asked for
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

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lc-apphost-library-{tag}-{}", std::process::id()));
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
    fn opening_another_library_remembers_both_and_says_what_opened() {
        let dir = temp("open");
        let mut s = session_in(&dir.join("first"));
        let mut recent = Vec::new();
        let o = open(&mut s, &dir.join("second").to_string_lossy(), &mut recent).unwrap();
        assert_eq!(o.name, "second");
        assert_eq!(o.photos, 0);
        assert_eq!(s.library.as_ref().unwrap().dir, dir.join("second"));
        assert!(lightcraft_engine::library::is_library(&dir.join("second")));
        let names: Vec<_> = recent.iter().map(|r| name(Path::new(r))).collect();
        assert_eq!(names, ["second", "first"]);
        let shown = super::recent(&recent, Some(&dir.join("second")));
        assert!(shown[0].open && shown[0].exists && !shown[1].open);
        assert_eq!(shown[1].location, dir.display().to_string());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A library on a share that isn't connected: its folder is gone, so is the folder it was in.
    /// Opening it from the recent list says so, creates nothing, keeps the library open now, and
    /// keeps it in the list for when the share is back.
    #[test]
    fn a_remembered_library_that_isnt_there_is_never_made_anew() {
        let dir = temp("gone");
        let mut s = session_in(&dir.join("first"));
        let gone = dir.join("share").join("Library").to_string_lossy().into_owned();
        let mut recent = vec![gone.clone()];
        let e = open(&mut s, &gone, &mut recent).unwrap_err();
        assert!(e.contains("isn't connected"), "{e}");
        assert!(!Path::new(&gone).exists() && !dir.join("share").exists(), "nothing made in its place");
        assert_eq!(recent, std::slice::from_ref(&gone), "kept for when its disk is back");
        assert!(s.library.as_ref().unwrap().dir.ends_with("first"));
        // an empty mount point left by the share: still not a place to make the remembered library in
        std::fs::create_dir_all(dir.join("share")).unwrap();
        assert!(open(&mut s, &gone, &mut recent).is_err());
        assert!(!Path::new(&gone).exists());
        // a new library in a folder that isn't there either is refused too; in one that is, it's made
        let nowhere = dir.join("unmounted").join("New Library");
        let e = open(&mut s, &nowhere.to_string_lossy(), &mut Vec::new()).unwrap_err();
        assert!(e.contains("isn't there"), "{e}");
        assert!(!dir.join("unmounted").exists());
        open(&mut s, &dir.join("share").join("Other").to_string_lossy(), &mut recent).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_new_library_is_made_in_an_empty_folder_only() {
        let dir = temp("new");
        let mut s = session_in(&dir.join("first"));
        let mut recent = Vec::new();
        let o = new(&mut s, &dir, Some("  Client A "), false, &mut recent).unwrap();
        assert_eq!(o.name, "Client A");
        let e = new(&mut s, &dir, Some("Client A"), false, &mut recent).unwrap_err();
        assert!(e.contains("library here already"), "{e}");
        std::fs::create_dir_all(dir.join("full")).unwrap();
        std::fs::write(dir.join("full").join("x.txt"), b"x").unwrap();
        assert!(new(&mut s, &dir.join("full"), None, false, &mut recent).unwrap_err().contains("isn't empty"));
        for bad in ["a/b", "name.", "..", " ", "a:b"] {
            assert!(new(&mut s, &dir, Some(bad), false, &mut recent).unwrap_err().contains("library name"), "{bad:?}");
        }
        // a place picked in a dialog gets a new folder named after the default library
        let o = new(&mut s, &dir, None, true, &mut recent).unwrap();
        assert_eq!(o.name, lightcraft_engine::library::DEFAULT_NAME);
        let o = new(&mut s, &dir, None, true, &mut recent).unwrap();
        assert_eq!(o.name, format!("{} 2", lightcraft_engine::library::DEFAULT_NAME));
        assert!(s.library.as_ref().unwrap().dir.ends_with(format!("{} 2", lightcraft_engine::library::DEFAULT_NAME)));
        let _ = std::fs::remove_dir_all(&dir);
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
    }

    /// Open Library… on a drive or a server: the library's folder, or the folder it is in (a share's top folder),
    /// opens it; a folder with no library, or with several, says so instead of becoming a library.
    #[test]
    fn find_opens_a_library_or_the_one_in_a_folder_and_never_makes_one() {
        let dir = temp("find");
        Session::new().with_fs().open_library(dir.join("share").join("LightCraft Library"), false).unwrap();
        std::fs::create_dir_all(dir.join("share").join("2019")).unwrap();
        let lib = dir.join("share").join("LightCraft Library");
        assert_eq!(find(&lib), Ok(lib.clone()));
        assert_eq!(find(&dir.join("share")), Ok(lib.clone()));
        let e = find(&dir.join("share").join("2019")).unwrap_err();
        assert!(e.contains("no library here"), "{e}");
        assert!(!lightcraft_engine::library::is_library(&dir.join("share").join("2019")));
        Session::new().with_fs().open_library(dir.join("share").join("Archive"), false).unwrap();
        let e = find(&dir.join("share")).unwrap_err();
        assert!(e.contains("2 libraries here (Archive, LightCraft Library)"), "{e}");
        assert!(find(&dir.join("gone")).unwrap_err().contains("isn't connected"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_tells_a_disk_that_isnt_there_from_a_new_folder() {
        let dir = temp("missing");
        assert_eq!(missing(&dir, true), None);
        assert_eq!(missing(&dir.join("new"), false), None);
        assert!(missing(&dir.join("new"), true).is_some());
        assert!(missing(&dir.join("gone").join("new"), false).is_some());
        assert_eq!(missing(Path::new("relative-new-library"), false), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
