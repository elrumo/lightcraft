//! The sync status popover, the choice after signing in to a server that has a library, and the
//! Settings ▸ Sync page that doesn't cut its text off, with a tiny in-process server answering the
//! requests (the engine's own tests cover the protocol).

use std::path::{Path, PathBuf};
use std::time::Duration;

use lightcraft_catalog::sync::proto;
use lightcraft_catalog::{Catalog, Op, Photo, PhotoId, Source};
use lightcraft_engine::Session;
use lightcraft_engine::cloud_sync::{Done, Task};
use serde_json::{Value, json};

use crate::headless::Headless;
use crate::state::Dialog;
use crate::sync_ui::SyncExec;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(30);
const SETTLE: Duration = Duration::from_secs(120);
const PHONE: [f32; 2] = [390.0, 844.0];

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-sync-ui-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write_png(path: &Path, seed: u8) {
    let (w, h) = (96usize, 64usize);
    let data: Vec<[u8; 4]> = (0..w * h).map(|i| [(i % w * 2) as u8, (i / w * 3) as u8, seed, 255]).collect();
    let img = lightcraft_raster::Rgba8 { width: w, height: h, data };
    let bytes = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

/// The server's library: `n` photos with files this device doesn't have.
fn server_catalog(n: u64) -> String {
    let mut c = Catalog::new();
    for i in 1..=n {
        let hash = format!("{i:02x}").repeat(16);
        let mut p = Photo::new(
            PhotoId(i),
            Source::File { path: format!("web/{hash}/server-{i}.jpg") },
            &format!("server-{i}.jpg"),
            "JPEG",
            60,
            40,
            "2026-01-01T00:00:00",
        );
        p.content_hash = Some(hash);
        c.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    }
    c.to_snapshot()
}

/// A server that signs in, has `photos` photos and says what it is doing: the requests a device
/// makes at sign-in and while someone looks at the popover.
fn exec(photos: u64, activity: proto::Activity) -> SyncExec {
    exec_with(photos, activity, false)
}

/// [`exec`], optionally leaving the requests for photo files unanswered (`hang_files`).
fn exec_with(photos: u64, activity: proto::Activity, hang_files: bool) -> SyncExec {
    let catalog: Value = serde_json::from_str(&server_catalog(photos)).unwrap();
    Box::new(move |task: Task, tx, ctx| {
        let id = task.id();
        let Task::Http { method, url, .. } = &task else {
            let _ = tx.send(Done::failed(id, "no previews here"));
            return;
        };
        let path = url.strip_prefix("http://fake").unwrap_or("");
        let ok = |v: Value| Done { id, status: 200, body: v.to_string() };
        let done = match (*method, path) {
            ("POST", "/api/login") => ok(json!({"token": "t1", "device": 1, "space": 1, "library": "lib-1"})),
            // (an empty server library is at op 0: the first device uploads its library)
            ("GET", p) if p.starts_with("/api/snapshot") => {
                ok(json!({"library": "lib-1", "seq": if photos == 0 { 0 } else { 7 }, "catalog": catalog}))
            }
            ("POST", "/api/ops") => ok(json!({"head": 8})),
            // (photo files: never answered, so the transfers stay in flight and can be looked at)
            (_, p) if p.starts_with("/api/blobs/") && hang_files => return,
            ("GET", "/api/activity") => ok(serde_json::to_value(&activity).unwrap()),
            // (a server with 9 photos is the one whose disk is almost full: 4 of its 500 GB free)
            ("GET", "/api/usage") => {
                let free = if photos == 9 { 4_000_000_000u64 } else { 200_000_000_000u64 };
                ok(
                    json!({"photos": photos, "original": {"files": 3, "bytes": 3_000_000_000u64}, "disk": {"total": 500_000_000_000u64, "free": free}}),
                )
            }
            ("GET", p) if p.starts_with("/api/ops") => ok(json!({"head": 7, "ops": [], "presets": 0})),
            ("GET", "/api/presets") => ok(json!({"version": 0, "presets": []})),
            _ => Done { id, status: 404, body: json!({"error": "not here"}).to_string() },
        };
        let _ = tx.send(done);
        ctx.request_repaint();
    })
}

/// A library on disk with the PNGs `files` (name, seed) imported, in a window of `size`.
fn app(tag: &str, files: &[(&str, u8)], exec: SyncExec, size: [f32; 2]) -> (Headless, PathBuf) {
    app_at(tag, files, exec, size, 1.0)
}

/// [`app`] drawn at `pixels_per_point` (a phone's screen is 3×).
fn app_at(tag: &str, files: &[(&str, u8)], exec: SyncExec, size: [f32; 2], pixels_per_point: f32) -> (Headless, PathBuf) {
    let root = temp(tag);
    for (name, seed) in files {
        write_png(&root.join("in").join(name), *seed);
    }
    let mut s = Session::new().with_fs();
    s.open_library(root.join("lib"), false).unwrap();
    if !files.is_empty() {
        s.execute("library.import", &json!({"paths": [root.join("in").to_string_lossy()]})).unwrap();
    }
    let png: crate::PngEncode = Box::new(|img: &lightcraft_raster::Rgba8| {
        lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(img), &lightcraft_codecs::EncodeMeta::default()).unwrap_or_default()
    });
    let app = LightcraftApp::new(
        s,
        Services { png: Some(png), write: Some(Box::new(lightcraft_engine::export::write_file)), sync_exec: Some(exec), ..Default::default() },
    );
    let mut h = Headless::new(app, size, pixels_per_point);
    h.app.ui.view = crate::state::ViewMode::PhotoGrid;
    h.settle(SETTLE);
    (h, root)
}

