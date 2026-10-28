use super::*;

const TARGET: &str = "aarch64-apple-ios";

/// Workspace packages that must build for iOS: all L0–L5 crates (the engine, GPU and egui shell
/// included) plus the iOS glue and the egui iOS app, when the workspace has them.
fn ios_set() -> Result<Vec<String>, String> {
    let crates = layers::from_metadata(&metadata()?)?;
    Ok(crates
        .into_iter()
        .filter(|c| match layers::classify(&c.name) {
            Some(layers::Class::Layer(l)) => l <= 5,
            Some(layers::Class::Standalone) => true,
            _ => matches!(c.name.as_str(), "lightcraft-ios-host" | "lightcraft-ios"),
        })
        .map(|c| c.name)
        .collect())
}

/// Is the iOS target installed (`rustup target add aarch64-apple-ios`)? `ci` skips the check when
/// it isn't, so a machine without the target (or a gate that doesn't install it) is not failed by it.
pub(crate) fn target_installed() -> bool {
    Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).lines().any(|l| l.trim() == TARGET))
}

/// The iOS device target, one `cargo check` over the whole set (`--keep-going` reports every crate
/// that fails). A type-check only: it proves the crates have no desktop-only code on the iOS path,
/// not that an app links and runs; that needs macOS and Xcode (docs/ios.md).
pub(crate) fn cmd_ios() -> Result<(), String> {
    cmd_ios_for(None)
}

/// The iOS check of the crates that build for iOS, or only those of them in `only` (`ci --quick`).
pub(crate) fn cmd_ios_for(only: Option<&std::collections::BTreeSet<String>>) -> Result<(), String> {
    let set: Vec<String> = ios_set()?.into_iter().filter(|p| only.is_none_or(|o| o.contains(p))).collect();
    if set.is_empty() {
        println!("{TARGET} check: no changed crate builds for iOS");
        return Ok(());
    }
    let mut c = cargo();
    c.args(["check", "--target", TARGET, "--keep-going"]);
    for pkg in &set {
        c.args(["-p", pkg]);
    }
    run(c, &format!("cargo check --target {TARGET} --keep-going -p {}", set.join(" -p ")))
        .map_err(|e| format!("{e} (is the target installed? `rustup target add {TARGET}`)"))
}
