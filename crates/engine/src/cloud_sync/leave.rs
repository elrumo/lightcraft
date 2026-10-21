//! Signing out of a server (`docs/sync.md`, "Signing out"): what this device still has to send is sent first, then the
//! device signs out on the server and the library is emptied, so the next account to sign in starts from nothing and
//! never meets this one's photos, edits or presets (ids are only unique within a library: account A's photo 4 is not
//! account B's).
//!
//! - [`Session::sync_sign_out`] starts it: a library that never joined a server's keeps its photos and signs out at once;
//!   one that did waits ([`SyncConfig::leaving`]) unless told to `force` it.
//! - [`Session::leave_step`] runs at the end of every [`Session::sync_tasks`] while waiting: it answers whether there is
//!   nothing left to send, and [`Session::finish_leaving`] then signs the device out on the server and wipes.
//! - [`Session::reset_synced_library`] is the wipe. It is safe to run again: its marker ([`SyncConfig::wiping`]) is
//!   written first and `sync.json` is emptied last, so a wipe cut short is finished when the library opens.
//!
//! What goes: the catalog, the sync state, the shared documents and the files sync made (downloaded originals, smart
//! previews in the library folder, what faces and search wrote by photo id). What never does: photos imported into the
//! library folder (`Originals/`), a previews folder the user chose, and what describes this computer.

use std::time::Duration;

use lightcraft_catalog::sync::{Outbox, carry_local};
use lightcraft_catalog::{Catalog, Store};
use web_time::Instant;

use super::{Body, CONFIG, DOCS, Job, OUTBOX, PRESETS, SyncConfig, SyncState, Task, UPLOADED};
use crate::activity::Cancel;
use crate::{EngineError, LibrarySource, Result, Session};

/// The kind of the activity rows: waiting for what is left to be sent, then removing the library's files.
const ACTIVITY: &str = "signOut";

/// Background work that adds photos to the catalog: signing out waits for it (the emptied catalog would take them).
const ADDS_PHOTOS: [&str; 8] = ["import", "scan", "lightroom", "merge", "expand", "superRes", "move", "sync"];

/// A count of the photos still to send is not made again for this long while it isn't zero (it walks the library).
const COUNT_EVERY: Duration = Duration::from_secs(1);

fn other(why: impl Into<String>) -> EngineError {
    EngineError::Other(why.into())
}

/// Overwrite `name` in the library's files with valid, empty content (the files have no remove).
fn empty(files: &mut dyn Store, name: &str, content: &[u8]) -> Result<()> {
    files.write_atomic(name, content).map_err(|e| other(format!("{name}: {e}")))
}

impl Session {
    /// Is a sign-out waiting to finish, or removing the library's files? (Signing in waits for it.)
    pub(super) fn signing_out_rows(&self) -> bool {
        self.activity.list().iter().any(|t| t.kind == ACTIVITY)
    }

    /// Is something running that adds photos to the library?
    fn adds_photos(&self) -> bool {
        self.activity.list().iter().any(|t| ADDS_PHOTOS.contains(&t.kind))
    }

    /// Sign out of the server (`sync.signOut`).
    ///
    /// A library that never joined the server's (the choice at sign-in is waiting, or the sign-in hasn't been answered)
    /// has none of its photos there: it signs out at once and stays as it is. One that did removes itself from this
    /// device, which asks for `confirm`: it waits until everything this device has made has reached the server, then
    /// signs out on the server and empties the library. `force` doesn't wait: what hasn't been sent is lost (a copy of
    /// the catalog is kept in the library's folder first, always: whether anything was unsent is not for this to guess,
    /// it isn't known when the server was set up again or signed this device out), and it is the only way when the
    /// server signed this device out.
    pub fn sync_sign_out(&mut self, confirm: bool, force: bool) -> Result<()> {
        let Some(st) = self.sync.as_mut() else { return Ok(()) };
        if st.config.library.is_empty() {
            st.signed_out("signed out");
            st.error = None;
            return self.persist();
        }
        if st.config.leaving && !force {
            return Ok(());
        }
        if !confirm && !force {
            return Err(other(
                "signing out removes this library from this device (it stays on the server, and signing in again brings it back): \
                 pass `confirm: true` once the user has agreed. It waits until everything has been sent; `force: true` doesn't \
                 wait, and what was not sent is lost",
            ));
        }
        if force {
            return self.sign_out_now();
        }
        if !st.has_token() {
            return Err(other(
                "this device was signed out by the server, so nothing can be sent: pass `force: true` to remove this library from \
                 this device (what was not sent is lost)",
            ));
        }
        st.config.leaving = true;
        // (a paused sync would never send what is left: it is paused again if the wait is cancelled)
        st.signout_paused = std::mem::replace(&mut st.config.paused, false);
        st.retry_at = None;
        st.pull_at = None;
        self.persist()
    }

