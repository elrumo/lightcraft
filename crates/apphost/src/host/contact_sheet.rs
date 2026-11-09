//! Contact sheets made on a worker (`export.contactSheet`), so the engine answers other requests meanwhile. A sheet
//! has its row in the activity stack (✕ stops it); a preview (`preview: true`) has none and gives way to a newer
//! preview (answered `cancelled`).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;

use lightcraft_engine::activity::Cancel;
use lightcraft_engine::contact_sheet;
use serde_json::Value;

use super::{Engine, ErrorBody, Event, Msg};

const ID: &str = "export.contactSheet";

impl Engine {
    /// Start `export.contactSheet`: `None` (answered when it ends) or the error now.
    pub(super) fn start_contact_sheet(&mut self, id: u64, call: Option<&Sender<Event>>, p: &Value) -> Option<Event> {
        let bad = |code: &str, m: String| Some(Event::err(id, ErrorBody::new(code, format!("`{ID}`: {m}"))));
        let Some(path) = p.get("path").and_then(Value::as_str).filter(|s| !s.trim().is_empty()) else {
            return bad("badParams", "missing path".into());
        };
        let preview = p.get("preview").and_then(Value::as_bool) == Some(true);
        if preview && let Some(older) = self.sheet_preview.take() {
            older.store(true, Ordering::Relaxed);
        }
        let work = match contact_sheet::prepare(&mut self.session, p) {
            Ok(work) => work,
            Err(e) => return bad("badParams", e),
        };
        let paths = work.paths(path);
        for path in &paths {
            if let Err(e) = self.session.check_write_target(path) {
                return bad("other", e);
            }
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let task = (!preview).then(|| self.session.activity.start("export", "Exporting contact sheet", Cancel::Flag(cancel.clone())));
        if preview {
            self.sheet_preview = Some(cancel.clone());
        }
        let total = work.len() as u64;
        let (events, reply, wake) = (self.events.clone(), call.cloned(), self.wake.clone());
        let spawned = std::thread::Builder::new().name("lc-contact-sheet".into()).spawn(move || {
            let made = lightcraft_engine::guard::catch("the contact sheet", || {
                let doc = work.run(&mut |done, name| {
                    if let Some(task) = &task {
                        task.progress(done as u64, total);
                        task.detail(name);
                    }
                    !cancel.load(Ordering::Relaxed)
                })?;
                contact_sheet::write(&doc, &paths, &mut |path, bytes| lightcraft_engine::export::write_file(path, bytes))
            });
            let event = match made {
                Ok(Ok(v)) => Event::ok(id, v),
                Ok(Err(_)) if cancel.load(Ordering::Relaxed) => Event::err(id, ErrorBody::new("cancelled", "cancelled")),
                Ok(Err(e)) => Event::err(id, ErrorBody::new("other", format!("`{ID}`: {e}"))),
                Err(e) => Event::err(id, ErrorBody::new("panic", e)),
            };
            drop(task);
            Self::reply(&events, reply, event);
            let _ = wake.send(Msg::Wake);
        });
        match spawned {
            Ok(_) => None,
            Err(e) => bad("other", format!("can't start a thread: {e}")),
        }
    }
}
