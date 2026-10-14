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
