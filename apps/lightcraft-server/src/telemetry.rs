//! Anonymous, optional server telemetry. No account or device identifiers are sent.
//!
//! Two parts, both switched off by `LIGHTBITES_TELEMETRY=0` (and both off in a build without a Bugsink DSN, which is
//! how a test or a build from source stays quiet):
//! - Bugsink gets an install and a start event and the status class of every HTTP 5xx ([`start`], [`server_error`]).
//! - lightbites.app gets a heartbeat ([`run`]): a random install id the server makes and keeps in its data folder, the
//!   version and platform, how long it has been up, and a few coarse counts and size buckets. Never a name, a path, a
//!   photo or an address; the site stores it as `/admin` ▸ Servers (`apps/lightbites-web/server/utils/serverTelemetry.ts`
//!   holds the allow-list that decides what is kept).

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::{State, accounts};

/// Where heartbeats go unless `LIGHTBITES_TELEMETRY_URL` says otherwise.
const URL: &str = "https://lightbites.app/api/telemetry/server";

/// The first heartbeat waits this long (counting the libraries is work the server's start should not wait for), the
/// others come this far apart.
const FIRST: Duration = Duration::from_secs(60);
const EVERY: Duration = Duration::from_secs(6 * 3600);

/// HTTP 5xx answers since the last heartbeat.
static ERRORS: AtomicU64 = AtomicU64::new(0);

fn opted_out() -> bool {
    std::env::var("LIGHTBITES_TELEMETRY").is_ok_and(|value| matches!(value.as_str(), "0" | "false" | "off"))
}

fn dsn() -> Option<String> {
    std::env::var("LIGHTBITES_BUGSINK_DSN")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| option_env!("LIGHTBITES_BUGSINK_DSN").filter(|value| !value.is_empty()).map(str::to_owned))
}

/// Where to send heartbeats: `None` when the owner turned telemetry off or this isn't an official build.
pub fn endpoint() -> Option<String> {
    if opted_out() {
        return None;
    }
    std::env::var("LIGHTBITES_TELEMETRY_URL").ok().filter(|value| !value.is_empty()).or_else(|| dsn().map(|_| URL.to_string()))
}

/// Starts Bugsink reporting for the lifetime of the server process and reports aggregate usage.
pub fn start(data: &Path) -> Option<sentry::ClientInitGuard> {
    if opted_out() {
        return None;
    }
    let dsn = dsn()?.parse::<sentry::types::Dsn>().ok()?;
    let mut options = sentry::ClientOptions::new();
    options.dsn = Some(dsn);
    options.release = Some(format!("lightbites-server@{}", env!("CARGO_PKG_VERSION")).into());
    options.send_default_pii = false;
    options.default_integrations = false;
    let guard = sentry::init(options);

    let install_marker = data.join(".lightbites-telemetry-installed");
    if let Ok(file) = std::fs::OpenOptions::new().write(true).create_new(true).open(install_marker) {
        drop(file);
        sentry::with_scope(
            |scope| scope.set_fingerprint(Some(&["lightbites", "server-install"])),
            || sentry::capture_message("server install", sentry::Level::Info),
        );
    }

    if let Ok(users) = accounts::read_users(data) {
        let device_count =
            users.users.keys().fold(0usize, |total, user| total.saturating_add(accounts::devices(data, user).map_or(0, |file| file.devices.len())));
        sentry::with_scope(
            |scope| {
                scope.set_fingerprint(Some(&["lightbites", "server-start"]));
                scope.set_tag("users", users.users.len());
                scope.set_tag("devices", device_count);
            },
            || sentry::capture_message("server start", sentry::Level::Info),
        );
    }
    Some(guard)
}

/// Reports only the status class; the underlying error may contain a path or user supplied text.
pub fn server_error(status: u16) {
    if status >= 500 {
        ERRORS.fetch_add(1, Ordering::Relaxed);
        sentry::with_scope(
            |scope| {
                scope.set_fingerprint(Some(&["lightbites", "server-error"]));
                scope.set_tag("http.status", status);
            },
            || sentry::capture_message("server request failed", sentry::Level::Error),
        );
    }
}

/// The install's random id, made the first time and kept in `<data>/.lightbites-telemetry-id` (delete the file for a
/// new one). `None` when the folder can't be written: no heartbeat then, never a made-up id that changes every start.
pub(crate) fn install_id(data: &Path) -> Option<String> {
    let path = data.join(".lightbites-telemetry-id");
    if let Some(have) = std::fs::read_to_string(&path).ok().map(|s| s.trim().to_string()).filter(|s| is_uuid(s)) {
        return Some(have);
    }
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).ok()?;
    // (a version 4 UUID: the site accepts nothing else)
    b[6] = b[6] & 0x0f | 0x40;
    b[8] = b[8] & 0x3f | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    let id = format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32]);
    std::fs::write(path, &id).ok()?;
    Some(id)
}

