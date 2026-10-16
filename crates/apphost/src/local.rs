//! Local: the folders on this computer the sidebar offers to browse without adding them to the
//! library (Lightroom's Local tab), shared by the egui app and the SwiftUI Mac app. No drawing
//! here: which places are listed and in what order, the kept roots and hidden places (the app
//! keeps both with its own UI state and passes them in), and a folder's subfolders.

use serde::Serialize;

/// The built-in places, by name and folder inside the home folder (`""`: the home folder).
pub const BUILTIN: [(&str, &str); 4] = [("Pictures", "Pictures"), ("Desktop", "Desktop"), ("Downloads", "Downloads"), ("Home", "")];

/// Whether two paths name the same folder, however they are spelled (separators, trailing
/// slash, `.`/`..`, drive-letter case; see `lightcraft_catalog::query::folder_key`).
pub fn same_folder(a: &str, b: &str) -> bool {
    a == b || lightcraft_catalog::query::folder_key(a) == lightcraft_catalog::query::folder_key(b)
}

/// `sub` inside `home`, joined with the platform's separator like the paths browsing gives back.
pub fn builtin_path(home: &str, sub: &str) -> String {
    if sub.is_empty() { home.to_string() } else { std::path::Path::new(home).join(sub).to_string_lossy().to_string() }
}

