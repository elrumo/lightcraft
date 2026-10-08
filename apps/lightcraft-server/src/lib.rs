//! LightCraft sync server: each user's library — photos, edits, albums, presets — shared by
//! their devices (the desktop app, the web build it serves at `/`, later iOS). Self-hosted, one
//! binary; see `docs/sync.md`.
//!
//! The server decodes photos only to build previews of library folders' photos and to search by
//! description ([`vision`]). It keeps the single order of every device's changes
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
//! <data>/users/<name>/folders.json        the user's library folders as last scanned
//! ```
//!
//! **Library folders** ([`folders`]): photo folders already on the server, given to a user by an
//! admin, are read in place — never copied, moved or written — and their photos join the user's
//! library like any other (the server builds their previews and serves their originals).
//!
//! Users, devices and library folders are managed from the command line (`lightcraft-server
//! user …`, `folder …`) or the admin page at `/admin` ([`admin`]).
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod accounts;
pub mod admin;
pub mod api;
pub mod folders;
pub mod gc;
pub mod throttle;
pub mod vision;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// The largest id space (ids stay below 2^53: exact in JavaScript). Devices get the ones below
/// it; it is the server's own ([`folders::SPACE`]).
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
    /// Scan every user's library folders this often (`None`: at start and on demand only).
    pub scan_interval: Option<std::time::Duration>,
    /// Threads building previews of photos found in library folders.
    pub preview_threads: usize,
    /// Where the search model's files are (default `<data>/models/siglip2`).
    pub vision_dir: Option<PathBuf>,
    /// A model to search with instead of SigLIP 2 from `vision_dir` (tests).
    pub embedder: Option<Arc<dyn lightcraft_vision::Embedder>>,
}

impl Config {
    /// Serve `data` on `listen`: no web build, the default limits, library folders scanned every
    /// 15 minutes.
    pub fn new(data: impl Into<PathBuf>, listen: impl Into<String>) -> Config {
        Config {
            data: data.into(),
            listen: listen.into(),
            web: None,
            max_requests: 64,
            scan_interval: Some(std::time::Duration::from_secs(15 * 60)),
            preview_threads: 2,
            vision_dir: None,
            embedder: None,
        }
    }
}

/// What every request thread shares.
pub struct State {
    pub(crate) data: PathBuf,
    pub(crate) web: Option<PathBuf>,
    /// The address served (shown on the admin page).
    pub(crate) listen: String,
    /// The one-time code that creates the first admin from `/admin` (while there is none).
    pub(crate) setup: Mutex<Option<String>>,
    pub(crate) setup_tries: AtomicU32,
    pub(crate) accounts: Mutex<accounts::Accounts>,
    pub(crate) libs: Mutex<HashMap<String, Arc<Mutex<api::UserLib>>>>,
    /// Library folder scans and preview building.
    pub(crate) folders: folders::Scanner,
    /// Search by description: the model and each user's index.
    pub(crate) vision: vision::Search,
    /// Failed sign-ins (password guessing).
    pub(crate) throttle: Mutex<throttle::Throttle>,
    busy: AtomicUsize,
    max: usize,
}

/// A running server.
pub struct Server {
    http: Arc<tiny_http::Server>,
    addr: SocketAddr,
    thread: Option<std::thread::JoinHandle<()>>,
    /// The library folder scanner and preview builders.
    workers: Vec<std::thread::JoinHandle<()>>,
    state: Arc<State>,
}

impl Server {
    /// Listen and serve on a background thread.
    pub fn start(cfg: Config) -> Result<Server, String> {
        std::fs::create_dir_all(&cfg.data).map_err(|e| format!("{}: {e}", cfg.data.display()))?;
        let http = Arc::new(tiny_http::Server::http(&cfg.listen).map_err(|e| format!("can't listen on {}: {e}", cfg.listen))?);
        let addr = http.server_addr().to_ip().ok_or("not listening on an IP address")?;
        let setup = if accounts::has_admin(&cfg.data) { None } else { admin::new_setup_code() };
        if let Some(code) = &setup {
            let at = if addr.ip().is_unspecified() { format!("http://<this server>:{}", addr.port()) } else { format!("http://{addr}") };
            log::warn!("no admin yet: open {at}/admin (or your domain's /admin) and enter the setup code {code}");
        }
        let vision_dir = cfg.vision_dir.clone().unwrap_or_else(|| cfg.data.join("models").join("siglip2"));
        let state = Arc::new(State {
            vision: vision::Search::new(vision_dir, cfg.embedder),
            accounts: Mutex::new(accounts::Accounts::new(&cfg.data)),
            listen: addr.to_string(),
            setup: Mutex::new(setup),
            setup_tries: AtomicU32::new(0),
            data: cfg.data,
            web: cfg.web,
            libs: Mutex::new(HashMap::new()),
            folders: folders::Scanner::new(cfg.scan_interval),
            throttle: Mutex::new(throttle::Throttle::default()),
            busy: AtomicUsize::new(0),
            max: cfg.max_requests.max(1),
        });
        let (h, s) = (http.clone(), state.clone());
        let thread = std::thread::Builder::new().name("lc-accept".into()).spawn(move || accept(&h, &s)).map_err(|e| e.to_string())?;
        let mut workers = Vec::new();
        let s = state.clone();
        workers.push(std::thread::Builder::new().name("lc-scan".into()).spawn(move || folders::run_scanner(&s)).map_err(|e| e.to_string())?);
        let s = state.clone();
        workers.push(std::thread::Builder::new().name("lc-vision".into()).spawn(move || vision::run(&s)).map_err(|e| e.to_string())?);
        for i in 0..cfg.preview_threads.max(1) {
            let s = state.clone();
            let t = std::thread::Builder::new().name(format!("lc-previews-{i}")).spawn(move || folders::run_previews(&s));
            workers.push(t.map_err(|e| e.to_string())?);
        }
        Ok(Server { http, addr, thread: Some(thread), workers, state })
    }

    /// The first-run setup code, while the server has no admin (tests; it is also logged).
    pub fn setup_code(&self) -> Option<String> {
        self.state.setup.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// What the last (or current) scan of a user's library folders did (tests, the admin page).
    pub fn folder_status(&self, user: &str) -> folders::Status {
        self.state.folders.status(user)
    }

    /// Scan a user's library folders soon.
    pub fn scan(&self, user: &str) {
        self.state.folders.request(user);
    }

    /// Look for photos to index for search now (tests; it happens by itself as previews arrive).
    pub fn index_for_search(&self) {
        self.state.vision.wake();
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
        self.state.folders.shut_down();
        self.state.vision.shut_down();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        for t in self.workers.drain(..) {
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
