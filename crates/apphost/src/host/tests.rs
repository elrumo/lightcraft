use std::cell::RefCell;

use super::*;

/// A host with the events read so far and not yet asked for: one poll can bring several, and a test
/// waiting for one of them must not lose the others.
struct Watch {
    host: Host,
    backlog: RefCell<VecDeque<Value>>,
}

impl std::ops::Deref for Watch {
    type Target = Host;
    fn deref(&self) -> &Host {
        &self.host
    }
}

impl Watch {
    fn new(host: Host) -> Self {
        Watch { host, backlog: RefCell::new(VecDeque::new()) }
    }

    /// The first event, read now or before, that matches `want` (within 30 s).
    fn wait_for(&self, want: impl Fn(&Value) -> bool) -> Value {
        let t0 = Instant::now();
        loop {
            {
                let mut backlog = self.backlog.borrow_mut();
                if let Some(i) = backlog.iter().position(&want) {
                    return backlog.remove(i).unwrap();
                }
            }
            self.backlog.borrow_mut().extend(self.host.poll(Duration::from_millis(50)));
            assert!(t0.elapsed() < Duration::from_secs(30), "no matching event in {:?}", self.backlog.borrow());
        }
    }

    fn reply_to(&self, id: u64) -> Value {
        self.wait_for(|e| e["kind"] == "reply" && e["id"] == id)
    }
}

fn demo() -> Watch {
    let tmp = std::env::temp_dir().join(format!("lc-host-{}-{}", std::process::id(), lightcraft_preview::next_tick()));
    std::fs::create_dir_all(&tmp).unwrap();
    let config = OpenConfig { demo: true, tmp: Some(tmp.to_string_lossy().to_string()), render_threads: Some(2), ..Default::default() };
    Watch::new(Host::open(config).unwrap().0)
}

fn req(id: u64, method: &str, params: Value) -> Vec<u8> {
    serde_json::to_vec(&json!({"id": id, "method": method, "params": params})).unwrap()
}

// The demo library opens on the engine thread and answers commands, in order, as reply events
#[test]
fn commands_are_answered_in_order_as_events() {
    let h = demo();
    h.submit(&req(1, "engine.execute", json!({"command": "catalog.query", "params": {"limit": 3}}))).unwrap();
    h.submit(&req(2, "engine.execute", json!({"command": "no.such.command"}))).unwrap();
    let first = h.reply_to(1);
    assert_eq!(first["result"]["photos"].as_array().map(Vec::len), Some(3), "{first}");
    let second = h.reply_to(2);
    assert_eq!(second["error"]["code"], "unknownCommand", "{second}");
    let commands = h.call(&req(3, "engine.commands", json!({})), Duration::from_secs(30));
    let ids: Vec<&str> = commands["result"].as_array().unwrap().iter().filter_map(|c| c["id"].as_str()).collect();
    assert!(ids.contains(&"develop.set") && ids.contains(&"app.export"), "{ids:?}");
}

// What the frontend sends that isn't a request is an answer, never a crash
#[test]
fn bad_requests_are_errors() {
    let h = demo();
    assert_eq!(h.submit(b"{not json").unwrap_err().code, "badRequest");
    let r = h.call(&req(4, "no.such.method", json!({})), Duration::from_secs(30));
    assert_eq!(r["error"]["code"], "unknownMethod", "{r}");
    let r = h.call(&req(5, "render.view", json!({"photo": 1})), Duration::from_secs(30));
    assert_eq!(r["error"]["code"], "badParams", "{r}");
    let r = h.call(&req(6, "render.cancel", json!({"slots": ["nope"]})), Duration::from_secs(30));
    assert_eq!(r["error"]["code"], "badParams", "{r}");
    let r = h.call(&req(7, "ui.screenshot", json!({})), Duration::from_secs(1));
    assert!(r["error"].is_object(), "nobody answers the UI in a test: {r}");
}

// A thumbnail arrives as an image event whose pixels are a lease the frontend takes and releases
#[test]
fn a_thumbnail_arrives_as_a_leased_image() {
    let h = demo();
    let state = h.call(&req(1, "host.state", json!({})), Duration::from_secs(30));
    assert!(state["result"]["visibleCount"].as_u64().unwrap() > 0, "{state}");
    let first = h.call(&req(2, "engine.execute", json!({"command": "catalog.query", "params": {"limit": 1}})), Duration::from_secs(30));
    let photo = first["result"]["photos"][0]["id"].as_u64().unwrap();
    h.submit(&req(3, "render.thumb", json!({"photo": photo, "edge": 128}))).unwrap();
    let img = h.wait_for(|e| e["kind"] == "image" && e["slot"] == format!("thumb:{photo}"));
    let token = img["token"].as_u64().unwrap();
    let pixels = h.leases().take_image(token).expect("the picture is there until released");
    assert_eq!((pixels.width, pixels.height), (img["width"].as_u64().unwrap() as u32, img["height"].as_u64().unwrap() as u32));
    assert!(pixels.width <= 128 && pixels.height <= 128 && pixels.len == (pixels.width * pixels.height * 4) as usize);
    h.leases().release(token);
    assert!(h.leases().take_image(token).is_none());
}

