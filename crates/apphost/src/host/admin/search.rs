//! What the numbers of `GET /api/admin/search` mean for the Processing screen, worked out as the
//! web console does (`apps/lightcraft-server/admin/admin.js`: `liveState`, `showSched`, `userLines`), so
//! the SwiftUI app only words and draws them. `host.admin.search` adds it to the server's answer as
//! `progress`:
//!
//! ```text
//! progress: {search | text | faces | sensitive: null | {
//!   paused, window: null | {from, to},
//!   live:  null | {state: "on" | "stuck" | "idle", why, user, secs, lastSecs},
//!   left:  {state: "done" | "paused" | "window" | "estimate" | "counting", photos, secs, perMinute, from, to},
//!   users: {<name>: {secs, perMinute, estimating, waiting, failed, problem}}}}
//! ```
//!
//! - `paused` and `window` (minutes of the day in UTC) are the schedule as it is, what Pause sends back
//!   with `paused` turned over.
//! - `live` is the line with the dot: what the worker is on now (`why`: `hung` on `user`'s photos for
//!   `secs` without finishing one, `now` on `user`'s photos, `turns` working in turn with the other
//!   indexes, `previews` nothing to do until the photos left have previews, `quiet` nothing done for
//!   `lastSecs`, `waiting` for its turn); `lastSecs` is how long ago it last finished a photo. `null`
//!   while there is nothing to do, it is paused, or it waits for its daily window.
//! - `left` is the time left of the whole part: up to date, paused, waiting for the window `from`–`to`
//!   (minutes of the day in UTC), `estimate` (about `secs` left for `photos` at `perMinute`) or only a
//!   count.
//! - `users` holds each user the part has numbers for: their own time left (`secs` at `perMinute`, or
//!   `estimating` while the pace is still being measured), the photos waiting for previews and the ones
//!   that couldn't be done, and the last problem (capitalised, only while photos are left).
//!
//! A part is `null` while it isn't on (no model, switched off; People: no face models or nobody it is
//! on for), and on a server older than these numbers (no `schedule`).

use serde_json::{Map, Value, json};

/// A turn without a finished photo for this long looks hung (each finishes a photo every few seconds).
const HUNG_SECS: u64 = 600;
/// Nothing finished for this long, with photos ready, looks stuck.
const QUIET_SECS: u64 = 1800;
/// The longest time left said (a hundred years): a pace near zero is a very long time, not a wrap.
const MAX_ETA: u64 = 100 * 365 * 24 * 3600;

/// Each kind of index: its key in the answer (and in a user's `work`), and the per-user count of its
/// photos done.
const PARTS: [(&str, &str); 4] = [("search", "indexed"), ("text", "read"), ("faces", "found"), ("sensitive", "checked")];

/// Where `part`'s schedule is in the answer (the description's is at the top).
fn schedule<'a>(s: &'a Value, part: &str) -> &'a Value {
    if part == "search" { &s["schedule"] } else { &s[part]["schedule"] }
}

/// Whether the server does `part` at all: its models are there and it is switched on (People: for
/// someone).
fn active(s: &Value, part: &str) -> bool {
    match part {
        "search" => s["active"].as_bool() == Some(true),
        "faces" => {
            let anyone = s["users"].as_array().is_some_and(|us| us.iter().any(|u| u["faces"].as_bool() == Some(true)));
            s["faces"]["installed"].as_bool() == Some(true) && anyone
        }
        _ => s[part]["active"].as_bool() == Some(true),
    }
}

/// A part's `schedule`, read leniently (a field the server left out counts as nothing).
struct Schedule {
    paused: bool,
    runs_now: bool,
    left: u64,
    ready: u64,
    per_minute: Option<f64>,
    last: Option<u64>,
    now: Option<(String, u64)>,
    window: Option<(u64, u64)>,
}

impl Schedule {
    fn read(c: &Value) -> Option<Self> {
        c.as_object()?;
        let now = c["now"]
            .as_object()
            .map(|n| (n.get("user").and_then(Value::as_str).unwrap_or_default().to_string(), n.get("secs").and_then(Value::as_u64).unwrap_or(0)));
        Some(Self {
            paused: c["paused"].as_bool() == Some(true),
            runs_now: c["runsNow"].as_bool() != Some(false),
            left: c["left"].as_u64().unwrap_or(0),
            ready: c["ready"].as_u64().unwrap_or(0),
            per_minute: pace(&c["perMinute"]),
            last: c["lastSecs"].as_u64(),
            now,
            window: c["from"].as_u64().zip(c["to"].as_u64()),
        })
    }

