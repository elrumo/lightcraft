//! Server administration for the SwiftUI app: `host.admin.*` calls the sync server's `/api/admin/*`,
//! what its web console at `/admin` does (`apps/lightcraft-server/src/admin.rs`).
//!
//! - `host.admin.signIn {server, user, password}` — `POST /api/admin/login`; keeps the server, the
//!   admin's token and the user in memory (never on disk). Answers `{user, server}`.
//! - `host.admin.call {method, path: [segment…], body?, server?}` — any other admin route, the path
//!   after `/api/admin/` given as segments (percent-encoded here: the frontend never builds a URL).
//!   Answers the server's JSON (`{}` when it sends none). Without a session only `state` and `setup`
//!   work, against `server`.
//! - `host.admin.signOut` — forget the session (the server is told, best effort).
//! - `host.admin.session` — `{signedIn, server, user}`.
//!
//! The admin session is not the sync device's: the server keeps them apart (a device token gets 401
//! on `/api/admin/*`), so the user types the admin password here. A request runs on a worker thread
//! ([`cloud_sync::run`], the sync driver's transport) and is answered when it ends, so a server that
//! doesn't answer never stalls the engine thread. A `401` on a signed-in call (the server forgets a
//! session after two idle hours) clears the session and answers code `unauthorized`.

use std::sync::mpsc::{Receiver, TryRecvError, channel};

use lightcraft_engine::cloud_sync::{self, Body, Done, Task};
use serde_json::{Value, json};

use super::{Engine, ErrorBody, Event, Sender};

/// Most path segments a call may carry (the deepest admin route has five).
const MAX_SEGMENTS: usize = 8;

/// An admin session.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Login {
    server: String,
    token: String,
    user: String,
}

/// What a request in flight is for (how its answer is read).
#[derive(Debug, PartialEq)]
pub(super) enum Kind {
    SignIn {
        server: String,
        user: String,
    },
    /// `signed_in`: it carries the session's token (a `401` then ends the session).
    Call {
        signed_in: bool,
    },
    SignOut,
}

/// What `host.admin.*` request does.
pub(super) enum Plan {
    /// Answered at once.
    Now(Value),
    /// Sent to the server.
    Send(Kind, Task),
}

/// What an answer does to the session.
#[derive(Debug, PartialEq)]
pub(super) enum Change {
    Keep,
    Set(Login),
    Clear,
}

/// The session and the requests in flight.
#[derive(Default)]
pub(super) struct Admin {
    login: Option<Login>,
    pending: Vec<Pending>,
}

struct Pending {
    id: u64,
    reply: Option<Sender<Event>>,
    kind: Kind,
    rx: Receiver<Done>,
}

fn bad(method: &str, why: &str) -> ErrorBody {
    ErrorBody::new("badParams", format!("`{method}`: {why}"))
}

fn text<'a>(p: &'a Value, key: &str) -> Option<&'a str> {
    p.get(key).and_then(Value::as_str).filter(|s| !s.trim().is_empty())
}

