//! Which of a user's devices has which photo open, so that two devices showing the same photo can tell each other
//! ("also open on Ann's iPhone") and pick up each other's edits quickly. Nothing is kept on disk and nothing is
//! logged: a device says now and then which photo it shows, and the entry expires by itself.
//!
//! ```text
//! POST /api/presence   {photo: key | null}   → {others: [{device, name}], head, next, off?}
//! ```
//!
//! `photo` is the sync key of the photo the device shows (`null`: none). `others` are the user's other devices that
//! showed the same photo within [`TTL`]. `head` is the sync log's head (`null` when the library was busy: the answer
//! never waits for it), so a device that sees it move can pull at once. `next` is how many seconds the device should
//! wait before it says again ([`NORMAL`], or [`SHARED`] while another device shows the same photo).
//!
//! Presence is best-effort and costs the server almost nothing, and it is bounded on every side:
//! - a device may speak at most once per [`MIN_GAP`] (`429` + `Retry-After`, the table untouched);
//! - at most [`PER_USER`] entries per user and [`TOTAL`] overall (beyond that the entry is not stored and the device
//!   is sent away for a minute with `503`);
//! - entries expire after [`TTL`], swept while requests come (no thread, no timer);
//! - at most a few presence requests are answered at once (`503` + `Retry-After`: presence gives way before sync does);
//! - an admin can switch it off (`<data>/presence.json`); devices then fall back to the ordinary 5 s poll.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Value, json};

use crate::State;
use crate::accounts::{self, Session};
use crate::api::{self, Resp, UserLib, error, header};
use crate::http::Request;

/// How long an entry counts without a new message from its device.
pub const TTL: Duration = Duration::from_secs(20);
/// The least time between two messages of one device.
pub const MIN_GAP: Duration = Duration::from_secs(1);
/// What devices are asked to wait between messages.
pub const NORMAL: u64 = 8;
/// ... while another device shows the same photo.
pub const SHARED: u64 = 2;
/// ... when the server is asking to be left alone (the table is full, or presence is off).
pub const BACK_OFF: u64 = 60;
/// Entries per user at most (a user has a few devices).
pub const PER_USER: usize = 16;
/// Entries at most.
pub const TOTAL: usize = 4096;
/// The longest photo key taken (a sync key is `p` and a number).
const KEY_MAX: usize = 64;
/// The biggest request body.
const BODY_MAX: u64 = 1 << 10;
/// The file in the data folder.
const FILE: &str = "presence.json";
/// How often the whole table is swept for expired entries.
const SWEEP: Duration = Duration::from_secs(60);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Debug)]
struct Entry {
    device: u64,
    name: String,
    photo: Option<String>,
    seen: Instant,
}

#[derive(Default)]
struct Table {
    by_user: HashMap<String, Vec<Entry>>,
    swept: Option<Instant>,
}

impl Table {
    fn total(&self) -> usize {
        self.by_user.values().map(Vec::len).sum()
    }

    /// Drops the entries that expired (all users', at most once per [`SWEEP`] unless `force`).
    fn sweep(&mut self, now: Instant, force: bool) {
        if !force && self.swept.is_some_and(|at| now.saturating_duration_since(at) < SWEEP) {
            return;
        }
        self.swept = Some(now);
        for v in self.by_user.values_mut() {
            v.retain(|e| now.saturating_duration_since(e.seen) < TTL);
        }
        self.by_user.retain(|_, v| !v.is_empty());
    }
}

/// What a device is told.
#[derive(Debug, PartialEq, Eq)]
pub enum Beat {
    /// Stored (or not, when the table is full); the other devices on the same photo, and when to speak next.
    Ok { others: Vec<(u64, String)>, next: u64 },
    /// Too soon after the last message: wait this many seconds.
    TooFast(u64),
    /// The table is full (this user has [`PER_USER`] devices saying, or [`TOTAL`] overall): not stored, and the device
    /// is told to stay away for [`BACK_OFF`] seconds (it can't be counted, so it can't be rate-limited any other way).
    Full,
    /// Presence is switched off.
    Off,
}

