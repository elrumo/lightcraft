//! The app settings and UI state (`<config>/ui.json`, [`UiState`]): read at launch, written when
//! they change (desktop and iOS hosts; the browser keeps them in its own storage).

use crate::{LightcraftApp, UiState};

/// `<config>/ui.json`, the saved UI state and app settings — `None` when nothing is read or
/// written: `LIGHTCRAFT_NO_PREFS` (tests, scripts) or no home folder.
pub fn prefs_path() -> Option<std::path::PathBuf> {
    if std::env::var_os("LIGHTCRAFT_NO_PREFS").is_some() {
        return None;
    }
    lightcraft_engine::config::config_dir().map(|d| d.join("ui.json"))
}

/// The saved UI state and app settings (`<config>/ui.json`), if any, and a warning for the user
/// when the file exists but can't be used (issue #103): a damaged file is kept as
/// `ui.json.corrupt-<unix time>` first, so the next save can't lose the library location in it;
/// one that can't be read at all is not written this session (`keep_file`).
///
/// A `--memory` session reads the settings (so it looks like the user's app) but writes nothing
/// (issue #164), so a damaged file is left as it is there, not set aside.
pub fn load_prefs(in_memory: bool) -> (Option<UiState>, Option<String>, bool) {
    let Some(path) = prefs_path() else { return (None, None, false) };
    load_prefs_at(&path, !in_memory)
}

/// Compatibility recovery is limited to persisted docking, not runtime UiState commands.
pub fn decode_saved_ui(bytes: &[u8]) -> Result<(UiState, bool), String> {
    let mut saved: serde_json::Value = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    let fields = saved.as_object_mut().ok_or("settings must be an object")?;
    let recovered = fields.get("docking").is_some_and(|value| match serde_json::from_value::<Option<crate::docking::Workspace>>(value.clone()) {
        Ok(Some(workspace)) => workspace.validate().is_err(),
        Ok(None) => false,
        Err(_) => true,
    });
    if recovered {
        fields.remove("docking");
    }
    let ui = serde_json::from_value(saved).map_err(|error| error.to_string())?;
    Ok((ui, recovered))
}

pub fn load_prefs_at(path: &std::path::Path, set_aside_damaged: bool) -> (Option<UiState>, Option<String>, bool) {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (None, None, false),
        Err(e) => {
            let msg = format!("The app settings ({}) couldn't be read: {e}. Defaults are used, and the file isn't overwritten.", path.display());
            return (None, Some(msg), true);
        }
    };
    match decode_saved_ui(&bytes) {
        Ok((ui, false)) => (Some(ui.sanitized()), None, false),
        Ok((ui, true)) => {
            let msg = format!(
                "The saved panel layout in {} couldn't be restored. Other settings were loaded; the original file is preserved and settings won't be saved this session.",
                path.display()
            );
            (Some(ui.sanitized()), Some(msg), true)
        }
        Err(e) if !set_aside_damaged => {
            let msg = format!("The app settings ({}) are damaged ({e}). Defaults are used; nothing is saved in this session.", path.display());
            (None, Some(msg), true)
        }
        Err(e) => {
            let secs = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            let keep = path.with_file_name(format!("ui.json.corrupt-{secs}"));
            let kept = std::fs::rename(path, &keep);
            let what = match &kept {
                Ok(()) => format!("it was kept as {}", keep.display()),
                Err(r) => format!("it couldn't be set aside ({r})"),
            };
            let msg = format!(
                "The app settings ({}) are damaged ({e}); {what}. Defaults are used — reopen your library with Settings → Open Library… if it isn't shown.",
                path.display()
            );
            (None, Some(msg), kept.is_err())
        }
    }
}

/// Write `bytes` to `path` atomically: a temp file, fsynced, renamed over it (a crash or a full
/// disk leaves the old file or the new one, never a truncated one).
pub fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let tmp = path.with_extension("json.tmp");
    let written = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    std::fs::rename(&tmp, path)?;
    if let Some(dir) = path.parent()
        && let Ok(d) = std::fs::File::open(dir)
    {
        let _ = d.sync_all();
    }
    Ok(())
}

