//! Headless tests of the Map view: photos with a position become markers, tiles from the host are
//! drawn (and credited), the built-in map stands in without a tile server, and searching by place
//! narrows the map like it narrows the grid.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use lightcraft_engine::Session;
use lightcraft_engine::tiles::{Tile, TileResult};
use serde_json::json;

use crate::headless::Headless;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(30);
const SETTLE: Duration = Duration::from_secs(120);

/// The demo library with some photos taken in Madrid (two near each other, one in a suburb), Paris
/// and Sydney.
fn library() -> (Session, Vec<u64>) {
    let mut s = Session::with_demo();
    let ids: Vec<u64> = s.visible_cloned().iter().map(|i| i.0).collect();
    for (i, gps) in ["40.4169, -3.7035", "40.4170, -3.7036", "40.5400, -3.6400", "48.8584, 2.2945", "-33.8568, 151.2153"].iter().enumerate() {
        s.execute("photo.setMeta", &json!({"ids": [ids[i]], "gps": gps})).unwrap();
    }
    (s, ids)
}

/// A tile server that answers at once with a checkerboard.
fn checker_exec(calls: Arc<AtomicUsize>, offline_calls: Arc<AtomicUsize>) -> crate::panels::map::TileExec {
    Box::new(move |req, tx, ctx| {
        calls.fetch_add(1, Ordering::SeqCst);
        if !req.online {
            offline_calls.fetch_add(1, Ordering::SeqCst);
        }
        let mut rgba = Vec::with_capacity(256 * 256 * 4);
        for y in 0..256u32 {
            for x in 0..256u32 {
                let dark = (x / 32 + y / 32 + req.id.x + req.id.y) % 2 == 0;
                rgba.extend_from_slice(if dark { &[196, 220, 170, 255] } else { &[226, 238, 208, 255] });
            }
        }
        let _ = tx.send(TileResult { id: req.id, tile: Ok(Tile { width: 256, height: 256, rgba }) });
        ctx.request_repaint();
    })
}

