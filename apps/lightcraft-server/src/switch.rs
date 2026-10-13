//! The server-wide switches for search by description and for reading the text in photos, and
//! fetching the models of those two and of People from the admin page.
//!
//! With a switch on and its model installed, every device that signs in to this server uses it
//! without downloading anything. The switches are `<data>/search.json`
//! (`{"enabled": bool, "text": bool}`; missing, or damaged, means on: a server that had the models
//! installed before there were switches keeps working). They are read each time they matter, so
//! `lightcraft-server model disable` takes effect in a running server too. People has no switch
//! here: it is per user (`user faces NAME on`), because faces are personal data.
//!
//! A download is the CLI's (`model download`: resumable, pinned sizes and SHA-256) run on a
//! thread, so an admin needs no shell on the server: the admin page starts it after showing the
//! licence, watches [`Switch::status`] and can cancel it.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use lightcraft_engine::guard;
use lightcraft_vision::models::{self, Options, Progress};
use serde::Serialize;
use serde_json::{Value, json};

/// The file in the data folder.
pub const FILE: &str = "search.json";

/// The models an admin can fetch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// SigLIP 2: search by description.
    Search,
    /// PP-OCRv6: the text in photos.
    Text,
    /// YuNet and SFace: the people in photos.
    Faces,
}

impl Part {
    fn slot(self) -> usize {
        self as usize
    }

    fn what(self) -> &'static str {
        match self {
            Part::Search => "the search model",
            Part::Text => "the text-reading models",
            Part::Faces => "the face models",
        }
    }
}

/// Where a model download is, as the admin page shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Fetch {
    pub running: bool,
    pub done: u64,
    pub total: u64,
    /// The file being fetched.
    pub file: String,
    /// Why the last download stopped, if it did (cleared when a new one starts).
    pub error: Option<String>,
    /// The last download finished and the files are in place.
    pub finished: bool,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Default)]
struct Slot {
    fetch: Arc<Mutex<Fetch>>,
    cancel: Arc<AtomicBool>,
}

/// Whether search and text reading are on, and the model downloads an admin started.
#[derive(Default)]
pub struct Switch {
    /// `<data>/search.json` (`None`: no data folder, always on).
    file: Option<PathBuf>,
    slots: [Slot; 3],
}

impl Switch {
    pub fn new(data: &Path) -> Switch {
        Switch { file: Some(data.join(FILE)), ..Switch::default() }
    }