fn has(h: &Headless, id: &str) -> bool {
    h.app.widgets.iter().any(|(w, _)| w == id)
}

fn rect_of(h: &Headless, id: &str) -> egui::Rect {
    h.app.widgets.iter().find(|(w, _)| w == id).map(|(_, r)| *r).unwrap_or_else(|| panic!("{id} is not on screen"))
}

fn tap(h: &mut Headless, id: &str) {
    let r = h.request("ui.clickWidget", json!({"id": id}), T);
    assert_eq!(r["ok"], true, "{id}: {r}");
    h.settle(SETTLE);
}

fn run(h: &mut Headless, command: &str, params: Value) -> Value {
    let r = h.request("engine.execute", json!({"command": command, "params": params}), T);
    assert_eq!(r["ok"], true, "{command}: {r}");
    h.settle(SETTLE);
    r["result"].clone()
}

fn frames(h: &mut Headless, n: u32) {
    for _ in 0..n {
        h.step();
    }
}

/// Sign in and let the requests run (the frame loop hands them to the fake server).
fn sign_in(h: &mut Headless) {
    run(h, "sync.signIn", json!({"server": "http://fake", "user": "ann", "password": "pw", "device": "test"}));
    for _ in 0..40 {
        frames(h, 5);
        if h.app.session.sync_state().is_some_and(|st| st.state() != "syncing" || st.conflict().is_some()) && h.app.cloud_sync.in_flight == 0 {
            break;
        }
    }
    h.settle(SETTLE);
}

fn state(h: &Headless) -> &'static str {
    h.app.session.sync_state().map_or("off", |st| st.state())
}

