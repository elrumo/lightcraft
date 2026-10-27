//! `lightbites-menubar`: a menu-bar icon for the installed LightBites Server (macOS). It shows whether the server is
//! running and starts or stops it by running `lightbites-server start` / `stop` (the same commands a terminal uses), so
//! there is one place that knows how the server is installed. `--port N` sets the port the Admin item opens (8080).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

// the state module is shared; off macOS only its tests use it
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod state;

#[cfg(target_os = "macos")]
mod mac;

#[cfg(target_os = "macos")]
fn main() {
    if let Err(e) = mac::run() {
        eprintln!("lightbites-menubar: {e}");
        std::process::exit(1);
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("lightbites-menubar is for macOS; on Linux use `lightbites-server start|stop|status` (systemd user service)");
    std::process::exit(1);
}
