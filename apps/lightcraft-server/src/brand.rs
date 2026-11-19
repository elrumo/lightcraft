//! The server's name, command and links, fixed when it is built: `build.rs` reads `LIGHTCRAFT_SERVER_PRODUCT`,
//! `LIGHTCRAFT_SERVER_COMMAND`, `LIGHTCRAFT_SERVER_MENUBAR`, `LIGHTCRAFT_SERVER_GUIDE_URL`,
//! `LIGHTCRAFT_SERVER_DOWNLOADS_URL`, `LIGHTCRAFT_SERVER_TELEMETRY_URL`, `LIGHTCRAFT_SERVER_BUGSINK_DSN` and
//! `LIGHTCRAFT_SERVER_ICON` from the environment, with LightCraft's values when they are unset. A distribution that
//! ships the server under its own name sets them in its build (docs/self-hosting.md → Building under another name);
//! the source stays the same.

/// The product's name, as people read it ("LightCraft": "LightCraft Server", "In LightCraft, open Settings").
pub const PRODUCT: &str = env!("LIGHTCRAFT_SERVER_PRODUCT");
/// The program's name, as commands and messages show it (`lightcraft-server user add ann`).
pub const COMMAND: &str = env!("LIGHTCRAFT_SERVER_COMMAND");
/// The menu-bar program a Mac release carries beside the server, if any.
pub const MENUBAR: &str = env!("LIGHTCRAFT_SERVER_MENUBAR");
/// Where the setup guides are.
pub const GUIDE_URL: &str = env!("LIGHTCRAFT_SERVER_GUIDE_URL");
/// Where heartbeats go when telemetry is on (empty: nowhere, unless `LIGHTCRAFT_TELEMETRY_URL` says so).
pub const TELEMETRY_URL: &str = env!("LIGHTCRAFT_SERVER_TELEMETRY_URL");
/// The Bugsink (Sentry-compatible) address error reports go to (empty: none, unless `LIGHTCRAFT_BUGSINK_DSN` says so).
pub const BUGSINK_DSN: &str = env!("LIGHTCRAFT_SERVER_BUGSINK_DSN");

/// The product's name in file names and fingerprints ("lightcraft").
pub fn slug() -> String {
    PRODUCT.to_lowercase()
}

/// A setting from the environment. The server's variables are `LIGHTCRAFT_*`; under another product name the same
/// variable with that name (`LIGHTCRAFT_DATA` → `OTHER_DATA`) works too and wins, so a Docker `ENV LIGHTCRAFT_DATA`
/// default doesn't override what the user set.
pub fn env(name: &str) -> Option<String> {
    env_pick(name, |k| std::env::var(k).ok())
}

/// [`env`] over any lookup (tests).
pub fn env_pick(name: &str, get: impl Fn(&str) -> Option<String>) -> Option<String> {
    let prefix = PRODUCT.to_uppercase();
    let alias = name.strip_prefix("LIGHTCRAFT_").filter(|_| prefix != "LIGHTCRAFT").map(|rest| format!("{prefix}_{rest}"));
    alias.and_then(|a| get(&a)).or_else(|| get(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_product_variable_works_and_wins() {
        let alias = format!("{}_DATA", PRODUCT.to_uppercase());
        let both = [("LIGHTCRAFT_DATA".to_string(), "/a"), (alias.clone(), "/b")];
        let get = |k: &str| both.iter().find(|(n, _)| n == k).map(|(_, v)| v.to_string());
        let want = if alias == "LIGHTCRAFT_DATA" { "/a" } else { "/b" };
        assert_eq!(env_pick("LIGHTCRAFT_DATA", get).as_deref(), Some(want));
        let only = |k: &str| (k == alias).then(|| "/b".to_string());
        assert_eq!(env_pick("LIGHTCRAFT_DATA", only).as_deref(), Some("/b"));
        assert_eq!(env_pick("LIGHTCRAFT_DATA", |_| None), None);
    }
}
