//! File ▸ Import Profiles & Presets…: `.cube` LUTs become profiles, everything else presets, and
//! one toast says what happened to both (issue #942). What a person picks or drops is shown in the
//! Import Presets review first (`preset_import`); a script that names its files imports at once.

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

/// The open Import Presets dialog's state.
fn review(h: &Headless) -> &crate::preset_import::Review {
    match &h.app.ui.dialog {
        Some(crate::state::Dialog::PresetImport { review }) => review,
        other => panic!("no Import Presets dialog: {other:?}"),
    }
}

/// Press the dialog's action (Import, then Show Presets).
fn confirm(h: &mut Headless) -> serde_json::Value {
    let r = h.request("ui.dialog.confirm", json!({}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    r
}

/// A zip archive of stored entries (written for these tests).
fn zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let (mut out, mut central) = (Vec::new(), Vec::new());
    for (name, data) in entries {
        let at = out.len() as u32;
        for (sig, local) in [(b"PK\x03\x04", true), (b"PK\x01\x02", false)] {
            let head = if local { &mut out } else { &mut central };
            head.extend_from_slice(sig);
            if !local {
                head.extend_from_slice(&20u16.to_le_bytes());
            }
            head.extend_from_slice(&20u16.to_le_bytes());
            head.extend_from_slice(&[0; 6]); // flags, method (stored), time
            head.extend_from_slice(&[0; 6]); // date, crc (not checked)
            head.extend_from_slice(&(data.len() as u32).to_le_bytes());
            head.extend_from_slice(&(data.len() as u32).to_le_bytes());
            head.extend_from_slice(&(name.len() as u16).to_le_bytes());
            head.extend_from_slice(&0u16.to_le_bytes());
            if local {
                head.extend_from_slice(name.as_bytes());
                head.extend_from_slice(data);
            } else {
                head.extend_from_slice(&[0; 10]);
                head.extend_from_slice(&at.to_le_bytes());
                head.extend_from_slice(name.as_bytes());
            }
        }
    }
    let start = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(b"PK\x05\x06\0\0\0\0");
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&start.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

/// A pack as a shop ships it: presets in folders, a preview picture and a guide, one broken file.
fn pack(dir: &std::path::Path) -> String {
    let (a, b, c) = (xmp("Soft", "+0.25"), xmp("Warm", "+0.5"), xmp("Cool", "-0.5"));
    let z = zip(&[
        ("Film Pack/Lightroom Classic/Presets/Soft.xmp", &a),
        ("Film Pack/Lightroom Classic/Presets/Warm.xmp", &b),
        ("Film Pack/Moody/Cool.xmp", &c),
        ("Film Pack/Moody/Broken.xmp", b"<not xml"),
        ("Film Pack/Previews/Soft.jpg", b"\xff\xd8 a picture"),
        ("Film Pack/Guide.pdf", b"%PDF"),
    ]);
    let path = dir.join("Film Pack.zip");
    std::fs::write(&path, z).unwrap();
    path.to_string_lossy().to_string()
}

fn open_review(h: &mut Headless, paths: &[&str]) -> serde_json::Value {
    let r = h.request("engine.execute", json!({"command": "file.importPresets", "params": {"paths": paths, "review": true}}), T);
    h.settle(SETTLE);
    assert_eq!(r["ok"], true, "{r}");
    r
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
    // what a person picks is shown first: nothing is added until Import
    assert!(!has_preset(&h, "Soft"));
    let r = review(&h);
    assert_eq!(r.items.iter().map(|i| i.preset.name.as_str()).collect::<Vec<_>>(), ["Soft"]);
    let problems: Vec<&str> = r.problems.iter().map(|(f, _)| f.as_str()).collect();
    assert!(problems.contains(&"Huge.zip") && problems.contains(&"Warm.cube"), "{problems:?}");
    confirm(&mut h);
    assert!(has_preset(&h, "Soft"));
    let o = review(&h).outcome.clone().expect("the dialog says how it went");
    assert_eq!((o.presets, o.profiles), (1, 0));
    assert!(o.problems.iter().any(|(f, why)| f == "Huge.zip" && why.contains("couldn't be read")), "{:?}", o.problems);
    assert!(h.app.session.lut_profiles.is_empty());
    confirm(&mut h);
    assert!(h.app.ui.dialog.is_none());

    // iOS hands over copies: .cube files among them become profiles as on the desktop
    let dir = temp_dir("host-paths");
    std::fs::write(dir.join("Look.xmp"), xmp("Look", "-0.5")).unwrap();
    std::fs::write(dir.join("Warm.cube"), identity_cube()).unwrap();
    h.app.run("file.importPresets", json!({})).unwrap();
    let paths = ["Look.xmp", "Warm.cube"].map(|n| dir.join(n).to_string_lossy().to_string()).to_vec();
    asked.lock().unwrap()[2].1.send(HostPicked { paths, ..Default::default() }).unwrap();
    h.settle(SETTLE);
    assert!(!has_preset(&h, "Look") && h.app.session.lut_profiles.is_empty());
    assert_eq!((review(&h).items.len(), review(&h).profiles.len()), (1, 1));
    confirm(&mut h);
    assert!(has_preset(&h, "Look"));
    assert_eq!(h.app.session.lut_profiles.len(), 1);
    let o = review(&h).outcome.clone().unwrap();
    assert_eq!((o.presets, o.profiles), (1, 1));
    h.app.ui.dialog = None;

    // cancelled: nothing; nothing handed over but a reason: the reason (there is nothing to review)
    h.app.ui.toast = None;
    let n = h.app.session.presets.len();
    h.app.run("file.importPresets", json!({})).unwrap();
    asked.lock().unwrap()[3].1.send(HostPicked::default()).unwrap();
    h.settle(SETTLE);
    assert_eq!((h.app.session.presets.len(), h.app.ui.toast.is_none(), h.app.pending_picks.is_empty()), (n, true, true));
    assert!(h.app.ui.dialog.is_none());
    h.app.run("file.importPresets", json!({})).unwrap();
    asked.lock().unwrap()[4].1.send(HostPicked { failed: vec!["Big.zip: too large to read".into()], ..Default::default() }).unwrap();
    h.settle(SETTLE);
    assert!(toast(&h).contains("too large"), "{}", toast(&h));
    assert!(h.app.ui.dialog.is_none());
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

/// A zip of presets is read without adding anything: the dialog lists every preset by group, with the
/// files that couldn't be read; Import adds them and the same dialog says where they went.
#[test]
fn a_zip_of_presets_is_shown_first_then_imported() {
    let dir = temp_dir("zip-review");
    let zip = pack(&dir);
    let mut h = app();
    let before = h.app.session.presets.len();
    let r = open_review(&mut h, &[&zip]);
    assert_eq!(r["result"], json!({"review": true, "presets": 3, "already": 0, "profiles": 0, "failed": 1}), "{r}");
    assert_eq!(h.app.session.presets.len(), before, "nothing is added before Import");
    let rv = review(&h);
    assert_eq!(rv.source, "Film Pack.zip");
    let listed: Vec<(&str, &str)> = rv.items.iter().map(|i| (i.preset.group.as_str(), i.preset.name.as_str())).collect();
    assert_eq!(listed, [("Film Pack", "Soft"), ("Film Pack", "Warm"), ("Moody", "Cool")]);
    assert_eq!(rv.problems.len(), 1);
    assert!(rv.problems[0].0.ends_with("Broken.xmp"), "{:?}", rv.problems);
    assert_eq!(crate::preset_import::ok_label(rv, false), "Import 3 Presets");
    assert_eq!(crate::preset_import::cancel_label(rv), "Cancel");
    // each preset is on screen, in its group
    for id in rv.items.iter().map(|i| i.preset.id.clone()) {
        assert!(h.app.widgets.iter().any(|(w, _)| *w == format!("presetImport:item:{id}")), "{id}");
    }
    assert!(h.app.widgets.iter().any(|(w, _)| w == "presetImport:group:Moody"));
    // Import
    let r = confirm(&mut h);
    assert_eq!(r["result"]["imported"], 3, "{r}");
    assert_eq!(h.app.session.presets.len(), before + 3);
    let o = review(&h).outcome.clone().expect("how it went");
    assert_eq!((o.presets, o.skipped, o.groups.clone()), (3, 0, vec![("Film Pack".to_string(), 2), ("Moody".to_string(), 1)]));
    assert_eq!(o.problems.len(), 1, "the broken file stays reported: {:?}", o.problems);
    assert!(h.app.widgets.iter().any(|(w, _)| w == "presetImport:where"));
    assert_eq!(crate::preset_import::ok_label(review(&h), false), "Show Presets");
    assert_eq!(crate::preset_import::cancel_label(review(&h)), "Done");
    // Show Presets closes the dialog and opens the panel
    h.app.ui.presets = false;
    confirm(&mut h);
    assert!(h.app.ui.dialog.is_none() && h.app.ui.presets);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Cancel adds nothing; the same files again are "already in your library".
#[test]
fn cancel_adds_nothing_and_a_second_look_says_it_is_all_there() {
    let dir = temp_dir("zip-again");
    let zip = pack(&dir);
    let mut h = app();
    let before = h.app.session.presets.len();
    open_review(&mut h, &[&zip]);
    h.request("ui.dialog.cancel", json!({}), T);
    h.settle(SETTLE);
    assert!(h.app.ui.dialog.is_none());
    assert_eq!(h.app.session.presets.len(), before);
    // imported for real (a script names its files: no review), then shown again
    h.app.run("file.importPresets", json!({"paths": [&zip]})).unwrap();
    assert_eq!(h.app.session.presets.len(), before + 3);
    open_review(&mut h, &[&zip]);
    let rv = review(&h);
    assert_eq!((rv.items.len(), rv.already), (0, 3));
    assert!(crate::preset_import::informational(rv), "nothing to add: one Close button");
    assert_eq!(crate::preset_import::ok_label(rv, false), "Close");
    // Return (or Close) just closes it
    let r = confirm(&mut h);
    assert_eq!(r["ok"], true, "{r}");
    assert!(h.app.ui.dialog.is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Files that hold no presets say why, in the review and not as a toast that goes away.
#[test]
fn files_that_are_not_presets_say_why() {
    let dir = temp_dir("not-presets");
    let (bad, photos) = (dir.join("Not a pack.zip"), dir.join("Only photos.zip"));
    std::fs::write(&bad, b"this is not a zip file").unwrap();
    std::fs::write(&photos, zip(&[("a.jpg", b"\xff\xd8"), ("readme.txt", b"hi")])).unwrap();
    let mut h = app();
    let before = h.app.session.presets.len();
    open_review(&mut h, &[&bad.to_string_lossy(), &photos.to_string_lossy()]);
    let rv = review(&h);
    assert!(rv.items.is_empty() && rv.profiles.is_empty());
    let named: Vec<&str> = rv.problems.iter().map(|(f, _)| f.rsplit('/').next().unwrap_or(f)).collect();
    assert_eq!(named, ["Not a pack.zip", "Only photos.zip"], "{:?}", rv.problems);
    assert!(rv.problems.iter().all(|(_, why)| !why.is_empty()), "{:?}", rv.problems);
    assert!(crate::preset_import::informational(rv));
    confirm(&mut h);
    assert!(h.app.ui.dialog.is_none());
    assert_eq!(h.app.session.presets.len(), before);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A zip with a preset and a LUT lists both, imports both, and is not reported as "no presets" for the LUT.
#[test]
fn a_zip_with_presets_and_luts_imports_both() {
    let dir = temp_dir("zip-luts");
    let z = zip(&[("Both/Clean.xmp", &xmp("Clean", "+0.2")), ("Both/LUTs/Teal.cube", identity_cube().as_bytes())]);
    let path = dir.join("Both.zip");
    std::fs::write(&path, z).unwrap();
    let mut h = app();
    open_review(&mut h, &[&path.to_string_lossy()]);
    let rv = review(&h);
    assert_eq!((rv.items.len(), rv.profiles.len(), rv.problems.len()), (1, 1, 0), "{rv:?}");
    assert!(h.app.session.lut_profiles.is_empty(), "the review adds no profile");
    confirm(&mut h);
    assert!(has_preset(&h, "Clean"));
    assert_eq!(h.app.session.lut_profiles.len(), 1);
    let o = review(&h).outcome.clone().unwrap();
    assert_eq!((o.presets, o.profiles, o.problems.len()), (1, 1, 0));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Scripts and agents that name files import at once, as before; asking for the review is explicit.
#[test]
fn naming_files_imports_at_once_unless_the_review_is_asked_for() {
    let dir = temp_dir("at-once");
    let zip = pack(&dir);
    let mut h = app();
    let r = h.request("engine.execute", json!({"command": "file.importPresets", "params": {"paths": [&zip]}}), T);
    // (read before frames run on: a toast goes away by the clock)
    let said = toast(&h);
    h.settle(SETTLE);
    assert_eq!(r["ok"], true, "{r}");
    assert!(h.app.ui.dialog.is_none());
    assert_eq!(r["result"]["imported"].as_array().map(Vec::len), Some(3), "{r}");
    assert!(said.contains("Imported 3 presets"), "{said}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// On a phone the review is a page with Import in its bar; afterwards the strip shows the group the
/// presets went to, its chip in view.
#[test]
fn on_a_phone_the_review_is_a_page_and_the_strip_shows_the_new_group() {
    let dir = temp_dir("phone");
    let zip = pack(&dir);
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [390.0, 844.0], 1.0);
    h.settle(SETTLE);
    h.request("engine.execute", json!({"command": "library.select", "params": {"ids": [1]}}), T);
    h.app.ui.view = crate::state::ViewMode::Detail;
    h.app.run("panel.presets", json!({})).unwrap();
    h.settle(SETTLE);
    assert!(h.app.compact);
    open_review(&mut h, &[&zip]);
    let has = |h: &Headless, id: &str| h.app.widgets.iter().find(|(w, _)| w == id).map(|(_, r)| *r);
    let page = has(&h, "sheet:dialog").expect("a page");
    assert!(page.width() >= 389.0, "{page:?}");
    let import = has(&h, "button:sheetOk").expect("Import in the bar");
    assert!(import.right() <= 390.0 && import.width() < 130.0, "{import:?}");
    assert_eq!(crate::preset_import::ok_label(review(&h), true), "Import");
    for id in review(&h).items.iter().map(|i| i.preset.id.clone()).take(1) {
        let row = has(&h, &format!("presetImport:item:{id}")).expect("a row");
        assert!(row.left() >= 0.0 && row.right() <= 390.0, "{row:?}");
    }
    let r = h.request("ui.clickWidget", json!({"id": "button:sheetOk"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert!(review(&h).outcome.is_some());
    // the bar's button is short enough not to run under the title
    let show = has(&h, "button:sheetOk").expect("Show in the bar");
    assert!(show.width() < 90.0, "{show:?}");
    let r = h.request("ui.clickWidget", json!({"id": "button:sheetOk"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    h.settle(SETTLE);
    assert!(h.app.ui.dialog.is_none());
    // the strip shows the first group the presets went to, with its chip on screen
    let chip = has(&h, "button:presetGroup:Film Pack").expect("the chip of the new group");
    assert!(chip.left() >= 0.0 && chip.right() <= 390.0, "{chip:?}");
    assert!(has(&h, "button:presetGroup:Moody").is_some());
    let tiles = h.app.widgets.iter().filter(|(w, _)| w.starts_with("preset:user.imp.")).count();
    assert_eq!(tiles, 2, "the strip is on Film Pack: its two presets");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A zip of LUTs alone holds no presets and that is no failure; a zip of photos next to it still is.
#[test]
fn a_zip_of_luts_is_not_a_failure_but_a_zip_of_photos_is() {
    let dir = temp_dir("luts-and-photos");
    let luts = dir.join("LUTs.zip");
    std::fs::write(&luts, zip(&[("LUTs/Teal.cube", identity_cube().as_bytes())])).unwrap();
    let photos = dir.join("Photos.zip");
    std::fs::write(&photos, zip(&[("a.jpg", b"\xff\xd8"), ("readme.txt", b"hi")])).unwrap();
    let mut h = app();
    open_review(&mut h, &[&luts.to_string_lossy(), &photos.to_string_lossy()]);
    let rv = review(&h);
    assert_eq!((rv.items.len(), rv.profiles.len()), (0, 1), "{rv:?}");
    let named: Vec<&str> = rv.problems.iter().map(|(f, _)| f.rsplit('/').next().unwrap_or(f)).collect();
    assert_eq!(named, ["Photos.zip"], "{:?}", rv.problems);
    confirm(&mut h);
    assert_eq!(h.app.session.lut_profiles.len(), 1);
    let o = review(&h).outcome.clone().unwrap();
    let named: Vec<&str> = o.problems.iter().map(|(f, _)| f.rsplit('/').next().unwrap_or(f)).collect();
    assert_eq!((o.presets, o.profiles, named), (0, 1, vec!["Photos.zip"]), "{o:?}");
    let _ = std::fs::remove_dir_all(&dir);
}
