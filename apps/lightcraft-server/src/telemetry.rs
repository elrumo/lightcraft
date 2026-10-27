//! Anonymous, optional server telemetry. No account or device identifiers are sent.

use std::path::Path;

use crate::accounts;

/// Starts Bugsink reporting for the lifetime of the server process and reports aggregate usage.
pub fn start(data: &Path) -> Option<sentry::ClientInitGuard> {
    if std::env::var("LIGHTBITES_TELEMETRY").is_ok_and(|value| matches!(value.as_str(), "0" | "false" | "off")) {
        return None;
    }
    let dsn = std::env::var("LIGHTBITES_BUGSINK_DSN")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| option_env!("LIGHTBITES_BUGSINK_DSN").filter(|value| !value.is_empty()).map(str::to_owned))?;
    let dsn = dsn.parse::<sentry::types::Dsn>().ok()?;
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
        sentry::with_scope(
            |scope| {
                scope.set_fingerprint(Some(&["lightbites", "server-error"]));
                scope.set_tag("http.status", status);
            },
            || sentry::capture_message("server request failed", sentry::Level::Error),
        );
    }
}
