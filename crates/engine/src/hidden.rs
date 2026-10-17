//! The Hidden album's lock. Hidden photos ([`lightcraft_catalog::Photo::hidden`]) are left out of every view
//! until the session is unlocked: with the library's Hidden password (the egui apps, the CLI, MCP) or by the
//! host itself after Face ID, Touch ID or the device passcode (the SwiftUI app, [`Session::hidden_unlock_by_device`]).
//!
//! It keeps people from seeing those photos by looking at the screen, like Apple Photos' Hidden album. It is not
//! encryption: the catalog and the files are as readable as before (docs/hidden.md).
//!
//! The password is kept as an argon2id hash in the library's preferences and synced on its own (the `privacy`
//! document, `cloud_sync/docs.rs`), so one password opens Hidden on every device of the library.

use argon2::Argon2;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use lightcraft_catalog::HiddenMode;
use serde_json::{Value, json};
use sha2::Digest;

use crate::{EngineError, LibrarySource, Result, Session};

/// The shortest password Hidden takes.
pub const MIN_PASSWORD: usize = 4;
/// Wrong passwords in a row before unlocking waits.
const MISSES_BEFORE_WAIT: u32 = 5;
/// How long it waits then.
const WAIT: std::time::Duration = std::time::Duration::from_secs(30);

/// Whether Hidden is unlocked, and what for. Never saved: a new session, an opened library and a relaunch start locked.
#[derive(Debug, Default)]
pub struct HiddenLock {
    unlocked: bool,
    /// Show Hidden: hidden photos shown with the rest of every view.
    show: bool,
    /// The library's Hidden password, an argon2id PHC string (`None`: none set yet).
    pub(crate) password: Option<String>,
    misses: u32,
    wait_until: Option<web_time::Instant>,
}

impl HiddenLock {
    pub fn unlocked(&self) -> bool {
        self.unlocked
    }
    /// Show Hidden is on (and Hidden unlocked).
    pub fn showing(&self) -> bool {
        self.unlocked && self.show
    }
    pub fn has_password(&self) -> bool {
        self.password.is_some()
    }
    /// What views other than the Hidden album do with hidden photos.
    pub(crate) fn mode(&self) -> HiddenMode {
        if self.showing() { HiddenMode::Include } else { HiddenMode::Exclude }
    }
    pub(crate) fn lock(&mut self) {
        self.unlocked = false;
        self.show = false;
    }
    /// A new library's lock: locked, with that library's password.
    pub(crate) fn for_library(password: Option<String>) -> HiddenLock {
        HiddenLock { password, ..Default::default() }
    }
}

/// `password` as an argon2id PHC string. The salt needs to be unique, not secret: it comes from the clock and
/// a counter rather than the OS's random source, which the web build doesn't have.
pub fn hash_password(password: &str) -> std::result::Result<String, String> {
    static COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let now = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or_default();
    let n = COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let seed = sha2::Sha256::digest(format!("lightcraft-hidden|{now}|{n}"));
    let salt = SaltString::encode_b64(seed.get(..16).unwrap_or(&seed)).map_err(|e| e.to_string())?;
    Argon2::default().hash_password(password.as_bytes(), &salt).map(|h| h.to_string()).map_err(|e| e.to_string())
}

/// Whether `password` is the one `phc` was made from. A damaged `phc` is an error, not a wrong password.
fn password_matches(phc: &str, password: &str) -> std::result::Result<bool, String> {
    let parsed = PasswordHash::new(phc).map_err(|e| e.to_string())?;
    Ok(Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok())
}

fn locked(msg: impl Into<String>) -> EngineError {
    EngineError::Locked(msg.into())
}

