//! Libraries for a native frontend: the list it shows (`host.libraries`), switching to another one
//! without restarting the engine (`host.library.open`, `host.library.new`), and deleting one that
//! isn't open (`host.library.delete`: a phone's, whose libraries live in the app's own folder; a
//! computer's go to the Trash from the file manager). The rules are [`crate::libraries`]'s; here,
//! what the engine thread lets go of when the library changes, and what it refuses meanwhile.
//!
//! The app keeps the recent list with its own settings and passes it in; every answer gives it back
//! as it should be kept. Paths go both ways as the app keeps them: `~/…` inside its home folder
//! ([`crate::libraries::portable`]), since an iOS app's container moves with each update.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::{Engine, ErrorBody, Event};
use crate::libraries;
use crate::startup::Profile;

/// Background work that doesn't stop a library from being left: what it does is for no library
/// (model downloads, the server's work this device does) or let go of by every open (the face and
/// focus scans, the thumbnail cache's tidying), and signing out goes on when the library opens again.
const SWITCH_OK: [&str; 8] = ["download", "faces", "facesGpu", "helper", "signOut", "focus", "cache", "geocode"];

/// Files counted at most when a library's size is said before it is deleted.
const SIZE_FILES_MAX: usize = 500_000;

impl Engine {
    /// Where the app keeps its libraries: beside its default one (`Documents` on a phone, `Pictures`
    /// on a computer).
    fn libraries_location(&self) -> Option<PathBuf> {
        crate::startup::library_dir_for(self.profile, None, self.home.clone().map(Into::into)).and_then(|d| d.parent().map(Path::to_path_buf))
    }

    fn portable(&self, path: &Path) -> String {
        libraries::portable(path, self.home.as_deref())
    }

