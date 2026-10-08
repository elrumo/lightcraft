//! `lightcraft-server`: the self-hosted LightCraft sync server (see `docs/sync.md`).
//!
//! ```text
//! lightcraft-server serve [--data DIR] [--listen HOST:PORT] [--web DIR] [--scan-interval MINUTES]
//! lightcraft-server user add NAME [--admin]   (password: typed, stdin, or $LIGHTCRAFT_PASSWORD)
//! lightcraft-server user passwd NAME
//! lightcraft-server user admin NAME on|off
//! lightcraft-server user remove NAME
//! lightcraft-server user list
//! lightcraft-server device list NAME
//! lightcraft-server device revoke NAME ID
//! lightcraft-server folder add NAME PATH [--name FOLDER]
//! lightcraft-server folder remove NAME FOLDER
//! lightcraft-server folder list [NAME]
//! lightcraft-server folder ignore NAME [PATTERN…]
//! lightcraft-server scan [NAME]
//! lightcraft-server gc [--dry-run]
//! lightcraft-server health
//! ```
//!
//! Admins also manage users and devices from the server's web page, `/admin` (until the first
//! admin exists, it asks for the setup code the server logs at start).
//!
//! `--data` defaults to `$LIGHTCRAFT_DATA` or `./lightcraft-data`, `--listen` to
//! `$LIGHTCRAFT_LISTEN` or `127.0.0.1:8080` (only this computer: put a TLS proxy in front, or
//! listen on a private network such as Tailscale).
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::io::{BufRead, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use lightcraft_server::{Config, Server, accounts, folders, gc};

const USAGE: &str = "usage:
  lightcraft-server serve [--data DIR] [--listen HOST:PORT] [--web DIR] [--scan-interval MINUTES]
  lightcraft-server user add NAME [--admin]   (password: typed, stdin or $LIGHTCRAFT_PASSWORD)
  lightcraft-server user passwd|remove NAME
  lightcraft-server user admin NAME on|off    (admins manage the server at /admin)
  lightcraft-server user faces NAME on|off    (find the people in NAME's photos; needs `model download --faces`)
  lightcraft-server user list
  lightcraft-server device list NAME
  lightcraft-server device revoke NAME ID
  lightcraft-server folder add NAME PATH [--name FOLDER]   (a photo folder on this server, read in place)
  lightcraft-server folder remove NAME FOLDER
  lightcraft-server folder list [NAME]
  lightcraft-server folder ignore NAME [PATTERN…]   (names the scan skips, like '*.fcpbundle'; none: show them, '': clear)
  lightcraft-server scan [NAME]                (read the library folders now)
  lightcraft-server model status|download [--accept-licences] [--text] [--faces] [--vision-dir DIR]   (the search model: about 1.5 GB, Google's Apache License 2.0; --text adds the models that read the text in photos: about 31 MB, Baidu's Apache License 2.0; --faces adds the models that find and tell apart faces: about 39 MB, MIT and Apache 2.0)
  lightcraft-server gc [--dry-run]
  lightcraft-server health                     (is the server answering? exit status)
options (any command): --data DIR (default $LIGHTCRAFT_DATA or ./lightcraft-data)
";

/// Log to stderr (`$LIGHTCRAFT_LOG`: error, warn, info (default), debug).
struct Stderr;

static LOGGER: Stderr = Stderr;

impl log::Log for Stderr {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= log::max_level()
    }
    fn log(&self, r: &log::Record) {
        if self.enabled(r.metadata()) {
            eprintln!("{} {}", r.level(), r.args());
        }
    }
    fn flush(&self) {}
}

fn option(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

/// The arguments that aren't options or option values.
fn positional(args: &[String]) -> Vec<&str> {
    let mut out = Vec::new();
    let mut skip = false;
    for a in args {
        if skip {
            skip = false;
        } else if matches!(a.as_str(), "--data" | "--listen" | "--web" | "--name" | "--scan-interval" | "--vision-dir") {
            skip = true;
        } else if !a.starts_with("--") {
            out.push(a.as_str());
        }
    }
    out
}

fn password() -> Result<String, String> {
    if let Ok(p) = std::env::var("LIGHTCRAFT_PASSWORD")
        && !p.is_empty()
    {
        return Ok(p);
    }
    // typed at a terminal: don't show it (best effort, `stty` where there is one)
    let tty = std::io::stdin().is_terminal();
    let echo = |on: bool| {
        if tty {
            let _ = std::process::Command::new("stty").arg(if on { "echo" } else { "-echo" }).stdin(std::process::Stdio::inherit()).status();
        }
    };
    eprint!("password: ");
    echo(false);
    let mut line = String::new();
    let read = std::io::stdin().lock().read_line(&mut line);
    echo(true);
    eprintln!();
    read.map_err(|e| e.to_string())?;
    let p = line.trim_end_matches(['\r', '\n']).to_string();
    if p.is_empty() { Err("no password given".into()) } else { Ok(p) }
}

fn run(args: &[String]) -> Result<(), String> {
    let data = PathBuf::from(option(args, "--data").or_else(|| std::env::var("LIGHTCRAFT_DATA").ok()).unwrap_or_else(|| "lightcraft-data".into()));
    let pos = positional(args);
    match pos.as_slice() {
        ["serve"] => {
            let listen = option(args, "--listen").or_else(|| std::env::var("LIGHTCRAFT_LISTEN").ok()).unwrap_or_else(|| "127.0.0.1:8080".into());
            let web = option(args, "--web").or_else(|| std::env::var("LIGHTCRAFT_WEB").ok()).map(PathBuf::from).filter(|w| w.is_dir());
            let users = accounts::read_users(&data)?;
            if users.users.is_empty() {
                log::warn!("no users yet: add them on the admin page (/admin) or with `lightcraft-server user add NAME --data {}`", data.display());
            }
            // minutes between scans of the library folders (0: at start and on demand only)
            let minutes = option(args, "--scan-interval")
                .or_else(|| std::env::var("LIGHTCRAFT_SCAN_INTERVAL").ok())
                .map(|m| m.trim().parse::<u64>().map_err(|_| format!("--scan-interval takes minutes, not `{m}`")))
                .transpose()?
                .unwrap_or(15);
            let threads = std::env::var("LIGHTCRAFT_PREVIEW_THREADS").ok().and_then(|t| t.trim().parse::<usize>().ok());
            let mut cfg = Config::new(data.clone(), listen);
            cfg.vision_dir = Some(vision_dir(args, &data));
            cfg.web = web.clone();
            cfg.scan_interval = (minutes > 0).then(|| Duration::from_secs(minutes.saturating_mul(60)));
            cfg.preview_threads = threads.unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| (n.get() / 2).clamp(1, 4)));
            if let Some(n) = std::env::var("LIGHTCRAFT_RENDER_THREADS").ok().and_then(|t| t.trim().parse::<usize>().ok()) {
                cfg.render_threads = n;
            }
            let s = Server::start(cfg)?;
            log::info!(
                "serving {} on http://{}{}",
                data.display(),
                s.addr(),
                web.map(|w| format!(" (web build: {})", w.display())).unwrap_or_default()
            );
            s.wait();
            Ok(())
        }
        ["user", "add", name] => {
            accounts::set_user(&data, name, &password()?, false)?;
            let admin = args.iter().any(|a| a == "--admin");
            if admin {
                accounts::set_admin(&data, name, true)?;
            }
            println!("added {name}{}", if admin { " (admin)" } else { "" });
            Ok(())
        }
        ["user", "admin", name, on @ ("on" | "off")] => {
            accounts::set_admin(&data, name, *on == "on")?;
            println!("{name} {} an admin", if *on == "on" { "is" } else { "is no longer" });
            Ok(())
        }
        ["user", "faces", name, on @ ("on" | "off")] => {
            accounts::set_faces(&data, name, *on == "on")?;
            println!("{name}: finding the people in their photos is {}", if *on == "on" { "on (once the face models are installed)" } else { "off" });
            Ok(())
        }
        ["user", "passwd", name] => {
            accounts::set_user(&data, name, &password()?, true)?;
            println!("changed {name}'s password");
            Ok(())
        }
        ["user", "remove", name] => {
            accounts::remove_user(&data, name)?;
            println!("removed {name} (their files stay in {})", accounts::user_dir(&data, name).display());
            Ok(())
        }
        ["user", "list"] => {
            for (name, u) in accounts::read_users(&data)?.users {
                println!("{name}{}{}", if u.admin { "\tadmin" } else { "" }, if u.faces { "\tfaces" } else { "" });
            }
            Ok(())
        }
        ["device", "list", name] => {
            for d in accounts::devices(&data, name)?.devices {
                println!("{}\t{}\tspace {}", d.id, d.name, d.space);
            }
            Ok(())
        }
        ["device", "revoke", name, id] => {
            let id = id.parse().map_err(|_| format!("not a device id: {id}"))?;
            accounts::revoke(&data, name, id)?;
            println!("signed out device {id}");
            Ok(())
        }
        ["folder", "add", name, path] => {
            let f = accounts::add_folder(&data, name, path, option(args, "--name").as_deref())?;
            println!("{name} has the library folder {} ({}): its photos are read in place, never copied or changed", f.name, f.path);
            request_scan(&data, name);
            Ok(())
        }
        ["folder", "remove", name, folder] => {
            accounts::remove_folder(&data, name, folder)?;
            println!("{name} no longer has the library folder {folder} (its photos stay in the library)");
            Ok(())
        }
        ["folder", "ignore", name, patterns @ ..] => {
            let list = if patterns.is_empty() {
                accounts::read_users(&data)?.users.get(*name).map(|u| u.ignore.clone()).ok_or_else(|| format!("no user `{name}`"))?
            } else {
                let list = accounts::set_ignore(&data, name, &patterns.iter().map(|p| p.to_string()).collect::<Vec<_>>())?;
                request_scan(&data, name);
                list
            };
            println!("{name}'s library folders are scanned without: {}", if list.is_empty() { "(nothing)".to_string() } else { list.join(", ") });
            Ok(())
        }
        ["folder", "list", rest @ ..] => {
            for (name, u) in accounts::read_users(&data)?.users {
                if rest.first().is_some_and(|n| *n != name) {
                    continue;
                }
                for f in &u.folders {
                    let there = if Path::new(&f.path).is_dir() { "" } else { "\t(not there)" };
                    println!("{name}\t{}\t{}{there}", f.name, f.path);
                }
            }
            Ok(())
        }
        ["scan", rest @ ..] => {
            let users: Vec<String> = match rest {
                [name] => vec![name.to_string()],
                [] => accounts::read_users(&data)?.users.into_iter().filter(|(_, u)| !u.folders.is_empty()).map(|(n, _)| n).collect(),
                _ => return Err(USAGE.into()),
            };
            for u in users {
                scan(&data, &u)?;
            }
            Ok(())
        }
        ["model", "status"] => {
            println!("{}", lightcraft_server::vision::model_status(&vision_dir(args, &data)));
            Ok(())
        }
        ["model", "download"] => {
            let dir = vision_dir(args, &data);
            let (text, accepted) = (args.iter().any(|a| a == "--text"), args.iter().any(|a| a == "--accept-licences"));
            let mirrors = dir.parent().map(|p| p.join("siglip2-mirrors.txt"));
            let search_missing = !lightcraft_vision::siglip::is_model_dir(&dir);
            let text_missing = text && !lightcraft_vision::ocr::is_model_dir(&dir.join(lightcraft_engine::vision::TEXT_DIR));
            let faces = args.iter().any(|a| a == "--faces");
            let faces_missing = faces && !lightcraft_vision::faces::model::is_model_dir(&dir.join(lightcraft_engine::vision::FACES_DIR));
            if !search_missing && !text_missing && !faces_missing {
                println!("the model{} installed in {}", if text { "s are" } else { " is" }, dir.display());
                return Ok(());
            }
            if !accepted {
                return Err(format!(
                    "{}\n\nThe models are not part of LightCraft. Downloading them means accepting those licences; add --accept-licences to download them.",
                    lightcraft_server::vision::model_status(&dir)
                ));
            }
            if search_missing {
                lightcraft_server::vision::download_model(&dir, mirrors.as_deref())?;
                println!("the search model is installed in {}", dir.display());
            }
            if faces_missing {
                lightcraft_server::vision::download_face_models(&dir, mirrors.as_deref())?;
                println!("the face models are installed in {}", dir.join(lightcraft_engine::vision::FACES_DIR).display());
            }
            if text_missing {
                lightcraft_server::vision::download_text_models(&dir, mirrors.as_deref())?;
                println!("the text-reading models are installed in {}", dir.join(lightcraft_engine::vision::TEXT_DIR).display());
            }
            println!("the running server finds them within a minute");
            Ok(())
        }
        ["health"] => {
            health(&option(args, "--listen").or_else(|| std::env::var("LIGHTCRAFT_LISTEN").ok()).unwrap_or_else(|| "127.0.0.1:8080".into()))
        }
        ["gc"] => {
            let dry = args.iter().any(|a| a == "--dry-run");
            let r = gc::run(&data, dry)?;
            println!("{} {} file(s), {:.1} MB; kept {}", if dry { "would remove" } else { "removed" }, r.removed, r.bytes as f64 / 1e6, r.kept);
            for e in &r.errors {
                eprintln!("{e}");
            }
            Ok(())
        }
        _ => Err(USAGE.into()),
    }
}

