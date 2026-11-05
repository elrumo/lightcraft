//! Paging through raws in the loupe as a phone does, driven through the engine host: each page turn asks for the
//! landed photo (`render.view`) and its two neighbours (`render.prefetch`), `every` ms apart, with no wait for the
//! render. Prints the process's lifetime peak footprint (what iOS ends an app by) and the engine's working memory
//! (`host.memory` → work gate) sampled during the run.
//!
//! `cargo run --release -p lightcraft-apphost --example page_bench -- <raw>… [--every ms] [--turns n] [--size WxH]`
//! e.g. `… corpus/raw/cr3-*.cr3 --every 300 --turns 30 --size 1198x1797`. A file given once is imported under
//! several names (`--copies n`, 4 by default), so each page decodes a new photo. `BENCH_MEMORY_MB` sets the memory
//! the engine is told it has (an iPhone's limit).

use std::time::{Duration, Instant};

use lightcraft_apphost::host::{Host, OpenConfig};
use serde_json::{Value, json};

#[global_allocator]
static ALLOC: lightcraft_sysmem::ReturningAlloc = lightcraft_sysmem::ReturningAlloc;

fn req(id: u64, method: &str, params: Value) -> Vec<u8> {
    serde_json::to_vec(&json!({"id": id, "method": method, "params": params})).unwrap_or_default()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let every: u64 = flag("--every").and_then(|v| v.parse().ok()).unwrap_or(300);
    let turns: usize = flag("--turns").and_then(|v| v.parse().ok()).unwrap_or(30);
    let no_prefetch = args.iter().any(|a| a == "--no-prefetch");
    let copies: usize = flag("--copies").and_then(|v| v.parse().ok()).unwrap_or(4);
    let (w, h) = flag("--size")
        .and_then(|s| s.split_once('x').and_then(|(a, b)| Some((a.parse::<u64>().ok()?, b.parse::<u64>().ok()?))))
        .unwrap_or((1198, 1797));
    let mut files = Vec::new();
    let mut skip = false;
    for a in &args {
        if skip {
            skip = false;
            continue;
        }
        if a == "--no-prefetch" {
            continue;
        }
        if a.starts_with("--") {
            skip = true;
            continue;
        }
        files.push(a.clone());
    }
    if files.is_empty() {
        eprintln!("usage: page_bench <raw>… [--every ms] [--turns n] [--size WxH] [--copies n]");
        std::process::exit(2);
    }
    let root = std::env::temp_dir().join(format!("lc-page-bench-{}", std::process::id()));
    let src = root.join("src");
    let _ = std::fs::create_dir_all(&src);
    // (copies under other names: a page then decodes a photo it has not seen)
    let mut paths = Vec::new();
    for c in 0..copies {
        for (i, f) in files.iter().enumerate() {
            let ext = std::path::Path::new(f).extension().and_then(|e| e.to_str()).unwrap_or("raw");
            let to = src.join(format!("p{c}-{i}.{ext}"));
            // (a few bytes past the end make each copy another file: the import keeps one of identical files)
            if let Ok(mut bytes) = std::fs::read(f) {
                bytes.extend_from_slice(format!("page-bench-{c}").as_bytes());
                if std::fs::write(&to, bytes).is_ok() {
                    paths.push(to.to_string_lossy().to_string());
                }
            }
        }
    }
    let config = OpenConfig {
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
        host.call(&req(id, method, params), Duration::from_secs(600))
    };
    let r = call("engine.execute", json!({"command": "library.import", "params": {"paths": paths}}));
    if r["error"].is_object() {
        eprintln!("import: {r}");
        std::process::exit(1);
    }
    let q = call("engine.execute", json!({"command": "catalog.query", "params": {"limit": 1000}}));
    let ids: Vec<u64> = q["result"]["photos"].as_array().map(|a| a.iter().filter_map(|p| p["id"].as_u64()).collect()).unwrap_or_default();
    if ids.len() < 3 {
        eprintln!("too few photos: {q}");
        std::process::exit(1);
    }
    // (the import's own work settles first: its thumbnails are not what is measured)
    std::thread::sleep(Duration::from_secs(3));
    for _ in host.poll(Duration::from_millis(1)) {}
    let (floor, _) = footprint();
    println!("{} photos, view {w}x{h}, a page every {every} ms, {turns} turns; footprint before {floor:.0} MB", ids.len());

    let mut n = 1_000_000u64;
    let mut work_peak = 0u64;
    let mut sample = |host: &Host, n: &mut u64| {
        *n += 1;
        let m = host.call(&req(*n, "host.memory", json!({})), Duration::from_secs(30));
        let used = m["result"]["engine"]["workBytes"].as_u64().or_else(|| m["result"]["engine"]["work_bytes"].as_u64()).unwrap_or(0);
        work_peak = work_peak.max(used);
    };
    // `BENCH_TRACE=1`: the engine's own accounting sampled between polls; the sample with the most in total is printed
    let trace = std::env::var("BENCH_TRACE").is_ok_and(|v| v == "1");
    let mut maxima = [0u64; 6];
    let t0 = Instant::now();
    for t in 0..turns {
        let i = t % ids.len();
        let photo = ids[i];
        n += 1;
        let _ = host.submit(&req(n, "render.view", json!({"photo": photo, "width": w, "height": h})));
        let next = ids[(i + 1) % ids.len()];
        let prev = ids[(i + ids.len() - 1) % ids.len()];
        n += 1;
        let _ = (!no_prefetch).then(|| host.submit(&req(n, "render.prefetch", json!({"photos": [next, prev], "width": w, "height": h}))));
        let until = Instant::now() + Duration::from_millis(every);
        while Instant::now() < until {
            for _ in host.poll(Duration::from_millis(5)) {}
            if trace {
                n += 1;
                let m = host.call(&req(n, "host.memory", json!({})), Duration::from_secs(30))["result"].clone();
                let e = &m["engine"];
                let v = &m["views"]["stageCaches"];
                let get = |v: &Value| v.as_u64().unwrap_or(0);
                let now = [
                    get(&e["workBytes"]),
                    get(&e["gpu"]["allocated"]),
                    get(&e["previewSources"]["bytes"]) + get(&e["fullSource"]["bytes"]) + get(&e["rendered"]["bytes"]),
                    get(&v["cpuBytes"]),
                    get(&v["gpuBytes"]),
                ];
                let sum: u64 = now.iter().sum();
                if sum > maxima[5] {
                    maxima = [now[0], now[1], now[2], now[3], now[4], sum];
                }
            }
        }
        sample(&host, &mut n);
    }
    // the last page's picture
    let settle = Instant::now();
    while settle.elapsed() < Duration::from_secs(20) {
        let mut done = false;
        for e in host.poll(Duration::from_millis(5)) {
            done |= e["kind"] == "image" && e["slot"] == "main" && e["final"] == true;
        }
        if done {
            break;
        }
    }
    let (now, peak) = footprint();
    let m = host.call(&req(n + 1, "host.memory", json!({})), Duration::from_secs(30))["result"].clone();
    println!(
        "paged in {:.1} s: footprint now {now:.0} MB, lifetime peak {peak:.0} MB (+{:.0} over the floor); work gate peak sampled {:.0} MB",
        t0.elapsed().as_secs_f64(),
        peak - floor,
        work_peak as f64 / 1048576.0
    );
    println!("host.memory: {}", m["engine"]);
    if trace {
        let mb = |b: u64| b / 1048576;
        println!(
            "largest sample: work {} MB, gpu allocated {} MB, caches {} MB, view stages cpu {} MB + gpu {} MB = {} MB",
            mb(maxima[0]),
            mb(maxima[1]),
            mb(maxima[2]),
            mb(maxima[3]),
            mb(maxima[4]),
            mb(maxima[5])
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// The process's footprint (what iOS counts against the app's limit) and its lifetime peak, in MB.
fn footprint() -> (f64, f64) {
    let out = std::process::Command::new("/usr/bin/footprint").args(["--noCategories", "-p"]).arg(std::process::id().to_string()).output();
    let text = out.map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default();
    let field = |name: &str| {
        let Some(rest) = text.split(name).nth(1) else { return f64::NAN };
        let mut it = rest.split_whitespace();
        let n: f64 = it.next().and_then(|n| n.parse().ok()).unwrap_or(f64::NAN);
        match it.next() {
            Some("KB") => n / 1024.0,
            Some("GB") => n * 1024.0,
            _ => n,
        }
    };
    (field("phys_footprint: "), field("phys_footprint_peak: "))
}