/// A phone's grid has the cloud beside Select and "…": it opens what syncing is doing, and Pause
/// and Sync Now work from there.
#[test]
fn the_grid_has_a_cloud_that_opens_what_syncing_is_doing() {
    let activity = proto::Activity {
        scan: Some(proto::Scan {
            scanning: true,
            phase: "reading".into(),
            done: 120,
            todo: 4000,
            files: 4120,
            eta_secs: Some(300),
            ..Default::default()
        }),
        previews: proto::Progress { done: 3, total: 40, eta_secs: Some(90) },
        search: Some(proto::Progress { done: 10, total: 3000, eta_secs: None }),
        ..Default::default()
    };
    let (mut h, root) = app("popover", &[], exec(2, activity), PHONE);
    assert!(h.app.compact);
    assert!(has(&h, "icon:cloud"), "the grid bar has the cloud, signed in or not");
    // a library that never synced says how to start
    tap(&mut h, "icon:cloud");
    assert!(has(&h, "actions:syncStatus"), "the popover is open");
    assert!(has(&h, "button:syncStatusPrimary"), "Set Up Sync…");
    assert!(!has(&h, "button:syncPause"), "nothing to pause yet");
    tap(&mut h, "button:syncStatusPrimary");
    assert!(matches!(h.app.ui.dialog, Some(Dialog::Settings { ref tab }) if tab == "sync"), "Settings ▸ Sync: {:?}", h.app.ui.dialog);
    tap(&mut h, "button:sheetOk");

    // signed in (to a server with no photos of its own to choose over): it shows the server's work
    sign_in(&mut h);
    assert_eq!(state(&h), "idle");
    tap(&mut h, "icon:cloud");
    assert!(has(&h, "button:syncPause") && has(&h, "button:syncNow") && has(&h, "button:syncSettings"));
    frames(&mut h, 30);
    let a = h.app.session.sync_state().unwrap().activity().cloned().expect("asked while the popover is open");
    assert!(a.busy(), "{a:?}");
    assert_eq!(a.previews.total, 40);
    assert!(h.app.session.sync_state().unwrap().usage().is_some(), "storage too");
    let r = rect_of(&h, "actions:syncStatus");
    assert!(r.left() >= 0.0 && r.right() <= PHONE[0], "the popover fits the phone: {r:?}");
    // pause from the popover: it stays open, and the button turns into Resume
    // (every section of it is on screen, in the popover)
    let (pop, pause) = (rect_of(&h, "actions:syncStatus"), rect_of(&h, "button:syncPause"));
    assert!(pause.bottom() <= pop.bottom() + 0.5 && pause.top() >= pop.top(), "the footer is inside the popover: {pause:?} in {pop:?}");
    tap(&mut h, "button:syncPause");
    assert!(h.app.session.sync_state().unwrap().config.paused);
    assert!(has(&h, "actions:syncStatus"), "still open");
    tap(&mut h, "button:syncPause");
    assert!(!h.app.session.sync_state().unwrap().config.paused);
    // the gear: Settings ▸ Sync, and the popover closes
    tap(&mut h, "button:syncSettings");
    assert!(!has(&h, "actions:syncStatus"));
    assert!(matches!(h.app.ui.dialog, Some(Dialog::Settings { ref tab }) if tab == "sync"));
    drop(h);
    let _ = std::fs::remove_dir_all(&root);
}

/// The popover also opens from the desktop's top bar, and by command.
#[test]
fn the_desktop_cloud_opens_the_same_popover() {
    let (mut h, root) = app("desktop", &[], exec(0, proto::Activity::default()), [1280.0, 800.0]);
    assert!(!h.app.compact);
    tap(&mut h, "icon:cloud");
    assert!(has(&h, "actions:syncStatus"));
    let r = rect_of(&h, "actions:syncStatus");
    assert!(r.right() <= 1280.0 && r.width() <= 400.0, "{r:?}");
    // a tap outside closes it; the command opens it again
    h.request("ui.click", json!({"x": 300.0, "y": 600.0}), T);
    h.settle(SETTLE);
    frames(&mut h, 30);
    assert!(!has(&h, "actions:syncStatus"));
    run(&mut h, "view.syncStatus", json!({}));
    frames(&mut h, 30);
    assert!(has(&h, "actions:syncStatus"));
    drop(h);
    let _ = std::fs::remove_dir_all(&root);
}

