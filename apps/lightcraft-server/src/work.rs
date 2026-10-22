//! Devices as processing units. A user can move the model work on their photos (search vectors, text,
//! faces, sensitive content) off this server onto a device with a better processor, a GPU and more
//! memory, without moving the photos: the device claims a few photos, fetches only their previews,
//! processes them in memory and sends the results back, then forgets them. The server keeps the results
//! exactly as it keeps its own ([`crate::vision`]); editing never moves, it stays on the device.
//!
//! ```text
//! POST /api/work/claim      {parts: {search|text|faces|sensitive: engine, previews: any}, max?} → {items: [{part, key, kind, photo}], refused: {part: why}, progress: {previews: {done, total}}, ttl}
//! POST /api/work/release    {items: [{part, key, failed?}]}   photos the device can't do (failed: don't offer them again)
//! GET  /api/work/settings   {moved: [part], helperSeen: seconds | null}
//! PUT  /api/work/settings   {moved: [part]} | {add?: [part], remove?: [part]}   the parts this server leaves to the user's devices
//! ```
//!
//! `previews` is not a model: the device is given photos (`kind: original`) whose smart and mini previews are
//! missing, fetches each original (`GET /api/blobs/original/<hash>`), builds the previews itself and puts them
//! (`PUT /api/blobs/smart|mini/<hash>`); this server's own preview builders then leave a moved user's scanned
//! photos to it (a photo a device is showing is still built here, at once).
//!
//! A claimed photo is the device's for [`TTL`] (so another device, or this server, doesn't repeat it);
//! the results go through the routes devices already use for them (`POST /api/index/embeddings`,
//! `/text`, `/faces`, `/sensitive`), which keep only photos in the user's library and refuse a damaged
//! or other-model upload whole. A part the user moved is skipped by this server's own worker for that
//! user: it waits for a device, and [`Work::helper_seen`] says when one last came.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use lightcraft_vision::Key;
use serde_json::{Value, json};

use crate::State;
use crate::accounts;
use crate::api::{self, Resp, error};
use crate::http::Request;
use crate::switch::Part;

/// How long a claimed photo is the device's.
pub const TTL: Duration = Duration::from_secs(10 * 60);
/// How long a user's devices may go without claiming photos before an admin's "Start now" takes their work back.
const IDLE: Duration = Duration::from_secs(10 * 60);
/// Photos in one claim at most.
const MAX_CLAIM: usize = 64;
const DEFAULT_CLAIM: usize = 16;
/// The file in a user's folder.
const FILE: &str = "offload.json";
const PARTS: [Part; 4] = [Part::Search, Part::Text, Part::Faces, Part::Sensitive];
/// The work that isn't a model's: building previews.
pub const PREVIEWS: &str = "previews";

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Photos with devices, when each user's devices last claimed, and where the users' settings are.
#[derive(Default)]
pub struct Work {
    data: Option<PathBuf>,
    leases: Mutex<HashMap<(String, Part, Key), Instant>>,
    /// The same for previews (a photo's key is its content hash).
    preview_leases: Mutex<HashMap<(String, Key), Instant>>,
    seen: Mutex<HashMap<String, Instant>>,
}

impl Work {
    pub fn new(data: &Path) -> Work {
        Work { data: Some(data.to_path_buf()), ..Work::default() }
    }

    /// Whether a device has `user`'s photo `key` to process for `part` now.
    pub fn leased(&self, user: &str, part: Part, key: &Key) -> bool {
        let mut leases = lock(&self.leases);
        let k = (user.to_string(), part, *key);
        match leases.get(&k) {
            Some(at) if at.elapsed() < TTL => true,
            Some(_) => {
                leases.remove(&k);
                false
            }
            None => false,
        }
    }

    fn lease(&self, user: &str, part: Part, key: Key) {
        lock(&self.leases).insert((user.to_string(), part, key), Instant::now());
    }

    fn release(&self, user: &str, part: Part, key: Key) {
        lock(&self.leases).remove(&(user.to_string(), part, key));
    }

    fn file(&self, user: &str) -> Option<PathBuf> {
        self.data.as_deref().map(|d| accounts::user_dir(d, user).join(FILE))
    }