    /// The recent list the app passed in (`recent`), each path written the one way the app keeps it.
    fn recent_param(&self, p: &Value) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for s in p.get("recent").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
            let path = self.portable(&libraries::expand(s, self.home.as_deref()));
            if !out.contains(&path) && out.len() < libraries::RECENT {
                out.push(path);
            }
        }
        out
    }

    /// `host.libraries {recent?, forget?, clear?}` → `{recent, location, demo, libraries}`: the libraries the app
    /// offers, the open one first, then the recent ones, then the others in `location` (where new ones go), each
    /// `{path, recent, …}` with [`libraries::describe`]'s facts. The open library joins the front of the recent
    /// list; a recent one that was in `location` and is gone (deleted) leaves it; `forget` takes one out of it,
    /// `clear` all.
    pub(super) fn libraries(&mut self, p: &Value) -> Result<Value, ErrorBody> {
        let home = self.home.clone();
        let mut recent = self.recent_param(p);
        if p.get("clear").and_then(Value::as_bool) == Some(true) {
            recent.clear();
        }
        if let Some(forget) = p.get("forget").and_then(Value::as_str) {
            let forget = libraries::expand(forget, home.as_deref());
            recent.retain(|r| !libraries::same(&libraries::expand(r, home.as_deref()), &forget));
        }
        let location = self.libraries_location();
        if let Some(loc) = &location {
            recent.retain(|r| {
                let d = libraries::expand(r, home.as_deref());
                d.exists() || !d.parent().is_some_and(|parent| libraries::same(parent, loc))
            });
        }
        let open = libraries::open_dir(&self.session).map(Path::to_path_buf);
        if let Some(open) = &open {
            libraries::remember(&mut recent, &self.portable(open));
        }
        let mut dirs: Vec<PathBuf> = open.into_iter().collect();
        let found = location.as_deref().map(libraries::found_in).unwrap_or_default();
        for d in recent.iter().map(|r| libraries::expand(r, home.as_deref())).chain(found) {
            if !dirs.iter().any(|x| libraries::same(x, &d)) {
                dirs.push(d);
            }
        }
        let list: Vec<Value> = dirs
            .iter()
            .map(|d| {
                let path = self.portable(d);
                let mut v = libraries::describe(d, &self.session);
                v["recent"] = json!(recent.contains(&path));
                v["path"] = json!(path);
                v
            })
            .collect();
        Ok(json!({
            "recent": recent,
            "location": location.as_deref().map(|l| self.portable(l)),
            "demo": self.demo,
            "libraries": list,
        }))
    }

    /// Why the library can't be changed now (`None`: it can): work that adds photos to the open library or
    /// changes them is under way, and would land in the next one.
    fn switch_refused(&self) -> Option<ErrorBody> {
        let busy = |what: &str| Some(ErrorBody::new("other", format!("wait for {what} to finish before switching libraries")));
        if self.import.is_some() || self.scan.is_some() {
            return busy("the import");
        }
        if self.browse.is_some() {
            return busy("the folder being read");
        }
        if self.export.is_some() {
            return busy("the export");
        }
        if self.lightroom.running() {
            return busy("the Lightroom catalog import");
        }
        if self.super_res.is_some() || self.merge.busy() || self.session.expand_running().is_some() || self.session.erase_running() > 0 {
            return busy("the photo being made");
        }
        let tasks = self.session.activity.list();
        let running = tasks.iter().find(|t| !SWITCH_OK.contains(&t.kind))?;
        busy(&format!("“{}”", running.label))
    }

    /// `host.library.open {path, recent?}`: leave the open library for the one at `path` (an existing folder: a
    /// library is made there when it holds none). → `{path, name, photos, recent, library}`. Refused, with the open
    /// library left as it was, while work runs on it, when the folder is gone (a library deleted or on a drive
    /// that isn't there: never an empty one in its place), is a Lightroom catalog (`lightroomCatalog`) or is open
    /// in another program (`libraryInUse`).
    pub(super) fn open_library(&mut self, p: &Value) -> Result<Value, ErrorBody> {
        let bad = |m: &str| ErrorBody::new("badParams", format!("`host.library.open`: {m}"));
        let path = p.get("path").and_then(Value::as_str).filter(|s| !s.is_empty()).ok_or_else(|| bad("missing `path`"))?;
        let dir = libraries::expand(path, self.home.as_deref());
        if let Some(e) = self.switch_refused() {
            return Err(e);
        }
        if !dir.exists() {
            return Err(ErrorBody::new(
                "other",
                format!("{}: the library is no longer there (deleted, moved, renamed or on a drive that isn't connected)", dir.display()),
            ));
        }
        self.switch_to(&dir, self.recent_param(p))
    }

    /// `host.library.new {name, location?, recent?}`: make an empty library called `name` in `location` (where the
    /// app keeps its libraries, by default) and switch to it; answered as `host.library.open`. A name taken by a
    /// library or a file there, or one no folder can have, is refused, as anything `host.library.open` refuses.
    pub(super) fn new_library(&mut self, p: &Value) -> Result<Value, ErrorBody> {
        let bad = |m: &str| ErrorBody::new("badParams", format!("`host.library.new`: {m}"));
        let name = p.get("name").and_then(Value::as_str).ok_or_else(|| bad("missing `name`"))?;
        let location = match p.get("location").and_then(Value::as_str).filter(|s| !s.is_empty()) {
            Some(l) => libraries::expand(l, self.home.as_deref()),
            None => self.libraries_location().ok_or_else(|| ErrorBody::new("other", "there is no home folder to keep libraries in"))?,
        };
        if let Some(e) = self.switch_refused() {
            return Err(e);
        }
        let dir = libraries::new_dir(&location, Some(name), false).map_err(|e| ErrorBody::new("badParams", e))?;
        let opened = self.switch_to(&dir, self.recent_param(p));
        if opened.is_err() {
            // (the folder made for it, still empty)
            let _ = std::fs::remove_dir(&dir);
        }
        opened
    }

    /// Switch to the library at `dir` and let go of everything the engine thread held of the one before.
    fn switch_to(&mut self, dir: &Path, mut recent: Vec<String>) -> Result<Value, ErrorBody> {
        let generation = self.session.library_generation();
        let switched = libraries::switch(&mut self.session, dir);
        if self.session.library_generation() != generation {
            self.library_switched();
        }
        let switched = switched.map_err(|e| ErrorBody::engine(&e))?;
        if let Some(prev) = switched.previous.as_deref().map(|d| self.portable(d)).filter(|p| !recent.contains(p)) {
            libraries::remember(&mut recent, &prev);
        }
        let path = self.portable(&switched.dir);
        libraries::remember(&mut recent, &path);
        log::info!("library {}: {} photos (switched)", switched.dir.display(), self.session.catalog.len());
        // (as `host.libraries` lists it)
        let mut library = libraries::describe(&switched.dir, &self.session);
        library["path"] = json!(path);
        library["recent"] = json!(true);
        Ok(json!({
            "path": path,
            "name": libraries::name(&switched.dir),
            "photos": self.session.catalog.len(),
            "recent": recent,
            "library": library,
        }))
    }

    /// Another library is open (or the one before again): what the engine thread kept of the old one names its
    /// photos (renders, thumbnails asked for, the import's candidates, a merge preview, a zoom window, Edit In's
    /// files, sync requests in flight) and goes. The `state` event that follows has a new `libraryGeneration`.
    fn library_switched(&mut self) {
        self.sched.forget_all();
        self.thumbs_wanted.clear();
        self.variants.clear();
        self.candidates.clear();
        self.region = None;
        self.histogram_sent = None;
        self.faces_scan = None;
        self.merge.cancel_preview();
        if let Some((id, reply)) = self.merge_preview.take() {
            Self::reply(&self.events, reply, Event::err(id, ErrorBody::new("cancelled", "another library was opened")));
        }
        self.lightroom = Default::default();
        // (answers for the old library's requests come back to a channel nobody reads)
        self.sync = crate::sync::Driver::default();
        self.edits = crate::external_edit::Watch::load(&self.session);
        let dir = libraries::open_dir(&self.session).map(|d| d.display().to_string());
        self.demo = dir.is_none();
        self.opened = json!({"library": dir, "photos": self.session.catalog.len(), "demo": self.demo, "warnings": []});
        self.busy = true;
    }

    /// `host.library.delete {path, check?, recent?}`: delete a library that isn't open, with its photos and
    /// everything in its folder; `check: true` deletes nothing and answers what would go: `{name, bytes, capped,
    /// sync, unsent}` (`unsent`: changes it hasn't sent to its server, lost with it). Only libraries where the app
    /// keeps them, on a phone; a computer's go to the Trash from the file manager. → `{recent}` without it.
    pub(super) fn delete_library(&mut self, p: &Value) -> Result<Value, ErrorBody> {
        let refuse = |m: String| Err(ErrorBody::new("other", m));
        let path = p.get("path").and_then(Value::as_str).filter(|s| !s.is_empty());
        let path = path.ok_or_else(|| ErrorBody::new("badParams", "`host.library.delete`: missing `path`"))?;
        let dir = libraries::expand(path, self.home.as_deref());
        if self.profile == Profile::Desktop {
            return refuse(format!("{}: move the library's folder to the Trash in the Finder to delete it", dir.display()));
        }
        if !lightcraft_engine::library::is_library(&dir) {
            return refuse(format!("{}: there is no library here", dir.display()));
        }
        if libraries::open_dir(&self.session).is_some_and(|o| libraries::same(o, &dir)) {
            return refuse(format!("{}: this library is open; open another one first", dir.display()));
        }
        let location = self.libraries_location();
        if !location.is_some_and(|l| dir.parent().is_some_and(|parent| libraries::same(parent, &l))) {
            return refuse(format!("{}: only the libraries this app keeps can be deleted here", dir.display()));
        }
        if p.get("check").and_then(Value::as_bool) == Some(true) {
            let (bytes, capped) = libraries::size_on_disk(&dir, SIZE_FILES_MAX);
            let described = libraries::describe(&dir, &self.session);
            return Ok(json!({
                "name": libraries::name(&dir),
                "bytes": bytes,
                "capped": capped,
                "sync": described["sync"],
                "unsent": libraries::unsent(&dir),
            }));
        }
        // (held while it goes: another program can't open it meanwhile)
        let lock = lightcraft_catalog::LibraryLock::acquire(&dir, "LightCraft (deleting the library)")
            .map_err(|e| ErrorBody::new("libraryInUse", format!("{}: {e}", dir.display())))?;
        let removed = std::fs::remove_dir_all(&dir);
        drop(lock);
        removed.map_err(|e| ErrorBody::new("other", format!("{}: the library could not be deleted completely: {e}", dir.display())))?;
        log::info!("library {}: deleted", dir.display());
        let mut recent = self.recent_param(p);
        let gone = self.portable(&dir);
        recent.retain(|r| r != &gone);
        Ok(json!({"recent": recent}))
    }
}
