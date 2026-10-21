//! Libraries for a native frontend (`host.library.open | new | recent`, [`crate::library`]): another
//! library is opened on the engine thread, as egui's File menu does, and what the host held of the
//! one before goes with it. The frontend keeps the recent list (as it keeps Local's roots) and gets it
//! back after each change.

use std::path::Path;
use std::sync::mpsc::Sender;

use serde_json::{Value, json};

use super::{Engine, ErrorBody, Event, Msg};

/// The `recent` paths a request carries.
fn recent_in(p: &Value) -> Vec<String> {
    p.get("recent").and_then(Value::as_array).into_iter().flatten().filter_map(|v| v.as_str().map(str::to_string)).collect()
}

impl Engine {
    /// `host.library.open {path, existing?, recent?}` (`existing`: only a library that is there, the folder or
    /// one folder inside it, [`crate::library::find`]: never one made by opening a folder),
    /// `host.library.new {location, name?, recent?}` (no `name`: a new folder named after the default
    /// library in `location`). Answers `{path, name, photos, warnings, relocated, recent: [path]}`.
    pub(super) fn library_request(&mut self, method: &str, p: &Value) -> Result<Value, ErrorBody> {
        let bad = |msg: &str| ErrorBody::new("badParams", format!("`{method}`: {msg}"));
        let mut recent = recent_in(p);
        let generation = self.session.library_generation();
        let opened = match method {
            "host.library.open" => {
                let path = p.get("path").and_then(Value::as_str).filter(|s| !s.is_empty()).ok_or_else(|| bad("missing `path`"))?;
                self.can_switch_library()?;
                if p.get("existing").and_then(Value::as_bool) == Some(true) {
                    crate::library::find(Path::new(path)).and_then(|dir| crate::library::open(&mut self.session, &dir.to_string_lossy(), &mut recent))
                } else {
                    crate::library::open(&mut self.session, path, &mut recent)
                }
            }
            "host.library.new" => {
                let location = p.get("location").and_then(Value::as_str).filter(|s| !s.is_empty()).ok_or_else(|| bad("missing `location`"))?;
                let name = p.get("name").and_then(Value::as_str);
                self.can_switch_library()?;
                crate::library::new(&mut self.session, Path::new(location), name, name.is_none(), &mut recent)
            }
            other => return Err(ErrorBody::new("unknownMethod", format!("unknown method `{other}`"))),
        };
        // (opening can fail after the library before was closed: it is open again then, as a new library)
        if self.session.library_generation() != generation {
            self.library_switched();
        }
        let o = opened.map_err(|e| ErrorBody::new("other", e))?;
        let warnings = self.session.take_library_warnings();
        let relocated = self.session.library.as_ref().and_then(|l| l.relocated.as_ref());
        Ok(crate::library::opened_answer(&o, &warnings, relocated, &recent))
    }

    /// `host.library.recent {recent?, remember?, clear?}`: the list as a menu shows it, `{recent: [{path, name,
    /// location, exists, open}]}`, answered from a thread of its own (whether each library is there is asked of its
    /// disk, and a server that went away can take long to say). `remember`: the library open now goes first (at
    /// launch); `clear`: forget them all.
    pub(super) fn recent_libraries(&mut self, id: u64, call: Option<&Sender<Event>>, p: &Value) -> Option<Event> {
        let mut recent = recent_in(p);
        if p.get("clear").and_then(Value::as_bool) == Some(true) {
            recent.clear();
        }
        let open = self.session.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.clone());
        if p.get("remember").and_then(Value::as_bool) == Some(true)
            && let Some(dir) = &open
        {
            crate::library::remember(&mut recent, &dir.to_string_lossy());
        }
        let (events, reply, wake) = (self.events.clone(), call.cloned(), self.wake.clone());
        let spawned = std::thread::Builder::new().name("lc-recent".into()).spawn(move || {
            let rows = crate::library::recent(&recent, open.as_deref());
            Self::reply(&events, reply, Event::ok(id, json!({"recent": rows})));
            let _ = wake.send(Msg::Wake);
        });
        match spawned {
            Ok(_) => None,
            Err(e) => Some(Event::err(id, ErrorBody::new("other", format!("can't start a thread: {e}")))),
        }
    }

    /// Work that would land in the wrong library holds a switch back (egui's rule, issue #1509): an
    /// import adds its remaining batches to whichever library is open, and so would the rest.
    fn can_switch_library(&self) -> Result<(), ErrorBody> {
        let what = if self.import.is_some() || self.scan.is_some() {
            Some("the import")
        } else if self.lightroom.running() {
            Some("Lightroom catalog import")
        } else if self.export.is_some() {
            Some("the export")
        } else if self.merge.merging.is_some() {
            Some("the photo merge")
        } else if self.super_res.is_some() {
            Some("Super Resolution")
        } else {
            None
        };
        match what {
            Some(w) => Err(ErrorBody::new("other", format!("wait for {w} to finish before switching libraries"))),
            None => Ok(()),
        }
    }

    /// Another library is open: let go of what named the old one's photos (pictures and the renders
    /// asked for, the import review's candidates, a merge preview, a folder being read into Local, the
    /// sync's answers in flight), and watch the new one's external edits.
    fn library_switched(&mut self) {
        self.sched.forget_all();
        self.sync = crate::sync::Driver::default();
        self.thumbs_wanted.clear();
        self.variants.clear();
        self.candidates.clear();
        self.region = None;
        self.faces_scan = None;
        self.merge.cancel_preview();
        if let Some((id, reply)) = self.merge_preview.take() {
            Self::reply(&self.events, reply, Event::err(id, ErrorBody::new("cancelled", "another library was opened")));
        }
        self.cancel_browse("another library was opened");
        self.edits = crate::external_edit::Watch::load(&self.session);
        let library = self.session.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.display().to_string());
        self.demo = library.is_none();
        self.opened = json!({"library": library, "photos": self.session.catalog.len(), "demo": self.demo, "warnings": []});
        self.busy = true;
    }
}
