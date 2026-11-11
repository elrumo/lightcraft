//! Opening a library through the engine host and quitting the way the Mac app does (`host.lifecycle`
//! `terminate`, then closing the engine), timed: what quitting costs, and whether it rewrote `catalog.snap`.
//!
//! `cargo run --release -p lightcraft-apphost --example close_bench -- <library folder> [--edit]`
//!
//! `--edit` rates the first photo before quitting (a library with a change). Run it on a copy of a big library
//! (`cp -c`, see `open_bench`), never on one in use. With `CLOSE_BENCH_STEP=1` it waits for a line on stdin after
//! opening and again after closing, so a wrapper can read the process's disk writes around the close
//! (`proc_pid_rusage`).

use std::io::BufRead;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

use lightcraft_apphost::host::{Host, OpenConfig};
use serde_json::{Value, json};

// the apps' allocator
#[global_allocator]
static ALLOC: lightcraft_sysmem::ReturningAlloc = lightcraft_sysmem::ReturningAlloc;

fn req(id: u64, method: &str, params: Value) -> Vec<u8> {
    serde_json::to_vec(&json!({"id": id, "method": method, "params": params})).unwrap_or_default()
}

/// `catalog.snap`'s change time and size.
fn snap(library: &str) -> Option<(SystemTime, u64)> {
    let m = std::fs::metadata(Path::new(library).join("catalog.snap")).ok()?;
    Some((m.modified().ok()?, m.len()))
}

fn step(on: bool, what: &str) {
    if on {
        println!("{what}");
        let _ = std::io::stdin().lock().lines().next();
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(library) = args.first().cloned() else {
        eprintln!("usage: close_bench <library folder> [--edit]");
        std::process::exit(2);
    };
    let edit = args.iter().any(|a| a == "--edit");
    let stepped = std::env::var_os("CLOSE_BENCH_STEP").is_some();
    let tmp = std::env::temp_dir().join(format!("lc-close-bench-{}", std::process::id()));
    let config = OpenConfig { library: Some(library.clone()), tmp: Some(tmp.to_string_lossy().into()), ..Default::default() };
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
    let mut call = |method: &str, params: Value| {
        id += 1;
        let t = Instant::now();
        let reply = host.call(&req(id, method, params), Duration::from_secs(600));
        if let Some(e) = reply.get("error") {
            eprintln!("{method}: {e}");
        }
        (t.elapsed().as_secs_f64() * 1e3, reply)
    };
    let execute = |c: &str, p: Value| json!({"command": c, "params": p});
    // the first screen's list, so the open is done with
    call("engine.execute", execute("catalog.query", json!({"offset": 0, "limit": 1})));
    if edit {
        let (_, r) = call("engine.execute", execute("catalog.query", json!({"offset": 0, "limit": 1})));
        let first = r["result"]["ids"].get(0).or_else(|| r["result"]["photos"].get(0).and_then(|p| p.get("id"))).cloned();
        let (ms, _) = call("engine.execute", execute("photo.rate", json!({"ids": [first], "rating": 3})));
        println!("edit               {ms:7.1} ms  (photo {})", first.unwrap_or(Value::Null));
    }
    let before = snap(&library);
    step(stepped, "opened");
    let (ms, _) = call("host.lifecycle", json!({"event": "terminate"}));
    let after_terminate = snap(&library);
    println!("terminate          {ms:7.1} ms  (catalog.snap {})", if after_terminate != before { "rewritten" } else { "untouched" });
    let t = Instant::now();
    host.close(Duration::from_secs(600));
    let after = snap(&library);
    let ms = t.elapsed().as_secs_f64() * 1e3;
    println!("close              {ms:7.1} ms  (catalog.snap {})", if after != after_terminate { "rewritten" } else { "untouched" });
    println!("catalog.snap       {} MB", after.map_or(0, |a| a.1 >> 20));
    step(stepped, "closed");
    let _ = std::fs::remove_dir_all(&tmp);
}
