//! The version and commit the admin page shows (`LIGHTCRAFT_BUILD_VERSION`, `LIGHTCRAFT_COMMIT`), and the server's
//! name, command and links (`src/brand.rs`): `LIGHTCRAFT_SERVER_*` at build time, LightCraft's when unset. The admin
//! and welcome pages are copied to `OUT_DIR` with their `{{…}}` placeholders filled in.

use std::path::PathBuf;

include!("../../xtask/src/build_info.rs");

/// The build-time settings: variable, field in the pages (`{{FIELD}}`), default.
const BRAND: &[(&str, &str, &str)] = &[
    ("LIGHTCRAFT_SERVER_PRODUCT", "PRODUCT", "LightCraft"),
    ("LIGHTCRAFT_SERVER_COMMAND", "COMMAND", "lightcraft-server"),
    ("LIGHTCRAFT_SERVER_MENUBAR", "MENUBAR", "lightcraft-menubar"),
    ("LIGHTCRAFT_SERVER_GUIDE_URL", "GUIDE_URL", "https://github.com/storytold/lightcraft/blob/main/docs/self-hosting.md"),
    ("LIGHTCRAFT_SERVER_DOWNLOADS_URL", "DOWNLOADS_URL", "https://github.com/storytold/lightcraft/releases"),
    ("LIGHTCRAFT_SERVER_TELEMETRY_URL", "TELEMETRY_URL", ""),
    ("LIGHTCRAFT_SERVER_BUGSINK_DSN", "BUGSINK_DSN", ""),
];

const PAGES: &[&str] = &["index.html", "admin.js", "welcome.html", "welcome.js"];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../xtask/src/build_info.rs");
    emit_for_build_script(env!("CARGO_PKG_VERSION"));
    if let Err(e) = brand() {
        println!("cargo:warning={e}");
        std::process::exit(1);
    }
}

fn brand() -> Result<(), String> {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").map_err(|e| e.to_string())?);
    let out = PathBuf::from(std::env::var("OUT_DIR").map_err(|e| e.to_string())?);
    let mut fields = Vec::new();
    for (var, field, default) in BRAND {
        println!("cargo:rerun-if-env-changed={var}");
        let value = std::env::var(var).ok().filter(|v| !v.is_empty()).unwrap_or_else(|| (*default).to_string());
        println!("cargo:rustc-env={var}={value}");
        fields.push((format!("{{{{{field}}}}}"), value));
    }
    // the guide's address as a link's text, and the Docker Compose service (`docker compose logs lightcraft`)
    let guide = fields.iter().find(|(f, _)| f == "{{GUIDE_URL}}").map(|(_, v)| v.clone()).unwrap_or_default();
    fields.push(("{{GUIDE_TEXT}}".into(), guide.trim_start_matches("https://").trim_start_matches("http://").into()));
    let product = fields.first().map(|(_, v)| v.to_lowercase()).unwrap_or_default();
    fields.push(("{{SERVICE}}".into(), product));

    for page in PAGES {
        let src = manifest.join("admin").join(page);
        println!("cargo:rerun-if-changed={}", src.display());
        let mut text = std::fs::read_to_string(&src).map_err(|e| format!("{}: {e}", src.display()))?;
        for (field, value) in &fields {
            text = text.replace(field.as_str(), value);
        }
        if let Some(left) = text.find("{{").and_then(|i| text.get(i..).and_then(|t| t.find("}}").map(|j| &t[..j + 2]))) {
            return Err(format!("{}: unknown placeholder {left}", src.display()));
        }
        std::fs::write(out.join(page), text).map_err(|e| format!("{}: {e}", out.display()))?;
    }

    // the page's icon: `LIGHTCRAFT_SERVER_ICON` (a PNG, relative to the workspace), else LightCraft's app icon
    println!("cargo:rerun-if-env-changed=LIGHTCRAFT_SERVER_ICON");
    let workspace = manifest.parent().and_then(Path::parent).ok_or("the package is not two folders inside the workspace")?;
    let icon = std::env::var("LIGHTCRAFT_SERVER_ICON")
        .ok()
        .filter(|v| !v.is_empty())
        .map(|v| if Path::new(&v).is_absolute() { PathBuf::from(v) } else { workspace.join(v) })
        .unwrap_or_else(|| workspace.join("assets/app-icon/hicolor/128x128/apps/ai.storyteller.lightcraft.png"));
    println!("cargo:rerun-if-changed={}", icon.display());
    std::fs::copy(&icon, out.join("icon.png")).map_err(|e| format!("{}: {e}", icon.display()))?;
    Ok(())
}