fn map(services: Services, ui: serde_json::Value) -> (Headless, Vec<u64>) {
    let (session, ids) = library();
    let mut h = Headless::new(LightcraftApp::new(session, Services { png: None, ..services }), [1200.0, 800.0], 1.0);
    let r = h.request("ui.set", ui, T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    (h, ids)
}

fn widget(h: &Headless, id: &str) -> Option<egui::Rect> {
    h.app.widgets.iter().find(|(w, _)| w == id).map(|(_, r)| *r)
}

fn markers(h: &Headless) -> Vec<String> {
    h.app.widgets.iter().filter(|(w, _)| w.starts_with("marker:")).map(|(w, _)| w.clone()).collect()
}

#[test]
fn photos_with_a_position_are_markers() {
    let (mut h, _) = map(Services::default(), json!({"view": "map"}));
    assert!(widget(&h, "map").is_some(), "the map is drawn");
    let m = h.app.map.stats.markers_drawn;
    assert!(m >= 1, "{m} markers");
    // the view fits all the photos: Sydney to Madrid is nearly the whole world
    let (_, _, zoom) = h.app.map.camera().unwrap();
    assert!(zoom < 4.0, "all photos in view: zoom {zoom}");
    // zoomed to Madrid: its photos separate from Paris and Sydney
    let r = h.request("engine.execute", json!({"command": "view.map", "params": {"lat": 40.42, "lon": -3.70, "zoom": 15}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    let near = markers(&h);
    assert_eq!(h.app.map.stats.markers_drawn, near.len());
    assert!(!near.is_empty() && near.len() <= 2, "the two central photos: {near:?}");
}

#[test]
fn clicking_a_marker_selects_its_photo() {
    let (mut h, ids) = map(Services::default(), json!({"view": "map"}));
    let r = h.request("engine.execute", json!({"command": "view.map", "params": {"lat": 48.8584, "lon": 2.2945, "zoom": 14}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert_eq!(markers(&h), vec![format!("marker:{}", ids[3])], "Paris only");
    let r = h.request("ui.clickWidget", json!({"id": format!("marker:{}", ids[3])}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert_eq!(h.app.session.selection.active.map(|i| i.0), Some(ids[3]));
}

#[test]
fn searching_by_place_narrows_the_map() {
    let (mut h, ids) = map(Services::default(), json!({"view": "map"}));
    let all = h.app.map.located_points().len();
    assert_eq!(all, 5);
    let r = h.request("engine.execute", json!({"command": "library.filter", "params": {"text": "photos in madrid"}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    let ps: Vec<u64> = h.app.map.located_points().iter().map(|p| p.id).collect();
    assert_eq!(ps.len(), 3, "{ps:?}");
    assert!(ps.contains(&ids[0]) && ps.contains(&ids[2]) && !ps.contains(&ids[3]));
    // and it zoomed to them: Madrid is a city, not a continent
    let (_, _, zoom) = h.app.map.camera().unwrap();
    assert!(zoom > 8.0, "fitted to Madrid's photos: zoom {zoom}");
    // the chip says what "photos in madrid" was understood as
    let chips = lightcraft_engine::filter_chips(&h.app.session.filter, &h.app.session.catalog);
    let chips = crate::panels::chips::with_understanding(&mut h.app, chips);
    assert!(chips[0].label.contains("Madrid, Spain"), "{}", chips[0].label);
}

#[test]
fn tiles_from_the_host_are_drawn_and_credited() {
    let calls = Arc::new(AtomicUsize::new(0));
    let offline = Arc::new(AtomicUsize::new(0));
    let services = Services { tile_exec: Some(checker_exec(calls.clone(), offline.clone())), ..Default::default() };
    let (mut h, _) = map(services, json!({"view": "map"}));
    for _ in 0..10 {
        h.step();
    }
    assert!(calls.load(Ordering::SeqCst) > 0, "tiles were asked for");
    assert_eq!(offline.load(Ordering::SeqCst), 0, "online by default");
    assert!(h.app.map.stats.tiles_drawn > 0, "{:?}", h.app.map.stats);
    assert!(widget(&h, "map:attribution").is_some(), "the map says whose tiles these are");
    // turning the network off still asks the host (it can answer from its cache), but says so
    h.app.ui.settings.map_online = false;
    let r = h.request("engine.execute", json!({"command": "view.map", "params": {"lat": 10.0, "lon": 10.0, "zoom": 6}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert!(offline.load(Ordering::SeqCst) > 0, "offline requests are marked");
}

#[test]
fn without_a_tile_server_the_built_in_map_is_enough() {
    let (mut h, _) = map(Services::default(), json!({"view": "map"}));
    for _ in 0..5 {
        h.step();
    }
    assert_eq!(h.app.map.stats.tiles_drawn, 0);
    assert_eq!(h.app.map.stats.tiles_requested, 0);
    assert!(widget(&h, "map:attribution").is_some());
    assert!(!markers(&h).is_empty(), "the photos are still on it");
}

#[test]
fn an_empty_map_says_why() {
    let mut h = Headless::new(LightcraftApp::new(Session::with_demo(), Services { png: None, ..Default::default() }), [1200.0, 800.0], 1.0);
    h.request("ui.set", json!({"view": "map"}), T);
    h.settle(SETTLE);
    assert!(widget(&h, "map").is_some());
    assert!(markers(&h).is_empty());
    assert_eq!(h.app.map.located_points().len(), 0);
}

#[test]
fn the_map_commands_validate_their_input() {
    let (mut h, _) = map(Services::default(), json!({"view": "map"}));
    for p in [json!({"lat": 91.0, "lon": 0.0}), json!({"lat": 0.0, "lon": 400.0}), json!({"place": "zzzzqq"})] {
        let r = h.request("engine.execute", json!({"command": "view.map", "params": p}), T);
        assert_eq!(r["ok"], false, "{p}: {r}");
    }
    let r = h.request("engine.execute", json!({"command": "view.map", "params": {"place": "sydney"}}), T);
    assert_eq!(r["ok"], true, "{r}");
    let (lat, _, _) = h.app.map.camera().unwrap();
    assert!(lat < -30.0, "looking at Sydney: {lat}");
    // wild gestures can't break the view
    h.request("ui.scroll", json!({"x": 600.0, "y": 400.0, "dy": 1e9}), T);
    h.request("ui.scroll", json!({"x": 600.0, "y": 400.0, "dy": -1e9}), T);
    h.step();
    let (lat, lon, zoom) = h.app.map.camera().unwrap();
    assert!(lat.is_finite() && lon.is_finite() && (0.0..=19.0).contains(&zoom), "{lat} {lon} {zoom}");
}

#[test]
fn the_map_fits_a_phone() {
    let (session, _) = library();
    let mut h = Headless::new(LightcraftApp::new(session, Services { png: None, ..Default::default() }), [390.0, 844.0], 1.0);
    h.request("ui.set", json!({"view": "map"}), T);
    h.settle(SETTLE);
    assert!(h.app.compact);
    let map = widget(&h, "map").expect("the map is drawn");
    assert!(map.left() >= 0.0 && map.right() <= 390.0 + 0.5 && map.height() > 500.0, "{map:?}");
    let search = widget(&h, "map:search").unwrap();
    assert!(search.left() >= 0.0 && search.right() <= 390.0, "the place search fits the screen: {search:?}");
    assert!(h.app.map.stats.markers_drawn >= 1);
    // the zoom buttons are inside the map and a thumb can reach them
    for id in ["map:zoomIn", "map:zoomOut", "map:fit"] {
        let b = widget(&h, id).unwrap_or_else(|| panic!("no {id}"));
        assert!(map.contains_rect(b), "{id} {b:?} outside {map:?}");
    }
}

/// Pictures of the Map for a person to look at:
/// `LC_MAP_SHOTS=/some/dir cargo test -p lightcraft-ui-egui --lib -- --ignored map_screenshots`
#[test]
#[ignore = "writes PNGs for a human to look at"]
fn map_screenshots() {
    let Some(dir) = std::env::var_os("LC_MAP_SHOTS").map(std::path::PathBuf::from) else { return };
    std::fs::create_dir_all(&dir).unwrap();
    let (mut session, ids) = library();
    // a few more places: a trip around Iberia and a weekend in Lisbon
    for (i, gps) in [
        "41.3874, 2.1686",
        "41.3880, 2.1690",
        "37.3891, -5.9845",
        "38.7223, -9.1393",
        "38.7230, -9.1400",
        "38.7210, -9.1380",
        "43.2630, -2.9350",
        "39.4699, -0.3763",
    ]
    .iter()
    .enumerate()
    {
        if let Some(id) = ids.get(5 + i) {
            session.execute("photo.setMeta", &json!({"ids": [id], "gps": gps})).unwrap();
        }
    }
    let calls = Arc::new(AtomicUsize::new(0));
    let tile_exec = std::env::var_os("LC_MAP_NOTILES").is_none().then(|| checker_exec(calls.clone(), Arc::new(AtomicUsize::new(0))));
    let services = Services { png: None, tile_exec, ..Default::default() };
    let mut h = Headless::new(LightcraftApp::new(session, services), [1200.0, 800.0], 1.0);
    let shot = |h: &mut Headless, name: &str| {
        let img = h.snapshot(SETTLE);
        let mut px = lightcraft_raster::Rgba8::new(img.size[0], img.size[1]);
        for (o, c) in px.data.iter_mut().zip(&img.pixels) {
            *o = c.to_array();
        }
        let png = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&px), &lightcraft_codecs::EncodeMeta::default()).unwrap();
        std::fs::write(dir.join(name), png).unwrap();
    };
    h.request("ui.set", json!({"view": "map"}), T);
    for _ in 0..8 {
        h.step();
    }
    shot(&mut h, "map-fit-all.png");
    h.request("engine.execute", json!({"command": "view.map", "params": {"place": "portugal"}}), T);
    for _ in 0..8 {
        h.step();
    }
    shot(&mut h, "map-portugal.png");
    h.request("engine.execute", json!({"command": "view.map", "params": {"lat": 38.7223, "lon": -9.1393, "zoom": 16}}), T);
    for _ in 0..8 {
        h.step();
    }
    shot(&mut h, "map-lisbon-street.png");
    h.request("engine.execute", json!({"command": "library.filter", "params": {"text": "photos in spain in june"}}), T);
    for _ in 0..10 {
        h.step();
    }
    shot(&mut h, "map-search-spain.png");
    // and on a phone
    let (mut session, ids) = library();
    session.execute("photo.setMeta", &json!({"ids": [ids[5]], "gps": "41.3874, 2.1686"})).unwrap();
    let services = Services { png: None, tile_exec: Some(checker_exec(calls, Arc::new(AtomicUsize::new(0)))), ..Default::default() };
    let mut phone = Headless::new(LightcraftApp::new(session, services), [390.0, 844.0], 2.0);
    phone.request("ui.set", json!({"view": "map"}), T);
    phone.request("engine.execute", json!({"command": "view.map", "params": {"place": "spain"}}), T);
    for _ in 0..8 {
        phone.step();
    }
    shot(&mut phone, "map-phone.png");
}
