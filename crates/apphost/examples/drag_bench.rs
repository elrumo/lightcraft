//! A slider drag on a real photo, driven through the engine host the way a native frontend drives it
//! (`develop.set`, wait for its answer, `render.view {draft}`, again), timed.
//!
//! `cargo run --release -p lightcraft-apphost --example drag_bench -- <photo> [WxH] [seconds] [controls…]`
//! e.g. `… corpus/raw/cr3-canon-r6-raw.cr3 2400x1600 3 light.exposure wb.temp`. Prints, per control:
//! answers to `develop.set` (ms), drafts a second, their render time and size, and the time from the
//! release to the sharp picture. `LIGHTCRAFT_PROFILE=1` adds the pipeline's stage timings.
//!
//! With `zoom` for a control: the photo zoomed in as a phone does (2×, 4×, 8×, then panned), with the
//! process's footprint and the engine's memory (`host.memory`) after each step. `BENCH_MEMORY_MB` sets
//! the memory the engine is told it has (an iPhone's limit), `BENCH_PROFILE=desktop` a Mac's rules.

use std::time::{Duration, Instant};

use lightcraft_apphost::host::{Host, OpenConfig};
use serde_json::{Value, json};

// the apps' allocator (freed large blocks leave the footprint at once: `lightcraft_sysmem::ReturningAlloc`)
#[global_allocator]
static ALLOC: lightcraft_sysmem::ReturningAlloc = lightcraft_sysmem::ReturningAlloc;