/// Both libraries have photos: the sign-in asks what to do, the choices are on screen and fit a phone,
/// putting it off keeps asking from the cloud, and every button does what it says.
#[test]
fn two_libraries_with_photos_ask_what_to_do_on_a_phone() {
    let (mut h, root) = app("choice", &[("a.png", 1), ("b.png", 2)], exec(3, proto::Activity::default()), PHONE);
    assert_eq!(h.app.session.catalog.len(), 2);
    sign_in(&mut h);
    assert_eq!(state(&h), "conflict");
    assert_eq!(h.app.ui.dialog, Some(Dialog::SyncChoice), "asked at once");
    for id in ["button:syncChoiceUpload", "button:syncChoiceUseServer", "button:syncChoiceCancel", "button:sheetCancel"] {
        assert!(has(&h, id), "{id}");
        let r = rect_of(&h, id);
        assert!(r.left() >= 0.0 && r.right() <= PHONE[0], "{id} fits the phone: {r:?}");
    }
    assert!(!has(&h, "button:sheetOk"), "the choices are in the page");
    // put off: still waiting, nothing synced, the cloud says so and offers the choice again
    tap(&mut h, "button:sheetCancel");
    assert_eq!(h.app.ui.dialog, None);
    assert_eq!(state(&h), "conflict");
    assert_eq!(h.app.session.catalog.len(), 2);
    frames(&mut h, 30);
    assert_eq!(h.app.ui.dialog, None, "asked once, not every frame");
    tap(&mut h, "icon:cloud");
    assert!(has(&h, "button:syncStatusPrimary"), "Choose What to Do…");
    tap(&mut h, "button:syncStatusPrimary");
    assert_eq!(h.app.ui.dialog, Some(Dialog::SyncChoice));
    // Settings ▸ Sync offers it too
    tap(&mut h, "button:sheetCancel");
    run(&mut h, "app.settings", json!({"tab": "sync"}));
    assert!(has(&h, "button:syncChoose") && has(&h, "button:syncSignOut") && !has(&h, "button:syncNow"));
    tap(&mut h, "button:sheetOk");
    // use the server's: this library becomes its three photos
    run(&mut h, "dialog.syncChoice", json!({}));
    tap(&mut h, "button:syncChoiceUseServer");
    assert_eq!(h.app.ui.dialog, None, "choosing closes it");
    assert_eq!(h.app.session.catalog.len(), 3);
    assert!(h.app.session.catalog.photos().all(|p| p.file_name.starts_with("server-")));
    assert_ne!(state(&h), "conflict");
    drop(h);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn choosing_to_add_this_library_from_the_desktop_dialog_keeps_everyones_photos() {
    let (mut h, root) = app("choice-desktop", &[("a.png", 1), ("b.png", 2)], exec(3, proto::Activity::default()), [1280.0, 800.0]);
    sign_in(&mut h);
    assert_eq!(h.app.ui.dialog, Some(Dialog::SyncChoice));
    assert!(has(&h, "button:syncChoiceUpload") && has(&h, "button:dialogCancel"));
    tap(&mut h, "button:syncChoiceUpload");
    assert_eq!(h.app.ui.dialog, None);
    assert_eq!(h.app.session.catalog.len(), 5, "the server's three and these two");
    let mut names: Vec<String> = h.app.session.catalog.photos().map(|p| p.file_name.clone()).collect();
    names.sort();
    assert_eq!(names, ["a.png", "b.png", "server-1.jpg", "server-2.jpg", "server-3.jpg"]);
    drop(h);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn signing_out_from_the_choice_leaves_the_library_alone() {
    let (mut h, root) = app("choice-cancel", &[("a.png", 1)], exec(2, proto::Activity::default()), PHONE);
    sign_in(&mut h);
    assert_eq!(h.app.ui.dialog, Some(Dialog::SyncChoice));
    tap(&mut h, "button:syncChoiceCancel");
    assert_eq!(h.app.ui.dialog, None);
    assert_eq!(state(&h), "signedOut");
    assert_eq!(h.app.session.catalog.len(), 1);
    drop(h);
    let _ = std::fs::remove_dir_all(&root);
}

/// Settings ▸ Sync on a phone: a status as long as a sentence wraps under its label, inside the
/// screen, instead of being cut off with "…".
#[test]
fn the_status_on_a_phone_wraps_instead_of_being_cut_off() {
    let (mut h, root) = app("wrap", &[("a.png", 1)], exec(2, proto::Activity::default()), PHONE);
    sign_in(&mut h);
    assert_eq!(state(&h), "conflict");
    tap(&mut h, "button:sheetCancel");
    run(&mut h, "app.settings", json!({"tab": "sync"}));
    let status = rect_of(&h, "label:syncStatus");
    assert!(status.height() > 30.0, "two lines or more: {status:?}");
    assert!(status.left() >= 0.0 && status.right() <= PHONE[0], "inside the screen: {status:?}");
    // the same through a server address too long for its row
    let long = "https://a-rather-long-host-name.photos.example.com:8443/lightcraft";
    h.app.cloud_sync.form.server = long.into();
    tap(&mut h, "button:syncSignOut");
    assert!(has(&h, "field:syncServer"));
    drop(h);
    let _ = std::fs::remove_dir_all(&root);
}

/// While photos upload, the popover lists them: how many, how far, and which are moving now.
#[test]
fn the_popover_lists_the_uploads_in_flight() {
    let (mut h, root) = app("uploads", &[("a.png", 1), ("b.png", 2), ("c.png", 3)], exec_with(0, proto::Activity::default(), true), PHONE);
    // (nothing here `settle`s: it would wait for the requests that never come back)
    let r = h.request(
        "engine.execute",
        json!({"command": "sync.signIn", "params": {"server": "http://fake", "user": "ann", "password": "pw", "device": "test"}}),
        T,
    );
    assert_eq!(r["ok"], true, "{r}");
    for _ in 0..60 {
        frames(&mut h, 5);
        if h.app.session.sync_transfers().running.len() == 3 {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(state(&h), "syncing");
    let t = h.app.session.sync_transfers();
    assert_eq!((t.uploads, t.uploads_done, t.running.len()), (3, 0, 3), "{t:?}");
    assert!(t.running.iter().all(|r| r.upload && r.what == "original" && r.name.ends_with(".png")), "{t:?}");
    tap_no_settle(&mut h, "icon:cloud");
    frames(&mut h, 20);
    assert!(has(&h, "actions:syncStatus") && has(&h, "button:syncPause"));
    drop(h);
    let _ = std::fs::remove_dir_all(&root);
}

fn tap_no_settle(h: &mut Headless, id: &str) {
    let r = h.request("ui.clickWidget", json!({"id": id}), T);
    assert_eq!(r["ok"], true, "{id}: {r}");
}

/// Like Lightroom's "storage full": a server whose disk is almost full says so in the popover.
#[test]
fn a_nearly_full_server_disk_is_a_warning_in_the_popover() {
    // (9 photos on the server: this fake's disk has 4 of its 500 GB free, under 1 %)
    let (mut h, root) = app("diskfull", &[], exec(9, proto::Activity::default()), PHONE);
    sign_in(&mut h);
    tap(&mut h, "icon:cloud");
    frames(&mut h, 30);
    assert!(has(&h, "banner:serverDisk"), "the warning is there");
    let r = rect_of(&h, "banner:serverDisk");
    assert!(r.left() >= 0.0 && r.right() <= PHONE[0] && r.height() > 30.0, "{r:?}");
    drop(h);
    let _ = std::fs::remove_dir_all(&root);
    // and a roomy disk has none
    let (mut h, root) = app("diskok", &[], exec(2, proto::Activity::default()), PHONE);
    sign_in(&mut h);
    tap(&mut h, "icon:cloud");
    frames(&mut h, 30);
    assert!(!has(&h, "banner:serverDisk"));
    drop(h);
    let _ = std::fs::remove_dir_all(&root);
}

/// [`exec`] for a server busy with its library folders, whose usage answer is `usage`, and that
/// leaves the changes sent to it unanswered once `hold` is set (they stay "to send").
fn exec_busy(activity: proto::Activity, usage: Value, hold: std::sync::Arc<std::sync::atomic::AtomicBool>) -> SyncExec {
    let inner = exec(0, activity);
    Box::new(move |task: Task, tx, ctx| {
        if let Task::Http { method, url, .. } = &task {
            match (*method, url.strip_prefix("http://fake").unwrap_or("")) {
                ("POST", "/api/ops") if hold.load(std::sync::atomic::Ordering::SeqCst) => return,
                ("GET", "/api/usage") => {
                    let _ = tx.send(Done { id: task.id(), status: 200, body: usage.to_string() });
                    ctx.request_repaint();
                    return;
                }
                _ => {}
            }
        }
        inner(task, tx, ctx)
    })
}

/// What a server scanning its library folders looks like: a change on its way, the scan and the
/// previews with their bars, the files it couldn't read, and its disk. Every part is inside the
/// popover, on a phone; with `LIGHTCRAFT_TEST_SHOTS` set, pictures of it (light and dark) go there.
#[test]
fn the_popover_shows_a_server_scanning_its_folders() {
    let activity = proto::Activity {
        scan: Some(proto::Scan {
            scanning: true,
            phase: "reading".into(),
            done: 26,
            todo: 182,
            files: 192_377,
            eta_secs: Some(17 * 60),
            problems: 45,
            ..Default::default()
        }),
        previews: proto::Progress { done: 1, total: 2, eta_secs: None },
        ..Default::default()
    };
    let usage = json!({
        "photos": 192_195u64,
        "original": {"files": 4000, "bytes": 103_000_000_000u64},
        "folders": {"files": 192_195u64, "bytes": 2_000_000_000_000u64},
        "disk": {"total": 28_800_000_000_000u64, "free": 10_800_000_000_000u64},
    });
    let hold = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (mut h, root) = app_at("busy", &[], exec_busy(activity, usage, hold.clone()), PHONE, 2.0);
    sign_in(&mut h);
    assert_eq!(state(&h), "idle");
    // a change made now waits for the server
    hold.store(true, std::sync::atomic::Ordering::SeqCst);
    let r = h.request("engine.execute", json!({"command": "album.create", "params": {"name": "Trips"}}), T);
    assert_eq!(r["ok"], true, "{r}");
    frames(&mut h, 10);
    assert!(h.app.session.sync_transfers().pending > 0);
    assert_eq!(state(&h), "syncing");
    tap_no_settle(&mut h, "icon:cloud");
    frames(&mut h, 40);
    assert!(h.app.session.sync_state().unwrap().usage().is_some() && h.app.session.sync_state().unwrap().activity().is_some());
    if let Some(dir) = std::env::var_os("LIGHTCRAFT_TEST_SHOTS") {
        for mode in ["light", "dark"] {
            let r = h.request("engine.execute", json!({"command": format!("app.appearance.{mode}")}), T);
            assert_eq!(r["ok"], true, "{r}");
            frames(&mut h, 20);
            let path = std::path::Path::new(&dir).join(format!("sync-popover-{mode}.png"));
            let r = h.request("ui.screenshot", json!({"path": path.to_string_lossy(), "headless": true}), T);
            assert_eq!(r["ok"], true, "{r}");
        }
    }
    let pop = rect_of(&h, "actions:syncStatus");
    assert!(pop.left() >= 0.0 && pop.right() <= PHONE[0] && pop.bottom() <= PHONE[1], "{pop:?}");
    for id in ["button:syncPause", "button:syncNow", "button:syncSettings"] {
        let r = rect_of(&h, id);
        assert!(pop.contains_rect(r), "{id} is inside the popover: {r:?} in {pop:?}");
    }
    drop(h);
    let _ = std::fs::remove_dir_all(&root);
}

/// A server with everything to do, on a small phone: the sections scroll between the status and the
/// buttons, which stay in reach (they used to scroll away under the sections).
#[test]
fn the_buttons_stay_in_reach_when_the_server_has_a_lot_to_do() {
    let busy = proto::Progress { done: 10, total: 3000, eta_secs: Some(4000) };
    let activity = proto::Activity {
        scan: Some(proto::Scan {
            scanning: true,
            phase: "reading".into(),
            done: 26,
            todo: 182,
            eta_secs: Some(60),
            problems: 3,
            ..Default::default()
        }),
        previews: busy,
        search: Some(busy),
        text: Some(busy),
        faces: Some(busy),
    };
    let usage = json!({"photos": 5, "original": {"files": 3, "bytes": 3_000_000_000u64}, "folders": {"files": 900, "bytes": 9_000_000_000u64},
        "disk": {"total": 500_000_000_000u64, "free": 200_000_000_000u64}});
    let small = [375.0, 600.0];
    let (mut h, root) = app("small", &[], exec_busy(activity, usage, Default::default()), small);
    sign_in(&mut h);
    tap(&mut h, "icon:cloud");
    frames(&mut h, 40);
    let pop = rect_of(&h, "actions:syncStatus");
    assert!(pop.bottom() <= small[1] && pop.height() <= small[1] * 0.8 + 20.0, "{pop:?}");
    let server = rect_of(&h, "section:syncServer");
    assert!(server.height() > 300.0, "five rows of work and the unreadable files: {server:?}");
    for id in ["button:syncPause", "button:syncNow", "button:syncSettings"] {
        let r = rect_of(&h, id);
        assert!(pop.contains_rect(r), "{id} is inside the popover: {r:?} in {pop:?}");
    }
    let storage = rect_of(&h, "section:syncStorage");
    assert!(storage.top() > pop.bottom(), "storage is further down, to scroll to: {storage:?} in {pop:?}");
    drop(h);
    let _ = std::fs::remove_dir_all(&root);
}

/// A tap outside closes the popover; opened again, it is on top of its dim, not under it: drawn
/// clear, and its buttons work (Pause used to close it instead, and it looked greyed).
#[test]
fn the_popover_opens_on_top_after_a_tap_outside_closed_it() {
    let (mut h, root) = app("reopen", &[], exec(2, proto::Activity::default()), PHONE);
    sign_in(&mut h);
    for _ in 0..2 {
        tap(&mut h, "icon:cloud");
        frames(&mut h, 30);
        assert!(has(&h, "actions:syncStatus"));
        let pause = rect_of(&h, "button:syncPause");
        let popover = egui::LayerId::new(egui::Order::Foreground, egui::Id::new(("lc-sheet", crate::panels::sync_status::ID)));
        assert_eq!(h.view.ctx.layer_id_at(pause.center()), Some(popover), "the popover is on top");
        h.request("ui.click", json!({"x": 200.0, "y": 800.0}), T);
        h.settle(SETTLE);
        frames(&mut h, 30);
        assert!(!has(&h, "actions:syncStatus"), "a tap outside closes it");
    }
    tap(&mut h, "icon:cloud");
    frames(&mut h, 30);
    tap(&mut h, "button:syncPause");
    assert!(h.app.session.sync_state().unwrap().config.paused, "Pause pauses");
    assert!(has(&h, "actions:syncStatus"), "and the popover stays open");
    drop(h);
    let _ = std::fs::remove_dir_all(&root);
}

/// An error as long as a server's whole error page scrolls with the sections: the buttons stay on
/// screen, in the popover.
#[test]
fn a_long_error_scrolls_and_the_buttons_stay() {
    let fail = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let failing = fail.clone();
    let inner = exec(2, proto::Activity::default());
    let exec: SyncExec = Box::new(move |task: Task, tx, ctx| {
        if let Task::Http { method: "POST", url, .. } = &task
            && url.ends_with("/api/ops")
            && failing.load(std::sync::atomic::Ordering::SeqCst)
        {
            let long = "upstream unavailable, try again later. ".repeat(40);
            let _ = tx.send(Done { id: task.id(), status: 502, body: json!({"error": long}).to_string() });
            ctx.request_repaint();
            return;
        }
        inner(task, tx, ctx)
    });
    let (mut h, root) = app("longerror", &[], exec, PHONE);
    sign_in(&mut h);
    fail.store(true, std::sync::atomic::Ordering::SeqCst);
    run(&mut h, "album.create", json!({"name": "Trips"}));
    frames(&mut h, 20);
    assert_eq!(state(&h), "error");
    assert!(h.app.session.sync_state().unwrap().error().unwrap().len() > 1000, "the whole message is kept");
    tap(&mut h, "icon:cloud");
    frames(&mut h, 30);
    let pop = rect_of(&h, "actions:syncStatus");
    assert!(pop.bottom() <= PHONE[1], "{pop:?}");
    for id in ["button:syncPause", "button:syncNow", "button:syncSettings"] {
        let r = rect_of(&h, id);
        assert!(pop.contains_rect(r), "{id} is inside the popover: {r:?} in {pop:?}");
    }
    drop(h);
    let _ = std::fs::remove_dir_all(&root);
}

/// Requests for a server's photo files that wait until the test answers them: a slow network.
type Held = std::sync::Arc<std::sync::Mutex<Vec<(Task, std::sync::mpsc::Sender<Done>)>>>;

/// [`exec`] for a server of `photos` photos whose files (`/api/blobs/…`) are held in `held`.
fn exec_held(photos: u64, held: Held) -> SyncExec {
    let inner = exec(photos, proto::Activity::default());
    Box::new(move |task, tx, ctx| {
        if matches!(&task, Task::Http { url, .. } if url.contains("/api/blobs/")) {
            held.lock().unwrap().push((task, tx));
        } else {
            inner(task, tx, ctx);
        }
    })
}

/// Answer what is held: a mini preview comes as the file the device asked for, anything else isn't there. How many.
fn answer_held(held: &Held, mini: &[u8]) -> usize {
    let tasks = std::mem::take(&mut *held.lock().unwrap());
    let n = tasks.len();
    for (task, tx) in tasks {
        let Task::Http { id, url, save_to, .. } = task else { continue };
        let done = match save_to {
            Some(dest) if url.contains("/api/blobs/mini/") => {
                std::fs::create_dir_all(Path::new(&dest).parent().unwrap()).unwrap();
                std::fs::write(&dest, mini).unwrap();
                Done { id, status: 200, body: String::new() }
            }
            _ => Done { id, status: 404, body: json!({"error": "not here"}).to_string() },
        };
        let _ = tx.send(done);
    }
    n
}

/// Photos on the server that this device has no preview of fill the grid with empty cells, and every one of them comes
/// down — not only the first few — however slowly the server answers: a cell that is still on screen stays asked for.
/// (The grid builds a cell's render job once, and the render that failed for want of the preview isn't asked for again,
/// so asks that weren't renewed lapsed after thirty frames and the rest of the grid stayed empty.)
#[test]
fn every_cell_on_screen_gets_its_preview_however_slow_the_server() {
    const N: usize = 40;
    let held: Held = Default::default();
    let (mut h, root) = app("slow-minis", &[], exec_held(N as u64, held.clone()), PHONE);
    // (the requests for photo files wait in `held`: nothing here settles)
    let r = h.request(
        "engine.execute",
        json!({"command": "sync.signIn", "params": {"server": "http://fake", "user": "ann", "password": "pw", "device": "test"}}),
        T,
    );
    assert_eq!(r["ok"], true, "{r}");
    for _ in 0..80 {
        frames(&mut h, 5);
        if h.app.session.catalog.len() == N {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(h.app.session.catalog.len(), N);
    write_png(&root.join("mini.png"), 7);
    let (_, mini) =
        lightcraft_engine::smart::encode_pair(&std::fs::read(root.join("mini.png")).unwrap(), lightcraft_engine::cloud_sync::MINI_EDGE).unwrap();
    // the server answers four requests at a time, with forty frames of the grid in between
    let (mut answered, mut idle) = (0, 0);
    while idle < 3 {
        frames(&mut h, 40);
        let n = answer_held(&held, &mini);
        answered += n;
        idle = if n == 0 { idle + 1 } else { 0 };
        frames(&mut h, 5);
    }
    // the cells the grid draws are the screen and a screen either side of it: more than the four a time the server answers
    let drawn: Vec<PhotoId> =
        h.app.widgets.iter().filter_map(|(w, _)| w.strip_prefix("thumb:")).filter_map(|n| n.parse().ok()).map(PhotoId).collect();
    assert!(drawn.len() > 8, "the grid draws a screenful: {drawn:?}");
    let tier = |h: &Headless, id: PhotoId| h.app.session.catalog.photo(id).and_then(|p| h.app.session.synced_tier(p));
    let missing: Vec<PhotoId> = drawn.iter().copied().filter(|id| tier(&h, *id) != Some(1)).collect();
    assert!(missing.is_empty(), "{answered} previews came down, but not those of {} of the {} cells drawn: {missing:?}", missing.len(), drawn.len());
    // (and nothing beyond the grid's reach is downloaded)
    let beyond = h.app.session.catalog.photos().filter(|p| !drawn.contains(&p.id)).filter(|p| tier(&h, p.id) != Some(0)).count();
    assert_eq!(beyond, 0, "only the cells in reach download");
    // and the cells show them
    let shown = h.step_until(Duration::from_secs(20), |h| drawn.iter().all(|id| h.app.renderer.thumb(*id).is_some()));
    let why: Vec<String> = drawn
        .iter()
        .filter(|id| h.app.renderer.thumb(**id).is_none())
        .take(3)
        .map(|id| format!("{id:?}: {:?} (tier {:?})", h.app.renderer.failure(crate::render::Slot::Thumb(*id)), tier(&h, *id)))
        .collect();
    assert!(shown, "the cells draw the previews: {why:?}");
    drop(h);
    let _ = std::fs::remove_dir_all(&root);
}
