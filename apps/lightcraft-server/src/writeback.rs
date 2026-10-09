//! Library folders the server may write in (an admin's choice, per folder; the default is read-only).
//!
//! Two things, and only these:
//! - **XMP sidecars.** When a device changes a photo that lives in a writable library folder (its rating, flag,
//!   label, edits or metadata), the server writes the photo's `.xmp` beside it a few seconds later — the same
//!   file, merged the same way, as the desktop app's *Automatically Write Changes into XMP*, so Lightroom and
//!   other programs see the edits. The photo itself is never opened for writing. A sidecar that another program
//!   changed since the server last read it is not overwritten: it is read first (a scan is asked for) and the
//!   edit waits for the next change.
//! - **Filing uploads.** A photo a device uploads is kept in the server's own store (`blobs/original`); when
//!   the user has an *imports* folder, the server instead files it there as `<year>/<date>/<name>` (by the
//!   photo's capture date), makes that file the photo's place on the server, and drops its own copy. It is
//!   copied under a temporary name and linked into place, never over a file that is there; a crash between the
//!   steps leaves a file the next scan recognises by its content.
//!
//! One worker thread does both, off the request threads.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use lightcraft_engine::catalog::{Catalog, MediaKind, Op, Photo, PhotoId, Source};
use lightcraft_engine::sidecar::{self, SidecarNaming};

use crate::State;
use crate::accounts::{self, LibraryFolder};
use crate::api::{UserLib, blob_path, lib};
use crate::folders::{self, Entry};

/// A photo's changes are written together, this long after the last.
const SETTLE: Duration = Duration::from_secs(3);
/// Filing is looked at again this often even if nothing was uploaded (a photo's record may arrive after its file).
const PERIODIC: Duration = Duration::from_secs(300);
/// Photos filed per pass at most (the rest in the next).
const FILE_MAX: usize = 500;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Default)]
struct Pending {
    /// Photos whose sidecar is to be written, with when.
    photos: HashMap<(String, u64), Instant>,
    /// Users whose uploads are to be filed, with when.
    filing: HashMap<String, Instant>,
}

/// What the worker has been asked to do.
pub struct Writer {
    pending: Mutex<Pending>,
    wake: Condvar,
    stop: AtomicBool,
}

impl Default for Writer {
    fn default() -> Self {
        Writer::new()
    }
}

impl Writer {
    pub fn new() -> Writer {
        Writer { pending: Mutex::new(Pending::default()), wake: Condvar::new(), stop: AtomicBool::new(false) }
    }

    /// These photos of `user` changed on a device: their sidecars are due.
    pub fn photos_changed(&self, user: &str, ids: &[PhotoId]) {
        if ids.is_empty() {
            return;
        }
        let due = Instant::now() + SETTLE;
        let mut p = lock(&self.pending);
        for id in ids {
            p.photos.insert((user.to_string(), id.0), due);
        }
        self.wake.notify_all();
    }

    /// `user` has new uploads (or a scan ended): file them soon.
    pub fn file_uploads(&self, user: &str) {
        lock(&self.pending).filing.insert(user.to_string(), Instant::now() + Duration::from_secs(1));
        self.wake.notify_all();
    }

    pub fn shut_down(&self) {
        self.stop.store(true, Ordering::SeqCst);
        self.wake.notify_all();
    }
}

