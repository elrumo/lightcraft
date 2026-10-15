// The version and commit a build reports (the sync server's admin page, System Info, the web
// bundle's `version.json`). Shared source: `cargo xtask web` uses it as a module, and the build
// scripts of `lightcraft-server` and `lightcraft-ios-swift` `include!` it.
//
// `LIGHTCRAFT_VERSION` / `LIGHTCRAFT_COMMIT` in the environment win (a Docker build has no `.git`:
// `make push-server` passes them as build args); otherwise `git rev-parse` in `dir`. No git, or not
// a checkout: "unknown".

use std::path::Path;
use std::process::Command;

fn git_in(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).current_dir(dir).output().ok()?;
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (out.status.success() && !s.is_empty()).then_some(s)
}

fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

/// `LIGHTCRAFT_VERSION`, else `fallback` (the workspace version).
pub fn version(fallback: &str) -> String {
    env_nonempty("LIGHTCRAFT_VERSION").unwrap_or_else(|| fallback.to_string())
}

/// `LIGHTCRAFT_COMMIT`, else the checkout's short commit, with `-dirty` when `dirty_check` and
/// tracked files have uncommitted changes.
pub fn commit(dir: &Path, dirty_check: bool) -> String {
    env_nonempty("LIGHTCRAFT_COMMIT")
        .or_else(|| {
            let head = git_in(dir, &["rev-parse", "--short=10", "HEAD"])?;
            let dirty = dirty_check && git_in(dir, &["status", "--porcelain", "--untracked-files=no"]).is_some();
            Some(if dirty { format!("{head}-dirty") } else { head })
        })
        .unwrap_or_else(|| "unknown".into())
}

/// For a build script: rerun it when the commit (or the overrides) change, and set
/// `LIGHTCRAFT_BUILD_VERSION` and `LIGHTCRAFT_COMMIT` for the crate. No `-dirty` here: a build
/// script reruns only on the paths it names, so it couldn't keep that up to date.
#[allow(dead_code)]
pub fn emit_for_build_script(fallback_version: &str) {
    let here = Path::new(".");
    println!("cargo:rerun-if-env-changed=LIGHTCRAFT_VERSION");
    println!("cargo:rerun-if-env-changed=LIGHTCRAFT_COMMIT");
    // a new commit moves HEAD, or the branch it points at
    let branch = git_in(here, &["symbolic-ref", "-q", "HEAD"]).and_then(|r| git_in(here, &["rev-parse", "--git-path", &r]));
    for path in
        [git_in(here, &["rev-parse", "--git-path", "HEAD"]), branch, git_in(here, &["rev-parse", "--git-path", "packed-refs"])].into_iter().flatten()
    {
        println!("cargo:rerun-if-changed={path}");
    }
    println!("cargo:rustc-env=LIGHTCRAFT_BUILD_VERSION={}", version(fallback_version));
    println!("cargo:rustc-env=LIGHTCRAFT_COMMIT={}", commit(here, false));
}