    /// The settings file's object (empty when there is none, or it is damaged).
    fn read(&self) -> serde_json::Map<String, Value> {
        let Some(file) = &self.file else { return Default::default() };
        match std::fs::read(file).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) {
            Some(Value::Object(o)) => o,
            _ => Default::default(),
        }
    }

    fn on(&self, key: &str) -> bool {
        self.read().get(key).and_then(Value::as_bool) != Some(false)
    }

    fn set(&self, key: &str, on: bool) -> Result<(), String> {
        let Some(file) = &self.file else { return Err("this server has no data folder to keep the setting in".into()) };
        let mut o = self.read();
        o.insert(key.to_string(), json!(on));
        crate::accounts::write_private_json(file, &Value::Object(o))
    }

    /// Search by description: on unless an admin turned it off.
    pub fn enabled(&self) -> bool {
        self.on("enabled")
    }

    pub fn set_enabled(&self, on: bool) -> Result<(), String> {
        self.set("enabled", on)
    }

    /// Reading the text in photos: on unless an admin turned it off.
    pub fn text_enabled(&self) -> bool {
        self.on("text")
    }

    pub fn set_text_enabled(&self, on: bool) -> Result<(), String> {
        self.set("text", on)
    }

    pub fn status(&self, part: Part) -> Fetch {
        lock(&self.slots[part.slot()].fetch).clone()
    }

    /// Ask a running download to stop (what has arrived is kept, and a new download resumes it).
    pub fn cancel(&self, part: Part) -> bool {
        let slot = &self.slots[part.slot()];
        let running = lock(&slot.fetch).running;
        if running {
            slot.cancel.store(true, Ordering::SeqCst);
        }
        running
    }

    /// Fetch `part`'s models into the model folder `dir` on a thread. False when that one is
    /// already running.
    pub fn start(&self, part: Part, dir: &Path, mirrors_file: Option<PathBuf>) -> Result<bool, String> {
        let env = std::env::var(models::MIRRORS_ENV).ok();
        let (env, file) = (env.as_deref(), mirrors_file.as_deref());
        let sets: Vec<models::Download> = match part {
            Part::Search => vec![(models::SIGLIP_FILES, models::mirrors(env, file), dir.to_path_buf())],
            Part::Text => models::ocr_downloads(&dir.join(lightcraft_engine::vision::TEXT_DIR), env, file),
            Part::Faces => models::face_downloads(&dir.join(lightcraft_engine::vision::FACES_DIR), env, file),
        };
        let slot = &self.slots[part.slot()];
        {
            let mut f = lock(&slot.fetch);
            if f.running {
                return Ok(false);
            }
            let total = sets.iter().flat_map(|(files, _, _)| files.iter()).filter_map(|f| f.size).sum();
            *f = Fetch { running: true, total, ..Fetch::default() };
        }
        slot.cancel.store(false, Ordering::SeqCst);
        let (fetch, cancel) = (slot.fetch.clone(), slot.cancel.clone());
        let what = part.what();
        let spawned = std::thread::Builder::new().name("lc-model-download".into()).spawn(move || {
            // (bytes of the sets finished so far, which the next set's progress adds to)
            let mut before = 0u64;
            let mut result = Ok(());
            for (files, mirrors, to) in &sets {
                let mut report = |p: &Progress| {
                    let mut f = lock(&fetch);
                    f.done = before + p.done;
                    f.total = f.total.max(f.done);
                    f.file.clone_from(&p.file);
                };
                let done =
                    guard::catch(what, || models::download(files, mirrors, to, &Options::default(), &cancel, &mut report).map_err(|e| e.to_string()));
                if let Ok(Err(e)) | Err(e) = done {
                    result = Err(e);
                    break;
                }
                before += files.iter().filter_map(|f| f.size).sum::<u64>();
            }
            let mut f = lock(&fetch);
            f.running = false;
            match result {
                Ok(()) => f.finished = true,
                Err(e) => {
                    log::warn!("downloading {what}: {e}");
                    f.error = Some(e);
                }
            }
        });
        match spawned {
            Ok(_) => Ok(true),
            Err(e) => {
                lock(&slot.fetch).running = false;
                Err(format!("can't start the download: {e}"))
            }
        }
    }

    /// Stops what runs (the server is shutting down).
    pub fn shut_down(&self) {
        for s in &self.slots {
            s.cancel.store(true, Ordering::SeqCst);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lc-switch-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::create_dir_all(&d);
        d
    }

    #[test]
    fn on_until_turned_off_and_a_damaged_file_means_on() {
        let d = dir("flag");
        let s = Switch::new(&d);
        assert!(s.enabled() && s.text_enabled(), "a server with no file keeps working");
        s.set_enabled(false).unwrap();
        assert!(!s.enabled());
        assert!(s.text_enabled(), "one switch doesn't turn the other off");
        s.set_text_enabled(false).unwrap();
        assert!(!s.text_enabled() && !s.enabled(), "and setting one keeps the other");
        s.set_enabled(true).unwrap();
        assert!(s.enabled() && !s.text_enabled());
        std::fs::write(d.join(FILE), b"{not json").unwrap();
        assert!(s.enabled() && s.text_enabled(), "damaged: on, not a crash");
        s.set_text_enabled(false).unwrap();
        assert!(!s.text_enabled(), "and it can be written over");
        assert!(Switch::default().enabled());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_second_download_does_not_start_while_one_runs() {
        let s = Switch::default();
        lock(&s.slots[Part::Text.slot()].fetch).running = true;
        assert_eq!(s.start(Part::Text, Path::new("nowhere"), None), Ok(false));
        assert!(s.cancel(Part::Text), "a running download is asked to stop");
        assert!(!s.cancel(Part::Search), "the others are not running");
        lock(&s.slots[Part::Text.slot()].fetch).running = false;
        assert!(!s.cancel(Part::Text), "nothing to stop");
    }
}
