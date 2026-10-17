//! What a develop edit invalidates: a slider tick changes one photo's develop settings, so the
//! library-wide grid query only runs again when the current filter, sort or smart album reads
//! develop state (see `lightcraft_catalog::revision`).

use lightcraft_catalog::{Album, AlbumId, Filter, Op, Photo, PhotoId, RuleSet, Sort, SortKey, Source};
use serde_json::json;

use crate::{LibrarySource, Session};

/// A library of `n` photos with capture dates and a keyword each.
fn library(n: u64) -> Session {
    let mut s = Session::new();
    let ops = (0..n)
        .map(|i| {
            let mut p = Photo::new(
                PhotoId(i + 1),
                Source::Demo { scene: (i % 20) as u32 },
                &format!("IMG_{i:06}.jpg"),
                "JPEG",
                6000,
                4000,
                "2026-01-01T00:00:00",
            );
            p.captured = Some(format!("20{:02}-{:02}-{:02}T10:00:00", 10 + i % 16, 1 + i % 12, 1 + i % 28));
            p.meta.keywords = vec![format!("kw{}", i % 300)];
            Op::AddPhoto { photo: Box::new(p) }
        })
        .collect();
    s.commit("Add", Op::Batch { ops }).unwrap();
    s
}

/// Change photo `id`'s exposure (a committed edit, or a slider tick inside an interaction).
fn expose(s: &mut Session, id: PhotoId, ev: f64) {
    let mut d = (*s.develop_of(id).unwrap()).clone();
    d.light.exposure = ev;
    s.set_develop(id, d, "Exposure").unwrap();
}

fn smart(s: &mut Session, id: u64, rules: serde_json::Value) {
    let mut a = Album::new(AlbumId(id), format!("Smart {id}"));
    let rules: RuleSet = serde_json::from_value(rules).unwrap();
    a.smart = Some(Box::new(Filter { rule_set: Some(rules), ..Default::default() }));
    s.commit("New Smart Album", Op::AddAlbum { album: a }).unwrap();
}

/// Slider ticks and committed develop edits leave the grid's list as it is (same generation: the
/// query didn't run) when nothing in view reads develop state; a library change still redoes it.
#[test]
fn develop_edits_dont_redo_the_library_query() {
    let mut s = library(50);
    let (generation, _) = s.visible_shared();
    let total = s.source_total();
    let id = s.visible()[7];
    s.execute("library.select", &json!({"ids": [id.0]})).unwrap();
    s.execute("develop.beginInteraction", &json!({"label": "Exposure"})).unwrap();
    for i in 0..10 {
        s.execute("develop.set", &json!({"control": "light.exposure", "value": i as f64 / 10.0})).unwrap();
        assert_eq!(s.visible_shared().0, generation, "tick {i}");
    }
    s.execute("develop.endInteraction", &json!({})).unwrap();
    expose(&mut s, id, 2.0);
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(s.visible_shared().0, generation, "a committed edit and its undo");
    assert_eq!(s.source_total(), total);
    // a library change does
    s.execute("photo.rate", &json!({"rating": 4})).unwrap();
    assert_ne!(s.visible_shared().0, generation);
}

