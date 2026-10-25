//! Community and project links (Help menu, About dialog, top-bar Discord button).

pub use lightcraft_engine::about::{APP, APP_PAGE, CAMERAS, DISCORD, FEEDBACK, GITHUB, HELP, LINKS, WEBSITE, url_of};

/// Open `url` in the user's browser (through the host's `open_url` service).
pub fn open(app: &mut crate::LightcraftApp, url: &str) -> Result<serde_json::Value, String> {
    let open = app.services.open_url.as_mut().ok_or("can't open links here")?;
    open(url)?;
    Ok(serde_json::json!({ "url": url }))
}
