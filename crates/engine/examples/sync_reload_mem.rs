//! Peak memory of a sync reload (the server's whole library read again over the open one, HB-236): a library of N
//! photos (a tenth edited) is filled in from an in-process server, then the device reloads it. Prints the bytes the
//! open library holds and the peak above that during the reload, counted by the global allocator.
//!
//! `cargo run --release -p lightcraft-engine --example sync_reload_mem -- [photos, default 20000]`
use std::sync::Arc;

use lightcraft_catalog::sync::{Pull, ServerCore, compact_page, proto};
use lightcraft_catalog::{MemStore, Op, Photo, PhotoId, Source};
use lightcraft_engine::Session;
use lightcraft_engine::cloud_sync::{Done, Task};
use serde_json::json;

#[global_allocator]
static ALLOC: lightcraft_sysmem::CountingAlloc = lightcraft_sysmem::CountingAlloc;

const MB: f64 = 1024.0 * 1024.0;

struct Server {
    core: ServerCore,
    /// Answer the next pull `410` (the log no longer goes back that far): the device reloads.
    gone: bool,
}

impl Server {
    fn handle(&mut self, t: &Task) -> Done {
        let Task::Http { id, method, url, .. } = t else { return Done::failed(t.id(), "no previews here") };
        let ok = |body: serde_json::Value| Done { id: *id, status: 200, body: body.to_string() };
        let path = url.trim_start_matches("http://bench");
        let q = path.split_once('?').map_or("", |(_, q)| q);
        let arg = |k: &str| q.split('&').filter_map(|kv| kv.split_once('=')).find(|(key, _)| *key == k).map(|(_, v)| v.to_string());
        match (*method, path.split('?').next().unwrap_or("")) {
            ("POST", "/api/login") => ok(json!({"token": "t", "device": 1, "space": 1, "library": "lib"})),
            ("GET", "/api/snapshot") => {
                let limit = arg("limit").and_then(|l| l.parse().ok()).unwrap_or(2000);
                match self.core.snapshot_page(arg("cursor").as_deref(), limit) {
                    Some(page) => match compact_page(proto::Snapshot { library: "lib".into(), ..page }) {
                        Ok(page) => Done { id: *id, status: 200, body: serde_json::to_string(&page).unwrap_or_default() },
                        Err(e) => Done { id: *id, status: 500, body: e.to_string() },
                    },
                    None => Done { id: *id, status: 410, body: "{}".into() },
                }
            }
            ("GET", "/api/ops") if std::mem::take(&mut self.gone) => Done { id: *id, status: 410, body: "{}".into() },
            ("GET", "/api/ops") => {
                let since = arg("since").and_then(|s| s.parse().ok()).unwrap_or(0);
                match self.core.since(since, 1000) {
                    Ok(Pull::Ops(ops)) => ok(json!({"head": self.core.head(), "ops": ops, "presets": 0})),
                    _ => Done { id: *id, status: 410, body: "{}".into() },
                }
            }
            ("GET", "/api/presets") => ok(json!({"version": 0, "presets": []})),
            _ => Done { id: *id, status: 404, body: json!({"error": "no such route"}).to_string() },
        }
    }
}

fn in_use() -> f64 {
    lightcraft_sysmem::counted().0 as f64 / MB
}

fn main() {
    let n: u64 = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(20_000);
    let Ok(mut core) = ServerCore::open(Box::new(MemStore::new())) else { return eprintln!("no server") };
    let ops: Vec<Op> = (1..=n)
        .map(|i| {
            let mut p = Photo::new(
                PhotoId(i),
                Source::File { path: format!("/Volumes/Photos/IMG_{i:06}.CR3") },
                &format!("IMG_{i:06}.CR3"),
                "CR3",
                6000,
                4000,
                "2026-01-01T00:00:00",
            );
            p.content_hash = Some(format!("{i:064x}"));
            if i % 10 == 0 {
                let mut d = (*p.develop).clone();
                d.light.exposure = 0.5;
                p.develop = Arc::new(d);
            }
            Op::AddPhoto { photo: Box::new(p) }
        })
        .collect();
    for chunk in ops.chunks(5000) {
        if let Err(e) = core.push(core.head(), chunk) {
            return eprintln!("server: {e:?}");
        }
    }
    drop(ops);
    let mut server = Server { core, gone: false };
    let dir = std::env::temp_dir().join(format!("sync-reload-mem-{}", std::process::id()));
    let mut s = Session::new().with_fs();
    if let Err(e) = s.open_library(&dir, false) {
        return eprintln!("library: {e}");
    }
    if let Err(e) = s.execute("sync.signIn", &json!({"server": "http://bench", "user": "u", "password": "p", "device": "bench"})) {
        return eprintln!("sign in: {e}");
    }
    let st = s.sync_now_with(100_000, &mut |t| server.handle(t));
    println!("filled in: {} photos, state {}", s.catalog.len(), st["state"]);
    let open = in_use();
    server.gone = true;
    let t = std::time::Instant::now();
    // (the peak counted while the device handles each answer: what the in-process server spends making them is not the
    // device's)
    let mut peak = 0;
    s.sync_soon();
    for _ in 0..100_000 {
        let tasks = s.sync_tasks();
        if tasks.is_empty() {
            break;
        }
        for t in tasks {
            let d = server.handle(&t);
            lightcraft_sysmem::reset_counted_peak();
            s.sync_done(d);
            peak = peak.max(lightcraft_sysmem::counted().1);
        }
    }
    let peak = peak as f64 / MB;
    println!(
        "reloaded in {:.2} s: {} photos, state {}",
        t.elapsed().as_secs_f64(),
        s.catalog.len(),
        s.sync_state().map_or("signed out", |st| st.state())
    );
    println!("in use before {open:.0} MB, peak during the reload {peak:.0} MB (+{:.0} MB), after {:.0} MB", peak - open, in_use());
    let _ = std::fs::remove_dir_all(&dir);
}
