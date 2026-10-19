//! What the app says about itself, the same in every UI: the project's links (Help menu, About),
//! What's New, and the facts System Info lists (`about.links`, `about.whatsNew`,
//! `about.systemInfo`).

use crate::Session;

/// The app's id in ArtCraft URLs.
pub const APP: &str = "lightcraft";
/// The ArtCraft community Discord.
pub const DISCORD: &str = "https://discord.gg/artcraft";
/// The ArtCraft website.
pub const WEBSITE: &str = "https://getartcraft.com";
/// This app's page on the ArtCraft website.
pub const APP_PAGE: &str = "https://getartcraft.com/apps/lightcraft";
/// This app's source repository.
pub const GITHUB: &str = "https://github.com/storytold/lightcraft";
/// The user documentation (docs/ in the repository).
pub const HELP: &str = "https://github.com/storytold/lightcraft/tree/main/docs";
/// Where feedback and bug reports go.
pub const FEEDBACK: &str = "https://github.com/storytold/lightcraft/issues/new";

/// (UI command id, menu label, URL) for each link, in Help-menu order.
pub const LINKS: &[(&str, &str, &str)] = &[
    ("app.help", "LightCraft Help", HELP),
    ("app.discord", "Join the ArtCraft Discord…", DISCORD),
    ("app.website", "LightCraft Website", APP_PAGE),
    ("app.github", "LightCraft on GitHub", GITHUB),
    ("app.artcraft", "ArtCraft Website", WEBSITE),
    ("app.feedback", "Send Feedback…", FEEDBACK),
];

/// The URL behind a link command id.
pub fn url_of(cmd: &str) -> Option<&'static str> {
    LINKS.iter().find(|(id, _, _)| *id == cmd).map(|(_, _, u)| *u)
}

/// What's new in this version (Markdown: `##` headings, `-` items).
pub const WHATS_NEW: &str = include_str!("../../../docs/whats-new.md");

static COMMIT: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
static APP_NAME: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();

/// A host that is another product built on the engine sets, once at start, the name its files are signed with.
pub fn set_app_name(name: &'static str) {
    let _ = APP_NAME.set(name);
}

/// The name this program signs the files it writes with: the EXIF Software tag, the XMP creator tool, the PDF
/// producer. `LightCraft` unless the host set its own.
pub fn app_name() -> &'static str {
    APP_NAME.get().copied().unwrap_or("LightCraft")
}

static APP_URL: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();

/// A host that is another product sets, once at start, the page that says what it is (the contact in the
/// User-Agent of the requests it makes, which map-tile servers require).
pub fn set_app_url(url: &'static str) {
    let _ = APP_URL.set(url);
}

/// The page that says what this program is: [`GITHUB`] unless the host set its own.
pub fn app_url() -> &'static str {
    APP_URL.get().copied().unwrap_or(GITHUB)
}

/// The commit this app was built from, for System Info. The app sets it once (its build script
/// stamps it: one in the engine would recompile the engine on every commit).
pub fn set_commit(commit: &'static str) {
    let _ = COMMIT.set(commit);
}

/// System Info's facts that don't depend on the UI, in order: the version, the system, the CPU
/// threads, the GPU, the memory budget, the library's photos, albums and folder, and the last GPU
/// fallback (a UI adds its own rows: its settings, frame times).
pub fn system_rows(s: &Session) -> Vec<(String, String)> {
    let gpu = (lightcraft_gpu::ready() && lightcraft_gpu::available()).then(lightcraft_gpu::adapter_name).flatten();
    let mb = |b: u64| format!("{:.0} MB", b as f64 / (1u64 << 20) as f64);
    let mut rows = vec![("Version".to_string(), env!("CARGO_PKG_VERSION").to_string())];
    if let Some(c) = COMMIT.get() {
        rows.push(("Commit".into(), c.to_string()));
    }
    rows.extend([
        ("System".to_string(), format!("{} ({})", std::env::consts::OS, std::env::consts::ARCH)),
        ("CPU threads".to_string(), std::thread::available_parallelism().map(|n| n.get().to_string()).unwrap_or_else(|_| "?".into())),
        (
            "GPU".to_string(),
            gpu.unwrap_or_else(|| match lightcraft_gpu::unavailable_reason() {
                Some(why) => format!("none (CPU rendering): {why}"),
                None => "none (CPU rendering)".into(),
            }),
        ),
        ("Memory budget".to_string(), mb(crate::memory::default_budget() as u64)),
        ("Photos".to_string(), s.catalog.len().to_string()),
        ("Albums".to_string(), s.catalog.albums().count().to_string()),
    ]);
    if let Some(dir) = s.library.as_ref().map(|l| l.dir.display().to_string()) {
        rows.push(("Library".into(), dir));
    }
    if let Some(f) = lightcraft_gpu::last_fallback() {
        rows.push(("Last GPU fallback".into(), f));
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_are_https_and_name_this_app() {
        for (id, label, url) in LINKS {
            assert!(url.starts_with("https://"), "{id}");
            assert!(!label.is_empty());
            assert_eq!(url_of(id), Some(*url));
        }
        assert!(APP_PAGE.ends_with(&format!("/apps/{APP}")));
        assert!(GITHUB.ends_with(&format!("/storytold/{APP}")));
    }

    #[test]
    fn system_info_lists_the_version_and_the_library() {
        let s = Session::with_demo();
        let rows = system_rows(&s);
        assert_eq!(rows[0], ("Version".to_string(), env!("CARGO_PKG_VERSION").to_string()));
        assert!(rows.iter().any(|(k, v)| k == "Photos" && v == &s.catalog.len().to_string()));
        assert!(!WHATS_NEW.trim().is_empty());
    }

    #[test]
    fn system_info_shows_the_commit_an_app_set() {
        set_commit("ffae40d8ab");
        let rows = system_rows(&Session::with_demo());
        assert_eq!(rows[1], ("Commit".to_string(), "ffae40d8ab".to_string()));
    }

    #[test]
    fn files_are_signed_as_lightcraft_unless_the_host_names_itself() {
        // (no test sets the name: it can be set once per process)
        assert_eq!(app_name(), "LightCraft");
        assert_eq!(app_url(), GITHUB);
    }
}
