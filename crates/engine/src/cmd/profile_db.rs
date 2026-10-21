//! The profile database's commands (`profileDb.*`, [`crate::profile_db`]): what is installed, an update from a feed
//! in the background (shown in the activity stack, cancellable), removal; and `photo.lensProfile`, which lens
//! correction a photo has and where it comes from.

use std::sync::{Arc, Mutex, PoisonError};

use serde_json::{Value, json};

use super::{CommandSpec, always, bad, bool_or, cmd};
use crate::profile_db::{self, Outcome, Store};
use crate::{Result, Session};

#[derive(Default)]
struct Shared {
    running: bool,
    last: Option<Outcome>,
}

/// The session's profile database updates: whether one is running and how the last one ended.
#[derive(Default)]
pub struct Updates {
    shared: Arc<Mutex<Shared>>,
}

impl Updates {
    fn lock(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn running(&self) -> bool {
        self.lock().running
    }
}

fn store(c: &str) -> Result<Store> {
    profile_db::dir().map(Store::at).ok_or_else(|| bad(c, "there is no settings folder to keep the profile database in"))
}

fn outcome_json(o: &Outcome) -> Value {
    match o {
        Outcome::UpToDate { serial } => json!({"result": "upToDate", "serial": serial}),
        Outcome::Updated { serial, files, bytes } => json!({"result": "updated", "serial": serial, "files": files, "bytes": bytes}),
        Outcome::Failed(e) => json!({"result": "failed", "error": e}),
        Outcome::Cancelled => json!({"result": "cancelled"}),
    }
}

/// `profileDb.status`; `brief`: the last check and the update in progress only, without reading the database (a
/// frontend deciding whether a check is due).
fn status(s: &mut Session, p: &Value) -> Result<Value> {
    let dir = profile_db::dir();
    let state = dir.clone().map(Store::at).map(|s| s.state()).unwrap_or_default();
    let checks = || {
        let updates = s.profile_db.lock();
        json!({
            "url": (!state.url.is_empty()).then_some(state.url.clone()),
            "checked": (state.checked > 0).then_some(state.checked),
            "updated": (state.installed > 0).then_some(state.installed),
            "error": state.error.clone(),
            "running": updates.running,
            "last": updates.last.as_ref().map(outcome_json),
        })
    };
    if bool_or(p, "brief", false) {
        return Ok(checks());
    }
    let mut out = checks();
    let db = profile_db::installed();
    let sources: Vec<Value> = db
        .manifest
        .iter()
        .flat_map(|m| {
            m.sources.iter().map(move |src| {
                let files = m.known().filter(|e| e.source == src.id).count();
                json!({"id": src.id, "title": src.title, "licence": src.licence, "url": src.url, "credit": src.credit, "revision": src.revision, "files": files})
            })
        })
        .collect();
    let mut cameras: Vec<&str> = db.cameras.keys().map(String::as_str).collect();
    cameras.sort_unstable();
    let more = json!({
        "installed": db.manifest.is_some(),
        "serial": db.manifest.as_ref().map(|m| m.serial),
        "published": db.manifest.as_ref().map(|m| m.published.clone()),
        "cameras": cameras,
        "lenses": db.lenses.lenses.len(),
        "lensCameras": db.lenses.cameras.len(),
        "bytes": db.bytes,
        "sources": sources,
        "skipped": db.skipped,
        "path": dir,
        "folder": std::env::var_os(crate::lens_profiles::ENV).map(std::path::PathBuf::from),
    });
    if let (Some(out), Value::Object(more)) = (out.as_object_mut(), more) {
        out.extend(more);
    }
    Ok(out)
}

/// `profileDb.remove`.
fn remove(s: &mut Session, _: &Value) -> Result<Value> {
    const C: &str = "profileDb.remove";
    if s.profile_db.running() {
        return Err(bad(C, "the profile database is updating: wait for it or cancel it first"));
    }
    store(C)?.remove().map_err(|e| bad(C, e))?;
    profile_db::changed();
    Ok(json!({"removed": true}))
}

/// `photo.lensProfile`: the active (or given) photo's lens correction and where it comes from.
fn lens_profile(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "photo.lensProfile";
    let id = match p.get("id").and_then(Value::as_u64) {
        Some(id) => lightcraft_catalog::PhotoId(id),
        None => s.active().ok_or_else(|| bad(C, "no photo selected"))?,
    };
    let photo = s.catalog.photo(id).ok_or_else(|| bad(C, "no such photo"))?;
    let (lens_name, camera_name, enabled, raw) =
        (photo.meta.lens.clone(), photo.meta.camera.clone(), photo.develop.optics.lens_profile, photo.develops_raw());
    let lens = s.embedded_lens(id);
    use lightcraft_develop::LensOrigin;
    let origin = lens.map(|l| match l.origin {
        LensOrigin::Database { .. } => "database",
        _ if l.measured() => "measured",
        _ => "file",
    });
    // the profile's names, for a correction from a lens database (found again from the catalog's camera and lens)
    let profile = match lens.map(|l| l.origin) {
        Some(LensOrigin::Database { .. }) => crate::lens_profiles::database().and_then(|db| {
            let camera = camera_name.split_once(' ').and_then(|(make, model)| db.find_camera(make, model)).cloned();
            db.find_lens(None, &lens_name, camera.as_ref()).map(|l| json!({"maker": l.maker, "model": l.models.first(), "mounts": l.mounts}))
        }),
        _ => None,
    };
    Ok(json!({
        "id": id.0,
        "raw": raw,
        "lens": (!lens_name.is_empty()).then_some(lens_name),
        "origin": origin,
        "serial": lens.and_then(|l| match l.origin { LensOrigin::Database { serial } => Some(serial), _ => None }),
        "distortion": lens.is_some_and(|l| l.warp.is_some() || l.has_stages()),
        "vignetting": lens.is_some_and(|l| l.vignette.is_some()),
        "profile": profile,
        "enabled": enabled,
    }))
}

#[cfg(not(target_arch = "wasm32"))]
mod download {
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;
    use crate::activity::{Cancel, Unit};
    use crate::cmd::str_param;

