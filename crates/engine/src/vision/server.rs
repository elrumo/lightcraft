//! Search by description through the sync server: a device that can't run the model (the web
//! build, iOS) sends the sentence and gets photo ids back, a desktop that can run it may send the
//! server the vectors it computed (so nothing is computed twice), and the app learns whether the
//! server can search at all. All of it rides the sync transport: requests are queued here, any
//! host runs them as it runs the library's own (`Session::sync_tasks`), and the answers come back
//! through `Session::sync_done`. Without a host loop (CLI, MCP, tests) `wait` drives them inline.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use lightcraft_catalog::PhotoId;
use lightcraft_vision::Key;
use serde_json::{Value, json};

use super::{IndexSpec, with_index};
use crate::Session;
use crate::sync::{Body, Done, SyncState};

/// Vectors in one upload (`records × (16 + 2 × dim)` bytes: ~6 MB for SigLIP 2).
pub const SHARE_CHUNK: usize = 4000;
/// How long the server's answer to the status question is trusted.
const STATUS_TTL: Duration = Duration::from_secs(60);
/// Automatic sharing waits at least this long between runs.
const SHARE_EVERY: Duration = Duration::from_secs(60);

/// A request to the server beside the library's own sync, and what its answer is for.
#[derive(Debug)]
pub(crate) enum Aux {
    /// `GET /api/search/status`: can this server search, and how far along is it?
    Status,
    /// `GET /api/search?q=…` for the search numbered `seq`.
    Search { seq: u64, query: String },
    /// `GET /api/index/embeddings/keys`: what the server has, so only the rest is sent.
    Keys,
    /// `POST /api/index/embeddings`: one piece of the local index (`path`: the file sent).
    Upload { path: PathBuf, keys: Vec<Key> },
}

/// A search the server answered, on its way to the view.
pub(crate) struct ServerHits {
    pub seq: u64,
    pub query: String,
    pub result: Result<Vec<(PhotoId, f32)>, String>,
}

/// Sending the local index to the server.
#[derive(Default)]
pub(crate) struct Share {
    pub running: bool,
    spec: Option<IndexSpec>,
    have: HashSet<Key>,
    /// Vectors sent so far in this run.
    pub sent: usize,
    pub error: Option<String>,
    /// The index's size when this run started (what automatic sharing compares with).
    started_len: usize,
}

/// What this device knows about the server's search.
#[derive(Default)]
pub(crate) struct Remote {
    /// The server's answer to `/api/search/status` (`None`: not asked or unreachable).
    pub status: Option<Value>,
    status_at: Option<Instant>,
    probing: bool,
    pub arrived: Vec<ServerHits>,
    /// The newest search that was sent to the server and hasn't been answered.
    pub pending_seq: Option<u64>,
    pub share: Share,
    /// The index size that was last sent completely.
    shared_len: usize,
    share_at: Option<Instant>,
}

impl Remote {
    /// The server can search by description (whether or not its model is installed yet).
    pub fn supported(&self) -> bool {
        self.status.as_ref().and_then(|s| s["available"].as_bool()) == Some(true)
    }

    /// The server can search now.
    pub fn ready(&self) -> bool {
        self.supported() && self.status.as_ref().and_then(|s| s["installed"].as_bool()) == Some(true)
    }

    /// Takes `status` as the server's answer (tests of the UI).
    pub fn set_status(&mut self, status: Option<Value>) {
        self.status = status;
        self.status_at = Some(Instant::now());
    }

    pub fn share_json(&self) -> Value {
        json!({"running": self.share.running, "sent": self.share.sent, "error": self.share.error})
    }
}

/// `%xx`-escapes everything but unreserved characters (a query string value).
pub(crate) fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn why(d: &Done) -> String {
    let said = serde_json::from_str::<Value>(&d.body).ok().and_then(|v| v["error"].as_str().map(str::to_string));
    match (d.status, said) {
        (0, _) => format!("can't reach the server: {}", d.body),
        (404, _) => "this server is too old to search by description: update it".into(),
        (_, Some(e)) => e,
        (s, None) => format!("the server answered {s}"),
    }
}

