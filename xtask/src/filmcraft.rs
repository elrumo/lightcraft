//! `cargo xtask filmcraft`: keeps the FilmCraft pin moving (docs/video.md, "FilmCraft pin").
//!
//! Every `filmcraft-*` entry in the root `Cargo.toml` is a git dependency on one `rev` (Dependabot does not move those).
//!
//! - `--check [--strict]`: compares the pin with the head of FilmCraft's `main`, lists the commits in between and the files
//!   they changed in the crates we use, and FilmCraft's `rust-version`. Exits non-zero only with `--strict` when behind.
//! - `--update [REV] [--no-test]`: rewrites every pin (default: `main`'s head), runs `cargo update` for the FilmCraft
//!   packages only, then the video tests. Refuses a rev whose `rust-version` is higher than ours and the old pin's.
//!   `--no-test` leaves the tests to CI (the weekly workflow opens a pull request that CI builds).
//!
//! The network is `git ls-remote`, `curl` (GitHub's API and raw files; `GITHUB_TOKEN` is used when set) and nothing else.

use std::path::Path;
use std::process::Command;

const REPO: &str = "storytold/filmcraft";
const REMOTE: &str = "https://github.com/storytold/filmcraft";
/// Files that carry the pin: the workspace, and the measurement crate that must build with the same decoders.
const PIN_FILES: &[&str] = &["Cargo.toml", "crates/video/hw-bench/Cargo.toml"];
/// FilmCraft crates the product uses (a change elsewhere in FilmCraft cannot change a picture we decode).
const USED_CRATES: &[&str] = &["isobmff", "h264", "hevc", "av1", "aac", "h264enc", "bitstream", "frame", "color", "time"];
/// The most commit subjects listed.
const LIST_MAX: usize = 40;
/// Tests that must stay green across a bump (golden pictures, hostile files, the engine's import path).
const VIDEO_TESTS: &[&[&str]] = &[&["test", "-p", "lightcraft-video", "--features", "testdata"], &["test", "-p", "lightcraft-engine", "tests_video"]];

pub fn run(root: &Path, args: &[&str]) -> Result<(), String> {
    let flag = |f: &str| args.contains(&f);
    let rev_arg = args.iter().find(|a| !a.starts_with("--")).copied();
    match (flag("--check"), flag("--update")) {
        (true, false) => check(root, flag("--strict")),
        (false, true) => update(root, rev_arg, flag("--no-test")),
        _ => Err("usage: cargo xtask filmcraft --check [--strict] | --update [REV] [--no-test]".into()),
    }
}

// ---- reading and rewriting the pin ------------------------------------------------------------------------------------

/// The `filmcraft-*` git dependencies of a manifest: (crate, rev).
pub fn pins(manifest: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in manifest.lines() {
        let t = line.trim_start();
        if t.starts_with('#') || !t.starts_with("filmcraft-") || !t.contains(REMOTE) {
            continue;
        }
        let name = t.split([' ', '=']).next().unwrap_or_default();
        if let Some(rev) = quoted_after(t, "rev") {
            out.push((name.to_string(), rev.to_string()));
        }
    }
    out
}

/// The text between the quotes after `key = ` in `line`.
fn quoted_after<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let at = line.find(&format!("{key} = \""))? + key.len() + 4;
    let rest = line.get(at..)?;
    rest.get(..rest.find('"')?)
}

/// The one rev every pin shares, or why there isn't one.
pub fn single_rev(pins: &[(String, String)]) -> Result<String, String> {
    let first = pins.first().map(|(_, r)| r.clone()).ok_or("no filmcraft-* dependency pinned by rev")?;
    match pins.iter().find(|(_, r)| *r != first) {
        Some((name, rev)) => Err(format!("the pins disagree: `{name}` is at {rev}, others at {first} (they must move together)")),
        None => Ok(first),
    }
}