fn req(id: u64, method: &str, params: Value) -> Vec<u8> {
    serde_json::to_vec(&json!({"id": id, "method": method, "params": params})).unwrap_or_default()
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f64::total_cmp);
    v[((v.len() - 1) as f64 * p).round() as usize]
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // `BENCH_NO_REUSE=1`: every freed large block unmapped (no kept blocks for the next picture)
    if std::env::var("BENCH_NO_REUSE").is_ok_and(|v| v == "1") {
        lightcraft_sysmem::set_block_reuse(false);
    }
    let Some(photo) = args.first() else {
        eprintln!("usage: drag_bench <photo> [WxH] [seconds] [controls…]");
        std::process::exit(2);
    };
    let (w, h) =
        args.get(1).and_then(|s| s.split_once('x')).and_then(|(a, b)| Some((a.parse::<u64>().ok()?, b.parse::<u64>().ok()?))).unwrap_or((2400, 1600));
    let secs: f64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(3.0);
    let mut controls: Vec<String> = args.iter().skip(3).cloned().collect();
    if controls.is_empty() {
        controls = ["light.exposure", "light.shadows", "wb.temp", "effects.clarity", "detail.nrLuminance"].map(String::from).to_vec();
    }
    let root = std::env::temp_dir().join(format!("lc-drag-bench-{}", std::process::id()));
    let config = OpenConfig {
        library: Some(root.join("Lib").to_string_lossy().into()),
        tmp: Some(root.join("tmp").to_string_lossy().into()),
        profile: if std::env::var("BENCH_PROFILE").is_ok_and(|p| p == "desktop") {
            lightcraft_apphost::startup::Profile::Desktop
        } else {
            Default::default()
        },
        // what an iPhone lets the app use (`os_proc_available_memory`), when asked to act as one
        available_memory: std::env::var("BENCH_MEMORY_MB").ok().and_then(|m| m.parse::<u64>().ok()).map(|m| m << 20),
        // (only the photo imported below: a new library otherwise starts with the demo photos)
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
        host.call(&req(id, method, params), Duration::from_secs(120))
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
    // `BENCH_HDR=1`: an HDR edit on a display with headroom (the loupe renders its float HDR picture)
    let hdr = std::env::var("BENCH_HDR").is_ok_and(|v| v == "1");
    if hdr {
        call("engine.execute", json!({"command": "develop.hdr", "params": {"enabled": true}}));
        call("render.hdrDisplay", json!({"on": true}));
    }
    let name = q["result"]["photos"][0]["name"].as_str().unwrap_or("?").to_string();
    println!("{name}: gpu {:?}, view {w}x{h}{}", lightcraft_engine::gpu::adapter_name(), if hdr { ", HDR" } else { "" });

    // events: wait for the loupe's sharp picture
    let wait_final = |host: &Host, t0: Instant| -> f64 {
        loop {
            for e in host.poll(Duration::from_millis(5)) {
                if e["kind"] == "image" && e["slot"] == "main" && e["final"] == true {
                    return t0.elapsed().as_secs_f64() * 1e3;
                }
            }
            if t0.elapsed() > Duration::from_secs(120) {
                return f64::NAN;
            }
        }
    };
    let t0 = Instant::now();
    host.submit(&req(1_000_000, "render.view", json!({"photo": pid, "width": w, "height": h}))).ok();
    // `BENCH_DRAG_AT_ONCE=1`: the first drag starts while the photo is still opening (a slider grabbed at once)
    if std::env::var("BENCH_DRAG_AT_ONCE").is_ok_and(|v| v == "1") {
        println!("dragging at once");
    } else {
        println!("open: sharp picture after {:.0} ms", wait_final(&host, t0));
    }
    // the second time is warm (decoded source cached)
    if !std::env::var("BENCH_DRAG_AT_ONCE").is_ok_and(|v| v == "1") {
        let t0 = Instant::now();
        host.submit(&req(1_000_001, "render.view", json!({"photo": pid, "width": w, "height": h - 2}))).ok();
        println!("same photo, other size: {:.0} ms", wait_final(&host, t0));
    }

    if controls.iter().any(|c| c == "zoom") {
        zoom(&host, pid, w, h);
        host.close(Duration::from_secs(30));
        let _ = std::fs::remove_dir_all(&root);
        return;
    }
    for control in &controls {
        let spec = call("engine.execute", json!({"command": "develop.controls", "params": {}}));
        let s = spec["result"].as_array().and_then(|a| a.iter().find(|c| c["id"] == control.as_str()).cloned()).unwrap_or_default();
        if s.is_null() {
            let ids: Vec<&str> = spec["result"].as_array().into_iter().flatten().filter_map(|c| c["id"].as_str()).collect();
            eprintln!("{control}: not a control of this photo (it has {})", ids.join(", "));
            continue;
        }
        let (lo, hi, def) = (s["min"].as_f64().unwrap_or(-1.0), s["max"].as_f64().unwrap_or(1.0), s["default"].as_f64().unwrap_or(0.0));
        call("engine.execute", json!({"command": "develop.beginInteraction", "params": {"label": control}}));
        let (mut sets, mut drafts, mut draft_ms, mut sizes) = (Vec::new(), 0usize, Vec::new(), (0u64, 0u64));
        let mut gaps = Vec::new();
        let (mut asked_at, mut latency) = (std::collections::HashMap::<String, Instant>::new(), Vec::new());
        let mut last_draft: Option<Instant> = None;
        let start = Instant::now();
        while start.elapsed().as_secs_f64() < secs {
            // a finger moving: a new value every 8 ms (120 Hz), sent once the last one is answered
            let t = start.elapsed().as_secs_f64();
            let v = def + (hi - lo) * 0.3 * (t * 2.0).sin();
            let ts = Instant::now();
            let r = call("engine.execute", json!({"command": "develop.set", "params": {"control": control, "value": v}}));
            sets.push(ts.elapsed().as_secs_f64() * 1e3);
            if r["error"].is_object() {
                eprintln!("{control}: {r}");
                break;
            }
            // the reply names the render the picture will be: its age when it lands is the drag's latency
            let asked = call("render.view", json!({"photo": pid, "width": w, "height": h, "draft": true}));
            if let Some(key) = asked["result"]["key"].as_str() {
                asked_at.entry(key.to_string()).or_insert(ts);
            }
            // the pictures landing until the next step is due, timed as they land
            let next = start + Duration::from_secs_f64((ts - start).as_secs_f64() + 0.008);
            loop {
                let wait = next.checked_duration_since(Instant::now()).unwrap_or_default();
                for e in host.poll(wait) {
                    if e["kind"] == "image" && e["slot"] == "main" && e["draft"] == true {
                        drafts += 1;
                        draft_ms.push(e["ms"].as_f64().unwrap_or(0.0));
                        sizes = (e["width"].as_u64().unwrap_or(0), e["height"].as_u64().unwrap_or(0));
                        let now = Instant::now();
                        if let Some(at) = e["key"].as_u64().and_then(|k| asked_at.get(&k.to_string())) {
                            latency.push(now.duration_since(*at).as_secs_f64() * 1e3);
                        }
                        if let Some(l) = last_draft {
                            gaps.push(now.duration_since(l).as_secs_f64() * 1e3);
                        }
                        last_draft = Some(now);
                    }
                }
                if Instant::now() >= next {
                    break;
                }
            }
        }
        let elapsed = start.elapsed().as_secs_f64();
        let t0 = Instant::now();
        call("engine.execute", json!({"command": "develop.endInteraction"}));
        host.submit(&req(3_000_000, "render.view", json!({"photo": pid, "width": w, "height": h}))).ok();
        let settle = wait_final(&host, t0);
        println!(
            "{control:<20} [{lo}, {hi}] set {:>5.1} ms (p95 {:>5.1}, max {:>6.1}) | {:>5.1} drafts/s, render {:>5.1} ms (p95 {:>5.1}), step→picture {:>5.1} ms (p95 {:>5.1}), gap p95 {:>5.1} ms, {}x{} | release→sharp {:>5.0} ms",
            pct(&mut sets.clone(), 0.5),
            pct(&mut sets.clone(), 0.95),
            pct(&mut sets, 1.0),
            drafts as f64 / elapsed,
            pct(&mut draft_ms.clone(), 0.5),
            pct(&mut draft_ms, 0.95),
            pct(&mut latency.clone(), 0.5),
            pct(&mut latency, 0.95),
            pct(&mut gaps, 0.95),
            sizes.0,
            sizes.1,
            settle
        );
        call("engine.execute", json!({"command": "develop.set", "params": {"control": control, "value": def}}));
    }
    host.close(Duration::from_secs(30));
    let _ = std::fs::remove_dir_all(&root);
}

