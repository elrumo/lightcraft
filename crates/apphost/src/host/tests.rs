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
