//! File ▸ Import Profiles & Presets…: `.cube` LUTs become profiles, everything else presets, and
//! one toast says what happened to both (issue #942).

use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

fn app() -> Headless {
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1200.0, 800.0], 1.0);
    h.settle(SETTLE);
    h
}

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("lc-ui-preset-import-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn import(h: &mut Headless, paths: &[&std::path::Path]) -> serde_json::Value {
    let paths: Vec<String> = paths.iter().map(|p| p.to_string_lossy().to_string()).collect();
    let r = h.request("engine.execute", json!({"command": "file.importPresets", "params": {"paths": paths}}), T);
    h.settle(SETTLE);
    r
}

fn toast(h: &Headless) -> String {
    h.app.ui.toast.clone().map(|t| t.0).unwrap_or_default()
}

/// A 2×2×2 identity LUT.
fn identity_cube() -> String {
    let mut s = String::from("TITLE \"Identity\"\nLUT_3D_SIZE 2\n");
    for b in 0..2 {
        for g in 0..2 {
            for r in 0..2 {
                s += &format!("{r} {g} {b}\n");
            }
        }
    }
    s
}

/// Importing only `.cube` files reports the profiles, not "No new presets".
#[test]
fn a_cube_only_import_reports_the_profiles() {
    let dir = temp_dir("ok");
    let cube = dir.join("Warm.cube");
    std::fs::write(&cube, identity_cube()).unwrap();
    let mut h = app();
    let r = import(&mut h, &[&cube]);
    assert_eq!(r["ok"], true, "{r}");
    let t = toast(&h);
    assert!(t.contains("1 profile"), "{t}");
    assert!(!t.contains("preset"), "{t}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A `.cube` that can't be imported says which file and why.
#[test]
fn a_broken_cube_says_why() {
    let dir = temp_dir("bad");
    let bad = dir.join("Broken.cube");
    std::fs::write(&bad, "LUT_3D_SIZE 2\n0 0 0\n").unwrap();
    let mut h = app();
    let r = import(&mut h, &[&bad]);
    assert_eq!(r["ok"], true, "{r}");
    let t = toast(&h);
    assert!(t.contains("Broken"), "{t}");
    assert!(!t.contains("No new presets"), "{t}");
    let reason = r["result"]["profiles"]["failed"][0][1].as_str().unwrap_or_default().to_string();
    assert!(!reason.is_empty() && t.contains(&reason), "{t} / {r}");
    // with a good one alongside, both outcomes are in the one toast
    let good = dir.join("Good.cube");
    std::fs::write(&good, identity_cube()).unwrap();
    import(&mut h, &[&good, &bad]);
    let t = toast(&h);
    assert!(t.contains("1 profile") && t.contains("Broken"), "{t}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The answers a host picker was asked for, each with the sender that answers it.
type Asked = std::sync::Arc<std::sync::Mutex<Vec<(crate::pick::PickRequest, std::sync::mpsc::Sender<crate::pick::HostPicked>)>>>;

/// A windowless app whose host has only its own file picker, as the web and iOS apps do: each
/// request is recorded with the sender that answers it.
fn app_with_host_picker() -> (Headless, Asked) {
    let mut app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), Services { png: None, ..Default::default() });
    let asked: Asked = Default::default();
    let a = asked.clone();
    app.services.host_files = Some(Box::new(move |req| {
        let (tx, rx) = std::sync::mpsc::channel();
        a.lock().unwrap().push((req, tx));
        Ok(rx)
    }));
    let mut h = Headless::new(app, [1200.0, 800.0], 1.0);
    h.settle(SETTLE);
    (h, asked)
}

/// An XMP preset named `name` (hand-written for these tests).
fn xmp(name: &str, exposure: &str) -> Vec<u8> {
    format!(
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
      <rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:PresetType="Normal"
        crs:Name="{name}" crs:Exposure2012="{exposure}"/></rdf:RDF></x:xmpmeta>"#
    )
    .into_bytes()
}

fn has_preset(h: &Headless, name: &str) -> bool {
    h.app.session.presets.iter().any(|p| p.name == name && !p.builtin)
}

/// The web and iOS: Import Presets… shows the host's own picker (the command returns at once), and
/// what it hands over is imported: files read into memory (the browser's chooser has no paths) or
/// copies (iOS). A LUT read into memory can't become a profile and says so, as do files the picker
/// couldn't hand over.
#[test]
fn the_hosts_own_picker_imports_what_it_hands_over() {
    use crate::pick::{HostPicked, PickKind, PickedFile};
    let (mut h, asked) = app_with_host_picker();
    let r = h.request("engine.execute", json!({"command": "file.importPresets", "params": {}}), T);
    assert_eq!(r["ok"], true, "{r}");
    {
        let asked = asked.lock().unwrap();
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].0.kind, PickKind::Files);
        assert_eq!(asked[0].0.filter.as_ref().map(|f| f.1), Some(crate::pick::PRESET_EXTENSIONS));
    }
    // asked again (a browser says nothing when its chooser is cancelled): shown again, and only the
    // last one counts
    h.app.run("file.importPresets", json!({})).unwrap();
    assert_eq!((asked.lock().unwrap().len(), h.app.pending_picks.len()), (2, 1));
    assert!(asked.lock().unwrap()[0].1.send(HostPicked::default()).is_err(), "the first one's answer goes nowhere");
    let files = vec![
        PickedFile { name: "Soft.xmp".into(), bytes: xmp("Soft", "+0.25") },
        PickedFile { name: "Warm.cube".into(), bytes: identity_cube().into_bytes() },
    ];
    let picked = HostPicked { files, failed: vec!["Huge.zip: couldn't be read".into()], ..Default::default() };
    asked.lock().unwrap()[1].1.send(picked).unwrap();
    h.settle(SETTLE);
    assert!(h.app.pending_picks.is_empty());
    assert!(has_preset(&h, "Soft"));
    let t = toast(&h);
    assert!(t.contains("Imported 1 preset") && t.contains("Huge.zip") && t.contains("Warm.cube"), "{t}");
    assert!(h.app.session.lut_profiles.is_empty());

    // iOS hands over copies: .cube files among them become profiles as on the desktop
    let dir = temp_dir("host-paths");
    std::fs::write(dir.join("Look.xmp"), xmp("Look", "-0.5")).unwrap();
    std::fs::write(dir.join("Warm.cube"), identity_cube()).unwrap();
    h.app.run("file.importPresets", json!({})).unwrap();
    let paths = ["Look.xmp", "Warm.cube"].map(|n| dir.join(n).to_string_lossy().to_string()).to_vec();
    asked.lock().unwrap()[2].1.send(HostPicked { paths, ..Default::default() }).unwrap();
    h.settle(SETTLE);
    assert!(has_preset(&h, "Look"));
    assert_eq!(h.app.session.lut_profiles.len(), 1);
    let t = toast(&h);
    assert!(t.contains("1 profile") && t.contains("1 preset"), "{t}");

    // cancelled: nothing; nothing handed over but a reason: the reason
    h.app.ui.toast = None;
    let n = h.app.session.presets.len();
    h.app.run("file.importPresets", json!({})).unwrap();
    asked.lock().unwrap()[3].1.send(HostPicked::default()).unwrap();
    h.settle(SETTLE);
    assert_eq!((h.app.session.presets.len(), h.app.ui.toast.is_none(), h.app.pending_picks.is_empty()), (n, true, true));
    h.app.run("file.importPresets", json!({})).unwrap();
    asked.lock().unwrap()[4].1.send(HostPicked { failed: vec!["Big.zip: too large to read".into()], ..Default::default() }).unwrap();
    h.settle(SETTLE);
    assert!(toast(&h).contains("too large"), "{}", toast(&h));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The Presets panel's Import Presets… (its ⋯ menu, on phones too) opens the host's picker, and a
/// host that has no picker at all says so instead of doing nothing.
#[test]
fn the_panels_import_button_opens_the_picker_or_says_why_not() {
    let click = |h: &mut Headless, id: &str| {
        let r = h.request("ui.clickWidget", json!({"id": id}), T);
        assert_eq!(r["ok"], true, "{id}: {r}");
        h.settle(SETTLE);
    };
    let open_menu = |h: &mut Headless| {
        h.app.ui.view = crate::state::ViewMode::Detail;
        h.app.run("panel.presets", json!({})).unwrap();
        h.settle(SETTLE);
        click(h, "icon:presetMore");
    };
    let (mut h, asked) = app_with_host_picker();
    open_menu(&mut h);
    click(&mut h, "presetMenu:import");
    assert_eq!(asked.lock().unwrap().len(), 1, "the picker is up");
    let mut bare = app();
    open_menu(&mut bare);
    click(&mut bare, "presetMenu:import");
    assert!(toast(&bare).contains("no file dialog"), "{}", toast(&bare));
}

/// Files given by content work through the command itself too (scripts, agents, the web's
/// `command()`), and a host picker answer carries them as `files`.
#[test]
fn files_given_by_content_import_through_the_command() {
    let mut h = app();
    let files = lightcraft_engine::inline_files::to_param([("Crisp.xmp", xmp("Crisp", "+0.1").as_slice())]);
    let r = h.request("engine.execute", json!({"command": "file.importPresets", "params": {"files": files}}), T);
    h.settle(SETTLE);
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(r["result"]["imported"][0]["name"], "Crisp", "{r}");
    assert!(has_preset(&h, "Crisp"));
    let picked = crate::pick::HostPicked {
        paths: vec!["/a.xmp".into()],
        files: vec![crate::pick::PickedFile { name: "b.xmp".into(), bytes: b"<x/>".to_vec() }],
        failed: vec!["c: no".into()],
    };
    let p = crate::pick::with_host_answer(&json!({"group": "G"}), &picked);
    assert_eq!(p["group"], "G");
    assert_eq!(p["paths"], json!(["/a.xmp"]));
    assert_eq!(p["files"], json!([{"name": "b.xmp", "data": "PHgvPg=="}]));
    assert_eq!(p["pickFailed"], json!(["c: no"]));
    assert_eq!(crate::pick::with_host_answer(&json!({}), &Default::default()), json!({}));
}
