//! `lightbites-server install` / `uninstall`: run the server on a Mac without Docker or a terminal left open.
//!
//! `install` copies the program and its `web` folder to `~/Library/Application Support/LightCraft Server/program`
//! and starts it with a launchd agent (`~/Library/LaunchAgents/ai.lightcraft.server.plist`): it runs at login, is
//! restarted if it stops, and keeps its data in `…/LightCraft Server/data`. `uninstall` stops it and removes the
//! agent and the program; the data stays. `start`, `stop` and `status` control the installed agent without removing
//! it (a stopped server stays stopped across logins until `start`). Other systems use their own service manager (docs/self-hosting.md).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const LABEL: &str = "ai.lightcraft.server";
/// Most nested folders `copy_dir` follows (the web build is two or three deep).
const MAX_DEPTH: usize = 8;

fn xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// The launchd agent that runs `program serve …` at login and keeps it running.
fn plist(program: &Path, data: &Path, listen: &str, web: &Path, log: &Path) -> String {
    let args = [
        program.to_string_lossy(),
        "serve".into(),
        "--data".into(),
        data.to_string_lossy(),
        "--listen".into(),
        listen.into(),
        "--web".into(),
        web.to_string_lossy(),
    ];
    let args: String = args.iter().map(|a| format!("    <string>{}</string>\n", xml(a))).collect();
    let log = xml(&log.to_string_lossy());
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n  <key>Label</key><string>{LABEL}</string>\n  <key>ProgramArguments</key>\n  <array>\n{args}  </array>\n  <key>RunAtLoad</key><true/>\n  <key>KeepAlive</key><true/>\n  <key>StandardOutPath</key><string>{log}</string>\n  <key>StandardErrorPath</key><string>{log}</string>\n</dict>\n</plist>\n"
    )
}

fn copy_dir(from: &Path, to: &Path, depth: usize) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err(format!("{}: folders nested too deeply", from.display()));
    }
    std::fs::create_dir_all(to).map_err(|e| format!("{}: {e}", to.display()))?;
    for entry in std::fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))?.flatten() {
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() {
            copy_dir(&src, &dst, depth + 1)?;
        } else if kind.is_file() {
            std::fs::copy(&src, &dst).map_err(|e| format!("{}: {e}", src.display()))?;
        }
    }
    Ok(())
}

fn run(cmd: &str, args: &[&str]) -> bool {
    Command::new(cmd).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

struct Paths {
    app: PathBuf,
    plist: PathBuf,
    log: PathBuf,
    domain: String,
}

fn paths() -> Result<Paths, String> {
    if !cfg!(target_os = "macos") {
        return Err("install and uninstall are for macOS; on Linux and Windows, run the server as a service as docs/self-hosting.md describes".into());
    }
    let home = std::env::var("HOME").ok().filter(|h| !h.is_empty()).map(PathBuf::from).ok_or("$HOME is not set")?;
    let uid = Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|u| u.trim().to_string())
        .filter(|u| !u.is_empty())
        .ok_or("can't tell which user this is (`id -u` failed)")?;
    Ok(Paths {
        app: home.join("Library/Application Support/LightCraft Server"),
        plist: home.join(format!("Library/LaunchAgents/{LABEL}.plist")),
        log: home.join("Library/Logs/LightCraft Server/server.log"),
        domain: format!("gui/{uid}"),
    })
}

