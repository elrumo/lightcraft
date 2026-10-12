//! Remove tool commands: add (automatic source), select, move target/source, edit, refresh, delete.

use serde_json::json;

use crate::Session;

fn spots(s: &Session) -> Vec<lightcraft_develop::Spot> {
    s.develop_of(s.active().unwrap()).unwrap().spots.clone()
}

#[test]
fn spots_select_move_edit_refresh_and_delete() {
    let mut s = Session::with_demo();
    // a remove spot gets a resolved source (a pin to show) and is selected
    let r = s.execute("spot.add", &json!({"points": [[0.4, 0.5]], "size": 0.02})).unwrap();
    assert_eq!(r["index"], 0);
    assert_eq!(s.active_spot, Some(0));
    let src0 = spots(&s)[0].source_offset.expect("automatic source resolved");
    assert!(src0.x.hypot(src0.y) > 0.02, "the source is outside the spot: {src0:?}");
    s.execute("spot.add", &json!({"mode": "clone", "points": [[0.7, 0.3]], "source": [0.1, 0.0]})).unwrap();
    assert_eq!(s.active_spot, Some(1));
    assert_eq!(s.execute("library.state", &json!({})).unwrap()["activeSpot"], 1);
    // select
    s.execute("spot.select", &json!({"index": 0})).unwrap();
    assert_eq!(s.active_spot, Some(0));
    assert!(s.execute("spot.select", &json!({"index": 9})).is_err());
    // move the target (the source follows: its offset is relative), then the source alone
    s.execute("spot.update", &json!({"move": [0.05, -0.1]})).unwrap();
    let sp = &spots(&s)[0];
    assert!((sp.points[0].x - 0.45).abs() < 1e-9 && (sp.points[0].y - 0.4).abs() < 1e-9);
    assert_eq!(sp.source_offset, Some(src0));
    s.execute("spot.update", &json!({"moveSource": [0.01, 0.02]})).unwrap();
    let o = spots(&s)[0].source_offset.unwrap();
    assert!((o.x - src0.x - 0.01).abs() < 1e-9 && (o.y - src0.y - 0.02).abs() < 1e-9);
    // size / feather / opacity of the selected spot (clamped), one undo step each
    s.execute("spot.update", &json!({"size": 0.03, "feather": 20, "opacity": 150})).unwrap();
    let sp = &spots(&s)[0];
    assert_eq!((sp.size, sp.feather, sp.opacity), (0.03, 20.0, 100.0));
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(spots(&s)[0].size, 0.02);
    // refresh: a different source, at least a spot radius away
    let before = spots(&s)[0].source_offset.unwrap();
    let r = s.execute("spot.refreshSource", &json!({})).unwrap();
    let after = spots(&s)[0].source_offset.unwrap();
    assert!((after.x - before.x).hypot(after.y - before.y) > 0.01, "{r}");
    // delete the selected spot: the selection clears; deleting before a selected one shifts it
    s.execute("spot.delete", &json!({})).unwrap();
    assert_eq!((spots(&s).len(), s.active_spot), (1, None));
    assert_eq!(spots(&s)[0].mode, lightcraft_develop::SpotMode::Clone);
    assert!(s.execute("spot.delete", &json!({})).is_err(), "nothing selected");
    s.execute("spot.add", &json!({"points": [[0.2, 0.2]], "source": [0.05, 0.0]})).unwrap();
    s.execute("spot.delete", &json!({"index": 0})).unwrap();
    assert_eq!(s.active_spot, Some(0));
    // another photo: no selection
    s.execute("library.next", &json!({})).unwrap();
    assert_eq!(s.active_spot, None);
}

/// AI Remove without its model says so (the apps offer the download on that message) and adds
/// nothing; a model that isn't the pinned file is refused.
#[test]
fn ai_remove_without_its_model_says_so_and_adds_nothing() {
    let mut s = Session::with_demo();
    let e = s.execute("spot.add", &json!({"points": [[0.4, 0.5]], "ai": true})).unwrap_err().to_string();
    assert!(e.contains(crate::erase::NOT_INSTALLED), "{e}");
    assert!(spots(&s).is_empty());
    assert!(!s.eraser_ready());
    assert!(s.execute("spot.add", &json!({"mode": "heal", "points": [[0.4, 0.5]], "ai": true})).is_err(), "AI fills Remove spots only");
    assert!(s.install_eraser(b"not the model").is_err());
    s.execute("spot.add", &json!({"mode": "clone", "points": [[0.4, 0.5]], "source": [0.1, 0.0]})).unwrap();
    assert!(s.execute("spot.erase", &json!({"index": 0})).is_err(), "a Clone spot");
}

/// With the model (when a copy is at `$LIGHTCRAFT_MIGAN`; it is never part of the repository) an
/// AI spot carries its fill, one undo step; moving the spot moves the fill until it is redone.
#[test]
fn ai_remove_stores_the_models_fill_with_the_spot() {
    let Some(path) = std::env::var_os("LIGHTCRAFT_MIGAN") else { return };
    let mut s = Session::with_demo();
    s.install_eraser(&std::fs::read(path).unwrap()).unwrap();
    assert!(s.eraser_ready());
    let r = s.execute("spot.add", &json!({"points": [[0.4, 0.5], [0.45, 0.52]], "size": 0.03, "ai": true})).unwrap();
    assert_eq!(r["index"], 0);
    let sp = &spots(&s)[0];
    let f = sp.fill.as_ref().expect("a fill");
    let [x0, y0, x1, y1] = f.bounds().unwrap();
    assert!(x0 < 0.37 && x1 > 0.48 && y0 < 0.47 && y1 > 0.55, "the fill covers the stroke: {:?}", f.rect);
    assert!(f.w.max(f.h) <= 512 && f.pixels().is_some());
    assert_eq!(sp.source_offset, None, "no source pin for an AI spot");
    // it renders without the model being asked again (the fill is stored), and exports the same
    s.execute("spot.update", &json!({"move": [0.1, 0.0]})).unwrap();
    let moved = spots(&s)[0].fill.clone().unwrap();
    assert!((moved.rect[0] - x0 - 0.1).abs() < 1e-9);
    s.execute("spot.erase", &json!({})).unwrap();
    let redone = spots(&s)[0].fill.clone().unwrap();
    assert_ne!(redone.data, moved.data, "painted again at the new place");
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(spots(&s)[0].fill.as_ref().map(|f| f.rect), Some(moved.rect));
}
