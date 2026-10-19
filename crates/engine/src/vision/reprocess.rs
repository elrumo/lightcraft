//! Forgetting what one kind of processing found, to have it done again (`vision.reprocess`): on this device, on the
//! sync server (the user's own results there), or both. The models stay; only their results go.

use std::sync::PoisonError;
use std::sync::atomic::Ordering;

use serde_json::{Value, json};

use super::server::Aux;
use crate::Session;
use crate::cloud_sync::Body;

/// The processing a device or the server does, by the names `vision.setHelper` and the server's admin use.
const PARTS: [&str; 4] = ["search", "text", "faces", "sensitive"];

impl Session {
    /// Forget what `part` (`search`, `text`, `faces` or `sensitive`) found: `scope` `device` (this library's results),
    /// `server` (this user's results on the sync server, which then does every photo again, or has a device that does it
    /// for it do so) or `both`. → what was done; a run of indexing starts for this device when it can.
    pub fn vision_reprocess(&mut self, part: &str, scope: &str) -> Result<Value, String> {
        let part = PARTS.iter().copied().find(|p| *p == part).ok_or("`part` is search, text, faces or sensitive")?;
        let (device, server) = match scope {
            "device" => (true, false),
            "server" => (false, true),
            "both" => (true, self.vision_signed_in()),
            _ => return Err("`scope` is device, server or both".into()),
        };
        if server && !self.vision_signed_in() {
            return Err("this library isn't signed in to a server".into());
        }
        // (ask first: nothing is forgotten here when the server can't be asked)
        if server {
            let (path, aux) = match part {
                "search" => ("/api/index/embeddings", Aux::Forget),
                "faces" => ("/api/index/faces", Aux::FacesDelete),
                "text" => ("/api/index/text", Aux::Forget),
                _ => ("/api/index/sensitive", Aux::Forget),
            };
            self.vision_aux("DELETE", path, Body::Empty, aux)?;
        }
        let mut indexing = false;
        if device {
            self.vision_forget_local(part);
            indexing = self.vision_index(None, false).is_ok();
        }
        Ok(json!({"part": part, "device": device, "server": server, "indexing": indexing}))
    }

    /// Delete this library's results of `part` (every engine's) and let go of them in memory. A run of indexing that is
    /// going on is stopped first, as it would put its photos back.
    fn vision_forget_local(&mut self, part: &str) {
        if let Some(job) = self.vision.job.as_ref().filter(|j| !j.finished.load(Ordering::Relaxed)) {
            job.cancel.store(true, Ordering::Relaxed);
            // (it stops between photos: a few seconds at most)
            for _ in 0..100 {
                if job.finished.load(Ordering::Relaxed) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
        if let Some(dir) = self.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.join("search")) {
            let model_file = self.vision.spec_only().ok().map(|(model, _)| format!("{model}.bin"));
            let prefix = match part {
                "text" => Some("text-"),
                "faces" => Some("faces-"),
                "sensitive" => Some("sensitive-"),
                _ => None,
            };
            for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                let ours = match prefix {
                    Some(p) => name.starts_with(p) && name.ends_with(".bin"),
                    None => model_file.as_deref() == Some(name.as_str()),
                };
                if ours && let Err(e) = std::fs::remove_file(entry.path()) {
                    log::warn!("can't delete {}: {e}", entry.path().display());
                }
            }
        }
        let s = &self.vision.shared;
        match part {
            "search" => {
                *s.index.lock().unwrap_or_else(PoisonError::into_inner) = None;
                s.indexed.store(0, Ordering::Relaxed);
                s.opened.store(false, Ordering::Relaxed);
            }
            "text" => {
                *s.text.lock().unwrap_or_else(PoisonError::into_inner) = None;
                s.text_len.store(0, Ordering::Relaxed);
                s.text_opened.store(false, Ordering::Relaxed);
            }
            "faces" => {
                *s.face_ix.lock().unwrap_or_else(PoisonError::into_inner) = None;
                s.face_len.store(0, Ordering::Relaxed);
                s.face_photos.store(0, Ordering::Relaxed);
                s.faces_opened.store(false, Ordering::Relaxed);
                self.vision.people = Default::default();
            }
            _ => {
                *s.sensitive_ix.lock().unwrap_or_else(PoisonError::into_inner) = None;
                s.sensitive_len.store(0, Ordering::Relaxed);
                s.sensitive_rev.fetch_add(1, Ordering::Relaxed);
                s.sensitive_opened.store(false, Ordering::Relaxed);
                self.vision.sensitive_list = None;
            }
        }
    }
}
