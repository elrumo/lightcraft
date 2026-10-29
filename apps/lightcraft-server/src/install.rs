//! `lightbites-server install` / `uninstall`: run the server on a Mac or a Linux desktop without Docker or a terminal left open.
//!
//! macOS: `install` copies the program and its `web` folder to `~/Library/Application Support/LightCraft Server/program`
//! and starts it with a launchd agent (`~/Library/LaunchAgents/ai.lightcraft.server.plist`): it runs at login, is
//! restarted if it stops, and keeps its data in `…/LightCraft Server/data`. Linux: the same with a systemd user service
//! (`~/.config/systemd/user/lightbites-server.service`) and `~/.local/share/lightbites-server`. `uninstall` stops it and
//! removes the agent and the program; the data stays. `start`, `stop` and `status` control the installed agent without
//! removing it (a stopped server stays stopped across logins until `start`); `status --porcelain` prints one word
//! (`running`, `stopped` or `not-installed`) for the menu-bar app and scripts. On a Mac, `install` also starts the
//! menu-bar app that came in the zip with a second launchd agent (`ai.lightcraft.server.menubar`), so its icon is there
//! now and at every login; `uninstall` removes it. Windows uses its own service manager
//! (docs/self-hosting.md).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const LABEL: &str = "ai.lightcraft.server";
const MENUBAR_LABEL: &str = "ai.lightcraft.server.menubar";
const UNIT: &str = "lightbites-server.service";
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

/// The launchd agent that shows the menu-bar icon at login. No `KeepAlive`: the icon's Quit item must stay quit.
fn menubar_plist(program: &Path, port: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n  <key>Label</key><string>{MENUBAR_LABEL}</string>\n  <key>ProgramArguments</key>\n  <array>\n    <string>{}</string>\n    <string>--port</string>\n    <string>{}</string>\n  </array>\n  <key>RunAtLoad</key><true/>\n  <key>ProcessType</key><string>Interactive</string>\n</dict>\n</plist>\n",
        xml(&program.to_string_lossy()),
        xml(port)
    )
}