    /// What `user`'s file says: the work they left to their devices, and the parts an admin took back from them
    /// (missing or damaged file: none), each by name and in the same order every time.
    fn read(&self, user: &str) -> (Vec<String>, Vec<String>) {
        let v: Option<Value> = self.file(user).and_then(|f| std::fs::read(f).ok()).and_then(|b| serde_json::from_slice(&b).ok());
        let names = |key: &str| -> Vec<String> {
            let have: Vec<&str> =
                v.as_ref().and_then(|v| v[key].as_array()).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
            PARTS.iter().map(|p| p.key()).chain([PREVIEWS]).filter(|n| have.contains(n)).map(String::from).collect()
        };
        (names("moved"), names("takenBack"))
    }

    /// The work `user` left to their devices, by name.
    fn moved_names(&self, user: &str) -> Vec<String> {
        self.read(user).0
    }

    /// The parts an admin took back from `user`'s devices: their devices are told (the claim's answer) and drop them,
    /// and keep their hands off until the person chooses the part again.
    pub fn taken_back(&self, user: &str) -> Vec<String> {
        self.read(user).1
    }

    /// The model work `user` left to their devices.
    pub fn moved_parts(&self, user: &str) -> Vec<Part> {
        let names = self.moved_names(user);
        PARTS.into_iter().filter(|p| names.iter().any(|n| n == p.key())).collect()
    }

    /// Whether `user` left `part` to their devices: this server's worker doesn't do it for them.
    pub fn moved(&self, user: &str, part: Part) -> bool {
        self.moved_parts(user).contains(&part)
    }

    /// Whether `user` left building previews to their devices.
    pub fn previews_moved(&self, user: &str) -> bool {
        self.moved_names(user).iter().any(|n| n == PREVIEWS)
    }

    /// Whether a device has `user`'s photo `key` to build previews of now.
    pub fn previews_leased(&self, user: &str, key: &Key) -> bool {
        let mut leases = lock(&self.preview_leases);
        let k = (user.to_string(), *key);
        match leases.get(&k) {
            Some(at) if at.elapsed() < TTL => true,
            Some(_) => {
                leases.remove(&k);
                false
            }
            None => false,
        }
    }

    pub(crate) fn lease_previews(&self, user: &str, key: Key) {
        lock(&self.preview_leases).insert((user.to_string(), key), Instant::now());
    }

    fn release_previews(&self, user: &str, key: Key) {
        lock(&self.preview_leases).remove(&(user.to_string(), key));
    }

    #[cfg(test)]
    fn set_moved(&self, user: &str, names: &[String]) -> Result<(), String> {
        let taken = self.taken_back(user);
        self.write(user, names, &taken)
    }

    fn write(&self, user: &str, moved: &[String], taken: &[String]) -> Result<(), String> {
        let file = self.file(user).ok_or("this server has no data folder to keep the setting in")?;
        accounts::write_private_json(&file, &json!({"moved": moved, "takenBack": taken}))
    }

    /// Takes `part` back from `user`'s devices: this server's worker does it for them again, and their devices are told
    /// to stop claiming it (they leave it off until the person chooses it again).
    pub fn take_back(&self, user: &str, part: Part) -> Result<(), String> {
        let (mut moved, mut taken) = self.read(user);
        moved.retain(|n| n != part.key());
        if !taken.iter().any(|n| n == part.key()) {
            taken.push(part.key().to_string());
        }
        self.write(user, &moved, &taken)
    }

    /// `take_back` for each user whose devices have not claimed photos for [`IDLE`] (or since this server started) and
    /// who left `part` to them: who they are.
    pub fn take_back_idle(&self, users: &[String], part: Part) -> Vec<String> {
        let mut back = Vec::new();
        for user in users {
            if self.moved(user, part) && self.helper_seen(user).is_none_or(|d| d > IDLE) && self.take_back(user, part).is_ok() {
                back.push(user.clone());
            }
        }
        back
    }

    /// How long ago one of `user`'s devices last claimed photos.
    pub fn helper_seen(&self, user: &str) -> Option<Duration> {
        lock(&self.seen).get(user).map(Instant::elapsed)
    }
}

/// Why `user`'s devices may not process `part` for them, or `None` when they may.
fn refusal(st: &State, user: &str, part: Part, engine: &str) -> Option<String> {
    let v = &st.vision;
    let allowed = match part {
        Part::Search => v.switch.enabled(),
        Part::Text => v.switch.text_enabled(),
        Part::Sensitive => v.switch.sensitive_enabled(),
        Part::Faces => accounts::read_users(&st.data).is_ok_and(|f| f.users.get(user).is_some_and(|u| u.faces)),
    };
    if !allowed {
        return Some(format!("{} is off for this user on this server", part.key()));
    }
    // (an admin who paused a part, or limited it to hours, paused it for the devices that do it too)
    if !v.switch.may_run(part) {
        return Some(format!("{} is paused on this server", part.key()));
    }
    let ours = v.engine_of(part);
    (ours != engine).then(|| format!("this server keeps results of {ours}, not {engine}"))
}

