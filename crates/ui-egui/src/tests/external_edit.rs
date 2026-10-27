//! Photo ▸ Edit In from the app: the Edit Photo dialog for image files (edit a copy with the
//! edits, a plain copy or the original), and what the editor saves coming back when the window is
//! focused again — after a restart too, and when the editor saved under another extension.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lightcraft_catalog::PhotoId;
use serde_json::json;

use crate::headless::Headless;
use crate::state::{Dialog, EditInMode};
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);

/// What the host was asked to open: (file, editor).
type Opened = Arc<Mutex<Vec<(String, String)>>>;

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-ui-extedit-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A flat 16-bit PSD of `w` × `h`, every sample `v`.
fn write_psd(path: &Path, w: u32, h: u32, v: u16) {
    let px = vec![v; (w * h * 3) as usize];
    let img = lightcraft_codecs::EncodeImage::new(w, h, 3, lightcraft_codecs::Samples::U16(&px));
    std::fs::write(path, lightcraft_codecs::encode_psd(&img, &Default::default()).unwrap()).unwrap();
}

/// An app whose library has the file at `path`, recording what it opens in an editor.
fn app_with(path: &Path) -> (LightcraftApp, Opened, PhotoId) {
    let mut session = lightcraft_engine::Session::new().with_fs();
    let r = session.execute("library.import", &json!({"paths": [path.to_string_lossy()]})).unwrap();
    let id = PhotoId(r["imported"][0].as_u64().unwrap());
    session.execute("library.select", &json!({"ids": [id.0]})).unwrap();
    let opened: Opened = Default::default();
    let o = opened.clone();
    let services = Services {
        png: None,
        open_with: Some(Box::new(move |path: &str, app: &str| {
            o.lock().unwrap().push((path.to_string(), app.to_string()));
            Ok(())
        })),
        ..Default::default()
    };
    (LightcraftApp::new(session, services), opened, id)
}

fn exec(h: &mut Headless, command: &str, params: serde_json::Value) -> serde_json::Value {
    let r = h.request("engine.execute", json!({"command": command, "params": params}), T);
    assert_eq!(r["ok"], true, "{command}: {r}");
    r["result"].clone()
}

/// The window is focused again (back from the editor).
fn refocus(h: &mut Headless) {
    h.app.ui.was_focused = false;
    h.step();
}

fn toast(h: &Headless) -> String {
    h.app.ui.toast.as_ref().map(|t| t.0.clone()).unwrap_or_default()
}

