//! What a library takes on this computer's disk, and what is left there: the numbers behind
//! Settings ▸ Sync's storage section and the `sync.usage` command. (What the *server* takes is
//! [`lightcraft_catalog::sync::proto::Usage`], asked of it by [`crate::sync`].)
//!
//! Walking a library's folders reads every file's size, so a host with a UI runs [`measure`] on a
//! worker thread ([`Session::local_dirs`] gives it the folders to look at).

use std::path::{Path, PathBuf};

use lightcraft_catalog::sync::proto::{Disk, Files};
use serde::Serialize;

use crate::Session;
use crate::sync::is_remote;

/// Deepest folder level counted (copied originals may sit in dated sub-folders). A folder tree
/// is never followed through links, so this only bounds an unusually deep one.
const DEPTH: u32 = 12;

/// Where a library's files are on disk.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LocalDirs {
    /// The library folder: its top-level files are the catalog, edits, presets and settings.
    pub library: PathBuf,
    /// Rendered thumbnails (safe to delete).
    pub thumbnails: PathBuf,
    /// Smart previews (`.lcsp`) and small previews (`.lcsm`), downloaded or built here.
    pub previews: PathBuf,
    /// Originals downloaded from the sync server.
    pub downloaded: PathBuf,
    /// Photos imported with "copy into library".
    pub imported: PathBuf,
}

/// What a library takes on this computer.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct LocalUsage {
    /// Catalog, edits, presets and settings.
    pub library: u64,
    pub thumbnails: Files,
    pub smart: Files,
    pub mini: Files,
    pub downloaded: Files,
    pub imported: Files,
    /// The disk the library is on (`None`: this platform can't say).
    pub disk: Option<Disk>,
}

impl LocalUsage {
    /// Bytes the library takes in all.
    pub fn total(&self) -> u64 {
        [self.thumbnails, self.smart, self.mini, self.downloaded, self.imported].iter().fold(self.library, |t, f| t.saturating_add(f.bytes))
    }
}

/// The files under `dir` that `keep` accepts (links are not followed; unreadable folders count
/// as empty).
fn walk(dir: &Path, depth: u32, keep: &dyn Fn(&Path) -> bool, out: &mut Files) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let Ok(kind) = e.file_type() else { continue };
        if kind.is_dir() {
            if depth > 0 {
                walk(&e.path(), depth - 1, keep, out);
            }
        } else if kind.is_file() {
            let path = e.path();
            if keep(&path) {
                out.files += 1;
                out.bytes = out.bytes.saturating_add(e.metadata().map_or(0, |m| m.len()));
            }
        }
    }
}

fn everything(_: &Path) -> bool {
    true
}

fn with_extension(ext: &'static str) -> impl Fn(&Path) -> bool {
    move |p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext))
}

/// Measure a library's folders.
pub fn measure(dirs: &LocalDirs) -> LocalUsage {
    let sized = |dir: &Path, keep: &dyn Fn(&Path) -> bool| {
        let mut f = Files::default();
        walk(dir, DEPTH, keep, &mut f);
        f
    };
    // the top level only: the folders below it are counted on their own
    let mut top = Files::default();
    walk(&dirs.library, 0, &everything, &mut top);
    LocalUsage {
        library: top.bytes,
        thumbnails: sized(&dirs.thumbnails, &everything),
        smart: sized(&dirs.previews, &with_extension("lcsp")),
        mini: sized(&dirs.previews, &with_extension("lcsm")),
        downloaded: sized(&dirs.downloaded, &everything),
        imported: sized(&dirs.imported, &everything),
        disk: disk_space(&dirs.library),
    }
}

/// Bytes on the file system holding `dir`, from `df` (Unix; `None` elsewhere, or when it can't
/// say).
pub fn disk_space(dir: &Path) -> Option<Disk> {
    #[cfg(all(unix, not(target_os = "ios")))]
    {
        // (an absolute path: `df` would take a name starting with `-` for an option)
        let dir = std::fs::canonicalize(dir).ok()?;
        let out = std::process::Command::new("df").arg("-Pk").arg(dir).output().ok().filter(|o| o.status.success())?;
        parse_df(&String::from_utf8_lossy(&out.stdout))
    }
    #[cfg(not(all(unix, not(target_os = "ios"))))]
    {
        let _ = dir;
        None
    }
}