/// The photo ids a device's ops change in ways the XMP sidecar holds.
pub fn sidecar_photos(ops: &[Op]) -> Vec<PhotoId> {
    let mut ids: Vec<PhotoId> = ops
        .iter()
        .filter_map(|op| match op {
            Op::SetRating { id, .. } | Op::SetFlag { id, .. } | Op::SetLabel { id, .. } | Op::SetDevelop { id, .. } | Op::SetMeta { id, .. } => {
                Some(*id)
            }
            _ => None,
        })
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

/// The worker thread.
pub(crate) fn run(st: &Arc<State>) {
    let w = &st.writer;
    let mut periodic_at = Instant::now();
    while !w.stop.load(Ordering::SeqCst) {
        let (photos, filing) = {
            let mut g = lock(&w.pending);
            let now = Instant::now();
            let photos: Vec<(String, u64)> = g.photos.iter().filter(|(_, due)| **due <= now).map(|(k, _)| k.clone()).collect();
            let filing: Vec<String> = g.filing.iter().filter(|(_, due)| **due <= now).map(|(k, _)| k.clone()).collect();
            for k in &photos {
                g.photos.remove(k);
            }
            for k in &filing {
                g.filing.remove(k);
            }
            if photos.is_empty() && filing.is_empty() {
                let next = g.photos.values().chain(g.filing.values()).min().map_or(Duration::from_secs(5), |d| d.saturating_duration_since(now));
                let _ = w.wake.wait_timeout(g, next.clamp(Duration::from_millis(20), Duration::from_secs(5)));
                if periodic_at.elapsed() >= PERIODIC {
                    periodic_at = Instant::now();
                    for u in users_with_imports(&st.data) {
                        w.file_uploads(&u);
                    }
                }
                continue;
            }
            (photos, filing)
        };
        let mut by_user: HashMap<String, Vec<PhotoId>> = HashMap::new();
        for (user, id) in photos {
            by_user.entry(user).or_default().push(PhotoId(id));
        }
        for (user, ids) in by_user {
            if let Err(e) = lightcraft_engine::guard::catch("writing sidecars", || write_sidecars(st, &user, &ids)) {
                log::error!("{user}: {e}");
            }
        }
        for user in filing {
            if let Err(e) = lightcraft_engine::guard::catch("filing uploads", || file_uploads(st, &user)) {
                log::error!("{user}: {e}");
            }
        }
    }
}

fn users_with_imports(data: &Path) -> Vec<String> {
    accounts::read_users(data)
        .map(|f| f.users.into_iter().filter(|(_, u)| u.folders.iter().any(|f| f.imports)).map(|(n, _)| n).collect())
        .unwrap_or_default()
}

/// A user's folders the server may write in, by lower-case name.
fn writable_folders(st: &State, user: &str) -> HashMap<String, LibraryFolder> {
    accounts::read_users(&st.data)
        .ok()
        .and_then(|f| f.users.get(user).map(|u| u.folders.iter().filter(|f| f.writable).map(|f| (f.name.to_lowercase(), f.clone())).collect()))
        .unwrap_or_default()
}

fn now_iso() -> String {
    lightcraft_engine::catalog::dates::civil(i64::try_from(accounts::now()).unwrap_or(i64::MAX))
}

/// Which sidecar name each photo in the library folders uses: a raw and a JPEG of the same name share the stem,
/// so the raw (else the first by name) has `IMG_1.xmp` and the others `IMG_1.JPG.xmp`.
fn naming_of(c: &Catalog) -> HashMap<PhotoId, SidecarNaming> {
    let stem = |sp: &str| Path::new(sp).with_extension("").to_string_lossy().to_lowercase();
    let mut groups: HashMap<String, Vec<(bool, String, PhotoId)>> = HashMap::new();
    for p in c.photos().filter(|p| p.copy_of.is_none()) {
        if let Some(sp) = &p.server_path {
            groups.entry(stem(sp)).or_default().push((p.kind != MediaKind::Raw, sp.to_lowercase(), p.id));
        }
    }
    let mut out = HashMap::new();
    for (_, mut g) in groups {
        g.sort();
        for (i, (_, _, id)) in g.iter().enumerate() {
            out.insert(*id, if i == 0 { SidecarNaming::Stem } else { SidecarNaming::Full });
        }
    }
    out
}

/// Write the sidecars of `ids` (photos of `user` that changed), where the folder allows it.
fn write_sidecars(st: &State, user: &str, ids: &[PhotoId]) {
    let folders = writable_folders(st, user);
    if folders.is_empty() {
        return;
    }
    let Ok(l) = lib(st, user) else { return };
    let stamp = now_iso();
    let mut l = lock(&l);
    let UserLib { core, index, dir, .. } = &mut *l;
    let catalog = core.catalog();
    let naming = naming_of(catalog);
    let (mut changed, mut read_first) = (false, false);
    for id in ids {
        let Some(p) = catalog.photo(*id).filter(|p| p.copy_of.is_none()) else { continue };
        let Some(key) = p.server_path.as_deref() else { continue };
        let Some(root) = key.split_once('/').map(|(r, _)| r.to_lowercase()) else { continue };
        if !folders.contains_key(&root) {
            continue;
        }
        let Some(path) = index.path_of(key) else { continue };
        let original = path.to_string_lossy().to_string();
        let preferred = naming.get(id).copied().unwrap_or_default();
        // an existing sidecar is the one to write (whichever of the two names it has)
        let existing = sidecar::find_sidecar(&original, preferred);
        let chosen = match &existing {
            Some(e) if *e == sidecar::sidecar_path(&original, SidecarNaming::Full) => SidecarNaming::Full,
            Some(_) => SidecarNaming::Stem,
            None => preferred,
        };
        // another program edited it since the server last read it: read it before writing
        let known = index.files.get(key).and_then(|e| e.sidecar);
        let on_disk = existing.as_deref().and_then(sidecar::modified);
        if on_disk.is_some() && on_disk != known {
            read_first = true;
            continue;
        }
        match sidecar::write_sidecar_for(p, catalog, &original, chosen, &stamp) {
            Ok(saved) => {
                if let Some(e) = index.files.get_mut(key) {
                    e.sidecar = sidecar::modified(&saved.path);
                    changed = true;
                }
            }
            Err(e) => log::warn!("{user}: XMP for {key}: {e}"),
        }
    }
    if changed && let Err(e) = index.save(dir) {
        log::warn!("{user}: {e}");
    }
    drop(l);
    if read_first {
        st.folders.request(user);
    }
}

/// A file name that is safe on any disk: no separators, nothing hidden, a sensible length.
pub fn safe_file_name(name: &str) -> String {
    let last = name.rsplit(['/', '\\']).next().unwrap_or("");
    let mut n: String = last.chars().map(|c| if c.is_control() || "/\\:*?\"<>|".contains(c) { '_' } else { c }).take(150).collect();
    n = n.trim().to_string();
    if n.is_empty() || n.starts_with('.') { format!("photo{n}") } else { n }
}

/// `YYYY-MM-DD` of a photo, from its capture time (else when it was imported); `None` if neither reads as a date.
fn date_of(p: &Photo) -> Option<String> {
    let ok = |s: &str| {
        let d = s.get(..10)?;
        let b = d.as_bytes();
        let digits = b.iter().enumerate().all(|(i, c)| if i == 4 || i == 7 { *c == b'-' } else { c.is_ascii_digit() });
        (b.len() == 10 && digits && d.get(..4)? != "0000").then(|| d.to_string())
    };
    p.captured.as_deref().and_then(ok).or_else(|| ok(&p.imported))
}

struct Filing {
    id: PhotoId,
    hash: String,
    name: String,
    date: String,
}

/// Put the file at `src` into `<folder>/<year>/<date>/` under a name no file there has: copied to a temporary name,
/// synced, then linked (or, where the disk can't link, renamed) into place. Returns the path inside the folder
/// (`2026/2026-04-12/IMG_1.CR3`), the file's size and its modification time.
fn file_into(folder: &Path, src: &Path, name: &str, date: &str) -> Result<(String, u64, u64), String> {
    let year = date.get(..4).ok_or("no date")?;
    let dir = folder.join(year).join(date);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let temp = dir.join(format!(".lightcraft-filing-{}", accounts::random_hex(4)?));
    let copied = (|| -> std::io::Result<u64> {
        let n = std::fs::copy(src, &temp)?;
        std::fs::File::open(&temp)?.sync_all()?;
        Ok(n)
    })();
    let size = copied.map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        format!("{}: {e}", temp.display())
    })?;
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.to_string(), String::new()),
    };
    for n in 0..1000 {
        let candidate = if n == 0 { name.to_string() } else { format!("{stem}-{n}{ext}") };
        let target = dir.join(&candidate);
        let placed = match std::fs::hard_link(&temp, &target) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            // a disk that can't link: rename, unless something is there (a race with another writer is not expected)
            Err(_) if !target.exists() => std::fs::rename(&temp, &target),
            Err(_) => continue,
        };
        let _ = std::fs::remove_file(&temp);
        return match placed {
            Ok(()) => {
                let mtime = std::fs::metadata(&target).map(|m| folders::mtime_of(&m)).unwrap_or(0);
                Ok((format!("{year}/{date}/{candidate}"), size, mtime))
            }
            Err(e) => Err(format!("{}: {e}", target.display())),
        };
    }
    let _ = std::fs::remove_file(&temp);
    Err(format!("no free file name for {name} in {}", dir.display()))
}