/// Filters, sorts and smart albums that read develop state follow every develop edit, slider
/// ticks included.
#[test]
fn views_that_read_develop_state_follow_develop_edits() {
    let mut s = library(20);
    let ids = s.visible().to_vec();
    let (a, b) = (ids[3], ids[11]);
    s.execute("library.select", &json!({"ids": [a.0]})).unwrap();

    // the filter bar's Edited, during a drag
    s.filter = Filter { edited: Some(true), ..Default::default() };
    assert!(s.visible().is_empty());
    s.execute("develop.beginInteraction", &json!({"label": "Exposure"})).unwrap();
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 0.5})).unwrap();
    assert_eq!(s.visible(), [a], "a slider tick makes the photo edited");
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 0.0})).unwrap();
    assert!(s.visible().is_empty(), "and back");
    s.execute("develop.endInteraction", &json!({})).unwrap();
    // a search token
    s.filter = Filter { text: "edited:yes".into(), ..Default::default() };
    expose(&mut s, b, 1.0);
    assert_eq!(s.visible(), [b]);

    // Edit Date: the photo edited last comes first
    s.filter = Filter::default();
    s.sort = Sort { key: SortKey::EditDate, ascending: false, ..Default::default() };
    s.clock = Box::new(|| "2100-05-01T10:00:00".to_string());
    expose(&mut s, a, 0.3);
    assert_eq!(s.visible()[0], a);
    s.clock = Box::new(|| "2100-05-01T11:00:00".to_string());
    expose(&mut s, b, 0.7);
    assert_eq!(s.visible()[0], b);

    // a smart album testing Cropped, and one testing it through an Album rule
    s.sort = Sort::default();
    smart(&mut s, 100, json!({"rules": [{"field": "cropped", "op": "is", "value": true}]}));
    smart(&mut s, 101, json!({"rules": [{"field": "album", "op": "is", "value": 100}]}));
    for album in [100, 101] {
        s.source = LibrarySource::Album(AlbumId(album));
        assert!(s.visible().is_empty());
        assert_eq!(s.source_total(), Some(0));
    }
    let mut d = (*s.develop_of(a).unwrap()).clone();
    d.crop.geometry.rect = lightcraft_geom::Rect::new(0.1, 0.1, 0.9, 0.9);
    s.set_develop(a, d, "Crop").unwrap();
    for album in [100, 101] {
        s.source = LibrarySource::Album(AlbumId(album));
        assert_eq!(s.visible(), [a], "album {album}");
        assert_eq!(s.source_total(), Some(1), "album {album}");
    }
    // and the size rules (a crop changes the shown shape)
    s.source = LibrarySource::All;
    let square: RuleSet = serde_json::from_value(json!({"rules": [{"field": "aspect", "op": "is", "value": "square"}]})).unwrap();
    s.filter = Filter { rule_set: Some(square), ..Default::default() };
    assert!(s.visible().is_empty());
    let mut d = (*s.develop_of(b).unwrap()).clone();
    d.crop.geometry.rect = lightcraft_geom::Rect::new(0.0, 0.0, 4000.0 / 6000.0, 1.0);
    s.set_develop(b, d, "Crop").unwrap();
    assert_eq!(s.visible(), [b]);
}

/// Per-tick cost of a slider drag on a big library (the develop op plus the grid query the
/// filmstrip asks for every frame):
/// `cargo test --release -p lightcraft-engine -- --ignored drag_tick_100k --nocapture`.
#[test]
#[ignore]
fn drag_tick_100k() {
    let mut s = library(100_000);
    let id = s.visible()[50_000];
    s.execute("library.select", &json!({"ids": [id.0]})).unwrap();
    s.execute("develop.beginInteraction", &json!({"label": "Exposure"})).unwrap();
    let n = 60;
    let tick = |s: &mut Session, name: &str| {
        s.visible();
        let t = std::time::Instant::now();
        for i in 0..n {
            s.execute("develop.set", &json!({"control": "light.exposure", "value": i as f64 / 100.0})).unwrap();
            s.visible();
        }
        let ms = t.elapsed().as_secs_f64() * 1e3 / n as f64;
        eprintln!("slider tick at 100k photos, {name}: {ms:.3} ms (develop.set + visible)");
    };
    tick(&mut s, "warm-up");
    tick(&mut s, "by capture date");
    s.sort = Sort { key: SortKey::FileName, ..Default::default() };
    tick(&mut s, "by file name");
    // a view that reads develop state still asks again on every tick
    s.sort = Sort::default();
    s.filter = Filter { edited: Some(false), ..Default::default() };
    tick(&mut s, "Unedited filter");
    s.execute("develop.endInteraction", &json!({})).unwrap();
}
