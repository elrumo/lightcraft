//! `lightcraft-server`: the self-hosted LightCraft sync server (see `docs/sync.md`).
//!
//! ```text
//! lightcraft-server serve [--data DIR] [--listen HOST:PORT] [--web DIR]
//! lightcraft-server user add NAME        (password: first line of stdin, or $LIGHTCRAFT_PASSWORD)
//! lightcraft-server user passwd NAME
//! lightcraft-server user remove NAME
//! lightcraft-server user list
//! lightcraft-server device list NAME
//! lightcraft-server device revoke NAME ID
//! lightcraft-server gc [--dry-run]
//! ```
//!
//! `--data` defaults to `$LIGHTCRAFT_DATA` or `./lightcraft-data`, `--listen` to
//! `$LIGHTCRAFT_LISTEN` or `127.0.0.1:8080` (only this computer: put a TLS proxy in front, or
//! listen on a private network such as Tailscale).
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::io::BufRead;
use std::path::PathBuf;
use std::process::ExitCode;

use lightcraft_server::{Config, Server, accounts, gc};

const USAGE: &str = "usage:
  lightcraft-server serve [--data DIR] [--listen HOST:PORT] [--web DIR]
  lightcraft-server user add|passwd|remove NAME   (password: stdin or $LIGHTCRAFT_PASSWORD)
  lightcraft-server user list
  lightcraft-server device list NAME
  lightcraft-server device revoke NAME ID
  lightcraft-server gc [--dry-run]
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
        } else if matches!(a.as_str(), "--data" | "--listen" | "--web") {
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
    eprintln!("password (one line on stdin):");
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line).map_err(|e| e.to_string())?;
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
                log::warn!("no users yet: add one with `lightcraft-server user add NAME --data {}`", data.display());
            }
            let s = Server::start(Config { data: data.clone(), listen, web: web.clone(), max_requests: 64 })?;
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
            println!("added {name}");
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
            for name in accounts::read_users(&data)?.users.keys() {
                println!("{name}");
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