/// File `user`'s uploaded originals into their imports folder.
fn file_uploads(st: &State, user: &str) {
    let Some(dest) =
        accounts::read_users(&st.data).ok().and_then(|f| f.users.get(user).and_then(|u| u.folders.iter().find(|f| f.imports && f.writable).cloned()))
    else {
        return;
    };
    // (the scan decides which files are missing: don't add files under it)
    if st.folders.status(user).scanning {
        st.writer.file_uploads(user);
        return;
    }
    let Ok(l) = lib(st, user) else { return };
    let dir = accounts::user_dir(&st.data, user);
    let blobs = dir.join("blobs");
    let todo: Vec<Filing> = {
        let l = lock(&l);
        let mut seen = std::collections::HashSet::new();
        l.core
            .catalog()
            .photos()
            .filter(|p| !p.local && !p.deleted && p.copy_of.is_none() && p.server_path.is_none())
            .filter(|p| matches!(&p.source, Source::File { path } if path.starts_with(lightcraft_engine::catalog::sync::PATH_PREFIX)))
            .filter_map(|p| {
                let hash = lightcraft_engine::catalog::sync::content_key(p)?;
                // a copy of this content is in a library folder already (the scan makes it the photo's place)
                let upload = blob_path(&blobs, "original", &hash).filter(|b| b.is_file())?;
                (!l.index.has(&hash) && upload.is_file() && seen.insert(hash.clone())).then(|| Filing {
                    id: p.id,
                    hash,
                    name: safe_file_name(&p.file_name),
                    date: date_of(p).unwrap_or_else(|| now_iso().chars().take(10).collect()),
                })
            })
            .take(FILE_MAX)
            .collect()
    };
    let mut filed = 0;
    for f in todo {
        let Some(src) = blob_path(&blobs, "original", &f.hash).filter(|b| b.is_file()) else { continue };
        let (rel, size, mtime) = match file_into(Path::new(&dest.path), &src, &f.name, &f.date) {
            Ok(r) => r,
            Err(e) => {
                log::warn!("{user}: filing {}: {e}", f.name);
                continue;
            }
        };
        let key = format!("{}/{rel}", dest.name);
        let mut l = lock(&l);
        l.index.roots.entry(dest.name.clone()).or_insert_with(|| dest.path.clone());
        l.index.files.insert(key.clone(), Entry { size, mtime, hash: f.hash.clone(), photo: Some(f.id.0), ..Default::default() });
        l.index.reindex();
        let pushed = folders::push(&mut l, &[(Op::SetServerPath { id: f.id, path: Some(key.clone()) }, 0)], &|_| key.clone());
        if let Err(e) = l.index.save(&dir) {
            log::warn!("{user}: {e}");
        }
        // the file is the photo's place now (and its content is served from there): the server's own copy can go
        if pushed.is_some_and(|refused| refused.is_empty()) {
            let _ = std::fs::remove_file(&src);
            filed += 1;
        }
    }
    if filed > 0 {
        log::info!("{user}: filed {filed} uploaded photo(s) into {}", dest.name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_are_safe() {
        assert_eq!(safe_file_name("IMG_001.CR3"), "IMG_001.CR3");
        assert_eq!(safe_file_name("../../etc/passwd"), "passwd");
        assert_eq!(safe_file_name("C:\\x\\y:z?.jpg"), "y_z_.jpg");
        assert_eq!(safe_file_name(".hidden"), "photo.hidden");
        assert_eq!(safe_file_name(""), "photo");
        assert_eq!(safe_file_name("日本語.jpg"), "日本語.jpg");
        assert!(safe_file_name(&"a".repeat(500)).chars().count() <= 150);
    }

    #[test]
    fn files_are_placed_without_overwriting() {
        let d = std::env::temp_dir().join(format!("lc-writeback-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let src = d.join("src.bin");
        std::fs::write(&src, b"one").unwrap();
        let folder = d.join("in");
        std::fs::create_dir_all(&folder).unwrap();
        let (a, size, _) = file_into(&folder, &src, "x.png", "2026-04-12").unwrap();
        assert_eq!((a.as_str(), size), ("2026/2026-04-12/x.png", 3));
        std::fs::write(&src, b"two!").unwrap();
        let (b, ..) = file_into(&folder, &src, "x.png", "2026-04-12").unwrap();
        assert_eq!(b, "2026/2026-04-12/x-1.png", "another file with the name is never replaced");
        assert_eq!(std::fs::read(folder.join(&a)).unwrap(), b"one");
        assert_eq!(std::fs::read(folder.join(&b)).unwrap(), b"two!");
        // nothing temporary is left behind
        let left: Vec<_> =
            std::fs::read_dir(folder.join("2026/2026-04-12")).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
        assert_eq!(left.len(), 2, "{left:?}");
        assert!(file_into(&folder, &src, "x.png", "xx").is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn dates_come_from_the_capture_time_else_the_import() {
        let mut p = Photo::new(PhotoId(1), Source::Demo { scene: 0 }, "a.jpg", "JPEG", 1, 1, "2026-02-03T10:00:00");
        assert_eq!(date_of(&p).as_deref(), Some("2026-02-03"));
        p.captured = Some("2019-07-14T08:30:00".into());
        assert_eq!(date_of(&p).as_deref(), Some("2019-07-14"));
        p.captured = Some("garbage!!!".into());
        assert_eq!(date_of(&p).as_deref(), Some("2026-02-03"));
        p.imported = "later".into();
        assert_eq!(date_of(&p), None);
    }

    #[test]
    fn a_raw_keeps_the_plain_sidecar_name_and_its_jpeg_gets_the_long_one() {
        let mut c = Catalog::new();
        let mut add = |name: &str, kind: MediaKind| {
            let id = c.alloc_photo_id();
            let mut p = Photo::new(id, Source::Demo { scene: 0 }, name, "X", 1, 1, "");
            p.kind = kind;
            p.server_path = Some(format!("Photos/{name}"));
            c.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
            id
        };
        let (jpg, raw, alone) = (add("IMG_1.JPG", MediaKind::Image), add("IMG_1.CR3", MediaKind::Raw), add("IMG_2.JPG", MediaKind::Image));
        let n = naming_of(&c);
        assert_eq!((n[&raw], n[&jpg], n[&alone]), (SidecarNaming::Stem, SidecarNaming::Full, SidecarNaming::Stem));
    }
}