    /// Stay signed in after all (`sync.cancelSignOut`): nothing was removed, and the sync goes on.
    pub fn sync_cancel_sign_out(&mut self) -> Result<()> {
        let Some(st) = self.sync.as_mut().filter(|st| st.config.leaving) else {
            return Err(other("this library isn't signing out"));
        };
        st.stop_leaving();
        self.persist()
    }

    /// Whether signing out is waiting for what is left to be sent (`sync.signOut` was asked and has not finished).
    pub fn sync_leaving(&self) -> bool {
        self.sync.as_ref().is_some_and(|st| st.config.leaving)
    }

    /// The photos whose files this device still has to send: what the upload plan would ask the server about, with the
    /// waits after a failure over (a failed upload is still to be sent). Originals the server refused aren't counted.
    pub(super) fn unsent_originals(&self, st: &SyncState, now: Instant) -> usize {
        let later = now + Duration::from_secs(3600);
        self.plan_blobs(st, later).iter().filter(|j| matches!(j, Job::Head { .. })).count()
    }

    /// Is there nothing left to send? Called at the end of [`Session::sync_tasks`] while signing out; it also keeps
    /// the activity row and the count the cloud shows up to date, and ends the wait when the row is cancelled.
    pub(super) fn leave_step(&mut self, st: &mut SyncState, now: Instant) -> bool {
        let row = st.signout.get_or_insert_with(|| self.activity.start(ACTIVITY, "Finishing sync before signing out", Cancel::Yes));
        if row.is_cancelled() {
            self.stop_leaving_saved(st);
            return false;
        }
        let waiting = st.control.is_some()
            || st.login.is_some()
            || !st.jobs.is_empty()
            || !st.ready.is_empty()
            || !st.outbox.is_empty()
            || self.interaction.is_some()
            || !st.shares.idle()
            || self.presets_changed(st)
            || self.docs_pending(st)
            || self.adds_photos();
        // (counted only when nothing is moving: while photos upload their queue says how many are left)
        let unsent = match st.unsent {
            Some((at, n)) if n > 0 && now.duration_since(at) < COUNT_EVERY => n,
            _ if waiting => st.unsent.map_or(0, |(_, n)| n),
            _ => {
                let n = self.unsent_originals(st, now);
                st.unsent = Some((now, n));
                n
            }
        };
        let queued = st.transfers().0;
        let left = st.outbox.len() + if queued > 0 { queued } else { unsent };
        st.signout_peak = st.signout_peak.max(left);
        if let Some(row) = &st.signout {
            row.progress(st.signout_peak.saturating_sub(left) as u64, st.signout_peak as u64);
            let moving = st.jobs.values().find_map(|j| match j {
                Job::Head { id, .. } | Job::Put { id, .. } | Job::Proxies { id, .. } => self.catalog.photo(*id).map(|p| p.file_name.clone()),
                Job::Get { .. } => None,
            });
            row.detail(&moving.unwrap_or_default());
        }
        !waiting && unsent == 0
    }

    /// Everything has been sent: sign the device out on the server (best effort: it is told in the Task returned with
    /// `tasks`, whose answer is ignored) and empty the library.
    pub(super) fn finish_leaving(&mut self, mut st: SyncState, mut tasks: Vec<Task>) -> Vec<Task> {
        // the point of no return: a cancel accepted until now is honoured, a later one is refused
        if let Some(row) = &st.signout {
            row.set_cancellable(false);
            if row.is_cancelled() {
                self.stop_leaving_saved(&mut st);
                self.sync = Some(st);
                return tasks;
            }
        }
        let logout = st.http("POST", "/api/logout", Body::Empty, None);
        if let Err(e) = self.begin_wipe(&mut st) {
            // (nothing was removed: the sign-out is given up, not tried again every frame; the error says why)
            log::error!("sync: signing out: {e}");
            st.config.leaving = false;
            st.signout = None;
            st.unsent = None;
            st.error = Some(format!("can't sign out: {e}"));
            self.sync = Some(st);
            return tasks;
        }
        drop(st);
        if let Err(e) = self.reset_synced_library() {
            // (the marker stays: the library finishes it the next time it opens)
            log::error!("sync: emptying the library after signing out: {e}");
        }
        tasks.push(logout);
        tasks
    }