/// A path segment as a URL has it: everything but unreserved characters as `%XX`.
fn encode(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for b in segment.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The server's lower-case error message, as the web console shows it.
fn capitalised(s: &str) -> String {
    let mut chars = s.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// What `method` with `p` means, given the session (`None`: signed out).
pub(super) fn plan(id: u64, method: &str, p: &Value, login: Option<&Login>) -> Result<Plan, ErrorBody> {
    let http = |verb: &'static str, server: &str, path: &str, token: &str, body: Body| Task::Http {
        id,
        method: verb,
        url: format!("{server}/api/admin/{path}"),
        token: token.to_string(),
        body,
        save_to: None,
    };
    match method {
        "host.admin.session" => Ok(Plan::Now(json!({
            "signedIn": login.is_some(),
            "server": login.map(|l| l.server.as_str()),
            "user": login.map(|l| l.user.as_str()),
        }))),
        "host.admin.signIn" => {
            let server = text(p, "server")
                .and_then(cloud_sync::server_address)
                .ok_or_else(|| ErrorBody::new("badParams", "That is not a server address: use one like https://photos.example.com."))?;
            let user = text(p, "user").ok_or_else(|| bad(method, "missing `user`"))?;
            let password = p.get("password").and_then(Value::as_str).filter(|s| !s.is_empty()).ok_or_else(|| bad(method, "missing `password`"))?;
            let body = Body::Json(json!({"user": user, "password": password}).to_string());
            let task = http("POST", &server, "login", "", body);
            Ok(Plan::Send(Kind::SignIn { server, user: user.to_string() }, task))
        }
        "host.admin.signOut" => Ok(match login {
            Some(l) => Plan::Send(Kind::SignOut, http("POST", &l.server, "logout", &l.token, Body::Empty)),
            None => Plan::Now(json!({})),
        }),
        "host.admin.call" => {
            let verb = match text(p, "method") {
                Some("GET") => "GET",
                Some("POST") => "POST",
                Some("PUT") => "PUT",
                Some("DELETE") => "DELETE",
                _ => return Err(bad(method, "`method` is GET, POST, PUT or DELETE")),
            };
            let segments: Vec<&str> = p.get("path").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
            let declared = p.get("path").and_then(Value::as_array).map_or(0, Vec::len);
            if segments.is_empty() || segments.len() != declared || segments.len() > MAX_SEGMENTS {
                return Err(bad(method, "`path` is a list of 1–8 strings"));
            }
            if segments.iter().any(|s| s.is_empty() || *s == "." || *s == ".." || s.len() > 256) {
                return Err(bad(method, "`path` has an empty or dotted segment"));
            }
            let path = segments.iter().map(|s| encode(s)).collect::<Vec<_>>().join("/");
            let body = match p.get("body") {
                None | Some(Value::Null) => Body::Empty,
                Some(b) => Body::Json(b.to_string()),
            };
            let task = match login {
                Some(l) => http(verb, &l.server, &path, &l.token, body),
                None => {
                    // (before signing in the console asks whether this server still needs its setup)
                    if !matches!(segments.first(), Some(&("state" | "setup"))) {
                        return Err(ErrorBody::new("unauthorized", "Sign in as an admin."));
                    }
                    let server = text(p, "server").and_then(cloud_sync::server_address).ok_or_else(|| bad(method, "missing `server`"))?;
                    http(verb, &server, &path, "", body)
                }
            };
            Ok(Plan::Send(Kind::Call { signed_in: login.is_some() }, task))
        }
        other => Err(ErrorBody::new("unknownMethod", format!("`{other}` is not a host.admin method"))),
    }
}

/// The answer to a request, and what it does to the session.
pub(super) fn finish(kind: Kind, done: &Done) -> (Result<Value, ErrorBody>, Change) {
    let why = || match done.status {
        0 => format!("Can't reach the server: {}", done.body),
        s => serde_json::from_str::<Value>(&done.body)
            .ok()
            .and_then(|b| b["error"].as_str().map(capitalised))
            .unwrap_or_else(|| format!("The server answered {s}.")),
    };
    let ok = (200..300).contains(&done.status);
    let parsed = || if done.body.trim().is_empty() { Some(json!({})) } else { serde_json::from_str::<Value>(&done.body).ok() };
    let misread = || ErrorBody::new("other", "The server's answer wasn't understood.");
    match kind {
        Kind::SignOut => (Ok(json!({})), Change::Clear),
        Kind::SignIn { server, user } if ok => {
            let body = parsed().unwrap_or_default();
            match body["token"].as_str().filter(|t| !t.is_empty()) {
                Some(token) => {
                    let user = body["user"].as_str().unwrap_or(&user).to_string();
                    let answer = json!({"user": user, "server": server});
                    (Ok(answer), Change::Set(Login { server, token: token.to_string(), user }))
                }
                None => (Err(misread()), Change::Keep),
            }
        }
        Kind::Call { .. } if ok => (parsed().ok_or_else(misread), Change::Keep),
        Kind::Call { signed_in: true } if done.status == 401 => (Err(ErrorBody::new("unauthorized", why())), Change::Clear),
        _ => (Err(ErrorBody::new("other", why())), Change::Keep),
    }
}

impl Engine {
    /// `host.admin.*`: `None` (answered when the server has) or the answer now.
    pub(super) fn start_admin(&mut self, id: u64, call: Option<&Sender<Event>>, method: &str, p: &Value) -> Option<Event> {
        // (signing out forgets the session before the server has heard of it)
        let login = if method == "host.admin.signOut" { self.admin.login.take() } else { self.admin.login.clone() };
        let (kind, task) = match plan(id, method, p, login.as_ref()) {
            Ok(Plan::Now(v)) => return Some(Event::ok(id, v)),
            Ok(Plan::Send(kind, task)) => (kind, task),
            Err(e) => return Some(Event::err(id, e)),
        };
        let (tx, rx) = channel();
        let wake = self.wake.clone();
        let spawned = std::thread::Builder::new().name("lc-admin".into()).spawn(move || {
            let _ = tx.send(cloud_sync::run(&task));
            let _ = wake.send(super::Msg::Wake);
        });
        if let Err(e) = spawned {
            return Some(Event::err(id, ErrorBody::new("other", format!("can't start a thread for the request: {e}"))));
        }
        self.admin.pending.push(Pending { id, reply: call.cloned(), kind, rx });
        None
    }

    /// Answer the admin requests the server has answered.
    pub(super) fn poll_admin(&mut self) {
        if self.admin.pending.is_empty() {
            return;
        }
        self.busy = true;
        let mut waiting = Vec::new();
        for p in std::mem::take(&mut self.admin.pending) {
            let done = match p.rx.try_recv() {
                Ok(done) => done,
                Err(TryRecvError::Empty) => {
                    waiting.push(p);
                    continue;
                }
                Err(TryRecvError::Disconnected) => Done::failed(p.id, "the request was lost"),
            };
            let (result, change) = finish(p.kind, &done);
            match change {
                Change::Keep => {}
                Change::Set(login) => self.admin.login = Some(login),
                Change::Clear => self.admin.login = None,
            }
            let event = match result {
                Ok(v) => Event::ok(p.id, v),
                Err(e) => Event::err(p.id, e),
            };
            Self::reply(&self.events, p.reply, event);
        }
        self.admin.pending = waiting;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn login() -> Login {
        Login { server: "http://nas:8080".into(), token: "tok".into(), user: "ann".into() }
    }

    fn sent(plan: Result<Plan, ErrorBody>) -> (Kind, Task) {
        match plan {
            Ok(Plan::Send(kind, task)) => (kind, task),
            Ok(Plan::Now(v)) => panic!("answered at once: {v}"),
            Err(e) => panic!("refused: {e:?}"),
        }
    }

    fn url_and_token(task: &Task) -> (&str, &str) {
        match task {
            Task::Http { url, token, .. } => (url, token),
            other => panic!("not an HTTP task: {other:?}"),
        }
    }

    // A path is segments, each percent-encoded: a folder called "My Photos/2" can't change the route
    #[test]
    fn path_segments_are_encoded() {
        let p = json!({"method": "PUT", "path": ["users", "ann", "folders", "My Photos/2"], "body": {"writable": true}});
        let (kind, task) = sent(plan(7, "host.admin.call", &p, Some(&login())));
        assert_eq!(kind, Kind::Call { signed_in: true });
        assert_eq!(url_and_token(&task), ("http://nas:8080/api/admin/users/ann/folders/My%20Photos%2F2", "tok"));
        assert!(matches!(task, Task::Http { method: "PUT", body: Body::Json(ref b), .. } if b == r#"{"writable":true}"#));
    }

    // The Limits page saves with `PUT limits`: `null` (automatic) stays in the body, so the server clears that limit
    #[test]
    fn limits_are_saved_with_nulls_kept() {
        let p = json!({"method": "PUT", "path": ["limits"], "body": {"threads": null, "previews": 3}});
        let (kind, task) = sent(plan(5, "host.admin.call", &p, Some(&login())));
        assert_eq!(kind, Kind::Call { signed_in: true });
        assert_eq!(url_and_token(&task), ("http://nas:8080/api/admin/limits", "tok"));
        let Task::Http { method: "PUT", body: Body::Json(body), .. } = task else { panic!("not a PUT with a JSON body") };
        assert_eq!(serde_json::from_str::<Value>(&body).ok(), Some(json!({"threads": null, "previews": 3})));
        // the server's own refusal (a number over the machine's maximum) reaches the page as its words
        let over = Done { id: 5, status: 400, body: r#"{"error":"previews must be from 1 to 64"}"#.into() };
        let (r, change) = finish(Kind::Call { signed_in: true }, &over);
        assert_eq!(r.err().map(|e| e.message), Some("Previews must be from 1 to 64".into()));
        assert_eq!(change, Change::Keep);
    }

    // Dotted, empty, missing and too many segments, and unknown verbs, are refused before anything is sent
    #[test]
    fn bad_calls_are_refused() {
        let refused = |p: Value| plan(1, "host.admin.call", &p, Some(&login())).err().map(|e| e.code);
        assert_eq!(refused(json!({"method": "GET", "path": ["users", ".."]})), Some("badParams".into()));
        assert_eq!(refused(json!({"method": "GET", "path": ["users", ""]})), Some("badParams".into()));
        assert_eq!(refused(json!({"method": "GET", "path": []})), Some("badParams".into()));
        assert_eq!(refused(json!({"method": "GET", "path": ["users", 3]})), Some("badParams".into()));
        assert_eq!(refused(json!({"method": "GET", "path": vec!["a"; 9]})), Some("badParams".into()));
        assert_eq!(refused(json!({"method": "PATCH", "path": ["users"]})), Some("badParams".into()));
    }

    // Signed out, only the first-run questions work, against the server named; nothing else is sent
    #[test]
    fn signed_out_only_state_and_setup_are_asked() {
        let state = json!({"method": "GET", "path": ["state"], "server": "nas.local:8080"});
        let (kind, task) = sent(plan(1, "host.admin.call", &state, None));
        assert_eq!(kind, Kind::Call { signed_in: false });
        assert_eq!(url_and_token(&task), ("http://nas.local:8080/api/admin/state", ""));
        let users = json!({"method": "GET", "path": ["users"], "server": "nas.local:8080"});
        assert_eq!(plan(2, "host.admin.call", &users, None).err().map(|e| e.code), Some("unauthorized".into()));
        let no_server = json!({"method": "GET", "path": ["state"]});
        assert_eq!(plan(3, "host.admin.call", &no_server, None).err().map(|e| e.code), Some("badParams".into()));
    }

    // Signing in checks its parameters; the answer keeps the session and says who and where
    #[test]
    fn signing_in_keeps_the_session() {
        let missing = json!({"server": "nas:8080", "user": "ann"});
        assert_eq!(plan(1, "host.admin.signIn", &missing, None).err().map(|e| e.code), Some("badParams".into()));
        let p = json!({"server": "NAS:8080/", "user": "ann", "password": "secret!!"});
        let (kind, task) = sent(plan(1, "host.admin.signIn", &p, None));
        assert_eq!(url_and_token(&task).0, "http://nas:8080/api/admin/login");
        let done = Done { id: 1, status: 200, body: r#"{"token":"abc","user":"Ann"}"#.into() };
        let (result, change) = finish(kind, &done);
        assert_eq!(result.ok(), Some(json!({"user": "Ann", "server": "http://nas:8080"})));
        assert_eq!(change, Change::Set(Login { server: "http://nas:8080".into(), token: "abc".into(), user: "Ann".into() }));
    }

    // A wrong password is the server's own words and keeps nothing; a signed-in 401 ends the session
    #[test]
    fn errors_say_what_the_server_said() {
        let wrong = Done { id: 1, status: 401, body: r#"{"error":"wrong user name or password, or not an admin"}"#.into() };
        let (r, change) = finish(Kind::SignIn { server: "http://nas".into(), user: "ann".into() }, &wrong);
        let e = r.err().map(|e| (e.code, e.message));
        assert_eq!(e, Some(("other".into(), "Wrong user name or password, or not an admin".into())));
        assert_eq!(change, Change::Keep);
        let ended = Done { id: 2, status: 401, body: r#"{"error":"sign in as an admin"}"#.into() };
        let (r, change) = finish(Kind::Call { signed_in: true }, &ended);
        assert_eq!(r.err().map(|e| e.code), Some("unauthorized".into()));
        assert_eq!(change, Change::Clear);
        let (r, change) = finish(Kind::Call { signed_in: false }, &ended);
        assert_eq!(r.err().map(|e| e.code), Some("other".into()));
        assert_eq!(change, Change::Keep);
        let down = Done::failed(3, "Connection refused");
        let (r, _) = finish(Kind::Call { signed_in: true }, &down);
        assert_eq!(r.err().map(|e| e.message), Some("Can't reach the server: Connection refused".into()));
        let odd = Done { id: 4, status: 502, body: "<html>".into() };
        let (r, _) = finish(Kind::Call { signed_in: true }, &odd);
        assert_eq!(r.err().map(|e| e.message), Some("The server answered 502.".into()));
    }

    // Empty answers are `{}`; an answer that isn't JSON is an error, not a panic
    #[test]
    fn answers_are_json() {
        let empty = Done { id: 1, status: 200, body: String::new() };
        assert_eq!(finish(Kind::Call { signed_in: true }, &empty).0.ok(), Some(json!({})));
        let junk = Done { id: 1, status: 200, body: "not json".into() };
        assert!(finish(Kind::Call { signed_in: true }, &junk).0.is_err());
        let no_token = Done { id: 1, status: 200, body: "{}".into() };
        assert!(finish(Kind::SignIn { server: "http://nas".into(), user: "a".into() }, &no_token).0.is_err());
    }
}
