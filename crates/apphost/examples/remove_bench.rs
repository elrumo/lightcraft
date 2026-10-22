//! Remove / Heal strokes on a real photo, driven through the engine host the way a native frontend drives
//! them (`spot.add`, then the loupe's `render.view`), with the process's footprint after each batch: strokes
//! that leave memory behind show as a footprint that keeps climbing.
//!
//! `cargo run --release -p lightcraft-apphost --example remove_bench -- <photo> [strokes] [mode…]`
//! where a mode is `heal`, `clone`, `remove`, `fast` (AI Remove, MI-GAN) or `high` (AI Remove, LaMa; models
//! from the models folder). `BENCH_MEMORY_MB` tells the engine how much an iPhone would let it use,
//! `BENCH_BG=1` paints AI fills on the worker (`wait: false`) as the apps do.

use std::time::{Duration, Instant};

use lightcraft_apphost::host::{Host, OpenConfig};
use serde_json::{Value, json};

#[global_allocator]
static ALLOC: lightcraft_sysmem::ReturningAlloc = lightcraft_sysmem::ReturningAlloc;

fn req(id: u64, method: &str, params: Value) -> Vec<u8> {
    serde_json::to_vec(&json!({"id": id, "method": method, "params": params})).unwrap_or_default()
}

/// The process's footprint in MB (what iOS counts against the app).
fn footprint_mb() -> f64 {
    let out = std::process::Command::new("/usr/bin/footprint").args(["--noCategories", "-p"]).arg(std::process::id().to_string()).output();
    let text = out.map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default();
    text.lines()
        .find_map(|l| l.split_once("phys_footprint: ").map(|(_, v)| v.trim().to_string()))
        .map(|v| {
            let mut it = v.split_whitespace();
            let n: f64 = it.next().and_then(|n| n.parse().ok()).unwrap_or(0.0);
            match it.next() {
                Some("GB") => n * 1024.0,
                Some("KB") => n / 1024.0,
                Some("B") => n / 1048576.0,
                _ => n,
            }
        })
        .unwrap_or(0.0)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(photo) = args.first() else {
        eprintln!("usage: remove_bench <photo> [strokes] [heal|clone|remove|fast|high…]");
        std::process::exit(2);
    };
    let strokes: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(12);
    let mut modes: Vec<String> = args.iter().skip(2).cloned().collect();
    if modes.is_empty() {
        modes = ["heal", "remove", "fast"].map(String::from).to_vec();
    }
    let background = std::env::var("BENCH_BG").is_ok_and(|v| v == "1");
    let root = std::env::temp_dir().join(format!("lc-remove-bench-{}", std::process::id()));
    // a phone keeps its models in the home folder's `Library/Caches/LightCraft/models`: the Mac app's folder stands in
    let home = root.join("home");
    if let Some(models) = std::env::var_os("HOME").map(|h| std::path::Path::new(&h).join("Library/Application Support/LightCraft/models")) {
        let _ = std::fs::create_dir_all(home.join("Library/Caches/LightCraft"));
        let _ = std::os::unix::fs::symlink(models, home.join("Library/Caches/LightCraft/models"));
    }
    let config = OpenConfig {
        home: Some(home.to_string_lossy().into()),
        library: Some(root.join("Lib").to_string_lossy().into()),
        tmp: Some(root.join("tmp").to_string_lossy().into()),
        available_memory: std::env::var("BENCH_MEMORY_MB").ok().and_then(|m| m.parse::<u64>().ok()).map(|m| m << 20),
        empty: true,
        ..Default::default()
    };
    let (host, _) = match Host::open(config) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("open: {e}");
            std::process::exit(1);
        }
    };
    let mut id = 0u64;
    let mut call = |method: &str, params: Value| {
        id += 1;
        host.call(&req(id, method, params), Duration::from_secs(300))
    };
    let r = call("engine.execute", json!({"command": "library.import", "params": {"paths": [photo]}}));
    if r["error"].is_object() {
        eprintln!("import: {r}");
        std::process::exit(1);
    }
    let q = call("engine.execute", json!({"command": "catalog.query", "params": {"limit": 1}}));
    let Some(pid) = q["result"]["photos"][0]["id"].as_u64() else {
        eprintln!("no photo: {q}");
        std::process::exit(1);
    };
    call("engine.execute", json!({"command": "library.select", "params": {"ids": [pid]}}));
    let (w, h) = (1206u64, 2400u64);
    let wait_final = |host: &Host| {
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(120) {
            for e in host.poll(Duration::from_millis(5)) {
                if e["kind"] == "image" && e["slot"] == "main" && e["final"] == true {
                    return;
                }
            }
        }
    };
    host.submit(&req(900_000, "render.view", json!({"photo": pid, "width": w, "height": h}))).ok();
    wait_final(&host);
    println!("start: footprint {:.0} MB", footprint_mb());
    let mut n = 1_000_000u64;
    for mode in &modes {
        let (spot_mode, ai, quality) = match mode.as_str() {
            "fast" => ("remove", true, "fast"),
            "high" => ("remove", true, "high"),
            m => (m, false, "fast"),
        };
        let t0 = Instant::now();
        for i in 0..strokes {
            let f = i as f64 / strokes.max(1) as f64;
            let (x, y) = (0.2 + 0.6 * f, 0.3 + 0.4 * ((f * 7.0).sin() * 0.5 + 0.5));
            let pts: Vec<Value> = (0..6).map(|k| json!([x + k as f64 * 0.006, y + k as f64 * 0.004])).collect();
            let params = json!({"mode": spot_mode, "points": pts, "size": 0.03, "ai": ai, "quality": quality, "wait": !(ai && background)});
            let r = call("engine.execute", json!({"command": "spot.add", "params": params}));
            if r["error"].is_object() {
                eprintln!("{mode}: {r}");
                break;
            }
            // (a background fill lands when the worker is done: the app's tick stores it)
            n += 1;
            host.submit(&req(n, "render.view", json!({"photo": pid, "width": w, "height": h, "draft": true}))).ok();
            n += 1;
            host.submit(&req(n, "render.view", json!({"photo": pid, "width": w, "height": h}))).ok();
            wait_final(&host);
            if (i + 1) % 4 == 0 {
                println!("  {mode} {:>3}: footprint {:.0} MB", i + 1, footprint_mb());
            }
        }
        while ai && background {
            let busy = call("engine.execute", json!({"command": "catalog.query", "params": {"limit": 1}}));
            let _ = busy;
            std::thread::sleep(Duration::from_millis(200));
            let rep = call("host.memory", json!({}));
            let _ = rep;
            if t0.elapsed() > Duration::from_secs(5) {
                break;
            }
        }
        println!("{mode}: {strokes} strokes in {:.1} s, footprint {:.0} MB", t0.elapsed().as_secs_f64(), footprint_mb());
        call("engine.execute", json!({"command": "develop.reset", "params": {}}));
        n += 1;
        host.submit(&req(n, "render.view", json!({"photo": pid, "width": w, "height": h}))).ok();
        wait_final(&host);
        std::thread::sleep(Duration::from_millis(500));
        println!("{mode}: after reset footprint {:.0} MB", footprint_mb());
        // `BENCH_IDLE=<s>`: let the engine sit idle that long (the networks are unloaded after a while), then measure
        if let Some(secs) = std::env::var("BENCH_IDLE").ok().and_then(|s| s.parse::<u64>().ok()).filter(|_| ai) {
            let t = Instant::now();
            while t.elapsed() < Duration::from_secs(secs) {
                host.poll(Duration::from_millis(200));
            }
            println!("{mode}: after {secs} s idle footprint {:.0} MB", footprint_mb());
        }
    }
    let mem = call("host.memory", json!({}))["result"].clone();
    println!("{mem}");
    host.close(Duration::from_secs(30));
    let _ = std::fs::remove_dir_all(&root);
}