    /// The worker may be at this part now.
    fn going(&self) -> bool {
        !self.paused && self.runs_now
    }

    /// Whether the worker is at it: the line with the dot (`None`: no line).
    fn live(&self) -> Option<Value> {
        if self.left == 0 || !self.going() {
            return None;
        }
        let last = self.last;
        let line = |state: &str, why: &str, user: Option<&str>, secs: Option<u64>| json!({"state": state, "why": why, "user": user, "secs": secs, "lastSecs": last});
        if let Some((user, secs)) = &self.now {
            return Some(if *secs > HUNG_SECS && last.is_none_or(|l| l > HUNG_SECS) {
                line("stuck", "hung", Some(user.as_str()), Some(*secs))
            } else {
                line("on", "now", Some(user.as_str()), None)
            });
        }
        // (the three take turns, a few minutes each on a slow server)
        Some(if self.per_minute.is_some() {
            line("on", "turns", None, None)
        } else if self.ready == 0 {
            line("idle", "previews", None, None)
        } else if last.is_some_and(|l| l > QUIET_SECS) {
            line("stuck", "quiet", None, None)
        } else {
            line("idle", "waiting", None, None)
        })
    }

    /// The time left of the whole part.
    fn left(&self) -> Value {
        let photos = self.left;
        if photos == 0 {
            return json!({"state": "done", "photos": 0});
        }
        if self.paused {
            return json!({"state": "paused", "photos": photos});
        }
        if let (false, Some((from, to))) = (self.runs_now, self.window) {
            return json!({"state": "window", "photos": photos, "from": from, "to": to});
        }
        match self.per_minute.filter(|_| self.ready > 0) {
            Some(pm) => json!({"state": "estimate", "photos": photos, "secs": eta(self.ready, pm), "perMinute": pm}),
            None => json!({"state": "counting", "photos": photos}),
        }
    }
}

/// A pace the server measured: photos a minute, `None` when it did none.
fn pace(v: &Value) -> Option<f64> {
    v.as_f64().filter(|pm| pm.is_finite() && *pm > 0.0)
}

/// Seconds to do `photos` at `per_minute`, at least one.
fn eta(photos: u64, per_minute: f64) -> u64 {
    let secs = (photos as f64 / per_minute * 60.0).round();
    // (`as` saturates, and NaN is 0)
    (secs as u64).clamp(1, MAX_ETA)
}