fn is_uuid(s: &str) -> bool {
    s.len() == 36 && s.char_indices().all(|(i, c)| if matches!(i, 8 | 13 | 18 | 23) { c == '-' } else { c.is_ascii_hexdigit() })
}

/// A count as a range: exact numbers of photos would make a library recognisable.
fn count_range(n: u64) -> &'static str {
    match n {
        0 => "0",
        1..=99 => "1-99",
        100..=999 => "100-999",
        1_000..=9_999 => "1k-10k",
        10_000..=99_999 => "10k-100k",
        _ => "100k+",
    }
}

/// A size in bytes as a range.
fn size_range(bytes: u64) -> &'static str {
    const GB: u64 = 1 << 30;
    match bytes {
        0 => "0",
        b if b < GB => "under 1 GB",
        b if b <= 10 * GB => "1-10 GB",
        b if b <= 100 * GB => "10-100 GB",
        b if b <= 1024 * GB => "100 GB-1 TB",
        _ => "over 1 TB",
    }
}

/// The machine's memory as a range.
fn memory_range(bytes: Option<u64>) -> &'static str {
    const GB: u64 = 1 << 30;
    match bytes {
        None => "unknown",
        Some(b) if b < 2 * GB => "under 2 GB",
        Some(b) if b < 4 * GB => "2-4 GB",
        Some(b) if b < 8 * GB => "4-8 GB",
        Some(b) if b < 16 * GB => "8-16 GB",
        Some(b) if b < 32 * GB => "16-32 GB",
        Some(_) => "32 GB or more",
    }
}

/// What a heartbeat tells, before it becomes JSON.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Facts {
    pub docker: bool,
    pub cores: usize,
    pub memory: Option<u64>,
    pub uptime: u64,
    pub users: usize,
    pub devices: usize,
    pub folders: usize,
    pub photos: u64,
    pub videos: u64,
    pub stored: u64,
    pub search: bool,
    pub faces_users: usize,
    pub requests_24h: u64,
    pub active_devices: usize,
    pub errors: u64,
}

/// The heartbeat's body. `env`, `settings` and `library` are flat maps of facts that replace the last ones; `usage` and
/// `uptime` are what the server did lately.
pub(crate) fn payload(id: &str, f: &Facts) -> Value {
    json!({
        "installId": id,
        "uptime": f.uptime,
        "env": {
            "version": env!("CARGO_PKG_VERSION"),
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "kind": if f.docker { "docker" } else { "program" },
            "cores": f.cores,
            "memory": memory_range(f.memory),
        },
        "settings": { "search": f.search, "faces": f.faces_users > 0 },
        "library": {
            "users": f.users,
            "devices": f.devices,
            "folders": f.folders,
            "photos": count_range(f.photos),
            "videos": count_range(f.videos),
            "stored": size_range(f.stored),
        },
        "usage": { "requests": f.requests_24h, "activeDevices": f.active_devices, "errors": f.errors },
    })
}

/// The server's numbers now. Opens each user's library to count it (the admin page's Users list does the same, and the
/// count is kept until the library changes).
fn gather(st: &State, started: Instant) -> Facts {
    let machine = crate::limits::Machine::this();
    let users = accounts::read_users(&st.data).map(|f| f.users).unwrap_or_default();
    let mut f = Facts {
        docker: Path::new("/.dockerenv").exists() || Path::new("/run/.containerenv").exists(),
        cores: machine.cores,
        memory: machine.memory,
        uptime: started.elapsed().as_secs(),
        users: users.len(),
        search: st.vision.installed(),
        faces_users: users.values().filter(|u| u.faces).count(),
        requests_24h: st.usage.history().iter().fold(0u64, |n, s| n.saturating_add(s.requests)),
        active_devices: st.usage.active_devices(),
        errors: ERRORS.swap(0, Ordering::Relaxed),
        ..Facts::default()
    };
    for (name, u) in &users {
        f.devices = f.devices.saturating_add(accounts::devices(&st.data, name).map_or(0, |d| d.devices.len()));
        f.folders = f.folders.saturating_add(u.folders.len());
        f.stored = f.stored.saturating_add(st.sizes.used(&st.data, name));
        if let Ok(lib) = crate::api::lib(st, name) {
            let (tally, _) = crate::api::library_stats(st, name, &lib);
            f.photos = f.photos.saturating_add(tally.photos as u64);
            f.videos = f.videos.saturating_add(tally.videos as u64);
        }
    }
    f
}