impl Session {
    /// Unlock Hidden with the library's password. Five wrong ones in a row make the next try wait 30 seconds.
    pub fn hidden_unlock(&mut self, password: &str) -> Result<()> {
        let now = web_time::Instant::now();
        if let Some(until) = self.hidden.wait_until.filter(|u| *u > now) {
            let secs = until.duration_since(now).as_secs().max(1);
            return Err(locked(format!("Too many wrong passwords: try again in {secs} s")));
        }
        let Some(phc) = self.hidden.password.clone() else {
            return Err(locked("Hidden has no password yet: set one first (hidden.setPassword)"));
        };
        let ok = password_matches(&phc, password).map_err(|e| {
            locked(format!("The Hidden password saved with this library is damaged ({e}): reset it with `lightcraft-cli hidden reset-password`"))
        })?;
        if !ok {
            self.hidden.misses = self.hidden.misses.saturating_add(1);
            if self.hidden.misses >= MISSES_BEFORE_WAIT {
                self.hidden.misses = 0;
                self.hidden.wait_until = Some(now + WAIT);
            }
            return Err(locked("Wrong password"));
        }
        self.hidden.misses = 0;
        self.hidden.wait_until = None;
        self.hidden.unlocked = true;
        Ok(())
    }

    /// Unlock Hidden because the host checked who the person is itself (Face ID, Touch ID, the device passcode).
    /// Only the SwiftUI app's host calls this (`host.hidden.unlock`): no command reaches it.
    pub fn hidden_unlock_by_device(&mut self) {
        self.hidden.unlocked = true;
        self.hidden.misses = 0;
        self.hidden.wait_until = None;
    }

    /// Set the Hidden password. Changing one needs the current password, or Hidden unlocked.
    pub fn hidden_set_password(&mut self, password: &str, current: Option<&str>) -> Result<()> {
        const C: &str = "hidden.setPassword";
        if password.chars().count() < MIN_PASSWORD {
            return Err(crate::cmd::bad(C, format!("the password needs at least {MIN_PASSWORD} characters")));
        }
        if self.hidden.password.is_some() && !self.hidden.unlocked {
            self.hidden_unlock(current.ok_or_else(|| locked("Enter the current password first"))?)?;
        }
        self.hidden.password = Some(hash_password(password).map_err(|e| EngineError::Other(format!("{C}: {e}")))?);
        self.hidden.unlocked = true;
        self.save_prefs()?;
        Ok(())
    }

    /// Forget the Hidden password, so the next unlock sets a new one. For a forgotten password: the SwiftUI
    /// app after Face ID (`host.hidden.resetPassword`) and `lightcraft-cli hidden reset-password`.
    pub fn hidden_reset_password(&mut self) -> Result<()> {
        self.hidden.password = None;
        self.hidden.lock();
        self.leave_hidden_views();
        self.save_prefs()?;
        Ok(())
    }

    /// Lock Hidden: hidden photos leave every view, and the Hidden album shows All Photos.
    pub fn hidden_lock(&mut self) {
        self.hidden.lock();
        self.leave_hidden_views();
    }

    /// Show Hidden on or off. On needs Hidden unlocked.
    pub fn hidden_show(&mut self, on: bool) -> Result<()> {
        if on && !self.hidden.unlocked {
            return Err(locked("Hidden is locked: unlock it to show hidden photos"));
        }
        self.hidden.show = on;
        self.relock_hidden_when_left();
        self.keep_active_in_view();
        Ok(())
    }

    /// Hidden locks again once nothing shows hidden photos: the Hidden album was left with Show Hidden off,
    /// or Show Hidden went off outside it.
    pub(crate) fn relock_hidden_when_left(&mut self) {
        if self.source != LibrarySource::Hidden && !self.hidden.show {
            self.hidden.unlocked = false;
        }
    }

    fn leave_hidden_views(&mut self) {
        if self.source == LibrarySource::Hidden {
            self.source = LibrarySource::All;
        }
        self.keep_active_in_view();
    }

    /// `hidden.status`, and the `hidden` block of `library.state`.
    pub fn hidden_status(&self) -> Value {
        let unlocked = self.hidden.unlocked;
        json!({
            "locked": !unlocked,
            "hasPassword": self.hidden.has_password(),
            "showHidden": self.hidden.showing(),
            // how many: only while unlocked
            "count": unlocked.then(|| self.catalog.photos().filter(|p| p.in_library() && p.hidden).count()),
        })
    }
}