// A SwiftUI cell asks for its thumbnail once, when it appears. The host keeps asking while the cell is
// on screen (the engine downloads a synced photo's preview only while asked), and the render that
// failed for want of the preview runs again by itself once the preview has landed
#[test]
fn a_thumbnail_asked_once_renders_when_its_preview_lands() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let tmp = std::env::temp_dir().join(format!("lc-host-{}-{}", std::process::id(), lightcraft_preview::next_tick()));
    std::fs::create_dir_all(&tmp).unwrap();
    let config = OpenConfig { demo: true, tmp: Some(tmp.to_string_lossy().to_string()), render_threads: Some(1), ..Default::default() };
    let (events, (tx, _rx)) = (Arc::new(Events::default()), channel());
    let mut e = Engine::start(&config, events.clone(), Arc::new(Leases::default()), tx).unwrap();
    let landed = Arc::new(AtomicBool::new(false));
    let l = landed.clone();
    e.session.media.file_loader = Some(Arc::new(move |_, _| {
        if !l.load(Ordering::SeqCst) {
            return Err("still downloading".into());
        }
        let mut image = lightcraft_raster::Rgb32f::new(8, 8);
        image.data.fill([0.3, 0.2, 0.1]);
        Ok((image, Default::default()))
    }));
    let id = PhotoId(9_000_001);
    let photo = lightcraft_catalog::Photo::new(id, lightcraft_catalog::Source::File { path: "a.jpg".into() }, "a.jpg", "JPEG", 8, 8, "2026-01-01");
    e.session.catalog.apply(lightcraft_catalog::Op::AddPhoto { photo: Box::new(photo) }).unwrap();
    let slot = format!("thumb:{}", id.0);
    let seen = |e: &mut Engine, want: &dyn Fn(&Value) -> bool| {
        let t0 = Instant::now();
        loop {
            e.tick();
            if events.poll(Duration::from_millis(5)).iter().any(want) {
                return;
            }
            assert!(t0.elapsed() < Duration::from_secs(30), "no matching event");
        }
    };
    e.handle(Msg::Request(Request { id: 1, method: "render.thumb".into(), params: json!({"photo": id.0, "edge": 64}) }));
    seen(&mut e, &|ev| ev["kind"] == "renderFailed" && ev["slot"] == slot.as_str());
    assert!(e.thumbs_wanted.contains_key(&id), "a cell without a picture is asked for again");
    landed.store(true, Ordering::SeqCst);
    e.session.media.note_arrived(id);
    seen(&mut e, &|ev| ev["kind"] == "image" && ev["slot"] == slot.as_str());
    e.tick();
    assert!(!e.thumbs_wanted.contains_key(&id), "not asked for again once it has its picture");
    // a cell that scrolled away is let go
    e.thumbs_wanted.insert(id, (64, 10));
    e.handle(Msg::Request(Request { id: 2, method: "render.retain".into(), params: json!({"thumbs": []}) }));
    assert!(e.thumbs_wanted.is_empty());
}