/// One word of a systemd `ExecStart=` line: quoted, with the characters systemd expands escaped.
fn unit_word(w: &str) -> String {
    let mut o = String::from("\"");
    for c in w.chars() {
        match c {
            '\\' => o.push_str("\\\\"),
            '"' => o.push_str("\\\""),
            '%' => o.push_str("%%"),
            '$' => o.push_str("$$"),
            '\n' => o.push(' '),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// The systemd user service that runs `program serve …` at login and keeps it running.
fn unit(program: &Path, data: &Path, listen: &str, web: &Path, log: &Path) -> String {
    let words = [
        program.to_string_lossy(),
        "serve".into(),
        "--data".into(),
        data.to_string_lossy(),
        "--listen".into(),
        listen.into(),
        "--web".into(),
        web.to_string_lossy(),
    ];
    let exec = words.iter().map(|w| unit_word(w)).collect::<Vec<_>>().join(" ");
    let log = format!("append:{}", log.to_string_lossy().replace('%', "%%"));
    format!(
        "[Unit]\nDescription=LightBites Server\nAfter=network-online.target\n\n[Service]\nExecStart={exec}\nRestart=always\nRestartSec=3\nStandardOutput={log}\nStandardError={log}\n\n[Install]\nWantedBy=default.target\n"
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
    /// The launchd plist (macOS) or the systemd unit (Linux).
    agent: PathBuf,
    /// The menu-bar icon's launchd plist (macOS; unused on Linux).
    menubar_agent: PathBuf,
    log: PathBuf,
    /// launchd's `gui/<uid>` domain (macOS); empty on Linux.
    domain: String,
}

/// `s` as one word for a POSIX shell (single quotes, a quote inside as `'\''`): the install folder has a space on a Mac.
fn shell_quote(s: &Path) -> String {
    format!("'{}'", s.to_string_lossy().replace('\'', "'\\''"))
}

/// A line to paste in a terminal that runs `verb` on the installed program. The install folder is not on `PATH`, so a
/// bare `lightbites-server start` would not be found.
fn command_line(p: &Paths, verb: &str) -> String {
    format!("{} {verb}", shell_quote(&p.app.join("program/lightbites-server")))
}

fn home() -> Result<PathBuf, String> {
    std::env::var("HOME").ok().filter(|h| !h.is_empty()).map(PathBuf::from).ok_or_else(|| "$HOME is not set".into())
}

fn paths() -> Result<Paths, String> {
    let home = home()?;
    if cfg!(target_os = "macos") {
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
            agent: home.join(format!("Library/LaunchAgents/{LABEL}.plist")),
            menubar_agent: home.join(format!("Library/LaunchAgents/{MENUBAR_LABEL}.plist")),
            log: home.join("Library/Logs/LightCraft Server/server.log"),
            domain: format!("gui/{uid}"),
        })
    } else if cfg!(target_os = "linux") {
        let xdg = |var: &str, fallback: &str| {
            std::env::var(var).ok().filter(|d| d.starts_with('/')).map(PathBuf::from).unwrap_or_else(|| home.join(fallback))
        };
        Ok(Paths {
            app: xdg("XDG_DATA_HOME", ".local/share").join("lightbites-server"),
            agent: xdg("XDG_CONFIG_HOME", ".config").join("systemd/user").join(UNIT),
            menubar_agent: PathBuf::new(),
            log: xdg("XDG_STATE_HOME", ".local/state").join("lightbites-server/server.log"),
            domain: String::new(),
        })
    } else {
        Err("install, start, stop and status are for macOS and Linux; on Windows, run the server as a service as docs/self-hosting.md describes"
            .into())
    }
}

fn systemctl(args: &[&str]) -> bool {
    let mut all = vec!["--user"];
    all.extend_from_slice(args);
    run("systemctl", &all)
}

/// The agent's definition for this system.
fn agent_file(p: &Paths, program: &Path, data: &Path, listen: &str, web: &Path) -> String {
    if cfg!(target_os = "macos") { plist(program, data, listen, web, &p.log) } else { unit(program, data, listen, web, &p.log) }
}

/// Stops the agent for now (it may still start at the next login).
fn unload(p: &Paths) {
    if cfg!(target_os = "macos") {
        run("launchctl", &["bootout", &format!("{}/{LABEL}", p.domain)]);
    } else {
        systemctl(&["stop", UNIT]);
    }
}

/// Quits the menu-bar icon's agent (macOS; nothing to quit the first time).
fn unload_menubar(p: &Paths) {
    if cfg!(target_os = "macos") {
        run("launchctl", &["bootout", &format!("{}/{MENUBAR_LABEL}", p.domain)]);
    }
}

/// Starts the menu-bar icon now and at every login. Best effort: the server works without its icon.
fn load_menubar(p: &Paths, program_dir: &Path, port: &str) {
    let program = program_dir.join("lightbites-menubar");
    if !cfg!(target_os = "macos") || !program.is_file() {
        return;
    }
    if std::fs::write(&p.menubar_agent, menubar_plist(&program, port)).is_ok() {
        let path = p.menubar_agent.to_string_lossy();
        let _ = run("launchctl", &["bootstrap", &p.domain, &path]) || run("launchctl", &["load", "-w", &path]);
    }
}

/// Lets the agent start at login (again).
fn enable(p: &Paths) {
    if cfg!(target_os = "macos") {
        run("launchctl", &["enable", &format!("{}/{LABEL}", p.domain)]);
    } else {
        systemctl(&["enable", UNIT]);
    }
}

/// Keeps the agent from starting at login.
fn disable(p: &Paths) {
    if cfg!(target_os = "macos") {
        run("launchctl", &["disable", &format!("{}/{LABEL}", p.domain)]);
    } else {
        systemctl(&["disable", UNIT]);
    }
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
    unload(&p);
    unload_menubar(&p);

    if exe != installed_exe {
        std::fs::create_dir_all(&program_dir).map_err(|e| format!("{}: {e}", program_dir.display()))?;
        std::fs::copy(&exe, &installed_exe).map_err(|e| format!("{}: {e}", installed_exe.display()))?;
        // releases before the rename installed it as `lightcraft-server`
        let _ = std::fs::remove_file(program_dir.join("lightcraft-server"));
    }
    // the Mac zips carry the menu-bar app beside the server: keep it with the installed program
    if let Some(menubar) = exe.parent().map(|d| d.join("lightbites-menubar")).filter(|m| m.is_file()) {
        let _ = std::fs::copy(&menubar, program_dir.join("lightbites-menubar"));
    }
    if let Some(src) = web_src.as_ref().filter(|s| s.canonicalize().ok().as_deref() != installed_web.canonicalize().ok().as_deref()) {
        let _ = std::fs::remove_dir_all(&installed_web);
        copy_dir(src, &installed_web, 0)?;
    } else if web_src.is_none() {
        eprintln!("no `web` folder next to the program: the web app won't be served (the admin page and the apps still work)");
    }
    // a browser download is marked "from the internet"; the copy no longer needs it
    if cfg!(target_os = "macos") {
        run("xattr", &["-dr", "com.apple.quarantine", &program_dir.to_string_lossy()]);
    }

    for dir in [p.agent.parent(), p.log.parent(), Some(data.as_path())].into_iter().flatten() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let _ = std::fs::remove_file(&p.log);
    std::fs::write(&p.agent, agent_file(&p, &installed_exe, &data, listen, &installed_web)).map_err(|e| format!("{}: {e}", p.agent.display()))?;
    enable(&p);
    load(&p)?;
    let port = listen.rsplit(':').next().unwrap_or("8080");
    load_menubar(&p, &program_dir, port);

    println!("LightCraft Server is installed and running (it starts at every login).");
    println!("  admin page: http://localhost:{port}/admin");
    println!("  data:       {}", data.display());
    println!("  log:        {}", p.log.display());
    println!("  stop/start: {} stop | start | status", shell_quote(&installed_exe));
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

/// Starts the agent now (macOS: bootstrap; Linux: reload the unit file, then `enable --now`).
fn load(p: &Paths) -> Result<(), String> {
    if cfg!(target_os = "macos") {
        let plist_path = p.agent.to_string_lossy();
        if run("launchctl", &["bootstrap", &p.domain, &plist_path]) || run("launchctl", &["load", "-w", &plist_path]) {
            return Ok(());
        }
    } else {
        systemctl(&["daemon-reload"]);
        if systemctl(&["enable", "--now", UNIT]) {
            return Ok(());
        }
    }
    Err(format!("the service manager could not start the server; its agent is {}", p.agent.display()))
}

fn running(p: &Paths) -> bool {
    if cfg!(target_os = "macos") {
        run("launchctl", &["print", &format!("{}/{LABEL}", p.domain)])
    } else {
        systemctl(&["is-active", "--quiet", UNIT])
    }
}

fn installed(p: &Paths) -> Result<(), String> {
    if p.agent.is_file() { Ok(()) } else { Err("LightCraft Server is not installed here: run the installer first (see docs/self-hosting.md)".into()) }
}

/// Starts the installed server (and lets it start at login again).
pub fn start() -> Result<(), String> {
    let p = paths()?;
    installed(&p)?;
    enable(&p);
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
    disable(&p);
    unload(&p);
    println!("LightCraft Server is stopped. Start it again with: {}", command_line(&p, "start"));
    Ok(())
}

/// Ok when the server is running; an error (exit status 1) when it is not. `porcelain` prints one word
/// (`running`, `stopped` or `not-installed`) and always succeeds, for the menu-bar app and scripts.
pub fn status(porcelain: bool) -> Result<(), String> {
    let p = paths()?;
    if porcelain {
        println!(
            "{}",
            if installed(&p).is_err() {
                "not-installed"
            } else if running(&p) {
                "running"
            } else {
                "stopped"
            }
        );
        return Ok(());
    }
    installed(&p)?;
    if running(&p) {
        println!("LightCraft Server is running (log: {}).", p.log.display());
        Ok(())
    } else {
        Err(format!("LightCraft Server is stopped. Start it with: {}", command_line(&p, "start")))
    }
}

pub fn uninstall() -> Result<(), String> {
    let p = paths()?;
    disable(&p);
    unload(&p);
    enable(&p);
    unload_menubar(&p);
    let _ = std::fs::remove_file(&p.agent);
    if cfg!(target_os = "macos") {
        let _ = std::fs::remove_file(&p.menubar_agent);
    }
    if !cfg!(target_os = "macos") {
        systemctl(&["daemon-reload"]);
    }
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
    fn menubar_plist_runs_the_icon_at_login_with_the_port_and_without_keepalive() {
        let p = menubar_plist(Path::new("/a & b/lightbites-menubar"), "9090");
        assert!(p.contains("<string>/a &amp; b/lightbites-menubar</string>\n    <string>--port</string>\n    <string>9090</string>"));
        assert!(p.contains("<key>RunAtLoad</key><true/>"));
        assert!(!p.contains("KeepAlive"));
        if cfg!(target_os = "macos") {
            let file = std::env::temp_dir().join(format!("lc-menubar-plist-{}.plist", std::process::id()));
            std::fs::write(&file, &p).unwrap();
            assert!(run("plutil", &["-lint", &file.to_string_lossy()]), "plutil rejected:\n{p}");
            std::fs::remove_file(&file).unwrap();
        }
    }

    #[test]
    fn unit_quotes_words_and_escapes_what_systemd_expands() {
        let u = unit(Path::new("/a b/lightbites-server"), Path::new("/d%1/$x"), "0.0.0.0:8080", Path::new("/w"), Path::new("/l 100%.log"));
        assert!(
            u.contains("ExecStart=\"/a b/lightbites-server\" \"serve\" \"--data\" \"/d%%1/$$x\" \"--listen\" \"0.0.0.0:8080\" \"--web\" \"/w\"\n"),
            "{u}"
        );
        assert!(u.contains("StandardOutput=append:/l 100%%.log\n"));
        assert!(u.contains("Restart=always") && u.contains("WantedBy=default.target"));
        assert_eq!(unit_word("a\"b\\c\nd"), "\"a\\\"b\\\\c d\"");
    }

    #[test]
    fn the_commands_to_paste_name_the_installed_program_in_full() {
        let p = Paths {
            app: PathBuf::from("/Users/a b/Library/Application Support/LightCraft Server"),
            agent: PathBuf::new(),
            menubar_agent: PathBuf::new(),
            log: PathBuf::new(),
            domain: String::new(),
        };
        // the install folder is not on PATH and has a space in it
        assert_eq!(command_line(&p, "start"), "'/Users/a b/Library/Application Support/LightCraft Server/program/lightbites-server' start");
        assert_eq!(shell_quote(Path::new("/it's/x")), "'/it'\\''s/x'");
        // what the shell makes of it: one word, the same path
        if cfg!(unix) {
            let script = format!("printf '%s' {}", shell_quote(Path::new("/it's a $x/y")));
            let out = Command::new("sh").arg("-c").arg(script).output().unwrap();
            assert_eq!(String::from_utf8_lossy(&out.stdout), "/it's a $x/y");
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