/// The work a setting names: a model's part or `previews`, each once.
fn parts_of(v: &Value) -> Result<Vec<String>, String> {
    let list = v.as_array().ok_or("`moved` is a list of parts")?;
    let mut out: Vec<String> = Vec::new();
    for name in list {
        let n = name.as_str().filter(|n| *n == PREVIEWS || Part::from_key(n).is_some());
        let n = n.ok_or_else(|| format!("{name} is not work a device can take (search, text, faces, sensitive, previews)"))?;
        if !out.iter().any(|o| o == n) {
            out.push(n.to_string());
        }
    }
    Ok(out)
}

fn body_json(req: &mut Request) -> Result<Value, Resp> {
    let b = api::read_body(req, 64 << 10)?;
    serde_json::from_slice(&b).map_err(|e| error(400, format!("not JSON: {e}")))
}

/// `POST /api/work/claim`
pub(crate) fn claim(st: &State, req: &mut Request, user: &str) -> Resp {
    let body = match body_json(req) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let Some(asked) = body["parts"].as_object() else { return error(400, "`parts` maps each part to the engine that will process it") };
    let max = body["max"].as_u64().map_or(DEFAULT_CLAIM, |n| usize::try_from(n).unwrap_or(MAX_CLAIM)).clamp(1, MAX_CLAIM);
    let wanted: Vec<(Part, &str)> = asked.iter().filter_map(|(k, e)| Some((Part::from_key(k)?, e.as_str()?))).collect();
    let previews = asked.contains_key(PREVIEWS);
    let per = max.div_ceil((wanted.len() + usize::from(previews)).max(1));
    let (mut items, mut refused, mut progress) = (Vec::new(), serde_json::Map::new(), serde_json::Map::new());
    if previews {
        match crate::vision::pending_previews(st, user, per) {
            Ok((list, covered)) => {
                if let Some((done, total)) = covered {
                    progress.insert(PREVIEWS.into(), json!({"done": done, "total": total}));
                }
                for p in list {
                    let Ok(photo) = serde_json::to_value(&*p.photo) else { continue };
                    st.vision.work.lease_previews(user, p.key);
                    items.push(json!({"part": PREVIEWS, "key": p.key.to_hex(), "kind": p.kind, "photo": photo}));
                }
            }
            Err(e) => {
                refused.insert(PREVIEWS.into(), json!(e));
            }
        }
    }
    let taken = st.vision.work.taken_back(user);
    for (part, engine) in wanted {
        if taken.iter().any(|n| n == part.key()) {
            refused.insert(part.key().into(), json!("this server took it back"));
            continue;
        }
        if let Some(why) = refusal(st, user, part, engine) {
            refused.insert(part.key().into(), json!(why));
            continue;
        }
        match crate::vision::pending(st, user, part, per) {
            Ok(list) => {
                for p in list {
                    let Ok(photo) = serde_json::to_value(&*p.photo) else { continue };
                    st.vision.work.lease(user, part, p.key);
                    items.push(json!({"part": part.key(), "key": p.key.to_hex(), "kind": p.kind, "photo": photo}));
                }
            }
            Err(e) => {
                refused.insert(part.key().into(), json!(e));
            }
        }
    }
    lock(&st.vision.work.seen).insert(user.to_string(), Instant::now());
    let taken: Vec<&String> = taken.iter().filter(|n| asked.contains_key(n.as_str())).collect();
    api::json(200, &json!({"items": items, "refused": refused, "progress": progress, "ttl": TTL.as_secs(), "takenBack": taken}))
}

/// `POST /api/work/release`
pub(crate) fn release(st: &State, req: &mut Request, user: &str) -> Resp {
    let body = match body_json(req) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let Some(items) = body["items"].as_array() else { return error(400, "`items` is a list of {part, key}") };
    let mut released = 0;
    for it in items {
        if it["part"].as_str() == Some(PREVIEWS) {
            let Some(key) = it["key"].as_str().and_then(Key::from_hex) else { continue };
            st.vision.work.release_previews(user, key);
            if it["failed"].as_bool() == Some(true) {
                st.folders.mark_failed(user, &key.to_hex(), "a device couldn't build its previews");
            }
            released += 1;
            continue;
        }
        let (Some(part), Some(key)) = (it["part"].as_str().and_then(Part::from_key), it["key"].as_str().and_then(Key::from_hex)) else { continue };
        st.vision.work.release(user, part, key);
        if it["failed"].as_bool() == Some(true) {
            st.vision.mark_failed(user, part, key);
        }
        released += 1;
    }
    api::json(200, &json!({"released": released}))
}