/// The built-in places that are folders here, as `(name, path)`: [`BUILTIN`] in the home folder
/// (`name` translated by `tr`), then on a Mac the disks mounted in `/Volumes` (not the startup
/// disk's link to `/`). Touches the disk: call it off a UI thread.
pub fn builtin_places(home: &str, tr: impl Fn(&str) -> String) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if !home.is_empty() {
        for (name, sub) in BUILTIN {
            let p = builtin_path(home, sub);
            if std::path::Path::new(&p).is_dir() {
                out.push((tr(name), p));
            }
        }
    }
    if cfg!(target_os = "macos") {
        let mut disks: Vec<(String, String)> = std::fs::read_dir("/Volumes")
            .map(|rd| {
                rd.flatten()
                    .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                    .filter_map(|e| {
                        let name = e.file_name().to_string_lossy().to_string();
                        (!name.starts_with('.') && !time_machine(&name)).then(|| (name, e.path().to_string_lossy().to_string()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        disks.sort_by_key(|(n, _)| n.to_lowercase());
        out.extend(disks);
    }
    out
}

/// A disk Time Machine mounts for itself (its backups, its local snapshots): not a place photos are browsed in.
fn time_machine(volume: &str) -> bool {
    volume.starts_with("com.apple.TimeMachine") || volume.starts_with("Backups of ")
}

/// Local's top-level folders and how the folder being browsed sits among them.
#[derive(Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalPlaces {
    /// (label, path) of each top-level folder, in order.
    pub places: Vec<(String, String)>,
    /// The top-level folder the browsed folder lies in (the innermost one): its tree opens on
    /// the way down to it. None when that way passes through a hidden folder.
    pub owner: Option<usize>,
    /// A folder listed only for this session because the browsed folder is in no saved
    /// location (browsed from a breadcrumb, the CLI…); it stays while browsing below it.
    pub browse_root: Option<String>,
}

fn folder_label(path: &str) -> String {
    std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string())
}

/// Local's top-level folders, in order: the built-in places, the folders kept with Browse
/// Folder… / Keep in Local (`saved`), then (only when the browsed folder lies in none of those)
/// a session root for it: the previous one (`browse_root`) while browsing stays below it, else
/// the browsed folder itself. A folder inside a listed one is shown inside that one's tree (the
/// root stays; its siblings stay reachable), never as a root of its own. Each folder is listed
/// once however its path is spelled (the first spelling wins), and hidden ones are left out:
/// they stay reachable through breadcrumbs and Browse Folder….
pub fn local_places(
    builtin: Vec<(String, String)>,
    saved: &[String],
    browsing: Option<&str>,
    browse_root: Option<&str>,
    hidden: &[String],
) -> LocalPlaces {
    use lightcraft_catalog::query::{folder_key, folder_within};
    let is_hidden = |p: &str| hidden.iter().any(|h| same_folder(h, p));
    let mut places = builtin;
    for path in saved {
        if !places.iter().any(|(_, p)| same_folder(p, path)) {
            places.push((folder_label(path), path.clone()));
        }
    }
    places.retain(|(_, p)| !is_hidden(p));
    let mut out = LocalPlaces::default();
    if let Some(c) = browsing
        && !places.iter().any(|(_, p)| folder_within(c, p))
    {
        let root = browse_root.filter(|r| folder_within(c, r)).unwrap_or(c);
        if !is_hidden(root) {
            places.push((folder_label(root), root.to_string()));
            out.browse_root = Some(root.to_string());
        }
    }
    if let Some(c) = browsing {
        // A tree never opens on the way down through a hidden folder: hiding a kept folder
        // beneath Home would otherwise reveal it again inside Home's (possibly huge) tree.
        let through_hidden = |p: &str| hidden.iter().any(|h| folder_within(c, h) && folder_within(h, p));
        out.owner = places
            .iter()
            .enumerate()
            .filter(|(_, (_, p))| folder_within(c, p) && !through_hidden(p))
            .max_by_key(|(_, (_, p))| folder_key(p).len())
            .map(|(i, _)| i);
    }
    out.places = places;
    out
}

/// The subfolders of `path` (not hidden ones), sorted, as `(name, path)`. Touches the disk (a
/// network share can take long): call it off a UI thread.
pub fn list_subfolders(path: &str) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = std::fs::read_dir(path)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    (!name.starts_with('.')).then(|| (name, e.path().to_string_lossy().to_string()))
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort_by_key(|(n, _)| n.to_lowercase());
    v
}

/// Keep `path` in Local (Browse Folder…, Keep in Local): added to `roots` once however it is
/// spelled, and shown again if it was hidden. Nothing on disk changes. Returns the path as kept.
pub fn add_root(roots: &mut Vec<String>, hidden: &mut Vec<String>, path: &str) -> Result<String, String> {
    let abs = std::path::absolute(path).map(|a| a.to_string_lossy().trim_end_matches(['/', '\\']).to_string()).unwrap_or(path.into());
    let abs = if abs.is_empty() { path.to_string() } else { abs };
    if !std::path::Path::new(&abs).is_dir() {
        return Err(format!("{path}: not a folder"));
    }
    if !roots.iter().any(|r| same_folder(r, &abs)) {
        roots.push(abs.clone());
    }
    hidden.retain(|h| !same_folder(h, &abs));
    Ok(abs)
}

/// Remove `path` from Local (its shortcut only: nothing on disk or in the catalog changes).
pub fn hide(hidden: &mut Vec<String>, path: &str) {
    if !hidden.iter().any(|h| same_folder(h, path)) {
        hidden.push(path.to_string());
    }
}

/// Show `path` again, or (no path) every hidden place.
pub fn restore_hidden(hidden: &mut Vec<String>, path: Option<&str>) {
    match path {
        Some(path) => hidden.retain(|h| !same_folder(h, path)),
        None => hidden.clear(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: &[(String, String)]) -> Vec<&str> {
        v.iter().map(|(n, _)| n.as_str()).collect()
    }

    /// A built-in location browsed (or picked) under another spelling of its path is the same
    /// row, not a second root, for every built-in, not just named ones.
    #[test]
    fn equivalent_paths_are_one_local_location() {
        for (home, sep) in [("D:\\Example", '\\'), ("D:/Example", '/'), ("/home/example", '/')] {
            let builtin: Vec<(String, String)> =
                ["Pictures", "Desktop", "Downloads"].iter().map(|s| (s.to_string(), format!("{home}{sep}{s}"))).collect();
            for (_, p) in builtin.clone() {
                // the browsed spelling: other separators, a trailing one, a different drive-letter case
                let flipped = p.replace(['/', '\\'], if sep == '/' { "\\" } else { "/" });
                for browsing in [flipped.clone(), format!("{p}{sep}"), p.replacen("D:", "d:", 1)] {
                    let l = local_places(builtin.clone(), std::slice::from_ref(&browsing), Some(&browsing), None, &[]);
                    assert_eq!(names(&l.places), ["Pictures", "Desktop", "Downloads"], "{p} browsed as {browsing}");
                    assert_eq!(l.places[l.owner.unwrap()].1, p, "the built-in row is the one highlighted");
                }
                // hiding under one spelling hides the other
                let l = local_places(builtin.clone(), &[], None, None, std::slice::from_ref(&flipped));
                assert_eq!(l.places.len(), 2, "hidden {flipped}");
            }
        }
        // a different folder is still added
        let l = local_places(vec![("Pictures".into(), "/home/example/Pictures".into())], &[], Some("/home/example/Pictures2"), None, &[]);
        assert_eq!(names(&l.places), ["Pictures", "Pictures2"]);
    }

    /// A folder inside a kept root is shown inside that root's tree: the root stays (its other
    /// subfolders stay reachable) and no second root appears; other kept roots stay too.
    #[test]
    fn browsing_below_a_kept_root_keeps_the_root() {
        let photos = "/data/Photos".to_string();
        let other = "/data/Scans".to_string();
        let saved = [photos.clone(), other.clone()];
        let builtin = || vec![("Home".to_string(), "/home/example".to_string())];
        for browsing in ["/data/Photos/2026/20260101", "/data/Photos/2026", "/data/Photos", "D:\\x"] {
            let l = local_places(builtin(), &saved, Some(browsing), None, &[]);
            let roots: Vec<&str> = l.places.iter().map(|(_, p)| p.as_str()).collect();
            if browsing.starts_with("/data") {
                assert_eq!(roots, ["/home/example", "/data/Photos", "/data/Scans"], "{browsing}");
                assert_eq!(l.owner, Some(1), "the Photos tree opens down to {browsing}");
                assert_eq!(l.browse_root, None);
            } else {
                assert_eq!(roots, ["/home/example", "/data/Photos", "/data/Scans", "D:\\x"], "a folder outside them gets a row");
            }
        }
        // the innermost containing root is the one that opens
        let l = local_places(builtin(), &["/home/example/Pictures".into()], Some("/home/example/Pictures/Trip"), None, &[]);
        assert_eq!(l.owner, Some(1));
    }

    /// A browsed folder outside every kept root gets a session row, which stays while browsing
    /// below it and gives way when browsing moves elsewhere.
    #[test]
    fn session_root_stays_while_browsing_below_it() {
        let l = local_places(Vec::new(), &[], Some("/t/base"), None, &[]);
        assert_eq!(l.browse_root.as_deref(), Some("/t/base"));
        let l = local_places(Vec::new(), &[], Some("/t/base/Trip/Day 1"), l.browse_root.as_deref(), &[]);
        assert_eq!((l.places.len(), l.browse_root.as_deref(), l.owner), (1, Some("/t/base"), Some(0)), "{l:?}");
        let l = local_places(Vec::new(), &[], Some("/elsewhere"), l.browse_root.as_deref(), &[]);
        assert_eq!(l.browse_root.as_deref(), Some("/elsewhere"));
        // hidden: no row at all
        let l = local_places(Vec::new(), &[], Some("/t/base"), None, &["/t/base/".into()]);
        assert!(l.places.is_empty() && l.browse_root.is_none());
    }

    /// A hidden folder inside a listed one (a kept folder beneath Home) is not revealed in that
    /// one's tree while it is browsed, nor are the folders below it; hiding Home itself still
    /// lets Pictures open down to a folder browsed inside it.
    #[test]
    fn hidden_folder_is_not_revealed_in_an_outer_tree() {
        let builtin = || vec![("Pictures".to_string(), "/home/example/Pictures".to_string()), ("Home".to_string(), "/home/example".to_string())];
        let kept = ["/home/example/AppData/Temp/lc".to_string()];
        let l = local_places(builtin(), &kept, Some("/home/example/AppData/Temp/lc"), None, &[]);
        assert_eq!(l.owner, Some(2), "shown as its own kept row");
        for browsing in ["/home/example/AppData/Temp/lc", "/home/example/AppData/Temp/lc/Day 1"] {
            let l = local_places(builtin(), &kept, Some(browsing), None, &["/home/example/AppData/Temp/lc/".into()]);
            assert_eq!(names(&l.places), ["Pictures", "Home"], "{browsing}");
            assert_eq!((l.owner, l.browse_root.as_deref()), (None, None), "Home does not open down to {browsing}");
        }
        let l = local_places(builtin(), &[], Some("/home/example/Pictures/Trip"), None, &["/home/example".into()]);
        assert_eq!((names(&l.places), l.owner), (vec!["Pictures"], Some(0)));
    }

    #[test]
    fn time_machine_disks_are_not_places() {
        assert!(time_machine("Backups of MacBook Pro") && time_machine("com.apple.TimeMachine.localsnapshots"));
        assert!(!time_machine("Photos 2026") && !time_machine("Macintosh HD"));
    }

    #[test]
    fn roots_and_hidden_places_are_kept_once_however_spelled() {
        let dir = std::env::temp_dir().join(format!("lc-apphost-local-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.to_string_lossy().to_string();
        let (mut roots, mut hidden) = (Vec::new(), vec![format!("{p}/")]);
        assert_eq!(add_root(&mut roots, &mut hidden, &format!("{p}/")).unwrap(), p);
        add_root(&mut roots, &mut hidden, &p).unwrap();
        assert_eq!((roots.len(), hidden.len()), (1, 0), "kept once and shown again");
        assert!(add_root(&mut roots, &mut hidden, &dir.join("nothing").to_string_lossy()).is_err());
        hide(&mut hidden, &p);
        hide(&mut hidden, &format!("{p}/"));
        assert_eq!(hidden.len(), 1);
        restore_hidden(&mut hidden, Some(&format!("{p}/")));
        assert!(hidden.is_empty());
        std::fs::create_dir_all(dir.join("b")).unwrap();
        std::fs::create_dir_all(dir.join("A")).unwrap();
        std::fs::create_dir_all(dir.join(".hidden")).unwrap();
        assert_eq!(names(&list_subfolders(&p)), ["A", "b"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