/// The server's lower-case problem as a sentence.
fn sentence(s: &str) -> String {
    let mut chars = s.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// Under a user's bar: their time left, what waits for previews, what couldn't be done and the last
/// problem (`None`: the server has no numbers on how the worker gets on with their photos).
fn user(c: &Schedule, live: Option<&Value>, u: &Value, done: u64, w: &Value) -> Option<Value> {
    w.as_object()?;
    let total = u["total"].as_u64().unwrap_or(0);
    let left = total.saturating_sub(done);
    let waiting = w["waiting"].as_u64().unwrap_or(0);
    let failed = w["failed"].as_u64().unwrap_or(0);
    let ready = left.saturating_sub(waiting.saturating_add(failed));
    let per_minute = pace(&w["perMinute"]);
    let problem = w["problem"].as_str().filter(|p| !p.trim().is_empty());
    let secs = per_minute.filter(|_| ready > 0 && c.going()).map(|pm| eta(ready, pm));
    let on = live.is_some_and(|l| l["state"] == "on");
    Some(json!({
        "secs": secs,
        "perMinute": secs.and(per_minute),
        "estimating": secs.is_none() && ready > 0 && problem.is_none() && on,
        "waiting": waiting,
        "failed": failed,
        "problem": problem.filter(|_| left > 0).map(sentence),
    }))
}

/// `progress` for the answer `s` of `GET /api/admin/search`.
pub(super) fn progress(s: &Value) -> Value {
    let users = s["users"].as_array().map(Vec::as_slice).unwrap_or_default();
    let mut out = Map::new();
    for (key, count) in PARTS {
        let Some(c) = Schedule::read(schedule(s, key)).filter(|_| active(s, key)) else {
            out.insert(key.into(), Value::Null);
            continue;
        };
        let live = c.live();
        let mut rows = Map::new();
        for u in users {
            let (Some(name), Some(done)) = (u["name"].as_str(), u[count].as_u64()) else { continue };
            // (People counts only for the users it is on for)
            if key == "faces" && u["faces"].as_bool() != Some(true) {
                continue;
            }
            if let Some(row) = user(&c, live.as_ref(), u, done, &u["work"][key]) {
                rows.insert(name.to_string(), row);
            }
        }
        let window = c.window.map(|(from, to)| json!({"from": from, "to": to}));
        out.insert(key.into(), json!({"paused": c.paused, "window": window, "live": live, "left": c.left(), "users": rows}));
    }
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sched(extra: Value) -> Value {
        let mut c = json!({"paused": false, "from": null, "to": null, "runsNow": true, "left": 1000, "ready": 900, "perMinute": null, "lastSecs": null, "now": null});
        if let (Some(c), Some(extra)) = (c.as_object_mut(), extra.as_object()) {
            c.extend(extra.clone());
        }
        c
    }

    fn work(per_minute: Value, waiting: u64, failed: u64, problem: Value) -> Value {
        json!({"perMinute": per_minute, "waiting": waiting, "failed": failed, "problem": problem})
    }

    /// An answer with `c` as the description's schedule and one user, ann, 400 of 1,000 described.
    fn answer(c: Value, w: Value) -> Value {
        json!({
            "schedule": c,
            "active": true,
            "text": {"active": true, "schedule": sched(json!({"left": 0, "ready": 0}))},
            "faces": {"installed": true, "schedule": sched(json!({"left": 0, "ready": 0}))},
            "users": [{"name": "ann", "faces": false, "total": 1000, "indexed": 400, "read": 1000, "found": null, "work": {"search": w, "text": work(json!(null), 0, 0, json!(null)), "faces": work(json!(null), 0, 0, json!(null))}}],
        })
    }

    fn live(c: Value) -> Value {
        progress(&answer(c, json!({})))["search"]["live"].clone()
    }

    // The dot's line says what the worker is on, and calls a turn that finished nothing in ten minutes stuck
    #[test]
    fn the_live_line_says_what_the_worker_is_on() {
        let on = live(sched(json!({"now": {"user": "ann", "secs": 30}, "lastSecs": 2, "perMinute": 140.5})));
        assert_eq!(on, json!({"state": "on", "why": "now", "user": "ann", "secs": null, "lastSecs": 2}));
        let hung = live(sched(json!({"now": {"user": "ann", "secs": 4 * 3600}, "lastSecs": 5 * 3600})));
        assert_eq!(hung, json!({"state": "stuck", "why": "hung", "user": "ann", "secs": 4 * 3600, "lastSecs": 5 * 3600}));
        // (a long turn that finishes photos is fine)
        assert_eq!(live(sched(json!({"now": {"user": "ann", "secs": 900}, "lastSecs": 3})))["state"], "on");
        assert_eq!(live(sched(json!({"perMinute": 17.6, "lastSecs": 95})))["why"], "turns");
        assert_eq!(live(sched(json!({"ready": 0})))["why"], "previews");
        assert_eq!(live(sched(json!({"lastSecs": 3600})))["why"], "quiet");
        assert_eq!(live(sched(json!({"lastSecs": 3600})))["state"], "stuck");
        assert_eq!(live(sched(json!({"lastSecs": 60})))["why"], "waiting");
        // no line while there's nothing to do, it's paused, or it waits for its window
        assert_eq!(live(sched(json!({"left": 0}))), Value::Null);
        assert_eq!(live(sched(json!({"paused": true, "now": {"user": "ann", "secs": 3}}))), Value::Null);
        assert_eq!(live(sched(json!({"runsNow": false}))), Value::Null);
    }

    // The part's time left comes from the photos that can be done now, at the pace of the last ten minutes
    #[test]
    fn the_time_left_is_worked_out_from_the_ready_photos() {
        let left = |c: Value| progress(&answer(c, json!({})))["search"]["left"].clone();
        assert_eq!(left(sched(json!({"perMinute": 60.0}))), json!({"state": "estimate", "photos": 1000, "secs": 900, "perMinute": 60.0}));
        assert_eq!(left(sched(json!({"perMinute": 60.0, "ready": 0}))), json!({"state": "counting", "photos": 1000}));
        assert_eq!(left(sched(json!({}))), json!({"state": "counting", "photos": 1000}));
        assert_eq!(left(sched(json!({"left": 0}))), json!({"state": "done", "photos": 0}));
        assert_eq!(left(sched(json!({"paused": true, "perMinute": 60.0}))), json!({"state": "paused", "photos": 1000}));
        let window = left(sched(json!({"runsNow": false, "from": 1320, "to": 360})));
        assert_eq!(window, json!({"state": "window", "photos": 1000, "from": 1320, "to": 360}));
        // (a pace too small to measure is a very long time, never zero)
        assert_eq!(left(sched(json!({"perMinute": 1e-300})))["secs"], json!(MAX_ETA));
        assert_eq!(eta(1, 1e9), 1);
    }

    // Under a user's bar: their own time left, what waits for previews, what failed, and the problem as a sentence
    #[test]
    fn each_user_gets_their_own_lines() {
        let row = |c: Value, w: Value| progress(&answer(c, w))["search"]["users"]["ann"].clone();
        let c = sched(json!({"now": {"user": "ann", "secs": 30}, "lastSecs": 2, "perMinute": 140.0}));
        // 600 left, 100 wait for previews and 50 failed: 450 ready at 90 a minute is 300 s
        let r = row(c.clone(), work(json!(90.0), 100, 50, json!("the model can't read this photo")));
        assert_eq!(
            r,
            json!({"secs": 300, "perMinute": 90.0, "estimating": false, "waiting": 100, "failed": 50, "problem": "The model can't read this photo"})
        );
        // no pace of their own yet while the worker is on: the time left is being worked out
        let r = row(c.clone(), work(json!(null), 0, 0, json!(null)));
        assert_eq!((r["estimating"].clone(), r["secs"].clone()), (json!(true), json!(null)));
        // a problem says why instead
        assert_eq!(row(c.clone(), work(json!(null), 0, 0, json!("broken")))["estimating"], json!(false));
        // paused: no time left
        let r = row(sched(json!({"paused": true})), work(json!(90.0), 0, 0, json!(null)));
        assert_eq!((r["secs"].clone(), r["perMinute"].clone()), (json!(null), json!(null)));
        // a server without per-user numbers gives no row
        let old = answer(c, json!(null));
        assert_eq!(progress(&old)["search"]["users"], json!({}));
    }

    // A user who is done shows no problem; People only has rows for the users it is on for
    #[test]
    fn done_users_and_people_off_have_no_lines() {
        let mut s = answer(sched(json!({})), work(json!(null), 0, 0, json!(null)));
        s["users"][0]["work"]["text"] = work(json!(null), 0, 0, json!("reading text keeps failing"));
        s["faces"]["schedule"] = sched(json!({}));
        s["users"][0]["found"] = json!(10);
        let p = progress(&s);
        assert_eq!(p["text"]["users"]["ann"]["problem"], Value::Null);
        // (nobody has People on: nothing to say about it)
        assert_eq!(p["faces"], Value::Null);
        if let Some(us) = s["users"].as_array_mut() {
            us.push(json!({"name": "bob", "faces": true, "total": 5, "found": 1, "work": {"faces": {}}}));
        }
        assert_eq!(
            progress(&s)["faces"]["users"],
            json!({"bob": {"secs": null, "perMinute": null, "estimating": false, "waiting": 0, "failed": 0, "problem": null}})
        );
        s["users"][0]["faces"] = json!(true);
        assert_eq!(progress(&s)["faces"]["users"]["ann"]["waiting"], json!(0));
    }

    // An older server (no schedules) and answers that aren't what they should be give nulls, never a panic
    #[test]
    fn odd_answers_give_nothing() {
        assert_eq!(progress(&json!({"users": []})), json!({"search": null, "text": null, "faces": null, "sensitive": null}));
        assert_eq!(progress(&json!("not an object")), json!({"search": null, "text": null, "faces": null, "sensitive": null}));
        let weird = json!({"active": true, "schedule": {"left": -3, "ready": "many", "perMinute": "fast", "now": {"secs": -1}, "lastSecs": 1.5}, "users": [{"name": 3}, {"name": "bob", "indexed": 5, "total": 2, "work": {"search": {"waiting": u64::MAX, "failed": u64::MAX, "perMinute": f64::MAX}}}]});
        let p = progress(&weird);
        assert_eq!(p["search"]["left"], json!({"state": "done", "photos": 0}));
        assert_eq!(p["search"]["users"]["bob"]["secs"], Value::Null);
        assert_eq!((p["search"]["paused"].clone(), p["search"]["window"].clone()), (json!(false), Value::Null));
    }

    // Nothing is said about an index the server doesn't do; Pause sends the schedule back as it is
    #[test]
    fn parts_that_are_off_have_no_progress() {
        let mut s = answer(sched(json!({"paused": true, "from": 1320, "to": 360})), json!({}));
        let p = progress(&s);
        assert_eq!((p["search"]["paused"].clone(), p["search"]["window"].clone()), (json!(true), json!({"from": 1320, "to": 360})));
        s["active"] = json!(false);
        s["text"]["active"] = json!(false);
        s["faces"]["installed"] = json!(false);
        assert_eq!(progress(&s), json!({"search": null, "text": null, "faces": null, "sensitive": null}));
    }
}
