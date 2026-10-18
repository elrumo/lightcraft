//! Devices as processing units. A user can move the model work on their photos (search vectors, text,
//! faces, sensitive content) off this server onto a device with a better processor, a GPU and more
//! memory, without moving the photos: the device claims a few photos, fetches only their previews,
//! processes them in memory and sends the results back, then forgets them. The server keeps the results
//! exactly as it keeps its own ([`crate::vision`]); editing never moves, it stays on the device.
//!
//! ```text
//! POST /api/work/claim      {parts: {search|text|faces|sensitive: engine}, max?} → {items: [{part, key, kind, photo}], refused: {part: why}, progress: {part: {done, total}}, ttl}
//! POST /api/work/release    {items: [{part, key, failed?}]}   photos the device can't do (failed: don't offer them again)
//! GET  /api/work/settings   {moved: [part], helperSeen: seconds | null}
//! PUT  /api/work/settings   {moved: [part]} | {add?: [part], remove?: [part]}   the parts this server leaves to the user's devices
//! ```
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
/// Photos in one claim at most.
const MAX_CLAIM: usize = 64;
const DEFAULT_CLAIM: usize = 16;
/// The file in a user's folder.
const FILE: &str = "offload.json";
const PARTS: [Part; 4] = [Part::Search, Part::Text, Part::Faces, Part::Sensitive];

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Photos with devices, when each user's devices last claimed, and where the users' settings are.
#[derive(Default)]
pub struct Work {
    data: Option<PathBuf>,
    leases: Mutex<HashMap<(String, Part, Key), Instant>>,
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

    /// The parts `user` left to their devices (missing or damaged file: none).
    pub fn moved_parts(&self, user: &str) -> Vec<Part> {
        let v: Option<Value> = self.file(user).and_then(|f| std::fs::read(f).ok()).and_then(|b| serde_json::from_slice(&b).ok());
        let names: Vec<&str> =
            v.as_ref().and_then(|v| v["moved"].as_array()).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
        PARTS.into_iter().filter(|p| names.contains(&p.key())).collect()
    }

    /// Whether `user` left `part` to their devices: this server's worker doesn't do it for them.
    pub fn moved(&self, user: &str, part: Part) -> bool {
        self.moved_parts(user).contains(&part)
    }

    fn set_moved(&self, user: &str, parts: &[Part]) -> Result<(), String> {
        let file = self.file(user).ok_or("this server has no data folder to keep the setting in")?;
        let names: Vec<&str> = parts.iter().map(|p| p.key()).collect();
        accounts::write_private_json(&file, &json!({"moved": names}))
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
    let ours = v.engine_of(part);
    (ours != engine).then(|| format!("this server keeps results of {ours}, not {engine}"))
}

fn parts_of(v: &Value) -> Result<Vec<Part>, String> {
    let list = v.as_array().ok_or("`moved` is a list of parts")?;
    let mut out = Vec::new();
    for name in list {
        let part = name.as_str().and_then(Part::from_key).ok_or_else(|| format!("{name} is not a part (search, text, faces, sensitive)"))?;
        if !out.contains(&part) {
            out.push(part);
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
    let per = max.div_ceil(wanted.len().max(1));
    let (mut items, mut refused, mut progress) = (Vec::new(), serde_json::Map::new(), serde_json::Map::new());
    for (part, engine) in wanted {
        if let Some(why) = refusal(st, user, part, engine) {
            refused.insert(part.key().into(), json!(why));
            continue;
        }
        match crate::vision::pending(st, user, part, per) {
            Ok((list, (done, total))) => {
                progress.insert(part.key().into(), json!({"done": done, "total": total}));
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
    api::json(200, &json!({"items": items, "refused": refused, "progress": progress, "ttl": TTL.as_secs()}))
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
    let moved: Vec<&str> = w.moved_parts(user).iter().map(|p| p.key()).collect();
    api::json(200, &json!({"moved": moved, "helperSeen": w.helper_seen(user).map(|d| d.as_secs())}))
}

/// `PUT /api/work/settings`
pub(crate) fn set_settings(st: &State, req: &mut Request, user: &str) -> Resp {
    let body = match body_json(req) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let w = &st.vision.work;
    // (a device adds and removes its own parts: another device's stay)
    let parts = if body.get("moved").is_some() {
        parts_of(&body["moved"])
    } else {
        let (add, remove) = (parts_of(body.get("add").unwrap_or(&json!([]))), parts_of(body.get("remove").unwrap_or(&json!([]))));
        add.and_then(|add| remove.map(|remove| (add, remove))).map(|(add, remove)| {
            let mut now = w.moved_parts(user);
            now.retain(|p| !remove.contains(p));
            add.into_iter().filter(|p| !now.contains(p)).collect::<Vec<_>>().into_iter().fold(now, |mut all, p| {
                all.push(p);
                all
            })
        })
    };
    let parts = match parts {
        Ok(p) => p,
        Err(e) => return error(400, e),
    };
    match w.set_moved(user, &parts) {
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
        w.set_moved("ann", &[Part::Sensitive, Part::Search]).unwrap();
        assert_eq!(w.moved_parts("ann"), [Part::Search, Part::Sensitive]);
        assert!(w.moved("ann", Part::Search) && !w.moved("ann", Part::Text) && !w.moved("bob", Part::Search));
        std::fs::write(accounts::user_dir(&dir, "ann").join(FILE), b"{damaged").unwrap();
        assert!(w.moved_parts("ann").is_empty(), "a damaged file means nothing moved");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_parts_of_a_setting_are_checked() {
        assert_eq!(parts_of(&json!(["faces", "faces", "text"])).unwrap(), [Part::Faces, Part::Text]);
        assert!(parts_of(&json!(["video"])).is_err() && parts_of(&json!("faces")).is_err());
    }
}