/// Saves `ui.json` (app settings, incl. the library to open at launch): as soon as the library
/// changes, every few seconds when anything else changed, and at exit.
///
/// Nothing is written while the session is one that promises to save nothing: `--memory`
/// (`path` is `None`; issue #164), or the one running because the library couldn't be opened — the
/// question window and Continue Without Saving (issue #100; `app.openLibrary` ends it, and the
/// library it opened is saved then).
#[derive(Default)]
pub struct PrefsWriter {
    /// Where to write; `None` writes nothing (`--memory`, `LIGHTCRAFT_NO_PREFS`, no home folder).
    pub path: Option<std::path::PathBuf>,
    /// What was last written (or read at launch): only changes are written.
    pub written: Vec<u8>,
    /// The library location last written: a change is written at once.
    pub library: String,
    /// When (app time, s) the settings were last compared with what was written.
    pub checked: f64,
    /// The last write failed (reported once until a write works).
    pub failing: bool,
    /// `ui.json` couldn't be read at launch: never overwrite it this session.
    pub keep_file: bool,
    /// Follows the window, to remember the size it rests at (`UiState::window`).
    pub window: crate::window_place::WindowWatch,
    /// When the window was first looked at: its watch goes by the clock from then on, not by
    /// the interface's time, which stands still while the window is hidden.
    pub window_since: Option<web_time::Instant>,
}

impl PrefsWriter {
    /// A writer to [`prefs_path`] for `app`'s settings as they are now (only changes are written);
    /// `keep_file`: `ui.json` couldn't be read at launch ([`load_prefs`]), so it is never overwritten.
    pub fn new(app: &LightcraftApp, keep_file: bool) -> PrefsWriter {
        PrefsWriter {
            path: prefs_path(),
            written: serde_json::to_vec_pretty(&app.ui).unwrap_or_default(),
            library: app.ui.settings.library_path.clone(),
            keep_file,
            ..Default::default()
        }
    }

    pub fn save(&mut self, app: &LightcraftApp) -> Result<(), String> {
        self.save_ui(&app.ui, app.library_problem.as_ref().is_some_and(crate::panels::library_problem::LibraryProblem::saves_nothing))
    }

    /// Write `ui` if it changed since the last write; `temporary` (the library couldn't be
    /// opened) saves nothing.
    pub fn save_ui(&mut self, ui: &UiState, temporary: bool) -> Result<(), String> {
        if self.keep_file || temporary {
            return Ok(());
        }
        let Some(path) = self.path.clone() else { return Ok(()) };
        if let Some(workspace) = &ui.docking {
            workspace.validate()?;
        }
        let bytes = serde_json::to_vec_pretty(ui).map_err(|e| e.to_string())?;
        if bytes == self.written {
            return Ok(());
        }
        path.parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| write_atomic(&path, &bytes))
            .map_err(|e| format!("saving the app settings failed: {e}"))?;
        self.written = bytes;
        self.library = ui.settings.library_path.clone();
        Ok(())
    }

    /// What to remember of the window as the app quits ([`WindowWatch::at_exit`]).
    ///
    /// [`WindowWatch::at_exit`]: crate::window_place::WindowWatch::at_exit
    pub fn window_at_exit(&mut self, remembered: Option<crate::window_place::WindowPlace>) -> Option<crate::window_place::WindowPlace> {
        match self.window_since {
            Some(since) => self.window.at_exit(remembered, since.elapsed().as_secs_f64()),
            None => remembered,
        }
    }

    pub fn tick(&mut self, app: &mut LightcraftApp, ctx: &egui::Context) {
        // (asked for before the input is read: the context is not asked from inside its own lock)
        let zoom = ctx.zoom_factor();
        let (now, seen) = ctx.input(|i| (i.time, crate::window_place::Seen::of(i.viewport(), i.content_rect(), zoom)));
        // how the window is, to open like that next time (written with the settings below). What
        // the settings hold is checked first: a script may have set it (`ui.set`)
        let remembered = app.ui.window.and_then(crate::window_place::WindowPlace::checked);
        let at = self.window_since.get_or_insert_with(web_time::Instant::now).elapsed().as_secs_f64();
        app.ui.window = self.window.note(remembered, &seen, at);
        if let Some(wait) = self.window.waits(at) {
            // (a size is only remembered once the window has rested at it: look again then, though
            // nothing else asks for a frame)
            ctx.request_repaint_after(wait);
        }
        let moved = app.ui.settings.library_path != self.library;
        if !moved && now - self.checked < 3.0 {
            return;
        }
        self.checked = now;
        match self.save(app) {
            Ok(()) => self.failing = false,
            Err(e) => {
                if !self.failing {
                    log::warn!("{e}");
                    app.notices.push(format!("{e}. LightCraft keeps trying."));
                }
                self.failing = true;
                // don't retry every frame
                self.library = app.ui.settings.library_path.clone();
            }
        }
    }
}
