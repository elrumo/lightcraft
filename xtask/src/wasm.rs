use super::*;

/// Workspace packages that must build on every non-desktop target (wasm32, iOS): all L0–L5 crates.
fn portable_set() -> Result<Vec<String>, String> {
    let crates = layers::from_metadata(&metadata()?)?;
    Ok(crates
        .into_iter()
        .filter(|c| match layers::classify(&c.name) {
            Some(layers::Class::Layer(l)) => l <= 5,
            Some(layers::Class::Standalone) => true,
            _ => false,
        })
        .map(|c| c.name)
        .collect())
}

/// Workspace packages that must build for wasm32: the portable set plus the web app.
fn wasm_set() -> Result<Vec<String>, String> {
    Ok(portable_set()?.into_iter().chain(std::iter::once("lightcraft-web".to_string())).collect())
}

/// Workspace packages that must build for iOS: the portable set plus the egui iOS app.
fn ios_set() -> Result<Vec<String>, String> {
    Ok(portable_set()?.into_iter().chain(std::iter::once("lightcraft-ios".to_string())).collect())
}

/// One `cargo clippy` over the whole set; `--keep-going` reports every crate that fails.
/// Also lints the std clock (`xtask/wasm/clippy.toml`): it panics there, and `cargo check` can't see that.
pub(crate) fn cmd_wasm() -> Result<(), String> {
    cmd_wasm_for(None)
}

/// The wasm check of the wasm-safe crates, or only those of them in `only` (`ci --quick`).
pub(crate) fn cmd_wasm_for(only: Option<&std::collections::BTreeSet<String>>) -> Result<(), String> {
    let set: Vec<String> = wasm_set()?.into_iter().filter(|p| only.is_none_or(|o| o.contains(p))).collect();
    if set.is_empty() {
        println!("wasm32-unknown-unknown check: no changed crate builds for wasm32");
        return Ok(());
    }
    check_target("wasm32-unknown-unknown", &set, true)
}

/// iOS device target. A type-check only: it proves the crates have no desktop-only code on the iOS path (the
/// engine, GPU and egui shell all compile for it), not that an app links and runs; that needs macOS + Xcode.
pub(crate) fn cmd_ios() -> Result<(), String> {
    cmd_ios_for(None)
}

/// The iOS check of the crates that build for iOS, or only those of them in `only` (`ci --quick`).
pub(crate) fn cmd_ios_for(only: Option<&std::collections::BTreeSet<String>>) -> Result<(), String> {
    let set: Vec<String> = ios_set()?.into_iter().filter(|p| only.is_none_or(|o| o.contains(p))).collect();
    if set.is_empty() {
        println!("aarch64-apple-ios check: no changed crate builds for iOS");
        return Ok(());
    }
    check_target("aarch64-apple-ios", &set, false)
}

/// One `cargo check` (`clippy` with the clock lint) over the whole set for `target`.
fn check_target(target: &str, set: &[String], clock_lint: bool) -> Result<(), String> {
    let tool = if clock_lint { "clippy" } else { "check" };
    let mut c = cargo();
    c.args([tool, "--target", target, "--keep-going"]);
    for pkg in set {
        c.args(["-p", pkg]);
    }
    if clock_lint {
        c.env("CLIPPY_CONF_DIR", root().join("xtask/wasm")).args(["--", "-A", "clippy::all", "-D", "clippy::disallowed_methods"]);
    }
    run(c, &format!("cargo {tool} --target {target} --keep-going -p {}", set.join(" -p ")))
        .map_err(|e| format!("{e} (is the target installed? `rustup target add {target}`)"))
}