pub fn install(data: Option<PathBuf>, listen: &str, web: Option<PathBuf>) -> Result<(), String> {
    let p = paths()?;
    let exe = std::env::current_exe().map_err(|e| format!("can't find this program: {e}"))?.canonicalize().map_err(|e| e.to_string())?;
    let program_dir = p.app.join("program");
    let installed_exe = program_dir.join("lightbites-server");
    let installed_web = program_dir.join("web");
    let web_src = web.or_else(|| exe.parent().map(|d| d.join("web"))).filter(|w| w.is_dir());
    let data = match data {
        Some(d) => std::path::absolute(d).map_err(|e| e.to_string())?,
        None => p.app.join("data"),
    };

    // stop the old copy before replacing it (nothing to stop the first time)
    let target = format!("{}/{LABEL}", p.domain);
    run("launchctl", &["bootout", &target]);

    if exe != installed_exe {
        std::fs::create_dir_all(&program_dir).map_err(|e| format!("{}: {e}", program_dir.display()))?;
        std::fs::copy(&exe, &installed_exe).map_err(|e| format!("{}: {e}", installed_exe.display()))?;
        // releases before the rename installed it as `lightcraft-server`
        let _ = std::fs::remove_file(program_dir.join("lightcraft-server"));
    }
    if let Some(src) = web_src.as_ref().filter(|s| s.canonicalize().ok().as_deref() != installed_web.canonicalize().ok().as_deref()) {
        let _ = std::fs::remove_dir_all(&installed_web);
        copy_dir(src, &installed_web, 0)?;
    } else if web_src.is_none() {
        eprintln!("no `web` folder next to the program: the web app won't be served (the admin page and the apps still work)");
    }
    // a browser download is marked "from the internet"; the copy no longer needs it
    run("xattr", &["-dr", "com.apple.quarantine", &program_dir.to_string_lossy()]);

    for dir in [p.plist.parent(), p.log.parent(), Some(data.as_path())].into_iter().flatten() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let _ = std::fs::remove_file(&p.log);
    std::fs::write(&p.plist, plist(&installed_exe, &data, listen, &installed_web, &p.log)).map_err(|e| format!("{}: {e}", p.plist.display()))?;
    run("launchctl", &["enable", &target]);
    load(&p)?;

    let port = listen.rsplit(':').next().unwrap_or("8080");
    println!("LightCraft Server is installed and running (it starts at every login).");
    println!("  admin page: http://localhost:{port}/admin");
    println!("  data:       {}", data.display());
    println!("  log:        {}", p.log.display());
    println!("  stop/start: lightbites-server stop | start | status  (the program is {})", installed_exe.display());
    // the first start logs the one-time code that claims the server: show it
    let until = Instant::now() + Duration::from_secs(20);
    while Instant::now() < until {
        if let Some(line) = std::fs::read_to_string(&p.log).ok().and_then(|l| l.lines().find(|l| l.contains("setup code")).map(str::to_string)) {
            println!("{line}");
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    println!("(no setup code yet: it is in the log once the server has started, unless an admin already exists)");
    Ok(())
}

fn load(p: &Paths) -> Result<(), String> {
    let plist_path = p.plist.to_string_lossy();
    if run("launchctl", &["bootstrap", &p.domain, &plist_path]) || run("launchctl", &["load", "-w", &plist_path]) {
        Ok(())
    } else {
        Err(format!("launchctl could not start the server; its agent is {}", p.plist.display()))
    }
}

fn running(p: &Paths) -> bool {
    run("launchctl", &["print", &format!("{}/{LABEL}", p.domain)])
}

fn installed(p: &Paths) -> Result<(), String> {
    if p.plist.is_file() { Ok(()) } else { Err("LightCraft Server is not installed here: run the installer first (see docs/self-hosting.md)".into()) }
}

/// Starts the installed server (and lets it start at login again).
pub fn start() -> Result<(), String> {
    let p = paths()?;
    installed(&p)?;
    run("launchctl", &["enable", &format!("{}/{LABEL}", p.domain)]);
    if running(&p) {
        println!("LightCraft Server is already running.");
        return Ok(());
    }
    load(&p)?;
    println!("LightCraft Server is running (it starts at every login).");
    Ok(())
}

/// Stops the server and keeps it stopped, also after the next login, until `start`.
pub fn stop() -> Result<(), String> {
    let p = paths()?;
    installed(&p)?;
    let target = format!("{}/{LABEL}", p.domain);
    run("launchctl", &["disable", &target]);
    run("launchctl", &["bootout", &target]);
    println!("LightCraft Server is stopped. Start it again with: lightbites-server start");
    Ok(())
}

/// Ok when the server is running; an error (exit status 1) when it is not.
pub fn status() -> Result<(), String> {
    let p = paths()?;
    installed(&p)?;
    if running(&p) {
        println!("LightCraft Server is running (log: {}).", p.log.display());
        Ok(())
    } else {
        Err("LightCraft Server is stopped. Start it with: lightbites-server start".into())
    }
}

pub fn uninstall() -> Result<(), String> {
    let p = paths()?;
    run("launchctl", &["bootout", &format!("{}/{LABEL}", p.domain)]);
    run("launchctl", &["enable", &format!("{}/{LABEL}", p.domain)]);
    let _ = std::fs::remove_file(&p.plist);
    let _ = std::fs::remove_dir_all(p.app.join("program"));
    println!(
        "LightCraft Server is stopped and removed. Its data is kept in {}; delete that folder to remove your libraries too.",
        p.app.join("data").display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plist_escapes_paths_and_lists_the_serve_arguments() {
        let p = plist(Path::new("/a & b/lightbites-server"), Path::new("/d"), "0.0.0.0:8080", Path::new("/w<1>"), Path::new("/l.log"));
        assert!(p.contains("<string>/a &amp; b/lightbites-server</string>\n    <string>serve</string>"));
        assert!(p.contains("<string>/w&lt;1&gt;</string>"));
        assert!(p.contains("<key>KeepAlive</key><true/>"));
        // macOS's own checker agrees it is a property list
        if cfg!(target_os = "macos") {
            let file = std::env::temp_dir().join(format!("lc-plist-{}.plist", std::process::id()));
            std::fs::write(&file, &p).unwrap();
            assert!(run("plutil", &["-lint", &file.to_string_lossy()]), "plutil rejected:\n{p}");
            std::fs::remove_file(&file).unwrap();
        }
    }

    #[test]
    fn copy_dir_stops_at_the_depth_limit() {
        let root = std::env::temp_dir().join(format!("lc-install-{}", std::process::id()));
        let deep = (0..MAX_DEPTH + 2).fold(root.join("src"), |d, i| d.join(format!("d{i}")));
        std::fs::create_dir_all(&deep).unwrap();
        assert!(copy_dir(&root.join("src"), &root.join("dst"), 0).is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