fn send(url: &str, body: &Value) -> Result<(), String> {
    let tls = ureq::tls::TlsConfig::builder()
        .provider(ureq::tls::TlsProvider::Rustls)
        .root_certs(ureq::tls::RootCerts::WebPki)
        .unversioned_rustls_crypto_provider(Arc::new(rustls_rustcrypto::provider()))
        .build();
    let agent: ureq::Agent =
        ureq::Agent::config_builder().http_status_as_error(false).timeout_global(Some(Duration::from_secs(20))).tls_config(tls).build().into();
    let answer = agent.post(url).header("Content-Type", "application/json").send(body.to_string().as_bytes()).map_err(|e| e.to_string())?;
    if answer.status().is_success() { Ok(()) } else { Err(format!("answered {}", answer.status())) }
}

/// Send a heartbeat now (tests call this through [`run`] with a short wait).
fn beat(st: &State, url: &str, id: &str, started: Instant) -> Result<(), String> {
    send(url, &payload(id, &gather(st, started)))
}

/// Send a heartbeat soon after the server starts and every [`EVERY`] until it shuts down. A failed one (offline, the
/// site down) is only logged: the next one tells the same story.
pub(crate) fn run(st: &Arc<State>, url: &str, first: Option<Duration>) {
    let Some(id) = install_id(&st.data) else {
        log::debug!("telemetry: no install id could be kept in {}: no heartbeat", st.data.display());
        return;
    };
    let started = Instant::now();
    let mut wait = first.unwrap_or(FIRST);
    loop {
        let until = Instant::now() + wait;
        while Instant::now() < until {
            if st.usage.stopped() {
                return;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        match beat(st, url, &id, started) {
            Ok(()) => log::debug!("telemetry: heartbeat sent"),
            Err(e) => log::debug!("telemetry: heartbeat not sent: {e}"),
        }
        wait = EVERY;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_and_sizes_are_told_as_ranges() {
        assert_eq!(
            [0, 1, 99, 100, 999, 1_000, 9_999, 10_000, 99_999, 100_000].map(count_range),
            ["0", "1-99", "1-99", "100-999", "100-999", "1k-10k", "1k-10k", "10k-100k", "10k-100k", "100k+"]
        );
        let gb = 1u64 << 30;
        assert_eq!(
            [0, 1, gb - 1, gb, 10 * gb, 10 * gb + 1, 100 * gb, 100 * gb + 1, 1024 * gb, 1024 * gb + 1].map(size_range),
            ["0", "under 1 GB", "under 1 GB", "1-10 GB", "1-10 GB", "10-100 GB", "10-100 GB", "100 GB-1 TB", "100 GB-1 TB", "over 1 TB"]
        );
        assert_eq!(memory_range(None), "unknown");
        assert_eq!(memory_range(Some(3 << 30)), "2-4 GB");
        assert_eq!(memory_range(Some(u64::MAX)), "32 GB or more");
    }

    #[test]
    fn the_install_id_is_made_once_and_kept() {
        let dir = std::env::temp_dir().join(format!("lc-telemetry-id-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let id = install_id(&dir).unwrap();
        assert!(is_uuid(&id) && id.as_bytes()[14] == b'4', "{id}");
        assert_eq!(install_id(&dir), Some(id.clone()), "the same one the next time");
        std::fs::write(dir.join(".lightbites-telemetry-id"), "not an id").unwrap();
        let fresh = install_id(&dir).unwrap();
        assert!(is_uuid(&fresh) && fresh != id, "a damaged file is replaced");
        assert_eq!(install_id(&dir.join("missing")), None, "an unwritable folder gives no id, not a different one each time");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_heartbeat_holds_coarse_facts_and_no_names() {
        let facts = Facts {
            cores: 4,
            memory: Some(8 << 30),
            uptime: 90,
            users: 2,
            devices: 3,
            photos: 12_345,
            stored: 5 << 30,
            search: true,
            requests_24h: 800,
            active_devices: 2,
            ..Facts::default()
        };
        let p = payload("3f2504e0-4f89-41d3-9a0c-0305e82c3301", &facts);
        assert_eq!(p["env"]["kind"], "program");
        assert_eq!(p["env"]["memory"], "8-16 GB");
        assert_eq!(p["library"]["photos"], "10k-100k", "never the exact count");
        assert_eq!(p["library"]["stored"], "1-10 GB");
        assert_eq!(p["library"]["users"], 2);
        assert_eq!(p["settings"]["search"], true);
        assert_eq!(p["usage"]["requests"], 800);
        assert_eq!(p["uptime"], 90);
        assert_eq!(p["env"]["version"], env!("CARGO_PKG_VERSION"));
    }
}