/// `df -Pk`'s one line for the file system: `name 1024-blocks used available capacity mount`.
/// The name and the mount point may hold spaces, so the numbers are found by the `%` that ends
/// the capacity column.
fn parse_df(text: &str) -> Option<Disk> {
    let cols: Vec<&str> = text.lines().nth(1)?.split_whitespace().collect();
    let k = |s: &str| s.parse::<u64>().ok().map(|n| n.saturating_mul(1024));
    (3..cols.len()).find_map(|i| {
        let (total, used, free) = (k(cols.get(i - 3)?)?, k(cols.get(i - 2)?)?, k(cols.get(i - 1)?)?);
        (cols.get(i)?.ends_with('%') && used <= total).then_some(Disk { total, free })
    })
}

impl Session {
    /// Where this library's files are (`None`: not on a disk: the browser's storage, a library
    /// that isn't saved).
    pub fn local_dirs(&self) -> Option<LocalDirs> {
        let lib = self.library.as_ref().filter(|l| l.on_disk)?;
        Some(LocalDirs {
            library: lib.dir.clone(),
            thumbnails: lib.thumbs_dir(),
            previews: self.media.smart_dir.clone().unwrap_or_else(|| crate::smart::dir(&lib.dir)),
            downloaded: self.originals_dir()?,
            imported: lib.originals_dir(),
        })
    }

    /// (photos in the library, those whose file isn't on this device: only their previews are).
    /// Virtual copies and photos that stay on this device (`Local`) aren't counted.
    pub fn photo_counts(&self) -> (usize, usize) {
        let mut counts = (0, 0);
        for p in self.catalog.photos().filter(|p| !p.local && p.copy_of.is_none()) {
            counts.0 += 1;
            counts.1 += usize::from(is_remote(p));
        }
        counts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh folder of this process's own.
    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lc-usage-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write(path: &Path, bytes: usize) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, vec![0u8; bytes]).unwrap();
    }

    #[test]
    fn measures_each_folder_and_not_twice() {
        let tmp = temp_dir("measure");
        let lib = tmp.join("Library");
        let dirs = LocalDirs {
            library: lib.clone(),
            thumbnails: lib.join("thumbs"),
            previews: lib.join("Smart Previews"),
            downloaded: lib.join("sync").join("originals"),
            imported: lib.join("Originals"),
        };
        write(&lib.join("catalog.log"), 100);
        write(&lib.join("prefs.json"), 20);
        write(&dirs.thumbnails.join("a.jpg"), 7);
        write(&dirs.previews.join("a.lcsp"), 1000);
        write(&dirs.previews.join("b.lcsp"), 2000);
        write(&dirs.previews.join("a.lcsm"), 30);
        write(&dirs.previews.join("notes.txt"), 99_999);
        write(&dirs.downloaded.join("k1").join("IMG_1.CR3"), 5000);
        write(&dirs.imported.join("2024").join("05").join("x.dng"), 400);
        let u = measure(&dirs);
        assert_eq!(u.library, 120, "top-level files only");
        assert_eq!(u.thumbnails, Files { files: 1, bytes: 7 });
        assert_eq!(u.smart, Files { files: 2, bytes: 3000 });
        assert_eq!(u.mini, Files { files: 1, bytes: 30 });
        assert_eq!(u.downloaded, Files { files: 1, bytes: 5000 });
        assert_eq!(u.imported, Files { files: 1, bytes: 400 });
        assert_eq!(u.total(), 120 + 7 + 3000 + 30 + 5000 + 400);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn missing_folders_are_empty() {
        let tmp = temp_dir("missing");
        let gone = tmp.join("nothing");
        let dirs = LocalDirs {
            library: gone.clone(),
            thumbnails: gone.join("t"),
            previews: gone.join("p"),
            downloaded: gone.join("d"),
            imported: gone.join("i"),
        };
        let u = measure(&dirs);
        assert_eq!(u.total(), 0);
        assert_eq!(u.disk, None, "no such disk");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn df_lines_with_spaces_in_names() {
        let plain = "Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/disk3s1 971350180 894213448 31457280 97% /System/Volumes/Data\n";
        assert_eq!(parse_df(plain), Some(Disk { total: 971_350_180 * 1024, free: 31_457_280 * 1024 }));
        let spaced = "Filesystem 1024-blocks Used Available Capacity Mounted on\n//elias@nas/My Photos 1000 400 600 40% /Volumes/My Photos 2\n";
        assert_eq!(parse_df(spaced), Some(Disk { total: 1000 * 1024, free: 600 * 1024 }));
        assert_eq!(parse_df("garbage"), None);
        assert_eq!(parse_df("h\nonly three cols"), None);
    }

    #[cfg(all(unix, not(target_os = "ios")))]
    #[test]
    fn the_disk_of_a_real_folder_is_known() {
        let tmp = temp_dir("disk");
        let d = disk_space(&tmp).expect("df answers for the temp folder");
        assert!(d.total > 0 && d.free <= d.total);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