/// Where the search model's files are: `--vision-dir`, `$LIGHTCRAFT_VISION_DIR`, else `<data>/models/siglip2`.
fn vision_dir(args: &[String], data: &Path) -> PathBuf {
    option(args, "--vision-dir")
        .or_else(|| std::env::var("LIGHTCRAFT_VISION_DIR").ok())
        .map(PathBuf::from)
        .unwrap_or_else(|| data.join("models").join("siglip2"))
}

/// Ask a running server to scan a user's library folders (it looks for the request every few
/// seconds; a server that isn't running scans at start).
fn request_scan(data: &Path, user: &str) {
    let _ = std::fs::write(accounts::user_dir(data, user).join(folders::REQUEST), b"");
}

/// `scan NAME`: here and now, or by the running server.
fn scan(data: &Path, user: &str) -> Result<(), String> {
    let lib_dir = accounts::user_dir(data, user).join("library");
    std::fs::create_dir_all(&lib_dir).map_err(|e| format!("{}: {e}", lib_dir.display()))?;
    if lightcraft_engine::catalog::LibraryLock::acquire(&lib_dir, "lightcraft-server scan").is_err() {
        request_scan(data, user);
        println!("{user}: the server is running: it scans the library folders within a few seconds (progress on the admin page)");
        return Ok(());
    }
    let lib = std::sync::Mutex::new(lightcraft_server::api::UserLib::open_for(data, user)?);
    let stop = std::sync::atomic::AtomicBool::new(false);
    let mut jobs = Vec::new();
    let tty = std::io::stderr().is_terminal();
    let s = folders::scan(
        data,
        user,
        &lib,
        &stop,
        true,
        &mut |s| {
            if tty && s.todo > 0 {
                eprint!("\r{user}: read {} of {} file(s)", s.done, s.todo);
            }
        },
        &mut |hash, file| jobs.push((hash, file)),
    )?;
    if tty {
        eprintln!();
    }
    println!(
        "{user}: {} photo file(s); {} new, {} moved, {} changed, {} uploaded before, {} not read, {} missing",
        s.files, s.added, s.moved, s.changed, s.linked, s.failed, s.missing
    );
    for e in &s.errors {
        eprintln!("  {e}");
    }
    let blobs = accounts::user_dir(data, user).join("blobs");
    let (mut built, mut failed) = (0, 0);
    let n = jobs.len();
    for (i, (hash, file)) in jobs.into_iter().enumerate() {
        match folders::build_previews(&blobs, &hash, &file) {
            Ok(b) => built += usize::from(b),
            Err(e) => {
                failed += 1;
                eprintln!("{}: {e}", file.display());
            }
        }
        if tty {
            eprint!("\r{user}: previews {} of {n}", i + 1);
        }
    }
    if tty && n > 0 {
        eprintln!();
    }
    println!("{user}: built previews of {built} photo(s){}", if failed > 0 { format!(", {failed} failed") } else { String::new() });
    Ok(())
}

