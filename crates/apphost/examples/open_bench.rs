//! Opening a library through the engine host and asking for the first screen, the way a native frontend
//! does, timed: the open itself, then each request in the order such a frontend sends them.
//!
//! `cargo run --release -p lightcraft-apphost --example open_bench -- <library folder> [desktop|mobile]`
//!
//! Run it on a copy of a big library (`cp -c -R`, which costs no disk on APFS): opening writes to it
//! (the lock, view.json, a snapshot when the format changed), and remove its `sync.*` files first, or
//! the engine may talk to the library's sync server. `OPEN_BENCH_SESSION=1` first times the session
//! alone, and the library view built from nothing three times. `LIGHTCRAFT_PROFILE=1` adds the engine's
//! own timings.

use std::time::{Duration, Instant};

use lightcraft_apphost::host::{Host, OpenConfig};
use lightcraft_apphost::startup::Profile;
use serde_json::{Value, json};

// the apps' allocator
#[global_allocator]
static ALLOC: lightcraft_sysmem::ReturningAlloc = lightcraft_sysmem::ReturningAlloc;

fn req(id: u64, method: &str, params: Value) -> Vec<u8> {
    serde_json::to_vec(&json!({"id": id, "method": method, "params": params})).unwrap_or_default()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(library) = args.first() else {
        eprintln!("usage: open_bench <library folder> [desktop|mobile]");
        std::process::exit(2);
    };
    let profile = if args.get(1).map(String::as_str) == Some("mobile") { Profile::Mobile } else { Profile::Desktop };
    let tmp = std::env::temp_dir().join(format!("lc-open-bench-{}", std::process::id()));
    let config = OpenConfig { library: Some(library.clone()), tmp: Some(tmp.to_string_lossy().into()), profile, ..Default::default() };

    if std::env::var_os("OPEN_BENCH_SESSION").is_some() {
        let t = Instant::now();
        let s = lightcraft_apphost::startup::open_session_or_err(Some(std::path::Path::new(library)));
        println!("session only       {:7.0} ms  ({})", t.elapsed().as_secs_f64() * 1e3, s.as_ref().map_or(0, |s| s.catalog.len()));
        if let Ok(mut s) = s {
            for _ in 0..3 {
                s.refresh_view();
                let t = Instant::now();
                let n = s.visible().len();
                println!("  visible()        {:7.0} ms  ({n})", t.elapsed().as_secs_f64() * 1e3);
            }
        }
    }
    let t0 = Instant::now();
    let (host, opened) = match Host::open(config) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("can't open {library}: {e}");
            std::process::exit(1);
        }
    };
    println!("open               {:7.0} ms  ({} photos)", t0.elapsed().as_secs_f64() * 1e3, opened["photos"]);

    let mut id = 0;
    let mut ask = |name: &str, method: &str, params: Value| {
        id += 1;
        let t = Instant::now();
        let reply = host.call(&req(id, method, params), Duration::from_secs(120));
        let ms = t.elapsed().as_secs_f64() * 1e3;
        let bytes = reply["result"].to_string().len();
        let failed = if reply.get("error").is_some() { "  FAILED" } else { "" };
        println!("{name:<18} {ms:7.1} ms  ({bytes} B){failed}");
    };
    let execute = |c: &str, p: Value| json!({"command": c, "params": p});
    // the first screen, as a native frontend asks for it
    ask("view.stacks", "engine.execute", execute("view.stacks", json!({"on": true})));
    ask("host.state", "host.state", json!({}));
    ask("library.state", "engine.execute", execute("library.state", json!({})));
    ask("library.groups", "engine.execute", execute("library.groups", json!({"by": "day"})));
    ask("catalog.query", "engine.execute", execute("catalog.query", json!({"offset": 0, "limit": 200})));
    // what waits for the first pictures
    ask("catalog.stats", "engine.execute", execute("catalog.stats", json!({"counts": true})));
    ask("label.names", "engine.execute", execute("label.names", json!({})));
    ask("albums.list", "engine.execute", execute("albums.list", json!({})));
    ask("host.sidebar", "host.sidebar", json!({}));
    ask("library.filterChips", "engine.execute", execute("library.filterChips", json!({})));
    ask("sync.status", "engine.execute", execute("sync.status", json!({})));
    ask("catalog.query p2", "engine.execute", execute("catalog.query", json!({"offset": 200, "limit": 200})));
    println!("total              {:7.0} ms", t0.elapsed().as_secs_f64() * 1e3);
    // (not closed: closing saves a snapshot, which the app does when it quits, not when it opens)
    std::process::exit(0);
}