/// `manifest` with `old` replaced by `new` on the `filmcraft-*` lines only.
pub fn repin(manifest: &str, old: &str, new: &str) -> String {
    let mut out = String::with_capacity(manifest.len());
    for line in manifest.split_inclusive('\n') {
        let t = line.trim_start();
        if t.starts_with("filmcraft-") && t.contains(REMOTE) {
            out.push_str(&line.replace(&format!("rev = \"{old}\""), &format!("rev = \"{new}\"")));
        } else {
            out.push_str(line);
        }
    }
    out
}

/// The first 12 characters of a rev (all of it when shorter).
fn short(rev: &str) -> &str {
    rev.get(..12).unwrap_or(rev)
}

fn is_rev(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// `rust-version = "1.95"` of a manifest as (major, minor).
pub fn rust_version(manifest: &str) -> Option<(u32, u32)> {
    let line = manifest.lines().find(|l| l.trim_start().starts_with("rust-version"))?;
    let mut parts = quoted_after(line, "rust-version")?.split('.');
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

/// Whether a compare-API file path is in a crate the product uses.
pub fn is_used(path: &str) -> bool {
    path.strip_prefix("crates/").and_then(|p| p.split('/').next()).is_some_and(|c| USED_CRATES.contains(&c))
}

// ---- the network -------------------------------------------------------------------------------------------------------

fn head_of_main() -> Result<String, String> {
    let out = Command::new("git").args(["ls-remote", REMOTE, "main"]).output().map_err(|e| format!("git ls-remote: {e}"))?;
    if !out.status.success() {
        return Err(format!("git ls-remote {REMOTE}: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    text.split_whitespace().next().filter(|r| is_rev(r)).map(str::to_string).ok_or_else(|| format!("no `main` in {REMOTE}"))
}

fn curl(url: &str, json: bool) -> Result<String, String> {
    let mut c = Command::new("curl");
    c.args(["-fsSL", "--max-time", "60"]);
    if json {
        c.args(["-H", "Accept: application/vnd.github+json"]);
    }
    if let Ok(token) = std::env::var("GITHUB_TOKEN")
        && !token.is_empty()
    {
        c.args(["-H", &format!("Authorization: Bearer {token}")]);
    }
    let out = c.arg(url).output().map_err(|e| format!("curl: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(format!("curl {url}: {}", String::from_utf8_lossy(&out.stderr).trim()))
    }
}

/// FilmCraft's `rust-version` at `rev`.
fn rust_version_at(rev: &str) -> Result<(u32, u32), String> {
    let text = curl(&format!("https://raw.githubusercontent.com/{REPO}/{rev}/Cargo.toml"), false)?;
    rust_version(&text).ok_or_else(|| format!("no rust-version in FilmCraft's Cargo.toml at {rev}"))
}

fn full_rev(rev: &str) -> Result<String, String> {
    if is_rev(rev) {
        return Ok(rev.to_string());
    }
    let v: serde_json::Value =
        serde_json::from_str(&curl(&format!("https://api.github.com/repos/{REPO}/commits/{rev}"), true)?).map_err(|e| format!("{rev}: {e}"))?;
    v["sha"].as_str().filter(|s| is_rev(s)).map(str::to_string).ok_or_else(|| format!("`{rev}` is not a commit of {REPO}"))
}

fn read_pin(root: &Path) -> Result<(String, Vec<String>), String> {
    let text = std::fs::read_to_string(root.join("Cargo.toml")).map_err(|e| format!("Cargo.toml: {e}"))?;
    let pins = pins(&text);
    Ok((single_rev(&pins)?, pins.into_iter().map(|(n, _)| n).collect()))
}

fn our_rust_version(root: &Path) -> Result<(u32, u32), String> {
    let text = std::fs::read_to_string(root.join("Cargo.toml")).map_err(|e| format!("Cargo.toml: {e}"))?;
    rust_version(&text).ok_or_else(|| "no rust-version in Cargo.toml".to_string())
}

// ---- the commands -----------------------------------------------------------------------------------------------------

fn check(root: &Path, strict: bool) -> Result<(), String> {
    let (pin, names) = read_pin(root)?;
    let head = head_of_main()?;
    println!("FilmCraft pin: {} ({})", short(&pin), names.join(", "));
    if pin == head {
        println!("up to date with main ({})", short(&head));
        return Ok(());
    }
    println!("main is at {}", short(&head));
    let ours = our_rust_version(root)?;
    match (rust_version_at(&pin), rust_version_at(&head)) {
        (Ok(old), Ok(new)) => println!(
            "rust-version: pin {}.{}, main {}.{}, ours {}.{}{}",
            old.0,
            old.1,
            new.0,
            new.1,
            ours.0,
            ours.1,
            if new > old.max(ours) { "  <- raises it: --update will refuse until ours is bumped" } else { "" }
        ),
        (a, b) => println!("rust-version: could not read ({})", a.err().or(b.err()).unwrap_or_default()),
    }
    match curl(&format!("https://api.github.com/repos/{REPO}/compare/{pin}...{head}"), true)
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).map_err(|e| e.to_string()))
    {
        Ok(v) => {
            let status = v["status"].as_str().unwrap_or("?");
            let commits = v["commits"].as_array().cloned().unwrap_or_default();
            let (ahead, behind) = (v["ahead_by"].as_u64().unwrap_or(0), v["behind_by"].as_u64().unwrap_or(0));
            let diverged = if behind > 0 { format!(" (and the pin has {behind} that main lacks: {status})") } else { String::new() };
            println!("main has {ahead} commit(s) the pin lacks{diverged}");
            for c in commits.iter().take(LIST_MAX) {
                let sha = c["sha"].as_str().unwrap_or("").get(..8).unwrap_or("");
                let subject = c["commit"]["message"].as_str().unwrap_or("").lines().next().unwrap_or("");
                println!("  {sha} {subject}");
            }
            if commits.len() > LIST_MAX {
                println!("  … and {} more", commits.len() - LIST_MAX);
            }
            let files = v["files"].as_array().cloned().unwrap_or_default();
            let used: Vec<&str> = files.iter().filter_map(|f| f["filename"].as_str()).filter(|p| is_used(p)).collect();
            println!("{} of {} changed file(s) are in crates we use ({}):", used.len(), files.len(), USED_CRATES.join(", "));
            for p in used.iter().take(LIST_MAX) {
                println!("  {p}");
            }
        }
        Err(e) => println!("could not list the commits in between: {e}"),
    }
    println!("update with: cargo xtask filmcraft --update");
    if strict { Err("the FilmCraft pin is behind main".into()) } else { Ok(()) }
}

fn update(root: &Path, rev_arg: Option<&str>, no_test: bool) -> Result<(), String> {
    let (pin, names) = read_pin(root)?;
    let target = match rev_arg {
        Some(r) => full_rev(r)?,
        None => head_of_main()?,
    };
    if target == pin {
        println!("already at {}", short(&pin));
        return Ok(());
    }
    let (ours, old, new) = (our_rust_version(root)?, rust_version_at(&pin)?, rust_version_at(&target)?);
    if new > old.max(ours) {
        return Err(format!(
            "{} needs Rust {}.{}, above ours ({}.{}) and the old pin's ({}.{}): bump rust-version in Cargo.toml deliberately first",
            short(&target),
            new.0,
            new.1,
            ours.0,
            ours.1,
            old.0,
            old.1
        ));
    }
    for file in PIN_FILES {
        let path = root.join(file);
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let new_text = repin(&text, &pin, &target);
        if new_text != text {
            std::fs::write(&path, new_text).map_err(|e| format!("{file}: {e}"))?;
            println!("{file}: {} -> {}", short(&pin), short(&target));
        }
    }
    let mut cargo = crate::cargo();
    cargo.arg("update");
    for n in &names {
        cargo.args(["-p", n]);
    }
    crate::run(cargo, "cargo update (the filmcraft-* packages)")?;
    let diff = Command::new("git")
        .current_dir(root)
        .args(["diff", "--stat", "--", "Cargo.toml", "Cargo.lock"])
        .output()
        .map_err(|e| format!("git diff: {e}"))?;
    print!("{}", String::from_utf8_lossy(&diff.stdout));
    if no_test {
        println!("pin moved to {}; tests left to CI (--no-test)", short(&target));
        return Ok(());
    }
    for args in VIDEO_TESTS {
        let mut c = crate::cargo();
        c.args(*args);
        crate::run(c, &format!("cargo {}", args.join(" ")))?;
    }
    println!("pin moved to {}; video tests pass. Run `cargo xtask ci` before committing.", short(&target));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"
# FilmCraft: one rev for all (filmcraft-codecs is banned)
filmcraft-isobmff = { git = "https://github.com/storytold/filmcraft", rev = "d643a886650bfbf16f24762274054575aa1e8f20" }
filmcraft-h264 = { git = "https://github.com/storytold/filmcraft", rev = "d643a886650bfbf16f24762274054575aa1e8f20" }
craft-ui = { git = "https://github.com/storytold/craft-ui", rev = "d643a886650bfbf16f24762274054575aa1e8f20" }
# filmcraft-old = { git = "https://github.com/storytold/filmcraft", rev = "0000000000000000000000000000000000000000" }
rust-version = "1.90"
"#;

    #[test]
    fn the_pins_are_the_filmcraft_lines_only() {
        let p = pins(MANIFEST);
        assert_eq!(p.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["filmcraft-isobmff", "filmcraft-h264"], "not craft-ui, not a comment");
        assert_eq!(single_rev(&p).unwrap(), "d643a886650bfbf16f24762274054575aa1e8f20");
        assert!(single_rev(&[]).is_err());
        let split = vec![("filmcraft-a".to_string(), "aa".to_string()), ("filmcraft-b".to_string(), "bb".to_string())];
        assert!(single_rev(&split).unwrap_err().contains("filmcraft-b"), "pins that drift apart are refused");
    }

    #[test]
    fn repinning_touches_filmcraft_lines_and_nothing_else() {
        let new = "1".repeat(40);
        let out = repin(MANIFEST, "d643a886650bfbf16f24762274054575aa1e8f20", &new);
        assert_eq!(pins(&out).iter().filter(|(_, r)| *r == new).count(), 2);
        assert!(out.contains("craft-ui = { git = \"https://github.com/storytold/craft-ui\", rev = \"d643a886"), "craft-ui keeps its rev");
        assert_eq!(out.lines().count(), MANIFEST.lines().count(), "same lines");
        assert_eq!(repin(&out, &new, "d643a886650bfbf16f24762274054575aa1e8f20"), MANIFEST, "and back");
    }

    #[test]
    fn rust_versions_compare_as_numbers() {
        assert_eq!(rust_version(MANIFEST), Some((1, 90)));
        assert_eq!(rust_version("[workspace.package]\nrust-version = \"1.100\"\n"), Some((1, 100)));
        assert!(rust_version("rust-version = \"1.100\"").unwrap() > rust_version("rust-version = \"1.95\"").unwrap(), "1.100 is above 1.95");
        assert_eq!(rust_version("no version here"), None);
        assert_eq!(rust_version("rust-version = \"abc\""), None);
    }

    #[test]
    fn only_the_crates_we_use_count() {
        assert!(is_used("crates/h264/src/lib.rs") && is_used("crates/isobmff/src/demux.rs"));
        assert!(!is_used("crates/platform/src/lib.rs") && !is_used("docs/h264.md") && !is_used("crates/"));
        assert!(is_rev(&"a".repeat(40)) && !is_rev("abc") && !is_rev(&"g".repeat(40)));
        assert_eq!((short("abc"), short(&"a".repeat(40)).len()), ("abc", 12), "a hand-edited short rev must not panic");
    }
}