/// `health`: does the server at `listen` answer (the Docker health check)?
fn health(listen: &str) -> Result<(), String> {
    use std::net::ToSocketAddrs;
    let local = listen.replacen("0.0.0.0:", "127.0.0.1:", 1).replacen("[::]:", "[::1]:", 1);
    let addr = local.to_socket_addrs().map_err(|e| format!("{local}: {e}"))?.next().ok_or_else(|| format!("{local}: no address"))?;
    let mut c = std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(3)).map_err(|e| format!("{addr}: {e}"))?;
    c.set_read_timeout(Some(Duration::from_secs(5))).map_err(|e| e.to_string())?;
    c.write_all(b"GET /api/health HTTP/1.0\r\nHost: localhost\r\n\r\n").map_err(|e| e.to_string())?;
    let mut answer = String::new();
    c.take(4096).read_to_string(&mut answer).map_err(|e| e.to_string())?;
    let status = answer.split_whitespace().nth(1).unwrap_or("");
    if status == "200" { Ok(()) } else { Err(format!("{addr}: unhealthy ({})", answer.lines().next().unwrap_or("no answer"))) }
}

fn main() -> ExitCode {
    let level = match std::env::var("LIGHTCRAFT_LOG").as_deref() {
        Ok("error") => log::LevelFilter::Error,
        Ok("warn") => log::LevelFilter::Warn,
        Ok("debug") => log::LevelFilter::Debug,
        _ => log::LevelFilter::Info,
    };
    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(level);
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