    /// The wait was cancelled (the activity row's ✕): stay signed in, and say so on disk too, so that quitting right
    /// after doesn't bring the wait back at the next launch.
    fn stop_leaving_saved(&mut self, st: &mut SyncState) {
        st.stop_leaving();
        if let Some(lib) = self.library.as_mut()
            && let Err(e) = st.save_config(lib.files_mut())
        {
            log::warn!("sync: {CONFIG}: {e}");
        }
    }

    /// Remove this library from this device now, without waiting for what is left to be sent.
    fn sign_out_now(&mut self) -> Result<()> {
        if self.adds_photos() {
            return Err(other("something is still adding photos to this library: wait for it to finish, or cancel it, first"));
        }
        let Some(mut st) = self.sync.take() else { return Ok(()) };
        // what could be lost is kept first (a library that is not on disk, the browser's, keeps nothing)
        if self.library.as_ref().is_some_and(|l| l.on_disk)
            && let Err(e) = self.keep_signout_copy()
        {
            self.sync = Some(st);
            return Err(e);
        }
        if let Err(e) = self.begin_wipe(&mut st) {
            self.sync = Some(st);
            return Err(e);
        }
        drop(st);
        self.reset_synced_library()
    }

    /// The copy kept before what was not sent is removed: the catalog (in the format of `catalog.snap`: put it in an
    /// empty folder under that name and open the folder as a library) and the settings files beside it.
    fn keep_signout_copy(&mut self) -> Result<()> {
        let name = self.keep_library_copy("catalog-before-signout")?;
        let stamp = name.strip_prefix("catalog-before-signout-").and_then(|n| n.strip_suffix(".snap")).unwrap_or_default().to_string();
        let lib = self.library.as_mut().ok_or_else(|| other("sync needs a library that is saved (open or create one first)"))?;
        for (file, stem) in [("presets.json", "presets"), ("prefs.json", "prefs")] {
            let Some(bytes) = lib.files_mut().read(file).map_err(|e| other(format!("{file}: {e}: nothing was changed")))? else { continue };
            empty(lib.files_mut(), &format!("{stem}-before-signout-{stamp}.json"), &bytes)?;
        }
        Ok(())
    }

    /// The wipe's marker: `sync.json` says the library is being emptied and holds no token any more. Written before
    /// anything is removed; if it can't be, nothing is and the sign-out is as it was.
    fn begin_wipe(&mut self, st: &mut SyncState) -> Result<()> {
        let lib = self.library.as_mut().ok_or_else(|| other("sync needs a library that is saved (open or create one first)"))?;
        let (token, leaving) = (std::mem::take(&mut st.config.token), std::mem::take(&mut st.config.leaving));
        st.config.wiping = true;
        if let Err(e) = st.save_config(lib.files_mut()) {
            st.config.token = token;
            st.config.leaving = leaving;
            st.config.wiping = false;
            return Err(other(format!("{CONFIG}: {e}: nothing was removed")));
        }
        Ok(())
    }