impl Session {
    /// Whether this library is signed in to a server (the token is there).
    pub(super) fn vision_signed_in(&self) -> bool {
        self.sync.as_ref().is_some_and(|st| !st.config.token.is_empty())
    }

    /// Queue a request beside the library's own sync. It goes out with the next
    /// [`Session::sync_tasks`]; its answer reaches [`Session::vision_server_done`].
    fn vision_aux(&mut self, method: &'static str, path: &str, body: Body, aux: Aux) -> Result<(), String> {
        let st = self.sync.as_mut().filter(|st| !st.config.token.is_empty()).ok_or("this library isn't signed in to a server")?;
        queue(st, method, path, body, aux);
        Ok(())
    }

    /// Runs what is queued, here and now (native hosts without a frame loop: CLI, MCP, tests).
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn vision_drive(&mut self) {
        // (each round runs what the last one queued: uploads follow one another)
        for _ in 0..10_000 {
            let tasks: Vec<_> = self.sync.as_mut().map(|st| st.aux_ready.drain(..).collect()).unwrap_or_default();
            if tasks.is_empty() {
                break;
            }
            for t in tasks {
                let done = crate::sync::run(&t);
                self.sync_done(done);
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn vision_drive(&mut self) {}

    /// Ask the server whether it can search (when this hasn't been asked lately).
    pub fn vision_probe_server(&mut self) {
        if !self.vision_signed_in() || self.vision.remote.probing {
            return;
        }
        if self.vision.remote.status_at.is_some_and(|t| t.elapsed() < STATUS_TTL) {
            return;
        }
        if self.vision_aux("GET", "/api/search/status", Body::Empty, Aux::Status).is_ok() {
            self.vision.remote.probing = true;
        }
    }

    /// Ask the server for the best matches of `query`: the view is set when the answer arrives
    /// ([`Session::vision_poll`]), or here with `wait`.
    pub(crate) fn vision_server_search(&mut self, query: &str, limit: usize, wait: bool) -> Result<serde_json::Value, String> {
        self.vision.search_seq += 1;
        let seq = self.vision.search_seq;
        let path = format!("/api/search?q={}&limit={limit}", percent_encode(query));
        self.vision_aux("GET", &path, Body::Empty, Aux::Search { seq, query: query.to_string() })?;
        self.vision.remote.pending_seq = Some(seq);
        if !wait && self.vision.background {
            return Ok(json!({"query": query, "status": "searching", "source": "server"}));
        }
        // no frame loop to run it (CLI, MCP, tests): here and now
        self.vision_drive();
        let hits = std::mem::take(&mut self.vision.remote.arrived);
        let Some(done) = hits.into_iter().find(|h| h.seq == seq) else {
            return Err("the server didn't answer".into());
        };
        let found = done.result?;
        Ok(self.apply_ids(&done.query, &found, "server"))
    }

    /// Send the server the vectors it doesn't have (`vision.share`). Only what the server's
    /// library holds is kept, and only for the same model. With `wait` this returns when done.
    pub fn vision_share(&mut self, wait: bool) -> Result<Value, String> {
        if self.vision.remote.share.running {
            return Err("the search data is already being sent".into());
        }
        let (model, dim) = self.vision.spec_only()?;
        let spec = IndexSpec { path: self.vision_index_path(&model), model, dim };
        let len = with_index(&self.vision.shared, &spec, |ix| ix.len())?;
        if len == 0 {
            return Err("nothing to send yet: index the photos first (`vision.index`)".into());
        }
        let share = &mut self.vision.remote.share;
        *share = Share { running: true, spec: Some(spec), started_len: len, ..Default::default() };
        if let Err(e) = self.vision_aux("GET", "/api/index/embeddings/keys", Body::Empty, Aux::Keys) {
            self.vision.remote.share.running = false;
            return Err(e);
        }
        if wait {
            self.vision_drive();
        }
        Ok(self.vision.remote.share_json())
    }

    /// An answer to a request queued by [`Session::vision_aux`].
    pub(crate) fn vision_server_done(&mut self, st: &mut SyncState, aux: Aux, d: &Done) {
        match aux {
            Aux::Status => {
                let r = &mut self.vision.remote;
                r.probing = false;
                r.status_at = Some(Instant::now());
                r.status = if d.ok() {
                    serde_json::from_str(&d.body).ok()
                } else if d.status == 404 {
                    // a server from before search: it can't
                    Some(json!({"available": false}))
                } else {
                    None
                };
            }
            Aux::Search { seq, query } => {
                if self.vision.remote.pending_seq == Some(seq) {
                    self.vision.remote.pending_seq = None;
                }
                let result = if d.ok() { self.server_hits(&d.body) } else { Err(why(d)) };
                self.vision.remote.arrived.push(ServerHits { seq, query, result });
            }
            Aux::Keys => self.share_keys(st, d),
            Aux::Upload { path, keys } => {
                let _ = std::fs::remove_file(&path);
                if !d.ok() {
                    self.share_failed(why(d));
                    return;
                }
                let r = &mut self.vision.remote;
                r.share.sent += keys.len();
                // (what the server skipped counts as handled too: it would be skipped again)
                r.share.have.extend(keys);
                self.share_next(st);
            }
        }
    }

    /// The server's hits as photos this device has.
    fn server_hits(&self, body: &str) -> Result<Vec<(PhotoId, f32)>, String> {
        let v: Value = serde_json::from_str(body).map_err(|e| format!("the server's answer isn't what was expected: {e}"))?;
        let ids = v["ids"].as_array().ok_or("the server's answer has no photos")?;
        let scores = v["scores"].as_array().map(Vec::as_slice).unwrap_or_default();
        Ok(ids
            .iter()
            .enumerate()
            .filter_map(|(i, id)| {
                let id = PhotoId(id.as_u64()?);
                // (a photo another device added that this one hasn't pulled yet isn't shown)
                self.catalog.photo(id)?;
                Some((id, scores.get(i).and_then(Value::as_f64).unwrap_or(0.0) as f32))
            })
            .collect())
    }

    fn share_failed(&mut self, e: String) {
        log::warn!("sending the search data: {e}");
        let share = &mut self.vision.remote.share;
        share.running = false;
        share.error = Some(e);
    }

    /// The server's keys arrived: send what it lacks.
    fn share_keys(&mut self, st: &mut SyncState, d: &Done) {
        if !d.ok() {
            return self.share_failed(why(d));
        }
        let Ok(v) = serde_json::from_str::<Value>(&d.body) else { return self.share_failed("the server's answer isn't what was expected".into()) };
        let Some(spec) = self.vision.remote.share.spec.clone() else { return };
        let (model, dim) = (v["model"].as_str().unwrap_or_default(), v["dim"].as_u64().unwrap_or(0) as usize);
        if model != spec.model || dim != spec.dim {
            return self.share_failed(format!("the server searches with {model}, not {}: nothing was sent", spec.model));
        }
        self.vision.remote.share.have = v["keys"].as_array().into_iter().flatten().filter_map(|k| k.as_str().and_then(Key::from_hex)).collect();
        self.share_next(st);
    }

    /// Sends the next piece of the local index, or ends the run.
    fn share_next(&mut self, st: &mut SyncState) {
        let Some(spec) = self.vision.remote.share.spec.clone() else { return };
        let have = std::mem::take(&mut self.vision.remote.share.have);
        let exported = with_index(&self.vision.shared, &spec, |ix| {
            let bytes = ix.export(|k| have.contains(k), SHARE_CHUNK);
            // the keys in it, to count as handled once sent
            let rec = lightcraft_vision::store::KEY_LEN + 2 * ix.dim();
            let keys: Vec<Key> =
                bytes.get(lightcraft_vision::store::HEADER_LEN..).unwrap_or_default().chunks_exact(rec).filter_map(Key::read).collect();
            (bytes, keys)
        });
        self.vision.remote.share.have = have;
        let (bytes, keys) = match exported {
            Ok(x) => x,
            Err(e) => return self.share_failed(e),
        };
        if keys.is_empty() {
            let r = &mut self.vision.remote;
            r.share.running = false;
            r.shared_len = r.share.started_len;
            return;
        }
        let dir = self.library.as_ref().filter(|l| l.on_disk).map_or_else(std::env::temp_dir, |l| l.dir.join("search"));
        let path = dir.join(format!("sending-{}.part", crate::vision::next_id()));
        if let Err(e) = std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, &bytes)) {
            return self.share_failed(format!("{}: {e}", path.display()));
        }
        queue(st, "POST", "/api/index/embeddings", Body::File(path.to_string_lossy().into_owned()), Aux::Upload { path, keys });
    }

    /// Frame-loop upkeep for the server side: learn whether the server can search, apply the
    /// answers that arrived, and send the local vectors when the user asked for that.
    pub(crate) fn vision_server_poll(&mut self, out: &mut super::VisionPolled) {
        if self.vision_signed_in() {
            self.vision_probe_server();
        }
        for done in std::mem::take(&mut self.vision.remote.arrived) {
            // only the newest search counts: an older one is already out of date
            if done.seq != self.vision.search_seq {
                continue;
            }
            match done.result {
                Ok(hits) => {
                    self.apply_ids(&done.query, &hits, "server");
                    out.changed = true;
                }
                Err(e) => out.messages.push(e),
            }
        }
        let r = &self.vision.remote;
        let due = self.vision.share_with_server
            && self.vision_signed_in()
            && r.supported()
            && !r.share.running
            && self.vision.indexed() > r.shared_len
            && r.share_at.is_none_or(|t| t.elapsed() >= SHARE_EVERY);
        if due {
            self.vision.remote.share_at = Some(Instant::now());
            if let Err(e) = self.vision_share(false) {
                log::debug!("sending the search data: {e}");
            }
        }
    }
}

/// Queues `aux`'s request to go out with the next [`Session::sync_tasks`].
fn queue(st: &mut SyncState, method: &'static str, path: &str, body: Body, aux: Aux) {
    let task = st.http(method, path, body, None);
    st.aux.insert(task.id(), aux);
    st.aux_ready.push_back(task);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queries_are_escaped_for_a_url() {
        assert_eq!(percent_encode("a dog on a beach"), "a%20dog%20on%20a%20beach");
        assert_eq!(percent_encode("猫 & dog?=#"), "%E7%8C%AB%20%26%20dog%3F%3D%23");
        assert_eq!(percent_encode("plain-text_1.~"), "plain-text_1.~");
        assert_eq!(percent_encode(""), "");
    }

    #[test]
    fn an_answer_is_explained_in_words() {
        let d = |status: u16, body: &str| Done { id: 1, status, body: body.into() };
        assert!(why(&d(0, "connection refused")).contains("can't reach the server"));
        assert!(why(&d(404, "{}")).contains("too old"));
        assert_eq!(
            why(&d(503, r#"{"error":"the search model is not installed on this server"}"#)),
            "the search model is not installed on this server"
        );
        assert_eq!(why(&d(500, "oops")), "the server answered 500");
    }

    #[test]
    fn a_server_that_cannot_search_is_not_ready() {
        let mut r = Remote::default();
        assert!(!r.supported() && !r.ready());
        r.status = Some(json!({"available": true, "installed": false}));
        assert!(r.supported() && !r.ready());
        r.status = Some(json!({"available": true, "installed": true}));
        assert!(r.supported() && r.ready());
        r.status = Some(json!({"available": false}));
        assert!(!r.supported() && !r.ready());
    }
}