/// The presence of every user's devices.
pub struct Presence {
    file: Option<PathBuf>,
    on: AtomicBool,
    table: Mutex<Table>,
    /// Requests being answered now.
    busy: AtomicUsize,
}

impl Default for Presence {
    fn default() -> Presence {
        Presence { file: None, on: AtomicBool::new(true), table: Mutex::new(Table::default()), busy: AtomicUsize::new(0) }
    }
}

/// One presence request being answered.
pub struct Busy<'a>(&'a AtomicUsize);

impl Drop for Busy<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Presence {
    /// Presence for a server keeping its data in `data` (an admin's switch is read from there).
    pub fn new(data: &Path) -> Presence {
        let file = data.join(FILE);
        let on =
            std::fs::read(&file).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).and_then(|v| v["enabled"].as_bool()).unwrap_or(true);
        Presence { file: Some(file), on: AtomicBool::new(on), ..Presence::default() }
    }

    pub fn enabled(&self) -> bool {
        self.on.load(Ordering::Relaxed)
    }

    /// Switches presence on or off and keeps the choice. Off forgets everything shown.
    pub fn set_enabled(&self, on: bool) -> Result<(), String> {
        if let Some(f) = &self.file {
            accounts::write_private_json(f, &json!({"enabled": on}))?;
        }
        self.on.store(on, Ordering::Relaxed);
        if !on {
            *lock(&self.table) = Table::default();
        }
        Ok(())
    }

    /// Takes a place among the presence requests answered at once, `at_most` of them (`None`: give way).
    pub fn take(&self, at_most: usize) -> Option<Busy<'_>> {
        if self.busy.fetch_add(1, Ordering::SeqCst) >= at_most {
            self.busy.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        Some(Busy(&self.busy))
    }

    /// How many devices show a photo now, and how many entries there are (the admin page).
    pub fn counts(&self, now: Instant) -> (usize, usize) {
        let t = lock(&self.table);
        let showing = t.by_user.values().flatten().filter(|e| e.photo.is_some() && now.saturating_duration_since(e.seen) < TTL).count();
        (showing, t.total())
    }

    /// `device` of `user` says it shows `photo` (`None`: none). `name` is asked only for a device not seen before.
    pub fn beat(&self, user: &str, device: u64, name: impl FnOnce() -> String, photo: Option<&str>, now: Instant) -> Beat {
        if !self.enabled() {
            return Beat::Off;
        }
        let mut t = lock(&self.table);
        t.sweep(now, false);
        // (this user's expired entries go at once: the answer never counts a device that left)
        if let Some(v) = t.by_user.get_mut(user) {
            v.retain(|e| now.saturating_duration_since(e.seen) < TTL);
        }
        if let Some(e) = t.by_user.get(user).and_then(|v| v.iter().find(|e| e.device == device)) {
            let since = now.saturating_duration_since(e.seen);
            if since < MIN_GAP {
                return Beat::TooFast((MIN_GAP - since).as_secs().max(1));
            }
        }
        let known = t.by_user.get(user).is_some_and(|v| v.iter().any(|e| e.device == device));
        if !known {
            if t.by_user.get(user).is_some_and(|v| v.len() >= PER_USER) {
                return Beat::Full;
            }
            if t.total() >= TOTAL {
                t.sweep(now, true);
                if t.total() >= TOTAL {
                    return Beat::Full;
                }
            }
        }
        let entries = t.by_user.entry(user.to_string()).or_default();
        let photo = photo.map(str::to_string);
        match entries.iter_mut().find(|e| e.device == device) {
            Some(e) => {
                e.photo = photo.clone();
                e.seen = now;
            }
            None => entries.push(Entry { device, name: name(), photo: photo.clone(), seen: now }),
        }
        let others: Vec<(u64, String)> = match &photo {
            Some(p) => entries.iter().filter(|e| e.device != device && e.photo.as_ref() == Some(p)).map(|e| (e.device, e.name.clone())).collect(),
            None => Vec::new(),
        };
        let next = if others.is_empty() { NORMAL } else { SHARED };
        Beat::Ok { others, next }
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Body {
    photo: Option<String>,
}

/// Presence requests answered at once at most: a share of the requests the server answers, and a few at least.
fn at_most(requests: usize) -> usize {
    (requests / 8).max(4)
}

fn retry_after(mut r: Resp, secs: u64) -> Resp {
    if let Some(h) = header("Retry-After", &secs.to_string()) {
        r.add_header(h);
    }
    r
}

/// `POST /api/presence`
pub(crate) fn heartbeat(st: &State, req: &mut Request, l: &Mutex<UserLib>, who: &Session) -> Resp {
    let p = &st.presence;
    if !p.enabled() {
        return api::json(200, &json!({"others": [], "head": null, "next": BACK_OFF, "off": true}));
    }
    // (presence gives way before sync does)
    let Some(_busy) = p.take(at_most(st.limits.effective().requests)) else {
        return retry_after(error(503, "the server is busy: try again in a moment"), 5);
    };
    let body = match api::read_body(req, BODY_MAX) {
        Ok(b) if b.is_empty() => Body::default(),
        Ok(b) => match serde_json::from_slice::<Body>(&b) {
            Ok(b) => b,
            Err(e) => return error(400, format!("not the JSON this route takes: {e}")),
        },
        Err(r) => return r,
    };
    if body.photo.as_ref().is_some_and(|k| k.is_empty() || k.len() > KEY_MAX) {
        return error(400, format!("`photo` is a sync key of at most {KEY_MAX} characters"));
    }
    let name = || {
        let mut a = st.accounts.lock().unwrap_or_else(PoisonError::into_inner);
        let found = a.devices_of(&who.user).into_iter().find(|d| d.id == who.device).map(|d| d.name);
        found.unwrap_or_else(|| "another device".to_string())
    };
    match p.beat(&who.user, who.device, name, body.photo.as_deref(), Instant::now()) {
        Beat::Off => api::json(200, &json!({"others": [], "head": null, "next": BACK_OFF, "off": true})),
        Beat::TooFast(secs) => retry_after(error(429, "too often: wait a moment"), secs),
        Beat::Full => retry_after(error(503, "too many devices are showing photos: try again later"), BACK_OFF),
        Beat::Ok { others, next } => {
            // (never waits for the library: a push or a pull in progress holds it, and the device asks again soon)
            let head = match l.try_lock() {
                Ok(g) => Some(g.core.head()),
                Err(std::sync::TryLockError::Poisoned(g)) => Some(g.into_inner().core.head()),
                Err(std::sync::TryLockError::WouldBlock) => None,
            };
            let others: Vec<Value> = others.into_iter().map(|(device, name)| json!({"device": device, "name": name})).collect();
            api::json(200, &json!({"others": others, "head": head, "next": next}))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(s: &str) -> impl FnOnce() -> String {
        let s = s.to_string();
        move || s
    }

    fn ok(b: Beat) -> (Vec<(u64, String)>, u64) {
        match b {
            Beat::Ok { others, next } => (others, next),
            other => panic!("expected an answer, got {other:?}"),
        }
    }

    #[test]
    fn devices_on_the_same_photo_see_each_other_and_are_told_to_speak_sooner() {
        let p = Presence::default();
        let t0 = Instant::now();
        let (others, next) = ok(p.beat("ann", 1, name("Mac"), Some("p7"), t0));
        assert!(others.is_empty() && next == NORMAL, "alone on the photo");
        let (others, next) = ok(p.beat("ann", 2, name("iPhone"), Some("p7"), t0));
        assert_eq!((others, next), (vec![(1, "Mac".to_string())], SHARED));
        // a later message from the Mac sees the iPhone too
        let t1 = t0 + Duration::from_secs(2);
        assert_eq!(ok(p.beat("ann", 1, name("ignored"), Some("p7"), t1)).0, vec![(2, "iPhone".to_string())]);
        // another photo, another user: not the same
        assert!(ok(p.beat("ann", 2, name("x"), Some("p8"), t1)).0.is_empty());
        assert!(ok(p.beat("bob", 9, name("Bob's"), Some("p7"), t1)).0.is_empty());
        // closing the photo (none) leaves nobody to see
        let t2 = t1 + Duration::from_secs(2);
        assert!(ok(p.beat("ann", 1, name("x"), None, t2)).0.is_empty());
    }

    #[test]
    fn entries_expire_without_a_message() {
        let p = Presence::default();
        let t0 = Instant::now();
        ok(p.beat("ann", 1, name("Mac"), Some("p7"), t0));
        let late = t0 + TTL + Duration::from_secs(1);
        let (others, next) = ok(p.beat("ann", 2, name("iPhone"), Some("p7"), late));
        assert!(others.is_empty() && next == NORMAL, "the Mac went quiet");
        assert_eq!(p.counts(late).0, 1);
        // the sweep forgets a user whose devices all went quiet
        let later = late + TTL + SWEEP;
        ok(p.beat("bob", 5, name("B"), Some("p1"), later));
        assert_eq!(p.counts(later).1, 1, "ann's entries are gone");
    }

    #[test]
    fn a_device_that_speaks_too_often_is_refused_and_nothing_changes() {
        let p = Presence::default();
        let t0 = Instant::now();
        ok(p.beat("ann", 1, name("Mac"), Some("p7"), t0));
        assert!(matches!(p.beat("ann", 1, name("Mac"), Some("p8"), t0 + Duration::from_millis(300)), Beat::TooFast(s) if s >= 1));
        // still on p7: a second device on p7 sees it
        let (others, _) = ok(p.beat("ann", 2, name("iPhone"), Some("p7"), t0 + Duration::from_millis(400)));
        assert_eq!(others.len(), 1);
        // after the gap it may speak
        assert!(matches!(p.beat("ann", 1, name("Mac"), Some("p8"), t0 + MIN_GAP), Beat::Ok { .. }));
    }

    #[test]
    fn the_table_is_bounded_per_user_and_overall() {
        let p = Presence::default();
        let t0 = Instant::now();
        for d in 0..PER_USER as u64 {
            ok(p.beat("ann", d, name("d"), Some("p1"), t0));
        }
        // one more device of the same user is not stored, and is sent away
        assert_eq!(p.beat("ann", 999, name("extra"), Some("p1"), t0), Beat::Full);
        assert_eq!(p.counts(t0).1, PER_USER);
        // a known device still speaks
        assert!(matches!(p.beat("ann", 0, name("d"), Some("p1"), t0 + MIN_GAP), Beat::Ok { next, .. } if next == SHARED));
        // the whole table is capped too
        let p = Presence::default();
        for u in 0..TOTAL {
            ok(p.beat(&format!("u{u}"), 1, name("d"), Some("p1"), t0));
        }
        assert_eq!(p.beat("one-too-many", 1, name("d"), Some("p1"), t0), Beat::Full);
        assert_eq!(p.counts(t0).1, TOTAL);
        // once they expire there is room again
        let later = t0 + TTL + Duration::from_secs(1);
        assert_eq!(ok(p.beat("one-too-many", 1, name("d"), Some("p1"), later)).1, NORMAL);
    }

    #[test]
    fn presence_gives_way_when_many_requests_are_being_answered() {
        let p = Presence::default();
        let held: Vec<_> = (0..4).filter_map(|_| p.take(4)).collect();
        assert_eq!(held.len(), 4);
        assert!(p.take(4).is_none(), "the fifth is turned away");
        drop(held);
        assert!(p.take(4).is_some(), "a place frees when a request ends");
        assert_eq!(at_most(64), 8);
        assert_eq!(at_most(10), 4, "a few at least");
    }

    #[test]
    fn an_admin_can_switch_it_off_and_the_choice_is_kept() {
        let dir = std::env::temp_dir().join(format!("lc-server-presence-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        let p = Presence::new(&dir);
        let t0 = Instant::now();
        ok(p.beat("ann", 1, name("Mac"), Some("p7"), t0));
        p.set_enabled(false).unwrap();
        assert_eq!(p.beat("ann", 2, name("x"), Some("p7"), t0 + MIN_GAP), Beat::Off);
        assert_eq!(p.counts(t0).1, 0, "switching off forgets what was shown");
        assert!(!Presence::new(&dir).enabled(), "kept across a restart");
        p.set_enabled(true).unwrap();
        assert!(Presence::new(&dir).enabled());
        std::fs::write(dir.join(FILE), b"{damaged").unwrap();
        assert!(Presence::new(&dir).enabled(), "a damaged file means on");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
