//! Photos dragged in from outside the window: into the album under them or the one the grid shows,
//! and a tile in the grid for each until it is in the catalog.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lightcraft_catalog::AlbumId;
use serde_json::json;

use crate::file_drop::State;
use crate::headless::Headless;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(60);

/// `n` stand-in JPEGs in a folder, and an app (empty library) whose file probe takes `ms` each.
fn setup(tag: &str, n: usize, ms: u64) -> (Headless, Vec<String>, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("lc-file-drop-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let files: Vec<String> = (0..n)
        .map(|i| {
            let path = dir.join(format!("IMG_{i:03}.jpg"));
            std::fs::write(&path, format!("not really a jpeg {i}")).unwrap();
            path.to_string_lossy().to_string()
        })
        .collect();
    let mut s = lightcraft_engine::Session::new();
    s.media.file_probe = Some(Arc::new(move |p: &str| {
        std::thread::sleep(Duration::from_millis(ms));
        Ok(lightcraft_engine::media::ProbeInfo {
            width: 60,
            height: 40,
            format: "JPEG".into(),
            content_hash: Some(p.to_string()),
            ..Default::default()
        })
    }));
    let mut app = LightcraftApp::new(s, Services { png: None, ..Default::default() });
    app.ui.view = crate::state::ViewMode::PhotoGrid;
    app.ui.left_panel = true;
    let mut h = Headless::new(app, [1300.0, 900.0], 1.0);
    h.settle(T);
    (h, files, dir)
}

fn album(h: &mut Headless, name: &str) -> u64 {
    h.app.session.execute("album.create", &json!({"name": name})).unwrap()["id"].as_u64().unwrap()
}

fn show(h: &mut Headless, id: u64) {
    h.app.run("library.source", json!({"kind": "album", "id": id})).unwrap();
    h.settle(T);
}

fn members(h: &Headless, id: u64) -> usize {
    h.app.session.catalog.album_photos(AlbumId(id)).len()
}

fn drop_files(h: &mut Headless, files: &[String]) {
    h.dropped_files = files.iter().map(Into::into).collect();
    h.step();
}

fn finish(h: &mut Headless) {
    assert!(h.step_until(T, |h| h.app.import.is_none()), "the import finished");
}

fn has(h: &Headless, id: &str) -> bool {
    h.app.widgets.iter().any(|(w, _)| w == id)
}

fn row(h: &Headless, album: u64) -> egui::Rect {
    let id = format!("source:album:{album}");
    h.app.widgets.iter().find(|(w, _)| *w == id).map(|(_, r)| *r).unwrap_or_else(|| panic!("no widget {id}"))
}

/// Make the host report the pointer wherever the test puts it.
fn pointer(h: &mut Headless) -> Arc<Mutex<Option<egui::Pos2>>> {
    let at = Arc::new(Mutex::new(None));
    let read = at.clone();
    h.app.services.file_drag_pointer = Some(Box::new(move || *read.lock().unwrap()));
    at
}

#[test]
fn files_dropped_on_the_grid_go_to_the_album_it_shows() {
    let (mut h, files, dir) = setup("grid", 3, 1);
    let trip = album(&mut h, "Trip");
    let other = album(&mut h, "Other");
    show(&mut h, trip);
    drop_files(&mut h, &files);
    finish(&mut h);
    assert_eq!(h.app.session.catalog.len(), 3);
    assert_eq!(members(&h, trip), 3);
    assert_eq!(members(&h, other), 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn files_dropped_while_the_library_is_shown_are_in_no_album() {
    let (mut h, files, dir) = setup("library", 2, 1);
    let trip = album(&mut h, "Trip");
    drop_files(&mut h, &files);
    finish(&mut h);
    assert_eq!(h.app.session.catalog.len(), 2);
    assert_eq!(members(&h, trip), 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_album_row_under_the_files_lights_up_and_takes_the_drop() {
    let (mut h, files, dir) = setup("row", 2, 1);
    let (a, b) = (album(&mut h, "Alpha"), album(&mut h, "Beta"));
    // the grid shows Alpha; the drop goes to Beta, which the files are over
    show(&mut h, a);
    let at = pointer(&mut h);
    h.hovering_files = vec![files[0].clone().into()];
    *at.lock().unwrap() = Some(row(&h, b).center());
    h.step();
    h.step();
    assert_eq!(crate::file_drop::over(&h.view.ctx), Some(b), "Beta lights up");
    // the pointer moves to another row, then off the rows
    *at.lock().unwrap() = Some(row(&h, a).center());
    h.step();
    assert_eq!(crate::file_drop::over(&h.view.ctx), Some(a));
    *at.lock().unwrap() = None;
    h.step();
    assert_eq!(crate::file_drop::over(&h.view.ctx), None);
    // …and back to Beta, where they are let go
    *at.lock().unwrap() = Some(row(&h, b).center());
    h.step();
    drop_files(&mut h, &files);
    finish(&mut h);
    assert_eq!(members(&h, b), 2);
    assert_eq!(members(&h, a), 0, "not the album the grid shows");
    assert_eq!(crate::file_drop::over(&h.view.ctx), None, "forgotten once dropped");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn leaving_the_window_without_a_drop_lights_nothing() {
    let (mut h, files, dir) = setup("leave", 1, 1);
    let a = album(&mut h, "Alpha");
    h.step();
    h.step();
    let at = pointer(&mut h);
    h.hovering_files = vec![files[0].clone().into()];
    *at.lock().unwrap() = Some(row(&h, a).center());
    h.step();
    h.step();
    assert_eq!(crate::file_drop::over(&h.view.ctx), Some(a));
    h.hovering_files.clear();
    h.step();
    assert_eq!(crate::file_drop::over(&h.view.ctx), None);
    assert!(h.app.import.is_none(), "nothing was dropped");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn files_dropped_on_the_sidebar_outside_any_album_skip_the_album_shown() {
    let (mut h, files, dir) = setup("sidebar", 2, 1);
    let a = album(&mut h, "Alpha");
    show(&mut h, a);
    let at = pointer(&mut h);
    let sidebar = h.view.ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new("file-drop-sidebar"))).expect("the sidebar is drawn");
    // (below its last row)
    *at.lock().unwrap() = Some(egui::pos2(sidebar.center().x, sidebar.bottom() - 6.0));
    h.hovering_files = vec![files[0].clone().into()];
    h.step();
    h.step();
    assert_eq!(crate::file_drop::over(&h.view.ctx), None);
    drop_files(&mut h, &files);
    finish(&mut h);
    assert_eq!(h.app.session.catalog.len(), 2);
    assert_eq!(members(&h, a), 0, "only in the library");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn every_dropped_photo_has_a_tile_from_the_start_until_it_is_in_the_catalog() {
    let n = lightcraft_engine::import::batch_size() * 2 + 1;
    let (mut h, files, dir) = setup("tiles", n, 25);
    let trip = album(&mut h, "Trip");
    show(&mut h, trip);
    // an empty album: no "No photos" while the dropped ones are on their way
    assert!(h.painted_text().iter().any(|t| t == "No photos"));
    drop_files(&mut h, &files);
    let pending = h.app.import.as_ref().and_then(|t| t.pending()).expect("a drop has tiles").clone();
    assert_eq!(pending.len(), n, "a tile for each, before anything is read");
    assert!(has(&h, "pending:0"), "drawn in the frame of the drop");
    let text = h.painted_text();
    assert!(text.iter().any(|t| *t == format!("Adding photos ({n})")), "{text:?}");
    assert!(!text.iter().any(|t| t == "No photos"), "{text:?}");

    let mut adding = false;
    let mut before = n;
    let t0 = Instant::now();
    while h.app.import.is_some() && t0.elapsed() < T {
        h.step();
        let (tiles, photos) = (pending.len(), h.app.session.catalog.len());
        // a photo's tile gives way in the frame it joins the catalog
        assert_eq!(tiles + photos, n, "{tiles} tiles and {photos} photos");
        assert!(tiles <= before, "tiles only go away");
        before = tiles;
        adding |= pending.slice(0, usize::MAX).iter().any(|t| t.state == State::Adding);
    }
    assert!(adding, "the photos being read are marked");
    assert!(pending.is_empty());
    assert_eq!(members(&h, trip), n);
    h.step();
    assert!(!has(&h, "pending:0"), "no tile is left");
    assert!(!h.painted_text().iter().any(|t| t.starts_with("Adding photos")));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn tiles_show_in_the_views_the_photos_will_be_in() {
    let (mut h, files, dir) = setup("views", 40, 80);
    let (trip, other) = (album(&mut h, "Trip"), album(&mut h, "Other"));
    show(&mut h, trip);
    drop_files(&mut h, &files);
    assert!(crate::file_drop::pending(&h.app).is_some(), "the album they go to");
    // (not `show`: it waits for the import to end)
    h.app.run("library.source", json!({"kind": "album", "id": other})).unwrap();
    h.step();
    assert!(h.app.import.is_some(), "still importing");
    assert!(crate::file_drop::pending(&h.app).is_none(), "another album");
    h.app.run("library.source", json!({"kind": "all"})).unwrap();
    h.step();
    assert!(h.app.import.is_some(), "still importing");
    assert!(crate::file_drop::pending(&h.app).is_some(), "the whole library");
    finish(&mut h);
    assert!(crate::file_drop::pending(&h.app).is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_import_that_is_not_a_drop_has_no_tiles() {
    let (mut h, files, dir) = setup("picked", 3, 1);
    crate::import::start_paths(&mut h.app, files).unwrap();
    h.step();
    assert!(h.app.import.as_ref().is_some_and(|t| t.pending().is_none()));
    assert!(crate::file_drop::pending(&h.app).is_none());
    finish(&mut h);
    assert!(!has(&h, "pending:0"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_dropped_folder_gets_its_tiles_once_it_is_read() {
    let (mut h, files, dir) = setup("folder", 5, 40);
    drop_files(&mut h, &[dir.to_string_lossy().to_string()]);
    let pending = h.app.import.as_ref().and_then(|t| t.pending()).expect("a drop has tiles").clone();
    // (nothing is known of what is inside until the worker has read it)
    assert_eq!(pending.len(), 0);
    let mut listed = false;
    let t0 = Instant::now();
    while h.app.import.is_some() && t0.elapsed() < T {
        h.step();
        listed |= pending.len() == files.len();
    }
    assert!(listed, "the folder's files got their tiles");
    assert_eq!(h.app.session.catalog.len(), files.len());
    assert!(pending.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}