/// `GET /api/work/settings`
pub(crate) fn settings(st: &State, user: &str) -> Resp {
    let w = &st.vision.work;
    let moved = w.moved_names(user);
    api::json(200, &json!({"moved": moved, "takenBack": w.taken_back(user), "helperSeen": w.helper_seen(user).map(|d| d.as_secs())}))
}

/// `PUT /api/work/settings`
pub(crate) fn set_settings(st: &State, req: &mut Request, user: &str) -> Resp {
    let body = match body_json(req) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let w = &st.vision.work;
    // (what the person chose on a device, not what a device does on its own when it starts: only a choice undoes an
    // admin's taking a part back)
    let chosen = body["chosen"].as_bool() == Some(true);
    let (moved, taken) = (w.moved_names(user), w.taken_back(user));
    // (a device adds and removes its own parts: another device's stay)
    let parts = if body.get("moved").is_some() {
        parts_of(&body["moved"]).map(|all| (all, true))
    } else {
        let (add, remove) = (parts_of(body.get("add").unwrap_or(&json!([]))), parts_of(body.get("remove").unwrap_or(&json!([]))));
        add.and_then(|add| remove.map(|remove| (add, remove))).map(|(add, remove)| {
            let mut now = moved.clone();
            now.retain(|p| !remove.contains(p));
            let add: Vec<String> = add.into_iter().filter(|p| (chosen || !taken.contains(p)) && !now.contains(p)).collect();
            now.extend(add);
            (now, chosen)
        })
    };
    let (parts, chosen) = match parts {
        Ok(p) => p,
        Err(e) => return error(400, e),
    };
    let taken: Vec<String> = taken.into_iter().filter(|t| !(chosen && parts.contains(t))).collect();
    match w.write(user, &parts, &taken) {
        Ok(()) => settings(st, user),
        Err(e) => error(500, e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lease_lasts_until_released_and_moved_parts_are_remembered() {
        let dir = std::env::temp_dir().join(format!("lc-server-work-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let w = Work::new(&dir);
        let key = Key([7; 16]);
        assert!(!w.leased("ann", Part::Text, &key));
        w.lease("ann", Part::Text, key);
        assert!(w.leased("ann", Part::Text, &key) && !w.leased("bob", Part::Text, &key) && !w.leased("ann", Part::Faces, &key));
        w.release("ann", Part::Text, key);
        assert!(!w.leased("ann", Part::Text, &key));

        assert!(w.moved_parts("ann").is_empty(), "nothing moved until the user says so");
        w.set_moved("ann", &["sensitive".to_string(), "search".to_string()]).unwrap();
        assert_eq!(w.moved_parts("ann"), [Part::Search, Part::Sensitive]);
        assert!(w.moved("ann", Part::Search) && !w.moved("ann", Part::Text) && !w.moved("bob", Part::Search));
        assert!(!w.previews_moved("ann"));
        w.take_back("ann", Part::Search).unwrap();
        assert_eq!(w.moved_parts("ann"), [Part::Sensitive], "only that part comes back");
        w.set_moved("ann", &["previews".to_string()]).unwrap();
        assert!(w.previews_moved("ann") && !w.previews_moved("bob") && w.moved_parts("ann").is_empty());
        w.set_moved("ann", &["sensitive".to_string(), "search".to_string()]).unwrap();
        w.lease_previews("ann", key);
        assert!(w.previews_leased("ann", &key) && !w.previews_leased("bob", &key));
        w.release_previews("ann", key);
        assert!(!w.previews_leased("ann", &key));
        std::fs::write(accounts::user_dir(&dir, "ann").join(FILE), b"{damaged").unwrap();
        assert!(w.moved_parts("ann").is_empty(), "a damaged file means nothing moved");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_parts_of_a_setting_are_checked() {
        assert_eq!(parts_of(&json!(["faces", "faces", "text", "previews"])).unwrap(), ["faces", "text", "previews"]);
        assert!(parts_of(&json!(["video"])).is_err() && parts_of(&json!("faces")).is_err());
    }
}
