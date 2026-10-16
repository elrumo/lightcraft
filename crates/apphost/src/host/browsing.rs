//! Local for the SwiftUI Mac app: `library.browse` read on a worker ([`crate::browse`]), Local's places
//! (`host.local`, [`crate::local`]) and a folder's subfolders (`host.local.subfolders`, on a thread of its own).

use std::sync::mpsc::Sender;

use serde_json::{Value, json};

use super::{Engine, ErrorBody, Event, Msg};

/// A `library.browse` the frontend asked for: answered when its folder has been read.
pub(super) struct PendingBrowse {
    id: u64,
    reply: Option<Sender<Event>>,
    browse: crate::browse::Browse,
}

impl Engine {
    /// Start showing a folder (`library.browse {path, subfolders?}`). A browse already reading another folder gives
    /// way (answered `cancelled`); one reading the same folder is left to go on and answers both.
    pub(super) fn start_browse(&mut self, id: u64, call: Option<&Sender<Event>>, p: &Value) -> Option<Event> {
        if self.scan.is_some() || self.import.is_some() {
            return Some(Event::err(id, ErrorBody::new("other", "an import is running")));
        }
        let Some(path) = p.get("path").and_then(Value::as_str) else {
            return Some(Event::err(id, ErrorBody::new("badParams", "`library.browse`: missing `path`")));
        };
        let subfolders = p.get("subfolders").and_then(Value::as_bool);
        if let Some(old) = self.browse.take() {
            if old.browse.same(path, subfolders) {
                // (asked again for the folder being read: nothing restarts)
                Self::reply(&self.events, old.reply, Event::ok(old.id, json!({"path": old.browse.path, "scanning": true})));
                self.browse = Some(PendingBrowse { id, reply: call.cloned(), browse: old.browse });
                return None;
            }
            Self::reply(&self.events, old.reply, Event::err(old.id, ErrorBody::new("cancelled", "another folder is shown")));
        }
        match crate::browse::Browse::start(&mut self.session, path, subfolders) {
            Ok(browse) => {
                self.browse = Some(PendingBrowse { id, reply: call.cloned(), browse });
                None
            }
            Err(e) => Some(Event::err(id, ErrorBody::new("badParams", format!("`library.browse`: {e}")))),
        }
    }

    /// Take the browse's next step; answer it when it ends.
    pub(super) fn poll_browse(&mut self) {
        let Some(b) = self.browse.as_mut() else { return };
        self.busy = true;
        let Some(result) = b.browse.poll(&mut self.session) else { return };
        let Some(b) = self.browse.take() else { return };
        let event = match result {
            Ok(v) => Event::ok(b.id, v),
            Err(e) if e == "cancelled" => Event::err(b.id, ErrorBody::new("cancelled", e)),
            Err(e) => Event::err(b.id, ErrorBody::new("other", e)),
        };
        Self::reply(&self.events, b.reply, event);
    }

    /// `host.local`: Local's places from the roots and hidden places the app keeps, after an edit of them.
    pub(super) fn local_places(&mut self, p: &Value) -> Result<Value, String> {
        let strings = |k: &str| -> Vec<String> {
            p.get(k).and_then(Value::as_array).into_iter().flatten().filter_map(|v| v.as_str().map(str::to_string)).collect()
        };
        let (mut roots, mut hidden) = (strings("roots"), strings("hidden"));
        let path = p.get("path").and_then(Value::as_str);
        match p.get("command").and_then(Value::as_str) {
            None => {}
            Some("addRoot") => {
                crate::local::add_root(&mut roots, &mut hidden, path.ok_or("addRoot needs a path")?)?;
            }
            Some("hide") => crate::local::hide(&mut hidden, path.ok_or("hide needs a path")?),
            Some("restoreHidden") => crate::local::restore_hidden(&mut hidden, path),
            Some(other) => return Err(format!("unknown command `{other}` (addRoot | hide | restoreHidden)")),
        }
        let home = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap_or_default();
        // ponytail: the built-in places are checked on the engine thread; a worker if a home on a share is slow
        let builtin = crate::local::builtin_places(&home, str::to_string);
        let current = self.session.browse.clone().filter(|_| self.session.source == lightcraft_engine::LibrarySource::Folder).map(|b| b.path);
        let browse_root = p.get("browseRoot").and_then(Value::as_str);
        let l = crate::local::local_places(builtin, &roots, current.as_deref(), browse_root, &hidden);
        let places: Vec<Value> = l.places.iter().map(|(label, path)| json!({"label": label, "path": path})).collect();
        Ok(json!({"roots": roots, "hidden": hidden, "places": places, "owner": l.owner, "browseRoot": l.browse_root, "current": current}))
    }

    /// `host.local.subfolders {path}`: answered from a thread of its own with `[{name, path}]`.
    pub(super) fn list_subfolders(&mut self, id: u64, call: Option<&Sender<Event>>, p: &Value) -> Option<Event> {
        let Some(path) = p.get("path").and_then(Value::as_str).map(str::to_string) else {
            return Some(Event::err(id, ErrorBody::new("badParams", "`host.local.subfolders`: missing `path`")));
        };
        let (events, reply, wake) = (self.events.clone(), call.cloned(), self.wake.clone());
        let spawned = std::thread::Builder::new().name("lc-subfolders".into()).spawn(move || {
            let list: Vec<Value> = crate::local::list_subfolders(&path).into_iter().map(|(name, path)| json!({"name": name, "path": path})).collect();
            Self::reply(&events, reply, Event::ok(id, Value::Array(list)));
            let _ = wake.send(Msg::Wake);
        });
        match spawned {
            Ok(_) => None,
            Err(e) => Some(Event::err(id, ErrorBody::new("other", format!("can't start a thread: {e}")))),
        }
    }
}
