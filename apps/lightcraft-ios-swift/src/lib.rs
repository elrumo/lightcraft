//! LightCraft's SwiftUI iOS app, Rust side: the C ABI (`lc_*`, [`abi`]) that the Swift package in
//! `LightCraft/` calls (`docs/ios-swift.md`). Everything behind it is shared with the other apps:
//! the engine runs on [`lightcraft_apphost::host`]'s thread and answers JSON requests with JSON
//! events; pictures and answers cross the ABI as leases.
//!
//! This file is the safe half: engines by handle, buffers by token. [`abi`] only turns C arguments
//! into slices and the answers into C structs.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

#[allow(unsafe_code)]
pub mod abi;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, Once, PoisonError};
use std::time::Duration;

use lightcraft_apphost::host::{BytesRef, Host, ImageRef, Leases, OpenConfig};
use serde_json::{Value, json};

/// The ABI this library speaks (`LC_ABI_VERSION` in the header).
pub const ABI_VERSION: u32 = 1;

/// Status codes (`LC_*` in the header).
pub mod status {
    pub const OK: i32 = 0;
    pub const ERROR: i32 = 1;
    pub const BAD_HANDLE: i32 = 2;
    pub const BAD_INPUT: i32 = 3;
    pub const PANIC: i32 = 4;
    pub const CLOSED: i32 = 5;
    pub const GONE: i32 = 6;
}

/// How long closing an engine waits for its thread.
const CLOSE_WAIT: Duration = Duration::from_secs(5);
/// The longest an `lc_poll` or `lc_call` may block.
const MAX_WAIT_MS: u32 = 60_000;

struct Bridge {
    engines: Mutex<HashMap<u64, Arc<Host>>>,
    next: AtomicU64,
    /// One registry for every engine's buffers, so a token alone names its buffer.
    leases: Arc<Leases>,
}

static BRIDGE: LazyLock<Bridge> =
    LazyLock::new(|| Bridge { engines: Mutex::new(HashMap::new()), next: AtomicU64::new(1), leases: Arc::new(Leases::default()) });

/// A JSON answer handed out: its status and its leased bytes.
#[derive(Clone, Copy, Debug)]
pub struct Answer {
    pub status: i32,
    pub bytes: BytesRef,
}

fn answer(status: i32, value: &Value) -> Answer {
    let bytes = serde_json::to_vec(value).unwrap_or_else(|_| b"{}".to_vec());
    Answer { status, bytes: BRIDGE.leases.put_bytes(bytes) }
}

/// An error answer: `{"code", "message"}`.
pub fn error(status: i32, code: &str, message: &str) -> Answer {
    answer(status, &json!({"code": code, "message": message}))
}

fn engine(handle: u64) -> Option<Arc<Host>> {
    BRIDGE.engines.lock().unwrap_or_else(PoisonError::into_inner).get(&handle).cloned()
}

fn no_engine(handle: u64) -> Answer {
    error(status::BAD_HANDLE, "badHandle", &format!("no engine {handle} (closed, or never opened)"))
}

/// What the process needs once, before the first engine: logs, the panic log, the system's
/// HEIC / AVIF decoder (iOS). A Mac logs to stderr like the egui app (`LIGHTCRAFT_LOG`) and decodes
/// HEIC in Rust: `ios-host`'s ImageIO decoder is iOS-only, and installing it without porting its
/// `decode_image` would fail every HEIC instead of falling back to the Rust decoder.
fn setup(config: &OpenConfig) {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        #[cfg(target_os = "ios")]
        {
            lightcraft_ios_host::install_logger();
            lightcraft_ios_host::install_imageio_decoder();
        }
        #[cfg(target_os = "macos")]
        lightcraft_engine::logging::install("lightcraft");
        let tmp = config.tmp.as_deref().map_or_else(std::env::temp_dir, std::path::PathBuf::from);
        lightcraft_engine::guard::install_hook(tmp.join("lightcraft-panics.log"));
    });
}

/// `lc_open`: start an engine from a JSON [`OpenConfig`].
pub fn open(config: &[u8]) -> Answer {
    let config: OpenConfig = match serde_json::from_slice(config) {
        Ok(c) => c,
        Err(e) => return error(status::BAD_INPUT, "badInput", &format!("not an engine configuration: {e}")),
    };
    setup(&config);
    match Host::open_with(config, BRIDGE.leases.clone()) {
        Ok((host, mut opened)) => {
            let handle = BRIDGE.next.fetch_add(1, Ordering::Relaxed);
            BRIDGE.engines.lock().unwrap_or_else(PoisonError::into_inner).insert(handle, Arc::new(host));
            opened["engine"] = json!(handle);
            answer(status::OK, &opened)
        }
        Err(e) => error(status::ERROR, "openFailed", &e),
    }
}

/// `lc_close`: save, stop and forget the engine (an unknown handle: nothing).
pub fn close(handle: u64) {
    let host = BRIDGE.engines.lock().unwrap_or_else(PoisonError::into_inner).remove(&handle);
    if let Some(host) = host {
        host.close(CLOSE_WAIT);
    }
}

/// `lc_submit`: queue a request.
pub fn submit(handle: u64, request: &[u8]) -> i32 {
    let Some(host) = engine(handle) else { return status::BAD_HANDLE };
    match host.submit(request) {
        Ok(()) => status::OK,
        Err(e) if e.code == "closed" => status::CLOSED,
        Err(_) => status::BAD_INPUT,
    }
}

/// `lc_call`: run a request and wait for its reply event.
pub fn call(handle: u64, request: &[u8], timeout_ms: u32) -> Answer {
    let Some(host) = engine(handle) else { return no_engine(handle) };
    let reply = host.call(request, Duration::from_millis(u64::from(timeout_ms.min(MAX_WAIT_MS))));
    answer(status::OK, &reply)
}

/// `lc_poll`: the events since the last poll, as a JSON array.
pub fn poll(handle: u64, timeout_ms: u32) -> Answer {
    let Some(host) = engine(handle) else { return no_engine(handle) };
    let events = host.poll(Duration::from_millis(u64::from(timeout_ms.min(MAX_WAIT_MS))));
    answer(status::OK, &Value::Array(events))
}

/// `lc_wake`: a waiting `lc_poll` returns now.
pub fn wake(handle: u64) {
    if let Some(host) = engine(handle) {
        host.wake();
    }
}

/// `lc_image`: the pixels of an `image` event's token.
pub fn image(token: u64) -> Result<ImageRef, i32> {
    BRIDGE.leases.take_image(token).ok_or(status::GONE)
}

/// `lc_release`: free a buffer by its token.
pub fn release(token: u64) {
    BRIDGE.leases.release(token);
}

#[cfg(test)]
mod fixtures;