/// The process's memory as iOS counts it against the app's limit (`footprint`: dirty memory, the GPU's
/// buffers included on Apple silicon), in MB.
fn footprint_mb() -> f64 {
    footprint().0
}

/// [`footprint_mb`] and the highest it has been since the process started (what the system ends an app
/// by: a decode's temporaries count there, gone by the time a step ends), in MB.
fn footprint() -> (f64, f64) {
    let out = std::process::Command::new("/usr/bin/footprint").args(["--noCategories", "-p"]).arg(std::process::id().to_string()).output();
    let text = out.map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default();
    // "phys_footprint: 2480 KB", "phys_footprint_peak: 1.2 GB"
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

/// Zoom in as a phone does (`render.view {zoom}` after each pinch), then pan at the deepest zoom; the
/// footprint after each sharp picture.
fn zoom(host: &Host, pid: u64, w: u64, h: u64) {
    let wait = |what: &str| {
        let t0 = Instant::now();
        let mut got = (false, false);
        while t0.elapsed() < Duration::from_secs(20) && !(got.0 && got.1) {
            for e in host.poll(Duration::from_millis(5)) {
                if e["kind"] == "image" && e["slot"] == "main" && e["final"] == true {
                    got.0 = true;
                }
                if e["kind"] == "image" && e["slot"] == "region" {
                    got = (true, true);
                    print!("  {}x{} window in {:?}; ", e["width"], e["height"], e["window"]["full"]);
                }
                if e["kind"] == "renderFailed" {
                    println!("  {what}: render failed: {e}");
                    got = (true, true);
                }
            }
            if what == "fit" && got.0 {
                break;
            }
        }
        t0.elapsed().as_secs_f64() * 1e3
    };
    host.submit(&req(4_000_000, "render.view", json!({"photo": pid, "width": w, "height": h}))).ok();
    let ms = wait("fit");
    let m = host.call(&req(3_999_999, "host.memory", json!({})), Duration::from_secs(30))["result"].clone();
    let (now, peak) = footprint();
    println!("fit: {ms:.0} ms, footprint {now:.0} MB, peak {peak:.0}, {m}");
    if let Some(secs) = std::env::var("BENCH_HOLD").ok().and_then(|s| s.parse::<u64>().ok()) {
        println!("holding {secs} s (pid {})", std::process::id());
        std::thread::sleep(Duration::from_secs(secs));
    }
    if std::env::var("BENCH_LEAK").is_ok() {
        for i in 0..4u64 {
            host.call(&req(3_999_900 + i, "host.lifecycle", json!({"event": "memoryWarning"})), Duration::from_secs(30));
            let t0 = Instant::now();
            host.submit(&req(3_999_800 + i, "render.view", json!({"photo": pid, "width": w - 2 - 2 * i, "height": h}))).ok();
            let ms = wait("fit");
            println!("  decoded again ({:.0} ms, {ms:.0}): footprint {:.0} MB", t0.elapsed().as_secs_f64() * 1e3, footprint_mb());
        }
    }
    if std::env::var("BENCH_WARN").is_ok() {
        lightcraft_sysmem::release_free_memory();
        println!("  allocator release: footprint {:.0} MB", footprint_mb());
        host.call(&req(3_999_998, "host.lifecycle", json!({"event": "memoryWarning"})), Duration::from_secs(30));
        std::thread::sleep(Duration::from_millis(500));
        println!("  memory warning: footprint {:.0} MB", footprint_mb());
        if let Ok(o) = std::process::Command::new("/usr/bin/footprint").arg("-p").arg(std::process::id().to_string()).output() {
            for line in String::from_utf8_lossy(&o.stdout).lines().skip(4).take(14) {
                println!("    {line}");
            }
        }
    }
    let long = w.max(h) as f64;
    let mut n = 4_000_001;
    let mut ask = |z: f64, cx: f64, cy: f64, what: String| {
        let half = 0.5 / z;
        let visible = [cx - half, cy - half, cx + half, cy + half];
        n += 1;
        host.submit(&req(n, "render.view", json!({"photo": pid, "width": w, "height": h, "zoom": {"drawn": long * z, "visible": visible}}))).ok();
        let ms = wait(&what);
        // `BENCH_SETTLE_MS`: measure that long after the window arrives (a render's buffers are freed after it)
        if let Some(settle) = std::env::var("BENCH_SETTLE_MS").ok().and_then(|s| s.parse::<u64>().ok()) {
            std::thread::sleep(Duration::from_millis(settle));
        }
        n += 1;
        let all = host.call(&req(n, "host.memory", json!({})), Duration::from_secs(30))["result"].clone();
        let (m, v) = (&all["engine"], &all["views"]["stageCaches"]);
        let mb = |v: &Value| v.as_f64().unwrap_or(0.0) / 1048576.0;
        println!(
            "{what}: {ms:.0} ms, footprint {:.0} MB, peak {:.0} (view stages: cpu {:.0}, gpu {:.0}; sources: preview {:.0}, full {:.0}; gpu {:.0} + pooled {:.0}; rendered {:.0})",
            footprint().0,
            footprint().1,
            mb(&v["cpuBytes"]),
            mb(&v["gpuBytes"]),
            mb(&m["previewSources"]["bytes"]),
            mb(&m["fullSource"]["bytes"]),
            mb(&m["gpu"]["allocated"]),
            mb(&m["gpu"]["pooled"]),
            mb(&m["rendered"]["bytes"])
        );
        // `BENCH_CATEGORIES=1`: where the footprint is (malloc, the GPU's buffers, …)
        if std::env::var("BENCH_CATEGORIES").is_ok()
            && let Ok(o) = std::process::Command::new("/usr/bin/footprint").arg("-p").arg(std::process::id().to_string()).output()
        {
            for line in String::from_utf8_lossy(&o.stdout).lines().skip(5).take(30) {
                println!("    {line}");
            }
        }
    };
    // `BENCH_ZOOMS=8,4`: the zoom steps (default 2, 4, 8; the first decodes the original)
    let zooms: Vec<f64> =
        std::env::var("BENCH_ZOOMS").map_or_else(|_| vec![2.0, 4.0, 8.0], |z| z.split(',').filter_map(|z| z.trim().parse().ok()).collect());
    for z in zooms {
        ask(z, 0.5, 0.5, format!("zoom {z}x"));
    }
    for i in 0..8 {
        let f = f64::from(i) / 7.0;
        ask(8.0, 0.0625 + f * 0.875, 0.3 + 0.4 * f, format!("pan {i} at 8x"));
    }
    std::thread::sleep(Duration::from_secs(4));
    println!("4 s later: footprint {:.0} MB", footprint_mb());
}