/// Edit In on a PSD asks how (no file made yet); Edit Original, picked in the dialog, opens the
/// file itself, adds nothing, and is remembered for the next time.
#[test]
fn image_files_ask_how_to_edit_and_edit_original_opens_the_file() {
    let dir = temp_dir("dialog");
    let src = dir.join("pier.psd");
    write_psd(&src, 40, 24, 9000);
    let (app, opened, id) = app_with(&src);
    let mut h = Headless::new(app, [1200.0, 800.0], 1.0);
    h.step();
    let r = exec(&mut h, "photo.editInExternal", json!({}));
    assert_eq!(r["dialog"], "editIn", "{r}");
    assert_eq!(h.app.ui.dialog, Some(Dialog::EditIn { photocraft: false, mode: EditInMode::Render }));
    assert!(opened.lock().unwrap().is_empty() && h.app.session.catalog.len() == 1, "nothing made before the choice");
    h.step();
    for widget in ["radio:editIn-original", "button:dialogOk"] {
        let r = h.request("ui.clickWidget", json!({"id": widget}), T);
        assert_eq!(r["ok"], true, "{widget}: {r}");
        h.step();
    }
    assert!(h.app.ui.dialog.is_none());
    let calls = opened.lock().unwrap().clone();
    assert_eq!(calls, [(src.to_string_lossy().to_string(), String::new())], "the original, in the system's editor");
    assert_eq!(h.app.session.catalog.len(), 1, "no copy");
    assert_eq!(h.app.session.active(), Some(id));
    assert_eq!(h.app.ui.external_edits.iter().map(|t| t.path.as_str()).collect::<Vec<_>>(), [&*src.to_string_lossy()], "watched");
    assert_eq!(h.app.ui.settings.edit_in_mode, EditInMode::Original, "remembered");
    exec(&mut h, "photo.editInExternal", json!({}));
    assert_eq!(h.app.ui.dialog, Some(Dialog::EditIn { photocraft: false, mode: EditInMode::Original }));
    // Edit a Copy, given as a param (the control channel, MCP): no dialog
    h.app.ui.dialog = None;
    let r = exec(&mut h, "photo.editInExternal", json!({"mode": "copy"}));
    assert!(r["path"].as_str().unwrap().ends_with("pier-Edit.psd") && h.app.ui.dialog.is_none(), "{r}");
    assert_eq!(h.app.ui.external_edits.len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The watch list is saved with the UI state: a save the editor made while LightCraft was closed
/// comes back at the next launch, and later saves when the window is focused again.
#[test]
fn saves_come_back_after_a_restart() {
    let dir = temp_dir("restart");
    let src = dir.join("pier.psd");
    write_psd(&src, 40, 24, 9000);
    let (app, _, _) = app_with(&src);
    let mut h = Headless::new(app, [1200.0, 800.0], 1.0);
    h.step();
    exec(&mut h, "photo.editInExternal", json!({"mode": "original"}));
    let saved = serde_json::to_string(&h.app.ui).unwrap();
    drop(h);
    // LightCraft starts again (the library as it was) with what ui.json kept…
    let (mut app, _, id) = app_with(&src);
    app.ui = serde_json::from_str::<crate::UiState>(&saved).unwrap().sanitized();
    // …after the editor saved while it was closed: read on the first frame
    write_psd(&src, 30, 20, 60000);
    let mut h = Headless::new(app, [1200.0, 800.0], 1.0);
    h.step();
    let ph = h.app.session.catalog.photo(id).unwrap();
    assert_eq!((ph.width, ph.height), (30, 20), "read at launch");
    assert_eq!(h.app.ui.external_edits.len(), 1, "still watched after the restart");
    // the editor saves again while LightCraft is in the background
    write_psd(&src, 24, 16, 30000);
    refocus(&mut h);
    let ph = h.app.session.catalog.photo(id).unwrap();
    assert_eq!((ph.width, ph.height), (24, 16), "read again when focused");
    assert!(toast(&h).contains("Updated the edits"), "{}", toast(&h));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The editor saved the TIFF copy it was given as a PSD (Save As): back in LightCraft, the PSD is
/// on top of the stack with the copy and the original, and watched too.
#[test]
fn a_save_as_in_the_editor_joins_the_stack() {
    let dir = temp_dir("saveas");
    let src = dir.join("beach.psd");
    write_psd(&src, 40, 24, 9000);
    let (app, opened, orig) = app_with(&src);
    let mut h = Headless::new(app, [1200.0, 800.0], 1.0);
    h.step();
    let r = exec(&mut h, "photo.editInExternal", json!({"mode": "render"}));
    let copy = PhotoId(r["id"].as_u64().unwrap());
    assert!(opened.lock().unwrap()[0].0.ends_with("beach-Edit.tif"));
    let saved = dir.join("beach-Edit.psd");
    write_psd(&saved, 40, 24, 30000);
    refocus(&mut h);
    let new = h.app.session.catalog.photos().find(|p| p.file_name == "beach-Edit.psd").map(|p| p.id).expect("added");
    let st = h.app.session.catalog.stack_of(new).expect("stacked");
    assert_eq!(st.top(), new);
    assert!(st.photos.contains(&copy) && st.photos.contains(&orig));
    assert_eq!(toast(&h), "Added beach-Edit.psd, saved in the external editor, to its stack");
    assert!(h.app.ui.external_edits.iter().any(|t| t.path == saved.to_string_lossy()), "the PSD is watched too");
    // added once
    refocus(&mut h);
    assert_eq!(h.app.session.catalog.photos().filter(|p| p.file_name == "beach-Edit.psd").count(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}
