//! People on a phone: the People row in the albums list, round faces with names, naming a face from
//! the People view or by tapping it on the photo, marking a face by hand, renaming a person, and
//! confirming the faces that look like them.

use std::time::Duration;

use lightcraft_catalog::PhotoId;
use lightcraft_engine::Session;
use serde_json::json;

use crate::headless::Headless;
use crate::state::{RightPanel, ViewMode};
use crate::tests_grid_touch::{click, has, rect_of, snap};
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(30);
const SETTLE: Duration = Duration::from_secs(120);

fn phone() -> Headless {
    let mut app = LightcraftApp::new(Session::with_demo(), Services { png: None, ..Default::default() });
    app.ui.view = ViewMode::SquareGrid;
    let mut h = Headless::new(app, [390.0, 844.0], 1.0);
    h.settle(SETTLE);
    h
}

fn run(h: &mut Headless, cmd: &str, params: serde_json::Value) -> serde_json::Value {
    let r = h.request("engine.execute", json!({"command": cmd, "params": params}), T);
    assert_eq!(r["ok"], true, "{cmd}: {r}");
    h.settle(SETTLE);
    r
}

fn type_text(h: &mut Headless, text: &str) {
    let r = h.request("ui.text", json!({"text": text}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
}

fn regions(h: &Headless, id: u64) -> Vec<lightcraft_meta::Region> {
    h.app.session.catalog.photo(PhotoId(id)).unwrap().meta.regions.clone()
}

/// The albums list has a People row (desktop and phone): on a phone it opens People as a screen of
/// its own with a back button, and a face's name is given from there.
#[test]
fn people_opens_from_the_albums_list_and_names_a_face() {
    let mut h = phone();
    let ids: Vec<u64> = h.app.session.visible().iter().take(2).map(|i| i.0).collect();
    run(&mut h, "photo.addRegion", json!({"id": ids[0], "rect": {"x0": 0.3, "y0": 0.2, "x1": 0.5, "y1": 0.5}, "name": "Ana"}));
    run(&mut h, "photo.addRegion", json!({"id": ids[1], "rect": {"x0": 0.3, "y0": 0.2, "x1": 0.5, "y1": 0.5}}));
    click(&mut h, "button:collections");
    assert!(has(&h, "source:people"), "a People row in the albums list");
    click(&mut h, "source:people");
    assert_eq!(h.app.ui.view, ViewMode::People);
    assert!(!h.app.ui.left_panel, "the albums page closed");
    assert!(has(&h, "person:Ana") && has(&h, "people:title") && has(&h, "icon:back"));
    snap(&h, "people-phone");
    // the unnamed face: tap it, type a name, Done
    let face = format!("unnamed-face:{}:0", ids[1]);
    assert!(has(&h, &face), "{:?}", h.app.widgets.iter().map(|(w, _)| w).collect::<Vec<_>>());
    click(&mut h, &face);
    assert!(has(&h, "sheet:faceName") && has(&h, "field:faceNamePhone"));
    assert!(has(&h, "button:faceNamePick-0"), "the people already named are offered");
    snap(&h, "people-naming");
    type_text(&mut h, "Bo");
    click(&mut h, "button:sheetOk");
    assert_eq!(regions(&h, ids[1])[0].name.as_deref(), Some("Bo"));
    assert!(h.app.ui.face_naming.is_none());
    assert!(has(&h, "person:Bo"));
    // a person's page: rename them on every photo at once
    click(&mut h, "person:Ana");
    assert_eq!(h.app.ui.person_page.as_deref(), Some("Ana"));
    snap(&h, "people-person");
    click(&mut h, "person:rename");
    assert_eq!(h.app.ui.face_naming.as_ref().map(|n| n.text.as_str()), Some("Ana"), "the page starts with the name");
    // (what the field holds, replaced)
    h.app.ui.face_naming.as_mut().unwrap().text = "Ana Lima".into();
    h.settle(SETTLE);
    click(&mut h, "button:sheetOk");
    assert_eq!(regions(&h, ids[0])[0].name.as_deref(), Some("Ana Lima"));
    assert_eq!(h.app.ui.person_page.as_deref(), Some("Ana Lima"), "the page follows the new name");
    // back: to People, then to the grid
    click(&mut h, "icon:back");
    assert_eq!((h.app.ui.view, h.app.ui.person_page.clone()), (ViewMode::People, None));
    click(&mut h, "icon:back");
    assert_eq!(h.app.ui.view, ViewMode::SquareGrid);
}

/// In the photo editor a tap on a face names it, and "Tag a Face" marks one the detector missed.
#[test]
fn a_face_on_the_photo_is_named_by_tapping_it_and_one_is_marked_by_hand() {
    let mut h = phone();
    let id = h.app.session.visible()[0].0;
    run(&mut h, "photo.addRegion", json!({"id": id, "rect": {"x0": 0.3, "y0": 0.2, "x1": 0.5, "y1": 0.5}}));
    run(&mut h, "library.select", json!({"ids": [id]}));
    run(&mut h, "view.detail", json!({}));
    h.app.ui.right = RightPanel::None;
    h.settle(SETTLE);
    assert!(has(&h, "face:0"), "the face is outlined on the photo");
    snap(&h, "loupe-faces");
    click(&mut h, "face:0");
    assert!(has(&h, "sheet:faceName"));
    type_text(&mut h, "Cy");
    let r = h.request("ui.key", json!({"key": "Enter"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert_eq!(regions(&h, id)[0].name.as_deref(), Some("Cy"), "Return names it");
    // Tag a Face: a tap on the photo marks a face there, then it is named
    click(&mut h, "icon:photoMore");
    click(&mut h, "button:tagFace");
    assert!(h.app.ui.tag_face && has(&h, "hint:tagFace"));
    let r = h.request("ui.pointer", json!({"events": [{"kind": "down", "x": 0.7, "y": 0.6}, {"kind": "up", "x": 0.7, "y": 0.6}]}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert!(!h.app.ui.tag_face && has(&h, "sheet:faceName"), "a tap marked it and asks who it is");
    type_text(&mut h, "Dee");
    click(&mut h, "button:sheetOk");
    let all = regions(&h, id);
    assert_eq!(all.len(), 2);
    let new = &all[1];
    assert_eq!(new.name.as_deref(), Some("Dee"));
    let (cx, cy) = ((new.rect.x0 + new.rect.x1) / 2.0, (new.rect.y0 + new.rect.y1) / 2.0);
    assert!((cx - 0.7).abs() < 0.03 && (cy - 0.6).abs() < 0.03, "where the finger was: {cx}, {cy}");
    assert!(has(&h, "face:1"));
    // a box that isn't a face is removed from the same page
    click(&mut h, "face:1");
    click(&mut h, "button:removeFace");
    assert_eq!(regions(&h, id).len(), 1);
}

/// While the editor is open with no tool in hand, `rect_of` is enough to see the faces' places.
#[test]
fn the_face_frames_follow_the_photo() {
    let mut h = phone();
    let id = h.app.session.visible()[0].0;
    run(&mut h, "photo.addRegion", json!({"id": id, "rect": {"x0": 0.0, "y0": 0.0, "x1": 0.5, "y1": 0.5}}));
    run(&mut h, "library.select", json!({"ids": [id]}));
    run(&mut h, "view.detail", json!({}));
    let img = h.app.image_rect.unwrap();
    let face = rect_of(&h, "face:0");
    assert!((face.left() - img.left()).abs() < 1.5 && (face.top() - img.top()).abs() < 1.5, "{face:?} {img:?}");
    assert!((face.width() - img.width() / 2.0).abs() < 1.5);
    // review mode (the bars away) shows the photo alone
    run(&mut h, "view.reviewMode", json!({"on": true}));
    assert!(!has(&h, "face:0"));
}

/// The unnamed faces of one person are one round face (the face finder grouped them, here or on the
/// server): a tap and a name names every one of them.
#[test]
fn a_persons_unnamed_faces_are_one_face_and_named_together() {
    let mut h = phone();
    h.app.session.vision.set_face_finder(std::sync::Arc::new(lightcraft_vision::fake::FakeFaces));
    h.app.session.vision.text_edge = 192;
    run(&mut h, "vision.setFaces", json!({"on": true}));
    run(&mut h, "vision.index", json!({"wait": true}));
    // face boxes where the stand-in finds faces (the two halves of a photo), as Detect Faces would add them
    let ids: Vec<u64> = h.app.session.catalog.photos().map(|p| p.id.0).collect();
    for id in ids {
        for (x0, x1) in [(0.1, 0.45), (0.55, 0.9)] {
            h.app.run("photo.addRegion", json!({"id": id, "rect": {"x0": x0, "y0": 0.2, "x1": x1, "y1": 0.8}})).unwrap();
        }
    }
    run(&mut h, "view.people", json!({}));
    let r = h.app.session.execute("faces.unnamed", &json!({})).unwrap();
    let first: Vec<(u64, u64)> = r["faces"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["group"] == 0)
        .map(|f| (f["photo"].as_u64().unwrap(), f["index"].as_u64().unwrap()))
        .collect();
    assert!(first.len() > 1, "{r}");
    let face = |(photo, index): (u64, u64)| format!("unnamed-face:{photo}:{index}");
    assert!(first[1..].iter().all(|f| !has(&h, &face(*f))), "one face for the person");
    // (below the found people: scroll down to it)
    for _ in 0..20 {
        if has(&h, &face(first[0])) {
            break;
        }
        h.request("ui.move", json!({"x": 195.0, "y": 500.0}), T);
        h.request("ui.scroll", json!({"dy": -250.0}), T);
        h.settle(SETTLE);
    }
    snap(&h, "people-phone-grouped");
    click(&mut h, &face(first[0]));
    assert!(has(&h, "sheet:faceName"));
    assert!(!has(&h, "button:removeFace"), "removing is for one face, not a person");
    type_text(&mut h, "Dee");
    click(&mut h, "button:sheetOk");
    for (photo, index) in &first {
        assert_eq!(regions(&h, *photo)[*index as usize].name.as_deref(), Some("Dee"), "every face of theirs is named");
    }
    assert!(has(&h, "person:Dee"));
}