// Against a live sync server, as the SwiftUI grid asks: each thumbnail once, when its cell appears. Not run in
// CI: `LC_LIVE_LIBRARY=<a library signed in to the server> [LC_LIVE_THUMBS=120] cargo test -p lightcraft-apphost
// --release -- --ignored --nocapture live_thumbs` (the library's photos must not have their originals here)
#[test]
#[ignore = "needs a library signed in to a live sync server (LC_LIVE_LIBRARY)"]
fn live_thumbs_asked_once_all_arrive() {
    let Some(dir) = std::env::var("LC_LIVE_LIBRARY").ok() else { return };
    let want = std::env::var("LC_LIVE_THUMBS").ok().and_then(|n| n.parse::<usize>().ok()).unwrap_or(120);
    let tmp = std::env::temp_dir().join(format!("lc-live-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let config = OpenConfig { library: Some(dir), tmp: Some(tmp.to_string_lossy().to_string()), render_threads: Some(2), ..Default::default() };
    let h = Watch::new(Host::open(config).unwrap().0);
    let t0 = Instant::now();
    let ids: Vec<u64> = loop {
        let r = h.call(&req(1, "engine.execute", json!({"command": "catalog.query", "params": {"limit": want}})), Duration::from_secs(30));
        let ids: Vec<u64> = r["result"]["photos"].as_array().into_iter().flatten().filter_map(|p| p["id"].as_u64()).collect();
        if ids.len() >= want || (t0.elapsed() > Duration::from_secs(60) && !ids.is_empty()) {
            break ids;
        }
        assert!(t0.elapsed() < Duration::from_secs(300), "the library stayed empty: nothing on the server?");
        std::thread::sleep(Duration::from_secs(1));
    };
    println!("{} photos listed after {:.1?}", ids.len(), t0.elapsed());
    for id in &ids {
        h.submit(&req(2, "render.thumb", json!({"photo": id, "edge": 256}))).unwrap();
    }
    h.submit(&req(3, "render.retain", json!({"thumbs": ids}))).unwrap();
    let (t1, mut last, mut got) = (Instant::now(), Instant::now(), std::collections::HashSet::new());
    while got.len() < ids.len() && t1.elapsed() < Duration::from_secs(90) {
        for ev in h.host.poll(Duration::from_millis(100)) {
            if ev["kind"] == "image" && ev["slot"].as_str().is_some_and(|s| s.starts_with("thumb:")) {
                got.insert(ev["slot"].as_str().unwrap_or_default().to_string());
            }
        }
        if last.elapsed() > Duration::from_secs(5) {
            println!("{:>5.1?}: {} of {} thumbnails", t1.elapsed(), got.len(), ids.len());
            last = Instant::now();
        }
    }
    println!("{} of {} thumbnails after {:.1?}", got.len(), ids.len(), t1.elapsed());
    assert_eq!(got.len(), ids.len(), "cells asked for once stay blank");
}

// A slider drag: drafts while it moves, one undo step and a final render when it ends
#[test]
fn a_drag_renders_drafts_then_a_final_view() {
    let h = demo();
    let first = h.call(&req(1, "engine.execute", json!({"command": "catalog.query", "params": {"limit": 1}})), Duration::from_secs(30));
    let photo = first["result"]["photos"][0]["id"].as_u64().unwrap();
    h.call(&req(2, "engine.execute", json!({"command": "library.select", "params": {"ids": [photo]}})), Duration::from_secs(30));
    h.call(&req(3, "engine.execute", json!({"command": "develop.beginInteraction", "params": {"label": "Exposure"}})), Duration::from_secs(30));
    h.call(
        &req(4, "engine.execute", json!({"command": "develop.set", "params": {"control": "light.exposure", "value": 0.5}})),
        Duration::from_secs(30),
    );
    h.submit(&req(5, "render.view", json!({"width": 160, "height": 120, "draft": true}))).unwrap();
    let draft = h.wait_for(|e| e["kind"] == "image" && e["slot"] == "main");
    assert_eq!((draft["draft"].as_bool(), draft["final"].as_bool()), (Some(true), Some(false)), "{draft}");
    assert!(draft["histogram"]["luma"].as_array().is_some_and(|b| b.len() == 256), "the loupe's picture has its histogram");
    h.call(&req(6, "engine.execute", json!({"command": "develop.endInteraction", "params": {}})), Duration::from_secs(30));
    let state = h.wait_for(|e| e["kind"] == "state" && e["state"]["undo"].is_string() && e["state"]["interaction"].is_null());
    assert!(state["state"]["undo"].as_str().unwrap().contains("Exposure"), "{state}");
    h.submit(&req(7, "render.view", json!({"width": 160, "height": 120}))).unwrap();
    let full = h.wait_for(|e| e["kind"] == "image" && e["slot"] == "main" && e["draft"] == false);
    assert_eq!(full["final"], true, "{full}");
    // drafts are smaller, as egui's loupe draws them (`DraftBudget`): a full-size one per tick lags on a phone
    assert!(draft["width"].as_u64() < full["width"].as_u64(), "draft {draft} vs full {full}");
}

// Closing saves and stops; the host refuses what comes after, and dropping it again is fine
#[test]
fn close_stops_the_engine() {
    let h = demo();
    h.close(Duration::from_secs(30));
    h.wait_for(|e| e["kind"] == "closed");
    assert_eq!(h.submit(&req(1, "host.state", json!({}))).unwrap_err().code, "closed");
    assert_eq!(h.call(&req(2, "host.state", json!({})), Duration::from_secs(1))["error"]["code"], "closed");
    h.close(Duration::from_secs(1));
}

// A library on disk: an edit made through the host is there when the library is opened again
#[test]
fn edits_are_saved_on_close() {
    let root = std::env::temp_dir().join(format!("lc-host-lib-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let config = OpenConfig {
        library: Some(root.join("Lib").to_string_lossy().to_string()),
        tmp: Some(root.join("tmp").to_string_lossy().to_string()),
        ..Default::default()
    };
    let (h, opened) = Host::open(config.clone()).unwrap();
    assert_eq!(opened["demo"], false, "{opened}");
    let first = h.call(&req(1, "engine.execute", json!({"command": "catalog.query", "params": {"limit": 1}})), Duration::from_secs(30));
    let photo = first["result"]["photos"][0]["id"].as_u64().unwrap();
    h.call(&req(2, "engine.execute", json!({"command": "library.select", "params": {"ids": [photo]}})), Duration::from_secs(30));
    let r = h.call(&req(3, "engine.execute", json!({"command": "photo.rate", "params": {"rating": 4}})), Duration::from_secs(30));
    assert!(r["error"].is_null(), "{r}");
    drop(h);
    let (h, _) = Host::open(config).unwrap();
    let again = h.call(&req(1, "engine.execute", json!({"command": "catalog.query", "params": {"limit": 1}})), Duration::from_secs(30));
    assert_eq!(again["result"]["photos"][0]["rating"], 4, "{again}");
    drop(h);
    let _ = std::fs::remove_dir_all(&root);
}

// An automation script runs at start: engine requests are answered, UI ones go to the frontend
#[test]
fn a_script_runs_and_writes_its_replies() {
    let tmp = std::env::temp_dir().join(format!("lc-host-script-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    std::fs::write(
        tmp.join("tour.jsonl"),
        "{\"method\": \"host.state\"}\nnot json\n{\"sleep\": 1}\n{\"method\": \"ui.screenshot\", \"params\": {\"path\": \"shot.png\"}}\n",
    )
    .unwrap();
    let config = OpenConfig { demo: true, tmp: Some(tmp.to_string_lossy().to_string()), script: Some("tour.jsonl".into()), ..Default::default() };
    let h = Watch::new(Host::open(config).unwrap().0);
    let ui = h.wait_for(|e| e["kind"] == "uiRequest");
    assert_eq!(ui["method"], "ui.screenshot");
    assert_eq!(ui["params"]["path"], tmp.join("shot.png").to_string_lossy().as_ref(), "a relative path is beside the script");
    h.submit(&req(1, "ui.reply", json!({"id": ui["id"], "result": {"width": 3}}))).unwrap();
    let out = tmp.join("tour.out");
    let t0 = Instant::now();
    let text = loop {
        let text = std::fs::read_to_string(&out).unwrap_or_default();
        if text.lines().count() >= 4 {
            break text;
        }
        assert!(t0.elapsed() < Duration::from_secs(30), "{text}");
        std::thread::sleep(Duration::from_millis(10));
    };
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines[0].contains("visibleCount"), "{text}");
    assert!(lines[1].contains("not a request"), "{text}");
    assert!(lines[3].contains("\"width\":3"), "{text}");
    drop(h);
    let _ = std::fs::remove_dir_all(&tmp);
}

// Export Now runs on a worker: the engine keeps answering meanwhile, the activity stack shows it,
// and the files come in the reply when it ends; a second export waits its turn
#[test]
fn exports_run_in_the_background_and_answer_when_done() {
    let h = demo();
    let dir = std::env::temp_dir().join(format!("lc-host-export-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let ids: Vec<u64> =
        h.call(&req(1, "engine.execute", json!({"command": "catalog.query", "params": {"limit": 4}})), Duration::from_secs(30))["result"]["photos"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["id"].as_u64().unwrap())
            .collect();
    let params = json!({"dir": dir.to_string_lossy(), "ids": ids, "longEdge": 64, "format": "png"});
    h.submit(&req(2, "engine.execute", json!({"command": "app.export", "params": params}))).unwrap();
    h.submit(&req(3, "engine.execute", json!({"command": "app.export", "params": params}))).unwrap();
    h.submit(&req(4, "host.state", json!({}))).unwrap();
    // the state answers while the export runs; the second export is refused
    let state = h.reply_to(4);
    assert!(state["result"]["visibleCount"].as_u64().unwrap() > 0, "{state}");
    let second = h.reply_to(3);
    assert!(second["error"]["message"].as_str().unwrap().contains("already running"), "{second}");
    let done = h.reply_to(2);
    let files = done["result"]["files"].as_array().unwrap();
    assert_eq!(files.len(), 4, "{done}");
    assert_eq!(done["result"]["cancelled"], false);
    assert!(Path::new(files[0]["path"].as_str().unwrap()).is_file());
    // a photo dragged out is exported without becoming what Export with Previous repeats
    let drag = json!({"dir": dir.to_string_lossy(), "ids": [ids[1]], "longEdge": 32, "format": "jpeg", "remember": false});
    let dragged = h.call(&req(7, "engine.execute", json!({"command": "app.export", "params": drag})), Duration::from_secs(60));
    assert_eq!(dragged["result"]["files"].as_array().map(Vec::len), Some(1), "{dragged}");
    // Export with Previous: the same options again, for the photo chosen now
    let one = ids[0];
    h.call(&req(5, "engine.execute", json!({"command": "library.select", "params": {"ids": [one], "active": one}})), Duration::from_secs(30));
    let again = h.call(&req(6, "host.exportPrevious", json!({})), Duration::from_secs(60));
    assert_eq!(again["result"]["files"].as_array().map(Vec::len), Some(1), "{again}");
    let png = again["result"]["files"][0]["path"].as_str().unwrap_or_default().to_lowercase();
    assert!(png.ends_with(".png"), "the PNG export is the one repeated, not the dragged JPEG: {again}");
    let _ = std::fs::remove_dir_all(&dir);
}

// Import as the phone does it: scan for the review (a thumbnail for a candidate), then import the
// chosen ones in the background — one undo step, the first photo selected
#[test]
fn photos_are_scanned_reviewed_and_imported_in_the_background() {
    let base = std::env::temp_dir().join(format!("lc-host-import-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let (src, lib, tmp) = (base.join("src"), base.join("lib"), base.join("tmp"));
    std::fs::create_dir_all(&tmp).unwrap();
    let mut demo = Session::with_demo();
    let ids: Vec<u64> = demo.visible_cloned().iter().take(3).map(|id| id.0).collect();
    crate::export::export_now(&mut demo, &json!({"dir": src.to_string_lossy(), "ids": ids, "longEdge": 48, "format": "png"}), &mut |_, _, _| true)
        .unwrap();
    let config = OpenConfig {
        library: Some(lib.to_string_lossy().to_string()),
        tmp: Some(tmp.to_string_lossy().to_string()),
        render_threads: Some(2),
        ..Default::default()
    };
    let h = Watch::new(Host::open(config).unwrap().0);
    let before = h.call(&req(9, "host.state", json!({})), Duration::from_secs(30))["result"]["visibleCount"].as_u64().unwrap();
    h.submit(&req(1, "import.scan", json!({"paths": [src.to_string_lossy()]}))).unwrap();
    let scan = h.reply_to(1);
    let candidates = scan["result"]["candidates"].as_array().unwrap();
    assert_eq!(candidates.len(), 3, "{scan}");
    assert_eq!(candidates[0]["inTrash"], false);
    h.submit(&req(2, "render.candidate", json!({"index": 0, "edge": 64}))).unwrap();
    assert_eq!(h.reply_to(2)["result"]["slot"], "import:0");
    h.wait_for(|e| e["kind"] == "image" && e["slot"] == "import:0");
    h.submit(&req(3, "import.start", json!({"indexes": [0, 1, 2], "mode": "add", "albumName": "Trip"}))).unwrap();
    let done = h.reply_to(3);
    assert_eq!(done["result"]["imported"], 3, "{done}");
    assert_eq!(done["result"]["cancelled"], false);
    assert!(done["result"]["album"].as_u64().is_some(), "a new album: {done}");
    let state = h.call(&req(4, "host.state", json!({})), Duration::from_secs(30));
    assert_eq!(state["result"]["visibleCount"].as_u64(), Some(before + 3), "{state}");
    assert_eq!(state["result"]["undo"], "Add 3 Photos");
    let _ = std::fs::remove_dir_all(&base);
}

// Previews of a preset, a profile and a version on the active photo come as `variant:` pictures
#[test]
fn presets_profiles_and_versions_preview() {
    let h = demo();
    let preset = h.call(&req(1, "engine.execute", json!({"command": "presets.list"})), Duration::from_secs(30))["result"][0]["id"].clone();
    h.submit(&req(2, "render.variant", json!({"preset": preset, "edge": 96}))).unwrap();
    let slot = h.reply_to(2)["result"]["slot"].as_str().unwrap().to_string();
    assert!(slot.starts_with("variant:"), "{slot}");
    let img = h.wait_for(|e| e["kind"] == "image" && e["slot"] == slot.as_str());
    assert!(img["width"].as_u64().unwrap() <= 96, "{img}");
    h.submit(&req(3, "render.variant", json!({"profile": "lc.mono", "edge": 96}))).unwrap();
    let mono = h.reply_to(3)["result"]["slot"].as_str().unwrap().to_string();
    assert_ne!(mono, slot, "another look, another picture");
    h.wait_for(|e| e["kind"] == "image" && e["slot"] == mono.as_str());
    h.call(&req(4, "engine.execute", json!({"command": "version.create", "params": {"name": "A"}})), Duration::from_secs(30));
    h.submit(&req(5, "render.variant", json!({"version": 0, "edge": 96}))).unwrap();
    assert!(h.reply_to(5)["result"]["slot"].as_str().unwrap().starts_with("variant:"));
    h.submit(&req(6, "render.variant", json!({"edge": 96}))).unwrap();
    assert!(h.reply_to(6)["error"].is_object(), "nothing to preview");
}

/// `render.view`'s overlay: the selected mask with its colour, Visualize Spots, nothing for nonsense.
#[test]
fn view_overlays_parse() {
    use lightcraft_pipeline::{MaskView, Overlay};
    assert_eq!(
        overlay(Some(&json!({"kind": "mask", "id": 3, "color": [1, 2, 3], "opacity": 80}))),
        Overlay::Mask { id: 3, view: MaskView::default(), color: [1, 2, 3], opacity: 80 }
    );
    assert_eq!(
        overlay(Some(&json!({"kind": "allMasks", "opacity": 30}))),
        Overlay::AllMasks { view: MaskView::default(), color: [230, 30, 40], opacity: 30 },
        "every mask at once (Show All Mask Overlays)"
    );
    assert_eq!(overlay(Some(&json!({"kind": "spots", "threshold": 900}))), Overlay::Spots(100));
    assert_eq!(overlay(Some(&json!({"kind": "mask"}))), Overlay::None, "a mask overlay needs its id");
    assert_eq!(overlay(Some(&json!("hdr"))), Overlay::None);
    assert_eq!(overlay(None), Overlay::None);
}

// Zoomed in: what is on screen comes sharp, as a window of the zoomed frame that says where it goes;
// back at fit no window is asked for
#[test]
fn a_zoomed_view_gets_a_window_of_what_is_on_screen() {
    let h = demo();
    let first = h.call(&req(1, "engine.execute", json!({"command": "catalog.query", "params": {"limit": 1}})), Duration::from_secs(30));
    let photo = first["result"]["photos"][0]["id"].as_u64().unwrap();
    h.call(&req(2, "engine.execute", json!({"command": "library.select", "params": {"ids": [photo]}})), Duration::from_secs(30));
    // a 400 px canvas showing the photo 1200 px long: the middle third on screen
    let zoom = json!({"drawn": 1200, "visible": [0.333, 0.333, 0.667, 0.667]});
    h.submit(&req(3, "render.view", json!({"width": 400, "height": 300, "zoom": zoom}))).unwrap();
    let region = h.wait_for(|e| e["kind"] == "image" && e["slot"] == "region");
    let w = &region["window"];
    let n = |v: &Value| v.as_u64().unwrap();
    let (fw, fh, x, y, ww, wh) = (n(&w["full"][0]), n(&w["full"][1]), n(&w["x"]), n(&w["y"]), n(&w["w"]), n(&w["h"]));
    assert!(fw.max(fh) <= 1200 && fw.max(fh) > 400, "a frame larger than the canvas: {region}");
    assert!(x <= fw / 3 && x + ww >= fw * 2 / 3 && y <= fh / 3 && y + wh >= fh * 2 / 3, "it covers what is on screen: {region}");
    assert_eq!((n(&region["width"]), n(&region["height"])), (ww, wh), "the picture is the window");
    // the whole-frame render stays canvas-sized and has no window
    let main = h.wait_for(|e| e["kind"] == "image" && e["slot"] == "main");
    assert!(n(&main["width"]).max(n(&main["height"])) <= 512 && main["window"].is_null(), "{main}");
}

// People's faces: a face box renders as its own small picture
#[test]
fn a_face_renders_on_its_own() {
    let h = demo();
    let first = h.call(&req(1, "engine.execute", json!({"command": "catalog.query", "params": {"limit": 1}})), Duration::from_secs(30));
    let photo = first["result"]["photos"][0]["id"].as_u64().unwrap();
    let r = h.call(&req(2, "render.face", json!({"photo": photo, "rect": [0.3, 0.3, 0.5, 0.6], "edge": 96})), Duration::from_secs(30));
    let slot = r["result"]["slot"].as_str().unwrap().to_string();
    assert!(slot.starts_with("variant:"), "{r}");
    let image = h.wait_for(|e| e["kind"] == "image" && e["slot"] == slot.as_str());
    assert!(image["width"].as_u64().unwrap() <= 96 && image["height"].as_u64().unwrap() <= 96, "{image}");
    let bad = h.call(&req(3, "render.face", json!({"photo": photo, "rect": {"x0": "a"}})), Duration::from_secs(30));
    assert_eq!(bad["error"]["code"], "badParams", "{bad}");
    // the photo as it is, at a size of its own (Compare, Survey)
    let r = h.call(&req(4, "render.variant", json!({"photo": photo, "edited": true, "edge": 200})), Duration::from_secs(30));
    let slot = r["result"]["slot"].as_str().unwrap().to_string();
    let image = h.wait_for(|e| e["kind"] == "image" && e["slot"] == slot.as_str());
    assert!(image["width"].as_u64().unwrap() <= 200 && image["height"].as_u64().unwrap() <= 200, "{image}");
}

// Photo Merge as the phone does it: previews that a newer one replaces, then the full merge in the
// background, which says how it went (the demo's photos may not merge: either way it answers)
#[test]
fn merges_preview_and_run_in_the_background() {
    let h = demo();
    // two photo files (a merge needs files, not the demo's drawn photos): the demo's, exported
    let dir = std::env::temp_dir().join(format!("lc-host-merge-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let first = h.call(&req(1, "engine.execute", json!({"command": "catalog.query", "params": {"limit": 2}})), Duration::from_secs(30));
    let demo_ids: Vec<u64> = first["result"]["photos"].as_array().unwrap().iter().map(|p| p["id"].as_u64().unwrap()).collect();
    let exported = h.call(
        &req(
            10,
            "engine.execute",
            json!({"command": "app.export", "params": {"dir": dir.to_string_lossy(), "ids": demo_ids, "longEdge": 96, "format": "png"}}),
        ),
        Duration::from_secs(60),
    );
    let files: Vec<String> = exported["result"]["files"].as_array().unwrap().iter().map(|f| f["path"].as_str().unwrap().to_string()).collect();
    let imported = h.call(&req(11, "engine.execute", json!({"command": "library.import", "params": {"paths": files}})), Duration::from_secs(60));
    let ids: Vec<u64> = imported["result"]["imported"].as_array().map(|a| a.iter().filter_map(|v| v.as_u64()).collect()).unwrap_or_default();
    assert_eq!(ids.len(), 2, "{imported}");
    h.submit(&req(2, "host.merge.preview", json!({"command": "merge.hdr", "ids": ids}))).unwrap();
    h.submit(&req(3, "host.merge.preview", json!({"command": "merge.hdr", "ids": ids, "options": {"deghost": "low"}}))).unwrap();
    // the first is answered too: dropped for the second, or drawn before it came
    let older = h.reply_to(2);
    assert!(older["error"]["code"] == "cancelled" || older["result"].is_object(), "{older}");
    let newer = h.reply_to(3);
    assert!(newer["result"]["slot"] == "merge" || newer["error"]["message"].is_string(), "{newer}");
    if newer["result"].is_object() {
        h.wait_for(|e| e["kind"] == "image" && e["slot"] == "merge");
    }
    let one = h.call(&req(4, "host.merge.start", json!({"command": "merge.hdr", "ids": [ids[0]]})), Duration::from_secs(30));
    assert!(one["error"]["message"].as_str().unwrap().contains("at least 2"), "{one}");
    let started = h.call(&req(5, "host.merge.start", json!({"command": "merge.hdr", "ids": ids})), Duration::from_secs(30));
    assert_eq!(started["result"]["started"], true, "{started}");
    let said = h.wait_for(|e| e["kind"] == "message" && e["text"].as_str().is_some_and(|t| t.starts_with("HDR merge")));
    assert!(said["text"].as_str().unwrap().contains("merge"), "{said}");
    let _ = std::fs::remove_dir_all(&dir);
}

// The UI commands' table, for the SwiftUI app's menu bar and keys: egui's own, shortcuts included
#[test]
fn the_ui_commands_are_listed_with_their_keys() {
    let h = demo();
    let r = h.call(&req(1, "host.uiCommands", json!({})), Duration::from_secs(30));
    let list = r["result"].as_array().unwrap();
    assert_eq!(list.len(), crate::ui_commands::UI_COMMANDS.len());
    let grid = list.iter().find(|c| c["id"] == "view.gridToggle").unwrap();
    assert_eq!((grid["shortcut"].as_str(), grid["label"].as_str()), (Some("G"), Some("Grid")), "{grid}");
}

#[test]
fn export_with_previous_needs_an_export_first() {
    let h = demo();
    let r = h.call(&req(1, "host.exportPrevious", json!({})), Duration::from_secs(30));
    assert!(r["error"]["message"].as_str().unwrap().contains("nothing exported yet"), "{r}");
}

// Soft proofing the loupe: a proof in another space gives another picture; a bad one is refused
#[test]
fn the_loupe_can_be_soft_proofed() {
    let h = demo();
    let first = h.call(&req(1, "engine.execute", json!({"command": "catalog.query", "params": {"limit": 1}})), Duration::from_secs(30));
    let photo = first["result"]["photos"][0]["id"].as_u64().unwrap();
    h.call(&req(2, "render.view", json!({"photo": photo, "width": 160, "height": 120})), Duration::from_secs(30));
    let plain = h.wait_for(|e| e["kind"] == "image" && e["slot"] == "main" && e["final"] == true);
    h.call(
        &req(3, "render.view", json!({"photo": photo, "width": 160, "height": 120, "proof": {"space": "srgb", "destWarning": true}})),
        Duration::from_secs(30),
    );
    let proofed = h.wait_for(|e| e["kind"] == "image" && e["slot"] == "main" && e["final"] == true && e["key"] != plain["key"]);
    assert_ne!(proofed["key"], plain["key"], "{proofed}");
    let bad = h.call(&req(4, "render.view", json!({"photo": photo, "width": 160, "height": 120, "proof": {"space": 7}})), Duration::from_secs(30));
    assert_eq!(bad["error"]["code"], "badParams", "{bad}");
}

// The sidebar's sections: hidden, folded and moved by the shared commands, on the arrangement the app keeps
#[test]
fn the_sidebar_is_arranged_by_the_shared_commands() {
    let h = demo();
    let r = h.call(&req(1, "host.sidebar", json!({})), Duration::from_secs(30));
    assert_eq!(r["result"]["shown"][0], "albums", "{r}");
    let hide = json!({"command": "view.sidebarSection", "params": {"section": "albums", "show": false}});
    let r = h.call(&req(2, "host.sidebar", hide), Duration::from_secs(30));
    assert!(!r["result"]["shown"].as_array().unwrap().contains(&json!("albums")), "{r}");
    let layout = r["result"]["layout"].clone();
    let up = json!({"layout": layout, "command": "view.sidebarMoveSection", "params": {"section": "keywords", "before": "byDate"}});
    let r = h.call(&req(3, "host.sidebar", up), Duration::from_secs(30));
    let shown: Vec<&str> = r["result"]["shown"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert!(shown.iter().position(|s| *s == "keywords") < shown.iter().position(|s| *s == "byDate"), "{r}");
    assert_eq!(r["result"]["layout"][0]["hidden"], true, "albums stays hidden: {r}");
    let reset = json!({"layout": r["result"]["layout"], "command": "view.sidebarReset"});
    let r = h.call(&req(4, "host.sidebar", reset), Duration::from_secs(30));
    assert_eq!(r["result"]["shown"][0], "albums", "{r}");
    let wrong = h.call(&req(5, "host.sidebar", json!({"command": "view.sidebarSection", "params": {"section": "nope"}})), Duration::from_secs(30));
    assert_eq!(wrong["error"]["code"], "badParams", "{wrong}");
}

// A Lightroom catalog is read on a worker and answered when that ends (found from the folder
// holding it): a broken one is an error, the status keeps what last ran, and a folder without a
// catalog is refused at once
#[test]
fn a_lightroom_catalog_is_read_in_the_background() {
    let h = demo();
    let dir = std::env::temp_dir().join(format!("lc-host-lightroom-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Lightroom")).unwrap();
    std::fs::write(dir.join("Lightroom").join("Lightroom Catalog.lrcat"), b"bad catalog").unwrap();
    let inspect =
        |n: u64, p: Value| h.call(&req(n, "engine.execute", json!({"command": "library.inspectLightroom", "params": p})), Duration::from_secs(60));
    let r = inspect(1, json!({"path": dir.to_string_lossy()}));
    assert!(r["error"]["message"].is_string(), "{r}");
    let status = inspect(2, json!({"status": true}));
    assert!(status["result"]["error"].is_string(), "{status}");
    assert!(status["result"]["path"].as_str().unwrap().ends_with("Lightroom Catalog.lrcat"), "{status}");
    std::fs::create_dir_all(dir.join("Empty")).unwrap();
    let none = inspect(3, json!({"path": dir.join("Empty").to_string_lossy()}));
    assert!(none["error"]["message"].as_str().unwrap().contains("no Lightroom Classic catalog"), "{none}");
    let _ = std::fs::remove_dir_all(&dir);
}

// Generative Expand started without waiting (as the phone's Generate does): the host stores the fill
// when the job ends and says so, and the engine answers meanwhile
#[test]
fn a_background_generate_is_stored_when_it_ends() {
    let h = demo();
    let ask = |id: u64, command: &str, params: Value| {
        h.call(&req(id, "engine.execute", json!({"command": command, "params": params})), Duration::from_secs(30))
    };
    ask(1, "crop.expand", json!({"on": true}));
    ask(2, "crop.set", json!({"rect": [0.0, -0.2, 1.0, 1.0]}));
    let started = ask(3, "crop.generate", json!({"model": "classic", "wait": false}));
    assert_eq!(started["result"]["started"], true, "{started}");
    let said = h.wait_for(|e| e["kind"] == "message");
    assert!(said["text"].as_str().unwrap_or_default().starts_with("New area painted with"), "{said}");
    let got = ask(4, "develop.get", json!({}));
    assert_eq!(got["result"]["expand_fill"]["model"], "classic", "{got}");
    let models = ask(5, "crop.expandModels", json!({}));
    assert!(models["result"]["running"].is_null(), "nothing left running: {models}");
}

// An HDR edit on a display that can show it comes with its HDR picture, a second lease (RGBA16F); an SDR
// display, or an SDR edit, gets the 8-bit picture alone (LR-VIEW-HDR-DISPLAY)
#[test]
fn an_hdr_edit_on_an_hdr_display_comes_with_its_hdr_picture() {
    let h = demo();
    let t = Duration::from_secs(30);
    let first = h.call(&req(1, "engine.execute", json!({"command": "catalog.query", "params": {"limit": 1}})), t);
    let photo = first["result"]["photos"][0]["id"].as_u64().unwrap();
    h.call(&req(2, "engine.execute", json!({"command": "library.select", "params": {"ids": [photo]}})), t);
    h.call(&req(3, "engine.execute", json!({"command": "develop.hdr", "params": {"enabled": true, "maxEv": 3}})), t);
    h.call(&req(4, "engine.execute", json!({"command": "develop.set", "params": {"control": "light.exposure", "value": 1.0}})), t);
    // a display without headroom: the SDR rendition
    h.submit(&req(5, "render.view", json!({"width": 160, "height": 120}))).unwrap();
    let sdr = h.wait_for(|e| e["kind"] == "image" && e["slot"] == "main" && e["draft"] == false);
    assert!(sdr.get("hdr").is_none(), "{sdr}");
    // a display with: the same edit, rendered again, with its float picture
    assert_eq!(h.call(&req(6, "render.hdrDisplay", json!({"on": true})), t)["result"], json!({}));
    h.submit(&req(7, "render.view", json!({"width": 160, "height": 120}))).unwrap();
    let hdr = h.wait_for(|e| e["kind"] == "image" && e["slot"] == "main" && e["draft"] == false && e.get("hdr").is_some());
    assert_ne!(hdr["key"], sdr["key"], "the HDR picture is its own render");
    let info = &hdr["hdr"];
    assert_eq!((info["width"].as_u64(), info["height"].as_u64()), (hdr["width"].as_u64(), hdr["height"].as_u64()), "{hdr}");
    assert_eq!(info["peak"].as_f64(), Some(8.0), "3 stops of headroom: {hdr}");
    let leased = h.leases().take_image(info["token"].as_u64().unwrap()).expect("the HDR lease");
    let (w, h_px) = (hdr["width"].as_u64().unwrap() as u32, hdr["height"].as_u64().unwrap() as usize);
    assert_eq!((leased.width, leased.stride, leased.len), (w, w * 8, leased.stride as usize * h_px));
    let sdr_lease = h.leases().take_image(hdr["token"].as_u64().unwrap()).expect("the 8-bit picture too");
    assert_eq!(sdr_lease.stride, sdr_lease.width * 4);
    // the display went away (the window moved to another): the SDR rendition again, no hdr
    h.call(&req(8, "render.hdrDisplay", json!({"on": false})), t);
    h.submit(&req(9, "render.view", json!({"width": 160, "height": 120}))).unwrap();
    let back = h.wait_for(|e| e["kind"] == "image" && e["slot"] == "main" && e["draft"] == false && e["key"] == sdr["key"]);
    assert!(back.get("hdr").is_none(), "{back}");
    assert_eq!(h.call(&req(10, "render.hdrDisplay", json!({})), t)["error"]["code"], "badParams");
}

/// A one-thread HTTP server for the admin API: logs each request (head and body) and answers by
/// `reply(first line, whole request)` with `(status, body)`.
fn fake_admin_server(reply: impl Fn(&str, &str) -> (u16, String) + Send + 'static) -> (u16, Arc<Mutex<Vec<String>>>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { break };
            let mut got = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                let text = String::from_utf8_lossy(&got).to_string();
                if let Some((head, body)) = text.split_once("\r\n\r\n") {
                    let want =
                        head.to_ascii_lowercase().split("content-length:").nth(1).and_then(|l| l.split_whitespace().next()?.parse::<usize>().ok());
                    if body.len() >= want.unwrap_or(0) {
                        break;
                    }
                }
                match s.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => got.extend_from_slice(&buf[..n]),
                }
            }
            let request = String::from_utf8_lossy(&got).to_string();
            let line = request.lines().next().unwrap_or_default().to_string();
            log.lock().unwrap().push(request.clone());
            let (status, body) = reply(&line, &request);
            let _ = write!(
                s,
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    (port, seen)
}

// The server's admin API from the phone: sign in (a wrong password is the server's own words), calls carry
// the admin token and nothing else does, a session the server forgot ends with `unauthorized`, and signing
// out tells the server. All on a worker: the host answers when the server has.
#[test]
fn the_admin_api_is_called_on_a_worker() {
    let (port, seen) = fake_admin_server(|line, request| {
        let signed = request.to_ascii_lowercase().contains("authorization: bearer t1");
        match line {
            l if l.starts_with("POST /api/admin/login") && request.contains(r#""password":"right""#) => {
                (200, r#"{"token":"T1","user":"ann"}"#.into())
            }
            l if l.starts_with("POST /api/admin/login") => (401, r#"{"error":"wrong user name or password, or not an admin"}"#.into()),
            l if l.starts_with("GET /api/admin/users/ann/folders/My%20Photos") && signed => (200, r#"{"folders":[]}"#.into()),
            l if l.starts_with("GET /api/admin/gone") => (401, r#"{"error":"sign in as an admin"}"#.into()),
            l if l.starts_with("POST /api/admin/logout") && signed => (200, "{}".into()),
            _ => (404, r#"{"error":"no admin route"}"#.into()),
        }
    });
    let server = format!("127.0.0.1:{port}");
    let h = demo();
    let ask = |n: u64, method: &str, params: Value| h.call(&req(n, method, params), Duration::from_secs(30));
    let call = |n: u64, verb: &str, path: Value| ask(n, "host.admin.call", json!({"method": verb, "path": path}));

    assert_eq!(ask(1, "host.admin.session", json!({}))["result"]["signedIn"], false);
    let early = call(2, "GET", json!(["users"]));
    assert_eq!(early["error"]["code"], "unauthorized", "{early}");

    let wrong = ask(3, "host.admin.signIn", json!({"server": server, "user": "ann", "password": "wrong"}));
    assert_eq!(wrong["error"]["message"], "Wrong user name or password, or not an admin", "{wrong}");
    assert_eq!(ask(4, "host.admin.session", json!({}))["result"]["signedIn"], false);

    let right = ask(5, "host.admin.signIn", json!({"server": server, "user": "ann", "password": "right"}));
    assert_eq!(right["result"]["user"], "ann", "{right}");
    assert_eq!(right["result"]["server"], format!("http://{server}"), "{right}");
    assert_eq!(ask(6, "host.admin.session", json!({}))["result"]["signedIn"], true);

    let folders = call(7, "GET", json!(["users", "ann", "folders", "My Photos"]));
    assert_eq!(folders["result"]["folders"], json!([]), "{folders}");
    assert!(seen.lock().unwrap().iter().any(|r| r.starts_with("GET /api/admin/users/ann/folders/My%20Photos ")), "{:?}", seen.lock().unwrap());

    let ended = call(8, "GET", json!(["gone"]));
    assert_eq!(ended["error"]["code"], "unauthorized", "{ended}");
    assert_eq!(ask(9, "host.admin.session", json!({}))["result"]["signedIn"], false);

    ask(10, "host.admin.signIn", json!({"server": server, "user": "ann", "password": "right"}));
    assert_eq!(ask(11, "host.admin.signOut", json!({}))["result"], json!({}));
    assert_eq!(ask(12, "host.admin.session", json!({}))["result"]["signedIn"], false);
    assert!(seen.lock().unwrap().iter().any(|r| r.starts_with("POST /api/admin/logout")), "the server was told");
}
