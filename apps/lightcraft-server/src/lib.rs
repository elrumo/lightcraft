//! LightCraft sync server: each user's library — photos, edits, albums, presets — shared by
//! their devices (the desktop app, the web build it serves at `/`, later iOS). Self-hosted, one
//! binary; see `docs/sync.md`.
//!
//! The server never decodes a photo. It keeps the single order of every device's changes
//! ([`lightcraft_catalog::sync::ServerCore`]: each push is validated by applying it to the
//! user's catalog, all or nothing), the photo files by content hash (originals and the smart and
//! mini previews devices build), the presets document, and who may sign in. Plain HTTP: put it
//! behind a TLS reverse proxy (Caddy) or a private network (Tailscale).
//!
//! ```text
//! <data>/users.json                       users: argon2 password hash, library id
//! <data>/users/<name>/library/            the op log (catalog.log)
//! <data>/users/<name>/blobs/<kind>/<xx>/<hash>   original | smart | mini
//! <data>/users/<name>/presets.json        { version, presets }
//! <data>/users/<name>/devices.json        signed-in devices (token hashes, id spaces)
//! ```
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod accounts;
pub mod api;
pub mod gc;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// The largest id space handed out (ids stay below 2^53: exact in JavaScript).
pub const MAX_SPACE: u32 = (1 << 21) - 1;

pub struct Config {
    /// Where everything is kept.
    pub data: PathBuf,
    /// `host:port` to listen on.
    pub listen: String,
    /// The web build to serve at `/` (`cargo xtask web` → `target/web/`).
    pub web: Option<PathBuf>,
    /// Requests served at once (more get `503`).
    pub max_requests: usize,
}

/// What every request thread shares.
pub struct State {
    pub(crate) data: PathBuf,
    pub(crate) web: Option<PathBuf>,
    pub(crate) accounts: Mutex<accounts::Accounts>,
    pub(crate) libs: Mutex<HashMap<String, Arc<Mutex<api::UserLib>>>>,
    busy: AtomicUsize,
    max: usize,
}

/// A running server.
pub struct Server {
    http: Arc<tiny_http::Server>,
    addr: SocketAddr,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Server {
    /// Listen and serve on a background thread.
    pub fn start(cfg: Config) -> Result<Server, String> {
        std::fs::create_dir_all(&cfg.data).map_err(|e| format!("{}: {e}", cfg.data.display()))?;
        let http = Arc::new(tiny_http::Server::http(&cfg.listen).map_err(|e| format!("can't listen on {}: {e}", cfg.listen))?);
        let addr = http.server_addr().to_ip().ok_or("not listening on an IP address")?;
        let state = Arc::new(State {
            accounts: Mutex::new(accounts::Accounts::new(&cfg.data)),
            data: cfg.data,
            web: cfg.web,
            libs: Mutex::new(HashMap::new()),
            busy: AtomicUsize::new(0),
            max: cfg.max_requests.max(1),
        });
        let h = http.clone();
        let thread = std::thread::Builder::new().name("lc-accept".into()).spawn(move || accept(&h, &state)).map_err(|e| e.to_string())?;
        Ok(Server { http, addr, thread: Some(thread) })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Serve until the process ends.
    pub fn wait(mut self) {
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.http.unblock();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn accept(http: &tiny_http::Server, state: &Arc<State>) {
    for req in http.incoming_requests() {
        if state.busy.fetch_add(1, Ordering::SeqCst) >= state.max {
            state.busy.fetch_sub(1, Ordering::SeqCst);
            let _ = req.respond(tiny_http::Response::from_string("busy, try again").with_status_code(503));
            continue;
        }
        let st = state.clone();
        let spawned = std::thread::Builder::new().name("lc-request".into()).spawn(move || {
            // a panic in one request is that request's failure (its connection drops)
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| api::handle(&st, req))).is_err() {
                log::error!("a request failed unexpectedly");
            }
            st.busy.fetch_sub(1, Ordering::SeqCst);
        });
        if let Err(e) = spawned {
            log::error!("can't start a request thread: {e}");
            state.busy.fetch_sub(1, Ordering::SeqCst);
        }
    }
}