    fn now() -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
    }

    /// `profileDb.update`: starts checking the feed and installing what is new, in the background.
    pub(super) fn start(s: &mut Session, p: &Value) -> Result<Value> {
        const C: &str = "profileDb.update";
        let store = store(C)?;
        let given = str_param(p, "url").map(str::to_owned).or_else(|| std::env::var(profile_db::ENV_URL).ok()).or_else(|| {
            let url = store.state().url;
            (!url.is_empty()).then_some(url)
        });
        let Some(given) = given else { return Err(bad(C, "no profile database address: give `url`")) };
        let Some(base) = profile_db::feed_base(&given) else { return Err(bad(C, "the profile database address must start with https://")) };
        let shared = s.profile_db.shared.clone();
        {
            let mut sh = s.profile_db.lock();
            if sh.running {
                return Ok(json!({"started": false, "running": true}));
            }
            sh.running = true;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let task = s.activity.start("download", "Updating camera and lens profiles", Cancel::Flag(cancel.clone()));
        task.set_unit(Unit::Bytes);
        task.detail(base.split('/').nth(2).unwrap_or_default());
        let handle = task.handle();
        let thread_shared = shared.clone();
        let thread_base = base.clone();
        let spawned = std::thread::Builder::new().name("profile-db-update".into()).spawn(move || {
            // the activity row goes when the thread ends, however it ends
            let _task = task;
            let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                profile_db::update(&store, &thread_base, &cancel, &mut |done, total| handle.progress(done, total.max(done)))
            }));
            let outcome = match run {
                Ok(Ok(o)) => o,
                _ if cancel.load(Ordering::Relaxed) => Outcome::Cancelled,
                Ok(Err(e)) => Outcome::Failed(format!("Could not update the camera and lens profiles: {e}.")),
                Err(_) => Outcome::Failed("The camera and lens profiles update stopped unexpectedly. Try again.".into()),
            };
            let mut state = store.state();
            state.url = thread_base;
            state.checked = now();
            state.error = match &outcome {
                Outcome::Failed(e) => Some(e.clone()),
                _ => None,
            };
            if matches!(outcome, Outcome::Updated { .. }) {
                state.installed = state.checked;
                profile_db::changed();
            }
            if let Err(e) = store.save_state(&state) {
                log::warn!("profile database: {e}");
            }
            if let Outcome::Failed(e) = &outcome {
                log::warn!("{e}");
            }
            let mut sh = thread_shared.lock().unwrap_or_else(PoisonError::into_inner);
            sh.running = false;
            sh.last = Some(outcome);
        });
        if let Err(e) = spawned {
            shared.lock().unwrap_or_else(PoisonError::into_inner).running = false;
            return Err(bad(C, format!("could not start the update: {e}")));
        }
        Ok(json!({"started": true, "running": true, "url": base}))
    }
}

pub fn specs() -> Vec<CommandSpec> {
    #[allow(unused_mut)]
    let mut v = vec![
        cmd!(
            query "profileDb.status",
            "Camera and Lens Profiles Status",
            [],
            None,
            "{brief?: only the check fields, without reading the database} → {installed, serial, published, cameras: [model], lenses, lensCameras, bytes, sources: [{id, title, licence, url, credit, revision, files}], skipped: [why], path, url, checked, updated (seconds since 1970), error, running, last: {result: upToDate|updated|failed|cancelled, serial?, files?, bytes?, error?}, folder} — the downloaded profile database (docs/profile-db.md); show `sources`' credits wherever its profiles are offered",
            always,
            status
        ),
        cmd!(
            query "profileDb.remove",
            "Remove Downloaded Camera and Lens Profiles",
            [],
            None,
            "{} → {removed} — deletes the downloaded profile database; photos keep the lens corrections already recorded",
            always,
            remove
        ),
        cmd!(
            query "photo.lensProfile",
            "Lens Profile",
            [],
            None,
            "{id?} → {id, raw, lens, origin: file|measured|database|null, serial, distortion, vignetting, profile: {maker, model, mounts}|null, enabled} — the photo's lens correction (Enable Profile Corrections applies it) and where it comes from",
            always,
            lens_profile
        ),
    ];
    #[cfg(not(target_arch = "wasm32"))]
    v.push(cmd!(
        query "profileDb.update",
        "Update Camera and Lens Profiles",
        [],
        None,
        "{url?: feed address (default: LIGHTCRAFT_PROFILE_DB_URL, else the last one used)} → {started, running, url} — checks the feed in the background (an activity row) and installs what is new; only files the device lacks are downloaded, each checked against its SHA-256; no photo data is sent",
        always,
        download::start
    ));
    v
}