    /// Empty the library of everything that belonged to the account it was signed in as. Safe to run again.
    pub(crate) fn reset_synced_library(&mut self) -> Result<()> {
        // no sync state from here on: a catalog changed now must queue nothing for a server (a removal would delete
        // the account's photos there and on every device)
        self.sync = None;
        let _ = self.end_interaction();
        {
            let lib = self.library.as_mut().ok_or_else(|| other("sync needs a library that is saved (open or create one first)"))?;
            let files = lib.files_mut();
            empty(files, OUTBOX, &serde_json::to_vec(&Outbox::default()).unwrap_or_default())?;
            // (appended to as files are uploaded: left, it would tell the next server it has files it lacks)
            empty(files, UPLOADED, b"")?;
            empty(files, PRESETS, b"[]")?;
            empty(files, DOCS, b"{}")?;
        }
        // what belongs to the device alone (Local records, the folders browsed) is not the account's
        let mut fresh = Catalog::new();
        carry_local(&self.catalog, &mut fresh);
        fresh.set_id_space(0);
        self.replace_catalog_saved(fresh)?;
        self.reset_shared_documents();
        self.media.thumb_asks.clear();
        self.forget_selection();
        self.source = LibrarySource::All;
        self.filter = Default::default();
        self.forget_library_state();
        self.library_generation = crate::LIBRARY_GEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.library_identity = std::sync::Arc::new(());
        // (rendered pictures are keyed by content and stay on disk; what is shown from them is dropped, in every host)
        match self.thumb_cache_dir() {
            Some(dir) => self.media.attach_disk_cache(&dir, self.cache_bytes(), self.thumbs_dir.is_some()),
            None => self.media.rendered.clear(),
        }
        self.purge_synced_files();
        // (the account's Live Photos' clips go with their photos; a Local record carried over keeps its own)
        self.tidy_live_photos();
        if let Err(e) = self.persist() {
            log::error!("sync: {e}");
        }
        // last: only now is the library one that never synced
        let lib = self.library.as_mut().ok_or_else(|| other("sync needs a library that is saved (open or create one first)"))?;
        empty(lib.files_mut(), CONFIG, b"{}")
    }

    /// Remove the files sync made for the account: the originals downloaded from the server and the smart previews in
    /// the library folder (native: on a thread, under an activity row, and signing in waits for it), and what faces
    /// keep by photo id (at once); in a browser the host is asked to delete the preview keys it holds ([`Session::take_sync_purge`]).
    /// Never `Originals/`, never a previews folder somebody chose (libraries may share it).
    fn purge_synced_files(&mut self) {
        if let Some(store) = self.sync_store.as_mut()
            && self.media.smart_dir.is_none()
        {
            let prefix = store.prefix.clone();
            self.sync_purge.extend(store.proxies.drain().map(|name| format!("{prefix}{name}")));
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let Some(dir) = self.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.clone()) else { return };
            let folders = [dir.join("sync").join("originals"), crate::smart::dir(&dir)];
            // what faces keep by photo id goes at once, before the wipe is over: if the app is killed while the folders
            // below are being removed, nothing may be left that the next account's numbers would meet (the folders hold
            // files named by content: leftovers are only wasted room)
            if let Ok(rd) = std::fs::read_dir(&dir) {
                for path in rd.flatten().map(|e| e.path()) {
                    let ours = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n == "face-scanned.bin" || (n.starts_with("face-embeddings-") && n.ends_with(".bin")));
                    if ours {
                        let _ = std::fs::remove_file(path);
                    }
                }
            }
            let row = self.activity.start(ACTIVITY, "Removing the library from this device", Cancel::No);
            let spawned = std::thread::Builder::new().name("lc-sign-out".into()).spawn(move || {
                for folder in folders {
                    if let Err(e) = std::fs::remove_dir_all(&folder)
                        && e.kind() != std::io::ErrorKind::NotFound
                    {
                        log::warn!("sync: can't remove {}: {e}", folder.display());
                    }
                }
                drop(row);
            });
            if let Err(e) = spawned {
                log::warn!("sync: can't start the clean-up thread: {e}");
            }
        }
    }

    /// The preview keys a browser host should delete from its storage now (signing out left them), taken once.
    pub fn take_sync_purge(&mut self) -> Vec<String> {
        std::mem::take(&mut self.sync_purge)
    }

    /// Does `sync.json` still carry the marker of a wipe that did not finish (a failed write, a kill)?
    pub(super) fn wipe_pending(&mut self) -> bool {
        let Some(lib) = self.library.as_mut() else { return false };
        let config = lib.files_mut().read(CONFIG).ok().flatten().and_then(|b| serde_json::from_slice::<SyncConfig>(&b).ok());
        config.is_some_and(|c| c.wiping)
    }

    /// Run the sync until it has nothing left to do, then wait for the clean-up of a sign-out (blocking: the CLI, tests).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn sync_sign_out_wait(&mut self, limit: usize) {
        self.sync_sign_out_wait_with(limit, &mut super::run);
    }

    /// [`Session::sync_sign_out_wait`] with another transport (tests).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn sync_sign_out_wait_with(&mut self, limit: usize, run: &mut dyn FnMut(&Task) -> super::Done) {
        self.sync_now_with(limit, run);
        while self.signing_out_rows() && self.sync.is_none() {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
